#!/usr/bin/env python3
"""Validate fixed UI models on a second scene split unused for loss selection."""

import json, hashlib
from pathlib import Path
import train_ui_upscaler as t
import torch, numpy as np
import torch.nn.functional as F

torch.set_num_threads(4)
p = t.ROOT / "assets/upscaler"
valid = (
    torch.from_numpy(np.stack([t.scene(950000 + i) for i in range(64)]))
    .permute(0, 3, 1, 2)
    .float()
    / 255
)
models = [
    t.Net((p / f).read_bytes()).eval()
    for f in ["ui-cnn-2x.f32", "ui-cnn-4x-first.f32", "ui-cnn-4x.f32"]
]
metrics = {}
with torch.no_grad():
    for scale in [2, 4]:
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
        actual = (
            models[0](lo).clamp(0, 1)
            if scale == 2
            else models[2](models[1](lo).clamp(0, 1).mul(255).round().div(255)).clamp(
                0, 1
            )
        )
        values = {
            "psnr_bilinear": t.psnr(linear, valid),
            "psnr_ui": t.psnr(actual, valid),
            "edge_error_bilinear": t.edge_error(linear, valid),
            "edge_error_ui": t.edge_error(actual, valid),
            "stroke_contrast_error_bilinear": t.stroke_error(linear, valid),
            "stroke_contrast_error_ui": t.stroke_error(actual, valid),
        }
        print(scale, values, flush=True)
        assert values["psnr_ui"] > values["psnr_bilinear"] + 0.15
        assert values["edge_error_ui"] < values["edge_error_bilinear"]
        metrics[str(scale)] = values
meta = json.loads((p / "ui-training.json").read_text())
meta["test_seed_range"] = [950000, 950063]
meta["test_images"] = 64
meta["test_metrics"] = metrics
meta["test_protocol"] = (
    "Fresh procedural scenes, generated after objective selection; no optimization on these images."
)
meta["font_sha256"] = {
    f: hashlib.sha256((t.FONT_ROOT / f).read_bytes()).hexdigest()
    for f in t.FONTS + ["DejaVuSerif-Bold.ttf"]
}
meta["initialization_sha256"] = hashlib.sha256(
    (p / "anime4k-cnn-x2-s.f32").read_bytes()
).hexdigest()
meta["training_script_sha256"] = hashlib.sha256(
    Path(t.__file__).read_bytes()
).hexdigest()
meta["validation_script_sha256"] = hashlib.sha256(
    Path(__file__).read_bytes()
).hexdigest()
meta["requirements_sha256"] = hashlib.sha256(
    (t.ROOT / "tools/ui-upscaler-requirements.txt").read_bytes()
).hexdigest()
(p / "ui-training.json").write_text(json.dumps(meta, indent=2) + "\n")
