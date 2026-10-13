//! Self-hosted, signed updates for waved (workstream H, `docs/UPDATES.md`).
//!
//! - `format`: the `.wup` package (signed header, gzip files) and its checks.
//! - `index`: `latest.json`, the server's pointer to the newest package.
//! - `version`, `key`: version ordering and the compiled-in signing key.
//! - `trial`: the first starts of a new version and the fall-back to the old one.
//! - `install`: the `Installer` trait, the file-system installer and a fake.
//! - `gate`: when installing may start, and when a new version is healthy.
//! - `fetch`, `service`: the HTTP fetcher and `UpdateService`, which publishes `OtaState`.
//! - `device`: `spawn_device`, what `main.rs` calls.
//! - `elf`, `release`: the checks `waved-pack` runs before it signs.
//! - `paths`, `space`: where everything lives on the Wave, and the room it takes.

pub mod device;
pub mod elf;
pub mod fetch;
pub mod format;
pub mod gate;
pub mod index;
pub mod install;
#[cfg(test)]
mod install_script;
pub mod key;
pub mod paths;
pub mod release;
pub mod service;
pub mod space;
pub mod trial;
pub mod version;
