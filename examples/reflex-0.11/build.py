"""Builds the Reflex 0.11 release reel (reflex-0.11.lahza) from code.

Design system
  Palette   ink #0c0c10, paper #fcfcfd, Reflex violet #6e56cf with its light
            (#baa7ff) and pale (#ebe4ff) steps, one warning red #e5484d.
  Type      Inter Display (Black/Bold for headlines, Medium/Light for copy),
            JetBrains Mono for code, which is always real Reflex code.
  Subject   the block: the Reflex mark is five blocks on a 48 px grid. One
            violet block opens the film, the mark assembles from blocks, and
            every feature shot is built from the same blocks.
  Motion    snap/glide moves, slam on hits, springs on scale only; 120 BPM
            (beat 0.5 s, bar 2 s), cuts on the score's impacts.

Run music.py first (writes track.wav and track-markers.json here), then this.
`--slang` adds the Slang shaders (needs slangc on PATH or LAHZA_SLANGC).
"""
import json
import os
import shutil
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from PIL import ImageFont

HERE = os.getcwd()
SRC = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "reflex-0.11.lahza")
INTER = "/usr/share/fonts/opentype/inter/"
MONO = "/usr/share/fonts/truetype/jetbrains-mono/"
SYSTEM_FONTS = {
    "InterDisplay-Black.otf": INTER, "InterDisplay-Bold.otf": INTER, "InterDisplay-Medium.otf": INTER,
    "InterDisplay-Light.otf": INTER, "JetBrainsMono-Medium.ttf": MONO, "JetBrainsMono-Bold.ttf": MONO,
}
W, H, FPS = 1920, 1080, 30
DURATION = 47.0
M = 110
SLANG = False

INK = "#0c0c10"
INK2 = "#17161f"
PAPER = "#fcfcfd"
VIOLET = "#6e56cf"
VIOLET_D = "#3f2f8c"
LILAC = "#baa7ff"
PALE = "#ebe4ff"
GREY = "#8b8a94"
RED = "#e5484d"
GREEN = "#46a758"

F_BLACK = "assets/fonts/InterDisplay-Black.otf"
F_BOLD = "assets/fonts/InterDisplay-Bold.otf"
F_MED = "assets/fonts/InterDisplay-Medium.otf"
F_LIGHT = "assets/fonts/InterDisplay-Light.otf"
F_MONO = "assets/fonts/JetBrainsMono-Medium.ttf"
F_MONOB = "assets/fonts/JetBrainsMono-Bold.ttf"

BEAT = 0.5


def f(frames):
    return frames / FPS


_fonts = {}


def text_width(s, font, size, tracking=0.0):
    key = (font, size)
    if key not in _fonts:
        name = os.path.basename(font)
        _fonts[key] = ImageFont.truetype(SYSTEM_FONTS[name] + name, size)
    return max(_fonts[key].getlength(l) + tracking / 1000 * size * max(0, len(l) - 1) for l in s.split("\n"))


def glyph_path(ch, font, size):
    """SVG outline of one glyph, y down, centred on its box; returns (d, advance px, width, height)."""
    name = os.path.basename(font)
    tt = TTFont(SYSTEM_FONTS[name] + name)
    gs = tt.getGlyphSet()
    g = gs[tt.getBestCmap()[ord(ch)]]
    from fontTools.pens.boundsPen import BoundsPen
    bp = BoundsPen(gs)
    g.draw(bp)
    x0, y0, x1, y1 = bp.bounds
    s = size / tt["head"].unitsPerEm
    pen = SVGPathPen(gs, ntos=lambda v: f"{v:.2f}")
    g.draw(TransformPen(pen, (s, 0, 0, -s, -(x0 + x1) / 2 * s, (y0 + y1) / 2 * s)))
    return pen.getCommands(), g.width * s, (x1 - x0) * s, (y1 - y0) * s


def k(at, v, **kw):
    d = {"at": at, "v": v}
    for a, b in kw.items():
        d["from" if a == "frm" else a] = b
    return d


def keys(*ks, add=None):
    d = {"keys": list(ks)}
    if add:
        d["add"] = add
    return d


def enter(at, v, frm, ease="glide", dur="12f"):
    return keys(k(at, v, frm=frm, ease=ease, dur=dur, anchor="start"))


def fade_in(at, dur="8f", to=1):
    return enter(at, to, 0, "glide", dur)


def window(start, end=None):
    d = {"start": start}
    if end is not None:
        d["end"] = end
    return d


def text(id, s, x, y, size, font=F_MONO, color=PAPER, align="center", tracking=0, **kw):
    layer = {"id": id, "kind": "text", "text": s, "font": font, "size": size, "color": color,
             "transform": {"position": [round(x, 1), round(y, 1)]}}
    if tracking:
        layer["tracking"] = tracking
    if align != "center":
        layer["align"] = align
    for key, v in kw.items():
        if key in ("opacity", "scale", "rotation", "anchor", "z", "rotation_x", "rotation_y"):
            layer["transform"][key] = v
        else:
            layer[key] = v
    return layer


def typed(at, per="1f", dur="2f"):
    """Glyphs appear in order, like typing."""
    return [{"at": at, "unit": "glyph", "order": "forward", "stagger": per, "dur": dur, "from": {"opacity": 0}}]


def rise(at, stagger="1f", dur="12f", y=40, unit="glyph", order="forward"):
    return [{"at": at, "unit": unit, "order": order, "stagger": stagger, "dur": dur, "ease": "glide", "from": {"opacity": 0, "y": y}}]


def box(id, x, y, w, h, depth, color, bevel=6, preset="glossy", z=0, **kw):
    layer = {"id": id, "kind": "mesh", "mesh": {"type": "box", "size": [w, h], "depth": depth, "bevel": bevel},
             "material": {"preset": preset, "color": color}, "transform": {"position": [x, y], "z": z}}
    for key, v in kw.items():
        if key in ("opacity", "scale", "rotation", "rotation_x", "rotation_y"):
            layer["transform"][key] = v
        elif key == "material":
            layer["material"].update(v)
        else:
            layer[key] = v
    return layer


def floor(id, y, color=None, **mat):
    """A ground plane: a shadow catcher over a 2D background, or a lit surface with `color`."""
    m = {"preset": "shadow", "color": "#000000", "opacity": 0.5} if color is None else {"preset": "matte", "color": color}
    m.update(mat)
    return {"id": id, "kind": "mesh", "mesh": {"type": "plane", "size": [24000, 24000]}, "material": m, "role": "background",
            "transform": {"position": [W / 2, y], "rotation_x": 90}}


# The Reflex mark: five blocks on a 48 px grid (from the favicon's outline).
MARK = [  # (id, x, y, w, h) in grid px, origin at the mark's top left
    ("stem", 0, 0, 48, 224), ("bar", 48, 0, 96, 48), ("bowl", 144, 0, 48, 96), ("mid", 48, 96, 96, 48), ("leg", 144, 144, 48, 80),
]
MARK_W, MARK_H = 192, 224


def mark_blocks(prefix, cx, cy, unit, depth, color, preset="glossy", z=0, **mat):
    """The mark's five blocks as meshes centred on (cx, cy); `unit` is px per 48."""
    s = unit / 48
    out = []
    for name, x, y, w, h in MARK:
        px = cx + (x + w / 2 - MARK_W / 2) * s
        py = cy + (y + h / 2 - MARK_H / 2) * s
        out.append(box(f"{prefix}-{name}", px, py, w * s, h * s, depth, color, bevel=max(2, unit * 0.07), preset=preset, z=z, material=mat))
    return out


# ------------------------------------------------------------------ scenes

def s01():
    """OPEN: one violet block turns on the ticks while `import reflex as rx` types; the block swallows the frame."""
    sid, start, end = "s01", 0.0, 4.0
    cx, ground = W / 2, 700
    L = []
    L.append({"id": "o-bg", "kind": "shape", "shape": "field", "size": [W + 40, H + 40], "transform": {"position": [W / 2, H / 2]},
              "space": "screen", "softness": 1.3, "role": "background",
              "points": [{"position": [0, 120], "color": "#231c45", "radius": 520},
                         {"position": [-900, -500], "color": "#09090c", "radius": 500}, {"position": [900, -500], "color": "#09090c", "radius": 500},
                         {"position": [-900, 520], "color": "#0b0b10", "radius": 420}, {"position": [900, 520], "color": "#0b0b10", "radius": 420}]})
    L.append(floor("o-floor", ground, opacity=0.8))
    ticks = [0.0, 0.5, 1.0, 1.5] + [2.0 + i * 0.25 for i in range(8)]
    rot = [k(0.0, 35)]
    for i, t in enumerate(ticks[1:]):
        rot.append(k(t, 35 + 45 * (i + 1), ease="snap", dur="6f", anchor="start"))
    rot.append(k(4.0, 35 + 45 * 12 + 10, ease="glide", dur="10f", anchor="end"))  # ends face-on (585 = 225 + 360)
    sc = [k(0.0, 1)]
    for t in ticks[1:]:
        sc.append(k(t, 1.07, ease="snap", dur="2f", anchor="start"))
        sc.append(k(t + f(2), 1.0, ease="pop", anchor="start"))
    L.append(box("o-block", cx, ground - 130, 260, 260, 260, VIOLET, bevel=18, preset="coated", role="subject",
                 material={"roughness": 0.22}, rotation_y=keys(*rot), scale=keys(*sc)))
    L.append(text("o-code", "import reflex as rx", cx, 910, 76, F_MONO, PAPER, space="screen", animators=typed(0.5, "2.2f"),
                  opacity=keys(k(3.3, 1), k(3.6, 0, ease="glide"))))
    camera = {"fov": 32, "orbit": keys(k(0.0, [-10, 14]), k(3.4, [0, 10], ease="linear"), k(4.0, [0, 0], ease="glide")),
              "dolly": keys(k(0.0, -160), k(3.3, 0, ease="linear"), k(4.0, 1800, ease="zoom", dur="14f", anchor="end"))}
    lights = [{"type": "area", "position": [cx - 700, 60, -700], "aim": [cx, ground - 130, 0], "size": [700, 500], "color": "#fff3e8", "intensity": 1.5, "softness": 0.6},
              {"type": "directional", "from": [1.0, -0.3, 1.0], "color": "#9f8cff", "intensity": 0.9},
              {"type": "ambient", "intensity": 0.08}]
    return {"id": sid, "start": start, "end": end, "background": INK, "layers": L, "camera": camera, "lights": lights,
            "environment": {"preset": "studio", "intensity": 0.45}}


def s02():
    """MARK: five white blocks slam into the Reflex mark on the impact; the name and the promise."""
    sid, start, end = "s02", 4.0, 8.0
    L = []
    unit = 84
    name_size = 300
    name_w = text_width("reflex", F_BOLD, name_size, -20)
    gap = 90
    mw, mh = MARK_W * unit / 48, MARK_H * unit / 48
    x0 = (W - (mw + gap + name_w)) / 2
    mx, cy = x0 + mw / 2, 470
    L.append({"id": "m-wall", "kind": "mesh", "mesh": {"type": "plane", "size": [9000, 9000]},
              "material": {"preset": "shadow", "color": "#1d1247", "opacity": 0.6}, "transform": {"position": [W / 2, H / 2], "z": 46}})
    fly = {"stem": (-1500, 0, -900), "bar": (200, -1100, -700), "bowl": (1700, -500, -1200), "mid": (900, 900, -600), "leg": (1500, 700, -1000)}
    for i, b in enumerate(mark_blocks("m", mx, cy, unit, 84, PAPER, "glossy", roughness=0.28)):
        dx, dy, dz = fly[b["id"][2:]]
        p = b["transform"]["position"]
        t = 4.0 - f(3) + f(i) / 2
        b["role"] = "subject"
        b["transform"]["position"] = keys(k(t, p, frm=[p[0] + dx * 0.6, p[1] + dy * 0.6], ease="slam", dur="6f", anchor="start"))
        b["transform"]["z"] = keys(k(t, 0, frm=dz, ease="slam", dur="7f", anchor="start"))
        b["transform"]["rotation_y"] = keys(k(t, 0, frm=70 if dx > 0 else -70, ease="slam", dur="7f", anchor="start"))
        L.append(b)
    L.append(text("m-name", "reflex", x0 + mw + gap + name_w / 2, cy - 18, name_size, F_BOLD, PAPER, tracking=-20, three_d=True, z=42,
                  animators=[{"at": 4.0 + f(5), "unit": "glyph", "order": "forward", "stagger": "1f", "dur": "8f", "ease": "glide",
                              "from": {"opacity": 0, "x": 90}}]))
    L.append(text("m-line", "Web apps in pure Python.", W / 2, cy + 300, 78, F_MED, PAPER, tracking=-8, three_d=True, z=42,
                  animators=[{"at": 5.0, "unit": "word", "order": "forward", "stagger": "2f", "dur": "12f", "ease": "glide",
                              "from": {"opacity": 0, "y": 44}}]))
    camera = {"fov": 30, "orbit": keys(k(4.0, [-9, 3]), k(8.0, [7, 5], ease="linear")),
              "dolly": keys(k(4.0, 160, frm=-500, ease="settle", dur="20f", anchor="start"), k(7.45, 220, ease="linear"),
                            k(8.0, -700, ease="zoom", dur="12f", anchor="end"))}
    lights = [{"type": "directional", "from": [-0.7, -1.0, -1.0], "color": "#ffffff", "intensity": 1.5, "shadow": True, "softness": 0.6},
              {"type": "directional", "from": [1.0, 0.2, -0.4], "color": "#d9d0ff", "intensity": 0.5},
              {"type": "ambient", "intensity": 0.25}]
    return {"id": sid, "start": start, "end": end, "background": VIOLET, "layers": L, "camera": camera, "lights": lights,
            "environment": {"preset": "soft", "intensity": 0.9}}


def s04():
    """0.11: the version as chrome-edged violet numerals on a dark stage, ringed, with an orbiting camera."""
    sid, start, end = "s04", 16.0, 20.0
    cx, ground = W / 2, 760
    L = []
    if SLANG:
        L.append({"id": "v-bg", "kind": "shader", "function": "aurora", "size": [W, H], "space": "screen", "role": "background",
                  "transform": {"position": [W / 2, H / 2]}, "params": {"tint": VIOLET, "energy": keys(k(16.0, 1.0, frm=2.4, ease="glide", dur="30f", anchor="start"))}})
    else:
        L.append({"id": "v-bg", "kind": "shape", "shape": "field", "size": [W + 40, H + 40], "transform": {"position": [W / 2, H / 2]},
                  "space": "screen", "softness": 1.3, "role": "background",
                  "points": [{"position": {"base": [0, 200], "add": [{"type": "wiggle", "hz": 0.25, "amp": [160, 40]}]}, "color": "#2b2060", "radius": 560},
                             {"position": [-900, -500], "color": "#08080b", "radius": 520}, {"position": [900, -500], "color": "#08080b", "radius": 520},
                             {"position": [-960, 540], "color": "#0b0a12", "radius": 420}, {"position": [960, 540], "color": "#0b0a12", "radius": 420}]})
    L.append(floor("v-floor", ground, color="#07060d", preset="coated", roughness=0.3))
    size = 560
    glyphs = [glyph_path(c, F_BLACK, size) for c in "0.11"]
    track = -18
    total = sum(g[1] for g in glyphs) + track * 3
    x = cx - total / 2
    cap = max(g[3] for g in glyphs)
    mat = {"preset": "coated", "color": VIOLET, "roughness": 0.22, "clearcoat": 1.0}
    if SLANG:
        mat["slang"] = {"function": "circuit", "params": [46, keys(k(16.0, 0.0), k(16.6, 1.0, ease="glide"))]}
    else:
        mat["noise"] = {"type": "fbm", "scale": 240, "color": "#9a84ff", "amount": 0.35}
    for i, (d, adv, gw, gh) in enumerate(glyphs):
        gx = x + adv / 2
        gy = ground - gh / 2 - 6  # glyph boxes are centred; sit them on the floor
        t = 16.0 + f(2 * i)
        L.append({"id": f"v-g{i}", "kind": "mesh", "role": "subject", "mesh": {"type": "extrude", "d": d, "depth": 150, "bevel": 9},
                  "material": dict(mat), "parts": {"side": {"preset": "chrome", "color": "#cfc6ff", "roughness": 0.12},
                                                    "bevel": {"preset": "chrome", "color": "#ffffff", "roughness": 0.05}},
                  "transform": {"position": keys(k(t, [gx, gy], frm=[gx, gy - 1500], ease="bounce", anchor="land", dur="16f")),
                                "rotation_y": keys(k(t, 0, frm=-50 + 30 * i, ease="settle", dur="20f", anchor="start"))}})
        x += adv + track
    spin = lambda a, b: keys(k(16.0, a), k(20.0, b, ease="linear"))
    L.append({"id": "v-ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 820, "tube": 9}, "segments": 44, "gap": 3,
              "material": {"preset": "chrome"}, "role": "decoration",
              "transform": {"position": [cx, ground - cap / 2], "rotation_x": 80, "rotation_y": -6, "rotation": spin(0, 60),
                            "scale": enter(16.0, 1, 2.2, "settle", "26f")}})
    L.append({"id": "v-ring2", "kind": "mesh", "mesh": {"type": "torus", "radius": 930, "tube": 3.5}, "role": "decoration",
              "material": {"preset": "emissive", "color": LILAC, "emissive": LILAC, "emissive_strength": 2.2},
              "transform": {"position": [cx, ground - cap / 2], "rotation_x": 74, "rotation_y": 9, "rotation": spin(30, -30),
                            "scale": enter(16.0 + f(3), 1, 2.6, "settle", "28f")}})
    L.append(text("v-name", "Reflex 0.11 is here", cx, 120, 64, F_MED, PAPER, tracking=-6, space="screen", role="foreground",
                  animators=rise(17.0, "1.5f", "12f", 30, "word")))
    camera = {"fov": 34, "orbit": keys(k(16.0, [-26, 6]), k(20.0, [22, 11], ease="linear")),
              "dolly": keys(k(16.0, -240), k(19.4, 60, ease="linear"), k(20.0, 900, ease="zoom", dur="14f", anchor="end")),
              "aperture": 0.7, "focus": "v-g2"}
    lights = [{"type": "area", "position": [cx - 500, -500, -1500], "aim": [cx, ground - 250, 0], "size": [900, 500], "color": "#fff4ea", "intensity": 1.9, "softness": 0.55},
              {"type": "directional", "from": [1.0, -0.35, 1.0], "color": "#8f78ff", "intensity": 1.3},
              {"type": "point", "position": [cx + 700, ground - 600, -500], "color": "#ffd9ff", "intensity": 0.5},
              {"type": "ambient", "intensity": 0.06}]
    return {"id": sid, "start": start, "end": end, "background": "#09090c", "layers": L, "camera": camera, "lights": lights,
            "environment": {"preset": "night", "intensity": 0.5},
            "look": {"tone": "filmic", "bloom": {"threshold": 0.9, "radius": 26, "intensity": 0.6}}}


def card(id, x, y, w, h, fill, layers, radius=28, **tr):
    """A 2.5D card: a rounded panel with its content, one comp."""
    panel = {"id": f"{id}-panel", "kind": "shape", "shape": "rect", "size": [w, h], "radius": radius, "fill": fill,
             "effects": [{"type": "shadow", "color": "#1b114540", "offset": [0, 26], "radius": 34}]}
    t = {"position": [x, y]}
    role = tr.pop("role", None)
    t.update(tr)
    out = {"id": id, "kind": "comp", "three_d": True, "transform": t, "layers": [panel] + layers}
    if role:
        out["role"] = role
    return out


CODE = ["class State(rx.State):", "    count: int = 0", "", "    @rx.event", "    def increment(self):", "        self.count += 1"]


def s03():
    """IDEA: state is a Python class (typed on the beat); the UI card follows it, counting with every click."""
    sid, start, end = "s03", 8.0, 16.0
    L = []
    L.append({"id": "i-bg", "kind": "shape", "shape": "field", "size": [W + 40, H + 40], "transform": {"position": [W / 2, H / 2]},
              "space": "screen", "softness": 1.5, "role": "background",
              "points": [{"position": [-700, -300], "color": "#ffffff", "radius": 600}, {"position": [800, 400], "color": "#e6defc", "radius": 620},
                         {"position": [-800, 500], "color": "#f1ecff", "radius": 500}, {"position": [800, -500], "color": "#f7f5ff", "radius": 500}]})
    # Headlines.
    L.append(text("i-h1", "State is a Python class.", W / 2, 132, 92, F_BOLD, INK, tracking=-14, time=window(8.0, 12.0), role="foreground",
                  animators=rise(8.0, "2f", "12f", 50, "word"),
                  opacity=keys(k(8.0, 1), k(11.8, 0, ease="gesture", dur="6f", anchor="start"))))
    L.append(text("i-h2", "The UI follows it.", W / 2, 132, 92, F_BOLD, INK, tracking=-14, time=window(12.0), role="foreground",
                  animators=rise(12.0, "2f", "12f", 50, "word")))
    # The code card.
    cw, ch, ls, fs = 860, 520, 66, 40
    lines = []
    for i, line in enumerate(CODE):
        if not line:
            continue
        lines.append(text(f"i-code{i}", line, -cw / 2 + 64, -ch / 2 + 92 + i * ls, fs, F_MONO, "#e9e6f5", "left", role="decoration",
                          animators=typed(8.0 + 0.5 * min(i, 5) + (0.25 if i == 5 else 0.0), "0.7f")))
    # The line that runs, lit on every click.
    hl = [k(8.0, 0)]
    for b in range(8):
        t = 12.0 + b * BEAT
        hl += [k(t, 0.9, ease="snap", dur="2f", anchor="start"), k(t + f(3), 0.0, ease="glide", dur="9f", anchor="start")]
    lines.insert(0, {"id": "i-hl", "kind": "shape", "shape": "rect", "size": [cw - 56, ls - 6], "radius": 12, "fill": VIOLET,
                     "transform": {"position": [0, -ch / 2 + 92 + 5 * ls], "opacity": keys(*hl)}})
    dots = [{"id": f"i-dot{i}", "kind": "shape", "shape": "circle", "size": 16, "fill": c, "transform": {"position": [-cw / 2 + 44 + i * 30, -ch / 2 + 38]}}
            for i, c in enumerate(("#3a3850", "#3a3850", "#3a3850"))]
    code_card = card("i-code", 590, 610, cw, ch, INK2, dots + lines, rotation_y=14,
                     z=0, role="subject")
    code_card["transform"]["position"] = keys(k(8.0, [590, 610], frm=[590 - 500, 610], ease="settle", dur="16f", anchor="start"),
                                              k(12.0, [540, 610], ease="glide", dur="14f", anchor="start"))
    code_card["transform"]["opacity"] = fade_in(8.0, "5f")
    L.append(code_card)
    # The wire: an event travels from the button to the handler on every beat.
    wire = "M 1310 790 C 1180 900 1040 880 900 800"
    L.append({"id": "i-wire", "kind": "shape", "shape": "path", "d": wire, "time": window(12.0), "role": "decoration",
              "stroke": {"width": 5, "cap": "round", "dash": [2, 16], "gradient": {"stops": [[0, "#6e56cf90"], [1, "#6e56cf90"]]}},
              "trim": keys(k(12.0, [0, 1], frm=[0, 0], ease="glide", dur="10f", anchor="start"))})
    for b in range(1, 8):
        t = 12.0 + b * BEAT
        L.append({"id": f"i-pulse{b}", "kind": "shape", "shape": "circle", "size": 26, "fill": VIOLET, "time": window(t - f(7), t + f(1)),
                  "role": "decoration", "follow": {"layer": "i-wire", "at": keys(k(t - f(7), 0.0), k(t, 1.0, ease="glide"))}})
    # The app card.
    aw, ah = 520, 520
    ui = []
    for n in range(9):
        t0 = 12.0 + (n - 1) * BEAT if n else 11.0
        t1 = 12.0 + n * BEAT if n < 8 else None
        ui.append(text(f"i-n{n}", str(n), 0, -70, 250, F_BLACK, INK, time=window(t0, t1), allow=["line_unread"],
                       scale=keys(k(t0, 1, frm=1.25, ease="pop", anchor="start")) if n else 1))
    press = [k(11.0, 1)]
    for b in range(8):
        t = 12.0 + b * BEAT
        press += [k(t, 0.93, ease="snap", dur="2f", anchor="end"), k(t + f(1), 1.0, ease="pop", anchor="start")]
    ui.append({"id": "i-btn-bg", "kind": "shape", "shape": "rect", "size": [340, 96], "radius": 48, "fill": VIOLET,
               "transform": {"position": [0, 150], "scale": keys(*press)}})
    ui.append(text("i-btn-t", "Increment", 0, 147, 42, F_MED, PAPER, scale=keys(*press), role="decoration"))
    app = card("i-app", 1400, 610, aw, ah, "#ffffff", ui, rotation_y=-14, role="subject")
    app["time"] = window(11.5)
    app["transform"]["position"] = keys(k(12.0, [1400, 610], frm=[1400 + 900, 610], ease="settle", dur="14f", anchor="land"))
    L.append(app)
    camera = {"fov": 30, "orbit": keys(k(8.0, [5, 2]), k(16.0, [-5, 3], ease="linear")),
              "dolly": keys(k(8.0, 60), k(16.0, 150, ease="linear"))}
    return {"id": sid, "start": start, "end": end, "background": PAPER, "layers": L, "camera": camera,
            "transition": {"type": "push", "direction": "left", "dur": "10f"}}


def edge_x(align):
    return {"left": M, "center": W / 2, "right": W - M}[align]


def feature_head(prefix, t, title, tone="dark", size=88, align="left", y=150):
    """A feature's one line, on the cut."""
    c = PAPER if tone == "dark" else INK
    return text(f"{prefix}-head", title, edge_x(align), y, size, F_BOLD, c, align, -12, space="screen", role="foreground",
                animators=rise(t, "2f", "12f", 46, "word"))


def code_line(prefix, t, code, tone="dark", size=52, y=H - 128, align="left"):
    """One line of real Reflex code, typed."""
    c = LILAC if tone == "dark" else VIOLET
    return text(f"{prefix}-code", code, edge_x(align), y, size, F_MONO, c, align, space="screen", role="foreground", animators=typed(t, "0.8f"))


def stage_bg(id, tone):
    if tone == "dark":
        pts = [{"position": [300, 260], "color": "#241b4d", "radius": 560}, {"position": [-900, -500], "color": "#09090c", "radius": 520},
               {"position": [900, -540], "color": "#0a0a0e", "radius": 520}, {"position": [-960, 540], "color": "#0b0a12", "radius": 420}]
    else:
        pts = [{"position": [-500, -300], "color": "#ffffff", "radius": 620}, {"position": [700, 380], "color": "#e4dbfb", "radius": 640},
               {"position": [-900, 540], "color": "#f1ecff", "radius": 480}, {"position": [900, -540], "color": "#f8f6ff", "radius": 480}]
    return {"id": id, "kind": "shape", "shape": "field", "size": [W + 40, H + 40], "transform": {"position": [W / 2, H / 2]},
            "space": "screen", "softness": 1.4, "role": "background", "points": pts}


DARK_LIGHTS = lambda cx, cy: [
    {"type": "area", "position": [cx - 800, cy - 700, -800], "aim": [cx, cy, 0], "size": [800, 500], "color": "#fff4ea", "intensity": 1.6, "softness": 0.55},
    {"type": "directional", "from": [1.0, -0.35, 1.0], "color": "#a996ff", "intensity": 1.0},
    {"type": "ambient", "intensity": 0.07}]
LIGHT_LIGHTS = lambda cx, cy: [
    {"type": "area", "position": [cx - 700, cy - 800, -700], "aim": [cx, cy, 0], "size": [900, 600], "color": "#ffffff", "intensity": 1.5, "softness": 0.6},
    {"type": "directional", "from": [1.0, -0.4, 0.6], "color": "#e3dcff", "intensity": 0.6},
    {"type": "ambient", "intensity": 0.22}]


def label3d(id, s, x, y, z, size, color, **kw):
    return text(id, s, x, y, size, F_MONOB, color, three_d=True, z=z, role="foreground", **kw)


def s05():
    """AWAIT: one handler calls another; the call runs out along a tube, the value comes back, twice."""
    sid, start, end = "s05", 20.0, 24.0
    cx, ground = W / 2, 760
    ax, bx, by = 520, 1400, ground - 110
    L = [stage_bg("a-bg", "dark"), floor("a-floor", ground, opacity=0.85)]
    pop = lambda t: keys(k(t, 1, frm=0, ease="pop", anchor="start"))
    hit = lambda *ts: keys(k(20.0 - f(2), 1, frm=0.5, ease="pop", anchor="start"),
                           *[x for t in ts for x in (k(t, 1.12, ease="snap", dur="2f", anchor="end"), k(t + f(1), 1.0, ease="pop", anchor="start"))])
    L.append(box("a-caller", ax, by, 380, 220, 220, VIOLET, bevel=16, preset="coated", role="subject", rotation_y=-18,
                 material={"roughness": 0.22}, scale=hit(22.0, 23.5)))
    L.append(box("a-callee", bx, by, 300, 220, 220, PAPER, bevel=16, preset="glossy", role="subject", rotation_y=18,
                 material={"roughness": 0.3}, scale=hit(21.0, 23.0)))
    L.append(label3d("a-l1", "add_twice()", ax - 20, by - 190, -60, 44, PAPER, animators=rise(20.0 + f(4), "0.6f", "8f", 20)))
    L.append(label3d("a-l2", "add()", bx + 20, by - 190, -60, 44, PAPER, animators=rise(20.0 + f(6), "0.6f", "8f", 20)))
    out_d = f"M {ax + 150 - cx} -120 C {ax + 380 - cx} -430 {bx - 360 - cx} -430 {bx - 130 - cx} -120"
    back_d = f"M {bx - 130 - cx} 60 C {bx - 360 - cx} 330 {ax + 380 - cx} 330 {ax + 150 - cx} 60"
    trips = [(20.5, 21.0, 21.5, 22.0), (22.5, 23.0, 23.0 + f(3), 23.5)]
    for i, (t0, t1, t2, t3) in enumerate(trips):
        for name, d, a, b, col in (("out", out_d, t0, t1, LILAC), ("back", back_d, t2, t3, "#ffffff")):
            L.append({"id": f"a-{name}{i}", "kind": "mesh", "role": "subject", "time": window(a, b + 0.45),
                      "mesh": {"type": "sweep", "d": d, "radius": 7, "head": 26,
                               "trim": keys(k(a, [0, 0]), k(b, [0, 1], ease="glide"), k(b + 0.4, [1, 1], ease="glide"))},
                      "material": {"preset": "emissive", "color": col, "emissive": col, "emissive_strength": 2.4},
                      "transform": {"position": [cx, by - 30], "z": 0}})
    for i, (t, v) in enumerate(((22.0, "total = 1"), (23.5, "total = 2"))):
        L.append(label3d(f"a-v{i}", v, ax - 20, by - 285, -60, 56, LILAC, time=window(t, 23.5 if i == 0 else None), allow=["line_unread"],
                         scale=keys(k(t, 1, frm=0.4, ease="pop", anchor="start"))))
    L.append(feature_head("a", 20.0, "Await your event handlers."))
    L.append(code_line("a", 20.5, "total = await State.add(amount)"))
    camera = {"fov": 32, "orbit": keys(k(20.0, [-10, 9]), k(24.0, [10, 12], ease="linear")), "dolly": keys(k(20.0, -120), k(24.0, 40, ease="linear"))}
    return {"id": sid, "start": start, "end": end, "background": "#09090c", "layers": L, "camera": camera, "lights": DARK_LIGHTS(cx, by),
            "environment": {"preset": "studio", "intensity": 0.45}, "look": {"tone": "agx", "bloom": {"threshold": 0.85, "radius": 22, "intensity": 0.7}}}


def s06():
    """CLIENT STATE: a switch flips on every beat in the browser; the server behind it never wakes."""
    sid, start, end = "s06", 24.0, 28.0
    cx, ground = 1180, 800
    cy = ground - 190
    L = [stage_bg("c-bg", "light"), floor("c-floor", ground, color=None, opacity=0.32)]
    on = [k(24.0, 0)]
    for b in range(1, 8):
        t = 24.0 + b * BEAT
        on.append(k(t, b % 2, ease="snap", dur="5f", anchor="end"))
    kx = [dict(q, v=[cx - 150 + 300 * q["v"], cy]) for q in on]
    col = [dict(q, v=VIOLET if q["v"] else "#8d8a9e") for q in on]
    L.append({"id": "c-track", "kind": "mesh", "role": "subject", "mesh": {"type": "capsule", "radius": 150, "height": 600},
              "material": {"preset": "coated", "roughness": 0.3, "color": {"keys": col}},
              "transform": {"position": [cx, cy], "rotation": 90, "scale": enter(24.0, 1, 0.6, "pop", "10f")}})
    L.append({"id": "c-knob", "kind": "mesh", "role": "subject", "mesh": {"type": "sphere", "radius": 128},
              "material": {"preset": "glossy", "color": "#ffffff", "roughness": 0.18},
              "transform": {"position": {"keys": kx}, "z": -90, "scale": enter(24.0, 1, 0.6, "pop", "10f")}})
    # The server: a dark rack far behind, out of focus, its lamp off.
    for i in range(3):
        L.append(box(f"c-rack{i}", 330, ground - 60 - i * 124, 360, 104, 300, "#2a2838", bevel=10, preset="plastic", z=900,
                     material={"roughness": 0.5}, role="background", rotation_y=24))
    L.append(feature_head("c", 24.0, "State that stays in the browser.", "light", align="right"))
    L.append(code_line("c", 24.5, 'dark = rx.client_state(False, name="dark")', "light", align="right"))
    camera = {"fov": 32, "orbit": keys(k(24.0, [-10, 8]), k(28.0, [6, 10], ease="linear")), "dolly": keys(k(24.0, -80), k(28.0, 60, ease="linear")),
              "aperture": 1.0, "focus": "c-track"}
    return {"id": sid, "start": start, "end": end, "background": PAPER, "layers": L, "camera": camera, "lights": LIGHT_LIGHTS(cx, cy),
            "environment": {"preset": "soft", "intensity": 0.85}}


def s07():
    """SETVAR: two rings of members; each operation lifts and lights exactly its result."""
    sid, start, end = "s07", 28.0, 32.0
    cx, ground = W / 2, 720
    L = [stage_bg("s-bg", "dark"), floor("s-floor", ground, opacity=0.85)]
    r, off = 330, 190
    for name, x, colr in (("a", cx - off, VIOLET), ("b", cx + off, "#e9e6f5")):
        L.append({"id": f"s-ring-{name}", "kind": "mesh", "role": "subject", "mesh": {"type": "torus", "radius": r, "tube": 13},
                  "material": {"preset": "coated", "color": colr, "roughness": 0.2},
                  "transform": {"position": [x, ground - 14], "rotation_x": 90,
                                "scale": enter(28.0, 1, 0.3, "land", "14f")}})
    # Members: 4 only in a, 4 in both, 4 only in b (positions on the floor plane: x, depth).
    members = [("a", -400, -110), ("a", -330, 90), ("a", -430, 30), ("a", -300, -40),
               ("ab", -60, -120), ("ab", 60, -40), ("ab", -50, 60), ("ab", 50, 150),
               ("b", 400, -110), ("b", 330, 90), ("b", 430, 30), ("b", 300, -40)]
    ops = [(28.0, ("a", "ab", "b"), "tags | other"), (29.0, ("ab",), "tags & other"), (30.0, ("a",), "tags - other"), (31.0, ("a", "b"), "tags ^ other")]
    for i, (grp, mx, mz) in enumerate(members):
        ykeys, ckeys, skeys = [], [], []
        for t, lit, _ in ops:
            on = grp in lit
            tt = t + f(i % 4)
            ykeys.append(k(tt, [cx + mx, ground - 44 - (70 if on else 0)], ease="pop" if on else "glide", **({"anchor": "start"} if on else {"dur": "6f", "anchor": "start"})))
            ckeys.append(k(tt, LILAC if on else "#3a3850", ease="snap", dur="3f", anchor="start"))
            skeys.append(k(tt, 1.0 if on else 0.62, ease="pop" if on else "glide", **({"anchor": "start"} if on else {"dur": "6f", "anchor": "start"})))
        ykeys[0] = k(28.0 + f(i % 4), ykeys[0]["v"], frm=[cx + mx, ground - 44 - 900], ease="bounce", anchor="land", dur="14f")
        skeys[0] = k(28.0, 1.0)
        ckeys[0] = k(28.0, LILAC)
        L.append({"id": f"s-m{i}", "kind": "mesh", "role": "subject", "mesh": {"type": "sphere", "radius": 44},
                  "material": {"preset": "coated", "roughness": 0.2, "color": {"keys": ckeys}},
                  "transform": {"position": {"keys": ykeys}, "z": mz, "scale": {"keys": skeys}}})
    for j, (t, _, code) in enumerate(ops):
        L.append(text(f"s-op{j}", code, W / 2, H - 128, 56, F_MONO, LILAC, "center", space="screen", role="foreground", allow=["line_unread"],
                      time=window(t, ops[j + 1][0] if j < 3 else None), animators=typed(t, "0.5f")))
    L.append(feature_head("s", 28.0, "Sets are state vars now.", align="center", y=140))
    camera = {"fov": 32, "orbit": keys(k(28.0, [-12, 30]), k(32.0, [12, 34], ease="linear")), "dolly": keys(k(28.0, -60), k(32.0, 60, ease="linear"))}
    return {"id": sid, "start": start, "end": end, "background": "#09090c", "layers": L, "camera": camera, "lights": DARK_LIGHTS(cx, ground - 100),
            "environment": {"preset": "studio", "intensity": 0.45}}


def s08():
    """ALEMBIC: revisions drop onto the database one beat apart; the command underneath is plain Alembic."""
    sid, start, end = "s08", 32.0, 36.0
    cx, ground = 1130, 880
    L = [stage_bg("d-bg", "light"), floor("d-floor", ground, color=None, opacity=0.32)]
    discs = [(31.6, VIOLET_D), (32.0 + f(3), VIOLET), (33.0, "#8e77f0"), (34.0, LILAC), (35.0, PALE)]
    hh, gap = 92, 14
    for i, (t, colr) in enumerate(discs):
        y = ground - hh / 2 - i * (hh + gap)
        pos = [cx, y] if i == 0 else keys(k(t, [cx, y], frm=[cx, y - 1100], ease="bounce", anchor="land", dur="14f"))
        L.append({"id": f"d-disc{i}", "kind": "mesh", "role": "subject", "mesh": {"type": "cylinder", "radius": 250, "height": hh, "bevel": 14},
                  "material": {"preset": "coated", "color": colr, "roughness": 0.25},
                  "time": window(32.0 if i == 0 else t - 0.4), "transform": {"position": pos}})
    revs = ["", "a41f users", "c2d9 tags", "e7b0 index", "head"]
    for i, (t, _) in enumerate(discs[1:], 1):
        y = ground - hh / 2 - i * (hh + gap)
        L.append(text(f"d-rev{i}", revs[i], cx + 310, y - 4, 44, F_MONO, INK, "left", three_d=True, role="decoration", z=-40,
                      time=window(t), animators=rise(t, "0.5f", "8f", 14), allow=["line_unread"]))
    L.append(feature_head("d", 32.0, "Migrations are\nplain Alembic.", "light", 100, y=360))
    L[-1]["line_height"] = 1.05
    L.append(code_line("d", 32.5, "$ reflex db migrate", "light", 52, 640))
    L.append(text("d-code2", "= alembic upgrade head", M, 716, 52, F_MONO, GREY, "left", space="screen", role="foreground",
                  animators=typed(33.5, "0.8f")))
    camera = {"fov": 30, "orbit": keys(k(32.0, [-14, 14]), k(36.0, [-2, 18], ease="linear")), "dolly": keys(k(32.0, -100), k(36.0, 60, ease="linear"))}
    return {"id": sid, "start": start, "end": end, "background": PAPER, "layers": L, "camera": camera, "lights": LIGHT_LIGHTS(cx, ground - 300),
            "environment": {"preset": "soft", "intensity": 0.85}}


MONTAGE = [  # two words each: half a second is enough for two
    ("BLOCKING-CALL\nWARNINGS", INK, PAPER), ("REACT\nCOMPILER", VIOLET, PAPER), ("STRICTER\nCORS", PAPER, INK),
    ("REPRODUCIBLE\nBUILDS", INK, LILAC), ("REDIS-PY\n8", VIOLET, PAPER), ("ASYNC\nSQLITE", PAPER, VIOLET), ("20+\nFIXES", INK, PAPER),
]


def montage():
    """One more thing per beat; the last beat is silent: one block, waiting."""
    out = []
    for i, (word, bg, ink) in enumerate(MONTAGE):
        t = 36.0 + i * BEAT
        sid = f"s09{chr(97 + i)}"
        size = 230 if max(len(w) for w in word.split("\n")) <= 9 else 170
        L = [text(f"g{i}-w", word, W / 2, H / 2 - 10, size, F_BLACK, ink, tracking=-24, line_height=0.95, role="subject",
                  scale=keys(k(t, 1, frm=1.5 if i % 2 == 0 else 0.7, ease="slam", dur="5f", anchor="start"),
                             add=[{"type": "drift", "rate": 0.002, "per": "frame", "mode": "mul"}]))]
        out.append({"id": sid, "start": t, "end": t + BEAT, "background": bg, "layers": L, "lead": 0})
    t = 39.5
    L = [{"id": "g-block", "kind": "shape", "shape": "rect", "size": [84, 84], "radius": 8, "fill": VIOLET, "role": "subject",
          "transform": {"position": [W / 2, H / 2], "scale": keys(k(t, 1, frm=0.5, ease="pop", anchor="start"), k(40.0, 0.7, ease="gesture", dur="6f", anchor="end"))}}]
    out.append({"id": "s09h", "start": t, "end": 40.0, "background": INK, "layers": L, "lead": 0})
    return out


def s10():
    """END: a field of blocks ripples on the impact, the mark assembles in violet, and the film says how to get it."""
    sid, start, end = "s10", 40.0, DURATION
    L = []
    L.append({"id": "e-grid", "kind": "instancer", "transform": {"position": [W / 2, H / 2]}, "role": "background",
              "layout": {"type": "grid", "columns": 41, "rows": 23, "spacing": [48, 48]},
              "fields": [{"type": "wave", "center": [0, 0], "at": 40.0, "speed": 1500, "wavelength": 420, "scale": 3.2, "color": VIOLET, "ease": "gesture"},
                         {"type": "noise", "size": 380, "speed": 0.35, "seed": 5, "scale": 0.5}],
              "layers": [{"id": "e-cell", "kind": "shape", "shape": "rect", "size": [8, 8], "radius": 2, "fill": "#e9e3fa"}]})
    unit = 76
    name_size = 270
    name_w = text_width("reflex", F_BOLD, name_size, -20)
    ver_w = text_width("0.11", F_BOLD, 150, -6) + 90
    gap = 80
    mw = MARK_W * unit / 48
    total = mw + gap + name_w + 50 + ver_w
    x0 = (W - total) / 2
    mx, cy = x0 + mw / 2, 400
    L.append({"id": "e-wall", "kind": "mesh", "mesh": {"type": "plane", "size": [9000, 9000]},
              "material": {"preset": "shadow", "color": "#2a1c66", "opacity": 0.38}, "transform": {"position": [W / 2, H / 2], "z": 42}})
    fly = {"stem": (-1400, 0, -900), "bar": (0, -1000, -700), "bowl": (1500, -600, -1200), "mid": (700, 900, -600), "leg": (1300, 800, -1000)}
    for i, b in enumerate(mark_blocks("e", mx, cy, unit, 76, VIOLET, "coated", roughness=0.22)):
        dx, dy, dz = fly[b["id"][2:]]
        p = b["transform"]["position"]
        t = 40.0 - f(3) + f(i) / 2
        b["role"] = "subject"
        b["transform"]["position"] = keys(k(t, p, frm=[p[0] + dx, p[1] + dy], ease="slam", dur="7f", anchor="start"))
        b["transform"]["z"] = keys(k(t, 0, frm=dz, ease="slam", dur="7f", anchor="start"))
        b["transform"]["rotation_y"] = keys(k(t, 0, frm=70 if dx > 0 else -70, ease="slam", dur="7f", anchor="start"))
        L.append(b)
    nx = x0 + mw + gap
    L.append(text("e-name", "reflex", nx + name_w / 2, cy - 16, name_size, F_BOLD, INK, tracking=-20, three_d=True, z=38,
                  animators=[{"at": 40.0 + f(5), "unit": "glyph", "order": "forward", "stagger": "1f", "dur": "8f", "ease": "glide", "from": {"opacity": 0, "x": 90}}]))
    vx = nx + name_w + 50 + ver_w / 2
    L.append({"id": "e-ver", "kind": "comp", "three_d": True, "role": "subject",
              "transform": {"position": [vx, cy + 16], "z": 38, "scale": keys(k(40.5, 1, frm=0, ease="pop", anchor="start"))}, "layers": [
                  {"id": "e-ver-bg", "kind": "shape", "shape": "rect", "size": [ver_w, 190], "radius": 95, "fill": VIOLET},
                  text("e-ver-t", "0.11", 0, -8, 150, F_BOLD, PAPER, tracking=-6)]})
    pill_w = text_width("pip install --upgrade reflex", F_MONO, 54) + 120
    L.append({"id": "e-pip", "kind": "comp", "three_d": True, "role": "subject", "time": window(42.0),
              "transform": {"position": [W / 2, 720], "z": 38, "scale": keys(k(42.0, 1, frm=0.6, ease="pop", anchor="start")), "opacity": fade_in(42.0, "4f")},
              "layers": [{"id": "e-pip-bg", "kind": "shape", "shape": "rect", "size": [pill_w, 124], "radius": 30, "fill": INK2},
                         text("e-pip-t", "pip install --upgrade reflex", 0, -4, 54, F_MONO, PAPER, animators=typed(42.0 + f(3), "0.7f"))]})
    L.append(text("e-url", "reflex.dev", W / 2, 880, 56, F_MED, VIOLET, tracking=-4, three_d=True, z=38, animators=rise(43.0, "1f", "12f", 30)))
    camera = {"fov": 30, "orbit": keys(k(40.0, [-8, 3]), k(DURATION, [6, 4], ease="linear")),
              "dolly": keys(k(40.0, 100, frm=-500, ease="settle", dur="20f", anchor="start"), k(DURATION, 190, ease="linear"))}
    lights = [{"type": "directional", "from": [-0.7, -1.0, -1.0], "color": "#ffffff", "intensity": 1.5, "shadow": True, "softness": 0.6},
              {"type": "directional", "from": [1.0, 0.2, -0.4], "color": "#d9d0ff", "intensity": 0.5}, {"type": "ambient", "intensity": 0.25}]
    return {"id": sid, "start": start, "end": end, "background": PAPER, "layers": L, "camera": camera, "lights": lights,
            "environment": {"preset": "soft", "intensity": 0.9}}


SCENES = [s01, s02, s03, s04, s05, s06, s07, s08, montage, s10]


# ------------------------------------------------------------------ project

def build(only=None):
    if os.path.exists(OUT):
        shutil.rmtree(OUT)
    for d in ("scenes", "markers", "assets/fonts"):
        os.makedirs(os.path.join(OUT, d))
    for fn, d in SYSTEM_FONTS.items():
        shutil.copy(d + fn, os.path.join(OUT, "assets", "fonts", fn))
    shutil.copy("/usr/share/doc/fonts-inter/copyright", os.path.join(OUT, "assets", "fonts", "LICENSE-Inter.txt"))
    shutil.copy("/usr/share/doc/fonts-jetbrains-mono/copyright", os.path.join(OUT, "assets", "fonts", "LICENSE-JetBrainsMono.txt"))
    shutil.copy(os.path.join(HERE, "track.wav"), os.path.join(OUT, "assets", "track.wav"))
    if SLANG:
        shutil.copytree(os.path.join(SRC, "shaders"), os.path.join(OUT, "shaders"))
    marks = json.load(open(os.path.join(HERE, "track-markers.json")))
    for name, ms in marks.items():
        json.dump({"name": name, "source": "music.py (exact grid)", "markers": ms}, open(os.path.join(OUT, "markers", f"{name}.json"), "w"))
    scenes = []
    for fn in SCENES:
        r = fn()
        scenes.extend(r if isinstance(r, list) else [r])
    if only:
        scenes = [s for s in scenes if s["id"] in only]
    for s in scenes:
        json.dump(s, open(os.path.join(OUT, "scenes", f"{s['id']}.json"), "w"), ensure_ascii=False)
    assets = [{"path": "assets/track.wav", "source": "original", "licence": "original score, synthesized for this reel (music.py)"}]
    for fn in sorted(SYSTEM_FONTS):
        lic = "LICENSE-Inter.txt" if "Inter" in fn else "LICENSE-JetBrainsMono.txt"
        assets.append({"path": f"assets/fonts/{fn}", "source": "Debian fonts-inter / fonts-jetbrains-mono",
                       "licence": f"SIL Open Font License 1.1 (assets/fonts/{lic})"})
    project = {"version": 1, "settings": {"width": W, "height": H, "fps": FPS, "duration": DURATION, "background": INK, "renderer": "path", "samples": 24,
                            "motion_blur": {"shutter": 180, "samples": 8}},
               "audio": {"music": {"path": "assets/track.wav", "at": 0.0}}, "assets": assets, "scenes": [s["id"] for s in scenes]}
    json.dump(project, open(os.path.join(OUT, "project.json"), "w"), indent=1)
    print("wrote", OUT, [s["id"] for s in scenes])


if __name__ == "__main__":
    args = sys.argv[1:]
    SLANG = "--slang" in args
    build([a for a in args if not a.startswith("--")] or None)
