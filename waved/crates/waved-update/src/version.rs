//! Versions are `MAJOR.MINOR.PATCH`, compared numerically. The running
//! version is the crate version, which every crate in the workspace shares.
//! `waved-pack` reads it out of a binary through `VERSION_MARKER`, so a
//! package can never claim a version its binary does not report.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a MAJOR.MINOR.PATCH version: {0:?}")]
pub struct BadVersion(pub String);

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl FromStr for Version {
    type Err = BadVersion;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || BadVersion(text.chars().take(32).collect());
        let mut parts = text.split('.');
        let mut next = || -> Result<u32, BadVersion> {
            let part = parts.next().ok_or_else(bad)?;
            let digits_only = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
            let leading_zero = part.len() > 1 && part.starts_with('0');
            if !digits_only || leading_zero {
                return Err(bad());
            }
            part.parse().map_err(|_| bad())
        };
        let version = Version::new(next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What every waved binary carries, NUL-terminated: `waved-version:0.10.0`.
pub const MARKER_PREFIX: &str = "waved-version:";
pub static VERSION_MARKER: &str = concat!("waved-version:", env!("CARGO_PKG_VERSION"), "\0");

/// The version of this build, read from the marker (which keeps the marker
/// in the binary).
pub fn running() -> Version {
    let text = VERSION_MARKER
        .strip_prefix(MARKER_PREFIX)
        .and_then(|rest| rest.strip_suffix('\0'))
        .expect("the marker is built by concat!");
    text.parse()
        .expect("the workspace version is MAJOR.MINOR.PATCH")
}

/// The version a waved binary reports, from its marker. `None` when the
/// binary has no marker, or two that disagree.
pub fn find_marker(binary: &[u8]) -> Option<Version> {
    let prefix = MARKER_PREFIX.as_bytes();
    let mut found: Option<Version> = None;
    let mut at = 0;
    while let Some(offset) = find(&binary[at..], prefix) {
        let start = at + offset + prefix.len();
        let end = binary[start..]
            .iter()
            .take(40)
            .position(|b| *b == 0)
            .map(|len| start + len);
        at = start;
        let Some(end) = end else { continue };
        let Ok(version) = std::str::from_utf8(&binary[start..end])
            .ok()
            .unwrap_or("")
            .parse::<Version>()
        else {
            continue;
        };
        match found {
            Some(seen) if seen != version => return None,
            _ => found = Some(version),
        }
    }
    found
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Whether a candidate may replace the running version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Newer,
    /// Same version, different build, and the owner allows sideways moves.
    Reinstall,
    /// Older, and the owner allows downgrades.
    Downgrade,
    UpToDate,
    /// Older, and downgrades are not allowed.
    RefusedDowngrade,
}

impl Verdict {
    pub fn installs(self) -> bool {
        matches!(self, Self::Newer | Self::Reinstall | Self::Downgrade)
    }
}

/// `allow_downgrade` is `security.ota_allow_downgrade`. A build id that is
/// empty on either side never counts as different, so an unknown build is
/// never reinstalled in a loop.
pub fn judge(
    running: Version,
    running_build: &str,
    candidate: Version,
    candidate_build: &str,
    allow_downgrade: bool,
) -> Verdict {
    use std::cmp::Ordering::*;
    match candidate.cmp(&running) {
        Greater => Verdict::Newer,
        Equal => {
            let different_build = !running_build.is_empty()
                && !candidate_build.is_empty()
                && running_build != candidate_build;
            if allow_downgrade && different_build {
                Verdict::Reinstall
            } else {
                Verdict::UpToDate
            }
        }
        Less if allow_downgrade => Verdict::Downgrade,
        Less => Verdict::RefusedDowngrade,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        text.parse().unwrap()
    }

    #[test]
    fn versions_order_numerically() {
        assert!(v("0.10.0") > v("0.9.3"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.9.10") > v("0.9.9"));
        assert_eq!(v("0.10.0").to_string(), "0.10.0");
    }

    #[test]
    fn only_three_plain_numbers_parse() {
        for bad in [
            "", "1", "1.2", "1.2.3.4", "1.2.x", "v1.2.3", "1.2.3-rc1", " 1.2.3", "01.2.3",
            "1..3", "1.2.99999999999",
        ] {
            assert!(bad.parse::<Version>().is_err(), "{bad:?}");
        }
        assert_eq!(v("0.0.0"), Version::new(0, 0, 0));
    }

    #[test]
    fn newer_installs_and_the_same_is_up_to_date() {
        assert_eq!(judge(v("0.9.3"), "a", v("0.10.0"), "b", false), Verdict::Newer);
        assert_eq!(judge(v("0.9.3"), "a", v("0.9.3"), "b", false), Verdict::UpToDate);
        assert_eq!(judge(v("0.9.3"), "a", v("0.9.3"), "a", true), Verdict::UpToDate);
    }

    #[test]
    fn a_downgrade_is_refused_unless_the_owner_allows_it() {
        assert_eq!(
            judge(v("0.10.0"), "a", v("0.9.3"), "b", false),
            Verdict::RefusedDowngrade
        );
        assert!(!Verdict::RefusedDowngrade.installs());
        assert_eq!(judge(v("0.10.0"), "a", v("0.9.3"), "b", true), Verdict::Downgrade);
        assert!(Verdict::Downgrade.installs());
    }

    #[test]
    fn a_sideways_reinstall_needs_the_allowance_and_two_known_builds() {
        assert_eq!(judge(v("0.9.3"), "a", v("0.9.3"), "b", true), Verdict::Reinstall);
        assert_eq!(judge(v("0.9.3"), "", v("0.9.3"), "b", true), Verdict::UpToDate);
        assert_eq!(judge(v("0.9.3"), "a", v("0.9.3"), "", true), Verdict::UpToDate);
    }

    #[test]
    fn the_running_version_is_the_crate_version_and_its_marker_is_findable() {
        assert_eq!(running().to_string(), env!("CARGO_PKG_VERSION"));
        let mut binary = vec![7u8; 1000];
        binary.extend_from_slice(VERSION_MARKER.as_bytes());
        binary.extend_from_slice(&[9u8; 1000]);
        assert_eq!(find_marker(&binary), Some(running()));
    }

    #[test]
    fn a_binary_without_a_marker_or_with_two_different_ones_has_no_version() {
        assert_eq!(find_marker(b"no marker here"), None);
        assert_eq!(find_marker(b"waved-version:garbage\0"), None);
        let two = b"waved-version:0.9.3\0....waved-version:0.10.0\0";
        assert_eq!(find_marker(two), None);
        let same = b"waved-version:0.9.3\0....waved-version:0.9.3\0";
        assert_eq!(find_marker(same), Some(v("0.9.3")));
        // A prefix with no NUL nearby is skipped, not read to the end.
        let unterminated = b"waved-version:0.9.3 and much more text that never ends with a NUL";
        assert_eq!(find_marker(unterminated), None);
    }
}
