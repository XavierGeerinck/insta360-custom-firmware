# Wi-Fi form polish and station-connect fix — implementation plan

**Goal:** Make the captive setup page polished and mobile-friendly, and accept
the camera firmware's informational `wpa_cli add_network` prefix.

**Design:** `docs/superpowers/specs/2026-07-16-wifi-form-connect-fix-design.md`

## Task 1: Harden station command compatibility

**File:** `crates/waved-wifi/src/wpa.rs`

1. Make the command wrapper explicitly pass `-i wlan0` before each operation.
2. Parse the final non-empty `add_network` output line as the ID only when it
   contains ASCII decimal digits exclusively.
3. Cover bare, banner-prefixed, blank-line, empty, `FAIL`, non-numeric, and
   mixed-output cases.
4. Run the Wi-Fi test suite and strict clippy.

**Commit:** `fix(wifi): accept prefixed wpa_cli network id`

## Task 2: Polish the captive setup page

**File:** `crates/waved-provision/src/http.rs`

1. Replace the unstyled form with the approved self-contained clean-light
   mobile design.
2. Keep the Network field editable and retain scanned SSIDs through a datalist.
3. Add an accessible Show/Hide password control, focus styles, local-security
   note, and disabled `Connecting…` submit state.
4. Restyle the acknowledgement page with the escaped submitted SSID and clear
   instructions about the temporary AP disappearing and checking the Wave.
5. Extend rendering tests for the new structure, behavior, and escaping.
6. Run provisioning tests and strict clippy.

**Commit:** `feat(provision): polish captive Wi-Fi form`

## Task 3: Verify and deploy

1. Run focused Wi-Fi/provisioning/UI tests, simulated-app tests/build, shell
   checks, and diff checks.
2. Cross-build the ARM64 `ui,capture-alsa` release and verify glibc `<= 2.31`.
3. Preserve the deployed binary, install atomically, and reboot under the
   watchdog.
4. Open provisioning, verify AP/DHCP/DNS/HTTP and inspect the live page on a
   phone-sized viewport.
5. Have the operator scan the QR and submit credentials; gate success on the
   requested SSID reaching `COMPLETED` with a real IPv4 address.
6. Verify AP/DNS/DHCP/config cleanup, no plaintext submitted password in logs
   or station config, healthy processes/storage, default-off mic capture, and
   reboot persistence.

## Rollback

Keep the current verified QR binary on `/mnt/udisk/debug`. If the new build
fails startup or provisioning, restore that copy atomically and reboot.
