use crate::{validate_config, ConfigError, WavedConfig};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The settings service and the setup page both save config.toml from
/// different tasks; they share the temporary and backup names, so one save
/// runs at a time.
static SAVE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn save_atomic(config: &WavedConfig, path: &Path) -> Result<(), ConfigError> {
    validate_config(config).map_err(|errors| {
        ConfigError::Validation(
            errors
                .into_iter()
                .map(|error| format!("{}: {}", error.path, error.message))
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    let _guard = SAVE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let contents = toml::to_string_pretty(config)?;
    // Reparse through the production path before touching the live file.
    WavedConfig::parse(&contents)?;

    let new_path = suffixed(path, ".new");
    let backup_path = suffixed(path, ".bak");
    let backup_new_path = suffixed(path, ".bak.new");

    write_secure_file(&new_path, contents.as_bytes())?;

    if path.exists() {
        fs::copy(path, &backup_new_path)?;
        fs::set_permissions(&backup_new_path, fs::Permissions::from_mode(0o600))?;
        File::open(&backup_new_path)?.sync_all()?;
        fs::rename(&backup_new_path, &backup_path)?;
    }

    fs::rename(&new_path, path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn write_secure_file(path: &Path, contents: &[u8]) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use crate::WavedConfig;

    #[test]
    fn concurrent_saves_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let threads: Vec<_> = (0..4u8)
            .map(|writer| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut config = WavedConfig::default();
                    config.display.brightness_pct = 10 + writer;
                    (0..15)
                        .map(|_| config.save(&path).map_err(|error| error.to_string()))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for thread in threads {
            for result in thread.join().unwrap() {
                assert!(result.is_ok(), "{result:?}");
            }
        }
        assert!(WavedConfig::load(&path).is_ok());
    }
}
