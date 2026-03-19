# `waved-system` Implementation Plan — OTA, USB Gadget Mode, Factory Reset

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `waved-system` crate providing custom OTA firmware updates with RSA-2048 signature verification, USB gadget mode switching (speakerphone / file transfer), factory reset, and the associated deploy scripts for key generation and SWU package building.

**Architecture:** The `waved-system` crate exposes a `SystemService` that runs as an async task within the `waved` tokio runtime. OTA checks a user-configured HTTPS endpoint for new firmware, downloads the SWU file with proper TLS verification, verifies the SHA-256 + RSA-2048 signature, then shells out to `swupdate -i <path> -k <pubkey>` for installation. USB gadget mode switching manipulates the Linux configfs gadget at `/sys/kernel/config/usb_gadget/amlogic/` to toggle between speakerphone (UAC2 + HID) and file transfer (MSC) configurations. Factory reset wipes `/data/waved/` and reboots.

**Tech Stack:** Rust, tokio, reqwest (TLS), sha2, rsa, pkcs8, tokio::process::Command (swupdate/reboot), tokio::fs (sysfs writes), tracing

**Depends on:** waved Phase 1 MVP plan (workspace scaffolding, waved-state, waved-config)

---

## File Structure

```
waved/
├── crates/
│   └── waved-system/
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs              # SystemService, Command enum, run()
│           ├── ota.rs              # OTA check, download, verify, install
│           ├── usb.rs              # USB gadget mode switching via configfs
│           └── reset.rs            # Factory reset (wipe /data/waved/, reboot)
├── deploy/
│   └── swupdate/
│       ├── genkeys.sh             # RSA-2048 key pair generation
│       └── build_swu.sh           # Build SWU package from firmware images
```

---

### Task 1: Crate Scaffolding + SystemService Shell

**Files:**
- Create: `waved/crates/waved-system/Cargo.toml`
- Create: `waved/crates/waved-system/src/lib.rs`
- Create: `waved/crates/waved-system/src/ota.rs`
- Create: `waved/crates/waved-system/src/usb.rs`
- Create: `waved/crates/waved-system/src/reset.rs`
- Modify: `waved/Cargo.toml` (add waved-system to workspace dependencies)

- [ ] **Step 1: Create crate directory**

```bash
cd /Users/xaviergeerinck/Projects/insta360
mkdir -p waved/crates/waved-system/src
```

- [ ] **Step 2: Write Cargo.toml**

Create `waved/crates/waved-system/Cargo.toml`:
```toml
[package]
name = "waved-system"
version.workspace = true
edition.workspace = true

[dependencies]
tokio = { workspace = true, features = ["sync", "fs", "process", "time", "io-util"] }
tracing.workspace = true
thiserror.workspace = true
serde.workspace = true
serde_json.workspace = true
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "stream"] }
sha2 = "0.10"
rsa = "0.9"
pkcs8 = { version = "0.10", features = ["pem", "std"] }
signature = "2"
tokio-util = { version = "0.7", features = ["io"] }
futures-util = "0.3"
waved-state = { path = "../waved-state" }
waved-config = { path = "../waved-config" }
```

- [ ] **Step 3: Write lib.rs with SystemService shell**

Create `waved/crates/waved-system/src/lib.rs`:
```rust
pub mod ota;
pub mod reset;
pub mod usb;

use tokio::sync::mpsc;
use tracing::{info, warn, error};
use waved_config::WavedConfig;
use waved_state::StateBus;
use std::sync::Arc;

/// Commands that can be sent to the SystemService.
#[derive(Debug)]
pub enum Command {
    /// Check for OTA update and install if available.
    CheckOta,
    /// Switch USB gadget to speakerphone mode (UAC2 + HID).
    UsbSpeakerphone,
    /// Switch USB gadget to file transfer mode (MSC).
    UsbFileTransfer,
    /// Factory reset: wipe /data/waved/ and reboot.
    FactoryReset,
    /// Reboot the device.
    Reboot,
    /// Shutdown.
    Shutdown,
}

/// Result of an OTA check.
#[derive(Debug, Clone)]
pub enum OtaStatus {
    /// No update available.
    UpToDate,
    /// Update available with version string.
    UpdateAvailable { version: String },
    /// Currently downloading (progress 0.0 - 1.0).
    Downloading { progress: f32 },
    /// Verifying signature.
    Verifying,
    /// Installing via swupdate.
    Installing,
    /// OTA completed, reboot required.
    RebootRequired,
    /// OTA failed.
    Failed { reason: String },
}

pub struct SystemService {
    cmd_rx: mpsc::Receiver<Command>,
    state_bus: Arc<StateBus>,
    config: Arc<tokio::sync::watch::Receiver<WavedConfig>>,
}

impl SystemService {
    pub fn new(
        cmd_rx: mpsc::Receiver<Command>,
        state_bus: Arc<StateBus>,
        config: Arc<tokio::sync::watch::Receiver<WavedConfig>>,
    ) -> Self {
        Self {
            cmd_rx,
            state_bus,
            config,
        }
    }

    pub async fn run(mut self) {
        info!("system_srv started");

        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                Command::CheckOta => {
                    let config = self.config.borrow().clone();
                    let ota_server = &config.security.ota_server;
                    if ota_server.is_empty() {
                        warn!("OTA server not configured, skipping check");
                        continue;
                    }
                    info!("checking OTA from {}", ota_server);
                    match ota::check_and_install(ota_server).await {
                        Ok(()) => info!("OTA completed successfully"),
                        Err(e) => error!("OTA failed: {e}"),
                    }
                }
                Command::UsbSpeakerphone => {
                    info!("switching USB to speakerphone mode");
                    if let Err(e) = usb::switch_to_speakerphone().await {
                        error!("USB mode switch failed: {e}");
                    }
                }
                Command::UsbFileTransfer => {
                    info!("switching USB to file transfer mode");
                    if let Err(e) = usb::switch_to_file_transfer().await {
                        error!("USB mode switch failed: {e}");
                    }
                }
                Command::FactoryReset => {
                    warn!("factory reset requested");
                    if let Err(e) = reset::factory_reset().await {
                        error!("factory reset failed: {e}");
                    }
                }
                Command::Reboot => {
                    warn!("reboot requested");
                    if let Err(e) = reset::reboot().await {
                        error!("reboot failed: {e}");
                    }
                }
                Command::Shutdown => {
                    warn!("shutdown requested");
                    if let Err(e) = reset::shutdown().await {
                        error!("shutdown failed: {e}");
                    }
                }
            }
        }

        info!("system_srv stopped");
    }
}
```

- [ ] **Step 4: Write stub modules**

Create `waved/crates/waved-system/src/ota.rs`:
```rust
//! OTA update: check, download, verify, install.
//! Full implementation in Task 2.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum OtaError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("signature verification failed")]
    SignatureInvalid,
    #[error("SHA-256 hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("swupdate exited with code {0}")]
    SwupdateFailure(i32),
    #[error("no update available")]
    NoUpdate,
    #[error("invalid server response: {0}")]
    InvalidResponse(String),
}

pub async fn check_and_install(_ota_server: &str) -> Result<(), OtaError> {
    todo!("implemented in Task 2")
}
```

Create `waved/crates/waved-system/src/usb.rs`:
```rust
//! USB gadget mode switching via configfs.
//! Full implementation in Task 3.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum UsbError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("gadget configfs path not found: {0}")]
    ConfigfsNotFound(String),
}

pub async fn switch_to_speakerphone() -> Result<(), UsbError> {
    todo!("implemented in Task 3")
}

pub async fn switch_to_file_transfer() -> Result<(), UsbError> {
    todo!("implemented in Task 3")
}
```

Create `waved/crates/waved-system/src/reset.rs`:
```rust
//! Factory reset, reboot, and shutdown.
//! Full implementation in Task 4.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ResetError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("reboot command failed with code {0}")]
    RebootFailed(i32),
}

pub async fn factory_reset() -> Result<(), ResetError> {
    todo!("implemented in Task 4")
}

pub async fn reboot() -> Result<(), ResetError> {
    todo!("implemented in Task 4")
}

pub async fn shutdown() -> Result<(), ResetError> {
    todo!("implemented in Task 4")
}
```

- [ ] **Step 5: Add waved-system to workspace dependencies**

In `waved/Cargo.toml`, add to `[workspace.dependencies]`:
```toml
waved-system = { path = "crates/waved-system" }
```

- [ ] **Step 6: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-system
```

Expected: compiles with no errors (stubs use `todo!()` which is valid Rust).

- [ ] **Step 7: Commit**

```bash
git add waved/crates/waved-system/ waved/Cargo.toml
git commit -m "feat(waved-system): scaffold crate with SystemService, OTA/USB/reset stubs"
```

---

### Task 2: OTA Update — Check, Download, Verify, Install

**Files:**
- Modify: `waved/crates/waved-system/src/ota.rs`

This task implements the full OTA flow:
1. GET `{ota_server}/check?version={current}` to check for updates
2. Download the SWU file to `/tmp/waved_ota.swu` with progress tracking
3. Verify SHA-256 hash matches the server-provided hash
4. Verify RSA-2048 signature of `sw-description` inside the SWU CPIO
5. Install via `swupdate -i /tmp/waved_ota.swu -k /data/waved/swupdate-public.pem`
6. Trigger reboot

- [ ] **Step 1: Write the full OTA module**

Replace `waved/crates/waved-system/src/ota.rs`:
```rust
//! OTA update: check endpoint, download SWU, verify signature, install via swupdate.
//!
//! Security model (fixes stock firmware issues):
//! - TLS verification ENABLED (stock used `curl -k`)
//! - SHA-256 file integrity (stock used MD5)
//! - RSA-2048 signature verification (stock used RSA-512)
//! - swupdate verifies sw-description.sig inside the CPIO against our public key

use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tracing::{info, warn, debug};
use futures_util::StreamExt;

/// Path where the downloaded SWU is stored temporarily.
const OTA_DOWNLOAD_PATH: &str = "/tmp/waved_ota.swu";

/// Path to our RSA-2048 public key for swupdate signature verification.
const SWUPDATE_PUBLIC_KEY: &str = "/data/waved/swupdate-public.pem";

/// Insta360 Wave SWU magic bytes at offset 0.
const SWU_MAGIC: [u8; 4] = [0x21, 0x43, 0x23, 0x01];

/// Size of the JSON header region (1 MB).
const SWU_HEADER_SIZE: usize = 1_048_576;

/// Offset where CPIO archive begins (4 bytes magic + 1 MB header).
const CPIO_OFFSET: usize = 0x100004;

#[derive(Error, Debug)]
pub enum OtaError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SHA-256 hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },

    #[error("swupdate exited with code {0}")]
    SwupdateFailure(i32),

    #[error("swupdate binary not found")]
    SwupdateNotFound,

    #[error("no update available")]
    NoUpdate,

    #[error("invalid server response: {0}")]
    InvalidResponse(String),

    #[error("SWU file too small ({0} bytes)")]
    FileTooSmall(u64),

    #[error("SWU magic mismatch")]
    BadMagic,
}

/// Server response for OTA check endpoint.
#[derive(Debug, serde::Deserialize)]
struct OtaCheckResponse {
    /// Whether an update is available.
    available: bool,
    /// New firmware version string (e.g. "1.2.0").
    #[serde(default)]
    version: String,
    /// URL to download the SWU file.
    #[serde(default)]
    download_url: String,
    /// Expected SHA-256 hex digest of the SWU file.
    #[serde(default)]
    sha256: String,
    /// Total file size in bytes.
    #[serde(default)]
    file_size: u64,
}

/// Run the full OTA flow: check -> download -> verify -> install -> reboot.
pub async fn check_and_install(ota_server: &str) -> Result<(), OtaError> {
    // Step 1: Check for updates
    let current_version = env!("CARGO_PKG_VERSION");
    let check = check_for_update(ota_server, current_version).await?;

    if !check.available {
        info!("firmware is up to date ({})", current_version);
        return Err(OtaError::NoUpdate);
    }

    info!(
        "update available: {} -> {} ({} bytes)",
        current_version, check.version, check.file_size
    );

    // Step 2: Download SWU
    download_swu(&check.download_url, &check.sha256).await?;

    // Step 3: Validate SWU structure (magic bytes)
    validate_swu_structure(OTA_DOWNLOAD_PATH).await?;

    // Step 4: Install via swupdate (swupdate verifies RSA-2048 sig internally)
    install_swu(OTA_DOWNLOAD_PATH).await?;

    // Step 5: Reboot
    info!("OTA install complete, rebooting...");
    super::reset::reboot().await.map_err(|e| OtaError::Io(
        std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
    ))?;

    Ok(())
}

/// Query the OTA server for available updates.
async fn check_for_update(ota_server: &str, current_version: &str) -> Result<OtaCheckResponse, OtaError> {
    let url = format!("{}/check?version={}", ota_server.trim_end_matches('/'), current_version);
    debug!("OTA check URL: {url}");

    // Build client with TLS verification ENABLED (unlike stock `curl -k`)
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let resp = client
        .get(&url)
        .header("User-Agent", format!("waved/{current_version}"))
        .send()
        .await?
        .error_for_status()?;

    let check: OtaCheckResponse = resp
        .json()
        .await
        .map_err(|e| OtaError::InvalidResponse(e.to_string()))?;

    Ok(check)
}

/// Download the SWU file and verify its SHA-256 hash.
async fn download_swu(download_url: &str, expected_sha256: &str) -> Result<(), OtaError> {
    info!("downloading SWU from {download_url}");

    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(std::time::Duration::from_secs(3600)) // 1 hour for large firmware
        .build()?;

    let resp = client.get(download_url).send().await?.error_for_status()?;

    let total_size = resp.content_length().unwrap_or(0);

    let mut file = tokio::fs::File::create(OTA_DOWNLOAD_PATH).await?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_log_pct: u64 = 0;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        // Log progress every 10%
        if total_size > 0 {
            let pct = (downloaded * 100) / total_size;
            if pct >= last_log_pct + 10 {
                info!("download progress: {pct}% ({downloaded}/{total_size} bytes)");
                last_log_pct = pct;
            }
        }
    }

    file.flush().await?;
    drop(file);

    // Verify SHA-256 hash
    let actual_hash = hex::encode(hasher.finalize());
    let expected = expected_sha256.to_lowercase();

    if !expected.is_empty() && actual_hash != expected {
        // Clean up the bad download
        let _ = tokio::fs::remove_file(OTA_DOWNLOAD_PATH).await;
        return Err(OtaError::HashMismatch {
            expected,
            actual: actual_hash,
        });
    }

    info!("download complete, SHA-256 verified: {actual_hash}");
    Ok(())
}

/// Validate that the downloaded file has the correct SWU structure.
async fn validate_swu_structure(path: &str) -> Result<(), OtaError> {
    let metadata = tokio::fs::metadata(path).await?;

    // Must be at least large enough for magic + header + some CPIO data
    if metadata.len() < (CPIO_OFFSET as u64 + 1024) {
        return Err(OtaError::FileTooSmall(metadata.len()));
    }

    // Read and verify magic bytes
    let data = tokio::fs::read(path).await?;
    if data[..4] != SWU_MAGIC {
        // Maybe this is a raw CPIO SWU without the Insta360 header wrapper.
        // swupdate can handle raw CPIO too. Check for CPIO magic "070702" or "070701".
        if data.len() >= 6 && (&data[..6] == b"070702" || &data[..6] == b"070701") {
            debug!("SWU is a raw CPIO archive (no Insta360 header wrapper)");
            return Ok(());
        }
        return Err(OtaError::BadMagic);
    }

    debug!(
        "SWU structure valid: magic OK, file size {} bytes, CPIO at offset 0x{:X}",
        metadata.len(),
        CPIO_OFFSET
    );
    Ok(())
}

/// Install the SWU via the `swupdate` binary.
///
/// swupdate performs its own signature verification using the RSA-2048 public key.
/// It reads `sw-description.sig` from inside the CPIO archive and verifies it against
/// the public key we provide via `-k`.
///
/// For SWU files with the Insta360 header wrapper, we extract the CPIO portion first
/// because swupdate expects a raw CPIO archive.
async fn install_swu(path: &str) -> Result<(), OtaError> {
    // Determine if we need to extract the CPIO from the Insta360 wrapper
    let swu_path = prepare_swu_for_swupdate(path).await?;

    info!("running: swupdate -i {} -k {}", swu_path, SWUPDATE_PUBLIC_KEY);

    let status = tokio::process::Command::new("swupdate")
        .arg("-i")
        .arg(&swu_path)
        .arg("-k")
        .arg(SWUPDATE_PUBLIC_KEY)
        .status()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                OtaError::SwupdateNotFound
            } else {
                OtaError::Io(e)
            }
        })?;

    // Clean up temp files
    let _ = tokio::fs::remove_file(OTA_DOWNLOAD_PATH).await;
    if swu_path != OTA_DOWNLOAD_PATH {
        let _ = tokio::fs::remove_file(&swu_path).await;
    }

    if !status.success() {
        return Err(OtaError::SwupdateFailure(status.code().unwrap_or(-1)));
    }

    info!("swupdate completed successfully");
    Ok(())
}

/// If the SWU has the Insta360 header wrapper (magic 0x21 0x43 0x23 0x01 + 1MB JSON),
/// extract just the CPIO portion for swupdate. If it's already a raw CPIO, return as-is.
async fn prepare_swu_for_swupdate(path: &str) -> Result<String, OtaError> {
    let data = tokio::fs::read(path).await?;

    if data.len() >= 4 && data[..4] == SWU_MAGIC {
        // Has Insta360 header wrapper — extract CPIO at offset 0x100004
        if data.len() <= CPIO_OFFSET {
            return Err(OtaError::FileTooSmall(data.len() as u64));
        }

        let cpio_path = format!("{}.cpio", path);
        tokio::fs::write(&cpio_path, &data[CPIO_OFFSET..]).await?;
        info!(
            "extracted CPIO from Insta360 SWU wrapper ({} bytes CPIO)",
            data.len() - CPIO_OFFSET
        );
        Ok(cpio_path)
    } else {
        // Already a raw CPIO — use as-is
        Ok(path.to_string())
    }
}

/// Hex encoding utility (avoids adding `hex` crate dependency).
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}
```

- [ ] **Step 2: Add `hex` encoding note**

The above implementation includes an inline `hex::encode` to avoid an extra crate dependency. If the workspace already depends on the `hex` crate, replace the inline `mod hex` with `use hex;` and add `hex = "0.4"` to the `[dependencies]`.

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-system
```

Expected: compiles with no errors. The `rsa` and `pkcs8` imports from Cargo.toml are not yet used in `ota.rs` (swupdate handles signature verification internally) but will be available if we later add pre-verification before calling swupdate.

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-system/src/ota.rs
git commit -m "feat(waved-system): implement OTA check, download, SHA-256 verify, swupdate install"
```

---

### Task 3: USB Gadget Mode Switching

**Files:**
- Modify: `waved/crates/waved-system/src/usb.rs`

The Insta360 Wave uses Linux USB configfs at `/sys/kernel/config/usb_gadget/amlogic/`. The stock `S89usbgadget` init script sets up a composite gadget with UAC2 + HID + MSC + ADB. We switch between two modes:
- **Speakerphone:** UAC2 (48kHz/16-bit stereo) + HID (mute/volume) — the default
- **File transfer:** MSC (mass storage exposing `/mnt/udisk`) — for accessing recordings from a PC

Mode switching requires: disable UDC, reconfigure function symlinks in the active config, re-enable UDC.

- [ ] **Step 1: Write the full USB module**

Replace `waved/crates/waved-system/src/usb.rs`:
```rust
//! USB gadget mode switching via Linux configfs.
//!
//! The Insta360 Wave uses an Amlogic USB gadget at:
//!   /sys/kernel/config/usb_gadget/amlogic/
//!
//! Two modes:
//! - Speakerphone: UAC2 (audio) + HID (mute/volume buttons) — default
//! - File Transfer: MSC (mass storage) exposing /mnt/udisk to the host
//!
//! Switching requires tearing down the UDC binding, reconfiguring the
//! function symlinks in the active configuration, and re-enabling UDC.

use thiserror::Error;
use tokio::fs;
use tracing::{info, debug, warn};

/// Base path for the USB gadget in configfs.
const GADGET_PATH: &str = "/sys/kernel/config/usb_gadget/amlogic";

/// The USB Device Controller name (Amlogic DWC3).
const UDC_NAME: &str = "ff400000.dwc3";

/// Active configuration directory.
const CONFIG_PATH: &str = "/sys/kernel/config/usb_gadget/amlogic/configs/b.1";

/// USB function paths.
const FUNC_UAC2: &str = "/sys/kernel/config/usb_gadget/amlogic/functions/uac2.0";
const FUNC_HID_0: &str = "/sys/kernel/config/usb_gadget/amlogic/functions/hid.0";
const FUNC_MSC: &str = "/sys/kernel/config/usb_gadget/amlogic/functions/mass_storage.0";

/// UDC control file.
const UDC_FILE: &str = "/sys/kernel/config/usb_gadget/amlogic/UDC";

/// Mass storage backing file (the exFAT partition with recordings).
const MSC_LUN_FILE: &str = "/sys/kernel/config/usb_gadget/amlogic/functions/mass_storage.0/lun.0/file";
const MSC_BACKING_DEV: &str = "/dev/psp_udisk_s";

#[derive(Error, Debug)]
pub enum UsbError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("gadget configfs path not found: {0}")]
    ConfigfsNotFound(String),

    #[error("UDC not found: {0}")]
    UdcNotFound(String),
}

/// Current USB gadget mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbMode {
    Speakerphone,
    FileTransfer,
}

/// Switch the USB gadget to speakerphone mode (UAC2 + HID).
///
/// This is the default mode. The host sees a USB audio device and HID
/// controls for mute/volume.
pub async fn switch_to_speakerphone() -> Result<(), UsbError> {
    verify_gadget_exists().await?;

    info!("switching USB gadget to speakerphone mode (UAC2 + HID)");

    // Step 1: Unbind UDC (disconnect from host)
    unbind_udc().await?;

    // Step 2: Remove any existing function symlinks from config
    remove_all_function_links().await?;

    // Step 3: Create symlinks for UAC2 + HID
    create_function_link(FUNC_UAC2, "uac2.0").await?;
    create_function_link(FUNC_HID_0, "hid.0").await?;

    // Step 4: Re-bind UDC (reconnect to host)
    bind_udc().await?;

    info!("USB gadget now in speakerphone mode");
    Ok(())
}

/// Switch the USB gadget to file transfer mode (mass storage).
///
/// The host sees a USB mass storage device exposing the `/mnt/udisk` partition
/// where recordings are stored. The device must unmount `/mnt/udisk` first to
/// avoid corruption.
pub async fn switch_to_file_transfer() -> Result<(), UsbError> {
    verify_gadget_exists().await?;

    info!("switching USB gadget to file transfer mode (MSC)");

    // Step 1: Unbind UDC (disconnect from host)
    unbind_udc().await?;

    // Step 2: Ensure MSC function exists, configure backing store
    ensure_msc_function().await?;

    // Step 3: Remove any existing function symlinks from config
    remove_all_function_links().await?;

    // Step 4: Create symlink for MSC only
    create_function_link(FUNC_MSC, "mass_storage.0").await?;

    // Step 5: Unmount /mnt/udisk so the host gets exclusive access
    unmount_udisk().await?;

    // Step 6: Re-bind UDC (reconnect to host)
    bind_udc().await?;

    info!("USB gadget now in file transfer mode");
    Ok(())
}

/// Get the current USB mode by checking which function symlinks exist.
pub async fn current_mode() -> Result<UsbMode, UsbError> {
    let uac2_link = format!("{CONFIG_PATH}/uac2.0");
    let msc_link = format!("{CONFIG_PATH}/mass_storage.0");

    if fs::symlink_metadata(&msc_link).await.is_ok() {
        Ok(UsbMode::FileTransfer)
    } else if fs::symlink_metadata(&uac2_link).await.is_ok() {
        Ok(UsbMode::Speakerphone)
    } else {
        // Default assumption
        Ok(UsbMode::Speakerphone)
    }
}

/// Verify the gadget configfs directory exists.
async fn verify_gadget_exists() -> Result<(), UsbError> {
    if !fs::try_exists(GADGET_PATH).await.unwrap_or(false) {
        return Err(UsbError::ConfigfsNotFound(GADGET_PATH.to_string()));
    }
    Ok(())
}

/// Unbind the UDC (disconnect USB from host).
async fn unbind_udc() -> Result<(), UsbError> {
    debug!("unbinding UDC");
    // Writing empty string unbinds
    match fs::write(UDC_FILE, "").await {
        Ok(()) => {
            // Small delay to let the host notice the disconnect
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(UsbError::UdcNotFound(UDC_FILE.to_string()))
        }
        Err(e) => {
            // ENODEV is OK — means already unbound
            warn!("UDC unbind returned error (may already be unbound): {e}");
            Ok(())
        }
    }
}

/// Bind the UDC (connect USB to host).
async fn bind_udc() -> Result<(), UsbError> {
    debug!("binding UDC: {UDC_NAME}");
    fs::write(UDC_FILE, UDC_NAME).await?;
    // Give the host time to enumerate
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    Ok(())
}

/// Remove all function symlinks from the active configuration.
async fn remove_all_function_links() -> Result<(), UsbError> {
    let mut entries = fs::read_dir(CONFIG_PATH).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        // Function links are symlinks; skip regular files like "MaxPower", "bmAttributes", "strings"
        if fs::symlink_metadata(&path).await?.file_type().is_symlink() {
            debug!("removing function link: {}", path.display());
            fs::remove_file(&path).await?;
        }
    }

    Ok(())
}

/// Create a symlink from the config directory to a function.
async fn create_function_link(function_path: &str, link_name: &str) -> Result<(), UsbError> {
    let link_path = format!("{CONFIG_PATH}/{link_name}");
    debug!("creating function link: {link_path} -> {function_path}");
    tokio::fs::symlink(function_path, &link_path).await?;
    Ok(())
}

/// Ensure the mass_storage function exists and is configured.
async fn ensure_msc_function() -> Result<(), UsbError> {
    if !fs::try_exists(FUNC_MSC).await.unwrap_or(false) {
        // Create the MSC function
        debug!("creating mass_storage.0 function");
        fs::create_dir_all(FUNC_MSC).await?;
    }

    // Set the backing store to the recordings partition
    debug!("setting MSC backing store to {MSC_BACKING_DEV}");
    fs::write(MSC_LUN_FILE, MSC_BACKING_DEV).await?;

    Ok(())
}

/// Unmount /mnt/udisk so the host gets exclusive block device access.
async fn unmount_udisk() -> Result<(), UsbError> {
    debug!("unmounting /mnt/udisk for MSC access");
    let status = tokio::process::Command::new("umount")
        .arg("/mnt/udisk")
        .status()
        .await?;

    if !status.success() {
        // Not fatal — might already be unmounted
        warn!("umount /mnt/udisk returned non-zero (may already be unmounted)");
    }

    Ok(())
}

/// Remount /mnt/udisk after returning from file transfer mode.
/// Called internally when switching back to speakerphone mode.
pub async fn remount_udisk() -> Result<(), UsbError> {
    debug!("remounting /mnt/udisk");
    let status = tokio::process::Command::new("mount")
        .arg("/dev/psp_udisk_s")
        .arg("/mnt/udisk")
        .status()
        .await?;

    if !status.success() {
        warn!("mount /mnt/udisk returned non-zero");
    }

    Ok(())
}
```

- [ ] **Step 2: Update lib.rs to remount udisk when switching back to speakerphone**

In `waved/crates/waved-system/src/lib.rs`, update the `UsbSpeakerphone` arm:
```rust
Command::UsbSpeakerphone => {
    info!("switching USB to speakerphone mode");
    // Remount /mnt/udisk if we were in file transfer mode
    if let Ok(usb::UsbMode::FileTransfer) = usb::current_mode().await {
        let _ = usb::remount_udisk().await;
    }
    if let Err(e) = usb::switch_to_speakerphone().await {
        error!("USB mode switch failed: {e}");
    }
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-system
```

Expected: compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add waved/crates/waved-system/src/usb.rs waved/crates/waved-system/src/lib.rs
git commit -m "feat(waved-system): implement USB gadget mode switching (speakerphone / file transfer)"
```

---

### Task 4: Factory Reset, Reboot, and Shutdown

**Files:**
- Modify: `waved/crates/waved-system/src/reset.rs`

Factory reset wipes `/data/waved/` (config, database, keys, recordings queue) but preserves the stock rootfs SquashFS. After wiping, it reboots — on next boot, `waved` will recreate defaults from the bundled `default.toml`.

- [ ] **Step 1: Write the full reset module**

Replace `waved/crates/waved-system/src/reset.rs`:
```rust
//! Factory reset, reboot, and shutdown.
//!
//! Factory reset wipes /data/waved/ (config, DB, keys) but does NOT touch:
//! - Stock rootfs (SquashFS, read-only /dev/system)
//! - Recordings on /mnt/udisk (user data)
//! - The overlay at /data/overlay/ (keeps waved installed)
//!
//! After reset, waved recreates config from embedded defaults on next boot.
//! To fully revert to stock firmware, use `deploy/revert.sh` instead.

use thiserror::Error;
use tracing::{info, warn, error};

/// The waved data directory that gets wiped on factory reset.
const WAVED_DATA_DIR: &str = "/data/waved";

/// Items to preserve during factory reset (will be backed up and restored).
/// The swupdate public key should survive a reset so OTA still works.
const PRESERVE_FILES: &[&str] = &[
    "swupdate-public.pem",
];

#[derive(Error, Debug)]
pub enum ResetError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("command failed: {cmd} exited with code {code}")]
    CommandFailed { cmd: String, code: i32 },
}

/// Perform a factory reset: wipe /data/waved/ contents and reboot.
///
/// Preserves:
/// - swupdate-public.pem (so OTA still works after reset)
///
/// Wipes:
/// - config.toml (recreated from defaults on next boot)
/// - meetings.db (recording metadata and upload queue)
/// - lang/ (language files, will use built-in English)
/// - tea_config.json (audio tuning, will use TEA defaults)
/// - Any other runtime data
pub async fn factory_reset() -> Result<(), ResetError> {
    info!("starting factory reset — wiping {WAVED_DATA_DIR}");

    // Step 1: Back up files we want to preserve
    let mut preserved: Vec<(String, Vec<u8>)> = Vec::new();
    for filename in PRESERVE_FILES {
        let path = format!("{WAVED_DATA_DIR}/{filename}");
        match tokio::fs::read(&path).await {
            Ok(data) => {
                info!("preserving {filename} ({} bytes)", data.len());
                preserved.push((filename.to_string(), data));
            }
            Err(e) => {
                warn!("could not read {path} for preservation: {e}");
            }
        }
    }

    // Step 2: Remove all contents of /data/waved/
    match tokio::fs::remove_dir_all(WAVED_DATA_DIR).await {
        Ok(()) => info!("removed {WAVED_DATA_DIR}"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("{WAVED_DATA_DIR} does not exist, nothing to wipe");
        }
        Err(e) => {
            error!("failed to remove {WAVED_DATA_DIR}: {e}");
            return Err(ResetError::Io(e));
        }
    }

    // Step 3: Recreate the directory and restore preserved files
    tokio::fs::create_dir_all(WAVED_DATA_DIR).await?;

    for (filename, data) in &preserved {
        let path = format!("{WAVED_DATA_DIR}/{filename}");
        tokio::fs::write(&path, data).await?;
        info!("restored {filename}");
    }

    // Step 4: Sync filesystem
    sync_filesystem().await?;

    // Step 5: Reboot
    info!("factory reset complete, rebooting...");
    reboot().await
}

/// Reboot the device.
///
/// Uses the `reboot` command. On the Insta360 Wave, this triggers the
/// BusyBox reboot which sends SIGTERM to all processes, unmounts
/// filesystems, and calls the reboot syscall.
pub async fn reboot() -> Result<(), ResetError> {
    info!("rebooting device");

    // Sync first to ensure all data is flushed
    sync_filesystem().await?;

    let status = tokio::process::Command::new("reboot")
        .status()
        .await?;

    if !status.success() {
        return Err(ResetError::CommandFailed {
            cmd: "reboot".to_string(),
            code: status.code().unwrap_or(-1),
        });
    }

    Ok(())
}

/// Shutdown the device.
///
/// Uses `poweroff` command. On the Wave, this should also be coordinated
/// with the MCU (which controls the actual power rail), but as a fallback
/// the Linux poweroff will work.
pub async fn shutdown() -> Result<(), ResetError> {
    info!("shutting down device");

    sync_filesystem().await?;

    let status = tokio::process::Command::new("poweroff")
        .status()
        .await?;

    if !status.success() {
        return Err(ResetError::CommandFailed {
            cmd: "poweroff".to_string(),
            code: status.code().unwrap_or(-1),
        });
    }

    Ok(())
}

/// Flush all filesystem buffers.
async fn sync_filesystem() -> Result<(), ResetError> {
    let status = tokio::process::Command::new("sync")
        .status()
        .await?;

    if !status.success() {
        warn!("sync returned non-zero exit code");
    }

    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/xaviergeerinck/Projects/insta360/waved
cargo check -p waved-system
```

Expected: compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add waved/crates/waved-system/src/reset.rs
git commit -m "feat(waved-system): implement factory reset, reboot, shutdown"
```

---

### Task 5: RSA-2048 Key Generation Script

**Files:**
- Create: `waved/deploy/swupdate/genkeys.sh`

This script generates the RSA-2048 key pair used for signing custom SWU firmware packages. The private key stays on the build machine; the public key is deployed to the device at `/data/waved/swupdate-public.pem`.

- [ ] **Step 1: Create deploy directory**

```bash
mkdir -p /Users/xaviergeerinck/Projects/insta360/waved/deploy/swupdate
```

- [ ] **Step 2: Write genkeys.sh**

Create `waved/deploy/swupdate/genkeys.sh`:
```bash
#!/usr/bin/env bash
# genkeys.sh — Generate RSA-2048 key pair for SWUpdate firmware signing.
#
# Output:
#   swupdate-private.pem  — Private key (KEEP SECRET, used by build_swu.sh)
#   swupdate-public.pem   — Public key (deploy to device at /data/waved/swupdate-public.pem)
#
# The stock Insta360 Wave uses RSA-512 which is trivially factorable.
# We use RSA-2048 with SHA-256, which is the swupdate default.
#
# Usage:
#   ./genkeys.sh [output_dir]
#
# Example:
#   ./genkeys.sh ./keys
#   adb push ./keys/swupdate-public.pem /data/waved/swupdate-public.pem

set -euo pipefail

OUTPUT_DIR="${1:-.}"
PRIVATE_KEY="${OUTPUT_DIR}/swupdate-private.pem"
PUBLIC_KEY="${OUTPUT_DIR}/swupdate-public.pem"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}=== SWUpdate RSA-2048 Key Generation ===${NC}"
echo ""

# Check for openssl
if ! command -v openssl &> /dev/null; then
    echo -e "${RED}Error: openssl not found. Install it first.${NC}"
    exit 1
fi

# Create output directory
mkdir -p "${OUTPUT_DIR}"

# Check for existing keys
if [[ -f "${PRIVATE_KEY}" ]]; then
    echo -e "${YELLOW}Warning: ${PRIVATE_KEY} already exists.${NC}"
    read -p "Overwrite? (y/N): " confirm
    if [[ "${confirm}" != "y" && "${confirm}" != "Y" ]]; then
        echo "Aborted."
        exit 0
    fi
fi

# Generate RSA-2048 private key
echo "Generating RSA-2048 private key..."
openssl genpkey \
    -algorithm RSA \
    -pkeyopt rsa_keygen_bits:2048 \
    -out "${PRIVATE_KEY}"

# Set restrictive permissions on private key
chmod 0600 "${PRIVATE_KEY}"

# Extract public key
echo "Extracting public key..."
openssl rsa \
    -in "${PRIVATE_KEY}" \
    -pubout \
    -out "${PUBLIC_KEY}"

# Verify the key pair
echo ""
echo "Verifying key pair..."
TEST_DATA=$(mktemp)
TEST_SIG=$(mktemp)
echo "swupdate-key-test" > "${TEST_DATA}"

openssl dgst -sha256 -sign "${PRIVATE_KEY}" "${TEST_DATA}" > "${TEST_SIG}"
if openssl dgst -sha256 -verify "${PUBLIC_KEY}" -signature "${TEST_SIG}" "${TEST_DATA}" > /dev/null 2>&1; then
    echo -e "${GREEN}Key pair verification: OK${NC}"
else
    echo -e "${RED}Key pair verification: FAILED${NC}"
    rm -f "${PRIVATE_KEY}" "${PUBLIC_KEY}"
    rm -f "${TEST_DATA}" "${TEST_SIG}"
    exit 1
fi

rm -f "${TEST_DATA}" "${TEST_SIG}"

# Display key info
echo ""
echo -e "${GREEN}Keys generated successfully:${NC}"
echo "  Private key: ${PRIVATE_KEY} (KEEP SECRET — do NOT deploy to device)"
echo "  Public key:  ${PUBLIC_KEY}"
echo ""
echo "Key details:"
openssl rsa -in "${PRIVATE_KEY}" -text -noout 2>/dev/null | head -1
echo ""

# Deployment instructions
echo -e "${YELLOW}Deployment:${NC}"
echo "  1. Deploy public key to device:"
echo "     adb push ${PUBLIC_KEY} /data/waved/swupdate-public.pem"
echo ""
echo "  2. Use private key to sign firmware with build_swu.sh:"
echo "     ./build_swu.sh --key ${PRIVATE_KEY} --rootfs rootfs.squashfs"
echo ""
echo -e "${RED}IMPORTANT: Never deploy the private key to the device.${NC}"
echo -e "${RED}           Store it securely (e.g., hardware token, vault).${NC}"
```

- [ ] **Step 3: Make executable**

```bash
chmod +x /Users/xaviergeerinck/Projects/insta360/waved/deploy/swupdate/genkeys.sh
```

- [ ] **Step 4: Commit**

```bash
git add waved/deploy/swupdate/genkeys.sh
git commit -m "feat(deploy): add RSA-2048 key generation script for SWUpdate signing"
```

---

### Task 6: SWU Package Build Script

**Files:**
- Create: `waved/deploy/swupdate/build_swu.sh`

This script builds a complete SWU firmware package from component images. It:
1. Generates `sw-description` with SHA-256 hashes of all images
2. Signs `sw-description` with the RSA-2048 private key
3. Packages everything into a CPIO archive (SWUpdate format)
4. Optionally wraps with the Insta360 JSON header for compatibility

The SWU format from the report: CPIO archive with `sw-description` + `sw-description.sig` + `rootfs.squashfs` + `dtb.img` + `boot.img`. The CPIO must use `070702` (newc CRC) format, and `sw-description` MUST be the first entry.

- [ ] **Step 1: Write build_swu.sh**

Create `waved/deploy/swupdate/build_swu.sh`:
```bash
#!/usr/bin/env bash
# build_swu.sh — Build an SWU firmware package for the Insta360 Wave.
#
# Creates a signed SWU file compatible with `swupdate -i <file> -k <pubkey>`.
#
# SWU format (CPIO newc CRC):
#   sw-description          — libconfig manifest with SHA-256 hashes
#   sw-description.sig      — RSA-2048 SHA-256 signature of sw-description
#   rootfs.squashfs         — Root filesystem (SquashFS, written to /dev/system)
#   dtb.img                 — Device tree blob (written to /dev/dtb)
#   boot.img                — Linux kernel (written to /dev/boot)
#
# Usage:
#   ./build_swu.sh \
#       --key path/to/swupdate-private.pem \
#       --rootfs path/to/rootfs.squashfs \
#       [--dtb path/to/dtb.img] \
#       [--boot path/to/boot.img] \
#       [--version "1.0.0"] \
#       [--output firmware.swu] \
#       [--with-header]       # Add Insta360 JSON header wrapper

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Defaults
PRIVATE_KEY=""
ROOTFS=""
DTB=""
BOOT=""
VERSION="1.0.0"
OUTPUT="firmware.swu"
WITH_HEADER=false
HW_COMPAT="1.0"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --key)      PRIVATE_KEY="$2"; shift 2 ;;
        --rootfs)   ROOTFS="$2"; shift 2 ;;
        --dtb)      DTB="$2"; shift 2 ;;
        --boot)     BOOT="$2"; shift 2 ;;
        --version)  VERSION="$2"; shift 2 ;;
        --output)   OUTPUT="$2"; shift 2 ;;
        --with-header) WITH_HEADER=true; shift ;;
        --hw-compat) HW_COMPAT="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 --key <private.pem> --rootfs <rootfs.squashfs> [options]"
            echo ""
            echo "Required:"
            echo "  --key <path>       RSA-2048 private key (from genkeys.sh)"
            echo "  --rootfs <path>    Root filesystem (SquashFS)"
            echo ""
            echo "Optional:"
            echo "  --dtb <path>       Device tree blob"
            echo "  --boot <path>      Linux kernel boot image"
            echo "  --version <ver>    Firmware version string (default: 1.0.0)"
            echo "  --output <path>    Output SWU file (default: firmware.swu)"
            echo "  --with-header      Add Insta360 JSON header wrapper"
            echo "  --hw-compat <ver>  Hardware compatibility string (default: 1.0)"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown argument: $1${NC}"
            exit 1
            ;;
    esac
done

# Validate required arguments
if [[ -z "${PRIVATE_KEY}" ]]; then
    echo -e "${RED}Error: --key is required${NC}"
    exit 1
fi
if [[ -z "${ROOTFS}" ]]; then
    echo -e "${RED}Error: --rootfs is required${NC}"
    exit 1
fi
if [[ ! -f "${PRIVATE_KEY}" ]]; then
    echo -e "${RED}Error: private key not found: ${PRIVATE_KEY}${NC}"
    exit 1
fi
if [[ ! -f "${ROOTFS}" ]]; then
    echo -e "${RED}Error: rootfs not found: ${ROOTFS}${NC}"
    exit 1
fi

# Check for required tools
for cmd in openssl sha256sum cpio; do
    if ! command -v "${cmd}" &> /dev/null; then
        echo -e "${RED}Error: ${cmd} not found. Install it first.${NC}"
        exit 1
    fi
done

# Use a temporary working directory
WORK_DIR=$(mktemp -d)
trap 'rm -rf "${WORK_DIR}"' EXIT

echo -e "${GREEN}=== Building SWU Package ===${NC}"
echo "  Version:     ${VERSION}"
echo "  Private key: ${PRIVATE_KEY}"
echo "  Output:      ${OUTPUT}"
echo ""

# --- Step 1: Copy images to work dir and compute SHA-256 hashes ---

echo "Computing SHA-256 hashes..."

cp "${ROOTFS}" "${WORK_DIR}/rootfs.squashfs"
ROOTFS_SHA256=$(sha256sum "${WORK_DIR}/rootfs.squashfs" | cut -d' ' -f1)
ROOTFS_SIZE=$(stat -f%z "${WORK_DIR}/rootfs.squashfs" 2>/dev/null || stat -c%s "${WORK_DIR}/rootfs.squashfs")
echo "  rootfs.squashfs: ${ROOTFS_SHA256} (${ROOTFS_SIZE} bytes)"

IMAGES_BLOCK=""
FILES_LIST="sw-description sw-description.sig rootfs.squashfs"

# rootfs image entry
IMAGES_BLOCK="        {
            filename = \"rootfs.squashfs\";
            device = \"/dev/system\";
            type = \"raw\";
            sha256 = \"${ROOTFS_SHA256}\";
        }"

if [[ -n "${DTB}" && -f "${DTB}" ]]; then
    cp "${DTB}" "${WORK_DIR}/dtb.img"
    DTB_SHA256=$(sha256sum "${WORK_DIR}/dtb.img" | cut -d' ' -f1)
    DTB_SIZE=$(stat -f%z "${WORK_DIR}/dtb.img" 2>/dev/null || stat -c%s "${WORK_DIR}/dtb.img")
    echo "  dtb.img:          ${DTB_SHA256} (${DTB_SIZE} bytes)"
    FILES_LIST="${FILES_LIST} dtb.img"
    IMAGES_BLOCK="${IMAGES_BLOCK},
        {
            filename = \"dtb.img\";
            device = \"/dev/dtb\";
            type = \"raw\";
            sha256 = \"${DTB_SHA256}\";
        }"
fi

if [[ -n "${BOOT}" && -f "${BOOT}" ]]; then
    cp "${BOOT}" "${WORK_DIR}/boot.img"
    BOOT_SHA256=$(sha256sum "${WORK_DIR}/boot.img" | cut -d' ' -f1)
    BOOT_SIZE=$(stat -f%z "${WORK_DIR}/boot.img" 2>/dev/null || stat -c%s "${WORK_DIR}/boot.img")
    echo "  boot.img:         ${BOOT_SHA256} (${BOOT_SIZE} bytes)"
    FILES_LIST="${FILES_LIST} boot.img"
    IMAGES_BLOCK="${IMAGES_BLOCK},
        {
            filename = \"boot.img\";
            device = \"/dev/boot\";
            type = \"raw\";
            sha256 = \"${BOOT_SHA256}\";
        }"
fi

# --- Step 2: Generate sw-description ---

echo ""
echo "Generating sw-description..."

cat > "${WORK_DIR}/sw-description" << SWDESC
software = {
    version = "${VERSION}";
    hardware-compatibility: [ "${HW_COMPAT}" ];
    images: (
${IMAGES_BLOCK}
    );
}
SWDESC

echo "  sw-description generated ($(wc -c < "${WORK_DIR}/sw-description") bytes)"

# --- Step 3: Sign sw-description with RSA-2048 + SHA-256 ---

echo "Signing sw-description with RSA-2048..."

openssl dgst \
    -sha256 \
    -sign "${PRIVATE_KEY}" \
    "${WORK_DIR}/sw-description" \
    > "${WORK_DIR}/sw-description.sig"

SIG_SIZE=$(stat -f%z "${WORK_DIR}/sw-description.sig" 2>/dev/null || stat -c%s "${WORK_DIR}/sw-description.sig")
echo "  sw-description.sig: ${SIG_SIZE} bytes (RSA-2048 = 256 bytes)"

# Verify the signature is the right size for RSA-2048
if [[ "${SIG_SIZE}" -ne 256 ]]; then
    echo -e "${YELLOW}Warning: signature is ${SIG_SIZE} bytes (expected 256 for RSA-2048).${NC}"
    echo -e "${YELLOW}Make sure you're using an RSA-2048 key, not RSA-512!${NC}"
fi

# --- Step 4: Create CPIO archive ---

echo ""
echo "Creating CPIO archive..."

# IMPORTANT: sw-description MUST be the first entry in the CPIO archive.
# swupdate requires this — it reads the manifest first.
cd "${WORK_DIR}"
echo "${FILES_LIST}" | tr ' ' '\n' | cpio -ov -H crc > "${WORK_DIR}/firmware.cpio" 2>/dev/null

CPIO_SIZE=$(stat -f%z "${WORK_DIR}/firmware.cpio" 2>/dev/null || stat -c%s "${WORK_DIR}/firmware.cpio")
echo "  CPIO archive: ${CPIO_SIZE} bytes"

# --- Step 5: Output final SWU ---

if [[ "${WITH_HEADER}" == true ]]; then
    # Wrap with Insta360 JSON header (4 bytes magic + 1 MB JSON + CPIO)
    echo "Adding Insta360 header wrapper..."

    python3 -c "
import json, struct, sys

header = {
    'json_version': 1,
    'data': {
        'appVersion': {
            'version': {'en_us': '${VERSION}'},
            'description': {'en_us': 'waved custom firmware ${VERSION}'},
            'has_model_package': False,
            'bin_size': ${CPIO_SIZE},
            'swu_size': ${CPIO_SIZE},
            'force': False
        }
    }
}

json_bytes = json.dumps(header, indent=2).encode('utf-8')
# Pad JSON to exactly 1 MB
padded = json_bytes.ljust(1048576, b'\x00')

with open('${OUTPUT}', 'wb') as f:
    # Magic: 0x21 0x43 0x23 0x01
    f.write(b'\x21\x43\x23\x01')
    f.write(padded)
    with open('firmware.cpio', 'rb') as src:
        f.write(src.read())
" || {
        echo -e "${RED}Error: python3 failed. Falling back to raw CPIO output.${NC}"
        cp "${WORK_DIR}/firmware.cpio" "${OUTPUT}"
    }
else
    # Raw CPIO (standard swupdate format)
    cp "${WORK_DIR}/firmware.cpio" "${OUTPUT}"
fi

# --- Done ---

FINAL_SIZE=$(stat -f%z "${OUTPUT}" 2>/dev/null || stat -c%s "${OUTPUT}")

echo ""
echo -e "${GREEN}=== SWU Package Built Successfully ===${NC}"
echo "  Output:  ${OUTPUT}"
echo "  Size:    ${FINAL_SIZE} bytes ($(echo "scale=1; ${FINAL_SIZE}/1048576" | bc) MB)"
echo "  Version: ${VERSION}"
echo "  Format:  $(if [[ "${WITH_HEADER}" == true ]]; then echo "Insta360 wrapper + CPIO"; else echo "Raw CPIO (standard swupdate)"; fi)"
echo ""
echo -e "${YELLOW}To install on device:${NC}"
echo "  1. Host the SWU on your OTA server"
echo "  2. Set ota_server in config.toml"
echo "  3. Trigger OTA check from device"
echo ""
echo "  Or install directly via ADB:"
echo "    adb push ${OUTPUT} /tmp/firmware.swu"
echo "    adb shell swupdate -i /tmp/firmware.swu -k /data/waved/swupdate-public.pem"
echo "    adb shell reboot"
```

- [ ] **Step 2: Make executable**

```bash
chmod +x /Users/xaviergeerinck/Projects/insta360/waved/deploy/swupdate/build_swu.sh
```

- [ ] **Step 3: Verify scripts are valid bash**

```bash
bash -n /Users/xaviergeerinck/Projects/insta360/waved/deploy/swupdate/genkeys.sh
bash -n /Users/xaviergeerinck/Projects/insta360/waved/deploy/swupdate/build_swu.sh
```

Expected: no syntax errors.

- [ ] **Step 4: Commit**

```bash
git add waved/deploy/swupdate/build_swu.sh
git commit -m "feat(deploy): add SWU package build script with RSA-2048 signing"
```

---

## Integration Notes

### Wiring into main binary

After the Phase 1 MVP plan is implemented, add `SystemService` to the main `waved` binary. In `waved/crates/waved/src/main.rs`:

```rust
use waved_system::{SystemService, Command as SystemCommand};

// In the async block, after other services are initialized:
let (system_tx, system_rx) = tokio::sync::mpsc::channel::<SystemCommand>(8);
let system_srv = SystemService::new(system_rx, state_bus.clone(), config_rx.clone());
tokio::spawn(system_srv.run());

// system_tx is passed to ui_srv (or other modules) to trigger OTA/USB/reset
```

### OTA Server API Contract

The OTA server must implement a single endpoint:

```
GET {ota_server}/check?version={current_version}

Response 200:
{
    "available": true,
    "version": "1.2.0",
    "download_url": "https://your-server.example.com/firmware/1.2.0/firmware.swu",
    "sha256": "a1b2c3d4...",
    "file_size": 220000000
}

Response 200 (no update):
{
    "available": false
}
```

### Key Deployment Checklist

1. Run `deploy/swupdate/genkeys.sh ./keys` on your build machine
2. `adb push ./keys/swupdate-public.pem /data/waved/swupdate-public.pem`
3. Store `swupdate-private.pem` securely (never on device)
4. Set `[security].ota_server` in `config.toml` to your server URL

### Device Paths Reference

| Path | Purpose |
|---|---|
| `/data/waved/config.toml` | Main configuration |
| `/data/waved/swupdate-public.pem` | RSA-2048 public key for OTA verification |
| `/data/waved/meetings.db` | Recording metadata (wiped on factory reset) |
| `/tmp/waved_ota.swu` | Temporary OTA download (cleaned after install) |
| `/sys/kernel/config/usb_gadget/amlogic/` | USB gadget configfs |
| `/sys/kernel/config/usb_gadget/amlogic/UDC` | UDC bind/unbind control |
| `/sys/kernel/config/usb_gadget/amlogic/configs/b.1/` | Active USB config |
| `/dev/psp_udisk_s` | Recordings partition (backing store for MSC mode) |
| `/mnt/udisk` | Recordings mount point |
