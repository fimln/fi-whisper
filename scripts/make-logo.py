"""Membuat assets/logo.png (512), logo.ico (16-256), dan logo-128.rgba dari palet fi-whisper.

Gaya mengikuti logo fi-shortener: kotak bulat dengan gradien diagonal dan teks "fi" gelap.
Jalankan: python scripts/make-logo.py (butuh Pillow).
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets"
SIZE = 512
SS = 4  # supersampling untuk tepi halus
TOP_LEFT = (0xFB, 0xBF, 0x24)  # Accent hover
BOTTOM_RIGHT = (0xD9, 0x77, 0x06)  # Amber gelap, ujung gradien
TEXT = (0x0F, 0x0E, 0x0C)  # Text on accent


def render() -> Image.Image:
    s = SIZE * SS
    margin = 10 * SS
    radius = 104 * SS
    # Gradien diagonal: t = (x + y) / (2s), kiri-atas terang ke kanan-bawah gelap.
    line = Image.linear_gradient("L").resize((s, s))  # vertikal: 0 di atas, 255 di bawah
    diag = Image.blend(line, line.rotate(90), 0.5)
    grad = Image.composite(
        Image.new("RGB", (s, s), BOTTOM_RIGHT), Image.new("RGB", (s, s), TOP_LEFT), diag
    )

    mask = Image.new("L", (s, s), 0)
    box = (margin, margin, s - margin, s - margin)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius, fill=255)
    logo = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    logo.paste(grad, (0, 0), mask)

    draw = ImageDraw.Draw(logo)
    # Garis tepi tipis lebih terang agar terbaca di taskbar gelap.
    draw.rounded_rectangle(box, radius, outline=(255, 236, 179, 70), width=3 * SS)

    font = ImageFont.truetype(str(ROOT / "fonts" / "Inter-SemiBold.ttf"), 300 * SS)
    left, top, right, bottom = draw.textbbox((0, 0), "fi", font=font)
    x = (s - (right - left)) / 2 - left
    y = (s - (bottom - top)) / 2 - top
    draw.text((x, y), "fi", font=font, fill=TEXT)
    return logo.resize((SIZE, SIZE), Image.Resampling.LANCZOS)


def render_icons() -> None:
    """Octicons (MIT, assets/icons) dirender jadi RGBA 64×64 putih; diwarnai saat runtime."""
    import pymupdf  # merender SVG

    for name in ("mark-github-24", "home-fill-24", "dash-24", "x-24"):
        svg = (OUT / "icons" / f"{name}.svg").read_text().replace("<path ", '<path fill="#fff" ')
        page = pymupdf.open(stream=svg.encode(), filetype="svg")[0]
        pix = page.get_pixmap(matrix=pymupdf.Matrix(64 / 24, 64 / 24), alpha=True)
        assert (pix.width, pix.height, pix.n) == (64, 64, 4)
        (OUT / "icons" / f"{name}-64.rgba").write_bytes(pix.samples)


def main() -> None:
    OUT.mkdir(exist_ok=True)
    render_icons()
    logo = render()
    logo.save(OUT / "logo.png")
    logo.save(
        OUT / "logo.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    (OUT / "logo-128.rgba").write_bytes(
        logo.resize((128, 128), Image.Resampling.LANCZOS).tobytes()
    )


if __name__ == "__main__":
    main()
