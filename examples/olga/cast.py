"""The cast of "Olga of Kyiv", drawn in code.

Olga      thin and tall: long hooked nose, cream mourning veil, red-gold brow
          band and big gold temple rings.
Envoy     huge: a slab of a jaw in a black barbed beard, gold teeth, a tall
          bear-fur hat, barrel body in a plaid tunic.
Igor      short and stout: conical gold helmet, enormous black moustache,
          jowls and a double chin.
Mal       lanky prince of the Drevlians: long pale beard, antler crown.
Extras    Kyivan guards (pointed helmets, spears) and Drevlians (fur hats),
          drawn with the same line and fewer marks.

Each lead is a jointed cut-out: a comp per part with its pivot at the
joint, one drawn head per expression (swapped with hold keys).
"""
import math
import random

from kit import *  # noqa: F401,F403
from face import eye, bags, brow, mouth, pores, skin_ticks, stubble, sparkle, teeth_row

LIGHT_SPLIT = 0.12  # slope of the hard diagonal light edge across a face


def matted(id, layers, matte_pts, fill=SKIN):
    """Light and texture inside a shape only: a comp matted by a copy of it."""
    return [shape(f"{id}-m", matte_pts, "#ffffff", w=0, line=False, rough=1.0),
            comp(id, layers, matte={"layer": f"{id}-m"})]


def light_split(id, cx, top, bottom, side=1, width=400, color=SKIN_DK):
    """A hard diagonal shadow over one side (light from the upper left)."""
    x0 = cx + side * 8
    return polys(id, [[(x0, top), (cx + side * width, top), (cx + side * width, bottom), (x0 - side * (bottom - top) * LIGHT_SPLIT, bottom)]], color)


# ----------------------------------------------------------------- Olga ----

def olga_face_pts(jaw=0.0):
    # A long lumpy mass: high temples, cheekbones that bulge, a long jaw
    # and a chin that juts down and forward.
    pts = [(0, -270), (40, -266), (72, -248), (88, -214), (86, -184), (98, -150), (96, -124), (80, -100), (70, -72),
           (52, -44), (30, -18), (14, 2), (2, 10), (-10, 4), (-30, -16), (-54, -44), (-74, -74), (-86, -102),
           (-100, -128), (-102, -154), (-90, -186), (-92, -216), (-76, -250), (-42, -267)]
    return [(x, y + jaw * 34 * max(0.0, min(1.0, (y + 90) / 90))) for x, y in pts]


OLGA_MOODS = ["neutral", "blink", "smug", "anger", "shock", "grief"]


def olga_head(id, mood="neutral"):
    jaw = 0.75 if mood == "shock" else 0.0
    j = jaw * 34
    face = olga_face_pts(jaw)
    L = []
    # Veil behind the head: hugs the crown, falls in folds past the shoulders.
    veil = [(0, -316), (64, -306), (108, -270), (124, -214), (128, -150), (138, -80), (160, -10), (196, 52), (226, 100),
            (150, 112), (60, 96), (0, 100), (-60, 96), (-150, 112), (-226, 100), (-196, 52), (-160, -10), (-138, -80),
            (-128, -150), (-124, -214), (-108, -270), (-64, -306)]
    L.append(shape(f"{id}-veil", veil, CREAM, w=4.0))
    L += matted(f"{id}-veilsh", [light_split(f"{id}-veilsh-s", 30, -330, 120, color=CREAM_DK)], veil)
    L.append(strokes(f"{id}-veilfold", [[(112, -150), (124, -70), (152, 20), (176, 90)], [(-110, -140), (-122, -60), (-150, 30), (-172, 96)],
                                        [(132, -10), (138, 50), (126, 104)], [(-132, 0), (-140, 56), (-124, 106)],
                                        [(170, 40), (190, 96)], [(-176, 46), (-196, 98)]], w=2.4, color=FINE, taper=(10, 10)))
    neck = [(-34, -40), (30, -40), (36, 40), (0, 56), (-38, 40)]
    L.append(shape(f"{id}-neck", neck, SKIN_DK, w=3.2))
    L.append(strokes(f"{id}-neckl", [[(-14, 8), (-4, 30)], [(14, 10), (8, 28)]], w=1.8, color=FINE))
    # Face mass, its shade and light.
    L.append(shape(f"{id}-face", face, SKIN, w=4.2))
    light = [light_split(f"{id}-sh", 18, -300, 60),
             polys(f"{id}-cheek", [[(-96, -146), (-74, -150), (-52, -140), (-62, -132), (-84, -132)]], SKIN_LT),
             polys(f"{id}-brow-lt", [[(-80, -218), (-30, -214), (-34, -206), (-78, -206)]], SKIN_LT),
             polys(f"{id}-jowl", [[(-66, -78 + j), (-40, -64 + j), (-30, -40 + j), (-52, -48 + j)]], SKIN_DK)]
    light += pores(f"{id}-pores", [(-88, -130), (-50, -126), (-46, -90), (-78, -92)], 18, seed=3)
    light += pores(f"{id}-pores2", [(40, -125), (84, -132), (72, -88), (40, -86)], 14, seed=4)
    light += skin_ticks(f"{id}-ticks", [(-70, -240), (60, -240), (60, -222), (-70, -222)], 6, 7, seed=5, angle=0)
    L += matted(f"{id}-light", light, face)
    # Cheekbone and jowl structure: lines that follow the skull.
    L.append(strokes(f"{id}-bones", [[(-92, -122), (-80, -110), (-62, -108)], [(88, -118), (74, -106), (58, -104)],
                                     [(-40, -110), (-52, -86 + j * 0.3), (-50, -58 + j * 0.6)], [(40, -110), (52, -86 + j * 0.3), (50, -58 + j * 0.6)]],
                     w=2.2, color=FINE, taper=(6, 6)))
    # Eyes crowded in toward a long hooked nose.
    look = {"smug": (0.6, 0.1), "anger": (0.2, 0.0), "grief": (0.0, 0.7)}.get(mood, (0.3, 0.0))
    for s in (-1, 1):
        L += eye(f"{id}-eye{s}", s * 30, -164, w=34, h=15, look=look, mood=mood, side=s, tilt=-6, lid=0.3, lash=5.6)
        L += brow(f"{id}-brow{s}", s * 34, -194, w=40, mood=mood, side=s, thick=6)
        if mood != "shock":
            L += bags(f"{id}-bags{s}", s * 30, -150, w=28, n=2)
    nose = [(-6, -182), (6, -180), (10, -150), (20, -122), (30, -100), (24, -84 + j * 0.15), (6, -80 + j * 0.15),
            (-12, -84), (-22, -94), (-18, -104), (-8, -112), (-6, -146)]
    L.append(shape(f"{id}-nose", nose, SKIN, w=3.8, smooth=0.9))
    L.append(polys(f"{id}-noseshade", [[(8, -150), (18, -122), (28, -100), (22, -86), (10, -104)]], SKIN_DK))
    L.append(polys(f"{id}-nosehl", [[(0, -160), (4, -132), (0, -128), (-3, -150)]], SKIN_LT))
    L.append(shape(f"{id}-nostril", [(-14, -92), (-6, -96), (2, -90), (-6, -86)], INK, w=0, line=False, rough=0))
    L.append(line(f"{id}-wing", [(-20, -104), (-26, -94), (-18, -86)], w=2.2, taper=(3, 3)))
    L.append(polys(f"{id}-noseshadow", [[(-14, -80), (14, -78), (4, -72), (-10, -73)]], SKIN_DK))
    # Mouth: thin, red-lipped, under the overhanging nose.
    m = mood if mood != "blink" else "neutral"
    if m in ("neutral", "smug", "anger", "grief"):
        lip_up = {"smug": 4, "anger": -2, "grief": -4}.get(m, 0)
        L.append(shape(f"{id}-lips", [(-17, -58), (-6, -63), (0, -61), (6, -63), (17, -58 - lip_up), (6, -53), (-6, -53)], RED, w=1.6, rough=0.2, smooth=0.7))
    L += mouth(f"{id}-mouth", 0, -58, w=34, mood=m, jaw=jaw)
    L.append(line(f"{id}-chin", [(-12, -26 + j), (0, -20 + j), (12, -26 + j)], w=1.8, color=FINE, taper=(4, 4)))
    # Veil front: the cap over the crown and the brow band.
    cap = [(0, -322), (68, -310), (110, -270), (112, -238), (60, -246), (0, -250), (-60, -246), (-112, -238), (-110, -270), (-68, -310)]
    L.append(shape(f"{id}-cap", cap, CREAM, w=4.0))
    L += matted(f"{id}-capsh", [light_split(f"{id}-capsh-s", 30, -340, -200, color=CREAM_DK)], cap)
    L.append(strokes(f"{id}-capfold", [[(-40, -304), (-30, -272)], [(20, -310), (28, -276)], [(70, -294), (72, -266)]], w=2.0, color=FINE, taper=(5, 5)))
    band = [(-114, -246), (-60, -258), (0, -262), (60, -258), (114, -246), (108, -224), (60, -234), (0, -238), (-60, -234), (-108, -224)]
    L.append(shape(f"{id}-band", band, RED, w=3.4))
    for k, x in enumerate([-80, -40, 0, 40, 80]):
        y = -245 - 6 * math.cos(x / 114 * math.pi / 2)
        L.append(shape(f"{id}-med{k}", [(x + 8 * math.cos(a), y + 7 * math.sin(a)) for a in [i * math.tau / 8 for i in range(8)]], GOLD, w=1.6, rough=0.2))
        L.append(dots(f"{id}-medc{k}", [(x, y)], r=2.0, color=RED_DK))
    L.append(dots(f"{id}-banddots", [(-60, -248), (-20, -252), (20, -252), (60, -248)], r=1.6, color=GOLD))
    # Temple rings (kolty): a chain and a big gold crescent pendant each side.
    for s in (-1, 1):
        x = s * 108
        L.append(line(f"{id}-chain{s}", [(x, -232), (x + s * 3, -204), (x + s * 2, -180)], w=2.2, taper=(2, 2)))
        lun = [(x - 24, -180), (x, -172), (x + 24, -180), (x + 22, -158), (x + 9, -141), (x, -137), (x - 9, -141), (x - 22, -158)]
        L.append(shape(f"{id}-kolt{s}", lun, GOLD, w=2.8, rough=0.4))
        L.append(polys(f"{id}-koltsh{s}", [[(x + 4, -174), (x + 22, -178), (x + 20, -158), (x + 8, -143)]], GOLD_DK))
        L.append(dots(f"{id}-gran{s}", [(x - 13, -163), (x - 5, -155), (x + 4, -155), (x + 12, -163), (x, -147)], r=1.9, color=RED_DK))
    L.append(sparkle(f"{id}-glint", -100, -168, r=11))
    return L


def olga_hand(id, pose="rest", lit=True, side=-1):
    return hand(id, pose, lit, side, ring=True, size=1.0)


def olga_body(id):
    """Olga's robe, from the neck (0, 0) to the floor (0, 560)."""
    L = []
    robe = [(-50, -10), (50, -10), (104, 24), (112, 120), (104, 260), (124, 420), (156, 556), (0, 566), (-156, 556), (-124, 420), (-104, 260), (-112, 120), (-104, 24)]
    L.append(shape(f"{id}-robe", robe, COBALT, w=4.2))
    L += matted(f"{id}-robesh", [light_split(f"{id}-robesh-s", 10, -20, 600, color=COBALT_DK)], robe)
    # Gold trim down the front and round the hem.
    L.append(shape(f"{id}-trim", [(-10, 0), (10, 0), (18, 560), (-18, 560)], GOLD, w=2.6, smooth=0.3))
    L.append(shape(f"{id}-hem", [(-150, 528), (-60, 540), (0, 542), (60, 540), (150, 528), (152, 556), (0, 566), (-152, 556)], GOLD, w=2.6))
    # Red brocade medallions with gold scrolls on the robe.
    meds = []
    for k, (x, y) in enumerate([(-50, 110), (52, 160), (-60, 260), (62, 310), (-74, 410), (78, 450), (-40, 480)]):
        meds.append(shape(f"{id}-bm{k}", [(x + 15 * math.cos(a), y + 15 * math.sin(a)) for a in [i * math.tau / 10 for i in range(10)]], RED, w=2.0, rough=0.4))
        meds.append(strokes(f"{id}-bs{k}", [[(x - 9, y), (x - 4, y - 7), (x + 3, y - 5), (x + 4, y + 1)], [(x + 9, y), (x + 4, y + 7), (x - 3, y + 5), (x - 4, y - 1)]], w=1.6, color=GOLD, taper=(2, 2)))
    L += meds
    # Belt.
    L.append(shape(f"{id}-belt", [(-108, 150), (108, 150), (106, 172), (-106, 172)], RED, w=3.0, smooth=0.4))
    L.append(shape(f"{id}-buckle", [(-12, 146), (12, 146), (14, 176), (-14, 176)], GOLD, w=2.4, smooth=0.4))
    # Fold ticks on the skirt.
    L.append(strokes(f"{id}-folds", [[(-40, 200), (-50, 320), (-62, 450)], [(40, 220), (52, 340), (66, 470)], [(-8, 300), (-10, 420)]], w=2.2, color=COBALT_DK, taper=(10, 10)))
    return L


def sleeve(id, length=250, width=56, fill=COBALT, cuff=GOLD, shade=COBALT_DK):
    """An arm hanging from the shoulder at (0, 0), wrist at (0, length)."""
    pts = [(-width * 0.45, 0), (width * 0.45, 4), (width * 0.55, length * 0.5), (width * 0.6, length - 14), (-width * 0.55, length - 10), (-width * 0.5, length * 0.5)]
    L = [shape(f"{id}-sleeve", pts, fill, w=3.8)]
    L += matted(f"{id}-sh", [light_split(f"{id}-sh-s", 2, -20, length + 20, color=shade)], pts)
    L.append(strokes(f"{id}-folds", [[(-6, length * 0.25), (-10, length * 0.55)], [(10, length * 0.45), (12, length * 0.75)]], w=2.0, color=shade, taper=(6, 6)))
    if cuff:
        L.append(shape(f"{id}-cuff", [(-width * 0.58, length - 24), (width * 0.62, length - 28), (width * 0.66, length - 4), (-width * 0.6, length)], cuff, w=2.8, smooth=0.4))
    return L


# ------------------------------------------------------------------ hands ----

def finger(id, base, angle, length, width, bend=12, fill=SKIN, nail=True, joints=True):
    """A padded finger from `base` at `angle` degrees (0 = down), curling by
    `bend` degrees a joint; a short dark nail at the tip."""
    segs = [0.45, 0.32, 0.23]
    pts = [base]
    a = angle
    for k, s in enumerate(segs):
        a += bend if k else bend * 0.4
        r = math.radians(a)
        p = pts[-1]
        pts.append((p[0] - math.sin(r) * length * s, p[1] + math.cos(r) * length * s))
    left, right = [], []
    for k, p in enumerate(pts):
        if k == 0:
            q = pts[1]
        else:
            q = p
            p = pts[k - 1]
        dx, dy = q[0] - p[0], q[1] - p[1]
        l = math.hypot(dx, dy) or 1
        nx, ny = -dy / l, dx / l
        w = width * (0.5 if k in (1, 2) else 0.56)  # padded between joints, narrower at them
        c = pts[k]
        left.append((c[0] + nx * w, c[1] + ny * w))
        right.append((c[0] - nx * w, c[1] - ny * w))
    tip = pts[-1]
    r = math.radians(a)
    tipc = (tip[0] - math.sin(r) * width * 0.45, tip[1] + math.cos(r) * width * 0.45)
    outline = left + [tipc] + right[::-1]
    L = [shape(id, outline, fill, w=2.4, rough=0.2)]
    if joints:
        segs_ = []
        for k in (1, 2):
            c = pts[k]
            dx, dy = pts[k + 1][0] - pts[k - 1][0], pts[k + 1][1] - pts[k - 1][1]
            l = math.hypot(dx, dy) or 1
            nx, ny = -dy / l, dx / l
            segs_.append([(c[0] + nx * width * 0.25, c[1] + ny * width * 0.25), (c[0] - nx * width * 0.1, c[1] - ny * width * 0.1)])
            segs_.append([(c[0] + nx * width * 0.05 + dx / l * 3, c[1] + ny * width * 0.05 + dy / l * 3), (c[0] - nx * width * 0.25 + dx / l * 3, c[1] - ny * width * 0.25 + dy / l * 3)])
        L.append(strokes(f"{id}-crease", segs_, w=1.4, color=FINE, taper=(1.5, 1.5)))
    if nail:
        # Stops short of the fingertip.
        c = lerp(pts[-2], pts[-1], 0.78)
        dx, dy = pts[-1][0] - pts[-2][0], pts[-1][1] - pts[-2][1]
        l = math.hypot(dx, dy) or 1
        ux, uy = dx / l, dy / l
        nx, ny = -uy, ux
        nw, nh = width * 0.3, width * 0.36
        nailp = [(c[0] + nx * nw - ux * nh, c[1] + ny * nw - uy * nh), (c[0] + nx * nw * 0.9 + ux * nh * 0.6, c[1] + ny * nw * 0.9 + uy * nh * 0.6),
                 (c[0] - nx * nw * 0.9 + ux * nh * 0.6, c[1] - ny * nw * 0.9 + uy * nh * 0.6), (c[0] - nx * nw - ux * nh, c[1] - ny * nw - uy * nh)]
        L.append(shape(f"{id}-nail", nailp, RED_DK, w=1.0, rough=0, smooth=0.6))
    return L


def hand(id, pose="rest", lit=True, side=-1, ring=False, size=1.0, fill=None):
    """A hand at the wrist (0, 0), fingers pointing down. side −1 is a left
    hand (thumb toward +x as seen), 1 a right hand. Poses: rest, open, fist,
    point, grip (holding a staff: fingers wrapped round x = 0)."""
    fill = fill or (SKIN if lit else SKIN_DK)
    k = size
    L = []
    palm = [(-20 * k, 0), (20 * k, 0), (26 * k, 30 * k), (22 * k, 52 * k), (-22 * k, 54 * k), (-26 * k, 28 * k)]
    palm = [(x * -side, y) for x, y in palm]
    L.append(shape(f"{id}-palm", palm, fill, w=2.8, rough=0.3))
    fingers = []
    if pose in ("rest", "open", "point"):
        spread = 9 if pose == "open" else 3
        bend = {"rest": 14, "open": 4, "point": 40}[pose]
        for i, x in enumerate([-15, -5, 5, 15]):
            b = bend if not (pose == "point" and i == 1) else 2
            ln = [34, 40, 38, 30][i] * k
            fingers += finger(f"{id}-f{i}", (x * k * -side, 50 * k), (x / 15) * spread * -side, ln, 10.5 * k, bend=b * -side * 0.5, fill=fill)
    elif pose in ("fist", "grip"):
        for i, y in enumerate([44, 54, 63, 71]):
            fingers.append(shape(f"{id}-k{i}", [(-24 * k * -side, (y - 6) * k), (6 * k * -side, (y - 7) * k), (12 * k * -side, (y + 1) * k), (6 * k * -side, (y + 8) * k), (-24 * k * -side, (y + 7) * k)], fill, w=2.2, rough=0.2))
        fingers.append(strokes(f"{id}-knuckles", [[(-10 * k * -side, 40 * k), (-6 * k * -side, 44 * k)], [(-2 * k * -side, 41 * k), (2 * k * -side, 45 * k)]], w=1.4, color=FINE))
    L += fingers
    # Thumb.
    L += finger(f"{id}-thumb", (24 * k * -side, 14 * k), -40 * -side, 34 * k, 12 * k, bend=10 * side, fill=fill)
    # Knuckle bumps and wrist crease.
    L.append(strokes(f"{id}-bumps", [[(-14 * k, 44 * k), (-9 * k, 42 * k)], [(-3 * k, 45 * k), (3 * k, 43 * k)], [(8 * k, 44 * k), (13 * k, 42 * k)]], w=1.4, color=FINE))
    if ring:
        x = (-5 if pose != "fist" else -2) * k * -side
        L.append(shape(f"{id}-ring", [(x - 7 * k, 58 * k), (x + 7 * k, 56 * k), (x + 7 * k, 63 * k), (x - 7 * k, 65 * k)], GOLD, w=1.6, rough=0, smooth=0.5))
    return L


def head_swaps(id, make, moods, keys=None):
    """One comp per expression; `keys` [(t, mood), …] swaps them on holds."""
    layers = []
    for m in moods:
        c = comp(f"{id}-{m}", make(f"{id}-{m}", m))
        if keys is not None:
            ks = [{"at": 0, "v": 1 if keys[0][1] == m else 0}]
            for t, mm in keys:
                ks.append({"at": round(t, 3), "v": 1 if mm == m else 0, "ease": "hold"})
            c["transform"]["opacity"] = {"keys": ks}
        elif m != moods[0]:
            c["hidden"] = True
        layers.append(c)
    return layers


def olga(id, pos, scale=1.0, mood_keys=None, moods=None, arm_l=None, arm_r=None, hand_l="rest", hand_r="rest", **extra):
    """Olga standing, feet at `pos`. Arms: rotation keys (degrees) or None."""
    moods = moods or OLGA_MOODS
    neck = (0, -600)
    parts = []
    parts.append(comp(f"{id}-body", olga_body(f"{id}-b"), pos=(0, -600)))
    for s, rot, hp in ((-1, arm_l, hand_l), (1, arm_r, hand_r)):
        arm = comp(f"{id}-arm{s}", sleeve(f"{id}-a{s}", 240, 54) + [comp(f"{id}-hand{s}", hand(f"{id}-h{s}", hp, lit=s < 0, side=s, ring=s < 0), pos=(0, 232))],
                   pos=(s * 104, -560))
        if rot is not None:
            arm["transform"]["rotation"] = rot
        parts.insert(0 if s > 0 else len(parts), arm)
    head = comp(f"{id}-head", head_swaps(f"{id}-hd", olga_head, moods, mood_keys), pos=(0, -596), anchor=(0, 0))
    parts.append(head)
    return comp(id, parts, pos=pos, **({"transform": {"position": r2(pos), "scale": [scale, scale]}} if scale != 1 else {}), **extra)


def jawed(pts, jaw, top=-130, amount=46):
    """Points below `top` dropped by the open jaw (more the lower they sit)."""
    return [(x, y + jaw * amount * max(0.0, min(1.0, (y - top) / (0 - top + 1e-9) + 0.25))) if y > top else (x, y) for x, y in pts]


# ---------------------------------------------------------------- the envoy ----

ENVOY_MOODS = ["neutral", "blink", "joy", "smug", "shock", "fear"]


def envoy_head(id, mood="neutral"):
    """The Drevlian envoy: a slab of a jaw in a black barbed beard, a tall
    bear-fur hat, little eyes, a flat bulb of a nose, gold teeth."""
    jaw = {"shock": 0.9, "fear": 0.45, "joy": 0.35}.get(mood, 0.0)
    L = []
    face = [(0, -300), (60, -298), (96, -270), (112, -226), (126, -178), (146, -126), (150, -80), (138, -40), (106, -12), (50, 4),
            (0, 8), (-50, 4), (-106, -12), (-138, -40), (-150, -80), (-146, -126), (-126, -178), (-112, -226), (-96, -270), (-60, -298)]
    face = jawed(face, jaw)
    for s in (-1, 1):  # big ears
        ear = [(s * 112, -222), (s * 140, -236), (s * 152, -204), (s * 144, -172), (s * 124, -170)]
        L.append(shape(f"{id}-ear{s}", ear, SKIN if s < 0 else SKIN_DK, w=3.6))
        L.append(line(f"{id}-earl{s}", [(s * 130, -222), (s * 142, -206), (s * 132, -186)], w=2.0, color=FINE, taper=(4, 4)))
    L.append(shape(f"{id}-face", face, SKIN, w=4.4))
    light = [light_split(f"{id}-sh", 26, -320, 80),
             polys(f"{id}-cheek", [[(-120, -150), (-96, -164), (-74, -150), (-96, -136)]], SKIN_LT),
             polys(f"{id}-fore", [[(-80, -280), (-20, -286), (-24, -270), (-78, -264)]], SKIN_LT)]
    light += pores(f"{id}-pores", [(-130, -150), (-70, -150), (-70, -110), (-130, -110)], 22, seed=11)
    light += skin_ticks(f"{id}-wr", [(-60, -262), (60, -262), (60, -244), (-60, -244)], 8, 10, seed=12, angle=0)
    L += matted(f"{id}-light", light, face)
    # Beard: solid black, barbed; it drops with the jaw.
    beard = [(-132, -150), (-104, -120), (-70, -106), (-40, -96), (0, -92), (40, -96), (70, -106), (104, -120), (132, -150),
             (150, -96), (146, -40), (124, 10), (84, 48), (40, 66), (0, 74), (-40, 66), (-84, 48), (-124, 10), (-146, -40), (-150, -96)]
    beard = jawed(beard, jaw, top=-110, amount=46)
    L.append(shape(f"{id}-beard", barbed(beard, depth=9, every=1, seed=seed_of(id)), INK, w=2.0, rough=0.6, smooth=0.5))
    L.append(strokes(f"{id}-beardhair", ticks_in(beard, 26, 12, angle=95, seed=13, jitter=20), w=1.6, color=COBALT_DK, taper=(3, 3)))
    # Moustache over the mouth.
    m = mood if mood != "blink" else "neutral"
    open_mouth = m in ("joy", "shock", "fear")
    mo = 4 if open_mouth else 0
    mous = [(-70, -112), (-40, -128 - mo), (0, -122 - mo), (40, -128 - mo), (70, -112), (58, -100), (30, -110 - mo), (0, -104 - mo), (-30, -110 - mo), (-58, -100)]
    # Mouth: a gap in the beard, wide enough to clear it when it grins.
    mood_mouth = {"neutral": "neutral", "smug": "smug", "joy": "grin", "shock": "shock", "fear": "fear", "anger": "anger"}.get(m, "neutral")
    if mood_mouth in ("neutral", "smug", "anger"):
        L.append(shape(f"{id}-lipgap", [(-30, -98), (30, -98), (26, -80), (-26, -80)], SKIN_DK, w=0, line=False, rough=0.5))
    L += mouth(f"{id}-mouth", 0, -92 if not open_mouth else -86, w=96 if open_mouth else 50, mood=mood_mouth, jaw=jaw,
               gold=[1, 2, 6] if open_mouth else None)
    L.append(shape(f"{id}-mous", barbed(mous, depth=6, every=1, seed=seed_of(id) + 1), INK, w=1.6, rough=0.4, smooth=0.6))
    # Nose: a flat bulb with wide nostrils.
    nose = [(-30, -190), (-14, -204), (14, -204), (32, -186), (44, -158), (32, -136), (10, -132), (-10, -134), (-34, -138), (-44, -158)]
    L.append(shape(f"{id}-nose", nose, SKIN, w=4.0))
    L.append(polys(f"{id}-nosesh", [[(10, -200), (34, -184), (44, -158), (30, -138), (14, -140)]], SKIN_DK))
    L.append(polys(f"{id}-nosehl", [[(-24, -176), (-10, -182), (-8, -168), (-22, -164)]], SKIN_LT))
    L.append(dots(f"{id}-nostrils", [(-18, -142), (16, -142)], r=4.2))
    # Little eyes high up, under bushy barbed brows.
    look = {"joy": (0.0, 0.0), "smug": (-0.5, 0.0), "fear": (0.0, -0.6)}.get(m, (0.2, 0.0))
    for s in (-1, 1):
        L += eye(f"{id}-eye{s}", s * 44, -212, w=26, h=11, look=look, mood=mood if mood != "joy" else "smug", side=s, tilt=4, lid=0.35, lash=5.0)
        L += brow(f"{id}-brow{s}", s * 46, -238, w=50, mood={"joy": "joy"}.get(m, m), side=s, thick=11, barbs=True)
        if mood not in ("shock",):
            L += bags(f"{id}-bags{s}", s * 46, -198, w=26, n=3)
    # Bear-fur hat: tall, gold-brown, rows of dashes, barbed edge.
    hat = [(-112, -238), (-118, -300), (-104, -380), (-70, -432), (0, -452), (70, -430), (104, -378), (118, -300), (112, -238), (60, -252), (0, -256), (-60, -252)]
    L.append(shape(f"{id}-hat", barbed(hat, depth=7, every=1, seed=seed_of(id) + 2), GOLD, w=3.6, rough=0.4, smooth=0.6))
    L += matted(f"{id}-hatsh", [light_split(f"{id}-hatsh-s", 24, -470, -220, color=GOLD_DK),
                                strokes(f"{id}-fur", dash_rows(-120, 120, -440, -250, gap=13, dash=9, slant=70, seed=14), w=2.0, color=INK, taper=(2, 3))], hat)
    band = [(-116, -270), (0, -282), (116, -270), (114, -238), (0, -250), (-114, -238)]
    L.append(shape(f"{id}-hatband", barbed(band, depth=5, every=1, seed=seed_of(id) + 3), GOLD_DK, w=3.2, rough=0.4, smooth=0.6))
    L.append(strokes(f"{id}-bandfur", dash_rows(-110, 110, -270, -246, gap=8, dash=7, slant=100, seed=15), w=1.8, color=INK, taper=(2, 2)))
    return L


def envoy_body(id, tunic=RED, plaid=CREAM):
    """A barrel of a man, from the neck (0, 0) to the floor (0, 470)."""
    L = []
    legs = []
    for s in (-1, 1):
        leg = [(s * 30, 300), (s * 92, 300), (s * 96, 440), (s * 40, 440)]
        legs.append(shape(f"{id}-leg{s}", leg, CREAM, w=3.4, smooth=0.4))
        legs.append(strokes(f"{id}-wrap{s}", [[(s * 32, 320 + k * 26), (s * 94, 340 + k * 26)] for k in range(5)] + [[(s * 94, 320 + k * 26), (s * 32, 340 + k * 26)] for k in range(5)], w=2.0, color=RED_DK, taper=(3, 3)))
        boot = [(s * 26, 430), (s * 100, 430), (s * (100 + 30), 468), (s * 22, 470)]
        legs.append(shape(f"{id}-boot{s}", boot, INK, w=2.4, smooth=0.5))
    L += legs
    body = [(-100, 0), (100, 0), (196, 60), (236, 170), (232, 260), (196, 324), (110, 344), (0, 348), (-110, 344), (-196, 324), (-232, 260), (-236, 170), (-196, 60)]
    L.append(shape(f"{id}-tunic", body, tunic, w=4.4))
    plaid_segs = [[(x, -10), (x + 6, 350)] for x in range(-200, 210, 34)] + [[(-210, y), (210, y + 4)] for y in range(30, 340, 34)]
    L += matted(f"{id}-plaid", [light_split(f"{id}-sh", 10, -20, 380, color=RED_DK),
                                strokes(f"{id}-pl", plaid_segs, w=3.6, color=plaid, taper=(0, 0), pressure=0.15),
                                strokes(f"{id}-pl2", [[(x + 9, -10), (x + 15, 350)] for x in range(-200, 210, 68)], w=1.4, color=INK, taper=(0, 0), pressure=0.1)], body)
    L.append(shape(f"{id}-belt", [(-232, 214), (0, 226), (232, 214), (230, 242), (0, 254), (-230, 242)], INK, w=2.4, smooth=0.5))
    L.append(shape(f"{id}-buckle", [(-18, 212), (18, 212), (18, 248), (-18, 248)], GOLD, w=2.4, smooth=0.3))
    # Fur cloak over the shoulders.
    cloak = [(-180, 40), (-100, -10), (0, 6), (100, -10), (180, 40), (190, 110), (150, 96), (110, 120), (60, 92), (0, 110), (-60, 92), (-110, 120), (-150, 96), (-190, 110)]
    L.append(shape(f"{id}-cloak", barbed(cloak, depth=7, every=1, seed=seed_of(id)), GOLD, w=3.4, rough=0.5, smooth=0.6))
    L += matted(f"{id}-cloaksh", [light_split(f"{id}-cloaksh-s", 20, -20, 140, color=GOLD_DK),
                                  strokes(f"{id}-cfur", dash_rows(-190, 190, 0, 120, gap=12, dash=8, slant=80, seed=16), w=1.8, color=INK, taper=(2, 2))], cloak)
    return L


def envoy(id, pos, scale=1.0, mood_keys=None, arm_l=None, arm_r=None, hand_l="fist", hand_r="fist", tunic=RED, **extra):
    neck_y = -470
    parts = [comp(f"{id}-body", envoy_body(f"{id}-b", tunic=tunic), pos=(0, neck_y))]
    for s, rot, hp in ((-1, arm_l, hand_l), (1, arm_r, hand_r)):
        arm = comp(f"{id}-arm{s}", sleeve(f"{id}-a{s}", 200, 82, fill=tunic, cuff=GOLD, shade=RED_DK)
                   + [comp(f"{id}-hand{s}", hand(f"{id}-h{s}", hp, lit=s < 0, side=s, size=1.35), pos=(0, 192))],
                   pos=(s * 200, neck_y + 50))
        if rot is not None:
            arm["transform"]["rotation"] = rot
        parts.insert(0 if s > 0 else len(parts), arm)
    parts.append(comp(f"{id}-head", head_swaps(f"{id}-hd", envoy_head, ENVOY_MOODS, mood_keys), pos=(0, neck_y + 10)))
    t = {"position": r2(pos)}
    if scale != 1:
        t["scale"] = [scale, scale]
    return {"id": id, "kind": "comp", "transform": t, "layers": parts, **extra}


# ------------------------------------------------------------------- Igor ----

IGOR_MOODS = ["neutral", "blink", "joy", "shock"]


def igor_head(id, mood="neutral"):
    """Prince Igor: a round lumpy head under a conical gold helmet, an
    enormous black moustache, jowls and a double chin."""
    jaw = {"shock": 0.8, "joy": 0.3}.get(mood, 0.0)
    L = []
    face = [(0, -250), (70, -244), (110, -210), (124, -160), (128, -110), (118, -66), (104, -36), (70, -10), (36, 6), (0, 10),
            (-36, 6), (-70, -10), (-104, -36), (-118, -66), (-128, -110), (-124, -160), (-110, -210), (-70, -244)]
    face = jawed(face, jaw, top=-80, amount=36)
    chin2 = jawed([(-70, -12), (-36, 10), (0, 16), (36, 10), (70, -12), (60, 22), (0, 38), (-60, 22)], jaw, top=-80, amount=36)
    L.append(shape(f"{id}-chin2", chin2, SKIN_DK, w=3.6))
    L.append(shape(f"{id}-face", face, SKIN, w=4.4))
    light = [light_split(f"{id}-sh", 20, -280, 60),
             polys(f"{id}-cheek", [[(-110, -110), (-84, -122), (-62, -108), (-86, -96)]], SKIN_LT)]
    light += stubble(f"{id}-stub", jawed([(-110, -70), (110, -70), (90, -10), (0, 6), (-90, -10)], jaw, top=-80, amount=36), 70, seed=21)
    light += pores(f"{id}-pores", [(-120, -120), (-70, -120), (-70, -80), (-120, -80)], 16, seed=22)
    L += matted(f"{id}-light", light, face)
    L.append(strokes(f"{id}-jowls", [[(-98, -84), (-104, -54 + jaw * 20), (-90, -28 + jaw * 30)], [(98, -84), (104, -54 + jaw * 20), (90, -28 + jaw * 30)]], w=2.4, color=FINE, taper=(6, 6)))
    m = mood if mood != "blink" else "neutral"
    L += mouth(f"{id}-mouth", 0, -54, w=40 if m in ("neutral", "smug") else 52, mood={"joy": "grin"}.get(m, m), jaw=jaw)
    # Nose: a red-tipped potato.
    nose = [(-16, -150), (16, -150), (30, -124), (34, -100), (18, -86), (0, -90), (-18, -86), (-34, -100), (-30, -124)]
    L.append(shape(f"{id}-nose", nose, SKIN, w=3.8))
    L.append(polys(f"{id}-nosesh", [[(10, -148), (30, -124), (34, -100), (18, -88), (12, -110)]], SKIN_DK))
    L.append(dots(f"{id}-nostrils", [(-12, -94), (12, -94)], r=3.2))
    # The moustache: a black handlebar wider than the face, barbed.
    mous = [(0, -84), (40, -92), (90, -88), (150, -100), (180, -122), (170, -94), (140, -70), (90, -60), (40, -66), (0, -72),
            (-40, -66), (-90, -60), (-140, -70), (-170, -94), (-180, -122), (-150, -100), (-90, -88), (-40, -92)]
    L.append(shape(f"{id}-mous", barbed(mous, depth=6, every=1, seed=seed_of(id)), INK, w=1.6, rough=0.4, smooth=0.6))
    for s in (-1, 1):
        L += eye(f"{id}-eye{s}", s * 42, -170, w=28, h=12, look=(0.2, 0.0), mood=mood if mood != "joy" else "smug", side=s, tilt=0, lid=0.4)
        L += brow(f"{id}-brow{s}", s * 44, -196, w=42, mood=m, side=s, thick=9, barbs=True)
        if mood != "shock":
            L += bags(f"{id}-bags{s}", s * 42, -156, w=28, n=3)
    # Conical helmet with a red band and rivets, glinting.
    helm = [(-122, -196), (-110, -260), (-70, -330), (0, -410), (70, -330), (110, -260), (122, -196), (60, -210), (0, -214), (-60, -210)]
    L.append(shape(f"{id}-helm", helm, GOLD, w=4.0, smooth=0.7))
    L += matted(f"{id}-helmsh", [light_split(f"{id}-helmsh-s", 6, -430, -180, color=GOLD_DK),
                                 strokes(f"{id}-ribs", [[(0, -406), (-50, -214)], [(0, -406), (50, -214)]], w=2.0, color=GOLD_DK, taper=(6, 2))], helm)
    L.append(shape(f"{id}-helmband", [(-124, -226), (0, -240), (124, -226), (124, -196), (0, -210), (-124, -196)], RED, w=3.2, smooth=0.5))
    L.append(dots(f"{id}-rivets", [(-90, -216), (-45, -224), (0, -226), (45, -224), (90, -216)], r=3.4, color=GOLD))
    L.append(sparkle(f"{id}-glint", -40, -330, r=13))
    return L


def igor_body(id):
    """Short and stout: a mail shirt and a red cloak, neck (0, 0) to floor (0, 330)."""
    L = []
    for s in (-1, 1):
        L.append(shape(f"{id}-leg{s}", [(s * 20, 220), (s * 80, 220), (s * 82, 300), (s * 24, 300)], COBALT_DK, w=3.2, smooth=0.4))
        L.append(shape(f"{id}-boot{s}", [(s * 16, 292), (s * 86, 292), (s * 112, 328), (s * 14, 330)], INK, w=2.4, smooth=0.5))
    cloak = [(-110, 0), (110, 0), (178, 120), (190, 250), (0, 262), (-190, 250), (-178, 120)]
    L.append(shape(f"{id}-cloak", cloak, RED, w=4.0))
    L += matted(f"{id}-cloaksh", [light_split(f"{id}-cloaksh-s", 10, -20, 280, color=RED_DK)], cloak)
    mail = [(-80, 0), (80, 0), (130, 70), (136, 170), (110, 236), (0, 246), (-110, 236), (-136, 170), (-130, 70)]
    L.append(shape(f"{id}-mail", mail, COBALT_LT, w=4.0))
    L += matted(f"{id}-mailt", [light_split(f"{id}-mailsh", 10, -20, 260, color=COBALT),
                                strokes(f"{id}-rings", dash_rows(-140, 140, 6, 240, gap=9, dash=6, slant=60, seed=23), w=1.6, color=COBALT_DK, taper=(1.5, 1.5))], mail)
    L.append(shape(f"{id}-belt", [(-134, 150), (0, 160), (134, 150), (132, 174), (0, 184), (-132, 174)], INK, w=2.4, smooth=0.5))
    L.append(shape(f"{id}-buckle", [(-16, 152), (16, 152), (16, 184), (-16, 184)], GOLD, w=2.4, smooth=0.3))
    return L


def igor(id, pos, scale=1.0, mood_keys=None, arm_l=None, arm_r=None, hand_l="fist", hand_r="open", **extra):
    neck_y = -330
    parts = [comp(f"{id}-body", igor_body(f"{id}-b"), pos=(0, neck_y))]
    for s, rot, hp in ((-1, arm_l, hand_l), (1, arm_r, hand_r)):
        arm = comp(f"{id}-arm{s}", sleeve(f"{id}-a{s}", 170, 64, fill=COBALT_LT, cuff=GOLD, shade=COBALT)
                   + [comp(f"{id}-hand{s}", hand(f"{id}-h{s}", hp, lit=s < 0, side=s, ring=s > 0, size=1.15), pos=(0, 162))],
                   pos=(s * 120, neck_y + 40))
        if rot is not None:
            arm["transform"]["rotation"] = rot
        parts.insert(0 if s > 0 else len(parts), arm)
    parts.append(comp(f"{id}-head", head_swaps(f"{id}-hd", igor_head, IGOR_MOODS, mood_keys), pos=(0, neck_y + 20)))
    t = {"position": r2(pos)}
    if scale != 1:
        t["scale"] = [scale, scale]
    return {"id": id, "kind": "comp", "transform": t, "layers": parts, **extra}


# --------------------------------------------------------------------- Mal ----

def mal(id, pos, scale=1.0, **extra):
    """Prince Mal: lanky, a long pale barbed beard, an antler crown, a smug
    grin. A single drawing (an extra) that breathes."""
    L = []
    robe = [(-60, -560), (60, -560), (90, -420), (100, -200), (120, 0), (-120, 0), (-100, -200), (-90, -420)]
    L.append(shape(f"{id}-robe", robe, RED, w=4.0))
    L += matted(f"{id}-robesh", [light_split(f"{id}-rs", 0, -600, 20, color=RED_DK),
                                 strokes(f"{id}-pl", [[(x, -580), (x + 8, 20)] for x in range(-110, 120, 30)] + [[(-130, y), (130, y + 4)] for y in range(-540, 10, 34)], w=3.0, color=GOLD, taper=(0, 0), pressure=0.1)], robe)
    face = [(0, -800), (40, -796), (58, -760), (62, -700), (56, -640), (40, -600), (0, -590), (-40, -600), (-56, -640), (-62, -700), (-58, -760), (-40, -796)]
    L.append(shape(f"{id}-face", face, SKIN, w=4.0))
    L += matted(f"{id}-fl", [light_split(f"{id}-fs", 8, -820, -560)], face)
    beard = [(-50, -660), (-20, -640), (0, -636), (20, -640), (50, -660), (46, -580), (30, -500), (0, -440), (-30, -500), (-46, -580)]
    L.append(shape(f"{id}-beard", barbed(beard, depth=8, every=1, seed=seed_of(id)), CREAM, w=3.0, rough=0.5, smooth=0.6))
    L.append(strokes(f"{id}-bh", ticks_in(beard, 14, 14, angle=95, seed=31), w=1.6, color=CREAM_DK))
    L += mouth(f"{id}-mouth", 0, -662, w=40, mood="grin", jaw=0.15, gold=[3])
    L.append(shape(f"{id}-nose", [(-8, -740), (8, -740), (22, -690), (8, -680), (-14, -684)], SKIN, w=3.4))
    L.append(dots(f"{id}-nostril", [(-4, -686)], r=2.6))
    for s in (-1, 1):
        L += eye(f"{id}-eye{s}", s * 26, -728, w=24, h=10, look=(-0.4, 0), mood="smug", side=s, lid=0.5)
        L += brow(f"{id}-brow{s}", s * 28, -750, w=30, mood="smug", side=s, thick=5)
    # An antler crown: a gold band with branching tines.
    L.append(shape(f"{id}-crownband", [(-62, -800), (0, -808), (62, -800), (60, -780), (0, -788), (-60, -780)], GOLD, w=3.0, smooth=0.5))
    for s_ in (-1, 1):
        beam = [(s_ * 40, -804), (s_ * 70, -850), (s_ * 120, -880), (s_ * 160, -930)]
        L.append(line(f"{id}-beam{s_}", beam, w=11, color=GOLD_DK, taper=(2, 10), pressure=0.2))
        L.append(line(f"{id}-beamink{s_}", beam, w=3.0, taper=(2, 8)))
        tines = [[(s_ * 70, -850), (s_ * 60, -900)], [(s_ * 100, -870), (s_ * 98, -930)], [(s_ * 130, -888), (s_ * 150, -950)], [(s_ * 120, -880), (s_ * 170, -880)]]
        for k, t_ in enumerate(tines):
            L.append(line(f"{id}-tine{s_}{k}", t_, w=7, color=GOLD_DK, taper=(1, 7), pressure=0.2))
    L.append(sparkle(f"{id}-glint", 98, -930, r=10))
    t = {"position": r2(pos)}
    if scale != 1:
        t["scale"] = [scale, scale]
    return {"id": id, "kind": "comp", "transform": t, "layers": L, **extra}


# ------------------------------------------------------------------ extras ----

def guard(id, pos, scale=1.0, flip=False, spear=True, mood="neutral", **extra):
    """A Kyivan guard: pointed cobalt helmet, red tunic, a spear or a shovel."""
    rnd = random.Random(seed_of(id))
    L = []
    w = rnd.uniform(0.9, 1.1)
    body = [(-60 * w, -360), (60 * w, -360), (80 * w, -200), (84 * w, -60), (-84 * w, -60), (-80 * w, -200)]
    for s in (-1, 1):
        L.append(shape(f"{id}-leg{s}", [(s * 12, -70), (s * 56, -70), (s * 58, -10), (s * 14, -10)], CREAM_DK, w=3.0, smooth=0.4))
        L.append(shape(f"{id}-boot{s}", [(s * 8, -16), (s * 62, -16), (s * 80, 4), (s * 8, 6)], INK, w=2.2, smooth=0.5))
    L.append(shape(f"{id}-tunic", body, RED, w=3.8))
    L += matted(f"{id}-ts", [light_split(f"{id}-tss", 8, -380, 0, color=RED_DK)], body)
    L.append(shape(f"{id}-belt", [(-82 * w, -170), (82 * w, -170), (82 * w, -152), (-82 * w, -152)], INK, w=2.0, smooth=0.4))
    face = blob(0, -430, 54, 66, n=12, lump=0.06, seed=seed_of(id))
    L.append(shape(f"{id}-face", face, SKIN, w=3.8))
    L += matted(f"{id}-fl", [light_split(f"{id}-fs", 6, -520, -350)], face)
    nose = [(-8, -446), (10, -446), (22, -414), (8, -404), (-12, -408)]
    L.append(shape(f"{id}-nose", nose, SKIN, w=3.2))
    for s in (-1, 1):
        L += eye(f"{id}-eye{s}", s * 20, -448, w=18, h=8, look=(0.3, 0), mood=mood, side=s, lash=4.0)
    L.append(shape(f"{id}-mous", barbed([(-34, -398), (0, -404), (34, -398), (24, -388), (0, -392), (-24, -388)], depth=4, every=1, seed=seed_of(id)), INK, w=1.4, smooth=0.6))
    L += mouth(f"{id}-mouth", 0, -384, w=24, mood={"neutral": "neutral", "joy": "grin"}.get(mood, mood))
    helm = [(-60, -470), (-40, -520), (0, -580), (40, -520), (60, -470), (0, -480)]
    L.append(shape(f"{id}-helm", helm, COBALT_LT, w=3.6, smooth=0.6))
    L += matted(f"{id}-hs", [light_split(f"{id}-hss", 4, -600, -460, color=COBALT)], helm)
    if spear:
        L.insert(0, line(f"{id}-shaft", [(96, 20), (96, -560)], w=5, color=INK, taper=(0, 0), pressure=0.1))
        L.insert(1, shape(f"{id}-tip", [(96, -640), (110, -580), (96, -560), (82, -580)], CREAM, w=2.6, smooth=0.0))
        L.append(comp(f"{id}-hand", hand(f"{id}-h", "grip", side=1), pos=(100, -300)))
    t = {"position": r2(pos)}
    sx = -scale if flip else scale
    if scale != 1 or flip:
        t["scale"] = [sx, scale]
    return {"id": id, "kind": "comp", "transform": t, "layers": L, **extra}


def drevlian(id, pos, scale=1.0, flip=False, mood="neutral", tunic=CREAM, **extra):
    """A Drevlian man: fur hat, beard, plaid tunic. Fewer marks than a lead."""
    rnd = random.Random(seed_of(id))
    L = []
    fat = rnd.uniform(0.95, 1.3)
    body = [(-70 * fat, -330), (70 * fat, -330), (100 * fat, -200), (96 * fat, -40), (-96 * fat, -40), (-100 * fat, -200)]
    for s in (-1, 1):
        L.append(shape(f"{id}-leg{s}", [(s * 12, -50), (s * 60, -50), (s * 60, -8), (s * 14, -8)], CREAM_DK, w=3.0, smooth=0.4))
        L.append(shape(f"{id}-boot{s}", [(s * 8, -14), (s * 64, -14), (s * 80, 4), (s * 8, 6)], INK, w=2.2, smooth=0.5))
    L.append(shape(f"{id}-tunic", body, tunic, w=3.8))
    L += matted(f"{id}-ts", [light_split(f"{id}-tss", 8, -360, 0, color=CREAM_DK if tunic == CREAM else RED_DK),
                             strokes(f"{id}-pl", [[(x, -340), (x + 4, -30)] for x in range(-100, 110, 36)], w=2.4, color=RED, taper=(0, 0), pressure=0.1)], body)
    face = blob(0, -400, 62, 64, n=12, lump=0.08, seed=seed_of(id))
    L.append(shape(f"{id}-face", face, SKIN, w=3.8))
    L += matted(f"{id}-fl", [light_split(f"{id}-fs", 6, -480, -320)], face)
    beard = [(-62, -400), (-30, -380), (0, -376), (30, -380), (62, -400), (52, -340), (0, -300), (-52, -340)]
    L.append(shape(f"{id}-beard", barbed(beard, depth=6, every=1, seed=seed_of(id)), INK, w=1.6, rough=0.4, smooth=0.5))
    L.append(shape(f"{id}-nose", blob(0, -404, 18, 15, n=8, lump=0.1, seed=seed_of(id) + 1), SKIN, w=3.2))
    for s in (-1, 1):
        L += eye(f"{id}-eye{s}", s * 24, -428, w=18, h=8, look=(0.2, 0), mood=mood, side=s, lash=4.0)
    L += mouth(f"{id}-mouth", 0, -370, w=26, mood={"neutral": "neutral", "joy": "grin"}.get(mood, mood), jaw=0.5 if mood in ("shock", "fear") else 0.1)
    hat = [(-66, -440), (-70, -490), (-40, -530), (0, -540), (40, -530), (70, -490), (66, -440)]
    L.append(shape(f"{id}-hat", barbed(hat, depth=5, every=1, seed=seed_of(id) + 2), GOLD, w=3.2, rough=0.4, smooth=0.6))
    L.append(strokes(f"{id}-fur", dash_rows(-60, 60, -528, -448, gap=12, dash=8, slant=75, seed=seed_of(id)), w=1.6, color=INK, taper=(2, 2)))
    t = {"position": r2(pos)}
    sx = -scale if flip else scale
    if scale != 1 or flip:
        t["scale"] = [sx, scale]
    return {"id": id, "kind": "comp", "transform": t, "layers": L, **extra}
