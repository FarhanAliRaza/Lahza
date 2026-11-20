# TODO

## Where things stand

Every section below was worked through in cloud containers (four cores,
no GPU: Mesa's software Vulkan, Xvfb, a stand-in for the agent). Each
section says what was done, what was measured and what is left; this is
the short of it.

Done and run here (pictures looked at; on the merged tree, one thread,
Mesa's software Vulkan: motion-render 186 tests pass and 21 are ignored
(measures), motion-core 135, motion-sim 35, motion-mcp 38 and its
integration tests, the app 122 and its four window tests under Xvfb;
since, the editor's own later session: motion-sim 36, motion-mcp 39,
the app 125 with three of `timeline_preview`'s failing in that
container as before its changes (no `avenc_ffv1`: GStreamer's libav
plugins were not installed), and seven window tests under Xvfb):
- The editor: drags that stalled (a tick changes one layer and compiles
  one scene), the prompt bar and its two agent runners (Claude Code, or
  an API key alone), undo of a whole request, guarded writes, the
  inspector's tabs and hand edits, the preview's choice of raster, sky or
  traced, the bake pill.
- Simulation: baked rigid bodies and joints, glTF skinning on the CPU and
  in the GPU's mesh pool, ragdolls that fall, act, get up and do not
  pass through themselves, directing takes, a policy's interface and a
  search controller that steps, crowds of instances of one character
  with far agents as pictures (5 000 baked and drawn), that plan round
  walls and step aside.
- The fast mode: a fast path without NVIDIA, its tests through a stand-in
  for Ray Reconstruction, `snapshot --fast`, history at cuts, jumps and
  transitions.
- Lighting: sky visibility in the raster with the light that comes back
  once and again, cards and see-through things in it, surfaces told
  apart at edges, the same rays on the CPU (the two agree), off unless
  asked for.
- The renderer and its tests: a GPU shadow map's frame from its casters'
  balls and, for a point light, as the CPU frames it; a still mesh group
  not redrawn under motion blur; shadow maps asked along the surface's
  own normal; debug names only where a tool reads them; a report of
  where the GPU's frame leaves the CPU's; a benchmark for motion blur
  over still groups.
- Since (a later session, the same kind of container): sky visibility
  tells surfaces apart by their planes (no steps along an edge), cards
  and see-through meshes hide the sky; a point light's GPU-drawn shadow
  map is framed as the CPU's; a mesh group that stands still is left as
  drawn by a motion-blurred frame's later sub-frames (none ever was); a
  normal-mapped face no longer shadows itself, which was the spots in
  the GPU-against-CPU comparison. And a second round: volumes, filled
  wireframes and unlit surfaces in the sky's rays; a directional
  light's GPU-drawn shadow map framed as the CPU's; a shape that changes
  size or paint takes the sub-frames its motion needs; the fast mode's
  catcher shadow over nothing smoothed; measures of a third return, of
  cards in the rays, of a rasteriser's grid in the two other tests that
  fail on the card, of a traced frame's lists. (motion-render on that
  branch: 188 pass and 25 are ignored, the whole suite, before the
  catcher change; its 13 fast-mode and catcher tests after it.)

- Since, in the simulation (its own section has each with its test): a
  rope of links, bodies two instancers deep and as an emitter's
  particles; eight joints a point, dual quaternions beside linear blend
  on the CPU and in the pool's kernel, imported rotations that follow
  the file's slerp, a `.gltf` with its buffer and pictures; a ragdoll's
  limits from its bones' names, aimed arms led round the body; a take's
  events on its sheet, reactions that differ by seed; the search with
  elbows and getting up; a crowd's guard pose, far agents that come in
  over their pictures, pictures lit by stepped lights.

To do on the owner's machine (the RTX 3060, a desktop, Claude Code signed
in), since none of it can be seen from a container:
1. Send a request with the API-key runner (it needs a key; it has only
   met a stub server). A real request was sent from the prompt bar to
   Claude Code under Xvfb (18 s, five turns, $0.139, the edit made and
   undone as one): the same on the desktop is worth a look, with a
   mouse ("Directing an agent from the editor").
2. Drag values in the inspector: whether the bars now move freely
   ("Drags that stall").
3. The fast mode on the card: `fast_tests`, its measure, and Ray
   Reconstruction's picture of what changed (the specular guide, the
   blur, the reset at cuts); the preview with Traced chosen, for its
   pacing.
4. Sky visibility on the card: its cost, and its picture through the
   hardware visibility pass; then whether it is on by default.
5. The tests on the card: in parallel (the crash in the loader), the four
   GPU-against-CPU ones again (plainly, and with
   `LAHZA_MOTION_VIS_REPORT=1`): whether surface detail and area lights
   now pass, as the changes made since should have them, the two tracer
   tests that took whatever denoiser was installed.
6. Simulation on the card: what skinning and crowds (instances and
   pictures were never drawn by the GPU or the tracer) cost there, with a
   character of real size; a bake on another kind of CPU (both
   containers were Xeons with FMA).
7. Slang: both of its points need slangc, which a cloud session cannot
   fetch.
8. The motion blur benchmarks (shapes, and a still mesh group) and the
   shadow maps' frames on the card.
9. NVIDIA's licence for the Ray Reconstruction shim: to read and decide
   (packaging/ngx/README.md).

Not done, and why:
- A trained policy for ragdolls: a GPU simulator, motion capture and
  days of training. Its interface and the no-training search are there.
- What each section names as left after its own work (for example
  volumes in the sky's rays, a third return of its light, pictures of
  far agents taking the scene's
  shadows, a ragdoll that gets up from its back): smaller than the
  points above, each with its reason where it is named.

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
  texels). Run on the RTX 3060 since: see "Tests on a hardware GPU" below.
  Since: a directional map's frame takes each caster's span from its box
  and from the ball its points are in (the farthest point from the box's
  middle, kept with the pooled mesh; `CasterBox::span`), whichever is
  less, so a round caster no longer widens it. On the shadow test scene
  the frame is the floor's and nothing changes (456 and 472 pixels off
  the CPU's by more than 24 at 4x, before and after): the gain is for
  round casters with no floor under the light.
  Since, a point light's map (`ShadowMap::gpu_point`,
  `CasterBox::seen_from`; an area light's is one too): it is aimed as
  the CPU's is, at the mean of the casters' points (each pooled mesh
  keeps its own mean; it was the middle of the boxes' corners, another
  axis and so another frame), and a caster in front of the light is seen
  no wider than its ball is (the planes through the light that touch its
  ellipsoid). One with corners on both sides of the light's plane no
  longer gives the whole view ([-3, 3] each way): it is framed from its
  triangles wholly in front, as the CPU leaves out one with a point
  behind the light, where its points are at hand and few (4096
  triangles: a floor, a wall); else from its box cut at that plane.
  `gpu_matches_cpu_area_lights` on software Vulkan through the
  visibility pass at 4x (`LAHZA_MOTION_VIS_REPORT=1`,
  `LAHZA_SHADOW_FRAMES=1` prints both frames): the CPU's frame is
  [-0.1350, -0.1964, 0.3989, 0.1300]; the GPU's was [-0.352, -0.164,
  0.219, 0.292] (1.07 and 1.40 times as wide, about another axis) and is
  [-0.1351, -0.1964, 0.3989, 0.1302]. Pixels over 3 levels: 699 before,
  609 after (the rest are the 4x pass's edges); away from edges 13
  (worst 5) and 7 (worst 4). Seen in the difference picture: the dotted
  ring along the ball's and the cube's shadows is gone. What that test
  showed was this, not a caster around the light: its floor crosses the
  camera and casts nothing, so no caster there is on both sides. That
  case is tested on its own
  (`a_point_lights_frame_holds_its_casters_and_little_more`: a floor
  around the light framed as the CPU frames it; a ball's frame within 5%
  of its points', its box's a third larger). Not run on the card: the
  118 pixels there are to be counted again.
  Since, a directional light's map too (`gpu_directional`): each side
  of its frame is taken from the points of the casters that could be its
  outermost (the widest by box and ball first, until the next one's box
  is within what the points have given; a caster of up to 16384
  triangles, 200000 points a list: `FRAME_TRIS`, `FRAME_POINTS`), which
  is the CPU's frame: that one is from the points of the triangles
  facing the light, and a closed caster's outline is among them. With
  `LAHZA_SHADOW_FRAMES=1` the two frames are the same to the last digit
  in `gpu_matches_cpu_meshes`, `gpu_matches_cpu_shadows_and_occluders`
  and `gpu_matches_cpu_surface_detail` (in the first the GPU's was 13
  and 15 units wider on two sides). The 300 bubbles' maps still cost
  the list 0.24 ms (2.1 ms with every caster's points walked, which is
  why only the outermost are).
  As before, and why: a mesh the GPU generates in its pool is framed by
  its box (the CPU has no points of it, and draws no such mesh: there is
  no CPU frame to be like; its true bounds would have to be read back
  from the GPU, a frame late), and the aim of a point light's map is the
  middle of the boxes as soon as one caster is such a mesh. A caster of
  more than 16384 triangles is framed by its box and ball. A closed
  caster around a point light is framed with its faces turned from the
  light too. To run on the RTX 3060: `LAHZA_SHADOW_FRAMES=1
  LAHZA_MOTION_VIS_REPORT=1 cargo test --release -p lahza-motion-render
  gpu_matches_cpu_area_lights -- --nocapture --test-threads=1`, then the
  same test without the two variables: whether it passes.
- **Motion blur: what is left.** A frame takes the sub-frames its motion
  needs (`motion_px` in motion-render's lib.rs), and a sub-frame does not
  draw again what the one before it left as it needs it: a group whose
  primitives (and the groups they read) are the same (`gpu::same_groups`:
  a blurred backdrop, an effect on a still layer), and a GPU shadow map
  whose casters and lights did not move (`gpu::same_shadows`). Checked
  only as "frames are what they were" on the Slang reel and the CPU film;
  no span rendered there has a still group under blur, so the saving is
  not measured. Not done: traced frames (their sub-frames' groups sit
  side by side), groups with a mesh when any mesh of the sub-frame moved,
  and shapes, text and shader pictures that change form (they still take
  every sample). Since: `benchmark_motion_blur_with_still_groups`
  (motion-render's tests, `--ignored`) is such a span: a blurred backdrop
  and a glow that stand still behind a bar that moves, against the same
  frame with both drifting. On Mesa's software Vulkan both groups are
  reused in each of the frame's four sub-frames and the frame takes 110 ms
  against 112: within the noise there, where the time is the passes that
  draw the frame itself. To run on the RTX 3060 (its profile gives the
  passes' times) before any of the "not done" is worth doing.
  Since, groups with a mesh: per group. `same_groups` goes by what each
  mesh primitive reads (`list::MeshRead`, written as the batch is
  placed: its own rows, the environment's, its lights' shadow maps and
  their casters, its triangles on the target, its tile bins; and the
  pooled meshes and shadow jobs in those rows), not by the list's mesh
  rows whole. And it found that no group with a mesh had ever been left
  as drawn, even with every mesh still: a mesh primitive carries its
  layer list's number, which went on counting from one sub-frame's list
  to the next, so the primitives were never the same. The number now
  starts again in every list (`MeshCache::begin`). With it two frames
  of a scene with meshes in which nothing moves are the same lists, and
  the second comes from the renderer's last frame as a scene without
  meshes always did (the sky's measures drew 7 ms frames until they were
  made to draw each one). Tested through the
  compute raster and the visibility pass at 4x
  (`a_still_mesh_group_is_not_drawn_again_beside_a_moving_mesh`: a comp
  with a mesh under a glow beside a mesh that moves is left as drawn,
  the frame is bit for bit the one with every group drawn, and a comp
  whose mesh turns is drawn each time). A mesh under an effect in the
  same layer list as the moving mesh is still drawn each time, rightly:
  it reads the list's shadow maps and has the mover as an occluder.
  `benchmark_motion_blur_with_a_still_mesh_group` (`--ignored`, 960 x
  540, 8 sub-frames) on Mesa's software Vulkan, the compute raster: 677
  ms a frame with the group left as drawn (4 groups a frame), 679 with
  every group drawn, 682 with the comp's mesh turning (657, 676 and 650
  in another run): within the noise. The wait for the GPU is 10 to 18 ms
  shorter and telling the stretches apart costs 8 ms of CPU (the two
  512 x 512 CPU-drawn shadow maps are rows of the list and are compared
  whole; a list for the visibility pass has no such rows). Nothing is
  claimed for a card: to run there (`LAHZA_MOTION_SOFTWARE_GPU` unset:
  `cargo test --release -p lahza-motion-render -- --ignored --nocapture
  --test-threads=1 benchmark_motion_blur`; its profile has the passes'
  times).
  Since: a mesh primitive counts as reading groups only when it does (a
  card's picture, a glass mesh's backdrop, the sky's:
  `gpu::mesh_reads_groups`), so a group before it that moved no longer
  has it drawn again (the test's scene has such a group now). And *a
  shape that changes form* no longer takes every sample when its points'
  way can be bounded (`motion_px`'s `shape`): its paint alone is no
  motion; its size, corner radius or thickness, for the kinds drawn in
  their box, moves no point farther than the box's corners. A circle
  growing a pixel a second takes two samples where it took eight
  (`motion_blur_takes_the_samples_the_motion_needs`,
  `a_shape_changing_form_moves_as_far_as_its_box`).
  Still every sample, and why: a path that morphs or is trimmed and a
  field's points (how far a point goes along a path is not in the
  layer's state), text whose glyphs are animated (their places are laid
  out by the fonts, which the estimate does not have), a shader picture
  (what its function paints cannot be bounded from outside).
  Not to do, traced frames' groups: a traced picture is new in every
  sub-frame by its rays, and the 2D groups beside it sit side by side
  for the samples that share the trace (`merge`'s `separate`), so
  leaving one as drawn would mean reading another sample's texels; a
  traced frame's time is its rays (486 ms on s01, below) and a 2D
  group's pass is under the noise even in a raster frame (above).
- **Image memory: what is left.** Textures are held at the size they show
  (`Images::get_sized`), the pool's budget follows the GPU's memory, and
  new texels are added to the GPU's copy (all of it again only after
  `compact` or when its buffer grows). The last pictures decoded are kept
  (256 MB, `Images::decoded`) for loading them at another size. A mesh's
  maps are sized by its UVs where the mesh is at hand
  (`Mesh::uv_stretch`, never below the old estimate; s01 of the CPU film
  still fits its pool), in `check` too (`list::frame_image_texels` is
  given the meshes' measures). A picture's whole mip chain is kept, so a
  copy at another size is a run of its levels (`images::Chain`).
- **Path-traced renders: what is left of the idle GPU.** Measured on s01
  of the CPU film (`~/Videos/cpu/work_a.lahza`, ~400 scanned-rock layers,
  4-sample motion blur, half size, 16 spp): 486 ms an output frame, of
  which the GPU worked 170. Measured there again at 4 spp (`lahza motion
  render ~/Videos/cpu/work_a.lahza --from 4 --to 7 --pt --spp 4 --scale
  0.5 --profile`): 216 ms a frame before the three changes marked *new*
  below, 153 ms after, the same frames; its first second took 20.6 s and
  10.8 s. Not measured again at 16 spp. Done:
  - Frames overlap: an export pushes frames through the renderer
    (`Renderer::push`/`pop`), up to four on the GPU (one traced, one at
    Open Image Denoise, one composited, one read back), each with its own
    set of the buffers a frame writes. A trace's bands of rows queue up behind each
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
  - *New:* the tracer's vertex, index and tag arrays take a new mesh at
    their end (`Tracer::shared`) and are packed only once a fifth of them
    is dead. They were packed and uploaded whole whenever a mesh came or
    went, every frame of this film: a third of the CPU's time.
  - *New:* Open Image Denoise is begun without being waited for
    (`oidnExecuteFilterAsync`, `Gpu::start_denoise`), so the next frame is
    built and recorded while it runs; four frames may be in flight. It
    blocked the renderer for 95 ms a frame.
  - *New:* new glyphs, images and pooled meshes go through the frame's
    own staging (`gpu::Stage`) into what the frames in flight read; rows
    the mesh pool frees stay unwritten until those frames are done
    (`MeshPool::limbo`). The frames in flight are waited for only when
    what they read would move: a font atlas started over, an image pool
    packed or outgrown, a mesh pool with no free rows.
  - OIDN ran on the RTX 3060 here with no "copied through the host"
    warning. (Whether its pictures were shared or its device does not
    take the tracer's memory at all was not looked at.)
  Still open:
  - **Display lists per sub-frame**: about 20 ms a frame on s01 (by a
    perf call graph, half of it is `Images::get_sized` and
    `MeshCache::trace_instance`, once per layer per sub-frame). The frame
    is now bound by the GPU (trace 55 ms, OIDN beside it), so reusing
    unchanged layers' rows would not shorten it; not done.
    Measured since without the film
    (`mesh_tests::benchmark_traced_lists_of_400_layers`, `--ignored`:
    four sub-frames of 400 mesh layers that move, no image maps, four
    cores of a Xeon): 1.4 ms a list, 204 material rows a layer, all made
    anew in every list. So the rows are 5.6 ms of a frame there, and the
    film's 20 ms is mostly its scans' maps (`Images::get_sized`). Not to
    do without the film: a layer's rows hold where its maps are in the
    image pool, which may move between lists, so keeping them across
    sub-frames needs that checked against the film's own scenes. Needs
    the owner's machine (the film is in ~/Videos/cpu): `perf record -g
    lahza motion render ~/Videos/cpu/work_a.lahza --from 4 --to 7 --pt
    --spp 4 --scale 0.5`, and only if the CPU, not the GPU, is then what
    a frame waits for.
  - If the desktop stutters now that bands are not waited for, see
    `LAHZA_TRACE_YIELD` (and say so here).
  Measured at the film's own settings (s01, 1080 x 1920, 64 spp, 4
  sub-frames, no `--profile`: it doubles a traced frame's time): 1.54 s a
  frame, the GPU busy throughout; 1.0 s with `LAHZA_DENOISER=builtin`. Open
  Image Denoise (CUDA, one tile) takes 0.25 s of GPU on its own whatever
  its `quality` (the profile's "oidn pack" is that run, not the pass).
  430 million ray queries a frame, 42% of them shadow tests (the profile
  now says), 0.5 candidates a ray; lanes of a lockstep block make the same
  number of queries (99 to 100%: nothing to win by starting a lane's next
  path early). Tried against a 1024 spp reference (PSNR of two H.264
  frames, so under 1 dB is noise):
  - *Kept:* a shadow test in a scene with no volume ends at the first
    surface that blocks and reads only its material (`blocked`): the trace
    is 9% shorter at half size, the video the same bit for bit.
  - *Removed again:* adaptive sampling in the kernel (a settled texel
    takes a quarter of its samples): a third to a half fewer rays, but
    7 to 36% less time (settled and unsettled texels share blocks) and
    2 dB worse; 48 spp everywhere is as fast and 1.6 dB better.
  - *Removed again:* a Kronecker sequence for a path's first 32 numbers:
    +0.9 dB at 64 spp, -1.2 at 32.
  - 32 spp is 1.4 dB under 64 spp at half the trace's time.
  - Not to do: one shadow test a vertex. With the film's one or two
    lights that cast shadows it saves at most half the shadow tests (a
    fifth of the rays) for twice the direct light's spread in each
    path, the trade adaptive sampling lost above; it would pay in a
    scene of many lights, which no project has. To settle on the card,
    should one come: the same PSNR against 1024 spp as above, at equal
    time, on `~/Videos/cpu/work_a.lahza`.

- **White specks on imported rocks (fixed).** A material that names no
  preset starts from "glossy", which has a clear coat of 0.35 at
  roughness 0.06, and `model-import` wrote glTF materials with neither a
  preset nor a coat: every imported scan wore a lacquer that mirrored the
  sun at a few points of it, first-hit samples worth thousands that Open
  Image Denoise made into white blobs jumping from frame to frame (s01 of
  the CPU film). The importer now writes `"clearcoat": 0`; work_a's 801
  imported materials were given it by hand (their scenes as they were:
  ~/Videos/cpu/backup/work_a-scenes-before-clearcoat-off). Found by
  reading the shaded surface's values back through the profile's
  counters; filtering the normal map, clamping bounce light, clamping a
  bounce's weight and a floor on roughness after a bounce were each tried
  and did nothing, and a clamp on every sample hid the specks but dimmed
  real highlights: none is kept.

## The fast path-traced mode (`--fast`)

Two paths a texel at half size each way, through one jittered point of
the texel, made the full picture by NVIDIA's DLSS Ray Reconstruction on the
GPU (`pt.rs` `reconstruct`, `ngx.rs`, the shim in packaging/ngx: `just ngx`
builds it from a DLSS SDK checkout into ~/.local/lib/lahza). On the
RTX 3060, 1080 x 1920: s01 of the CPU film 24 ms a frame (1.5 s as usual),
work_e's scenes 18 ms; the reconstruction itself 6 ms. A changed trace
kernel costs the driver about a minute once. The whole CPU film (5 856
frames) in 2 min 46 s, loading included. (Measured before what follows;
nothing below was run on that card.)

- **Elsewhere than NVIDIA**, or without the shim, the fast mode makes the
  picture itself (`TraceOptions::fast` without `rr` in pt.rs): four paths
  a texel at full size (`FAST_SPP_OWN`; a frame with none before it, the
  first or the first after a cut, sixteen), accumulated over the frames
  along each surface's motion and denoised (Open Image Denoise, else the
  built-in filter), then the same passes as with Ray Reconstruction: the
  visibility pass's coverage and flat colours over it, depth of field,
  motion blur. A warning says Ray Reconstruction was not used.
  `LAHZA_FAST=own` leaves Ray Reconstruction out where it is at hand, to
  compare.
  - With so few paths their own spread says little of the noise (at one
    path, nothing), so the built-in filter left the grain. The lighting's
    spread over the frames (its luminance's two moments, kept with the
    history) and, for the first four frames, over the texels around tell
    it instead (pt.wgsl `temporal`, this mode only).
  - Measured on Mesa's software Vulkan (llvmpipe, LLVM 20, four cores;
    its numbers, not a card's): 320 x 180, eight frames of a ball rolling
    past a metal box and a glass ball while the camera turns, the last
    frame against 512 paths not denoised
    (`pt::fast_tests::measure_the_fast_mode_against_the_usual_trace`,
    `--ignored`). Seconds a frame after the first, and the last
    frame's dB:

    | | seconds a frame | built-in filter | Open Image Denoise 2.3.3 (CPU) |
    |---|---|---|---|
    | usual, 64 paths | 1.99 | 33.9 dB | 34.3 dB |
    | usual, 16 | 0.51 | 33.4 dB | 33.9 dB |
    | usual, 4 | 0.18 | 27.5 dB | 27.7 dB |
    | fast, 8 | 0.32 | 31.8 dB | 32.3 dB |
    | fast, 4 | 0.19 | 31.0 dB | 31.4 dB |
    | fast, 2 | 0.11 | 30.6 dB | 31.1 dB |
    | fast, 1 | 0.10 | 29.8 dB | 30.2 dB |

    Four paths: a tenth of the usual trace's time for 2.9 dB less, and
    the time of the usual trace at the same four paths for 3.5 dB more.
    (The seconds are with the built-in filter and nothing else running;
    with Open Image Denoise they were only taken while other tests shared
    the machine, where it was a tenth slower at this size.) Looked at:
    the fast picture is close to the usual one; the glass ball is the
    blotchiest part of both.
  - Open Image Denoise is not in Ubuntu 24.04's packages (`apt-cache
    search` has no `libopenimagedenoise`). The release from
    github.com/RenderKit/oidn (2.3.3, x86_64 linux) works unpacked
    anywhere with `LAHZA_OIDN=<it>/lib/libOpenImageDenoise.so.2`.
  - After a cut (160 x 120, a cube before a wall, against 256 paths not
    denoised): the first frame 44.5 dB (its sixteen paths), the second and
    third 35.4 and 35.6 dB (four paths each, accumulating).
- **Cuts.** The history was never reset: a traced list's accumulated
  lighting, and Ray Reconstruction's own history, went on into the next
  scene (kept by the list's order in the frame, and `reset` was only set
  on a tracer's first frame). Now a frame none of whose mesh layers were
  traced in that list the frame before starts again: the tracer's own
  accumulation (the usual trace's too) and, through `reset`, Ray
  Reconstruction's. `Renderer::forget_history` does the same on demand.
  Two more cases, tested through the stand-in:
  - A camera that jumps within a scene (the layers are the same) starts
    the history again: the camera, from a layer's two transforms, turned
    more than 25 degrees since the frame before, or the scene's middle
    moved more than half the picture's smaller side (`JUMP_TURN`,
    `JUMP_SHIFT` in pt.rs). A whip pan that fast is taken for a jump
    too; little of the frame before is left in the picture then.
  - Through a transition two scenes are traced lists 0 and 1, and after
    it the one that came in is list 0. A list now takes with it the
    history another slot holds of its layers (the tracer's and Ray
    Reconstruction's), so each scene starts once and not again when the
    transition ends.
- **`snapshot` takes `fast`** (and the CLI's `snapshot --fast [--warm N]`).
  A frame alone has no history, so up to `warm` frames before it (default
  8; only those of its scene) are traced first and thrown away:
  `Renderer::try_render_warm`, which the editor's preview can call after
  a scrub (`forget_history` and consecutive `try_render`s while playing).
  Run through the CLI here: a 480 x 270 frame with four frames before it
  in 22 s, the usual trace's single frame at 64 paths in 24 s, the raster
  in 14 s (all three mostly the process's start and its pipelines, and
  taken while other tests shared the machine).
- **Tests** (`pt_fast_tests.rs`), with a stand-in for Ray Reconstruction
  as Open Image Denoise has one (`Tracer::pass_through_ngx`: the six
  images are read back from their textures, and the picture is the colour
  image enlarged between the trace's jittered samples, put where Ray
  Reconstruction's would be), and without Ray Reconstruction:
  what Ray Reconstruction is handed (the images' sizes and contents, the
  motion against how far the ball's texels moved, the jitter, `reset`);
  the picture against the usual trace; flat things' exact colours and
  edges; depth of field lit and unlit; motion blur over a floor, over
  nothing and past a thing that stands still; a shadow catcher's shadow;
  a cut, a camera jump and a transition; a frame alone and warmed up. All pass on software Vulkan, with
  the tracer's 40 other tests. The stand-in's picture is noisy and blocky
  (it is not a denoiser): what Ray Reconstruction makes of the same
  images is not tested by it.
- **The shim** is still built by hand (`just ngx`); what it needs, and a
  reading of NVIDIA's licence, are in packaging/ngx/README.md
  (`packaging/ngx/check.sh` lists what is missing of the SDK). In short:
  NVIDIA's library may be shipped inside the application as object code,
  under terms as protective as theirs and never under an open-source
  licence (Lahza is MIT: their files need their own notice and stay out
  of the repository), for NVIDIA GPUs only, with NVIDIA told before a
  commercial release, their marks in the about box, and not in a cloud
  service. Left for the owner: whether to ship it, the application id
  (the shim passes a made-up one), the notice and about-box line, and
  where the snap's build gets the SDK.
- **The picture.** Changed, and checked with the stand-in and without
  Ray Reconstruction (not with it):
  - Motion blur looked along each texel's own motion only, so a moving
    edge did not smear over what stood still and nothing smeared over
    where nothing was hit. Now the picture is cut in tiles of 16 texels
    with each tile's longest motion (`fast_tiles`), a texel takes the
    longest that reaches it from the tiles around, and each tap along it
    is what is there if that passes over the texel in the shutter's time
    and is not behind it, or if the texel's own surface is away by then
    and that is behind it (`resolve_fast`). A ball crossing a floor is a
    streak of its width and its way; alone over nothing its streak is as
    long as the usual trace's sub-frames make it; a post in front of it
    keeps its edge. A motion is followed up to 64 texels a shutter. With
    the stand-in the ball's outline shows faintly inside the streak (the
    motion has half the picture's size there).
  - Depth of field found a blurred thing in front of a sharp one only
    within reach of eight probes. The tiles hold their widest circle of
    confusion too, and a texel looks as far as the widest that reaches
    it: a bead of five px far in front of a sharp wall blurs over the
    wall as wide as the lens blurs it in the usual trace. A circle is
    still at most 24 texels.
  - A shadow catcher's shadow on what is flat (footage, a card) was the
    visibility pass's four paths a texel: grain. The pass now keeps the
    flat colour and what the catchers let through apart, and that is
    smoothed across the texels around that show the same flat thing, nine
    in a row and nine in a column (`fast_catcher`): less grain than 256
    paths not denoised (0.8 and 0.6 levels against 1.1). A catcher's
    shadow over nothing (it is the coverage there) was still four
    paths'. Since: `visible` marks a texel all of whose coverage is
    under a catcher's shadow with nothing flat behind, and
    `fast_catcher` smooths that coverage the same way, across the
    marked texels and the empty ones around (which take their share: in
    a soft edge most texels have no shadowed path of their four).
    `a_shadow_catchers_shadow_over_nothing_is_smooth`, with the stand-in
    and without Ray Reconstruction: grain of the coverage 0.5 levels
    where it was 14.9 and 256 paths not denoised have 1.9; the shadow's
    edge 135 of 255 for the usual trace's 141. Seen on the way: the usual trace's filter widens a
    catcher's soft shadow a good deal (against the same scene not
    denoised); not changed (it is the export's look, and narrowing it
    changes every exported shadow: for the owner to see on a film).
  - The specular guide was the surface's F0. It is now what the surface
    mirrors of an even light from where it is seen (the environment BRDF
    of F0 and roughness, by the raster's fit `env_brdf`), which is what
    NVIDIA's integration guide asks for; their own polynomial
    (`EnvBRDFApprox2`) is in the SDK's doc/ folder, which was not at
    hand, so the two fits are not compared number by number.

To run again on the RTX 3060 (nothing above was):
- `cargo test -p lahza-motion-render fast_tests -- --test-threads=1`, and
  the measure (`-- --ignored measure_the_fast --nocapture`,
  `LAHZA_MEASURE=1080,1920,8,256`), with and without `LAHZA_FAST=own`:
  the table above for a card, and Ray Reconstruction beside it.
- With Ray Reconstruction itself, looking at: the new specular guide
  (mirrors and rough metals), the motion blur's streaks at half-size
  motion, the first frames after a cut now that `reset` is sent there,
  a camera jump, and a transition between two traced scenes.
- The time of `fast_tiles`, `fast_catcher` and the new `resolve_fast` at
  1080 x 1920 (they were about 7 ms for the visibility pass and under a
  second a film for the depth of field before).

What is missing:
- **The editor's preview takes `fast` (wired; not seen on a card).** A
  choice beside the prompt bar, kept with the app's settings: Raster,
  Raster + sky (`set_sky`), Traced (`fast`). Both preview workers go
  through `src/motion_preview.rs`: playing builds on the frames before
  (up to 8 dropped frames still follow), a still frame after a scrub, an
  edit or a change of size is warmed up with 8 frames
  (`try_render_warm`), a jump while playing starts afresh
  (`forget_history`), and a drag always draws the raster proxy. Run on
  Mesa's software Vulkan at 160 x 90
  (`a_traced_preview_warms_up_builds_on_and_gives_way_to_a_drag`,
  `--ignored`, `LAHZA_MOTION_SOFTWARE_GPU=1`): the first still frame
  17 s with its pipelines cold, the next 0.03 s, a warmed frame after a
  drag 0.25 s, with the sky 0.23 s, the raster 0.02 s; the three
  pictures looked at and they differ as they should. A still traced
  frame takes the scene's motion blur as the fast mode's own (`shutter`,
  180° being half a frame; `traced`); a playing one does without. What
  is left needs a real card, since a software GPU's times and noise say
  nothing of it: the window with Traced chosen at a size a person would
  use, Ray Reconstruction, the look of the fast mode's motion blur, and
  two numbers to settle there: whether a warm-up of 8 frames (a fifth of
  a second at the 24 ms measured on the RTX 3060, during which the
  worker draws nothing else) is felt, and whether history should survive
  8 dropped frames while playing.
- **Ray Reconstruction's picture** of all that changed here.

## Lighting between the raster and the traced picture

The raster lights from the environment as a sphere of light (`envmap.rs`:
a diffuse level and levels blurred for roughness; motion.wgsl
`env_diffuse`, `env_light`), with shadow maps for the lights. What it had
not got, and the traced picture has: the environment's light is not
hidden by anything (creases and where things touch do not darken, things
seem to float), and nothing lights or mirrors anything else.

The fast mode is the first answer where it runs (above: it now runs
without NVIDIA too, and `snapshot` and the preview's renderer can take
it). What follows is for a card where it is not quick enough.

- **Sky visibility in the raster, with the light that comes back**
  (`RenderOptions::sky`, `snapshot` and `render` take `sky`, the CLI
  `--sky`; off unless asked). A rasterised list with meshes also gives
  its solid meshes to the tracer (`list.rs` `sky_list`: a `Trace` with
  `TraceOptions::sky`, and a group of half the target's size). For each
  texel the tracer finds the surface it shows and sends four rays from it
  (`SKY_RAYS`), spread by the cosine, toward points of one fixed set: a
  texel has those of its place in a 4 x 4 tile, so sixteen texels
  together have 64 directions (pt.wgsl `sky`). An open ray adds its
  direction and the sky's light along it; one that ends on a surface
  adds that surface's light: its colour under the lights (shadows by
  rays) and under what it sees itself (one ray more from it: see "What a
  ray ends on", below). `sky_filter` makes the sixteen one value over the texels on
  the same plane, each place of the tile weighing the same, and smooths
  that once more two texels apart: each texel has every direction but
  each from another point near it, and its value went in steps that
  showed as blotches on large flat things (two rays and one pass: a
  wall's brightness spread 1.3 levels where the traced picture's has
  0.8; now 0.7, and the test holds it there). The group
  holds, a texel, the mean open direction, the open part and the depth,
  and the light that comes back. The raster's shading reads it by depth
  (motion.wgsl `sky_at`): the light the rays brought (the sky's own
  through the open ones and what came back from the others) added to the
  diffuse light, the lobes times what is open of them, ambient lights
  times the open part, what is mirrored of the environment less by it
  (Lagarde's fit); with an environment image, `env_diffuse` along the
  open direction times the open part.
  Nothing is drawn by lot from frame to frame and nothing is kept from
  the frame before: a raster frame is cached by its lists and drawn in
  any order, so the same frame has to be the same picture (tested).
  The three points:
  - *The structures for a raster frame.* Built by the tracer as for a
    traced list: a mesh's structure once (kept by its parameters; a mesh
    that changes every frame is built every frame, as traced), the
    list's top structure every frame. On software Vulkan, 480 x 270,
    five meshes, nothing else running
    (`sky_tests::measure_sky_visibility`, `--ignored`): a frame takes
    87 ms against 47 ms without (72 ms at two rays a texel and one
    filter pass). By the profile, summed over five frames: the meshes'
    structures 6 ms (built once), the top structure 0.5 ms, the tables
    33 ms on the CPU, and the kernel's pipeline 1.1 s once. Software
    Vulkan's numbers. In the editor's preview: not run.
    With motion blur each sub-frame has its own rays (they are not
    shared as a traced picture's are).
  - *The CPU path* casts the same rays (`sky_cpu.rs`): a BVH over the
    list's triangles in camera space, built each frame, the same
    directions from the same points, the same light back and the same
    two filter passes, all in f32 as the shader, and the CPU's shading
    reads the group as `sky_at` does. For the two to find the same light
    back, that light is one plain rule on both sides (`sky_lit`: the
    surface's base colour, less what is metal, under what its own ray
    brings, the lobes or the image and the ambient lights by how far
    that ray is open, and each other light from its middle with a
    shadow by one ray; an area light as a point), not the tracer's full
    material and soft lights as at first.
    GPU against CPU on software Vulkan, as the other tests of the two
    measure it: no pixel more than one level apart in the test's scene
    (lit by the environment, and with an environment image). A mesh the
    GPU makes in its pool (Slang) is not in the CPU's rays, as it is not
    in the CPU's frame.
    What it costs the CPU at the preview's size (1080 x 608, four cores,
    `sky_tests::measure_the_cpus_rays`, `LAHZA_SKY_TIMES=1`): a frame
    takes 260 to 360 ms against 95 to 125 ms without (two runs). The
    rays 129 ms (164 000 texels, about five million rays a second a
    core), the two filter passes 22 to 29 ms, the BVH 0.3 ms (652
    triangles: a film's scene has many times that), the rest the
    shading's lookups. Nothing of it runs with `sky` off.
  - *Without hardware ray tracing* a GPU draws the frame as today (the
    builder asks for sky visibility where the GPU traces, or where the
    CPU draws).
  So `sky` is still off unless asked, now for what it costs and no
  longer because the two renderers would differ: on the CPU it nearly
  triples a preview frame, and what it costs a card is not known (on
  software Vulkan at 1080 x 608, 165 ms a frame against 104 ms). To turn
  it on where it is cheap enough is a decision for after the card's
  numbers.
  Against a traced frame of the same scene (a ball, a box with a slab
  over it, a floor and a wall; 320 x 180 and 480 x 270, 64 paths):
  - Lit by the environment only: where the ball stands the floor is 0.72
    of the plain raster's (traced: 0.62; the second filter pass softens
    it, 0.69 before), the open floor 0.94 (0.99); the whole picture
    29.5 dB from the traced one (26.0 dB without sky visibility), 3%
    darker than traced (27.6 dB and 4% before the light's second
    return, below).
  - With the default lights: 33.6 dB from the traced picture, 27.3 dB
    without (480 x 270, with the first rule for the light back; at
    1080 x 608 with `sky_lit`, 38.4 dB and 32.5 dB). Seen: the wall takes the floor's light, the ball's underside
    is lit from below, the gap under the slab is dark, as traced.
  - Under a table (a top on four legs, the default lights;
    `sky_tests::measure_under_a_table`): the floor between the legs is
    173 of 255 where the traced picture has 184 and the plain raster
    226: 6% darker than traced (170 and 8% before the second return), the light that comes back a second time
    off the floor, the legs and the top's underside missing. The whole
    picture: 32.5 dB from the traced one, 25.6 dB without.
- **One bounce of diffuse light** was to come only if visibility alone
  still looked flat. It did worse than that: every face that looks at
  another lost the light the environment's ground and horizon stood for
  (the wall and the box's sides plainly darker), and the picture was 16.1 dB
  from the traced one where the plain raster is 26.0 dB. So the bounce
  is in, from the same rays, as above.

What is left:
- The editor's preview asks for it when Raster + sky is chosen
  (`src/motion_preview.rs`); not seen on a card.
- Run on the RTX 3060: `cargo test -p lahza-motion-render sky_tests --
  --test-threads=1` and its measure (`--ignored`,
  `LAHZA_MEASURE=1080,1920`): the cost there (rays, filter, the
  structures of a film's scenes: four rays a texel and two filter passes
  are what keeps flat things even), and the picture
  through the raster's hardware visibility pass, which software Vulkan
  does not have (only the compute raster's shading was run with it).
- **Cards and see-through meshes hide the sky, and see-through meshes
  take it: done**, by a fixed rule, nothing drawn by lot. At first with
  a step at one half (a ray ended on a surface at least half opaque and
  on a card where its picture covered more than half, and went through
  anything less). Since, *coverage is a ray's weight*: a surface takes
  of a ray what it covers (its material's opacity times what it does
  not let through: pt.wgsl `sky_cover`; a card its picture's coverage
  there), its light comes back by that much, and the rest of the ray
  goes on to the next thing, four things at most, until under 0.004 of
  it is left (`SKY_LEAST`; the loop in `sky`, the same in
  `sky_cpu.rs`). The shadow rays (a light's at a ray's end, and the last
  ray) are what every thing on their way leaves of them (`sky_shade`).
  It costs a ray only where something covers in part: a scene of solid
  things casts what it cast. Still a step at one half, and why: the
  camera's ray (it says whose texel it is, and a texel has one surface)
  and the ray that goes on from a ray's end (a share of a share: it
  would be a ray more for each thing met).
  - The sky's list has the list's cards as a traced list has them (the
    flat 3D layers `emit_run` draws among the meshes; `list.rs`
    `sky_list` makes their pictures before the group the rays fill, and
    `card_item` takes those instead of drawing each a second time) and
    its see-through meshes (`mesh3d::in_sky`).
  - A card's colour comes back as it is (a card is a flat picture, not
    lit, in the raster and traced alike: so it has no sky to take, and
    the camera's ray goes through cards to the surface behind, which
    keeps its texel).
  - A see-through batch reads the group for its own surfaces only: it
    has texels where it is at least half opaque (the camera's ray stops
    there); where the texels are of what is behind it, it is lit as
    before (`Batch::set_sky`'s `own`). What is behind a see-through mesh
    that took the texel reads the nearest plane, as at an edge.
  - The CPU's BVH has the cards' quads, and reads their pictures from
    the groups drawn before the sky's (`sky_cpu.rs`).
  Measured (`sky_tests::cards_and_see_through_meshes_hide_the_sky`,
  320 x 180, lit by the environment, software Vulkan): the floor under a
  card lying over it 192 of 255 where it is 219 without the card, and it
  takes the card's blue as the traced picture does; beside a box 0.7
  opaque 195, beside one 0.3 opaque 204, without the box 215: 0.91 and
  0.95 of the floor without it, for the traced picture's 0.90 and 0.96
  (with the step at one half the 0.3 opaque box hid nothing: 1.00, and
  the test asked for that); a box 0.99 opaque is lit as a solid one
  where sky visibility darkens that most (205 and 205, 223 without).
  GPU against CPU: no pixel over 3 levels in the three. Seen: the card's
  shadow on the floor against the traced one (the traced is grainier at
  its edge, the same size and tint).
  Since:
  - *Volumes* hide the sky by their density: every crossing of a
    volume's faces along a ray is found, the density between them is
    marched in eight even steps a crossing (pt.wgsl `sky_through`, the
    tracer's own density lookup; its walk through a volume is by lot,
    this is not), and that much less of the ray goes on: less of the
    open sky, less of the light back from what it ends on. The same on
    the CPU with its own density lookup (`sky_cpu.rs` `through`).
    Since, found while a volume was given its own sky: the march told
    volumes apart by their material row, and a box's faces each have a
    row of their own, so every ray counted one crossing of each face's
    "volume" and was taken to start inside it (the density from the
    ray's start to each face, not between the faces). They are told
    apart by instance now, on both sides. The floor before the slab of
    thick haze was 199 for 220 with nothing there; it is 216, and the
    traced picture has 221.
  - *What a volume takes of a ray, it scatters back: done* (it sent
    nothing back). By the raster's own rule for a volume's light
    (`volume_lights` without its tap toward the lights: every light by
    its phase, the environment and the ambient lights evenly), where the
    volume takes most of the ray, the middle of what it takes (pt.wgsl
    `sky_volume_light`, `vol_at`); the even part by what one ray from
    there finds, any way round: open, the sky's light along it, else the
    light of what it ends on (lit as a ray's second end is: `SKY_REST`).
    Without that ray a haze under a roof sent back the open sky's light.
    Where several volumes share a ray, the one that takes most of it
    stands for all. `sky_tests::a_volume_scatters_light_back_and_takes_the_sky`:
    the floor before a slab of red haze has 12 levels more red than
    before a black one (226 and 214 of 255; traced 17 more, 233 and
    216).
  - *A volume takes the sky itself: done* (it was lit by its own march
    as if open). The camera's ray finds the volume before its surface
    (or before nothing) and where it takes the ray; from there four
    rays all round, of the fixed set, an open one bringing the sky's
    light and another the light of what it ends on (the floor under a
    haze lights it from below, where the ground's light was). As one
    number, how much of the environment's mean light from above and
    below that is, which is what the raster lights a volume by: the
    group's third row's fourth number, filtered as the rest over the
    texels that have a volume, read by the volume's shading
    (motion.wgsl `sky_volume`; `volume_lights` takes the ambient lights
    and the environment by it; the same in mesh3d.rs and `sky_cpu.rs`).
    One number, so the colour of what lights it from below is lost; and
    the lights themselves are not shadowed on a volume, with or without
    sky visibility (the raster's volume has no shadow maps). The same
    test: a long box of haze, half of it under a dark roof, of 255: 233
    in the open and 148 under the roof (230 without sky visibility;
    traced 216 and 108, where the roof also shadows the lobe overhead
    as a light). Seen: the roof's shadow on the floor and the darker
    haze under it, as traced.
  - *A filled wireframe* hides the sky by its faces' fill (at least half
    opaque) and sends its fill's colour back as it is; *an unlit
    surface* sends its own colour back (it was lit like any other).
  `sky_tests::a_volume_hides_the_sky_by_its_density` (320 x 180, the
  floor in front of a slab hanging over it, of 255): nothing there 213,
  thin haze 209, thick haze 190, a cloud of the thick haze's density
  210, a filled wireframe 183; GPU against CPU no pixel over 3 levels in
  any (those with the march's fault, below; now 220, 219, 216, 219 and
  194, the thick haze's 216 for the traced 221). Seen: the slab of haze's soft shadow on the floor, the cloud's
  fainter one, the wireframe box's as a solid's.
  - *The cost of cards in the rays*
    (`sky_tests::measure_cards_in_the_rays`, `--ignored`, 480 x 270,
    software Vulkan): sky visibility added 26 ms a frame to the scene
    without a card (35 to 61) and 46 ms with one (41 to 87): a card's
    picture is read at every crossing. On the CPU 55 ms and 66 ms.
    With the rays' ends lit by their own rays and coverage as a ray's
    weight, in another container: 46 ms without a card (41 to 87) and
    35 ms with one (59 to 94); on the CPU 67 ms and 80 ms.
  Hiding nothing, and meant to: a shadow catcher (it is not there but
  for the shadows on it), a wireframe's lines without a fill (a px or
  two of the screen, whatever the distance: nothing a ray can meet), a
  mesh that blends otherwise than over what is behind it (a glow adds
  light), a flat layer under 0.999 opaque (it is drawn flat, not as a
  card in the depth). There is no opacity from a map (materials have
  none: that line was a guess). Needs slangc, which a container cannot
  fetch: a mesh whose coverage a Slang surface works out (`in_sky`
  leaves it out; with slangc, its coverage would be asked of the
  surface's function at each crossing).
- **The two-px steps at an edge between two surfaces: done.** The
  lookup told surfaces apart by depth alone, so two at nearly one depth
  mixed, a texel at a time (a crease, a slab's front over the gap under
  it). The group now has a third row of texels, each surface's own plane
  (its true normal as two numbers, and where it lies: `sky` had it for
  the filter), and `sky_at` takes a texel for the shaded point's surface
  when the point lies in its plane (within 0.01 z + 0.5, the filter's
  own rule) and it faces as the triangle does (the triangle's true
  normal: 0.9 and over whole, none under 0.8); with none, the texel
  whose plane is nearest. A floor seen at a glancing angle is in its
  texels' plane at every depth, which a tighter depth test was not. The
  same in `sky_cpu::at`. Measured on a box with a slab over a gap and a
  block (`sky_tests::an_edge_at_one_depth_has_no_steps`, 320 x 180,
  software Vulkan and the CPU alike): neighbours on one flat face more
  than 6 levels apart, 53 before (up to 15 levels), 1 after (9 levels,
  at the gap's end where three faces meet within a texel); GPU against
  CPU no pixel over 3 levels. Seen: the staircase under the slab's
  front edge is a straight dark line. The room's numbers moved a little
  with it (the contact 0.74 of the plain raster's for 0.72, 29.7 dB
  from traced for 29.5). The group is half again as large (three rows
  for two). Its cost measured again with all four of this session's
  changes in (software Vulkan and four cores, another container than
  the numbers above; the measures now draw every frame, see "Motion
  blur" for why a still scene's frames had come from the cache): at
  480 x 270 a frame takes 124 ms against 62 without (it was 87 against
  47: the same doubling, on a slower run); the CPU at 1080 x 608 329 ms
  against 118, the rays 125 ms and the two filter passes 30 to 47 ms
  (they were 129 and 22 to 29).
- **What a ray ends on is lit by what it sees itself: done** (it was lit
  as if it had the whole environment, which the session before measured
  as the largest error left: 148 for the traced 134 under the ball, 180
  for 194 under the table). Each ray that ends on a lit surface goes on
  from there once more, by the cosine, in a direction of a fixed set
  (pt.wgsl `sky_turn`, `sky_onward`; the same in `sky_cpu.rs`): open, it
  brings the sky's light along it; else the light of what it ends on,
  and that one is lit by whether a last ray from it is open (a shadow
  ray). A last ray that is not open counts as half the environment
  (`SKY_REST`), for all the light that would still come back.
  The plan had been to keep each ray's light apart and ask the texel
  its end is seen in (one more buffer row, no ray). Not done, by what
  the scenes show: the ends that matter are not in the picture (the
  ball's underside over the floor; the table top's underside, which the
  floor around it lights and which lights the floor under it). The
  second return that did go by the picture (`sky_second`) is gone with
  its two buffer rows: the ray has it, for surfaces the picture does not
  show too.
  And the measure showed that most of what the room was off by was not
  at the rays' ends at all. The open sky's own light was the raster's
  fit about the mean open direction, times the open part: a floor
  beside a wall took the ground's dark light (the mean direction leans
  from the wall, and the fit about it reaches under the horizon), and a
  lobe overhead was hidden by what a wall hides sideways (the open floor
  before the wall 210 for the traced 221). Now an open ray brings the
  sky's, horizon's or ground's own light along it, with the light back
  (the group's second row; the raster turns it by how a surface's detail
  turns the fit), and the lobes go by how much of their weight the rays
  found open (that row's fourth number; a narrow lobe the rays hardly
  meet, by the open part). An environment image is as before: its
  diffuse level about the mean open direction, times the open part.
  The second filter pass smooths less across a change of the open part
  (where things touch the dark is narrow, and the even tent spread it).
  Measured against the traced frame, 64 paths, 320 x 180, software
  Vulkan and the CPU, which give the same numbers to the last digit
  shown (`sky_tests::measure_against_traced`, `--ignored`; before, from
  the same scenes at the commit before):

  | | dB from traced | the picture | the dark floor | the open floor |
  |---|---|---|---|---|
  | the room, soft environment: traced | | 204.8 | 133 | 221 |
  | before | 29.7 | 198.8 | 148 | 210 |
  | now | 30.8 | 199.3 | 141 | 218 |
  | the room, no lobes: traced | | 171.8 | 115 | 184 |
  | before (the ends lit whole, no second return) | 34.3 | 169.5 | 124 | 180 |
  | now | 35.8 | 169.9 | 120 | 183 |
  | the room, default lights: traced | | 244.1 | 209 | 252 |
  | now | 36.1 | 242.0 | 202 | 251 |
  | the table, default lights: traced | | 238.5 | 184 | 251 |
  | before | 32.5 | 234.5 | 173 | 249 |
  | now | 34.4 | 235.4 | 177 | 251 |

  (The room under the default lights at 480 x 270: 37.0 dB before, 38.0
  now.) Tried on the way, on the CPU against the same traced frames: the
  ends lit by whether one ray from them is open and nothing else, 21.9 dB
  in the room and 27.8 under the table (the light the blocked rays stood
  for is gone); the second end lit as if open, 30.6 and 34.9 dB but the
  dark floor under the ball 148; with nothing beyond a last ray that is
  not open, 29.8 and 33.8 dB; half, as kept, 30.7 and 34.4 dB.
  What is left of the dark floor under the ball (141 for 133) is the
  filter's width, two passes of a 5 x 5 tent at half size: with the
  second pass even it was 146.
  The cost: on software Vulkan at 480 x 270 a frame takes 137 ms against
  64 without (117 against 66 at the commit before, in this container:
  the trace 119 ms a frame for 78); the CPU at 1080 x 608, four cores,
  416 ms against 128 without. Tested:
  `sky_tests::a_rays_end_is_lit_by_what_it_sees` (the floor under the
  table within 9 levels of traced, the open floor and the wall without
  lobes within 4 and 5, GPU against CPU).
  *A third return* (`sky_tests::measure_a_third_return`, `--ignored`,
  256 paths): the traced picture from 2 bounces to 3, +3.9 over the
  picture and +6.0 at the darkest floor (the room), +1.1 and +7.0 (the
  table); from 3 to 4, +1.0 and +1.7, +0.3 and +1.7; from 4 to 5, +0.3
  and +1.4, +0.1 and +0.8; from 5 to 6 under half a level. The rays now
  have the third (the last ray, where open) and `SKY_REST` stands for
  the rest.
  The method's limits, which more rays in the picture's plane do not
  lift (the fast mode has none of them): nothing is mirrored of other
  things; an environment image's light is its diffuse level about the
  mean open direction; a volume's sky is one number. A mesh the GPU
  generates in its pool is not in the CPU's rays, as it is not in the
  CPU's frame at all.

Not to do: light probes placed through the scene (irradiance volumes,
DDGI). They suit large scenes that stand still; these are small and
everything in them moves, a grid would be relit every frame, and probes
blur away the contact shadows that are what is missing.

## Simulation: rigid bodies, ragdolls, crowds

All seven parts are built as far as this machine could take them (no
GPU, no trained policy): what each does, what was measured and what is
left is below. Morph targets are still refused. Across all of it: a bake
is the same bytes from debug and release builds and on two containers
(both Intel Xeons with FMA and AVX-512; the hashes are pinned in
`motion-sim`'s tests, those for getting up, bones meeting, instances,
a way round a wall, stepping aside and the search confirmed on both;
the four pinned last, for a knocked agent's swing and held pose, bodies
in an instancer and getting up from its back, were made on one). Nothing a bake is made
from goes through the C library now: rapier's maths is software
(`enhanced-determinism`), the behaviours, crowds and search use `libm`,
and everything `compile` works out (layer rotations, curves, cameras)
goes through `core::dm` (`libm`'s sines, powers and logarithms; lengths
by multiply and square root), which left every pin as it was. Run on
other kinds of CPU by the coordinating session (before that last change,
at fa1dfd7): on x86-64 with glibc kept off its FMA and AVX paths
(`GLIBC_TUNABLES=glibc.cpu.hwcaps=-FMA,-AVX2,…`; a C program's sines
do hash differently with and without) all 35 of `motion-sim`'s tests
pass and every pin holds; on aarch64 under `qemu-user` 34 pass, every
pinned bake among them, the one failure being the policy test, whose
ONNX Runtime is x86-64 only. Not run: a real ARM machine.

A later round (one container, a Xeon with FMA; `motion-sim` 50 tests
with 7 ignored measures, debug and release) took the lines each part had
left. Pinned new, in `motion-sim`'s tests: a rope of links wound round a
bar (61580 bytes, FNV 11416658259355396517), bodies two instancers deep
(16679, 7626806709829618763), an emitter's particles (23075,
9110451255763848001), a human's limits from bone names (57712,
16705265968754114252), six takes of arms led round the body (36621,
11581137727660598137; 36488, 947309298340335586; 36488,
17833138615412773368; 36488, 16887651574034388283; 36552,
8588104402154634968; 36552, 7065585522355567997), a take varied by
its seed (57021, 6885115440864738725), getting up under the search
(126115, 13679216537685136859), a crowd with its guard (18476,
11403343376360516901). Changed, and why: getting up from four falls
(139438, 3354145111831044034 → 10638684219171540933: an arm on its
way back to the clip now meets the body) and the search's take (56950,
17133235751400427189 → 56831, 7627719729510559245: the same, and an
elbow's bend an arm in its plan). Every other pin is as it was: what
was added to a bake's input (a rope's links, a particle's lives,
`vary`, a guard, named feet) is hashed only where a scene has it, and
the takes pinned with 40° joints say them outright now that names give
a human's. These pins were made on one machine and not yet on a
second. In the order built:

1. **Rigid bodies and joints: done.** `crates/motion-sim` steps
   `rapier3d` 0.36 (`enhanced-determinism`: no SIMD, no threads, libm).
   - A layer with `"body"` is a body; the scene's `"simulation"` has the
     settings, a ground, joints (fixed, hinge, ball, slider, rope, spring;
     hinges and sliders with a keyed motor) and blasts. Pushes, releases
     and blasts take time references. Shapes come from the mesh (sphere,
     box, capsule, cylinder, plane as such; else the hull, convex pieces
     on request, or the triangles for what does not move), from a comp's
     meshes as one body, or are given. A body follows its keys until its
     `release` and leaves them with their speed.
   - Baked: `sim_bake` / `lahza motion bake` step at 240 Hz (a whole
     number of steps per frame) and write `sim/<scene>.bake` (a pose per
     frame per body, f32, plus the take's first touches with their
     closing speeds). `compile` places a body's layer from the bake
     (`layer_matrix`), between samples by interpolation, so scrubbing,
     motion blur's sub-frames, both renderers and exports read it like
     keys. The simulation is in the scene's space; bodies inside comps
     are placed through the comp's inverse.
   - Stale bakes: the bake carries a hash of what it was made from (the
     bodies' shapes, keyed poses per frame, joints, motor targets, pushes,
     settings, the bytes of glTF files colliders come from). `check`
     fails with `simulation` when a scene's bake is missing or its hash
     differs. A camera, material or light does not change the hash.
   - Deterministic: two bakes of a project are the same bytes, and the
     debug and release builds give the same bake (its hash is pinned in
     `motion-sim`'s tests; a second container baked the same). See the
     note above for what is not checked.
   - Measured (4 cores, one used): 32 bodies for 4.5 s, 1080 steps, in
     0.044 s; 501 boxes piling up for 5 s, 1200 steps, in 1.36 s (2.4 MB
     of bake); convex pieces of five extruded letters add 0.45 s. `check`
     re-gathers the input each call: 0.34 s in all for the 501 bodies.
   - Tests: free fall against ½gt², rest on the ground, a stack of six
     at rest, a hinge's limits and anchor held under pushes, a motor
     reaching its keyed target, release with the keys' speed, drawing
     through comps, parents and scale, flat (2D) bodies, kinematic
     pushers and blasts, seeds, stale-bake detection, and the tool end to
     end (unbaked, baked, stale, undo, reopened). `examples/sim/tumble.py`
     builds letters that tumble and a wall hit by a ball on a rope; its
     contact sheets were looked at (CPU renderer).
   - In the editor (done; seen under Xvfb, see "How it should be used"):
     it reloads when only `sim/` changes, says beside the prompt bar when
     a bake is missing or stale, and bakes in one click off the UI thread.
   - `snapshot` and `review` warn of a missing or stale bake and `render`
     refuses one unless given `unbaked`.
   - Also built since: a drawn rope (`joints[].draw`: a mesh layer
     stretched from anchor to anchor, in twelve pieces along the parabola
     a rope of its length hangs in when slack; tested end to end, looked
     at in `tumble.py`'s wall), and bodies in instancers (a layer with a
     `body` inside one is a body a copy, `<layer>#<n>` in the bake, in
     joints and in events; tested: a tipped grid of six bricks falls as
     six and is drawn where the bake has each; pinned).
   - A rope of links (`joints[].links`: that many capsules end to end,
     a ball joint between each two and at both ends, baked as
     `<joint>/<n>` and drawn joint to joint with the `draw` layer; the
     curve without `links` stays the cheap choice). The rope joint is
     kept beside the chain and bears the load, so what hangs is never
     farther than the rope is long; links never touch the two bodies the
     rope is tied to, and scenes with one take 12 solver passes. Tested:
     a 5 kg ball on 400 px of rope in 16 links swings through and its
     links stay end to end (under 404 px joint to joint); with a bar
     across its swing the rope lies on the bar and the ball is wound
     round it (no link's middle more than 1 px inside the bar), where
     the drawn curve swings on through. A heavier ball presses the rope
     into the bar: 2 px at 50 kg, 12 px at 500 kg (a link is light
     beside it). Pinned. `tumble.py`'s wall uses it (looked at: it sags
     a little by its own weight while held, bends on the brick it meets
     and whips after the hit); 16 links add nothing that shows to the
     bake's time (48 bodies, 0.25 s).
   - Bodies two instancers deep (`<layer>#<copy>#<copy>`; tested: two
     stacks of three bricks from an instancer in an instancer fall as
     six and are drawn where the bake has each; pinned), and three deep
     refused with the reason. Bodies as an emitter's particles: a body a
     slot (`<layer>#<slot>`), out of the world but while the slot holds
     a particle; at each birth it is put where the emitter has the
     particle, with the speed of its flight, and from then it is the
     simulation's (the emitter's own gravity and drag are not; it keeps
     the size it was born with, and its opacity and tint over life).
     Before a life's first sample it is drawn where the emitter has it;
     the bake's sample before a later life is where that particle came
     from. Tested: a slot's lives against the emitter's rate, the speed
     it leaves with, that it lands on a floor a particle falls through,
     drawn where the bake has it while it lives; pinned. `tumble.py`'s
     `pour` (blocks thrown onto a tilted plank, sliding into a heap; 33
     slots for 5 s bake in 0.05 s) was looked at. An emitter inside an
     instancer, or holding one, with bodies is refused.
   - Left, needing what is not here: Slang-generated meshes as
     colliders (their points come from `slangc`, which this machine has
     not; the code path is the hull of a generated mesh, untried).
   - A box landing flat at speed tips before it settles (3.7° on the
     test's box, which falls 6.5 m): looked into, and it is its bounce.
     The four corners of the face each get their rebound, worked out
     one after another by rapier's solver, and the first to be served
     lifts first. Tried on that drop (`measure_a_flat_landing_by_solver_settings`,
     an ignored test that prints the table): 16 solver passes 1.4°; 4
     inner PGS passes 2.6°; 4 inner stabilization passes 3.5°; all three
     3.7°; no warm start 3.7°; contact clustering or recycling off 3.7°;
     friction in the bias pass 3.2°; Coulomb friction 3.7°; contacts
     softer (10 Hz) 3.7°, stiffer (120 Hz) 3.7°, more damped 3.7°; no
     prediction distance 2.2°, ten times it 9.6°; a contact skin 3.3°;
     no friction 3.6°; a box for a floor 3.7°; 960 steps a second 8.1°.
     Only one thing takes it away: `"bounce": 0` on the body and on the
     ground, 0.24° (tested). No setting is changed: none of them mends
     it, each would change every pinned bake, and the default bounce
     stays 0.1 because things that land on an edge or a corner should
     rebound. For a slab that must land dead flat: `bounce` 0.
   - Left, needing what is not here: a solver that works out a face's
     contacts together (a block solver) is rapier's to have, not a
     setting of it; with one, a flat landing could keep its bounce.
   - Left, not done: a ragdoll's or a crowd's layers cannot be copies
     of an instancer (a crowd of instances is the way to many of one
     character).
2. **Skeletons and skinning: done.**
   - `model_import` keeps a glTF skin: a joint node is a comp like any
     node, and the skinned mesh layer has `"skin": {"index", "joints":
     [layer ids]}`. Joints are ordinary layers, so keys, parents and
     bakes pose the mesh; `compile` gives every skinned mesh of a frame
     its joints' matrices (`MeshState::skin`).
   - CPU and GPU skin from the same f32 matrices (`core::skin`): the CPU
     in the mesh cache (a mesh per pose, kept while drawn), the GPU in
     its mesh pool (`meshskin.wgsl`: the bind mesh with joints and
     weights uploaded once, three rows a joint per pose, a mesh per
     pose). The path tracer takes the skinned mesh from the pool. A
     skinned mesh's bounds are a box per joint carried by the joint.
   - Tested: points and normals against a pose worked by hand; the test
     arm's corner against its glTF by hand; rest pose equals the
     unskinned mesh; the GPU's pool skinning against the CPU's on six
     frames and one path-traced frame (lavapipe, pool counts checked);
     the import and a joint keyed by an edit, through the tools.
   - Eight joints a point: glTF's JOINTS_1 and WEIGHTS_1 are read
     (`SkinData::more`; weights are made to sum to one over the eight;
     JOINTS_2 is refused). The CPU sums them in order and the pool's
     bind mesh has six rows a point for such a skin. Tested: the test
     arm with each point's second joint moved to JOINTS_1 skins to the
     same points, to the bit, as the arm said with four; drawn the same.
   - Dual quaternions beside linear blend: a skin's `"blend": "dual"`
     (`skin::Posed`, `skin_point_dual`, and the second half of
     `meshskin.wgsl`: five rows a joint, the real and the dual
     quaternion and what is left of J · B, its scale, which is blended
     as matrices first). Each joint's quaternion is taken on the side
     of the point's first. Tested: a ring half on a joint twisted 180°
     is a point blended as matrices and as wide as it was as dual
     quaternions, turned a quarter; one joint alone is its matrix
     either way, scale and all; across the test arm's quarter bend the
     ring between the joints is within 1 % of its width at rest, and
     more than 5 % thinner blended as matrices (looked at: the elbow's
     outside is round, not cut). The GPU against the CPU on software
     Vulkan: dual quaternions with eight joints a point over four
     frames, and in the pool dual, eight, and both, three frames each
     (pool counts checked).
   - Imported rotations follow the file's slerp: glTF turns from key to
     key the short way at an even speed, a layer's three angles each go
     straight between their keys. `model::rotation_keys` adds keys
     between the file's until the angles' lines are within a quarter of
     a degree of the file's turn, and takes of a rotation's two sets of
     angles the one nearer the key before. Tested: a 170° turn about a
     skew axis and on through where the two sets meet stays within a
     third of a degree of slerp at every hundredth of a second (tens of
     degrees off with the file's keys alone); a turn about one axis
     gets no more keys than the file has.
   - A `.gltf` is imported with what is beside it: the files its buffers
     are in are copied with it (kept at their paths under
     `assets/models/<id>/`, registered as assets), and pictures it names
     by path are read from its folder and copied like embedded ones. A
     buffer that is a link or a path out of the folder is refused with
     its name. Tested through the tools: the arm with `arm.bin` and a
     picture beside it, imported, its source folder deleted, checks and
     draws.
   - Measured, one core, 100 000 points: 5.2 ms with four joints each
     (linear), 9.8 ms with dual quaternions, 5.7 ms with eight a point
     of which four have no weight. The GPU side is not timed (lavapipe
     is no measure).
   - Left, needing what is not here: a skinned mesh under a Slang
     deformer or a Slang displacement is not deformed (both need
     `slangc` to try). Morph targets are refused (a model that has them
     to try on: none here, and they are a second kernel in the pool,
     whose GPU half wants a card to look at). What the pool's kernel
     costs on a card, with a character of real size.
3. **A ragdoll that falls: done.** `simulation.ragdolls`: every bone
   under `root` (the joints' comps) is a body, a capsule along the bone
   toward its child (a given shape where that is wrong: box feet), mass
   shared by volume with no bone lighter than three quarters of what
   hangs from it. Joints are balls with swing and twist limits, or hinges
   (knees, elbows), measured from the pose at `rest`. Until `release` the
   bones follow the clip; they leave it with its pose and speed.
   - Tested: a limp figure falls in one piece (every bone within 4 px of
     its joint at every sample) and comes to rest; limits hold under
     hard hits, knees bend one way about their axle; release with the
     clip's speed and a raised arm; the same bytes twice, and from debug
     and release builds; a skinned model through the tools.
   - Found on the way: a pelvis far lighter than the legs it joins tore
     its joint (162° past a 40° limit), hence the mass rule and 12 solver
     iterations for scenes with ragdolls.
   - Its bones meet each other (`self_collide`, true by default): all
     but those jointed together and those whose shapes are within 2 cm
     of each other as the skeleton stands at rest (worked out once from
     the bind pose: a chest's capsule holds its shoulders, a hip's its
     thighs). Tested: the limp figure with a hand thrown across it has
     its forearm 21 px and its hand 24 px inside its chest without, and
     no bone more than 2 px inside another with (the forearm lies on the
     chest), through the fall and at rest; pinned. While it gets up the
     whole body still passes through itself (it is placed pose by pose).
   - Limits come from the bones' names where none are given
     (`sim::human_limit`: a table of a human's ranges by the names the
     usual rigs use, Blender's, Mixamo's, Unreal's and plain ones;
     knees and elbows hinges of 140° whose axle is across the bone and
     the way the figure faces, `facing`, so that knees fold back and
     elbows forward; thighs 70° and 25° of twist, upper arms 100/60,
     feet 35/10, hands 50/20, a head 35/40, a spine 25/20, and so on). A
     bone's own name is its id without what the skeleton's ids share
     (the model's id). They are defaults, round figures on the loose
     side: an entry in `limits` stands before them, and one for bone ""
     takes every bone the entries before it leave. A name that tells
     nothing keeps 40° and 20°. Tried on the test character: with no
     limits given, `ragdoll.py`'s takes bake to the same bytes as with
     the limits that were written for it by hand. Tested: the figure's
     hinges and ranges with none given; thrown hard, no joint past its
     range, knees back only and elbows forward only, about their axles;
     limp and shoved at 150 px/s a knee folds to its 140° and the hips
     are down in 0.9 s (forward) or 1.4 s (back), where 40° joints all
     round fold it 57° and it comes down as a plank in 2.1 s. Pinned.
     The takes pinned before say their old limits outright (an entry
     for ""), so their bytes are as they were.
   - An aimed arm goes round the body (`Brain::route`): a target inside
     the trunk is brought out to its surface (a hand goes onto a chest,
     not into it); while the straight way from the hand passes through
     the trunk or the head (their capsules, wider by the forearm's
     thickness) the hand makes for a point beside it there, on the side
     the way leans to and toward the body's front, and what it makes
     for is moved out until the forearm clears too; worked out anew
     each step. The forearm and hand of an aimed arm now meet the body
     (they passed through all of it), but for the one bone the arm
     hangs from, whose capsule is the fat one that holds the shoulders:
     with that met too, on the test character the arms were held up on
     their own chest. The upper arm still passes. Tested on six takes
     (arms about the head, a hand to a hit on the chest, to the other
     forearm, the left to the far thigh, and each hand to a ball across
     the body): a forearm or hand never more than 4 px into the hips,
     spine or head and 12 px into its own chest's capsule, where before
     forearms were 24 to 30 px into the spine in every one and a hand
     39 px into the chest; each hand gets where it was sent (wrists 25
     to 53 px from it; the far thigh, 88 px, is past the arm's reach
     round the hips), and it stands through all six. Pinned. On the
     test character (`ragdoll.py`'s `brace`) the wrists are 19 to 40 px
     from the head's joint from 1.2 s to 1.6 s (read off the bake; on
     the sheet the figure is small).
   - Left, not done: an arm goes round its trunk and head only, not
     round its legs, the other arm, or anything outside it; the elbow
     is where the hinge puts it (no swivel is chosen to keep it clear:
     the forearm is kept clear by moving what the hand makes for).
4. **A ragdoll that acts: done, with balance helped.**
   - Motors follow the clip: `strength` 0..1, keyed, per limb with
     `strengths`. A motor is a second joint beside the one with the
     limits, its frame turned to the target each step (rapier's motors
     take a turn apart by axis and swung a raised arm; explicit torques
     blew up the spine). Its torque is capped by what hangs from the
     joint, which is what stops it pressing into a wall harder.
   - Behaviours with params (`core::sim::BehaviourKind`,
     `motion-sim/behaviour.rs`): `balance` (ankle lean, and a force and
     torque on the hips capped at `assist` of its weight; past that it
     falls, says so and goes soft), `step` (the foot behind is carried to
     where the body is falling to), `guard`, `reach`, `protect` (two-bone
     arm aim toward a body, the last hit, the head), `get_up` (after a
     fall and 0.8 s still: a roll onto its front, hands pushed down
     under the shoulders, hips and knees folded, then up to the clip in
     2.4 s, facing the way its head lay; then balance again).
   - Getting up is helped, more than balance is. The motors shape it;
     what lifts and rights the body is a force on the hips capped at
     `help` (0.6) of its weight, and a torque capped at what that force
     would make at hip height. The `rise` event carries the percent of
     its weight the force bore over the rise: 23 to 35 on the test
     figure by the way it fell. With `help` 0.2 it still rises from all
     four falls (the force bearing 15 %; one of them on its second
     try), with 0 from none (it folds, pushes, and lies again).
   - Measured on the test figure (70 kg, pushes at the chest, px/s; its
     bones meeting each other, which made it sturdier: its legs no
     longer cross): a statue (motors only) holds 120 and falls at 200;
     ankle lean alone holds 300 and falls at 400; with the default help
     balance holds 1400 and falls at 1800; with steps it holds 1800 (six
     steps, 370 px) and falls at 2400. Steps without the help hold 700
     (eleven steps) and fall at 1000.
   - Tested: each of those thresholds, soft after a fall, hands at the
     head within 48 px and down again, a hand on the hit within 25 px in
     half a second and the other arm for a hit arm, a hand up to a ball
     that comes; knocked flat from four sides it is up
     again (hips within 16 px of where they stood, chest over them) and
     stays up, not without the force, the same bytes twice (pinned).
     `examples/sim/ragdoll.py` (limp, held, braced, rise) was looked at:
     the rise reads as a get-up (on its back, over, hands and knees, up),
     with a sway as it comes upright.
   - Getting up has two ways (`way`): from its front, or from its back
     (it sits up over its folded legs; from all four falls with `near`
     on the standing test figure, pinned). From its back it is less
     sure: in `ragdoll.py`'s `sit` scene, on the walking clip, it stands
     at 5.2 s and falls backwards again at 7.0 s.
   - Left, and it is the hard part the plan names; what would close it
     is a controller that finds its own footing, which is part 6's
     trained policy (a GPU farm, motion capture, days) or a far larger
     search than a bake can pay for: balance is not footwork, it is a
     bounded hand on the hips; a step is a foot carried by a force, not
     a leg that swings; getting up is a pose sequence under an outside
     force, not a body that finds its feet (its feet stay where they lay
     and it is drawn over them; it does not step to them); no behaviour
     looks where it is going.
   - Feet, hands and a head can be named in the document (`feet`,
     `hands`, `head` on a ragdoll: bone layer ids; a hand's arm is the
     two bones it hangs from, and it is the left one when it is on the
     figure's left as it faces); without them they are found by name
     (`foot`, a last `toe`, `hand`, `head`) as before. Tested: said as
     the names have them the take is the same sample for sample; one
     hand said, only that arm shields; the chest said to be the head,
     the hands go to it; what is no bone, or hangs from fewer than two,
     is refused by name.
5. **Directing it: done as far as the tools go.** Intent is written on
   the timeline in the document: `hits`, bodies thrown, `release`,
   `strength` keys per limb, behaviours over spans, all with time
   references. `sim_bake` returns the take: per ragdoll whether it fell
   and when and how many steps it took, then every event (`fall`, `step`,
   first touches with their speeds). `snapshot` and `review` draw it like
   any shot. `sim_bake {seed}` sets `simulation.seed` and bakes: with
   `jitter` each blow and each release differs by seed, and the same seed
   gives the same bytes back.
   - Tested through the tools alone (`motion-mcp/src/sim_tests.rs`): a
     push, arms up, steps, no fall; redirected to a harder push without
     steps, a fall reported at its time; a stale bake between; two seeds,
     two takes, the first again.
   - Left, needing what is not here: an LLM in the loop (the test is the
     calls one would make; a model to make them is outside this
     machine), and with it anything that scores a take against the
     intent, which is that model's judgement of the sheet.
   - `snapshot` says what happened by each frame since the frame before
     it in its scene (`frames[].events`, from the bake), and a sheet
     marks it over the foot of the frame: what a figure did by name
     (`fall 1.20`, `step ×2`), the hardest touches, and how many more.
     Tested through the tools: every event of the take up to the last
     frame is listed once, none on a frame before the push, and the
     marks are on the sheet (looked at).
   - Takes differ by seed in how a body reacts too, when asked:
     `simulation.vary` (0..1) gives each ragdoll a turn of mind for the
     take, drawn from the seed: its arms answer up to a sixth of a
     second late (`reach`, `protect`), its steps are up to a quarter
     longer or shorter and as much readier or slower to come (`step`,
     and how near a thing a `guard` waits for), its balance's gain up to
     15 % more or less. At 0 (the default) nothing is drawn on and
     bakes are as they were. Tested: with no jitter, six seeds of one
     pushed figure raise their arms between 0.73 and 0.80 s and end
     with the leading foot from x 1130 to 1183; all six step and stay
     up; the same seed is the same bytes; pinned.
6. **A network that sets the motors: the interface and the no-training
   route are built; no policy was trained.**
   - The interface (`motion-sim/src/policy.rs`): once a frame an
     observation of 16 + 11·(bones − 1) + feet numbers (up, the root's
     velocity and turning in its frame, its height, the nearest foreign
     body's place and speed, per bone its turn, turning and the clip's
     turn, per foot whether it touches) and an action of 3·(bones − 1)
     (a turn per bone on top of the clip's target). `controller:
     {"type": "policy", "src": …}` runs an ONNX file through `ort` on the
     runtime bundled for camera matting. Tested with four small files
     made by `tests/policy/make.py`: zeros leave the clip, a constant
     turns its joint by that much, a weight on the height observation
     arrives as 0.5 × 2.25 rad, a file of the wrong size is refused with
     the counts. The policy's file is in the bake's hash.
   - Not done, and not doable here: training. It needs a simulator run
     thousands of times at once on a GPU, motion capture (licences to be
     read) and days; and a policy trained in another simulator (Isaac,
     MuJoCo) may not hold in rapier with these capsules and motors. If
     one is trained, this layout is what it must be trained to.
   - The no-training route (`search.rs`, `controller: {"type":
     "search"}`): each 0.1 s, 16 tries 0.5 s ahead in copies of the
     world. A try is a plan: three leans (ankles, hips, spine), a swing
     of each upper arm and a bend of each elbow (twelve numbers; a
     search over every joint would need far more tries), and for a third
     of them, when its weight is headed off its feet, a step of the foot
     behind to where it is falling to or up to 1.8 times as far. Scored
     on falling, weight past the feet, tilt, height, effort and a little
     for a step. The tries run on every core; the plans are drawn before
     any is tried and the first of the least cost kept, so the take is
     the same on one core as on four (tested to the byte, pinned).
   - Knocked down, it gets up: with `get_up` among its behaviours (and
     no `balance`) a figure the search keeps up falls, lies, rises as
     `get_up` does it (helped, as said in part 4) and is the search's
     again once it stands; nothing is tried while it is down. Tested:
     shoved at 1800 px/s it falls, is up again within 7.5 s, and stands
     to the end of a 9 s take with its hips within 20 px of where they
     stood; pinned.
   - Measured on the test figure, pushes at the chest in px/s, with no
     force on its body (a step's foot is carried by a force, as the
     `step` behaviour's is), with the elbows in the plan: motors holding
     the clip fall at 300. Search at the default 0.5 s holds 1400 with
     up to five steps and falls at 1800 (before the elbows it held 1000
     and fell at 1400); at 1 s the same, 1400 and 1800. With a 0.25 s
     horizon it now falls at 300 (twelve numbers are more than a quarter
     second's look ahead can tell apart; it held 300 with ten); 32 tries
     hold 1000 and fall at 1400, less than 16 do (more tries are not
     better: the first of the least cost is kept, and a costlier look is
     not a longer one). Against part 4: `balance` with its help (a
     quarter of body weight on the hips) holds 1400, with steps 1800, at
     no cost.
   - What it costs, a 4 s take (0.2 s without): at the defaults 63 104
     steps tried for 960 taken, 12.5 s on one core, 7.1 s on two, 4.0 s
     on four; 1 s ahead, 7 s on four. So the search buys unhelped
     balance up to what the helped behaviour holds without steps, at
     about 20 times the bake on four cores. To decide: whether takes
     want the unhelped look enough to pay for it.
   - Left, and each is more numbers in the plan, so more tries a
     decision, so a longer look ahead to tell them apart (the quarter
     second above shows the price of two more): a step is the `step`
     behaviour's (a foot carried, not a leg swung), one at a time, and
     only toward where it is falling; an arm does not reach for
     support; the plan is twelve numbers, not a pose. In a try a
     crowd's agents walk on straight at the speed they had (to steer in
     it every try would copy and step the crowd: 16 crowds ten times a
     second). These are what a trained policy is for, which is the line
     above: needing what is not here.
7. **Crowds: done, as layers or as instances with impostors.**
   `simulation.crowds`. Once a frame every agent steers: toward its goal
   at its own pace, off agents within three radii (found by a grid), off
   and round the nearest point of the level box of any body that stands
   in its height. Its clip runs by the distance walked.
   - **A way round** (`plan`, true): when the straight line to its goal
     is shut, an agent takes the shortest open way (A* over the level in
     squares of 1.5 radii or more, 192 across at most; shut within a
     radius of what stands still) and heads for the farthest of its
     corners it can see. The ground is made again each frame, a way
     looked for again each second or when its corners are out of sight.
   - **Stepping aside** (`dodge`, 0.6 s): a body that, flying as it
     flies, would be on an agent within that time at a speed that would
     knock it down is stepped out of the line of, at up to 2.2 times the
     agent's pace (a `dodge` event). One too fast to clear still knocks
     it down. In the world an agent is a
   moved capsule; one struck at `knock` px/s or more goes down as a limp
   ragdoll placed where the clip had its bones.
   - **As layers** (`agents`): each agent is a comp with a character
     imported for it; the bake keeps a track per agent (`compile` shows
     a comp's children at the bake's clip time).
   - **As instances** (`character`, with `starts` or `count` and `area`,
     `goals` or `march`): one character, imported once and not drawn
     itself. The bake keeps four numbers an agent a frame (x, z, turn,
     the clip's moment) in a `HerdTrack` after the tracks. `compile`
     poses the character at up to 128 moments of its cycle once
     (`compile/crowd.rs`), and an agent is those meshes with its own
     world matrix and the pose shared (one `Arc<SkinPose>` a moment: the
     CPU skins it once, the mesh pool holds one mesh a moment, however
     many agents). Agents knocked down take a ragdoll from a pool
     (`ragdoll.max`, 24) whose bones are tracks `<crowd>#<n>/<bone>`;
     past the pool they stop where they stand.
   - **Impostors**: all but the nearest `full` (150), and all past
     `near`, are image layers on cards across the line of sight. Their
     pictures are in the frame (`FrameState::pictures`: layers, a camera
     far off along the way the agent is seen, the scene's lights); the
     renderer draws each once on the CPU and keeps it in the image pool
     (`Renderer::draw_pictures`). A picture is one of 8 moments, 16
     turns, 16 directions round and 7 up and down, sized to the
     character's box as seen from there (160 px corner to corner). It is
     named by those, so a frame is the same whenever it is drawn.
   - Tested: six agents reach their goals round a pillar and never
     closer than 40 px to each other; the clip turns one cycle a stride;
     a thrown ball knocks one down, whose bones are baked from that
     sample and end on the floor; six agents round a wall
     700 px long across their way, by both ends, none in it, where
     without a plan the wall holds them, and in a pen with no way out
     they stay; an agent a ball is thrown at from 900 px is 56 px and
     more off its line when it passes and walks back, is knocked down by
     one at 9000 px/s, and does not flinch at one going over or wide.
     Instances walk bit for bit as the same
     agents as layers do, are drawn where the bake has them at their own
     moment, are meshes for the nearest `full` and image layers with
     their pictures for the rest; one struck takes the pool's first
     ragdoll, its bones turning as the clip had them (a leg swung 120°/s
     in the clip leaves at 90 to 180°/s against its hips, the still leg
     under 25) and, with `ragdoll.strength`, holding the pose it fell in
     (its spine bends less than half what it does limp); a try ahead
     moves the agents' capsules where their speed takes them and leaves
     the take's alone; the bake survives its file; seven bakes are pinned.
     Rendered: 24 agents as 4 meshes and 20 pictures cover the same
     pixels as 24 meshes (within 12 %, under a third differing at all).
     `examples/sim/crowd.py` (12, 100) and `examples/sim/herd.py` (100,
     1000, 5000) were looked at.
   - Measured (four cores, CPU renderer, 1280×720, the test character:
     16 bones, four skinned meshes, 624 points; an 8 s take):

     | agents | as | bake | bake file | a second of export |
     |---|---|---|---|---|
     | 100 | layers (1700 comps; 48 s to import) | 0.75 s | | 29.1 s |
     | 1 | instances | | | 11.8 s (the field alone) |
     | 100 | instances, all meshes | 1.3 s | 0.8 MB | 24.9 s |
     | 1000 | instances, all meshes | 1.3 s | 4.0 MB | 65.7 s |
     | 1000 | 150 meshes, 850 pictures | 1.3 s | 4.0 MB | 39.9 s |
     | 5000 | 150 meshes, 4850 pictures | 6.5 s | 19.6 MB | 56.7 s |

     (The bakes with instances include a pool of 12 ragdolls, which is
     most of the 1.3 s. Those were before agents looked for what is
     thrown and for a way: 5000 now bake in 7.9 s, in 11.9 s with a wall
     a third of the field long across their way.) A frame of 1000: 24 ms to evaluate, 40 ms of
     display lists, 0.8 s to rasterise; the pictures a frame first needs
     take 1.4 to 2.4 s once. On this character a
     picture is not much cheaper than its 624 points on the CPU (a card
     is a blended image layer): the gain is two fifths at 1000, and
     5000 are drawn at all. Not measured: the GPU renderer and the path
     tracer (no GPU here), and a character of real size, where a picture
     should gain far more.
   - No pop at the switch: the farthest of the agents drawn as the
     character (an eighth of `full`, twelve at most) and those in the
     last tenth of `near` are drawn as the character, part see-through,
     over their picture, more of the character the nearer; a crowd
     whose `full` is under eight is as it was. Tested (states: with
     `near` just past the six, each is the character between none and
     all of it over its own picture; nearer all character, farther all
     picture). Looked at on the CPU renderer with 1000 (`herd.py`).
   - A picture's lights are stepped: it is lit by the scene's lights to
     the nearest step (a direction to under four degrees, a colour, the
     sky and the exposure to a sixteenth of a stop, a lamp's place to a
     sixteenth of how far off it is), and its key is of those. So a
     light that drifts makes a new picture a step, not a frame, and a
     frame's pictures are still the same whenever it is asked for.
     Tested: six pictures under a sun that drifts eight degrees and
     brightens a tenth in a second are made 30 times over its 150
     showings (every showing before: 150); under a still light 5; a sun
     that swings a quarter round in a second still makes one nearly
     every frame (120).
   - Measured again (four cores, CPU renderer, 1280×720, with the guard
     and the band): 1000 agents, 150 of them meshes, bake 1.1 s, a
     second of export 33.9 s (39.9 s before).
   - Left, needing what is not here: pictures cast and take no shadows
     from the scene, and are lit again (by steps now) when lights
     change. Both are the renderers' to mend, in the rasteriser's
     shader, its shadow maps and the path tracer (cards drawn into the
     shadow maps with their pictures' coverage; a picture's normals
     kept and lit in the shader; the tracer sees a card, not a body),
     which are another session's files this round and whose GPU half
     wants a card to look at. Not measured for the same want: the GPU
     renderer and the path tracer on crowds, and a character of real
     size (none here, and no licence read for one).
   - A guard (`crowds[].guard`: `pose`, a moment of the scene at which
     the character is keyed in its second pose; `ahead`, seconds; and
     `bones`, the bones the pose takes with all that hang from them,
     the rest walking on). An agent whose knocking down is on its way
     within `ahead` seconds has its guard up in a sixth of a second,
     holds it half a second after the last threat and lowers it over
     half a second. The bake keeps how far each agent's is up per
     sample (after the herds, only for crowds that have one: other
     bakes are the bytes they were). `compile` blends the pose over the
     walk's moment joint by joint, in quarters, and shares each blend
     among the agents in it; a far agent's picture is of the guard up
     when its is half up or more. Instances only (a crowd of layers
     says why not). Tested: the agent a ball is thrown at has its guard
     up before the ball arrives and down again after, the others never;
     it walks the same samples as without a guard; drawn, its arm is a
     quarter round with the guard up, between on the way, and what is
     not under the pose's bones is as it walks; pinned.
     `herd.py` with `GUARD=1` (36 agents, arms keyed up at 7.5 s) was
     looked at: the agents by the ball's line have their arms up as it
     comes and lower them after.
   - Left, not done: the guard is one pose, raised at what would knock
     an agent down: no duck by where the thing comes from, no look; one
     knocked down with its guard up falls from its walk's pose; none
     knocked down gets up (a ragdoll of the pool has no brain), and past
     the pool one stands frozen; it does not get out of the way of other
     agents' falls. A way is planned round what stands still only (its
     level box: a wall at an angle shuts its whole box), by each agent
     alone (no lanes, no waiting for a gap: a doorway jams), and not
     round a body in motion. `check` evaluates every agent at every
     sample it takes, which is slow at thousands.

## Directing an agent from the editor

Built: to say in the editor what is wanted ("add five seconds of … here",
"this one should look like …", "something is wrong with this one"), see
the agent at work, and scrub what it made. "Generated" here is motion the
agent builds with the engine's tools; a clip from a video model would be
something else to connect.

What was run: the unit tests, and two windows under Xvfb with Mesa's
software Vulkan (`cargo test --release -p lahza motion_prompt -- --ignored`,
screenshots looked at; with `LAHZA_MOTION_SOFTWARE_GPU=1` the preview's
first frame takes tens of seconds and the tests wait for it), with a runner of our own in Claude Code's place
that makes real edits through a `Toolbox`. Then real requests to Claude
Code: two with the runner's own command line, and one from the prompt
bar itself (see "A real request" below). Not seen: its wording when
signed out.

1. **A prompt box** (`motion_prompt_ui.rs`, the field from
   `text_field.rs`): under the preview on a motion project and on a
   recording's Motion tab.
2. **Starting an agent** (`motion_agent.rs`): `Runner` is the one small
   interface (available, start, and a `Running` to stop), with two
   runners behind it and a choice between them (`Chosen`; the second,
   `ApiLoop`, is described after this list). `ClaudeCode`
   is the first: `claude -p` with stream-json in and out, `lahza mcp` as its
   only server and no tools of its own, its lines read as they come.
   Tested with a stub program that writes the same stream. Not installed
   (the program is missing) and not signed in (`claude auth status`, or
   what a failed request says) are said in the bar and in the AI tab.
3. **What goes with the words** (`compose`): the project, the selected
   layers and their kinds, the marked span, the playhead and its frame,
   the scene, and the frame showing as a PNG. A follow-up continues the
   conversation (`--resume`); a chip that can be removed says so.
4. **The agent at work**: each step in the bar, in the AI tab and as a
   ghost clip; what it said at the end; Stop ends the child and the
   server it started (its process group).
5. **Undo for a whole request**: `HistoryEntry.request` (the server takes
   it from `LAHZA_REQUEST`); `history` has `undo_request`, and
   `by_request` makes a request one step of `undo`, which is what Ctrl+Z
   in the editor does. Redo puts it all back.
6. **More than one thing selected**: Ctrl- or Shift-click adds rows to
   what a prompt is about (the editor still holds one layer, the one the
   inspector shows); a span is marked on a strip over the ruler, or with
   Shift on a recording's ruler.

Settled:
- **Editing while the agent works: guarded writes.** `io::lock` (`flock`
  on `<project>/.lock`) is taken by `edit_apply`, by undo and redo, and
  by the writers of markers, shaders and the narration
  (`every_writer_of_a_project_waits_for_its_lock`);
  under it the writer loads what the other wrote, then writes. Two
  writers of 25 edits each lose none and take no number twice
  (`two_writers_do_not_lose_edits`). The editor stays usable.
- **How long it takes**: three small requests took 28 s, 25 s and 18 s
  (see "A real request" below); a scene of five seconds was
  not asked for. The bar and the ghost clip say the step, the seconds so
  far, and the steps and edits made. What is left is not known to the
  runner and is not said.

The second runner (`ApiLoop`, `motion_agent_api.rs`) needs only an API
key: our own loop on the Claude Messages API (raw HTTP through `ureq`,
the one new dependency; there is no official Rust SDK) with the `Toolbox`
in process: its own tool definitions, plus `read_doc` for the guide,
schema and catalog an MCP client reads as resources. `claude-opus-5-5`
(or `LAHZA_AGENT_MODEL`), `fallbacks: "default"`, the same `compose`d
words and picture and the same steps as the Claude Code runner.
- The answer is read as it streams: what the model says and each tool it
  asks for show as they come; the tools run once the answer is whole, and
  every result of a turn goes back in one message. A refusal, a cut-off
  answer, a stream that breaks, and a tool input that is not JSON run
  nothing half written. No connection, 429, 5xx and an overloaded stream
  are tried three times. Stop is looked at between events and calls.
- Its conversations are kept in `<config>/agent-conversations/`, so a
  follow-up continues after a restart, as Claude Code's do.
- What a request took shows in the AI tab: tokens in and out for this
  runner (the API gives no price), dollars for Claude Code (its stream
  gives them).
- The key is `ANTHROPIC_API_KEY`, or typed in the AI tab into a masked
  field (a star a byte, nothing to the clipboard) and kept in
  `<config>/anthropic-api-key` with mode 0600. It is sent in the
  `x-api-key` header and nowhere else, and never shown again: only where
  it is from and its last four characters (tests look for it in the
  project's files, the kept conversations and what is reported; the
  field was seen under Xvfb).
- The runner is a choice in the AI tab (Auto, Claude Code, API key),
  kept with the settings: Auto is Claude Code where installed and signed
  in, else the API loop where a key is set, and the bar names the one in
  use.
- Tested against a local stub server that streams a tool-use
  conversation, a refusal, a cut-off answer, a broken stream, an error
  in the stream, bad tool JSON, 401/400/404/429/500/529, nothing
  listening, a stop in the middle, and a second run of the app.

**A real request** (Claude Code 2.1.329, signed in, a small project in a
container; the server is the app's own `mcp`):
- With the runner's own command line, outside the window (`compose`'s
  text and the frame's PNG given by hand): "make this one red" with the
  title selected took 28 s, nine turns, $0.43; "make this one a circle
  and green" with the box selected 25 s, nine turns, $0.17. The first
  spent two calls on "no project is open" (`compose` now says to open
  it first); both wasted a `set` without `target`, and one a shape that
  does not exist ("ellipse").
- From the prompt bar (`a_real_request_is_sent_from_the_bar`, an ignored
  window test that costs money and so also needs `LAHZA_REAL_AGENT=1`;
  under Xvfb): the title selected,
  "make this one red" typed, Enter. 18 s, five turns, four calls
  (`project_open`, `timeline_get` with `full`, `edit_apply` with
  `set title.color`, `snapshot`), $0.139, no call failed and no
  permission was refused. Each step showed in the bar and on the ghost
  clip as it came, the proposal row came back with Keep, Undo, Again
  and Hold to compare, the AI tab listed the steps and the cost, the
  history has one entry with the request's id, and one Undo put the
  title back to white (screenshots looked at). It was sent a second
  time by mistake (a test filter, `prompt_bar`, matched its old name:
  hence the new name and the variable) and passed again; that run's
  turns and cost were not kept.
- Before that run the system prompt (`SYSTEM`, both runners) was given
  the exact form of `set`, `key`, `add` and `remove`, the shapes there
  are and where a colour lives, and told to read the layer
  (`timeline_get` with `full`) before changing it. The run made no
  wrong guess; one run does not prove it never will. Claude Code is
  given no tool to read the server's resources (`--tools ""`), so the
  schema is not something it can read: the forms are in the prompt.
- `LAHZA_AGENT_LOG=<file>` keeps Claude Code's stream (turns, refusals,
  the tools it had). `LAHZA_MOTION_SOFTWARE_GPU`, when the app has it,
  is passed to the server so its snapshots draw where there is no GPU.
- Whether the picture sent with the words was read was not shown (no
  request needed it).

Left, each with what it needs:
- **A request with the API runner.** It needs a key (none was set in any
  container, and none was looked for): its version header, model id and
  event shapes are from Anthropic's reference as read in November 2026,
  not from an answer. With `ANTHROPIC_API_KEY` set: choose "API key" in
  the AI tab and send one request.
- **A real desktop.** Requests kept in `requests.json` read back after a
  real restart (keeping and reading them is tested; a restart of the
  app is not); the whole of "How it should be used" with a mouse in a
  hand. Checked since under Xvfb
  (`a_pointer_on_the_picker_a_bake_stopped_and_the_window_closed`):
  closing the window while a request runs lets go of its runner (GPUI
  drops the studio; a real runner's `Drop` ends its child), and the X
  server's own pointer (xdotool) presses, drags and lets go of the
  colour picker and drags a span on the strip over the ruler.
- **A piece made on a recording, by a real agent.** With no piece open
  the words ask for a new one on the span; a stand-in that makes the
  real tool calls (`project_create` with the recording, `edit_apply`,
  `recording_place`) was followed under Xvfb
  (`a_piece_made_on_a_recording_is_seen_compared_and_undone`): the ghost
  clip shows on the Motion lane over the span (or five seconds from the
  playhead) while it works, the piece comes back on the timeline within
  half a second of the request ending, Hold to compare shows the video
  without the piece (it is left out of the preview; nothing is
  written), and Undo takes it off the video (its folder stays on disk;
  there is no redo for that). Not seen: the same with Claude Code
  itself (one more paid request, on the owner's word).
- **Undo after later edits** to the same things fails with the engine's
  message ("the project changed in a conflicting way"). Left as it is:
  merging a person's later edits into an undo is a decision about what
  undo means, for the owner.

## The motion editor: drags that stall, and how it should be used

### Drags that stall

Changing a selected layer's values was slow: the bars stuck, as if a
render had begun. Four causes were read in the code and timed apart from
the app; all four are fixed. Run under Xvfb with Mesa's software Vulkan
only (`editor_window_edits_and_reloads`): which of them was the one felt,
and whether the bars now move freely, is not confirmed on a real card.

- **Every drag draws the proxy.** A value dragged in the inspector, a
  layer moved and a key retimed now draw at half size with no motion
  blur, as a piece dragged on the timeline did. Both places ask one
  question: `motion_piece_frames` (motion_piece_ui.rs) whether a piece or
  anything of the open piece is dragged, the editor's `request_frame`
  `MotionEditor::dragging` (its canvas drag, or an unsaved preview
  showing). Left to look at on a real card: whether full frames on the
  GPU that also draws the window held the bars (the frames are on a
  worker thread, so the UI thread alone should not stall).
- **A tick changes one layer.** `preview` (motion_editor.rs) no longer
  turns the project into JSON: `edit::apply_to_layer` passes only the
  dragged layer through the same op code, into a copy of the project made
  once a drag (`preview_project`); `apply` is kept for the drop and for
  anything that is not `set`, `key` or `anchor` on one layer. The result
  equals `apply`'s (`a_drag_on_one_layer_equals_apply`). Timed in a cloud
  container (`cargo test --release -p lahza-motion-core drag_tick_timing
  -- --ignored --nocapture`):

  | layers | `apply` (before) | one layer (now) | compile (still every tick) |
  |---|---|---|---|
  | 30 | 0.20 ms | 0.012 ms | 0.04 ms |
  | 480 | 3.4 ms | 0.013 ms | 0.66 ms |
  | 1 200 | 22 ms | 0.014 ms | 1.7 ms |

  **A tick compiles one scene.** `Compiled::with_layer_changed`
  (compile.rs) compiles only the dragged layer's scene and shares the
  others (`scenes` is now `Vec<Arc<CScene>>`). It is right by
  construction rather than by a guess: a scene's compile
  (`Compiler::scene`) reads, outside itself, only other elements'
  windows, so the result is returned only when every window in the
  changed scene is what it was, and nothing else is shared: no
  components (expanded project-wide), no Slang (one library), the same
  settings, time base, scene count and project-wide errors. Anything
  else is `None` and the editor compiles in full, as before. Tested:
  equal to `Compiled::new` in every list and every frame for seven kinds
  of local edit, with the other scenes the same `Arc`s
  (`one_scene_compiled_again_equals_the_whole_compile`), and `None` for
  each case that could reach outside
  (`what_could_reach_another_scene_takes_the_whole_compile`). Timed
  (`drag_tick_timing`, a cloud container, milliseconds a tick):

  | layers | scenes, and where the drag is | whole compile | one scene |
  |---|---|---|---|
  | 480 | 1 | 1.0 | 1.0 (not tried: the whole compile) |
  | 480 | 12 equal | 1.4 | 0.16 |
  | 480 | 12, the first holds 3/4: in the first | 0.9 | 0.6 |
  | 480 | 12, the first holds 3/4: in a small one | 1.2 | 0.15 |
  | 1 200 | 1 | 2.1 | 2.1 (not tried) |
  | 1 200 | 12 equal | 2.2 | 0.41 |
  | 1 200 | 12, the first holds 3/4: in the first | 2.6 | 2.0 |
  | 1 200 | 12, the first holds 3/4: in a small one | 2.2 | 0.27 |

  So: five to eight times less where the layers are spread over scenes,
  about a fifth less in a scene that holds most of them, nothing in a
  project of one scene (where it is not tried: trying cost a tenth
  more). What a tick still does for every layer: the windows' table and
  the roles (`check::intents`). Not timed: an imported model of many
  parts. The numbers move by a third between runs on this machine.
- **The drop no longer reads the history.** `next_history_number` takes
  the highest file name: 0.6 ms at 2 000 entries, where reading them all
  takes 14 ms (`history_numbers_come_from_file_names -- --nocapture`).
  Undo and redo still read every entry.
- **The inspector asks for each value once**, and the layer is turned
  into JSON once per change, not twice per value per draw
  (`layer_value` keeps it). During a drag the number shown is the dragged
  one. X and Y of one property are still two evaluations.

### How it should be used

The rule: select, type, Enter; everything else is by choice. The fewest
clicks for the common things, the same in the motion editor and on a
recording's timeline. Built as below and seen in screenshots under Xvfb
(see "Directing an agent from the editor" for what was run); not used
with a mouse on a real desktop.

- **One prompt bar, the same everywhere**, under the preview, with chips
  that can be removed ("Layer: title", "0.5 s to 2.0 s", "Continues the
  conversation"). Not a modal.
- **Typing with something selected begins a prompt.** On a motion
  project any printable key does, selected or not; on a recording only
  with a layer selected, because letters are shortcuts there. Space
  still plays.
- **A span is marked** by one gesture in both places: a drag along the
  strip at the top of the ruler (over it on a motion project, its top
  8 px on a recording; the rest of the ruler still scrubs). A click
  clears it; Shift-drag anywhere on the ruler does the same. On a motion
  project the strip was dragged by the X server's pointer under Xvfb
  (xdotool: 4 s to 10 s marked); on a recording it is seen in a
  screenshot and its drag was driven through the code.
- **The inspector has two tabs**: Properties, and AI (what was asked
  about this layer, each step, what it said, keep / undo / again). With
  nothing selected the AI list shows every request.
- **The agent's work is seen and does not block**: the ghost clip over
  the span (the whole length without one) with the step it is at; the
  editor edits meanwhile.
- **What comes back is a proposal**: Keep, Undo, Again, and Hold to
  compare (the button, or `\` held). Compare applies the request's
  inverses in the preview only; nothing is written.
- **A drag never waits**: see "Drags that stall". This holds with the
  preview set to Traced or Raster + sky too: a drag's frames are the
  raster proxy (`motion_preview::ask`), and the traced picture comes back
  warmed up on release.
- **A simulation that needs baking says so.** Beside the prompt bar: a
  pill when a scene's bodies have no bake or changed since it
  ("Simulation not baked · Bake", "Simulation changed · Bake again"; the
  tooltip names the scenes), found off the UI thread after each load
  (`sim_needs`: `check`'s `simulation` issues). One click bakes through
  the `sim_bake` tool on a thread of its own and reloads; a body dragged
  afterwards brings the pill back. A scene that cannot be baked shows in
  red with the engine's reason. Seen under Xvfb
  (`a_simulation_is_baked_from_the_bar_and_goes_stale_when_a_body_moves`).
  While it bakes the pill says the scene, the share done and the
  seconds, with a Stop beside it: `lahza_motion_sim::bake_scene_watched`
  tells a `Watch` the share before each frame and stops when it answers
  no (`a_bake_says_its_share_and_stops_when_told`; `Toolbox::bake_watch`
  passes it through `sim_bake`). A stopped bake writes nothing: the
  simulation stays as it was. Seen under Xvfb with 150 boxes over
  twenty seconds: the pill at 4% and its Stop, and the bake ended
  0.3 s after it began with no file written.
- **By hand stays cheap**: the inspector edits a text layer's words (shown
  as typed, saved when the field is left), its font (the bundled ones and
  the font files in the project's `assets/`), colour, size and tracking;
  each effect's numbers, two-part values (a shadow's offset as X and Y)
  and colours; and adds or removes every effect but `lut`, a gradient
  map with a colour for each stop among them. A text also has its
  alignment, line height and an outline (on or off, its width and
  colour) and a rim light (`edge`: on or off, its width, angle and
  colour). A colour is picked from ten swatches, typed as #rrggbb, or
  dragged in a picker that opens with the typed field: a square of
  saturation and value over a strip of hues (two gradients and six, no
  image to release); the drag shows in the preview as a value's does
  and is one edit on release, and a colour's alpha is kept. A gradient
  map's stops are moved (each between its neighbours), added (in the
  widest gap, the colour between) and taken out (two stay). `lut` is
  added with its table: one of the project's `.cube` files, or
  "+ LUT file…" (the desktop's file chooser; the table is copied into
  `assets/`), and its table can be changed for another of the
  project's. Seen under Xvfb
  (`by_hand_a_rim_light_a_lut_gradient_stops_and_the_picker`). Left for
  a desktop: the file chooser itself (a portal dialog: `add_motion_lut`
  behind it was run with a path). The picker was dragged by the X
  server's pointer under Xvfb (xdotool), not by a hand.

## Slang

Nothing here was run since: slangc cannot be had in a cloud session (its
releases are on a host the session's network does not allow; a build of
it from the slangpy wheel's libslang was refused). Both points need the
machine with slangc.

- **To compile once with slangc.** These were written where slangc could
  not be run, and are checked only as text (`cargo test -p
  lahza-motion-core slang`): shader layers pass their params in a
  `<function>_params` struct where the module declares one (by field
  name; one by one in name order where it does not, as before);
  generators and deformers are passed the struct whenever the module
  declares it, params or not; the compiler's errors in Lahza's generated
  calls are shown as the call. The generated Slang follows what the
  geometry generators already compiled (`T p = {}; p.field = …;`), in a
  compute kernel too for shader layers. `cargo test -p lahza-motion-core
  slang` passes with slangc (11 tests, on the machine with the RTX 3060),
  and the Slang reel renders without warnings; a project whose shader
  layer gives params to a `<function>_params` struct has not been run.
- **Slang geometry on the GPU: what is left.** A generator whose layer
  gives `bounds` is generated by the GPU in its mesh pool, a deformer
  moves its mesh there (`Source::Deformed`: the mesh before the move goes
  up once, `Kernel::deformer` moves its points each frame), and the path
  tracer takes both from the pool (`TraceMesh::Pool`, pt_pool.wgsl)
  (`mesh_gen.rs`, docs/slang/geometry.md). Run with slangc on the RTX
  3060: `the_gpu_runs_the_projects_generators` and
  `the_gpu_runs_the_projects_deformers` pass, and the Slang reel's
  landscape (traced) and blob (rasterised and traced) match the CPU's
  meshes (PSNR 51 to 53 dB; mesh building 59 ms a frame became 9 on the
  blob). Since:
  - A generated mesh with a deformer is generated, readied (copies of a
    point welded on the GPU, meshweld.wgsl) and moved in the pool, where
    the layer gives `bounds` and the deformer does not subdivide
    (`MeshParams::gpu_deformed_generator`, `Source::DeformedGenerated`).
  - A mesh the CPU readies and the GPU moves has its moved points' own
    box (`slang_mesh::deform_extent`: the CPU runs the deformer for it),
    and is refused whole where a point moves to a value that is not a
    number. That run costs: the blob's mesh building is 24 ms a frame,
    not 9 (72 on the CPU).
  `the_gpu_moves_the_meshes_it_generates` covers both.
  - A deformer that subdivides a generated mesh has it subdivided on
    the GPU (meshsubdivide.wgsl: an edge two triangles share gets one
    midpoint by a hash table of edges; the rows keep one for each
    triangle's edge, so the mesh has more points than the CPU's,
    `slang_mesh::gpu_subdivided`).
  - A mesh the GPU moves is refused whole there where a point moves to a
    value that is not a number (`deform_triangles`), and the render says
    so once the frame is back.
  Not on the GPU, and why:
  - A generator with a `displace` is generated on the CPU. Displacement
    subdivides until the mesh's edges are short enough, so how many
    points the mesh has follows from points only the GPU would have,
    and the CPU must give the mesh its rows before the GPU makes it.
    (It would take the mesh's size from the frame before, or rows for
    the most it may be.)
  - A generated mesh the GPU also moves has its layer's `bounds` padded
    by `reach`, not its moved points' own box: the CPU places and culls
    the mesh, and frames its shadow, before the GPU has made the points.

## Tests on a hardware GPU

Run on the RTX 3060 (`cargo test -p lahza-motion-render -- --test-threads=1`).

- In parallel the test binary segfaults most runs, in the Vulkan
  loader's `vkSetDebugUtilsObjectName` (devices and pipelines being made
  on several threads), with or without the changes of this branch's last
  commits. One thread runs through. Since: objects are named only where
  a tool reads the names (blade's `set_object_name`: with validation or
  capture on, or `LAHZA_VK_NAMES` set), so a run without them does not go
  through that call at all. Not run on the card (to do: the tests there
  without `--test-threads=1`); on Mesa's software Vulkan the crash does
  not show, and four devices at once run a 15 GB container out of memory.
  The app could meet the same crash (a renderer per preview, each with
  its device): the change covers it too.
- The hardware visibility pass drew wrong pictures there, which no test
  on Mesa's software Vulkan shows (it has no 8x, and the comparisons with
  the CPU run without the pass). Fixed:
  - A mesh drawn in a picture of its own (a glow) is also a depth-only
    occluder in the list's other batches; both landed on one page at the
    same depth and the occluder, drawn later, kept the samples: the mesh
    was not drawn at all, nor the floor under it. Occluders are now drawn
    first (`VisPrim`).
  - Cards on one plane (text on a panel) fought for samples; each card is
    now drawn nearer than the one before it by `TIE`.
  - Cards and wires were tested at the pixel centre, not at each sample:
    the fragment shader now gives the samples they stand on.
  `cpu.lahza` at 3.0, 9.0, 10.4 and 20.0 s (540 wide) is the CPU
  renderer's frame but for 179 to 3830 pixels of the 518 400, nearly all
  by under 16 levels (reflections on the rocks).
- Six tests still fail. Four are the GPU's frame against the CPU's by a
  little more than they allow, by a few levels: `gpu_matches_cpu_meshes`
  (215 pixels, 103 allowed), `gpu_matches_cpu_surface_detail` (1089, 76:
  the bumps' shading), `gpu_matches_cpu_area_lights` (118, 23: a shadow's
  edge, from the GPU's wider shadow map frame),
  `defocused_instancer_copies_keep_their_depth` (3, 0). Two assumed no Open
  Image Denoise and Mesa's software Vulkan
  (`pt::physics_tests::{frames_in_flight_are_the_frames_drawn_one_at_a_time,
  the_denoiser_works_on_the_tracers_memory_where_it_can}`). Changed, not
  run on the card: the first now names its denoiser either way (the
  built-in filter through `Tracer::built_in_denoiser` where it used
  whatever was installed), the second is skipped off software Vulkan
  (its stand-in denoiser reads the shared memory as files, which a
  card's driver is taken not to allow: from reading, to be confirmed
  there). To run there: those two by name.
  Since, for the four: `LAHZA_MOTION_VIS_REPORT=1` makes every
  GPU-against-CPU comparison say where the two differ instead of failing
  (pixels over 3 levels, how many of them away from an edge and by how
  much; `LAHZA_MOTION_DUMP=dir` keeps the frames), through the visibility
  pass: the card's own 8x, or 4x on Mesa's software Vulkan, where the
  four show the same kind of difference. Away from edges there, at 4x:
  meshes 172 and 164 pixels (worst 55), surface detail 474 (worst 30),
  area lights 13 (worst 5), the instancer's copies 0; of tests that pass
  on the card, shadows and occluders 3 and principled materials 83. In
  the surface-detail frame they lie along the seams between triangles
  (a pixel two triangles share is shaded for each at the middle of the
  samples it won, and the pass's samples fall otherwise than the CPU's:
  bumps make that show) and as spots on the face with a normal map (not
  looked into: the pooled vertex has two rows where the CPU's has four,
  so some of what shading reads is worked out on the GPU). Not fixed. To
  do on the card: run the four with the report, see whether its 8x leaves
  the seams, and look at the normal-mapped face first.
  Since, both looked into on software Vulkan:
  - *The spots on the normal-mapped face: found and fixed.* Not the
    pooled vertex's two rows (what the GPU works out of them is the same
    over a flat face). They needed a light that casts shadows and the
    normal map together (the test's scene with one directional light:
    375 pixels away from edges with shadows, 16 without; 21 after the
    fix): the shadow lookup was moved along the normal the map had bent,
    turned over where that pointed from the light, so into the face,
    which then shadowed itself in a patch a period of the map, on the
    CPU and on the GPU alike but by other amounts (the GPU's map is
    drawn by the GPU through the visibility pass, in another frame, the
    CPU's by the CPU: where a lookup is on the threshold the two fall
    either way). Shadow maps are now asked along the surface's own
    normal, before its detail (motion.wgsl `shade_tri`'s `ns`, the same
    in mesh3d.rs): a real fault in the picture, not only in the
    comparison. `a_normal_mapped_face_does_not_shadow_itself`: the
    box's front face under a light that casts shadows against the same
    light casting none, 1016 px of 7040 darker by more than 2 levels (up
    to 15) before, none after, on the CPU and through the visibility
    pass. The report for `gpu_matches_cpu_surface_detail` at 4x: 4376
    pixels over 3 levels before, 3285 after; away from edges 474 and 227
    (the box 1811 and 721 with its edges). Every bump and normal map
    under a shadow-casting light changes a little with this (the ball's
    and the can's bumps too): the patches were wrong, so no test's
    numbers were kept for them.
  - *The seams, at the card's own 8x: they should nearly all go.* From
    the code: the pass's eight samples are the standard pattern's, in
    its order, and `mesh3d::SAMPLES` is that pattern, so at 8x a sample
    is the same point in both and goes to the same triangle; a triangle
    is then shaded at the middle of the same samples. (At 4x the eight
    are read from four, `s & 3`, at other points: nearly every pixel on
    a seam differs, which is what the report here shows.) What is left
    at 8x is a sample within a rasteriser's grid of an edge: hardware
    has a triangle's corners on 1 / 256 of a pixel, the CPU as they are.
    Measured on the CPU alone (`mesh_tests::measure_seams`, `--ignored`:
    its frame against its frame with the corners on that grid): 16
    pixels of that scene over 3 levels (406 on 1 / 16). Shading every
    triangle of a pixel at the pixel's middle instead does not help (17):
    not done. To make those last agree the CPU would have to round its
    corners as the device does (its `subPixelPrecisionBits` and its
    rounding): not done, 16 pixels are within what the test allows (76).
    So the card's 1089 were not seams. Most likely they were the patches
    above, on the ball's, the box's and the can's bumps (on the box
    alone the fix took about 1090 pixels out of the 4x report), since
    the card draws its own shadow maps where the test's usual run on
    software Vulkan gives the GPU the CPU's: to be counted again there.
  - *The other two, `gpu_matches_cpu_meshes` (215 on the card, 103
    allowed) and `defocused_instancer_copies_keep_their_depth` (3, none
    allowed).* The grid alone, measured on the CPU
    (`mesh_tests::measure_snapping_in_the_cards_other_tests`,
    `--ignored`): the instancer's copies 3 pixels over 3 levels, the
    card's own count, so that test fails there by the rasteriser's grid
    and nothing else; meshes 32 and 40 pixels at its two moments, of the
    card's 215. For the rest of those: at 4x here the report has 158 and
    153 pixels away from edges, all of them looked at and all on faint
    edges the report does not take for edges (the star's dark side, the
    glass cone), none on a face; and the GPU's directional shadow map
    was framed 13 and 15 units wider than the CPU's on two sides in that
    scene, now the same (see "Shadow maps on the GPU"), which moves a
    shadow's edge by a part of a texel. So what is left of the 215
    should be within the 103; not known until run there.
    To run on the RTX 3060: `cargo test --release -p lahza-motion-render
    -- --test-threads=1 gpu_matches_cpu defocused_instancer`, and for
    any that still fails the same with `LAHZA_MOTION_VIS_REPORT=1
    LAHZA_MOTION_DUMP=<dir> -- --nocapture`. If only the instancer's 3
    remain, allow them: `assert_gpu_matches_cpu_within(&p, &[0.0],
    0.0001)` in that test (the grid is the device's, not a fault).
