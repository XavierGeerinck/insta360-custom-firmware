---
title: Install
description: Build waved, put it on your Wave over USB, and reboot into it.
---

Plan on about half an hour, most of it the first build. Everything the
installer changes lives on the Wave's `/data` partition, apart from an empty
`recordings` folder on the recordings volume. The read-only system partition is
never touched, and [one script puts stock back](/insta360-custom-firmware/start/revert/).

:::caution
This is unofficial firmware. It is tested on one Wave at a time by its
developers. Read [Revert to stock](/insta360-custom-firmware/start/revert/)
before you start.
:::

Check the [requirements](/insta360-custom-firmware/start/requirements/) first:
you need Rust, Docker, `cross`, `adb` and Python with `pyusb`.

## 1. Enable ADB

Already running waved? Skip to [step 2](/insta360-custom-firmware/start/install/#2-build):
waved turns ADB on at every boot, and `enable_adb.py` only works with the
stock app.

ADB is how your computer talks to the Wave. Stock firmware keeps it off. With
the Wave on stock firmware, connected over USB-C, and its screen awake on the
home screen, run this from the repository root:

```bash
sudo python3 waved/scripts/enable_adb.py
sleep 3
adb devices        # shows PSP_…    device
```

This ADB session lasts until the Wave reboots. That is enough: the installer
turns ADB on at every boot from then on. If `adb devices` shows nothing, see
[Troubleshooting](/insta360-custom-firmware/start/troubleshooting/#adb-devices-is-empty).

:::danger
Never send the HID "factory devmode" command (`byte[5] = 0xEE`). It chains
into a factory reset that wipes recordings, cloud binding and region. Details:
[Device access](/insta360-custom-firmware/research/device-access/#other-hid-commands).
:::

## 2. Build

From the repository root:

```bash
cd waved
WAVED_BUILD_ID=$(git rev-parse --short HEAD) \
  cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```

The first build also builds the Docker image `cross` compiles in, so it takes
a while. To let the Wave install signed updates later, set
`WAVED_UPDATE_PUBKEY` before building
([Updates](/insta360-custom-firmware/guides/updates/)). A build without it
can only be replaced over ADB.

## 3. Install

Still in `waved/`:

```bash
./deploy/install.sh
```

The installer checks before it changes anything: that a Wave answers on ADB,
that the build exists, that it can strip the binary, and that `/data` has room.
Then it

1. strips the binary, copies it to the Wave's `/tmp` and checks that it
   arrived intact (`Copying waved … to the Wave`),
2. patches the USB gadget start-up script so ADB comes up at every boot, the
   volume ranges work and the HID controls appear, then checks that ADB will
   be on after a reboot (`Patching the USB gadget`). If it won't, the
   installer stops here and the Wave still runs what it ran before,
3. stops the stock app and any running waved (`Stopping the stock app and any
   running waved`). If waved does not stop, it refuses to replace anything,
4. swaps in the new waved binary (`Installing waved`),
5. writes a default `config.toml` if the Wave has none; an existing one is kept
   (`Writing the default config.toml`),
6. installs the start-up scripts, the boot splash, a logging fix that keeps
   `/data` from filling up, and the firewall rules (`Installing start-up
   scripts, splash, logging and firewall rules`), and
7. last of all, switches the stock app and its telemetry library off with two
   stubs, and reads them back to check they took (`Switching the stock app
   and its telemetry off`).

The order is deliberate. A Wave without the stock app and without ADB can't
be reached again, so nothing stock is switched off until ADB is known to come
up at the next boot.

It ends with `=== Done. Reboot the Wave to start waved: adb reboot ===`. If a
step fails, the installer stops there and usually prints an `ERROR:` line that
says what to do next. Messages that point to a Troubleshooting section:
["/data is full"](/insta360-custom-firmware/start/troubleshooting/#data-is-full)
and ["ADB at boot"](/insta360-custom-firmware/start/troubleshooting/#adb-at-boot).

Running it again later upgrades waved and keeps your settings.

## 4. Reboot

```bash
adb reboot
```

The Insta360 logo from the bootloader shows briefly, then the waved splash.
About 10 to 15 seconds after power-on waved's own screens appear. On a fresh
install you land on **Set up Wave**.

To check that waved runs:

```bash
adb devices          # the Wave is back, without enable_adb.py
adb shell pidof waved  # prints a process number
```

Next: [First setup](/insta360-custom-firmware/start/first-setup/).
