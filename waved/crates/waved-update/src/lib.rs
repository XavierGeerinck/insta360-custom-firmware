//! Self-hosted, signed updates for waved (workstream H, `docs/UPDATES.md`).
//!
//! - `format`: the `.wup` package (signed header, gzip files) and its checks.
//! - `index`: `latest.json`, the server's pointer to the newest package.
//! - `version`, `key`: version ordering and the compiled-in signing key.
//! - `trial`: the first starts of a new version and the fall-back to the old one.
//! - `install`: the `Installer` trait, the file-system installer and a fake.
//! - `gate`, `health`: when installing may start, when a new version is healthy.
//! - `fetch`, `service`: the HTTP fetcher and `UpdateService`, which publishes `OtaState`.

pub mod format;
pub mod index;
pub mod key;
pub mod paths;
pub mod trial;
pub mod version;
