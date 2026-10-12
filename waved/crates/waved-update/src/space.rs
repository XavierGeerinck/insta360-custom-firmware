//! Room on `/data` (512 MiB, nearly full; the binary alone is ~63 MB) and
//! `/tmp` before a download or an install.

use std::io;
use std::path::Path;

/// Left free after staging, so logs and the meeting database can still grow.
pub const MARGIN: u64 = 16 * 1024 * 1024;

pub fn free_bytes(path: &Path) -> io::Result<u64> {
    let stat = nix::sys::statvfs::statvfs(path).map_err(io::Error::from)?;
    #[allow(clippy::unnecessary_cast)] // the field types differ between targets
    Ok(stat.blocks_available() as u64 * stat.fragment_size() as u64)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpacePlan {
    Fits,
    TooBig { need: u64, free: u64 },
}

/// `need` bytes plus `MARGIN`, out of `free`. Nothing is deleted to make
/// room: the `.prev` copies of the last update are the last known good
/// version, and the next install replaces them anyway.
pub fn plan(free: u64, need: u64) -> SpacePlan {
    let need = need.saturating_add(MARGIN);
    if free >= need {
        SpacePlan::Fits
    } else {
        SpacePlan::TooBig { need, free }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn it_fits_only_with_the_margin() {
        assert_eq!(plan(100 * MB, 63 * MB), SpacePlan::Fits);
        assert_eq!(plan(63 * MB + MARGIN, 63 * MB), SpacePlan::Fits);
        assert_eq!(
            plan(70 * MB, 63 * MB),
            SpacePlan::TooBig {
                need: 63 * MB + MARGIN,
                free: 70 * MB
            }
        );
    }

    #[test]
    fn free_space_is_measurable_here() {
        assert!(free_bytes(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap() > 0);
    }
}
