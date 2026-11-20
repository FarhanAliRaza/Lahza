"""Writes the tiny ONNX policies the tests run (needs `pip install onnx`).

    python3 make.py

Each is one Gemm: action = observation · W + b, for the tests' figure of
16 bones and 2 feet (observation 183 numbers, action 45).

  zeros.onnx   W = 0, b = 0: the clip, untouched
  bend.onnx    b turns bone 5 (the left forearm) 0.8 rad about z
  height.onnx  W: that turn = 0.5 × observation[9] (the root's height)
  wrong.onnx   gives 7 numbers
"""
import os

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

HERE = os.path.dirname(os.path.abspath(__file__))
OBS, ACT = 16 + 11 * 15 + 2, 45
FORE_L_Z = (5 - 1) * 3 + 2


def write(name, w, b):
    act = b.shape[0]
    graph = helper.make_graph(
        [helper.make_node("Gemm", ["observation", "W", "b"], ["action"])], name,
        [helper.make_tensor_value_info("observation", TensorProto.FLOAT, [1, OBS])],
        [helper.make_tensor_value_info("action", TensorProto.FLOAT, [1, act])],
        [numpy_helper.from_array(w.astype(np.float32), "W"), numpy_helper.from_array(b.astype(np.float32), "b")])
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
    model.ir_version = 8
    onnx.checker.check_model(model)
    onnx.save(model, os.path.join(HERE, name + ".onnx"))


zeros = (np.zeros((OBS, ACT)), np.zeros(ACT))
write("zeros", *zeros)
b = np.zeros(ACT)
b[FORE_L_Z] = 0.8
write("bend", np.zeros((OBS, ACT)), b)
w = np.zeros((OBS, ACT))
w[9, FORE_L_Z] = 0.5
write("height", w, np.zeros(ACT))
write("wrong", np.zeros((OBS, 7)), np.zeros(7))
