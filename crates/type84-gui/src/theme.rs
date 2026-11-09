//! Design tokens from the private hub layout notes (not published) §2 and their mapping onto egui's
//! `Style`/`Visuals`, plus font setup (Inter, SIL OFL 1.1; see
//! `assets/fonts/OFL.txt`).

use std::sync::Arc;

use egui::epaint::text::{FontData, FontDefinitions, FontFamily};
use egui::{Color32, CornerRadius, FontId, Margin, Stroke, Vec2};

// Colors. Glass tones are composited onto the solid page background so the
// result matches the screenshots without real translucency.
pub const BG: Color32 = Color32::from_rgb(0x0e, 0x10, 0x13);
pub const BG2: Color32 = Color32::from_rgb(0x14, 0x17, 0x1c);
pub const PANEL: Color32 = Color32::from_rgb(0x13, 0x14, 0x16);
pub const PANEL2: Color32 = Color32::from_rgb(0x21, 0x26, 0x2f);
pub const CARD_LOWER: Color32 = Color32::from_rgb(0x17, 0x19, 0x1b);
pub const PLATE: Color32 = Color32::from_rgb(0x24, 0x25, 0x28);
pub const LINE: Color32 = Color32::from_rgb(0x2b, 0x31, 0x3b);
pub const FG: Color32 = Color32::from_rgb(0xf7, 0xf7, 0xf7);
pub const MUTED: Color32 = Color32::from_rgb(0xd6, 0xd6, 0xd6);
pub const HINT: Color32 = Color32::from_rgb(0xc0, 0xc0, 0xc1);
pub const DIM: Color32 = Color32::from_rgb(0x81, 0x82, 0x83);
pub const ACCENT: Color32 = Color32::from_rgb(0xe5, 0x2b, 0x36);
pub const GOOD: Color32 = Color32::from_rgb(0x35, 0xc0, 0x7a);
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x38);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x5f, 0x6d);
pub const DANGER_TEXT: Color32 = Color32::from_rgb(0xff, 0xb4, 0xbf);
pub const DISABLED: Color32 = Color32::from_rgb(0xa4, 0xa4, 0xa4);
/// Text on the white "active" fill (`--wp-panel2`).
pub const ON_ACTIVE: Color32 = PANEL2;
/// "Active / selected" fill (`--interaction-active-background`).
pub const ACTIVE: Color32 = FG;
/// Control surface (`--glass-control` #ffffff14 over the panel).
pub const CONTROL: Color32 = Color32::from_rgb(0x25, 0x26, 0x28);
pub const CONTROL_HOVER: Color32 = Color32::from_rgb(0x34, 0x35, 0x37);
pub const CONTROL_BORDER: Color32 = Color32::from_rgb(0x55, 0x56, 0x58);
pub const VALUE_BOX: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x22);
pub const DIVIDER: Color32 = Color32::from_rgb(0x3a, 0x3b, 0x3d);
pub const PANEL_BORDER: Color32 = Color32::from_rgb(0x46, 0x47, 0x4a);
/// Key face/edge/hover (`--glass-key-*`) composited over the plate.
pub const KEY_FACE: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x20);
pub const KEY_EDGE: Color32 = Color32::from_rgb(0x5d, 0x5e, 0x61);
pub const KEY_HOVER: Color32 = Color32::from_rgb(0x36, 0x37, 0x3a);
pub const KEY_PLATE: Color32 = Color32::from_rgb(0x11, 0x12, 0x14);

// Radii.
pub const R_PANEL: u8 = 16;
pub const R_TOOL: u8 = 12;
pub const R_CONTROL: u8 = 7;
pub const R_KEY: u8 = 7;

// Spacing (`--wp-space-*`).
pub const SP_CAPTION: f32 = 4.0;
pub const SP_CONTROL: f32 = 8.0;
pub const SP_RELATED: f32 = 12.0;
pub const SP_GROUP: f32 = 16.0;
pub const SP_FIELD: f32 = 20.0;
pub const SP_SECTION: f32 = 24.0;
pub const SP_COLUMN: f32 = 28.0;
pub const PANEL_INSET: f32 = 20.0;

// Control anatomy.
pub const TRACK: f32 = 28.0;
pub const CONTROL_INSET: f32 = 3.0;
pub const THUMB: f32 = TRACK - 2.0 * CONTROL_INSET;
pub const BUTTON_H: f32 = 36.0;

// Type scale.
pub const FS_TITLE: f32 = 20.0;
pub const FS_BODY: f32 = 14.0;
pub const FS_HINT: f32 = 13.0;
pub const FS_SMALL: f32 = 12.0;
pub const FS_MICRO: f32 = 11.0;

const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

/// Font weights available to the UI. egui has no weight axis, so each
/// weight is its own named family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
}

pub fn font(size: f32, weight: Weight) -> FontId {
    let family = match weight {
        Weight::Regular => FontFamily::Proportional,
        Weight::Medium => FontFamily::Name("medium".into()),
        Weight::Semibold => FontFamily::Name("semibold".into()),
    };
    FontId::new(size, family)
}

fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    for (name, bytes) in [
        ("inter", INTER_REGULAR),
        ("inter-medium", INTER_MEDIUM),
        ("inter-semibold", INTER_SEMIBOLD),
    ] {
        defs.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // egui's bundled fonts stay as fallbacks for any glyph Inter lacks.
    let fallbacks = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let with = |primary: &str| {
        let mut v = vec![primary.to_string()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    defs.families
        .insert(FontFamily::Proportional, with("inter"));
    defs.families
        .insert(FontFamily::Name("medium".into()), with("inter-medium"));
    defs.families
        .insert(FontFamily::Name("semibold".into()), with("inter-semibold"));
    defs
}

/// Install fonts and the dark style on a context. Used by the app and by
/// the render tests so both draw identically.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.all_styles_mut(style_tweaks);
}

fn style_tweaks(style: &mut egui::Style) {
    use egui::{FontFamily as F, TextStyle as T};
    style.text_styles = [
        (T::Small, FontId::new(FS_SMALL, F::Proportional)),
        (T::Body, FontId::new(FS_BODY, F::Proportional)),
        (T::Button, font(FS_BODY, Weight::Medium)),
        (T::Heading, font(FS_TITLE, Weight::Semibold)),
        (T::Monospace, FontId::new(FS_BODY, F::Monospace)),
    ]
    .into();
    style.animation_time = 0.0;
    style.spacing.item_spacing = Vec2::new(SP_CONTROL, SP_CONTROL);
    style.spacing.button_padding = Vec2::new(12.0, 7.0);
    style.spacing.interact_size = Vec2::new(36.0, 28.0);
    style.spacing.combo_width = 170.0;
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.window_margin = Margin::same(12);

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.override_text_color = Some(FG);
    v.panel_fill = BG;
    v.window_fill = Color32::from_rgb(0x0d, 0x0d, 0x0d);
    v.window_stroke = Stroke::new(1.0, PANEL_BORDER);
    v.window_corner_radius = CornerRadius::same(R_TOOL);
    v.menu_corner_radius = CornerRadius::same(R_CONTROL);
    v.extreme_bg_color = VALUE_BOX;
    v.faint_bg_color = BG2;
    v.code_bg_color = CONTROL;
    v.text_edit_bg_color = Some(VALUE_BOX);
    v.selection.bg_fill = Color32::from_rgb(0x4a, 0x4b, 0x50);
    v.selection.stroke = Stroke::new(1.0, FG);
    v.hyperlink_color = FG;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.slider_trailing_fill = false;

    let radius = CornerRadius::same(R_CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, DIVIDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, FG);
    w.noninteractive.corner_radius = radius;
    w.inactive.bg_fill = CONTROL;
    w.inactive.weak_bg_fill = CONTROL;
    w.inactive.bg_stroke = Stroke::new(1.0, CONTROL_BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, FG);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = CONTROL_HOVER;
    w.hovered.weak_bg_fill = CONTROL_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, MUTED);
    w.hovered.fg_stroke = Stroke::new(1.0, FG);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = CONTROL_HOVER;
    w.active.weak_bg_fill = CONTROL_HOVER;
    w.active.bg_stroke = Stroke::new(1.0, FG);
    w.active.fg_stroke = Stroke::new(1.0, FG);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;
    w.open.bg_fill = CONTROL_HOVER;
    w.open.weak_bg_fill = CONTROL_HOVER;
    w.open.bg_stroke = Stroke::new(1.0, MUTED);
    w.open.fg_stroke = Stroke::new(1.0, FG);
    w.open.corner_radius = radius;
}

/// Linear blend, used for dimming whole blocks.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
