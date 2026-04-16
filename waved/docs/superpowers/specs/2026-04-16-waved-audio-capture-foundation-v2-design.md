# Waved Audio — Capture Foundation v2

**Supersedes:** `2026-04-16-waved-audio-capture-foundation-design.md` (v1) for SP#1 only. The six-sub-project decomposition in v1 §1.2 stands.

## 1. Why v2

The v1 SP#1 implementation (16 tasks, commits `d3f02e3..bc80dcc`) passed 47 host tests, passed multi-stage review, and cross-built cleanly after a container fix (`3426a9f`). On first on-device deploy it failed immediately:

```
WARN  channel map at /etc/channel_map_config_default.json invalid (missing field `version`), using fallback
ERROR ALSA capture open failed: snd_pcm_hw_params_set_channels failed with error 'Invalid argument (22)'
```

Two unverified assumptions were wrong:

1. **Device path.** v1 opened `hw:0,0` expecting 10 interleaved channels (8 mics + 2 references). Actual on-device capabilities:

   | ALSA device | Name | `CHANNELS` | Role |
   |---|---|---|---|
   | `hw:0,0` | TDM-A | `[1 2]` | Amp output path (not mics) |
   | `hw:0,2` | PDM | `[1 16]` | **8 MEMS mics live here** |
   | `hw:0,3` | LOOPBACK-A | `[1 32]` | Reference channels |

   Mics and references are on **separate ALSA devices**, not one interleaved stream.

2. **Channel map schema.** The device's `/etc/channel_map_config_default.json` uses a completely different shape (`param_channel_map.builtin_mic_map_setting[].builtin_mic_map`) than v1's parser expected. v1 fell back to the embedded default (correct behaviour), but that default assumed the same wrong 10-ch model.

v2 fixes both, and while we're redesigning, makes the capture path durable for SP#2–5 so we don't repeat this.

## 2. Scope

**In scope for SP#1 v2:**

- Real ALSA capture from `hw:0,2` (mics) + `hw:0,3` (references) as **two coordinated streams**
- Aligned `AudioFrame` delivery with mic PCM + reference sidecar + single timestamp
- Forward-compatible config schema + file at a new path
- Runtime hardware probe that replaces opaque ALSA errors with actionable diagnostics
- Delete stub `ffi.rs`; speaker controls via `amixer`; `SetAudioMode` becomes state-only pending SP#3
- Per-task on-device verification gate
- `TODO.md` at repo root tracking the SP#1–6 roadmap

**Out of scope (still):**

- DSP of any kind (AEC/NS/beamforming/EQ stay in SP#2–5)
- UAC2 routing (SP#6)
- Schema extensions for mic geometry, beamforming presets, playback pipeline — reserved for later SPs under a `version` bump

## 3. Preserved from v1 (no changes)

- `metrics.rs` — 7 pure measurement functions
- `fixtures.rs` — 8-ch synthetic generators
- `capture/mock.rs` — `MockPcmSource` for unit tests
- `capture/replay.rs` — `ReplayPcmSource` for host replay
- `capture/thread.rs` — `run_capture_loop` (generic over `PcmSource`)
- `waved-audio-bench` — offline stats CLI
- `stats_collector` — broadcast tap + atomic JSON writer (minor extension: report reference RMS + alignment drift)
- `PcmSource` trait — unchanged for single-stream use (ALSA-per-device still uses it)
- 47 existing host tests remain green

## 4. Architecture

### 4.1 Dual-stream capture

```
┌─────────────────┐    ┌─────────────────┐
│ AlsaPcmSource   │    │ AlsaPcmSource   │
│  hw:0,2 (8ch)   │    │  hw:0,3 (2ch)   │
│  S32_LE 48kHz   │    │  S32_LE 48kHz   │
└────────┬────────┘    └────────┬────────┘
         │                      │
         │ snd_pcm_link at startup (if supported)
         │ else: per-thread CLOCK_MONOTONIC timestamps
         ▼                      ▼
┌──────────────────────────────────────────┐
│      AlsaDualStreamSource                │
│   paired-frame aligner                   │
│   emits: (mic_buf, ref_buf, ts_ms)       │
└──────────────────┬───────────────────────┘
                   ▼
         run_capture_loop
                   ▼
              AudioFrame
```

**Alignment strategy (try hard path first, fall back):**

1. Open both `AlsaPcmSource`s in prepared state.
2. Call `snd_pcm_link(mic_pcm.as_ptr(), ref_pcm.as_ptr())`.
3. If it returns OK: single `snd_pcm_start` starts both on the same sample clock. Read from each with `snd_pcm_readi`; frames are assumed sample-aligned by hardware. Log `alignment: kernel-linked`.
4. If `snd_pcm_link` errors: switch to timestamp-aligned path. Two reader threads each tag frames with `CLOCK_MONOTONIC` on return from `snd_pcm_readi`; aligner matches within ±1 period via timestamp diff. Log `alignment: timestamp-fallback`.
5. Record per-frame alignment method in `/tmp/audio_stats.json`. The timestamp-fallback path also reports the p95 timestamp-diff between paired mic/ref frames as `alignment_drift_us_p95`.

Note: we cannot empirically validate that `snd_pcm_link` actually aligns the two streams without an external signal source. We trust the API: if `snd_pcm_link` returns OK, we log the outcome; drift observability comes from SP#2's cross-correlation work once a known signal flows through the system.

The chosen path is invisible to consumers: both emit the same `AudioFrame` shape.

### 4.2 `AudioFrame` shape

```rust
pub struct AudioFrame {
    pub data: Vec<i16>,                  // 8-ch mic interleaved
    pub reference: Option<Vec<i16>>,     // 2-ch loopback, same timestamp
    pub sample_rate: u32,
    pub channels: u16,                   // mic channel count
    pub reference_channels: u16,         // 0 if reference is None
    pub timestamp_ms: u64,
}
```

- `i16` stays (meeting WAV writes it directly; SP#2 DSP converts to `f32` at its own boundary)
- `reference = None` is legal (simulate path, replay of mic-only fixtures) — mic-only consumers must handle it gracefully
- `timestamp_ms` applies to **both** `data` and `reference` — that's the alignment contract

### 4.3 Configuration

New file at `/etc/waved-audio-config.json`. Leaves device's existing `/etc/channel_map_config_default.json` untouched (unknown external consumers).

```json
{
  "version": 2,
  "capture": {
    "mic_device": "hw:0,2",
    "mic_channels": 8,
    "mic_sample_rate": 48000,
    "mic_format": "S32_LE",
    "reference_device": "hw:0,3",
    "reference_channels": 2,
    "reference_sample_rate": 48000,
    "reference_format": "S32_LE"
  },
  "period_frames": 256,
  "buffer_periods": 4
}
```

Loader: `load_or_default(path) -> (AudioConfig, ConfigSource)` returns `ConfigSource::File` or `ConfigSource::Fallback`. Fallback matches the verified hardware.

Forward-compatibility: future SPs add fields under the same version bump. SP#3 adds `mic_geometry`, SP#5 adds `playback`, etc.

### 4.4 Runtime hardware probe

At the top of `AlsaPcmSource::open`, before any `hw_params_set_*`:

```rust
let hwp = HwParams::any(&pcm)?;              // enumerate actual ranges
let ch_min = hwp.get_channels_min()?;
let ch_max = hwp.get_channels_max()?;
if !(ch_min..=ch_max).contains(&cfg.channels) { return Err(AlsaError::Mismatch { ... }); }
// … and same for format, rate, period_size
```

Error message format:
```
ALSA config/hardware mismatch on hw:0,2:
  channels: config=10, supported=[1..16]  MISMATCH
  format:   config=S32_LE, supported=[S16_LE S24_LE S32_LE]  OK
  rate:     config=48000, supported=[8000..96000]  OK
```

Replaces today's opaque `Invalid argument (22)`.

### 4.5 FFI deletion

- Delete `waved-audio/src/ffi.rs` and `pub mod ffi;` from `lib.rs`.
- `Command::ToggleSpeakerMute` and `Command::SetSpeakerVolume` shell out to `amixer set <control> <value>` (blocking task). Which ALSA mixer control to use is discovered at startup by enumerating `/proc/asound/card0/controls` or via `amixer scontrols`; stored in `AudioConfig` under `playback.speaker_control` for override.
- `Command::SetAudioMode` updates state only (`audio_tx.send_modify`); logs `tracing::debug!("SetAudioMode {mode:?}: state-only until SP#3 (beamforming) or SP#4 (NS)")`.
- TODO.md tracks the owner SP for each deferred piece.

## 5. Data flow

```
AlsaDualStreamSource
   │ paired frames (mic + ref + ts)
   ▼
run_capture_loop (generic)
   │ AudioFrame
   ├──► stats_collector (broadcast tap)
   │      writes /tmp/audio_stats.json every 500ms
   │      fields: rms_per_channel (8), reference_rms (2), xrun_count,
   │              frames_emitted, frames_dropped, alignment,
   │              alignment_drift_samples_p95, config_source
   │
   ▼
AudioService
   │ (apply mic mute, compute VU, fan out)
   ├──► VU state (8 bands from `data`)
   ├──► meeting_tx (frame.data only — WAV writes i16 mono/stereo)
   └──► reference available to future AEC consumer in SP#2
```

## 6. Error handling

| Failure | Handling |
|---|---|
| Config file missing/malformed | Warn, use embedded fallback, log `config_source: "fallback"` |
| Mic ALSA device (`hw:0,2`) open fails | Log fatal error with probe summary, exit 1 (daemon respawn) |
| Reference ALSA device (`hw:0,3`) open fails, reference configured | Log fatal error with probe summary, exit 1 — if an operator wants mic-only, they remove `reference_device` from config |
| Reference device omitted from config | Run mic-only; `AudioFrame.reference = None`; stats collector's `reference_rms` is an empty array |
| Config/hardware mismatch at probe | Log exact mismatch diagnosis, exit 1 |
| `snd_pcm_link` unsupported | Log, fall back to timestamp alignment, continue |
| Per-stream xrun | Each `AlsaPcmSource` recovers independently via `snd_pcm_prepare`; counter per stream |
| Alignment drift between mic/ref frames exceeds 1 period (timestamp path only) | Log warn, drop misaligned pair, increment `drift_drops` counter |
| Meeting channel full (backpressure) | Drop newest mic frame (preserves contiguous history); reference follows |
| `amixer` shell-out fails | Log warn, return to state-update only |

## 7. Testing — per-task verification gate

**Hard rule:** Any task that touches `AlsaPcmSource`, `AlsaDualStreamSource`, `AudioConfig` parsing, or ALSA error paths must include an explicit **"push to device, run, verify"** step in the plan before the task can be marked complete.

Verification step template:

```
adb push target/aarch64-unknown-linux-gnu/debug/waved /data/waved/waved
adb shell "killall waved; sleep 2; /etc/init.d/S60waved start"
sleep 8
adb shell "cat /tmp/audio_stats.json" | python3 -m json.tool
# Assert: rms_per_channel has 8 entries, reference_rms has 2,
#         frames_emitted > 0, alignment is "kernel-linked" or "timestamp-fallback"
```

Tasks that **don't** touch ALSA (metrics, fixtures, replay, stats collector schema changes) keep host-only TDD as before. Classification is declared per task in the plan.

Tiers (unchanged from v1):

- **Tier 1 (host, per task):** `cargo test -p waved-audio` — pure logic, no device
- **Tier 2 (host, per task):** `cargo test -p waved-audio --features capture-replay` — replay-based integration
- **Tier 3 (device, per ALSA task):** push + run + stats verification (new)
- **Tier 3 (device, end of plan):** `scripts/test-capture-on-device.sh` full run

## 8. Deliverables / success criteria

A PR that merges this is done when **all** of the following hold:

1. `cargo test -p waved-audio` green (existing 47 + new: dual-stream alignment, config parser, hardware probe)
2. `cargo test -p waved-audio --features capture-replay` green (existing 3 replay tests still pass; reference-sidecar replay test added)
3. `cross build --target aarch64-unknown-linux-gnu -p waved -p waved-latency-probe -p waved-capture-record --features "ui,capture-alsa"` clean
4. On device: `waved` starts, does not crash, writes `/tmp/audio_stats.json` with 8 mic RMS + 2 reference RMS, `alignment` populated, `frames_emitted > 0`, `xrun_count` present
5. `waved-latency-probe` returns a non-error JSON with `delay_ms` in a plausible range (5 probe-runs, stddev < 5ms)
6. `waved-capture-record --duration 10 --out smoke.pcm` writes `smoke.pcm` (8-ch i16) and `smoke.meta.json` **plus** an optional `smoke.ref.pcm` (2-ch i16 reference) when reference capture is enabled
7. `scripts/test-capture-on-device.sh` passes end-to-end
8. `TODO.md` at repo root committed, covering SP#1 current state and SP#2–6 carry-overs
9. `docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md` committed (this file)

## 9. Future hooks — what SP#2–5 inherit from v2

- **SP#2 (AEC):** `AudioFrame.reference` sidecar already aligned. AEC flips to `Vec<f32>` at its DSP boundary; `AudioFrame` stays `i16`. Owns the `NoiseReduction` audio mode.
- **SP#3 (Beamforming):** Adds `mic_geometry` to `AudioConfig` under a `version` bump. Wires `Command::SetAudioMode` (Omni/Cardioid/Supercardioid/Figure8/Stereo) to MVDR/DAS presets in a new DSP stage consuming `AudioFrame.data`.
- **SP#4 (Neural NS):** DeepFilterNet 3 stage, single-channel, consumes post-beamforming f32. No capture changes.
- **SP#5 (Playback EQ):** Introduces playback pipeline. Retires `amixer` shell-outs in favour of a proper gain stage. Adds `playback` block to `AudioConfig` under a version bump.
- **SP#6 (UAC2):** Out of scope for this roadmap; eventually consumes post-DSP frames.

## 10. Risks

1. **`snd_pcm_link` silently no-ops on Amlogic/dummy driver.** We cannot detect this without an external reference signal (SP#2 supplies one via AEC playback). For v2 we trust the API: if `snd_pcm_link` returns OK, we log `alignment: kernel-linked` and proceed. If real alignment turns out to be absent when SP#2 measures it, the timestamp-fallback path is already available as the alignment strategy — we just flip a config flag. Acceptable: worst case is we always run the fallback, which is still within SP#1's accuracy bounds.
2. **`amixer` speaker-control name unknown.** Mitigation: discover at startup, allow override in config. If discovery fails, log warning and make the UI mute button state-only (no regression vs. stub — which also did nothing).
3. **Device `/etc/waved-audio-config.json` absent on deployed devices.** Mitigation: embedded fallback matches the hardware we verified today. Deploy step can drop the file if operators want file-based overrides.
4. **Hardware variations across device revisions.** Out of scope for v2. The runtime probe plus the exit-with-diagnostic behaviour makes future hardware variants fail loudly with actionable messages, not opaquely.

## 11. Non-goals

- Replacing libInstaAudio for audio mode DSP (SP#3/4 own that)
- Migrating or parsing the device's existing `/etc/channel_map_config_default.json` (irrelevant to our model)
- Implementing reference-channel-aware metrics in `waved-audio-bench` (bench stays mic-only in SP#1 — refs become useful once SP#2 lands)
- Addressing the `_frame_tx` health-detection minor issue from v1 final review — deferred to follow-up
