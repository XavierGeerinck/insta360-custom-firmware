# Wi-Fi & Provisioning

How `waved` manages Wi-Fi: station-mode monitoring/connection, and on-device
phone/web provisioning. Design spec:
[`docs/superpowers/specs/2026-06-04-wifi-provisioning-design.md`](superpowers/specs/2026-06-04-wifi-provisioning-design.md).

## Hardware / stack

- **Chipset:** ATBM6031 (ATBM613x family) over **SDIO**, dual-band 2.4/5 GHz.
  Kernel modules: `aml_wifi.ko` (Amlogic glue) → `atbm613x_wifi_sdio.ko`,
  exposed to userspace via **nl80211**.
- **Userspace tools** (provided by the stock rootfs, reused by `waved`):
  `wpa_supplicant`, `wpa_cli`, `wpa_passphrase`, `udhcpc`, and — for AP mode —
  `hostapd` + `dnsmasq`.
- Single radio: `wlan0` is **either STA or AP**, never both. Provisioning is a
  mode toggle.

> README note: the top-level `README.md` once listed "RTL8733BU" for Wi-Fi.
> That is incorrect — all firmware-dump evidence (loaded kmods, live process
> list, binary strings) is ATBM6031.

## Station mode (`waved-wifi`)

`WifiService` (`crates/waved-wifi/src/lib.rs`) runs a `tokio::select!` loop:

- **Poll** every 10 s via `wpa::status()` → `wpa_cli status` + `wpa_cli
  signal_poll`, publishing `WifiState { connected, ssid, ip_addr,
  signal_strength }` to the state bus. NTP (`ntpd -q`) is kicked on connect.
- **Commands**: consumes `Command::WifiConnect { ssid, password }` from the bus
  and calls `wpa::connect`.

`wpa.rs` wraps `wpa_cli` with:
- Real error type `WifiError`; every call checks exit status **and** parses the
  `OK`/`FAIL` stdout token (wpa_cli exits 0 on logical failure).
- A `tokio::time::timeout` (10 s) on every external command so a wedged binary
  can't freeze the loop.
- **Quote-safe encoding**: SSID is set as hex (`set_network N ssid <hex>`); the
  PSK is derived through `wpa_passphrase` and stored as the 64-hex PMK
  (`set_network N psk <hex>`). This handles any valid SSID/passphrase without
  quoting issues **and** keeps the passphrase hashed at rest.
- **PSK never logged**: the `wpa_cli` debug log masks the value after a `psk`
  arg (`psk ****`).
- Roam handling tracks the last SSID, firing `WifiDisconnected` +
  `WifiConnected` when the network changes without a full drop.

## Provisioning (`waved-provision`)

`ProvisioningService` owns the flow and publishes `ProvisioningState` to the bus
so the UI can render it. Triggered by `Command::OpenWifiProvisioning` (the Wi-Fi
row in System settings).

### State machine

```
Idle → Scanning → ApActive{ssid,passphrase,url} → Connecting{ssid} → Connected{ssid}
                                                                    ↘ Failed{reason}
                          (timeout / cancel) ───────────────────────→ Idle
```

1. **Scanning** — `wpa::scan()` runs while still in STA mode (a single radio
   can't reliably scan as AP); the SSID list is cached for the form.
2. **ApActive** — `ApController` brings `wlan0` up as AP (see below). The
   on-device screen shows the AP SSID, passphrase, and `http://192.168.2.1`.
3. The embedded HTTP server serves a one-page form (`GET /`) and accepts
   `POST /connect` (SSID dropdown + password). Resolves on creds, a **5-minute
   timeout**, or `Command::CancelWifiProvisioning`.
4. AP is torn down (STA restored) on **every** exit path, then `wpa::connect`
   joins the chosen network → `Connected` / `Failed`.

### AP network layout

| Item | Value |
|---|---|
| AP SSID | `Wave-XXXX` (XXXX = last 4 hex of the Wi-Fi MAC, uppercase) |
| Security | **WPA2-PSK** (not open — see Security) |
| Passphrase | 8 hex chars derived from the MAC, shown on the device screen |
| Gateway IP | `192.168.2.1` |
| DHCP range | `192.168.2.50` – `192.168.2.200`, 12 h lease (`dnsmasq`) |
| HTTP | `192.168.2.1:80`, lifetime == AP lifetime |

Runtime configs are written to `/tmp/waved-hostapd.conf` and
`/tmp/waved-dnsmasq.conf`.

### On-device commands used

AP up (`ApController::start`):
```
wpa_cli disconnect ; killall -q wpa_supplicant
ip addr flush dev wlan0 ; ip link set wlan0 up ; ip addr add 192.168.2.1/24 dev wlan0
hostapd -B /tmp/waved-hostapd.conf
dnsmasq -C /tmp/waved-dnsmasq.conf
```
AP down / STA restore (`ApController::stop`):
```
killall -q dnsmasq ; killall -q hostapd ; ip addr flush dev wlan0
wpa_supplicant -B -Dnl80211 -iwlan0 -c /tmp/wpa_supplicant.conf
udhcpc -i wlan0 -b
```

> **TODO (reconcile on hardware):** the exact STA-restore invocation must match
> how the device's own init (`S50_ko_install` / `S60wifi`) starts
> `wpa_supplicant` — verify the config path and driver flags on-device and
> update `ApController::stop` if they differ.

## Security decisions (improvements over stock)

- **WPA2 provisioning AP** with an on-screen passphrase. Stock ships an *open*
  AP; ours encrypts the link so the user's home Wi-Fi password isn't sniffable
  over the air during the plain-HTTP `POST`.
- **No PSK in logs** at any level.
- **PSK hashed at rest** via `wpa_passphrase` (no plaintext passphrase written to
  `wpa_supplicant.conf`).

## Prerequisite probe (Task 0)

Approach A assumes `hostapd`, `dnsmasq`, `wpa_passphrase`, `wpa_cli` exist in the
rootfs and the radio supports AP mode. Verify before relying on provisioning:
```bash
adb shell 'for b in hostapd dnsmasq wpa_passphrase wpa_cli; do printf "%s: " $b; command -v $b || echo MISSING; done'
adb shell 'iw list 2>/dev/null | grep -A6 "Supported interface modes"'   # expect "* AP"
```
**Probe result:** _not yet run — fill in once checked on hardware._ If
`hostapd`/`dnsmasq` are missing, bundle static aarch64 binaries or fall back to
spec Approach C (configs written by `waved`, daemons run by init.d).

## Acceptance

`scripts/test-wifi-provisioning.sh <ssid> <psk>` — triggers provisioning,
asserts the AP + page are reachable, POSTs credentials, and asserts STA
reconnect.

## Not implemented (YAGNI for v1)

Captive-portal auto-redirect; BLE GATT provisioning; on-device keyboard / text
entry; WPS / 802.1X / EAP (device is WPA2-PSK only).
