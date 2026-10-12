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
    /// healthy. `S60waved` reads it (docs/UPDATES.md, "Trial starts").
    pub fn trial(&self) -> PathBuf {
        self.update_dir().join("trial")
    }

    /// One line per start of a version on trial, written by `S60waved`.
    pub fn starts(&self) -> PathBuf {
        self.update_dir().join("starts")
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

    /// The previous copy of a file, kept for the fall-back.
    pub fn prev(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(".prev");
        PathBuf::from(name)
    }
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
            Path::new("/tmp/x/data/waved/waved.prev")
        );
    }
}
