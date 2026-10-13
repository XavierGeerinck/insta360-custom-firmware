---
title: Testing
description: Host unit tests, the Linux container for the crates macOS can't build, UI snapshot tests, and the scripts that test on a Wave.
---

Run the commands on this page from `waved/` unless a step says otherwise.

## Unit tests on your computer

Unit tests run on your computer, without a Wave, one crate at a time:

```bash
cargo test -p waved-ui --lib
```

Swap in any crate name. The service crates test this way on macOS and Linux,
except `waved-meeting` and `waved` (see the next section).

`waved-audio` also has integration tests that replay recorded audio through
the real audio service:

```bash
cargo test -p waved-audio --features capture-replay
```

## Crates that need Linux

`waved-meeting` and the `waved` binary depend on `sqlx`. On macOS the linker
in Xcode 27 builds `sqlx`'s macro library in a form macOS then refuses to
load, so those two build and test in a Linux container.
`scripts/linux-test.sh` runs `cargo` in a Debian container with your
arguments:

```bash
scripts/linux-test.sh test -p waved-meeting --lib
scripts/linux-test.sh check -p waved --features ui,capture-alsa
```

It needs Docker (or OrbStack) running and some free space in its VM. The Rust
toolchain and the cargo registry stay in Docker volumes between runs, and the
build goes to `waved/target-linux/`. On a Linux machine you can run the same
`cargo` commands directly.

The `waved` crate's integration tests (`crates/waved/tests/`) start the
services on simulated hardware, with the `simulate` feature:

```bash
scripts/linux-test.sh test -p waved --features simulate
```

## UI snapshot tests

Each screen in `waved-ui` has a test that renders it with LVGL on your
computer. The test checks that every tappable object lies inside the visible
circle and is big enough to hit, then compares the frame with a PNG in
`waved/crates/waved-ui/golden/`. They run with the crate's unit tests:

```bash
cargo test -p waved-ui --lib
```

A frame that differs fails the test and is written to
`waved/target/golden-diff/<name>.actual.png`. When you change a screen on
purpose, write new goldens and look at them:

```bash
UPDATE_GOLDEN=1 cargo test -p waved-ui --lib
```

Compare them with the design mockups in `docs/mockups/png/`. With
`UI_PREVIEW=1` the tests also write copies dimmed outside the visible circle
to `waved/target/ui-preview/`, which are easier to compare.

The screenshots on this site are copies of these goldens, made by the site's
sync step (the `screens` list in `site/sync/manifest.mjs`).

## The install and revert scripts

```bash
deploy/tests/test-deploy-scripts.sh
```

It runs `deploy/install.sh` and `deploy/revert.sh` against a fake `adb` that
logs every call, so no Wave is needed. It covers the order of the steps, a
copy that doesn't match, a full `/data`, a missing Wave and the ADB-at-boot
check.

## The site

From the repository root:

```bash
cd site
npm test
npm run build
```

`npm test` tests the sync scripts. `npm run build` syncs the docs and builds
the site; it fails on an internal link or anchor that doesn't exist.

## On a Wave

These need a Wave on ADB, usually running waved, and most need a build. Read a
script's header before you run it: some stop waved or its start-up script,
some need extra hardware such as a speaker at a known angle or a Mac on USB,
and some prompt you as they go.

`scripts/smoke.sh` is a quick health check: waved is running, its memory use,
and `config.toml` and `meetings.db` exist. Run it without `--deploy`. That
option pushes the binary straight over `/data/waved/waved`, which can leave
the Wave without one ([Display, Deploy](/insta360-custom-firmware/reference/display-hardware/#deploy)).

The `test-*.sh` scripts in `waved/scripts/` are acceptance tests, one per
feature:

| Script | Checks |
|---|---|
| `test-capture-on-device.sh` | Microphone capture |
| `test-aec-on-device.sh` | Echo cancellation, with a test tone |
| `test-aec-conferencing.sh` | Echo cancellation with both sides talking; needs a speech recording (`SPEECH_FIXTURE`) |
| `test-beamforming.sh` | Beam front-to-back ratio, direction accuracy and CPU |
| `test-doa-improvements.sh`, `test-doa-elevation.sh` | Direction of arrival |
| `test-diarization.sh` | Speaker numbers in a real recording |
| `test-mic-volume.sh` | The microphone gain, end to end |
| `test-playback-eq.sh` | The speaker's equaliser and gain |
| `test-uac2-routing.sh`, `test-uac2-routing-host.sh` | USB audio both ways (the second runs on the Mac) |
| `test-hid-control.sh`, `test-hid-control-host.sh` | The USB HID controls (the second runs on the Mac) |
| `test-neural-ns.sh` | Noise reduction on the CPU |
| `test-latency-benchmark.sh` | Processing latency when speech starts after silence |
| `test-recording-resilience.sh` | A long recording, storage pruning, and reboots during a recording. It reboots the Wave three times (`ALLOW_DEVICE_REBOOT=yes`) and needs builds with `recording-integrity-test-hooks`; `DURATION_SECONDS=3600` is the one-hour run |
| `test-recording-integrity.sh` | Recording when the audio pipeline fails; needs a build with `recording-integrity-test-hooks` |
| `test-wifi-provisioning.sh` | Wi-Fi setup end to end |
| `tests/test-patch-usbgadget-on-device.sh` | The USB gadget patch, on a copy in `/tmp` |

`waved-device-test` and `scripts/hil.sh` are older. `waved-device-test` runs
on the Wave and checks the serial port, the HID device, a Wi-Fi scan, the
battery, ALSA, the file system, `config.toml` and the recordings folder. Its
serial and battery checks still talk to `/dev/ttyS1` the old way, but the MCU
is on `/dev/ttyS4` ([MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/#which-uart-is-which)),
so don't read those two results as MCU health. `hil.sh` copies `/data/waved`
to your computer, pushes waved and `waved-device-test` straight into it, runs
`smoke.sh` and `waved-device-test`, then pushes the old waved back. Those
direct pushes carry the same risk as `smoke.sh --deploy`.
