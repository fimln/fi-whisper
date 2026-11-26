use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Response, RichText, Sense, Shadow, Stroke, TextStyle, Ui, vec2,
};
use std::sync::Arc;

pub const HOVER_ANIM: f32 = 0.14;
pub const PRESS_ANIM: f32 = 0.07;
pub const STATE_ANIM: f32 = 0.16;
pub const CARD_ANIM: f32 = 0.26;

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    egui::lerp(egui::Rgba::from(a)..=egui::Rgba::from(b), t).into()
}

pub fn animate_bool_ease(ctx: &egui::Context, id: egui::Id, value: bool, duration: f32) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, value, duration, egui::emath::easing::cubic_out)
}

/// Interpolasi warna di ruang linier Rgba agar transisi status tidak menggelap di tengah.
pub fn animate_color(ctx: &egui::Context, id: egui::Id, target: Color32, duration: f32) -> Color32 {
    let target = egui::Rgba::from(target).to_array();
    let channels = std::array::from_fn::<_, 4, _>(|i| {
        ctx.animate_value_with_time(id.with(i), target[i], duration)
    });
    egui::Rgba::from_rgba_premultiplied(channels[0], channels[1], channels[2], channels[3]).into()
}

fn motion(ui: &Ui, response: &Response, enabled: bool) -> (f32, f32) {
    let enabled = enabled && response.enabled();
    (
        animate_bool_ease(
            ui.ctx(),
            response.id.with("hover"),
            enabled && response.hovered(),
            HOVER_ANIM,
        ),
        animate_bool_ease(
            ui.ctx(),
            response.id.with("press"),
            enabled && (response.is_pointer_button_down_on() || response.clicked()),
            PRESS_ANIM,
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
    let hover = animate_bool_ease(
        ui.ctx(),
        id.with("hover"),
        previous.0 && ui.is_enabled(),
        HOVER_ANIM,
    );
    let press = animate_bool_ease(
        ui.ctx(),
        id.with("press"),
        previous.1 && ui.is_enabled(),
        PRESS_ANIM,
    );
    let response = ui
        .scope(|ui| {
            // Di dalam popup/menu (ScrollArea dropdown), expansion memotong border 1 px dan menggeser tepi item.
            let in_popup = ui
                .stack()
                .iter()
                .any(|s| matches!(s.kind(), Some(egui::UiKind::Popup | egui::UiKind::Menu)));
            let expansion = if in_popup { 0.0 } else { hover - press };
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
            let border = mix(
                mix(rest.bg_stroke.color, w.hovered.bg_stroke.color, hover),
                w.active.bg_stroke.color,
                press,
            );
            for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                state.bg_fill = fill;
                state.weak_bg_fill = weak;
                state.bg_stroke.color = border;
                state.expansion = expansion;
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

pub fn combo(
    ui: &mut Ui,
    combo: egui::ComboBox,
    chevron: &egui::TextureHandle,
    add: impl FnOnce(&mut Ui),
) -> Response {
    let style = ui.style().clone();
    let icon_id = ui.next_auto_id().with("combo-chevron");
    interactive(ui, |ui| {
        let chevron = chevron.id();
        combo
            .icon(move |ui, rect, _, is_open| {
                let open = animate_bool_ease(ui.ctx(), icon_id, is_open, STATE_ANIM);
                let tint = mix(SECONDARY, ACCENT_HOVER, open);
                egui::Image::new((chevron, vec2(13.0, 13.0)))
                    .tint(tint)
                    .rotate(open * std::f32::consts::PI, vec2(0.5, 0.5))
                    .paint_at(
                        ui,
                        egui::Rect::from_center_size(rect.center(), vec2(13.0, 13.0)),
                    );
            })
            .truncate()
            .popup_style(egui::style::StyleModifier::new(|style| {
                // Pindahkan 1 pt margin ke dalam ScrollArea agar border item tidak terpotong batas klip.
                style.spacing.menu_margin = Margin::same(5);
                style.spacing.scroll.content_margin = Margin::same(1);
            }))
            .show_ui(ui, |ui| {
                ui.set_style(style);
                add(ui);
            })
            .response
    })
}

// Fondasi
pub const BG: Color32 = Color32::from_gray(12);
pub const SURFACE: Color32 = Color32::from_gray(20);
pub const RAISED: Color32 = Color32::from_gray(28);
pub const BORDER: Color32 = Color32::from_gray(42);
/// Area inset (hasil, input) memakai warna background agar tampak "masuk" ke dalam card.
pub const INPUT: Color32 = Color32::from_gray(10);
// Aksen
pub const ACCENT: Color32 = Color32::from_rgb(0xF5, 0x9E, 0x0B);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xFB, 0xBF, 0x24);
pub const ACCENT_ACTIVE: Color32 = Color32::from_rgb(0xD9, 0x77, 0x06);
/// rgba(245, 158, 11, 0.12); dipakai untuk badge, highlight, dan glow.
pub const ACCENT_TINT: Color32 = Color32::from_rgba_unmultiplied_const(0xF5, 0x9E, 0x0B, 31);
pub const ACCENT_BORDER: Color32 = Color32::from_rgb(0x78, 0x35, 0x0F);
// Teks
pub const TEXT: Color32 = Color32::from_gray(245);
pub const SECONDARY: Color32 = Color32::from_gray(175);
pub const MUTED: Color32 = Color32::from_gray(112);
pub const ON_ACCENT: Color32 = Color32::from_rgb(26, 17, 0);
// Status
pub const DANGER: Color32 = Color32::from_rgb(0xEF, 0x44, 0x44);
pub const SUCCESS: Color32 = Color32::from_rgb(0x10, 0xB9, 0x81);
/// Peringatan memakai amber terang agar beda dari aksi utama tetapi tetap dalam palet.
pub const WARN: Color32 = ACCENT_HOVER;
/// Warna pil tautan beranda, dibedakan dari amber (GitHub) dan hijau (status).
pub const LINK_HOME: Color32 = Color32::from_rgb(0x38, 0xBD, 0xF8);

/// Logo 128×128 RGBA (dibuat oleh `cargo xtask make-logo`).
pub const LOGO_SIZE: usize = 128;
pub const LOGO_RGBA: &[u8] = include_bytes!("../assets/logo-128.rgba");

pub fn icon() -> egui::IconData {
    egui::IconData {
        rgba: LOGO_RGBA.to_vec(),
        width: LOGO_SIZE as u32,
        height: LOGO_SIZE as u32,
    }
}

pub struct Icons {
    pub check: egui::TextureHandle,
    pub caret: egui::TextureHandle,
    pub chevron: egui::TextureHandle,
    pub github: egui::TextureHandle,
    pub home: egui::TextureHandle,
    pub minimize: egui::TextureHandle,
    pub close: egui::TextureHandle,
}

impl Icons {
    pub fn load(ctx: &egui::Context) -> Self {
        let load = |name, alpha: &[u8]| {
            let pixels = alpha
                .iter()
                .map(|&a| Color32::from_white_alpha(a))
                .collect();
            ctx.load_texture(
                name,
                egui::ColorImage::new([64, 64], pixels),
                egui::TextureOptions::LINEAR,
            )
        };
        Self {
            github: load(
                "wire-github",
                include_bytes!("../assets/icons/wire-github.alpha"),
            ),
            home: load(
                "wire-home",
                include_bytes!("../assets/icons/wire-home.alpha"),
            ),
            minimize: load(
                "wire-minimize",
                include_bytes!("../assets/icons/wire-minimize.alpha"),
            ),
            close: load(
                "wire-close",
                include_bytes!("../assets/icons/wire-close.alpha"),
            ),
            chevron: load(
                "wire-chevron",
                include_bytes!("../assets/icons/wire-chevron.alpha"),
            ),
            check: load(
                "wire-check",
                include_bytes!("../assets/icons/wire-check.alpha"),
            ),
            caret: load(
                "wire-caret",
                include_bytes!("../assets/icons/wire-caret.alpha"),
            ),
        }
    }
}

/// Pil berbentuk `badge` yang bisa diklik: ikon (dan teks opsional) dengan warna `color`.
fn pill(ui: &mut Ui, icon: &egui::TextureHandle, text: Option<&str>, color: Color32) -> Response {
    let mut frame = Frame::new()
        .fill(color.gamma_multiply(0.14))
        .corner_radius(99)
        .inner_margin(if text.is_some() {
            Margin::symmetric(13, 7)
        } else {
            Margin::same(9)
        })
        .begin(ui);
    {
        let ui = &mut frame.content_ui;
        ui.spacing_mut().item_spacing.x = 6.0;
        // Induknya right_to_left (header): teks ditambahkan dulu agar ikon tampil di kiri.
        if let Some(text) = text {
            ui.label(RichText::new(text).font(medium(12.5)).color(color));
        }
        ui.add(egui::Image::new((icon.id(), vec2(16.0, 16.0))).tint(color));
    }
    let response = frame
        .allocate_space(ui)
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let (hover, press) = motion(ui, &response, true);
    frame.frame.fill = color.gamma_multiply(0.14 + 0.14 * hover + 0.10 * press);
    frame.frame.stroke = Stroke::new(1.0, color.gamma_multiply(0.35 * hover));
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
    // Jendela tanpa dekorasi memakai tema sendiri; hindari SetTheme native saat frame pertama.
    ctx.options_mut(|options| options.sync_window_theme = false);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.animation_time = STATE_ANIM;
        style.text_styles = [
            (TextStyle::Heading, semibold(20.0)),
            (TextStyle::Body, FontId::proportional(14.5)),
            (TextStyle::Button, medium(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 7.0);
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
                Color32::from_rgb(0x32, 0x2E, 0x28),
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
        .corner_radius(15)
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
        .inner_margin(Margin::same(13))
}

pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(semibold(16.0)).color(TEXT));
}

pub fn field_label(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(12.5).color(SECONDARY));
}

pub const BADGE_PADDING: i8 = 13;

/// Pil status: latar tint dengan teks berwarna.
pub fn status_badge(ui: &mut Ui, text: &str, color: Color32, background: Color32) {
    Frame::new()
        .fill(background.gamma_multiply(0.14))
        .corner_radius(99)
        .inner_margin(Margin::symmetric(BADGE_PADDING, 7))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(text).font(medium(12.5)).color(color)).truncate(),
            );
        })
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, text));
}

/// Input satu baris dengan bingkai inset dan highlight aksen saat hover/fokus.
pub fn text_field(
    ui: &mut Ui,
    id_salt: &str,
    text: &mut String,
    width: f32,
    hint: &str,
) -> Response {
    let id = egui::Id::unique(id_salt);
    let hover_key = id.with("hover");
    let prev_hover = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(hover_key))
        .unwrap_or(false);
    let focused = ui.memory(|m| m.has_focus(id));
    let hover = animate_bool_ease(
        ui.ctx(),
        hover_key,
        prev_hover && ui.is_enabled(),
        HOVER_ANIM,
    );
    let focus = animate_bool_ease(ui.ctx(), id.with("focus"), focused, STATE_ANIM);
    let border = mix(
        mix(BORDER, ACCENT_BORDER, 0.45 * hover),
        ACCENT_BORDER,
        focus,
    );
    let fill = mix(INPUT, Color32::from_gray(14), 0.5 * hover * (1.0 - focus));
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .desired_width(width)
            .hint_text(RichText::new(hint).color(MUTED))
            .frame(
                Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, border))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(11, 8)),
            ),
    );
    let now_hover = response.enabled() && response.hovered();
    ui.ctx().data_mut(|d| d.insert_temp(hover_key, now_hover));
    if now_hover != prev_hover {
        ui.ctx().request_repaint();
    }
    response
}

const ACTION_HEIGHT: f32 = 34.0;

/// Kedua tombol dikte memakai bingkai egui yang sama dan animasi hover/tekan di setiap keadaan.
pub fn dictation_button(
    ui: &mut Ui,
    enabled: bool,
    fill: Option<Color32>,
    width: f32,
    text: &str,
) -> Response {
    ui.scope(|ui| {
        let mut fg = if enabled { TEXT } else { MUTED };
        if let Some(fill) = fill {
            fg = if enabled { ON_ACCENT } else { MUTED };
            if enabled {
                let hover_fill = if fill == ACCENT_HOVER {
                    mix(ACCENT_HOVER, Color32::WHITE, 0.14)
                } else {
                    ACCENT_HOVER
                };
                let w = &mut ui.visuals_mut().widgets;
                for (state, color) in [
                    (&mut w.inactive, fill),
                    (&mut w.hovered, hover_fill),
                    (&mut w.active, ACCENT_ACTIVE),
                ] {
                    state.bg_fill = color;
                    state.weak_bg_fill = color;
                    state.bg_stroke.color = color;
                }
            }
        }
        interactive(ui, |ui| {
            ui.add_enabled(
                enabled,
                egui::Button::new(RichText::new(text).color(fg))
                    .min_size(vec2(width, ACTION_HEIGHT)),
            )
        })
    })
    .inner
}

/// Tombol sekunder; saat `accent` aktif, memakai latar tint amber untuk menonjolkan perubahan.
pub fn action_button(
    ui: &mut Ui,
    enabled: bool,
    accent: bool,
    min_width: f32,
    text: &str,
) -> Response {
    ui.scope(|ui| {
        let fg = if !enabled {
            MUTED
        } else if accent {
            let w = &mut ui.visuals_mut().widgets;
            for (state, fill, stroke) in [
                (&mut w.inactive, ACCENT_TINT, ACCENT_BORDER),
                (&mut w.hovered, ACCENT_TINT.gamma_multiply(1.8), ACCENT),
                (&mut w.active, ACCENT_TINT.gamma_multiply(2.4), ACCENT_HOVER),
            ] {
                state.bg_fill = fill;
                state.weak_bg_fill = fill;
                state.bg_stroke.color = stroke;
            }
            ACCENT_HOVER
        } else {
            TEXT
        };
        interactive(ui, |ui| {
            ui.add_enabled(
                enabled,
                egui::Button::new(RichText::new(text).color(fg))
                    .min_size(vec2(min_width, ACTION_HEIGHT)),
            )
        })
    })
    .inner
}

/// Tombol sekunder dengan ukuran yang sejajar dengan tombol utama.
pub fn secondary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    action_button(ui, enabled, false, 0.0, text)
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

        // Pastikan tombol dikte primer (amber) dan sekunder beranimasi hover tanpa menggeser layout.
        use egui::epaint::Shape;
        for (fill, idle, hovered) in [(Some(ACCENT), ACCENT, ACCENT_HOVER), (None, RAISED, BORDER)]
        {
            let ctx = egui::Context::default();
            install(&ctx);
            let mut t = 0.0;
            let mut run = |events| {
                t += 1.0 / 60.0;
                let mut rect = egui::Rect::NOTHING;
                let output = ctx.run_ui(
                    egui::RawInput {
                        time: Some(t),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        rect = dictation_button(ui, true, fill, 120.0, "Mulai dikte").rect;
                    },
                );
                let painted = output.shapes.iter().find_map(|s| match &s.shape {
                    Shape::Rect(r) if r.rect.width() >= 100.0 => Some(r.fill),
                    _ => None,
                });
                (rect, painted.expect("tombol harus menggambar persegi"))
            };
            let (rest_rect, rest_fill) = run(vec![]);
            assert_eq!(rest_fill, idle);
            run(vec![egui::Event::PointerMoved(rest_rect.center())]);
            let mut fills = Vec::new();
            for _ in 0..14 {
                let (r, c) = run(vec![]);
                assert_eq!(r, rest_rect);
                fills.push(c);
            }
            assert!(fills.iter().any(|&c| c != idle && c != hovered));
            assert_eq!(*fills.last().unwrap(), hovered);
        }
    }

    #[test]
    fn combo_item_hover_border_stays_inside_clip_rect_and_aligned() {
        use egui::epaint::Shape;

        let ctx = egui::Context::default();
        install(&ctx);
        let icons = Icons::load(&ctx);
        let mut selected = 0usize;
        let labels = ["White noise", "Pink noise", "Brown noise"];
        let mut time = 0.0;

        let mut run = |events: Vec<egui::Event>| {
            time += 1.0 / 60.0;
            let mut combo_rect = egui::Rect::NOTHING;
            let mut item_rects = Vec::new();
            let output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    combo_rect = combo(
                        ui,
                        egui::ComboBox::from_id_salt("test-combo")
                            .width(180.0)
                            .selected_text(labels[selected]),
                        &icons.chevron,
                        |ui| {
                            for (i, label) in labels.iter().enumerate() {
                                let r = interactive(ui, |ui| {
                                    ui.selectable_value(&mut selected, i, *label)
                                });
                                item_rects.push(r.rect);
                            }
                        },
                    )
                    .rect;
                },
            );
            (combo_rect, item_rects, output.shapes)
        };

        let (combo_rect, _, _) = run(vec![]);
        let click = |pos| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ]
        };
        run(click(combo_rect.center()));
        let (_, item_rects, _) = run(vec![]);
        assert_eq!(item_rects.len(), labels.len());

        for &idx in &[0usize, 1, 2] {
            let target = item_rects[idx];
            run(vec![egui::Event::PointerMoved(target.center())]);
            let mut last_shapes = Vec::new();
            for _ in 0..14 {
                last_shapes = run(vec![]).2;
            }
            let hovered_shape = last_shapes
                .iter()
                .find_map(|cs| match &cs.shape {
                    Shape::Rect(r)
                        if r.stroke.color == ACCENT_BORDER && r.rect.intersects(target) =>
                    {
                        Some((cs.clip_rect, r.rect))
                    }
                    _ => None,
                })
                .expect("item dropdown yang di-hover harus menggambar border ACCENT_BORDER");
            let (clip, painted) = hovered_shape;
            assert!(
                clip.contains_rect(painted),
                "item {idx}: border {painted:?} terpotong clip_rect {clip:?}"
            );
            assert!(
                (painted.left() - target.left()).abs() <= 0.01
                    && (painted.right() - target.right()).abs() <= 0.01
                    && (painted.top() - target.top()).abs() <= 0.01
                    && (painted.bottom() - target.bottom()).abs() <= 0.01,
                "item {idx}: border {painted:?} harus sejajar dengan rect item {target:?}"
            );
        }
    }
}
