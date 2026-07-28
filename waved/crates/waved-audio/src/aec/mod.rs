//! Acoustic echo cancellation + noise suppression.
//!
//! The module provides two DSP primitives used by `AudioService`:
//!
//! - `EchoCanceller` — removes the speaker signal (reference) from the mic
//!   signal using an adaptive filter.
//! - `NoiseSuppressor` — spectral noise reduction, with optional linkage to
//!   an echo canceller for residual-echo suppression.
//!
//! Both come in a speexdsp-backed implementation. Production conferencing
//! uses `speex::SpeexAecStage`, which couples linear cancellation and the
//! linked residual-echo preprocessor with safe native drop ordering. The
//! no-op `passthrough::PassthroughCanceller` remains for tests and DSP-disabled
//! paths.

pub mod deep_filter;
pub mod passthrough;
pub mod speex;

#[allow(non_camel_case_types, non_snake_case, dead_code)]
mod sys;

/// Filter tail length in milliseconds. Chosen per spec §8: 200 ms covers
/// typical small-room reverb while keeping the adaptive filter's memory
/// footprint and per-period CPU cost bounded.
pub const FILTER_TAIL_MS: usize = 200;

/// One frame of mono 16-bit audio in, one frame out. Callers are responsible
/// for channel mixdown (mic 8ch → mono, reference 2ch → mono) *before*
/// invoking `process`. The DSP works at a fixed frame size configured at
/// construction; `mic`, `reference`, and `out` must all be `frame_size()` long.
pub trait EchoCanceller: Send {
    fn process(&mut self, mic: &[i16], reference: &[i16], out: &mut [i16]);
    fn frame_size(&self) -> usize;
}

/// In-place spectral noise suppression. Optionally tied to an echo canceller's
/// state for residual-echo suppression (see `speex::SpeexNoiseSuppressor`).
pub trait NoiseSuppressor: Send {
    fn process(&mut self, samples: &mut [i16]);
}

/// Returns a no-op canceller. Used when AEC is disabled (spec §8 `Raw` path
/// and unit tests that shouldn't pull in the DSP).
pub fn passthrough(frame_size: usize) -> Box<dyn EchoCanceller> {
    Box::new(passthrough::PassthroughCanceller { frame_size })
}
