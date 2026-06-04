use std::process::Output;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tracing::debug;

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

pub fn parse_scan_results(output: &str) -> Vec<String> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| line.split('\t').nth(4).map(String::from))
        .filter(|ssid| !ssid.is_empty())
        .collect()
}

pub async fn scan() -> Result<Vec<String>, WifiError> {
    wpa_cli(&["scan"]).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let output = wpa_cli(&["scan_results"]).await?;
    Ok(parse_scan_results(&output))
}

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

pub fn parse_wpa_status(output: &str) -> Option<(String, String)> {
    let mut ssid = None;
    let mut ip = None;
    for line in output.lines() {
        if let Some(val) = line.strip_prefix("ssid=") {
            ssid = Some(val.to_string());
        }
        if let Some(val) = line.strip_prefix("ip_address=") {
            ip = Some(val.to_string());
        }
    }
    match (ssid, ip) {
        (Some(s), Some(i)) => Some((s, i)),
        _ => None,
    }
}

/// Parse the `RSSI=<dbm>` line from `wpa_cli signal_poll`.
pub fn parse_signal(output: &str) -> Option<i8> {
    output
        .lines()
        .find_map(|l| l.strip_prefix("RSSI="))
        .and_then(|v| v.trim().parse::<i8>().ok())
}

pub async fn status() -> Option<(String, String, i8)> {
    let output = wpa_cli(&["status"]).await.ok()?;
    let (ssid, ip) = parse_wpa_status(&output)?;
    let signal = match wpa_cli(&["signal_poll"]).await {
        Ok(s) => parse_signal(&s).unwrap_or(0),
        Err(_) => 0,
    };
    Some((ssid, ip, signal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connected_status() {
        let output = "bssid=aa:bb:cc:dd:ee:ff\nfreq=5180\nssid=MyNetwork\nip_address=192.168.1.42\nwpa_state=COMPLETED\n";
        let (ssid, ip) = parse_wpa_status(output).unwrap();
        assert_eq!(ssid, "MyNetwork");
        assert_eq!(ip, "192.168.1.42");
    }

    #[test]
    fn parse_disconnected_status() {
        let output = "wpa_state=DISCONNECTED\n";
        assert!(parse_wpa_status(output).is_none());
    }

    #[test]
    fn parse_ssid_only_no_ip() {
        let output = "ssid=MyNetwork\nwpa_state=COMPLETED\n";
        assert!(parse_wpa_status(output).is_none());
    }

    #[test]
    fn parse_scan_results_normal() {
        let output = "bssid / frequency / signal level / flags / ssid\naa:bb:cc:dd:ee:ff\t5180\t-45\t[WPA2-PSK]\tHomeWifi\n11:22:33:44:55:66\t2437\t-70\t[WPA2-PSK]\tOffice\n";
        let ssids = parse_scan_results(output);
        assert_eq!(ssids, vec!["HomeWifi", "Office"]);
    }

    #[test]
    fn parse_scan_results_empty() {
        let output = "bssid / frequency / signal level / flags / ssid\n";
        assert!(parse_scan_results(output).is_empty());
    }

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

    #[test]
    fn parse_signal_rssi() {
        let out = "RSSI=-57\nLINKSPEED=72\nNOISE=-92\n";
        assert_eq!(parse_signal(out), Some(-57));
    }

    #[test]
    fn parse_signal_missing() {
        assert_eq!(parse_signal("LINKSPEED=72\n"), None);
    }
}
