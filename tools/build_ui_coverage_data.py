"""Coverage-supervised screen pairs, including the same hinted glyph at 1x/2x.

    Hint once at the source size, scale the outline, then rasterize separately.
    This preserves shape instead of re-hinting a new high-DPI glyph.
"""

import argparse
from ctypes import byref
from functools import lru_cache
import hashlib
import json
from pathlib import Path
import random

import freetype
import numpy as np
from PIL import Image, ImageDraw

import build_upscaler_data as render
from distill_ui_upscaler import codec

SEEDS = {"train": 3100000, "valid": 3200000, "test": 3300000}
STRINGS = ["Il1!|", "0O8B", "rn m", "{}[]()", "aegop", "<>/=+", "1.25px",
           "const", "f(x)", "i:j;", "...", "!= ==", "+-*/", "2026", "WMW"]


@lru_cache(maxsize=8192)
def glyph_bitmap(path, px, character, scale, phase):
    face = freetype.Face(path)
    face.set_pixel_sizes(0, px)
    if not face.get_char_index(ord(character)):
        raise ValueError(f"Missing glyph {character!r} in {path}")
    face.load_char(character, freetype.FT_LOAD_DEFAULT | freetype.FT_LOAD_NO_BITMAP)
    advance = face.glyph.advance.x / 64
    glyph = face.glyph.get_glyph()
    matrix = freetype.Matrix(scale * 65536, 0, 0, scale * 65536)
    translation = freetype.Vector(int(phase * 64 * scale), 0)
    error = freetype.FT_Glyph_Transform(glyph._FT_Glyph, byref(matrix), byref(translation))
    if error:
        raise freetype.FT_Exception(error)
    bitmap = glyph.to_bitmap(freetype.FT_RENDER_MODE_NORMAL, freetype.Vector(0, 0), True)
    b = bitmap.bitmap
    array = np.array(b.buffer, dtype=np.uint8).reshape(b.rows, abs(b.pitch))[:, :b.width].copy()
    return array, bitmap.left, bitmap.top, advance


def text_mask(rows, path, scale, size):
    mask = np.zeros((size * scale, size * scale), np.uint8)
    for text, px, x, baseline, phase in rows:
        for character in text:
            a, left, top, advance = glyph_bitmap(str(path), px, character, scale, phase)
            xx, yy = round(x * scale) + left, round(baseline * scale) - top
            h, w = a.shape
            x0, y0, x1, y1 = max(xx, 0), max(yy, 0), min(xx+w, size*scale), min(yy+h, size*scale)
            if x1 > x0 and y1 > y0:
                region = mask[y0:y1, x0:x1]
                np.maximum(region, a[y0-yy:y1-yy, x0-xx:x1-xx], out=region)
            x += advance
    return mask


def example(seed, fonts, size=48, cjk=False):
    rng = random.Random(seed)
    path = render.font_path(rng.choice(fonts))
    rows = []
    y = 1
    while y < size - 8:
        px = rng.randint(8, 21)
        text = rng.choice(["中文界面", "字体测试", "大小文字", "一二三四"] if cjk else STRINGS)
        rows.append((text, px, rng.randint(0, 4), y + px, rng.choice([0., .25, .5, .75])))
        y += px + rng.randint(3, 6)
    low_mask, high_mask = [text_mask(rows, path, scale, size) for scale in [1, 2]]
    if not cjk and seed % 5 == 0:
        im = Image.new("L", (size*4, size*4))
        draw = ImageDraw.Draw(im)
        for _ in range(4):
            x, y = rng.randrange(size-12), rng.randrange(size-12)
            box = (x*4, y*4, (x+rng.randint(5, 16))*4, (y+rng.randint(5, 16))*4)
            if rng.random() < .5:
                draw.rectangle(box, outline=255, width=rng.randint(1, 3)*4)
            else:
                draw.ellipse(box, outline=255, width=rng.randint(1, 3)*4)
        low_mask = np.asarray(im.resize((size, size), Image.Resampling.BOX)).copy()
        high_mask = np.asarray(im.resize((size*2, size*2), Image.Resampling.BOX)).copy()
    dark = rng.randint(10, 60)
    light = rng.randint(190, 245)
    bg, fg = np.full(3, light, dtype=np.float32), np.full(3, dark, dtype=np.float32)
    if rng.random() < .35:
        fg = np.array([rng.randint(5, 140) for _ in range(3)], np.float32)
        bg = np.array([rng.randint(190, 250) for _ in range(3)], np.float32)
    if rng.random() < .5:
        fg, bg = 255-fg, 255-bg
    if fg.mean() < bg.mean():
        fg, bg = bg, fg
        low_mask, high_mask = 255-low_mask, 255-high_mask
    compose = lambda a: np.rint(a[..., None]/255*fg + (1-a[..., None]/255)*bg).astype(np.uint8)
    hi = compose(high_mask)
    native = compose(low_mask)
    down = np.rint(hi.reshape(size, 2, size, 2, 3).mean((1, 3))).astype(np.uint8)
    return native, down, hi, high_mask[..., None], np.concatenate([fg, bg]).astype(np.uint8)


def generate(split, count, size=48, cjk=False):
    fonts = ["noto/NotoSansCJK-Regular.ttc"] if cjk else (
        render.TRAIN_FONTS if split == "train" else render.HELDOUT_FONTS)
    start = SEEDS[split] + (10000 if cjk else 0)
    arrays = list(zip(*(example(start+i, fonts, size, cjk) for i in range(count))))
    native, down, hi, alpha, colors = [np.stack(a) for a in arrays]
    return {"native": native, "down": down, "hi": hi, "alpha": alpha,
            "colors": colors, "native_vp8": codec(native), "down_vp8": codec(down)}


def prepare(out, count):
    out.mkdir(parents=True, exist_ok=True)
    for split, n in [("train", count), ("valid", 128)]:
        bank = generate(split, n)
        np.savez(out/f'{split}.npz', **bank)
        print(f"Prepared {split}: {n} paired masks/outlined glyphs", flush=True)
    metadata = {"seeds": SEEDS, "train_count": count, "valid_count": 128,
                "freetype": freetype.version(), "codec": "VP8 q28, yuv420p, g1",
                "native_target": "source-size hinting preserved by scaling the loaded outline",
                "train_fonts": render.TRAIN_FONTS, "heldout_fonts": render.HELDOUT_FONTS,
                "fonts_sha256": {f: hashlib.sha256(render.font_path(f).read_bytes()).hexdigest()
                                 for f in render.TRAIN_FONTS+render.HELDOUT_FONTS},
                "hinted_native_weight": .5, "compressed_weight": .5}
    (out/'data.json').write_text(json.dumps(metadata, indent=2)+'\n')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--out', type=Path, default=Path('ignore/upscaler-coverage-data'))
    p.add_argument('--count', type=int, default=4096)
    args = p.parse_args()
    prepare(args.out, args.count)
