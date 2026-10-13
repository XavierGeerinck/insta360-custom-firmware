---
title: Troubleshooting
description: What to check and what to do when ADB, the build, the installer or waved itself gets stuck.
---

## Installer messages

| The installer says | Read |
|---|---|
| `no waved binary at …` | Build first: [Install, step 2](/insta360-custom-firmware/start/install/#2-build) |
| `llvm-objcopy not found` | [`llvm-objcopy` not found](/insta360-custom-firmware/start/troubleshooting/#llvm-objcopy-not-found) |
| `no Wave found on adb` | [`adb devices` is empty](/insta360-custom-firmware/start/troubleshooting/#adb-devices-is-empty) |
| `/data has … MB available and waved needs … MB` | ["/data is full"](/insta360-custom-firmware/start/troubleshooting/#data-is-full) |
| `the copy on the Wave does not match (md5)` | [The copy did not match](/insta360-custom-firmware/start/troubleshooting/#the-copy-did-not-match) |
| `the USB gadget patch did not finish` | ["ADB at boot"](/insta360-custom-firmware/start/troubleshooting/#adb-at-boot) |
| `ADB is not enabled at boot` | ["ADB at boot"](/insta360-custom-firmware/start/troubleshooting/#adb-at-boot) |
| `waved did not stop`, `could not copy waved into /data/waved`, `install.sh stopped at: …` | [The installer stopped part-way](/insta360-custom-firmware/start/troubleshooting/#the-installer-stopped-part-way) |

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
to use and says `no Wave found on adb`. Unplug the others, or set
`ANDROID_SERIAL` to the Wave's serial (the `PSP_…` name `adb devices` shows).

## "/data is full"

The installer stopped before it changed anything. Check the space:

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
no ADB can't be reached again. At this point nothing else was changed. The
Wave still runs what it ran before, the stock app or an older waved. Only
`/etc/init.d/S89usbgadget` may have changed. Before the patch changes it, it
saves a copy as `/data/waved/S89usbgadget.bak.<time>`.

Run the installer again. The patch is safe to repeat:

```bash
./deploy/install.sh
```

If it still refuses, look at the lines it changes:

```bash
adb shell grep -n adbd_enable /etc/init.d/S89usbgadget
```

The two lines in the boot branches must read `adbd_enable`, without a `#` in
front. The stock script has them as `#adbd_enable` and `# adbd_enable`. If
they still start with `#`, see
[Making ADB Survive a pspapp Failure](/insta360-custom-firmware/research/device-access/#making-adb-survive-a-pspapp-failure)
in the device access guide.

If the Wave reboots in the meantime, it comes back on what it ran before. On
stock firmware that means ADB is off: turn it on again with `enable_adb.py`
([Install, step 1](/insta360-custom-firmware/start/install/#1-enable-adb)).

## The copy did not match

The binary that arrived in the Wave's `/tmp` is not the one on your computer.
Nothing was changed. Run the installer again. If it keeps happening, try
another USB-C cable.

## The installer stopped part-way

`waved did not stop` means the old waved was still running, so the installer
replaced nothing. The new binary waits in `/tmp/waved.new` until the next
reboot. Run the installer again.

`could not copy waved into /data/waved` means the old binary was already
removed when the copy failed. The new one is still in `/tmp/waved.new`. Check
the space with `adb shell df -h /data`, then run the installer again.

`install.sh stopped at: …` names the step that failed. Run the installer
again; each run starts from the beginning.

Once the installer is past `Patching the USB gadget`, ADB comes up at every
boot. So even if the Wave reboots after a later step failed, you can reach it
to run the installer again or to [revert](/insta360-custom-firmware/start/revert/).

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
