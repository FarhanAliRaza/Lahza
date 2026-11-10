# TODO

## Motion renderer

- **Shadow maps on the GPU: to measure on a real device.** Lists for the
  hardware visibility pass now leave their shadow maps to the GPU: the
  builder writes each list's casters (transform and pooled mesh) and each
  map's frame, the GPU draws them from the mesh pool into pages of a depth
  atlas (`record_shadows`, `shadow_vertex`) and `shadow_texel` reads them
  there. On 300 glass bubbles under two lights the maps' share of list
  building went from 17 ms (40 before they were built on every core) to
  0.2 ms. It has only run on Mesa's software Vulkan, through the
  visibility pass at 4x (a test mode: `TEST_VIS_AT_4X`); on a device's own
  8x, run `lahza motion snapshot cpu.lahza --at 9.0,10.4 --width 540
  --profile` and compare the frame against `--backend cpu`. A map's frame
  comes from its casters' boxes, not from every vertex, so it is a little
  wider than the CPU's (a single sphere's by up to 1.7 times: softer
  texels); bounding spheres would tighten it.
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
  - **OIDN on the tracer's memory: to run on NVIDIA.** Where the device
    exports memory (`VK_KHR_external_memory_fd`) and OIDN's device
    imports it (`externalMemoryTypes`, `oidnNewSharedBufferFromFD`), the
    packed picture and the result are shared buffers and nothing goes
    through the host. It has only run with a stand-in for OIDN on Mesa's
    software Vulkan. On the real thing the first shared picture is
    checked against the same picture denoised the copied way; a mismatch
    or an error switches sharing off with a warning ("copied through the
    host: ...") and the copied way carries on. If that warning shows on
    the RTX, its reason is what to fix.
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

- **To compile once with slangc.** These were written where slangc could
  not be run, and are checked only as text (`cargo test -p
  lahza-motion-core slang`): shader layers pass their params in a
  `<function>_params` struct where the module declares one (by field
  name; one by one in name order where it does not, as before);
  generators and deformers are passed the struct whenever the module
  declares it, params or not; the compiler's errors in Lahza's generated
  calls are shown as the call. The generated Slang follows what the
  geometry generators already compiled (`T p = {}; p.field = …;`), in a
  compute kernel too for shader layers: run the Slang tests and a
  project with a shader layer's params once.
- **Slang geometry on the GPU: what is left.** A generator whose layer
  gives `bounds` is generated by the GPU in its mesh pool
  (`mesh_gen.rs`, `slang_mesh::Kernel`, docs/slang/geometry.md). Not yet:
  - *The kernel slangc makes has not been compiled.* It was written where
    slangc could not be run; the tests here run a hand-written WGSL
    kernel in its place. With slangc, `cargo test -p lahza-motion-core
    slang_mesh` builds a real kernel and `cargo test -p
    lahza-motion-render the_gpu_runs_the_projects_generators` draws with
    it against the CPU's mesh. If the kernel's WGSL is refused, the mesh
    comes from the CPU and the render warns.
  - *Deformers* still run on the CPU (they need the welded normals of
    the mesh they move, and a CPU mesh to subdivide).
  - *The path tracer* takes the CPU's mesh: its acceleration structures
    are built from buffers of their own, not from the pool.
  - *A new mesh still waits for the frames in flight* (`MeshPool::changes`):
    an animated mesh is new every frame, so its frames do not overlap on
    the GPU. Rows freed for a frame's mesh would have to stay unwritten
    until the frames that read them are done.
