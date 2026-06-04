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

/// Controller for the provisioning AP. `start` brings the AP up; `stop` always
/// restores STA mode.
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
        // Release wpa_supplicant's hold on the iface (STA mode).
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
        // Restart STA the same way the stock init does (see docs/WIFI.md, Task 13,
        // for the exact reconciled command).
        let _ = run(
            "wpa_supplicant",
            &["-B", "-Dnl80211", "-iwlan0", "-c", "/tmp/wpa_supplicant.conf"],
        )
        .await;
        let _ = run("udhcpc", &["-i", IFACE, "-b"]).await;
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
