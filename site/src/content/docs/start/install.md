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

The installer checks before it changes anything: that the build exists, that
it can strip the binary, that a Wave answers on ADB, that no over-the-air
update is on trial, that `/etc` and `/usr` are the overlays it writes
through, and that `/data` has room (`Checking the Wave`, `Checking free space
on /data`). Then it

1. strips the binary and copies it to the Wave's `/tmp`, with the start-up
   scripts, the boot splash, the logging fix, the firewall rules, the USB
   gadget patch and, if the Wave has no `config.toml` yet, the default one,
   and checks that each one arrived intact (`Pushing to /tmp on the Wave (md5
   checked)`),
2. if the Wave has a `config.toml` (an update), copies it to
   `/data/waved/config.toml.before-<UTC time>` (`Keeping a copy of
   config.toml`). Your `config.toml` itself is never replaced,
3. patches the USB gadget start-up script so ADB comes up at every boot, the
   volume ranges work and the HID controls appear (`Patching the USB
   gadget`), then checks that ADB will be on after a reboot (`Checking that
   ADB comes up at boot`). If it won't, the installer stops here, before
   anything is stopped, and the Wave still runs what it ran before,
4. stops the stock app, waved's watchdog and any running waved (`Stopping
   stock firmware, the watchdog and waved`). If waved does not exit, it
   replaces nothing,
5. swaps in the new waved binary (`Swapping the binary`). The old one waits in
   `/tmp/waved.old`, and goes back in place if the new one does not fit,
6. installs the start-up scripts, the boot splash, a logging fix that keeps
   `/data` from filling up, and the firewall rules (`Installing S60waved,
   S03splash, rsyslog.conf, the splash and the firewall rules`),
7. if the Wave had no `config.toml`, writes the default one (`Writing the
   default config.toml`), and
8. last of all, switches the stock app and its telemetry library off with two
   stubs (`Stock pspapp off, telemetry library stubbed`). A stub an earlier
   install left is kept as it is (`kept …`); a missing one is written and read
   back to check it took (`wrote …`).

The order is deliberate. A Wave without the stock app and without ADB can't
be reached again, so nothing is stopped or switched off until ADB is known to
come up at the next boot.

How it ends depends on whether waved was on the Wave already:

- **A first install** (no waved on the Wave yet; the stock app ran) ends with
  `=== Done! Reboot to start waved ===` and `Run: adb reboot`. Go on with
  [step 4](/insta360-custom-firmware/start/install/#4-reboot).
- **An update** (waved was on the Wave) starts waved's watchdog again
  (`Starting the watchdog`), which starts the new waved, and ends with
  `=== Done. The previous binary is in /tmp/waved.old until the Wave
  restarts. ===`. Your settings stay. The new waved runs without a reboot.
  If the update also changed what only takes effect at boot (the USB
  gadget patch, the boot splash or the logging settings), a last line says
  so, for example `The new boot splash and logging settings take effect at
  the next boot: adb reboot`. Reboot when it suits you.

If a step fails, the installer stops there and prints an `ERROR:` line that
says what to do next. This holds for Ctrl-C, a closed terminal or a closed
output pipe too. Once it has stopped anything, it also puts things back as
far as it can: after a failed update it puts the previous binary back if the
swap was cut off, then starts the watchdog again (`The watchdog runs
again.`); after a failed first install it says `Stock firmware was stopped:
adb reboot, then run this again.` Every message and what to do about it:
[Installer messages](/insta360-custom-firmware/start/troubleshooting/#installer-messages).

## 4. Reboot

After a first install, or when an update's last line asks for it:

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
