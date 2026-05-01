# Waved — Roadmap

This repository replaces the stock Insta360 Wave firmware's closed-source audio stack with an open-source equivalent. The work is split into six sub-projects (SPs); each gets its own spec → plan → implementation cycle.

See `docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md` for the authoritative SP#1 design.

## Sub-projects

- [~] **SP#1 — Capture foundation + measurement harness** (v2 in progress)
- [x] **SP#2 — AEC + playback path** (speexdsp + ProcessedAudioFrame pipeline landed; on-device ERLE 24.3 dB single-tone / 22.7 dB speech-band — beats the 20 dB spec; see `scripts/test-aec-on-device.sh` and `scripts/test-aec-conferencing.sh`)
- [x] **SP#3 — Beamforming + DOA** (DAS + first-order differential cardioid/supercard/hypercard/figure-8/stereo, GCC-PHAT + pure-Z pair DOA; on-device acceptance passes all three gates — FBR ≥ 4 dB, DOA az error ≤ 10°, beam CPU delta 0.29% < 5%; see `scripts/test-beamforming.sh` and `docs/AUDIO_HARDWARE.md`)
- [x] **SP#4 — Neural NS + dereverb (DeepFilterNet 3, CPU)** — DFN3 running on Cortex-A55 via `tract-NEON`. Acceptance: init clean, process alive ≥ 8 s under load, CPU delta well under the 5 % budget (sampled noise; DFN3 cost ~0–2 %). NPU path was deliberately abandoned after a full toolchain survey — see "SP#4 NPU history" below for what we learned.
  - Crate: `crates/waved-npu/` exposes `NpuInfer` trait + `Dfn3CpuInfer` (bundled DFN3 low-latency model via `default-model-ll` feature).
  - Pipeline: `crates/waved-audio/src/aec/deep_filter.rs` implements `NoiseSuppressor` over `Dfn3CpuInfer`. Internally hop-buffers 1024-sample ALSA periods into 480-sample DFN3 hops. Init failure falls back to `SpeexNoiseSuppressor`.
  - Driving: SIGHUP + `/tmp/waved-nr` (off | low | med | high) — see `crates/waved/src/main.rs::mode_switch_handler`.
  - Acceptance: `scripts/test-neural-ns.sh` (init engages, process alive, CPU delta ≤ 5 %).
  - Future-NPU: the `NpuInfer` trait stays — when/if a usable VIP9000 toolchain lands (see `docs/COMPUTE_HARDWARE.md`), drop in a `Dfn3NpuInfer` impl, no caller changes.

### SP#4 NPU history (closed; preserved for context)

The NPU path was probed exhaustively before pivoting:

  - ✅ NPU reachable from our process (`vxCreateContext`, `vsi_nn_CreateContext` — `crates/waved-npu-probe/`).
  - ✅ TIM-VX cross-compiles + loads on device with `USE_EXTERNAL_OVXLIB=ON`.
  - ✅ TIM-VX host `CompileToBinary` produces valid VPMN NBG (but prebuilt host SDK archmodel targets GC7000, not our chip).
  - ✅ Public 6.4.18.5 SDK located at `github.com/eYs3D/eCV5546_*`; chip identified as `vip9000nano-di+` (ChipID 0x9000 / Rev 0x9002 / ProductID 0x5090009).
  - ✅ `libnnsdk.so` reverse complete: ~30-symbol clean C API (`NNsdk_module_create/run_graph/...`), `dlopen`s the stack at runtime — would be trivial to FFI-bind from Rust if we had a DFN3 `.nb`.
  - ✅ Khadas redistributable Acuity Toolkit found (GPL); ships VIPNANOQI target descriptors but not VIPNANODI+ (our variant).
  - ❌ Insta360's `libOpenVX` was built without the NN binary subsystem — `libNN*Binary.so` is never `dlopen`ed.
  - ❌ 6.4.18.5 userspace × 6.4.19 galcore: `vsi_nn_AddTensor` ioctl mismatch; pure-userspace mix hangs at context init.
  - ❌ ORT 1.22 + VSI-NPU EP cross-compile: three upstream bugs.
  - ❌ Replacing galcore with 6.4.18.5: feasible but requires sourcing Insta360's GPL kernel + porting an Amlogic platform file into eYs3D's source — weeks of work, not days.

  **Net**: producing a `.nb` that runs on this exact chip without a VeriSilicon Acuity license costs weeks; running DFN3 ONNX on CPU costs days and the CPU has the headroom. The NPU stack stays a future optimization. Full path-status matrix in `docs/COMPUTE_HARDWARE.md`.
- [x] **SP#5 — Playback EQ + gain staging** (HPF + 4-biquad EQ + 4-band MBAGC + lookahead limiter inside `PlaybackService`; voice preset matches stock `dynamic_eq_config_2.json` byte-for-byte; media preset bypasses MBAGC and flattens EQ; AEC reference tap moved to post-DSP — ERLE 23.2 dB on device, beats SP#2 baseline 22.7 dB; preset switch via `/tmp/waved-playback-preset` + SIGHUP. See `scripts/test-playback-eq.sh`, `scripts/test-aec-on-device.sh`, `docs/AUDIO_HARDWARE.md` Playback DSP Chain section.)
- [x] **SP#6 — UAC2 routing for Wave Controller** (`crates/waved-audio/src/uac/`: `UacBridge` reads `hw:1,0c` → `play_batch` (host audio gets the SP#5 DSP chain + AEC reference for free), `UacUpstreamSink` reads `subscribe_processed()` → `hw:1,0p` (post-AEC mono → stereo dup), `UacControl` mirrors `hw:1` mixer events ↔ `AudioState`. ALSA-mixer-based volume + mute is now end-to-end through one mechanism — `speaker_mixer::amixer` shell-out gone. **AEC ERLE with host-played music: median 26.6 dB** (bar ≥ 20). See `scripts/test-uac2-routing.sh`, `docs/AUDIO_HARDWARE.md` UAC2 Routing section.)

ASR (Silero VAD, WeNet, SenseVoice) is already open-source and out of scope.

## SP#1 carry-overs for later SPs

Items intentionally deferred during SP#1 v2 because they belong to their owning sub-project:

### SP#3 (Beamforming) — all done
- [x] `mic_geometry` in `AudioConfig` v3 (Task 8).
- [x] `Command::SetAudioMode` wired to beamforming presets (Task 11).
- [x] DOA consumer exposed via `doa_horizontal` in `/tmp/audio_stats.json` (Task 11 / stats_collector).

### SP#3 known residuals (not blockers, not scheduled)
- Elevation bias +3° to +18° after pure-Z pair swap — may be partly real speaker-above-ring. Investigate only if a downstream feature needs elevation (current consumer is 2D arrow).
- Azimuth error at 270° is ~19° post-pair-swap (vs ~7° before). Candidate fix: per-pair GCC-PHAT peak-sharpness confidence weighting in the LS solve.
- Raw mic signal at ~−75 dBFS sits close to the VAD floor (0.0003). Preamp / gain staging belongs in SP#5.

### SP#4 (Neural NS + dereverb) — done
- DeepFilterNet 3 v0.5.6 integrated on CPU via `tract`; bundled low-latency model. See `crates/waved-npu/`, `crates/waved-audio/src/aec/deep_filter.rs`, `scripts/test-neural-ns.sh`.

### SP#4 follow-ups (not blockers)
- Tract session creation takes ~1.5 s; engaging NR live drops ~50 frames during the rebuild. Pre-build DFN3 at startup (regardless of initial NR level) and only flip the active backend on `SetNrLevel` to eliminate the warm-up gap.
- Speech-loaded CPU profile: current measurement was a quiet room (DFN3's lsnr gating skipped decoder layers most of the time). Re-measure with the AEC-conferencing playback running to see actual sustained cost.
- `ns_backend` + `ns_frame_ms` stats fields (per spec §5.2) — useful for ongoing observability; deferred until we have a stats consumer that needs them.
- NR strength wiring: NrLevel::Low/Med/High currently all engage the same DFN3 atten settings. Map to `RuntimeParams::with_atten_lim` per the SP#4 spec §2.5 wet/dry idea.

### SP#5 follow-ups (not blockers)
- **MBAGC `knee_db` is dead.** `MbagcParams.knee_db` is populated from stock JSON and round-trips through config, but `band_gain_linear` ignores it (hard clamp `raw.min(max_gain).max(0.0)`). Voice preset therefore diverges from stock at the gain-curve knee. Implement a soft knee (e.g., quadratic transition over `knee_db` dB approaching the target from below) so the stock numbers actually behave per stock.
- **Preset switch cosine ramp not implemented.** Spec §2.8 calls for a 5 ms cosine envelope on switch to mask coefficient discontinuity. Hard swap currently lands directly at the period boundary. Manual listening on `scripts/test-playback-eq.sh` test 4 didn't surface a click, but the risk is real if presets ever differ more aggressively. Add `pending_tuning` + N-frame gain envelope in `PlaybackDspChain::set_preset`.
- **`limiter_reduction_db` snapshot is last-block, not peak-hold.** Acceptance test 3 reads after playback finishes → silence → `reduction_db = 0.0`. Add peak-hold-with-decay (window matching the 500 ms stats cadence) so the field reflects observed limiter activity over the recent past.
- **`scripts/cpu_delta.py` baseline (11.5%) is pre-SP#4.** With DFN3 enabled by default, the post-SP#4 baseline at silence is ~43%. Re-baseline once SP#4's pre-build-at-startup follow-up lands; until then test 5 will fire on outdated numbers.
- **`AudioConfig::validate()` doesn't shape-check the EQ array.** Spec §5.1 lists `eq.len() == 2` and `eq[0].len() == 4` checks; only HPF/limiter/crossover bounds are enforced. The Rust type system catches it for the embedded preset (fixed-size array), but a malformed JSON gets a serde error, not a `ConfigError::Invalid` with a readable message.
- **No unit-level test for mid-playback preset switch.** The acceptance script's manual test 4 is the only coverage. Add a `playback.rs::tests` case that sends a preset on `preset_tx` after N batches and asserts MBAGC envelope state was reset.
- **Replace `speaker_mixer::amixer` shell-outs with a proper playback pipeline.** Originally listed pre-implementation; descoped per the SP#5 spec §4 non-goals (the amixer path manages the unused TDMOUT_A; cleanup belongs alongside SP#6 once the new pipeline owns volume control end-to-end).

### SP#6 follow-ups (not blockers)
- **No `/tmp/waved-volume` SIGHUP source.** The acceptance script test 4 falls back to writing the UAC2 mixer directly (`amixer -c 1 sset PCM 75%`), which exercises the host→device path but not the device→host mirror. Add a `/tmp/waved-volume` reader to `mode_switch_handler` (mirror the `/tmp/waved-playback-preset` pattern) so the touch-UI volume path can be acceptance-tested without UI hardware.
- **Wave Controller protocol probe.** Spec §8 open question: stock firmware likely handles Wave Controller-specific UAC2 control surface (sidetone, mic gain table, telephony class units). Standard UAC2 currently works for generic apps (Apple Music, Zoom, etc.) — but Wave Controller integration may need vendor descriptors. Diagnose with `usbmon` + Wireshark vs stock firmware *if* a host-side test reveals incompatibility.
- **Multichannel UAC2 upstream (raw 8-ch mics).** Future SP#6.5 if a host-side beamforming app wants the raw array. Requires kernel `c_chmask=0xff` configfs change and a separate UAC2 vendor-specific interface descriptor.
- **Upstream EQ / gain.** Stock firmware had `Insta360_UplinkAsr*` paths suggesting upstream signal shaping (probably mic-FR compensation). Defer until a host-side listening test reveals a need; one biquad in the upstream sink would suffice.

## SP#1 v1 → v2 minor items (follow-ups, not blocking)

- `_frame_tx` in `AudioService` keeps the frame channel open even if the capture thread dies silently — add a health watch once SP#2 lands (context exists there to react).
- `stats_collector` does not actively reap stale `/tmp/audio_stats.json.tmp` on rename failure — self-corrects on next write; low priority.
- ALSA device / sample-rate / period values are fully in `AudioConfig`; no follow-up required.
- Re-verify `snd_pcm_link` kernel alignment using a known playback signal; if drift > 1 period, force timestamp-fallback path via config. (Deferred: software-reference path is the default — only matters if/when hardware-loopback is re-enabled.)
