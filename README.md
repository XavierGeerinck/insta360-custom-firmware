# Wave Open Firmware

Open firmware for the **Insta360 Wave** speakerphone. `waved`, one Rust
program, replaces the stock software: no telemetry, recordings that stay
yours, and one script back to stock.

**Documentation: https://xaviergeerinck.github.io/insta360-custom-firmware/**

- [Install](https://xaviergeerinck.github.io/insta360-custom-firmware/start/install/)
- [What works today](https://xaviergeerinck.github.io/insta360-custom-firmware/start/overview/)
- [How it works](https://xaviergeerinck.github.io/insta360-custom-firmware/how/architecture/)
- [Reverse-engineering research](https://xaviergeerinck.github.io/insta360-custom-firmware/research/report/)

## Why

The stock firmware sends more than 35 kinds of usage events to ByteDance's
DataFinder service and routes meeting audio and transcripts to Insta360's
cloud ([Security Q&A](docs/QUESTIONS_SECURITY.md)). It signs its updates with
a 512-bit RSA key
([reverse-engineering report](docs/REPORT.md#soc-firmware-update-flow)).

## Quick start

```bash
pip3 install pyusb                             # once, see Requirements
sudo python3 waved/scripts/enable_adb.py       # ADB, until the next reboot
cd waved
WAVED_BUILD_ID=$(git rev-parse --short HEAD) \
  cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
./deploy/install.sh && adb reboot
```

Requirements and every step explained: [Install](https://xaviergeerinck.github.io/insta360-custom-firmware/start/install/).
Back to stock, from `waved/`: `./deploy/revert.sh && adb reboot`.

## Repository

| Path | What |
|---|---|
| `waved/` | the firmware (Rust workspace), deploy scripts, hardware docs |
| `docs/` | reverse-engineering reports and guides |
| `site/` | the documentation site |
| `scripts/` | decompilation, cross-version diff and LVGL asset tools |

The stock firmware images and the decompiled stock binaries are not distributed.
`scripts/decompile_version.sh` regenerates the Ghidra decompilation of the six
target binaries from a stock firmware image, once you unpack its rootfs to
`firmwares/<version>/extracted/rootfs`; the output lands in
`firmwares/<version>/decompiled/`. It needs Ghidra and JDK 21 and hardcodes their
paths, so edit `ROOT`, `GHIDRA` and `JAVA_HOME` at the top of the script first.

## Status

Unofficial and not affiliated with Insta360. Use at your own risk; the install
never touches the system partition and `revert.sh` restores stock.

## License

[MIT](LICENSE)
