//! Starting the service on the Wave (`main.rs` calls `spawn_device`).

use std::sync::Arc;

use ed25519_dalek::VerifyingKey;
use tokio::task::JoinHandle;
use tracing::warn;
use waved_state::StateBus;

use crate::fetch::{FakeFetcher, HttpFetcher};
use crate::install::FsInstaller;
use crate::paths::Paths;
use crate::service::{Inputs, Settings, UpdateService};
use crate::{key, version};

/// The build id in About's version line: `"0.1.0 (374015f)"` gives `"374015f"`.
pub fn build_id(about_version: &str) -> &str {
    about_version
        .split_once(" (")
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or("")
}

/// The update service on the device's paths with the compiled-in key.
/// `server` and `allow_downgrade` are `security.ota_server` and
/// `security.ota_allow_downgrade`; `about_version` is
/// `waved_system::about::version()`.
pub fn spawn_device(
    bus: &StateBus,
    server: &str,
    allow_downgrade: bool,
    about_version: &str,
) -> JoinHandle<()> {
    spawn_at(
        bus,
        Paths::device(),
        key::trusted_key(),
        Settings::new(
            server,
            allow_downgrade,
            version::running(),
            build_id(about_version),
        ),
    )
}

/// `spawn_device` with the paths, key and settings given (tests).
pub fn spawn_at(
    bus: &StateBus,
    paths: Paths,
    trusted: Option<VerifyingKey>,
    settings: Settings,
) -> JoinHandle<()> {
    let inputs = Inputs::from_bus(bus);
    let installer = FsInstaller::new(paths.clone());
    let state = Arc::clone(&bus.ota);
    match HttpFetcher::new() {
        Ok(fetcher) => tokio::spawn(
            UpdateService::new(settings, trusted, paths, fetcher, installer, state).run(inputs),
        ),
        Err(e) => {
            // Without HTTP there is nothing to check, but a version on trial
            // must still commit: run with checks off.
            warn!("updates are off: no HTTP client ({e})");
            tokio::spawn(
                UpdateService::new(
                    settings,
                    None,
                    paths,
                    FakeFetcher::default(),
                    installer,
                    state,
                )
                .run(inputs),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Target;
    use crate::trial::{self, Trial};
    use crate::version::Version;
    use std::time::Duration;
    use waved_state::OtaPhase;

    #[test]
    fn the_build_id_comes_from_about() {
        assert_eq!(build_id("0.1.0 (374015f)"), "374015f");
        assert_eq!(build_id("0.1.0"), "");
        assert_eq!(build_id(""), "");
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_key_updates_are_off_but_a_trial_still_commits() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(paths.target(Target::Waved).parent().unwrap()).unwrap();
        let running = version::running();
        trial::begin(
            &paths,
            &Trial {
                version: running,
                build: String::new(),
                previous: Version::new(0, 0, 1),
                restore: vec![],
                remove: vec![],
            },
        )
        .unwrap();
        let bus = StateBus::new();
        let settings = Settings::new("http://updates.lan/wave", false, running, "");
        spawn_at(&bus, paths.clone(), None, settings);
        tokio::time::sleep(Duration::from_secs(61)).await;
        assert_eq!(bus.ota.borrow().phase, OtaPhase::Off);
        assert_eq!(bus.ota.borrow().current_version, running.to_string());
        assert!(trial::pending(&paths).is_none());
    }
}
