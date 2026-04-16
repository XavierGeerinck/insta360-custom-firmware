//! Hardware audio control.
//!
//! Speaker volume + mute are applied via ALSA's `amixer` CLI. The `Master`
//! control range is 0–16 on this hardware (Amlogic AML-AUGESOUND); pspapp's
//! `Insta360_SetSpkVolume` accepts 0–100, which we scale accordingly.
//!
//! Mic mute is implemented in software by zeroing PCM frames in the capture
//! loop (see `apply_mic_mute` in `lib.rs`). libInstaAudio's `setMicMute` does
//! the same thing — multiplies the gain ratio by 0 in its DSP chain — so
//! short-circuiting at the frame boundary is equivalent without needing the
//! full audio-server singleton.
//!
//! Audio mode selection (`Insta360_SetAudioMode`) remains stubbed: it is
//! genuinely DSP-internal (TeaProcessSdk coefficient switching) with no ALSA
//! or sysfs alternative. Wiring it requires initializing the libInstaAudio
//! singleton (`Insta360_InitAudio` + opaque `insta_audio_database` struct)
//! and linking the .so at build time. State still updates so the UI reflects
//! intent; hardware effect is a TODO pending that integration.

use std::process::{Command, Stdio};
use tracing::{info, warn};

const MASTER_MAX: u16 = 16;

pub fn set_work_mode(mode: u32) -> Result<(), String> {
    // libInstaAudio::Insta360_SetAudioMode requires full audio-server init.
    info!("set_work_mode({mode}): stub (requires libInstaAudio init)");
    Ok(())
}

pub fn set_dump_enable(enable: bool) {
    info!("set_dump_enable({enable}): stub");
}

// ── Speaker control ─────────────────────────────────────────────────────────

/// Map pspapp 0-100 volume index to the Master control's 0-16 step range.
pub fn scale_volume(pspapp_vol: u16) -> u16 {
    let clamped = pspapp_vol.min(100);
    // Round-to-nearest instead of truncation so vol=50 → 8, not 7.
    ((clamped as u32 * MASTER_MAX as u32 + 50) / 100) as u16
}

/// Run `amixer sset Master <N>` to set absolute volume.
fn amixer_set_master(step: u16) -> Result<(), String> {
    let output = Command::new("amixer")
        .arg("sset")
        .arg("Master")
        .arg(step.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("amixer spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("amixer sset Master {step} failed: {stderr}"));
    }
    Ok(())
}

/// Apply speaker volume to the amplifier. Expects pspapp's 0-100 scale.
pub fn set_speaker_volume(pspapp_vol: u16) -> Result<(), String> {
    let step = scale_volume(pspapp_vol);
    match amixer_set_master(step) {
        Ok(()) => {
            info!("set_speaker_volume: pspapp={pspapp_vol} -> Master={step}/{MASTER_MAX}");
            Ok(())
        }
        Err(e) => {
            warn!("set_speaker_volume({pspapp_vol}) failed: {e}");
            Err(e)
        }
    }
}

/// Apply speaker mute. `muted=true` drops Master to 0; `muted=false` restores
/// `restore_pspapp_vol` (clamped ≥10 so unmute is always audible, matching the
/// pspapp `Insta360_SetSpkMute(0)` → then `SetSpkVolume(max(vol,10))` UI flow).
pub fn set_speaker_mute(muted: bool, restore_pspapp_vol: u16) -> Result<(), String> {
    if muted {
        match amixer_set_master(0) {
            Ok(()) => {
                info!("set_speaker_mute: Master=0 (muted)");
                Ok(())
            }
            Err(e) => {
                warn!("set_speaker_mute(true) failed: {e}");
                Err(e)
            }
        }
    } else {
        let restore = restore_pspapp_vol.max(10);
        set_speaker_volume(restore)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_volume_bounds() {
        assert_eq!(scale_volume(0),   0);
        assert_eq!(scale_volume(100), 16);
        assert_eq!(scale_volume(150), 16); // clamped
    }

    #[test]
    fn scale_volume_round_to_nearest() {
        // 50 * 16 / 100 = 8 exactly
        assert_eq!(scale_volume(50), 8);
        // 25 * 16 / 100 = 4
        assert_eq!(scale_volume(25), 4);
        // 10 * 16 / 100 = 1.6 → 2 with rounding
        assert_eq!(scale_volume(10), 2);
    }
}
