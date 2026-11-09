//! Fn-layer write encoding: SET_FN_KEY (0x26), 512 bytes (PROTOCOL.md §3.3
//! record layout, §4.2 write procedure).
//!
//! Hardware status: IMPLEMENTED, UNVERIFIED on hardware. The record
//! encodings follow the vendor hub (`$e` encoder, `Rn` display decoder).
//!
//! Only layer 1 (the Fn layer) is writable. Layer 0 uses a different opcode
//! (SET_KEY, 0x22) that is not on the write allowlist.
//!
//! The action vocabulary is deliberately small ([`FnAction`]):
//! - `key:<name>` with optional modifiers, e.g. `key:F13`, `key:Ctrl+F10`:
//!   record `02 mods usage 00`. The modifier byte is the HID modifier
//!   bitmask (bit0 LCtrl, bit1 LShift, bit2 LAlt, bit3 LGui, bit4 RCtrl,
//!   bit5 RShift, bit6 RAlt, bit7 RGui); the hub renders a non-zero byte 1
//!   exactly like that ("Ctrl+F10"), but never writes one itself, so
//!   whether the firmware honours the modifiers is UNVERIFIED.
//! - `consumer:<name>`, e.g. `consumer:PlayPause`: record `03 lo hi 00`.
//! - `none`: `00 00 00 00` (unassigned).
//! - `factory`: the record from GET_DEFAULT_FN_KEY_MATRIX (0x1C).
//!
//! Policy for a typed patch:
//! - only the 84 physical keys, never the Fn key itself (index 85);
//! - a key may be changed only while it holds `none`, a keyboard record or
//!   a consumer record; vendor functions (FUNC, e.g. Fn+Delete = reset,
//!   Fn+Win = Win lock), macros, DKS and advanced keys are never
//!   overwritten (except by `factory`, which copies the factory record);
//! - every other record, physical or not, stays exactly as read.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hid_usage::{
    consumer_usage_by_name, consumer_usage_name, keyboard_usage_by_name, keyboard_usage_name,
};
use crate::keymap::{RECORDS, RECORD_LEN, SIZE};
use crate::layout;

/// Index of the Fn key on both layers.
pub const FN_KEY_INDEX: usize = 85;
/// The Fn key record (`02 00 AF 00`).
pub const FN_KEY_RECORD: [u8; 4] = [0x02, 0x00, 0xAF, 0x00];
/// The hub refuses keymap writes for index >= 126 (PROTOCOL.md §3.9).
pub const MAX_WRITABLE_INDEX: usize = 125;

const TYPE_NONE: u8 = 0;
const TYPE_KEYBOARD: u8 = 2;
const TYPE_CONSUMER: u8 = 3;

/// HID modifier bits, in report order.
pub const MODIFIER_NAMES: [&str; 8] = [
    "Ctrl", "Shift", "Alt", "Win", "RCtrl", "RShift", "RAlt", "RWin",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FnLayerError {
    #[error("key layer must be {SIZE} bytes, got {0}")]
    Size(usize),
    #[error("invalid action {0:?}: {1}")]
    Parse(String, String),
    #[error("key index {0} is not one of the 84 physical keys")]
    NotPhysical(usize),
    #[error("the Fn key (index 85) is never remapped")]
    FnKey,
    #[error("key index {0} is listed more than once")]
    Duplicate(usize),
    #[error("{label} (index {index}) holds {current} on the Fn layer; only unassigned, keyboard or consumer records may be replaced (use `factory` to restore it)")]
    Protected {
        index: usize,
        label: String,
        current: String,
    },
    #[error("the Fn key record at index 85 must stay {:02x?}", FN_KEY_RECORD)]
    FnKeyChanged,
    #[error("record {0} (>= 126) differs; the hub never writes those indices")]
    HighIndexChanged(usize),
}

/// One Fn-layer action of a preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnAction {
    /// Unassigned (`00 00 00 00`).
    None,
    /// The factory record of this key (GET_DEFAULT_FN_KEY_MATRIX).
    Factory,
    /// Keyboard usage with a modifier bitmask (`02 mods usage 00`).
    Key { mods: u8, usage: u8 },
    /// Consumer usage (`03 lo hi 00`).
    Consumer(u16),
}

impl FnAction {
    /// Parse `none`, `factory`, `key:[Mod+...]Name` or `consumer:Name`.
    pub fn parse(s: &str) -> Result<Self, FnLayerError> {
        let err = |m: &str| FnLayerError::Parse(s.to_string(), m.to_string());
        let t = s.trim();
        match t.to_ascii_lowercase().as_str() {
            "none" | "unassigned" => return Ok(FnAction::None),
            "factory" | "default" => return Ok(FnAction::Factory),
            _ => {}
        }
        let (kind, rest) = t
            .split_once(':')
            .ok_or_else(|| err("expected none, factory, key:<name> or consumer:<name>"))?;
        match kind.trim().to_ascii_lowercase().as_str() {
            "key" => {
                let parts: Vec<&str> = rest.split('+').map(str::trim).collect();
                let (key, mods) = parts.split_last().ok_or_else(|| err("empty key"))?;
                let mut m = 0u8;
                for name in mods {
                    let bit = modifier_bit(name)
                        .ok_or_else(|| err(&format!("unknown modifier {name:?}")))?;
                    m |= bit;
                }
                let usage = keyboard_usage_by_name(key)
                    .ok_or_else(|| err(&format!("unknown key {key:?}")))?;
                if usage == 0 {
                    return Err(err("usage 0 is not a key; use none"));
                }
                if usage == 0xAF {
                    return Err(err("0xAF is the Fn key itself"));
                }
                Ok(FnAction::Key { mods: m, usage })
            }
            "consumer" => {
                let usage = consumer_usage_by_name(rest.trim())
                    .ok_or_else(|| err(&format!("unknown consumer usage {:?}", rest.trim())))?;
                if usage == 0 || usage > 0x033C {
                    // The board's consumer report covers usages 0..0x33C.
                    return Err(err("consumer usage must be 0x0001..0x033C"));
                }
                Ok(FnAction::Consumer(usage))
            }
            _ => Err(err("expected none, factory, key:<name> or consumer:<name>")),
        }
    }

    /// The 4-byte record, given the factory record of the same key.
    pub fn record(&self, factory: [u8; 4]) -> [u8; 4] {
        match *self {
            FnAction::None => [0; 4],
            FnAction::Factory => factory,
            FnAction::Key { mods, usage } => [TYPE_KEYBOARD, mods, usage, 0],
            FnAction::Consumer(u) => [TYPE_CONSUMER, (u & 0xFF) as u8, (u >> 8) as u8, 0],
        }
    }

    /// The canonical action for a record, if it is expressible.
    pub fn from_record(r: [u8; 4]) -> Option<Self> {
        match r {
            [0, 0, 0, 0] => Some(FnAction::None),
            [TYPE_KEYBOARD, mods, usage, 0] if usage != 0 => Some(FnAction::Key { mods, usage }),
            [TYPE_CONSUMER, lo, hi, 0] => Some(FnAction::Consumer(u16::from_le_bytes([lo, hi]))),
            _ => None,
        }
    }
}

impl fmt::Display for FnAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            FnAction::None => write!(f, "none"),
            FnAction::Factory => write!(f, "factory"),
            FnAction::Key { mods, usage } => {
                write!(f, "key:")?;
                for (i, n) in MODIFIER_NAMES.iter().enumerate() {
                    if mods & (1 << i) != 0 {
                        write!(f, "{n}+")?;
                    }
                }
                match keyboard_usage_name(usage) {
                    Some(n) => write!(f, "{n}"),
                    None => write!(f, "0x{usage:02X}"),
                }
            }
            FnAction::Consumer(u) => match consumer_usage_name(u) {
                Some(n) => write!(f, "consumer:{n}"),
                None => write!(f, "consumer:0x{u:04X}"),
            },
        }
    }
}

impl Serialize for FnAction {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for FnAction {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        FnAction::parse(&s).map_err(serde::de::Error::custom)
    }
}

fn modifier_bit(name: &str) -> Option<u8> {
    Some(match name.to_ascii_lowercase().as_str() {
        "ctrl" | "lctrl" | "control" => 0x01,
        "shift" | "lshift" => 0x02,
        "alt" | "lalt" => 0x04,
        "win" | "lwin" | "gui" | "lgui" | "meta" | "super" => 0x08,
        "rctrl" => 0x10,
        "rshift" => 0x20,
        "ralt" | "altgr" => 0x40,
        "rwin" | "rgui" => 0x80,
        _ => return None,
    })
}

/// Human-readable description of any Fn-layer record.
pub fn describe_record(r: [u8; 4]) -> String {
    match FnAction::from_record(r) {
        Some(a) => a.to_string(),
        None => crate::keymap::KeyAction::decode(r).describe(),
    }
}

fn record(block: &[u8], i: usize) -> [u8; 4] {
    let o = i * RECORD_LEN;
    [block[o], block[o + 1], block[o + 2], block[o + 3]]
}

/// Whether a typed patch may replace this record: unassigned, a keyboard
/// record or a consumer record. Everything else (vendor FUNC, macro, DKS,
/// advanced keys) is protected.
pub fn replaceable(r: [u8; 4]) -> bool {
    matches!(r[0], TYPE_NONE | TYPE_KEYBOARD | TYPE_CONSUMER) && (r[0] != TYPE_NONE || r == [0; 4])
}

/// Merge a typed patch into the Fn layer read from the board.
/// `factory` is the GET_DEFAULT_FN_KEY_MATRIX block.
pub fn apply_patch(
    base: &[u8],
    factory: &[u8],
    patch: &[(u8, FnAction)],
) -> Result<Vec<u8>, FnLayerError> {
    for b in [base, factory] {
        if b.len() != SIZE {
            return Err(FnLayerError::Size(b.len()));
        }
    }
    let mut out = base.to_vec();
    let mut seen = std::collections::BTreeSet::new();
    for &(index, action) in patch {
        let i = index as usize;
        let Some(label) = layout::label(i) else {
            return Err(FnLayerError::NotPhysical(i));
        };
        if i == FN_KEY_INDEX {
            return Err(FnLayerError::FnKey);
        }
        if i > MAX_WRITABLE_INDEX {
            return Err(FnLayerError::NotPhysical(i));
        }
        if !seen.insert(i) {
            return Err(FnLayerError::Duplicate(i));
        }
        let cur = record(base, i);
        let new = action.record(record(factory, i));
        if new != cur && action != FnAction::Factory && !replaceable(cur) {
            return Err(FnLayerError::Protected {
                index: i,
                label: label.to_string(),
                current: describe_record(cur),
            });
        }
        out[i * RECORD_LEN..(i + 1) * RECORD_LEN].copy_from_slice(&new);
    }
    validate_write(base, &out)?;
    Ok(out)
}

/// Checks every Fn-layer write (typed patch or raw restore) must pass:
/// the Fn key record stays in place and records >= 126 are unchanged.
pub fn validate_write(base: &[u8], new: &[u8]) -> Result<(), FnLayerError> {
    for b in [base, new] {
        if b.len() != SIZE {
            return Err(FnLayerError::Size(b.len()));
        }
    }
    if record(new, FN_KEY_INDEX) != FN_KEY_RECORD {
        return Err(FnLayerError::FnKeyChanged);
    }
    for i in MAX_WRITABLE_INDEX + 1..RECORDS {
        if record(base, i) != record(new, i) {
            return Err(FnLayerError::HighIndexChanged(i));
        }
    }
    Ok(())
}

/// Indices whose record differs.
pub fn changed_indices(old: &[u8], new: &[u8]) -> Vec<usize> {
    (0..RECORDS)
        .filter(|&i| old.get(i * 4..i * 4 + 4) != new.get(i * 4..i * 4 + 4))
        .collect()
}

/// `label: old → new` lines for every changed record (physical keys by
/// label, other indices by number).
pub fn diff_lines(old: &[u8], new: &[u8]) -> Vec<String> {
    changed_indices(old, new)
        .into_iter()
        .map(|i| {
            let name = layout::label(i)
                .map(|l| format!("Fn+{l}"))
                .unwrap_or_else(|| format!("index {i}"));
            format!(
                "  {:<14} {} → {}",
                format!("{name}:"),
                describe_record(record(old, i)),
                describe_record(record(new, i))
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/type84-20261109-220355-fw1.17.json");

    fn blocks() -> (Vec<u8>, Vec<u8>) {
        let v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        let b = |n: &str| hex::decode(v["blocks"][n]["data"].as_str().unwrap()).unwrap();
        (b("keymap_layer1"), b("default_fn_keymap"))
    }

    #[test]
    fn parse_and_format_round_trip() {
        for (s, rec) in [
            ("key:F13", [2, 0, 0x68, 0]),
            ("key:Ctrl+F10", [2, 0x01, 0x43, 0]),
            ("key:Shift+PrintScreen", [2, 0x02, 0x46, 0]),
            ("key:Ctrl+Shift+Alt+Win+F1", [2, 0x0F, 0x3A, 0]),
            ("consumer:WWWHome", [3, 0x23, 0x02, 0]),
            ("consumer:Browser", [3, 0x96, 0x01, 0]),
            ("consumer:PlayPause", [3, 0xCD, 0x00, 0]),
            ("none", [0, 0, 0, 0]),
        ] {
            let a = FnAction::parse(s).unwrap();
            assert_eq!(a.record([9; 4]), rec, "{s}");
            assert_eq!(a.to_string(), s);
            assert_eq!(FnAction::from_record(rec), Some(a));
        }
        assert_eq!(
            FnAction::parse("factory").unwrap().record([1, 2, 3, 4]),
            [1, 2, 3, 4]
        );
        assert_eq!(
            FnAction::parse("key:print").unwrap().to_string(),
            "key:PrintScreen"
        );
        assert_eq!(
            FnAction::parse("consumer:0x0223").unwrap(),
            FnAction::Consumer(0x223)
        );
        for bad in [
            "",
            "key:",
            "key:Hyper+F1",
            "key:nope",
            "key:0xAF",
            "key:0x00",
            "macro:1",
            "consumer:0x0000",
            "consumer:0x0400",
            "F13",
        ] {
            assert!(FnAction::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn patch_touches_only_listed_keys() {
        let (base, factory) = blocks();
        let patch = vec![
            (1, FnAction::parse("consumer:WWWHome").unwrap()),
            (2, FnAction::parse("key:Ctrl+F10").unwrap()),
            (17, FnAction::parse("key:F13").unwrap()),
        ];
        let new = apply_patch(&base, &factory, &patch).unwrap();
        assert_eq!(changed_indices(&base, &new), vec![1, 2, 17]);
        assert_eq!(&new[4..8], &[3, 0x23, 0x02, 0]);
        assert_eq!(&new[8..12], &[2, 1, 0x43, 0]);
        assert_eq!(&new[68..72], &[2, 0, 0x68, 0]);
        let lines = diff_lines(&base, &new);
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].contains("Fn+F1:") && lines[0].contains("none → consumer:WWWHome"),
            "{lines:?}"
        );
        assert!(
            lines[2].contains("Fn+1:") && lines[2].contains("key:1 → key:F13"),
            "{lines:?}"
        );
        // `factory` restores the original block.
        let back: Vec<(u8, FnAction)> =
            patch.iter().map(|&(i, _)| (i, FnAction::Factory)).collect();
        assert_eq!(apply_patch(&new, &factory, &back).unwrap(), factory);
    }

    #[test]
    fn protected_records_are_refused() {
        let (base, factory) = blocks();
        let k = |s: &str| FnAction::parse(s).unwrap();
        // Fn key itself.
        assert_eq!(
            apply_patch(&base, &factory, &[(85, k("key:F1"))]),
            Err(FnLayerError::FnKey)
        );
        // Non-physical index.
        assert!(matches!(
            apply_patch(&base, &factory, &[(29, k("key:F1"))]),
            Err(FnLayerError::NotPhysical(29))
        ));
        // Vendor functions: Fn+Delete (reset, FUNC 1), Fn+Win (Win lock).
        for i in [106u8, 81] {
            assert!(
                matches!(
                    apply_patch(&base, &factory, &[(i, k("key:F1"))]),
                    Err(FnLayerError::Protected { .. })
                ),
                "index {i}"
            );
            // `factory` on a FUNC key is a no-op and allowed.
            assert_eq!(
                apply_patch(&base, &factory, &[(i, FnAction::Factory)]).unwrap(),
                base
            );
        }
        // Duplicates.
        assert!(matches!(
            apply_patch(&base, &factory, &[(1, k("key:F1")), (1, k("key:F2"))]),
            Err(FnLayerError::Duplicate(1))
        ));
        // Raw writes that drop the Fn key are refused.
        let mut bad = base.clone();
        bad[85 * 4..86 * 4].copy_from_slice(&[0; 4]);
        assert_eq!(validate_write(&base, &bad), Err(FnLayerError::FnKeyChanged));
        let mut bad = base.clone();
        bad[127 * 4] = 2;
        assert_eq!(
            validate_write(&base, &bad),
            Err(FnLayerError::HighIndexChanged(127))
        );
    }
}
