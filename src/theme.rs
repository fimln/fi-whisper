use rune::{Color, El, TextStyle, col, el, text};
pub fn c(hex: u32) -> Color {
    rune::color::hex(&format!("#{hex:06x}")).unwrap()
}
pub fn bg() -> Color {
    c(0x090807)
}
pub fn surface() -> Color {
    c(0x12100e)
}
pub fn border() -> Color {
    c(0x28241f)
}
pub fn accent() -> Color {
    c(0xf59e0b)
}
pub fn fg() -> Color {
    c(0xf5f5f4)
}
pub fn secondary() -> Color {
    c(0xa8a29e)
}
pub fn muted() -> Color {
    c(0x57534e)
}
pub fn label(s: impl Into<String>) -> El {
    text(s)
        .font(TextStyle::new(12.0, 500.0))
        .color(secondary())
        .wrap()
}
pub fn title(s: impl Into<String>) -> El {
    text(s).font(TextStyle::new(16.0, 600.0)).color(fg())
}
pub fn card(key: &str) -> El {
    col()
        .key(key)
        .grow(1.0)
        .min_w(0.0)
        .gap(9.0)
        .padding(16.0)
        .radius(12.0)
        .fill(surface())
        .border_gradient(1.0, c(0x53402a), border())
        .shadow(6.0, 24.0, c(0).alpha(0.35))
}
pub fn button(key: &str, label: &str, primary: bool, enabled: bool) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .disabled(!enabled)
        .px(14.0)
        .h(34.0)
        .radius(6.0)
        .center()
        .fill(if primary { accent() } else { c(0x1c1916) })
        .border(1.0, if primary { c(0xfbbf24) } else { border() })
        .hover(c(0xfbbf24).alpha(0.12))
        .press(c(0).alpha(0.13))
        .press_scale(0.97)
        .motion(rune::tok::motion::SNAPPY)
        .layer_opacity(if enabled { 1.0 } else { 0.45 })
        .child(
            text(label)
                .font(TextStyle::new(14.0, 500.0))
                .truncate()
                .color(if primary { bg() } else { fg() }),
        )
}
pub fn kit() -> rune_kit::Kit {
    let mut k = rune_kit::Kit::neutral(true);
    k.c.bg_base = bg();
    k.c.text_base = fg();
    k.c.text_muted = secondary();
    k.c.text_accent = accent();
    k.c.border_base = border();
    k.c.border_focus = accent();
    k.c.layer_02 = surface();
    k.c.layer_03 = c(0x1c1916);
    k
}
pub fn logo() -> rune::Pixels {
    let p = rune::Pixels::new();
    p.set(128, 128, include_bytes!("../assets/logo-128.rgba").to_vec());
    p
}

pub const LOGO_SIZE: usize = 128;
pub const LOGO_RGBA: &[u8] = include_bytes!("../assets/logo-128.rgba");

pub struct Icons {
    pub home: rune::Pixels,
    pub github: rune::Pixels,
    pub minimize: rune::Pixels,
    pub close: rune::Pixels,
}
impl Icons {
    pub fn new() -> Self {
        fn pixels(bytes: &[u8]) -> rune::Pixels {
            let p = rune::Pixels::new();
            p.set(64, 64, bytes.to_vec());
            p
        }
        Self {
            home: pixels(include_bytes!("../assets/icons/home-fill-24-64.rgba")),
            github: pixels(include_bytes!("../assets/icons/mark-github-24-64.rgba")),
            minimize: pixels(include_bytes!("../assets/icons/dash-24-64.rgba")),
            close: pixels(include_bytes!("../assets/icons/x-24-64.rgba")),
        }
    }
}
pub fn icon_button(key: &str, name: &str, pixels: &rune::Pixels) -> El {
    el().key(key)
        .role("button")
        .label(name)
        .help(name)
        .focusable(true)
        .size(28.0, 28.0)
        .center()
        .radius(6.0)
        .fill(c(0x1c1916))
        .hover(accent().alpha(0.12))
        .press_scale(0.96)
        .child(el().size(14.0, 14.0).pixels(pixels))
}
