//! GET_KEY (0x12), GET_FN_KEY (0x16), GET_DEFAULT_FN_KEY_MATRIX (0x1C):
//! 512 bytes = 128 records of 4 bytes, indexed by key index (PROTOCOL.md §3.3).

use serde::{Deserialize, Serialize};

use crate::hid_usage::{consumer_usage_name, keyboard_usage_name};

pub const SIZE: usize = 512;
pub const RECORD_LEN: usize = 4;
pub const RECORDS: usize = 128;

/// Decoded keymap record. Opaque types keep their raw bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KeyAction {
    None,
    Mouse { raw: [u8; 4] },
    Keyboard { usage: u8, raw: [u8; 4] },
    Consumer { usage: u16 },
    System { raw: [u8; 4] },
    Extra { raw: [u8; 4] },
    Macro { id: u8 },
    Combo { k1: u8, k2: u8, k3: u8 },
    Dks { slot: u8 },
    ModTap { hold: u8, tap: u8, hold_ms: u16 },
    Toggle { usage: u8 },
    Socd { mode: u8, k1: u8, k2: u8 },
    RappySnappy { k1: u8, k2: u8 },
    Function { id: u8, raw: [u8; 4] },
    Other { raw: [u8; 4] },
}

impl KeyAction {
    pub fn decode(r: [u8; 4]) -> Self {
        let [t, a, b, c] = r;
        match t {
            0 if r == [0; 4] => KeyAction::None,
            1 => KeyAction::Mouse { raw: r },
            2 => KeyAction::Keyboard { usage: b, raw: r },
            3 => KeyAction::Consumer {
                usage: u16::from_le_bytes([a, b]),
            },
            4 => KeyAction::System { raw: r },
            5 => KeyAction::Extra { raw: r },
            6 => KeyAction::Macro { id: a },
            7 => KeyAction::Combo {
                k1: a,
                k2: b,
                k3: c,
            },
            8 => KeyAction::Dks { slot: a },
            9 => KeyAction::ModTap {
                hold: a,
                tap: b,
                hold_ms: c as u16 * 10,
            },
            10 => KeyAction::Toggle { usage: a },
            11 => KeyAction::Socd {
                mode: a,
                k1: b,
                k2: c,
            },
            12 => KeyAction::RappySnappy { k1: b, k2: c },
            13 => KeyAction::Function { id: c, raw: r },
            _ => KeyAction::Other { raw: r },
        }
    }

    /// Short human-readable description.
    pub fn describe(&self) -> String {
        let kb = |u: u8| {
            keyboard_usage_name(u)
                .map(str::to_string)
                .unwrap_or_else(|| format!("0x{u:02X}"))
        };
        let raw = |r: &[u8; 4]| hex::encode(r);
        match self {
            KeyAction::None => "-".into(),
            KeyAction::Keyboard { usage, raw: r } => {
                if r[1] != 0 || r[3] != 0 {
                    format!("{} (raw {})", kb(*usage), raw(r))
                } else if *usage == 0xAF {
                    "Fn".into()
                } else {
                    kb(*usage)
                }
            }
            KeyAction::Consumer { usage } => match consumer_usage_name(*usage) {
                Some(n) => format!("Consumer:{n}"),
                None => format!("Consumer:0x{usage:04X}"),
            },
            KeyAction::Mouse { raw: r } => format!("Mouse({})", raw(r)),
            KeyAction::System { raw: r } => format!("System({})", raw(r)),
            KeyAction::Extra { raw: r } => format!("Extra({})", raw(r)),
            KeyAction::Macro { id } => format!("Macro#{id}"),
            KeyAction::Combo { k1, k2, k3 } => {
                format!("Combo({},{},{})", kb(*k1), kb(*k2), kb(*k3))
            }
            KeyAction::Dks { slot } => format!("DKS#{slot}"),
            KeyAction::ModTap { hold, tap, hold_ms } => {
                format!("MT(hold {} / tap {}, {hold_ms} ms)", kb(*hold), kb(*tap))
            }
            KeyAction::Toggle { usage } => format!("TGL({})", kb(*usage)),
            KeyAction::Socd { mode, k1, k2 } => {
                format!("SOCD(mode {mode}, {},{})", kb(*k1), kb(*k2))
            }
            KeyAction::RappySnappy { k1, k2 } => format!("RS({},{})", kb(*k1), kb(*k2)),
            KeyAction::Function { id, .. } => format!("Func#0x{id:02X}"),
            KeyAction::Other { raw: r } => format!("?({})", raw(r)),
        }
    }
}

/// Split a 512-byte layer into raw records.
pub fn records(block: &[u8]) -> Vec<[u8; 4]> {
    block
        .chunks_exact(RECORD_LEN)
        .take(RECORDS)
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect()
}

pub fn decode_block(block: &[u8]) -> Vec<KeyAction> {
    records(block).into_iter().map(KeyAction::decode).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured packets of GET_KEY (layer 0, addr 0) and GET_FN_KEY (layer 1, addr 0xE0), FW 1.17.
    const KEY_FRAME_0: &str = "55123800000000000200290002003a0002003b0002003c0002003d0002003e0002003f0002004000020041000200420002004300020044000200450000000000";
    const FN_FRAME_E0: &str = "551638e0000000000d00005b0d00001b02003300020034000200310002005c0002005d0002005e000200e10002001d0002001b00020006000200190002000500";

    #[test]
    fn decode_captured_layer0() {
        let f = hex::decode(KEY_FRAME_0).unwrap();
        let recs = decode_block(&f[8..64]);
        assert_eq!(
            recs[0],
            KeyAction::Keyboard {
                usage: 0x29,
                raw: [2, 0, 0x29, 0]
            }
        );
        assert_eq!(recs[0].describe(), "Esc");
        for (i, r) in recs.iter().enumerate().take(13).skip(1) {
            assert_eq!(
                *r,
                KeyAction::Keyboard {
                    usage: 0x39 + i as u8,
                    raw: [2, 0, 0x39 + i as u8, 0]
                }
            );
        }
        assert_eq!(recs[1].describe(), "F1");
        assert_eq!(recs[13], KeyAction::None);
    }

    #[test]
    fn decode_captured_fn_layer() {
        let f = hex::decode(FN_FRAME_E0).unwrap();
        let recs = decode_block(&f[8..64]);
        // index 56 = K, 57 = L on the base layer; on Fn they are vendor functions here
        assert_eq!(
            recs[0],
            KeyAction::Function {
                id: 0x5B,
                raw: [0x0D, 0, 0, 0x5B]
            }
        );
        assert_eq!(recs[1].describe(), "Func#0x1B");
        assert_eq!(recs[2].describe(), ";");
    }

    #[test]
    fn decode_advanced_records() {
        assert_eq!(
            KeyAction::decode([0x08, 3, 0, 0]),
            KeyAction::Dks { slot: 3 }
        );
        assert_eq!(
            KeyAction::decode([0x09, 0xE1, 0x04, 40]),
            KeyAction::ModTap {
                hold: 0xE1,
                tap: 4,
                hold_ms: 400
            }
        );
        assert_eq!(
            KeyAction::decode([0x0B, 1, 4, 7]),
            KeyAction::Socd {
                mode: 1,
                k1: 4,
                k2: 7
            }
        );
        assert_eq!(
            KeyAction::decode([0x03, 0x92, 0x01, 0]).describe(),
            "Consumer:Calculator"
        );
        assert_eq!(KeyAction::decode([0x02, 0, 0xAF, 0]).describe(), "Fn");
        assert_eq!(
            KeyAction::decode([0x00, 1, 0, 0]),
            KeyAction::Other { raw: [0, 1, 0, 0] }
        );
    }
}
