//! USB storage (G1, `docs/USB_STORAGE.md`): the computer opens the recordings
//! volume as a drive.
//!
//! Storage mode is stock's own gadget switch, `S89usbgadget uac_hid_msc`: the
//! gadget is unbound, adbd stopped and adb unlinked, `/dev/udisk` loaded into
//! the drive, the drive and HID linked next to the microphone and speaker
//! (`uac2.0` stays linked), and the gadget bound again. adb is off while it
//! lasts: this controller cannot run the drive next to adb (measured
//! 2026-10-13). The way back is stock's too: unbound (which ends the
//! computer's hold on the drive), the drive emptied and unlinked, a fresh
//! adbd, adb and HID linked, bound.
//!
//! The two sides never write the volume at once: the drive gets the partition
//! only once the volume is unmounted, and the volume is mounted again only
//! once the drive reads back empty. Neither the kernel's LUN open nor `mount`
//! would stop a double writer by itself.
//!
//! Nothing unbinds while waved has a file on the gadget's devices: before
//! every unbind the holds (`waved_state::GadgetHold`) are raised and every
//! user must confirm it closed its file within `HOLD_WAIT`, else USB is not
//! switched. Linux 5.4 `f_hid` frees `hid.0` under open files (a read, write
//! or poll on one touches freed kernel memory), and the f_uac2 unbind waits,
//! uninterruptibly, for every file on `hw:1` to close. `/dev/hidg0` stays
//! closed for the whole session; `hw:1` only around each unbind and bind.
//!
//! Device access sits behind `Gadget` and `Volume`; `device` has the configfs,
//! adbd and system implementations.

pub mod device;

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::{self, error::TryRecvError};
use tokio::sync::watch;
use tracing::{error, info, warn};
use waved_state::{
    Command, DeviceEvent, GadgetHold, MeetingState, OtaState, UsbStoragePhase, UsbStorageProblem,
    UsbStorageState,
};

/// The recordings partition (`mmcblk0p16`, exFAT).
pub const PARTITION: &str = "/dev/udisk";
/// Where the recordings partition is mounted on Wave.
pub const MOUNT_POINT: &str = "/mnt/udisk";
/// Polls in a row without a computer (one a second) that count as a pulled cable.
pub const GONE_POLLS: u32 = 3;
/// Between "Starting" and the unmount: an upload or playback that began just
/// before opens its file now, so the unmount sees it busy instead of pulling
/// the file away mid-open.
pub const SETTLE: Duration = Duration::from_secs(1);
/// After the switch to storage mode, the computer has this long to enumerate
/// Wave again (`configured`); otherwise Wave goes back to normal USB.
pub const ENUMERATE_WAIT: Duration = Duration::from_secs(10);
const ENUMERATE_POLL: Duration = Duration::from_millis(250);
/// Tries at bringing normal USB (adb included) back, `RESTORE_PAUSE` apart.
pub const RESTORE_TRIES: u32 = 3;
const RESTORE_PAUSE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(100);
/// Before an unbind, every user of `/dev/hidg0` and `hw:1` has this long to
/// confirm it closed its file (`GadgetHold`). Longer than the HID reader's
/// 1 s read plus its 200 ms look at the hold (waved-hid `node`).
pub const HOLD_WAIT: Duration = Duration::from_secs(3);
const HOLD_POLL: Duration = Duration::from_millis(50);

/// What the USB device controller says about the computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostLink {
    /// Enumerated and configured.
    Configured,
    /// The computer sleeps. It keeps its drive.
    Suspended,
    /// No computer: unplugged, or not (yet) enumerated.
    #[default]
    Gone,
}

impl HostLink {
    /// From the `state` files of every UDC (`/sys/class/udc/*/state`).
    pub fn from_states<'a>(states: impl IntoIterator<Item = &'a str>) -> Self {
        let mut link = HostLink::Gone;
        for state in states {
            match state.trim() {
                "configured" => return HostLink::Configured,
                "suspended" => link = HostLink::Suspended,
                _ => {}
            }
        }
        link
    }
}

/// What the USB gadget is doing (`Gadget::mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GadgetMode {
    /// Bound without the drive: Wave's normal USB (audio, HID, and adb on a
    /// unit that has it).
    Normal,
    /// The drive is linked (storage mode), or a waved left the gadget part
    /// way through a switch (its note is there and the gadget is not bound).
    Storage,
    /// Not bound, and no switch of waved's under way: the boot has not bound
    /// it yet (`S89usbgadget` binds after waved starts).
    Unbound,
    /// No gadget, or no mass-storage function in it.
    Unavailable,
}

/// How a filesystem is mounted: one `/proc/mounts` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    pub source: String,
    pub target: String,
    pub fstype: String,
    pub options: String,
}

impl MountSpec {
    /// How stock's `udisk_fix` mounts the recordings at boot (1.2.2.6).
    pub fn boot() -> Self {
        Self {
            source: PARTITION.into(),
            target: MOUNT_POINT.into(),
            fstype: "exfat".into(),
            options: "rw,fmask=0077".into(),
        }
    }
}

/// The USB gadget: its mode switch and the drive's medium.
pub trait Gadget: Send {
    fn mode(&self) -> GadgetMode;
    /// Stock's `uac_hid_msc`: note the normal mode, unbind, stop adbd, unlink
    /// adb, the serial port, the drive and HID, load `partition` into the
    /// drive, link the drive and HID, bind. Ok once the gadget reads bound.
    fn enter_storage(&mut self, partition: &str) -> io::Result<()>;
    /// Back to the normal mode noted on the way in (else Wave's boot mode):
    /// unbind (the computer's hold on the drive ends), stop adbd, unlink, empty
    /// the drive, start a fresh adbd, link what was linked, bind. Ok once the
    /// gadget reads bound. Also from part way through a switch.
    fn leave_storage(&mut self) -> io::Result<()>;
    /// The medium loaded in the drive (`lun.0/file`), `None` when empty.
    /// An error means there is no drive at all (no gadget yet).
    fn loaded(&self) -> io::Result<Option<String>>;
    /// Empty the drive. Fails with EBUSY while the computer holds the medium.
    fn unload(&mut self) -> io::Result<()>;
    fn host(&self) -> HostLink;
}

/// The recordings filesystem.
pub trait Volume: Send {
    fn sync(&mut self);
    /// How the recordings volume is mounted now; `None` when it is not.
    fn mounted(&self) -> io::Result<Option<MountSpec>>;
    fn unmount(&mut self) -> io::Result<()>;
    /// Check (and repair what is safe to repair) the unmounted filesystem on
    /// `source`. Returns a line for the log. `ErrorKind::TimedOut` means the
    /// check was stopped part way.
    fn check(&mut self, source: &str) -> io::Result<String>;
    fn mount(&mut self, spec: &MountSpec) -> io::Result<()>;
    /// Note, before unmounting, that waved takes the volume off Wave and how
    /// it was mounted. The note is on tmpfs: a waved that restarts before it
    /// mounted the volume again finds it and mounts it; after a reboot there
    /// is none and the boot mounts the volume.
    fn note_unmount(&mut self, spec: &MountSpec) -> io::Result<()>;
    /// The note a waved left, if any.
    fn unmount_noted(&self) -> Option<MountSpec>;
    /// The volume is mounted on Wave again: drop the note.
    fn clear_unmount_note(&mut self);
}

/// What `reclaim_at_start` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reclaim {
    /// Nothing to take back: USB was in its normal mode, and no waved left
    /// the volume unmounted.
    Nothing,
    /// USB was in storage mode and is back to normal, or a waved left the
    /// volume unmounted; the volume is mounted.
    Reclaimed,
    /// The volume could not be mounted (or its check was stopped, or the
    /// drive could not be emptied): storage stays Stopping, so the meeting
    /// service keeps away from it.
    NotMounted,
}

/// Whether the recordings volume is mounted on Wave now (`/proc/mounts` has
/// `MOUNT_POINT`). The meeting service reconciles the recordings only then.
pub fn recordings_mounted() -> bool {
    device::SystemVolume::device()
        .mounted()
        .is_ok_and(|mount| mount.is_some())
}

/// Whether the meeting service still owns a take: recording, paused,
/// finishing it (`recording` stays true until the file is final) or waiting
/// for the user's decision.
pub fn take_in_progress(meeting: &MeetingState) -> bool {
    meeting.recording || meeting.paused || meeting.pending_decision.is_some()
}

pub struct UsbStorage {
    gadget: Box<dyn Gadget>,
    volume: Box<dyn Volume>,
    state_tx: Arc<watch::Sender<UsbStorageState>>,
    events_tx: broadcast::Sender<DeviceEvent>,
    commands_tx: broadcast::Sender<Command>,
    partition: String,
    /// How the volume was mounted before the computer got it.
    mount: MountSpec,
    /// Polls in a row that found no computer.
    gone_polls: u32,
    /// Firmware updates (`with_ota`).
    ota: Option<watch::Receiver<OtaState>>,
    /// The holds on waved's USB handles (`with_holds`): `/dev/hidg0`
    /// (raised for the whole session) and `hw:1` (around each switch).
    hid_hold: Option<GadgetHold>,
    uac_hold: Option<GadgetHold>,
    sleep: Box<dyn Fn(Duration) + Send>,
}

impl UsbStorage {
    pub fn new(
        gadget: Box<dyn Gadget>,
        volume: Box<dyn Volume>,
        state_tx: Arc<watch::Sender<UsbStorageState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
        commands_tx: broadcast::Sender<Command>,
    ) -> Self {
        Self {
            gadget,
            volume,
            state_tx,
            events_tx,
            commands_tx,
            partition: PARTITION.into(),
            mount: MountSpec::boot(),
            gone_polls: 0,
            ota: None,
            hid_hold: None,
            uac_hold: None,
            sleep: Box::new(std::thread::sleep),
        }
    }

    /// The device: the configfs gadget, adbd, `/mnt/udisk` and `/dev/udisk`.
    pub fn device(
        state_tx: Arc<watch::Sender<UsbStorageState>>,
        events_tx: broadcast::Sender<DeviceEvent>,
        commands_tx: broadcast::Sender<Command>,
    ) -> Self {
        Self::new(
            Box::new(device::ConfigfsGadget::device()),
            Box::new(device::SystemVolume::device()),
            state_tx,
            events_tx,
            commands_tx,
        )
    }

    /// Replace the pauses (`SETTLE`, the waits for the computer and for
    /// normal USB) in tests.
    pub fn with_sleep(mut self, sleep: impl Fn(Duration) + Send + 'static) -> Self {
        self.sleep = Box::new(sleep);
        self
    }

    /// Follow firmware updates: no start while one installs (it restarts
    /// Wave, and the computer would lose its drive unejected). Before that
    /// point the update waits for storage to end (waved-update).
    pub fn with_ota(mut self, ota: watch::Receiver<OtaState>) -> Self {
        self.ota = Some(ota);
        self
    }

    /// Hold waved's USB handles around every unbind (`StateBus::hid_hold`,
    /// `StateBus::uac_hold`).
    pub fn with_holds(mut self, hid: GadgetHold, uac: GadgetHold) -> Self {
        self.hid_hold = Some(hid);
        self.uac_hold = Some(uac);
        self
    }

    fn holds(&self) -> impl Iterator<Item = &GadgetHold> {
        self.hid_hold.iter().chain(self.uac_hold.iter())
    }

    /// Raise both holds: waved's users of `/dev/hidg0` and `hw:1` close
    /// their files and confirm (`await_handles_closed`).
    fn raise_holds(&self) {
        for hold in self.holds() {
            hold.raise();
        }
    }

    /// Before any unbind: wait, at most `HOLD_WAIT`, until every user of a
    /// raised hold confirmed it closed its file. Linux 5.4 `f_hid` frees
    /// `hid.0` under open files (a read, write or poll on one touches freed
    /// memory), and the f_uac2 unbind waits for every file on `hw:1` to close.
    /// An error names who did not confirm: nothing may unbind then.
    fn await_handles_closed(&self) -> io::Result<()> {
        let mut waited = Duration::ZERO;
        loop {
            let waiting: Vec<&str> = self.holds().flat_map(GadgetHold::waiting_for).collect();
            if waiting.is_empty() {
                info!("usb storage: waved's USB handles are closed for the switch ({} ms)", waited.as_millis());
                return Ok(());
            }
            if waited >= HOLD_WAIT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{} did not close its USB handle within {} s",
                        waiting.join(", "),
                        HOLD_WAIT.as_secs()
                    ),
                ));
            }
            (self.sleep)(HOLD_POLL);
            waited += HOLD_POLL;
        }
    }

    /// The handles may open again: `hw:1` always, `/dev/hidg0` only once
    /// storage mode is over (it stays closed for the whole session).
    fn lower_holds(&self, storage_on: bool) {
        if let Some(hold) = &self.uac_hold {
            hold.lower();
        }
        if !storage_on {
            if let Some(hold) = &self.hid_hold {
                hold.lower();
            }
        }
        if storage_on {
            info!("usb storage: audio may open hw:1 again; /dev/hidg0 stays closed while storage is on");
        } else {
            info!("usb storage: waved's USB handles may open again");
        }
    }

    fn installing_update(&self) -> bool {
        self.ota
            .as_ref()
            .is_some_and(|ota| ota.borrow().phase.takes_over_screen())
    }

    pub fn phase(&self) -> UsbStoragePhase {
        self.state_tx.borrow().phase
    }

    fn publish(&self, phase: UsbStoragePhase) {
        self.state_tx.send_if_modified(|state| {
            let changed = state.phase != phase;
            state.phase = phase;
            changed
        });
    }

    fn problem(&self, problem: UsbStorageProblem) {
        let _ = self.events_tx.send(DeviceEvent::UsbStorageProblem(problem));
    }

    /// `Command::StartUsbStorage`: hand the volume to the computer. Refused
    /// (nothing changes) while a take is in progress, while a firmware update
    /// installs, with the gadget not in its normal mode, with no computer or
    /// with the volume not mounted here; a busy volume, or a USB handle of
    /// waved's that does not close for the switch (`HOLD_WAIT`), leaves
    /// everything as it was. A switch that does not come up goes straight
    /// back to normal USB and mounts the volume again. `/dev/hidg0` stays
    /// closed while storage is on; `hw:1` opens again once the computer
    /// configured the gadget.
    pub fn start(&mut self, meeting: &MeetingState) {
        if self.phase() != UsbStoragePhase::Off {
            return;
        }
        if take_in_progress(meeting) {
            info!("usb storage: not started, a recording is in progress");
            return self.problem(UsbStorageProblem::Recording);
        }
        if self.installing_update() {
            info!("usb storage: not started, a firmware update is installing");
            return self.problem(UsbStorageProblem::Unavailable);
        }
        let mode = self.gadget.mode();
        if mode != GadgetMode::Normal {
            warn!("usb storage: not started, the USB gadget is not in its normal mode ({mode:?})");
            return self.problem(UsbStorageProblem::Unavailable);
        }
        if self.gadget.host() != HostLink::Configured {
            info!("usb storage: not started, no computer on USB");
            return self.problem(UsbStorageProblem::NoComputer);
        }
        let spec = match self.volume.mounted() {
            Ok(Some(spec)) => spec,
            Ok(None) => {
                warn!("usb storage: not started, {MOUNT_POINT} is not mounted");
                return self.problem(UsbStorageProblem::NotMounted);
            }
            Err(error) => {
                warn!("usb storage: not started, the mounts cannot be read: {error}");
                return self.problem(UsbStorageProblem::NotMounted);
            }
        };
        // Stock's msc_init loads the partition into the drive at boot,
        // unlinked: the computer never saw it. Emptied first, so a switch
        // that fails part way leaves nothing loaded.
        match self.gadget.loaded() {
            Ok(None) => {}
            Ok(Some(medium)) => {
                info!("usb storage: emptying the unlinked drive ({medium}, stock's boot)");
                if !self.unload_verified() {
                    warn!("usb storage: not started, the unlinked drive cannot be emptied");
                    return self.problem(UsbStorageProblem::Unavailable);
                }
            }
            Err(error) => {
                warn!("usb storage: not started, the drive cannot be read: {error}");
                return self.problem(UsbStorageProblem::Unavailable);
            }
        }

        self.publish(UsbStoragePhase::Starting);
        let _ = self.commands_tx.send(Command::StopPlayback);
        // waved's USB handles close while playback lets go of its file.
        self.raise_holds();
        (self.sleep)(SETTLE);
        if let Err(error) = self.await_handles_closed() {
            warn!("usb storage: not started, {error}; USB not switched");
            self.lower_holds(false);
            self.publish(UsbStoragePhase::Off);
            return self.problem(UsbStorageProblem::Unavailable);
        }
        self.volume.sync();
        // Before the unmount: a waved that dies from here on leaves the note,
        // and the next one mounts the volume again (`reclaim_at_start`).
        if let Err(error) = self.volume.note_unmount(&spec) {
            warn!("usb storage: not started, the unmount cannot be noted: {error}");
            self.lower_holds(false);
            self.publish(UsbStoragePhase::Off);
            return self.problem(UsbStorageProblem::Unavailable);
        }
        if let Err(error) = self.volume.unmount() {
            info!("usb storage: not started, {MOUNT_POINT} is busy: {error}");
            self.volume.clear_unmount_note();
            self.lower_holds(false);
            self.publish(UsbStoragePhase::Off);
            return self.problem(UsbStorageProblem::Busy);
        }
        self.mount = spec;
        info!("usb storage: switching USB to storage mode (adb and HID are off until it ends)");
        let switched = self
            .gadget
            .enter_storage(&self.partition)
            .and_then(|()| self.await_computer());
        match switched {
            Ok(()) => {
                info!("usb storage: on, the computer has {}", self.partition);
                self.gone_polls = 0;
                // Configured: audio opens the new card; HID stays closed.
                self.lower_holds(true);
                self.publish(UsbStoragePhase::On);
            }
            Err(error) => {
                error!("usb storage: storage mode did not come up ({error}); back to normal USB");
                self.problem(UsbStorageProblem::Unavailable);
                self.leave(false);
            }
        }
    }

    /// After the switch: the computer enumerates Wave again within
    /// `ENUMERATE_WAIT`.
    fn await_computer(&self) -> io::Result<()> {
        let mut waited = Duration::ZERO;
        loop {
            if self.gadget.host() == HostLink::Configured {
                return Ok(());
            }
            if waited >= ENUMERATE_WAIT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no computer {} s after the switch", ENUMERATE_WAIT.as_secs()),
                ));
            }
            (self.sleep)(ENUMERATE_POLL);
            waited += ENUMERATE_POLL;
        }
    }

    /// `Command::StopUsbStorage`: take the volume back. While the computer
    /// still holds the drive this says "eject it there first" and storage
    /// stays on; it never forces the computer off.
    pub fn stop(&mut self) {
        if self.phase() != UsbStoragePhase::On {
            return;
        }
        self.publish(UsbStoragePhase::Stopping);
        // Emptied while still bound: the computer's lock shows as EBUSY. The
        // way back unbinds the gadget, which would end the lock unasked.
        if !self.unload_verified() {
            info!("usb storage: still on, the computer holds the drive");
            self.publish(UsbStoragePhase::On);
            return self.problem(UsbStorageProblem::EjectFirst);
        }
        info!("usb storage: stopped on Wave");
        self.leave(true);
    }

    /// Once a second: the computer ejected the drive (the LUN reads empty), or
    /// the cable is out (no computer for `GONE_POLLS` polls; also a gadget
    /// that fell off the bus). A sleeping computer keeps its drive.
    pub fn poll(&mut self) {
        if self.phase() != UsbStoragePhase::On {
            return;
        }
        if let Ok(None) = self.gadget.loaded() {
            info!("usb storage: the computer ejected the drive");
            self.leave(true);
            return;
        }
        if self.gadget.host() != HostLink::Gone {
            self.gone_polls = 0;
            return;
        }
        self.gone_polls += 1;
        if self.gone_polls < GONE_POLLS {
            return;
        }
        info!("usb storage: no computer for {GONE_POLLS} s (cable out?); taking the recordings back");
        self.leave(true);
    }

    /// At start-up, before the meeting service opens the recordings: take
    /// back what a waved that exited during USB storage left behind. USB left
    /// in storage mode (or part way through a switch) goes back to normal
    /// first, adb included; that ends the computer's hold on the drive. A
    /// volume that waved unmounted (the unmount note) is then checked and
    /// mounted again. A medium in the unlinked drive (stock's `msc_init` at
    /// boot) is only emptied: the computer never saw it. Without a note an
    /// unmounted volume is the boot's to mount: stock's `udisk_fix` may
    /// still be checking it. A gadget the boot has not bound yet is left
    /// alone.
    pub fn reclaim_at_start(&mut self) -> Reclaim {
        let note = self.volume.unmount_noted();
        if let Some(spec) = &note {
            self.mount = spec.clone();
        }
        let exported = match self.gadget.mode() {
            GadgetMode::Storage => {
                warn!("usb storage: USB was left in storage mode; back to normal USB first");
                self.back_to_normal_usb();
                true
            }
            GadgetMode::Normal | GadgetMode::Unbound | GadgetMode::Unavailable => {
                if let Ok(Some(medium)) = self.gadget.loaded() {
                    if let Err(error) = self.gadget.unload() {
                        warn!("usb storage: could not empty the unlinked drive ({medium}): {error}");
                    }
                }
                false
            }
        };
        if exported {
            match self.gadget.loaded() {
                Ok(None) => {}
                outcome => {
                    error!(
                        "usb storage: the drive still holds the partition ({outcome:?}); \
                         the recordings stay unmounted"
                    );
                    self.publish(UsbStoragePhase::Stopping);
                    self.problem(UsbStorageProblem::NotMounted);
                    return Reclaim::NotMounted;
                }
            }
        }
        match self.volume.mounted() {
            Ok(Some(_)) => {
                if exported {
                    // Mounted here while exported: the drive is empty now, so
                    // the computer cannot write any more. The mount stays.
                    error!("usb storage: the recordings were mounted while exported; emptied the drive");
                }
                if note.is_some() {
                    self.volume.clear_unmount_note();
                }
                self.publish(UsbStoragePhase::Off);
                if exported {
                    Reclaim::Reclaimed
                } else {
                    Reclaim::Nothing
                }
            }
            Ok(None) if exported || note.is_some() => {
                if !exported {
                    warn!("usb storage: a waved before this one took the recordings off Wave; mounting them again");
                }
                if self.mount_back(false) {
                    info!("usb storage: reclaimed, recordings mounted");
                    Reclaim::Reclaimed
                } else {
                    Reclaim::NotMounted
                }
            }
            Ok(None) => Reclaim::Nothing,
            Err(error) => {
                warn!("usb storage: the mounts cannot be read at start-up: {error}");
                Reclaim::Nothing
            }
        }
    }

    /// Leave storage mode: normal USB back (adb included), the drive read
    /// back empty, then the volume checked and mounted. The volume is never
    /// mounted while the drive may still hold the partition.
    fn leave(&mut self, announce: bool) -> bool {
        self.publish(UsbStoragePhase::Stopping);
        self.back_to_normal_usb();
        match self.gadget.loaded() {
            Ok(None) => self.mount_back(announce),
            outcome => {
                error!(
                    "usb storage: the drive still holds the partition ({outcome:?}); \
                     the recordings stay unmounted until Wave restarts"
                );
                self.problem(UsbStorageProblem::NotMounted);
                false
            }
        }
    }

    /// Normal USB again (`restore_gadget`); then waved's USB handles may open
    /// again, once the computer configured the gadget (or after
    /// `ENUMERATE_WAIT` without one: a pulled cable), or at once when normal
    /// USB did not come back. True if it came back.
    fn back_to_normal_usb(&mut self) -> bool {
        let restored = self.restore_gadget();
        if restored && self.holds().any(|hold| hold.users() > 0) {
            if let Err(error) = self.await_computer() {
                info!("usb storage: {error}; waved's USB handles open again anyway");
            }
        }
        self.lower_holds(false);
        restored
    }

    /// Normal USB again, tried `RESTORE_TRIES` times. False if it did not
    /// come back: Wave may then be off USB, adb included, until it restarts.
    /// Never unbinds before waved's USB handles are closed (HID has been
    /// since the start; audio closes now): if one stays open, USB stays as
    /// it is and this is false at once.
    fn restore_gadget(&mut self) -> bool {
        self.raise_holds();
        if let Err(error) = self.await_handles_closed() {
            error!(
                "usb storage: USB not switched back, {error}; \
                 Wave stays in storage mode until it restarts"
            );
            return false;
        }
        for attempt in 1..=RESTORE_TRIES {
            match self.gadget.leave_storage() {
                Ok(()) => {
                    info!("usb storage: normal USB is back");
                    return true;
                }
                Err(error) => {
                    error!("usb storage: normal USB did not come back ({attempt}/{RESTORE_TRIES}): {error}")
                }
            }
            if attempt < RESTORE_TRIES {
                (self.sleep)(RESTORE_PAUSE);
            }
        }
        error!("usb storage: Wave may be off USB, adb included, until it restarts");
        false
    }

    /// Empty the drive and read it back empty. False while the computer holds it.
    fn unload_verified(&mut self) -> bool {
        if let Err(error) = self.gadget.unload() {
            info!("usb storage: the drive was not emptied: {error}");
            return false;
        }
        matches!(self.gadget.loaded(), Ok(None))
    }

    /// The LUN is empty: check the filesystem and mount it as it was (else as
    /// the boot mounts it). Storage is Off only once `/proc/mounts` shows the
    /// volume: Off tells the meeting service the recordings are back, and it
    /// reconciles them. Until then the phase stays Stopping, so nothing
    /// records, uploads or reconciles on the empty mount point; the user is
    /// told to restart Wave (the next waved, or the boot, mounts it).
    /// `announce` logs the outcome as the end of storage mode. Returns
    /// whether the volume is mounted.
    fn mount_back(&mut self, announce: bool) -> bool {
        match self.volume.mounted() {
            Ok(Some(_)) => {
                self.volume.clear_unmount_note();
                self.publish(UsbStoragePhase::Off);
                return true;
            }
            Ok(None) => {}
            Err(error) => warn!("usb storage: the mounts cannot be read: {error}"),
        }
        self.publish(UsbStoragePhase::Stopping);
        let spec = self.mount.clone();
        match self.volume.check(&spec.source) {
            Ok(summary) => info!("usb storage: checked {}: {summary}", spec.source),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                // Stopped part way through its repairs: the filesystem may be
                // half repaired. Not mounted at all, so nothing writes to it;
                // the next waved checks it again (the note stays), and a
                // reboot mounts it as stock does.
                error!(
                    "usb storage: the check of {} was stopped ({error}); \
                     the recordings stay unmounted until Wave restarts",
                    spec.source
                );
                self.problem(UsbStorageProblem::NotMounted);
                return false;
            }
            // It did not run (no checker, no process): it changed nothing.
            // Stock's boot mounts the volume after a read-only check anyway.
            Err(error) => warn!("usb storage: could not check {}: {error}", spec.source),
        }
        let mut mounted = self.volume.mount(&spec);
        let boot = MountSpec::boot();
        if let Err(error) = &mounted {
            warn!("usb storage: mount as before failed ({error}); trying the boot mount");
            if spec != boot {
                mounted = self.volume.mount(&boot);
            }
        }
        let mounted = mounted.and_then(|()| match self.volume.mounted() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(io::Error::other(format!("mount succeeded, but {MOUNT_POINT} is not in the mounts"))),
            Err(error) => Err(error),
        });
        match mounted {
            Ok(()) => {
                self.volume.clear_unmount_note();
                self.publish(UsbStoragePhase::Off);
                if announce {
                    info!("usb storage: off, recordings mounted on Wave");
                }
                true
            }
            Err(error) => {
                error!(
                    "usb storage: the recordings could not be mounted: {error}; \
                     they stay away from the meeting service until Wave restarts"
                );
                self.problem(UsbStorageProblem::NotMounted);
                false
            }
        }
    }

    /// One pass of the service loop: the queued commands, then the poll when due.
    pub fn handle(&mut self, command: &Command, meeting: &MeetingState) {
        match command {
            Command::StartUsbStorage => self.start(meeting),
            Command::StopUsbStorage => self.stop(),
            _ => {}
        }
    }

    /// The service thread: commands from the bus, and a poll every second.
    pub fn run(mut self, mut commands: broadcast::Receiver<Command>, meeting: watch::Receiver<MeetingState>) {
        let mut last_poll = Instant::now();
        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => {
                        let meeting = meeting.borrow().clone();
                        self.handle(&command, &meeting);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Lagged(skipped)) => {
                        warn!("usb storage: lagged {skipped} commands; a start or stop may be lost");
                    }
                    Err(TryRecvError::Closed) => return,
                }
            }
            if last_poll.elapsed() >= POLL {
                last_poll = Instant::now();
                self.poll();
            }
            std::thread::sleep(TICK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uuid::Uuid;
    use waved_state::{GadgetHold, GadgetHoldUser, OtaPhase, OtaState, PendingDecision};

    /// One world for both fakes, so they can check the rule this module
    /// exists for: the computer and Wave never both have the volume.
    #[derive(Debug)]
    struct World {
        mode: GadgetMode,
        lun: Option<String>,
        host: HostLink,
        /// The computer prevents medium removal (the drive is in use there).
        /// An unbind (leaving storage mode) ends it, as a disconnect does.
        locked: bool,
        /// The switch to storage mode loads and links the drive, but the bind
        /// fails.
        enter_fails: bool,
        /// The computer after the switch to storage mode (it stays as it was
        /// when `None`).
        host_after_enter: Option<HostLink>,
        /// Ways back to normal USB that fail (the drive emptied, the bind not).
        leave_failures: u32,
        /// The way back fails before it empties the drive.
        leave_keeps_lun: bool,
        mounted: Option<MountSpec>,
        /// Something has a file open on the volume.
        busy: bool,
        /// Mount attempts that fail before one succeeds.
        mount_failures: u32,
        /// `mount` exits 0, but `/proc/mounts` does not show the volume.
        mount_vanishes: bool,
        /// The check runs past its time and is stopped.
        check_times_out: bool,
        /// The tmpfs note that waved took the volume off Wave.
        note: Option<MountSpec>,
        /// Pauses taken (`UsbStorage::sleep`).
        pauses: u32,
        log: Vec<String>,
        /// The holds on `/dev/hidg0` and `hw:1` (shared with the service).
        hid: GadgetHold,
        uac: GadgetHold,
        /// Users that close their file and confirm at the next pause, as the
        /// real ones do within their wake-up.
        users: Vec<GadgetHoldUser>,
        /// Users that never confirm (stuck in a call on the device).
        stuck: Vec<GadgetHoldUser>,
        /// The computer after the way back to normal USB.
        host_after_leave: Option<HostLink>,
        /// The computer configures the gadget after this many pauses without it.
        configured_after_pauses: Option<u32>,
        /// At every pause: (hid held, uac held, the computer).
        at_pauses: Vec<(bool, bool, HostLink)>,
        /// (hid held, uac held) when the volume was checked.
        holds_at_check: Option<(bool, bool)>,
    }

    impl World {
        /// A `/dev/hidg0` user (`hid`) or a `hw:1` user that never closes.
        fn with_stuck(mut self, hid: bool) -> Self {
            let user = if hid { self.hid.register("the HID reader") } else { self.uac.register("the UAC upstream") };
            self.stuck.push(user);
            self
        }
    }

    impl Default for World {
        fn default() -> Self {
            let hid = GadgetHold::new();
            let uac = GadgetHold::new();
            let users = vec![hid.register("the HID key writer"), uac.register("the UAC upstream")];
            Self {
                hid,
                uac,
                users,
                stuck: Vec::new(),
                host_after_leave: None,
                configured_after_pauses: None,
                at_pauses: Vec::new(),
                holds_at_check: None,
                mode: GadgetMode::Normal,
                lun: None,
                host: HostLink::Configured,
                locked: false,
                enter_fails: false,
                host_after_enter: None,
                leave_failures: 0,
                leave_keeps_lun: false,
                mounted: Some(live_mount()),
                busy: false,
                mount_failures: 0,
                mount_vanishes: false,
                check_times_out: false,
                note: None,
                pauses: 0,
                log: Vec::new(),
            }
        }
    }

    fn live_mount() -> MountSpec {
        MountSpec {
            source: "/dev/udisk".into(),
            target: "/mnt/udisk".into(),
            fstype: "exfat".into(),
            options: "rw,relatime,fmask=0077,dmask=0022".into(),
        }
    }

    impl World {
        /// The rule this hold exists for: nothing unbinds while a user may
        /// still have a file on `/dev/hidg0` or `hw:1`.
        fn assert_handles_closed(&self) {
            assert!(
                self.hid.all_closed() && self.uac.all_closed(),
                "unbound with a USB handle open: hid {:?}, uac {:?}",
                self.hid,
                self.uac
            );
        }

        /// One `UsbStorage::sleep`: the users look at the holds, and the
        /// computer may configure the gadget.
        fn pause(&mut self) {
            self.pauses += 1;
            for user in &self.users {
                if let Some(generation) = user.held() {
                    user.confirm(generation);
                }
            }
            let seen = (self.hid.signal().held, self.uac.signal().held, self.host);
            self.at_pauses.push(seen);
            if self.host == HostLink::Configured {
                return;
            }
            match self.configured_after_pauses {
                Some(n) if n <= 1 => {
                    self.host = HostLink::Configured;
                    self.configured_after_pauses = None;
                }
                Some(n) => self.configured_after_pauses = Some(n - 1),
                None => {}
            }
        }
    }

    type Shared = Arc<Mutex<World>>;

    struct FakeGadget(Shared);
    struct FakeVolume(Shared);

    impl Gadget for FakeGadget {
        fn mode(&self) -> GadgetMode {
            self.0.lock().unwrap().mode
        }
        fn enter_storage(&mut self, partition: &str) -> io::Result<()> {
            let mut w = self.0.lock().unwrap();
            w.log.push(format!("enter {partition}"));
            assert!(w.mounted.is_none(), "exported while mounted on Wave");
            w.assert_handles_closed();
            w.mode = GadgetMode::Storage;
            w.lun = Some(partition.to_string());
            if let Some(host) = w.host_after_enter {
                w.host = host;
            }
            if w.enter_fails {
                return Err(io::Error::other("UDC does not read fdd00000.crgudc2 after the bind"));
            }
            Ok(())
        }
        fn leave_storage(&mut self) -> io::Result<()> {
            let mut w = self.0.lock().unwrap();
            w.log.push("leave".into());
            w.assert_handles_closed();
            if let Some(host) = w.host_after_leave {
                w.host = host;
            }
            if w.leave_keeps_lun {
                return Err(io::Error::other("still bound after none"));
            }
            // Unbound: the computer's hold ends, the drive is emptied.
            w.locked = false;
            w.lun = None;
            if w.leave_failures > 0 {
                w.leave_failures -= 1;
                w.mode = GadgetMode::Storage;
                return Err(io::Error::other("UDC does not read fdd00000.crgudc2 after the bind"));
            }
            w.mode = GadgetMode::Normal;
            Ok(())
        }
        fn loaded(&self) -> io::Result<Option<String>> {
            Ok(self.0.lock().unwrap().lun.clone())
        }
        fn unload(&mut self) -> io::Result<()> {
            let mut w = self.0.lock().unwrap();
            w.log.push("unload".into());
            if w.locked && w.lun.is_some() {
                return Err(io::Error::from_raw_os_error(libc::EBUSY));
            }
            w.lun = None;
            Ok(())
        }
        fn host(&self) -> HostLink {
            self.0.lock().unwrap().host
        }
    }

    impl Volume for FakeVolume {
        fn sync(&mut self) {
            self.0.lock().unwrap().log.push("sync".into());
        }
        fn mounted(&self) -> io::Result<Option<MountSpec>> {
            Ok(self.0.lock().unwrap().mounted.clone())
        }
        fn unmount(&mut self) -> io::Result<()> {
            let mut w = self.0.lock().unwrap();
            w.log.push("unmount".into());
            if w.busy {
                return Err(io::Error::from_raw_os_error(libc::EBUSY));
            }
            w.mounted = None;
            Ok(())
        }
        fn check(&mut self, source: &str) -> io::Result<String> {
            let mut w = self.0.lock().unwrap();
            assert!(w.mounted.is_none(), "checked while mounted");
            w.log.push(format!("check {source}"));
            w.holds_at_check = Some((w.hid.signal().held, w.uac.signal().held));
            if w.check_times_out {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "stopped after 120 s"));
            }
            Ok("clean".into())
        }
        fn mount(&mut self, spec: &MountSpec) -> io::Result<()> {
            let mut w = self.0.lock().unwrap();
            w.log.push(format!("mount {}", spec.options));
            assert!(w.lun.is_none(), "mounted on Wave while exported");
            if w.mount_failures > 0 {
                w.mount_failures -= 1;
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            if !w.mount_vanishes {
                w.mounted = Some(spec.clone());
            }
            Ok(())
        }
        fn note_unmount(&mut self, spec: &MountSpec) -> io::Result<()> {
            self.0.lock().unwrap().note = Some(spec.clone());
            Ok(())
        }
        fn unmount_noted(&self) -> Option<MountSpec> {
            self.0.lock().unwrap().note.clone()
        }
        fn clear_unmount_note(&mut self) {
            self.0.lock().unwrap().note = None;
        }
    }

    struct Rig {
        world: Shared,
        storage: UsbStorage,
        state: watch::Receiver<UsbStorageState>,
        events: broadcast::Receiver<DeviceEvent>,
        commands: broadcast::Receiver<Command>,
    }

    impl Rig {
        fn new(world: World) -> Self {
            let (hid, uac) = (world.hid.clone(), world.uac.clone());
            let world = Arc::new(Mutex::new(world));
            let state_tx = Arc::new(watch::channel(UsbStorageState::default()).0);
            let state = state_tx.subscribe();
            let (events_tx, events) = broadcast::channel(16);
            let (commands_tx, commands) = broadcast::channel(16);
            let pauses = Arc::clone(&world);
            let storage = UsbStorage::new(
                Box::new(FakeGadget(Arc::clone(&world))),
                Box::new(FakeVolume(Arc::clone(&world))),
                state_tx,
                events_tx,
                commands_tx,
            )
            .with_holds(hid, uac)
            .with_sleep(move |_| pauses.lock().unwrap().pause());
            Self {
                world,
                storage,
                state,
                events,
                commands,
            }
        }

        /// In storage mode, as after a start.
        fn on(world: World) -> Self {
            let mut rig = Self::new(world);
            rig.storage.start(&MeetingState::default());
            assert_eq!(rig.phase(), UsbStoragePhase::On);
            rig.log();
            rig.problems();
            rig
        }

        fn phase(&self) -> UsbStoragePhase {
            self.state.borrow().phase
        }

        fn world(&self) -> std::sync::MutexGuard<'_, World> {
            self.world.lock().unwrap()
        }

        /// The device operations since the last call.
        fn log(&self) -> Vec<String> {
            std::mem::take(&mut self.world().log)
        }

        fn problems(&mut self) -> Vec<UsbStorageProblem> {
            let mut out = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                if let DeviceEvent::UsbStorageProblem(problem) = event {
                    out.push(problem);
                }
            }
            out
        }

        fn sent(&mut self) -> Vec<Command> {
            let mut out = Vec::new();
            while let Ok(command) = self.commands.try_recv() {
                out.push(command);
            }
            out
        }
    }

    fn idle() -> MeetingState {
        MeetingState::default()
    }

    const LIVE: &str = "mount rw,relatime,fmask=0077,dmask=0022";

    #[test]
    fn start_unmounts_first_then_switches_usb_to_storage_mode() {
        let mut rig = Rig::new(World::default());
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::On);
        assert_eq!(rig.log(), ["sync", "unmount", "enter /dev/udisk"]);
        assert_eq!(rig.sent(), [Command::StopPlayback], "playback lets go of its file first");
        assert!(rig.problems().is_empty());
        let world = rig.world();
        assert!(world.mounted.is_none());
        assert_eq!(world.mode, GadgetMode::Storage);
    }

    #[test]
    fn a_take_in_progress_refuses_to_start_and_touches_nothing() {
        let decision = PendingDecision {
            id: Uuid::new_v4(),
            audio_path: "/mnt/udisk/recordings/x/audio.wav".into(),
            duration_secs: 60,
            size_bytes: 1,
            endpoint: String::new(),
            speakers: 0,
            marks: 0,
        };
        let busy = [
            MeetingState { recording: true, ..idle() },
            MeetingState { recording: true, paused: true, ..idle() },
            MeetingState { pending_decision: Some(decision), ..idle() },
        ];
        for meeting in busy {
            let mut rig = Rig::new(World::default());
            rig.storage.start(&meeting);
            assert_eq!(rig.phase(), UsbStoragePhase::Off);
            assert!(rig.log().is_empty(), "{meeting:?}");
            assert_eq!(rig.problems(), [UsbStorageProblem::Recording]);
            assert!(rig.world().mounted.is_some());
        }
    }

    #[test]
    fn a_firmware_install_refuses_to_start_and_touches_nothing() {
        // The install restarts Wave: the computer would lose its drive unejected.
        for phase in [OtaPhase::Installing, OtaPhase::Restarting] {
            let mut rig = Rig::new(World::default());
            let (ota_tx, ota_rx) = watch::channel(OtaState::default());
            rig.storage = rig.storage.with_ota(ota_rx);
            ota_tx.send_modify(|ota| ota.phase = phase.clone());
            rig.storage.start(&idle());
            assert_eq!(rig.phase(), UsbStoragePhase::Off, "{phase:?}");
            assert!(rig.log().is_empty(), "{phase:?}");
            assert_eq!(rig.problems(), [UsbStorageProblem::Unavailable]);
        }
        // Before the point of no return the update waits for storage instead.
        let mut rig = Rig::new(World::default());
        let (ota_tx, ota_rx) = watch::channel(OtaState::default());
        rig.storage = rig.storage.with_ota(ota_rx);
        ota_tx.send_modify(|ota| ota.phase = OtaPhase::Waiting);
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::On);
    }

    #[test]
    fn no_computer_refuses_to_start() {
        for host in [HostLink::Gone, HostLink::Suspended] {
            let mut rig = Rig::new(World { host, ..World::default() });
            rig.storage.start(&idle());
            assert_eq!(rig.phase(), UsbStoragePhase::Off);
            assert!(rig.log().is_empty());
            assert_eq!(rig.problems(), [UsbStorageProblem::NoComputer]);
        }
    }

    #[test]
    fn a_gadget_not_in_its_normal_mode_refuses_to_start() {
        for mode in [GadgetMode::Unavailable, GadgetMode::Unbound, GadgetMode::Storage] {
            let mut rig = Rig::new(World { mode, ..World::default() });
            rig.storage.start(&idle());
            assert_eq!(rig.phase(), UsbStoragePhase::Off, "{mode:?}");
            assert!(rig.log().is_empty(), "{mode:?}");
            assert_eq!(rig.problems(), [UsbStorageProblem::Unavailable], "{mode:?}");
        }
    }

    #[test]
    fn the_boot_medium_in_the_unlinked_drive_is_emptied_first() {
        // Stock's msc_init loads the partition into the drive at boot,
        // unlinked: the computer never saw it. Emptied before anything
        // changes, so a switch that fails part way leaves nothing loaded.
        let mut rig = Rig::new(World {
            lun: Some("/dev/mmcblk0p16".into()),
            ..World::default()
        });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::On);
        assert_eq!(rig.log(), ["unload", "sync", "unmount", "enter /dev/udisk"]);
    }

    #[test]
    fn an_unmounted_volume_refuses_to_start() {
        let mut rig = Rig::new(World { mounted: None, ..World::default() });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert!(rig.log().is_empty());
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
    }

    #[test]
    fn a_busy_volume_leaves_everything_as_it_was() {
        let mut rig = Rig::new(World { busy: true, ..World::default() });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.log(), ["sync", "unmount"]);
        assert_eq!(rig.problems(), [UsbStorageProblem::Busy]);
        let world = rig.world();
        assert!(world.mounted.is_some() && world.lun.is_none());
        assert_eq!(world.mode, GadgetMode::Normal, "USB never switched");
    }

    #[test]
    fn a_switch_that_does_not_bind_goes_straight_back_to_normal_usb() {
        let mut rig = Rig::new(World { enter_fails: true, ..World::default() });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(
            rig.log(),
            ["sync", "unmount", "enter /dev/udisk", "leave", "check /dev/udisk", LIVE]
        );
        assert_eq!(rig.problems(), [UsbStorageProblem::Unavailable]);
        let world = rig.world();
        assert_eq!(world.mounted, Some(live_mount()), "mounted as it was");
        assert_eq!(world.mode, GadgetMode::Normal);
    }

    #[test]
    fn no_computer_after_the_switch_goes_back_to_normal_usb() {
        let mut rig = Rig::new(World {
            host_after_enter: Some(HostLink::Gone),
            ..World::default()
        });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(
            rig.log(),
            ["sync", "unmount", "enter /dev/udisk", "leave", "check /dev/udisk", LIVE]
        );
        assert_eq!(rig.problems(), [UsbStorageProblem::Unavailable]);
        // Waited for the computer after the switch, and again (bounded)
        // after the way back before the USB handles may open again.
        let polls = (ENUMERATE_WAIT.as_millis() / ENUMERATE_POLL.as_millis()) as u32;
        let pauses = rig.world().pauses;
        assert!(pauses >= 2 * polls && pauses <= 2 * polls + 3, "waited {pauses} pauses, bounded");
    }

    #[test]
    fn stop_empties_the_drive_then_goes_back_to_normal_usb_and_mounts() {
        let mut rig = Rig::on(World::default());
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.log(), ["unload", "leave", "check /dev/udisk", LIVE]);
        assert!(rig.problems().is_empty());
        let world = rig.world();
        assert_eq!(world.mounted, Some(live_mount()));
        assert_eq!(world.mode, GadgetMode::Normal);
    }

    #[test]
    fn stop_while_the_computer_holds_the_drive_asks_to_eject_there_first() {
        let mut rig = Rig::on(World::default());
        rig.world().locked = true;
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::On);
        assert_eq!(rig.log(), ["unload"], "never forced, USB not switched, never mounted");
        assert_eq!(rig.problems(), [UsbStorageProblem::EjectFirst]);
        assert!(rig.world().mounted.is_none());
        // Ejected on the computer: the next poll takes the volume back.
        {
            let mut world = rig.world();
            world.locked = false;
            world.lun = None;
        }
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.log(), ["leave", "check /dev/udisk", LIVE]);
        assert!(rig.world().mounted.is_some());
    }

    #[test]
    fn the_computer_ejecting_the_drive_ends_storage() {
        let mut rig = Rig::on(World::default());
        rig.world().lun = None;
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.log(), ["leave", "check /dev/udisk", LIVE]);
        assert!(rig.problems().is_empty());
        assert_eq!(rig.world().mode, GadgetMode::Normal);
    }

    #[test]
    fn a_pulled_cable_ends_storage_after_three_polls() {
        // The computer locked the drive; the way back unbinds the gadget,
        // which ends the hold.
        let mut rig = Rig::on(World { locked: true, ..World::default() });
        rig.world().host = HostLink::Gone;
        rig.storage.poll();
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::On, "a re-enumeration is not a pull");
        assert!(rig.log().is_empty());
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.log(), ["leave", "check /dev/udisk", LIVE]);
        assert_eq!(rig.world().mode, GadgetMode::Normal);
    }

    #[test]
    fn a_computer_back_within_the_grace_keeps_its_drive() {
        let mut rig = Rig::on(World::default());
        rig.world().host = HostLink::Gone;
        rig.storage.poll();
        rig.storage.poll();
        rig.world().host = HostLink::Configured;
        rig.storage.poll();
        rig.world().host = HostLink::Gone;
        rig.storage.poll();
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::On, "the count starts again");
    }

    #[test]
    fn a_sleeping_computer_keeps_its_drive() {
        let mut rig = Rig::on(World::default());
        rig.world().host = HostLink::Suspended;
        for _ in 0..10 {
            rig.storage.poll();
        }
        assert_eq!(rig.phase(), UsbStoragePhase::On);
        assert!(rig.log().is_empty());
    }

    #[test]
    fn normal_usb_that_does_not_come_back_is_tried_again() {
        let mut rig = Rig::on(World::default());
        rig.world().leave_failures = 1;
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.log(), ["unload", "leave", "leave", "check /dev/udisk", LIVE]);
        assert_eq!(rig.world().mode, GadgetMode::Normal);
    }

    #[test]
    fn the_recordings_come_back_even_when_normal_usb_does_not() {
        // The drive is empty (unbound, unlinked): the computer cannot write.
        let mut rig = Rig::on(World::default());
        rig.world().leave_failures = 10;
        rig.storage.stop();
        let log = rig.log();
        assert_eq!(log.iter().filter(|op| *op == "leave").count(), RESTORE_TRIES as usize);
        assert_eq!(log.last().map(String::as_str), Some(LIVE));
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
    }

    #[test]
    fn a_drive_that_still_holds_the_partition_is_never_mounted() {
        // A pulled cable leaves the partition in the drive until the way
        // back empties it; here the way back fails before it does.
        let mut rig = Rig::on(World::default());
        {
            let mut world = rig.world();
            world.leave_keeps_lun = true;
            world.host = HostLink::Gone;
        }
        for _ in 0..GONE_POLLS {
            rig.storage.poll();
        }
        assert!(!rig.log().iter().any(|op| op.starts_with("mount")));
        assert_eq!(rig.phase(), UsbStoragePhase::Stopping);
        assert!(rig.state.borrow().recordings_away());
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
        assert!(rig.world().mounted.is_none());
        // Stop empties the drive while still bound: the same failing way back
        // then still gives the recordings back.
        let mut rig = Rig::on(World::default());
        rig.world().leave_keeps_lun = true;
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
    }

    #[test]
    fn a_failed_mount_falls_back_to_the_boot_mount() {
        let mut rig = Rig::on(World::default());
        rig.world().mount_failures = 1;
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(
            rig.log(),
            ["unload", "leave", "check /dev/udisk", LIVE, "mount rw,fmask=0077"]
        );
        assert!(rig.problems().is_empty());
        assert_eq!(rig.world().mounted, Some(MountSpec::boot()));
    }

    #[test]
    fn a_volume_that_will_not_mount_says_so() {
        let mut rig = Rig::on(World::default());
        rig.world().mount_failures = 2;
        rig.storage.stop();
        assert_eq!(
            rig.phase(),
            UsbStoragePhase::Stopping,
            "the computer no longer has it, and Wave has not got it back"
        );
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
        assert!(rig.world().mounted.is_none());
    }

    #[test]
    fn start_and_stop_only_act_from_their_own_phase() {
        let mut rig = Rig::new(World::default());
        rig.storage.stop();
        rig.storage.poll();
        assert!(rig.log().is_empty(), "nothing to stop");
        let mut rig = Rig::on(World::default());
        rig.storage.start(&idle());
        assert!(rig.log().is_empty(), "already on");
        assert!(rig.problems().is_empty());
    }

    #[test]
    fn handle_routes_the_two_commands() {
        let mut rig = Rig::new(World::default());
        rig.storage.handle(&Command::StopPlayback, &idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        rig.storage.handle(&Command::StartUsbStorage, &idle());
        assert_eq!(rig.phase(), UsbStoragePhase::On);
        rig.storage.handle(&Command::StopUsbStorage, &idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
    }

    #[test]
    fn reclaim_with_normal_usb_does_nothing() {
        let mut rig = Rig::new(World::default());
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Nothing);
        assert!(rig.log().is_empty());
    }

    #[test]
    fn reclaim_brings_normal_usb_back_before_it_mounts() {
        // A waved that exited in storage mode, with the computer holding the
        // drive: the way back unbinds the gadget, which ends the hold.
        let mut rig = Rig::new(World {
            mode: GadgetMode::Storage,
            lun: Some("/dev/mmcblk0p16".into()),
            locked: true,
            mounted: None,
            note: Some(live_mount()),
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(rig.log(), ["leave", "check /dev/udisk", LIVE], "mounted as the note says");
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        let world = rig.world();
        assert_eq!(world.mode, GadgetMode::Normal);
        assert_eq!(world.note, None);
    }

    #[test]
    fn reclaim_of_storage_mode_without_a_note_mounts_as_the_boot_does() {
        let mut rig = Rig::new(World {
            mode: GadgetMode::Storage,
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(rig.log(), ["leave", "check /dev/udisk", "mount rw,fmask=0077"]);
    }

    #[test]
    fn reclaim_never_mounts_while_the_drive_holds_the_partition() {
        let mut rig = Rig::new(World {
            mode: GadgetMode::Storage,
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            leave_keeps_lun: true,
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::NotMounted);
        assert!(!rig.log().iter().any(|op| op.starts_with("mount")));
        assert!(rig.state.borrow().recordings_away(), "the meeting service must not reconcile");
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
    }

    #[test]
    fn reclaim_only_empties_a_drive_the_computer_never_saw() {
        // Stock's msc_init loads the partition at boot into the unlinked
        // drive; the boot mounts the volume itself.
        for mode in [GadgetMode::Normal, GadgetMode::Unbound] {
            let mut rig = Rig::new(World {
                mode,
                lun: Some("/dev/mmcblk0p16".into()),
                mounted: None,
                ..World::default()
            });
            assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Nothing, "{mode:?}");
            assert_eq!(rig.log(), ["unload"], "{mode:?}");
            assert!(rig.world().mounted.is_none());
        }
    }

    #[test]
    fn reclaim_leaves_a_gadget_the_boot_has_not_bound_yet_alone() {
        // S89usbgadget binds after waved starts: never a switch next to it.
        let mut rig = Rig::new(World {
            mode: GadgetMode::Unbound,
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Nothing);
        assert!(rig.log().is_empty());
    }

    #[test]
    fn reclaim_of_storage_mode_with_the_volume_mounted_keeps_the_mount() {
        let mut rig = Rig::new(World {
            mode: GadgetMode::Storage,
            ..World::default()
        });
        rig.world().lun = Some("/dev/mmcblk0p16".into());
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(rig.log(), ["leave"], "already mounted: nothing to mount");
        assert!(rig.world().lun.is_none());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
    }

    #[test]
    fn start_notes_the_unmount_and_the_mount_back_clears_the_note() {
        let mut rig = Rig::new(World::default());
        rig.storage.start(&idle());
        assert_eq!(rig.world().note, Some(live_mount()), "how it was mounted");
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.world().note, None);
    }

    #[test]
    fn a_busy_volume_leaves_no_note() {
        let mut rig = Rig::new(World { busy: true, ..World::default() });
        rig.storage.start(&idle());
        assert_eq!(rig.world().note, None, "still mounted on Wave");
    }

    #[test]
    fn storage_stays_away_until_the_volume_is_mounted() {
        // Off tells the meeting service the recordings are back: it reconciles
        // them. On an empty mount point every one would read as missing.
        let mut rig = Rig::on(World::default());
        rig.world().mount_failures = 2;
        rig.storage.stop();
        assert_ne!(rig.phase(), UsbStoragePhase::Off);
        assert!(rig.state.borrow().recordings_away());
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
        assert!(rig.world().note.is_some(), "a restart still owes the mount");
    }

    #[test]
    fn a_mount_that_proc_mounts_does_not_show_is_not_a_mount() {
        let mut rig = Rig::on(World::default());
        rig.world().mount_vanishes = true;
        rig.storage.stop();
        assert_ne!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
    }

    #[test]
    fn an_ejected_drive_that_will_not_mount_keeps_the_recordings_away() {
        let mut rig = Rig::on(World::default());
        {
            let mut world = rig.world();
            world.lun = None;
            world.mount_failures = 2;
        }
        rig.storage.poll();
        assert!(rig.state.borrow().recordings_away());
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
    }

    #[test]
    fn a_check_stopped_part_way_leaves_the_volume_unmounted() {
        // fsck.exfat -p killed at 120 s may have left a half-repaired
        // filesystem: never mount that read-write.
        let mut rig = Rig::on(World::default());
        rig.world().check_times_out = true;
        rig.storage.stop();
        assert_eq!(rig.log(), ["unload", "leave", "check /dev/udisk"], "no mount");
        assert_eq!(rig.phase(), UsbStoragePhase::Stopping);
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
        assert!(rig.world().note.is_some(), "a restart checks it again");
    }

    #[test]
    fn reclaim_that_cannot_mount_keeps_the_recordings_away() {
        let mut rig = Rig::new(World {
            mode: GadgetMode::Storage,
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            mount_failures: 2,
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::NotMounted);
        assert!(rig.state.borrow().recordings_away(), "the meeting service must not reconcile");
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
    }

    #[test]
    fn reclaim_mounts_a_volume_a_waved_left_unmounted() {
        // waved died between the unmount and the switch, or between emptying
        // the drive and the mount (the check takes up to 120 s): USB is
        // normal, the volume unmounted, and the note says waved owes the mount.
        let mut rig = Rig::new(World {
            mounted: None,
            note: Some(live_mount()),
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(rig.log(), ["check /dev/udisk", LIVE], "mounted as the note says");
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(rig.world().note, None);
    }

    #[test]
    fn reclaim_leaves_an_unmounted_volume_to_the_boot_without_a_note() {
        // A fresh boot: stock's udisk_fix may still be checking the volume.
        let mut rig = Rig::new(World { mounted: None, ..World::default() });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Nothing);
        assert!(rig.log().is_empty(), "never a second fsck or mount next to the boot's");
        assert!(rig.problems().is_empty());
    }

    #[test]
    fn reclaim_drops_a_note_once_the_volume_is_mounted() {
        let mut rig = Rig::new(World {
            note: Some(live_mount()),
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Nothing);
        assert!(rig.log().is_empty());
        assert_eq!(rig.world().note, None);
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
    }

    // --- The holds on waved's USB handles (/dev/hidg0, hw:1) ---

    fn held(rig: &Rig) -> (bool, bool) {
        let world = rig.world();
        (world.hid.signal().held, world.uac.signal().held)
    }

    #[test]
    fn start_closes_every_usb_handle_before_the_unbind_and_keeps_hid_closed_while_on() {
        // FakeGadget::enter_storage asserts that every user confirmed.
        let mut rig = Rig::new(World::default());
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::On);
        assert_eq!(rig.log(), ["sync", "unmount", "enter /dev/udisk"]);
        assert_eq!(held(&rig), (true, false), "HID stays closed, audio runs on the new card");
    }

    #[test]
    fn a_usb_handle_that_does_not_close_aborts_the_start_before_anything_changes() {
        for hid in [true, false] {
            let mut rig = Rig::new(World::default().with_stuck(hid));
            rig.storage.start(&idle());
            assert_eq!(rig.phase(), UsbStoragePhase::Off, "hid {hid}");
            assert_eq!(rig.problems(), [UsbStorageProblem::Unavailable], "USB storage isn't available");
            assert!(rig.log().is_empty(), "no unmount, no switch (hid {hid})");
            assert_eq!(held(&rig), (false, false), "the handles may open again");
            let world = rig.world();
            assert_eq!(world.mode, GadgetMode::Normal);
            assert_eq!(world.mounted, Some(live_mount()));
            assert_eq!(world.note, None);
            let bound = (HOLD_WAIT.as_millis() / HOLD_POLL.as_millis()) as u32 + 2;
            assert!(world.pauses <= bound, "{} pauses: the wait is bounded", world.pauses);
        }
    }

    #[test]
    fn the_hold_wait_is_longer_than_the_hid_readers_read() {
        // The reader confirms after its read returns (up to 1 s) and its
        // 200 ms look at the hold.
        assert!(HOLD_WAIT >= Duration::from_secs(3));
    }

    #[test]
    fn a_busy_volume_lowers_the_holds() {
        let mut rig = Rig::new(World { busy: true, ..World::default() });
        rig.storage.start(&idle());
        assert_eq!(rig.problems(), [UsbStorageProblem::Busy]);
        assert_eq!(held(&rig), (false, false));
    }

    #[test]
    fn the_way_back_closes_the_audio_handle_and_lowers_both_holds_once_normal_usb_is_configured() {
        let mut rig = Rig::on(World::default());
        {
            let mut world = rig.world();
            world.host_after_leave = Some(HostLink::Gone);
            world.configured_after_pauses = Some(3);
            world.at_pauses.clear();
        }
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(held(&rig), (false, false));
        let world = rig.world();
        // The audio user confirms at the first pause; after the unbind, the
        // holds stay up while the computer has not configured normal USB.
        let after_leave: Vec<_> = world.at_pauses.iter().filter(|(.., host)| *host == HostLink::Gone).collect();
        assert_eq!(after_leave.len(), 3, "{:?}", world.at_pauses);
        assert!(after_leave.iter().all(|(hid, uac, _)| *hid && *uac), "{:?}", world.at_pauses);
        assert_eq!(world.holds_at_check, Some((false, false)), "lowered before the check, not after it");
    }

    #[test]
    fn the_holds_are_lowered_after_the_way_back_gives_up_on_the_computer() {
        // A pulled cable: nobody configures normal USB.
        let mut rig = Rig::on(World::default());
        rig.world().host = HostLink::Gone;
        for _ in 0..GONE_POLLS {
            rig.storage.poll();
        }
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(held(&rig), (false, false));
    }

    #[test]
    fn a_switch_that_does_not_come_up_lowers_the_holds_after_the_way_back() {
        let mut rig = Rig::new(World { enter_fails: true, ..World::default() });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(held(&rig), (false, false));
    }

    #[test]
    fn reclaim_closes_the_usb_handles_before_it_unbinds() {
        // A new waved's users opened the storage-mode nodes at start-up.
        let mut rig = Rig::new(World {
            mode: GadgetMode::Storage,
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            note: Some(live_mount()),
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(rig.log(), ["leave", "check /dev/udisk", LIVE]);
        assert_eq!(held(&rig), (false, false));
    }

    #[test]
    fn reclaim_never_unbinds_while_a_usb_handle_stays_open() {
        let mut rig = Rig::new(
            World {
                mode: GadgetMode::Storage,
                lun: Some("/dev/mmcblk0p16".into()),
                mounted: None,
                note: Some(live_mount()),
                ..World::default()
            }
            .with_stuck(false),
        );
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::NotMounted);
        assert!(rig.log().is_empty(), "no unbind, no mount");
        assert_eq!(rig.phase(), UsbStoragePhase::Stopping, "the recordings stay away");
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted], "restart Wave");
        assert_eq!(held(&rig), (false, false));
        assert_eq!(rig.world().mode, GadgetMode::Storage);
    }

    #[test]
    fn host_link_reads_every_controller() {
        assert_eq!(HostLink::from_states(["configured\n"]), HostLink::Configured);
        assert_eq!(HostLink::from_states(["suspended\n"]), HostLink::Suspended);
        assert_eq!(
            HostLink::from_states(["not attached", "configured"]),
            HostLink::Configured
        );
        for state in ["not attached\n", "powered", "default", "addressed", ""] {
            assert_eq!(HostLink::from_states([state]), HostLink::Gone, "{state:?}");
        }
        assert_eq!(HostLink::from_states([]), HostLink::Gone);
    }
}
