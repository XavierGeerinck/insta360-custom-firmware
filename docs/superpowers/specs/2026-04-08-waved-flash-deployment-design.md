# waved Flash Deployment Design — 2026-04-08

## Context

This spec defines the deployment strategy for landing the `waved` open-firmware
binary on a real Insta360 Wave for the first time. The device under test is
`PSPLB0000AAAAA` running stock firmware `PSP_1.1.6.6`, slot `_b` active, with
factory devmode (`unifykeys[3] = 0x9257`) persistent in the hardware-backed
unifykey partition.

This spec is written immediately after two safety checkpoints completed today:

1. **Boot-time ADB verified** via an overlay edit to `/etc/init.d/S89usbgadget`
   that uncomments `adbd_enable` in both branches of `start_usbgadget`. The
   edit lives only in `/data/overlay/upper/etc/init.d/S89usbgadget`
   (md5 `554342b3eceeda0d5899b84b2a80e345`); the SquashFS lower layer remains
   stock (md5 `e7c3f5acfee8bce8d67b1af853d05448`). Reboot test confirmed
   `adbd` PID 606 starts at uptime ~8.5s, ~10s before pspapp's `uac_adb`
   call would have started its own adbd. ADB now reaches the device at
   uptime ~8s regardless of whether pspapp or any replacement is running.

2. **`pyamlboot` installed and verified** on the host (`AmlogicSoC` class
   imports cleanly, default VID/PID `0x1b8e:0xc003`, `boot-g12.py` available).
   Device is currently visible at `0x18f0:0x4e40` (normal mode). MaskROM entry
   is unverified and gated on UART or eMMC pad work — but the toolchain is
   ready when needed.

Combined, these mean: a broken `waved` deployment can no longer cost us ADB
access, and a fully bricked device has a documented MaskROM recovery path.
That changes the risk math enough to attempt the first real deployment.

This spec also assumes the existence of full md5-verified partition backups
under `backups/partitions/` (16 partitions, 3.5 GiB) captured today.

## Goals

- Run the `waved` binary persistently on a Wave for the first time.
- Suppress `pspapp` and the ByteDance DataFinder telemetry library
  (`libapplogrs.so`) as part of the same deployment.
- Preserve a one-command rollback path to pure stock firmware at every step.
- Define a deferred design for a real partition flash to slot A, to be
  executed once `waved` has been validated as a persistent service across
  multiple reboot/use cycles.

## Non-goals

- LVGL UI work (Phase 2). The screen will go blank after C2 because waved
  Phase 1 has no UI. Expected, not a regression.
- PUC camera, OTA system, audio replacement (Phases 3–5).
- Custom kernel work. Stock kernel is reused throughout C1–C4.
- Producing a flashable `.swu` package. C4 writes raw partitions via `dd`.

## Architecture: Four checkpoints, three executed now

The design is structured as four checkpoints. C1–C3 are executed in this
round and operate entirely above the partition layer (binary in `/data/`,
init scripts in the overlay upper layer). C4 is the eventual partition flash
to slot A and is documented now but executed later, after C3 has been
stable for some period.

| #  | Name                          | Persistent? | Proves                                                                                                      | Rollback                        |
|----|-------------------------------|-------------|-------------------------------------------------------------------------------------------------------------|---------------------------------|
| C1 | Foreground smoke test         | No          | `waved` launches on this hardware, hardware FFI works, no immediate panic                                   | `pkill waved`                   |
| C2 | Overlay install               | Yes (until revert) | `waved` installs as init-managed service, pspapp stubbed, telemetry stub in place — kernel + SquashFS untouched | `revert.sh`                     |
| C3 | Reboot survival validation    | Yes         | `S60waved` actually starts `waved` automatically, ADB still up at uptime ~8s, hardware functional from waved | `revert.sh + reboot`            |
| C4 | Slot A partition flash        | Yes — and now in flash | Custom firmware lives in a real partition; slot B remains untouched stock                                   | misc rewrite back to slot B     |

### Safety nets in play across all checkpoints

1. **Boot-time ADB** via the verified S89usbgadget overlay edit. `adbd`
   starts at uptime ~8s regardless of pspapp/waved state.
2. **Factory devmode** persistent in unifykey hardware partition. Even if
   the S89usbgadget overlay file is somehow lost, pspapp would call
   `uac_adb` at boot — *but only under C1, not C2/C3*, because C2 stubs
   pspapp out. See R6 in the risk register.
3. **Slot B untouched stock** through all of C1–C3. The SquashFS rootfs
   has not been modified.
4. **Full partition backups** (3.5 GiB md5-verified) under
   `backups/partitions/`.
5. **`/data/overlay/upper/` is the entire blast radius** for C2/C3.
   Clearing the overlay returns to pure stock.

## C1 — Foreground smoke test

Four sub-steps in order. No persistent state changes except leaving the
binary in `/data/waved/` for reuse by C2.

### C1a — Host pre-flight (Mac)

```bash
cd waved
cargo build --release -p waved --features simulate
cargo run -p waved --features simulate
```

**Pass criteria:** binary boots in simulator, no panics, expected services
start. If this fails, do not cross-build.

### C1b — Cross build (fresh)

```bash
cross build --release --target aarch64-unknown-linux-gnu -p waved
cross build --release --target aarch64-unknown-linux-gnu -p waved-device-test
```

A fresh cross build is required because `waved-meeting/Cargo.toml`,
`waved-uart/Cargo.toml`, and `waved-device-test/Cargo.toml` have been
modified since the last build artifact (March 23). If `cross` itself
fails to detect rustup (the known 0.2.5 issue noted in the README),
install from git per the README workaround.

**Pass criteria:** both binaries produced under
`target/aarch64-unknown-linux-gnu/release/`.

### C1c — On-device hardware self-test

```bash
adb shell mkdir -p /data/waved
adb push target/aarch64-unknown-linux-gnu/release/waved-device-test /data/waved/
adb shell killall pspapp_daemon pspapp_watchdog pspapp
adb shell /data/waved/waved-device-test
```

This is the highest-information sub-step. `waved-device-test` is a focused
hardware FFI validation suite that exists specifically for first-run device
validation. It exercises UART/MCU/BT/HID/audio FFI without running the
full daemon.

**Pass criteria:** all per-test results pass. Any FFI failure here means
the daemon will fail too — fix the failing subsystem before proceeding.

### C1d — On-device foreground daemon

```bash
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/
adb push waved/config/default.toml /data/waved/config.toml
adb shell timeout 60 /data/waved/waved
```

**Pass criteria:** binary produces output within 30s (any startup logging),
no panics or aborts within the full 60s window, no unhandled segfaults
visible in `dmesg`. Specific log content is not asserted because waved
has not run on real hardware before — we are establishing the baseline,
not matching it. The `timeout 60` ensures we always recover the shell.

After C1d succeeds, the binary is left in `/data/waved/`. C2 will reuse
it. No reboot is necessary between C1 and C2 — but if anything in C1
left the device in a weird state, `adb reboot` is the cleanest reset.

## C2 — Overlay install

```bash
./waved/deploy/install.sh
```

`install.sh` performs seven steps:

1. `killall pspapp pspapp_daemon pspapp_watchdog`
2. `adb push waved → /data/waved/waved` (chmod +x)
3. Push `config/default.toml → /data/waved/config.toml` if not present
4. Push `S60waved → /data/overlay/upper/etc/init.d/S60waved` (chmod +x)
5. Write empty stub to `/data/overlay/upper/etc/init.d/S60pspapp_daemon`
6. Write empty file to `/data/overlay/upper/usr/lib/libapplogrs.so`
7. Push `iptables.rules` and `mkdir -p /mnt/udisk/recordings`

### C2 validation (no reboot yet)

```bash
adb shell '
ls -la /data/overlay/upper/etc/init.d/S60waved \
       /data/overlay/upper/etc/init.d/S60pspapp_daemon \
       /data/overlay/upper/etc/init.d/S89usbgadget \
       /data/overlay/upper/usr/lib/libapplogrs.so
test -x /data/waved/waved && echo "binary present"
md5sum /data/waved/waved /data/waved/config.toml /data/waved/iptables.rules
ls -la /mnt/udisk/recordings
'
```

**Critical extra check:** confirm
`/data/overlay/upper/etc/init.d/S89usbgadget` md5 is still
`554342b3eceeda0d5899b84b2a80e345`. If `install.sh` touched it, that's a
bug to investigate before reboot — see R6 in the risk register.

If anything is missing or any md5 is wrong, **do not reboot.** Debug
`install.sh` manually, fix, re-run.

## C3 — Reboot survival validation

```bash
adb reboot
adb wait-for-device
```

Then run the verification block:

```bash
adb shell '
echo "uptime: $(cat /proc/uptime | cut -d\" \" -f1)"
echo "--- waved process ---"
ps -o pid,ppid,user,comm | grep -E "waved|S60waved" | grep -v grep
echo "--- pspapp process (should be EMPTY) ---"
ps -o pid,user,comm | grep -E "pspapp" | grep -v grep || echo "no pspapp ✓"
echo "--- adbd start time ---"
dmesg | grep "adbd" | head
echo "--- waved logs ---"
logread 2>/dev/null | grep waved | tail -30 || dmesg | grep waved | tail -30
echo "--- libapplogrs neutralized? ---"
ls -la /usr/lib/libapplogrs.so
echo "--- USB gadget config ---"
cat /sys/kernel/config/usb_gadget/amlogic/configs/amlogic.1/strings/0x409/configuration
ls /sys/kernel/config/usb_gadget/amlogic/configs/amlogic.1/ | grep -v -E "MaxPower|bmAttributes|strings"
'
```

### C3 pass criteria

1. ADB came back within ~15s of `adb reboot`
2. `waved` process is running, parent is the S60waved respawn loop or init
3. Zero `pspapp*` processes
4. `dmesg` shows adbd starting around uptime ~8s
5. USB gadget config: adb-enabled, **HID also present** (because pspapp
   isn't running to call `uac_adb` and drop HID — this is the visible
   improvement vs. the current state, where HID is dropped by pspapp's
   uac_adb call)
6. `/usr/lib/libapplogrs.so` is the empty overlay file
7. waved logs show expected startup banner and no panic loops

If any criterion fails: `./waved/deploy/revert.sh && adb reboot`.

## C4 — Slot A partition flash (deferred)

This section is the target architecture C1–C3 are building toward. Not
executed in this round.

### Approach: freeze the C2 overlay into slot A's SquashFS

The C2 overlay is a small, well-defined set of file diffs against the stock
SquashFS:

- `/etc/init.d/S60waved` — added
- `/etc/init.d/S60pspapp_daemon` — replaced with stub
- `/etc/init.d/S89usbgadget` — already replaced via the verified edit
- `/usr/lib/libapplogrs.so` — emptied
- `/opt/waved/` — entire dir (binary, config, iptables.rules) — note this
  moves from `/data/waved/` to `/opt/waved/` for C4, since `/data` is
  per-device state

For C4, build a new `/dev/system_a` SquashFS with all of those frozen in.
Slot A's kernel (`/dev/boot_a`) is **not** touched — it stays as backed-up
stock. This minimizes the diff: the bootable kernel + DTB are unchanged.

### Mechanical sketch

```
1. Mount the backed-up p11_system_a.img read-only on the host
2. rsync -a to a working directory
3. Apply the overlay diffs into the working dir
4. Bake /data/waved/ contents into /opt/waved/
5. Adjust S60waved to look in /opt/waved/ instead of /data/waved/
6. mksquashfs working/ slot_a_new.img
   (compression + block size MUST match the original)
7. md5sum slot_a_new.img — record for future verification
8. adb push slot_a_new.img /tmp/
9. adb shell 'dd if=/tmp/slot_a_new.img of=/dev/system_a bs=1M && sync'
10. Switch active slot via /dev/misc (format TBD)
11. adb reboot
12. Boots into slot A. Slot B remains untouched.
```

### TBDs to resolve before C4 can run

1. **mksquashfs parameters** — what compression and block size does the
   stock SquashFS use? Inspect via `unsquashfs -s
   backups/partitions/p11_system_a.img`.
2. **misc partition format** — what does the A/B state struct look like?
   Inspect `backups/partitions/p07_misc.img`. Compare against AOSP bootctl,
   Amlogic vendor docs, and the bootloader binary at
   `backups/partitions/p01_bootloader.img`.
3. **Bootloader fallback behavior** — if slot A doesn't boot N times, does
   the bootloader auto-switch back to slot B? Or do we need an explicit
   trigger? Test by intentionally flashing a broken slot once we trust the
   recovery path.
4. **dm-verity / signature verification** — `androidboot.verifiedbootstate=green`
   is in the kernel cmdline. Confirm whether the bootloader actually
   enforces a SquashFS hash check, and if so, whether it's permissive
   ("green" without enforcement) or strict.

### C4 rollback

- Slot B is untouched stock through all of C1–C3 and C4. The misc
  partition is the only thing that changed at the bootloader level.
- Recovery: rewrite misc to point at slot B → reboot → back to working
  stock.
- Worst case: restore `backups/partitions/p07_misc.img` from backup.
- Worst-worst case (misc wedged, bootloader broken): pyamlboot from
  MaskROM. Toolchain already verified.

## Risk register

| #  | Risk                                                                                              | Probability     | Impact                              | Mitigation                                                                                          | Recovery                                                          |
|----|---------------------------------------------------------------------------------------------------|-----------------|-------------------------------------|-----------------------------------------------------------------------------------------------------|-------------------------------------------------------------------|
| R1 | `waved` panics on real hardware (TEA SDK FFI mismatch, missing .so, etc.)                         | High — first run | Low — caught at C1c/C1d             | C1a host simulator first; C1c device hardware test before C1d daemon                                | `pkill waved`, debug, rebuild                                      |
| R2 | `waved` runs but mis-talks to MCU                                                                 | Medium          | Low–Medium — buttons unresponsive    | C1c hardware test exercises MCU                                                                      | `revert.sh + reboot`                                               |
| R3 | `install.sh` overlay writes silently fail                                                          | Low             | Medium — partial install             | C2 validation step                                                                                   | Manual cleanup + retry                                             |
| R4 | Empty `libapplogrs.so` breaks the dynamic loader for some other binary                            | Low–Medium      | Medium — boot may fail               | Pre-check: `grep -lr libapplogrs /usr/bin /usr/sbin`                                                 | `revert.sh + reboot`                                               |
| R5 | `S60waved` respawn loop hammers CPU if `waved` keeps crashing                                     | Medium          | Medium — CPU pinned                  | 3-second sleep in S60waved limits this; ADB still reachable via uptime-8s adbd                        | `adb shell killall waved && rm /data/overlay/upper/etc/init.d/S60waved && reboot` |
| R6 | After reboot, S89usbgadget overlay edit is rolled back (corruption) AND pspapp is stubbed out      | Low             | **High — total ADB loss**            | Verify md5 of overlay file before AND after C3 reboot                                                | UART (when soldered) or pyamlboot from MaskROM                     |
| R7 | `/var/log` or `/data` fills up from `logger -t waved` flooding                                    | Low             | Low–Medium — degraded over hours     | Check disk usage in C3 verification                                                                  | `truncate /var/log/messages`                                       |
| R8 | Device screen goes blank (no LVGL UI in waved Phase 1)                                            | High (will happen) | Cosmetic only                     | Pre-warned                                                                                           | None — expected                                                    |
| R9 | Cross-build fails (cross 0.2.5 / rustup detection issue)                                          | Medium — known   | Blocks C1b entirely                  | README workaround: install cross from git                                                            | Manual Docker build                                                |

**R6 is the riskiest single failure** because it has no in-band recovery
under C2/C3. Verifying the S89usbgadget overlay md5 immediately before
and after the C3 reboot is therefore mandatory, not optional.

## GO / NO-GO gates

### Gate 1 — Before C2

All of C1a/b/c/d passed cleanly. If any sub-step failed, do not run
`install.sh`.

### Gate 2 — Before C3 reboot

All five C2 validation checks pass (overlay files present, md5 correct on
binary + config + iptables, recordings dir exists). **Plus** the extra
check: confirm
`/data/overlay/upper/etc/init.d/S89usbgadget` md5 is still
`554342b3eceeda0d5899b84b2a80e345`. If C2's install touched it, that's a
new bug to investigate.

### Gate 3 — Before considering C3 "passed"

All seven C3 pass criteria met. If even one fails, run
`./waved/deploy/revert.sh && adb reboot` and return to a clean stock
state.

## Decision points (when to abort)

| Stage          | Trigger                                                          | Action                                                                                                  |
|----------------|------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------|
| C1a            | Panic, hang, or "obviously broken" output in host simulator       | STOP. Don't cross-build. Fix in source.                                                                  |
| C1b            | Build error                                                      | STOP. Fix toolchain or fix code.                                                                         |
| C1c            | Any FFI test fails                                               | STOP. Debug failing subsystem before touching the daemon.                                                |
| C1d            | No banner within 30s, or crash within 60s                        | STOP. `pkill waved`. Debug.                                                                              |
| C2 validation  | Any of the 5 overlay targets missing or wrong md5                 | STOP. **Do not reboot.** Debug `install.sh` manually.                                                    |
| C3 reboot      | ADB doesn't come back within 30s wall-clock                       | Wait 30s more. Then unplug/replug USB-C. Then escalate to UART or MaskROM (factory devmode no longer applies because pspapp is stubbed). |
| C3 verification | `pspapp` process found running                                  | STOP. The S60pspapp_daemon stub didn't take. `revert.sh + reboot`.                                       |
| C3 verification | `waved` not running                                             | Check S60waved logs via `logread` or `dmesg`. If unrecoverable: `revert.sh + reboot`.                    |
| C3 verification | dmesg shows adbd starting LATER than ~10s                       | The S89usbgadget overlay didn't apply. **R6 territory.** Investigate immediately, do not run further tests. |

## Open questions

None blocking C1–C3. C4 has the four TBDs listed in its section.

## References

- `waved/deploy/install.sh` — the C2 install script
- `waved/deploy/revert.sh` — the rollback script
- `waved/deploy/S60waved` — the init script that starts waved at boot
- `backups/README.md` — partition backup inventory and restore commands
- `docs/GUIDE_DEVICE_ACCESS.md` — ADB access methods, factory devmode details
- `docs/superpowers/specs/2026-03-19-waved-open-firmware-design.md` — overall waved architecture
- `docs/superpowers/plans/2026-03-19-waved-phase1-mvp.md` — Phase 1 MVP scope
