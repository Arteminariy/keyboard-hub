//! GET_LED_EFFECT (0x13), 16 bytes, and GET_CUSTOM_LED_DATA (0x14), 512 bytes
//! (PROTOCOL.md §3.5, §3.6), plus the matching writes SET_LED_EFFECT (0x23)
//! and SET_CUSTOM_LED_DATA (0x24) (§4.5, §4.6).
//!
//! The encodings follow the vendor hub (`Rt` / `Gt` in `owned-BuJoYyKq.js`).
//! The hub declares no lighting quirks for this board (no effect remap, no
//! parameter encodings, no mode readback table), so effect ids are written
//! as the firmware bytes 1..=19 and 128.
//!
//! What the firmware reports back (VERIFIED, FW 1.17, see PROTOCOL.md §3.5):
//! - byte 4 and the `AA 55` check code read back as 0;
//! - the colour (bytes 1..=3) and colour mode (byte 8) are applied but not
//!   reported: after writing mode 11 with #00A0FF single-colour the board
//!   showed blue, yet read back `FF FF FF` / multicolour;
//! - writing the custom effect 128 reads back as mode 20 (custom).
//!
//! Two comparisons follow from that. Diff and skip decisions use
//! [`effect_fields_eq`], which still includes the colour, so a colour change
//! is written. The post-write check uses [`effect_verify_eq`], which compares
//! only the fields the board reports.

use serde::{Deserialize, Serialize};

pub const EFFECT_SIZE: usize = 16;
pub const CUSTOM_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedEffect {
    pub mode: u8,
    pub color: [u8; 3],
    pub driver_setting: u8,
    pub secondary_color: [u8; 3],
    pub color_mode: u8,
    pub brightness: u8,
    pub speed: u8,
    pub direction: u8,
    pub effect_mode_type: u8,
    pub byte13: u8,
    pub check_code: [u8; 2],
}

impl LedEffect {
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < EFFECT_SIZE {
            return None;
        }
        Some(LedEffect {
            mode: b[0],
            color: [b[1], b[2], b[3]],
            driver_setting: b[4],
            secondary_color: [b[5], b[6], b[7]],
            color_mode: b[8],
            brightness: b[9],
            speed: b[10],
            direction: b[11],
            effect_mode_type: b[12],
            byte13: b[13],
            check_code: [b[14], b[15]],
        })
    }

    pub fn mode_name(&self) -> &'static str {
        effect_name(self.mode)
    }

    /// The 16 bytes the hub writes (`Rt`): byte 4 = 0xFF, byte 13 = 0,
    /// bytes 14..15 = `AA 55`.
    pub fn encode(&self) -> [u8; EFFECT_SIZE] {
        let mut b = [0u8; EFFECT_SIZE];
        b[0] = self.mode;
        b[1..4].copy_from_slice(&self.color);
        b[4] = 0xFF;
        b[5..8].copy_from_slice(&self.secondary_color);
        b[8] = self.color_mode;
        b[9] = self.brightness;
        b[10] = self.speed;
        b[11] = self.direction;
        b[12] = self.effect_mode_type;
        b[14] = 0xAA;
        b[15] = 0x55;
        b
    }
}

/// Mode the board reports after the custom effect 128 is written
/// (VERIFIED, FW 1.17). The hub shows it as "custom" too (`It`, `Xt`).
pub const REPORTED_CUSTOM_MODE: u8 = 20;

/// Whether two mode bytes mean the same effect on this board: equal, or the
/// custom effect written as 128 and reported as 20.
pub fn same_mode(a: u8, b: u8) -> bool {
    let canon = |m: u8| if m == REPORTED_CUSTOM_MODE { 128 } else { m };
    canon(a) == canon(b)
}

/// Equality for diff and skip decisions: bytes 0..=3 and 5..=12, with modes
/// compared by [`same_mode`]. The colour is included on purpose: the board
/// does not report it, so a colour that differs from the report is written.
pub fn effect_fields_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() >= EFFECT_SIZE
        && b.len() >= EFFECT_SIZE
        && same_mode(a[0], b[0])
        && a[1..4] == b[1..4]
        && a[5..13] == b[5..13]
}

/// Bytes of an effect block the board reports back faithfully after a
/// write, besides the mode (VERIFIED, FW 1.17): brightness, speed,
/// direction. Secondary colour and effectModeType always read 0 and are
/// written as read, so they are compared too.
pub const VERIFIED_EFFECT_BYTES: [usize; 7] = [5, 6, 7, 9, 10, 11, 12];

/// Post-write check: compares the mode ([`same_mode`]) and
/// [`VERIFIED_EFFECT_BYTES`]. The colour and colour mode are not compared,
/// because the board does not report them ([`colour_verifiable`]).
pub fn effect_verify_eq(written: &[u8], read_back: &[u8]) -> bool {
    written.len() >= EFFECT_SIZE
        && read_back.len() >= EFFECT_SIZE
        && same_mode(written[0], read_back[0])
        && VERIFIED_EFFECT_BYTES
            .iter()
            .all(|&i| written[i] == read_back[i])
}

/// Whether a readback confirms the written colour and colour mode. On FW
/// 1.17 it does only by coincidence (the board reports `FF FF FF`,
/// multicolour, whatever was written).
pub fn colour_verifiable(written: &[u8], read_back: &[u8]) -> bool {
    written.len() >= EFFECT_SIZE
        && read_back.len() >= EFFECT_SIZE
        && written[1..4] == read_back[1..4]
        && written[8] == read_back[8]
}

/// Changes to the lighting effect. Unset fields keep the board's value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectPatch {
    pub mode: Option<u8>,
    pub color: Option<[u8; 3]>,
    /// 0 = single colour, 1 = multicolour (hub `hfd.light.colorMode`).
    pub color_mode: Option<u8>,
    /// Step 0..=5.
    pub brightness: Option<u8>,
    /// Step 0..=5.
    pub speed: Option<u8>,
    /// 0 = forward, 1 = reverse.
    pub direction: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LedError {
    #[error("LED effect block must be {EFFECT_SIZE} bytes, got {0}")]
    EffectSize(usize),
    #[error("custom LED block must be {CUSTOM_SIZE} bytes, got {0}")]
    CustomSize(usize),
    #[error("effect id {0} is not offered on this board")]
    Mode(u8),
    #[error("{0} must be a step 0..=5, got {1}")]
    Step(&'static str, u8),
    #[error("{0} must be 0 or 1, got {1}")]
    Flag(&'static str, u8),
    #[error("key index {0} is not one of the 84 physical keys")]
    NotPhysical(usize),
    #[error("key index {0} is listed more than once")]
    Duplicate(usize),
    #[error("per-key colours need the custom effect (mode 128); the effect would be {0}")]
    NotCustom(String),
}

impl EffectPatch {
    /// Apply to the effect block read from the board. Setting a colour
    /// without a colour mode selects single-colour mode, as the hub does.
    pub fn apply(&self, base: &[u8]) -> Result<[u8; EFFECT_SIZE], LedError> {
        let mut e = LedEffect::decode(base).ok_or(LedError::EffectSize(base.len()))?;
        if let Some(m) = self.mode {
            e.mode = m;
        }
        if let Some(c) = self.color {
            e.color = c;
            e.color_mode = 0;
        }
        if let Some(c) = self.color_mode {
            e.color_mode = c;
        }
        if let Some(b) = self.brightness {
            e.brightness = b;
        }
        if let Some(s) = self.speed {
            e.speed = s;
        }
        if let Some(d) = self.direction {
            e.direction = d;
        }
        validate_effect(&e)?;
        Ok(e.encode())
    }
}

/// Values every effect write must hold.
pub fn validate_effect(e: &LedEffect) -> Result<(), LedError> {
    if !BOARD_EFFECTS.contains(&e.mode) && e.mode != REPORTED_CUSTOM_MODE {
        return Err(LedError::Mode(e.mode));
    }
    if e.brightness > 5 {
        return Err(LedError::Step("brightness", e.brightness));
    }
    if e.speed > 5 {
        return Err(LedError::Step("speed", e.speed));
    }
    if e.direction > 1 {
        return Err(LedError::Flag("direction", e.direction));
    }
    if e.color_mode > 1 {
        return Err(LedError::Flag("colour mode", e.color_mode));
    }
    Ok(())
}

/// Normalise a raw effect block the way the hub writes it (restore path).
pub fn normalise_effect(raw: &[u8]) -> Result<[u8; EFFECT_SIZE], LedError> {
    let e = LedEffect::decode(raw).ok_or(LedError::EffectSize(raw.len()))?;
    validate_effect(&e)?;
    Ok(e.encode())
}

/// One-line description of an effect block. The colour is the one in the
/// block; for a block read from the board it is not the colour shown.
pub fn describe_effect(raw: &[u8]) -> String {
    match LedEffect::decode(raw) {
        Some(e) => format!(
            "{} «{}» (mode {}), colour #{:02X}{:02X}{:02X} {}, brightness {}/5, speed {}/5, direction {}",
            e.mode_name(),
            effect_label_ru(e.mode),
            e.mode,
            e.color[0],
            e.color[1],
            e.color[2],
            if e.color_mode == 0 { "single" } else { "multicolour" },
            e.brightness,
            e.speed,
            if e.direction == 0 { "forward" } else { "reverse" },
        ),
        None => "invalid effect block".into(),
    }
}

/// Parse `#RRGGBB` or `RRGGBB`.
pub fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn format_hex_color(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Effect ids this board offers (hub list `Kt`: 1..=19 and 128), plus 0 = off.
/// A write may also carry [`REPORTED_CUSTOM_MODE`] (restoring a block read
/// in custom mode, as the hub's snapshot restore does), but it is not
/// offered by name.
pub const BOARD_EFFECTS: [u8; 21] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 128,
];

/// Effect id by name (case-insensitive) or number, restricted to
/// [`BOARD_EFFECTS`].
pub fn effect_id(name: &str) -> Option<u8> {
    if let Ok(n) = name.trim().parse::<u8>() {
        return BOARD_EFFECTS.contains(&n).then_some(n);
    }
    BOARD_EFFECTS
        .into_iter()
        .find(|&m| effect_name(m).eq_ignore_ascii_case(name.trim()))
}

/// Per-key colours are shown and writable only in these modes (hub `It`).
/// On this board mode 20 is what custom (128) reads back as; it shows the
/// stored per-key block (VERIFIED, FW 1.17).
pub fn is_custom_mode(mode: u8) -> bool {
    matches!(mode, 20 | 21 | 22 | 128)
}

pub fn effect_name(mode: u8) -> &'static str {
    match mode {
        0 => "off",
        1 => "static",
        2 => "onPress",
        3 => "offPress",
        4 => "stars",
        5 => "snow",
        6 => "riot",
        7 => "breathing",
        8 => "colorWheel",
        9 => "fountain",
        10 => "ripple",
        11 => "wave",
        12 => "serpentine",
        13 => "flash",
        14 => "double",
        15 => "circles",
        16 => "stream",
        17 => "ridges",
        18 => "slantRain",
        19 => "backAndForth",
        20 | 128 => "custom",
        _ => "unknown",
    }
}

/// The hub's Russian label for an effect (`hfd.effect.*` in the ru table of
/// `index-DQigh6Ga.js`). The user's observation of 0x0F matches: the hub
/// writes 15 for «Расходящаяся рябь».
pub fn effect_label_ru(mode: u8) -> &'static str {
    match mode {
        0 => "Выкл.",
        1 => "Статичный цвет",
        2 => "Включение при нажатии",
        3 => "Выключение при нажатии",
        4 => "Звёздное небо",
        5 => "Падающий снег",
        6 => "Цветочное великолепие",
        7 => "Динамическое дыхание",
        8 => "Спектральный цикл",
        9 => "Цветной фонтан",
        10 => "Многоцветные волны",
        11 => "По волнам",
        12 => "Горный серпантин",
        13 => "Горизонтальные разряды",
        14 => "Двух зайцев одним выстрелом",
        15 => "Расходящаяся рябь",
        16 => "Непрерывный поток",
        17 => "Горные хребты",
        18 => "Косой дождь",
        19 => "Туда и обратно",
        20 | 128 => "Пользовательский",
        _ => "?",
    }
}

/// One per-key colour record `[led_id, r, g, b]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedColor {
    pub led_id: u8,
    pub rgb: [u8; 3],
}

pub fn decode_custom(block: &[u8]) -> Vec<LedColor> {
    block
        .chunks_exact(4)
        .take(128)
        .map(|c| LedColor {
            led_id: c[0],
            rgb: [c[1], c[2], c[3]],
        })
        .collect()
}

/// Per-key colour patch merged into the custom LED block read from the
/// board. Every record is re-encoded with `ledId = index`, as the hub does.
pub fn apply_custom_patch(base: &[u8], patch: &[(u8, [u8; 3])]) -> Result<Vec<u8>, LedError> {
    if base.len() != CUSTOM_SIZE {
        return Err(LedError::CustomSize(base.len()));
    }
    let mut out = normalise_custom(base)?;
    let mut seen = std::collections::BTreeSet::new();
    for &(index, rgb) in patch {
        let i = index as usize;
        if !crate::layout::is_physical(i) {
            return Err(LedError::NotPhysical(i));
        }
        if !seen.insert(i) {
            return Err(LedError::Duplicate(i));
        }
        out[i * 4 + 1..i * 4 + 4].copy_from_slice(&rgb);
    }
    Ok(out)
}

/// Re-encode a custom LED block with `ledId = index` (restore path).
pub fn normalise_custom(raw: &[u8]) -> Result<Vec<u8>, LedError> {
    if raw.len() != CUSTOM_SIZE {
        return Err(LedError::CustomSize(raw.len()));
    }
    let mut out = raw.to_vec();
    for i in 0..128 {
        out[i * 4] = i as u8;
    }
    Ok(out)
}

/// Compare the colours of two custom LED blocks, ignoring the ledId bytes.
pub fn custom_colors_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == CUSTOM_SIZE
        && b.len() == CUSTOM_SIZE
        && (0..128).all(|i| a[i * 4 + 1..i * 4 + 4] == b[i * 4 + 1..i * 4 + 4])
}

/// Indices whose colour differs.
pub fn custom_changed(a: &[u8], b: &[u8]) -> Vec<usize> {
    (0..128)
        .filter(|&i| a.get(i * 4 + 1..i * 4 + 4) != b.get(i * 4 + 1..i * 4 + 4))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EFFECT_FRAME: &str = "55131000000001000fffffff00000000010505000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
    const CUSTOM_FRAME_0: &str = "5514380000000000000000000100000002000000030000000400000005000000060000000700000008000000090000000a0000000b0000000c0000000d000000";

    #[test]
    fn decode_captured_effect() {
        let f = hex::decode(EFFECT_FRAME).unwrap();
        let e = LedEffect::decode(&f[8..24]).unwrap();
        assert_eq!(e.mode, 15);
        assert_eq!(e.mode_name(), "circles");
        assert_eq!(e.color, [0xFF, 0xFF, 0xFF]);
        assert_eq!(e.color_mode, 1);
        assert_eq!(e.brightness, 5);
        assert_eq!(e.speed, 5);
    }

    #[test]
    fn encode_matches_captured_old_app_write() {
        // PROTOCOL.md §4.5: AA 23 10 00 00 00 01 00 | 80 FF 00 00 FF 00 00 00 00 03 05 00 00 00 AA 55
        let want = hex::decode("80ff0000ff000000000305000000aa55").unwrap();
        let mut base = [0u8; 16];
        base[0] = 15;
        let p = EffectPatch {
            mode: Some(128),
            color: Some([0xFF, 0, 0]),
            brightness: Some(3),
            speed: Some(5),
            ..Default::default()
        };
        let got = p.apply(&base).unwrap();
        assert_eq!(&got[..], &want[..16]);
    }

    #[test]
    fn effect_patch_and_field_compare() {
        // The user's board (FW 1.17): circles, white, multicolour, 5/4.
        let read = hex::decode("0fffffff000000000105040000000000").unwrap();
        let same = EffectPatch::default().apply(&read).unwrap();
        assert!(
            effect_fields_eq(&read, &same),
            "only bytes 4/13/14/15 differ"
        );
        assert_ne!(&read[..], &same[..]);
        let wave = EffectPatch {
            mode: Some(effect_id("wave").unwrap()),
            color: Some(parse_hex_color("#00A0FF").unwrap()),
            ..Default::default()
        }
        .apply(&read)
        .unwrap();
        assert_eq!(wave[0], 11);
        assert_eq!(&wave[1..4], &[0x00, 0xA0, 0xFF]);
        assert_eq!(wave[8], 0, "a colour selects single-colour mode");
        assert_eq!((wave[9], wave[10]), (5, 4), "brightness and speed kept");
        assert!(!effect_fields_eq(&read, &wave));
        assert!(EffectPatch {
            brightness: Some(6),
            ..Default::default()
        }
        .apply(&read)
        .is_err());
        assert!(EffectPatch {
            mode: Some(23),
            ..Default::default()
        }
        .apply(&read)
        .is_err());
        assert!(EffectPatch {
            direction: Some(2),
            ..Default::default()
        }
        .apply(&read)
        .is_err());
        assert_eq!(effect_id("Wave"), Some(11));
        assert_eq!(effect_id("128"), Some(128));
        assert_eq!(effect_id("20"), None);
        assert_eq!(format_hex_color([1, 0xAB, 0xFF]), "#01ABFF");
        assert_eq!(parse_hex_color("zz0000"), None);
    }

    #[test]
    fn verify_ignores_unreported_colour_and_aliases_custom() {
        // FW 1.17, observed: wrote mode 11, #00A0FF single colour, 5/4;
        // the board showed blue but read back white multicolour.
        let read = hex::decode("0fffffff000000000105040000000000").unwrap();
        let written = EffectPatch {
            mode: Some(11),
            color: Some([0x00, 0xA0, 0xFF]),
            ..Default::default()
        }
        .apply(&read)
        .unwrap();
        let back = hex::decode("0bffffff000000000105040000000000").unwrap();
        assert!(effect_verify_eq(&written, &back));
        assert!(!colour_verifiable(&written, &back));
        assert!(!effect_fields_eq(&written, &back), "diffs still see colour");
        // A brightness, speed or direction the board did not take fails.
        for i in [9, 10, 11] {
            let mut bad = back.clone();
            bad[i] ^= 1;
            assert!(!effect_verify_eq(&written, &bad), "byte {i}");
        }
        // Wrong mode fails.
        let mut bad = back.clone();
        bad[0] = 7;
        assert!(!effect_verify_eq(&written, &bad));

        // FW 1.17, observed: wrote 128 (custom), read back 20.
        let custom = EffectPatch {
            mode: Some(128),
            ..Default::default()
        }
        .apply(&read)
        .unwrap();
        let mut back20 = read.clone();
        back20[0] = 20;
        assert!(effect_verify_eq(&custom, &back20));
        assert!(effect_fields_eq(&custom, &back20));
        assert!(same_mode(20, 128) && same_mode(128, 20) && !same_mode(21, 128));
        assert!(is_custom_mode(20));
        assert_eq!(effect_name(20), "custom");
        // A block read in mode 20 can be restored and patched.
        assert_eq!(normalise_effect(&back20).unwrap()[0], 20);
        let dimmer = EffectPatch {
            brightness: Some(2),
            ..Default::default()
        }
        .apply(&back20)
        .unwrap();
        assert_eq!((dimmer[0], dimmer[9]), (20, 2));
        assert_eq!(effect_id("custom"), Some(128), "custom is written as 128");
        assert_eq!(effect_label_ru(15), "Расходящаяся рябь");
        assert!(describe_effect(&back20).starts_with("custom «Пользовательский» (mode 20)"));
    }

    #[test]
    fn custom_patch_sets_led_ids_and_colors() {
        let base = vec![0u8; 512];
        let out = apply_custom_patch(&base, &[(34, [0, 0xFF, 0])]).unwrap();
        assert_eq!(
            &out[34 * 4..35 * 4],
            &[34, 0, 0xFF, 0],
            "py capture: W = 22 00 FF 00"
        );
        assert_eq!(out[127 * 4], 127);
        assert!(custom_colors_eq(&base, &normalise_custom(&base).unwrap()));
        assert_eq!(custom_changed(&base, &out), vec![34]);
        assert!(apply_custom_patch(&base, &[(29, [1, 1, 1])]).is_err());
        assert!(apply_custom_patch(&base, &[(34, [1, 1, 1]), (34, [2, 2, 2])]).is_err());
    }

    #[test]
    fn decode_captured_custom_colors() {
        let f = hex::decode(CUSTOM_FRAME_0).unwrap();
        let c = decode_custom(&f[8..64]);
        assert_eq!(c.len(), 14);
        for (i, rec) in c.iter().enumerate() {
            assert_eq!(rec.led_id, i as u8);
            assert_eq!(rec.rgb, [0, 0, 0]);
        }
    }
}
