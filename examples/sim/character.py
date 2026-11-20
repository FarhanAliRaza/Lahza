"""A skinned test character, written as glTF (nothing to download).

    python3 character.py OUT.gltf

Sixteen joints (hips; spine, chest, head; two arms of upper, fore, hand;
two legs of thigh, shin, foot), 1.75 m tall, facing glTF's +z. Its mesh is
a box per bone, each box's points weighted to its bone and, near the
joint at its start, half to the bone before, so joints bend smoothly.
One animation, `walk`: a one-second cycle in place (legs and arms swing,
knees bend), which loops.

`write(path)` is used by the other examples here.
"""
import base64
import json
import math
import struct
import sys

# name, parent, offset from the parent's joint (metres, y up), box half
# width (x) and half depth (z).
BONES = [
    ("hips", None, (0, 0.98, 0), 0.17, 0.10),
    ("spine", "hips", (0, 0.12, 0), 0.15, 0.09),
    ("chest", "spine", (0, 0.20, 0), 0.19, 0.11),
    ("head", "chest", (0, 0.28, 0), 0.10, 0.10),
    ("upper_l", "chest", (0.26, 0.20, 0), 0.05, 0.05),
    ("fore_l", "upper_l", (0, -0.28, 0), 0.045, 0.045),
    ("hand_l", "fore_l", (0, -0.26, 0), 0.04, 0.03),
    ("upper_r", "chest", (-0.26, 0.20, 0), 0.05, 0.05),
    ("fore_r", "upper_r", (0, -0.28, 0), 0.045, 0.045),
    ("hand_r", "fore_r", (0, -0.26, 0), 0.04, 0.03),
    ("thigh_l", "hips", (0.10, -0.06, 0), 0.075, 0.075),
    ("shin_l", "thigh_l", (0, -0.44, 0), 0.06, 0.06),
    ("foot_l", "shin_l", (0, -0.42, 0), 0.05, 0.05),
    ("thigh_r", "hips", (-0.10, -0.06, 0), 0.075, 0.075),
    ("shin_r", "thigh_r", (0, -0.44, 0), 0.06, 0.06),
    ("foot_r", "shin_r", (0, -0.42, 0), 0.05, 0.05),
]
# Where each bone's box ends, from its joint (the next joint, or its own end).
END = {"hips": (0, 0.12, 0), "spine": (0, 0.20, 0), "chest": (0, 0.26, 0), "head": (0, 0.22, 0),
       "hand_l": (0, -0.12, 0), "hand_r": (0, -0.12, 0), "foot_l": (0, -0.06, 0), "foot_r": (0, -0.06, 0)}
COLORS = {"head": (0.95, 0.76, 0.6), "hand": (0.95, 0.76, 0.6), "upper": (0.2, 0.45, 0.9), "fore": (0.2, 0.45, 0.9),
          "chest": (0.2, 0.45, 0.9), "spine": (0.2, 0.45, 0.9), "hips": (0.16, 0.18, 0.28), "thigh": (0.16, 0.18, 0.28),
          "shin": (0.16, 0.18, 0.28), "foot": (0.85, 0.3, 0.2)}

NAMES = [b[0] for b in BONES]


def world():
    at = {}
    for name, parent, off, _, _ in BONES:
        base = at[parent] if parent else (0, 0, 0)
        at[name] = tuple(base[k] + off[k] for k in range(3))
    return at


def quat_x(deg):
    h = math.radians(deg) / 2
    return (math.sin(h), 0.0, 0.0, math.cos(h))


def write(path):
    at = world()
    child = {}
    for name, parent, off, _, _ in BONES:
        if parent and parent not in child and name.split("_")[0] not in ("upper", "thigh"):
            child[parent] = off
    pos, nor, joints, weights, idx = [], [], [], [], []
    groups = {}  # colour → index list
    for j, (name, parent, _, hw, hd) in enumerate(BONES):
        a = at[name]
        end = END.get(name) or child[name]
        b = tuple(a[k] + end[k] for k in range(3))
        lo = [min(a[0], b[0]) - hw, min(a[1], b[1]), min(a[2], b[2]) - hd]
        hi = [max(a[0], b[0]) + hw, max(a[1], b[1]), max(a[2], b[2]) + hd]
        if name.startswith("foot"):
            hi[2] += 0.14  # toes forward
        if name == "hips":
            lo[1] -= 0.08
        color = next(c for k, c in COLORS.items() if name.startswith(k))
        out = groups.setdefault(color, [])
        pj = NAMES.index(parent) if parent else j
        # Six faces, four points each; three rings along y so the box bends.
        ys = [lo[1], (lo[1] + hi[1]) / 2, hi[1]]
        faces = [((1, 0, 0), lambda u, y: (hi[0], y, lo[2] + (hi[2] - lo[2]) * (1 - u))),
                 ((-1, 0, 0), lambda u, y: (lo[0], y, lo[2] + (hi[2] - lo[2]) * u)),
                 ((0, 0, 1), lambda u, y: (lo[0] + (hi[0] - lo[0]) * u, y, hi[2])),
                 ((0, 0, -1), lambda u, y: (lo[0] + (hi[0] - lo[0]) * (1 - u), y, lo[2]))]
        near = a[1]  # the joint's height: points at it blend with the parent
        def add(p, n):
            w = 0.5 if parent and abs(p[1] - near) < 1e-6 and not name.startswith(("upper", "thigh")) else 0.0
            pos.append(p); nor.append(n); joints.append((j, pj, 0, 0)); weights.append((1 - w, w, 0, 0))
            return len(pos) - 1
        for n, f in faces:
            grid = [[add(f(u, y), n) for u in (0, 1)] for y in ys]
            for r in range(2):
                p00, p01, p10, p11 = grid[r][0], grid[r][1], grid[r + 1][0], grid[r + 1][1]
                out += [p00, p01, p11, p00, p11, p10]
        for y, n in ((hi[1], (0, 1, 0)), (lo[1], (0, -1, 0))):
            q = [add((x, y, z), n) for x, z in ((lo[0], lo[2]), (hi[0], lo[2]), (hi[0], hi[2]), (lo[0], hi[2]))]
            out += [q[0], q[2], q[1], q[0], q[3], q[2]] if n[1] > 0 else [q[0], q[1], q[2], q[0], q[2], q[3]]
    ibm = []
    for name in NAMES:
        x, y, z = at[name]
        ibm += [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -x, -y, -z, 1]
    # The walk: nine keys over a second.
    times = [k / 8 for k in range(9)]
    swing = lambda amp, phase: [quat_x(amp * math.sin(2 * math.pi * (t + phase))) for t in times]
    bend = lambda amp, phase: [quat_x(amp * max(0.0, math.sin(2 * math.pi * (t + phase)))) for t in times]
    tracks = {"thigh_l": swing(-28, 0), "thigh_r": swing(-28, 0.5), "shin_l": bend(50, 0.3), "shin_r": bend(50, 0.8),
              "upper_l": swing(22, 0), "upper_r": swing(22, 0.5), "fore_l": bend(-25, 0.25), "fore_r": bend(-25, 0.75)}
    blob, views, accessors = bytearray(), [], []

    def acc(data, fmt, ctype, kind, minmax=None):
        while len(blob) % 4:
            blob.append(0)
        raw = struct.pack("<%d%s" % (len(data), fmt), *data)
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(raw)})
        blob.extend(raw)
        n = {"SCALAR": 1, "VEC3": 3, "VEC4": 4, "MAT4": 16}[kind]
        a = {"bufferView": len(views) - 1, "componentType": ctype, "count": len(data) // n, "type": kind}
        if minmax:
            a["min"], a["max"] = minmax
        accessors.append(a)
        return len(accessors) - 1

    flat = lambda rows: [v for r in rows for v in r]
    lo = [min(p[k] for p in pos) for k in range(3)]
    hi = [max(p[k] for p in pos) for k in range(3)]
    a_pos = acc(flat(pos), "f", 5126, "VEC3", (lo, hi))
    a_nor = acc(flat(nor), "f", 5126, "VEC3")
    a_joints = acc(flat(joints), "H", 5123, "VEC4")
    a_weights = acc(flat(weights), "f", 5126, "VEC4")
    a_ibm = acc(ibm, "f", 5126, "MAT4")
    a_times = acc(times, "f", 5126, "SCALAR", ([0.0], [1.0]))
    prims, materials = [], []
    for color, indices in groups.items():
        materials.append({"pbrMetallicRoughness": {"baseColorFactor": [*color, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.7}})
        prims.append({"attributes": {"POSITION": a_pos, "NORMAL": a_nor, "JOINTS_0": a_joints, "WEIGHTS_0": a_weights},
                      "indices": acc(indices, "I", 5125, "SCALAR"), "material": len(materials) - 1})
    samplers, channels = [], []
    for name, quats in tracks.items():
        samplers.append({"input": a_times, "output": acc(flat(quats), "f", 5126, "VEC4"), "interpolation": "LINEAR"})
        channels.append({"sampler": len(samplers) - 1, "target": {"node": NAMES.index(name), "path": "rotation"}})
    nodes = []
    for name, parent, off, _, _ in BONES:
        kids = [NAMES.index(n) for n, p, *_ in BONES if p == name]
        node = {"name": name, "translation": list(off)}
        if kids:
            node["children"] = kids
        nodes.append(node)
    nodes.append({"name": "body", "mesh": 0, "skin": 0})
    doc = {
        "asset": {"version": "2.0", "generator": "lahza examples/sim/character.py"},
        "scene": 0, "scenes": [{"nodes": [0, len(BONES)]}], "nodes": nodes,
        "skins": [{"joints": list(range(len(BONES))), "inverseBindMatrices": a_ibm, "skeleton": 0}],
        "meshes": [{"primitives": prims}], "materials": materials,
        "animations": [{"name": "walk", "samplers": samplers, "channels": channels}],
        "buffers": [{"uri": "data:application/octet-stream;base64," + base64.b64encode(bytes(blob)).decode(), "byteLength": len(blob)}],
        "bufferViews": views, "accessors": accessors,
    }
    with open(path, "w") as f:
        json.dump(doc, f)
    return path


if __name__ == "__main__":
    print(write(sys.argv[1] if len(sys.argv) > 1 else "character.gltf"))
