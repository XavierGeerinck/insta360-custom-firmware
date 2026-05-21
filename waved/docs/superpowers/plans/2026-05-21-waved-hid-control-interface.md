# SP#7(d') — HID Consumer/Telephony Control Interface — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `functions/hid.0` USB-gadget HID interface (Consumer + Telephony pages) so Wave touch UI volume/mute mirror to the host via media keys, and host LED writes for Zoom/Teams call state surface in `AudioState`.

**Architecture:** New `crates/waved-audio/src/hid/` module (`mod.rs`, `writer.rs`, `reader.rs`) mirroring the SP#6 `uac/` layout. Two tokio tasks share `/dev/hidg0`: `HidWriter` consumes an `mpsc<HidEvent>` and emits press-release pairs; `HidReader` parses host Set_Report bytes and updates new `AudioState.call_*` bool fields via the existing `audio_tx.send_modify` path. Two glue subscribers on `audio_tx` (`volume_watch`, `mute_watch`) translate `speaker_volume` / `speaker_muted` deltas into `HidEvent`s. `scripts/patch-stock-usbgadget.sh` extends the existing configfs patcher to install `functions/hid.0` with the stock-extracted Vendor+Telephony_V4+Consumer report descriptor.

**Tech Stack:** Rust 1.85 (edition 2021), tokio (existing features `sync rt rt-multi-thread macros time`), serde, tracing, std::fs for /dev/hidg0 blocking I/O inside dedicated tasks (no new tokio feature flags needed).

**Spec reference:** `docs/superpowers/specs/2026-05-21-waved-hid-control-interface-design.md` (commit `bf8b713`).

---

## Task 1: Add `call_*` fields to `AudioState`

**Files:**
- Modify: `crates/waved-state/src/lib.rs:16-40` (AudioState struct + Default impl)
- Test: `crates/waved-state/src/lib.rs` (existing `#[cfg(test)]` block)

- [ ] **Step 1: Write the failing test**

Add inside the existing `#[cfg(test)] mod tests { ... }` block in `crates/waved-state/src/lib.rs` (locate the existing tests via `grep -n 'mod tests' crates/waved-state/src/lib.rs`):

```rust
#[test]
fn audio_state_default_call_fields_false() {
    let s = AudioState::default();
    assert!(!s.call_off_hook);
    assert!(!s.call_ringing);
    assert!(!s.call_host_muted);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p waved-state --lib audio_state_default_call_fields_false`
Expected: FAIL — `no field call_off_hook on type 'AudioState'`.

- [ ] **Step 3: Add the fields**

In `crates/waved-state/src/lib.rs`, add to the `AudioState` struct (after `playback_preset`):

```rust
    /// Host wrote Report 0x05 (Off-hook LED) — `true` while the host
    /// signals "you are in a call". Plumbed by `waved-audio::hid::HidReader`.
    pub call_off_hook: bool,
    /// Host wrote Report 0x07 (Ring LED) — `true` while the host signals
    /// "you have an incoming call".
    pub call_ringing: bool,
    /// Host wrote Report 0x06 (Mute LED) — `true` while the host signals
    /// "the call is muted host-side". Informational only.
    pub call_host_muted: bool,
```

And inside `impl Default for AudioState`, add the three matching defaults (locate via `Default for AudioState` in the same file):

```rust
            call_off_hook: false,
            call_ringing: false,
            call_host_muted: false,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p waved-state --lib audio_state_default_call_fields_false`
Expected: PASS.

- [ ] **Step 5: Run the full waved-state test suite to confirm no regressions**

Run: `cargo test -p waved-state --lib`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/waved-state/src/lib.rs
git commit -m "feat(state): add call_off_hook/call_ringing/call_host_muted to AudioState"
```

---

## Task 2: Add `HidConfig` to `audio_config.rs`

**Files:**
- Modify: `crates/waved-audio/src/audio_config.rs` (add `HidConfig` block alongside `UacConfig`, add it to top-level `AudioConfig`)
- Test: `crates/waved-audio/src/audio_config.rs` (existing `#[cfg(test)]` block)

- [ ] **Step 1: Write the failing test**

Add inside the existing `#[cfg(test)] mod tests` block in `crates/waved-audio/src/audio_config.rs` (find it via `grep -n 'mod tests' crates/waved-audio/src/audio_config.rs`):

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

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p waved-audio --lib audio_config::tests::hid_config`
Expected: FAIL — `no field hid on type 'AudioConfig'`.

- [ ] **Step 3: Add `HidConfig` and the optional `hid` field on `AudioConfig`**

In `crates/waved-audio/src/audio_config.rs`, immediately after the `impl Default for UacConfig { ... }` block (find via `grep -n 'impl Default for UacConfig' crates/waved-audio/src/audio_config.rs`), add:

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

Locate the top-level `pub struct AudioConfig` declaration (in the same file). Add the `hid` field next to the existing optional `uac` field (look for `pub uac: Option<UacConfig>` — there should be a `#[serde(default)] pub uac: Option<UacConfig>,` line):

```rust
    #[serde(default)]
    pub hid: Option<HidConfig>,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p waved-audio --lib audio_config::tests::hid_config`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/audio_config.rs
git commit -m "feat(audio_config): add HidConfig block (default enabled, /dev/hidg0)"
```

---

## Task 3: Scaffold the `hid` module — `HidEvent`, `HidStats`, `HidStatsSnapshot`

**Files:**
- Create: `crates/waved-audio/src/hid/mod.rs`
- Modify: `crates/waved-audio/src/lib.rs` (add `pub mod hid;`)

- [ ] **Step 1: Create `crates/waved-audio/src/hid/mod.rs` with the scaffold and inline tests**

Write the complete file `crates/waved-audio/src/hid/mod.rs`:

```rust
//! HID Consumer / Telephony control surface — SP#7(d').
//!
//! Exposes a `functions/hid.0` USB-gadget interface so the device emits
//! Consumer media keys (Volume Up/Down, Mute, Play/Pause, Next, Prev) and
//! Telephony Phone-Mute to the host, and receives Telephony LED writes
//! (Off-hook, Mute, Ring) which mutate `AudioState.call_*`.
//!
//! Vendor channel (Report 0x03) is declared in the report descriptor (copied
//! verbatim from stock) but **not** wired here — it is the deliverable of
//! SP#7(a) (Wave Controller protocol probe).
//!
//! See `docs/superpowers/specs/2026-05-21-waved-hid-control-interface-design.md`.

pub mod reader;
pub mod writer;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Events posted from glue tasks (volume_watch, mute_watch) to `HidWriter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidEvent {
    /// Report 0x01 (Consumer) — one volume step. `HidWriter` emits a
    /// press-then-release pair on the appropriate bit.
    ConsumerVolumeStep { up: bool },
    /// Report 0x01 bit2 — Mute key. Press-release pair.
    ConsumerMute,
    /// Report 0x01 bit3 — Play/Pause toggle.
    ConsumerPlayPause,
    /// Report 0x01 bit4 — Scan Next Track.
    ConsumerNext,
    /// Report 0x01 bit5 — Scan Previous Track.
    ConsumerPrev,
    /// Report 0x02 bit1 — Telephony Phone Mute.
    TelephonyPhoneMute,
}

/// Atomic counters surfaced into `/tmp/audio_stats.json` via `stats_collector`.
#[derive(Debug, Default)]
pub struct HidStats {
    /// `true` once `/dev/hidg0` has been opened successfully at least once.
    pub device_present: AtomicBool,
    /// Counters of writes per event variant.
    pub events_volume_step: AtomicU64,
    pub events_mute: AtomicU64,
    pub events_phone_mute: AtomicU64,
    pub events_play_pause: AtomicU64,
    pub events_next: AtomicU64,
    pub events_prev: AtomicU64,
    /// Counters of LED Set_Report writes received from the host.
    pub leds_off_hook: AtomicU64,
    pub leds_ringing: AtomicU64,
    pub leds_mute: AtomicU64,
    /// State mirrors of the latest LED report bit.
    pub call_off_hook: AtomicBool,
    pub call_ringing: AtomicBool,
    pub call_host_muted: AtomicBool,
    /// Counters of write failures and dropped mpsc events.
    pub write_errors: AtomicU64,
    pub glue_drops: AtomicU64,
}

impl HidStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

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
    fn stats_snapshot_starts_zero() {
        let s = HidStats::default();
        let snap = s.snapshot();
        assert!(!snap.device_present);
        assert_eq!(snap.events_emitted.volume_step, 0);
        assert_eq!(snap.leds_received.off_hook, 0);
        assert!(!snap.call_off_hook);
    }

    #[test]
    fn stats_snapshot_reflects_writes() {
        let s = HidStats::default();
        s.device_present.store(true, Ordering::Relaxed);
        s.events_volume_step.fetch_add(42, Ordering::Relaxed);
        s.leds_off_hook.fetch_add(3, Ordering::Relaxed);
        s.call_off_hook.store(true, Ordering::Relaxed);
        let snap = s.snapshot();
        assert!(snap.device_present);
        assert_eq!(snap.events_emitted.volume_step, 42);
        assert_eq!(snap.leds_received.off_hook, 3);
        assert!(snap.call_off_hook);
    }
}
```

- [ ] **Step 2: Create the empty placeholder files referenced by `mod` declarations**

These will be filled in later tasks. For now just stub them so the crate compiles:

`crates/waved-audio/src/hid/writer.rs`:

```rust
//! HID Writer — owns `/dev/hidg0` write side, consumes `HidEvent`s.
//! Implementation in Task 5/6.

#![allow(dead_code)]
```

`crates/waved-audio/src/hid/reader.rs`:

```rust
//! HID Reader — owns `/dev/hidg0` read side, parses LED reports.
//! Implementation in Task 7.

#![allow(dead_code)]
```

- [ ] **Step 3: Wire the module into `crates/waved-audio/src/lib.rs`**

Locate the `pub mod uac;` line in `crates/waved-audio/src/lib.rs` (use `grep -n 'pub mod uac' crates/waved-audio/src/lib.rs`). Add immediately after it:

```rust
pub mod hid;
```

- [ ] **Step 4: Build the crate and run the new tests**

Run: `cargo build -p waved-audio` and then `cargo test -p waved-audio --lib hid::tests`
Expected: build succeeds; both `stats_snapshot_starts_zero` and `stats_snapshot_reflects_writes` PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/hid/ crates/waved-audio/src/lib.rs
git commit -m "feat(hid): scaffold module + HidEvent + HidStats"
```

---

## Task 4: Wire `HidStats` into `stats_collector`

**Files:**
- Modify: `crates/waved-audio/src/stats_collector.rs` (add `hid: Option<HidStatsSnapshot>` to `AudioStats`; thread `hid_stats: Option<Arc<HidStats>>` through `run` and `compute_stats_with_processed`)
- Modify: `crates/waved/src/main.rs` (forward `hid_stats` into the `stats_collector::run` call — same pattern as `uac_stats_handle`)

- [ ] **Step 1: Write the failing test**

Add at the bottom of `crates/waved-audio/src/stats_collector.rs` inside an existing `#[cfg(test)] mod tests` block (or create one if none — search via `grep -n 'mod tests' crates/waved-audio/src/stats_collector.rs`; if absent, append the block below at the end of the file):

```rust
#[cfg(test)]
mod tests_hid_wiring {
    use super::*;
    use crate::hid::HidStats;

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

- [ ] **Step 2: Run the test (it must fail to compile because `hid` is not a field yet)**

Run: `cargo test -p waved-audio --lib stats_collector::tests_hid_wiring`
Expected: FAIL — `no field hid on type 'AudioStats'`.

- [ ] **Step 3: Add the `hid` field to `AudioStats`**

In `crates/waved-audio/src/stats_collector.rs`, locate the `pub struct AudioStats` declaration (it already has a `pub uac: Option<...>` line under a `#[cfg(feature = "capture-alsa")]` gate). Add immediately after the `uac` field:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hid: Option<crate::hid::HidStatsSnapshot>,
```

(No cfg gate — HID compiles unconditionally.)

- [ ] **Step 4: Thread `hid_stats` through `run`**

In the same file, locate `pub async fn run(...)` (around line 65). Add a new parameter at the end:

```rust
    hid_stats: Option<Arc<crate::hid::HidStats>>,
```

Inside `run`, find where it calls `compute_stats_with_processed(...)` (search for `compute_stats_with_processed`). Add `hid_stats.as_deref()` as the last argument — same pattern as the existing `uac_stats.as_deref()`.

- [ ] **Step 5: Add `hid_stats` parameter to `compute_stats_with_processed`**

In the same file, locate `pub fn compute_stats_with_processed(...)`. Append a new parameter at the end (no cfg gate):

```rust
    hid_stats: Option<&crate::hid::HidStats>,
```

Inside that function, just before the `AudioStats { ... }` literal, add:

```rust
    let hid = hid_stats.map(|s| s.snapshot());
```

Then in the `AudioStats { ... }` literal, add the field:

```rust
        hid,
```

- [ ] **Step 6: Update the no-processed wrapper**

Locate `pub fn compute_stats(...)` (the wrapper that calls `compute_stats_with_processed`). Add a trailing `None` to the inner call — same shape as the existing `None` for `uac_stats`.

- [ ] **Step 7: Forward from `main.rs`**

In `crates/waved/src/main.rs`, find the existing block where `uac_stats_handle` is threaded into the `stats_collector::run(...)` call (use `grep -n 'stats_collector::run' crates/waved/src/main.rs`). Above the existing `uac_stats_handle` definition (line ~227), add:

```rust
    let mut hid_stats_handle: Option<Arc<waved_audio::hid::HidStats>> = None;
```

Then locate the `stats_collector::run(...)` invocation and add `hid_stats_handle.clone()` as the last argument — same shape as the existing `uac_stats_handle.clone()`.

- [ ] **Step 8: Build and test**

Run: `cargo build -p waved` and `cargo test -p waved-audio --lib stats_collector::tests_hid_wiring`
Expected: build succeeds; both tests PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/waved-audio/src/stats_collector.rs crates/waved/src/main.rs
git commit -m "feat(hid): wire HidStats into stats_collector and /tmp/audio_stats.json"
```

---

## Task 5: Pure-function serialization in `writer.rs`

Pure, unit-testable. No I/O.

**Files:**
- Modify: `crates/waved-audio/src/hid/writer.rs` (replace stub)

- [ ] **Step 1: Replace the stub with pure serialization functions + tests**

Write the complete file `crates/waved-audio/src/hid/writer.rs`:

```rust
//! HID Writer — pure serialization plus a tokio task that drives
//! `/dev/hidg0` with press-release pairs.

#![allow(dead_code)]

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{HidEvent, HidStats};

/// Report 0x01 — Consumer page bit map.
const CONSUMER_VOLUME_UP:   u8 = 1 << 0;
const CONSUMER_VOLUME_DOWN: u8 = 1 << 1;
const CONSUMER_MUTE:        u8 = 1 << 2;
const CONSUMER_PLAY_PAUSE:  u8 = 1 << 3;
const CONSUMER_NEXT:        u8 = 1 << 4;
const CONSUMER_PREV:        u8 = 1 << 5;

/// Report 0x02 — Telephony page bit map.
const TELEPHONY_PHONE_MUTE: u8 = 1 << 1;

/// Returns `(press_bytes, release_bytes)` to write for a single `HidEvent`.
/// Both buffers include the leading Report ID byte.
pub fn event_to_press_release(ev: HidEvent) -> ([u8; 2], [u8; 2]) {
    match ev {
        HidEvent::ConsumerVolumeStep { up: true }  => ([0x01, CONSUMER_VOLUME_UP],   [0x01, 0x00]),
        HidEvent::ConsumerVolumeStep { up: false } => ([0x01, CONSUMER_VOLUME_DOWN], [0x01, 0x00]),
        HidEvent::ConsumerMute                     => ([0x01, CONSUMER_MUTE],        [0x01, 0x00]),
        HidEvent::ConsumerPlayPause                => ([0x01, CONSUMER_PLAY_PAUSE],  [0x01, 0x00]),
        HidEvent::ConsumerNext                     => ([0x01, CONSUMER_NEXT],        [0x01, 0x00]),
        HidEvent::ConsumerPrev                     => ([0x01, CONSUMER_PREV],        [0x01, 0x00]),
        HidEvent::TelephonyPhoneMute               => ([0x02, TELEPHONY_PHONE_MUTE], [0x02, 0x00]),
    }
}

/// Bump the counter on `stats` matching `ev`.
pub fn bump_event_stat(stats: &HidStats, ev: HidEvent) {
    match ev {
        HidEvent::ConsumerVolumeStep { .. } => { stats.events_volume_step.fetch_add(1, Ordering::Relaxed); }
        HidEvent::ConsumerMute              => { stats.events_mute.fetch_add(1, Ordering::Relaxed); }
        HidEvent::ConsumerPlayPause         => { stats.events_play_pause.fetch_add(1, Ordering::Relaxed); }
        HidEvent::ConsumerNext              => { stats.events_next.fetch_add(1, Ordering::Relaxed); }
        HidEvent::ConsumerPrev              => { stats.events_prev.fetch_add(1, Ordering::Relaxed); }
        HidEvent::TelephonyPhoneMute        => { stats.events_phone_mute.fetch_add(1, Ordering::Relaxed); }
    }
}

/// Task 6 will implement this — spawns the writer loop.
pub struct HidWriter {
    _phantom: (),
}

#[derive(Debug, Clone)]
pub struct HidWriterConfig {
    pub device: String,
    pub press_release_gap_ms: u64,
}

impl HidWriter {
    /// Stub — see Task 6 for the real impl.
    pub fn spawn(
        _cfg: HidWriterConfig,
        _events: mpsc::Receiver<HidEvent>,
        _stats: Arc<HidStats>,
    ) -> Self {
        Self { _phantom: () }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_up_press_release() {
        let (p, r) = event_to_press_release(HidEvent::ConsumerVolumeStep { up: true });
        assert_eq!(p, [0x01, 0b0000_0001]);
        assert_eq!(r, [0x01, 0x00]);
    }

    #[test]
    fn volume_down_press_release() {
        let (p, r) = event_to_press_release(HidEvent::ConsumerVolumeStep { up: false });
        assert_eq!(p, [0x01, 0b0000_0010]);
        assert_eq!(r, [0x01, 0x00]);
    }

    #[test]
    fn consumer_mute_press_release() {
        let (p, r) = event_to_press_release(HidEvent::ConsumerMute);
        assert_eq!(p, [0x01, 0b0000_0100]);
        assert_eq!(r, [0x01, 0x00]);
    }

    #[test]
    fn telephony_phone_mute_press_release() {
        let (p, r) = event_to_press_release(HidEvent::TelephonyPhoneMute);
        assert_eq!(p, [0x02, 0b0000_0010]);
        assert_eq!(r, [0x02, 0x00]);
    }

    #[test]
    fn bump_event_stat_increments_right_counter() {
        let s = HidStats::default();
        bump_event_stat(&s, HidEvent::ConsumerVolumeStep { up: true });
        bump_event_stat(&s, HidEvent::ConsumerMute);
        bump_event_stat(&s, HidEvent::TelephonyPhoneMute);
        assert_eq!(s.events_volume_step.load(Ordering::Relaxed), 1);
        assert_eq!(s.events_mute.load(Ordering::Relaxed), 1);
        assert_eq!(s.events_phone_mute.load(Ordering::Relaxed), 1);
        assert_eq!(s.events_play_pause.load(Ordering::Relaxed), 0);
    }
}
```

(Note: the top-level `use std::io::Write;` and `use ... Duration;` are unused at this point but referenced by Task 6 — `#![allow(dead_code)]` keeps the build clean.)

- [ ] **Step 2: Run the tests**

Run: `cargo test -p waved-audio --lib hid::writer::tests`
Expected: all 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/hid/writer.rs
git commit -m "feat(hid/writer): event-to-bytes serializer + per-event stat bump"
```

---

## Task 6: `HidWriter::spawn` — the actual writer task

**Files:**
- Modify: `crates/waved-audio/src/hid/writer.rs` (replace the stub `spawn` with the real one)

- [ ] **Step 1: Replace the stub `HidWriter::spawn`**

In `crates/waved-audio/src/hid/writer.rs`, replace the stub implementation of `HidWriter::spawn` with:

```rust
impl HidWriter {
    /// Spawn a tokio task that drains `events` and writes each as a
    /// press-release pair to `cfg.device`. The task exits cleanly if the
    /// device cannot be opened (logs WARN, marks `stats.device_present = false`).
    pub fn spawn(
        cfg: HidWriterConfig,
        mut events: mpsc::Receiver<HidEvent>,
        stats: Arc<HidStats>,
    ) -> Self {
        tokio::spawn(async move {
            // Open the device once at startup. `/dev/hidg0` is a character
            // device — std::fs::OpenOptions().write(true) is sufficient.
            // Held inside an Option so reopen-on-error is straightforward.
            let mut dev: Option<std::fs::File> = match std::fs::OpenOptions::new()
                .write(true)
                .open(&cfg.device)
            {
                Ok(f) => {
                    stats.device_present.store(true, Ordering::Relaxed);
                    info!("hid-writer: opened {}", cfg.device);
                    Some(f)
                }
                Err(e) => {
                    warn!(
                        "hid-writer: cannot open {}: {} — HID writes disabled",
                        cfg.device, e
                    );
                    stats.device_present.store(false, Ordering::Relaxed);
                    None
                }
            };

            let gap = Duration::from_millis(cfg.press_release_gap_ms);

            while let Some(ev) = events.recv().await {
                let (press, release) = event_to_press_release(ev);
                let Some(f) = dev.as_mut() else {
                    // Drop the event; counter not bumped since we did not write.
                    stats.write_errors.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                if let Err(e) = f.write_all(&press) {
                    warn!("hid-writer: write press failed: {} — disabling", e);
                    stats.write_errors.fetch_add(1, Ordering::Relaxed);
                    stats.device_present.store(false, Ordering::Relaxed);
                    dev = None;
                    continue;
                }
                tokio::time::sleep(gap).await;
                if let Some(f2) = dev.as_mut() {
                    if let Err(e) = f2.write_all(&release) {
                        warn!("hid-writer: write release failed: {} — disabling", e);
                        stats.write_errors.fetch_add(1, Ordering::Relaxed);
                        stats.device_present.store(false, Ordering::Relaxed);
                        dev = None;
                        continue;
                    }
                }
                bump_event_stat(&stats, ev);
            }
            info!("hid-writer: event channel closed, exiting");
        });
        Self { _phantom: () }
    }
}
```

(Note: `tokio::spawn` reasoning — `f.write_all` is a blocking syscall but each report is 2 bytes and writes to a character device complete in microseconds, well below the 10 ms gap. No `spawn_blocking` needed.)

- [ ] **Step 2: Build**

Run: `cargo build -p waved-audio`
Expected: build succeeds, no warnings beyond pre-existing.

- [ ] **Step 3: Add an integration test that uses a temp file as the "device"**

Append to `crates/waved-audio/src/hid/writer.rs` inside the existing `#[cfg(test)] mod tests` block (after the existing tests):

```rust
    #[tokio::test(flavor = "current_thread")]
    async fn writer_writes_press_release_pair_to_file() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hidg0");
        // Pre-create the file so the writer's `OpenOptions::write(true).open(...)`
        // can find it (we are not creating, only opening).
        std::fs::File::create(&path).unwrap();

        let (tx, rx) = mpsc::channel(8);
        let stats = HidStats::new();
        let _w = HidWriter::spawn(
            HidWriterConfig {
                device: path.to_string_lossy().into_owned(),
                press_release_gap_ms: 1,
            },
            rx,
            Arc::clone(&stats),
        );
        tx.send(HidEvent::ConsumerVolumeStep { up: true }).await.unwrap();
        // Allow the writer task to drain.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut buf = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf, vec![0x01, 0b0000_0001, 0x01, 0x00]);
        assert!(stats.device_present.load(Ordering::Relaxed));
        assert_eq!(stats.events_volume_step.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn writer_missing_device_does_not_crash() {
        let (tx, rx) = mpsc::channel(8);
        let stats = HidStats::new();
        let _w = HidWriter::spawn(
            HidWriterConfig {
                device: "/tmp/does-not-exist-hidg0-xyz".into(),
                press_release_gap_ms: 1,
            },
            rx,
            Arc::clone(&stats),
        );
        tx.send(HidEvent::ConsumerMute).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!stats.device_present.load(Ordering::Relaxed));
        assert_eq!(stats.events_mute.load(Ordering::Relaxed), 0);
        assert_eq!(stats.write_errors.load(Ordering::Relaxed), 1);
    }
```

- [ ] **Step 4: Add `tempfile` to dev-dependencies if not already present**

Check: `grep -n '^tempfile' crates/waved-audio/Cargo.toml`. If absent, add under `[dev-dependencies]`:

```toml
tempfile = "3"
```

- [ ] **Step 5: Run the integration tests**

Run: `cargo test -p waved-audio --lib hid::writer::tests`
Expected: all 7 tests PASS (5 from Task 5 + 2 new).

- [ ] **Step 6: Commit**

```bash
git add crates/waved-audio/src/hid/writer.rs crates/waved-audio/Cargo.toml
git commit -m "feat(hid/writer): HidWriter task with /dev/hidg0 press-release loop"
```

---

## Task 7: `HidReader` — parse LED reports, update `AudioState`

**Files:**
- Modify: `crates/waved-audio/src/hid/reader.rs` (replace stub)

- [ ] **Step 1: Write the pure LED parser + tests first**

Replace the stub at `crates/waved-audio/src/hid/reader.rs` with:

```rust
//! HID Reader — owns `/dev/hidg0` read side; parses LED Set_Report writes
//! from the host and mirrors them into `AudioState.call_*`.

#![allow(dead_code)]

use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::sync::watch;
use tracing::{info, warn};

use super::HidStats;
use waved_state::AudioState;

/// One LED report parsed off the wire. Each variant maps to a single bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedReport {
    OffHook(bool),
    Mute(bool),
    Ringing(bool),
}

/// Decode a single `[report_id, byte]` write into a `LedReport`. Returns
/// `None` if the report id is unrecognised — we silently drop unknown
/// reports so the host can write any other future reports without breaking.
pub fn parse_led(buf: &[u8]) -> Option<LedReport> {
    if buf.len() < 2 { return None; }
    let bit = (buf[1] & 0x01) != 0;
    match buf[0] {
        0x05 => Some(LedReport::OffHook(bit)),
        0x06 => Some(LedReport::Mute(bit)),
        0x07 => Some(LedReport::Ringing(bit)),
        _    => None,
    }
}

/// Apply a parsed `LedReport`: bump the matching counter and store the
/// bit into both `HidStats` mirrors and the shared `AudioState`.
pub fn apply_led(
    led: LedReport,
    stats: &HidStats,
    audio_tx: &watch::Sender<AudioState>,
) {
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

#[derive(Debug, Clone)]
pub struct HidReaderConfig {
    pub device: String,
}

pub struct HidReader {
    _phantom: (),
}

impl HidReader {
    pub fn spawn(
        cfg: HidReaderConfig,
        audio_tx: Arc<watch::Sender<AudioState>>,
        stats: Arc<HidStats>,
    ) -> Self {
        std::thread::spawn(move || {
            // Blocking read loop on /dev/hidg0. Runs in its own OS thread
            // because `read()` on the gadget file does not return until the
            // host writes a Set_Report — tokio's executor would otherwise
            // be stuck on the read for unbounded time.
            let mut f = match std::fs::OpenOptions::new().read(true).open(&cfg.device) {
                Ok(f) => {
                    info!("hid-reader: opened {} for read", cfg.device);
                    f
                }
                Err(e) => {
                    warn!(
                        "hid-reader: cannot open {} for read: {} — LED mirror disabled",
                        cfg.device, e
                    );
                    return;
                }
            };
            let mut buf = [0u8; 64];
            loop {
                match f.read(&mut buf) {
                    Ok(0) => {
                        warn!("hid-reader: EOF on {}, exiting", cfg.device);
                        return;
                    }
                    Ok(n) => {
                        if let Some(led) = parse_led(&buf[..n]) {
                            apply_led(led, &stats, &audio_tx);
                        }
                    }
                    Err(e) => {
                        warn!("hid-reader: read error: {} — exiting", e);
                        return;
                    }
                }
            }
        });
        Self { _phantom: () }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::watch;
    use waved_state::AudioState;

    #[test]
    fn parse_off_hook_on() {
        assert_eq!(parse_led(&[0x05, 0x01]), Some(LedReport::OffHook(true)));
    }
    #[test]
    fn parse_off_hook_off() {
        assert_eq!(parse_led(&[0x05, 0x00]), Some(LedReport::OffHook(false)));
    }
    #[test]
    fn parse_mute_on() {
        assert_eq!(parse_led(&[0x06, 0x01]), Some(LedReport::Mute(true)));
    }
    #[test]
    fn parse_ringing_on() {
        assert_eq!(parse_led(&[0x07, 0x01]), Some(LedReport::Ringing(true)));
    }
    #[test]
    fn parse_unknown_report_id_returns_none() {
        assert_eq!(parse_led(&[0x03, 0xff]), None);
    }
    #[test]
    fn parse_short_buf_returns_none() {
        assert_eq!(parse_led(&[0x05]), None);
        assert_eq!(parse_led(&[]), None);
    }
    #[test]
    fn parse_ignores_padding_bits() {
        // Per descriptor, bits 1-7 of the LED byte are padding.
        assert_eq!(parse_led(&[0x05, 0xfe]), Some(LedReport::OffHook(false)));
        assert_eq!(parse_led(&[0x05, 0xff]), Some(LedReport::OffHook(true)));
    }

    #[test]
    fn apply_led_off_hook_updates_state_and_stats() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::OffHook(true), &stats, &tx);
        assert!(rx.borrow().call_off_hook);
        assert_eq!(stats.leds_off_hook.load(Ordering::Relaxed), 1);
        assert!(stats.call_off_hook.load(Ordering::Relaxed));
    }

    #[test]
    fn apply_led_ringing_updates_state_and_stats() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::Ringing(true), &stats, &tx);
        assert!(rx.borrow().call_ringing);
        assert_eq!(stats.leds_ringing.load(Ordering::Relaxed), 1);
        assert!(stats.call_ringing.load(Ordering::Relaxed));
    }

    #[test]
    fn apply_led_mute_updates_call_host_muted_only() {
        let (tx, rx) = watch::channel(AudioState::default());
        let stats = HidStats::default();
        apply_led(LedReport::Mute(true), &stats, &tx);
        let s = rx.borrow();
        assert!(s.call_host_muted);
        assert!(!s.speaker_muted, "Mute LED must not touch speaker_muted");
        assert!(!s.mic_muted, "Mute LED must not touch mic_muted");
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p waved-audio --lib hid::reader::tests`
Expected: all 10 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/waved-audio/src/hid/reader.rs
git commit -m "feat(hid/reader): LED report parser + HidReader thread"
```

---

## Task 8: `volume_watch` glue — `AudioState.speaker_volume` → `HidEvent::ConsumerVolumeStep`

**Files:**
- Modify: `crates/waved-audio/src/hid/mod.rs` (add `spawn_volume_watch` helper + tests)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/waved-audio/src/hid/mod.rs`:

```rust
    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_one_event_per_step_up() {
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        let mut state = AudioState::default();
        state.speaker_volume = 50;
        let (tx, rx) = watch::channel(state);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);

        let stats = HidStats::new();
        let _h = spawn_volume_watch(rx, ev_tx, stats, 1000); // no rate cap for the test

        tx.send_modify(|s| s.speaker_volume = 53);
        // Drain three events.
        let mut got = vec![];
        for _ in 0..3 {
            got.push(tokio::time::timeout(
                std::time::Duration::from_millis(200),
                ev_rx.recv(),
            ).await.unwrap().unwrap());
        }
        assert_eq!(got, vec![
            HidEvent::ConsumerVolumeStep { up: true },
            HidEvent::ConsumerVolumeStep { up: true },
            HidEvent::ConsumerVolumeStep { up: true },
        ]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_emits_down_events() {
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        let mut state = AudioState::default();
        state.speaker_volume = 50;
        let (tx, rx) = watch::channel(state);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);

        let stats = HidStats::new();
        let _h = spawn_volume_watch(rx, ev_tx, stats, 1000);

        tx.send_modify(|s| s.speaker_volume = 48);
        let got = vec![
            tokio::time::timeout(std::time::Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap(),
            tokio::time::timeout(std::time::Duration::from_millis(200), ev_rx.recv()).await.unwrap().unwrap(),
        ];
        assert_eq!(got, vec![
            HidEvent::ConsumerVolumeStep { up: false },
            HidEvent::ConsumerVolumeStep { up: false },
        ]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn volume_watch_ignores_non_volume_state_changes() {
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);

        let stats = HidStats::new();
        let _h = spawn_volume_watch(rx, ev_tx, stats, 1000);

        // Change a different field; speaker_volume stays at default.
        tx.send_modify(|s| s.mic_muted = !s.mic_muted);
        let timeout = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            ev_rx.recv(),
        ).await;
        assert!(timeout.is_err(), "expected no event on unrelated state change");
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p waved-audio --lib hid::tests::volume_watch`
Expected: FAIL — `cannot find function spawn_volume_watch`.

- [ ] **Step 3: Implement `spawn_volume_watch`**

First, extend the `use` statements at the **top** of `crates/waved-audio/src/hid/mod.rs` so they read:

```rust
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tracing::warn;

use waved_state::AudioState;
```

Then add the function **immediately after the `HidStats` impl block** (and above the `HidStatsSnapshot` / `HidEventCounts` / `HidLedCounts` struct definitions):

```rust
/// Subscribe to `audio_rx` and emit one `ConsumerVolumeStep` per integer
/// step delta in `speaker_volume`. Rate-capped at `volume_step_rate_hz`.
///
/// Returns the spawned task handle for the caller to keep alive (drop = stop).
pub fn spawn_volume_watch(
    mut audio_rx: watch::Receiver<AudioState>,
    events: mpsc::Sender<HidEvent>,
    stats: Arc<HidStats>,
    volume_step_rate_hz: u32,
) -> tokio::task::JoinHandle<()> {
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
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p waved-audio --lib hid::tests::volume_watch`
Expected: all 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-audio/src/hid/mod.rs
git commit -m "feat(hid/glue): spawn_volume_watch emits ConsumerVolumeStep per delta"
```

---

## Task 9: `mute_watch` glue — `speaker_muted` edge → `HidEvent::ConsumerMute` (+ `TelephonyPhoneMute` when off-hook)

**Files:**
- Modify: `crates/waved-audio/src/hid/mod.rs` (add `spawn_mute_watch` + tests)

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_emits_consumer_mute_on_toggle() {
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);

        let stats = HidStats::new();
        let _h = spawn_mute_watch(rx, ev_tx, stats);

        tx.send_modify(|s| s.speaker_muted = true);
        let ev = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            ev_rx.recv(),
        ).await.unwrap().unwrap();
        assert_eq!(ev, HidEvent::ConsumerMute);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_emits_both_when_off_hook() {
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        let mut s = AudioState::default();
        s.call_off_hook = true;
        let (tx, rx) = watch::channel(s);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);

        let stats = HidStats::new();
        let _h = spawn_mute_watch(rx, ev_tx, stats);

        tx.send_modify(|s| s.speaker_muted = true);
        let a = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            ev_rx.recv(),
        ).await.unwrap().unwrap();
        let b = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            ev_rx.recv(),
        ).await.unwrap().unwrap();
        let got = vec![a, b];
        assert!(got.contains(&HidEvent::ConsumerMute));
        assert!(got.contains(&HidEvent::TelephonyPhoneMute));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mute_watch_ignores_no_op_changes() {
        use tokio::sync::{mpsc, watch};
        use waved_state::AudioState;

        let (tx, rx) = watch::channel(AudioState::default());
        let (ev_tx, mut ev_rx) = mpsc::channel(16);

        let stats = HidStats::new();
        let _h = spawn_mute_watch(rx, ev_tx, stats);

        // Set speaker_muted to its existing value — no edge.
        tx.send_modify(|s| s.speaker_muted = false);
        let timeout = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            ev_rx.recv(),
        ).await;
        assert!(timeout.is_err(), "expected no event without an edge");
    }
```

- [ ] **Step 2: Implement `spawn_mute_watch`**

Add to `crates/waved-audio/src/hid/mod.rs`, immediately after `spawn_volume_watch`:

```rust
/// Subscribe to `audio_rx` and emit `ConsumerMute` on each transition of
/// `speaker_muted`; also emit `TelephonyPhoneMute` if `call_off_hook == true`
/// at the moment of the transition.
pub fn spawn_mute_watch(
    mut audio_rx: watch::Receiver<AudioState>,
    events: mpsc::Sender<HidEvent>,
    stats: Arc<HidStats>,
) -> tokio::task::JoinHandle<()> {
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
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p waved-audio --lib hid::tests::mute_watch`
Expected: all 3 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/waved-audio/src/hid/mod.rs
git commit -m "feat(hid/glue): spawn_mute_watch emits Consumer + Phone mute"
```

---

## Task 10: Wire HID into `main.rs`

**Files:**
- Modify: `crates/waved/src/main.rs` (spawn HidWriter, HidReader, glue tasks in the SoftwareRef branch; populate `hid_stats_handle`)

- [ ] **Step 1: Locate the SP#6 UAC spawn block**

Run: `grep -n "SP#6: UAC2 routing" crates/waved/src/main.rs`
Expected: one match, around line 296.

- [ ] **Step 2: Add the HID block**

Immediately after the existing UAC spawn block — find the `} else { tracing::info!("uac: disabled by config"); }` and add **after** that brace closes:

```rust
            // SP#7(d'): HID Consumer/Telephony control surface.
            let hid_cfg = cfg.hid.clone().unwrap_or_default();
            let hid_stats: Arc<waved_audio::hid::HidStats> = waved_audio::hid::HidStats::new();
            hid_stats_handle = Some(Arc::clone(&hid_stats));

            if hid_cfg.enabled {
                let (ev_tx, ev_rx) = tokio::sync::mpsc::channel::<waved_audio::hid::HidEvent>(64);

                let writer_cfg = waved_audio::hid::writer::HidWriterConfig {
                    device: hid_cfg.device.clone(),
                    press_release_gap_ms: hid_cfg.press_release_gap_ms,
                };
                let _writer = waved_audio::hid::writer::HidWriter::spawn(
                    writer_cfg,
                    ev_rx,
                    Arc::clone(&hid_stats),
                );

                let reader_cfg = waved_audio::hid::reader::HidReaderConfig {
                    device: hid_cfg.device.clone(),
                };
                let _reader = waved_audio::hid::reader::HidReader::spawn(
                    reader_cfg,
                    Arc::clone(&bus.audio),
                    Arc::clone(&hid_stats),
                );

                let _vh = waved_audio::hid::spawn_volume_watch(
                    bus.audio.subscribe(),
                    ev_tx.clone(),
                    Arc::clone(&hid_stats),
                    hid_cfg.volume_step_rate_hz,
                );
                let _mh = waved_audio::hid::spawn_mute_watch(
                    bus.audio.subscribe(),
                    ev_tx,
                    Arc::clone(&hid_stats),
                );

                tracing::info!(
                    "hid: control surface live (device={} press_release_gap_ms={} step_rate_hz={})",
                    hid_cfg.device, hid_cfg.press_release_gap_ms, hid_cfg.volume_step_rate_hz
                );
            } else {
                tracing::info!("hid: disabled by config");
            }
```

(Verified: `crates/waved-state/src/lib.rs:244` declares `pub audio: Arc<watch::Sender<AudioState>>` so `Arc::clone(&bus.audio)` is the correct call.)

- [ ] **Step 3: Build the binary**

Run: `cargo build -p waved --features ui,capture-alsa`
Expected: build succeeds.

- [ ] **Step 4: Build for the target device cross-compile**

Run: `cross build --release --target aarch64-unknown-linux-gnu --features ui,capture-alsa -p waved`
Expected: build succeeds. (If `cross` is not installed in this dev environment, skip and run the host-side build only — the cross build will run later as part of acceptance.)

- [ ] **Step 5: Commit**

```bash
git add crates/waved/src/main.rs
git commit -m "feat(hid): spawn HidWriter/Reader + volume/mute watchers in main"
```

---

## Task 11: Rename and extend the configfs patcher

**Files:**
- Rename: `scripts/patch-uac2-volume-range.sh` → `scripts/patch-stock-usbgadget.sh`
- Modify: the renamed file (extend with `functions/hid.0` setup)
- Update: `docs/AUDIO_HARDWARE.md` (search for the script name)
- Update: any `README.md` reference

- [ ] **Step 1: Rename the script**

Run from the repo root:

```bash
git mv scripts/patch-uac2-volume-range.sh scripts/patch-stock-usbgadget.sh
```

- [ ] **Step 2: Update the header docstring + add the HID block**

Open `scripts/patch-stock-usbgadget.sh` and replace its full content with:

```bash
#!/usr/bin/env bash
# Patch /etc/init.d/S89usbgadget on a Wave so the device exposes:
#   1. A useful playback (Output) volume range to UAC2 hosts.
#   2. A functions/hid.0 control surface advertising the stock-extracted
#      Vendor + Telephony_V4 + Consumer report descriptor, so that the
#      Wave touch UI can mirror media keys / mute / phone-mute to the
#      host (SP#7(d')) and so future Wave Controller compatibility work
#      (SP#7(a)) has the vendor channel reachable on /dev/hidg0.
#
# Idempotent: re-running adds nothing after the first patch. Backs up
# the original to S89usbgadget.bak.<ts> on first run.
#
# After patching, reboot the device:   adb reboot

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
    # Insert the hid.0 function setup before the gadget enable step (`usb_flash`).
    # The descriptor bytes are copied verbatim from stock fw v1.1.6.6's
    # S89usbgadget:199 — the active (uncommented) Vendor+Telephone_V4+Consumer
    # combined descriptor. Indented with a leading tab to match the existing
    # shell-function style.
    DESCRIPTOR='\\x06\\x00\\xff\\x09\\x01\\xa1\\x01\\x85\\x03\\x09\\x01\\x15\\x00\\x26\\xff\\x00\\x95\\x3f\\x75\\x08\\x81\\x02\\x09\\x01\\x15\\x00\\x26\\xff\\x00\\x95\\x3f\\x75\\x08\\x91\\x22\\xc0\\x05\\x0b\\x09\\x05\\xa1\\x01\\x85\\x02\\x15\\x00\\x25\\x01\\x75\\x01\\x95\\x01\\x09\\x20\\x81\\x22\\x09\\x2f\\x81\\x06\\x09\\x21\\x81\\x02\\x09\\x24\\x81\\x06\\x09\\x07\\x05\\x09\\x09\\x01\\x81\\x06\\x75\\x03\\x81\\x01\\x85\\x05\\x05\\x08\\x09\\x17\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\x85\\x06\\x09\\x09\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\x85\\x07\\x09\\x18\\x75\\x01\\x91\\x02\\x75\\x07\\x91\\x03\\xc0\\x05\\x0c\\x09\\x01\\xa1\\x01\\x85\\x01\\x05\\x0c\\x15\\x00\\x25\\x01\\x09\\xe9\\x09\\xea\\x09\\xe2\\x09\\xcd\\x09\\xb5\\x09\\xb6\\x09\\xb0\\x09\\xb1\\x75\\x01\\x95\\x08\\x81\\x02\\xc0'

    # Append a setup block right before the first 'usb_flash' invocation.
    # Using awk because we need to splice multi-line content at a marker.
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
echo "relevant lines:"
grep -n "p_volume_min\|p_volume_max\|p_volume_res\|functions/hid.0" "$INIT" || true

echo
echo "reboot the device for the new gadget config to take effect:"
echo "  adb reboot"
```

- [ ] **Step 3: Update any references to the old name**

Run: `grep -rn "patch-uac2-volume-range" docs/ scripts/ crates/ README.md 2>/dev/null` and replace each occurrence with `patch-stock-usbgadget`. There should be at least one match in `docs/AUDIO_HARDWARE.md` (`§f_uac2 configfs gotcha`).

- [ ] **Step 4: Smoke-test the awk splice locally**

Run on the dev host (not the device):

```bash
cp fw_v1.1.6.6/rootfs/etc/init.d/S89usbgadget /tmp/S89.test
INIT=/tmp/S89.test bash -c '
    set -euo pipefail
    sed -i.bak "s|echo -100 > functions/uac2.0/p_volume_min|echo -16384 > functions/uac2.0/p_volume_min|" /tmp/S89.test
'
# Now invoke the awk block standalone on /tmp/S89.test:
DESC_TEST='\x06\x00\xff'
awk -v desc="$DESC_TEST" '
    BEGIN { inserted = 0 }
    /^[[:space:]]*usb_flash[[:space:]]*$/ && !inserted {
        print "\t# test marker"
        print "\tmkdir -p functions/hid.0"
        print "\techo -ne \"" desc "\" > functions/hid.0/report_desc"
        inserted = 1
    }
    { print }
' /tmp/S89.test | grep -n "test marker\|usb_flash\|hid.0" | head -10
```

Expected: the splice inserts the marker + mkdir + echo lines **above** the first `usb_flash` line — not below, not duplicated.

- [ ] **Step 5: Commit**

```bash
git add scripts/patch-stock-usbgadget.sh docs/AUDIO_HARDWARE.md
git commit -m "feat(scripts): patch-stock-usbgadget.sh installs UAC volume range + hid.0"
```

---

## Task 12: Acceptance script `scripts/test-hid-control.sh`

**Files:**
- Create: `scripts/test-hid-control.sh`
- Create: `scripts/test-hid-control-host.sh`

- [ ] **Step 1: Create the on-device script**

Write `scripts/test-hid-control.sh`:

```bash
#!/usr/bin/env bash
# SP#7(d') device-side acceptance.
# Pre: device booted, waved running, S89usbgadget patched, Mac connected.
#
# Tests:
#   1. /dev/hidg0 present
#   2. hid.device_present true in stats
#   3. Volume tick over /tmp/waved-volume → events_emitted.volume_step increments
#   4. Mute toggle via Command::ToggleSpeakerMute → events_emitted.mute >= 1
#   5. (Manual) Zoom call on host → hid.call_off_hook true
#   6. Hostless boot — no crash, stats remain queryable
#
# Some tests require a host-side helper running scripts/test-hid-control-host.sh.

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

# Test 1: /dev/hidg0 present.
echo "==> /dev/hidg0 presence" | tee -a "$LOG"
if adb shell "[ -c /dev/hidg0 ] && echo OK || echo MISSING" | grep -q OK; then
    echo "1/6 hidg0 char device: PASS" | tee -a "$LOG"
else
    echo "1/6 hidg0 char device: FAIL — patch-stock-usbgadget.sh + reboot first" | tee -a "$LOG"
    exit 1
fi

# Test 2: device_present in stats.
echo "==> hid.device_present" | tee -a "$LOG"
for i in $(seq 1 10); do
    P=$(read_hid_field "s['hid']['device_present']")
    if [ "$P" = "True" ]; then break; fi
    sleep 1
done
if [ "$P" = "True" ]; then
    echo "2/6 device_present: PASS" | tee -a "$LOG"
else
    echo "2/6 device_present: FAIL — hid.device_present=$P after 10 s" | tee -a "$LOG"
    exit 1
fi

# Test 3: volume step increments.
echo "==> volume step counter" | tee -a "$LOG"
BEFORE=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
# Drive volume from 50 → 53 via the configured SIGHUP source.
adb shell "echo 50 > /tmp/waved-volume; kill -HUP \$(pidof waved); sleep 0.5; echo 53 > /tmp/waved-volume; kill -HUP \$(pidof waved)"
sleep 1
AFTER=$(read_hid_field "s['hid']['events_emitted']['volume_step']")
DELTA=$(python3 -c "print(int($AFTER) - int($BEFORE))")
if [ "$DELTA" -ge 3 ]; then
    echo "3/6 volume_step delta: PASS ($BEFORE → $AFTER, +$DELTA)" | tee -a "$LOG"
else
    echo "3/6 volume_step delta: FAIL ($BEFORE → $AFTER, +$DELTA, expected ≥ 3)" | tee -a "$LOG"
    exit 1
fi

# Test 4: mute toggle.
echo "==> mute toggle counter" | tee -a "$LOG"
BEFORE=$(read_hid_field "s['hid']['events_emitted']['mute']")
adb shell "amixer -c 1 cset numid=4 1 >/dev/null 2>&1 || true; sleep 0.3; amixer -c 1 cset numid=4 0 >/dev/null 2>&1 || true"
sleep 1
AFTER=$(read_hid_field "s['hid']['events_emitted']['mute']")
DELTA=$(python3 -c "print(int($AFTER) - int($BEFORE))")
if [ "$DELTA" -ge 1 ]; then
    echo "4/6 mute counter: PASS ($BEFORE → $AFTER, +$DELTA)" | tee -a "$LOG"
else
    echo "4/6 mute counter: FAIL ($BEFORE → $AFTER, expected ≥ 1)" | tee -a "$LOG"
    exit 1
fi

# Test 5: Off-hook LED from host (manual).
echo "==> Off-hook LED (manual: start a Zoom call now; 30 s to detect)" | tee -a "$LOG"
for i in $(seq 1 30); do
    if [ "$(read_hid_field "s['hid']['call_off_hook']")" = "True" ]; then
        echo "5/6 call_off_hook: PASS (detected after ${i} s)" | tee -a "$LOG"
        break
    fi
    sleep 1
done
if [ "$(read_hid_field "s['hid']['call_off_hook']")" != "True" ]; then
    echo "5/6 call_off_hook: SKIP — no Zoom call detected within 30 s (manual test, not a hard fail)" | tee -a "$LOG"
fi

# Test 6: hostless stats still queryable.
echo "==> hostless stats query" | tee -a "$LOG"
J=$(adb shell "cat /tmp/audio_stats.json")
echo "$J" | python3 -c "import json, sys; json.loads(sys.stdin.read())['hid']" >/dev/null
echo "6/6 hostless stats query: PASS" | tee -a "$LOG"

echo "==> acceptance complete; see $LOG" | tee -a "$LOG"
```

Make it executable:

```bash
chmod +x scripts/test-hid-control.sh
```

- [ ] **Step 2: Create the host-side helper**

Write `scripts/test-hid-control-host.sh`:

```bash
#!/usr/bin/env bash
# SP#7(d') host-side companion to scripts/test-hid-control.sh.
# Reads system volume / mute state before and after triggering device
# touch-UI events. Run this on the Mac/Linux host while the device script
# runs on the device.

set -euo pipefail

osx_get_vol()   { osascript -e 'output volume of (get volume settings)' 2>/dev/null || echo "?"; }
osx_get_muted() { osascript -e 'output muted of (get volume settings)' 2>/dev/null || echo "?"; }

echo "host volume before: $(osx_get_vol)"
echo "host muted before:  $(osx_get_muted)"

echo
echo "Now run scripts/test-hid-control.sh on the device."
echo "Press [Return] when the device script reports 4/6 mute counter PASS."
read -r _

echo "host volume after:  $(osx_get_vol)"
echo "host muted after:   $(osx_get_muted)"
echo
echo "If 'muted before/after' differs by one toggle, mute mirror works."
echo "If 'volume before/after' differs by ≥ 3 steps, volume mirror works."
```

Make it executable:

```bash
chmod +x scripts/test-hid-control-host.sh
```

- [ ] **Step 3: Local syntax check both scripts**

Run: `bash -n scripts/test-hid-control.sh && bash -n scripts/test-hid-control-host.sh && echo OK`
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add scripts/test-hid-control.sh scripts/test-hid-control-host.sh
git commit -m "feat(scripts): SP#7(d') acceptance test-hid-control.sh + host helper"
```

---

## Task 13: Update `TODO.md` and `docs/AUDIO_HARDWARE.md`

**Files:**
- Modify: `TODO.md` (mark SP#7 split: d-original deferred, d'-new shipped)
- Modify: `docs/AUDIO_HARDWARE.md` (add HID control surface section)

- [ ] **Step 1: Update `TODO.md`**

Open `TODO.md`. Locate the `### SP#7 scope — Wave Controller native compatibility (deferred)` block and add this paragraph as the first bullet:

```markdown
- [x] **SP#7(d') — HID Consumer/Telephony control surface** (`crates/waved-audio/src/hid/`: `HidWriter` emits Consumer Volume/Mute/Play media keys + Telephony Phone Mute over `/dev/hidg0`; `HidReader` mirrors host Off-hook/Mute/Ring LED writes into `AudioState.call_*`; `scripts/patch-stock-usbgadget.sh` installs the stock Vendor+Telephony_V4+Consumer report descriptor; verified via `scripts/test-hid-control.sh`.) Original SP#7(d) (`f_uac2` Master Volume kernel patch) deferred — see "Mac F11/F12 / Output slider" below; the HID Consumer page is the stock-equivalent path and ships without a kernel build.
```

- [ ] **Step 2: Update `docs/AUDIO_HARDWARE.md`**

Open `docs/AUDIO_HARDWARE.md`. Find the section `### Mac → Wave control gaps (kernel f_uac2 5.4 limitation)`. Immediately before it, add a new section:

```markdown
### HID control surface (SP#7(d'))

The device exposes a `functions/hid.0` USB-gadget interface with a combined
**Vendor (Report 0x03) + Telephony_V4 (Reports 0x02/0x05/0x06/0x07) +
Consumer (Report 0x01)** report descriptor, copied byte-for-byte from stock
fw v1.1.6.6. `crates/waved-audio/src/hid/` owns `/dev/hidg0`:

- `HidWriter` task — drains `mpsc<HidEvent>` and writes press-release pairs.
  Consumes events from two glue subscribers on `audio_tx`:
  - `volume_watch` — one `ConsumerVolumeStep` per `speaker_volume` step delta,
    rate-capped at `hid.volume_step_rate_hz` (default 40).
  - `mute_watch` — one `ConsumerMute` per `speaker_muted` edge; additionally
    one `TelephonyPhoneMute` when `call_off_hook == true`.
- `HidReader` thread — blocking read on `/dev/hidg0`, parses LED Set_Reports
  (0x05/0x06/0x07) into `AudioState.call_off_hook / call_host_muted /
  call_ringing`.

Vendor channel (Report 0x03) is declared but not handled (SP#7(a) future).

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

- [ ] **Run the full waved-audio test suite**

Run: `cargo test -p waved-audio --lib`
Expected: all tests PASS (HID + UAC + capture + playback).

- [ ] **Run the waved-state test suite**

Run: `cargo test -p waved-state --lib`
Expected: PASS.

- [ ] **Cross-compile for device**

Run: `cross build --release --target aarch64-unknown-linux-gnu --features ui,capture-alsa -p waved`
Expected: build succeeds, no warnings beyond pre-existing.

- [ ] **Deploy + reboot**

```bash
adb push scripts/patch-stock-usbgadget.sh /tmp/
adb shell "sh /tmp/patch-stock-usbgadget.sh"
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb reboot
# wait ~60s
```

- [ ] **Run acceptance**

Terminal 1 (device): `./scripts/test-hid-control.sh`
Terminal 2 (Mac): `./scripts/test-hid-control-host.sh`
Expected: device script reports `6/6 PASS` (tests 5 is SKIP-without-Zoom which is fine).

- [ ] **Push**

```bash
git push
```
