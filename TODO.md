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
  Since then the maps are built on every core (300 bubbles under two
  lights: about 40 ms became 17 on four cores,
  `benchmark_shadow_maps_of_300_bubbles`); drawing them on the GPU is
  not done yet: it runs only where meshes go through the hardware
  visibility pass (8x MSAA), which Mesa's software Vulkan lacks, so it
  could not be run where the rest of this list was worked through. A plan
  that fits the code as it is:
  - The list builder writes, per layer list, its casters as rows (object
    → camera transform, the pooled mesh's place as `mesh3d::IROWS` row 13
    has it, mirrored flag) for every mesh of the list, placed on a target
    or not, and per shadow-casting light the map's six header rows without
    texels; the display list names them as jobs (`merge` re-bases them).
    Bounds come from each caster's box (and bounding sphere, exact for the
    bubbles), not from every vertex.
  - The GPU draws each job into a page of a depth atlas (one per frame
    set, `Depth32Float`, compare greater, cleared to 0) with instanced
    draws from the pool, as `vis_instance` does: directional maps write
    (u, v) in texels and closeness scaled into 0..1 by the casters' extent
    along the light; point maps write clip = (x − u0·z, y − v0·z, 1, z)
    scaled to the page, so depth is 1 / z, the near plane is z = 1 and the
    hardware clips where the CPU drops triangles. Faces are culled by the
    pool's side bits against the light, as `ShadowMap::build` does.
  - `shadow_texel` (motion.wgsl) reads a GPU map from the atlas (a flag
    and the page's corner in the header, patched in before upload, as the
    pool patches instance rows) and a CPU map from its rows as now, so
    lists for the CPU and devices without the pass are untouched.
  - To test it without such a device: let the visibility pass run at 4x
    for tests (the raster then loads sample `s & 3`), which Mesa has.

- **Motion blur: what is left.** A frame takes the sub-frames its motion
  needs (`motion_px` in motion-render's lib.rs), so slow passages are
  cheap; a fast one still draws everything once per sub-frame. What does
  not change between sub-frames could be drawn once: groups whose
  primitives (and the groups they read) are the same in every sub-frame
  (a blurred backdrop, an effect on a still layer), and shadow maps whose
  casters and lights did not move. Shapes, text and shader pictures that
  change form are not measured and take every sample.
- **Image memory: what is left.** Textures are held at the size they show
  (`Images::get_sized`: the file halved while it still has the texels a
  layer or a mesh's map needs on screen), the pool's budget follows the
  GPU's memory (`images::budget_for`), and `check` counts a frame's needs
  as the renderer holds them (`list::frame_image_texels`). Still open: a
  texture is decoded in full each time it is loaded at a new size (a slow
  zoom into a 4K map decodes it a few times), the whole pool is uploaded
  again whenever one image changes, and a mesh's need is estimated from
  its bounds on screen and its `uv` repeats (`list::map_want`), not from
  its UVs: an atlas that gives a small part of the mesh most of its
  texels shows that part softer than the file allows.
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
