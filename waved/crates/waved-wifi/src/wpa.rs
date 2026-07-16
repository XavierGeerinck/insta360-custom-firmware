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
const MAX_NETWORK_PRIORITY: i32 = i32::MAX;
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
    #[error("invalid saved network list: {0:?}")]
    BadSavedNetworks(String),
    #[error("invalid network priority: {0:?}")]
    BadPriority(String),
    #[error("invalid credential: {0}")]
    InvalidCredential(&'static str),
    #[error("timed out waiting for Wi-Fi connection")]
    ConnectionTimeout,
    #[error("process did not exit: {0}")]
    ProcessDidNotExit(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn final_nonempty_line(stdout: &str) -> Option<&str> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

/// Classify wpa_cli stdout. A final `FAIL` line is an error; everything else
/// (including informational prefixes, `OK`, status dumps, and numeric ids)
/// passes through unchanged.
pub fn check_wpa_ok(stdout: &str) -> Result<String, WifiError> {
    if final_nonempty_line(stdout) == Some("FAIL") {
        return Err(WifiError::CommandFailed);
    }
    Ok(stdout.to_string())
}

fn wpa_cli_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut command_args = Vec::with_capacity(args.len() + 2);
    command_args.extend(["-i", IFACE]);
    command_args.extend_from_slice(args);
    command_args
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
    let command_args = wpa_cli_args(args);
    let fut = Command::new("wpa_cli").args(&command_args).output();
    let output: Output = match tokio::time::timeout(CMD_TIMEOUT, fut).await {
        Ok(res) => res?,
        Err(_) => return Err(WifiError::Timeout),
    };
    if !output.status.success() {
        return Err(WifiError::BadExit(output.status.code().unwrap_or(-1)));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    debug!(
        "wpa_cli {:?} → {}",
        redact_psk(&command_args),
        stdout.trim()
    );
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

async fn stop_process(name: &str) -> Result<(), WifiError> {
    if !process_running(name).await? {
        return Ok(());
    }
    if let Err(error) = command_output("killall", &["-TERM", name]).await {
        // A process can exit between pidof and killall. Only preserve the
        // command error if it is still present afterward.
        if process_running(name).await? {
            return Err(error);
        }
        return Ok(());
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while process_running(name).await? {
        if tokio::time::Instant::now() >= deadline {
            debug!("{name} ignored SIGTERM; forcing exit before radio handoff");
            command_output("killall", &["-KILL", name]).await?;
            let kill_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            while process_running(name).await? {
                if tokio::time::Instant::now() >= kill_deadline {
                    return Err(WifiError::ProcessDidNotExit(name.to_string()));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
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
    stop_process("udhcpc").await?;
    stop_process("wpa_supplicant").await?;
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

/// `add_network` ends with a network index on success. Some firmware builds
/// prefix it with an informational interface-selection line.
pub fn parse_network_id(stdout: &str) -> Result<String, WifiError> {
    let id = final_nonempty_line(stdout).unwrap_or_default();
    id.parse::<u32>()
        .map(|id| id.to_string())
        .map_err(|_| WifiError::BadNetworkId(stdout.trim().to_string()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedNetwork {
    pub id: u32,
    pub ssid: String,
    pub disabled: bool,
    pub current: bool,
}

/// Parse the tab-separated `wpa_cli list_networks` table. Firmware may prefix
/// the table with an interface-selection banner, so parsing begins only at the
/// exact header. Rows after that header are required to have all four fields.
pub fn parse_saved_networks(output: &str) -> Result<Vec<SavedNetwork>, WifiError> {
    const HEADER: &str = "network id / ssid / bssid / flags";

    let mut found_header = false;
    let mut networks = Vec::new();
    for line in output.lines() {
        if !found_header {
            found_header = line.trim() == HEADER;
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }

        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 4 || fields[1].is_empty() {
            return Err(WifiError::BadSavedNetworks(line.to_string()));
        }
        let id = fields[0]
            .parse::<u32>()
            .map_err(|_| WifiError::BadSavedNetworks(line.to_string()))?;
        let flags = fields[3];
        networks.push(SavedNetwork {
            id,
            ssid: fields[1].to_string(),
            disabled: flags.contains("[DISABLED]"),
            current: flags.contains("[CURRENT]"),
        });
    }

    if !found_header {
        return Err(WifiError::BadSavedNetworks(output.trim().to_string()));
    }
    Ok(networks)
}

pub fn parse_priority(output: &str) -> Result<i32, WifiError> {
    let value = final_nonempty_line(output).unwrap_or_default();
    value
        .parse::<i32>()
        .map_err(|_| WifiError::BadPriority(output.trim().to_string()))
}

pub fn next_network_priority(priorities: &[i32]) -> i32 {
    priorities
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .max(0)
        .checked_add(1)
        .unwrap_or(MAX_NETWORK_PRIORITY)
}

fn matching_network<'a>(networks: &'a [SavedNetwork], ssid: &str) -> Option<&'a SavedNetwork> {
    networks
        .iter()
        .filter(|network| network.ssid == ssid)
        .max_by_key(|network| network.current)
}

async fn saved_networks() -> Result<Vec<SavedNetwork>, WifiError> {
    let output = wpa_cli(&["list_networks"]).await?;
    parse_saved_networks(&output)
}

async fn saved_priorities(networks: &[SavedNetwork]) -> Result<Vec<i32>, WifiError> {
    let mut priorities = Vec::with_capacity(networks.len());
    for network in networks {
        let id = network.id.to_string();
        let output = wpa_cli(&["get_network", &id, "priority"]).await?;
        priorities.push(parse_priority(&output)?);
    }
    Ok(priorities)
}

async fn persist_network(
    id: &str,
    ssid: &str,
    password: &str,
    priority: i32,
) -> Result<(), WifiError> {
    let ssid_hex = ssid_to_hex(ssid);
    wpa_cli(&["set_network", id, "ssid", &ssid_hex]).await?;

    if password.is_empty() {
        wpa_cli(&["set_network", id, "key_mgmt", "NONE"]).await?;
    } else {
        let psk_hex = derive_wpa2_psk(ssid, password);
        wpa_cli(&["set_network", id, "key_mgmt", "WPA-PSK"]).await?;
        wpa_cli(&["set_network", id, "psk", &psk_hex]).await?;
    }

    let priority = priority.to_string();
    wpa_cli(&["set_network", id, "priority", &priority]).await?;
    wpa_cli(&["enable_network", id]).await?;
    wpa_cli(&["save_config"]).await?;
    command_output("chmod", &["600", SUPPLICANT_CONF]).await?;
    wpa_cli(&["reassociate"]).await?;
    Ok(())
}

async fn remove_incomplete_network(id: &str) {
    if wpa_cli(&["remove_network", id]).await.is_ok() {
        let _ = wpa_cli(&["save_config"]).await;
        let _ = command_output("chmod", &["600", SUPPLICANT_CONF]).await;
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

    let networks = saved_networks().await?;
    let priorities = saved_priorities(&networks).await?;
    let priority = next_network_priority(&priorities);
    let existing = matching_network(&networks, ssid);
    let (id, created) = match existing {
        Some(network) => (network.id.to_string(), false),
        None => (parse_network_id(&wpa_cli(&["add_network"]).await?)?, true),
    };

    let result = persist_network(&id, ssid, password, priority).await;
    if result.is_err() && created {
        remove_incomplete_network(&id).await;
    }
    result
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
        let complete =
            parse_station_status("ssid=Home\nip_address=192.168.1.42\nwpa_state=COMPLETED\n");
        assert!(connection_complete(&complete, "Home"));
        assert!(!connection_complete(&complete, "Other"));

        let unspecified =
            parse_station_status("ssid=Home\nip_address=0.0.0.0\nwpa_state=COMPLETED\n");
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
        assert!(matches!(
            check_wpa_ok("FAIL\n"),
            Err(WifiError::CommandFailed)
        ));
        assert!(matches!(
            check_wpa_ok("Selected interface 'wlan0'\nFAIL\n"),
            Err(WifiError::CommandFailed)
        ));
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
        assert_eq!(
            parse_network_id("Selected interface 'wlan0'\n1\n").unwrap(),
            "1"
        );
        assert_eq!(parse_network_id("banner\n\n42\n\n").unwrap(), "42");
    }

    #[test]
    fn validate_network_id_rejects_malformed_output() {
        for output in [
            "",
            "\n\n",
            "FAIL\n",
            "banner only\n",
            "network 1\n",
            "999999999999999999999\n",
        ] {
            assert!(matches!(
                parse_network_id(output),
                Err(WifiError::BadNetworkId(_))
            ));
        }
    }

    #[test]
    fn parse_saved_networks_with_banner_flags_and_spaces() {
        let output = "Selected interface 'wlan0'\n\
network id / ssid / bssid / flags\n\
0\tGuest network\tany\t[DISABLED]\n\
7\tOffice 5G\taa:bb:cc:dd:ee:01\t[CURRENT]\n";
        assert_eq!(
            parse_saved_networks(output).unwrap(),
            vec![
                SavedNetwork {
                    id: 0,
                    ssid: "Guest network".to_string(),
                    disabled: true,
                    current: false,
                },
                SavedNetwork {
                    id: 7,
                    ssid: "Office 5G".to_string(),
                    disabled: false,
                    current: true,
                },
            ]
        );
    }

    #[test]
    fn parse_saved_networks_accepts_empty_table() {
        let output = "network id / ssid / bssid / flags\n";
        assert!(parse_saved_networks(output).unwrap().is_empty());
    }

    #[test]
    fn parse_saved_networks_rejects_missing_header_and_malformed_rows() {
        for output in [
            "Selected interface 'wlan0'\n",
            "network id / ssid / bssid / flags\nnot-a-number\tHome\tany\t\n",
            "network id / ssid / bssid / flags\n1\tHome\tany\n",
            "network id / ssid / bssid / flags\n1\t\tany\t\n",
        ] {
            assert!(matches!(
                parse_saved_networks(output),
                Err(WifiError::BadSavedNetworks(_))
            ));
        }
    }

    #[test]
    fn matching_network_prefers_current_duplicate() {
        let networks = vec![
            SavedNetwork {
                id: 2,
                ssid: "Office".to_string(),
                disabled: false,
                current: false,
            },
            SavedNetwork {
                id: 8,
                ssid: "Office".to_string(),
                disabled: false,
                current: true,
            },
        ];
        assert_eq!(matching_network(&networks, "Office").unwrap().id, 8);
        assert!(matching_network(&networks, "Other").is_none());
    }

    #[test]
    fn parse_and_increment_network_priorities() {
        assert_eq!(
            parse_priority("Selected interface 'wlan0'\n42\n").unwrap(),
            42
        );
        assert_eq!(next_network_priority(&[]), 1);
        assert_eq!(next_network_priority(&[-5, 0, 9]), 10);
        assert_eq!(
            next_network_priority(&[MAX_NETWORK_PRIORITY]),
            MAX_NETWORK_PRIORITY
        );
    }

    #[test]
    fn malformed_priority_is_rejected() {
        for output in ["", "FAIL\n", "priority=7\n", "2147483648\n"] {
            assert!(matches!(
                parse_priority(output),
                Err(WifiError::BadPriority(_))
            ));
        }
    }

    #[test]
    fn all_wpa_cli_calls_select_wlan0_explicitly() {
        assert_eq!(
            wpa_cli_args(&["set_network", "1", "ssid", "486f6d65"]),
            ["-i", "wlan0", "set_network", "1", "ssid", "486f6d65"]
        );
    }

    #[test]
    fn validate_psk_length() {
        assert!(matches!(
            check_psk_len("short"),
            Err(WifiError::InvalidCredential(_))
        ));
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
