# waved-audio

Audio capture + state bridge for waved. See the v2 design spec:
`docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-v2-design.md`

## Features

- `capture-alsa` — real ALSA capture from `hw:0,2` (8 mics) + `hw:0,3` (2-ch reference). Device only.
- `capture-replay` — file-backed replay for host-side testing / DSP iteration.
- `simulate` — synthetic silence generator (mutually exclusive with `capture-alsa`).

## Configuration

Runtime config at `/etc/waved-audio-config.json` (version 2). Falls back to
the embedded default (mics `hw:0,2` @ 8ch S32_LE 48000, refs `hw:0,3` @
2ch S32_LE 48000, period 256, 4 buffers).

## Common commands

```bash
# Host unit tests
cargo test -p waved-audio

# Host integration tests (replay)
cargo test -p waved-audio --features capture-replay

# Cross-build for device
cross build --target aarch64-unknown-linux-gnu -p waved-audio --features capture-alsa

# Offline metrics on a captured file (8-ch i16)
cargo run -p waved-audio-bench -- my-capture.pcm --format i16_8

# On-device smoke test (full Tier-3 gate)
scripts/test-capture-on-device.sh
```

## Capture data flow

```
 hw:0,2 (8ch mics)        hw:0,3 (2ch reference)
        │                          │
        ▼                          ▼
  AlsaPcmSource               AlsaPcmSource
         \                      /
          AlsaDualStreamSource     (snd_pcm_link or timestamp-fallback)
                  │
                  ▼
           run_capture_loop        (backpressure drop-newest, xrun recovery)
                  │
                  ▼   AudioFrame { data, reference, ts_ms, ... }
                  ├──► stats_collector  → /tmp/audio_stats.json
                  ├──► AudioService     → VU state, meeting channel
                  └──► AudioService fan-out broadcast tee
```

## Next sub-projects

See `TODO.md` at repo root and the v2 spec §9. SP#2 (AEC) consumes
`AudioFrame.reference` and flips to `Vec<f32>` at its DSP boundary. SP#3–6
plug in via further extensions of `AudioConfig`.
