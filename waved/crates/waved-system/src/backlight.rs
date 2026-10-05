//! The LCD backlight in sysfs: `brightness` takes 0..=`max_brightness`
//! (255 on the device). 0 blanks the panel (docs/DISPLAY_HARDWARE.md).

use std::io;
use std::path::{Path, PathBuf};

pub const BACKLIGHT_DIR: &str = "/sys/class/leds/lcd-backlight";
/// Used when `max_brightness` can't be read; the device reports 255.
pub const DEFAULT_MAX: u32 = 255;

/// `pct` (0..=100) of `max`, rounded. A lit percentage never rounds to 0.
pub fn raw_level(pct: u8, max: u32) -> u32 {
    if pct == 0 {
        return 0;
    }
    ((u32::from(pct.min(100)) * max + 50) / 100).max(1)
}

#[derive(Debug)]
pub struct Backlight {
    brightness: PathBuf,
    max: u32,
    last: Option<u32>,
}

impl Backlight {
    pub fn open(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let max = std::fs::read_to_string(dir.join("max_brightness"))
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|max| *max > 0)
            .unwrap_or(DEFAULT_MAX);
        Self {
            brightness: dir.join("brightness"),
            max,
            last: None,
        }
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    /// Write the level for `pct` when it differs from the last level written
    /// or attempted. `Ok(true)` when it was written.
    pub fn set_percent(&mut self, pct: u8) -> io::Result<bool> {
        let raw = raw_level(pct, self.max);
        if self.last == Some(raw) {
            return Ok(false);
        }
        self.last = Some(raw);
        std::fs::write(&self.brightness, raw.to_string())?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn led(max: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(max) = max {
            std::fs::write(dir.path().join("max_brightness"), max).unwrap();
        }
        std::fs::write(dir.path().join("brightness"), "255").unwrap();
        dir
    }

    fn level(dir: &tempfile::TempDir) -> String {
        std::fs::read_to_string(dir.path().join("brightness")).unwrap()
    }

    #[test]
    fn percent_maps_onto_the_backlight_range() {
        assert_eq!(raw_level(80, 255), 204);
        assert_eq!(raw_level(100, 255), 255);
        assert_eq!(raw_level(5, 255), 13);
        assert_eq!(raw_level(0, 255), 0);
        assert_eq!(raw_level(1, 10), 1, "a lit level never rounds down to dark");
        assert_eq!(raw_level(200, 255), 255);
    }

    #[test]
    fn max_brightness_comes_from_sysfs_with_a_fallback() {
        assert_eq!(Backlight::open(led(Some("1023\n")).path()).max(), 1023);
        assert_eq!(Backlight::open(led(None).path()).max(), DEFAULT_MAX);
        assert_eq!(Backlight::open(led(Some("0")).path()).max(), DEFAULT_MAX);
        assert_eq!(Backlight::open(led(Some("junk")).path()).max(), DEFAULT_MAX);
    }

    #[test]
    fn the_level_is_written_only_when_it_changes() {
        let dir = led(Some("255\n"));
        let mut backlight = Backlight::open(dir.path());
        assert!(backlight.set_percent(80).unwrap());
        assert_eq!(level(&dir), "204");
        std::fs::write(dir.path().join("brightness"), "x").unwrap();
        assert!(!backlight.set_percent(80).unwrap());
        assert_eq!(level(&dir), "x", "an unchanged level is not rewritten");
        assert!(backlight.set_percent(0).unwrap());
        assert_eq!(level(&dir), "0");
    }

    #[test]
    fn a_missing_backlight_fails_once_per_level() {
        let mut backlight = Backlight::open("/nonexistent/lcd-backlight");
        assert!(backlight.set_percent(50).is_err());
        assert!(!backlight.set_percent(50).unwrap(), "no retry storm every frame");
    }
}
