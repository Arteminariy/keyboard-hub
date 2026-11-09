//! Opt-in write session.
//!
//! This is the only code in the workspace that can send a write frame. It is
//! deliberately separate from [`crate::ReadOnlyTransport`]:
//! - It is opened only through [`crate::Hid::open_write_session`] (or
//!   [`WriteSession::new`]), both of which demand a [`WriteOptIn`] token. The
//!   token names the sections the caller has the user's confirmation for;
//!   each section has its own explicit constructor:
//!   - Hall/RT table: SET_MAGNETIC_AXIS_RT (0x27), VERIFIED on FW 1.17;
//!   - Fn layer: SET_FN_KEY (0x26), UNVERIFIED;
//!   - lighting: SET_LED_EFFECT (0x23) and SET_CUSTOM_LED_DATA (0x24),
//!     UNVERIFIED.
//! - Frame allowlist, checked immediately before every `hid_write`
//!   ([`check_session_frame`]): any valid read frame, plus write frames of
//!   the single section currently being written, and only with the exact
//!   header of a chunk of that section's full block. Feature reports
//!   cannot be sent at all.
//! - It exposes no raw send: the one way to write is
//!   [`WriteSession::apply_plan`] (or its Hall-only wrapper
//!   [`WriteSession::apply_hall_table`]), which runs a fixed sequence:
//!   1. refuse unless every step is covered by the opt-in token and no
//!      section appears twice;
//!   2. read GET_DEVICE_INFO; refuse unless VID/PID match, the board is
//!      magnetic and `firmwareStatus == 0`;
//!   3. run the caller's pre-write backup (it gets a read-only view) and
//!      refuse to continue unless it returns a saved path;
//!   4. for every step: fresh read, refuse if it differs from what the
//!      caller previewed, build and validate the new block; all steps are
//!      validated before the first write; abort if nothing changes;
//!   5. write each changed section in a fixed order (Fn layer, effect,
//!      per-key colours, Hall), full block, one packet in flight, ACK per
//!      packet, 3 resends on timeout, abort on a device-reset frame;
//!   6. after each section: wait (40 ms, or 500 ms for lighting), read it
//!      back and compare (bytes for Hall and the Fn layer; the fields the
//!      firmware keeps for lighting, see `type84_proto::led`).
//!
//! Every error after step 3 carries the pre-write backup path and the
//! restore command for the affected section. No automatic rollback is
//! attempted (it would be a second, equally unverified write).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use type84_proto::device_info::DeviceInfo;
use type84_proto::fn_layer::{self, FnAction};
use type84_proto::frame::{Frame, FRAME_LEN, HEADER_LEN, PAYLOAD_LEN, REQUEST_MARK};
use type84_proto::hall::{self, HallTable, KeyPatch, NonPhysicalPolicy};
use type84_proto::led::{self, EffectPatch};
use type84_proto::opcode::{is_allowed_read_opcode, WriteOp};
use type84_proto::rt::RtScale;
use type84_proto::transfer::{
    read_block, write_block, FrameIo, IoError, TransferError, TransferEvent, WriteEvent,
};
use type84_proto::ReadOp;

use crate::{check_read_frame, Error, RawHid, PRODUCT_ID, VENDOR_ID};

/// Proof that the caller explicitly chose to write, listing the sections it
/// may write. There is no `Default`; each section has one constructor.
#[derive(Debug, Clone)]
pub struct WriteOptIn {
    ops: BTreeSet<WriteOp>,
}

/// Former name, kept for callers that only write the Hall table.
pub type HallWriteOptIn = WriteOptIn;

impl WriteOptIn {
    /// The user confirmed writing the Hall/RT table (0x27).
    pub fn user_confirmed_hall_write() -> Self {
        WriteOptIn {
            ops: BTreeSet::from([WriteOp::AxisRt]),
        }
    }

    /// The user confirmed writing the Fn layer (0x26).
    pub fn user_confirmed_fn_layer_write() -> Self {
        WriteOptIn {
            ops: BTreeSet::from([WriteOp::FnKeymap]),
        }
    }

    /// The user confirmed writing the lighting effect and per-key colours
    /// (0x23, 0x24).
    pub fn user_confirmed_lighting_write() -> Self {
        WriteOptIn {
            ops: BTreeSet::from([WriteOp::LedEffect, WriteOp::CustomLed]),
        }
    }

    /// Union of two confirmations.
    pub fn and(mut self, other: WriteOptIn) -> Self {
        self.ops.extend(other.ops);
        self
    }

    pub fn allows(&self, op: WriteOp) -> bool {
        self.ops.contains(&op)
    }
}

/// Validate a host frame against the write-session policy: any valid read
/// frame, or a chunk of a full block of one of the `allowed` write opcodes
/// (`addr` a multiple of 56 inside the block, `len` = min(56, size - addr),
/// byte 6 = 1 only on the final chunk, zero reserved bytes and padding).
/// Everything else is refused.
pub fn check_session_frame(frame: &Frame, allowed: &[WriteOp]) -> Result<(), Error> {
    if frame[0] != REQUEST_MARK {
        return Err(Error::Refused("byte 0 is not the 0xAA request mark"));
    }
    if is_allowed_read_opcode(frame[1]) {
        return check_read_frame(frame);
    }
    let Some(op) = allowed.iter().copied().find(|op| op.code() == frame[1]) else {
        return Err(Error::OpcodeNotAllowed(frame[1]));
    };
    let size = op.block_size();
    let len = frame[2] as usize;
    let addr = u16::from_le_bytes([frame[3], frame[4]]) as usize;
    if !addr.is_multiple_of(PAYLOAD_LEN) || addr >= size {
        return Err(Error::Refused(
            "write address is not a chunk boundary inside the block",
        ));
    }
    if len != PAYLOAD_LEN.min(size - addr) {
        return Err(Error::Refused(
            "write length does not match the full-block chunk plan",
        ));
    }
    if frame[5] != 0 || frame[7] != 0 {
        return Err(Error::Refused("reserved header bytes must be zero"));
    }
    if frame[6] != (addr + len == size) as u8 {
        return Err(Error::Refused(
            "last-packet flag must be set exactly on the final chunk",
        ));
    }
    if frame[HEADER_LEN + len..].iter().any(|&b| b != 0) {
        return Err(Error::Refused("bytes past the payload length must be zero"));
    }
    Ok(())
}

/// Optional traffic trace: (tag, frame) with tags "TX", "RX", "SKIP".
pub type Trace = Box<dyn FnMut(&'static str, &Frame)>;

/// What to write to the Hall/RT table.
#[derive(Debug, Clone, PartialEq)]
pub enum HallTarget {
    /// Typed settings for some physical keys, merged into the table read
    /// inside the session. Non-physical records, `axis_type` and unknown
    /// flag bits stay as read.
    Patch(Vec<(u8, KeyPatch)>),
    /// A full raw block (restore from a backup), written verbatim.
    /// `rt_precision` is the device-info byte 29 the block was read with;
    /// the write is refused if the board reports a different one.
    Raw { block: Vec<u8>, rt_precision: u8 },
}

/// What to write to the Fn layer.
#[derive(Debug, Clone, PartialEq)]
pub enum FnLayerTarget {
    /// Per-key actions merged into the layer read inside the session
    /// (`type84_proto::fn_layer::apply_patch`).
    Patch(Vec<(u8, FnAction)>),
    /// A full raw layer (restore from a backup).
    Raw(Vec<u8>),
}

/// What to write to the lighting effect.
#[derive(Debug, Clone, PartialEq)]
pub enum EffectTarget {
    Patch(EffectPatch),
    /// A raw 16-byte block (restore); re-encoded the way the hub writes it.
    Raw(Vec<u8>),
    /// A raw 16-byte block written even when it equals the readback. The
    /// board does not report the colour (PROTOCOL.md §3.5), so a block whose
    /// only change is the colour can look unchanged; the caller decides
    /// from its own record of the last write.
    Forced(Vec<u8>),
}

/// What to write to the per-key colours.
#[derive(Debug, Clone, PartialEq)]
pub enum CustomLedTarget {
    Patch(Vec<(u8, [u8; 3])>),
    /// A raw 512-byte block (restore); re-encoded with `ledId = index`.
    Raw(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteTarget {
    Hall(HallTarget),
    FnLayer(FnLayerTarget),
    LedEffect(EffectTarget),
    CustomLed(CustomLedTarget),
}

impl WriteTarget {
    pub fn op(&self) -> WriteOp {
        match self {
            WriteTarget::Hall(_) => WriteOp::AxisRt,
            WriteTarget::FnLayer(_) => WriteOp::FnKeymap,
            WriteTarget::LedEffect(_) => WriteOp::LedEffect,
            WriteTarget::CustomLed(_) => WriteOp::CustomLed,
        }
    }
}

/// One section of a write plan.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteStep {
    pub target: WriteTarget,
    /// The block the user saw when confirming. If set and the fresh read
    /// differs (e.g. the case switch was moved), nothing is written.
    pub expected_base: Option<Vec<u8>>,
}

/// Fixed write order. Unlike the hub's snapshot restore (PROTOCOL.md §7.2),
/// the effect goes before the per-key colours, so the board is already in
/// the custom mode when the colours land (GET_CUSTOM_LED_DATA returns the
/// stored block in any mode, VERIFIED on FW 1.17, so the readback itself
/// does not depend on the order).
pub fn write_rank(op: WriteOp) -> u8 {
    match op {
        WriteOp::FnKeymap => 0,
        WriteOp::LedEffect => 1,
        WriteOp::CustomLed => 2,
        WriteOp::AxisRt => 3,
    }
}

/// The `type84 restore --only` value that undoes a section.
pub fn restore_section(op: WriteOp) -> &'static str {
    match op {
        WriteOp::AxisRt => "hall",
        WriteOp::FnKeymap => "fn-layer",
        WriteOp::LedEffect | WriteOp::CustomLed => "lighting",
    }
}

/// Whether a readback confirms a write of `op`. For the effect this compares
/// only what the board reports (`led::effect_verify_eq`: not the colour or
/// colour mode, custom 128 reads back as 20).
pub fn readback_matches(op: WriteOp, written: &[u8], read_back: &[u8]) -> bool {
    match op {
        WriteOp::LedEffect => led::effect_verify_eq(written, read_back),
        _ => blocks_equal(op, written, read_back),
    }
}

/// Whether two blocks of `op` are equal as far as the firmware keeps them
/// (diffs, skip decisions, preview checks). The effect comparison includes
/// the colour, which the board does not report (`led::effect_fields_eq`).
pub fn blocks_equal(op: WriteOp, a: &[u8], b: &[u8]) -> bool {
    match op {
        WriteOp::LedEffect => led::effect_fields_eq(a, b),
        WriteOp::CustomLed => led::custom_colors_eq(a, b),
        WriteOp::FnKeymap | WriteOp::AxisRt => a == b,
    }
}

/// Record indices that differ between two blocks of `op`.
pub fn changed_records(op: WriteOp, a: &[u8], b: &[u8]) -> Vec<usize> {
    match op {
        WriteOp::LedEffect => {
            if led::effect_fields_eq(a, b) {
                vec![]
            } else {
                vec![0]
            }
        }
        WriteOp::CustomLed => led::custom_changed(a, b),
        WriteOp::FnKeymap => fn_layer::changed_indices(a, b),
        WriteOp::AxisRt => (0..type84_proto::rt::RECORDS)
            .filter(|&i| a.get(i * 8..i * 8 + 8) != b.get(i * 8..i * 8 + 8))
            .collect(),
    }
}

/// Outcome of one section.
#[derive(Debug, Clone)]
pub struct StepReport {
    pub op: WriteOp,
    pub before: Vec<u8>,
    pub written: Vec<u8>,
    pub changed: Vec<usize>,
    /// 0 when the section already matched and was skipped.
    pub frames_written: usize,
    /// Caveats of the readback, e.g. that the colour could not be verified.
    pub notes: Vec<String>,
    /// The block read back after the write (`before` when skipped).
    pub read_back: Vec<u8>,
}

/// Note added to an effect write whose colour the readback cannot confirm.
pub const COLOUR_NOT_VERIFIABLE: &str =
    "colour not verifiable by readback: the board does not report the colour or colour mode (it applies them)";

/// Result of a successful plan.
#[derive(Debug, Clone)]
pub struct WriteReport {
    pub backup_path: PathBuf,
    pub steps: Vec<StepReport>,
    pub rt_precision: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HallWriteRequest {
    pub target: HallTarget,
    pub expected_base: Option<Vec<u8>>,
}

/// Result of a successful Hall write.
#[derive(Debug, Clone)]
pub struct HallWriteReport {
    pub backup_path: PathBuf,
    pub before: Vec<u8>,
    pub written: Vec<u8>,
    pub changed: Vec<usize>,
    pub frames_written: usize,
    pub rt_precision: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("reading the board before the write failed (nothing written): {0}")]
    PreRead(#[source] TransferError),
    #[error("refusing to write (nothing written): {0}")]
    Refused(String),
    #[error("pre-write backup failed (nothing written): {0}")]
    Backup(#[source] IoError),
    #[error("reading {} failed (nothing written; pre-write backup {}): {source}", op.read_op().name(), backup.display())]
    BaseRead {
        backup: PathBuf,
        op: WriteOp,
        source: TransferError,
    },
    #[error("{} on the board changed since it was previewed (nothing written; pre-write backup {}); re-run the dry run", op.read_op().name(), backup.display())]
    BaseChanged { backup: PathBuf, op: WriteOp },
    #[error("invalid {} data (nothing written; pre-write backup {}): {msg}", op.name(), backup.display())]
    Invalid {
        backup: PathBuf,
        op: WriteOp,
        msg: String,
    },
    #[error("the board already holds this; nothing to write (pre-write backup {})", backup.display())]
    NoChanges { backup: PathBuf },
    #[error("WRITE FAILED, {} may be partially written{}: {source}. Restore with `type84 restore {} --only {}`", op.name(), done_note(done), backup.display(), restore_section(*op))]
    WriteFailed {
        backup: PathBuf,
        op: WriteOp,
        /// Sections written and verified before the failure.
        done: Vec<WriteOp>,
        source: TransferError,
    },
    #[error("{} sent but the readback failed{}: {source}. Check with `type84 verify-backup`; restore with `type84 restore {} --only {}`", op.name(), done_note(done), backup.display(), restore_section(*op))]
    ReadbackFailed {
        backup: PathBuf,
        op: WriteOp,
        done: Vec<WriteOp>,
        source: TransferError,
    },
    #[error("READBACK MISMATCH after {} at record indices {indices:?}{}. Restore with `type84 restore {} --only {}`", op.name(), done_note(done), backup.display(), restore_section(*op))]
    ReadbackMismatch {
        backup: PathBuf,
        op: WriteOp,
        done: Vec<WriteOp>,
        indices: Vec<usize>,
        read_back: Vec<u8>,
    },
}

fn done_note(done: &[WriteOp]) -> String {
    if done.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = done.iter().map(|o| o.name()).collect();
        format!(" (already written and verified: {})", names.join(", "))
    }
}

impl ApplyError {
    /// The pre-write backup, when one was saved before the error.
    pub fn backup_path(&self) -> Option<&std::path::Path> {
        match self {
            ApplyError::PreRead(_) | ApplyError::Refused(_) | ApplyError::Backup(_) => None,
            ApplyError::BaseRead { backup, .. }
            | ApplyError::BaseChanged { backup, .. }
            | ApplyError::Invalid { backup, .. }
            | ApplyError::NoChanges { backup }
            | ApplyError::WriteFailed { backup, .. }
            | ApplyError::ReadbackFailed { backup, .. }
            | ApplyError::ReadbackMismatch { backup, .. } => Some(backup),
        }
    }
}

/// Frame I/O over the session's device with a fixed policy: reads, plus
/// write frames of at most one section.
struct SessionIo<'a, D: RawHid> {
    dev: &'a D,
    allow_write: Option<WriteOp>,
    trace: &'a mut Option<Trace>,
}

impl<D: RawHid> SessionIo<'_, D> {
    fn send(&mut self, frame: &Frame) -> Result<(), Error> {
        match self.allow_write {
            Some(op) => check_session_frame(frame, &[op])?,
            None => check_read_frame(frame)?,
        }
        let mut buf = [0u8; FRAME_LEN + 1];
        buf[1..].copy_from_slice(frame);
        let n = self.dev.write(&buf)?;
        if n < FRAME_LEN {
            return Err(Error::ShortWrite {
                written: n,
                expected: FRAME_LEN + 1,
            });
        }
        Ok(())
    }

    fn recv_frame(&mut self, timeout: Duration) -> Result<Option<Frame>, Error> {
        let ms = if timeout.is_zero() {
            0
        } else {
            timeout.as_millis().clamp(1, i32::MAX as u128) as i32
        };
        let mut buf = [0u8; FRAME_LEN + 1];
        let n = self.dev.read_timeout(&mut buf, ms)?;
        if n == 0 {
            return Ok(None);
        }
        let mut f = [0u8; FRAME_LEN];
        let n = n.min(FRAME_LEN);
        f[..n].copy_from_slice(&buf[..n]);
        Ok(Some(f))
    }
}

impl<D: RawHid> FrameIo for SessionIo<'_, D> {
    fn send_request(&mut self, frame: &Frame) -> Result<(), IoError> {
        self.send(frame).map_err(|e| Box::new(e) as IoError)?;
        if let Some(t) = self.trace.as_mut() {
            t("TX", frame);
        }
        Ok(())
    }
    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, IoError> {
        let f = self
            .recv_frame(timeout)
            .map_err(|e| Box::new(e) as IoError)?;
        if let (Some(t), Some(f)) = (self.trace.as_mut(), f.as_ref()) {
            t("RX", f);
        }
        Ok(f)
    }
}

/// Session that can write the sections named by its [`WriteOptIn`], and
/// nothing else.
pub struct WriteSession<D: RawHid> {
    dev: D,
    opt_in: WriteOptIn,
    trace: Option<Trace>,
}

/// A validated step, ready to write.
struct Prepared {
    op: WriteOp,
    before: Vec<u8>,
    new: Vec<u8>,
    changed: Vec<usize>,
}

impl<D: RawHid> WriteSession<D> {
    /// Wrap an already-selected configuration-channel handle.
    pub fn new(dev: D, opt_in: WriteOptIn) -> Self {
        WriteSession {
            dev,
            opt_in,
            trace: None,
        }
    }

    /// Log every frame sent and received.
    pub fn set_trace(&mut self, trace: Trace) {
        self.trace = Some(trace);
    }

    fn io(&mut self, allow_write: Option<WriteOp>) -> SessionIo<'_, D> {
        SessionIo {
            dev: &self.dev,
            allow_write,
            trace: &mut self.trace,
        }
    }

    fn read(&mut self, op: ReadOp) -> Result<Vec<u8>, TransferError> {
        let mut io = self.io(None);
        Ok(read_block(&mut io, op, 0, op.block_size(), &mut |_: TransferEvent| {})?.data)
    }

    /// Hall-only wrapper around [`WriteSession::apply_plan`].
    pub fn apply_hall_table(
        &mut self,
        req: &HallWriteRequest,
        pre_write_backup: &mut dyn FnMut(&mut dyn FrameIo) -> Result<PathBuf, IoError>,
    ) -> Result<HallWriteReport, ApplyError> {
        let plan = [WriteStep {
            target: WriteTarget::Hall(req.target.clone()),
            expected_base: req.expected_base.clone(),
        }];
        let rep = self.apply_plan(&plan, pre_write_backup)?;
        let s = rep.steps.into_iter().next().expect("one step");
        Ok(HallWriteReport {
            backup_path: rep.backup_path,
            before: s.before,
            written: s.written,
            changed: s.changed,
            frames_written: s.frames_written,
            rt_precision: rep.rt_precision,
        })
    }

    /// Run the full guarded write sequence (module docs). `pre_write_backup`
    /// receives a read-only frame channel and must save a complete backup,
    /// returning its path; no write frame is sent unless it succeeds.
    pub fn apply_plan(
        &mut self,
        plan: &[WriteStep],
        pre_write_backup: &mut dyn FnMut(&mut dyn FrameIo) -> Result<PathBuf, IoError>,
    ) -> Result<WriteReport, ApplyError> {
        // 1. Plan shape and opt-in, before any I/O.
        if plan.is_empty() {
            return Err(ApplyError::Refused("empty write plan".into()));
        }
        let mut seen = BTreeSet::new();
        for s in plan {
            let op = s.target.op();
            if !self.opt_in.allows(op) {
                return Err(ApplyError::Refused(format!(
                    "{} is not covered by this session's opt-in",
                    op.name()
                )));
            }
            if !seen.insert(op) {
                return Err(ApplyError::Refused(format!(
                    "{} appears twice in the plan",
                    op.name()
                )));
            }
        }
        let mut order: Vec<&WriteStep> = plan.iter().collect();
        order.sort_by_key(|s| write_rank(s.target.op()));

        // 2. Identity and firmware state.
        let raw_info = self.read(ReadOp::DeviceInfo).map_err(ApplyError::PreRead)?;
        let info = DeviceInfo::decode(&raw_info).map_err(|e| ApplyError::Refused(e.to_string()))?;
        if (info.vendor_id, info.product_id) != (VENDOR_ID, PRODUCT_ID) {
            return Err(ApplyError::Refused(format!(
                "device info reports {:04x}:{:04x}, not a Type 84",
                info.vendor_id, info.product_id
            )));
        }
        if !info.is_magnetic() {
            return Err(ApplyError::Refused(
                "device info reports a non-magnetic board".into(),
            ));
        }
        if !info.is_operational() {
            return Err(ApplyError::Refused(format!(
                "firmwareStatus = {} (firmware busy)",
                info.firmware_status
            )));
        }
        for s in &order {
            check_raw_shape(&s.target, info.rt_precision)?;
        }
        let scale = RtScale::from_precision(info.rt_precision);

        // 3. Mandatory pre-write backup through a read-only view.
        let backup = {
            let mut ro = self.io(None);
            pre_write_backup(&mut ro).map_err(ApplyError::Backup)?
        };

        // 4. Fresh reads, preview checks, merge, validation, diff (all
        //    sections before the first write).
        let mut prepared = Vec::with_capacity(order.len());
        let mut final_effect_mode = None;
        for s in &order {
            let p = self.prepare(s, scale, &backup)?;
            if p.op == WriteOp::LedEffect {
                final_effect_mode = Some(p.new[0]);
            }
            prepared.push(p);
        }
        if let Some(p) = prepared
            .iter()
            .find(|p| p.op == WriteOp::CustomLed && !p.changed.is_empty())
        {
            let mode = match final_effect_mode {
                Some(m) => m,
                None => self
                    .read(ReadOp::LedEffect)
                    .map_err(|source| ApplyError::BaseRead {
                        backup: backup.clone(),
                        op: WriteOp::LedEffect,
                        source,
                    })?[0],
            };
            if !led::is_custom_mode(mode) {
                return Err(ApplyError::Invalid {
                    backup,
                    op: p.op,
                    msg: led::LedError::NotCustom(format!(
                        "{} (mode {mode})",
                        led::effect_name(mode)
                    ))
                    .to_string(),
                });
            }
        }
        if prepared.iter().all(|p| p.changed.is_empty()) {
            return Err(ApplyError::NoChanges { backup });
        }

        // 5./6. Write each changed section, then read it back.
        let mut done = Vec::new();
        let mut steps = Vec::with_capacity(prepared.len());
        for p in prepared {
            if p.changed.is_empty() {
                steps.push(StepReport {
                    op: p.op,
                    read_back: p.before.clone(),
                    before: p.before,
                    written: p.new,
                    changed: vec![],
                    frames_written: 0,
                    notes: vec![],
                });
                continue;
            }
            let op = p.op;
            let acks = {
                let mut rw = self.io(Some(op));
                write_block(&mut rw, op, 0, &p.new, &mut |_: WriteEvent| {}).map_err(|source| {
                    ApplyError::WriteFailed {
                        backup: backup.clone(),
                        op,
                        done: done.clone(),
                        source,
                    }
                })?
            };
            std::thread::sleep(op.settle_delay());
            let read_back =
                self.read(op.read_op())
                    .map_err(|source| ApplyError::ReadbackFailed {
                        backup: backup.clone(),
                        op,
                        done: done.clone(),
                        source,
                    })?;
            if !readback_matches(op, &p.new, &read_back) {
                return Err(ApplyError::ReadbackMismatch {
                    backup,
                    op,
                    indices: changed_records(op, &read_back, &p.new),
                    done,
                    read_back,
                });
            }
            let mut notes = Vec::new();
            // Always, even when the report happens to match: the board
            // reports `FF FF FF` / multicolour whatever was written.
            if op == WriteOp::LedEffect {
                notes.push(COLOUR_NOT_VERIFIABLE.to_string());
            }
            done.push(op);
            steps.push(StepReport {
                op,
                before: p.before,
                written: p.new,
                changed: p.changed,
                frames_written: acks.len(),
                notes,
                read_back,
            });
        }
        Ok(WriteReport {
            backup_path: backup,
            steps,
            rt_precision: info.rt_precision,
        })
    }

    fn prepare(
        &mut self,
        s: &WriteStep,
        scale: RtScale,
        backup: &std::path::Path,
    ) -> Result<Prepared, ApplyError> {
        let op = s.target.op();
        let backup = backup.to_path_buf();
        let before = self
            .read(op.read_op())
            .map_err(|source| ApplyError::BaseRead {
                backup: backup.clone(),
                op,
                source,
            })?;
        // Compared the way the firmware keeps the block: an effect block that
        // was just written reads back with bytes 4/13..15 zeroed.
        if s.expected_base
            .as_ref()
            .is_some_and(|e| !blocks_equal(op, e, &before))
        {
            return Err(ApplyError::BaseChanged { backup, op });
        }
        let invalid = |msg: String| ApplyError::Invalid {
            backup: backup.clone(),
            op,
            msg,
        };
        let new: Vec<u8> = match &s.target {
            WriteTarget::Hall(t) => {
                let base = HallTable::from_block(&before).map_err(|e| invalid(e.to_string()))?;
                let new = match t {
                    HallTarget::Patch(p) => base
                        .apply_patch(p, scale)
                        .map_err(|e| invalid(e.to_string()))?,
                    HallTarget::Raw { block, .. } => {
                        let t = HallTable::from_block(block).map_err(|e| invalid(e.to_string()))?;
                        hall::validate_write(&base, &t, scale, NonPhysicalPolicy::AllowFromBackup)
                            .map_err(|e| invalid(e.to_string()))?;
                        t
                    }
                };
                new.to_block().to_vec()
            }
            WriteTarget::FnLayer(t) => match t {
                FnLayerTarget::Patch(p) => {
                    let factory = self.read(ReadOp::DefaultFnKeymap).map_err(|source| {
                        ApplyError::BaseRead {
                            backup: backup.clone(),
                            op,
                            source,
                        }
                    })?;
                    fn_layer::apply_patch(&before, &factory, p)
                        .map_err(|e| invalid(e.to_string()))?
                }
                FnLayerTarget::Raw(block) => {
                    fn_layer::validate_write(&before, block).map_err(|e| invalid(e.to_string()))?;
                    block.clone()
                }
            },
            WriteTarget::LedEffect(t) => match t {
                EffectTarget::Patch(p) => p.apply(&before).map_err(|e| invalid(e.to_string()))?,
                EffectTarget::Raw(b) | EffectTarget::Forced(b) => {
                    led::normalise_effect(b).map_err(|e| invalid(e.to_string()))?
                }
            }
            .to_vec(),
            WriteTarget::CustomLed(t) => match t {
                CustomLedTarget::Patch(p) => {
                    led::apply_custom_patch(&before, p).map_err(|e| invalid(e.to_string()))?
                }
                CustomLedTarget::Raw(b) => {
                    led::normalise_custom(b).map_err(|e| invalid(e.to_string()))?
                }
            },
        };
        if new.len() != op.block_size() {
            return Err(invalid(format!(
                "block is {} bytes, expected {}",
                new.len(),
                op.block_size()
            )));
        }
        let mut changed = changed_records(op, &before, &new);
        if changed.is_empty() && matches!(s.target, WriteTarget::LedEffect(EffectTarget::Forced(_)))
        {
            changed = vec![0];
        }
        Ok(Prepared {
            op,
            before,
            new,
            changed,
        })
    }
}

fn check_raw_shape(t: &WriteTarget, board_precision: u8) -> Result<(), ApplyError> {
    let (len, want) = match t {
        WriteTarget::Hall(HallTarget::Raw {
            block,
            rt_precision,
        }) => {
            if *rt_precision != board_precision {
                return Err(ApplyError::Refused(format!(
                    "the block was read with rtPrecision {rt_precision}, the board reports {board_precision}; raw units differ"
                )));
            }
            (block.len(), WriteOp::AxisRt.block_size())
        }
        WriteTarget::FnLayer(FnLayerTarget::Raw(b)) => (b.len(), WriteOp::FnKeymap.block_size()),
        WriteTarget::LedEffect(EffectTarget::Raw(b) | EffectTarget::Forced(b)) => {
            (b.len(), WriteOp::LedEffect.block_size())
        }
        WriteTarget::CustomLed(CustomLedTarget::Raw(b)) => {
            (b.len(), WriteOp::CustomLed.block_size())
        }
        _ => return Ok(()),
    };
    if len != want {
        return Err(ApplyError::Refused(format!(
            "raw {} block is {len} bytes, expected {want}",
            t.op().name()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use type84_proto::frame::encode_read_request;
    use type84_proto::hall::{RtSensitivity, WRITE_FRAME_COUNT};
    use type84_proto::transfer::write_frames;

    /// Simulated board behind the raw HID interface.
    #[derive(Default)]
    struct Board {
        images: HashMap<u8, Vec<u8>>,
        queue: VecDeque<Frame>,
        written: Vec<Frame>,
        write_packets: usize,
        /// Send a device-reset frame instead of the ACK of this write packet.
        reset_on_write: Option<usize>,
        /// Ignore this (opcode, byte) of a written image (readback mismatch).
        stuck_byte: Option<(u8, usize)>,
    }

    struct MockDev(RefCell<Board>);

    impl RawHid for MockDev {
        fn write(&self, data: &[u8]) -> Result<usize, hidapi::HidError> {
            let mut b = self.0.borrow_mut();
            assert_eq!(data.len(), 65);
            assert_eq!(data[0], 0);
            let mut f = [0u8; 64];
            f.copy_from_slice(&data[1..]);
            b.written.push(f);
            let (op, len) = (f[1], f[2] as usize);
            let addr = u16::from_le_bytes([f[3], f[4]]) as usize;
            let mut r = f;
            r[0] = 0x55;
            if WriteOp::from_code(op).is_some() {
                b.write_packets += 1;
                if b.reset_on_write == Some(b.write_packets) {
                    let mut n = [0u8; 64];
                    n[..3].copy_from_slice(&[0x55, 0xFC, 0x05]);
                    b.queue.push_back(n);
                    return Ok(65);
                }
                let stuck = b.stuck_byte;
                let img = b.images.get_mut(&(op - 0x10)).unwrap();
                for k in 0..len {
                    if stuck != Some((op, addr + k)) {
                        img[addr + k] = f[8 + k];
                    }
                }
                // Emulate what the real board reads back (FW 1.17): the
                // effect's byte 4 and check code read 0, the colour reads
                // white multicolour whatever was written, and custom 128
                // reads back as 20. Per-key records read back as written.
                if op == 0x23 {
                    for k in [4, 13, 14, 15] {
                        img[k] = 0;
                    }
                    img[1..4].copy_from_slice(&[0xFF; 3]);
                    img[8] = 1;
                    if img[0] == 128 {
                        img[0] = 20;
                    }
                }
            } else {
                let img = &b.images[&op];
                r[8..8 + len].copy_from_slice(&img[addr..addr + len]);
            }
            b.queue.push_back(r);
            Ok(65)
        }
        fn read_timeout(&self, buf: &mut [u8], _ms: i32) -> Result<usize, hidapi::HidError> {
            match self.0.borrow_mut().queue.pop_front() {
                Some(f) => {
                    buf[..64].copy_from_slice(&f);
                    Ok(64)
                }
                None => Ok(0),
            }
        }
    }

    const FIXTURE: &str =
        include_str!("../../type84-proto/tests/fixtures/type84-20261109-220355-fw1.17.json");

    fn fixture_block(name: &str) -> Vec<u8> {
        let v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        hex::decode(v["blocks"][name]["data"].as_str().unwrap()).unwrap()
    }

    fn board() -> MockDev {
        let mut b = Board::default();
        for op in ReadOp::ALL {
            b.images.insert(op.code(), vec![0u8; op.block_size()]);
        }
        let info = hex::decode("00000092450cd6801701000066010c110a0000000f").unwrap();
        b.images.get_mut(&0x10).unwrap()[..info.len()].copy_from_slice(&info);
        let rt = b.images.get_mut(&0x17).unwrap();
        for i in 0..128 {
            rt[i * 8 + 2] = 120; // switch preset 1.20 mm on all records
        }
        b.images.insert(0x16, fixture_block("keymap_layer1"));
        b.images.insert(0x1C, fixture_block("default_fn_keymap"));
        b.images.insert(
            0x13,
            hex::decode("0fffffff000000000105040000000000").unwrap(),
        );
        MockDev(RefCell::new(b))
    }

    fn session(dev: MockDev) -> WriteSession<MockDev> {
        let all = WriteOptIn::user_confirmed_hall_write()
            .and(WriteOptIn::user_confirmed_fn_layer_write())
            .and(WriteOptIn::user_confirmed_lighting_write());
        WriteSession::new(dev, all)
    }

    fn w_patch() -> HallWriteRequest {
        let p = KeyPatch {
            actuation_mm: 0.8,
            rt: Some(RtSensitivity {
                press_mm: 0.3,
                release_mm: 0.3,
            }),
            whole_travel: true,
        };
        HallWriteRequest {
            target: HallTarget::Patch(vec![(34, p)]),
            expected_base: None,
        }
    }

    fn fn_step() -> WriteStep {
        let a = |s: &str| FnAction::parse(s).unwrap();
        WriteStep {
            target: WriteTarget::FnLayer(FnLayerTarget::Patch(vec![
                (1, a("consumer:WWWHome")),
                (2, a("key:Ctrl+F10")),
                (17, a("key:F13")),
            ])),
            expected_base: None,
        }
    }

    fn ok_backup(
        calls: &RefCell<usize>,
    ) -> impl FnMut(&mut dyn FrameIo) -> Result<PathBuf, IoError> + '_ {
        move |io: &mut dyn FrameIo| {
            *calls.borrow_mut() += 1;
            // The backup gets a read-only view: reads work, writes are refused.
            read_block(io, ReadOp::Keymap, 0, 512, &mut |_| {})?;
            for op in [0x23u8, 0x24, 0x26, 0x27] {
                let mut w = [0u8; 64];
                w[..8].copy_from_slice(&[0xAA, op, 0x38, 0, 0, 0, 0, 0]);
                assert!(io.send_request(&w).is_err(), "backup view must not write");
            }
            Ok(PathBuf::from("/tmp/backups/auto/pre.json"))
        }
    }

    fn write_frames_of(s: &WriteSession<MockDev>, op: u8) -> usize {
        s.dev
            .0
            .borrow()
            .written
            .iter()
            .filter(|f| f[1] == op)
            .count()
    }

    fn all_write_frames(s: &WriteSession<MockDev>) -> usize {
        [0x23, 0x24, 0x26, 0x27]
            .into_iter()
            .map(|op| write_frames_of(s, op))
            .sum()
    }

    fn write_ops(s: &WriteSession<MockDev>) -> Vec<u8> {
        s.dev
            .0
            .borrow()
            .written
            .iter()
            .map(|f| f[1])
            .filter(|&o| WriteOp::from_code(o).is_some())
            .collect()
    }

    /// Sweep all 256 opcodes, read-shaped and write-shaped, for the
    /// read-only view and for each single-section write policy.
    #[test]
    fn session_allowlist_sweeps_all_256_opcodes() {
        for allow in [
            None,
            Some(WriteOp::AxisRt),
            Some(WriteOp::FnKeymap),
            Some(WriteOp::LedEffect),
            Some(WriteOp::CustomLed),
        ] {
            let dev = board();
            let mut trace = None;
            let mut accepted = 0;
            {
                let mut io = SessionIo {
                    dev: &dev,
                    allow_write: allow,
                    trace: &mut trace,
                };
                for op in 0u8..=255 {
                    // Read-shaped frame (16-byte single packet, zero payload).
                    // For the 16-byte effect section this is also a valid
                    // (all-zero) full-block chunk.
                    let mut f = [0u8; 64];
                    f[..8].copy_from_slice(&[0xAA, op, 0x10, 0, 0, 0, 1, 0]);
                    let ok = io.send(&f).is_ok();
                    let led_chunk = allow == Some(WriteOp::LedEffect) && op == 0x23;
                    assert_eq!(
                        ok,
                        is_allowed_read_opcode(op) || led_chunk,
                        "{allow:?}: zero payload, opcode {op:#04x}"
                    );
                    accepted += ok as usize;
                    // Write-shaped first chunk of the section's full block.
                    let size = allow.map_or(1024, |o| o.block_size());
                    let first_len = size.min(56);
                    let mut f = [0u8; 64];
                    f[..8].copy_from_slice(&[
                        0xAA,
                        op,
                        first_len as u8,
                        0,
                        0,
                        0,
                        (size <= 56) as u8,
                        0,
                    ]);
                    f[8..8 + first_len].fill(0x5A);
                    let ok = io.send(&f).is_ok();
                    assert_eq!(
                        ok,
                        allow.is_some_and(|o| o.code() == op),
                        "{allow:?}: payload, opcode {op:#04x}"
                    );
                    accepted += ok as usize;
                }
            }
            let written = dev.0.borrow().written.clone();
            assert_eq!(written.len(), accepted);
            let extra = match allow {
                None => 0,
                Some(WriteOp::LedEffect) => 2,
                Some(_) => 1,
            };
            assert_eq!(written.len(), 10 + extra, "{allow:?}");
            assert!(written
                .iter()
                .all(|f| is_allowed_read_opcode(f[1]) || allow.is_some_and(|o| o.code() == f[1])));
            for forbidden in [
                0x0F, 0x1F, 0x21, 0x22, 0x25, 0x28, 0x34, 0x41, 0x50, 0x64, 0x69, 0x82, 0x83, 0x85,
                0x86,
            ] {
                assert!(written.iter().all(|f| f[1] != forbidden));
            }
        }
    }

    #[test]
    fn malformed_write_frames_are_refused() {
        let good = hall::encode_write_frames(&HallTable::from_block(&[1u8; 1024]).unwrap());
        for f in &good {
            check_session_frame(f, &[WriteOp::AxisRt]).unwrap();
            assert!(
                check_session_frame(f, &[WriteOp::FnKeymap]).is_err(),
                "other section"
            );
            assert!(check_session_frame(f, &[]).is_err());
        }
        let mutate = |i: usize, f: &dyn Fn(&mut Frame)| {
            let mut x = good[i];
            f(&mut x);
            check_session_frame(&x, &[WriteOp::AxisRt])
        };
        assert!(mutate(0, &|f| f[3] = 1).is_err(), "unaligned addr");
        assert!(mutate(0, &|f| f[2] = 55).is_err(), "short chunk");
        assert!(mutate(0, &|f| f[6] = 1).is_err(), "early last flag");
        assert!(mutate(18, &|f| f[6] = 0).is_err(), "missing last flag");
        assert!(mutate(18, &|f| f[2] = 56).is_err(), "past block end");
        assert!(mutate(0, &|f| f[5] = 1).is_err());
        assert!(mutate(0, &|f| f[7] = 1).is_err());
        assert!(mutate(18, &|f| f[8 + 16] = 1).is_err(), "data past len");
        assert!(mutate(0, &|f| f[0] = 0x55).is_err());
        assert!(
            mutate(0, &|f| {
                f[3] = 0x00;
                f[4] = 0x04;
            })
            .is_err(),
            "addr 1024"
        );
        // 512-byte sections: 9 × 56 + 8, last chunk at 0x1F8; nothing past 512.
        for op in [WriteOp::FnKeymap, WriteOp::CustomLed] {
            let frames = write_frames(op, 0, &[7u8; 512]).unwrap();
            assert_eq!(frames.len(), 10);
            assert_eq!(&frames[9][..8], &[0xAA, op.code(), 8, 0xF8, 0x01, 0, 1, 0]);
            for f in &frames {
                check_session_frame(f, &[op]).unwrap();
            }
            let mut x = frames[9];
            x[2] = 56;
            assert!(check_session_frame(&x, &[op]).is_err(), "past 512");
            let mut x = frames[0];
            x[3] = 0x00;
            x[4] = 0x02;
            assert!(check_session_frame(&x, &[op]).is_err(), "addr 512");
        }
        // 16-byte effect: exactly one frame, len 16, addr 0, last = 1.
        let f = write_frames(WriteOp::LedEffect, 0, &[1u8; 16]).unwrap();
        assert_eq!(f.len(), 1);
        check_session_frame(&f[0], &[WriteOp::LedEffect]).unwrap();
        let mut x = f[0];
        x[6] = 0;
        assert!(check_session_frame(&x, &[WriteOp::LedEffect]).is_err());
        let mut x = f[0];
        x[2] = 56;
        assert!(check_session_frame(&x, &[WriteOp::LedEffect]).is_err());
        // ISP magic frames.
        for magic in [
            [0xAA, 0x55, 0xA5, 0x5A, 0xFF, 0x00, 0x33, 0xCC],
            [0xAA, 0x42, 0x89, 0x5A, 0xFF, 0x71, 0x62, 0xCC],
        ] {
            let mut f = [0u8; 64];
            f[..8].copy_from_slice(&magic);
            assert!(check_session_frame(&f, &WriteOp::ALL).is_err());
        }
        // Reads keep the read rules: payload on a read is refused.
        let mut r = encode_read_request(ReadOp::AxisRt, 56, 0, false);
        check_session_frame(&r, &[]).unwrap();
        r[8] = 1;
        assert!(check_session_frame(&r, &WriteOp::ALL).is_err());
    }

    #[test]
    fn apply_writes_19_frames_and_verifies() {
        let mut s = session(board());
        let calls = RefCell::new(0);
        let rep = s
            .apply_hall_table(&w_patch(), &mut ok_backup(&calls))
            .unwrap();
        assert_eq!(*calls.borrow(), 1);
        assert_eq!(rep.frames_written, WRITE_FRAME_COUNT);
        assert_eq!(rep.changed, vec![34]);
        assert_eq!(&rep.written[34 * 8..35 * 8], &[0, 1, 80, 0, 30, 0, 30, 0]);
        assert_eq!(write_frames_of(&s, 0x27), 19);
        assert_eq!(all_write_frames(&s), 19);
        let b = s.dev.0.borrow();
        assert_eq!(b.images[&0x17], rep.written);
        // Order: device info, backup reads, Hall read, 19 writes, Hall readback.
        let ops: Vec<u8> = b.written.iter().map(|f| f[1]).collect();
        let first_write = ops.iter().position(|&o| o == 0x27).unwrap();
        assert_eq!(ops[0], 0x10);
        assert!(
            ops[..first_write].contains(&0x12),
            "backup ran before the write"
        );
        assert!(
            ops[first_write + 19..].iter().all(|&o| o == 0x17)
                && ops.len() == first_write + 19 + 19
        );
    }

    #[test]
    fn fn_layer_write_changes_only_listed_records() {
        let mut s = session(board());
        let calls = RefCell::new(0);
        let before = s.dev.0.borrow().images[&0x16].clone();
        let rep = s.apply_plan(&[fn_step()], &mut ok_backup(&calls)).unwrap();
        let st = &rep.steps[0];
        assert_eq!(st.op, WriteOp::FnKeymap);
        assert_eq!(st.frames_written, 10);
        assert_eq!(st.changed, vec![1, 2, 17]);
        let after = s.dev.0.borrow().images[&0x16].clone();
        assert_eq!(fn_layer::changed_indices(&before, &after), vec![1, 2, 17]);
        assert_eq!(&after[8..12], &[2, 1, 0x43, 0]);
        assert_eq!(write_ops(&s), vec![0x26; 10], "no other section written");
        // A second run is a no-op.
        let err = s
            .apply_plan(&[fn_step()], &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(err, ApplyError::NoChanges { .. }));
        assert_eq!(all_write_frames(&s), 10);
    }

    #[test]
    fn opt_in_is_per_section() {
        let mut s = WriteSession::new(board(), WriteOptIn::user_confirmed_hall_write());
        let calls = RefCell::new(0);
        let err = s
            .apply_plan(&[fn_step()], &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(err, ApplyError::Refused(_)), "{err}");
        assert_eq!(*calls.borrow(), 0);
        assert!(
            s.dev.0.borrow().written.is_empty(),
            "refused before any I/O"
        );
        // Duplicate sections are refused too.
        let mut s = session(board());
        let err = s
            .apply_plan(&[fn_step(), fn_step()], &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(err, ApplyError::Refused(_)));
        assert!(s.dev.0.borrow().written.is_empty());
    }

    #[test]
    fn lighting_write_compares_kept_fields_only() {
        let mut s = session(board());
        let calls = RefCell::new(0);
        let step = WriteStep {
            target: WriteTarget::LedEffect(EffectTarget::Patch(EffectPatch {
                mode: Some(11),
                color: Some([0, 0xA0, 0xFF]),
                ..Default::default()
            })),
            expected_base: None,
        };
        // The mock zeroes bytes 4/13/14/15 and reports white multicolour
        // like the real board; the readback check passes with a note.
        let rep = s.apply_plan(&[step], &mut ok_backup(&calls)).unwrap();
        assert_eq!(rep.steps[0].frames_written, 1);
        assert_eq!(rep.steps[0].notes, vec![COLOUR_NOT_VERIFIABLE.to_string()]);
        assert_eq!(
            hex::encode(&rep.steps[0].written),
            "0b00a0ffff000000000504000000aa55"
        );
        let f = s
            .dev
            .0
            .borrow()
            .written
            .iter()
            .find(|f| f[1] == 0x23)
            .copied()
            .unwrap();
        assert_eq!(&f[..8], &[0xAA, 0x23, 0x10, 0, 0, 0, 1, 0]);
        assert_eq!(write_ops(&s), vec![0x23]);
    }

    #[test]
    fn effect_write_then_restore_in_one_session() {
        // The anim flash pattern: write an effect, then restore the old one,
        // using the written block (with its check code) as the expected base.
        let mut s = session(board());
        let calls = RefCell::new(0);
        let old = s.dev.0.borrow().images[&0x13].clone();
        let target = EffectPatch {
            mode: Some(11),
            ..Default::default()
        }
        .apply(&old)
        .unwrap()
        .to_vec();
        let step = |t: &[u8], base: &[u8]| WriteStep {
            target: WriteTarget::LedEffect(EffectTarget::Raw(t.to_vec())),
            expected_base: Some(base.to_vec()),
        };
        s.apply_plan(&[step(&target, &old)], &mut ok_backup(&calls))
            .unwrap();
        s.apply_plan(&[step(&old, &target)], &mut ok_backup(&calls))
            .unwrap();
        assert_eq!(write_ops(&s), vec![0x23, 0x23]);
        assert!(led::effect_fields_eq(&s.dev.0.borrow().images[&0x13], &old));
    }

    #[test]
    fn forced_effect_is_written_even_when_it_reads_the_same() {
        let calls = RefCell::new(0);
        let old = board().0.borrow().images[&0x13].clone();
        // Same as the readback except bytes the board zeroes: Raw skips it,
        // Forced writes it.
        let blk = led::normalise_effect(&old).unwrap().to_vec();
        let mut s = session(board());
        let raw = WriteStep {
            target: WriteTarget::LedEffect(EffectTarget::Raw(blk.clone())),
            expected_base: Some(old.clone()),
        };
        let err = s.apply_plan(&[raw], &mut ok_backup(&calls)).unwrap_err();
        assert!(matches!(err, ApplyError::NoChanges { .. }), "{err}");
        let forced = WriteStep {
            target: WriteTarget::LedEffect(EffectTarget::Forced(blk)),
            expected_base: Some(old.clone()),
        };
        let rep = s.apply_plan(&[forced], &mut ok_backup(&calls)).unwrap();
        assert_eq!(rep.steps[0].frames_written, 1);
        assert_eq!(rep.steps[0].read_back, s.dev.0.borrow().images[&0x13]);
        assert_eq!(write_ops(&s), vec![0x23]);
    }

    #[test]
    fn per_key_colours_need_custom_mode() {
        let calls = RefCell::new(0);
        let colours = WriteStep {
            target: WriteTarget::CustomLed(CustomLedTarget::Patch(vec![(34, [0xFF, 0, 0])])),
            expected_base: None,
        };
        // Board is in mode 15 (circles): refused, nothing written.
        let mut s = session(board());
        let err = s
            .apply_plan(std::slice::from_ref(&colours), &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(err, ApplyError::Invalid { .. }), "{err}");
        assert_eq!(all_write_frames(&s), 0);
        // With the effect switched to custom in the same plan: effect first, then colours.
        let mut s = session(board());
        let effect = WriteStep {
            target: WriteTarget::LedEffect(EffectTarget::Patch(EffectPatch {
                mode: Some(128),
                ..Default::default()
            })),
            expected_base: None,
        };
        let rep = s
            .apply_plan(&[effect, colours], &mut ok_backup(&calls))
            .unwrap();
        let ops: Vec<WriteOp> = rep.steps.iter().map(|s| s.op).collect();
        assert_eq!(ops, vec![WriteOp::LedEffect, WriteOp::CustomLed]);
        // The written white multicolour matches the report by coincidence;
        // the note is printed anyway.
        assert_eq!(rep.steps[0].notes, vec![COLOUR_NOT_VERIFIABLE.to_string()]);
        assert!(rep.steps[1].notes.is_empty());
        {
            let b = s.dev.0.borrow();
            assert_eq!(&b.images[&0x14][34 * 4..35 * 4], &[34, 0xFF, 0, 0]);
            assert_eq!(b.images[&0x13][0], 20, "custom 128 reads back as 20");
        }
        assert_eq!(write_ops(&s), [vec![0x23], vec![0x24; 10]].concat());
        // Re-applying custom on a board that reports 20 changes nothing.
        let again = WriteStep {
            target: WriteTarget::LedEffect(EffectTarget::Patch(EffectPatch {
                mode: Some(128),
                ..Default::default()
            })),
            expected_base: None,
        };
        let err = s.apply_plan(&[again], &mut ok_backup(&calls)).unwrap_err();
        assert!(matches!(err, ApplyError::NoChanges { .. }), "{err}");
    }

    #[test]
    fn multi_section_plan_one_backup_fixed_order() {
        let mut s = session(board());
        let calls = RefCell::new(0);
        let hall = WriteStep {
            target: WriteTarget::Hall(w_patch().target),
            expected_base: None,
        };
        let rep = s
            .apply_plan(&[hall, fn_step()], &mut ok_backup(&calls))
            .unwrap();
        assert_eq!(*calls.borrow(), 1, "one backup for the whole plan");
        let ops: Vec<WriteOp> = rep.steps.iter().map(|s| s.op).collect();
        assert_eq!(ops, vec![WriteOp::FnKeymap, WriteOp::AxisRt]);
        assert_eq!(write_ops(&s), [vec![0x26; 10], vec![0x27; 19]].concat());
    }

    #[test]
    fn invalid_second_section_blocks_the_first() {
        let mut s = session(board());
        let calls = RefCell::new(0);
        let bad_hall = WriteStep {
            target: WriteTarget::Hall(HallTarget::Patch(vec![(
                29,
                KeyPatch {
                    actuation_mm: 1.0,
                    rt: None,
                    whole_travel: false,
                },
            )])),
            expected_base: None,
        };
        let err = s
            .apply_plan(&[fn_step(), bad_hall], &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::Invalid {
                op: WriteOp::AxisRt,
                ..
            }
        ));
        assert_eq!(all_write_frames(&s), 0, "validated before the first write");
    }

    #[test]
    fn no_write_without_backup_or_with_empty_diff_or_busy_firmware() {
        // Backup fails.
        let mut s = session(board());
        let err = s
            .apply_hall_table(&w_patch(), &mut |_: &mut dyn FrameIo| {
                Err("disk full".into())
            })
            .unwrap_err();
        assert!(matches!(err, ApplyError::Backup(_)));
        assert_eq!(all_write_frames(&s), 0);

        // Empty diff.
        let mut s = session(board());
        let calls = RefCell::new(0);
        let same = KeyPatch {
            actuation_mm: 1.2,
            rt: None,
            whole_travel: false,
        };
        let req = HallWriteRequest {
            target: HallTarget::Patch(vec![(34, same)]),
            expected_base: None,
        };
        let err = s
            .apply_hall_table(&req, &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(err, ApplyError::NoChanges { .. }));
        assert_eq!(all_write_frames(&s), 0);

        // firmwareStatus != 0: refused before the backup.
        let dev = board();
        dev.0.borrow_mut().images.get_mut(&0x10).unwrap()[32] = 1;
        let mut s = session(dev);
        let calls = RefCell::new(0);
        let err = s
            .apply_hall_table(&w_patch(), &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(err, ApplyError::Refused(_)));
        assert_eq!(*calls.borrow(), 0);
        assert_eq!(all_write_frames(&s), 0);

        // Board changed since the preview.
        let mut s = session(board());
        let calls = RefCell::new(0);
        let mut req = w_patch();
        req.expected_base = Some(vec![0u8; 1024]);
        let err = s
            .apply_hall_table(&req, &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(err, ApplyError::BaseChanged { .. }));
        assert_eq!(all_write_frames(&s), 0);

        // Invalid patch (non-physical index).
        let mut s = session(board());
        let calls = RefCell::new(0);
        let p = KeyPatch {
            actuation_mm: 1.0,
            rt: None,
            whole_travel: false,
        };
        let req = HallWriteRequest {
            target: HallTarget::Patch(vec![(29, p)]),
            expected_base: None,
        };
        assert!(matches!(
            s.apply_hall_table(&req, &mut ok_backup(&calls)),
            Err(ApplyError::Invalid { .. })
        ));
        assert_eq!(all_write_frames(&s), 0);

        // Raw block with another rtPrecision.
        let mut s = session(board());
        let calls = RefCell::new(0);
        let req = HallWriteRequest {
            target: HallTarget::Raw {
                block: vec![0; 1024],
                rt_precision: 1,
            },
            expected_base: None,
        };
        assert!(matches!(
            s.apply_hall_table(&req, &mut ok_backup(&calls)),
            Err(ApplyError::Refused(_))
        ));
        assert_eq!(all_write_frames(&s), 0);

        // Fn-layer patch on a protected vendor function (Fn+Delete = reset).
        let mut s = session(board());
        let calls = RefCell::new(0);
        let step = WriteStep {
            target: WriteTarget::FnLayer(FnLayerTarget::Patch(vec![(
                106,
                FnAction::parse("key:F1").unwrap(),
            )])),
            expected_base: None,
        };
        assert!(matches!(
            s.apply_plan(&[step], &mut ok_backup(&calls)),
            Err(ApplyError::Invalid { .. })
        ));
        assert_eq!(all_write_frames(&s), 0);
    }

    #[test]
    fn readback_mismatch_reports_backup_path() {
        let dev = board();
        dev.0.borrow_mut().stuck_byte = Some((0x27, 34 * 8 + 2));
        let mut s = session(dev);
        let calls = RefCell::new(0);
        let err = s
            .apply_hall_table(&w_patch(), &mut ok_backup(&calls))
            .unwrap_err();
        match &err {
            ApplyError::ReadbackMismatch { indices, .. } => assert_eq!(indices, &vec![34]),
            e => panic!("unexpected {e}"),
        }
        assert!(err.to_string().contains("/tmp/backups/auto/pre.json"));
        assert!(err.to_string().contains("--only hall"));
        assert_eq!(
            err.backup_path().unwrap(),
            std::path::Path::new("/tmp/backups/auto/pre.json")
        );

        // Fn layer: the error names the fn-layer restore.
        let dev = board();
        dev.0.borrow_mut().stuck_byte = Some((0x26, 2 * 4 + 1));
        let mut s = session(dev);
        let err = s
            .apply_plan(&[fn_step()], &mut ok_backup(&calls))
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::ReadbackMismatch {
                op: WriteOp::FnKeymap,
                ..
            }
        ));
        assert!(err.to_string().contains("--only fn-layer"), "{err}");

        // Effect: a brightness the board did not take is still a mismatch.
        let dev = board();
        dev.0.borrow_mut().stuck_byte = Some((0x23, 9));
        let mut s = session(dev);
        let step = WriteStep {
            target: WriteTarget::LedEffect(EffectTarget::Patch(EffectPatch {
                brightness: Some(2),
                ..Default::default()
            })),
            expected_base: None,
        };
        let err = s.apply_plan(&[step], &mut ok_backup(&calls)).unwrap_err();
        assert!(
            matches!(
                err,
                ApplyError::ReadbackMismatch {
                    op: WriteOp::LedEffect,
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn device_reset_during_write_aborts_with_backup_path() {
        let dev = board();
        dev.0.borrow_mut().reset_on_write = Some(5);
        let mut s = session(dev);
        let calls = RefCell::new(0);
        let err = s
            .apply_hall_table(&w_patch(), &mut ok_backup(&calls))
            .unwrap_err();
        assert!(
            matches!(
                err,
                ApplyError::WriteFailed {
                    source: TransferError::DeviceReset { .. },
                    ..
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("partially written"));
        assert!(err.to_string().contains("/tmp/backups/auto/pre.json"));
        assert_eq!(
            write_frames_of(&s, 0x27),
            5,
            "no resend and no further packets after a reset"
        );
    }

    #[test]
    fn raw_restore_writes_block_verbatim() {
        let mut s = session(board());
        let calls = RefCell::new(0);
        let mut block = vec![0u8; 1024];
        for i in 0..128 {
            block[i * 8 + 2] = 150;
            block[i * 8 + 1] = 0xFC & (i as u8); // unknown flag bits survive
        }
        let req = HallWriteRequest {
            target: HallTarget::Raw {
                block: block.clone(),
                rt_precision: 0,
            },
            expected_base: None,
        };
        let rep = s.apply_hall_table(&req, &mut ok_backup(&calls)).unwrap();
        assert_eq!(rep.written, block);
        assert_eq!(s.dev.0.borrow().images[&0x17], block);
    }
}
