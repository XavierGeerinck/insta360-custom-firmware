# waved — Insta360 Wave firmware replacement

This repository is an open-source firmware replacement for the Insta360 Wave.
Target device: ARM64 Linux (custom PSP userspace) with circular LCD, 8-mic
array, touch input, Bluetooth and Wi-Fi.

## Device Hardware References

Keep these docs authoritative for anything device-specific — values here come
from on-device measurement and are hard to rederive.

| Area              | Doc                                   |
|-------------------|---------------------------------------|
| Circular LCD      | [`docs/DISPLAY_HARDWARE.md`](docs/DISPLAY_HARDWARE.md) |
| 8-mic array, DOA  | [`docs/AUDIO_HARDWARE.md`](docs/AUDIO_HARDWARE.md)     |
| Compute (NPU/DSP) | [`docs/COMPUTE_HARDWARE.md`](docs/COMPUTE_HARDWARE.md) |
| UI kit (current)  | [`crates/waved-ui/src/kit/README.md`](crates/waved-ui/src/kit/README.md) |
| UI coordinates    | [`docs/ui/COORDINATE_SYSTEM.md`](docs/ui/COORDINATE_SYSTEM.md) |
| Stock UI assets / home screen | [`docs/ui/ASSETS.md`](docs/ui/ASSETS.md), [`docs/ui/HOME_SCREEN.md`](docs/ui/HOME_SCREEN.md) (reference only) |
| Wi-Fi / provisioning | [`docs/WIFI.md`](docs/WIFI.md)                      |
| System services (power key, backlight, RTC, tones, reset) | [`crates/waved-system/README.md`](crates/waved-system/README.md) |
| MCU link (`/dev/ttyS4`, MAVLink: battery, motor, power key, power off) | [`docs/MCU_PROTOCOL.md`](docs/MCU_PROTOCOL.md) |
| Bluetooth module (`/dev/ttyS1` 921600, Telink; audio on TDM-A `hw:0,0`; `BT.bin`/`DONGLE.bin`) | [`docs/BLUETOOTH_PROTOCOL.md`](docs/BLUETOOTH_PROTOCOL.md) |

When you learn something new about the device that is **not derivable from
reading the code** (measured offsets, magic numbers, hardware quirks,
reverse-engineered protocols, init.d watchdog behaviour, …) add it to the
relevant doc. Do not rely on commit messages — they rot.

## Build & Deploy

```bash
WAVED_BUILD_ID=$(git rev-parse --short HEAD) cross build --release --target aarch64-unknown-linux-gnu \
    --features "ui,capture-alsa" -p waved
```

Then deploy through `/tmp` with the watchdog stopped: `/data` is nearly full and a
direct `adb push` over the binary can leave the device with none. The full
procedure (strip, md5 check, swap, watchdog restart) is in
[`docs/DISPLAY_HARDWARE.md`](docs/DISPLAY_HARDWARE.md#deploy).

For one-off runs with injected env vars, stop the watchdog first and start waved
yourself:
```bash
adb shell "killall waved 2>/dev/null; sleep 2; \
           (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```

`setsid` is required — without it, `adb shell` sends SIGHUP to waved on
session close.

`WAVED_BUILD_ID` is the build id the About screen shows (`Cross.toml` passes it
into the container). The power key, the battery, the lift and power off go
through the MCU on `/dev/ttyS4` (`waved-mcu`, `docs/MCU_PROTOCOL.md`); waved
owns that port, so stop it before using `mcutool`. `WAVED_POWER_KEY=1` finds no
input-device key on the Wave (`vad_keypad` is the speech-wake detector).

`/etc/init.d/S60waved` auto-respawns waved within ~3 s of any exit. When
testing with injected env vars, kill the watchdog first (see
`docs/AUDIO_HARDWARE.md`).

## Testing Conventions

- Unit tests run in `cargo test -p <crate> --lib`.
- `waved-meeting` and the `waved` binary depend on sqlx, whose proc-macro dylib
  macOS (Xcode 27 linker) refuses to load. Build and test them in Linux:
  `scripts/linux-test.sh test -p waved-meeting --lib`,
  `scripts/linux-test.sh check -p waved --features ui,capture-alsa`.
  Every other crate tests on the host with `cargo test -p <crate> --lib`.
- On-device acceptance: scripts in `scripts/` (e.g. `test-beamforming.sh`,
  `test-audio-smoke.sh`).
- Stats are written to `/tmp/audio_stats.json` every 500 ms;
  `/tmp/waved-mode` + `SIGHUP` switches runtime audio mode.

## Project Layout

- `crates/waved/` — main binary, service orchestration
- `crates/waved-audio/` — capture, AEC, beamforming, DOA
- `crates/waved-ui/` — LVGL UI
- `crates/waved-mic-map/` — tap harness for ALSA→physical mic mapping

Each crate has its own README where relevant.
