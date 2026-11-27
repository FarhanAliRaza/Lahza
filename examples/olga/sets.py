"""Sets for "Olga of Kyiv", each in depth layers (sky, back, mid, fore) so a
moving camera gets parallax. Day: pale blue sky, warm sunlit faces,
blue-violet shade. Night: salmon lit faces, cobalt shade, a pale moon with a
halo. Interiors: one shaft of window light on a cool room.

Every set function returns {"sky": [...], "back": [...], "mid": [...],
"fore": [...]} in world px; the frame is 1080 × 1920 and sets run wider and
taller so the camera can move without showing their edges.
"""
import math
import random

from kit import *  # noqa: F401,F403
from face import sparkle

SET_W, SET_H = 1500, 2300


def rect(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def sky(id, top, bottom, night=False):
    c = NIGHT_SHADE if night else SKY
    return [shape(f"{id}", rect(-200, top, SET_W + 200, bottom), c, w=0, line=False, rough=0)]


def moon(id, x, y, r=70):
    """A pale moon with a soft halo."""
    halo = shape(f"{id}-halo", blob(x, y, r * 2.3, r * 2.3, n=20, lump=0.0), CREAM, w=0, line=False, rough=0,
                 transform={"opacity": 0.16}, effects=[{"type": "blur", "radius": 40}])
    disc = shape(f"{id}", blob(x, y, r, r, n=16, lump=0.03, seed=3), CREAM, w=3.0)
    craters = dots(f"{id}-cr", [(x - 20, y - 14), (x + 18, y + 10), (x - 6, y + 24), (x + 26, y - 22)], r=6, color=CREAM_DK)
    return [halo, disc, craters]


def cloud(id, x, y, s=1.0, color=CREAM):
    pts = []
    for i, (dx, r) in enumerate([(-110, 40), (-60, 62), (0, 78), (64, 58), (112, 38)]):
        pts += arc(x + dx * s, y, r * s, r * s * 0.8, 180, 360, 5)
    pts += [(x + 140 * s, y + 18 * s), (x - 140 * s, y + 18 * s)]
    return shape(id, pts, color, w=3.0, smooth=0.6)


def bird_v(id, x, y, s=1.0):
    return line(id, [(x - 18 * s, y - 6 * s), (x - 6 * s, y), (x, y + 3 * s), (x + 6 * s, y), (x + 18 * s, y - 6 * s)], w=2.6, taper=(4, 4))


def log_wall(id, x0, y0, x1, y1, lit, shade, pointed=True, log_w=46, seed=0):
    """A palisade of upright logs: lit left halves, shaded right halves,
    grain ticks, pointed tops."""
    rnd = random.Random(seed)
    L = []
    shades, grain = [], []
    x = x0
    k = 0
    while x < x1:
        w = log_w * rnd.uniform(0.9, 1.1)
        top = y0 - (rnd.uniform(26, 40) if pointed else 0)
        pts = [(x, y0), (x + w * 0.5, top), (x + w, y0), (x + w, y1), (x, y1)] if pointed else rect(x, y0, x + w, y1)
        L.append(shape(f"{id}-log{k}", pts, lit, w=3.2, smooth=0.0, rough=0.8))
        shades.append([(x + w * 0.55, top + 8 if pointed else y0), (x + w - 2, y0 + 4), (x + w - 2, y1), (x + w * 0.62, y1)])
        for _ in range(int((y1 - y0) / 60)):
            gy = rnd.uniform(y0 + 10, y1 - 20)
            gx = x + rnd.uniform(6, w * 0.5)
            grain.append([(gx, gy), (gx + rnd.uniform(-1, 1), gy + rnd.uniform(8, 16))])
        x += w
        k += 1
    L.append(polys(f"{id}-shade", shades, shade))
    L.append(strokes(f"{id}-grain", grain, w=1.8, color=FINE, taper=(2, 2)))
    return L


def ground(id, y, x0=-200, x1=SET_W + 200, bottom=SET_H + 200, fill=CREAM_DK, ticks=60, seed=0):
    L = [shape(id, [(x0, y), (x1, y + 10), (x1, bottom), (x0, bottom)], fill, w=3.4, rough=1.5)]
    rnd = random.Random(seed)
    segs = []
    for _ in range(ticks):
        gx, gy = rnd.uniform(x0, x1), rnd.uniform(y + 20, bottom - 20)
        segs.append([(gx, gy), (gx + rnd.uniform(6, 12), gy + rnd.uniform(-1, 1))])
    L.append(strokes(f"{id}-ticks", segs, w=1.8, color=FINE, taper=(2, 2)))
    return L


def flame(id, x, y, s=1.0, frames=4, seed=0, until=4.0, fps=8):
    """A hand-drawn flame that boils at `fps` (drawings swapped on holds)."""
    rnd = random.Random(seed)
    shapes = []
    for f in range(frames):
        pts = [(x - 26 * s, y)]
        for k in range(5):
            a = k / 4
            pts.append((x - 26 * s + 52 * s * a + rnd.uniform(-6, 6) * s, y - (60 + 50 * math.sin(math.pi * a) + rnd.uniform(-14, 22)) * s))
        pts.append((x + 26 * s, y))
        pts.append((x, y + 8 * s))
        shapes.append(pts)
    keys = []
    t = 0.0
    i = 0
    while t <= until:
        keys.append({"at": round(t, 3), "v": {"points": [r2(p) for p in shapes[i % frames]], "closed": True}, "ease": "hold"})
        t += 1 / fps
        i += 1
    outer = {"id": id, "kind": "shape", "shape": "path", "d": {"keys": keys}, "fill": GOLD,
             "stroke": {"color": RED, "width": 3, "ink": {"seed": seed, "fixed": True, "taper": [0, 0]}}}
    inner_keys = [{"at": k["at"], "v": {"points": [r2((x + (px - x) * 0.55, y + (py - y) * 0.55)) for px, py in shapes[(j + 1) % frames]], "closed": True}, "ease": "hold"} for j, k in enumerate(keys)]
    inner = {"id": f"{id}-in", "kind": "shape", "shape": "path", "d": {"keys": inner_keys}, "fill": CREAM}
    return [outer, inner]


def thatch_house(id, x, y, w=150, h=110, lit=NIGHT_LIT, shade=NIGHT_SHADE, roof=GOLD, roof_dk=GOLD_DK, seed=0):
    """A log house with a steep thatched roof and deep eaves."""
    L = []
    walls = rect(x, y - h, x + w, y)
    L.append(shape(f"{id}-wall", walls, lit, w=3.0, rough=0.8))
    L.append(polys(f"{id}-wsh", [[(x + w * 0.6, y - h), (x + w, y - h), (x + w, y), (x + w * 0.6 - 20, y)]], shade))
    L.append(strokes(f"{id}-logs", [[(x, y - h + k * 18), (x + w, y - h + k * 18 + 2)] for k in range(1, int(h / 18))], w=1.8, color=FINE, taper=(0, 0)))
    L.append(shape(f"{id}-door", rect(x + w * 0.2, y - h * 0.62, x + w * 0.42, y), INK, w=2.0, rough=0.4))
    roofp = [(x - 22, y - h + 6), (x + w * 0.5, y - h - w * 0.75), (x + w + 22, y - h + 6), (x + w + 10, y - h + 20), (x - 10, y - h + 20)]
    L.append(shape(f"{id}-roof", roofp, roof, w=3.2, rough=1.0, smooth=0.2))
    L.append(polys(f"{id}-rsh", [[(x + w * 0.5, y - h - w * 0.75), (x + w + 22, y - h + 6), (x + w + 10, y - h + 20), (x + w * 0.5 + 6, y - h + 20)]], roof_dk))
    L.append(strokes(f"{id}-straw", clip_segs(dash_rows(x - 20, x + w + 20, y - h - w * 0.75, y - h + 18, gap=10, dash=9, slant=100, seed=seed), roofp), w=1.6, color=INK, taper=(2, 2)))
    L.append(line(f"{id}-eave", [(x - 22, y - h + 8), (x + w + 22, y - h + 8)], w=3.4, taper=(2, 2)))
    return L


# --------------------------------------------------------------------- sets ----

def yard(id="yard"):
    """Olga's courtyard in Kyiv, by day: the hall, the palisade and a fresh pit."""
    sky_ = sky(f"{id}-sky", -300, 1100) + [cloud(f"{id}-cl1", 300, 280, 1.1), cloud(f"{id}-cl2", 1100, 420, 0.8)]
    back = []
    # The hall (terem): a tall plank house with a steep roof and a carved ridge.
    hx, hy = 820, 980
    hall = rect(hx, hy - 420, hx + 520, hy)
    back.append(shape(f"{id}-hall", hall, GOLD, w=3.6, rough=1.0))
    seams, grain = planks(hx, hy - 420, hx + 520, hy, 44, vertical=False, seed=4)
    back.append(polys(f"{id}-hallsh", [[(hx + 300, hy - 420), (hx + 520, hy - 420), (hx + 520, hy), (hx + 230, hy)]], SHADOW_DAY))
    back.append(strokes(f"{id}-hallseams", seams, w=2.0, color=INK, taper=(0, 0), pressure=0.2))
    back.append(strokes(f"{id}-hallgrain", grain, w=1.6, color=FINE))
    roof = [(hx - 40, hy - 400), (hx + 260, hy - 700), (hx + 560, hy - 400), (hx + 540, hy - 376), (hx - 20, hy - 376)]
    back.append(shape(f"{id}-roof", roof, RED, w=3.8, smooth=0.1))
    back.append(polys(f"{id}-roofsh", [[(hx + 260, hy - 700), (hx + 560, hy - 400), (hx + 540, hy - 376), (hx + 262, hy - 376)]], RED_DK))
    back.append(strokes(f"{id}-shingles", clip_segs(dash_rows(hx - 40, hx + 560, hy - 700, hy - 380, gap=18, dash=14, slant=0, seed=5), roof), w=1.8, color=INK, taper=(2, 2)))
    back.append(shape(f"{id}-ridge", [(hx + 230, hy - 690), (hx + 260, hy - 760), (hx + 290, hy - 690)], GOLD, w=3.0, smooth=0.0))
    back.append(sparkle(f"{id}-ridgeglint", hx + 260, hy - 744, r=12))
    for k, wx in enumerate([hx + 70, hx + 230, hx + 390]):
        back.append(shape(f"{id}-win{k}", rect(wx, hy - 300, wx + 60, hy - 200), COBALT_DK, w=3.0, rough=0.5))
        back.append(shape(f"{id}-winf{k}", rect(wx - 8, hy - 310, wx + 68, hy - 296), RED, w=2.4, rough=0.3))
    # A pennant on the roof (it flaps: the shot keys it).
    back.append(line(f"{id}-pole", [(hx + 260, hy - 760), (hx + 260, hy - 900)], w=4, taper=(0, 0)))
    back.append(shape(f"{id}-flag", [(hx + 262, hy - 898), (hx + 380, hy - 880), (hx + 262, hy - 850)], RED, w=3, smooth=0.3))
    # The palisade.
    back += log_wall(f"{id}-pal", -200, 920, 820, 1180, GOLD, SHADOW_DAY, seed=6)
    mid = ground(f"{id}-ground", 1170, seed=7)
    # The pit: a long dark trench with a lit far wall and a dirt mound.
    pit = [(220, 1420), (520, 1390), (900, 1400), (1180, 1440), (1150, 1560), (860, 1600), (500, 1610), (240, 1560)]
    mid.append(shape(f"{id}-pit", pit, COBALT_DK, w=4.0, rough=2.0))
    mid.append(polys(f"{id}-pitwall", [[(240, 1430), (520, 1400), (900, 1410), (1170, 1446), (1160, 1480), (880, 1452), (520, 1446), (250, 1476)]], SHADOW_DAY))
    mid.append(strokes(f"{id}-pitcracks", cracks(400, 1440, 60, seed=8, depth=2) + cracks(900, 1450, 50, seed=9, depth=2), w=1.8, color=FINE))
    mound = [(1060, 1320), (1180, 1240), (1320, 1220), (1440, 1280), (1500, 1380), (1100, 1390)]
    mid.append(shape(f"{id}-mound", mound, CREAM_DK, w=3.6, rough=2.0))
    mid.append(polys(f"{id}-moundsh", [[(1320, 1222), (1440, 1280), (1500, 1380), (1330, 1388)]], SHADOW_DAY))
    mid.append(dots(f"{id}-clods", points_in(mound, 30, seed=10), r=3.0, color=FINE))
    for k, (sx_, sy_) in enumerate([(1240, 1250), (1370, 1240)]):
        mid.append(line(f"{id}-shovel{k}", [(sx_, sy_), (sx_ + 20, sy_ - 170)], w=6, color=INK, taper=(0, 2), pressure=0.1))
        mid.append(shape(f"{id}-blade{k}", [(sx_ - 22, sy_ - 4), (sx_ + 20, sy_ - 6), (sx_ + 16, sy_ + 40), (sx_ - 4, sy_ + 52), (sx_ - 20, sy_ + 38)], CREAM, w=2.6, smooth=0.4))
    fore = [shape(f"{id}-fore", [(-200, 2000), (300, 1940), (700, 1980), (1200, 1930), (1700, 1990), (1700, 2600), (-200, 2600)], CREAM_DK, w=3.6, rough=2.5),
            strokes(f"{id}-grass", [[(x, 1980 + (x % 37)), (x + 6, 1950 + (x % 29))] for x in range(-180, 1700, 23)], w=2.2, color=INK, taper=(1, 4))]
    return {"sky": sky_, "back": back, "mid": mid, "fore": fore}


def boat(id, x, y, s=1.0):
    """The Drevlians' boat: a long dugout with a carved horse-head prow."""
    L = []
    hull = [(x - 340 * s, y - 60 * s), (x - 200 * s, y - 30 * s), (x + 200 * s, y - 30 * s), (x + 340 * s, y - 70 * s),
            (x + 300 * s, y + 20 * s), (x + 160 * s, y + 60 * s), (x - 160 * s, y + 60 * s), (x - 300 * s, y + 20 * s)]
    L.append(shape(f"{id}-hull", hull, RED, w=4.0, rough=1.0))
    L.append(polys(f"{id}-hullsh", [[(x - 300 * s, y + 20 * s), (x + 300 * s, y + 20 * s), (x + 160 * s, y + 60 * s), (x - 160 * s, y + 60 * s)]], RED_DK))
    L.append(line(f"{id}-strake", [(x - 320 * s, y - 20 * s), (x, y + 6 * s), (x + 320 * s, y - 24 * s)], w=3.0, color=GOLD, taper=(6, 6)))
    prow = [(x + 330 * s, y - 64 * s), (x + 360 * s, y - 150 * s), (x + 400 * s, y - 170 * s), (x + 420 * s, y - 150 * s), (x + 386 * s, y - 136 * s), (x + 360 * s, y - 60 * s)]
    L.append(shape(f"{id}-prow", prow, GOLD, w=3.4, smooth=0.6))
    L.append(dots(f"{id}-eye", [(x + 396 * s, y - 154 * s)], r=3.6))
    L.append(strokes(f"{id}-grain", [[(x + dx * s, y + 20 * s), (x + (dx + 12) * s, y + 22 * s)] for dx in range(-260, 260, 40)], w=1.6, color=FINE))
    return L


def hall(id="hall"):
    """Olga's hall inside: cool log walls, one window, a shaft of warm light."""
    back = [shape(f"{id}-wall", rect(-200, -200, SET_W + 200, 1500), COBALT_LT, w=0, line=False, rough=0)]
    logs = [[(-200, y), (SET_W + 200, y + 3)] for y in range(-160, 1500, 58)]
    back.append(strokes(f"{id}-logs", logs, w=2.8, color=COBALT_DK, taper=(0, 0), pressure=0.25))
    rnd = random.Random(2)
    back.append(strokes(f"{id}-grain", [[(x, y), (x + rnd.uniform(8, 14), y + 1)] for x, y in [(rnd.uniform(-200, 1700), rnd.uniform(-160, 1500)) for _ in range(160)]], w=1.6, color=COBALT_DK))
    win = rect(160, 220, 400, 560)
    back.append(shape(f"{id}-win", win, CREAM, w=4.0, rough=0.5))
    back.append(strokes(f"{id}-mullions", [[(280, 220), (280, 560)], [(160, 390), (400, 390)]], w=6, color=RED_DK, taper=(0, 0), pressure=0.1))
    back.append(shape(f"{id}-sill", rect(140, 556, 420, 586), RED, w=3.0, rough=0.4))
    # A hanging lamp, its glint.
    back.append(line(f"{id}-chain", [(1080, -200), (1080, 300)], w=2.6, taper=(0, 0)))
    back.append(shape(f"{id}-lamp", [(1040, 300), (1120, 300), (1100, 360), (1060, 360)], GOLD, w=3.0, smooth=0.3))
    back.append(sparkle(f"{id}-lampglint", 1090, 322, r=12))
    mid = [shape(f"{id}-floor", rect(-200, 1480, SET_W + 200, SET_H + 200), COBALT, w=3.4, rough=1.0)]
    seams, grain = planks(-200, 1480, SET_W + 200, SET_H + 200, 70, vertical=False, seed=3)
    mid.append(strokes(f"{id}-floorseams", seams, w=2.4, color=COBALT_DK, taper=(0, 0), pressure=0.2))
    # The shaft: warm light from the window to a patch on the floor.
    shaft = [(160, 220), (400, 220), (1120, 1760), (560, 1880)]
    mid.append(shape(f"{id}-shaft", shaft, GOLD, w=0, line=False, rough=0, transform={"opacity": 0.22}, blend="screen"))
    patch = [(560, 1700), (1120, 1620), (1240, 1880), (640, 1980)]
    mid.append(shape(f"{id}-patch", patch, GOLD, w=0, line=False, rough=1.5, transform={"opacity": 0.75}))
    # Olga's carved chair.
    cx, cy = 820, 1500
    chair = [(cx - 150, cy), (cx - 150, cy - 520), (cx - 110, cy - 600), (cx, cy - 640), (cx + 110, cy - 600), (cx + 150, cy - 520), (cx + 150, cy)]
    mid.append(shape(f"{id}-chair", chair, RED, w=4.0, smooth=0.5))
    mid.append(polys(f"{id}-chairsh", [[(cx + 30, cy - 630), (cx + 150, cy - 520), (cx + 150, cy), (cx + 60, cy)]], RED_DK))
    mid.append(strokes(f"{id}-carve", [[(cx - 60, cy - 560), (cx - 30, cy - 590), (cx, cy - 570), (cx + 30, cy - 590), (cx + 60, cy - 560)],
                                         [(cx - 80, cy - 480), (cx + 80, cy - 480)]], w=3.0, color=GOLD, taper=(3, 3)))
    mid.append(sparkle(f"{id}-chairglint", cx - 40, cy - 590, r=10))
    return {"sky": [], "back": back, "mid": mid, "fore": []}


def bathhouse(id="bath", burning=False):
    """The bathhouse at night: a log cabin with a turf roof and a barred door."""
    sky_ = sky(f"{id}-sky", -300, 1300, night=True) + moon(f"{id}-moon", 1160, 300)
    sky_.append(dots(f"{id}-stars", [(random.Random(5).uniform(0, 1500), random.Random(k).uniform(0, 900)) for k in range(40)], r=2.0, color=CREAM))
    back = [shape(f"{id}-hill", [(-200, 1180), (400, 1080), (900, 1120), (1700, 1060), (1700, 1500), (-200, 1500)], COBALT_DK, w=3.4, rough=2.0)]
    back += [line(f"{id}-tree{k}", [(x, 1110), (x, 900)], w=6, taper=(0, 4)) for k, x in enumerate([80, 240, 1380])]
    back += [shape(f"{id}-pine{k}", [(x - 70, 1060), (x, 840), (x + 70, 1060)], NIGHT_SHADE, w=3.0, smooth=0.0) for k, x in enumerate([80, 240, 1380])]
    mid = ground(f"{id}-ground", 1460, fill=NIGHT_SHADE, seed=6)
    bx, by, bw, bh = 260, 1560, 900, 520
    walls = rect(bx, by - bh, bx + bw, by)
    mid.append(shape(f"{id}-walls", walls, NIGHT_LIT, w=4.0, rough=1.2))
    mid.append(polys(f"{id}-wsh", [[(bx + bw * 0.62, by - bh), (bx + bw, by - bh), (bx + bw, by), (bx + bw * 0.55, by)]], NIGHT_SHADE))
    mid.append(strokes(f"{id}-logs", [[(bx - 10, by - bh + k * 52), (bx + bw + 10, by - bh + k * 52 + 3)] for k in range(1, 10)], w=3.0, color=INK, taper=(0, 0), pressure=0.2))
    mid.append(strokes(f"{id}-loggrain", [[(bx + (k * 97) % bw, by - bh + 20 + (k * 53) % (bh - 40)), (bx + (k * 97) % bw + 12, by - bh + 21 + (k * 53) % (bh - 40))] for k in range(60)], w=1.6, color=FINE))
    roof = [(bx - 70, by - bh + 10), (bx + bw * 0.5, by - bh - 300), (bx + bw + 70, by - bh + 10), (bx + bw + 40, by - bh + 40), (bx - 40, by - bh + 40)]
    mid.append(shape(f"{id}-roof", barbed(roof, depth=8, every=1, seed=7), COBALT, w=3.8, rough=1.0, smooth=0.3))
    mid.append(strokes(f"{id}-turf", clip_segs(dash_rows(bx - 70, bx + bw + 70, by - bh - 300, by - bh + 40, gap=14, dash=10, slant=-70, seed=8), roof), w=1.8, color=INK, taper=(1, 3)))
    door = rect(bx + 330, by - 330, bx + 520, by)
    mid.append(shape(f"{id}-door", door, RED_DK, w=3.6, rough=0.6))
    mid.append(strokes(f"{id}-doorplanks", [[(bx + 330 + k * 38, by - 330), (bx + 330 + k * 38, by)] for k in range(1, 5)], w=2.0, color=INK, taper=(0, 0)))
    mid.append(shape(f"{id}-win", rect(bx + 640, by - 380, bx + 760, by - 300), GOLD, w=3.0, rough=0.4))
    # Steam from the window and the roof: soft cream puffs (the shot drifts them).
    mid.append(shape(f"{id}-steam", blob(bx + 700, by - 440, 70, 40, n=12, lump=0.15, seed=9), CREAM, w=2.4, opacity=0.55))
    fore = [shape(f"{id}-fore", [(-200, 2050), (500, 1990), (1700, 2040), (1700, 2600), (-200, 2600)], NIGHT_SHADE, w=3.4, rough=2.0)]
    return {"sky": sky_, "back": back, "mid": mid, "fore": fore, "door": door, "bar_y": by - 180, "bx": bx, "bw": bw, "by": by, "bh": bh}


def grave(id="grave"):
    """Igor's barrow at dusk: a great mound, the feast table, torches."""
    sky_ = sky(f"{id}-sky", -300, 1300, night=True) + moon(f"{id}-moon", 300, 360, 60)
    sky_.append(shape(f"{id}-dusk", rect(-200, 760, SET_W + 200, 1300), NIGHT_LIT, w=0, line=False, rough=0))
    back = []
    mound = [(-100, 1240), (200, 1000), (600, 860), (1000, 880), (1400, 1040), (1700, 1240)]
    back.append(shape(f"{id}-mound", mound, COBALT, w=4.0, rough=2.5))
    back.append(polys(f"{id}-moundlit", [[(-100, 1240), (200, 1000), (600, 860), (700, 860), (420, 1240)]], NIGHT_LIT))
    back.append(strokes(f"{id}-grass", [[(x, 1100 - 0.2 * abs(x - 750) + (x % 31)), (x + 5, 1086 - 0.2 * abs(x - 750) + (x % 23))] for x in range(0, 1500, 29)], w=2.0, color=INK, taper=(1, 3)))
    back.append(line(f"{id}-post", [(760, 870), (760, 700)], w=9, color=INK, taper=(0, 2), pressure=0.1))
    back.append(shape(f"{id}-postcap", [(730, 710), (790, 710), (760, 660)], GOLD, w=3.0, smooth=0.0))
    mid = ground(f"{id}-ground", 1230, fill=NIGHT_SHADE, seed=11)
    # The feast table: planks on trestles, mead horns, cups, bread.
    tx, ty = 160, 1600
    top = [(tx, ty), (tx + 1180, ty - 20), (tx + 1200, ty + 30), (tx - 10, ty + 50)]
    mid.append(shape(f"{id}-table", top, GOLD, w=3.8, rough=0.8))
    mid.append(polys(f"{id}-tablesh", [[(tx - 10, ty + 30), (tx + 1200, ty + 12), (tx + 1200, ty + 30), (tx - 10, ty + 50)]], GOLD_DK))
    for k, lx in enumerate([tx + 80, tx + 1080]):
        mid.append(shape(f"{id}-trestle{k}", [(lx, ty + 40), (lx + 40, ty + 40), (lx + 70, ty + 260), (lx - 30, ty + 260)], GOLD_DK, w=3.2, smooth=0.0))
    for k, cx in enumerate(range(tx + 120, tx + 1150, 170)):
        mid.append(shape(f"{id}-cup{k}", [(cx - 20, ty - 60), (cx + 20, ty - 60), (cx + 14, ty - 4), (cx - 14, ty - 4)], RED, w=2.8, smooth=0.3))
        mid.append(sparkle(f"{id}-cupglint{k}", cx - 8, ty - 46, r=7))
        hx = cx + 70
        mid.append(shape(f"{id}-horn{k}", [(hx, ty - 8), (hx + 60, ty - 40), (hx + 80, ty - 90), (hx + 66, ty - 92), (hx + 40, ty - 56), (hx + 4, ty - 26)], CREAM, w=2.6, smooth=0.6))
    mid.append(shape(f"{id}-bread", blob(tx + 600, ty - 30, 70, 30, n=12, lump=0.1, seed=12), GOLD, w=3.0))
    mid.append(strokes(f"{id}-breadcuts", [[(tx + 560, ty - 44), (tx + 570, ty - 20)], [(tx + 600, ty - 50), (tx + 610, ty - 22)], [(tx + 640, ty - 46), (tx + 650, ty - 22)]], w=2.2))
    # Torches on poles.
    for k, x in enumerate([60, 1440]):
        mid.append(line(f"{id}-torchpole{k}", [(x, 1700), (x, 1250)], w=8, color=INK, taper=(0, 0), pressure=0.1))
    fore = [shape(f"{id}-fore", [(-200, 2060), (600, 2010), (1700, 2050), (1700, 2600), (-200, 2600)], NIGHT_SHADE, w=3.4, rough=2.0)]
    return {"sky": sky_, "back": back, "mid": mid, "fore": fore, "torches": [(60, 1250), (1440, 1250)]}


def iskorosten(id="isk", dusk=False):
    """Iskorosten on its hill at night: log walls, thatched houses, deep eaves."""
    sky_ = sky(f"{id}-sky", -300, 1300, night=True) + moon(f"{id}-moon", 1180, 260, 64)
    if dusk:
        sky_.append(shape(f"{id}-dusk", rect(-200, 820, SET_W + 200, 1300), NIGHT_LIT, w=0, line=False, rough=0))
    sky_.append(dots(f"{id}-stars", [(random.Random(k * 3).uniform(0, 1500), random.Random(k * 7).uniform(-200, 700)) for k in range(46)], r=2.0, color=CREAM))
    back = []
    hill = [(-200, 1500), (100, 1220), (500, 1100), (1000, 1110), (1400, 1240), (1700, 1500), (1700, 2600), (-200, 2600)]
    back.append(shape(f"{id}-hill", hill, NIGHT_SHADE, w=4.0, rough=2.5))
    back.append(polys(f"{id}-hilllit", [[(-200, 1500), (100, 1220), (500, 1100), (620, 1104), (300, 1400), (60, 1700), (-200, 1800)]], COBALT))
    houses = []
    rnd = random.Random(14)
    spots = [(150, 1180), (330, 1120), (520, 1090), (720, 1080), (900, 1090), (1080, 1120), (1260, 1170), (250, 1040), (620, 1010), (980, 1020)]
    for k, (x, y) in enumerate(spots):
        houses += thatch_house(f"{id}-h{k}", x - 70, y, w=rnd.uniform(120, 170), h=rnd.uniform(80, 110), seed=k)
    back += houses
    wall_y0, wall_y1 = 1160, 1300
    back += log_wall(f"{id}-wall", -200, wall_y0, 1700, wall_y1, NIGHT_LIT, NIGHT_SHADE, log_w=40, seed=15)
    mid = []
    fore = [shape(f"{id}-fore", [(-200, 2000), (700, 1950), (1700, 2010), (1700, 2600), (-200, 2600)], NIGHT_SHADE, w=3.4, rough=2.0),
            strokes(f"{id}-fgrass", [[(x, 1990 + (x % 41)), (x + 6, 1956 + (x % 33))] for x in range(-180, 1700, 21)], w=2.2, color=INK, taper=(1, 4))]
    return {"sky": sky_, "back": back, "mid": mid, "fore": fore, "eaves": [(x + 10, y - 100) for x, y in spots]}


def icon(id="icon"):
    """The closing: a gold icon ground and a round halo."""
    back = [shape(f"{id}-gold", rect(-200, -200, SET_W + 200, SET_H + 200), GOLD, w=0, line=False, rough=0)]
    rnd = random.Random(3)
    back.append(strokes(f"{id}-punch", [[(x, y), (x + 4, y + 1)] for x, y in [(rnd.uniform(-200, 1700), rnd.uniform(-200, 2500)) for _ in range(260)]], w=1.8, color=GOLD_DK))
    arch = [(160, 1900), (160, 700), (300, 380), (750, 240), (1200, 380), (1340, 700), (1340, 1900)]
    back.append(shape(f"{id}-arch", arch, COBALT_DK, w=5.0, smooth=0.6))
    back.append(line(f"{id}-archtrim", arch, w=8, color=RED, taper=(0, 0), pressure=0.1))
    halo = blob(750, 760, 300, 300, n=24, lump=0.0)
    back.append(shape(f"{id}-halo", halo, GOLD, w=5.0))
    back.append(strokes(f"{id}-rays", [[(750 + math.cos(a) * 240, 760 + math.sin(a) * 240), (750 + math.cos(a) * 290, 760 + math.sin(a) * 290)] for a in [i * math.tau / 36 for i in range(36)]], w=2.6, color=GOLD_DK, taper=(2, 2)))
    back.append(sparkle(f"{id}-haloglint", 560, 560, r=18))
    mid = []
    for k, x in enumerate([260, 1240]):
        mid.append(shape(f"{id}-candle{k}", rect(x - 18, 1640, x + 18, 1900), CREAM, w=3.0, rough=0.4))
    return {"sky": [], "back": back, "mid": mid, "fore": [], "candles": [(260, 1640), (1240, 1640)]}


SETS = {"yard": yard, "hall": hall, "bath": bathhouse, "grave": grave, "isk": iskorosten, "icon": icon}
