# Waved Audio — SP#4 Neural Noise Suppression + Dereverb Design

**Date:** 2026-04-23
**Depends on:** SP#1 capture foundation v2 (2026-04-16) + SP#2 AEC (2026-04-20, ERLE 22.7 dB speech-band) + SP#3 beamforming (2026-04-23, DOA ≤ 10°, FBR ≥ 4 dB in-room)
**Status:** spec — implementation plan to be created via `writing-plans`

---

## 1. Context

After SP#3 the pipeline is: 8-ch capture → DAS beamform (5 presets) → AEC (speex) → **speex NS** → mono f32 out. Noise suppression is the weakest link: speex NS is a 2003-era Wiener-filter design, tuned for telephony-band stationary noise. It visibly fails on non-stationary sources (keyboard clicks, door slams, HVAC spikes) and has no notion of dereverberation.

SP#4 replaces the speex NS stage with a **neural model running on the on-board Vivante VIP NPU**. The target is DeepFilterNet 3 (DFN3): 2 M params, 10 ms hop, 20 ms algorithmic latency, int8-quantisable, published pre-trained weights, and an upstream ONNX-export path. It covers both stationary and non-stationary noise and does light dereverberation.

The NPU is real on this device. Measured 2026-04-23 (see `docs/COMPUTE_HARDWARE.md` and `crates/waved-npu-probe/`): our own process can open `/dev/galcore`, dlopen `libOpenVX.so` + `libovxlib.so`, and obtain working VX and `vsi_nn` contexts. Stock firmware uses the same NPU for ASR via `libnnsdk.so` → `libovxlib.so` → `/dev/galcore`. SP#4 picks a runtime that gives us the same NPU without depending on stock.

## 2. Decisions (locked in brainstorming)

1. **Model: DeepFilterNet 3.** Chosen for: edge-targeted architecture (not a research-only model), clean ONNX export, int8 quantisation path, published benchmarks in the 10–30 ms frame regime, MIT/Apache license compatible with open firmware. MossFormer2 and DPCRN variants are candidates for a later SP#4.5 A/B; out of scope here.

2. **Runtime: onnxruntime + VSI-NPU EP (our own build).** The on-device `libonnxruntime.so.1.15.1` is CPU-only (no VSI provider — the VSI-NPU EP only landed upstream at ORT 1.17+). We cross-compile our own ORT ≥ 1.17 with `--use_vsinpu`, which pulls TIM-VX, links against the on-device `libovxlib.so` / `libGAL.so` / `/dev/galcore`. Output is a drop-in `libonnxruntime.so` shipped in `/data/waved/lib/`, not overlaid onto `/usr/lib`.

   Rationale vs the two rejected alternatives:
   - *TIM-VX direct from Rust* would save the ORT build but costs us hand-writing the DFN3 graph construction and every future model swap. Rejected: the model-swap cost recurs on every experiment (DPCRN, MossFormer2-S, …).
   - *Call stock `libnnsdk.so`* would need us to produce `.nb` files via VeriSilicon's Acuity Toolkit, which is under NDA and not available to us. Rejected: closed toolchain.

3. **Pipeline position: post-AEC, replace speex NS.** Beamform → AEC → **DFN3** → mono f32. DFN3 is single-channel (post-beam mono); it does not consume the reference signal or multi-mic input. Keeping NS as a post-AEC stage mirrors stock and matches DFN3's training data (AEC-clean noisy speech → clean speech).

4. **Pre/post framing: 10 ms hop at 48 kHz (480 samples).** DFN3's native hop; also matches waved's existing `ProcessedAudioFrame` size, so no resampling and no buffering gymnastics. Algorithmic latency is 20 ms (two hops lookahead) — documented as an end-to-end cost bump, acceptable for the conferencing use case (Teams / Zoom tolerate 150 ms easily).

5. **NS strength mapping.** The existing `NrLevel { Off, Low, Medium, High }` state selector stays. Off bypasses DFN3 (frame copied through). Low/Medium/High map to DFN3's post-filter wet/dry blend (0.3 / 0.6 / 1.0). The wet/dry mix is done in f32 after DFN3's output, not inside the model, so switching levels is a coefficient change with no graph rebuild.

6. **Model storage: bundled on-device at `/data/waved/models/deepfilternet3.onnx`.** Shipped by the deploy script, not compiled into the binary. ~8 MB int8 INT8. Version-gated via a 16-byte header check in the loader (`DFN3\0` + `u64` hash); load failures fall back to speex NS with a warn! and the existing NrLevel wiring unchanged.

7. **Fallback: speex NS stays compiled in.** If NPU init fails, if the model file is missing, or if VSI EP rejects an op, we emit a one-time error log and continue with speex NS. No runtime toggle is needed — this is crash-loop avoidance, not a user-visible mode. The existing `NoiseSuppressor` trait gains a second impl (`DeepFilterNs`) rather than being removed.

8. **Validation criteria (acceptance tests, ship with `scripts/test-neural-ns.sh`):**
   - **Latency**: DFN3 inference per 10 ms frame ≤ 5 ms wall-clock on the NPU path (50 % real-time headroom). Measured via in-process timestamps, 1000-frame average.
   - **Stationary noise attenuation**: ≥ 15 dB reduction at −20 dB SNR on pink noise, speech preserved within ±1 dB RMS during speech-only segments. Bar is set below the DNS-challenge numbers DFN3 achieves on desktop because int8 NPU quantisation will lose some headroom.
   - **Non-stationary noise**: keyboard / door-slam test clip attenuated ≥ 10 dB at the transient, no audible speech-onset wobble. Manual listening test, not automated.
   - **CPU delta**: waved process CPU under Cardioid+NS at silence ≤ current waved+speex-NS baseline + 2 %. NPU does the matmul; we only pay for FFT + pre/post framing on A55.
   - **Model fallback**: removing the ONNX file must produce a clean warn!, speex NS activates, no crash. Unit-tested host-side.

## 3. Scope

**In scope:**
- New Rust crate `waved-npu` wrapping our own ORT build + VSI-NPU EP via the existing `ort` crate (Rust binding to ONNX Runtime). Exposes a minimal trait: `NpuInfer::run(input: &[f32]) -> Vec<f32>` per frame.
- New submodule `waved-audio::ns::deep_filter` implementing `NoiseSuppressor` on top of `waved-npu`. Handles 10 ms framing (upstream already delivers this), STFT pre/post (reuse `rustfft` already in `waved-audio`), and DFN3's specific input packing.
- `NoiseSuppressor` trait left as-is (defined in `waved-audio/src/aec/mod.rs`). Speex impl stays; `DeepFilterNs` becomes a peer. `waved-audio::init_processing()` tries DFN3 first, falls back to speex on any `anyhow::Error`.
- Build system: `Cross.toml` gains a second target image with TIM-VX + ORT prerequisites for building `libonnxruntime.so.1.17+` against aarch64. Deploy script pushes the built .so and the DFN3 ONNX to `/data/waved/lib/` and `/data/waved/models/` respectively, sets `LD_LIBRARY_PATH` via the existing `setsid /data/waved/waved` wrapper.
- DFN3 ONNX export: documented in `scripts/export-dfn3-onnx.sh` — clone DeepFilterNet, run their upstream `export.py --int8`, smoke-test the resulting file with `waved-npu-probe` extended with graph-load.
- On-device acceptance script `scripts/test-neural-ns.sh` covering the four validation criteria above. Same shape as `test-beamforming.sh`: adb push binary + models, adjust runtime mode via `/tmp/waved-mode` + SIGHUP, measure from `/tmp/audio_stats.json`.
- `COMPUTE_HARDWARE.md` extended with the NPU integration path (what libraries, what paths, how to diagnose lock contention with `audioserver`).

**Out of scope (later SPs):**
- Model A/B between DFN3, DPCRN, MossFormer2-S — SP#4.5, only if DFN3 quality is marginal.
- Multi-channel NS consuming the 8-mic raw frames instead of the post-beam mono. DFN3 is single-channel and matches the existing pipeline exit point; multi-channel NS belongs to a different architectural conversation.
- Dereverberation tuning knob. DFN3 has a small dereverb head at fixed strength; exposing it to the UI is SP#5 territory.
- HiFi-class audio DSP (`/dev/audiodsp0`) for any part of the pipeline. The Cadence/Tensilica toolchain is vendor-locked; NPU + A55 will cover the SP#4 budget.
- Replacing AEC. SP#2's speex AEC stays unchanged.

## 4. Non-goals

- **We do not write a custom inference engine.** ORT + VSI-NPU EP is the maintained path; TIM-VX direct would be a fork of our own creation. If ORT's VSI EP is broken we re-scope SP#4, we don't hand-roll.
- **We do not patch stock firmware to stop using the NPU.** `audioserver` is already killed in our deploy flow (it competes for ALSA too, not just the NPU). If we find it holds an exclusive `/dev/galcore` lock at runtime we add a kill-on-startup to the waved init path — documented but not a whole-firmware restructure.
- **We do not ship float32 ONNX as the default.** The NPU's int8 path is the only one that hits the CPU-budget target. Float32 stays available as a `--fp32` build flag for host-side debugging / quality comparison.
- **We do not expose NPU to non-NS callers in SP#4.** `waved-npu` is a general enough crate that future consumers (e.g. on-device speaker VAD, wake-word) can reuse it, but SP#4 ships with DFN3 as the only user. Premature generalisation costs more than it saves at this stage.

## 5. Data model changes

### 5.1 `AudioConfig` v4

```rust
pub struct AudioConfig {
    pub version: u32,                         // bumped from 3 to 4
    pub capture: CaptureConfig,
    pub aec: AecConfig,
    pub beam: BeamConfig,                     // v3
    pub mic_geometry: MicGeometry,            // v3
    pub neural_ns: NeuralNsConfig,            // v4 — NEW
}

pub struct NeuralNsConfig {
    pub enabled: bool,                        // default true
    pub model_path: PathBuf,                  // default "/data/waved/models/deepfilternet3.onnx"
    pub fallback_to_speex: bool,              // default true
    pub latency_budget_ms: f32,               // default 5.0; log warn if per-frame > budget
}
```

Backwards-compatible via `serde(default)` on `neural_ns`, mirroring how v3 handled `mic_geometry`. Existing on-device `/etc/waved-audio-config.json` keeps working.

### 5.2 `AudioStats` (exposed via `/tmp/audio_stats.json`)

```rust
pub struct AudioStats {
    /* existing fields unchanged */
    pub ns_backend: &'static str,             // "deep_filter" | "speex" | "off"
    pub ns_frame_ms: Option<f32>,             // DFN3 inference wall-time, EWMA over 100 frames
}
```

`test-neural-ns.sh` reads `ns_backend` to assert the NPU path actually activated, and `ns_frame_ms` for the latency gate.

### 5.3 `NoiseSuppressor` trait (unchanged signature)

```rust
pub trait NoiseSuppressor: Send {
    fn process_frame(&mut self, mono: &mut [f32]);
    fn name(&self) -> &'static str;
}
```

Two impls: existing `SpeexNs` + new `DeepFilterNs`. No trait object indirection in the hot path after init — `waved-audio::init_processing()` stores one Box<dyn NoiseSuppressor> in `AudioService` the same way it does today.

## 6. Algorithms

### 6.1 DFN3 input packing

DFN3 expects a 10 ms mono f32 frame in the time domain, plus the last N frames of state as an LSTM hidden vector passed model-side. The ORT session owns the hidden state between calls; we just pass `mono: &[f32; 480]` in and read `enhanced: [f32; 480]` out.

Pre-filter: DC-block HPF at 60 Hz (single biquad) to match DFN3 training-data bandwidth. Post-filter: wet/dry mix per NrLevel (§2.5). No other DSP.

### 6.2 NPU graph load + session lifecycle

1. `DeepFilterNs::new(model_path)` — ORT `SessionBuilder` with `vsi_npu` provider first, CPU second in the provider list. If VSI EP fails to init (library load, device probe, op compatibility), ORT auto-falls back to CPU — logged at warn! but still works.
2. `process_frame()` — zero-copy where possible (ORT's `Value::from_array` borrows the input slice). Hidden state is model-internal; we never touch it.
3. Drop: `Session::drop` releases NPU resources. We do not keep hidden state across `AudioService` restarts — first frame after restart has a warm-up transient (~3 frames, inaudible in the 10 ms regime).

### 6.3 Fallback chain on startup

```
try DeepFilterNs::new(cfg.model_path)
  → OK → return Box::new(dfn)
  → Err(NoNpu | NoModel | OrtInit) 
    → warn!("neural NS unavailable: {}; falling back to speex", e);
    → if cfg.fallback_to_speex { SpeexNs::new() } else { return Err }
```

Single retry on `NoNpu` if `audioserver` is detected running — kill it, wait 2 s (matches the ALSA release window we already use), retry. Documented, not automatic in SP#4 step 1; enable if acceptance testing finds stock holds a lock.

## 7. Toolchain risk + build plan

The one non-trivial build problem is producing a working `libonnxruntime.so` with VSI-NPU EP that links against the on-device `libovxlib.so` 6.4.19.

Three known rough edges:
- **TIM-VX branch selection.** Matching the on-device galcore / libovxlib version matters (the VSI NN API has had ABI drift). Start with TIM-VX `main` pinned to a release that corresponds to galcore 6.4.19.x; if linkage fails we git-bisect.
- **ORT provider registration.** ORT's `--use_vsinpu` was experimental through 1.16; by 1.17 it's merged but may need minor patches for cross-compile. Fallback: build ORT 1.18 which has the cleanest VSI EP story as of the January 2026 cutoff.
- **aarch64 cross toolchain for ORT.** `cross` gives us a Debian-based aarch64 sysroot; ORT's build system (cmake) is happy with it. Expected build time: 25–40 min on a laptop, with incremental rebuilds in seconds.

The SP#4 plan starts with a contained spike (build TIM-VX alone, smoke-test against the on-device libs) **before** the full ORT build, so a toolchain dead-end is caught in hours rather than days.

## 8. Open questions

1. **Does stock `audioserver` hold `/dev/galcore` exclusively at runtime?** The NPU probe passed because we had already killed audioserver indirectly via earlier SP#3 watchdog kills. Re-probe with stock running before finalising §6.3 retry logic.
2. **Does int8 DFN3 clear the quality bar on this specific chip revision (0x9002)?** VSI NPUs sometimes lose >2 dB SNR going fp32 → int8 on recurrent layers. Benchmark on host fp32 and on-device int8, compare against the §2.8 gates. If int8 misses by > 1 dB, evaluate fp16 (`.nb` files in stock show fp16 is supported).
3. **Is there a quieter teardown of `libTEAProcessSdk.so`?** We're leaving Tencent's stuff in the stock rootfs untouched, but if at runtime it starts threads that touch the NPU while we're using it, we may need to `killall audioserver` unconditionally. Defer to acceptance testing.
4. **Does `/tmp/waved-mode`'s existing runtime-mode SIGHUP flow extend to an NS sub-mode?** The current flow swaps beamforming presets. Wiring DFN3 on/off through the same path keeps one mode-switch mechanism; alternative is a separate `/tmp/waved-nr` file + handler. Decide in plan.

## 9. Self-review

- §1–2 decisions are all grounded in already-measured evidence (`AUDIO_HARDWARE.md`, `COMPUTE_HARDWARE.md`, and the NPU probe's 2026-04-23 run). No floating assumptions.
- The one real risk — ORT+VSI EP cross-compile — is addressed by the spike-first sequencing in §7 and the fallback to speex NS in §2.7.
- Scope is narrower than SP#3's (single new consumer, single new model, one new crate). Should fit a ~8-task plan.
