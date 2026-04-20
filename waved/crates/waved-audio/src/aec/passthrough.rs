use super::EchoCanceller;

pub struct PassthroughCanceller {
    pub frame_size: usize,
}

impl EchoCanceller for PassthroughCanceller {
    fn process(&mut self, mic: &[i16], _reference: &[i16], out: &mut [i16]) {
        assert_eq!(mic.len(), self.frame_size, "mic frame size mismatch");
        assert_eq!(out.len(), self.frame_size, "out frame size mismatch");
        out.copy_from_slice(mic);
    }

    fn frame_size(&self) -> usize {
        self.frame_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_copies_mic_to_out_ignoring_reference() {
        let mut c = PassthroughCanceller { frame_size: 8 };
        let mic: [i16; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        let reference: [i16; 8] = [100, 100, 100, 100, 100, 100, 100, 100];
        let mut out = [0i16; 8];
        c.process(&mic, &reference, &mut out);
        assert_eq!(out, mic);
    }

    #[test]
    fn passthrough_reports_frame_size() {
        let c = PassthroughCanceller { frame_size: 480 };
        assert_eq!(c.frame_size(), 480);
    }
}
