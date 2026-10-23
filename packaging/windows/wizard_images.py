"""Renders the Inno Setup wizard bitmaps from assets/icon.png.

    python packaging/windows/wizard_images.py <output folder>

Writes wizard-100.bmp / wizard-200.bmp (the side panel on the first and last
pages) and wizard-small-100.bmp / wizard-small-200.bmp (top-right corner).
Requires Pillow. Run by the release workflow; the bitmaps are not committed.
"""
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
TOP = (139, 92, 246)  # violet, as in the logo
BOTTOM = (6, 182, 212)  # cyan


def panel(width: int, height: int, icon: Image.Image) -> Image.Image:
    """Vertical violet-to-cyan gradient with the logo in the upper third."""
    img = Image.new("RGB", (width, height))
    px = img.load()
    for y in range(height):
        t = y / (height - 1)
        row = tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM))
        for x in range(width):
            px[x, y] = row
    size = round(width * 0.62)
    logo = icon.resize((size, size), Image.LANCZOS)
    img.paste(logo, ((width - size) // 2, round(height * 0.22)), logo)
    return img


def small(size: int, icon: Image.Image) -> Image.Image:
    # The logo on white, which is what the wizard's header shows behind it.
    img = Image.new("RGB", (size, size), (255, 255, 255))
    logo = icon.resize((size, size), Image.LANCZOS)
    img.paste(logo, (0, 0), logo)
    return img


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    out.mkdir(parents=True, exist_ok=True)
    icon = Image.open(ROOT / "assets" / "icon.png").convert("RGBA")
    panel(164, 314, icon).save(out / "wizard-100.bmp")
    panel(328, 628, icon).save(out / "wizard-200.bmp")
    small(55, icon).save(out / "wizard-small-100.bmp")
    small(110, icon).save(out / "wizard-small-200.bmp")


if __name__ == "__main__":
    main()
