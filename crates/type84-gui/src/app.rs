//! Application shell: device list with the device card, device page header
//! with tabs and toolbar, toasts, worker plumbing, physical-key selection,
//! the write flow (preview → confirm → guarded write) and the tray glue.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};
use type84_proto::WriteOp;

use crate::confirm::{op_label, Plan, PlanOrigin, PlanRequest};
use crate::dialogs::Dialog;
use crate::geometry;
use crate::i18n::{self, fill, s};
use crate::model::{BoardState, Edits, Source};
use crate::paths::{self, GuiSettings, Paths};
use crate::theme::{self, font, Weight};
use crate::tray::{TrayAction, TrayModel};
use crate::widgets::{self, Tone};
use crate::worker::{Command, ErrorKind, Event, Presence, Worker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Devices,
    Device,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Keymap,
    Advanced,
    Response,
    Lighting,
    Device,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Keymap,
        Tab::Advanced,
        Tab::Response,
        Tab::Lighting,
        Tab::Device,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Keymap => s().tab_keymap,
            Tab::Advanced => s().tab_advanced,
            Tab::Response => s().tab_response,
            Tab::Lighting => s().tab_lighting,
            Tab::Device => s().tab_device,
        }
    }
}

/// A notice shown at the top of the page.
#[derive(Debug, Clone)]
pub struct Toast {
    pub tone: Tone,
    pub text: String,
    pub detail: Option<String>,
    /// `ctx` time (seconds) after which it disappears; `None` = sticky.
    pub until: Option<f64>,
}

/// What the worker is doing for the UI right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Preview,
    Apply,
    Backup,
}

/// The Hall table as it was right before the case switch rewrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct PreSwitch {
    pub block: Vec<u8>,
    pub rt_precision: u8,
    /// Local time of the read that held it ("HH:MM:SS").
    pub time: String,
}

/// State of the "Клавиши" tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeymapUi {
    /// Show the Fn layer (otherwise layer 0).
    pub fn_layer: bool,
    /// Key whose Fn action is being picked.
    pub key: Option<u8>,
    /// Picker category.
    pub group: usize,
}

/// State of the "Подсветка" tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightingUi {
    /// Keys selected for painting.
    pub selection: BTreeSet<u8>,
    pub brush: [u8; 3],
}

impl Default for LightingUi {
    fn default() -> Self {
        LightingUi {
            selection: BTreeSet::new(),
            brush: [0xE5, 0x2B, 0x36],
        }
    }
}

pub struct App {
    worker: Option<Worker>,
    pub paths: Paths,
    pub settings: GuiSettings,
    pub screen: Screen,
    pub tab: Tab,
    /// Live presence of the board (enumeration only).
    pub presence: Option<Presence>,
    pub reading: bool,
    pub busy: Option<Busy>,
    pub board: Option<BoardState>,
    pub edits: Option<Edits>,
    pub selection: BTreeSet<u8>,
    pub add_mode: bool,
    pub select_by_press: bool,
    pub toasts: Vec<Toast>,
    pub dialog: Option<Dialog>,
    pub keymap: KeymapUi,
    pub lighting: LightingUi,
    backup_path: Option<PathBuf>,
    /// True after the device vanished while its page was open.
    lost: bool,
    prev_mods: egui::Modifiers,
    /// The Hall table changed on the board since it was read (case switch).
    pub hall_changed: bool,
    pub pre_switch: Option<PreSwitch>,
    last_hall_poll: f64,
    was_focused: bool,
    /// Hall polls skipped because another process held the write lock.
    pub poll_skips: usize,
    /// The user asked to open the device page (card button, tray); a
    /// background read on hot-plug only loads the state.
    open_requested: bool,
    /// A tray «Применить пресет» that arrived before the state was read.
    queued_plan: Option<PlanRequest>,
    tray: Option<crate::tray::Tray>,
    tray_rx: Option<Receiver<TrayAction>>,
    tray_model: Option<TrayModel>,
    quit: Arc<AtomicBool>,
}

/// Light Hall-table poll period while a live device page is open.
pub const HALL_POLL_SECS: f64 = 5.0;

fn temp_paths() -> Paths {
    Paths::under(
        &std::env::temp_dir().join(format!("keyboard-hub-detached-{}", std::process::id())),
    )
}

impl App {
    fn blank(worker: Option<Worker>, paths: Paths) -> Self {
        App {
            worker,
            settings: GuiSettings::load(&paths.settings_file),
            paths,
            screen: Screen::Devices,
            tab: Tab::Response,
            presence: None,
            reading: false,
            busy: None,
            board: None,
            edits: None,
            selection: BTreeSet::new(),
            add_mode: true,
            select_by_press: false,
            toasts: Vec::new(),
            dialog: None,
            keymap: KeymapUi {
                fn_layer: true,
                ..Default::default()
            },
            lighting: LightingUi::default(),
            backup_path: None,
            lost: false,
            prev_mods: egui::Modifiers::NONE,
            hall_changed: false,
            pre_switch: None,
            last_hall_poll: 0.0,
            was_focused: true,
            poll_skips: 0,
            open_requested: false,
            queued_plan: None,
            tray: None,
            tray_rx: None,
            tray_model: None,
            quit: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Live app with the HID worker on the real board, the user's paths
    /// and the tray icon.
    pub fn live(ctx: &egui::Context) -> Self {
        let paths = Paths::user().unwrap_or_else(|e| {
            eprintln!("warning: {e:#}; using a temporary directory");
            temp_paths()
        });
        let worker = Worker::spawn(Some(ctx.clone()), paths.clone());
        let mut app = Self::blank(Some(worker), paths);
        let (tx, rx) = std::sync::mpsc::channel();
        let model = app.tray_model_now();
        let cmds = app.worker.as_ref().map(Worker::commands);
        app.tray = crate::tray::Tray::spawn(ctx.clone(), tx, app.quit.clone(), cmds, model.clone());
        if app.tray.is_some() {
            app.tray_rx = Some(rx);
            app.tray_model = Some(model);
        }
        app
    }

    /// App driven by a given worker (tests use a simulated board).
    pub fn with_worker(worker: Worker, paths: Paths) -> Self {
        Self::blank(Some(worker), paths)
    }

    /// App without a worker (tests and renders). Set `presence` to show a card.
    pub fn detached() -> Self {
        Self::blank(None, temp_paths())
    }

    /// Offline viewer for a backup file; opens straight on "Отклик". Every
    /// write is disabled.
    pub fn from_backup(state: BoardState, path: Option<PathBuf>) -> Self {
        let mut a = Self::blank(None, temp_paths());
        a.set_state(state);
        a.backup_path = path;
        a.screen = Screen::Device;
        a
    }

    /// Handle a re-read Hall table: rebase edits and raise the banner when
    /// the board's values changed behind our back.
    pub fn on_hall_block(&mut self, raw: &[u8]) {
        if self.busy == Some(Busy::Apply) {
            return; // our own write; the full re-read follows
        }
        let (Some(board), Some(edits)) = (&mut self.board, &mut self.edits) else {
            return;
        };
        match crate::model::hall_from_block(raw, board.rt_precision) {
            Ok(fresh) => {
                let before = board.raw.hall.clone();
                if edits.rebase_keys(fresh.clone()) {
                    self.pre_switch = Some(PreSwitch {
                        block: before,
                        rt_precision: board.rt_precision,
                        time: chrono::Local::now().format("%H:%M:%S").to_string(),
                    });
                    board.set_hall(raw, fresh);
                    self.hall_changed = true;
                    self.refresh_tray();
                }
            }
            Err(e) => eprintln!("hall poll: {e:#}"),
        }
    }

    /// Re-read the Hall table on focus regain and every few seconds while a
    /// live device page is open (the case switch rewrites it silently).
    fn poll_hall(&mut self, ctx: &egui::Context, now: f64) {
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        let regained = focused && !self.was_focused;
        self.was_focused = focused;
        let live = self.worker.is_some()
            && self.screen == Screen::Device
            && !self.is_backup()
            && self.board.is_some();
        if !live || self.presence.is_none() || self.reading || self.busy.is_some() {
            return;
        }
        if regained || now - self.last_hall_poll >= HALL_POLL_SECS {
            self.last_hall_poll = now;
            if let Some(w) = &self.worker {
                w.send(Command::ReadHall);
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(HALL_POLL_SECS));
    }

    /// Poll the Hall table now (the 5 s timer does the same).
    pub fn request_hall_poll(&mut self) {
        if let Some(w) = &self.worker {
            w.send(Command::ReadHall);
        }
    }

    /// Open the board (the device card's «Открыть»).
    pub fn open_device(&mut self) {
        self.open_requested = true;
        self.request_read();
    }

    pub fn set_state(&mut self, state: BoardState) {
        self.hall_changed = false;
        self.edits = Some(Edits::new(&state));
        self.board = Some(state);
        self.refresh_tray();
    }

    pub fn push_toast(
        &mut self,
        tone: Tone,
        text: impl Into<String>,
        detail: Option<String>,
        ttl: Option<f64>,
        now: f64,
    ) {
        self.toasts.push(Toast {
            tone,
            text: text.into(),
            detail,
            until: ttl.map(|t| now + t),
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    pub fn is_backup(&self) -> bool {
        matches!(
            self.board.as_ref().map(|b| &b.source),
            Some(Source::Backup(_))
        )
    }

    /// Whether writes are possible now.
    pub fn can_write(&self) -> bool {
        self.worker.is_some()
            && !self.is_backup()
            && self.presence.is_some()
            && self.board.is_some()
            && !self.reading
            && self.busy.is_none()
    }

    pub fn pending(&self) -> usize {
        self.edits.as_ref().map(|e| e.diff_count()).unwrap_or(0)
    }

    fn request_read(&mut self) {
        if let Some(w) = &self.worker {
            w.send(Command::ReadAll);
            self.reading = true;
        }
    }

    /// Ask the worker to plan `req` against a fresh read; the confirm
    /// dialog opens when the plan arrives.
    pub fn request_plan(&mut self, req: PlanRequest) {
        if !self.can_write() {
            return;
        }
        if let Some(w) = &self.worker {
            w.send(Command::Preview(Box::new(req)));
            self.busy = Some(Busy::Preview);
        }
    }

    /// «Применить» on the pending bar.
    pub fn apply_pending(&mut self) {
        let Some(p) = self.edits.as_ref().and_then(|e| e.to_preset()) else {
            return;
        };
        self.request_plan(PlanRequest::Presets {
            presets: vec![p],
            origin: PlanOrigin::Pending,
        });
    }

    /// «Записать» in the confirm dialog.
    pub fn confirm_plan(&mut self, plan: Plan) {
        if self.is_backup() || plan.is_noop() {
            return;
        }
        if let Some(w) = &self.worker {
            w.send(Command::Apply(Box::new(plan)));
            self.busy = Some(Busy::Apply);
        }
    }

    pub fn request_backup(&mut self) {
        if !self.can_write() {
            return;
        }
        if let Some(w) = &self.worker {
            w.send(Command::Backup);
            self.busy = Some(Busy::Backup);
        }
    }

    /// "Перечитать": re-read the board (or reload the backup file). Pending
    /// edits are discarded and rebuilt on top of the fresh state.
    pub fn reread(&mut self, now: f64) {
        if self.is_backup() {
            if let Some(p) = self.backup_path.clone() {
                match BoardState::load_backup(&p) {
                    Ok(st) => {
                        self.set_state(st);
                        self.push_toast(Tone::Info, s().read_ok, None, Some(3.0), now);
                    }
                    Err(e) => self.push_toast(
                        Tone::Error,
                        s().backup_failed,
                        Some(format!("{e:#}")),
                        None,
                        now,
                    ),
                }
            }
        } else {
            self.request_read();
        }
    }

    fn on_applied(&mut self, applied: &type84_cli::apply::Applied, now: f64) {
        if applied.nothing_to_write {
            self.push_toast(Tone::Info, s().nothing_to_write, None, Some(6.0), now);
            return;
        }
        let mut ops: Vec<WriteOp> = applied
            .steps
            .iter()
            .filter(|s| s.frames_written > 0)
            .map(|s| s.op)
            .collect();
        ops.dedup();
        let names: Vec<&str> = ops.iter().map(|o| op_label(*o)).collect();
        let mut detail = fill(
            s().applied_backup,
            &[(
                "path",
                &paths::absolute(&applied.backup_path).display().to_string(),
            )],
        );
        if applied.steps.iter().any(|s| {
            s.notes
                .iter()
                .any(|n| n == type84_hid::write::COLOUR_NOT_VERIFIABLE)
        }) {
            detail.push('\n');
            detail.push_str(s().applied_colour);
        }
        if let Some(w) = &applied.record_warning {
            detail.push_str(&format!("\n{}: {w}", s().lighting_record_failed));
        }
        self.push_toast(
            Tone::Success,
            fill(s().applied_ok, &[("sections", &names.join(", "))]),
            Some(detail),
            Some(12.0),
            now,
        );
        self.hall_changed = false;
    }

    fn on_apply_failed(&mut self, fail: &crate::backend::ApplyFail, now: f64) {
        let mut detail = fail.message.clone();
        if let Some(b) = &fail.backup {
            let abs = paths::absolute(b).display().to_string();
            detail.push('\n');
            detail.push_str(&fill(s().applied_backup, &[("path", &abs)]));
            if !fail.nothing_written {
                detail.push('\n');
                detail.push_str(&fill(
                    s().apply_restore_hint,
                    &[
                        ("path", &abs),
                        (
                            "section",
                            fail.op
                                .map(type84_hid::write::restore_section)
                                .unwrap_or("hall|fn-layer|lighting"),
                        ),
                    ],
                ));
            }
        }
        let text = if fail.nothing_written {
            s().apply_refused
        } else {
            s().apply_failed
        };
        self.push_toast(Tone::Error, text, Some(detail), None, now);
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let Some(w) = &self.worker else { return };
        let mut events = Vec::new();
        while let Some(e) = w.try_recv() {
            events.push(e);
        }
        for e in events {
            match e {
                Event::Presence(p) => {
                    let was = self.presence.is_some();
                    self.presence = p;
                    let on_device_page = self.screen == Screen::Device && !self.is_backup();
                    if was && self.presence.is_none() && on_device_page {
                        self.lost = true;
                        self.push_toast(Tone::Error, s().device_gone, None, None, now);
                    }
                    // Load the state in the background when the board shows
                    // up, so the tray knows the current presets.
                    if !was && self.presence.is_some() && self.board.is_none() && !self.reading {
                        self.request_read();
                    }
                    if self.presence.is_some() && self.lost && on_device_page {
                        self.lost = false;
                        self.toasts.retain(|t| t.text != s().device_gone);
                        // Reopen by itself, as the hub does, unless that would drop edits.
                        if self.pending() == 0 {
                            self.request_read();
                        }
                    }
                }
                Event::Reading => self.reading = true,
                Event::HallBlock(raw) => self.on_hall_block(&raw),
                Event::PollSkipped => self.poll_skips += 1,
                Event::Error {
                    context: ErrorKind::Poll,
                    detail,
                } => eprintln!("hall poll failed: {detail}"),
                Event::State(st) => {
                    self.reading = false;
                    self.lost = false;
                    if self.busy == Some(Busy::Apply) {
                        self.busy = None;
                    }
                    self.set_state(*st);
                    if self.open_requested {
                        self.screen = Screen::Device;
                        self.open_requested = false;
                    }
                    if let Some(req) = self.queued_plan.take() {
                        self.request_plan(req);
                    }
                    self.last_hall_poll = now;
                    self.toasts.retain(|t| {
                        t.tone != Tone::Error
                            || t.text == s().apply_failed
                            || t.text == s().apply_refused
                    });
                }
                Event::Plan(p) => {
                    self.busy = None;
                    if p.is_noop() {
                        self.push_toast(Tone::Info, s().nothing_to_write, None, Some(6.0), now);
                    } else {
                        self.dialog = Some(Dialog::Confirm(p));
                    }
                }
                Event::PlanFailed { detail, .. } => {
                    self.busy = None;
                    self.push_toast(Tone::Error, s().plan_failed, Some(detail), None, now);
                }
                Event::Applied { applied, .. } => {
                    // The state re-read follows; keep `busy` until it lands.
                    self.on_applied(&applied, now);
                }
                Event::ApplyFailed { fail, .. } => self.on_apply_failed(&fail, now),
                Event::BackupSaved(p) => {
                    if self.busy == Some(Busy::Backup) {
                        self.busy = None;
                    }
                    self.push_toast(
                        Tone::Success,
                        s().backup_saved,
                        Some(paths::absolute(&p).display().to_string()),
                        Some(10.0),
                        now,
                    );
                }
                Event::Error { context, detail } => {
                    self.reading = false;
                    self.busy = None;
                    let text = match context {
                        ErrorKind::HidInit => s().hid_init_failed,
                        ErrorKind::Open => s().connect_failed,
                        ErrorKind::Read | ErrorKind::Poll => s().read_failed,
                        ErrorKind::Busy => s().busy_lock,
                        ErrorKind::Backup => s().backup_save_failed,
                    };
                    self.push_toast(Tone::Error, text, Some(detail), Some(12.0), now);
                }
            }
        }
    }

    /// "Выбор нажатием клавиш": physical presses toggle keys in the selection.
    fn handle_physical_keys(&mut self, ctx: &egui::Context) {
        let mods = ctx.input(|i| i.modifiers);
        let prev = std::mem::replace(&mut self.prev_mods, mods);
        let active = self.select_by_press
            && self.screen == Screen::Device
            && self.tab == Tab::Response
            && self.edits.is_some()
            && self.dialog.is_none();
        if !active || ctx.egui_wants_keyboard_input() {
            return;
        }
        // Mapped key events are consumed so a focused widget (e.g. the toggle
        // that enabled this mode) never sees Space/Enter/arrows.
        let mut hits: Vec<u8> = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    ..
                } => match geometry::index_for_key(physical_key.unwrap_or(*key)) {
                    Some(idx) => {
                        if *pressed && !*repeat {
                            hits.push(idx);
                        }
                        false
                    }
                    None => true,
                },
                _ => true,
            })
        });
        if mods.shift && !prev.shift {
            hits.push(geometry::SHIFT_INDEX);
        }
        if mods.ctrl && !prev.ctrl {
            hits.push(geometry::CTRL_INDEX);
        }
        if mods.alt && !prev.alt {
            hits.push(geometry::ALT_INDEX);
        }
        for i in hits {
            if !self.selection.remove(&i) {
                self.selection.insert(i);
            }
        }
    }

    /// Current presets per section and the preset names, for the tray.
    pub fn tray_model_now(&self) -> TrayModel {
        let mut m = TrayModel {
            tooltip: s().tray_tooltip.to_string(),
            current: Vec::new(),
            presets: Vec::new(),
            close_to_tray: self.settings.close_to_tray,
        };
        let loaded = type84_cli::apply::load_presets(&self.paths.presets_dir).unwrap_or_default();
        let good: Vec<(String, type84_cli::preset::Preset)> = loaded
            .into_iter()
            .filter_map(|(n, p)| p.ok().map(|p| (n, p)))
            .collect();
        m.presets = good.iter().map(|(n, _)| n.clone()).collect();
        if let Some(b) = self.board.as_ref().filter(|b| b.source == Source::Device) {
            let matches = type84_cli::apply::preset_matches(&good, &b.raw);
            for (sec, label) in [
                (type84_cli::preset::Section::Hall, s().section_hall),
                (type84_cli::preset::Section::FnLayer, s().section_fn),
                (type84_cli::preset::Section::Lighting, s().section_lighting),
            ] {
                let names = matches
                    .by_section
                    .get(&sec)
                    .map(|v| v.join(", "))
                    .unwrap_or_else(|| s().tray_none.to_string());
                m.current.push((label.to_string(), names));
            }
        }
        m
    }

    /// Push a changed tray model to the tray thread.
    pub fn refresh_tray(&mut self) {
        let Some(t) = &self.tray else { return };
        let m = self.tray_model_now();
        if self.tray_model.as_ref() != Some(&m) {
            t.update(m.clone());
            self.tray_model = Some(m);
        }
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.tray_rx else { return };
        let actions: Vec<TrayAction> = rx.try_iter().collect();
        for a in actions {
            match a {
                TrayAction::ApplyPreset(name) => {
                    // Never write from the tray: open the confirm dialog.
                    match type84_cli::preset::resolve(&name, &self.paths.presets_dir)
                        .and_then(|p| type84_cli::preset::Preset::load(&p))
                    {
                        Ok(p) => {
                            self.screen = Screen::Device;
                            let req = PlanRequest::Presets {
                                presets: vec![p],
                                origin: PlanOrigin::Preset(name),
                            };
                            if self.can_write() {
                                self.request_plan(req);
                            } else {
                                // Run it once the state is read.
                                self.queued_plan = Some(req);
                                self.open_requested = true;
                                if !self.reading {
                                    self.request_read();
                                }
                            }
                        }
                        Err(e) => {
                            let now = ctx.input(|i| i.time);
                            self.push_toast(
                                Tone::Error,
                                s().plan_failed,
                                Some(format!("{e:#}")),
                                None,
                                now,
                            );
                        }
                    }
                }
                TrayAction::Backup => self.request_backup(),
                TrayAction::ToggleCloseToTray => {
                    self.set_close_to_tray(!self.settings.close_to_tray)
                }
            }
        }
    }

    pub fn set_close_to_tray(&mut self, on: bool) {
        self.settings.close_to_tray = on;
        if let Err(e) = self.settings.save(&self.paths.settings_file) {
            eprintln!("warning: saving settings: {e:#}");
        }
        self.refresh_tray();
    }

    /// Close button: hide to the tray when enabled and a tray exists.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.quit.load(Ordering::SeqCst) || !self.settings.close_to_tray || self.tray.is_none() {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        crate::tray::hide_window(ctx);
    }

    /// Draw one frame. Shared by eframe and the kittest harness.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.handle_close(&ctx);
        self.handle_tray(&ctx);
        self.poll_worker(&ctx);
        self.handle_physical_keys(&ctx);
        let now = ctx.input(|i| i.time);
        self.poll_hall(&ctx, now);
        self.poll_file_picker();
        self.toasts.retain(|t| t.until.is_none_or(|u| u > now));
        if let Some(next) = self.toasts.iter().filter_map(|t| t.until).reduce(f64::min) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((next - now).max(0.05)));
        }
        if self.busy.is_some() || self.reading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        if self.screen == Screen::Device && self.pending() > 0 {
            egui::Panel::bottom("pending")
                .frame(
                    Frame::new()
                        .fill(theme::BG2)
                        .stroke(Stroke::new(1.0, theme::PANEL_BORDER))
                        .inner_margin(Margin::symmetric(theme::SP_COLUMN as i8, 12)),
                )
                .show_separator_line(false)
                .show(ui, |ui| self.pending_bar(ui));
        }
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::symmetric(theme::SP_COLUMN as i8, 16)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.toasts_ui(ui, now);
                        match self.screen {
                            Screen::Devices => self.devices_screen(ui),
                            Screen::Device => self.device_page(ui),
                        }
                    });
            });
        self.dialogs_ui(&ctx);
    }

    fn toasts_ui(&mut self, ui: &mut Ui, _now: f64) {
        if let Some(b) = self.busy {
            let text = match b {
                Busy::Preview => s().busy_preview,
                Busy::Apply => s().busy_applying,
                Busy::Backup => s().busy_backup,
            };
            ui.horizontal(|ui| {
                ui.set_max_width(ui.available_width().min(760.0));
                widgets::notice(ui, Tone::Info, text, None);
            });
            ui.add_space(theme::SP_RELATED);
        }
        let mut close = None;
        for (i, t) in self.toasts.iter().enumerate() {
            let r = ui.horizontal(|ui| {
                ui.set_max_width(ui.available_width().min(760.0));
                widgets::notice(ui, t.tone, &t.text, t.detail.as_deref())
            });
            if r.inner.interact(egui::Sense::click()).clicked() {
                close = Some(i);
            }
            ui.add_space(theme::SP_RELATED);
        }
        if let Some(i) = close {
            self.toasts.remove(i);
        }
    }

    fn devices_screen(&mut self, ui: &mut Ui) {
        ui.label(RichText::new(s().devices_title).font(font(theme::FS_TITLE, Weight::Semibold)));
        ui.add_space(theme::SP_GROUP);
        match self.presence.clone() {
            Some(_) => self.device_card(ui),
            None => {
                widgets::panel_frame().show(ui, |ui| {
                    ui.set_max_width(560.0);
                    widgets::title(ui, s().no_devices_title);
                    ui.add_space(theme::SP_CAPTION);
                    widgets::hint(ui, s().no_devices_hint);
                    ui.add_space(theme::SP_GROUP);
                    if widgets::button(ui, s().refresh_list).clicked() {
                        if let Some(w) = &self.worker {
                            w.send(Command::Rescan);
                        }
                    }
                });
            }
        }
    }

    fn device_card(&mut self, ui: &mut Ui) {
        let w = ui.available_width().min(600.0);
        Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::PANEL_BORDER))
            .corner_radius(CornerRadius::same(theme::R_PANEL))
            .show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 0.0;
                // Upper area with the keyboard thumbnail.
                Frame::new()
                    .fill(theme::mix(theme::PANEL, theme::FG, 0.03))
                    .corner_radius(CornerRadius {
                        nw: theme::R_PANEL,
                        ne: theme::R_PANEL,
                        sw: 0,
                        se: 0,
                    })
                    .inner_margin(Margin::symmetric(0, 24))
                    .show(ui, |ui| {
                        ui.set_width(w);
                        ui.vertical_centered(|ui| {
                            crate::keyboard::card_thumbnail(ui, (w * 0.775).round());
                        });
                    });
                // Lower area: name, link, kind, button.
                Frame::new()
                    .fill(theme::CARD_LOWER)
                    .corner_radius(CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: theme::R_PANEL,
                        se: theme::R_PANEL,
                    })
                    .inner_margin(Margin {
                        left: 24,
                        right: 24,
                        top: 22,
                        bottom: 22,
                    })
                    .show(ui, |ui| {
                        ui.set_width(w - 48.0);
                        ui.label(
                            RichText::new(s().device_name)
                                .font(font(theme::FS_TITLE, Weight::Semibold)),
                        );
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(s().connected_cable)
                                .font(font(theme::FS_BODY, Weight::Regular)),
                        );
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(s().kind_magnetic)
                                .font(font(theme::FS_BODY, Weight::Regular))
                                .color(theme::HINT),
                        );
                        ui.add_space(22.0);
                        let text = if self.reading {
                            s().connecting
                        } else {
                            s().open
                        };
                        let r = ui
                            .add_enabled_ui(!self.reading, |ui| {
                                widgets::primary_button(ui, text, 130.0)
                            })
                            .inner;
                        if r.clicked() {
                            self.open_device();
                        }
                    });
            });
    }

    fn device_page(&mut self, ui: &mut Ui) {
        self.header(ui);
        ui.add_space(theme::SP_RELATED);
        self.toolbar(ui);
        ui.add_space(theme::SP_RELATED);
        match self.tab {
            Tab::Response => self.response_tab(ui),
            Tab::Keymap => self.keymap_tab(ui),
            Tab::Lighting => self.lighting_tab(ui),
            Tab::Device => self.device_tab(ui),
            Tab::Advanced => {
                widgets::panel_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    widgets::title(ui, s().placeholder_title);
                    ui.add_space(theme::SP_CAPTION);
                    widgets::hint(ui, s().placeholder_text);
                    ui.add_space(theme::SP_GROUP);
                    widgets::notice(ui, Tone::Info, s().hw_switches_note, None);
                });
            }
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        let now = ui.ctx().input(|i| i.time);
        Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::PANEL_BORDER))
            .corner_radius(CornerRadius::same(theme::R_PANEL))
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.label(
                            RichText::new(s().device_name)
                                .font(font(theme::FS_TITLE, Weight::Semibold)),
                        );
                        let fw = self
                            .board
                            .as_ref()
                            .map(|b| b.firmware.clone())
                            .unwrap_or_default();
                        let status = if self.is_backup() {
                            s().status_backup
                        } else if self.reading {
                            s().status_reading
                        } else if self.presence.is_some() {
                            s().status_ready
                        } else {
                            s().status_gone
                        };
                        let line = format!(
                            "{} · {}",
                            fill(s().summary_magnetic, &[("firmware", &fw)]),
                            status
                        );
                        ui.label(
                            RichText::new(line)
                                .font(font(theme::FS_SMALL, Weight::Regular))
                                .color(theme::HINT),
                        );
                    });
                    ui.add_space(theme::SP_COLUMN);
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for t in Tab::ALL {
                        if widgets::tab_button(ui, t.label(), self.tab == t).clicked() {
                            self.tab = t;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = theme::SP_CONTROL;
                        if widgets::ghost_button(ui, s().nav_devices).clicked() {
                            self.screen = Screen::Devices;
                        }
                        let can = self.is_backup()
                            || (self.presence.is_some() && !self.reading && self.busy.is_none());
                        if ui
                            .add_enabled_ui(can, |ui| widgets::button(ui, s().reread))
                            .inner
                            .clicked()
                        {
                            self.reread(now);
                        }
                    });
                });
            });
    }

    /// Presets, backups and restore.
    fn toolbar(&mut self, ui: &mut Ui) {
        let can = self.can_write();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::SP_CONTROL;
            let tip = |r: egui::Response| r.on_disabled_hover_text(s().backup_mode_no_write);
            if widgets::button(ui, s().presets_button).clicked() {
                self.open_presets();
            }
            if ui
                .add_enabled_ui(self.board.is_some(), |ui| {
                    widgets::button(ui, s().save_preset_button)
                })
                .inner
                .clicked()
            {
                self.open_save_preset();
            }
            if tip(ui
                .add_enabled_ui(can, |ui| widgets::button(ui, s().restore_button))
                .inner)
            .clicked()
            {
                self.open_restore();
            }
            if tip(ui
                .add_enabled_ui(can, |ui| widgets::ghost_button(ui, s().backup_button))
                .inner)
            .clicked()
            {
                self.request_backup();
            }
        });
    }

    fn pending_bar(&mut self, ui: &mut Ui) {
        let n = self.pending();
        let can = self.can_write();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    RichText::new(fill(s().pending_count, &[("n", &n.to_string())]))
                        .font(font(theme::FS_BODY, Weight::Semibold)),
                );
                ui.label(
                    RichText::new(s().pending_hint)
                        .font(font(theme::FS_HINT, Weight::Regular))
                        .color(theme::HINT),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = ui
                    .add_enabled_ui(can, |ui| widgets::primary_button(ui, s().apply, 130.0))
                    .inner
                    .on_disabled_hover_text(s().apply_disabled);
                if r.clicked() {
                    self.apply_pending();
                }
                if ui
                    .add_enabled_ui(self.busy.is_none(), |ui| widgets::button(ui, s().discard))
                    .inner
                    .clicked()
                {
                    if let Some(e) = &mut self.edits {
                        e.discard();
                    }
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(theme::BG).to_array()
    }
}

/// Selected keys in layout order.
pub fn ordered(selection: &BTreeSet<u8>) -> Vec<u8> {
    geometry::keys()
        .iter()
        .map(|k| k.index)
        .filter(|i| selection.contains(i))
        .collect()
}

/// Shorthand used by the tab modules.
pub(crate) fn mm(h: u16) -> String {
    i18n::mm_comma(h)
}
