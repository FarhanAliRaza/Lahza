"""Simulation, part 7: a crowd of instances.

    python3 herd.py OUT_DIR [N] [path/to/lahza-motion]

Imports the test character (character.py) once and makes it the
character of a crowd of N (default 1000) that marches across a field
round a pillar, each agent at its own moment of the walk. The nearest
FULL (default 150) are drawn as the character, the rest as pictures of
it. A ball is thrown through them at 2 s: whoever it strikes goes down as
a ragdoll of the pool. Bakes, checks (CHECK=1), writes a contact sheet,
and times the bake and (TIME=1) a second of export on the CPU renderer.
"""
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import character  # noqa: E402

OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "herd-out")
N = int(sys.argv[2]) if len(sys.argv) > 2 else 1000
BIN = sys.argv[3] if len(sys.argv) > 3 else os.path.join(HERE, "../../target/release/lahza-motion")
PROJECT = os.path.join(OUT, "herd.lahza")
BACKEND = "--gpu" if os.environ.get("GPU") else "--cpu"
FULL = int(os.environ.get("FULL", "150"))
W, H, UNIT, FLOOR, LEN = 1280, 720, 90, 560, 8.0


def run(*args):
    out = subprocess.run([BIN, *args], capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"{' '.join(args)}: {out.stderr or out.stdout}")
    return out.stdout


def apply(ops):
    path = os.path.join(OUT, "ops.json")
    json.dump({"ops": ops}, open(path, "w"))
    return run("motion", "apply", PROJECT, path)


os.makedirs(OUT, exist_ok=True)
model = character.write(os.path.join(OUT, "figure.gltf"))
if os.path.isdir(PROJECT):
    import shutil
    shutil.rmtree(PROJECT)
run("motion", "create", PROJECT, "--size", f"{W}x{H}", "--fps", "30", "--duration", str(LEN))
# A square field with a metre and a bit a head; they march a third of it.
side = max(6.0, (N ** 0.5) * 1.15) * UNIT
stride = 1.6 * UNIT
crowd = {
    "id": "walkers", "character": "figure", "count": N, "area": [W / 2 - side / 2, -side / 2, W / 2 + side / 2, side / 2],
    "march": [0.25 * side, -0.35 * side], "full": FULL,
    "speed": stride, "vary": 0.15, "radius": 0.28 * UNIT, "height": 1.75 * UNIT, "stride": stride, "cycle": 1.0, "knock": 4 * UNIT,
    "ragdoll": {"mass": 70, "max": 12, "limits": [
        {"bone": "shin", "swing": 140, "hinge": [-1, 0, 0]}, {"bone": "fore", "swing": 140, "hinge": [1, 0, 0]},
        {"bone": "thigh", "swing": 70, "twist": 25}, {"bone": "upper", "swing": 100, "twist": 60}]},
}
scene = {
    "id": "s1", "start": 0, "end": LEN, "background": "#eef0f4",
    "camera": {"fov": 40, "orbit": [-18, 28], "dolly": -0.75 * side, "position": [W / 2, FLOOR - 0.1 * side]},
    "lights": [{"from": [-0.5, -1, -0.8], "intensity": 1.3}, {"type": "ambient", "intensity": 0.35}],
    "simulation": {"unit": UNIT, "crowds": [crowd]},
    "layers": [
        {"id": "floor", "kind": "mesh", "mesh": {"type": "box", "size": [4 * side, 40], "depth": 4 * side},
         "material": {"preset": "matte", "color": "#d9dce3"}, "transform": {"position": [W / 2, FLOOR + 20]},
         "body": {"type": "fixed", "friction": 0.9}, "allow": ["draws_off_frame"]},
        {"id": "pillar", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 0.5 * UNIT, "height": 2.4 * UNIT},
         "material": {"preset": "plastic", "color": "#ffc800"}, "transform": {"position": [W / 2 + 0.1 * side, FLOOR - 1.2 * UNIT], "z": 0.3 * side},
         "body": {"type": "fixed"}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 0.3 * UNIT},
         "material": {"preset": "glossy", "color": "#ff5a36"},
         "transform": {"position": {"keys": [{"at": 1.5, "v": [W / 2 - 0.1 * side, FLOOR - 1.0 * UNIT]}, {"at": 2.0, "v": [W / 2 - 0.1 * side, FLOOR - 1.0 * UNIT]}]},
                       "z": {"keys": [{"at": 1.5, "v": 0.5 * side + 900}, {"at": 2.0, "v": 0.5 * side + 300, "ease": "linear"}]}},
         "time": {"start": 1.5}, "body": {"release": 2.0, "mass": 25, "bounce": 0.2}, "allow": ["draws_off_frame"]},
    ],
}
apply([{"op": "add_scene", "scene": scene}])
# One character, where the field's corner is: its level, turn and scale are every agent's.
run("motion", "model-import", PROJECT, model, "--scene", "s1", "--id", "figure", "--unit", str(UNIT), "--position", f"{W / 2},{FLOOR}", "--z", "0")
began = time.time()
baked = json.loads(run("motion", "bake", PROJECT))["scenes"][0]
knocked = [e["a"] for e in baked["events"] if e["kind"] == "knock"]
print(f"baked {N} agents: {baked['bodies']} bodies, {baked['steps']} steps in {baked['seconds']} s ({time.time() - began:.1f} s with the file), {baked['bytes']} bytes, {baked['tracks']} tracks; knocked down: {knocked}")
if os.environ.get("CHECK"):
    began = time.time()
    print(run("motion", "check", PROJECT, BACKEND).strip(), f"({time.time() - began:.1f} s)")
sheet = os.path.join(OUT, "herd.png")
began = time.time()
run("motion", "snapshot", PROJECT, BACKEND, "--sheet", "--at", "0.5,2.2,2.6,3.2,5.0,7.5", "-o", sheet)
print(f"wrote {sheet} (6 frames in {time.time() - began:.1f} s)")
if os.environ.get("TIME"):
    out = os.path.join(OUT, "second.mp4")
    began = time.time()
    run("motion", "render", PROJECT, BACKEND, "--from", "2", "--to", "3", "--no-audio", "-o", out)
    print(f"a second of export (30 frames, {N} agents, {FULL} of them meshes): {time.time() - began:.1f} s")
if os.environ.get("RENDER"):
    out = os.path.join(OUT, "herd.mp4")
    run("motion", "render", PROJECT, BACKEND, "-o", out)
    print("wrote", out)
