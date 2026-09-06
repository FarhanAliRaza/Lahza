#!/usr/bin/env python3
"""Generate the Lahza app icons from the brand mark.

The mark is the Lahza logo glyph: four corner brackets framing a red record
dot, taken from the brand SVG (packaging/icons/lahza.svg). Everything is
defined here in unit coordinates so the SVG and the PNGs cannot drift apart.

    python3 packaging/icons/generate-icons.py

Writes packaging/icons/lahza.svg and Lahza.png, the 512x512 icon used by the
Snap Store, the desktop entry, and the README.
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
ICONS = ROOT / "packaging" / "icons"

INK = (27, 27, 26, 255)  # bracket charcoal
DOT = (208, 55, 52, 255)  # record red
BG = (251, 251, 251, 255)
BORDER = (228, 227, 224, 255)

SIZE = 512

# Icon layout, as fractions of the canvas.
CORNER_RADIUS = 0.225
MARGIN = 0.02  # transparent gutter around the rounded square
BORDER_WIDTH = 0.004
MARK_SIZE = 0.58  # mark box, relative to the canvas

# Mark geometry, as fractions of the mark box.
ARM = 0.185  # bracket thickness
REACH = 0.49  # how far each bracket arm runs (0.5 would close the frame)
DOT_RADIUS = 0.224

SS = 8  # supersampling factor for the PNG rasterizer


def bracket(flip_x: bool, flip_y: bool) -> list[tuple[float, float]]:
    """One corner bracket, in mark-box unit coordinates."""
    points = [
        (0.0, 0.0),
        (0.0, REACH),
        (ARM, REACH),
        (ARM, ARM),
        (REACH, ARM),
        (REACH, 0.0),
    ]
    return [(1.0 - x if flip_x else x, 1.0 - y if flip_y else y) for x, y in points]


BRACKETS = [bracket(fx, fy) for fx in (False, True) for fy in (False, True)]


def render_png(size: int) -> Image.Image:
    canvas = size * SS
    image = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    inset = MARGIN * canvas
    draw.rounded_rectangle(
        (inset, inset, canvas - 1 - inset, canvas - 1 - inset),
        radius=CORNER_RADIUS * canvas,
        fill=BG,
        outline=BORDER,
        width=max(1, round(BORDER_WIDTH * canvas)),
    )

    box = MARK_SIZE * canvas
    origin = (canvas - box) / 2
    for points in BRACKETS:
        draw.polygon([(origin + x * box, origin + y * box) for x, y in points], fill=INK)

    center = canvas / 2
    radius = DOT_RADIUS * box
    draw.ellipse(
        (center - radius, center - radius, center + radius, center + radius), fill=DOT
    )

    return image.resize((size, size), Image.LANCZOS)


def render_svg() -> str:
    box = MARK_SIZE
    origin = (1 - box) / 2

    def polygon(points: list[tuple[float, float]]) -> str:
        coords = " ".join(
            f"{(origin + x * box) * 512:.2f},{(origin + y * box) * 512:.2f}"
            for x, y in points
        )
        return f'  <polygon points="{coords}" fill="rgb(27,27,26)"/>'

    inset = MARGIN * 512
    stroke = BORDER_WIDTH * 512
    shapes = "\n".join(polygon(points) for points in BRACKETS)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">
  <title>Lahza</title>
  <rect x="{inset + stroke / 2:.2f}" y="{inset + stroke / 2:.2f}" \
width="{512 - 2 * inset - stroke:.2f}" height="{512 - 2 * inset - stroke:.2f}" \
rx="{CORNER_RADIUS * 512:.2f}" fill="rgb(251,251,251)" \
stroke="rgb(228,227,224)" stroke-width="{stroke:.2f}"/>
{shapes}
  <circle cx="256" cy="256" r="{DOT_RADIUS * box * 512:.2f}" fill="rgb(208,55,52)"/>
</svg>
"""


def main() -> None:
    ICONS.mkdir(parents=True, exist_ok=True)
    (ICONS / "lahza.svg").write_text(render_svg())

    render_png(SIZE).save(ROOT / "Lahza.png", optimize=True)


if __name__ == "__main__":
    main()
