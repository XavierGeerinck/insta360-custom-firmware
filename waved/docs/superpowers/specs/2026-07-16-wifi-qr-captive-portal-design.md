# One-scan Wi-Fi QR and captive portal

**Date:** 2026-07-16
**Status:** Approved for implementation
**Builds on:** `2026-07-16-wifi-provisioning-runtime-integration-design.md`

## Goal

Replace manual entry of the temporary `Wave-XXXX` SSID and password with one
phone scan. The QR joins the secured provisioning AP; the phone's captive
network handling then opens, or prompts the user to open, the existing Wi-Fi
setup page.

## User flow

1. User opens More → Settings → Wi-Fi.
2. The Wave shows `Scanning…` while preparing the AP.
3. When ready, the Wave shows a large QR code and “Scan to set up Wi-Fi.”
4. The phone scans a standard Wi-Fi payload and asks to join `Wave-XXXX`.
5. DHCP advertises the Wave as router and DNS. The phone performs its normal
   captive-network check.
6. In-process wildcard DNS resolves the probe host to `192.168.2.1`; HTTP probe
   requests receive a redirect to `http://192.168.2.1/`.
7. The phone opens its captive login sheet, or displays its normal “Sign in to
   network” prompt. The existing setup form is shown there.
8. After credentials are submitted, the Wave tears down the AP and joins the
   selected network as already designed.

Phone vendors control whether their captive sheet opens immediately or behind
a notification. The Wave therefore keeps the SSID, password, and URL available
as a compact fallback below or alongside the QR.

## QR payload and rendering

Use the standard Wi-Fi QR payload:

```text
WIFI:T:WPA;S:<escaped SSID>;P:<escaped passphrase>;H:false;;
```

Escape `\`, `;`, `,`, `:`, and `"` with a leading backslash. The generated
AP SSID and passphrase are currently alphanumeric, but the helper is tested
independently so future naming changes remain safe.

Enable LVGL's built-in `LV_USE_QRCODE`. Render a 140 px black-on-white QR
inside a larger white square that provides a quiet zone; the LVGL QR widget
does not add that margin itself. Keep the title, QR, compact fallback details,
and bottom Back button inside the circular screen's measured safe area.

The QR object exists only for `ApActive`. Other states continue to show text.
When the state changes, hide/show the QR and fallback objects rather than
recreating the screen. QR update failure falls back to the existing text
instructions without ending provisioning.

## Captive DNS

Add a bounded UDP/53 task owned by the provisioning session:

- parse one DNS question from an IPv4 UDP query;
- accept standard `A` and `ANY` questions;
- respond with one `A 192.168.2.1` answer and a short TTL;
- return a valid no-answer response for unsupported types;
- preserve transaction ID and recursion-desired flag;
- reject malformed names, compression loops/pointers in queries, truncated
  packets, multiple questions, and oversized labels;
- cap datagrams at 512 bytes;
- bind to `wlan0` using the same Linux socket discipline as DHCP;
- stop and await the task before removing the AP address.

This is a session-local captive DNS responder, not a recursive resolver. It
does not forward queries or handle TCP, DNSSEC, IPv6, or mDNS.

## Captive HTTP behavior

Keep `GET /` as the setup form and `POST /connect` as credential submission.
For every other valid `GET` or `HEAD` path, respond:

```text
HTTP/1.1 302 Found
Location: http://192.168.2.1/
```

This covers legacy cleartext connectivity probes without hard-coding vendor
hostnames or paths. Unsupported methods keep returning `405`; malformed and
oversized requests retain their current errors. HTTPS probes are not
intercepted because the Wave cannot present a publicly trusted certificate for
arbitrary names. Android's documented legacy behavior continues to use HTTP
redirects when no captive portal API is advertised; the Wave deliberately does
not advertise the newer API because it requires a valid HTTPS API endpoint.

Once credentials are accepted, the acknowledgement is written before AP
teardown as today. No captive-portal request or DNS query is logged at a level
that could leak user-entered form data.

## Lifecycle and error handling

- AP startup succeeds only after hostapd, DHCP, captive DNS, and HTTP bind are
  ready.
- Failure to bind UDP/53 ends startup, cleans partial components, restores STA,
  and shows a user-safe failure.
- Unexpected DHCP or DNS task exit ends the active session through the shared
  cleanup path.
- Cancel, timeout, connection failure, and process shutdown stop both UDP
  services before hostapd and station restoration.
- DNS/HTTP service errors never expose the temporary AP passphrase in logs.

## Verification

Automated:

- Wi-Fi QR escaping and complete payload tests.
- QR state-text fallback and visibility decision tests.
- DNS parsing/encoding tests: valid A/ANY, unsupported type, malformed label,
  compression pointer, multiple questions, and truncated packet.
- HTTP tests for `/`, `/connect`, arbitrary captive GET/HEAD redirect, methods,
  limits, and `Location` header.
- Existing provisioning, Wi-Fi, UI, simulated app, clippy, and full ARM build.

On device:

1. Open provisioning and capture the framebuffer; confirm a square QR with
   quiet zone, fallback details, and reachable Back button.
2. Decode the framebuffer QR with an independent phone or decoder; verify the
   payload contains the live SSID/passphrase and no home-network credential.
3. Scan on a phone, accept the join, and verify DHCP lease plus DNS answer.
4. Verify the captive setup sheet opens automatically or appears through the
   phone's sign-in prompt; verify the form loads.
5. Submit real Wi-Fi credentials and complete the existing STA/IPv4, cleanup,
   secret-scan, process, display, and reboot acceptance checks.

## Out of scope

- A custom mobile app or proprietary QR/deep link.
- A publicly trusted HTTPS captive portal endpoint.
- Silent Wi-Fi joins without the phone's user confirmation.
- General DNS service after the provisioning AP is torn down.
