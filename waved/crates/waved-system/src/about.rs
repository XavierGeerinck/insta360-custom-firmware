//! What About shows (TODO E7): version, serial, storage and licences. The
//! device name and IP come from config and Wi-Fi state, not from here.

use std::path::{Path, PathBuf};
use waved_state::AboutInfo;

/// Where the serial may live, first match wins. Confirmed in the device pass.
pub const SERIAL_SOURCES: &[&str] = &["/unifykey/usid", "/proc/device-tree/serial-number"];

pub const LICENCES: &[&str] = &[
    "waved: MIT",
    "LVGL: MIT",
    "Barlow fonts: SIL Open Font License 1.1",
    "Tabler Icons: MIT",
    "Other libraries: listed in the waved source code",
];

/// "0.1.0 (374015f)", or just "0.1.0" when the build had no git.
pub fn version() -> String {
    let id = env!("WAVED_BUILD_ID");
    if id.is_empty() {
        env!("CARGO_PKG_VERSION").to_string()
    } else {
        format!("{} ({id})", env!("CARGO_PKG_VERSION"))
    }
}

/// The first source that holds printable text, trimmed of NULs and whitespace.
pub fn read_serial(sources: &[PathBuf]) -> String {
    sources
        .iter()
        .filter_map(|path| std::fs::read(path).ok())
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_string()
        })
        .find(|text| !text.is_empty() && text.chars().all(|c| c.is_ascii_graphic()))
        .unwrap_or_default()
}

/// Free and total bytes of the filesystem holding `path`.
#[allow(clippy::unnecessary_cast)] // statvfs field types differ between Linux and macOS
pub fn storage(path: &Path) -> Option<(u64, u64)> {
    let stats = nix::sys::statvfs::statvfs(path).ok()?;
    let block = stats.fragment_size() as u64;
    Some((stats.blocks_available() as u64 * block, stats.blocks() as u64 * block))
}

pub fn collect(serial_sources: &[PathBuf], storage_path: &Path) -> AboutInfo {
    let (free, total) = storage(storage_path).unwrap_or((0, 0));
    AboutInfo {
        version: version(),
        serial: read_serial(serial_sources),
        storage_free_bytes: free,
        storage_total_bytes: total,
        licences: LICENCES.iter().map(|line| line.to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_names_the_crate_version() {
        let version = version();
        assert!(version.starts_with(env!("CARGO_PKG_VERSION")), "{version}");
        if !env!("WAVED_BUILD_ID").is_empty() {
            assert!(version.ends_with(')'), "{version}");
        }
    }

    #[test]
    fn the_first_readable_serial_wins() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        let junk = dir.path().join("junk");
        let good = dir.path().join("good");
        std::fs::write(&empty, "\0\n").unwrap();
        std::fs::write(&junk, [1u8, 2, 3]).unwrap();
        std::fs::write(&good, "WAVE1234567\n\0").unwrap();
        let sources = vec![dir.path().join("missing"), empty, junk, good];
        assert_eq!(read_serial(&sources), "WAVE1234567");
        assert_eq!(read_serial(&[]), "", "no serial is an empty string");
    }

    #[test]
    fn storage_reports_the_filesystem_of_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let (free, total) = storage(dir.path()).unwrap();
        assert!(total > 0 && free <= total);
        assert_eq!(storage(Path::new("/nonexistent/recordings")), None);
    }

    #[test]
    fn about_lists_the_licences() {
        let dir = tempfile::tempdir().unwrap();
        let about = collect(&[], dir.path());
        assert!(about.licences.iter().any(|line| line == "LVGL: MIT"));
        assert!(about.storage_total_bytes > 0);
        assert_eq!(about.serial, "");
    }
}
