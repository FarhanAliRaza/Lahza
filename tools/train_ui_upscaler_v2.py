#!/usr/bin/env python3
"""Train Lahza UI CNN v2 candidates on natively rendered screen pairs.

Data comes from tools/build_upscaler_data.py. Each candidate is a plain
3×3-convolution network (ReLU, one long skip) predicting an RGB residual that
is pixel-shuffled onto bilinear RGB, so it maps directly onto WGSL compute.
Selection uses the validation split only; the test split is reported once.
"""

from pathlib import Path
import argparse, hashlib, json, math, random, sys, time, os

os.environ.setdefault("CUBLAS_WORKSPACE_CONFIG", ":4096:8")
import numpy as np
from PIL import Image
import torch
from torch import nn
import torch.nn.functional as F

sys.path.insert(0, str(Path(__file__).resolve().parent))
import train_ui_upscaler as v1

ROOT = v1.ROOT


class Net(nn.Module):
    def __init__(self, scale, channels, layers):
        super().__init__()
        self.scale = scale
        conv = lambda i, o: nn.Conv2d(i, o, 3, padding=1, padding_mode="replicate")
        self.head = conv(3, channels)
        self.body = nn.ModuleList(conv(channels, channels) for _ in range(layers - 2))
        self.tail = conv(channels, 3 * scale * scale)
        nn.init.zeros_(self.tail.weight)
        nn.init.zeros_(self.tail.bias)

    def forward(self, x):
        f = F.relu(self.head(x))
        h = f
        for layer in self.body:
            h = F.relu(layer(h))
        base = F.interpolate(x, scale_factor=self.scale, mode="bilinear", align_corners=False)
        return base + F.pixel_shuffle(self.tail(h + f), self.scale)

    def parameters_count(self):
        return sum(p.numel() for p in self.parameters())

    def macs_per_input_pixel(self):
        return sum(m.weight.numel() for m in self.modules() if isinstance(m, nn.Conv2d))


def load(path, device):
    z = np.load(path)
    t = lambda a: torch.from_numpy(a).to(device)
    return {k: t(z[k]) for k in z.files}


def as_float(x):
    return x.permute(0, 3, 1, 2).float().div_(255)


def batch(sets, scale, size, count, gen):
    """Random LR/HR crop pairs over every source and degradation, plus the
    clean LR crop that the output must reproduce when area-downscaled."""
    los, his, cleans = [], [], []
    for _ in range(count):
        data = sets[int(torch.randint(len(sets), (1,), generator=gen))]
        n, h, w, _ = data["lo"].shape
        i = int(torch.randint(n, (1,), generator=gen))
        kind = float(torch.rand(1, generator=gen))
        f = 1
        if kind < 0.55:
            # Standard SR input: the HR render area-downscaled, mostly after VP8.
            hi_img = data[f"hi{scale}"][i]
            clean_img = data[f"down{scale}"][i]
            lo_img = data[f"down{scale}codec"][i] if kind < 0.4 else clean_img
        elif scale == 2 and kind < 0.65:
            # Native 2×→4× pairs (HiDPI sources), clean.
            lo_img, hi_img, f = data["hi2"][i], data["hi4"][i], 2
            clean_img = lo_img
        else:
            # Natively rasterized 1× input (different hinting), clean or VP8.
            hi_img = data[f"hi{scale}"][i]
            clean_img = data["lo"][i]
            lo_img = clean_img if kind < 0.8 else data["codec"][i, int(torch.randint(2, (1,), generator=gen))]
        hh, ww = h * f, w * f
        y = int(torch.randint(hh - size + 1, (1,), generator=gen))
        x = int(torch.randint(ww - size + 1, (1,), generator=gen))
        hi = hi_img[y * scale : (y + size) * scale, x * scale : (x + size) * scale]
        lo = as_float(lo_img[y : y + size, x : x + size][None])[0]
        clean = as_float(clean_img[y : y + size, x : x + size][None])[0]
        los.append(lo)
        cleans.append(clean)
        his.append(as_float(hi[None])[0])
    lo, hi, clean = torch.stack(los), torch.stack(his), torch.stack(cleans)
    # Light/dark inversion and channel order keep the model theme- and hue-neutral.
    flip = torch.rand(count, 1, 1, 1, generator=gen).to(lo.device) < 0.5
    lo, hi, clean = (torch.where(flip, 1 - t, t) for t in (lo, hi, clean))
    perm = torch.randperm(3, generator=gen).to(lo.device)
    return tuple(t[:, perm].contiguous() for t in (lo, hi, clean))


def grad_l1(a, b):
    return ((a[..., 1:] - a[..., :-1]) - (b[..., 1:] - b[..., :-1])).abs().mean() + (
        (a[..., 1:, :] - a[..., :-1, :]) - (b[..., 1:, :] - b[..., :-1, :])
    ).abs().mean()


class V1:
    """The shipped UI CNN v1 (2× model, 4× two-stage cascade)."""

    def __init__(self, device):
        p = ROOT / "assets/upscaler"
        self.m = [v1.Net((p / f).read_bytes()).to(device).eval()
                  for f in ["ui-cnn-2x.f32", "ui-cnn-4x-first.f32", "ui-cnn-4x.f32"]]

    def __call__(self, lo, scale):
        if scale == 2:
            return self.m[0](lo)
        return self.m[2](self.m[1](lo).clamp(0, 1).mul(255).round().div(255))


def evaluate(sets, scale, models, device, chunk=8):
    """Per-set metrics for bilinear and every model; inputs are exactly as stored."""
    out = {}
    for name, (lo_all, hi_all) in sets.items():
        sums = {}
        n = len(lo_all)
        for s in range(0, n, chunk):
            lo = as_float(lo_all[s : s + chunk])
            hi = as_float(hi_all[s : s + chunk])
            preds = {"bilinear": F.interpolate(lo, scale_factor=scale, mode="bilinear", align_corners=False)}
            for label, model in models.items():
                preds[label] = (model(lo, scale) if isinstance(model, V1) else model(lo)).clamp(0, 1)
            for label, pred in preds.items():
                # Quantize like the RGBA8 output.
                pred = pred.mul(255).round().div(255)
                m = sums.setdefault(label, {"mse": 0.0, "edge": 0.0, "stroke": 0.0})
                k = len(lo) / n
                m["mse"] += float((pred - hi).square().mean()) * k
                m["edge"] += v1.edge_error(pred, hi) * k
                m["stroke"] += v1.stroke_error(pred, hi) * k
        out[name] = {
            label: {"psnr": -10 * math.log10(m["mse"]), "edge_error": m["edge"], "stroke_error": m["stroke"]}
            for label, m in sums.items()
        }
    return out


def eval_sets(data_dir, split, scale, device):
    sets = {}
    for source in ["docs", "scenes", "code"]:
        z = load(data_dir / f"{split}-{source}.npz", device)
        hi = z[f"hi{scale}"]
        sets[f"{source}-native"] = (z["lo"], hi)
        sets[f"{source}-native-vp8"] = (z["codec"][:, 1], hi)
        sets[f"{source}-down-vp8"] = (z[f"down{scale}codec"], hi)
    return sets


def local_range(x):
    return F.max_pool2d(x, 3, 1, 1) + F.max_pool2d(-x, 3, 1, 1)


@torch.no_grad()
def crispness(models, scale, device, frames):
    """Evaluation only, on real Lahza recordings (never trained on): how much of
    the source's local contrast survives area-downscaling the output back."""
    crops = []
    for path in frames:
        im = torch.from_numpy(np.asarray(Image.open(path).convert("RGB")).copy())
        x = as_float(im[None].to(device))
        energy = F.avg_pool2d(local_range(x.mean(1, keepdim=True)), 128, 128)[0, 0]
        for index in energy.flatten().topk(4).indices.tolist():
            y, x0 = divmod(index, energy.shape[1])
            crops.append(x[:, :, y * 128 : y * 128 + 128, x0 * 128 : x0 * 128 + 128])
    crop = torch.cat(crops)
    source = local_range(crop.mean(1, keepdim=True))
    mask = source > 0.1
    preds = {"bilinear": F.interpolate(crop, scale_factor=scale, mode="bilinear", align_corners=False)}
    for label, model in models.items():
        preds[label] = (model(crop, scale) if isinstance(model, V1) else model(crop)).clamp(0, 1)
    out = {}
    for label, pred in preds.items():
        back = F.avg_pool2d(pred, scale)
        # Halo: output brighter/darker than anything in its source neighbourhood.
        top = F.interpolate(F.max_pool2d(crop, 3, 1, 1), scale_factor=scale, mode="nearest")
        bottom = F.interpolate(-F.max_pool2d(-crop, 3, 1, 1), scale_factor=scale, mode="nearest")
        halo = (pred - top).clamp_min(0) + (bottom - pred).clamp_min(0)
        out[label] = {
            "contrast_retention": float(local_range(back.mean(1, keepdim=True))[mask].mean() / source[mask].mean()),
            "backprojection_mae": float((back - crop).abs().mean()),
            "halo": float(halo.mean() * 1000),
            "edge_energy": float((pred[..., 1:] - pred[..., :-1]).abs().mean() * 1000 * scale),
        }
    return out


@torch.no_grad()
def timing(model, scale, device, size):
    x = torch.rand(1, 3, size[1], size[0], device=device).half()
    m = model.half()
    for _ in range(3):
        m(x)
    torch.cuda.synchronize()
    start = time.perf_counter()
    for _ in range(20):
        m(x)
    torch.cuda.synchronize()
    model.float()
    return (time.perf_counter() - start) / 20 * 1000


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--scale", type=int, choices=[2, 4], required=True)
    p.add_argument("--channels", type=int, default=16)
    p.add_argument("--layers", type=int, default=6)
    p.add_argument("--steps", type=int, default=20000)
    p.add_argument("--batch", type=int, default=32)
    p.add_argument("--crop", type=int, default=48, help="LR crop size")
    p.add_argument("--data", type=Path, default=ROOT / "ignore/upscaler-data")
    p.add_argument("--out", type=Path, default=ROOT / "ignore/upscaler-v2-runs")
    p.add_argument("--test", action="store_true", help="also report the held-out test split")
    p.add_argument("--edge", type=float, default=1.0, help="gradient L1 weight")
    p.add_argument("--consistency", type=float, default=1.0, help="area back-projection L1 weight")
    p.add_argument("--name", default="")
    a = p.parse_args()
    seed = 2604
    torch.manual_seed(seed)
    random.seed(seed)
    np.random.seed(seed)
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    device = "cuda"
    name = f"{a.scale}x-c{a.channels}-l{a.layers}{a.name}"
    out = a.out / name
    out.mkdir(parents=True, exist_ok=True)
    train = [load(a.data / f"train-{s}.npz", device) for s in ["docs", "scenes", "code"]]
    gen = torch.Generator().manual_seed(seed)
    model = Net(a.scale, a.channels, a.layers).to(device)
    opt = torch.optim.Adam(model.parameters(), lr=1e-3)
    sched = torch.optim.lr_scheduler.OneCycleLR(opt, max_lr=1e-3, total_steps=a.steps, pct_start=0.05)
    print(name, "params", model.parameters_count(), flush=True)
    start = time.time()
    for step in range(a.steps):
        lo, hi, clean = batch(train, a.scale, a.crop, a.batch, gen)
        pred = model(lo)
        # Hinting differs between native scales, so the HR target alone rewards
        # averaged, rounded glyphs. Back-projection onto the clean LR crop keeps
        # the source's pixel structure (stroke gaps, counters) intact.
        loss = (
            torch.sqrt((pred - hi).square() + 1e-6).mean()
            + a.edge * grad_l1(pred, hi)
            + a.consistency * (F.avg_pool2d(pred, a.scale) - clean).abs().mean()
        )
        opt.zero_grad(set_to_none=True)
        loss.backward()
        nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        sched.step()
        if step % 1000 == 0:
            print(name, step, round(loss.item(), 5), round(time.time() - start, 1), flush=True)
    model.eval()
    reference = V1(device)
    with torch.no_grad():
        result = {
            "model": "Lahza UI CNN v2 candidate",
            "scale": a.scale,
            "channels": a.channels,
            "layers": a.layers,
            "parameters": model.parameters_count(),
            "macs_per_lr_pixel": model.macs_per_input_pixel(),
            "v1_macs_per_lr_pixel": 4 * 9 * 8 * 4 if a.scale == 2 else 2 * 4 * 9 * 8 * 4,
            "steps": a.steps,
            "seed": seed,
            "training_seconds": time.time() - start,
            "torch": torch.__version__,
            "data_manifest_sha256": hashlib.sha256((a.data / "manifest.json").read_bytes()).hexdigest(),
            "valid": evaluate(eval_sets(a.data, "valid", a.scale, device), a.scale,
                              {"v1": reference, "v2": model}, device),
        }
        if a.test:
            result["test"] = evaluate(eval_sets(a.data, "test", a.scale, device), a.scale,
                                      {"v1": reference, "v2": model}, device)
        frames = sorted((a.data / "real").glob("*.png"))
        if frames:
            result["real_crispness"] = crispness({"v1": reference, "v2": model}, a.scale, device, frames)
            print("real", json.dumps(result["real_crispness"]), flush=True)
        size = (960, 540) if a.scale == 2 else (480, 270)
        result["torch_fp16_ms"] = {"lr_size": size, "v2": timing(model, a.scale, device, size)}
        # Side-by-side sample from the validation docs.
        z = np.load(a.data / "valid-docs.npz")
        lo = as_float(torch.from_numpy(z["codec"][:4, 1]).to(device))
        rows = [F.interpolate(lo, scale_factor=a.scale, mode="bilinear", align_corners=False),
                reference(lo, a.scale), model(lo), as_float(torch.from_numpy(z[f"hi{a.scale}"][:4]).to(device))]
        grid = torch.cat([torch.cat(list(r.clamp(0, 1)), 1) for r in rows], 2)
        Image.fromarray(grid.permute(1, 2, 0).mul(255).round().byte().cpu().numpy()).save(
            out / "valid-bilinear-v1-v2-target.png")
    torch.save(model.state_dict(), out / "model.pt")
    (out / "metrics.json").write_text(json.dumps(result, indent=2) + "\n")
    for split in ["valid", "test"]:
        for set_name, row in result.get(split, {}).items():
            print(split, set_name, " ".join(f"{k}={v['psnr']:.2f}" for k, v in row.items()), flush=True)
    print("fp16 ms", result["torch_fp16_ms"], flush=True)


if __name__ == "__main__":
    main()
