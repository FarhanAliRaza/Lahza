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

- **A frame whose images exceed the image pool is a hard error.** The pool
  is 48 Mi texels (with mips). It used to leave the image that did not fit
  undrawn and decode it again every frame (s01 of the CPU film went from
  ~0.5 to ~6 s a frame, with only a warning at the end); now the frame fails
  with `Images::overflow_report` (the images, their sizes, what to do).
  Still open: `check` does not catch it before a render, and the budget is
  fixed rather than sized from the GPU. A texture's size could also follow
  its on-screen footprint (mip residency) so a 4K map seen small costs
  little.
- **Path-traced renders keep the GPU idle two thirds of the time.** s01 of
  the CPU film (`~/Videos/cpu/work_a.lahza`: ~400 scanned-rock layers,
  4-sample motion blur, half size, 16 spp) takes 486 ms an output frame,
  of which the GPU works 170 ms (trace 121, built-in denoise 20, OIDN
  copies 21). Measure with `lahza motion render ~/Videos/cpu/work_a.lahza
  --from 4 --to 5 --pt --spp 16 --scale 0.5 --profile`. The CPU side, per
  output frame, and what to do about each:
  - **CPU and GPU take turns (153 ms waiting for the trace, 17 ms more
    before OIDN).** The next frame is evaluated and recorded only after
    this one's trace finishes. Overlap them: record frame N+1 (and its
    motion-blur sub-frames) while the GPU traces frame N, with a buffer
    set per frame in flight.
  - **Record + upload (96 ms).** Per sub-frame work for every layer,
    times 4 motion-blur sub-frames; most of it likely re-uploads data
    that did not change. Profile which buffers are written and upload
    only what changed (instance transforms, not whole meshes or
    material tables).
  - **OIDN round trip (80 ms).** OIDN runs on CUDA (its default device on
    the RTX 3060), but the tracer is Vulkan, so every frame downloads
    ~50 MB of lighting and guides to host memory, repacks them on one CPU
    thread, uploads to CUDA, reads the result back and uploads it to
    Vulkan (`pt.rs` `oidn_denoise`). Share the memory instead: allocate
    the denoise buffers exportable (`VK_KHR_external_memory_fd` in the
    vendored blade), import them with OIDN's shared buffers
    (`oidnNewSharedBufferFromFD`), pack colour/albedo/normal with a
    compute pass, and keep one sync point. Expected ~10-20 ms. Also the
    profile stage is labelled "OIDN denoise (CPU)" but times the whole
    host round trip: rename it.
  - **Trace setup (47 ms).** Structures, buffers and the pipeline are set
    up every frame even when nothing moved. Keep the TLAS and per-frame
    buffers, refit or rebuild only what changed.
  - Display lists (31 ms) and scene evaluation (7 ms) are small now.
  Target: close to the GPU's own time, ~170 ms a frame here (about 3×),
  for every path-traced scene.

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
