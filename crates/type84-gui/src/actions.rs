//! Key actions in Russian: keycap legends and descriptions of keymap
//! records, the Fn-layer picker vocabulary, and which Fn-layer keys are
//! locked. The lock rule is `type84_proto::fn_layer::replaceable`, the same
//! predicate the write path enforces.

use type84_proto::fn_layer::{self, FnAction, FN_KEY_INDEX};
use type84_proto::hid_usage::{consumer_usage_name, keyboard_usage_by_name, keyboard_usage_name};
use type84_proto::keymap::KeyAction;

/// Consumer usages offered by the picker, with the hub's Russian names.
pub const CONSUMER: [(u16, &str); 18] = [
    (0x00CD, "Воспр./пауза"),
    (0x00B7, "Стоп"),
    (0x00B6, "Пред. трек"),
    (0x00B5, "След. трек"),
    (0x00E9, "Громк. +"),
    (0x00EA, "Громк. −"),
    (0x00E2, "Без звука"),
    (0x0223, "Домой в браузере"),
    (0x0224, "Назад"),
    (0x0225, "Вперёд"),
    (0x022A, "Избранное"),
    (0x0221, "Поиск"),
    (0x0183, "Проигрыватель"),
    (0x018A, "Почта"),
    (0x0192, "Калькулятор"),
    (0x0194, "Мой компьютер"),
    (0x0196, "Браузер"),
    (0x006F, "Яркость экрана +"),
];

/// Russian name of a consumer usage.
pub fn consumer_ru(u: u16) -> Option<&'static str> {
    match u {
        0x0070 => Some("Яркость экрана −"),
        _ => CONSUMER.iter().find(|(c, _)| *c == u).map(|(_, n)| *n),
    }
}

/// Vendor function (FUNC record, type 13) in words. Ids from the user's
/// factory Fn layer (PROTOCOL.md §3.3).
pub fn func_ru(id: u32) -> String {
    match id {
        1 => "Сброс настроек".into(),
        9 => "Режим Mac".into(),
        11 => "Смена эффекта".into(),
        12 => "Смена цвета".into(),
        13 => "Подсветка ярче".into(),
        14 => "Подсветка темнее".into(),
        15 => "Эффект быстрее".into(),
        16 => "Эффект медленнее".into(),
        22 => "Блокировка Win".into(),
        27 => "Предыдущий эффект".into(),
        n => format!("Функция платы {n}"),
    }
}

fn func_short(id: u32) -> String {
    match id {
        1 => "СБРОС".into(),
        9 => "MAC".into(),
        11 => "ЭФФЕКТ".into(),
        12 => "ЦВЕТ".into(),
        13 => "ЯРЧЕ".into(),
        14 => "ТЕМНЕЕ".into(),
        15 => "БЫСТРЕЕ".into(),
        16 => "МЕДЛ.".into(),
        22 => "WIN LOCK".into(),
        27 => "ЭФФ. ←".into(),
        n => format!("Ф{n}"),
    }
}

fn func_id(r: [u8; 4]) -> u32 {
    (r[1] as u32) << 16 | (r[2] as u32) << 8 | r[3] as u32
}

fn key_name(u: u8) -> String {
    match u {
        0xAF => "Fn".into(),
        _ => keyboard_usage_name(u)
            .map(str::to_string)
            .unwrap_or_else(|| format!("0x{u:02X}")),
    }
}

fn mods_prefix(m: u8) -> String {
    type84_proto::fn_layer::MODIFIER_NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| m & (1 << i) != 0)
        .map(|(_, n)| format!("{n}+"))
        .collect()
}

/// Full description of a keymap record (diff rows, picker status).
pub fn record_label(r: [u8; 4]) -> String {
    match KeyAction::decode(r) {
        KeyAction::None => "не назначено".into(),
        KeyAction::Keyboard { usage, raw } => {
            format!("клавиша {}{}", mods_prefix(raw[1]), key_name(usage))
        }
        KeyAction::Consumer { usage } => match consumer_ru(usage) {
            Some(n) => format!("медиа: {n}"),
            None => format!(
                "медиа: {}",
                consumer_usage_name(usage)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("0x{usage:04X}"))
            ),
        },
        KeyAction::Function { .. } => func_ru(func_id(r)),
        KeyAction::Macro { id } => format!("макрос {id}"),
        KeyAction::Dks { slot } => format!("DKS, слот {slot}"),
        KeyAction::ModTap { .. } => "Mod-Tap".into(),
        KeyAction::Toggle { .. } => "переключатель (TGL)".into(),
        KeyAction::Socd { .. } => "Snap Tap (SOCD)".into(),
        KeyAction::RappySnappy { .. } => "Rappy Snappy".into(),
        other => format!("особая функция ({})", other.describe()),
    }
}

/// Short keycap legend of a record.
pub fn record_short(r: [u8; 4]) -> String {
    match KeyAction::decode(r) {
        KeyAction::None => String::new(),
        KeyAction::Keyboard { usage, raw } => {
            let base = match usage {
                0xE0 => "L-Ctrl".to_string(),
                0xE1 => "L-Shift".into(),
                0xE2 => "L-Alt".into(),
                0xE3 => "L-Win".into(),
                0xE4 => "R-Ctrl".into(),
                0xE5 => "R-Shift".into(),
                0xE6 => "R-Alt".into(),
                0xE7 => "R-Win".into(),
                0x2A => "Backspace".into(),
                0x46 => "Print".into(),
                0x39 => "Caps".into(),
                0x47 => "ScrLk".into(),
                _ => key_name(usage),
            };
            if raw[1] != 0 {
                format!("{}{base}", mods_prefix(raw[1]))
            } else {
                base
            }
        }
        KeyAction::Consumer { usage } => consumer_ru(usage)
            .map(str::to_string)
            .unwrap_or_else(|| "Медиа".into()),
        KeyAction::Function { .. } => func_short(func_id(r)),
        KeyAction::Macro { id } => format!("M{id}"),
        KeyAction::Dks { .. } => "DKS".into(),
        KeyAction::ModTap { .. } => "MT".into(),
        KeyAction::Toggle { .. } => "TGL".into(),
        KeyAction::Socd { .. } => "SOCD".into(),
        KeyAction::RappySnappy { .. } => "RS".into(),
        _ => "?".into(),
    }
}

/// Why an Fn-layer key cannot be reassigned, if it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lock {
    /// The Fn key itself.
    FnKey,
    /// A record the write path never overwrites (vendor function, macro,
    /// DKS, advanced key).
    Firmware,
}

/// Lock state of Fn-layer key `index` holding `record` on the board.
pub fn fn_lock(index: u8, record: [u8; 4]) -> Option<Lock> {
    if index as usize == FN_KEY_INDEX {
        Some(Lock::FnKey)
    } else if !fn_layer::replaceable(record) {
        Some(Lock::Firmware)
    } else {
        None
    }
}

/// A picker group: title and actions.
pub struct Group {
    pub title: &'static str,
    pub items: Vec<FnAction>,
}

fn keys(names: &[&str]) -> Vec<FnAction> {
    names
        .iter()
        .map(|n| FnAction::Key {
            mods: 0,
            usage: keyboard_usage_by_name(n).unwrap_or_else(|| panic!("usage {n}")),
        })
        .collect()
}

/// The picker vocabulary, by category.
pub fn groups() -> Vec<Group> {
    let fkeys: Vec<String> = (13..=24).chain(1..=12).map(|n| format!("F{n}")).collect();
    let fkeys: Vec<&str> = fkeys.iter().map(String::as_str).collect();
    let letters: Vec<String> = ('A'..='Z')
        .map(|c| c.to_string())
        .chain((1..=9).map(|d| d.to_string()))
        .chain(["0".to_string()])
        .collect();
    let letters: Vec<&str> = letters.iter().map(String::as_str).collect();
    vec![
        Group {
            title: "Функциональные",
            items: keys(&fkeys),
        },
        Group {
            title: "Медиа",
            items: CONSUMER
                .iter()
                .map(|(u, _)| FnAction::Consumer(*u))
                .chain([FnAction::Consumer(0x0070)])
                .collect(),
        },
        Group {
            title: "Навигация",
            items: keys(&[
                "Insert",
                "Delete",
                "Home",
                "End",
                "PgUp",
                "PgDn",
                "Up",
                "Down",
                "Left",
                "Right",
                "PrintScreen",
                "ScrollLock",
                "Pause",
            ]),
        },
        Group {
            title: "Управление",
            items: keys(&[
                "Esc",
                "Enter",
                "Tab",
                "Backspace",
                "Space",
                "CapsLock",
                "Menu",
                "NumLock",
            ]),
        },
        Group {
            title: "Буквы и цифры",
            items: keys(&letters),
        },
    ]
}

/// Picker button text of an action.
pub fn action_short(a: FnAction) -> String {
    match a {
        FnAction::None => "Не назначено".into(),
        FnAction::Factory => "Заводское".into(),
        FnAction::Key { mods, usage } => record_short([2, mods, usage, 0]),
        FnAction::Consumer(u) => consumer_ru(u)
            .map(str::to_string)
            .unwrap_or_else(|| format!("0x{u:04X}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_locks() {
        assert_eq!(record_label([2, 0, 0x68, 0]), "клавиша F13");
        assert_eq!(record_label([3, 0xCD, 0, 0]), "медиа: Воспр./пауза");
        assert_eq!(record_label([0x0D, 0, 0, 1]), "Сброс настроек");
        assert_eq!(record_label([2, 1, 0x43, 0]), "клавиша Ctrl+F10");
        assert_eq!(record_short([0x0D, 0, 0, 22]), "WIN LOCK");
        assert_eq!(fn_lock(85, [2, 0, 0xAF, 0]), Some(Lock::FnKey));
        assert_eq!(fn_lock(106, [0x0D, 0, 0, 1]), Some(Lock::Firmware));
        assert_eq!(fn_lock(1, [0, 0, 0, 0]), None);
        assert_eq!(fn_lock(92, [2, 0, 0x47, 0]), None);
        assert_eq!(fn_lock(5, [6, 1, 0, 0]), Some(Lock::Firmware), "macro");
    }

    #[test]
    fn picker_covers_f13_f24_and_media() {
        let g = groups();
        let all: Vec<FnAction> = g.iter().flat_map(|g| g.items.clone()).collect();
        for n in 0x68..=0x73u8 {
            assert!(all.contains(&FnAction::Key { mods: 0, usage: n }));
        }
        for (u, _) in CONSUMER {
            assert!(all.contains(&FnAction::Consumer(u)));
        }
        // Every picker action is a record the write path accepts.
        for a in &all {
            assert_eq!(FnAction::from_record(a.record([0; 4])), Some(*a));
        }
    }
}
