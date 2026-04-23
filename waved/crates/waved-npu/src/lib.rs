//! ONNX Runtime wrapper with VSI-NPU execution provider preference.
//!
//! `OrtInfer` is constructed from a model path; it tries providers in
//! [`providers::preferred_providers()`] order and falls through to CPU.
//! Exposes a single per-frame entry point `run(&[f32]) -> Vec<f32>` sized
//! to match the model's declared output shape.
//!
//! This crate's public surface is stable from Task 1. The real ORT session
//! lands in Task 6 — `OrtInfer::new` currently errors so that downstream
//! callers (`DeepFilterNs` in Task 7) can compile against the final trait
//! shape without waiting for the toolchain work.

use anyhow::Result;
use std::path::Path;

pub mod providers;

pub trait NpuInfer: Send {
    /// Run one forward pass. `input.len()` must match the declared input
    /// size; returned vector length equals the declared output size.
    fn run(&mut self, input: &[f32]) -> Result<Vec<f32>>;

    /// Name of the execution provider actually used at construction —
    /// "vsi_npu", "cpu", etc. Used by stats_collector for `ns_backend`.
    fn provider(&self) -> &'static str;
}

pub struct OrtInfer {
    _stub: (),
}

impl OrtInfer {
    /// Placeholder constructor; real body in Task 6.
    pub fn new(_model_path: &Path) -> Result<Self> {
        anyhow::bail!("OrtInfer not yet implemented — see Task 6")
    }
}

impl NpuInfer for OrtInfer {
    fn run(&mut self, _input: &[f32]) -> Result<Vec<f32>> {
        anyhow::bail!("OrtInfer not yet implemented — see Task 6")
    }

    fn provider(&self) -> &'static str {
        "stub"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn stub_constructor_errors_cleanly() {
        let r = OrtInfer::new(&PathBuf::from("/nonexistent"));
        assert!(r.is_err(), "stub should error, not panic");
    }

    #[test]
    fn provider_name_is_static_str() {
        // Compile-time assertion that the trait signature is what we expect.
        // If a future edit changes `provider` to return `String` or take
        // `&mut self`, this won't compile.
        fn _check<T: NpuInfer>() {
            let _: fn(&T) -> &'static str = T::provider;
        }
        _check::<OrtInfer>();
    }
}
