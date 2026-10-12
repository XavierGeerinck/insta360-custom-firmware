//! Putting a verified package in place. `Installer` is the seam: the service
//! talks to it, `FsInstaller` is the real one and `RecordingInstaller` the
//! host fake.
//!
//! **`FsInstaller` is unverified on the device.** Its host tests run it on a
//! temporary root; the Wave-specific parts (overlay writes to `/etc/init.d`,
//! busybox `reboot`) are listed in `docs/UPDATES.md`, "Device checks".
//!
//! Order, so a power cut at any point leaves a Wave that starts:
//! 1. syntax-check the staged init scripts (`sh -n`);
//! 2. hard-link every file about to be replaced to `<file>.prev`;
//! 3. write `update/trial` (from here `S60waved` counts starts);
//! 4. replace the deploy files (copy to `<file>.new`, rename);
//! 5. rename the staged binary over `/data/waved/waved`;
//! 6. sync. The service then reboots.
//!
//! Cut before 3: nothing changed that matters. Cut between 3 and 5: the old
//! binary starts, finds the trial is not its own and puts the deploy files
//! back (`trial::check_at_start`). Cut after 5: the new binary is on trial.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::format::{Staged, Target};
use crate::paths::Paths;
use crate::trial::{self, Trial};
use crate::version::Version;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("{0:?} is not a valid shell script")]
    BadScript(Target),
    #[error("a trial of another version is still running")]
    TrialPending,
    #[error("reboot failed: {0}")]
    Reboot(String),
}

pub trait Installer: Send + Sync {
    /// Put `staged` in place and start its trial. `running` is this version.
    /// An error means the waved binary was not replaced (deploy files may
    /// have been; `trial::check_at_start` puts them back).
    fn install(&self, staged: &Staged, running: Version) -> Result<(), InstallError>;
    /// Restart Wave into the installed version. Returns only on failure
    /// (or right away on the fake).
    fn reboot(&self) -> Result<(), InstallError>;
}

pub struct FsInstaller {
    paths: Paths,
}

impl FsInstaller {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }
}

fn link_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    match fs::remove_file(to) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    if fs::hard_link(from, to).is_err() {
        fs::copy(from, to)?;
    }
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn sync_dir(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) => fs::File::open(dir)?.sync_all(),
        None => Ok(()),
    }
}

/// Copy `from` next to `to` and rename it over `to`, so readers see the old
/// file or the new one.
/// The directory sync after the rename is best effort: once the rename is
/// done the file is in place, and `install` must not report an error then.
fn replace_by_copy(from: &Path, to: &Path) -> io::Result<()> {
    let new = with_suffix(to, ".new");
    fs::copy(from, &new)?;
    fs::File::open(&new)?.sync_all()?;
    fs::rename(&new, to)?;
    let _ = sync_dir(to);
    Ok(())
}

fn syntax_ok(script: &Path) -> io::Result<bool> {
    Ok(Command::new("sh").arg("-n").arg(script).status()?.success())
}

impl Installer for FsInstaller {
    fn install(&self, staged: &Staged, running: Version) -> Result<(), InstallError> {
        if trial::pending(&self.paths).is_some() {
            return Err(InstallError::TrialPending);
        }
        for target in staged.targets().filter(|t| t.is_init_script()) {
            if !syntax_ok(&staged.path(target))? {
                return Err(InstallError::BadScript(target));
            }
        }
        // Binary last, so it is the final step.
        let mut targets: Vec<Target> = staged.targets().filter(|t| *t != Target::Waved).collect();
        targets.push(Target::Waved);

        let (mut restore, mut remove) = (Vec::new(), Vec::new());
        for target in &targets {
            let dest = self.paths.target(*target);
            if dest.exists() {
                link_or_copy(&dest, &Paths::prev(&dest))?;
                restore.push(dest);
            } else {
                remove.push(dest);
            }
        }
        trial::begin(
            &self.paths,
            &Trial {
                version: staged.version,
                build: staged.manifest.build.clone(),
                previous: running,
                restore,
                remove,
            },
        )?;
        for target in &targets {
            let dest = self.paths.target(*target);
            if let Some(dir) = dest.parent() {
                fs::create_dir_all(dir)?;
            }
            let from = staged.path(*target);
            // The binary is last: once it is renamed into place nothing may
            // fail, because the caller takes an error to mean it was not.
            if *target == Target::Waved && fs::rename(&from, &dest).is_ok() {
                let _ = sync_dir(&dest);
                continue;
            }
            replace_by_copy(&from, &dest)?;
        }
        let _ = fs::remove_dir_all(&staged.dir);
        nix::unistd::sync();
        Ok(())
    }

    fn reboot(&self) -> Result<(), InstallError> {
        nix::unistd::sync();
        // busybox reboot runs the rcK stop scripts first (S60waved stop).
        let status = Command::new("/sbin/reboot")
            .status()
            .map_err(|e| InstallError::Reboot(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(InstallError::Reboot(status.to_string()))
        }
    }
}

/// What the fake saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
    Install { version: Version, from: Version },
    Reboot,
}

/// Host fake: records calls, can be told to fail.
#[derive(Clone, Default)]
pub struct RecordingInstaller {
    log: Arc<Mutex<Vec<Installed>>>,
    fail: Arc<Mutex<bool>>,
}

impl RecordingInstaller {
    pub fn calls(&self) -> Vec<Installed> {
        self.log.lock().unwrap().clone()
    }

    pub fn fail_next_install(&self) {
        *self.fail.lock().unwrap() = true;
    }
}

impl Installer for RecordingInstaller {
    fn install(&self, staged: &Staged, running: Version) -> Result<(), InstallError> {
        if std::mem::take(&mut *self.fail.lock().unwrap()) {
            return Err(InstallError::Io(io::Error::other("fake failure")));
        }
        self.log.lock().unwrap().push(Installed::Install {
            version: staged.version,
            from: running,
        });
        Ok(())
    }

    fn reboot(&self) -> Result<(), InstallError> {
        self.log.lock().unwrap().push(Installed::Reboot);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{pack, verify_and_stage};
    use crate::key;
    use crate::trial::StartCheck;

    const OLD: Version = Version::new(0, 9, 3);

    struct Device {
        _dir: tempfile::TempDir,
        paths: Paths,
    }

    impl Device {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::under(dir.path());
            let device = Self { _dir: dir, paths };
            device.put(Target::Waved, "old waved");
            device.put(Target::S60waved, "#!/bin/sh\necho old\n");
            device
        }

        fn put(&self, target: Target, text: &str) {
            let path = self.paths.target(target);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn read(&self, path: &Path) -> Option<String> {
            fs::read_to_string(path).ok()
        }

        fn stage(&self, files: &[(Target, &str)]) -> Staged {
            let key = key::generate().unwrap();
            let files: Vec<_> = files
                .iter()
                .map(|(t, text)| (*t, text.as_bytes().to_vec()))
                .collect();
            let (_, package) = pack(&files, Version::new(0, 10, 0), "abc1234", 0, &key).unwrap();
            fs::create_dir_all(self.paths.download_dir()).unwrap();
            fs::write(self.paths.download(), package).unwrap();
            verify_and_stage(
                &self.paths.download(),
                &key.verifying_key(),
                &self.paths.staging(),
                &mut |_, _| {},
            )
            .unwrap()
        }
    }

    #[test]
    fn files_land_with_old_copies_kept_and_the_trial_started() {
        let device = Device::new();
        let staged = device.stage(&[
            (Target::Waved, "new waved"),
            (Target::S60waved, "#!/bin/sh\necho new\n"),
            (Target::SplashFb, "splash"),
        ]);
        FsInstaller::new(device.paths.clone())
            .install(&staged, OLD)
            .unwrap();

        let waved = device.paths.target(Target::Waved);
        let init = device.paths.target(Target::S60waved);
        let splash = device.paths.target(Target::SplashFb);
        assert_eq!(device.read(&waved).as_deref(), Some("new waved"));
        assert_eq!(
            device.read(&Paths::prev(&waved)).as_deref(),
            Some("old waved")
        );
        assert_eq!(device.read(&init).as_deref(), Some("#!/bin/sh\necho new\n"));
        assert_eq!(
            device.read(&Paths::prev(&init)).as_deref(),
            Some("#!/bin/sh\necho old\n")
        );
        assert_eq!(device.read(&splash).as_deref(), Some("splash"));
        assert!(!device.paths.staging().exists(), "staging is cleaned up");

        let trial = trial::pending(&device.paths).unwrap();
        assert_eq!(trial.version, Version::new(0, 10, 0));
        assert_eq!(trial.previous, OLD);
        assert_eq!(trial.restore, vec![init, waved.clone()]);
        assert_eq!(trial.remove, vec![splash]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&waved).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }
        assert!(matches!(
            trial::check_at_start(&device.paths, Version::new(0, 10, 0)).unwrap(),
            StartCheck::OnTrial(_)
        ));
    }

    #[test]
    fn a_broken_init_script_installs_nothing() {
        let device = Device::new();
        let staged = device.stage(&[
            (Target::Waved, "new waved"),
            (Target::S60waved, "#!/bin/sh\nif then fi (\n"),
        ]);
        let err = FsInstaller::new(device.paths.clone())
            .install(&staged, OLD)
            .unwrap_err();
        assert!(
            matches!(err, InstallError::BadScript(Target::S60waved)),
            "{err}"
        );
        assert_eq!(
            device.read(&device.paths.target(Target::Waved)).as_deref(),
            Some("old waved")
        );
        assert!(trial::pending(&device.paths).is_none());
    }

    #[test]
    fn no_install_on_top_of_a_running_trial() {
        let device = Device::new();
        let staged = device.stage(&[(Target::Waved, "new waved")]);
        let installer = FsInstaller::new(device.paths.clone());
        installer.install(&staged, OLD).unwrap();
        let again = device.stage(&[(Target::Waved, "newer waved")]);
        assert!(matches!(
            installer.install(&again, OLD).unwrap_err(),
            InstallError::TrialPending
        ));
    }

    #[test]
    fn a_second_update_replaces_the_old_copy_with_the_committed_version() {
        let device = Device::new();
        let installer = FsInstaller::new(device.paths.clone());
        installer
            .install(&device.stage(&[(Target::Waved, "v2")]), OLD)
            .unwrap();
        trial::commit(&device.paths).unwrap();
        installer
            .install(&device.stage(&[(Target::Waved, "v3")]), OLD)
            .unwrap();
        let waved = device.paths.target(Target::Waved);
        assert_eq!(device.read(&waved).as_deref(), Some("v3"));
        assert_eq!(device.read(&Paths::prev(&waved)).as_deref(), Some("v2"));
    }

    #[test]
    fn the_fake_records_and_fails_on_request() {
        let device = Device::new();
        let staged = device.stage(&[(Target::Waved, "new")]);
        let fake = RecordingInstaller::default();
        fake.fail_next_install();
        assert!(fake.install(&staged, OLD).is_err());
        fake.install(&staged, OLD).unwrap();
        fake.reboot().unwrap();
        assert_eq!(
            fake.calls(),
            vec![
                Installed::Install {
                    version: Version::new(0, 10, 0),
                    from: OLD
                },
                Installed::Reboot
            ]
        );
    }
}
