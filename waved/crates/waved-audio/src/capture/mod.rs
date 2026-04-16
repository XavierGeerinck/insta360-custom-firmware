//! Audio capture: ALSA on device, file replay on host, mock in tests.
//! See docs/superpowers/specs/2026-04-16-waved-audio-capture-foundation-design.md

pub mod channel_map;

pub use channel_map::{ChannelMap, ChannelMapError, ChannelMapSource};
