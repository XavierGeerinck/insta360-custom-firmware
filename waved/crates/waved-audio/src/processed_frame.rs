//! DSP boundary frame: mic signal after linear AEC, residual echo suppression,
//! environmental NS, user gain, and limiting, paired with the raw 8-ch mic
//! the meeting recorder / stats still want.
//!
//! See spec §4 for why we carry both the processed mono and the raw 8-ch
//! buffer: stats want per-channel RMS on raw mic, meeting replay wants raw
//! for future reprocessing, and the uplink wants the processed mono (or, in
//! Stereo mode, the pair rebuilt around it by `rebuild_stereo`).

use crate::callbacks::AudioFrame;
use serde::Serialize;

/// Reference activity floor in the i16 RMS domain. ERLE is not meaningful
/// below this level, so derived suppression fields stay absent.
pub const AEC_REFERENCE_ACTIVITY_RMS: f32 = 10.0;

/// Metrics captured at exact boundaries of one processed audio frame.
///
/// All RMS fields use the i16 scale even though some intermediate buffers are
/// f32. Pre-gain fields therefore remain directly comparable without the
/// software microphone gain corrupting ERLE.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AecFrameMetrics {
    pub reference_rms: f32,
    pub mic_pre_aec_rms: f32,
    pub mic_post_linear_aec_rms: f32,
    pub mic_post_residual_echo_rms: f32,
    /// After the dereverb and voice EQ (`crate::uplink_voice`).
    pub mic_post_voice_rms: f32,
    pub mic_post_noise_suppression_rms: f32,
    pub mic_uplink_rms: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linear_erle_db: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_echo_suppression_db: Option<f32>,
    pub residual_echo_suppression_active: bool,
    pub residual_echo_suppress_db: u8,
    pub residual_echo_suppress_active_db: u8,
    /// Fixed gain before noise suppression (`crate::makeup`), in dB.
    pub uplink_makeup_gain_db: u8,
    pub noise_suppression_backend: String,
    /// What the beam did this frame; `None` off the 8-channel path. The
    /// stats publish it at the top level (`AudioStats::beam`), not here.
    #[serde(skip)]
    pub beam: Option<BeamMetrics>,
}

/// Where the beam pointed and how far it leaned on the plain average for
/// one frame (C19): shows whether Whole room's talker alignment and the
/// steered tracker engage in a real room.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct BeamMetrics {
    pub az_deg: f32,
    pub el_deg: f32,
    /// 0 = the pattern (Whole room: the sum aligned on the talker),
    /// 1 = the plain 8-mic average.
    pub omni_mix: f32,
}

impl Default for AecFrameMetrics {
    fn default() -> Self {
        Self {
            reference_rms: 0.0,
            mic_pre_aec_rms: 0.0,
            mic_post_linear_aec_rms: 0.0,
            mic_post_residual_echo_rms: 0.0,
            mic_post_voice_rms: 0.0,
            mic_post_noise_suppression_rms: 0.0,
            mic_uplink_rms: 0.0,
            linear_erle_db: None,
            effective_echo_suppression_db: None,
            residual_echo_suppression_active: false,
            residual_echo_suppress_db: 0,
            residual_echo_suppress_active_db: 0,
            uplink_makeup_gain_db: 0,
            noise_suppression_backend: "off".to_string(),
            beam: None,
        }
    }
}

impl AecFrameMetrics {
    pub fn update_suppression(&mut self) {
        self.linear_erle_db = None;
        self.effective_echo_suppression_db = None;
        if self.reference_rms < AEC_REFERENCE_ACTIVITY_RMS {
            return;
        }
        self.linear_erle_db = attenuation_db(self.mic_pre_aec_rms, self.mic_post_linear_aec_rms);
        // The fixed make-up gain raises speech and residual echo alike; leave
        // it out so the figure stays comparable with builds before it.
        self.effective_echo_suppression_db = attenuation_db(self.mic_pre_aec_rms, self.mic_uplink_rms)
            .map(|db| db + f32::from(self.uplink_makeup_gain_db));
    }
}

fn attenuation_db(input_rms: f32, output_rms: f32) -> Option<f32> {
    if !input_rms.is_finite() || !output_rms.is_finite() || input_rms <= 0.0 || output_rms < 0.0 {
        return None;
    }
    let db = 20.0 * (input_rms / output_rms.max(0.01)).log10();
    Some(db.clamp(-120.0, 120.0))
}

#[derive(Debug, Clone)]
pub struct ProcessedAudioFrame {
    /// Final uplink mono signal after all AEC, optional environmental NS,
    /// user gain, and limiting, in [-1.0, 1.0].
    pub mic_mono_f32: Vec<f32>,
    /// Optional stereo pair produced by Stereo beam mode (L, R).
    /// Same length as `mic_mono_f32` when present.
    pub mic_stereo_f32: Option<(Vec<f32>, Vec<f32>)>,
    /// Reference signal fed into AEC (mono downmix, float). Kept for
    /// diagnostic replay and in-process ERLE measurement.
    pub reference_f32: Vec<f32>,
    pub sample_rate: u32,
    pub timestamp_ms: u64,
    /// Original interleaved 8-ch mic data as captured — used by meeting
    /// recording (diagnostic sidecar) and stats.
    pub raw_mic_i16: Vec<i16>,
    pub raw_mic_channels: u16,
    pub aec_metrics: AecFrameMetrics,
}

impl ProcessedAudioFrame {
    /// Build from a raw `AudioFrame` plus the DSP-produced mono mic and mono
    /// reference (both i16, native speex representation). Allocates the
    /// float buffers once here.
    pub fn from_raw(frame: &AudioFrame, mic_mono_i16: &[i16], reference_mono_i16: &[i16]) -> Self {
        let mut mic_mono_f32 = vec![0.0f32; mic_mono_i16.len()];
        i16_to_f32(mic_mono_i16, &mut mic_mono_f32);

        let mut reference_f32 = vec![0.0f32; reference_mono_i16.len()];
        i16_to_f32(reference_mono_i16, &mut reference_f32);

        Self {
            mic_mono_f32,
            mic_stereo_f32: None,
            reference_f32,
            sample_rate: frame.sample_rate,
            timestamp_ms: frame.timestamp_ms,
            raw_mic_i16: frame.data.clone(),
            raw_mic_channels: frame.channels,
            aec_metrics: AecFrameMetrics::default(),
        }
    }

    /// Attach a stereo pair. L and R must match `mic_mono_f32.len()`.
    pub fn with_stereo(mut self, l: Vec<f32>, r: Vec<f32>) -> Self {
        assert_eq!(
            l.len(),
            r.len(),
            "stereo length mismatch: L={} R={}",
            l.len(),
            r.len()
        );
        assert_eq!(
            l.len(),
            self.mic_mono_f32.len(),
            "stereo length mismatch vs mono: stereo={} mono={}",
            l.len(),
            self.mic_mono_f32.len()
        );
        self.mic_stereo_f32 = Some((l, r));
        self
    }

    /// Interleaved S16 for the 2-channel USB microphone: the Stereo pair
    /// when present, otherwise the mono signal on both channels.
    pub fn uplink_interleaved_i16(&self, out: &mut Vec<i16>) {
        out.clear();
        out.reserve(self.mic_mono_f32.len() * 2);
        match &self.mic_stereo_f32 {
            Some((left, right)) => {
                for (l, r) in left.iter().zip(right) {
                    out.push(to_uplink_i16(*l));
                    out.push(to_uplink_i16(*r));
                }
            }
            None => {
                for &m in &self.mic_mono_f32 {
                    let s = to_uplink_i16(m);
                    out.push(s);
                    out.push(s);
                }
            }
        }
    }
}

/// Convert i16 → f32 scaled to [-1.0, 1.0]. Division by 32768.0 — not 32767 —
/// so the mapping is exact for i16::MIN and matches conventional DSP scaling.
pub fn i16_to_f32(src: &[i16], dst: &mut [f32]) {
    assert_eq!(src.len(), dst.len(), "src/dst length mismatch");
    let scale = 1.0 / 32768.0;
    for (s, d) in src.iter().zip(dst.iter_mut()) {
        *d = (*s as f32) * scale;
    }
}

/// Convert f32 → i16 with saturating clamp. Matches the meeting-encode
/// boundary.
pub fn f32_to_i16_saturating(src: &[f32], dst: &mut [i16]) {
    assert_eq!(src.len(), dst.len(), "src/dst length mismatch");
    for (s, d) in src.iter().zip(dst.iter_mut()) {
        let scaled = (*s * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32);
        *d = scaled as i16;
    }
}

/// Downmix interleaved `channels`-ch i16 audio to mono by averaging. i32
/// accumulation guarantees we don't wrap when channels sum above i16 range.
pub fn mix_to_mono(interleaved: &[i16], channels: usize) -> Vec<i16> {
    assert!(channels > 0, "channels must be > 0");
    assert_eq!(
        interleaved.len() % channels,
        0,
        "interleaved length {} not a multiple of channels {}",
        interleaved.len(),
        channels
    );
    let frames = interleaved.len() / channels;
    let mut mono = vec![0i16; frames];
    for (i, slot) in mono.iter_mut().enumerate() {
        let base = i * channels;
        let sum: i32 = (0..channels)
            .map(|c| interleaved[base + c] as i32)
            .sum::<i32>();
        *slot = (sum / channels as i32) as i16;
    }
    mono
}

fn rms_f32(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// How much of the beam's side signal to keep: the fraction of the mid that
/// survived AEC and noise suppression, clamped to [0, 1]. Zero when the beam
/// was silent.
pub fn side_gain(raw_mid_rms: f32, processed_mid_rms: f32) -> f32 {
    if raw_mid_rms <= 1e-6 {
        return 0.0;
    }
    (processed_mid_rms / raw_mid_rms).clamp(0.0, 1.0)
}

/// Rebuild the Stereo-mode pair around the processed mid (C4). The raw beam
/// pair comes before AEC and NS, so it is not sent as is (the caller delays
/// it by the mid's DSP latency first, `crate::stereo_align`):
/// `L = mid' + g·side`, `R = mid' − g·side`, with `side = (L − R)/2` from
/// the beam and `g` the mid's suppression ratio, ramped linearly from
/// `prev_side_gain` across the period so it never clicks. `makeup` is the
/// fixed gain the mid received before noise suppression; the side gets the
/// same, and the ratio is measured without it. `(L + R)/2` is
/// the processed mid (before the recording limiters, which run separately
/// on the mono signal and the pair). Returns the pair and the gain reached.
pub fn rebuild_stereo(
    raw_l: &[f32],
    raw_r: &[f32],
    processed_mid: &[f32],
    prev_side_gain: f32,
    makeup: f32,
) -> (Vec<f32>, Vec<f32>, f32) {
    let n = processed_mid.len().min(raw_l.len()).min(raw_r.len());
    let raw_mid: Vec<f32> = raw_l[..n]
        .iter()
        .zip(&raw_r[..n])
        .map(|(l, r)| 0.5 * (l + r))
        .collect();
    let target = side_gain(rms_f32(&raw_mid) * makeup, rms_f32(&processed_mid[..n]));
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    for i in 0..n {
        let gain = prev_side_gain + (target - prev_side_gain) * (i + 1) as f32 / n as f32;
        let side = 0.5 * (raw_l[i] - raw_r[i]) * gain * makeup;
        left.push(processed_mid[i] + side);
        right.push(processed_mid[i] - side);
    }
    (left, right, target)
}

fn to_uplink_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_to_f32_roundtrip_preserves_samples() {
        let src: Vec<i16> = vec![-32768, -1, 0, 1, 32767, 16384, -16384];
        let mut f = vec![0.0f32; src.len()];
        i16_to_f32(&src, &mut f);
        let mut back = vec![0i16; src.len()];
        f32_to_i16_saturating(&f, &mut back);
        // Round-trip exact for values that are not i16::MAX (32767 * 1/32768
        // is 0.99996… which scales back to 32766 — i.e. off by 1). That's
        // expected and matches conventional DSP scaling.
        for (orig, rt) in src.iter().zip(back.iter()) {
            let diff = (*orig as i32 - *rt as i32).abs();
            assert!(
                diff <= 1,
                "i16 {} roundtripped to {} (diff {})",
                orig,
                rt,
                diff
            );
        }
    }

    #[test]
    fn f32_clamps_out_of_range_values() {
        let src = [2.0, -2.0, 0.5, -0.5];
        let mut dst = [0i16; 4];
        f32_to_i16_saturating(&src, &mut dst);
        assert_eq!(dst[0], i16::MAX);
        assert_eq!(dst[1], i16::MIN);
        assert_eq!(dst[2], 16384);
        assert_eq!(dst[3], -16384);
    }

    #[test]
    fn mix_mono_identical_channels_equals_input() {
        let mono = [100i16, 200, -300, 400];
        let mut stereo = vec![0i16; mono.len() * 2];
        for (i, &s) in mono.iter().enumerate() {
            stereo[2 * i] = s;
            stereo[2 * i + 1] = s;
        }
        let mixed = mix_to_mono(&stereo, 2);
        assert_eq!(mixed, mono);
    }

    #[test]
    fn mix_mono_opposite_channels_cancels() {
        let stereo: Vec<i16> = vec![100, -100, 500, -500, -200, 200];
        let mixed = mix_to_mono(&stereo, 2);
        assert_eq!(mixed, vec![0, 0, 0]);
    }

    #[test]
    fn mix_mono_8ch_averages() {
        // 8 identical channels of the same value — averaged value equals it.
        let mut interleaved = vec![0i16; 8 * 4];
        for frame in 0..4 {
            for ch in 0..8 {
                interleaved[frame * 8 + ch] = 300 * (frame as i16 + 1);
            }
        }
        let mixed = mix_to_mono(&interleaved, 8);
        assert_eq!(mixed, vec![300, 600, 900, 1200]);
    }

    #[test]
    fn default_has_none_for_stereo() {
        let frame = AudioFrame::mic_only(vec![1, 2, 3, 4], 1, 48000, 17);
        let mic_mono = [1000i16, 2000, -3000, 4000];
        let ref_mono = [0i16; 4];
        let p = ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono);
        assert!(p.mic_stereo_f32.is_none());
    }

    #[test]
    fn with_stereo_accepts_and_preserves_lengths() {
        let frame = AudioFrame::mic_only(vec![1, 2, 3, 4], 1, 48000, 17);
        let mic_mono = [1000i16; 4];
        let ref_mono = [0i16; 4];
        let p = ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono)
            .with_stereo(vec![0.1, 0.2, 0.3, 0.4], vec![-0.1, -0.2, -0.3, -0.4]);
        let (l, r) = p.mic_stereo_f32.expect("stereo set");
        assert_eq!(l.len(), 4);
        assert_eq!(r.len(), 4);
        assert!((l[2] - 0.3).abs() < 1e-6);
        assert!((r[2] + 0.3).abs() < 1e-6);
        assert_eq!(p.mic_mono_f32.len(), 4);
    }

    #[test]
    #[should_panic(expected = "stereo length mismatch")]
    fn with_stereo_rejects_mismatched_lengths() {
        let frame = AudioFrame::mic_only(vec![1, 2], 1, 48000, 0);
        let mic_mono = [100i16; 2];
        let ref_mono = [0i16; 2];
        let _ = ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono)
            .with_stereo(vec![0.1, 0.2, 0.3], vec![-0.1, -0.2]);
    }

    #[test]
    fn from_raw_carries_metadata_and_raw_mic() {
        let frame = AudioFrame::mic_only(vec![1, 2, 3, 4], 1, 48000, 17);
        let mic_mono = [1000i16, 2000, -3000, 4000];
        let ref_mono = [0i16; 4];
        let p = ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono);
        assert_eq!(p.sample_rate, 48000);
        assert_eq!(p.timestamp_ms, 17);
        assert_eq!(p.raw_mic_i16, vec![1, 2, 3, 4]);
        assert_eq!(p.raw_mic_channels, 1);
        assert_eq!(p.mic_mono_f32.len(), 4);
        assert_eq!(p.reference_f32.len(), 4);
        // f32 scaled exactly
        assert!((p.mic_mono_f32[0] - 1000.0 / 32768.0).abs() < 1e-6);
    }

    #[test]
    fn aec_metrics_compute_same_frame_suppression() {
        let mut metrics = AecFrameMetrics {
            reference_rms: 100.0,
            mic_pre_aec_rms: 1000.0,
            mic_post_linear_aec_rms: 100.0,
            mic_post_residual_echo_rms: 10.0,
            mic_post_noise_suppression_rms: 10.0,
            mic_uplink_rms: 40.0,
            ..AecFrameMetrics::default()
        };
        metrics.update_suppression();
        assert!((metrics.linear_erle_db.unwrap() - 20.0).abs() < 0.01);
        assert!((metrics.effective_echo_suppression_db.unwrap() - 27.9588).abs() < 0.01);
    }

    #[test]
    fn aec_metrics_omit_suppression_below_reference_floor() {
        let mut metrics = AecFrameMetrics {
            reference_rms: AEC_REFERENCE_ACTIVITY_RMS - 0.1,
            mic_pre_aec_rms: 1000.0,
            mic_post_linear_aec_rms: 10.0,
            mic_uplink_rms: 10.0,
            ..AecFrameMetrics::default()
        };
        metrics.update_suppression();
        assert_eq!(metrics.linear_erle_db, None);
        assert_eq!(metrics.effective_echo_suppression_db, None);
    }

    #[test]
    fn from_raw_initializes_empty_aec_metrics() {
        let frame = AudioFrame::mic_only(vec![1, 2], 1, 48_000, 0);
        let p = ProcessedAudioFrame::from_raw(&frame, &[1, 2], &[0, 0]);
        assert_eq!(p.aec_metrics, AecFrameMetrics::default());
    }

    #[test]
    fn untouched_mid_gives_back_the_beam_pair() {
        let l = vec![0.4, 0.2, -0.1, 0.0];
        let r = vec![0.0, 0.2, 0.3, -0.2];
        let mid: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
        let (sl, sr, gain) = rebuild_stereo(&l, &r, &mid, 1.0, 1.0);
        assert!((gain - 1.0).abs() < 1e-6);
        for i in 0..4 {
            assert!(
                (sl[i] - l[i]).abs() < 1e-6 && (sr[i] - r[i]).abs() < 1e-6,
                "sample {i}"
            );
        }
    }

    #[test]
    fn rebuilt_pair_averages_to_the_processed_mid() {
        let l = vec![0.5; 480];
        let r = vec![-0.1; 480];
        let processed = vec![0.05; 480]; // AEC/NS took the mid from 0.2 to 0.05
        let (sl, sr, gain) = rebuild_stereo(&l, &r, &processed, 0.25, 1.0);
        assert!((gain - 0.25).abs() < 1e-6);
        for i in 0..480 {
            assert!((0.5 * (sl[i] + sr[i]) - processed[i]).abs() < 1e-6);
        }
        assert!(
            (sl[0] - sr[0] - 0.6 * 0.25).abs() < 1e-5,
            "side ducked like the mid"
        );
    }

    #[test]
    fn side_gain_ramps_across_the_period() {
        let l = vec![0.5; 100];
        let r = vec![-0.5; 100];
        let processed = vec![0.0; 100];
        // Raw mid is 0: no side at all, whatever came before.
        let (sl, _, gain) = rebuild_stereo(&l, &r, &processed, 1.0, 1.0);
        assert_eq!(gain, 0.0);
        assert!(sl[0] > sl[99], "ramps down from the previous gain");
        assert!(sl[99].abs() < 1e-6);
    }

    #[test]
    fn side_gain_is_the_mid_suppression_ratio() {
        assert_eq!(side_gain(0.0, 0.1), 0.0);
        assert!((side_gain(0.2, 0.05) - 0.25).abs() < 1e-6);
        assert_eq!(side_gain(0.1, 0.5), 1.0, "never boosts");
    }

    #[test]
    fn uplink_sends_the_pair_in_stereo_and_mono_twice_otherwise() {
        let frame = AudioFrame::mic_only(vec![0; 3], 1, 48_000, 0);
        let mono = ProcessedAudioFrame::from_raw(&frame, &[0, 16_384, -16_384], &[0, 0, 0]);
        let mut out = Vec::new();
        mono.uplink_interleaved_i16(&mut out);
        assert_eq!(out, vec![0, 0, 16_383, 16_383, -16_383, -16_383]);
        let stereo = mono.with_stereo(vec![1.5, 0.0, -1.5], vec![-0.5, 0.5, 0.0]);
        stereo.uplink_interleaved_i16(&mut out);
        assert_eq!(out, vec![32_767, -16_383, 0, 16_383, -32_767, 0]);
    }
}
