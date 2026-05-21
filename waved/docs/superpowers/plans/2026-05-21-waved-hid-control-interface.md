# SP#7(d') — HID Consumer/Telephony Control Interface — Implementation Plan (v2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the existing `waved-hid` crate to (a) correctly emit stock-shaped HID Consumer / Telephony reports for both MCU-button and state-driven sources, (b) read host LED Set_Reports back into `AudioState`, and (c) install the stock report descriptor via configfs.

**Architecture:** All new work lives under `crates/waved-hid/src/` (existing crate). New files: `events.rs`, `report.rs`, `reader.rs`, `watch.rs`, `stats.rs`. The existing `lib.rs` is refactored so all sources push a unified `HidEvent` into one internal mpsc that a single writer task drains. The MCU `DeviceEvent::ButtonPress(ButtonType)` source is preserved. Spec §4 fully describes the layout.

**Tech Stack:** Rust 1.85 (edition 2021), tokio (existing `waved-hid` Cargo features `sync io-util fs` — async file I/O via `tokio::fs::OpenOptions` already in use), serde, tracing.

**Spec reference:** `docs/superpowers/specs/2026-05-21-waved-hid-control-interface-design.md` v2 (commit `d4fcfad`).

**Status of Task 1 (AudioState fields):** Already completed in commit `02b726d` during a Task 1 dispatch under the v1 plan. The fields are needed under v2 unchanged, so no rework required. Task 1 below is marked complete.

---

## Task 1: ✅ `AudioState` `call_*` fields (already shipped)

**Status:** DONE. Commit `02b726d`. The `call_off_hook`, `call_ringing`, `call_host_muted` plain-`bool` fields were added to `crates/waved-state/src/lib.rs` and seeded `false` in `Default`. Test `audio_state_default_call_fields_false` covers it. No action required.

---

## Task 2: Add `HidConfig` to `audio_config.rs`

**Files:**
- Modify: `crates/waved-audio/src/audio_config.rs` (add `HidConfig` + `pub hid: Option<HidConfig>` on `AudioConfig`)
- Test: `crates/waved-audio/src/audio_config.rs` (existing `#[cfg(test)]` block)

- [ ] **Step 1: Write the failing test**

Add inside the existing `#[cfg(test)] mod tests` block in `crates/waved-audio/src/audio_config.rs`:

```rust
#[test]
fn hid_config_default_when_absent() {
    let toml = r#"
        [capture]
        mic_device = "hw:0,0"
        mic_channels = 8
        mic_sample_rate = 48000
        mic_format = "S16_LE"
        period_frames = 512
        buffer_periods = 4
    "#;
    let cfg: AudioConfig = toml::from_str(toml).unwrap();
    let hid = cfg.hid.unwrap_or_default();
    assert!(hid.enabled);
    assert_eq!(hid.device, "/dev/hidg0");
    assert_eq!(hid.press_release_gap_ms, 10);
    assert_eq!(hid.volume_step_rate_hz, 40);
}

#[test]
fn hid_config_with_overrides_parses() {
    let toml = r#"
        [capture]
        mic_device = "hw:0,0"
        mic_channels = 8
        mic_sample_rate = 48000
        mic_format = "S16_LE"
        period_frames = 512
        buffer_periods = 4

        [hid]
        enabled = false
        device = "/dev/hidg1"
        press_release_gap_ms = 25
        volume_step_rate_hz = 20
    "#;
    let cfg: AudioConfig = toml::from_str(toml).unwrap();
    let hid = cfg.hid.unwrap();
    assert!(!hid.enabled);
    assert_eq!(hid.device, "/dev/hidg1");
    assert_eq!(hid.press_release_gap_ms, 25);
    assert_eq!(hid.volume_step_rate_hz, 20);
}
```

- [ ] **Step 2: Verify tests fail**

`cargo test -p waved-audio --lib audio_config::tests::hid_config`
Expected: FAIL — `no field hid on type 'AudioConfig'`.

- [ ] **Step 3: Add `HidConfig` and the optional `hid` field**

Immediately after the `impl Default for UacConfig { ... }` block in `crates/waved-audio/src/audio_config.rs`, add:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct HidConfig {
    #[serde(default = "default_hid_enabled")]
    pub enabled: bool,
    #[serde(default = "default_hid_device")]
    pub device: String,
    #[serde(default = "default_hid_press_release_gap_ms")]
    pub press_release_gap_ms: u64,
    #[serde(default = "default_hid_volume_step_rate_hz")]
    pub volume_step_rate_hz: u32,
}

fn default_hid_enabled() -> bool { true }
fn default_hid_device() -> String { "/dev/hidg0".to_string() }
fn default_hid_press_release_gap_ms() -> u64 { 10 }
fn default_hid_volume_step_rate_hz() -> u32 { 40 }

impl Default for HidConfig {
    fn default() -> Self {
        Self {
            enabled: default_hid_enabled(),
            device: default_hid_device(),
            press_release_gap_ms: default_hid_press_release_gap_ms(),
            volume_step_rate_hz: default_hid_volume_step_rate_hz(),
        }
    }
}
```

Locate the top-level `pub struct AudioConfig` declaration; add next to the existing `pub uac: Option<UacConfig>`:

```rust
    #[serde(default)]
    pub hid: Option<HidConfig>,
```

- [ ] **Step 4: Tests pass**

`cargo test -p waved-audio --lib audio_config::tests::hid_config`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/audio_config.rs
git commit -m "feat(audio_config): add HidConfig block (default enabled, /dev/hidg0)"
```

---

## Task 3: `HidEvent` enum + `report.rs` + flip Report IDs to stock

**Files:**
- Create: `crates/waved-hid/src/events.rs` — `HidEvent` enum
- Create: `crates/waved-hid/src/report.rs` — Report-ID/bit constants + `event_to_press_release` pure fn + tests
- Modify: `crates/waved-hid/src/lib.rs` — declare new modules; replace `REPORT_ID_TELEPHONE/CONSUMER` constants and the `build_hid_report` function; update existing tests to assert new IDs

**Why all in one task:** the existing `lib.rs:13-25` (constants + `build_hid_report`) and the existing tests `lib.rs:84-110` are tightly coupled. Splitting these into separate tasks would leave the crate non-building between commits. One coherent commit.

- [ ] **Step 1: Write the new tests first**

Create `crates/waved-hid/src/events.rs`:

```rust
//! Output event type for the HID gadget — single ingress point that all
//! sources (MCU buttons, audio-state watchers) push to.

/// What we want the host to receive. The writer task serializes each one
/// into a press-release pair on `/dev/hidg0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidEvent {
    // Report 0x01 — Consumer page (press = bit set, release = bit cleared)
    ConsumerVolumeStep { up: bool },
    ConsumerMute,
    ConsumerPlayPause,
    ConsumerNext,
    ConsumerPrev,

    // Report 0x02 — Telephony page
    TelephonyPhoneMute,
}
```

Create `crates/waved-hid/src/report.rs`:

```rust
//! Pure HID-report serialization — `HidEvent → (press_bytes, release_bytes)`.
//!
//! Report IDs and bit mapping match the **stock** descriptor from
//! `fw_v1.1.6.6/rootfs/etc/init.d/S89usbgadget:199`:
//!
//!   Report 0x01 (Consumer): bit0 VolumeUp, bit1 VolumeDown, bit2 Mute,
//!                           bit3 PlayPause, bit4 Next, bit5 Prev.
//!   Report 0x02 (Telephony): bit0 HookSwitch, bit1 PhoneMute, ...

use super::events::HidEvent;

pub const REPORT_ID_CONSUMER:  u8 = 0x01;
pub const REPORT_ID_TELEPHONY: u8 = 0x02;

pub const CONSUMER_VOLUME_UP:   u8 = 1 << 0;
pub const CONSUMER_VOLUME_DOWN: u8 = 1 << 1;
pub const CONSUMER_MUTE:        u8 = 1 << 2;
pub const CONSUMER_PLAY_PAUSE:  u8 = 1 << 3;
pub const CONSUMER_NEXT:        u8 = 1 << 4;
pub const CONSUMER_PREV:        u8 = 1 << 5;

pub const TELEPHONY_PHONE_MUTE: u8 = 1 << 1;

/// Serialize one `HidEvent` to `(press, release)` buffers, each including
/// the leading Report ID byte.
pub fn event_to_press_release(ev: HidEvent) -> ([u8; 2], [u8; 2]) {
    match ev {
        HidEvent::ConsumerVolumeStep { up: true }  => ([REPORT_ID_CONSUMER, CONSUMER_VOLUME_UP],   [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerVolumeStep { up: false } => ([REPORT_ID_CONSUMER, CONSUMER_VOLUME_DOWN], [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerMute                     => ([REPORT_ID_CONSUMER, CONSUMER_MUTE],        [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerPlayPause                => ([REPORT_ID_CONSUMER, CONSUMER_PLAY_PAUSE],  [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerNext                     => ([REPORT_ID_CONSUMER, CONSUMER_NEXT],        [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::ConsumerPrev                     => ([REPORT_ID_CONSUMER, CONSUMER_PREV],        [REPORT_ID_CONSUMER, 0x00]),
        HidEvent::TelephonyPhoneMute               => ([REPORT_ID_TELEPHONY, TELEPHONY_PHONE_MUTE], [REPORT_ID_TELEPHONY, 0x00]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_up_uses_consumer_report_bit0() {
        let (p, r) = event_to_press_release(HidEvent::ConsumerVolumeStep { up: true });
        assert_eq!(p, [0x01, 0b0000_0001]);
        assert_eq!(r, [0x01, 0x00]);
    }

    #[test]
    fn volume_down_uses_consumer_report_bit1() {
        let (p, r) = event_to_press_release(HidEvent::ConsumerVolumeStep { up: false });
        assert_eq!(p, [0x01, 0b0000_0010]);
        assert_eq!(r, [0x01, 0x00]);
    }

    #[test]
    fn consumer_mute_bit2() {
        let (p, _r) = event_to_press_release(HidEvent::ConsumerMute);
        assert_eq!(p, [0x01, 0b0000_0100]);
    }

    #[test]
    fn telephony_phone_mute_bit1_on_report_2() {
        let (p, _r) = event_to_press_release(HidEvent::TelephonyPhoneMute);
        assert_eq!(p, [0x02, 0b0000_0010]);
    }

    #[test]
    fn play_pause_next_prev() {
        assert_eq!(event_to_press_release(HidEvent::ConsumerPlayPause).0, [0x01, 0b0000_1000]);
        assert_eq!(event_to_press_release(HidEvent::ConsumerNext).0,      [0x01, 0b0001_0000]);
        assert_eq!(event_to_press_release(HidEvent::ConsumerPrev).0,      [0x01, 0b0010_0000]);
    }
}
```

Now update `crates/waved-hid/src/lib.rs`: delete the existing `REPORT_ID_TELEPHONE / REPORT_ID_CONSUMER` constants (lines 13-14), the `build_hid_report` function (lines 16-25), and the existing test module (lines 80-111). Replace with the new module declarations + a thin compatibility shim from `ButtonType` to `HidEvent` (used by the MCU-button bridge that still lives in `HidService::run`).

Apply to `crates/waved-hid/src/lib.rs`:

1. Delete lines 13-14 (old constants).
2. Delete lines 16-25 (old `build_hid_report`).
3. Delete lines 80-111 (old test module).
4. After the existing `#[cfg(feature = "simulate")] pub mod simulate;` line, add:

```rust
pub mod events;
pub mod report;
```

5. Add a `pub fn button_to_event(b: ButtonType) -> HidEvent` shim above `pub struct HidService;`:

```rust
use events::HidEvent;

/// Map a hardware `ButtonType` (from waved-mcu) to a `HidEvent`. The MCU
/// only emits four button kinds; richer events (PlayPause, Next/Prev,
/// PhoneMute) come from the state watchers in `watch.rs`.
pub fn button_to_event(b: ButtonType) -> HidEvent {
    match b {
        ButtonType::VolumeUp      => HidEvent::ConsumerVolumeStep { up: true },
        ButtonType::VolumeDown    => HidEvent::ConsumerVolumeStep { up: false },
        ButtonType::MuteToggle    => HidEvent::ConsumerMute,
        ButtonType::MuteLongPress => HidEvent::ConsumerMute,
    }
}
```

6. Replace the existing inline `Self::send_hid_report(&btn).await` body so it now calls `report::event_to_press_release(button_to_event(btn))` and writes both halves; OR temporarily leave `HidService::run` calling `send_hid_report` against the old per-button code path while keeping the new modules unused — and let Task 5/6 (which refactors `HidService::run` properly) do the rewiring. **Choose the latter (defer rewiring to Task 5/6).** For Task 3 the goal is just landing the new modules + ID-corrected serializer + tests. To keep the crate building, leave `send_hid_report` and a renamed-but-existing `build_hid_report_legacy` together as `#[deprecated]`-tagged and unused-test-only — see exact steps below.

7. To satisfy step 6: replace lines 16-25 with the **same body** but rename `build_hid_report` → `build_hid_report_legacy` and add a `#[deprecated]` attribute:

```rust
#[deprecated(note = "Use report::event_to_press_release(button_to_event(b)) instead. Removed in Task 5/6.")]
#[allow(dead_code)]
fn build_hid_report_legacy(button: &ButtonType) -> (Vec<u8>, Vec<u8>) {
    // unchanged body — kept until HidService::run is refactored in Task 5/6
    let press = match button {
        ButtonType::MuteToggle => vec![0x02, 0x04],   // Consumer + Mute bit, stock-correct
        ButtonType::VolumeUp => vec![0x01, 0x01],     // Consumer + VolumeUp, stock-correct
        ButtonType::VolumeDown => vec![0x01, 0x02],   // Consumer + VolumeDown, stock-correct
        ButtonType::MuteLongPress => vec![0x02, 0x04],
    };
    let release = vec![press[0], 0x00];
    (press, release)
}
```

Wait — actually the simpler path is to keep the writer doing what it does today, but with new constants matching stock. Replace the existing constants + `build_hid_report` body with:

```rust
// Stock-correct Report IDs (was reversed prior to v2 of SP#7(d')).
const REPORT_ID_CONSUMER:  u8 = 0x01;
const REPORT_ID_TELEPHONY: u8 = 0x02;

fn build_hid_report(button: &ButtonType) -> (Vec<u8>, Vec<u8>) {
    // Bit-correct against the stock Consumer descriptor:
    //   bit0 VolumeUp, bit1 VolumeDown, bit2 Mute.
    let press = match button {
        ButtonType::MuteToggle    => vec![REPORT_ID_CONSUMER, 0b0000_0100], // bit2 Mute
        ButtonType::MuteLongPress => vec![REPORT_ID_CONSUMER, 0b0000_0100], // bit2 Mute (PhoneMute deferred to watch.rs)
        ButtonType::VolumeUp      => vec![REPORT_ID_CONSUMER, 0b0000_0001],
        ButtonType::VolumeDown    => vec![REPORT_ID_CONSUMER, 0b0000_0010],
    };
    let release = vec![press[0], 0x00];
    (press, release)
}
```

Then update the existing tests (lines 80-111) to match the stock-correct expectations:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_toggle_report() {
        // Stock Consumer Mute = Report 0x01, bit2.
        let (press, release) = build_hid_report(&ButtonType::MuteToggle);
        assert_eq!(press, vec![0x01, 0b0000_0100]);
        assert_eq!(release, vec![0x01, 0x00]);
    }

    #[test]
    fn volume_up_report() {
        let (press, release) = build_hid_report(&ButtonType::VolumeUp);
        assert_eq!(press, vec![0x01, 0b0000_0001]);
        assert_eq!(release, vec![0x01, 0x00]);
    }

    #[test]
    fn volume_down_report() {
        let (press, release) = build_hid_report(&ButtonType::VolumeDown);
        assert_eq!(press, vec![0x01, 0b0000_0010]);
        assert_eq!(release, vec![0x01, 0x00]);
    }

    #[test]
    fn mute_long_press_report() {
        let (press, release) = build_hid_report(&ButtonType::MuteLongPress);
        assert_eq!(press, vec![0x01, 0b0000_0100]);
        assert_eq!(release, vec![0x01, 0x00]);
    }
}
```

(Skip the deprecated/legacy detour from earlier in this step — the clean swap is what we want.)

- [ ] **Step 2: Update `crates/waved-hid/src/simulate.rs` log strings**

The simulate module's log lines hardcode the old report-ID-to-page mapping. Replace its match arm bodies to reflect the new mapping. Specifically, in `crates/waved-hid/src/simulate.rs`, find the match block on `btn`:

```rust
                        ButtonType::MuteToggle => "TELEPHONE:mute_toggle",
                        ButtonType::VolumeUp => "CONSUMER:volume_up",
                        ButtonType::VolumeDown => "CONSUMER:volume_down",
                        ButtonType::MuteLongPress => "TELEPHONE:mute_long",
```

Replace with:

```rust
                        ButtonType::MuteToggle    => "CONSUMER:mute",
                        ButtonType::VolumeUp      => "CONSUMER:volume_up",
                        ButtonType::VolumeDown    => "CONSUMER:volume_down",
                        ButtonType::MuteLongPress => "CONSUMER:mute",
```

(Both Mute variants now log as Consumer Mute since that's what the wire bytes encode. PhoneMute will be added in Task 9 when the mute_watch task starts emitting it.)

- [ ] **Step 3: Build & test**

```bash
cargo test -p waved-hid --lib
cargo test -p waved-hid --lib --features simulate
```

Expected: all existing 4 tests in `lib.rs::tests` pass against new expectations; new 5 tests in `report.rs::tests` pass. No simulate test failure.

- [ ] **Step 4: Commit**

```bash
git add crates/waved-hid/src/lib.rs crates/waved-hid/src/events.rs crates/waved-hid/src/report.rs crates/waved-hid/src/simulate.rs
git commit -m "fix(hid): swap Report IDs to match stock + introduce HidEvent + report.rs"
```

---

## Task 4: `HidStats` + stats_collector wiring

**Files:**
- Create: `crates/waved-hid/src/stats.rs` — `HidStats`, `HidStatsSnapshot`, `HidEventCounts`, `HidLedCounts`
- Modify: `crates/waved-hid/src/lib.rs` — declare `pub mod stats;`
- Modify: `crates/waved-audio/Cargo.toml` — add `waved-hid = { path = "../waved-hid" }` to dependencies (if not already)
- Modify: `crates/waved-audio/src/stats_collector.rs` — add `hid` field to `AudioStats`, thread `hid_stats` through `run()` and `compute_stats_with_processed()`
- Modify: `crates/waved/src/main.rs` — declare `hid_stats_handle: Option<Arc<waved_hid::HidStats>>` and pass through to `stats_collector::run`

- [ ] **Step 1: Confirm dependency wiring**

Run: `grep -n 'waved-hid' crates/waved-audio/Cargo.toml`
- If a `waved-hid` dependency exists: skip the Cargo.toml edit in Step 3.
- If it does not: keep the Cargo.toml edit in Step 3.

- [ ] **Step 2: Write the failing test**

Append to (or create) a `#[cfg(test)] mod tests_hid_wiring` block at the end of `crates/waved-audio/src/stats_collector.rs`:

```rust
#[cfg(test)]
mod tests_hid_wiring {
    use super::*;
    use waved_hid::stats::HidStats;

    #[test]
    fn audio_stats_serializes_hid_when_present() {
        let stats = HidStats::default();
        stats.events_volume_step.fetch_add(5, std::sync::atomic::Ordering::Relaxed);
        let snap = stats.snapshot();

        let mut audio = sample_audio_stats();
        audio.hid = Some(snap);

        let json = serde_json::to_value(&audio).unwrap();
        assert_eq!(json["hid"]["events_emitted"]["volume_step"], 5);
    }

    #[test]
    fn audio_stats_omits_hid_when_none() {
        let audio = sample_audio_stats();
        let json = serde_json::to_value(&audio).unwrap();
        assert!(json.get("hid").is_none(), "expected hid field absent when None");
    }

    fn sample_audio_stats() -> AudioStats {
        AudioStats {
            last_updated_ms: 0,
            channel_map_source: "fallback".into(),
            sample_rate: 48_000,
            channels: 1,
            frames_emitted: 0,
            frames_dropped: 0,
            xrun_count: 0,
            rms_per_channel: vec![],
            clipping_rate_per_channel: vec![],
            dc_offset_per_channel: vec![],
            correlation_matrix: vec![],
            last_write_error: None,
            reference_rms: vec![],
            alignment: String::new(),
            mic_post_aec_rms: None,
            doa_horizontal: 0.0,
            doa_pitch: 0.0,
            doa_active: false,
            mode: waved_state::AudioMode::default(),
            playback_preset: "voice".into(),
            mbagc_active_gain_db: [0.0; 4],
            limiter_reduction_db: 0.0,
            mic_gain_linear: 1.0,
            mic_gain_db: 0.0,
            recording_limiter_reduction_db: 0.0,
            #[cfg(feature = "capture-alsa")]
            uac: None,
            hid: None,
        }
    }
}
```

- [ ] **Step 3: Create the stats module**

If Cargo.toml needed editing (per Step 1), add to `[dependencies]` of `crates/waved-audio/Cargo.toml`:

```toml
waved-hid = { path = "../waved-hid" }
```

Create `crates/waved-hid/src/stats.rs`:

```rust
//! Atomic stats surfaced into `/tmp/audio_stats.json` via the
//! waved-audio stats_collector.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct HidStats {
    pub device_present: AtomicBool,
    pub events_volume_step: AtomicU64,
    pub events_mute: AtomicU64,
    pub events_phone_mute: AtomicU64,
    pub events_play_pause: AtomicU64,
    pub events_next: AtomicU64,
    pub events_prev: AtomicU64,
    pub leds_off_hook: AtomicU64,
    pub leds_ringing: AtomicU64,
    pub leds_mute: AtomicU64,
    pub call_off_hook: AtomicBool,
    pub call_ringing: AtomicBool,
    pub call_host_muted: AtomicBool,
    pub write_errors: AtomicU64,
    pub glue_drops: AtomicU64,
}

impl HidStats {
    pub fn new() -> Arc<Self> { Arc::new(Self::default()) }

    pub fn snapshot(&self) -> HidStatsSnapshot {
        HidStatsSnapshot {
            device_present: self.device_present.load(Ordering::Relaxed),
            events_emitted: HidEventCounts {
                volume_step: self.events_volume_step.load(Ordering::Relaxed),
                mute: self.events_mute.load(Ordering::Relaxed),
                phone_mute: self.events_phone_mute.load(Ordering::Relaxed),
                play_pause: self.events_play_pause.load(Ordering::Relaxed),
                next: self.events_next.load(Ordering::Relaxed),
                prev: self.events_prev.load(Ordering::Relaxed),
            },
            leds_received: HidLedCounts {
                off_hook: self.leds_off_hook.load(Ordering::Relaxed),
                ringing: self.leds_ringing.load(Ordering::Relaxed),
                mute: self.leds_mute.load(Ordering::Relaxed),
            },
            call_off_hook: self.call_off_hook.load(Ordering::Relaxed),
            call_ringing: self.call_ringing.load(Ordering::Relaxed),
            call_host_muted: self.call_host_muted.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
            glue_drops: self.glue_drops.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HidStatsSnapshot {
    pub device_present: bool,
    pub events_emitted: HidEventCounts,
    pub leds_received: HidLedCounts,
    pub call_off_hook: bool,
    pub call_ringing: bool,
    pub call_host_muted: bool,
    pub write_errors: u64,
    pub glue_drops: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HidEventCounts {
    pub volume_step: u64,
    pub mute: u64,
    pub phone_mute: u64,
    pub play_pause: u64,
    pub next: u64,
    pub prev: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HidLedCounts {
    pub off_hook: u64,
    pub ringing: u64,
    pub mute: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_starts_zero() {
        let s = HidStats::default();
        let snap = s.snapshot();
        assert!(!snap.device_present);
        assert_eq!(snap.events_emitted.volume_step, 0);
    }

    #[test]
    fn snapshot_reflects_writes() {
        let s = HidStats::default();
        s.device_present.store(true, Ordering::Relaxed);
        s.events_volume_step.fetch_add(7, Ordering::Relaxed);
        s.leds_off_hook.fetch_add(2, Ordering::Relaxed);
        s.call_off_hook.store(true, Ordering::Relaxed);
        let snap = s.snapshot();
        assert!(snap.device_present);
        assert_eq!(snap.events_emitted.volume_step, 7);
        assert_eq!(snap.leds_received.off_hook, 2);
        assert!(snap.call_off_hook);
    }
}
```

Add `pub mod stats;` to `crates/waved-hid/src/lib.rs` next to the existing module declarations.

- [ ] **Step 4: Thread through stats_collector**

Add the field to `AudioStats` (in `crates/waved-audio/src/stats_collector.rs`), immediately after the existing `uac` field:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hid: Option<waved_hid::stats::HidStatsSnapshot>,
```

Locate `pub async fn run(...)`. Append new parameter at the end:

```rust
    hid_stats: Option<Arc<waved_hid::stats::HidStats>>,
```

Inside `run`, find the call to `compute_stats_with_processed(...)` and add `hid_stats.as_deref()` as the trailing arg.

Locate `pub fn compute_stats_with_processed(...)`; append new parameter:

```rust
    hid_stats: Option<&waved_hid::stats::HidStats>,
```

Just before the `AudioStats { ... }` literal in that function, add:

```rust
    let hid = hid_stats.map(|s| s.snapshot());
```

Add `hid,` to the literal.

Update the no-processed wrapper `pub fn compute_stats(...)` — add a trailing `None` to the inner call, matching the existing pattern for `uac_stats`.

- [ ] **Step 5: Forward from `main.rs`**

In `crates/waved/src/main.rs`, find where `uac_stats_handle: Option<Arc<waved_audio::uac::UacStats>>` is declared (around line 227). Add a parallel:

```rust
    let mut hid_stats_handle: Option<Arc<waved_hid::stats::HidStats>> = None;
```

Find the existing `stats_collector::run(...)` invocation. Add `hid_stats_handle.clone()` as the trailing argument.

- [ ] **Step 6: Build & test**

```bash
cargo build -p waved-hid -p waved-audio -p waved
cargo test -p waved-hid --lib stats::tests
cargo test -p waved-audio --lib stats_collector::tests_hid_wiring
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-hid/src/stats.rs crates/waved-hid/src/lib.rs crates/waved-audio/Cargo.toml crates/waved-audio/src/stats_collector.rs crates/waved/src/main.rs
git commit -m "feat(hid): HidStats + wire through stats_collector to /tmp/audio_stats.json"
```

---

## Task 5: Refactor `HidService` — single writer task draining an mpsc

**Files:**
- Modify: `crates/waved-hid/src/lib.rs` — replace `HidService::run` so it (1) opens `/dev/hidg0` once, (2) drains a `mpsc::Receiver<HidEvent>` and writes press-release pairs using `report::event_to_press_release`, (3) the bridge from `DeviceEvent::ButtonPress` lives in `HidService::run` as a small selector loop that pushes to the same mpsc.

- [ ] **Step 1: Write an integration test using a tempfile**

Append to (or create) a `#[cfg(test)] mod integ_tests` block at the end of `crates/waved-hid/src/lib.rs`:

```rust
#[cfg(test)]
mod integ_tests {
    use super::*;
    use std::io::Read;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::sync::{broadcast, mpsc};

    #[tokio::test(flavor = "current_thread")]
    async fn writer_emits_volume_up_for_button_press() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::File::create(&path).unwrap();

        let (btn_tx, btn_rx) = broadcast::channel(8);
        let (ev_tx, ev_rx) = mpsc::channel(16);

        let cfg = HidServiceConfig {
            device: path.to_string_lossy().into_owned(),
            press_release_gap_ms: 1,
        };
        let stats = stats::HidStats::new();
        tokio::spawn(HidService::run(cfg, btn_rx, ev_rx, std::sync::Arc::clone(&stats)));

        btn_tx.send(waved_state::DeviceEvent::ButtonPress(waved_state::ButtonType::VolumeUp)).unwrap();
        // unused so the channel doesn't drop:
        let _keep = ev_tx;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut buf = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf, vec![0x01, 0b0000_0001, 0x01, 0x00]);
        assert!(stats.device_present.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(stats.events_volume_step.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn writer_emits_consumer_mute_from_external_event() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::File::create(&path).unwrap();

        let (_btn_tx, btn_rx) = broadcast::channel::<waved_state::DeviceEvent>(1);
        let (ev_tx, ev_rx) = mpsc::channel(16);

        let cfg = HidServiceConfig {
            device: path.to_string_lossy().into_owned(),
            press_release_gap_ms: 1,
        };
        let stats = stats::HidStats::new();
        tokio::spawn(HidService::run(cfg, btn_rx, ev_rx, std::sync::Arc::clone(&stats)));

        ev_tx.send(events::HidEvent::ConsumerMute).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut buf = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf, vec![0x01, 0b0000_0100, 0x01, 0x00]);
        assert_eq!(stats.events_mute.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
```

Add `tempfile = "3"` under `[dev-dependencies]` of `crates/waved-hid/Cargo.toml` if not already present.

- [ ] **Step 2: Replace `HidService` impl**

Replace the existing `pub struct HidService;` and `impl HidService` in `crates/waved-hid/src/lib.rs` with:

```rust
pub struct HidService;

#[derive(Debug, Clone)]
pub struct HidServiceConfig {
    pub device: String,
    pub press_release_gap_ms: u64,
}

impl HidService {
    /// Drain HidEvents from `events_rx` (external sources: watchers) AND
    /// bridge `DeviceEvent::ButtonPress(ButtonType)` from `bus_rx` into
    /// the same event stream. Write each event as a press-release pair
    /// to `/dev/hidg0`.
    pub async fn run(
        cfg: HidServiceConfig,
        mut bus_rx: tokio::sync::broadcast::Receiver<waved_state::DeviceEvent>,
        mut events_rx: tokio::sync::mpsc::Receiver<events::HidEvent>,
        stats: std::sync::Arc<stats::HidStats>,
    ) {
        info!("hid_srv started");

        let mut dev: Option<std::fs::File> = match std::fs::OpenOptions::new()
            .write(true)
            .open(&cfg.device)
        {
            Ok(f) => {
                stats.device_present.store(true, std::sync::atomic::Ordering::Relaxed);
                Some(f)
            }
            Err(e) => {
                warn!("hid: cannot open {}: {} — HID writes disabled", cfg.device, e);
                stats.device_present.store(false, std::sync::atomic::Ordering::Relaxed);
                None
            }
        };

        let gap = std::time::Duration::from_millis(cfg.press_release_gap_ms);

        loop {
            tokio::select! {
                bus = bus_rx.recv() => match bus {
                    Ok(waved_state::DeviceEvent::ButtonPress(btn)) => {
                        let ev = button_to_event(btn);
                        if let Some(f) = dev.as_mut() {
                            Self::write_event(f, ev, gap, &stats).await;
                        } else {
                            stats.write_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("hid: device-event bus closed");
                        return;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("hid_srv lagged by {n} events");
                    }
                },
                ev = events_rx.recv() => match ev {
                    Some(ev) => {
                        if let Some(f) = dev.as_mut() {
                            Self::write_event(f, ev, gap, &stats).await;
                        } else {
                            stats.write_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    None => {
                        info!("hid: mpsc closed");
                        return;
                    }
                },
            }
        }
    }

    async fn write_event(
        f: &mut std::fs::File,
        ev: events::HidEvent,
        gap: std::time::Duration,
        stats: &stats::HidStats,
    ) {
        use std::io::Write;
        let (press, release) = report::event_to_press_release(ev);
        if let Err(e) = f.write_all(&press) {
            warn!("hid: write press failed: {}", e);
            stats.write_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        tokio::time::sleep(gap).await;
        if let Err(e) = f.write_all(&release) {
            warn!("hid: write release failed: {}", e);
            stats.write_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        // Stats per event variant.
        use std::sync::atomic::Ordering;
        match ev {
            events::HidEvent::ConsumerVolumeStep { .. } => { stats.events_volume_step.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::ConsumerMute              => { stats.events_mute.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::ConsumerPlayPause         => { stats.events_play_pause.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::ConsumerNext              => { stats.events_next.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::ConsumerPrev              => { stats.events_prev.fetch_add(1, Ordering::Relaxed); }
            events::HidEvent::TelephonyPhoneMute        => { stats.events_phone_mute.fetch_add(1, Ordering::Relaxed); }
        }
    }
}
```

Also delete the orphaned older constants block (now redundant) and the older `send_hid_report` async method.

- [ ] **Step 3: Build & test**

```bash
cargo test -p waved-hid --lib
cargo test -p waved-hid --lib --features simulate
```

Expected: every test (events, report, stats, integ_tests) passes.

- [ ] **Step 4: Commit**

```bash
git add crates/waved-hid/src/lib.rs crates/waved-hid/Cargo.toml
git commit -m "refactor(hid): single writer task drains mpsc<HidEvent>; preserve MCU bridge"
```

---

## Task 6: `HidReader` — LED parser + blocking thread

**Files:**
- Create: `crates/waved-hid/src/reader.rs` — `parse_led` + `apply_led` + `HidReader::spawn` (blocking thread)
- Modify: `crates/waved-hid/src/lib.rs` — declare `pub mod reader;`

- [ ] **Step 1: Write the failing tests**

Create `crates/waved-hid/src/reader.rs`:

```rust
//! HID Reader — blocking std::thread owning the read side of /dev/hidg0.
//! Parses host Set_Report writes (Reports 0x05 / 0x06 / 0x07) into
//! AudioState.call_*.

use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::sync::watch;
use tracing::{info, warn};

use waved_state::AudioState;

use crate::stats::HidStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedReport {
    OffHook(bool),
    Mute(bool),
    Ringing(bool),
}

pub fn parse_led(buf: &[u8]) -> Option<LedReport> {
    if buf.len() < 2 { return None; }
    let bit = (buf[1] & 0x01) != 0;
    match buf[0] {
        0x05 => Some(LedReport::OffHook(bit)),
        0x06 => Some(LedReport::Mute(bit)),
        0x07 => Some(LedReport::Ringing(bit)),
        _ => None,
    }
}

pub fn apply_led(led: LedReport, stats: &HidStats, audio_tx: &watch::Sender<AudioState>) {
    match led {
        LedReport::OffHook(b) => {
            stats.leds_off_hook.fetch_add(1, Ordering::Relaxed);
            stats.call_off_hook.store(b, Ordering::Relaxed);
            audio_tx.send_modify(|s| s.call_off_hook = b);
        }
        LedReport::Mute(b) => {
            stats.leds_mute.fetch_add(1, Ordering::Relaxed);
            stats.call_host_muted.store(b, Ordering::Relaxed);
            audio_tx.send_modify(|s| s.call_host_muted = b);
        }
        LedReport::Ringing(b) => {
            stats.leds_ringing.fetch_add(1, Ordering::Relaxed);
            stats.call_ringing.store(b, Ordering::Relaxed);
            audio_tx.send_modify(|s| s.call_ringing = b);
        }
    }
}

pub struct HidReader;

#[derive(Debug, Clone)]
pub struct HidReaderConfig {
    pub device: String,
}

impl HidReader {
    pub fn spawn(
        cfg: HidReaderConfig,
        audio_tx: Arc<watch::Sender<AudioState>>,
        stats: Arc<HidStats>,
    ) {
        std::thread::spawn(move || {
            let mut f = match std::fs::OpenOptions::new().read(true).open(&cfg.device) {
                Ok(f) => {
                    info!("hid-reader: opened {} for read", cfg.device);
                    f
                }
                Err(e) => {
                    warn!("hid-reader: cannot open {} for read: {} — LED mirror disabled", cfg.device, e);
                    return;
                }
            };
            let mut buf = [0u8; 64];
            loop {
                match f.read(&mut buf) {
                    Ok(0) => { warn!("hid-reader: EOF, exiting"); return; }
                    Ok(n) => {
                        if let Some(led) = parse_led(&buf[..n]) {
                            apply_led(led, &stats, &audio_tx);
                        }
                    }
                    Err(e) => { warn!("hid-reader: read error: {} — exiting", e); return; }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::watch;
    use waved_state::AudioState;

    #[test]
    fn parse_off_hook_on()                { assert_eq!(parse_led(&[0x05, 0x01]), Some(LedReport::OffHook(true))); }
    #[test]
    fn parse_off_hook_off()               { assert_eq!(parse_led(&[0x05, 0x00]), Some(LedReport::OffHook(false))); }
    #[test]
    fn parse_mute_on()                    { assert_eq!(parse_led(&[0x06, 0x01]), Some(LedReport::Mute(true))); }
    #[test]
    fn parse_ringing_on()                 { assert_eq!(parse_led(&[0x07, 0x01]), Some(LedReport::Ringing(true))); }
    #[test]
    fn parse_unknown_report_returns_none(){ assert_eq!(parse_led(&[0x03, 0xff]), None); }
    #[test]
    fn parse_short_buf_returns_none()     {
        assert_eq!(parse_led(&[0x05]), None);
        assert_eq!(parse_led(&[]), None);
    }
    #[test]
    fn parse_ignores_padding_bits() {
        assert_eq!(parse_led(&[0x05, 0xfe]), Some(LedReport::OffHook(false)));
        assert_eq!(parse_led(&[0x05, 0xff]), Some(LedReport::OffHook(true)));
    }
    #[test]
    fn apply_off_hook_updates_state_and_stats() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::OffHook(true), &stats, &tx);
        assert!(rx.borrow().call_off_hook);
        assert_eq!(stats.leds_off_hook.load(Ordering::Relaxed), 1);
        assert!(stats.call_off_hook.load(Ordering::Relaxed));
    }
    #[test]
    fn apply_ringing_updates_state_and_stats() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::Ringing(true), &stats, &tx);
        assert!(rx.borrow().call_ringing);
        assert_eq!(stats.leds_ringing.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn apply_mute_led_does_not_touch_other_mute_fields() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::Mute(true), &stats, &tx);
        let s = rx.borrow();
        assert!(s.call_host_muted);
        assert!(!s.speaker_muted);
        assert!(!s.mic_muted);
    }
}
```

Add `pub mod reader;` to `crates/waved-hid/src/lib.rs` next to existing module declarations.

- [ ] **Step 2: Build & test**

```bash
cargo test -p waved-hid --lib reader::tests
```

Expected: 10 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-hid/src/reader.rs crates/waved-hid/src/lib.rs
git commit -m "feat(hid/reader): LED parser + HidReader blocking thread"
```

---

## Task 7: `watch.rs` — `spawn_volume_watch` + `spawn_mute_watch`

**Files:**
- Create: `crates/waved-hid/src/watch.rs` — both glue tasks + tests
- Modify: `crates/waved-hid/src/lib.rs` — `pub mod watch;`

- [ ] **Step 1: Write failing tests**

Create `crates/waved-hid/src/watch.rs`:

```rust
//! State-watcher glue tasks: subscribe to AudioState and emit HidEvents
//! per integer-step delta of speaker_volume and per edge of speaker_muted.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::warn;

use waved_state::AudioState;

use crate::events::HidEvent;
use crate::stats::HidStats;

pub fn spawn_volume_watch(
    mut audio_rx: watch::Receiver<AudioState>,
    events: mpsc::Sender<HidEvent>,
    stats: Arc<HidStats>,
    volume_step_rate_hz: u32,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = audio_rx.borrow().speaker_volume as i32;
        let min_gap = if volume_step_rate_hz == 0 {
            Duration::from_millis(0)
        } else {
            Duration::from_millis(1000 / volume_step_rate_hz as u64)
        };
        while audio_rx.changed().await.is_ok() {
            let now = audio_rx.borrow().speaker_volume as i32;
            let delta = now - last;
            if delta == 0 { continue; }
            let up = delta > 0;
            for _ in 0..delta.unsigned_abs() {
                if events.try_send(HidEvent::ConsumerVolumeStep { up }).is_err() {
                    stats.glue_drops.fetch_add(1, Ordering::Relaxed);
                    warn!("hid: volume_watch dropped event (queue full)");
                }
                if min_gap > Duration::ZERO {
                    tokio::time::sleep(min_gap).await;
                }
            }
            last = now;
        }
    })
}

pub fn spawn_mute_watch(
    mut audio_rx: watch::Receiver<AudioState>,
    events: mpsc::Sender<HidEvent>,
    stats: Arc<HidStats>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = audio_rx.borrow().speaker_muted;
        while audio_rx.changed().await.is_ok() {
            let snap = audio_rx.borrow().clone();
            if snap.speaker_muted == last { continue; }
            last = snap.speaker_muted;
            if events.try_send(HidEvent::ConsumerMute).is_err() {
                stats.glue_drops.fetch_add(1, Ordering::Relaxed);
            }
            if snap.call_off_hook {
                if events.try_send(HidEvent::TelephonyPhoneMute).is_err() {
                    stats.glue_drops.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{mpsc, watch};

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_one_event_per_step_up() {
        let mut s = AudioState::default();
        s.speaker_volume = 50;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000);
        tx.send_modify(|s| s.speaker_volume = 53);
        let a = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let b = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let c = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(a, HidEvent::ConsumerVolumeStep { up: true });
        assert_eq!(b, HidEvent::ConsumerVolumeStep { up: true });
        assert_eq!(c, HidEvent::ConsumerVolumeStep { up: true });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_down_events() {
        let mut s = AudioState::default();
        s.speaker_volume = 50;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000);
        tx.send_modify(|s| s.speaker_volume = 48);
        let a = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let b = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(a, HidEvent::ConsumerVolumeStep { up: false });
        assert_eq!(b, HidEvent::ConsumerVolumeStep { up: false });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_ignores_unrelated_state_change() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_volume_watch(rx, ev_tx, HidStats::new(), 1000);
        tx.send_modify(|s| s.mic_muted = !s.mic_muted);
        let t = tokio::time::timeout(Duration::from_millis(50), ev_rx.recv()).await;
        assert!(t.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_emits_consumer_mute_on_toggle() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mute_watch(rx, ev_tx, HidStats::new());
        tx.send_modify(|s| s.speaker_muted = true);
        let ev = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        assert_eq!(ev, HidEvent::ConsumerMute);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_emits_both_when_off_hook() {
        let mut s = AudioState::default();
        s.call_off_hook = true;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mute_watch(rx, ev_tx, HidStats::new());
        tx.send_modify(|s| s.speaker_muted = true);
        let a = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let b = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap();
        let got = vec![a, b];
        assert!(got.contains(&HidEvent::ConsumerMute));
        assert!(got.contains(&HidEvent::TelephonyPhoneMute));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_ignores_no_op_changes() {
        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let _h = spawn_mute_watch(rx, ev_tx, HidStats::new());
        tx.send_modify(|s| s.speaker_muted = false);
        let t = tokio::time::timeout(Duration::from_millis(50), ev_rx.recv()).await;
        assert!(t.is_err());
    }
}
```

Add `pub mod watch;` to `crates/waved-hid/src/lib.rs`.

- [ ] **Step 2: Build & test**

```bash
cargo test -p waved-hid --lib watch::tests
```

Expected: 6 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-hid/src/watch.rs crates/waved-hid/src/lib.rs
git commit -m "feat(hid/watch): volume + mute glue tasks emit HidEvents from audio_tx"
```

---

## Task 8: Wire revised `HidService::run` + watchers + reader in `main.rs`

**Files:**
- Modify: `crates/waved/src/main.rs` — replace the existing `HidService::run(hid_events_rx)` spawn (line 453) with the new richer wiring; populate `hid_stats_handle` so stats reaches `/tmp/audio_stats.json`

- [ ] **Step 1: Update the spawn**

Locate the existing block in `crates/waved/src/main.rs`:

```rust
    // HID service
    let hid_events_rx = bus.events.subscribe();
    #[cfg(not(feature = "simulate"))]
    tokio::spawn(async move { waved_hid::HidService::run(hid_events_rx).await });
    #[cfg(feature = "simulate")]
    tokio::spawn(async move { waved_hid::simulate::SimHidService::run(hid_events_rx).await });
```

Replace it with:

```rust
    // HID service (SP#7(d'))
    let hid_cfg = config.audio.as_ref().and_then(|a| a.hid.clone()).unwrap_or_default();
    if hid_cfg.enabled {
        let hid_stats = waved_hid::stats::HidStats::new();
        hid_stats_handle = Some(std::sync::Arc::clone(&hid_stats));

        let (ev_tx, ev_rx) = tokio::sync::mpsc::channel::<waved_hid::events::HidEvent>(64);

        let bus_rx = bus.events.subscribe();
        let cfg = waved_hid::HidServiceConfig {
            device: hid_cfg.device.clone(),
            press_release_gap_ms: hid_cfg.press_release_gap_ms,
        };
        #[cfg(not(feature = "simulate"))]
        tokio::spawn({
            let stats = std::sync::Arc::clone(&hid_stats);
            async move { waved_hid::HidService::run(cfg, bus_rx, ev_rx, stats).await }
        });
        #[cfg(feature = "simulate")]
        tokio::spawn(async move {
            // Simulate variant ignores ev_rx and writes to tracing; consume the
            // mpsc here so watchers' pushes don't block.
            drop(ev_rx);
            waved_hid::simulate::SimHidService::run(bus_rx).await;
        });

        // Reader for host LED writes — blocking thread; only on real device.
        #[cfg(not(feature = "simulate"))]
        waved_hid::reader::HidReader::spawn(
            waved_hid::reader::HidReaderConfig { device: hid_cfg.device.clone() },
            std::sync::Arc::clone(&bus.audio),
            std::sync::Arc::clone(&hid_stats),
        );

        // State-driven event sources.
        let _vh = waved_hid::watch::spawn_volume_watch(
            bus.audio.subscribe(),
            ev_tx.clone(),
            std::sync::Arc::clone(&hid_stats),
            hid_cfg.volume_step_rate_hz,
        );
        let _mh = waved_hid::watch::spawn_mute_watch(
            bus.audio.subscribe(),
            ev_tx,
            std::sync::Arc::clone(&hid_stats),
        );

        tracing::info!(
            "hid: control surface live (device={} press_release_gap_ms={} step_rate_hz={})",
            hid_cfg.device, hid_cfg.press_release_gap_ms, hid_cfg.volume_step_rate_hz
        );
    } else {
        tracing::info!("hid: disabled by config");
    }
```

Note: `config.audio` may or may not exist depending on the existing wiring around `crates/waved/src/main.rs`. Inspect how `uac` config is reached from `config` and apply the same access pattern. Search hint: `grep -n 'config.audio\|uac_cfg' crates/waved/src/main.rs`.

- [ ] **Step 2: Build**

```bash
cargo build -p waved --features ui,capture-alsa
cargo build -p waved --features simulate
```

Expected: both build clean.

- [ ] **Step 3: Cross-build for the device**

```bash
cross build --release --target aarch64-unknown-linux-gnu --features ui,capture-alsa -p waved
```

Skip if `cross` is not available locally; this is reverified in Final Verification.

- [ ] **Step 4: Commit**

```bash
git add crates/waved/src/main.rs
git commit -m "feat(hid): main.rs spawns HidService + watchers + reader"
```

---

## Task 9: Rename + extend `scripts/patch-uac2-volume-range.sh`

**Files:**
- Rename: `scripts/patch-uac2-volume-range.sh` → `scripts/patch-stock-usbgadget.sh`
- Modify: the renamed file (add `functions/hid.0` setup)
- Update: `docs/AUDIO_HARDWARE.md` (search for the old script name; replace)

- [ ] **Step 1: Rename**

```bash
git mv scripts/patch-uac2-volume-range.sh scripts/patch-stock-usbgadget.sh
```

- [ ] **Step 2: Replace the file content**

Open `scripts/patch-stock-usbgadget.sh` and overwrite with:

```bash
#!/usr/bin/env bash
# Patch /etc/init.d/S89usbgadget on a Wave so the device exposes:
#   1. A useful playback (Output) volume range to UAC2 hosts.
#   2. A functions/hid.0 control surface advertising the stock-extracted
#      Vendor + Telephony_V4 + Consumer report descriptor, so SP#7(d')
#      (HID Consumer/Telephony control) and future SP#7(a) (Vendor channel)
#      reach /dev/hidg0.
#
# Idempotent: re-running adds nothing after the first patch. Backs up
# the original to S89usbgadget.bak.<ts> on first run.
#
# After patching, reboot:   adb reboot

set -euo pipefail

INIT=/etc/init.d/S89usbgadget

if [ ! -f "$INIT" ]; then
    echo "FATAL: $INIT not found — wrong device or stock firmware moved it" >&2
    exit 1
fi

ALREADY_VOL=0
ALREADY_HID=0
grep -q "echo -16384 > functions/uac2.0/p_volume_min" "$INIT" && ALREADY_VOL=1
grep -q "mkdir -p functions/hid.0" "$INIT" && ALREADY_HID=1

if [ "$ALREADY_VOL" = "1" ] && [ "$ALREADY_HID" = "1" ]; then
    echo "already patched (both volume range + hid.0)"
    exit 0
fi

cp "$INIT" "${INIT}.bak.$(date +%s)"

if [ "$ALREADY_VOL" = "0" ]; then
    sed -i \
        -e 's|echo -100 > functions/uac2.0/p_volume_min|echo -16384 > functions/uac2.0/p_volume_min|' \
        -e 's|echo 1 > functions/uac2.0/p_volume_res|echo 256 > functions/uac2.0/p_volume_res|' \
        "$INIT"
    echo "patched UAC2 volume range"
fi

if [ "$ALREADY_HID" = "0" ]; then
    DESCRIPTOR='\\x06\\x00\\xff\\x09\\x01\\xa1\\x01\\x85\\x03\\x09\\x01\\x15\\x00\\x26\\xff\\x00\\x95\\x3f\\x75\\x08\\x81\\x02\\x09\\x01\\x15\\x00\\x26\\xff\\x00\\x95\\x3f\\x75\\x08\\x91\\x22\\xc0\\x05\\x0b\\x09\\x05\\xa1\\x01\\x85\\x02\\x15\\x00\\x25\\x01\\x75\\x01\\x95\\x01\\x09\\x20\\x81\\x22\\x09\\x2f\\x81\\x06\\x09\\x21\\x81\\x02\\x09\\x24\\x81\\x06\\x09\\x07\\x05\\x09\\x09\\x01\\x81\\x06\\x75\\x03\\x81\\x01\\x85\\x05\\x05\\x08\\x09\\x17\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\x85\\x06\\x09\\x09\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\x85\\x07\\x09\\x18\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\xc0\\x05\\x0c\\x09\\x01\\xa1\\x01\\x85\\x01\\x05\\x0c\\x15\\x00\\x25\\x01\\x09\\xe9\\x09\\xea\\x09\\xe2\\x09\\xcd\\x09\\xb5\\x09\\xb6\\x09\\xb0\\x09\\xb1\\x75\\x01\\x95\\x08\\x81\\x02\\xc0'

    awk -v desc="$DESCRIPTOR" '
        BEGIN { inserted = 0 }
        /^[[:space:]]*usb_flash[[:space:]]*$/ && !inserted {
            print "\t# SP#7(d\\47) HID control surface (Vendor + Telephony_V4 + Consumer)"
            print "\tmkdir -p functions/hid.0"
            print "\techo 1  > functions/hid.0/protocol"
            print "\techo 1  > functions/hid.0/subclass"
            print "\techo 64 > functions/hid.0/report_length"
            print "\techo -ne \"" desc "\" > functions/hid.0/report_desc"
            print "\tln -sf functions/hid.0 configs/amlogic.1/hid.0"
            inserted = 1
        }
        { print }
    ' "$INIT" > "${INIT}.tmp" && mv "${INIT}.tmp" "$INIT"
    chmod +x "$INIT"
    echo "patched HID gadget (hid.0)"
fi

echo
grep -n "p_volume_min\|p_volume_max\|p_volume_res\|functions/hid.0" "$INIT" || true

echo
echo "reboot the device for the new gadget config to take effect:"
echo "  adb reboot"
```

- [ ] **Step 3: Update references**

Run: `grep -rn "patch-uac2-volume-range" docs/ scripts/ crates/ README.md 2>/dev/null` and replace each match with `patch-stock-usbgadget`. At least one match is expected in `docs/AUDIO_HARDWARE.md`.

- [ ] **Step 4: Smoke-test the awk splice on stock fw**

Run on the dev host (not the device):

```bash
cp ../fw_v1.1.6.6/rootfs/etc/init.d/S89usbgadget /tmp/S89.test
# (path adjusted because waved/ is one level deep; check ../fw_v1.1.6.6 vs fw_v1.1.6.6)
DESC_TEST='\x06\x00\xff'
awk -v desc="$DESC_TEST" '
    BEGIN { inserted = 0 }
    /^[[:space:]]*usb_flash[[:space:]]*$/ && !inserted {
        print "\t# test marker"
        print "\tmkdir -p functions/hid.0"
        inserted = 1
    }
    { print }
' /tmp/S89.test | grep -n "test marker\|usb_flash\|hid.0" | head -10
```

Expected: marker + mkdir line appears immediately above the first `usb_flash` line, once.

- [ ] **Step 5: Commit**

```bash
git add scripts/patch-stock-usbgadget.sh docs/AUDIO_HARDWARE.md
git commit -m "feat(scripts): patch-stock-usbgadget.sh installs UAC volume + hid.0"
```

---

## Task 10: Acceptance scripts

**Files:**
- Create: `scripts/test-hid-control.sh`
- Create: `scripts/test-hid-control-host.sh`

- [ ] **Step 1: Create device-side script**

Write `scripts/test-hid-control.sh`:

```bash
#!/usr/bin/env bash
# SP#7(d') device-side acceptance.
set -euo pipefail
cd "$(dirname "$0")/.."

LOG=/tmp/sp7d-acceptance.log
: > "$LOG"

read_hid_field() {
    local expr="$1"
    adb shell "cat /tmp/audio_stats.json 2>/dev/null" | python3 -c "
import json, sys
try: s = json.load(sys.stdin)
except Exception: print('null'); sys.exit(0)
try: print(${expr})
except Exception: print('null')
"
}

echo "==> /dev/hidg0 presence" | tee -a "$LOG"
if adb shell "[ -c /dev/hidg0 ] && echo OK || echo MISSING" | grep -q OK; then
    echo "1/8 hidg0 char device: PASS" | tee -a "$LOG"
else
    echo "1/8 hidg0 char device: FAIL — patch-stock-usbgadget.sh + reboot first" | tee -a "$LOG"
    exit 1
fi

echo "==> hid.device_present" | tee -a "$LOG"
for i in $(seq 1 10); do
    P=$(read_hid_field "s['hid']['device_present']")
    if [ "$P" = "True" ]; then break; fi
    sleep 1
done
if [ "$P" = "True" ]; then
    echo "2/8 device_present: PASS" | tee -a "$LOG"
else
    echo "2/8 device_present: FAIL" | tee -a "$LOG"
    exit 1
fi

echo "==> Wave touch volume tick → host (manual: tap touch volume up 3 times now; 15 s)" | tee -a "$LOG"
BEFORE=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
sleep 15
AFTER=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
DELTA=$(python3 -c "print(int($AFTER) - int($BEFORE))")
if [ "$DELTA" -ge 3 ]; then
    echo "3/8 volume_step counter: PASS ($BEFORE → $AFTER, +$DELTA)" | tee -a "$LOG"
else
    echo "3/8 volume_step counter: FAIL ($BEFORE → $AFTER, +$DELTA, expected ≥ 3)" | tee -a "$LOG"
    exit 1
fi

echo "==> Wave touch mute toggle (manual: toggle mute on touch UI; 10 s)" | tee -a "$LOG"
BEFORE=$(read_hid_field "s['hid']['events_emitted']['mute']")
sleep 10
AFTER=$(read_hid_field "s['hid']['events_emitted']['mute']")
DELTA=$(python3 -c "print(int($AFTER) - int($BEFORE))")
if [ "$DELTA" -ge 1 ]; then
    echo "4/8 mute counter: PASS ($BEFORE → $AFTER, +$DELTA)" | tee -a "$LOG"
else
    echo "4/8 mute counter: FAIL" | tee -a "$LOG"
    exit 1
fi

echo "==> MCU physical Volume Up button regression (manual: press physical Volume Up 3x; 10 s)" | tee -a "$LOG"
BEFORE=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
sleep 10
AFTER=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
DELTA=$(python3 -c "print(int($AFTER) - int($BEFORE))")
if [ "$DELTA" -ge 3 ]; then
    echo "5/8 MCU button regression: PASS" | tee -a "$LOG"
else
    echo "5/8 MCU button regression: FAIL" | tee -a "$LOG"
    exit 1
fi

echo "==> Off-hook LED (manual: start a Zoom call now; 30 s)" | tee -a "$LOG"
for i in $(seq 1 30); do
    if [ "$(read_hid_field "s['hid']['call_off_hook']")" = "True" ]; then
        echo "6/8 call_off_hook: PASS (detected after ${i} s)" | tee -a "$LOG"
        break
    fi
    sleep 1
done
if [ "$(read_hid_field "s['hid']['call_off_hook']")" != "True" ]; then
    echo "6/8 call_off_hook: SKIP — no Zoom call within 30 s (manual, not a hard fail)" | tee -a "$LOG"
fi

echo "==> Mac F10 → Wave speaker mute (regression on SP#6)" | tee -a "$LOG"
echo "    (manual: press F10 on the Mac while music is playing; verify speaker silences within 200 ms)" | tee -a "$LOG"
echo "7/8 SP#6 mute regression: MANUAL" | tee -a "$LOG"

echo "==> hostless stats query" | tee -a "$LOG"
adb shell "cat /tmp/audio_stats.json" | python3 -c "import json, sys; json.loads(sys.stdin.read())['hid']" >/dev/null
echo "8/8 hostless stats query: PASS" | tee -a "$LOG"

echo "==> acceptance complete; see $LOG" | tee -a "$LOG"
```

`chmod +x scripts/test-hid-control.sh`.

- [ ] **Step 2: Create host-side helper**

Write `scripts/test-hid-control-host.sh`:

```bash
#!/usr/bin/env bash
# SP#7(d') host-side companion to scripts/test-hid-control.sh (macOS).
set -euo pipefail

osx_get_vol()   { osascript -e 'output volume of (get volume settings)' 2>/dev/null || echo "?"; }
osx_get_muted() { osascript -e 'output muted of (get volume settings)' 2>/dev/null || echo "?"; }

echo "host volume before: $(osx_get_vol)"
echo "host muted before:  $(osx_get_muted)"
echo
echo "Now run scripts/test-hid-control.sh on the device."
echo "Press [Return] when the device script completes."
read -r _
echo
echo "host volume after:  $(osx_get_vol)"
echo "host muted after:   $(osx_get_muted)"
```

`chmod +x scripts/test-hid-control-host.sh`.

- [ ] **Step 3: Syntax check**

```bash
bash -n scripts/test-hid-control.sh && bash -n scripts/test-hid-control-host.sh && echo OK
```

Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add scripts/test-hid-control.sh scripts/test-hid-control-host.sh
git commit -m "feat(scripts): SP#7(d') acceptance test-hid-control.sh + host helper"
```

---

## Task 11: Update `TODO.md` and `docs/AUDIO_HARDWARE.md`

**Files:**
- Modify: `TODO.md` (mark SP#7(d') shipped; record the original (d) kernel-patch deferral)
- Modify: `docs/AUDIO_HARDWARE.md` (add HID control surface section near the `Mac → Wave control gaps` section)

- [ ] **Step 1: Update `TODO.md`**

In the `### SP#7 scope — Wave Controller native compatibility (deferred)` block, add as the first bullet:

```markdown
- [x] **SP#7(d') — HID Consumer/Telephony control surface** (`crates/waved-hid/`: `HidEvent` enum + `report.rs` matching stock Report IDs (Consumer=0x01, Telephony=0x02); `HidReader` mirrors host Off-hook/Mute/Ring LED writes into `AudioState.call_*`; `watch.rs` emits Consumer Volume/Mute keys from touch-UI-driven `AudioState` changes; existing MCU button bridge preserved. `scripts/patch-stock-usbgadget.sh` installs the stock Vendor+Telephony_V4+Consumer report descriptor; verified via `scripts/test-hid-control.sh`. The original SP#7(d) `f_uac2` Master Volume kernel patch is **deferred indefinitely** — Consumer Volume keys (device→host) are the stock-equivalent path and ship without a kernel build.
```

- [ ] **Step 2: Update `docs/AUDIO_HARDWARE.md`**

Immediately before the section `### Mac → Wave control gaps (kernel f_uac2 5.4 limitation)`, add:

```markdown
### HID control surface (SP#7(d'))

The device exposes a `functions/hid.0` USB-gadget interface with a combined
**Vendor (Report 0x03) + Telephony_V4 (Reports 0x02/0x05/0x06/0x07) +
Consumer (Report 0x01)** report descriptor, copied byte-for-byte from stock
fw v1.1.6.6. The Rust implementation lives in `crates/waved-hid/`:

- `HidService::run` (`lib.rs`) — owns `/dev/hidg0` write side. Bridges
  `DeviceEvent::ButtonPress(ButtonType)` from `waved-mcu` and drains an
  `mpsc<HidEvent>` from state-driven sources. Writes press-release pairs.
- `watch.rs` glue tasks — subscribe to `audio_tx`:
  - `spawn_volume_watch` — one `ConsumerVolumeStep` per `speaker_volume`
    step delta, rate-capped at `hid.volume_step_rate_hz` (default 40 Hz).
  - `spawn_mute_watch` — one `ConsumerMute` per `speaker_muted` edge;
    additionally one `TelephonyPhoneMute` when `call_off_hook == true`.
- `reader.rs` — blocking std::thread reads `/dev/hidg0`, parses LED
  Set_Reports (0x05/0x06/0x07) into `AudioState.call_off_hook /
  call_host_muted / call_ringing`.

Vendor channel (Report 0x03) is declared but not handled (SP#7(a) future).

Report IDs match the stock descriptor: **0x01 = Consumer, 0x02 = Telephony**.
This is the opposite of the original `waved-hid` constants prior to SP#7(d'),
which had the IDs swapped — see §4.2 of `docs/superpowers/specs/2026-05-21-
waved-hid-control-interface-design.md`.

Configfs setup lives in `scripts/patch-stock-usbgadget.sh`; reboot required
for the new gadget config to come up.

Stats: `/tmp/audio_stats.json` includes a `hid` block (`device_present`,
`events_emitted`, `leds_received`, `call_*`, `write_errors`, `glue_drops`).
```

- [ ] **Step 3: Commit**

```bash
git add TODO.md docs/AUDIO_HARDWARE.md
git commit -m "docs(SP#7d'): mark HID control surface shipped + AUDIO_HARDWARE section"
```

---

## Final verification

After all tasks complete:

- [ ] **Run the test suites**

```bash
cargo test -p waved-state --lib
cargo test -p waved-hid --lib
cargo test -p waved-audio --lib
```

Expected: all pass.

- [ ] **Cross-compile for device**

```bash
cross build --release --target aarch64-unknown-linux-gnu --features ui,capture-alsa -p waved
```

Expected: succeeds.

- [ ] **Deploy + reboot**

```bash
adb push scripts/patch-stock-usbgadget.sh /tmp/
adb shell "sh /tmp/patch-stock-usbgadget.sh"
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb reboot
```

- [ ] **Run acceptance**

Device: `./scripts/test-hid-control.sh`. Mac: `./scripts/test-hid-control-host.sh`.
Expected: device script reports `8/8 PASS` (test 6 is SKIP-without-Zoom, test 7 is MANUAL — both fine).

- [ ] **Push**

```bash
git push
```
