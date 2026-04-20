//! DSP boundary frame: mic signal after AEC + NS in float, paired with the
//! raw 8-ch mic the meeting recorder / stats still want.
//!
//! See spec §4 for why we carry both the processed mono and the raw 8-ch
//! buffer: stats want per-channel RMS on raw mic, meeting replay wants raw
//! for future reprocessing, and the uplink wants the processed mono.

use crate::callbacks::AudioFrame;

#[derive(Debug, Clone)]
pub struct ProcessedAudioFrame {
    /// Post-AEC, post-NS mono signal in [-1.0, 1.0].
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
            assert!(diff <= 1, "i16 {} roundtripped to {} (diff {})", orig, rt, diff);
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
}
