"""Lahza motion reel: a 37 s motion-design reel built only through MCP.

Every frame is drawn by the Lahza motion engine: shapes, type, morphs,
2.5D, effects and blend modes, with every cut and hit anchored to the
music's beats. The music is synthesised by music.py.

    python3 reel.py OUT_DIR [path/to/lahza-motion]

Set SKIP_RENDER=1 to stop after the contact sheet.
"""
import base64
import json
import math
import os
import random
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "reel-out")
BIN = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "../../target/release/lahza-motion")
os.makedirs(OUT, exist_ok=True)


class Mcp:
    """A minimal MCP client over stdio."""

    def __init__(self):
        self.proc = subprocess.Popen([BIN, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, cwd=OUT, text=True)
        self.id = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "reel", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, msg):
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.id += 1
        self.send({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params})
        while True:
            reply = json.loads(self.proc.stdout.readline())
            if reply.get("id") == self.id:
                if "error" in reply:
                    raise RuntimeError(reply["error"])
                return reply["result"]

    def tool(self, name, **args):
        started = time.time()
        result = self.request("tools/call", {"name": name, "arguments": args})
        text = result["content"][0]["text"]
        if result.get("isError"):
            raise RuntimeError(f"{name}: {text}")
        print(f"  {name:<14} {time.time() - started:6.1f}s")
        return json.loads(text), [c for c in result["content"][1:] if c["type"] == "image"]


W, H = 1920, 1080
C = [960, 540]
INK = "#0b0b10"
LIME, PURPLE, PINK, TEAL, GOLD, BLUE, ORANGE, CREAM = "#d7ff3a", "#7b61ff", "#ff3355", "#33ddaa", "#ffd23f", "#2d9cff", "#ff8a00", "#f4efe6"
BLACK = "assets/fonts/InterDisplay-Black.otf"
BLACK_I = "assets/fonts/InterDisplay-BlackItalic.otf"
THIN = "assets/fonts/InterDisplay-Thin.otf"
FONTS = ["InterDisplay-Black.otf", "InterDisplay-BlackItalic.otf", "InterDisplay-Thin.otf"]


# --- time --------------------------------------------------------------------

def bar(b):
    return f"bar:{b}"


def beat(b, k):
    """Beat k (1-4) of bar b; k may run past 4 into the next bars."""
    return f"beat:{(b - 1) * 4 + k}"


def half(b, k):
    """Eighth k (1-8) of bar b."""
    return f"half:{(b - 1) * 8 + k}"


def sixteenth(b, k):
    return f"sixteenth:{(b - 1) * 16 + k}"


def plus(t, frames):
    return f"{t} + {frames}f" if frames >= 0 else f"{t} - {-frames}f"


# --- layers ------------------------------------------------------------------

def add(scene, layer):
    return {"op": "add", "scene": scene, "layer": layer}


def window(layer, start=None, end=None):
    layer["time"] = {k: v for k, v in (("start", start), ("end", end)) if v is not None}
    return layer


def rect(id, fill, size=None, pos=None, **extra):
    layer = {"id": id, "kind": "shape", "shape": "rect", "size": size or [W + 40, H + 40], "fill": fill, "transform": {"position": pos or C}}
    layer.update(extra)
    return layer


def word(id, txt, size, pos=None, color="#ffffff", font=BLACK, **extra):
    layer = {"id": id, "kind": "text", "text": txt, "size": size, "color": color, "font": font, "transform": {"position": pos or C}}
    layer.update(extra)
    return layer


def glyphs(at, frm, dur="7f", stagger="1f", ease="slam", unit="glyph", order="forward", anchor="start"):
    return {"at": at, "from": frm, "dur": dur, "stagger": stagger, "ease": ease, "unit": unit, "order": order, "anchor": anchor}


def grain(amount=0.05):
    return {"type": "grain", "amount": amount, "size": 1.5}


def backdrop(id, inner, outer=INK):
    return rect(id, {"gradient": "radial", "from": [0, 0], "to": [0, 1000], "stops": [[0, inner], [1, outer]]}, effects=[grain()])


def shockwave(id, at, colour, size=220, grow=7, thickness=6, pos=None):
    return {"id": id, "kind": "shape", "shape": "ring", "size": size, "thickness": thickness, "fill": colour,
            "time": {"start": at, "end": plus(at, 24)},
            "transform": {"position": pos or C,
                          "scale": {"keys": [{"at": at, "from": 0.4, "v": grow, "ease": "out", "dur": "22f", "anchor": "start"}]},
                          "opacity": {"keys": [{"at": at, "from": 1, "v": 0, "ease": "in", "dur": "22f", "anchor": "start"}]}}}


def wipe_circle(id, at, colour, pos=None, dur="11f"):
    """A circle that grows from nothing to cover the frame."""
    return {"id": id, "kind": "shape", "shape": "circle", "size": 100, "fill": colour, "time": {"start": at},
            "transform": {"position": pos or C,
                          "scale": {"keys": [{"at": at, "from": 0, "v": 46, "ease": "in-out", "dur": dur, "anchor": "start"}]}}}


def flash(id, at, peak=0.8):
    return rect(id, "#ffffff", blend="add", transform={"position": C, "opacity": {"keys": [{"at": at, "from": peak, "v": 0, "ease": "settle", "dur": "14f", "anchor": "start"}]}})


def spin(rate):
    return {"keys": [{"at": 0, "v": 0}], "add": [{"type": "drift", "rate": rate}]}


# --- shots -------------------------------------------------------------------

def opening():
    """A dot falls, bounces on the beat, and bursts the frame open."""
    s = "open"
    floor = 760
    ops = [{"op": "add_scene", "id": s, "start": 0, "background": INK}, add(s, backdrop("open-bg", "#17172a"))]
    ops.append(add(s, {"id": "floor", "kind": "shape", "shape": "line", "size": [1300, 0], "thickness": 3, "fill": "#ffffff55",
                       "transform": {"position": [960, floor]},
                       "trim": {"keys": [{"at": beat(1, 1), "from": [0.5, 0.5], "v": [0, 1], "ease": "glide", "dur": "16f", "anchor": "start"},
                                         {"at": half(1, 8), "v": [0.5, 0.5], "ease": "snap", "dur": "8f", "anchor": "start"}]}}))
    # Height keys: classic keyframes, ease-in to each landing, ease-out to each apex.
    y = [{"at": 0, "v": -140}, {"at": beat(1, 1), "v": floor, "ease": "in"},
         {"at": half(1, 2), "v": floor - 330, "ease": "out"}, {"at": beat(1, 2), "v": floor, "ease": "in"},
         {"at": half(1, 4), "v": floor - 250, "ease": "out"}, {"at": beat(1, 3), "v": floor, "ease": "in"},
         {"at": half(1, 6), "v": floor - 170, "ease": "out"}, {"at": beat(1, 4), "v": floor, "ease": "in"},
         {"at": half(1, 8), "v": 595, "ease": "out"}]
    x = [{"at": 0, "v": 420}, {"at": beat(1, 1), "v": 420}, {"at": beat(1, 2), "v": 640, "ease": "linear"},
         {"at": beat(1, 3), "v": 820, "ease": "linear"}, {"at": beat(1, 4), "v": 960, "ease": "linear"}]
    squash = [{"at": 0, "v": [0.85, 1.25]}]
    for k in range(1, 5):
        squash += [{"at": plus(beat(1, k), -1), "v": [0.8, 1.3], "ease": "out", "dur": "3f", "anchor": "end"},
                   {"at": beat(1, k), "v": [1.5, 0.55], "ease": "snap", "dur": "2f", "anchor": "end"},
                   {"at": plus(beat(1, k), 1), "v": [0.9, 1.15] if k < 4 else [1, 1], "ease": "pop", "dur": "9f", "anchor": "start"}]
    squash.append({"at": half(1, 8), "v": [1, 1], "ease": "settle", "dur": "6f", "anchor": "start"})
    ops.append(add(s, window({"id": "dot", "kind": "shape", "shape": "circle", "size": 110, "fill": LIME,
                              "transform": {"anchor": [0, 55], "position": {"keys": [{"at": 0, "v": [420, -140]}]},
                                            "scale": {"keys": squash}},
                              "effects": [{"type": "glow", "color": LIME, "radius": 18, "intensity": 0.6}]}, end=plus(bar(2), 2))))
    # The dot's position: x and y keyed separately through a parent null.
    ops[-1]["layer"]["transform"]["position"] = {"keys": [{"at": k["at"], "v": [xv, yv]} | ({"ease": k.get("ease")} if k.get("ease") else {})
                                                          for k, xv, yv in _merge(x, y)]}
    # Contact shadow.
    ops.append(add(s, window({"id": "dot-shadow", "kind": "shape", "shape": "circle", "size": [130, 16], "fill": "#00000099",
                              "transform": {"position": {"keys": [{"at": k["at"], "v": [xv, floor + 6]} for k, xv, _ in _merge(x, y)]},
                                            "opacity": {"keys": [{"at": 0, "v": 0}, {"at": beat(1, 1), "v": 1, "ease": "in"},
                                                                 {"at": half(1, 8), "v": 0, "ease": "out"}]}},
                              "effects": [{"type": "blur", "radius": 6}]}, end=bar(2))))
    for k in range(1, 4):
        ops.append(add(s, shockwave(f"bounce-ring{k}", beat(1, k), "#ffffff88", size=120, grow=2.6, thickness=3, pos=[[420, 640, 820][k - 1], floor])))
    # Bar 2: the burst. Lime floods out from the dot, rays fire, and three
    # more colours wipe through on the beats.
    ops.append(add(s, wipe_circle("wipe-lime", bar(2), LIME, pos=[960, 540], dur="9f")))
    rays = []
    for i in range(24):
        a = i * 15
        rad = math.radians(a)
        rays.append({"id": f"ray{i}", "kind": "shape", "shape": "line", "size": [380 if i % 2 else 560, 0], "thickness": 10 if i % 2 else 5, "fill": INK,
                     "transform": {"position": [560 * math.cos(rad), 560 * math.sin(rad)], "rotation": a},
                     "trim": {"keys": [{"at": plus(bar(2), 2), "from": [0, 0], "v": [0, 1], "ease": "snap", "dur": "7f", "anchor": "start"},
                                       {"at": plus(bar(2), 8), "v": [1, 1], "ease": "glide", "dur": "12f", "anchor": "start"}]}})
    ops.append(add(s, window({"id": "rays", "kind": "comp", "layers": rays, "transform": {"position": C, "rotation": spin(30)}}, bar(2), beat(2, 3))))
    ops.append(add(s, window(word("open-hit", "HELLO", 330, color=INK, tracking=-20,
                                  animators=[glyphs(plus(bar(2), 3), {"scale": 2.2, "opacity": 0}, dur="6f", order="center")]), bar(2), beat(2, 2))))
    ops.append(add(s, wipe_circle("wipe-purple", beat(2, 2), PURPLE, pos=[1500, 260])))
    ops.append(add(s, window(word("open-w2", "WE", 330, color="#ffffff", tracking=-20,
                                  animators=[glyphs(beat(2, 2), {"y": 200, "opacity": 0}, dur="7f", ease="land", stagger="2f")]), plus(beat(2, 2), 3), beat(2, 3))))
    ops.append(add(s, wipe_circle("wipe-pink", beat(2, 3), PINK, pos=[380, 860])))
    ops.append(add(s, window(word("open-w3", "MAKE", 330, color=INK, font=BLACK_I, tracking=-20,
                                  animators=[glyphs(beat(2, 3), {"x": 260, "opacity": 0}, dur="7f", ease="land", stagger="1f")]), plus(beat(2, 3), 3), beat(2, 4))))
    ops.append(add(s, wipe_circle("wipe-ink", beat(2, 4), INK)))
    ops.append(add(s, window(word("open-w4", "THINGS MOVE", 210, color=LIME, tracking=-10,
                                  animators=[glyphs(beat(2, 4), {"scale": 0.2, "opacity": 0}, dur="8f", ease="pop", stagger="1f", order="center")],
                                  effects=[{"type": "rgb_split", "amount": {"keys": [{"at": beat(2, 4), "from": 26, "v": 0, "ease": "settle", "dur": "14f", "anchor": "start"}]}}]),
                             plus(beat(2, 4), 3))))
    for k, colour in ((1, INK), (2, "#ffffff"), (3, INK), (4, LIME)):
        ops.append(add(s, shockwave(f"burst-ring{k}", beat(2, k), colour, size=240, grow=8, thickness=8)))
    return ops


def _merge(x, y):
    """Join x and y key lists on their times (x holds after its last key)."""
    xs = {k["at"]: k["v"] for k in x}
    last = x[0]["v"]
    for k in y:
        last = xs.get(k["at"], last)
        yield k, last, k["v"]


def kinetic():
    """Four words, four beats, four looks; then MOTION drops in letter by letter."""
    s = "type"
    ops = [{"op": "add_scene", "id": s, "start": bar(3), "background": INK, "motion_blur": {"shutter": 180, "samples": 5}}]
    # SHAPE: lime, with three outlined shapes punching in behind.
    ops += [add(s, window(rect("shape-bg", LIME), bar(3), beat(3, 2)))]
    for i, (shape, extra) in enumerate((("circle", {}), ("polygon", {"sides": 3}), ("rect", {"radius": 12}))):
        ops.append(add(s, window({"id": f"shape-{shape}", "kind": "shape", "shape": shape, "size": 300, "fill": "#00000000",
                                  "stroke": {"color": INK, "width": 10}, **extra,
                                  "transform": {"position": [560 + 400 * i, 540], "rotation": spin(90 if i % 2 else -90),
                                                "scale": {"keys": [{"at": plus(bar(3), i * 2), "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]}}},
                                 bar(3), beat(3, 2))))
    ops.append(add(s, window(word("shape-w", "SHAPE", 360, color=INK, tracking=-30,
                                  animators=[glyphs(bar(3), {"y": -240, "opacity": 0}, dur="7f", ease="land", stagger="1f")]), bar(3), beat(3, 2))))
    # TYPE: the thin cut, letter-spacing collapsing on the beat.
    ops.append(add(s, window(rect("type-bg", CREAM), beat(3, 2), beat(3, 3))))
    ops.append(add(s, window(word("type-w", "TYPE", 420, color=INK, font=THIN,
                                  tracking={"keys": [{"at": beat(3, 2), "from": 900, "v": 40, "ease": "snap", "dur": "10f", "anchor": "start"}]},
                                  animators=[glyphs(beat(3, 2), {"opacity": 0}, dur="4f", ease="out", stagger="1f", order="edges")]),
                             beat(3, 2), beat(3, 3))))
    # TIME: a clock face sweeps around the word.
    ops.append(add(s, window(rect("time-bg", PURPLE), beat(3, 3), beat(3, 4))))
    ops.append(add(s, window({"id": "time-arc", "kind": "shape", "shape": "ring", "size": 760, "thickness": 18, "fill": "#ffffff",
                              "trim": {"keys": [{"at": beat(3, 3), "from": [0, 0], "v": [0, 1], "ease": "in-out", "dur": "14f", "anchor": "start"}]},
                              "transform": {"position": C, "rotation": -90}}, beat(3, 3), beat(3, 4))))
    for i in range(12):
        a = math.radians(i * 30)
        ops.append(add(s, window({"id": f"tick{i}", "kind": "shape", "shape": "capsule", "size": [10, 44 if i % 3 == 0 else 24], "fill": "#ffffff",
                                  "transform": {"position": [960 + 320 * math.sin(a), 540 - 320 * math.cos(a)], "rotation": i * 30,
                                                "scale": {"keys": [{"at": plus(beat(3, 3), i), "from": 0, "v": 1, "ease": "pop", "dur": "6f", "anchor": "start"}]}}},
                                 beat(3, 3), beat(3, 4))))
    ops.append(add(s, window(word("time-w", "TIME", 300, font=BLACK_I, tracking=-20,
                                  animators=[glyphs(beat(3, 3), {"x": -160, "opacity": 0}, dur="6f", ease="land", stagger="1f")]),
                             beat(3, 3), beat(3, 4))))
    # SPACE: the word swings round in 3D.
    ops.append(add(s, window(backdrop("space-bg", "#14204a", "#05060c"), beat(3, 4), bar(4))))
    ops.append(add(s, window(word("space-w", "SPACE", 340, color=TEAL, three_d=True, tracking=-20,
                                  transform={"position": C, "rotation_y": {"keys": [{"at": beat(3, 4), "from": -80, "v": 18, "ease": "land", "dur": "14f", "anchor": "start"}]},
                                             "rotation_x": {"keys": [{"at": beat(3, 4), "from": 30, "v": -8, "ease": "land", "dur": "14f", "anchor": "start"}]}},
                                  effects=[{"type": "glow", "color": TEAL, "radius": 24, "intensity": 0.6}]),
                             beat(3, 4), bar(4))))
    # MOTION: pink, one letter per eighth note, then the word blows past camera.
    ops.append(add(s, window(rect("motion-bg", PINK), bar(4))))
    ops.append(add(s, window(word("motion-shadow", "MOTION", 330, color=INK, tracking=-30, blend="multiply",
                                  transform={"position": [972, 556], "opacity": 0.35, "scale": {"keys": [{"at": beat(4, 4), "v": 1}, {"at": plus(bar(5), -1), "v": 9, "ease": "in"}]}},
                                  animators=[glyphs(bar(4), {"y": -700, "rotation": -25}, dur="8f", ease="land", stagger=0.25)]),
                             bar(4))))
    ops.append(add(s, window(word("motion-w", "MOTION", 330, color="#ffffff", tracking=-30,
                                  transform={"position": C, "scale": {"keys": [{"at": beat(4, 4), "v": 1}, {"at": plus(bar(5), -1), "v": 9, "ease": "in"}]}},
                                  animators=[glyphs(bar(4), {"y": -700, "rotation": -25}, dur="8f", ease="land", stagger=0.25)]),
                             bar(4))))
    return ops


MORPHS = [
    {"shape": "circle", "size": 420},
    {"shape": "star", "size": 600, "sides": 6, "inner": 0.5},
    {"shape": "rect", "size": [520, 520], "radius": 120},
    {"shape": "star", "size": 560, "sides": 12, "inner": 0.78},
    {"shape": "polygon", "size": 560, "sides": 3, "radius": 40},
    {"shape": "capsule", "size": [700, 240]},
    {"shape": "star", "size": 620, "sides": 4, "inner": 0.32},
    {"shape": "polygon", "size": 520, "sides": 6, "radius": 30},
]


def liquid():
    """A shape that changes its mind on every beat, with a coloured echo trail."""
    s = "liquid"
    ops = [{"op": "add_scene", "id": s, "start": bar(5), "background": "#090b1c"}, add(s, backdrop("liquid-bg", "#1d1450", "#07081a"))]
    # Background: slow arcs.
    for i, (size, rate, colour) in enumerate(((980, 20, "#ffffff22"), (1240, -14, "#7b61ff55"), (1500, 9, "#ffffff14"))):
        ops.append(add(s, {"id": f"arc{i}", "kind": "shape", "shape": "arc", "size": size, "thickness": 3, "fill": colour, "angles": [0, 250 - 60 * i],
                           "transform": {"position": C, "rotation": spin(rate)}}))
    morph_keys = lambda lead: [{"at": beat(5, 1), "v": MORPHS[0]}] + [
        {"at": beat(5, k + 1), "v": MORPHS[k % len(MORPHS)], "ease": "pop", "dur": "12f", "anchor": "start", "lead": f"{lead}f"} for k in range(1, 8)]
    rot_keys = lambda lead: [{"at": beat(5, 1), "v": 0}] + [
        {"at": beat(5, k + 1), "v": 45 * k, "ease": "snap", "dur": "12f", "anchor": "start", "lead": f"{lead}f"} for k in range(1, 8)]
    scale_keys = lambda lead: [{"at": beat(5, 1), "from": 0, "v": 1, "ease": "land", "anchor": "start", "lead": f"{lead}f"}]
    for i, (lead, colour, opacity) in enumerate(((9, BLUE, 0.35), (6, PINK, 0.5), (3, PURPLE, 0.7))):
        ops.append(add(s, {"id": f"echo{i}", "kind": "shape", "shape": "path", "d": {"keys": morph_keys(lead)}, "fill": colour, "blend": "screen",
                           "transform": {"position": C, "opacity": opacity, "rotation": {"keys": rot_keys(lead)}, "scale": {"keys": scale_keys(lead)}}}))
    ops.append(add(s, {"id": "blob", "kind": "shape", "shape": "path", "d": {"keys": morph_keys(0)},
                       "fill": {"gradient": "linear", "from": [-300, -300], "to": [300, 300], "stops": [[0, LIME], [0.6, TEAL], [1, BLUE]]},
                       "transform": {"position": C, "rotation": {"keys": rot_keys(0)}, "scale": {"keys": scale_keys(0)}},
                       "effects": [{"type": "glow", "color": LIME, "radius": 30, "intensity": 0.55}]}))
    # Satellites that orbit the blob.
    sats = [{"id": f"sat{i}", "kind": "shape", "shape": "circle", "size": 22 + 10 * (i % 2), "fill": [LIME, PINK, "#ffffff", TEAL][i],
             "transform": {"position": [470 * math.cos(i * math.pi / 2), 470 * math.sin(i * math.pi / 2)]}} for i in range(4)]
    ops.append(add(s, {"id": "sats", "kind": "comp", "layers": sats, "transform": {"position": C, "rotation": spin(-70)}}))
    for k in range(1, 9):
        ops.append(add(s, shockwave(f"liquid-ring{k}", beat(5, k), "#ffffff66", size=500, grow=2.2, thickness=3)))
    return ops


def grid():
    """A field of dots that ripples with the beat, turns, and collapses into the stop."""
    s = "grid"
    cols, rows, step = 15, 9, 124
    ops = [{"op": "add_scene", "id": s, "start": bar(7), "background": INK}, add(s, backdrop("grid-bg", "#1a1a2a"))]
    dots = []
    palette = [LIME, PINK, PURPLE, TEAL, GOLD, "#ffffff"]
    rng = random.Random(7)
    origins = [(0, 0), (-700, -350), (700, 350), (-700, 350), (700, -350), (0, 0), (0, 0)]
    for r in range(rows):
        for c in range(cols):
            x, y = (c - (cols - 1) / 2) * step, (r - (rows - 1) / 2) * step
            delay = round(math.hypot(x, y) / 90)
            scale = [{"at": plus(bar(7), delay), "from": 0, "v": 1, "ease": "pop", "dur": "10f", "anchor": "start"}]
            fill = [{"at": bar(7), "v": "#ffffff"}]
            for k in range(2, 8):  # beats 2..7 of the two bars: a ripple from a new origin each beat
                ox, oy = origins[k - 1]
                d = round(math.hypot(x - ox, y - oy) / 110)
                at = plus(beat(7, k), d)
                fill.append({"at": at, "v": palette[(k + (r + c) % 2) % len(palette)], "ease": "snap", "dur": "4f", "anchor": "start"})
                scale.append({"at": at, "v": 0.45, "ease": "snap", "dur": "3f", "anchor": "start"})
                scale.append({"at": plus(at, 3), "v": 1, "ease": "pop", "dur": "9f", "anchor": "start"})
            # The stop: everything collapses on the last beat before the drop.
            scale.append({"at": plus(beat(8, 4), round(rng.random() * 3)), "v": 0, "ease": "in", "dur": "5f", "anchor": "end"})
            dots.append({"id": f"d{r}-{c}", "kind": "shape", "shape": "rect" if (r + c) % 3 == 0 else "circle", "size": 56, "radius": 8,
                         "fill": {"keys": fill}, "transform": {"position": [x, y], "scale": {"keys": scale}}})
    ops.append(add(s, {"id": "dots", "kind": "comp", "layers": dots,
                       "transform": {"position": C,
                                     "rotation": {"keys": [{"at": bar(7), "v": 0}, {"at": bar(8), "v": 45, "ease": "gesture", "dur": "26f", "anchor": "peak"}]},
                                     "scale": {"keys": [{"at": bar(7), "v": 1}, {"at": bar(8), "v": 1.35, "ease": "gesture", "dur": "26f", "anchor": "peak"}]}}}))
    return ops


def cube(s, at):
    """A solid cube of six 2.5D planes inside a tilted ring of sparks."""
    h = 205
    faces = [("front", [0, 0], -h, 0, 0, LIME), ("back", [0, 0], h, 0, 180, PURPLE),
             ("right", [h, 0], 0, 0, -90, TEAL), ("left", [-h, 0], 0, 0, 90, PINK),
             ("top", [0, -h], 0, 90, 0, GOLD), ("bottom", [0, h], 0, 90, 0, BLUE)]
    ops = [add(s, {"id": "cube", "kind": "shape", "shape": "circle", "size": 1, "fill": "#00000000", "three_d": True,
                   "transform": {"position": [960, 540],
                                 "scale": {"keys": [{"at": at, "from": 0.2, "v": 1, "ease": "land", "anchor": "start"}]},
                                 "rotation_y": {"keys": [{"at": at, "from": -200, "v": 30, "ease": "land", "dur": "26f", "anchor": "start"}],
                                                "add": [{"type": "drift", "rate": 34, "from": plus(at, 26)}]},
                                 "rotation_x": {"keys": [{"at": at, "from": 80, "v": -24, "ease": "land", "dur": "26f", "anchor": "start"},
                                                         {"at": bar(10), "v": 18, "ease": "gesture", "dur": "20f", "anchor": "peak"}]}}})]
    for name, pos, z, rx, ry, colour in faces:
        ops.append(add(s, {"id": f"face-{name}", "kind": "comp", "three_d": True, "parent": "cube",
                           "transform": {"position": pos, "z": z, "rotation_x": rx, "rotation_y": ry},
                           "layers": [
                               {"id": f"face-{name}-bg", "kind": "shape", "shape": "rect", "size": [2 * h, 2 * h], "radius": 6,
                                "fill": {"gradient": "linear", "from": [-h, -h], "to": [h, h], "stops": [[0, colour], [0.55, "#1a1430"], [1, "#0b0b14"]]},
                                "stroke": {"color": colour, "width": 6}},
                               {"id": f"face-{name}-ring", "kind": "shape", "shape": "ring", "size": 170, "thickness": 14, "fill": "#ffffffd0"},
                               {"id": f"face-{name}-dot", "kind": "shape", "shape": "circle", "size": 46, "fill": colour},
                           ]}))
    ops.append(add(s, {"id": "orbit", "kind": "shape", "shape": "circle", "size": 1, "fill": "#00000000", "three_d": True,
                       "transform": {"position": [960, 540], "rotation_x": 74,
                                     "rotation": {"keys": [{"at": at, "from": -120, "v": 0, "ease": "land", "dur": "30f", "anchor": "start"}],
                                                  "add": [{"type": "drift", "rate": -50, "from": plus(at, 30)}]},
                                     "scale": {"keys": [{"at": plus(at, 2), "from": 0.3, "v": 1, "ease": "land", "anchor": "start"}]}}}))
    for i in range(36):
        a = math.radians(i * 10)
        r = 660 if i % 2 else 720
        colour = [LIME, PURPLE, PINK, TEAL][i % 4]
        ops.append(add(s, {"id": f"spark{i}", "kind": "shape", "shape": "circle", "size": 26 if i % 3 else 40, "fill": colour,
                           "three_d": True, "parent": "orbit",
                           "transform": {"position": [r * math.cos(a), r * math.sin(a)], "rotation_x": -74},
                           "effects": [{"type": "glow", "color": colour, "radius": 16, "intensity": 0.9}]}))
    return ops


def drop():
    s = "drop"
    rays = []
    for i in range(18):
        a = i * 20
        rad = math.radians(a)
        rays.append({"id": f"dray{i}", "kind": "shape", "shape": "line", "size": [520, 0], "thickness": 4, "fill": LIME if i % 2 else PURPLE,
                     "transform": {"position": [720 * math.cos(rad), 720 * math.sin(rad)], "rotation": a},
                     "trim": {"keys": [{"at": bar(9), "from": [0, 0], "v": [0, 1], "ease": "snap", "dur": "8f", "anchor": "start"},
                                       {"at": plus(bar(9), 9), "v": [1, 1], "ease": "glide", "dur": "16f", "anchor": "start"}]}})
    ops = [{"op": "add_scene", "id": s, "start": bar(9), "background": "#05050a", "motion_blur": {"shutter": 180, "samples": 5},
            "camera": {"position": {"keys": [{"at": bar(9), "v": C}], "add": [{"type": "wiggle", "hz": 9, "amp": [10, 10], "seed": 3, "from": bar(9), "until": plus(bar(9), 12)}]}}},
           add(s, backdrop("drop-bg", "#161036", "#05050a")),
           add(s, {"id": "drays", "kind": "comp", "layers": rays, "effects": [{"type": "glow", "color": LIME, "radius": 14, "intensity": 0.8}],
                   "transform": {"position": C, "rotation": spin(12)}})]
    ops += cube(s, bar(9))
    for k in (1, 3, 5, 7):
        ops.append(add(s, shockwave(f"drop-ring{k}", beat(9, k), LIME if k % 4 == 1 else PURPLE, size=400, grow=4, thickness=5)))
    ops.append(add(s, flash("drop-flash", bar(9), 1.0)))
    return ops


def tunnel():
    """Fly through a tunnel of hexagons, one step per beat."""
    s = "tunnel"
    spacing, count = 420, 26
    ops = [{"op": "add_scene", "id": s, "start": bar(11), "background": "#04040a", "camera": {"fov": 70}},
           add(s, backdrop("tunnel-bg", "#120a2e", "#04040a")),
           add(s, {"id": "core", "kind": "shape", "shape": "circle", "size": 160, "fill": "#ffffff",
                   "transform": {"position": C, "scale": {"keys": [{"at": bar(11), "v": 1}], "add": [{"type": "sine", "hz": 2, "amp": 0.15}]}},
                   "effects": [{"type": "glow", "color": PINK, "radius": 60, "intensity": 1.2}]})]
    z_keys = [{"at": bar(11), "v": 0}] + [{"at": beat(11, k + 1), "v": -spacing * 2 * k, "ease": "snap", "dur": "12f", "anchor": "start"} for k in range(1, 8)]
    roll = [{"at": bar(11), "v": 0}] + [{"at": beat(11, k + 1), "v": 30 * k, "ease": "snap", "dur": "12f", "anchor": "start"} for k in range(1, 8)]
    ops.append(add(s, {"id": "rig", "kind": "shape", "shape": "circle", "size": 1, "fill": "#00000000", "three_d": True,
                       "transform": {"position": C, "z": {"keys": z_keys, "add": [{"type": "drift", "rate": -260}]}, "rotation": {"keys": roll}}}))
    palette = [LIME, PURPLE, PINK, TEAL, BLUE, GOLD]
    # Far rings first so the near ones paint over them.
    for i in reversed(range(count)):
        ops.append(add(s, {"id": f"hex{i}", "kind": "shape", "shape": "polygon", "sides": 6, "size": 1000, "radius": 20, "fill": "#00000000",
                           "stroke": {"color": palette[i % len(palette)], "width": 14 if i % 2 else 6},
                           "three_d": True, "parent": "rig",
                           "transform": {"position": [0, 0], "z": 300 + spacing * i, "rotation": i * 9}}))
    return ops


def marquee():
    """Rows of type sliding against each other under a difference-blend lens."""
    s = "marquee"
    ops = [{"op": "add_scene", "id": s, "start": bar(13), "background": INK}]
    rows = []
    styles = [(BLACK, "#ffffff"), (THIN, LIME), (BLACK_I, PURPLE), (BLACK, "#ffffff"), (THIN, PINK), (BLACK_I, TEAL), (BLACK, "#ffffff")]
    for i, (font, colour) in enumerate(styles):
        direction = 1 if i % 2 else -1
        x0 = direction * -420
        rows.append({"id": f"row{i}", "kind": "text", "text": "LAHZA  MOTION  LAHZA  MOTION", "size": 190, "font": font, "color": colour, "tracking": -10,
                     "transform": {"position": {"keys": [{"at": bar(13), "v": [x0, (i - 3) * 205]}, {"at": bar(15), "v": [x0 + direction * 840, (i - 3) * 205], "ease": "linear"}]}}})
    ops.append(add(s, {"id": "rows", "kind": "comp", "layers": rows,
                       "transform": {"position": C, "rotation": -12,
                                     "scale": {"keys": [{"at": bar(13), "from": 1.6, "v": 1.15, "ease": "land", "dur": "20f", "anchor": "start"}]}}}))
    spots = [[960, 540], [560, 380], [1340, 700], [760, 760], [1240, 330], [960, 540], [500, 640], [1420, 460]]
    pos = [{"at": bar(13), "v": spots[0]}] + [{"at": beat(13, k + 1), "v": spots[k], "ease": "snap", "dur": "10f", "anchor": "start"} for k in range(1, 8)]
    size = [{"at": bar(13), "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]
    for k in range(1, 8):
        size += [{"at": beat(13, k + 1), "v": 1.25, "ease": "snap", "dur": "3f", "anchor": "start"},
                 {"at": plus(beat(13, k + 1), 3), "v": 1, "ease": "settle", "dur": "10f", "anchor": "start"}]
    ops.append(add(s, {"id": "lens", "kind": "shape", "shape": "circle", "size": 560, "fill": "#ffffff", "blend": "difference",
                       "transform": {"position": {"keys": pos}, "scale": {"keys": size}}}))
    ops.append(add(s, {"id": "lens-ring", "kind": "shape", "shape": "ring", "size": 640, "thickness": 3, "fill": LIME,
                       "transform": {"position": {"keys": pos}, "scale": {"keys": size}, "rotation": spin(60)}, "trim": [0, 0.8]}))
    return ops


def radial():
    """A radial equaliser on the sixteenths around a shape that morphs on the beat."""
    s = "radial"
    rng = random.Random(11)
    n = 56
    ops = [{"op": "add_scene", "id": s, "start": bar(15), "background": "#0a0618"}, add(s, backdrop("radial-bg", "#2a1060", "#0a0618"))]
    bars_ = []
    for i in range(n):
        a = i * 360 / n
        rad = math.radians(a)
        keys = [{"at": bar(15), "v": [1, 0.2]}]
        for k in range(1, 33):
            accent = 1.0 if k % 4 == 1 else 0.55
            keys.append({"at": sixteenth(15, k), "v": [1, 0.3 + accent * (0.4 + 1.6 * rng.random())], "ease": "snap", "dur": "3f", "anchor": "start"})
        colour = LIME if i % 2 == 0 else (PINK if i % 4 == 1 else TEAL)
        bars_.append({"id": f"eq{i}", "kind": "shape", "shape": "capsule", "size": [16, 110], "fill": colour,
                      "transform": {"position": [340 * math.sin(rad), -340 * math.cos(rad)], "rotation": a, "anchor": [0, 55], "scale": {"keys": keys}}})
    ops.append(add(s, {"id": "eq", "kind": "comp", "layers": bars_, "effects": [{"type": "glow", "color": LIME, "radius": 12, "intensity": 0.5}],
                       "transform": {"position": C, "rotation": spin(-16),
                                     "scale": {"keys": [{"at": bar(15), "from": 0.3, "v": 1, "ease": "land", "anchor": "start"}]}}}))
    shapes = [MORPHS[1], MORPHS[0], MORPHS[6], MORPHS[0], MORPHS[3], MORPHS[0], MORPHS[7], MORPHS[0]]
    d = [{"at": bar(15), "v": {**shapes[0], "size": 300}}] + [
        {"at": beat(15, k + 1), "v": {**shapes[k], "size": 300 if shapes[k]["shape"] != "circle" else 240}, "ease": "pop", "dur": "10f", "anchor": "start"} for k in range(1, 8)]
    ops.append(add(s, {"id": "heart", "kind": "shape", "shape": "path", "d": {"keys": d},
                       "fill": {"gradient": "radial", "from": [0, 0], "to": [0, 200], "stops": [[0, "#ffffff"], [1, PINK]]},
                       "transform": {"position": C, "rotation": spin(40)},
                       "effects": [{"type": "glow", "color": PINK, "radius": 40, "intensity": 0.8}]}))
    for k in range(1, 9):
        ops.append(add(s, shockwave(f"radial-ring{k}", beat(15, k), "#ffffff", size=700, grow=2.4, thickness=4)))
    return ops


def stripes():
    """Diagonal stripes sweep the frame on each beat; the last one clears to black."""
    s = "stripes"
    ops = [{"op": "add_scene", "id": s, "start": bar(17), "background": INK, "motion_blur": {"shutter": 180, "samples": 5}}]
    colours = [LIME, PURPLE, PINK, INK]
    words = ["MAKE", "IT", "MOVE", ""]
    for k, colour in enumerate(colours, start=1):
        at = beat(17, k)
        stripes_ = []
        for i in range(10):
            x = -1200 + i * 260
            stripes_.append({"id": f"st{k}-{i}", "kind": "shape", "shape": "rect", "size": [270, 2600], "fill": colour,
                             "transform": {"position": {"keys": [{"at": plus(at, i), "from": [x - 2400, 0], "v": [x, 0], "ease": "out", "dur": "9f", "anchor": "start", "lead": "-6f"}]}}})
        ops.append(add(s, window({"id": f"sweep{k}", "kind": "comp", "layers": stripes_, "transform": {"position": C, "rotation": 24}}, plus(at, -6))))
        if words[k - 1]:
            ops.append(add(s, window(word(f"sweep-w{k}", words[k - 1], 380, color=INK if colour != INK else "#ffffff", font=BLACK_I, tracking=-30,
                                          animators=[glyphs(plus(at, 3), {"x": 300, "opacity": 0}, dur="6f", ease="land", stagger="1f")]),
                                     plus(at, 3))))
    return ops


def finale():
    s = "end"
    ops = [{"op": "add_scene", "id": s, "start": bar(18), "end": 37.0, "background": INK},
           add(s, backdrop("end-bg", "#1b1446")),
           add(s, {"id": "end-ring", "kind": "shape", "shape": "ring", "size": 620, "thickness": 4, "fill": LIME,
                   "trim": {"keys": [{"at": bar(18), "from": [0, 0], "v": [0, 1], "ease": "in-out", "dur": "18f", "anchor": "start"}]},
                   "transform": {"position": [960, 470], "rotation": spin(20),
                                 "scale": {"keys": [{"at": bar(18), "from": 0.55, "v": 1, "ease": "land", "anchor": "start"}]}},
                   "effects": [{"type": "glow", "color": LIME, "radius": 20, "intensity": 0.9}]}),
           add(s, word("end-logo", "LAHZA", 230, [960, 470], tracking=60,
                       transform={"position": [960, 470], "scale": {"keys": [{"at": bar(18), "v": 1}], "add": [{"type": "drift", "rate": 0.0007, "per": "frame", "mode": "mul"}]}},
                       animators=[glyphs(bar(18), {"scale": 2.2, "opacity": 0}, dur="7f", order="center", anchor="end")],
                       effects=[{"type": "glow", "color": "#ffffff", "radius": 18, "intensity": 0.4},
                                {"type": "rgb_split", "amount": {"keys": [{"at": bar(18), "from": 30, "v": 0, "ease": "settle", "dur": "16f", "anchor": "start"}]}}])),
           add(s, word("end-tag", "MOTION  REEL", 46, [960, 860], "#d0d0e0", THIN, tracking=600,
                       animators=[glyphs(beat(18, 2), {"y": 20, "opacity": 0}, dur="14f", stagger="2f", ease="glide", order="center")])),
           add(s, flash("end-flash", bar(18), 0.9))]
    for k in range(1, 4):
        ops.append(add(s, shockwave(f"end-ring{k}", plus(bar(18), 3 * k), LIME, size=620, grow=2.5, thickness=3, pos=[960, 470])))
    ops.append(add(s, rect("end-fade", "#000000", transform={"position": C, "opacity": {"keys": [{"at": 35.9, "v": 0}, {"at": 36.9, "v": 1, "ease": "in-out"}]}})))
    return ops


SHOTS = (opening, kinetic, liquid, grid, drop, tunnel, marquee, radial, stripes, finale)
SHEET = [1.2, 2.5, 3.0, 3.7, 4.4, 4.9, 5.4, 5.9, 7.2, 9.0, 10.6, 12.9, 13.6, 15.0, 16.6, 18.6, 21.0, 22.7, 24.6, 26.4, 29.0, 31.0, 32.6, 33.6, 35.0]


def main():
    print(f"building into {OUT}")
    track = os.path.join(OUT, "track.wav")
    if not os.path.exists(track):
        subprocess.run([sys.executable, os.path.join(HERE, "music.py"), track], check=True)
    mcp = Mcp()
    mcp.tool("project_create", path="reel", width=W, height=H, fps=30, duration=37, style="promo.kinetic", background=INK,
             music="track.wav", licence="original, generated for this video", overwrite=True)
    fonts = os.path.join(OUT, "reel.lahza/assets/fonts")
    os.makedirs(fonts, exist_ok=True)
    for name in FONTS:
        shutil.copy(os.path.join("/usr/share/fonts/opentype/inter", name), fonts)
    analysis, _ = mcp.tool("audio_analyze")
    print(f"  music: {analysis['bpm']} BPM, first downbeat {analysis['first_downbeat']} s")
    for shot in SHOTS:
        result, _ = mcp.tool("edit_apply", ops=shot())
        if result["errors"]:
            print("   errors:", result["errors"][:5])
    report, _ = mcp.tool("check", verbose=False)
    print(f"  check: ok={report['ok']} errors={report['errors']} warnings={report['warnings']} sync points={report['sync_count']} max drift={report['max_drift_frames']} f")
    for issue in report["issues"][:12]:
        print(f"    {issue['severity']:<8} {issue.get('element', '')}: {issue['message']}")
    _, images = mcp.tool("snapshot", sheet=True, width=320, at=[float(os.environ.get("SHEET_SHIFT", 0)) + t for t in SHEET])
    with open(os.path.join(OUT, "contact-sheet.png"), "wb") as f:
        f.write(base64.b64decode(images[0]["data"]))
    if os.environ.get("SKIP_RENDER"):
        return
    job, _ = mcp.tool("render", output=os.path.join(OUT, "lahza-reel.mp4"), quality=os.environ.get("QUALITY", "final"), wait=True)
    print(json.dumps(job.get("report", job), indent=2))


if __name__ == "__main__":
    main()
