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

pub async fn scan() -> Vec<String> {
    if let Err(e) = wpa_cli(&["scan"]).await {
        warn!("wifi scan trigger failed: {e}");
        return vec![];
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    match wpa_cli(&["scan_results"]).await {
        Ok(output) => parse_scan_results(&output),
        Err(_) => vec![],
    }
}

pub async fn connect(ssid: &str, password: &str) -> bool {
    let id = match wpa_cli(&["add_network"]).await {
        Ok(s) => s.trim().to_string(),
        Err(_) => return false,
    };
    let _ = wpa_cli(&["set_network", &id, "ssid", &format!("\"{}\"", ssid)]).await;
    let _ = wpa_cli(&["set_network", &id, "psk", &format!("\"{}\"", password)]).await;
    let _ = wpa_cli(&["enable_network", &id]).await;
    let _ = wpa_cli(&["save_config"]).await;
    true
}

pub async fn disconnect() -> bool {
    wpa_cli(&["disconnect"]).await.is_ok()
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

pub async fn status() -> Option<(String, String)> {
    let output = wpa_cli(&["status"]).await.ok()?;
    parse_wpa_status(&output)
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
}
