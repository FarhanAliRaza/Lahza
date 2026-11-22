"""Checks a plane track against the footage itself.

    lahza motion track-plane clip.mp4 --corners ... --at S --detail > track.json
    python3 verify.py track.json clip.mp4 sheet.png [frames_dir]

Each frame's tracked region is warped back onto the marked rectangle. If
the track is right, the warped pictures are one picture: what is left is
measured cell by cell with a small correlation search of its own (not the
tracker's), against the marked frame and against ten frames earlier, in
source pixels. sheet.png shows six frames with the region drawn on them
and the warped region under each: the same surface should sit still in
the lower row. Needs numpy and Pillow, and ffmpeg for the frames.
"""
import json, sys, os, subprocess
import numpy as np
from PIL import Image, ImageDraw

if len(sys.argv) < 4:
    sys.exit(__doc__)
track_json, video, out = sys.argv[1], sys.argv[2], sys.argv[3]
frames_dir = sys.argv[4] if len(sys.argv) > 4 else out + ".frames"
t = json.load(open(track_json))
os.makedirs(frames_dir, exist_ok=True)
if not os.listdir(frames_dir):
    subprocess.run(["ffmpeg", "-v", "error", "-i", video, "-fps_mode", "passthrough", os.path.join(frames_dir, "%04d.png")], check=True)
names = sorted(os.listdir(frames_dir))
rows = t["track"]
marked = next(r for r in rows if r.get("match") == 1.0 and r["found"])
q0 = marked["corners"]
W = int(round(max(abs(q0[1][0] - q0[0][0]), 64)))
H = int(round(max(abs(q0[3][1] - q0[0][1]), 64)))

def rectified(i):
    r = rows[i]
    img = Image.open(os.path.join(frames_dir, names[i])).convert("L")
    c = r["corners"]
    # The perspective map from the output rectangle to the frame (PIL's
    # QUAD is bilinear, which is not how a plane is seen).
    src = [(0, 0), (W, 0), (W, H), (0, H)]
    A, b = [], []
    for (x, y), (u, v) in zip(src, c):
        A.append([x, y, 1, 0, 0, 0, -u * x, -u * y]); b.append(u)
        A.append([0, 0, 0, x, y, 1, -v * x, -v * y]); b.append(v)
    k = np.linalg.solve(np.array(A, float), np.array(b, float))
    # PIL samples at pixel centres: output (x + .5, y + .5) -> input.
    return np.asarray(img.transform((W, H), Image.PERSPECTIVE, tuple(k), Image.BICUBIC), dtype=np.float32)

def blur(a, n=2):
    for _ in range(n):
        a = (a + np.roll(a, 1, 0) + np.roll(a, -1, 0) + np.roll(a, 1, 1) + np.roll(a, -1, 1)) / 5
    return a

def shift_of(a, b, r=6):
    """Shift (dx, dy) of b against a, to a fraction of a pixel, by ZNCC."""
    best = (-2, 0, 0); scores = {}
    A = a[r:-r, r:-r]; A = A - A.mean(); na = np.sqrt((A * A).sum()) + 1e-9
    for dy in range(-r, r + 1):
        for dx in range(-r, r + 1):
            B = b[r + dy:b.shape[0] - r + dy, r + dx:b.shape[1] - r + dx]; B = B - B.mean()
            s = float((A * B).sum() / (na * (np.sqrt((B * B).sum()) + 1e-9)))
            scores[(dx, dy)] = s
            if s > best[0]: best = (s, dx, dy)
    s, dx, dy = best
    def sub(m, c, p):
        d = m - 2 * c + p
        return 0.0 if abs(d) < 1e-9 else 0.5 * (m - p) / d
    fx = sub(scores.get((dx - 1, dy), s), s, scores.get((dx + 1, dy), s)) if abs(dx) < r else 0
    fy = sub(scores.get((dx, dy - 1), s), s, scores.get((dx, dy + 1), s)) if abs(dy) < r else 0
    return dx + fx, dy + fy, s

def residual(a, b):
    ds = []
    for gy in range(G):
        for gx in range(G):
            ys, xs = slice(gy * H // G, (gy + 1) * H // G), slice(gx * W // G, (gx + 1) * W // G)
            dx, dy, s = shift_of(a[ys, xs], b[ys, xs])
            if s > 0.5 and abs(dx) < 5.5 and abs(dy) < 5.5: ds.append((dx * dx + dy * dy) ** 0.5)
    return (float(np.median(ds)), len(ds)) if len(ds) >= 4 else (float("nan"), len(ds))

G = 4
rect = {i: blur(rectified(i)) for i, r in enumerate(rows) if r["found"]}
m = rows.index(marked)
print(f"residual shift of the warped region, source pixels ({W}x{H} region, {G*G} cells; a cell counts when it correlates over 0.5)")
print("  frame: against the marked frame | against 10 frames earlier | match")
worst_back = 0.0
for i in range(0, len(rows), max(1, len(rows) // 16)):
    if i not in rect:
        print(f"  {rows[i]['frame']}: lost"); continue
    a, na = residual(rect[m], rect[i])
    line = f"  {rows[i]['frame']}: {a:.2f} px ({na} cells)"
    if i - 10 in rect:
        b, nb = residual(rect[i - 10], rect[i]); line += f" | {b:.2f} px ({nb} cells)"
    print(line + f" | {rows[i]['match']}")
steps = [residual(rect[i - 10], rect[i])[0] for i in range(10, len(rows), 2) if i in rect and i - 10 in rect]
print(f"over any 10 frames: median {np.nanmedian(steps):.2f} px, worst {np.nanmax(steps):.2f} px")

# A sheet: the quad drawn on the frame, and the warped region under it.
pick = list(range(0, len(rows), max(1, len(rows) // 6)))[:6]
tw = 320; th = None; tiles = []
for i in pick:
    img = Image.open(os.path.join(frames_dir, names[i])).convert("RGB")
    d = ImageDraw.Draw(img)
    if rows[i]["found"]:
        c = [tuple(p) for p in rows[i]["corners"]]
        d.line(c + [c[0]], fill=(0, 255, 80), width=5)
        for k, p in enumerate(c):
            d.ellipse((p[0] - 9, p[1] - 9, p[0] + 9, p[1] + 9), fill=[(255, 60, 60), (255, 220, 0), (60, 160, 255), (255, 255, 255)][k])
    th = int(img.height * tw / img.width)
    top = img.resize((tw, th))
    rect = Image.fromarray(np.clip(rectified(i), 0, 255).astype(np.uint8)).convert("RGB") if rows[i]["found"] else Image.new("RGB", (W, H))
    rh = int(H * tw / W)
    tile = Image.new("RGB", (tw, th + rh + 4), (20, 20, 24))
    tile.paste(top, (0, 0)); tile.paste(rect.resize((tw, rh)), (0, th + 4))
    tiles.append(tile)
sheet = Image.new("RGB", (tw * len(tiles) + 4 * (len(tiles) - 1), tiles[0].height), (20, 20, 24))
for k, tile in enumerate(tiles):
    sheet.paste(tile, (k * (tw + 4), 0))
sheet.save(out)
