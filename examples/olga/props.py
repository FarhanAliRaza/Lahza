"""Props for "Olga of Kyiv": every noun gets a drawing in the same ink and
flat colour. Each returns layers around its own origin."""
import math
import random

from kit import *  # noqa: F401,F403
from face import sparkle


def pigeon(id, wing=0, s=1.0, bundle=False, lit=False):
    """A pigeon facing right: grey-blue body, a dark wing, red feet. wing 0
    is up, 1 down (alternate them to flap)."""
    L = []
    body = [(-40 * s, 0), (-10 * s, -22 * s), (30 * s, -24 * s), (52 * s, -40 * s), (66 * s, -36 * s), (64 * s, -20 * s), (44 * s, 2 * s), (10 * s, 16 * s), (-30 * s, 12 * s), (-62 * s, 6 * s)]
    L.append(shape(f"{id}-body", body, COBALT_LT, w=2.8))
    L.append(polys(f"{id}-belly", [[(-20 * s, 8 * s), (30 * s, 0), (40 * s, 6 * s), (6 * s, 16 * s)]], CREAM))
    L.append(polys(f"{id}-neck", [[(34 * s, -24 * s), (52 * s, -36 * s), (52 * s, -20 * s), (40 * s, -8 * s)]], RED))
    L.append(shape(f"{id}-beak", [(64 * s, -34 * s), (76 * s, -30 * s), (64 * s, -27 * s)], GOLD, w=1.6, smooth=0.0))
    L.append(dots(f"{id}-eye", [(58 * s, -33 * s)], r=2.2 * s))
    if wing == 0:
        wp = [(-6 * s, -14 * s), (20 * s, -20 * s), (-10 * s, -78 * s), (-36 * s, -86 * s), (-30 * s, -40 * s)]
    else:
        wp = [(-6 * s, -10 * s), (24 * s, -12 * s), (-4 * s, 40 * s), (-34 * s, 52 * s), (-30 * s, 10 * s)]
    L.append(shape(f"{id}-wing", wp, COBALT, w=2.6))
    L.append(strokes(f"{id}-feathers", [[lerp(wp[1], wp[3], k), lerp(wp[0], wp[4], k)] for k in (0.35, 0.6, 0.8)], w=1.4, color=FINE))
    L.append(strokes(f"{id}-feet", [[(0, 14 * s), (4 * s, 26 * s)], [(14 * s, 12 * s), (18 * s, 24 * s)]], w=2.6 * s, color=RED, taper=(0, 0)))
    if bundle:
        L += sulphur(f"{id}-sul", 6 * s, 30 * s, 0.5 * s, lit=lit)
    return L


def sparrow(id, wing=0, s=1.0, bundle=False, lit=False):
    """A sparrow facing right: brown-gold, cream breast, a dark cap."""
    L = []
    body = [(-30 * s, 0), (-4 * s, -18 * s), (24 * s, -18 * s), (36 * s, -8 * s), (26 * s, 8 * s), (-4 * s, 12 * s), (-44 * s, 4 * s)]
    L.append(shape(f"{id}-body", body, GOLD_DK, w=2.6))
    L.append(polys(f"{id}-breast", [[(6 * s, 0), (28 * s, -4 * s), (22 * s, 8 * s), (0, 10 * s)]], CREAM))
    L.append(polys(f"{id}-cap", [[(14 * s, -18 * s), (30 * s, -16 * s), (32 * s, -10 * s), (16 * s, -12 * s)]], RED_DK))
    L.append(shape(f"{id}-beak", [(35 * s, -10 * s), (44 * s, -7 * s), (35 * s, -4 * s)], INK, w=1.2, smooth=0.0))
    L.append(dots(f"{id}-eye", [(28 * s, -10 * s)], r=1.8 * s))
    wp = [(-4 * s, -10 * s), (14 * s, -12 * s), (-6 * s, -56 * s), (-24 * s, -54 * s)] if wing == 0 else [(-4 * s, -6 * s), (16 * s, -6 * s), (-2 * s, 30 * s), (-22 * s, 34 * s)]
    L.append(shape(f"{id}-wing", wp, RED_DK, w=2.2))
    L.append(strokes(f"{id}-dash", dash_rows(-20 * s, 10 * s, -14 * s, 4 * s, gap=6 * s, dash=4 * s, slant=30, seed=seed_of(id)), w=1.2, color=INK, taper=(1, 1)))
    if bundle:
        L += sulphur(f"{id}-sul", 0, 24 * s, 0.4 * s, lit=lit)
    return L


def flier(id, kind, x0, y0, x1, y1, t0, t1, s=1.0, flap=0.09, bundle=False, lit=False, arc=-120):
    """A bird flying from (x0, y0) to (x1, y1) over [t0, t1], flapping."""
    draw = pigeon if kind == "pigeon" else sparrow
    up = comp(f"{id}-u", draw(f"{id}-u", 0, s, bundle, lit))
    dn = comp(f"{id}-d", draw(f"{id}-d", 1, s, bundle, lit))
    ks_u, ks_d = [], []
    t, k = t0, 0
    while t < t1 + flap:
        ks_u.append({"at": round(t, 3), "v": 1 if k % 2 == 0 else 0, "ease": "hold"})
        ks_d.append({"at": round(t, 3), "v": 0 if k % 2 == 0 else 1, "ease": "hold"})
        t += flap
        k += 1
    up["transform"]["opacity"] = {"keys": ks_u}
    dn["transform"]["opacity"] = {"keys": ks_d}
    mid = ((x0 + x1) / 2, (y0 + y1) / 2 + arc)
    pos = {"keys": [{"at": round(t0, 3), "v": r2((x0, y0))}, {"at": round((t0 + t1) / 2, 3), "v": r2(mid), "ease": "linear"},
                    {"at": round(t1, 3), "v": r2((x1, y1)), "ease": "linear"}]}
    sx = 1 if x1 >= x0 else -1
    bird = {"id": id, "kind": "comp", "facing": "right", "transform": {"position": pos, "scale": [sx, 1]}, "layers": [up, dn]}
    if not lit:
        return [bird]
    # A trail of burning sparks along the same path, a little behind.
    trail = []
    for k in range(1, 7):
        lag = 0.05 * k
        keys = [dict(kk, at=round(kk["at"] + lag, 3)) for kk in pos["keys"]]
        d = dots(f"{id}-tr{k}", [(0, 24 * s)], r=(9 - k) * s * 1.6, color=GOLD if k % 2 else RED)
        d["transform"] = {"position": {"keys": keys}, "opacity": round(1 - k / 7, 2)}
        d["time"] = {"start": round(t0 + lag, 3), "end": round(t1 + lag, 3)}
        trail.append(d)
    return trail + [bird]


def sulphur(id, x, y, s=1.0, lit=False):
    """A little rag bundle of sulphur tied with thread (lit: on fire)."""
    L = [line(f"{id}-thread", [(x, y - 20 * s), (x + 2 * s, y)], w=1.6, taper=(1, 1)),
         shape(f"{id}-rag", [(x - 14 * s, y), (x + 14 * s, y - 2 * s), (x + 18 * s, y + 16 * s), (x + 4 * s, y + 26 * s), (x - 16 * s, y + 18 * s)], CREAM, w=2.2),
         dots(f"{id}-sulphur", [(x - 4 * s, y + 10 * s), (x + 6 * s, y + 8 * s), (x, y + 16 * s)], r=3 * s, color=GOLD)]
    if lit:
        k = 2.2
        L.append(shape(f"{id}-fire", [(x - 14 * s * k, y + 6 * s), (x - 8 * s * k, y - 30 * s * k), (x + 2 * s, y - 14 * s * k), (x + 8 * s * k, y - 42 * s * k), (x + 16 * s * k, y + 6 * s)], GOLD, w=2.4, color=RED))
        L.append(shape(f"{id}-firein", [(x - 6 * s * k, y + 2 * s), (x, y - 18 * s * k), (x + 6 * s * k, y + 2 * s)], CREAM, w=0, line=False, rough=0))
    return L


def letter(id, w=520, h=300, draw_t=None):
    """A birch-bark letter: cream bark with gold edges and scratched lines
    (draw_t: (t0, t1) to scratch the lines on)."""
    L = []
    bark = [(-w / 2, -h / 2), (w / 2, -h / 2 + 8), (w / 2 + 10, h / 2 - 6), (-w / 2 + 6, h / 2)]
    L.append(shape(f"{id}-bark", bark, CREAM, w=3.6, rough=2.0))
    L.append(strokes(f"{id}-birch", [[(x, -h / 2 + 4 + (x * 7) % 30), (x + 22, -h / 2 + 4 + (x * 7) % 30)] for x in range(int(-w / 2) + 20, int(w / 2) - 30, 47)]
                     + [[(x, h / 2 - 20 - (x * 5) % 24), (x + 18, h / 2 - 20 - (x * 5) % 24)] for x in range(int(-w / 2) + 30, int(w / 2) - 30, 61)], w=2.2, color=INK, taper=(2, 2)))
    rnd = random.Random(seed_of(id))
    for row in range(5):
        y = -h / 2 + 60 + row * 44
        pts = []
        x = -w / 2 + 40
        while x < w / 2 - 50:
            pts.append((x, y + rnd.uniform(-8, 8)))
            x += rnd.uniform(12, 20)
        lay = line(f"{id}-script{row}", pts, w=2.6, color=RED_DK, taper=(3, 3), pressure=0.6, wobble=1.2, smooth=0.4)
        if draw_t:
            a, b = draw_t
            ta = a + (b - a) * row / 5
            tb = a + (b - a) * (row + 1) / 5
            lay["trim"] = {"keys": [{"at": round(ta, 3), "v": [0, 0]}, {"at": round(tb, 3), "v": [0, 1], "ease": "linear"}]}
        L.append(lay)
    return L


def stylus(id, s=1.0):
    return [line(f"{id}", [(0, 0), (90 * s, -150 * s)], w=7 * s, color=GOLD_DK, taper=(1, 0), pressure=0.1),
            line(f"{id}-ink", [(0, 0), (90 * s, -150 * s)], w=2.2, taper=(1, 0))]


def tribute(id, s=1.0):
    """A heap of tribute: furs, a honey pot, gold coins."""
    L = []
    rnd = random.Random(seed_of(id))
    for k, (x, y) in enumerate([(-120, -20), (0, -50), (120, -10), (-50, 20), (70, 30)]):
        fur = blob(x * s, y * s, 90 * s, 40 * s, n=12, lump=0.15, seed=k)
        L.append(shape(f"{id}-fur{k}", barbed(fur, depth=6 * s, every=1, seed=k), GOLD_DK if k % 2 else GOLD, w=3.0, smooth=0.5))
        L.append(strokes(f"{id}-furd{k}", clip_segs(dash_rows((x - 80) * s, (x + 80) * s, (y - 30) * s, (y + 30) * s, gap=9 * s, dash=7 * s, slant=70, seed=k), fur), w=1.6, color=INK, taper=(1, 2)))
    L.append(shape(f"{id}-pot", [(150 * s, -110 * s), (220 * s, -110 * s), (240 * s, -40 * s), (210 * s, 0), (160 * s, 0), (130 * s, -40 * s)], RED, w=3.2))
    L.append(shape(f"{id}-honey", [(150 * s, -112 * s), (220 * s, -112 * s), (210 * s, -84 * s), (190 * s, -70 * s), (176 * s, -92 * s), (156 * s, -86 * s)], GOLD, w=2.2))
    for k in range(9):
        x, y = rnd.uniform(-180, 120) * s, rnd.uniform(30, 80) * s
        L.append(shape(f"{id}-coin{k}", blob(x, y, 18 * s, 12 * s, n=10, lump=0.02), GOLD, w=2.2))
    L.append(sparkle(f"{id}-glint", -60 * s, 40 * s, r=14 * s))
    return L


def torch(id, x, y, s=1.0, t_until=70.0, seed=0):
    from sets import flame
    return [line(f"{id}-stick", [(x, y), (x + 10 * s, y + 260 * s)], w=9 * s, color=INK, taper=(0, 0), pressure=0.1),
            shape(f"{id}-wrap", [(x - 14 * s, y - 6 * s), (x + 16 * s, y - 6 * s), (x + 14 * s, y + 30 * s), (x - 12 * s, y + 30 * s)], CREAM_DK, w=2.4)] + flame(f"{id}-fl", x, y - 4 * s, 0.8 * s, seed=seed, until=t_until)


def door_bar(id, w=300, s=1.0):
    pts = [(-w / 2 * s, -18 * s), (w / 2 * s, -14 * s), (w / 2 * s, 18 * s), (-w / 2 * s, 16 * s)]
    seams, grain = planks(-w / 2 * s, -16 * s, w / 2 * s, 16 * s, 40 * s, vertical=False, seed=seed_of(id))
    return [shape(f"{id}", pts, GOLD_DK, w=3.6, rough=1.0), strokes(f"{id}-grain", grain, w=1.6, color=FINE)]


def dirt_clods(id, x0, x1, y_top, y_bot, t0, t1, n=40, seed=0):
    """Clods of earth falling from y_top to y_bot between t0 and t1."""
    rnd = random.Random(seed)
    L = []
    for k in range(n):
        x = rnd.uniform(x0, x1)
        start = t0 + (t1 - t0) * rnd.uniform(0, 0.7)
        dur = rnd.uniform(0.35, 0.6)
        r = rnd.uniform(7, 17)
        clod = shape(f"{id}-{k}", blob(0, 0, r, r * 0.8, n=7, lump=0.3, seed=k), GOLD_DK if k % 3 else RED_DK, w=2.0, rough=0.5)
        clod["transform"] = {"position": {"keys": [{"at": round(start, 3), "v": r2((x, y_top - rnd.uniform(0, 300)))}, {"at": round(start + dur, 3), "v": r2((x + rnd.uniform(-40, 40), y_bot)), "ease": "in"}]},
                             "rotation": {"keys": [{"at": round(start, 3), "v": 0}, {"at": round(start + dur, 3), "v": rnd.uniform(-180, 180), "ease": "linear"}]}}
        clod["time"] = {"start": round(start, 3), "end": round(t1 + 2, 3)}
        L.append(clod)
    return L


def embers(id, x0, x1, y0, t0, t1, n=24, seed=0, rise=500):
    """Sparks drifting up from a fire."""
    rnd = random.Random(seed)
    L = []
    for k in range(n):
        x = rnd.uniform(x0, x1)
        start = t0 + (t1 - t0) * rnd.random() - 0.8
        dur = rnd.uniform(0.9, 1.6)
        e = dots(f"{id}-{k}", [(0, 0)], r=rnd.uniform(2.5, 5), color=GOLD if k % 2 else CREAM)
        e["transform"] = {"position": {"keys": [{"at": round(start, 3), "v": r2((x, y0))}, {"at": round(start + dur, 3), "v": r2((x + rnd.uniform(-80, 80), y0 - rise * rnd.uniform(0.6, 1.2))), "ease": "out"}]},
                          "opacity": {"keys": [{"at": round(start, 3), "v": 1}, {"at": round(start + dur, 3), "v": 0, "ease": "in"}]}}
        L.append(e)
    return L


def puffs(id, x, y, t0, t1, n=6, s=1.0, color=CREAM, rise=420, seed=0, every=None, lead=1.0):
    """Smoke or steam puffs rising and spreading. `lead`: seconds the first
    puff starts before t0 (continuous smoke is already rising when a shot
    opens; an impact's dust must not appear before the impact: lead 0)."""
    rnd = random.Random(seed)
    L = []
    every = every or (t1 - t0) / n
    for k in range(n):
        start = t0 + k * every - lead
        dur = rnd.uniform(1.8, 2.6)
        p = shape(f"{id}-{k}", blob(0, 0, 50 * s, 36 * s, n=10, lump=0.18, seed=k), color, w=2.4, rough=0.6)
        p["transform"] = {"position": {"keys": [{"at": round(start, 3), "v": r2((x, y))}, {"at": round(start + dur, 3), "v": r2((x + rnd.uniform(-60, 120) * s, y - rise * s)), "ease": "out"}]},
                          "scale": {"keys": [{"at": round(start, 3), "v": [0.4, 0.4]}, {"at": round(start + dur, 3), "v": [1.6, 1.6], "ease": "out"}]},
                          "opacity": {"keys": [{"at": round(start, 3), "v": 0.85}, {"at": round(start + dur, 3), "v": 0, "ease": "in"}]}}
        L.append(p)
    return L
