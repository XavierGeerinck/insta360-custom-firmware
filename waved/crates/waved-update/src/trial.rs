//! Trial boots and the fall-back to the previous version (H1).
//!
//! The installer writes `update/trial` just before it renames the new binary
//! into place. While it exists, `deploy/S60waved` appends a line to
//! `update/starts` before every start; on the fourth start it puts every
//! `restore` file back from its `.<name>.prev` copy beside it, deletes every
//! `remove` file and moves the trial file to `update/rolled-back`. The new
//! waved removes the trial file (commits) once it has been healthy for a
//! minute (`health`).
//!
//! The file is plain lines so busybox `sh` can read it:
//!
//! ```text
//! version 0.10.0
//! build 374015f
//! previous 0.9.3
//! restore /data/waved/waved
//! restore /etc/init.d/S60waved
//! remove /data/waved/splash.fb
//! ```

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::format::Target;
use crate::paths::Paths;
use crate::version::Version;

/// Boots a version on trial gets to report healthy; `S60waved` goes back at
/// the start of the next one. Boots, not starts: one boot can take several
/// starts (the first is always killed after 5 s as a display workaround, and
/// the speaker can be busy for the first seconds; seen 2026-10-13).
pub const MAX_TRIAL_BOOTS: u32 = 2;

/// Starts within one boot before `S60waved` goes back: a crash loop.
pub const MAX_TRIAL_STARTS_PER_BOOT: u32 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    pub version: Version,
    pub build: String,
    pub previous: Version,
    /// Files whose old copy is at `Paths::prev` (`.<name>.prev` beside them).
    pub restore: Vec<PathBuf>,
    /// Files the update added; going back deletes them.
    pub remove: Vec<PathBuf>,
}

impl Trial {
    pub fn to_text(&self) -> String {
        let mut text = format!("version {}\n", self.version);
        if !self.build.is_empty() {
            text += &format!("build {}\n", self.build);
        }
        text += &format!("previous {}\n", self.previous);
        for path in &self.restore {
            text += &format!("restore {}\n", path.display());
        }
        for path in &self.remove {
            text += &format!("remove {}\n", path.display());
        }
        text
    }

    pub fn parse(text: &str) -> Option<Trial> {
        let (mut version, mut previous, mut build) = (None, None, String::new());
        let (mut restore, mut remove) = (Vec::new(), Vec::new());
        for line in text.lines() {
            let Some((key, value)) = line.split_once(' ') else {
                continue;
            };
            match key {
                "version" => version = value.parse().ok(),
                "previous" => previous = value.parse().ok(),
                "build" => build = value.to_string(),
                "restore" => restore.push(PathBuf::from(value)),
                "remove" => remove.push(PathBuf::from(value)),
                _ => {}
            }
        }
        Some(Trial {
            version: version?,
            build,
            previous: previous?,
            restore,
            remove,
        })
    }
}

/// Write `path` so a power cut leaves the old content or the new, never half.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = fs::File::create(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    fs::File::open(dir)?.sync_all()
}

/// Start a trial: fresh counts, then the trial file.
pub fn begin(paths: &Paths, trial: &Trial) -> io::Result<()> {
    clear_counts(paths)?;
    write_atomic(&paths.trial(), trial.to_text().as_bytes())
}

fn clear_counts(paths: &Paths) -> io::Result<()> {
    remove_if_present(&paths.boots())?;
    remove_if_present(&paths.boot_mark())?;
    remove_if_present(&paths.boot_starts())
}

/// The new version is healthy: stop counting. The `.prev` copies stay as the
/// last known good version until the next install replaces them; the copy of
/// the settings goes, because putting it back later would lose every setting
/// changed since.
pub fn commit(paths: &Paths) -> io::Result<()> {
    remove_if_present(&paths.trial())?;
    clear_counts(paths)?;
    remove_if_present(&Paths::prev(&paths.config()))
}

/// Boots counted so far for the version on trial.
pub fn boots(paths: &Paths) -> u32 {
    fs::read_to_string(paths.boots())
        .map(|text| text.lines().count() as u32)
        .unwrap_or(0)
}

pub fn pending(paths: &Paths) -> Option<Trial> {
    Trial::parse(&fs::read_to_string(paths.trial()).ok()?)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Put every file back as it was before the install (what `S60waved` does
/// on its own when the new version fails). `keep_binary` leaves the waved
/// binary alone: after an interrupted install it is already the old one.
pub fn restore_files(paths: &Paths, trial: &Trial, keep_binary: bool) -> io::Result<()> {
    let binary = paths.target(Target::Waved);
    for path in &trial.restore {
        if keep_binary && *path == binary {
            remove_if_present(&Paths::prev(path))?;
            continue;
        }
        let prev = Paths::prev(path);
        if prev.exists() {
            fs::rename(&prev, path)?;
        }
    }
    for path in &trial.remove {
        remove_if_present(path)?;
    }
    Ok(())
}

/// What a start finds, before anything else runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartCheck {
    Normal,
    /// This is the new version on trial; commit it once healthy.
    OnTrial(Trial),
    /// An install stopped before the new binary was in place, so the old one
    /// runs. The deploy files it had replaced are back.
    Interrupted(Trial),
    /// `S60waved` went back to this version after the trial failed.
    RolledBack(Trial),
}

pub fn check_at_start(paths: &Paths, running: Version) -> io::Result<StartCheck> {
    if let Ok(text) = fs::read_to_string(paths.rolled_back()) {
        remove_if_present(&paths.rolled_back())?;
        if let Some(trial) = Trial::parse(&text) {
            return Ok(StartCheck::RolledBack(trial));
        }
    }
    let Ok(text) = fs::read_to_string(paths.trial()) else {
        return Ok(StartCheck::Normal);
    };
    match Trial::parse(&text) {
        Some(trial) if trial.version == running => Ok(StartCheck::OnTrial(trial)),
        Some(trial) => {
            restore_files(paths, &trial, true)?;
            commit(paths)?;
            Ok(StartCheck::Interrupted(trial))
        }
        None => {
            commit(paths)?;
            Ok(StartCheck::Normal)
        }
    }
}

/// Versions automatic checks pass over: `version build` per line.
pub fn skipped(paths: &Paths) -> Vec<(Version, String)> {
    fs::read_to_string(paths.skip())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (version, build) = line.split_once(' ').unwrap_or((line, ""));
            Some((version.parse().ok()?, build.to_string()))
        })
        .collect()
}

pub fn is_skipped(paths: &Paths, version: Version, build: &str) -> bool {
    skipped(paths)
        .iter()
        .any(|(v, b)| *v == version && b == build)
}

pub fn skip(paths: &Paths, version: Version, build: &str) -> io::Result<()> {
    let mut list = skipped(paths);
    if list.iter().any(|(v, b)| *v == version && b == build) {
        return Ok(());
    }
    list.push((version, build.to_string()));
    // Keep the file small: the eight most recent.
    let from = list.len().saturating_sub(8);
    let text: String = list[from..]
        .iter()
        .map(|(v, b)| format!("{v} {b}\n"))
        .collect();
    write_atomic(&paths.skip(), text.as_bytes())
}

pub fn unskip(paths: &Paths, version: Version, build: &str) -> io::Result<()> {
    let list = skipped(paths);
    if !list.iter().any(|(v, b)| *v == version && b == build) {
        return Ok(());
    }
    let text: String = list
        .iter()
        .filter(|(v, b)| !(*v == version && b == build))
        .map(|(v, b)| format!("{v} {b}\n"))
        .collect();
    write_atomic(&paths.skip(), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const NEW: Version = Version::new(0, 10, 0);
    const OLD: Version = Version::new(0, 9, 3);

    struct Device {
        _dir: tempfile::TempDir,
        paths: Paths,
    }

    impl Device {
        /// A device mid-trial: new files in place, old ones at `.prev`, a
        /// splash the update added.
        fn mid_trial() -> (Device, Trial) {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::under(dir.path());
            let waved = paths.target(Target::Waved);
            let init = paths.target(Target::S60waved);
            let boot_splash = paths.target(Target::S03splash);
            let config = paths.config();
            let splash = paths.target(Target::SplashFb);
            for (path, new, old) in [
                (&waved, "new waved", "old waved"),
                (&init, "new init", "old init"),
                (&boot_splash, "new boot splash", "old boot splash"),
                (&config, "new settings", "old settings"),
            ] {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, new).unwrap();
                fs::write(Paths::prev(path), old).unwrap();
            }
            fs::write(&splash, "new splash").unwrap();
            let trial = Trial {
                version: NEW,
                build: "abc1234".into(),
                previous: OLD,
                restore: vec![waved, init, boot_splash, config],
                remove: vec![splash],
            };
            begin(&paths, &trial).unwrap();
            (Device { _dir: dir, paths }, trial)
        }

        fn read(&self, target: Target) -> Option<String> {
            fs::read_to_string(self.paths.target(target)).ok()
        }

        /// A power cycle: `/tmp` is a tmpfs and comes back empty.
        fn reboot(&self) {
            let _ = fs::remove_dir_all(self.paths.run_dir());
        }

        fn settings(&self) -> Option<String> {
            fs::read_to_string(self.paths.config()).ok()
        }

        /// One pass of the guard in `deploy/S60waved`, as the watchdog runs it.
        fn guard(&self) {
            let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/S60waved");
            let out = Command::new("sh")
                .arg(&script)
                .arg("guard")
                .env("WAVED_DIR", self.paths.waved_dir())
                .env("WAVED_RUN_DIR", self.paths.run_dir())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn the_trial_file_round_trips() {
        let (_device, trial) = Device::mid_trial();
        assert_eq!(Trial::parse(&trial.to_text()), Some(trial.clone()));
        assert!(trial
            .to_text()
            .starts_with("version 0.10.0\nbuild abc1234\nprevious 0.9.3\n"));
        assert_eq!(Trial::parse("version 0.10.0\n"), None);
    }

    #[test]
    fn the_new_version_runs_on_trial_until_it_commits() {
        let (device, trial) = Device::mid_trial();
        assert_eq!(
            check_at_start(&device.paths, NEW).unwrap(),
            StartCheck::OnTrial(trial)
        );
        commit(&device.paths).unwrap();
        assert_eq!(
            check_at_start(&device.paths, NEW).unwrap(),
            StartCheck::Normal
        );
        assert_eq!(device.read(Target::Waved).as_deref(), Some("new waved"));
        assert!(
            Paths::prev(&device.paths.target(Target::Waved)).exists(),
            "last known good stays"
        );
        assert_eq!(device.settings().as_deref(), Some("new settings"));
        assert!(
            !Paths::prev(&device.paths.config()).exists(),
            "the settings backup goes: restoring it later would lose settings"
        );
    }

    #[test]
    fn an_install_interrupted_before_the_binary_swap_puts_the_deploy_files_back() {
        let (device, trial) = Device::mid_trial();
        // Power was cut after the trial file but before the rename: the old
        // binary is still in place (and waved.prev is its hard link).
        fs::write(device.paths.target(Target::Waved), "old waved").unwrap();
        assert_eq!(
            check_at_start(&device.paths, OLD).unwrap(),
            StartCheck::Interrupted(trial)
        );
        assert_eq!(device.read(Target::Waved).as_deref(), Some("old waved"));
        assert_eq!(device.read(Target::S60waved).as_deref(), Some("old init"));
        assert_eq!(device.read(Target::SplashFb), None);
        assert_eq!(device.settings().as_deref(), Some("old settings"));
        assert!(pending(&device.paths).is_none());
    }

    /// Names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_guard_goes_back_on_the_third_boot_without_a_commit() {
        let (device, _trial) = Device::mid_trial();
        let init_d = device.paths.target(Target::S60waved);
        let init_d = init_d.parent().unwrap().to_path_buf();
        // The old copies are dot names, out of rcS's S??* glob; the guard
        // must find them by the same rule.
        assert_eq!(
            names(&init_d),
            [".S03splash.prev", ".S60waved.prev", "S03splash", "S60waved"]
        );
        assert!(device.paths.waved_dir().join(".waved.prev").exists());
        // Two boots with a few starts each (the speaker can be busy for the
        // first seconds of a boot, and the first start is always killed
        // after 5 s): still the new version.
        for boot in 1..=MAX_TRIAL_BOOTS {
            for _ in 0..3 {
                device.guard();
            }
            assert_eq!(boots(&device.paths), boot);
            assert_eq!(device.read(Target::Waved).as_deref(), Some("new waved"));
            device.reboot();
        }
        device.guard();
        assert_eq!(device.read(Target::Waved).as_deref(), Some("old waved"));
        assert_eq!(device.read(Target::S60waved).as_deref(), Some("old init"));
        assert_eq!(
            device.read(Target::S03splash).as_deref(),
            Some("old boot splash")
        );
        assert_eq!(names(&init_d), ["S03splash", "S60waved"]);
        assert!(!device.paths.waved_dir().join(".waved.prev").exists());
        // The settings go back with the binary: the old one rejects keys it
        // does not know and would run on defaults.
        assert_eq!(device.settings().as_deref(), Some("old settings"));
        assert!(!device.paths.waved_dir().join(".config.toml.prev").exists());
        assert_eq!(
            device.read(Target::SplashFb),
            None,
            "the added file is gone"
        );
        assert!(pending(&device.paths).is_none());
        assert_eq!(boots(&device.paths), 0);

        // The old waved finds out at its start and reports it once.
        match check_at_start(&device.paths, OLD).unwrap() {
            StartCheck::RolledBack(trial) => assert_eq!(trial.version, NEW),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            check_at_start(&device.paths, OLD).unwrap(),
            StartCheck::Normal
        );
    }

    #[test]
    fn a_boot_with_many_starts_keeps_the_new_version() {
        let (device, _trial) = Device::mid_trial();
        for _ in 0..MAX_TRIAL_STARTS_PER_BOOT {
            device.guard();
        }
        assert_eq!(boots(&device.paths), 1);
        assert_eq!(device.read(Target::Waved).as_deref(), Some("new waved"));
    }

    #[test]
    fn a_crash_loop_goes_back_within_the_boot() {
        let (device, _trial) = Device::mid_trial();
        for _ in 0..MAX_TRIAL_STARTS_PER_BOOT {
            device.guard();
        }
        device.guard();
        assert_eq!(device.read(Target::Waved).as_deref(), Some("old waved"));
        assert!(pending(&device.paths).is_none());
    }

    #[test]
    fn the_guard_limits_match_the_init_script() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/S60waved");
        let text = fs::read_to_string(script).unwrap();
        assert!(text.contains(&format!("MAX_TRIAL_BOOTS={MAX_TRIAL_BOOTS}\n")));
        assert!(text.contains(&format!("MAX_TRIAL_STARTS_PER_BOOT={MAX_TRIAL_STARTS_PER_BOOT}\n")));
    }

    #[test]
    fn the_guard_leaves_a_normal_start_alone() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        fs::create_dir_all(paths.waved_dir()).unwrap();
        fs::write(paths.target(Target::Waved), "waved").unwrap();
        let device = Device { _dir: dir, paths };
        device.guard();
        device.guard();
        assert!(!device.paths.boots().exists());
        assert!(!device.paths.boot_starts().exists());
        assert_eq!(device.read(Target::Waved).as_deref(), Some("waved"));
    }

    #[test]
    fn a_commit_between_starts_stops_the_count() {
        let (device, _trial) = Device::mid_trial();
        device.guard();
        device.guard();
        commit(&device.paths).unwrap();
        for _ in 0..3 {
            for _ in 0..MAX_TRIAL_STARTS_PER_BOOT + 1 {
                device.guard();
            }
            device.reboot();
        }
        assert_eq!(device.read(Target::Waved).as_deref(), Some("new waved"));
    }

    #[test]
    fn the_init_script_is_valid_sh() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/S60waved");
        let out = Command::new("sh").arg("-n").arg(&script).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn skipped_versions_are_remembered_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        assert!(!is_skipped(&paths, NEW, "a"));
        skip(&paths, NEW, "a").unwrap();
        skip(&paths, NEW, "a").unwrap();
        assert!(is_skipped(&paths, NEW, "a"));
        assert!(!is_skipped(&paths, NEW, "b"));
        assert_eq!(skipped(&paths).len(), 1);
        unskip(&paths, NEW, "a").unwrap();
        assert!(!is_skipped(&paths, NEW, "a"));
        for patch in 0..20 {
            skip(&paths, Version::new(1, 0, patch), "").unwrap();
        }
        assert_eq!(skipped(&paths).len(), 8);
    }
}
