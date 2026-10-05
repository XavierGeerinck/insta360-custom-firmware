//! Publishes `SystemState`: the time zone offset every minute and on a zone
//! change (TODO E3), and About (E7). Restores the system clock from the RTC at
//! start-up and writes the RTC when network time arrives.

use crate::about::{collect, storage, SERIAL_SOURCES};
use crate::rtc::{restore_time, I2cRtc, Rtc, SystemClock, WallClock, RTC_ADDRESS, RTC_BUS};
use crate::tz::{TimeZone, ZONEINFO_DIR};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{info, warn};
use waved_state::{SettingsState, SystemState, WifiState};

pub struct SystemService {
    system_tx: Arc<watch::Sender<SystemState>>,
    storage_path: PathBuf,
    zoneinfo: PathBuf,
    serial_sources: Vec<PathBuf>,
    clock: Arc<dyn WallClock>,
    rtc: Option<Box<dyn Rtc>>,
}

impl SystemService {
    /// No RTC; the system clock and the device's zoneinfo and serial paths.
    pub fn new(system_tx: Arc<watch::Sender<SystemState>>, storage_path: impl Into<PathBuf>) -> Self {
        Self {
            system_tx,
            storage_path: storage_path.into(),
            zoneinfo: PathBuf::from(ZONEINFO_DIR),
            serial_sources: SERIAL_SOURCES.iter().map(PathBuf::from).collect(),
            clock: Arc::new(SystemClock),
            rtc: None,
        }
    }

    /// As `new`, with the RTC on i2c-3 when it opens.
    pub fn device(system_tx: Arc<watch::Sender<SystemState>>, storage_path: impl Into<PathBuf>) -> Self {
        let rtc = match I2cRtc::open(RTC_BUS, RTC_ADDRESS) {
            Ok(rtc) => Some(Box::new(rtc) as Box<dyn Rtc>),
            Err(error) => {
                warn!("RTC on {RTC_BUS} unavailable, the clock waits for network time: {error}");
                None
            }
        };
        Self::new(system_tx, storage_path).with_rtc(rtc)
    }

    pub fn with_zoneinfo(mut self, dir: impl Into<PathBuf>) -> Self {
        self.zoneinfo = dir.into();
        self
    }

    pub fn with_serial_sources(mut self, sources: Vec<PathBuf>) -> Self {
        self.serial_sources = sources;
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn WallClock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_rtc(mut self, rtc: Option<Box<dyn Rtc>>) -> Self {
        self.rtc = rtc;
        self
    }

    /// Runs until the settings or Wi-Fi sender is dropped.
    pub async fn run(mut self, mut settings: watch::Receiver<SettingsState>, mut wifi: watch::Receiver<WifiState>) {
        self.restore_clock();
        let mut zone_name = settings.borrow_and_update().time_zone.clone();
        let mut zone = self.load_zone(&zone_name);
        let mut synced = wifi.borrow_and_update().clock_synchronized;
        self.system_tx.send_replace(SystemState {
            utc_offset_min: zone.offset_at(self.clock.now_unix_s()) / 60,
            about: collect(&self.serial_sources, &self.storage_path),
        });
        loop {
            let to_next_minute = 60 - self.clock.now_unix_s().rem_euclid(60) as u64;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(to_next_minute)) => self.refresh_storage(),
                changed = settings.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let next = settings.borrow_and_update().time_zone.clone();
                    if next != zone_name {
                        zone = self.load_zone(&next);
                        zone_name = next;
                    }
                }
                changed = wifi.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let now_synced = wifi.borrow_and_update().clock_synchronized;
                    if now_synced && !synced {
                        self.save_clock_to_rtc();
                    }
                    synced = now_synced;
                }
            }
            let offset = zone.offset_at(self.clock.now_unix_s()) / 60;
            self.system_tx.send_if_modified(|state| {
                let changed = state.utc_offset_min != offset;
                state.utc_offset_min = offset;
                changed
            });
        }
    }

    fn restore_clock(&mut self) {
        let Some(rtc) = self.rtc.as_mut() else {
            return;
        };
        match rtc.read() {
            Ok(rtc_now) => match restore_time(self.clock.now_unix_s(), rtc_now) {
                Some(time) => match self.clock.set(time) {
                    Ok(()) => info!("clock restored from the RTC: {time}"),
                    Err(error) => warn!("clock not restored from the RTC: {error}"),
                },
                None if rtc_now.is_none() => info!("the RTC holds no valid time yet; waiting for network time"),
                None => {}
            },
            Err(error) => warn!("RTC read failed, the clock waits for network time: {error}"),
        }
    }

    fn save_clock_to_rtc(&mut self) {
        let Some(rtc) = self.rtc.as_mut() else {
            return;
        };
        match rtc.write(self.clock.now_unix_s()) {
            Ok(()) => info!("RTC set from network time"),
            Err(error) => warn!("RTC not set from network time: {error}"),
        }
    }

    fn load_zone(&self, name: &str) -> TimeZone {
        TimeZone::load(&self.zoneinfo, name).unwrap_or_else(|error| {
            warn!("time zone {name}: {error}; the clock shows UTC");
            TimeZone::utc()
        })
    }

    fn refresh_storage(&self) {
        let Some((free, total)) = storage(&self.storage_path) else {
            return;
        };
        self.system_tx.send_if_modified(|state| {
            let about = &mut state.about;
            let changed = (about.storage_free_bytes, about.storage_total_bytes) != (free, total);
            about.storage_free_bytes = free;
            about.storage_total_bytes = total;
            changed
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tz::fixture::tzif;
    use std::io;
    use std::sync::Mutex;

    const JULY_2026: i64 = 1_784_116_800;
    const OCT_2026: i64 = 1_791_210_727;

    struct FakeClock {
        state: Mutex<(i64, tokio::time::Instant)>,
        sets: Mutex<Vec<i64>>,
    }

    impl FakeClock {
        fn at(unix_s: i64) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new((unix_s, tokio::time::Instant::now())),
                sets: Mutex::new(Vec::new()),
            })
        }
    }

    impl WallClock for FakeClock {
        fn now_unix_s(&self) -> i64 {
            let (base, since) = *self.state.lock().unwrap();
            base + since.elapsed().as_secs() as i64
        }
        fn set(&self, unix_s: i64) -> io::Result<()> {
            *self.state.lock().unwrap() = (unix_s, tokio::time::Instant::now());
            self.sets.lock().unwrap().push(unix_s);
            Ok(())
        }
    }

    struct FakeRtc {
        read: Result<Option<i64>, ()>,
        writes: Arc<Mutex<Vec<i64>>>,
    }

    impl Rtc for FakeRtc {
        fn read(&mut self) -> io::Result<Option<i64>> {
            self.read.map_err(|()| io::Error::other("bus error"))
        }
        fn write(&mut self, unix_s: i64) -> io::Result<()> {
            self.writes.lock().unwrap().push(unix_s);
            Ok(())
        }
    }

    struct Harness {
        settings: watch::Sender<SettingsState>,
        wifi: watch::Sender<WifiState>,
        system: watch::Receiver<SystemState>,
        _zoneinfo: tempfile::TempDir,
    }

    fn zoneinfo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, offset, footer) in [
            ("Europe/Brussels", 3_600, "CET-1CEST,M3.5.0,M10.5.0/3"),
            ("America/New_York", -18_000, "EST5EDT,M3.2.0,M11.1.0"),
        ] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, tzif(&[], &[(offset, false)], footer)).unwrap();
        }
        dir
    }

    fn start(zone: &str, clock: Arc<FakeClock>, rtc: Option<FakeRtc>) -> Harness {
        let dir = zoneinfo();
        let (settings, settings_rx) = watch::channel(SettingsState {
            time_zone: zone.into(),
            ..SettingsState::default()
        });
        let (wifi, wifi_rx) = watch::channel(WifiState::default());
        let system_tx = Arc::new(watch::channel(SystemState::default()).0);
        let system = system_tx.subscribe();
        let service = SystemService::new(system_tx, dir.path())
            .with_zoneinfo(dir.path())
            .with_serial_sources(Vec::new())
            .with_clock(clock)
            .with_rtc(rtc.map(|rtc| Box::new(rtc) as Box<dyn Rtc>));
        tokio::spawn(service.run(settings_rx, wifi_rx));
        Harness {
            settings,
            wifi,
            system,
            _zoneinfo: dir,
        }
    }

    async fn offset_becomes(system: &mut watch::Receiver<SystemState>, minutes: i32) {
        tokio::time::timeout(Duration::from_secs(120), system.wait_for(|s| s.utc_offset_min == minutes))
            .await
            .unwrap_or_else(|_| panic!("offset never became {minutes}"))
            .expect("system channel open");
    }

    async fn started(system: &mut watch::Receiver<SystemState>) {
        tokio::time::timeout(Duration::from_secs(5), system.wait_for(|s| !s.about.version.is_empty()))
            .await
            .expect("service published")
            .expect("system channel open");
    }

    fn settle() -> tokio::time::Sleep {
        tokio::time::sleep(Duration::from_millis(10))
    }

    #[tokio::test(start_paused = true)]
    async fn publishes_the_zone_offset_and_about_at_start() {
        let mut h = start("Europe/Brussels", FakeClock::at(JULY_2026), None);
        offset_becomes(&mut h.system, 120).await;
        let about = h.system.borrow().about.clone();
        assert!(about.version.starts_with(env!("CARGO_PKG_VERSION")));
        assert!(about.licences.iter().any(|line| line == "LVGL: MIT"));
        assert!(about.storage_total_bytes > 0 && about.storage_free_bytes <= about.storage_total_bytes);
    }

    #[tokio::test(start_paused = true)]
    async fn the_offset_follows_a_zone_change() {
        let mut h = start("Europe/Brussels", FakeClock::at(JULY_2026), None);
        offset_becomes(&mut h.system, 120).await;
        h.settings.send_modify(|s| s.time_zone = "America/New_York".into());
        offset_becomes(&mut h.system, -240).await;
        h.settings.send_modify(|s| s.time_zone = "Mars/Olympus".into());
        offset_becomes(&mut h.system, 0).await; // unknown zone: UTC
    }

    #[tokio::test(start_paused = true)]
    async fn the_offset_changes_at_the_dst_minute() {
        // One minute before 2026-03-29 01:00 UTC.
        let mut h = start("Europe/Brussels", FakeClock::at(1_774_745_940), None);
        offset_becomes(&mut h.system, 60).await;
        offset_becomes(&mut h.system, 120).await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_rtc_sets_an_unset_clock_at_start() {
        let clock = FakeClock::at(0);
        let rtc = FakeRtc {
            read: Ok(Some(OCT_2026)),
            writes: Arc::default(),
        };
        let mut h = start("UTC", Arc::clone(&clock), Some(rtc));
        started(&mut h.system).await;
        assert_eq!(*clock.sets.lock().unwrap(), vec![OCT_2026]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_set_clock_is_left_alone() {
        let clock = FakeClock::at(OCT_2026);
        let rtc = FakeRtc {
            read: Ok(Some(1_700_000_000)),
            writes: Arc::default(),
        };
        let mut h = start("UTC", Arc::clone(&clock), Some(rtc));
        started(&mut h.system).await;
        assert!(clock.sets.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn network_time_is_saved_to_the_rtc_once_per_sync() {
        let writes: Arc<Mutex<Vec<i64>>> = Arc::default();
        let rtc = FakeRtc {
            read: Ok(None),
            writes: Arc::clone(&writes),
        };
        let mut h = start("UTC", FakeClock::at(OCT_2026), Some(rtc));
        started(&mut h.system).await;
        h.wifi.send_modify(|w| w.clock_synchronized = true);
        settle().await;
        h.wifi.send_modify(|w| w.signal_strength = -40);
        settle().await;
        assert_eq!(writes.lock().unwrap().len(), 1);
        assert!(writes.lock().unwrap()[0] >= OCT_2026);
        h.wifi.send_modify(|w| w.clock_synchronized = false);
        settle().await;
        h.wifi.send_modify(|w| w.clock_synchronized = true);
        settle().await;
        assert_eq!(writes.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_rtc_does_not_stop_the_service() {
        let rtc = FakeRtc {
            read: Err(()),
            writes: Arc::default(),
        };
        let mut h = start("Europe/Brussels", FakeClock::at(JULY_2026), Some(rtc));
        offset_becomes(&mut h.system, 120).await;
    }
}
