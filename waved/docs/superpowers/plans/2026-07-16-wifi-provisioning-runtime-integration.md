# Wi-Fi provisioning runtime integration — implementation plan

**Goal:** Make the existing phone/web provisioning flow work on the Wave
without `dnsmasq` or `wpa_passphrase`, and prove recovery on real hardware.

**Design:** `docs/superpowers/specs/2026-07-16-wifi-provisioning-runtime-integration-design.md`

## Constraints

- Keep the UI and command/state APIs already shipped.
- Preserve `/etc/wpa_supplicant.conf`; never expose PSKs in logs.
- Restore STA mode on every AP exit and after every partial startup failure.
- Keep the device-visible app running until a verified replacement is ready.
- Build the device binary with the pinned glibc-compatible cross image and
  `ui,capture-alsa` features.
- Do not include the user's unrelated `Cargo.lock` and `TODO.md` edits except
  for dependency-lock changes demonstrably required by this implementation.

## Task 1: Native WPA2 PSK derivation

**Files:**

- Modify `crates/waved-wifi/Cargo.toml`
- Modify `crates/waved-wifi/src/wpa.rs`
- Update `Cargo.lock` only through Cargo

**Work:**

1. Add direct `hmac` and `sha1` dependencies already present transitively.
2. Write failing tests for the IEEE and `ThisIsAPassword` PBKDF2 vectors.
3. Implement PBKDF2-HMAC-SHA1 (4096 rounds, 32 bytes) in a focused helper.
4. Replace the missing `wpa_passphrase` process call with the helper.
5. Retain passphrase validation/open-network behavior and verify no plaintext
   reaches command debug formatting.
6. Run `cargo test -p waved-wifi`.

**Commit:** `fix(wifi): derive WPA2 keys without wpa_passphrase`

## Task 2: Station lifecycle and connection completion

**Files:**

- Modify `crates/waved-wifi/src/wpa.rs`
- Modify `crates/waved-wifi/src/lib.rs`

**Work:**

1. Add checked, timeout-bounded command execution and process detection.
2. Add idempotent `ensure_station`, `stop_station`, and `ensure_dhcp` APIs
   using `/etc/wpa_supplicant.conf`.
3. Parse association state separately from IPv4 acquisition.
4. Start station mode when `WifiService` starts and retry from its poll loop.
5. Start `udhcpc` only after association when no IPv4 address exists.
6. Add `wait_for_connection(ssid, timeout)` requiring `COMPLETED`, matching
   SSID, and a non-empty IPv4 address.
7. Unit-test parsing, success predicates, and non-zero command classification.
8. Run `cargo test -p waved-wifi` and clippy for the crate.

**Commit:** `fix(wifi): own station lifecycle and await IPv4`

## Task 3: In-process provisioning DHCP

**Files:**

- Add `crates/waved-provision/src/dhcp.rs`
- Modify `crates/waved-provision/src/lib.rs`
- Modify `crates/waved-provision/Cargo.toml`
- Update `Cargo.lock` only through Cargo

**Work:**

1. Add a packet-only DHCP module with strict BOOTP/options parsing.
2. Add deterministic session-local lease allocation across `.50–.200`.
3. Encode OFFER, ACK, and NAK packets with server/router/DNS/subnet/lease
   options.
4. Add malformed-packet, discover/offer, request/ack, invalid-request/nak,
   lease-stability, and pool-bound tests.
5. Add the async UDP server, broadcast replies, `SO_BINDTODEVICE=wlan0`, and
   cancellation.
6. Run `cargo test -p waved-provision` and clippy for the crate.

**Commit:** `feat(provision): serve DHCP in process`

## Task 4: AP lifecycle and secure cleanup

**Files:**

- Modify `crates/waved-provision/src/ap.rs`
- Modify `crates/waved-provision/src/lib.rs`
- Modify `crates/waved-provision/Cargo.toml`

**Work:**

1. Remove `dnsmasq` config/process handling.
2. Use `waved_wifi::wpa::stop_station()` before AP configuration and the shared
   station restore API during cleanup.
3. Make command failures fatal, bounded, and diagnostic.
4. Write the hostapd config as `0600`; remove it on all teardown paths.
5. Generate a random per-session eight-hex AP password.
6. Start DHCP after hostapd, supervise it with HTTP/cancel/timeout, stop and
   await it before removing the AP address.
7. Make session cleanup single-path and idempotent; ignore re-open commands
   while the current `provision()` future owns the service loop.
8. After credential submission, call `connect` and wait up to 45 seconds for
   the requested SSID plus IPv4 before publishing `Connected`.
9. Unit-test config contents, password shape, and lifecycle decision helpers.

**Commit:** `fix(provision): integrate AP DHCP and STA recovery`

## Task 5: HTTP request framing and validation

**Files:**

- Modify `crates/waved-provision/src/http.rs`
- Modify `crates/waved-provision/src/lib.rs`

**Work:**

1. Parse the request head independently from the body and validate
   `Content-Length`.
2. Read split TCP requests with 8 KiB header and 4 KiB body limits plus timeouts.
3. Reject malformed URL escapes/NUL and validate SSID byte length and PSK
   character length.
4. Add explicit 400/404/405/413 responses without reflecting secrets.
5. Keep invalid credential submissions inside the AP form loop for correction.
6. Add unit tests for split framing helpers, bounds, methods/routes, validation,
   and escaping.
7. Run `cargo test -p waved-provision`.

**Commit:** `fix(provision): harden provisioning HTTP input`

## Task 6: Documentation and acceptance harness

**Files:**

- Modify `docs/WIFI.md`
- Modify `scripts/test-wifi-provisioning.sh`

**Work:**

1. Record the measured daemon/radio probe and correct STA config path.
2. Document native key derivation and in-process DHCP.
3. Make the script accept `ADB_SERIAL`, automate preflight, AP-side assertions,
   credential POST encoding, STA/IPv4 wait, cleanup checks, and secret scan.
4. Keep the phone DHCP/page check as an explicit operator gate.
5. Run shell syntax checks and `git diff --check`.

**Commit:** `docs(wifi): document and automate provisioning acceptance`

## Task 7: Full verification and device deployment

1. Run focused crate tests, workspace tests with `simulate`, clippy, and the
   simulated app build.
2. Build ARM release with `ui,capture-alsa` in the pinned cross image.
3. Inspect ELF architecture and required glibc versions (must be `<= 2.31`).
4. Push beside the running binary, hash-check, atomically replace, and reboot.
5. Verify boot-owned watchdog, visible framebuffer, default-off diagnostic mic
   capture, and stable storage.
6. Verify station initialization and saved-network connectivity.
7. Start provisioning from the UI; verify hostapd, AP IP, DHCP server, and page.
8. Operator joins from a phone, loads the page, and submits real credentials.
9. Verify requested SSID plus IPv4, AP/config cleanup, no PSK in logs, and a
   healthy UI/process.
10. Exercise Cancel immediately. Exercise the timeout with a test-only shortened
    timeout if five unattended minutes would add no distinct code-path coverage;
    otherwise run the real timeout.
11. Reboot and repeat steady-state health checks.

## Rollback

Before replacement, retain the currently deployed binary hash and a local exact
artifact. If the new binary fails to start, restore that artifact immediately.
If AP mode fails to return to STA, stop hostapd, flush `wlan0`, bring it up, and
start `wpa_supplicant` with `/etc/wpa_supplicant.conf`; `udhcpc` follows after
association. A device reboot remains the final safe radio reset.
