# WiFi: full fix + phone/web provisioning — design

**Date:** 2026-06-04
**Status:** Approved (brainstorming) → ready for implementation plan
**Crates touched:** `waved-wifi`, `waved-provision` (new), `waved-state`, `waved-ui`, `waved`

## Problem

`waved` WiFi management is currently **read-only**: `WifiService` polls `wpa_cli
status` every 10 s and publishes `WifiState`, but `wpa::scan/connect/disconnect`
are **dead code** — nothing calls them, and there is **no on-device path to join
a network**. The device only connects to networks already saved in
`wpa_supplicant.conf`. The existing functions also carry real bugs (see §2), and
the UI has no text-entry capability at all.

This design (a) hardens the `waved-wifi` backend and (b) adds **phone/web
companion provisioning** so a user can join a new network without `adb`.

## Decisions (locked during brainstorming)

| Decision | Choice |
|---|---|
| Scope | Full provisioning — bug fixes + reachable API + on-device flow |
| Password entry | **Phone/web companion** (no on-device keyboard — circular OLED can't host one well) |
| Transport | **AP hotspot + embedded web server** (Approach A) |
| Trigger | **Wi-Fi row in System settings** (currently inert) |
| Provisioning AP security | **WPA2** with on-screen passphrase (stock ships open — we don't) |
| HTTP server | Hand-rolled minimal tokio listener (no new heavy dep) |
| Provisioning timeout | 5 min, then auto-revert to STA |
| SSID list source | Pre-AP scan (single radio can't reliably scan as AP) |

Single radio constraint: `wlan0` is either STA **or** AP, never both — so
provisioning is inherently a mode toggle, same as stock.

## End-to-end flow

1. User taps **Wi-Fi** row in System settings → `Command::OpenWifiProvisioning`.
2. `waved-wifi` **scans while still in STA mode**, caches the SSID list.
3. `waved-provision` tears down STA, brings `wlan0` up as **AP `Wave-XXXX`**
   (XXXX = last 4 of WiFi MAC) at `192.168.2.1`, starts `dnsmasq` (DHCP
   `.50–.200`) and the embedded HTTP server.
4. On-device `WifiProvisioning` screen shows: SSID `Wave-XXXX`, the WPA2
   passphrase, and `http://192.168.2.1`.
5. Phone joins, opens the page → form lists scanned SSIDs (dropdown) + password
   field (+ manual SSID for hidden networks). `POST /connect`.
6. `waved-provision` tears down AP/`dnsmasq`, calls the hardened
   `wpa::connect`, transitions back to STA, polls `wpa::status`. Page shows
   result; device screen shows connecting → connected/failed.
7. **Timeout**: if no creds within 5 min, AP auto-tears-down and returns to STA.

## §2 — Foundation: `waved-wifi` hardening (prerequisite)

On the critical path because provisioning *calls* `connect`.

- **`WifiError` enum** (using the currently-unused `thiserror` dep) — replaces
  `bool` / bare `io::Error` returns.
- **`wpa_cli()`** checks `output.status` **and** parses the `OK`/`FAIL` stdout
  token (wpa_cli exits 0 on logical failure). Returns `Result<String, WifiError>`.
- **`connect()`** → `Result<(), WifiError>`; validates `add_network` returns a
  numeric id; aborts on any failed step instead of returning unconditional `true`.
- **SSID/PSK escaping**: escape `"` and `\`, reject embedded newline, before
  quoting in `set_network`.
- **PSK redaction**: `wpa_cli` debug log masks the `psk` arg → `psk ****`.
  Closes the stock plaintext-password log leak; applies at every log level.
- **Real signal strength**: `status()` also reads `signal_poll` → populate
  `WifiState.signal_strength` (unblocks the status-bar icon).
- **Timeouts**: every `wpa_cli` / `ntpd` invocation wrapped in
  `tokio::time::timeout` (default 10 s) so a wedged binary can't freeze the loop.
- **Roam fix**: track last SSID (not just a `bool`) so an SSID change fires
  `WifiDisconnected` + `WifiConnected`.
- Extend existing `wpa.rs` unit tests: OK/FAIL parsing, escaping, numeric-id
  validation, signal parsing.

## §3 — New crate: `waved-provision`

Single-purpose; AP orchestration + HTTP, observable from the UI.

- **`ApController`** — shells to on-device `hostapd` + `dnsmasq`; writes runtime
  configs to `/tmp/` (mirrors stock `192.168.2.1`, DHCP `.50–.200`).
  `start()/stop()` idempotent; **always restores STA on drop/error**.
- **Embedded HTTP** — minimal hand-rolled tokio listener (no new heavy dep,
  matches the tokio-everywhere idiom). Routes: `GET /` (form with SSID list
  injected), `POST /connect` (parse `ssid`/`psk` → state machine). Single static
  HTML file, no JS framework. Bound to `192.168.2.1` only.
- **`ProvisioningService`** — owns the state machine, subscribes to
  `bus.commands`, publishes `ProvisioningState` to the bus.
  States: `Idle → Scanning → ApActive{ssid,pass,url} → Connecting →
  Connected | Failed{reason} → Idle`.

## §4 — `waved-state` additions

- `Command::{OpenWifiProvisioning, WifiConnect{ssid, password}, CancelWifiProvisioning}`.
- `ProvisioningState` enum + a `watch` channel on `StateBus` (separate from
  `WifiState` — it's a transient flow, not steady-state).
- `WifiService` gains a `bus.commands` subscription so `scan`/`connect` are
  reachable (eliminates the dead-code finding).

## §5 — `waved-ui` additions

- `UiScreen::WifiProvisioning`; new `screens/wifi_provisioning.rs` reusing the
  `SystemScreen` list/label idiom — **no new LVGL widgets** (labels + spinner +
  Cancel button only). Renders current `ProvisioningState`.
- Wire the **Wi-Fi row** in `system.rs` from `RowAction::Bookmark` →
  `RowAction::OpenWifiProvisioning`.
- Status-bar icon reflects real `signal_strength`.

## §6 — Security decisions (improvements over stock)

- **Provisioning AP is WPA2, not open.** Device shows an 8-char passphrase
  on-screen; `hostapd` uses it. Encrypts the link so the user's *home* WiFi
  password is not sniffable over the air during the plain-HTTP `POST`. Stock
  ships an open AP.
- **No PSK in logs** anywhere, any level (the §2 redaction).
- Provisioning HTTP bound to `192.168.2.1`; server lifetime == AP lifetime.

## §7 — Prerequisites / risks

- **Probe required (one open unknown):** confirm `hostapd` and `dnsmasq` exist in
  the device rootfs (`wpa_cli` already proven present). If absent → bundle static
  binaries or fall back to Approach C (waved writes configs, init.d runs daemons).
  Verify on-device **before** coding the AP layer.
- Single-radio STA↔AP handoff timing on the ATBM6031 is the riskiest moment; the
  state machine restores STA on every error path.

## §8 — Testing

- **Unit:** `wpa.rs` parsing/escaping/error-mapping; HTTP request parsing;
  state-machine transitions (table-driven).
- **On-device acceptance:** `scripts/test-wifi-provisioning.sh` (follows existing
  `scripts/test-*.sh` convention): trigger → assert AP up + page reachable → POST
  creds → assert STA reconnect.

## §9 — Docs (same-PR rule)

- New `waved/docs/WIFI.md`: provisioning state machine, AP network layout,
  single-radio constraint, on-device daemon dependencies (reverse-engineered /
  measured detail per `waved/CLAUDE.md`).

## Out of scope (YAGNI)

- Captive-portal auto-redirect (user browses to `192.168.2.1` manually for v1).
- BLE GATT provisioning (Approach B).
- On-device keyboard / arbitrary on-device text entry.
- WPS / 802.1X / EAP (device is WPA2-PSK only).
