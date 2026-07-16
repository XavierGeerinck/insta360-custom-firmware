# Wi-Fi status menu and saved-network auto-connect

## Goal

Turn the existing Wi-Fi row into a non-disruptive management screen that shows
the current connection and provides an explicit Connect action for adding or
changing networks. Preserve multiple known networks so the Wave reconnects
automatically at boot or after a disconnect.

## Approved behavior

- Opening Settings → Wi-Fi does not interrupt the current connection.
- When connected, the screen shows the current SSID, connection status, signal
  quality, and IPv4 address.
- When disconnected, it shows a clear Not connected state.
- A `Connect to another network` button immediately starts the existing QR and
  captive-portal provisioning flow. There is no confirmation dialog.
- The screen does not list every saved network in this version.
- The Wave automatically connects only to enabled, previously saved profiles.
  It never joins an unknown network, including an unknown open network.
- A newly provisioned profile is preferred for the next selection and is used
  immediately at the end of provisioning.
- A working connection is not interrupted merely because a higher-priority
  saved network later becomes visible. Selection happens at boot, after
  disconnect, or after the user explicitly starts provisioning.
- Profiles already marked disabled remain disabled unless the user explicitly
  provisions that same SSID again.

## Command and screen separation

The current `OpenWifiProvisioning` command has two jobs: it navigates the UI and
starts the disruptive AP flow. These responsibilities will be separated:

- `OpenWifiSettings` is UI-only and opens the status/management screen.
- `OpenWifiProvisioning` starts provisioning and keeps its existing service
  meaning. The Connect button emits this command while the screen is already
  open.

The System settings Wi-Fi row will emit `OpenWifiSettings`. Existing command
consumers remain isolated: the provisioning service ignores the UI-only
command, and the UI can recreate the screen in provisioning mode when Connect
is tapped.

## On-device UI

The 400×304 round-screen safe area will contain:

1. a `Wi-Fi` title below the persistent status overlay;
2. a central connection card containing:
   - `Connected` or `Not connected`,
   - the current SSID when connected,
   - signal quality expressed as `Excellent`, `Good`, `Fair`, or `Weak`,
   - the current IPv4 address;
3. a wide `Connect to another network` button;
4. the existing bottom Back button.

During provisioning, the screen continues to show Scanning, the live QR and
fallback credentials, Connecting, and failure states. After successful
provisioning, the live `WifiState` becomes the management screen's source of
truth. Labels are updated only when either the provisioning state or Wi-Fi
state changes, avoiding work on every UI tick.

Signal labels use deterministic RSSI thresholds:

- `>= -55 dBm`: Excellent
- `>= -67 dBm`: Good
- `>= -75 dBm`: Fair
- below `-75 dBm`: Weak

## Saved-profile selection

`wpa_supplicant` remains the sole automatic network selector. The application
will not add a competing scan/retry state machine.

When credentials are submitted:

1. list saved profiles and their flags;
2. reuse a profile with the same decoded SSID when present, otherwise add one;
3. configure the submitted SSID and derived 64-hex PMK (or `key_mgmt=NONE` for
   an explicitly submitted open network);
4. set its priority to one greater than the highest saved priority, with a
   bounded fallback if priorities are already at the supported maximum;
5. enable only that selected profile;
6. persist the configuration and request reassociation.

The implementation will stop using `select_network`, because that command
disables every other profile and prevents automatic fallback. It will not call
`enable_network all`; previously disabled profiles therefore remain disabled.
At boot and after link loss, the normal enabled-profile behavior of
`wpa_supplicant` chooses the highest-priority available saved network.

Re-provisioning a disabled profile is an explicit user choice, so that matching
profile is updated, enabled, and promoted. No unrelated disabled profile is
changed.

## Failure handling

- Failure to enumerate or inspect profiles aborts the credential update and is
  shown through the existing provisioning failure state.
- Malformed profile IDs or priorities are rejected or treated as unset; no
  untrusted output is passed back as a command argument.
- If configuration fails after a brand-new profile is added, that incomplete
  profile is removed on a best-effort basis before returning the error.
- Existing profiles are never globally enabled or disabled as error recovery.
- AP/DHCP/DNS/HTTP cleanup and station restoration remain mandatory on every
  provisioning exit path.
- Submitted passwords remain absent from logs. Newly written PSKs remain
  derived 64-hex PMKs, and the station configuration is tightened to mode
  `0600` after persistence.

## Verification

Automated coverage will include:

1. saved-profile list parsing, including banners, current/disabled flags,
   malformed rows, and SSIDs containing spaces;
2. priority parsing and bounded next-priority selection;
3. matching-profile reuse and preservation of unrelated disabled profiles;
4. the absence of `select_network` and use of enable-selected, save, and
   reassociate behavior;
5. connected/disconnected menu text and RSSI quality thresholds;
6. Connect and Back command routing;
7. existing Wi-Fi, provisioning, UI, and simulated-app suites;
8. ARM64/glibc verification and boot-watchdog deployment;
9. live current SSID/status/signal/IP rendering without dropping Wi-Fi;
10. provisioning a second network, automatic connection to each saved network
    when it is the available one, persistence across reboot, and no switching
    solely because a preferred profile appears while the current link works;
11. AP/service/config cleanup, credential-secret scan, storage/process health,
    and default-off mic capture.

## Scope

This version does not expose a saved-network list, Forget action, manual
priority editor, confirmation dialog, unknown-network auto-join, or an
application-managed scan loop.
