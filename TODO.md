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
builds it from a DLSS SDK checkout into ~/.local/lib/lahza). Motion blur is
a blur of that picture along each texel's motion (`resolve_fast`). On the
RTX 3060, 1080 x 1920: s01 of the CPU film 24 ms a frame (1.5 s as usual),
work_e's scenes 18 ms; the reconstruction itself 6 ms. A changed trace
kernel costs the driver about a minute once.
- Depth of field is a gather on Ray Reconstruction's picture by each
  texel's circle of confusion (`dof_fast`; what is lit and what is flat
  in a texel have their own depths, from `visible`), before the motion
  blur. A circle is at most 24 texels; a blurred thing in front of a sharp
  one is only found within reach of eight probes. It cost under a second
  on s01's 449 frames, 8 s on work_e's 964.
- **Elsewhere than NVIDIA** (or without the shim) the frames are traced as
  usual, with a warning, and without motion blur. To do: few samples with
  temporal accumulation and Open Image Denoise.
- What is flat (unlit cards and pictures seen directly) and every
  coverage come from a pass of four primary paths a texel of the full
  picture, through fixed points (pt.wgsl `visible`), so edges and
  backdrops stand still and are sharp; Ray Reconstruction's lit colour is
  laid under that coverage. It costs about 7 ms a frame on s01. A shadow
  catcher's shadow there is those four paths' (grainy).
- Motion blur follows a texel's own motion only (a moving edge does not
  smear over what is still), and what nothing was hit behind has none.
- The specular guide is the surface's F0, the albedo guide the denoiser's
  (a metal's is its colour); not compared against NVIDIA's formula.
- The whole CPU film (5 856 frames) in 2 min 46 s, loading included.
- Fixed on the way, in the usual path-traced export too (it was there
  before this branch): a picture over a traced one (an effect on a traced
  scene) was drawn before the trace's picture was resolved, so it showed
  what its buffers held some frames before, at that frame's place in the
  buffer: the scene jumped sideways and its first frames were empty.
  Such groups are now drawn with the rest (`Rest::later`).
- Not run: `cargo test`; the editor's preview (only exports take `fast`).

What is missing, most wanted first:
- **The editor's preview takes `fast`.** Only `render` does (`export.rs`),
  so what is looked at while working is the raster. Where the card allows,
  the preview would be the traced picture with no new lighting to write.
  To settle: scrubbing breaks Ray Reconstruction's history (a frame with
  none before it is its worst), and the preview's own size and pacing.
- **Elsewhere than NVIDIA.** As above: few samples with temporal
  accumulation and Open Image Denoise, in place of 1.5 s a frame. Motion
  blur and depth of field as the fast mode's passes (`resolve_fast`,
  `dof_fast`), which do not need Ray Reconstruction.
- **`snapshot` takes `fast`.** An agent cannot look at a frame as a fast
  export will draw it without rendering the film. A frame alone has no
  history: a snapshot would trace some frames before `at` first.
- **Tests.** There are none for the fast mode, and `cargo test` was not
  run with it. A stand-in for Ray Reconstruction as Open Image Denoise has
  (`pass_through_oidn`) would let the packing, the visibility pass, the
  depth of field and the motion blur be checked without the shim.
- **The shim is built by hand** (`just ngx`, from a DLSS SDK checkout in
  ignore/); neither `just install` nor the snap has it. Whether NVIDIA's
  library may be shipped is to be read in its licence.
- **What is said of it is old.** `render`'s `fast` (tools.rs) and
  `TraceOptions::fast` (pt.rs) say there is no motion blur or depth of
  field; there are both.
- **The picture**, from the notes above:
  - motion blur follows a texel's own motion: a moving edge does not
    smear over what is still, and what nothing was hit behind has none;
  - a circle of confusion is at most 24 texels, and a blurred thing in
    front of a sharp one is found only within reach of eight probes;
  - a shadow catcher's shadow is the visibility pass's four paths
    (grainy);
  - the specular guide is not compared against NVIDIA's formula;
  - to look at: the first frames after a cut (whether the history is
    reset there, and how soon the picture is clean).

## Lighting between the raster and the traced picture

Not started; nothing here is measured. The raster already lights from the
environment as a sphere of light (`envmap.rs`: a diffuse level and levels
blurred for roughness; motion.wgsl `env_diffuse`, `env_light`), with shadow
maps for the lights. What it has not got, and the traced picture has:
- **Occlusion of the environment's light.** Every point takes the whole
  sphere, so creases and where things touch do not darken and things seem
  to float. This is most of what is seen to differ.
- **Light between things.** Nothing lights or mirrors anything else: a
  reflection is the environment's only, and glass is screen space.

First what is missing of the fast mode above (the preview takes it; a
fast mode elsewhere than NVIDIA). Where it runs it is all of the traced
picture for about the rays the visibility below would take (an estimate),
and there is nothing new to keep matching. What follows is for the
preview on a card where neither is quick enough, in this order:
- **Sky visibility in the raster.** One or two rays a texel at half size
  against the tracer's acceleration structures (pt.rs `geometry`), giving
  how much of the sphere a point sees and from where (a bent normal);
  `env_diffuse` is looked up along it and weighted by it, the specular
  term by it too. Kept over frames along each surface's motion and
  filtered as the tracer's lighting is (pt.wgsl `atrous`). To settle
  first:
  - The structures are built for traced lists only: a raster frame would
    build them too (what that costs a frame, and in the editor's preview).
  - The CPU path draws the raster's picture: it needs the same rays (its
    own BVH), or the two no longer agree.
  - Without hardware ray tracing the frame is drawn as today.
  - It is a third lighting to keep close to the traced one: compare
    against a traced frame of the same scene (`--pt`), as the lobes'
    diffuse light was.
- **One bounce of diffuse light**, from the same rays (what a ray hits,
  lit by its shadow map and the environment): colour between things that
  are close. Only if the visibility alone still looks flat.

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

  Left: the tick still compiles every scene (`Compiled::new`, 1.7 ms at
  1 200 plain layers; more with an imported model of many parts, not
  timed). Compiling only the scene that changed needs a way into
  `compile.rs`, which was another branch's file while this was done.
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
