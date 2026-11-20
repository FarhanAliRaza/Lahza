"""Simulation, part 1: rigid bodies and joints, baked.

    python3 tumble.py OUT_DIR [path/to/lahza-motion]

Builds a 1280×720 project with two scenes, every moving thing a body of
the scene's simulation (`"body"` on a layer; the scene's `"simulation"`):

  letters — extruded letters dropped onto a floor: they tumble, land on
            each other and are knocked about by a ball thrown in at 2.2 s
  wall    — a wall of boxes at rest; a heavy ball on a rope swings into it

then bakes both (`lahza-motion bake`), checks, and writes a contact sheet
per scene. The CPU renderer draws them unless GPU=1. Set RENDER=1 for an
MP4 as well.
"""
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "sim-out")
BIN = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "../../target/release/lahza-motion")
PROJECT = os.path.join(OUT, "tumble.lahza")
BACKEND = "--gpu" if os.environ.get("GPU") else "--cpu"


def run(*args):
    out = subprocess.run([BIN, *args], capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"{' '.join(args)}: {out.stderr or out.stdout}")
    return out.stdout


W, H = 1280, 720
FLOOR = 600
LIGHTS = [
    {"from": [-0.6, -1, -0.7], "intensity": 1.3},
    {"from": [0.9, -0.5, -0.4], "color": "#bcd0ff", "intensity": 0.5},
    {"type": "ambient", "intensity": 0.25},
]

# Block letters, 160 px tall, centred on the layer origin (y down).
LETTERS = {
    "L": "M -50 -80 L -15 -80 L -15 45 L 50 45 L 50 80 L -50 80 Z",
    "A": "M -62 80 L -20 -80 L 20 -80 L 62 80 L 27 80 L 17 42 L -17 42 L -27 80 Z M -9 12 L 0 -34 L 9 12 Z",
    "H": "M -55 -80 L -20 -80 L -20 -18 L 20 -18 L 20 -80 L 55 -80 L 55 80 L 20 80 L 20 18 L -20 18 L -20 80 L -55 80 Z",
    "Z": "M -55 -80 L 55 -80 L 55 -50 L -8 45 L 55 45 L 55 80 L -55 80 L -55 50 L 8 -45 L -55 -45 Z",
}
COLORS = ["#ff5a36", "#ffc800", "#2d5bff", "#19c38a", "#ff3d7f"]


def floor(depth=1400):
    return {"id": "floor", "kind": "mesh", "mesh": {"type": "box", "size": [2400, 40], "depth": depth},
            "material": {"preset": "matte", "color": "#e9e4d8"},
            "transform": {"position": [W / 2, FLOOR + 20]},
            "body": {"type": "fixed", "friction": 0.8}}


letters = {
    "id": "letters", "start": 0, "end": 4.5, "background": "#f4f1ea",
    "camera": {"fov": 38, "orbit": [-14, 12]},
    "lights": LIGHTS,
    "simulation": {"seed": 3, "jitter": 20},
    "layers": [floor()] + [
        {"id": f"l{i}", "kind": "mesh",
         "mesh": {"type": "extrude", "d": LETTERS[ch], "depth": 46, "bevel": 4},
         "material": {"preset": "glossy", "color": COLORS[i]},
         "transform": {"position": [W / 2 - 300 + i * 150, -120 - i * 170], "z": 30 * (i % 2) - 15,
                       "rotation": [8, -14, 22, -9, 15][i], "rotation_y": [20, -30, 10, 35, -15][i]},
         # Cut into convex pieces: the A's counter and the L's notch stay open.
         "body": {"shape": {"type": "pieces"}, "bounce": 0.25, "friction": 0.5, "spin": [0, 0, [40, -60, 30, -50, 70][i]]}}
        for i, ch in enumerate("LAHZA")
    ] + [
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 46},
         "material": {"preset": "chrome", "color": "#ffffff"},
         # Keyed in from the left until 2.2 s, then the simulation's.
         "transform": {"position": {"keys": [{"at": 1.7, "v": [-200, 470]}, {"at": 2.2, "v": [150, 500], "ease": "linear"}]}},
         "time": {"start": 1.7},
         "body": {"release": 2.2, "density": 6, "bounce": 0.3}},
    ],
}

rows, cols = 5, 6
wall = {
    "id": "wall", "start": 4.5, "end": 9.0, "background": "#10131c",
    "camera": {"fov": 38, "orbit": [18, 10]},
    "lights": LIGHTS,
    "environment": {"preset": "night", "intensity": 0.8},
    "simulation": {"joints": [
        # The ball hangs from a point above the wall by a rope.
        {"id": "rope", "type": "rope", "a": "wrecker", "b": None, "anchor_b": [W / 2 - 60, 60, 0], "length": 440, "draw": "cord",
         # Sixteen bodies end to end: it swings, and lies on what it meets.
         "links": 16},
    ]},
    "layers": [dict(floor(), id="floor2", material={"preset": "matte", "color": "#2a3040"})] + [
        {"id": f"b{r}-{c}", "kind": "mesh",
         "mesh": {"type": "box", "size": [78, 58], "depth": 58, "bevel": 3},
         "material": {"preset": "plastic", "color": COLORS[(r + c) % len(COLORS)]},
         "transform": {"position": [W / 2 + 60 + (c - cols / 2) * 80, FLOOR - 29 - r * 58]},
         "body": {"friction": 0.7, "bounce": 0.05}}
        for r in range(rows) for c in range(cols)
    ] + [
        {"id": "wrecker", "kind": "mesh", "mesh": {"type": "sphere", "radius": 70},
         "material": {"preset": "metal", "color": "#c9ccd6", "roughness": 0.3},
         # Held out to the left, on the rope's arc; let go at 5.2 s.
         "transform": {"position": [W / 2 - 60 - 440, 60]},
         "body": {"release": 5.2, "density": 8}},
        # The rope is drawn with this layer, stretched from the ball to the pin.
        {"id": "cord", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 4, "height": 100},
         "material": {"preset": "matte", "color": "#d8c9a8"}},
        {"id": "pin", "kind": "mesh", "mesh": {"type": "sphere", "radius": 10},
         "material": {"preset": "emissive", "color": "#ffffff"}, "transform": {"position": [W / 2 - 60, 60]}},
    ],
}

for s in (letters, wall):
    for layer in s["layers"]:
        layer.setdefault("allow", ["draws_off_frame"])

os.makedirs(OUT, exist_ok=True)
if not os.path.isdir(PROJECT):
    run("motion", "create", PROJECT, "--size", f"{W}x{H}", "--fps", "30", "--duration", "9")
ops = [{"op": "remove", "id": s["id"]} for s in (letters, wall)]
ops_file = os.path.join(OUT, "ops.json")
# Replace the scenes of an earlier run.
info = json.loads(run("motion", "info", PROJECT))
have = [s["id"] for s in info.get("scenes", [])]
ops = [{"op": "remove", "id": i} for i in have] + [{"op": "add_scene", "scene": s} for s in (letters, wall)]
json.dump({"ops": ops}, open(ops_file, "w"))
run("motion", "apply", PROJECT, ops_file)

baked = json.loads(run("motion", "bake", PROJECT))
for s in baked["scenes"]:
    hits = [e for e in s.get("events", []) if e["speed"] >= 300]
    print(f"{s['scene']}: {s['status']}, {s.get('bodies')} bodies, {s.get('steps')} steps in {s.get('seconds')} s, {s.get('bytes')} bytes; "
          f"{s.get('events_total')} touches, the hard ones: " + ", ".join(f"{e['a']}×{e['b']} at {e['t']} s" for e in hits[:6]))
print(run("motion", "check", PROJECT, BACKEND))
for scene, times in (("letters", [0.4, 0.9, 1.4, 1.9, 2.3, 2.6, 3.0, 3.6, 4.3]), ("wall", [4.6, 5.3, 5.8, 6.1, 6.3, 6.6, 7.0, 7.8, 8.9])):
    sheet = os.path.join(OUT, f"{scene}.png")
    run("motion", "snapshot", PROJECT, BACKEND, "--sheet", "--at", ",".join(str(t) for t in times), "-o", sheet)
    print("wrote", sheet)
if os.environ.get("RENDER"):
    out = os.path.join(OUT, "tumble.mp4")
    run("motion", "render", PROJECT, BACKEND, "-o", out)
    print("wrote", out)
