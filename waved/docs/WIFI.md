# Wi-Fi & Provisioning

How `waved` manages Wi-Fi: station-mode monitoring/connection, and on-device
phone/web provisioning. Design spec:
[`docs/superpowers/specs/2026-06-04-wifi-provisioning-design.md`](superpowers/specs/2026-06-04-wifi-provisioning-design.md).
The one-scan QR/captive extension is specified in
[`docs/superpowers/specs/2026-07-16-wifi-qr-captive-portal-design.md`](superpowers/specs/2026-07-16-wifi-qr-captive-portal-design.md).
The status menu and saved-network selection behavior are specified in
[`docs/superpowers/specs/2026-07-16-wifi-status-auto-connect-design.md`](superpowers/specs/2026-07-16-wifi-status-auto-connect-design.md).

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
  signal_poll`, publishing connection details to the state bus. On connect,
  the bounded in-process SNTP client validates a response from the configured
  servers and sets the system clock; the device does not depend on the absent
  `ntpd` executable.
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

### Saved networks and automatic selection

`wpa_supplicant` is the automatic selector; `waved` does not run a competing
scan/retry loop. At boot and after link loss, the supplicant chooses among
enabled, previously saved profiles. Unknown networks, including unknown open
networks, are never joined automatically.

When the user provisions credentials, `waved`:

1. reuses a profile with the same SSID, or adds one if it is new;
2. assigns it a priority above the current saved maximum;
3. enables that selected profile without enabling or disabling unrelated
   profiles;
4. saves the configuration, tightens `/etc/wpa_supplicant.conf` to mode `0600`,
   and requests reassociation.

The implementation intentionally does not use `select_network`, because that
command disables every other profile and breaks automatic fallback. A profile
that was already disabled remains disabled unless the user explicitly
re-provisions that same SSID. A healthy current connection is not replaced
merely because a higher-priority saved network appears; selection happens at
boot, after disconnect, or after an explicit Connect action.

## Wi-Fi menu

Menu → Wi-Fi opens a non-disruptive management screen. It shows the network
name, the connection status with the signal (excellent, good, fair or weak),
and the IPv4 address. Opening this screen does not stop station mode or start
the provisioning AP.

Its button opens the **Set up Wi-Fi** screen, which starts QR/captive
provisioning. Not connected, the button reads **Set up Wi-Fi** and goes
straight there; connected, it reads **Change network** and asks first
(**Change network?**), because Wave leaves its network while the setup network
is open. The setup screen shows the QR with the manual fallback under it,
progress such as **Connecting to …**, or a failure with **Try again**, and
**Cancel** closes the setup network.

## Provisioning (`waved-provision`)

`ProvisioningService` owns the flow and publishes `ProvisioningState` to the bus
so the UI can render it. Triggered by `Command::OpenWifiProvisioning`, which
the **Set up Wi-Fi** screen sends when it opens and on **Try again**.

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
4. The embedded HTTP server serves the typed `config.toml` setup form
   (`GET /`). `POST /save` persists settings and reconnects a saved station
   profile; `POST /save-connect` also provisions the selected SSID.
   Device, audio, webhook, storage, security, NTP, and Wi-Fi settings are
   available. Secrets are write-only, requests are bounded to 32 KiB, and a
   per-session CSRF token protects both final actions.
5. Invalid settings leave the AP and form active with field-level errors.
   Valid settings are atomically saved before teardown.
6. AP is torn down (STA restored) on **every** exit path, then station mode
   reconnects. A controlled process restart applies one consistent
   configuration snapshot and triggers the pending webhook setup test.

Webhook configuration and its multipart contract are documented in
[`docs/WEBHOOKS.md`](WEBHOOKS.md).

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
- The persisted station configuration is mode `0600` after credential updates.
- **Device PIN** (`security.device_password`, D10): the form takes 4 to 6 digits
  only, and config.toml keeps a salted PBKDF2-HMAC-SHA256 record
  (`pin1$<digits>$<iterations>$<salt>$<hash>`), never the digits. A plain 4–6
  digit value from a hand edit is hashed when the file loads; anything else is
  dropped with a warning (the keypad can't type it). With at most 10^6 PINs the
  hash only keeps the digits out of the file and its copies; the file's `0600`
  mode is what protects it. `waved_config::verify_device_pin` is the one check
  (PIN screen and Wave Controller).

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

`scripts/test-wifi-provisioning.sh <ssid> <psk>` — the operator opens Menu →
Wi-Fi and taps **Set up Wi-Fi** or **Change network**; the script then
asserts AP/DHCP/DNS/HTTP readiness, checks a captive-probe redirect, pauses for
the operator to scan the QR and observe the captive setup sheet/prompt, POSTs
credentials, and asserts STA reconnect, saved-profile reuse, enabled state, and
secure config permissions.

## Not implemented (YAGNI for v1)

BLE GATT provisioning; on-device keyboard / text entry; WPS / 802.1X / EAP
(device is WPA2-PSK only).
