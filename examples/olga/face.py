"""Face parts shared by the cast: hooded eyes, brows, noses, mouths with
teeth and tongues, bags, folds, pores and stubble. Each returns layers in
the head's own space (origin at the base of the neck, y down)."""
import math
import random

from kit import (INK, FINE, LILAC, MOUTH, RED, RED_DK, CREAM, SKIN, SKIN_DK, SKIN_LT, GOLD, GOLD_DK,
                 shape, line, strokes, dots, arc, lerp, mirror, points_in, ticks_in)


def eye(id, cx, cy, w=30, h=13, look=(0.0, 0.0), lid=0.45, tilt=0.0, mood="neutral", side=1, skin=SKIN, lash=5.2):
    """A small hooded eye: lilac white, dark pupil with a glint, a skin lid
    over the top `lid` of it, a thick lash line and a crease above.
    mood: neutral | blink | shock | anger | smug | grief | fear."""
    out = []
    t = math.radians(tilt * side)

    def rot(p):
        x, y = p[0] - cx, p[1] - cy
        return (cx + x * math.cos(t) - y * math.sin(t), cy + x * math.sin(t) + y * math.cos(t))

    if mood == "blink":
        out.append(line(f"{id}-shut", [rot((cx - w * 0.55, cy - 1)), rot((cx - w * 0.15, cy + h * 0.35)), rot((cx + w * 0.25, cy + h * 0.32)), rot((cx + w * 0.58, cy - 2))], w=lash, taper=(6, 4)))
        out.append(line(f"{id}-crease", [rot((cx - w * 0.45, cy - h * 0.55)), rot((cx, cy - h * 0.85)), rot((cx + w * 0.5, cy - h * 0.6))], w=2.2, color=FINE, taper=(5, 5)))
        return out
    if mood in ("shock", "fear"):
        r = h * (1.25 if mood == "shock" else 1.05)
        white = [(cx + math.cos(a) * r * 1.05, cy + math.sin(a) * r) for a in [i * math.tau / 12 for i in range(12)]]
        out.append(shape(f"{id}-white", white, LILAC, w=2.6, rough=0.4))
        px, py = cx + look[0] * r * 0.3, cy + look[1] * r * 0.3
        out.append(dots(f"{id}-pupil", [(px, py)], r=2.4 if mood == "shock" else 3.2))
        out.append(line(f"{id}-crease", [(cx - r * 1.1, cy - r * 1.25), (cx, cy - r * 1.55), (cx + r * 1.1, cy - r * 1.3)], w=2.4, color=FINE, taper=(5, 5)))
        return out
    # Almond white.
    white = [rot(p) for p in [(cx - w * 0.55, cy), (cx - w * 0.25, cy - h * 0.55), (cx + w * 0.2, cy - h * 0.6),
                              (cx + w * 0.56, cy - h * 0.05), (cx + w * 0.22, cy + h * 0.45), (cx - w * 0.25, cy + h * 0.4)]]
    out.append(shape(f"{id}-white", white, LILAC, w=0, rough=0, line=False))
    pr = h * (0.42 if mood != "anger" else 0.3)
    px, py = cx + look[0] * w * 0.28, cy + look[1] * h * 0.2 + h * 0.05
    out.append(shape(f"{id}-iris", [(px + math.cos(a) * pr, py + math.sin(a) * pr) for a in [i * math.tau / 10 for i in range(10)]], INK, w=0, line=False, rough=0))
    out.append(dots(f"{id}-glint", [(px - pr * 0.35, py - pr * 0.35)], r=1.3, color=CREAM))
    # The lid comes down over the top of the eye (more when smug or angry).
    lid = {"smug": 0.62, "anger": 0.5, "grief": 0.6}.get(mood, lid)
    ly = cy - h * 0.6 + h * 1.0 * lid
    slope = {"anger": -0.22, "grief": 0.25}.get(mood, 0.0) * side
    lidp = [rot(p) for p in [(cx - w * 0.75, cy - h * 1.6), (cx + w * 0.75, cy - h * 1.6),
                             (cx + w * 0.62, ly - 0.5 * h - slope * w), (cx + w * 0.2, ly - slope * w * 0.4),
                             (cx - w * 0.25, ly + slope * w * 0.4), (cx - w * 0.62, ly - 0.4 * h + slope * w)]]
    out.append(shape(f"{id}-lid", lidp, skin, w=0, line=False, rough=0))
    # Thick lash line along the lid's edge, and the lower lid.
    out.append(line(f"{id}-lash", [rot((cx - w * 0.6, ly - 0.25 * h + slope * w)), rot((cx - w * 0.2, ly + slope * w * 0.4)),
                                    rot((cx + w * 0.25, ly - slope * w * 0.4)), rot((cx + w * 0.62, ly - 0.4 * h - slope * w))], w=lash, taper=(4, 7)))
    out.append(line(f"{id}-low", [rot((cx - w * 0.5, cy + h * 0.15)), rot((cx - w * 0.1, cy + h * 0.5)), rot((cx + w * 0.4, cy + h * 0.35))], w=1.8, color=FINE, taper=(4, 4)))
    # Hood crease.
    out.append(line(f"{id}-crease", [rot((cx - w * 0.55, cy - h * 0.95)), rot((cx, cy - h * 1.25)), rot((cx + w * 0.6, cy - h * 0.9))], w=2.2, color=FINE, taper=(5, 5)))
    return out


def bags(id, cx, cy, w=28, n=2, side=1):
    segs = []
    for k in range(n):
        y = cy + 6 + k * 6
        segs.append([(cx - w * 0.45, y - 2), (cx - w * 0.1, y + 3 + k), (cx + w * 0.35, y + 1)])
    return [strokes(f"{id}", segs, w=1.8, color=FINE, taper=(4, 4))]


def brow(id, cx, cy, w=36, mood="neutral", side=1, thick=7, color=INK, barbs=False):
    """A brow as a filled wedge; anger drives the inner end down, shock lifts it."""
    lift = {"neutral": 0, "anger": 0, "shock": -10, "fear": -8, "smug": -2, "grief": -6, "joy": -4, "blink": 0}.get(mood, 0)
    inner_drop = {"anger": 12, "grief": -9, "fear": -7, "smug": 3}.get(mood, 0)
    # inner end toward the nose (x toward 0)
    inner = (cx - side * w * 0.5, cy + lift + inner_drop)
    outer = (cx + side * w * 0.5, cy + lift - (4 if mood == "smug" else 0) + (2 if mood == "grief" else 0))
    mid = (cx + side * w * 0.05, cy + lift - 7 + (inner_drop * 0.3))
    top = [inner, lerp(inner, mid, 0.5), mid, outer]
    bot = [(outer[0] - side * 3, outer[1] + 3), (mid[0], mid[1] + thick), (inner[0] + side * 2, inner[1] + thick * 0.9)]
    pts = top + bot
    if barbs:
        from kit import barbed
        pts = barbed(pts, depth=4, every=1, seed=len(id))
    return [shape(id, pts, color, w=1.6, rough=0.5, smooth=0.6)]


def teeth_row(id, x0, x1, y, h, curve=4, n=7, gold=None, gums=None, down=1):
    """Teeth one by one along a gentle arc (down = 1 upper row hangs down,
    -1 lower row stands up). `gold`: indices of gold teeth."""
    out = []
    gold = gold or []
    tw = (x1 - x0) / n
    if gums:
        g = [(x0 - 2, y - down * 6)] + [(x0 + tw * (i + 0.5), y - down * (6 + curve * math.sin(math.pi * (i + 0.5) / n))) for i in range(n)] + [(x1 + 2, y - down * 6), (x1, y + down * 3), (x0, y + down * 3)]
        out.append(shape(f"{id}-gums", g, gums, w=1.6, rough=0.3))
    for i in range(n):
        cx = x0 + tw * (i + 0.5)
        cy = y + down * curve * math.sin(math.pi * (i + 0.5) / n) * 0.4
        hh = h * (0.85 + 0.25 * math.sin(math.pi * (i + 0.5) / n))
        pts = [(cx - tw * 0.46, cy), (cx + tw * 0.46, cy), (cx + tw * 0.42, cy + down * hh * 0.8), (cx, cy + down * hh), (cx - tw * 0.42, cy + down * hh * 0.8)]
        out.append(shape(f"{id}-{i}", pts, GOLD if i in gold else CREAM, w=1.7, rough=0.2, smooth=0.5))
    return out


def mouth(id, cx, cy, w=34, mood="neutral", jaw=0.0, lip=RED_DK, gold=None, side_bias=0.0, gums="#d9604f"):
    """Mouths. neutral/smug/anger/grief are lines with lips; open moods draw
    the lip rim, the dark inside, the tongue and both rows of teeth."""
    out = []
    hw = w / 2
    if mood in ("neutral", "blink"):
        out.append(line(f"{id}-line", [(cx - hw, cy + 1), (cx - hw * 0.3, cy - 1), (cx + hw * 0.3, cy), (cx + hw, cy - 1)], w=3.2, taper=(6, 6)))
        out.append(line(f"{id}-lower", [(cx - hw * 0.45, cy + 7), (cx, cy + 9), (cx + hw * 0.45, cy + 7)], w=1.8, color=FINE, taper=(4, 4)))
        return out
    if mood == "smug":
        # One corner up and pushed into the cheek, lips pressed.
        out.append(line(f"{id}-line", [(cx - hw, cy + 2), (cx - hw * 0.2, cy + 1), (cx + hw * 0.5, cy - 2), (cx + hw * 1.05, cy - 9)], w=3.4, taper=(6, 4)))
        out.append(line(f"{id}-dimple", [(cx + hw * 1.15, cy - 15), (cx + hw * 1.25, cy - 7), (cx + hw * 1.12, cy - 1)], w=2.0, color=FINE, taper=(3, 3)))
        out.append(line(f"{id}-lower", [(cx - hw * 0.4, cy + 7), (cx + hw * 0.1, cy + 9), (cx + hw * 0.5, cy + 6)], w=1.8, color=FINE, taper=(4, 4)))
        return out
    if mood in ("anger", "grief"):
        down = 7 if mood == "grief" else 5
        out.append(line(f"{id}-line", [(cx - hw, cy + down), (cx - hw * 0.4, cy - 1), (cx + hw * 0.4, cy - 1), (cx + hw, cy + down)], w=3.6, taper=(5, 5)))
        if mood == "anger":
            out.append(strokes(f"{id}-lines", [[(cx - hw * 0.4, cy + 10), (cx - hw * 0.3, cy + 15)], [(cx + hw * 0.35, cy + 10), (cx + hw * 0.25, cy + 15)]], w=1.8, color=FINE))
        return out
    # Open mouths: shock (jaw dropped, an O), joy / grin (wide, teeth), fear.
    if mood == "shock":
        top = cy - 4
        bot = cy + 18 + 40 * jaw
        mw = hw * 0.9
        rim = [(cx - mw, (top + bot) / 2), (cx - mw * 0.7, top + 2), (cx, top - 3), (cx + mw * 0.7, top + 2), (cx + mw, (top + bot) / 2),
               (cx + mw * 0.75, bot - 4), (cx, bot + 2), (cx - mw * 0.75, bot - 4)]
        out.append(shape(f"{id}-in", rim, MOUTH, w=4.2, rough=0.4))
        out.append(shape(f"{id}-tongue", [(cx - mw * 0.65, bot - 6), (cx - mw * 0.2, bot - 16), (cx + mw * 0.35, bot - 15), (cx + mw * 0.66, bot - 5), (cx, bot)], RED, w=1.6, rough=0.3))
        out += teeth_row(f"{id}-up", cx - mw * 0.6, cx + mw * 0.6, top + 1, 8, curve=2, n=5, gold=gold, gums=gums)
        out += teeth_row(f"{id}-dn", cx - mw * 0.5, cx + mw * 0.5, bot - 2, 6, curve=2, n=4, down=-1)
        return out
    if mood in ("joy", "grin", "fear"):
        wide = hw * (1.25 if mood != "fear" else 1.0)
        top = cy - 6 + side_bias
        bot = cy + 14 + 30 * jaw
        curve = 10 if mood != "fear" else -6
        rim = [(cx - wide, top - curve * 0.6), (cx - wide * 0.5, top + 1), (cx, top + 2), (cx + wide * 0.5, top + 1), (cx + wide, top - curve * 0.6),
               (cx + wide * 0.72, bot - 6), (cx, bot + 2), (cx - wide * 0.72, bot - 6)]
        out.append(shape(f"{id}-in", rim, MOUTH, w=4.2, rough=0.4))
        out.append(shape(f"{id}-tongue", [(cx - wide * 0.45, bot - 5), (cx, bot - 12), (cx + wide * 0.45, bot - 5), (cx, bot)], RED, w=1.4, rough=0.3))
        out += teeth_row(f"{id}-up", cx - wide * 0.78, cx + wide * 0.78, top + 2, 10, curve=4, n=8, gold=gold, gums=gums)
        out += teeth_row(f"{id}-dn", cx - wide * 0.6, cx + wide * 0.6, bot - 3, 7, curve=3, n=6, down=-1)
        # Cheeks bunch up into the grin.
        if mood != "fear":
            for s in (-1, 1):
                out.append(line(f"{id}-fold{s}", [(cx + s * wide * 1.05, top - 22), (cx + s * wide * 1.25, top - 4), (cx + s * wide * 1.05, top + 12)], w=2.4, color=FINE, taper=(4, 4)))
        return out
    raise ValueError(mood)


def pores(id, poly, n=24, seed=0):
    rnd = random.Random(seed)
    pts = points_in(poly, n, seed)
    return [dots(id, pts, r=1.05, color=FINE)]


def skin_ticks(id, poly, n=14, length=6, seed=0, angle=None):
    return [strokes(id, ticks_in(poly, n, length, angle=angle, seed=seed), w=1.6, color=FINE, taper=(2, 2))]


def stubble(id, poly, n=60, seed=0):
    return [strokes(id, ticks_in(poly, n, 4, angle=80, seed=seed, jitter=30), w=1.4, color=INK, taper=(1.5, 1.5))]


def sparkle(id, x, y, r=12, color=CREAM):
    """A four-point glint on a lamp, coin or blade."""
    pts = []
    for i in range(8):
        a = math.tau * i / 8 - math.pi / 2
        rr = r if i % 2 == 0 else r * 0.22
        pts.append((x + math.cos(a) * rr, y + math.sin(a) * rr))
    return shape(id, pts, color, w=1.4, color=INK, rough=0, smooth=0.0)
