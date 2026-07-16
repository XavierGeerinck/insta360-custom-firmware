# Wi-Fi provisioning runtime integration

**Date:** 2026-07-16
**Status:** Approved design; ready for implementation planning
**Builds on:** `2026-06-04-wifi-provisioning-design.md`

## Problem

The phone/web provisioning flow is implemented but cannot run on the Wave
rootfs as deployed. Hardware probing found:

- the ATBM6031 radio supports `managed` and `AP/VLAN` modes;
- `hostapd`, `wpa_supplicant`, `wpa_cli`, `udhcpc`, and `curl` are present;
- `dnsmasq` and `wpa_passphrase` are absent;
- the persistent station configuration is `/etc/wpa_supplicant.conf`, not the
  `/tmp/wpa_supplicant.conf` path currently used by `ApController::stop()`;
- no station supplicant or DHCP client is running after boot, so steady-state
  Wi-Fi must be started and supervised by `waved`.

Provisioning must become self-contained without adding separately deployed
executables that can drift from the application binary.

## Decisions

| Concern | Decision |
|---|---|
| WPA2 key derivation | Implement PBKDF2-HMAC-SHA1 in `waved-wifi` using Rust crates |
| Provisioning DHCP | Add a minimal in-process IPv4 DHCP server to `waved-provision` |
| AP management | Continue using the stock `/usr/sbin/hostapd` |
| STA configuration | Use `/etc/wpa_supplicant.conf` |
| STA ownership | `waved-wifi` ensures `wpa_supplicant` and `udhcpc` are running |
| Success condition | Requested SSID is `COMPLETED` and has a non-empty IPv4 address |
| AP passphrase | Random 8-character hexadecimal value per provisioning session |
| Runtime secrets | Config mode `0600`, never log PSKs, remove configs on teardown |

Bundling `dnsmasq` and `wpa_passphrase` was rejected because it adds binary
packaging, licensing, and target-libc maintenance. Depending on the stock
rootfs was rejected because both executables are confirmed absent.

## Architecture

### Station lifecycle (`waved-wifi`)

Add an idempotent station controller around external commands:

1. Bring `wlan0` up.
2. Start `wpa_supplicant -B -Dnl80211 -iwlan0 -c
   /etc/wpa_supplicant.conf` only when it is not already running.
3. Start `udhcpc -i wlan0 -b` only after association and only when no suitable
   client is already running.
4. On provisioning entry, stop station DHCP and the supplicant before changing
   interface mode.
5. On every provisioning exit—including AP, HTTP, timeout, cancellation, and
   connection failures—restore station mode through the same idempotent API.

External command wrappers must treat a non-zero exit as an error, apply a
bounded timeout, and include the program and exit status in diagnostic logs.
Credential values must never appear in arguments rendered to logs.

`WifiService` calls `ensure_station()` during startup and retries failed
station initialization from its existing poll loop. This makes Wi-Fi recover
after boot and after transient daemon failures without introducing another
watchdog.

### WPA2 key derivation (`waved-wifi`)

Replace the `wpa_passphrase` subprocess with the WPA/WPA2-PSK derivation
defined by IEEE 802.11i: PBKDF2-HMAC-SHA1, SSID bytes as salt, 4096 iterations,
32-byte output, encoded as 64 lowercase hexadecimal characters. Existing
8–63-character passphrase validation remains. Open networks continue to use
`key_mgmt NONE`.

Unit tests use published WPA test vectors and verify that passphrases are not
present in formatted command diagnostics.

### Provisioning DHCP (`waved-provision`)

Add a small DHCPv4 service bound to UDP port 67 on `wlan0`. It supports only
the messages required for provisioning:

- parse BOOTP/DHCP packets with strict length and option bounds;
- answer `DHCPDISCOVER` with `DHCPOFFER`;
- answer a valid `DHCPREQUEST` with `DHCPACK`;
- answer an invalid or unavailable request with `DHCPNAK`;
- allocate `192.168.2.50–192.168.2.200` deterministically by client identifier
  or MAC for a 12-hour lease;
- advertise subnet `255.255.255.0`, router/DNS `192.168.2.1`, and server ID
  `192.168.2.1`.

The service is an async task owned by the provisioning session. It starts only
after `wlan0` has `192.168.2.1/24`, and is cancelled and awaited before the AP
address is removed. It does not implement DHCP relay, IPv6, persistence, or a
general DNS resolver. Advertising the device as DNS is sufficient for the
v1 manual URL flow; captive-portal DNS interception remains out of scope.

Socket setup pins the server to `wlan0`, enables broadcast, and fails the AP
startup if port 67 cannot be acquired. Packet parsing and response construction
are isolated from socket I/O and covered by unit tests.

### Provisioning session (`waved-provision`)

The session order becomes:

1. Subscribe for cancellation and publish `Scanning`.
2. Ensure station mode, scan, and deduplicate/sort SSIDs.
3. Stop station services.
4. Generate a random AP passphrase.
5. Write the hostapd config with mode `0600`.
6. Configure `wlan0`, start hostapd, then start in-process DHCP.
7. Publish `ApActive` and serve the bounded HTTP form for at most five minutes.
8. Stop HTTP/DHCP/hostapd, remove runtime configs, and restore station mode.
9. If credentials were submitted, configure/select the network and wait up to
   45 seconds for the requested SSID plus a real IPv4 address.
10. Publish `Connected`, `Failed`, or `Idle` as appropriate.

Cleanup uses one shared teardown path and remains idempotent. A partial AP
startup cannot skip station restoration. A second `OpenWifiProvisioning`
command while a session is active is ignored.

### HTTP hardening

Keep the existing single-page server, with these bounds:

- accept only `GET /` and `POST /connect`;
- read headers up to 8 KiB and a form body up to 4 KiB;
- honor and validate `Content-Length`, including split TCP reads;
- apply per-connection read/write timeouts;
- return explicit `400`, `404`, `405`, and `413` responses;
- validate SSID byte length (1–32) and PSK length before accepting;
- never log the POST body or password;
- HTML-escape SSIDs and URL-decode without accepting malformed escapes or NUL.

The acknowledgement is written before AP teardown. Because the phone loses
the provisioning network immediately afterward, final success is shown on the
Wave display rather than relying on the browser connection.

## Errors and recovery

- AP startup failure: stop every component that may have started, restore STA,
  publish a user-safe failure, and log the specific internal cause.
- DHCP/HTTP task failure: end the provisioning session and follow the same
  cleanup path.
- Timeout or Cancel: tear down cleanly and publish `Idle`.
- Invalid submitted credentials: keep the AP/form active and return a `400`
  response so the user can correct them.
- Association/DHCP timeout: publish `Failed`, leave station services running,
  and allow normal background recovery or a new provisioning attempt.
- Process shutdown: provisioning task cleanup restores station mode before
  `waved` exits whenever the runtime permits orderly shutdown.

## Verification

### Automated

- `waved-wifi`: WPA test vectors, validation, station command classification,
  association/IP success predicate, and secret-redaction tests.
- `waved-provision`: DHCP parsing/encoding/allocation, AP config permissions and
  lifecycle decisions, HTTP framing/limits/validation, and cancellation/error
  transitions.
- Workspace tests, clippy, simulated build, and the pinned full-feature ARM
  release build.
- ELF inspection must show a maximum required glibc version no newer than the
  Wave's glibc 2.31.

### On-device acceptance

1. Deploy and reboot; verify `waved` is boot-watchdog owned and the UI is live.
2. Verify station services initialize and a saved network can acquire an IP.
3. Tap System → Wi-Fi; verify the screen shows `Wave-XXXX`, a session password,
   and `http://192.168.2.1`.
4. Join the AP from a phone and verify it receives an address in the configured
   lease range.
5. Load the form, submit a real SSID/password, and verify AP teardown.
6. Verify the Wave reports the requested SSID and a real IPv4 address within
   45 seconds and remains running.
7. Repeat cancellation and five-minute-timeout paths, verifying STA recovery.
8. Search logs/configs for the submitted password and confirm it is absent;
   confirm runtime AP configs were removed.
9. Reboot once more and verify normal station connectivity, visible UI, stable
   storage, and no diagnostic microphone capture file.

The acceptance script may automate device-side assertions, but the phone join
and real credential entry remain an explicit manual step because ADB cannot
prove a separate client received DHCP or rendered the page.

## Out of scope

- Captive-portal auto-launch or wildcard DNS interception.
- BLE provisioning, WPS, 802.1X/EAP, or an on-device keyboard.
- A reusable general-purpose DHCP or DNS server.
- Persisting DHCP leases across provisioning sessions.
