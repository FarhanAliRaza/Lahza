"""Numerical checks for edge fusion, coverage targets and tiled inference."""

import unittest

import numpy as np
import torch

import build_ui_coverage_data as data
from ui_coverage_model import EdgeConv, CoverageNet, matched_models


class CoverageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(2)
        torch.manual_seed(47)

    def test_edge_fusion_including_borders_and_input_gradient(self):
        layer = EdgeConv(3, 7)
        with torch.no_grad():
            layer.coefficients.normal_(0, .1)
        x = torch.randn(2, 3, 11, 17, requires_grad=True)
        original = layer(x)
        original.square().sum().backward()
        gradient = x.grad.clone()
        self.assertGreater(layer.coefficients.grad.abs().sum(), 0)
        x.grad.zero_()
        fused = layer.fused()(x)
        fused.square().sum().backward()
        torch.testing.assert_close(original, fused, atol=1e-6, rtol=1e-6)
        torch.testing.assert_close(gradient, x.grad, atol=1e-5, rtol=1e-5)

    def test_matched_initial_trunk_and_rgb_head(self):
        models = matched_models()
        reference = models['rgb']
        for name in ['rgb_edge', 'coverage', 'coverage_edge']:
            model = models[name].fused()
            for a, b in zip([reference.head, *reference.body, reference.tail],
                            [model.head, *model.body, model.tail]):
                self.assertTrue(torch.equal(a.weight, b.weight))
                self.assertTrue(torch.equal(a.bias, b.bias))

    @torch.no_grad()
    def test_fused_model_and_tiles_match_full_image(self):
        model = CoverageNet().eval()
        for p in model.parameters():
            p.add_(torch.randn_like(p)*.01)
        x = torch.rand(1, 3, 23, 31)
        expected = model(x)
        model = model.fused()
        torch.testing.assert_close(model(x), expected, atol=1e-6, rtol=1e-6)
        tiled = torch.empty_like(expected)
        for y in range(0, 23, 9):
            for xx in range(0, 31, 11):
                y1, x1 = min(y+9, 23), min(xx+11, 31)
                sy, sx, ey, ex = max(y-8, 0), max(xx-8, 0), min(y1+8, 23), min(x1+8, 31)
                pred = model(x[..., sy:ey, sx:ex])
                tiled[..., y*2:y1*2, xx*2:x1*2] = pred[..., (y-sy)*2:(y1-sy)*2, (xx-sx)*2:(x1-sx)*2]
        torch.testing.assert_close(tiled, expected, atol=2e-6, rtol=1e-6)

    def test_coverage_targets_recompose_and_preserve_hinted_outline(self):
        for seed in [3100001, 3100005, 3200002]:
            native, down, hi, alpha, colors = data.example(seed, data.render.HELDOUT_FONTS)
            pred = alpha.astype(float)/255*colors[:3] + (1-alpha.astype(float)/255)*colors[3:]
            self.assertLessEqual(np.abs(np.rint(pred)-hi).max(), 1)
            self.assertGreater(colors[:3].mean(), colors[3:].mean())
            self.assertLess(np.abs(native.astype(float)-down).mean(), 5)
            self.assertEqual(hi.shape, (96, 96, 3))


if __name__ == '__main__':
    unittest.main()
