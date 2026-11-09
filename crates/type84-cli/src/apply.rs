//! The guarded write pipeline shared by the CLI and the GUI: the write
//! lock, the lighting-colour record, restore steps from a backup, presets
//! built from the board, and the run of a plan through
//! `type84_hid::WriteSession::apply_plan` with the automatic pre-write
//! backup. Nothing here can write except [`run_steps`], which only calls
//! `apply_plan` on a session the caller opened with an explicit opt-in.
//!
//! Every path (state dir, backup dir) is a parameter so tests never touch
//! the user's real files.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use type84_hid::write::StepReport;
use type84_hid::{
    ApplyError, CustomLedTarget, EffectTarget, FnLayerTarget, HallTarget, RawHid, WriteSession,
    WriteStep, WriteTarget,
};
use type84_proto::fn_layer;
use type84_proto::hall::{self, HallTable, NonPhysicalPolicy};
use type84_proto::led;
use type84_proto::{ReadOp, WriteOp};

use crate::anim;
use crate::backup::Backup;
use crate::lighting_state::{self, Knowledge};
use crate::plan::{BlockChange, BoardState, SectionPreview};
use crate::preset::{Preset, Section};
use crate::write;

/// Message of the error returned when another process holds the write lock.
pub const LOCK_HELD: &str =
    "another type84 process is writing to the keyboard (the write lock is held); nothing written";

/// Take the exclusive write lock (`<state dir>/anim.lock`, the lock of
/// `type84 anim` and `--flash`) without waiting. Fails with [`LOCK_HELD`]
/// when another process holds it.
pub fn write_lock(state_dir: &Path) -> Result<anim::AnimLock> {
    match anim::try_lock(state_dir)? {
        Some(l) => Ok(l),
        None => bail!(LOCK_HELD),
    }
}

/// Take the write lock, waiting up to `timeout` for another process (for
/// example the GUI's short Hall poll) to release it. Fails with
/// [`LOCK_HELD`] after that.
pub fn write_lock_wait(state_dir: &Path, timeout: std::time::Duration) -> Result<anim::AnimLock> {
    let end = std::time::Instant::now() + timeout;
    loop {
        if let Some(l) = anim::try_lock(state_dir)? {
            return Ok(l);
        }
        if std::time::Instant::now() >= end {
            bail!(LOCK_HELD);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// How long CLI writes wait for the lock.
pub const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether another process holds the write lock right now. Takes and drops
/// the lock, so it never blocks anybody for longer than the probe.
pub fn lock_held(state_dir: &Path) -> bool {
    matches!(anim::try_lock(state_dir), Ok(None))
}

/// What is known about the effect block `read` (the colour is not reported
/// by the board; see `lighting_state`).
pub fn effect_knowledge(state_dir: &Path, read: &[u8]) -> Knowledge {
    lighting_state::knowledge(&lighting_state::path(state_dir), read)
}

/// Fill `board.effect_known` from the lighting record in `state_dir`.
pub fn attach_knowledge(board: &mut BoardState, state_dir: &Path) -> Knowledge {
    let k = effect_knowledge(state_dir, &board.effect);
    board.effect_known = k.known().map(<[u8]>::to_vec);
    k
}

/// Outcome of a successful [`run_steps`].
#[derive(Debug, Clone)]
pub struct Applied {
    pub backup_path: PathBuf,
    pub steps: Vec<StepReport>,
    /// The board already held everything (`ApplyError::NoChanges`).
    pub nothing_to_write: bool,
    /// Saving the lighting record failed (the write itself succeeded).
    pub record_warning: Option<String>,
}

/// Run `steps` on an open session: automatic full pre-write backup into
/// `backup_dir`, the guarded sequence of `apply_plan`, then the lighting
/// record in `state_dir`. The caller holds the write lock.
pub fn run_steps<D: RawHid>(
    session: &mut WriteSession<D>,
    iface: &type84_hid::Interface,
    steps: &[WriteStep],
    backup_dir: &Path,
    state_dir: &Path,
    verbose: bool,
) -> Result<Applied, ApplyError> {
    let mut backup_step = write::auto_backup(iface, backup_dir, verbose);
    match session.apply_plan(steps, &mut backup_step) {
        Ok(rep) => {
            let record_warning =
                lighting_state::record(&lighting_state::path(state_dir), &rep.steps)
                    .err()
                    .map(|e| format!("{e:#}"));
            Ok(Applied {
                backup_path: rep.backup_path,
                steps: rep.steps,
                nothing_to_write: false,
                record_warning,
            })
        }
        Err(ApplyError::NoChanges { backup }) => Ok(Applied {
            backup_path: backup,
            steps: Vec::new(),
            nothing_to_write: true,
            record_warning: None,
        }),
        Err(e) => Err(e),
    }
}

/// Refuse a restore from a backup of another device or onto a busy board.
/// Returns a warning when the firmware versions differ.
pub fn check_backup_matches_board(b: &Backup, board: &BoardState) -> Result<Option<String>> {
    let (bi, ni) = (&b.device.info, &board.info);
    if (bi.vendor_id, bi.product_id) != (ni.vendor_id, ni.product_id) {
        bail!(
            "backup is from {:04x}:{:04x}, board is {:04x}:{:04x}",
            bi.vendor_id,
            bi.product_id,
            ni.vendor_id,
            ni.product_id
        );
    }
    if !ni.is_operational() {
        bail!(
            "firmwareStatus = {} (firmware busy); refusing",
            ni.firmware_status
        );
    }
    Ok((bi.version_hundredths != ni.version_hundredths).then(|| {
        format!(
            "backup firmware {} differs from board firmware {}",
            bi.version_string(),
            ni.version_string()
        )
    }))
}

/// One section of a raw restore: the write steps and the blocks they
/// would leave on the board.
#[derive(Debug, Clone)]
pub struct RestoreSection {
    pub section: Section,
    pub steps: Vec<(WriteStep, Vec<u8>)>,
    /// E.g. that per-key colours are not restored outside custom mode.
    pub notes: Vec<String>,
}

/// Restore steps for one section of a backup, validated against `board`
/// exactly as `type84 restore --only` does.
pub fn restore_section(b: &Backup, section: Section, board: &BoardState) -> Result<RestoreSection> {
    let mut steps: Vec<(WriteStep, Vec<u8>)> = Vec::new();
    let mut notes = Vec::new();
    match section {
        Section::Hall => {
            let block = b.blocks.axis_rt.bytes()?;
            steps.push(hall_raw_step(&block, b.device.info.rt_precision, board)?);
        }
        Section::FnLayer => {
            let block = b.blocks.keymap_layer1.bytes()?;
            fn_layer::validate_write(&board.fn_layer, &block)?;
            steps.push((
                WriteStep {
                    target: WriteTarget::FnLayer(FnLayerTarget::Raw(block.clone())),
                    expected_base: Some(board.fn_layer.clone()),
                },
                block,
            ));
        }
        Section::Lighting => {
            let effect = b.blocks.led_effect.bytes()?;
            let custom = b.blocks.custom_led.bytes()?;
            let eff = led::normalise_effect(&effect)?.to_vec();
            if led::is_custom_mode(effect[0]) {
                steps.push((
                    WriteStep {
                        target: WriteTarget::CustomLed(CustomLedTarget::Raw(custom.clone())),
                        expected_base: Some(board.custom.clone()),
                    },
                    led::normalise_custom(&custom)?,
                ));
            } else {
                notes.push(format!(
                    "the backup's effect ({}) does not show per-key colours; they are not restored.",
                    led::effect_name(effect[0])
                ));
            }
            steps.push((
                WriteStep {
                    target: WriteTarget::LedEffect(EffectTarget::Raw(effect)),
                    expected_base: Some(board.effect.clone()),
                },
                eff,
            ));
        }
    }
    Ok(RestoreSection {
        section,
        steps,
        notes,
    })
}

/// A raw Hall restore of `block` (taken with `rt_precision`), validated
/// against the board like `type84 restore --only hall`.
pub fn hall_raw_step(
    block: &[u8],
    rt_precision: u8,
    board: &BoardState,
) -> Result<(WriteStep, Vec<u8>)> {
    if rt_precision != board.info.rt_precision {
        bail!(
            "backup rtPrecision {} differs from the board's {}; raw Hall units differ, refusing",
            rt_precision,
            board.info.rt_precision
        );
    }
    let base = HallTable::from_block(&board.hall)?;
    let target = HallTable::from_block(block)?;
    hall::validate_write(
        &base,
        &target,
        board.scale(),
        NonPhysicalPolicy::AllowFromBackup,
    )?;
    Ok((
        WriteStep {
            target: WriteTarget::Hall(HallTarget::Raw {
                block: block.to_vec(),
                rt_precision,
            }),
            expected_base: Some(board.hall.clone()),
        },
        block.to_vec(),
    ))
}

/// Turn restore steps into previews (diff against `board`), so restores and
/// preset applies share one diff and confirm path.
pub fn restore_previews(
    sections: &[RestoreSection],
    board: &BoardState,
    source: &str,
) -> Vec<SectionPreview> {
    sections
        .iter()
        .map(|s| SectionPreview {
            section: s.section,
            preset: source.to_string(),
            blocks: s
                .steps
                .iter()
                .map(|(step, after)| {
                    let op = step.target.op();
                    let before = board.block(op).to_vec();
                    let changed = type84_hid::write::changed_records(op, &before, after);
                    BlockChange {
                        op,
                        before,
                        after: after.clone(),
                        changed,
                        step: step.clone(),
                    }
                })
                .collect(),
        })
        .collect()
}

/// Build a preset holding `sections` of the blocks given by `blocks`.
pub fn build_preset(
    name: &str,
    sections: &[Section],
    blocks: &dyn Fn(ReadOp) -> Result<Vec<u8>>,
    scale: type84_proto::rt::RtScale,
    source: String,
) -> Result<Preset> {
    let mut p = Preset::empty(name, Some(source))?;
    for s in sections {
        match s {
            Section::Hall => {
                p.hall = Preset::from_hall(name, &blocks(ReadOp::AxisRt)?, scale, None)?.hall;
            }
            Section::FnLayer => {
                p.fn_layer = Some(Preset::fn_layer_from_blocks(
                    &blocks(ReadOp::FnKeymap)?,
                    &blocks(ReadOp::DefaultFnKeymap)?,
                )?);
            }
            Section::Lighting => {
                p.lighting = Some(Preset::lighting_from_blocks(
                    &blocks(ReadOp::LedEffect)?,
                    &blocks(ReadOp::CustomLed)?,
                )?);
            }
        }
    }
    Ok(p)
}

/// A preset of the board's current state. The effect uses the colour from
/// the lighting record when it is known (`effect_base`).
pub fn preset_from_board(
    name: &str,
    sections: &[Section],
    b: &BoardState,
    source: String,
) -> Result<Preset> {
    let blocks = |op: ReadOp| -> Result<Vec<u8>> {
        Ok(match op {
            ReadOp::AxisRt => b.hall.clone(),
            ReadOp::FnKeymap => b.fn_layer.clone(),
            ReadOp::DefaultFnKeymap => b.fn_factory.clone(),
            ReadOp::LedEffect => b.effect_base().to_vec(),
            ReadOp::CustomLed => b.custom.clone(),
            _ => bail!("not a preset block"),
        })
    };
    build_preset(name, sections, &blocks, b.scale(), source)
}

/// A preset from a backup file's blocks.
pub fn preset_from_backup(
    name: &str,
    sections: &[Section],
    b: &Backup,
    source: String,
) -> Result<Preset> {
    let scale = type84_proto::rt::RtScale::from_precision(b.device.info.rt_precision);
    let blocks = |op: ReadOp| -> Result<Vec<u8>> {
        let bl = &b.blocks;
        match op {
            ReadOp::AxisRt => bl.axis_rt.bytes(),
            ReadOp::FnKeymap => bl.keymap_layer1.bytes(),
            ReadOp::DefaultFnKeymap => bl.default_fn_keymap.bytes(),
            ReadOp::LedEffect => bl.led_effect.bytes(),
            ReadOp::CustomLed => bl.custom_led.bytes(),
            _ => bail!("not a preset block"),
        }
    };
    build_preset(name, sections, &blocks, scale, source)
}

/// Which saved presets the board already holds (`type84 preset current`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Matches {
    /// Presets matching in full.
    pub matching: Vec<String>,
    /// Per section, the presets whose section matches.
    pub by_section: std::collections::BTreeMap<Section, Vec<String>>,
    /// Every preset with its sections and whether each matches.
    pub rows: Vec<MatchRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchRow {
    pub name: String,
    pub sections: Vec<(Section, bool)>,
    pub all: bool,
}

/// A preset section matches when applying it would change nothing.
/// `presets` are (name, preset) pairs; presets without known sections are
/// skipped.
pub fn preset_matches(presets: &[(String, Preset)], board: &BoardState) -> Matches {
    let mut m = Matches::default();
    for (name, p) in presets {
        let secs = p.sections();
        if secs.is_empty() {
            continue;
        }
        let mut row = MatchRow {
            name: name.clone(),
            sections: Vec::new(),
            all: true,
        };
        for s in secs {
            let ok = crate::plan::preview_section(p, s, board).is_ok_and(|pv| pv.is_noop());
            row.all &= ok;
            if ok {
                m.by_section.entry(s).or_default().push(name.clone());
            }
            row.sections.push((s, ok));
        }
        if row.all {
            m.matching.push(name.clone());
        }
        m.rows.push(row);
    }
    m
}

/// Load every preset of `dir` as (file stem, preset). Broken files are
/// returned as errors next to their path, never applied.
pub fn load_presets(dir: &Path) -> Result<Vec<(String, std::result::Result<Preset, String>)>> {
    Ok(crate::preset::list(dir)?
        .into_iter()
        .map(|f| {
            let stem = f
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            (stem, Preset::load(&f).map_err(|e| format!("{e:#}")))
        })
        .collect())
}

/// The section label for messages.
pub fn section_label(op: WriteOp) -> &'static str {
    match op {
        WriteOp::AxisRt => "Hall/RT table",
        WriteOp::FnKeymap => "Fn layer",
        WriteOp::LedEffect => "lighting effect",
        WriteOp::CustomLed => "per-key colours",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // flock is a no-op on Windows (no cross-process lock there), so lock semantics are unix-only.
    #[cfg(unix)]
    #[test]
    fn lock_is_exclusive_and_probe_releases() {
        let dir = std::env::temp_dir().join(format!("type84-apply-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!lock_held(&dir));
        let l = write_lock(&dir).unwrap();
        assert!(lock_held(&dir));
        let e = write_lock(&dir).unwrap_err();
        assert!(e.to_string().contains("another type84 process"));
        drop(l);
        assert!(!lock_held(&dir));
        assert!(!lock_held(&dir), "the probe itself must not keep the lock");

        // A short holder: the waiting lock gets it once it is released.
        let l = write_lock(&dir).unwrap();
        let t = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(l);
        });
        let got = write_lock_wait(&dir, std::time::Duration::from_secs(3));
        assert!(got.is_ok());
        t.join().unwrap();
        // Held past the timeout: refused.
        let e = write_lock_wait(&dir, std::time::Duration::from_millis(200)).unwrap_err();
        assert!(e.to_string().contains("another type84 process"));
        drop(got);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
