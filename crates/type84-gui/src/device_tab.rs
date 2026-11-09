//! The "Устройство" tab: model, firmware, link, and the program settings
//! (close to tray).

use egui::{vec2, RichText, Ui};

use crate::app::App;
use crate::i18n::s;
use crate::theme::{self, font, Weight};
use crate::widgets::{self, Tri};

impl App {
    pub(crate) fn device_tab(&mut self, ui: &mut Ui) {
        let fw = self
            .board
            .as_ref()
            .map(|b| b.firmware.clone())
            .unwrap_or_default();
        let link = if self.is_backup() {
            s().status_backup.to_string()
        } else {
            s().connected_cable.to_string()
        };
        widgets::panel_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::title(ui, s().device_title);
            ui.add_space(theme::SP_CONTROL);
            egui::Grid::new("device-rows")
                .num_columns(2)
                .spacing(vec2(24.0, 12.0))
                .show(ui, |ui| {
                    for (k, v) in [
                        (s().device_model, s().device_name.to_string()),
                        (s().device_firmware, fw),
                        (s().device_link, link),
                        (s().device_power, s().device_no_battery.to_string()),
                    ] {
                        ui.label(
                            RichText::new(k)
                                .font(font(theme::FS_BODY, Weight::Regular))
                                .color(theme::HINT),
                        );
                        ui.label(RichText::new(v).font(font(theme::FS_BODY, Weight::Medium)));
                        ui.end_row();
                    }
                });
        });
        ui.add_space(theme::SP_SECTION);
        widgets::panel_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::title(ui, s().app_settings);
            ui.add_space(theme::SP_CONTROL);
            ui.horizontal(|ui| {
                let st = if self.settings.close_to_tray {
                    Tri::On
                } else {
                    Tri::Off
                };
                if widgets::toggle(ui, st, s().close_to_tray).clicked() {
                    self.set_close_to_tray(!self.settings.close_to_tray);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    widgets::title(ui, s().close_to_tray);
                    widgets::hint(ui, s().close_to_tray_hint);
                });
            });
        });
        ui.add_space(theme::SP_SECTION);
    }
}
