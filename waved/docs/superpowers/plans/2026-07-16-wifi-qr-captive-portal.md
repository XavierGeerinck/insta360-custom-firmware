# One-scan Wi-Fi QR and captive portal — implementation plan

**Goal:** Let a phone scan one QR to join the temporary WPA2 AP and reach the
existing setup page through normal captive-network handling.

**Design:** `docs/superpowers/specs/2026-07-16-wifi-qr-captive-portal-design.md`

## Task 1: Enable and test LVGL QR support

**Files:**

- Modify `crates/waved-ui/lv_conf.h`
- Modify `crates/waved-ui/src/screens/wifi_provisioning.rs`

**Work:**

1. Enable `LV_USE_QRCODE` and ensure its canvas dependency is enabled.
2. Add a pure `wifi_qr_payload()` helper with standard escaping tests.
3. Add UI object fields for QR container, QR widget, fallback label, and normal
   state label.
4. In `ApActive`, update and show the QR plus compact fallback credentials; in
   all other states hide QR objects and show centered status text.
5. Render a 140 px QR inside a white quiet-zone container, with all controls in
   the circular safe area and the existing bottom Back button retained.
6. Treat QR update failure as text-fallback-only; do not fail provisioning.
7. Run the UI test suite in simulate mode.

**Commit:** `feat(ui): show provisioning Wi-Fi QR`

## Task 2: Add in-process captive DNS

**Files:**

- Add `crates/waved-provision/src/dns.rs`
- Modify `crates/waved-provision/src/lib.rs`

**Work:**

1. Implement a bounded one-question DNS parser with strict label/packet checks.
2. Encode A/ANY responses pointing to `192.168.2.1`; return valid no-answer
   responses for unsupported types.
3. Add tests for A, ANY, unsupported type, malformed/truncated packet,
   compression pointer, and multiple questions.
4. Bind UDP/53 to `wlan0`, cap datagrams at 512 bytes, and supervise via a
   watch shutdown receiver like DHCP.
5. Run provisioning tests and strict clippy.

**Commit:** `feat(provision): add captive DNS responder`

## Task 3: Captive HTTP redirects and lifecycle integration

**Files:**

- Modify `crates/waved-provision/src/http.rs`
- Modify `crates/waved-provision/src/lib.rs`

**Work:**

1. Route arbitrary valid `GET` and `HEAD` requests to a captive redirect.
2. Render `302 Found` with `Location: http://192.168.2.1/`, an empty body for
   HEAD, and no reflected host/path values.
3. Preserve `GET /`, `POST /connect`, explicit method errors, framing limits,
   and credential validation.
4. Bind DNS before publishing `ApActive`; supervise DHCP and DNS together.
5. Stop and await both UDP tasks before hostapd/address teardown on credentials,
   cancel, timeout, service error, and shutdown.
6. Add HTTP routing/status tests and rerun provisioning tests/clippy.

**Commit:** `feat(provision): redirect captive portal probes`

## Task 4: Documentation and acceptance harness

**Files:**

- Modify `docs/WIFI.md`
- Modify `scripts/test-wifi-provisioning.sh`

**Work:**

1. Document QR payload, wildcard DNS, and captive prompt behavior.
2. Extend preflight to assert UDP/53 and a captive probe redirect.
3. Make the operator gate require scanning the QR, joining successfully, and
   observing the setup sheet or sign-in prompt.
4. Run shell syntax, shellcheck, and diff checks.

**Commit:** `docs(wifi): document QR captive provisioning`

## Task 5: Full verification and deployment

1. Run focused provisioning/UI/Wi-Fi tests, strict clippy for changed crates,
   simulated app tests/build, and the full ARM `ui,capture-alsa` release build.
2. Verify ARM64 and maximum required glibc `<= 2.31`.
3. Preserve the deployed binary, atomically install the new artifact, and
   reboot under the boot-owned watchdog.
4. Open provisioning and capture/decode the framebuffer; inspect QR quiet zone,
   fallback text, and Back button.
5. Independently decode the QR and compare its live SSID/password with the
   on-screen state while keeping the payload out of retained logs.
6. Scan with the phone; verify WPA2 join, DHCP lease, UDP/53 response, and
   captive setup sheet/sign-in prompt.
7. Submit real credentials and run the existing SSID+IPv4, cleanup, secret,
   process, UI, storage, default-off mic capture, and reboot gates.

## Rollback

The current verified binary remains on `/mnt/udisk/debug` and locally. Any AP
failure is cleaned by stopping hostapd, flushing `wlan0`, removing the runtime
config, and rebooting into the previous artifact if necessary.
