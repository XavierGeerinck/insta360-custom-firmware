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
    check(config)?;
    let _guard = lock();
    save_locked(config, path)
}

/// Save `config`, first moving aside a config.toml that exists but does not
/// load. The old file is renamed to `<name>.unloadable` (or
/// `<name>.unloadable.<n>` when that name is taken), so nothing earlier is
/// overwritten and `.bak` rotation never touches it. Returns the path the old
/// file was kept at, or `None` when the file was missing or loaded fine. The
/// check and the save happen under one lock hold. If the save fails, the old
/// file is put back.
pub(crate) fn save_replacing_unloadable(
    config: &WavedConfig,
    path: &Path,
) -> Result<Option<PathBuf>, ConfigError> {
    check(config)?;
    let _guard = lock();
    let kept = match WavedConfig::load(path) {
        Ok(_) => None,
        Err(ConfigError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(quarantine(path)?),
    };
    if let Err(error) = save_locked(config, path) {
        if let Some(kept) = &kept {
            if let Err(restore) = fs::rename(kept, path) {
                tracing::warn!("could not put {} back: {restore}", path.display());
            }
        }
        return Err(error);
    }
    Ok(kept)
}

fn check(config: &WavedConfig) -> Result<(), ConfigError> {
    validate_config(config).map_err(|errors| {
        ConfigError::Validation(
            errors
                .into_iter()
                .map(|error| format!("{}: {}", error.path, error.message))
                .collect::<Vec<_>>()
                .join("; "),
        )
    })
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    SAVE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Rename `path` to the first free `.unloadable[.n]` name beside it.
fn quarantine(path: &Path) -> Result<PathBuf, std::io::Error> {
    let mut target = suffixed(path, ".unloadable");
    let mut n = 1u32;
    while fs::symlink_metadata(&target).is_ok() {
        n += 1;
        target = suffixed(path, &format!(".unloadable.{n}"));
    }
    fs::rename(path, &target)?;
    // The file may hold secrets; keep it as private as the live one.
    if let Err(error) = fs::set_permissions(&target, fs::Permissions::from_mode(0o600)) {
        tracing::warn!("could not restrict {}: {error}", target.display());
    }
    Ok(target)
}

fn save_locked(config: &WavedConfig, path: &Path) -> Result<(), ConfigError> {
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

    fn broken_files() -> Vec<(&'static str, &'static str)> {
        vec![
            ("syntax error", "api]\nauth_token = \"s3cr3t-token\n"),
            ("newer version", "config_version = 99\n"),
            ("unknown key", "[device]\nnickname = \"x\"\n"),
        ]
    }

    #[test]
    fn saving_over_an_unloadable_file_keeps_the_original_aside() {
        for (what, broken) in broken_files() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, broken).unwrap();
            let mut config = WavedConfig::default();
            config.device.name = "Board room".into();

            let kept = config.save_replacing_unloadable(&path).unwrap();

            let kept = kept.unwrap_or_else(|| panic!("{what}: nothing was kept"));
            assert_eq!(kept, dir.path().join("config.toml.unloadable"), "{what}");
            assert_eq!(std::fs::read_to_string(&kept).unwrap(), broken, "{what}");
            assert_eq!(WavedConfig::load(&path).unwrap(), config, "{what}");
        }
    }

    #[test]
    fn a_second_unloadable_file_does_not_overwrite_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = WavedConfig::default();

        std::fs::write(&path, "config_version = 99\n").unwrap();
        let first = config.save_replacing_unloadable(&path).unwrap().unwrap();
        std::fs::write(&path, "not toml at all [\n").unwrap();
        let second = config.save_replacing_unloadable(&path).unwrap().unwrap();

        assert_ne!(first, second);
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "config_version = 99\n"
        );
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            "not toml at all [\n"
        );
        assert!(WavedConfig::load(&path).is_ok());
    }

    #[test]
    fn saving_over_a_loadable_or_missing_file_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = WavedConfig::default();
        assert_eq!(config.save_replacing_unloadable(&path).unwrap(), None);
        assert_eq!(config.save_replacing_unloadable(&path).unwrap(), None);
        assert!(!dir.path().join("config.toml.unloadable").exists());
    }

    #[test]
    fn quarantining_leaves_the_backup_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut good = WavedConfig::default();
        good.device.name = "Good".into();
        good.save(&path).unwrap();
        WavedConfig::default().save(&path).unwrap(); // rotates "Good" into .bak
        std::fs::write(&path, "config_version = 99\n").unwrap();

        WavedConfig::default()
            .save_replacing_unloadable(&path)
            .unwrap();

        let backup = std::fs::read_to_string(dir.path().join("config.toml.bak")).unwrap();
        assert!(backup.contains("Good"), "{backup}");
    }
}
