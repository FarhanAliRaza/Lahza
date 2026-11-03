"""Builds the Lahza showreel project (reel.lahza) from code.

Design system
  Palette   ink #0e0e0d, paper #f2eee6, brand red #d03734 (from the wordmark),
            plus greys from the ink. One accent; red marks "the moment".
  Type      Inter Display (Black for headlines, Light/Thin for contrast),
            JetBrains Mono Medium for labels and readouts (tracked +120..400).
  Grid      1920x1080, 96 px margins, 12 columns of 122 px, 24 px gutters.
  Motion    exp-lag glide/snap for moves, slam on hits, springs on scale only,
            everything on the 120 BPM grid (beat 0.5 s, bar 2 s); cuts lead 1 f.
  HUD       screen-space: wordmark label, section index, frame counter and a
            timeline with the red dot as playhead, the same in every scene.
"""
import json
import math
import os
import shutil
import sys

from PIL import ImageFont

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from svgpath import wordmark  # noqa: E402

HERE = os.getcwd()  # where music.py wrote track.wav; the project goes here too
OUT = os.path.join(HERE, "reel.lahza")
SYSTEM_FONTS = {  # Debian/Ubuntu: fonts-inter, fonts-jetbrains-mono
    "InterDisplay-Black.otf": "/usr/share/fonts/opentype/inter/", "InterDisplay-Thin.otf": "/usr/share/fonts/opentype/inter/",
    "InterDisplay-Light.otf": "/usr/share/fonts/opentype/inter/", "InterDisplay-SemiBold.otf": "/usr/share/fonts/opentype/inter/",
    "InterDisplay-Medium.otf": "/usr/share/fonts/opentype/inter/", "JetBrainsMono-Medium.ttf": "/usr/share/fonts/truetype/jetbrains-mono/",
    "JetBrainsMono-Regular.ttf": "/usr/share/fonts/truetype/jetbrains-mono/",
}
W, H, FPS = 1920, 1080, 30
DURATION = 51.0
M = 96  # margin

INK = "#0e0e0d"
INK2 = "#1b1b1a"
PAPER = "#f2eee6"
RED = "#d03734"
GREY = "#8a8780"

F_BLACK = "assets/fonts/InterDisplay-Black.otf"
F_THIN = "assets/fonts/InterDisplay-Thin.otf"
F_LIGHT = "assets/fonts/InterDisplay-Light.otf"
F_SEMI = "assets/fonts/InterDisplay-SemiBold.otf"
F_MED = "assets/fonts/InterDisplay-Medium.otf"
F_MONO = "assets/fonts/JetBrainsMono-Medium.ttf"

BEAT = 0.5
BAR = 2.0


def beat(n):
    """Reference to beat n (1-based); beat:1 is 0.0 s."""
    return f"beat:{n}"


def bt(n):
    return (n - 1) * BEAT


def bar(n):
    return f"bar:{n}"


def f(frames):
    return frames / FPS


# ------------------------------------------------------------------ helpers

_fonts = {}


def text_width(text, font, size, tracking=0.0):
    key = (font, size)
    if key not in _fonts:
        name = os.path.basename(font)
        _fonts[key] = ImageFont.truetype(SYSTEM_FONTS[name] + name, size)
    lines = text.split("\n")
    return max(_fonts[key].getlength(l) + tracking / 1000 * size * max(0, len(l) - 1) for l in lines)


def k(at, v, **kw):
    d = {"at": at, "v": v}
    d.update(kw)
    return d


def keys(*ks, add=None):
    d = {"keys": list(ks)}
    if add:
        d["add"] = add
    return d


def text(id, s, x, y, size, font=F_MONO, color=PAPER, align="center", tracking=0, **kw):
    """A text layer; `align` left/right places its edge at x (measured)."""
    cx = x  # the engine anchors left/right aligned text at its edge
    layer = {"id": id, "kind": "text", "text": s, "font": font, "size": size, "color": color,
             "transform": {"position": [round(cx, 1), y]}}
    if tracking:
        layer["tracking"] = tracking
    if align != "center":
        layer["align"] = align
    for key, v in kw.items():
        if key in ("opacity", "scale", "rotation", "anchor"):
            layer["transform"][key] = v
        else:
            layer[key] = v
    return layer


def rect(id, x, y, w, h, fill, **kw):
    layer = {"id": id, "kind": "shape", "shape": "rect", "size": [w, h], "fill": fill, "transform": {"position": [x, y]}}
    layer.update(kw)
    return layer


def fade_in(at, dur="8f", ease="glide"):
    return keys(k(at, 1, **{"from": 0, "ease": ease, "dur": dur, "anchor": "start"}))


def fade_inout(t_in, t_out, dur="8f"):
    return keys(k(t_in, 1, **{"from": 0, "ease": "glide", "dur": dur, "anchor": "start"}),
                k(t_out, 0, ease="gesture", dur=dur, anchor="end"))


def window(start, end=None):
    d = {"start": start}
    if end is not None:
        d["end"] = end
    return d


# ------------------------------------------------------------------ HUD

SECTIONS = [  # (start s, label)
    (0.0, "MOMENT"), (112 / FPS, "TITLE"), (8.0, "TYPE"), (14.0, "SHAPE"), (16.0, "LIGHT"),
    (21.0, "FORM"), (26.0, "DATA"), (34.0, "SYNC"), (40.0, "MOTION"), (44.0, "LAHZA"),
]


def hud(scene_id, start, end, tone="dark", index=None):
    """Screen-space chrome shared by every scene. tone: dark (paper ink on dark) or light."""
    c = PAPER if tone != "light" else INK
    dim = 0.55 if tone != "light" else 0.62
    out = []
    p = scene_id
    y_top = M - 30
    out.append(text(f"{p}-hud-mark", "LAHZA", M, y_top, 17, F_MONO, c, "left", 260, opacity=dim, space="screen"))
    out.append(text(f"{p}-hud-mark2", "MOTION ENGINE", M + 104, y_top, 17, F_MONO, c, "left", 260, opacity=dim * 0.6, space="screen"))
    if index is not None:
        n, label = index
        out.append(text(f"{p}-hud-sec", f"{n:02d} / 10   {label}", W - M, y_top, 17, F_MONO, c, "right", 260, opacity=dim, space="screen"))
    # Timeline: hairline across the bottom, ticks at section starts, red playhead.
    y = H - M + 34
    x0, x1 = M, W - M
    out.append({"id": f"{p}-hud-line", "kind": "shape", "shape": "line", "size": [x1 - x0, 0], "thickness": 1,
                "fill": c, "transform": {"position": [(x0 + x1) / 2, y], "opacity": dim * 0.55}, "space": "screen"})
    for i, (t, _) in enumerate(SECTIONS):
        x = x0 + (x1 - x0) * t / DURATION
        out.append({"id": f"{p}-hud-tick{i}", "kind": "shape", "shape": "rect", "size": [1, 9], "fill": c,
                    "transform": {"position": [round(x, 1) + 0.5, y], "opacity": dim * 0.8}, "space": "screen"})
    xs = x0 + (x1 - x0) * start / DURATION
    xe = x0 + (x1 - x0) * end / DURATION
    out.append({"id": f"{p}-hud-head", "kind": "shape", "shape": "circle", "size": 9, "fill": RED, "space": "screen",
                "transform": {"position": keys(k(start, [xs, y]), k(end, [xe, y], ease="linear"))}})
    # Frame counter, left, above the line; resolution, right.
    f0, f1 = round(start * FPS), round(end * FPS)
    cw = text_width("F 0000", F_MONO, 17, 260)
    out.append({"id": f"{p}-hud-frame", "kind": "component", "component": "counter",
                "params": {"from": f0, "to": f1, "at": start, "dur": end - start, "ease": "linear", "decimals": 0,
                           "separator": "", "prefix": "F ", "size": 17, "font": F_MONO, "color": c},
                "transform": {"position": [M + cw / 2, y - 26], "opacity": dim}, "space": "screen"})
    out.append(text(f"{p}-hud-spec", "1920×1080 · 30 FPS · 120 BPM", W - M, y - 26, 17, F_MONO, c, "right", 160, opacity=dim * 0.8, space="screen"))
    return out


# ------------------------------------------------------------------ scenes

def s01():
    """MOMENT: a clock dial ticks with the score; the red dot swallows the frame."""
    sid, start, end = "s01", 0.0, 112 / FPS
    cx, cy = W / 2, H / 2 - 20
    L = []
    # Ticks: quarters in bar 1, eighths in bar 2 (from the score).
    ticks = [0.0, 0.5, 1.0, 1.5] + [2.0 + i * 0.25 for i in range(7)]
    # Dial: 60 marks, every fifth long; drawn on clockwise over the first bar.
    L.append({"id": "dial", "kind": "instancer", "transform": {"position": [cx, cy]},
              "layout": {"type": "radial", "count": 60, "radius": 236, "align": True},
              "stagger": {"total": "40f", "order": "forward"},
              "layers": [{"id": "mark", "kind": "shape", "shape": "rect", "size": [2, 14], "fill": PAPER,
                          "transform": {"opacity": keys(k(0.0, 0.42, **{"from": 0, "ease": "glide", "dur": "10f", "anchor": "start"}))}}]})
    L.append({"id": "dial-major", "kind": "instancer", "transform": {"position": [cx, cy]},
              "layout": {"type": "radial", "count": 12, "radius": 230, "align": True},
              "stagger": {"total": "40f", "order": "forward"},
              "layers": [{"id": "major", "kind": "shape", "shape": "rect", "size": [3, 30], "fill": PAPER,
                          "transform": {"opacity": keys(k(0.0, 0.9, **{"from": 0, "ease": "glide", "dur": "10f", "anchor": "start"}))}}]})
    # Hairline orbit that draws on with the dial.
    L.append({"id": "orbit", "kind": "shape", "shape": "path", "d": {"shape": "circle", "size": 400},
              "stroke": {"color": PAPER, "width": 1, "feather": 60}, "transform": {"position": [cx, cy], "opacity": 0.35},
              "trim": keys(k(0.0, [0, 0]), k(1.4, [0, 1], ease="glide", dur="40f", anchor="start"))})
    # The hand: ticks clockwise on every tick, 30° a step, with a tiny settle.
    rot = [k(0.0, 0)]
    for i, t in enumerate(ticks):
        rot.append(k(t, 30 * (i + 1), ease="snap", dur="5f", anchor="start"))
    L.append({"id": "hand", "kind": "shape", "shape": "rect", "size": [3, 196], "fill": PAPER, "radius": 1.5,
              "transform": {"position": [cx, cy], "anchor": [0, 84], "rotation": keys(*rot), "opacity": fade_in(0.0, "6f")}})
    # A ripple ring leaves the dot on every tick.
    for i, t in enumerate(ticks):
        big = t < 2.0
        L.append({"id": f"ripple{i}", "kind": "shape", "shape": "ring", "size": 44, "thickness": 1.5, "fill": RED,
                  "time": window(t, t + 0.9),
                  "transform": {"position": [cx, cy],
                                "scale": keys(k(t, 7.5 if big else 5, **{"from": 1, "ease": "glide", "dur": "26f", "anchor": "start"})),
                                "opacity": keys(k(t, 0, **{"from": 0.9 if big else 0.6, "ease": "glide", "dur": "26f", "anchor": "start"}))}})
    # The dot: pops on the first tick, pulses on each, then swallows the frame.
    sc = [k(0.0, 1, **{"from": 0, "ease": "pop", "anchor": "start"})]
    for t in [t for t in ticks[1:] if t < 2.9]:
        sc.append(k(t, 1.18, ease="snap", dur="2f", anchor="start"))
        sc.append(k(t + f(2), 1.0, ease="pop", anchor="start"))
    # Anticipation, then it grows to cover the frame by the cut.
    sc.append(k(3.0, 0.62, ease="gesture", dur="7f", anchor="start"))
    sc.append(k(end, 52, ease="zoom", dur="12f", anchor="end"))
    L.append({"id": "dot", "kind": "shape", "shape": "circle", "size": 44, "fill": RED,
              "transform": {"position": [cx, cy], "scale": keys(*sc)}})
    # Copy: the idea in one line, wiped on; the dial and copy leave before the dot grows.
    L.append(text("copy", "Every frame is a moment.", cx, cy + 340, 46, F_LIGHT, PAPER, tracking=-10,
                  animators=[{"at": 0.75, "unit": "glyph", "order": {"axis": "x", "feather": 0.35}, "stagger": "1f",
                              "dur": "14f", "ease": "glide", "from": {"opacity": 0, "y": 10}}]))
    L.append(text("copy2", "LAHZA  —  URDU FOR “A MOMENT”", cx, cy + 404, 17, F_MONO, GREY, tracking=260,
                  animators=[{"at": 2.0, "unit": "glyph", "order": "forward", "stagger": "0.4f", "dur": "8f", "from": {"opacity": 0}}]))
    # Everything but the dot fades as it grows.
    for l in L:
        if l["id"] in ("dot",) or l["id"].startswith("ripple"):
            continue
        tr = l.setdefault("transform", {})
        op = tr.get("opacity")
        base = op if isinstance(op, (int, float)) else None
        if isinstance(op, dict):
            op["keys"].append(k(3.0, 0, ease="gesture", dur="10f", anchor="start"))
        else:
            tr["opacity"] = keys(k(0.0, 1 if base is None else base), k(3.0, 0, ease="gesture", dur="10f", anchor="start"))
    L += hud(sid, start, end, "dark", (1, "MOMENT"))
    return {"id": sid, "start": start, "end": end, "background": INK, "layers": L}


def s02():
    """TITLE: LAHZA slams on the impact, framed by the viewfinder brackets."""
    sid, start, end = "s02", 112 / FPS, 8.0
    hit = "hit:1"  # 4.0 s
    cx, cy = W / 2, H / 2 - 40
    L = []
    title_w = text_width("LAHZA", F_BLACK, 300, -30)
    # Hairline 12-column grid, drawn on after the hit, very faint.
    col = (W - 2 * M - 11 * 24) / 12
    for i in range(13):
        x = M + i * (col + 24) - (12 if 0 < i < 12 else 0) if i < 12 else W - M
        L.append({"id": f"grid{i}", "kind": "shape", "shape": "line", "size": [0, H - 2 * M], "thickness": 1, "fill": PAPER,
                  "transform": {"position": [round(x, 1) + 0.5, H / 2],
                                "opacity": keys(k(4.0 + i * f(1), 0.13, **{"from": 0, "ease": "glide", "dur": "12f", "anchor": "start"}))},
                  "trim": keys(k(4.0 + i * f(1), [0, 1], **{"from": [0.5, 0.5], "ease": "glide", "dur": "18f", "anchor": "start"}))})
    # The title.
    L.append(text("title", "LAHZA", cx, cy, 300, F_BLACK, PAPER, tracking=-30,
                  animators=[{"at": hit, "unit": "glyph", "order": "center", "stagger": "1f", "dur": "6f", "ease": "slam",
                              "anchor": "end", "from": {"scale": 2.6, "opacity": 0}}],
                  scale=keys(k(4.0, 1), add=[{"type": "drift", "rate": 0.0006, "per": "frame", "mode": "mul"}])))
    # Viewfinder brackets (from the wordmark) frame the title, snapping in from outside.
    bw, bh = title_w + 150, 360
    corners = [(-1, -1), (1, -1), (1, 1), (-1, 1)]
    for i, (sx, sy) in enumerate(corners):
        d = f"M {sx*0} {sy*0} L {-sx*52} 0 L {-sx*52} {sy*0+sy*14} L {-sx*0+sx*0} {sy*14} Z"
        x, y = cx + sx * bw / 2, cy + sy * bh / 2
        L.append({"id": f"br{i}", "kind": "shape", "shape": "path",
                  "d": f"M 0 0 L {-sx*56} 0 L {-sx*56} {-sy*12} L {-sx*12} {-sy*12} L {-sx*12} {-sy*56} L 0 {-sy*56} Z",
                  "fill": PAPER,
                  "transform": {"position": keys(k(4.0, [x, y], **{"from": [x + sx * 140, y + sy * 90], "ease": "glide", "dur": "16f", "anchor": "start"})),
                                "opacity": fade_in(4.0, "4f")}})
    # Rule and kicker.
    L.append({"id": "rule", "kind": "shape", "shape": "line", "size": [title_w, 0], "thickness": 2, "fill": PAPER,
              "transform": {"position": [cx, cy + 150]},
              "trim": keys(k(4.25, [0, 1], **{"from": [0.5, 0.5], "ease": "glide", "dur": "16f", "anchor": "start"}))})
    L.append(text("kicker", "MOTION ENGINE", cx, cy + 196, 26, F_MONO, PAPER, tracking=520,
                  animators=[{"at": 4.5, "unit": "glyph", "order": "center", "stagger": "1f", "dur": "10f", "ease": "glide",
                              "from": {"opacity": 0, "y": 12}}]))
    # Three facts on the grid, bar 4.
    facts = [("01", "LANGUAGE", "Rust"), ("02", "RENDER", "GPU + CPU"), ("03", "TIMING", "Frame-accurate")]
    colx = [M + 3 * (col + 24), M + 6 * (col + 24), M + 9 * (col + 24)]
    fy = H - M - 130
    for i, (n, lab, val) in enumerate(facts):
        t0 = 6.0 + i * f(3)
        L.append(text(f"fact{i}-n", f"{n}  {lab}", colx[i], fy, 15, F_MONO, PAPER, "left", 300,
                      opacity=keys(k(t0, 0.7, **{"from": 0, "ease": "glide", "dur": "10f", "anchor": "start"}))))
        L.append(text(f"fact{i}-v", val, colx[i], fy + 40, 34, F_MED, PAPER, "left", -10,
                      animators=[{"at": t0 + f(2), "unit": "glyph", "stagger": "0.5f", "dur": "10f", "ease": "glide",
                                  "from": {"opacity": 0, "y": 18}}]))
        L.append({"id": f"fact{i}-rule", "kind": "shape", "shape": "line", "size": [col * 2 + 24, 0], "thickness": 1, "fill": PAPER,
                  "transform": {"position": [colx[i] + col + 12, fy - 26], "opacity": 0.5},
                  "trim": keys(k(t0, [0, 1], **{"from": [0, 0], "ease": "glide", "dur": "14f", "anchor": "start"}))})
    # Exit: the title and facts lift away on beat 16 (7.5 s), peak speed on the beat.
    for l in L:
        if l["id"].startswith(("title", "kicker", "fact", "rule", "br")):
            tr = l["transform"]
            pos = tr["position"]
            p0 = pos if isinstance(pos, list) else pos["keys"][-1]["v"]
            ks = pos["keys"] if isinstance(pos, dict) else [k(4.0, p0)]
            ks.append(k(beat(16), [p0[0], p0[1] - 90], ease="gesture", dur="10f", anchor="peak"))
            tr["position"] = {"keys": ks}
            op = tr.get("opacity")
            if isinstance(op, dict):
                op["keys"].append(k(beat(16), 0, ease="gesture", dur="10f", anchor="peak"))
            else:
                tr["opacity"] = keys(k(4.0, op if isinstance(op, (int, float)) else 1), k(beat(16), 0, ease="gesture", dur="10f", anchor="peak"))
    L += hud(sid, start, end, "dark", (2, "TITLE"))
    return {"id": sid, "start": start, "end": end, "background": RED, "layers": L,
            "motion_blur": {"shutter": 180, "samples": 6}}


COL = (W - 2 * M - 11 * 24) / 12


def colx(i):
    """Left edge of column i (0-based)."""
    return M + i * (COL + 24)


def label(id, s, x, y, color, at, size=15, dot=True, **kw):
    """A mono label with a small red square before it, typed on at `at`."""
    out = []
    if dot:
        out.append(rect(f"{id}-dot", x + 4, y, 8, 8, RED, transform={"position": [x + 4, y], "scale": keys(k(at, 1, **{"from": 0, "ease": "pop", "anchor": "start"}))}))
        x += 22
    out.append(text(id, s, x, y, size, F_MONO, color, "left", 220,
                    animators=[{"at": at, "unit": "glyph", "stagger": "0.5f", "dur": "4f", "ease": "snap", "from": {"opacity": 0}}], **kw))
    return out


def s03():
    """TYPE: a Swiss specimen sheet; each line shows a text animator."""
    sid, start, end = "s03", 8.0, 14.0
    L = []
    # Column rule.
    L.append({"id": "t-rule", "kind": "shape", "shape": "line", "size": [0, H - 2 * M - 120], "thickness": 1, "fill": INK,
              "transform": {"position": [colx(6) - 12.5, H / 2], "opacity": 0.25},
              "trim": keys(k(8.0, [0, 1], **{"from": [0, 0], "ease": "glide", "dur": "24f", "anchor": "start"}))})
    # The specimen: Aa, rising in, then thin, bold, outline, red.
    ax, ay = colx(0) + (6 * COL + 5 * 24) / 2, H / 2 - 10
    L.append(text("aa", "Aa", ax, ay, 520, F_BLACK, INK, tracking=-40,
                  animators=[{"at": 8.0, "unit": "glyph", "stagger": "3f", "dur": "16f", "ease": "glide",
                              "from": {"y": 220, "opacity": 0}}],
                  weight=keys(k(9.0, -9, ease="glide", dur="14f", anchor="start"), k(10.0, 5, ease="land", anchor="start"),
                              k(12.0, 0, ease="glide", dur="10f", anchor="start")),
                  stroke={"color": keys(k(11.0, INK)), "width": keys(k(11.0, 4, **{"from": 0, "ease": "glide", "dur": "8f", "anchor": "start"}),
                                                                      k(12.0, 0, ease="glide", dur="8f", anchor="start")), "align": "center"}))
    L[-1]["color"] = keys(k(11.0, "#0e0e0d00", **{"from": INK, "ease": "glide", "dur": "8f", "anchor": "start"}),
                          k(12.0, RED, ease="glide", dur="8f", anchor="start"))
    # Caption under the specimen: what is changing.
    caps = [(8.0, "INTER DISPLAY BLACK · 520 PX"), (9.0, "WEIGHT  −9 PX"), (10.0, "WEIGHT  +5 PX · SPRING"),
            (11.0, "OUTLINE · STROKE 4 PX"), (12.0, "FILL  #D03734"), (13.0, "")]
    for i, (t, c) in enumerate(caps[:-1]):
        L.append(text(f"aa-cap{i}", c, colx(0), H - M - 120, 15, F_MONO, INK, "left", 220, time=window(t, caps[i + 1][0]),
                      animators=[{"at": t, "unit": "glyph", "stagger": "0.5f", "dur": "3f", "from": {"opacity": 0}}], opacity=0.75))
    # Right column: one animator per line, on the beat.
    lines = [
        (8.5, "Per glyph", "GLYPH · STAGGER 1F · ENTER", {"unit": "glyph", "stagger": "1f", "dur": "12f", "ease": "enter", "from": {"y": 70, "opacity": 0}}),
        (9.5, "Word by word", "WORD · ROTATE · GLIDE", {"unit": "word", "stagger": "4f", "dur": "14f", "ease": "glide", "from": {"rotation": -14, "y": 50, "opacity": 0}}),
        (10.5, "Feathered wipe", "AXIS X · FEATHER 0.4", {"unit": "glyph", "order": {"axis": "x", "feather": 0.4}, "stagger": "1f", "dur": "14f", "ease": "glide", "from": {"opacity": 0, "x": -24}}),
        (11.5, "From the centre", "RADIAL · SCALE · POP", {"unit": "glyph", "order": {"radial": [0.5, 0.5]}, "stagger": "1f", "dur": "10f", "ease": "pop", "from": {"scale": 0, "opacity": 0}}),
        (12.5, "In any order", "RANDOM · SEED 4", {"unit": "glyph", "order": "random", "seed": 4, "stagger": "0.7f", "dur": "8f", "ease": "snap", "from": {"opacity": 0, "y": -30}}),
    ]
    x = colx(6) + 24
    for i, (t, s_, lab, anim) in enumerate(lines):
        y = 236 + i * 142
        an = dict(anim)
        an["at"] = beat(int(t / BEAT) + 1)
        L += label(f"l{i}-lab", lab, x, y - 50, INK, beat(int(t / BEAT) + 1), opacity=0.7)
        L.append(text(f"l{i}", s_, x, y, 74, F_MED, INK, "left", -25, animators=[an]))
    # Exit: the sheet slides up out of frame on beat 28 (13.5 s), peak on the beat.
    for l in L:
        if l["id"].startswith(("t-", "aa", "l")):
            tr = l.setdefault("transform", {})
            pos = tr.get("position")
            if isinstance(pos, list):
                tr["position"] = keys(k(8.0, pos), k(13.82, [pos[0], pos[1] - 1100], ease="gesture", dur="12f", anchor="peak"))
    L += hud(sid, start, end, "light", (3, "TYPE"))
    return {"id": sid, "start": start, "end": end, "background": PAPER, "layers": L,
            "transition": {"type": "wipe", "direction": "up", "dur": "10f"}}


def s04():
    """SHAPE: one outline morphs on every hit of the snare roll; a ring of echoes follows it."""
    sid, start, end = "s04", 14.0, 16.0
    cx, cy = W / 2, H / 2 - 10
    L = []
    # Morph sequence on the roll: 14.0, 14.5, 14.75, 15.0, 15.125, 15.25, 15.375 → collapse at 15.5.
    seq = [(14.0, {"shape": "circle", "size": 300}),
           (14.5, {"shape": "rect", "size": [300, 300], "radius": 36}),
           (14.75, {"shape": "polygon", "size": 340, "sides": 3, "radius": 12}),
           (15.0, {"shape": "star", "size": 360, "sides": 5, "inner": 0.48, "radius": 6}),
           (15.125, {"shape": "polygon", "size": 320, "sides": 6, "radius": 10}),
           (15.25, {"shape": "star", "size": 360, "sides": 8, "inner": 0.62, "radius": 4}),
           (15.375, {"shape": "circle", "size": 300})]
    morph = {"keys": [k(seq[0][0], seq[0][1])] + [k(t, d, ease="gesture", dur="5f", anchor="end") for t, d in seq[1:]]}
    spin = keys(k(14.0, 0), k(15.5, 180, ease="glide", dur="40f", anchor="end"))
    collapse = keys(k(14.0, 1, **{"from": 0.2, "ease": "glide", "dur": "10f", "anchor": "start"}), k(15.5, 0, ease="slam", dur="6f", anchor="end"))
    # Ring of 24 echoes, each a beat behind the next (stagger), drawn as hairlines.
    L.append({"id": "echoes", "kind": "instancer", "transform": {"position": [cx, cy], "rotation": keys(k(14.0, 0), k(16.0, -40, ease="linear")), "scale": collapse},
              "layout": {"type": "radial", "count": 24, "radius": 360, "align": True},
              "stagger": {"each": "1f", "order": "forward"},
              "palette": [PAPER, PAPER, RED], "palette_order": "cycle",
              "layers": [{"id": "echo", "kind": "shape", "shape": "path", "d": {"keys": [dict(x, d={**x["v"], "size": 0}) if False else {**x, "v": {**x["v"], "size": ([v * 0.16 for v in x["v"]["size"]] if isinstance(x["v"]["size"], list) else x["v"]["size"] * 0.16)}} for x in morph["keys"]]},
                          "fill": "#00000000", "stroke": {"color": PAPER, "width": 1.5}, "transform": {"rotation": spin}}]})
    # Orbits: colour along the length + feathered heads, and marching ants.
    for i, (r, w, a, b, dur) in enumerate([(560, 2, RED, PAPER, "30f"), (660, 1.5, PAPER, RED, "36f")]):
        L.append({"id": f"orbit{i}", "kind": "shape", "shape": "path", "d": {"shape": "circle", "size": r},
                  "stroke": {"width": w, "feather": 260, "feather_start": 40, "gradient": {"stops": [[0, a], [1, b]], "along": "length"}},
                  "trim": keys(k(14.0 + i * 0.25, [0, 0.85], **{"from": [0, 0], "ease": "glide", "dur": dur, "anchor": "start"})),
                  "transform": {"position": [cx, cy], "rotation": keys(k(14.0, -90 + i * 120), k(16.0, 30 + i * 200, ease="linear")), "scale": collapse}})
    L.append({"id": "ants", "kind": "shape", "shape": "path", "d": {"shape": "circle", "size": 470},
              "stroke": {"color": PAPER, "width": 1.5, "cap": "butt", "dash": [3, 9], "dash_offset": keys(k(14.0, 0), k(16.0, -120, ease="linear"))},
              "transform": {"position": [cx, cy], "opacity": 0.45, "scale": collapse}})
    # The hero outline: red fill, morphing, spinning a half turn over the roll.
    L.append({"id": "hero", "kind": "shape", "shape": "path", "d": morph, "fill": RED,
              "transform": {"position": [cx, cy], "rotation": spin, "scale": collapse}})
    # Labels, bottom-left, one per idea.
    for i, (t, s_) in enumerate([(14.0, "PATH MORPH · ANY OUTLINE TO ANY OUTLINE"), (14.5, "INSTANCER · 24 COPIES · STAGGER 1F"), (15.0, "STROKE · GRADIENT ALONG LENGTH · FEATHER")]):
        L += label(f"sh-lab{i}", s_, M, H - M - 150 + i * 30, PAPER, t, opacity=keys(k(t, 0.7), k(15.5, 0, ease="snap", dur="4f", anchor="end")))
    # The dot, alone in the silent beat before the drop.
    L.append({"id": "s4-dot", "kind": "shape", "shape": "circle", "size": 44, "fill": RED, "time": window(15.5),
              "transform": {"position": [cx, cy], "scale": keys(k(15.5, 1, **{"from": 0, "ease": "pop", "anchor": "start"}), k(16.0, 0.0, ease="slam", dur="5f", anchor="end"))}})
    L += hud(sid, start, end, "dark", (4, "SHAPE"))
    return {"id": sid, "start": start, "end": end, "background": INK, "layers": L,
            "motion_blur": {"shutter": 180, "samples": 16}, "transition": {"type": "push", "direction": "left", "dur": "8f"}}



def spec_list(prefix, items, x, y0, color, step=34):
    """Numbered spec lines (screen space): red number, label, value."""
    out = []
    for i, (t, name, val) in enumerate(items):
        y = y0 + i * step
        out.append(text(f"{prefix}{i}-n", f"{i + 1:02d}", x, y, 15, F_MONO, RED, "left", 200, space="screen",
                        opacity=keys(k(t, 1, **{"from": 0, "ease": "snap", "dur": "3f", "anchor": "start"}))))
        out.append(text(f"{prefix}{i}-a", name, x + 48, y, 15, F_MONO, color, "left", 220, space="screen",
                        animators=[{"at": t, "unit": "glyph", "stagger": "0.5f", "dur": "3f", "from": {"opacity": 0}}]))
        out.append(text(f"{prefix}{i}-b", val, x + 270, y, 15, F_MONO, color, "left", 220, space="screen", opacity=0.55,
                        animators=[{"at": t + f(4), "unit": "glyph", "stagger": "0.5f", "dur": "3f", "from": {"opacity": 0}}]))
    return out


def s05():
    """LIGHT: a glass lens bends the wordmark behind it; chrome rings and the red dot orbit it."""
    sid, start, end = "s05", 16.0, 21.0
    cx, cy = W / 2, H / 2 - 20
    L = []
    # Warm light behind everything.
    L.append({"id": "l-bg", "kind": "shape", "shape": "field", "size": [W + 40, H + 40], "transform": {"position": [W / 2, H / 2]},
              "softness": 1.4, "space": "screen",
              "points": [{"position": [0, 60], "color": "#30231f", "radius": 520}, {"position": [-900, 500], "color": "#0a0a09", "radius": 500},
                         {"position": [900, -500], "color": "#0a0a09", "radius": 500}, {"position": [900, 500], "color": "#0c0c0b", "radius": 400},
                         {"position": [-900, -500], "color": "#0c0c0b", "radius": 400}]})
    # The word behind the lens (3D, far back so the camera's orbit gives parallax).
    L.append(text("l-word", "LAHZA", cx, cy - 40, 420, F_BLACK, PAPER, tracking=-30, three_d=True,
                  opacity=keys(k(16.0, 1, **{"from": 0, "ease": "glide", "dur": "10f", "anchor": "start"}))))
    L[-1]["transform"]["z"] = 760
    # Floor.
    L.append({"id": "l-floor", "kind": "mesh", "mesh": {"type": "plane", "size": [24000, 24000]},
              "material": {"preset": "shadow", "color": "#000000", "opacity": 0.85},
              "transform": {"position": [cx, cy + 330], "z": 0, "rotation_x": 90}})
    # Rings: chrome segments and a thin red line, flying in on the drop.
    ring_in = keys(k(16.0, 1, **{"from": 2.4, "ease": "settle", "dur": "26f", "anchor": "start"}))
    L.append({"id": "l-ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 330, "tube": 9}, "segments": 40, "gap": 3.2,
              "material": {"preset": "chrome"},
              "transform": {"position": [cx, cy], "rotation_x": 76, "rotation_y": -8, "scale": ring_in,
                            "rotation": keys(k(16.0, 0), k(21.0, 70, ease="linear"))}})
    L.append({"id": "l-ring2", "kind": "mesh", "mesh": {"type": "torus", "radius": 400, "tube": 3.5},
              "material": {"preset": "coated", "color": RED, "roughness": 0.25},
              "transform": {"position": [cx, cy], "rotation_x": 70, "rotation_y": 10,
                            "scale": keys(k(16.0 + f(3), 1, **{"from": 2.8, "ease": "settle", "dur": "28f", "anchor": "start"})),
                            "rotation": keys(k(16.0, 40), k(21.0, -20, ease="linear"))}})
    # The red dot as an object: orbits the lens on a tilted ellipse.
    L.append({"id": "l-dot", "kind": "mesh", "mesh": {"type": "sphere", "radius": 38},
              "material": {"preset": "coated", "color": RED, "roughness": 0.18},
              "transform": {"position": {"base": [cx, cy], "add": [{"type": "sine", "hz": 0.22, "amp": [400, 52], "phase": 0.0}]},
                            "z": {"base": 0, "add": [{"type": "sine", "hz": 0.22, "amp": 300, "phase": 0.25}]},
                            "scale": keys(k(16.0 + f(6), 1, **{"from": 0, "ease": "pop", "anchor": "start"}))}})
    # The lens.
    L.append({"id": "l-glass", "kind": "mesh", "mesh": {"type": "sphere", "radius": 215},
              "material": {"preset": "glass", "ior": 1.5, "thickness": 320, "roughness": 0.0, "color": "#f6f8ff"},
              "transform": {"position": [cx, cy], "scale": keys(k(16.0, 1, **{"from": 0.4, "ease": "land", "anchor": "start"}))}})
    # Spec list, screen space, on the beat.
    L += spec_list("l-spec", [(16.5, "GLASS", "IOR 1.50 · REFRACTION"), (17.0, "CHROME", "PBR · METALLIC 1.0"),
                              (17.5, "SHADOWS", "SOFT · PCF"), (18.0, "DEPTH OF FIELD", "APERTURE 0.9"),
                              (18.5, "CAMERA", "ORBIT · DOLLY")], M, H - M - 230, PAPER)
    L += hud(sid, start, end, "dark", (5, "LIGHT"))
    camera = {"fov": 39.6,
              "orbit": keys(k(16.0, [-26, 7]), k(21.0, [20, 13], ease="linear")),
              "dolly": keys(k(16.0, -60), k(20.0, 120, ease="linear"), k(21.0, 620, ease="zoom", dur="16f", anchor="end")),
              "aperture": 0.9, "focus": "l-glass"}
    lights = [{"type": "directional", "from": [-0.7, -1.0, -0.8], "color": "#fff1e2", "intensity": 1.5, "shadow": True, "softness": 0.55},
              {"type": "directional", "from": [1.0, -0.3, 1.0], "color": "#cfe0ff", "intensity": 0.75},
              {"type": "point", "position": [cx + 500, cy - 300, -300], "color": "#ffd9d4", "intensity": 0.5},
              {"type": "ambient", "intensity": 0.06}]
    return {"id": sid, "start": start, "end": end, "background": "#0b0b0a", "layers": L, "camera": camera, "lights": lights,
            "environment": {"preset": "studio", "intensity": 0.38}}


def s06():
    """FORM: Bauhaus primitives land on the beat and cast soft shadows on paper."""
    sid, start, end = "s06", 21.0, 26.0
    cx, floor = W / 2, H / 2 + 170
    L = []
    L.append({"id": "f-catcher", "kind": "mesh", "mesh": {"type": "plane", "size": [9000, 9000]},
              "material": {"preset": "shadow", "color": "#2a2620", "opacity": 0.55},
              "transform": {"position": [cx, floor], "rotation_x": 90}})

    def drop(t, rest, height=1700):
        return keys(k(t, rest, **{"from": [rest[0], rest[1] - height], "ease": "bounce", "anchor": "land"}))

    def falls(n):
        """Exists from 0.45 s before landing on beat n (its shadow too)."""
        return window(bt(n) - 0.45)

    # Box (extrude family: a bevelled box), ink.
    L.append({"id": "f-box", "kind": "mesh", "time": falls(43), "mesh": {"type": "box", "size": [230, 230], "depth": 230, "bevel": 12},
              "material": {"preset": "plastic", "color": INK2, "roughness": 0.55},
              "transform": {"position": drop(beat(43), [cx - 560, floor - 115]), "z": 60, "rotation_y": -28}})
    # The dot, now a big red ball.
    L.append({"id": "f-ball", "kind": "mesh", "time": falls(44), "mesh": {"type": "sphere", "radius": 128},
              "material": {"preset": "coated", "color": RED, "roughness": 0.25},
              "transform": {"position": drop(beat(44), [cx - 230, floor - 128]), "z": -60}})
    # A revolved vessel, paper white (revolve from a profile path).
    L.append({"id": "f-vase", "kind": "mesh", "time": falls(45),
              "mesh": {"type": "revolve", "profile": "M 0 -150 L 46 -150 C 40 -110 34 -80 64 -30 C 100 30 104 90 82 150 L 0 150"},
              "material": {"preset": "glossy", "color": "#f7f4ee", "roughness": 0.3},
              "transform": {"position": drop(beat(45), [cx + 90, floor - 150]), "z": 80}})
    # A standing ring, ink, split into two colours.
    L.append({"id": "f-ring", "kind": "mesh", "time": falls(46), "mesh": {"type": "torus", "radius": 112, "tube": 36}, "segments": 2, "spans": [3, 1],
              "colors": [INK2, RED], "material": {"preset": "plastic", "roughness": 0.45},
              "transform": {"position": drop(beat(46), [cx + 420, floor - 148]), "z": -40, "rotation_y": 24}})
    # An extruded viewfinder corner (from the wordmark), lying flat in front.
    corner = "M 0 0 L 160 0 L 160 46 L 46 46 L 46 160 L 0 160 Z"
    L.append({"id": "f-corner", "kind": "mesh", "time": falls(47), "mesh": {"type": "extrude", "d": corner, "depth": 40, "bevel": 6},
              "material": {"preset": "coated", "color": RED, "roughness": 0.3},
              "transform": {"position": drop(beat(47), [cx + 650, floor - 20]), "z": -260, "rotation_x": 90, "rotation": 25}})
    # A swept tube drawing a line between them on the floor, its head a red ball.
    route = "M -760 160 C -520 300 -380 -40 -160 120 S 120 320 300 100 S 640 -60 820 140"
    L.append({"id": "f-sweep", "kind": "mesh", "mesh": {"type": "sweep", "d": route, "radius": 7, "head": 15,
                                                         "trim": keys(k(beat(48), [0, 0]), k(25.2, [0, 1], ease="glide", dur="44f", anchor="start"))},
              "material": {"preset": "plastic", "color": INK2, "roughness": 0.4},
              "transform": {"position": [cx, floor - 8], "z": -120, "rotation_x": 90}})
    L += spec_list("f-spec", [(21.0, "BOX", "BEVEL 12 · PLASTIC"), (21.5, "SPHERE", "COATED · CLEARCOAT"),
                              (22.0, "REVOLVE", "FROM A PROFILE PATH"), (22.5, "TORUS", "2 SEGMENTS · SPANS 3:1"),
                              (23.0, "EXTRUDE", "ANY OUTLINE · BEVEL 6"), (23.5, "SWEEP", "TRIM 0 → 1 · HEAD")], M, M + 60, INK)
    L.append(text("f-note", "SPRINGS: BOUNCE, ANCHORED TO THE BEAT", W - M, M + 60, 15, F_MONO, INK, "right", 220, space="screen", opacity=0.55,
                  animators=[{"at": 21.0, "unit": "glyph", "stagger": "0.4f", "dur": "3f", "from": {"opacity": 0}}]))
    L += hud(sid, start, end, "light", (6, "FORM"))
    camera = {"fov": 30, "orbit": keys(k(21.0, [-14, 16]), k(26.0, [12, 22], ease="linear")),
              "dolly": keys(k(21.0, -220), k(26.0, 40, ease="linear"))}
    lights = [{"type": "directional", "from": [-0.8, -1.1, -0.6], "color": "#fff6ea", "intensity": 1.5, "shadow": True, "softness": 0.6},
              {"type": "directional", "from": [1.0, -0.4, 0.6], "color": "#e6eeff", "intensity": 0.6},
              {"type": "ambient", "intensity": 0.12}]
    return {"id": sid, "start": start, "end": end, "background": PAPER, "layers": L, "camera": camera, "lights": lights,
            "environment": {"preset": "soft", "intensity": 0.8}, "transition": {"type": "zoom", "dur": "10f"}}


def s07():
    """DATA I: a native vector map; arcs leave Karachi (where the word lives) on the beat."""
    sid, start, end = "s07", 26.0, 30.0
    L = []
    karachi = [67.01, 24.86]
    cities = [("London", 27.5), ("New York", 28.0), ("Tokyo", 28.5), ("Sydney", 29.0), ("Lagos", 29.25), ("San Francisco", 29.5)]
    L.append({"id": "map", "kind": "component", "component": "map", "transform": {"position": [0, 0]},
              "params": {
                  "camera": {"keys": [{"at": 26.0, "center": [40, 18], "zoom": 0.35},
                                      {"at": 27.2, "center": karachi, "zoom": 2.4, "ease": "zoom", "dur": "34f"},
                                      {"at": 29.6, "center": [10, 22], "zoom": 0.55, "ease": "glide", "dur": "50f"}]},
                  "land": "#232321", "water": "#0d0d0c", "border": {"color": "#3a3936", "width": 1},
                  "fills": {"PAK": "#3a2a28"},
                  "label": {"size": 17, "color": PAPER, "font": F_MONO, "uppercase": True},
                  "places": [{"name": "Karachi", "color": RED, "size": 12, "at": 26.5}] + [{"name": n, "color": PAPER, "size": 6, "at": t + 0.35} for n, t in cities],
                  "arcs": [{"from": "Karachi", "to": n, "at": t, "dur": "16f", "color": RED, "width": 4, "lift": 0.25} for n, t in cities],
                  "detail": 0.35}})
    L += spec_list("m-spec", [(26.5, "MAP", "NATURAL EARTH 1:50M · NATIVE"), (27.2, "CAMERA", "ZOOM 0.35 → 2.4 → 0.55"),
                              (27.5, "ARCS", "GREAT CIRCLES · DRAWN ON THE BEAT")], M, H - M - 160, PAPER)
    L.append(text("m-title", "Karachi, to everywhere.", M, M + 70, 46, F_LIGHT, PAPER, "left", -10, space="screen",
                  animators=[{"at": 26.5, "unit": "glyph", "order": {"axis": "x", "feather": 0.3}, "stagger": "1f", "dur": "14f", "from": {"opacity": 0}}]))
    L += hud(sid, start, end, "dark", (7, "DATA"))
    return {"id": sid, "start": start, "end": end, "background": "#0d0d0c", "layers": L, "transition": {"type": "fade", "dur": "10f"}}


def loudness(path):
    """The track's RMS level per beat in dB (floored at -40), for the chart."""
    import numpy as np
    from scipy.io import wavfile
    sr, x = wavfile.read(path)
    x = x.astype(float).mean(axis=1) / 32768
    n = int(DURATION / BEAT)
    return [round(max(-40.0, 20 * math.log10(math.sqrt(float(np.mean(x[int(i * BEAT * sr):int((i + 1) * BEAT * sr)] ** 2))) + 1e-9)), 1) for i in range(n)]


def s07b():
    """DATA II: this film, counted and charted: its own soundtrack, beat by beat."""
    sid, start, end = "s07b", 30.0, 34.0
    L = []
    loud = loudness(os.path.join(HERE, "track.wav"))
    L.append(text("d-k1", "FRAMES IN THIS FILM", colx(0), 300, 15, F_MONO, PAPER, "left", 300, opacity=0.6,
                  animators=[{"at": 30.0, "unit": "glyph", "stagger": "0.4f", "dur": "3f", "from": {"opacity": 0}}]))
    L.append({"id": "d-count", "kind": "component", "component": "counter",
              "params": {"from": 0, "to": int(DURATION * FPS), "at": 30.0, "dur": "40f", "ease": "glide", "separator": ",",
                         "size": 210, "font": F_BLACK, "color": PAPER},
              "transform": {"position": [colx(0) + 300, 420]}})
    rows = [("DURATION", "51.0 S · 25 BARS · 120 BPM"), ("RENDERER", "ONE ENGINE · NO PLUG-INS"), ("SOURCE", "ONE JSON PROJECT")]
    for i, (a, b_) in enumerate(rows):
        t = 31.0 + i * 0.5
        y = 600 + i * 46
        L.append({"id": f"d-r{i}", "kind": "shape", "shape": "line", "size": [5 * COL + 4 * 24, 0], "thickness": 1, "fill": PAPER,
                  "transform": {"position": [colx(0) + (5 * COL + 4 * 24) / 2, y - 22], "opacity": 0.3},
                  "trim": keys(k(t, [0, 1], **{"from": [0, 0], "ease": "glide", "dur": "12f", "anchor": "start"}))})
        L.append(text(f"d-r{i}a", a, colx(0), y, 15, F_MONO, RED, "left", 260,
                      animators=[{"at": t, "unit": "glyph", "stagger": "0.4f", "dur": "3f", "from": {"opacity": 0}}]))
        L.append(text(f"d-r{i}b", b_, colx(2), y, 15, F_MONO, PAPER, "left", 220,
                      animators=[{"at": t + f(3), "unit": "glyph", "stagger": "0.4f", "dur": "3f", "from": {"opacity": 0}}]))
    # The soundtrack as a line chart, drawn on through the breakdown.
    L.append(text("d-k2", "THE SOUNDTRACK · LOUDNESS PER BEAT · DB", colx(6) + 24, 300, 15, F_MONO, PAPER, "left", 300, opacity=0.6,
                  animators=[{"at": 30.5, "unit": "glyph", "stagger": "0.4f", "dur": "3f", "from": {"opacity": 0}}]))
    L.append({"id": "d-chart", "kind": "component", "component": "chart",
              "params": {"type": "line", "data": loud, "size": [6 * COL + 5 * 24 - 160, 420], "min": -40, "max": -8,
                         "at": 30.5, "dur": "2.6s", "color": RED, "axis_color": "#f2eee640", "text_color": PAPER, "value_format": " dB"},
              "transform": {"position": [colx(6) + 24 + (6 * COL + 5 * 24 - 160) / 2, 560]}})
    L += hud(sid, start, end, "dark", (7, "DATA"))
    return {"id": sid, "start": start, "end": end, "background": INK, "layers": L, "transition": {"type": "slide", "direction": "left", "dur": "10f"}}


def s08():
    """SYNC: a field of 1,100 dots; a wave leaves the centre on every beat."""
    sid, start, end = "s08", 34.0, 40.0
    L = []
    centres = [[0, 0], [-520, -200], [520, 200], [0, 0], [-520, 200], [520, -200]]
    fields = [{"type": "noise", "size": 260, "speed": 0.4, "seed": 3, "opacity": 0.35, "amount": 0.8}]
    for i in range(12):
        t = 34.0 + i * 0.5
        fields.append({"type": "wave", "center": centres[(i // 2) % len(centres)], "at": t, "speed": 1500, "wavelength": 260,
                       "scale": 1.8, "color": RED, "opacity": 0.6})
    L.append({"id": "y-field", "kind": "instancer", "transform": {"position": [W / 2, H / 2]},
              "layout": {"type": "grid", "columns": 45, "rows": 25, "spacing": [44, 44], "hex": True},
              "stagger": {"total": "16f", "order": "center"},
              "fields": fields,
              "layers": [{"id": "y-dot", "kind": "shape", "shape": "circle", "size": 6, "fill": PAPER,
                          "transform": {"opacity": keys(k(34.0, 0.32, **{"from": 0, "ease": "glide", "dur": "10f", "anchor": "start"})),
                                        "scale": keys(k(34.0, 1, **{"from": 0, "ease": "pop", "anchor": "start"}))}}]})
    # One word per beat, slammed in, centred as a line.
    words = ["Locked", "to", "the", "beat."]
    full = "Locked to the beat."
    size = 150
    total = text_width(full, F_BLACK, size, -30)
    x = W / 2 - total / 2
    for i, w_ in enumerate(words):
        piece = (" " if i else "") + w_
        wx = x + text_width(" ".join(words[:i]) + (" " if i else ""), F_BLACK, size, -30)
        L.append(text(f"y-w{i}", w_, wx, H / 2 - 10, size, F_BLACK, PAPER if i < 3 else RED, "left", -30,
                      animators=[{"at": beat(69 + i), "unit": "glyph", "stagger": "0.5f", "dur": "6f", "ease": "slam", "anchor": "end",
                                  "from": {"scale": 1.8, "opacity": 0}}]))
    L.append(text("y-sub", "MARKERS FROM THE MUSIC · KEYS ANCHORED TO THEM · CHECKED ON THE FRAME GRID", W / 2, H / 2 + 110, 15, F_MONO, PAPER,
                  tracking=260, opacity=0.6, animators=[{"at": 36.0, "unit": "glyph", "stagger": "0.25f", "dur": "3f", "from": {"opacity": 0}}]))
    L += hud(sid, start, end, "dark", (8, "SYNC"))
    return {"id": sid, "start": start, "end": end, "background": INK, "layers": L, "transition": {"type": "zoom", "dur": "8f"}}


MONTAGE = [  # word, bg, ink, transition, caption
    ("WHIP", RED, PAPER, {"type": "whip", "direction": "left", "dur": "8f"}, "TRANSITION · WHIP · 8F"),
    ("PUSH", INK, PAPER, {"type": "push", "direction": "up", "dur": "8f"}, "TRANSITION · PUSH · 8F"),
    ("SLIDE", PAPER, INK, {"type": "slide", "direction": "left", "dur": "8f"}, "TRANSITION · SLIDE · 8F"),
    ("ZOOM", RED, PAPER, {"type": "zoom", "dur": "8f"}, "TRANSITION · ZOOM · 8F"),
    ("WIPE", INK, PAPER, {"type": "wipe", "direction": "right", "dur": "8f"}, "TRANSITION · WIPE · 8F"),
    ("BLUR", PAPER, INK, {"type": "fade", "dur": "4f"}, "EFFECT · BLUR 60 → 0"),
    ("GLITCH", INK, PAPER, {"type": "whip", "direction": "down", "dur": "6f"}, "EFFECT · RGB SPLIT · DISPLACE"),
]


def montage():
    out = []
    for i, (word, bg, ink, tr, cap) in enumerate(MONTAGE):
        sid = f"s09{chr(97 + i)}"
        t0 = 40.0 + i * 0.5
        L = []
        effects = []
        if word == "BLUR":
            effects = [{"type": "blur", "radius": keys(k(t0, 0, **{"from": 60, "ease": "glide", "dur": "10f", "anchor": "start"}))}]
        if word == "GLITCH":
            effects = [{"type": "rgb_split", "amount": keys(k(t0, 4, **{"from": 46, "ease": "snap", "dur": "8f", "anchor": "start"})), "angle": 0},
                       {"type": "displace", "amount": keys(k(t0, 0, **{"from": 40, "ease": "snap", "dur": "8f", "anchor": "start"})), "scale": 30, "speed": 6}]
        L.append(text(f"{sid}-w", word, W / 2, H / 2 - 30, 330, F_BLACK, ink, tracking=-35,
                      animators=[{"at": t0, "unit": "glyph", "order": "center", "stagger": "0.5f", "dur": "6f", "ease": "snap",
                                  "from": {"scale": 1.25, "opacity": 0}}],
                      effects=effects or None))
        if not effects:
            del L[-1]["effects"]
        L.append(text(f"{sid}-c", cap, W / 2, H / 2 + 170, 17, F_MONO, ink, tracking=320, opacity=0.7,
                      animators=[{"at": t0 + f(2), "unit": "glyph", "stagger": "0.2f", "dur": "2f", "from": {"opacity": 0}}]))
        L += hud(sid, t0, t0 + 0.5, "light" if bg == PAPER else "dark", (9, "MOTION"))
        out.append({"id": sid, "start": beat(81 + i), "end": beat(82 + i), "background": bg, "layers": L, "transition": tr,
                    "motion_blur": {"shutter": 180, "samples": 16}})
    # The silent beat: only the dot.
    L = [{"id": "s09h-dot", "kind": "shape", "shape": "circle", "size": 44, "fill": RED,
          "transform": {"position": [W / 2, H / 2], "scale": keys(k(43.5, 1, **{"from": 0, "ease": "pop", "anchor": "start"}))}}]
    L += hud("s09h", 43.5, 44.0, "dark", (9, "MOTION"))
    out.append({"id": "s09h", "start": beat(88), "end": beat(89), "background": INK, "layers": L})
    return out


def s10():
    """END: the Urdu wordmark assembles: the dot lands, the viewfinder closes, the letters draw on."""
    sid, start, end = "s10", 44.0, DURATION
    cx, cy = W / 2, H / 2 - 60
    letters, brackets, (dx, dy, dr) = wordmark(1000)
    L = []
    # Letters: stroke draws on (feathered head), then the fill arrives and the stroke goes.
    L.append({"id": "e-ink", "kind": "shape", "shape": "path", "d": letters,
              "fill": keys(k(45.4, INK2, **{"from": "#1b1b1a00", "ease": "glide", "dur": "16f", "anchor": "start"})),
              "stroke": {"color": keys(k(45.9, "#1b1b1a00", **{"from": INK2, "ease": "glide", "dur": "10f", "anchor": "start"})), "width": 2.5, "feather": 140},
              "trim": keys(k(44.15, [0, 1], **{"from": [0, 0], "ease": "gesture", "dur": "36f", "anchor": "start"})),
              "transform": {"position": [cx, cy]}})
    # Brackets: the viewfinder closes around the dot, one bracket per frame.
    corners = [(-1, -1), (1, -1), (-1, 1), (1, 1)]
    for i, d in enumerate(brackets):
        ox, oy = corners[i] if i < 4 else (0, 0)
        L.append({"id": f"e-br{i}", "kind": "shape", "shape": "path", "d": d, "fill": INK2,
                  "transform": {"position": keys(k(44.0 + f(i), [cx, cy], **{"from": [cx + ox * 70, cy + oy * 70], "ease": "glide", "dur": "14f", "anchor": "start"})),
                                "opacity": keys(k(44.0 + f(i), 1, **{"from": 0, "ease": "snap", "dur": "4f", "anchor": "start"}))}})
    # The dot travels from the centre of the frame (where the last beat left it) into the mark.
    L.append({"id": "e-dot", "kind": "shape", "shape": "circle", "size": 2 * dr, "fill": RED,
              "transform": {"position": keys(k(44.0, [cx + dx, cy + dy], **{"from": [W / 2, H / 2], "ease": "settle", "dur": "18f", "anchor": "start"})),
                            "scale": keys(k(44.0, 1, **{"from": 44 / (2 * dr), "ease": "settle", "dur": "18f", "anchor": "start"}),
                                          k(48.0, 1.08, ease="snap", dur="2f", anchor="start"), k(48.0 + f(2), 1, ease="pop", anchor="start"))}})
    # Name, line, tagline.
    L.append(text("e-name", "LAHZA", cx, cy + 250, 30, F_SEMI, INK, tracking=1100,
                  animators=[{"at": 46.0, "unit": "glyph", "order": "center", "stagger": "1f", "dur": "12f", "ease": "glide", "from": {"opacity": 0, "y": 10}}]))
    L.append(text("e-tag", "Every moment, engineered.", cx, cy + 320, 44, F_LIGHT, INK, tracking=-10,
                  animators=[{"at": 46.5, "unit": "glyph", "order": {"axis": "x", "feather": 0.35}, "stagger": "1f", "dur": "16f", "from": {"opacity": 0}}]))
    L.append(text("e-meta", "MOTION ENGINE  ·  RUST  ·  GPU + CPU  ·  THIS FILM WAS RENDERED BY IT", cx, cy + 400, 15, F_MONO, INK, tracking=260, opacity=0.6,
                  animators=[{"at": 47.5, "unit": "glyph", "stagger": "0.2f", "dur": "3f", "from": {"opacity": 0}}]))
    L += hud(sid, start, end, "light", (10, "LAHZA"))
    return {"id": sid, "start": start, "end": end, "background": PAPER, "layers": L,
            "camera": {"zoom": keys(k(44.0, 1.0), k(51.0, 1.035, ease="linear"))}}

SCENES = [s01, s02, s03, s04, s05, s06, s07, s07b, s08, montage, s10]


# ------------------------------------------------------------------ project

def build(only=None):
    if os.path.exists(OUT):
        shutil.rmtree(OUT)
    os.makedirs(os.path.join(OUT, "scenes"))
    os.makedirs(os.path.join(OUT, "markers"))
    os.makedirs(os.path.join(OUT, "assets", "fonts"))
    for fn, d in SYSTEM_FONTS.items():
        shutil.copy(d + fn, os.path.join(OUT, "assets", "fonts", fn))
    shutil.copy("/usr/share/doc/fonts-inter/copyright", os.path.join(OUT, "assets", "fonts", "LICENSE-Inter.txt"))
    shutil.copy("/usr/share/doc/fonts-jetbrains-mono/copyright", os.path.join(OUT, "assets", "fonts", "LICENSE-JetBrainsMono.txt"))
    shutil.copy(os.path.join(HERE, "track.wav"), os.path.join(OUT, "assets", "track.wav"))
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
        lic = "SIL Open Font License 1.1 (assets/fonts/LICENSE-Inter.txt)" if "Inter" in fn else "SIL Open Font License 1.1 (assets/fonts/LICENSE-JetBrainsMono.txt)"
        assets.append({"path": f"assets/fonts/{fn}", "source": "Debian fonts-inter / fonts-jetbrains-mono", "licence": lic})
    project = {
        "version": 1,
        "settings": {"width": W, "height": H, "fps": FPS, "duration": DURATION, "background": INK},
        "audio": {"music": {"path": "assets/track.wav", "at": 0.0}},
        "assets": assets,
        "scenes": [s["id"] for s in scenes],
    }
    json.dump(project, open(os.path.join(OUT, "project.json"), "w"), indent=1)
    print("wrote", OUT, [s["id"] for s in scenes])


if __name__ == "__main__":
    build(sys.argv[1:] or None)
