use std::process::Output;
use tokio::process::Command;
use tracing::{debug, warn};

async fn wpa_cli(args: &[&str]) -> Result<String, std::io::Error> {
    let output: Output = Command::new("wpa_cli")
        .args(args)
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    debug!("wpa_cli {:?} → {}", args, stdout.trim());
    Ok(stdout)
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
}
