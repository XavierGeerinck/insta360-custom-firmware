# Waved Audio — Capture Foundation + Measurement Harness

**Date:** 2026-04-16
**Status:** Draft — pending user review
**Scope:** Sub-project #1 of 6 in the libInstaAudio replacement effort
**Supersedes:** Stubbed `waved-audio/src/ffi.rs::init_audio` (no-op)

---

## 1. Context

### 1.1 Why this exists

Insta360's stock firmware ships an 8-stage audio pipeline from the Roomix TEA SDK (`libTEAProcessSdk.so` v3.10.1.1153), orchestrated by `libInstaAudio.so`. Analysis in `docs/DEBUG_LATENCY.md` (git commit `c567e7f`) documents concrete quality and latency bugs:

- Multi-second effective muting after speech pauses (AEC + AGC + beamformer converge on silence independently)
- `ec_aggr=1.5` over-subtracts echo; near-end speech attenuated while AEC re-adapts
- `agc_alpha=0.1`, `agc_attack=0.5` — orders of magnitude too slow for conversational speech
- WPE dereverberation uses 1024-point FFT (~21 ms window latency alone)

The user's priorities (confirmed in brainstorming):

| Priority | Description |
|---|---|
| A | Muffled / distant voices in recordings |
| B | Echo / speaker feedback |
| C | Background noise poorly suppressed |
| D | Room reverb ("bathroom" sound) |
| F | Speaker output (playback) sounds bad |
| **H (critical)** | **Latency — hear own voice delayed** |

**Latency is the north-star constraint.** Teams-class quality (DeepFilterNet-family NS, AEC3-class echo cancellation) is achievable in pure Rust if the capture + routing infrastructure does not impose unnecessary buffering.

### 1.2 Parent decomposition

The full libInstaAudio replacement is broken into six sub-projects, each with its own spec → plan → implementation cycle:

1. **Capture foundation + measurement harness** ← *this spec*
2. AEC + playback path (addresses B, H)
3. Beamforming — classical MVDR/DAS, 6 modes + DOA (addresses A)
4. Neural NS + dereverb — DeepFilterNet 3 class (addresses C, D)
5. Playback EQ + gain staging (addresses F)
6. UAC2 routing for Wave Controller (Mac)

ASR (Silero VAD, WeNet, SenseVoice) is already open-source and out of scope.

### 1.3 What this sub-project delivers

Everything downstream sub-projects need as prerequisite:

- Real 10-channel ALSA capture from `pcmC0D0c`, replacing the current stub
- Offline-first development loop: captures on device → transferred to host → processed by same Rust code
- Measurement harness: latency probe, per-channel SNR, clipping, correlation matrix, stats sidecar
- A set of ~15-20 "golden" scenario fixtures that become permanent regression inputs for every subsequent sub-project

**Out of scope for this sub-project:** any DSP (no AEC, no NS, no beamforming, no EQ). Those arrive in sub-projects 2-6.

---

## 2. Goals

1. Capture raw 10-ch / 48 kHz / 32-bit PCM from the physical mic array reliably on device
2. Expose channel-map-aware 8-channel (mics only, references dropped) frames to `AudioService`
3. Enable **offline iteration**: any change to `AudioService` can be validated bit-identically on the dev machine using captured fixtures
4. Provide objective measurements (latency, SNR, clipping, correlation, xrun rate) — so every future sub-project can claim "better than stock" with numbers
5. Minimum ALSA latency within driver constraints
6. Never drop capture due to downstream backpressure; never starve ALSA

## 3. Non-goals

- DSP of any kind (deferred to sub-projects 2-6)
- UAC2 / USB audio class integration (sub-project 6)
- Mic-health continuous monitoring beyond one-shot reports (future)
- Snd-dummy-based host testing (marginal value vs complexity)
- Full simulation of synthetic speech + RIR (deferred to sub-project 4 if still needed)

## 4. Constraints

- **Hardware:** 8 MEMS mics + 2 loopback/reference channels, exposed as ALSA card 0 device 0 (`pcmC0D0c`). Format fixed by kernel driver: 48 kHz / S32_LE / 10 ch. Amlogic AML-AUGESOUND, ARM Cortex-A53 (aarch64).
- **Latency budget:** "as low as physically possible on this hardware". No algorithm in this sub-project may impose > 1 ALSA period of buffering on top of capture.
- **No coexistence with stock pspapp audio server** — waved fully owns ALSA. Acceptable because waved already replaces pspapp at runtime.
- **Cross-compilation via `cross`** (aarch64-unknown-linux-gnu) — any host-only dependencies must be `cfg`-gated, not introduced unconditionally.
- **CI budget:** Tier 1 + Tier 2 tests must complete in < 30 seconds.

---

## 5. Architecture

### 5.1 Picture

```
 ┌─────────── on device (aarch64) ───────────┐    ┌───── dev machine (host) ─────┐
 │                                           │    │                              │
 │  pcmC0D0c ──► CaptureThread ──► frame_tx  │    │  *.pcm fixture files         │
 │  (ALSA)       (OS thread)      (mpsc)     │    │      │                       │
 │                    │              │       │    │      ▼                       │
 │                    │              ▼       │    │  ReplayReader                │
 │                    │       AudioService   │    │      │   (cfg: replay)       │
 │                    │      (existing loop) │    │      ▼                       │
 │                    │       apply_mic_mute │    │   frame_tx (mpsc, shared)    │
 │                    │       compute_vu     │    │      │                       │
 │                    │           │          │    │      ▼                       │
 │                    ▼           ▼          │    │  AudioService (SAME code)    │
 │           optional tee    meeting_tx      │    │      │                       │
 │           /tmp/cap.pcm                    │    │      ▼                       │
 │                    │                      │    │  waved-audio-bench CLI       │
 │                    ▼                      │    │  ──► metrics.json            │
 │            /tmp/audio_stats.json          │    │                              │
 └───────────────────────────────────────────┘    └──────────────────────────────┘
```

### 5.2 Principles

1. **Same `AudioService` code path on device and host.** Only the frame source swaps (real ALSA vs file replay). Guarantees offline iteration is bit-identical to on-device processing.
2. **Capture runs on a dedicated OS thread**, not a tokio worker. ALSA `snd_pcm_readi` blocks; a tokio task would stall the runtime.
3. **Feature-flag gated:**
   - `capture-alsa` — real capture (default for the aarch64 `ui` build)
   - `capture-replay` — file-backed replay (dev machine)
   - `simulate` — existing synthetic silence generator (integration tests)
   - Mutually exclusive at runtime; compile-time validated with a `compile_error!` guard.
4. **Measurement is separate from the capture path.** Stats collection is a consumer of `frame_tx` via a broadcast tee, never in the hot path. `waved-audio-bench` is a host binary that *replays* fixtures — doesn't touch ALSA.

---

## 6. Components

### 6.1 Capture path

**`waved-audio/src/capture/mod.rs` — `PcmSource` trait**

Abstraction boundary between "something that produces raw 10-ch i32 frames" and `CaptureThread`'s downmix + emit logic. Three implementations exist:

```rust
pub trait PcmSource: Send {
    fn read(&mut self, buf: &mut [i32]) -> std::io::Result<usize>;
}
```

- `AlsaPcmSource` — on-device, wraps `snd_pcm_readi`
- `ReplayPcmSource` — host, wraps a PCM file reader
- `MockPcmSource` — tests, wraps `VecDeque<Vec<i32>>`

`CaptureThread` is generic over `P: PcmSource` so the same thread code (channel-map downmix, timestamping, backpressure policy) runs against any source.

**`waved-audio/src/capture/alsa.rs` — `AlsaPcmSource` + `CaptureThread`** (on-device)

- Opens `pcmC0D0c` at 48 kHz / 32-bit / 10 ch (matches kernel driver config)
- Blocking OS thread; spawned via `std::thread::Builder::name("audio-capture")`
- Stopped via `Arc<AtomicBool>` polled between reads
- Period size: 256 frames (~5.3 ms), buffer: 4 periods (~21 ms total). Negotiates down if driver rejects; final values logged at `info!`.
- Deps: `alsa` crate (safe bindings, well-maintained — also used by `cpal`, `bevy_audio`)

**`waved-audio/src/capture/channel_map.rs` — `ChannelMap`**

- Parses `/etc/channel_map_config_default.json` at startup
- Produces `{ physical_mics: [u8; 8], reference_channels: Vec<u8> }`
- CaptureThread uses it to downselect 10→8 before frame emission
- Fallback on missing/malformed file: hardcoded default (mics 0-7, refs 8-9) + `"channel_map_source": "fallback"` surfaced in stats JSON

**`waved-audio/src/capture/replay.rs` — `ReplayReader`** (host only, `cfg(feature = "capture-replay")`)

- `ReplayReader::new(path, format) -> Self` → `spawn(frame_tx) -> JoinHandle`
- Reads raw interleaved PCM, paces frames via `tokio::time::sleep` at real-time rate
- `--speed` flag: `1.0` = real-time, `10.0` = fast, `0.0` = drain instantly (unit tests)
- Emits the same `AudioFrame` shape as `CaptureThread`; `AudioService` can't tell them apart

### 6.2 Measurement path

**`waved-audio/src/metrics.rs`** — pure functions, no state, no I/O, infallible:

```rust
pub fn rms(samples: &[i16]) -> f32;
pub fn rms_per_channel(interleaved: &[i16], channels: usize) -> Vec<f32>;
pub fn clipping_rate(samples: &[i16], threshold: i16) -> f32;
pub fn correlation_matrix(interleaved: &[i16], channels: usize) -> Vec<Vec<f32>>;
pub fn dc_offset(samples: &[i16]) -> f32;
```

Every one unit-testable with handwritten inputs. Degenerate inputs (empty, zero-variance) return defined values (0.0), not NaN. No `Result<_, _>` — metrics are infallible by contract.

**`waved-audio/src/stats_collector.rs`** — tokio task, taps `frame_tx` via `broadcast` tee. Writes `/tmp/audio_stats.json` every 500 ms using atomic-rename pattern (write to `.tmp`, rename to final). Never in the capture hot path; a slow disk cannot starve ALSA.

**`crates/waved-audio-bench/`** (new binary crate)

```
waved-audio-bench <input.pcm> --metric rms,clipping,corr [--config channel_map.json] [--json]
```

- Instantiates a `ReplayReader`, feeds the real `AudioService`, collects metrics, emits JSON
- Same binary runs on host and device
- **Deterministic:** two runs on the same input must produce byte-identical output (enforced in Tier 2 tests)

**`crates/waved-latency-probe/`** (new binary crate, on-device only)

- Plays a known chirp (5 kHz / 20 ms burst, 960 samples @ 48 k) via `aplay /tmp/chirp.wav`
- Simultaneously captures mic (2 s window, 500 ms lead-in)
- Cross-correlates recorded mic ch 0 vs the chirp reference → reports mouth-to-mic delay in ms
- Single number output; run repeatedly to assess variance

### 6.3 Scenario fixture recorder

**`waved-capture-record`** (tiny on-device binary, or `waved-audio-bench --record` subcommand — implementation-time choice):

```
waved-capture-record --duration 30 --out /mnt/udisk/fixtures/single-speaker.pcm
```

Pure dump — raw 8-channel i16 PCM post-downmix. Operator speaks/plays scenario, transfers file to host, commits `.meta.json` sidecar describing it.

### 6.4 Sample-format policy

`AudioFrame` stays `Vec<i16>` for this sub-project. Rationale:

- Matches existing `AudioService`, `compute_vu`, `apply_mic_mute` — minimum churn
- 32-bit ALSA samples are downscaled in `CaptureThread` before frame emission
- **Sub-project #2 (AEC) will switch to `Vec<f32>`** — DSP libraries want floats, and AEC needs the headroom. Contained in that sub-project's blast radius.

---

## 7. Data flow

### 7.1 Startup — real capture (on-device)

```
main.rs
  └─> AudioService::new(..)
        └─> CaptureThread::spawn(frame_tx, stop_flag)
              1. Parse /etc/channel_map_config_default.json → ChannelMap (fallback: default)
              2. snd_pcm_open("hw:0,0", SND_PCM_STREAM_CAPTURE, 0)
              3. Negotiate: 48kHz, S32_LE, 10ch, period=256, buffer=1024
              4. Log final params (kernel may down-negotiate)
              5. snd_pcm_prepare → snd_pcm_start
              6. loop {
                   snd_pcm_readi(buf, 256) -> 10ch × 256 × i32
                   downmix_channels(buf, &channel_map) -> 8ch × 256 × i16
                   frame_tx.try_send(AudioFrame { data, ts = Instant::now(), ... })
                     └─ on backpressure: drop newest + increment dropped_counter
                 }
```

If steps 2-4 fail → `CaptureThread` returns error, `AudioService` logs fatally, process exits. Watchdog restarts waved. Silent degraded mode is worse than a restart.

### 7.2 Startup — replay (host)

```
waved-audio-bench capture.pcm --metric rms,clipping,corr
  └─> ReplayReader::spawn(file, frame_tx)
        loop {
          read 8ch × 960 samples × i16 from file
          tokio::time::sleep(20ms)   -- pace at real-time
          frame_tx.send(AudioFrame { ... })
        }
```

Identical `AudioFrame` contract; `AudioService` runs unchanged.

### 7.3 Steady-state frame flow (existing code, re-used unchanged)

```
CaptureThread / ReplayReader
       │ (AudioFrame via mpsc, bounded=32)
       ▼
 AudioService::run loop
       │
       ├─► apply_mic_mute(&mut frame.data, muted)   [existing]
       ├─► compute_vu(&frame.data, 8)               [existing]
       └─► meeting_tx.try_send(frame)               [existing]

 (sidecar) stats_collector via broadcast tee
       └─► every 500ms: write /tmp/audio_stats.json (atomic rename)
```

The broadcast tee is the only new element. `AudioService` semantics unchanged.

### 7.4 Backpressure policy

`frame_tx` bounded at 32 frames (~640 ms). On full:

- **Drop newest frame**, increment `capture.dropped_frames` counter
- Log once per second (rate-limited)
- Never block capture thread — dropping is strictly better than starving ALSA (which produces `EPIPE` xruns)

Dropping newest (not oldest) preserves contiguous history for future AEC reference alignment (sub-project #2).

Stats task on `broadcast::error::Lagged` → log + continue. Lost stats are fine; lost audio is not.

### 7.5 Timestamping

- **Capture:** `Instant::now().duration_since(start_epoch).as_millis()` at the moment `snd_pcm_readi` returns
- **Replay:** synthesized as `frame_index * 20` — monotonic, exact, deterministic metrics across runs

Fixture files get sidecar `.meta.json` with `started_at`, `duration_ms`, `channel_count`, scenario description.

### 7.6 Shutdown

`Command::Shutdown` → `AudioService` sets `stop_flag` → `CaptureThread` notices between reads → `snd_pcm_drain` + `snd_pcm_close` → thread joins. Bounded by one period length (~5 ms) + drain, total never more than ~30 ms.

### 7.7 Latency probe sequence

```
waved-latency-probe
  ├─► write /tmp/chirp.wav (5kHz 20ms burst, 960 samples @ 48k)
  ├─► spawn capture thread → /tmp/probe_capture.pcm (2s duration)
  ├─► 500ms after capture starts: Command::new("aplay").arg("/tmp/chirp.wav").spawn()
  ├─► wait for capture to finish
  ├─► cross_correlate(captured_ch0, chirp_reference) → peak_index
  └─► delay_ms = peak_index × 1000 / 48000
```

One number out. Expected variance < 2 ms across repeats; high variance is itself a diagnostic (jitter / scheduling issue).

---

## 8. Error handling

### 8.1 ALSA open / negotiation (fatal)

`snd_pcm_open` failure with EBUSY/ENOENT/EACCES → log with full context (device node, perms, dmesg hint) → `CaptureThread` returns Err → `AudioService::run` logs fatal + exits → watchdog restarts waved.

Rationale: without capture the device has no mic; silent degraded mode is worse.

Exception: **weaker negotiated params** (e.g. 44.1 kHz available, not 48 kHz) — accept and log at `warn!`. Flag in stats. Future DSP reads actual rate from `AudioFrame.sample_rate`, not a hardcoded constant.

### 8.2 ALSA read errors (recoverable)

```rust
match snd_pcm_readi(...) {
    Ok(frames) => emit(frames),
    Err(EPIPE) => { warn!("xrun, count={xrun_count}"); xrun_count += 1; snd_pcm_prepare(); /* continue */ }
    Err(ESTRPIPE) => { snd_pcm_resume() or prepare() with backoff },
    Err(transient) => retry up to 3x with 10ms backoff,
    Err(other) => return Err (CaptureThread dies, AudioService escalates to fatal),
}
```

`xrun_count` exposed in stats JSON. Sustained xruns (>5/s) logged as `error!` — real-time scheduling problem.

### 8.3 Channel map config

| Case | Action |
|---|---|
| File missing | Warn, use hardcoded default, continue, flag `channel_map_source: fallback` in stats |
| Bad JSON | Error with parse detail, fall back, continue, flag |
| Indices out of 0..10 | Error, fall back, continue, flag |

All three continue capture — hardware layout is stable enough that a sane default is usable. Fallback is surfaced so the operator knows.

### 8.4 Replay reader (host)

| Case | Action |
|---|---|
| `File::open` ENOENT | Err → bench exits code 1 |
| Incomplete trailing frame | Log + truncate + EOF |
| Unexpected EOF mid-frame | Log + EOF (not an error) |
| Corrupted `.meta.json` | Log + treat as 8ch/48k/i16 defaults |

Host-side path; errors propagate to process exit code for CI.

### 8.5 Metrics — degenerate-input contract

All `metrics.rs` functions return defined values on edge cases:

- `rms(&[])` → 0.0
- `correlation(a, b)` where `variance(a) < EPSILON` → 0.0 (dead mic)
- No NaN / Inf ever returned

This is a tested contract, not an incidental behavior.

### 8.6 Latency probe failure modes

| Case | Action |
|---|---|
| `aplay` missing or fails | Non-zero exit, message "speaker playback failed" |
| No chirp detected (peak corr < threshold) | Error: "chirp not heard — speaker muted? wrong device? mic dead?" + max corr value |
| Multiple peaks | Report strongest, log secondary-peak offsets (useful AEC data for SP #2) |
| Peak at absurd delay (>500 ms) | Report anyway, flag "outlier" — probably wrong mic channel |

Probe never retries silently; operator sees the diagnostic.

### 8.7 Stats writer I/O failures

`/tmp/audio_stats.json` write fails → log once, silence for 60 s, continue capturing. Stats are diagnostic, not required. Expose `last_write_error` in the next successful write.

### 8.8 Invariants

No `panic!()` in the capture hot path. Any violated invariant logs + continues with safe default. Panics allowed only at startup (setup errors) and in unit tests.

### 8.9 Non-errors worth surfacing

- **Continuously clipping channel** → structured warning every 5 s. Broken mic/preamp; not our fix in this SP, but we surface.
- **All-silent channel when others active** → log once as mic-health warning.

---

## 9. Testing

### 9.1 Three tiers

| Tier | What | Runs | Command |
|---|---|---|---|
| 1 — Unit | Pure-function tests (metrics, channel_map, downmix, backpressure) | Every commit | `cargo test -p waved-audio` |
| 2 — Integration | `ReplayReader` → `AudioService` → `waved-audio-bench` end-to-end on fixtures | Every audio commit | `cargo test -p waved-audio --features capture-replay` |
| 3 — On-device smoke | Real `pcmC0D0c` 30-s capture, stats schema + sanity checks | Manual before merge + nightly | `scripts/test-capture-on-device.sh` |

Snd-dummy host testing intentionally skipped — marginal value vs complexity; snd-dummy diverges from real driver behavior enough to give false confidence.

### 9.2 Committed synthetic fixtures

`tests/fixtures/audio/` (via Git LFS):

| File | Purpose |
|---|---|
| `silence_10s.pcm` | All zeros — VU zero, RMS zero |
| `tone_1khz_mono_2s.pcm` | 1 kHz sine on ch 0, silence elsewhere |
| `clip_saturated_1s.pcm` | i16::MAX everywhere — clipping rate ~1.0 |
| `chirp_5khz_burst_1s.pcm` | Latency probe reference |
| `correlation_pair_2s.pcm` | Ch 0 = tone, ch 1 = tone + 1-sample delay (known correlation) |

**Alternative considered:** generate synthetic fixtures in `build.rs` on demand. Cleaner, no LFS dep. **Decision deferred to implementation time** — prefer generate-on-demand if it doesn't complicate CI caching.

Real scripted scenarios (15-20 × 30-s captures: single/multi-speaker, echo, noise, reverb) — too large for LFS. Stored in a separate object store or on local disk with an `rsync` script. Not required for Tier 1/2 CI. Captured once at the start of implementation; reused by every subsequent sub-project.

### 9.3 Per-tier assertions

**Tier 1 (fast):**

- `rms()` on silence = 0.0; on full-scale ≈ i16::MAX
- `clipping_rate()` counts saturation correctly
- `correlation_matrix()` of `[tone, tone]` = diag 1.0, off-diag 1.0
- `correlation_matrix()` of `[tone, silence]` = off-diag 0.0 (no crash on dead mic)
- `ChannelMap::from_json` parses valid, rejects invalid, falls back on missing
- `downmix_10ch_to_8ch` preserves mapped, drops references
- Backpressure: feed 100 frames into cap=32, assert drop counter = 68, no panic

**Tier 2 (end-to-end):**

- Replay `silence_10s.pcm` → `AudioService` produces `vu_level = [0; 8]` consistently
- Replay `tone_1khz_mono_2s.pcm` → ch 0 VU elevated, others zero
- Replay `clip_saturated_1s.pcm` → stats (mocked to memory) shows clipping_rate ≈ 1.0
- Replay `chirp_5khz_burst_1s.pcm` through latency-probe logic → peak at sample 0 (±2)
- **Determinism:** replay same fixture twice → byte-identical metrics JSON

**Tier 3 (real hardware, manual):**

- `waved-capture-record --duration 10 --out /tmp/smoke.pcm` → file exists, size ≈ 10 s × 48 k × 8 ch × 2 B = 7.68 MB
- `/tmp/audio_stats.json` written ≥ 15 times during 10 s run
- `xrun_count` field present and numeric
- Per-channel RMS populated for all 8 mics (dead mic = 0.0, not missing)
- Latency probe × 5: stddev < 2 ms, mean logged as device baseline

### 9.4 Fixture capture procedure (one-time, on device)

```
waved-capture-record --duration 30 --out /mnt/udisk/fixtures/01-single-speaker.pcm
waved-capture-record --duration 30 --out /mnt/udisk/fixtures/02-two-speakers.pcm
waved-capture-record --duration 30 --out /mnt/udisk/fixtures/03-silence-quiet-room.pcm
waved-capture-record --duration 30 --out /mnt/udisk/fixtures/04-music-playing-from-speaker.pcm
waved-capture-record --duration 30 --out /mnt/udisk/fixtures/05-typing-noise-bg.pcm
waved-capture-record --duration 30 --out /mnt/udisk/fixtures/06-reverberant-room.pcm
# ~15-20 total, each with .meta.json sidecar describing scenario / expected SNR / dominant azimuth

adb pull /mnt/udisk/fixtures/ ./audio-fixtures/
# Upload to chosen storage; tests fetch on demand
```

### 9.5 Mocking ALSA for Tier 1

```rust
trait PcmSource {
    fn read(&mut self, buf: &mut [i32]) -> Result<usize>;
}
struct AlsaPcmSource { /* on-device */ }
struct MockPcmSource { frames: VecDeque<Vec<i32>> }
```

`CaptureThread` generic over `PcmSource`. Tests feed pre-built frame sequences and assert downmix correctness, backpressure counter, xrun handling (`MockPcmSource` returns `EPIPE` at frame 50 → assert counter increments, no crash).

### 9.6 CI budget

Tier 1 + Tier 2 total < 30 s. If it grows past that, gate heavy tests behind a `--features heavy-audio-tests` opt-in and keep default fast.

---

## 10. Open decisions (defer to implementation)

1. **Fixture storage strategy** — Git LFS vs `build.rs` synthesis vs object-store. Recommendation: `build.rs` synthesis for tiny synthetic fixtures (no LFS dep), object-store + rsync for large real scenarios.
2. **Large-fixture object store** — S3-compatible, local disk + rsync, or other. Deferred; local disk is fine for the first iteration.
3. **`waved-capture-record` as separate binary vs `waved-audio-bench --record`** — implementation choice; I'd lean separate for clarity.
4. **Where ChannelMap default constant lives** — committed JSON in `crates/waved-audio/config/channel_map_default.json`, loaded via `include_str!`, mirrors on-device file structure.

## 11. Success criteria

Sub-project #1 is done when:

1. `waved` running on device produces populated `/tmp/audio_stats.json` with all 8 mic RMS values, clipping rate, and xrun count, refreshed every 500 ms
2. `waved-latency-probe` reports a single mouth-to-mic delay in ms with < 2 ms stddev across 5 runs
3. `waved-audio-bench silence_10s.pcm --metric rms` on dev machine returns RMS = 0.0 for all 8 channels
4. `waved-audio-bench` on the same fixture twice produces byte-identical JSON
5. A scripted multi-speaker capture (`02-two-speakers.pcm`) transferred to host and replayed through `AudioService` produces the same per-channel RMS and correlation matrix as measured on-device (within 1% tolerance — quantization aside)
6. 15-20 golden scenario fixtures committed (meta only in git; data in object store / local disk)
7. `cargo test -p waved-audio` and `cargo test -p waved-audio --features capture-replay` both pass in < 30 s total
8. Mic-mute (`apply_mic_mute`) demonstrably zeroes captured audio — validated by muting via UI, recording 5 s, replaying through bench, asserting RMS = 0

## 12. Future hooks (for subsequent sub-projects)

This sub-project intentionally creates the seams that sub-projects 2-6 will plug into:

- `PcmSource` trait → sub-project #2 can add `AecReferencedPcmSource` wrapping the raw source
- `AudioFrame` stays `Vec<i16>` → sub-project #2 switches to `Vec<f32>` in one blast
- `stats_collector` broadcast tee → every future sub-project adds its own metrics consumers without touching capture
- `waved-audio-bench` `--metric` flag → every sub-project adds new metric names
- Golden fixtures corpus → every sub-project gains regression tests at zero marginal cost
