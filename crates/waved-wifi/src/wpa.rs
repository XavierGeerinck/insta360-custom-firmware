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

pub async fn scan() -> Vec<String> {
    if let Err(e) = wpa_cli(&["scan"]).await {
        warn!("wifi scan trigger failed: {e}");
        return vec![];
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    match wpa_cli(&["scan_results"]).await {
        Ok(output) => output
            .lines()
            .skip(1)
            .filter_map(|line| line.split('\t').nth(4).map(String::from))
            .filter(|ssid| !ssid.is_empty())
            .collect(),
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

pub async fn status() -> Option<(String, String)> {
    let output = wpa_cli(&["status"]).await.ok()?;
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
