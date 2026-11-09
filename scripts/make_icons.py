"""Builds every app icon from the logo artwork.

    python scripts/make_icons.py [path/to/artwork]

The artwork is the white cairn on a flat dark background. Its stones are cut out as an alpha
mask (`assets/icon/mark.png`, the source of truth once made), then drawn on a rounded dark tile
at each size the platforms need:

    assets/icon/icon.png        1024 px, full-bleed tile (Windows, Linux, the window icon)
    assets/icon/icon.ico        16-256 px, for the installer and the exe
    assets/icon/icon-mac.png    1024 px on Apple's grid (824 px tile, transparent margin)
    assets/icon/icon.icns       macOS app bundle
    assets/icon/icon-256.png    the window/taskbar icon and the logo in the app's UI
"""
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent / "assets" / "icon"
TILE = (0x3A, 0x3A, 0x3A)
# The mark fills this share of the tile's width (or height, whichever binds).
MARK_SHARE = 0.66
SS = 4  # supersampling for the tile's rounded corners


def cut_mark(artwork: Path) -> Image.Image:
    """The white stones as a white image with alpha, cropped to them, from the artwork."""
    lum = Image.open(artwork).convert("L")
    # Background ~58, stones ~250: map 95..215 to 0..255, which keeps anti-aliased edges and
    # drops the JPEG noise around them.
    alpha = lum.point(lambda v: max(0, min(255, round((v - 95) * 255 / 120))))
    alpha = alpha.crop(alpha.getbbox())
    mark = Image.new("RGBA", alpha.size, (255, 255, 255, 0))
    mark.putalpha(alpha)
    return mark


def rounded_tile(size: int, radius: float) -> Image.Image:
    big = Image.new("L", (size * SS, size * SS), 0)
    ImageDraw.Draw(big).rounded_rectangle((0, 0, size * SS - 1, size * SS - 1), radius=radius * SS, fill=255)
    mask = big.resize((size, size), Image.LANCZOS)
    tile = Image.new("RGBA", (size, size), TILE + (255,))
    tile.putalpha(mask)
    return tile


def icon(mark: Image.Image, size: int, tile_share: float = 1.0) -> Image.Image:
    """`mark` centred on a rounded tile `tile_share` of `size` wide, on a transparent square."""
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    t = round(size * tile_share)
    tile = rounded_tile(t, t * 0.225)
    # Small icons give the mark more of the tile, so the stones stay readable at 16 px.
    share = MARK_SHARE if t >= 64 else 0.8
    scale = share * t / max(mark.width, mark.height)
    m = mark.resize((max(1, round(mark.width * scale)), max(1, round(mark.height * scale))), Image.LANCZOS)
    # Optical centre: the cairn is heavier at the bottom, so sit it a touch high.
    tile.alpha_composite(m, ((t - m.width) // 2, (t - m.height) // 2 - round(t * 0.01)))
    canvas.alpha_composite(tile, ((size - t) // 2, (size - t) // 2))
    return canvas


def main() -> None:
    ROOT.mkdir(parents=True, exist_ok=True)
    mark_path = ROOT / "mark.png"
    if len(sys.argv) > 1:
        cut_mark(Path(sys.argv[1])).save(mark_path, optimize=True)
    mark = Image.open(mark_path).convert("RGBA")

    big = icon(mark, 1024)
    big.save(ROOT / "icon.png", optimize=True)
    # Each .ico size drawn on its own, so small sizes stay sharp.
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    frames = [icon(mark, s) for s in sizes]
    frames[-1].save(ROOT / "icon.ico", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
    mac = icon(mark, 1024, tile_share=824 / 1024)
    mac.save(ROOT / "icon-mac.png", optimize=True)
    mac.save(ROOT / "icon.icns")
    icon(mark, 256).save(ROOT / "icon-256.png", optimize=True)
    print("icons written to", ROOT)


if __name__ == "__main__":
    main()
