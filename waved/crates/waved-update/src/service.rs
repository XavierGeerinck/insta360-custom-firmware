//! `UpdateService` (H2): checks `security.ota_server`, downloads, verifies,
//! waits until nothing is in use, installs, reboots, and commits a new
//! version once it is healthy. Publishes `OtaState`.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::time::{sleep, sleep_until, Instant};
use tracing::{info, warn};
use waved_state::{
    AudioState, BatteryState, Command, McuState, MeetingState, OtaPhase, OtaState, PlaybackState,
    ProvisioningState, StateBus,
};

use crate::fetch::{FetchError, Fetcher};
use crate::format::{self, FormatError, Staged};
use crate::gate::{self, Activity};
use crate::index::{self, Index};
use crate::install::Installer;
use crate::paths::Paths;
use crate::space::{self, SpacePlan};
use crate::trial::{self, StartCheck};
use crate::version::{judge, Version};

/// What the user reads when an update fails (`OtaPhase::Failed`).
pub mod copy {
    pub const CANT_REACH: &str = "Can't reach the update server";
    pub const BAD_INDEX: &str = "The update server's list is broken";
    pub const DOWNLOAD_STOPPED: &str = "The download stopped";
    pub const DAMAGED: &str = "The download was damaged";
    pub const NOT_SIGNED: &str = "The update isn't signed for this Wave";
    pub const BROKEN: &str = "The update package is broken";
    pub const MISMATCH: &str = "The update doesn't match the server's list";
    pub const OLDER: &str = "The update is older than this version";
    pub const NO_SPACE: &str = "Not enough space on Wave";
    pub const INSTALL_FAILED: &str = "The update could not be installed";
    pub const RESTART_FAILED: &str = "Restart Wave to finish the update";
    pub const INTERRUPTED: &str = "The update stopped before it finished";

    pub fn rolled_back(version: &str) -> String {
        format!("Version {version} didn't start, so Wave went back")
    }
}

/// While waiting for Wave to be free, look again this often.
const WAIT_POLL: Duration = Duration::from_secs(5);
/// A trial that is not healthy yet is checked again this often.
const HEALTH_RETRY: Duration = Duration::from_secs(30);
/// Long enough for the screen to show "Restarting".
const RESTART_PAUSE: Duration = Duration::from_secs(1);

/// The first automatic check waits for start-up (and Wi-Fi) to settle.
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(2 * 60);
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

pub struct Settings {
    /// `security.ota_server`; empty turns checks off.
    pub server: String,
    /// `security.ota_allow_downgrade`.
    pub allow_downgrade: bool,
    pub running: Version,
    /// Git build id of this binary; may be empty.
    pub running_build: String,
    pub first_check_after: Duration,
    pub check_every: Duration,
}

impl Settings {
    pub fn new(server: &str, allow_downgrade: bool, running: Version, running_build: &str) -> Self {
        Self {
            server: server.trim().to_string(),
            allow_downgrade,
            running,
            running_build: running_build.to_string(),
            first_check_after: FIRST_CHECK_AFTER,
            check_every: CHECK_EVERY,
        }
    }
}

/// What the service listens to.
pub struct Inputs {
    pub commands: broadcast::Receiver<Command>,
    pub meeting: watch::Receiver<MeetingState>,
    pub audio: watch::Receiver<AudioState>,
    pub playback: watch::Receiver<PlaybackState>,
    pub provisioning: watch::Receiver<ProvisioningState>,
    pub battery: watch::Receiver<BatteryState>,
    pub mcu: watch::Receiver<McuState>,
}

impl Inputs {
    pub fn from_bus(bus: &StateBus) -> Self {
        Self {
            commands: bus.commands.subscribe(),
            meeting: bus.meeting.subscribe(),
            audio: bus.audio.subscribe(),
            playback: bus.playback.subscribe(),
            provisioning: bus.provisioning.subscribe(),
            battery: bus.battery.subscribe(),
            mcu: bus.mcu.subscribe(),
        }
    }
}

pub struct UpdateService<F, I> {
    settings: Settings,
    key: Option<VerifyingKey>,
    paths: Paths,
    fetcher: F,
    installer: Arc<I>,
    state: Arc<watch::Sender<OtaState>>,
    free_space: fn(&Path) -> io::Result<u64>,
}

impl<F: Fetcher, I: Installer + 'static> UpdateService<F, I> {
    pub fn new(
        settings: Settings,
        key: Option<VerifyingKey>,
        paths: Paths,
        fetcher: F,
        installer: I,
        state: Arc<watch::Sender<OtaState>>,
    ) -> Self {
        Self {
            settings,
            key,
            paths,
            fetcher,
            installer: Arc::new(installer),
            state,
            free_space: crate::space::free_bytes,
        }
    }

    /// Replace the free-space probe (tests).
    pub fn with_free_space(mut self, probe: fn(&Path) -> io::Result<u64>) -> Self {
        self.free_space = probe;
        self
    }

    fn enabled(&self) -> bool {
        self.key.is_some() && index::index_url(&self.settings.server).is_ok()
    }

    fn publish(&self, phase: OtaPhase, progress_pct: u8) {
        self.state.send_if_modified(|ota| {
            let changed = ota.phase != phase || ota.progress_pct != progress_pct;
            ota.phase = phase;
            ota.progress_pct = progress_pct;
            changed
        });
    }

    fn set_target(&self, target: Option<String>) {
        self.state.send_if_modified(|ota| {
            let changed = ota.target_version != target;
            ota.target_version = target;
            changed
        });
    }

    fn fail(&self, reason: impl Into<String>) {
        let reason = reason.into();
        warn!("update failed: {reason}");
        self.publish(OtaPhase::Failed { reason }, 0);
    }

    pub async fn run(mut self, mut inputs: Inputs) {
        let started = Instant::now();
        let running = self.settings.running;
        let enabled = self.enabled();
        self.state.send_modify(|ota| {
            *ota = OtaState {
                phase: if enabled {
                    OtaPhase::Idle
                } else {
                    OtaPhase::Off
                },
                current_version: running.to_string(),
                target_version: None,
                progress_pct: 0,
            }
        });
        if !enabled {
            if let Some(problem) = crate::key::compiled_key_problem() {
                warn!(
                    "updates are off: {} is unusable ({problem})",
                    crate::key::KEY_ENV
                );
            } else if self.key.is_none() {
                info!("updates are off: this build trusts no signing key");
            } else {
                info!("updates are off: no usable security.ota_server");
            }
        }

        // The trial bookkeeping runs even with updates off: a version on
        // trial must commit, or S60waved takes it back.
        crate::install::clear_leftovers(&self.paths);
        let mut health_at = None;
        match trial::check_at_start(&self.paths, running) {
            Ok(StartCheck::Normal) => {}
            Ok(StartCheck::OnTrial(on_trial)) => {
                info!(
                    "version {} is on trial; it commits once healthy",
                    on_trial.version
                );
                health_at = Some(started + gate::HEALTHY_AFTER);
            }
            Ok(StartCheck::Interrupted(cut)) => {
                self.set_target(Some(cut.version.to_string()));
                self.fail(copy::INTERRUPTED);
            }
            Ok(StartCheck::RolledBack(failed)) => {
                if let Err(e) = trial::skip(&self.paths, failed.version, &failed.build) {
                    warn!("could not record the failed version: {e}");
                }
                self.set_target(Some(failed.version.to_string()));
                self.fail(copy::rolled_back(&failed.version.to_string()));
            }
            Err(e) => warn!("could not read the update trial: {e}"),
        }

        let mut next_check = started + self.settings.first_check_after;
        loop {
            let health = async {
                match health_at {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            // No new install while this version is on trial.
            let free = enabled && health_at.is_none();
            tokio::select! {
                _ = health => {
                    let capture = inputs.audio.borrow().capture_healthy;
                    if gate::healthy(started.elapsed(), capture) {
                        match trial::commit(&self.paths) {
                            Ok(()) => info!("version {running} is healthy; update committed"),
                            Err(e) => warn!("could not commit the update: {e}"),
                        }
                        health_at = None;
                    } else {
                        health_at = Some(Instant::now() + HEALTH_RETRY);
                    }
                }
                _ = sleep_until(next_check), if free => {
                    next_check = Instant::now() + self.settings.check_every;
                    self.cycle(Mode::Auto, &mut inputs).await;
                }
                command = inputs.commands.recv() => match command {
                    Ok(Command::CheckForUpdate) if free => self.cycle(Mode::Check, &mut inputs).await,
                    Ok(Command::InstallUpdate) if free => self.cycle(Mode::Manual, &mut inputs).await,
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
            }
        }
    }

    async fn cycle(&mut self, mode: Mode, inputs: &mut Inputs) {
        let before = self.state.borrow().clone();
        match self.try_cycle(mode, inputs).await {
            Ok(()) => {}
            Err(Failure::Quiet(why)) => {
                info!("automatic update check: {why}");
                self.publish(before.phase, before.progress_pct);
            }
            Err(Failure::Shown(reason)) => self.fail(reason),
        }
    }

    async fn try_cycle(&mut self, mode: Mode, inputs: &mut Inputs) -> Result<(), Failure> {
        let running = self.settings.running;
        let key = self.key.expect("cycles run only with a key");
        let quiet_or = |reason: &'static str, why: String| {
            if mode == Mode::Auto {
                Failure::Quiet(why)
            } else {
                Failure::Shown(reason)
            }
        };

        self.publish(OtaPhase::Checking, 0);
        let index_url = index::index_url(&self.settings.server)
            .map_err(|e| quiet_or(copy::CANT_REACH, e.to_string()))?;
        let body = self
            .fetcher
            .get(&index_url, index::MAX_INDEX_BYTES)
            .await
            .map_err(|e| quiet_or(copy::CANT_REACH, e.to_string()))?;
        let (index, offered) =
            Index::parse(&body).map_err(|e| quiet_or(copy::BAD_INDEX, e.to_string()))?;
        let verdict = judge(
            running,
            &self.settings.running_build,
            offered,
            &index.build,
            self.settings.allow_downgrade,
        );
        if !verdict.installs() {
            info!("update check: {offered} offered, {running} running: {verdict:?}");
            self.set_target(None);
            self.publish(OtaPhase::UpToDate, 0);
            return Ok(());
        }
        self.set_target(Some(offered.to_string()));
        let skipped = trial::is_skipped(&self.paths, offered, &index.build);
        if mode == Mode::Check || (mode == Mode::Auto && skipped) {
            self.publish(OtaPhase::Available, 0);
            return Ok(());
        }
        if skipped {
            let _ = trial::unskip(&self.paths, offered, &index.build);
        }

        let package_url = index::package_url(&index_url, &index)
            .map_err(|e| quiet_or(copy::BAD_INDEX, e.to_string()))?;
        self.download(&package_url, &index).await?;
        let staged = self.verify(key, &index, offered).await?;
        self.wait_until_free(mode, inputs).await?;
        self.install(staged).await
    }

    async fn download(&self, url: &reqwest::Url, index: &Index) -> Result<(), Failure> {
        let dir = self.paths.download_dir();
        let dest = self.paths.download();
        let sidecar = dir.join("package.sha256");
        fs::create_dir_all(&dir).map_err(|_| Failure::Shown(copy::NO_SPACE))?;
        // Never append to part of some other package.
        if fs::read_to_string(&sidecar).ok().as_deref() != Some(index.sha256.as_str()) {
            let _ = fs::remove_file(&dest);
            fs::write(&sidecar, &index.sha256).map_err(|_| Failure::Shown(copy::NO_SPACE))?;
        }
        let have = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        let free = (self.free_space)(&dir).unwrap_or(0);
        if let SpacePlan::TooBig { .. } = space::plan(free, index.size.saturating_sub(have), &[]) {
            return Err(Failure::Shown(copy::NO_SPACE));
        }

        let size = index.size.max(1);
        let pct = move |bytes: u64| (bytes.min(size) * 90 / size) as u8;
        self.publish(OtaPhase::Downloading, pct(have));
        let state = Arc::clone(&self.state);
        let mut progress = move |bytes: u64| {
            let pct = pct(bytes);
            state.send_if_modified(|ota| {
                let changed = ota.progress_pct != pct;
                ota.progress_pct = pct;
                changed
            });
        };
        match self
            .fetcher
            .download(url, &dest, index.size, &mut progress)
            .await
        {
            Ok(()) => {}
            Err(FetchError::TooLarge) => {
                remove_download(&self.paths);
                return Err(Failure::Shown(copy::DAMAGED));
            }
            Err(e) => {
                warn!("update download stopped: {e}");
                return Err(Failure::Shown(copy::DOWNLOAD_STOPPED));
            }
        }

        let whole = dest.clone();
        let hash = tokio::task::spawn_blocking(move || file_sha256(&whole))
            .await
            .ok()
            .and_then(Result::ok);
        if hash.as_deref() != Some(index.sha256.as_str()) {
            remove_download(&self.paths);
            return Err(Failure::Shown(copy::DAMAGED));
        }
        Ok(())
    }

    async fn verify(
        &self,
        key: VerifyingKey,
        index: &Index,
        offered: Version,
    ) -> Result<Staged, Failure> {
        self.publish(OtaPhase::Verifying, 90);
        let download = self.paths.download();
        let reject = |paths: &Paths, reason: &'static str| {
            remove_download(paths);
            let _ = trial::skip(paths, offered, &index.build);
            Failure::Shown(reason)
        };
        let header = match format::read_header_at(&download, &key) {
            Ok(header) => header,
            // A local read error says nothing about the package: no skip.
            Err(FormatError::Io(e)) => {
                warn!("could not read the downloaded update: {e}");
                remove_download(&self.paths);
                return Err(Failure::Shown(copy::DAMAGED));
            }
            Err(e) => return Err(reject(&self.paths, rejection(&e))),
        };
        if header.version != offered || header.manifest.build != index.build {
            return Err(reject(&self.paths, copy::MISMATCH));
        }
        let verdict = judge(
            self.settings.running,
            &self.settings.running_build,
            header.version,
            &header.manifest.build,
            self.settings.allow_downgrade,
        );
        if !verdict.installs() {
            return Err(reject(&self.paths, copy::OLDER));
        }

        // Room on /data for the unpacked files, deleting last-known-good
        // copies if that is what it takes.
        let update_dir = self.paths.update_dir();
        fs::create_dir_all(&update_dir).map_err(|_| Failure::Shown(copy::NO_SPACE))?;
        let free = (self.free_space)(&update_dir).unwrap_or(0);
        let need = header.manifest.installed_size();
        match space::plan(free, need, &space::reclaimable(&self.paths)) {
            SpacePlan::Fits => {}
            SpacePlan::FitsAfterDeleting(files) => {
                for file in files {
                    info!("removing {} to make room for the update", file.display());
                    let _ = fs::remove_file(file);
                }
            }
            SpacePlan::TooBig { need, free } => {
                warn!("update needs {need} bytes on /data, {free} free");
                return Err(Failure::Shown(copy::NO_SPACE));
            }
        }

        let staging = self.paths.staging();
        let state = Arc::clone(&self.state);
        let staged = tokio::task::spawn_blocking(move || {
            format::verify_and_stage(&download, &key, &staging, &mut |done, total| {
                let pct = 90 + (done.min(total) * 8 / total.max(1)) as u8;
                state.send_if_modified(|ota| {
                    let changed = ota.progress_pct != pct;
                    ota.progress_pct = pct;
                    changed
                });
            })
        })
        .await
        .map_err(|_| Failure::Shown(copy::INSTALL_FAILED))?;
        match staged {
            Ok(staged) => {
                remove_download(&self.paths);
                Ok(staged)
            }
            Err(FormatError::Io(e)) => {
                warn!("could not unpack the update: {e}");
                Err(Failure::Shown(copy::NO_SPACE))
            }
            Err(e) => Err(reject(&self.paths, rejection(&e))),
        }
    }

    async fn wait_until_free(&self, mode: Mode, inputs: &mut Inputs) -> Result<(), Failure> {
        let mut manual = mode == Mode::Manual;
        loop {
            let activity = Activity::read(
                &inputs.meeting.borrow(),
                &inputs.audio.borrow(),
                &inputs.playback.borrow(),
                &inputs.provisioning.borrow(),
                &inputs.battery.borrow(),
                &inputs.mcu.borrow(),
            );
            let Some(blocker) = gate::blocker(&activity, manual) else {
                return Ok(());
            };
            if self.state.borrow().phase != OtaPhase::Waiting {
                info!("update ready; waiting ({blocker:?})");
            }
            self.publish(OtaPhase::Waiting, 98);
            tokio::select! {
                _ = sleep(WAIT_POLL) => {}
                command = inputs.commands.recv() => match command {
                    Ok(Command::InstallUpdate) => manual = true,
                    Err(RecvError::Closed) => return Err(Failure::Quiet("shutting down".into())),
                    _ => {}
                },
            }
        }
    }

    async fn install(&self, staged: Staged) -> Result<(), Failure> {
        let running = self.settings.running;
        self.publish(OtaPhase::Installing, 99);
        info!("installing waved {} over {running}", staged.version);
        let installer = Arc::clone(&self.installer);
        let staging = staged.dir.clone();
        let installed =
            tokio::task::spawn_blocking(move || installer.install(&staged, running)).await;
        if !matches!(installed, Ok(Ok(()))) {
            if let Ok(Err(e)) = &installed {
                warn!("install failed: {e}");
            }
            // The installer never leaves the binary swapped on an error, so
            // this puts any replaced deploy files back.
            let _ = trial::check_at_start(&self.paths, running);
            let _ = fs::remove_dir_all(&staging);
            return Err(Failure::Shown(copy::INSTALL_FAILED));
        }
        self.publish(OtaPhase::Restarting, 100);
        sleep(RESTART_PAUSE).await;
        let installer = Arc::clone(&self.installer);
        match tokio::task::spawn_blocking(move || installer.reboot()).await {
            Ok(Ok(())) => Ok(()),
            _ => Err(Failure::Shown(copy::RESTART_FAILED)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The timer: installs when Wave is free, quiet about network trouble.
    Auto,
    /// "Check now": reports, downloads nothing.
    Check,
    /// Install / Retry: also tries a skipped version, ignores a listening computer.
    Manual,
}

enum Failure {
    /// Logged; the state goes back to what it was.
    Quiet(String),
    Shown(&'static str),
}

fn rejection(error: &FormatError) -> &'static str {
    match error {
        FormatError::BadSignature => copy::NOT_SIGNED,
        FormatError::Io(_) => copy::DAMAGED,
        _ => copy::BROKEN,
    }
}

fn remove_download(paths: &Paths) {
    let _ = fs::remove_file(paths.download());
    let _ = fs::remove_file(paths.download_dir().join("package.sha256"));
}

fn file_sha256(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::FakeFetcher;
    use crate::format::{self, Target};
    use crate::install::{Installed, RecordingInstaller};
    use crate::key;
    use crate::trial::{self, Trial};
    use ed25519_dalek::SigningKey;
    use std::fs;
    use waved_state::OtaPhase;

    const SERVER: &str = "http://updates.lan/wave";
    const INDEX: &str = "http://updates.lan/wave/latest.json";
    const NEVER: Duration = Duration::from_secs(100 * 24 * 60 * 60);

    fn v(text: &str) -> Version {
        text.parse().unwrap()
    }

    fn package_url(version: &str) -> String {
        format!("{SERVER}/waved-{version}.wup")
    }

    struct Harness {
        _dir: tempfile::TempDir,
        paths: Paths,
        key: SigningKey,
        bus: StateBus,
        fetcher: FakeFetcher,
        installer: RecordingInstaller,
    }

    impl Harness {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::under(dir.path());
            fs::create_dir_all(paths.waved_dir()).unwrap();
            Self {
                _dir: dir,
                paths,
                key: key::generate().unwrap(),
                bus: StateBus::new(),
                fetcher: FakeFetcher::default(),
                installer: RecordingInstaller::default(),
            }
        }

        fn package(&self, version: &str, build: &str, key: &SigningKey) -> Vec<u8> {
            let binary: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 253) as u8).collect();
            format::pack(&[(Target::Waved, binary)], v(version), build, 0, key)
                .unwrap()
                .1
        }

        /// Serve `package` under an index that announces `version`/`build`.
        fn serve(&self, version: &str, build: &str, package: Vec<u8>, sha_of: &[u8]) {
            let index = crate::index::Index {
                format: 1,
                version: version.into(),
                build: build.into(),
                package: format!("waved-{version}.wup"),
                size: package.len() as u64,
                sha256: format::sha256_hex(sha_of),
                installed_size: 300_000,
            };
            self.fetcher.serve(INDEX, index.to_json().into_bytes());
            self.fetcher.serve(&package_url(version), package);
        }

        fn publish(&self, version: &str) {
            let package = self.package(version, "b2", &self.key);
            self.serve(version, "b2", package.clone(), &package);
        }

        fn settings(&self, running: &str) -> Settings {
            let mut settings = Settings::new(SERVER, false, v(running), "b1");
            settings.first_check_after = NEVER;
            settings
        }

        fn spawn(&self, settings: Settings) -> tokio::task::JoinHandle<()> {
            self.spawn_with(
                settings,
                Some(self.key.verifying_key()),
                crate::space::free_bytes,
            )
        }

        fn spawn_with(
            &self,
            settings: Settings,
            key: Option<VerifyingKey>,
            free_space: fn(&Path) -> io::Result<u64>,
        ) -> tokio::task::JoinHandle<()> {
            let service = UpdateService::new(
                settings,
                key,
                self.paths.clone(),
                self.fetcher.clone(),
                self.installer.clone(),
                Arc::clone(&self.bus.ota),
            )
            .with_free_space(free_space);
            tokio::spawn(service.run(Inputs::from_bus(&self.bus)))
        }

        fn send(&self, command: Command) {
            self.bus.commands.send(command).unwrap();
        }

        fn ota(&self) -> OtaState {
            self.bus.ota.borrow().clone()
        }

        /// Let the service run (simulated time) until `done` holds.
        async fn until(&self, what: &str, done: impl Fn(&OtaState) -> bool) -> OtaState {
            for _ in 0..20_000 {
                let state = self.ota();
                if done(&state) {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("never reached {what}: {:?}", self.ota());
        }

        fn installs(&self) -> Vec<Installed> {
            self.installer.calls()
        }

        /// Restarting shows for a moment before the reboot.
        async fn until_rebooted(&self) -> OtaState {
            let state = self
                .until("restarting", |s| s.phase == OtaPhase::Restarting)
                .await;
            for _ in 0..100 {
                if self.installs().contains(&Installed::Reboot) {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("no reboot: {:?}", self.installs());
        }
    }

    fn failed(state: &OtaState) -> bool {
        matches!(state.phase, OtaPhase::Failed { .. })
    }

    fn reason(state: &OtaState) -> String {
        match &state.phase {
            OtaPhase::Failed { reason } => reason.clone(),
            other => panic!("not failed: {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn off_without_a_server_or_a_key() {
        let h = Harness::new();
        h.publish("0.10.0");
        let mut settings = h.settings("0.9.3");
        settings.server.clear();
        h.spawn(settings);
        h.send(Command::InstallUpdate);
        let state = h.until("off", |s| !s.current_version.is_empty()).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(h.ota().phase, OtaPhase::Off);
        assert_eq!(state.current_version, "0.9.3");

        let h = Harness::new();
        h.publish("0.10.0");
        h.spawn_with(h.settings("0.9.3"), None, crate::space::free_bytes);
        h.send(Command::InstallUpdate);
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(h.ota().phase, OtaPhase::Off);
        assert!(h.fetcher.gets().is_empty(), "nothing is fetched");
    }

    #[tokio::test(start_paused = true)]
    async fn the_same_version_is_up_to_date() {
        let h = Harness::new();
        h.publish("0.9.3");
        h.spawn(h.settings("0.9.3"));
        h.until("idle", |s| s.phase == OtaPhase::Idle).await;
        h.send(Command::CheckForUpdate);
        h.until("up to date", |s| s.phase == OtaPhase::UpToDate)
            .await;
        assert!(h.fetcher.downloads().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_finds_a_newer_version_without_downloading_it() {
        let h = Harness::new();
        h.publish("0.10.0");
        h.spawn(h.settings("0.9.3"));
        h.send(Command::CheckForUpdate);
        let state = h
            .until("available", |s| s.phase == OtaPhase::Available)
            .await;
        assert_eq!(state.target_version.as_deref(), Some("0.10.0"));
        assert!(h.fetcher.downloads().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn install_downloads_verifies_installs_and_restarts() {
        let h = Harness::new();
        h.publish("0.10.0");
        h.spawn(h.settings("0.9.3"));
        h.send(Command::InstallUpdate);
        let state = h.until_rebooted().await;
        assert_eq!(state.progress_pct, 100);
        assert_eq!(state.target_version.as_deref(), Some("0.10.0"));
        assert_eq!(
            h.installs(),
            vec![
                Installed::Install {
                    version: v("0.10.0"),
                    from: v("0.9.3")
                },
                Installed::Reboot
            ]
        );
        assert!(
            !h.paths.download().exists(),
            "the download is removed once staged"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_automatic_check_installs_by_itself_when_idle() {
        let h = Harness::new();
        h.publish("0.10.0");
        let mut settings = h.settings("0.9.3");
        settings.first_check_after = Duration::from_secs(120);
        h.spawn(settings);
        h.until_rebooted().await;
        assert_eq!(h.installs().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn an_interrupted_download_resumes_where_it_stopped() {
        let h = Harness::new();
        h.publish("0.10.0");
        h.fetcher.cut_next_download_at(1_000);
        h.spawn(h.settings("0.9.3"));
        h.send(Command::InstallUpdate);
        let state = h.until("failed", failed).await;
        assert_eq!(reason(&state), "The download stopped");
        assert_eq!(fs::metadata(h.paths.download()).unwrap().len(), 1_000);
        assert!(h.installs().is_empty());

        h.send(Command::InstallUpdate);
        h.until_rebooted().await;
        let url = package_url("0.10.0");
        assert_eq!(h.fetcher.downloads(), vec![(url.clone(), 0), (url, 1_000)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_damaged_download_is_never_installed() {
        let h = Harness::new();
        let good = h.package("0.10.0", "b2", &h.key);
        let mut bad = good.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        h.serve("0.10.0", "b2", bad, &good);
        h.spawn(h.settings("0.9.3"));
        h.send(Command::InstallUpdate);
        let state = h.until("failed", failed).await;
        assert_eq!(reason(&state), "The download was damaged");
        assert!(h.installs().is_empty());
        assert!(
            !h.paths.download().exists(),
            "a damaged download starts over"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_package_signed_with_another_key_is_refused_and_not_fetched_again_automatically() {
        let h = Harness::new();
        let other = key::generate().unwrap();
        let package = h.package("0.10.0", "b2", &other);
        h.serve("0.10.0", "b2", package.clone(), &package);
        let mut settings = h.settings("0.9.3");
        settings.first_check_after = Duration::from_secs(60);
        settings.check_every = Duration::from_secs(600);
        h.spawn(settings);
        let state = h.until("failed", failed).await;
        assert_eq!(reason(&state), "The update isn't signed for this Wave");
        assert!(h.installs().is_empty());
        tokio::time::sleep(Duration::from_secs(1_300)).await;
        assert_eq!(
            h.fetcher.downloads().len(),
            1,
            "skipped by later automatic checks"
        );
        assert_eq!(h.ota().phase, OtaPhase::Available);
    }

    #[tokio::test(start_paused = true)]
    async fn an_older_version_is_refused_unless_downgrades_are_allowed() {
        let h = Harness::new();
        h.publish("0.9.3");
        h.spawn(h.settings("0.10.0"));
        h.send(Command::InstallUpdate);
        h.until("up to date", |s| s.phase == OtaPhase::UpToDate)
            .await;
        assert!(h.fetcher.downloads().is_empty());

        let h = Harness::new();
        h.publish("0.9.3");
        let mut settings = h.settings("0.10.0");
        settings.allow_downgrade = true;
        h.spawn(settings);
        h.send(Command::InstallUpdate);
        h.until_rebooted().await;
        assert_eq!(
            h.installs()[0],
            Installed::Install {
                version: v("0.9.3"),
                from: v("0.10.0")
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_signed_version_must_be_the_one_the_index_announced() {
        let h = Harness::new();
        // An index that promises 0.11.0 but serves a signed, older 0.10.0
        // package: the signed version decides, and it does not match.
        let package = h.package("0.10.0", "b2", &h.key);
        h.serve("0.11.0", "b2", package.clone(), &package);
        h.spawn(h.settings("0.9.3"));
        h.send(Command::InstallUpdate);
        let state = h.until("failed", failed).await;
        assert_eq!(reason(&state), "The update doesn't match the server's list");
        assert!(h.installs().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn it_waits_for_a_recording_to_end() {
        let h = Harness::new();
        h.publish("0.10.0");
        h.bus.meeting.send_modify(|m| m.recording = true);
        h.spawn(h.settings("0.9.3"));
        h.send(Command::InstallUpdate);
        h.until("waiting", |s| s.phase == OtaPhase::Waiting).await;
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(h.installs().is_empty());
        h.bus.meeting.send_modify(|m| m.recording = false);
        h.until_rebooted().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_rolled_back_version_is_reported_and_only_a_manual_install_retries_it() {
        let h = Harness::new();
        h.publish("0.10.0");
        fs::create_dir_all(h.paths.update_dir()).unwrap();
        let failed_trial = Trial {
            version: v("0.10.0"),
            build: "b2".into(),
            previous: v("0.9.3"),
            restore: vec![],
            remove: vec![],
        };
        fs::write(h.paths.rolled_back(), failed_trial.to_text()).unwrap();
        let mut settings = h.settings("0.9.3");
        settings.first_check_after = Duration::from_secs(60);
        h.spawn(settings);
        let state = h.until("failed", failed).await;
        assert_eq!(
            reason(&state),
            "Version 0.10.0 didn't start, so Wave went back"
        );
        h.until("available", |s| s.phase == OtaPhase::Available)
            .await;
        assert!(h.fetcher.downloads().is_empty(), "no automatic retry");

        h.send(Command::InstallUpdate);
        h.until_rebooted().await;
        assert!(!trial::is_skipped(&h.paths, v("0.10.0"), "b2"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_version_commits_after_a_healthy_minute() {
        let h = Harness::new();
        let on_trial = Trial {
            version: v("0.10.0"),
            build: "b2".into(),
            previous: v("0.9.3"),
            restore: vec![],
            remove: vec![],
        };
        trial::begin(&h.paths, &on_trial).unwrap();
        h.spawn(h.settings("0.10.0"));
        tokio::time::sleep(Duration::from_secs(59)).await;
        assert!(trial::pending(&h.paths).is_some());
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(trial::pending(&h.paths).is_none(), "committed");
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_version_with_dead_capture_does_not_commit() {
        let h = Harness::new();
        let on_trial = Trial {
            version: v("0.10.0"),
            build: String::new(),
            previous: v("0.9.3"),
            restore: vec![],
            remove: vec![],
        };
        trial::begin(&h.paths, &on_trial).unwrap();
        h.bus.audio.send_modify(|a| a.capture_healthy = false);
        h.spawn(h.settings("0.10.0"));
        tokio::time::sleep(Duration::from_secs(300)).await;
        assert!(trial::pending(&h.paths).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn an_install_cut_before_the_swap_is_reported() {
        let h = Harness::new();
        let cut = Trial {
            version: v("0.10.0"),
            build: String::new(),
            previous: v("0.9.3"),
            restore: vec![],
            remove: vec![],
        };
        trial::begin(&h.paths, &cut).unwrap();
        h.spawn(h.settings("0.9.3"));
        let state = h.until("failed", failed).await;
        assert_eq!(reason(&state), "The update stopped before it finished");
        assert!(trial::pending(&h.paths).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_start_deletes_half_written_copies_a_power_cut_left() {
        let h = Harness::new();
        let init = h.paths.target(Target::S60waved);
        fs::create_dir_all(init.parent().unwrap()).unwrap();
        fs::write(&init, "#!/bin/sh\n").unwrap();
        fs::write(Paths::prev(&init), "#!/bin/sh\nold\n").unwrap();
        for target in Target::ALL {
            fs::write(Paths::new_copy(&h.paths.target(target)), "half").unwrap();
        }
        h.spawn(h.settings("0.9.3"));
        h.until("started", |s| s.phase == OtaPhase::Idle).await;
        for target in Target::ALL {
            let new = Paths::new_copy(&h.paths.target(target));
            assert!(!new.exists(), "{}", new.display());
        }
        assert!(Paths::prev(&init).exists(), "the old copy stays");
        assert!(init.exists());
    }

    #[tokio::test(start_paused = true)]
    async fn not_enough_space_fails_before_unpacking() {
        let h = Harness::new();
        h.publish("0.10.0");
        h.spawn_with(h.settings("0.9.3"), Some(h.key.verifying_key()), |_| {
            Ok(1024)
        });
        h.send(Command::InstallUpdate);
        let state = h.until("failed", failed).await;
        assert_eq!(reason(&state), "Not enough space on Wave");
        assert!(h.installs().is_empty());
        assert!(!h.paths.staging().exists());
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreachable_server_fails_a_manual_check_but_not_an_automatic_one() {
        let h = Harness::new();
        h.fetcher.set_unreachable(true);
        let mut settings = h.settings("0.9.3");
        settings.first_check_after = Duration::from_secs(60);
        h.spawn(settings);
        tokio::time::sleep(Duration::from_secs(120)).await;
        assert_eq!(h.fetcher.gets().len(), 1);
        assert_eq!(h.ota().phase, OtaPhase::Idle);
        h.send(Command::CheckForUpdate);
        let state = h.until("failed", failed).await;
        assert_eq!(reason(&state), "Can't reach the update server");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_install_reports_and_installs_nothing() {
        let h = Harness::new();
        h.publish("0.10.0");
        h.installer.fail_next_install();
        h.spawn(h.settings("0.9.3"));
        h.send(Command::InstallUpdate);
        let state = h.until("failed", failed).await;
        assert_eq!(reason(&state), "The update could not be installed");
        assert!(h.installs().is_empty());
        assert_eq!(state.current_version, "0.9.3");
    }
}
