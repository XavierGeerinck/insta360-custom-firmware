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
//!     Omni           delay-and-sum on the talker (no subtraction)
//! Stereo is two cardioids at DOA ± 45°.
//!
//! Each pattern output then goes through a bass shelf (`bass.rs`) that undoes
//! the differential's 6 dB/octave roll-off, capped at +12 dB, and a flat
//! make-up gain that puts its talker at Whole room's level over 1–3 kHz
//! (`bass::presence_makeup`), so switching mode keeps the level. A mode change
//! crossfades from the old pattern to the new one over `MODE_FADE_MS`.
//!
//! See `weights.rs` for the partitioning; see SP#3 design doc for the physics.

pub mod geometry;
pub mod aim;
pub mod bass;
pub mod delays;
pub mod steered;
pub mod weights;

pub use geometry::{MicGeometry, Position3D};

use bass::BassShelf;
use delays::DelayLine;
use waved_state::AudioMode;

/// Crossfade from the previous mode's pattern to the new one, so a mode
/// change does not click.
pub const MODE_FADE_MS: u32 = 10;

pub struct Beamformer {
    geom: MicGeometry,
    sample_rate: u32,
    mode: AudioMode,
    doa_az_deg: f32,
    doa_el_deg: f32,
    delays: [DelayLine; 8],
    pattern: Pattern,
    /// The previous mode's pattern while it fades out after a mode change.
    fading: Option<Pattern>,
    fade_pos: u32,
    fade_len: u32,
    /// Samples went through since the last reset; before that a mode change
    /// has nothing to fade from.
    primed: bool,
    /// Bass compensation and make-up on (always, outside tests that measure
    /// the raw pattern).
    bass_on: bool,
}

/// Everything one mode needs to form its output from the delay lines, plus
/// the bass filter state of its outputs.
#[derive(Clone)]
struct Pattern {
    is_omni: bool,
    is_stereo: bool,
    w_front_l: [f32; 8],
    w_back_l: [f32; 8],
    back_delay_l: f32,
    w_front_r: [f32; 8],
    w_back_r: [f32; 8],
    back_delay_r: f32,
    /// Whole room (Omni): per-channel delays that line the eight mics up on
    /// the tracked talker. Summing unaligned mics cancels the 2–6 kHz voice
    /// range off the vertical (−12 dB at 4 kHz from the side, −25 dB with the
    /// mouth 30° up), which made calls sound muffled and far away.
    align_delays: [f32; 8],
    /// In [0, 1]. 0 = pure pattern (cardioid/super/fig8/stereo), 1 = pure
    /// omni-mix output. Steered mode blends towards the omni average; Whole
    /// room blends its aligned sum towards the plain average.
    omni_mix: f32,
    /// Bass compensation of the differential outputs (L, or the mono pattern;
    /// R in Stereo). Not used by Whole room.
    bass_l: BassShelf,
    bass_r: BassShelf,
    /// Flat make-up gains (linear) that put the talker at Whole room's
    /// level over 1–3 kHz (`bass::presence_makeup`). Applied with the shelf,
    /// before the omni blend.
    makeup_l: f32,
    makeup_r: f32,
}

pub struct BeamOutput {
    pub mono: Vec<f32>,
    /// `Some((L, R))` when mode = Stereo; `None` otherwise.
    pub stereo: Option<(Vec<f32>, Vec<f32>)>,
}

impl Beamformer {
    pub fn new(geom: MicGeometry, sample_rate: u32) -> Self {
        let mode = AudioMode::Omni;
        let mut s = Self {
            geom,
            sample_rate,
            pattern: Pattern::new(&mode),
            mode,
            doa_az_deg: 0.0,
            doa_el_deg: 0.0,
            delays: std::array::from_fn(|_| DelayLine::new()),
            fading: None,
            fade_pos: 0,
            fade_len: (sample_rate * MODE_FADE_MS / 1_000).max(1),
            primed: false,
            bass_on: true,
        };
        s.recompute();
        s
    }

    pub fn set_mode(&mut self, mode: AudioMode) {
        if mode != self.mode {
            // Whole room starts on the plain average until it has a talker;
            // the patterns start unblended, with fresh bass filters.
            let old = std::mem::replace(&mut self.pattern, Pattern::new(&mode));
            if self.primed {
                // A second change inside the fade drops the oldest pattern.
                self.fading = Some(old);
                self.fade_pos = 0;
            }
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

    /// Where the pattern points now (azimuth, elevation), in degrees.
    pub fn direction(&self) -> (f32, f32) {
        (self.doa_az_deg, self.doa_el_deg)
    }

    pub fn set_omni_mix(&mut self, mix: f32) {
        self.pattern.omni_mix = mix.clamp(0.0, 1.0);
    }

    /// The current blend towards the plain average: 0 = the pattern (or, in
    /// Whole room, the sum aligned on the talker), 1 = the plain average.
    pub fn omni_mix(&self) -> f32 {
        self.pattern.omni_mix
    }

    /// Measure the raw differential patterns, without shelf or make-up
    /// (tests only).
    #[cfg(test)]
    fn set_bass_compensation(&mut self, on: bool) {
        self.bass_on = on;
    }

    /// The pattern make-up gains (L, R) in dB (tests only).
    #[cfg(test)]
    fn makeup_db(&self) -> (f32, f32) {
        (20.0 * self.pattern.makeup_l.log10(), 20.0 * self.pattern.makeup_r.log10())
    }

    fn recompute(&mut self) {
        self.pattern.steer(
            &self.geom,
            &self.mode,
            self.doa_az_deg,
            self.doa_el_deg,
            self.sample_rate,
        );
    }

    /// Process an 8-channel interleaved f32 frame. `interleaved.len()` must
    /// be a multiple of 8.
    pub fn process(&mut self, interleaved: &[f32]) -> BeamOutput {
        let frames = interleaved.len() / 8;
        let is_stereo = self.pattern.is_stereo;
        let mut mono = Vec::with_capacity(frames);
        let mut stereo_l = if is_stereo { Vec::with_capacity(frames) } else { Vec::new() };
        let mut stereo_r = if is_stereo { Vec::with_capacity(frames) } else { Vec::new() };

        for i in 0..frames {
            for alsa_ch in 0..8 {
                self.delays[alsa_ch].write(interleaved[i * 8 + alsa_ch]);
            }

            let (mut m, mut l, mut r) = self.pattern.sample(&self.delays, self.bass_on);
            if let Some(old) = self.fading.as_mut() {
                let (old_m, old_l, old_r) = old.sample(&self.delays, self.bass_on);
                // Weight of the new pattern, rising linearly over the fade.
                let w = (self.fade_pos + 1) as f32 / (self.fade_len + 1) as f32;
                m = old_m + (m - old_m) * w;
                l = old_l + (l - old_l) * w;
                r = old_r + (r - old_r) * w;
                self.fade_pos += 1;
                if self.fade_pos >= self.fade_len {
                    self.fading = None;
                }
            }

            mono.push(m);
            if is_stereo {
                stereo_l.push(l);
                stereo_r.push(r);
            }
        }
        self.primed |= frames > 0;

        BeamOutput {
            mono,
            stereo: if is_stereo { Some((stereo_l, stereo_r)) } else { None },
        }
    }

    pub fn reset(&mut self) {
        for d in &mut self.delays {
            d.reset();
        }
        self.pattern.bass_l.reset();
        self.pattern.bass_r.reset();
        self.fading = None;
        self.primed = false;
    }
}

impl Pattern {
    fn new(mode: &AudioMode) -> Self {
        let is_omni = matches!(mode, AudioMode::Omni);
        Self {
            is_omni,
            is_stereo: matches!(mode, AudioMode::Stereo),
            w_front_l: [0.0; 8],
            w_back_l: [0.0; 8],
            back_delay_l: 0.0,
            w_front_r: [0.0; 8],
            w_back_r: [0.0; 8],
            back_delay_r: 0.0,
            align_delays: [0.0; 8],
            // Whole room starts with no direction: the plain average.
            omni_mix: if is_omni { 1.0 } else { 0.0 },
            bass_l: BassShelf::default(),
            bass_r: BassShelf::default(),
            makeup_l: 1.0,
            makeup_r: 1.0,
        }
    }

    /// Aim at (az, el). The bass filters are retuned, not reset, so a moving
    /// beam stays smooth.
    fn steer(&mut self, geom: &MicGeometry, mode: &AudioMode, az: f32, el: f32, sample_rate: u32) {
        if self.is_omni {
            self.align_delays = weights::align_delays(geom, az, el, sample_rate);
            return;
        }

        let scale = back_delay_scale(mode.clone());

        let (az_l, az_r) = if self.is_stereo { (az - 45.0, az + 45.0) } else { (az, az) };

        // The bass shelves are tuned for the talker the pattern is for: on
        // its axis the output is front − back delayed by Δ + τ. A Stereo side
        // is for the centre of the image, 45° off its axis, where the
        // acoustic part shrinks to Δ·cos 45°.
        let acoustic = if self.is_stereo { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };

        let hw_l = weights::hemispheric_weights(geom, az_l, el, sample_rate);
        self.w_front_l = hw_l.w_front;
        self.w_back_l = hw_l.w_back;
        self.back_delay_l = (scale * hw_l.delta_samples).max(0.0);
        let d_l = acoustic * hw_l.delta_samples + self.back_delay_l;
        self.bass_l.retune(d_l, sample_rate);
        self.makeup_l = bass::presence_makeup(d_l, sample_rate, &self.bass_l);

        if self.is_stereo {
            let hw_r = weights::hemispheric_weights(geom, az_r, el, sample_rate);
            self.w_front_r = hw_r.w_front;
            self.w_back_r = hw_r.w_back;
            self.back_delay_r = (scale * hw_r.delta_samples).max(0.0);
            let d_r = acoustic * hw_r.delta_samples + self.back_delay_r;
            self.bass_r.retune(d_r, sample_rate);
            self.makeup_r = bass::presence_makeup(d_r, sample_rate, &self.bass_r);
        }
    }

    /// One output sample from the delay lines: (mono, L, R). Outside Stereo
    /// L and R repeat the mono sample.
    fn sample(&mut self, delays: &[DelayLine; 8], bass_on: bool) -> (f32, f32, f32) {
        if self.is_omni {
            // Delay-and-sum on the talker, faded towards the plain
            // average while the direction is unknown (`omni_mix` 1).
            let (mut aligned, mut plain) = (0.0f32, 0.0f32);
            for (alsa_ch, d) in delays.iter().enumerate() {
                aligned += d.read(self.align_delays[alsa_ch]);
                plain += d.read(0.0);
            }
            let mix = self.omni_mix;
            let y = ((1.0 - mix) * aligned + mix * plain) / 8.0;
            return (y, y, y);
        }

        let (mut front_l, mut back_l) = (0.0f32, 0.0f32);
        let (mut front_r, mut back_r) = (0.0f32, 0.0f32);
        for (alsa_ch, d) in delays.iter().enumerate() {
            let x_now = d.read(0.0);
            front_l += self.w_front_l[alsa_ch] * x_now;
            back_l += self.w_back_l[alsa_ch] * d.read(self.back_delay_l);
            if self.is_stereo {
                front_r += self.w_front_r[alsa_ch] * x_now;
                back_r += self.w_back_r[alsa_ch] * d.read(self.back_delay_r);
            }
        }
        // The shelves run every sample so their state stays continuous when
        // the omni blend moves.
        let mut y_l = front_l - back_l;
        if bass_on {
            y_l = self.makeup_l * self.bass_l.process(y_l);
        }

        // Compute omni sample (mean of all 8 mics' current values) if mixed.
        // It is not bass-compensated: the plain average does not roll off.
        let omni_sample = if self.omni_mix > 1e-3 {
            delays.iter().map(|d| d.read(0.0)).sum::<f32>() / 8.0
        } else {
            0.0
        };

        let y_l_mixed = (1.0 - self.omni_mix) * y_l + self.omni_mix * omni_sample;

        if self.is_stereo {
            let mut y_r = front_r - back_r;
            if bass_on {
                y_r = self.makeup_r * self.bass_r.process(y_r);
            }
            let y_r_mixed = (1.0 - self.omni_mix) * y_r + self.omni_mix * omni_sample;
            (0.5 * (y_l_mixed + y_r_mixed), y_l_mixed, y_r_mixed)
        } else {
            (y_l_mixed, y_l_mixed, y_l_mixed)
        }
    }
}

fn back_delay_scale(mode: AudioMode) -> f32 {
    match mode {
        AudioMode::Cardioid | AudioMode::Stereo | AudioMode::Steered { .. } => 1.0,
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

    /// 8-channel plane wave: a unit sine at `freq` from (az, el), `frames` long.
    fn tone(az: f32, el: f32, freq: f32, frames: usize) -> Vec<f32> {
        let geom = MicGeometry::default();
        let d = weights::direction_vector(az, el);
        let omega = 2.0 * std::f32::consts::PI * freq / 48_000.0;
        let mut x = vec![0.0; frames * 8];
        for ch in 0..8 {
            let p = geom.position_for_alsa(ch);
            let tau = (p.x * d.x + p.y * d.y + p.z * d.z) * 1e-3 / 343.0 * 48_000.0;
            for i in 0..frames {
                x[i * 8 + ch] = ((i as f32 + tau) * omega).sin();
            }
        }
        x
    }

    /// Level in dB of a unit sine after settling (the first 50 ms are skipped).
    fn sine_db(y: &[f32]) -> f32 {
        20.0 * (rms(&y[2_400..]) / std::f32::consts::FRAC_1_SQRT_2).log10()
    }

    /// Gain in dB of the beam's mono output for a sine from (az, el).
    fn tone_gain_db(b: &mut Beamformer, az: f32, el: f32, freq: f32) -> f32 {
        let y = b.process(&tone(az, el, freq, 9_600));
        sine_db(&y.mono)
    }

    /// A beam in `mode` aimed straight ahead at the horizon.
    fn aimed(mode: AudioMode) -> Beamformer {
        let mut b = Beamformer::new(MicGeometry::default(), 48_000);
        b.set_mode(mode);
        b.set_doa(0.0, 0.0);
        b
    }

    /// Mono response to a talker in front at el 0, at `freq` relative to 1 kHz.
    fn front_response(mode: AudioMode, freq: f32) -> f32 {
        let mut b = aimed(mode);
        let at_1k = tone_gain_db(&mut b, 0.0, 0.0, 1_000.0);
        b.reset();
        tone_gain_db(&mut b, 0.0, 0.0, freq) - at_1k
    }

    #[test]
    fn directional_patterns_keep_their_bass() {
        // Uncompensated, a front talker in Cardioid read −15.7 dB at 125 Hz
        // and −9.7 dB at 250 Hz against 1 kHz (first-order differential
        // roll-off). Cardioid's own on-axis null sits at fs/D ≈ 4.8 kHz, so
        // its band ends at 3 kHz (see `cardioid_top_octave_is_the_pattern_null`).
        let voice = [125.0, 250.0, 500.0, 2_000.0, 3_000.0];
        let wide = [125.0, 250.0, 500.0, 2_000.0, 3_000.0, 4_000.0];
        let cases: [(AudioMode, &[f32], f32); 4] = [
            (AudioMode::Cardioid, &voice, 3.0),
            (AudioMode::Steered { initial_az_deg: 0.0 }, &voice, 3.0),
            (AudioMode::Supercardioid, &wide, 3.0),
            (AudioMode::Figure8, &wide, 4.0),
        ];
        for (mode, freqs, tol) in cases {
            for &freq in freqs {
                let db = front_response(mode.clone(), freq);
                assert!(db.abs() <= tol, "{mode:?} {freq} Hz: {db:+.1} dB vs 1 kHz (±{tol})");
            }
        }
    }

    /// Mean level (dB) over the presence band, 1–3 kHz, of a front talker
    /// at el 0: (mono, L, R).
    fn presence_db(mode: AudioMode) -> (f32, f32, f32) {
        let freqs = [1_000.0, 1_500.0, 2_000.0, 2_500.0, 3_000.0];
        let (mut m, mut l, mut r) = (0.0, 0.0, 0.0);
        for freq in freqs {
            let mut b = aimed(mode.clone());
            let out = b.process(&tone(0.0, 0.0, freq, 9_600));
            let mono = sine_db(&out.mono);
            let (dl, dr) = out.stereo.map_or((mono, mono), |(sl, sr)| (sine_db(&sl), sine_db(&sr)));
            m += mono;
            l += dl;
            r += dr;
        }
        let n = freqs.len() as f32;
        (m / n, l / n, r / n)
    }

    #[test]
    fn switching_mode_keeps_the_talkers_level() {
        // Every pattern carries a talker in front at the level Whole room
        // gives it once aligned on them, over the voice presence band.
        let whole_room = {
            let mut b = aimed(AudioMode::Omni);
            b.set_omni_mix(0.0);
            let freqs = [1_000.0, 1_500.0, 2_000.0, 2_500.0, 3_000.0];
            freqs
                .iter()
                .map(|&f| {
                    b.reset();
                    tone_gain_db(&mut b, 0.0, 0.0, f)
                })
                .sum::<f32>()
                / freqs.len() as f32
        };
        for mode in [
            AudioMode::Cardioid,
            AudioMode::Supercardioid,
            AudioMode::Figure8,
            AudioMode::Steered { initial_az_deg: 0.0 },
            AudioMode::Stereo,
        ] {
            let (m, l, r) = presence_db(mode.clone());
            for (name, db) in [("mono", m), ("L", l), ("R", r)] {
                assert!(
                    (db - whole_room).abs() <= 1.0,
                    "{mode:?} {name}: {db:+.1} dB vs Whole room {whole_room:+.1} dB"
                );
            }
        }
    }

    #[test]
    fn bass_boost_stays_within_twelve_db_at_50_hz() {
        for mode in [
            AudioMode::Cardioid,
            AudioMode::Supercardioid,
            AudioMode::Figure8,
            AudioMode::Steered { initial_az_deg: 0.0 },
            AudioMode::Stereo,
        ] {
            let mut raw = aimed(mode.clone());
            raw.set_bass_compensation(false);
            let mut compensated = aimed(mode.clone());
            // The flat presence make-up is not bass boost.
            let boost = tone_gain_db(&mut compensated, 0.0, 0.0, 50.0)
                - tone_gain_db(&mut raw, 0.0, 0.0, 50.0)
                - compensated.makeup_db().0;
            assert!(boost > 6.0, "{mode:?}: the bass is lifted ({boost:+.1} dB)");
            assert!(boost <= bass::MAX_BOOST_DB, "{mode:?}: {boost:+.1} dB at 50 Hz");
        }
    }

    #[test]
    fn reset_clears_the_bass_filters() {
        let mut b = aimed(AudioMode::Stereo);
        b.process(&tone(0.0, 0.0, 100.0, 4_800));
        b.reset();
        let out = b.process(&vec![0.0; 8 * 64]);
        let (l, r) = out.stereo.unwrap();
        assert!(out.mono.iter().chain(&l).chain(&r).all(|&v| v == 0.0));
    }

    #[test]
    fn cardioid_top_octave_is_the_pattern_null() {
        // The compensation only shapes the bass; at 4 kHz the cardioid is
        // already falling into its on-axis null (fs/D, D = 10.1 samples).
        // Guard it from getting worse than the design table in the report.
        let db = front_response(AudioMode::Cardioid, 4_000.0);
        assert!(db > -8.0, "Cardioid 4 kHz: {db:+.1} dB vs 1 kHz");
    }

    #[test]
    fn stereo_sides_each_keep_their_bass() {
        // Each side is a cardioid 45° off the front talker; its null for
        // that talker is near 3.9 kHz, so the band checked ends at 2 kHz.
        let side_db = |freq: f32| {
            let mut b = aimed(AudioMode::Stereo);
            let (l, r) = b.process(&tone(0.0, 0.0, freq, 9_600)).stereo.unwrap();
            (sine_db(&l), sine_db(&r))
        };
        let (l_1k, r_1k) = side_db(1_000.0);
        for freq in [125.0, 250.0, 500.0, 2_000.0] {
            let (l, r) = side_db(freq);
            assert!((l - l_1k).abs() <= 3.0, "L {freq} Hz: {:+.1} dB", l - l_1k);
            assert!((r - r_1k).abs() <= 3.0, "R {freq} Hz: {:+.1} dB", r - r_1k);
        }
    }

    #[test]
    fn whole_room_gets_no_bass_boost() {
        for mix in [0.0, 1.0] {
            let mut b = aimed(AudioMode::Omni);
            b.set_omni_mix(mix);
            for freq in [50.0, 125.0, 1_000.0] {
                b.reset();
                let db = tone_gain_db(&mut b, 0.0, 0.0, freq);
                assert!(db.abs() < 0.5, "omni_mix {mix}, {freq} Hz: {db:+.1} dB");
            }
        }
    }

    #[test]
    fn steered_omni_blend_is_not_boosted() {
        // At omni_mix 1 Steered is the plain average: flat, like Whole room
        // without a talker.
        let mut b = aimed(AudioMode::Steered { initial_az_deg: 0.0 });
        b.set_omni_mix(1.0);
        let db = tone_gain_db(&mut b, 0.0, 0.0, 125.0);
        assert!(db.abs() < 0.5, "125 Hz: {db:+.1} dB");
    }

    #[test]
    fn mode_change_does_not_click() {
        // A 150 Hz talker in front: the band the compensation boosts most.
        // The largest sample-to-sample step after a switch must stay within
        // what the louder of the two modes produces on its own.
        let x = tone(0.0, 0.0, 150.0, 9_600);
        let half = 4_800 * 8;
        let max_step = |y: &[f32]| y.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let pairs = [
            (AudioMode::Cardioid, AudioMode::Figure8),
            (AudioMode::Cardioid, AudioMode::Omni),
            (AudioMode::Omni, AudioMode::Stereo),
            (AudioMode::Stereo, AudioMode::Supercardioid),
        ];
        for (from, to) in pairs {
            let steady = |mode: AudioMode| {
                let mut b = aimed(mode);
                max_step(&b.process(&x).mono[2_400..])
            };
            let bound = steady(from.clone()).max(steady(to.clone()));
            let mut b = aimed(from.clone());
            let before = b.process(&x[..half]).mono;
            b.set_mode(to.clone());
            b.set_doa(0.0, 0.0);
            let after = b.process(&x[half..]).mono;
            let joined: Vec<f32> = before[2_400..].iter().chain(&after).copied().collect();
            let step = max_step(&joined);
            assert!(
                step <= 1.5 * bound,
                "{from:?} → {to:?}: step {step:.4} vs steady {bound:.4}"
            );
        }
    }

    #[test]
    fn whole_room_on_the_talker_keeps_the_voice_range() {
        // Mouth 30° above the table, in front: the plain average lost 25 dB
        // at 4 kHz there. Aligned on the talker it stays within ~1.5 dB
        // (linear-interpolation delays cost a little at the top).
        let mut b = Beamformer::new(MicGeometry::default(), 48_000);
        b.set_mode(AudioMode::Omni);
        b.set_doa(0.0, 30.0);
        b.set_omni_mix(0.0);
        for freq in [250.0, 1_000.0, 2_000.0, 3_000.0, 4_000.0, 6_000.0, 8_000.0] {
            b.reset();
            let db = tone_gain_db(&mut b, 0.0, 30.0, freq);
            assert!(db.abs() < 1.5, "{freq} Hz: {db:.1} dB");
        }
    }

    #[test]
    fn whole_room_without_a_direction_is_the_plain_average() {
        let mut b = Beamformer::new(MicGeometry::default(), 48_000);
        b.set_mode(AudioMode::Omni);
        b.set_doa(0.0, 30.0);
        b.set_omni_mix(1.0);
        let db = tone_gain_db(&mut b, 0.0, 30.0, 4_000.0);
        assert!(db < -20.0, "the unaligned average cancels 4 kHz off-axis: {db:.1} dB");
        assert!(tone_gain_db(&mut b, 0.0, 30.0, 250.0).abs() < 0.5);
    }

    #[test]
    fn align_delays_hold_back_the_mic_the_wave_reaches_first() {
        let geom = MicGeometry::default();
        let delays = weights::align_delays(&geom, 0.0, 0.0, 48_000);
        assert!(delays.iter().all(|&d| d >= 0.0));
        assert!(delays.contains(&0.0));
        let front = (0..8).max_by(|&a, &b| {
            geom.position_for_alsa(a).x.total_cmp(&geom.position_for_alsa(b).x)
        });
        let most = (0..8).max_by(|&a, &b| delays[a].total_cmp(&delays[b]));
        assert_eq!(
            geom.position_for_alsa(front.unwrap()).x,
            geom.position_for_alsa(most.unwrap()).x,
            "a front mic waits longest for a wave from the front"
        );
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
