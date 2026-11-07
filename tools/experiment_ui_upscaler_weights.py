#!/usr/bin/env python3
"""Reproduce a 2x v2 weight-edit experiment without changing shipped assets.

Search projects selected tail kernels toward RGB neutrality, preserving their
RGB mean at each pixel-shuffle phase. A second family averages chroma over
pixel-shuffle phases. No gradients are used for either family of edits.
A separate short Adam fine-tune uses the original reconstruction objective.
Selection uses validation crops only. The test command freezes both candidates
before opening held-out data; its results must not be used for another search.
"""

import argparse
import copy
import hashlib
import json
import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont
import torch
import torch.nn.functional as F

import train_ui_upscaler_v2 as v2
from export_ui_upscaler_v2 import flat, numpy_forward

ROOT = v2.ROOT
SEED = 2601103


def save_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def baseline():
    model = v2.Net(2, 16, 6).eval()
    shipped = (ROOT / "assets/upscaler/ui-cnn-v2-2x.f32").read_bytes()
    weights = np.frombuffer(shipped, dtype="<f4")
    offset = 0
    with torch.no_grad():
        for conv in [model.head, *model.body, model.tail]:
            outputs, inputs = conv.weight.shape[:2]
            og, ig = (outputs+3)//4, (inputs+3)//4
            count = og*9*ig*16
            packed = weights[offset:offset+count].reshape(og, 9, ig, 4, 4)
            kernel = packed.transpose(0, 4, 2, 3, 1).reshape(og*4, ig*4, 3, 3)
            conv.weight.copy_(torch.from_numpy(kernel[:outputs, :inputs].copy()))
            conv.bias.copy_(torch.from_numpy(weights[offset+count:offset+count+outputs].copy()))
            offset += count+og*4
    assert offset == len(weights)
    assert flat(model).tobytes() == shipped
    return model


def quantize(x):
    return x.clamp(0, 1).mul(255).round().div(255)


def features(model, x):
    head = model.head(x).relu()
    h = head
    for layer in model.body:
        h = layer(h).relu()
    return h + head


def tail_prediction(model, x, h):
    return F.interpolate(x, scale_factor=2, mode="bilinear", align_corners=False) + F.pixel_shuffle(model.tail(h), 2)


def project(model, channels, strength, projection="rgb"):
    edited = copy.deepcopy(model)
    with torch.no_grad():
        w = edited.tail.weight.view(3, 4, 16, 3, 3)
        for c in channels:
            kernel = w[:, :, c]
            chroma = kernel - kernel.mean(0, keepdim=True)
            # Phase averaging preserves the mean chroma correction of the 2x2
            # output block while reducing differences between its four pixels.
            correction = chroma if projection == "rgb" else chroma-chroma.mean(1, keepdim=True)
            kernel.sub_(strength * correction)
    return edited


def probe_inputs(heldout=False):
    size = 40 if heldout else 32
    low, high = (0.12, 0.91) if heldout else (0.2, 0.8)
    x = torch.full((8, 3, size, size), low)
    mid = size // 2
    x[0, :, :, mid:] = high
    x[1, :, mid:, :] = high
    x[2, :, :, mid] = high
    x[3, :, mid, :] = high
    x[4, :, :, mid-4:mid] = high
    x[4, :, :, mid+1:mid+5] = high
    for i in range(size):
        x[5, :, i, (size-1-i if heldout else i):] = high
    x[6, :, :, ::3 if heldout else 2] = high
    x[7] = torch.linspace(low, high, size).view(1, 1, size)
    return x


def gray_text(heldout=False):
    fonts = ["DejaVuSerif-Bold.ttf", "LiberationSerif-Regular.ttf"] if heldout else ["DejaVuSans.ttf", "DejaVuSansMono.ttf"]
    rows = []
    for name in fonts:
        font_path = next(Path("/usr/share/fonts/truetype").rglob(name))
        for size in [9, 12, 16, 21]:
            im = Image.new("RGB", (192, 48), (32, 32, 32))
            draw = ImageDraw.Draw(im)
            draw.text((3, 3), "{} 0O 1Il rn m <> /=", font=ImageFont.truetype(str(font_path), size), fill=(224, 224, 224))
            a = v2.as_float(torch.from_numpy(np.asarray(im).copy())[None])
            rows.extend([a[0], 1-a[0]])
    return torch.stack(rows)


def measurements(pred, target, source):
    pred = quantize(pred)
    mse = (pred-target).square().mean().item()
    upper = F.interpolate(F.max_pool2d(source, 3, 1, 1), scale_factor=2, mode="nearest")
    lower = F.interpolate(-F.max_pool2d(-source, 3, 1, 1), scale_factor=2, mode="nearest")
    chroma = lambda a: a-a.mean(1, keepdim=True)
    return {
        "mse": mse,
        "psnr": -10*math.log10(max(mse, 1e-15)),
        "edge_error": v2.v1.edge_error(pred, target),
        "stroke_error": v2.v1.stroke_error(pred, target) if (target[..., 1:]-target[..., :-1]).abs().gt(0.12).any() else None,
        "chroma_mse": (chroma(pred)-chroma(target)).square().mean().item(),
        "halo": ((pred-upper).clamp_min(0)+(lower-pred).clamp_min(0)).mean().item()*255,
    }


@torch.no_grad()
def diagnostics(model, heldout=False):
    out = {}
    for name, x in [("geometry", probe_inputs(heldout)), ("text", gray_text(heldout))]:
        pred = quantize(model(x))
        spread = (pred.amax(1)-pred.amin(1))*255
        out[name] = {"chroma_rms_8bit": spread.square().mean().sqrt().item(), "chroma_max_8bit": spread.max().item()}
    gray = torch.linspace(0, 1, 256).view(-1, 1, 1, 1).expand(-1, 3, 1, 1)
    out["flat_max_error_8bit"] = ((quantize(model(gray))*255).round()-(gray*255).round()).abs().max().item()
    # Translation equivariance is necessary for stable integer-pixel pans.
    x = torch.rand(1, 3, 40, 40, generator=torch.Generator().manual_seed(113))
    a, b = quantize(model(x)), quantize(model(torch.roll(x, 1, -1)))
    out["integer_pan_max_error_8bit"] = ((b[..., 16:-16, 18:-14]-a[..., 16:-16, 16:-16])*255).abs().max().item()
    return out


@torch.no_grad()
def motion_error(model):
    """Motion-compensated error for synthetic quarter-pixel text pans.

    This measures resampling sensitivity, not real encoded-video flicker.
    """
    x = gray_text(True)[4:5]
    width = x.shape[-1]
    previous = None
    errors = []
    for shift in np.arange(0, 2.01, 0.25):
        matrix = torch.tensor([[[1, 0, 2*shift/width], [0, 1, 0]]], dtype=torch.float32)
        grid = F.affine_grid(matrix, x.shape, align_corners=False)
        frame = F.grid_sample(x, grid, padding_mode="border", align_corners=False)
        pred = quantize(model(frame))
        if previous is not None:
            matrix[0, 0, 2] = 0.5/width
            grid = F.affine_grid(matrix, pred.shape, align_corners=False)
            warped = F.grid_sample(previous, grid, padding_mode="border", align_corners=False)
            errors.append((pred[..., 16:-16, 16:-16]-warped[..., 16:-16, 16:-16]).abs().mean().item()*255)
        previous = pred
    return sum(errors)/len(errors)


def validation_crops(data):
    result = {}
    rng = np.random.default_rng(SEED)
    for kind in ["docs", "scenes", "code"]:
        with np.load(data / f"valid-{kind}.npz") as z:
            hi = z["hi2"]
            for label, key in [("native", "lo"), ("native-vp8", "codec"), ("down-vp8", "down2codec")]:
                lo = z[key]
                if key == "codec":
                    lo = lo[:, 1]
                xs, ys = [], []
                for i in range(len(lo)):
                    y = int(rng.integers(8, lo.shape[1]-56))
                    x = int(rng.integers(8, lo.shape[2]-56))
                    xs.append(lo[i, y:y+48, x:x+48])
                    ys.append(hi[i, y*2:y*2+96, x*2:x*2+96])
                result[f"{kind}-{label}"] = (v2.as_float(torch.from_numpy(np.stack(xs))), v2.as_float(torch.from_numpy(np.stack(ys))))
    return result


@torch.no_grad()
def crop_scores(model, data, cached):
    return {name: measurements(tail_prediction(model, x, cached[name]), y, x) for name, (x, y) in data.items()}


def acceptable(scores, reference):
    return all(
        row["psnr"] >= reference[name]["psnr"]-0.05
        and row["edge_error"] <= reference[name]["edge_error"]*1.01
        and (row["stroke_error"] is None or row["stroke_error"] <= reference[name]["stroke_error"]*1.01)
        and row["chroma_mse"] <= reference[name]["chroma_mse"]*1.02
        and row["halo"] <= reference[name]["halo"]*1.03+0.002
        for name, row in scores.items()
    )


@torch.no_grad()
def search(args):
    model = baseline()
    data = validation_crops(args.data)
    cached = {name: features(model, x) for name, (x, _) in data.items()}
    reference = crop_scores(model, data, cached)
    before = diagnostics(model)
    candidates = []
    specs = [(list(range(16)), s) for s in [0.1, 0.25, 0.5, 1.0]]
    if args.projection == "rgb":
        specs += [([c], s) for c in range(16) for s in [0.25, 0.5, 1.0]]
    best = None
    for channels, strength in specs:
        edited = project(model, channels, strength, args.projection)
        diag = diagnostics(edited)
        score = crop_scores(edited, data, cached)
        gain = 1-diag["text"]["chroma_rms_8bit"]/before["text"]["chroma_rms_8bit"]
        ok = acceptable(score, reference) and gain >= 0.1 and diag["flat_max_error_8bit"] == 0
        row = {"projection": args.projection, "channels": channels, "strength": strength, "diagnostics": diag, "scores": score, "text_chroma_reduction": gain, "acceptable": ok}
        candidates.append(row)
        print("candidate", channels, strength, "text reduction", round(gain, 4), "accepted", ok, flush=True)
        if ok and (best is None or gain > best["text_chroma_reduction"]):
            best = row
    report = {"seed": SEED, "baseline_diagnostics": before, "baseline_scores": reference, "candidates": candidates, "selected": best}
    save_json(args.out / f"search-{args.projection}.json", report)
    # Compare both analytic families using validation evidence only.
    choices = [json.loads(path.read_text())["selected"] for path in sorted(args.out.glob("search-*.json"))]
    choices = [choice for choice in choices if choice is not None]
    best = max(choices, key=lambda row: row["text_chroma_reduction"]) if choices else None
    if best is not None:
        edited = project(model, best["channels"], best["strength"], best["projection"])
        torch.save(edited.state_dict(), args.out / "edited.pt")
        save_json(args.out / "selected.json", best)
        print("SELECTED", best["channels"], best["strength"], flush=True)
    else:
        print("No candidate passed the predeclared validation gates.", flush=True)


def training_patches(data, crops_per_source=1024):
    """Cache paired crops with the original 2x degradation proportions.

    Extract each NPZ array separately to bound memory; no validation/test inputs.
    """
    rng = np.random.default_rng(SEED)
    inputs, targets, cleans = [], [], []
    for source in ["docs", "scenes", "code"]:
        with np.load(data / f"train-{source}.npz") as z:
            lo = z["lo"]
            n, height, width, _ = lo.shape
            jobs = []
            for _ in range(crops_per_source):
                p = rng.random()
                if p < 0.55:
                    lk, ck, hk, factor, codec = ("down2codec" if p < 0.4 else "down2"), "down2", "hi2", 1, None
                elif p < 0.65:
                    lk, ck, hk, factor, codec = "hi2", "hi2", "hi4", 2, None
                else:
                    lk, ck, hk, factor, codec = ("lo" if p < 0.8 else "codec"), "lo", "hi2", 1, int(rng.integers(2))
                jobs.append((int(rng.integers(n)), int(rng.integers(height*factor-48+1)), int(rng.integers(width*factor-48+1)), lk, ck, hk, codec))
            banks = [np.empty((len(jobs), s, s, 3), np.uint8) for s in [48, 96, 48]]
            del lo
            for key in sorted({j[k] for j in jobs for k in [3, 4, 5]}):
                array = z[key]
                for i, (image, y, x, lk, ck, hk, codec) in enumerate(jobs):
                    for bank, wanted, factor in [(banks[0], lk, 1), (banks[1], hk, 2), (banks[2], ck, 1)]:
                        if key == wanted:
                            im = array[image, codec] if key == "codec" else array[image]
                            bank[i] = im[y*factor:(y+48)*factor, x*factor:(x+48)*factor]
                del array
            inputs.append(banks[0]); targets.append(banks[1]); cleans.append(banks[2])
    return tuple(v2.as_float(torch.from_numpy(np.concatenate(bank))) for bank in [inputs, targets, cleans])


def finetune(args):
    torch.manual_seed(SEED)
    model = baseline().train()
    lo, hi, clean = training_patches(args.data)
    optimizer = torch.optim.Adam(model.parameters(), lr=1e-5)
    for step in range(args.steps):
        ids = torch.randint(len(lo), (16,))
        x, y, c = lo[ids], hi[ids], clean[ids]
        flip = torch.rand(16, 1, 1, 1) < 0.5
        perm = torch.randperm(3)
        x, y, c = (torch.where(flip, 1-t, t)[:, perm] for t in [x, y, c])
        pred = model(x)
        loss = torch.sqrt((pred-y).square()+1e-6).mean() + v2.grad_l1(pred, y) + (F.avg_pool2d(pred, 2)-c).abs().mean()
        optimizer.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1)
        optimizer.step()
        if step % 50 == 0:
            print("fine-tune", step, loss.item(), flush=True)
    torch.save(model.eval().state_dict(), args.out / "finetuned.pt")
    save_json(args.out / "finetune.json", {"seed": SEED, "steps": args.steps, "lr": 1e-5, "batch": 16, "training_crops": len(lo), "objective": "unchanged Charbonnier + gradient L1 + backprojection L1"})


@torch.no_grad()
def evaluate(args):
    models = {"v2": baseline()}
    for label, filename in [("edited", "edited.pt"), ("finetuned", "finetuned.pt")]:
        if (args.out / filename).exists():
            model = baseline()
            model.load_state_dict(torch.load(args.out / filename, map_location="cpu", weights_only=True))
            models[label] = model
    if "edited" in models:
        assert all(torch.equal(value, models["v2"].state_dict()[key])
                   for key, value in models["edited"].state_dict().items() if not key.startswith("tail."))
    hashes = {name: hashlib.sha256(flat(model).tobytes()).hexdigest() for name, model in models.items()}
    save_json(args.out / f"{args.split}-frozen-models.json", hashes)
    result = {"hashes": hashes, "diagnostics": {name: diagnostics(m, args.split == "test") for name, m in models.items()}, "sets": {}}
    previous = args.out / f"{args.split}.json"
    if args.sources != ["docs", "scenes", "code"] and previous.exists():
        old = json.loads(previous.read_text(), parse_constant=lambda _: None)
        assert old["hashes"] == hashes, "Partial reevaluation requires identical frozen models"
        result["sets"] = old["sets"]
    for source in args.sources:
        with np.load(args.data / f"{args.split}-{source}.npz") as z:
            hi = z["hi2"]
            for label, key in [("native", "lo"), ("native-vp8", "codec"), ("down-vp8", "down2codec")]:
                lo = z[key]
                if key == "codec":
                    lo = lo[:, 1]
                sums = {}
                stroke_weights = {}
                for start in range(0, len(lo), 2):
                    x = v2.as_float(torch.from_numpy(lo[start:start+2]))
                    y = v2.as_float(torch.from_numpy(hi[start:start+2]))
                    predictions = {"bilinear": F.interpolate(x, scale_factor=2, mode="bilinear", align_corners=False)}
                    shared = features(models["v2"], x)
                    for name, model in models.items():
                        if name in {"v2", "edited"}:
                            predictions[name] = tail_prediction(model, x, shared)
                        else:
                            predictions[name] = model(x)
                    for name, pred in predictions.items():
                        row = sums.setdefault(name, {})
                        for metric, value in measurements(pred, y, x).items():
                            if value is None:
                                continue
                            row[metric] = row.get(metric, 0)+value*len(x)/len(lo)
                            if metric == "stroke_error":
                                stroke_weights[name] = stroke_weights.get(name, 0)+len(x)/len(lo)
                for name, row in sums.items():
                    row["psnr"] = -10*math.log10(row["mse"])
                    row["stroke_error"] = row.get("stroke_error", 0)/stroke_weights[name] if stroke_weights.get(name, 0) else None
                name = f"{source}-{label}"
                result["sets"][name] = sums
                print(name, {k: round(v["psnr"], 4) for k, v in sums.items()}, flush=True)
                save_json(args.out / f"{args.split}.json", result)
    frames = sorted((args.data / "real").glob("*.png"))
    if args.split == "test" and frames:
        result["real_crispness"] = v2.crispness(models, 2, "cpu", frames)
    if args.split == "test":
        result["synthetic_pan_error_8bit"] = {name: motion_error(m) for name, m in models.items()}
    save_json(args.out / f"{args.split}.json", result)
    if "edited" in models:
        m = models["edited"]
        weights = flat(m)
        x = torch.rand(1, 3, 23, 31, generator=torch.Generator().manual_seed(7))
        error = float(np.abs(numpy_forward(weights, x[0].numpy(), 2, 16, 6)-m.tail(features(m, x))[0].numpy()).max())
        assert error < 1e-4, error
        (args.out / "edited-2x.f32").write_bytes(weights.tobytes())
        save_json(args.out / "export.json", {"layout_max_error": error, "changed_parameters": sum(int((p != q).sum()) for p, q in zip(m.parameters(), models["v2"].parameters()))})
    # Fixed panels: nearest source, v2, optional edit, optional fine-tune.
    xs = [("diagonal", probe_inputs(args.split == "test")[5:6]), ("gray-text", gray_text(args.split == "test")[4:5])]
    for name, x in xs:
        panels = [F.interpolate(x, scale_factor=2, mode="nearest"), *[quantize(m(x)) for m in models.values()]]
        array = torch.cat(panels, -1)[0].permute(1, 2, 0).mul(255).round().byte().numpy()
        Image.fromarray(array).save(args.out / f"{args.split}-{name}.png")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("command", choices=["search", "finetune", "evaluate"])
    p.add_argument("--data", type=Path, default=ROOT / "ignore/upscaler-data")
    p.add_argument("--out", type=Path, default=ROOT / "ignore/upscaler-weight-edit")
    p.add_argument("--steps", type=int, default=300)
    p.add_argument("--projection", choices=["rgb", "phase"], default="rgb")
    p.add_argument("--split", choices=["valid", "test"], default="valid")
    p.add_argument("--sources", nargs="+", choices=["docs", "scenes", "code"], default=["docs", "scenes", "code"])
    args = p.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    torch.set_num_threads(4)
    if args.command in {"search", "finetune"} and (args.out / "test-frozen-models.json").exists():
        p.error("Test models are frozen in this output directory; do not tune on test results.")
    if args.command == "search" and (args.out / f"search-{args.projection}.json").exists():
        p.error("This search already exists; use a fresh --out directory for a new experiment.")
    provenance = args.out / "provenance.json"
    if not provenance.exists():
        save_json(provenance, {
            "torch": torch.__version__, "numpy": np.__version__, "device": "cpu", "seed": SEED,
            "data_manifest_sha256": hashlib.sha256((args.data / "manifest.json").read_bytes()).hexdigest(),
            "baseline_sha256": hashlib.sha256(flat(baseline()).tobytes()).hexdigest(),
        })
    {"search": search, "finetune": finetune, "evaluate": evaluate}[args.command](args)


if __name__ == "__main__":
    main()
