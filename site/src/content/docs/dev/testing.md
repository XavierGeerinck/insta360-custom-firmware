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

It runs `deploy/install.sh` and `deploy/revert.sh` against a fake `adb`, so
no Wave is needed, and it aborts if `adb` is not the fake. The fake logs every
call and runs the scripts meant for the Wave in a directory that stands in
for the Wave's `/data`, `/tmp`, `/etc`, `/usr` and `/mnt`, with fake `ps`,
`kill`, `killall`, `pidof`, `start-stop-daemon` and `df`. Like the Wave's
overlay, it copies whatever a script changes under `/etc` and `/usr` into
`/data/overlay/upper`, and it flags a script that writes there directly. The
USB gadget patch it runs is the real one, on an `S89usbgadget`-shaped copy.
It covers:

- a first install: every file through `/tmp` with an md5 check, the gadget
  patch and the ADB-at-boot check before anything is stopped, the swap,
  `S60waved`, `S03splash`, `rsyslog.conf`, `splash.fb` and `iptables.rules`
  each through a dot-named copy and a rename, the default `config.toml`, both
  stubs written through the `/etc` and `/usr` mounts (never into
  `/data/overlay/upper` directly), read back and found in the upper
  directory, last, and `adb reboot` printed only at the end;
- an update: `config.toml` kept and copied to `config.toml.before-<time>`,
  the gadget found already patched, the old binary in `/tmp/waved.old`, the
  stubs already in the upper directory kept untouched (a compiled 8352-byte
  `libapplogrs.so`, or an older installer's one-byte one), the watchdog
  started again at the end, and the old binary's space counted as free;
- refusals before anything changes: too little room on `/data` (before any
  push), a copy that doesn't match, a failed push, an over-the-air update on
  trial, no Wave, no build, a Wave that stops answering;
- no ADB at boot, no further: a failed patch, a patch that leaves `adbd_enable`
  commented out, a boot function without `adbd_enable`, a check without an
  answer. None of them stops or stubs anything;
- failures once waved is stopped: waved that won't exit (no swap), a binary
  that doesn't fit (the old one back), a failed install step (no stubs),
  stubs that don't read back as written or never reach the upper directory,
  each with the watchdog started again after an update or "adb reboot" after
  a first install;
- Ctrl-C during a step: the message names the step, and an update gets its
  watchdog back;
- `revert.sh`: what it deletes, only in the overlay's upper directory.

`VERBOSE=1` prints the installer's output and the `adb` log of a failed
check. `cargo test -p waved-update --lib` runs a shorter host test of
`install.sh` (`crates/waved-update/src/install_script.rs`) with real processes
standing in for the watchdog and waved.

The USB gadget patch the installer runs on the Wave has its own test. It
patches copies of an `S89usbgadget`-shaped script under busybox, the shell the
Wave uses, and checks ADB at boot, the volume ranges, the HID controls, that a
second run changes nothing, and that an older USB-storage boot step is removed:

```bash
docker run --rm --network none -v "$PWD/scripts:/w:ro" busybox:1.36 sh /w/test-patch-stock-usbgadget.sh
```

Set `STOCK_S89` to a stock rootfs copy of the script, mounted into the
container, to patch that too.

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

`waved-device-test` and `scripts/hil.sh` are older. `waved-device-test` runs
on the Wave and checks the serial port, the HID device, a Wi-Fi scan, the
battery, ALSA, the file system, `config.toml` and the recordings folder. Its
serial and battery checks still talk to `/dev/ttyS1` the old way, but the MCU
is on `/dev/ttyS4` ([MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/#which-uart-is-which)),
so don't read those two results as MCU health. `hil.sh` copies `/data/waved`
to your computer, pushes waved and `waved-device-test` straight into it, runs
`smoke.sh` and `waved-device-test`, then pushes the old waved back. Those
direct pushes carry the same risk as `smoke.sh --deploy`.
