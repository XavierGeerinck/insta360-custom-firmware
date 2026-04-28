# Waved — Roadmap

This repository replaces the stock Insta360 Wave firmware's closed-source audio stack with an open-source equivalent. The work is split into six sub-projects (SPs); each gets its own spec → plan → implementation cycle.

See `docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md` for the authoritative SP#1 design.

## Sub-projects

- [~] **SP#1 — Capture foundation + measurement harness** (v2 in progress)
- [x] **SP#2 — AEC + playback path** (speexdsp + ProcessedAudioFrame pipeline landed; on-device ERLE 24.3 dB single-tone / 22.7 dB speech-band — beats the 20 dB spec; see `scripts/test-aec-on-device.sh` and `scripts/test-aec-conferencing.sh`)
- [x] **SP#3 — Beamforming + DOA** (DAS + first-order differential cardioid/supercard/hypercard/figure-8/stereo, GCC-PHAT + pure-Z pair DOA; on-device acceptance passes all three gates — FBR ≥ 4 dB, DOA az error ≤ 10°, beam CPU delta 0.29% < 5%; see `scripts/test-beamforming.sh` and `docs/AUDIO_HARDWARE.md`)
- [~] **SP#4 — Neural NS + dereverb (DeepFilterNet 3)** — partial. Probing complete, one version-skew blocker remains:
  - ✅ NPU reachable from our process (`vxCreateContext`, `vsi_nn_CreateContext` — see `crates/waved-npu-probe/`)
  - ✅ TIM-VX cross-compiles + loads on device with `USE_EXTERNAL_OVXLIB=ON` delegating to device's libovxlib
  - ✅ TIM-VX host `CompileToBinary` produces valid VPMN NBG (but TIM-VX prebuilt host SDK targets simulator/GC7000, not our chip)
  - ✅ **Found public VIP9000-aware userspace SDK** at `github.com/eYs3D/eCV5546_rootfs` (versions 6.4.15.9 + 6.4.18.5; full archmodel + NN binary blobs + headers; aarch64). Companion `eYs3D/eCV5546_linux` has the matching galcore kernel driver source.
  - ✅ Confirmed our chip is `vip9000nano-di+` (ChipID 0x9000 / Rev 0x9002 / ProductID 0x5090009) from eYs3D's `gc_feature_database.h`.
  - ❌ Insta360's libOpenVX was built without the NN binary subsystem — adding `libNN*Binary.so` to device doesn't help (never dlopened).
  - ❌ Mixing 6.4.18.5 userspace with our device's 6.4.19 galcore kernel driver: `vsi_nn_AddTensor` fails (ioctl protocol mismatch); pure-userspace mix hangs at context init.
  - ❌ ORT 1.22 + VSI-NPU EP cross-compile — three upstream bugs; same archmodel issue underneath.
  - **Remaining blocker is version skew between Insta360's 6.4.19 galcore kernel driver and the public 6.4.18.5 userspace.** Three paths to unblock (see `docs/COMPUTE_HARDWARE.md`): replace galcore with 6.4.18.5 (source available), find a 6.4.19 userspace from another public BSP, or business ask to Insta360/Amlogic.
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
- DeepFilterNet 3 integration (post-beamforming single-channel f32) — blocked on a single version-skew issue (6.4.18.5 public userspace vs our device's 6.4.19 galcore). Both galcore source and userspace binaries are public on `github.com/eYs3D/eCV5546_*` repos.

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
