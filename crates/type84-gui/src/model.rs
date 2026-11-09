//! Board state as the GUI sees it, built from raw protocol blocks.
//!
//! One decode path feeds both the live read (worker) and the offline
//! backup loader. Distances are kept in hundredths of a millimetre (the UI
//! step), which avoids float drift when counting pending changes.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use type84_cli::backup::Backup;
use type84_cli::preset::{
    FnLayerKey, FnLayerSection, HallKey, HallSection, LightingKey, LightingSection, Preset,
};
use type84_cli::snapshot::Snapshot;
use type84_proto::device_info::DeviceInfo;
use type84_proto::fn_layer::FnAction;
use type84_proto::hall::RtSensitivity;
use type84_proto::hid_usage::keyboard_usage_name;
use type84_proto::keymap::{self, KeyAction};
use type84_proto::layout;
use type84_proto::led;
use type84_proto::rt::{self, RtScale};
use type84_proto::settings::Settings;

/// Ranges from the descriptor quirks (UI_SPEC §1), in hundredths of a mm.
pub const ACTUATION_MIN: u16 = 10;
pub const ACTUATION_MAX: u16 = 320;
pub const ACTUATION_PRESET: u16 = 120;
pub const SENS_MIN: u16 = 1;
pub const SENS_MAX: u16 = 320;
pub const SENS_PRESET: u16 = 10;
pub const DEAD_ZONE_MAX: u16 = 50;
pub const REPORT_RATES: [u32; 3] = [1000, 4000, 8000];

/// Per-key Hall settings in UI terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyHall {
    pub actuation: u16,
    pub rt: bool,
    pub press: u16,
    pub release: u16,
    /// UI-only: whether press and release are edited independently. The
    /// firmware has no such bit; it is derived as `press != release`.
    pub separate: bool,
    pub whole_travel: bool,
}

impl KeyHall {
    fn from_record(r: &rt::AxisRecord, scale: RtScale) -> Self {
        let hund = |raw: u16, div: u16| ((raw as f64) * 100.0 / div as f64).round() as u16;
        let press = hund(r.press_raw, scale.sensitivity);
        let release = hund(r.release_raw, scale.sensitivity);
        KeyHall {
            actuation: hund(r.actuation_raw, scale.trigger),
            rt: r.rt_enabled(),
            press,
            release,
            separate: press != release,
            whole_travel: r.whole_travel(),
        }
    }

    /// What the firmware would store: RT off means zero sensitivities.
    pub fn firmware_view(&self) -> (u16, u16, u16, bool) {
        if self.rt {
            (self.actuation, self.press, self.release, self.whole_travel)
        } else {
            (self.actuation, 0, 0, self.whole_travel)
        }
    }
}

/// Keyboard-wide settings in UI terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardSettings {
    pub report_rate_hz: Option<u32>,
    pub auto_calibration: bool,
    pub stability: bool,
    pub dead_zones: bool,
    pub top_dead_zone: u16,
    pub bottom_dead_zone: u16,
}

impl BoardSettings {
    fn from_settings(s: &Settings) -> Self {
        BoardSettings {
            report_rate_hz: s.report_rate_hz,
            auto_calibration: s.auto_calibration != 0,
            stability: s.stability_mode != 0,
            dead_zones: s.top_dead_zone > 0 || s.bottom_dead_zone > 0,
            top_dead_zone: s.top_dead_zone as u16,
            bottom_dead_zone: s.bottom_dead_zone as u16,
        }
    }
}

/// One SOCD (Snap Tap) record found on layer 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocdPair {
    pub mode: u8,
    /// HID keyboard usages of the pair (not key indices; PROTOCOL.md §3.3).
    pub usage1: u8,
    pub usage2: u8,
    /// Key indices that carry the record (normally both keys of the pair).
    pub keys: Vec<u8>,
}

impl SocdPair {
    pub fn names(&self) -> (String, String) {
        let n = |u: u8| {
            keyboard_usage_name(u)
                .map(str::to_string)
                .unwrap_or_else(|| format!("0x{u:02X}"))
        };
        (n(self.usage1), n(self.usage2))
    }
}

/// Where the state came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Device,
    Backup(String),
}

/// Raw blocks a [`BoardState`] is decoded from.
pub struct Blocks<'a> {
    pub device_info: &'a [u8],
    pub settings: &'a [u8],
    pub keymap0: &'a [u8],
    pub keymap1: &'a [u8],
    pub fn_factory: &'a [u8],
    pub effect: &'a [u8],
    pub custom: &'a [u8],
    pub axis_rt: &'a [u8],
}

/// Everything the tabs need: decoded values plus the raw writable blocks
/// (`raw`), which previews and writes start from.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardState {
    pub product: String,
    pub firmware: String,
    pub source: Source,
    /// Device-info byte 29; selects the units of the Hall table.
    pub rt_precision: u8,
    pub keys: BTreeMap<u8, KeyHall>,
    pub settings: BoardSettings,
    pub socd: Vec<SocdPair>,
    /// Layer 0 (512 B), read-only in this program.
    pub layer0: Vec<u8>,
    /// The writable blocks, as the CLI's preview code wants them. The
    /// effect colour is known only when `raw.effect_known` is set.
    pub raw: type84_cli::plan::BoardState,
    /// Why the effect colour is unknown (no or stale last-write record).
    pub colour_unknown: Option<String>,
}

impl BoardState {
    pub fn from_blocks(product: &str, source: Source, b: &Blocks) -> Result<Self> {
        let info = DeviceInfo::decode(b.device_info)?;
        let settings = Settings::decode(b.settings)?;
        for (name, blk, size) in [
            ("keymap layer 0", b.keymap0, keymap::SIZE),
            ("Fn layer", b.keymap1, keymap::SIZE),
            ("factory Fn layer", b.fn_factory, keymap::SIZE),
            ("LED effect", b.effect, led::EFFECT_SIZE),
            ("custom LED", b.custom, led::CUSTOM_SIZE),
        ] {
            anyhow::ensure!(
                blk.len() >= size,
                "{name} block too short: {} bytes",
                blk.len()
            );
        }
        Ok(BoardState {
            product: product.to_string(),
            firmware: info.version_string(),
            source,
            rt_precision: info.rt_precision,
            keys: hall_from_block(b.axis_rt, info.rt_precision)?,
            settings: BoardSettings::from_settings(&settings),
            socd: find_socd(&keymap::decode_block(b.keymap0)),
            layer0: b.keymap0[..keymap::SIZE].to_vec(),
            raw: type84_cli::plan::BoardState {
                info,
                hall: b.axis_rt[..rt::SIZE].to_vec(),
                fn_layer: b.keymap1[..keymap::SIZE].to_vec(),
                fn_factory: b.fn_factory[..keymap::SIZE].to_vec(),
                effect: b.effect[..led::EFFECT_SIZE].to_vec(),
                custom: b.custom[..led::CUSTOM_SIZE].to_vec(),
                effect_known: None,
            },
            colour_unknown: Some("нет данных о последней записи".into()),
        })
    }

    pub fn from_snapshot(s: &Snapshot, product: &str) -> Result<Self> {
        Self::from_blocks(
            product,
            Source::Device,
            &Blocks {
                device_info: &s.device_info.data,
                settings: &s.settings.data,
                keymap0: &s.keymap_layer0.data,
                keymap1: &s.keymap_layer1.data,
                fn_factory: &s.default_fn_keymap.data,
                effect: &s.led_effect.data,
                custom: &s.custom_led.data,
                axis_rt: &s.axis_rt.data,
            },
        )
    }

    pub fn from_backup(b: &Backup, file_name: &str) -> Result<Self> {
        let bl = &b.blocks;
        Self::from_blocks(
            b.device.product.as_deref().unwrap_or("IO Type 84 Magnetic"),
            Source::Backup(file_name.to_string()),
            &Blocks {
                device_info: &bl.device_info.bytes()?,
                settings: &bl.settings.bytes()?,
                keymap0: &bl.keymap_layer0.bytes()?,
                keymap1: &bl.keymap_layer1.bytes()?,
                fn_factory: &bl.default_fn_keymap.bytes()?,
                effect: &bl.led_effect.bytes()?,
                custom: &bl.custom_led.bytes()?,
                axis_rt: &bl.axis_rt.bytes()?,
            },
        )
    }

    /// Load a `type84 backup` JSON file (validated by the CLI loader).
    pub fn load_backup(path: &Path) -> Result<Self> {
        let b = Backup::load(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_backup(&b, &name).with_context(|| format!("decoding {}", path.display()))
    }

    /// Take the lighting last-write record from `state_dir` into account.
    pub fn attach_knowledge(&mut self, state_dir: &Path) {
        match type84_cli::apply::attach_knowledge(&mut self.raw, state_dir) {
            type84_cli::lighting_state::Knowledge::Known(_) => self.colour_unknown = None,
            type84_cli::lighting_state::Knowledge::Unknown { reason } => {
                self.colour_unknown = Some(reason)
            }
        }
    }

    /// A newly polled Hall block.
    pub fn set_hall(&mut self, raw: &[u8], keys: BTreeMap<u8, KeyHall>) {
        self.raw.hall = raw.to_vec();
        self.keys = keys;
    }
}

/// Decode the Hall/RT block (1024 B) for every physical key.
pub fn hall_from_block(axis_rt: &[u8], rt_precision: u8) -> Result<BTreeMap<u8, KeyHall>> {
    anyhow::ensure!(
        axis_rt.len() >= rt::SIZE,
        "RT block too short: {} bytes",
        axis_rt.len()
    );
    let scale = RtScale::from_precision(rt_precision);
    let records = rt::decode_block(axis_rt);
    Ok(type84_proto::layout::keys()
        .iter()
        .map(|k| {
            (
                k.index,
                KeyHall::from_record(&records[k.index as usize], scale),
            )
        })
        .collect())
}

/// Collect SOCD records; the same pair is stored on both of its keys.
fn find_socd(actions: &[KeyAction]) -> Vec<SocdPair> {
    let mut pairs: Vec<SocdPair> = Vec::new();
    for (i, a) in actions.iter().enumerate() {
        if let KeyAction::Socd { mode, k1, k2 } = *a {
            if !type84_proto::layout::is_physical(i) {
                continue;
            }
            match pairs
                .iter_mut()
                .find(|p| p.mode == mode && p.usage1 == k1 && p.usage2 == k2)
            {
                Some(p) => p.keys.push(i as u8),
                None => pairs.push(SocdPair {
                    mode,
                    usage1: k1,
                    usage2: k2,
                    keys: vec![i as u8],
                }),
            }
        }
    }
    pairs
}

/// The lighting effect in UI terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Light {
    /// Effect id; the custom effect is shown as 128 whether the board
    /// reports 20 or 128.
    pub mode: u8,
    pub color: [u8; 3],
    pub multi: bool,
    /// Step 0..=5.
    pub brightness: u8,
    /// Step 0..=5.
    pub speed: u8,
    pub reverse: bool,
}

impl Light {
    pub fn from_block(b: &[u8]) -> Self {
        let e = led::LedEffect::decode(b).unwrap_or(led::LedEffect {
            mode: 0,
            color: [0xFF; 3],
            driver_setting: 0,
            secondary_color: [0; 3],
            color_mode: 1,
            brightness: 0,
            speed: 0,
            direction: 0,
            effect_mode_type: 0,
            byte13: 0,
            check_code: [0; 2],
        });
        Light {
            mode: if led::is_custom_mode(e.mode) {
                128
            } else {
                e.mode
            },
            color: e.color,
            multi: e.color_mode != 0,
            brightness: e.brightness.min(5),
            speed: e.speed.min(5),
            reverse: e.direction != 0,
        }
    }

    pub fn is_custom(&self) -> bool {
        led::is_custom_mode(self.mode)
    }
}

/// Which lighting fields differ.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LightChanges {
    pub mode: bool,
    pub color: bool,
    pub multi: bool,
    pub brightness: bool,
    pub speed: bool,
    pub reverse: bool,
}

impl LightChanges {
    pub fn between(a: &Light, b: &Light) -> Self {
        LightChanges {
            mode: !led::same_mode(a.mode, b.mode),
            color: a.color != b.color,
            multi: a.multi != b.multi,
            brightness: a.brightness != b.brightness,
            speed: a.speed != b.speed,
            reverse: a.reverse != b.reverse,
        }
    }

    pub fn any(&self) -> bool {
        self.mode || self.color || self.multi || self.brightness || self.speed || self.reverse
    }
}

fn custom_colours(block: &[u8]) -> BTreeMap<u8, [u8; 3]> {
    layout::keys()
        .iter()
        .map(|k| {
            let i = k.index as usize * 4;
            (k.index, [block[i + 1], block[i + 2], block[i + 3]])
        })
        .collect()
}

fn fn_record(block: &[u8], i: u8) -> [u8; 4] {
    let o = i as usize * 4;
    [block[o], block[o + 1], block[o + 2], block[o + 3]]
}

/// Local edits on top of the last read state, for every writable section.
/// They reach the board only through «Применить»: [`Edits::to_preset`]
/// turns them into a preset of the changed keys and fields, which the
/// worker previews against a fresh read and writes after confirmation.
#[derive(Debug, Clone, PartialEq)]
pub struct Edits {
    pub base_keys: BTreeMap<u8, KeyHall>,
    pub base_settings: BoardSettings,
    pub keys: BTreeMap<u8, KeyHall>,
    /// Keyboard-wide settings are shown read-only (SET_GAME_MODE 0x21 is
    /// not writable); kept for display.
    pub settings: BoardSettings,
    /// Fn layer (512 B) as read, and as edited.
    pub fn_base: Vec<u8>,
    pub fn_layer: Vec<u8>,
    pub fn_factory: Vec<u8>,
    /// Effect as known (colour from the last-write record when fresh).
    pub light_base: Light,
    pub light: Light,
    /// Per-key colours of the physical keys.
    pub custom_base: BTreeMap<u8, [u8; 3]>,
    pub custom: BTreeMap<u8, [u8; 3]>,
}

impl Edits {
    pub fn new(state: &BoardState) -> Self {
        let light = Light::from_block(state.raw.effect_base());
        let custom = custom_colours(&state.raw.custom);
        Edits {
            base_keys: state.keys.clone(),
            base_settings: state.settings,
            keys: state.keys.clone(),
            settings: state.settings,
            fn_base: state.raw.fn_layer.clone(),
            fn_layer: state.raw.fn_layer.clone(),
            fn_factory: state.raw.fn_factory.clone(),
            light_base: light,
            light,
            custom_base: custom.clone(),
            custom,
        }
    }

    fn hall_changed(&self) -> Vec<u8> {
        self.keys
            .iter()
            .filter(|(i, k)| {
                self.base_keys.get(i).map(|b| b.firmware_view()) != Some(k.firmware_view())
            })
            .map(|(i, _)| *i)
            .collect()
    }

    /// Physical keys whose Fn-layer record was edited.
    pub fn fn_changed(&self) -> Vec<u8> {
        layout::keys()
            .iter()
            .map(|k| k.index)
            .filter(|&i| fn_record(&self.fn_base, i) != fn_record(&self.fn_layer, i))
            .collect()
    }

    pub fn custom_changed(&self) -> Vec<u8> {
        self.custom
            .iter()
            .filter(|(i, c)| self.custom_base.get(i) != Some(c))
            .map(|(i, _)| *i)
            .collect()
    }

    pub fn light_changes(&self) -> LightChanges {
        LightChanges::between(&self.light_base, &self.light)
    }

    /// Number of pending changes: one per key whose Hall values, Fn-layer
    /// action or custom colour would change, plus one for the effect.
    /// UI-only toggles ("Раздельно" with equal values) do not count.
    pub fn diff_count(&self) -> usize {
        self.hall_changed().len()
            + self.fn_changed().len()
            + self.custom_changed().len()
            + self.light_changes().any() as usize
    }

    pub fn key_changed(&self, index: u8) -> bool {
        match (self.keys.get(&index), self.base_keys.get(&index)) {
            (Some(k), Some(b)) => k.firmware_view() != b.firmware_view(),
            _ => false,
        }
    }

    /// The Fn-layer record of a key as edited.
    pub fn fn_record(&self, index: u8) -> [u8; 4] {
        fn_record(&self.fn_layer, index)
    }

    pub fn fn_base_record(&self, index: u8) -> [u8; 4] {
        fn_record(&self.fn_base, index)
    }

    pub fn fn_factory_record(&self, index: u8) -> [u8; 4] {
        fn_record(&self.fn_factory, index)
    }

    /// Set the Fn-layer action of a key (the caller checks the lock).
    pub fn set_fn(&mut self, index: u8, action: FnAction) {
        let r = action.record(self.fn_factory_record(index));
        let o = index as usize * 4;
        self.fn_layer[o..o + 4].copy_from_slice(&r);
    }

    /// Take a freshly re-read Hall table as the new base. Keys the user has
    /// not edited follow the board; edited keys keep the user's values.
    /// Returns true when the board's stored values differ from the old base
    /// (e.g. the case actuation switch rewrote the table, PROTOCOL.md §9.1).
    pub fn rebase_keys(&mut self, fresh: BTreeMap<u8, KeyHall>) -> bool {
        let changed = fresh.iter().any(|(i, k)| {
            self.base_keys.get(i).map(|b| b.firmware_view()) != Some(k.firmware_view())
        });
        if !changed {
            return false;
        }
        for (i, k) in &fresh {
            let untouched = self.keys.get(i).map(|e| e.firmware_view())
                == self.base_keys.get(i).map(|b| b.firmware_view());
            if untouched {
                self.keys.insert(*i, *k);
            }
        }
        self.base_keys = fresh;
        true
    }

    pub fn discard(&mut self) {
        self.keys = self.base_keys.clone();
        self.settings = self.base_settings;
        self.fn_layer = self.fn_base.clone();
        self.light = self.light_base;
        self.custom = self.custom_base.clone();
    }

    /// Apply `f` to every selected key.
    pub fn edit_keys(&mut self, selection: &[u8], mut f: impl FnMut(&mut KeyHall)) {
        for i in selection {
            if let Some(k) = self.keys.get_mut(i) {
                f(k);
            }
        }
    }

    /// The pending edits as a preset holding only what changed: Hall
    /// values of edited keys, Fn-layer actions of edited keys, the changed
    /// effect fields and the changed per-key colours. `None` when nothing
    /// changed. Labels come from `type84_proto::layout`, as presets need.
    pub fn to_preset(&self) -> Option<Preset> {
        let mut p = Preset::empty("pending", Some("Keyboard Hub: pending edits".into()))
            .expect("valid name");
        let label = |i: u8| layout::label(i as usize).unwrap_or("?").to_string();
        let hall: Vec<HallKey> = self
            .hall_changed()
            .into_iter()
            .map(|i| {
                let k = self.keys[&i];
                HallKey {
                    index: i,
                    label: label(i),
                    actuation_mm: k.actuation as f64 / 100.0,
                    rt: k.rt.then(|| RtSensitivity {
                        press_mm: k.press as f64 / 100.0,
                        release_mm: k.release as f64 / 100.0,
                    }),
                    whole_travel: k.whole_travel,
                }
            })
            .collect();
        if !hall.is_empty() {
            p.hall = Some(HallSection { keys: hall });
        }
        let fn_keys: Vec<FnLayerKey> = self
            .fn_changed()
            .into_iter()
            .map(|i| {
                let r = self.fn_record(i);
                // Only "Заводское" can produce a record a preset cannot
                // name (a factory vendor function): write it as `factory`.
                let action = FnAction::from_record(r).unwrap_or(FnAction::Factory);
                FnLayerKey {
                    index: i,
                    label: label(i),
                    action,
                    note: None,
                }
            })
            .collect();
        if !fn_keys.is_empty() {
            p.fn_layer = Some(FnLayerSection { keys: fn_keys });
        }
        let ch = self.light_changes();
        let colours = self.custom_changed();
        if ch.any() || !colours.is_empty() {
            let l = &self.light;
            let mut sec = LightingSection::default();
            // Per-key colours show only in custom mode: make sure the
            // effect after the write is custom.
            if !colours.is_empty() && !l.is_custom() {
                sec.effect = Some("custom".into());
            } else if ch.mode || (!colours.is_empty() && !self.light_base.is_custom()) {
                sec.effect = Some(led::effect_name(l.mode).to_string());
            }
            if ch.color {
                sec.color = Some(led::format_hex_color(l.color));
            }
            if ch.color || ch.multi {
                sec.color_mode = Some(if l.multi { "multi" } else { "single" }.into());
            }
            if ch.brightness {
                sec.brightness = Some(l.brightness);
            }
            if ch.speed {
                sec.speed = Some(l.speed);
            }
            if ch.reverse {
                sec.direction = Some(if l.reverse { "reverse" } else { "forward" }.into());
            }
            if !colours.is_empty() {
                sec.keys = Some(
                    colours
                        .into_iter()
                        .map(|i| LightingKey {
                            index: i,
                            label: label(i),
                            color: led::format_hex_color(self.custom[&i]),
                        })
                        .collect(),
                );
            }
            p.lighting = Some(sec);
        }
        (!p.sections().is_empty()).then_some(p)
    }
}

/// Round and clamp a slider value (mm) to hundredths.
pub fn to_hundredths(mm: f64, min: u16, max: u16) -> u16 {
    ((mm * 100.0).round().clamp(min as f64, max as f64)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(flags: u8, act: u16, p: u16, r: u16) -> rt::AxisRecord {
        rt::AxisRecord {
            axis_type: 0,
            flags,
            actuation_raw: act,
            press_raw: p,
            release_raw: r,
        }
    }

    #[test]
    fn key_from_record_uses_scale() {
        let k = KeyHall::from_record(&rec(1, 80, 30, 30), RtScale::from_precision(0));
        assert_eq!(
            (
                k.actuation,
                k.press,
                k.release,
                k.rt,
                k.whole_travel,
                k.separate
            ),
            (80, 30, 30, true, true, false)
        );
        let k = KeyHall::from_record(&rec(0, 80, 300, 450), RtScale::from_precision(1));
        assert_eq!(
            (k.actuation, k.press, k.release, k.separate),
            (80, 30, 45, true)
        );
        let k = KeyHall::from_record(&rec(0, 150, 0, 0), RtScale::from_precision(0));
        assert!(!k.rt);
    }

    #[test]
    fn socd_pairs_are_merged() {
        let mut acts = vec![KeyAction::None; 128];
        acts[49] = KeyAction::Socd {
            mode: 3,
            k1: 4,
            k2: 7,
        };
        acts[51] = KeyAction::Socd {
            mode: 3,
            k1: 4,
            k2: 7,
        };
        let p = find_socd(&acts);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].keys, vec![49, 51]);
        assert_eq!(p[0].names(), ("A".to_string(), "D".to_string()));
    }

    fn fixture_state() -> BoardState {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/type84-20261109-220355-fw1.17.json");
        BoardState::load_backup(&p).unwrap()
    }

    #[test]
    fn diff_counts_firmware_changes_only() {
        let state = fixture_state();
        let mut e = Edits::new(&state);
        assert_eq!(e.diff_count(), 0);
        assert!(e.to_preset().is_none());
        e.edit_keys(&[34], |k| k.separate = true);
        assert_eq!(e.diff_count(), 0, "UI-only toggle");
        e.settings.report_rate_hz = Some(1000);
        assert_eq!(e.diff_count(), 0, "keyboard-wide settings are read-only");
        e.edit_keys(&[0, 34], |k| k.actuation = 100);
        assert_eq!(e.diff_count(), 2);
        assert!(e.key_changed(0));
        e.edit_keys(&[34], |k| k.rt = false);
        assert_eq!(e.diff_count(), 2, "same key");
        e.set_fn(
            1,
            FnAction::Key {
                mods: 0,
                usage: 0x68,
            },
        );
        assert_eq!(e.diff_count(), 3);
        e.light.brightness = 2;
        e.light.speed = 1;
        assert_eq!(e.diff_count(), 4, "the effect counts once");
        e.custom.insert(34, [0xFF, 0, 0]);
        assert_eq!(e.diff_count(), 5);
        let p = e.to_preset().unwrap();
        assert_eq!(p.hall.as_ref().unwrap().keys.len(), 2);
        let w = p
            .hall
            .as_ref()
            .unwrap()
            .keys
            .iter()
            .find(|k| k.index == 34)
            .unwrap();
        assert_eq!((w.label.as_str(), w.actuation_mm, w.rt), ("W", 1.0, None));
        let f = &p.fn_layer.as_ref().unwrap().keys;
        assert_eq!(
            (f.len(), f[0].action.to_string()),
            (1, "key:F13".to_string())
        );
        let l = p.lighting.as_ref().unwrap();
        assert_eq!(
            (l.brightness, l.speed, l.color.as_ref()),
            (Some(2), Some(1), None)
        );
        assert_eq!(l.effect.as_deref(), Some("custom"), "colours force custom");
        assert_eq!(l.keys.as_ref().unwrap().len(), 1);
        // The preset passes the CLI's own validation.
        Preset::from_json(&serde_json::to_string(&p).unwrap()).unwrap();
        e.discard();
        assert_eq!(e.diff_count(), 0);

        // External rewrite (case switch): untouched keys follow, edited keep.
        e.edit_keys(&[0], |k| k.actuation = 100);
        let mut fresh = e.base_keys.clone();
        for k in fresh.values_mut() {
            *k = KeyHall {
                actuation: 200,
                rt: false,
                press: 0,
                release: 0,
                separate: false,
                whole_travel: k.whole_travel,
            };
        }
        assert!(e.rebase_keys(fresh.clone()));
        assert_eq!(e.keys[&34].actuation, 200);
        assert!(!e.keys[&34].rt);
        assert_eq!(e.keys[&0].actuation, 100, "user edit kept");
        assert_eq!(e.diff_count(), 1);
        assert!(!e.rebase_keys(fresh), "same table again is not a change");
    }

    #[test]
    fn colour_pick_sets_single_mode_in_preset() {
        let mut e = Edits::new(&fixture_state());
        assert!(e.light_base.multi, "board reports multicolour");
        e.light.color = [0x22, 0xD0, 0x7C];
        e.light.multi = false;
        let l = e.to_preset().unwrap().lighting.unwrap();
        assert_eq!(l.color.as_deref(), Some("#22D07C"));
        assert_eq!(l.color_mode.as_deref(), Some("single"));
        assert!(l.effect.is_none(), "effect unchanged");
    }

    #[test]
    fn rounding() {
        assert_eq!(to_hundredths(0.804, ACTUATION_MIN, ACTUATION_MAX), 80);
        assert_eq!(to_hundredths(0.0, ACTUATION_MIN, ACTUATION_MAX), 10);
        assert_eq!(to_hundredths(9.0, SENS_MIN, SENS_MAX), 320);
    }
}
