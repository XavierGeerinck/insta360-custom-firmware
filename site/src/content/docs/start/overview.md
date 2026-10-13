---
title: Overview
description: What waved is, what it changes on your Wave, and what works today.
---

## What it is

`waved` is one Rust program that replaces the Insta360 Wave's stock app
(`pspapp`) and its telemetry. You build it from this repository and install it
over USB. It runs the touch screen, the microphones and speaker, USB audio,
Wi-Fi, recording and uploads.

This project is not affiliated with Insta360.

## What changes on your Wave, and what doesn't

The installer writes only to the Wave's `/data` partition:

- `/data/waved/`: waved itself, `config.toml`, the boot splash
  (`splash.fb`) and the firewall rules (`iptables.rules`). waved keeps its log
  and its recordings list here too.
- The `/etc` and `/usr` overlays, stored under `/data/overlay/upper`:
  - `S60waved`, which starts waved and restarts it if it exits,
  - `S03splash`, which shows the splash early in the boot,
  - `rsyslog.conf`, which keeps the system log in RAM instead of on `/data`,
  - a patched `S89usbgadget`, which turns ADB on at every boot and sets up the
    USB volume ranges and HID controls,
  - a stub `S60pspapp_daemon`, so the stock app no longer starts, and
  - an empty `libapplogrs.so` in place of the stock telemetry library.

Recordings go to `/mnt/udisk/recordings`, on the Wave's recordings volume.
The installer creates that folder and changes nothing else there.

The system partition is never written. The Wave keeps the stock system it
had, which is why [Revert to stock](/insta360-custom-firmware/start/revert/) is
one script and a reboot.

## What works today

"On the Wave" means it has run on a real Wave. "Host-tested" means it passes
its tests on a computer but has not run on a Wave yet.

| Feature | Status | More |
|---|---|---|
| Touch screens: Home, Recording, Files, Menu and settings | Works, on the Wave | [Features](/insta360-custom-firmware/features/) |
| USB speakerphone (UAC2) with echo cancellation | Works, on the Wave, with macOS | [Audio hardware](/insta360-custom-firmware/reference/audio-hardware/#uac2-routing-sp6) |
| Noise reduction | Off for now: too slow on the CPU | [Audio hardware](/insta360-custom-firmware/reference/audio-hardware/#uplink-level-and-cpu-device-pass-2026-10-06) |
| Recording on the Wave | Works, on the Wave. The one-hour and power-loss certification is still to do. | [Recording webhooks](/insta360-custom-firmware/guides/webhooks/) |
| Uploading recordings to your own server | Works, on the Wave. The full failure-and-recovery test is still to do. | [Recording webhooks](/insta360-custom-firmware/guides/webhooks/) |
| Wi-Fi setup with a QR code | Works, on the Wave | [Wi-Fi](/insta360-custom-firmware/guides/wifi/) |
| Power key: sleep, wake, hold to power off | Works, on the Wave | [MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/) |
| Bluetooth pairing and calls | Host-tested. The Bluetooth module's pairing commands were checked on the Wave. | [Bluetooth protocol](/insta360-custom-firmware/reference/bluetooth-protocol/) |
| Bluetooth phone audio | Host-tested | [Audio hardware](/insta360-custom-firmware/reference/audio-hardware/#bluetooth-audio-i2) |
| Wave Controller app | Built from the decompiled stock code; not yet tried with the real app | [Wave Controller protocol](/insta360-custom-firmware/reference/wave-controller-protocol/) |
| Signed over-the-air updates from your own server | Host-tested only | [Updates](/insta360-custom-firmware/guides/updates/) |
| PIN lock | Host-tested | [First setup](/insta360-custom-firmware/start/first-setup/#pin-lock) |
| Battery level | Host-tested | [MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/) |
| Mic height | The lift moves on the Wave; the Mic height screen is host-tested | [MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/) |
| USB storage mode | Not yet | |
| Languages other than English | Not yet | |

## Why

- The stock firmware runs ByteDance's DataFinder telemetry SDK. It reports more
  than 35 kinds of event, from every boot to every mute, keyed to the device
  serial, with no visible opt-out.
  [Security Q&A](/insta360-custom-firmware/research/security/#bytedance-telemetry-always-active)
- Stock uploads meeting audio, transcripts and voiceprints to Insta360's
  cloud, to a data centre picked by region. waved sends recordings only to a
  server you configure, or keeps them on the Wave.
  [Security Q&A](/insta360-custom-firmware/research/security/#meeting-data-geo-routing)
- Stock hardcodes outside services, such as Chinese time servers and a
  connectivity check against `114.114.114.114`. waved's time servers are a
  setting you can change.
  [Security Q&A](/insta360-custom-firmware/research/security/#other-chinese-infrastructure)

Next: [Requirements](/insta360-custom-firmware/start/requirements/).
