# Waved — Roadmap

This repository replaces the stock Insta360 Wave firmware's closed-source audio stack with an open-source equivalent. The work is split into six sub-projects (SPs); each gets its own spec → plan → implementation cycle.

See `docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md` for the authoritative SP#1 design.

## Sub-projects

- [~] **SP#1 — Capture foundation + measurement harness** (v2 in progress)
- [x] **SP#2 — AEC + playback path** (speexdsp + ProcessedAudioFrame pipeline landed; on-device ERLE 24.3 dB single-tone / 22.7 dB speech-band — beats the 20 dB spec; see `scripts/test-aec-on-device.sh` and `scripts/test-aec-conferencing.sh`)
- [x] **SP#3 — Beamforming + DOA** (DAS + first-order differential cardioid/supercard/hypercard/figure-8/stereo, GCC-PHAT + pure-Z pair DOA; on-device acceptance passes all three gates — FBR ≥ 4 dB, DOA az error ≤ 10°, beam CPU delta 0.29% < 5%; see `scripts/test-beamforming.sh` and `docs/AUDIO_HARDWARE.md`)
- [~] **SP#4 — Neural NS + dereverb (DeepFilterNet 3)** — partial. Probing complete, blocked on NPU toolchain:
  - ✅ NPU reachable from our process (`vxCreateContext`, `vsi_nn_CreateContext` — see `crates/waved-npu-probe/`)
  - ✅ TIM-VX cross-compiles + loads on device with `USE_EXTERNAL_OVXLIB=ON` delegating to device's libovxlib
  - ✅ TIM-VX host `CompileToBinary` produces valid VPMN NBG
  - ❌ Host prebuilt SDK's archmodel is GC7000-class + CPU simulator; device NPU is VIP9000Nano-S — compiled NBG targets wrong silicon, device-side `ops::NBG` fails at instantiation
  - ❌ ORT 1.22 + VSI-NPU EP cross-compile — three upstream bugs; same archmodel issue underneath
  - ❌ Device-side JIT compile: blocked. Device has libVSC/libOpenVX but zero shader source files (`.vx`, `.cl`) — stripped from image since stock only runs pre-compiled NBGs.
  - Unblocking is external (Acuity Toolkit / Amlogic BSP / Insta360 handoff).
  - See `docs/COMPUTE_HARDWARE.md` for the full path-status matrix.
- [ ] SP#5 — Playback EQ + gain staging
- [ ] SP#6 — UAC2 routing for Wave Controller

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

### SP#4 (Neural NS + dereverb)
- DeepFilterNet 3 integration (post-beamforming single-channel f32) — blocked on NPU toolchain access (VIP9000 archmodel not in any open SDK). Device-side JIT still an open probe.

### SP#5 (Playback EQ + gain staging)
- Replace `speaker_mixer::amixer` shell-outs with a proper playback pipeline.
- Add `playback` block to `AudioConfig` under a version bump.

### SP#6 (UAC2 routing)
- Expose Wave Controller audio via USB Audio Class 2 gadget.
- Out of scope for the first five SPs.

## SP#1 v1 → v2 minor items (follow-ups, not blocking)

- `_frame_tx` in `AudioService` keeps the frame channel open even if the capture thread dies silently — add a health watch once SP#2 lands (context exists there to react).
- `stats_collector` does not actively reap stale `/tmp/audio_stats.json.tmp` on rename failure — self-corrects on next write; low priority.
- ALSA device / sample-rate / period values are fully in `AudioConfig`; no follow-up required.
- Re-verify `snd_pcm_link` kernel alignment using a known playback signal; if drift > 1 period, force timestamp-fallback path via config. (Deferred: software-reference path is the default — only matters if/when hardware-loopback is re-enabled.)
