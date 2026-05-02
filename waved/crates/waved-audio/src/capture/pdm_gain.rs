//! PDM Gain hardware preamp init — one-shot ALSA mixer write at daemon
//! startup so the ADC sees a consistent level forever. See the
//! mic-volume-pipeline spec for why we don't move this dynamically.

use alsa::ctl::{Ctl, ElemId, ElemIface, ElemType, ElemValue};
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum PdmGainError {
    #[error("alsa: {0}")]
    Alsa(#[from] alsa::Error),
    #[error("control numid={0} not found on card {1}")]
    NotFound(u32, String),
}

/// Set the PDM Gain mixer control on `card` (e.g. `"hw:0"`) to `value`
/// (validated 0..=48 by the caller). Logs and returns the error rather
/// than panicking — capture can still run with whatever gain was
/// previously set.
pub fn init(card: &str, value: u8) -> Result<(), PdmGainError> {
    const PDM_GAIN_NUMID: u32 = 9;

    let ctl = Ctl::new(card, false)?;
    let mut id = ElemId::new(ElemIface::Mixer);
    id.set_numid(PDM_GAIN_NUMID);

    // ElemValue::new takes the type and allocates with the max count for
    // that type (128 for Integer), so set_integer(0, …) is always in-bounds.
    let mut elem = ElemValue::new(ElemType::Integer)?;
    elem.set_id(&id);
    elem.set_integer(0, value as i32)
        .ok_or(PdmGainError::NotFound(PDM_GAIN_NUMID, card.to_string()))?;

    // elem_write takes &ElemValue (not &mut) in alsa 0.9.x.
    ctl.elem_write(&elem)?;
    info!("pdm_gain: set numid=9 on {card} → {value}");
    Ok(())
}

/// Convenience: log a warning on failure and continue. Capture is still
/// viable with the prior mixer value.
pub fn init_or_warn(card: &str, value: u8) {
    if let Err(e) = init(card, value) {
        warn!("pdm_gain init on {card} failed: {e}; capture continues at prior gain");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_or_warn_does_not_panic_on_missing_card() {
        // No ALSA card by this name on the host — function must handle
        // the error and return without panicking.
        init_or_warn("hw:99", 24);
    }

    #[test]
    fn init_returns_error_on_missing_card() {
        let result = init("hw:99", 24);
        assert!(result.is_err(), "expected error on missing card, got {result:?}");
    }
}
