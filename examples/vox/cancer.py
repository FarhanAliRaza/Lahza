"""Why is cancer so hard to cure? A 68 s Vox-style vertical short, built
entirely through the Lahza motion engine's MCP tools.

Collage on paper, cut-out cards with tape, highlighter swipes, stepped
12 fps animation, captions that follow the narration, generated microscope
slides, and real 3D (meshes, glass, shadows) where things are objects:
coins, cells, a DNA helix, a tumour, a pill.

    python3 cancer.py OUT_DIR [path/to/lahza-motion]

OUT_DIR must already hold vo.wav + vo.json (narrate.py), music.wav
(music.py) and art/ (assets.py). Set SKIP_RENDER=1 for the contact sheet
only, QUALITY=draft for a half-size render.
"""
import base64
import json
import math
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "vox-out")
BIN = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "../../target/release/lahza-motion")
VO = json.load(open(os.path.join(OUT, "vo.json")))
L = VO["lines"]

W, H = 1080, 1920
C = [W / 2, H / 2]
PAPER = "#f4f0e7"
INK = "#1b1b1f"
YELLOW = "#ffd23f"
RED = "#e63946"
CORAL = "#ff7b6b"
TEAL = "#2a9d8f"
BLUE = "#3a6ea5"
PURPLE = "#6d4c9f"
CREAM = "#fffaf0"
MUTED = ["#c9b8a0", "#9fb3c8", "#d4a5a5", "#a8c3a0", "#c4a7d0", "#e0c58f", "#8fb8b0"]

BLACK = "assets/fonts/InterDisplay-Black.otf"
BOLD = "assets/fonts/InterDisplay-Bold.otf"
SEMI = "assets/fonts/Inter-SemiBold.otf"
SERIF_B = "assets/fonts/LiberationSerif-Bold.ttf"
SERIF_I = "assets/fonts/LiberationSerif-Italic.ttf"
SERIF_BI = "assets/fonts/LiberationSerif-BoldItalic.ttf"
MONO = "assets/fonts/JetBrainsMono-Bold.ttf"
FONT_FILES = {
    "InterDisplay-Black.otf": "/usr/share/fonts/opentype/inter/InterDisplay-Black.otf",
    "InterDisplay-Bold.otf": "/usr/share/fonts/opentype/inter/InterDisplay-Bold.otf",
    "Inter-SemiBold.otf": "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf",
    "LiberationSerif-Bold.ttf": "/usr/share/fonts/truetype/liberation/LiberationSerif-Bold.ttf",
    "LiberationSerif-Italic.ttf": "/usr/share/fonts/truetype/liberation/LiberationSerif-Italic.ttf",
    "LiberationSerif-BoldItalic.ttf": "/usr/share/fonts/truetype/liberation/LiberationSerif-BoldItalic.ttf",
    "JetBrainsMono-Bold.ttf": "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Bold.ttf",
}


class Mcp:
    """A minimal MCP client over stdio."""

    def __init__(self, cwd):
        self.proc = subprocess.Popen([BIN, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, cwd=cwd, text=True)
        self.id = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "vox", "version": "1"}})
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

    def tool(self, tool_name, **args):
        started = time.time()
        result = self.request("tools/call", {"name": tool_name, "arguments": args})
        text = result["content"][0]["text"]
        if result.get("isError"):
            raise RuntimeError(f"{tool_name}: {text}")
        if tool_name != "sfx_add":
            print(f"  {tool_name:<14} {time.time() - started:6.1f}s")
        return json.loads(text), [c for c in result["content"][1:] if c["type"] == "image"]


# --- time --------------------------------------------------------------------

def T(i, d=0.0):
    """Start of narration line i, plus d seconds."""
    return round(L[i]["start"] + d, 3)


def E(i, d=0.0):
    return round(L[i]["end"] + d, 3)


def at_word(i, word, d=0.0):
    """Roughly when `word` is said in line i (by its place in the text)."""
    text = L[i]["text"]
    k = text.lower().find(word.lower())
    assert k >= 0, (word, text)
    return round(L[i]["start"] + (L[i]["end"] - L[i]["start"]) * k / len(text) + d, 3)


def scene_window(name):
    lines = [l for l in L if l["scene"] == name]
    order = [l["scene"] for l in L]
    names = list(dict.fromkeys(order))
    k = names.index(name)
    start = 0.0 if k == 0 else round(lines[0]["start"] - 0.35, 3)
    end = VO["duration"] if k == len(names) - 1 else round([l for l in L if l["scene"] == names[k + 1]][0]["start"] - 0.35, 3)
    return start, end


# --- animation helpers ---------------------------------------------------------

def q(t):
    """On the 12 fps step grid (stepped layers land exactly there)."""
    return round(round(t * 12) / 12, 4)


def tween(t, frm, to, dur="8f", ease="enter"):
    # Stepped layers show a change one 12 fps step after a key starts:
    # lead by a step so the change lands on the word.
    t = q(t - 1 / 12)
    return {"keys": [{"at": t, "from": frm, "v": to, "ease": ease, "dur": dur, "anchor": "start"}]}


def keys(*pts, ease="glide"):
    return {"keys": [{"at": q(t), "v": v, "ease": ease} for t, v in pts]}


def pop(t, dur="9f", to=1.0):
    return tween(t, 0, to, dur, "pop")


def fade_in(t, dur="6f"):
    return tween(t, 0, 1, dur, "enter")


def inout(t0, t1, dur="6f"):
    """Opacity in at t0, out at t1."""
    return {"keys": [{"at": q(t0 - 1 / 12), "from": 0, "v": 1, "ease": "enter", "dur": dur, "anchor": "start"},
                     {"at": q(t1), "v": 0, "ease": "glide", "dur": dur, "anchor": "end"}]}


def tr(pos, **extra):
    t = {"position": pos}
    t.update(extra)
    return t


def win(layer, start=None, end=None):
    t = layer.setdefault("time", {})
    for k, v in (("start", start), ("end", end)):
        if v is not None:
            t[k] = q(v)
    return layer


# --- building blocks -------------------------------------------------------------

def paper(sid):
    """The page: textured paper with film grain that boils at 12 fps."""
    return {"id": f"{sid}-paper", "kind": "image", "src": "assets/art/paper.png", "size": [W, H], "transform": tr(C),
            "effects": [{"type": "grain", "amount": 0.09, "size": 1.7, "seed": 7}]}


def text(id, s, size, pos, color=INK, font=BOLD, **extra):
    layer = {"id": id, "kind": "text", "text": s, "size": size, "color": color, "font": font, "transform": tr(pos)}
    layer.update(extra)
    return layer


def highlight(id, at, pos, width, height=60, dur="7f", color=YELLOW):
    """A highlighter swipe drawn left to right (multiplied over the paper)."""
    return {"id": id, "kind": "shape", "shape": "line", "size": [width, 0], "thickness": height, "fill": color, "blend": "multiply",
            "trim": tween(at, [0, 0], [0, 1], dur, "glide"), "transform": tr(pos, rotation=-1.5)}


def card(id, size, pos, at, rotation=-2.0, fill=CREAM):
    """A cut-out card with a soft drop shadow, popping in."""
    return {"id": id, "kind": "shape", "shape": "rect", "size": size, "radius": 6, "fill": fill,
            "effects": [{"type": "grain", "amount": 0.07, "size": 1.5, "seed": 3}, {"type": "shadow", "color": "#00000038", "offset": [8, 14], "radius": 14}],
            "transform": tr(pos, rotation=rotation, scale=tween(at, 0.6, 1, "8f", "land"), opacity=fade_in(at, "3f"))}


def moire_rings(id, center, radius, color, start, end, opacity=0.3, step=15, drift=46):
    """Two sets of concentric rings sliding over each other: live moiré."""
    n = int(radius / step)
    def rings(tag):
        return [{"id": f"{id}-{tag}{k}", "kind": "shape", "shape": "ring", "size": (k + 1) * step * 2, "thickness": step * 0.42, "fill": color,
                 "transform": tr([0, 0])} for k in range(n)]
    a = {"id": f"{id}-A", "kind": "comp", "transform": tr([0, 0]), "layers": rings("a")}
    b = {"id": f"{id}-B", "kind": "comp", "layers": rings("b"),
         "transform": tr([0, 0], position=keys((start, [-drift, -drift * 0.4]), (end, [drift, drift * 0.7]), ease="linear"))}
    return win({"id": id, "kind": "comp", "blend": "multiply", "layers": [a, b],
                "transform": tr(center, scale=tween(start, 0.85, 1, "10f", "land"),
                                opacity={"keys": [{"at": q(start - 1 / 12), "from": 0, "v": opacity, "ease": "enter", "dur": "6f", "anchor": "start"}]})}, start, end)


def moire_lines(id, center, size, color, start, end, opacity=0.25, spacing=13, turn=9):
    """Two line gratings, one turning slowly over the other, inside a box."""
    w, h = size
    n = int(h * 1.5 / spacing)
    def grating(tag, rotation):
        line = {"id": f"{id}-{tag}l", "kind": "shape", "shape": "line", "size": [w * 1.6, 0], "thickness": spacing * 0.45, "fill": color}
        return {"id": f"{id}-{tag}", "kind": "instancer", "layout": {"type": "line", "count": n, "from": [0, -h * 0.75], "to": [0, h * 0.75]},
                "transform": tr([0, 0], rotation=rotation), "layers": [line]}
    mask = {"id": f"{id}-mask", "kind": "shape", "shape": "rect", "size": [w, h], "radius": 18, "fill": "#ffffff", "hidden": True}
    lines = {"id": f"{id}-lines", "kind": "comp", "matte": {"layer": f"{id}-mask", "mode": "alpha"},
             "layers": [grating("a", 0), grating("b", keys((start, -turn / 2), (end, turn / 2), ease="linear"))]}
    return win({"id": id, "kind": "comp", "blend": "multiply", "layers": [mask, lines],
                "transform": tr(center, opacity={"keys": [{"at": q(start - 1 / 12), "from": 0, "v": opacity, "ease": "enter", "dur": "6f", "anchor": "start"}]})}, start, end)


def tape(id, pos, at, rotation):
    return {"id": id, "kind": "image", "src": "assets/art/tape.png", "size": [170, 46], "transform": tr(pos, rotation=rotation, opacity=fade_in(at, "2f"))}


def chapter(sid, label, start):
    w = 36 * len(label) * 0.64 + 80
    return [
        {"id": f"{sid}-tag", "kind": "shape", "shape": "rect", "size": [w, 80], "radius": 40, "fill": INK,
         "transform": tr([W / 2, 190], scale=tween(start + 0.1, 0.5, 1, "7f", "land"))},
        text(f"{sid}-tagtx", label, 36, [W / 2, 190], CREAM, SEMI, tracking=70, transform=tr([W / 2, 192], opacity=fade_in(start + 0.15, "3f"))),
    ]


def wrap(s, width=24):
    words, lines, cur = s.split(), [], ""
    for w in words:
        if cur and len(cur) + 1 + len(w) > width:
            lines.append(cur)
            cur = w
        else:
            cur = (cur + " " + w).strip()
    lines.append(cur)
    return "\n".join(lines)


def captions(sid, end):
    """Each narration line as a caption whose words light up as they are said."""
    out = []
    idx = [i for i, l in enumerate(L) if l["scene"] == sid]
    for n, i in enumerate(idx):
        line = L[i]
        nxt = L[idx[n + 1]]["start"] - 0.15 if n + 1 < len(idx) else end
        words = len(line["text"].split())
        per = max(1, round((line["end"] - line["start"]) / words * 30 * 0.9))
        long = len(line["text"]) > 70
        out.append(win(text(f"{sid}-cap{n}", wrap(line["text"], 30 if long else 25), 46 if long else 54, [W / 2, CAPTION_Y], INK, BOLD, line_height=1.16,
                            animators=[{"unit": "word", "at": q(T(i, -0.02)), "from": {"opacity": 0.22}, "stagger": f"{per}f", "dur": "3f", "ease": "enter"}]),
                       T(i, -0.1), round(nxt, 3)))
    return out


TRANSITIONS = {"many": "push", "self": "slide", "typo": "push", "crowd": "slide", "spread": "push", "hope": "wipe", "end": "push"}


# Shorts-safe layout: everything a scene draws (designed on the full
# 1080×1920 page) sits in a stage scaled into the middle band, clear of
# the platform's top bar, side buttons and bottom captions; the narration
# captions sit just below the stage.
STAGE_SCALE = 0.8
STAGE_FROM = [W / 2, 905]
STAGE_TO = [W / 2, 845]
CAPTION_Y = 1410


def scene(sid, layers, step=12, camera=None, lights=None, environment=None):
    start, end = scene_window(sid)
    page = [l for l in layers if l["id"].endswith("-paper")]
    caps = [l for l in layers if "-cap" in l["id"]]
    rest = [l for l in layers if l not in page and l not in caps]
    stage = {"id": f"{sid}-stage", "kind": "comp", "layers": rest,
             "transform": {"position": STAGE_TO, "anchor": STAGE_FROM, "scale": STAGE_SCALE}}
    s = {"op": "add_scene", "id": sid, "start": start, "end": end, "background": PAPER, "step_fps": step,
         "camera": camera or {"zoom": keys((start, 1.0), (end, 1.03), ease="linear")}, "layers": page + [stage] + caps}
    if sid in TRANSITIONS:
        s["transition"] = {"type": TRANSITIONS[sid], "dur": "12f", "direction": "up"}
    if lights:
        s["lights"] = lights
    if environment:
        s["environment"] = environment
    return s


def smooth(layer):
    """3D objects move smoothly (not stepped)."""
    layer.setdefault("time", {})["step_fps"] = 0
    return layer


def shadow_catcher(sid, z=260, opacity=0.28):
    return {"id": f"{sid}-catch", "kind": "mesh", "mesh": {"type": "plane", "size": [3200, 4400]},
            "material": {"preset": "shadow", "opacity": opacity}, "transform": tr(C, z=z)}


SOFT_LIGHT = [{"from": [-0.6, -1.0, -1.2], "softness": 0.7, "intensity": 1.05}, {"from": [0.9, -0.3, 0.8], "intensity": 0.55}]


# --- scenes --------------------------------------------------------------------------

def hook():
    sid = "hook"
    start, end = scene_window(sid)
    t1971 = at_word(0, "1971", 0.05)
    war = at_word(0, "war", -0.05)
    fifty = T(1, 0.0)
    billions = at_word(1, "billions")
    cure = at_word(1, "cure", -0.15)
    why = T(2, -0.05)
    leave = fifty - 0.25
    L_ = [paper(sid)]
    # The 1971 document, taped to the page.
    doc = [
        card("doc", [760, 560], [W / 2, 720], 0.35, -2.5),
        text("doc-h", "THE NATIONAL\nCANCER ACT", 82, [W / 2, 640], INK, SERIF_B, line_height=1.0, transform=tr([W / 2, 650], rotation=-2.5, opacity=fade_in(0.45, "3f"))),
        text("doc-s", "Signed into law\nDecember 23, 1971", 44, [W / 2, 860], "#3b3b40", SERIF_I, transform=tr([W / 2, 850], rotation=-2.5, opacity=fade_in(0.6, "3f"))),
        tape("doc-t1", [W / 2 - 330, 455], 0.5, -18), tape("doc-t2", [W / 2 + 330, 470], 0.55, 14),
        {"id": "stamp", "kind": "shape", "shape": "ring", "size": 250, "thickness": 12, "fill": RED, "transform": tr([820, 960], rotation=-14, scale=tween(t1971, 1.8, 1, "6f", "land"), opacity=fade_in(t1971, "2f"))},
        text("stamp-tx", "1971", 76, [820, 960], RED, BLACK, transform=tr([820, 962], rotation=-14, scale=tween(t1971, 1.8, 1, "6f", "land"), opacity=fade_in(t1971, "2f"))),
    ]
    for l in doc:
        l["transform"]["position"] = l["transform"]["position"]
        l["transform"].setdefault("opacity", 1)
        L_.append(win(l, None, leave + 0.3))
    # Slide the document up and away when the years go by.
    L_.append(win(text("war", "WAR ON CANCER", 104, [W / 2, 1270], INK, BLACK, transform=tr([W / 2, 1270], opacity=fade_in(war, "2f"))), war, leave + 0.3))
    L_.insert(len(L_) - 1, win(highlight("war-hl", war - 0.05, [W / 2, 1272], 900, 108), war - 0.05, leave + 0.3))
    # Fifty years, billions: stacks of gold coins (3D).
    years = win(text("years", "50 YEARS", 150, [W / 2, 520], INK, BLACK, transform=tr([W / 2, 520], scale=tween(fifty, 0.7, 1, "6f", "land"), opacity=fade_in(fifty, "2f"))), fifty, why - 0.2)
    later = win(text("later", "of research", 56, [W / 2, 660], "#55555c", SERIF_I, transform=tr([W / 2, 660], opacity=fade_in(fifty + 0.3, "3f"))), fifty, why - 0.2)
    L_ += [years, later]
    L_.insert(1, moire_lines("coins-moire", [W / 2, 1060], [820, 560], "#b8862b", billions - 0.9, why - 0.2, 0.22))
    for k, (x, n) in enumerate([(300, 6), (540, 9), (780, 7)]):
        coin = {"id": f"coin{k}", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 92, "height": 22, "bevel": 5},
                "material": {"preset": "metal", "color": "#e8b84a", "roughness": 0.28}, "parts": {"side": {"color": "#c9952e"}},
                "transform": tr([0, 0], rotation_x=-62, position=tween(billions - 0.6 + k * 0.12, [0, -520], [0, 0], "9f", "land"))}
        stack = {"id": f"stack{k}", "kind": "instancer", "layout": {"type": "line", "count": n, "from": [0, 0], "to": [0, -24 * (n - 1)]},
                 "stagger": {"each": "2f"}, "transform": tr([x, 1150]), "layers": [coin]}
        L_.append(smooth(win(stack, billions - 0.8, why - 0.2)))
    L_.append(win(text("bn", "$ BILLIONS", 92, [W / 2, 1360], INK, BLACK, transform=tr([W / 2, 1360], opacity=fade_in(billions, "3f"))), billions, why - 0.2))
    L_.append(win({"id": "nocure", "kind": "shape", "shape": "rect", "size": [620, 120], "radius": 10, "fill": RED,
                   "transform": tr([W / 2, 820], rotation=4, scale=tween(cure, 1.6, 1, "5f", "land"), opacity=fade_in(cure, "2f"))}, cure, why - 0.2))
    L_.append(win(text("nocure-tx", "NO CURE", 92, [W / 2, 820], CREAM, BLACK, transform=tr([W / 2, 822], rotation=4, scale=tween(cure, 1.6, 1, "5f", "land"), opacity=fade_in(cure, "2f"))), cure, why - 0.2))
    # Why? Over live moiré rings.
    L_.append(moire_rings("why-moire", [W / 2, 870], 520, RED, why - 0.1, end, 0.32))
    L_.append(win(text("why", "Why?", 380, [W / 2, 860], INK, SERIF_BI, transform=tr([W / 2, 860], scale=tween(why, 0.5, 1, "8f", "pop"), opacity=fade_in(why, "2f"))), why, None))
    L_.append(win(highlight("why-hl", why + 0.25, [W / 2 + 10, 1010], 620, 34, "6f", RED), why + 0.2, None))
    L_ += captions(sid, end)
    sfx = [("thud", t1971, -4), ("swipe", war - 0.05, -8), ("pop", billions - 0.5, -10), ("pop", billions - 0.3, -10), ("pop", billions - 0.1, -10), ("thud", cure, -2), ("whoosh", why - 0.15, -8)]
    return scene(sid, L_, lights=SOFT_LIGHT, environment={"preset": "studio"}), sfx


def many():
    sid = "many"
    start, end = scene_window(sid)
    two = at_word(4, "two hundred", -0.1)
    L_ = [paper(sid)] + chapter(sid, "1 · NOT ONE DISEASE", start)
    L_.append(moire_lines("cancer-moire", [W / 2, 860], [820, 380], INK, start + 0.2, at_word(4, "two hundred", 0.1), 0.2))
    # One label, "CANCER", that turns out to be many.
    one = win(text("one", "CANCER", 150, [W / 2, 860], INK, BLACK, transform=tr([W / 2, 860], scale=tween(T(3), 0.8, 1, "6f", "land"), opacity=inout(T(3) - 0.1, two - 0.1, "3f"))), start, two + 0.1)
    one_hl = win(highlight("one-hl", T(3) + 0.4, [W / 2, 866], 600, 120), start, two + 0.1)
    L_ += [one_hl, one]
    dot = {"id": "type", "kind": "shape", "shape": "circle", "size": 44, "fill": "#999999",
           "transform": tr([0, 0], scale=tween(two, 0, 1, "6f", "pop"))}
    grid = {"id": "types", "kind": "instancer", "layout": {"type": "grid", "columns": 13, "rows": 15, "spacing": [66, 54], "hex": True},
            "palette": MUTED + [CORAL, "#b07aa1"], "palette_order": "random", "stagger": {"total": 1.0, "order": "random", "seed": 4},
            "random": {"seed": 9, "scale": [0.7, 1.15]}, "transform": tr([W / 2, 900]), "layers": [dot]}
    L_.append(win(grid, two - 0.1, None))
    big = win(text("n200", "200+", 210, [W / 2, 430], INK, BLACK, transform=tr([W / 2, 430], scale=tween(two + 0.2, 0.6, 1, "7f", "pop"), opacity=fade_in(two + 0.2, "2f"))), two, None)
    L_ += [win(highlight("n200-hl", two + 0.35, [W / 2, 450], 520, 140), two, None), big]
    names = [("lung", [250, 640]), ("breast", [800, 700]), ("leukemia", [230, 1180]), ("melanoma", [820, 1120]), ("brain", [560, 590]), ("pancreas", [560, 1300])]
    rules = at_word(4, "rules", -0.6)
    for k, (name, pos) in enumerate(names):
        t = two + 0.5 + k * 0.28
        L_.append(win({"id": f"lbl{k}", "kind": "shape", "shape": "rect", "size": [len(name) * 24 + 50, 62], "radius": 4, "fill": CREAM,
                       "effects": [{"type": "shadow", "color": "#00000030", "offset": [4, 8], "radius": 8}],
                       "transform": tr(pos, rotation=(-4 if k % 2 else 3), scale=tween(t, 0.4, 1, "6f", "land"))}, t, None))
        L_.append(win(text(f"lbl{k}tx", name, 40, pos, INK, SERIF_I, transform=tr([pos[0], pos[1] + 2], rotation=(-4 if k % 2 else 3), opacity=fade_in(t + 0.03, "2f"))), t, None))
    L_ += captions(sid, end)
    sfx = [("pop", two, -6), ("sparkle", two + 0.2, -12)] + [("tick", two + 0.5 + k * 0.28, -10) for k in range(len(names))]
    return scene(sid, L_), sfx


def cell(id, pos, nucleus, at, radius=170, nucleus_r=70, extra=None):
    """A glass cell (refracting what is inside) with a nucleus and organelles."""
    layers = [
        {"id": f"{id}-nuc", "kind": "mesh", "mesh": {"type": "sphere", "radius": nucleus_r}, "material": {"preset": "glossy", "color": nucleus},
         "transform": tr([pos[0] + 20, pos[1] + 10], z=10, scale=pop(at, "10f"))},
        {"id": f"{id}-mito", "kind": "mesh", "mesh": {"type": "capsule", "radius": 16, "height": 70}, "material": {"preset": "glossy", "color": "#f08a4b"},
         "transform": tr([pos[0] - 70, pos[1] + 70], z=40, rotation=40, scale=pop(at + 0.1, "10f"))},
        {"id": f"{id}-mito2", "kind": "mesh", "mesh": {"type": "capsule", "radius": 14, "height": 60}, "material": {"preset": "glossy", "color": "#f08a4b"},
         "transform": tr([pos[0] + 80, pos[1] - 80], z=-30, rotation=-30, scale=pop(at + 0.15, "10f"))},
        {"id": f"{id}-mem", "kind": "mesh", "mesh": {"type": "sphere", "radius": radius},
         "material": {"preset": "glass", "color": "#e8f6f3", "ior": 1.25, "thickness": 120, "roughness": 0.12},
         "transform": tr(pos, scale=pop(at, "10f"))},
    ]
    for l in layers:
        smooth(l)
    return layers


def self_():
    sid = "self"
    start, end = scene_window(sid)
    virus_t = at_word(5, "virus", -0.2)
    own = at_word(6, "own cells", -0.1)
    hit = at_word(6, "hit", -0.1)
    you = at_word(6, "hitting you", -0.05)
    L_ = [paper(sid)] + chapter(sid, "2 · IT'S MADE OF YOU", start)
    L_.append(moire_rings("virus-moire", [W / 2, 860], 380, "#c0392b", start + 0.2, own - 0.3, 0.26, 13, 40))
    # A virus: a foreign invader, easy to tell apart.
    spike = {"id": "spike", "kind": "mesh", "mesh": {"type": "capsule", "radius": 10, "height": 70}, "material": {"preset": "glossy", "color": "#d9534f"},
             "transform": tr([0, 0], position=[0, -40])}
    tip = {"id": "tip", "kind": "mesh", "mesh": {"type": "sphere", "radius": 17}, "material": {"preset": "glossy", "color": "#f2c14e"},
           "transform": tr([0, -80])}
    virus = [
        smooth(win({"id": "virus-body", "kind": "mesh", "mesh": {"type": "sphere", "radius": 120}, "material": {"preset": "coated", "color": "#c0392b"},
                    "transform": tr([W / 2, 860], scale=pop(virus_t, "10f"))}, start, own - 0.3)),
        smooth(win({"id": "virus-spikes", "kind": "instancer", "layout": {"type": "radial", "count": 14, "radius": 150, "align": True},
                    "transform": tr([W / 2, 860], rotation=keys((virus_t, 0), (own, 30), ease="linear"), scale=pop(virus_t + 0.1, "10f")), "layers": [spike, tip]}, start, own - 0.3)),
    ]
    L_ += virus
    L_.append(win(text("invader", "INVADER?", 96, [W / 2, 1180], RED, BLACK, transform=tr([W / 2, 1180], rotation=-3, opacity=fade_in(at_word(5, "invader", -0.2), "2f"))), start, own - 0.3))
    L_.append(win({"id": "invader-x", "kind": "shape", "shape": "line", "size": [520, 0], "thickness": 14, "fill": INK,
                   "trim": tween(at_word(5, "isn't", -0.1), [0, 0], [0, 1], "5f", "glide"), "transform": tr([W / 2, 1180], rotation=-8)}, start, own - 0.3))
    # Your own cells: healthy and cancerous look alike.
    L_ += [win(l, own - 0.25, None) for l in cell("good", [300, 860], "#6d8fd8", own - 0.2)]
    L_ += [win(l, own - 0.25, None) for l in cell("bad", [780, 860], "#7a3b8c", own, nucleus_r=82)]
    L_.append(win(text("good-l", "healthy", 50, [300, 1110], INK, SERIF_I, transform=tr([300, 1110], opacity=fade_in(own + 0.3, "3f"))), own, None))
    L_.append(win(text("bad-l", "cancer", 50, [780, 1110], RED, SERIF_BI, transform=tr([780, 1110], opacity=fade_in(own + 0.5, "3f"))), own, None))
    # A crosshair that can't decide.
    x = keys((hit - 0.3, 780), (hit + 0.5, 300), (hit + 1.0, 760), (you, 540), ease="gesture")
    for k, (shape, size, th) in enumerate([("ring", 200, 8), ("ring", 70, 6)]):
        L_.append(win({"id": f"aim{k}", "kind": "shape", "shape": shape, "size": size, "thickness": th, "fill": RED,
                       "transform": {"position": [780, 860], "opacity": fade_in(hit - 0.3, "3f")}}, hit - 0.3, None))
    for k, (dx, dy, rot) in enumerate([(0, -130, 90), (0, 130, 90), (-130, 0, 0), (130, 0, 0)]):
        L_.append(win({"id": f"aimx{k}", "kind": "shape", "shape": "line", "size": [80, 0], "thickness": 6, "fill": RED, "parent": "aim0",
                       "transform": {"position": [dx, dy], "rotation": rot}}, hit - 0.3, None))
    for l in L_:
        if l["id"] in ("aim0", "aim1"):
            l["transform"]["position"] = {"keys": [{"at": k["at"], "v": [k["v"], 860], "ease": k["ease"]} for k in x["keys"]]}
    L_.append(win(text("both", "HITS BOTH", 72, [W / 2, 1290], CREAM, BLACK, transform=tr([W / 2, 1290], opacity=fade_in(you + 0.25, "2f"))), you + 0.2, None))
    L_.insert(len(L_) - 1, win({"id": "both-bg", "kind": "shape", "shape": "rect", "size": [470, 110], "radius": 8, "fill": RED,
                                "transform": tr([W / 2, 1290], rotation=-2, scale=tween(you + 0.2, 1.5, 1, "5f", "land"))}, you + 0.2, None))
    L_.append(shadow_catcher(sid))
    L_ += captions(sid, end)
    sfx = [("pop", virus_t, -8), ("swipe", at_word(5, "isn't", -0.1), -8), ("whoosh", own - 0.3, -10), ("tick", hit - 0.2, -10), ("tick", hit + 0.5, -10), ("tick", hit + 1.0, -10), ("hit", you + 0.2, -6)]
    return scene(sid, L_, lights=SOFT_LIGHT, environment={"preset": "soft"}), sfx


def helix(at, end):
    """A DNA double helix: two strands of beads with base-pair rungs (3D)."""
    layers = [{"id": "dna", "kind": "shape", "shape": "circle", "size": 1, "three_d": True,
               "transform": tr([W / 2, 760], rotation=-12, rotation_x=keys((at, 0), (end, 220), ease="linear"))}]
    n, length, amp = 21, 820, 104
    pairs = [("#f2c14e", "#e76f51"), ("#2a9d8f", "#3a6ea5")]
    for i in range(n):
        x = -length / 2 + length * i / (n - 1)
        a = i * 0.62
        for s, ph in ((0, 0.0), (1, math.pi)):
            y, z = amp * math.cos(a + ph), amp * math.sin(a + ph)
            layers.append({"id": f"bead{s}-{i}", "kind": "mesh", "mesh": {"type": "sphere", "radius": 17}, "parent": "dna",
                           "material": {"preset": "glossy", "color": "#2c3e66" if s == 0 else "#5b3d8a"},
                           "transform": tr([x, y], z=z, scale=pop(at + i * 0.03, "8f"))})
        # The rung: two halves (one base each) across the helix at angle a.
        ca, cb = pairs[i % 2] if i % 3 else pairs[(i + 1) % 2]
        for h, color in ((0, ca), (1, cb)):
            off = (amp / 2) * (1 if h == 0 else -1)
            layers.append({"id": f"rung{h}-{i}", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 7, "height": amp * 0.95}, "parent": "dna",
                           "material": {"preset": "glossy", "color": color},
                           "transform": tr([x, off * math.cos(a)], z=off * math.sin(a), rotation_x=-math.degrees(a), scale=pop(at + i * 0.03 + 0.1, "8f"))})
    return [smooth(win(l, at - 0.1, end)) for l in layers]


def typo():
    sid = "typo"
    start, end = scene_window(sid)
    copies = at_word(7, "copies")
    letters = at_word(7, "three billion", -0.1)
    typos = at_word(8, "typos", -0.15)
    fail = at_word(8, "fail", -0.25)
    one = T(9, -0.05)
    two = at_word(9, "two", -0.1)
    four = at_word(9, "four", -0.1)
    billions = at_word(9, "billions", -0.15)
    L_ = [paper(sid)] + chapter(sid, "3 · TYPOS IN THE CODE", start)
    L_ += helix(T(7, 0.1), one - 0.1)
    L_.append(win(text("3bn", "3,000,000,000", 92, [W / 2, 1060], INK, BLACK, transform=tr([W / 2, 1060], opacity=fade_in(letters, "3f"))), letters, typos - 0.65))
    L_.append(win(text("3bn-l", "letters, copied every time a cell divides", 38, [W / 2, 1140], "#4a4a50", SERIF_I, transform=tr([W / 2, 1140], opacity=fade_in(letters + 0.3, "3f"))), letters, typos - 0.65))
    # The code, typed, and one letter that goes wrong.
    code = "ATGCCGTATTGGCGTACCAGT"
    k = 10
    step = 0.6 * 60
    xk = W / 2 + (k - (len(code) - 1) / 2) * step
    L_.append(win(text("code", code, 60, [W / 2, 1100], INK, MONO, animators=[{"unit": "glyph", "at": q(typos - 0.6), "from": {"opacity": 0}, "stagger": "1f", "dur": "1f"}]), typos - 0.6, one - 0.1))
    L_.append(win({"id": "mut-box", "kind": "shape", "shape": "rect", "size": [42, 70], "radius": 4, "fill": RED, "transform": tr([xk, 1098], scale=tween(typos + 0.35, 1.8, 1, "5f", "land"))}, typos + 0.35, one - 0.1))
    L_.append(win(text("mut-a", "A", 60, [xk, 1100], CREAM, MONO), typos + 0.35, one - 0.1))
    L_.append(win({"id": "mut-ring", "kind": "shape", "shape": "arc", "size": 150, "thickness": 7, "angles": [-20, 330], "fill": RED,
                   "trim": tween(typos + 0.5, [0, 0], [0, 1], "8f", "glide"), "transform": tr([xk, 1100], rotation=10)}, typos + 0.5, one - 0.1))
    L_.append(win(text("mut-l", "typo!", 64, [xk + 150, 980], RED, SERIF_BI, transform=tr([xk + 150, 980], rotation=-8, opacity=fade_in(typos + 0.7, "2f"))), typos + 0.7, one - 0.1))
    # A growth switch flips off.
    sx, sy = W / 2, 1300
    L_.append(win(text("sw-l", "GROWTH BRAKES", 40, [sx - 150, sy], INK, SEMI, tracking=60, transform=tr([sx - 150, sy], opacity=fade_in(typos + 1.2, "3f"))), typos + 1.1, one - 0.1))
    L_.append(win({"id": "sw-track", "kind": "shape", "shape": "capsule", "size": [150, 70],
                   "fill": keys((fail - 0.05, TEAL), (fail + 0.15, RED), ease="linear"), "transform": tr([sx + 170, sy], opacity=fade_in(typos + 1.2, "3f"))}, typos + 1.1, one - 0.1))
    L_.append(win({"id": "sw-knob", "kind": "shape", "shape": "circle", "size": 56, "fill": CREAM,
                   "transform": tr([sx + 210, sy], position=keys((fail - 0.05, [sx + 210, sy]), (fail + 0.2, [sx + 130, sy]), ease="snap"), opacity=fade_in(typos + 1.2, "3f"))}, typos + 1.1, one - 0.1))
    L_.append(win(text("sw-off", "OFF", 40, [sx + 300, sy], RED, BLACK, transform=tr([sx + 300, sy - 70], opacity=fade_in(fail + 0.2, "2f"))), fail, one - 0.1))
    # One becomes two, four... billions, over a moiré that starts to boil.
    L_.append(moire_lines("div-moire", [W / 2, 860], [860, 760], RED, one - 0.15, billions + 0.2, 0.2, 12, 14))
    def ball(id, pos_keys, at, until, r=80, grow=True):
        scale = pop(at, "8f") if grow else 1
        return smooth(win({"id": id, "kind": "mesh", "mesh": {"type": "sphere", "radius": r}, "material": {"preset": "coated", "color": RED},
                           "transform": {"position": pos_keys, "scale": scale}}, at, until))
    L_.append(ball("c1", [W / 2, 860], one, two))
    L_.append(ball("c2a", keys((two, [W / 2, 860]), (two + 0.3, [W / 2 - 110, 860]), ease="snap"), two, four, 72, False))
    L_.append(ball("c2b", keys((two, [W / 2, 860]), (two + 0.3, [W / 2 + 110, 860]), ease="snap"), two, four, 72, False))
    for j, (dx, dy) in enumerate([(-110, -110), (110, -110), (-110, 110), (110, 110)]):
        sx = -1 if dx < 0 else 1
        L_.append(ball(f"c4{j}", keys((four, [W / 2 + 110 * sx, 860]), (four + 0.3, [W / 2 + dx * 1.3, 860 + dy * 1.2]), ease="snap"), four, billions + 0.3, 64, False))
    burst_dot = {"id": "bdot", "kind": "shape", "shape": "circle", "size": 26, "fill": RED,
                 "fill_gradient": None,
                 "transform": tr([0, 0], scale=tween(billions, 0, 1, "6f", "pop"))}
    burst_dot.pop("fill_gradient")
    burst = {"id": "burst", "kind": "instancer", "layout": {"type": "random", "count": 900, "size": [1160, 1500], "area": "circle", "seed": 3},
             "palette": [RED, CORAL, "#c1121f", "#ff9f9f", "#9d0208"], "palette_order": "random",
             "stagger": {"total": 0.9, "order": "center"}, "random": {"seed": 5, "scale": [0.5, 1.4]}, "transform": tr([W / 2, 880]), "layers": [burst_dot]}
    L_.append(win(burst, billions - 0.05, None))
    L_.append(win(text("bn2", "BILLIONS", 170, [W / 2, 880], CREAM, BLACK, effects=[{"type": "shadow", "color": "#00000066", "offset": [0, 10], "radius": 18}],
                       transform=tr([W / 2, 880], scale=tween(billions + 0.6, 0.6, 1, "7f", "pop"), opacity=fade_in(billions + 0.6, "2f"))), billions + 0.5, None))
    L_.append(shadow_catcher(sid))
    L_ += captions(sid, end)
    sfx = [("type", typos - 0.6, -12), ("glitch", typos + 0.35, -8), ("click", fail, -4), ("pop", one, -8), ("pop", two, -8), ("pop", four, -8),
           ("riser", billions - 1.2, -10), ("boom", billions, -8)]
    return scene(sid, L_, lights=SOFT_LIGHT, environment={"preset": "studio"}), sfx


def crowd():
    sid = "crowd"
    start, end = scene_window(sid)
    crowd_t = at_word(10, "crowd", -0.3)
    drug = T(11, 0.1)
    kill = at_word(11, "kill", 0.15)
    resist = at_word(12, "resist", -0.2)
    back = at_word(12, "grow back", -0.2)
    L_ = [paper(sid)] + chapter(sid, "4 · A CROWD THAT EVOLVES", start)
    L_.append(moire_rings("drug-moire", [W / 2, 900], 470, INK, drug - 0.1, back - 0.1, 0.16, 14, 60))
    tumor_reds = [RED, CORAL, "#9d0208", "#f4a261", "#b5179e", "#d62828"]
    cellm = {"id": "tc", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"preset": "glossy", "color": "#ffffff"},
             "transform": tr([0, 0], scale=tween(T(10, 0.2), 0, 1, "8f", "pop"))}
    tumor = {"id": "tumor", "kind": "instancer", "layout": {"type": "random", "count": 130, "size": [600, 600], "area": "circle", "seed": 21},
             "palette": tumor_reds, "palette_order": "random", "stagger": {"total": 1.2, "order": "center"}, "random": {"seed": 2, "scale": [0.7, 1.25]},
             "fields": [{"type": "radial", "center": [0, 0], "radius": keys((kill, 1), (kill + 0.9, 520), ease="settle"), "falloff": 60,
                         "scale": -1, "opacity": -1}],
             "transform": tr([W / 2, 900]), "layers": [cellm]}
    L_.append(smooth(win(tumor, start, None)))
    surv = {"id": "sv", "kind": "mesh", "mesh": {"type": "sphere", "radius": 33}, "material": {"preset": "coated", "color": TEAL},
            "transform": tr([0, 0], scale={"keys": [{"at": T(10, 1.0), "from": 0, "v": 1, "ease": "pop", "dur": "8f", "anchor": "start"},
                                                    {"at": back, "v": 1.3, "ease": "land", "dur": "8f", "anchor": "start"}]})}
    survivors = {"id": "survivors", "kind": "instancer", "layout": {"type": "random", "count": 6, "size": [560, 560], "area": "circle", "seed": 77},
                 "stagger": {"each": "2f"}, "transform": tr([W / 2, 900]), "layers": [surv]}
    L_.append(smooth(win(survivors, start, None)))
    regrow_m = {"id": "rg", "kind": "mesh", "mesh": {"type": "sphere", "radius": 28}, "material": {"preset": "coated", "color": TEAL},
                "transform": tr([0, 0], scale=tween(back + 0.1, 0, 1, "7f", "pop"))}
    regrow = {"id": "regrow", "kind": "instancer", "layout": {"type": "random", "count": 90, "size": [620, 620], "area": "circle", "seed": 31},
              "stagger": {"total": 1.3, "order": "random", "seed": 2}, "random": {"seed": 3, "scale": [0.7, 1.15]}, "palette": [TEAL, "#21867a", "#52b69a"],
              "transform": tr([W / 2, 900]), "layers": [regrow_m]}
    L_.append(smooth(win(regrow, back, None)))
    # Under the microscope: the real thing is a crowd too.
    slide_t = T(10, 1.6)
    L_.append(win(card("slide-card", [400, 440], [790, 470], slide_t, 4, "#ffffff"), slide_t, drug))
    L_.append(win({"id": "slide", "kind": "image", "src": "assets/art/slide_tumor.png", "size": [360, 360],
                   "transform": tr([790, 450], rotation=4, opacity=fade_in(slide_t + 0.05, "3f"))}, slide_t, drug))
    L_.append(win(text("slide-l", "under the microscope", 30, [790, 662], "#4a4a50", SERIF_I, transform=tr([790, 662], rotation=4, opacity=fade_in(slide_t + 0.1, "3f"))), slide_t, drug))
    L_.append(win(tape("slide-tape", [790, 250], slide_t + 0.1, -6), slide_t, drug))
    L_.append(win(text("diff", "every cell a little different", 46, [W / 2, 1300], INK, SERIF_I, transform=tr([W / 2, 1300], opacity=fade_in(crowd_t, "3f"))), crowd_t, drug))
    # The drug: a pill drops in, the wave clears most of the tumour.
    for k, (color, dy) in enumerate([("#ffffff", -45), (RED, 45)]):
        L_.append(smooth(win({"id": f"pill{k}", "kind": "mesh", "mesh": {"type": "capsule", "radius": 46, "height": 120 if k == 0 else 120},
                              "material": {"preset": "glossy", "color": color},
                              "transform": tr([W / 2, 900], rotation=35,
                                              position=keys((drug, [W / 2 + 60 + dy * 0.6, 300 + dy]), (kill, [W / 2 + dy * 0.6, 900 + dy]), ease="snap"),
                                              z=-160, scale=keys((kill, 1), (kill + 0.35, 0.0), ease="glide"))}, drug, kill + 0.4)))
    L_.append(win({"id": "wave", "kind": "shape", "shape": "ring", "size": 120, "thickness": 10, "fill": "#2b2d42",
                   "transform": tr([W / 2, 900], scale=tween(kill, 0.5, 9, "16f", "settle"), opacity=tween(kill, 0.8, 0, "16f", "glide"))}, kill, kill + 0.6))
    L_.append(win(text("killed", "MOST DIE", 110, [W / 2, 520], INK, BLACK, transform=tr([W / 2, 520], opacity=fade_in(kill + 0.5, "2f"))), kill + 0.4, back - 0.2))
    # The resistant few.
    L_.append(win(highlight("res-hl", resist, [W / 2, 1302], 560, 74), resist, None))
    L_.append(win(text("res", "RESISTANT", 72, [W / 2, 1300], INK, BLACK, transform=tr([W / 2, 1300], opacity=fade_in(resist + 0.05, "2f"))), resist, None))
    L_.append(win(text("back", "...and they grow back", 52, [W / 2, 520], TEAL, SERIF_BI, transform=tr([W / 2, 520], opacity=fade_in(back + 0.2, "3f"))), back, None))
    L_.append(shadow_catcher(sid, opacity=0.22))
    L_ += captions(sid, end)
    sfx = [("sparkle", T(10, 0.2), -12), ("whoosh", drug, -6), ("boom", kill, -8), ("ding", resist, -10), ("pop", back + 0.1, -10), ("pop", back + 0.5, -12)]
    return scene(sid, L_, lights=SOFT_LIGHT, environment={"preset": "studio"}), sfx


def spread():
    sid = "spread"
    start, end = scene_window(sid)
    move = at_word(13, "moves", -0.2)
    blood = at_word(13, "blood", -0.4)
    deaths = at_word(14, "deaths", -0.2)
    L_ = [paper(sid)] + chapter(sid, "5 · IT SPREADS", start)
    cx = W / 2
    ink = {"stroke": {"color": INK, "width": 9}}
    body = [
        {"id": "head", "kind": "shape", "shape": "circle", "size": 170, **ink, "transform": tr([cx, 430])},
        {"id": "torso", "kind": "shape", "shape": "rect", "size": [330, 520], "radius": 120, **ink, "transform": tr([cx, 820])},
        {"id": "armL", "kind": "shape", "shape": "capsule", "size": [80, 440], **ink, "transform": tr([cx - 245, 820], rotation=8)},
        {"id": "armR", "kind": "shape", "shape": "capsule", "size": [80, 440], **ink, "transform": tr([cx + 245, 820], rotation=-8)},
        {"id": "legL", "kind": "shape", "shape": "capsule", "size": [100, 380], **ink, "transform": tr([cx - 85, 1255])},
        {"id": "legR", "kind": "shape", "shape": "capsule", "size": [100, 380], **ink, "transform": tr([cx + 85, 1255])},
        {"id": "brain", "kind": "shape", "shape": "circle", "size": [110, 90], "fill": "#f3c4cf", "transform": tr([cx, 415])},
        {"id": "lungL", "kind": "shape", "shape": "capsule", "size": [95, 200], "fill": "#f3c4cf", "transform": tr([cx - 70, 700])},
        {"id": "lungR", "kind": "shape", "shape": "capsule", "size": [95, 200], "fill": "#f3c4cf", "transform": tr([cx + 70, 700])},
        {"id": "liver", "kind": "shape", "shape": "circle", "size": [190, 110], "fill": "#d9a38b", "transform": tr([cx + 40, 900])},
        {"id": "bone", "kind": "shape", "shape": "capsule", "size": [34, 240], "fill": "#e9e1cf", "transform": tr([cx - 85, 1265])},
    ]
    for k, l in enumerate(body):
        l["transform"]["opacity"] = fade_in(start + 0.1 + k * 0.04, "3f")
        L_.append(l)
    # The first tumour, in a lung, sending out moiré ripples.
    L_.insert(1, moire_rings("spread-moire", [cx - 80, 690], 330, RED, T(13, 0.0), end, 0.3, 12, 30))
    L_.append({"id": "primary", "kind": "shape", "shape": "circle", "size": 46, "fill": RED, "transform": tr([cx - 80, 690], scale=pop(T(13, 0.1), "8f"))})
    routes = [
        ("toLiver", f"M {cx - 80} 690 C {cx - 160} 780 {cx - 30} 880 {cx + 40} 900", move + 0.2),
        ("toBrain", f"M {cx - 80} 690 C {cx - 150} 600 {cx - 40} 480 {cx} 420", move + 0.5),
        ("toBone", f"M {cx - 80} 690 C {cx - 160} 900 {cx - 120} 1120 {cx - 85} 1265", blood - 0.2),
    ]
    for k, (rid, d, t0) in enumerate(routes):
        L_.append({"id": rid, "kind": "shape", "shape": "path", "d": d, "stroke": {"color": "#c1121f", "width": 7, "dash": [14, 10]},
                   "trim": tween(t0, [0, 0], [0, 1], "12f", "glide")})
        for j in range(3):
            ta = t0 + 0.2 + j * 0.25
            L_.append(win({"id": f"{rid}-c{j}", "kind": "shape", "shape": "circle", "size": 26, "fill": RED,
                           "follow": {"layer": rid, "at": tween(ta, 0, 1, "24f", "glide")}}, ta, ta + 1.0))
        dest = {"toLiver": [cx + 40, 900], "toBrain": [cx, 420], "toBone": [cx - 85, 1265]}[rid]
        ta = t0 + 0.2 + 0.8
        L_.append(win({"id": f"{rid}-met", "kind": "shape", "shape": "circle", "size": 40, "fill": RED, "transform": tr(dest, scale=pop(ta, "8f"))}, ta, None))
        L_.append(win({"id": f"{rid}-ring", "kind": "shape", "shape": "ring", "size": 60, "thickness": 6, "fill": RED,
                       "transform": tr(dest, scale=tween(ta, 0.6, 3.5, "14f", "settle"), opacity=tween(ta, 1, 0, "14f", "glide"))}, ta, ta + 0.6))
    L_.append(win(card("deadly-card", [760, 150], [W / 2, 1190], deaths, -2, "#ffffff"), deaths, None))
    L_.append(win(text("deadly", "The deadliest step", 66, [W / 2, 1190], INK, SERIF_BI, transform=tr([W / 2, 1190], rotation=-2, opacity=fade_in(deaths + 0.1, "3f"))), deaths, None))
    L_.append(win(highlight("deadly-hl", deaths + 0.3, [W / 2 + 120, 1198], 300, 56), deaths + 0.2, None))
    L_ += captions(sid, end)
    sfx = [("pop", T(13, 0.1), -8)] + [("whoosh", t0 + 0.2, -14) for _, _, t0 in routes] + [("pop", t0 + 1.0, -10) for _, _, t0 in routes] + [("thud", deaths, -6)]
    return scene(sid, L_), sfx


def hope():
    sid = "hope"
    start, end = scene_window(sid)
    losing = at_word(15, "losing", -0.3)
    s49 = at_word(16, "forty-nine", -0.2)
    s69 = at_word(16, "sixty-nine", -0.2)
    L_ = [paper(sid)] + chapter(sid, "BUT", start)
    # A line that keeps climbing.
    L_.append(win({"id": "trend", "kind": "shape", "shape": "path", "d": "M 140 1050 C 340 1000 420 900 560 860 S 820 640 940 560",
                   "stroke": {"color": TEAL, "width": 14, "cap": "round"}, "trim": tween(T(15, 0.1), [0, 0], [0, 1], "20f", "glide")}, start, T(16, -0.2)))
    L_.append(win(text("notlosing", "not a war we're losing", 64, [W / 2, 460], INK, SERIF_BI, transform=tr([W / 2, 460], opacity=fade_in(losing, "3f"))), start, T(16, -0.2)))
    # Survival: 49% then 69%, over a soft moiré.
    base = 1240
    L_.append(moire_lines("chart-moire", [W / 2, 900], [900, 820], TEAL, T(16, -0.2), end, 0.16, 14, 10))
    L_.append(win(text("chart-t", "5-YEAR SURVIVAL, ALL CANCERS (US)", 34, [W / 2, 300], INK, SEMI, tracking=60, transform=tr([W / 2, 300], opacity=fade_in(T(16), "3f"))), T(16, -0.1), None))
    L_.append(win({"id": "axis", "kind": "shape", "shape": "line", "size": [820, 0], "thickness": 6, "fill": INK, "trim": tween(T(16), [0, 0], [0, 1], "8f", "glide"), "transform": tr([W / 2, base])}, T(16, -0.1), None))
    for k, (x, pct, label, color, t) in enumerate([(330, 49, "1975–77", "#9a958c", s49), (750, 69, "2013–19", TEAL, s69)]):
        h = pct * 9.5
        L_.append(win({"id": f"bar{k}", "kind": "shape", "shape": "rect", "size": [260, h], "fill": color,
                       "transform": {"position": [x, base], "anchor": [0, h / 2], "scale": tween(t - 0.2, [1, 0], [1, 1], "12f", "land")}}, t - 0.25, None))
        L_.append(win(text(f"pct{k}", f"{pct}%", 110, [x, base - h - 80], INK, BLACK, transform=tr([x, base - h - 80], scale=tween(t + 0.2, 0.6, 1, "6f", "pop"), opacity=fade_in(t + 0.2, "2f"))), t, None))
        L_.append(win(text(f"yr{k}", label, 44, [x, base + 50], INK, SERIF_I, transform=tr([x, base + 50], opacity=fade_in(t - 0.1, "3f"))), t - 0.2, None))
    L_.append(win(highlight("pct-hl", s69 + 0.45, [750, base - 69 * 9.5 - 80], 260, 100), s69 + 0.4, None))
    L_.append(win(text("src", "Source: American Cancer Society, Cancer Statistics 2024", 26, [W / 2, base + 120], "#6a6a70", SERIF_I, transform=tr([W / 2, base + 120], opacity=fade_in(s49, "4f"))), s49, None))
    L_ += captions(sid, end)
    sfx = [("riser", T(15, -0.3), -14), ("pop", s49, -8), ("pop", s69, -6), ("sparkle", s69 + 0.4, -10)]
    return scene(sid, L_), sfx


def finale():
    sid = "end"
    start, end = scene_window(sid)
    one = T(17, 0.0)
    many_t = at_word(17, "many", -0.25)
    L_ = [paper(sid)]
    L_.append(win(text("onecure", "ONE CURE", 150, [W / 2, 520], INK, BLACK, transform=tr([W / 2, 520], opacity=fade_in(one, "2f"))), one, None))
    L_.append(win({"id": "strike", "kind": "shape", "shape": "line", "size": [760, 0], "thickness": 18, "fill": RED,
                   "trim": tween(at_word(17, "won't", 0.1), [0, 0], [0, 1], "5f", "glide"), "transform": tr([W / 2, 525], rotation=-6)}, one, None))
    L_.append(moire_rings("end-moire", [W / 2, 1020], 560, "#c9952e", many_t - 0.2, end, 0.3, 16, 50))
    tiles = [("targeted\ndrugs", "target"), ("immuno-\ntherapy", "shield"), ("early\ndetection", "lens"), ("surgery &\nradiation", "plus")]
    for k, (label, icon) in enumerate(tiles):
        x, y = 320 + (k % 2) * 440, 860 + (k // 2) * 330
        t = many_t + k * 0.12
        L_.append(win(card(f"tile{k}", [390, 280], [x, y], t, (-3, 2, 2, -2)[k], "#ffffff"), t, None))
        ic = [x, y - 50]
        if icon == "target":
            for j, s in enumerate([110, 70, 30]):
                L_.append(win({"id": f"ic{k}{j}", "kind": "shape", "shape": "ring" if j < 2 else "circle", "size": s, "thickness": 10, "fill": RED, "transform": tr(ic, scale=pop(t + 0.05, "8f"))}, t, None))
        elif icon == "shield":
            L_.append(win({"id": f"ic{k}", "kind": "shape", "shape": "path", "d": "M 0 -55 L 48 -38 L 44 18 C 38 42 18 54 0 62 C -18 54 -38 42 -44 18 L -48 -38 Z", "fill": TEAL, "transform": tr(ic, scale=pop(t + 0.05, "8f"))}, t, None))
        elif icon == "lens":
            L_.append(win({"id": f"ic{k}a", "kind": "shape", "shape": "ring", "size": 90, "thickness": 12, "fill": BLUE, "transform": tr([ic[0] - 12, ic[1] - 12], scale=pop(t + 0.05, "8f"))}, t, None))
            L_.append(win({"id": f"ic{k}b", "kind": "shape", "shape": "line", "size": [55, 0], "thickness": 14, "fill": BLUE, "transform": tr([ic[0] + 38, ic[1] + 38], rotation=45, scale=pop(t + 0.05, "8f"))}, t, None))
        else:
            L_.append(win({"id": f"ic{k}a", "kind": "shape", "shape": "rect", "size": [100, 32], "radius": 6, "fill": PURPLE, "transform": tr(ic, scale=pop(t + 0.05, "8f"))}, t, None))
            L_.append(win({"id": f"ic{k}b", "kind": "shape", "shape": "rect", "size": [32, 100], "radius": 6, "fill": PURPLE, "transform": tr(ic, scale=pop(t + 0.05, "8f"))}, t, None))
        L_.append(win(text(f"tile{k}tx", label, 40, [x, y + 70], INK, SERIF_B, line_height=1.0, transform=tr([x, y + 70], opacity=fade_in(t + 0.1, "3f"))), t, None))
    L_.append(win(text("many-word", "MANY", 150, [W / 2, 1395], INK, BLACK, transform=tr([W / 2, 1395], scale=tween(many_t + 0.6, 0.5, 1, "8f", "pop"), opacity=fade_in(many_t + 0.6, "2f"))), many_t + 0.55, None))
    L_.append(win(highlight("many-hl", many_t + 0.8, [W / 2, 1402], 500, 120), many_t + 0.75, None))
    title_t = E(17, 0.6)
    L_.append(win(text("title", "WHY CANCER IS\nSO HARD TO CURE", 62, [W / 2, 1620], INK, BLACK, line_height=1.05, transform=tr([W / 2, 1620], opacity=fade_in(title_t, "4f"))), title_t, None))
    L_.append(win(text("credit", "Sources: American Cancer Society; Cancer Research UK\nMade with the Lahza motion engine", 24, [W / 2, 1745], "#6a6a70", SERIF_I, transform=tr([W / 2, 1745], opacity=fade_in(title_t + 0.3, "4f"))), title_t, None))
    caps = captions(sid, title_t - 0.1)
    L_ += caps
    sfx = [("swipe", at_word(17, "won't", 0.1), -8)] + [("pop", many_t + k * 0.12, -10) for k in range(4)] + [("boom", many_t + 0.6, -10), ("ding", title_t, -12)]
    return scene(sid, L_), sfx


SCENES = [hook, many, self_, typo, crowd, spread, hope, finale]
SHEET = [0.5, 3.0, 6.5, 9.2, 11.5, 14.6, 17.5, 21.5, 25.5, 29.5, 33.2, 38.0, 41.2, 43.6, 47.5, 50.5, 53.5, 60.6, 63.5, 66.5]


def main():
    print(f"building into {OUT}")
    mcp = Mcp(OUT)
    mcp.tool("project_create", path="cancer", width=W, height=H, fps=30, duration=VO["duration"], style="explainer.evidence", background=PAPER,
             music="music.wav", licence="original, generated for this video (music.py)", overwrite=True)
    proj = os.path.join(OUT, "cancer.lahza")
    for name, src in FONT_FILES.items():
        os.makedirs(os.path.join(proj, "assets/fonts"), exist_ok=True)
        shutil.copy(src, os.path.join(proj, "assets/fonts", name))
    shutil.copytree(os.path.join(OUT, "art"), os.path.join(proj, "assets/art"), dirs_exist_ok=True)
    shutil.copy(os.path.join(OUT, "vo.wav"), os.path.join(proj, "assets/vo.wav"))
    for art in sorted(os.listdir(os.path.join(proj, "assets/art"))):
        mcp.tool("media_import", path=os.path.join(proj, "assets/art", art), copy=False, source="generated by assets.py", licence="original, made for this video")
    ops = [{"op": "add_sound", "kind": "vo", "path": "assets/vo.wav", "at": 0},
           {"op": "set", "target": "music.gain_db", "value": -9},
           {"op": "set", "target": "music.duck_db", "value": -6}]
    result, _ = mcp.tool("edit_apply", ops=ops)
    if result["errors"]:
        print("   errors:", result["errors"][:5])
    mcp.tool("markers_import", kind="user", name="line", markers=[{"t": l["start"], "label": l["text"]} for l in L])
    all_sfx = []
    for build in SCENES:
        op, sfx = build()
        result, _ = mcp.tool("edit_apply", ops=[op])
        if result["errors"]:
            print(f"   {op['id']} errors:", result["errors"][:8])
        all_sfx += sfx
    for sound, t, gain in all_sfx:
        mcp.tool("sfx_add", sound=sound, at=round(max(0.0, t), 3), gain_db=gain)
    report, _ = mcp.tool("check", verbose=False)
    print(f"  check: ok={report['ok']} errors={report['errors']} warnings={report['warnings']}")
    for issue in report["issues"][:20]:
        print(f"    {issue['severity']:<8} {issue.get('element', '')}: {issue['message']}")
    _, images = mcp.tool("snapshot", sheet=True, width=270, at=SHEET)
    with open(os.path.join(OUT, "contact-sheet.png"), "wb") as f:
        f.write(base64.b64decode(images[0]["data"]))
    if os.environ.get("SKIP_RENDER"):
        return
    job, _ = mcp.tool("render", output=os.path.join(OUT, "why-cancer-is-hard-to-cure.mp4"), quality=os.environ.get("QUALITY", "final"), wait=True)
    print(json.dumps(job.get("report", job), indent=2)[:1500])


if __name__ == "__main__":
    main()
