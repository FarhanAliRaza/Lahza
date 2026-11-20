"""Simulation, part 7: a crowd.

    python3 crowd.py OUT_DIR [N] [path/to/lahza-motion]

Imports the test character (character.py) N times (default 12) as the
agents of a crowd that crosses the floor round a pillar, each playing the
walk by the distance it covers. A ball is thrown through them at 2 s:
whoever it strikes goes down as a ragdoll. Bakes, checks, writes a
contact sheet, and times the bake and (TIME=1) a second of export on the
CPU renderer.
"""
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import character  # noqa: E402

OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "crowd-out")
N = int(sys.argv[2]) if len(sys.argv) > 2 else 12
BIN = sys.argv[3] if len(sys.argv) > 3 else os.path.join(HERE, "../../target/release/lahza-motion")
PROJECT = os.path.join(OUT, "crowd.lahza")
BACKEND = "--gpu" if os.environ.get("GPU") else "--cpu"
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
cols = max(1, round(N ** 0.5))
agents = [f"p{i}" for i in range(N)]
# The walk covers about 1.6 m a cycle (legs of 0.86 m swinging 28° each way).
stride = 1.6 * UNIT
crowd = {
    "id": "walkers", "agents": agents, "speed": stride, "vary": 0.15, "radius": 0.28 * UNIT, "height": 1.75 * UNIT,
    "stride": stride, "cycle": 1.0, "knock": 4 * UNIT,
    "goals": [[W / 2 + 520 + (i // cols) * 0.8 * UNIT, -260 + (i % cols) * 0.9 * UNIT] for i in range(N)],
    "ragdoll": {"mass": 70, "limits": [
        {"bone": "shin", "swing": 140, "hinge": [-1, 0, 0]}, {"bone": "fore", "swing": 140, "hinge": [1, 0, 0]},
        {"bone": "thigh", "swing": 70, "twist": 25}, {"bone": "upper", "swing": 100, "twist": 60}]},
}
scene = {
    "id": "s1", "start": 0, "end": LEN, "background": "#eef0f4",
    "camera": {"fov": 40, "orbit": [-18, 24], "dolly": -260},
    "lights": [{"from": [-0.5, -1, -0.8], "intensity": 1.3}, {"type": "ambient", "intensity": 0.35}],
    "simulation": {"unit": UNIT, "crowds": [crowd]},
    "layers": [
        {"id": "floor", "kind": "mesh", "mesh": {"type": "box", "size": [5000, 40], "depth": 5000},
         "material": {"preset": "matte", "color": "#d9dce3"}, "transform": {"position": [W / 2, FLOOR + 20]},
         "body": {"type": "fixed", "friction": 0.9}, "allow": ["draws_off_frame"]},
        {"id": "pillar", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 0.5 * UNIT, "height": 2.4 * UNIT},
         "material": {"preset": "plastic", "color": "#ffc800"}, "transform": {"position": [W / 2, FLOOR - 1.2 * UNIT], "z": 0},
         "body": {"type": "fixed"}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 0.3 * UNIT},
         "material": {"preset": "glossy", "color": "#ff5a36"},
         "transform": {"position": {"keys": [{"at": 1.5, "v": [W / 2 - 330, FLOOR - 1.0 * UNIT]}, {"at": 2.0, "v": [W / 2 - 330, FLOOR - 1.0 * UNIT]}]},
                       "z": {"keys": [{"at": 1.5, "v": -900}, {"at": 2.0, "v": -500, "ease": "linear"}]}},
         "time": {"start": 1.5}, "body": {"release": 2.0, "mass": 25, "bounce": 0.2}, "allow": ["draws_off_frame"]},
    ],
}
apply([{"op": "add_scene", "scene": scene}])
began = time.time()
for i, a in enumerate(agents):
    x = W / 2 - 560 - (i // cols) * 0.9 * UNIT
    z = -220 + (i % cols) * 0.9 * UNIT
    run("motion", "model-import", PROJECT, model, "--scene", "s1", "--id", a, "--unit", str(UNIT), "--position", f"{x},{FLOOR}", "--z", str(z))
print(f"{N} agents imported in {time.time() - began:.1f} s")
# (The imports made the scene's layers; the crowd names them.)
baked = json.loads(run("motion", "bake", PROJECT))["scenes"][0]
knocked = [e["a"] for e in baked["events"] if e["kind"] == "knock"]
print(f"baked: {baked['bodies']} bodies, {baked['steps']} steps in {baked['seconds']} s, {baked['bytes']} bytes, {baked['tracks']} tracks; knocked down: {knocked}")
print(run("motion", "check", PROJECT, BACKEND).strip())
sheet = os.path.join(OUT, "crowd.png")
began = time.time()
run("motion", "snapshot", PROJECT, BACKEND, "--sheet", "--at", "0.5,1.5,2.2,2.6,3.2,4.0,5.0,6.0,7.5", "-o", sheet)
print(f"wrote {sheet} (9 frames in {time.time() - began:.1f} s)")
if os.environ.get("TIME"):
    out = os.path.join(OUT, "second.mp4")
    began = time.time()
    run("motion", "render", PROJECT, BACKEND, "--from", "2", "--to", "3", "--no-audio", "-o", out)
    print(f"a second of export (30 frames, {N} agents): {time.time() - began:.1f} s")
if os.environ.get("RENDER"):
    out = os.path.join(OUT, "crowd.mp4")
    run("motion", "render", PROJECT, BACKEND, "-o", out)
    print("wrote", out)
