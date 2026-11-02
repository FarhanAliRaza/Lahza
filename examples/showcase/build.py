"""Builds the Lahza showcase video through MCP, the way an agent would.

Every step is a tool call to `lahza mcp` (here the stand-alone
`lahza-motion mcp`): create the project, analyse the music, import
footage, spin a native 3D object, build ten scenes with
edit_apply, check sync, snapshot a contact sheet, render.

    python3 build.py OUT_DIR [path/to/lahza-motion]
"""
import json
import math
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "showcase-out")
BIN = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "../../target/release/lahza-motion")
os.makedirs(OUT, exist_ok=True)


class Mcp:
    def __init__(self):
        self.proc = subprocess.Popen([BIN, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, cwd=OUT, text=True)
        self.id = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "showcase", "version": "1"}})
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
LIME, PURPLE, PINK, TEAL, GOLD, BLUE, ORANGE = "#d7ff3a", "#7b61ff", "#ff3355", "#33ddaa", "#ffd23f", "#2d9cff", "#ff8a00"


def bar(b):
    return f"bar:{b}"


def beat(b, k):
    """Beat k (1–4) of bar b."""
    return f"beat:{(b - 1) * 4 + k}"


def half(b, k):
    """Eighth k (1–8) of bar b."""
    return f"half:{(b - 1) * 8 + k}"


def sixteenth(b, k):
    return f"sixteenth:{(b - 1) * 16 + k}"


def drift(at, rate=0.0012):
    return [{"type": "drift", "rate": rate, "per": "frame", "mode": "mul", "from": at}]


def text(id, txt, size, pos, color="#ffffff", font="Inter SemiBold", **extra):
    layer = {"id": id, "kind": "text", "text": txt, "size": size, "color": color, "font": font, "transform": {"position": pos}}
    layer.update(extra)
    return layer


def enter(at, frm, dur="8f", stagger="1f", ease="enter", unit="glyph", order="forward", anchor="start"):
    return {"at": at, "from": frm, "dur": dur, "stagger": stagger, "ease": ease, "unit": unit, "order": order, "anchor": anchor}


def add(scene, layer):
    return {"op": "add", "scene": scene, "layer": layer}


def flash(scene, id, at, peak=0.85, colour="#ffffff"):
    return add(scene, {"id": id, "kind": "shape", "shape": "rect", "size": [W, H], "fill": colour, "blend": "add",
                       "transform": {"position": C, "opacity": {"keys": [{"at": at, "from": peak, "v": 0, "ease": "settle", "dur": "16f", "anchor": "start"}]}}})


def backdrop(scene, id, inner, outer="#06060b"):
    return add(scene, {"id": id, "kind": "shape", "shape": "rect", "size": [W, H], "transform": {"position": C},
                       "fill": {"gradient": "radial", "from": [0, 0], "to": [0, 900], "stops": [[0, inner], [1, outer]]},
                       "effects": [{"type": "grain", "amount": 0.045, "size": 1.5}]})


def intro():
    s = "intro"
    return [
        {"op": "add_scene", "id": s, "start": 0, "background": "#06060b"},
        backdrop(s, "intro-bg", "#1b1446"),
        add(s, {"id": "intro-line", "kind": "shape", "shape": "line", "size": [1500, 0], "thickness": 3, "fill": "#ffffff",
                "transform": {"position": [960, 680], "opacity": 0.6},
                "trim": {"keys": [{"at": beat(1, 1), "from": [0.5, 0.5], "v": [0, 1], "ease": "glide", "dur": "28f", "anchor": "start"}]}}),
        add(s, {"id": "intro-ring", "kind": "shape", "shape": "ring", "size": 600, "thickness": 4, "fill": LIME,
                "transform": {"position": C, "rotation": {"keys": [{"at": 0, "v": -90}], "add": [{"type": "drift", "rate": 18}]}},
                "trim": {"keys": [{"at": beat(1, 2), "from": [0, 0], "v": [0, 1], "ease": "settle", "dur": "40f", "anchor": "start"}]},
                "effects": [{"type": "glow", "color": LIME, "radius": 18, "intensity": 0.9}]}),
        add(s, text("intro-logo", "LAHZA", 230, C, tracking=140,
                    transform={"position": C, "scale": {"keys": [{"at": bar(2), "v": 1}], "add": drift(bar(2), 0.0008)}},
                    animators=[enter(bar(2), {"scale": 2.6, "opacity": 0}, dur="7f", stagger="1f", ease="slam", order="center", anchor="end")],
                    effects=[{"type": "glow", "color": "#ffffff", "radius": 22, "intensity": 0.55},
                             {"type": "rgb_split", "amount": {"keys": [{"at": bar(2), "from": 16, "v": 0, "ease": "settle", "dur": "18f", "anchor": "start"}]}}])),
        add(s, text("intro-sub", "MOTION ENGINE  ·  BUILT FOR AGENTS", 34, [960, 760], "#b9b9d0", "Inter Medium", tracking=260,
                    animators=[enter(beat(2, 2), {"y": 30, "opacity": 0}, dur="14f", stagger="3f", ease="glide", unit="word")])),
        flash(s, "intro-flash", bar(2), 0.7),
    ]


def timing():
    s = "timing"
    words = [("TIMING", 300, 170, "#ffffff"), ("IS", 465, 110, "#ffffff"), ("THE", 590, 110, "#ffffff"), ("CRAFT", 790, 210, LIME)]
    ops = [
        {"op": "add_scene", "id": s, "start": bar(3), "background": "#0b0b12",
         "transition": {"type": "whip", "dur": "8f", "direction": "left"},
         "motion_blur": {"shutter": 180, "samples": 5},
         "camera": {"zoom": {"keys": [{"at": bar(3), "v": 1.0}, {"at": bar(5), "v": 1.07, "ease": "push"}]}}},
        backdrop(s, "timing-bg", "#14142a", "#0b0b12"),
        add(s, text("timing-cap", "every key lands on its beat — measured to the frame", 30, [960, 120], "#9a9ab5", "Inter Medium",
                    animators=[enter(beat(4, 1), {"y": -20, "opacity": 0}, dur="12f", stagger="2f", ease="glide", unit="word")])),
    ]
    for k, (w, y, size, colour) in enumerate(words):
        effects = [{"type": "glow", "color": LIME, "radius": 18, "intensity": 0.6}, {"type": "rgb_split", "amount": 3}] if w == "CRAFT" else []
        ops.append(add(s, text(f"timing-{w.lower()}", w, size, [960, y], colour, tracking=40,
                               transform={"position": [960, y], "scale": {"keys": [{"at": beat(3, k + 1), "v": 1}], "add": drift(beat(3, k + 1), 0.001)}},
                               time={"start": beat(3, k + 1)},
                               animators=[enter(beat(3, k + 1), {"y": 110, "opacity": 0, "rotation": 8}, dur="9f", stagger="1f")],
                               effects=effects)))
    for i in range(8):
        ops.append(add(s, {"id": f"timing-dot{i}", "kind": "shape", "shape": "circle", "size": 40, "fill": [PINK, GOLD, TEAL, BLUE][i % 4],
                           "transform": {"position": [645 + i * 90, 960],
                                         "scale": {"keys": [{"at": half(4, i + 1), "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]}}}))
    return ops


def morph():
    s = "morph"
    shapes = [
        (beat(5, 1), {"shape": "circle", "size": 380}, "land"),
        (beat(5, 2), {"shape": "star", "size": 540, "sides": 5, "inner": 0.42}, "pop"),
        (beat(5, 3), {"shape": "rect", "size": [560, 360], "radius": 70}, "pop"),
        (beat(5, 4), {"shape": "polygon", "size": 480, "sides": 6, "radius": 20}, "gesture"),
        (beat(6, 1), {"shape": "capsule", "size": [640, 230]}, "pop"),
        (beat(6, 2), {"shape": "star", "size": 540, "sides": 8, "inner": 0.68}, "pop"),
        (beat(6, 3), {"shape": "polygon", "size": 520, "sides": 3, "radius": 30}, "pop"),
        (beat(6, 4), {"shape": "circle", "size": 420}, "land"),
    ]
    keys = [{"at": shapes[0][0], "v": shapes[0][1]}]
    keys += [{"at": at, "v": v, "ease": ease, "dur": "14f", "anchor": "start"} for at, v, ease in shapes[1:]]
    ops = [
        {"op": "add_scene", "id": s, "start": bar(5), "background": "#07101a",
         "transition": {"type": "zoom", "dur": "10f"}},
        backdrop(s, "morph-bg", "#0d2a36", "#07101a"),
        add(s, {"id": "morph-orbit", "kind": "shape", "shape": "ring", "size": 820, "thickness": 2, "fill": "#ffffff", "trim": [0, 0.72],
                "transform": {"position": [960, 500], "opacity": 0.3, "rotation": {"keys": [{"at": 0, "v": 0}], "add": [{"type": "drift", "rate": 45}]}}}),
        add(s, {"id": "morph-shape", "kind": "shape", "shape": "path", "d": {"keys": keys},
                "fill": {"gradient": "linear", "from": [-260, -260], "to": [260, 260], "stops": [[0, LIME], [1, TEAL]]},
                "stroke": {"color": "#ffffff", "width": 4},
                "transform": {"position": [960, 500], "rotation": {"keys": [{"at": 0, "v": 0}], "add": [{"type": "sine", "hz": 0.5, "amp": 8}]}},
                "effects": [{"type": "glow", "color": LIME, "radius": 26, "intensity": 0.7}, {"type": "shadow", "offset": [0, 26], "radius": 26, "color": "#00000099"}]}),
        add(s, text("morph-title", "MORPH ANY SHAPE INTO ANY SHAPE", 56, [960, 930], tracking=60,
                    animators=[enter(bar(5) + " + 4f", {"y": 40, "opacity": 0}, dur="12f", stagger="2f", ease="glide", unit="word")])),
        add(s, text("morph-cap", "keyed paths · resampled · aligned · eased like any property", 28, [960, 1000], "#8fb3c0", "Inter Medium",
                    animators=[enter(beat(5, 3), {"opacity": 0}, dur="10f", stagger="2f", ease="glide", unit="word")])),
    ]
    sats = []
    for i in range(3):
        a = i * 2 * math.pi / 3
        sats.append({"id": f"morph-sat{i}", "kind": "shape", "shape": "circle", "size": 26, "fill": [PINK, GOLD, PURPLE][i],
                     "transform": {"position": [410 * math.sin(a), -410 * math.cos(a)]}})
    ops.append(add(s, {"id": "morph-sats", "kind": "comp", "layers": sats,
                       "transform": {"position": [960, 500], "rotation": {"keys": [{"at": 0, "v": 0}], "add": [{"type": "drift", "rate": -60}]}}}))
    return ops


def depth():
    s = "depth"
    cards = [("FOOTAGE", PINK, "circle"), ("3D", PURPLE, "polygon"), ("EFFECTS", TEAL, "star"), ("BLEND", GOLD, "ring"), ("MORPHS", BLUE, "star"), ("AGENTS", ORANGE, "capsule")]
    ops = [
        {"op": "add_scene", "id": s, "start": bar(7), "background": "#07070d",
         "transition": {"type": "push", "dur": "10f", "direction": "left"},
         "motion_blur": {"shutter": 180, "samples": 5},
         "camera": {"orbit": {"keys": [{"at": bar(7), "v": [-14, 10]}, {"at": beat(8, 4), "v": [10, -6], "ease": "gesture"}]},
                    "dolly": {"keys": [{"at": bar(7), "v": -200}, {"at": beat(8, 4), "v": 260, "ease": "gesture"}]}}},
        backdrop(s, "depth-bg", "#1a1030", "#07070d"),
        add(s, {"id": "pivot", "kind": "shape", "shape": "circle", "size": 1, "fill": "#00000000", "three_d": True,
                "transform": {"position": [960, 520], "z": 360, "rotation_y": {"keys": [{"at": beat(7, 1), "v": 0}] + [
                    {"at": beat(7 + (k + 1) // 4, (k + 1) % 4 + 1), "v": -30 * (k + 1), "ease": "snap", "dur": "12f", "anchor": "start"} for k in range(6)]}}}),
    ]
    for i, (label, colour, icon) in enumerate(cards):
        a = i * 60
        x, z = 560 * math.sin(math.radians(a)), -560 * math.cos(math.radians(a))
        icon_layer = {"id": f"card{i}-icon", "kind": "shape", "shape": icon, "size": [110, 70] if icon == "capsule" else 120, "sides": 6, "thickness": 14,
                      "fill": "#ffffff", "transform": {"position": [0, -60], "opacity": 0.9}}
        ops.append(add(s, {"id": f"card{i}", "kind": "comp", "three_d": True, "parent": "pivot",
                           "transform": {"position": [x, 0], "z": z, "rotation_y": -a},
                           "effects": [{"type": "shadow", "offset": [0, 24], "radius": 18, "color": "#000000aa"}],
                           "layers": [
                               {"id": f"card{i}-bg", "kind": "shape", "shape": "rect", "size": [300, 400], "radius": 30,
                                "fill": {"gradient": "linear", "from": [-150, -200], "to": [150, 200], "stops": [[0, colour], [1, "#14141f"]]},
                                "stroke": {"color": "#ffffff33", "width": 2}},
                               icon_layer,
                               {"id": f"card{i}-label", "kind": "text", "text": label, "size": 44, "color": "#ffffff", "transform": {"position": [0, 110]}},
                           ]}))
    ops += [
        add(s, text("depth-title", "2.5D LAYERS  ·  PERSPECTIVE CAMERA  ·  DEPTH SORT", 40, [960, 990], tracking=80,
                    animators=[enter(bar(7) + " + 4f", {"y": 30, "opacity": 0}, dur="12f", stagger="2f", ease="glide", unit="word")])),
        # The stop: one silent beat before the drop.
        add(s, {"id": "depth-stop", "kind": "shape", "shape": "rect", "size": [W, H], "fill": "#000000",
                "transform": {"position": C, "opacity": {"keys": [{"at": beat(8, 4), "from": 0, "v": 0.88, "ease": "snap", "dur": "6f", "anchor": "start"}]}}}),
    ]
    return ops


def cube(s):
    """A solid cube built from six 2.5D planes, plus a tilted ring of particles
    that passes in front of and behind it; the engine depth-sorts all of it."""
    h = 205
    faces = [  # id, position, z, rotation_x, rotation_y, colour
        ("front", [0, 0], -h, 0, 0, LIME), ("back", [0, 0], h, 0, 180, PURPLE),
        ("right", [h, 0], 0, 0, -90, TEAL), ("left", [-h, 0], 0, 0, 90, PINK),
        ("top", [0, -h], 0, 90, 0, GOLD), ("bottom", [0, h], 0, 90, 0, BLUE),
    ]
    ops = [add(s, {"id": "cube", "kind": "shape", "shape": "circle", "size": 1, "fill": "#00000000", "three_d": True,
                   "transform": {"position": [960, 575], "z": 0,
                                 "scale": {"keys": [{"at": bar(9), "from": 0.2, "v": 1, "ease": "land", "anchor": "start"}]},
                                 "rotation_y": {"keys": [{"at": bar(9), "from": -200, "v": 30, "ease": "land", "dur": "26f", "anchor": "start"}],
                                                "add": [{"type": "drift", "rate": 34, "from": bar(9) + " + 26f"}]},
                                 "rotation_x": {"keys": [{"at": bar(9), "from": 80, "v": -24, "ease": "land", "dur": "26f", "anchor": "start"},
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
    # The particle ring: a tilted pivot spinning in its own plane.
    ops.append(add(s, {"id": "orbit", "kind": "shape", "shape": "circle", "size": 1, "fill": "#00000000", "three_d": True,
                       "transform": {"position": [960, 575], "rotation_x": 74,
                                     "rotation": {"keys": [{"at": bar(9), "from": -120, "v": 0, "ease": "land", "dur": "30f", "anchor": "start"}],
                                                  "add": [{"type": "drift", "rate": -50, "from": bar(9) + " + 30f"}]},
                                     "scale": {"keys": [{"at": bar(9) + " + 2f", "from": 0.3, "v": 1, "ease": "land", "anchor": "start"}]}}}))
    for i in range(36):
        a = math.radians(i * 360 / 36)
        r = 660 if i % 2 else 720
        colour = [LIME, PURPLE, PINK, TEAL][i % 4]
        ops.append(add(s, {"id": f"spark{i}", "kind": "shape", "shape": "circle", "size": 26 if i % 3 else 40, "fill": colour,
                           "three_d": True, "parent": "orbit",
                           "transform": {"position": [r * math.cos(a), r * math.sin(a)], "rotation_x": -74},
                           "effects": [{"type": "glow", "color": colour, "radius": 16, "intensity": 0.9}]}))
    return ops


def drop():
    s = "drop"
    lines = []
    for i in range(18):
        a = i * 20
        rad = math.radians(a)
        lines.append({"id": f"ray{i}", "kind": "shape", "shape": "line", "size": [520, 0], "thickness": 4, "fill": LIME if i % 2 else PURPLE,
                      "transform": {"position": [700 * math.cos(rad), 700 * math.sin(rad)], "rotation": a},
                      "trim": {"keys": [{"at": bar(9), "from": [0, 0], "v": [0, 1], "ease": "snap", "dur": "8f", "anchor": "start"},
                                        {"at": bar(9) + " + 9f", "v": [1, 1], "ease": "glide", "dur": "16f", "anchor": "start"}]}})
    return [
        {"op": "add_scene", "id": s, "start": bar(9), "background": "#05050a", "motion_blur": {"shutter": 180, "samples": 5},
         "camera": {"position": {"keys": [{"at": bar(9), "v": C}], "add": [{"type": "wiggle", "hz": 9, "amp": [9, 9], "seed": 3, "from": bar(9), "until": bar(9) + " + 12f"}]}}},
        backdrop(s, "drop-bg", "#161036", "#05050a"),
        add(s, {"id": "rays", "kind": "comp", "layers": lines, "effects": [{"type": "glow", "color": LIME, "radius": 14, "intensity": 0.8}],
                "transform": {"position": C, "rotation": {"keys": [{"at": 0, "v": 0}], "add": [{"type": "drift", "rate": 12}]}}}),
        *cube(s),
        add(s, text("drop-real", "NATIVE", 130, [770, 150], tracking=40,
                    animators=[enter(bar(9), {"x": -300, "opacity": 0}, dur="8f", stagger="1f", ease="slam", anchor="end")])),
        add(s, text("drop-3d", "3D", 150, [1250, 150], LIME,
                    animators=[enter(bar(9), {"x": 300, "opacity": 0}, dur="8f", stagger="1f", ease="slam", anchor="end")],
                    effects=[{"type": "glow", "color": LIME, "radius": 24, "intensity": 0.8}])),
        add(s, text("drop-cap", "six planes · one perspective camera · depth sorted every frame · drawn on the GPU", 30, [960, 994], "#b9b9d0", "Inter Medium",
                    animators=[enter(beat(9, 3), {"y": 20, "opacity": 0}, dur="12f", stagger="2f", ease="glide", unit="word")])),
        flash(s, "drop-flash", bar(9), 1.0),
    ]


def footage():
    s = "footage"
    return [
        {"op": "add_scene", "id": s, "start": bar(11), "background": "#0a0a10", "transition": {"type": "wipe", "dur": "10f", "direction": "left"}},
        add(s, {"id": "fmatte", "kind": "shape", "shape": "rect", "fill": "#ffffff", "radius": 48, "transform": {"position": C},
                "size": {"keys": [{"at": bar(11), "from": [360, 220], "v": [1740, 900], "ease": "settle", "dur": "30f", "anchor": "start"}]}}),
        add(s, {"id": "plasma", "kind": "video", "src": "assets/plasma.mp4", "size": [W, H], "matte": {"layer": "fmatte"},
                "transform": {"position": C},
                "time_remap": {"keys": [{"at": bar(11), "v": 0.0}, {"at": beat(11, 4), "v": 1.2, "ease": "gesture"}, {"at": bar(13), "v": 7.5, "ease": "enter"}]},
                "effects": [{"type": "gradient_map", "stops": [[0, "#0d0826"], [0.5, PURPLE], [1, LIME]], "amount": 0.95}, {"type": "vignette", "amount": 0.45}]}),
        add(s, text("footage-word", "FOOTAGE", 270, [960, 520], tracking=60, blend="difference",
                    animators=[enter(bar(11) + " + 6f", {"y": 140, "opacity": 0}, dur="10f", stagger="1f")])),
        add(s, text("footage-cap", "video · image sequences · alpha · time remap · track mattes · LUTs · blend modes", 30, [960, 994], "#d0d0e0", "Inter Medium",
                    animators=[enter(beat(11, 3), {"y": 20, "opacity": 0}, dur="12f", stagger="2f", ease="glide", unit="word")])),
    ]


def effects():
    s = "effects"
    fx = [
        ("exposure", {"type": "exposure", "stops": 1.0}), ("hue", {"type": "hue_saturation", "hue": 140}),
        ("levels", {"type": "levels", "gamma": 1.9, "in_black": 0.15}), ("tint", {"type": "tint", "black": "#0d0826", "white": LIME}),
        ("gradient map", {"type": "gradient_map", "stops": [[0, "#1a0533"], [0.5, PINK], [1, GOLD]]}), ("invert", {"type": "invert"}),
        ("displace", {"type": "displace", "amount": 12, "scale": 28, "speed": 1.5}), ("directional blur", {"type": "directional_blur", "angle": 0, "length": 40}),
        ("rgb split", {"type": "rgb_split", "amount": 9}), ("vignette", {"type": "vignette", "amount": 0.95, "softness": 0.6}),
        ("grain", {"type": "grain", "amount": 0.35, "size": 2}), ("glow", {"type": "glow", "color": "#ffffff", "radius": 16, "intensity": 1.2}),
    ]
    ops = [
        {"op": "add_scene", "id": s, "start": bar(13), "background": "#0b0b12", "transition": {"type": "slide", "dur": "10f", "direction": "up"}},
        backdrop(s, "fx-bg", "#151530", "#0b0b12"),
        add(s, text("fx-title", "16 EFFECTS  ·  19 BLEND MODES  ·  GPU AND CPU, SAME PIXELS", 42, [960, 105], tracking=60,
                    animators=[enter(bar(13) + " + 4f", {"y": -30, "opacity": 0}, dur="12f", stagger="2f", ease="glide", unit="word")])),
    ]
    for i, (name, effect) in enumerate(fx):
        col, row = i % 4, i // 4
        x, y = 330 + col * 420, 320 + row * 270
        ops.append(add(s, {"id": f"tile{i}", "kind": "comp", "effects": [effect],
                           "transform": {"position": [x, y], "scale": {"keys": [{"at": sixteenth(13, 1 + i), "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]}},
                           "layers": [
                               {"id": f"tile{i}-bg", "kind": "shape", "shape": "rect", "size": [380, 210], "radius": 20,
                                "fill": {"gradient": "linear", "from": [-190, 0], "to": [190, 0], "stops": [[0, PINK], [0.35, GOLD], [0.7, TEAL], [1, BLUE]]}},
                               {"id": f"tile{i}-dot", "kind": "shape", "shape": "circle", "size": 100, "fill": "#ffffff", "transform": {"position": [-100, -10]}},
                               {"id": f"tile{i}-aa", "kind": "text", "text": "Aa", "size": 86, "color": "#101018", "transform": {"position": [80, -10]}},
                           ]}))
        ops.append(add(s, text(f"tile{i}-label", name, 26, [x, y + 135], "#c8c8dc", "Inter Medium",
                               animators=[enter(sixteenth(13, 3 + i), {"opacity": 0, "y": 10}, dur="8f", stagger="1f", ease="glide", unit="word")])))
    return ops


def sync():
    s = "sync"
    ops = [
        {"op": "add_scene", "id": s, "start": bar(15), "background": "#07070c", "transition": {"type": "fade", "dur": "8f"}},
        backdrop(s, "sync-bg", "#10162e", "#07070c"),
        add(s, text("sync-title", "THE MUSIC DRIVES THE CUT", 64, [960, 120], tracking=80,
                    animators=[enter(bar(15) + " + 3f", {"y": -30, "opacity": 0}, dur="12f", stagger="2f", ease="glide", unit="word")])),
        add(s, {"id": "sync-ring", "kind": "shape", "shape": "ring", "size": 360, "thickness": 10, "fill": LIME,
                "effects": [{"type": "glow", "color": LIME, "radius": 20, "intensity": 0.9}],
                "transform": {"position": [960, 470], "scale": {"keys": [{"at": beat(b, k), "from": 1.28, "v": 1.0, "ease": "settle", "dur": "14f", "anchor": "start"}
                                                                          for b in (15, 16) for k in range(1, 5)]}}}),
        add(s, text("sync-cap", "audio_analyze → beats · bars · hits · stops · drops · tempo maps → anchored keys", 28, [960, 994], "#9a9ab5", "Inter Medium",
                    animators=[enter(beat(15, 2), {"opacity": 0}, dur="10f", stagger="2f", ease="glide", unit="word")])),
    ]
    for b in (15, 16):
        for k in range(1, 5):
            ops.append(add(s, text(f"count{b}{k}", str(k), 170, [960, 470], "#ffffff",
                                   time={"start": beat(b, k), "duration": 0.5},
                                   animators=[enter(beat(b, k), {"scale": 1.5, "opacity": 0}, dur="6f", ease="snap")])))
    heights = [2.2, 3.4, 4.6, 3.1, 5.2, 3.8, 2.7, 4.3, 5.6, 3.3, 2.5, 4.9, 3.6, 5.1, 2.9, 4.0]
    for i in range(32):
        h = heights[i % 16]
        keys = [{"at": half(b, k), "from": [1, h * (0.6 if k % 2 == 0 else 1.0)], "v": [1, 1], "ease": "settle", "dur": "10f", "anchor": "start"}
                for b in (15, 16) for k in range(1, 9)]
        ops.append(add(s, {"id": f"eq{i}", "kind": "shape", "shape": "capsule", "size": [24, 50],
                           "fill": {"gradient": "linear", "from": [0, 25], "to": [0, -25], "stops": [[0, PURPLE], [1, LIME]]},
                           "transform": {"position": [185 + i * 50, 960], "anchor": [0, 25], "scale": {"keys": keys}}}))
    return ops


def agents():
    s = "agents"
    lines = [
        ("> lahza mcp", LIME),
        ('edit_apply({ "op": "key", "at": "beat:65",', "#e6e6f0"),
        ('             "anchor": "peak", "ease": "pop" })', "#e6e6f0"),
        ("check   →  max drift 0.00 f   ✓", "#9be37a"),
        ("render  →  showcase.mp4", "#e6e6f0"),
    ]
    ops = [
        {"op": "add_scene", "id": s, "start": bar(17), "background": "#08080e", "transition": {"type": "whip", "dur": "8f", "direction": "up"}},
        backdrop(s, "agents-bg", "#1b1030", "#08080e"),
        add(s, {"id": "code-panel", "kind": "shape", "shape": "rect", "size": [900, 440], "radius": 24, "fill": "#12121cf0",
                "stroke": {"color": "#ffffff22", "width": 2}, "transform": {"position": [560, 540]},
                "effects": [{"type": "shadow", "offset": [0, 30], "radius": 30, "color": "#00000099"}]}),
    ]
    for i, (line, colour) in enumerate(lines):
        ops.append(add(s, text(f"code{i}", line, 28, [140, 380 + i * 78], colour, "Inter Medium", align="left",
                               animators=[enter(f"{bar(17)} + {3 + i * 5}f", {"opacity": 0}, dur="1f", stagger="0.35f", ease="hold")])))
    ops += [
        add(s, text("agents-built", "BUILT FOR", 72, [1450, 410], tracking=80,
                    animators=[enter(bar(17) + " + 2f", {"x": 80, "opacity": 0}, dur="10f", stagger="1f", ease="glide")])),
        add(s, text("agents-word", "AGENTS", 180, [1450, 560], LIME,
                    animators=[enter(beat(17, 2), {"scale": 1.8, "opacity": 0}, dur="7f", stagger="1f", ease="slam", order="center", anchor="end")],
                    effects=[{"type": "glow", "color": LIME, "radius": 22, "intensity": 0.7}])),
        add(s, text("agents-cap", "MCP · CLI · editor — one project, one history", 30, [1450, 690], "#b9b9d0", "Inter Medium",
                    animators=[enter(beat(17, 3), {"y": 20, "opacity": 0}, dur="12f", stagger="2f", ease="glide", unit="word")])),
    ]
    return ops


def end():
    s = "end"
    return [
        {"op": "add_scene", "id": s, "start": bar(18), "end": 37.0, "background": "#06060b", "transition": {"type": "zoom", "dur": "12f"}},
        backdrop(s, "end-bg", "#1b1446"),
        add(s, {"id": "end-ring", "kind": "shape", "shape": "ring", "size": 600, "thickness": 4, "fill": LIME,
                "transform": {"position": [960, 450], "scale": {"keys": [{"at": bar(18), "from": 0.55, "v": 1, "ease": "land", "anchor": "start"}], "add": drift(bar(18) + " + 1", 0.0006)}},
                "effects": [{"type": "glow", "color": LIME, "radius": 20, "intensity": 0.9}]}),
        add(s, text("end-logo", "LAHZA", 210, [960, 450], tracking=140,
                    transform={"position": [960, 450], "scale": {"keys": [{"at": bar(18), "v": 1}], "add": drift(bar(18), 0.0007)}},
                    animators=[enter(bar(18), {"scale": 1.9, "opacity": 0}, dur="7f", stagger="1f", ease="slam", order="center", anchor="end")],
                    effects=[{"type": "glow", "color": "#ffffff", "radius": 20, "intensity": 0.5}])),
        add(s, text("end-tag", "LINUX-FIRST MOTION STUDIO  ·  GPU  ·  OPEN FORMAT", 32, [960, 850], "#b9b9d0", "Inter Medium", tracking=220,
                    animators=[enter(beat(18, 2), {"y": 24, "opacity": 0}, dur="14f", stagger="3f", ease="glide", unit="word")])),
        add(s, text("end-cmd", "lahza mcp", 34, [960, 920], LIME, "Inter Medium",
                    animators=[enter(beat(18, 3), {"opacity": 0}, dur="1f", stagger="1f", ease="hold")])),
        flash(s, "end-flash", bar(18), 0.8),
        add(s, {"id": "end-fade", "kind": "shape", "shape": "rect", "size": [W, H], "fill": "#000000",
                "transform": {"position": C, "opacity": {"keys": [{"at": 35.9, "v": 0}, {"at": 36.9, "v": 1, "ease": "gesture"}]}}}),
    ]


def main():
    print(f"building into {OUT}")
    subprocess.run([sys.executable, os.path.join(HERE, "music.py"), os.path.join(OUT, "track.wav")], check=True)
    clip = os.path.join(OUT, "plasma.mp4")
    if not os.path.exists(clip):
        plasma = ("nullsrc=s=960x540:r=30:d=8,format=yuv420p,"
                  "geq=lum='128+50*sin(X/37+T*1.7)+40*sin((X+Y)/53-T*1.1)+35*sin(hypot(X-480-120*sin(T),Y-270)/23-T*3)'"
                  ":cb='128+60*sin(Y/61+T)':cr='128+60*cos(X/71-T*0.7)'")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", plasma,
                        "-c:v", "libx264", "-crf", "16", "-pix_fmt", "yuv420p", clip + ".part.mp4"], check=True)
        os.replace(clip + ".part.mp4", clip)
    mcp = Mcp()
    mcp.tool("project_create", path="showcase", width=W, height=H, fps=30, duration=37, style="promo.kinetic", background="#06060b",
             music="track.wav", licence="original, generated for this video", overwrite=True)
    analysis, _ = mcp.tool("audio_analyze")
    print(f"  music: {analysis['bpm']} BPM, first downbeat {analysis['first_downbeat']} s, {analysis['beats_per_bar']}/4")
    mcp.tool("media_import", path="plasma.mp4", licence="generated (ffmpeg geq plasma)", source="generated")
    for build in (intro, timing, morph, depth, drop, footage, effects, sync, agents, end):
        result, _ = mcp.tool("edit_apply", ops=build())
        if result["errors"]:
            print("   errors:", result["errors"][:5])
    report, _ = mcp.tool("check", verbose=False)
    print(f"  check: ok={report['ok']} errors={report['errors']} warnings={report['warnings']} sync points={report['sync_count']} max drift={report['max_drift_frames']} f")
    for issue in report["issues"][:12]:
        print(f"    {issue['severity']:<8} {issue.get('element', '')}: {issue['message']}")
    _, images = mcp.tool("snapshot", sheet=True, width=300,
                          at=[1.0, 2.6, 5.5, 7.9, 9.4, 10.6, 13.6, 15.2, 18.4, 19.4, 22.6, 24.4, 27.4, 29.4, 31.4, 33.2, 34.6, 35.5])
    import base64
    with open(os.path.join(OUT, "contact-sheet.png"), "wb") as f:
        f.write(base64.b64decode(images[0]["data"]))
    if os.environ.get("SKIP_RENDER"):
        return
    job, _ = mcp.tool("render", output=os.path.join(OUT, "lahza-showcase.mp4"), quality=os.environ.get("QUALITY", "final"), wait=True)
    print(json.dumps(job.get("report", job), indent=2))


if __name__ == "__main__":
    main()
