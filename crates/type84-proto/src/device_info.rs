//! GET_DEVICE_INFO (0x10), 48 bytes (PROTOCOL.md §3.1).

use serde::{Deserialize, Serialize};

pub const SIZE: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub rom_size: u8,
    /// Raw u16 at payload 2..3. Ignored by the vendor hub for this board,
    /// which assumes a 512-byte macro area.
    pub macro_space_size_raw: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Firmware version in hundredths (117 = 1.17).
    pub version_hundredths: u16,
    pub sensor: u16,
    pub manufacturer: u16,
    pub product: u16,
    pub work_mode: u8,
    pub battery_level: u8,
    pub charge_status: u8,
    pub current_profile: u8,
    pub axis_info: u16,
    pub tft_max_frames: u16,
    pub gif_max_frames: u16,
    pub led_max_frames: u16,
    pub tft_direction: u8,
    /// Selects RT units (see [`crate::rt::RtScale`]).
    pub rt_precision: u8,
    pub frame_version: u8,
    pub lighting_version: u8,
    /// 0 means operational; anything else means the firmware is busy.
    pub firmware_status: u8,
}

#[derive(Debug, thiserror::Error)]
#[error("device info payload too short: {0} bytes, need {SIZE}")]
pub struct ShortDeviceInfo(pub usize);

impl DeviceInfo {
    pub fn decode(p: &[u8]) -> Result<Self, ShortDeviceInfo> {
        if p.len() < SIZE {
            return Err(ShortDeviceInfo(p.len()));
        }
        let u16le = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        let b8 = p[8] as u16;
        let b9 = p[9] as u16;
        Ok(DeviceInfo {
            rom_size: p[0],
            macro_space_size_raw: u16le(2),
            vendor_id: u16le(4),
            product_id: u16le(6),
            // The version is BCD-ish: low byte holds two decimal digits as
            // nibbles, high byte counts hundreds.
            version_hundredths: (b8 & 0x0F) + (b8 >> 4) * 10 + b9 * 100,
            sensor: u16le(10),
            manufacturer: u16le(12),
            product: u16le(14),
            work_mode: p[16],
            battery_level: p[17],
            charge_status: p[18],
            current_profile: p[19],
            axis_info: u16le(20),
            tft_max_frames: u16le(22),
            gif_max_frames: u16le(24),
            led_max_frames: u16le(26),
            tft_direction: p[28],
            rt_precision: p[29],
            frame_version: p[30],
            lighting_version: p[31],
            firmware_status: p[32],
        })
    }

    pub fn version_string(&self) -> String {
        format!(
            "{}.{:02}",
            self.version_hundredths / 100,
            self.version_hundredths % 100
        )
    }

    pub fn is_magnetic(&self) -> bool {
        self.axis_info > 0
    }

    pub fn is_operational(&self) -> bool {
        self.firmware_status == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Response to GET_DEVICE_INFO captured from the old vendor app (FW 1.17).
    pub const CAPTURED_FRAME: &str = "551030000000010000000092450cd6801701000066010c110a0000000f0000000000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn decode_captured_device_info() {
        let f = hex::decode(CAPTURED_FRAME).unwrap();
        let info = DeviceInfo::decode(&f[8..8 + SIZE]).unwrap();
        assert_eq!(info.vendor_id, 0x0C45);
        assert_eq!(info.product_id, 0x80D6);
        assert_eq!(info.version_string(), "1.17");
        assert_eq!(info.manufacturer, 0x0166);
        assert_eq!(info.product, 0x110C);
        assert_eq!(info.work_mode, 0x0A);
        assert_eq!(info.axis_info, 15);
        assert!(info.is_magnetic());
        assert_eq!(info.rt_precision, 0);
        assert!(info.is_operational());
        assert_eq!(info.macro_space_size_raw, 0x9200);
    }

    #[test]
    fn short_payload_rejected() {
        assert!(DeviceInfo::decode(&[0u8; 47]).is_err());
    }
}
