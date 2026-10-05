//! Factory reset (TODO E6): erase config, the meetings database, recordings
//! and saved Wi-Fi networks. The request only leaves a marker and restarts
//! waved; the erase runs at the next start-up, before anything opens these
//! files, and repeats after a power cut.

use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use waved_state::Command;

const STEP_CONFIG: &str = "config";
const STEP_DATABASE: &str = "database";
const STEP_RECORDINGS: &str = "recordings";
const STEP_WIFI: &str = "wifi";
const STEPS: [&str; 4] = [STEP_CONFIG, STEP_DATABASE, STEP_RECORDINGS, STEP_WIFI];

pub const MARKER_NAME: &str = "factory-reset.pending";
/// Start-ups that try to finish a reset before it is given up.
pub const MAX_ATTEMPTS: u32 = 3;
pub const WIFI_CONF: &str = "/etc/wpa_supplicant.conf";
pub const RECORDINGS_MOUNT: &str = "/mnt/udisk";
/// The station config with no saved networks (the stock image's header).
pub const CLEAN_WPA_CONF: &str = "ctrl_interface=/var/run/wpa_supplicant\nap_scan=1\nupdate_config=1\n";

#[derive(Clone, Debug, PartialEq)]
pub struct ResetPaths {
    pub marker: PathBuf,
    /// Removed with every sibling whose name starts with its name.
    pub config: PathBuf,
    /// Removed with every sibling whose name starts with its name.
    pub database: PathBuf,
    /// Emptied; the directory itself stays.
    pub recordings: PathBuf,
    /// When set, recordings are erased only while this is a mount point.
    pub recordings_mount: Option<PathBuf>,
    /// When set, rewritten to `CLEAN_WPA_CONF`.
    pub wifi_conf: Option<PathBuf>,
    /// Run `wpa_cli -i wlan0 reconfigure` after rewriting the Wi-Fi config.
    pub reload_wifi: bool,
}

impl ResetPaths {
    pub fn new(config: impl Into<PathBuf>, database: impl Into<PathBuf>, recordings: impl Into<PathBuf>) -> Self {
        let config = config.into();
        Self {
            marker: config.with_file_name(MARKER_NAME),
            config,
            database: database.into(),
            recordings: recordings.into(),
            recordings_mount: None,
            wifi_conf: None,
            reload_wifi: false,
        }
    }

    /// The device's recordings mount check and Wi-Fi config.
    pub fn on_device(mut self) -> Self {
        self.recordings_mount = Some(PathBuf::from(RECORDINGS_MOUNT));
        self.wifi_conf = Some(PathBuf::from(WIFI_CONF));
        self.reload_wifi = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetOutcome {
    NothingPending,
    Completed,
    /// A step failed, or the attempt could not be saved (then nothing was
    /// erased); the next start-up tries again.
    Retrying { attempt: u32 },
    /// Still failing after `MAX_ATTEMPTS`; the marker is removed.
    GaveUp { attempt: u32 },
}

/// Leave the marker for the next start-up. Nothing is erased yet.
pub fn request(paths: &ResetPaths) -> io::Result<()> {
    write_marker(&paths.marker, 0, &[])
}

/// Finish a requested reset. Call first thing at start-up, before config.toml
/// is loaded and before any service opens the database or recordings.
pub fn complete_pending(paths: &ResetPaths) -> ResetOutcome {
    let Some((previous, mut done)) = read_marker(&paths.marker) else {
        return ResetOutcome::NothingPending;
    };
    let attempt = previous.saturating_add(1);
    // Without a saved count the attempt cap cannot hold, and steps that still
    // work (recordings, Wi-Fi) would repeat on every boot. Erase nothing.
    if let Err(error) = write_marker(&paths.marker, attempt, &done) {
        error!(
            "factory reset: attempt {attempt} could not be saved, so nothing is erased this start-up; the next start-up tries again: {error}"
        );
        return ResetOutcome::Retrying { attempt };
    }
    let next = if attempt >= MAX_ATTEMPTS {
        "the reset gives up"
    } else {
        "the next start-up tries again"
    };
    let mut ok = true;
    // A step that finished on an earlier start-up is never run again: the user
    // may have set the device up since.
    let mut step = |name: &'static str, run: &dyn Fn() -> io::Result<()>| {
        if done.contains(&name) {
            return;
        }
        match run() {
            Ok(()) => {
                done.push(name);
                if let Err(error) = write_marker(&paths.marker, attempt, &done) {
                    warn!("factory reset: {name} erased but not noted as done; a retry would erase it again: {error}");
                }
            }
            Err(error) => {
                warn!("factory reset: {name} not erased; {next}: {error}");
                ok = false;
            }
        }
    };
    step(STEP_CONFIG, &|| remove_prefixed(&paths.config));
    step(STEP_DATABASE, &|| remove_prefixed(&paths.database));
    step(STEP_RECORDINGS, &|| erase_recordings(paths));
    if let Some(conf) = &paths.wifi_conf {
        step(STEP_WIFI, &|| reset_wifi(conf, paths.reload_wifi));
    }
    if ok {
        finish(paths);
        info!("factory reset done");
        ResetOutcome::Completed
    } else if attempt >= MAX_ATTEMPTS {
        finish(paths);
        error!("factory reset gave up after {attempt} attempts; the steps that failed are not tried again");
        ResetOutcome::GaveUp { attempt }
    } else {
        ResetOutcome::Retrying { attempt }
    }
}

/// True when `path` is on a different filesystem from its parent.
pub fn is_mount_point(path: &Path) -> bool {
    let (Ok(own), Some(parent)) = (std::fs::metadata(path), path.parent()) else {
        return false;
    };
    std::fs::metadata(parent).is_ok_and(|parent| parent.dev() != own.dev())
}

/// On `Command::FactoryReset`: leave the marker and restart waved
/// (`RestartForConfig`). The erase runs at the next start-up.
pub async fn run(paths: ResetPaths, mut commands: broadcast::Receiver<Command>, bus: broadcast::Sender<Command>) {
    loop {
        match commands.recv().await {
            Ok(Command::FactoryReset) => match request(&paths) {
                Ok(()) => {
                    info!("factory reset requested; restarting to erase");
                    let _ = bus.send(Command::RestartForConfig);
                }
                Err(error) => error!("factory reset not started, nothing was erased and waved keeps running: {error}"),
            },
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!("factory reset handler lagged {skipped} commands; a reset request may have been missed");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// `attempts=N` and `done=config,database,...` (the steps already finished).
fn write_marker(marker: &Path, attempts: u32, done: &[&str]) -> io::Result<()> {
    let temp = marker.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temp)?;
    writeln!(file, "attempts={attempts}")?;
    writeln!(file, "done={}", done.join(","))?;
    file.sync_all()?;
    std::fs::rename(&temp, marker)?;
    if let Some(dir) = marker.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// The attempts so far and the steps already done. An old, short or garbled
/// marker (even one that is not UTF-8) still counts as a request, with nothing
/// done yet.
fn read_marker(marker: &Path) -> Option<(u32, Vec<&'static str>)> {
    let bytes = std::fs::read(marker).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut attempts = 0;
    let mut done = Vec::new();
    for line in text.lines() {
        if let Some(n) = line.trim().strip_prefix("attempts=") {
            attempts = n.parse().unwrap_or(0);
        } else if let Some(list) = line.trim().strip_prefix("done=") {
            done = STEPS.into_iter().filter(|step| list.split(',').any(|name| name == *step)).collect();
        }
    }
    Some((attempts, done))
}

fn finish(paths: &ResetPaths) {
    if let Err(error) = std::fs::remove_file(&paths.marker) {
        warn!("factory reset: marker not removed; the next start-up runs only the steps not noted as done, then removes it: {error}");
    }
}

/// Remove `path` and every regular file beside it whose name starts with its
/// name (config.toml.bak, meetings.db-wal, ...).
fn remove_prefixed(path: &Path) -> io::Result<()> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(());
    };
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let name = name.to_string_lossy();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(name.as_ref()) && entry.file_type()?.is_file() {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// The directory to empty, only when it is a real place on the recordings
/// volume: never empty, never the volume itself, never outside it.
fn recordings_target(paths: &ResetPaths) -> io::Result<&Path> {
    let recordings = paths.recordings.as_path();
    let refuse = |why: &str| {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("recordings path {} {why}", recordings.display()),
        ))
    };
    if recordings.as_os_str().is_empty() {
        return refuse("is empty");
    }
    if recordings.components().any(|part| part == std::path::Component::ParentDir) {
        return refuse("contains ..");
    }
    if let Some(mount) = &paths.recordings_mount {
        if !recordings.is_absolute() || !recordings.starts_with(mount) {
            return refuse(&format!("is not inside {}", mount.display()));
        }
        if recordings == mount.as_path() {
            return refuse("is the volume itself");
        }
    }
    Ok(recordings)
}

fn erase_recordings(paths: &ResetPaths) -> io::Result<()> {
    let recordings = recordings_target(paths)?;
    if let Some(mount) = &paths.recordings_mount {
        if !is_mount_point(mount) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not mounted", mount.display()),
            ));
        }
    }
    let entries = match std::fs::read_dir(recordings) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    // The recordings volume is not /data: the marker's sync does not cover it.
    // Flush the deletes before the step is noted as done.
    sync_filesystem(recordings)
}

/// Flush the filesystem that holds `dir` to disk.
fn sync_filesystem(dir: &Path) -> io::Result<()> {
    let file = std::fs::File::open(dir)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `file` stays open for the call, so the descriptor is valid,
        // and syncfs reads no memory of ours.
        if unsafe { libc::syncfs(file.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Host builds only: syncfs is Linux-only.
        file.sync_all()
    }
}

/// Written in place, because `/etc/wpa_supplicant.conf` may be a link into `/data`.
fn reset_wifi(conf: &Path, reload: bool) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(conf)?;
    file.write_all(CLEAN_WPA_CONF.as_bytes())?;
    file.sync_all()?;
    std::fs::set_permissions(conf, std::fs::Permissions::from_mode(0o600))?;
    if reload {
        match std::process::Command::new("wpa_cli").args(["-i", "wlan0", "reconfigure"]).output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => warn!(
                "factory reset: wpa_cli reconfigure exited with {}; saved networks stay live until the next restart",
                output.status
            ),
            Err(error) => warn!("factory reset: wpa_cli not run; saved networks stay live until the next restart: {error}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Device {
        dir: tempfile::TempDir,
        paths: ResetPaths,
    }

    fn device() -> Device {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data/waved");
        let recordings = dir.path().join("udisk/recordings");
        let etc = dir.path().join("etc");
        for path in [data.clone(), recordings.join("2026-10-05_0930"), etc.clone()] {
            std::fs::create_dir_all(path).unwrap();
        }
        for (path, body) in [
            (data.join("config.toml"), "config_version = 2\n"),
            (data.join("config.toml.bak"), "old"),
            (data.join("config.toml.unloadable.2"), "bad"),
            (data.join("meetings.db"), "db"),
            (data.join("meetings.db-wal"), "wal"),
            (data.join("waved"), "binary"),
            (data.join("playback.s16le"), "pcm"),
            (recordings.join("2026-10-05_0930/audio.wav"), "RIFF"),
            (recordings.join("stray.tmp"), "tmp"),
            (etc.join("wpa_supplicant.conf"), "network={\n ssid=\"office\"\n}\n"),
        ] {
            std::fs::write(path, body).unwrap();
        }
        let mut paths = ResetPaths::new(data.join("config.toml"), data.join("meetings.db"), recordings);
        paths.wifi_conf = Some(etc.join("wpa_supplicant.conf"));
        Device { dir, paths }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_next_start_erases_config_database_recordings_and_wifi() {
        let d = device();
        let data = d.paths.config.parent().unwrap().to_path_buf();
        request(&d.paths).unwrap();
        assert!(d.paths.config.exists(), "a request erases nothing by itself");
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Completed);
        assert_eq!(names(&data), ["playback.s16le", "waved"], "the binary survives");
        assert!(d.paths.recordings.is_dir());
        assert!(names(&d.paths.recordings).is_empty());
        let conf = d.paths.wifi_conf.as_ref().unwrap();
        assert_eq!(std::fs::read_to_string(conf).unwrap(), CLEAN_WPA_CONF);
        assert_eq!(std::fs::metadata(conf).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(complete_pending(&d.paths), ResetOutcome::NothingPending);
    }

    #[test]
    fn nothing_pending_touches_nothing() {
        let d = device();
        assert_eq!(complete_pending(&d.paths), ResetOutcome::NothingPending);
        assert!(d.paths.config.exists() && d.paths.database.exists());
    }

    #[test]
    fn an_unmounted_recordings_volume_is_retried_then_given_up() {
        let mut d = device();
        // Same filesystem as its parent, so not a mount point.
        d.paths.recordings_mount = Some(d.dir.path().join("udisk"));
        request(&d.paths).unwrap();
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Retrying { attempt: 1 });
        assert!(!d.paths.config.exists(), "the other steps still ran");
        assert!(d.paths.marker.exists());
        assert!(d.paths.recordings.join("stray.tmp").exists(), "an unmounted volume is never erased");
        // The user sets the device up again between start-ups.
        let conf = d.paths.wifi_conf.clone().unwrap();
        let set_up = |d: &Device| {
            std::fs::write(&d.paths.config, "config_version = 2\nfirst_run_done = true\n").unwrap();
            std::fs::write(&d.paths.database, "new meetings").unwrap();
            std::fs::write(&conf, "network={\n ssid=\"home\"\n}\n").unwrap();
        };
        set_up(&d);
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Retrying { attempt: 2 });
        assert!(d.paths.config.exists() && d.paths.database.exists(), "finished steps are not repeated");
        assert!(std::fs::read_to_string(&conf).unwrap().contains("home"));
        assert_eq!(complete_pending(&d.paths), ResetOutcome::GaveUp { attempt: 3 });
        assert!(d.paths.config.exists() && d.paths.database.exists());
        assert!(std::fs::read_to_string(&conf).unwrap().contains("home"));
        assert!(!d.paths.marker.exists(), "no erase loop on every boot");
    }

    #[test]
    fn a_step_not_noted_as_done_is_repeated() {
        let d = device();
        std::fs::write(&d.paths.marker, "attempts=1\ndone=database\n").unwrap();
        std::fs::write(&d.paths.database, "new").unwrap();
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Completed);
        assert!(!d.paths.config.exists(), "config was not noted, so it is erased");
        assert!(d.paths.database.exists(), "the database was noted, so it is kept");
    }

    #[test]
    fn an_unreadable_done_list_means_nothing_done() {
        let d = device();
        std::fs::write(&d.paths.marker, "attempts=1\ndone=%%,,wifi-ish\n").unwrap();
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Completed);
        assert!(!d.paths.config.exists() && !d.paths.database.exists());
    }

    #[test]
    fn recordings_are_erased_only_inside_the_volume() {
        let mut d = device();
        let mount = d.dir.path().join("udisk");
        d.paths.recordings_mount = Some(mount.clone());
        let invalid = |recordings: PathBuf| {
            let mut paths = d.paths.clone();
            paths.recordings = recordings;
            erase_recordings(&paths).unwrap_err().kind() == io::ErrorKind::InvalidInput
        };
        assert!(invalid(mount.clone()), "the volume itself");
        assert!(invalid(d.dir.path().join("data/waved")), "outside the volume");
        assert!(invalid(PathBuf::new()), "empty");
        assert!(invalid(mount.join("../data/waved")), "dot-dot");
        assert!(invalid(PathBuf::from("udisk/recordings")), "relative");
        let mut host = d.paths.clone();
        host.recordings_mount = None;
        host.recordings = PathBuf::new();
        assert!(erase_recordings(&host).is_err(), "empty is refused without a mount too");
        assert!(d.dir.path().join("data/waved/waved").exists());
        assert!(mount.join("recordings/stray.tmp").exists());
        // The reset as a whole does not report success for it.
        d.paths.recordings = mount;
        request(&d.paths).unwrap();
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Retrying { attempt: 1 });
        assert!(d.paths.recordings.join("recordings/stray.tmp").exists());
    }

    #[test]
    fn a_power_cut_mid_reset_repeats_it() {
        let d = device();
        std::fs::write(&d.paths.marker, "attempts=1\n").unwrap(); // cut after the first count
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Completed);
        assert!(!d.paths.config.exists() && !d.paths.marker.exists());
    }

    #[test]
    fn an_attempt_that_cannot_be_counted_erases_nothing() {
        let d = device();
        // A directory where the marker's temp file goes: every marker write
        // fails, as it would on a read-only /data, even for root.
        std::fs::create_dir(d.paths.marker.with_extension("tmp")).unwrap();
        for (marker, attempt) in [("attempts=0\ndone=\n", 1), ("attempts=2\ndone=\n", 3)] {
            std::fs::write(&d.paths.marker, marker).unwrap();
            assert_eq!(complete_pending(&d.paths), ResetOutcome::Retrying { attempt });
            assert!(d.paths.config.exists() && d.paths.database.exists(), "no step ran");
            assert!(d.paths.recordings.join("stray.tmp").exists(), "recordings are not erased");
            let conf = std::fs::read_to_string(d.paths.wifi_conf.as_ref().unwrap()).unwrap();
            assert!(conf.contains("office"), "saved networks stay");
            assert_eq!(std::fs::read_to_string(&d.paths.marker).unwrap(), marker, "the marker is left as it was");
        }
    }

    #[test]
    fn a_marker_that_is_not_text_still_counts_as_a_request() {
        let d = device();
        std::fs::write(&d.paths.marker, b"attempts=1\n\xff\xfe\x80\n").unwrap();
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Completed);
        assert!(!d.paths.config.exists() && !d.paths.database.exists());
        assert!(names(&d.paths.recordings).is_empty());
        assert!(!d.paths.marker.exists());
    }

    #[test]
    fn flushing_the_recordings_volume_reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        sync_filesystem(dir.path()).unwrap();
        assert!(sync_filesystem(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn plain_directories_are_not_mount_points() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(!is_mount_point(&dir.path().join("sub")));
        assert!(!is_mount_point(&dir.path().join("missing")));
        assert!(!is_mount_point(Path::new("/")));
    }

    #[tokio::test]
    async fn the_handler_restarts_waved_after_a_reset_request() {
        let d = device();
        let (bus, rx) = broadcast::channel(8);
        let mut watcher = bus.subscribe();
        tokio::spawn(run(d.paths.clone(), rx, bus.clone()));
        bus.send(Command::FactoryReset).unwrap();
        loop {
            let command = tokio::time::timeout(Duration::from_secs(5), watcher.recv()).await.unwrap().unwrap();
            if matches!(command, Command::RestartForConfig) {
                break;
            }
        }
        assert!(d.paths.marker.exists());
        assert!(d.paths.config.exists(), "the erase waits for the next start");
    }
}
