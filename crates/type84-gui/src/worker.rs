//! Background HID worker. It owns the device backend, talks to the UI over
//! channels and never blocks the UI thread.
//!
//! Reads use `type84_hid`'s read-only transport. The only write path is
//! [`Command::Apply`], which runs a confirmed [`Plan`] through
//! `type84_cli::apply::run_steps` (automatic pre-write backup, guarded
//! `apply_plan`, readback verification, lighting record) while holding the
//! write lock shared with the `type84` CLI (`<state dir>/anim.lock`).
//! Every device access holds that lock: the Hall poll takes it without
//! waiting (and is skipped while another process writes); full reads,
//! previews, backups and writes wait up to [`LOCK_WAIT`] for it.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use type84_cli::apply::{self, Applied};
use type84_cli::snapshot::Reader;

use crate::backend::{ApplyFail, Backend, HidBackend};
use crate::confirm::{build_plan, Plan, PlanOrigin, PlanRequest};
use crate::model::BoardState;
use crate::paths::Paths;

/// How often the worker re-enumerates USB devices for hot-plug.
pub const RESCAN_PERIOD: Duration = Duration::from_millis(1000);
/// How long an explicit read waits for another process's write to finish.
pub const LOCK_WAIT: Duration = Duration::from_secs(5);

/// Requests from the UI.
#[derive(Debug)]
pub enum Command {
    /// Re-enumerate now and report presence.
    Rescan,
    /// Open the board, read the full state, close it again.
    ReadAll,
    /// Re-read only the Hall/RT table (light poll, PROTOCOL.md §9.1).
    /// Skipped while another process holds the write lock.
    ReadHall,
    /// Read the board fresh and plan the request against it.
    Preview(Box<PlanRequest>),
    /// Write a confirmed plan, then re-read the full state.
    Apply(Box<Plan>),
    /// Full read saved as a backup file.
    Backup,
    Shutdown,
}

/// Board presence as seen by enumeration (nothing is opened for this).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    pub product: String,
    pub path: String,
}

/// Messages to the UI.
#[derive(Debug)]
pub enum Event {
    /// Presence changed: `Some` when the configuration interface appeared,
    /// `None` when it disappeared.
    Presence(Option<Presence>),
    /// A full read started.
    Reading,
    /// A full read finished.
    State(Box<BoardState>),
    /// Raw Hall/RT block (1024 B) from a light poll.
    HallBlock(Vec<u8>),
    /// The Hall poll was skipped because another process holds the lock.
    PollSkipped,
    /// A plan, ready for the confirm dialog.
    Plan(Box<Plan>),
    /// Planning failed (nothing written).
    PlanFailed { origin: PlanOrigin, detail: String },
    /// A plan was written and verified (or the board already held it).
    Applied {
        origin: PlanOrigin,
        applied: Box<Applied>,
    },
    /// A write failed.
    ApplyFailed { origin: PlanOrigin, fail: ApplyFail },
    /// «Сделать бэкап» finished.
    BackupSaved(PathBuf),
    /// Opening or reading failed. `context` is a UI string, `detail` the
    /// underlying error.
    Error { context: ErrorKind, detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    HidInit,
    Open,
    Read,
    /// A light Hall poll failed; the UI only logs it.
    Poll,
    /// Another process held the write lock for too long.
    Busy,
    Backup,
}

pub struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Event>,
    join: Option<JoinHandle<()>>,
}

/// Builds the backend inside the worker thread (hidapi contexts stay on
/// the thread that created them).
pub type BackendFactory = Box<dyn FnOnce() -> Box<dyn Backend> + Send>;

impl Worker {
    /// Start the worker on the real device with the user's paths. `ctx`
    /// (when present) is woken after every event so the UI repaints.
    pub fn spawn(ctx: Option<egui::Context>, paths: Paths) -> Self {
        Self::spawn_with(Box::new(|| Box::new(HidBackend::default())), paths, ctx)
    }

    /// Start the worker on any backend (tests use a simulated board).
    pub fn spawn_with(backend: BackendFactory, paths: Paths, ctx: Option<egui::Context>) -> Self {
        let (tx, cmd_rx) = mpsc::channel();
        let (evt_tx, rx) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("type84-hid".into())
            .spawn(move || {
                let out = Ctx {
                    tx: evt_tx,
                    repaint: ctx,
                };
                run(backend(), paths, cmd_rx, out)
            })
            .expect("spawning the HID worker thread");
        Worker {
            tx,
            rx,
            join: Some(join),
        }
    }

    /// A sender for commands from other threads (the tray).
    pub fn commands(&self) -> Sender<Command> {
        self.tx.clone()
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, t: Duration) -> Option<Event> {
        self.rx.recv_timeout(t).ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

struct Ctx {
    tx: Sender<Event>,
    repaint: Option<egui::Context>,
}

impl Ctx {
    fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
        if let Some(c) = &self.repaint {
            c.request_repaint();
        }
    }
}

/// Take the write lock for a device access, waiting up to [`LOCK_WAIT`]
/// for another process's write to finish. The caller holds the guard for
/// the whole access, so no `type84` write can start in the middle of it.
fn lock(paths: &Paths) -> Result<type84_cli::anim::AnimLock, String> {
    apply::write_lock_wait(&paths.state_dir, LOCK_WAIT).map_err(|e| format!("{e:#}"))
}

fn run(mut backend: Box<dyn Backend>, paths: Paths, rx: Receiver<Command>, out: Ctx) {
    let mut presence: Option<Presence> = None;
    let mut init_error_reported = false;
    let mut next_scan = Instant::now();

    loop {
        let wait = next_scan.saturating_duration_since(Instant::now());
        let cmd = match rx.recv_timeout(wait) {
            Ok(Command::Shutdown) => return,
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };

        match backend.scan() {
            Ok(now) => {
                if now != presence {
                    presence = now.clone();
                    out.emit(Event::Presence(now));
                }
            }
            Err(e) => {
                if !init_error_reported {
                    out.emit(Event::Error {
                        context: ErrorKind::HidInit,
                        detail: e.clone(),
                    });
                    init_error_reported = true;
                }
                next_scan = Instant::now() + RESCAN_PERIOD;
                match cmd {
                    Some(Command::Preview(req)) => out.emit(Event::PlanFailed {
                        origin: req.origin(),
                        detail: e,
                    }),
                    Some(Command::Apply(plan)) => out.emit(Event::ApplyFailed {
                        origin: plan.origin.clone(),
                        fail: ApplyFail::before_write(e),
                    }),
                    _ => {}
                }
                continue;
            }
        }
        next_scan = Instant::now() + RESCAN_PERIOD;

        let Some(cmd) = cmd else { continue };
        match cmd {
            Command::Rescan | Command::Shutdown => {}
            Command::ReadHall => {
                if presence.is_none() {
                    continue;
                }
                // Never touch the device while another process writes; hold
                // the lock for the few packets of the poll.
                let lock = match type84_cli::anim::try_lock(&paths.state_dir) {
                    Ok(Some(l)) => l,
                    _ => {
                        out.emit(Event::PollSkipped);
                        continue;
                    }
                };
                let r = read_hall(backend.as_mut());
                drop(lock);
                match r {
                    Ok(b) => out.emit(Event::HallBlock(b)),
                    Err(e) => out.emit(Event::Error {
                        context: ErrorKind::Poll,
                        detail: format!("{e:#}"),
                    }),
                }
            }
            Command::ReadAll => {
                out.emit(Event::Reading);
                match lock(&paths) {
                    Ok(_guard) => read_all_and_emit(backend.as_mut(), &paths, &out, &mut presence),
                    Err(e) => out.emit(Event::Error {
                        context: ErrorKind::Busy,
                        detail: e,
                    }),
                }
            }
            Command::Preview(req) => {
                let origin = req.origin();
                let _guard = match lock(&paths) {
                    Ok(g) => g,
                    Err(e) => {
                        out.emit(Event::PlanFailed { origin, detail: e });
                        continue;
                    }
                };
                let r = read_raw(backend.as_mut(), &paths).and_then(|b| build_plan(&req, &b));
                match r {
                    Ok(p) => out.emit(Event::Plan(Box::new(p))),
                    Err(e) => out.emit(Event::PlanFailed {
                        origin,
                        detail: format!("{e:#}"),
                    }),
                }
            }
            Command::Apply(plan) => {
                let origin = plan.origin.clone();
                // One lock for the write and the re-read after it.
                let guard = lock(&paths);
                let r = match &guard {
                    Err(e) => Err(ApplyFail::before_write(e.clone())),
                    Ok(_) => backend.apply(&plan.steps, &paths.auto_backup_dir, &paths.state_dir),
                };
                match r {
                    Ok(a) => out.emit(Event::Applied {
                        origin,
                        applied: Box::new(a),
                    }),
                    Err(fail) => out.emit(Event::ApplyFailed { origin, fail }),
                }
                // Show what the board holds now, success or not.
                if guard.is_ok() {
                    out.emit(Event::Reading);
                    read_all_and_emit(backend.as_mut(), &paths, &out, &mut presence);
                }
                drop(guard);
            }
            Command::Backup => {
                let _guard = match lock(&paths) {
                    Ok(g) => g,
                    Err(e) => {
                        out.emit(Event::Error {
                            context: ErrorKind::Busy,
                            detail: e,
                        });
                        continue;
                    }
                };
                match save_backup(backend.as_mut(), &paths) {
                    Ok(p) => out.emit(Event::BackupSaved(p)),
                    Err(e) => out.emit(Event::Error {
                        context: ErrorKind::Backup,
                        detail: format!("{e:#}"),
                    }),
                }
            }
        }
    }
}

/// Full read (the caller holds the write lock).
fn read_all_and_emit(
    backend: &mut dyn Backend,
    paths: &Paths,
    out: &Ctx,
    presence: &mut Option<Presence>,
) {
    match read_all(backend, paths) {
        Ok(state) => out.emit(Event::State(Box::new(state))),
        Err((kind, e)) => {
            out.emit(Event::Error {
                context: kind,
                detail: format!("{e:#}"),
            });
            // A failed read often means the board just went away.
            if let Ok(now) = backend.scan() {
                if now != *presence {
                    presence.clone_from(&now);
                    out.emit(Event::Presence(now));
                }
            }
        }
    }
}

/// Open the board, read the Hall/RT block only, drop the handle.
fn read_hall(b: &mut dyn Backend) -> anyhow::Result<Vec<u8>> {
    let (_iface, mut t) = b.open_read()?;
    let mut r = Reader::new(t.as_mut(), false);
    Ok(r.read_full(type84_proto::ReadOp::AxisRt)?.data)
}

/// The writable blocks, fresh, with the lighting record attached.
fn read_raw(b: &mut dyn Backend, paths: &Paths) -> anyhow::Result<type84_cli::plan::BoardState> {
    let (_iface, mut t) = b.open_read()?;
    let mut r = Reader::new(t.as_mut(), false);
    let mut board = type84_cli::plan::BoardState::read(&mut r)?;
    apply::attach_knowledge(&mut board, &paths.state_dir);
    Ok(board)
}

/// Open the board, read everything, drop the handle.
fn read_all(b: &mut dyn Backend, paths: &Paths) -> Result<BoardState, (ErrorKind, anyhow::Error)> {
    let (iface, mut t) = b.open_read().map_err(|e| (ErrorKind::Open, e))?;
    let mut r = Reader::new(t.as_mut(), false);
    let snap = r.snapshot().map_err(|e| (ErrorKind::Read, e))?;
    let product = iface.product.as_deref().unwrap_or("IO Type 84 Magnetic");
    let mut st = BoardState::from_snapshot(&snap, product).map_err(|e| (ErrorKind::Read, e))?;
    st.attach_knowledge(&paths.state_dir);
    Ok(st)
}

/// Full read saved under the manual backup dir, never overwriting.
fn save_backup(b: &mut dyn Backend, paths: &Paths) -> anyhow::Result<PathBuf> {
    let (iface, mut t) = b.open_read()?;
    let snap = Reader::new(t.as_mut(), false).snapshot()?;
    let backup = type84_cli::backup::Backup::from_snapshot(&snap, &iface)?;
    type84_cli::write::save_backup_new(&backup, &paths.backup_dir, "gui")
}
