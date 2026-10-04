pub mod ap;
pub mod dhcp;
pub mod dns;
pub mod http;
mod setup_form;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::watch;
use tracing::{info, warn};
use waved_config::{apply_form, field_specs, ConfigError, FieldError, FormSubmission, WavedConfig};
use waved_state::{Command, ProvisioningState, StateBus};

const PROV_TIMEOUT: Duration = Duration::from_secs(300); // 5 min
const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Generate a new WPA2-compatible eight-hex passphrase for each AP session.
pub fn gen_passphrase() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

pub struct ProvisioningService {
    bus: Arc<StateBus>,
    mac: String,
    config_path: PathBuf,
}

struct SetupAction {
    wifi: Option<(String, String)>,
    test_webhook: bool,
}

impl ProvisioningService {
    pub fn new(bus: Arc<StateBus>, mac: String, config_path: impl Into<PathBuf>) -> Self {
        Self {
            bus,
            mac,
            config_path: config_path.into(),
        }
    }

    pub async fn run(self, mut commands: broadcast::Receiver<Command>) {
        info!("provision_srv started");
        loop {
            match commands.recv().await {
                Ok(Command::OpenWifiProvisioning) => {
                    self.provision().await;
                    // The primary receiver queues commands while provision()
                    // owns the service. Drop duplicate Open/Cancel messages so
                    // a tap during the active session cannot start another AP.
                    while commands.try_recv().is_ok() {}
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    fn set(&self, state: ProvisioningState) {
        // The UI reads the retained value directly from the sender instead of
        // keeping a watch Receiver. `send()` discards the new value when there
        // are zero receivers; `send_replace()` always updates it.
        self.bus.provisioning.send_replace(state);
    }

    async fn provision(&self) {
        // Subscribe to the command bus BEFORE the long scan/AP-bringup phase: a
        // broadcast receiver only sees messages sent after it subscribes, so a
        // Cancel pressed during Scanning/start() would otherwise be lost and the
        // radio could stay in AP mode until the 5-min timeout.
        let mut cancel_rx = self.bus.commands.subscribe();

        // 1. Scan as STA.
        self.set(ProvisioningState::Scanning);
        if let Err(error) = waved_wifi::wpa::ensure_station().await {
            warn!("station preparation failed: {error}");
        }
        let mut ssids = waved_wifi::wpa::scan().await.unwrap_or_default();
        ssids.sort();
        ssids.dedup();
        let config = WavedConfig::load_or_default(&self.config_path);
        let csrf = uuid::Uuid::new_v4().simple().to_string();

        if matches!(cancel_rx.try_recv(), Ok(Command::CancelWifiProvisioning)) {
            self.set(ProvisioningState::Idle);
            return;
        }

        // 2. Bring up AP.
        let passphrase = gen_passphrase();
        let ap = ap::ApController::new(&self.mac, passphrase.clone());
        if let Err(e) = ap.start().await {
            warn!("AP start failed: {e}");
            self.set(ProvisioningState::Failed {
                reason: "AP start failed".into(),
            });
            let _ = ap.stop().await;
            return;
        }

        let dhcp = match dhcp::Server::bind() {
            Ok(server) => server,
            Err(error) => {
                warn!("DHCP bind failed: {error}");
                let _ = ap.stop().await;
                self.set(ProvisioningState::Failed {
                    reason: "DHCP start failed".into(),
                });
                return;
            }
        };
        let dns = match dns::Server::bind() {
            Ok(server) => server,
            Err(error) => {
                warn!("DNS bind failed: {error}");
                let _ = ap.stop().await;
                self.set(ProvisioningState::Failed {
                    reason: "DNS start failed".into(),
                });
                return;
            }
        };
        let http_listener = match TcpListener::bind((ap::AP_IP, 80)).await {
            Ok(listener) => listener,
            Err(error) => {
                warn!("provisioning HTTP bind failed: {error}");
                let _ = ap.stop().await;
                self.set(ProvisioningState::Failed {
                    reason: "setup page start failed".into(),
                });
                return;
            }
        };
        let (services_shutdown, dhcp_rx) = watch::channel(false);
        let dns_rx = dhcp_rx.clone();
        let mut dhcp_task = tokio::spawn(dhcp.serve(dhcp_rx));
        let mut dns_task = tokio::spawn(dns.serve(dns_rx));
        self.set(ProvisioningState::ApActive {
            ssid: ap.ssid.clone(),
            passphrase,
            url: format!("http://{}", ap::AP_IP),
        });
        info!(ssid = %ap.ssid, url = %format!("http://{}", ap::AP_IP), "provisioning AP ready");

        // 3. Serve the form until creds arrive, timeout, or cancel.
        enum Outcome {
            Saved(SetupAction),
            Cancelled,
            Failed(String),
        }
        let outcome = tokio::select! {
            action = self.serve(http_listener, &ssids, &csrf, config) => match action {
                Some(action) => Outcome::Saved(action),
                None => Outcome::Failed("setup page stopped".into()),
            },
            _ = tokio::time::sleep(PROV_TIMEOUT) => Outcome::Cancelled,
            _ = wait_for_cancel(&mut cancel_rx) => Outcome::Cancelled,
            result = &mut dhcp_task => {
                let reason = match result {
                    Ok(Ok(())) => "DHCP server stopped".to_string(),
                    Ok(Err(error)) => format!("DHCP server failed: {error}"),
                    Err(error) => format!("DHCP task failed: {error}"),
                };
                Outcome::Failed(reason)
            },
            result = &mut dns_task => {
                let reason = match result {
                    Ok(Ok(())) => "DNS server stopped".to_string(),
                    Ok(Err(error)) => format!("DNS server failed: {error}"),
                    Err(error) => format!("DNS task failed: {error}"),
                };
                Outcome::Failed(reason)
            },
        };

        // 4. Tear down AP, restore STA.
        let _ = services_shutdown.send(true);
        if !dhcp_task.is_finished() {
            match dhcp_task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => warn!("DHCP shutdown failed: {error}"),
                Err(error) => warn!("DHCP task join failed: {error}"),
            }
        }
        if !dns_task.is_finished() {
            match dns_task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => warn!("DNS shutdown failed: {error}"),
                Err(error) => warn!("DNS task join failed: {error}"),
            }
        }
        if let Err(error) = ap.stop().await {
            warn!("AP cleanup/STA restore failed: {error}");
        }

        // 5. Connect (or report idle/failure).
        match outcome {
            Outcome::Saved(action) => {
                let connected_ssid = match action.wifi {
                    Some((ssid, psk)) => {
                        self.set(ProvisioningState::Connecting { ssid: ssid.clone() });
                        match waved_wifi::wpa::connect(&ssid, &psk).await {
                            Ok(()) => match waved_wifi::wpa::wait_for_connection(
                                &ssid,
                                Duration::from_secs(45),
                            )
                            .await
                            {
                                Ok(_) => Some(ssid),
                                Err(error) => {
                                    self.set(ProvisioningState::Failed {
                                        reason: format!("connection timed out: {error}"),
                                    });
                                    None
                                }
                            },
                            Err(error) => {
                                self.set(ProvisioningState::Failed {
                                    reason: format!("connect failed: {error}"),
                                });
                                None
                            }
                        }
                    }
                    None => {
                        self.set(ProvisioningState::Reconnecting);
                        match wait_for_any_connection(Duration::from_secs(45)).await {
                            Ok(ssid) => Some(ssid),
                            Err(error) => {
                                self.set(ProvisioningState::Failed { reason: error });
                                None
                            }
                        }
                    }
                };
                if let Some(ssid) = connected_ssid {
                    if action.test_webhook {
                        if let Err(error) = mark_webhook_test_pending(&self.config_path) {
                            warn!("failed to persist webhook test marker: {error}");
                        }
                    }
                    self.set(ProvisioningState::Restarting);
                    info!(%ssid, "setup applied; requesting controlled restart");
                    let _ = self.bus.commands.send(Command::RestartForConfig);
                }
            }
            Outcome::Cancelled => self.set(ProvisioningState::Idle),
            Outcome::Failed(reason) => {
                warn!("provisioning session failed: {reason}");
                self.set(ProvisioningState::Failed {
                    reason: "provisioning service failed".into(),
                });
            }
        }
    }

    /// Setup HTTP server on AP_IP:80. Invalid submissions are rendered in the
    /// same AP session; a valid save returns the optional station credentials.
    async fn serve(
        &self,
        listener: TcpListener,
        ssids: &[String],
        csrf: &str,
        mut config: WavedConfig,
    ) -> Option<SetupAction> {
        loop {
            let (mut sock, _) = listener.accept().await.ok()?;
            let request = match tokio::time::timeout(HTTP_IO_TIMEOUT, http::read_request(&mut sock))
                .await
            {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    let response = http::render_error(error);
                    let _ =
                        tokio::time::timeout(HTTP_IO_TIMEOUT, sock.write_all(response.as_bytes()))
                            .await;
                    continue;
                }
                Err(_) => {
                    let response = http::render_error(http::HttpError::BadRequest);
                    let _ = sock.write_all(response.as_bytes()).await;
                    continue;
                }
            };
            match http::route(request) {
                Ok(http::Route::Form) => {
                    let response = setup_form::render_page(&config, ssids, csrf, &[], None);
                    let _ =
                        tokio::time::timeout(HTTP_IO_TIMEOUT, sock.write_all(response.as_bytes()))
                            .await;
                }
                Ok(http::Route::CaptiveRedirect { head_only }) => {
                    let response = http::render_captive_redirect(head_only);
                    let _ =
                        tokio::time::timeout(HTTP_IO_TIMEOUT, sock.write_all(response.as_bytes()))
                            .await;
                }
                Ok(route @ (http::Route::Save { .. } | http::Route::SaveConnect { .. })) => {
                    let (form, connect) = match route {
                        http::Route::Save { form } => (form, false),
                        http::Route::SaveConnect { form } => (form, true),
                        _ => unreachable!(),
                    };
                    if form.get("csrf").map(String::as_str) != Some(csrf) {
                        let response = http::render_error(http::HttpError::BadRequest);
                        let _ = sock.write_all(response.as_bytes()).await;
                        let _ = sock.shutdown().await;
                        continue;
                    }

                    let wifi = if connect {
                        match http::wifi_credentials(&form) {
                            Ok(credentials) => Some(credentials),
                            Err(_) => {
                                let response = setup_form::render_page(
                                    &config,
                                    ssids,
                                    csrf,
                                    &[],
                                    Some("Select a valid Wi-Fi network and password."),
                                );
                                let _ = sock.write_all(response.as_bytes()).await;
                                let _ = sock.shutdown().await;
                                continue;
                            }
                        }
                    } else {
                        None
                    };

                    let submission = config_submission(&form);
                    match save_submission(&self.config_path, &config, &submission) {
                        Ok(saved) => {
                            self.set(ProvisioningState::Saving);
                            config = saved.config;
                            let kept_as = saved
                                .kept
                                .as_deref()
                                .and_then(Path::file_name)
                                .map(|name| name.to_string_lossy().into_owned());
                            let response = setup_form::render_ack(
                                wifi.as_ref().map(|(ssid, _)| ssid.as_str()),
                                !config.api.endpoint.is_empty(),
                                kept_as.as_deref(),
                            );
                            let _ = tokio::time::timeout(
                                HTTP_IO_TIMEOUT,
                                sock.write_all(response.as_bytes()),
                            )
                            .await;
                            let _ = sock.shutdown().await;
                            return Some(SetupAction {
                                wifi,
                                test_webhook: !config.api.endpoint.is_empty(),
                            });
                        }
                        Err(SaveError::Config(error)) => {
                            warn!("setup page save failed: {error}");
                            let response = setup_form::render_page(
                                &config,
                                ssids,
                                csrf,
                                &[],
                                Some(SAVE_FAILED),
                            );
                            let _ = sock.write_all(response.as_bytes()).await;
                        }
                        Err(SaveError::Form(errors)) => {
                            let response =
                                setup_form::render_page(&config, ssids, csrf, &errors, None);
                            let _ = sock.write_all(response.as_bytes()).await;
                        }
                    }
                }
                Err(error) => {
                    let response = http::render_error(error);
                    let _ =
                        tokio::time::timeout(HTTP_IO_TIMEOUT, sock.write_all(response.as_bytes()))
                            .await;
                }
            }
            let _ = sock.shutdown().await;
        }
    }
}

/// Shown whenever config.toml can't be written. Config error text can quote
/// the file (toml parse errors print the offending line, secrets included), so
/// it goes to the log and never to the page.
const SAVE_FAILED: &str =
    "Wave couldn't save the settings file. Nothing was changed. Try again, or restart Wave.";

enum SaveError {
    Form(Vec<FieldError>),
    Config(ConfigError),
}

struct Saved {
    config: WavedConfig,
    /// Where an unreadable config.toml was kept, if there was one.
    kept: Option<PathBuf>,
}

/// Apply a setup-page submission to config.toml as it is on disk now, so
/// device-side changes saved while the page was open survive. The session
/// snapshot is used when the file does not exist, and also when it exists but
/// does not load: the snapshot is what the page showed (the backup or the
/// defaults), and the unreadable file is kept aside as `config.toml.unloadable`
/// rather than overwritten. Only this explicit user save does that; the
/// settings service never writes over a file that does not load.
fn save_submission(
    path: &Path,
    snapshot: &WavedConfig,
    submission: &FormSubmission,
) -> Result<Saved, SaveError> {
    let base = match WavedConfig::load(path) {
        Ok(fresh) => fresh,
        Err(ConfigError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            snapshot.clone()
        }
        Err(error) => {
            warn!(
                "{} does not load ({error}); the setup save will keep it aside",
                path.display()
            );
            snapshot.clone()
        }
    };
    let next = apply_form(&base, submission).map_err(SaveError::Form)?;
    let kept = next
        .save_replacing_unloadable(path)
        .map_err(SaveError::Config)?;
    Ok(Saved { config: next, kept })
}

fn config_submission(form: &HashMap<String, String>) -> FormSubmission {
    let paths: BTreeSet<&str> = field_specs().iter().map(|spec| spec.path).collect();
    let mut submission = FormSubmission {
        values: BTreeMap::new(),
        clear_secrets: BTreeSet::new(),
    };
    for (key, value) in form {
        if paths.contains(key.as_str()) || key == "audio.steered_initial_az_deg" {
            submission.values.insert(key.clone(), value.clone());
        } else if let Some(path) = key.strip_prefix("clear.") {
            if paths.contains(path) && value == "true" {
                submission.clear_secrets.insert(path.to_string());
            }
        }
    }
    submission
}

async fn wait_for_any_connection(timeout: Duration) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Err(error) = waved_wifi::wpa::ensure_station().await {
            return Err(format!("station restore failed: {error}"));
        }
        if let Some((ssid, _, _)) = waved_wifi::wpa::status().await {
            return Ok(ssid);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("timed out reconnecting to a saved network".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn mark_webhook_test_pending(config_path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let marker = parent.join("webhook-test.pending");
    std::fs::write(&marker, b"pending\n")?;
    std::fs::set_permissions(marker, std::fs::Permissions::from_mode(0o600))
}

/// Resolve when a `CancelWifiProvisioning` command arrives on `rx` (or the
/// channel closes). Takes a pre-subscribed receiver so a cancel sent during the
/// scan/AP-bringup phase isn't missed.
async fn wait_for_cancel(rx: &mut broadcast::Receiver<Command>) {
    loop {
        match rx.recv().await {
            Ok(Command::CancelWifiProvisioning) => return,
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service_without_watch_receivers() -> ProvisioningService {
        ProvisioningService::new(
            Arc::new(StateBus::new()),
            "00:11:22:33:44:55".into(),
            "/tmp/waved-test-config.toml",
        )
    }

    fn named(name: &str) -> FormSubmission {
        FormSubmission {
            values: BTreeMap::from([("device.name".to_string(), name.to_string())]),
            clear_secrets: BTreeSet::new(),
        }
    }

    #[test]
    fn a_form_save_keeps_device_side_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let snapshot = WavedConfig::load(&path).unwrap();

        // The settings service writes while the setup page is open.
        let mut device_side = WavedConfig::load(&path).unwrap();
        device_side.device.first_run_done = true;
        device_side.audio.beam_aim_deg = 270.0;
        device_side.save(&path).unwrap();

        let saved = match save_submission(&path, &snapshot, &named("Board room")) {
            Ok(saved) => saved,
            Err(_) => panic!("save failed"),
        };
        let on_disk = WavedConfig::load(&path).unwrap();
        assert_eq!(saved.config, on_disk);
        assert_eq!(saved.kept, None);
        assert_eq!(on_disk.device.name, "Board room");
        assert!(on_disk.device.first_run_done, "device-side value kept");
        assert_eq!(on_disk.audio.beam_aim_deg, 270.0);
    }

    #[test]
    fn a_form_save_without_a_file_starts_from_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut snapshot = WavedConfig::default();
        snapshot.api.endpoint = "https://notes.example.com/hook".into();

        assert!(save_submission(&path, &snapshot, &named("Board room")).is_ok());
        let on_disk = WavedConfig::load(&path).unwrap();
        assert_eq!(on_disk.device.name, "Board room");
        assert_eq!(on_disk.api.endpoint, "https://notes.example.com/hook");
    }

    const BROKEN_FILES: [(&str, &str); 3] = [
        ("syntax error", "api]\nauth_token = \"s3cr3t-token\n"),
        (
            "newer version",
            "config_version = 99\nauth_token = \"s3cr3t-token\"\n",
        ),
        (
            "unknown key",
            "[api]\nauth_token = \"s3cr3t-token\"\nbogus = 1\n",
        ),
    ];

    #[test]
    fn a_form_save_replaces_a_file_that_does_not_load_and_keeps_it() {
        for (what, broken) in BROKEN_FILES {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, broken).unwrap();
            let mut snapshot = WavedConfig::default();
            snapshot.display.brightness_pct = 33;

            let saved = match save_submission(&path, &snapshot, &named("Board room")) {
                Ok(saved) => saved,
                Err(_) => panic!("{what}: save failed"),
            };

            let kept = dir.path().join("config.toml.unloadable");
            assert_eq!(saved.kept.as_deref(), Some(kept.as_path()), "{what}");
            assert_eq!(std::fs::read_to_string(&kept).unwrap(), broken, "{what}");
            let on_disk = WavedConfig::load(&path).unwrap();
            assert_eq!(on_disk, saved.config, "{what}");
            assert_eq!(on_disk.device.name, "Board room", "{what}");
            assert_eq!(on_disk.display.brightness_pct, 33, "{what}: snapshot base");
        }
    }

    #[test]
    fn a_second_unloadable_file_is_kept_beside_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let snapshot = WavedConfig::default();
        std::fs::write(&path, "config_version = 99\n").unwrap();
        assert!(save_submission(&path, &snapshot, &named("One")).is_ok());
        std::fs::write(&path, "broken [\n").unwrap();
        let second = match save_submission(&path, &snapshot, &named("Two")) {
            Ok(saved) => saved.kept.unwrap(),
            Err(_) => panic!("second save failed"),
        };
        assert_ne!(second, dir.path().join("config.toml.unloadable"));
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "broken [\n");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml.unloadable")).unwrap(),
            "config_version = 99\n"
        );
    }

    async fn post(addr: std::net::SocketAddr, path: &str, body: &str) -> String {
        use tokio::io::AsyncReadExt;
        let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: wave\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        sock.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        sock.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn save_and_connect_works_over_an_unloadable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let broken = "api]\nauth_token = \"s3cr3t-token\n";
        std::fs::write(&path, broken).unwrap();
        let service = ProvisioningService::new(Arc::new(StateBus::new()), "mac".into(), &path);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (action, response) = tokio::join!(
            service.serve(listener, &[], "csrf1", WavedConfig::default()),
            post(
                addr,
                "/save-connect",
                "csrf=csrf1&ssid=Home&psk=password123&device.name=Board+room"
            )
        );

        let action = action.expect("the save should return an action");
        assert_eq!(
            action.wifi,
            Some(("Home".to_string(), "password123".to_string()))
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml.unloadable")).unwrap(),
            broken
        );
        assert_eq!(WavedConfig::load(&path).unwrap().device.name, "Board room");
        assert!(response.contains("config.toml.unloadable"), "{response}");
        assert!(!response.contains("s3cr3t"), "{response}");
    }

    #[tokio::test]
    async fn a_failed_save_shows_a_plain_sentence_and_no_config_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let broken = "api]\nauth_token = \"s3cr3t-token\n";
        std::fs::write(&path, broken).unwrap();
        // A directory where the temporary file goes makes the save fail.
        std::fs::create_dir(dir.path().join("config.toml.new")).unwrap();
        let service = ProvisioningService::new(Arc::new(StateBus::new()), "mac".into(), &path);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let response = tokio::select! {
            _ = service.serve(listener, &[], "csrf1", WavedConfig::default()) => {
                panic!("the save should have failed")
            }
            response = post(addr, "/save", "csrf=csrf1&device.name=Board+room") => response,
        };

        assert!(response.contains(SAVE_FAILED), "{response}");
        for leaked in [
            "s3cr3t",
            "failed to",
            "Is a directory",
            "os error",
            "TOML",
            "line 1",
        ] {
            assert!(!response.contains(leaked), "{leaked}: {response}");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken, "put back");
    }

    #[test]
    fn passphrase_is_8_chars_hex() {
        let p = gen_passphrase();
        assert_eq!(p.len(), 8);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn state_is_retained_without_watch_receiver() {
        let service = service_without_watch_receivers();
        assert_eq!(service.bus.provisioning.receiver_count(), 0);
        service.set(ProvisioningState::Scanning);
        assert_eq!(
            *service.bus.provisioning.borrow(),
            ProvisioningState::Scanning
        );
    }

    #[test]
    fn config_submission_filters_transport_fields_and_handles_secret_clear() {
        let form = HashMap::from([
            ("csrf".into(), "session".into()),
            ("ssid".into(), "Home".into()),
            ("psk".into(), "password123".into()),
            ("device.name".into(), "Board room".into()),
            ("api.endpoint".into(), "https://receiver.test/hook".into()),
            ("clear.api.auth_token".into(), "true".into()),
            ("unknown.field".into(), "ignored".into()),
        ]);
        let submission = config_submission(&form);
        assert_eq!(
            submission.values.get("device.name").map(String::as_str),
            Some("Board room")
        );
        assert_eq!(
            submission.values.get("api.endpoint").map(String::as_str),
            Some("https://receiver.test/hook")
        );
        assert!(submission.clear_secrets.contains("api.auth_token"));
        assert!(!submission.values.contains_key("ssid"));
        assert!(!submission.values.contains_key("unknown.field"));
    }
}
