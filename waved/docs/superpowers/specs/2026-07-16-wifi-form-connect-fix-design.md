# Wi-Fi form polish and station-connect compatibility

## Goal

Make the captive setup page feel like a finished mobile product and make a
submitted network connect successfully on the Wave firmware's `wpa_cli`
variant.

## Observed failure

The deployed camera accepts a phone connection to its temporary AP, serves the
captive form, and receives credentials. Station connection then fails with:

```text
invalid network id from add_network: "Selected interface 'wlan0'\n1"
```

The current `parse_network_id()` expects all trimmed `add_network` output to be
one numeric token. The stock tool prints an informational interface-selection
line before the numeric network ID, so valid output is rejected.

## Command compatibility design

All `wpa_cli` invocations will explicitly select the owned interface with
`-i wlan0`. This removes ambiguity on systems with more than one control
socket and normally suppresses the interface-selection banner.

`parse_network_id()` will also tolerate informational output by inspecting
non-empty lines and accepting the final line only when it consists entirely of
ASCII decimal digits. It will reject empty output, `FAIL`, a non-numeric final
line, and output whose final line mixes a number with other text. This keeps
the parser compatible with the measured firmware without binding it to the
exact wording of the banner.

The parser returns the ID as a string because subsequent `wpa_cli` commands use
it as an argument. No command output or error will include the submitted home
Wi-Fi password.

## Setup-page visual design

The page will use the approved clean-light direction:

- a soft neutral page background and centered white setup card;
- compact Wave identity treatment using CSS only;
- clear title and one-sentence instruction;
- an editable Network input backed by a datalist of scanned SSID suggestions,
  preserving support for hidden or unlisted networks;
- a Password input with a Show/Hide button;
- a large, high-contrast Connect button;
- concise local-security copy explaining that credentials stay on the device;
- responsive sizing, comfortable touch targets, visible keyboard focus, and a
  mobile viewport declaration.

The page remains a single self-contained HTTP response. It will not depend on
external fonts, images, JavaScript libraries, stylesheets, or internet access.
SSID values will continue to be HTML-escaped before entering attributes or
visible text.

Client-side scripting will be limited to toggling password visibility and
changing the button to a disabled `Connecting…` state on form submission. The
server remains authoritative for SSID and password validation.

## Submission and confirmation flow

`POST /connect` retains the existing URL-encoded form contract and bounded
request handling. After valid credentials arrive, the server responds with a
polished confirmation page before tearing down the temporary AP. It will say:

- the Wave is connecting to the HTML-escaped submitted network name;
- the temporary Wave Wi-Fi will disappear;
- the user can close the page and check the Wave screen for the result.

The server will make a best-effort write and socket shutdown as it does today.
The confirmation cannot promise to display on every phone because the AP must
switch modes; the on-device `Connecting`, `Connected`, and `Failed` states
remain the source of truth.

## Error handling

- Invalid form framing or credentials retain explicit HTTP 4xx responses.
- An empty or malformed `add_network` result produces `BadNetworkId` without
  attempting any `set_network` command.
- A valid banner followed by a numeric ID proceeds normally.
- Connection failure remains visible on the Wave screen and does not expose
  the submitted password.
- AP, DHCP, DNS, HTTP, and station cleanup behavior is unchanged.

## Verification

Automated coverage will include:

1. bare numeric, banner-prefixed numeric, and extra-blank-line network IDs;
2. empty, `FAIL`, non-numeric, and mixed final-line rejection;
3. explicit `wlan0` selection in the `wpa_cli` command wrapper;
4. HTML escaping of SSID suggestions and confirmation text;
5. editable network suggestions, password Show/Hide controls, mobile viewport,
   submission state, and security note in the rendered page;
6. the existing provisioning, Wi-Fi, UI, and simulated-app suites;
7. an ARM64 release build with the device-compatible glibc ceiling;
8. deployment, QR join, form submit, requested-SSID plus IPv4 gate, AP cleanup,
   credential-secret scan, default-off mic capture, and reboot persistence.

## Scope

This change does not add internet dependencies, an on-device keyboard, async
browser polling, enterprise Wi-Fi, WPS, or a new multi-step flow. It fixes the
measured station-connect incompatibility and polishes the existing one-page
provisioning experience.
