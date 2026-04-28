//! Neural inference backends for waved-audio's NS pipeline.
//!
//! `NpuInfer` is the abstract surface; `Dfn3CpuInfer` is the only impl today,
//! wrapping DeepFilterNet 3 via the upstream `deep_filter` crate (which uses
//! `tract` under the hood with NEON kernels on aarch64). The NPU backends we
//! probed earlier (Vivante VIP9000) are blocked on toolchain access — see
//! `docs/COMPUTE_HARDWARE.md`. Future NPU impls plug into the same trait.
//!
//! The bundled DFN3 low-latency model ships inside the binary via the
//! `default-model-ll` cargo feature on `deep_filter`. No separate model file
//! to deploy.

use anyhow::{Context, Result};
use ndarray::Array2;

/// Per-frame neural inference. Implementations expose a fixed sample rate
/// and hop size; callers must match those before invoking `run`.
pub trait NpuInfer: Send {
    /// Run one forward pass on a single mono frame of `hop_size()` samples.
    /// Returns the enhanced frame as a fresh `Vec<f32>` of the same length.
    fn run(&mut self, input: &[f32]) -> Result<Vec<f32>>;

    /// Backend name — surfaced to `/tmp/audio_stats.json` as `ns_backend`.
    fn provider(&self) -> &'static str;

    /// Sample rate the backend's model expects (e.g. 48_000 for DFN3).
    fn sr(&self) -> usize;

    /// Frame size (samples per channel) per call to `run`.
    fn hop_size(&self) -> usize;
}

/// DeepFilterNet 3 inference on CPU. Uses the bundled low-latency model
/// shipped via `default-model-ll`. Single channel, 48 kHz, 10 ms hop.
///
/// `atten_lim_db` caps how much attenuation DFN3 applies — 100 dB ≈ no cap
/// (full enhancement), 12 dB is a gentle floor that keeps a touch of room
/// tone, 0 dB disables the model entirely (caller should bypass instead).
pub struct Dfn3CpuInfer {
    model: df::tract::DfTract,
    sr: usize,
    hop_size: usize,
    noisy_buf: Array2<f32>,
    enh_buf: Array2<f32>,
}

// `DfTract` holds `Rc<Tensor>` internally (tract's single-threaded runtime
// model), making it !Send by default. We never share `Dfn3CpuInfer` across
// threads — `AudioService` constructs it once and moves it into the
// processing thread. tract spawns no internal threads, so the Rcs never
// escape; moving the whole struct between threads is sound.
unsafe impl Send for Dfn3CpuInfer {}

impl Dfn3CpuInfer {
    /// Default — 100 dB attenuation cap (full DFN3 enhancement).
    pub fn new() -> Result<Self> {
        Self::with_atten_lim_db(100.0)
    }

    /// Construct with a specific attenuation cap (in dB). Maps to DFN3's
    /// `RuntimeParams::with_atten_lim` — the model mixes back some of the
    /// noisy signal so output is at most this many dB quieter than input.
    pub fn with_atten_lim_db(atten_lim_db: f32) -> Result<Self> {
        let r_params = df::tract::RuntimeParams::default().with_atten_lim(atten_lim_db);
        let df_params = df::tract::DfParams::default();
        let model = df::tract::DfTract::new(df_params, &r_params)
            .context("DfTract::new (bundled default-model-ll)")?;
        let sr = model.sr;
        let hop_size = model.hop_size;
        let noisy_buf = Array2::<f32>::zeros((1, hop_size));
        let enh_buf = Array2::<f32>::zeros((1, hop_size));
        tracing::info!(sr, hop_size, atten_lim_db, "Dfn3CpuInfer initialized");
        Ok(Self { model, sr, hop_size, noisy_buf, enh_buf })
    }

    /// Hot-swap the attenuation cap without rebuilding the session — much
    /// cheaper than `new()` (~1.5 s) so this is the right hook for a live
    /// NR-strength UI change.
    pub fn set_atten_lim_db(&mut self, atten_lim_db: f32) {
        self.model.set_atten_lim(atten_lim_db);
    }
}

impl NpuInfer for Dfn3CpuInfer {
    fn run(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        if input.len() != self.hop_size {
            anyhow::bail!(
                "input len {} != hop_size {}",
                input.len(),
                self.hop_size
            );
        }
        self.noisy_buf
            .row_mut(0)
            .as_slice_mut()
            .expect("contiguous row")
            .copy_from_slice(input);
        let _lsnr = self
            .model
            .process(self.noisy_buf.view(), self.enh_buf.view_mut())
            .context("DfTract::process")?;
        Ok(self
            .enh_buf
            .row(0)
            .as_slice()
            .expect("contiguous row")
            .to_vec())
    }

    fn provider(&self) -> &'static str {
        "deep_filter_cpu"
    }

    fn sr(&self) -> usize {
        self.sr
    }

    fn hop_size(&self) -> usize {
        self.hop_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dfn3_constructs_with_bundled_model() {
        let m = Dfn3CpuInfer::new().expect("bundled model should load");
        assert_eq!(m.sr(), 48_000);
        assert_eq!(m.hop_size(), 480);
        assert_eq!(m.provider(), "deep_filter_cpu");
    }

    #[test]
    fn dfn3_passes_silence_through_clean() {
        let mut m = Dfn3CpuInfer::new().expect("bundled model should load");
        let silence = vec![0.0f32; m.hop_size()];
        let out = m.run(&silence).expect("inference on silence should not error");
        assert_eq!(out.len(), m.hop_size());
        let max = out.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        assert!(max < 1e-3, "silence in -> ~silence out, got peak {}", max);
    }

    #[test]
    fn dfn3_rejects_wrong_frame_size() {
        let mut m = Dfn3CpuInfer::new().expect("bundled model should load");
        assert!(m.run(&[0.0f32; 100]).is_err());
        assert!(m.run(&[0.0f32; 1024]).is_err());
    }

    #[test]
    fn dfn3_attenuates_white_noise() {
        let mut m = Dfn3CpuInfer::new().expect("bundled model should load");
        // White-noise burst at moderate level; DFN3 should reduce energy.
        let mut x = 0u32;
        let noisy: Vec<f32> = (0..m.hop_size())
            .map(|_| {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                ((x >> 16) as i32 as f32) / 32768.0 * 0.3
            })
            .collect();
        let in_rms = (noisy.iter().map(|s| s * s).sum::<f32>() / noisy.len() as f32).sqrt();
        // Need a few frames of warm-up since DFN3 has lookahead and internal state.
        let mut last = vec![0.0f32; m.hop_size()];
        for _ in 0..16 {
            last = m.run(&noisy).unwrap();
        }
        let out_rms = (last.iter().map(|s| s * s).sum::<f32>() / last.len() as f32).sqrt();
        assert!(
            out_rms < in_rms * 0.7,
            "expected ≥ 3 dB attenuation, got in={} out={}",
            in_rms,
            out_rms
        );
    }
}
