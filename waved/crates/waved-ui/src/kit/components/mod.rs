//! Reusable pieces from the component table in `docs/mockups/README.md`.

pub mod buttons;
pub mod controls;
pub mod dial;
/// Files list rows (workstream D, D1).
pub mod file_row;
pub mod list;
/// Radial audio-mode picker (workstream B, B2).
pub mod mode_picker;
pub mod row;
/// Playback scrubber (workstream D, D2).
pub mod scrubber;
pub mod status;
/// Wi-Fi signal arcs and the setup QR code (workstream B, B7).
pub mod wifi;

#[cfg(test)]
mod compositions;
