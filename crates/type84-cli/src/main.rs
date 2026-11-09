//! `type84`: command-line tool for the IO by Red Square Type 84 Magnetic
//! keyboard. Everything is read-only except `preset apply`, `restore` and
//! `anim flash|restore` without `--dry-run`, which write through the
//! guarded `type84_hid::WriteSession` (see docs/SAFETY.md and docs/CLI.md).
//!
//! Exit codes: 0 ok, 1 differs (verify-backup), 2 error, 3 no matching
//! preset (preset current), 4 device not found.

mod render;
mod verify;

use type84_cli::{anim, apply, backup, lighting_state, plan, preset, snapshot};

use std::fs::OpenOptions;
use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;
use type84_hid::{
    ApplyError, EffectTarget, RawHid, WriteOptIn, WriteSession, WriteStep, WriteTarget,
};
use type84_proto::fn_layer;
use type84_proto::keymap;
use type84_proto::layout;
use type84_proto::led::{self, EffectPatch};
use type84_proto::rt::{self, RtScale};
use type84_proto::{ReadOp, WriteOp};

use backup::Backup;
use plan::{BoardState, SectionPreview};
use preset::{Preset, Section};
use snapshot::Reader;

const EXIT_DIFFERS: u8 = 1;
const EXIT_ERROR: u8 = 2;
const EXIT_NO_MATCH: u8 = 3;
const EXIT_NOT_FOUND: u8 = 4;

#[derive(Parser)]
#[command(
    name = "type84",
    version,
    about = "Tool for the IO by Red Square Type 84 Magnetic keyboard (read-only except the guarded preset/restore/anim writes)",
    after_help = "Exit codes: 0 ok, 1 differs (verify-backup), 2 error, 3 no matching preset (preset current), 4 device not found. See docs/CLI.md."
)]
struct Cli {
    /// Print every frame sent and received to stderr.
    #[arg(short, long, global = true)]
    verbose: bool,
    /// Print only the essential result (errors still go to stderr).
    #[arg(short, long, global = true)]
    quiet: bool,
    /// Machine-readable JSON output (info, dump, preset current).
    #[arg(long, global = true)]
    json: bool,
    /// Preset directory (default: <user config dir>/type84/presets).
    #[arg(long, global = true, value_name = "DIR")]
    presets_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the board's HID interfaces and read GET_DEVICE_INFO.
    Info,
    /// Read everything and print settings and a per-key table.
    Dump,
    /// Read everything and write a versioned JSON backup.
    Backup {
        /// Output file (default: ./backups/type84-<timestamp>-fw<version>.json).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Re-read the board and compare it with a backup file.
    VerifyBackup { file: PathBuf },
    /// Presets: Hall/RT, Fn layer and lighting sections.
    Preset {
        #[command(subcommand)]
        cmd: PresetCmd,
    },
    /// Restore one section of a backup to the board (WRITES unless --dry-run).
    Restore {
        /// A backup file, or for `--only lighting` also an anim state file.
        file: PathBuf,
        /// Which section to restore.
        #[arg(long, value_enum)]
        only: Only,
        /// Show the diff and frame count; open no write session.
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive "yes" confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Lighting animations built from the board's own effects.
    Anim {
        #[command(subcommand)]
        cmd: AnimCmd,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Only {
    /// The Hall/RT table (SET_MAGNETIC_AXIS_RT).
    Hall,
    /// The Fn layer (SET_FN_KEY).
    FnLayer,
    /// The lighting effect and per-key colours (SET_LED_EFFECT, SET_CUSTOM_LED_DATA).
    Lighting,
}

#[derive(Subcommand)]
enum PresetCmd {
    /// Read sections from the board and save them as a preset.
    Save {
        name: String,
        /// Sections to save (comma-separated: hall, fn_layer, lighting).
        #[arg(long, value_delimiter = ',', default_value = "hall")]
        sections: Vec<String>,
        /// Overwrite an existing preset of that name.
        #[arg(long)]
        force: bool,
    },
    /// List presets.
    List,
    /// Print a preset.
    Show { name: String },
    /// Apply one or more presets (WRITES unless --dry-run). Presets must
    /// cover different sections, e.g. `apply cs2 fn-fbx50c`.
    Apply {
        /// Preset names, or paths to preset files.
        #[arg(required = true)]
        names: Vec<String>,
        /// Show the diff and frame count; open no write session.
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive "yes" confirmation.
        #[arg(long)]
        yes: bool,
        /// Flash the whole board in this colour (`#RRGGBB`) first: one
        /// SET_LED_EFFECT write, the other sections are written while it
        /// shows, then the preset's lighting (or the previous effect).
        #[arg(long, value_name = "#RRGGBB")]
        flash: Option<String>,
        /// Built-in effect of the flash.
        #[arg(long, value_enum, default_value = "breathing", requires = "flash")]
        flash_effect: FlashEffect,
        /// Speed step 0..=5 of the flash effect (breathing period; tunable).
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(0..=5), requires = "flash")]
        flash_speed: u8,
        /// How long the flash shows, counted from its write, in ms (100..=10000).
        #[arg(long, default_value_t = 2000, value_parser = clap::value_parser!(u64).range(100..=10000), requires = "flash")]
        flash_ms: u64,
    },
    /// Which saved presets match the board, per section (exit 0 if at
    /// least one preset matches in full, 3 if none).
    Current,
    /// Build a preset from a backup file (no device access).
    ImportBackup {
        file: PathBuf,
        name: String,
        /// Sections to import (comma-separated: hall, fn_layer, lighting).
        #[arg(long, value_delimiter = ',', default_value = "hall")]
        sections: Vec<String>,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FlashEffect {
    /// Static colour (effect 1).
    Static,
    /// Breathing (effect 7, «Динамическое дыхание»).
    Breathing,
}

impl FlashEffect {
    fn mode(self) -> u8 {
        match self {
            FlashEffect::Static => 1,
            FlashEffect::Breathing => 7,
        }
    }
}

/// `preset apply --flash` settings.
#[derive(Clone, Copy, Debug)]
struct FlashOpts {
    color: [u8; 3],
    effect: FlashEffect,
    speed: u8,
    duration: Duration,
}

#[derive(Subcommand)]
enum AnimCmd {
    /// Switch to a built-in effect, wait, then restore the previous effect:
    /// two single-frame SET_LED_EFFECT writes (WRITES unless --dry-run).
    Flash {
        /// Built-in effect: name (wave, ripple, breathing, ...) or id.
        #[arg(long, default_value = "wave")]
        effect: String,
        /// Colour `#RRGGBB` (selects single-colour mode).
        #[arg(long)]
        color: Option<String>,
        /// Brightness step 0..=5 (default: keep).
        #[arg(long)]
        brightness: Option<u8>,
        /// Speed step 0..=5 (default: keep).
        #[arg(long)]
        speed: Option<u8>,
        /// How long to show the effect, in seconds (0.1..=60).
        #[arg(long, default_value = "2", value_parser = anim::parse_seconds)]
        duration: Duration,
        /// Show what would be written; open no write session.
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive "yes" confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Put back the effect saved by an interrupted `anim flash`.
    Restore {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
}

/// Output settings shared by all commands.
#[derive(Clone, Copy)]
struct Out {
    verbose: bool,
    quiet: bool,
    json: bool,
}

macro_rules! say {
    ($o:expr, $($arg:tt)*) => {
        if !$o.quiet { println!($($arg)*); }
    };
}

fn open() -> Result<(type84_hid::Interface, type84_hid::Transport)> {
    let hid = type84_hid::Hid::new().context("initialising hidapi")?;
    Ok(hid.open_config()?)
}

fn cmd_info(o: Out) -> Result<()> {
    let hid = type84_hid::Hid::new().context("initialising hidapi")?;
    let ifaces = hid.list_interfaces();
    let (iface, mut t) = hid.open_config()?;
    let mut r = Reader::new(&mut t, o.verbose);
    let (info, raw) = r.device_info()?;
    if o.json {
        let list: Vec<_> = ifaces
            .iter()
            .map(|i| {
                json!({"path": i.path_str(), "usage_page": i.usage_page, "usage": i.usage,
                       "interface": i.interface_number, "config": i.is_config()})
            })
            .collect();
        let v = json!({
            "config_path": iface.path_str(),
            "product": iface.product,
            "firmware": info.version_string(),
            "operational": info.is_operational(),
            "device_info": info,
            "raw": hex::encode(&raw.data),
            "interfaces": list,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    if o.quiet {
        println!("{}", info.version_string());
        return Ok(());
    }
    println!("HID collections with VID 0c45 PID 80d6 (enumeration only):");
    for i in &ifaces {
        println!(
            "  {:<16} usage page {:#06x} usage {:#06x} interface {}{}",
            i.path_str(),
            i.usage_page,
            i.usage,
            i.interface_number,
            if i.is_config() {
                "  <- configuration channel"
            } else {
                ""
            }
        );
    }
    println!(
        "Opened {} ({} / {})\n",
        iface.path_str(),
        iface.manufacturer.as_deref().unwrap_or("?"),
        iface.product.as_deref().unwrap_or("?")
    );
    print!("{}", render::device_info(&info));
    println!("Raw                {}", hex::encode(&raw.data));
    Ok(())
}

fn dump_json(snap: &snapshot::Snapshot) -> serde_json::Value {
    let scale = RtScale::from_precision(snap.info.rt_precision);
    let rt = rt::decode_block(&snap.axis_rt.data);
    let l0 = keymap::decode_block(&snap.keymap_layer0.data);
    let fnl = &snap.keymap_layer1.data;
    let custom = &snap.custom_led.data;
    let keys: Vec<_> = layout::keys()
        .iter()
        .map(|k| {
            let i = k.index as usize;
            let r = &rt[i];
            json!({
                "index": k.index,
                "label": k.label,
                "actuation_mm": r.actuation_mm(scale),
                "rt": r.rt_enabled().then(|| json!({"press_mm": r.press_mm(scale), "release_mm": r.release_mm(scale)})),
                "whole_travel": r.whole_travel(),
                "layer0": l0[i].describe(),
                "fn_layer": fn_layer::describe_record([fnl[i * 4], fnl[i * 4 + 1], fnl[i * 4 + 2], fnl[i * 4 + 3]]),
                "color": led::format_hex_color([custom[i * 4 + 1], custom[i * 4 + 2], custom[i * 4 + 3]]),
            })
        })
        .collect();
    json!({
        "firmware": snap.info.version_string(),
        "device_info": snap.info,
        "settings": type84_proto::settings::Settings::decode(&snap.settings.data).ok(),
        "lighting": {
            "effect": led::LedEffect::decode(&snap.led_effect.data),
            "effect_name": led::effect_name(snap.led_effect.data[0]),
            "description": led::describe_effect(&snap.led_effect.data),
        },
        "macros": snap.macros.macros.len(),
        "keys": keys,
    })
}

fn cmd_dump(o: Out) -> Result<()> {
    let (iface, mut t) = open()?;
    let mut r = Reader::new(&mut t, o.verbose);
    let snap = r.snapshot()?;
    if o.json {
        println!("{}", serde_json::to_string_pretty(&dump_json(&snap))?);
        return Ok(());
    }
    if o.quiet {
        return Ok(());
    }
    println!(
        "Board: {} at {}\n",
        iface.product.as_deref().unwrap_or("?"),
        iface.path_str()
    );
    print!("{}", render::dump(&snap));
    Ok(())
}

fn read_backup(verbose: bool) -> Result<Backup> {
    let (iface, mut t) = open()?;
    let mut r = Reader::new(&mut t, verbose);
    let snap = r.snapshot()?;
    Backup::from_snapshot(&snap, &iface)
}

fn cmd_backup(o: Out, output: Option<PathBuf>) -> Result<()> {
    let b = read_backup(o.verbose)?;
    let path = match output {
        Some(p) => p,
        None => {
            let dir = PathBuf::from("backups");
            std::fs::create_dir_all(&dir).context("creating ./backups")?;
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            dir.join(format!("type84-{ts}-fw{}.json", b.device.firmware))
        }
    };
    let json = serde_json::to_string_pretty(&b)?;
    // Never overwrite an existing file.
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("creating {} (refusing to overwrite)", path.display()))?;
    f.write_all(json.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    // Read it back through the loader to prove it parses and validates.
    Backup::load(&path)?;
    if o.quiet {
        println!("{}", path.display());
        return Ok(());
    }
    println!("Backup written: {}", path.display());
    println!(
        "Firmware {}, {} macros, {} macro table errors",
        b.device.firmware,
        b.macros.walk.macros.len(),
        b.macros.walk.errors.len()
    );
    Ok(())
}

fn cmd_verify(o: Out, file: PathBuf) -> Result<ExitCode> {
    let old = Backup::load(&file)?;
    let new = read_backup(o.verbose)?;
    let (lines, restore_diff) = verify::compare(&old, &new)?;
    if !o.quiet {
        for l in &lines {
            println!("{l}");
        }
    }
    if restore_diff {
        say!(o, "RESULT: board differs from {}", file.display());
        Ok(ExitCode::from(EXIT_DIFFERS))
    } else {
        say!(
            o,
            "RESULT: board matches {} (all restorable blocks identical)",
            file.display()
        );
        Ok(ExitCode::SUCCESS)
    }
}

fn presets_dir(opt: &Option<PathBuf>) -> Result<PathBuf> {
    match opt {
        Some(d) => Ok(d.clone()),
        None => preset::default_dir(),
    }
}

fn read_board(verbose: bool) -> Result<BoardState> {
    let (_iface, mut t) = open()?;
    let mut r = Reader::new(&mut t, verbose);
    BoardState::read(&mut r)
    // The read-only transport is dropped (closed) here.
}

fn lighting_state_path() -> Result<PathBuf> {
    Ok(lighting_state::path(&anim::state_dir()?))
}

/// What is known about the effect block `read` (the colour is not reported
/// by the board; see `lighting_state`).
fn effect_knowledge(read: &[u8]) -> lighting_state::Knowledge {
    match anim::state_dir() {
        Ok(d) => apply::effect_knowledge(&d, read),
        Err(e) => lighting_state::Knowledge::Unknown {
            reason: format!("{e:#}"),
        },
    }
}

/// Fill `board.effect_known` from the lighting state file.
fn attach_knowledge(board: &mut BoardState) -> lighting_state::Knowledge {
    let k = effect_knowledge(&board.effect);
    board.effect_known = k.known().map(<[u8]>::to_vec);
    k
}

/// Record the last effect write of a successful plan. A failure only warns:
/// the next run then treats the colour as unknown and writes it again.
fn record_lighting(steps: &[type84_hid::write::StepReport]) {
    let r = lighting_state_path().and_then(|p| lighting_state::record(&p, steps));
    if let Err(e) = r {
        eprintln!("warning: could not save the lighting state: {e:#}");
    }
}

fn section_label(op: WriteOp) -> &'static str {
    apply::section_label(op)
}

fn print_block_writes(o: Out, blocks: &[plan::BlockChange]) {
    for b in blocks.iter().filter(|b| !b.changed.is_empty()) {
        say!(
            o,
            "  would write {} {} ({:#04x}) frame(s) (full {}-byte block), then read it back and compare",
            b.op.frame_count(),
            b.op.name(),
            b.op.code(),
            b.op.block_size()
        );
    }
}

/// Print a preview; returns the number of blocks that change.
fn print_previews(o: Out, previews: &[SectionPreview], scale: RtScale) -> usize {
    let mut changed_blocks = 0;
    for s in previews {
        let lines = s.lines(scale);
        say!(
            o,
            "[{}] from {}: {}",
            s.section.name(),
            s.preset,
            if s.is_noop() {
                "no changes".to_string()
            } else {
                format!("{} change(s)", lines.len())
            }
        );
        for l in &lines {
            say!(o, "{l}");
        }
        print_block_writes(o, &s.blocks);
        changed_blocks += s.blocks.iter().filter(|b| !b.changed.is_empty()).count();
    }
    changed_blocks
}

fn confirm(yes: bool, what: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("stdin is not a terminal; pass --yes to confirm the write non-interactively");
    }
    print!("This WRITES the {what} to the board (persistent). Type \"yes\" to continue: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if line.trim() != "yes" {
        bail!("not confirmed; nothing written");
    }
    Ok(())
}

fn describe_steps(steps: &[WriteStep]) -> String {
    let mut ops: Vec<WriteOp> = steps.iter().map(|s| s.target.op()).collect();
    ops.sort_by_key(|&op| type84_hid::write::write_rank(op));
    let v: Vec<&str> = ops.into_iter().map(section_label).collect();
    v.join(" and ")
}

/// Open a write session for exactly the sections of `steps` and run them
/// with the automatic full pre-write backup, holding the write lock.
fn run_plan(o: Out, steps: Vec<WriteStep>) -> Result<ExitCode> {
    let opt_in = plan::opt_in_for(&steps).context("nothing to write")?;
    let state_dir = anim::state_dir()?;
    let _lock = apply::write_lock_wait(&state_dir, apply::LOCK_WAIT)?;
    let hid = type84_hid::Hid::new().context("initialising hidapi")?;
    let (iface, mut session) = hid.open_write_session(opt_in)?;
    if o.verbose {
        session.set_trace(Box::new(|tag, f| eprintln!("{tag} {}", hex::encode(f))));
    }
    let dir = type84_cli::write::auto_backup_dir();
    let rep = apply::run_steps(&mut session, &iface, &steps, &dir, &state_dir, o.verbose)?;
    if let Some(w) = &rep.record_warning {
        eprintln!("warning: could not save the lighting state: {w}");
    }
    if rep.nothing_to_write {
        say!(
            o,
            "Nothing to write: the board already holds this (pre-write backup {}).",
            rep.backup_path.display()
        );
        return Ok(ExitCode::SUCCESS);
    }
    say!(o, "Pre-write backup: {}", rep.backup_path.display());
    print_step_reports(o, &rep.steps);
    Ok(ExitCode::SUCCESS)
}

fn print_step_reports(o: Out, steps: &[type84_hid::write::StepReport]) {
    for s in steps {
        if s.frames_written == 0 {
            say!(o, "{}: already up to date, skipped.", section_label(s.op));
        } else {
            say!(
                o,
                "{}: wrote {} frame(s); readback matches ({} record(s) changed).",
                section_label(s.op),
                s.frames_written,
                s.changed.len()
            );
        }
        for n in &s.notes {
            say!(o, "  note: {n}");
        }
    }
}

/// The `--flash` plan: the flash effect, the steps written while it shows,
/// and the lighting written after it.
struct FlashPlan {
    flash: Vec<u8>,
    during: Vec<WriteStep>,
    final_effect: Vec<u8>,
    final_custom: Option<WriteStep>,
}

impl FlashPlan {
    fn new(f: &FlashOpts, board: &BoardState, previews: &[SectionPreview]) -> Result<Self> {
        let base = board.effect_base();
        let flash = EffectPatch {
            mode: Some(f.effect.mode()),
            color: Some(f.color),
            brightness: Some(5),
            speed: Some(f.speed),
            ..Default::default()
        }
        .apply(base)?
        .to_vec();
        let mut final_effect = base.to_vec();
        let mut final_custom = None;
        let mut during = Vec::new();
        for b in previews.iter().flat_map(|s| s.blocks.iter()) {
            match b.op {
                // Always written after the flash, changed or not.
                WriteOp::LedEffect => final_effect = b.after.clone(),
                WriteOp::CustomLed if !b.changed.is_empty() => final_custom = Some(b.step.clone()),
                WriteOp::CustomLed => {}
                WriteOp::AxisRt | WriteOp::FnKeymap if !b.changed.is_empty() => {
                    during.push(b.step.clone())
                }
                WriteOp::AxisRt | WriteOp::FnKeymap => {}
            }
        }
        if final_custom.is_some() && !led::is_custom_mode(final_effect[0]) {
            bail!("lighting: per-key colours need the custom effect after the flash");
        }
        Ok(FlashPlan {
            flash,
            during,
            final_effect,
            final_custom,
        })
    }

    fn opt_in(&self) -> WriteOptIn {
        let lighting = WriteOptIn::user_confirmed_lighting_write();
        match plan::opt_in_for(&self.during) {
            Some(t) => t.and(lighting),
            None => lighting,
        }
    }

    fn print(&self, o: Out, f: &FlashOpts, colour_known: bool) {
        say!(
            o,
            "Flash plan: 1 SET_LED_EFFECT (0x23) frame: {}",
            led::describe_effect(&self.flash)
        );
        if !self.during.is_empty() {
            say!(
                o,
                "  then, while it shows: {}",
                describe_steps(&self.during)
            );
        }
        say!(
            o,
            "  then, {} ms after the flash write: 1 SET_LED_EFFECT (0x23) frame: {}{}",
            f.duration.as_millis(),
            led::describe_effect(&self.final_effect),
            if colour_known {
                ""
            } else {
                " (colour unknown: the board does not report it and there is no valid record of the last write)"
            }
        );
        if self.final_custom.is_some() {
            say!(
                o,
                "  plus {} SET_CUSTOM_LED_DATA (0x24) frame(s) (per-key block differs)",
                WriteOp::CustomLed.frame_count()
            );
        } else {
            say!(o, "  per-key colours already match: no 0x24 write");
        }
        let lighting_frames = 2 + if self.final_custom.is_some() {
            WriteOp::CustomLed.frame_count()
        } else {
            0
        };
        say!(
            o,
            "  lighting writes: {} frame(s) ({} block write(s))",
            lighting_frames,
            2 + self.final_custom.is_some() as usize
        );
    }
}

/// Run a `--flash` plan in one write session with one pre-write backup.
/// Once the flash is on the board, the final lighting is written even if a
/// step in between fails or a stop signal arrives.
fn run_flash_plan(o: Out, fp: FlashPlan, board: &BoardState, f: &FlashOpts) -> Result<ExitCode> {
    let dir = anim::state_dir()?;
    let _lock = apply::write_lock_wait(&dir, apply::LOCK_WAIT)?;
    let hid = type84_hid::Hid::new().context("initialising hidapi")?;
    let (iface, mut session) = hid.open_write_session(fp.opt_in())?;
    if o.verbose {
        session.set_trace(Box::new(|tag, f| eprintln!("{tag} {}", hex::encode(f))));
    }
    let backup_dir = type84_cli::write::auto_backup_dir();
    let mut real_backup = type84_cli::write::auto_backup(&iface, &backup_dir, o.verbose);
    let mut saved: Option<PathBuf> = None;
    let mut backup_once = |io: &mut dyn type84_proto::transfer::FrameIo| {
        if let Some(p) = &saved {
            return Ok(p.clone());
        }
        let p = real_backup(io)?;
        saved = Some(p.clone());
        Ok(p)
    };
    anim::install_stop_handler();

    // 1. The flash.
    let flash_step = WriteStep {
        target: WriteTarget::LedEffect(EffectTarget::Forced(fp.flash.clone())),
        expected_base: Some(board.effect.clone()),
    };
    let rep = match session.apply_plan(&[flash_step], &mut backup_once) {
        Ok(rep) => rep,
        Err(e) => {
            // If the flash frame may have reached the board, put the final
            // effect back (no preview check: the board state is unclear).
            if matches!(
                e,
                ApplyError::WriteFailed { .. }
                    | ApplyError::ReadbackFailed { .. }
                    | ApplyError::ReadbackMismatch { .. }
            ) {
                let undo = WriteStep {
                    target: WriteTarget::LedEffect(EffectTarget::Forced(fp.final_effect.clone())),
                    expected_base: None,
                };
                match session.apply_plan(&[undo], &mut backup_once) {
                    Ok(rep) => record_lighting(&rep.steps),
                    Err(e2) => eprintln!("error: putting the effect back also failed: {e2}"),
                }
            }
            return Err(e.into());
        }
    };
    let t0 = Instant::now();
    record_lighting(&rep.steps);
    say!(o, "Pre-write backup: {}", rep.backup_path.display());
    say!(o, "flash: wrote 1 frame.");
    let flash_read = rep.steps[0].read_back.clone();

    // 2. The other sections while the flash shows.
    let during_result = if fp.during.is_empty() {
        Ok(())
    } else {
        match session.apply_plan(&fp.during, &mut backup_once) {
            Ok(rep) => {
                print_step_reports(o, &rep.steps);
                Ok(())
            }
            Err(ApplyError::NoChanges { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    };

    // 3. Wait out the flash (cut short by a stop signal or an error).
    if during_result.is_ok() {
        anim::sleep_interruptible(f.duration.saturating_sub(t0.elapsed()));
    }
    if anim::stop_requested() {
        say!(o, "Stop requested: writing the final lighting now.");
    }

    // 4. The final lighting.
    let mut final_steps = vec![WriteStep {
        target: WriteTarget::LedEffect(EffectTarget::Forced(fp.final_effect.clone())),
        expected_base: Some(flash_read),
    }];
    final_steps.extend(fp.final_custom.clone());
    let final_result = session.apply_plan(&final_steps, &mut backup_once);
    if let Ok(rep) = &final_result {
        record_lighting(&rep.steps);
        print_step_reports(o, &rep.steps);
    }
    match (during_result, final_result) {
        (Ok(()), Ok(_)) => Ok(ExitCode::SUCCESS),
        (Err(e), Ok(_)) => Err(anyhow::Error::from(e)
            .context("a section written during the flash failed; the final lighting was written")),
        (_, Err(e)) => Err(anyhow::Error::from(e).context(
            "the final lighting write failed: the board may still show the flash; \
             re-run the command or `type84 restore <pre-write backup> --only lighting`",
        )),
    }
}

fn parse_sections(v: &[String]) -> Result<Vec<Section>> {
    let mut out = Vec::new();
    for s in v {
        let s = Section::parse(s)?;
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

fn cmd_preset_save(
    o: Out,
    dir: &Path,
    name: &str,
    sections: &[Section],
    force: bool,
) -> Result<()> {
    preset::validate_name(name)?;
    let (iface, mut t) = open()?;
    let mut r = Reader::new(&mut t, o.verbose);
    let mut b = BoardState::read(&mut r)?;
    // The effect colour comes from the last-write record when it is fresh.
    attach_knowledge(&mut b);
    let source = format!(
        "board {} fw {} at {}",
        iface.path_str(),
        b.info.version_string(),
        chrono::Local::now().to_rfc3339()
    );
    let p = apply::preset_from_board(name, sections, &b, source)?;
    let path = preset::resolve(name, dir)?;
    p.save(&path, force)?;
    say!(
        o,
        "Preset saved: {} [{}]",
        path.display(),
        p.section_names().join(", ")
    );
    Ok(())
}

fn cmd_preset_import(
    o: Out,
    dir: &Path,
    file: &Path,
    name: &str,
    sections: &[Section],
    force: bool,
) -> Result<()> {
    preset::validate_name(name)?;
    let b = Backup::load(file)?;
    let source = format!(
        "backup {} (fw {}, {})",
        file.display(),
        b.device.firmware,
        b.created_at
    );
    let p = apply::preset_from_backup(name, sections, &b, source)?;
    let path = preset::resolve(name, dir)?;
    p.save(&path, force)?;
    say!(
        o,
        "Preset saved: {} [{}]",
        path.display(),
        p.section_names().join(", ")
    );
    Ok(())
}

fn hall_summary(p: &Preset) -> String {
    let Some(h) = &p.hall else {
        return String::new();
    };
    let mut groups: std::collections::BTreeMap<String, Vec<&str>> = Default::default();
    for k in &h.keys {
        let rt =
            k.rt.map(|r| format!("RT {:.2}/{:.2}", r.press_mm, r.release_mm))
                .unwrap_or_else(|| "RT off".into());
        let wt = if k.whole_travel { ", whole travel" } else { "" };
        groups
            .entry(format!("{:.2} mm {rt}{wt}", k.actuation_mm))
            .or_default()
            .push(&k.label);
    }
    groups
        .into_iter()
        .map(|(g, keys)| {
            if keys.len() <= 8 {
                format!("{g}: {}", keys.join(" "))
            } else {
                format!("{g}: {} keys", keys.len())
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn preset_summary(p: &Preset) -> String {
    let mut parts = Vec::new();
    if p.hall.is_some() {
        parts.push(hall_summary(p));
    }
    if let Some(f) = &p.fn_layer {
        parts.push(format!("Fn layer: {} key(s)", f.keys.len()));
    }
    if let Some(l) = &p.lighting {
        let mut s = format!(
            "lighting: {}",
            l.effect.as_deref().unwrap_or("(effect kept)")
        );
        if let Some(c) = &l.color {
            s.push_str(&format!(" {c}"));
        }
        if let Some(k) = &l.keys {
            s.push_str(&format!(", {} key colour(s)", k.len()));
        }
        parts.push(s);
    }
    parts.join(" | ")
}

fn cmd_preset_list(o: Out, dir: &Path) -> Result<()> {
    let files = preset::list(dir)?;
    if files.is_empty() {
        say!(o, "No presets in {}", dir.display());
    }
    for f in files {
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match Preset::load(&f) {
            Ok(_) if o.quiet => println!("{stem}"),
            Ok(p) => println!(
                "{:<16} [{}] {}",
                stem,
                p.section_names().join(", "),
                preset_summary(&p)
            ),
            Err(e) => eprintln!("{:<16} ERROR: {e:#}", f.display()),
        }
    }
    Ok(())
}

fn cmd_preset_show(dir: &Path, name: &str) -> Result<()> {
    let path = preset::resolve(name, dir)?;
    let p = Preset::load(&path)?;
    println!("Preset {} ({})", p.name, path.display());
    if let Some(c) = &p.created_at {
        println!("Created  {c}");
    }
    if let Some(s) = &p.source {
        println!("Source   {s}");
    }
    println!("Sections {}", p.section_names().join(", "));
    for s in p.unknown_sections.keys() {
        println!(
            "  note: section {s:?} is not supported by this build; it would be skipped by apply"
        );
    }
    if let Some(h) = &p.hall {
        println!(
            "\n[hall]\n{:>3}  {:<10} {:>6}  {:<3} {:>5} {:>7}  whole travel",
            "idx", "key", "act", "RT", "press", "release"
        );
        for k in &h.keys {
            let (rt, pr, rl) = match k.rt {
                Some(r) => (
                    "on",
                    format!("{:.2}", r.press_mm),
                    format!("{:.2}", r.release_mm),
                ),
                None => ("off", "-".into(), "-".into()),
            };
            println!(
                "{:>3}  {:<10} {:>6.2}  {:<3} {:>5} {:>7}  {}",
                k.index,
                k.label,
                k.actuation_mm,
                rt,
                pr,
                rl,
                if k.whole_travel { "on" } else { "off" }
            );
        }
        let missing = layout::keys().len() - h.keys.len();
        if missing > 0 {
            println!("{missing} physical key(s) not in the preset are left unchanged by apply.");
        }
    }
    if let Some(f) = &p.fn_layer {
        println!("\n[fn_layer] (keys not listed keep their current Fn action)");
        for k in &f.keys {
            println!(
                "  Fn+{:<10} {:<28} {}",
                k.label,
                k.action.to_string(),
                k.note.as_deref().unwrap_or("")
            );
        }
    }
    if let Some(l) = &p.lighting {
        println!("\n[lighting] (absent fields keep the board's value)");
        let f = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
        let n = |v: Option<u8>| v.map(|x| format!("{x}/5")).unwrap_or_else(|| "-".into());
        println!("  effect      {}", f(&l.effect));
        println!("  colour      {}", f(&l.color));
        println!("  colour mode {}", f(&l.color_mode));
        println!("  brightness  {}", n(l.brightness));
        println!("  speed       {}", n(l.speed));
        println!("  direction   {}", f(&l.direction));
        for k in l.keys.iter().flatten() {
            println!("  key {:<10} {}", k.label, k.color);
        }
    }
    Ok(())
}

fn cmd_preset_apply(
    o: Out,
    dir: &Path,
    names: &[String],
    dry_run: bool,
    yes: bool,
    flash: Option<FlashOpts>,
) -> Result<ExitCode> {
    let mut presets = Vec::new();
    for n in names {
        let path = preset::resolve(n, dir)?;
        let p = Preset::load(&path)?;
        say!(
            o,
            "Preset {} ({}) [{}]",
            p.name,
            path.display(),
            p.section_names().join(", ")
        );
        for s in p.unknown_sections.keys() {
            say!(
                o,
                "note: section {s:?} is not supported by this build; skipped"
            );
        }
        presets.push(p);
    }
    let mut board = read_board(o.verbose)?;
    if !board.info.is_operational() {
        bail!(
            "firmwareStatus = {} (firmware busy); refusing",
            board.info.firmware_status
        );
    }
    let knowledge = attach_knowledge(&mut board);
    say!(
        o,
        "Board firmware {}, rtPrecision {}. Diff (board → preset):",
        board.info.version_string(),
        board.info.rt_precision
    );
    let mut previews = plan::preview_presets(&presets, &board)?;
    if plan::force_unknown_effect(&mut previews, &board) {
        if let lighting_state::Knowledge::Unknown { reason } = &knowledge {
            say!(
                o,
                "note: the board does not report its lighting colour and it is unknown ({reason}); the effect is written anyway"
            );
        }
    }
    if let Some(f) = flash {
        print_previews(o, &previews, board.scale());
        let fp = FlashPlan::new(&f, &board, &previews)?;
        fp.print(o, &f, board.effect_known.is_some() || has_effect(&previews));
        if dry_run {
            say!(o, "Dry run: nothing written, no write session opened.");
            return Ok(ExitCode::SUCCESS);
        }
        confirm(
            yes,
            "flash and the lighting (plus any other changed section; an automatic backup is saved first)",
        )?;
        return run_flash_plan(o, fp, &board, &f);
    }
    if previews.is_empty() {
        say!(
            o,
            "The preset(s) have no applicable section; nothing to apply."
        );
        return Ok(ExitCode::SUCCESS);
    }
    let changed = print_previews(o, &previews, board.scale());
    if changed == 0 {
        say!(o, "No differences: the board already matches.");
        return Ok(ExitCode::SUCCESS);
    }
    if dry_run {
        say!(o, "Dry run: nothing written, no write session opened.");
        return Ok(ExitCode::SUCCESS);
    }
    let steps = plan::steps(&previews);
    confirm(
        yes,
        &format!(
            "{} (an automatic backup is saved first)",
            describe_steps(&steps)
        ),
    )?;
    run_plan(o, steps)
}

/// Whether a preview sets the effect (so its colour comes from the preset,
/// not from what is known about the board).
fn has_effect(previews: &[SectionPreview]) -> bool {
    previews
        .iter()
        .flat_map(|s| s.blocks.iter())
        .any(|b| b.op == WriteOp::LedEffect)
}

type MatchRow = (String, serde_json::Map<String, serde_json::Value>, bool);

fn cmd_preset_current(o: Out, dir: &Path) -> Result<ExitCode> {
    let mut board = read_board(o.verbose)?;
    attach_knowledge(&mut board);
    let mut presets = Vec::new();
    for (stem, p) in apply::load_presets(dir)? {
        match p {
            Ok(p) => presets.push((stem, p)),
            Err(e) => eprintln!(
                "warning: skipping {}: {e}",
                dir.join(format!("{stem}.json")).display()
            ),
        }
    }
    let m = apply::preset_matches(&presets, &board);
    let matching = m.matching.clone();
    let by_section: std::collections::BTreeMap<&str, Vec<String>> = m
        .by_section
        .iter()
        .map(|(s, v)| (s.name(), v.clone()))
        .collect();
    let rows: Vec<MatchRow> = m
        .rows
        .iter()
        .map(|r| {
            let mut res = serde_json::Map::new();
            for (s, ok) in &r.sections {
                res.insert(s.name().into(), (*ok).into());
            }
            (r.name.clone(), res, r.all)
        })
        .collect();
    if o.json {
        let presets: Vec<_> = rows
            .iter()
            .map(|(n, s, all)| json!({"name": n, "sections": s, "matches": all}))
            .collect();
        let v = json!({"matching": matching, "by_section": by_section, "presets": presets});
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else if o.quiet {
        for m in &matching {
            println!("{m}");
        }
    } else {
        for (n, s, all) in &rows {
            let secs: Vec<String> = s
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{k} {}",
                        if v.as_bool() == Some(true) {
                            "yes"
                        } else {
                            "no"
                        }
                    )
                })
                .collect();
            println!(
                "{:<16} {:<5} ({})",
                n,
                if *all { "MATCH" } else { "-" },
                secs.join(", ")
            );
        }
        for (sec, names) in &by_section {
            println!("{sec}: matched by {}", names.join(", "));
        }
        if matching.is_empty() {
            println!("No saved preset matches the board in full.");
        }
    }
    Ok(if matching.is_empty() {
        ExitCode::from(EXIT_NO_MATCH)
    } else {
        ExitCode::SUCCESS
    })
}

fn check_backup_matches_board(b: &Backup, board: &BoardState) -> Result<()> {
    if let Some(w) = apply::check_backup_matches_board(b, board)? {
        eprintln!("WARNING: {w}");
    }
    Ok(())
}

/// Print the restore diff of each step; returns the number of blocks that change.
fn restore_preview(o: Out, board: &BoardState, steps: &[(WriteStep, Vec<u8>)]) -> usize {
    let mut n = 0;
    for (step, after) in steps {
        let op = step.target.op();
        let before = board.block(op);
        let changed = type84_hid::write::changed_records(op, before, after);
        if changed.is_empty() {
            say!(o, "{}: no differences.", section_label(op));
            continue;
        }
        n += 1;
        let pv = SectionPreview {
            section: Section::Lighting,
            preset: String::new(),
            blocks: vec![plan::BlockChange {
                op,
                before: before.to_vec(),
                after: after.clone(),
                changed,
                step: step.clone(),
            }],
        };
        for l in pv.lines(board.scale()) {
            say!(o, "{l}");
        }
        print_block_writes(o, &pv.blocks);
    }
    n
}

fn cmd_restore(o: Out, file: &Path, only: Only, dry_run: bool, yes: bool) -> Result<ExitCode> {
    // An anim state file restores the lighting effect only.
    if matches!(only, Only::Lighting) {
        if let Ok(text) = std::fs::read_to_string(file) {
            if let Ok(st) = anim::AnimState::from_json(&text) {
                let _lock = apply::write_lock_wait(&anim::state_dir()?, apply::LOCK_WAIT)?;
                restore_effect(o, &st.effect_bytes()?, file, dry_run, yes)?;
                return Ok(ExitCode::SUCCESS);
            }
        }
    }
    let b = Backup::load(file)?;
    let board = read_board(o.verbose)?;
    check_backup_matches_board(&b, &board)?;
    say!(
        o,
        "Restoring {} from {} (created {}).",
        match only {
            Only::Hall => "the Hall/RT table",
            Only::FnLayer => "the Fn layer",
            Only::Lighting => "the lighting",
        },
        file.display(),
        b.created_at
    );
    let section = match only {
        Only::Hall => Section::Hall,
        Only::FnLayer => Section::FnLayer,
        Only::Lighting => Section::Lighting,
    };
    let rs = apply::restore_section(&b, section, &board)?;
    for n in &rs.notes {
        say!(o, "note: {n}");
    }
    let steps = rs.steps;
    if restore_preview(o, &board, &steps) == 0 {
        say!(o, "No differences: the board already matches the backup.");
        return Ok(ExitCode::SUCCESS);
    }
    if dry_run {
        say!(o, "Dry run: nothing written, no write session opened.");
        return Ok(ExitCode::SUCCESS);
    }
    let steps: Vec<WriteStep> = steps.into_iter().map(|(s, _)| s).collect();
    confirm(
        yes,
        &format!(
            "{} (an automatic backup is saved first)",
            describe_steps(&steps)
        ),
    )?;
    run_plan(o, steps)
}

/// Read device info and the LED effect only.
fn read_effect(verbose: bool) -> Result<(type84_proto::device_info::DeviceInfo, Vec<u8>)> {
    let (_iface, mut t) = open()?;
    let mut r = Reader::new(&mut t, verbose);
    let (info, _) = r.device_info()?;
    let eff = r
        .read_full(ReadOp::LedEffect)
        .context("GET_LED_EFFECT failed")?
        .data;
    Ok((info, eff))
}

/// Write one effect block. The pre-write backup of this write is the anim
/// state file, which holds the effect block, the only block written.
/// Returns the number of frames written and the block read back (the raw
/// readback `current` is the expected base of the write).
fn write_effect<D: RawHid>(
    o: Out,
    session: &mut WriteSession<D>,
    target: &[u8],
    current: &[u8],
    state_file: &Path,
) -> Result<(usize, Vec<u8>)> {
    // Forced: the board does not report the colour, so a block that differs
    // only in colour would otherwise look unchanged and be skipped.
    let step = WriteStep {
        target: WriteTarget::LedEffect(EffectTarget::Forced(target.to_vec())),
        expected_base: Some(current.to_vec()),
    };
    let saved = state_file.to_path_buf();
    let mut backup = move |_: &mut dyn type84_proto::transfer::FrameIo| {
        anim::AnimState::load(&saved)
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("{} is missing", saved.display()))?;
        Ok(saved.clone())
    };
    match session.apply_plan(&[step], &mut backup) {
        Ok(rep) => {
            record_lighting(&rep.steps);
            for n in rep.steps.iter().flat_map(|s| &s.notes) {
                say!(o, "note: {n}");
            }
            let read_back = rep
                .steps
                .last()
                .map(|s| s.read_back.clone())
                .unwrap_or_default();
            Ok((rep.steps.iter().map(|s| s.frames_written).sum(), read_back))
        }
        Err(ApplyError::NoChanges { .. }) => {
            say!(o, "Effect already set; write skipped.");
            Ok((0, current.to_vec()))
        }
        Err(e) => Err(e.into()),
    }
}

/// Write back a saved effect (anim state file).
fn restore_effect(
    o: Out,
    baseline: &[u8],
    state_file: &Path,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    let (info, current) = read_effect(o.verbose)?;
    if !info.is_operational() {
        bail!(
            "firmwareStatus = {} (firmware busy); refusing",
            info.firmware_status
        );
    }
    let ours = state_file == anim::state_path(&anim::state_dir()?);
    let known = effect_knowledge(&current).known().map(<[u8]>::to_vec);
    say!(o, "Saved effect:   {}", led::describe_effect(baseline));
    say!(
        o,
        "Current effect: {}{}",
        led::describe_effect(known.as_deref().unwrap_or(&current)),
        if known.is_some() {
            ""
        } else {
            " (colour unknown)"
        }
    );
    // Without a valid record of the last write the colour is unknown, so
    // the saved effect is written.
    if known.is_some_and(|k| led::effect_fields_eq(baseline, &k)) {
        say!(
            o,
            "Nothing to restore: the board already shows the saved effect."
        );
        if !dry_run && ours {
            std::fs::remove_file(state_file).ok();
        }
        return Ok(());
    }
    say!(
        o,
        "Would write 1 SET_LED_EFFECT (0x23) frame, then read it back and compare."
    );
    if dry_run {
        say!(o, "Dry run: nothing written, no write session opened.");
        return Ok(());
    }
    confirm(yes, "lighting effect")?;
    let hid = type84_hid::Hid::new().context("initialising hidapi")?;
    let (_iface, mut session) =
        hid.open_write_session(WriteOptIn::user_confirmed_lighting_write())?;
    let (n, _) = write_effect(o, &mut session, baseline, &current, state_file)?;
    say!(o, "Restored ({n} frame).");
    if ours {
        std::fs::remove_file(state_file).ok();
    }
    Ok(())
}

struct FlashArgs<'a> {
    effect: &'a str,
    color: Option<&'a str>,
    brightness: Option<u8>,
    speed: Option<u8>,
    duration: Duration,
    dry_run: bool,
    yes: bool,
}

fn cmd_anim_flash(o: Out, a: FlashArgs) -> Result<ExitCode> {
    let mode = led::effect_id(a.effect).with_context(|| {
        format!(
            "unknown effect {:?} (use a name like wave, ripple, breathing, or an id 1..19)",
            a.effect
        )
    })?;
    if mode == 0 {
        bail!("effect 0 (off) cannot be flashed");
    }
    let color = match a.color {
        Some(c) => {
            Some(led::parse_hex_color(c).with_context(|| format!("colour {c:?} is not #RRGGBB"))?)
        }
        None => None,
    };
    let patch = EffectPatch {
        mode: Some(mode),
        color,
        brightness: a.brightness,
        speed: a.speed,
        ..Default::default()
    };
    let dir = anim::state_dir()?;
    let state_path = anim::state_path(&dir);
    // A short wait lets a brief holder (the GUI's Hall poll) finish; a real
    // concurrent animation holds the lock far longer and is skipped.
    let Ok(_lock) = apply::write_lock_wait(&dir, Duration::from_millis(500)) else {
        say!(
            o,
            "Another `type84 anim` or write is running; skipped (nothing written)."
        );
        return Ok(ExitCode::SUCCESS);
    };
    let (info, current) = read_effect(o.verbose)?;
    if !info.is_operational() {
        bail!(
            "firmwareStatus = {} (firmware busy); refusing",
            info.firmware_status
        );
    }
    let current_known = effect_knowledge(&current).known().map(<[u8]>::to_vec);
    let saved = anim::AnimState::load(&state_path)?;
    let baseline = match &saved {
        Some(s) => {
            say!(
                o,
                "note: reusing the effect saved by an interrupted run ({}) as the one to restore",
                state_path.display()
            );
            s.effect_bytes()?
        }
        None => current_known.clone().unwrap_or_else(|| current.clone()),
    };
    let target = patch.apply(&baseline)?.to_vec();
    // The board does not report the colour: without a valid record of the
    // last write the switch is always written.
    let first = current_known
        .as_ref()
        .is_none_or(|k| !led::effect_fields_eq(k, &target));
    let second = !led::effect_fields_eq(&target, &baseline);
    say!(
        o,
        "Current effect: {}{}",
        led::describe_effect(current_known.as_deref().unwrap_or(&current)),
        if current_known.is_some() {
            ""
        } else {
            " (colour unknown)"
        }
    );
    say!(
        o,
        "Flash effect:   {} for {:.1} s",
        led::describe_effect(&target),
        a.duration.as_secs_f64()
    );
    say!(o, "Then restore:   {}", led::describe_effect(&baseline));
    say!(
        o,
        "Would write {} SET_LED_EFFECT (0x23) frame(s) in total ({} to switch, {} to restore), each read back and compared. State file: {}",
        first as usize + second as usize,
        first as usize,
        second as usize,
        state_path.display()
    );
    if a.dry_run {
        say!(o, "Dry run: nothing written, no write session opened.");
        return Ok(ExitCode::SUCCESS);
    }
    if !first && !second {
        say!(o, "Nothing to do: the board already shows this effect.");
        return Ok(ExitCode::SUCCESS);
    }
    confirm(a.yes, "lighting effect (twice: switch and restore)")?;
    anim::install_stop_handler();
    if saved.is_none() {
        anim::AnimState::new(&baseline).save(&state_path)?;
    }
    let hid = type84_hid::Hid::new().context("initialising hidapi")?;
    let (_iface, mut session) =
        hid.open_write_session(WriteOptIn::user_confirmed_lighting_write())?;
    if o.verbose {
        session.set_trace(Box::new(|tag, f| eprintln!("{tag} {}", hex::encode(f))));
    }
    let hint = || {
        format!(
            "the board may still show the flash effect; run `type84 anim restore` (saved effect in {})",
            state_path.display()
        )
    };
    let (mut n1, mut n2) = (0, 0);
    let mut board_now = current.clone();
    if first {
        let (n, read_back) =
            write_effect(o, &mut session, &target, &current, &state_path).with_context(hint)?;
        n1 = n;
        board_now = read_back;
    }
    anim::sleep_interruptible(a.duration);
    if second || first {
        n2 = write_effect(o, &mut session, &baseline, &board_now, &state_path)
            .with_context(hint)?
            .0;
    }
    std::fs::remove_file(&state_path).ok();
    say!(o, "Done: {} frame(s) written in total.", n1 + n2);
    Ok(ExitCode::SUCCESS)
}

fn cmd_anim_restore(o: Out, dry_run: bool, yes: bool) -> Result<ExitCode> {
    let dir = anim::state_dir()?;
    let path = anim::state_path(&dir);
    let _lock = apply::write_lock_wait(&dir, apply::LOCK_WAIT)?;
    let Some(st) = anim::AnimState::load(&path)? else {
        say!(
            o,
            "No saved effect ({} does not exist); nothing to restore.",
            path.display()
        );
        return Ok(ExitCode::SUCCESS);
    };
    restore_effect(o, &st.effect_bytes()?, &path, dry_run, yes)?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_preset(o: Out, dir_opt: &Option<PathBuf>, cmd: PresetCmd) -> Result<ExitCode> {
    let dir = presets_dir(dir_opt)?;
    match cmd {
        PresetCmd::Save {
            name,
            sections,
            force,
        } => cmd_preset_save(o, &dir, &name, &parse_sections(&sections)?, force)
            .map(|_| ExitCode::SUCCESS),
        PresetCmd::List => cmd_preset_list(o, &dir).map(|_| ExitCode::SUCCESS),
        PresetCmd::Show { name } => cmd_preset_show(&dir, &name).map(|_| ExitCode::SUCCESS),
        PresetCmd::Apply {
            names,
            dry_run,
            yes,
            flash,
            flash_effect,
            flash_speed,
            flash_ms,
        } => {
            let flash = match flash {
                Some(c) => Some(FlashOpts {
                    color: led::parse_hex_color(&c)
                        .with_context(|| format!("--flash {c:?} is not #RRGGBB"))?,
                    effect: flash_effect,
                    speed: flash_speed,
                    duration: Duration::from_millis(flash_ms),
                }),
                None => None,
            };
            cmd_preset_apply(o, &dir, &names, dry_run, yes, flash)
        }
        PresetCmd::Current => cmd_preset_current(o, &dir),
        PresetCmd::ImportBackup {
            file,
            name,
            sections,
            force,
        } => cmd_preset_import(o, &dir, &file, &name, &parse_sections(&sections)?, force)
            .map(|_| ExitCode::SUCCESS),
    }
}

/// Exit code for an error: 4 when the board is not connected, else 2.
fn error_code(e: &anyhow::Error) -> u8 {
    let not_found = e.chain().any(|c| {
        matches!(
            c.downcast_ref::<type84_hid::Error>(),
            Some(type84_hid::Error::NotFound)
        )
    });
    if not_found {
        EXIT_NOT_FOUND
    } else {
        EXIT_ERROR
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let o = Out {
        verbose: cli.verbose,
        quiet: cli.quiet,
        json: cli.json,
    };
    let r = match cli.cmd {
        Cmd::Info => cmd_info(o).map(|_| ExitCode::SUCCESS),
        Cmd::Dump => cmd_dump(o).map(|_| ExitCode::SUCCESS),
        Cmd::Backup { output } => cmd_backup(o, output).map(|_| ExitCode::SUCCESS),
        Cmd::VerifyBackup { file } => cmd_verify(o, file),
        Cmd::Preset { cmd } => cmd_preset(o, &cli.presets_dir, cmd),
        Cmd::Restore {
            file,
            only,
            dry_run,
            yes,
        } => cmd_restore(o, &file, only, dry_run, yes),
        Cmd::Anim { cmd } => match cmd {
            AnimCmd::Flash {
                effect,
                color,
                brightness,
                speed,
                duration,
                dry_run,
                yes,
            } => cmd_anim_flash(
                o,
                FlashArgs {
                    effect: &effect,
                    color: color.as_deref(),
                    brightness,
                    speed,
                    duration,
                    dry_run,
                    yes,
                },
            ),
            AnimCmd::Restore { dry_run, yes } => cmd_anim_restore(o, dry_run, yes),
        },
    };
    match r {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(error_code(&e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_maps_to_exit_4() {
        let e = anyhow::Error::from(type84_hid::Error::NotFound).context("opening");
        assert_eq!(error_code(&e), EXIT_NOT_FOUND);
        let e = anyhow::anyhow!("anything else");
        assert_eq!(error_code(&e), EXIT_ERROR);
    }

    #[test]
    fn cli_parses_documented_commands() {
        for args in [
            &["type84", "preset", "apply", "cs2", "fn-fbx50c", "--dry-run"][..],
            &["type84", "preset", "apply", "cs2", "--yes", "--quiet"],
            &["type84", "preset", "current", "--json"],
            &[
                "type84",
                "preset",
                "save",
                "x",
                "--sections",
                "hall,fn_layer,lighting",
            ],
            &[
                "type84",
                "restore",
                "b.json",
                "--only",
                "fn-layer",
                "--dry-run",
            ],
            &[
                "type84",
                "anim",
                "flash",
                "--effect",
                "wave",
                "--color",
                "#00A0FF",
                "--duration",
                "1.5",
                "--dry-run",
            ],
            &["type84", "anim", "restore", "--yes"],
            &["type84", "info", "-q"],
            &["type84", "dump", "--json"],
        ] {
            Cli::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
        assert!(Cli::try_parse_from(["type84", "anim", "flash", "--duration", "0"]).is_err());
        assert!(Cli::try_parse_from(["type84", "restore", "b.json", "--only", "dks"]).is_err());
    }
}
