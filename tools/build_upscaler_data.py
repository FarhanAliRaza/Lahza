#!/usr/bin/env python3
"""Build paired screen-content data for Lahza UI CNN v2.

Two sources, both rendered natively at every scale (no user recordings):
- Headless Chrome screenshots of the locally installed Rust documentation at
  device scale factors 1, 2 and 4, in light and forced-dark themes. Each book
  belongs to exactly one split.
- Procedural UI scenes drawn at 1× and at s× coordinates, so text and strokes
  are rasterized at each size instead of being downsampled.

Low-resolution inputs are additionally stored after Lahza's recording codec
path (VP8 realtime, yuv420p) at a range of qualities.
Code-editor and terminal pages are generated from openly licensed local
sources (Rust library, Python stdlib, package listings) highlighted with
pygments in editor themes. This repository's own code and git log form the
code test split only.
"""

from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
import argparse, hashlib, json, random, re, subprocess, tempfile

import numpy as np
from PIL import Image, ImageDraw, ImageFont
from pygments import highlight
from pygments.formatters import HtmlFormatter
from pygments.lexers import get_lexer_for_filename

ROOT = Path(__file__).resolve().parents[1]
DOCS = next((Path.home() / ".rustup/toolchains").glob("*/share/doc/rust/html"))
SPLITS = {
    "train": ["std", "alloc", "book", "rust-by-example", "unstable-book", "reference"],
    "valid": ["nomicon", "edition-guide"],
    "test": ["cargo", "rustc", "embedded-book"],
}
PAGES = {"train": 360, "valid": 48, "test": 48}
CODE_PAGES = {"train": 300, "valid": 40, "test": 40}
CODE_STYLES = ["vs", "default", "github-dark", "monokai", "one-dark", "dracula", "solarized-dark",
               "solarized-light", "nord", "gruvbox-dark", "material", "xcode", "native", "zenburn"]
MONO_FONTS = ["Ubuntu Mono", "DejaVu Sans Mono", "Liberation Mono", "Noto Mono", "Ubuntu Sans Mono"]
TERMINAL_THEMES = [  # background, foreground, prompt, path
    ("#1e1e1e", "#cccccc", "#23d18b", "#3b8eea"),
    ("#300a24", "#ffffff", "#8ae234", "#729fcf"),
    ("#000000", "#d0d0d0", "#4e9a06", "#3465a4"),
    ("#282c34", "#abb2bf", "#98c379", "#61afef"),
    ("#ffffff", "#333333", "#00a000", "#0451a5"),
    ("#fdf6e3", "#657b83", "#859900", "#268bd2"),
]
SCENES = {"train": (200000, 1200), "valid": (910000, 96), "test": (960000, 96)}
WINDOW = (400, 240)  # CSS pixels; 4× pairs are 1600×960.
FONT_DIRS = [Path("/usr/share/fonts/truetype"), Path("/usr/share/fonts/opentype")]
TRAIN_FONTS = [
    "dejavu/DejaVuSans.ttf",
    "dejavu/DejaVuSansMono.ttf",
    "dejavu/DejaVuSans-Bold.ttf",
    "dejavu/DejaVuSerif.ttf",
    "liberation/LiberationSans-Regular.ttf",
    "liberation/LiberationSans-Bold.ttf",
    "liberation/LiberationMono-Regular.ttf",
    "freefont/FreeSans.ttf",
    "freefont/FreeMono.ttf",
    "crosextra/Carlito-Regular.ttf",
]
# Held out from training: validation and test text uses these families only.
HELDOUT_FONTS = [
    "dejavu/DejaVuSerif-Bold.ttf",
    "liberation/LiberationSerif-Regular.ttf",
    "crosextra/Caladea-Regular.ttf",
]
CHARS = "abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ0123456789()[]{} <>:;,.+-=_/"


def font_path(name):
    for root in FONT_DIRS:
        if (root / name).exists():
            return root / name
    raise FileNotFoundError(name)


def scene(seed, scale, fonts, size=(160, 120)):
    """One procedural UI frame rasterized natively at `scale`."""
    r = random.Random(seed)
    w, h = size
    s = scale
    light = r.random() > 0.45
    bg = (r.randint(225, 252) if light else r.randint(12, 40),) * 3
    if r.random() < 0.25:
        bg = tuple(r.randint(200, 250) if light else r.randint(15, 60) for _ in range(3))
    im = Image.new("RGB", (w * s, h * s), bg)
    d = ImageDraw.Draw(im)
    fg = r.randint(15, 70) if light else r.randint(180, 250)
    palette = [
        (fg, fg, fg),
        (50, 126, 224),
        (39, 167, 102),
        (208, 92, 55),
        (166, 86, 199),
        (214, 160, 30),
        (220, 50, 80),
    ]

    def box(x0, y0, x1, y1):
        # Inclusive pixel rectangles cover s×s pixels per source pixel.
        return (x0 * s, y0 * s, x1 * s + s - 1, y1 * s + s - 1)

    panel = r.randint(205, 225) if light else r.randint(42, 65)
    d.rectangle(box(0, 0, w, r.randint(12, 22)), fill=(panel,) * 3)
    if r.random() < 0.5:
        d.rectangle(box(0, 0, r.randint(24, 48), h), fill=(panel,) * 3)
    for _ in range(r.randint(4, 12)):
        x, y = r.randint(-20, w - 10), r.randint(16, h - 5)
        bw, bh = r.randint(8, 90), r.randint(2, 30)
        color = r.choice(palette)
        kind = r.random()
        if kind < 0.45:
            d.rounded_rectangle(
                box(x, y, x + bw, y + bh),
                radius=r.randint(0, 8) * s,
                outline=color,
                width=r.randint(1, 2) * s,
            )
        elif kind < 0.65:
            d.rounded_rectangle(
                box(x, y, x + bw, y + bh), radius=r.randint(0, 8) * s, fill=color
            )
        elif kind < 0.8:
            rad = r.randint(2, 7)
            d.ellipse(box(x, y, x + rad * 2, y + rad * 2), outline=color, width=s)
        else:
            # Line centres map through pixel centres at every scale.
            c = lambda v: (v + 0.5) * s - 0.5
            d.line((c(x), c(y), c(x + bw), c(y + bh)), fill=color, width=r.randint(1, 2) * s)
    y = r.randint(18, 30)
    while y < h:
        font_file = font_path(r.choice(fonts))
        px = r.randint(8, 18)
        font = ImageFont.truetype(str(font_file), px * s)
        text = "".join(r.choice(CHARS) for _ in range(r.randint(6, 30)))
        x = r.randint(-12, 30)
        if r.random() < 0.3:
            d.text((x * s, y * s), text[: len(text) // 2], font=font, fill=r.choice(palette))
            x += len(text) // 2 * px // 2 + 4
            text = text[len(text) // 2 :]
        d.text((x * s, y * s), text, font=font, fill=r.choice(palette))
        y += r.randint(px + 2, px + 12)
    if r.random() < 0.3:
        x = r.randint(0, w - 30)
        for i in range(r.randint(3, 10)):
            d.rectangle(box(x + i * 3, 40, x + i * 3, 80), fill=palette[0])
    return np.asarray(im).copy()


def code_sources():
    """Openly licensed code by split; held-out text is this repository's own."""
    rust = next((Path.home() / ".rustup/toolchains").glob("*/lib/rustlib/src/rust/library"))
    files = sorted([*rust.glob("*/src/**/*.rs"), *Path("/usr/lib/python3.14").glob("*.py")])
    files = [f for f in files if f.stat().st_size > 3000]
    bucket = lambda f: int(hashlib.sha256(str(f).encode()).hexdigest(), 16) % 10
    run = lambda *cmd: subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT).stdout
    return {
        "train": ([f for f in files if bucket(f) != 0],
                  run("ls", "-la", "/usr/lib", "/usr/share") + run("dpkg", "-l")),
        "valid": ([f for f in files if bucket(f) == 0], run("ls", "-la", "/usr/bin")),
        "test": (sorted([*(ROOT / "src").rglob("*.rs"), *(ROOT / "tools").glob("*.py")]),
                 run("git", "log", "--stat", "-n", "300")),
    }


def code_page(r, files, terminal_text, out):
    """One VS Code-like editor or terminal page, written as standalone HTML."""
    font = r.choice(MONO_FONTS)
    size = r.choice([12, 13, 13, 14, 14, 15, 16])
    if r.random() < 0.6:
        path = r.choice(files)
        lines = path.read_text(errors="ignore").splitlines()
        start = r.randrange(max(1, len(lines) - 30))
        code = "\n".join(lines[start : start + 30])
        style = r.choice(CODE_STYLES)
        formatter = HtmlFormatter(style=style, linenos="table", linenostart=start + 1, nobackground=False)
        body = highlight(code, get_lexer_for_filename(path.name, stripall=False), formatter)
        css = formatter.get_style_defs(".highlight")
        bg = formatter.style.background_color or "#ffffff"
        page = f"<style>{css}body{{margin:0;background:{bg}}}.highlight,.linenos,pre{{font-family:'{font}';font-size:{size}px;line-height:1.4;margin:0;background:{bg}}}.linenos{{opacity:.55;padding-right:12px}}td{{padding:4px 6px;vertical-align:top}}</style>{body}"
    else:
        bg, fg, prompt, folder = r.choice(TERMINAL_THEMES)
        lines = terminal_text.splitlines()
        start = r.randrange(max(1, len(lines) - 30))
        esc = lambda t: t.replace("&", "&amp;").replace("<", "&lt;")
        cmd = r.choice(["ls -la", "cargo build --release", "git log --stat", "dpkg -l", "just install"])
        head = (f"<span style='color:{prompt};font-weight:bold'>farhan@lahza</span>:"
                f"<span style='color:{folder};font-weight:bold'>~/code/screendrop-rs</span>$ {cmd}\n")
        body = head + esc("\n".join(lines[start : start + 30]))
        page = f"<body style='margin:0;padding:6px;background:{bg};color:{fg}'><pre style=\"font-family:'{font}';font-size:{size}px;line-height:1.3;margin:0\">{body}</pre></body>"
    out.write_text("<!doctype html><meta charset=utf-8>" + page)
    return out


def chrome(url, out, scale, dark):
    if out.exists():
        return np.asarray(Image.open(out).convert("RGB")).copy()
    args = [
        "google-chrome",
        "--headless=new",
        "--disable-gpu",
        "--hide-scrollbars",
        "--no-first-run",
        "--disable-extensions",
        "--allow-file-access-from-files",
        f"--user-data-dir={out.parent / ('profile-' + out.stem)}",
        f"--window-size={WINDOW[0]},{WINDOW[1]}",
        f"--force-device-scale-factor={scale}",
        f"--screenshot={out}",
        url,
    ]
    if dark:
        args.insert(2, "--blink-settings=forceDarkModeEnabled=true")
    subprocess.run(args, check=True, capture_output=True, timeout=60)
    subprocess.run(["rm", "-rf", str(out.parent / ("profile-" + out.stem))])
    return np.asarray(Image.open(out).convert("RGB")).copy()


def vp8(image, q):
    """Round-trip one frame through Lahza's VP8 realtime/yuv420p recording path."""
    h, w, _ = image.shape
    with tempfile.TemporaryDirectory() as tmp:
        src, mid, dst = (Path(tmp) / n for n in ["in.png", "mid.webm", "out.png"])
        Image.fromarray(image).save(src)
        subprocess.run(
            ["ffmpeg", "-v", "error", "-y", "-i", src, "-c:v", "libvpx",
             "-deadline", "realtime", "-cpu-used", "8", "-qmin", str(q),
             "-qmax", str(q), "-b:v", "50M", "-pix_fmt", "yuv420p", mid],
            check=True,
        )
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", mid, "-pix_fmt", "rgb24", dst], check=True)
        out = np.asarray(Image.open(dst).convert("RGB")).copy()
    assert out.shape == (h, w, 3)
    return out


def degraded(lo, rng):
    """Codec variants stored beside the clean input: two VP8 qualities."""
    return np.stack([vp8(lo, rng.randint(8, 24)), vp8(lo, rng.randint(28, 48))])


def register(shots, margin=4):
    """Align Chrome DPR captures; anchor scrolling can round to a different
    pixel per scale. Shifts every capture onto the 1× grid and trims margins."""
    lo = shots[0].astype(np.float32)
    out = [shots[0][margin:-margin, margin:-margin]]
    for s, hi in zip((2, 4), shots[1:]):
        h, w, _ = hi.shape
        area = hi.astype(np.float32).reshape(h // s, s, w // s, s, 3).mean((1, 3))
        inner = lambda a, dy, dx: a[margin + dy : a.shape[0] - margin + dy, margin + dx : a.shape[1] - margin + dx]
        dy, dx = min(
            ((dy, dx) for dy in range(-3, 4) for dx in range(-3, 4)),
            key=lambda d: np.abs(inner(area, *d) - inner(lo, 0, 0)).mean(),
        )
        out.append(hi[(margin + dy) * s : h - (margin - dy) * s, (margin + dx) * s : w - (margin - dx) * s])
    return out


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--out", type=Path, default=ROOT / "ignore/upscaler-data")
    p.add_argument("--jobs", type=int, default=8)
    a = p.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)
    manifest = {"docs": str(DOCS), "window": WINDOW, "splits": {}}
    for split, books in SPLITS.items():
        rng = random.Random(f"lahza-v2-{split}")
        pages = sorted(
            page for book in books for page in (DOCS / book).rglob("*.html")
            if "/src/" not in str(page) and page.stat().st_size > 4000
        )
        pages = rng.sample(pages, PAGES[split])
        shots = a.out / "shots" / split
        shots.mkdir(parents=True, exist_ok=True)

        def capture(item):
            i, page = item
            dark = i % 3 == 2
            # Jump to a random in-page anchor so shots show body content.
            ids = re.findall(r'id="([A-Za-z][\w.-]*)"', page.read_text(errors="ignore"))
            r = random.Random(f"{split}-{i}")
            url = page.as_uri() + (f"#{r.choice(ids)}" if ids and r.random() < 0.8 else "")
            return register([chrome(url, shots / f"{i:04d}-{s}x.png", s, dark) for s in (1, 2, 4)])

        with ThreadPoolExecutor(a.jobs) as pool:
            docs = list(pool.map(capture, enumerate(pages)))
        files, terminal_text = code_sources()[split]
        pages_dir = a.out / "code" / split
        pages_dir.mkdir(parents=True, exist_ok=True)

        def capture_code(i):
            html = code_page(random.Random(f"code-{split}-{i}"), files, terminal_text, pages_dir / f"{i:04d}.html")
            return register([chrome(html.as_uri(), shots / f"code-{i:04d}-{s}x.png", s, False) for s in (1, 2, 4)])

        with ThreadPoolExecutor(a.jobs) as pool:
            code = list(pool.map(capture_code, range(CODE_PAGES[split])))
        start, count = SCENES[split]
        fonts = TRAIN_FONTS if split == "train" else HELDOUT_FONTS
        scenes = [[scene(start + i, s, fonts) for s in (1, 2, 4)] for i in range(count)]
        for name, group in [("docs", docs), ("scenes", scenes), ("code", code)]:
            lo = np.stack([g[0] for g in group])
            with ThreadPoolExecutor(a.jobs) as pool:
                codec = np.stack(list(pool.map(
                    lambda iv: degraded(iv[1], random.Random(f"{split}-{name}-{iv[0]}")),
                    enumerate(lo),
                )))
            hi2, hi4 = np.stack([g[1] for g in group]), np.stack([g[2] for g in group])
            # Standard SR inputs: the HR render area-downscaled to 1×, then VP8.
            down = {s: hi.reshape(len(hi), hi.shape[1] // s, s, hi.shape[2] // s, s, 3)
                    .mean((2, 4)).round().astype(np.uint8) for s, hi in ((2, hi2), (4, hi4))}
            with ThreadPoolExecutor(a.jobs) as pool:
                down_codec = {s: np.stack(list(pool.map(
                    lambda iv: vp8(iv[1], random.Random(f"{split}-{name}-down{s}-{iv[0]}").randint(12, 40)),
                    enumerate(down[s])))) for s in (2, 4)}
            path = a.out / f"{split}-{name}.npz"
            np.savez(path, lo=lo, codec=codec, hi2=hi2, hi4=hi4, down2=down[2], down4=down[4],
                     down2codec=down_codec[2], down4codec=down_codec[4])
            manifest["splits"][f"{split}-{name}"] = {
                "count": len(group),
                "sources": [str(pg.relative_to(DOCS)) for pg in pages] if name == "docs"
                else [start, start + count - 1] if name == "scenes"
                else sorted({str(f) for f in files}),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
            print(split, name, lo.shape, flush=True)
    manifest["train_fonts"] = TRAIN_FONTS
    manifest["heldout_fonts"] = HELDOUT_FONTS
    (a.out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
