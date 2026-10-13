//! The device side of USB storage: the configfs mass-storage function, the
//! UDC state files and the recordings mount (`docs/USB_STORAGE.md`).

use super::{Gadget, HostLink, MountSpec, Volume, MOUNT_POINT};
use std::ffi::CString;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const GADGET_DIR: &str = "/sys/kernel/config/usb_gadget/amlogic";
/// The drive's first (only) LUN.
pub const LUN_DIR: &str = "/sys/kernel/config/usb_gadget/amlogic/functions/mass_storage.usb0/lun.0";
/// The link that puts the drive in the configuration the computer sees.
pub const CONFIG_LINK: &str = "/sys/kernel/config/usb_gadget/amlogic/configs/amlogic.1/mass_storage.usb0";
pub const PROC_MOUNTS: &str = "/proc/mounts";
/// exfat-utils' `exfatfsck` (`/sbin/fsck.exfat` links to it): `-p` repairs
/// what is safe without asking.
pub const FSCK: &str = "/sbin/fsck.exfat";
pub const MOUNT: &str = "/bin/mount";
/// A full check of the 21 GB volume takes seconds; past this it is stopped
/// and the volume is mounted anyway.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(120);
const REPLUG_PAUSE: Duration = Duration::from_secs(1);

/// `mass_storage.usb0` in the configfs gadget, and the UDCs under
/// `/sys/class/udc`.
pub struct ConfigfsGadget {
    lun: PathBuf,
    link: PathBuf,
    udc_dir: PathBuf,
}

impl ConfigfsGadget {
    pub fn new(lun: impl Into<PathBuf>, link: impl Into<PathBuf>, udc_dir: impl Into<PathBuf>) -> Self {
        Self {
            lun: lun.into(),
            link: link.into(),
            udc_dir: udc_dir.into(),
        }
    }

    pub fn device() -> Self {
        Self::new(LUN_DIR, CONFIG_LINK, crate::usb::UDC_DIR)
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

/// Write an existing attribute file, as `echo value > path` does (never
/// creates it).
fn write_attribute(path: &Path, value: &str) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(value.as_bytes())
}

impl Gadget for ConfigfsGadget {
    fn drive_ready(&self) -> bool {
        let linked = std::fs::symlink_metadata(&self.link).is_ok();
        let removable = std::fs::read_to_string(self.lun.join("removable"))
            .is_ok_and(|value| value.trim() == "1");
        linked && removable
    }

    fn loaded(&self) -> io::Result<Option<String>> {
        let file = std::fs::read_to_string(self.lun.join("file"))?;
        let file = file.trim();
        Ok((!file.is_empty()).then(|| file.to_string()))
    }

    fn load(&mut self, path: &str) -> io::Result<()> {
        write_attribute(&self.lun.join("file"), path)
    }

    fn unload(&mut self) -> io::Result<()> {
        // f_mass_storage drops the trailing newline; an empty name closes the
        // medium. A zero-byte write would not reach it.
        write_attribute(&self.lun.join("file"), "\n")
    }

    fn host(&self) -> HostLink {
        let states: Vec<String> = self
            .controllers()
            .iter()
            .filter_map(|udc| std::fs::read_to_string(udc.join("state")).ok())
            .collect();
        HostLink::from_states(states.iter().map(String::as_str))
    }

    fn replug(&mut self) -> io::Result<()> {
        let controllers = self.controllers();
        if controllers.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no USB device controller"));
        }
        for udc in &controllers {
            write_attribute(&udc.join("soft_connect"), "disconnect")?;
        }
        std::thread::sleep(REPLUG_PAUSE);
        let mut result = Ok(());
        for udc in &controllers {
            if let Err(error) = write_attribute(&udc.join("soft_connect"), "connect") {
                result = Err(error);
            }
        }
        result
    }
}

/// The recordings mount, through `/proc/mounts`, umount(2), `fsck.exfat` and
/// busybox `mount`.
pub struct SystemVolume {
    target: PathBuf,
    proc_mounts: PathBuf,
}

impl SystemVolume {
    pub fn new(target: impl Into<PathBuf>, proc_mounts: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            proc_mounts: proc_mounts.into(),
        }
    }

    pub fn device() -> Self {
        Self::new(MOUNT_POINT, PROC_MOUNTS)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dirs {
        _root: tempfile::TempDir,
        lun: PathBuf,
        link: PathBuf,
        udc: PathBuf,
    }

    fn dirs() -> Dirs {
        let root = tempfile::tempdir().unwrap();
        let lun = root.path().join("functions/mass_storage.usb0/lun.0");
        std::fs::create_dir_all(&lun).unwrap();
        std::fs::write(lun.join("file"), "").unwrap();
        std::fs::write(lun.join("removable"), "1\n").unwrap();
        let link = root.path().join("configs/amlogic.1/mass_storage.usb0");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let udc = root.path().join("udc");
        std::fs::create_dir_all(udc.join("fdd00000.crgudc2")).unwrap();
        std::fs::write(udc.join("fdd00000.crgudc2/state"), "configured\n").unwrap();
        std::fs::write(udc.join("fdd00000.crgudc2/soft_connect"), "").unwrap();
        Dirs {
            _root: root,
            lun,
            link,
            udc,
        }
    }

    fn gadget(d: &Dirs) -> ConfigfsGadget {
        ConfigfsGadget::new(&d.lun, &d.link, &d.udc)
    }

    #[test]
    fn the_drive_is_ready_only_when_linked_and_removable() {
        let d = dirs();
        assert!(!gadget(&d).drive_ready(), "not linked");
        std::os::unix::fs::symlink(d.lun.parent().unwrap(), &d.link).unwrap();
        assert!(gadget(&d).drive_ready());
        std::fs::write(d.lun.join("removable"), "0\n").unwrap();
        assert!(!gadget(&d).drive_ready(), "a fixed drive cannot change media while bound");
    }

    #[test]
    fn the_lun_file_is_the_medium() {
        let d = dirs();
        let mut g = gadget(&d);
        assert_eq!(g.loaded().unwrap(), None);
        g.load("/dev/udisk").unwrap();
        assert_eq!(std::fs::read_to_string(d.lun.join("file")).unwrap(), "/dev/udisk");
        std::fs::write(d.lun.join("file"), "/dev/mmcblk0p16\n").unwrap();
        assert_eq!(g.loaded().unwrap().as_deref(), Some("/dev/mmcblk0p16"));
        // configfs closes the medium on "\n" and then reads back empty.
        g.unload().unwrap();
        assert_eq!(std::fs::read_to_string(d.lun.join("file")).unwrap(), "\n");
        assert_eq!(g.loaded().unwrap(), None);
    }

    #[test]
    fn no_gadget_is_an_error_not_an_empty_drive() {
        let d = dirs();
        std::fs::remove_file(d.lun.join("file")).unwrap();
        assert!(gadget(&d).loaded().is_err());
        assert!(gadget(&d).load("/dev/udisk").is_err(), "never creates the attribute");
    }

    #[test]
    fn the_host_link_follows_the_controller() {
        let d = dirs();
        let state = d.udc.join("fdd00000.crgudc2/state");
        assert_eq!(gadget(&d).host(), HostLink::Configured);
        std::fs::write(&state, "suspended\n").unwrap();
        assert_eq!(gadget(&d).host(), HostLink::Suspended);
        std::fs::write(&state, "not attached\n").unwrap();
        assert_eq!(gadget(&d).host(), HostLink::Gone);
    }

    #[test]
    fn replug_disconnects_then_connects() {
        let d = dirs();
        gadget(&d).replug().unwrap();
        assert_eq!(
            std::fs::read_to_string(d.udc.join("fdd00000.crgudc2/soft_connect")).unwrap(),
            "connect"
        );
        let empty = tempfile::tempdir().unwrap();
        assert!(ConfigfsGadget::new(&d.lun, &d.link, empty.path()).replug().is_err());
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
