# TODO

## Where things stand

Every section below was worked through in cloud containers (four cores,
no GPU: Mesa's software Vulkan, Xvfb, a stand-in for the agent). Each
section says what was done, what was measured and what is left; this is
the short of it.

Done and run here (pictures looked at; on the merged tree, one thread,
Mesa's software Vulkan: motion-render 186 tests pass and 21 are ignored
(measures), motion-core 135, motion-sim 35, motion-mcp 38 and its
integration tests, the app 122 and its four window tests under Xvfb):
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
  the GPU-against-CPU comparison.

To do on the owner's machine (the RTX 3060, a desktop, Claude Code signed
in), since none of it can be seen from a container:
1. Send one real request from the prompt bar, with each runner (Claude
   Code signed in; an API key alone, which was only run against a stub
   server here): whether the tools run without a permission refusal,
   whether the picture is read, how long a scene takes ("Directing an
   agent from the editor").
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
  volumes in the sky's rays, a third return of its light, a place in the
  app to type an API key, pictures of far agents taking the scene's
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
  118 pixels there are to be counted again. Still as before: meshes the
  GPU generates (the CPU has no points: their boxes, and the boxes'
  middle for the aim as soon as one caster has none), and a closed
  caster around the light is framed with its faces turned from the light
  (the CPU leaves those out).
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
  claimed for a card: to run there. Not done: traced frames' groups (a
  traced group is never the same: its picture is new each sub-frame, and
  where samples share traces nothing is compared at all), and a mesh
  primitive still counts as reading the groups before it, so one
  earlier group that moved has it drawn again.
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
  - Not done: one shadow test a vertex (the film's scenes have one or two
    lights that cast shadows).

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
    shadow over nothing (it is the coverage there) is still four paths'.
    Seen on the way: the usual trace's filter widens a catcher's soft
    shadow a good deal (against the same scene not denoised); not
    changed.
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
  pictures looked at and they differ as they should. Not run: the
  window with Traced chosen, any size a person would use, a real card,
  Ray Reconstruction. Not settled: the pacing (a warm-up of 8 frames
  blocks the worker for as long as they take; at the 24 ms measured on
  the RTX 3060 that is a fifth of a second), the fast mode's own motion
  blur (not asked for: `shutter` stays 0), and whether 8 dropped frames
  is the right reach for the history while playing.
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
  direction; one that ends on a surface adds that surface's light: its
  colour under the lights (shadows by rays) and under the environment
  (its sky, horizon, ground and lobes, or its image's diffuse level). `sky_filter` makes the sixteen one value over the texels on
  the same plane, each place of the tile weighing the same, and smooths
  that once more two texels apart: each texel has every direction but
  each from another point near it, and its value went in steps that
  showed as blotches on large flat things (two rays and one pass: a
  wall's brightness spread 1.3 levels where the traced picture's has
  0.8; now 0.7, and the test holds it there). The group
  holds, a texel, the mean open direction, the open part and the depth,
  and the light that comes back. The raster's shading reads it by depth
  (motion.wgsl `sky_at`): `env_diffuse` along the open direction times
  the open part, ambient lights times it, what is mirrored of the
  environment less by it (Lagarde's fit), and the light that comes back
  added to the diffuse light.
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
    surface's base colour, less what is metal, under the environment
    with its lobes or its image, the ambient lights, and each other
    light from its middle with a shadow by one ray; an area light as a
    point), not the tracer's full material and soft lights as at first.
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
  take it: done**, by a fixed rule, nothing drawn by lot. A ray ends on
  a surface that is at least half opaque (its material's opacity times
  what it does not let through: pt.wgsl `sky_blocks`, the rule the
  light's shadow rays had) and on a card where its picture covers more
  than half; anything less it goes through. So a mesh 0.7 opaque hides
  the sky as a solid one and one 0.3 opaque hides none of it: a step at
  one half, where coverage as a ray's weight would be even (a ray going
  on through each thing it meets: not done, it costs rays).
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
  card lying over it 182 of 255 where it is 212 without the card, and it
  takes the card's blue as the traced picture does; beside a box 0.7
  opaque 195 for 208, beside one 0.3 opaque 208; a box 0.99 opaque is
  lit as a solid one where sky visibility darkens that most (206 and
  206, 224 without). GPU against CPU: no pixel over 3 levels in the
  three. Seen: the card's shadow on the floor against the traced one
  (the traced is grainier at its edge, the same size and tint).
  Left, lit as before and hiding nothing: volumes (they need the
  tracer's marching), wires and shadow catchers (now gone through by
  every ray: before, one that met a catcher first counted as open),
  meshes that blend otherwise than over what is behind them, meshes
  whose coverage a Slang surface works out, opacity from a map (the rule
  reads the material's own), and flat layers under 0.999 opaque (they
  are not cards). The cost of cards in the rays (their pictures read at
  each crossing) was not measured.
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
- **The light's second return** is in, from the same rays and no more
  of them: where the picture shows the surface a ray ended on (the texel
  its end is seen in, at its depth), that texel's own first light back,
  times the mean base colour of what the rays hit, is added (pt.wgsl
  `sky_second`, the same in `sky_cpu.rs`; GPU and CPU still within a
  level). It closes about a third of the gap (4% to 3%, 8% to 6% under
  the table). What is left: surfaces the picture does not show send
  nothing back a second time, the environment at what a ray hits is
  taken whole (not by what that point sees of it), and there is no
  third return. Anything mirrored of other things: not done. The fast
  mode has all of it.
- Not done, with what each needs:
  - *Wires, shadow catchers and volumes hiding the sky and taking it,
    and coverage as a ray's weight* in place of the step at one half
    (above). Volumes need the tracer's marching.
  - *Meshes the GPU generates in its pool* are not in the CPU's rays.
    Their bounds as a stand-in were not added: the CPU's frame does not
    draw such a mesh at all, and a box hiding the sky from the rest
    where nothing is drawn would be wrong another way.

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
`motion-sim`'s tests; the eight pinned since, for getting up, bones
meeting, instances, a way round a wall, stepping aside and the search,
were made on one of them and are to be confirmed on the other). Nothing a bake is made
from goes through the C library now: rapier's maths is software
(`enhanced-determinism`), the behaviours, crowds and search use `libm`,
and everything `compile` works out (layer rotations, curves, cameras)
goes through `core::dm` (`libm`'s sines, powers and logarithms; lengths
by multiply and square root), which left every pin as it was. Whether
the pins hold on a CPU without FMA and on ARM is being run by the
coordinating session. In the order built:

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
   - Left: a box landing flat at speed tips a degree or
     two before settling (rapier's contact solver; more iterations do not
     change it). Ropes are not drawn. Bodies inside instancers are
     refused. Slang-generated meshes cannot be colliders (give a shape).
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
   - Measured: 3.9 ms to skin 100 000 points with four joints each on
     one core. The GPU side is not timed (lavapipe is no measure).
   - Left: four joints a point (glTF's JOINTS_1 is not read); no dual
     quaternions (linear blend: elbows thin at sharp bends); the file's
     rotations become three layer angles keyed linearly, not slerped;
     `model_import` copies one file, so a `.gltf` must embed its buffer
     (or be a `.glb`); a skinned mesh under a Slang deformer or
     displacement is not deformed.
3. **A ragdoll that falls: done.** `simulation.ragdolls`: every bone
   under `root` (the joints' comps) is a body, a capsule along the bone
   toward its child (a given shape where that is wrong: box feet), mass
   shared by volume with no bone lighter than three quarters of what
   hangs from it. Joints are balls with swing and twist limits, or hinges
   (knees, elbows), measured from the pose at `rest`. Until `release` the
   bones follow the clip; they leave it with its pose and speed. Bones of
   one ragdoll do not collide with each other.
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
     chest), through the fall and at rest; pinned. A bone a behaviour is
     placing (an aimed arm, the whole body while it gets up) passes
     through the rest for those steps: arm targets are worked out
     without the body in the way, and stopped short of the head and of
     a hit when their arms met the chest on the way.
   - Left: limits are per bone by hand (no defaults by bone name); a
     limp figure with the default 40° limits falls like a plank,
     slowly. Arms a behaviour aims do not go round the body, they go
     through it.
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
   - Left, and it is the hard part the plan names: balance is not
     footwork, it is a bounded hand on the hips; a step is a foot carried
     by a force, not a leg that swings; getting up is a pose sequence
     under an outside force, not a body that finds its feet (its feet
     stay where they lay and it is drawn over them; it does not step to
     them), and it gets up the one way, front down;
     no behaviour looks where it is going. Feet and hands are found by
     bone name (`foot`, `hand`, `head`).
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
   - Left: no LLM was put in the loop here (the test is the calls one
     would make). A take is judged from events and a sheet; nothing
     scores it against the intent. `snapshot` does not mark where on the
     sheet the events fall. Variation by seed is in the blows and
     releases only, not in how a body reacts.
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
     of each upper arm (ten numbers; a search over every joint would
     need far more tries), and for a third of them, when its weight is
     headed off its feet, a step of the foot behind to where it is
     falling to or up to 1.8 times as far. Scored on falling, weight past
     the feet, tilt, height, effort and a little for a step. The tries
     run on every core; the plans are drawn before any is tried and the
     first of the least cost kept, so the take is the same on one core
     as on four (tested to the byte, pinned).
   - Measured on the test figure, pushes at the chest in px/s, with no
     force on its body (a step's foot is carried by a force, as the
     `step` behaviour's is): motors holding the clip fall at 300. Search
     with a 0.25 s horizon (too short to see a step land) holds 300 and
     falls at 450, as it did when it only leaned; at the default 0.5 s
     it holds 1000 with one or two steps and falls at 1400; at 1 s it
     holds 1400 and falls at 1800; 32 tries hold no more than 16.
     Against part 4: `balance` with its help (a quarter of body weight
     on the hips) holds 1400, with steps 1800, at no cost.
   - What it costs, a 4 s take (0.2 s without): at the defaults 63 104
     steps tried for 960 taken, 18.9 s on one core, 12.2 s on two, 6.3 s
     on four (a try's copy of the world is made on its own core; four
     cores give three times, not four); 1 s ahead, 9 s on four. So the
     search now buys unhelped balance up to what the helped behaviour
     holds, at 30 to 45 times the bake on four cores. To decide: whether
     takes want the unhelped look enough to pay for it.
   - Left: a step is the `step` behaviour's (a foot carried, not a leg
     swung), one at a time, and only toward where it is falling; arms
     only swing from the shoulder (no elbow, no reach for support); the
     plan is ten numbers, not a pose; it does not get up. In a try a
     crowd's agents walk on straight at the speed they had (they do not
     steer in it).
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
   - Left: pictures cast and take no shadows from the scene, and are
     drawn again whenever the lights change (every frame under an
     animated light); an agent pops when it changes between mesh and
     picture; the path tracer sees a card, not a body. A fallen agent
     past the pool stands frozen, and none gets up. A way
     is planned round what stands still only (its level box: a wall at
     an angle shuts its whole box), by each agent alone (no lanes, no
     waiting for a gap: a doorway jams), and not round a body in
     motion. An agent steps aside and does nothing else: no guard, no
     duck, no look (it has one clip), and it does not get out of the
     way of other agents' falls. `check` evaluates every agent at every
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
that makes real edits through a `Toolbox`. **No request was sent to
Claude Code** (it costs money). Claude Code 2.1.329 was started once
against an address nothing listens on: it took the flags, started
`lahza mcp`, listed only its tools (`mcp__lahza__…`) and wrote the stream
the reader expects, then failed to connect, at no cost. So not seen: a
real agent calling the tools without being asked for permission
(`--allowedTools mcp__lahza --permission-mode dontAsk`), reading the
picture, or its wording when signed out.

1. **A prompt box** (`motion_prompt_ui.rs`, the field from
   `text_field.rs`): under the preview on a motion project and on a
   recording's Motion tab.
2. **Starting an agent** (`motion_agent.rs`): `Runner` is the one small
   interface (available, start, and a `Running` to stop), with two
   runners behind it and a choice between them (`Chosen`; the second,
   `ApiLoop`, is under "Left" below for what was not run). `ClaudeCode`
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
  on `<project>/.lock`) is taken by `edit_apply` and by undo and redo;
  under it the writer loads what the other wrote, then writes. Two
  writers of 25 edits each lose none and take no number twice
  (`two_writers_do_not_lose_edits`). The editor stays usable. Not under
  the lock: tools that write other files (markers, shaders, narration).
- **How long it takes**: not measured, since no real request was run. The
  bar and the ghost clip say the step, the seconds so far, and the steps
  and edits made. What is left is not known to the runner and is not
  said.

Left:
- **A real request, on a real desktop.** The first thing to do: send one
  from the bar and watch for a permission refusal in the stream.
- **Requests are kept beside the project** (`requests.json` in the
  project's or the recording's folder: the words, the steps, the outcome
  and the conversation to continue), so the AI tab and a follow-up
  survive the app. Read and written in a unit test; not seen across a
  real restart. A request running when the app closes reads back as
  stopped, and the agent is not ended with the app unless the window's
  close drops the studio (not checked).
- **A piece made on a recording.** With no piece open the words ask for
  a new one on the span. Undo then takes back the edits inside it, not
  the piece or its place on the video; compare needs the piece open; and
  no ghost clip shows, only the bar. Not run with a real agent, so "a
  piece placed there comes back" is not seen.
- **Undo after later edits** to the same things fails with the engine's
  message ("the project changed in a conflicting way").
- **The second runner, against the real API.** `ApiLoop`
  (`motion_agent_api.rs`) needs only an API key: our own loop on the
  Claude Messages API (raw HTTP through `ureq`, the one new dependency;
  there is no official Rust SDK) with the `Toolbox` in process: its own
  tool definitions, plus `read_doc` for the guide, schema and catalog an
  MCP client reads as resources. `claude-opus-5-5` (or
  `LAHZA_AGENT_MODEL`), one request a turn, not streamed, with
  `fallbacks: "default"`; the same `compose`d words and picture and the
  same steps as the Claude Code runner; every result of a turn goes back
  in one message; a refusal or a cut-off answer runs none of its tools;
  no connection, 429 and 5xx are tried three times; Stop is looked at
  between calls (a request already sent is waited for and then dropped).
  The key is `ANTHROPIC_API_KEY` or one line in
  `<config>/anthropic-api-key`, read when a request starts, sent in the
  `x-api-key` header and nowhere else (a test looks for it in the
  project's files and in what is reported). The runner is a choice in
  the AI tab (Auto, Claude Code, API key), kept with the settings: Auto
  is Claude Code where installed and signed in, else the API loop where
  a key is set, and the bar names the one in use. Tested against a local
  stub server that replays a tool-use conversation, a refusal, a cut-off
  answer, 401/400/404/429/500/529, nothing listening, and a stop in the
  middle. **No request was sent to the API**: the version header, the
  model id and the request's shape are from Anthropic's reference as
  read in November 2026, not from an answer. Left: nowhere in the app to
  type the key (the field has no masked input: the environment or the
  file); the loop's conversations are kept in memory only, so after a
  restart a follow-up starts a new one; `cost` is not reported; no
  streaming, so a long turn shows nothing until it ends.

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
  clears it; Shift-drag anywhere on the ruler does the same. The strip on
  a recording is seen in a screenshot; its drag was driven through the
  code, not with a pointer.
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
  Left: no progress for a long bake (only "Baking the simulation…"), and
  no way to stop one.
- **By hand stays cheap**: the inspector edits a text layer's words (shown
  as typed, saved when the field is left), its font (the bundled ones and
  the font files in the project's `assets/`), colour, size and tracking;
  each effect's numbers, two-part values (a shadow's offset as X and Y)
  and colours; and adds or removes every effect but `gradient_map` and
  `lut` (a list of stops, a file). A colour is picked from ten swatches or
  typed as #rrggbb. Left: those two effects, a text's stroke, edge, line
  height and alignment, and a colour picker beyond the swatches.

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
    The same holds for a shadow's edge in the other three tests: the
    GPU's directional map is still framed by boxes and balls, a little
    wider than the CPU's from every point.
