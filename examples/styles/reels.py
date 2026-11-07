"""Style reels: one 10 s vertical reel per look that depends on how light
behaves, the looks a browser engine only fakes.

    python3 reels.py OUT_DIR [reel ...] [--bin path/to/lahza-motion]
                     [--sheets] [--render] [--spp N] [--scale F]

Builds one project per reel in OUT_DIR (1080x1920, 24 fps, 10 s,
path-traced exports), a raster contact sheet of each with --sheets, and
renders them with --render (path traced; --spp and --scale for drafts).
Reels: product, glass, type, haze, soft, clay, lens, chrome, screen.

`screen` needs a screen recording at OUT_DIR/screen-feed.mp4 (feed.js
makes one with the bundled Chromium).
"""
import json
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
W, H, FPS, DUR = 1080, 1920, 24, 10
CX, CY = W / 2, H / 2
FONTS = "/usr/share/fonts/opentype/inter"


# ----------------------------------------------------------------- helpers

def keys(*pairs, ease="glide"):
    """Keys from (time, value[, ease]) tuples."""
    out = []
    for p in pairs:
        k = {"at": p[0], "v": p[1]}
        e = p[2] if len(p) > 2 else ease
        if e:
            k["ease"] = e
        out.append(k)
    return {"keys": out}


def lin(*pairs):
    return keys(*pairs, ease="linear")


def label(text, sub, at=0.4, color="#ffffff"):
    """The reel's caption: a small kicker and a line, screen-space."""
    return [
        {"id": "kicker", "kind": "text", "text": text, "font": "fonts/Inter-Bold.otf", "size": 30, "tracking": 220,
         "color": color, "space": "screen", "transform": {"position": [CX, 1700], "opacity": keys((at, 0), (at + 0.5, 0.9))}},
        {"id": "sub", "kind": "text", "text": sub, "font": "fonts/Inter-Medium.otf", "size": 26, "color": color + "b0",
         "space": "screen", "transform": {"position": [CX, 1750], "opacity": keys((at + 0.3, 0), (at + 0.8, 0.8))}},
    ]


def glyph_path(text, font="Inter-Black.otf", size=400, tracking=0.0):
    """The outline of `text` as one SVG path (y down, centred on 0, 0),
    for extruded type."""
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.pens.transformPen import TransformPen
    from fontTools.ttLib import TTFont

    f = TTFont(os.path.join(FONTS, font))
    gs = f.getGlyphSet()
    cmap = f.getBestCmap()
    upm = f["head"].unitsPerEm
    s = size / upm
    x = 0.0
    pieces = []
    for ch in text:
        name = cmap[ord(ch)]
        pieces.append((name, x))
        x += gs[name].width + tracking * upm
    width = (x - tracking * upm) * s
    cap = f["OS/2"].sCapHeight * s
    pen = SVGPathPen(gs)
    for name, gx in pieces:
        # Font units (y up) to px (y down), centred.
        t = TransformPen(pen, (s, 0, 0, -s, gx * s - width / 2, cap / 2))
        gs[name].draw(t)
    return pen.getCommands()


# ------------------------------------------------------------------ reels

def product():
    """Perfume hero shot: amber glass, gold cap, black mirror floor, softboxes."""
    floor_y = 1380
    bottle = ("M 0 -250 L 150 -250 Q 172 -250 172 -228 L 172 228 Q 172 250 150 250 L 0 250 Z")
    liquid = ("M 0 -60 L 146 -60 L 146 222 Q 146 236 132 236 L 0 236 Z")
    return {
        "background": "#08080a",
        "look": {"tone": "agx", "exposure": 0.2, "bloom": {"threshold": 0.9, "radius": 30, "intensity": 0.5}},
        "camera": {"fov": 30, "orbit": keys((0, [-32, 10]), (10, [26, 3]), ease="linear"),
                   "dolly": keys((0, -500), (10, 350), ease="linear"), "aperture": 1.2,
                   "position": [CX, 1020]},
        "environment": {"preset": "night", "intensity": 0.6},
        "lights": [
            {"type": "area", "position": [-700, 500, -500], "aim": [CX, 1080, 0], "size": [700, 1600], "intensity": 1.8},
            {"type": "area", "position": [1700, 700, 700], "aim": [CX, 1080, 0], "size": [160, 1500], "intensity": 3.0, "color": "#ffd9a0"},
            {"type": "area", "position": [CX, 200, -200], "aim": [CX, 1080, 0], "size": [500, 300], "shape": "disc", "intensity": 0.8},
            {"type": "area", "position": [CX + 300, 300, 1200], "aim": [CX, 700, 2200], "size": [900, 900], "intensity": 1.2, "color": "#ffcf96"},
        ],
        "layers": [
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [6000, 6000]},
             "material": {"color": "#050506", "roughness": 0.12, "specular": 0.6},
             "transform": {"position": [CX, floor_y], "rotation_x": 90}},
            # A lit backdrop far behind: the gradient the glass bends.
            {"id": "backdrop", "kind": "mesh", "mesh": {"type": "plane", "size": [7000, 5000]},
             "material": {"preset": "matte", "color": "#3a3029"},
             "transform": {"position": [CX, 0], "z": 2200}},
            {"id": "liquid", "kind": "mesh", "mesh": {"type": "revolve", "profile": liquid},
             "material": {"preset": "liquid", "color": "#ffc46b", "thickness": 260},
             "transform": {"position": [CX, floor_y - 250]}},
            {"id": "bottle", "kind": "mesh", "mesh": {"type": "revolve", "profile": bottle},
             "material": {"preset": "glass", "color": "#fff4e0", "thickness": 60, "roughness": 0.02},
             "transform": {"position": [CX, floor_y - 250]}},
            {"id": "neck", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 48, "height": 50},
             "material": {"preset": "metal", "color": "#d6aa52", "roughness": 0.3},
             "transform": {"position": [CX, floor_y - 525]}},
            {"id": "cap", "kind": "mesh", "mesh": {"type": "box", "size": [190, 170], "depth": 190, "bevel": 18},
             "material": {"preset": "metal", "color": "#e2b45c", "roughness": 0.18},
             "transform": {"position": [CX, floor_y - 630]}},
            {"id": "title", "kind": "text", "text": "NOIR", "font": "fonts/Inter-Thin.otf", "size": 170, "tracking": 400,
             "color": "#f3e6cf", "space": "screen",
             "transform": {"position": [CX + 40, 330], "opacity": keys((1.2, 0), (2.4, 1))},
             "animators": [{"at": 1.2, "from": {"opacity": 0, "y": 30}, "stagger": "3f", "dur": "18f"}]},
            {"id": "line", "kind": "text", "text": "EAU DE PARFUM", "font": "fonts/Inter-Medium.otf", "size": 30, "tracking": 500,
             "color": "#cdb894", "space": "screen", "transform": {"position": [CX + 15, 450], "opacity": keys((2.4, 0), (3.2, 1))}},
            *label("PRODUCT LIGHT", "softboxes, a mirror floor, amber glass: path traced"),
        ],
    }


def glass():
    """Liquid glass: clear, dispersive shapes sliding over live, coloured type."""
    return {
        "background": "#101018",
        "look": {"tone": "agx"},
        "camera": {"fov": 34, "orbit": lin((0, [-6, 2]), (10, [6, -2]))},
        "environment": {"preset": "studio", "intensity": 1.0},
        "lights": [{"type": "area", "position": [CX - 300, -200, -900], "size": [900, 500], "intensity": 1.4},
                   {"type": "area", "position": [1400, 1500, -300], "size": [300, 900], "intensity": 0.8, "color": "#cfe0ff"}],
        "layers": [
            {"id": "wash", "kind": "shape", "shape": "field", "three_d": True, "size": [1700, 2800],
             "transform": {"position": [CX, CY], "z": 700}, "softness": 1.2, "warp": 80,
             "points": [
                 {"position": {"base": [-380, -700], "add": [{"type": "wiggle", "hz": 0.15, "amp": [220, 160]}]}, "color": "#ff4f8b", "radius": 520},
                 {"position": {"base": [400, -200], "add": [{"type": "wiggle", "hz": 0.12, "amp": [200, 220], "seed": 2}]}, "color": "#ffb000", "radius": 480},
                 {"position": {"base": [-300, 500], "add": [{"type": "wiggle", "hz": 0.1, "amp": [260, 180], "seed": 3}]}, "color": "#3d5bff", "radius": 560},
                 {"position": {"base": [380, 900], "add": [{"type": "wiggle", "hz": 0.14, "amp": [200, 200], "seed": 4}]}, "color": "#00c2a8", "radius": 460}]},
            {"id": "w1", "kind": "text", "text": "LIQUID", "font": "fonts/Inter-Black.otf", "size": 250, "tracking": -20, "color": "#ffffff",
             "three_d": True, "transform": {"position": [CX, 770], "z": 260}},
            {"id": "w2", "kind": "text", "text": "GLASS", "font": "fonts/Inter-Black.otf", "size": 290, "tracking": -20, "color": "#101018",
             "three_d": True, "transform": {"position": [CX, 1060], "z": 260}},
            {"id": "pill", "kind": "mesh", "mesh": {"type": "capsule", "radius": 150, "height": 760},
             "material": {"preset": "glass", "color": "#ffffff", "ior": 1.5, "thickness": 300, "dispersion": 0.05},
             "transform": {"position": keys((0, [CX, 520]), (3.2, [CX, 1180]), (6.4, [CX, 760]), (10, [CX, 1000])),
                           "rotation": 90, "rotation_x": lin((0, -12), (10, 14))}},
            {"id": "ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 160, "tube": 62},
             "material": {"preset": "glass", "color": "#e8f2ff", "ior": 1.5, "thickness": 120, "dispersion": 0.05},
             "transform": {"position": lin((0, [300, 1480]), (10, [780, 1420])), "rotation_x": lin((0, 40), (10, 220)),
                           "rotation_y": lin((0, 10), (10, 70))}},
            {"id": "frost", "kind": "mesh", "mesh": {"type": "box", "size": [300, 300], "depth": 60, "bevel": 40},
             "material": {"preset": "glass", "color": "#ffffff", "roughness": 0.18, "thickness": 60},
             "transform": {"position": lin((0, [820, 330]), (10, [700, 360])), "rotation": lin((0, -12), (10, 18)),
                           "rotation_y": lin((0, -20), (10, 25))}},
            *label("LIQUID GLASS", "real refraction and dispersion through live type"),
        ],
    }


def type_():
    """Type inside the 3D scene: rings pass through the letters."""
    rings = [(360, 24, "#ff5a1f", 72, 0), (430, 18, "#e9edf5", 64, 60), (300, 16, "#3d6bff", 80, 120)]
    return {
        "background": "#07080f",
        "look": {"tone": "agx", "bloom": {"threshold": 0.95, "radius": 26, "intensity": 0.4}},
        "camera": {"fov": 34, "orbit": keys((0, [-24, 14]), (10, [18, -6]), ease="linear"), "aperture": 0.8},
        "environment": {"preset": "studio", "intensity": 0.9},
        "lights": [{"type": "area", "position": [-200, 0, -900], "size": [800, 600], "intensity": 1.3},
                   {"type": "area", "position": [1500, 1600, 500], "size": [300, 1000], "intensity": 1.5, "color": "#9fc0ff"}],
        "layers": [
            {"id": "word", "kind": "text", "text": "ORBIT", "font": "fonts/Inter-Black.otf", "size": 300, "tracking": -30,
             "color": "#ffffff", "three_d": True, "transform": {"position": [CX, CY]},
             "animators": [{"at": 0.2, "from": {"opacity": 0, "y": 60}, "stagger": "2f", "dur": "14f"}]},
            *[{"id": f"ring{i}", "kind": "mesh", "mesh": {"type": "torus", "radius": r, "tube": t},
               "material": {"preset": "coated", "color": c, "roughness": 0.12} if i != 1 else {"preset": "chrome"},
               "transform": {"position": [CX, CY], "rotation_x": rx,
                             "rotation_y": lin((0, ph), (10, ph + 140 * (1 if i % 2 else -1))),
                             "scale": keys((0.4 + 0.25 * i, 0), (1.1 + 0.25 * i, 1), ease="land")}}
              for i, (r, t, c, rx, ph) in enumerate(rings)],
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 120},
             "material": {"preset": "chrome"},
             "transform": {"position": keys((0, [CX + 260, CY - 520]), (10, [CX - 200, CY + 420]), ease="linear"),
                           "z": keys((0, -700), (5, 650), (10, -500), ease="glide")}},
            *label("TYPE IN 3D SPACE", "letters, rings and light share one depth buffer"),
        ],
    }


def haze():
    """Atmosphere: shafts of light through a window grid sweep across a hazy room."""
    holes = "".join(f"M {x0} {y0} H {x0 + 110} V {y0 + 170} H {x0} Z " for x0 in range(-600, 600, 200) for y0 in range(-420, 420, 260))
    window = "M -3200 -3200 H 3200 V 3200 H -3200 Z " + holes
    return {
        "background": "#020203",
        "look": {"tone": "agx", "exposure": 0.8, "bloom": {"threshold": 0.85, "radius": 40, "intensity": 0.5}},
        "camera": {"fov": 36, "orbit": lin((0, [6, -4]), (10, [-6, -2])), "dolly": lin((0, -150), (10, 200))},
        "environment": {"preset": "none"},
        "lights": [{"type": "area", "position": lin((0, [-2300, -500, 500]), (10, [-2300, 200, -200])), "aim": [CX + 200, 1500, 0],
                    "size": [160, 160], "shape": "disc", "intensity": 16.0, "color": "#ffd2a0"},
                   {"type": "area", "position": [1600, 300, -800], "aim": [CX, 1300, 0], "size": [300, 300], "intensity": 0.25, "color": "#4a7dff",
                    "shadow": False}],
        "layers": [
            {"id": "air", "kind": "mesh", "mesh": {"type": "box", "size": [7000, 5000], "depth": 6000},
             "material": {"volume": {"density": 0.02, "color": "#ffffff", "anisotropy": 0.6}},
             "transform": {"position": [CX, CY], "z": 1000}},
            # The window wall stands off frame to the left, facing the light.
            {"id": "window", "kind": "mesh", "mesh": {"type": "extrude", "d": window, "depth": 40},
             "material": {"preset": "matte", "color": "#111114"},
             "transform": {"position": [-420, 520], "z": 200, "rotation_y": 90, "rotation": lin((0, -6), (10, 6))}},
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [6000, 6000]},
             "material": {"color": "#1a1a1e", "roughness": 0.35},
             "transform": {"position": [CX, 1560], "rotation_x": 90}},
            {"id": "figure", "kind": "mesh", "mesh": {"type": "capsule", "radius": 70, "height": 420},
             "material": {"preset": "matte", "color": "#2a2a2e"}, "transform": {"position": [CX + 160, 1350], "z": -100}},
            {"id": "neon", "kind": "text", "text": "AFTER HOURS", "font": "fonts/Inter-Bold.otf", "size": 92, "tracking": 120,
             "color": "#ff3d6e", "space": "screen", "transform": {"position": [CX, 360], "opacity": keys((1.5, 0), (2.5, 1))},
             "effects": [{"type": "glow", "radius": 30, "intensity": 1.2, "color": "#ff3d6e"}]},
            *label("ATMOSPHERE", "light shafts traced through real haze"),
        ],
    }


def soft():
    """Soft matter: jelly cubes drop and squash, a marble ball rolls."""
    floor = 1420

    def cube(i, x, color, t0):
        land = t0 + 0.55
        return {"id": f"jelly{i}", "kind": "mesh", "mesh": {"type": "box", "size": [250, 250], "depth": 250, "bevel": 46},
                "material": {"color": color, "roughness": 0.12, "clearcoat": 1, "subsurface": 1,
                             "subsurface_radius": [1, 0.45, 0.3], "subsurface_scale": 60},
                "transform": {"position": keys((t0, [x, -400]), (land, [x, floor - 125], "in"), (land + 0.25, [x, floor - 100], "out"),
                                               (land + 0.6, [x, floor - 125])),
                              "scale": keys((land - 0.01, [0.9, 1.12]), (land + 0.12, [1.25, 0.72], "out"), (land + 0.4, [0.94, 1.08]),
                                            (land + 0.8, [1, 1], "out")),
                              "rotation_y": lin((0, 18 - i * 14), (10, 34 - i * 14))}}
    return {
        "background": "#f3cfd6",
        "look": {"tone": "agx", "exposure": 0.25},
        "camera": {"fov": 30, "orbit": lin((0, [-8, 14]), (10, [10, 10])), "aperture": 1.0, "position": [CX, 1180]},
        "environment": {"preset": "soft", "intensity": 0.9},
        "lights": [{"type": "area", "position": [-400, 200, -700], "size": [1200, 1200], "intensity": 1.4},
                   {"type": "area", "position": [1500, 700, 600], "size": [500, 900], "intensity": 1.0, "color": "#fff0e0"}],
        "layers": [
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [6000, 6000]},
             "material": {"preset": "matte", "color": "#f1c4cd"}, "transform": {"position": [CX, floor], "rotation_x": 90}},
            {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [6000, 4000]},
             "material": {"preset": "matte", "color": "#f6d7dd"}, "transform": {"position": [CX, floor - 2000], "z": 1100}},
            cube(0, CX - 290, "#ff3f7a", 0.3),
            cube(1, CX, "#8fe04a", 0.9),
            cube(2, CX + 290, "#ffb020", 1.5),
            {"id": "marble", "kind": "mesh", "mesh": {"type": "sphere", "radius": 130},
             "material": {"color": "#f4f1ec", "roughness": 0.08, "subsurface": 0.7, "subsurface_radius": [1, 0.9, 0.8], "subsurface_scale": 40,
                          "noise": {"type": "fbm", "scale": 90, "octaves": 5, "color": "#7c8494"}},
             "transform": {"position": keys((2.2, [-300, floor - 130]), (7.0, [CX - 10, floor - 130], "out")),
                           "z": -420, "rotation": keys((2.2, -300), (7.0, 0), ease="out")}},
            *label("SOFT MATTER", "light scattering inside: jelly, wax, marble", at=0.6, color="#5a2130"),
        ],
    }


def clay():
    """A clay island with a tilt-shift lens."""
    top = 1040
    trees = [(-170, -90, 1.0), (-60, 160, 0.8), (190, 140, 1.15), (-230, 110, 0.7), (120, -200, 0.9)]
    layers = [
        {"id": "sea", "kind": "mesh", "mesh": {"type": "plane", "size": [30000, 30000]},
         "material": {"color": "#59b8f0", "roughness": 0.25, "noise": {"scale": 70, "bump": 2, "drift": [20, 0, 15]}},
         "transform": {"position": [CX, top + 110], "rotation_x": 90}},
        {"id": "dirt", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 360, "height": 150, "bevel": 30},
         "material": {"preset": "matte", "color": "#b98b5e"}, "transform": {"position": [CX, top + 75]}},
        {"id": "grass", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 352, "height": 40, "bevel": 18},
         "material": {"preset": "matte", "color": "#86d16c", "sheen": 0.4}, "transform": {"position": [CX, top]}},
        {"id": "house", "kind": "mesh", "mesh": {"type": "box", "size": [150, 120], "depth": 130, "bevel": 10},
         "material": {"preset": "matte", "color": "#fff3e0"}, "transform": {"position": [CX + 30, top - 80], "z": -40}},
        {"id": "roof", "kind": "mesh", "mesh": {"type": "extrude", "d": "M -100 40 L 0 -55 L 100 40 Z", "depth": 160, "bevel": 8},
         "material": {"preset": "matte", "color": "#ff6b4a"}, "transform": {"position": [CX + 30, top - 180], "z": -40, "rotation_y": 90}},
        {"id": "mill", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 34, "top": 22, "height": 260, "bevel": 6},
         "material": {"preset": "matte", "color": "#fde7c8"}, "transform": {"position": [CX - 230, top - 130], "z": -170}},
        {"id": "rotor", "kind": "shape", "shape": "circle", "size": 1, "fill": "#00000000", "three_d": True, "transform": {"position": [CX - 230, top - 250], "z": -210, "rotation": lin((0, 0), (10, 420))}},
        *[{"id": f"sail{i}", "kind": "mesh", "parent": "rotor", "mesh": {"type": "box", "size": [36, 150], "depth": 8, "bevel": 3},
           "material": {"preset": "matte", "color": "#ffffff"}, "transform": {"position": [0, 0], "anchor": [0, 80], "rotation": i * 90}}
          for i in range(4)],
    ]
    for i, (dx, dz, s) in enumerate(trees):
        layers += [
            {"id": f"trunk{i}", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 12 * s, "height": 50 * s},
             "material": {"preset": "matte", "color": "#8a5a3b"}, "transform": {"position": [CX + dx, top - 20 - 25 * s], "z": dz}},
            {"id": f"tree{i}", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 52 * s, "top": 0, "height": 150 * s, "bevel": 8},
             "material": {"preset": "matte", "color": "#2f9e5a"}, "transform": {"position": [CX + dx, top - 45 * s - 75 * s], "z": dz}},
        ]
    for i, (x, y, z) in enumerate([(-260, 620, -100), (250, 560, 200)]):
        layers += [{"id": f"cloud{i}_{j}", "kind": "mesh", "mesh": {"type": "sphere", "radius": r},
                    "material": {"preset": "matte", "color": "#ffffff"},
                    "transform": {"position": {"base": [CX + x + ox, y + oy], "add": [{"type": "sine", "hz": 0.25, "amp": [0, 14], "phase": i}]}, "z": z}}
                   for j, (ox, oy, r) in enumerate([(0, 0, 60), (60, 10, 48), (-55, 12, 44), (25, -30, 46)])]
    return {
        "background": "#bfe6ff",
        "look": {"tone": "agx", "exposure": 0.3},
        "camera": {"fov": 16, "orbit": lin((0, [-50, 32]), (10, [10, 30])), "aperture": 1.5, "position": [CX, top - 60]},
        "environment": {"preset": "soft", "sky": "#cfeaff", "horizon": "#e8f4ff", "ground": "#9fd7a0", "intensity": 1.0},
        "lights": [{"type": "directional", "from": [-0.6, -1, -0.5], "intensity": 1.2, "color": "#fff1dc", "softness": 0.5}],
        "layers": layers + label("CLAY WORLDS", "soft bounce light and a tilt-shift lens", color="#1d4a6b"),
    }


def lens():
    """Camera-real: rack focus down a row of pieces, then a whip with real shutter blur."""
    pawn = ("M 0 -175 C 30 -175 52 -152 52 -122 C 52 -98 38 -82 22 -76 L 44 -60 L 44 -48 L 26 -44 "
            "L 40 90 C 70 100 86 116 86 134 L 86 175 L 0 175 Z")
    mats = [{"preset": "chrome"}, {"preset": "metal", "color": "#e3b04b", "roughness": 0.2},
            {"preset": "glossy", "color": "#0e0e10", "roughness": 0.15}, {"preset": "glass", "color": "#ffffff", "thickness": 120},
            {"preset": "coated", "color": "#d6262f", "roughness": 0.1}]
    pieces = []
    for i in range(9):
        z = -900 + i * 380
        x = CX - 260 + i * 70
        pieces.append({"id": f"pawn{i}", "kind": "mesh", "mesh": {"type": "revolve", "profile": pawn},
                       "material": mats[i % len(mats)], "transform": {"position": [x, 1240], "z": z}})
    return {
        "background": "#0a0a0c",
        "look": {"tone": "agx", "exposure": 0.2, "bloom": {"threshold": 0.9, "radius": 24, "intensity": 0.4}},
        "camera": {"fov": 30, "position": [CX, 1100],
                   "orbit": keys((0, [-28, 8]), (5.6, [-22, 8], "linear"), (6.2, [48, 10], "gesture"), (10, [58, 12], "out")),
                   "dolly": lin((0, 500), (10, 800)), "aperture": 2.5,
                   "focus": keys((0, -900), (2.2, -900), (3.6, 1900, "glide"), (5.0, 1900), (6.4, 400, "glide"))},
        "environment": {"preset": "studio", "intensity": 0.8},
        "lights": [{"type": "area", "position": [-500, 300, -700], "size": [900, 900], "intensity": 1.4},
                   {"type": "area", "position": [1600, 600, 1500], "size": [300, 1200], "intensity": 2.0, "color": "#ffd8b0"},
                   *[{"type": "point", "position": [200 + 120 * k, 700 + (k % 3) * 60, 2600], "intensity": 0.6, "color": c, "softness": 0.2}
                     for k, c in enumerate(["#ffb347", "#ff6b6b", "#ffd27a", "#7ab8ff", "#ffb347", "#ff9f6b"])]],
        "layers": [
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [8000, 8000]},
             "material": {"color": "#121214", "roughness": 0.18}, "transform": {"position": [CX, 1415], "rotation_x": 90}},
            # Small bright lights far behind: real bokeh discs.
            *[{"id": f"bulb{k}", "kind": "mesh", "mesh": {"type": "sphere", "radius": 14},
               "material": {"preset": "emissive", "color": c, "emissive_strength": 4},
               "transform": {"position": [80 + 140 * k, 760 + (k % 3) * 90], "z": 3200}}
              for k, c in enumerate(["#ffb347", "#ff6b6b", "#ffd27a", "#7ab8ff", "#ffb347", "#ff9f6b", "#ffe2a0", "#ff7a7a"])],
            *pieces,
            *label("CAMERA REAL", "a thin lens and a shutter: rack focus, bokeh, motion blur"),
        ],
    }


def chrome():
    """Y2K chrome: extruded chrome type in a holographic world."""
    d = glyph_path("Y2K", "Inter-Black.otf", 360, tracking=-0.02)
    return {
        "background": "#120a2a",
        "look": {"tone": "agx", "exposure": 0.3, "bloom": {"threshold": 0.9, "radius": 30, "intensity": 0.6}},
        "camera": {"fov": 32, "orbit": lin((0, [-10, 6]), (10, [10, -4]))},
        "environment": {"image": "assets/holo.jpg", "intensity": 1.4, "rotation": lin((0, 0), (10, 120))},
        "lights": [{"type": "area", "position": [CX, -300, -900], "size": [1200, 400], "intensity": 1.0}],
        "layers": [
            {"id": "bg", "kind": "shape", "shape": "field", "size": [1080, 1920], "transform": {"position": [CX, CY]}, "softness": 1.3, "warp": 120,
             "points": [{"position": {"base": [-300, -600], "add": [{"type": "wiggle", "hz": 0.1, "amp": [200, 200]}]}, "color": "#ff5fd2", "radius": 520},
                        {"position": {"base": [300, 0], "add": [{"type": "wiggle", "hz": 0.1, "amp": [200, 260], "seed": 3}]}, "color": "#4c2bff", "radius": 560},
                        {"position": {"base": [-200, 700], "add": [{"type": "wiggle", "hz": 0.12, "amp": [200, 200], "seed": 5}]}, "color": "#00e1ff", "radius": 480}]},
            {"id": "word", "kind": "mesh", "mesh": {"type": "extrude", "d": d, "depth": 110, "bevel": 16},
             "material": {"preset": "chrome", "roughness": 0.04},
             "transform": {"position": {"base": [CX, CY - 40], "add": [{"type": "sine", "hz": 0.2, "amp": [0, 22]}]},
                           "rotation_y": keys((0, -70), (1.6, -18, "land"), (10, 22, "linear")), "rotation_x": lin((0, 8), (10, -6))}},
            {"id": "blob", "kind": "mesh", "mesh": {"type": "sphere", "radius": 150}, "detail": 2,
             "displace": {"type": "fbm", "scale": 120, "amount": 40, "drift": [0, 60, 40]},
             "material": {"preset": "chrome", "color": "#f2e6ff", "roughness": 0.05},
             "transform": {"position": [CX + 230, 1420], "z": -200, "rotation_y": lin((0, 0), (10, 180))}},
            {"id": "ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 150, "tube": 34},
             "material": {"preset": "chrome", "roughness": 0.05},
             "transform": {"position": [CX - 250, 560], "z": -100, "rotation_x": lin((0, 60), (10, 240)), "rotation_y": lin((0, 0), (10, 90))}},
            *[{"id": f"glint{i}", "kind": "shape", "shape": "star", "sides": 4, "size": 60, "inner": 0.12, "fill": "#ffffff",
               "space": "screen", "transform": {"position": p, "scale": keys((t, 0), (t + 0.25, 1, "out"), (t + 0.6, 0, "in")),
                                                "rotation": lin((t, 0), (t + 0.6, 45))}}
              for i, (p, t) in enumerate([([300, 820], 2.0), ([820, 1080], 3.4), ([420, 1250], 5.1), ([760, 700], 6.6), ([560, 980], 8.2)])],
            *label("CHROME / Y2K", "chrome that reflects a real environment"),
        ],
    }


def screen():
    """3D objects inside a screen recording: reflected, refracted, shadowed."""
    heart = ("M 0 90 C -40 60 -150 -10 -150 -80 C -150 -150 -60 -180 0 -110 C 60 -180 150 -150 150 -80 C 150 -10 40 60 0 90 Z")
    return {
        "background": "#0d0d10",
        "look": {"tone": "agx"},
        "camera": {"fov": 34, "orbit": keys((0, [0, 0]), (2.0, [0, 0]), (4.0, [-16, 10], "glide"), (10, [14, 6], "glide")),
                   "dolly": keys((0, 0), (4, -500), (10, -400))},
        "environment": {"preset": "studio", "intensity": 0.9},
        "lights": [{"type": "area", "position": [-200, 200, -1200], "size": [260, 260], "intensity": 1.4, "shadow": True},
                   {"type": "directional", "from": [-0.5, -1, -0.9], "intensity": 0.6, "softness": 0.4}],
        "layers": [
            {"id": "rec", "kind": "video", "src": "assets/screen-feed.mp4", "three_d": True, "transform": {"position": [CX, CY]}},
            {"id": "catch", "kind": "mesh", "mesh": {"type": "plane", "size": [1080, 1920]},
             "material": {"preset": "shadow", "opacity": 0.7}, "transform": {"position": [CX, CY], "z": -2}},
            {"id": "heart", "kind": "mesh", "mesh": {"type": "extrude", "d": heart, "depth": 90, "bevel": 30},
             "material": {"preset": "coated", "color": "#ff2d55", "roughness": 0.1},
             "transform": {"position": keys((2.0, [740, 1250]), (2.9, [720, 900], "land")), "z": keys((2.0, 20), (2.9, -380, "land")),
                           "scale": keys((2.0, 0), (2.6, 1.15, "land"), (3.0, 1)),
                           "rotation_y": keys((2.0, -90), (3.4, 0, "land"), (10, 40, "linear"))}},
            *[{"id": f"coin{i}", "kind": "mesh", "mesh": {"type": "extrude", "d": {"shape": "circle", "size": 120}, "depth": 18, "bevel": 4},
               "material": {"preset": "metal", "color": "#ffc23a", "roughness": 0.15},
               "transform": {"position": keys((2.4 + 0.12 * i, [740, 1250]), (3.6 + 0.12 * i, [380 + 140 * i, 700 + 160 * (i % 2)], "out")),
                             "z": keys((2.4 + 0.12 * i, 10), (3.6 + 0.12 * i, -200 - 60 * i, "out")),
                             "rotation_x": lin((2.4, 20 * i), (10, 20 * i + 540)), "rotation_y": lin((2.4, 0), (10, 300 + 40 * i))}}
              for i in range(4)],
            {"id": "lens", "kind": "mesh", "mesh": {"type": "sphere", "radius": 140},
             "material": {"preset": "glass", "color": "#ffffff", "thickness": 280},
             "transform": {"position": keys((4.5, [-200, 1500]), (10, [700, 1450])), "z": -160}},
            *label("INSIDE THE RECORDING", "chrome reflects it, glass bends it, shadows fall on it", at=0.3),
        ],
    }


REELS = {
    "product": product,
    "glass": glass,
    "type": type_,
    "haze": haze,
    "soft": soft,
    "clay": clay,
    "lens": lens,
    "chrome": chrome,
    "screen": screen,
}


# ------------------------------------------------------------------- build

def run(bin_, *args, check=True):
    out = subprocess.run([bin_, "motion", *args], capture_output=True, text=True)
    if check and out.returncode != 0:
        sys.exit(f"motion {' '.join(args)}: {out.stderr or out.stdout}")
    return out.stdout


def build(bin_, out, name):
    project = os.path.join(out, f"{name}.lahza")
    if not os.path.exists(os.path.join(project, "project.json")):
        run(bin_, "create", project, "--size", f"{W}x{H}", "--fps", str(FPS), "--duration", str(DUR))
    pj = os.path.join(project, "project.json")
    with open(pj) as f:
        doc = json.load(f)
    doc["settings"].update({"renderer": "path", "samples": 64})
    with open(pj, "w") as f:
        json.dump(doc, f, indent=2)
    fonts = os.path.join(project, "fonts")
    os.makedirs(fonts, exist_ok=True)
    for f in ("Inter-Thin.otf", "Inter-Medium.otf", "Inter-Bold.otf", "Inter-Black.otf"):
        if not os.path.exists(os.path.join(fonts, f)):
            shutil.copy(os.path.join(FONTS, f), fonts)
    assets = os.path.join(project, "assets")
    os.makedirs(assets, exist_ok=True)
    if name == "chrome":
        shutil.copy(os.path.join(HERE, "../../assets/wallpapers/uihssn/uihssn-6.jpg"), os.path.join(assets, "holo.jpg"))
    if name == "screen":
        feed = os.path.join(out, "screen-feed.mp4")
        if not os.path.exists(feed):
            sys.exit(f"screen needs {feed} (node feed.js {out})")
        shutil.copy(feed, assets)
    if name == "lens":
        pj_doc = json.load(open(os.path.join(project, "project.json")))
        pj_doc["settings"]["motion_blur"] = {"shutter": 180, "samples": 4}
        json.dump(pj_doc, open(os.path.join(project, "project.json"), "w"), indent=2)
    scene = REELS[name]()
    scene.update({"id": name, "start": 0, "end": DUR})
    info = json.loads(run(bin_, "info", project))
    ops = [{"op": "remove", "id": s["id"]} for s in info.get("scenes", [])]
    ops.append(dict(op="add_scene", **scene))
    path = os.path.join(out, f"{name}.ops.json")
    with open(path, "w") as f:
        json.dump(ops, f, indent=1)
    run(bin_, "apply", project, path)
    return project


def main():
    args = sys.argv[1:]
    bin_ = os.path.join(HERE, "../../target/release/lahza-motion")
    spp, scale = None, None
    flags = set()
    rest = []
    i = 0
    while i < len(args):
        a = args[i]
        if a in ("--bin", "--spp", "--scale"):
            v = args[i + 1]
            i += 2
            if a == "--bin":
                bin_ = v
            elif a == "--spp":
                spp = v
            else:
                scale = v
            continue
        if a.startswith("--"):
            flags.add(a)
        else:
            rest.append(a)
        i += 1
    out = os.path.abspath(rest[0] if rest else "reels-out")
    names = rest[1:] or list(REELS)
    os.makedirs(out, exist_ok=True)
    for name in names:
        project = build(bin_, out, name)
        print(run(bin_, "check", project, check=False)[:1500])
        if "--sheets" in flags:
            run(bin_, "snapshot", project, "--at", "0.5,3,6,9.5", "--sheet", "--width", "270",
                "-o", os.path.join(out, f"{name}-sheet.png"))
        if "--render" in flags:
            cmd = ["render", project, "-o", os.path.join(out, f"{name}.mp4"), "--pt", "--no-audio"]
            if spp:
                cmd += ["--spp", spp]
            if scale:
                cmd += ["--scale", scale]
            print(run(bin_, *cmd)[-600:])
        print("built", project)


if __name__ == "__main__":
    main()
