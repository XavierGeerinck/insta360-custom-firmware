---
title: Requirements
description: The Wave, the computer and the tools you need before you install waved.
---

## Your Wave

- An Insta360 Wave conference speakerphone, model CINSABCA.
- Stock firmware. waved has run on a Wave with stock firmware 1.1.6.6. The
  latest stock release, 1.2.2.6, has not been tried yet.
- A USB-C cable that carries data, not only power.

## Your computer

macOS or Linux. The developers build and install from macOS; the install
script also handles Linux tools (`md5sum` where macOS has `md5`).

Windows is not supported: nothing in the repository documents building or
installing from it.

| Tool | What it is for | Install |
|---|---|---|
| Rust (stable, via `rustup`) | Builds waved. The repository pins the stable toolchain and the `aarch64-unknown-linux-gnu` target. | [rustup.rs](https://rustup.rs) |
| `llvm-tools` | `llvm-objcopy`, which the installer uses to strip the binary. | `rustup component add llvm-tools` |
| Docker, running | `cross` compiles inside a container. | Docker Desktop (macOS), Docker Engine (Linux) |
| `cross` | Cross-compiles waved for the Wave. Install it from git: release 0.2.5 has a rustup detection bug on macOS arm64. | `cargo install cross --git https://github.com/cross-rs/cross` |
| `adb` | Talks to the Wave over USB. | `brew install android-platform-tools` (macOS), `sudo apt install adb` (Debian, Ubuntu) |
| Python 3 with `pyusb` | Runs `enable_adb.py`, which turns ADB on once. | `pip3 install pyusb` |
| `libusb` | The USB library `pyusb` talks through. | `brew install libusb` (macOS), `sudo apt install libusb-1.0-0` (Debian, Ubuntu) |

Run `rustup component add llvm-tools` from the repository's `waved/`
directory, so it lands in the toolchain the build uses.

If `pip3` refuses with `externally-managed-environment`, add
`--break-system-packages`, as the
[device access guide](/insta360-custom-firmware/research/device-access/#prerequisites)
does:

```bash
pip3 install --break-system-packages pyusb
```

`enable_adb.py` runs with `sudo`, so `pyusb` must be importable by the
`python3` that `sudo` finds.

## Disk and time

- **Disk:** several GB for the `cross` Docker image and a release build.
- **Time:** about half an hour, most of it the first build.

Next: [Install](/insta360-custom-firmware/start/install/).
