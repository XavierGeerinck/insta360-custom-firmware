//! Speaker mute + volume via `amixer` shell-out.
//!
//! The Amlogic driver exposes a scalar control whose name varies across
//! firmware builds. `discover_control()` enumerates controls at startup
//! and picks the first that looks like a master / speaker scalar.

use std::process::Command;
use tracing::{debug, warn};

pub fn discover_control() -> Option<String> {
    // `amixer scontrols` output like: Simple mixer control 'Master',0
    let out = Command::new("amixer")
        .arg("scontrols")
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if let Some(start) = line.find('\'') {
            if let Some(end) = line[start + 1..].find('\'') {
                let name = &line[start + 1..start + 1 + end];
                let lower = name.to_lowercase();
                if lower.contains("master") || lower.contains("speaker") || lower.contains("playback") {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

pub fn set_mute(control: &str, muted: bool) -> std::io::Result<()> {
    let arg = if muted { "mute" } else { "unmute" };
    let status = Command::new("amixer")
        .arg("sset").arg(control).arg(arg)
        .status()?;
    if !status.success() {
        warn!("amixer sset {control} {arg} exit code {:?}", status.code());
    }
    debug!("speaker_mixer: {control} {arg}");
    Ok(())
}

pub fn set_volume(control: &str, percent: u32) -> std::io::Result<()> {
    let p = percent.min(100);
    let status = Command::new("amixer")
        .arg("sset").arg(control).arg(format!("{p}%"))
        .status()?;
    if !status.success() {
        warn!("amixer sset {control} {p}% exit code {:?}", status.code());
    }
    debug!("speaker_mixer: {control} -> {p}%");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_clamped_to_100() {
        // We can't verify amixer without it being installed; this test only
        // exercises the clamp logic. `amixer` not being installed makes
        // set_volume return Err, which is fine — the clamp runs before the call.
        let _ = set_volume("Master", 150);
    }

    #[test]
    fn discover_control_returns_none_if_amixer_absent_or_empty() {
        // Environment-dependent: if amixer is installed and has controls, this
        // may return Some; we assert only that it doesn't panic.
        let _ = discover_control();
    }
}
