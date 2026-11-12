"""Engine 3D (mesh layers): the three looks that used to be faked in 2D.

    python3 shots.py OUT_DIR [path/to/lahza-motion]

Builds a 720×900 project like the event-opener replica's 3D shots, with
every object real geometry lit in the engine (no Blender, no tracing):

  unites  — a smooth, lit torus arc that draws itself around a glass ball
  viewport — glossy petals (extruded, bevelled) and rounded cubes orbiting,
             dropping soft shadows onto the flat background
  rings   — three glossy rings, each split into 4 coloured segments

then a contact sheet per scene and an MP4. Set SKIP_RENDER=1 for sheets only.
"""
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "mesh-out")
BIN = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "../../target/release/lahza-motion")
PROJECT = os.path.join(OUT, "mesh.lahza")


def run(*args):
    out = subprocess.run([BIN, *args], capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"{' '.join(args)}: {out.stderr or out.stdout}")
    return out.stdout


def keys(*pairs, ease="glide"):
    return {"keys": [{"at": t, "v": v, "ease": ease} for t, v in pairs]}


W, H = 720, 900
CX, CY = W / 2, H / 2

# --------------------------------------------------------------- unites
unites = {
    "id": "unites", "start": 0, "end": 2.0, "background": "#05060a",
    "camera": {"fov": 45, "orbit": keys((0, [-6, 4]), (2.0, [8, -2]), ease="linear")},
    "lights": [
        {"from": [-0.8, -1, -0.9], "intensity": 1.15},
        {"from": [1, -0.3, 0.9], "color": "#9fc4ff", "intensity": 1.4},
        {"type": "ambient", "intensity": 0.03},
    ],
    "environment": {"preset": "night", "intensity": 1.2},
    "layers": [
        {"id": "arc", "kind": "mesh",
         "mesh": {"type": "torus", "radius": 230, "tube": 34, "caps": "round",
                  "angles": keys((0.0, [200, 200]), (1.1, [200, 520]), ease="gesture")},
         "material": {"preset": "coated", "color": "#1b2cff", "roughness": 0.35},
         "transform": {"position": [CX, CY - 40], "rotation_x": 58, "rotation_y": -14,
                       "rotation": keys((0, -20), (2.0, 25), ease="linear")}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 62},
         "material": {"preset": "glass", "color": "#e6ecff", "ior": 1.5, "thickness": 90},
         "transform": {"position": [CX, CY - 40], "z": 0,
                       "scale": keys((0.3, 0), (0.75, 1), ease="land")}},
        {"id": "word", "kind": "text", "text": "unites", "size": 96, "color": "#ffffff", "font": "Inter SemiBold",
         "transform": {"position": [CX, CY + 260]},
         "animators": [{"at": 0.6, "from": {"y": 40, "opacity": 0}, "stagger": "1f", "dur": "10f"}]},
    ],
}

# ------------------------------------------------------------- viewport
petal = "M 0 -95 C 52 -60 52 40 0 95 C -52 40 -52 -60 0 -95 Z"
pastels = ["#ff7aa8", "#ffc24a", "#5fd4ff", "#a98bff", "#6ff0b4", "#ff9d6b"]
viewport = {
    "id": "viewport", "start": 2.0, "end": 4.5, "background": "#f3f1ec",
    "camera": {"fov": 40, "orbit": keys((2.0, [0, 18]), (4.5, [0, 6]))},
    "environment": {"preset": "soft"},
    "lights": [{"from": [-0.7, -1, -1.1], "softness": 0.6}, {"from": [0.8, -0.3, 0.9], "intensity": 0.6}],
    "layers": [
        # A backdrop that only takes shadows: the objects drop soft shadows
        # onto the flat background.
        {"id": "backdrop", "kind": "mesh", "mesh": {"type": "plane", "size": [1600, 1800]},
         "material": {"preset": "shadow", "opacity": 0.35},
         "transform": {"position": [CX, CY], "z": 140}},
        {"id": "flower", "kind": "instancer",
         "layout": {"type": "radial", "count": 6, "radius": 120, "align": True},
         "palette": pastels,
         "transform": {"position": [CX, CY], "rotation": keys((2.0, 0), (4.5, 120), ease="linear"),
                       "scale": keys((2.0, 0.2), (2.6, 1), ease="land")},
         "layers": [{"id": "petal", "kind": "mesh",
                     "mesh": {"type": "extrude", "d": petal, "depth": 22, "bevel": 8},
                     "material": {"preset": "glossy", "color": "#ffffff"},
                     "transform": {"rotation_x": 22}}]},
        {"id": "core", "kind": "mesh", "mesh": {"type": "sphere", "radius": 46},
         "material": {"preset": "coated", "color": "#ffffff"},
         "transform": {"position": [CX, CY], "z": -20, "scale": keys((2.2, 0), (2.7, 1), ease="land")}},
        {"id": "cubes", "kind": "instancer",
         "layout": {"type": "radial", "count": 5, "radius": 285},
         "palette": ["#2d5bff", "#ff3d7f", "#ffd23f", "#00c89a", "#7b61ff"],
         "stagger": {"each": "3f"},
         "transform": {"position": [CX, CY], "rotation": keys((2.0, 30), (4.5, -40), ease="linear")},
         "layers": [{"id": "cube", "kind": "mesh",
                     "mesh": {"type": "box", "size": [78, 78], "depth": 78, "bevel": 14},
                     "material": {"preset": "glossy", "color": "#ffffff"},
                     "transform": {"rotation_x": keys((2.0, -40), (4.5, 140), ease="linear"),
                                   "rotation_y": keys((2.0, 30), (4.5, 250), ease="linear"),
                                   "scale": keys((2.3, 0), (2.8, 1), ease="land")}}]},
    ],
}

# ---------------------------------------------------------------- rings
ring_colors = [["#ff3d7f", "#ffd23f", "#2d5bff", "#00d4a6"],
               ["#ffffff", "#ff8a3d", "#ffffff", "#ff8a3d"],
               ["#7b61ff", "#00d4ff", "#7b61ff", "#00d4ff"]]
rings = {
    "id": "rings", "start": 4.5, "end": 6.0, "background": "#07070b",
    "camera": {"fov": 42},
    "lights": [
        {"from": [-0.6, -1, -0.8], "intensity": 1.0},
        {"from": [0.9, -0.2, 1], "intensity": 1.6},
        {"from": [-1, 0.6, 0.6], "color": "#8ab4ff", "intensity": 0.8},
    ],
    "environment": {"preset": "night", "intensity": 1.4},
    "layers": [
        {"id": f"ring{i}", "kind": "mesh",
         "mesh": {"type": "torus", "radius": r, "tube": tube, "caps": "round"},
         "segments": 4, "gap": 7, "colors": ring_colors[i],
         "material": {"preset": "coated", "roughness": 0.3},
         "transform": {"position": [CX, CY],
                       "rotation_x": keys((4.5, rx), (6.0, rx + 50), ease="linear"),
                       "rotation_y": keys((4.5, ry), (6.0, ry - 35), ease="linear"),
                       "rotation": keys((4.5, i * 30), (6.0, i * 30 + 70), ease="linear"),
                       "scale": keys((4.5 + i * 0.08, 0.6), (4.85 + i * 0.08, 1), ease="land")}}
        for i, (r, tube, rx, ry) in enumerate([(250, 30, 62, -10), (185, 26, -35, 40), (120, 22, 20, 70)])
    ],
}


def main():
    os.makedirs(OUT, exist_ok=True)
    if not os.path.exists(os.path.join(PROJECT, "project.json")):
        run("motion", "create", PROJECT, "--size", f"{W}x{H}", "--fps", "30", "--duration", "6")
    ops = [{"op": "remove", "id": s["id"]} for s in json.loads(run("motion", "info", PROJECT)).get("scenes", [])]
    ops += [dict(op="add_scene", **scene) for scene in (unites, viewport, rings)]
    path = os.path.join(OUT, "ops.json")
    with open(path, "w") as f:
        json.dump(ops, f)
    print(run("motion", "apply", PROJECT, path)[:400])
    print(run("motion", "check", PROJECT)[:2000])
    for scene, times in [("unites", "0.3,0.7,1.1,1.8"), ("viewport", "2.2,2.6,3.4,4.4"), ("rings", "4.6,4.9,5.4,5.9")]:
        run("motion", "snapshot", PROJECT, "--at", times, "--sheet", "--width", "360", "-o", os.path.join(OUT, f"{scene}.png"))
    if not os.environ.get("SKIP_RENDER"):
        run("motion", "render", PROJECT, "-o", os.path.join(OUT, "mesh-shots.mp4"))
    print("wrote", OUT)


if __name__ == "__main__":
    main()
