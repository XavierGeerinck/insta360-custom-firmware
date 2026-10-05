# Display Redesign — Milestone 2, Workstream E: System Services — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the device services behind the redesign in a new crate, `crates/waved-system`:
- display power: dim and off timers as a pure controller, plus the backlight writer;
- the power key reader;
- time zone offsets, the RTC on i2c-3 and the minute refresh;
- recording start and stop tones;
- factory reset;
- About information.

Every piece is host-tested. The integrator wires them in at the milestone 2 runtime switch.

**Architecture:**
- **One crate, no UI.** `waved-system` depends on `waved-state` only. It never touches `waved-ui`. The UI runtime uses its pure `DisplayPower` controller and its `Backlight` writer. `main.rs` spawns its services. Both happen at the runtime switch.
- **Pure logic, thin device edges.** Every device access sits behind a small type with an injected path or trait, so host tests can stand in for it:
  - sysfs backlight: `Backlight::open(dir)`
  - `/dev/input` evdev: `find_power_key(sys, dev)` and `run_decoder(rx, ..)`
  - I2C RTC: the `Rtc` trait
  - the system clock: the `WallClock` trait
  - zoneinfo files: a `zoneinfo` directory
  - playback: the `ToneSink` trait
  - reset paths: `ResetPaths`
- **State out, events in.**
  - `SystemService` publishes a new `SystemState` watch channel (time zone offset, About).
  - The power key sends `DeviceEvent::PowerKey(PowerKeyPress)`.
  - The kit snapshot formats the status clock with the published offset.
  - Factory reset leaves a marker and restarts waved. The erase runs at the next start-up, before anything opens the files.

**Tech Stack:** Rust 2021 (stable 1.94), tokio 1.53 (`watch`, `broadcast`, `test-util`), `libc` 0.2 (`ioctl`, `settimeofday`), `nix` 0.29 (`statvfs`), `thiserror` 2, `tempfile` 3 (tests).

**Spec:**
- `../TODO.md`: E1, E2, E3, E5, E6, E7, decisions D1–D10
- `../docs/mockups/README.md`: interaction model and timings
- `../docs/mockups/index.html`: Resting, System, Display, Date and time, Confirmation
- `docs/DISPLAY_HARDWARE.md`: backlight, LCD
- `docs/AUDIO_HARDWARE.md:670`: `vad_keypad` on `event0` reports only `KEY_POWER`
- the stock firmware RTC driver: `../firmwares/fw_v1.2.2.6/decompiled/libins_entry.so.c`, `rtc_init` / `rtc_gettime` / `rtc_settime` (around line 142 516)
- the planning brief: `../.superpowers/m2-planning/planning-brief.md`

All code paths in this plan are relative to `waved/`. Paths that start with `../` point into the monorepo root.

### TODO coverage

| Item | Covered here | Left to others |
|---|---|---|
| E1 Display | Brightness through `/sys/class/leds/lcd-backlight`. Dim after / off after as the pure controller `DisplayPower`. Never fully off while recording or in a call. The wake guard decision (first touch only wakes). | The Display screen is B (`screens_v2/display.rs`). The Resting screen visuals are A (`ambient.rs`). The runtime wiring is the integrator's. |
| E2 Power key | `KEY_POWER` reader: node discovery, evdev decoding, 2 s hold decoder, `DeviceEvent::PowerKey`. Short press sleeps or wakes, hold 2 s opens the power-off confirmation (controller outcome). | The Confirmation screen and what it sends are B (`confirm.rs`). Mapping events to the router is the integrator's. |
| E3 Time | Time zone offsets from the device's TZif files (including POSIX footers). The 24-hour clock through the snapshot request. The RTC on i2c-3: restore at start-up, write after NTP. A minute refresh of the offset. | The Date and time screen is B (`datetime.rs`). The clock text formatting is the integrator's (contract request CR3). |
| E4 Languages | **Excluded** (D5). | — |
| E5 Alert tones | Rising start and falling stop chimes, synthesised. The tones on/off setting is honoured (`tones_enabled`). Played through a `ToneSink`. | The `ToneSink` adapter over `waved-audio`'s `PlaybackService` and the spawn are the integrator's. Removing legacy `audio_fx` goes with the legacy loop. |
| E6 Factory reset | Erases config, the meetings database, recordings and saved Wi-Fi networks. Survives a power cut. Never loops. Driven by `Command::FactoryReset`, then restarts waved. | The confirmation is B. **Bluetooth pairings:** waved keeps no pairing store yet (`waved-bt` has none). Workstream I adds its store to `ResetPaths` in milestone 3. |
| E7 About | Version with git build id, serial, storage free and total, licences, published in `SystemState.about`. | The About screen is B (`about.rs`). It also shows `ctx.config.device.name` and `snap.wifi.ip_addr`. |

---

## Global Constraints

These come from the spec and the brief. Every task's requirements include them.

- **Timings** (`../docs/mockups/README.md`, "Timings" and "Interaction model"):

  | What | Value |
  |---|---|
  | Resting screen ("Dim after") | 2 min without a touch, configurable |
  | Screen off | 10 min without a touch, unless recording or in a call (then resting only) |
  | First tap on the resting screen | Wake only, nothing else happens |
  | Power key, short press | Anywhere: screen to resting/off, or wake |
  | Power key, hold 2 s | Anywhere: power-off confirmation |

- **Settings ranges** (`waved-config/src/clamp.rs`; already clamped before they reach `SettingsState`):
  - `brightness_pct` 5..=100, default 80
  - `dim_after_s` 0 (never) or 15..=3600, default 120
  - `off_after_s` 0 (never) or 60..=86400 and never before dimming, default 600
  - `time_zone` an IANA name, default `"UTC"`
  - `clock_24h` default true
  - `tones_enabled` default true
- **Panel:** LCD with a PWM backlight, `/sys/class/leds/lcd-backlight/brightness`, 0..=`max_brightness` (255 on the device). Writing 0 blanks the image completely. There is no burn-in to manage (`docs/DISPLAY_HARDWARE.md`).
- **"In a call"** is `AudioState::call_off_hook` (`UiSnapshot::in_call()`). **Recording** is `MeetingState::recording`, which stays true while paused.
- **Clock validity:** wall-clock time before 2020-01-01 (`1_577_836_800`) is not believable. The recorder already uses this threshold (`waved-meeting/src/recorder.rs:159`).
- **Ownership:** this workstream edits only `crates/waved-system/**`, plus the `Cargo.lock` entry Cargo adds for the new package. It never edits `waved-state`, `waved-config`, `waved-settings`, `waved/src/main.rs`, `waved-ui/**` or legacy UI files. Those changes are in **Contract requests** and **Integration requests**.
- **Device space:** `/data` is about 440 MB and nearly full; the waved binary is about 62 MB.
  - The crate embeds no time zone database. It reads the device's `/usr/share/zoneinfo`.
  - The crate writes nothing to `/data` except the factory-reset marker, which is a few bytes and short-lived.
- **Copy:**
  - Plain, sentence case.
  - Log messages say what happened and what follows.
  - No "…" placeholders.
- **Commits:** conventional `type(scope): lowercase summary`, a bullet-list body, ending in
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- **Verification:** run with `set -o pipefail`, from `waved/`:
  - Host tests: `cargo test -p waved-system --lib`
  - Lint gate: `cargo clippy -p waved-system --all-targets -- -D warnings`
  - The crate has no sqlx dependency, so it never needs `scripts/linux-test.sh`.

## Review Focus

These five inputs aren't exercised by the happy-path tests but are likely to hurt someone using the device. Each one has a test, named here.

1. **Slim zoneinfo files and odd zones.** The device's TZif files are slim: `Europe/Brussels` is 1 103 bytes, and every date after about 2007 lives only in the POSIX footer `CET-1CEST,M3.5.0,M10.5.0/3`.
   - DST edges must be exact to the second, including:
     - the southern hemisphere, where DST spans the new year;
     - rule times past midnight (`Asia/Jerusalem`: `M3.4.4/26`);
     - Julian-day rules.
   - A hostile zone name such as `../etc/passwd` must never open a file.

   *(Task 4: `brussels_switches_at_the_eu_dates`, `southern_and_western_zones`, `rule_times_past_midnight_and_julian_days`, `hostile_names_and_files_are_rejected`)*
2. **A recording or a call while the timers expire or the power key is pressed.** The screen may rest but never goes fully dark, so the red ring stays visible. A recording that starts while the screen is off (from the computer) lights the resting screen. *(Task 1: `never_fully_off_while_recording_or_in_a_call`, `a_recording_that_starts_while_off_lights_the_resting_screen`, `key_sleep_during_a_recording_turns_off_when_it_ends`)*
3. **The tap that wakes the screen.** No part of that press may reach the UI, or a bump on the table could mute or record. *(Task 1: `the_first_touch_only_wakes`)*
4. **A factory reset interrupted by power loss, or started before the recordings volume is mounted.**
   - It must finish later.
   - It must never erase the waved binary that sits next to `config.toml` in `/data/waved`.
   - It must never loop forever, erasing settings on every boot.

   *(Task 9: `the_next_start_erases_config_database_recordings_and_wifi`, `an_unmounted_recordings_volume_is_retried_then_given_up`, `a_power_cut_mid_reset_repeats_it`)*
5. **An RTC holding garbage, a chip that was never set, and the 1970 system clock at boot.** Invalid BCD, impossible dates, the oscillator-halt and century bits, and the 12-hour flag must not become a wrong wall clock. A clock that is already valid is never overwritten from the RTC. *(Task 5: `impossible_dates_are_not_a_time`, `oscillator_and_century_bits_are_ignored`, `the_rtc_restores_only_an_unset_system_clock`)*

---

## Contract requests

These land in the integrator's M2.0 task, before any task below runs. Tasks 1, 3, 5, 6, 7 and 8 assume them.

### CR1: the power key event (`crates/waved-state/src/lib.rs`)

```rust
/// The power key, decoded by waved-system (TODO E2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerKeyPress {
    /// Released before 2 s: sleep or wake the screen.
    Short,
    /// Held for 2 s (sent while still held): ask to power off.
    Long,
}

// in `pub enum DeviceEvent`:
    /// The power key was pressed (waved-system power_key reader).
    PowerKey(PowerKeyPress),
```

**Why:** the reader runs on its own thread in the services. The UI runtime already receives discrete device events on `bus.events`. No `match` on `DeviceEvent` is exhaustive today (checked: `waved-hid`, `waved-ui/src/upload_notice.rs`), so adding a variant breaks nothing.

### CR2: system state (`crates/waved-state/src/lib.rs`)

```rust
/// Wall-clock time before 2020-01-01 is not believable (no NTP, no RTC yet).
pub const CLOCK_VALID_AFTER_UNIX_S: i64 = 1_577_836_800;

/// What About shows, collected by waved-system (TODO E7).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AboutInfo {
    /// "0.1.0 (374015f)": crate version and git build id.
    pub version: String,
    /// Device serial; empty when the device exposes none.
    pub serial: String,
    /// The recordings filesystem.
    pub storage_free_bytes: u64,
    pub storage_total_bytes: u64,
    /// One line per component, e.g. "LVGL: MIT".
    pub licences: Vec<String>,
}

/// Produced by waved-system's SystemService.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemState {
    /// Minutes east of UTC of `SettingsState::time_zone` now, DST included.
    /// Refreshed every minute and on a zone change.
    pub utc_offset_min: i32,
    pub about: AboutInfo,
}

// in `pub struct StateBus`:
    pub system: Arc<watch::Sender<SystemState>>,
// in `StateBus::new()`:
    let (system_tx, _) = watch::channel(SystemState::default());
    // ... system: Arc::new(system_tx),
```

**Why:** the offset and the About values come from files and devices that the UI thread must not read on every frame. The integrator's existing test `state_bus_carries_settings_files_and_playback` can add `assert_eq!(bus.system.borrow().utc_offset_min, 0);`.

### CR3: the status clock uses the zone (`crates/waved-ui/src/kit/snapshot.rs`)

```rust
use waved_state::{CLOCK_VALID_AFTER_UNIX_S, SystemState /* , existing imports */};

pub struct UiSnapshot {
    // ... existing fields ...
    /// Time zone offset and About (waved-system).
    pub system: SystemState,
}

impl UiSnapshot {
    pub fn capture(bus: &StateBus, now_unix_s: i64, now_ms: u64) -> Self {
        let settings = bus.settings.borrow().clone();
        let system = bus.system.borrow().clone();
        let clock = if now_unix_s >= CLOCK_VALID_AFTER_UNIX_S {
            format_clock(now_unix_s, system.utc_offset_min, settings.clock_24h)
        } else {
            "--:--".into()
        };
        Self { /* ... existing ..., */ system, clock }
    }

    /// Local wall-clock seconds (for date lines on Date and time and Resting).
    pub fn local_unix_s(&self) -> i64 {
        self.now_unix_s + i64::from(self.system.utc_offset_min) * 60
    }
}
```

Tests to add to `snapshot.rs`:
- after `bus.system.send_modify(|s| s.utc_offset_min = 120)`, `capture(&bus, 1_759_588_320, 0).clock == "16:32"`;
- `capture(&bus, 0, 0).clock == "--:--"`.

**Why:**
- This fixes BUG-9 (UTC clock) for the new runtime.
- Before NTP or the RTC set the clock, the status shows `--:--` instead of a 1970 time. `-` (0x2D) is in both `TEXT_RANGE` and `DIGIT_RANGE`, so Num13 and Num48 render it.
- The minute refresh comes for free: the runtime captures a snapshot every loop iteration, and `StatusArea::update` re-sets the clock text whenever it changes.

## Integration requests (runtime switch, after this plan's tasks)

These need code from this plan, so they can't land in M2.0. The integrator applies them in the runtime-switch task.

**IR1. `crates/waved/Cargo.toml` and `crates/waved-ui/Cargo.toml`:** add `waved-system = { path = "../waved-system" }`.

**IR2. `crates/waved/src/main.rs`.** The factory reset must complete before the config loads, which today is at line 47:

```rust
// first statement after tracing init, before WavedConfig::load_or_default:
let reset_paths = waved_system::reset::ResetPaths::new(CONFIG_PATH, DB_PATH, RECORDINGS_DIR);
#[cfg(not(feature = "simulate"))]
let reset_paths = reset_paths.on_device();
match waved_system::reset::complete_pending(&reset_paths) {
    waved_system::reset::ResetOutcome::NothingPending => {}
    outcome => info!("factory reset at start-up: {outcome:?}"),
}
```

In `run_async`, next to the settings service (pass `reset_paths` into `run_async`):

```rust
tokio::spawn(waved_system::reset::run(reset_paths, bus.commands.subscribe(), bus.commands.clone()));

#[cfg(not(feature = "simulate"))]
let system = waved_system::SystemService::device(Arc::clone(&bus.system), RECORDINGS_DIR);
#[cfg(feature = "simulate")]
let system = waved_system::SystemService::new(Arc::clone(&bus.system), RECORDINGS_DIR);
tokio::spawn(system.run(bus.settings.subscribe(), bus.wifi.subscribe()));

#[cfg(not(feature = "simulate"))]
waved_system::power_key::start(bus.events.clone());
```

In the `capture-alsa` playback block, right after `let playback = Arc::new(playback);`:

```rust
struct PlaybackTones(Arc<waved_audio::playback::PlaybackService>);
impl waved_system::tones::ToneSink for PlaybackTones {
    fn format(&self) -> waved_system::tones::PcmFormat {
        waved_system::tones::PcmFormat {
            sample_rate: self.0.sample_rate(),
            channels: self.0.channels() as usize,
            period_frames: self.0.period_frames(),
        }
    }
    fn submit(&self, period: Vec<i16>) -> bool {
        self.0.play_batch(period).is_ok()
    }
}
tokio::spawn(waved_system::tones::run(
    bus.meeting.subscribe(),
    bus.settings.subscribe(),
    Arc::new(PlaybackTones(Arc::clone(&playback))),
));
```

**IR3. The UI runtime (`waved-ui/src/lib.rs`, new loop).** It owns one `DisplayPower` and one `Backlight` and drives them like this:

```rust
use waved_system::{backlight::{Backlight, BACKLIGHT_DIR}, display::{DisplayPower, Transition, TouchDisposition}};

let mut power = DisplayPower::new(now_ms);
let mut backlight = Backlight::open(BACKLIGHT_DIR);
let apply = |router: &mut Router, t: Option<Transition>, snap: &UiSnapshot| match t {
    Some(t) if t.rests() => router.handle(&UiEvent::Rest, snap),
    Some(t) if t.wakes() => router.handle(&UiEvent::Wake, snap),
    _ => {}
};

// every loop iteration, after `let snap = UiSnapshot::capture(..)`:
let keep_lit = snap.meeting.recording || snap.in_call();
while let Ok(event) = event_rx.try_recv() {
    if let DeviceEvent::PowerKey(press) = event {
        let out = power.on_power_key(press, snap.now_ms, keep_lit);
        apply(&mut router, out.transition, &snap);
        if out.confirm_power_off {
            router.handle(&UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff)), &snap);
        }
    }
}
let tick = power.tick(snap.now_ms, &snap.settings, keep_lit);
apply(&mut router, tick, &snap);
if let Err(error) = backlight.set_percent(power.backlight_pct(snap.settings.brightness_pct)) {
    warn!("backlight not set: {error}"); // set_percent retries only when the level changes
}

// in the touch read callback, for each sample:
let out = power.on_touch(pressed, now_ms);
apply(&mut router, out.transition, &snap);
let report_pressed = pressed && out.disposition == TouchDisposition::Deliver;
```

The legacy `display.rs` init still writes `255` once. The first loop iteration overwrites it with the configured level, so that write can stay.

**IR4. Legacy sounds.** The legacy `audio_fx` calls in `waved-ui/src/lib.rs:371–392` go away with the legacy loop. Only recording start and stop carry over (E5). The mute and volume clicks and `shutdown.wav` are not in the spec. `aplay` also cannot open `hw:0,1` while `PlaybackService` holds it.

**IR5. Docs.** Link `crates/waved-system/README.md` (Task 10) from `CLAUDE.md`'s hardware table, or fold its device facts into `docs/DISPLAY_HARDWARE.md` / `docs/AUDIO_HARDWARE.md` during L1.

## What other workstreams read

- **A (Resting screen):** time comes from `snap.clock`, which is `--:--` until the clock is valid, and the local date from `snap.local_unix_s()`. The runtime and `DisplayPower` decide when Resting shows. The screen never sets the backlight itself.
- **B (System, Display, Date and time, About, Confirmation):**
  - Brightness and Dim after dispatch the existing `Command::SetBrightness` / `SetDimAfter`. `DisplayPower` and `Backlight` follow `snap.settings` live.
  - The time zone shown is `snap.settings.time_zone`. The device's zoneinfo has every IANA zone, so any name `valid_time_zone` accepts works, and an unknown one shows UTC.
  - The About screen reads `snap.system.about.{version, serial, storage_free_bytes, storage_total_bytes, licences}`.
  - Power off confirms with `Command::Shutdown`, as the legacy screen does (`screens/system.rs:78`).
  - Factory reset confirms with `Command::FactoryReset`. Waved then restarts within about 3 s and opens on first start.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/waved-system/Cargo.toml` | Crate manifest | Create (Task 1) |
| `crates/waved-system/src/lib.rs` | Module root and re-exports | Create (Task 1), extend in each task |
| `crates/waved-system/src/display.rs` | `DisplayPower`: rest / off / wake decisions, wake guard, power-key meaning, backlight percent | Create (Task 1) |
| `crates/waved-system/src/backlight.rs` | `Backlight`: sysfs writer, percent → raw level | Create (Task 2) |
| `crates/waved-system/src/power_key.rs` | Node discovery, evdev parsing, `PowerKeyDecoder`, reader threads | Create (Task 3) |
| `crates/waved-system/src/civil.rs` | Calendar arithmetic (days ↔ civil dates, weekday) | Create (Task 4) |
| `crates/waved-system/src/tz.rs` | `TimeZone`: TZif v1/v2 parser with POSIX TZ footer rules | Create (Task 4) |
| `crates/waved-system/src/rtc.rs` | BCD codec, `Rtc` trait, `I2cRtc`, `WallClock`, `restore_time` | Create (Task 5) |
| `crates/waved-system/build.rs` | `WAVED_BUILD_ID` from `git rev-parse` | Create (Task 6) |
| `crates/waved-system/src/about.rs` | Version, serial, storage, licences → `AboutInfo` | Create (Task 6) |
| `crates/waved-system/src/service.rs` | `SystemService`: publishes `SystemState`, RTC restore and save | Create (Task 7) |
| `crates/waved-system/src/tones.rs` | Tone synthesis, `ToneSink`, the tone task | Create (Task 8) |
| `crates/waved-system/src/reset.rs` | Factory reset request, start-up completion, command handler | Create (Task 9) |
| `crates/waved-system/README.md` | Device facts the crate relies on, wiring summary | Create (Task 10) |
| `Cargo.lock` | New package entry (no new external crates; `libc`, `nix`, `thiserror`, `tokio`, `tempfile` are already locked) | Modify (Task 1) |

---

### Task 1: Crate scaffold and the display power controller (E1)

**Files:**
- Create: `crates/waved-system/Cargo.toml`
- Create: `crates/waved-system/src/lib.rs`
- Create: `crates/waved-system/src/display.rs` (tests in `mod tests`)

**Interfaces:**
- Consumes:
  - `waved_state::{SettingsState, PowerKeyPress}` (CR1)
- Produces:
  - `pub enum ScreenPower { Awake, Resting, Off }`
  - `pub struct Transition { pub from: ScreenPower, pub to: ScreenPower }`, with `fn rests(&self) -> bool` and `fn wakes(&self) -> bool`
  - `pub enum TouchDisposition { Deliver, Swallow }`
  - `pub struct TouchOutcome { pub disposition: TouchDisposition, pub transition: Option<Transition> }`
  - `pub struct PowerKeyOutcome { pub transition: Option<Transition>, pub confirm_power_off: bool }`
  - `pub struct DisplayPower`, with:
    - `fn new(now_ms: u64) -> Self`
    - `fn state(&self) -> ScreenPower`
    - `fn on_touch(&mut self, pressed: bool, now_ms: u64) -> TouchOutcome`
    - `fn on_power_key(&mut self, press: PowerKeyPress, now_ms: u64, keep_lit: bool) -> PowerKeyOutcome`
    - `fn tick(&mut self, now_ms: u64, settings: &SettingsState, keep_lit: bool) -> Option<Transition>`
    - `fn backlight_pct(&self, brightness_pct: u8) -> u8`
  - `pub fn resting_pct(brightness_pct: u8) -> u8`
  - `pub const RESTING_SHARE_PCT: u16 = 30; pub const MIN_LIT_PCT: u8 = 5;`

- [ ] **Step 1: Create the manifest** `crates/waved-system/Cargo.toml`:

```toml
[package]
name = "waved-system"
version.workspace = true
edition.workspace = true

[dependencies]
libc = "0.2"
nix.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true
waved-state = { path = "../waved-state" }

[dev-dependencies]
tempfile = "3"
tokio = { workspace = true, features = ["test-util"] }
```

The workspace uses `members = ["crates/*"]`, so the crate is picked up automatically. The workspace `nix` enables `fs`, which `statvfs` needs (Task 6).

- [ ] **Step 2: Create** `crates/waved-system/src/lib.rs`:

```rust
//! System services for the Wave (workstream E): display power, the power key,
//! time zone and RTC, alert tones, factory reset and About. Device access sits
//! behind small types and injected paths so everything runs in host tests.

pub mod display;
```

- [ ] **Step 3: Write the failing tests.** Create `crates/waved-system/src/display.rs` with the module doc, the `use` lines and this test module only:

```rust
//! When the screen rests, turns off and wakes (TODO E1). A pure controller:
//! the UI runtime feeds it touches, power-key presses and the time, and applies
//! what it returns (router events and the backlight level). No I/O here.

use waved_state::{PowerKeyPress, SettingsState};

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn timers(dim_after_s: u32, off_after_s: u32) -> SettingsState {
        SettingsState {
            dim_after_s,
            off_after_s,
            ..SettingsState::default()
        }
    }

    #[test]
    fn rests_after_dim_after_and_turns_off_after_off_after() {
        let s = SettingsState::default(); // 120 s / 600 s
        let mut p = DisplayPower::new(0);
        assert_eq!(p.tick(2 * MIN - 1, &s, false), None);
        let rest = p.tick(2 * MIN, &s, false).unwrap();
        assert!(rest.rests() && !rest.wakes());
        assert_eq!(p.state(), ScreenPower::Resting);
        assert_eq!(p.tick(10 * MIN - 1, &s, false), None);
        let off = p.tick(10 * MIN, &s, false).unwrap();
        assert_eq!(
            off,
            Transition {
                from: ScreenPower::Resting,
                to: ScreenPower::Off
            }
        );
        assert!(!off.rests() && !off.wakes(), "the router already shows Resting");
    }

    #[test]
    fn never_fully_off_while_recording_or_in_a_call() {
        let mut p = DisplayPower::new(0);
        let s = SettingsState::default();
        p.tick(2 * MIN, &s, true);
        assert_eq!(p.tick(10 * MIN, &s, true), None);
        assert_eq!(p.tick(60 * MIN, &s, true), None);
        assert_eq!(p.state(), ScreenPower::Resting);

        // With dimming off, the off timer rests the screen instead.
        let mut q = DisplayPower::new(0);
        let t = q.tick(10 * MIN, &timers(0, 600), true).unwrap();
        assert_eq!(t.to, ScreenPower::Resting);
        assert!(t.rests());
    }

    #[test]
    fn a_recording_that_starts_while_off_lights_the_resting_screen() {
        let mut p = DisplayPower::new(0);
        let s = SettingsState::default();
        p.tick(10 * MIN, &s, false);
        assert_eq!(p.state(), ScreenPower::Off);
        let t = p.tick(10 * MIN + 500, &s, true).unwrap();
        assert_eq!(
            t,
            Transition {
                from: ScreenPower::Off,
                to: ScreenPower::Resting
            }
        );
        assert!(!t.rests() && !t.wakes());
        assert_eq!(p.backlight_pct(80), 24);
    }

    #[test]
    fn the_first_touch_only_wakes() {
        let s = SettingsState::default();
        let mut p = DisplayPower::new(0);
        p.tick(2 * MIN, &s, false);
        let wake = p.on_touch(true, 2 * MIN + 10);
        assert_eq!(wake.disposition, TouchDisposition::Swallow);
        assert!(wake.transition.unwrap().wakes());
        assert_eq!(p.state(), ScreenPower::Awake);
        // The rest of that press, held or released, never reaches the UI.
        assert_eq!(p.on_touch(true, 2 * MIN + 50).disposition, TouchDisposition::Swallow);
        assert_eq!(p.on_touch(false, 2 * MIN + 90).disposition, TouchDisposition::Swallow);
        // The next press does.
        assert_eq!(
            p.on_touch(true, 2 * MIN + 500),
            TouchOutcome {
                disposition: TouchDisposition::Deliver,
                transition: None
            }
        );
    }

    #[test]
    fn touches_while_awake_keep_the_screen_awake() {
        let s = SettingsState::default();
        let mut p = DisplayPower::new(0);
        p.on_touch(true, 100_000);
        p.on_touch(true, 110_000); // held: still activity
        p.on_touch(false, 111_000);
        assert_eq!(p.tick(110_000 + 2 * MIN - 1, &s, false), None);
        assert!(p.tick(110_000 + 2 * MIN, &s, false).unwrap().rests());
    }

    #[test]
    fn zero_means_never() {
        let mut p = DisplayPower::new(0);
        assert_eq!(p.tick(10 * 24 * 60 * MIN, &timers(0, 0), false), None);
        assert_eq!(p.state(), ScreenPower::Awake);
    }

    #[test]
    fn a_longer_dim_setting_does_not_wake_a_resting_screen() {
        let mut p = DisplayPower::new(0);
        p.tick(2 * MIN, &SettingsState::default(), false);
        assert_eq!(p.tick(3 * MIN, &timers(600, 600), false), None);
        assert_eq!(p.state(), ScreenPower::Resting);
    }

    #[test]
    fn power_key_short_press_sleeps_and_wakes() {
        let mut p = DisplayPower::new(0);
        let sleep = p.on_power_key(PowerKeyPress::Short, 1_000, false);
        assert_eq!(sleep.transition.unwrap().to, ScreenPower::Off);
        assert!(sleep.transition.unwrap().rests() && !sleep.confirm_power_off);
        let wake = p.on_power_key(PowerKeyPress::Short, 2_000, false);
        assert!(wake.transition.unwrap().wakes());
        let resting = p.on_power_key(PowerKeyPress::Short, 3_000, true);
        assert_eq!(resting.transition.unwrap().to, ScreenPower::Resting);
    }

    #[test]
    fn key_sleep_during_a_recording_turns_off_when_it_ends() {
        let s = SettingsState::default();
        let mut p = DisplayPower::new(0);
        p.on_power_key(PowerKeyPress::Short, 1_000, true);
        assert_eq!(p.tick(5_000, &s, true), None);
        assert_eq!(p.state(), ScreenPower::Resting);
        assert_eq!(p.tick(6_000, &s, false).unwrap().to, ScreenPower::Off);
    }

    #[test]
    fn long_press_wakes_and_asks_to_power_off() {
        let mut p = DisplayPower::new(0);
        p.tick(10 * MIN, &SettingsState::default(), false);
        let out = p.on_power_key(PowerKeyPress::Long, 10 * MIN + 1, false);
        assert!(out.confirm_power_off && out.transition.unwrap().wakes());
        let awake = p.on_power_key(PowerKeyPress::Long, 10 * MIN + 2, false);
        assert_eq!(
            awake,
            PowerKeyOutcome {
                transition: None,
                confirm_power_off: true
            }
        );
    }

    #[test]
    fn backlight_follows_the_state() {
        let mut p = DisplayPower::new(0);
        assert_eq!(p.backlight_pct(80), 80);
        assert_eq!(p.backlight_pct(0), MIN_LIT_PCT);
        p.tick(2 * MIN, &SettingsState::default(), false);
        assert_eq!(p.backlight_pct(80), 24);
        p.tick(10 * MIN, &SettingsState::default(), false);
        assert_eq!(p.backlight_pct(80), 0);
        assert_eq!((resting_pct(100), resting_pct(5), resting_pct(250)), (30, 5, 30));
    }
}
```

- [ ] **Step 4: Run and watch it fail**

Run: `cargo test -p waved-system --lib display`
Expected: compile errors such as ``cannot find type `DisplayPower` in this scope``.

- [ ] **Step 5: Implement.** Insert between the `use` line and `#[cfg(test)]` in `display.rs`:

```rust
/// Share of the chosen brightness the resting screen keeps, in percent.
pub const RESTING_SHARE_PCT: u16 = 30;
/// The dimmest lit level, in percent of full backlight.
pub const MIN_LIT_PCT: u8 = 5;

/// Ordered from lit to dark, so "deeper" is `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenPower {
    Awake,
    Resting,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub from: ScreenPower,
    pub to: ScreenPower,
}

impl Transition {
    /// Leaving Awake: the router shows the resting screen and relocks the PIN
    /// (`UiEvent::Rest`).
    pub fn rests(&self) -> bool {
        self.from == ScreenPower::Awake && self.to != ScreenPower::Awake
    }

    /// Back to Awake (`UiEvent::Wake`).
    pub fn wakes(&self) -> bool {
        self.to == ScreenPower::Awake && self.from != ScreenPower::Awake
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchDisposition {
    /// Report the touch to LVGL.
    Deliver,
    /// Report "released": this press only woke the screen.
    Swallow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchOutcome {
    pub disposition: TouchDisposition,
    pub transition: Option<Transition>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerKeyOutcome {
    pub transition: Option<Transition>,
    /// Open the power-off confirmation (`Route::Confirm(ConfirmKind::PowerOff)`).
    pub confirm_power_off: bool,
}

#[derive(Clone, Debug)]
pub struct DisplayPower {
    state: ScreenPower,
    last_activity_ms: u64,
    touching: bool,
    swallowing: bool,
    slept_by_key: bool,
}

impl DisplayPower {
    pub fn new(now_ms: u64) -> Self {
        Self {
            state: ScreenPower::Awake,
            last_activity_ms: now_ms,
            touching: false,
            swallowing: false,
            slept_by_key: false,
        }
    }

    pub fn state(&self) -> ScreenPower {
        self.state
    }

    /// One touch sample. A press that starts while the screen is not awake
    /// wakes it and is swallowed until it is released.
    pub fn on_touch(&mut self, pressed: bool, now_ms: u64) -> TouchOutcome {
        let started = pressed && !self.touching;
        self.touching = pressed;
        if self.swallowing {
            if !pressed {
                self.swallowing = false;
            }
            return TouchOutcome {
                disposition: TouchDisposition::Swallow,
                transition: None,
            };
        }
        if started && self.state != ScreenPower::Awake {
            let transition = self.wake(now_ms);
            self.swallowing = true;
            return TouchOutcome {
                disposition: TouchDisposition::Swallow,
                transition,
            };
        }
        if pressed {
            self.last_activity_ms = now_ms;
        }
        TouchOutcome {
            disposition: TouchDisposition::Deliver,
            transition: None,
        }
    }

    /// Short: sleep (off, or resting while `keep_lit`) or wake. Long: wake and
    /// ask to power off.
    pub fn on_power_key(&mut self, press: PowerKeyPress, now_ms: u64, keep_lit: bool) -> PowerKeyOutcome {
        match press {
            PowerKeyPress::Short if self.state == ScreenPower::Awake => {
                self.slept_by_key = true;
                let to = if keep_lit { ScreenPower::Resting } else { ScreenPower::Off };
                PowerKeyOutcome {
                    transition: self.set(to),
                    confirm_power_off: false,
                }
            }
            PowerKeyPress::Short => PowerKeyOutcome {
                transition: self.wake(now_ms),
                confirm_power_off: false,
            },
            PowerKeyPress::Long => PowerKeyOutcome {
                transition: self.wake(now_ms),
                confirm_power_off: true,
            },
        }
    }

    /// Apply the timers. Only moves deeper (Awake → Resting → Off), except that
    /// `keep_lit` (recording or in a call) never lets the screen go fully off
    /// and lifts Off back to Resting.
    pub fn tick(&mut self, now_ms: u64, settings: &SettingsState, keep_lit: bool) -> Option<Transition> {
        let target = if self.slept_by_key {
            ScreenPower::Off
        } else {
            self.timed_target(now_ms, settings)
        };
        let next = if keep_lit {
            if self.state == ScreenPower::Off {
                ScreenPower::Resting
            } else {
                self.state.max(target.min(ScreenPower::Resting))
            }
        } else {
            self.state.max(target)
        };
        self.set(next)
    }

    /// Backlight in percent for the current state.
    pub fn backlight_pct(&self, brightness_pct: u8) -> u8 {
        match self.state {
            ScreenPower::Awake => brightness_pct.clamp(MIN_LIT_PCT, 100),
            ScreenPower::Resting => resting_pct(brightness_pct),
            ScreenPower::Off => 0,
        }
    }

    fn timed_target(&self, now_ms: u64, settings: &SettingsState) -> ScreenPower {
        let idle_ms = now_ms.saturating_sub(self.last_activity_ms);
        let elapsed = |seconds: u32| seconds != 0 && idle_ms >= u64::from(seconds) * 1_000;
        if elapsed(settings.off_after_s) {
            ScreenPower::Off
        } else if elapsed(settings.dim_after_s) {
            ScreenPower::Resting
        } else {
            ScreenPower::Awake
        }
    }

    fn wake(&mut self, now_ms: u64) -> Option<Transition> {
        self.last_activity_ms = now_ms;
        self.slept_by_key = false;
        self.set(ScreenPower::Awake)
    }

    fn set(&mut self, to: ScreenPower) -> Option<Transition> {
        if to == self.state {
            return None;
        }
        let transition = Transition { from: self.state, to };
        self.state = to;
        Some(transition)
    }
}

/// The resting screen's backlight: a share of the chosen brightness, never
/// below `MIN_LIT_PCT`.
pub fn resting_pct(brightness_pct: u8) -> u8 {
    let share = u16::from(brightness_pct.min(100)) * RESTING_SHARE_PCT / 100;
    (share as u8).max(MIN_LIT_PCT)
}
```

- [ ] **Step 6: Run and watch it pass**

Run: `cargo test -p waved-system --lib display`
Expected: `test result: ok. 11 passed`.

- [ ] **Step 7: Lint**

Run: `cargo clippy -p waved-system --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Commit** (include the `Cargo.lock` hunk that adds `waved-system`)

```bash
git add crates/waved-system Cargo.lock
git commit -m "feat(system): add waved-system with the display power controller

- dim after / off after timers as a pure controller the UI runtime drives
- never fully off while recording or in a call; off lifts to resting
- first touch after resting only wakes; power key short sleeps or wakes, long asks to power off
- resting backlight is 30 % of the chosen brightness, never below 5 %

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Backlight writer (E1)

**Files:**
- Create: `crates/waved-system/src/backlight.rs` (tests in `mod tests`)
- Modify: `crates/waved-system/src/lib.rs` (add `pub mod backlight;`)

**Interfaces:**
- Consumes: nothing beyond `std`.
- Produces:
  - `pub const BACKLIGHT_DIR: &str = "/sys/class/leds/lcd-backlight"`
  - `pub const DEFAULT_MAX: u32 = 255`
  - `pub fn raw_level(pct: u8, max: u32) -> u32`
  - `pub struct Backlight`, with:
    - `fn open(dir: impl AsRef<Path>) -> Self`
    - `fn max(&self) -> u32`
    - `fn set_percent(&mut self, pct: u8) -> io::Result<bool>`

- [ ] **Step 1: Add the module** to `lib.rs`, after `pub mod display;`: `pub mod backlight;`. Keep the module lines in alphabetical order.

- [ ] **Step 2: Write the failing tests.** Create `crates/waved-system/src/backlight.rs`:

```rust
//! The LCD backlight in sysfs: `brightness` takes 0..=`max_brightness`
//! (255 on the device). 0 blanks the panel (docs/DISPLAY_HARDWARE.md).

use std::io;
use std::path::{Path, PathBuf};

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
```

- [ ] **Step 3: Run and watch it fail**

Run: `cargo test -p waved-system --lib backlight`
Expected: ``cannot find function `raw_level` in this scope``.

- [ ] **Step 4: Implement.** Insert before `#[cfg(test)]`:

```rust
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
```

- [ ] **Step 5: Run and watch it pass**

Run: `cargo test -p waved-system --lib backlight`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 6: Lint.** Run `cargo clippy -p waved-system --all-targets -- -D warnings`; expect no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-system
git commit -m "feat(system): write the lcd backlight level from a percentage

- reads max_brightness, falls back to 255
- writes only when the level changes, so a failing sysfs file is not retried every frame

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Power key reader (E2)

**Which node.** `docs/AUDIO_HARDWARE.md:670` records that `vad_keypad` (`event0`) advertises only `KEY_POWER`, and that `ILITEK_TDDI` (`event1`) is touch. The legacy `input.rs:40` agrees. The reader finds the node in two passes:
1. by kernel name (`/sys/class/input/eventN/device/name == "vad_keypad"`);
2. by the `KEY_POWER` bit in `device/capabilities/key`.

It never hard-codes `event0`, because input numbering follows probe order. The device pass first confirms that pressing the physical key produces these events (see Device verification, item 4).

**Files:**
- Create: `crates/waved-system/src/power_key.rs` (tests in `mod tests`)
- Modify: `crates/waved-system/src/lib.rs` (add `pub mod power_key;`)

**Interfaces:**
- Consumes:
  - `waved_state::{DeviceEvent, PowerKeyPress}` (CR1)
  - `tokio::sync::broadcast::Sender<DeviceEvent>`
- Produces:
  - constants `POWER_KEY_HOLD_MS: u64 = 2_000`, `SYS_INPUT`, `DEV_INPUT`, `POWER_KEY_DEVICE = "vad_keypad"`, `INPUT_EVENT_SIZE = 24`
  - `pub fn find_power_key(sys_input: &Path, dev_input: &Path) -> Option<PathBuf>`
  - `pub fn has_key(bitmap: &str, code: u16) -> bool`
  - `pub struct InputEvent { pub kind: u16, pub code: u16, pub value: i32 }`
  - `pub fn parse_event(raw: &[u8; INPUT_EVENT_SIZE]) -> InputEvent`
  - `pub fn power_edge(event: InputEvent) -> Option<bool>`
  - `pub struct PowerKeyDecoder`, with:
    - `fn new(hold_ms: u64) -> Self`
    - `fn on_edge(&mut self, pressed: bool, now_ms: u64) -> Option<PowerKeyPress>`
    - `fn deadline_ms(&self) -> Option<u64>`
    - `fn on_time(&mut self, now_ms: u64) -> Option<PowerKeyPress>`
  - `pub fn run_decoder(edges: mpsc::Receiver<bool>, hold_ms: u64, emit: impl FnMut(PowerKeyPress))`
  - `pub fn spawn(node: PathBuf, events: broadcast::Sender<DeviceEvent>) -> io::Result<()>`
  - `pub fn start(events: broadcast::Sender<DeviceEvent>) -> bool`

- [ ] **Step 1: Add the module** to `lib.rs`: `pub mod power_key;`.

- [ ] **Step 2: Write the failing tests.** Create `crates/waved-system/src/power_key.rs`:

```rust
//! The power key (TODO E2): find its input node, read KEY_POWER edges and turn
//! them into short and long presses. The UI runtime receives them as
//! `DeviceEvent::PowerKey` and decides what they mean (`display::DisplayPower`).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::{info, warn};
use waved_state::{DeviceEvent, PowerKeyPress};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn input_tree(devices: &[(&str, &str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (node, name, keys) in devices {
            let device = dir.path().join(node).join("device");
            std::fs::create_dir_all(device.join("capabilities")).unwrap();
            std::fs::write(device.join("name"), format!("{name}\n")).unwrap();
            std::fs::write(device.join("capabilities/key"), format!("{keys}\n")).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("input0")).unwrap(); // not an event node
        dir
    }

    fn raw(kind: u16, code: u16, value: i32) -> [u8; INPUT_EVENT_SIZE] {
        let mut raw = [0u8; INPUT_EVENT_SIZE];
        raw[16..18].copy_from_slice(&kind.to_le_bytes());
        raw[18..20].copy_from_slice(&code.to_le_bytes());
        raw[20..24].copy_from_slice(&value.to_le_bytes());
        raw
    }

    const TOUCH_KEYS: &str = "400 0 0 0 0 0"; // BTN_TOUCH (330)
    const POWER_KEYS: &str = "10000000000000 0"; // KEY_POWER (116)

    #[test]
    fn the_power_key_node_is_found_by_name_then_by_capability() {
        let dev = Path::new("/dev/input");
        let wave = input_tree(&[("event1", "ILITEK_TDDI", TOUCH_KEYS), ("event0", "vad_keypad", POWER_KEYS)]);
        assert_eq!(find_power_key(wave.path(), dev), Some(dev.join("event0")));
        let renamed = input_tree(&[("event1", "ILITEK_TDDI", TOUCH_KEYS), ("event3", "gpio-keys", POWER_KEYS)]);
        assert_eq!(find_power_key(renamed.path(), dev), Some(dev.join("event3")));
        let none = input_tree(&[("event1", "ILITEK_TDDI", TOUCH_KEYS)]);
        assert_eq!(find_power_key(none.path(), dev), None);
        assert_eq!(find_power_key(Path::new("/nonexistent/sys"), dev), None);
    }

    #[test]
    fn key_bitmaps_are_read_most_significant_word_first() {
        assert!(has_key(POWER_KEYS, 116));
        assert!(!has_key(POWER_KEYS, 115));
        assert!(has_key(TOUCH_KEYS, 330));
        assert!(!has_key(TOUCH_KEYS, 116));
        assert!(!has_key("0", 116));
        assert!(!has_key("", 116));
        assert!(!has_key("zz 0", 116));
    }

    #[test]
    fn input_events_decode_from_the_64_bit_layout() {
        let press = parse_event(&raw(1, 116, 1));
        assert_eq!(press, InputEvent { kind: 1, code: 116, value: 1 });
        assert_eq!(power_edge(press), Some(true));
        assert_eq!(power_edge(parse_event(&raw(1, 116, 0))), Some(false));
        assert_eq!(power_edge(parse_event(&raw(1, 116, 2))), None, "auto-repeat");
        assert_eq!(power_edge(parse_event(&raw(1, 115, 1))), None, "another key");
        assert_eq!(power_edge(parse_event(&raw(0, 0, 0))), None, "EV_SYN");
    }

    #[test]
    fn short_and_long_presses_split_at_two_seconds() {
        let mut d = PowerKeyDecoder::new(POWER_KEY_HOLD_MS);
        assert_eq!(d.on_edge(true, 0), None);
        assert_eq!(d.on_edge(false, 1_999), Some(PowerKeyPress::Short));

        assert_eq!(d.on_edge(true, 10_000), None);
        assert_eq!(d.deadline_ms(), Some(12_000));
        assert_eq!(d.on_time(11_999), None);
        assert_eq!(d.on_time(12_000), Some(PowerKeyPress::Long));
        assert_eq!(d.deadline_ms(), None, "a long press is reported once");
        assert_eq!(d.on_edge(false, 15_000), None);

        // The timer ran late: the release still counts as a long press.
        assert_eq!(d.on_edge(true, 20_000), None);
        assert_eq!(d.on_edge(false, 22_000), Some(PowerKeyPress::Long));

        // A release without a press, and a missed release.
        assert_eq!(d.on_edge(false, 30_000), None);
        d.on_edge(true, 40_000);
        d.on_edge(true, 45_000);
        assert_eq!(d.on_edge(false, 45_100), Some(PowerKeyPress::Short));
    }

    #[test]
    fn the_decoder_thread_reports_a_held_key_before_release() {
        let (tx, rx) = mpsc::channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let thread = std::thread::spawn(move || run_decoder(rx, 50, move |p| sink.lock().unwrap().push(p)));
        tx.send(true).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(*seen.lock().unwrap(), vec![PowerKeyPress::Long]);
        tx.send(false).unwrap();
        tx.send(true).unwrap();
        tx.send(false).unwrap();
        drop(tx);
        thread.join().unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![PowerKeyPress::Long, PowerKeyPress::Short]);
    }
}
```

- [ ] **Step 3: Run and watch it fail**

Run: `cargo test -p waved-system --lib power_key`
Expected: ``cannot find function `find_power_key` in this scope`` and similar.

- [ ] **Step 4: Implement.** Insert before `#[cfg(test)]`:

```rust
/// Holding the key this long asks to power off (docs/mockups/README.md).
pub const POWER_KEY_HOLD_MS: u64 = 2_000;
pub const SYS_INPUT: &str = "/sys/class/input";
pub const DEV_INPUT: &str = "/dev/input";
/// The input device that advertises KEY_POWER (docs/AUDIO_HARDWARE.md).
pub const POWER_KEY_DEVICE: &str = "vad_keypad";
/// `struct input_event` on 64-bit Linux: timeval (16) + type (2) + code (2) + value (4).
pub const INPUT_EVENT_SIZE: usize = 24;

const EV_KEY: u16 = 0x01;
const KEY_POWER: u16 = 116;

/// The `/dev/input/eventN` node of the power key: by device name first, then
/// by the KEY_POWER capability bit.
pub fn find_power_key(sys_input: &Path, dev_input: &Path) -> Option<PathBuf> {
    let mut nodes: Vec<String> = std::fs::read_dir(sys_input)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("event"))
        .collect();
    nodes.sort_by_key(|name| name["event".len()..].parse::<u32>().unwrap_or(u32::MAX));
    let read = |node: &str, file: &str| std::fs::read_to_string(sys_input.join(node).join("device").join(file)).ok();
    let by_name = nodes
        .iter()
        .find(|node| read(node, "name").is_some_and(|name| name.trim() == POWER_KEY_DEVICE));
    let by_capability = || {
        nodes
            .iter()
            .find(|node| read(node, "capabilities/key").is_some_and(|keys| has_key(&keys, KEY_POWER)))
    };
    by_name.or_else(by_capability).map(|node| dev_input.join(node))
}

/// A sysfs key bitmap: hex words, most significant first, 64 bits each (aarch64).
pub fn has_key(bitmap: &str, code: u16) -> bool {
    let words: Vec<&str> = bitmap.split_whitespace().collect();
    let index = usize::from(code / 64);
    let Some(word) = words.len().checked_sub(index + 1).and_then(|i| words.get(i)) else {
        return false;
    };
    u64::from_str_radix(word, 16).is_ok_and(|bits| bits & (1 << (code % 64)) != 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

pub fn parse_event(raw: &[u8; INPUT_EVENT_SIZE]) -> InputEvent {
    InputEvent {
        kind: u16::from_le_bytes([raw[16], raw[17]]),
        code: u16::from_le_bytes([raw[18], raw[19]]),
        value: i32::from_le_bytes([raw[20], raw[21], raw[22], raw[23]]),
    }
}

/// `Some(pressed)` for a KEY_POWER press or release; auto-repeat and every
/// other event are `None`.
pub fn power_edge(event: InputEvent) -> Option<bool> {
    match (event.kind, event.code, event.value) {
        (EV_KEY, KEY_POWER, 1) => Some(true),
        (EV_KEY, KEY_POWER, 0) => Some(false),
        _ => None,
    }
}

/// Press and release edges → `Short` (released before the hold time) or
/// `Long` (reported once, as soon as the hold time passes).
#[derive(Clone, Debug)]
pub struct PowerKeyDecoder {
    hold_ms: u64,
    pressed_at: Option<u64>,
    long_sent: bool,
}

impl PowerKeyDecoder {
    pub fn new(hold_ms: u64) -> Self {
        Self {
            hold_ms,
            pressed_at: None,
            long_sent: false,
        }
    }

    pub fn on_edge(&mut self, pressed: bool, now_ms: u64) -> Option<PowerKeyPress> {
        if pressed {
            self.pressed_at = Some(now_ms);
            self.long_sent = false;
            return None;
        }
        let pressed_at = self.pressed_at.take()?;
        if std::mem::take(&mut self.long_sent) {
            None
        } else if now_ms.saturating_sub(pressed_at) >= self.hold_ms {
            Some(PowerKeyPress::Long)
        } else {
            Some(PowerKeyPress::Short)
        }
    }

    /// When `on_time` should run next, while the key is held.
    pub fn deadline_ms(&self) -> Option<u64> {
        match (self.pressed_at, self.long_sent) {
            (Some(at), false) => Some(at + self.hold_ms),
            _ => None,
        }
    }

    pub fn on_time(&mut self, now_ms: u64) -> Option<PowerKeyPress> {
        let deadline = self.deadline_ms()?;
        if now_ms >= deadline {
            self.long_sent = true;
            Some(PowerKeyPress::Long)
        } else {
            None
        }
    }
}

/// Decode edges until the sender hangs up, waking at the hold deadline.
pub fn run_decoder(edges: mpsc::Receiver<bool>, hold_ms: u64, mut emit: impl FnMut(PowerKeyPress)) {
    let start = Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    let mut decoder = PowerKeyDecoder::new(hold_ms);
    loop {
        let received = match decoder.deadline_ms() {
            Some(deadline) => edges.recv_timeout(Duration::from_millis(deadline.saturating_sub(now()))),
            None => edges.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let press = match received {
            Ok(pressed) => decoder.on_edge(pressed, now()),
            Err(RecvTimeoutError::Timeout) => decoder.on_time(now()),
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if let Some(press) = press {
            emit(press);
        }
    }
}

/// Read `node` on one thread, decode on another, send presses to `events`.
pub fn spawn(node: PathBuf, events: broadcast::Sender<DeviceEvent>) -> io::Result<()> {
    let mut file = File::open(&node)?;
    info!("power key: reading {}", node.display());
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("power-key-read".into())
        .spawn(move || {
            let mut raw = [0u8; INPUT_EVENT_SIZE];
            loop {
                if let Err(error) = file.read_exact(&mut raw) {
                    warn!("power key: read failed, no more presses: {error}");
                    return;
                }
                if let Some(pressed) = power_edge(parse_event(&raw)) {
                    if tx.send(pressed).is_err() {
                        return;
                    }
                }
            }
        })?;
    std::thread::Builder::new().name("power-key".into()).spawn(move || {
        run_decoder(rx, POWER_KEY_HOLD_MS, |press| {
            info!("power key: {press:?} press");
            let _ = events.send(DeviceEvent::PowerKey(press));
        })
    })?;
    Ok(())
}

/// Find the power key and start reading it. Logs and returns false when the
/// device has none or it can't be opened.
pub fn start(events: broadcast::Sender<DeviceEvent>) -> bool {
    let Some(node) = find_power_key(Path::new(SYS_INPUT), Path::new(DEV_INPUT)) else {
        warn!("power key: no input device advertises KEY_POWER; the key does nothing");
        return false;
    };
    match spawn(node, events) {
        Ok(()) => true,
        Err(error) => {
            warn!("power key: not started: {error}");
            false
        }
    }
}
```

- [ ] **Step 5: Run and watch it pass**

Run: `cargo test -p waved-system --lib power_key`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 6: Lint.** Run `cargo clippy -p waved-system --all-targets -- -D warnings`; expect no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-system
git commit -m "feat(system): read the power key and decode short and long presses

- finds the node by name (vad_keypad), then by the KEY_POWER capability bit
- decodes 64-bit input_event records; ignores auto-repeat
- long press is reported once, at 2 s while still held; sends DeviceEvent::PowerKey

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Calendar arithmetic and time zone offsets (E3)

**Files:**
- Create: `crates/waved-system/src/civil.rs` (tests in `mod tests`)
- Create: `crates/waved-system/src/tz.rs` (tests in `mod tests`, plus a `#[cfg(test)] pub(crate) mod fixture` that Task 7 reuses)
- Modify: `crates/waved-system/src/lib.rs` (add `mod civil;` and `pub mod tz;`)

**Interfaces:**
- Consumes: nothing beyond `std` and `thiserror`.
- Produces:
  - `civil` (crate-private):
    - `is_leap(i64) -> bool`
    - `month_len(i64, u32) -> i64`
    - `days_from_civil(i64, u32, u32) -> i64`
    - `civil_from_days(i64) -> (i64, u32, u32)`
    - `weekday(i64) -> u32` (0 = Sunday)
  - `tz`:
    - `pub const ZONEINFO_DIR: &str = "/usr/share/zoneinfo"`
    - `pub enum TzError { BadName(String), Io(io::Error), Format(&'static str) }`
    - `pub struct TimeZone`, with:
      - `fn utc() -> Self`
      - `fn load(zoneinfo: &Path, name: &str) -> Result<Self, TzError>`
      - `fn parse(data: &[u8]) -> Result<Self, TzError>`
      - `fn offset_at(&self, unix_s: i64) -> i32` (seconds east of UTC)
    - `tz::fixture::tzif(transitions: &[(i64, u8)], offsets: &[(i32, bool)], footer: &str) -> Vec<u8>` (test only)

- [ ] **Step 1: Add the modules** to `lib.rs`: `mod civil;` and `pub mod tz;`.

- [ ] **Step 2: Write the failing civil tests.** Create `crates/waved-system/src/civil.rs`:

```rust
//! Proleptic Gregorian calendar arithmetic on days since 1970-01-01
//! (Howard Hinnant's algorithms), shared by the time zone rules and the RTC.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates_map_to_their_day_numbers() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 1, 1), 20_454);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(civil_from_days(20_731), (2026, 10, 5));
    }

    #[test]
    fn day_numbers_round_trip() {
        for days in (-1_000..40_000).step_by(37) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
    }

    #[test]
    fn weekdays_and_month_lengths() {
        assert_eq!(weekday(0), 4, "1970-01-01 was a Thursday");
        assert_eq!(weekday(days_from_civil(2026, 10, 5)), 1, "Monday");
        assert_eq!((month_len(2024, 2), month_len(2026, 2), month_len(2100, 2)), (29, 28, 28));
        assert_eq!((month_len(2026, 4), month_len(2026, 12)), (30, 31));
        assert!(is_leap(2000) && !is_leap(1900));
    }
}
```

- [ ] **Step 3: Write the failing tz tests.** Create `crates/waved-system/src/tz.rs`:

```rust
//! Time zone offsets from the device's TZif files (/usr/share/zoneinfo). The
//! device ships the slim format: a few historic transitions, then a POSIX TZ
//! footer that carries the rules for every later date. No database is
//! embedded in the binary.

use crate::civil::{civil_from_days, days_from_civil, is_leap, month_len, weekday};
use std::path::Path;

#[cfg(test)]
pub(crate) mod fixture {
    /// A TZif v2 file as `zic -b slim` writes it: a minimal v1 block, then the
    /// v2 transitions, local time types and the footer.
    pub(crate) fn tzif(transitions: &[(i64, u8)], offsets: &[(i32, bool)], footer: &str) -> Vec<u8> {
        fn header(out: &mut Vec<u8>, timecnt: usize, typecnt: usize, charcnt: usize) {
            out.extend_from_slice(b"TZif2");
            out.extend_from_slice(&[0; 15]);
            for count in [0, 0, 0, timecnt, typecnt, charcnt] {
                out.extend_from_slice(&(count as u32).to_be_bytes());
            }
        }
        let mut out = Vec::new();
        header(&mut out, 0, 1, 1);
        out.extend_from_slice(&[0; 7]); // one v1 type (6 bytes) and one designation byte
        header(&mut out, transitions.len(), offsets.len(), 1);
        for (at, _) in transitions {
            out.extend_from_slice(&at.to_be_bytes());
        }
        for (_, kind) in transitions {
            out.push(*kind);
        }
        for (offset, dst) in offsets {
            out.extend_from_slice(&offset.to_be_bytes());
            out.push(u8::from(*dst));
            out.push(0);
        }
        out.push(0);
        out.push(b'\n');
        out.extend_from_slice(footer.as_bytes());
        out.push(b'\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::tzif;
    use super::*;

    fn footer_only(std_offset: i32, footer: &str) -> TimeZone {
        TimeZone::parse(&tzif(&[], &[(std_offset, false)], footer)).unwrap()
    }

    const JAN_15_2026: i64 = 1_768_478_400; // 12:00 UTC
    const JUL_15_2026: i64 = 1_784_116_800; // 12:00 UTC

    #[test]
    fn brussels_switches_at_the_eu_dates() {
        let zone = footer_only(3_600, "CET-1CEST,M3.5.0,M10.5.0/3");
        assert_eq!(zone.offset_at(JAN_15_2026), 3_600);
        assert_eq!(zone.offset_at(JUL_15_2026), 7_200);
        // 2026-03-29 01:00 UTC and 2026-10-25 01:00 UTC.
        assert_eq!(zone.offset_at(1_774_745_999), 3_600);
        assert_eq!(zone.offset_at(1_774_746_000), 7_200);
        assert_eq!(zone.offset_at(1_792_889_999), 7_200);
        assert_eq!(zone.offset_at(1_792_890_000), 3_600);
    }

    #[test]
    fn southern_and_western_zones() {
        let sydney = footer_only(36_000, "AEST-10AEDT,M10.1.0,M4.1.0/3");
        assert_eq!(sydney.offset_at(JAN_15_2026), 39_600);
        assert_eq!(sydney.offset_at(JUL_15_2026), 36_000);
        let new_york = footer_only(-18_000, "EST5EDT,M3.2.0,M11.1.0");
        assert_eq!(new_york.offset_at(JAN_15_2026), -18_000);
        assert_eq!(new_york.offset_at(JUL_15_2026), -14_400);
        assert_eq!(footer_only(19_800, "IST-5:30").offset_at(JUL_15_2026), 19_800);
        assert_eq!(footer_only(-10_800, "<-03>3").offset_at(JUL_15_2026), -10_800);
        assert_eq!(footer_only(20_700, "<+0545>-5:45").offset_at(JUL_15_2026), 20_700);
        assert_eq!(footer_only(0, "UTC0").offset_at(JUL_15_2026), 0);
    }

    #[test]
    fn rule_times_past_midnight_and_julian_days() {
        // Asia/Jerusalem: the 4th Thursday of March at 26:00 = Friday 02:00 local.
        let jerusalem = footer_only(7_200, "IST-2IDT,M3.4.4/26,M10.5.0");
        assert_eq!(jerusalem.offset_at(1_774_569_599), 7_200); // 2026-03-26 23:59:59 UTC
        assert_eq!(jerusalem.offset_at(1_774_569_600), 10_800);
        // Jn ignores 29 February: J60 is 1 March even in 2024.
        let julian = footer_only(0, "AAA0BBB,J60/0,J300/0");
        assert_eq!(julian.offset_at(1_709_251_199), 0);
        assert_eq!(julian.offset_at(1_709_251_200), 3_600); // 2024-03-01 00:00 UTC
        // n counts from 0 and includes 29 February.
        let zero_based = footer_only(0, "AAA0BBB,59/0,299/0");
        assert_eq!(zero_based.offset_at(1_709_164_799), 0);
        assert_eq!(zero_based.offset_at(1_709_164_800), 3_600); // 2024-02-29 00:00 UTC
    }

    #[test]
    fn transitions_before_the_footer_win() {
        let table = TimeZone::parse(&tzif(&[(1_000_000_000, 1)], &[(0, false), (7_200, false)], "")).unwrap();
        assert_eq!(table.offset_at(999_999_999), 0, "type 0 before the first transition");
        assert_eq!(table.offset_at(1_000_000_000), 7_200);
        assert_eq!(table.offset_at(2_000_000_000), 7_200, "no footer: the last type holds");

        let both = TimeZone::parse(&tzif(&[(0, 1)], &[(1_000, false), (3_600, false)], "CET-1CEST,M3.5.0,M10.5.0/3")).unwrap();
        assert_eq!(both.offset_at(-1), 1_000);
        assert_eq!(both.offset_at(JUL_15_2026), 7_200, "the footer covers later dates");
    }

    #[test]
    fn hostile_names_and_files_are_rejected() {
        let dir = Path::new("/usr/share/zoneinfo");
        for name in ["../etc/passwd", "/etc/passwd", "Europe/../../etc", "Europe//Brussels", "", "Europe/Brus sels"] {
            assert!(matches!(TimeZone::load(dir, name), Err(TzError::BadName(_))), "{name:?}");
        }
        assert!(TimeZone::parse(b"nope").is_err());
        let good = tzif(&[], &[(3_600, false)], "CET-1");
        assert!(TimeZone::parse(&good[..good.len() - 20]).is_err(), "truncated");
        assert!(TimeZone::parse(&tzif(&[(0, 5)], &[(0, false)], "")).is_err(), "type index out of range");
        assert!(TimeZone::parse(&tzif(&[(5, 0), (1, 0)], &[(0, false)], "")).is_err(), "out of order");
        assert!(TimeZone::parse(&tzif(&[], &[(0, false)], "garbage!!")).is_err(), "bad footer");
        assert!(TimeZone::parse(&tzif(&[], &[], "UTC0")).is_err(), "no local time types");
    }

    #[test]
    fn utc_needs_no_file_and_load_reads_the_named_file() {
        assert_eq!(TimeZone::load(Path::new("/nonexistent"), "UTC").unwrap().offset_at(JUL_15_2026), 0);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Europe")).unwrap();
        std::fs::write(dir.path().join("Europe/Brussels"), tzif(&[], &[(3_600, false)], "CET-1CEST,M3.5.0,M10.5.0/3")).unwrap();
        assert_eq!(TimeZone::load(dir.path(), "Europe/Brussels").unwrap().offset_at(JUL_15_2026), 7_200);
        assert!(matches!(TimeZone::load(dir.path(), "Mars/Olympus"), Err(TzError::Io(_))));
    }
}
```

- [ ] **Step 4: Run and watch them fail**

Run: `cargo test -p waved-system --lib civil tz`
Expected: ``cannot find function `days_from_civil` in this scope`` and ``cannot find type `TimeZone` in this scope``.

- [ ] **Step 5: Implement `civil.rs`.** Insert before `#[cfg(test)]`:

```rust
pub(crate) fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

pub(crate) fn month_len(year: i64, month: u32) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (month, day) = (i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month 1..=12, day 1..=31) for days since 1970-01-01.
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// 0 = Sunday.
pub(crate) fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}
```

- [ ] **Step 6: Implement `tz.rs`.** Insert between the `use` lines and `#[cfg(test)] pub(crate) mod fixture`:

```rust
pub const ZONEINFO_DIR: &str = "/usr/share/zoneinfo";

#[derive(Debug, thiserror::Error)]
pub enum TzError {
    #[error("time zone name {0:?} is not a plain IANA name")]
    BadName(String),
    #[error("cannot read the time zone file: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad TZif data: {0}")]
    Format(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimeZone {
    transitions: Vec<i64>,
    /// Index into `offsets` for each transition.
    kinds: Vec<u8>,
    /// UTC offset in seconds of each local time type.
    offsets: Vec<i32>,
    footer: Option<PosixTz>,
}

impl TimeZone {
    pub fn utc() -> Self {
        Self {
            transitions: Vec::new(),
            kinds: Vec::new(),
            offsets: vec![0],
            footer: None,
        }
    }

    /// `zoneinfo/name`. "UTC" needs no file. Names with `..`, empty parts or
    /// anything but `[A-Za-z0-9_+-]` are refused before any file is opened.
    pub fn load(zoneinfo: &Path, name: &str) -> Result<Self, TzError> {
        if name == "UTC" {
            return Ok(Self::utc());
        }
        if !plain_name(name) {
            return Err(TzError::BadName(name.into()));
        }
        Self::parse(&std::fs::read(zoneinfo.join(name))?)
    }

    /// RFC 8536 TZif, version 1 (32-bit times) or 2+ (64-bit times and footer).
    pub fn parse(data: &[u8]) -> Result<Self, TzError> {
        let first = Header::read(data)?;
        let (head, body, time_size) = if first.version >= b'2' {
            let second_at = 44 + first.data_len(4);
            let second = Header::read(data.get(second_at..).ok_or(TzError::Format("truncated v1 block"))?)?;
            (second, second_at + 44, 8)
        } else {
            (first, 44, 4)
        };
        let end = body + head.data_len(time_size);
        let block = data.get(body..end).ok_or(TzError::Format("truncated data block"))?;
        if head.typecnt == 0 {
            return Err(TzError::Format("no local time types"));
        }
        let mut at = 0;
        let mut transitions = Vec::with_capacity(head.timecnt);
        for _ in 0..head.timecnt {
            transitions.push(if time_size == 8 {
                i64::from_be_bytes(block[at..at + 8].try_into().expect("8 bytes"))
            } else {
                i64::from(i32::from_be_bytes(block[at..at + 4].try_into().expect("4 bytes")))
            });
            at += time_size;
        }
        let kinds = block[at..at + head.timecnt].to_vec();
        at += head.timecnt;
        let mut offsets = Vec::with_capacity(head.typecnt);
        for _ in 0..head.typecnt {
            offsets.push(i32::from_be_bytes(block[at..at + 4].try_into().expect("4 bytes")));
            at += 6;
        }
        if kinds.iter().any(|kind| usize::from(*kind) >= head.typecnt) {
            return Err(TzError::Format("transition type out of range"));
        }
        if transitions.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(TzError::Format("transitions out of order"));
        }
        let footer = if time_size == 8 { parse_footer(&data[end..])? } else { None };
        Ok(Self {
            transitions,
            kinds,
            offsets,
            footer,
        })
    }

    /// Seconds east of UTC at `unix_s`.
    pub fn offset_at(&self, unix_s: i64) -> i32 {
        let n = self.transitions.partition_point(|at| *at <= unix_s);
        if n == self.transitions.len() {
            if let Some(footer) = &self.footer {
                return footer.offset_at(unix_s);
            }
        }
        if n == 0 {
            return self.offsets[0];
        }
        self.offsets[usize::from(self.kinds[n - 1])]
    }
}

fn plain_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
        })
}

struct Header {
    version: u8,
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

impl Header {
    fn read(data: &[u8]) -> Result<Self, TzError> {
        if data.len() < 44 || &data[..4] != b"TZif" {
            return Err(TzError::Format("missing TZif header"));
        }
        let count = |i: usize| u32::from_be_bytes(data[20 + 4 * i..24 + 4 * i].try_into().expect("4 bytes")) as usize;
        Ok(Self {
            version: data[4],
            isutcnt: count(0),
            isstdcnt: count(1),
            leapcnt: count(2),
            timecnt: count(3),
            typecnt: count(4),
            charcnt: count(5),
        })
    }

    fn data_len(&self, time_size: usize) -> usize {
        self.timecnt * time_size
            + self.timecnt
            + self.typecnt * 6
            + self.charcnt
            + self.leapcnt * (time_size + 4)
            + self.isstdcnt
            + self.isutcnt
    }
}

fn parse_footer(rest: &[u8]) -> Result<Option<PosixTz>, TzError> {
    let text = std::str::from_utf8(rest).map_err(|_| TzError::Format("footer is not text"))?;
    let Some(inner) = text.strip_prefix('\n') else {
        return Ok(None);
    };
    let tz = inner.split('\n').next().unwrap_or("");
    if tz.is_empty() {
        return Ok(None);
    }
    PosixTz::parse(tz).map(Some).ok_or(TzError::Format("unsupported TZ footer"))
}

/// A POSIX TZ string: `std offset [dst [offset] ,start[/time],end[/time]]`.
#[derive(Clone, Debug, PartialEq)]
struct PosixTz {
    /// Seconds east of UTC (POSIX writes the opposite sign).
    std_offset: i32,
    dst: Option<Dst>,
}

#[derive(Clone, Debug, PartialEq)]
struct Dst {
    offset: i32,
    start: RuleDate,
    /// Local standard time of day, in seconds; may exceed a day or be negative.
    start_time: i32,
    end: RuleDate,
    /// Local daylight time of day, in seconds.
    end_time: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RuleDate {
    /// `Mm.w.d`: weekday `d` (0 = Sunday) of week `w` (5 = last) of month `m`.
    MonthWeekDay { month: u32, week: u32, weekday: u32 },
    /// `Jn`: day 1..=365, never counting 29 February.
    JulianNoLeap(u32),
    /// `n`: day 0..=365, counting 29 February.
    ZeroBased(u32),
}

impl RuleDate {
    /// The rule's date in `year`, as days since 1970-01-01.
    fn day(self, year: i64) -> i64 {
        match self {
            RuleDate::MonthWeekDay { month, week, weekday: wanted } => {
                let first = days_from_civil(year, month, 1);
                let offset = (i64::from(wanted) - i64::from(weekday(first))).rem_euclid(7);
                let mut day = 1 + offset + 7 * (i64::from(week) - 1);
                while day > month_len(year, month) {
                    day -= 7;
                }
                first + day - 1
            }
            RuleDate::JulianNoLeap(n) => {
                let n = i64::from(n);
                days_from_civil(year, 1, 1) + n - 1 + i64::from(is_leap(year) && n >= 60)
            }
            RuleDate::ZeroBased(n) => days_from_civil(year, 1, 1) + i64::from(n),
        }
    }
}

impl PosixTz {
    fn parse(text: &str) -> Option<Self> {
        let mut c = Cursor { s: text.as_bytes(), i: 0 };
        c.name()?;
        let std_offset = -c.time(24)?;
        if c.done() {
            return Some(Self { std_offset, dst: None });
        }
        c.name()?;
        let offset = if c.peek() == Some(b',') { std_offset + 3_600 } else { -c.time(24)? };
        if !c.eat(b',') {
            return None;
        }
        let (start, start_time) = c.rule()?;
        if !c.eat(b',') {
            return None;
        }
        let (end, end_time) = c.rule()?;
        c.done().then_some(Self {
            std_offset,
            dst: Some(Dst {
                offset,
                start,
                start_time,
                end,
                end_time,
            }),
        })
    }

    fn offset_at(&self, unix_s: i64) -> i32 {
        let Some(dst) = &self.dst else {
            return self.std_offset;
        };
        let (year, _, _) = civil_from_days((unix_s + i64::from(self.std_offset)).div_euclid(86_400));
        let start = dst.start.day(year) * 86_400 + i64::from(dst.start_time) - i64::from(self.std_offset);
        let end = dst.end.day(year) * 86_400 + i64::from(dst.end_time) - i64::from(dst.offset);
        let in_dst = if start < end {
            start <= unix_s && unix_s < end
        } else {
            !(end <= unix_s && unix_s < start)
        };
        if in_dst {
            dst.offset
        } else {
            self.std_offset
        }
    }
}

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.i += 1;
        }
        found
    }

    fn done(&self) -> bool {
        self.i == self.s.len()
    }

    /// One to three digits, at most `max`.
    fn number(&mut self, max: u32) -> Option<u32> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == start || self.i - start > 3 {
            return None;
        }
        let n: u32 = std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()?;
        (n <= max).then_some(n)
    }

    /// A zone abbreviation: three or more letters, or anything between < and >.
    fn name(&mut self) -> Option<()> {
        if self.eat(b'<') {
            while let Some(c) = self.peek() {
                self.i += 1;
                if c == b'>' {
                    return Some(());
                }
            }
            return None;
        }
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        (self.i - start >= 3).then_some(())
    }

    /// `[+-]hh[:mm[:ss]]` in seconds. `max_hours` is 24 for offsets and 167 for rule times.
    fn time(&mut self, max_hours: u32) -> Option<i32> {
        let sign = if self.eat(b'-') {
            -1
        } else {
            self.eat(b'+');
            1
        };
        let mut seconds = self.number(max_hours)? as i32 * 3_600;
        if self.eat(b':') {
            seconds += self.number(59)? as i32 * 60;
            if self.eat(b':') {
                seconds += self.number(59)? as i32;
            }
        }
        Some(sign * seconds)
    }

    fn rule(&mut self) -> Option<(RuleDate, i32)> {
        let date = if self.eat(b'M') {
            let month = self.number(12).filter(|m| *m >= 1)?;
            if !self.eat(b'.') {
                return None;
            }
            let week = self.number(5).filter(|w| *w >= 1)?;
            if !self.eat(b'.') {
                return None;
            }
            RuleDate::MonthWeekDay {
                month,
                week,
                weekday: self.number(6)?,
            }
        } else if self.eat(b'J') {
            RuleDate::JulianNoLeap(self.number(365).filter(|n| *n >= 1)?)
        } else {
            RuleDate::ZeroBased(self.number(365)?)
        };
        let time = if self.eat(b'/') { self.time(167)? } else { 7_200 };
        Some((date, time))
    }
}
```

- [ ] **Step 7: Run and watch them pass**

Run: `cargo test -p waved-system --lib civil tz`
Expected: `test result: ok. 9 passed` (3 civil, 6 tz).

- [ ] **Step 8: Lint.** Run `cargo clippy -p waved-system --all-targets -- -D warnings`; expect no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/waved-system
git commit -m "feat(system): time zone offsets from the device's zoneinfo files

- parses TZif v1 and v2 and the POSIX TZ footer slim files rely on
- Mm.w.d, Jn and n rules, rule times past midnight, southern hemisphere DST
- refuses zone names that could escape the zoneinfo directory

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The RTC on i2c-3 and the system clock (E3)

**Facts used.** The stock firmware (`libins_entry.so.c`, `rtc_init` / `rtc_gettime` / `rtc_settime`) does the following:
- opens `/dev/i2c-3`;
- selects slave `0x68` with `ioctl(I2C_SLAVE = 0x0703)`;
- reads 7 BCD registers from register 0: seconds, minutes, hours, weekday (0 = Sunday), day, month, year since 2000;
- treats them as UTC (`timegm`).

Writes use the same layout. waved keeps that layout, so a unit moved back to stock firmware keeps the right time.

**Files:**
- Create: `crates/waved-system/src/rtc.rs` (tests in `mod tests`)
- Modify: `crates/waved-system/src/lib.rs` (add `pub mod rtc;`)

**Interfaces:**
- Consumes:
  - `crate::civil::{civil_from_days, days_from_civil, month_len, weekday}`
  - `waved_state::CLOCK_VALID_AFTER_UNIX_S` (CR2)
- Produces:
  - `pub const RTC_BUS: &str = "/dev/i2c-3"` and `pub const RTC_ADDRESS: u16 = 0x68`
  - `pub fn bcd(u8) -> Option<u8>` and `pub fn to_bcd(u8) -> u8`
  - `pub fn decode(regs: [u8; 7]) -> Option<i64>` and `pub fn encode(unix_s: i64) -> Option<[u8; 7]>`
  - `pub trait Rtc: Send { fn read(&mut self) -> io::Result<Option<i64>>; fn write(&mut self, unix_s: i64) -> io::Result<()>; }`
  - `pub struct I2cRtc`, with `fn open(bus: &str, address: u16) -> io::Result<Self>`
  - `pub trait WallClock: Send + Sync { fn now_unix_s(&self) -> i64; fn set(&self, unix_s: i64) -> io::Result<()>; }`
  - `pub struct SystemClock`
  - `pub fn restore_time(system_now: i64, rtc_now: Option<i64>) -> Option<i64>`

- [ ] **Step 1: Add the module** to `lib.rs`: `pub mod rtc;`.

- [ ] **Step 2: Write the failing tests.** Create `crates/waved-system/src/rtc.rs`:

```rust
//! The battery-backed clock: a BCD chip at 0x68 on /dev/i2c-3, driven the way
//! the stock firmware drives it (registers 0..=6: seconds, minutes, hours,
//! weekday, day, month, year since 2000; UTC). waved restores the system clock
//! from it at start-up and writes it after each network time sync.

use crate::civil::{civil_from_days, days_from_civil, month_len, weekday};
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};
use waved_state::CLOCK_VALID_AFTER_UNIX_S;

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-05 14:32:07 UTC, a Monday.
    const OCT_5: i64 = 1_791_210_727;
    const OCT_5_REGS: [u8; 7] = [0x07, 0x32, 0x14, 1, 0x05, 0x10, 0x26];

    #[test]
    fn bcd_round_trips_and_rejects_non_decimal() {
        assert_eq!(bcd(0x59), Some(59));
        assert_eq!(bcd(0x00), Some(0));
        assert_eq!(bcd(0x5a), None);
        assert_eq!(bcd(0xa0), None);
        assert_eq!(to_bcd(59), 0x59);
    }

    #[test]
    fn rtc_registers_decode_like_the_stock_firmware() {
        assert_eq!(decode(OCT_5_REGS), Some(OCT_5));
        assert_eq!(encode(OCT_5), Some(OCT_5_REGS));
    }

    #[test]
    fn oscillator_and_century_bits_are_ignored() {
        let mut regs = OCT_5_REGS;
        regs[0] |= 0x80; // DS1307 clock-halt
        regs[5] |= 0x80; // DS3231 century
        assert_eq!(decode(regs), Some(OCT_5));
    }

    #[test]
    fn impossible_dates_are_not_a_time() {
        let with = |i: usize, v: u8| {
            let mut regs = OCT_5_REGS;
            regs[i] = v;
            regs
        };
        assert_eq!(decode(with(5, 0x13)), None, "month 13");
        assert_eq!(decode(with(5, 0x00)), None, "month 0");
        assert_eq!(decode([0x00, 0x00, 0x00, 0, 0x30, 0x02, 0x26]), None, "30 February");
        assert_eq!(decode(with(4, 0x00)), None, "day 0");
        assert_eq!(decode(with(2, 0x24)), None, "hour 24");
        assert_eq!(decode(with(2, 0x40 | 0x02)), None, "12-hour mode");
        assert_eq!(decode(with(1, 0x60)), None, "minute 60");
        assert_eq!(decode([0xff; 7]), None, "erased chip");
    }

    #[test]
    fn encode_refuses_times_outside_the_chip_range() {
        assert_eq!(encode(0), None, "1970");
        assert_eq!(encode(4_102_444_800), None, "2100");
        assert_eq!(decode(encode(946_684_800).unwrap()), Some(946_684_800), "2000-01-01");
    }

    #[test]
    fn the_rtc_restores_only_an_unset_system_clock() {
        assert_eq!(restore_time(0, Some(OCT_5)), Some(OCT_5));
        assert_eq!(restore_time(1_791_000_000, Some(OCT_5)), None, "a valid clock is kept");
        assert_eq!(restore_time(0, Some(100)), None, "an RTC that was never set");
        assert_eq!(restore_time(0, None), None);
    }

    #[test]
    fn the_system_clock_reads_a_believable_time_on_the_host() {
        assert!(SystemClock.now_unix_s() >= CLOCK_VALID_AFTER_UNIX_S);
    }
}
```

- [ ] **Step 3: Run and watch it fail**

Run: `cargo test -p waved-system --lib rtc`
Expected: ``cannot find function `bcd` in this scope`` and similar.

- [ ] **Step 4: Implement.** Insert before `#[cfg(test)]`:

```rust
pub const RTC_BUS: &str = "/dev/i2c-3";
pub const RTC_ADDRESS: u16 = 0x68;
/// `I2C_SLAVE` from linux/i2c-dev.h.
const I2C_SLAVE: u64 = 0x0703;

pub fn bcd(value: u8) -> Option<u8> {
    let (high, low) = (value >> 4, value & 0x0f);
    (high <= 9 && low <= 9).then_some(high * 10 + low)
}

pub fn to_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

/// Registers 0..=6 → Unix seconds. `None` for anything that is not a real date.
pub fn decode(regs: [u8; 7]) -> Option<i64> {
    if regs[2] & 0x40 != 0 {
        return None; // 12-hour mode, which the stock firmware never writes
    }
    let second = bcd(regs[0] & 0x7f)?; // bit 7: oscillator halt (DS1307-style)
    let minute = bcd(regs[1] & 0x7f)?;
    let hour = bcd(regs[2] & 0x3f)?;
    let day = bcd(regs[4] & 0x3f)?;
    let month = bcd(regs[5] & 0x1f)?; // bit 7: century (DS3231-style)
    let year = 2000 + i64::from(bcd(regs[6])?);
    if second > 59 || minute > 59 || hour > 23 || !(1..=12).contains(&month) {
        return None;
    }
    if day == 0 || i64::from(day) > month_len(year, u32::from(month)) {
        return None;
    }
    let days = days_from_civil(year, u32::from(month), u32::from(day));
    Some(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
}

/// Unix seconds → registers 0..=6, for 2000..=2099 only.
pub fn encode(unix_s: i64) -> Option<[u8; 7]> {
    let days = unix_s.div_euclid(86_400);
    let secs = unix_s.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(2000..=2099).contains(&year) {
        return None;
    }
    Some([
        to_bcd((secs % 60) as u8),
        to_bcd((secs / 60 % 60) as u8),
        to_bcd((secs / 3_600) as u8),
        weekday(days) as u8,
        to_bcd(day as u8),
        to_bcd(month as u8),
        to_bcd((year - 2000) as u8),
    ])
}

pub trait Rtc: Send {
    /// The chip's time, or `None` when it holds no valid date.
    fn read(&mut self) -> io::Result<Option<i64>>;
    fn write(&mut self, unix_s: i64) -> io::Result<()>;
}

pub struct I2cRtc {
    file: std::fs::File,
}

impl I2cRtc {
    pub fn open(bus: &str, address: u16) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        let file = std::fs::OpenOptions::new().read(true).write(true).open(bus)?;
        // SAFETY: I2C_SLAVE takes the 7-bit slave address as its integer argument.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), I2C_SLAVE as _, libc::c_ulong::from(address)) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }
}

impl Rtc for I2cRtc {
    fn read(&mut self) -> io::Result<Option<i64>> {
        use std::io::{Read, Write};
        self.file.write_all(&[0])?;
        let mut regs = [0u8; 7];
        self.file.read_exact(&mut regs)?;
        Ok(decode(regs))
    }

    fn write(&mut self, unix_s: i64) -> io::Result<()> {
        use std::io::Write;
        let regs = encode(unix_s)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "time outside the RTC's 2000..2099 range"))?;
        let mut frame = [0u8; 8]; // register address 0, then the seven registers
        frame[1..].copy_from_slice(&regs);
        self.file.write_all(&frame)
    }
}

/// The system wall clock, behind a trait so tests can replace it.
pub trait WallClock: Send + Sync {
    fn now_unix_s(&self) -> i64;
    fn set(&self, unix_s: i64) -> io::Result<()>;
}

pub struct SystemClock;

impl WallClock for SystemClock {
    fn now_unix_s(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0)
    }

    #[allow(clippy::unnecessary_cast)] // time_t is i64 on the device but not on every host
    fn set(&self, unix_s: i64) -> io::Result<()> {
        let time = libc::timeval {
            tv_sec: unix_s as libc::time_t,
            tv_usec: 0,
        };
        // SAFETY: `time` is a valid timeval for the call; a null time zone is allowed.
        if unsafe { libc::settimeofday(&time, std::ptr::null()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// What to set the system clock to at start-up: the RTC's time, when the
/// system clock is not believable yet and the RTC's is.
pub fn restore_time(system_now: i64, rtc_now: Option<i64>) -> Option<i64> {
    if system_now >= CLOCK_VALID_AFTER_UNIX_S {
        return None;
    }
    rtc_now.filter(|time| *time >= CLOCK_VALID_AFTER_UNIX_S)
}
```

- [ ] **Step 5: Run and watch it pass**

Run: `cargo test -p waved-system --lib rtc`
Expected: `test result: ok. 7 passed`.

- [ ] **Step 6: Lint.** Run `cargo clippy -p waved-system --all-targets -- -D warnings`; expect no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-system
git commit -m "feat(system): read and write the rtc on i2c-3 like the stock firmware

- BCD registers 0..6 at slave 0x68, UTC, weekday 0 = Sunday
- invalid BCD, impossible dates, 12-hour mode and an erased chip are not a time
- the system clock is restored from the RTC only while it is still before 2020

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: About information and the build id (E7)

**Files:**
- Create: `crates/waved-system/build.rs`
- Create: `crates/waved-system/src/about.rs` (tests in `mod tests`)
- Modify: `crates/waved-system/src/lib.rs` (add `pub mod about;`)

**Interfaces:**
- Consumes:
  - `waved_state::AboutInfo` (CR2)
  - `nix::sys::statvfs`
- Produces:
  - `pub const SERIAL_SOURCES: &[&str]` and `pub const LICENCES: &[&str]`
  - `pub fn version() -> String`
  - `pub fn read_serial(sources: &[PathBuf]) -> String`
  - `pub fn storage(path: &Path) -> Option<(u64, u64)>` (free, total)
  - `pub fn collect(serial_sources: &[PathBuf], storage_path: &Path) -> AboutInfo`

- [ ] **Step 1: Create** `crates/waved-system/build.rs`:

```rust
//! Sets WAVED_BUILD_ID to the short git commit, or to "" when git is missing
//! (for example inside the cross build container).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

fn main() {
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        // Commits, checkouts and resets all append to logs/HEAD.
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
    let id = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=WAVED_BUILD_ID={id}");
}
```

- [ ] **Step 2: Add the module** to `lib.rs`: `pub mod about;`.

- [ ] **Step 3: Write the failing tests.** Create `crates/waved-system/src/about.rs`:

```rust
//! What About shows (TODO E7): version, serial, storage and licences. The
//! device name and IP come from config and Wi-Fi state, not from here.

use std::path::{Path, PathBuf};
use waved_state::AboutInfo;

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
```

- [ ] **Step 4: Run and watch it fail**

Run: `cargo test -p waved-system --lib about`
Expected: ``cannot find function `version` in this scope`` and similar.

- [ ] **Step 5: Implement.** Insert before `#[cfg(test)]`:

```rust
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
```

- [ ] **Step 6: Run and watch it pass**

Run: `cargo test -p waved-system --lib about`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 7: Lint.** Run `cargo clippy -p waved-system --all-targets -- -D warnings`; expect no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/waved-system
git commit -m "feat(system): collect about information with a git build id

- version reads 0.1.0 (<short sha>) when git is available at build time
- serial from the first readable source; storage free and total via statvfs
- licence lines for waved, LVGL, Barlow and Tabler Icons

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: SystemService — offset every minute, About, RTC restore and save (E3, E7)

**Files:**
- Create: `crates/waved-system/src/service.rs` (tests in `mod tests`)
- Modify: `crates/waved-system/src/lib.rs` (add `mod service;` and `pub use service::SystemService;`)

**Interfaces:**
- Consumes:
  - `waved_state::{SettingsState, SystemState, WifiState}` (CR2)
  - `crate::tz::{TimeZone, ZONEINFO_DIR}`
  - `crate::rtc::{Rtc, I2cRtc, WallClock, SystemClock, restore_time, RTC_BUS, RTC_ADDRESS}`
  - `crate::about::{collect, storage, SERIAL_SOURCES}`
- Produces:
  - `pub struct SystemService`, with:
    - `fn new(system_tx: Arc<watch::Sender<SystemState>>, storage_path: impl Into<PathBuf>) -> Self` (no RTC, `SystemClock`, `ZONEINFO_DIR`, `SERIAL_SOURCES`)
    - `fn device(system_tx: Arc<watch::Sender<SystemState>>, storage_path: impl Into<PathBuf>) -> Self` (also opens the RTC)
    - `fn with_zoneinfo(self, dir: impl Into<PathBuf>) -> Self`
    - `fn with_serial_sources(self, sources: Vec<PathBuf>) -> Self`
    - `fn with_clock(self, clock: Arc<dyn WallClock>) -> Self`
    - `fn with_rtc(self, rtc: Option<Box<dyn Rtc>>) -> Self`
    - `async fn run(self, settings: watch::Receiver<SettingsState>, wifi: watch::Receiver<WifiState>)`, which runs until either watch sender is dropped

- [ ] **Step 1: Add the module** to `lib.rs`: `mod service;` plus `pub use service::SystemService;`.

- [ ] **Step 2: Write the failing tests.** Create `crates/waved-system/src/service.rs`:

```rust
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
```

- [ ] **Step 3: Run and watch it fail**

Run: `cargo test -p waved-system --lib service`
Expected: ``cannot find type `SystemService` in this scope``.

- [ ] **Step 4: Implement.** Insert before `#[cfg(test)]`:

```rust
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
```

- [ ] **Step 5: Run and watch it pass**

Run: `cargo test -p waved-system --lib service`
Expected: `test result: ok. 7 passed`.

- [ ] **Step 6: Lint.** Run `cargo clippy -p waved-system --all-targets -- -D warnings`; expect no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-system
git commit -m "feat(system): publish the zone offset and about, keep the rtc in step

- recomputes the offset at every minute boundary and on a zone change; unknown zones show UTC
- restores an unset system clock from the RTC at start-up
- writes the RTC each time network time arrives
- refreshes storage free and total every minute

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Alert tones (E5)

**Design.** The tones are synthesised: a two-note chime, E5 (659.25 Hz) for 110 ms and A5 (880 Hz) for 160 ms, rising for start and falling for stop, peak 0.4 of full scale, with 8 ms raised-cosine fades.

The stock `airecord.wav` was not used, for three reasons:
- there is no stock stop tone;
- decoding and resampling a WAV is more code than a sine;
- the tones stay deterministic in tests.

The trigger is the `MeetingState::recording` flag on the meeting watch, not `DeviceEvent::RecordingStarted`, which `waved-meeting` never sends. A start blocked by storage or integrity faults never chimes. Pausing is not a change.

Playback goes through `ToneSink`. The integrator's adapter (IR2) submits periods to `PlaybackService::play_batch`, paced one period at a time, the same way `main.rs`'s SIGUSR2 handler does.

**Files:**
- Create: `crates/waved-system/src/tones.rs` (tests in `mod tests`)
- Modify: `crates/waved-system/src/lib.rs` (add `pub mod tones;`)

**Interfaces:**
- Consumes:
  - `waved_state::{MeetingState, SettingsState}`
  - `tokio::sync::watch`
- Produces:
  - `pub enum Tone { RecordingStarted, RecordingStopped }`, with `fn notes(self) -> [(f32, u32); 2]`
  - `pub struct PcmFormat { pub sample_rate: u32, pub channels: usize, pub period_frames: usize }`
  - `pub trait ToneSink: Send + Sync { fn format(&self) -> PcmFormat; fn submit(&self, period: Vec<i16>) -> bool; }`
  - `pub const TONE_PEAK: f32 = 0.4`
  - `pub fn render(tone: Tone, format: PcmFormat) -> Vec<i16>`
  - `pub fn tone_for(was_recording: bool, recording: bool) -> Option<Tone>`
  - `pub async fn run(meeting: watch::Receiver<MeetingState>, settings: watch::Receiver<SettingsState>, sink: Arc<dyn ToneSink>)`

- [ ] **Step 1: Add the module** to `lib.rs`: `pub mod tones;`.

- [ ] **Step 2: Write the failing tests.** Create `crates/waved-system/src/tones.rs`:

```rust
//! Recording start and stop tones (TODO E5), so everyone in the room hears
//! when recording begins and ends. Synthesised here and played through a
//! `ToneSink`; the integrator's sink feeds waved-audio's playback path.

use std::f32::consts::PI;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::debug;
use waved_state::{MeetingState, SettingsState};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const FORMAT: PcmFormat = PcmFormat {
        sample_rate: 48_000,
        channels: 2,
        period_frames: 480,
    };

    fn crossings(samples: &[i16]) -> usize {
        samples.windows(2).filter(|pair| (pair[0] < 0) != (pair[1] < 0)).count()
    }

    fn left(samples: &[i16]) -> Vec<i16> {
        samples.iter().step_by(2).copied().collect()
    }

    #[test]
    fn start_rises_and_stop_falls() {
        let (first, second) = (5_280, 5_280 + 7_680); // 110 ms and 160 ms at 48 kHz
        let rate = |samples: &[i16], from: usize, to: usize| crossings(&samples[from..to]) * 1_000 / (to - from);
        let start = left(&render(Tone::RecordingStarted, FORMAT));
        assert!(rate(&start, 0, first) < rate(&start, first, second));
        let stop = left(&render(Tone::RecordingStopped, FORMAT));
        assert!(rate(&stop, 0, first) > rate(&stop, first, second));
    }

    #[test]
    fn tones_stay_under_the_peak_start_silent_and_fill_whole_periods() {
        let samples = render(Tone::RecordingStarted, FORMAT);
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(f32::from(peak) <= TONE_PEAK * f32::from(i16::MAX) + 1.0, "{peak}");
        assert!(peak > 10_000, "audible: {peak}");
        assert_eq!(samples[0], 0);
        assert_eq!(samples.len() % (480 * 2), 0);
        assert_eq!(samples.len(), 27 * 480 * 2, "270 ms is 27 periods of 10 ms");
        assert!(samples.chunks(2).all(|frame| frame[0] == frame[1]), "both channels carry the tone");
    }

    #[test]
    fn only_a_change_of_the_recording_flag_plays_a_tone() {
        assert_eq!(tone_for(false, true), Some(Tone::RecordingStarted));
        assert_eq!(tone_for(true, false), Some(Tone::RecordingStopped));
        assert_eq!(tone_for(true, true), None);
        assert_eq!(tone_for(false, false), None);
    }

    struct FakeSink {
        periods: Mutex<Vec<Vec<i16>>>,
        accept: bool,
    }

    impl ToneSink for FakeSink {
        fn format(&self) -> PcmFormat {
            FORMAT
        }
        fn submit(&self, period: Vec<i16>) -> bool {
            self.periods.lock().unwrap().push(period);
            self.accept
        }
    }

    fn sink(accept: bool) -> Arc<FakeSink> {
        Arc::new(FakeSink {
            periods: Mutex::default(),
            accept,
        })
    }

    fn settle() -> tokio::time::Sleep {
        tokio::time::sleep(Duration::from_secs(1))
    }

    #[tokio::test(start_paused = true)]
    async fn the_service_plays_start_and_stop_and_honours_the_setting() {
        let sink = sink(true);
        let (meeting, meeting_rx) = watch::channel(MeetingState::default());
        let (settings, settings_rx) = watch::channel(SettingsState::default());
        tokio::spawn(run(meeting_rx, settings_rx, sink.clone()));
        meeting.send_modify(|m| m.recording = true);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().concat(), render(Tone::RecordingStarted, FORMAT));
        meeting.send_modify(|m| m.recording = false);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().len(), 2 * 27);
        settings.send_modify(|s| s.tones_enabled = false);
        meeting.send_modify(|m| m.recording = true);
        settle().await;
        meeting.send_modify(|m| m.recording = false);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().len(), 2 * 27, "tones off: silent");
    }

    #[tokio::test(start_paused = true)]
    async fn pausing_plays_nothing() {
        let sink = sink(true);
        let (meeting, meeting_rx) = watch::channel(MeetingState::default());
        let (_settings, settings_rx) = watch::channel(SettingsState::default());
        tokio::spawn(run(meeting_rx, settings_rx, sink.clone()));
        meeting.send_modify(|m| m.recording = true);
        settle().await;
        meeting.send_modify(|m| m.paused = true);
        settle().await;
        meeting.send_modify(|m| m.paused = false);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().len(), 27);
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_queue_cuts_the_tone_short() {
        let sink = sink(false);
        let (meeting, meeting_rx) = watch::channel(MeetingState::default());
        let (_settings, settings_rx) = watch::channel(SettingsState::default());
        tokio::spawn(run(meeting_rx, settings_rx, sink.clone()));
        meeting.send_modify(|m| m.recording = true);
        settle().await;
        assert_eq!(sink.periods.lock().unwrap().len(), 1);
    }
}
```

- [ ] **Step 3: Run and watch it fail**

Run: `cargo test -p waved-system --lib tones`
Expected: ``cannot find type `PcmFormat` in this scope`` and similar.

- [ ] **Step 4: Implement.** Insert before `#[cfg(test)]`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    RecordingStarted,
    RecordingStopped,
}

/// The playback path's format: interleaved i16 at `sample_rate`, submitted
/// one period of `period_frames` frames at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: usize,
    pub period_frames: usize,
}

pub trait ToneSink: Send + Sync {
    fn format(&self) -> PcmFormat;
    /// Queue one period. False when playback stopped or its queue is full.
    fn submit(&self, period: Vec<i16>) -> bool;
}

/// Peak level as a fraction of full scale (about −8 dBFS before the speaker volume).
pub const TONE_PEAK: f32 = 0.4;
const FADE_MS: u64 = 8;
const LOW_HZ: f32 = 659.25; // E5
const HIGH_HZ: f32 = 880.0; // A5

impl Tone {
    /// (frequency in Hz, length in ms): rising for start, falling for stop.
    pub fn notes(self) -> [(f32, u32); 2] {
        match self {
            Tone::RecordingStarted => [(LOW_HZ, 110), (HIGH_HZ, 160)],
            Tone::RecordingStopped => [(HIGH_HZ, 110), (LOW_HZ, 160)],
        }
    }
}

/// Interleaved samples, every channel the same, padded with silence to whole periods.
pub fn render(tone: Tone, format: PcmFormat) -> Vec<i16> {
    let rate = u64::from(format.sample_rate.max(1));
    let mut mono = Vec::new();
    for (hz, ms) in tone.notes() {
        let len = (rate * u64::from(ms) / 1_000) as usize;
        let fade = ((rate * FADE_MS / 1_000) as usize).max(1);
        for i in 0..len {
            let edge = i.min(len - 1 - i);
            let gain = if edge < fade {
                0.5 - 0.5 * (PI * edge as f32 / fade as f32).cos()
            } else {
                1.0
            };
            let phase = 2.0 * PI * hz * i as f32 / rate as f32;
            mono.push((phase.sin() * gain * TONE_PEAK * f32::from(i16::MAX)) as i16);
        }
    }
    let period = format.period_frames.max(1);
    let channels = format.channels.max(1);
    let frames = mono.len().div_ceil(period) * period;
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        out.extend(std::iter::repeat_n(mono.get(i).copied().unwrap_or(0), channels));
    }
    out
}

/// The tone a change of the recording flag calls for. Pausing is not a change.
pub fn tone_for(was_recording: bool, recording: bool) -> Option<Tone> {
    match (was_recording, recording) {
        (false, true) => Some(Tone::RecordingStarted),
        (true, false) => Some(Tone::RecordingStopped),
        _ => None,
    }
}

/// Plays the start and stop tones while `tones_enabled`. Runs until the
/// meeting sender is dropped.
pub async fn run(
    mut meeting: watch::Receiver<MeetingState>,
    settings: watch::Receiver<SettingsState>,
    sink: Arc<dyn ToneSink>,
) {
    let mut was_recording = meeting.borrow_and_update().recording;
    while meeting.changed().await.is_ok() {
        let recording = meeting.borrow_and_update().recording;
        let Some(tone) = tone_for(was_recording, recording) else {
            continue;
        };
        was_recording = recording;
        if settings.borrow().tones_enabled {
            play(tone, sink.as_ref()).await;
        }
    }
}

async fn play(tone: Tone, sink: &dyn ToneSink) {
    let format = sink.format();
    let samples = render(tone, format);
    let period = format.period_frames.max(1) * format.channels.max(1);
    let pace = Duration::from_micros(format.period_frames as u64 * 1_000_000 / u64::from(format.sample_rate.max(1)));
    for chunk in samples.chunks(period) {
        if !sink.submit(chunk.to_vec()) {
            debug!("{tone:?} tone cut short: playback stopped or its queue is full");
            return;
        }
        tokio::time::sleep(pace).await;
    }
}
```

- [ ] **Step 5: Run and watch it pass**

Run: `cargo test -p waved-system --lib tones`
Expected: `test result: ok. 6 passed`.

- [ ] **Step 6: Lint.** Run `cargo clippy -p waved-system --all-targets -- -D warnings`; expect no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-system
git commit -m "feat(system): recording start and stop tones

- rising chime on start, falling on stop, -8 dBFS with raised-cosine fades
- driven by the recording flag, so blocked starts and pauses stay silent
- honours the alert tones setting; plays through a ToneSink the integrator wires to playback

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Factory reset (E6)

**Design.** `Command::FactoryReset` writes a marker next to `config.toml`, then sends `Command::RestartForConfig`; waved exits and `S60waved` respawns it within about 3 s.

At start-up, `complete_pending` runs before the config loads. It takes these steps:

1. **Count the attempt.** It increments the attempt count in the marker.
2. **Erase.**
   - `config.toml*` (config, `.bak`, `.unloadable*`, temp files)
   - `meetings.db*` (database, `-wal`, `-shm`, `-journal`)
   - everything inside the recordings directory, but only while `/mnt/udisk` is a mount point
   - `/etc/wpa_supplicant.conf`, rewritten to the stock header with no networks; then `wpa_cli -i wlan0 reconfigure` drops the running station's networks
3. **Finish.** It removes the marker only when everything succeeded, or after 3 attempts. Then a persistent failure never erases the settings a user makes after the reset on every boot.

Siblings are matched by file-name prefix only, inside the config and database directories. The waved binary in `/data/waved` survives. The next start loads defaults with `first_run_done = false`, so first start shows.

**Files:**
- Create: `crates/waved-system/src/reset.rs` (tests in `mod tests`)
- Modify: `crates/waved-system/src/lib.rs` (add `pub mod reset;`)

**Interfaces:**
- Consumes:
  - `waved_state::Command::{FactoryReset, RestartForConfig}`
  - `tokio::sync::broadcast`
- Produces:
  - constants `MARKER_NAME`, `MAX_ATTEMPTS = 3`, `WIFI_CONF`, `RECORDINGS_MOUNT`, `CLEAN_WPA_CONF`
  - `pub struct ResetPaths { pub marker, pub config, pub database, pub recordings: PathBuf, pub recordings_mount: Option<PathBuf>, pub wifi_conf: Option<PathBuf>, pub reload_wifi: bool }`, with:
    - `fn new(config, database, recordings) -> Self`
    - `fn on_device(self) -> Self`
  - `pub enum ResetOutcome { NothingPending, Completed, Retrying { attempt: u32 }, GaveUp { attempt: u32 } }`
  - `pub fn request(paths: &ResetPaths) -> io::Result<()>`
  - `pub fn complete_pending(paths: &ResetPaths) -> ResetOutcome`
  - `pub fn is_mount_point(path: &Path) -> bool`
  - `pub async fn run(paths: ResetPaths, commands: broadcast::Receiver<Command>, bus: broadcast::Sender<Command>)`

- [ ] **Step 1: Add the module** to `lib.rs`: `pub mod reset;`.

- [ ] **Step 2: Write the failing tests.** Create `crates/waved-system/src/reset.rs`:

```rust
//! Factory reset (TODO E6): erase config, the meetings database, recordings
//! and saved Wi-Fi networks. The request only leaves a marker and restarts
//! waved; the erase runs at the next start-up, before anything opens these
//! files, and repeats after a power cut.

use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use waved_state::Command;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Device {
        dir: tempfile::TempDir,
        paths: ResetPaths,
    }

    fn device() -> Device {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data/waved");
        let recordings = dir.path().join("udisk/recordings");
        let etc = dir.path().join("etc");
        for path in [data.clone(), recordings.join("2026-10-05_0930"), etc.clone()] {
            std::fs::create_dir_all(path).unwrap();
        }
        for (path, body) in [
            (data.join("config.toml"), "config_version = 2\n"),
            (data.join("config.toml.bak"), "old"),
            (data.join("config.toml.unloadable.2"), "bad"),
            (data.join("meetings.db"), "db"),
            (data.join("meetings.db-wal"), "wal"),
            (data.join("waved"), "binary"),
            (data.join("playback.s16le"), "pcm"),
            (recordings.join("2026-10-05_0930/audio.wav"), "RIFF"),
            (recordings.join("stray.tmp"), "tmp"),
            (etc.join("wpa_supplicant.conf"), "network={\n ssid=\"office\"\n}\n"),
        ] {
            std::fs::write(path, body).unwrap();
        }
        let mut paths = ResetPaths::new(data.join("config.toml"), data.join("meetings.db"), recordings);
        paths.wifi_conf = Some(etc.join("wpa_supplicant.conf"));
        Device { dir, paths }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_next_start_erases_config_database_recordings_and_wifi() {
        let d = device();
        let data = d.paths.config.parent().unwrap().to_path_buf();
        request(&d.paths).unwrap();
        assert!(d.paths.config.exists(), "a request erases nothing by itself");
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Completed);
        assert_eq!(names(&data), ["playback.s16le", "waved"], "the binary survives");
        assert!(d.paths.recordings.is_dir());
        assert!(names(&d.paths.recordings).is_empty());
        let conf = d.paths.wifi_conf.as_ref().unwrap();
        assert_eq!(std::fs::read_to_string(conf).unwrap(), CLEAN_WPA_CONF);
        assert_eq!(std::fs::metadata(conf).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(complete_pending(&d.paths), ResetOutcome::NothingPending);
    }

    #[test]
    fn nothing_pending_touches_nothing() {
        let d = device();
        assert_eq!(complete_pending(&d.paths), ResetOutcome::NothingPending);
        assert!(d.paths.config.exists() && d.paths.database.exists());
    }

    #[test]
    fn an_unmounted_recordings_volume_is_retried_then_given_up() {
        let mut d = device();
        // Same filesystem as its parent, so not a mount point.
        d.paths.recordings_mount = Some(d.dir.path().join("udisk"));
        request(&d.paths).unwrap();
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Retrying { attempt: 1 });
        assert!(!d.paths.config.exists(), "the other steps still ran");
        assert!(d.paths.marker.exists());
        assert!(d.paths.recordings.join("stray.tmp").exists(), "an unmounted volume is never erased");
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Retrying { attempt: 2 });
        assert_eq!(complete_pending(&d.paths), ResetOutcome::GaveUp { attempt: 3 });
        assert!(!d.paths.marker.exists(), "no erase loop on every boot");
    }

    #[test]
    fn a_power_cut_mid_reset_repeats_it() {
        let d = device();
        std::fs::write(&d.paths.marker, "attempts=1\n").unwrap(); // cut after the first count
        assert_eq!(complete_pending(&d.paths), ResetOutcome::Completed);
        assert!(!d.paths.config.exists() && !d.paths.marker.exists());
    }

    #[test]
    fn plain_directories_are_not_mount_points() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(!is_mount_point(&dir.path().join("sub")));
        assert!(!is_mount_point(&dir.path().join("missing")));
        assert!(!is_mount_point(Path::new("/")));
    }

    #[tokio::test]
    async fn the_handler_restarts_waved_after_a_reset_request() {
        let d = device();
        let (bus, rx) = broadcast::channel(8);
        let mut watcher = bus.subscribe();
        tokio::spawn(run(d.paths.clone(), rx, bus.clone()));
        bus.send(Command::FactoryReset).unwrap();
        loop {
            let command = tokio::time::timeout(Duration::from_secs(5), watcher.recv()).await.unwrap().unwrap();
            if matches!(command, Command::RestartForConfig) {
                break;
            }
        }
        assert!(d.paths.marker.exists());
        assert!(d.paths.config.exists(), "the erase waits for the next start");
    }
}
```

- [ ] **Step 3: Run and watch it fail**

Run: `cargo test -p waved-system --lib reset`
Expected: ``cannot find type `ResetPaths` in this scope`` and similar.

- [ ] **Step 4: Implement.** Insert before `#[cfg(test)]`:

```rust
pub const MARKER_NAME: &str = "factory-reset.pending";
/// Start-ups that try to finish a reset before it is given up.
pub const MAX_ATTEMPTS: u32 = 3;
pub const WIFI_CONF: &str = "/etc/wpa_supplicant.conf";
pub const RECORDINGS_MOUNT: &str = "/mnt/udisk";
/// The station config with no saved networks (the stock image's header).
pub const CLEAN_WPA_CONF: &str = "ctrl_interface=/var/run/wpa_supplicant\nap_scan=1\nupdate_config=1\n";

#[derive(Clone, Debug, PartialEq)]
pub struct ResetPaths {
    pub marker: PathBuf,
    /// Removed with every sibling whose name starts with its name.
    pub config: PathBuf,
    /// Removed with every sibling whose name starts with its name.
    pub database: PathBuf,
    /// Emptied; the directory itself stays.
    pub recordings: PathBuf,
    /// When set, recordings are erased only while this is a mount point.
    pub recordings_mount: Option<PathBuf>,
    /// When set, rewritten to `CLEAN_WPA_CONF`.
    pub wifi_conf: Option<PathBuf>,
    /// Run `wpa_cli -i wlan0 reconfigure` after rewriting the Wi-Fi config.
    pub reload_wifi: bool,
}

impl ResetPaths {
    pub fn new(config: impl Into<PathBuf>, database: impl Into<PathBuf>, recordings: impl Into<PathBuf>) -> Self {
        let config = config.into();
        Self {
            marker: config.with_file_name(MARKER_NAME),
            config,
            database: database.into(),
            recordings: recordings.into(),
            recordings_mount: None,
            wifi_conf: None,
            reload_wifi: false,
        }
    }

    /// The device's recordings mount check and Wi-Fi config.
    pub fn on_device(mut self) -> Self {
        self.recordings_mount = Some(PathBuf::from(RECORDINGS_MOUNT));
        self.wifi_conf = Some(PathBuf::from(WIFI_CONF));
        self.reload_wifi = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetOutcome {
    NothingPending,
    Completed,
    /// A step failed; the next start-up tries again.
    Retrying { attempt: u32 },
    /// Still failing after `MAX_ATTEMPTS`; the marker is removed.
    GaveUp { attempt: u32 },
}

/// Leave the marker for the next start-up. Nothing is erased yet.
pub fn request(paths: &ResetPaths) -> io::Result<()> {
    write_marker(&paths.marker, 0)
}

/// Finish a requested reset. Call first thing at start-up, before config.toml
/// is loaded and before any service opens the database or recordings.
pub fn complete_pending(paths: &ResetPaths) -> ResetOutcome {
    let Some(previous) = read_attempts(&paths.marker) else {
        return ResetOutcome::NothingPending;
    };
    let attempt = previous.saturating_add(1);
    if let Err(error) = write_marker(&paths.marker, attempt) {
        warn!("factory reset: could not count attempt {attempt}: {error}");
    }
    let mut ok = true;
    let mut step = |name: &str, result: io::Result<()>| {
        if let Err(error) = result {
            warn!("factory reset: {name} not erased: {error}");
            ok = false;
        }
    };
    step("config", remove_prefixed(&paths.config));
    step("database", remove_prefixed(&paths.database));
    step("recordings", erase_recordings(paths));
    if let Some(conf) = &paths.wifi_conf {
        step("Wi-Fi networks", reset_wifi(conf, paths.reload_wifi));
    }
    if ok {
        finish(paths);
        info!("factory reset done");
        ResetOutcome::Completed
    } else if attempt >= MAX_ATTEMPTS {
        finish(paths);
        error!("factory reset still incomplete after {attempt} attempts; giving up");
        ResetOutcome::GaveUp { attempt }
    } else {
        ResetOutcome::Retrying { attempt }
    }
}

/// True when `path` is on a different filesystem from its parent.
pub fn is_mount_point(path: &Path) -> bool {
    let (Ok(own), Some(parent)) = (std::fs::metadata(path), path.parent()) else {
        return false;
    };
    std::fs::metadata(parent).is_ok_and(|parent| parent.dev() != own.dev())
}

/// On `Command::FactoryReset`: leave the marker and restart waved
/// (`RestartForConfig`). The erase runs at the next start-up.
pub async fn run(paths: ResetPaths, mut commands: broadcast::Receiver<Command>, bus: broadcast::Sender<Command>) {
    loop {
        match commands.recv().await {
            Ok(Command::FactoryReset) => match request(&paths) {
                Ok(()) => {
                    info!("factory reset requested; restarting to erase");
                    let _ = bus.send(Command::RestartForConfig);
                }
                Err(error) => error!("factory reset not started: {error}"),
            },
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!("factory reset handler lagged {skipped} commands");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

fn write_marker(marker: &Path, attempts: u32) -> io::Result<()> {
    let temp = marker.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temp)?;
    writeln!(file, "attempts={attempts}")?;
    file.sync_all()?;
    std::fs::rename(&temp, marker)?;
    if let Some(dir) = marker.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

fn read_attempts(marker: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(marker).ok()?;
    Some(
        text.trim()
            .strip_prefix("attempts=")
            .and_then(|n| n.parse().ok())
            .unwrap_or(0),
    )
}

fn finish(paths: &ResetPaths) {
    if let Err(error) = std::fs::remove_file(&paths.marker) {
        warn!("factory reset: marker not removed: {error}");
    }
}

/// Remove `path` and every regular file beside it whose name starts with its
/// name (config.toml.bak, meetings.db-wal, ...).
fn remove_prefixed(path: &Path) -> io::Result<()> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(());
    };
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let name = name.to_string_lossy();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(name.as_ref()) && entry.file_type()?.is_file() {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn erase_recordings(paths: &ResetPaths) -> io::Result<()> {
    if let Some(mount) = &paths.recordings_mount {
        if !is_mount_point(mount) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not mounted", mount.display()),
            ));
        }
    }
    let entries = match std::fs::read_dir(&paths.recordings) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Written in place, because `/etc/wpa_supplicant.conf` may be a link into `/data`.
fn reset_wifi(conf: &Path, reload: bool) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(conf)?;
    file.write_all(CLEAN_WPA_CONF.as_bytes())?;
    file.sync_all()?;
    std::fs::set_permissions(conf, std::fs::Permissions::from_mode(0o600))?;
    if reload {
        match std::process::Command::new("wpa_cli").args(["-i", "wlan0", "reconfigure"]).output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => warn!("factory reset: wpa_cli reconfigure exited with {}", output.status),
            Err(error) => warn!("factory reset: wpa_cli not run: {error}"),
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Run and watch it pass**

Run: `cargo test -p waved-system --lib reset`
Expected: `test result: ok. 6 passed`.

- [ ] **Step 6: Lint.** Run `cargo clippy -p waved-system --all-targets -- -D warnings`; expect no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-system
git commit -m "feat(system): factory reset that finishes at the next start-up

- FactoryReset leaves a marker and restarts waved; the erase runs before config loads
- erases config.toml*, meetings.db*, the recordings and saved Wi-Fi networks; keeps the binary
- recordings only while /mnt/udisk is mounted; survives a power cut; gives up after 3 starts

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Crate README and the full gate

**Files:**
- Create: `crates/waved-system/README.md`

**Interfaces:** none (documentation and verification).

- [ ] **Step 1: Write** `crates/waved-system/README.md`:

````markdown
# waved-system

System services for the Wave (TODO workstream E). Everything device-specific
sits behind a path or a trait so the crate tests on the host:
`cargo test -p waved-system --lib`.

| Module | What | Device edge |
|---|---|---|
| `display` | `DisplayPower`: rest after `dim_after_s`, off after `off_after_s`, never off while recording or in a call, first touch only wakes, power-key meaning | none (pure) |
| `backlight` | `Backlight::set_percent` | `/sys/class/leds/lcd-backlight/{brightness,max_brightness}` (0–255) |
| `power_key` | `KEY_POWER` → `DeviceEvent::PowerKey(Short / Long)` (long = held 2 s) | `/sys/class/input/event*/device/{name,capabilities/key}`, `/dev/input/eventN` |
| `tz` | `TimeZone::offset_at` from TZif + POSIX footer | `/usr/share/zoneinfo` (slim files) |
| `rtc` | BCD RTC, system clock | `/dev/i2c-3`, slave `0x68`, `settimeofday` |
| `about` | version (`0.1.0 (sha)`), serial, storage, licences | `/unifykey/usid`, `/proc/device-tree/serial-number`, `statvfs` |
| `SystemService` | publishes `SystemState` (offset each minute, About); RTC restore and save | the above |
| `tones` | recording start and stop chimes | `ToneSink` (waved-audio playback) |
| `reset` | factory reset at the next start-up | `/data/waved/config.toml*`, `meetings.db*`, `/mnt/udisk/recordings`, `/etc/wpa_supplicant.conf` |

## Device facts

- **Power key.** `vad_keypad` (`event0`) advertises only `KEY_POWER`
  (`docs/AUDIO_HARDWARE.md`). The reader finds it by name, then by capability,
  never by number. Record here whether the physical key really produces these
  events (milestone 2 device pass). The stock firmware also receives button
  timings from the MCU (`mcu_evt_handle`: `BTN_CLICK`, `BTN_LONG_PRESS_2s_5s_UP`).
- **RTC.** BCD at `0x68` on `/dev/i2c-3`, registers 0–6 = seconds, minutes,
  hours, weekday (0 = Sunday), day, month, year − 2000, in UTC. This is the stock
  `rtc_gettime` / `rtc_settime` layout. Bit 7 of seconds and of month is masked.
- **Time zones.** The device ships slim TZif files (`Europe/Brussels` is
  1 103 bytes). Dates after the last transition come from the POSIX footer,
  for example `CET-1CEST,M3.5.0,M10.5.0/3`.
- **Wi-Fi reset.** The `wpa_supplicant.conf` header is taken from the stock image.
  `wpa_cli reconfigure` follows, because waved-wifi's `save_config` would
  otherwise write the running networks back.

## Wiring

The integrator wires the crate in at the milestone 2 runtime switch. See
`docs/superpowers/plans/2026-10-05-m2-e-system-services.md`, Integration requests.
````

- [ ] **Step 2: Check the module root.** `crates/waved-system/src/lib.rs` must now read exactly:

```rust
//! System services for the Wave (workstream E): display power, the power key,
//! time zone and RTC, alert tones, factory reset and About. Device access sits
//! behind small types and injected paths so everything runs in host tests.

pub mod about;
pub mod backlight;
mod civil;
pub mod display;
pub mod power_key;
pub mod reset;
pub mod rtc;
mod service;
pub mod tones;
pub mod tz;

pub use service::SystemService;
```

- [ ] **Step 3: Run the whole crate**

Run: `set -o pipefail; cargo test -p waved-system --lib 2>&1 | tail -3`
Expected: `test result: ok. 59 passed; 0 failed` (display 11, backlight 4, power_key 5, civil 3, tz 6, rtc 7, about 4, service 7, tones 6, reset 6).

- [ ] **Step 4: Lint the crate**

Run: `cargo clippy -p waved-system --all-targets -- -D warnings`
Expected: `Finished` with no warnings.

- [ ] **Step 5: Check that the workspace still builds on the host**

Run: `cargo check -p waved-system -p waved-state -p waved-ui`
Expected: `Finished`. Nothing outside `waved-system` changed, so this only guards against a broken `Cargo.lock`.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-system/README.md
git commit -m "docs(system): device facts and module map for waved-system

- power key node, RTC register layout, slim zoneinfo, Wi-Fi reset header
- points to the plan's integration requests for the runtime switch

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Device verification (end-of-milestone-2 device pass)

Run these after the runtime switch has wired IR1–IR3. Before deploying, run `adb shell df -h /data` and leave at least 70 MB free. Push to `/data/waved/waved.new`, compare `md5sum`, then `mv` it over `waved` (`project_device_deploy_gotchas`).

1. **Backlight range.**
   - `adb shell cat /sys/class/leds/lcd-backlight/max_brightness` should read 255.
   - On Display, step brightness from 5 % to 100 %. Each step should be visible.
   - Let the screen rest at 80 % (resting level 24 %) and at 5 % (resting 5 %). If 5 % looks dark, raise `MIN_LIT_PCT`.
2. **Timers.**
   - Set Dim after to 15 s; set `off_after_s = 60` on the setup page.
   - Resting should show at 15 s and the screen should turn off at 60 s.
   - Start a recording and wait past 60 s. The screen stays resting with the red ring visible.
   - Repeat with a Teams/Zoom call (HID off-hook).
   - With the screen off, start a recording from the computer. The resting screen should light.
3. **Wake guard.** Rest the screen, then tap the dial. The screen wakes and the mic does **not** mute.
4. **Power key source.**
   - Run `adb shell cat /proc/bus/input/devices` and note the `vad_keypad` handler.
   - Run `adb shell hexdump -C /dev/input/event0`. Press the power key briefly, then hold it for 3 s.
   - Expect records with type `01 00`, code `74 00` and value `01 00 00 00` / `00 00 00 00`.
   - Check `waved.log` for `power key: Short press` / `Long press`.
   - **If nothing arrives:** the physical key reaches the MCU, not Linux. The stock firmware handles it in `mcu_evt_handle` (`BTN_CLICK`, `BTN_LONG_PRESS_2s_*`). Record this in the crate README and hand an MCU source for `DeviceEvent::PowerKey` to workstream F (milestone 3). `DisplayPower` and the event contract stay the same.
   - Also note whether holding the key for 5 s or more cuts power in hardware.
5. **Power key behaviour.**
   - A short press puts the screen off (resting while recording).
   - A second short press wakes it.
   - Holding for 2 s opens "Power off?" while the key is still held.
   - Confirm. Does the unit actually power off, or does waved restart? See Unsure item 2.
6. **RTC.**
   - Run `adb shell i2cdetect -y 3` if busybox has it; expect `68`, not `UU`.
   - Connect Wi-Fi. The log shows `RTC set from network time`.
   - Power-cycle with Wi-Fi off. The log shows `clock restored from the RTC`, and the status clock is right.
7. **Time zone and format.**
   - Set `Europe/Brussels`. The status clock should be UTC+2 in October; compare with `adb shell TZ=Europe/Brussels date`.
   - Toggle 24-hour.
   - Check that `--:--` shows before any time source.
8. **Minute refresh.** Watch the status clock roll over to the next minute without any other change on screen.
9. **Tones.**
   - Start a recording: a rising chime plays. Stop: a falling chime.
   - Turn Alert tones off: silence.
   - While the computer streams audio over USB, start a recording and listen for glitches beyond the tone.
10. **Factory reset.**
    - Save a Wi-Fi network and make a recording, then reset.
    - Waved restarts into first start.
    - `cat /data/waved/config.toml` shows defaults or is absent.
    - `ls /mnt/udisk/recordings` is empty.
    - `wpa_cli -i wlan0 list_networks` shows none.
    - `ls /data/waved` still has `waved`.
11. **About.**
    - The version shows the git id.
    - Compare the serial with the label on the unit. If it is empty or wrong, find the source (`ls /unifykey`, `cat /proc/device-tree/serial-number`) and update `SERIAL_SOURCES`.
    - Storage matches `df /mnt/udisk`.
12. **Binary size.** Compare the stripped release binary with the previous deploy. `waved-system` should add well under 1 MB.

## Self-review

- **Spec coverage:**
  - E1: Tasks 1, 2 and IR3.
  - E2: Task 3, Task 1 (`on_power_key`) and IR3.
  - E3: Tasks 4, 5 and 7, plus CR3.
  - E5: Task 8 and IR2.
  - E6: Task 9 and IR2.
  - E7: Tasks 6 and 7.
  - E4 is excluded (D5). Bluetooth pairings in E6 are deferred to I1, with the reason given.
  - Mockup timings (2 min / 10 min / 2 s / first tap wakes only) are pinned by Task 1 and Task 3 tests.
- **Types checked against the real code:**
  - `SettingsState` fields: `brightness_pct`, `dim_after_s`, `off_after_s`, `time_zone`, `clock_24h`, `tones_enabled`.
  - `MeetingState.{recording, paused}`.
  - `WifiState.{clock_synchronized, signal_strength}` (`Default` derived).
  - `AudioState.call_off_hook`.
  - `Command::{FactoryReset, RestartForConfig, Shutdown}`.
  - `kit::dispatch::UiEvent::{Rest, Wake, Navigate}`, `kit::route::{Route::Confirm, ConfirmKind::PowerOff}`, `kit::snapshot::{format_clock, UiSnapshot::capture, in_call}`.
  - `PlaybackService::{play_batch, channels, period_frames, sample_rate}`.
  - `MeetingState` has no `PartialEq`; nothing here compares it.
  - New types (`PowerKeyPress`, `SystemState`, `AboutInfo`, `CLOCK_VALID_AFTER_UNIX_S`, `StateBus.system`) are all in CR1/CR2.
- **Placeholders:** none. Every step has the complete code or command.
- **Review focus:** each of the five items names an existing test in Tasks 1, 4, 5 or 9.
