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
- **Path-traced renders: what is left of the idle GPU.** Measured on s01
  of the CPU film (`~/Videos/cpu/work_a.lahza`, ~400 scanned-rock layers,
  4-sample motion blur, half size, 16 spp): 486 ms an output frame, of
  which the GPU worked 170. Done since, and not yet measured on that
  machine (`lahza motion render ~/Videos/cpu/work_a.lahza --from 4 --to 5
  --pt --spp 16 --scale 0.5 --profile`; the stages are finer now):
  - Frames overlap: an export pushes frames through the renderer
    (`Renderer::push`/`pop`), up to three on the GPU (one traced, one at
    Open Image Denoise, one read back), each with its own set of the
    buffers a frame writes. A trace's bands of rows queue up behind each
    other instead of each being waited for.
  - A motion-blurred frame's sub-frames are traced into one picture and
    denoised once (four OIDN runs and four filter chains a frame became
    one).
  - The tracer keeps its tables, scratch and top-level structures between
    frames (nothing is allocated or freed in a steady frame), builds a
    top-level structure only when its instances changed, and writes
    instance records itself with each mesh's address looked up once.
  - To OIDN and back is packed on the GPU (36 bytes a texel down, 12 up,
    no per-texel work on the CPU; it was 96 down, 32 up and two passes
    over them on one thread).
  Still open:
  - **OIDN's memory is still copied.** OIDN runs on CUDA, the tracer on
    Vulkan: the packed picture goes through host memory both ways. Share
    it instead (`VK_KHR_external_memory_fd` in the vendored blade,
    `oidnNewSharedBufferFromFD`), with one sync point. Needs an NVIDIA
    machine to write against.
  - **Display lists are rebuilt per sub-frame** (31 ms on s01): each
    sub-frame evaluates the scene and builds every layer's instance and
    material rows again. Layers whose state did not change between
    sub-frames (most materials) could reuse theirs.
  - **Shared uploads wait for the GPU.** New glyphs, new images and new
    pooled meshes are uploaded only once the frames in flight are off the
    GPU (`drain`), so a film that adds one every frame (animated Slang
    geometry) does not overlap. Per-frame staging would lift that.
  - If the desktop stutters now that bands are not waited for, see
    `LAHZA_TRACE_YIELD` (and say so here).

## Slang

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
