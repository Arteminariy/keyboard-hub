//! GET_GAME_MODE (0x11), 56 bytes: keyboard-wide settings (PROTOCOL.md §3.2).
//!
//! The raw block is kept verbatim; unknown bytes must be preserved for a
//! future read-modify-write.

use serde::{Deserialize, Serialize};

pub const SIZE: usize = 56;

pub const GAME_MODE: usize = 1;
pub const FN_SWITCH: usize = 2;
pub const SLEEP_TIME: usize = 3;
pub const KEY_DELAY: usize = 4;
pub const REPORT_RATE: usize = 5;
pub const SYSTEM_MODE: usize = 6;
pub const TFT_DISPLAY_TIME: usize = 7;
pub const TOP_DEAD_ZONE: usize = 8;
pub const BOTTOM_DEAD_ZONE: usize = 9;
pub const STABILITY_MODE: usize = 11;
pub const AUTO_CALIBRATION: usize = 14;
pub const SINGLE_KEY_WAKEUP: usize = 15;
pub const PUSH_BUTTON_MODE: usize = 16;
pub const NKRO_SWITCH: usize = 17;
pub const WIRELESS_REPORT_RATE: usize = 18;

/// Report-rate code to Hz.
pub fn report_rate_hz(code: u8) -> Option<u32> {
    Some(match code {
        0 => 125,
        1 => 250,
        2 => 500,
        3 => 1000,
        4 => 2000,
        5 => 4000,
        6 => 8000,
        _ => return None,
    })
}

/// Decoded view of the settings block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub game_mode: u8,
    pub fn_switch: u8,
    pub sleep_time_min: u8,
    pub key_delay: u8,
    pub report_rate_code: u8,
    pub report_rate_hz: Option<u32>,
    pub system_mode: u8,
    pub tft_display_time: u8,
    /// Hundredths of a millimetre.
    pub top_dead_zone: u8,
    /// Hundredths of a millimetre.
    pub bottom_dead_zone: u8,
    pub stability_mode: u8,
    pub auto_calibration: u8,
    pub single_key_wakeup: u8,
    pub push_button_mode: u8,
    pub nkro_switch: u8,
    pub wireless_report_rate_hz: u16,
}

#[derive(Debug, thiserror::Error)]
#[error("settings payload too short: {0} bytes, need {SIZE}")]
pub struct ShortSettings(pub usize);

impl Settings {
    pub fn decode(p: &[u8]) -> Result<Self, ShortSettings> {
        if p.len() < SIZE {
            return Err(ShortSettings(p.len()));
        }
        Ok(Settings {
            game_mode: p[GAME_MODE],
            fn_switch: p[FN_SWITCH],
            sleep_time_min: p[SLEEP_TIME],
            key_delay: p[KEY_DELAY],
            report_rate_code: p[REPORT_RATE],
            report_rate_hz: report_rate_hz(p[REPORT_RATE]),
            system_mode: p[SYSTEM_MODE],
            tft_display_time: p[TFT_DISPLAY_TIME],
            top_dead_zone: p[TOP_DEAD_ZONE],
            bottom_dead_zone: p[BOTTOM_DEAD_ZONE],
            stability_mode: p[STABILITY_MODE],
            auto_calibration: p[AUTO_CALIBRATION],
            single_key_wakeup: p[SINGLE_KEY_WAKEUP],
            push_button_mode: p[PUSH_BUTTON_MODE],
            nkro_switch: p[NKRO_SWITCH],
            wireless_report_rate_hz: u16::from_le_bytes([
                p[WIRELESS_REPORT_RATE],
                p[WIRELESS_REPORT_RATE + 1],
            ]),
        })
    }

    pub fn top_dead_zone_mm(&self) -> f64 {
        self.top_dead_zone as f64 / 100.0
    }

    pub fn bottom_dead_zone_mm(&self) -> f64 {
        self.bottom_dead_zone as f64 / 100.0
    }
}

/// Byte offsets of the settings block that the protocol does not model.
pub fn unknown_offsets() -> impl Iterator<Item = usize> {
    const KNOWN: [usize; 16] = [
        GAME_MODE,
        FN_SWITCH,
        SLEEP_TIME,
        KEY_DELAY,
        REPORT_RATE,
        SYSTEM_MODE,
        TFT_DISPLAY_TIME,
        TOP_DEAD_ZONE,
        BOTTOM_DEAD_ZONE,
        STABILITY_MODE,
        AUTO_CALIBRATION,
        SINGLE_KEY_WAKEUP,
        PUSH_BUTTON_MODE,
        NKRO_SWITCH,
        WIRELESS_REPORT_RATE,
        WIRELESS_REPORT_RATE + 1,
    ];
    (0..SIZE).filter(|i| !KNOWN.contains(i))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURED_FRAME: &str = "55113800000001000000000100060000000000010000010000000000000000000000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn decode_captured_settings() {
        let f = hex::decode(CAPTURED_FRAME).unwrap();
        let s = Settings::decode(&f[8..8 + SIZE]).unwrap();
        assert_eq!(s.sleep_time_min, 1);
        assert_eq!(s.report_rate_code, 6);
        assert_eq!(s.report_rate_hz, Some(8000));
        assert_eq!(s.stability_mode, 1);
        assert_eq!(s.auto_calibration, 1);
        assert_eq!(s.top_dead_zone, 0);
        assert_eq!(s.bottom_dead_zone, 0);
    }

    #[test]
    fn rate_codes() {
        assert_eq!(report_rate_hz(3), Some(1000));
        assert_eq!(report_rate_hz(5), Some(4000));
        assert_eq!(report_rate_hz(7), None);
    }
}
