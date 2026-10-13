# M3 K — Security and privacy (K1, K3, K4) + System → PIN row

Workstream K of milestone 3. Branch off `feat/m3`. Host-only TDD
(`cargo test -p <crate> --lib`); never `cross` or `scripts/linux-test.sh`,
never the device. Additive edits to hotspots only.

Other agents own USB storage, Bluetooth and buttons/LEDs in parallel. Never
touch `docs/npu`, `waved/scripts/npu`, `waved/tools/npu-compiler`,
`waved/tools/onnx-hybrid`.

## What runs today (findings)

- **adb**: not waved's. Stock `/etc/init.d/S89usbgadget` (rootfs of
  `firmwares/fw_v1.2.2.6/extracted`) brings the USB gadget up in `start` mode =
  `uac2.0 + hid.0`; `adbd_enable` is **commented out** in that branch, so a
  fresh boot has **no adb**. adb comes up only when something switches the
  gadget to `uac_adb` (the vendor HID command, factory devmode, or an `/etc`
  overlay edit) — see `docs/GUIDE_DEVICE_ACCESS.md`. waved's `deploy/install.sh`
  does **not** enable adb; it relies on whatever the owner set up.
- **telnet**: `/etc/inetd.conf` has a telnet line but `inetd` is never started
  by any init script, and `/etc/inittab`'s telnetd line is commented. No
  dropbear/sshd in the rootfs. waved's `deploy/iptables.rules` drops all
  inbound except loopback/established/DHCP/mDNS, so even if a daemon started it
  is unreachable over the network.
- **root password**: `/etc/shadow` is `$5$` (SHA-256-crypt). 1.2.2.6 shipped a
  **different** root hash from older builds (salt length 8 vs 10/11; TODO F6
  note confirmed). Older public builds cracked to `123456`
  (`docs/GUIDE_DEVICE_ACCESS.md`); waved keeps no root hash of its own.
- **PIN (K2)**: done. `security.device_password` is a PBKDF2 record
  (`waved-config/src/pin.rs`); `SettingsState.pin_enabled` mirrors whether one
  is set; the router gates Files/Wi-Fi/Bluetooth/Uploads/System on it.
- **K3 setup AP**: `ProvisioningService` (`waved-provision`) already bounds the
  AP with `PROV_TIMEOUT = 300 s`, mints a fresh `gen_passphrase()` per session,
  and a per-session CSRF token gates `/save` and `/save-connect`. hostapd.conf
  is written `0600` to `/tmp` and removed on teardown.
- **K4 privacy indicators**: all present. The resting/ambient screen shows the
  recording ring + timer while recording and a "Pre-recording" chip while
  `MeetingState.pre_recording_active`; the status pill leads with a teal dot on
  the same flag. **C12 is already done**: `waved-meeting` has a producer
  (`pre_recording_active()` + `publish_pre_recording()` on the meeting loop).
  Start/stop tones exist (`waved-system/src/tones.rs`, wired in `main.rs`) and
  honour `tones.enabled`. Mute silences the recording: `apply_mic_mute()` zeroes
  the capture frame in place (`waved-audio/src/lib.rs`) before it forks to the
  recorder and the uplink.

## Decisions

- **K1 is a config flag, not a build feature.** `security.release_lockdown`
  (default `false`). A build feature could not be turned back off without a
  rebuild and a reflash, which risks the hard constraint (never lose dev
  access). A config flag is reversible from the setup page over Wi-Fi with no
  adb. Cost if wrong: a flag that no init step reads is inert — acceptable,
  because enforcement is a documented, opt-in `/etc` overlay step, and waved
  itself must never disable adb (branch rule).
- **System → PIN row turns the PIN off after confirming (D10).** System is
  already PIN-gated, so reaching the row proves the owner knows the PIN;
  turning it off needs only a confirmation, no re-entry. Setting/changing a PIN
  stays on the setup page (D10: 4–6 digits set there; the device keypad is
  unlock-only). When no PIN is set the row reads "Off" and a tap explains where
  to set one. Cost if wrong: a one-tap confirm that clears the PIN — bounded by
  the gate that already protected System, and reversible on the setup page.

## Steps (TDD, each its own commit)

1. **Plan** (this file).
2. **K1 config flag** — `SecurityConfig.release_lockdown: bool` (serde default
   false, no migration needed); `config/default.toml`; a setup-form Boolean in
   the Advanced section (the way back); tests in `waved-config`
   (`cargo test -p waved-config --lib`).
3. **K1 doc** — `waved/docs/SECURITY.md`; link row in `waved/CLAUDE.md`.
4. **K3 tests** — in `waved-provision`: bounded-timeout constant, fresh
   passphrase per session, CSRF rejection at `serve()`. Note the bounded
   timeout + per-session rotation in `docs/WIFI.md`
   (`cargo test -p waved-provision --lib`).
5. **System → PIN row turn-off** —
   - `waved-state`: add `Command::ClearDevicePin` (additive).
   - `waved-settings`: `Change::ClearDevicePin` → clears `device_password`,
     sets `pin_enabled = false` (`cargo test -p waved-settings --lib`).
   - `waved-ui`: `ConfirmKind::TurnOffPin` (+ `guarded_by` System), its
     `spec`/`outcome` in `confirm.rs`, `SystemRow::Pin` in `system.rs`.
     Regenerate the `screen_system` and `screen_confirm_turn_off_pin` goldens
     with `UPDATE_GOLDEN=1` (`cargo test -p waved-ui --lib`).
6. **K4 findings** — record in `TODO.md` (C12 done, K4 audited, no gaps).
7. Update `TODO.md` K section status.

## Device-pass checklist

Run on the milestone-3 device pass. **Read-only first.** Anything that changes
access is done only with the owner present and has an undo.

Read-only (safe, no change):
1. `adb shell 'cat /etc/inittab | grep -n telnet'` — telnetd stays commented.
2. `adb shell 'pidof inetd telnetd dropbear sshd || echo none'` — expect
   `none` (no inbound shell daemon running).
3. `adb shell 'iptables -S'` — INPUT policy DROP; only lo/established/DHCP/mDNS
   accepted (matches `deploy/iptables.rules`).
4. `adb shell 'grep -n adbd_enable /etc/init.d/S89usbgadget'` — the `start`
   branch's `adbd_enable` is still commented (adb not on by default), unless
   the owner opted into the overlay step.
5. On the Wave: open **System**. With a PIN set, the keypad appears first
   (K2). Confirm the **PIN** row shows **On**; with no PIN it shows **Off**.
6. Resting screen while recording shows the ring + timer; with pre-recording on
   and not recording/muted it shows the "Pre-recording" chip (K4).
7. Start a recording: a rising start tone; stop it: a falling stop tone (when
   Alert tones is on). Mute mid-recording: the recording and the far end go
   silent (the recorded file is zero during the mute).

Changes access — only with the owner present, each reversible:
8. **Turn the PIN off on device** (System → PIN → confirm). Undo: set a new PIN
   on the setup page (start Wi-Fi setup on the device, connect, enter a PIN,
   Save).
9. **Enable release lockdown** (only if the owner asks): set
   `security.release_lockdown = true` on the setup page or in `config.toml`,
   then apply the documented `/etc` overlay lockdown step in `SECURITY.md`.
   **Undo / way back (test before enabling):** start Wi-Fi setup on the device,
   connect, untick **Release lockdown**, Save — reachable without adb. Or
   `deploy/revert.sh` to stock, or restore the `S89usbgadget` overlay. Never
   enable lockdown until the way back has been shown to work on this unit.
