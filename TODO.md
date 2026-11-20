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
  texels); bounding spheres would tighten it. Run on the RTX 3060 since: see
  "Tests on a hardware GPU" below.
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
  every sample).
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
  Not covered: a camera that jumps within a scene (the layers are the
  same; what was accumulated is tested texel by texel against depth and
  normal, as before), and a transition between two scenes, during which
  the two are traced lists 0 and 1 and after which the second becomes
  list 0 and starts again though it had a history.
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
  a cut; a frame alone and warmed up. All pass on software Vulkan, with
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
  and a transition between two traced scenes.
- The time of `fast_tiles`, `fast_catcher` and the new `resolve_fast` at
  1080 x 1920 (they were about 7 ms for the visibility pass and under a
  second a film for the depth of field before).

What is missing:
- **The editor's preview takes `fast`.** Only `render` and `snapshot` do.
  What it needs is there (`Renderer::set_path` with `fast`,
  `try_render_warm` after a scrub); src/* is to call it. To settle there:
  the preview's own size and pacing.
- **Transitions and camera jumps** keep or lose the history as above.
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
  texel the tracer finds the surface it shows and sends two rays from it
  (`SKY_RAYS`), spread by the cosine, toward points of one fixed set: a
  texel has those of its place in a 4 x 4 tile, so sixteen texels
  together have 32 directions (pt.wgsl `sky`). An open ray adds its
  direction; one that ends on a surface adds that surface's light: its
  colour under the lights (shadows by rays) and under the sky, horizon
  and ground. `sky_filter` makes the sixteen one value over the texels on
  the same plane, each place of the tile weighing the same. The group
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
    72 ms against 44 ms without. By the profile, summed over five
    frames: the rays 16 ms, the filter 4.5 ms, the meshes' structures
    5.5 ms (built once), the top structure 0.6 ms, the tables 34 ms on
    the CPU, and the kernel's pipeline 1.1 s once. Software Vulkan's
    numbers. In the editor's preview: not run.
    With motion blur each sub-frame has its own rays (they are not
    shared as a traced picture's are).
  - *The CPU path.* Not done: the CPU draws as without (tested), so with
    `sky` on the GPU's and the CPU's pictures differ, which is why it is
    off unless asked and the tests that compare the two run without it.
    A GPU that fails in the middle of an export falls back to the CPU
    and the lighting changes there. The rays are a fixed set with no
    history so that the CPU can cast the same: it needs a BVH over the
    list's triangles in camera space, the same directions, the hit's
    lighting (its shadow maps would stand in for the shadow rays, so not
    quite the same) and the same filter.
  - *Without hardware ray tracing* the frame is drawn as today: the
    builder only asks for sky visibility where the GPU traces.
  Against a traced frame of the same scene (a ball, a box with a slab
  over it, a floor and a wall; 320 x 180 and 480 x 270, 64 paths):
  - Lit by the environment only: where the ball stands the floor is 0.69
    of the plain raster's (traced: 0.62), the open floor 0.94 (0.99); the
    whole picture 25.4 dB from the traced one (26.0 dB without sky
    visibility: the contacts are there now, but the picture is 6% darker
    than traced, one bounce being less than all of them, and the light
    that comes back is blotchy at two rays a texel).
  - With the default lights: 33.6 dB from the traced picture, 27.3 dB
    without. Seen: the wall takes the floor's light, the ball's underside
    is lit from below, the gap under the slab is dark, as traced.
- **One bounce of diffuse light** was to come only if visibility alone
  still looked flat. It did worse than that: every face that looks at
  another lost the light the environment's ground and horizon stood for
  (the wall and the box's sides plainly darker), and the picture was 16.1 dB
  from the traced one where the plain raster is 26.0 dB. So the bounce
  is in, from the same rays, as above.

What is left:
- The CPU's rays, as above; until then `sky` stays off by default.
- The editor's preview does not ask for it (src/*: `Renderer::set_sky`).
- Run on the RTX 3060: `cargo test -p lahza-motion-render sky_tests --
  --test-threads=1` and its measure (`--ignored`,
  `LAHZA_MEASURE=1080,1920`): the cost there (rays, filter, the
  structures of a film's scenes), whether four rays a texel (`SKY_RAYS`)
  are affordable (they would halve the blotches), and the picture
  through the raster's hardware visibility pass, which software Vulkan
  does not have (only the compute raster's shading was run with it).
- Only solid meshes are in the rays: what is see-through, cards, wires,
  volumes and shadow catchers neither hide the sky nor take it (they are
  lit as before).
- With an environment image the light that comes back is the lights'
  only (the image is not looked up at what the rays hit).
- A second bounce (the 6%), and anything mirrored of other things: not
  done. The fast mode has them.

Not to do: light probes placed through the scene (irradiance volumes,
DDGI). They suit large scenes that stand still; these are small and
everything in them moves, a grid would be relit every frame, and probes
blur away the contact shadows that are what is missing.

## Simulation: rigid bodies, ragdolls, crowds

Not started; nothing here is measured. The engine has no physics and no
skeletons or skinning (`model_import` reads the animation of nodes; morph
targets are refused), so every part below is new. In the order to build:

1. **Rigid bodies and joints.** `rapier3d`, built to give the same result
   everywhere (`enhanced-determinism`); it has joint limits and motors.
   - A simulation has state, and a frame here is drawn from its time
     alone. So it is baked: stepped at a fixed rate from its start, each
     body's transform kept per frame in the project's folder, and baked
     again when what it depends on changes. Drawing reads the bake, so
     scrubbing, the CPU and GPU pictures and exports stay as they are.
   - In the document: which mesh layers are bodies (shape, mass,
     friction, bounce), what stands still, and pushes at times (time
     references as keys take them).
   - Of use by itself: letters that tumble, things that drop, stack and
     knock into each other.
2. **Skeletons and skinning.** glTF skins (joints, weights, inverse bind
   matrices); skinning as a deformer in the GPU's mesh pool, with the
   CPU's the same. Needed for any character.
3. **A ragdoll that falls.** A capsule per bone and joint limits from the
   skeleton; it takes the pose and speed an animation left it with.
4. **A ragdoll that acts.** Motors follow a clip; a limb's strength is
   keyed (a leg that was hit bears no weight); behaviours with params set
   the motors' targets: balance, a step to keep it, a hand toward what is
   coming, a reach for where it was hit, arms about the head. Balance and
   stepping are the hard part, and what stops a motor pressing into a
   wall.
5. **Directing it.** What writes the film (an LLM) writes intent on the
   timeline ("pushed from the left at 2.1 s, tries to stop it with the
   right hand, stumbles, recovers") as events and behaviours; the bake
   runs; `snapshot` and `review` look at the take, and another seed is
   another take. Nothing is in real time, so takes cost only time. This
   stays the same whatever sets the motors.
6. **A network that sets the motors**, behind 5: each step it reads the
   body's state and what it touches or is about to, and gives the
   motors' targets. Running one is `ort`, already here. Getting one:
   - a policy trained by reinforcement learning: a simulator run many
     thousands of times at once on a GPU, motion capture to learn from
     (its licences to be read), days of training; a policy holds for the
     simulator and the body it was trained with, so one trained elsewhere
     may not hold in rapier;
   - or no training: each step, try many targets a little way ahead in
     the simulator and keep the best. No data; it costs time a shot.
   To decide after 4, against what 4's behaviours already do.
7. **Crowds.** Steering for where each one goes, a clip played per agent,
   a ragdoll only for the ones hit. A skinned agent is a mesh of its own
   every frame (in the tracer an acceleration structure each): how many an
   export bears is to be measured; thousands would be pictures of agents
   (impostors), not meshes.

## Directing an agent from the editor

Not started. To say in the editor what is wanted ("add five seconds of …
here", "this one should look like …", "something is wrong with this one"),
see the agent at work, and scrub what it made. "Generated" here is motion
the agent builds with the engine's tools; a clip from a video model would
be something else to connect.

Already here:
- An agent's edits appear live: the editor polls the project's folder
  (`motion_editor.rs`, every 30 ticks) and reloads.
- One history: each edit is an entry with its author and its inverse
  (`io.rs` `HistoryEntry`), so undo works on what an agent did.
- The editor knows what is pointed at: the selected layer (`selected`),
  its box (`selection_bounds`) and the playhead.
- The agent can look: `snapshot`, `review`.

To build, in this order (1 to 4 with one selected layer already cover
"fix this one"):
1. **A prompt box in the motion editor.** It has no text input; the app
   has one elsewhere (`text_field.rs`).
2. **Starting an agent from the app.** First: Claude Code without its
   terminal, as a child process on `lahza mcp`, its steps read as they
   come (a stream of JSON). Behind one small interface, so another client
   or a loop of our own (an API key, the `Toolbox` in process) can take
   its place. It needs Claude Code installed and signed in: say so where
   it is not.
3. **What goes with the words.** The selected layer's id, the playhead,
   the scene, and a snapshot of the frame as it is, so "this one" needs no
   describing. A follow-up continues the same conversation.
4. **A panel for the agent at work.** Each step as it happens (what was
   added, what was checked, what is being drawn), what it said at the end,
   and a stop button that ends the child.
5. **Undo for a whole request.** An agent makes many edits a request and
   each is its own entry: entries need the request they belong to, and
   one undo takes them all back.
6. **More than one thing selected.** A span of time ("five seconds from
   here") and several layers; the editor holds one layer.

To settle first:
- **Editing while the agent works.** Nothing was found that keeps the
  editor and the server from writing the folder at once (both number
  history entries from what is there). Either the editor only reads while
  a request runs, or the writes are guarded.
- **How long it takes.** A scene of five seconds is many calls: tens of
  seconds to minutes (not measured). The live reload shows it being put
  together; the panel should say what is left where that is known.

## The motion editor: drags that stall, and how it should be used

### Drags that stall

Changing a selected layer's values is slow: the bars stick, as if a
render had begun. Read in the code and timed apart from the app; the
editor was not run, so which of these is the one felt is not confirmed.

- **A drag draws at full quality (first to fix).** A drag is meant to
  draw at half size with no motion blur. `motion_piece_frames`
  (motion_piece_ui.rs) asks only whether a piece is dragged on the
  timeline (`motion_piece_drag`); a value dragged in the inspector, a
  layer moved and a key retimed are `motion_layer_drag`, so every tick
  asks for a full frame with motion blur. The editor's own
  `request_frame` (motion_editor.rs) has the same gap: its `drag` is set
  by the canvas only. Both should ask one question: is anything dragged.
  To look at on a real card once fixed: whether full frames on the GPU
  that also draws the window are what holds the bars (the frames are on
  a worker thread, so the UI thread alone should not stall).
- **Every tick goes through the whole project.** `preview`
  (motion_editor.rs) calls `edit::apply`, which turns the whole project
  into JSON and back, then `Compiled::new` on all of it. Timed in a cloud
  container on projects of plain shapes and text (a throwaway example,
  not kept):

  | layers | a tick (apply + compile) | the drop (apply, save, load, 2 compiles) |
  |---|---|---|
  | 30 | 0.4 ms | 1.3 ms |
  | 480 | 7.1 ms | 19 ms |
  | 1 200 | 19 ms | 52 ms |

  Nearly all of a tick is `apply` (6.0 of 7.1 ms at 480 layers; compile
  1.1). Past 16 ms on a large project, and sooner with an imported model
  of many parts. To do: a drag changes the one property in a copy of the
  project (no JSON) and recompiles only its scene; `apply` stays for the
  drop.
- **The drop reads all the history.** `next_history_number` reads and
  parses every entry to find the last: 2.4 ms at 500 entries, 10.5 ms at
  2 000, and agents write many. To do: the highest file name.
- **The inspector asks for each value twice a draw** (`field_state`:
  `eval` for whether it is keyed, then for the value), each turning the
  layer into JSON and compiling the property. 0.004 ms for a plain layer;
  a comp is turned with all its children. To do: once per field, from the
  compiled layer.

### How it should be used

The rule: select, type, Enter; everything else is by choice. The fewest
clicks for the common things, the same in the motion editor and on a
recording's timeline. This is how the parts under "Directing an agent
from the editor" look.

- **One prompt bar, the same everywhere**, under the preview. It shows
  what it will act on as chips that can be removed ("Layer: title",
  "2.1 s to 7.1 s"). Not a modal: that covers what is being talked about.
- **Typing with something selected begins a prompt** with it attached:
  "fix this one" is a click, the words, Enter.
- **A drag on the ruler marks a span**: mark five seconds, type what
  should be there, Enter. On a recording's timeline what comes back is a
  motion piece placed there, as pieces already are.
- **The inspector has two tabs**: Properties (by hand), and AI (what was
  asked about this layer, and what came of it).
- **The agent's work is seen and does not block**: a ghost clip over the
  span being made, with the step it is at; the rest of the editor stays
  usable (see "Editing while the agent works" above).
- **What comes back is a proposal**: keep, undo, again, and a key held to
  compare before and after (the request's entries taken back and put
  back).
- **A drag never waits**: the proxy on every drag, the number at once,
  saved on release.
- **By hand stays cheap**: a nudge or a retime is quicker than a prompt,
  so the inspector has to edit text and effects, which it does not.

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
  commits. One thread runs through.
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
  `defocused_instancer_copies_keep_their_depth` (3, 0). Two assume no Open
  Image Denoise and Mesa's software Vulkan
  (`pt::physics_tests::{frames_in_flight_are_the_frames_drawn_one_at_a_time,
  the_denoiser_works_on_the_tracers_memory_where_it_can}`).
