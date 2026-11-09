//! Key geometry from the KLE layout (docs/type84_layout.kle.json) and the
//! mapping from egui key events to firmware key indices.

use std::sync::OnceLock;

const KLE_JSON: &str = include_str!("../../../docs/type84_layout.kle.json");

/// One key in layout units (1u = one standard key).
#[derive(Debug, Clone, PartialEq)]
pub struct KeyGeom {
    pub index: u8,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Legend lines as in the KLE file, without the "r,c" prefix
    /// (shifted symbol first, base symbol last).
    pub legend: Vec<String>,
}

/// Parse standard KLE rows: each row starts at x=0 one unit below the
/// previous row; property objects (`x`, `y`, `w`, `h`) adjust the next key.
fn parse(json: &str) -> Vec<KeyGeom> {
    let rows: serde_json::Value =
        serde_json::from_str(json).expect("embedded KLE layout is valid JSON");
    let mut out = Vec::new();
    let mut y = 0.0f32;
    for row in rows.as_array().expect("KLE root is an array") {
        let Some(items) = row.as_array() else {
            continue;
        };
        let mut x = 0.0f32;
        let (mut w, mut h) = (1.0f32, 1.0f32);
        for item in items {
            if let Some(obj) = item.as_object() {
                let num = |k: &str| obj.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
                if let Some(dx) = num("x") {
                    x += dx;
                }
                if let Some(dy) = num("y") {
                    y += dy;
                }
                if let Some(v) = num("w") {
                    w = v;
                }
                if let Some(v) = num("h") {
                    h = v;
                }
                continue;
            }
            let Some(text) = item.as_str() else { continue };
            let mut lines = text.split('\n');
            let rc = lines.next().unwrap_or("");
            if let Some(index) = rc
                .split(',')
                .nth(1)
                .and_then(|s| s.trim().parse::<u8>().ok())
            {
                out.push(KeyGeom {
                    index,
                    x,
                    y,
                    w,
                    h,
                    legend: lines.map(str::to_string).collect(),
                });
            }
            x += w;
            w = 1.0;
            h = 1.0;
        }
        y += 1.0;
    }
    out
}

pub fn keys() -> &'static [KeyGeom] {
    static KEYS: OnceLock<Vec<KeyGeom>> = OnceLock::new();
    KEYS.get_or_init(|| parse(KLE_JSON))
}

/// Overall size in units.
pub fn size() -> (f32, f32) {
    keys().iter().fold((0.0f32, 0.0f32), |(w, h), k| {
        (w.max(k.x + k.w), h.max(k.y + k.h))
    })
}

/// Firmware index for an egui key (physical position, so it works with
/// any OS layout, including ЙЦУКЕН).
pub fn index_for_key(key: egui::Key) -> Option<u8> {
    use egui::Key as K;
    Some(match key {
        K::Escape => 0,
        K::F1 => 1,
        K::F2 => 2,
        K::F3 => 3,
        K::F4 => 4,
        K::F5 => 5,
        K::F6 => 6,
        K::F7 => 7,
        K::F8 => 8,
        K::F9 => 9,
        K::F10 => 10,
        K::F11 => 11,
        K::F12 => 12,
        K::Home => 104,
        K::End => 107,
        K::Backtick => 16,
        K::Num1 => 17,
        K::Num2 => 18,
        K::Num3 => 19,
        K::Num4 => 20,
        K::Num5 => 21,
        K::Num6 => 22,
        K::Num7 => 23,
        K::Num8 => 24,
        K::Num9 => 25,
        K::Num0 => 26,
        K::Minus => 27,
        K::Equals => 28,
        K::Backspace => 92,
        K::Insert => 103,
        K::PageUp => 105,
        K::Tab => 32,
        K::Q => 33,
        K::W => 34,
        K::E => 35,
        K::R => 36,
        K::T => 37,
        K::Y => 38,
        K::U => 39,
        K::I => 40,
        K::O => 41,
        K::P => 42,
        K::OpenBracket => 43,
        K::CloseBracket => 44,
        K::Backslash => 60,
        K::Delete => 106,
        K::PageDown => 108,
        K::A => 49,
        K::S => 50,
        K::D => 51,
        K::F => 52,
        K::G => 53,
        K::H => 54,
        K::J => 55,
        K::K => 56,
        K::L => 57,
        K::Semicolon => 58,
        K::Quote => 59,
        K::Enter => 76,
        K::Z => 65,
        K::X => 66,
        K::C => 67,
        K::V => 68,
        K::B => 69,
        K::N => 70,
        K::M => 71,
        K::Comma => 72,
        K::Period => 73,
        K::Slash => 74,
        K::ArrowUp => 90,
        K::Space => 83,
        K::ArrowLeft => 88,
        K::ArrowDown => 89,
        K::ArrowRight => 91,
        _ => return None,
    })
}

/// Modifier keys arrive only as a combined state without left/right, so
/// they map to the left-hand keys. Win, Fn, Caps Lock and Print Screen are
/// not reported by egui at all.
pub const SHIFT_INDEX: u8 = 64;
pub const CTRL_INDEX: u8 = 80;
pub const ALT_INDEX: u8 = 82;

pub const PRESET_WASD: [u8; 4] = [34, 49, 50, 51];
pub const PRESET_ARROWS: [u8; 4] = [90, 88, 89, 91];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_matches_spec() {
        let k = keys();
        assert_eq!(k.len(), 84);
        let (w, h) = size();
        assert!((w - 17.25).abs() < 1e-4, "width {w}");
        assert!((h - 6.25).abs() < 1e-4, "height {h}");
        let space = k.iter().find(|k| k.index == 83).unwrap();
        assert_eq!((space.x, space.y, space.w), (3.75, 5.25, 6.25));
        let pgdn = k.iter().find(|k| k.index == 108).unwrap();
        assert_eq!((pgdn.x, pgdn.y), (16.25, 2.25));
        let up = k.iter().find(|k| k.index == 90).unwrap();
        assert_eq!((up.x, up.y), (15.25, 4.25));
    }

    #[test]
    fn key_map_hits_physical_keys() {
        for key in egui::Key::ALL {
            if let Some(i) = index_for_key(*key) {
                assert!(
                    type84_proto::layout::is_physical(i as usize),
                    "{key:?} -> {i}"
                );
            }
        }
        assert_eq!(index_for_key(egui::Key::W), Some(34));
        for i in
            PRESET_WASD
                .iter()
                .chain(&PRESET_ARROWS)
                .chain(&[SHIFT_INDEX, CTRL_INDEX, ALT_INDEX])
        {
            assert!(type84_proto::layout::is_physical(*i as usize));
        }
    }
}
