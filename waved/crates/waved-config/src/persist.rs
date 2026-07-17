use crate::{validate_config, ConfigError, WavedConfig};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

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
