#!/usr/bin/env python3
"""Train Lahza's tiny UI reconstruction networks on generated, non-user artwork.

Requires torch, numpy, Pillow. Reproducible seeds, train/validation separation,
source coefficients, complete metrics, and weight hashes are recorded with output.
No user screenshots or third-party weights are training inputs.
"""

from pathlib import Path
import argparse, hashlib, json, math, random, time, os

os.environ.setdefault("CUBLAS_WORKSPACE_CONFIG", ":4096:8")
import numpy as np
from PIL import Image, ImageDraw, ImageFont
import torch
from torch import nn
import torch.nn.functional as F

ROOT = Path(__file__).resolve().parents[1]
FONT_ROOT = Path("/usr/share/fonts/truetype/dejavu")
FONTS = [
    "DejaVuSans.ttf",
    "DejaVuSansMono.ttf",
    "DejaVuSans-Bold.ttf",
    "DejaVuSerif.ttf",
]


def scene(seed, size=192):
    r = random.Random(seed)
    light = r.random() > 0.5
    bg = r.randint(225, 252) if light else r.randint(12, 40)
    im = Image.new("RGB", (size, size), (bg, bg, bg))
    d = ImageDraw.Draw(im)
    fg = r.randint(15, 70) if light else r.randint(180, 250)
    palette = [
        (fg, fg, fg),
        (50, 126, 224),
        (39, 167, 102),
        (208, 92, 55),
        (166, 86, 199),
    ]
    panel = r.randint(205, 225) if light else r.randint(42, 65)
    d.rectangle((0, 0, size, r.randint(14, 25)), fill=(panel, panel, panel))
    for _ in range(r.randint(4, 10)):
        x, y = r.randint(-20, size - 10), r.randint(20, size - 5)
        w, h = r.randint(8, 110), r.randint(2, 36)
        color = r.choice(palette)
        if r.random() < 0.55:
            d.rounded_rectangle(
                (x, y, x + w, y + h),
                radius=r.randint(0, 9),
                outline=color,
                width=r.randint(1, 2),
            )
        else:
            d.line((x, y, x + w, y + h), fill=color, width=r.randint(1, 2))
    y = r.randint(22, 36)
    while y < size:
        font = ImageFont.truetype(str(FONT_ROOT / r.choice(FONTS)), r.randint(8, 20))
        s = "".join(
            r.choice(
                "abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ0123456789()[]{} <>:;,.+-=_/"
            )
            for _ in range(r.randint(6, 28))
        )
        d.text(
            (r.randint(-12, 18), y),
            s,
            font=font,
            fill=r.choice(palette),
            stroke_width=0,
        )
        y += r.randint(12, 25)
    if r.random() < 0.3:
        x = r.randint(0, size - 30)
        for i in range(r.randint(3, 10)):
            d.line((x + i * 3, 50, x + i * 3, 90), fill=palette[0], width=1)
    return np.asarray(im).copy()


def glyph_fixture():
    im = Image.new("RGB", (192, 192), "white")
    d = ImageDraw.Draw(im)
    font = ImageFont.truetype(str(FONT_ROOT / "DejaVuSerif-Bold.ttf"), 18)
    for i, text in enumerate(
        ["AaBbCc 01234", "Il1 O0 mn vw", "[]{}() <>+=", "Quick fox 789", "Zoom: 200%"]
    ):
        d.text((6, 8 + i * 34), text, font=font, fill=(20, 20, 20))
    return np.asarray(im).copy()


def stroke_error(actual, target):
    delta = (target[:, :, :, 1:] - target[:, :, :, :-1]).abs()
    predicted = (actual[:, :, :, 1:] - actual[:, :, :, :-1]).abs()
    mask = delta > 0.12
    return float((predicted[mask] - delta[mask]).abs().mean())


class Net(nn.Module):
    def __init__(self, weights=None):
        super().__init__()
        self.layers = nn.ModuleList(
            [
                nn.Conv2d(4 if i == 0 else 8, 4, 3, padding=1, padding_mode="replicate")
                for i in range(4)
            ]
        )
        if weights is not None:
            self.load_flat(weights)

    def load_flat(self, weights):
        a = np.frombuffer(weights, dtype="<f4").reshape(4, 292)
        with torch.no_grad():
            for i, layer in enumerate(self.layers):
                matrices = a[i, :288].reshape(2, 3, 3, 4, 4)
                # stored [sign,x,y,input,output]; Torch [output,input,y,x].
                values = matrices.transpose(4, 0, 3, 2, 1).reshape(4, 8, 3, 3)
                layer.weight.copy_(
                    torch.from_numpy(values[:, : 4 if i == 0 else 8].copy())
                )
                layer.bias.copy_(torch.from_numpy(a[i, 288:].copy()))

    def flat(self):
        output = []
        for i, layer in enumerate(self.layers):
            w = layer.weight.detach().cpu().numpy()
            if i == 0:
                w = np.concatenate([w, np.zeros_like(w)], axis=1)
            output.extend(w.reshape(4, 2, 4, 3, 3).transpose(1, 4, 3, 2, 0).reshape(-1))
            output.extend(layer.bias.detach().cpu().numpy())
        return np.asarray(output, dtype="<f4").tobytes()

    def forward(self, x):
        v = torch.cat([x, torch.ones_like(x[:, :1])], 1)
        for i, layer in enumerate(self.layers):
            if i:
                v = torch.cat([v.clamp_min(0), (-v).clamp_min(0)], 1)
            v = layer(v)
        return F.interpolate(
            x, scale_factor=2, mode="bilinear", align_corners=False
        ) + F.pixel_shuffle(v, 2)


def psnr(a, b):
    return float(-10 * torch.log10((a - b).square().mean().clamp_min(1e-12)))


def edge_error(a, b):
    return float(
        ((a[:, :, :, 1:] - a[:, :, :, :-1]) - (b[:, :, :, 1:] - b[:, :, :, :-1]))
        .abs()
        .mean()
        + ((a[:, :, 1:] - a[:, :, :-1]) - (b[:, :, 1:] - b[:, :, :-1])).abs().mean()
    )


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--steps", type=int, default=3000)
    p.add_argument("--four-steps", type=int, default=6000)
    p.add_argument("--images", type=int, default=512)
    p.add_argument("--out", type=Path, default=ROOT / "assets/upscaler")
    a = p.parse_args()
    torch.set_num_threads(4)
    torch.manual_seed(1317)
    np.random.seed(1317)
    random.seed(1317)
    torch.use_deterministic_algorithms(True)
    torch.backends.cudnn.deterministic = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    device = "cuda" if torch.cuda.is_available() else "cpu"
    torch.backends.cudnn.benchmark = False
    a.out.mkdir(parents=True, exist_ok=True)
    print("device", device, "torch", torch.__version__, flush=True)
    train = (
        torch.from_numpy(np.stack([scene(100000 + i) for i in range(a.images)]))
        .permute(0, 3, 1, 2)
        .float()
        .div_(255)
        .to(device)
    )
    valid = (
        torch.from_numpy(np.stack([scene(900000 + i) for i in range(64)]))
        .permute(0, 3, 1, 2)
        .float()
        .div_(255)
        .to(device)
    )
    glyph = (
        torch.from_numpy(glyph_fixture())
        .permute(2, 0, 1)
        .unsqueeze(0)
        .float()
        .div_(255)
        .to(device)
    )
    original = (ROOT / "assets/upscaler/anime4k-cnn-x2-s.f32").read_bytes()
    base = Net(original).to(device).eval()
    two = Net(original).to(device)
    four = Net(original).to(device)
    results = {}
    start = time.time()
    for scale, model in [(2, two), (4, four)]:
        if scale == 4:
            model.load_flat(two.flat())
            four_first = Net(two.flat()).to(device)
        parameters = list(model.parameters()) + (
            list(four_first.parameters()) if scale == 4 else []
        )
        opt = torch.optim.Adam(parameters, lr=0.001 if scale == 2 else 0.0005)
        steps = a.steps if scale == 2 else a.four_steps
        for step in range(steps):
            ids = torch.randint(len(train), (24,), device=device)
            x = random.randrange(0, 192 - 96 + 1)
            y = random.randrange(0, 192 - 96 + 1)
            hi = train[ids, :, y : y + 96, x : x + 96]
            lo = F.interpolate(
                hi,
                scale_factor=1 / scale,
                mode="bilinear",
                align_corners=False,
                antialias=True,
            )
            if scale == 4:
                intermediate = four_first(lo).clamp(0, 1)
                # Straight-through quantization models the GPU's RGBA8 intermediate.
                lo = (
                    intermediate
                    + (intermediate.mul(255).round().div(255) - intermediate).detach()
                )
            pred = model(lo)
            # RGB reconstruction plus stroke-edge consistency, no perceptual/GAN hallucination loss.
            error = (pred - hi).square().mean()
            edge = (
                (pred[:, :, :, 1:] - pred[:, :, :, :-1])
                - (hi[:, :, :, 1:] - hi[:, :, :, :-1])
            ).abs().mean() + (
                (pred[:, :, 1:] - pred[:, :, :-1]) - (hi[:, :, 1:] - hi[:, :, :-1])
            ).abs().mean()
            loss = error + (0.015 if scale == 2 else 0.6) * edge
            opt.zero_grad(set_to_none=True)
            loss.backward()
            nn.utils.clip_grad_norm_(parameters, 1.0)
            opt.step()
            if step % 250 == 0:
                print(
                    scale,
                    step,
                    float(loss),
                    "elapsed",
                    round(time.time() - start, 1),
                    flush=True,
                )
            if step in [int(steps * 0.6), int(steps * 0.85)]:
                for group in opt.param_groups:
                    group["lr"] *= 0.3
        model.eval()
        with torch.no_grad():
            lo = F.interpolate(
                valid,
                scale_factor=1 / scale,
                mode="bilinear",
                align_corners=False,
                antialias=True,
            )
            linear = F.interpolate(
                lo, scale_factor=scale, mode="bilinear", align_corners=False
            )
            legacy = base(lo).clamp(0, 1)
            if scale == 4:
                oldbase = F.interpolate(
                    legacy, scale_factor=2, mode="bilinear", align_corners=False
                )
                legacy = oldbase + (base(legacy) - oldbase) * 0.2
                actual = model(
                    four_first(lo).clamp(0, 1).mul(255).round().div(255)
                ).clamp(0, 1)
            else:
                actual = model(lo).clamp(0, 1)
            result = {
                "psnr_bilinear": psnr(linear, valid),
                "psnr_anime4k": psnr(legacy, valid),
                "psnr_ui": psnr(actual, valid),
                "edge_error_bilinear": edge_error(linear, valid),
                "edge_error_ui": edge_error(actual, valid),
            }
            glyph_lo = F.interpolate(
                glyph,
                scale_factor=1 / scale,
                mode="bilinear",
                align_corners=False,
                antialias=True,
            )
            glyph_base = F.interpolate(
                glyph_lo, scale_factor=scale, mode="bilinear", align_corners=False
            )
            glyph_actual = model(
                glyph_lo
                if scale == 2
                else four_first(glyph_lo).clamp(0, 1).mul(255).round().div(255)
            ).clamp(0, 1)
            result.update(
                stroke_contrast_error_bilinear=stroke_error(linear, valid),
                stroke_contrast_error_ui=stroke_error(actual, valid),
                glyph_psnr_bilinear=psnr(glyph_base, glyph),
                glyph_psnr_ui=psnr(glyph_actual, glyph),
                glyph_stroke_error_bilinear=stroke_error(glyph_base, glyph),
                glyph_stroke_error_ui=stroke_error(glyph_actual, glyph),
            )
            Image.fromarray(
                glyph_actual[0].permute(1, 2, 0).mul(255).round().byte().cpu().numpy()
            ).save(a.out / f"validation-{scale}x-glyphs.png")
            print(scale, result, flush=True)
            assert result["psnr_ui"] > result["psnr_bilinear"] + 0.15, (
                "held-out reconstruction must improve over interpolation"
            )
            assert result["edge_error_ui"] < result["edge_error_bilinear"], (
                "held-out strokes must improve"
            )
            for label, im in [
                ("target", valid[0]),
                ("bilinear", linear[0]),
                ("ui", actual[0]),
                ("anime4k", legacy[0]),
            ]:
                Image.fromarray(
                    im.permute(1, 2, 0)
                    .mul(255)
                    .round()
                    .clamp(0, 255)
                    .byte()
                    .cpu()
                    .numpy()
                ).save(a.out / f"validation-{scale}x-{label}.png")
        data = model.flat()
        filename = f"ui-cnn-{scale}x.f32"
        (a.out / filename).write_bytes(data)
        result.update(file=filename, sha256=hashlib.sha256(data).hexdigest())
        if scale == 4:
            first = four_first.flat()
            (a.out / "ui-cnn-4x-first.f32").write_bytes(first)
            result.update(
                first_stage_file="ui-cnn-4x-first.f32",
                first_stage_sha256=hashlib.sha256(first).hexdigest(),
            )
        results[str(scale)] = result
    metadata = {
        "model": "Lahza UI CNN v1",
        "architecture": "4 convolution layers;4 features;positive/negative feature split;luma-residual pixelshuffle2;4x uses a jointly trained two-stage cascade with its own first and second stage",
        "seed": 1317,
        "train_seed_range": [100000, 100000 + a.images - 1],
        "validation_seed_range": [900000, 900063],
        "training_images": a.images,
        "validation_images": 64,
        "steps_per_scale": {"2": a.steps, "4": a.four_steps},
        "torch": torch.__version__,
        "numpy": np.__version__,
        "pillow": Image.__version__,
        "device": device,
        "loss": "RGB MSE + L1 gradient (weight0.015 for2x;0.6 for4x)",
        "initialization": "MIT Anime4K x2 S",
        "input_data": "procedural UI,text,lines; no user or vendor screenshots",
        "font_files": FONTS,
        "glyph_holdout_font": "DejaVuSerif-Bold.ttf (not used in training)",
        "training_seconds": time.time() - start,
        "metrics": results,
    }
    (a.out / "ui-training.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()
