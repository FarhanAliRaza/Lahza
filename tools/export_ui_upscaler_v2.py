#!/usr/bin/env python3
"""Export trained UI CNN v2 runs to the shader's flat float32 layout.

Per layer, for each output group of four channels, for each 3×3 tap (row-major,
offset (tap % 3 - 1, tap / 3 - 1)), for each input group of four channels: one
column-major 4×4 matrix (column = input channel). The layer's biases follow as
one vec4 per output group. Missing channels (RGB input) are zero.
The export is checked against PyTorch with an independent NumPy evaluation of
that layout before anything is written.
"""

from pathlib import Path
import argparse, hashlib, json, sys

import numpy as np
import torch
import torch.nn.functional as F

sys.path.insert(0, str(Path(__file__).resolve().parent))
import train_ui_upscaler_v2 as v2


def groups(n):
    return (n + 3) // 4


def flat(model):
    out = []
    for conv in [model.head, *model.body, model.tail]:
        w = conv.weight.detach().cpu().numpy()  # [out, in, ky, kx]
        o, i = w.shape[:2]
        og, ig = groups(o), groups(i)
        padded = np.zeros((og * 4, ig * 4, 3, 3), np.float32)
        padded[:o, :i] = w
        # [og, tap, ig, column(in), row(out)]
        m = padded.reshape(og, 4, ig, 4, 9).transpose(0, 4, 2, 3, 1)
        out.append(m.reshape(-1))
        bias = np.zeros(og * 4, np.float32)
        bias[:o] = conv.bias.detach().cpu().numpy()
        out.append(bias)
    return np.concatenate(out).astype("<f4")


def numpy_forward(weights, x, scale, channels, layers):
    """Independent evaluation that reads only the flat layout."""
    h, w = x.shape[1:]
    pos = 0

    def conv(v, cin, cout):
        nonlocal pos
        ig, og = groups(cin), groups(cout)
        size = og * 9 * ig * 16
        m = weights[pos : pos + size].reshape(og, 9, ig, 4, 4)
        bias = weights[pos + size : pos + size + og * 4]
        pos += size + og * 4
        src = np.zeros((ig * 4, h, w), np.float32)
        src[:cin] = v
        pad = np.pad(src, ((0, 0), (1, 1), (1, 1)), mode="edge")
        result = np.zeros((og * 4, h, w), np.float32) + bias[:, None, None]
        for tap in range(9):
            dx, dy = tap % 3 - 1, tap // 3 - 1
            shifted = pad[:, 1 + dy : 1 + dy + h, 1 + dx : 1 + dx + w]
            for g in range(og):
                for k in range(ig):
                    # column j holds the weights of input channel k*4+j.
                    result[g * 4 : g * 4 + 4] += np.einsum("jr,jhw->rhw", m[g, tap, k], shifted[k * 4 : k * 4 + 4])
        return result[:cout]

    f = np.maximum(conv(x, 3, channels), 0)
    v = f
    for _ in range(layers - 2):
        v = np.maximum(conv(v, channels, channels), 0)
    assert pos + (groups(3 * scale * scale) * 9 * groups(channels) * 16 + groups(3 * scale * scale) * 4) == len(weights)
    return conv(v + f, channels, 3 * scale * scale)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("runs", nargs="+", type=Path)
    p.add_argument("--out", type=Path, default=v2.ROOT / "assets/upscaler")
    a = p.parse_args()
    for run in a.runs:
        metrics = json.loads((run / "metrics.json").read_text())
        scale, channels, layers = metrics["scale"], metrics["channels"], metrics["layers"]
        model = v2.Net(scale, channels, layers)
        model.load_state_dict(torch.load(run / "model.pt", map_location="cpu"))
        model.eval()
        weights = flat(model)
        x = torch.rand(1, 3, 23, 31, generator=torch.Generator().manual_seed(7))
        with torch.no_grad():
            f = F.relu(model.head(x))
            h = f
            for layer in model.body:
                h = F.relu(layer(h))
            expected = model.tail(h + f)[0].numpy()
        actual = numpy_forward(weights, x[0].numpy(), scale, channels, layers)
        error = float(np.abs(actual - expected).max())
        assert error < 1e-4, error
        name = f"ui-cnn-v2-{scale}x.f32"
        (a.out / name).write_bytes(weights.tobytes())
        print(name, len(weights), "floats", "layout max error", error,
              hashlib.sha256(weights.tobytes()).hexdigest())


if __name__ == "__main__":
    main()
