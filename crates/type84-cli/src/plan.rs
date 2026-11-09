//! Turning presets into write plans, previews and "does the board match"
//! checks. Pure functions over blocks read through the read-only transport;
//! the write itself happens only in `type84_hid::WriteSession::apply_plan`.

use anyhow::{bail, Context, Result};
use type84_hid::{
    CustomLedTarget, EffectTarget, FnLayerTarget, HallTarget, WriteOptIn, WriteStep, WriteTarget,
};
use type84_proto::device_info::DeviceInfo;
use type84_proto::fn_layer;
use type84_proto::hall::HallTable;
use type84_proto::layout;
use type84_proto::led;
use type84_proto::rt::RtScale;
use type84_proto::{ReadOp, WriteOp};

use crate::preset::{Preset, Section};
use crate::snapshot::Reader;
use crate::write::hall_diff;

/// The blocks a preset can touch, read from the board.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardState {
    pub info: DeviceInfo,
    pub hall: Vec<u8>,
    pub fn_layer: Vec<u8>,
    pub fn_factory: Vec<u8>,
    pub effect: Vec<u8>,
    pub custom: Vec<u8>,
    /// `effect` with the colour and colour mode of this tool's last write
    /// (`lighting_state`), when that record still matches the readback.
    /// `None`: the colour is unknown and diffs use the readback.
    pub effect_known: Option<Vec<u8>>,
}

impl BoardState {
    /// Read device info and every writable block (about 50 read packets).
    pub fn read(r: &mut Reader) -> Result<Self> {
        let (info, _) = r.device_info()?;
        let mut block = |op: ReadOp| -> Result<Vec<u8>> {
            Ok(r.read_full(op)
                .with_context(|| format!("{} failed", op.name()))?
                .data)
        };
        Ok(BoardState {
            info,
            hall: block(ReadOp::AxisRt)?,
            fn_layer: block(ReadOp::FnKeymap)?,
            fn_factory: block(ReadOp::DefaultFnKeymap)?,
            effect: block(ReadOp::LedEffect)?,
            custom: block(ReadOp::CustomLed)?,
            effect_known: None,
        })
    }

    /// The effect as far as it is known: [`Self::effect_known`] or the
    /// readback.
    pub fn effect_base(&self) -> &[u8] {
        self.effect_known.as_deref().unwrap_or(&self.effect)
    }

    pub fn scale(&self) -> RtScale {
        RtScale::from_precision(self.info.rt_precision)
    }

    pub fn block(&self, op: WriteOp) -> &[u8] {
        match op {
            WriteOp::AxisRt => &self.hall,
            WriteOp::FnKeymap => &self.fn_layer,
            WriteOp::LedEffect => &self.effect,
            WriteOp::CustomLed => &self.custom,
        }
    }
}

/// One block a section would change.
#[derive(Debug, Clone)]
pub struct BlockChange {
    pub op: WriteOp,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub changed: Vec<usize>,
    pub step: WriteStep,
}

/// What applying one section would do.
#[derive(Debug, Clone)]
pub struct SectionPreview {
    pub section: Section,
    /// The preset that provided the section.
    pub preset: String,
    pub blocks: Vec<BlockChange>,
}

impl SectionPreview {
    pub fn is_noop(&self) -> bool {
        self.blocks.iter().all(|b| b.changed.is_empty())
    }

    /// Human-readable diff lines (`label: old → new`).
    pub fn lines(&self, scale: RtScale) -> Vec<String> {
        let mut out = Vec::new();
        for b in &self.blocks {
            if b.changed.is_empty() {
                continue;
            }
            match b.op {
                WriteOp::AxisRt => {
                    let d = hall_diff(&b.before, &b.after, scale);
                    out.extend(d.lines);
                    if !d.non_physical_changed.is_empty() {
                        out.push(format!(
                            "  plus {} non-physical record(s) (indices {:?}); they are not keys",
                            d.non_physical_changed.len(),
                            d.non_physical_changed
                        ));
                    }
                }
                WriteOp::FnKeymap => out.extend(fn_layer::diff_lines(&b.before, &b.after)),
                WriteOp::LedEffect => out.push(format!(
                    "  effect:        {} → {}",
                    led::describe_effect(&b.before),
                    led::describe_effect(&b.after)
                )),
                WriteOp::CustomLed => {
                    for &i in &b.changed {
                        let c = |x: &[u8]| {
                            led::format_hex_color([x[i * 4 + 1], x[i * 4 + 2], x[i * 4 + 3]])
                        };
                        let name = layout::label(i)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("index {i}"));
                        out.push(format!(
                            "  {:<14} {} → {}",
                            format!("{name} colour:"),
                            c(&b.before),
                            c(&b.after)
                        ));
                    }
                }
            }
        }
        out
    }

    /// Frames written by this section (full blocks of every changed block).
    pub fn frames(&self) -> usize {
        self.blocks
            .iter()
            .filter(|b| !b.changed.is_empty())
            .map(|b| b.op.frame_count())
            .sum()
    }
}

fn change(op: WriteOp, board: &BoardState, after: Vec<u8>, target: WriteTarget) -> BlockChange {
    let before = board.block(op).to_vec();
    let changed = type84_hid::write::changed_records(op, &before, &after);
    BlockChange {
        op,
        step: WriteStep {
            target,
            expected_base: Some(before.clone()),
        },
        before,
        after,
        changed,
    }
}

/// Preview one section of a preset against the board.
pub fn preview_section(p: &Preset, section: Section, board: &BoardState) -> Result<SectionPreview> {
    let mut blocks = Vec::new();
    match section {
        Section::Hall => {
            let patch = p.hall_patch()?;
            let base = HallTable::from_block(&board.hall)?;
            let new = base.apply_patch(&patch, board.scale())?;
            blocks.push(change(
                WriteOp::AxisRt,
                board,
                new.to_block().to_vec(),
                WriteTarget::Hall(HallTarget::Patch(patch)),
            ));
        }
        Section::FnLayer => {
            let patch = p.fn_patch()?;
            let new = fn_layer::apply_patch(&board.fn_layer, &board.fn_factory, &patch)?;
            blocks.push(change(
                WriteOp::FnKeymap,
                board,
                new,
                WriteTarget::FnLayer(FnLayerTarget::Patch(patch)),
            ));
        }
        Section::Lighting => {
            let (effect, keys) = p.lighting_patch()?;
            let mut final_mode = board.effect[0];
            if let Some(e) = effect {
                // Diff against the known effect (with the colour of the last
                // write); the write is forced, because the readback the
                // session compares against never shows the colour.
                let base = board.effect_base().to_vec();
                let new = e.apply(&base)?.to_vec();
                final_mode = new[0];
                let changed = type84_hid::write::changed_records(WriteOp::LedEffect, &base, &new);
                blocks.push(BlockChange {
                    op: WriteOp::LedEffect,
                    step: WriteStep {
                        target: WriteTarget::LedEffect(EffectTarget::Forced(new.clone())),
                        expected_base: Some(board.effect.clone()),
                    },
                    before: base,
                    after: new,
                    changed,
                });
            }
            if !keys.is_empty() {
                let new = led::apply_custom_patch(&board.custom, &keys)?;
                blocks.push(change(
                    WriteOp::CustomLed,
                    board,
                    new,
                    WriteTarget::CustomLed(CustomLedTarget::Patch(keys)),
                ));
            }
            let colours_change = blocks
                .iter()
                .any(|b| b.op == WriteOp::CustomLed && !b.changed.is_empty());
            if colours_change && !led::is_custom_mode(final_mode) {
                bail!(
                    "lighting: per-key colours need the custom effect, but the effect would be {} (mode {final_mode}); add \"effect\": \"custom\"",
                    led::effect_name(final_mode)
                );
            }
        }
    }
    Ok(SectionPreview {
        section,
        preset: p.name.clone(),
        blocks,
    })
}

/// Preview every known section of several presets. Two presets defining
/// the same section is an error (combine presets that cover different
/// sections, e.g. `cs2 fn-fbx50c`).
pub fn preview_presets(presets: &[Preset], board: &BoardState) -> Result<Vec<SectionPreview>> {
    let mut owner: std::collections::BTreeMap<Section, &str> = Default::default();
    let mut out = Vec::new();
    for p in presets {
        for s in p.sections() {
            if let Some(other) = owner.insert(s, &p.name) {
                bail!(
                    "section {} is defined by both {other:?} and {:?}; combine presets that cover different sections",
                    s.name(),
                    p.name
                );
            }
            out.push(preview_section(p, s, board)?);
        }
    }
    out.sort_by_key(|s| s.section);
    Ok(out)
}

/// With the colour unknown (no `effect_known`), an effect block that looks
/// unchanged is written anyway: the readback cannot show whether the colour
/// matches. Returns whether a block was forced.
pub fn force_unknown_effect(previews: &mut [SectionPreview], board: &BoardState) -> bool {
    if board.effect_known.is_some() {
        return false;
    }
    let mut forced = false;
    for b in previews.iter_mut().flat_map(|s| s.blocks.iter_mut()) {
        if b.op == WriteOp::LedEffect && b.changed.is_empty() {
            b.changed = vec![0];
            forced = true;
        }
    }
    forced
}

/// Write steps for every block that changes.
pub fn steps(previews: &[SectionPreview]) -> Vec<WriteStep> {
    previews
        .iter()
        .flat_map(|s| s.blocks.iter())
        .filter(|b| !b.changed.is_empty())
        .map(|b| b.step.clone())
        .collect()
}

/// The opt-in token covering exactly the sections of `steps`.
pub fn opt_in_for(steps: &[WriteStep]) -> Option<WriteOptIn> {
    let mut out: Option<WriteOptIn> = None;
    for s in steps {
        let t = match s.target.op() {
            WriteOp::AxisRt => WriteOptIn::user_confirmed_hall_write(),
            WriteOp::FnKeymap => WriteOptIn::user_confirmed_fn_layer_write(),
            WriteOp::LedEffect | WriteOp::CustomLed => WriteOptIn::user_confirmed_lighting_write(),
        };
        out = Some(match out {
            Some(o) => o.and(t),
            None => t,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str =
        include_str!("../../type84-proto/tests/fixtures/type84-20261109-220355-fw1.17.json");

    fn board() -> BoardState {
        let v: serde_json::Value = serde_json::from_str(TARGET).unwrap();
        let b = |n: &str| hex::decode(v["blocks"][n]["data"].as_str().unwrap()).unwrap();
        BoardState {
            info: DeviceInfo::decode(&b("device_info")).unwrap(),
            hall: b("axis_rt"),
            fn_layer: b("keymap_layer1"),
            fn_factory: b("default_fn_keymap"),
            effect: b("led_effect"),
            custom: b("custom_led"),
            effect_known: None,
        }
    }

    fn preset(json: &str) -> Preset {
        Preset::from_json(json).unwrap()
    }

    #[test]
    fn fn_layer_preview_and_steps() {
        let b = board();
        let p = preset(
            r#"{"format":"type84-preset","format_version":1,"name":"fn",
            "fn_layer":{"keys":[{"index":2,"label":"F2","action":"key:Ctrl+F10"},
                                {"index":17,"label":"1","action":"key:F13"}]}}"#,
        );
        let pv = preview_presets(std::slice::from_ref(&p), &b).unwrap();
        assert_eq!(pv.len(), 1);
        assert_eq!(pv[0].frames(), 10);
        let lines = pv[0].lines(b.scale());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("Fn+F2:") && lines[0].contains("key:Ctrl+F10"));
        let st = steps(&pv);
        assert_eq!(st.len(), 1);
        assert!(opt_in_for(&st).unwrap().allows(WriteOp::FnKeymap));
        assert!(!opt_in_for(&st).unwrap().allows(WriteOp::AxisRt));
        // A second preset with the same section is refused.
        assert!(preview_presets(&[p.clone(), p], &b).is_err());
    }

    #[test]
    fn lighting_preview_checks_custom_mode() {
        let b = board();
        let keys_only = preset(
            r##"{"format":"type84-preset","format_version":1,"name":"l",
            "lighting":{"keys":[{"index":34,"label":"W","color":"#FF0000"}]}}"##,
        );
        assert!(
            preview_presets(&[keys_only], &b).is_err(),
            "board is in circles mode"
        );
        let ok = preset(
            r##"{"format":"type84-preset","format_version":1,"name":"l",
            "lighting":{"effect":"custom","keys":[{"index":34,"label":"W","color":"#FF0000"}]}}"##,
        );
        let pv = preview_presets(&[ok], &b).unwrap();
        assert_eq!(pv[0].frames(), 11);
        let st = steps(&pv);
        assert_eq!(st.len(), 2);
        // Current board lighting as a preset is a no-op.
        let cur = Preset {
            lighting: Some(Preset::lighting_from_blocks(&b.effect, &b.custom).unwrap()),
            ..Preset::empty("cur", None).unwrap()
        };
        assert!(preview_presets(std::slice::from_ref(&cur), &b).unwrap()[0].is_noop());
        // ... but with the colour unknown, applying it still writes the effect.
        let mut pv = preview_presets(&[cur], &b).unwrap();
        assert!(force_unknown_effect(&mut pv, &b));
        assert_eq!(steps(&pv).len(), 1);
    }

    #[test]
    fn colour_diff_uses_last_written_colour() {
        let mut b = board();
        let red = preset(
            r##"{"format":"type84-preset","format_version":1,"name":"r",
            "lighting":{"effect":"circles","color":"#FF3B4E","brightness":5}}"##,
        );
        // The board reports white multicolour; the record says the last write
        // was #FF3B4E single colour: nothing to write.
        let mut known = b.effect.clone();
        known[1..4].copy_from_slice(&[0xFF, 0x3B, 0x4E]);
        known[8] = 0;
        known[9] = 5;
        b.effect[9] = 5;
        b.effect_known = Some(known.clone());
        let mut pv = preview_presets(std::slice::from_ref(&red), &b).unwrap();
        assert!(pv[0].is_noop());
        assert!(!force_unknown_effect(&mut pv, &b));
        // Last write was another colour: the effect is written, as a forced
        // block checked against the raw readback.
        known[1..4].copy_from_slice(&[0x22, 0xD0, 0x7C]);
        b.effect_known = Some(known);
        let pv = preview_presets(&[red], &b).unwrap();
        let st = steps(&pv);
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].expected_base.as_deref(), Some(&b.effect[..]));
        match &st[0].target {
            WriteTarget::LedEffect(EffectTarget::Forced(blk)) => {
                assert_eq!(&blk[..4], &[15, 0xFF, 0x3B, 0x4E]);
                assert_eq!(blk[8], 0);
            }
            t => panic!("unexpected {t:?}"),
        }
    }
}
