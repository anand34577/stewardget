"""Renders the StewardGet logo and every icon derived from it.

The mark is a violet-to-cyan squircle carrying a white download arrow above a
wave: fetching, and the stream of parallel connections that makes it fast.

Requires Pillow (`pip install pillow`); maintainers only, the outputs are
committed:

    python tools/gen_icons.py

Writes
    assets/icon.png                      512 px, README and stores
    assets/icon.ico                      Windows executable icon
    crates/stewardget-gui/assets/icon-128.rgba raw RGBA, window icon and sidebar logo
    extension/icons/icon{16,32,48,128}.png
"""
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
S = 1024  # master canvas
TOP_LEFT = (139, 92, 246)  # violet
BOTTOM_RIGHT = (6, 182, 212)  # cyan
# The glyph, in master-canvas units (keep in step with assets/logo.svg).
ARROW_W, ARROW_TOP, ARROW_TIP, ARROW_ARMS = 110, 222, (512, 630), ((352, 470), (672, 470))
WAVE_W, WAVE_X, WAVE_Y, WAVE_LIFT, WAVE_HALF = 74, 336, 786, 30, 352 / 3


def squircle_mask(size: int, margin: int, power: float = 5.0) -> Image.Image:
    """Superellipse mask, 4x supersampled for smooth edges."""
    big = size * 4
    m = margin * 4
    r = (big - 2 * m) / 2
    c = big / 2
    mask = Image.new("L", (big, big), 0)
    px = mask.load()
    for y in range(big):
        ny = abs((y + 0.5 - c) / r)
        if ny > 1:
            continue
        # |x|^p + |y|^p <= 1  ->  |x| <= (1 - |y|^p)^(1/p)
        half = (1 - ny**power) ** (1 / power) * r
        x0, x1 = int(c - half), int(c + half)
        for x in range(max(x0, 0), min(x1, big)):
            px[x, y] = 255
    return mask.resize((size, size), Image.LANCZOS)


def gradient(size: int) -> Image.Image:
    img = Image.new("RGB", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            t = (x + y) / (2 * (size - 1))
            px[x, y] = tuple(round(a + (b - a) * t) for a, b in zip(TOP_LEFT, BOTTOM_RIGHT))
    return img


def mark(size: int) -> Image.Image:
    """The white glyph as an alpha mask (4x supersampled)."""
    k = size * 4 / 1024
    big = int(1024 * k)
    m = Image.new("L", (big, big), 0)
    d = ImageDraw.Draw(m)
    w = int(ARROW_W * k)

    def dot(x: float, y: float, width: int) -> None:
        d.ellipse([x * k - width / 2, y * k - width / 2, x * k + width / 2, y * k + width / 2], fill=255)

    # The arrow: a shaft and two arms meeting at the point, all with round ends.
    tip = ARROW_TIP
    for a in ((512, ARROW_TOP), *ARROW_ARMS):
        d.line([(a[0] * k, a[1] * k), (tip[0] * k, tip[1] * k)], fill=255, width=w)
        dot(*a, w)
    dot(*tip, w)
    # The wave under it: three quadratic half waves, as in assets/logo.svg.
    # Stamped as dots, since thick polylines show seams at their joints.
    wave_w = int(WAVE_W * k)
    for i, lift in enumerate((-1, 1, -1)):
        for step in range(257):
            t = step / 256
            # A quadratic curve's control point sits at twice the peak height.
            dot(WAVE_X + WAVE_HALF * (i + t), WAVE_Y + lift * 2 * WAVE_LIFT * t * (1 - t), wave_w)
    return m.resize((size, size), Image.LANCZOS)


def render(size: int, margin_ratio: float = 0.04) -> Image.Image:
    margin = round(size * margin_ratio)
    base = gradient(size).convert("RGBA")
    # Soft light from the top left.
    glow = Image.new("L", (size, size), 0)
    ImageDraw.Draw(glow).ellipse([-size * 0.35, -size * 0.45, size * 0.75, size * 0.55], fill=40)
    glow = glow.filter(ImageFilter.GaussianBlur(size * 0.12))
    base = Image.composite(Image.new("RGBA", (size, size), (255, 255, 255, 255)), base, glow)

    glyph = mark(size)
    shadow = glyph.filter(ImageFilter.GaussianBlur(max(size * 0.02, 0.6)))
    shadow = ImageChops.offset(shadow, 0, max(round(size * 0.012), 1)).point(lambda v: v * 0.35)
    base = Image.composite(Image.new("RGBA", (size, size), (40, 20, 90, 255)), base, shadow)
    base = Image.composite(Image.new("RGBA", (size, size), (255, 255, 255, 255)), base, glyph)

    base.putalpha(squircle_mask(size, margin))
    return base


def main() -> None:
    master = render(S)
    (ROOT / "assets").mkdir(exist_ok=True)
    master.resize((512, 512), Image.LANCZOS).save(ROOT / "assets" / "icon.png", optimize=True)
    master.save(
        ROOT / "assets" / "icon.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    for px in (16, 32, 48, 128):
        # Small sizes are rendered directly so strokes stay crisp.
        img = render(px * 4).resize((px, px), Image.LANCZOS)
        img.save(ROOT / "extension" / "icons" / f"icon{px}.png", optimize=True)
    gui = ROOT / "crates" / "stewardget-gui" / "assets"
    gui.mkdir(parents=True, exist_ok=True)
    (gui / "icon-128.rgba").write_bytes(master.resize((128, 128), Image.LANCZOS).tobytes())
    print("icons written")


if __name__ == "__main__":
    main()
