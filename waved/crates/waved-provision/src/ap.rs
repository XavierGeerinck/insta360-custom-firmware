//! Brings wlan0 up as a WPA2 AP via the on-device hostapd, and always
//! tears back down to STA. Single radio → AP and STA are mutually exclusive.

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tracing::{info, warn};

pub const AP_IP: &str = "192.168.2.1";
const HOSTAPD_CONF: &str = "/tmp/waved-hostapd.conf";
const IFACE: &str = "wlan0";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum ApError {
    #[error("Wi-Fi station transition failed: {0}")]
    Wifi(#[from] waved_wifi::wpa::WifiError),
    #[error("{program} timed out")]
    Timeout { program: String },
    #[error("{program} exited with status {status}")]
    CommandFailed { program: String, status: i32 },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// `Wave-XXXX` from the last 4 hex digits of the MAC (colons stripped, upper).
pub fn ap_ssid(mac: &str) -> String {
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let suffix: String = hex
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>()
        .to_uppercase();
    format!("Wave-{suffix}")
}

pub fn hostapd_conf(ssid: &str, passphrase: &str) -> String {
    format!(
        "interface={IFACE}\ndriver=nl80211\nssid={ssid}\nhw_mode=g\nchannel=1\n\
auth_algs=1\nwpa=2\nwpa_key_mgmt=WPA-PSK\nrsn_pairwise=CCMP\nwpa_passphrase={passphrase}\n"
    )
}

async fn run(program: &str, args: &[&str]) -> Result<(), ApError> {
    let future = Command::new(program).args(args).status();
    let status = tokio::time::timeout(COMMAND_TIMEOUT, future)
        .await
        .map_err(|_| ApError::Timeout {
            program: program.to_string(),
        })??;
    if status.success() {
        Ok(())
    } else {
        Err(ApError::CommandFailed {
            program: program.to_string(),
            status: status.code().unwrap_or(-1),
        })
    }
}

async fn process_running(name: &str) -> bool {
    tokio::time::timeout(COMMAND_TIMEOUT, Command::new("pidof").arg(name).status())
        .await
        .ok()
        .and_then(Result::ok)
        .map(|status| status.success())
        .unwrap_or(false)
}

async fn wait_for_process_exit(name: &str) -> Result<(), ApError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while process_running(name).await {
        if tokio::time::Instant::now() >= deadline {
            return Err(ApError::CommandFailed {
                program: format!("stop {name}"),
                status: -1,
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

fn write_hostapd_config(contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(HOSTAPD_CONF)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())
}

/// Controller for the provisioning AP. `start` brings the AP up; `stop` always
/// restores STA mode.
pub struct ApController {
    pub ssid: String,
    pub passphrase: String,
}

impl ApController {
    pub fn new(mac: &str, passphrase: String) -> Self {
        Self {
            ssid: ap_ssid(mac),
            passphrase,
        }
    }

    /// Tear down STA, write the config, start hostapd, and assign AP_IP.
    pub async fn start(&self) -> Result<(), ApError> {
        info!("provisioning AP up: {}", self.ssid);
        waved_wifi::wpa::stop_station().await?;
        write_hostapd_config(&hostapd_conf(&self.ssid, &self.passphrase))?;
        run("ip", &["addr", "flush", "dev", IFACE]).await?;
        run("ip", &["link", "set", IFACE, "up"]).await?;
        run("ip", &["addr", "add", &format!("{AP_IP}/24"), "dev", IFACE]).await?;
        run("hostapd", &["-B", HOSTAPD_CONF]).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        if !process_running("hostapd").await {
            return Err(ApError::CommandFailed {
                program: "hostapd".to_string(),
                status: -1,
            });
        }
        Ok(())
    }

    /// Stop AP daemons and hand the radio back to wpa_supplicant (STA).
    pub async fn stop(&self) -> Result<(), ApError> {
        info!("provisioning AP down → restoring STA");
        if process_running("hostapd").await {
            if let Err(error) = run("killall", &["-TERM", "hostapd"]).await {
                warn!("hostapd stop failed: {error}");
            }
            if let Err(error) = wait_for_process_exit("hostapd").await {
                warn!("hostapd exit wait failed: {error}");
            }
        }
        if let Err(error) = run("ip", &["addr", "flush", "dev", IFACE]).await {
            warn!("AP address cleanup failed: {error}");
        }
        if let Err(error) = std::fs::remove_file(HOSTAPD_CONF) {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn!("hostapd config cleanup failed: {error}");
            }
        }
        waved_wifi::wpa::restore_station().await?;
        Ok(())
    }
}

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
    fn ap_ssid_uses_mac_suffix() {
        assert_eq!(ap_ssid("aa:bb:cc:dd:1a:2b"), "Wave-1A2B");
    }
}
