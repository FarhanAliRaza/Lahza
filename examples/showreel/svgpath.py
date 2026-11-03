"""Converts the Lahza Urdu wordmark SVG into engine path data.

The SVG nests affine transforms; this applies them so every element is in
viewBox units (0..250 x 0..80), then centres and scales the result.
"""
import os
import re


def mul(a, b):
    # 2x3 affine matrices as (a, b, c, d, e, f): x' = a x + c y + e.
    return (a[0] * b[0] + a[2] * b[1], a[1] * b[0] + a[3] * b[1],
            a[0] * b[2] + a[2] * b[3], a[1] * b[2] + a[3] * b[3],
            a[0] * b[4] + a[2] * b[5] + a[4], a[1] * b[4] + a[3] * b[5] + a[5])


def apply(m, x, y):
    return (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])


def tokens(d):
    return re.findall(r"[MLHVCSQZmlhvcsqz]|-?\d*\.?\d+(?:e-?\d+)?", d)


def path_polys(d):
    """Absolute subpaths of (command, points) from SVG path data (M L H V C Z)."""
    out = []
    t = tokens(d)
    i = 0
    cmd = None
    cur = (0.0, 0.0)
    start = cur
    sub = None
    while i < len(t):
        if re.match(r"[A-Za-z]", t[i]):
            cmd = t[i]
            i += 1
            if cmd in "Zz":
                if sub:
                    sub.append(("Z",))
                cur = start
                continue
        num = lambda k: float(t[i + k])
        rel = cmd.islower()
        c = cmd.upper()
        if c == "M":
            p = (num(0), num(1))
            if rel:
                p = (cur[0] + p[0], cur[1] + p[1])
            sub = [("M", p)]
            out.append(sub)
            cur = start = p
            i += 2
            cmd = "l" if rel else "L"
        elif c == "L":
            p = (num(0), num(1))
            if rel:
                p = (cur[0] + p[0], cur[1] + p[1])
            sub.append(("L", p))
            cur = p
            i += 2
        elif c == "H":
            x = num(0) + (cur[0] if rel else 0)
            cur = (x, cur[1])
            sub.append(("L", cur))
            i += 1
        elif c == "V":
            y = num(0) + (cur[1] if rel else 0)
            cur = (cur[0], y)
            sub.append(("L", cur))
            i += 1
        elif c == "C":
            pts = [(num(0), num(1)), (num(2), num(3)), (num(4), num(5))]
            if rel:
                pts = [(cur[0] + x, cur[1] + y) for x, y in pts]
            sub.append(("C",) + tuple(pts))
            cur = pts[2]
            i += 6
        else:
            raise ValueError(cmd)
    return out


def emit(polys, m):
    parts = []
    for sub in polys:
        for el in sub:
            if el[0] == "Z":
                parts.append("Z")
            else:
                pts = [apply(m, *p) for p in el[1:]]
                parts.append(el[0] + " " + " ".join(f"{x:.2f} {y:.2f}" for x, y in pts))
    return " ".join(parts)


def wordmark(width=1000.0):
    """Returns (letters_d, brackets [d, …] TL TR BR BL order, dot (cx, cy, r)), centred at 0,0, `width` px wide."""
    root = (1, 0, 0, 1, -1272.206387, -644.277519)
    root = mul(root, (0.454861, 0, 0, 0.454861, 725.411432, 356.767231))
    k = width / 250.0
    norm = (k, 0, 0, k, -125 * k, -40 * k)
    base = mul(norm, root)
    letters = []
    rect = mul(base, (1, 0, 0, 1, 998.691182, 219.651334))
    x, y, w, h = 459.302, 421.885, 18.952, 21.867
    letters.append(emit([[("M", (x, y)), ("L", (x + w, y)), ("L", (x + w, y + h)), ("L", (x, y + h)), ("Z",)]], rect))
    repo = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
    svg = open(os.path.join(repo, "assets", "brand", "wordmark-urdu.svg")).read()
    groups = re.findall(r'<g transform="matrix\(([^)]*)\)">\s*<path d="([^"]*)"', svg)
    brackets = []
    for mat, d in groups:
        m = mul(base, tuple(float(v) for v in mat.split(",")))
        s = emit(path_polys(d), m)
        if "329.644" in d:
            brackets.append(s)
        else:
            letters.append(s)
    c = mul(base, (1, 0, 0, 1, 989.008561, -149.658048))
    cx, cy = apply(c, 283.291, 906.813)
    r = 22.37 * 0.454861 * k
    return " ".join(letters), brackets, (cx, cy, r)


if __name__ == "__main__":
    l, b, dot = wordmark()
    print(len(l), len(b), dot)
