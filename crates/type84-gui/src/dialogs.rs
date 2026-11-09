//! Modal dialogs: the write confirmation, «Сохранить пресет…», «Пресеты»,
//! «Восстановить из бэкапа…» and the banner's «Восстановить». None of them
//! writes on its own: every write goes through a worker preview and the
//! confirm dialog.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use egui::{vec2, Align, Color32, Layout, RichText, ScrollArea, Ui};
use type84_cli::apply::{self, Matches};
use type84_cli::backup::Backup;
use type84_cli::preset::{self, Preset, Section};

use crate::app::{App, PreSwitch};
use crate::confirm::{op_label, Plan, PlanOrigin, PlanRequest};
use crate::i18n::{fill, s};
use crate::paths;
use crate::theme::{self, font, Weight};
use crate::widgets::{self, Tone};

/// The open dialog.
pub enum Dialog {
    Confirm(Box<Plan>),
    SavePreset(SaveState),
    Presets(PresetsState),
    Restore(RestoreState),
    HallRestore(HallRestoreState),
}

pub struct SaveState {
    pub name: String,
    pub hall: bool,
    pub fn_layer: bool,
    pub lighting: bool,
    pub overwrite: bool,
    pub error: Option<String>,
}

pub struct PresetsState {
    pub list: Vec<(String, Result<Preset, String>)>,
    pub matches: Matches,
    pub confirm_delete: Option<String>,
}

pub struct RestoreState {
    pub path: Option<PathBuf>,
    pub backup: Option<Box<Backup>>,
    pub error: Option<String>,
    pub hall: bool,
    pub fn_layer: bool,
    pub lighting: bool,
    pub picking: Option<Receiver<Option<PathBuf>>>,
}

/// One source the banner can restore the Hall table from.
pub enum HallChoice {
    PreSwitch(PreSwitch),
    Backup {
        path: PathBuf,
        block: Vec<u8>,
        rt_precision: u8,
    },
    Preset(String, Box<Preset>),
}

impl HallChoice {
    fn label(&self) -> String {
        match self {
            HallChoice::PreSwitch(p) => fill(s().banner_pre_switch, &[("time", &p.time)]),
            HallChoice::Backup { path, .. } => {
                fill(s().banner_newest, &[("file", &file_name(path))])
            }
            HallChoice::Preset(n, _) => fill(s().banner_preset, &[("name", n)]),
        }
    }
}

pub struct HallRestoreState {
    pub choices: Vec<HallChoice>,
    pub selected: usize,
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn sections_of(hall: bool, fn_layer: bool, lighting: bool) -> Vec<Section> {
    [
        (hall, Section::Hall),
        (fn_layer, Section::FnLayer),
        (lighting, Section::Lighting),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, s)| s)
    .collect()
}

fn section_checkboxes(ui: &mut Ui, hall: &mut bool, fn_layer: &mut bool, lighting: &mut bool) {
    ui.checkbox(hall, s().section_hall);
    ui.checkbox(fn_layer, s().section_fn);
    ui.checkbox(lighting, s().section_lighting);
}

fn dialog_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(font(theme::FS_TITLE, Weight::Semibold)));
}

impl App {
    pub fn open_save_preset(&mut self) {
        self.dialog = Some(Dialog::SavePreset(SaveState {
            name: String::new(),
            hall: true,
            fn_layer: false,
            lighting: false,
            overwrite: false,
            error: None,
        }));
    }

    pub fn open_presets(&mut self) {
        let list = apply::load_presets(&self.paths.presets_dir).unwrap_or_default();
        let good: Vec<(String, Preset)> = list
            .iter()
            .filter_map(|(n, p)| p.as_ref().ok().map(|p| (n.clone(), p.clone())))
            .collect();
        let matches = match &self.board {
            Some(b) if !self.is_backup() => apply::preset_matches(&good, &b.raw),
            _ => Matches::default(),
        };
        self.dialog = Some(Dialog::Presets(PresetsState {
            list,
            matches,
            confirm_delete: None,
        }));
    }

    pub fn open_restore(&mut self) {
        self.dialog = Some(Dialog::Restore(RestoreState {
            path: None,
            backup: None,
            error: None,
            hall: true,
            fn_layer: false,
            lighting: false,
            picking: None,
        }));
    }

    /// Load a backup file into the open restore dialog (the file picker's
    /// result; tests call it directly).
    pub fn restore_load(&mut self, path: PathBuf) {
        if let Some(Dialog::Restore(st)) = &mut self.dialog {
            match Backup::load(&path) {
                Ok(b) => {
                    st.backup = Some(Box::new(b));
                    st.error = None;
                }
                Err(e) => {
                    st.backup = None;
                    st.error = Some(format!("{e:#}"));
                }
            }
            st.path = Some(path);
        }
    }

    pub(crate) fn poll_file_picker(&mut self) {
        let picked = match &mut self.dialog {
            Some(Dialog::Restore(st)) => match st.picking.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(p)) => {
                    st.picking = None;
                    p
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    st.picking = None;
                    None
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(p) = picked {
            self.restore_load(p);
        }
    }

    /// The banner's «Восстановить»: offer the table from before the switch,
    /// the newest backup whose Hall table differs from the board, and every
    /// preset with a Hall section.
    pub fn open_hall_restore(&mut self) {
        let mut choices = Vec::new();
        if let Some(p) = &self.pre_switch {
            choices.push(HallChoice::PreSwitch(p.clone()));
        }
        let current = self.board.as_ref().map(|b| b.raw.hall.clone());
        let mut files: Vec<PathBuf> = [&self.paths.auto_backup_dir, &self.paths.backup_dir]
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort_by_key(|p| std::cmp::Reverse(p.metadata().and_then(|m| m.modified()).ok()));
        for f in files.into_iter().take(20) {
            let Ok(b) = Backup::load(&f) else { continue };
            let Ok(block) = b.blocks.axis_rt.bytes() else {
                continue;
            };
            if Some(&block) != current.as_ref() {
                choices.push(HallChoice::Backup {
                    path: f,
                    block,
                    rt_precision: b.device.info.rt_precision,
                });
                break;
            }
        }
        for (n, p) in apply::load_presets(&self.paths.presets_dir).unwrap_or_default() {
            if let Ok(p) = p {
                if p.hall.is_some() {
                    choices.push(HallChoice::Preset(n, Box::new(p)));
                }
            }
        }
        self.dialog = Some(Dialog::HallRestore(HallRestoreState {
            choices,
            selected: 0,
        }));
    }

    pub(crate) fn dialogs_ui(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dialog.take() else {
            return;
        };
        let keep = match &mut d {
            Dialog::Confirm(plan) => self.confirm_ui(ctx, plan),
            Dialog::SavePreset(st) => self.save_ui(ctx, st),
            Dialog::Presets(st) => self.presets_ui(ctx, st),
            Dialog::Restore(st) => self.restore_ui(ctx, st),
            Dialog::HallRestore(st) => self.hall_restore_ui(ctx, st),
        };
        // A dialog may have opened another one (e.g. a plan request).
        if keep && self.dialog.is_none() {
            self.dialog = Some(d);
        }
    }

    /// Returns whether the dialog stays open.
    fn confirm_ui(&mut self, ctx: &egui::Context, plan: &mut Box<Plan>) -> bool {
        let mut result: Option<bool> = None;
        let pending = self.pending();
        let backup_dir = paths::absolute(&self.paths.auto_backup_dir);
        let r = egui::Modal::new(egui::Id::unique("confirm-write"))
            .frame(widgets::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(720.0f32.min(ctx.content_rect().width() - 80.0));
                dialog_title(ui, s().confirm_title);
                let origin = match &plan.origin {
                    PlanOrigin::Pending => s().confirm_from_pending.to_string(),
                    PlanOrigin::Preset(n) => fill(s().confirm_from_preset, &[("name", n)]),
                    PlanOrigin::Restore(p) => {
                        fill(s().confirm_from_backup, &[("file", &file_name(p))])
                    }
                    PlanOrigin::HallRestore(w) => fill(s().confirm_from_hall, &[("what", w)]),
                };
                ui.label(
                    RichText::new(origin)
                        .font(font(theme::FS_HINT, Weight::Regular))
                        .color(theme::HINT),
                );
                ui.add_space(theme::SP_RELATED);
                ui.horizontal_wrapped(|ui| {
                    for (op, n) in &plan.model.sections {
                        let t = format!("{}: {n}", op_label(*op));
                        egui::Frame::new()
                            .fill(theme::CONTROL)
                            .stroke(egui::Stroke::new(1.0, theme::CONTROL_BORDER))
                            .corner_radius(egui::CornerRadius::same(theme::R_CONTROL))
                            .inner_margin(egui::Margin::symmetric(10, 5))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(t).font(font(theme::FS_HINT, Weight::Medium)),
                                );
                            });
                    }
                });
                ui.add_space(theme::SP_RELATED);
                egui::Frame::new()
                    .fill(theme::BG2)
                    .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                    .corner_radius(egui::CornerRadius::same(theme::R_TOOL))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                            diff_table(ui, plan);
                        });
                    });
                if plan.model.non_physical > 0 {
                    ui.add_space(theme::SP_CONTROL);
                    widgets::hint(
                        ui,
                        &fill(
                            s().confirm_non_physical,
                            &[("n", &plan.model.non_physical.to_string())],
                        ),
                    );
                }
                ui.add_space(theme::SP_RELATED);
                for n in &plan.notes {
                    widgets::notice(ui, Tone::Warn, n, None);
                    ui.add_space(theme::SP_CONTROL);
                }
                if pending > 0 && plan.origin != PlanOrigin::Pending {
                    widgets::notice(ui, Tone::Warn, s().confirm_discards_edits, None);
                    ui.add_space(theme::SP_CONTROL);
                }
                widgets::hint(
                    ui,
                    &fill(
                        s().confirm_backup_note,
                        &[("dir", &backup_dir.display().to_string())],
                    ),
                );
                widgets::hint(ui, s().confirm_verify_note);
                ui.add_space(theme::SP_GROUP);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(ui, s().confirm_write, 130.0).clicked() {
                        result = Some(true);
                    }
                    if widgets::button(ui, s().cancel).clicked() {
                        result = Some(false);
                    }
                });
            });
        if r.should_close() && result.is_none() {
            result = Some(false);
        }
        match result {
            Some(true) => {
                let p = std::mem::replace(
                    plan,
                    Box::new(Plan {
                        origin: PlanOrigin::Pending,
                        previews: Vec::new(),
                        steps: Vec::new(),
                        model: Default::default(),
                        notes: Vec::new(),
                    }),
                );
                self.confirm_plan(*p);
                false
            }
            Some(false) => false,
            None => true,
        }
    }

    fn save_ui(&mut self, ctx: &egui::Context, st: &mut SaveState) -> bool {
        let mut result: Option<bool> = None;
        let dir = self.paths.presets_dir.clone();
        let valid = preset::validate_name(st.name.trim()).is_ok();
        let exists = valid && dir.join(format!("{}.json", st.name.trim())).exists();
        let r = egui::Modal::new(egui::Id::unique("save-preset"))
            .frame(widgets::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(520.0);
                dialog_title(ui, s().save_title);
                ui.add_space(theme::SP_RELATED);
                widgets::title(ui, s().save_name);
                let te = ui.add(
                    egui::TextEdit::singleline(&mut st.name)
                        .desired_width(f32::INFINITY)
                        .hint_text("cs2"),
                );
                te.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, s().save_name)
                });
                if !st.name.is_empty() && !valid {
                    ui.label(
                        RichText::new(s().save_name_invalid)
                            .font(font(theme::FS_SMALL, Weight::Regular))
                            .color(theme::DANGER_TEXT),
                    );
                }
                widgets::hint(
                    ui,
                    &fill(s().save_name_hint, &[("dir", &dir.display().to_string())]),
                );
                ui.add_space(theme::SP_RELATED);
                widgets::title(ui, s().save_sections);
                section_checkboxes(ui, &mut st.hall, &mut st.fn_layer, &mut st.lighting);
                widgets::hint(ui, s().save_from_board);
                if exists {
                    ui.add_space(theme::SP_CONTROL);
                    widgets::notice(ui, Tone::Warn, s().save_exists, None);
                    ui.checkbox(&mut st.overwrite, s().save_overwrite);
                }
                if let Some(e) = &st.error {
                    ui.add_space(theme::SP_CONTROL);
                    widgets::notice(ui, Tone::Error, s().save_failed, Some(e));
                }
                ui.add_space(theme::SP_GROUP);
                let any = st.hall || st.fn_layer || st.lighting;
                let can = valid && any && (!exists || st.overwrite);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled_ui(can, |ui| {
                            widgets::primary_button(ui, s().save_button, 130.0)
                        })
                        .inner
                        .clicked()
                    {
                        result = Some(true);
                    }
                    if widgets::button(ui, s().cancel).clicked() {
                        result = Some(false);
                    }
                });
            });
        if r.should_close() && result.is_none() {
            result = Some(false);
        }
        match result {
            Some(true) => match self.save_preset(st) {
                Ok(name) => {
                    let now = ctx.input(|i| i.time);
                    self.push_toast(
                        Tone::Success,
                        fill(s().saved_ok, &[("name", &name)]),
                        None,
                        Some(6.0),
                        now,
                    );
                    self.refresh_tray();
                    false
                }
                Err(e) => {
                    st.error = Some(format!("{e:#}"));
                    true
                }
            },
            Some(false) => false,
            None => true,
        }
    }

    /// Save the board's state (as last read) as a preset.
    pub fn save_preset(&self, st: &SaveState) -> anyhow::Result<String> {
        let name = st.name.trim().to_string();
        let b = self
            .board
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no board state"))?;
        let source = match &b.source {
            crate::model::Source::Device => format!(
                "Keyboard Hub, board fw {} at {}",
                b.firmware,
                chrono::Local::now().to_rfc3339()
            ),
            crate::model::Source::Backup(f) => format!("Keyboard Hub, backup {f}"),
        };
        let p = apply::preset_from_board(
            &name,
            &sections_of(st.hall, st.fn_layer, st.lighting),
            &b.raw,
            source,
        )?;
        let path = preset::resolve(&name, &self.paths.presets_dir)?;
        p.save(&path, st.overwrite)?;
        Ok(name)
    }

    fn presets_ui(&mut self, ctx: &egui::Context, st: &mut PresetsState) -> bool {
        let mut close = false;
        let mut apply_name: Option<(String, Preset)> = None;
        let mut delete: Option<String> = None;
        let can = self.can_write();
        let r = egui::Modal::new(egui::Id::unique("presets"))
            .frame(widgets::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(640.0);
                dialog_title(ui, s().presets_title);
                widgets::hint(ui, &self.paths.presets_dir.display().to_string());
                ui.add_space(theme::SP_RELATED);
                if st.list.is_empty() {
                    widgets::hint(ui, s().presets_empty);
                }
                ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    for (name, p) in &st.list {
                        egui::Frame::new()
                            .fill(theme::BG2)
                            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                            .corner_radius(egui::CornerRadius::same(theme::R_TOOL))
                            .inner_margin(egui::Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        widgets::title(ui, name);
                                        match p {
                                            Ok(p) => {
                                                let row = st
                                                    .matches
                                                    .rows
                                                    .iter()
                                                    .find(|r| &r.name == name);
                                                let secs: Vec<String> = p
                                                    .sections()
                                                    .into_iter()
                                                    .map(|sec| {
                                                        let ok = row.is_some_and(|r| {
                                                            r.sections.contains(&(sec, true))
                                                        });
                                                        format!(
                                                            "{}{}",
                                                            section_ru(sec),
                                                            if ok { " ✓" } else { "" }
                                                        )
                                                    })
                                                    .collect();
                                                let mut line = secs.join(" · ");
                                                if row.is_some_and(|r| r.all) {
                                                    line.push_str(&format!(
                                                        " — {}",
                                                        s().presets_match
                                                    ));
                                                }
                                                widgets::hint(ui, &line);
                                            }
                                            Err(e) => {
                                                ui.label(
                                                    RichText::new(fill(
                                                        s().presets_broken,
                                                        &[("err", e)],
                                                    ))
                                                    .font(font(theme::FS_SMALL, Weight::Regular))
                                                    .color(theme::DANGER_TEXT),
                                                );
                                            }
                                        }
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let del_label =
                                            if st.confirm_delete.as_deref() == Some(name) {
                                                s().presets_delete_confirm
                                            } else {
                                                s().presets_delete
                                            };
                                        if widgets::ghost_button(ui, del_label)
                                            .on_hover_text(format!("{} {name}", s().presets_delete))
                                            .clicked()
                                        {
                                            if st.confirm_delete.as_deref() == Some(name) {
                                                delete = Some(name.clone());
                                            } else {
                                                st.confirm_delete = Some(name.clone());
                                            }
                                        }
                                        if let Ok(p) = p {
                                            let b = ui
                                                .add_enabled_ui(can, |ui| {
                                                    widgets::button(ui, s().presets_apply)
                                                })
                                                .inner
                                                .on_hover_text(format!(
                                                    "{} {name}",
                                                    s().presets_apply
                                                ))
                                                .on_disabled_hover_text(s().backup_mode_no_write);
                                            if b.clicked() {
                                                apply_name = Some((name.clone(), p.clone()));
                                            }
                                        }
                                    });
                                });
                            });
                        ui.add_space(theme::SP_CONTROL);
                    }
                });
                ui.add_space(theme::SP_RELATED);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::button(ui, s().close).clicked() {
                        close = true;
                    }
                    if ui
                        .add_enabled_ui(self.board.is_some(), |ui| {
                            widgets::ghost_button(ui, s().save_preset_button)
                        })
                        .inner
                        .clicked()
                    {
                        close = true;
                        self.open_save_preset();
                    }
                });
            });
        let now = ctx.input(|i| i.time);
        if let Some(name) = delete {
            let path = self.paths.presets_dir.join(format!("{name}.json"));
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    st.list.retain(|(n, _)| n != &name);
                    st.confirm_delete = None;
                    self.push_toast(
                        Tone::Info,
                        fill(s().presets_deleted, &[("name", &name)]),
                        None,
                        Some(6.0),
                        now,
                    );
                    self.refresh_tray();
                }
                Err(e) => {
                    self.push_toast(Tone::Error, s().save_failed, Some(e.to_string()), None, now)
                }
            }
        }
        if let Some((name, p)) = apply_name {
            self.request_plan(PlanRequest::Presets {
                presets: vec![p],
                origin: PlanOrigin::Preset(name),
            });
            return false;
        }
        !(close || (r.should_close() && self.dialog.is_none()))
    }

    fn restore_ui(&mut self, ctx: &egui::Context, st: &mut RestoreState) -> bool {
        let mut result: Option<bool> = None;
        let r = egui::Modal::new(egui::Id::unique("restore"))
            .frame(widgets::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(600.0);
                dialog_title(ui, s().restore_title);
                ui.add_space(theme::SP_RELATED);
                ui.horizontal(|ui| {
                    let picking = st.picking.is_some();
                    if ui
                        .add_enabled_ui(!picking, |ui| widgets::button(ui, s().restore_pick))
                        .inner
                        .clicked()
                    {
                        st.picking = Some(spawn_picker(self.paths.auto_backup_dir.clone()));
                    }
                    match (&st.path, &st.backup) {
                        (Some(p), Some(b)) => widgets::hint(
                            ui,
                            &fill(
                                s().restore_file,
                                &[
                                    ("file", &file_name(p)),
                                    (
                                        "date",
                                        &b.created_at
                                            .chars()
                                            .take(19)
                                            .collect::<String>()
                                            .replace('T', " "),
                                    ),
                                    ("fw", &b.device.firmware),
                                ],
                            ),
                        ),
                        _ => widgets::hint(ui, s().restore_none),
                    };
                });
                if let Some(e) = &st.error {
                    ui.add_space(theme::SP_CONTROL);
                    widgets::notice(ui, Tone::Error, s().restore_load_failed, Some(e));
                }
                ui.add_space(theme::SP_RELATED);
                widgets::title(ui, s().restore_sections);
                section_checkboxes(ui, &mut st.hall, &mut st.fn_layer, &mut st.lighting);
                widgets::hint(ui, s().restore_hint);
                ui.add_space(theme::SP_GROUP);
                let can = st.backup.is_some()
                    && (st.hall || st.fn_layer || st.lighting)
                    && self.can_write();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled_ui(can, |ui| {
                            widgets::primary_button(ui, s().restore_preview, 160.0)
                        })
                        .inner
                        .clicked()
                    {
                        result = Some(true);
                    }
                    if widgets::button(ui, s().cancel).clicked() {
                        result = Some(false);
                    }
                });
            });
        if r.should_close() && result.is_none() {
            result = Some(false);
        }
        match result {
            Some(true) => {
                let (Some(b), Some(p)) = (st.backup.take(), st.path.clone()) else {
                    return true;
                };
                self.request_plan(PlanRequest::Restore {
                    backup: b,
                    path: p,
                    sections: sections_of(st.hall, st.fn_layer, st.lighting),
                });
                false
            }
            Some(false) => false,
            None => true,
        }
    }

    fn hall_restore_ui(&mut self, ctx: &egui::Context, st: &mut HallRestoreState) -> bool {
        let mut result: Option<bool> = None;
        let r = egui::Modal::new(egui::Id::unique("hall-restore"))
            .frame(widgets::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(560.0);
                dialog_title(ui, s().banner_title);
                ui.add_space(theme::SP_RELATED);
                if st.choices.is_empty() {
                    widgets::hint(ui, s().banner_no_source);
                }
                for (i, c) in st.choices.iter().enumerate() {
                    ui.radio_value(&mut st.selected, i, c.label());
                }
                ui.add_space(theme::SP_RELATED);
                widgets::hint(ui, s().restore_hint);
                ui.add_space(theme::SP_GROUP);
                let can = !st.choices.is_empty() && self.can_write();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled_ui(can, |ui| {
                            widgets::primary_button(ui, s().restore_preview, 160.0)
                        })
                        .inner
                        .clicked()
                    {
                        result = Some(true);
                    }
                    if widgets::button(ui, s().cancel).clicked() {
                        result = Some(false);
                    }
                });
            });
        if r.should_close() && result.is_none() {
            result = Some(false);
        }
        match result {
            Some(true) => {
                let Some(c) = st.choices.get(st.selected) else {
                    return false;
                };
                let what = c.label();
                let req = match c {
                    HallChoice::PreSwitch(p) => PlanRequest::HallRaw {
                        block: p.block.clone(),
                        rt_precision: p.rt_precision,
                        origin: PlanOrigin::HallRestore(what),
                    },
                    HallChoice::Backup {
                        block,
                        rt_precision,
                        ..
                    } => PlanRequest::HallRaw {
                        block: block.clone(),
                        rt_precision: *rt_precision,
                        origin: PlanOrigin::HallRestore(what),
                    },
                    HallChoice::Preset(n, p) => {
                        let mut only = (**p).clone();
                        only.fn_layer = None;
                        only.lighting = None;
                        PlanRequest::Presets {
                            presets: vec![only],
                            origin: PlanOrigin::Preset(n.clone()),
                        }
                    }
                };
                self.request_plan(req);
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}

fn section_ru(sec: Section) -> &'static str {
    match sec {
        Section::Hall => s().section_hall,
        Section::FnLayer => s().section_fn,
        Section::Lighting => s().section_lighting,
    }
}

/// The diff table of a plan.
fn diff_table(ui: &mut Ui, plan: &Plan) {
    egui::Grid::new("diff-table")
        .num_columns(3)
        .spacing(vec2(18.0, 6.0))
        .striped(false)
        .show(ui, |ui| {
            let head = |ui: &mut Ui, t: &str| {
                ui.label(
                    RichText::new(t)
                        .font(font(theme::FS_SMALL, Weight::Medium))
                        .color(theme::HINT),
                );
            };
            head(ui, s().confirm_col_key);
            head(ui, s().confirm_col_before);
            head(ui, s().confirm_col_after);
            ui.end_row();
            let mut last_op = None;
            for row in &plan.model.rows {
                if last_op != Some(row.op) {
                    ui.label(
                        RichText::new(op_label(row.op))
                            .font(font(theme::FS_HINT, Weight::Semibold))
                            .color(theme::FG),
                    );
                    ui.end_row();
                    last_op = Some(row.op);
                }
                ui.label(RichText::new(&row.what).font(font(theme::FS_HINT, Weight::Medium)));
                let cell = |ui: &mut Ui, text: &str, color: Option<[u8; 3]>, strong: bool| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if let Some(c) = color {
                            widgets::swatch(ui, c, 14.0);
                        }
                        ui.label(
                            RichText::new(text)
                                .font(font(theme::FS_HINT, Weight::Regular))
                                .color(if strong { theme::FG } else { theme::HINT }),
                        );
                    });
                };
                cell(ui, &row.before, row.colors.map(|c| c.0), false);
                cell(ui, &row.after, row.colors.map(|c| c.1), true);
                ui.end_row();
            }
        });
    let _ = Color32::TRANSPARENT;
}

/// Run the native file picker off the UI thread.
fn spawn_picker(start: PathBuf) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut d = rfd::FileDialog::new().add_filter("type84 backup", &["json"]);
        if start.is_dir() {
            d = d.set_directory(&start);
        }
        let _ = tx.send(d.pick_file());
    });
    rx
}
