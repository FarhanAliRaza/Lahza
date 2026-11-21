"""Numerical invariants for the weight-edit experiment; runs on CPU."""

import unittest

import torch
import torch.nn.functional as F

import experiment_ui_upscaler_weights as experiment


class WeightEditTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(2)
        cls.model = experiment.baseline()
        cls.input = torch.rand(2, 3, 24, 31, generator=torch.Generator().manual_seed(42))

    @torch.no_grad()
    def test_edits_preserve_rgb_mean_and_source_weights(self):
        original = {name: value.clone() for name, value in self.model.state_dict().items()}
        before = self.model(self.input)
        for projection in ["rgb", "phase"]:
            edited = experiment.project(self.model, list(range(16)), 0.25, projection)
            self.assertTrue(torch.allclose(edited(self.input).mean(1), before.mean(1), atol=2e-7))
        for name, value in self.model.state_dict().items():
            self.assertTrue(torch.equal(value, original[name]))

    @torch.no_grad()
    def test_phase_projection_preserves_block_mean(self):
        edited = experiment.project(self.model, list(range(16)), 1.0, "phase")
        self.assertTrue(torch.allclose(
            F.avg_pool2d(edited(self.input), 2),
            F.avg_pool2d(self.model(self.input), 2), atol=2e-7,
        ))

    @torch.no_grad()
    def test_cached_inference_matches_full_inference(self):
        edited = experiment.project(self.model, list(range(16)), 0.25)
        cached = experiment.tail_prediction(edited, self.input, experiment.features(self.model, self.input))
        self.assertTrue(torch.equal(cached, edited(self.input)))

    def test_flat_gray_is_exact_in_output_units(self):
        for model in [self.model, experiment.project(self.model, list(range(16)), 0.25)]:
            self.assertEqual(experiment.diagnostics(model)["flat_max_error_8bit"], 0)

    def test_blank_targets_have_no_stroke_score(self):
        source = torch.zeros(1, 3, 4, 4)
        target = torch.zeros(1, 3, 8, 8)
        metrics = experiment.measurements(target, target, source)
        self.assertIsNone(metrics["stroke_error"])
        self.assertEqual(metrics["halo"], 0)


if __name__ == "__main__":
    unittest.main()
