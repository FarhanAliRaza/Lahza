"""Dense line detail that follows a face's structure: broken forehead lines,
crow's feet, stacked eye bags, brow furrows, nose-to-jowl folds, cheek
creases, neck rings and hair strands. Every generator returns stroke
segments; `ink_layer` turns them into one boiling ink layer.

Lines are broken into pieces with gaps and vary in weight, the way a pen
lifts: the density of an inked close-up comes from many short, purposeful
marks, not from long even hatching.
"""
import math
import random

from kit import INK, FINE, strokes


def _broken(pts, rnd, gap=0.18, pieces=(1, 3)):
    """Split a polyline into 1–3 pieces with small gaps (a lifted pen)."""
    n = rnd.randint(*pieces)
    if n == 1 or len(pts) < 4:
        return [pts]
    cuts = sorted(rnd.sample(range(1, len(pts) - 1), min(n - 1, len(pts) - 2)))
    out, a = [], 0
    for c in cuts + [len(pts)]:
        seg = pts[a:c]
        if len(seg) >= 2:
            out.append(seg)
        a = c + (1 if rnd.random() < 0.7 else 0)
    return out


def arc_line(cx, cy, rx, ry, a0, a1, n=10, wave=0.0, rnd=None):
    rnd = rnd or random.Random(0)
    out = []
    ph = rnd.uniform(0, math.tau)
    for i in range(n):
        a = math.radians(a0 + (a1 - a0) * i / (n - 1))
        w = wave * math.sin(i * 1.7 + ph)
        out.append((cx + (rx + w) * math.cos(a), cy + (ry + w) * math.sin(a)))
    return out


def forehead(cx, y_top, y_bot, half_w, n=5, seed=0, sag=10):
    """Horizontal lines across a forehead, sagging at the sides, broken."""
    rnd = random.Random(seed)
    segs = []
    for k in range(n):
        y = y_top + (y_bot - y_top) * (k + 0.5) / n + rnd.uniform(-3, 3)
        hw = half_w * rnd.uniform(0.55, 0.95) * (1 - 0.1 * abs(k - n / 2) / n)
        pts = [(cx + hw * (2 * i / 11 - 1), y + sag * (2 * i / 11 - 1) ** 2 + rnd.uniform(-1.2, 1.2)) for i in range(12)]
        segs += _broken(pts, rnd, pieces=(2, 4))
    return segs


def crows_feet(x, y, side, n=5, length=26, spread=70, seed=0):
    """Fan of short creases from an eye's outer corner."""
    rnd = random.Random(seed)
    segs = []
    for k in range(n):
        a = math.radians(-spread / 2 + spread * k / max(1, n - 1) + rnd.uniform(-6, 6))
        l = length * rnd.uniform(0.6, 1.1)
        p0 = (x + side * 4 * math.cos(a), y + 4 * math.sin(a))
        pts = [(p0[0] + side * l * t * math.cos(a), p0[1] + l * t * math.sin(a) + 3 * t * t) for t in (0, 0.35, 0.7, 1.0)]
        segs.append(pts)
    return segs


def eye_bags(cx, cy, w, n=4, seed=0, step=6):
    """Stacked arcs under an eye, each shorter than the last."""
    rnd = random.Random(seed)
    segs = []
    for k in range(n):
        rx = w * (0.55 - 0.07 * k) * rnd.uniform(0.9, 1.1)
        y = cy + 4 + k * step
        segs += _broken(arc_line(cx + rnd.uniform(-2, 2), y - 6, rx, 7 + k * 2, 20, 160, 9, 0.6, rnd), rnd, pieces=(1, 2))
    return segs


def lids(cx, cy, w, h, n=3, seed=0, side=1):
    """Extra lid folds above an eye (hooded, heavy)."""
    rnd = random.Random(seed)
    segs = []
    for k in range(n):
        rx = w * (0.55 + 0.08 * k)
        segs += _broken(arc_line(cx + side * k * 2, cy - h * (0.9 + 0.45 * k), rx, h * 0.5 + k * 2, 195, 345, 9, 0.5, rnd), rnd, pieces=(1, 2))
    return segs


def furrows(cx, y0, y1, n=3, seed=0, spacing=8):
    """Short vertical creases between the brows."""
    rnd = random.Random(seed)
    return [[(cx + (k - (n - 1) / 2) * spacing + rnd.uniform(-1, 1) + dx, y0 + (y1 - y0) * t) for t, dx in ((0, 0), (0.5, rnd.uniform(-1.5, 1.5)), (1, rnd.uniform(-2, 2)))] for k in range(n)]


def fold(p0, p1, bulge, n=8, seed=0, pieces=(1, 2)):
    """A curved crease from p0 to p1 bowing sideways by `bulge`."""
    rnd = random.Random(seed)
    dx, dy = p1[0] - p0[0], p1[1] - p0[1]
    l = math.hypot(dx, dy) or 1
    nx, ny = -dy / l, dx / l
    pts = [(p0[0] + dx * t + nx * bulge * math.sin(math.pi * t), p0[1] + dy * t + ny * bulge * math.sin(math.pi * t)) for t in [i / (n - 1) for i in range(n)]]
    return _broken(pts, rnd, pieces=pieces)


def cheek_creases(cx, cy, rx, ry, a0, a1, n=4, seed=0):
    """Short comma strokes along an arc (cheekbone, jowl)."""
    rnd = random.Random(seed)
    segs = []
    for k in range(n):
        a = math.radians(a0 + (a1 - a0) * (k + rnd.uniform(0.2, 0.8)) / n)
        p = (cx + rx * math.cos(a), cy + ry * math.sin(a))
        t = (-math.sin(a), math.cos(a))
        l = rnd.uniform(8, 16)
        segs.append([(p[0] - t[0] * l / 2, p[1] - t[1] * l / 2), (p[0], p[1] + 1.5), (p[0] + t[0] * l / 2, p[1] + t[1] * l / 2 + 2)])
    return segs


def strands(poly_top, poly_bottom_y, x0, x1, n=40, seed=0, wave=4, sway=0.0):
    """Hair or beard strands falling from a top edge to a bottom line:
    `poly_top(x)` gives the top y at x."""
    rnd = random.Random(seed)
    segs = []
    for _ in range(n):
        x = rnd.uniform(x0, x1)
        y0 = poly_top(x) + rnd.uniform(0, 10)
        y1 = poly_bottom_y(x) * rnd.uniform(0.85, 1.0) + y0 * rnd.uniform(0.0, 0.15)
        ph = rnd.uniform(0, math.tau)
        pts = []
        for i in range(7):
            t = i / 6
            pts.append((x + wave * math.sin(t * 5 + ph) + sway * t * (x - (x0 + x1) / 2) / max(1, (x1 - x0) / 2) * 20, y0 + (y1 - y0) * t))
        segs.append(pts)
    return segs


def neck_rings(cx, y, hw, n=3, seed=0):
    rnd = random.Random(seed)
    segs = []
    for k in range(n):
        yy = y + k * 9 + rnd.uniform(-2, 2)
        segs += _broken([(cx - hw + i * hw / 4, yy + 3 * math.sin(i)) for i in range(9)], rnd, pieces=(1, 2))
    return segs


def ink_layer(id, segs, w=2.0, color=INK, boil=12, taper=(3, 3), pressure=0.5):
    """One boiling ink layer of many strokes."""
    layer = strokes(id, segs, w=w, color=color, taper=taper, pressure=pressure, wobble=0.3)
    layer["stroke"]["ink"]["boil"] = boil
    return layer


def locks(poly, n=40, length=46, seed=0, curl=10, angle=90):
    """Curled locks of hair flowing along `angle` (degrees, 90 = down)
    inside a shape: short S-curves, the way a pen draws a beard."""
    from kit import points_in
    rnd = random.Random(seed)
    segs = []
    for (x, y) in points_in(poly, n, seed):
        a = math.radians(angle + rnd.uniform(-18, 18))
        l = length * rnd.uniform(0.6, 1.2)
        c = curl * rnd.choice((-1, 1)) * rnd.uniform(0.6, 1.2)
        ux, uy = math.cos(a), math.sin(a)
        nx, ny = -uy, ux
        pts = []
        for i in range(7):
            t = i / 6
            off = c * math.sin(t * math.tau * 0.9)
            pts.append((x + ux * l * t + nx * off, y + uy * l * t + ny * off))
        segs.append(pts)
    return segs
