# Wi-Fi status menu and saved-network auto-connect — implementation plan

**Goal:** Make Settings → Wi-Fi a non-disruptive connection-status screen and
preserve multiple enabled saved profiles for automatic fallback.

**Design:** `docs/superpowers/specs/2026-07-16-wifi-status-auto-connect-design.md`

## Task 1: Separate menu navigation from provisioning

**Files:**

- Modify `crates/waved-state/src/lib.rs`
- Modify `crates/waved-ui/src/screens/system.rs`
- Modify `crates/waved-ui/src/lib.rs`
- Modify `crates/waved-ui/src/screens/wifi_provisioning.rs`

1. Add `Command::OpenWifiSettings` as a UI-only navigation command.
2. Make the System Wi-Fi row emit it instead of starting provisioning.
3. Add a Connect button on the Wi-Fi screen that emits the existing
   `OpenWifiProvisioning` command immediately.
4. Keep Back cancellation safe in both status and provisioning modes.
5. Add command-routing tests where practical.

**Commit:** `feat(ui): separate Wi-Fi status from provisioning`

## Task 2: Preserve and prioritize saved profiles

**File:** `crates/waved-wifi/src/wpa.rs`

1. Add strict parsing for `list_networks` output, IDs, SSIDs, and flags.
2. Add strict priority parsing and bounded next-priority selection.
3. Reuse a matching SSID profile or add a new one.
4. Configure credentials and promote the chosen profile above saved profiles.
5. Enable only the chosen profile, save, and reassociate without
   `select_network` or `enable_network all`.
6. Best-effort remove a newly added incomplete profile on failure.
7. Tighten the saved config to mode `0600` after persistence.
8. Test malformed output, profile matching, priority selection, and command
   sequence helpers; run Wi-Fi tests and strict clippy.

**Commit:** `feat(wifi): preserve saved network fallback`

## Task 3: Render live connection management state

**Files:**

- Modify `crates/waved-ui/src/screens/wifi_provisioning.rs`
- Modify `crates/waved-ui/src/lib.rs`

1. Add a circular-safe connection card with status, SSID, signal quality, and
   IPv4 address.
2. Show `Not connected` when no complete station state exists.
3. Hide management controls while Scanning, AP QR, or Connecting is active.
4. Return to live `WifiState` after success; preserve failure feedback with a
   Connect retry action.
5. Cache both provisioning and Wi-Fi states to avoid per-tick LVGL rewrites.
6. Test connected/disconnected text and RSSI thresholds; run UI tests.

**Commit:** `feat(ui): show live Wi-Fi connection status`

## Task 4: Documentation and full verification

**Files:**

- Modify `docs/WIFI.md`
- Modify `scripts/test-wifi-provisioning.sh` if acceptance assertions need
  extending.

1. Document non-disruptive menu behavior and saved-profile selection rules.
2. Run Wi-Fi/provisioning/UI tests, strict clippy for changed backend crates,
   simulated-app tests/build, shell checks, and diff checks.
3. Cross-build ARM64 `ui,capture-alsa` and verify glibc `<= 2.31`.
4. Preserve the current binary, install atomically, and reboot under the
   watchdog.
5. Verify opening Wi-Fi leaves the current link intact and displays the exact
   live SSID/status/signal/IP.
6. Verify Connect starts QR provisioning without confirmation.
7. Provision a second known network and test reconnect selection at boot/link
   loss while unrelated disabled profiles remain disabled.
8. Run AP cleanup, secret, permissions, process, storage, default-off capture,
   and reboot gates.

**Commit:** `docs(wifi): document saved-network auto-connect`

## Rollback

Keep the current verified native-selector binary under `/mnt/udisk/debug`.
Restore it atomically and reboot if the new menu, profile persistence, or boot
selection fails.
