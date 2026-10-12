//! The Wave Controller "PC link" over vendor HID report 0x03, as stock
//! firmware 1.2.2.6 speaks it. The protocol and every number in here are in
//! `docs/WAVE_CONTROLLER_PROTOCOL.md`.
//!
//! Layers, each pure and tested on its own:
//!
//! - [`vendor`]: the 64-byte report and the short heartbeat.
//! - `transport`: send ids, resend, sync and reset over vendor reports.
//! - MAVLink v2 (`waved-mavlink`, msgid 1000 PC_CMD_PKG).
//! - `packet`: the 16-byte PC-link header, segmentation and reassembly.
//! - `handler`: one ack per command, bus commands out.

pub mod vendor;
