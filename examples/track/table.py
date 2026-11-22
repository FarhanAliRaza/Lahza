"""Tracking footage: things stood on a table that is in a video.

    python3 table.py OUT_DIR [path/to/lahza-motion]

No footage is needed: the "video" is made here first. A textured table on
a floor is drawn by the engine under a camera that orbits, rolls, dollies
and slides, and rendered to shot.mp4. Then, as with any footage:

  1. `track-plane` follows the table's top from four corners marked on
     one frame, and writes the scene's camera for every frame;
  2. a comp puts the footage in screen space, a shadow-only plane on the
     table (the scene's z = 0), and a box, a ball and a cylinder on it.

Out: shot.mp4 (the footage), track.json (what the tracker said),
table.png (a sheet of the comp at five times) and table.mp4. Set
SKIP_RENDER=1 for the sheet only, CPU=1 to draw without a GPU.

With your own footage the steps are 1 and 2: mark four corners of a real
rectangle on the surface (in source pixels, going round it) on one frame.
"""
import json
import math
import os
import struct
import subprocess
import sys
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "track-out")
BIN = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "../../target/release/lahza-motion")
W, H, FPS, SECONDS = 1280, 720, 30, 3.0
TABLE = (700, 440)
CPU = ["--cpu"] if os.environ.get("CPU") else []


def run(*args):
    out = subprocess.run([BIN, *args], capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"{' '.join(args)}: {out.stderr or out.stdout}")
    return out.stdout


def keys(*pairs, ease="linear"):
    return {"keys": [{"at": t, "v": v, "ease": ease} for t, v in pairs]}


# ------------------------------------------------------------ a texture
def hash01(x, y, s):
    v = (x * 0x9E3779B97F4A7C15 ^ y * 0xC2B2AE3D27D4EB4F ^ s * 0x165667B19E3779F9) & 0xFFFFFFFFFFFFFFFF
    v ^= v >> 31
    v = (v * 0x7FB5D329728EA185) & 0xFFFFFFFFFFFFFFFF
    v ^= v >> 27
    return (v >> 11) / float(1 << 53)


def noise(x, y, cell, s):
    fx, fy = x / cell, y / cell
    ix, iy = math.floor(fx), math.floor(fy)
    tx, ty = fx - ix, fy - iy
    sx, sy = tx * tx * (3 - 2 * tx), ty * ty * (3 - 2 * ty)
    top = hash01(ix, iy, s) + (hash01(ix + 1, iy, s) - hash01(ix, iy, s)) * sx
    bottom = hash01(ix, iy + 1, s) + (hash01(ix + 1, iy + 1, s) - hash01(ix, iy + 1, s)) * sx
    return top + (bottom - top) * sy


def texture(path, w, h, tint, seed):
    """Wood-ish noise with a few things lying on it, as an RGB PNG."""
    rows = bytearray()
    for y in range(h):
        rows.append(0)
        for x in range(w):
            v = 0.45 * noise(x, y, 90, seed) + 0.3 * noise(x, y, 31, seed + 1) + 0.25 * noise(x, y, 11, seed + 2)
            cx, cy = x // 70, y // 70
            if hash01(cx, cy, seed + 9) < 0.5:
                ox = (cx + 0.3 + 0.4 * hash01(cx, cy, seed + 10)) * 70
                oy = (cy + 0.3 + 0.4 * hash01(cx, cy, seed + 11)) * 70
                r = 9 + 12 * hash01(cx, cy, seed + 12)
                if hash01(cx, cy, seed + 13) < 0.5:
                    d = max(abs(x - ox) - r, abs(y - oy) - r * 0.7)
                else:
                    d = math.hypot(x - ox, y - oy) - r
                cover = min(1.0, max(0.0, 0.5 - d / 2))
                v += (0.1 + 0.8 * hash01(cx, cy, seed + 14) - v) * cover
            g = 40 + 180 * v
            rows += bytes(int(min(255, g * c)) for c in tint)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(bytes(rows), 6)) + chunk(b"IEND", b""))


# ----------------------------------------------- the camera of the shot
def camera(t):
    k = t / SECONDS
    return dict(fov=45.0, orbit=[-16 + 30 * k, -58 + 10 * k], dolly=120 * k,
                position=[600 + 80 * k, 385 - 35 * k], rotation=-3 + 7 * k)


def project(cam, p):
    """Where the engine's camera shows a scene point (x, y, z)."""
    f = H / 2 / math.tan(math.radians(cam["fov"]) / 2)
    x, y, z = p[0] - cam["position"][0], p[1] - cam["position"][1], p[2]
    a = math.radians(-cam["orbit"][0])  # yaw, about y
    x, z = math.cos(a) * x - math.sin(a) * z, math.sin(a) * x + math.cos(a) * z
    a = math.radians(-cam["orbit"][1])  # pitch, about x
    y, z = math.cos(a) * y + math.sin(a) * z, -math.sin(a) * y + math.cos(a) * z
    a = math.radians(-cam["rotation"])  # roll, about z
    x, y = math.cos(a) * x - math.sin(a) * y, math.sin(a) * x + math.cos(a) * y
    z += f - cam["dolly"]
    return [f * x / z + W / 2, f * y / z + H / 2]


def main():
    os.makedirs(OUT, exist_ok=True)
    # ---- the footage
    shot = os.path.join(OUT, "shot.lahza")
    if not os.path.exists(os.path.join(shot, "project.json")):
        run("motion", "create", shot, "--size", f"{W}x{H}", "--fps", str(FPS), "--duration", str(SECONDS))
    os.makedirs(os.path.join(shot, "assets"), exist_ok=True)
    texture(os.path.join(shot, "assets/table.png"), TABLE[0], TABLE[1], (1.0, 0.86, 0.7), 1)
    texture(os.path.join(shot, "assets/floor.png"), 2000, 1400, (0.62, 0.66, 0.74), 40)
    a, b = camera(0), camera(SECONDS)
    ops = [{"op": "remove", "id": s["id"]} for s in json.loads(run("motion", "info", shot)).get("scenes", [])]
    ops.append({"op": "add_scene", "id": "s1", "background": "#20242c",
                "camera": {"fov": 45, **{k: keys((0, a[k]), (SECONDS, b[k])) for k in ("orbit", "dolly", "position", "rotation")}},
                "layers": [
                    {"id": "floor", "kind": "image", "src": "assets/floor.png", "three_d": True,
                     "transform": {"position": [W / 2, H / 2], "z": 260}},
                    {"id": "table", "kind": "image", "src": "assets/table.png", "three_d": True,
                     "transform": {"position": [W / 2, H / 2]}}]})
    with open(os.path.join(OUT, "shot-ops.json"), "w") as f:
        json.dump(ops, f)
    run("motion", "apply", shot, os.path.join(OUT, "shot-ops.json"))
    video = os.path.join(OUT, "shot.mp4")
    run("motion", "render", shot, "-o", video, "--no-audio", *CPU)

    # ---- the comp: the footage, and the camera the tracker gives
    comp = os.path.join(OUT, "table.lahza")
    if not os.path.exists(os.path.join(comp, "project.json")):
        run("motion", "create", comp, "--size", f"{W}x{H}", "--fps", str(FPS), "--duration", str(SECONDS))
    ops = [{"op": "remove", "id": s["id"]} for s in json.loads(run("motion", "info", comp)).get("scenes", [])]
    ops.append({"op": "add_scene", "id": "s1", "background": "#000000"})
    with open(os.path.join(OUT, "comp-ops.json"), "w") as f:
        json.dump(ops, f)
    run("motion", "apply", comp, os.path.join(OUT, "comp-ops.json"))
    # The table's corners as frame 45 shows them: what a person would
    # click, or an agent read off a frame.
    marked = 1.5
    tx, ty = TABLE[0] / 2, TABLE[1] / 2
    corners = [project(camera(marked), [W / 2 + sx * tx, H / 2 + sy * ty, 0]) for sx, sy in ((-1, -1), (1, -1), (1, 1), (-1, 1))]
    out = run("motion", "track-plane", video, "--corners", ",".join(f"{v:.2f}" for p in corners for v in p),
              "--at", str(marked), "--project", comp, "--scene", "s1", "--apply")
    track = json.loads(out)
    with open(os.path.join(OUT, "track.json"), "w") as f:
        json.dump(track, f, indent=1)
    cam, foot = track["camera"], track["footage"]
    print(f"tracked {track['found']} of {track['frames']} frames at {track['frames_per_second']:.0f} a second; "
          f"lens {cam['lens']} at {cam['fov']:.1f} degrees; camera within {cam['error_px']['worst']:.2f} px")

    # ---- what stands on the table. The rectangle is on z = 0; toward
    # the viewer is -z, so a thing d deep sits at z = -d / 2.
    (x0, y0), (x1, y1) = cam["rect"][0], cam["rect"][2]
    cx, cy, rw, rh = (x0 + x1) / 2, (y0 + y1) / 2, x1 - x0, y1 - y0
    u = rh / 10
    layers = [
        {"id": "footage", "kind": "video", "src": os.path.relpath(video, comp), "space": "screen", "mute": True,
         "size": foot["size"], "transform": {"position": foot["position"]}},
        {"id": "shadows", "kind": "mesh", "mesh": {"type": "plane", "size": [rw * 1.5, rh * 1.5]},
         "material": {"preset": "shadow", "opacity": 0.5}, "transform": {"position": [cx, cy], "z": 0}},
        {"id": "box", "kind": "mesh", "mesh": {"type": "box", "size": [2.6 * u, 2.0 * u], "depth": 2.2 * u, "bevel": 0.18 * u},
         "material": {"preset": "glossy", "color": "#ff5a4f"},
         "transform": {"position": [cx - 0.25 * rw, cy - 0.1 * rh], "z": -1.1 * u, "rotation": 18}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 1.1 * u},
         "material": {"preset": "coated", "color": "#2d7bff"},
         "transform": {"position": [cx + 0.05 * rw, cy + 0.22 * rh], "z": -1.1 * u}},
        {"id": "can", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 0.8 * u, "height": 3.2 * u},
         "material": {"preset": "metal", "color": "#e8d9b0"},
         "transform": {"position": [cx + 0.28 * rw, cy - 0.15 * rh], "z": -1.6 * u, "rotation_x": 90}},
    ]
    ops = [{"op": "set", "target": "s1.environment", "value": {"preset": "soft"}},
           {"op": "set", "target": "s1.lights", "value": [{"from": [-0.6, -0.5, -1.0], "softness": 0.5}, {"from": [0.7, 0.2, -0.6], "intensity": 0.4}]}]
    ops += [dict(op="add", scene="s1", **layer) for layer in layers]
    with open(os.path.join(OUT, "comp-layers.json"), "w") as f:
        json.dump(ops, f)
    print(run("motion", "apply", comp, os.path.join(OUT, "comp-layers.json"))[:300])
    run("motion", "snapshot", comp, "--at", "0,0.75,1.5,2.25,2.95", "--sheet", "--width", "640", *CPU, "-o", os.path.join(OUT, "table.png"))
    if not os.environ.get("SKIP_RENDER"):
        run("motion", "render", comp, "-o", os.path.join(OUT, "table.mp4"), "--no-audio", *CPU)
    print("wrote", OUT)


if __name__ == "__main__":
    main()
