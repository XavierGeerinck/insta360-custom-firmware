# Insta360 Wave — On-Device Backup Inventory

Captured via ADB root shell on **2026-04-08** from `PSP_0000000000000000` (serial `PSPLB0000AAAAA`), running firmware **`PSP_1.1.6.6`** (build 2025-1128-1949), kernel `Linux buildroot 5.4.242-ab575 aarch64`, hardware `amlogic`, bootloader `01.01.251002.224553`, slot `_b` active.

This backup was taken **immediately after** a forced factory reset triggered by an accidental HID `0xEE` command (see `docs/GUIDE_DEVICE_ACCESS.md` → "Known Firmware Quirks and Incidents"). The device was in a fresh post-setup state: factory devmode was active (`unifykeys[3] = 0x9257`), `/data` had been wiped and partially repopulated by first-run setup, `/mnt/udisk` had been reformatted, and the device was unbound from Insta360 cloud.

## Partition inventory

All images were captured from the running device with `dd if=<dev> of=/tmp/part.img bs=1M`, md5-summed device-side, pulled with `adb pull`, and the host-side md5 was confirmed to match. If a host-side hash drifts from what's listed here, the backup file is corrupted — do not flash it.

| File | Size | Device | Major:Minor | MD5 | Purpose |
|---|---|---|---|---|---|
| `partitions/p01_bootloader.img` | 4 MiB | `/dev/bootloader` | 179:1 | `ea1d8e8607dc94ea80fae86f5e6014ff` | U-Boot (primary) |
| `partitions/p02_raw.img` | 64 MiB | `mmcblk0p2` (no alias) | 179:2 | `0361174759541518fed454174cb836c7` | Reserved |
| `partitions/p04_env.img` | 8 MiB | `/dev/env` | 179:4 | `5ad246331fdd7875d47e41994ff1f71b` | U-Boot env (primary) |
| `partitions/p05_raw.img` | 8 MiB | `mmcblk0p5` (no alias) | 179:5 | `96995b58d4cbf6aaa9041b4f00c7f6ae` | Env backup (zero-filled on this unit — matches p08) |
| `partitions/p06_raw.img` | 8 MiB | `mmcblk0p6` (no alias) | 179:6 | `6e60d20525ba58468d030435acb099eb` | Unknown |
| `partitions/p07_misc.img` | 8 MiB | `/dev/misc` | 179:7 | `cffb5ef85514d519b30efe9f9aecef98` | A/B slot state |
| `partitions/p08_raw.img` | 8 MiB | `mmcblk0p8` (no alias) | 179:8 | `96995b58d4cbf6aaa9041b4f00c7f6ae` | Zero-filled on this unit — matches p05 |
| `partitions/p09_boot_a.img` | 64 MiB | `/dev/boot_a` | 179:9 | `beb9f8785e659fbff80ae3b38d8f326f` | Kernel slot A (inactive — this image can be used to flash custom firmware without touching the active slot) |
| `partitions/p10_boot_b.img` | 64 MiB | `/dev/boot_b` | 179:10 | `37322e051e8d5083158dd61c2a4beb80` | Kernel slot B (**active**) |
| `partitions/p11_system_a.img` | 512 MiB | `/dev/system_a` | 179:11 | `a58f20aee5029d397640199e3eef632e` | Rootfs slot A SquashFS (inactive) |
| `partitions/p12_system_b.img` | 512 MiB | `/dev/system_b` | 179:12 | `400fa6bce47e2fb74dcfb846043eaf42` | Rootfs slot B SquashFS (**active**) |
| `partitions/p13_data.img` | 512 MiB | `/dev/data` | 179:13 | `8793b3687b504a973a7174f27a78062c` | `/data` ext4 — post-reset minimal state (overlay dirs, fresh setup) |
| `partitions/p15_unifykey.img` | **64 MiB** | `/dev/no_use` | 179:15 | `5445a506cb431edbec5b42b903e26d6a` | **Hardware-backed secrets partition. IRREPLACEABLE.** Contains `usid` (serial), `mac`, `mac_bt`, `mac_wifi`, `bindkey`, `devmode` (= `0x9257` factory magic at time of capture), TEA SDK license key (`oemkey`), etc. Do NOT share this file publicly — it contains device-specific cryptographic material. |
| `partitions/p17_psp_udisk_s.img` | 512 MiB | `/dev/psp_udisk_s` | 179:17 | `33f01343f339b366a2519b7a5e8abd65` | `/udisk_s` system temp files (including `Insta360_Wave_Controller/` installer backed up during reset) |
| `partitions/p18_psp_model_s.img` | 1.2 GiB | `/dev/psp_model_s` | 179:18 | `c860a7d34af6fb44c088e61d6fe24ae9` | `/tmp/psp_model` model staging (OTA scratch area) |

**Not backed up (intentional):**
- `mmcblk0p14` `/dev/model` (4.6 GiB ext4 `/model`) — AI models. Already extracted to `docs/ai-models/` per `REPORT_DEVICE_DUMP.md`. Also recoverable from `software_model_1_1_6_6.swu`.
- `mmcblk0p16` `/dev/udisk` (21 GiB exFAT `/mnt/udisk`) — user recordings partition. Was wiped to `Insta360_Wave_Controller` only by the incident; 145 MiB used at capture time. Not worth 21 GiB of backup space.
- `mmcblk0boot0`/`mmcblk0boot1` (eMMC hardware boot partitions) — typically hold a secondary bootloader copy. Not captured this round; can add in a future pass.
- `mmcblk0rpmb` — Replay Protected Memory Block (secure, not readable via standard dd).

## Restoring a partition

From ADB shell on a device that is still booting and has `/data` accessible:

```bash
# Example: restore bootloader
adb push partitions/p01_bootloader.img /tmp/restore.img
adb shell 'md5sum /tmp/restore.img'   # should match: ea1d8e8607dc94ea80fae86f5e6014ff
adb shell 'dd if=/tmp/restore.img of=/dev/bootloader bs=1M && sync'
adb shell 'rm /tmp/restore.img'
```

For partitions with no `/dev/` alias (p02, p05, p06, p08), use `mknod` first:

```bash
adb shell 'mknod /tmp/p5 b 179 5'
adb push partitions/p05_raw.img /tmp/restore.img
adb shell 'dd if=/tmp/restore.img of=/tmp/p5 bs=1M && sync'
adb shell 'rm /tmp/p5 /tmp/restore.img'
```

**Never restore `p15_unifykey.img` to a different physical device** — it contains device-specific serials, MACs, and the TEA SDK license blob that is bound to this unit's hardware. Restoring to a different unit will cause serial-number collisions and may void the TEA license.

If the device won't boot and you cannot reach ADB, you need Amlogic USB MaskROM mode (`pyamlboot`) or UART-level U-Boot access (`mmc write` / `ums`). That recovery path is outside the scope of these backups — see `docs/GUIDE_DEVICE_ACCESS.md` → "UART Serial Console" and the Amlogic mask-rom section.

## `rootfs_etc/` — Captured rootfs text files

Pulled from the live `/etc` (which is overlayfs: read-only SquashFS + writable upper at `/data/overlay/upper/etc`). These files are text copies, not images — they're here for grepability and quick reference without mounting the SquashFS.

- `init.d/` — All of `/etc/init.d/` (13 files). Most important: `S89usbgadget` (the USB gadget mode switcher), `S60pspapp_daemon`, `S02overlayfs`, `rcS`.
- `inittab` — BusyBox init config. Confirms `ttyS0::respawn:-/bin/sh` (root serial console if you solder UART).
- `fstab` — mount table.
- `sw-versions` — JSON firmware version manifest.
- `shadow`, `passwd` — user/password database. Live device had MD5-crypt (`$1$`) whereas the firmware image has SHA-256-crypt (`$5$`). Runtime overlay must be rewriting this. Password was `123456` across all known versions.
- `datamount` — data partition mount/unmount script.

## `state/` — Captured runtime state

- `device_state.txt` — single dump of `/proc/cmdline`, `/proc/partitions`, `mount`, `/proc/cpuinfo`, `lsmod`, all unifykeys values (hex), `ps aux`, `ifconfig`, and the current USB gadget configuration (which functions are symlinked into the active config and which are available). Useful as a "this is what normal looks like" reference.

## Incident context

This backup was captured in response to the 2026-04-08 destructive HID `0xEE` probe (see `docs/GUIDE_DEVICE_ACCESS.md`). After that incident, the lesson is: **back up before probing.** This file is the start of a "defense in depth" policy for the Wave — every future modification should first verify the relevant partition's hash against this backup, and on any unexpected change, the backup gives us a recovery path.
