//! Room on `/data` (512 MiB, nearly full; the binary alone is ~63 MB) and
//! `/tmp` before a download or an install.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::format::Target;
use crate::paths::Paths;
use crate::trial;

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
    /// Fits once these files are deleted.
    FitsAfterDeleting(Vec<PathBuf>),
    TooBig {
        need: u64,
        free: u64,
    },
}

/// `need` bytes plus `MARGIN`, out of `free`, deleting `reclaimable` files
/// (path, bytes freed) in order until it fits.
pub fn plan(free: u64, need: u64, reclaimable: &[(PathBuf, u64)]) -> SpacePlan {
    let need = need.saturating_add(MARGIN);
    if free >= need {
        return SpacePlan::Fits;
    }
    let mut free_after = free;
    let mut delete = Vec::new();
    for (path, bytes) in reclaimable {
        if *bytes == 0 {
            continue;
        }
        free_after += bytes;
        delete.push(path.clone());
        if free_after >= need {
            return SpacePlan::FitsAfterDeleting(delete);
        }
    }
    SpacePlan::TooBig { need, free }
}

/// The `.prev` copies of the last update, which can go when the running
/// version is committed. A copy that is a hard link of a live file frees
/// nothing and is not counted. Nothing is reclaimable during a trial.
pub fn reclaimable(paths: &Paths) -> Vec<(PathBuf, u64)> {
    if trial::pending(paths).is_some() {
        return Vec::new();
    }
    Target::ALL
        .iter()
        .filter_map(|target| {
            let prev = Paths::prev(&paths.target(*target));
            let meta = fs::metadata(&prev).ok()?;
            Some((prev, freed_by_deleting(&meta)))
        })
        .collect()
}

#[cfg(unix)]
fn freed_by_deleting(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    if meta.nlink() > 1 {
        0
    } else {
        meta.len()
    }
}

#[cfg(not(unix))]
fn freed_by_deleting(meta: &fs::Metadata) -> u64 {
    meta.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn it_fits_with_the_margin_or_after_deleting_old_copies() {
        let prev = vec![
            (PathBuf::from("/data/waved/S60waved.prev"), 0),
            (PathBuf::from("/data/waved/waved.prev"), 63 * MB),
        ];
        assert_eq!(plan(100 * MB, 63 * MB, &prev), SpacePlan::Fits);
        assert_eq!(
            plan(70 * MB, 63 * MB, &prev),
            SpacePlan::FitsAfterDeleting(vec![PathBuf::from("/data/waved/waved.prev")])
        );
        assert_eq!(
            plan(5 * MB, 63 * MB, &prev),
            SpacePlan::TooBig {
                need: 63 * MB + MARGIN,
                free: 5 * MB
            }
        );
    }

    #[test]
    fn old_copies_count_only_when_deleting_them_frees_space_and_no_trial_runs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        fs::create_dir_all(paths.waved_dir()).unwrap();
        let waved = paths.target(Target::Waved);
        fs::write(&waved, vec![1u8; 1000]).unwrap();
        fs::hard_link(&waved, Paths::prev(&waved)).unwrap();
        let rules = paths.target(Target::IptablesRules);
        fs::write(Paths::prev(&rules), vec![1u8; 300]).unwrap();
        let mut found = reclaimable(&paths);
        found.sort();
        assert_eq!(
            found,
            vec![(Paths::prev(&rules), 300), (Paths::prev(&waved), 0)]
        );
        fs::create_dir_all(paths.update_dir()).unwrap();
        fs::write(paths.trial(), "version 0.10.0\nprevious 0.9.3\n").unwrap();
        assert!(reclaimable(&paths).is_empty());
    }

    #[test]
    fn free_space_is_measurable_here() {
        assert!(free_bytes(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap() > 0);
    }
}
