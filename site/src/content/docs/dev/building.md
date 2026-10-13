---
title: Building
description: Cross-compile waved for the Wave with cross and Docker, and what the build container, the features and the environment variables do.
---

You need the tools from [Requirements](/insta360-custom-firmware/start/requirements/):
Rust, Docker, `cross` and `llvm-tools`.

## Build waved

From the repository root:

```bash
cd waved
WAVED_BUILD_ID=$(git rev-parse --short HEAD) \
  cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```

The binary lands in `waved/target/aarch64-unknown-linux-gnu/release/waved`.
`./deploy/install.sh` strips it and puts it on the Wave
([Install](/insta360-custom-firmware/start/install/#3-install)).

The first build also builds the Docker image `cross` compiles in, so it takes
a while. Later builds reuse the image.

## Features

`waved/crates/waved/Cargo.toml` defines these:

| Feature | What it adds |
|---|---|
| `ui` | The touch screen (`waved-ui`). |
| `capture-alsa` | The real audio hardware through ALSA: the microphones, the speaker, USB audio, Bluetooth audio, and the USB HID controls that start with it. |
| `simulate` | Simulated hardware, for tests on a computer. It can't be combined with `capture-alsa`: the build stops with an error. |
| `recording-integrity-test-hooks` | Fault injection for `scripts/test-recording-integrity.sh`. Not for normal builds. |

A build for the Wave uses `ui,capture-alsa`.

## The build container

`cross` builds inside Docker. `waved/Cross.toml` points it at
`waved/Dockerfile.cross-aarch64`, which starts from cross's CentOS 7 image for
`aarch64-unknown-linux-gnu` and adds two things:

- **The C library stays old on purpose.** The image links against CentOS 7's
  glibc 2.17. The Wave runs glibc 2.31, so a binary linked against the older
  one runs there.
- **libclang and ALSA.** It installs LLVM 7's `libclang`, which `bindgen`
  needs to generate the LVGL bindings, and builds `alsa-lib` 1.2.11 for the
  target from a pinned, checksummed source release.

`Cross.toml` also passes environment variables into the container: the C
flags `ring` needs (`-D__ARM_ARCH=8`), the paths to `libclang` and the target
ALSA, and the two variables below.

## Environment variables

- **`WAVED_BUILD_ID`** is the build id the About screen shows next to the
  version. The container has no `git`, so pass it in as above. Without it, a
  `cross` build has an empty id.
- **`WAVED_UPDATE_PUBKEY`** is the public half of your update signing key, 64
  hex characters, compiled into waved. A build without it refuses every
  over-the-air update: its **System → Firmware update** row reads Off, and you
  can only replace it over ADB. To make a key:

  ```bash
  cargo run -p waved-update --bin waved-pack -- keygen ~/.config/waved/update.key
  ```

  It prints the public key to export before you build. The rest of the
  release steps are in
  [Updates, owner's quick start](/insta360-custom-firmware/guides/updates/#owners-quick-start).

`deploy/install.sh` reads two more: `WAVED_BINARY` installs a binary from
another path, and `OBJCOPY` names the strip tool if it isn't the
`llvm-objcopy` from your Rust toolchain.

## sccache

If your `~/.cargo/config.toml` sets `rustc-wrapper = "sccache"`, turn the
wrapper off for the container build: make a copy of `Cross.toml`, add
`"RUSTC_WRAPPER="` to the `passthrough` list under `[build.env]` in the copy,
and run `cross` with `CROSS_CONFIG` set to the copy's path.

Don't use `CROSS_BUILD_ENV_PASSTHROUGH` for this. It replaces the whole list
and drops the `-D__ARM_ARCH=8` flag that `ring` needs.

## Other binaries

To build one of the tools instead of waved, name its package. For example,
the on-device check:

```bash
cross build --release --target aarch64-unknown-linux-gnu -p waved-device-test
```

`waved-pack`, the update packer, runs on your computer. Without arguments it
prints its commands (`keygen`, `pubkey`, `pack`, `inspect`):

```bash
cargo run -p waved-update --bin waved-pack
```

## Deploying by hand

`install.sh` is the normal way onto a Wave. For quick iterations there is a
manual procedure: strip, push to `/tmp`, compare checksums, stop the start-up
script's restart loop, and swap the binary once waved has exited.
`/data` is nearly full, and a plain `adb push` over the running binary can
leave the Wave without one. Follow
[Display, Deploy](/insta360-custom-firmware/reference/display-hardware/#deploy)
step by step.
