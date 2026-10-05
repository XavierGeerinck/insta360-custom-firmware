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
    /// A step failed; the next start-up tries again.
    Retrying { attempt: u32 },
    /// Still failing after `MAX_ATTEMPTS`; the marker is removed.
    GaveUp { attempt: u32 },
}

/// Leave the marker for the next start-up. Nothing is erased yet.
pub fn request(paths: &ResetPaths) -> io::Result<()> {
    write_marker(&paths.marker, 0)
}

/// Finish a requested reset. Call first thing at start-up, before config.toml
/// is loaded and before any service opens the database or recordings.
pub fn complete_pending(paths: &ResetPaths) -> ResetOutcome {
    let Some(previous) = read_attempts(&paths.marker) else {
        return ResetOutcome::NothingPending;
    };
    let attempt = previous.saturating_add(1);
    if let Err(error) = write_marker(&paths.marker, attempt) {
        warn!("factory reset: could not count attempt {attempt}: {error}");
    }
    let mut ok = true;
    let mut step = |name: &str, result: io::Result<()>| {
        if let Err(error) = result {
            warn!("factory reset: {name} not erased: {error}");
            ok = false;
        }
    };
    step("config", remove_prefixed(&paths.config));
    step("database", remove_prefixed(&paths.database));
    step("recordings", erase_recordings(paths));
    if let Some(conf) = &paths.wifi_conf {
        step("Wi-Fi networks", reset_wifi(conf, paths.reload_wifi));
    }
    if ok {
        finish(paths);
        info!("factory reset done");
        ResetOutcome::Completed
    } else if attempt >= MAX_ATTEMPTS {
        finish(paths);
        error!("factory reset still incomplete after {attempt} attempts; giving up");
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
                Err(error) => error!("factory reset not started: {error}"),
            },
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!("factory reset handler lagged {skipped} commands");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

fn write_marker(marker: &Path, attempts: u32) -> io::Result<()> {
    let temp = marker.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temp)?;
    writeln!(file, "attempts={attempts}")?;
    file.sync_all()?;
    std::fs::rename(&temp, marker)?;
    if let Some(dir) = marker.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

fn read_attempts(marker: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(marker).ok()?;
    Some(
        text.trim()
            .strip_prefix("attempts=")
            .and_then(|n| n.parse().ok())
            .unwrap_or(0),
    )
}

fn finish(paths: &ResetPaths) {
    if let Err(error) = std::fs::remove_file(&paths.marker) {
        warn!("factory reset: marker not removed: {error}");
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

fn erase_recordings(paths: &ResetPaths) -> io::Result<()> {
    if let Some(mount) = &paths.recordings_mount {
        if !is_mount_point(mount) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not mounted", mount.display()),
            ));
        }
    }
    let entries = match std::fs::read_dir(&paths.recordings) {
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
    Ok(())
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
            Ok(output) => warn!("factory reset: wpa_cli reconfigure exited with {}", output.status),
            Err(error) => warn!("factory reset: wpa_cli not run: {error}"),
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
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Retrying { attempt: 2 });
        assert_eq!(complete_pending(&d.paths), ResetOutcome::GaveUp { attempt: 3 });
        assert!(!d.paths.marker.exists(), "no erase loop on every boot");
    }

    #[test]
    fn a_power_cut_mid_reset_repeats_it() {
        let d = device();
        std::fs::write(&d.paths.marker, "attempts=1\n").unwrap(); // cut after the first count
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Completed);
        assert!(!d.paths.config.exists() && !d.paths.marker.exists());
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
