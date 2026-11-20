"""Simulation, parts 2 to 4: a skinned character as a ragdoll.

    python3 ragdoll.py OUT_DIR [path/to/lahza-motion]

Writes the test character (character.py), imports it (`model-import`: its
joints become layers, its mesh a skinned mesh) and makes three scenes of
it walking on the spot until a ball strikes it at 0.87 s:

  limp   — every motor off from the hit: it drops as it was
  hold   — motors at full strength hold the walk's last pose: it is
           knocked over stiff, like a statue
  brace  — the same, with behaviours: balance, a step to keep it, arms
           about the head as the ball comes

then bakes, checks, and writes a contact sheet per scene (the CPU renderer
unless GPU=1). RENDER=1 writes an MP4 too.
"""
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import character  # noqa: E402

OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "ragdoll-out")
BIN = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "../../target/release/lahza-motion")
PROJECT = os.path.join(OUT, "ragdoll.lahza")
BACKEND = "--gpu" if os.environ.get("GPU") else "--cpu"
W, H, UNIT, FLOOR = 1280, 720, 240, 640
LEN = 5.0


def run(*args):
    out = subprocess.run([BIN, *args], capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"{' '.join(args)}: {out.stderr or out.stdout}")
    return out.stdout


def apply(ops):
    path = os.path.join(OUT, "ops.json")
    json.dump({"ops": ops}, open(path, "w"))
    return run("motion", "apply", PROJECT, path)


# Joint limits: knees and elbows are hinges about the bone's x (the way
# the character's limbs bend), the rest swing and twist.
LIMITS = [
    {"bone": "shin", "swing": 140, "hinge": [-1, 0, 0]},
    {"bone": "fore", "swing": 140, "hinge": [1, 0, 0]},
    {"bone": "thigh", "swing": 70, "twist": 25},
    {"bone": "upper", "swing": 100, "twist": 60},
    {"bone": "foot", "swing": 35, "twist": 10},
    {"bone": "hand", "swing": 50, "twist": 20},
    {"bone": "head", "swing": 35, "twist": 40},
    {"bone": "spine", "swing": 25, "twist": 20},
    {"bone": "chest", "swing": 25, "twist": 20},
]


def scene(name, start, ragdoll):
    hero = f"{name}-hero"
    doll = dict({"id": "doll", "root": f"{hero}-hips", "release": start + 0.8, "rest": start + 0.0, "limits": LIMITS, "mass": 70}, **ragdoll)
    return {
        "id": name, "start": start, "end": start + LEN, "background": "#eef0f4",
        "camera": {"fov": 38, "orbit": [-28, 10]},
        "lights": [{"from": [-0.5, -1, -0.8], "intensity": 1.3}, {"type": "ambient", "intensity": 0.35}],
        "simulation": {"unit": UNIT, "ragdolls": [doll]},
        "layers": [
            {"id": f"{name}-floor", "kind": "mesh", "mesh": {"type": "box", "size": [3000, 40], "depth": 3000},
             "material": {"preset": "matte", "color": "#d9dce3"}, "transform": {"position": [W / 2, FLOOR + 20]},
             "body": {"type": "fixed", "friction": 0.9}, "allow": ["draws_off_frame"]},
            {"id": f"{name}-ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 42},
             "material": {"preset": "glossy", "color": "#ff5a36"},
             # Keyed in from the right, the simulation's from 0.8 s: it
             # reaches the chest at about 1 s.
             "transform": {"position": {"keys": [{"at": start + 0.3, "v": [W + 100, FLOOR - 330]}, {"at": start + 0.8, "v": [W / 2 + 190, FLOOR - 320], "ease": "linear"}]}},
             "time": {"start": start + 0.3},
             "body": {"release": start + 0.8, "mass": 9, "bounce": 0.2}, "allow": ["draws_off_frame"]},
        ],
    }, hero


SCENES = [
    ("limp", {}),
    ("hold", {"strength": 1}),
    ("brace", {"strength": 1, "behaviours": [
        {"type": "balance"}, {"type": "step"},
        {"type": "protect", "from": 0.55, "to": 1.6},
    ]}),
]

os.makedirs(OUT, exist_ok=True)
model = character.write(os.path.join(OUT, "figure.gltf"))
if os.path.isdir(PROJECT):
    import shutil
    shutil.rmtree(PROJECT)
run("motion", "create", PROJECT, "--size", f"{W}x{H}", "--fps", "30", "--duration", str(LEN * len(SCENES)))
only = os.environ.get("ONLY")
made = []
for k, (name, ragdoll) in enumerate(SCENES):
    if only and name not in only.split(","):
        continue
    start = len(made) * LEN
    # (Behaviours' times are the scene's.)
    for b in ragdoll.get("behaviours", []):
        for key in ("from", "to"):
            if key in b:
                b[key] += start
    s, hero = scene(name, start, ragdoll)
    apply([{"op": "add_scene", "scene": s}])
    run("motion", "model-import", PROJECT, model, "--scene", name, "--id", hero, "--unit", str(UNIT), "--at", str(start),
        "--position", f"{W / 2},{FLOOR - 0.02 * UNIT}")
    made.append((name, start))
apply([{"op": "set", "target": "project.settings.duration", "value": LEN * len(made)}])

baked = json.loads(run("motion", "bake", PROJECT))
for s in baked["scenes"]:
    print(f"{s['scene']}: {s['status']}, {s.get('bodies')} bodies, {s.get('steps')} steps in {s.get('seconds')} s; "
          + ", ".join(f"{e['a']}×{e['b']} {e['t']} s" for e in s.get("events", []) if e["speed"] >= 250)[:300])
print(run("motion", "check", PROJECT, BACKEND))
for name, start in made:
    sheet = os.path.join(OUT, f"{name}.png")
    times = [start + t for t in (0.5, 0.9, 1.1, 1.3, 1.6, 2.0, 2.5, 3.2, 4.8)]
    run("motion", "snapshot", PROJECT, BACKEND, "--sheet", "--at", ",".join(f"{t:.2f}" for t in times), "-o", sheet)
    print("wrote", sheet)
if os.environ.get("RENDER"):
    out = os.path.join(OUT, "ragdoll.mp4")
    run("motion", "render", PROJECT, BACKEND, "-o", out)
    print("wrote", out)
