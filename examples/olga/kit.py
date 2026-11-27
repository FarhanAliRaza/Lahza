"""Hand-inked cel kit for the Lahza motion engine.

Every line is an engine path with a dip-pen `ink` stroke (tapered, pressure
swell, wobble) and every fill a hand-cut (`rough`) flat colour. Seeds come
from the layer id, so a part redraws identically on every frame and only
what is keyed moves.

The helpers return plain layer dicts for `edit_apply`.
"""
import hashlib
import json
import math
import os
import random
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("LAHZA_MOTION", os.path.join(HERE, "../../target/release/lahza-motion"))

# --- the locked palette ---------------------------------------------------------
# One blue, one skin for the whole cast, one pale, two accents. Every other
# value is a tone of these (shade / light / night).
INK = "#170b08"          # warm sepia-black line
FINE = "#4d140ab0"       # finest strokes: dark red, see-through
COBALT = "#2b46a8"
COBALT_DK = "#1c2c6e"
COBALT_LT = "#6d83cc"
SKY = "#b9c8e6"          # day sky: cobalt into cream
SKIN = "#ec7658"         # salmon, the whole cast
SKIN_DK = "#c4523b"      # where something throws shade
SKIN_LT = "#f7a487"      # where fat or bone catches light
CREAM = "#f3e4c2"
CREAM_DK = "#d9c49b"
LILAC = "#ddd0ec"        # eye whites
GOLD = "#dba73a"
GOLD_DK = "#a8741f"
RED = "#ad2e1d"
RED_DK = "#7a1b12"
MOUTH = "#3d0f0a"        # inside a mouth
SHADOW_DAY = "#8f8fc8"   # blue-violet shade by day
NIGHT_LIT = "#e98a78"    # lit surfaces at night (salmon)
NIGHT_SHADE = "#26346f"  # shadow at night (cobalt)

PALETTE = [("cobalt", COBALT), ("salmon skin", SKIN), ("cream", CREAM), ("gold", GOLD), ("red", RED)]


def seed_of(name):
    return int(hashlib.md5(name.encode()).hexdigest()[:6], 16)


def _ink(w, seed, taper=None, pressure=0.4, wobble=0.6, fixed=True):
    ink = {"pressure": pressure, "wobble": wobble, "seed": seed, "fixed": fixed}
    if taper is not None:
        ink["taper"] = list(taper)
    return ink


def r2(p):
    return [round(p[0], 2), round(p[1], 2)]


def shape(id, pts, fill=None, w=3.6, color=INK, closed=True, smooth=None, rough=1.0, taper=None,
          pressure=0.4, wobble=0.6, line=True, opacity=None, **extra):
    """A filled, inked outline through `pts` (Catmull-Rom; closed shapes of
    four points or fewer keep straight edges unless `smooth` says)."""
    if smooth is None:
        smooth = 0.0 if closed and len(pts) <= 4 else 1.0
    s = seed_of(id)
    layer = {"id": id, "kind": "shape", "shape": "path",
             "d": {"points": [r2(p) for p in pts], "closed": closed, **({} if smooth == 1.0 else {"smooth": smooth})}}
    if fill:
        layer["fill"] = fill
    if line and w > 0:
        layer["stroke"] = {"color": color, "width": w,
                           "ink": _ink(w, s, taper if taper is not None else ([0, 0] if closed else None), pressure, wobble)}
    if rough:
        layer["rough"] = {"amount": rough, "scale": 22, "seed": s}
    if opacity is not None:
        layer.setdefault("transform", {})["opacity"] = opacity
    layer.update(extra)
    return layer


def line(id, pts, w=3.6, color=INK, taper=(10, 10), pressure=0.45, wobble=0.6, smooth=1.0, **extra):
    """An open inked line (tapered ends)."""
    return shape(id, pts, None, w, color, closed=False, smooth=smooth, rough=0, taper=taper,
                 pressure=pressure, wobble=wobble, **extra)


def svg_lines(segs):
    return " ".join("M " + " L ".join(f"{p[0]:.1f} {p[1]:.1f}" for p in seg) for seg in segs if len(seg) >= 2)


def strokes(id, segs, w=2.4, color=INK, taper=(3, 3), pressure=0.3, wobble=0.2, **extra):
    """Many short strokes in one layer (ticks, dashes, grain, cracks)."""
    s = seed_of(id)
    layer = {"id": id, "kind": "shape", "shape": "path", "d": svg_lines(segs),
             "stroke": {"color": color, "width": w, "ink": _ink(w, s, taper, pressure, wobble)}}
    layer.update(extra)
    return layer


def dots(id, pts, r=1.6, color=INK, **extra):
    """Small filled dots (pores, stipple)."""
    d = []
    for (x, y) in pts:
        d.append(f"M {x - r:.1f} {y:.1f} A {r} {r} 0 1 0 {x + r:.1f} {y:.1f} A {r} {r} 0 1 0 {x - r:.1f} {y:.1f} Z")
    layer = {"id": id, "kind": "shape", "shape": "path", "d": " ".join(d), "fill": color}
    layer.update(extra)
    return layer


def polys(id, shapes, fill, **extra):
    """Many filled polygons in one layer (no outline): pattern, light patches."""
    d = " ".join("M " + " L ".join(f"{p[0]:.1f} {p[1]:.1f}" for p in pts) + " Z" for pts in shapes)
    layer = {"id": id, "kind": "shape", "shape": "path", "d": d, "fill": fill}
    layer.update(extra)
    return layer


def comp(id, layers, pos=(0, 0), anchor=None, parent=None, **extra):
    t = {"position": r2(pos)}
    if anchor is not None:
        t["anchor"] = r2(anchor)
    layer = {"id": id, "kind": "comp", "transform": t, "layers": layers}
    if parent:
        layer["parent"] = parent
    layer.update(extra)
    return layer


# --- geometry helpers -------------------------------------------------------------

def blob(cx, cy, rx, ry, n=14, lump=0.07, seed=0, start=0.0, squash=None):
    """A lumpy closed mass around an ellipse."""
    rnd = random.Random(seed)
    pts = []
    for i in range(n):
        a = start + math.tau * i / n
        k = 1 + rnd.uniform(-lump, lump)
        x, y = math.cos(a) * rx * k, math.sin(a) * ry * k
        if squash:
            x, y = squash(x, y)
        pts.append((cx + x, cy + y))
    return pts


def arc(cx, cy, rx, ry, a0, a1, n=8):
    """Points along an ellipse from a0 to a1 degrees (0 = right, clockwise, y down)."""
    return [(cx + rx * math.cos(math.radians(a0 + (a1 - a0) * i / (n - 1))),
             cy + ry * math.sin(math.radians(a0 + (a1 - a0) * i / (n - 1)))) for i in range(n)]


def lerp(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


def offset(pts, dx, dy):
    return [(x + dx, y + dy) for x, y in pts]


def scale_pts(pts, sx, sy=None, about=(0, 0)):
    sy = sx if sy is None else sy
    return [(about[0] + (x - about[0]) * sx, about[1] + (y - about[1]) * sy) for x, y in pts]


def mirror(pts, cx=0.0):
    return [(2 * cx - x, y) for x, y in pts]


def barbed(pts, depth=7, every=2, seed=0):
    """A shape's outline with barbs pushed out at every `every`-th point
    (facial hair, fur edges)."""
    rnd = random.Random(seed)
    out = []
    n = len(pts)
    cx = sum(p[0] for p in pts) / n
    cy = sum(p[1] for p in pts) / n
    for i, (x, y) in enumerate(pts):
        out.append((x, y))
        if i % every == 0:
            nx_, ny_ = pts[(i + 1) % n]
            mx, my = (x + nx_) / 2, (y + ny_) / 2
            dx, dy = mx - cx, my - cy
            l = math.hypot(dx, dy) or 1
            d = depth * rnd.uniform(0.6, 1.3)
            out.append((mx + dx / l * d, my + dy / l * d))
    return out


def resample_closed(pts, n):
    """`n` points evenly along a closed polygon."""
    segs = []
    total = 0
    for i in range(len(pts)):
        a, b = pts[i], pts[(i + 1) % len(pts)]
        l = math.hypot(b[0] - a[0], b[1] - a[1])
        segs.append((a, b, l))
        total += l
    out = []
    for k in range(n):
        d = total * k / n
        for a, b, l in segs:
            if d <= l:
                out.append(lerp(a, b, d / l if l else 0))
                break
            d -= l
    return out


# --- texture ------------------------------------------------------------------

def ticks_in(poly_pts, n, length=7, angle=None, seed=0, jitter=25):
    """Short tick strokes scattered inside a polygon."""
    rnd = random.Random(seed)
    xs = [p[0] for p in poly_pts]
    ys = [p[1] for p in poly_pts]
    out = []
    tries = 0
    while len(out) < n and tries < n * 30:
        tries += 1
        x, y = rnd.uniform(min(xs), max(xs)), rnd.uniform(min(ys), max(ys))
        if not inside(poly_pts, (x, y)):
            continue
        a = math.radians((angle if angle is not None else rnd.uniform(0, 180)) + rnd.uniform(-jitter, jitter))
        l = length * rnd.uniform(0.6, 1.2)
        out.append([(x - math.cos(a) * l / 2, y - math.sin(a) * l / 2), (x + math.cos(a) * l / 2, y + math.sin(a) * l / 2)])
    return out


def points_in(poly_pts, n, seed=0):
    rnd = random.Random(seed)
    xs = [p[0] for p in poly_pts]
    ys = [p[1] for p in poly_pts]
    out = []
    tries = 0
    while len(out) < n and tries < n * 40:
        tries += 1
        p = (rnd.uniform(min(xs), max(xs)), rnd.uniform(min(ys), max(ys)))
        if inside(poly_pts, p):
            out.append(p)
    return out


def inside(poly, p):
    x, y = p
    c = False
    n = len(poly)
    for i in range(n):
        x1, y1 = poly[i]
        x2, y2 = poly[(i + 1) % n]
        if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / (y2 - y1 + 1e-12) + x1:
            c = not c
    return c


def dash_rows(x0, x1, y0, y1, gap=9, dash=6, slant=35, seed=0):
    """Rows of small slanted dashes (fur, wool, knit) in a box."""
    rnd = random.Random(seed)
    out = []
    y = y0
    row = 0
    while y <= y1:
        x = x0 + (row % 2) * dash * 0.7
        while x <= x1:
            a = math.radians(slant + rnd.uniform(-12, 12))
            l = dash * rnd.uniform(0.7, 1.15)
            out.append([(x, y), (x + math.cos(a) * l, y + math.sin(a) * l)])
            x += dash * 1.6
        y += gap
        row += 1
    return out


def clip_segs(segs, poly):
    return [s for s in segs if inside(poly, s[0]) and inside(poly, s[-1])]


def cracks(x, y, length, seed=0, depth=3, angle=90):
    """A branching crack from (x, y)."""
    rnd = random.Random(seed)
    out = []

    def grow(px, py, a, l, d):
        pts = [(px, py)]
        for _ in range(4):
            a += rnd.uniform(-35, 35)
            px += math.cos(math.radians(a)) * l / 4
            py += math.sin(math.radians(a)) * l / 4
            pts.append((px, py))
        out.append(pts)
        if d > 0:
            for _ in range(rnd.randint(1, 2)):
                k = rnd.randint(1, len(pts) - 1)
                grow(pts[k][0], pts[k][1], a + rnd.choice([-1, 1]) * rnd.uniform(30, 60), l * 0.55, d - 1)

    grow(x, y, angle, length, depth)
    return out


def masonry(x0, y0, x1, y1, course=26, brick=54, seed=0):
    """Broken course lines and joints (stone)."""
    rnd = random.Random(seed)
    segs = []
    y = y0
    row = 0
    while y < y1:
        # course line in broken pieces
        x = x0
        while x < x1:
            l = rnd.uniform(brick * 0.6, brick * 1.4)
            if rnd.random() > 0.18:
                segs.append([(x, y + rnd.uniform(-1.5, 1.5)), (min(x + l, x1), y + rnd.uniform(-1.5, 1.5))])
            x += l + rnd.uniform(3, 8)
        x = x0 + (row % 2) * brick / 2
        while x < x1:
            if rnd.random() > 0.15:
                segs.append([(x, y + 2), (x + rnd.uniform(-2, 2), min(y + course - 2, y1))])
            x += brick * rnd.uniform(0.8, 1.2)
        y += course
        row += 1
    return segs


def planks(x0, y0, x1, y1, width=40, vertical=True, seed=0):
    """Plank seams and grain ticks (wood). Returns (seams, grain)."""
    rnd = random.Random(seed)
    seams, grain = [], []
    if vertical:
        x = x0
        while x < x1:
            seams.append([(x, y0), (x + rnd.uniform(-1.5, 1.5), y1)])
            for _ in range(int((y1 - y0) / 34)):
                gy = rnd.uniform(y0 + 6, y1 - 12)
                gx = x + rnd.uniform(6, width - 6)
                grain.append([(gx, gy), (gx + rnd.uniform(-1, 1), gy + rnd.uniform(6, 13))])
            x += width * rnd.uniform(0.9, 1.1)
    else:
        y = y0
        while y < y1:
            seams.append([(x0, y), (x1, y + rnd.uniform(-1.5, 1.5))])
            for _ in range(int((x1 - x0) / 34)):
                gx = rnd.uniform(x0 + 6, x1 - 12)
                gy = y + rnd.uniform(6, width - 6)
                grain.append([(gx, gy), (gx + rnd.uniform(6, 13), gy + rnd.uniform(-1, 1))])
            y += width * rnd.uniform(0.9, 1.1)
    return seams, grain


# --- the old-cartoon finish (applied to the composed frame's comp) ----------------

FINISH = [
    {"type": "blur", "radius": 0.55},                                   # soften the ink ~1 px
    {"type": "sharpen", "radius": 5, "amount": 0.6},                    # unsharp: faint pale rim
    {"type": "chroma_blur", "radius": 2.5, "offset": 2, "angle": 20},   # colour off-register
    {"type": "mottle", "amount": 0.035, "scale": 160, "speed": 0.2},    # slow mottling of fills
    {"type": "tint", "black": "#2c160e", "white": "#fff3dc", "amount": 0.14},  # warm, blacks lifted
    {"type": "levels", "out_black": 0.05},
    {"type": "glow", "color": "#ffe9c8", "radius": 18, "intensity": 0.12},     # gentle bloom
    {"type": "vignette", "amount": 0.32, "softness": 0.6},
    {"type": "grain", "amount": 0.035, "size": 1.2},                    # luma grain
]


# --- engine driver -----------------------------------------------------------------

def run(*args, check=True):
    r = subprocess.run([BIN, *map(str, args), "--cpu"], capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(map(str, args))}: {r.stderr or r.stdout}")
    return r.stdout


def apply(project, ops, path):
    with open(path, "w") as f:
        json.dump(ops, f)
    out = run("motion", "apply", project, path)
    data = json.loads(out[out.index("{"):])
    if data.get("errors"):
        raise RuntimeError(json.dumps(data["errors"])[:2000])
    return data
