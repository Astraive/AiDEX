//! The Appearance interface scale, applied as native webview zoom.
//!
//! The scale itself is saved in the drive's vault (`appearance.set`); the
//! `set_ui_scale` command only applies it to the window, so nothing about it is
//! kept on the host. Native zoom scales every px value and moves media and
//! container queries to the zoomed CSS viewport, which CSS `zoom` would not.

use crate::error::BridgeError;
use aidex_core::ServiceErrorCode;

/// Smallest interface scale in percent (`aidex-preferences::MIN_UI_SCALE`).
pub const MIN_UI_SCALE: u16 = 75;
/// Largest interface scale in percent (`aidex-preferences::MAX_UI_SCALE`).
pub const MAX_UI_SCALE: u16 = 125;

/// The webview zoom factor for `percent`, which must be 75-125.
pub fn zoom_factor(percent: u16) -> Result<f64, BridgeError> {
    if (MIN_UI_SCALE..=MAX_UI_SCALE).contains(&percent) {
        Ok(f64::from(percent) / 100.0)
    } else {
        Err(BridgeError::new(
            ServiceErrorCode::InvalidArgument,
            "invalid_params",
            "the interface scale must be 75-125 percent",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_scale_is_a_bounded_percentage() {
        assert_eq!(zoom_factor(100).unwrap(), 1.0);
        assert_eq!(zoom_factor(75).unwrap(), 0.75);
        assert_eq!(zoom_factor(125).unwrap(), 1.25);
        for refused in [0, 74, 126, 200, u16::MAX] {
            let error = zoom_factor(refused).unwrap_err();
            assert_eq!(error.code, "invalid_argument", "{refused}");
            assert_eq!(error.kind, "invalid_params", "{refused}");
        }
    }
}
