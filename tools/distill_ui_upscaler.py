#!/usr/bin/env python3
"""Train a UI teacher, then compare matched supervised/distilled 2x students.

All students retain the shipped 16-channel, six-layer architecture. Selection
uses validation only. The evaluation command freezes hashes before opening
test data. Generated typography uses disjoint fonts/seeds and exact renders.
"""

import argparse
import copy
import hashlib
import json
import random
import subprocess
import time
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont
import torch
from torch import nn
import torch.nn.functional as F

import experiment_ui_upscaler_weights as edit
import train_ui_upscaler_v2 as v2
import build_upscaler_data as render
from export_ui_upscaler_v2 import flat, numpy_forward

ROOT = v2.ROOT
SEED = 2601104


def json_write(path, data):
    path.write_text(json.dumps(data, indent=2, allow_nan=False)+"\n")


def save_model(model, path):
    torch.save({k: v.detach().cpu() for k, v in model.state_dict().items()}, path)


def teacher_init():
    source = v2.Net(2, 24, 7)
    source.load_state_dict(torch.load(ROOT / "ignore/upscaler-v2-runs/2x-c24-l7-sharp/model.pt", map_location="cpu", weights_only=True))
    teacher = v2.Net(2, 64, 10)
    with torch.no_grad():
        teacher.head.weight[:24].copy_(source.head.weight)
        teacher.head.bias[:24].copy_(source.head.bias)
        for i, layer in enumerate(teacher.body):
            if i < len(source.body):
                layer.weight[:24].zero_()
                layer.weight[:24, :24].copy_(source.body[i].weight)
                layer.bias[:24].copy_(source.body[i].bias)
            else:
                layer.weight.zero_()
                layer.bias.zero_()
                layer.weight[:, :, 1, 1].copy_(torch.eye(64))
        teacher.tail.weight.zero_()
        teacher.tail.weight[:, :24].copy_(source.tail.weight)
        teacher.tail.bias.copy_(source.tail.bias)
        x = torch.rand(2, 3, 21, 25)
        assert (teacher(x)-source(x)).abs().max() < 1e-6
    return teacher


def codec(frames, quality=28, keyframe=1):
    n, h, w, _ = frames.shape
    encoded = subprocess.run([
        "ffmpeg", "-v", "error", "-threads", "2", "-f", "rawvideo", "-pix_fmt", "rgb24",
        "-s", f"{w}x{h}", "-r", "30", "-i", "pipe:0", "-an", "-c:v", "libvpx",
        "-threads", "2", "-deadline", "realtime", "-cpu-used", "8", "-g", str(keyframe),
        "-qmin", str(quality), "-qmax", str(quality), "-b:v", "50M", "-pix_fmt", "yuv420p",
        "-f", "webm", "pipe:1",
    ], input=frames.tobytes(), capture_output=True, check=True).stdout
    decoded = subprocess.run([
        "ffmpeg", "-v", "error", "-threads", "2", "-i", "pipe:0", "-f", "rawvideo",
        "-pix_fmt", "rgb24", "pipe:1",
    ], input=encoded, capture_output=True, check=True).stdout
    return np.frombuffer(decoded, np.uint8).reshape(n, h, w, 3).copy()


def glyph(seed, scale, fonts, size=48):
    rng = random.Random(seed)
    dark = rng.random() < .5
    bg = rng.randint(12, 40) if dark else rng.randint(220, 250)
    fg = rng.randint(190, 240) if dark else rng.randint(15, 65)
    color = (fg, fg, fg) if rng.random() < .75 else tuple(rng.randint(60, 200) for _ in range(3))
    image = Image.new("RGB", (size*scale, size*scale), (bg,)*3)
    draw = ImageDraw.Draw(image)
    font_path = render.font_path(rng.choice(fonts))
    y = rng.randint(0, 3)
    while y < size-8:
        px = rng.randint(8, 19)
        font = ImageFont.truetype(str(font_path), px*scale)
        text = rng.choice(["Il1!|", "0O8B", "rn m", "{}[]()", "aegop", "<>/=+", "1.25px", "const", "f(x)"])
        draw.text((rng.randint(1, 4)*scale, y*scale), text, font=font, fill=color)
        y += px+4
    return np.asarray(image).copy()


def generated(split, count, size=48):
    first = {"train": 2100000, "valid": 2200000, "test": 2300000}[split]
    fonts = render.TRAIN_FONTS if split == "train" else render.HELDOUT_FONTS
    lo = np.stack([glyph(first+i, 1, fonts, size) for i in range(count)])
    hi = np.stack([glyph(first+i, 2, fonts, size) for i in range(count)])
    return lo, hi, codec(lo)


def prepare(args):
    lo, hi, clean = edit.training_patches(args.data, args.crops_per_source)
    native, target, compressed = generated("train", 1024)
    extra_lo = native.copy()
    extra_lo[::2] = compressed[::2]
    arrays = [torch.cat([a, v2.as_float(torch.from_numpy(b))]) for a, b in [(lo, extra_lo), (hi, target), (clean, native)]]
    np.savez(args.out / "training.npz", **{name: a.mul(255).round().byte().numpy() for name, a in zip(["lo", "hi", "clean"], arrays)})
    native, target, compressed = generated("valid", 96, 96)
    np.savez(args.out / "glyph-valid.npz", lo=native, hi=target, codec=compressed)
    json_write(args.out / "data.json", {
        "seed": SEED, "original_crop_seed": edit.SEED, "original_training_crops": args.crops_per_source*3, "generated_training_pairs": 1024,
        "training_seed_start": 2100000, "validation_seed_start": 2200000, "test_seed_start": 2300000,
        "train_fonts": render.TRAIN_FONTS, "heldout_fonts": render.HELDOUT_FONTS,
        "generated_codec": "VP8 realtime, yuv420p, q=28, g=1",
        "manifest_sha256": hashlib.sha256((args.data / "manifest.json").read_bytes()).hexdigest(),
    })
    print(f"Prepared {args.crops_per_source*3+1024} training crops and 96 validation glyph pairs", flush=True)


def validation(args):
    data = edit.validation_crops(args.data)
    rng = np.random.default_rng(edit.SEED)
    for kind in ["docs", "scenes", "code"]:
        with np.load(args.data / f"valid-{kind}.npz") as z:
            native = z["lo"]
            for label in ["native", "native-vp8", "down-vp8"]:
                clean = z["down2"] if label == "down-vp8" else native
                crops = []
                for im in clean:
                    y = int(rng.integers(8, im.shape[0]-56))
                    x = int(rng.integers(8, im.shape[1]-56))
                    crops.append(im[y:y+48, x:x+48])
                name = f"{kind}-{label}"
                data[name] = (*data[name], v2.as_float(torch.from_numpy(np.stack(crops))))
    with np.load(args.out / "glyph-valid.npz") as z:
        target = v2.as_float(torch.from_numpy(z["hi"]))
        for label, key in [("native", "lo"), ("vp8", "codec")]:
            data[f"glyph-{label}"] = (v2.as_float(torch.from_numpy(z[key])), target, v2.as_float(torch.from_numpy(z["lo"])))
    return data


@torch.no_grad()
def assess(model, data, device):
    model.eval()
    result = {}
    for name, pair in data.items():
        x, y = pair[:2]
        clean = pair[2] if len(pair) == 3 else x
        predictions = []
        for start in range(0, len(x), 8):
            predictions.append(model(x[start:start+8].to(device)).cpu())
        pred = torch.cat(predictions)
        row = edit.measurements(pred, y, x)
        row["source_gradient_error"] = float(v2.grad_l1(F.avg_pool2d(edit.quantize(pred), 2), clean))
        row["edge_energy"] = float(((edit.quantize(pred)[..., 1:]-edit.quantize(pred)[..., :-1]).abs().mean() + (edit.quantize(pred)[..., 1:, :]-edit.quantize(pred)[..., :-1, :]).abs().mean())*255)
        result[name] = row
    return result


def score(rows, base, preserve_source=False):
    values = []
    for name, row in rows.items():
        b = base[name]
        value = row["edge_error"]/b["edge_error"] + .25*row["mse"]/b["mse"]
        if b["stroke_error"]:
            value += .5*row["stroke_error"]/b["stroke_error"]
        if preserve_source:
            value += .5*row["source_gradient_error"]/b["source_gradient_error"]
        # Penalize accuracy and halo regressions rather than reward overshoot.
        value += 5*max(0, b["psnr"]-row["psnr"]-.15)
        value += max(0, row["halo"]-b["halo"]*1.1-.02)
        values.append(value)
    return sum(values)/len(values)


def reconstruction(pred, target, clean, source_gradient=0, edge_weight=1.5, halo_weight=.2):
    upper = F.interpolate(F.max_pool2d(clean, 3, 1, 1), scale_factor=2, mode="nearest")
    lower = F.interpolate(-F.max_pool2d(-clean, 3, 1, 1), scale_factor=2, mode="nearest")
    halo = ((pred-upper).relu()+(lower-pred).relu()).mean()
    return (torch.sqrt((pred-target).square()+1e-6).mean()
            + edge_weight*v2.grad_l1(pred, target)
            + (F.avg_pool2d(pred, 2)-clean).abs().mean() + halo_weight*halo
            + source_gradient*v2.grad_l1(F.avg_pool2d(pred, 2), clean))


def batch(bank, generator, count=32, grayscale_rate=.25):
    ids = torch.randint(len(bank[0]), (count,), generator=generator).to(bank[0].device)
    values = [a[ids].float()/255 for a in bank]
    flip = (torch.rand(count, 1, 1, 1, generator=generator)<.5).to(bank[0].device)
    gray = (torch.rand(count, 1, 1, 1, generator=generator)<grayscale_rate).to(bank[0].device)
    perm = torch.randperm(3, generator=generator).to(bank[0].device)
    return [torch.where(gray, t.mean(1, keepdim=True), t) for t in [torch.where(flip, 1-a, a)[:, perm] for a in values]]


def train(args):
    assert torch.cuda.is_available(), "CUDA required for the training run"
    device = "cuda"
    torch.manual_seed(SEED)
    torch.backends.cudnn.benchmark = True
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    val = validation(args)
    base = assess(edit.baseline().to(device), val, device)
    json_write(args.out / "baseline-validation.json", base)
    with np.load(args.out / "training.npz") as z:
        bank = [torch.from_numpy(z[key][-1024:] if args.streaming else z[key]).to(device) for key in ["lo", "hi", "clean"]]
    sets = []
    if args.streaming:
        torch.cuda.empty_cache()
        for source in ["docs", "scenes", "code"]:
            with np.load(args.data / f"train-{source}.npz") as z:
                sets.append({key:torch.from_numpy(z[key]).to(device) for key in ["lo","hi2","hi4","codec","down2","down2codec"]})
        print("Full training images loaded; GPU allocated GB",round(torch.cuda.memory_allocated()/1e9,2),flush=True)
    history = []
    stages = [("teacher", args.teacher_steps)] if args.stage == "teacher" else [("students", args.student_steps)]
    for stage, steps in stages:
        if stage == "teacher":
            models = {"teacher": teacher_init().to(device)}
            if args.teacher_start:
                models["teacher"].load_state_dict(torch.load(args.teacher_start, map_location=device, weights_only=True))
            teacher = None
            lr = 2e-4
        else:
            teacher = v2.Net(2, 64, 10).to(device).eval()
            teacher.load_state_dict(torch.load(args.out / "teacher.pt", map_location=device, weights_only=True))
            teacher.requires_grad_(False)
            if args.scratch:
                torch.manual_seed(args.init_seed)
            initial = v2.Net(2,16,6) if args.scratch else edit.baseline()
            models = {"supervised": copy.deepcopy(initial).to(device), "distilled": copy.deepcopy(initial).to(device)}
            lr = 5e-5
        if args.lr is not None:
            lr = args.lr
        optimizers = {name: torch.optim.Adam(model.parameters(), lr=lr) for name, model in models.items()}
        schedulers = {name: (torch.optim.lr_scheduler.OneCycleLR(opt,max_lr=lr,total_steps=steps,pct_start=.05) if args.schedule == "onecycle" else torch.optim.lr_scheduler.CosineAnnealingLR(opt, steps, eta_min=lr*.05)) for name, opt in optimizers.items()}
        scalers = {name: torch.amp.GradScaler("cuda",enabled=not args.fp32_students) for name in models}
        best = {}
        generator = torch.Generator().manual_seed(SEED)
        start_time = time.monotonic()
        for name, model in models.items():
            rows = assess(model, val, device)
            best[name] = score(rows, base, args.source_gradient > 0)
            save_model(model, args.out / f"{name}.pt")
            json_write(args.out / f"{name}-validation.json", {"step": 0, "score": best[name], "rows": rows})
        for step in range(1, steps+1):
            if sets:
                original = v2.batch(sets, 2, 48, 30, generator)
                extra = batch(bank, generator, 2, args.grayscale_rate)
                x, target, clean = [torch.cat([a,b]) for a,b in zip(original,extra)]
            else:
                x, target, clean = batch(bank, generator, grayscale_rate=args.grayscale_rate)
            if teacher is not None:
                with torch.no_grad(), torch.autocast("cuda", dtype=torch.float16):
                    guidance = teacher(x).float()
            for name, model in models.items():
                model.train()
                optimizers[name].zero_grad(set_to_none=True)
                with torch.autocast("cuda", dtype=torch.float16,enabled=not args.fp32_students):
                    pred = model(x).float()
                    loss = reconstruction(pred, target, clean, args.source_gradient, args.edge_weight, args.halo_weight)
                    if name == "distilled":
                        loss += args.kd_weight*(.5*(pred-guidance).abs().mean()+.5*v2.grad_l1(pred, guidance))
                scalers[name].scale(loss).backward()
                scalers[name].unscale_(optimizers[name])
                nn.utils.clip_grad_norm_(model.parameters(), 1.)
                scalers[name].step(optimizers[name])
                scalers[name].update()
                schedulers[name].step()
            if step % 250 == 0:
                print(stage, step, "seconds", round(time.monotonic()-start_time, 1), "loss", float(loss.detach()), flush=True)
            if step % 1000 == 0 or step == steps:
                for name, model in models.items():
                    rows = assess(model, val, device)
                    value = score(rows, base, args.source_gradient > 0)
                    row = {"name": name, "step": step, "score": value, "rows": rows}
                    history.append(row)
                    if value < best[name]:
                        best[name] = value
                        save_model(model, args.out / f"{name}.pt")
                        json_write(args.out / f"{name}-validation.json", row)
                    print("validation", name, step, round(value, 6), "best", round(best[name], 6), flush=True)
                json_write(args.out / f"{stage}-history.json", history)
        for name, model in models.items():
            save_model(model, args.out / f"{name}-last.pt")
        json_write(args.out / f"{stage}-options.json", {k:str(v) if isinstance(v,Path) else v for k,v in vars(args).items()})
        json_write(args.out / f"{stage}-training.json", {
            "seed": SEED, "init_seed":args.init_seed if args.scratch else None,
            "steps": steps, "seconds":time.monotonic()-start_time, "lr":lr,
            "batch":32, "torch":torch.__version__, "device":torch.cuda.get_device_name(),
            "parameters":{n:m.parameters_count() for n,m in models.items()},
            "base_objective":{"charbonnier":1,"gradient_l1":args.edge_weight,
                              "backprojection_l1":1,"halo":args.halo_weight},
            "source_gradient_weight":args.source_gradient,
            "teacher_start":str(args.teacher_start) if args.teacher_start else None,
            "distillation":"0.5 output L1 + 0.5 gradient L1", "kd_weight":args.kd_weight,
            "scratch":args.scratch, "streaming":args.streaming,"schedule":args.schedule,
            "fp32_students":args.fp32_students,
            "augmentation":{"inversion":True,"RGB_permutation":True,"grayscale_rate":args.grayscale_rate},
        })


def load_models(args, device):
    models = {"v2": edit.baseline().to(device)}
    for name in ["teacher", "supervised", "distilled"]:
        model = v2.Net(2, 64, 10) if name == "teacher" else v2.Net(2, 16, 6)
        model.load_state_dict(torch.load(args.out / f"{name}.pt", map_location="cpu", weights_only=True))
        models[name] = model.to(device).eval()
    return models


@torch.no_grad()
def encoded_pan(models, device):
    """Compare temporal error against known HR motion after interframe VP8."""
    images = [glyph(2500000, scale, render.HELDOUT_FONTS, 96) for scale in [1, 2]]
    sequences = []
    for array in images:
        source = v2.as_float(torch.from_numpy(array)[None])
        frames = []
        for i in range(24):
            # Equal normalized translations correspond to 0.25 LR / 0.5 HR px.
            matrix = torch.tensor([[[1, 0, 2*(i*.25)/96], [0, 1, 0]]], dtype=torch.float32)
            grid = F.affine_grid(matrix, source.shape, align_corners=False)
            frames.append(F.grid_sample(source, grid, padding_mode="border", align_corners=False)[0])
        sequences.append(torch.stack(frames))
    low, high = sequences
    raw = low.permute(0, 2, 3, 1).mul(255).round().byte().numpy()
    compressed = v2.as_float(torch.from_numpy(codec(raw, keyframe=60)))
    matrix = torch.tensor([[[1, 0, .5/96], [0, 1, 0]]], dtype=torch.float32).expand(23, -1, -1)
    grid = F.affine_grid(matrix, high[:-1].shape, align_corners=False)
    true_change = high[1:]-F.grid_sample(high[:-1], grid, padding_mode="border", align_corners=False)
    errors = {}
    for name, model in models.items():
        pred = torch.cat([edit.quantize(model(x.to(device))).cpu() for x in compressed.split(4)])
        change = pred[1:]-F.grid_sample(pred[:-1], grid, padding_mode="border", align_corners=False)
        errors[name] = float((change-true_change)[..., 16:-16, 16:-16].abs().mean()*255)
    return errors


@torch.no_grad()
def evaluate(args):
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    device = "cuda" if torch.cuda.is_available() else "cpu"
    models = load_models(args, device)
    hashes = {name: hashlib.sha256(flat(model).tobytes()).hexdigest() for name, model in models.items()}
    frozen = args.out / "frozen.json"
    if frozen.exists():
        assert json.loads(frozen.read_text()) == hashes, "Test candidates have already been frozen"
    json_write(frozen, hashes)
    result = {"hashes": hashes, "sets": {}, "diagnostics": {}, "synthetic_pan_error_8bit": {}}
    for name, model in models.items():
        model.cpu()
        result["diagnostics"][name] = edit.diagnostics(model, True)
        result["synthetic_pan_error_8bit"][name] = edit.motion_error(model)
        model.to(device)
    # Test sets are opened only after all candidate hashes are written.
    for source in ["docs", "scenes", "code"]:
        with np.load(args.data / f"test-{source}.npz") as z:
            target = z["hi2"]
            for suffix, key in [("native", "lo"), ("native-vp8", "codec"), ("down-vp8", "down2codec")]:
                lo = z[key]
                if key == "codec":
                    lo = lo[:, 1]
                clean_array = z["down2"] if suffix == "down-vp8" else z["lo"]
                scores = {name: {} for name in models}
                weights = {name: {} for name in models}
                for start in range(0, len(lo), 4):
                    x = v2.as_float(torch.from_numpy(lo[start:start+4]))
                    y = v2.as_float(torch.from_numpy(target[start:start+4]))
                    clean = v2.as_float(torch.from_numpy(clean_array[start:start+4]))
                    for name, model in models.items():
                        pred = model(x.to(device)).cpu()
                        row = edit.measurements(pred, y, x)
                        row["source_gradient_error"] = float(v2.grad_l1(F.avg_pool2d(edit.quantize(pred), 2), clean))
                        for key2, value in row.items():
                            if value is not None:
                                scores[name][key2] = scores[name].get(key2, 0)+value*len(x)
                                weights[name][key2] = weights[name].get(key2, 0)+len(x)
                for name in models:
                    scores[name] = {k:v/weights[name][k] for k,v in scores[name].items()}
                    scores[name]["psnr"] = float(-10*np.log10(scores[name]["mse"]))
                label = f"{source}-{suffix}"
                result["sets"][label] = scores
                print(label, {n: round(s["psnr"], 4) for n,s in scores.items()}, flush=True)
                json_write(args.out / "test.json", result)
    native, target, compressed = generated("test", 128, 96)
    fresh = {f"fresh-glyph-{name}": (v2.as_float(torch.from_numpy(x)), v2.as_float(torch.from_numpy(target)), v2.as_float(torch.from_numpy(native))) for name,x in [("native",native),("vp8",compressed)]}
    for name, model in models.items():
        rows = assess(model, fresh, device)
        for label,row in rows.items():
            result["sets"].setdefault(label, {})[name] = row
    # Entire procedural UI scenes, with seeds unused by either training stage.
    fresh_lo = np.stack([render.scene(2400000+i, 1, render.HELDOUT_FONTS) for i in range(32)])
    fresh_hi = np.stack([render.scene(2400000+i, 2, render.HELDOUT_FONTS) for i in range(32)])
    fresh_ui = {"fresh-ui-native": (v2.as_float(torch.from_numpy(fresh_lo)), v2.as_float(torch.from_numpy(fresh_hi)), v2.as_float(torch.from_numpy(fresh_lo))), "fresh-ui-vp8": (v2.as_float(torch.from_numpy(codec(fresh_lo))), v2.as_float(torch.from_numpy(fresh_hi)), v2.as_float(torch.from_numpy(fresh_lo)))}
    for name, model in models.items():
        for label,row in assess(model, fresh_ui, device).items():
            result["sets"].setdefault(label, {})[name] = row
    result["encoded_pan_excess_error_8bit"] = encoded_pan(models, device)
    cpu_models = {name:model.cpu() for name,model in models.items()}
    frames = sorted((args.data / "real").glob("*.png"))
    if frames:
        result["real_crispness"] = v2.crispness(cpu_models, 2, "cpu", frames)
    json_write(args.out / "test.json", result)
    for name in ["supervised", "distilled"]:
        model = cpu_models[name]
        weights = flat(model)
        x = torch.rand(1, 3, 23, 31, generator=torch.Generator().manual_seed(7))
        error = float(np.abs(numpy_forward(weights, x[0].numpy(), 2, 16, 6)-model.tail(edit.features(model,x))[0].numpy()).max())
        assert error < 1e-4
        (args.out / f"{name}-2x.f32").write_bytes(weights.tobytes())
        json_write(args.out / f"{name}-export.json", {"layout_max_error":error, "parameters":model.parameters_count(), "sha256":hashes[name]})
    # Labels and identical nearest-neighbor magnification make the comparison reviewable.
    for label, pair in {**fresh, **fresh_ui}.items():
        x,y = pair[:2]
        for index in [0,7]:
            a,b = x[index:index+1],y[index:index+1]
            panels = [("Input (nearest)",F.interpolate(a,scale_factor=2,mode="nearest")), *[(name,edit.quantize(model(a))) for name,model in cpu_models.items()], ("HR target",b)]
            tiles=[]
            for title,pred in panels:
                im = Image.fromarray(pred[0].permute(1,2,0).mul(255).round().byte().numpy()).resize((pred.shape[-1]*2,pred.shape[-2]*2),Image.Resampling.NEAREST)
                tile = Image.new("RGB",(im.width,im.height+24),(245,245,245))
                tile.paste(im,(0,24)); ImageDraw.Draw(tile).text((6,5),title,fill=(20,20,20))
                tiles.append(tile)
            canvas = Image.new("RGB",(tiles[0].width*3,tiles[0].height*2))
            for i,tile in enumerate(tiles):canvas.paste(tile,((i%3)*tile.width,(i//3)*tile.height))
            canvas.save(args.out/f"compare-{label}-{index}.png")
    print("Evaluation, candidates, and comparisons saved",flush=True)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("command",choices=["prepare","train","evaluate"])
    p.add_argument("--stage",choices=["teacher","students"],default="teacher")
    p.add_argument("--teacher-steps",type=int,default=8000)
    p.add_argument("--student-steps",type=int,default=6000)
    p.add_argument("--source-gradient",type=float,default=0)
    p.add_argument("--teacher-start",type=Path)
    p.add_argument("--lr",type=float)
    p.add_argument("--kd-weight",type=float,default=1)
    p.add_argument("--scratch",action="store_true")
    p.add_argument("--crops-per-source",type=int,default=1024)
    p.add_argument("--schedule",choices=["cosine","onecycle"],default="cosine")
    p.add_argument("--streaming",action="store_true")
    p.add_argument("--fp32-students",action="store_true")
    p.add_argument("--grayscale-rate",type=float,default=.25)
    p.add_argument("--edge-weight",type=float,default=1.5)
    p.add_argument("--halo-weight",type=float,default=.2)
    p.add_argument("--init-seed",type=int,default=2604)
    p.add_argument("--data",type=Path,default=ROOT/"ignore/upscaler-data")
    p.add_argument("--out",type=Path,default=ROOT/"ignore/upscaler-distill")
    args=p.parse_args(); args.out.mkdir(parents=True,exist_ok=True)
    torch.set_num_threads(4)
    if args.command != "evaluate" and (args.out/"frozen.json").exists():
        p.error("Candidates are frozen; do not tune against test results")
    {"prepare":prepare,"train":train,"evaluate":evaluate}[args.command](args)


if __name__=="__main__":
    main()
