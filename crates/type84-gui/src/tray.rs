//! Tray icon. Linux: StatusNotifierItem over D-Bus (`ksni`; KDE Plasma and
//! other SNI hosts). Windows: `tray-icon`. Elsewhere: none.
//!
//! The menu shows which saved presets the board currently matches per
//! section, «Применить пресет ▸» (opens the window and the confirm dialog;
//! the tray never writes by itself), «Сделать бэкап», «Открыть» and
//! «Выход». Window commands are sent from the tray side directly, because
//! eframe does not run the UI while the window is hidden.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::i18n::{fill, s};

/// What the menu shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayModel {
    pub tooltip: String,
    /// (section label, matching preset names or "—").
    pub current: Vec<(String, String)>,
    pub presets: Vec<String>,
    pub close_to_tray: bool,
}

/// Menu actions handled by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    ApplyPreset(String),
    Backup,
    ToggleCloseToTray,
}

/// Show and focus the window.
pub fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// Hide the window to the tray. Wayland has no way for a client to hide
/// its own window (winit ignores `Visible(false)` there), so it minimises.
pub fn hide_window(ctx: &egui::Context) {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("WINIT_UNIX_BACKEND").map_or(true, |b| b != "x11");
    if cfg!(target_os = "linux") && wayland {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    } else {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
}

/// Shared by both platform implementations.
#[derive(Clone)]
struct Glue {
    ctx: egui::Context,
    tx: Sender<TrayAction>,
    quit: Arc<AtomicBool>,
    /// The worker, so «Сделать бэкап» works while the window is hidden.
    worker: Option<Sender<crate::worker::Command>>,
}

impl Glue {
    fn open(&self) {
        show_window(&self.ctx);
    }

    fn act(&self, a: TrayAction) {
        if a == TrayAction::Backup {
            if let Some(w) = &self.worker {
                let _ = w.send(crate::worker::Command::Backup);
                return;
            }
        }
        if matches!(a, TrayAction::ApplyPreset(_)) {
            show_window(&self.ctx);
        }
        let _ = self.tx.send(a);
        self.ctx.request_repaint();
    }

    fn quit(&self) {
        self.quit.store(true, Ordering::SeqCst);
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        self.ctx.request_repaint();
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use ksni::blocking::TrayMethods as _;

    struct SniTray {
        glue: Glue,
        model: TrayModel,
    }

    impl ksni::Tray for SniTray {
        fn id(&self) -> String {
            "keyboard-hub".into()
        }
        fn title(&self) -> String {
            self.model.tooltip.clone()
        }
        fn category(&self) -> ksni::Category {
            ksni::Category::Hardware
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            [22, 32, 48, 64]
                .into_iter()
                .map(|n| ksni::Icon {
                    width: n as i32,
                    height: n as i32,
                    data: crate::icon::argb(n),
                })
                .collect()
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            let desc = self
                .model
                .current
                .iter()
                .map(|(sec, n)| format!("{sec}: {n}"))
                .collect::<Vec<_>>()
                .join("\n");
            ksni::ToolTip {
                title: self.model.tooltip.clone(),
                description: desc,
                ..Default::default()
            }
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            self.glue.open();
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            let mut items: Vec<ksni::MenuItem<Self>> = Vec::new();
            for (sec, names) in &self.model.current {
                items.push(
                    StandardItem {
                        label: fill(s().tray_current, &[("section", sec), ("names", names)]),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
            }
            if !items.is_empty() {
                items.push(MenuItem::Separator);
            }
            let presets: Vec<ksni::MenuItem<Self>> = self
                .model
                .presets
                .iter()
                .map(|n| {
                    let name = n.clone();
                    StandardItem {
                        label: n.replace('_', "__"),
                        activate: Box::new(move |t: &mut Self| {
                            t.glue.act(TrayAction::ApplyPreset(name.clone()))
                        }),
                        ..Default::default()
                    }
                    .into()
                })
                .collect();
            items.push(
                SubMenu {
                    label: s().tray_apply.into(),
                    enabled: !presets.is_empty(),
                    submenu: presets,
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                StandardItem {
                    label: s().tray_backup.into(),
                    activate: Box::new(|t: &mut Self| t.glue.act(TrayAction::Backup)),
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                CheckmarkItem {
                    label: s().close_to_tray.into(),
                    checked: self.model.close_to_tray,
                    activate: Box::new(|t: &mut Self| t.glue.act(TrayAction::ToggleCloseToTray)),
                    ..Default::default()
                }
                .into(),
            );
            items.push(MenuItem::Separator);
            items.push(
                StandardItem {
                    label: s().tray_open.into(),
                    activate: Box::new(|t: &mut Self| t.glue.open()),
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                StandardItem {
                    label: s().tray_quit.into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(|t: &mut Self| t.glue.quit()),
                    ..Default::default()
                }
                .into(),
            );
            items
        }
    }

    pub struct Tray {
        handle: ksni::blocking::Handle<SniTray>,
    }

    impl Tray {
        pub fn spawn(
            ctx: egui::Context,
            tx: Sender<TrayAction>,
            quit: Arc<AtomicBool>,
            worker: Option<Sender<crate::worker::Command>>,
            model: TrayModel,
        ) -> Option<Tray> {
            let t = SniTray {
                glue: Glue {
                    ctx,
                    tx,
                    quit,
                    worker,
                },
                model,
            };
            match t.spawn() {
                Ok(handle) => Some(Tray { handle }),
                Err(e) => {
                    eprintln!("note: no tray icon ({e})");
                    None
                }
            }
        }

        pub fn update(&self, model: TrayModel) {
            self.handle.update(move |t: &mut SniTray| t.model = model);
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

    fn build_menu(m: &TrayModel) -> Menu {
        let menu = Menu::new();
        for (sec, names) in &m.current {
            let _ = menu.append(&MenuItem::new(
                fill(s().tray_current, &[("section", sec), ("names", names)]),
                false,
                None,
            ));
        }
        if !m.current.is_empty() {
            let _ = menu.append(&PredefinedMenuItem::separator());
        }
        let sub = Submenu::new(s().tray_apply, !m.presets.is_empty());
        for n in &m.presets {
            let _ = sub.append(&MenuItem::with_id(format!("preset:{n}"), n, true, None));
        }
        let _ = menu.append(&sub);
        let _ = menu.append(&MenuItem::with_id("backup", s().tray_backup, true, None));
        let _ = menu.append(&CheckMenuItem::with_id(
            "close-to-tray",
            s().close_to_tray,
            true,
            m.close_to_tray,
            None,
        ));
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id("open", s().tray_open, true, None));
        let _ = menu.append(&MenuItem::with_id("quit", s().tray_quit, true, None));
        menu
    }

    /// Lives on the main thread (the tray icon is not `Send` on Windows).
    pub struct Tray {
        icon: TrayIcon,
    }

    impl Tray {
        pub fn spawn(
            ctx: egui::Context,
            tx: Sender<TrayAction>,
            quit: Arc<AtomicBool>,
            worker: Option<Sender<crate::worker::Command>>,
            model: TrayModel,
        ) -> Option<Tray> {
            let glue = Glue {
                ctx,
                tx,
                quit,
                worker,
            };
            let g = glue.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let id = e.id().0.as_str();
                match id {
                    "open" => g.open(),
                    "quit" => g.quit(),
                    "backup" => g.act(TrayAction::Backup),
                    "close-to-tray" => g.act(TrayAction::ToggleCloseToTray),
                    _ => {
                        if let Some(n) = id.strip_prefix("preset:") {
                            g.act(TrayAction::ApplyPreset(n.to_string()));
                        }
                    }
                }
            }));
            let g = glue.clone();
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::DoubleClick { .. } = e {
                    g.open();
                }
            }));
            let icon = tray_icon::Icon::from_rgba(crate::icon::rgba(32), 32, 32).ok()?;
            match TrayIconBuilder::new()
                .with_id("keyboard-hub")
                .with_tooltip(&model.tooltip)
                .with_icon(icon)
                .with_menu(Box::new(build_menu(&model)))
                .build()
            {
                Ok(icon) => Some(Tray { icon }),
                Err(e) => {
                    eprintln!("note: no tray icon ({e})");
                    None
                }
            }
        }

        pub fn update(&self, model: TrayModel) {
            self.icon.set_menu(Some(Box::new(build_menu(&model))));
            let _ = self.icon.set_tooltip(Some(&model.tooltip));
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod imp {
    use super::*;

    pub struct Tray;

    impl Tray {
        pub fn spawn(
            _ctx: egui::Context,
            _tx: Sender<TrayAction>,
            _quit: Arc<AtomicBool>,
            _worker: Option<Sender<crate::worker::Command>>,
            _model: TrayModel,
        ) -> Option<Tray> {
            None
        }

        pub fn update(&self, _model: TrayModel) {}
    }
}

pub use imp::Tray;
