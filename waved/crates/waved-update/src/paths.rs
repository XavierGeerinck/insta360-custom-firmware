//! Every path the update code touches, under one root so host tests run the
//! real code in a temporary directory. `Paths::device()` is the Wave.

use std::path::{Path, PathBuf};

use crate::format::Target;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn device() -> Self {
        Self::under("/")
    }

    pub fn under(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn at(&self, device_path: &str) -> PathBuf {
        self.root.join(device_path.trim_start_matches('/'))
    }

    pub fn target(&self, target: Target) -> PathBuf {
        self.at(target.device_path())
    }

    pub fn waved_dir(&self) -> PathBuf {
        self.at("/data/waved")
    }

    /// waved's settings (`CONFIG_PATH` in `crates/waved/src/main.rs`). An
    /// install keeps a copy for a rollback: every config section denies
    /// unknown fields, so an older binary rejects a file a newer one saved.
    pub fn config(&self) -> PathBuf {
        self.at("/data/waved/config.toml")
    }

    /// Bookkeeping that must survive a reboot. On `/data`, next to waved.
    pub fn update_dir(&self) -> PathBuf {
        self.at("/data/waved/update")
    }

    /// Verified files waiting to be renamed into place. Same filesystem as
    /// the binary, so the swap is a rename.
    pub fn staging(&self) -> PathBuf {
        self.update_dir().join("staged")
    }

    /// Present from just before the new binary is in place until it reports
    /// healthy. `S60waved` reads it (docs/UPDATES.md, "Install and fall-back").
    pub fn trial(&self) -> PathBuf {
        self.update_dir().join("trial")
    }

    /// One line per boot of a version on trial, written by `S60waved`.
    pub fn boots(&self) -> PathBuf {
        self.update_dir().join("boots")
    }

    /// Emptied at every boot (a tmpfs): the guard's per-boot bookkeeping.
    pub fn run_dir(&self) -> PathBuf {
        self.at("/tmp")
    }

    /// Present once this boot has been counted.
    pub fn boot_mark(&self) -> PathBuf {
        self.run_dir().join("waved-trial-boot")
    }

    /// One line per start of the version on trial during this boot.
    pub fn boot_starts(&self) -> PathBuf {
        self.run_dir().join("waved-trial-starts")
    }

    /// The trial file, moved here by `S60waved` when it went back.
    pub fn rolled_back(&self) -> PathBuf {
        self.update_dir().join("rolled-back")
    }

    /// Versions automatic checks pass over (they rolled back or would not verify).
    pub fn skip(&self) -> PathBuf {
        self.update_dir().join("skip")
    }

    /// Downloads go to RAM (`/tmp` is a 1.9 GiB tmpfs); `/data` is small.
    pub fn download_dir(&self) -> PathBuf {
        self.at("/tmp/waved-update")
    }

    pub fn download(&self) -> PathBuf {
        self.download_dir().join("package.wup")
    }

    /// The previous copy of a file, kept for the fall-back:
    /// `/etc/init.d/S60waved` gives `/etc/init.d/.S60waved.prev`.
    pub fn prev(path: &Path) -> PathBuf {
        beside(path, ".prev")
    }

    /// A new copy of a file while it is written, before it is renamed over
    /// the file: `/etc/init.d/.S60waved.new`.
    pub fn new_copy(path: &Path) -> PathBuf {
        beside(path, ".new")
    }
}

/// `.<name><suffix>` in the same directory as `path`. The leading dot keeps
/// every copy out of rcS's `/etc/init.d/S??*` glob, which runs whatever
/// matches at boot (and rcK at shutdown). `deploy/S60waved` builds the same
/// name: `${path%/*}/.${path##*/}.prev`.
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_and_test_roots_line_up() {
        let device = Paths::device();
        assert_eq!(device.target(Target::Waved), Path::new("/data/waved/waved"));
        assert_eq!(device.trial(), Path::new("/data/waved/update/trial"));
        assert_eq!(
            device.download(),
            Path::new("/tmp/waved-update/package.wup")
        );
        let test = Paths::under("/tmp/x");
        assert_eq!(
            test.target(Target::S60waved),
            Path::new("/tmp/x/etc/init.d/S60waved")
        );
        assert_eq!(
            Paths::prev(&test.target(Target::Waved)),
            Path::new("/tmp/x/data/waved/.waved.prev")
        );
    }

    /// rcS runs every `/etc/init.d/S??*` at boot (and rcK at shutdown), so a
    /// copy of an init script must never match that: a leading dot.
    #[test]
    fn copies_beside_a_file_are_hidden_from_the_boot_glob() {
        let device = Paths::device();
        let init = device.target(Target::S60waved);
        assert_eq!(
            Paths::prev(&init),
            Path::new("/etc/init.d/.S60waved.prev")
        );
        assert_eq!(
            Paths::new_copy(&init),
            Path::new("/etc/init.d/.S60waved.new")
        );
        assert_eq!(
            Paths::prev(&device.target(Target::RsyslogConf)),
            Path::new("/etc/.rsyslog.conf.prev")
        );
    }
}
