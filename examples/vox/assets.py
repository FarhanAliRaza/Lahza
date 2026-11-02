"""Generated art for the cancer short (all original, made here):

  paper.png          warm paper with fibres, for the Vox collage look
  slide_healthy.png  an H&E-stained tissue slide seen through a microscope:
                     small, even nuclei in orderly glands
  slide_tumor.png    the same stain, cancerous: big, dark, crowded,
                     irregular nuclei with no order
  tape.png           a strip of masking tape, to "stick" cut-outs

    python3 assets.py OUT_DIR
"""
import math
import os
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

rng = np.random.default_rng(11)


def blur(a, r):
    return np.asarray(Image.fromarray(np.uint8(np.clip(a, 0, 255))).filter(ImageFilter.GaussianBlur(r)), dtype=np.float32)


def noise(w, h, scale, octaves=4):
    out = np.zeros((h, w), np.float32)
    amp, total = 1.0, 0.0
    for o in range(octaves):
        s = max(1, int(scale / 2 ** o))
        small = rng.random((h // s + 2, w // s + 2)).astype(np.float32) * 255
        big = np.asarray(Image.fromarray(np.uint8(small)).resize((w + 2 * s, h + 2 * s), Image.BICUBIC), np.float32)[:h, :w]
        out += big * amp
        total += amp
        amp *= 0.5
    return out / total / 255.0


def paper(w, h):
    base = np.array([244, 240, 231], np.float32)
    n = noise(w, h, 220) - 0.5
    fine = rng.normal(0, 1, (h, w)).astype(np.float32)
    img = np.ones((h, w, 3), np.float32) * base
    img += n[..., None] * 16
    img += fine[..., None] * 2.2
    # Fibres: faint short strokes.
    layer = Image.new("L", (w, h), 0)
    d = ImageDraw.Draw(layer)
    for _ in range(1400):
        x, y = rng.uniform(0, w), rng.uniform(0, h)
        a = rng.uniform(0, math.pi)
        length = rng.uniform(6, 26)
        d.line([(x, y), (x + math.cos(a) * length, y + math.sin(a) * length)], fill=int(rng.uniform(20, 60)), width=1)
    fib = np.asarray(layer.filter(ImageFilter.GaussianBlur(0.6)), np.float32) / 255.0
    img -= fib[..., None] * 14
    # Soft vignette.
    yy, xx = np.mgrid[0:h, 0:w]
    r = np.hypot((xx - w / 2) / (w / 2), (yy - h / 2) / (h / 2))
    img *= (1 - 0.07 * np.clip(r - 0.6, 0, 1))[..., None]
    return Image.fromarray(np.uint8(np.clip(img, 0, 255)))


def slide(size, tumor):
    """H&E: eosin pink stroma, haematoxylin purple nuclei."""
    w = h = size
    pink = np.array([238, 178, 204], np.float32)
    img = np.ones((h, w, 3), np.float32) * pink
    stroma = noise(w, h, 60) - 0.5
    img += stroma[..., None] * np.array([30, 55, 35], np.float32)
    # Fibre streaks of the connective tissue.
    streak = blur(rng.random((h, w)) * 255, 1.5) / 255.0 - 0.5
    img += streak[..., None] * np.array([18, 40, 22], np.float32)
    nuclei = Image.new("L", (w, h), 0)
    d = ImageDraw.Draw(nuclei)
    cyto = Image.new("L", (w, h), 0)
    dc = ImageDraw.Draw(cyto)
    band = Image.new("L", (w, h), 0)
    db = ImageDraw.Draw(band)
    if not tumor:
        # Orderly glands: separate, slightly irregular rings of cells
        # (a band of cytoplasm, nuclei at its base) around pale lumens.
        glands = []
        for _ in range(400):
            cx, cy, R = rng.uniform(40, w - 40), rng.uniform(40, h - 40), rng.uniform(55, 85)
            if all(math.hypot(cx - gx, cy - gy) > R + gr + 26 for gx, gy, gr in glands):
                glands.append((cx, cy, R))
            if len(glands) >= 9:
                break
        for cx, cy, R in glands:
            sx, sy, rot = rng.uniform(0.85, 1.15), rng.uniform(0.85, 1.15), rng.uniform(0, math.pi)
            def at(a, rr):
                x, y = math.cos(a) * rr * sx, math.sin(a) * rr * sy
                return cx + x * math.cos(rot) - y * math.sin(rot), cy + x * math.sin(rot) + y * math.cos(rot)
            db.polygon([at(2 * math.pi * i / 48, R * 1.08) for i in range(48)], fill=255)
            dc.polygon([at(2 * math.pi * i / 48, R * 0.62) for i in range(48)], fill=255)
            n = int(2 * math.pi * R / 13)
            for i in range(n):
                a = 2 * math.pi * (i + rng.uniform(-0.2, 0.2)) / n
                x, y = at(a, R * rng.uniform(0.92, 1.0))
                r = rng.uniform(4.0, 5.2)
                # Elongated along the radius.
                ang = a + rot
                pts = [(x + math.cos(t) * r * 1.6 * math.cos(ang) - math.sin(t) * r * math.sin(ang),
                        y + math.cos(t) * r * 1.6 * math.sin(ang) + math.sin(t) * r * math.cos(ang)) for t in np.linspace(0, 2 * math.pi, 12)]
                d.polygon(pts, fill=int(rng.uniform(210, 255)))
        # Scattered thin fibroblast nuclei in the stroma.
        for _ in range(110):
            x, y, a = rng.uniform(0, w), rng.uniform(0, h), rng.uniform(0, math.pi)
            if any(math.hypot(x - gx, y - gy) < gr * 1.15 for gx, gy, gr in glands):
                continue
            l = rng.uniform(7, 12)
            d.line([(x - math.cos(a) * l, y - math.sin(a) * l), (x + math.cos(a) * l, y + math.sin(a) * l)], fill=190, width=4)
    else:
        # Sheets of crowded, big, dark, irregular nuclei; some dividing.
        db.rectangle([0, 0, w, h], fill=120)
        for _ in range(1100):
            x, y = rng.uniform(0, w), rng.uniform(0, h)
            r = rng.uniform(7, 15) * (1.6 if rng.random() < 0.06 else 1.0)
            k = 10
            ph = rng.uniform(0, 6.28)
            pts = [(x + math.cos(2 * math.pi * i / k) * r * (1 + 0.22 * math.sin(3 * (2 * math.pi * i / k) + ph)),
                    y + math.sin(2 * math.pi * i / k) * r * rng.uniform(0.75, 1.0)) for i in range(k)]
            d.polygon(pts, fill=int(rng.uniform(170, 255)))
        for _ in range(30):
            x, y = rng.uniform(0, w), rng.uniform(0, h)
            for dx in (-8, 8):
                d.ellipse([x + dx - 5, y - 9, x + dx + 5, y + 9], fill=255)
    cell = blur(np.asarray(band, np.float32), 3) / 255.0
    img = img * (1 - cell[..., None] * 0.35) + np.array([214, 140, 186], np.float32) * cell[..., None] * 0.35
    lumen = blur(np.asarray(cyto, np.float32), 6) / 255.0
    img = img * (1 - lumen[..., None] * 0.55) + np.array([250, 236, 242], np.float32) * lumen[..., None] * 0.55
    nuc = blur(np.asarray(nuclei, np.float32), 1.4) / 255.0
    texture = 0.8 + 0.4 * noise(w, h, 6, 2)
    purple = np.array([78, 40, 120], np.float32) * texture[..., None]
    img = img * (1 - nuc[..., None]) + purple * nuc[..., None]
    img += rng.normal(0, 3, img.shape).astype(np.float32)
    # The eyepiece: a round field with a dark rim, transparent outside.
    yy, xx = np.mgrid[0:h, 0:w]
    r = np.hypot(xx - w / 2, yy - h / 2) / (w / 2)
    img *= (1 - 0.55 * np.clip((r - 0.78) / 0.22, 0, 1) ** 2)[..., None]
    alpha = np.clip((1.0 - r) * w / 2 / 1.5, 0, 1) * 255
    rgba = np.dstack([np.clip(img, 0, 255), alpha]).astype(np.uint8)
    return Image.fromarray(rgba, "RGBA")


def tape(w, h):
    img = np.zeros((h, w, 4), np.float32)
    img[..., :3] = np.array([236, 226, 196], np.float32) + (noise(w, h, 8, 2)[..., None] - 0.5) * 18
    a = np.full((h, w), 205.0, np.float32)
    # Torn ends.
    for x0, side in ((0, 1), (w, -1)):
        edge = rng.uniform(4, 14, h)
        edge = blur(np.tile(edge, (3, 1)) * 10, 1)[1] / 10 if False else edge
        for y in range(h):
            for x in range(int(edge[y])):
                xi = x if side == 1 else w - 1 - x
                a[y, xi] = 0
    img[..., 3] = a
    return Image.fromarray(np.uint8(np.clip(img, 0, 255)), "RGBA")


def main():
    out = os.path.join(sys.argv[1], "art")
    os.makedirs(out, exist_ok=True)
    paper(1080, 1920).save(os.path.join(out, "paper.png"))
    slide(820, False).save(os.path.join(out, "slide_healthy.png"))
    slide(820, True).save(os.path.join(out, "slide_tumor.png"))
    tape(170, 46).save(os.path.join(out, "tape.png"))
    print("art written to", out)


if __name__ == "__main__":
    main()
