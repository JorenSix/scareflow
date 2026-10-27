//! Dark Halloween look: near-black purple night, pumpkin orange, slime green, candle glow.

use eframe::egui::{self, Color32, Stroke};
use egui_snarl::ui::SnarlStyle;

use crate::graph::Category;

pub const NIGHT: Color32 = Color32::from_rgb(14, 10, 18);
pub const PANEL: Color32 = Color32::from_rgb(22, 16, 28);
pub const BLOCK: Color32 = Color32::from_rgb(34, 26, 42);
pub const BONE: Color32 = Color32::from_rgb(232, 222, 205);
pub const PUMPKIN: Color32 = Color32::from_rgb(255, 120, 20);
pub const SLIME: Color32 = Color32::from_rgb(140, 220, 60);
pub const CANDLE: Color32 = Color32::from_rgb(255, 200, 60);
pub const BLOOD: Color32 = Color32::from_rgb(150, 10, 20);

/// Trigger cables and pins.
pub const TRIGGER: Color32 = PUMPKIN;
/// Value cables and pins.
pub const VALUE: Color32 = SLIME;

pub fn category_color(category: Category) -> Color32 {
    match category {
        Category::Event => Color32::from_rgb(90, 40, 130),
        Category::Flow => Color32::from_rgb(40, 75, 60),
        Category::Response => Color32::from_rgb(150, 60, 10),
    }
}

/// Applies the theme regardless of the system's light or dark setting.
pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(BONE);
    v.weak_text_color = Some(Color32::from_rgb(150, 135, 160));
    v.panel_fill = PANEL;
    v.window_fill = BLOCK;
    v.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(70, 50, 80));
    v.extreme_bg_color = Color32::from_rgb(10, 7, 13);
    v.faint_bg_color = Color32::from_rgb(28, 21, 34);
    v.code_bg_color = Color32::from_rgb(28, 21, 34);
    v.hyperlink_color = PUMPKIN;
    v.selection.bg_fill = Color32::from_rgb(170, 70, 10);
    v.selection.stroke = Stroke::new(1.0_f32, CANDLE);
    v.warn_fg_color = CANDLE;
    v.error_fg_color = Color32::from_rgb(255, 70, 70);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BLOCK;
    w.noninteractive.weak_bg_fill = BLOCK;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(60, 45, 70));
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, BONE);
    for (state, fill) in [
        (&mut w.inactive, Color32::from_rgb(48, 36, 58)),
        (&mut w.hovered, Color32::from_rgb(80, 52, 90)),
        (&mut w.active, Color32::from_rgb(130, 60, 15)),
        (&mut w.open, Color32::from_rgb(60, 42, 72)),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
    }
    w.hovered.bg_stroke = Stroke::new(1.0_f32, PUMPKIN);
    w.active.bg_stroke = Stroke::new(1.0_f32, CANDLE);

    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.set_theme(egui::ThemePreference::Dark);
}

pub fn snarl_style() -> SnarlStyle {
    SnarlStyle {
        bg_frame: Some(egui::Frame::new().fill(NIGHT)),
        bg_pattern_stroke: Some(Stroke::new(1.0_f32, Color32::from_rgb(38, 28, 46))),
        wire_width: Some(3.0),
        select_stoke: Some(Stroke::new(1.0_f32, PUMPKIN)),
        select_fill: Some(Color32::from_rgba_unmultiplied(255, 120, 20, 30)),
        ..SnarlStyle::new()
    }
}
