# Security and release lockdown (K1)

How `waved` keeps developer access to the device, and how an owner locks a unit
down for release. The hard rule for this project: **a development unit must
never lose access.** Lockdown is therefore opt-in, default off, and has a
documented, tested way back.

Related: `docs/GUIDE_DEVICE_ACCESS.md` (how root access is obtained on the Wave,
for research and recovery), `docs/WIFI.md` (the setup AP), `docs/UPDATES.md`
(paths back to stock).

## What runs today

Measured against the stock `fw_v1.2.2.6` rootfs and `waved/deploy/`.

| Surface | State under waved | Evidence |
|---|---|---|
| **adb** | Not controlled by waved. The stock USB gadget (`/etc/init.d/S89usbgadget`) boots in `start` mode = `uac2.0 + hid.0`; `adbd_enable` is **commented out** in that branch, so a fresh boot has **no adb**. adb exists only after the gadget is switched to `uac_adb` by the vendor HID command, factory devmode, or an `/etc` overlay edit. `deploy/install.sh` never switches it on. | `firmwares/fw_v1.2.2.6/extracted/rootfs/etc/init.d/S89usbgadget`, `docs/GUIDE_DEVICE_ACCESS.md` |
| **telnet** | Off. `/etc/inetd.conf` carries a telnet line but no init script starts `inetd`, and the `inittab` telnetd line is commented. | rootfs `etc/inetd.conf`, `etc/inittab` |
| **ssh / dropbear** | Absent from the rootfs. | — |
| **Inbound network** | `deploy/iptables.rules` sets INPUT policy DROP and accepts only loopback, established/related, DHCP and mDNS. Any daemon that did start is unreachable from the network. | `waved/deploy/iptables.rules` |
| **root password** | `/etc/shadow` is `$5$` (SHA-256-crypt). 1.2.2.6 shipped a **new** root hash (salt length 8, vs 10/11 in older builds; this is the TODO F6 note). waved stores no root hash of its own and never changes `/etc/shadow`. Older public builds' hashes cracked to a six-digit numeric password. | rootfs `etc/shadow` across versions |
| **Device PIN (K2)** | A per-owner PIN (`security.device_password`, a PBKDF2 record) gates Files, Wi-Fi, Bluetooth, Uploads and System on the touch UI. Never a root/shell credential. | `crates/waved-config/src/pin.rs` |

Takeaway: a default waved unit already exposes no inbound shell. The remaining
developer surface is **USB adb**, and it is enabled by the owner's own setup
(factory devmode or an overlay edit), not by waved.

## Release lockdown (opt-in)

`security.release_lockdown` (default **false**, `config/default.toml`,
`waved_config::release_lockdown_active`). It is a **config flag, not a build
feature**, on purpose: a build feature could not be turned off again without a
rebuild and reflash, which would risk the hard rule. A config flag is
reversible from the setup page over Wi-Fi, with no adb needed.

waved itself never disables adb and never edits the gadget script. The flag is
the owner's declared intent; a documented, opt-in `/etc` overlay step reads it
and keeps adb off (or authenticated) at boot. Nothing in this branch applies
that step automatically, and nothing disables adb at deploy.

### Applying lockdown (only after the way back is proven on the unit)

1. Decide and record how you will get back in (below) and test it first.
2. Set `security.release_lockdown = true` — on the setup page (Advanced →
   Release lockdown) or by hand in `config.toml`.
3. Apply the overlay step that enforces it. Because `/etc` is an overlay on
   `/data/overlay/upper/etc` (see `docs/GUIDE_DEVICE_ACCESS.md`), keep the stock
   `S89usbgadget`'s `start` branch with `adbd_enable` commented out (its
   default), so the gadget never brings adb up on its own. Do **not** set the
   factory-devmode sentinel and do **not** leave an overlay edit that enables
   `adbd` in `start`. Root password policy: before release, change the stock
   root password away from any published default (`passwd` on the device); the
   hash lives on the `/data` overlay `/etc/shadow` and never in this repo.
4. The firewall (`deploy/iptables.rules`) already blocks inbound shells; keep
   it in place.

### The way back (test before locking down)

Any of these restores developer access; the setup-page switch needs no adb:

- **Setup page switch (primary).** On the device, start Wi-Fi setup (System or
  the first-run flow), join the Wave setup network, untick **Release lockdown**,
  Save. Reachable with only physical access to the unit and a phone — no adb.
- **Revert to stock-style access.** `deploy/revert.sh` removes waved's overlay
  and `/data/waved`; a reboot brings stock `pspapp` back and with it the stock
  device-access paths in `docs/GUIDE_DEVICE_ACCESS.md`.
- **Overlay restore.** Restore the stock `S89usbgadget` (and remove any
  lockdown edit) under `/data/overlay/upper/etc/`; see `docs/UPDATES.md`
  "Paths back".
- **Serial UART.** Always-on root console on `ttyS0` (physical access);
  `docs/GUIDE_DEVICE_ACCESS.md`.

Never enable lockdown on a unit until at least the setup-page switch has been
shown to turn it back off on that unit.

## Rules for contributors

- Do not add code that disables adb by default or at deploy.
- Do not commit root password hashes, device PINs, or secrets. The PIN is
  stored only as a PBKDF2 record and never logged (`crates/waved-config/src/pin.rs`).
- Do not weaken `deploy/iptables.rules` or start an inbound shell service.
