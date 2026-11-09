//! GET_MAGNETIC_AXIS_RT (0x17), 1024 bytes: per-key Hall configuration
//! (PROTOCOL.md §3.4). 128 records of 8 bytes, indexed by key index.

use serde::{Deserialize, Serialize};

pub const SIZE: usize = 1024;
pub const RECORD_LEN: usize = 8;
pub const RECORDS: usize = 128;

pub const FLAG_WHOLE_TRAVEL: u8 = 0x01;
pub const FLAG_RAMPAGE: u8 = 0x02;
/// Flag bits with unknown meaning that must be preserved verbatim.
pub const FLAG_KEEP_MASK: u8 = 0xFC;

/// Divisors that turn raw u16 fields into millimetres. They depend on
/// device-info byte 29 (`rt_precision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtScale {
    pub trigger: u16,
    pub sensitivity: u16,
}

impl RtScale {
    pub fn from_precision(rt_precision: u8) -> Self {
        RtScale {
            trigger: if rt_precision == 2 { 1000 } else { 100 },
            sensitivity: if rt_precision > 0 { 1000 } else { 100 },
        }
    }
}

/// One raw 8-byte per-key record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisRecord {
    pub axis_type: u8,
    pub flags: u8,
    pub actuation_raw: u16,
    pub press_raw: u16,
    pub release_raw: u16,
}

impl AxisRecord {
    pub fn decode(r: &[u8]) -> Self {
        AxisRecord {
            axis_type: r[0],
            flags: r[1],
            actuation_raw: u16::from_le_bytes([r[2], r[3]]),
            press_raw: u16::from_le_bytes([r[4], r[5]]),
            release_raw: u16::from_le_bytes([r[6], r[7]]),
        }
    }

    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let a = self.actuation_raw.to_le_bytes();
        let p = self.press_raw.to_le_bytes();
        let r = self.release_raw.to_le_bytes();
        [
            self.axis_type,
            self.flags,
            a[0],
            a[1],
            p[0],
            p[1],
            r[0],
            r[1],
        ]
    }

    /// "Whole travel" / continuous RT (UI label "На всём ходе").
    pub fn whole_travel(&self) -> bool {
        self.flags & FLAG_WHOLE_TRAVEL != 0
    }

    pub fn rampage(&self) -> bool {
        self.flags & FLAG_RAMPAGE != 0
    }

    pub fn keep_flags(&self) -> u8 {
        self.flags & FLAG_KEEP_MASK
    }

    /// Rapid Trigger has no enable bit: it is on when either sensitivity is non-zero.
    pub fn rt_enabled(&self) -> bool {
        self.press_raw > 0 || self.release_raw > 0
    }

    pub fn actuation_mm(&self, s: RtScale) -> f64 {
        self.actuation_raw as f64 / s.trigger as f64
    }

    pub fn press_mm(&self, s: RtScale) -> f64 {
        self.press_raw as f64 / s.sensitivity as f64
    }

    pub fn release_mm(&self, s: RtScale) -> f64 {
        self.release_raw as f64 / s.sensitivity as f64
    }
}

/// Decode the whole 1024-byte block.
pub fn decode_block(block: &[u8]) -> Vec<AxisRecord> {
    block
        .chunks_exact(RECORD_LEN)
        .take(RECORDS)
        .map(AxisRecord::decode)
        .collect()
}

/// Encode millimetres the way the vendor does (`round(mm * scale)`).
pub fn mm_to_raw(mm: f64, scale: u16) -> u16 {
    (mm * scale as f64).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two packets of the RT block captured from the old vendor app (FW 1.17,
    // factory defaults): addr 0x000 and addr 0x0E0.
    const FRAME_0: &str = "551738000000000000008c000000000000008c000000000000008c000000000000008c000000000000008c000000000000008c000000000000008c0000000000";
    const FRAME_E0: &str = "551738e00000000000008c000000000000000000000000000000000000000000000000000000000000008c000000000000008c000000000000008c0000000000";

    #[test]
    fn decode_captured_rt_records() {
        let s = RtScale::from_precision(0);
        let f = hex::decode(FRAME_0).unwrap();
        let recs = decode_block(&f[8..64]);
        assert_eq!(recs.len(), 7);
        for r in &recs {
            assert_eq!(r.axis_type, 0);
            assert_eq!(r.flags, 0);
            assert_eq!(r.actuation_raw, 140);
            assert!((r.actuation_mm(s) - 1.40).abs() < 1e-9);
            assert!(!r.rt_enabled());
        }
        // addr 0xE0 = 224 = record 28. Records 29..31 are not physical keys and are zero.
        let f = hex::decode(FRAME_E0).unwrap();
        let recs = decode_block(&f[8..64]);
        assert_eq!(recs[0].actuation_raw, 140); // index 28 "="
        assert_eq!(recs[1].actuation_raw, 0); // 29
        assert_eq!(recs[3].actuation_raw, 0); // 31
        assert_eq!(recs[4].actuation_raw, 140); // 32 Tab
    }

    #[test]
    fn scales_follow_rt_precision() {
        assert_eq!(
            RtScale::from_precision(0),
            RtScale {
                trigger: 100,
                sensitivity: 100
            }
        );
        assert_eq!(
            RtScale::from_precision(1),
            RtScale {
                trigger: 100,
                sensitivity: 1000
            }
        );
        assert_eq!(
            RtScale::from_precision(2),
            RtScale {
                trigger: 1000,
                sensitivity: 1000
            }
        );
        let rec = AxisRecord::decode(&[0, 1, 80, 0, 0x2C, 0x01, 0x2C, 0x01]);
        let s1 = RtScale::from_precision(1);
        assert!((rec.actuation_mm(s1) - 0.80).abs() < 1e-9);
        assert!((rec.press_mm(s1) - 0.30).abs() < 1e-9);
        let rec2 = AxisRecord::decode(&[0, 0, 0x20, 0x03, 0, 0, 0, 0]);
        assert!((rec2.actuation_mm(RtScale::from_precision(2)) - 0.80).abs() < 1e-9);
    }

    #[test]
    fn flags_and_roundtrip() {
        let raw = [0x00, 0x01, 0x50, 0x00, 0x1E, 0x00, 0x1E, 0x00];
        let r = AxisRecord::decode(&raw);
        assert!(r.whole_travel());
        assert!(!r.rampage());
        assert!(r.rt_enabled());
        assert_eq!(r.encode(), raw);
        let r = AxisRecord::decode(&[0x00, 0xFE, 0x96, 0x00, 0, 0, 0, 0]);
        assert!(!r.whole_travel());
        assert!(r.rampage());
        assert_eq!(r.keep_flags(), 0xFC);
        assert_eq!(mm_to_raw(0.8, 100), 80);
        assert_eq!(mm_to_raw(0.3, 1000), 300);
    }
}
