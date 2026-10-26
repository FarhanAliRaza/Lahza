#!/usr/bin/env python3
"""Reproduce Lahza's TF-Restormer ONNX conversion from pinned upstream files.

Developer-only: needs torch, PyYAML, onnx, numpy and upstream dependencies.
Run sequentially, separately from application builds and quality validation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--threads", type=int, default=2)
    args = parser.parse_args()

    import numpy as np
    import onnx
    from onnx import helper, numpy_helper, TensorProto
    import torch
    import yaml

    torch.set_num_threads(max(1, args.threads))
    sys.path.insert(0, str(args.source.resolve()))
    from tf_restormer.models.TF_Restormer.model import Model

    checkpoint_hash = hashlib.sha256(args.checkpoint.read_bytes()).hexdigest()
    config_hash = hashlib.sha256(args.config.read_bytes()).hexdigest()
    expected_checkpoint = "d67115273bed06cb1ab06de57168e6ce6e56dfcf3c626b3e3fbc6eb945b5424e"
    expected_config = "d94e7509dfa84a09d76a81a71a4861fc8cf3684b753d45eb24b7ff5ef27daf8e"
    if checkpoint_hash != expected_checkpoint or config_hash != expected_config:
        raise ValueError("Expected the pinned corrected epoch-19 checkpoint/config")
    model = Model(**yaml.safe_load(args.config.read_text())["config"]["model"]).eval()
    state = torch.load(args.checkpoint, map_location="cpu", weights_only=True)
    for key in ("model_state_dict", "model", "state_dict"):
        if key in state:
            state = state[key]
    model.load_state_dict(state, strict=True)
    model.band_detect_enable = False
    for module in model.modules():
        if hasattr(module, "cache_if_possible"):
            module.cache_if_possible = False

    class FixedOutput(torch.nn.Module):
        def __init__(self, inner):
            super().__init__()
            self.model = inner

        def forward(self, spectrum):
            return self.model(spectrum, 961)

    args.output.mkdir(parents=True, exist_ok=True)
    full_path = args.output / "tf-restormer.onnx"
    packed_path = args.output / "tf-restormer-weights-f16.onnx"
    torch.manual_seed(0)
    with torch.no_grad():
        torch.onnx.export(
            FixedOutput(model),
            (torch.randn(1, 321, 51, 2) * 0.01,),
            full_path,
            input_names=["spectrum"],
            output_names=["restored"],
            dynamic_axes={"spectrum": {1: "frequency", 2: "frames"}, "restored": {2: "frames"}},
            opset_version=17,
            dynamo=False,
            do_constant_folding=True,
        )
    graph = onnx.load(full_path)
    for node in graph.graph.node:
        if node.op_type == "Transpose":
            for attribute in node.attribute:
                if attribute.name == "perm" and any(axis < 0 for axis in attribute.ints):
                    rank = len(attribute.ints)
                    fixed = [axis + rank if axis < 0 else axis for axis in attribute.ints]
                    del attribute.ints[:]
                    attribute.ints.extend(fixed)
    onnx.checker.check_model(graph)
    onnx.save(graph, full_path)
    casts = []
    for weight in graph.graph.initializer:
        if weight.data_type != TensorProto.FLOAT or np.prod(weight.dims) <= 1024:
            continue
        values = numpy_helper.to_array(weight)
        if np.max(np.abs(values)) > 65504:
            continue
        name = weight.name
        weight.CopyFrom(numpy_helper.from_array(values.astype(np.float16), name + "__stored_fp16"))
        casts.append(helper.make_node("Cast", [name + "__stored_fp16"], [name], to=TensorProto.FLOAT))
    nodes = list(graph.graph.node)
    del graph.graph.node[:]
    graph.graph.node.extend(casts + nodes)
    onnx.checker.check_model(graph)
    onnx.save(graph, packed_path)
    metadata = {
        "checkpoint_sha256": checkpoint_hash,
        "config_sha256": config_hash,
        "opset": 17,
        "packed_initializers": len(casts),
        "artifacts": {
            path.name: {"bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for path in (full_path, packed_path)
        },
    }
    (args.output / "conversion.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(json.dumps(metadata, indent=2))


if __name__ == "__main__":
    main()
