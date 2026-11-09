//! GET_MAGNETIC_AXIS_DKS_DATA (0x18), 1024 bytes: 64 Dynamic Keystroke slots
//! of 16 bytes (PROTOCOL.md §3.8).

use serde::{Deserialize, Serialize};

pub const SIZE: usize = 1024;
pub const SLOT_LEN: usize = 16;
pub const SLOTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DksSlot {
    /// Press points 1 and 2, in 0.1 mm.
    pub make: [u8; 2],
    /// Release points 1 and 2, in 0.1 mm.
    pub brk: [u8; 2],
    /// Reserved bytes 4, 6, 8, 10 (preserved).
    pub reserved: [u8; 4],
    /// HID keyboard usages of actions 1..4 (bytes 5, 7, 9, 11).
    pub actions: [u8; 4],
    /// Per-phase state bytes 12..15: bit a = action a tap, bit a+4 = hold.
    pub phases: [u8; 4],
}

impl DksSlot {
    pub fn decode(b: &[u8]) -> Self {
        DksSlot {
            make: [b[0], b[1]],
            brk: [b[2], b[3]],
            reserved: [b[4], b[6], b[8], b[10]],
            actions: [b[5], b[7], b[9], b[11]],
            phases: [b[12], b[13], b[14], b[15]],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.make == [0, 0] && self.brk == [0, 0]
    }
}

pub fn decode_block(block: &[u8]) -> Vec<DksSlot> {
    block
        .chunks_exact(SLOT_LEN)
        .take(SLOTS)
        .map(DksSlot::decode)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_slot() {
        let raw = [10, 36, 36, 10, 0, 0x04, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0];
        let s = DksSlot::decode(&raw);
        assert_eq!(s.make, [10, 36]);
        assert_eq!(s.brk, [36, 10]);
        assert_eq!(s.actions, [4, 0, 0, 0]);
        assert_eq!(s.phases[0], 0x10);
        assert!(!s.is_empty());
        assert!(DksSlot::decode(&[0u8; 16]).is_empty());
        assert_eq!(decode_block(&[0u8; SIZE]).len(), SLOTS);
    }
}
