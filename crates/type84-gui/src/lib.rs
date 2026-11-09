//! Keyboard Hub: desktop configurator for the IO by Red Square Type 84
//! Magnetic keyboard (egui).
//!
//! Edits stay local until «Применить». A write is planned by the worker
//! against a fresh read (the CLI's preview code), shown as a per-key diff
//! in a confirm dialog, and only then written through
//! `type84_cli::apply::run_steps`: write lock, automatic pre-write backup,
//! the guarded `type84_hid::WriteSession::apply_plan` and readback
//! verification. Keyboard-wide settings (0x21) and layer 0 (0x22) are
//! read-only.

pub mod actions;
pub mod app;
pub mod backend;
pub mod confirm;
mod device_tab;
pub mod dialogs;
pub mod geometry;
pub mod headless;
pub mod i18n;
pub mod icon;
pub mod keyboard;
mod keymap_tab;
mod lighting_tab;
pub mod model;
pub mod paths;
mod response;
pub mod theme;
pub mod tray;
pub mod widgets;
pub mod worker;

pub use app::App;
