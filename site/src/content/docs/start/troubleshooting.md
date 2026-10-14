---
title: Troubleshooting
description: What to check and what to do when ADB, the build, the installer or waved itself gets stuck.
---

## Installer messages

Every `ERROR:` the installer prints, in the order of its steps. The first
group stops before anything on the Wave changed: fix the cause and run the
installer again.

| The installer says | What it means | What to do |
|---|---|---|
| `no waved binary at …` | There is no build where the installer looks. | Build first: [Install, step 2](/insta360-custom-firmware/start/install/#2-build), or set `WAVED_BINARY` to your build. |
| `no llvm-objcopy at …` | The tool that strips the binary is missing. | [`llvm-objcopy` not found](/insta360-custom-firmware/start/troubleshooting/#llvm-objcopy-not-found) |
| `could not strip … with …` | `llvm-objcopy` failed on the build. | Check that the build finished without errors and build again. |
| `no Wave on adb (adb devices)` | No Wave answered on ADB, or more than one ADB device is attached. | [`adb devices` is empty](/insta360-custom-firmware/start/troubleshooting/#adb-devices-is-empty) |
| `no answer from the Wave about …`, `could not read the free space on /data (…)`, `could not read the size of /data/waved/waved (…)` | The Wave stopped answering during the checks. | [The Wave stopped answering](/insta360-custom-firmware/start/troubleshooting/#the-wave-stopped-answering) |
| `an over-the-air update is on trial (/data/waved/update/trial)` | An over-the-air update has not finished its trial; installing now would be undone. | [An update was on trial](/insta360-custom-firmware/start/troubleshooting/#an-update-was-on-trial) |
| `/data has … MB available and waved needs … MB` | Not enough room on `/data` for the new binary. | ["/data is full"](/insta360-custom-firmware/start/troubleshooting/#data-is-full) |
| `could not copy … to … on the Wave (adb push)`, `… on the Wave is not … (md5 …, want …)` | A file did not arrive in the Wave's `/tmp`, or arrived damaged. | [The copy did not match](/insta360-custom-firmware/start/troubleshooting/#the-copy-did-not-match) |
| `could not read …` | A file in your checkout could not be read for its md5. | Check that `waved/deploy/` and `waved/scripts/` are complete (`git status`). |
| `could not copy config.toml; nothing was changed` | The copy of your settings could not be written, usually because `/data` is full. | ["/data is full"](/insta360-custom-firmware/start/troubleshooting/#data-is-full) |
| `the USB gadget patch did not finish` | The patch that keeps ADB on at boot failed. Nothing was stopped. | ["ADB at boot"](/insta360-custom-firmware/start/troubleshooting/#adb-at-boot) |
| `ADB is not enabled at boot (…)` | The check after the patch found that ADB would be off after a reboot. Nothing was stopped. | ["ADB at boot"](/insta360-custom-firmware/start/troubleshooting/#adb-at-boot) |

The rest come after the installer stopped the stock app and waved. It puts
back what it can and says so on the next line:
[The installer stopped part-way](/insta360-custom-firmware/start/troubleshooting/#the-installer-stopped-part-way).

| The installer says | What it means | What to do |
|---|---|---|
| `could not stop waved` | waved did not exit within 40 seconds, not even when killed. Nothing was replaced. | Run the installer again; if it happens again, reboot the Wave first. |
| `the binary was not replaced` | The new binary did not fit in `/data/waved`. On an update the old one is back in place. | Make room (["/data is full"](/insta360-custom-firmware/start/troubleshooting/#data-is-full)), then run the installer again. |
| `S60waved, S03splash, rsyslog.conf, splash.fb or iptables.rules not installed (the new binary is)` | The new binary is in place, but some start-up files may still be the old ones. | Run the installer again. |
| `config.toml not written` | The default settings file could not be written. | Run the installer again. |
| `could not stub the stock services`, `the stock app and telemetry stubs did not take (…)` | Everything else is installed, but the stock app's stubs did not write or did not read back. The stock app would start next to waved at the next boot. | Run the installer again; [The installer stopped part-way](/insta360-custom-firmware/start/troubleshooting/#the-installer-stopped-part-way) if it stops here again. |
| `could not start the watchdog` | An update installed everything, but waved's watchdog did not start, so waved is not running. | `adb reboot`: waved starts at boot. |

Two can come at any step. From `Stopping stock firmware, the watchdog and
waved` on, the installer puts back what it can, as above.

| The installer says | What it means | What to do |
|---|---|---|
| `install.sh stopped during: …` | A command failed unexpectedly in the step it names. | Run the installer again; each run starts from the beginning. |
| `install.sh interrupted during: …` | Ctrl-C or a closed terminal stopped the installer in the step it names. | Run the installer again. |

## `adb devices` is empty

On stock firmware, `enable_adb.py` asks the stock app to turn ADB on. If
`adb devices` still lists nothing:

1. Check that the computer sees the Wave at all. On macOS:

   ```bash
   ioreg -p IOUSB -l | grep psp
   ```

2. Restart the ADB server:

   ```bash
   adb kill-server && adb start-server
   ```

3. Wake the Wave's screen and go to the home screen, not File Transfer or a
   settings menu. The stock app can stop handling the request while the
   screen sleeps.
4. Quit Wave Controller on your computer. It claims the USB HID interface
   `enable_adb.py` sends through.
5. Run `enable_adb.py` with `sudo`.
6. Power-cycle the Wave (hold the power key about 8 seconds) and try again.

On some Waves the request reaches the stock app and still does nothing, even
after a fresh boot. The device access guide has the way around it:
[ADB Device Not Showing](/insta360-custom-firmware/research/device-access/#adb-device-not-showing).

With waved installed, ADB comes up at every boot by itself, and
`enable_adb.py` no longer works: waved ignores that request. Give the Wave
time to finish booting, then check the cable and restart the ADB server.

If more than one ADB device is attached, the installer can't tell which one
to use and says `no Wave on adb`. Unplug the others, or set
`ANDROID_SERIAL` to the Wave's serial (the `PSP_…` name `adb devices` shows).

## "/data is full"

After `/data has … MB available and waved needs … MB`, the installer stopped
before it changed anything. Check the space:

```bash
adb shell df -h /data
```

`/data` is a 512 MB partition with about 440 MB usable. The stripped waved
binary is about 63 MB, and the installer wants room for it plus 20 MB. It
counts the space an installed waved will give back.

A known cause is the stock system log, `/opt/logs/psp.log`, which lives on
`/data`. The stock app kept it in check, but once the stock app no longer
runs, nothing rotates it. On one Wave it reached 273 MB and filled `/data`.
Look at its size:

```bash
adb shell ls -l /opt/logs
```

If it is large, empty it, then run the installer again:

```bash
adb shell 'cat /dev/null > /opt/logs/psp.log'
```

The installer's `rsyslog.conf` moves the system log to `/tmp`, which is in
RAM and cleared at every boot, so this does not come back once waved is
installed.

## "ADB at boot"

The installer stopped because it could not make ADB come up at every boot.
It refuses to go on without that: a Wave with the stock app switched off and
no ADB can't be reached again. At this point nothing was stopped and nothing
was installed. The Wave still runs what it ran before, the stock app or an
older waved. Only `/etc/init.d/S89usbgadget` may have changed. Before the
patch changes it, it saves a copy as `/data/waved/S89usbgadget.bak.<time>`.
Any lines above the `ERROR:` are what the patch printed when it failed.

Run the installer again. The patch is safe to repeat:

```bash
./deploy/install.sh
```

If it still refuses, look at the lines it changes:

```bash
adb shell grep -n adbd_enable /etc/init.d/S89usbgadget
```

The two lines in the boot branches, inside `start_usbgadget()`, must read
`adbd_enable`, without a `#` in front. The stock script has them as
`#adbd_enable` and `# adbd_enable`. The installer checks both: no
`adbd_enable` line in the file may still start with `#`, and at least one
must be live inside `start_usbgadget()`, the function that runs at boot. The
other `adbd_enable` lines in the file belong to modes the stock app switches
to, and don't count. If the lines still start with `#`, see
[Making ADB Survive a pspapp Failure](/insta360-custom-firmware/research/device-access/#making-adb-survive-a-pspapp-failure)
in the device access guide.

The installer already keeps a copy of the original script in `/data/waved/`.
Never put copies of init scripts in `/etc/init.d`: the Wave runs every `S??*`
file there at boot, so a backup such as `S89usbgadget.orig` would run too.

If the Wave reboots in the meantime, it comes back on what it ran before. On
stock firmware that means ADB is off: turn it on again with `enable_adb.py`
([Install, step 1](/insta360-custom-firmware/start/install/#1-enable-adb)).

## The Wave stopped answering

`no answer from the Wave about …` and `could not read the free space on
/data` mean the Wave did not answer one of the installer's checks. The
installer stopped before it changed anything. Check the connection:

```bash
adb devices
```

The Wave should be listed once, as `device`. If it shows `offline` or is
missing, unplug the cable and plug it in again, then run the installer
again. If it stays missing, see
[`adb devices` is empty](/insta360-custom-firmware/start/troubleshooting/#adb-devices-is-empty).

## The copy did not match

The installer copies every file to the Wave's `/tmp` first and compares each
copy with the file on your computer. `could not copy … (adb push)` means a
copy failed, and `… on the Wave is not …` means a copy arrived different.
Either way nothing on the Wave was changed. Run the installer again. If it
keeps happening, try another USB-C cable or port.

## The installer stopped part-way

These are the failures after the installer stopped the stock app and waved
(`Stopping stock firmware, the watchdog and waved` and later). The installer
then puts back what it can, and says so on the line after the `ERROR:`:

- `The watchdog runs again.` (an update): waved runs again, the old binary or
  the new one, whichever is in place.
- `Could not start the watchdog: …` (an update): waved is not running. Reboot
  the Wave with `adb reboot`; waved starts at boot.
- `Stock firmware was stopped: adb reboot, then run this again.` (a first
  install): reboot the Wave, then run the installer again.

ADB comes up at every boot by then: the installer checked it before it
stopped anything. So after a reboot you can always reach the Wave to run the
installer again or to [revert](/insta360-custom-firmware/start/revert/).

`could not stop waved` means waved did not exit, so the installer replaced
nothing. The new binary waits in `/tmp/waved.new` until the next reboot.

`the binary was not replaced` means the new binary did not fit in
`/data/waved`; the line above it says `the new binary did not fit on /data`.
On an update the old binary is back in place (a copy also waits in
`/tmp/waved.old` until the Wave restarts). On a first install there was no
old one: after the reboot the stock app runs again. Make room
(["/data is full"](/insta360-custom-firmware/start/troubleshooting/#data-is-full))
and run the installer again.

`could not stub the stock services` and `the stock app and telemetry stubs
did not take` mean everything else is installed, but the two stubs that keep
the stock app and its telemetry off did not write or did not read back as
written. With the stock app's start-up script still in place, the stock app
would start next to waved at the next boot. Run the installer again. If it
stops here again, look at what the Wave has:

```bash
adb shell 'cat /etc/init.d/S60pspapp_daemon; ls -l /usr/lib/libapplogrs.so'
```

The stub start-up script is the one line `#!/bin/sh`, and the stub library is
empty (0 bytes, or 1 on a Wave an older installer set up). The installer
writes a stub only where the overlay has none yet, so a different file in
the overlay stays. To clear it, delete the two from the overlay's upper
directory, reboot, and run the installer again:

```bash
adb shell rm -f /data/overlay/upper/etc/init.d/S60pspapp_daemon /data/overlay/upper/usr/lib/libapplogrs.so
adb reboot
```

`install.sh stopped during: …` names the step in which a command failed
unexpectedly, and `install.sh interrupted during: …` the step in which Ctrl-C
or a closed terminal stopped the installer. Run the installer again; each run
starts from the beginning.

## An update was on trial

This only matters if you use [over-the-air updates](/insta360-custom-firmware/guides/updates/),
which are host-tested only.

After an update installs, the new version is on trial until it has run for 60
seconds with working microphones. While it is, `/data/waved/update/trial`
exists. It lists the files the update replaced, and the Wave keeps the
previous version of each as a `.prev` copy. If you run `install.sh` during a
trial, those copies can come back over what you installed:

- at the start of the third boot of a trial that never finished (or after
  more than ten starts in one boot), `S60waved` moves every listed file back
  from its `.prev` copy, and
- a waved that finds a trial for a different version puts the listed
  start-up files and settings back.

Before you install, check:

```bash
adb shell ls /data/waved/update
```

If `trial` is listed, let the Wave run for a minute and check again. Install
once it is gone. If it stays, read
[Install and fall-back](/insta360-custom-firmware/guides/updates/#install-and-fall-back-trial-boots)
first.

## waved does not start

```bash
adb shell pidof waved
adb shell tail -50 /data/waved/waved.log
adb shell df -h /data
```

`S60waved` restarts waved 3 seconds after it exits, and writes
`waved exited (<code>), restarting in 3s...` to the log each time. A few such
lines in the first seconds after boot are normal. `S60waved` stops the first
waved after 5 seconds on purpose, because the screen doesn't work in the
first one, and the speaker can be busy early in a boot, which makes waved
exit and start again. If the lines keep coming, waved keeps crashing; the
lines above each one say why.

If `/data` is full, waved can exit at start-up without writing anything,
because even its log write fails. See ["/data is full"](/insta360-custom-firmware/start/troubleshooting/#data-is-full).

To go back to stock while you sort it out: [Revert](/insta360-custom-firmware/start/revert/).

## Build fails with sccache

If your `~/.cargo/config.toml` sets `rustc-wrapper = "sccache"`, the build
inside the `cross` container fails. Copy `waved/Cross.toml`, add
`"RUSTC_WRAPPER=",` to the `passthrough` list under `[build.env]` in the copy,
and point `cross` at it:

```bash
CROSS_CONFIG=/path/to/your/Cross.toml WAVED_BUILD_ID=$(git rev-parse --short HEAD) \
  cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```

Don't use `CROSS_BUILD_ENV_PASSTHROUGH` instead: it replaces the whole list
and drops a compiler flag one of the dependencies needs.

## `llvm-objcopy` not found

The installer strips the binary with `llvm-objcopy` from Rust's
`llvm-tools`. Install it from the `waved/` directory:

```bash
rustup component add llvm-tools
```
