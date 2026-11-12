"""Small 2x coverage/RGB models and exactly fusible edge parameterization."""

import copy

import torch
from torch import nn
import torch.nn.functional as F


class EdgeConv(nn.Conv2d):
    """An ordinary kernel plus learned coefficients of fixed derivative bases.

    The bases are added before convolution, including at replicated image edges.
    This is inspired by ECB, not an implementation of the complete ECBSR block.
    """

    def __init__(self, inputs, outputs):
        super().__init__(inputs, outputs, 3, padding=1, padding_mode="replicate")
        self.coefficients = nn.Parameter(torch.zeros(outputs, inputs, 4))
        self.register_buffer("bases", torch.tensor([
            [[-1., 0, 1], [-2, 0, 2], [-1, 0, 1]],
            [[-1., -2, -1], [0, 0, 0], [1, 2, 1]],
            [[0., 1, 0], [1, -4, 1], [0, 1, 0]],
            [[0., 0, 0], [0, 4, 0], [0, 0, 0]],
        ]) / 4)

    def kernel(self):
        return self.weight + torch.einsum("oik,khw->oihw", self.coefficients, self.bases)

    def forward(self, x):
        return F.conv2d(F.pad(x, (1, 1, 1, 1), mode="replicate"), self.kernel(), self.bias)

    def fused(self):
        conv = nn.Conv2d(self.in_channels, self.out_channels, 3, padding=1,
                         padding_mode="replicate").to(self.weight)
        with torch.no_grad():
            conv.weight.copy_(self.kernel())
            conv.bias.copy_(self.bias)
        return conv


class CoverageNet(nn.Module):
    """Shared 16-channel trunk; RGB residual and optional coverage/color heads.

    Coverage is scalar and shared across RGB. Six low-resolution channels give
    two RGB endpoints; four coverage and four gate channels are pixel-shuffled.
    The RGB fallback handles content that cannot be described by two colors.
    """

    def __init__(self, coverage=True, edges=True, channels=16):
        super().__init__()
        self.coverage = coverage
        conv = EdgeConv if edges else lambda i, o: nn.Conv2d(
            i, o, 3, padding=1, padding_mode="replicate")
        self.head = conv(3, channels)
        self.body = nn.ModuleList(conv(channels, channels) for _ in range(4))
        self.tail = nn.Conv2d(channels, 12, 3, padding=1, padding_mode="replicate")
        nn.init.zeros_(self.tail.weight)
        nn.init.zeros_(self.tail.bias)
        if coverage:
            self.geometry = nn.Conv2d(channels, 14, 3, padding=1, padding_mode="replicate")
            nn.init.zeros_(self.geometry.weight)
            nn.init.zeros_(self.geometry.bias)
            with torch.no_grad():
                self.geometry.bias[4:7] = 1.5
                self.geometry.bias[7:10] = -1.5
                self.geometry.bias[10:14] = -3

    def forward(self, x, auxiliary=False):
        first = self.head(x).relu()
        h = first
        for layer in self.body:
            h = layer(h).relu()
        h = h + first
        base = F.interpolate(x, scale_factor=2, mode="bilinear", align_corners=False)
        rgb = base + F.pixel_shuffle(self.tail(h), 2)
        if not self.coverage:
            return (rgb, {}) if auxiliary else rgb
        raw = self.geometry(h)
        alpha = F.pixel_shuffle(raw[:, :4], 2).sigmoid()
        colors = F.interpolate(raw[:, 4:10].sigmoid(), scale_factor=2,
                               mode="bilinear", align_corners=False)
        geometry = alpha * colors[:, :3] + (1 - alpha) * colors[:, 3:]
        gate = F.pixel_shuffle(raw[:, 10:14], 2).sigmoid()
        pred = gate * geometry + (1 - gate) * rgb
        if auxiliary:
            return pred, {"alpha": alpha, "colors": colors, "gate": gate,
                          "geometry": geometry, "rgb": rgb}
        return pred

    def fused(self):
        result = copy.deepcopy(self)
        if isinstance(result.head, EdgeConv):
            result.head = result.head.fused()
            result.body = nn.ModuleList(layer.fused() for layer in result.body)
        return result

    def parameters_count(self):
        return sum(p.numel() for p in self.parameters())

    def macs_per_input_pixel(self):
        return sum(m.weight.numel() for m in self.modules() if isinstance(m, nn.Conv2d))


VARIANTS = {"rgb": (False, False, 16), "rgb_edge": (False, True, 16),
            "coverage": (True, False, 16), "coverage_edge": (True, True, 16),
            "rgb_wide": (False, False, 18)}


def matched_models(seed=2604):
    models = {}
    for name, (coverage, edges, channels) in VARIANTS.items():
        torch.manual_seed(seed)
        models[name] = CoverageNet(coverage, edges, channels)
    return models
