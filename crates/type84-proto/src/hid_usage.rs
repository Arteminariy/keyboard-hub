//! Short names for HID usages (USB HID Usage Tables, keyboard page 0x07 and
//! consumer page 0x0C), used for human-readable output and for parsing the
//! action names of preset files.

/// Name of a keyboard-page usage, if it is a common one.
pub fn keyboard_usage_name(u: u8) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R",
        "S", "T", "U", "V", "W", "X", "Y", "Z",
    ];
    const DIGITS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
    const F1_F12: [&str; 12] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    ];
    const F13_F24: [&str; 12] = [
        "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    ];
    const MODS: [&str; 8] = [
        "LCtrl", "LShift", "LAlt", "LGui", "RCtrl", "RShift", "RAlt", "RGui",
    ];
    Some(match u {
        0x04..=0x1D => LETTERS[(u - 0x04) as usize],
        0x1E..=0x27 => DIGITS[(u - 0x1E) as usize],
        0x28 => "Enter",
        0x29 => "Esc",
        0x2A => "Backspace",
        0x2B => "Tab",
        0x2C => "Space",
        0x2D => "-",
        0x2E => "=",
        0x2F => "[",
        0x30 => "]",
        0x31 => "\\",
        0x32 => "NonUS#",
        0x33 => ";",
        0x34 => "'",
        0x35 => "`",
        0x36 => ",",
        0x37 => ".",
        0x38 => "/",
        0x39 => "CapsLock",
        0x3A..=0x45 => F1_F12[(u - 0x3A) as usize],
        0x46 => "PrintScreen",
        0x47 => "ScrollLock",
        0x48 => "Pause",
        0x49 => "Insert",
        0x4A => "Home",
        0x4B => "PgUp",
        0x4C => "Delete",
        0x4D => "End",
        0x4E => "PgDn",
        0x4F => "Right",
        0x50 => "Left",
        0x51 => "Down",
        0x52 => "Up",
        0x53 => "NumLock",
        0x54 => "KP/",
        0x55 => "KP*",
        0x56 => "KP-",
        0x57 => "KP+",
        0x58 => "KPEnter",
        0x59..=0x61 => [
            "KP1", "KP2", "KP3", "KP4", "KP5", "KP6", "KP7", "KP8", "KP9",
        ][(u - 0x59) as usize],
        0x62 => "KP0",
        0x63 => "KP.",
        0x64 => "NonUS\\",
        0x65 => "Menu",
        0x68..=0x73 => F13_F24[(u - 0x68) as usize],
        0xE0..=0xE7 => MODS[(u - 0xE0) as usize],
        _ => return None,
    })
}

/// Name of a consumer-page usage (0x0C), if it is a common one.
pub fn consumer_usage_name(u: u16) -> Option<&'static str> {
    Some(match u {
        0x00B5 => "NextTrack",
        0x00B6 => "PrevTrack",
        0x00B7 => "Stop",
        0x00CD => "PlayPause",
        0x00E2 => "Mute",
        0x00E9 => "VolumeUp",
        0x00EA => "VolumeDown",
        0x006F => "BrightnessUp",
        0x0070 => "BrightnessDown",
        0x0183 => "MediaSelect",
        0x018A => "Mail",
        0x0192 => "Calculator",
        0x0194 => "MyComputer",
        0x0196 => "Browser",
        0x0221 => "WWWSearch",
        0x0223 => "WWWHome",
        0x0224 => "WWWBack",
        0x0225 => "WWWForward",
        0x022A => "WWWFavorites",
        _ => return None,
    })
}

/// Keyboard usage by name (case-insensitive): the names of
/// [`keyboard_usage_name`] plus a few aliases, or `0xNN`.
pub fn keyboard_usage_by_name(name: &str) -> Option<u8> {
    let alias = match name.to_ascii_lowercase().as_str() {
        "print" | "prtsc" | "printscr" | "sysrq" => Some(0x46),
        "escape" => Some(0x29),
        "return" => Some(0x28),
        "del" => Some(0x4C),
        "ins" => Some(0x49),
        "pageup" => Some(0x4B),
        "pagedown" => Some(0x4E),
        "caps" => Some(0x39),
        "ctrl" => Some(0xE0),
        "shift" => Some(0xE1),
        "alt" => Some(0xE2),
        "win" | "lwin" | "super" | "meta" => Some(0xE3),
        "rwin" => Some(0xE7),
        _ => None,
    };
    if alias.is_some() {
        return alias;
    }
    if let Some(h) = name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
        return u8::from_str_radix(h, 16).ok();
    }
    (0u8..=0xFF).find(|&u| keyboard_usage_name(u).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// Consumer usage by name (case-insensitive): the names of
/// [`consumer_usage_name`] plus aliases, or `0xNNNN`.
pub fn consumer_usage_by_name(name: &str) -> Option<u16> {
    let alias = match name.to_ascii_lowercase().as_str() {
        "home" | "homepage" | "achome" => Some(0x0223),
        "back" | "acback" => Some(0x0224),
        "forward" => Some(0x0225),
        "search" | "acsearch" => Some(0x0221),
        "www" | "internetbrowser" => Some(0x0196),
        "play" => Some(0x00CD),
        "next" => Some(0x00B5),
        "prev" | "previous" => Some(0x00B6),
        "volup" => Some(0x00E9),
        "voldown" => Some(0x00EA),
        _ => None,
    };
    if alias.is_some() {
        return alias;
    }
    if let Some(h) = name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
        return u16::from_str_radix(h, 16).ok();
    }
    (0u16..=0x03FF).find(|&u| consumer_usage_name(u).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(keyboard_usage_name(0x04), Some("A"));
        assert_eq!(keyboard_usage_name(0x1D), Some("Z"));
        assert_eq!(keyboard_usage_name(0x27), Some("0"));
        assert_eq!(keyboard_usage_name(0x45), Some("F12"));
        assert_eq!(keyboard_usage_name(0xE7), Some("RGui"));
        assert_eq!(keyboard_usage_name(0xAF), None);
        assert_eq!(consumer_usage_name(0x0192), Some("Calculator"));
    }

    #[test]
    fn reverse_lookup() {
        assert_eq!(keyboard_usage_by_name("f13"), Some(0x68));
        assert_eq!(keyboard_usage_by_name("F10"), Some(0x43));
        assert_eq!(keyboard_usage_by_name("PrintScreen"), Some(0x46));
        assert_eq!(keyboard_usage_by_name("print"), Some(0x46));
        assert_eq!(keyboard_usage_by_name("0x6a"), Some(0x6A));
        assert_eq!(keyboard_usage_by_name("nope"), None);
        assert_eq!(consumer_usage_by_name("PlayPause"), Some(0x00CD));
        assert_eq!(consumer_usage_by_name("wwwhome"), Some(0x0223));
        assert_eq!(consumer_usage_by_name("Browser"), Some(0x0196));
        assert_eq!(consumer_usage_by_name("0x0196"), Some(0x0196));
        for u in [
            0xB5u16, 0xB6, 0xCD, 0xE2, 0xE9, 0xEA, 0x196, 0x221, 0x223, 0x224,
        ] {
            let n = consumer_usage_name(u).unwrap();
            assert_eq!(consumer_usage_by_name(n), Some(u), "{n}");
        }
    }
}
