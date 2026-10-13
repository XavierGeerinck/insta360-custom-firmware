//! Keeps a failure that repeats on every retry from filling the log. Used by
//! the ALSA bridges (`uac::bridge`, `bt_audio`); kept out of the ALSA-gated
//! modules so it is tested on every host.
//!
//! While the host is attached but not playing, `f_uac2` fails each read with
//! EIO, so the USB bridge retries every second for as long as the device
//! stays idle; the Bluetooth paths retry the same way while `hw:0,0` refuses.

#[derive(Debug, Default)]
pub struct RepeatFilter {
    last: Option<String>,
}

impl RepeatFilter {
    /// Whether `failure` differs from the one before it.
    pub fn is_new(&mut self, failure: &str) -> bool {
        if self.last.as_deref() == Some(failure) {
            return false;
        }
        self.last = Some(failure.to_owned());
        true
    }

    /// Forget the last failure; true if there was one.
    pub fn clear(&mut self) -> bool {
        self.last.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::RepeatFilter;

    #[test]
    fn a_failure_that_repeats_is_reported_once_until_audio_flows_again() {
        let mut failures = RepeatFilter::default();
        assert!(failures.is_new("drain stopped: EIO"));
        assert!(!failures.is_new("drain stopped: EIO"));
        assert!(!failures.is_new("drain stopped: EIO"));
        assert!(failures.is_new("open failed: ENODEV"));
        assert!(failures.clear());
        assert!(!failures.clear());
        assert!(failures.is_new("drain stopped: EIO"));
    }
}
