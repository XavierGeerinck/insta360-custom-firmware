//! USB storage (G1, `docs/USB_STORAGE.md`): the computer opens the recordings
//! volume as a drive while Wave keeps its microphone, speaker, HID and adb.
//!
//! The gadget carries a removable drive with no medium from boot
//! (`scripts/patch-stock-usbgadget.sh`). Storage mode swaps the medium, never
//! the gadget: it unmounts `/mnt/udisk`, then loads `/dev/udisk` into
//! `lun.0/file`. Taking it back unloads the LUN, checks the filesystem and
//! mounts it again. The UDC is never unbound, so no failure here can leave
//! the computer without audio or adb. The two sides never write the volume at
//! once: the LUN is loaded only while the volume is unmounted, and the volume
//! is mounted only once the LUN reads empty. Neither the kernel's LUN open nor
//! `mount` would stop a double writer by itself.
//!
//! Device access sits behind `Gadget` and `Volume`; `device` has the configfs
//! and system implementations.

pub mod device;

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::{self, error::TryRecvError};
use tokio::sync::watch;
use tracing::{error, info, warn};
use waved_state::{
    Command, DeviceEvent, MeetingState, OtaState, UsbStoragePhase, UsbStorageProblem,
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
const POLL: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(100);
/// Unload tries after a re-plug at start-up, `RETRY_PAUSE` apart.
const RELEASE_TRIES: u32 = 5;
const RETRY_PAUSE: Duration = Duration::from_millis(500);

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

/// The mass-storage function of the USB gadget.
pub trait Gadget: Send {
    /// A removable drive is linked into the gadget's configuration.
    fn drive_ready(&self) -> bool;
    /// The medium loaded in the drive (`lun.0/file`), `None` when empty.
    /// An error means there is no drive at all (no gadget yet).
    fn loaded(&self) -> io::Result<Option<String>>;
    /// Load `path` as the drive's medium.
    fn load(&mut self, path: &str) -> io::Result<()>;
    /// Empty the drive. Fails with EBUSY while the computer holds the medium.
    fn unload(&mut self) -> io::Result<()>;
    fn host(&self) -> HostLink;
    /// Drop the USB connection and make it again, as a cable re-plug does. The
    /// computer's hold on the medium ends with it.
    fn replug(&mut self) -> io::Result<()>;
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
    /// No medium was loaded: nothing to take back.
    Nothing,
    /// A medium was loaded and is unloaded now; the volume is mounted.
    Reclaimed,
    /// The computer still holds the medium: the volume stays unmounted and
    /// USB storage stays on.
    StillExported,
    /// Unloaded, but the volume could not be mounted.
    NotMounted,
}

/// `WAVED_USB_REPLUG=1`: at start-up, re-plug USB (`soft_connect`) when the
/// computer still holds a drive a previous waved left loaded. Off until the
/// device pass shows `soft_connect` brings USB, adb included, back on the
/// `crgudc2` controller; without it the drive stays with the computer until
/// it is ejected there or the cable is pulled.
pub const REPLUG_ENV: &str = "WAVED_USB_REPLUG";

/// Whether `REPLUG_ENV` switches the start-up re-plug on: only "1".
pub fn replug_opted_in(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
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
    /// `reclaim_at_start` may re-plug USB; cleared after a re-plug that did
    /// not bring the computer back.
    replug: bool,
    /// Firmware updates (`with_ota`).
    ota: Option<watch::Receiver<OtaState>>,
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
            replug: false,
            ota: None,
            sleep: Box::new(std::thread::sleep),
        }
    }

    /// The device: configfs gadget, `/mnt/udisk` and `/dev/udisk`. The
    /// start-up re-plug follows `REPLUG_ENV`.
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
        .with_replug(replug_opted_in(std::env::var(REPLUG_ENV).ok().as_deref()))
    }

    /// Replace the pauses (`SETTLE`, the re-plug's waits) in tests.
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

    fn installing_update(&self) -> bool {
        self.ota
            .as_ref()
            .is_some_and(|ota| ota.borrow().phase.takes_over_screen())
    }

    /// Let `reclaim_at_start` re-plug USB (`soft_connect`) to end a
    /// computer's hold on the drive. Off by default (`REPLUG_ENV`).
    pub fn with_replug(mut self, on: bool) -> Self {
        self.replug = on;
        self
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
    /// installs, with no computer, with no drive in the gadget or with the
    /// volume not mounted here; a busy volume leaves everything as it was.
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
        if !self.gadget.drive_ready() {
            warn!(
                "usb storage: not started, the gadget has no removable drive linked \
                 (run scripts/patch-stock-usbgadget.sh and reboot)"
            );
            return self.problem(UsbStorageProblem::Unavailable);
        }
        match self.gadget.loaded() {
            Ok(None) => {}
            Ok(Some(medium)) => {
                warn!("usb storage: not started, the drive already holds {medium}");
                return self.problem(UsbStorageProblem::Unavailable);
            }
            Err(error) => {
                warn!("usb storage: not started, the drive cannot be read: {error}");
                return self.problem(UsbStorageProblem::Unavailable);
            }
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

        self.publish(UsbStoragePhase::Starting);
        let _ = self.commands_tx.send(Command::StopPlayback);
        (self.sleep)(SETTLE);
        self.volume.sync();
        // Before the unmount: a waved that dies from here on leaves the note,
        // and the next one mounts the volume again (`reclaim_at_start`).
        if let Err(error) = self.volume.note_unmount(&spec) {
            warn!("usb storage: not started, the unmount cannot be noted: {error}");
            self.publish(UsbStoragePhase::Off);
            return self.problem(UsbStorageProblem::Unavailable);
        }
        if let Err(error) = self.volume.unmount() {
            info!("usb storage: not started, {MOUNT_POINT} is busy: {error}");
            self.volume.clear_unmount_note();
            self.publish(UsbStoragePhase::Off);
            return self.problem(UsbStorageProblem::Busy);
        }
        self.mount = spec;
        let loaded = self
            .gadget
            .load(&self.partition)
            .and_then(|()| self.gadget.loaded());
        match loaded {
            Ok(Some(medium)) => {
                info!("usb storage: on, the computer has {medium}");
                self.gone_polls = 0;
                self.publish(UsbStoragePhase::On);
            }
            outcome => {
                error!("usb storage: the drive did not take {}: {outcome:?}", self.partition);
                // Never mount while the LUN might still hold the partition.
                if !self.unload_verified() {
                    error!("usb storage: the drive cannot be emptied; the recordings stay unmounted");
                    self.publish(UsbStoragePhase::On);
                    return self.problem(UsbStorageProblem::Unavailable);
                }
                self.problem(UsbStorageProblem::Unavailable);
                self.mount_back(false);
            }
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
        if !self.unload_verified() {
            info!("usb storage: still on, the computer holds the drive");
            self.publish(UsbStoragePhase::On);
            return self.problem(UsbStorageProblem::EjectFirst);
        }
        info!("usb storage: stopped on Wave");
        self.mount_back(true);
    }

    /// Once a second: the computer ejected the drive (the LUN reads empty), or
    /// the cable is out (no computer for `GONE_POLLS` polls). A sleeping
    /// computer keeps its drive.
    pub fn poll(&mut self) {
        if self.phase() != UsbStoragePhase::On {
            return;
        }
        if let Ok(None) = self.gadget.loaded() {
            info!("usb storage: the computer ejected the drive");
            self.publish(UsbStoragePhase::Stopping);
            self.mount_back(true);
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
        // The disconnect ended the computer's hold on the medium.
        if self.unload_verified() {
            info!("usb storage: the cable is out, taking the recordings back");
            self.publish(UsbStoragePhase::Stopping);
            self.mount_back(true);
        } else {
            warn!("usb storage: no computer, but the drive cannot be emptied yet");
        }
    }

    /// At start-up, before the meeting service opens the recordings: take
    /// back what a waved that exited during USB storage left behind. A drive
    /// still loaded is emptied (if the computer still holds it, it is
    /// re-plugged when that is switched on); a volume that waved unmounted
    /// (the unmount note) is checked and mounted again. A medium loaded in an
    /// unlinked drive (stock's `msc_init` at boot) is only emptied: the
    /// computer never saw it. Without a note an unmounted volume is the
    /// boot's to mount: stock's `udisk_fix` may still be checking it.
    pub fn reclaim_at_start(&mut self) -> Reclaim {
        let note = self.volume.unmount_noted();
        if let Some(spec) = &note {
            self.mount = spec.clone();
        }
        let exported = match self.gadget.loaded() {
            Ok(Some(medium)) if self.gadget.drive_ready() => {
                warn!("usb storage: the drive still held {medium} at start-up; taking the recordings back");
                if !self.release_at_start() {
                    warn!("usb storage: the computer still holds the drive; storage stays on, the recordings unmounted");
                    self.publish(UsbStoragePhase::On);
                    return Reclaim::StillExported;
                }
                true
            }
            Ok(Some(medium)) => {
                if let Err(error) = self.gadget.unload() {
                    warn!("usb storage: could not empty the unlinked drive ({medium}): {error}");
                }
                false
            }
            Ok(None) | Err(_) => false,
        };
        match self.volume.mounted() {
            Ok(Some(_)) => {
                if exported {
                    // Mounted here while exported: the LUN is empty now, so
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

    /// Empty the drive the computer may still hold, at start-up. With the
    /// re-plug switched on, drop and remake the USB connection to end its
    /// hold. False while it still holds the drive.
    fn release_at_start(&mut self) -> bool {
        if self.unload_verified() {
            return true;
        }
        if !self.replug {
            warn!(
                "usb storage: the computer holds the drive; it keeps it until it is ejected \
                 there or the cable is pulled (start-up re-plug is off, {REPLUG_ENV}=1 turns it on)"
            );
            return false;
        }
        info!("usb storage: the computer holds the drive; re-plugging USB to release it");
        if let Err(error) = self.gadget.replug() {
            error!(
                "usb storage: USB did not come back after the re-plug ({error}); Wave may be off \
                 USB, adb included, until the cable is re-plugged. No more re-plugs."
            );
            self.replug = false;
        }
        for _ in 0..RELEASE_TRIES {
            (self.sleep)(RETRY_PAUSE);
            if self.unload_verified() {
                return true;
            }
        }
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
    use waved_state::{OtaPhase, OtaState, PendingDecision};

    /// One world for both fakes, so they can check the rule this module
    /// exists for: the computer and Wave never both have the volume.
    #[derive(Debug)]
    struct World {
        linked: bool,
        lun: Option<String>,
        host: HostLink,
        /// The computer prevents medium removal (the drive is in use there).
        locked: bool,
        /// A re-plug ends the computer's hold.
        replug_releases: bool,
        /// USB does not come back after the re-plug.
        replug_fails: bool,
        mounted: Option<MountSpec>,
        /// Something has a file open on the volume.
        busy: bool,
        load_fails: bool,
        /// Mount attempts that fail before one succeeds.
        mount_failures: u32,
        /// `mount` exits 0, but `/proc/mounts` does not show the volume.
        mount_vanishes: bool,
        /// The check runs past its time and is stopped.
        check_times_out: bool,
        /// The tmpfs note that waved took the volume off Wave.
        note: Option<MountSpec>,
        log: Vec<String>,
    }

    impl Default for World {
        fn default() -> Self {
            Self {
                linked: true,
                lun: None,
                host: HostLink::Configured,
                locked: false,
                replug_releases: true,
                replug_fails: false,
                mounted: Some(live_mount()),
                busy: false,
                load_fails: false,
                mount_failures: 0,
                mount_vanishes: false,
                check_times_out: false,
                note: None,
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

    type Shared = Arc<Mutex<World>>;

    struct FakeGadget(Shared);
    struct FakeVolume(Shared);

    impl Gadget for FakeGadget {
        fn drive_ready(&self) -> bool {
            self.0.lock().unwrap().linked
        }
        fn loaded(&self) -> io::Result<Option<String>> {
            Ok(self.0.lock().unwrap().lun.clone())
        }
        fn load(&mut self, path: &str) -> io::Result<()> {
            let mut w = self.0.lock().unwrap();
            w.log.push(format!("load {path}"));
            assert!(w.mounted.is_none(), "exported while mounted on Wave");
            if w.load_fails {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            w.lun = Some(path.to_string());
            Ok(())
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
        fn replug(&mut self) -> io::Result<()> {
            let mut w = self.0.lock().unwrap();
            w.log.push("replug".into());
            if w.replug_releases {
                w.locked = false;
            }
            if w.replug_fails {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no computer after the re-plug"));
            }
            Ok(())
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
            let world = Arc::new(Mutex::new(world));
            let state_tx = Arc::new(watch::channel(UsbStorageState::default()).0);
            let state = state_tx.subscribe();
            let (events_tx, events) = broadcast::channel(16);
            let (commands_tx, commands) = broadcast::channel(16);
            let storage = UsbStorage::new(
                Box::new(FakeGadget(Arc::clone(&world))),
                Box::new(FakeVolume(Arc::clone(&world))),
                state_tx,
                events_tx,
                commands_tx,
            )
            .with_sleep(|_| {});
            Self {
                world,
                storage,
                state,
                events,
                commands,
            }
        }

        /// Exported, as after a start.
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

    #[test]
    fn start_unmounts_first_then_hands_the_partition_to_the_computer() {
        let mut rig = Rig::new(World::default());
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::On);
        assert_eq!(rig.log(), ["sync", "unmount", "load /dev/udisk"]);
        assert_eq!(rig.sent(), [Command::StopPlayback], "playback lets go of its file first");
        assert!(rig.problems().is_empty());
        assert!(rig.world().mounted.is_none());
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
    fn a_gadget_without_the_drive_refuses_to_start() {
        let mut rig = Rig::new(World { linked: false, ..World::default() });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert!(rig.log().is_empty());
        assert_eq!(rig.problems(), [UsbStorageProblem::Unavailable]);
    }

    #[test]
    fn a_drive_that_already_holds_a_medium_refuses_to_start() {
        let mut rig = Rig::new(World {
            lun: Some("/dev/mmcblk0p16".into()),
            ..World::default()
        });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert!(rig.log().is_empty());
        assert_eq!(rig.problems(), [UsbStorageProblem::Unavailable]);
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
    }

    #[test]
    fn a_drive_that_refuses_the_partition_mounts_the_volume_again() {
        let mut rig = Rig::new(World { load_fails: true, ..World::default() });
        rig.storage.start(&idle());
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(
            rig.log(),
            [
                "sync",
                "unmount",
                "load /dev/udisk",
                "unload",
                "check /dev/udisk",
                "mount rw,relatime,fmask=0077,dmask=0022"
            ]
        );
        assert_eq!(rig.problems(), [UsbStorageProblem::Unavailable]);
        assert_eq!(rig.world().mounted, Some(live_mount()), "mounted as it was");
    }

    #[test]
    fn stop_empties_the_drive_checks_and_mounts_as_before() {
        let mut rig = Rig::on(World::default());
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(
            rig.log(),
            ["unload", "check /dev/udisk", "mount rw,relatime,fmask=0077,dmask=0022"]
        );
        assert!(rig.problems().is_empty());
        assert_eq!(rig.world().mounted, Some(live_mount()));
    }

    #[test]
    fn stop_while_the_computer_holds_the_drive_asks_to_eject_there_first() {
        let mut rig = Rig::on(World::default());
        rig.world().locked = true;
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::On);
        assert_eq!(rig.log(), ["unload"], "never forced, never mounted");
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
        assert!(rig.world().mounted.is_some());
    }

    #[test]
    fn the_computer_ejecting_the_drive_ends_storage() {
        let mut rig = Rig::on(World::default());
        rig.world().lun = None;
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(
            rig.log(),
            ["check /dev/udisk", "mount rw,relatime,fmask=0077,dmask=0022"]
        );
        assert!(rig.problems().is_empty());
    }

    #[test]
    fn a_pulled_cable_ends_storage_after_three_polls() {
        let mut rig = Rig::on(World { locked: true, ..World::default() });
        {
            // The disconnect ends the computer's hold on the medium.
            let mut world = rig.world();
            world.host = HostLink::Gone;
            world.locked = false;
        }
        rig.storage.poll();
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::On, "a re-enumeration is not a pull");
        assert!(rig.log().is_empty());
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(
            rig.log(),
            ["unload", "check /dev/udisk", "mount rw,relatime,fmask=0077,dmask=0022"]
        );
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
    fn a_failed_mount_falls_back_to_the_boot_mount() {
        let mut rig = Rig::on(World::default());
        rig.world().mount_failures = 1;
        rig.storage.stop();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert_eq!(
            rig.log(),
            [
                "unload",
                "check /dev/udisk",
                "mount rw,relatime,fmask=0077,dmask=0022",
                "mount rw,fmask=0077"
            ]
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
    fn reclaim_with_an_empty_drive_does_nothing() {
        let mut rig = Rig::new(World::default());
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Nothing);
        assert!(rig.log().is_empty());
    }

    #[test]
    fn reclaim_takes_back_a_drive_left_loaded() {
        let mut rig = Rig::new(World {
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(rig.log(), ["unload", "check /dev/udisk", "mount rw,fmask=0077"]);
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
    }

    #[test]
    fn reclaim_re_plugs_a_computer_that_holds_the_drive() {
        let mut rig = Rig::new(World {
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            locked: true,
            ..World::default()
        });
        rig.storage = rig.storage.with_replug(true);
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(
            rig.log(),
            ["unload", "replug", "unload", "check /dev/udisk", "mount rw,fmask=0077"]
        );
    }

    #[test]
    fn without_the_switch_reclaim_waits_for_the_computer_to_eject() {
        // Re-plugging is off until the device pass shows `soft_connect`
        // brings USB (and adb) back on this controller.
        let mut rig = Rig::new(World {
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            locked: true,
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::StillExported);
        assert_eq!(rig.log(), ["unload"], "never re-plugged");
        assert_eq!(rig.phase(), UsbStoragePhase::On, "the takeover shows; eject ends it");
        {
            let mut world = rig.world();
            world.locked = false;
            world.lun = None;
        }
        rig.storage.poll();
        assert_eq!(rig.phase(), UsbStoragePhase::Off);
        assert!(rig.world().mounted.is_some());
    }

    #[test]
    fn a_re_plug_that_does_not_bring_usb_back_is_not_tried_again() {
        let mut rig = Rig::new(World {
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            locked: true,
            replug_releases: false,
            replug_fails: true,
            ..World::default()
        });
        rig.storage = rig.storage.with_replug(true);
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::StillExported);
        assert_eq!(rig.log().iter().filter(|op| *op == "replug").count(), 1);
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::StillExported);
        assert!(!rig.log().iter().any(|op| op == "replug"), "no second unplug");
    }

    #[test]
    fn the_re_plug_switch_is_off_unless_set_to_1() {
        assert!(!replug_opted_in(None));
        assert!(!replug_opted_in(Some("")));
        assert!(!replug_opted_in(Some("0")));
        assert!(!replug_opted_in(Some("yes")));
        assert!(replug_opted_in(Some("1")));
        assert!(replug_opted_in(Some(" 1\n")));
    }

    #[test]
    fn reclaim_never_mounts_while_the_computer_keeps_the_drive() {
        let mut rig = Rig::new(World {
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            locked: true,
            replug_releases: false,
            ..World::default()
        });
        rig.storage = rig.storage.with_replug(true);
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::StillExported);
        assert_eq!(rig.phase(), UsbStoragePhase::On, "the takeover shows; stop or eject ends it");
        assert!(rig.world().mounted.is_none());
        assert!(!rig.log().iter().any(|op| op.starts_with("mount")));
    }

    #[test]
    fn reclaim_only_empties_a_drive_the_computer_never_saw() {
        // Stock's msc_init loads the partition at boot before the patch
        // empties it and links the drive; the boot mounts the volume itself.
        let mut rig = Rig::new(World {
            linked: false,
            lun: Some("/dev/mmcblk0p16".into()),
            mounted: None,
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Nothing);
        assert_eq!(rig.log(), ["unload"]);
        assert!(rig.world().mounted.is_none());
    }

    #[test]
    fn reclaim_empties_a_drive_exported_while_mounted() {
        let mut rig = Rig::new(World::default());
        rig.world().lun = Some("/dev/mmcblk0p16".into());
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(rig.log(), ["unload"], "already mounted: nothing to mount");
        assert!(rig.world().lun.is_none());
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
        assert_eq!(rig.log(), ["unload", "check /dev/udisk"], "no mount");
        assert_eq!(rig.phase(), UsbStoragePhase::Stopping);
        assert_eq!(rig.problems(), [UsbStorageProblem::NotMounted]);
        assert!(rig.world().note.is_some(), "a restart checks it again");
    }

    #[test]
    fn reclaim_that_cannot_mount_keeps_the_recordings_away() {
        let mut rig = Rig::new(World {
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
        // waved died between the unmount and the load, or between emptying
        // the drive and the mount (the check takes up to 120 s): the drive is
        // empty, the volume unmounted, and the note says waved owes the mount.
        let mut rig = Rig::new(World {
            mounted: None,
            note: Some(live_mount()),
            ..World::default()
        });
        assert_eq!(rig.storage.reclaim_at_start(), Reclaim::Reclaimed);
        assert_eq!(
            rig.log(),
            ["check /dev/udisk", "mount rw,relatime,fmask=0077,dmask=0022"],
            "mounted as the note says"
        );
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
