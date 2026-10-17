use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, RichText, Stroke,
    TextStyle, Ui, vec2,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x0A, 0x0D, 0x0E);
pub const SURFACE: Color32 = Color32::from_rgb(0x10, 0x15, 0x16);
pub const RAISED: Color32 = Color32::from_rgb(0x18, 0x20, 0x21);
pub const INPUT: Color32 = Color32::from_rgb(0x0C, 0x11, 0x12);
pub const BORDER: Color32 = Color32::from_rgb(0x1F, 0x29, 0x2A);
pub const TEXT: Color32 = Color32::from_rgb(0xDC, 0xE8, 0xE7);
pub const MUTED: Color32 = Color32::from_rgb(0x8E, 0xA3, 0xA2);
pub const ACCENT: Color32 = Color32::from_rgb(0x4F, 0xD1, 0xC5);
pub const DANGER: Color32 = Color32::from_rgb(0xF0, 0x72, 0x72);
pub const WARN: Color32 = Color32::from_rgb(0xF0, 0xBB, 0x5A);

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("inter", &include_bytes!("../fonts/Inter-Regular.ttf")[..]),
        ("medium", include_bytes!("../fonts/Inter-Medium.ttf")),
        ("semibold", include_bytes!("../fonts/Inter-SemiBold.ttf")),
        ("mono", include_bytes!("../fonts/JetBrainsMono-Regular.ttf")),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    for name in ["medium", "semibold"] {
        let mut family = vec![name.to_owned()];
        family.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), family);
    }
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .unwrap()
        .insert(0, "inter".into());
    fonts
        .families
        .get_mut(&FontFamily::Monospace)
        .unwrap()
        .insert(0, "mono".into());
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (TextStyle::Heading, semibold(20.0)),
            (TextStyle::Body, FontId::proportional(14.5)),
            (
                TextStyle::Button,
                FontId::new(14.0, FontFamily::Name("medium".into())),
            ),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(14.0, 7.0);
        style.spacing.interact_size.y = 34.0;
        style.spacing.scroll = egui::style::ScrollStyle::thin();
        let v = &mut style.visuals;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = INPUT;
        v.text_edit_bg_color = Some(INPUT);
        v.weak_text_color = Some(MUTED);
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.3);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.error_fg_color = DANGER;
        v.warn_fg_color = WARN;
        v.interact_cursor = Some(egui::CursorIcon::PointingHand);
        for (state, fill) in [
            (&mut v.widgets.noninteractive, SURFACE),
            (&mut v.widgets.inactive, RAISED),
            (&mut v.widgets.hovered, BORDER),
            (&mut v.widgets.active, Color32::from_rgb(0x27, 0x33, 0x34)),
            (&mut v.widgets.open, RAISED),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, BORDER);
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.corner_radius = egui::CornerRadius::same(8);
            state.expansion = 0.0;
        }
    });
}

pub fn card() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(14)
        .inner_margin(Margin::same(16))
}

pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(semibold(17.0)));
}

pub fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(MUTED));
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(color.gamma_multiply(0.12))
        .corner_radius(99)
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color));
        });
}

pub fn primary(ui: &mut Ui, enabled: bool, text: &str) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(BG))
            .fill(ACCENT)
            .min_size(vec2(150.0, 42.0)),
    )
}
