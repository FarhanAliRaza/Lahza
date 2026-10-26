Based on crates.io tract-onnx 0.22.5 (Sonos; Apache-2.0 OR MIT).

Lahza changes `src/ops/resize.rs` only: evaluation must derive resize scales
from the requested sizes when the optional ONNX scales tensor is empty.
The original shape inference already does so, but evaluation iterated the
empty scales vector and incorrectly returned the unresized input tensor.
This affected the official PPHumanSeg model's p2o.Resize.0 (24×24 → 48×48).

A regression test exercises that valid empty-scales/sizes representation.
The bundled neural model itself is unmodified. Upstream licenses are retained.

The same file also evaluates separable resize with contiguous tensor slices
instead of allocating a dynamic ndarray index for each output scalar. A
regression test compares every scalar against the original dynamic-index
implementation for linear/nearest interpolation and all supported coordinate
modes. The arithmetic and weights are unchanged; this removes a measured
~45 ms/frame allocation/indexing cost from the PPHumanSeg graph on this CPU.
