# Wi-Fi & Provisioning

How `waved` manages Wi-Fi: station-mode monitoring/connection, and on-device
phone/web provisioning. Design spec:
[`docs/superpowers/specs/2026-06-04-wifi-provisioning-design.md`](superpowers/specs/2026-06-04-wifi-provisioning-design.md).
The one-scan QR/captive extension is specified in
[`docs/superpowers/specs/2026-07-16-wifi-qr-captive-portal-design.md`](superpowers/specs/2026-07-16-wifi-qr-captive-portal-design.md).

## Hardware / stack

- **Chipset:** ATBM6031 (ATBM613x family) over **SDIO**, dual-band 2.4/5 GHz.
  Kernel modules: `aml_wifi.ko` (Amlogic glue) → `atbm613x_wifi_sdio.ko`,
  exposed to userspace via **nl80211**.
- **Userspace tools** reused from the stock rootfs: `wpa_supplicant`,
  `wpa_cli`, `udhcpc`, and `hostapd`. Hardware probing on 2026-07-16 found
  that `dnsmasq` and `wpa_passphrase` are absent, so `waved` supplies DHCP and
  WPA2 key derivation in process.
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
  PSK is derived in Rust with PBKDF2-HMAC-SHA1 (4096 rounds) and stored as the 64-hex PMK
  (`set_network N psk <hex>`). This handles any valid SSID/passphrase without
  quoting issues **and** keeps the passphrase hashed at rest.
- **PSK never logged**: the `wpa_cli` debug log masks the value after a `psk`
  arg (`psk ****`).
- Roam handling tracks the last SSID, firing `WifiDisconnected` +
  `WifiConnected` when the network changes without a full drop.
- `waved-wifi` owns station lifecycle: it idempotently starts
  `wpa_supplicant` with `/etc/wpa_supplicant.conf`, starts `udhcpc` only after
  association, and retries recovery from the normal ten-second poll loop.

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
   on-device screen shows a standard WPA Wi-Fi QR, with the AP SSID,
   passphrase, and `192.168.2.1` alongside as a manual fallback.
3. Scanning the QR joins the temporary network. Wildcard DNS resolves captive
   checks to the camera, and arbitrary HTTP `GET`/`HEAD` probes redirect to
   `http://192.168.2.1/`. Depending on the phone, the setup sheet opens or a
   **Sign in to network** prompt appears; tap that prompt if it is not opened
   automatically.
4. The embedded HTTP server serves a one-page form (`GET /`) and accepts
   `POST /connect` (SSID dropdown + password). Resolves on creds, a **5-minute
   timeout**, or `Command::CancelWifiProvisioning`.
5. AP is torn down (STA restored) on **every** exit path, then `wpa::connect`
   joins the chosen network → `Connected` / `Failed`.

The QR payload is `WIFI:T:WPA;S:<escaped SSID>;P:<escaped passphrase>;H:false;;`.
It contains only the temporary AP credentials; opening the setup form remains
the phone's normal captive-network flow.

### AP network layout

| Item | Value |
|---|---|
| AP SSID | `Wave-XXXX` (XXXX = last 4 hex of the Wi-Fi MAC, uppercase) |
| Security | **WPA2-PSK** (not open — see Security) |
| Passphrase | Random 8 hex chars generated for each AP session, shown on screen |
| Gateway IP | `192.168.2.1` |
| DHCP range | `192.168.2.50` – `192.168.2.200`, 12 h lease (in-process DHCPv4) |
| DNS | UDP/53 wildcard A/ANY response → `192.168.2.1` (in-process) |
| HTTP | `192.168.2.1:80`, lifetime == AP lifetime |

The hostapd runtime config is written to `/tmp/waved-hostapd.conf` with mode
`0600` and removed during every teardown path.

### On-device commands used

AP up (`ApController::start`):
```
waved-wifi stops udhcpc + wpa_supplicant
ip addr flush dev wlan0 ; ip link set wlan0 up ; ip addr add 192.168.2.1/24 dev wlan0
hostapd -B /tmp/waved-hostapd.conf
in-process DHCP binds wlan0:67
in-process captive DNS binds wlan0:53
```
AP down / STA restore (`ApController::stop`):
```
stop in-process DNS + DHCP ; killall -TERM hostapd ; ip addr flush dev wlan0
wpa_supplicant -B -Dnl80211 -i wlan0 -c /etc/wpa_supplicant.conf
udhcpc -i wlan0 -b  # after association
```

The rootfs has no Wi-Fi userspace init service: `S50_ko_install` loads the
ATBM6031 driver only. `waved` therefore owns the userspace station processes.

## Security decisions (improvements over stock)

- **WPA2 provisioning AP** with an on-screen passphrase. Stock ships an *open*
  AP; ours encrypts the link so the user's home Wi-Fi password isn't sniffable
  over the air during the plain-HTTP `POST`.
- **No PSK in logs** at any level.
- **PSK hashed at rest** via native PBKDF2 (no submitted plaintext passphrase
  written to `wpa_supplicant.conf`).

## Measured prerequisite probe (2026-07-16)

Probe command:
```bash
adb shell 'for b in hostapd dnsmasq wpa_passphrase wpa_cli udhcpc; do printf "%s: " $b; command -v $b || echo MISSING; done'
adb shell 'iw list 2>/dev/null | grep -A6 "Supported interface modes"'   # expect "* AP"
```

Result: `hostapd`, `wpa_cli`, and `udhcpc` are present; `dnsmasq` and
`wpa_passphrase` are missing; `iw` reports both `managed` and `AP/VLAN` modes.
The persistent station config is `/etc/wpa_supplicant.conf`.

## Acceptance

`scripts/test-wifi-provisioning.sh <ssid> <psk>` — triggers provisioning,
asserts AP/DHCP/DNS/HTTP readiness, checks a captive-probe redirect, pauses for
the operator to scan the QR and observe the captive setup sheet/prompt, POSTs
credentials, and asserts STA reconnect.

## Not implemented (YAGNI for v1)

BLE GATT provisioning; on-device keyboard / text entry; WPS / 802.1X / EAP
(device is WPA2-PSK only).
