# Waved — Roadmap

This repository replaces the stock Insta360 Wave firmware's closed-source audio stack with an open-source equivalent. The work is split into six sub-projects (SPs); each gets its own spec → plan → implementation cycle.

See `docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md` for the authoritative SP#1 design.

## Sub-projects

- [~] **SP#1 — Capture foundation + measurement harness** (v2 in progress)
- [x] **SP#2 — AEC + playback path** (speexdsp + ProcessedAudioFrame pipeline landed; on-device ERLE 24.3 dB single-tone / 22.7 dB speech-band — beats the 20 dB spec; see `scripts/test-aec-on-device.sh` and `scripts/test-aec-conferencing.sh`)
- [ ] SP#3 — Beamforming (MVDR/DAS, 6 modes + DOA)
- [ ] SP#4 — Neural NS + dereverb (DeepFilterNet 3)
- [ ] SP#5 — Playback EQ + gain staging
- [ ] SP#6 — UAC2 routing for Wave Controller

ASR (Silero VAD, WeNet, SenseVoice) is already open-source and out of scope.

## SP#1 carry-overs for later SPs

Items intentionally deferred during SP#1 v2 because they belong to their owning sub-project:

### SP#3 (Beamforming)
- Add `mic_geometry` field to `AudioConfig` under a version bump.
- Wire `Command::SetAudioMode` (Omni / Cardioid / Supercardioid / Figure8 / Stereo) to MVDR/DAS beamforming presets.
- DOA estimation consumer for on-UI directionality indicator.

### SP#4 (Neural NS + dereverb)
- DeepFilterNet 3 integration (post-beamforming single-channel f32).

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
