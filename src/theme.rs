use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Response, RichText, Sense, Shadow, Stroke, TextStyle, Ui, vec2,
};
use std::sync::Arc;

const ANIM: f32 = 0.12;

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    egui::lerp(egui::Rgba::from(a)..=egui::Rgba::from(b), t).into()
}

fn motion(ui: &Ui, response: &Response, enabled: bool) -> (f32, f32) {
    let enabled = enabled && response.enabled();
    (
        ui.ctx().animate_bool_with_time(
            response.id.with("hover"),
            enabled && response.hovered(),
            ANIM,
        ),
        ui.ctx().animate_bool_with_time(
            response.id.with("press"),
            enabled && (response.is_pointer_button_down_on() || response.clicked()),
            0.07,
        ),
    )
}

/// Widget bawaan digambar sebelum respons tersedia, jadi pakai interaksi frame sebelumnya.
pub fn interactive(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> Response) -> Response {
    let id = ui.next_auto_id().with("interaction");
    let previous = ui
        .ctx()
        .data(|d| d.get_temp::<(bool, bool)>(id))
        .unwrap_or_default();
    let hover =
        ui.ctx()
            .animate_bool_with_time(id.with("hover"), previous.0 && ui.is_enabled(), ANIM);
    let press =
        ui.ctx()
            .animate_bool_with_time(id.with("press"), previous.1 && ui.is_enabled(), 0.07);
    let response = ui
        .scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            let rest = w.inactive;
            let fill = mix(
                mix(rest.bg_fill, w.hovered.bg_fill, hover),
                w.active.bg_fill,
                press,
            );
            let weak = mix(
                mix(rest.weak_bg_fill, w.hovered.weak_bg_fill, hover),
                w.active.weak_bg_fill,
                press,
            );
            let border = mix(rest.bg_stroke.color, w.hovered.bg_stroke.color, hover);
            for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                state.bg_fill = fill;
                state.weak_bg_fill = weak;
                state.bg_stroke.color = border;
                state.expansion = hover - press;
            }
            add(ui)
        })
        .inner;
    let current = (
        response.enabled() && response.hovered(),
        response.enabled() && (response.is_pointer_button_down_on() || response.clicked()),
    );
    ui.ctx().data_mut(|d| d.insert_temp(id, current));
    if current != previous {
        ui.ctx().request_repaint();
    }
    response
}

pub fn combo(ui: &mut Ui, combo: egui::ComboBox, add: impl FnOnce(&mut Ui)) -> Response {
    let style = ui.style().clone();
    interactive(ui, |ui| {
        combo
            .show_ui(ui, |ui| {
                ui.set_style(style);
                add(ui);
            })
            .response
    })
}

// Fondasi
pub const BG: Color32 = Color32::from_rgb(0x0F, 0x0E, 0x0C);
pub const SURFACE: Color32 = Color32::from_rgb(0x18, 0x16, 0x13);
pub const RAISED: Color32 = Color32::from_rgb(0x22, 0x1F, 0x1B);
pub const BORDER: Color32 = Color32::from_rgb(0x2E, 0x2A, 0x24);
/// Area inset (hasil, input) memakai warna background agar tampak "masuk" ke dalam card.
pub const INPUT: Color32 = BG;
// Aksen
pub const ACCENT: Color32 = Color32::from_rgb(0xF5, 0x9E, 0x0B);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xFB, 0xBF, 0x24);
/// rgba(245, 158, 11, 0.12); dipakai untuk badge, highlight, dan glow.
pub const ACCENT_TINT: Color32 = Color32::from_rgba_unmultiplied_const(0xF5, 0x9E, 0x0B, 31);
pub const ACCENT_BORDER: Color32 = Color32::from_rgb(0x78, 0x35, 0x0F);
// Teks
pub const TEXT: Color32 = Color32::from_rgb(0xF5, 0xF5, 0xF4);
pub const SECONDARY: Color32 = Color32::from_rgb(0xA8, 0xA2, 0x9E);
pub const MUTED: Color32 = Color32::from_rgb(0x57, 0x53, 0x4E);
pub const ON_ACCENT: Color32 = BG;
// Status
pub const DANGER: Color32 = Color32::from_rgb(0xEF, 0x44, 0x44);
pub const SUCCESS: Color32 = Color32::from_rgb(0x10, 0xB9, 0x81);
/// Peringatan memakai amber terang agar beda dari aksi utama tetapi tetap dalam palet.
pub const WARN: Color32 = ACCENT_HOVER;
/// Warna pil tautan beranda, dibedakan dari amber (GitHub) dan hijau (status).
pub const LINK_HOME: Color32 = Color32::from_rgb(0x38, 0xBD, 0xF8);

/// Logo 128×128 RGBA (dibuat oleh scripts/make-logo.py).
pub const LOGO_SIZE: usize = 128;
pub const LOGO_RGBA: &[u8] = include_bytes!("../assets/logo-128.rgba");

pub fn icon() -> egui::IconData {
    egui::IconData {
        rgba: LOGO_RGBA.to_vec(),
        width: LOGO_SIZE as u32,
        height: LOGO_SIZE as u32,
    }
}

pub fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    ctx.load_texture(
        "logo",
        egui::ColorImage::from_rgba_unmultiplied([LOGO_SIZE, LOGO_SIZE], LOGO_RGBA),
        egui::TextureOptions::LINEAR,
    )
}

/// Ikon Octicons putih 64×64 (premultiplied), diwarnai lewat tint.
const ICON_SIZE: usize = 64;
const GITHUB_RGBA: &[u8] = include_bytes!("../assets/icons/mark-github-24-64.rgba");
const HOME_RGBA: &[u8] = include_bytes!("../assets/icons/home-fill-24-64.rgba");
const MINIMIZE_RGBA: &[u8] = include_bytes!("../assets/icons/dash-24-64.rgba");
const CLOSE_RGBA: &[u8] = include_bytes!("../assets/icons/x-24-64.rgba");

pub struct Icons {
    pub github: egui::TextureHandle,
    pub home: egui::TextureHandle,
    pub minimize: egui::TextureHandle,
    pub close: egui::TextureHandle,
}

impl Icons {
    pub fn load(ctx: &egui::Context) -> Self {
        let load = |name, rgba| {
            ctx.load_texture(
                name,
                egui::ColorImage::from_rgba_premultiplied([ICON_SIZE, ICON_SIZE], rgba),
                egui::TextureOptions::LINEAR,
            )
        };
        Self {
            github: load("icon-github", GITHUB_RGBA),
            home: load("icon-home", HOME_RGBA),
            minimize: load("icon-minimize", MINIMIZE_RGBA),
            close: load("icon-close", CLOSE_RGBA),
        }
    }
}

/// Pil berbentuk `badge` yang bisa diklik: ikon (dan teks opsional) dengan warna `color`.
fn pill(ui: &mut Ui, icon: &egui::TextureHandle, text: Option<&str>, color: Color32) -> Response {
    let mut frame = Frame::new()
        .fill(color.gamma_multiply(0.14))
        .corner_radius(99)
        .inner_margin(Margin::symmetric(10, 4))
        .begin(ui);
    {
        let ui = &mut frame.content_ui;
        ui.spacing_mut().item_spacing.x = 6.0;
        // Induknya right_to_left (header): teks ditambahkan dulu agar ikon tampil di kiri.
        if let Some(text) = text {
            ui.label(RichText::new(text).font(medium(12.0)).color(color));
        }
        ui.add(egui::Image::new((icon.id(), vec2(14.0, 14.0))).tint(color));
    }
    let response = frame
        .allocate_space(ui)
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let (hover, press) = motion(ui, &response, true);
    frame.frame.fill = color.gamma_multiply(0.14 + 0.10 * hover + 0.08 * press);
    frame.paint(ui);
    response
}

/// Pil tautan: membuka `url` di browser.
pub fn link_badge(
    ui: &mut Ui,
    icon: &egui::TextureHandle,
    text: Option<&str>,
    color: Color32,
    url: &str,
) {
    let response = pill(ui, icon, text, color).on_hover_text(url);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, url));
    if response.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// Pil kontrol jendela (minimize/close) dengan gaya yang sama seperti `link_badge`.
pub fn window_badge(ui: &mut Ui, icon: &egui::TextureHandle, color: Color32, label: &str) -> bool {
    let response = pill(ui, icon, None, color).on_hover_text(label);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    response.clicked()
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
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
            (TextStyle::Button, medium(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(14.0, 7.0);
        style.spacing.interact_size.y = 34.0;
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.scroll = egui::style::ScrollStyle::thin();
        let v = &mut style.visuals;
        v.panel_fill = BG;
        // Dropdown/popup memakai surface elevated.
        v.window_fill = RAISED;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(10);
        v.popup_shadow = Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        v.window_shadow = v.popup_shadow;
        v.extreme_bg_color = INPUT;
        v.text_edit_bg_color = Some(INPUT);
        v.faint_bg_color = SURFACE;
        v.code_bg_color = RAISED;
        v.hyperlink_color = ACCENT;
        v.weak_text_color = Some(SECONDARY);
        v.override_text_color = None;
        // Highlight/selection: tint amber dengan teks amber terang (juga item dropdown terpilih).
        v.selection.bg_fill = ACCENT_TINT;
        v.selection.stroke = Stroke::new(1.0, ACCENT_HOVER);
        v.error_fg_color = DANGER;
        v.warn_fg_color = WARN;
        v.interact_cursor = Some(egui::CursorIcon::PointingHand);
        v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
        for (state, fill, stroke, text) in [
            (&mut v.widgets.noninteractive, SURFACE, BORDER, TEXT),
            (&mut v.widgets.inactive, RAISED, BORDER, TEXT),
            (&mut v.widgets.hovered, BORDER, ACCENT_BORDER, TEXT),
            (
                &mut v.widgets.active,
                Color32::from_rgb(0x38, 0x33, 0x2C),
                ACCENT_BORDER,
                TEXT,
            ),
            (&mut v.widgets.open, RAISED, ACCENT_BORDER, TEXT),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, text);
            state.corner_radius = CornerRadius::same(8);
            state.expansion = 0.0;
        }
        // Label non-interaktif memakai TEXT; ikon/garis pemisah memakai border.
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    });
}

pub fn card() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(14)
        .inner_margin(Margin::same(18))
}

/// Area inset di dalam card (hasil transkripsi, input).
pub fn inset(focused: bool) -> Frame {
    Frame::new()
        .fill(INPUT)
        .stroke(Stroke::new(
            1.0,
            if focused { ACCENT_BORDER } else { BORDER },
        ))
        .corner_radius(10)
        .inner_margin(Margin::same(12))
}

pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(semibold(16.0)).color(TEXT));
}

pub fn field_label(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(12.5).color(SECONDARY));
}

/// Pil status: latar tint dengan teks berwarna.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(if color == ACCENT {
            ACCENT_TINT
        } else {
            color.gamma_multiply(0.14)
        })
        .corner_radius(99)
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).font(medium(12.0)).color(color));
        });
}

/// Tombol aksi utama amber: teks off-black, hover lebih terang, glow tint saat `glow`.
pub fn primary(ui: &mut Ui, enabled: bool, glow: bool, text: &str) -> Response {
    let font = medium(15.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), ON_ACCENT);
    let size = vec2((galley.size().x + 40.0).max(140.0), 44.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let (hover, press) = motion(ui, &response, enabled);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let (fill, fg) = if !enabled {
            (RAISED, MUTED)
        } else {
            (mix(ACCENT, ACCENT_HOVER, hover * (1.0 - press)), ON_ACCENT)
        };
        let rect = rect.shrink(press);
        if enabled {
            painter.rect_filled(
                rect.expand(5.0),
                15.0,
                ACCENT_TINT.gamma_multiply(if glow { 1.0 } else { hover }),
            );
        }
        painter.rect_filled(rect, 10.0, fill);
        if !enabled {
            painter.rect_stroke(
                rect,
                10.0,
                Stroke::new(1.0, BORDER),
                egui::StrokeKind::Inside,
            );
        }
        let galley = painter.layout_no_wrap(text.to_owned(), font, fg);
        painter.galley(rect.center() - galley.size() / 2.0, galley, fg);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    response
}

/// Tombol sekunder dengan ukuran yang sejajar dengan tombol utama.
pub fn secondary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    interactive(ui, |ui| {
        ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(text).color(if enabled { TEXT } else { MUTED }))
                .min_size(vec2(0.0, 34.0)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_and_press_animate_without_moving_layout_and_disabled_stays_inert() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut time = 0.0;
        let mut frame = |events, enabled| {
            time += 1.0 / 60.0;
            let mut sample = (egui::Rect::NOTHING, RAISED, false);
            let _ = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.add_enabled_ui(enabled, |ui| {
                        let response = interactive(ui, |ui| {
                            sample.1 = ui.visuals().widgets.inactive.bg_fill;
                            ui.button("Uji")
                        });
                        sample.0 = response.rect;
                        sample.2 = response.clicked();
                    });
                },
            );
            sample
        };
        let rest = frame(vec![], true);
        let pos = rest.0.center();
        frame(vec![egui::Event::PointerMoved(pos)], true);
        let mut samples = Vec::new();
        for _ in 0..12 {
            let sample = frame(vec![], true);
            assert_eq!(sample.0, rest.0);
            samples.push(sample.1);
        }
        assert!(samples.iter().any(|c| *c != RAISED && *c != BORDER));
        assert_eq!(*samples.last().unwrap(), BORDER);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        frame(vec![button(true)], true);
        for _ in 0..8 {
            frame(vec![], true);
        }
        let pressed = frame(vec![], true);
        assert_eq!(pressed.0, rest.0);
        assert_ne!(pressed.1, BORDER);
        assert!(frame(vec![button(false)], true).2);
        for _ in 0..12 {
            frame(vec![], false);
        }
        assert_eq!(frame(vec![], false).1, RAISED);
        assert!(!frame(vec![button(true), button(false)], false).2);
    }
}
