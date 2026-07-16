use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::fmt::Write as _;
use std::net::Ipv4Addr;
use std::process::Output;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tracing::debug;

const CMD_TIMEOUT: Duration = Duration::from_secs(10);
const IFACE: &str = "wlan0";
const SUPPLICANT_CONF: &str = "/etc/wpa_supplicant.conf";
static STATION_SUSPENDED: AtomicBool = AtomicBool::new(false);

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
    #[error("timed out waiting for Wi-Fi connection")]
    ConnectionTimeout,
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

async fn command_output(program: &str, args: &[&str]) -> Result<Output, WifiError> {
    let fut = Command::new(program).args(args).output();
    let output = match tokio::time::timeout(CMD_TIMEOUT, fut).await {
        Ok(res) => res?,
        Err(_) => return Err(WifiError::Timeout),
    };
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        debug!("{program} exited with status {code}");
        return Err(WifiError::BadExit(code));
    }
    Ok(output)
}

async fn process_running(name: &str) -> Result<bool, WifiError> {
    let fut = Command::new("pidof").arg(name).output();
    match tokio::time::timeout(CMD_TIMEOUT, fut).await {
        Ok(Ok(output)) => Ok(output.status.success()),
        Ok(Err(e)) => Err(WifiError::Io(e)),
        Err(_) => Err(WifiError::Timeout),
    }
}

/// Start the station supplicant when normal STA ownership is active.
/// Idempotent so the Wi-Fi poll loop can use it as its recovery primitive.
pub async fn ensure_station() -> Result<(), WifiError> {
    if STATION_SUSPENDED.load(Ordering::SeqCst) {
        return Ok(());
    }
    command_output("ip", &["link", "set", IFACE, "up"]).await?;
    if !process_running("wpa_supplicant").await? {
        command_output(
            "wpa_supplicant",
            &["-B", "-Dnl80211", "-i", IFACE, "-c", SUPPLICANT_CONF],
        )
        .await?;
    }
    Ok(())
}

/// Stop station processes and prevent the background poller from restarting
/// them while the single radio is owned by the provisioning AP.
pub async fn stop_station() -> Result<(), WifiError> {
    STATION_SUSPENDED.store(true, Ordering::SeqCst);
    if process_running("udhcpc").await? {
        command_output("killall", &["-TERM", "udhcpc"]).await?;
    }
    if process_running("wpa_supplicant").await? {
        command_output("killall", &["-TERM", "wpa_supplicant"]).await?;
    }
    Ok(())
}

/// Return radio ownership to STA mode after provisioning.
pub async fn restore_station() -> Result<(), WifiError> {
    STATION_SUSPENDED.store(false, Ordering::SeqCst);
    ensure_station().await
}

pub fn station_is_suspended() -> bool {
    STATION_SUSPENDED.load(Ordering::SeqCst)
}

/// Start the BusyBox DHCP client after association. It backgrounds itself;
/// process detection prevents repeated clients from accumulating.
pub async fn ensure_dhcp() -> Result<(), WifiError> {
    if station_is_suspended() || process_running("udhcpc").await? {
        return Ok(());
    }
    command_output("udhcpc", &["-i", IFACE, "-b"]).await?;
    Ok(())
}

/// Encode an SSID as the lowercase-hex form wpa_supplicant accepts unquoted
/// (`set_network N ssid <hex>`), sidestepping all quoting/escaping issues.
pub fn ssid_to_hex(ssid: &str) -> String {
    ssid.bytes().map(|b| format!("{b:02x}")).collect()
}

/// Derive the 256-bit WPA/WPA2 pre-shared key defined by IEEE 802.11i:
/// PBKDF2-HMAC-SHA1(passphrase, SSID, 4096), rendered as lowercase hex.
pub fn derive_wpa2_psk(ssid: &str, passphrase: &str) -> String {
    type HmacSha1 = Hmac<Sha1>;

    let mut derived = [0u8; 32];
    let mut written = 0;
    for block_index in 1u32.. {
        let mut mac = HmacSha1::new_from_slice(passphrase.as_bytes())
            .expect("HMAC accepts keys of any length");
        mac.update(ssid.as_bytes());
        mac.update(&block_index.to_be_bytes());
        let mut u = mac.finalize().into_bytes();
        let mut block = u;

        for _ in 1..4096 {
            let mut mac = HmacSha1::new_from_slice(passphrase.as_bytes())
                .expect("HMAC accepts keys of any length");
            mac.update(&u);
            u = mac.finalize().into_bytes();
            for (dst, src) in block.iter_mut().zip(u.iter()) {
                *dst ^= *src;
            }
        }

        let count = (derived.len() - written).min(block.len());
        derived[written..written + count].copy_from_slice(&block[..count]);
        written += count;
        if written == derived.len() {
            break;
        }
    }

    let mut hex = String::with_capacity(64);
    for byte in derived {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    hex
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

pub async fn connect(ssid: &str, password: &str) -> Result<(), WifiError> {
    if ssid.is_empty() {
        return Err(WifiError::InvalidCredential("empty ssid"));
    }
    check_psk_len(password)?;
    ensure_station().await?;

    let id = parse_network_id(&wpa_cli(&["add_network"]).await?)?;
    let ssid_hex = ssid_to_hex(ssid);
    wpa_cli(&["set_network", &id, "ssid", &ssid_hex]).await?;

    if password.is_empty() {
        wpa_cli(&["set_network", &id, "key_mgmt", "NONE"]).await?;
    } else {
        let psk_hex = derive_wpa2_psk(ssid, password);
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StationStatus {
    pub state: String,
    pub ssid: Option<String>,
    pub ip_addr: Option<String>,
}

pub fn parse_station_status(output: &str) -> StationStatus {
    let mut status = StationStatus::default();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("wpa_state=") {
            status.state = value.to_string();
        } else if let Some(value) = line.strip_prefix("ssid=") {
            status.ssid = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("ip_address=") {
            status.ip_addr = Some(value.to_string());
        }
    }
    status
}

fn valid_ipv4(value: &str) -> bool {
    value
        .parse::<Ipv4Addr>()
        .map(|ip| !ip.is_unspecified())
        .unwrap_or(false)
}

pub fn connection_complete(status: &StationStatus, requested_ssid: &str) -> bool {
    status.state == "COMPLETED"
        && status.ssid.as_deref() == Some(requested_ssid)
        && status.ip_addr.as_deref().map(valid_ipv4).unwrap_or(false)
}

pub async fn station_status() -> Result<StationStatus, WifiError> {
    wpa_cli(&["status"])
        .await
        .map(|output| parse_station_status(&output))
}

pub async fn wait_for_connection(
    requested_ssid: &str,
    timeout: Duration,
) -> Result<StationStatus, WifiError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        ensure_station().await?;
        let status = station_status().await?;
        if connection_complete(&status, requested_ssid) {
            return Ok(status);
        }
        if status.state == "COMPLETED" && status.ip_addr.is_none() {
            ensure_dhcp().await?;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(WifiError::ConnectionTimeout);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
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
    let station = station_status().await.ok()?;
    if station.state == "COMPLETED" && station.ip_addr.is_none() {
        let _ = ensure_dhcp().await;
    }
    let ssid = station.ssid?;
    let ip = station.ip_addr.filter(|value| valid_ipv4(value))?;
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
    fn parse_associated_without_ip() {
        let status = parse_station_status("ssid=Home\nwpa_state=COMPLETED\n");
        assert_eq!(status.state, "COMPLETED");
        assert_eq!(status.ssid.as_deref(), Some("Home"));
        assert_eq!(status.ip_addr, None);
        assert!(!connection_complete(&status, "Home"));
    }

    #[test]
    fn connection_requires_requested_ssid_and_real_ipv4() {
        let complete = parse_station_status(
            "ssid=Home\nip_address=192.168.1.42\nwpa_state=COMPLETED\n",
        );
        assert!(connection_complete(&complete, "Home"));
        assert!(!connection_complete(&complete, "Other"));

        let unspecified = parse_station_status(
            "ssid=Home\nip_address=0.0.0.0\nwpa_state=COMPLETED\n",
        );
        assert!(!connection_complete(&unspecified, "Home"));
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
    fn derive_wpa2_psk_ieee_vector() {
        assert_eq!(
            derive_wpa2_psk("IEEE", "password"),
            "f42c6fc52df0ebef9ebb4b90b38a5f90\
             2e83fe1b135a70e23aed762e9710a12e"
        );
    }

    #[test]
    fn derive_wpa2_psk_second_vector() {
        assert_eq!(
            derive_wpa2_psk("ThisIsASSID", "ThisIsAPassword"),
            "0dc0d6eb90555ed6419756b9a15ec3e3\
             209b63df707dd508d14581f8982721af"
        );
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
