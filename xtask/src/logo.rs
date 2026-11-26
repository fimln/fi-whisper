use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use image::imageops::FilterType;
use image::{ImageFormat, RgbaImage};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, fontdb};

const SIZE: u32 = 512;
const SS: u32 = 4; // supersampling untuk tepi halus
const ICO_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256];
const OCTICONS: &[&str] = &["mark-github-24", "home-fill-24", "dash-24", "x-24"];

pub fn run(root: &Path) -> Result<(), String> {
    let out = root.join("assets");
    fs::create_dir_all(&out).map_err(|e| format!("gagal membuat {}: {e}", out.display()))?;

    render_icons(&out)?;
    let logo = render_logo(root)?;

    logo.save_with_format(out.join("logo.png"), ImageFormat::Png)
        .map_err(|e| format!("gagal menyimpan logo.png: {e}"))?;

    let ico = build_ico(&logo, ICO_SIZES)?;
    fs::write(out.join("logo.ico"), ico).map_err(|e| format!("gagal menyimpan logo.ico: {e}"))?;

    let logo_128 = image::imageops::resize(&logo, 128, 128, FilterType::Lanczos3);
    fs::write(out.join("logo-128.rgba"), logo_128.as_raw())
        .map_err(|e| format!("gagal menyimpan logo-128.rgba: {e}"))?;

    println!("Aset logo dan ikon selesai dibuat di {}", out.display());
    Ok(())
}

/// Octicons (MIT, assets/icons) dirender jadi RGBA 64x64 putih; diwarnai saat runtime.
fn render_icons(assets: &Path) -> Result<(), String> {
    let icons_dir = assets.join("icons");
    let opts = usvg::Options::default();
    for &name in OCTICONS {
        let svg_path = icons_dir.join(format!("{name}.svg"));
        let svg = fs::read_to_string(&svg_path)
            .map_err(|e| format!("gagal membaca {}: {e}", svg_path.display()))?
            .replace("<path ", "<path fill=\"#fff\" ");
        let tree = usvg::Tree::from_str(&svg, &opts)
            .map_err(|e| format!("SVG {name} tidak valid: {e}"))?;
        let mut pixmap =
            Pixmap::new(64, 64).ok_or_else(|| "gagal mengalokasikan pixmap 64x64".to_string())?;
        let scale = 64.0 / 24.0;
        resvg::render(
            &tree,
            Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let rgba = unpremultiply(pixmap.data());
        let out_path = icons_dir.join(format!("{name}-64.rgba"));
        fs::write(&out_path, rgba)
            .map_err(|e| format!("gagal menulis {}: {e}", out_path.display()))?;
    }
    Ok(())
}

/// Membuat logo 512x512: kotak bulat gradien diagonal amber dengan teks "fi" gelap.
fn render_logo(root: &Path) -> Result<RgbaImage, String> {
    let s = (SIZE * SS) as f32;
    let margin = (10 * SS) as f32;
    let radius = (104 * SS) as f32;
    let stroke = (3 * SS) as f32;
    let font_size = (300 * SS) as f32;

    let font_path = root.join("fonts").join("Inter-SemiBold.ttf");
    let font_data =
        fs::read(&font_path).map_err(|e| format!("gagal membaca {}: {e}", font_path.display()))?;

    let mut db = fontdb::Database::new();
    db.load_font_data(font_data);
    let face = db
        .faces()
        .next()
        .ok_or_else(|| "font Inter-SemiBold.ttf tidak memuat face".to_string())?;
    let family = face
        .families
        .first()
        .map(|(name, _)| name.clone())
        .ok_or_else(|| "nama family font tidak ditemukan".to_string())?;
    let weight = face.weight.0;

    let opts = usvg::Options {
        fontdb: Arc::new(db),
        ..usvg::Options::default()
    };

    // Ukur bounding box visual "fi" agar posisinya terpusat secara optik seperti Pillow textbbox.
    let probe_svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}">
             <text x="0" y="0" font-family="{family}" font-weight="{weight}" font-size="{font_size}">fi</text>
           </svg>"#
    );
    let probe_tree = usvg::Tree::from_str(&probe_svg, &opts)
        .map_err(|e| format!("gagal mengukur teks logo: {e}"))?;
    let bbox = probe_tree
        .root()
        .bounding_box()
        .ok_or_else(|| "bounding box teks logo kosong".to_string())?;
    let tx = (s - bbox.width()) / 2.0 - bbox.left();
    let ty = (s - bbox.height()) / 2.0 - bbox.top();

    let box_size = s - 2.0 * margin;
    let half_stroke = stroke / 2.0;
    let stroke_xy = margin + half_stroke;
    let stroke_size = box_size - stroke;
    let stroke_radius = radius - half_stroke;
    let stroke_opacity = 70.0 / 255.0;

    let logo_svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
             <defs>
               <linearGradient id="g" x1="0" y1="0" x2="{s}" y2="{s}" gradientUnits="userSpaceOnUse">
                 <stop offset="0%" stop-color="#FBBF24"/>
                 <stop offset="100%" stop-color="#D97706"/>
               </linearGradient>
             </defs>
             <rect x="{margin}" y="{margin}" width="{box_size}" height="{box_size}" rx="{radius}" ry="{radius}" fill="url(#g)"/>
             <rect x="{stroke_xy}" y="{stroke_xy}" width="{stroke_size}" height="{stroke_size}" rx="{stroke_radius}" ry="{stroke_radius}" fill="none" stroke="#FFECB3" stroke-opacity="{stroke_opacity:.5}" stroke-width="{stroke}"/>
             <text x="{tx:.3}" y="{ty:.3}" font-family="{family}" font-weight="{weight}" font-size="{font_size}" fill="#0F0E0C">fi</text>
           </svg>"##
    );

    let tree =
        usvg::Tree::from_str(&logo_svg, &opts).map_err(|e| format!("SVG logo tidak valid: {e}"))?;
    let dim = SIZE * SS;
    let mut pixmap =
        Pixmap::new(dim, dim).ok_or_else(|| "gagal mengalokasikan pixmap logo".to_string())?;
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());

    let rgba = unpremultiply(pixmap.data());
    let hi_res = RgbaImage::from_raw(dim, dim, rgba)
        .ok_or_else(|| "ukuran buffer RGBA logo tidak cocok".to_string())?;
    Ok(image::imageops::resize(
        &hi_res,
        SIZE,
        SIZE,
        FilterType::Lanczos3,
    ))
}

fn unpremultiply(premul: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(premul.len());
    for px in premul.as_chunks::<4>().0 {
        let a = px[3];
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            out.extend_from_slice(px);
        } else {
            let a_u32 = u32::from(a);
            let un = |c: u8| -> u8 { ((u32::from(c) * 255 + a_u32 / 2) / a_u32).min(255) as u8 };
            out.extend_from_slice(&[un(px[0]), un(px[1]), un(px[2]), a]);
        }
    }
    out
}

/// Menulis kontainer ICO multi-ukuran berisi frame PNG (didukung Windows Vista+).
fn build_ico(source: &RgbaImage, sizes: &[u32]) -> Result<Vec<u8>, String> {
    let mut frames = Vec::with_capacity(sizes.len());
    for &size in sizes {
        let resized = image::imageops::resize(source, size, size, FilterType::Lanczos3);
        let mut png_bytes = Vec::new();
        resized
            .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
            .map_err(|e| format!("gagal encode frame ICO {size}px: {e}"))?;
        frames.push((size, png_bytes));
    }

    let count = u16::try_from(frames.len()).map_err(|_| "terlalu banyak frame ICO".to_string())?;
    let mut out = Vec::new();
    // Header ICONDIR: reserved (0), type (1 = ICO), count.
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());

    let mut offset = 6u32 + 16u32 * u32::from(count);
    for (size, data) in &frames {
        let dim_byte = if *size >= 256 { 0u8 } else { *size as u8 };
        let byte_len =
            u32::try_from(data.len()).map_err(|_| "frame ICO terlalu besar".to_string())?;
        out.push(dim_byte); // width
        out.push(dim_byte); // height
        out.push(0); // color palette
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&byte_len.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += byte_len;
    }
    for (_, data) in &frames {
        out.extend_from_slice(data);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpremultiply_restores_straight_alpha() {
        assert_eq!(unpremultiply(&[128, 64, 0, 128]), vec![255, 128, 0, 128]);
        assert_eq!(unpremultiply(&[0, 0, 0, 0]), vec![0, 0, 0, 0]);
        assert_eq!(unpremultiply(&[10, 20, 30, 255]), vec![10, 20, 30, 255]);
    }

    #[test]
    fn ico_header_and_entries_are_valid() {
        let img = RgbaImage::from_pixel(32, 32, image::Rgba([0xF5, 0x9E, 0x0B, 0xFF]));
        let bytes = build_ico(&img, &[16, 256]).unwrap();
        assert_eq!(&bytes[0..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(bytes[6], 16);
        assert_eq!(bytes[22], 0); // 256 disimpan sebagai 0 di ICO
        let first_offset =
            u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]) as usize;
        assert_eq!(first_offset, 6 + 32);
        assert_eq!(&bytes[first_offset..first_offset + 8], b"\x89PNG\r\n\x1a\n");
    }
}
