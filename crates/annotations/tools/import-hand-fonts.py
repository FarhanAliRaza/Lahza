"""Import the four Informal faces from a reference assets/fonts directory.

Requires fonttools[woff]. Converts WOFF2 to TTF for native rendering and maps
weight metadata to the normal/bold roles used by the reference's CSS aliases.
Glyph outlines, metrics, and shaping tables remain unchanged.
"""
import argparse
from pathlib import Path

from fontTools.ttLib import TTFont

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("source", type=Path)
args = parser.parse_args()
destination = Path(__file__).resolve().parents[1] / "assets" / "fonts"

for source_style, style, weight in [
    ("Regular", "Regular", 400),
    ("Bold", "Bold", 700),
    ("Regular_Italic", "Italic", 400),
    ("Bold_Italic", "BoldItalic", 700),
]:
    source = args.source / f"Shantell_Sans-Informal_{source_style}.woff2"
    font = TTFont(source, recalcTimestamp=False)
    font.flavor = None
    font["OS/2"].usWeightClass = weight
    # The upright face has a small designed lean, but native font matching
    # treats any nonzero post angle as Italic. CSS labels it as normal.
    if "Italic" not in style:
        font["post"].italicAngle = 0
    output = destination / f"ShantellSansInformal-{style}.ttf"
    font.save(output)
    restored = TTFont(output)
    assert restored["hmtx"].metrics == font["hmtx"].metrics
    for glyph in font.getGlyphOrder():
        assert restored["glyf"][glyph].getCoordinates(restored["glyf"]) == font["glyf"][glyph].getCoordinates(font["glyf"])
    print(output.name)
