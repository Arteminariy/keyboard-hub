//! How the worker reaches the board. [`HidBackend`] is the real hidapi
//! device; tests plug in a simulated board (`type84_hid::mock`). Either way
//! reads go through `type84_hid`'s read-only transport and writes only
//! through `type84_cli::apply::run_steps` on a `WriteSession` opened with
//! the opt-in for exactly the sections of the plan.

use std::path::{Path, PathBuf};

use type84_cli::apply::{self, Applied};
use type84_hid::{select_config, Hid, Interface, WriteStep};
use type84_proto::transfer::FrameIo;

use crate::worker::Presence;

/// A failed write, with the pre-write backup when one was saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyFail {
    pub message: String,
    pub backup: Option<PathBuf>,
    /// Nothing reached the board (refused before the first write packet).
    pub nothing_written: bool,
    /// The section that failed after writing started (for the restore hint).
    pub op: Option<type84_proto::WriteOp>,
}

impl ApplyFail {
    pub fn before_write(message: impl Into<String>) -> Self {
        ApplyFail {
            message: message.into(),
            backup: None,
            nothing_written: true,
            op: None,
        }
    }

    pub fn from_apply(e: &type84_hid::ApplyError) -> Self {
        use type84_hid::ApplyError as E;
        let op = match e {
            E::WriteFailed { op, .. }
            | E::ReadbackFailed { op, .. }
            | E::ReadbackMismatch { op, .. } => Some(*op),
            _ => None,
        };
        ApplyFail {
            message: e.to_string(),
            backup: e.backup_path().map(Path::to_path_buf),
            nothing_written: op.is_none(),
            op,
        }
    }
}

/// The board as the worker sees it.
pub trait Backend {
    /// Enumerate. `Err` when the HID subsystem itself cannot be opened.
    fn scan(&mut self) -> Result<Option<Presence>, String>;
    /// Open the configuration interface read-only.
    fn open_read(&mut self) -> anyhow::Result<(Interface, Box<dyn FrameIo + '_>)>;
    /// Open a write session for the sections of `steps` and run them with
    /// the automatic pre-write backup (the caller holds the write lock).
    fn apply(
        &mut self,
        steps: &[WriteStep],
        backup_dir: &Path,
        state_dir: &Path,
    ) -> Result<Applied, ApplyFail>;
}

/// The real board through hidapi.
#[derive(Default)]
pub struct HidBackend {
    hid: Option<Hid>,
}

impl HidBackend {
    fn hid(&mut self) -> Result<&mut Hid, String> {
        if self.hid.is_none() {
            // hidapi allows one context per process; keep it and refresh.
            self.hid = Some(Hid::new().map_err(|e| e.to_string())?);
        }
        Ok(self.hid.as_mut().expect("initialised above"))
    }
}

impl Backend for HidBackend {
    fn scan(&mut self) -> Result<Option<Presence>, String> {
        let h = self.hid()?;
        if h.refresh().is_err() {
            return Ok(None);
        }
        Ok(select_config(&h.list_interfaces())
            .ok()
            .map(|iface| Presence {
                product: iface
                    .product
                    .clone()
                    .unwrap_or_else(|| "IO Type 84 Magnetic".into()),
                path: iface.path_str(),
            }))
    }

    fn open_read(&mut self) -> anyhow::Result<(Interface, Box<dyn FrameIo + '_>)> {
        let h = self.hid().map_err(anyhow::Error::msg)?;
        let (iface, t) = h.open_config()?;
        Ok((iface, Box::new(t)))
    }

    fn apply(
        &mut self,
        steps: &[WriteStep],
        backup_dir: &Path,
        state_dir: &Path,
    ) -> Result<Applied, ApplyFail> {
        let opt_in = type84_cli::plan::opt_in_for(steps)
            .ok_or_else(|| ApplyFail::before_write("nothing to write"))?;
        let h = self.hid().map_err(ApplyFail::before_write)?;
        let (iface, mut session) = h
            .open_write_session(opt_in)
            .map_err(|e| ApplyFail::before_write(e.to_string()))?;
        apply::run_steps(&mut session, &iface, steps, backup_dir, state_dir, false)
            .map_err(|e| ApplyFail::from_apply(&e))
    }
}
