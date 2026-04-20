//! Beamforming: turn 8-ch raw mic into a direction-steered mono (or stereo) signal.
//!
//! Algorithm: first-order differential beamforming over hemispheric weighted
//! sums. For a steered DOA d̂ the mics are split into front/back hemispheres
//! and combined as
//!     y(t) = front_sum(t) − α · back_sum(t − scale · Δ)
//! where Δ is the centroid-to-centroid time-of-flight along d̂. Scaling the
//! back delay sweeps the first-order pattern family:
//!     Cardioid       scale = 1.000, α = 1
//!     Supercardioid  scale = 0.577, α = 1
//!     Figure-8       scale = 0.000, α = 1
//!     Omni           plain 8-mic average (no delay, no subtraction)
//! Stereo is two cardioids at DOA ± 45°.
//!
//! See `weights.rs` for the partitioning; see SP#3 design doc for the physics.

pub mod geometry;
pub mod delays;
pub mod weights;

pub use geometry::{MicGeometry, Position3D};

use delays::DelayLine;
use waved_state::AudioMode;

pub struct Beamformer {
    geom: MicGeometry,
    sample_rate: u32,
    mode: AudioMode,
    doa_az_deg: f32,
    doa_el_deg: f32,
    delays: [DelayLine; 8],
    w_front_l: [f32; 8],
    w_back_l: [f32; 8],
    back_delay_l: f32,
    w_front_r: [f32; 8],
    w_back_r: [f32; 8],
    back_delay_r: f32,
    is_stereo: bool,
    is_omni: bool,
}

pub struct BeamOutput {
    pub mono: Vec<f32>,
    /// `Some((L, R))` when mode = Stereo; `None` otherwise.
    pub stereo: Option<(Vec<f32>, Vec<f32>)>,
}

impl Beamformer {
    pub fn new(geom: MicGeometry, sample_rate: u32) -> Self {
        let mut s = Self {
            geom,
            sample_rate,
            mode: AudioMode::Omni,
            doa_az_deg: 0.0,
            doa_el_deg: 0.0,
            delays: std::array::from_fn(|_| DelayLine::new()),
            w_front_l: [0.0; 8],
            w_back_l: [0.0; 8],
            back_delay_l: 0.0,
            w_front_r: [0.0; 8],
            w_back_r: [0.0; 8],
            back_delay_r: 0.0,
            is_stereo: false,
            is_omni: true,
        };
        s.recompute();
        s
    }

    pub fn set_mode(&mut self, mode: AudioMode) {
        if mode != self.mode {
            self.mode = mode;
            self.recompute();
        }
    }

    pub fn set_doa(&mut self, az_deg: f32, el_deg: f32) {
        if (az_deg - self.doa_az_deg).abs() > 0.1 || (el_deg - self.doa_el_deg).abs() > 0.1 {
            self.doa_az_deg = az_deg;
            self.doa_el_deg = el_deg;
            self.recompute();
        }
    }

    fn recompute(&mut self) {
        self.is_omni = matches!(self.mode, AudioMode::Omni);
        self.is_stereo = matches!(self.mode, AudioMode::Stereo);
        if self.is_omni {
            return;
        }

        let scale = back_delay_scale(self.mode.clone());

        let (az_l, az_r) = if self.is_stereo {
            (self.doa_az_deg - 45.0, self.doa_az_deg + 45.0)
        } else {
            (self.doa_az_deg, self.doa_az_deg)
        };

        let hw_l = weights::hemispheric_weights(&self.geom, az_l, self.doa_el_deg, self.sample_rate);
        self.w_front_l = hw_l.w_front;
        self.w_back_l = hw_l.w_back;
        self.back_delay_l = (scale * hw_l.delta_samples).max(0.0);

        if self.is_stereo {
            let hw_r =
                weights::hemispheric_weights(&self.geom, az_r, self.doa_el_deg, self.sample_rate);
            self.w_front_r = hw_r.w_front;
            self.w_back_r = hw_r.w_back;
            self.back_delay_r = (scale * hw_r.delta_samples).max(0.0);
        }
    }

    /// Process an 8-channel interleaved f32 frame. `interleaved.len()` must
    /// be a multiple of 8.
    pub fn process(&mut self, interleaved: &[f32]) -> BeamOutput {
        let frames = interleaved.len() / 8;
        let mut mono = Vec::with_capacity(frames);
        let mut stereo_l = if self.is_stereo { Vec::with_capacity(frames) } else { Vec::new() };
        let mut stereo_r = if self.is_stereo { Vec::with_capacity(frames) } else { Vec::new() };

        for i in 0..frames {
            for alsa_ch in 0..8 {
                self.delays[alsa_ch].write(interleaved[i * 8 + alsa_ch]);
            }

            if self.is_omni {
                let mut sum = 0.0f32;
                for alsa_ch in 0..8 {
                    sum += self.delays[alsa_ch].read(0.0);
                }
                mono.push(sum / 8.0);
                continue;
            }

            let (mut front_l, mut back_l) = (0.0f32, 0.0f32);
            let (mut front_r, mut back_r) = (0.0f32, 0.0f32);
            for alsa_ch in 0..8 {
                let x_now = self.delays[alsa_ch].read(0.0);
                let x_del_l = self.delays[alsa_ch].read(self.back_delay_l);
                front_l += self.w_front_l[alsa_ch] * x_now;
                back_l += self.w_back_l[alsa_ch] * x_del_l;
                if self.is_stereo {
                    let x_del_r = self.delays[alsa_ch].read(self.back_delay_r);
                    front_r += self.w_front_r[alsa_ch] * x_now;
                    back_r += self.w_back_r[alsa_ch] * x_del_r;
                }
            }
            let y_l = front_l - back_l;

            if self.is_stereo {
                let y_r = front_r - back_r;
                mono.push(0.5 * (y_l + y_r));
                stereo_l.push(y_l);
                stereo_r.push(y_r);
            } else {
                mono.push(y_l);
            }
        }

        BeamOutput {
            mono,
            stereo: if self.is_stereo { Some((stereo_l, stereo_r)) } else { None },
        }
    }

    pub fn reset(&mut self) {
        for d in &mut self.delays {
            d.reset();
        }
    }
}

fn back_delay_scale(mode: AudioMode) -> f32 {
    match mode {
        AudioMode::Cardioid | AudioMode::Stereo => 1.0,
        AudioMode::Supercardioid => 0.577,
        AudioMode::Figure8 => 0.0,
        AudioMode::Omni => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthesize_plane_wave(
        az_deg: f32,
        el_deg: f32,
        sr: u32,
        frames: usize,
        geom: &MicGeometry,
    ) -> Vec<f32> {
        let d = weights::direction_vector(az_deg, el_deg);
        let freq = 1000.0f32;
        let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
        let mut out = vec![0.0; frames * 8];
        for alsa_ch in 0..8 {
            let p = geom.position_for_alsa(alsa_ch);
            let dot_mm = p.x * d.x + p.y * d.y + p.z * d.z;
            // Signal arrives at mic i at time t_i = (-p·d)/c relative to origin;
            // for a sinusoid s(t)=sin(ωt) arriving from direction d, mic i sees
            // sin(ω(t + p·d/c)) (mic closer to source leads origin).
            let tau_samples = (dot_mm * 1e-3 / 343.0) * sr as f32;
            for i in 0..frames {
                out[i * 8 + alsa_ch] = ((i as f32 + tau_samples) * omega).sin();
            }
        }
        out
    }

    fn rms(xs: &[f32]) -> f32 {
        if xs.is_empty() {
            return 0.0;
        }
        let sq: f32 = xs.iter().map(|x| x * x).sum();
        (sq / xs.len() as f32).sqrt()
    }

    #[test]
    fn omni_is_nondirectional() {
        let geom = MicGeometry::default();
        let mut bf = Beamformer::new(geom.clone(), 48000);
        bf.set_mode(AudioMode::Omni);

        let front = synthesize_plane_wave(0.0, 0.0, 48000, 4800, &geom);
        let back = synthesize_plane_wave(180.0, 0.0, 48000, 4800, &geom);

        let rms_front = rms(&bf.process(&front).mono[480..]);
        bf.reset();
        let rms_back = rms(&bf.process(&back).mono[480..]);

        let ratio_db = 20.0 * (rms_front / rms_back).log10();
        assert!(ratio_db.abs() < 1.0, "Omni front/back ratio {ratio_db} dB should be ~0");
    }

    #[test]
    fn cardioid_achieves_ten_db_front_to_back() {
        let geom = MicGeometry::default();
        let mut bf = Beamformer::new(geom.clone(), 48000);
        bf.set_mode(AudioMode::Cardioid);
        bf.set_doa(0.0, 0.0);

        let front = synthesize_plane_wave(0.0, 0.0, 48000, 4800, &geom);
        let back = synthesize_plane_wave(180.0, 0.0, 48000, 4800, &geom);

        let rms_front = rms(&bf.process(&front).mono[480..]);
        bf.reset();
        let rms_back = rms(&bf.process(&back).mono[480..]);

        let fbr_db = 20.0 * (rms_front / rms_back.max(1e-6)).log10();
        assert!(fbr_db >= 10.0, "Cardioid FBR {fbr_db:.1} dB < 10 dB spec");
    }

    #[test]
    fn stereo_produces_two_channels_with_different_content() {
        let geom = MicGeometry::default();
        let mut bf = Beamformer::new(geom.clone(), 48000);
        bf.set_mode(AudioMode::Stereo);
        bf.set_doa(0.0, 0.0);

        let source = synthesize_plane_wave(90.0, 0.0, 48000, 4800, &geom);
        let out = bf.process(&source);
        let (l, r) = out.stereo.expect("stereo mode produces stereo");
        assert_eq!(l.len(), r.len());
        let diff: f32 = l.iter().zip(r.iter()).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0, "L and R too similar (sum|diff|={diff})");
    }

    #[test]
    fn figure8_rejects_off_axis() {
        let geom = MicGeometry::default();
        let mut bf = Beamformer::new(geom.clone(), 48000);
        bf.set_mode(AudioMode::Figure8);
        bf.set_doa(0.0, 0.0);

        let front = synthesize_plane_wave(0.0, 0.0, 48000, 4800, &geom);
        let rms_front = rms(&bf.process(&front).mono[480..]);
        bf.reset();

        let side = synthesize_plane_wave(90.0, 0.0, 48000, 4800, &geom);
        let rms_side = rms(&bf.process(&side).mono[480..]);

        let ratio_db = 20.0 * (rms_front / rms_side.max(1e-6)).log10();
        assert!(ratio_db >= 15.0, "Figure-8 90°-rejection {ratio_db:.1} dB < 15 dB");
    }
}
