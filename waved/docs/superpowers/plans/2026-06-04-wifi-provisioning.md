# WiFi Fix + Phone/Web Provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the `waved-wifi` backend (7 correctness/security bugs) and add on-device phone/web WiFi provisioning via a new `waved-provision` crate, reachable from the System-settings Wi-Fi row.

**Architecture:** Single radio (`wlan0`) toggles STA↔AP. Tapping Wi-Fi scans (as STA), brings up a WPA2 AP `Wave-XXXX` at `192.168.2.1` via on-device `hostapd`+`dnsmasq`, and serves a one-page form from an embedded tokio HTTP server. `POST /connect` feeds the hardened `wpa::connect`, then reverts to STA. A `ProvisioningService` owns the state machine and publishes `ProvisioningState` to the `StateBus`; the UI renders it.

**Tech Stack:** Rust, tokio (process/sync/time/net), custom LVGL 9 FFI, `wpa_cli`/`wpa_passphrase`/`hostapd`/`dnsmasq` (on-device), `thiserror`.

**Spec:** `waved/docs/superpowers/specs/2026-06-04-wifi-provisioning-design.md`

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/waved-wifi/src/wpa.rs` | `wpa_cli` wrapper, encoding/parsing, error type | Modify |
| `crates/waved-wifi/src/lib.rs` | `WifiService` poll loop + command subscription | Modify |
| `crates/waved-state/src/lib.rs` | `Command` variants, `ProvisioningState`, `StateBus` channel | Modify |
| `crates/waved-provision/Cargo.toml` | New crate manifest | Create |
| `crates/waved-provision/src/lib.rs` | `ProvisioningService` state machine | Create |
| `crates/waved-provision/src/ap.rs` | `ApController` — hostapd/dnsmasq lifecycle | Create |
| `crates/waved-provision/src/http.rs` | Minimal HTTP request parse + page render | Create |
| `crates/waved-ui/src/screens/wifi_provisioning.rs` | On-device provisioning screen | Create |
| `crates/waved-ui/src/screens/mod.rs` | Register new screen module | Modify |
| `crates/waved-ui/src/nav.rs` | `UiScreen::WifiProvisioning` variant | Modify |
| `crates/waved-ui/src/screens/system.rs` | Wire Wi-Fi row → provisioning | Modify |
| `crates/waved-ui/src/lib.rs` | `handle_command` + update loop + status-bar signal | Modify |
| `crates/waved/src/main.rs` | Spawn `ProvisioningService` | Modify |
| `crates/waved/Cargo.toml` + workspace | Add `waved-provision` dependency/member | Modify |
| `waved/docs/WIFI.md` | Provisioning architecture doc | Create |
| `waved/scripts/test-wifi-provisioning.sh` | On-device acceptance | Create |

---

## Task 0: Prerequisite — confirm on-device daemons (BLOCKING)

**No code.** Before any AP work, verify the rootfs has what Approach A assumes.

- [ ] **Step 1: Probe the device**

Run (device on ADB):
```bash
adb shell 'for b in hostapd dnsmasq wpa_passphrase wpa_cli; do printf "%s: " $b; command -v $b || echo MISSING; done'
adb shell 'iw dev 2>/dev/null; iw list 2>/dev/null | grep -A6 "Supported interface modes"'
```
Expected: paths for all four binaries; `iw list` shows `* AP` under supported interface modes.

- [ ] **Step 2: Record the result**

If all present and AP mode supported → proceed with this plan unchanged.
If `hostapd`/`dnsmasq` MISSING → STOP and revisit: either bundle static aarch64 binaries (add a build/deploy task) or fall back to spec Approach C. Note the finding in `waved/docs/WIFI.md` (created in Task 19).

---

## Task 1: `WifiError` + honest `wpa_cli` (OK/FAIL + exit status)

**Files:**
- Modify: `crates/waved-wifi/src/wpa.rs:1-13`
- Modify: `crates/waved-wifi/Cargo.toml` (thiserror already present, line 14 — no change needed)

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `crates/waved-wifi/src/wpa.rs`:
```rust
    #[test]
    fn classify_ok_token() {
        assert!(check_wpa_ok("OK\n").is_ok());
    }

    #[test]
    fn classify_fail_token() {
        assert!(matches!(check_wpa_ok("FAIL\n"), Err(WifiError::CommandFailed)));
    }

    #[test]
    fn classify_passthrough_value() {
        // Non OK/FAIL stdout (e.g. status output, a network id) passes through.
        assert_eq!(check_wpa_ok("3\n").unwrap(), "3\n");
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p waved-wifi --lib classify`
Expected: FAIL — `check_wpa_ok` / `WifiError` not found.

- [ ] **Step 3: Implement**

At the top of `crates/waved-wifi/src/wpa.rs`, replace lines 1-13 with:
```rust
use std::process::Output;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, warn};

const CMD_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum WifiError {
    #[error("wpa_cli reported FAIL")]
    CommandFailed,
    #[error("wpa_cli exited with status {0}")]
    BadExit(i32),
    #[error("command timed out")]
    Timeout,
    #[error("invalid network id from add_network: {0:?}")]
    BadNetworkId(String),
    #[error("invalid credential: {0}")]
    InvalidCredential(&'static str),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Classify wpa_cli stdout. `FAIL` → error; everything else (incl. `OK`,
/// status dumps, numeric ids) passes through unchanged.
pub fn check_wpa_ok(stdout: &str) -> Result<String, WifiError> {
    if stdout.trim() == "FAIL" {
        return Err(WifiError::CommandFailed);
    }
    Ok(stdout.to_string())
}

/// Mask the value following a `psk` arg so passphrases never reach the log.
fn redact_psk(args: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(args.len());
    let mut mask_next = false;
    for &a in args {
        if mask_next {
            out.push("****".to_string());
            mask_next = false;
        } else {
            out.push(a.to_string());
            mask_next = a == "psk";
        }
    }
    out
}

async fn wpa_cli(args: &[&str]) -> Result<String, WifiError> {
    let fut = Command::new("wpa_cli").args(args).output();
    let output: Output = match tokio::time::timeout(CMD_TIMEOUT, fut).await {
        Ok(res) => res?,
        Err(_) => return Err(WifiError::Timeout),
    };
    if !output.status.success() {
        return Err(WifiError::BadExit(output.status.code().unwrap_or(-1)));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    debug!("wpa_cli {:?} → {}", redact_psk(args), stdout.trim());
    check_wpa_ok(&stdout)
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p waved-wifi --lib classify`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/waved-wifi/src/wpa.rs
git commit -m "feat(wifi): WifiError + OK/FAIL classification, exit-status check, PSK log redaction, timeouts"
```

---

## Task 2: SSID hex-encoding + `wpa_passphrase` PSK parsing (escaping-proof)

**Files:**
- Modify: `crates/waved-wifi/src/wpa.rs` (tests module + two pure fns)

Rationale: wpa_supplicant quoted strings can't represent `"`/`\` safely. Encoding the SSID as hex and deriving the PSK via `wpa_passphrase` (which yields a 64-hex PMK, no quoting) handles *any* valid input and stores the PSK hashed, not plaintext.

- [ ] **Step 1: Write failing tests**

Add to the `tests` module:
```rust
    #[test]
    fn ssid_hex_basic() {
        assert_eq!(ssid_to_hex("net"), "6e6574");
    }

    #[test]
    fn ssid_hex_quotes_and_spaces() {
        // A double-quote (0x22) and space (0x20) — would break quoting.
        assert_eq!(ssid_to_hex("a \"b"), "61202262");
    }

    #[test]
    fn parse_passphrase_extracts_psk_hex() {
        let out = "network={\n\tssid=\"x\"\n\t#psk=\"secret\"\n\tpsk=0123abcd\n}\n";
        assert_eq!(parse_wpa_passphrase_psk(out).unwrap(), "0123abcd");
    }

    #[test]
    fn parse_passphrase_ignores_commented_line() {
        let out = "\t#psk=\"plaintext\"\n";
        assert!(parse_wpa_passphrase_psk(out).is_none());
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p waved-wifi --lib ssid_hex parse_passphrase`
Expected: FAIL — functions not found.

- [ ] **Step 3: Implement**

Add to `crates/waved-wifi/src/wpa.rs` (after `wpa_cli`):
```rust
/// Encode an SSID as the lowercase-hex form wpa_supplicant accepts unquoted
/// (`set_network N ssid <hex>`), sidestepping all quoting/escaping issues.
pub fn ssid_to_hex(ssid: &str) -> String {
    ssid.bytes().map(|b| format!("{b:02x}")).collect()
}

/// Extract the derived `psk=<hex>` line from `wpa_passphrase` output,
/// skipping the commented `#psk="..."` plaintext echo.
pub fn parse_wpa_passphrase_psk(output: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if let Some(hex) = t.strip_prefix("psk=") {
            return Some(hex.to_string());
        }
    }
    None
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p waved-wifi --lib ssid_hex parse_passphrase`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/waved-wifi/src/wpa.rs
git commit -m "feat(wifi): hex-encode SSID and derive PSK via wpa_passphrase (quote-safe, hashed-at-rest)"
```

---

## Task 3: Rewrite `connect()` to return `Result` and use the safe encoding

**Files:**
- Modify: `crates/waved-wifi/src/wpa.rs:36-50` (current `connect`/`disconnect`)

- [ ] **Step 1: Write failing tests**

Add to the `tests` module:
```rust
    #[test]
    fn validate_network_id_numeric() {
        assert_eq!(parse_network_id("3\n").unwrap(), "3");
    }

    #[test]
    fn validate_network_id_rejects_fail() {
        assert!(matches!(parse_network_id("FAIL\n"), Err(WifiError::BadNetworkId(_))));
    }

    #[test]
    fn validate_psk_length() {
        assert!(matches!(check_psk_len("short"), Err(WifiError::InvalidCredential(_))));
        assert!(check_psk_len("12345678").is_ok());
        assert!(check_psk_len("").is_ok()); // empty == open network
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p waved-wifi --lib validate_network_id validate_psk`
Expected: FAIL — `parse_network_id`/`check_psk_len` not found.

- [ ] **Step 3: Implement**

Replace `connect` and `disconnect` (current `crates/waved-wifi/src/wpa.rs:36-50`) with:
```rust
/// `add_network` returns a bare network index on success.
pub fn parse_network_id(stdout: &str) -> Result<String, WifiError> {
    let id = stdout.trim();
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        Ok(id.to_string())
    } else {
        Err(WifiError::BadNetworkId(id.to_string()))
    }
}

/// WPA2-PSK passphrases are 8..=63 chars; empty means an open network.
pub fn check_psk_len(psk: &str) -> Result<(), WifiError> {
    if psk.is_empty() || (8..=63).contains(&psk.chars().count()) {
        Ok(())
    } else {
        Err(WifiError::InvalidCredential("psk must be 8..=63 chars"))
    }
}

async fn wpa_passphrase(ssid: &str, psk: &str) -> Result<String, WifiError> {
    let fut = Command::new("wpa_passphrase").arg(ssid).arg(psk).output();
    let output = match tokio::time::timeout(CMD_TIMEOUT, fut).await {
        Ok(res) => res?,
        Err(_) => return Err(WifiError::Timeout),
    };
    if !output.status.success() {
        return Err(WifiError::BadExit(output.status.code().unwrap_or(-1)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_wpa_passphrase_psk(&text)
        .ok_or(WifiError::InvalidCredential("wpa_passphrase produced no psk"))
}

pub async fn connect(ssid: &str, password: &str) -> Result<(), WifiError> {
    if ssid.is_empty() {
        return Err(WifiError::InvalidCredential("empty ssid"));
    }
    check_psk_len(password)?;

    let id = parse_network_id(&wpa_cli(&["add_network"]).await?)?;
    let ssid_hex = ssid_to_hex(ssid);
    wpa_cli(&["set_network", &id, "ssid", &ssid_hex]).await?;

    if password.is_empty() {
        wpa_cli(&["set_network", &id, "key_mgmt", "NONE"]).await?;
    } else {
        let psk_hex = wpa_passphrase(ssid, password).await?;
        wpa_cli(&["set_network", &id, "psk", &psk_hex]).await?;
    }

    wpa_cli(&["select_network", &id]).await?;
    wpa_cli(&["enable_network", &id]).await?;
    wpa_cli(&["save_config"]).await?;
    Ok(())
}

pub async fn disconnect() -> Result<(), WifiError> {
    wpa_cli(&["disconnect"]).await.map(|_| ())
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p waved-wifi --lib`
Expected: PASS (all wpa tests, including the pre-existing parse tests).

- [ ] **Step 5: Commit**

```bash
git add crates/waved-wifi/src/wpa.rs
git commit -m "feat(wifi): connect() returns Result, validates network id + psk, open-network support"
```

---

## Task 4: `scan()`/`status()` return `Result`; add `signal_poll`

**Files:**
- Modify: `crates/waved-wifi/src/wpa.rs:24-34` (`scan`), `:69-72` (`status`)

- [ ] **Step 1: Write failing test**

Add to the `tests` module:
```rust
    #[test]
    fn parse_signal_rssi() {
        let out = "RSSI=-57\nLINKSPEED=72\nNOISE=-92\n";
        assert_eq!(parse_signal(out), Some(-57));
    }

    #[test]
    fn parse_signal_missing() {
        assert_eq!(parse_signal("LINKSPEED=72\n"), None);
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p waved-wifi --lib parse_signal`
Expected: FAIL — `parse_signal` not found.

- [ ] **Step 3: Implement**

Add `parse_signal` near `parse_wpa_status`:
```rust
/// Parse the `RSSI=<dbm>` line from `wpa_cli signal_poll`.
pub fn parse_signal(output: &str) -> Option<i8> {
    output
        .lines()
        .find_map(|l| l.strip_prefix("RSSI="))
        .and_then(|v| v.trim().parse::<i8>().ok())
}
```
Replace `scan` (lines 24-34) with a `Result`-returning version:
```rust
pub async fn scan() -> Result<Vec<String>, WifiError> {
    wpa_cli(&["scan"]).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let output = wpa_cli(&["scan_results"]).await?;
    Ok(parse_scan_results(&output))
}
```
Replace `status` (lines 69-72) with one that also reports signal:
```rust
pub async fn status() -> Option<(String, String, i8)> {
    let output = wpa_cli(&["status"]).await.ok()?;
    let (ssid, ip) = parse_wpa_status(&output)?;
    let signal = match wpa_cli(&["signal_poll"]).await {
        Ok(s) => parse_signal(&s).unwrap_or(0),
        Err(_) => 0,
    };
    Some((ssid, ip, signal))
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p waved-wifi --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-wifi/src/wpa.rs
git commit -m "feat(wifi): scan() returns Result; status() reports RSSI signal strength"
```

---

## Task 5: `WifiService` — signal, roam fix, command subscription

**Files:**
- Modify: `crates/waved-wifi/src/lib.rs:11-64`

- [ ] **Step 1: Write failing test**

Create `crates/waved-wifi/tests/roam.rs`:
```rust
// Pure check of the SSID-change detector used by the poll loop.
use waved_wifi::ssid_changed;

#[test]
fn same_ssid_no_event() {
    assert!(!ssid_changed(Some("A"), "A"));
}

#[test]
fn different_ssid_fires() {
    assert!(ssid_changed(Some("A"), "B"));
}

#[test]
fn first_connect_fires() {
    assert!(ssid_changed(None, "A"));
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p waved-wifi --test roam`
Expected: FAIL — `ssid_changed` not found.

- [ ] **Step 3: Implement**

In `crates/waved-wifi/src/lib.rs`, add a `Command` import and the helper, then rework `run`. Replace lines 6-9 imports with:
```rust
use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use waved_config::WavedConfig;
use waved_state::{Command, DeviceEvent, WifiState};
```
Add after the imports:
```rust
/// True when the connected SSID differs from the last observed one
/// (or this is the first connect). Drives disconnect+reconnect events
/// on roam without a full drop.
pub fn ssid_changed(last: Option<&str>, current: &str) -> bool {
    last != Some(current)
}
```
Replace the `run` method body (lines 30-64) with:
```rust
    pub async fn run(self, mut commands: broadcast::Receiver<Command>) {
        info!("wifi_srv started");
        let mut last_ssid: Option<String> = None;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = tick.tick() => self.poll(&mut last_ssid).await,
                cmd = commands.recv() => {
                    if let Ok(cmd) = cmd { self.handle_command(cmd).await; }
                }
            }
        }
    }

    async fn poll(&self, last_ssid: &mut Option<String>) {
        let state = match wpa::status().await {
            Some((ssid, ip, signal)) => {
                if ssid_changed(last_ssid.as_deref(), &ssid) {
                    if last_ssid.is_some() {
                        let _ = self.events_tx.send(DeviceEvent::WifiDisconnected);
                    }
                    info!("wifi connected: {ssid} ({ip})");
                    let _ = self.events_tx.send(DeviceEvent::WifiConnected(ssid.clone()));
                    *last_ssid = Some(ssid.clone());
                    self.sync_ntp().await;
                }
                WifiState { connected: true, ssid, ip_addr: ip, signal_strength: signal }
            }
            None => {
                if last_ssid.is_some() {
                    info!("wifi disconnected");
                    let _ = self.events_tx.send(DeviceEvent::WifiDisconnected);
                    *last_ssid = None;
                }
                WifiState::default()
            }
        };
        let _ = self.wifi_tx.send(state);
    }

    async fn handle_command(&self, cmd: Command) {
        match cmd {
            Command::WifiConnect { ssid, password } => {
                match wpa::connect(&ssid, &password).await {
                    Ok(()) => info!("wifi connect requested for {ssid}"),
                    Err(e) => warn!("wifi connect failed: {e}"),
                }
            }
            _ => {}
        }
    }
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p waved-wifi`
Expected: PASS (lib + roam tests).

- [ ] **Step 5: Update the simulate stub signature**

In `crates/waved-wifi/src/simulate.rs:18`, change `pub async fn run(self)` to accept and ignore a command receiver so both services share a call shape:
```rust
    pub async fn run(self, _commands: tokio::sync::broadcast::Receiver<waved_state::Command>) {
```

- [ ] **Step 6: Commit**

```bash
git add crates/waved-wifi/src/lib.rs crates/waved-wifi/src/simulate.rs crates/waved-wifi/tests/roam.rs
git commit -m "feat(wifi): WifiService consumes commands (WifiConnect), reports signal, fixes roam events"
```

---

## Task 6: `waved-state` — Command variants, `ProvisioningState`, bus channel

**Files:**
- Modify: `crates/waved-state/src/lib.rs` (`Command` enum ~218-262, `StateBus` ~266-313)

- [ ] **Step 1: Write failing test**

Add to the `tests` module in `crates/waved-state/src/lib.rs`:
```rust
    #[test]
    fn provisioning_channel_defaults_idle() {
        let bus = StateBus::new();
        let rx = bus.provisioning.subscribe();
        assert_eq!(*rx.borrow(), ProvisioningState::Idle);
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p waved-state --lib provisioning_channel`
Expected: FAIL — `bus.provisioning` / `ProvisioningState` not found.

- [ ] **Step 3: Implement**

Add the state type near the other state structs (e.g. after `WifiState`, ~line 143):
```rust
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ProvisioningState {
    /// Not provisioning — normal STA operation.
    #[default]
    Idle,
    /// Scanning for networks before bringing up the AP.
    Scanning,
    /// AP is up; user should join `ssid` and browse to `url`.
    ApActive { ssid: String, passphrase: String, url: String },
    /// Credentials received; reconnecting as STA to `ssid`.
    Connecting { ssid: String },
    /// Provisioning finished — connected to `ssid`.
    Connected { ssid: String },
    /// Provisioning failed; `reason` is user-facing.
    Failed { reason: String },
}
```
Add to the `Command` enum (after `DiscardRecording(Uuid)`, ~line 261):
```rust
    /// UI-only: enter WiFi provisioning (scan → AP → web form).
    OpenWifiProvisioning,
    /// Provisioning web form / WifiService: join this network.
    WifiConnect { ssid: String, password: String },
    /// UI-only: abort provisioning and revert to STA.
    CancelWifiProvisioning,
```
Add the channel to `StateBus`. In the struct (after `wifi`, ~line 271):
```rust
    pub provisioning: Arc<watch::Sender<ProvisioningState>>,
```
In `StateBus::new`, add the channel creation (near the other `watch::channel` calls):
```rust
        let (provisioning_tx, _) = watch::channel(ProvisioningState::default());
```
and in the returned struct literal (after `wifi: ...`):
```rust
            provisioning: Arc::new(provisioning_tx),
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p waved-state --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-state/src/lib.rs
git commit -m "feat(state): ProvisioningState + WiFi provisioning commands + bus channel"
```

---

## Task 7: New crate `waved-provision` — scaffold + HTTP parsing

**Files:**
- Create: `crates/waved-provision/Cargo.toml`, `crates/waved-provision/src/lib.rs`, `crates/waved-provision/src/http.rs`
- Modify: workspace `Cargo.toml` members list

- [ ] **Step 1: Create the manifest**

`crates/waved-provision/Cargo.toml`:
```toml
[package]
name = "waved-provision"
version.workspace = true
edition.workspace = true

[dependencies]
waved-state = { path = "../waved-state" }
waved-wifi = { path = "../waved-wifi" }
tokio = { workspace = true, features = ["sync", "process", "time", "net", "io-util", "rt", "macros"] }
tracing.workspace = true
thiserror.workspace = true
```

- [ ] **Step 2: Register in the workspace**

In the workspace root `Cargo.toml`, add `"crates/waved-provision"` to `members`.

- [ ] **Step 3: Write failing HTTP tests**

`crates/waved-provision/src/http.rs`:
```rust
//! Minimal HTTP/1.1 request parsing + page rendering for provisioning.
//! Only what the one-page form needs: request line, and urlencoded body.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_get_root() {
        let req = parse_request("GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/");
        assert!(req.body.is_empty());
    }

    #[test]
    fn parse_post_with_body() {
        let raw = "POST /connect HTTP/1.1\r\nContent-Length: 19\r\n\r\nssid=Home&psk=secret";
        let req = parse_request(raw).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/connect");
        assert_eq!(req.body, "ssid=Home&psk=secret");
    }

    #[test]
    fn form_decode_pair() {
        let f = parse_form("ssid=My%20Net&psk=p%40ss%2bword");
        assert_eq!(f.get("ssid").unwrap(), "My Net");
        assert_eq!(f.get("psk").unwrap(), "p@ss+word");
    }

    #[test]
    fn page_lists_ssids() {
        let html = render_page(&["HomeNet".to_string(), "Cafe".to_string()]);
        assert!(html.contains("HomeNet"));
        assert!(html.contains("Cafe"));
        assert!(html.starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn page_escapes_html() {
        let html = render_page(&["<script>".to_string()]);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
```

- [ ] **Step 4: Run, verify fail**

Run: `cargo test -p waved-provision --lib http`
Expected: FAIL — module/functions not found.

- [ ] **Step 5: Implement `http.rs`**

Prepend to `crates/waved-provision/src/http.rs` (above the test module):
```rust
use std::collections::HashMap;

pub struct Request {
    pub method: String,
    pub path: String,
    pub body: String,
}

/// Parse a (small, fully-buffered) HTTP request. Returns None if malformed.
pub fn parse_request(raw: &str) -> Option<Request> {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut lines = head.lines();
    let mut parts = lines.next()?.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    Some(Request { method, path, body: body.to_string() })
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Decode an `application/x-www-form-urlencoded` body into key→value.
pub fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (urldecode(k), urldecode(v)))
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render the full HTTP response (status line + headers + body) for the
/// provisioning form, with one `<option>` per scanned SSID.
pub fn render_page(ssids: &[String]) -> String {
    let options: String = ssids
        .iter()
        .map(|s| format!("<option value=\"{0}\">{0}</option>", html_escape(s)))
        .collect();
    let body = format!(
        "<!doctype html><html><head><meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>Wave Wi-Fi Setup</title></head><body style=\"font-family:sans-serif;max-width:20em;margin:2em auto\">\
<h2>Wave Wi-Fi Setup</h2><form method=POST action=/connect>\
<label>Network<br><input list=nets name=ssid required></label>\
<datalist id=nets>{options}</datalist><br><br>\
<label>Password<br><input type=password name=psk></label><br><br>\
<button type=submit>Connect</button></form></body></html>"
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

/// A minimal response body for the post-submit acknowledgement page.
pub fn render_ack(ssid: &str) -> String {
    let body = format!(
        "<!doctype html><html><body style=\"font-family:sans-serif\"><h2>Connecting to {}…</h2>\
<p>You can close this page. The device will join the network.</p></body></html>",
        html_escape(ssid)
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}
```
Create `crates/waved-provision/src/lib.rs` with just the module declaration for now:
```rust
pub mod http;
```

- [ ] **Step 6: Run, verify pass**

Run: `cargo test -p waved-provision --lib http`
Expected: PASS (5 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/waved-provision/ Cargo.toml
git commit -m "feat(provision): new crate scaffold + minimal HTTP parse/render with tests"
```

---

## Task 8: `ApController` — hostapd/dnsmasq lifecycle

**Files:**
- Create: `crates/waved-provision/src/ap.rs`
- Modify: `crates/waved-provision/src/lib.rs` (add `pub mod ap;`)

- [ ] **Step 1: Write failing test**

`crates/waved-provision/src/ap.rs` test module (config generation is the pure, testable part):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostapd_conf_has_wpa2_and_ssid() {
        let c = hostapd_conf("Wave-1A2B", "pass1234");
        assert!(c.contains("ssid=Wave-1A2B"));
        assert!(c.contains("wpa=2"));
        assert!(c.contains("wpa_passphrase=pass1234"));
        assert!(c.contains("interface=wlan0"));
    }

    #[test]
    fn dnsmasq_conf_has_dhcp_range() {
        let c = dnsmasq_conf();
        assert!(c.contains("192.168.2.50,192.168.2.200"));
        assert!(c.contains("interface=wlan0"));
    }

    #[test]
    fn ap_ssid_uses_mac_suffix() {
        assert_eq!(ap_ssid("aa:bb:cc:dd:1a:2b"), "Wave-1A2B");
    }
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p waved-provision --lib ap`
Expected: FAIL — module not declared / functions missing.

- [ ] **Step 3: Implement**

Add `pub mod ap;` to `crates/waved-provision/src/lib.rs`. Then prepend to `ap.rs`:
```rust
//! Brings wlan0 up as a WPA2 AP via on-device hostapd + dnsmasq, and always
//! tears back down to STA. Single radio → AP and STA are mutually exclusive.

use std::time::Duration;
use tokio::process::Command;
use tracing::{info, warn};

pub const AP_IP: &str = "192.168.2.1";
const HOSTAPD_CONF: &str = "/tmp/waved-hostapd.conf";
const DNSMASQ_CONF: &str = "/tmp/waved-dnsmasq.conf";
const IFACE: &str = "wlan0";

/// `Wave-XXXX` from the last 4 hex digits of the MAC (colons stripped, upper).
pub fn ap_ssid(mac: &str) -> String {
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let suffix: String = hex.chars().rev().take(4).collect::<Vec<_>>()
        .into_iter().rev().collect::<String>().to_uppercase();
    format!("Wave-{suffix}")
}

pub fn hostapd_conf(ssid: &str, passphrase: &str) -> String {
    format!(
        "interface={IFACE}\ndriver=nl80211\nssid={ssid}\nhw_mode=g\nchannel=1\n\
auth_algs=1\nwpa=2\nwpa_key_mgmt=WPA-PSK\nrsn_pairwise=CCMP\nwpa_passphrase={passphrase}\n"
    )
}

pub fn dnsmasq_conf() -> String {
    format!(
        "interface={IFACE}\nbind-interfaces\ndhcp-range=192.168.2.50,192.168.2.200,12h\n\
dhcp-option=3,{AP_IP}\nno-resolv\n"
    )
}

async fn run(cmd: &str, args: &[&str]) -> std::io::Result<()> {
    let status = Command::new(cmd).args(args).status().await?;
    if !status.success() {
        warn!("{cmd} {args:?} exited with {status}");
    }
    Ok(())
}

/// RAII-ish controller. `start` is idempotent-ish; `stop` always restores STA.
pub struct ApController {
    pub ssid: String,
    pub passphrase: String,
}

impl ApController {
    pub fn new(mac: &str, passphrase: String) -> Self {
        Self { ssid: ap_ssid(mac), passphrase }
    }

    /// Tear down STA, write configs, start hostapd + dnsmasq, assign AP_IP.
    pub async fn start(&self) -> std::io::Result<()> {
        info!("provisioning AP up: {}", self.ssid);
        // Stop wpa_supplicant's hold on the iface (STA mode).
        let _ = run("wpa_cli", &["disconnect"]).await;
        let _ = run("killall", &["-q", "wpa_supplicant"]).await;
        tokio::fs::write(HOSTAPD_CONF, hostapd_conf(&self.ssid, &self.passphrase)).await?;
        tokio::fs::write(DNSMASQ_CONF, dnsmasq_conf()).await?;
        run("ip", &["addr", "flush", "dev", IFACE]).await?;
        run("ip", &["link", "set", IFACE, "up"]).await?;
        run("ip", &["addr", "add", &format!("{AP_IP}/24"), "dev", IFACE]).await?;
        run("hostapd", &["-B", HOSTAPD_CONF]).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        run("dnsmasq", &["-C", DNSMASQ_CONF]).await?;
        Ok(())
    }

    /// Stop AP daemons and hand the radio back to wpa_supplicant (STA).
    pub async fn stop(&self) -> std::io::Result<()> {
        info!("provisioning AP down → restoring STA");
        let _ = run("killall", &["-q", "dnsmasq"]).await;
        let _ = run("killall", &["-q", "hostapd"]).await;
        run("ip", &["addr", "flush", "dev", IFACE]).await?;
        // S50_ko_install / S60wifi normally start wpa_supplicant; restart it the
        // same way the stock init does (see docs/WIFI.md for the exact line).
        let _ = run(
            "wpa_supplicant",
            &["-B", "-Dnl80211", "-iwlan0", "-c", "/tmp/wpa_supplicant.conf"],
        )
        .await;
        let _ = run("udhcpc", &["-i", IFACE, "-b"]).await;
        Ok(())
    }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p waved-provision --lib ap`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/waved-provision/src/ap.rs crates/waved-provision/src/lib.rs
git commit -m "feat(provision): ApController — hostapd/dnsmasq lifecycle with STA restore"
```

---

## Task 9: `ProvisioningService` — state machine + HTTP server

**Files:**
- Modify: `crates/waved-provision/src/lib.rs`

- [ ] **Step 1: Write failing test**

Add a `tests` module to `crates/waved-provision/src/lib.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrase_is_8_chars_hex() {
        let p = gen_passphrase(0xA1B2C3D4);
        assert_eq!(p.len(), 8);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p waved-provision --lib passphrase`
Expected: FAIL — `gen_passphrase` not found.

- [ ] **Step 3: Implement the service**

Replace `crates/waved-provision/src/lib.rs` contents with:
```rust
pub mod ap;
pub mod http;

use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{info, warn};
use waved_state::{Command, ProvisioningState, StateBus};

const PROV_TIMEOUT: Duration = Duration::from_secs(300); // 5 min

/// Derive a stable 8-hex-char provisioning passphrase from a seed (the MAC's
/// numeric form). Deterministic so the on-screen value matches across restarts
/// within a session; not a secret beyond the provisioning window.
pub fn gen_passphrase(seed: u32) -> String {
    format!("{seed:08x}")
}

pub struct ProvisioningService {
    bus: Arc<StateBus>,
    mac: String,
    mac_seed: u32,
}

impl ProvisioningService {
    pub fn new(bus: Arc<StateBus>, mac: String) -> Self {
        let mac_seed = mac
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect::<String>()
            .get(0..8)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .unwrap_or(0);
        Self { bus, mac, mac_seed }
    }

    pub async fn run(self, mut commands: broadcast::Receiver<Command>) {
        info!("provision_srv started");
        loop {
            match commands.recv().await {
                Ok(Command::OpenWifiProvisioning) => self.provision().await,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    fn set(&self, state: ProvisioningState) {
        let _ = self.bus.provisioning.send(state);
    }

    async fn provision(&self) {
        // 1. Scan as STA.
        self.set(ProvisioningState::Scanning);
        let ssids = waved_wifi::wpa::scan().await.unwrap_or_default();

        // 2. Bring up AP.
        let passphrase = gen_passphrase(self.mac_seed);
        let ap = ap::ApController::new(&self.mac, passphrase.clone());
        if let Err(e) = ap.start().await {
            warn!("AP start failed: {e}");
            self.set(ProvisioningState::Failed { reason: "AP start failed".into() });
            let _ = ap.stop().await;
            return;
        }
        self.set(ProvisioningState::ApActive {
            ssid: ap.ssid.clone(),
            passphrase,
            url: format!("http://{}", ap::AP_IP),
        });

        // 3. Serve the form until creds arrive, timeout, or cancel.
        let creds = tokio::select! {
            c = self.serve(&ssids) => c,
            _ = tokio::time::sleep(PROV_TIMEOUT) => None,
            _ = self.wait_cancel() => None,
        };

        // 4. Tear down AP, restore STA.
        let _ = ap.stop().await;

        // 5. Connect (or report idle/failure).
        match creds {
            Some((ssid, psk)) => {
                self.set(ProvisioningState::Connecting { ssid: ssid.clone() });
                match waved_wifi::wpa::connect(&ssid, &psk).await {
                    Ok(()) => self.set(ProvisioningState::Connected { ssid }),
                    Err(e) => {
                        self.set(ProvisioningState::Failed { reason: format!("connect failed: {e}") });
                    }
                }
            }
            None => self.set(ProvisioningState::Idle),
        }
    }

    /// One-shot HTTP server on AP_IP:80. Returns the submitted (ssid, psk).
    async fn serve(&self, ssids: &[String]) -> Option<(String, String)> {
        let listener = match TcpListener::bind((ap::AP_IP, 80)).await {
            Ok(l) => l,
            Err(e) => {
                warn!("provisioning bind failed: {e}");
                return None;
            }
        };
        loop {
            let (mut sock, _) = listener.accept().await.ok()?;
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.ok()?;
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();
            let req = match http::parse_request(&raw) {
                Some(r) => r,
                None => continue,
            };
            if req.method == "POST" && req.path == "/connect" {
                let form = http::parse_form(&req.body);
                let ssid = form.get("ssid").cloned().unwrap_or_default();
                let psk = form.get("psk").cloned().unwrap_or_default();
                let _ = sock.write_all(http::render_ack(&ssid).as_bytes()).await;
                let _ = sock.shutdown().await;
                if !ssid.is_empty() {
                    return Some((ssid, psk));
                }
            } else {
                let _ = sock.write_all(http::render_page(ssids).as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        }
    }

    async fn wait_cancel(&self) {
        let mut rx = self.bus.commands.subscribe();
        loop {
            match rx.recv().await {
                Ok(Command::CancelWifiProvisioning) => return,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p waved-provision`
Expected: PASS (lib + http + ap tests).

- [ ] **Step 5: Commit**

```bash
git add crates/waved-provision/src/lib.rs
git commit -m "feat(provision): ProvisioningService state machine + one-page HTTP server"
```

---

## Task 10: UI — `WifiProvisioning` screen

**Files:**
- Create: `crates/waved-ui/src/screens/wifi_provisioning.rs`
- Modify: `crates/waved-ui/src/screens/mod.rs:1-8`
- Modify: `crates/waved-ui/src/nav.rs:4-20`

- [ ] **Step 1: Add the nav variant + module registration**

In `crates/waved-ui/src/nav.rs`, add to the `UiScreen` enum (after `Volume,`):
```rust
    /// WiFi provisioning flow — scan → AP info → connecting → result.
    WifiProvisioning,
```
In `crates/waved-ui/src/screens/mod.rs`, add (keep alphabetical-ish with the others):
```rust
pub mod wifi_provisioning;
```

- [ ] **Step 2: Implement the screen**

Create `crates/waved-ui/src/screens/wifi_provisioning.rs`. This mirrors `SystemScreen`'s idiom (root + title + back button) and renders the current `ProvisioningState` via `update_state`. No new widgets.
```rust
//! WiFi provisioning screen — renders ProvisioningState:
//!  - Scanning           → "Scanning…"
//!  - ApActive           → join SSID + passphrase + URL
//!  - Connecting         → spinner + "Connecting to <ssid>…"
//!  - Connected / Failed → result line
//! Back button cancels provisioning and returns to System settings.

use std::ffi::CString;
use std::sync::Arc;
use crate::bindings::*;
use crate::theme::*;
use waved_state::{Command, ProvisioningState, StateBus};

extern "C" {
    static lv_font_montserrat_14: lv_font_t;
    static lv_font_montserrat_16: lv_font_t;
}

unsafe extern "C" fn back_btn_cb(e: *mut lv_event_t) {
    if lv_event_get_code(e) == lv_event_code_t_LV_EVENT_CLICKED {
        let tx = lv_event_get_user_data(e)
            as *const tokio::sync::broadcast::Sender<Command>;
        if !tx.is_null() {
            let _ = (*tx).send(Command::CancelWifiProvisioning);
            let _ = (*tx).send(Command::CloseSettings);
        }
    }
}

pub struct WifiProvisioningScreen {
    bus: Arc<StateBus>,
    body: *mut lv_obj_t,
}

impl WifiProvisioningScreen {
    pub fn new(bus: Arc<StateBus>) -> Self {
        Self { bus, body: std::ptr::null_mut() }
    }

    fn state_text(state: &ProvisioningState) -> String {
        match state {
            ProvisioningState::Idle => "Wi-Fi".to_string(),
            ProvisioningState::Scanning => "Scanning…".to_string(),
            ProvisioningState::ApActive { ssid, passphrase, url } => {
                format!("Join Wi-Fi:\n{ssid}\nPass: {passphrase}\nThen open:\n{url}")
            }
            ProvisioningState::Connecting { ssid } => format!("Connecting to\n{ssid}…"),
            ProvisioningState::Connected { ssid } => format!("Connected to\n{ssid}"),
            ProvisioningState::Failed { reason } => format!("Failed:\n{reason}"),
        }
    }

    /// Refresh the body label from the latest ProvisioningState.
    pub unsafe fn update_state(&mut self, state: &ProvisioningState) {
        if self.body.is_null() { return; }
        let s = CString::new(Self::state_text(state)).unwrap();
        lv_label_set_text(self.body, s.as_ptr());
    }
}

impl super::Screen for WifiProvisioningScreen {
    fn create(&mut self) -> *mut lv_obj_t {
        unsafe {
            let screen = lv_obj_create(std::ptr::null_mut());
            lv_obj_set_size(screen, 400, 304);
            lv_obj_set_style_bg_color(screen, rgb(CLR_BG), 0);
            lv_obj_set_style_bg_opa(screen, 255, 0);
            lv_obj_set_style_border_width(screen, 0, 0);
            lv_obj_remove_flag(screen, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            let title = lv_label_create(screen);
            lv_label_set_text(title, c"Wi-Fi Setup".as_ptr());
            lv_obj_set_style_text_color(title, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(title, &lv_font_montserrat_16, 0);
            lv_obj_align(title, lv_align_t_LV_ALIGN_TOP_MID, 0, 20);

            let btn_back = lv_button_create(screen);
            lv_obj_set_size(btn_back, 56, 64);
            lv_obj_align(btn_back, lv_align_t_LV_ALIGN_TOP_LEFT, 8, 8);
            lv_obj_set_style_bg_opa(btn_back, 0, 0);
            lv_obj_set_style_border_width(btn_back, 0, 0);
            lv_obj_set_style_bg_image_src(
                btn_back,
                b"A:/etc/pspapp/lvgl/icon/btn/btn_back.png\0".as_ptr() as *const _,
                0,
            );
            let back_tx = Box::into_raw(Box::new(self.bus.commands.clone()));
            lv_obj_add_event_cb(btn_back, Some(back_btn_cb),
                lv_event_code_t_LV_EVENT_CLICKED, back_tx as *mut _);

            // Body label centered, multi-line.
            let body = lv_label_create(screen);
            lv_label_set_long_mode(body, lv_label_long_mode_t_LV_LABEL_LONG_WRAP);
            lv_obj_set_width(body, 280);
            lv_obj_set_style_text_align(body, lv_text_align_t_LV_TEXT_ALIGN_CENTER, 0);
            lv_obj_set_style_text_color(body, rgb(CLR_WHITE), 0);
            lv_obj_set_style_text_font(body, &lv_font_montserrat_14, 0);
            lv_obj_align(body, lv_align_t_LV_ALIGN_CENTER, 0, 10);
            let init = Self::state_text(&self.bus.provisioning.borrow());
            let s = CString::new(init).unwrap();
            lv_label_set_text(body, s.as_ptr());
            self.body = body;

            screen
        }
    }
}
```

- [ ] **Step 3: Build (compile check — UI feature)**

Run: `cargo build -p waved-ui --features ui`
Expected: compiles. (If `lv_label_long_mode_t_LV_LABEL_LONG_WRAP` / `lv_text_align_t_*` names differ in the generated bindings, grep `target/.../lvgl_bindings.rs` for the exact symbol and adjust.)

- [ ] **Step 4: Commit**

```bash
git add crates/waved-ui/src/screens/wifi_provisioning.rs crates/waved-ui/src/screens/mod.rs crates/waved-ui/src/nav.rs
git commit -m "feat(ui): WifiProvisioning screen rendering ProvisioningState"
```

---

## Task 11: UI — wire the Wi-Fi row + handle_command + update loop + signal

**Files:**
- Modify: `crates/waved-ui/src/screens/system.rs:33-50,68-73`
- Modify: `crates/waved-ui/src/lib.rs` (handle_command ~275-410, update loop ~239-267, status-bar block ~140-160, screen construction ~107-114, handle_command params)

- [ ] **Step 1: Add the row action**

In `crates/waved-ui/src/screens/system.rs`, extend `RowAction` (line 33-38):
```rust
#[derive(Clone, Copy)]
enum RowAction {
    OpenAudioMode,
    OpenWifiProvisioning,
    PowerOff,
    Bookmark, // placeholder for rows whose feature isn't implemented
}
```
Change the Wi-Fi row (line 45):
```rust
    ("Wi-Fi",           RowAction::OpenWifiProvisioning),
```
Add the match arm in `row_btn_cb` (after `OpenAudioMode`, line 69):
```rust
        RowAction::OpenWifiProvisioning => Command::OpenWifiProvisioning,
```

- [ ] **Step 2: Construct the screen + extend handle_command params**

In `crates/waved-ui/src/lib.rs`, add to the screen construction block (~line 107):
```rust
    let mut wifi_prov  = screens::wifi_provisioning::WifiProvisioningScreen::new(Arc::clone(&bus));
```
Add `wifi_prov: &mut screens::wifi_provisioning::WifiProvisioningScreen,` to the `handle_command` signature (after `system:` param, ~line 285) and pass `&mut wifi_prov,` at the call site (~line 187, after `&mut system,`).

- [ ] **Step 3: Add the navigation arm**

In `handle_command` (alongside `Command::OpenSystemSettings`, ~line 329):
```rust
        Command::OpenWifiProvisioning => {
            let root = wifi_prov.create();
            unsafe { lv_screen_load(root); }
            nav.navigate_to(UiScreen::WifiProvisioning);
            nav.set_root(root);
        }
```
(`lv_screen_load` matches the existing `OpenSystemSettings` arm pattern — confirm by reading line 329-333 and mirroring it exactly, including whether it calls `lv_screen_load`.)

- [ ] **Step 4: Render updates + auto-leave on Idle**

In the per-screen update match (~line 250, replacing `UiScreen::System => {}`), add:
```rust
            UiScreen::WifiProvisioning => {
                let prov = bus.provisioning.borrow().clone();
                unsafe { wifi_prov.update_state(&prov); }
            }
```

- [ ] **Step 5: Real signal in the status bar**

The status bar currently takes a `bool` for wifi (`status_bar.update(new_batt, new_wifi, new_bt, new_muted)`, ~line 152). Leave the icon boolean as-is for this plan (signal-strength icon tiers are a separate UI asset task); `WifiState.signal_strength` now carries real data for any future tiering. Add a one-line comment at line 152:
```rust
                // wifi.signal_strength now carries real RSSI (see waved-wifi); icon
                // tiering by strength is a follow-up UI-asset task.
```

- [ ] **Step 6: Build**

Run: `cargo build -p waved-ui --features ui`
Expected: compiles.

- [ ] **Step 7: Commit**

```bash
git add crates/waved-ui/src/screens/system.rs crates/waved-ui/src/lib.rs
git commit -m "feat(ui): wire Wi-Fi row → provisioning screen + state-driven updates"
```

---

## Task 12: Wire services in `main.rs`

**Files:**
- Modify: `crates/waved/src/main.rs:529-547`
- Modify: `crates/waved/Cargo.toml` (add `waved-provision`)

- [ ] **Step 1: Add the dependency**

In `crates/waved/Cargo.toml`, under `[dependencies]`:
```toml
waved-provision = { path = "../waved-provision" }
```

- [ ] **Step 2: Update WifiService spawn + add ProvisioningService**

In `crates/waved/src/main.rs`, the `#[cfg(not(feature = "simulate"))]` WiFi block (~532-538) must pass a command receiver to `run` and spawn the provisioning service. Replace that block with:
```rust
    #[cfg(not(feature = "simulate"))]
    {
        let wifi_cmds = bus.commands.subscribe();
        let wifi_svc = waved_wifi::WifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
            &config,
        );
        tokio::spawn(async move { wifi_svc.run(wifi_cmds).await });

        // WiFi MAC drives the provisioning AP SSID/passphrase. Read it from the
        // interface; fall back to a constant if unavailable (see docs/WIFI.md).
        let mac = std::fs::read_to_string("/sys/class/net/wlan0/address")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "00:00:00:00:00:00".to_string());
        let prov_cmds = bus.commands.subscribe();
        let prov_svc = waved_provision::ProvisioningService::new(Arc::clone(&bus), mac);
        tokio::spawn(async move { prov_svc.run(prov_cmds).await });
    }
```
In the `#[cfg(feature = "simulate")]` WiFi block (~540-547), update the sim `run` call to pass a receiver:
```rust
        let sim_cmds = bus.commands.subscribe();
        tokio::spawn(async move { sim_wifi.run(sim_cmds).await });
```

- [ ] **Step 3: Fix the boot_sequence integration test**

`crates/waved/tests/boot_sequence.rs:50` calls `SimWifiService::run`. Update the call to pass a receiver (read the test first; mirror how it builds `bus`). Example:
```rust
        let sim_cmds = bus.commands.subscribe();
        let sim_wifi = waved_wifi::simulate::SimWifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
        );
        tokio::spawn(async move { sim_wifi.run(sim_cmds).await });
```

- [ ] **Step 4: Build the whole workspace + run tests**

Run: `cargo build --features simulate -p waved && cargo test -p waved-wifi -p waved-state -p waved-provision`
Expected: builds; all unit tests pass.

- [ ] **Step 5: Build the real target (cross)**

Run:
```bash
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```
Expected: compiles for aarch64.

- [ ] **Step 6: Commit**

```bash
git add crates/waved/src/main.rs crates/waved/Cargo.toml crates/waved/tests/boot_sequence.rs
git commit -m "feat(waved): spawn ProvisioningService; pass command channel to WifiService"
```

---

## Task 13: Docs — `waved/docs/WIFI.md`

**Files:**
- Create: `waved/docs/WIFI.md`
- Modify: `waved/CLAUDE.md` (add WIFI.md to the Device Hardware References table)

- [ ] **Step 1: Write the doc**

Create `waved/docs/WIFI.md` covering: the ATBM6031/SDIO stack; the single-radio STA↔AP constraint; the provisioning state machine (`Idle → Scanning → ApActive → Connecting → Connected|Failed`); AP network layout (`192.168.2.1`, DHCP `.50–.200`, WPA2 with on-screen passphrase); the exact on-device commands used (`wpa_cli`, `wpa_passphrase`, `hostapd -B`, `dnsmasq -C`, the STA-restore `wpa_supplicant` line); the Task 0 probe result (which daemons exist); and the security decisions (WPA2 AP, no PSK in logs, PSK hashed via `wpa_passphrase`). Reference the spec.

- [ ] **Step 2: Link it from CLAUDE.md**

Add a row to the `## Device Hardware References` table in `waved/CLAUDE.md`:
```markdown
| Wi-Fi / provisioning | [`docs/WIFI.md`](docs/WIFI.md)                         |
```

- [ ] **Step 3: Commit**

```bash
git add waved/docs/WIFI.md waved/CLAUDE.md
git commit -m "docs(wifi): provisioning architecture, daemon deps, security decisions"
```

---

## Task 14: On-device acceptance script

**Files:**
- Create: `waved/scripts/test-wifi-provisioning.sh`

- [ ] **Step 1: Write the script**

Create `waved/scripts/test-wifi-provisioning.sh` (follows the `scripts/test-*.sh` convention; runs over ADB):
```bash
#!/usr/bin/env bash
# Acceptance: trigger provisioning, assert AP + page, POST creds, assert STA.
# Usage: ./test-wifi-provisioning.sh <test-ssid> <test-psk>
set -euo pipefail
SSID="${1:?test ssid}"; PSK="${2:?test psk}"

echo "[1/4] triggering provisioning (tap Wi-Fi row equivalent)…"
# waved listens on the command bus; simulate the UI command via the test hook
# documented in docs/WIFI.md, or tap the row on-device, then:
sleep 8  # scan + AP bring-up

echo "[2/4] asserting AP iface at 192.168.2.1…"
adb shell 'ip addr show wlan0 | grep -q 192.168.2.1' \
  && echo "  AP IP up" || { echo "  FAIL: no AP IP"; exit 1; }
adb shell 'pgrep hostapd >/dev/null' && echo "  hostapd running" || { echo "  FAIL: hostapd"; exit 1; }

echo "[3/4] asserting form reachable + POST creds…"
# From a host on the AP, or via the device loopback:
adb shell "curl -s -o /dev/null -w '%{http_code}' http://192.168.2.1/" | grep -q 200 \
  && echo "  GET / 200" || { echo "  FAIL: form not served"; exit 1; }
adb shell "curl -s -X POST --data 'ssid=${SSID}&psk=${PSK}' http://192.168.2.1/connect" >/dev/null \
  && echo "  POST /connect accepted"

echo "[4/4] asserting STA reconnect within 30s…"
for i in $(seq 1 30); do
  if adb shell "wpa_cli status 2>/dev/null | grep -q 'wpa_state=COMPLETED'"; then
    echo "  STA reconnected"; exit 0
  fi
  sleep 1
done
echo "  FAIL: did not reconnect as STA"; exit 1
```

- [ ] **Step 2: Make executable + commit**

```bash
chmod +x waved/scripts/test-wifi-provisioning.sh
git add waved/scripts/test-wifi-provisioning.sh
git commit -m "test(wifi): on-device provisioning acceptance script"
```

---

## Task 15: Final verification

- [ ] **Step 1: Full workspace test**

Run: `cargo test --workspace --exclude waved-ui`
Expected: all PASS. (Exclude `waved-ui` from host test run only if its LVGL FFI needs the device cross-target; otherwise include it.)

- [ ] **Step 2: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Cross-build for device**

Run:
```bash
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```
Expected: success.

- [ ] **Step 4: Deploy + on-device acceptance** (requires hardware)

Deploy per `waved/CLAUDE.md`, then run `waved/scripts/test-wifi-provisioning.sh <ssid> <psk>`. Expected: all 4 steps pass.

- [ ] **Step 5: Finishing the branch**

Use `superpowers:finishing-a-development-branch` to open the PR (the same-PR docs rule is satisfied by Task 13).

---

## Self-Review Notes

- **Spec coverage:** §2 → Tasks 1–5; §3 → Tasks 7–9; §4 → Task 6; §5 → Tasks 10–11; §6 → Tasks 1 (PSK redaction), 2/3 (`wpa_passphrase` hashing), 8 (WPA2 AP); §7 → Task 0 (probe); §8 → Tasks (unit tests inline) + 14 (acceptance); §9 → Task 13.
- **Escaping note:** The spec said "escape `"`/`\`"; planning improved this to hex-SSID + `wpa_passphrase`-derived PSK, which is quote-proof for all valid inputs and hashes the PSK at rest (also serves §6). `connect` still rejects out-of-range passphrases (`check_psk_len`).
- **Type consistency:** `WifiService::run(self, broadcast::Receiver<Command>)` and `SimWifiService::run(self, _)` share the call shape (Tasks 5, 12). `ProvisioningState` variants used identically in Tasks 6, 9, 10. `Command::{OpenWifiProvisioning, WifiConnect, CancelWifiProvisioning}` defined in Task 6, consumed in Tasks 5, 9, 10, 11.
- **Open risk:** Task 0 gates the AP path. The exact STA-restore command in `ap.rs::stop` must be reconciled with the device's real init (`S50_ko_install`/`S60wifi`) — capture it in `docs/WIFI.md` during Task 13.
