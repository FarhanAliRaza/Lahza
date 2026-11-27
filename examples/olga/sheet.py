"""Design sheet: the cast with every expression, the sets by day and night,
and the locked palette, rendered through the engine.

    python3 sheet.py OUT_DIR [page ...]
"""
import os
import sys

from kit import *  # noqa: F401,F403
import cast
import sets

OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "olga-out")
PAGES = sys.argv[2:] or ["olga"]
os.makedirs(OUT, exist_ok=True)
W, H = 1920, 1080


def label(id, text, x, y, size=30):
    return {"id": id, "kind": "text", "text": text, "font": "Inter SemiBold", "size": size, "color": INK,
            "role": "decoration", "transform": {"position": [x, y]}}


def page(name, layers, bg=CREAM):
    project = os.path.join(OUT, f"sheet-{name}.lahza")
    if not os.path.exists(project):
        run("motion", "create", project, "--size", f"{W}x{H}", "--fps", 30, "--duration", 1)
    else:
        apply(project, [{"op": "remove", "id": "s1"}], os.path.join(OUT, f"rm-{name}.json"))
    ops = [{"op": "add_scene", "id": "s1", "start": 0, "background": bg},
           {"op": "add", "scene": "s1", "id": "art", "kind": "comp", "layers": layers, "effects": FINISH}]
    apply(project, ops, os.path.join(OUT, f"ops-{name}.json"))
    png = os.path.join(OUT, f"sheet-{name}.png")
    run("motion", "snapshot", project, "--at", 0.5, "--width", W, "-o", png)
    print(png)


def olga_page():
    L = []
    for i, m in enumerate(cast.OLGA_MOODS):
        x = 150 + (i % 3) * 300
        y = 400 + (i // 3) * 470
        L.append(comp(f"oh{i}", cast.olga_head(f"oh{i}", m), transform={"position": [x, y], "scale": [0.95, 0.95]}))
        L.append(label(f"ol{i}", m, x, y + 150, 26))
    L.append(cast.olga("olga", (1500, 1050), scale=0.8, hand_l="rest", hand_r="point"))
    return L


def cast_page():
    L = []
    for i, m in enumerate(cast.ENVOY_MOODS):
        x = 130 + i * 190
        L.append(comp(f"eh{i}", cast.envoy_head(f"eh{i}", m), transform={"position": [x, 440], "scale": [0.62, 0.62]}))
        L.append(label(f"el{i}", m, x, 560, 24))
    for i, m in enumerate(cast.IGOR_MOODS):
        x = 1290 + i * 170
        L.append(comp(f"ih{i}", cast.igor_head(f"ih{i}", m), transform={"position": [x, 440], "scale": [0.6, 0.6]}))
        L.append(label(f"il{i}", m, x, 560, 24))
    L.append(cast.envoy("envoy", (260, 1060), scale=0.52, hand_l="open", hand_r="fist"))
    L.append(cast.igor("igor", (620, 1060), scale=0.62))
    L.append(cast.mal("mal", (880, 1060), scale=0.55))
    L.append(cast.guard("guard1", (1120, 1060), scale=0.7))
    L.append(cast.guard("guard2", (1300, 1060), scale=0.7, flip=True, spear=False, mood="smug"))
    L.append(cast.drevlian("drev1", (1500, 1060), scale=0.72))
    L.append(cast.drevlian("drev2", (1700, 1060), scale=0.72, tunic=RED, mood="fear"))
    return L


def tile(id, world, x, y, w=290, h=446, extra=None):
    """A set shrunk into a tile, clipped to it."""
    k = w / 1500
    layers = world["sky"] + world["back"] + world["mid"] + (extra or []) + world["fore"]
    inner = comp(f"{id}-w", layers, transform={"position": [0, 0], "scale": [k, k]})
    return [shape(f"{id}-clip", [(x, y), (x + w, y), (x + w, y + h), (x, y + h)], "#ffffff", w=0, line=False, rough=0),
            comp(id, [inner], transform={"position": [x, y]}, matte={"layer": f"{id}-clip"}),
            shape(f"{id}-frame", [(x, y), (x + w, y), (x + w, y + h), (x, y + h)], None, w=3.0, smooth=0.0, rough=0.5)]


def sets_page():
    L = []
    names = [("yard", "courtyard, day"), ("hall", "hall, day"), ("bath", "bathhouse, night"), ("grave", "barrow, dusk"), ("isk", "Iskorosten, night"), ("icon", "the saint")]
    for i, (n, title) in enumerate(names):
        x = 40 + i * 312
        world = sets.SETS[n]()
        extra = []
        if n == "yard":
            extra = sets.boat("boat", 700, 1500, 0.9)
        if n == "bath":
            for k, fx in enumerate([380, 700, 980]):
                extra += sets.flame(f"bf{k}", fx, 1060, 2.2, seed=k)
        if n == "grave":
            for k, (tx, ty) in enumerate(world["torches"]):
                extra += sets.flame(f"tf{k}", tx, ty, 0.9, seed=10 + k)
        if n == "isk":
            for k, (ex, ey) in enumerate(world["eaves"][:6]):
                extra += sets.flame(f"if{k}", ex + 40, ey + 40, 1.2, seed=20 + k)
        if n == "icon":
            extra = [comp("saint", cast.olga_head("saint", "smug"), transform={"position": [750, 1100], "scale": [2.6, 2.6]})]
        L += tile(f"t{i}", world, x, 120, extra=extra)
        L.append(label(f"tl{i}", title, x + 145, 610, 24))
    for i, (name, col) in enumerate(PALETTE):
        x = 330 + i * 270
        L.append(shape(f"sw{i}", [(x, 760), (x + 220, 760), (x + 220, 900), (x, 900)], col, w=3.6, smooth=0.0, rough=1.0))
        L.append(label(f"swl{i}", name + "  " + col, x + 110, 940, 22))
    tones = [INK, FINE[:7], SKIN_DK, SKIN_LT, LILAC, SKY, SHADOW_DAY, NIGHT_LIT, NIGHT_SHADE, CREAM_DK, GOLD_DK, RED_DK]
    for i, col in enumerate(tones):
        x = 330 + i * 106
        L.append(shape(f"tn{i}", [(x, 990), (x + 80, 990), (x + 80, 1040), (x, 1040)], col, w=2.4, smooth=0.0, rough=0.6))
    L.append(label("pl", "locked palette (top) and its tones: ink, fine ink, skin shade / light, eye white, day sky, day shade, night lit / shade", 960, 720, 22))
    return L


if __name__ == "__main__":
    for p in PAGES:
        page(p, {"olga": olga_page, "cast": cast_page, "sets": sets_page}[p]())
