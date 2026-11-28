"""Olga of Kyiv: a 65-second vertical short in the old hand-inked cel style,
built entirely through the Lahza motion engine.

    python3 film.py OUT_DIR [sheet|render|draft]

OUT_DIR holds manifest.json and vo/ from narrate.py. `sheet` (default)
builds the project and renders the contact sheet (one still per shot);
`render` renders the film; `draft` renders it at half size.

One scene holds the film: each shot is a timed comp inside a master comp
that carries the old-cartoon finish; shots move between each other with
blur-whips (push + directional blur, 4 frames) and the captions sit above
the finish so they stay crisp. The camera is keyed per depth layer (sky,
back, mid, fore) for parallax.
"""
import json
import math
import os
import random
import subprocess
import sys
import time

import soundfile as sf

from kit import *  # noqa: F401,F403
import cast
import props
import sets
from face import sparkle

OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "olga-film")
MODE = sys.argv[2] if len(sys.argv) > 2 else "sheet"
PROJECT = os.path.join(OUT, "olga.lahza")
W, H, FPS = 1080, 1920, 30
F = 1 / FPS
C = (W / 2, H / 2)

# ------------------------------------------------------------------ timing ----

MAN = json.load(open(os.path.join(OUT, "manifest.json")))
LEAD, PAUSE = MAN["lead"], MAN["pause"]
WORDS = {}      # (line, i) -> (start, end, text) absolute seconds
LINE_START = {}
t = LEAD
for clip in MAN["clips"]:
    audio, sr = sf.read(os.path.join(OUT, clip["audio"]))
    LINE_START[clip["id"]] = t
    for i, w in enumerate(clip["words"]):
        WORDS[(clip["id"], i)] = (t + w["start"], t + w["end"], w["text"])
    t += len(audio) / sr + PAUSE
SPEECH_END = max(e for _, e, _ in WORDS.values())
END = round(SPEECH_END + 2.4, 2)


def w(line, i):
    """When word i of a line starts."""
    return WORDS[(line, i)][0]


def we(line, i):
    return WORDS[(line, i)][1]


def cue(line, word):
    """When `word` (first match) is said in a line."""
    for (l_, i), (s, e, tx) in sorted(WORDS.items(), key=lambda kv: kv[1][0]):
        if l_ == line and tx.strip(".,:;!?").lower() == word.lower():
            return s
    raise KeyError((line, word))


# ------------------------------------------------------------------ camera ----

DEPTH = {"sky": 0.15, "back": 0.55, "mid": 1.0, "fore": 1.3}


def cam_comp(id, layers, p, cam, blur=0.0):
    """A depth layer seen by the camera `cam` [(t, cx, cy, zoom, ease)]: it
    moves `p` times as much as the subject plane (parallax)."""
    c0 = (cam[0][1], cam[0][2])
    anchor, scale = [], []
    for k, (tt, cx, cy, z, ease) in enumerate(cam):
        a = (c0[0] + (cx - c0[0]) * p, c0[1] + (cy - c0[1]) * p)
        zp = 1 + (z - 1) * p
        ka = {"at": round(tt, 3), "v": r2(a)}
        ks = {"at": round(tt, 3), "v": [round(zp, 4), round(zp, 4)]}
        if k:
            ka["ease"] = ease
            ks["ease"] = "zoom" if ease in ("glide", "gesture", "zoom") else ease
        anchor.append(ka)
        scale.append(ks)
    shake = [{"type": "wiggle", "hz": 0.9, "amp": round(6 * p, 2), "seed": 77}]
    t_ = {"position": [C[0], C[1]], "anchor": {"keys": anchor, "add": shake} if len(anchor) > 1 else {"base": anchor[0]["v"], "add": shake},
          "scale": {"keys": scale} if len(scale) > 1 else scale[0]["v"]}
    layer = {"id": id, "kind": "comp", "transform": t_, "layers": layers}
    if blur > 0:
        layer["effects"] = [{"type": "blur", "radius": blur}]
    return layer


def clamp_cam(cx, cy, z, x0=-150, y0=-250, x1=1650, y1=2550):
    hw, hh = W / 2 / z, H / 2 / z
    cx = min(max(cx, x0 + hw), x1 - hw)
    cy = min(max(cy, y0 + hh), y1 - hh)
    return cx, cy


def cam(*keys, clamp=True):
    """Camera keys [(t, cx, cy, z[, ease])], kept inside the set."""
    out = []
    for k in keys:
        tt, cx, cy, z = k[:4]
        ease = k[4] if len(k) > 4 else "glide"
        if clamp:
            cx, cy = clamp_cam(cx, cy, z)
        out.append((tt, cx, cy, z, ease))
    return out


# ----------------------------------------------------------------- acting ----

def ks(pairs, ease="pop", dur=None):
    """Keys from [(t, v)] (or (t, v, ease)); the first holds."""
    out = []
    for k, p in enumerate(pairs):
        e = p[2] if len(p) > 2 else ease
        key = {"at": round(p[0], 3), "v": p[1]}
        if k:
            key["ease"] = e
            if dur and e not in ("hold", "linear"):
                key["dur"] = dur
        out.append(key)
    return {"keys": out}


def mood_track(t0, t1, beats, blink_gap=0.75):
    """Expression keys from beats [(t, mood)], with blinks filling any hold
    longer than `blink_gap` so a face changes every 0.3–0.8 s."""
    beats = sorted(beats)
    out = []
    for k, (tt, m) in enumerate(beats):
        nxt = beats[k + 1][0] if k + 1 < len(beats) else t1
        out.append((tt, m))
        if m in ("blink", "shock", "fear"):
            continue
        s = tt + 0.55
        while nxt - s > 0.35:
            out.append((s, "blink"))
            out.append((s + 0.12, m))
            s += 0.12 + blink_gap * random.Random(int(s * 100)).uniform(0.6, 1.0)
    return out


def arm_deg(side, deg):
    """Rotation that raises an arm `deg` outward (side −1 left, 1 right)."""
    return -side * deg


def find(layer, id):
    if layer.get("id") == id:
        return layer
    for c in layer.get("layers", []):
        r = find(c, id)
        if r:
            return r
    return None


NOW = (0.0, 1.0)  # the shot being built (t0, t1): extras and beats use it


def beat_keys(t0, t1, amp, seed, every=(0.35, 0.55), shape=None):
    """Acting beats: a small pose change every 0.35–0.55 s that snaps with
    a little overshoot (alternating sides), so nothing on screen holds."""
    rnd = random.Random(seed)
    keys = [{"at": round(t0, 3), "v": 0}]
    t, side = t0 + rnd.uniform(0.1, 0.3), 1
    while t < t1:
        v = side * amp * rnd.uniform(0.5, 1.0)
        keys.append({"at": round(t, 3), "v": shape(v) if shape else round(v, 2), "ease": "pop", "dur": "6f"})
        side = -side if rnd.random() < 0.8 else side
        t += rnd.uniform(*every)
    if shape:
        keys[0]["v"] = shape(0)
    return {"keys": keys}


def beat_wrap(layer, id, t0, t1, rot_amp=0.0, bob_amp=0.0, seed=0):
    """Wrap a comp's layers in a beat comp (rotation about its origin, a
    vertical bob), leaving the comp's own keys alone."""
    inner = {"id": id, "kind": "comp", "transform": {}, "layers": layer["layers"]}
    if rot_amp:
        inner["transform"]["rotation"] = beat_keys(t0, t1, rot_amp, seed)
    if bob_amp:
        inner["transform"]["position"] = beat_keys(t0, t1, bob_amp, seed + 101, shape=lambda v: [0, -abs(round(v, 2))])
    layer["layers"] = [inner]
    return layer


def act(rig, head=None, body=None, rot=None, sway=1.5, bob=3.0, seed=0, flip=False, scale=1.0, t0=None, t1=None):
    """Head rotation keys, whole-body position/rotation keys and a breathing
    sway on a built rig, plus acting beats every 0.35–0.55 s."""
    id = rig["id"]
    t0, t1 = (t0, t1) if t0 is not None else NOW
    h = find(rig, f"{id}-head")
    if h:
        beat_wrap(h, f"{id}-headbeat", t0, t1, rot_amp=4.0, seed=seed + 11)
    beat_wrap(rig, f"{id}-bodybeat", t0, t1, bob_amp=7.0, seed=seed + 12)
    if h:
        base = ks(head) if head else {"base": 0}
        base["add"] = [{"type": "wiggle", "hz": 0.7, "amp": sway, "seed": seed + 1}]
        h["transform"]["rotation"] = base
    pos = rig["transform"]["position"]
    if body:
        pk = ks([(tt, r2((pos[0] + dx, pos[1] + dy))) + tuple(rest) for tt, (dx, dy), *rest in body])
    else:
        pk = {"base": pos}
    pk["add"] = [{"type": "wiggle", "hz": 0.5, "amp": bob, "seed": seed + 2}]
    rig["transform"]["position"] = pk
    if rot:
        rig["transform"]["rotation"] = ks(rot)
    if flip or scale != 1:
        rig["transform"]["scale"] = [-scale if flip else scale, scale]
    return rig


# ------------------------------------------------------------------ shots ----

SHOTS = []


def shot(sid, line, idx, **spec):
    SHOTS.append(dict(sid=sid, line=line, idx=idx, **spec))


# Shot starts: just before the clause's first word (a picture arrives with
# its words); ends at the next shot's start.
def build_times():
    for k, s in enumerate(SHOTS):
        s["t0"] = 0.0 if k == 0 else round(w(s["line"], s["idx"]) - 0.1, 3)
    for k, s in enumerate(SHOTS):
        s["t1"] = SHOTS[k + 1]["t0"] if k + 1 < len(SHOTS) else END


def olga_rig(id, pos, scale, t0, t1, beats, arm_l=None, arm_r=None, hand_l="rest", hand_r="rest", head=None, body=None, rot=None, flip=False, seed=0):
    r = cast.olga(id, pos, 1.0, mood_keys=mood_track(t0, t1, beats), arm_l=ks(arm_l) if arm_l else None, arm_r=ks(arm_r) if arm_r else None,
                  hand_l=hand_l, hand_r=hand_r)
    return act(r, head, body, rot, seed=seed, flip=flip, scale=scale, t0=t0, t1=t1)


def envoy_rig(id, pos, scale, t0, t1, beats, arm_l=None, arm_r=None, hand_l="fist", hand_r="fist", head=None, body=None, rot=None, flip=False, seed=0, tunic=RED):
    r = cast.envoy(id, pos, 1.0, mood_keys=mood_track(t0, t1, beats), arm_l=ks(arm_l) if arm_l else None, arm_r=ks(arm_r) if arm_r else None,
                   hand_l=hand_l, hand_r=hand_r, tunic=tunic)
    return act(r, head, body, rot, seed=seed, flip=flip, scale=scale, t0=t0, t1=t1)


def igor_rig(id, pos, scale, t0, t1, beats, arm_l=None, arm_r=None, hand_l="fist", hand_r="open", head=None, body=None, rot=None, flip=False, seed=0):
    r = cast.igor(id, pos, 1.0, mood_keys=mood_track(t0, t1, beats), arm_l=ks(arm_l) if arm_l else None, arm_r=ks(arm_r) if arm_r else None,
                  hand_l=hand_l, hand_r=hand_r)
    return act(r, head, body, rot, seed=seed, flip=flip, scale=scale, t0=t0, t1=t1)


def extra(kind, id, pos, scale=1.0, flip=False, mood="neutral", bob=4, seed=0, **kw):
    r = (cast.guard if kind == "guard" else cast.drevlian)(id, pos, scale, flip=flip, mood=mood, **kw)
    pk = {"base": r["transform"]["position"], "add": [{"type": "wiggle", "hz": 0.6, "amp": bob, "seed": seed}]}
    r["transform"]["position"] = pk
    # Extras sway from their feet in beats too (fewer marks, same life).
    beat_wrap(r, f"{id}-beat", NOW[0], NOW[1], rot_amp=2.5, bob_amp=4.0, seed=seed_of(id))
    return r


def screen(id, layers, t0, t1, drift=(0, -60), zoom=(1.0, 1.2)):
    """Layers fixed to the frame (inserts), with their own push and a
    handheld drift."""
    pos = ks([(t0, [0, 0]), (t1, list(drift), "linear")])
    pos["add"] = [{"type": "wiggle", "hz": 1.1, "amp": 8, "seed": seed_of(id)}]
    return {"id": id, "kind": "comp", "transform": {
        "position": pos,
        "anchor": [C[0], C[1]],
        "scale": ks([(t0, [zoom[0], zoom[0]]), (t1, [zoom[1], zoom[1]], "linear")])}, "layers": [
        {"id": f"{id}-in", "kind": "comp", "transform": {"position": [C[0], C[1]]}, "layers": layers}]}


# --------------------------------------------------------------- the film ----
# Each shot: who/what, an expression, a body action, a camera move. Cycle
# close-up → wide → insert → reaction.

def tremble(layer, hz=7, deg=1.6, seed=0):
    """Fear: a fast shiver of the whole body."""
    layer["transform"]["rotation"] = {"base": 0, "add": [{"type": "wiggle", "hz": hz, "amp": deg, "seed": seed}]}
    return layer


def flapper(id, kind, s_, t0, t1, at, bundle=False, lit=False, flap=0.083):
    """A perched bird that flaps on twos and hops in beats."""
    draw = props.pigeon if kind == "pigeon" else props.sparrow
    up = comp(f"{id}-u", draw(f"{id}-u", 0, s_, bundle, lit))
    dn = comp(f"{id}-d", draw(f"{id}-d", 1, s_, bundle, lit))
    ku, kd, t, k = [], [], t0, 0
    while t < t1:
        ku.append({"at": round(t, 3), "v": 1 if k % 2 else 0, "ease": "hold"})
        kd.append({"at": round(t, 3), "v": 0 if k % 2 else 1, "ease": "hold"})
        t += flap * (1 if (k // 6) % 2 == 0 else 3)  # bursts of flapping, then a rest
        k += 1
    up["transform"]["opacity"] = {"keys": ku}
    dn["transform"]["opacity"] = {"keys": kd}
    pos = beat_keys(t0, t1, 18, seed_of(id) + 5, shape=lambda v: [round(at[0], 2), round(at[1] - abs(v), 2)])
    return {"id": id, "kind": "comp", "transform": {"position": pos, "rotation": beat_keys(t0, t1, 8, seed_of(id))}, "layers": [up, dn]}


def S01(s):  # Twenty men sit in a boat at the bottom of a pit, — the boat in the pit: men cower, peek up, flinch; dirt trickles from the rim
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    cower, peek = 150, 95
    env = envoy_rig(f"{s['sid']}e", (590, 1600), 0.62, t0, t1,
                    [(t0, "fear"), (t0 + 0.45, "shock"), (t0 + 0.9, "fear"), (t0 + 1.4, "shock"), (t0 + 1.9, "fear"), (t0 + 2.3, "shock")],
                    head=[(t0, -16), (t0 + 0.45, -4), (t0 + 0.9, -18), (t0 + 1.4, 8), (t0 + 1.9, -14), (t0 + 2.3, -22)],
                    arm_l=[(t0, arm_deg(-1, 40)), (t0 + 0.45, arm_deg(-1, cower)), (t0 + 0.9, arm_deg(-1, peek)), (t0 + 1.4, arm_deg(-1, cower)), (t0 + 1.9, arm_deg(-1, peek)), (t0 + 2.3, arm_deg(-1, cower))],
                    arm_r=[(t0, arm_deg(1, 30)), (t0 + 0.5, arm_deg(1, cower - 10)), (t0 + 0.95, arm_deg(1, peek)), (t0 + 1.45, arm_deg(1, cower)), (t0 + 1.95, arm_deg(1, peek - 10)), (t0 + 2.35, arm_deg(1, cower))],
                    hand_l="open", hand_r="open",
                    body=[(t0, (0, 0)), (t0 + 0.45, (0, 26)), (t0 + 0.9, (0, 6)), (t0 + 1.4, (0, 30)), (t0 + 1.9, (0, 8)), (t0 + 2.3, (0, 32))], seed=1)
    men = [env]
    for k, (x, sc, fl, mood, tun) in enumerate([(790, 0.66, False, "fear", CREAM), (930, 0.62, True, "shock", RED), (420, 0.6, False, "shock", CREAM)]):
        men.append(tremble(extra("drev", f"{s['sid']}d{k}", (x, 1600), sc, flip=fl, mood=mood, tunic=tun, seed=2 + k), seed=k))
    boat = sets.boat(f"{s['sid']}b", 700, 1600, 0.95)
    trickle = props.dirt_clods(f"{s['sid']}tr", 300, 1100, 1380, 1620, t0, t1, n=22, seed=9)
    # The guards' shadows fall across the far wall of the pit.
    shadows = []
    for k, x in enumerate([380, 760, 1040]):
        sh = shape(f"{s['sid']}gs{k}", [(x - 40, 1400), (x - 20, 1300), (x, 1280), (x + 20, 1300), (x + 40, 1400)], COBALT_DK, w=0, line=False, rough=1.0, opacity=0.8)
        sh["transform"]["position"] = ks([(t0, [0, 0]), (t1, [30 * (k - 1), 10], "linear")])
        shadows.append(sh)
    return dict(pit=True, world=world, mid=men + [comp(f"{s['sid']}bt", boat)], fore_extra=trickle, back_extra=[], set_extra=shadows,
                cam=cam((t0, 700, 1180, 1.12), (t1, 690, 1360, 1.5, "gesture")),
                sfx=[("whoosh", t0 + 0.2, -16)])


def S02(s):  # and the first shovel of dirt comes down. — CU: a shovel tips its load; the envoy screams, arms up, buried in clods; the camera jolts
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    td = cue("l01", "dirt")
    env = envoy_rig(f"{s['sid']}e", (560, 1760), 0.8, t0, t1,
                    [(t0, "fear"), (t0 + 0.35, "fear"), (td - 0.05, "shock"), (td + 0.5, "shock"), (td + 0.9, "fear")],
                    head=[(t0, -12), (t0 + 0.35, -20), (td - 0.05, -26), (td + 0.12, 8), (td + 0.3, -6), (td + 0.6, 10)],
                    arm_l=[(t0, arm_deg(-1, 30)), (td - 0.1, arm_deg(-1, 120)), (td + 0.1, arm_deg(-1, 165)), (td + 0.4, arm_deg(-1, 140))],
                    arm_r=[(t0, arm_deg(1, 20)), (td - 0.05, arm_deg(1, 125)), (td + 0.12, arm_deg(1, 160)), (td + 0.45, arm_deg(1, 135))],
                    hand_l="open", hand_r="open",
                    body=[(t0, (0, 0)), (td - 0.05, (0, -10)), (td + 0.1, (0, 40)), (td + 0.3, (0, 30)), (td + 0.6, (0, 46))], seed=5)
    env["transform"]["scale"] = ks([(t0, [0.8, 0.8]), (td + 0.08, [0.86, 0.72], "snap"), (td + 0.3, [0.8, 0.8], "squash")])
    haft = [(-10, -40), (10, -40), (16, -460), (-4, -462)]
    blade = [(-70, -40), (70, -40), (64, 60), (30, 120), (0, 132), (-30, 120), (-64, 60)]
    shovel = comp(f"{s['sid']}sh", [shape(f"{s['sid']}sh-h", haft, GOLD_DK, w=3.6),
                                    strokes(f"{s['sid']}sh-g", [[(0, -100 - 60 * k), (1, -86 - 60 * k)] for k in range(6)], w=1.6, color=FINE),
                                    shape(f"{s['sid']}sh-b", blade, COBALT_LT, w=4),
                                    sparkle(f"{s['sid']}sh-gl", -36, -10, r=12)],
                  transform={"position": ks([(t0, [820, 560]), (td - 0.2, [760, 760], "glide")]), "anchor": [0, -300],
                             "rotation": ks([(t0, 160), (td - 0.25, 165), (td - 0.08, 215, "snap")])})
    load = shape(f"{s['sid']}load", blob(0, 0, 70, 40, n=12, lump=0.25, seed=2), GOLD_DK, w=3.2)
    load["transform"] = {"position": ks([(t0, [760, 760]), (td - 0.2, [740, 820]), (td + 0.12, [600, 1220], "in")]),
                         "scale": ks([(td - 0.05, [1, 1]), (td + 0.12, [1.5, 0.7], "in")]),
                         "opacity": ks([(td + 0.1, 1), (td + 0.16, 0, "linear")])}
    dirt = props.dirt_clods(f"{s['sid']}c", 380, 820, 820, 1560, td - 0.05, t1, n=60, seed=3)
    puff = []  # dust over his face hid the scream; the clods carry the impact
    jolt = [(td + 0.1, 580, 1120, 2.35, "snap"), (td + 0.16, 596, 1104, 2.38, "linear"), (td + 0.22, 570, 1130, 2.36, "linear"), (td + 0.3, 584, 1116, 2.37, "linear")]
    return dict(pit=True, world=world, mid=[env], fore_extra=[shovel, load] + dirt + puff, defocus=7,
                cam=cam((t0, 580, 1180, 1.85), (td, 580, 1140, 2.0, "glide"), *jolt, (t1, 580, 1130, 2.5, "linear"), clamp=False),
                sfx=[("whoosh-out", td - 0.25, -14), ("thud", td + 0.12, -8), ("sting", td, -12)])


def S03(s):  # The woman watching is Olga, princess of Kyiv. — CU Olga: looks down into the pit, lifts her chin to camera, a slow smug smile; strong push in
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    to = cue("l02", "olga")
    tk = cue("l02", "kyiv")
    ol = olga_rig(f"{s['sid']}o", (330, 1400), 1.0, t0, t1, [(t0, "neutral"), (t0 + 0.4, "blink"), (t0 + 0.52, "neutral"), (to - 0.05, "smug"), (tk, "smug")],
                  head=[(t0, 14), (t0 + 0.5, 10), (to - 0.1, -10, "pop"), (to + 0.3, -6, "pop"), (to + 0.5, -10, "pop"), (tk, -4, "glide")],
                  body=[(t0, (0, 8)), (to - 0.1, (0, -6)), (to + 0.15, (0, -14)), (to + 0.35, (0, -6)), (to + 0.55, (0, -12))], seed=6)
    g = extra("guard", f"{s['sid']}g", (640, 1420), 0.95, flip=True, mood="smug", seed=7)
    g["transform"]["rotation"] = ks([(t0, 0), (to + 0.2, -6), (to + 0.6, -2)])
    return dict(world=world, mid=[g, ol], defocus=10,
                cam=cam((t0, 420, 860, 2.0), (to, 400, 780, 2.4, "glide"), (t1, 380, 720, 2.9, "linear"), clamp=False),
                sfx=[("chime", to - 0.05, -14)])


def S04(s):  # The year is 945. — WIDE courtyard, drift
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    ol = olga_rig(f"{s['sid']}o", (330, 1400), 0.55, t0, t1, [(t0, "smug")], seed=8)
    gs = [extra("guard", f"{s['sid']}g{k}", (x, 1420), 0.5, flip=k % 2 == 1, seed=10 + k, spear=k != 1) for k, x in enumerate([520, 1180, 1320])]
    life = yard_life(s["sid"], t0, t1)
    boat = sets.boat(f"{s['sid']}b", 700, 1560, 0.95)
    return dict(world=world, mid=[comp(f"{s['sid']}bt", boat), ol] + gs, sky_extra=life,
                cam=cam((t0, 700, 1150, 0.92), (t1, 820, 1100, 0.98, "linear")),
                sfx=[("tick", cue("l02", "945"), -16)])


def iskorosten_day(sid):
    return sets.iskorosten(f"{sid}i", dusk=True)


def S05(s):  # Weeks ago, these men's tribe, the Drevlians, — WIDE: Drevlians outside their walls
    t0, t1 = s["t0"], s["t1"]
    world = iskorosten_day(s["sid"])
    crowd = []
    for k, (x, sc) in enumerate([(200, 0.8), (420, 0.86), (650, 0.8), (880, 0.9), (1100, 0.82), (1300, 0.78)]):
        crowd.append(extra("drev", f"{s['sid']}d{k}", (x, 1700 + (k % 2) * 40), sc, flip=k % 2 == 1, mood="anger" if k % 2 else "neutral", tunic=RED if k % 3 == 0 else CREAM, seed=20 + k))
    smoke = props.puffs(f"{s['sid']}sm", 520, 900, t0, t1, n=4, s=0.8, color=CREAM_DK, seed=1)
    return dict(world=world, mid=crowd, back_extra=smoke,
                cam=cam((t0, 760, 1380, 0.95), (t1, 700, 1420, 1.05, "linear")),
                sfx=[])


def S06(s):  # killed her husband, Prince Igor, — MEDIUM: Igor shocked, Drevlians fists up
    t0, t1 = s["t0"], s["t1"]
    world = iskorosten_day(s["sid"])
    ti = cue("l03", "igor")
    ig = igor_rig(f"{s['sid']}i", (540, 1720), 1.0, t0, t1, [(t0, "neutral"), (t0 + 0.25, "shock")],
                  head=[(t0, 0), (t0 + 0.25, 12)], body=[(t0, (0, 0)), (t0 + 0.25, (-30, 10))], rot=[(t0, 0), (t0 + 0.25, -8)],
                  arm_l=[(t0, 0), (t0 + 0.25, arm_deg(-1, 110))], arm_r=[(t0, 0), (t0 + 0.28, arm_deg(1, 100))], hand_l="open", hand_r="open", seed=30)
    d1 = extra("drev", f"{s['sid']}d1", (180, 1760), 0.95, mood="anger", tunic=RED, seed=31)
    d2 = extra("drev", f"{s['sid']}d2", (920, 1760), 0.95, flip=True, mood="anger", seed=32)
    club = comp(f"{s['sid']}cl", [line(f"{s['sid']}clb", [(0, 0), (0, -260)], w=18, color=GOLD_DK, taper=(0, 6), pressure=0.2)],
                transform={"position": [960, 1380], "rotation": ks([(t0, 30), (t0 + 0.2, -40, "slam")])})
    return dict(world=world, mid=[d1, d2, ig, club], defocus=4,
                cam=cam((t0, 540, 1200, 1.45), (t0 + 0.25, 540, 1180, 1.6, "snap"), (t1, 540, 1170, 1.66, "linear")),
                sfx=[("hit", t0 + 0.2, -8), ("sting", ti, -13)])


def S07(s):  # for coming back to take their tribute twice. — INSERT: Igor's greedy grab, twice
    t0, t1 = s["t0"], s["t1"]
    world = iskorosten_day(s["sid"])
    tt = cue("l03", "tribute")
    tw = cue("l03", "twice")
    ig = igor_rig(f"{s['sid']}i", (520, 1900), 1.1, t0, t1, [(t0, "joy"), (tw, "joy")], head=[(t0, -4), (tt, 6), (tw, -6)],
                  arm_l=[(t0, arm_deg(-1, 40)), (tt, arm_deg(-1, -20)), (tw, arm_deg(-1, 50))], arm_r=[(t0, arm_deg(1, 30)), (tt + 0.1, arm_deg(1, -25)), (tw + 0.1, arm_deg(1, 40))],
                  hand_l="grip", hand_r="grip", body=[(t0, (0, 0)), (tt, (0, 30)), (tw, (0, -10))], seed=33)
    pile = comp(f"{s['sid']}tp", props.tribute(f"{s['sid']}tr", 1.25), transform={"position": ks([(t0, [540, 1790]), (tt, [540, 1810], "snap"), (tt + 0.2, [540, 1790], "pop"), (tw, [540, 1815], "snap"), (tw + 0.2, [540, 1790], "pop")]),
                                                                              "rotation": ks([(t0, 0), (tt, 3, "snap"), (tt + 0.25, -1, "pop"), (tw, -4, "snap"), (tw + 0.25, 1, "pop")])})
    return dict(cap_y=1760, world=world, mid=[ig], fore_extra=[pile], defocus=6,
                cam=cam((t0, 540, 1330, 1.45), (tt - 0.08, 548, 1336, 1.56, "linear"), (tt, 540, 1330, 1.62, "snap"), (tt + 0.12, 552, 1240, 1.64, "linear"), (tw - 0.08, 530, 1262, 1.74, "linear"), (tw, 540, 1260, 1.8, "snap"), (tw + 0.12, 528, 1250, 1.82, "linear"), (t1, 540, 1250, 1.95, "linear")),
                sfx=[("pop", tt + 0.05, -14), ("pop", tw + 0.05, -12)])


def hall_world(sid):
    return sets.hall(f"{sid}h")


def hall_life(sid, t0, t1):
    rnd = random.Random(sid)
    motes = []
    for k in range(14):
        x, y = rnd.uniform(320, 1000), rnd.uniform(500, 1700)
        d = dots(f"{sid}m{k}", [(0, 0)], r=rnd.uniform(2, 3.5), color=CREAM)
        d["transform"] = {"position": ks([(t0, [x, y]), (t1, [x + rnd.uniform(-40, 40), y - rnd.uniform(60, 140)], "linear")]), "opacity": 0.7}
        motes.append(d)
    return motes


def S08(s):  # Now they want her to marry their own prince, Mal. — envoy presents; Mal pops in; Olga reacts
    t0, t1 = s["t0"], s["t1"]
    world = hall_world(s["sid"])
    tm = cue("l04", "mal")
    tmarry = cue("l04", "marry")
    env = envoy_rig(f"{s['sid']}e", (430, 2050), 0.95, t0, t1, [(t0, "joy"), (tmarry, "smug"), (tm, "joy")],
                    arm_r=[(t0, 0), (tmarry, arm_deg(1, 70))], hand_r="open", head=[(t0, 4), (tmarry, -6)], seed=40)
    ol = olga_rig(f"{s['sid']}o", (1060, 1950), 0.9, t0, t1, [(t0, "neutral"), (tm + 0.1, "smug")], head=[(t0, 0), (tm + 0.1, -8)], seed=41, flip=True)
    mal = cast.mal(f"{s['sid']}m", (0, 420), 0.75)
    frame = comp(f"{s['sid']}mf", [shape(f"{s['sid']}mfr", blob(0, 0, 230, 300, n=18, lump=0.02), CREAM, w=5),
                                    shape(f"{s['sid']}mfr2", blob(0, 0, 200, 270, n=18, lump=0.02), COBALT_LT, w=3),
                                    comp(f"{s['sid']}mal", [mal], transform={"position": [0, 0]})],
                 transform={"position": [780, 760], "scale": ks([(tm - 0.08, [0, 0]), (tm + 0.1, [1, 1], "pop")])}, time={"start": round(tm - 0.08, 3)})
    return dict(world=world, mid=[env, ol], fore_extra=[frame], defocus=3, sky_extra=hall_life(s["sid"], t0, t1),
                cam=cam((t0, 700, 1250, 1.05), (t1, 720, 1200, 1.18, "linear")),
                sfx=[("pop", tm, -10), ("sparkle", tm + 0.05, -18)])


def S09(s):  # So Olga smiles. — CU Olga: silent chuckle, chin up, head cocked
    t0, t1 = s["t0"], s["t1"]
    world = hall_world(s["sid"])
    ts = cue("l05", "smiles")
    ol = olga_rig(f"{s['sid']}o", (700, 1950), 1.0, t0, t1, [(t0, "neutral"), (ts - 0.05, "smug")],
                  head=[(t0, 0), (ts, -10, "pop"), (ts + 0.25, -6, "pop"), (ts + 0.45, -10, "pop")],
                  body=[(t0, (0, 0)), (ts + 0.1, (0, -10)), (ts + 0.3, (0, 0)), (ts + 0.5, (0, -8))], seed=42)
    return dict(world=world, mid=[ol], defocus=8,
                cam=cam((t0, 700, 1200, 2.4), (t1, 690, 1170, 2.65, "linear"), clamp=False),
                sfx=[])


def yard_life(sid, t0, t1):
    L = []
    for k in range(4):
        b = sets.bird_v(f"{sid}bv{k}", 0, 0, 1.2)
        y = 300 + k * 70
        b["transform"] = {"position": ks([(t0, [-100 - k * 80, y]), (t1, [900 - k * 60, y - 60], "linear")])}
        L.append(b)
    return L


def S10(s):  # Stay in your boat, she says. — WIDE: envoys in their boat, Olga's palm up
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    ts = cue("l05", "stay")
    boat = comp(f"{s['sid']}bt", sets.boat(f"{s['sid']}b", 760, 1280, 0.85))
    env = envoy_rig(f"{s['sid']}e", (640, 1300), 0.5, t0, t1, [(t0, "joy")], seed=43)
    d = [extra("drev", f"{s['sid']}d{k}", (x, 1300), 0.55, flip=k == 1, mood="joy", seed=44 + k) for k, x in enumerate([820, 960])]
    ol = olga_rig(f"{s['sid']}o", (250, 1500), 0.7, t0, t1, [(t0, "smug")], arm_r=[(t0, 0), (ts, arm_deg(1, 95))], hand_r="open",
                  head=[(t0, 0), (ts, -6)], seed=45)
    return dict(world=world, mid=[env] + d + [boat, ol], sky_extra=yard_life(s["sid"], t0, t1),
                cam=cam((t0, 560, 1100, 1.0), (t1, 620, 1120, 1.08, "linear")),
                sfx=[("swipe", ts, -16)])


def S11(s):  # My people will carry you in, as an honour. — guards lift the boat; envoys beam
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    tc = cue("l05", "carry")
    lift = ks([(t0, [0, 0]), (tc, [0, -120]), (tc + 0.35, [0, -100]), (tc + 0.7, [0, -125])], ease="land")
    inboat = [envoy_rig(f"{s['sid']}e", (640, 1300), 0.55, t0, t1, [(t0, "smug"), (tc, "joy")], head=[(t0, 0), (tc, -8)], seed=46),
              extra("drev", f"{s['sid']}d0", (840, 1300), 0.6, mood="joy", seed=47)]
    guards = [extra("guard", f"{s['sid']}g{k}", (x, 1560), 0.62, flip=k % 2 == 1, spear=False, mood="smug", seed=48 + k) for k, x in enumerate([420, 640, 900, 1100])]
    boat = sets.boat(f"{s['sid']}b", 760, 1280, 0.85)
    group = {"id": f"{s['sid']}grp", "kind": "comp", "transform": {"position": lift}, "layers": inboat + [comp(f"{s['sid']}bt", boat)]}
    return dict(world=world, mid=[group] + guards,
                cam=cam((t0, 720, 1300, 1.3), (t1, 740, 1240, 1.42, "linear")),
                sfx=[("thud", tc + 0.05, -12)])


def S12(s):  # They carry the boat straight into a pit. — WIDE: carried across, dropped in
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    tp = cue("l06", "pit")
    move = ks([(t0, [-420, -330]), (tp - 0.25, [0, -330], "linear"), (tp + 0.1, [0, 0], "in")])
    inboat = [envoy_rig(f"{s['sid']}e", (580, 1560), 0.55, t0, t1, [(t0, "joy"), (tp, "shock")], head=[(t0, 0), (tp, 12)],
                        arm_l=[(t0, 0), (tp, arm_deg(-1, 130))], arm_r=[(t0, 0), (tp, arm_deg(1, 130))], hand_l="open", hand_r="open", seed=50),
              extra("drev", f"{s['sid']}d0", (780, 1560), 0.6, mood="shock", seed=51)]
    boat = sets.boat(f"{s['sid']}b", 700, 1560, 0.95)
    group = {"id": f"{s['sid']}grp", "kind": "comp", "transform": {"position": move}, "layers": inboat + [comp(f"{s['sid']}bt", boat)]}
    dust = props.puffs(f"{s['sid']}dust", 700, 1500, tp + 0.1, tp + 1.0, n=3, s=1.4, color=CREAM_DK, rise=160, seed=3, lead=0)
    return dict(pit=True, world=world, mid=[group], fore_extra=dust,
                cam=cam((t0, 640, 1300, 1.0), (tp, 700, 1380, 1.12, "gesture"), (tp + 0.1, 700, 1400, 1.18, "snap"), (t1, 700, 1390, 1.2, "linear")),
                sfx=[("hit", tp + 0.1, -7)])


def S13(s):  # She leans over the edge and asks if the honour pleases them. — Olga leans over the pit's edge; the envoy looks up from inside
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    tl = cue("l07", "leans")
    tp = cue("l07", "pleases")
    ol = olga_rig(f"{s['sid']}o", (250, 1440), 0.85, t0, t1, [(t0, "smug"), (tp, "smug")], rot=[(t0, 0), (tl + 0.1, 20, "settle")],
                  head=[(t0, 0), (tl, 16), (tp, 8, "pop"), (tp + 0.2, 14, "pop")], arm_r=[(t0, 0), (tl, arm_deg(1, 40))], hand_r="open", seed=52)
    env = envoy_rig(f"{s['sid']}e", (800, 1950), 0.8, t0, t1, [(t0, "fear"), (tp, "fear")], head=[(t0, -14), (tp, -22)],
                    arm_l=[(t0, 0), (tp, arm_deg(-1, 60))], hand_l="open", seed=53)
    boat = comp(f"{s['sid']}bt", sets.boat(f"{s['sid']}b", 760, 1640, 1.0))
    return dict(pit=True, world=world, mid=[ol, env, boat], defocus=1.5,
                cam=cam((t0, 540, 1220, 1.22), (t1, 560, 1260, 1.32, "linear"), clamp=False),
                sfx=[])


def S14(s):  # Then the dirt comes down. — the envoy's face, buried in falling earth
    t0, t1 = s["t0"], s["t1"]
    world = sets.yard(f"{s['sid']}y")
    td = cue("l07", "dirt")
    env = envoy_rig(f"{s['sid']}e", (540, 2000), 1.0, t0, t1, [(t0, "fear"), (td - 0.05, "shock")], head=[(t0, -10), (td, -20)],
                    arm_l=[(t0, 0), (td, arm_deg(-1, 150))], arm_r=[(t0, 0), (td, arm_deg(1, 150))], hand_l="open", hand_r="open", seed=54)
    dirt = props.dirt_clods(f"{s['sid']}c", 150, 930, 400, 1700, td - 0.15, t1, n=70, seed=5)
    fill = shape(f"{s['sid']}fill", [(-200, 0), (1700, 0), (1700, 2600), (-200, 2600)], CREAM_DK, w=0, line=False, rough=0,
                 transform={"position": ks([(td + 0.25, [0, 2400]), (t1, [0, 500], "in")])})
    return dict(pit=True, world=world, mid=[env], fore_extra=dirt + [fill], defocus=6,
                cam=cam((t0, 540, 1150, 2.0), (td, 540, 1140, 2.3, "snap"), (t1, 540, 1160, 2.4, "linear"), clamp=False),
                sfx=[("sting", td - 0.02, -11), ("thud", td + 0.25, -9)])


def isk_life(sid, t0, t1, world):
    L = []
    for k, (x, y) in enumerate(world["eaves"][:4]):
        L += props.puffs(f"{sid}sm{k}", x + 60, y - 60, t0, t1, n=3, s=0.5, color=COBALT_LT, rise=260, seed=k)
    return L


def S15(s):  # The Drevlians never hear of it. — Iskorosten at dusk: Drevlians drinking, oblivious
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i", dusk=True)
    crowd = []
    for k, (x, sc) in enumerate([(260, 0.85), (520, 0.92), (790, 0.88), (1050, 0.86)]):
        d = extra("drev", f"{s['sid']}d{k}", (x, 1720), sc, flip=k % 2 == 1, mood="joy", tunic=RED if k % 2 else CREAM, seed=60 + k)
        d["transform"]["rotation"] = {"base": 0, "add": [{"type": "wiggle", "hz": 1.4, "amp": 4, "seed": k}]}
        crowd.append(d)
        crowd.append(comp(f"{s['sid']}h{k}", [shape(f"{s['sid']}horn{k}", [(0, 0), (40, -30), (60, -80), (48, -84), (26, -46), (2, -18)], CREAM, w=2.6)],
                          transform={"position": [x + 50 * sc * (-1 if k % 2 else 1), 1720 - 300 * sc], "rotation": {"base": 0, "add": [{"type": "wiggle", "hz": 1.2, "amp": 10, "seed": k + 9}]}}))
    return dict(world=world, mid=crowd, back_extra=isk_life(s["sid"], t0, t1, world),
                cam=cam((t0, 660, 1420, 1.0), (t1, 720, 1440, 1.06, "linear")),
                sfx=[])


def S16(s):  # So she writes again: — INSERT: Olga's ringed hand scratches a birch-bark letter
    t0, t1 = s["t0"], s["t1"]
    world = hall_world(s["sid"])
    tw_ = cue("l08", "writes")
    let = comp(f"{s['sid']}let", props.letter(f"{s['sid']}l", 760, 440, draw_t=(tw_ - 0.1, t1)), transform={"position": [540, 1150], "rotation": -4})
    # A pen rig whose origin is the stylus tip: it rides each line as it is scratched on.
    pen = comp(f"{s['sid']}pen", props.stylus(f"{s['sid']}st", 2.0) + [comp(f"{s['sid']}hd", cast.hand(f"{s['sid']}hdd", "grip", lit=True, side=1, ring=True, size=3.4),
                                                                         transform={"position": [150, -250], "rotation": 200})])
    a, b = tw_ - 0.1, t1
    pk = [(t0, [760, 900])]
    for row in range(5):
        y = 1150 - 160 + row * 44
        ta = a + (b - a) * row / 5
        tb = a + (b - a) * (row + 1) / 5
        pk += [(ta, [190, y], "snap"), (tb, [850, y + 8], "linear")]
    pen["transform"]["position"] = ks(pk)
    pen["transform"]["position"]["add"] = [{"type": "wiggle", "hz": 9, "amp": 4, "seed": 3}]
    hand = pen
    return dict(world=world, mid=[], screen_extra=[let, hand], defocus=9,
                cam=cam((t0, 600, 1300, 1.5), (t1, 620, 1280, 1.6, "linear")),
                sfx=[("type", [tw_ + 0.1, tw_ + 0.5], -18)])


def S17(s):  # send me your best men. — Mal reads it, delighted; a Drevlian nods behind
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i", dusk=True)
    tb = cue("l08", "best")
    mal = cast.mal(f"{s['sid']}m", (560, 1900), 1.1)
    mal["transform"]["position"] = {"base": [560, 1900], "add": [{"type": "wiggle", "hz": 0.6, "amp": 5, "seed": 1}]}
    mal["transform"]["rotation"] = ks([(t0, 0), (tb, -5), (tb + 0.3, 3)])
    let = comp(f"{s['sid']}let", props.letter(f"{s['sid']}l", 380, 220), transform={"position": [560, 1460], "rotation": ks([(t0, 6), (tb, -4)])})
    d = extra("drev", f"{s['sid']}d", (950, 1880), 0.95, flip=True, mood="joy", seed=70)
    d["transform"]["rotation"] = ks([(t0, 0), (tb, 6), (tb + 0.2, -3), (tb + 0.4, 5)])
    return dict(world=world, mid=[d, mal, let], defocus=5,
                cam=cam((t0, 620, 1260, 1.45), (t1, 600, 1230, 1.55, "linear")),
                sfx=[("pop", tb, -16)])


def road_world(sid):
    return sets.iskorosten(f"{sid}i", dusk=True)


def S18(s):  # They send their best. — the nobles march out, chests out; camera tracks
    t0, t1 = s["t0"], s["t1"]
    world = road_world(s["sid"])
    march = []
    for k in range(4):
        x = 160 + k * 230
        d = extra("drev", f"{s['sid']}n{k}", (x, 1780), 0.9, mood="smug" if k % 2 else "joy", tunic=RED, seed=80 + k, bob=0)
        d["transform"]["position"] = ks([(t0, [x - 150, 1780]), (t1, [x + 250, 1780], "linear")])
        d["transform"]["position"]["add"] = [{"type": "wiggle", "hz": 3.2, "amp": 8, "seed": k}]
        march.append(d)
    return dict(world=world, mid=march,
                cam=cam((t0, 560, 1400, 1.05), (t1, 760, 1400, 1.08, "linear")),
                sfx=[("thud", [t0 + 0.4], -20)])


def bath_world(sid, burning=False):
    return sets.bathhouse(f"{sid}b")


def S19(s):  # She offers them a bathhouse to wash off the road. — Olga's welcoming sweep; nobles file in
    t0, t1 = s["t0"], s["t1"]
    world = bath_world(s["sid"])
    to = cue("l09", "offers")
    ol = olga_rig(f"{s['sid']}o", (300, 2020), 0.78, t0, t1, [(t0, "smug")], arm_r=[(t0, 0), (to, arm_deg(1, 80))], hand_r="open", head=[(t0, 0), (to, -8)], seed=81)
    nobles = []
    for k in range(3):
        d = extra("drev", f"{s['sid']}n{k}", (900 + k * 160, 1900), 0.62, flip=True, mood="joy", tunic=RED, seed=82 + k, bob=0)
        d["transform"]["position"] = ks([(t0, [980 + k * 170, 1920]), (t1, [700 + k * 170, 1900], "linear")])
        nobles.append(d)
    steam = props.puffs(f"{s['sid']}st", 960, 1140, t0, t1, n=4, s=0.9, seed=4)
    return dict(world=world, mid=nobles + [ol], back_extra=steam,
                cam=cam((t0, 700, 1480, 1.0), (t1, 760, 1520, 1.08, "linear")),
                sfx=[])


def S20(s):  # She bars the door. — INSERT: the bar slams down across the door
    t0, t1 = s["t0"], s["t1"]
    world = bath_world(s["sid"])
    tb = cue("l09", "bars")
    bar = comp(f"{s['sid']}bar", props.door_bar(f"{s['sid']}br", 420, 1.0),
               transform={"position": ks([(t0, [855, 1140]), (tb + 0.05, [685, 1380], "slam")]), "rotation": ks([(t0, -30), (tb + 0.05, 0, "slam")])})
    hands = [comp(f"{s['sid']}h{k}", cast.hand(f"{s['sid']}hd{k}", "grip", lit=k == 0, side=1 - 2 * k, size=1.6),
                  transform={"position": ks([(t0, [700 + 340 * k, 1060]), (tb + 0.05, [530 + 340 * k, 1300], "slam")])}) for k in range(2)]
    return dict(world=world, mid=[bar] + hands,
                cam=cam((t0, 690, 1360, 2.0), (tb + 0.05, 690, 1380, 2.25, "snap"), (t1, 690, 1380, 2.3, "linear")),
                sfx=[("hit", tb + 0.05, -6)])


def S21(s):  # She burns it. — WIDE: the bathhouse in flames; Olga in the foreground with a torch
    t0, t1 = s["t0"], s["t1"]
    world = bath_world(s["sid"])
    flames = []
    for k, (fx, fy, sc) in enumerate([(380, 1060, 2.4), (700, 1040, 2.8), (1000, 1070, 2.4), (300, 1300, 1.6), (1120, 1320, 1.7), (600, 1250, 1.3)]):
        fl = sets.flame(f"{s['sid']}f{k}", fx, fy, sc, seed=k, until=t1 + 1)
        for f_ in fl:
            f_["time"] = {"start": round(t0 + 0.06 * k, 3)}
        flames += fl
    emb = props.embers(f"{s['sid']}em", 300, 1100, 950, t0, t1, n=30, seed=2)
    smoke = props.puffs(f"{s['sid']}sm", 700, 760, t0, t1, n=4, s=2.0, color=COBALT_DK, rise=500, seed=3)
    ol = olga_rig(f"{s['sid']}o", (280, 2120), 0.95, t0, t1, [(t0, "smug"), (t0 + 0.6, "anger")], arm_r=[(t0, arm_deg(1, 40))], hand_r="grip", seed=85)
    tor = comp(f"{s['sid']}tor", props.torch(f"{s['sid']}tc", 0, 0, 1.2, t_until=t1 + 1, seed=7), transform={"position": [440, 1460]})
    return dict(world=world, mid=flames + emb, back_extra=smoke, fore_extra=[ol, tor],
                cam=cam((t0, 700, 1350, 1.0), (t1, 690, 1330, 1.1, "linear")),
                sfx=[("boom", t0 + 0.05, -8)])


def grave_world(sid):
    return sets.grave(f"{sid}g")


def grave_torches(sid, world, t1):
    L = []
    for k, (tx, ty) in enumerate(world["torches"]):
        L += sets.flame(f"{sid}tf{k}", tx, ty, 0.9, seed=10 + k, until=t1 + 1)
    return L


def S22(s):  # At Igor's grave she holds a funeral feast. — WIDE: the barrow, Olga in grief, the table set
    t0, t1 = s["t0"], s["t1"]
    world = grave_world(s["sid"])
    ol = olga_rig(f"{s['sid']}o", (760, 1180), 0.5, t0, t1, [(t0, "grief")], head=[(t0, 10)], seed=90)
    guests = [extra("drev", f"{s['sid']}d{k}", (x, 1780), 0.75, flip=k % 2 == 1, mood="neutral", tunic=RED if k % 2 else CREAM, seed=91 + k) for k, x in enumerate([300, 560, 900, 1180])]
    return dict(world=world, mid=[ol] + guests + grave_torches(s["sid"], world, t1),
                cam=cam((t0, 760, 1300, 0.95), (t1, 760, 1260, 1.02, "linear")),
                sfx=[])


def S23(s):  # The Drevlians drink with her. — horns up, laughing; Olga raises a cup
    t0, t1 = s["t0"], s["t1"]
    world = grave_world(s["sid"])
    td = cue("l10", "drink")
    guests = []
    for k, x in enumerate([340, 640, 940]):
        d = extra("drev", f"{s['sid']}d{k}", (x, 1640), 0.95, flip=k == 2, mood="joy", tunic=RED if k % 2 else CREAM, seed=95 + k)
        d["transform"]["rotation"] = ks([(t0, 0), (td, -10), (td + 0.4, -6)])
        guests.append(d)
        guests.append(comp(f"{s['sid']}h{k}", [shape(f"{s['sid']}horn{k}", [(0, 0), (40, -30), (60, -80), (48, -84), (26, -46), (2, -18)], CREAM, w=2.6)],
                           transform={"position": ks([(t0, [x + 40, 1400]), (td, [x + 20, 1260])]), "rotation": ks([(t0, 0), (td, -40)])}))
    ol = olga_rig(f"{s['sid']}o", (1180, 1700), 0.8, t0, t1, [(t0, "smug")], arm_l=[(t0, 0), (td, arm_deg(-1, 100))], hand_l="grip", flip=True, seed=98)
    return dict(world=world, mid=guests + [ol] + grave_torches(s["sid"], world, t1), defocus=2,
                cam=cam((t0, 760, 1320, 1.3), (t1, 760, 1300, 1.4, "linear")),
                sfx=[("pop", td, -16)])


def S24(s):  # Five thousand of them never get up. — the table of slumped guests; guards stand over them
    t0, t1 = s["t0"], s["t1"]
    world = grave_world(s["sid"])
    tf = cue("l10", "five")
    slumped = []
    for k, x in enumerate([300, 560, 820, 1080]):
        d = extra("drev", f"{s['sid']}d{k}", (x, 1720), 0.85, flip=k % 2 == 1, mood="blink", tunic=RED if k % 2 else CREAM, seed=100 + k, bob=0)
        d["transform"]["rotation"] = ks([(t0, 0), (t0 + 0.15 * k + 0.3, 70 if k % 2 == 0 else -70, "bounce")])
        slumped.append(d)
    guards = []
    for k, (x0, x1) in enumerate([(-80, 200), (1580, 1300)]):
        g = extra("guard", f"{s['sid']}g{k}", (x0, 1500), 0.7, flip=k == 1, seed=105 + k, bob=0)
        g["transform"]["position"] = ks([(t0, [x0, 1500]), (t1, [x1, 1500], "linear")])
        g["transform"]["position"]["add"] = [{"type": "wiggle", "hz": 3.0, "amp": 6, "seed": k}]
        guards.append(g)
    ol = olga_rig(f"{s['sid']}o", (760, 1300), 0.6, t0, t1, [(t0, "anger"), (tf + 1.0, "smug")], seed=107)
    return dict(world=world, mid=[ol] + guards + slumped + grave_torches(s["sid"], world, t1),
                cam=cam((t0, 760, 1460, 1.35), (t1, 760, 1360, 1.0, "linear")),
                sfx=[("sting", tf, -11), ("thud", t0 + 0.45, -14)])


def S25(s):  # Their city, Iskorosten, still holds out. — night: Drevlians on their walls, fists up; spears below
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i")
    walls = [extra("drev", f"{s['sid']}d{k}", (x, 1200), 0.45, flip=k % 2 == 1, mood="anger", tunic=RED if k % 2 else CREAM, seed=110 + k) for k, x in enumerate([260, 520, 780, 1040, 1280])]
    spears = [extra("guard", f"{s['sid']}g{k}", (x, 2150), 1.0, flip=k % 2 == 1, seed=120 + k) for k, x in enumerate([120, 380, 980, 1260])]
    return dict(world=world, mid=walls, fore_extra=spears, back_extra=isk_life(s["sid"], t0, t1, world),
                cam=cam((t0, 760, 1300, 0.95), (t1, 760, 1360, 1.02, "linear")),
                sfx=[])


def S26(s):  # So Olga offers peace, at a tiny price: — CU Olga, finger and thumb pinched
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i")
    tp = cue("l11", "peace")
    tt = cue("l11", "tiny")
    ol = olga_rig(f"{s['sid']}o", (600, 2000), 1.0, t0, t1, [(t0, "neutral"), (tp, "smug"), (tt, "smug")],
                  arm_l=[(t0, 0), (tt - 0.1, arm_deg(-1, 150))], hand_l="point", head=[(t0, 0), (tp, -8), (tt, 6, "pop")], seed=130)
    return dict(world=world, mid=[ol], defocus=8,
                cam=cam((t0, 560, 1200, 2.0), (tt, 520, 1150, 2.2, "glide"), (t1, 520, 1140, 2.3, "linear"), clamp=False),
                sfx=[("tick", tt + 0.05, -14)])


def S27(s):  # three pigeons and three sparrows from every house. — INSERT: the birds pop up in a row; a Drevlian beams
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i")
    tp = cue("l11", "pigeons")
    tsp = cue("l11", "sparrows")
    birds = []
    for k in range(3):
        tk = t0 + 0.12 * k
        b = flapper(f"{s['sid']}p{k}", "pigeon", 1.6, tk, t1, (230 + k * 300, 980))
        b["transform"]["scale"] = ks([(tk - 0.01, [0, 0]), (tk + 0.05, [1, 1], "pop")])
        b["time"] = {"start": round(tk - 0.01, 3)}
        birds.append(b)
    for k in range(3):
        tk = tsp + 0.12 * k
        b = flapper(f"{s['sid']}s{k}", "sparrow", 1.8, tk, t1, (230 + k * 300, 1320))
        b["transform"]["scale"] = ks([(tk - 0.01, [0, 0]), (tk + 0.05, [1, 1], "pop")])
        b["time"] = {"start": round(tk - 0.01, 3)}
        birds.append(b)
    d = extra("drev", f"{s['sid']}d", (880, 2440), 1.2, flip=True, mood="joy", seed=131)
    return dict(world=world, mid=[d], screen_extra=birds, defocus=8,
                cam=cam((t0, 600, 1300, 1.3), (t1, 600, 1280, 1.38, "linear")),
                sfx=[("pop", [t0, tsp], -13)])


def S28(s):  # Her soldiers tie sulphur and rags to every bird — INSERT: hands knot a bundle to a pigeon
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i", dusk=True)
    ts = cue("l12", "sulphur")
    pg = flapper(f"{s['sid']}pg", "pigeon", 4.0, t0, t1, (470, 1050))
    bundle = comp(f"{s['sid']}bun", props.sulphur(f"{s['sid']}su", 0, 0, 3.0), transform={"position": ks([(t0, [700, 1500]), (ts, [500, 1160], "snap")])})
    hands = [comp(f"{s['sid']}h{k}", cast.hand(f"{s['sid']}hd{k}", "grip" if k else "fist", lit=k == 0, side=1 - 2 * k, size=2.4),
                  transform={"position": ks([(t0, [380 + 300 * k, 1400]), (ts, [400 + 220 * k, 1180])] + [(ts + 0.18 * j, [400 + 220 * k + (14 if (j + k) % 2 else -14), 1180 + (10 if j % 2 else -10)], "snap") for j in range(1, 14)]),
                             "rotation": 180}) for k in range(2)]
    return dict(world=world, mid=[], screen_extra=[pg, bundle] + hands, defocus=9,
                cam=cam((t0, 600, 1300, 1.3), (t1, 600, 1300, 1.36, "linear")),
                sfx=[("swipe", ts + 0.05, -14)])


def S29(s):  # and light them at dusk. — a torch touches the bundle; it flares; a soldier grins
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i", dusk=True)
    tl = cue("l12", "light")
    pg_dark = comp(f"{s['sid']}pgd", props.pigeon(f"{s['sid']}pd", 1, 3.2, bundle=True), transform={"position": [520, 1100]}, time={"end": round(tl + 0.05, 3)})
    pg_lit = comp(f"{s['sid']}pgl", props.pigeon(f"{s['sid']}pl", 0, 3.2, bundle=True, lit=True), transform={"position": [520, 1100], "scale": ks([(tl + 0.05, [0.95, 0.95]), (tl + 0.2, [1, 1], "pop")])}, time={"start": round(tl + 0.05, 3)})
    tor = comp(f"{s['sid']}tor", props.torch(f"{s['sid']}tc", 0, 0, 1.8, t_until=t1 + 1, seed=5), transform={"position": ks([(t0, [960, 1500]), (tl, [620, 1320], "snap")]), "rotation": -30})
    g = extra("guard", f"{s['sid']}g", (880, 2400), 1.2, flip=True, mood="joy", spear=False, seed=140)
    return dict(world=world, mid=[g], screen_extra=[pg_dark, pg_lit, tor], defocus=8,
                cam=cam((t0, 600, 1300, 1.2), (t1, 600, 1280, 1.3, "linear")),
                sfx=[("pop", tl + 0.05, -12)])


def S30(s):  # The birds fly home, to their nests under dry eaves. — WIDE: burning birds stream over the walls to the roofs
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i")
    rnd = random.Random(7)
    birds = []
    for k, (ex, ey) in enumerate(world["eaves"]):
        st = t0 + rnd.uniform(0.0, 0.8)
        birds += props.flier(f"{s['sid']}b{k}", "pigeon" if k % 2 else "sparrow", rnd.uniform(200, 1300), 2100, ex + 40, ey + 30, st, st + rnd.uniform(1.5, 2.1), s=1.0, bundle=True, lit=True, arc=-160)
    return dict(world=world, mid=birds, back_extra=isk_life(s["sid"], t0, t1, world),
                cam=cam((t0, 760, 1600, 1.12), (t1, 760, 1200, 1.3, "linear")),
                sfx=[])


def S31(s):  # Every house catches at once. — WIDE: every roof bursts into flame; shock on the walls
    t0, t1 = s["t0"], s["t1"]
    world = sets.iskorosten(f"{s['sid']}i")
    tc = cue("l13", "catches")
    flames = []
    for k, (ex, ey) in enumerate(world["eaves"]):
        fl = sets.flame(f"{s['sid']}f{k}", ex + 40, ey + 40, 1.5, seed=30 + k, until=t1 + 1)
        for f_ in fl:
            f_["time"] = {"start": round(tc, 3)}
            f_["transform"] = {"scale": ks([(tc, [0.2, 0.2]), (tc + 0.15, [1, 1], "pop")])}
        flames += fl
    emb = props.embers(f"{s['sid']}em", 100, 1400, 1100, tc, t1 + 0.5, n=40, seed=8, rise=600)
    walls = [extra("drev", f"{s['sid']}d{k}", (x, 1200), 0.45, flip=k % 2 == 1, mood="shock", tunic=RED if k % 2 else CREAM, seed=150 + k) for k, x in enumerate([300, 640, 980, 1240])]
    return dict(world=world, mid=flames + emb + walls,
                cam=cam((t0, 760, 1250, 1.15), (tc, 760, 1240, 1.2, "snap"), (t1, 760, 1300, 0.98, "glide")),
                sfx=[("boom", tc, -6), ("sting", tc + 0.05, -14)])


def S32(s):  # Later, the Church made Olga a saint. — the icon: Olga haloed, smug, blinking; candles; push in
    t0, t1 = s["t0"], s["t1"]
    world = sets.icon(f"{s['sid']}c")
    ts = cue("l14", "saint")
    ol = olga_rig(f"{s['sid']}o", (750, 1900), 1.3, t0, t1, [(t0, "neutral"), (ts, "smug")], head=[(t0, 0), (ts, -6, "pop")], seed=160)
    candles = []
    for k, (cx_, cy_) in enumerate(world["candles"]):
        candles += sets.flame(f"{s['sid']}cf{k}", cx_, cy_, 0.6, seed=40 + k, until=t1 + 1)
    return dict(world=world, mid=[ol] + candles,
                cam=cam((t0, 690, 1180, 1.0), (t1, 800, 1020, 1.5, "linear"), clamp=False),
                sfx=[("chime", ts, -10), ("riser", ts - 1.6, -20)])


TABLE = [
    ("S01", "l01", 0, S01), ("S02", "l01", 12, S02),
    ("S03", "l02", 0, S03), ("S04", "l02", 8, S04),
    ("S05", "l03", 0, S05), ("S06", "l03", 7, S06), ("S07", "l03", 12, S07),
    ("S08", "l04", 0, S08),
    ("S09", "l05", 0, S09), ("S10", "l05", 3, S10), ("S11", "l05", 9, S11),
    ("S12", "l06", 0, S12),
    ("S13", "l07", 0, S13), ("S14", "l07", 12, S14),
    ("S15", "l08", 0, S15), ("S16", "l08", 6, S16), ("S17", "l08", 10, S17),
    ("S18", "l09", 0, S18), ("S19", "l09", 4, S19), ("S20", "l09", 14, S20), ("S21", "l09", 18, S21),
    ("S22", "l10", 0, S22), ("S23", "l10", 8, S23), ("S24", "l10", 13, S24),
    ("S25", "l11", 0, S25), ("S26", "l11", 6, S26), ("S27", "l11", 14, S27),
    ("S28", "l12", 0, S28), ("S29", "l12", 9, S29),
    ("S30", "l13", 0, S30), ("S31", "l13", 10, S31),
    ("S32", "l14", 0, S32),
]
for sid, line_, idx, fn in TABLE:
    shot(sid, line_, idx, fn=fn)
build_times()

WHIP = 2 * F  # each side of the cut
TWOS = {"S01", "S02", "S06", "S12", "S14", "S24", "S31"}  # action shots
DAY = {"S01", "S02", "S03", "S04", "S08", "S09", "S10", "S11", "S12", "S13", "S14", "S16"}


def rimmed(layer, s):
    """Rim light on the cast: warm cream from the sun by day, salmon at night."""
    color = "#fff1cf" if s["sid"] in DAY else "#ff9878"
    layer["effects"] = layer.get("effects", []) + [{"type": "rim", "color": color, "width": 9, "angle": -55, "intensity": 0.95}]
    return layer
DIRS = [(-1, 0), (0, -1), (1, 0), (0, 1)]


def visible_push(c, gain=0.25):
    """A camera that barely zooms (under 12% over the shot) pushes in about
    18%, so the frame never sits still; pull-backs are left as they are."""
    z0, z1 = c[0][3], c[-1][3]
    if not (0.95 <= z1 / z0 < 1.12) or len(c) < 2:
        return c
    ta, tb = c[0][0], c[-1][0]
    out = []
    for (tt, cx, cy, z, ease) in c:
        u = (tt - ta) / max(1e-6, tb - ta)
        out.append((tt, cx, cy, round(z * (1 + gain * u), 4), ease))
    return out


def stage(s, spec, k):
    """A shot: its set's depth layers through the camera, its actors, its
    whips in and out."""
    sid, t0, t1 = s["sid"], s["t0"], s["t1"]
    world, c = spec["world"], visible_push(spec["cam"])
    blur = spec.get("defocus", 0)
    layers = [
        cam_comp(f"{sid}-sky", world["sky"] + spec.get("sky_extra", []), DEPTH["sky"], c, blur * 1.3),
        cam_comp(f"{sid}-back", world["back"] + spec.get("back_extra", []), DEPTH["back"], c, blur),
        cam_comp(f"{sid}-set", world["mid"] + spec.get("set_extra", []), DEPTH["mid"], c, blur * 0.8),
        # Action steps on twos; talking shots and the camera stay smooth.
        rimmed(cam_comp(f"{sid}-act", [{"id": f"{sid}-twos", "kind": "comp", "time": {"step_fps": 12 if sid in TWOS else 0}, "layers": spec.get("mid", [])}], DEPTH["mid"], c), s),
        cam_comp(f"{sid}-front", world.get("front", []) if spec.get("pit") else [], DEPTH["mid"], c, blur * 0.8),
        cam_comp(f"{sid}-fore", world["fore"] + spec.get("fore_extra", []), DEPTH["fore"], c),
    ]
    if spec.get("screen_extra"):
        layers.append(screen(f"{sid}-scr", spec["screen_extra"], t0, t1))
    root = {"id": sid, "kind": "comp", "layers": layers, "transform": {}}
    # Blur-whips: the outgoing and incoming shots move together edge to
    # edge (same ease, a full frame apart), smeared, two frames each side of
    # the cut.
    pos_keys, blur_keys, angle_keys = [], [], []
    dir_in = DIRS[(k - 1) % 4] if k else None
    dir_out = DIRS[k % 4] if k + 1 < len(SHOTS) else None
    span = lambda d: [d[0] * W, d[1] * H]
    if dir_in:
        pos_keys += [{"at": round(t0 - WHIP, 3), "v": [-v for v in span(dir_in)]}, {"at": round(t0 + WHIP, 3), "v": [0, 0], "ease": "gesture"}]
        blur_keys += [{"at": round(t0 - WHIP, 3), "v": 160}, {"at": round(t0, 3), "v": 260, "ease": "gesture"}, {"at": round(t0 + WHIP, 3), "v": 0, "ease": "gesture"}]
        angle_keys.append({"at": round(t0 - WHIP, 3), "v": 0 if dir_in[0] else 90})
    if dir_out:
        pos_keys += [{"at": round(t1 - WHIP, 3), "v": [0, 0]}, {"at": round(t1 + WHIP, 3), "v": span(dir_out), "ease": "gesture"}]
        blur_keys += [{"at": round(t1 - WHIP, 3), "v": 0}, {"at": round(t1, 3), "v": 260, "ease": "gesture"}, {"at": round(t1 + WHIP, 3), "v": 160, "ease": "gesture"}]
        angle_keys.append({"at": round(t1 - WHIP, 3), "v": 0 if dir_out[0] else 90, "ease": "hold"})
    if pos_keys:
        root["transform"]["position"] = {"keys": pos_keys}
        root["effects"] = [{"type": "directional_blur", "angle": {"keys": angle_keys} if len(angle_keys) > 1 else angle_keys[0]["v"], "length": {"keys": blur_keys}}]
    root["time"] = {"start": round(max(0, t0 - WHIP), 3), "end": round(min(END, t1 + WHIP), 3)}
    return root


# ---------------------------------------------------------------- captions ----

def chunks():
    """1–3 word caption chunks, broken at punctuation and pauses."""
    words = sorted(WORDS.values())
    out, cur = [], []
    for k, (s_, e_, tx) in enumerate(words):
        cur.append((s_, e_, tx))
        nxt = words[k + 1] if k + 1 < len(words) else None
        brk = tx[-1] in ",.:;!?" or len(cur) == 3 or (cur[-1][1] - cur[0][0]) > 0.9 or (nxt and nxt[0] - e_ > 0.15)
        if brk or nxt is None:
            out.append(cur)
            cur = []
    return out


CAPTION_FONT = "assets/fonts/Quicksand-Bold.ttf"


def captions():
    L = []
    ch = chunks()
    for k, c in enumerate(ch):
        a = c[0][0] - 2 * F
        b = ch[k + 1][0][0] - 2 * F if k + 1 < len(ch) else c[-1][1] + 1.2
        text = " ".join(tx for _, _, tx in c).rstrip(",;:")
        size = round(min(66, 0.84 * W / (0.6 * len(text))), 1)
        L.append({"id": f"cap{k:03d}", "kind": "text", "text": text, "font": CAPTION_FONT, "size": size, "color": "#ffe21a",
                  "weight": 2.0, "stroke": {"color": INK, "width": 9}, "tracking": 0, "role": "caption", "space": "screen",
                  "transform": {"position": [C[0], next((sh["cap_y"] for sh in SHOTS if sh["t0"] <= c[0][0] < sh["t1"]), 1540)], "opacity": {"keys": [{"at": round(a, 3), "v": 0.5}, {"at": round(a + 3 * F, 3), "v": 1, "ease": "hold"}]}},
                  "time": {"start": round(a, 3), "end": round(b, 3)}})
    return L


# ------------------------------------------------------------- MCP driver ----

class Mcp:
    def __init__(self, cwd):
        self.proc = subprocess.Popen([BIN, "mcp", "--cpu"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, cwd=cwd, text=True)
        self.id = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "olga", "version": "1"}})
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.id += 1
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params}) + "\n")
        self.proc.stdin.flush()
        while True:
            reply = json.loads(self.proc.stdout.readline())
            if reply.get("id") == self.id:
                if "error" in reply:
                    raise RuntimeError(reply["error"])
                return reply["result"]

    def tool(self, name, **args):
        started = time.time()
        r = self.request("tools/call", {"name": name, "arguments": args})
        text = r["content"][0]["text"]
        if r.get("isError"):
            raise RuntimeError(f"{name}: {text[:3000]}")
        print(f"  {name:<16} {time.time() - started:6.1f}s")
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text


def build():
    os.makedirs(OUT, exist_ok=True)
    mcp = Mcp(OUT)
    mcp.tool("project_create", path=PROJECT, width=W, height=H, fps=FPS, duration=END, overwrite=True)
    fonts = os.path.join(PROJECT, "assets", "fonts")
    os.makedirs(fonts, exist_ok=True)
    import shutil
    shutil.copy("/usr/share/fonts/truetype/quicksand/Quicksand-Bold.ttf", os.path.join(fonts, "Quicksand-Bold.ttf"))
    # Narration: clips and word/line/cue tracks.
    man = dict(MAN)
    man["clips"] = [dict(c, audio=os.path.join(OUT, c["audio"])) for c in MAN["clips"]]
    mp = os.path.join(OUT, "manifest-abs.json")
    json.dump(man, open(mp, "w"))
    rep = mcp.tool("narration_import", manifest=mp)
    if isinstance(rep, dict) and rep.get("problems"):
        print("  narration problems:", json.dumps(rep["problems"])[:800])
    shots = []
    sfx = []
    for k, s in enumerate(SHOTS):
        global NOW
        NOW = (s["t0"], s["t1"])
        spec = s["fn"](s)
        s["cap_y"] = spec.get("cap_y", 1540)
        shots.append(stage(s, spec, k))
        sfx += spec.get("sfx", [])
    film = {"id": "film", "kind": "comp", "layers": shots, "effects": FINISH}
    ops = [{"op": "add_scene", "id": "main", "start": 0, "background": INK},
           {"op": "add", "scene": "main", "layer": film}]
    for cap in captions():
        ops.append({"op": "add", "scene": "main", "layer": cap})
    assets = json.load(open(os.path.join(PROJECT, "project.json"))).get("assets", [])
    assets.append({"path": "assets/fonts/Quicksand-Bold.ttf", "source": "Debian package fonts-quicksand (Quicksand by Andrew Paglinawan)", "licence": "OFL-1.1"})
    ops.append({"op": "set", "target": "project.assets", "value": assets})
    path = os.path.join(OUT, "ops.json")
    json.dump(ops, open(path, "w"))
    print(f"  ops {os.path.getsize(path) / 1e6:.1f} MB, {len(SHOTS)} shots")
    mcp.tool("edit_apply", ops=ops)
    # Sound effects: whips, stings, hits, chimes.
    for k in range(1, len(SHOTS)):
        sfx.append(("whoosh", SHOTS[k]["t0"] - 0.18, -17))
    groups = {}
    for name, at, gain in sfx:
        for a in (at if isinstance(at, list) else [at]):
            groups.setdefault((name, gain), []).append(round(max(0, a), 3))
    for (name, gain), ats in sorted(groups.items()):
        mcp.tool("sfx_add", sound=name, at=sorted(ats), gain_db=gain)
    print(f"  {sum(len(v) for v in groups.values())} sound effects in {len(groups)} groups")
    return mcp


def contact_sheet(mcp):
    ats = ",".join(f"{(s['t0'] + s['t1']) / 2:.2f}" for s in SHOTS)
    png = os.path.join(OUT, "contact-sheet.png")
    run("motion", "snapshot", PROJECT, "--at", ats, "--sheet", "--width", 300, "-o", png)
    print(png)


def shot_list():
    lines = ["| # | time | words | on screen |", "|---|---|---|---|"]
    for s in SHOTS:
        words = " ".join(tx for (l_, i), (a, b, tx) in sorted(WORDS.items(), key=lambda kv: kv[1][0]) if s["t0"] <= a < s["t1"])
        doc = (s["fn"].__doc__ or "").strip()
        src = open(__file__).read()
        i = src.index(f"def {s['sid']}(s):")
        comment = src[i:src.index("\n", i)].split("#", 1)[1].split("—", 1)[1].strip()
        lines.append(f"| {s['sid']} | {s['t0']:.1f}–{s['t1']:.1f} | {words} | {comment} |")
    open(os.path.join(OUT, "shot-list.md"), "w").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    mcp = build()
    shot_list()
    if MODE == "sheet":
        contact_sheet(mcp)
    elif MODE == "test":
        print(run("motion", "render", PROJECT, "-o", os.path.join(OUT, "olga-test.mp4"), "--from", 0, "--to", round(SHOTS[3]["t0"] + 0.05, 2)))
    elif MODE in ("render", "draft"):
        args = ["motion", "render", PROJECT, "-o", os.path.join(OUT, "olga.mp4" if MODE == "render" else "olga-draft.mp4")]
        if MODE == "draft":
            args.append("--draft")
        print(run(*args))
