# Waved Audio — SP#2 Acoustic Echo Cancellation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consume `AudioFrame.reference` (populated by SP#1.5 `PlaybackService`), run it through an adaptive echo canceller plus optional noise suppressor, and emit a cleaned mono float signal (`ProcessedAudioFrame`) to downstream consumers (meeting recorder, future uplink). Wire `Command::SetAudioMode` so the user can flip AEC + NS on/off at runtime from the UI.

**Architecture:** New `waved_audio::aec` module wraps speexdsp's `SpeexEchoState` and `SpeexPreprocessState`. `AudioService::run` becomes the DSP boundary: raw `AudioFrame` in, `ProcessedAudioFrame` out. Stats continue to broadcast raw frames; meeting recording moves to processed frames. Implementation is i16 internally (speexdsp native) but exposes `Vec<f32>` at the module API, matching the design spec's boundary choice.

**Tech Stack:** Rust 2021, speexdsp-sys (vendored C), tokio mpsc/broadcast. Cross-compiled via `cross`.

**Reference spec:** `docs/superpowers/specs/2026-04-20-waved-audio-aec-design.md`

---

## File Structure

```
crates/waved-audio/
  Cargo.toml                                          (modify) add speexdsp-sys, bindgen build-dep
  build.rs                                            (new)    build + link vendored speexdsp
  vendor/speexdsp/                                    (new)    pinned speexdsp snapshot (git submodule or vendored tarball)
  src/
    aec/
      mod.rs                                          (new)    EchoCanceller trait + factory
      speex.rs                                        (new)    SpeexEchoCanceller + SpeexNoiseSuppressor
      passthrough.rs                                  (new)    no-op canceller for tests / Raw mode
    processed_frame.rs                                (new)    ProcessedAudioFrame + i16↔f32 conversions
    lib.rs                                            (modify) add aec, processed_frame; rewire AudioService

crates/waved-state/
  src/lib.rs                                          (modify) AudioMode::Raw variant, if not already present

crates/waved-meeting/
  src/lib.rs                                          (modify) accept ProcessedAudioFrame; convert mono_f32 → i16 for encoder

crates/waved-aec-probe/                               (new binary)
  Cargo.toml                                          (new)    depends on waved-audio
  src/main.rs                                         (new)    tone generator for on-device ERLE measurement

scripts/
  test-aec-on-device.sh                               (new)    ERLE smoke test
```

## Task classification

| Tasks | Classification | Gate |
|---|---|---|
| 1 | Vendoring / build-script | `cargo build -p waved-audio --features capture-alsa` green on both host and aarch64 cross |
| 2, 3, 4, 5 | Host-only TDD | `cargo test -p waved-audio` green |
| 6 | Host-only TDD (touches `AudioService` which flows to ALSA but contains no ALSA calls) | `cargo test` green; `cargo build` for aarch64 cross green |
| 7 | Host-only TDD | `cargo test -p waved-meeting` green |
| 8 | ALSA-touching helper binary | builds on both targets |
| 9 | Device-touching | push-to-device, ERLE ≥ 20 dB |

---

## Task 1: Vendor speexdsp and wire the build

**Files:**
- Modify: `crates/waved-audio/Cargo.toml`
- New: `crates/waved-audio/build.rs`
- New: `crates/waved-audio/vendor/speexdsp/` (pinned upstream tarball or submodule — see Step 2)

**Rationale:** speexdsp has no maintained high-level Rust crate. We vendor the C sources at a pinned commit (1.2.1 stable release, committed source tree) and compile via `cc` crate. Keeps us deterministic across cross-compile targets.

**Host vs device note:** speexdsp builds as a static archive. `cross` cross-compiles C via the toolchain in the container image; confirm a dummy `cc::Build::new().file("vendor/speexdsp/libspeexdsp/mdf.c").compile("speexdsp_probe")` works for aarch64 before proceeding.

- [ ] **Step 1: Add dependencies**

  ```toml
  # Cargo.toml additions
  [dependencies]
  speexdsp-sys = { path = "speexdsp-sys" }  # we vendor locally for pinning

  [build-dependencies]
  cc = "1.0"
  ```

- [ ] **Step 2: Vendor the C sources**

  Pull speexdsp 1.2.1 sources into `vendor/speexdsp/`. Only what we need:
  - `libspeexdsp/mdf.c`       — acoustic echo canceller
  - `libspeexdsp/preprocess.c` — noise suppressor + residual echo suppressor
  - `libspeexdsp/fftwrap.c`, `kiss_fft.c`, `kiss_fftr.c` — FFT backend
  - `libspeexdsp/smallft.c`   — alternate FFT (keep in case build prefers)
  - `libspeexdsp/filterbank.c`, `math_approx.h`, `*.h` — utilities
  - Skip: resampler, jitter buffer, SBR (not needed by mdf/preprocess).

  Commit the tree under `vendor/speexdsp/` with an upstream `README.md.upstream` noting the pinned version and license (BSD 3-clause).

- [ ] **Step 3: `build.rs` compiles the vendored sources**

  ```rust
  fn main() {
      println!("cargo:rerun-if-changed=vendor/speexdsp");
      let mut b = cc::Build::new();
      b.include("vendor/speexdsp/include");
      b.include("vendor/speexdsp/libspeexdsp");
      b.define("HAVE_CONFIG_H", None);
      b.define("FLOATING_POINT", None);
      b.define("EXPORT", Some(""));
      b.files([
          "vendor/speexdsp/libspeexdsp/mdf.c",
          "vendor/speexdsp/libspeexdsp/preprocess.c",
          "vendor/speexdsp/libspeexdsp/fftwrap.c",
          "vendor/speexdsp/libspeexdsp/kiss_fft.c",
          "vendor/speexdsp/libspeexdsp/kiss_fftr.c",
          "vendor/speexdsp/libspeexdsp/filterbank.c",
      ]);
      b.warnings(false); // upstream emits noisy but harmless warnings
      b.compile("speexdsp");
  }
  ```

- [ ] **Step 4: Hand-rolled sys bindings**

  Only the 6 functions we actually call (extracted into `src/aec/sys.rs`):

  ```rust
  // Echo canceller
  pub fn speex_echo_state_init(frame_size: c_int, filter_length: c_int) -> *mut SpeexEchoState;
  pub fn speex_echo_state_destroy(st: *mut SpeexEchoState);
  pub fn speex_echo_cancellation(st: *mut SpeexEchoState, rec: *const i16, play: *const i16, out: *mut i16);

  // Preprocessor (NS + residual echo suppression)
  pub fn speex_preprocess_state_init(frame_size: c_int, sampling_rate: c_int) -> *mut SpeexPreprocessState;
  pub fn speex_preprocess_state_destroy(st: *mut SpeexPreprocessState);
  pub fn speex_preprocess_run(st: *mut SpeexPreprocessState, x: *mut i16) -> c_int;
  pub fn speex_preprocess_ctl(st: *mut SpeexPreprocessState, request: c_int, ptr: *mut c_void) -> c_int;
  ```

  Bindgen is overkill; hand-typed is 30 LOC. Mirror upstream headers exactly.

- [ ] **Step 5: Cross-build sanity**

  ```bash
  cargo build -p waved-audio
  cross build --target aarch64-unknown-linux-gnu -p waved-audio --features capture-alsa
  ```

  Both must succeed before any other task proceeds.

**Commit:** `build(audio): vendor speexdsp 1.2.1 for SP#2 AEC`

---

## Task 2: `EchoCanceller` trait + `PassthroughCanceller`

**Files:**
- New: `crates/waved-audio/src/aec/mod.rs`
- New: `crates/waved-audio/src/aec/passthrough.rs`
- Modify: `crates/waved-audio/src/lib.rs` — `pub mod aec;`

**Rationale:** Start with the smallest possible surface. The trait decouples callers from speexdsp and gives us a no-op implementation for `AudioMode::Raw` and for unit tests that shouldn't pull in DSP.

- [ ] **Step 1: Define trait**

  ```rust
  // src/aec/mod.rs
  pub trait EchoCanceller: Send {
      /// Process one frame. `mic` and `reference` must both be `frame_size`
      /// i16 samples (mono, caller is responsible for channel mixdown).
      /// Writes the cancelled signal into `out`.
      fn process(&mut self, mic: &[i16], reference: &[i16], out: &mut [i16]);
      fn frame_size(&self) -> usize;
  }

  pub fn passthrough(frame_size: usize) -> Box<dyn EchoCanceller> {
      Box::new(passthrough::PassthroughCanceller { frame_size })
  }
  ```

- [ ] **Step 2: PassthroughCanceller** — copy `mic` to `out`, ignore `reference`. Unit test: output equals input.

- [ ] **Step 3: Test** — feed sine + reference, assert output == mic.

**TDD gate:** `cargo test -p waved-audio aec::` green.

---

## Task 3: `SpeexEchoCanceller`

**Files:**
- New: `crates/waved-audio/src/aec/speex.rs`
- Modify: `crates/waved-audio/src/aec/mod.rs` — factory function

**Rationale:** Wrap the speex FFI. Manage state lifecycle. Handle 2-ch reference → mono downmix inside `process()`.

- [ ] **Step 1: State wrapper with Drop**

  ```rust
  pub struct SpeexEchoCanceller {
      state: *mut sys::SpeexEchoState,
      frame_size: usize,
      ref_mono_buf: Vec<i16>,
  }
  unsafe impl Send for SpeexEchoCanceller {} // sys state is thread-local, we own it

  impl Drop for SpeexEchoCanceller {
      fn drop(&mut self) {
          unsafe { sys::speex_echo_state_destroy(self.state); }
      }
  }
  ```

- [ ] **Step 2: Constructor**

  ```rust
  pub fn new(frame_size: usize, sample_rate: u32) -> Self {
      let tail = sample_rate as usize * FILTER_TAIL_MS / 1000; // 200ms -> 9600 samples @48k
      let state = unsafe { sys::speex_echo_state_init(frame_size as c_int, tail as c_int) };
      assert!(!state.is_null(), "speex_echo_state_init failed");
      Self { state, frame_size, ref_mono_buf: vec![0; frame_size] }
  }
  ```

- [ ] **Step 3: `process()` with reference downmix**

  Reference is interleaved L/R. For each sample, `ref_mono_buf[i] = ((L + R) / 2) as i16` (saturating).

- [ ] **Step 4: Host test — ERLE on synthetic echo**

  ```rust
  #[test]
  fn cancels_synthetic_echo_above_20db() {
      let sr = 48000;
      let frame = 480; // 10ms
      let ref_sig: Vec<i16> = gen_sine(1000.0, sr, 2 * sr);   // 2 seconds of 1kHz
      let mic = simulate_echo(&ref_sig, 0.3, 20); // 30% gain, 20 sample delay
      let mut canceller = SpeexEchoCanceller::new(frame, sr as u32);
      let mut out = vec![0i16; mic.len()];
      for i in (0..mic.len()).step_by(frame) {
          canceller.process(&mic[i..i+frame], &ref_sig[i..i+frame], &mut out[i..i+frame]);
      }
      // Skip first ~500ms for filter to converge.
      let converged = sr / 2;
      let input_rms = rms_i16(&mic[converged..]);
      let out_rms = rms_i16(&out[converged..]);
      let erle_db = 20.0 * (input_rms / out_rms).log10();
      assert!(erle_db > 20.0, "expected >20dB ERLE, got {:.1} dB", erle_db);
  }
  ```

**TDD gate:** test green. Run with `RUSTFLAGS="-C opt-level=2"` locally — speex in debug can be slow enough to make the test flaky on small laptops.

---

## Task 4: `SpeexNoiseSuppressor`

**Files:**
- Modify: `crates/waved-audio/src/aec/speex.rs`
- Modify: `crates/waved-audio/src/aec/mod.rs` — expose `NoiseSuppressor` trait

**Rationale:** speexdsp's preprocessor ALSO handles residual echo suppression when associated with an echo state. We want that association active when AEC is on.

- [ ] **Step 1: Trait**

  ```rust
  pub trait NoiseSuppressor: Send {
      fn process(&mut self, samples: &mut [i16]);
  }
  ```

- [ ] **Step 2: `SpeexNoiseSuppressor::new(frame_size, sample_rate, echo_state: Option<&SpeexEchoState>)`**

  If `echo_state` is supplied, call `speex_preprocess_ctl(SPEEX_PREPROCESS_SET_ECHO_STATE, echo_state)` — this enables residual echo suppression.

- [ ] **Step 3: Host test — noise-only signal is suppressed by ≥ 8 dB**

  Feed 2 seconds of white noise at low amplitude. Assert `rms_after < rms_before × 0.4`.

**TDD gate:** test green.

---

## Task 5: `ProcessedAudioFrame`

**Files:**
- New: `crates/waved-audio/src/processed_frame.rs`
- Modify: `crates/waved-audio/src/lib.rs` — `pub mod processed_frame;`

**Rationale:** The pipeline boundary. i16↔f32 conversions in one place with scaling constants aligned.

- [ ] **Step 1: Struct per design spec §4.**

- [ ] **Step 2: `from_raw_frame(frame: &AudioFrame, mic_mono_f32: Vec<f32>, reference_f32: Vec<f32>) -> Self`**

- [ ] **Step 3: Conversion helpers**

  ```rust
  pub fn i16_to_f32(src: &[i16], dst: &mut [f32]) { … }         // scale 1.0/32768.0
  pub fn f32_to_i16_saturating(src: &[f32], dst: &mut [i16]) { … }
  pub fn mix_8ch_to_mono(interleaved: &[i16], channels: usize) -> Vec<i16> { … }
  pub fn mix_2ch_to_mono(interleaved: &[i16]) -> Vec<i16> { … }
  ```

- [ ] **Step 4: Host tests** — round-trip i16 → f32 → i16 preserves samples exactly up to rounding; mono mix of identical channels equals the original; 8-ch ch0-select returns every 8th sample.

**TDD gate:** unit tests green.

---

## Task 6: Wire DSP into `AudioService::run`

**Files:**
- Modify: `crates/waved-audio/src/lib.rs` — `AudioService` fields + `run` loop

**Rationale:** The spec §4 pipeline. This is the single place where mic + reference become a processed mono signal.

- [ ] **Step 1: Add fields**

  ```rust
  pub struct AudioService {
      // …existing…
      echo_canceller: Box<dyn EchoCanceller>,
      noise_suppressor: Option<Box<dyn NoiseSuppressor>>,
      processed_tx: broadcast::Sender<ProcessedAudioFrame>,
  }
  ```

- [ ] **Step 2: Construct in `AudioService::new` based on audio_mode initial value**

  Default audio_mode is `NoiseReduction` → speex AEC + speex NS.

- [ ] **Step 3: Frame processing branch**

  ```rust
  frame = self.frame_rx.recv() => {
      if let Some(mut frame) = frame {
          apply_mic_mute(&mut frame.data, muted);
          let vu = compute_vu(&frame.data, 8);
          self.audio_tx.send_modify(|s| s.vu_level.copy_from_slice(&vu));
          let _ = self.stats_tee.send(frame.clone()); // raw frame for stats

          let mic_mono = mix_8ch_to_mono(&frame.data, frame.channels as usize);
          let ref_mono = match &frame.reference {
              Some(r) => mix_2ch_to_mono(r),
              None => vec![0i16; mic_mono.len()],
          };
          let mut aec_out = vec![0i16; mic_mono.len()];
          self.echo_canceller.process(&mic_mono, &ref_mono, &mut aec_out);
          if let Some(ns) = self.noise_suppressor.as_mut() {
              ns.process(&mut aec_out);
          }

          let processed = ProcessedAudioFrame::from_raw_frame(&frame, aec_out, ref_mono);
          let _ = self.processed_tx.send(processed.clone());
          let _ = self.meeting_tx.try_send_processed(processed); // or equivalent
      }
  }
  ```

- [ ] **Step 4: Handle `Command::SetAudioMode`**

  Rebuild `echo_canceller` and `noise_suppressor` based on new mode (Raw → Passthrough + None, NoiseReduction → Speex + Speex, NoiseReductionOff → Speex + None). Cheap; happens on user action.

- [ ] **Step 5: Host test**

  Inject a synthetic `AudioFrame` with reference = 1kHz sine, mic = same sine at 50% gain. Run through `AudioService::process_frame` (extract the DSP block as a testable helper). Assert the processed frame's `mic_mono_f32` RMS < `raw_mic` RMS by ≥ 10 dB.

**TDD gate:** `cargo test -p waved-audio` green; `cargo build` for aarch64 cross green.

---

## Task 7: Meeting recorder accepts ProcessedAudioFrame

**Files:**
- Modify: `crates/waved-meeting/src/lib.rs`

**Rationale:** Meeting upload should carry the echo-cancelled mono signal. Convert mono_f32 → i16 at the encoder boundary.

- [ ] **Step 1: Replace the meeting mpsc type**

  `mpsc::Sender<AudioFrame>` → `mpsc::Sender<ProcessedAudioFrame>`. Or keep both and add a new channel; the old channel is deprecated.

- [ ] **Step 2: Encoder writes the processed mono**

  Wherever the meeting code writes PCM to disk, it converts `mic_mono_f32` → `Vec<i16>` via `f32_to_i16_saturating`, writes that.

- [ ] **Step 3: Keep raw mic 8-ch available for diagnostic replay**

  Optional: also write `raw_mic_i16` to a sidecar file `meeting_XXX_raw.pcm`. Use for post-hoc AEC tuning.

- [ ] **Step 4: Existing tests update to new type.**

**TDD gate:** `cargo test -p waved-meeting` green.

---

## Task 8: `waved-aec-probe` helper binary

**Files:**
- New: `crates/waved-aec-probe/Cargo.toml`
- New: `crates/waved-aec-probe/src/main.rs`

**Rationale:** Spec §9 smoke test needs a way to play a known tone through the same `PlaybackService` waved uses. Reusing the library makes this reproducible on device.

- [ ] **Step 1: CLI**

  ```
  waved-aec-probe --device hw:0,1 --freq 1000 --duration 3 --amplitude 0.3
  ```

- [ ] **Step 2: Build a `PlaybackService` and submit sine batches for the requested duration.**

- [ ] **Step 3: Cross-build and push.**

**Device gate:** binary runs, speaker produces the tone.

---

## Task 9: On-device ERLE smoke test

**Files:**
- New: `scripts/test-aec-on-device.sh`

**Rationale:** One-click verification that AEC works on hardware. Runs in CI once we have a loopback fixture board; before that, runs manually.

- [ ] **Step 1: Script flow**

  ```bash
  ssh-to-device or adb
  ensure waved running
  amixer sset "5707_A Master" 50
  baseline=$(cat /tmp/audio_stats.json | jq '.mic_post_aec_rms')
  launch waved-aec-probe in background for 3s
  wait 1s (let filter converge)
  active=$(cat /tmp/audio_stats.json | jq '.mic_post_aec_rms')
  reference=$(cat /tmp/audio_stats.json | jq '.reference_rms[0]')
  [[ $reference > 0.1 ]] || fail "reference tap did not see the probe tone"
  erle=$(python3 -c "import math; print(20*math.log10($baseline_mic / $active))")
  [[ $erle > 20 ]] || fail "ERLE $erle < 20 dB"
  ```

- [ ] **Step 2: Add `mic_post_aec_rms` to audio_stats.json**

  `stats_collector` will need to also subscribe `processed_tx` and compute RMS on the mono processed signal. Small change. If we're worried about scope creep, alternative: have `waved-aec-probe` print the ERLE from a brief in-process computation using the reference tap + the processed frame broadcast.

- [ ] **Step 3: Run manually on device. Document the measured ERLE in the commit message.**

**Device gate:** script passes, ERLE ≥ 20 dB, reference_rms tracks the tone, both decay when tone stops.

---

## Exit criteria for SP#2

- All 9 tasks' gates pass.
- `cargo test -p waved-audio` green (host).
- `cross build --target aarch64-unknown-linux-gnu -p waved --features "ui,capture-alsa"` green.
- On-device ERLE ≥ 20 dB measured by `scripts/test-aec-on-device.sh`.
- `Command::SetAudioMode(Raw)` disables AEC without xruns; `SetAudioMode(NoiseReduction)` re-enables it without an audible discontinuity on the uplink.
- `TODO.md` SP#2 line checked off.

## Follow-ups (not blocking SP#2 close)

- Dynamic filter-tail tuning based on measured acoustic delay.
- AEC state diagnostics exposed on `audio_stats.json` (divergence counter, convergence time estimate).
- Meeting sidecar `meeting_XXX_raw.pcm` retention policy — disk fills fast at 8ch/48k/i16.
