//! Physical layout: key index ↔ label (PROTOCOL.md §3.9).
//!
//! Parsed from the KLE layout in `docs/type84_layout.kle.json`. Each key
//! legend starts with `"r,c"`; the second number is the key index that
//! addresses keymap, RT and LED records.

use std::sync::OnceLock;

const KLE_JSON: &str = include_str!("../../../docs/type84_layout.kle.json");

/// One physical key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub index: u8,
    pub label: String,
    /// Visual row in the KLE layout (0 = F-row).
    pub row: usize,
}

/// Unambiguous names for keys whose KLE legend is duplicated or missing.
fn override_label(index: u8) -> Option<&'static str> {
    Some(match index {
        64 => "LShift",
        75 => "RShift",
        80 => "LCtrl",
        81 => "LWin",
        82 => "LAlt",
        83 => "Space",
        84 => "RAlt",
        87 => "RCtrl",
        48 => "CapsLock",
        _ => return None,
    })
}

fn parse_kle(json: &str) -> Vec<Key> {
    let rows: serde_json::Value =
        serde_json::from_str(json).expect("embedded KLE layout is valid JSON");
    let mut keys = Vec::new();
    for (row, items) in rows
        .as_array()
        .expect("KLE root is an array")
        .iter()
        .enumerate()
    {
        for item in items.as_array().into_iter().flatten() {
            let Some(legend) = item.as_str() else {
                continue;
            };
            let mut lines = legend.split('\n');
            let rc = lines.next().unwrap_or("");
            let Some(index) = rc
                .split(',')
                .nth(1)
                .and_then(|s| s.trim().parse::<u8>().ok())
            else {
                continue;
            };
            // The last legend line is the unshifted character ("~\n`" -> "`").
            let text = lines.next_back().unwrap_or("").to_string();
            let label = override_label(index).map(str::to_string).unwrap_or(text);
            keys.push(Key { index, label, row });
        }
    }
    keys
}

/// All physical keys in layout order (row by row, left to right).
pub fn keys() -> &'static [Key] {
    static KEYS: OnceLock<Vec<Key>> = OnceLock::new();
    KEYS.get_or_init(|| parse_kle(KLE_JSON))
}

/// Label of a key index, if it is a physical key.
pub fn label(index: usize) -> Option<&'static str> {
    keys()
        .iter()
        .find(|k| k.index as usize == index)
        .map(|k| k.label.as_str())
}

/// Index of a key by label (case-insensitive).
pub fn index_of(label: &str) -> Option<u8> {
    keys()
        .iter()
        .find(|k| k.label.eq_ignore_ascii_case(label))
        .map(|k| k.index)
}

pub fn is_physical(index: usize) -> bool {
    label(index).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_has_84_unique_keys() {
        let k = keys();
        assert_eq!(k.len(), 84);
        let mut idx: Vec<u8> = k.iter().map(|k| k.index).collect();
        idx.sort();
        idx.dedup();
        assert_eq!(idx.len(), 84);
        let mut labels: Vec<&str> = k.iter().map(|k| k.label.as_str()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 84, "labels must be unambiguous");
    }

    #[test]
    fn known_indices() {
        assert_eq!(label(0), Some("Esc"));
        assert_eq!(label(16), Some("`"));
        assert_eq!(label(34), Some("W"));
        assert_eq!(label(49), Some("A"));
        assert_eq!(label(50), Some("S"));
        assert_eq!(label(51), Some("D"));
        assert_eq!(label(64), Some("LShift"));
        assert_eq!(label(80), Some("LCtrl"));
        assert_eq!(label(83), Some("Space"));
        assert_eq!(label(99), Some("Print"));
        assert_eq!(label(108), Some("PgDn"));
        assert_eq!(label(85), Some("Fn"));
        assert_eq!(label(29), None);
        assert_eq!(index_of("space"), Some(83));
    }
}
