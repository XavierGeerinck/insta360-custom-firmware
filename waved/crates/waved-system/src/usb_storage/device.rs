//! The device side of USB storage: stock `S89usbgadget`'s mode switch on the
//! configfs gadget, adbd, the UDC state files and the recordings mount
//! (`docs/USB_STORAGE.md`).

use super::{Gadget, GadgetMode, HostLink, MountSpec, Volume, MOUNT_POINT};
use std::ffi::CString;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const GADGET_DIR: &str = "/sys/kernel/config/usb_gadget/amlogic";
/// The controller stock's `usb_flash` binds.
pub const UDC_NAME: &str = "fdd00000.crgudc2";
pub const PROC_MOUNTS: &str = "/proc/mounts";
/// exfat-utils' `exfatfsck` (`/sbin/fsck.exfat` links to it): `-p` repairs
/// what is safe without asking.
pub const FSCK: &str = "/sbin/fsck.exfat";
pub const MOUNT: &str = "/bin/mount";
/// waved's note that it took the recordings off Wave (`Volume::note_unmount`).
/// `/tmp` is tmpfs: a reboot clears it, and the boot mounts the volume.
pub const UNMOUNT_NOTE: &str = "/tmp/waved-usb-storage.unmounted";
/// waved's note of the gadget's normal mode: written before a switch to
/// storage mode changes anything, removed once normal USB is bound again. A
/// waved that restarts in between finds it and comes back to that mode. On
/// tmpfs: a reboot clears it, and the boot builds the gadget from scratch.
pub const GADGET_NOTE: &str = "/tmp/waved-usb-storage.gadget";
/// A full check of the 21 GB volume takes seconds; past this it is stopped,
/// and the volume is not mounted (it may be half repaired).
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(120);
pub const ADBD: &str = "/usr/bin/adbd";
/// How waved starts adbd: busybox `start-stop-daemon` (`/sbin`, stock
/// rootfs). `-b` forks and detaches it (setsid, stdio on /dev/null, other
/// descriptors closed), so it outlives waved and holds none of waved's files;
/// `-x` refuses a second one (`-o`: that is not an error).
pub const ADBD_LAUNCH: [&str; 5] = ["/sbin/start-stop-daemon", "-S", "-b", "-o", "-x"];

/// The gadget's only configuration.
const CONFIG: &str = "configs/amlogic.1";
const CONFIGURATION: &str = "configs/amlogic.1/strings/0x409/configuration";
const DRIVE: &str = "mass_storage.usb0";
const ADB: &str = "ffs.adb";
const SERIAL: &str = "acm.0";
const HID: &str = "hid.0";
/// What stock's `all_disable` unlinks (never `uac2.0`), in the order stock's
/// modes link them back.
const SWITCHED: [&str; 4] = [ADB, SERIAL, DRIVE, HID];
/// Stock's `usb_flash` waits this long before it binds.
const BIND_PAUSE: Duration = Duration::from_secs(2);
/// Before the bind on the way back (stock's boot: `sleep 4`): FunctionFS
/// binds only once the fresh adbd has written its descriptors.
const ADBD_PAUSE: Duration = Duration::from_secs(4);
const ADBD_GONE_WAIT: Duration = Duration::from_secs(2);
const ADBD_POLL: Duration = Duration::from_millis(100);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Write an existing attribute file, as `echo value > path` does (never
/// creates it).
fn write_attribute(path: &Path, value: &str) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(value.as_bytes())
}

fn read_trimmed(path: &Path) -> io::Result<String> {
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

/// The gadget's normal mode, to come back to after storage mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalMode {
    /// The controller it was bound to.
    pub udc: String,
    /// `configs/amlogic.1/strings/0x409/configuration`.
    pub configuration: String,
    /// Which of adb, the serial port and HID were linked.
    pub links: Vec<String>,
}

impl NormalMode {
    /// Wave's boot gadget on a development unit (`S89usbgadget start` with
    /// adb on): audio, adb and HID. The way back uses it when no note says
    /// otherwise: a unit must never lose adb (`docs/SECURITY.md`).
    pub fn boot() -> Self {
        Self {
            udc: UDC_NAME.into(),
            configuration: "hid".into(),
            links: vec![ADB.into(), HID.into()],
        }
    }

    fn to_note(&self) -> String {
        format!(
            "udc={}\nconfiguration={}\nlinks={}\n",
            self.udc,
            self.configuration,
            self.links.join(" ")
        )
    }

    /// `None` for a note cut short. Only what storage mode unlinks is
    /// linked back.
    fn from_note(text: &str) -> Option<Self> {
        let (mut udc, mut configuration, mut links) = (None, None, None);
        for line in text.lines() {
            match line.split_once('=') {
                Some(("udc", value)) => udc = Some(value.trim().to_string()),
                Some(("configuration", value)) => configuration = Some(value.trim().to_string()),
                Some(("links", value)) => {
                    links = Some(
                        value
                            .split_whitespace()
                            .filter(|link| SWITCHED.contains(link) && *link != DRIVE)
                            .map(String::from)
                            .collect(),
                    )
                }
                _ => {}
            }
        }
        Some(Self {
            udc: udc.filter(|udc| !udc.is_empty())?,
            configuration: configuration?,
            links: links?,
        })
    }
}

/// adbd, the daemon behind adb's FunctionFS function (`ffs.adb`).
pub trait Adbd: Send {
    /// Whether an adbd runs.
    fn running(&self) -> bool;
    /// `killall -9 adbd`, then wait (bounded) until none is left.
    fn stop(&mut self);
    /// A fresh adbd that outlives waved. Never a second one.
    fn start(&mut self) -> io::Result<()>;
}

/// adbd through `/proc` and `ADBD_LAUNCH`.
pub struct SystemAdbd {
    proc_dir: PathBuf,
    program: String,
    launch: Vec<String>,
    signal: Box<dyn Fn(i32) + Send>,
    sleep: Box<dyn Fn(Duration) + Send>,
}

impl SystemAdbd {
    pub fn new(proc_dir: impl Into<PathBuf>, program: &str, launch: &[&str]) -> Self {
        Self {
            proc_dir: proc_dir.into(),
            program: program.into(),
            launch: launch.iter().map(|arg| arg.to_string()).collect(),
            signal: Box::new(|pid| {
                // SAFETY: kill(2) with a pid read from /proc.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }),
            sleep: Box::new(std::thread::sleep),
        }
    }

    pub fn device() -> Self {
        Self::new("/proc", ADBD, &ADBD_LAUNCH)
    }

    /// Replace the SIGKILL in tests.
    pub fn with_signal(mut self, signal: impl Fn(i32) + Send + 'static) -> Self {
        self.signal = Box::new(signal);
        self
    }

    /// Replace the pauses in tests.
    pub fn with_sleep(mut self, sleep: impl Fn(Duration) + Send + 'static) -> Self {
        self.sleep = Box::new(sleep);
        self
    }

    /// The running adbds: `/proc/<pid>/comm` is `adbd`, not a zombie.
    fn pids(&self) -> Vec<i32> {
        let mut pids: Vec<i32> = std::fs::read_dir(&self.proc_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
                let dir = entry.path();
                let comm = std::fs::read_to_string(dir.join("comm")).ok()?;
                (comm.trim() == "adbd" && !zombie(&dir)).then_some(pid)
            })
            .collect();
        pids.sort_unstable();
        pids
    }
}

impl Adbd for SystemAdbd {
    fn running(&self) -> bool {
        !self.pids().is_empty()
    }

    fn stop(&mut self) {
        let pids = self.pids();
        if pids.is_empty() {
            return;
        }
        for pid in &pids {
            (self.signal)(*pid);
        }
        let mut waited = Duration::ZERO;
        while self.running() && waited < ADBD_GONE_WAIT {
            (self.sleep)(ADBD_POLL);
            waited += ADBD_POLL;
        }
        if self.running() {
            tracing::warn!(
                "usb storage: adbd {:?} still there {} s after SIGKILL",
                self.pids(),
                ADBD_GONE_WAIT.as_secs()
            );
        }
    }

    fn start(&mut self) -> io::Result<()> {
        if self.running() {
            tracing::info!("usb storage: adbd {:?} already runs; not starting another", self.pids());
            return Ok(());
        }
        let (program, args) = self
            .launch
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no adbd launcher"))?;
        // Every stdio on /dev/null: adbd holds no pipe of waved's open, and
        // the launcher returns as soon as it forked.
        let mut child = Command::new(program)
            .args(args)
            .arg(&self.program)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut waited = Duration::ZERO;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if waited >= LAUNCH_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{program} still running after {} s; stopped", LAUNCH_TIMEOUT.as_secs()),
                ));
            }
            std::thread::sleep(ADBD_POLL);
            waited += ADBD_POLL;
        };
        if status.success() {
            tracing::info!("usb storage: started a fresh {}", self.program);
            Ok(())
        } else {
            Err(io::Error::other(format!("{program} {}: {status}", self.program)))
        }
    }
}

/// The configfs gadget (stock `S89usbgadget`'s functions and configuration),
/// and the UDCs under `/sys/class/udc`.
pub struct ConfigfsGadget {
    root: PathBuf,
    udc_dir: PathBuf,
    note: PathBuf,
    adbd: Box<dyn Adbd>,
    sleep: Box<dyn Fn(Duration) + Send>,
    /// The normal mode noted on the way in.
    normal: Option<NormalMode>,
}

/// `/proc/<pid>/stat`'s state is Z.
fn zombie(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

impl ConfigfsGadget {
    pub fn new(
        root: impl Into<PathBuf>,
        udc_dir: impl Into<PathBuf>,
        note: impl Into<PathBuf>,
        adbd: Box<dyn Adbd>,
    ) -> Self {
        Self {
            root: root.into(),
            udc_dir: udc_dir.into(),
            note: note.into(),
            adbd,
            sleep: Box::new(std::thread::sleep),
            normal: None,
        }
    }

    pub fn device() -> Self {
        Self::new(
            GADGET_DIR,
            crate::usb::UDC_DIR,
            GADGET_NOTE,
            Box::new(SystemAdbd::device()),
        )
    }

    /// Replace the pauses before the binds in tests.
    pub fn with_sleep(mut self, sleep: impl Fn(Duration) + Send + 'static) -> Self {
        self.sleep = Box::new(sleep);
        self
    }

    fn lun(&self) -> PathBuf {
        self.root.join("functions").join(DRIVE).join("lun.0")
    }

    fn link_path(&self, function: &str) -> PathBuf {
        self.root.join(CONFIG).join(function)
    }

    fn linked(&self, function: &str) -> bool {
        std::fs::symlink_metadata(self.link_path(function)).is_ok()
    }

    /// stock: `ln -s $G/functions/<f> $G/configs/amlogic.1/<f>`.
    fn link(&self, function: &str) -> io::Result<()> {
        let target = self.root.join("functions").join(function);
        match std::os::unix::fs::symlink(target, self.link_path(function)) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            outcome => outcome,
        }
    }

    /// stock: `rm -rf $G/configs/amlogic.1/<f>`.
    fn unlink(&self, function: &str) -> io::Result<()> {
        match std::fs::remove_file(self.link_path(function)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            outcome => outcome,
        }
    }

    fn set_configuration(&self, name: &str) -> io::Result<()> {
        write_attribute(&self.root.join(CONFIGURATION), name)
    }

    /// The controller the gadget is bound to. configfs reads `UDC` empty
    /// once unbound; a plain file (tests) keeps the "none" written to it.
    fn bound_udc(&self) -> Option<String> {
        let udc = read_trimmed(&self.root.join("UDC")).ok()?;
        (!udc.is_empty() && udc != "none").then_some(udc)
    }

    /// stock: `echo none > $G/UDC`. Already unbound is fine.
    fn unbind(&self) -> io::Result<()> {
        if self.bound_udc().is_none() {
            return Ok(());
        }
        let written = write_attribute(&self.root.join("UDC"), "none");
        match self.bound_udc() {
            None => Ok(()),
            Some(udc) => Err(written
                .err()
                .unwrap_or_else(|| io::Error::other(format!("still bound to {udc} after \"none\"")))),
        }
    }

    /// stock: `echo fdd00000.crgudc2 > $G/UDC`, read back.
    fn bind(&self, udc: &str) -> io::Result<()> {
        let written = write_attribute(&self.root.join("UDC"), udc);
        match self.bound_udc() {
            Some(bound) if bound == udc => Ok(()),
            bound => Err(written
                .err()
                .unwrap_or_else(|| io::Error::other(format!("UDC reads {bound:?} after binding {udc}")))),
        }
    }

    /// The mode to come back to, read from the bound gadget.
    fn normal_now(&self) -> io::Result<NormalMode> {
        let udc = self
            .bound_udc()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "the gadget is not bound"))?;
        Ok(NormalMode {
            udc,
            configuration: read_trimmed(&self.root.join(CONFIGURATION))?,
            links: [ADB, SERIAL, HID]
                .into_iter()
                .filter(|function| self.linked(function))
                .map(String::from)
                .collect(),
        })
    }

    /// The note a waved left; Wave's boot mode without one.
    fn noted_normal(&self) -> NormalMode {
        match std::fs::read_to_string(&self.note) {
            Ok(text) => NormalMode::from_note(&text).unwrap_or_else(|| {
                tracing::warn!("usb storage: {} is cut short; back to the boot mode", self.note.display());
                NormalMode::boot()
            }),
            Err(_) => {
                tracing::warn!("usb storage: no note of the normal USB mode; back to the boot mode (audio, adb, HID)");
                NormalMode::boot()
            }
        }
    }

    /// Stock's `all_disable`: unbound, adbd stopped, adb, the serial port,
    /// the drive and HID unlinked (`uac2.0` stays).
    fn all_disable(&mut self) -> io::Result<()> {
        self.unbind()?;
        self.adbd.stop();
        for function in SWITCHED {
            self.unlink(function)?;
        }
        Ok(())
    }

    /// Every UDC's directory.
    fn controllers(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.udc_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect()
    }
}

impl Gadget for ConfigfsGadget {
    fn mode(&self) -> GadgetMode {
        if !self.lun().join("file").exists() {
            return GadgetMode::Unavailable;
        }
        if self.linked(DRIVE) {
            return GadgetMode::Storage;
        }
        if self.bound_udc().is_some() {
            return GadgetMode::Normal;
        }
        if self.note.exists() {
            GadgetMode::Storage
        } else {
            GadgetMode::Unbound
        }
    }

    fn enter_storage(&mut self, partition: &str) -> io::Result<()> {
        let normal = self.normal_now()?;
        std::fs::write(&self.note, normal.to_note())?;
        self.normal = Some(normal.clone());
        self.all_disable()?;
        // msc_enable, with the drive removable: the computer's eject (START
        // STOP UNIT) only empties a removable LUN.
        let lun = self.lun();
        if read_trimmed(&lun.join("removable")).ok().as_deref() != Some("1") {
            write_attribute(&lun.join("removable"), "1")?;
        }
        write_attribute(&lun.join("file"), partition)?;
        if self.loaded()?.is_none() {
            return Err(io::Error::other(format!("the drive did not take {partition}")));
        }
        self.set_configuration("msc")?;
        self.link(DRIVE)?;
        // hid_enable
        self.set_configuration("hid")?;
        self.link(HID)?;
        // usb_flash
        (self.sleep)(BIND_PAUSE);
        self.bind(&normal.udc)
    }

    fn leave_storage(&mut self) -> io::Result<()> {
        let normal = match &self.normal {
            Some(normal) => normal.clone(),
            // A switch that stopped before its first change: nothing to undo.
            None if self.mode() == GadgetMode::Normal => return Ok(()),
            None => self.noted_normal(),
        };
        self.all_disable()?;
        // Unbound, the computer's hold on the drive is gone: it empties.
        self.unload()?;
        if let Some(medium) = self.loaded()? {
            return Err(io::Error::other(format!("the drive still holds {medium}")));
        }
        for function in SWITCHED {
            if !normal.links.iter().any(|link| link == function) {
                continue;
            }
            let configuration = match function {
                ADB => {
                    // adbd_enable: a fresh adbd before the link. Without its
                    // daemon FunctionFS would fail the whole bind, so a
                    // failed start leaves adb out.
                    if let Err(error) = self.adbd.start() {
                        tracing::error!("usb storage: adbd did not start ({error}); binding without adb");
                        continue;
                    }
                    "adb"
                }
                SERIAL => "acm",
                _ => "hid",
            };
            self.set_configuration(configuration)?;
            self.link(function)?;
        }
        self.set_configuration(&normal.configuration)?;
        (self.sleep)(ADBD_PAUSE);
        self.bind(&normal.udc)?;
        self.normal = None;
        if let Err(error) = std::fs::remove_file(&self.note) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!("usb storage: could not remove {}: {error}", self.note.display());
            }
        }
        Ok(())
    }

    fn loaded(&self) -> io::Result<Option<String>> {
        let file = std::fs::read_to_string(self.lun().join("file"))?;
        let file = file.trim();
        Ok((!file.is_empty()).then(|| file.to_string()))
    }

    fn unload(&mut self) -> io::Result<()> {
        // f_mass_storage drops the trailing newline; an empty name closes the
        // medium. A zero-byte write would not reach it.
        write_attribute(&self.lun().join("file"), "\n")
    }

    fn host(&self) -> HostLink {
        let states: Vec<String> = self
            .controllers()
            .iter()
            .filter_map(|udc| std::fs::read_to_string(udc.join("state")).ok())
            .collect();
        HostLink::from_states(states.iter().map(String::as_str))
    }
}

/// The recordings mount, through `/proc/mounts`, umount(2), `fsck.exfat` and
/// busybox `mount`.
pub struct SystemVolume {
    target: PathBuf,
    proc_mounts: PathBuf,
    note: PathBuf,
}

impl SystemVolume {
    pub fn new(target: impl Into<PathBuf>, proc_mounts: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            proc_mounts: proc_mounts.into(),
            note: UNMOUNT_NOTE.into(),
        }
    }

    pub fn device() -> Self {
        Self::new(MOUNT_POINT, PROC_MOUNTS)
    }

    /// Keep the unmount note at `note` instead of `UNMOUNT_NOTE`.
    pub fn with_note(mut self, note: impl Into<PathBuf>) -> Self {
        self.note = note.into();
        self
    }
}

/// The last `/proc/mounts` entry on `target` (the one on top).
pub fn mount_on(mounts: &str, target: &Path) -> Option<MountSpec> {
    mounts
        .lines()
        .rev()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let spec = MountSpec {
                source: unescape(fields.next()?),
                target: unescape(fields.next()?),
                fstype: fields.next()?.to_string(),
                options: fields.next()?.to_string(),
            };
            (Path::new(&spec.target) == target).then_some(spec)
        })
}

/// `/proc/mounts` writes space, tab, newline and backslash as octal escapes.
fn escape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for c in field.chars() {
        match c {
            ' ' | '\t' | '\n' | '\\' => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 4 <= bytes.len()
            && bytes[i + 1..i + 4].iter().all(|b| (b'0'..=b'7').contains(b))
        {
            let value = (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(value);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Run `program` with `args`, stdin closed, for at most `timeout` (then it is
/// killed). Returns whether it exited 0, and its status with its last line of
/// output.
fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> io::Result<(bool, String)> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    // The pipes are read while the program runs, so a chatty one never blocks.
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(timeout) {
        Ok(output) => output?,
        Err(_) => {
            // SAFETY: kill(2) on the child's pid; it has not been reaped yet,
            // because the waiting thread is still waiting for it.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{program} still running after {} s; stopped", timeout.as_secs_f32()),
            ));
        }
    };
    let text = [output.stdout, output.stderr]
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let last = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string();
    Ok((output.status.success(), format!("{}: {last}", output.status)))
}

/// umount(2) without MNT_DETACH: a lazy unmount would leave open files
/// writing while the computer has the volume. A busy volume fails with EBUSY.
#[cfg(target_os = "linux")]
fn umount_now(target: &CString) -> io::Result<()> {
    // SAFETY: a valid NUL-terminated path.
    if unsafe { libc::umount2(target.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn umount_now(_target: &CString) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "unmount is Linux-only"))
}

impl Volume for SystemVolume {
    fn sync(&mut self) {
        // SAFETY: sync(2) takes no arguments and cannot fail.
        unsafe { libc::sync() };
    }

    fn mounted(&self) -> io::Result<Option<MountSpec>> {
        let mounts = std::fs::read_to_string(&self.proc_mounts)?;
        Ok(mount_on(&mounts, &self.target))
    }

    fn unmount(&mut self) -> io::Result<()> {
        let target = CString::new(self.target.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "mount point has a NUL"))?;
        umount_now(&target)
    }

    fn check(&mut self, source: &str) -> io::Result<String> {
        let (_, summary) = run_bounded(FSCK, &["-p", source], CHECK_TIMEOUT)?;
        Ok(summary)
    }

    fn mount(&mut self, spec: &MountSpec) -> io::Result<()> {
        let target = self.target.to_string_lossy().into_owned();
        let (ok, summary) = run_bounded(
            MOUNT,
            &["-t", &spec.fstype, "-o", &spec.options, &spec.source, &target],
            Duration::from_secs(30),
        )?;
        if ok {
            Ok(())
        } else {
            Err(io::Error::other(format!("mount failed, {summary}")))
        }
    }

    fn note_unmount(&mut self, spec: &MountSpec) -> io::Result<()> {
        // One `/proc/mounts` line, read back with `mount_on`.
        let line = format!(
            "{} {} {} {} 0 0\n",
            escape(&spec.source),
            escape(&spec.target),
            escape(&spec.fstype),
            escape(&spec.options)
        );
        std::fs::write(&self.note, line)
    }

    fn unmount_noted(&self) -> Option<MountSpec> {
        let note = std::fs::read_to_string(&self.note).ok()?;
        // A note cut short still says the volume was taken off Wave: it is
        // then mounted as the boot mounts it.
        Some(mount_on(&note, &self.target).unwrap_or_else(MountSpec::boot))
    }

    fn clear_unmount_note(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.note) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!("usb storage: could not remove {}: {error}", self.note.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<String>>>;

    /// A configfs-shaped gadget in a temporary directory: stock's functions,
    /// `links` in the configuration, bound to the controller, a computer
    /// configured. Plain files keep what was written: `UDC` reads "none"
    /// after an unbind (configfs reads empty), `lun.0/file` "\n" after an
    /// unload.
    struct Tree {
        _root: tempfile::TempDir,
        gadget: PathBuf,
        udc: PathBuf,
        note: PathBuf,
        log: Log,
        /// adbd runs.
        adbd: Arc<AtomicBool>,
        /// adbd will not start.
        adbd_fails: Arc<AtomicBool>,
    }

    fn tree(links: &[&str], configuration: &str) -> Tree {
        let root = tempfile::tempdir().unwrap();
        let gadget = root.path().join("amlogic");
        for function in ["uac2.0", ADB, SERIAL, HID] {
            std::fs::create_dir_all(gadget.join("functions").join(function)).unwrap();
        }
        let lun = gadget.join("functions/mass_storage.usb0/lun.0");
        std::fs::create_dir_all(&lun).unwrap();
        std::fs::write(lun.join("file"), "").unwrap();
        std::fs::write(lun.join("removable"), "1\n").unwrap();
        std::fs::create_dir_all(gadget.join("configs/amlogic.1/strings/0x409")).unwrap();
        std::fs::write(gadget.join(CONFIGURATION), format!("{configuration}\n")).unwrap();
        std::fs::write(gadget.join("UDC"), format!("{UDC_NAME}\n")).unwrap();
        for function in links {
            std::os::unix::fs::symlink(gadget.join("functions").join(function), gadget.join(CONFIG).join(function))
                .unwrap();
        }
        let udc = root.path().join("udc");
        std::fs::create_dir_all(udc.join(UDC_NAME)).unwrap();
        std::fs::write(udc.join(UDC_NAME).join("state"), "configured\n").unwrap();
        let note = root.path().join("gadget-note");
        Tree {
            _root: root,
            gadget,
            udc,
            note,
            log: Arc::default(),
            adbd: Arc::new(AtomicBool::new(true)),
            adbd_fails: Arc::default(),
        }
    }

    /// Wave's normal USB on the owner's unit.
    fn normal_tree() -> Tree {
        tree(&["uac2.0", ADB, HID], "hid")
    }

    impl Tree {
        fn udc_text(&self) -> String {
            read_trimmed(&self.gadget.join("UDC")).unwrap_or_else(|error| format!("{error}"))
        }

        /// The configuration's links, sorted.
        fn links(&self) -> String {
            let mut links: Vec<String> = std::fs::read_dir(self.gadget.join(CONFIG))
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_type().unwrap().is_symlink())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            links.sort();
            links.join(" ")
        }

        fn configuration(&self) -> String {
            read_trimmed(&self.gadget.join(CONFIGURATION)).unwrap()
        }

        fn lun_file(&self) -> String {
            read_trimmed(&self.gadget.join("functions/mass_storage.usb0/lun.0/file")).unwrap()
        }

        /// Each adbd call and pause, with the gadget as it was then.
        fn take_log(&self) -> Vec<String> {
            std::mem::take(&mut self.log.lock().unwrap())
        }

        fn gadget(&self) -> ConfigfsGadget {
            let adbd = FakeAdbd {
                gadget: self.gadget.clone(),
                log: Arc::clone(&self.log),
                running: Arc::clone(&self.adbd),
                fails: Arc::clone(&self.adbd_fails),
            };
            let gadget = self.gadget.clone();
            let log = Arc::clone(&self.log);
            ConfigfsGadget::new(&self.gadget, &self.udc, &self.note, Box::new(adbd)).with_sleep(move |pause| {
                let seen = snapshot(&gadget);
                log.lock().unwrap().push(format!("sleep {} {seen}", pause.as_secs()));
            })
        }
    }

    /// The UDC and the links, as one line.
    fn snapshot(gadget: &Path) -> String {
        let udc = read_trimmed(&gadget.join("UDC")).unwrap_or_else(|_| "?".into());
        let mut links: Vec<String> = std::fs::read_dir(gadget.join(CONFIG))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_type().unwrap().is_symlink())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        links.sort();
        format!("udc={udc} links={}", links.join(","))
    }

    struct FakeAdbd {
        gadget: PathBuf,
        log: Log,
        running: Arc<AtomicBool>,
        fails: Arc<AtomicBool>,
    }

    impl Adbd for FakeAdbd {
        fn running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }
        fn stop(&mut self) {
            self.log.lock().unwrap().push(format!("adbd stop {}", snapshot(&self.gadget)));
            self.running.store(false, Ordering::SeqCst);
        }
        fn start(&mut self) -> io::Result<()> {
            self.log.lock().unwrap().push(format!("adbd start {}", snapshot(&self.gadget)));
            if self.fails.load(Ordering::SeqCst) {
                return Err(io::Error::other("start-stop-daemon exited 1"));
            }
            assert!(!self.running.swap(true, Ordering::SeqCst), "a second adbd");
            Ok(())
        }
    }

    #[test]
    fn enter_storage_is_stock_uac_hid_msc() {
        let t = normal_tree();
        let mut g = t.gadget();
        assert_eq!(g.mode(), GadgetMode::Normal);
        g.enter_storage("/dev/udisk").unwrap();
        assert_eq!(
            t.take_log(),
            [
                "adbd stop udc=none links=ffs.adb,hid.0,uac2.0",
                "sleep 2 udc=none links=hid.0,mass_storage.usb0,uac2.0",
            ],
            "unbound before adbd stops (then unlinked, as stock); linked before the bind"
        );
        assert_eq!(t.links(), "hid.0 mass_storage.usb0 uac2.0", "uac2.0 stays, adb goes");
        assert_eq!(t.lun_file(), "/dev/udisk");
        assert_eq!(t.configuration(), "hid");
        assert_eq!(t.udc_text(), UDC_NAME);
        assert_eq!(g.mode(), GadgetMode::Storage);
        assert_eq!(
            std::fs::read_to_string(&t.note).unwrap(),
            "udc=fdd00000.crgudc2\nconfiguration=hid\nlinks=ffs.adb hid.0\n"
        );
    }

    #[test]
    fn leave_storage_brings_adb_back_with_a_fresh_adbd() {
        let t = normal_tree();
        let mut g = t.gadget();
        g.enter_storage("/dev/udisk").unwrap();
        t.take_log();
        g.leave_storage().unwrap();
        assert_eq!(
            t.take_log(),
            [
                "adbd stop udc=none links=hid.0,mass_storage.usb0,uac2.0",
                "adbd start udc=none links=uac2.0",
                "sleep 4 udc=none links=ffs.adb,hid.0,uac2.0",
            ],
            "adbd runs before ffs.adb is linked, and has 4 s before the bind"
        );
        assert_eq!(t.links(), "ffs.adb hid.0 uac2.0");
        assert_eq!(t.lun_file(), "", "the drive is empty");
        assert_eq!(t.configuration(), "hid");
        assert_eq!(t.udc_text(), UDC_NAME);
        assert_eq!(g.mode(), GadgetMode::Normal);
        assert!(!t.note.exists(), "the note goes once normal USB is bound");
    }

    #[test]
    fn the_way_back_restores_only_what_was_linked() {
        // A unit without adb (stock's start mode): waved never turns it on.
        let t = tree(&["uac2.0", HID], "uac");
        t.adbd.store(false, Ordering::SeqCst);
        let mut g = t.gadget();
        g.enter_storage("/dev/udisk").unwrap();
        g.leave_storage().unwrap();
        assert!(!t.take_log().iter().any(|op| op.starts_with("adbd start")));
        assert_eq!(t.links(), "hid.0 uac2.0");
        assert_eq!(t.configuration(), "uac", "the configuration string is restored");
    }

    #[test]
    fn a_restarted_waved_comes_back_as_the_note_says() {
        let t = tree(&["uac2.0", HID], "uac");
        t.adbd.store(false, Ordering::SeqCst);
        t.gadget().enter_storage("/dev/udisk").unwrap();
        // A new waved: nothing in memory, the note on tmpfs.
        let mut g = t.gadget();
        assert_eq!(g.mode(), GadgetMode::Storage);
        g.leave_storage().unwrap();
        assert!(!t.take_log().iter().any(|op| op.starts_with("adbd start")));
        assert_eq!(t.links(), "hid.0 uac2.0");
        assert_eq!(t.configuration(), "uac");
    }

    #[test]
    fn without_a_note_the_way_back_is_the_boot_mode() {
        // Storage mode set up by hand (S89usbgadget uac_hid_msc): no note.
        let t = tree(&["uac2.0", DRIVE, HID], "hid");
        std::fs::write(t.gadget.join("functions/mass_storage.usb0/lun.0/file"), "/dev/mmcblk0p16\n").unwrap();
        t.adbd.store(false, Ordering::SeqCst);
        let mut g = t.gadget();
        assert_eq!(g.mode(), GadgetMode::Storage);
        g.leave_storage().unwrap();
        assert!(t.take_log().iter().any(|op| op.starts_with("adbd start")));
        assert_eq!(t.links(), "ffs.adb hid.0 uac2.0", "audio, adb and HID");
        assert_eq!(t.lun_file(), "");
        assert_eq!(t.udc_text(), UDC_NAME);
    }

    #[test]
    fn the_mode_comes_from_the_gadget_and_the_note() {
        let t = normal_tree();
        assert_eq!(t.gadget().mode(), GadgetMode::Normal);
        std::fs::write(t.gadget.join("UDC"), "\n").unwrap();
        assert_eq!(t.gadget().mode(), GadgetMode::Unbound, "the boot has not bound it yet");
        std::fs::write(&t.note, NormalMode::boot().to_note()).unwrap();
        assert_eq!(t.gadget().mode(), GadgetMode::Storage, "a waved left a switch part way");
        std::fs::write(t.gadget.join("UDC"), UDC_NAME).unwrap();
        assert_eq!(t.gadget().mode(), GadgetMode::Normal, "bound normally: a stale note");
        std::fs::remove_file(t.gadget.join("functions/mass_storage.usb0/lun.0/file")).unwrap();
        assert_eq!(t.gadget().mode(), GadgetMode::Unavailable);
        let empty = tempfile::tempdir().unwrap();
        let adbd = SystemAdbd::new(empty.path(), ADBD, &ADBD_LAUNCH);
        assert_eq!(
            ConfigfsGadget::new(empty.path().join("amlogic"), empty.path(), empty.path().join("note"), Box::new(adbd))
                .mode(),
            GadgetMode::Unavailable,
            "no gadget yet"
        );
    }

    #[test]
    fn a_bind_that_does_not_take_is_an_error_and_the_way_back_retries_it() {
        let t = normal_tree();
        let broken = Arc::new(AtomicBool::new(true));
        let udc = t.gadget.join("UDC");
        let flag = Arc::clone(&broken);
        let mut g = t.gadget().with_sleep(move |_| {
            // The controller refuses: the bind does not reach UDC.
            if flag.load(Ordering::SeqCst) {
                let _ = std::fs::remove_file(&udc);
                let _ = std::fs::create_dir(&udc);
            } else if udc.is_dir() {
                std::fs::remove_dir(&udc).unwrap();
                std::fs::write(&udc, "").unwrap();
            }
        });
        assert!(g.enter_storage("/dev/udisk").is_err());
        assert!(g.leave_storage().is_err(), "still no bind");
        assert!(t.note.exists(), "normal USB is still owed");
        broken.store(false, Ordering::SeqCst);
        g.leave_storage().unwrap();
        assert_eq!(t.udc_text(), UDC_NAME);
        assert_eq!(t.links(), "ffs.adb hid.0 uac2.0");
        assert!(!t.note.exists());
    }

    #[test]
    fn enter_needs_a_bound_gadget_and_changes_nothing_without_one() {
        let t = normal_tree();
        std::fs::write(t.gadget.join("UDC"), "\n").unwrap();
        assert!(t.gadget().enter_storage("/dev/udisk").is_err());
        assert!(t.take_log().is_empty(), "adbd untouched");
        assert_eq!(t.links(), "ffs.adb hid.0 uac2.0");
        assert_eq!(t.lun_file(), "");
        assert!(!t.note.exists());
    }

    #[test]
    fn a_switch_that_failed_before_changing_anything_has_nothing_to_undo() {
        // The note could not be written: USB is as it was, and the way back
        // must not rebuild it from the boot mode (adb on a unit without it).
        let t = tree(&["uac2.0", HID], "uac");
        t.adbd.store(false, Ordering::SeqCst);
        std::fs::create_dir(&t.note).unwrap();
        let mut g = t.gadget();
        assert!(g.enter_storage("/dev/udisk").is_err());
        std::fs::remove_dir(&t.note).unwrap();
        g.leave_storage().unwrap();
        assert!(t.take_log().is_empty(), "never unbound, adbd untouched");
        assert_eq!(t.links(), "hid.0 uac2.0");
        assert_eq!(t.udc_text(), UDC_NAME);
    }

    #[test]
    fn an_adbd_that_will_not_start_leaves_adb_unlinked_so_usb_still_binds() {
        // FunctionFS without its daemon would fail the whole bind.
        let t = normal_tree();
        let mut g = t.gadget();
        g.enter_storage("/dev/udisk").unwrap();
        t.adbd_fails.store(true, Ordering::SeqCst);
        g.leave_storage().unwrap();
        assert_eq!(t.links(), "hid.0 uac2.0");
        assert_eq!(t.udc_text(), UDC_NAME);
    }

    #[test]
    fn the_drive_is_made_removable_before_it_is_loaded() {
        // The computer's eject (START STOP UNIT) only empties a removable LUN.
        let t = normal_tree();
        let removable = t.gadget.join("functions/mass_storage.usb0/lun.0/removable");
        std::fs::write(&removable, "0\n").unwrap();
        t.gadget().enter_storage("/dev/udisk").unwrap();
        assert_eq!(read_trimmed(&removable).unwrap(), "1");
    }

    #[test]
    fn the_normal_mode_note_reads_back() {
        let mode = NormalMode {
            udc: UDC_NAME.into(),
            configuration: "uac".into(),
            links: vec![HID.into()],
        };
        assert_eq!(NormalMode::from_note(&mode.to_note()), Some(mode));
        let none = NormalMode {
            links: Vec::new(),
            ..NormalMode::boot()
        };
        assert_eq!(NormalMode::from_note(&none.to_note()), Some(none));
        assert_eq!(NormalMode::from_note("udc=fdd00000.crgudc2\n"), None, "cut short");
        assert_eq!(NormalMode::from_note(""), None);
        assert_eq!(
            NormalMode::from_note("udc=x\nconfiguration=hid\nlinks=hid.0 mass_storage.usb0 uac2.0 junk\n")
                .unwrap()
                .links,
            [HID],
            "only what storage mode unlinks comes back"
        );
    }

    #[test]
    fn the_lun_file_is_the_medium() {
        let t = normal_tree();
        let mut g = t.gadget();
        assert_eq!(g.loaded().unwrap(), None);
        let file = t.gadget.join("functions/mass_storage.usb0/lun.0/file");
        std::fs::write(&file, "/dev/mmcblk0p16\n").unwrap();
        assert_eq!(g.loaded().unwrap().as_deref(), Some("/dev/mmcblk0p16"));
        // configfs closes the medium on "\n" and then reads back empty.
        g.unload().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "\n");
        assert_eq!(g.loaded().unwrap(), None);
    }

    #[test]
    fn no_gadget_is_an_error_not_an_empty_drive() {
        let t = normal_tree();
        std::fs::remove_file(t.gadget.join("functions/mass_storage.usb0/lun.0/file")).unwrap();
        assert!(t.gadget().loaded().is_err());
        assert!(t.gadget().unload().is_err(), "never creates the attribute");
    }

    #[test]
    fn the_host_link_follows_the_controller() {
        let t = normal_tree();
        let state = t.udc.join(UDC_NAME).join("state");
        assert_eq!(t.gadget().host(), HostLink::Configured);
        std::fs::write(&state, "suspended\n").unwrap();
        assert_eq!(t.gadget().host(), HostLink::Suspended);
        std::fs::write(&state, "not attached\n").unwrap();
        assert_eq!(t.gadget().host(), HostLink::Gone);
    }

    /// A `/proc` with `processes` (pid, comm, stat state).
    fn fake_proc(processes: &[(i32, &str, char)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (pid, comm, state) in processes {
            let p = dir.path().join(pid.to_string());
            std::fs::create_dir(&p).unwrap();
            std::fs::write(p.join("comm"), format!("{comm}\n")).unwrap();
            std::fs::write(p.join("stat"), format!("{pid} ({comm}) {state} 1 1 1")).unwrap();
        }
        std::fs::create_dir(dir.path().join("self")).unwrap();
        dir
    }

    /// `launch` that runs a shell script (never exec'd itself: no ETXTBSY
    /// while another test thread forks).
    fn script(dir: &Path, body: &str) -> Vec<String> {
        let path = dir.join("launch.sh");
        std::fs::write(&path, body).unwrap();
        vec!["/bin/sh".into(), path.to_string_lossy().into_owned()]
    }

    fn adbd_with(proc_dir: &Path, launch: &[String]) -> SystemAdbd {
        let launch: Vec<&str> = launch.iter().map(String::as_str).collect();
        SystemAdbd::new(proc_dir, ADBD, &launch).with_sleep(|_| {})
    }

    #[test]
    fn adbd_is_found_by_name_and_a_zombie_does_not_count() {
        let proc_dir = fake_proc(&[(12, "adbd", 'S'), (13, "waved", 'R')]);
        let adbd = adbd_with(proc_dir.path(), &[]);
        assert!(adbd.running());
        assert_eq!(adbd.pids(), [12]);
        let zombie = fake_proc(&[(12, "adbd", 'Z')]);
        assert!(!adbd_with(zombie.path(), &[]).running());
        let none = fake_proc(&[(13, "waved", 'R')]);
        assert!(!adbd_with(none.path(), &[]).running());
    }

    #[test]
    fn stop_kills_every_adbd_and_waits_for_them_to_go() {
        let proc_dir = fake_proc(&[(12, "adbd", 'S'), (14, "adbd", 'S'), (13, "waved", 'R')]);
        let root = proc_dir.path().to_path_buf();
        let killed = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&killed);
        let mut adbd = adbd_with(proc_dir.path(), &[]).with_signal(move |pid| {
            seen.lock().unwrap().push(pid);
            std::fs::remove_dir_all(root.join(pid.to_string())).unwrap();
        });
        adbd.stop();
        let mut killed = killed.lock().unwrap().clone();
        killed.sort();
        assert_eq!(killed, [12, 14], "never waved");
        assert!(!adbd.running());
    }

    #[test]
    fn stop_gives_up_on_an_adbd_that_stays_in_bounded_time() {
        let proc_dir = fake_proc(&[(12, "adbd", 'D')]);
        let pauses = Arc::new(Mutex::new(0u32));
        let count = Arc::clone(&pauses);
        let mut adbd = adbd_with(proc_dir.path(), &[])
            .with_signal(|_| {})
            .with_sleep(move |_| *count.lock().unwrap() += 1);
        adbd.stop();
        let most = (ADBD_GONE_WAIT.as_millis() / ADBD_POLL.as_millis()) as u32;
        assert!(*pauses.lock().unwrap() <= most);
    }

    #[test]
    fn start_launches_adbd_detached() {
        let proc_dir = fake_proc(&[]);
        let out = proc_dir.path().join("args");
        let launch = script(proc_dir.path(), &format!("echo \"$@\" > '{}'\n", out.display()));
        adbd_with(proc_dir.path(), &launch).start().unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), ADBD);
    }

    #[test]
    fn start_never_starts_a_second_adbd() {
        let proc_dir = fake_proc(&[(12, "adbd", 'S')]);
        let out = proc_dir.path().join("args");
        let launch = script(proc_dir.path(), &format!("echo \"$@\" > '{}'\n", out.display()));
        adbd_with(proc_dir.path(), &launch).start().unwrap();
        assert!(!out.exists(), "not launched");
    }

    #[test]
    fn a_launch_that_fails_is_an_error() {
        let proc_dir = fake_proc(&[]);
        let launch = script(proc_dir.path(), "exit 1\n");
        assert!(adbd_with(proc_dir.path(), &launch).start().is_err());
        let missing = vec!["/nonexistent/start-stop-daemon".to_string()];
        assert!(adbd_with(proc_dir.path(), &missing).start().is_err());
    }

    #[test]
    fn adbd_is_started_by_busybox_start_stop_daemon_in_the_background() {
        assert_eq!(ADBD_LAUNCH, ["/sbin/start-stop-daemon", "-S", "-b", "-o", "-x"]);
        assert_eq!(SystemAdbd::device().launch.last().map(String::as_str), Some("-x"));
    }

    #[test]
    fn the_top_mount_on_the_target_wins() {
        let mounts = "\
/dev/root / squashfs ro,relatime 0 0
/dev/udisk /mnt/udisk exfat rw,relatime,fmask=0077,dmask=0022,iocharset=utf8 0 0
/dev/psp_udisk_s /udisk_s exfat rw,relatime 0 0
tmpfs /mnt/udisk tmpfs ro 0 0
";
        let top = mount_on(mounts, Path::new("/mnt/udisk")).unwrap();
        assert_eq!(top.fstype, "tmpfs");
        let first = mount_on(&mounts.lines().take(3).collect::<Vec<_>>().join("\n"), Path::new("/mnt/udisk"))
            .unwrap();
        assert_eq!(
            first,
            MountSpec {
                source: "/dev/udisk".into(),
                target: "/mnt/udisk".into(),
                fstype: "exfat".into(),
                options: "rw,relatime,fmask=0077,dmask=0022,iocharset=utf8".into(),
            }
        );
        assert_eq!(mount_on(mounts, Path::new("/mnt")), None);
        assert_eq!(mount_on("/dev/udisk /mnt/udisk/ exfat rw 0 0", Path::new("/mnt/udisk")).map(|m| m.fstype), Some("exfat".into()));
    }

    #[test]
    fn escaped_mount_paths_are_read_back() {
        let spec = mount_on("/dev/sda1 /mnt/my\\040disk vfat rw 0 0", Path::new("/mnt/my disk")).unwrap();
        assert_eq!(spec.target, "/mnt/my disk");
    }

    #[test]
    fn the_volume_reads_its_mount_from_proc_mounts() {
        let dir = tempfile::tempdir().unwrap();
        let mounts = dir.path().join("mounts");
        std::fs::write(&mounts, "/dev/udisk /mnt/udisk exfat rw 0 0\n").unwrap();
        let volume = SystemVolume::new("/mnt/udisk", &mounts);
        assert_eq!(volume.mounted().unwrap().unwrap().source, "/dev/udisk");
        std::fs::write(&mounts, "/dev/root / squashfs ro 0 0\n").unwrap();
        assert_eq!(volume.mounted().unwrap(), None);
    }

    #[test]
    fn the_unmount_note_keeps_the_mount_until_it_is_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("note");
        let mut volume = SystemVolume::new("/mnt/my disk", dir.path().join("mounts")).with_note(&note);
        assert_eq!(volume.unmount_noted(), None, "no note: the boot mounts the volume");
        let spec = MountSpec {
            source: "/dev/udisk".into(),
            target: "/mnt/my disk".into(),
            fstype: "exfat".into(),
            options: "rw,relatime,fmask=0077".into(),
        };
        volume.note_unmount(&spec).unwrap();
        assert_eq!(volume.unmount_noted(), Some(spec));
        // Cut short: still a note, with the boot's mount.
        std::fs::write(&note, "/dev/udisk").unwrap();
        assert_eq!(volume.unmount_noted(), Some(MountSpec::boot()));
        volume.clear_unmount_note();
        assert_eq!(volume.unmount_noted(), None);
        volume.clear_unmount_note();
    }

    #[test]
    fn bounded_commands_report_and_time_out() {
        let (ok, summary) = run_bounded("/bin/sh", &["-c", "echo first; echo last >&2"], Duration::from_secs(5)).unwrap();
        assert!(ok);
        assert!(summary.ends_with("last"), "{summary}");
        let (ok, _) = run_bounded("/bin/sh", &["-c", "exit 3"], Duration::from_secs(5)).unwrap();
        assert!(!ok);
        let error = run_bounded("/bin/sh", &["-c", "sleep 5"], Duration::from_millis(200)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
