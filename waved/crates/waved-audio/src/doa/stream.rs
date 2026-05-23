//! Per-frame DoA publication. Emits one `DoaFrame` per ALSA capture period
//! on a `tokio::sync::broadcast` channel. Consumers include F3 (diarization),
//! the steered-beam controller (Task 14), and any UI/logging subscriber.
//!
//! Confidence is normalised to [0, 1] so consumers can threshold without
//! knowing which backend (GCC-PHAT or SRP-PHAT) actually ran.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Capacity of the broadcast channel. ~1.3 s of buffering at 21 ms cadence
/// (the production ALSA period of 1024 samples at 48 kHz). Slow consumers
/// get `RecvError::Lagged` past this; not an error.
const DOA_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoaBackend {
    GccPhat,
    SrpPhat,
}

impl Default for DoaBackend {
    fn default() -> Self {
        DoaBackend::GccPhat
    }
}

/// One DoA observation per ALSA period. When `vad` is false, `azimuth_deg`
/// and `elevation_deg` are `f32::NAN` and `confidence` is 0.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DoaFrame {
    pub frame_idx: u64,
    pub timestamp_ms: u64,
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub confidence: f32,
    pub vad: bool,
    pub backend: DoaBackend,
}

impl DoaFrame {
    /// Construct a "no speech / no direction" frame for a given frame index.
    pub fn silent(frame_idx: u64, timestamp_ms: u64, backend: DoaBackend) -> Self {
        Self {
            frame_idx,
            timestamp_ms,
            azimuth_deg: f32::NAN,
            elevation_deg: f32::NAN,
            confidence: 0.0,
            vad: false,
            backend,
        }
    }
}

pub struct DoaPublisher {
    tx: broadcast::Sender<DoaFrame>,
}

impl DoaPublisher {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(DOA_CHANNEL_CAPACITY);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DoaFrame> {
        self.tx.subscribe()
    }

    /// Send a frame. No-op when there are no subscribers (broadcast::send
    /// returns Err; we discard — consistent with other AudioService taps).
    pub fn emit(&self, frame: DoaFrame) {
        let _ = self.tx.send(frame);
    }
}

impl Default for DoaPublisher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_frame_has_nan_angles_and_zero_confidence() {
        let f = DoaFrame::silent(42, 1_000_000, DoaBackend::GccPhat);
        assert!(f.azimuth_deg.is_nan());
        assert!(f.elevation_deg.is_nan());
        assert_eq!(f.confidence, 0.0);
        assert!(!f.vad);
        assert_eq!(f.frame_idx, 42);
    }

    #[test]
    fn doa_frame_round_trips_to_json() {
        let f = DoaFrame {
            frame_idx: 7,
            timestamp_ms: 12_345,
            azimuth_deg: 45.5,
            elevation_deg: -3.2,
            confidence: 0.71,
            vad: true,
            backend: DoaBackend::SrpPhat,
        };
        let s = serde_json::to_string(&f).expect("serializes");
        assert!(s.contains("\"backend\":\"srp_phat\""), "json: {s}");
        assert!(s.contains("\"vad\":true"));
        assert!(s.contains("\"azimuth_deg\":45.5"));
    }

    #[test]
    fn publisher_emit_with_no_subscribers_does_not_panic() {
        let p = DoaPublisher::new();
        p.emit(DoaFrame::silent(0, 0, DoaBackend::GccPhat));
    }

    #[test]
    fn publisher_emit_delivers_to_subscriber() {
        let p = DoaPublisher::new();
        let mut rx = p.subscribe();
        p.emit(DoaFrame::silent(1, 0, DoaBackend::GccPhat));
        let got = rx.try_recv().expect("frame should be available");
        assert_eq!(got.frame_idx, 1);
    }

    #[test]
    fn backend_default_is_gcc_phat() {
        assert_eq!(DoaBackend::default(), DoaBackend::GccPhat);
    }
}
