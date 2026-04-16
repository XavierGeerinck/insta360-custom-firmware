# waved-audio

Audio capture + state bridge for waved. See the design spec:
`docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-design.md`

## Features

- `capture-alsa` — real 10-ch capture from `pcmC0D0c` (on-device only, requires libasound)
- `capture-replay` — file-backed replay for host-side testing / DSP iteration
- `simulate` — synthetic silence generator (mutually exclusive with `capture-alsa`)

Features are mutually exclusive at compile time where incompatible.

## Common commands

```bash
# Host unit tests
cargo test -p waved-audio

# Host integration tests (replay)
cargo test -p waved-audio --features capture-replay

# Cross-build for device
cross build --target aarch64-unknown-linux-gnu -p waved-audio --features capture-alsa

# Offline metrics on a captured file
cargo run -p waved-audio-bench -- my-capture.pcm --format i16_8

# On-device smoke test
scripts/test-capture-on-device.sh
```

## Next sub-projects

See spec §1.2 for the six-sub-project decomposition. This crate is the
foundation — AEC (SP #2), beamforming (SP #3), neural NS (SP #4), playback
EQ (SP #5), UAC2 routing (SP #6) plug in via the `PcmSource` trait or
after the `apply_mic_mute` / `compute_vu` stage.
