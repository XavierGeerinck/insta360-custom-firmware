# Updates (self-hosted, signed)

Workstream H: waved updates itself from a server its owner runs. Nothing talks to
Insta360 (`../TODO.md`, "Not planned"). Code: `crates/waved-update`; screen:
`crates/waved-ui/src/screens_v2/ota.rs`; fall-back: `deploy/S60waved`.

**Status: host-tested only.** No part of this has run on a Wave yet. The checks a
device pass must make are under [Device checks](#device-checks).

Paths are relative to `waved/`; `../` is the repository root.

## Owner's quick start

```bash
# 1. Once: a key pair. The private key stays outside the repo (waved-pack refuses a
#    path inside a git work tree) with mode 0600.
cargo run -p waved-update --bin waved-pack -- keygen ~/.config/waved/update.key
#    -> prints the public key; keep it, e.g. in your shell profile:
export WAVED_UPDATE_PUBKEY=<64 hex characters>

# 2. Every release: bump `version` in waved/Cargo.toml (equal is "up to date"),
#    build with the key compiled in, strip.
WAVED_BUILD_ID=$(git rev-parse --short HEAD) cross build --release \
  --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
$(rustc --print sysroot)/lib/rustlib/aarch64-apple-darwin/bin/llvm-objcopy --strip-all \
  target/aarch64-unknown-linux-gnu/release/waved /tmp/waved

# 3. Pack and sign. --include adds deploy files; keep deploy/S60waved (see below).
cargo run -p waved-update --bin waved-pack -- pack --key ~/.config/waved/update.key \
  --waved /tmp/waved --include deploy/S60waved --out /srv/wave-updates
#    -> waved-<version>.wup and latest.json

# 4. Serve the directory over HTTP(S) and point the Wave at it (setup page, Advanced):
#    security.ota_server = "http://updates.lan/wave"
```

`waved-pack inspect <file.wup> --pubkey <hex>` shows a package's manifest and checks
its signature. The first build that carries the key has to reach the Wave the old way
(adb, `docs/DISPLAY_HARDWARE.md#deploy`); from then on updates go over the air.

Wave installs a package only if the `S60waved` that will run at the new version's first
boots has the fall-back (`update_guard`, [below](#install-and-fall-back-trial-starts)):
the package's own when it carries one, otherwise the one on the Wave. A Wave whose
`/etc/init.d/S60waved` came from `main`, or that got only the binary over adb, has none and
refuses with "Wave can't go back if this update fails". Pack with
`--include deploy/S60waved` (or push it with `deploy/install.sh`) and that Wave updates.

`Cross.toml` passes `WAVED_UPDATE_PUBKEY` into the build container. A build without it
trusts no key: its System row reads "Off" and it can only be replaced over adb.
`waved-pack pack` refuses a binary that does not embed the signing key's public half, so a
release can never lock its owner out.

## Package (`.wup`)

All integers little-endian.

| Offset | Size | Content |
|---|---|---|
| 0 | 8 | magic `WAVEDUP1` |
| 8 | 4 | header length H, 1..=64 KiB |
| 12 | H | header: the manifest, UTF-8 JSON |
| 12+H | 64 | ed25519 signature over bytes 0..12+H |
| 76+H | … | each file's gzip bytes, in manifest order, back to back, nothing after |

```json
{"format": 1, "version": "0.10.0", "build": "374015f", "created_unix_s": 1791815358,
 "files": [{"target": "waved", "size": 63000000, "sha256": "…",
            "stored_size": 22000000, "stored_sha256": "…"}]}
```

- The signature covers the header; the header holds every file's stored and unpacked
  SHA-256 and size, so one signature binds every byte.
- Verification is two passes (`format::verify_and_stage`): first every stored byte is
  hashed against the signed header (and nothing may follow), then the trusted bytes are
  unpacked into the staging directory with size and content checked. Nothing untrusted
  reaches the decompressor.
- `waved` is required. The other targets are a closed list with fixed device paths; a
  package cannot name a path:

| Target | Device path | Mode |
|---|---|---|
| `waved` | `/data/waved/waved` | 755 |
| `S60waved` | `/etc/init.d/S60waved` | 755 |
| `S03splash` | `/etc/init.d/S03splash` | 755 |
| `splash.fb` | `/data/waved/splash.fb` | 644 |
| `iptables.rules` | `/data/waved/iptables.rules` | 644 |
| `rsyslog.conf` | `/etc/rsyslog.conf` | 644 |

`/etc` is an overlay whose upper dir is `/data/overlay/upper/etc`, so writes there persist
(`../docs/GUIDE_DEVICE_ACCESS.md`, "Overlay layout"). **No rootfs, kernel, partition or
bootloader changes:** the root is a read-only SquashFS slot, and writing slots is what
the stock updater does (see [Going back to stock](#going-back-to-stock-h4)).

## Server index (`latest.json`)

Wave fetches `<security.ota_server>/latest.json` (at most 16 KiB):

```json
{"format": 1, "version": "0.10.0", "build": "374015f", "package": "waved-0.10.0.wup",
 "size": 23456789, "sha256": "<hex of the whole .wup>", "installed_size": 63000000}
```

`package` is relative to the index or an absolute http(s) URL. The index is **not
signed**: it only decides whether to download. The package's signature decides whether
to install, and its signed version and build must equal the index's. A hostile server can
make Wave download junk that it then rejects, which a hostile network can do anyway.

## Versions

`MAJOR.MINOR.PATCH`, compared as numbers. The running version is the workspace version;
`waved-pack` reads the package version out of the binary (`waved-version:` marker), so the
two cannot disagree.

| Offered vs running | Result |
|---|---|
| newer | installs |
| equal | up to date (with `security.ota_allow_downgrade = true` and two different known build ids: reinstalls) |
| older | refused, unless `security.ota_allow_downgrade = true` in config.toml |

The downgrade switch lives on the device, not on the server or in the package, so an
attacker who controls the server cannot replay an old signed package. It is not on the
setup page.

## What Wave does

`UpdateService` (`crates/waved-update/src/service.rs`), started by `main.rs` through
`device::spawn_device`:

1. **At start:** deletes what an earlier update left behind (the staging directory, which
   a restart while Waiting leaves full, and half-written `.<name>.new` copies), reads the
   trial bookkeeping (below) and reports a rollback or an interrupted install as a
   failure. This runs even with updates off.
2. **Checks** 2 min after start, then every 6 h; System → Firmware update checks at once.
   An automatic check that cannot reach the server stays quiet; a tapped one fails
   visibly. No check or install, tapped or automatic, runs while `update/trial` exists:
   while this version is on trial, or after an install whose restart failed (step 6).
   Installing on top of an armed trial would make `.waved.prev` the new binary, and
   undoing it as an "interrupted install" would leave the new binary with no trial and no
   fall-back. An automatic check that finds a skipped version while a failure about that
   same version is on show leaves the failure up instead of showing Available.
3. **Downloads** to `/tmp/waved-update/` (tmpfs, RAM; `/data` is small). An interrupted
   download resumes with an HTTP `Range` request; `package.sha256` next to it keeps a part
   of one package from being continued with another. A server that ignores ranges starts
   the file over.
4. **Verifies:** whole-file SHA-256 against the index, then the signature, then the signed
   version against the index and the version rule, then (with any old staging deleted
   first) room on `/data`, then unpacks to `/data/waved/update/staged/` (same filesystem as
   the binary).
5. **Waits** until Wave is free (`gate.rs`): no recording or pause, no "Send this
   recording?", no call or ringing, no playback, no Wi-Fi setup, battery ≥ 30 % or USB
   connected. An automatic install also waits while the computer has the microphone open; a
   tapped one does not (some apps keep it open all day).
6. **Installs** (`install.rs`, below), shows "Restarting" for a second, then runs
   `/sbin/reboot`. If the reboot fails the update is installed and its trial armed, so
   the phase becomes `RestartNeeded` ("Restart Wave"), not a failure with Retry: the new
   version starts when the user turns Wave off and on, or when waved next exits (the
   watchdog runs the guard, then the new binary). If the installer itself finds a trial
   pending (`TrialPending`), it changes nothing and the service shows the same state; it
   never undoes that trial.

`OtaState.progress_pct`: download 0–90, verify and unpack 90–98, waiting 98, installing 99,
restarting 100.

### Space

`/data` is 512 MiB with ~440 MB usable and nearly full; the stripped binary is ~63 MB
(`docs/DISPLAY_HARDWARE.md`, "Deploy"). The download (gzip, smaller than the binary) goes
to RAM. Staging needs the unpacked size plus 16 MiB on `/data`, measured after any old
staging is deleted; if that is short the update fails with "Not enough space on Wave"
(`space.rs`). Nothing is deleted to make room: the `.prev` copies are the last known good
version, and the next install replaces them anyway (step 2 below). After an install `/data`
holds the new binary and `.waved.prev`, which is the old binary's own inode (a hard link,
no copy), so the peak is three binaries: the live one, `.waved.prev` and the staged one.

## Install and fall-back ("trial starts")

`FsInstaller` (`install.rs`), in this order so a power cut at any point leaves a Wave that
starts:

1. `sh -n` on every staged init script; a syntax error installs nothing. Then the fall-back
   check: the package's `S60waved` if it carries one, else `/etc/init.d/S60waved`, must
   contain `update_guard`, or nothing is installed (`NoFallBack`; automatic checks then
   pass over that version, a tapped Retry tries again).
2. Hard-link every file about to be replaced to `.<name>.prev` beside it (copy if linking
   fails), and copy `/data/waved/config.toml` to `.config.toml.prev` (mode 600; a copy, so
   a save during the trial cannot reach it).
3. Write `/data/waved/update/trial` (tmp file, fsync, rename) and clear `update/starts`.
4. Replace the deploy files: copy to `.<name>.new` in the same directory, fsync, rename.
5. Rename the staged binary over `/data/waved/waved`.
6. `sync`. The service then reboots.

Every copy beside a file has a **dot name** (`/etc/init.d/.S60waved.prev`,
`/etc/init.d/.S60waved.new`, `/data/waved/.waved.prev`; `Paths::prev`, `Paths::new_copy`).
Stock `/etc/init.d/rcS` runs every `/etc/init.d/S??*` with `start` at boot and `rcK` runs
them with `stop`: an `S60waved.prev` would start a second watchdog loop (two waveds
fighting over the devices, each start counted twice), and a half-written `S60waved.new`
left by a power cut would be run. When the service starts it deletes any `.<name>.new` a
cut install left.

`update/trial` is plain lines so busybox `sh` can read it:

```text
version 0.10.0
build 374015f
previous 0.9.3
restore /etc/init.d/S60waved
restore /data/waved/waved
restore /data/waved/config.toml
remove /data/waved/splash.fb
```

`config.toml` is listed like the files the update replaces (`remove` when there was none),
because every config section denies unknown fields: an older waved rejects a file a newer
one saved, and its `.bak` too, and runs on defaults (first-run screen, PIN off, API token
and uploads gone). **Settings changed during a trial are lost on a rollback**: the old
waved gets the settings it had when the update was installed.

While it exists, the watchdog loop in `deploy/S60waved` (`update_guard`) appends a line to
`update/starts` before every start. On the **fourth** start it moves each `restore` file
back from its `.<name>.prev` (`${path%/*}/.${path##*/}.prev`), deletes each `remove` file,
moves `trial` to `update/rolled-back` and starts the old waved. Three starts, because
`S60waved` SIGTERMs the first waved after every boot at 5 s (display workaround): one start
always "fails".

The new waved **commits** (deletes `trial`, `starts` and `.config.toml.prev`) once it has
run 60 s with `AudioState::capture_healthy` true; until then it starts no other update.
The old waved finds `rolled-back` at its start, reports "Version 0.10.0 didn't start, so
Wave went back" and adds the version to `update/skip`, which automatic checks pass over; a
tapped Retry or Install clears it. If waved finds a `trial` that is not its own version,
the install was cut between steps 3 and 5: it puts the deploy files and the settings back
and reports "The update stopped before it finished".

`.prev` files stay after a commit as the last known good version, until the next install
replaces them with the version it replaces.

## What the user sees

System → Firmware update (system.png, centred when System opens) shows Off, Check now,
Checking, Up to date, Available, 42 %, Ready, Installing, Restart or Failed. A tap checks
when there is nothing to install, installs and opens the update screen when a version is
Available, and otherwise only opens the update screen. After a failure that screen says
why before anything installs again: a row tap must not quietly reinstall a version that
rolled back.

The update screen (ota.png) is the dial as a progress ring with "Installing update", the
percentage, "waved 0.10.0", "Keep Wave plugged in" and "It restarts by itself when done", and
nothing to tap. Installing and Restarting take over the screen from anywhere
(`kit/router.rs`); only power off overrides it. A failure stays on screen as "Update
failed", "Wave is still on 0.9.3", the reason, Retry and Back. A failed reboot shows
"Restart Wave", "waved 0.10.0 is installed", "Turn Wave off and on to finish" and only
Back (`update_restart_needed` golden).

| Failure | Shown reason |
|---|---|
| server unreachable (tapped check) | Can't reach the update server |
| index malformed | The update server's list is broken |
| download cut | The download stopped |
| hash mismatch, larger than announced | The download was damaged |
| signature fails | The update isn't signed for this Wave |
| malformed package | The update package is broken |
| signed version or build differs from the index | The update doesn't match the server's list |
| signed version not allowed | The update is older than this version |
| no room | Not enough space on Wave |
| no `update_guard` in the `S60waved` the trial would boot with | Wave can't go back if this update fails |
| installer error | The update could not be installed |
| cut between trial and swap | The update stopped before it finished |
| new version never committed | Version X didn't start, so Wave went back |

## Decisions

| # | Decision | Cost if wrong |
|---|---|---|
| U1 | Own `.wup` format (signed JSON header, gzip files) instead of tar or swupdate's CPIO | Needs `waved-pack inspect` to look inside. |
| U2 | Closed target list: waved and the `deploy/` files; no rootfs, kernel or partitions | Anything else still needs adb. A bad `S60waved` breaks the fall-back itself; `sh -n` catches syntax only, and the `update_guard` check only that the word is there. |
| U3 | Unsigned index, signed package | A hostile server can waste a download. |
| U4 | Version from the binary; downgrades only by the device's own switch | Every release must bump the workspace version. |
| U5 | Download to `/tmp` (RAM), stage on `/data` | A reboot mid-download starts it over. |
| U6 | File-level fall-back in `S60waved`, 3 starts, commit after 60 s with capture healthy | A binary that hangs without exiting is rolled back only after three power cycles. A Wave whose microphones fail for unrelated reasons never commits and rolls back after three reboots. |
| U7 | Wait until free; manual installs ignore "computer listening" | An app that holds the microphone open blocks automatic installs. |
| U8 | `/sbin/reboot` after install | Unverified; a failure shows "Restart Wave" with nothing to retry (the trial is already armed) and blocks further installs until Wave restarts. |
| U9 | Public key from `WAVED_UPDATE_PUBKEY` at build time; `waved-pack` refuses binaries without it | A build made without the key can be replaced only over adb. |

## Device checks

Not verified on a Wave. Before trusting this, a device pass should check:

1. `update_guard` under busybox `sh` (`read -r`, `$((…))`, `mv -f`): run
   `WAVED_DIR=/tmp/g sh /etc/init.d/S60waved guard` against a fake trial in `/tmp/g`.
2. Hard links and renames through the `/etc` overlay: `ln /etc/init.d/S60waved
   /etc/init.d/.S60waved.prev`, then a `.S60waved.new` + rename, then a reboot to see both
   persist in `/data/overlay/upper/etc/init.d` and that rcS starts one waved, not two
   (`ps | grep -c '[w]aved$'`).
3. `/sbin/reboot` from waved runs the rcK stop scripts and the unit comes back.
4. Free space on `/data` and `/tmp` (`df -h`) against a real package; `statvfs` figures.
5. The whole cycle against a LAN server (`python3 -m http.server`), including Range resume
   (kill Wi-Fi mid-download) and a forced rollback (a package whose binary exits at once).
6. The 60 s commit: `update/trial` disappears about a minute after the second start.
7. rustls against the owner's HTTPS server (certificate roots are compiled in by
   reqwest's `rustls-tls`; a self-signed server needs plain HTTP on the LAN, which the
   signature makes safe).

## Device facts used here

| Fact | Evidence |
|---|---|
| eMMC `mmcblk0`; `system_a`/`system_b` 512 MiB SquashFS, mounted read-only as `/` | `../docs/GUIDE_DEVICE_ACCESS.md:457-480`; `../backups/state/device_state.txt:29-48` (main checkout only, gitignored) |
| `/data` = `mmcblk0p13`, ext4, 512 MiB; overlay upper dirs for `/etc`, `/opt`, `/usr` | same; `../firmwares/fw_v1.2.2.6/extracted/rootfs/etc/init.d/S02overlayfs` (main checkout only) |
| A/B: `boot_a/b`, `system_a/b`; slot state in `misc` (AvbABData "AB0": priority, tries, successful_boot); U-Boot `bcb_cmd=…get_valid_slot` | `../backups/partitions/p07_misc.img`, `p04_env.img` (main checkout only) |
| `/tmp` tmpfs ~1.9 GiB; RAM 3.8 GB | `../docs/REPORT_DEVICE_DUMP.md:26-27`; `../docs/npu/lab-notebook/2026-07-28-poc3-runtime-driver-trace.md:191` |
| waved lives in `/data/waved`; `S60waved` in the `/etc` overlay; the SquashFS is never modified | `deploy/install.sh` |

## Going back to stock (H4)

Research only: **nothing here has been tested**, and the slot-writing paths flash
partitions, which waved's own updater never does. Firmware files are gitignored and live in
the main checkout's `firmwares/` (`../firmwares/README.md`).

### How stock installs a `.swu`

- The vendor container is a 4-byte magic (`!C#\x01` consumer, `1BOT` business), a 1 MiB
  JSON header, then a swupdate CPIO at `0x100004` (`../firmwares/README.md`, "Container
  format").
- `pspapp` (`libins_entry.so`, decompiled in `../firmwares/fw_v1.2.2.6/decompiled/`):
  `extract_and_install_package` copies the CPIO to `/tmp/.software.swu` (~line 147325) and
  runs `swupdate -i /tmp/.software.swu -k /etc/swupdate-public.pem` (~147620);
  `SW_Slot_Switch()` runs `swupdate -s` (~147992); then it wipes `/data`:
  `cd /data && ls /data | grep -v overlay | grep -v common_settings | xargs rm -rf` and
  `cd /data/overlay/upper && ls /data/overlay/upper | grep -v opt | xargs rm -rf`
  (~45729), and has the MCU reboot with a slot change (~45712).
- Sources it installs from: its online download (Insta360 cloud, out of scope),
  `/mnt/udisk/psp.swu` dropped over USB storage (~168789), and the PC client.
- `/usr/bin/swupdate` is Amlogic's build: it finds the active slot from `/proc/mounts`,
  writes `rootfs.squashfs` to the *inactive* `/dev/system_*`, `boot.img` to the inactive
  boot slot, `dtb.img`, `udisk_s.exfat` to `/dev/psp_udisk_s`, and `MCU.bin`, `BT.bin`,
  `DONGLE.bin` to `/tmp/insta360/` (`sw-description` in each `extracted/` directory).
  Packages are signed (`sw-description.sig`, 512-bit RSA, `/etc/swupdate-public.pem`).
  swupdate refuses older versions only with `-N`, which stock does not pass, and every
  `sw-description` says `version = "1.0.1"`.

### Paths back, least invasive first

1. **Soft revert (normal path).** waved never touches the SquashFS slots, so the unit still
   runs the stock rootfs it had. `deploy/revert.sh` removes waved's overlay files
   (`S60waved`, the `S60pspapp_daemon` stub, the `libapplogrs.so` stub, update `.prev`
   copies) and `/data/waved`; after a reboot `pspapp` runs again. Not removed by it,
   because they were installed by hand: `S03splash`, `/etc/rsyslog.conf`, the
   `S89usbgadget` patch (`scripts/patch-stock-usbgadget.sh`). Remove those from
   `/data/overlay/upper/etc/` too for a clean stock `/etc`.
2. **Stock updater, after a soft revert.** With `pspapp` back, copy an original vendor
   package (container included, e.g. `software_model_1_1_6_6.swu`) to the recordings
   volume as `psp.swu` (`/mnt/udisk/psp.swu`) and let `pspapp` install it. It writes the
   inactive slot, switches, and wipes `/data` as above. Untested: whether the USB drop
   needs a UI confirmation, and whether `MCU.bin`/`BT.bin` from an older package downgrade
   the MCU or Bluetooth firmware.
3. **Manual swupdate over adb.** Run `revert.sh` first (a manual install does not wipe
   `/data`, so waved's overlay would keep replacing stock on the new slot). Cut the CPIO
   from `0x100004` into `/tmp` (for 1.2.2.6, concatenate `Insta360WaveFW_model_swu_only.bin`
   and `tail_extra.bin` first: the CPIO runs past `bin_size`,
   `../firmwares/fw_v1.2.2.6/CHANGELOG.md`), then
   `swupdate -i /tmp/x.swu -k /etc/swupdate-public.pem && swupdate -s && reboot`. Check
   `cat /etc/hwrevision` first: `sw-description` requires hardware `1.0`, and
   `S99_PSPLauncher` overwrites `/etc/hwrevision` in the overlay on every boot with `NONE`
   or an ADC index, which may fail swupdate's compatibility check.
4. **Raw restore.** `../backups/README.md` restores captured partitions with `dd`; `misc`
   (`p07_misc.img`) puts the slot choice back. Last resort: U-Boot's `update`
   (`usb_burning`, `recovery_from_udisk`), Amlogic MaskROM (`pyamlboot`) or UART.

Going back to waved afterwards: `deploy/install.sh` again (after the stock updater's wipe,
`/data/waved` is gone).
