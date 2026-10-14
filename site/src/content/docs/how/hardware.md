---
title: Hardware overview
description: What is inside the Insta360 Wave, from the SoC to the microphones, radios and USB functions, with a link to the full reference for each part.
---

Every fact here comes from the hardware and protocol reference pages, which
were measured on a Wave or read from the stock firmware. Each row links to
its page.

| Part | What it is | Reference |
|---|---|---|
| SoC | Amlogic A113X2 (AV400 audio platform). | [Compute hardware](/insta360-custom-firmware/reference/compute-hardware/) |
| CPU | 4 × Arm Cortex-A55, up to 2.016 GHz, NEON with `asimddp` and `asimdhp`. All of waved's audio processing runs here. | [Compute hardware](/insta360-custom-firmware/reference/compute-hardware/#summary) |
| NPU | Vivante VIP neural accelerator (`galcore` driver). Stock runs its speech models on it. The project's own native backend now runs a complete trained model on it, NVIDIA's MarbleNet voice-activity detector, 10.4× faster than one CPU core. waved doesn't use the NPU yet. | [Compute hardware](/insta360-custom-firmware/reference/compute-hardware/#vivante-npu), [MarbleNet on the NPU](/insta360-custom-firmware/research/npu/onnx/marblenet/) |
| Audio DSP | A HiFi-class DSP at `/dev/audiodsp0` with an 8 MB firmware region. Its toolchain is vendor-locked; waved doesn't use it. | [Compute hardware](/insta360-custom-firmware/reference/compute-hardware/#audio-dsp-devaudiodsp0) |
| RAM | 3,834 MB, plus 800 MB of compressed swap (`zram`). | [Device dump](/insta360-custom-firmware/research/device-dump/#2-memory) |
| Storage | 30 GB eMMC. Two slots each for the kernel and the read-only system (A/B); `/data` (512 MB, waved and the overlay); `/model` (4.6 GB, stock's AI models); `/mnt/udisk` (21.5 GB exFAT, recordings). | [Device dump](/insta360-custom-firmware/research/device-dump/#3-emmc-partition-map-30-gb) |
| Microphones | 8 MEMS mics in two rings of four, all on the diagonals (45°, 135°, 225°, 315°). Ring radius 25.5 mm, rings 78.2 mm apart. Read as one ALSA device, `hw:0,2`: 8 channels, 48 kHz, 32-bit samples that carry about 24 bits. | [Audio hardware](/insta360-custom-firmware/reference/audio-hardware/#mic-array-geometry) |
| Speaker | Driven by an `acm8625s` amplifier on the TDM-B link, ALSA `hw:0,1`. | [Audio hardware](/insta360-custom-firmware/reference/audio-hardware/#playback-dsp-chain-sp5-speaker-scenes-2026-10-13) |
| Display | Round LCD, driver `fb_ili79400`. 400×304 framebuffer at 16 bits (RGB565), of which a circle of radius about 198 around (200, 106) shows, cut flat at the top. Mounted upside down. PWM backlight, 0 to 255. | [Display](/insta360-custom-firmware/reference/display-hardware/#hardware) |
| Touch | The touch half of the same `ilitek,ili79400` controller (`ILITEK_TDDI`), on `/dev/input/event1`. Reports screen pixels and is mounted upside down like the display. | [Display](/insta360-custom-firmware/reference/display-hardware/#hardware) |
| Bluetooth | A Telink module that runs its own Bluetooth stack (A2DP sink, HFP hands-free, AVRCP, PBAP, SPP, LE) and a 2.4 GHz link to Insta360's USB dongle. The SoC drives it over `/dev/ttyS1` at 921600 baud with a small vendor protocol, not HCI. Its audio runs over the TDM-A link, ALSA `hw:0,0`: 48 kHz, 2 channels, 16-bit. | [Bluetooth protocol](/insta360-custom-firmware/reference/bluetooth-protocol/#hardware) |
| MCU | A separate microcontroller on `/dev/ttyS4` at 115200 baud, speaking a private MAVLink v2 dialect. It handles the battery, charging, the lift motor, the LED, the power key and power off. | [MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/) |
| Battery | Read through the MCU: level, voltage, temperature and current arrive in its heartbeat, about every 2 seconds. | [MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/#heartbeat-msgid-0-mcu--host-verified-on-device) |
| Power key | Not a Linux input: the MCU reports presses. Holding the key 2 seconds or more powers the Wave off in the MCU itself. | [MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/#the-power-key-verified-on-device) |
| Lift | A motor raises the microphone column: 30, 40 or 50 mm, or 0 mm retracted. The MCU drives it and keeps the height to rise to at power-on. | [MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/#motor-move-cmd-3-sub-0) |
| Wi-Fi | ATBM6031 (ATBM613x family) on SDIO, 2.4 and 5 GHz, interface `wlan0`. The driver is `atbm613x_wifi_sdio.ko`; its log lines are tagged `atbm_log`. One radio: the Wave is either a Wi-Fi client or an access point, never both. | [Wi-Fi](/insta360-custom-firmware/guides/wifi/#hardware--stack) |
| USB audio | The USB gadget's UAC2 function: a stereo, 16-bit, 48 kHz sound card in both directions (`card 1` on the Wave). | [Audio hardware](/insta360-custom-firmware/reference/audio-hardware/#uac2-routing-sp6) |
| USB HID | One HID interface (`/dev/hidg0`) with Consumer keys (volume, mute), Telephony reports (call state from the computer) and vendor report 0x03, which the Wave Controller app uses. | [Audio hardware](/insta360-custom-firmware/reference/audio-hardware/#hid-control-surface-sp7d) |

## The serial ports

Three UARTs, easy to mix up:

| Port | Speed | Peer |
|---|---|---|
| `/dev/ttyS1` | 921600 | Bluetooth module |
| `/dev/ttyS4` | 115200 | MCU |
| `/dev/ttyS3` | 115200 | Stock calls its peer the camera module ("PUC"); waved doesn't use it |

Until 2026-10-12 waved talked to `/dev/ttyS1` as if it were the MCU, at the
wrong speed. Older notes that describe MCU traffic on `/dev/ttyS1` come from
that time. [MCU protocol](/insta360-custom-firmware/reference/mcu-protocol/#which-uart-is-which)
has the story.

## The ALSA devices

Card 0 has four devices, one per audio link in the device tree:

| Device | Link | Carries |
|---|---|---|
| `hw:0,0` | TDM-A | Bluetooth audio, both directions |
| `hw:0,1` | TDM-B | The speaker |
| `hw:0,2` | PDM | The eight microphones |
| `hw:0,3` | Loopback | Unused in waved's default audio configuration |

Card 1 is the USB audio function.
[Bluetooth protocol](/insta360-custom-firmware/reference/bluetooth-protocol/#hardware)
and [Audio hardware](/insta360-custom-firmware/reference/audio-hardware/) have
the details.
