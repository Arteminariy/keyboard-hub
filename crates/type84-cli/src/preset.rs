//! Versioned, sectioned JSON presets.
//!
//! ```json
//! {
//!   "format": "type84-preset",
//!   "format_version": 1,
//!   "name": "cs2",
//!   "created_at": "2026-11-09T22:40:00+03:00",
//!   "source": "backup type84-20261109-220355-fw1.17.json",
//!   "hall": { "keys": [
//!     { "index": 34, "label": "W", "actuation_mm": 0.8,
//!       "rt": { "press_mm": 0.3, "release_mm": 0.3 }, "whole_travel": true },
//!     { "index": 0, "label": "Esc", "actuation_mm": 1.5, "rt": null, "whole_travel": false }
//!   ] }
//! }
//! ```
//!
//! Sections are optional. This build understands:
//! - `hall`: per-key actuation and Rapid Trigger (SET_MAGNETIC_AXIS_RT);
//! - `fn_layer`: per-key Fn-layer actions by label and index, with
//!   human-readable action names (`key:F13`, `key:Ctrl+F10`,
//!   `consumer:PlayPause`, `none`, `factory`; see
//!   `type84_proto::fn_layer::FnAction`) (SET_FN_KEY);
//! - `lighting`: effect, colour, colour mode, brightness, speed, direction
//!   and per-key colours (SET_LED_EFFECT, SET_CUSTOM_LED_DATA).
//!
//! ```json
//! "fn_layer": { "keys": [
//!   { "index": 1, "label": "F1", "action": "consumer:WWWHome", "note": "home" } ] },
//! "lighting": { "effect": "wave", "color": "#00A0FF", "brightness": 4,
//!   "keys": [ { "index": 34, "label": "W", "color": "#FF0000" } ] }
//! ```
//!
//! Any other top-level key is kept as an unknown section: it parses
//! cleanly, is reported, and is never applied. `apply` touches only
//! sections that are present; keys missing from a section are left
//! unchanged, and so are lighting fields that are absent.
//!
//! Values are millimetres; they are converted with the board's current
//! rtPrecision scale when applied. Each key is identified by its index and
//! must carry the matching label from the KLE layout (a guard against
//! applying a preset made for a different layout).

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use type84_proto::fn_layer::FnAction;
use type84_proto::hall::{HallTable, KeyPatch, RtSensitivity};
use type84_proto::layout;
use type84_proto::led::{self, EffectPatch, LedEffect};
use type84_proto::rt::RtScale;

pub const FORMAT: &str = "type84-preset";
pub const FORMAT_VERSION: u32 = 1;

/// One key of the `hall` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HallKey {
    pub index: u8,
    pub label: String,
    pub actuation_mm: f64,
    /// `null` = Rapid Trigger off.
    pub rt: Option<RtSensitivity>,
    pub whole_travel: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HallSection {
    pub keys: Vec<HallKey>,
}

/// Per-key colours: (key index, RGB).
pub type KeyColors = Vec<(u8, [u8; 3])>;

/// One key of the `fn_layer` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FnLayerKey {
    pub index: u8,
    pub label: String,
    pub action: FnAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FnLayerSection {
    pub keys: Vec<FnLayerKey>,
}

/// One per-key colour of the `lighting` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingKey {
    pub index: u8,
    pub label: String,
    /// `#RRGGBB`.
    pub color: String,
}

/// The `lighting` section. Absent fields keep the board's value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LightingSection {
    /// Effect name (`static`, `wave`, `custom`, ...) or id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    /// `#RRGGBB`; selects single-colour mode unless `color_mode` is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// `single` or `multi`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_mode: Option<String>,
    /// Step 0..=5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    /// Step 0..=5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<u8>,
    /// `forward` or `reverse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// Per-key colours; only shown by the `custom` effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<LightingKey>>,
}

/// A known preset section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Hall,
    FnLayer,
    Lighting,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Hall, Section::FnLayer, Section::Lighting];

    pub fn name(self) -> &'static str {
        match self {
            Section::Hall => "hall",
            Section::FnLayer => "fn_layer",
            Section::Lighting => "lighting",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(
            match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
                "hall" | "rt" => Section::Hall,
                "fn_layer" | "fn" => Section::FnLayer,
                "lighting" | "led" => Section::Lighting,
                other => bail!("unknown section {other:?} (use hall, fn_layer or lighting)"),
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub format: String,
    pub format_version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hall: Option<HallSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fn_layer: Option<FnLayerSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<LightingSection>,
    /// Sections this build does not know (kept verbatim, never applied).
    #[serde(flatten)]
    pub unknown_sections: BTreeMap<String, serde_json::Value>,
}

/// Preset names become file names: keep them boring.
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.contains("..");
    if !ok {
        bail!("invalid preset name {name:?}: use letters, digits, '-', '_' and '.' (no path separators, no leading dot)");
    }
    Ok(())
}

/// Default preset directory: `<config dir>/type84/presets`
/// (`~/.config/type84/presets` on Linux, `%APPDATA%\type84\presets` on Windows).
pub fn default_dir() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .context("cannot determine the user config directory; pass --presets-dir")?;
    Ok(base.join("type84").join("presets"))
}

/// A bare name maps to `<dir>/<name>.json`; anything that looks like a path
/// (contains a separator or ends in `.json`) is used as given.
pub fn resolve(name_or_path: &str, dir: &Path) -> Result<PathBuf> {
    if name_or_path.contains('/') || name_or_path.contains('\\') || name_or_path.ends_with(".json")
    {
        return Ok(PathBuf::from(name_or_path));
    }
    validate_name(name_or_path)?;
    Ok(dir.join(format!("{name_or_path}.json")))
}

impl Preset {
    /// Build a preset holding every physical key of a Hall block.
    pub fn from_hall(
        name: &str,
        block: &[u8],
        scale: RtScale,
        source: Option<String>,
    ) -> Result<Self> {
        validate_name(name)?;
        let t = HallTable::from_block(block)?;
        let keys = layout::keys()
            .iter()
            .map(|k| {
                let p = t.key_patch(k.index as usize, scale);
                HallKey {
                    index: k.index,
                    label: k.label.clone(),
                    actuation_mm: p.actuation_mm,
                    rt: p.rt,
                    whole_travel: p.whole_travel,
                }
            })
            .collect();
        Ok(Preset {
            format: FORMAT.into(),
            format_version: FORMAT_VERSION,
            name: name.into(),
            created_at: Some(chrono::Local::now().to_rfc3339()),
            source,
            hall: Some(HallSection { keys }),
            fn_layer: None,
            lighting: None,
            unknown_sections: BTreeMap::new(),
        })
    }

    /// An empty preset (no sections).
    pub fn empty(name: &str, source: Option<String>) -> Result<Self> {
        validate_name(name)?;
        Ok(Preset {
            format: FORMAT.into(),
            format_version: FORMAT_VERSION,
            name: name.into(),
            created_at: Some(chrono::Local::now().to_rfc3339()),
            source,
            hall: None,
            fn_layer: None,
            lighting: None,
            unknown_sections: BTreeMap::new(),
        })
    }

    /// `fn_layer` section holding every physical key whose Fn-layer record
    /// differs from the factory Fn layer. Fails if such a record cannot be
    /// expressed as an action (vendor function, macro, ...).
    pub fn fn_layer_from_blocks(layer: &[u8], factory: &[u8]) -> Result<FnLayerSection> {
        let rec = |b: &[u8], i: usize| [b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]];
        let mut keys = Vec::new();
        for k in layout::keys() {
            let i = k.index as usize;
            let (cur, fac) = (rec(layer, i), rec(factory, i));
            if cur == fac {
                continue;
            }
            let Some(action) = FnAction::from_record(cur) else {
                bail!(
                    "Fn+{} holds {} which a preset cannot express",
                    k.label,
                    type84_proto::fn_layer::describe_record(cur)
                );
            };
            keys.push(FnLayerKey {
                index: k.index,
                label: k.label.clone(),
                action,
                note: None,
            });
        }
        Ok(FnLayerSection { keys })
    }

    /// `lighting` section describing an effect block and, for the custom
    /// effect, every physical key's colour.
    pub fn lighting_from_blocks(effect: &[u8], custom: &[u8]) -> Result<LightingSection> {
        let e = LedEffect::decode(effect).context("LED effect block too short")?;
        let keys = led::is_custom_mode(e.mode).then(|| {
            layout::keys()
                .iter()
                .map(|k| {
                    let i = k.index as usize;
                    LightingKey {
                        index: k.index,
                        label: k.label.clone(),
                        color: led::format_hex_color([
                            custom[i * 4 + 1],
                            custom[i * 4 + 2],
                            custom[i * 4 + 3],
                        ]),
                    }
                })
                .collect()
        });
        Ok(LightingSection {
            effect: Some(led::effect_name(e.mode).to_string()),
            color: Some(led::format_hex_color(e.color)),
            color_mode: Some(if e.color_mode == 0 { "single" } else { "multi" }.into()),
            brightness: Some(e.brightness),
            speed: Some(e.speed),
            direction: Some(
                if e.direction == 0 {
                    "forward"
                } else {
                    "reverse"
                }
                .into(),
            ),
            keys,
        })
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let p: Preset = serde_json::from_str(text)?;
        if p.format != FORMAT {
            bail!("not a type84 preset (format = {:?})", p.format);
        }
        if p.format_version != FORMAT_VERSION {
            bail!(
                "unsupported preset format_version {} (this tool reads {FORMAT_VERSION})",
                p.format_version
            );
        }
        p.hall_patch()?;
        p.fn_patch()?;
        p.lighting_patch()?;
        Ok(p)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_json(&text).with_context(|| format!("loading preset {}", path.display()))
    }

    /// Write to a new file; refuses to overwrite unless `overwrite`.
    pub fn save(&self, path: &Path, overwrite: bool) -> Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let mut opts = OpenOptions::new();
        opts.write(true);
        if overwrite {
            opts.create(true).truncate(true);
        } else {
            opts.create_new(true);
        }
        let mut f = opts.open(path).with_context(|| {
            format!(
                "creating {} (use --force to overwrite an existing preset)",
                path.display()
            )
        })?;
        f.write_all(json.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        Self::load(path)?;
        Ok(())
    }

    /// Known sections present in this preset.
    pub fn sections(&self) -> Vec<Section> {
        Section::ALL
            .into_iter()
            .filter(|s| match s {
                Section::Hall => self.hall.is_some(),
                Section::FnLayer => self.fn_layer.is_some(),
                Section::Lighting => self.lighting.is_some(),
            })
            .collect()
    }

    /// Names of present sections, known and unknown.
    pub fn section_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .sections()
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        v.extend(self.unknown_sections.keys().cloned());
        v
    }

    /// The `fn_layer` section as a typed patch (empty if absent). Checks
    /// index/label agreement and duplicates; the write path checks the rest.
    pub fn fn_patch(&self) -> Result<Vec<(u8, FnAction)>> {
        let Some(f) = &self.fn_layer else {
            return Ok(Vec::new());
        };
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::with_capacity(f.keys.len());
        for k in &f.keys {
            check_key("fn_layer", k.index, &k.label)?;
            if !seen.insert(k.index) {
                bail!(
                    "fn_layer key {} (index {}) is listed twice",
                    k.label,
                    k.index
                );
            }
            if k.index as usize == type84_proto::fn_layer::FN_KEY_INDEX {
                bail!("fn_layer: the Fn key itself cannot be remapped");
            }
            out.push((k.index, k.action));
        }
        Ok(out)
    }

    /// The `lighting` section as an effect patch (None when no effect field
    /// is set) and per-key colours.
    pub fn lighting_patch(&self) -> Result<(Option<EffectPatch>, KeyColors)> {
        let Some(l) = &self.lighting else {
            return Ok((None, Vec::new()));
        };
        let mode = match &l.effect {
            Some(e) => {
                Some(led::effect_id(e).with_context(|| format!("lighting: unknown effect {e:?}"))?)
            }
            None => None,
        };
        let color = match &l.color {
            Some(c) => Some(
                led::parse_hex_color(c)
                    .with_context(|| format!("lighting: colour {c:?} is not #RRGGBB"))?,
            ),
            None => None,
        };
        let color_mode = match l.color_mode.as_deref() {
            None => None,
            Some("single") => Some(0),
            Some("multi") | Some("multicolor") | Some("multicolour") => Some(1),
            Some(o) => bail!("lighting: color_mode {o:?} must be single or multi"),
        };
        let direction = match l.direction.as_deref() {
            None => None,
            Some("forward") => Some(0),
            Some("reverse") => Some(1),
            Some(o) => bail!("lighting: direction {o:?} must be forward or reverse"),
        };
        for (what, v) in [("brightness", l.brightness), ("speed", l.speed)] {
            if v.is_some_and(|v| v > 5) {
                bail!("lighting: {what} must be a step 0..=5");
            }
        }
        let patch = EffectPatch {
            mode,
            color,
            color_mode,
            brightness: l.brightness,
            speed: l.speed,
            direction,
        };
        let mut keys = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for k in l.keys.iter().flatten() {
            check_key("lighting", k.index, &k.label)?;
            if !seen.insert(k.index) {
                bail!(
                    "lighting key {} (index {}) is listed twice",
                    k.label,
                    k.index
                );
            }
            let c = led::parse_hex_color(&k.color).with_context(|| {
                format!(
                    "lighting key {}: colour {:?} is not #RRGGBB",
                    k.label, k.color
                )
            })?;
            keys.push((k.index, c));
        }
        let effect = (patch != EffectPatch::default()).then_some(patch);
        Ok((effect, keys))
    }

    /// The `hall` section as a typed per-key patch (empty if absent).
    /// Checks index/label agreement, duplicates and physical-key-only.
    pub fn hall_patch(&self) -> Result<Vec<(u8, KeyPatch)>> {
        let Some(h) = &self.hall else {
            return Ok(Vec::new());
        };
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::with_capacity(h.keys.len());
        for k in &h.keys {
            let Some(label) = layout::label(k.index as usize) else {
                bail!(
                    "hall key index {} ({:?}) is not a physical key",
                    k.index,
                    k.label
                );
            };
            if label != k.label {
                bail!(
                    "hall key index {} is labelled {:?} in the preset but {:?} in the layout",
                    k.index,
                    k.label,
                    label
                );
            }
            if !seen.insert(k.index) {
                bail!("hall key {} (index {}) is listed twice", k.label, k.index);
            }
            out.push((
                k.index,
                KeyPatch {
                    actuation_mm: k.actuation_mm,
                    rt: k.rt,
                    whole_travel: k.whole_travel,
                },
            ));
        }
        Ok(out)
    }
}

fn check_key(section: &str, index: u8, preset_label: &str) -> Result<()> {
    let Some(label) = layout::label(index as usize) else {
        bail!("{section} key index {index} ({preset_label:?}) is not a physical key");
    };
    if label != preset_label {
        bail!(
            "{section} key index {index} is labelled {preset_label:?} in the preset but {label:?} in the layout"
        );
    }
    Ok(())
}

/// Every `*.json` file in `dir`, sorted.
pub fn list(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str =
        include_str!("../../type84-proto/tests/fixtures/type84-20261109-220355-fw1.17.json");
    const S: RtScale = RtScale {
        trigger: 100,
        sensitivity: 100,
    };

    fn target_block() -> Vec<u8> {
        let v: serde_json::Value = serde_json::from_str(TARGET).unwrap();
        hex::decode(v["blocks"]["axis_rt"]["data"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn from_backup_roundtrips_through_json() {
        let p = Preset::from_hall("cs2", &target_block(), S, Some("test".into())).unwrap();
        let keys = &p.hall.as_ref().unwrap().keys;
        assert_eq!(keys.len(), 84);
        let w = keys.iter().find(|k| k.label == "W").unwrap();
        assert_eq!(w.index, 34);
        assert_eq!(w.actuation_mm, 0.8);
        assert_eq!(
            w.rt,
            Some(RtSensitivity {
                press_mm: 0.3,
                release_mm: 0.3
            })
        );
        assert!(w.whole_travel);
        let esc = keys.iter().find(|k| k.label == "Esc").unwrap();
        assert_eq!(
            (esc.actuation_mm, esc.rt, esc.whole_travel),
            (1.5, None, false)
        );
        let json = serde_json::to_string_pretty(&p).unwrap();
        assert!(json.contains("\"actuation_mm\": 0.8"), "{json}");
        let back = Preset::from_json(&json).unwrap();
        assert_eq!(back, p);
        // Applying the preset to the backup's own table is the identity.
        let t = HallTable::from_block(&target_block()).unwrap();
        assert_eq!(t.apply_patch(&back.hall_patch().unwrap(), S).unwrap(), t);
    }

    #[test]
    fn sections_are_optional_and_unknown_ones_are_kept() {
        let minimal = r#"{"format":"type84-preset","format_version":1,"name":"empty"}"#;
        let p = Preset::from_json(minimal).unwrap();
        assert!(p.hall.is_none());
        assert!(p.hall_patch().unwrap().is_empty());
        assert!(p.section_names().is_empty());

        let future = r#"{"format":"type84-preset","format_version":1,"name":"media",
            "macros":{"slots":[{"id":1,"actions":[]}]},
            "hall":{"keys":[{"index":34,"label":"W","actuation_mm":0.8,"rt":null,"whole_travel":false}]}}"#;
        let p = Preset::from_json(future).unwrap();
        assert_eq!(
            p.section_names(),
            vec!["hall".to_string(), "macros".to_string()]
        );
        assert_eq!(p.hall_patch().unwrap().len(), 1);
        // Unknown sections survive a save/load cycle.
        let again = Preset::from_json(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(again.unknown_sections.contains_key("macros"));
    }

    #[test]
    fn fn_layer_and_lighting_sections() {
        let json = r##"{"format":"type84-preset","format_version":1,"name":"x",
            "fn_layer":{"keys":[
                {"index":1,"label":"F1","action":"consumer:WWWHome","note":"home"},
                {"index":2,"label":"F2","action":"key:Ctrl+F10"},
                {"index":17,"label":"1","action":"key:F13"}]},
            "lighting":{"effect":"wave","color":"#00a0ff","brightness":4,
                "keys":[{"index":34,"label":"W","color":"#FF0000"}]}}"##;
        let p = Preset::from_json(json).unwrap();
        assert_eq!(p.section_names(), vec!["fn_layer", "lighting"]);
        let f = p.fn_patch().unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f[1].1.to_string(), "key:Ctrl+F10");
        let (e, keys) = p.lighting_patch().unwrap();
        let e = e.unwrap();
        assert_eq!(e.mode, Some(11));
        assert_eq!(e.color, Some([0, 0xA0, 0xFF]));
        assert_eq!(e.brightness, Some(4));
        assert_eq!(keys, vec![(34, [0xFF, 0, 0])]);
        // Canonical action names survive a round trip.
        let back = Preset::from_json(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);

        for bad in [
            r#""fn_layer":{"keys":[{"index":1,"label":"F2","action":"key:F13"}]}"#,
            r#""fn_layer":{"keys":[{"index":85,"label":"Fn","action":"key:F13"}]}"#,
            r#""fn_layer":{"keys":[{"index":1,"label":"F1","action":"macro:3"}]}"#,
            r#""lighting":{"effect":"heartbeat"}"#,
            r#""lighting":{"brightness":9}"#,
            r#""lighting":{"keys":[{"index":34,"label":"W","color":"red"}]}"#,
        ] {
            let j = format!(r#"{{"format":"type84-preset","format_version":1,"name":"x",{bad}}}"#);
            assert!(Preset::from_json(&j).is_err(), "{bad}");
        }
    }

    #[test]
    fn sections_from_backup_blocks() {
        let v: serde_json::Value = serde_json::from_str(TARGET).unwrap();
        let b = |n: &str| hex::decode(v["blocks"][n]["data"].as_str().unwrap()).unwrap();
        let f = Preset::fn_layer_from_blocks(&b("keymap_layer1"), &b("default_fn_keymap")).unwrap();
        assert!(f.keys.is_empty(), "the user's Fn layer is the factory one");
        let l = Preset::lighting_from_blocks(&b("led_effect"), &b("custom_led")).unwrap();
        assert_eq!(l.effect.as_deref(), Some("circles"));
        assert_eq!(l.color_mode.as_deref(), Some("multi"));
        assert!(l.keys.is_none());
    }

    #[test]
    fn rejects_bad_presets() {
        let wrong_label = r#"{"format":"type84-preset","format_version":1,"name":"x",
            "hall":{"keys":[{"index":34,"label":"Q","actuation_mm":0.8,"rt":null,"whole_travel":false}]}}"#;
        assert!(Preset::from_json(wrong_label).is_err());
        let non_physical = r#"{"format":"type84-preset","format_version":1,"name":"x",
            "hall":{"keys":[{"index":29,"label":"W","actuation_mm":0.8,"rt":null,"whole_travel":false}]}}"#;
        assert!(Preset::from_json(non_physical).is_err());
        let dup = r#"{"format":"type84-preset","format_version":1,"name":"x",
            "hall":{"keys":[{"index":34,"label":"W","actuation_mm":0.8,"rt":null,"whole_travel":false},
                            {"index":34,"label":"W","actuation_mm":0.9,"rt":null,"whole_travel":false}]}}"#;
        assert!(Preset::from_json(dup).is_err());
        assert!(
            Preset::from_json(r#"{"format":"type84-preset","format_version":2,"name":"x"}"#)
                .is_err()
        );
        assert!(
            Preset::from_json(r#"{"format":"type84-backup","format_version":1,"name":"x"}"#)
                .is_err()
        );
        for bad in ["", "../x", "a/b", ".hidden", "a b", "x..y"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        validate_name("cs2").unwrap();
        validate_name("cs2-v1.0_b").unwrap();
    }

    #[test]
    fn resolve_and_save_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("type84-preset-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(resolve("cs2", &dir).unwrap(), dir.join("cs2.json"));
        assert_eq!(
            resolve("./x.json", &dir).unwrap(),
            PathBuf::from("./x.json")
        );
        let p = Preset::from_hall("cs2", &target_block(), S, None).unwrap();
        let path = resolve("cs2", &dir).unwrap();
        p.save(&path, false).unwrap();
        assert!(p.save(&path, false).is_err(), "must not overwrite");
        p.save(&path, true).unwrap();
        assert_eq!(list(&dir).unwrap(), vec![path.clone()]);
        assert_eq!(Preset::load(&path).unwrap().name, "cs2");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
