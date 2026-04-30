//! Playback DSP chain — runs inside `PlaybackService::run_playback_loop`
//! between the mpsc dequeue and the ALSA write. Order: HPF → EQ → MBAGC → Limiter.
//! The post-DSP buffer is what gets broadcast as the AEC reference signal.
//!
//! Submodules are added by subsequent tasks:
//!   biquad — Task 2 (this task)
//!   hpf    — Task 4
//!   eq     — Task 5
//!   mbagc  — Tasks 6 and 7
//!   limiter— Task 8
//!   chain  — Task 9 (adds `pub use chain::PlaybackDspChain;`)

pub mod biquad;
pub mod hpf;
