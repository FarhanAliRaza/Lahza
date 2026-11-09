# TODO

## Motion renderer

- **Shadow maps on the GPU.** `ShadowMap::build` (mesh3d.rs) still
  transforms every shadow caster's vertices and rasterises the light's depth
  map on the CPU, every frame. On the glass-bubble frames of
  `~/Videos/cpu/cpu.lahza` (9.0 s and 10.4 s, 540 px wide) it is about 36 of
  the 52 ms of display-list time, the largest CPU cost left. Draw the depth
  maps on the GPU from the mesh pool (`pool.rs`) instead, the way the
  visibility pass draws meshes. Measure with
  `lahza motion snapshot cpu.lahza --at 9.0,10.4 --width 540 --profile`
  (stage "display lists › meshes (shadow maps)").

- **Transition + motion blur overflows a GPU buffer.** A scene with a
  transition (whip, `"dur": "6f"`) and 8-sample motion blur over a
  full-frame shader layer and meshes fails the export:
  `GPU failed at frame 240: motion groups needs 800409600 bytes, over the
  GPU buffer limit`. Repro: `~/Videos/slang-reel/reel.lahza` with the
  `motion` scene back at 8 blur samples, then
  `lahza motion render reel.lahza --from 7.9 --to 8.1`. `check`,
  `snapshot` and `review` don't use motion blur, so nothing warns before
  the render. The reel works around it with 4 samples.

## Slang

- **A Slang track adds nothing before its `from`.** A cut on a marker lands
  a frame early, so a track starting on the beat shows the object at rest
  on the cut's first frame. Decide whether a track should hold its first
  value before `from`.
- **Generator errors point at Lahza's wrapper.** A geometry generator whose
  Slang has a `<fn>_params` struct but no `params` in the layer JSON fails
  with `lahza_cpu.slang:11:33: not enough arguments to call (got 2,
  expected 3)` and a hint that only mentions shader layers. Missing
  functions (geometry functions and scene parameters) also point into the
  generated wrapper's cache path instead of the author's file in
  `shaders/`.
- **Slang geometry runs on the CPU, at fixed sizes.** Generators and
  deformers are evaluated on the CPU and uploaded; a GPU compute version
  would keep large animated meshes off the CPU. Vertex and triangle counts
  can't change over time, so particles that haven't appeared yet have to be
  collapsed to zero size.
- **Shader-layer params are passed in alphabetical order.** A layer's
  `params` reach the author's function sorted by name, not as written, so a
  mismatch shows up as a compile error in the generated caller. Pass them
  by name instead (a params struct, as geometry generators do).
- **Path tracer can't run Slang materials.** Traced frames with a Slang
  material fail with a clear error; `pt.wgsl` needs the same dispatch the
  raster shader has.
