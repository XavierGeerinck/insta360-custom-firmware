//! Lightweight WAV playback for UI feedback.
//!
//! Calls `aplay` via `std::process::Command::spawn()` so playback is fire-and-forget
//! and never blocks the UI thread. On the device `aplay -q` exits when the clip
//! finishes (short feedback WAVs complete in under a second), so we don't hold
//! onto the child handle.
//!
//! This replaces the sound feedback that pspapp's business-logic daemon would
//! emit in response to IPC commands (`business_set_int_val`) — waved doesn't
//! talk to that daemon, so we play the stock WAVs directly.

use std::process::{Command, Stdio};
use tracing::warn;

/// Spawn `aplay -q <path>` in the background. Errors are logged, not returned —
/// audio feedback is non-critical and must never block UI thread progress.
pub fn play(path: &str) {
    let result = Command::new("aplay")
        .arg("-q")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    if let Err(e) = result {
        warn!("aplay spawn failed for {path}: {e}");
    }
}

// ── Stock pspapp asset paths ──────────────────────────────────────────────────

pub const SFX_MIC_MUTE: &str = "/etc/pspapp/micmute.wav";
pub const SFX_MIC_UNMUTE: &str = "/etc/pspapp/micunmute.wav";
pub const SFX_SPK_VOL: &str = "/etc/pspapp/speakervol.wav";
pub const SFX_AIRECORD: &str = "/etc/pspapp/airecord.wav";
pub const SFX_SHUTDOWN: &str = "/etc/pspapp/shutdown.wav";
