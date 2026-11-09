//! Write plans and the confirm-dialog model.
//!
//! A [`PlanRequest`] (pending edits as a preset, saved presets, a backup
//! restore, or the Hall table from before the case switch) is turned into a
//! [`Plan`] against a fresh read of the board by [`build_plan`], using the
//! CLI's preview code (`type84_cli::plan`, `type84_cli::apply`). The
//! dialog shows [`ConfirmModel`], a Russian per-key diff built from the
//! exact blocks the plan would write, never from the UI's own edits.

use std::path::PathBuf;

use anyhow::Result;
use type84_cli::apply;
use type84_cli::backup::Backup;
use type84_cli::plan::{self, BoardState as RawBoard, SectionPreview};
use type84_cli::preset::{Preset, Section};
use type84_hid::WriteStep;
use type84_proto::layout;
use type84_proto::led::{self, LedEffect};
use type84_proto::rt::{AxisRecord, RtScale, RECORD_LEN};
use type84_proto::WriteOp;

use crate::actions;
use crate::i18n::{self, s};

/// Where a plan came from (titles and toasts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOrigin {
    /// «Применить» on the pending bar.
    Pending,
    /// A saved preset.
    Preset(String),
    /// «Восстановить из бэкапа…».
    Restore(PathBuf),
    /// The banner's «Восстановить»: a Hall table from before the case
    /// switch, described for the user.
    HallRestore(String),
}

/// What to plan.
#[derive(Debug, Clone)]
pub enum PlanRequest {
    Presets {
        presets: Vec<Preset>,
        origin: PlanOrigin,
    },
    Restore {
        backup: Box<Backup>,
        path: PathBuf,
        sections: Vec<Section>,
    },
    HallRaw {
        block: Vec<u8>,
        rt_precision: u8,
        origin: PlanOrigin,
    },
}

impl PlanRequest {
    pub fn origin(&self) -> PlanOrigin {
        match self {
            PlanRequest::Presets { origin, .. } | PlanRequest::HallRaw { origin, .. } => {
                origin.clone()
            }
            PlanRequest::Restore { path, .. } => PlanOrigin::Restore(path.clone()),
        }
    }
}

/// A plan ready for confirmation.
#[derive(Debug, Clone)]
pub struct Plan {
    pub origin: PlanOrigin,
    pub previews: Vec<SectionPreview>,
    /// The steps of every changed block (empty: nothing to write).
    pub steps: Vec<WriteStep>,
    pub model: ConfirmModel,
    /// Caveats shown in the dialog (colour unknown, firmware differs, …).
    pub notes: Vec<String>,
}

impl Plan {
    pub fn is_noop(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Plan `req` against `board` (a fresh read with the lighting record
/// attached).
pub fn build_plan(req: &PlanRequest, board: &RawBoard) -> Result<Plan> {
    if !board.info.is_operational() {
        anyhow::bail!(
            "firmwareStatus = {} (firmware busy); refusing",
            board.info.firmware_status
        );
    }
    let mut notes = Vec::new();
    let previews = match req {
        PlanRequest::Presets { presets, .. } => {
            let mut pv = plan::preview_presets(presets, board)?;
            if plan::force_unknown_effect(&mut pv, board) {
                notes.push(s().note_colour_forced.to_string());
            }
            pv
        }
        PlanRequest::Restore {
            backup, sections, ..
        } => {
            if let Some(w) = apply::check_backup_matches_board(backup, board)? {
                notes.push(format!("{} ({w})", s().note_fw_differs));
            }
            let mut rs = Vec::new();
            for sec in sections {
                let r = apply::restore_section(backup, *sec, board)?;
                if !r.notes.is_empty() {
                    notes.push(s().note_custom_not_restored.to_string());
                }
                rs.push(r);
            }
            apply::restore_previews(&rs, board, "backup")
        }
        PlanRequest::HallRaw {
            block,
            rt_precision,
            ..
        } => {
            let step = apply::hall_raw_step(block, *rt_precision, board)?;
            let rs = apply::RestoreSection {
                section: Section::Hall,
                steps: vec![step],
                notes: Vec::new(),
            };
            apply::restore_previews(&[rs], board, "hall")
        }
    };
    let steps = plan::steps(&previews);
    let has_effect = steps.iter().any(|s| s.target.op() == WriteOp::LedEffect);
    if has_effect {
        notes.push(s().note_colour_unverifiable.to_string());
    }
    let model = ConfirmModel::from_previews(&previews, board.scale());
    Ok(Plan {
        origin: req.origin(),
        previews,
        steps,
        model,
        notes,
    })
}

/// One line of the confirm table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub op: WriteOp,
    /// Key or field name.
    pub what: String,
    pub before: String,
    pub after: String,
    /// Colour swatches (per-key colours, effect colour).
    pub colors: Option<([u8; 3], [u8; 3])>,
}

/// The confirm dialog's content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfirmModel {
    pub rows: Vec<DiffRow>,
    /// Per written block: changed physical keys (or fields).
    pub sections: Vec<(WriteOp, usize)>,
    /// Frames the plan writes.
    pub frames: usize,
    /// Changed non-physical Hall/Fn records (backup restores only).
    pub non_physical: usize,
}

/// Section name in the UI.
pub fn op_label(op: WriteOp) -> &'static str {
    match op {
        WriteOp::AxisRt => s().section_hall,
        WriteOp::FnKeymap => s().section_fn,
        WriteOp::LedEffect => s().section_effect,
        WriteOp::CustomLed => s().section_colors,
    }
}

fn key_label(i: usize) -> String {
    if layout::is_physical(i) {
        i18n::key_legend(i as u8).0.to_string()
    } else {
        format!("#{i}")
    }
}

/// One Hall record in words.
pub fn hall_ru(r: &AxisRecord, sc: RtScale) -> String {
    let mm = |v: f64| i18n::mm_comma((v * 100.0).round().max(0.0) as u16);
    let mut out = format!("{} мм", mm(r.actuation_mm(sc)));
    if r.rt_enabled() {
        out.push_str(&format!(
            " · RT {}/{}",
            mm(r.press_mm(sc)),
            mm(r.release_mm(sc))
        ));
        if r.whole_travel() {
            out.push_str(" · весь ход");
        }
    } else {
        out.push_str(" · RT выкл.");
    }
    out
}

fn effect_rows(before: &[u8], after: &[u8], out: &mut Vec<DiffRow>) {
    let (Some(a), Some(b)) = (LedEffect::decode(before), LedEffect::decode(after)) else {
        return;
    };
    let row = |what: &str, x: String, y: String| DiffRow {
        op: WriteOp::LedEffect,
        what: what.to_string(),
        before: x,
        after: y,
        colors: None,
    };
    let n0 = out.len();
    if !led::same_mode(a.mode, b.mode) {
        out.push(row(
            s().light_effect,
            led::effect_label_ru(a.mode).into(),
            led::effect_label_ru(b.mode).into(),
        ));
    }
    if a.color != b.color {
        out.push(DiffRow {
            colors: Some((a.color, b.color)),
            ..row(
                s().light_color,
                led::format_hex_color(a.color),
                led::format_hex_color(b.color),
            )
        });
    }
    let cm = |m: u8| {
        if m == 0 {
            s().light_single
        } else {
            s().light_multi
        }
        .to_string()
    };
    if a.color_mode != b.color_mode {
        out.push(row(
            s().light_color_mode,
            cm(a.color_mode),
            cm(b.color_mode),
        ));
    }
    let step = |v: u8| format!("{v} из 5");
    if a.brightness != b.brightness {
        out.push(row(
            s().light_brightness,
            step(a.brightness),
            step(b.brightness),
        ));
    }
    if a.speed != b.speed {
        out.push(row(s().light_speed, step(a.speed), step(b.speed)));
    }
    let dir = |d: u8| {
        if d == 0 {
            s().light_forward
        } else {
            s().light_reverse
        }
        .to_string()
    };
    if a.direction != b.direction {
        out.push(row(s().light_direction, dir(a.direction), dir(b.direction)));
    }
    if out.len() == n0 {
        // Forced write of an unchanged-looking effect (colour unknown).
        out.push(row(
            s().light_effect,
            led::effect_label_ru(a.mode).into(),
            format!("{} {}", led::effect_label_ru(b.mode), s().rewrite_same),
        ));
    }
}

impl ConfirmModel {
    pub fn from_previews(previews: &[SectionPreview], sc: RtScale) -> Self {
        let mut m = ConfirmModel::default();
        for b in previews.iter().flat_map(|p| p.blocks.iter()) {
            if b.changed.is_empty() {
                continue;
            }
            m.frames += b.op.frame_count();
            let n0 = m.rows.len();
            match b.op {
                WriteOp::AxisRt => {
                    let rec = |x: &[u8], i: usize| {
                        AxisRecord::decode(&x[i * RECORD_LEN..(i + 1) * RECORD_LEN])
                    };
                    for k in layout::keys() {
                        let i = k.index as usize;
                        let (x, y) = (rec(&b.before, i), rec(&b.after, i));
                        if x != y {
                            m.rows.push(DiffRow {
                                op: b.op,
                                what: key_label(i),
                                before: hall_ru(&x, sc),
                                after: hall_ru(&y, sc),
                                colors: None,
                            });
                        }
                    }
                    m.non_physical += (0..128)
                        .filter(|&i| {
                            !layout::is_physical(i) && rec(&b.before, i) != rec(&b.after, i)
                        })
                        .count();
                }
                WriteOp::FnKeymap => {
                    let rec =
                        |x: &[u8], i: usize| [x[i * 4], x[i * 4 + 1], x[i * 4 + 2], x[i * 4 + 3]];
                    for k in layout::keys() {
                        let i = k.index as usize;
                        let (x, y) = (rec(&b.before, i), rec(&b.after, i));
                        if x != y {
                            m.rows.push(DiffRow {
                                op: b.op,
                                what: format!("Fn+{}", key_label(i)),
                                before: actions::record_label(x),
                                after: actions::record_label(y),
                                colors: None,
                            });
                        }
                    }
                    m.non_physical += (0..128)
                        .filter(|&i| {
                            !layout::is_physical(i) && rec(&b.before, i) != rec(&b.after, i)
                        })
                        .count();
                }
                WriteOp::LedEffect => effect_rows(&b.before, &b.after, &mut m.rows),
                WriteOp::CustomLed => {
                    let c = |x: &[u8], i: usize| [x[i * 4 + 1], x[i * 4 + 2], x[i * 4 + 3]];
                    for k in layout::keys() {
                        let i = k.index as usize;
                        let (x, y) = (c(&b.before, i), c(&b.after, i));
                        if x != y {
                            m.rows.push(DiffRow {
                                op: b.op,
                                what: key_label(i),
                                before: led::format_hex_color(x),
                                after: led::format_hex_color(y),
                                colors: Some((x, y)),
                            });
                        }
                    }
                }
            }
            m.sections.push((b.op, m.rows.len() - n0));
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BoardState, Edits};
    use type84_proto::fn_layer::FnAction;

    fn state() -> BoardState {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/type84-20261109-220355-fw1.17.json");
        BoardState::load_backup(&p).unwrap()
    }

    fn pending(e: &Edits) -> PlanRequest {
        PlanRequest::Presets {
            presets: vec![e.to_preset().unwrap()],
            origin: PlanOrigin::Pending,
        }
    }

    #[test]
    fn n_edited_keys_give_n_rows_and_one_hall_step() {
        let st = state();
        let mut e = Edits::new(&st);
        e.edit_keys(&[0, 1, 2], |k| k.actuation = 100);
        let plan = build_plan(&pending(&e), &st.raw).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].target.op(), WriteOp::AxisRt);
        assert_eq!(plan.model.rows.len(), 3);
        assert_eq!(plan.model.sections, vec![(WriteOp::AxisRt, 3)]);
        assert_eq!(plan.model.frames, 19);
        let esc = &plan.model.rows[0];
        assert_eq!(esc.what, "Esc");
        assert_eq!(esc.before, "1,50 мм · RT выкл.");
        assert_eq!(esc.after, "1,00 мм · RT выкл.");
        assert!(plan.notes.is_empty());
    }

    #[test]
    fn combined_sections_in_one_plan() {
        let st = state();
        let mut e = Edits::new(&st);
        e.set_fn(
            2,
            FnAction::Key {
                mods: 0,
                usage: 0x68,
            },
        );
        e.set_fn(1, FnAction::Consumer(0xCD));
        e.light.brightness = 2;
        let plan = build_plan(&pending(&e), &st.raw).unwrap();
        let ops: Vec<WriteOp> = plan.steps.iter().map(|s| s.target.op()).collect();
        assert!(ops.contains(&WriteOp::FnKeymap) && ops.contains(&WriteOp::LedEffect));
        let fn_rows: Vec<&DiffRow> = plan
            .model
            .rows
            .iter()
            .filter(|r| r.op == WriteOp::FnKeymap)
            .collect();
        assert_eq!(fn_rows.len(), 2, "{fn_rows:?}");
        assert_eq!(fn_rows[0].what, "Fn+F1");
        assert_eq!(fn_rows[0].before, "не назначено");
        assert_eq!(fn_rows[0].after, "медиа: Воспр./пауза");
        assert_eq!(fn_rows[1].after, "клавиша F13");
        assert_eq!(plan.model.non_physical, 0);
        assert!(
            plan.notes.iter().any(|n| n.contains("Цвет")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn protected_fn_key_is_refused_by_the_plan() {
        let st = state();
        let mut e = Edits::new(&st);
        // Bypass the UI lock: Fn+Delete holds the vendor reset function.
        e.set_fn(
            106,
            FnAction::Key {
                mods: 0,
                usage: 0x68,
            },
        );
        let err = build_plan(&pending(&e), &st.raw).unwrap_err();
        assert!(format!("{err:#}").contains("Delete"), "{err:#}");
    }

    #[test]
    fn hall_restore_of_current_table_is_noop() {
        let st = state();
        let req = PlanRequest::HallRaw {
            block: st.raw.hall.clone(),
            rt_precision: st.rt_precision,
            origin: PlanOrigin::HallRestore("x".into()),
        };
        let plan = build_plan(&req, &st.raw).unwrap();
        assert!(plan.is_noop());
        assert!(plan.model.rows.is_empty());
    }
}
