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

Parts 1 to 4 are built; 5 to 7 are below as they stand. Morph targets
are still refused. In the order built:

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
     `motion-sim`'s tests). Not checked on another CPU architecture.
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
   - Left: the motion editor does not reload when only `sim/` changes
     (its own stamp list, `src/motion_editor.rs`), and has no bake
     button. `snapshot` and `render` draw a stale bake without saying so
     (only `check` does). A box landing flat at speed tips a degree or
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
   - Left: no self-collision (an arm passes through the trunk); limits
     are per bone by hand (no defaults by bone name); a limp figure with
     the default 40° limits falls like a plank, slowly.
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
     arm aim toward a body, the last hit, the head).
   - Measured on the test figure (70 kg, pushes at the chest, px/s): a
     statue (motors only) falls at 200; ankle lean alone holds 200 and
     falls at 300; with the default help balance holds 1400 and falls at
     1800; with steps it holds 1800 (four steps, 320 px) and falls at
     2400. Steps without the help do not recover it (it falls at 700).
   - Tested: each of those thresholds, soft after a fall, hands at the
     head within 48 px and down again, a hand on the hit within 25 px in
     half a second and the other arm for a hit arm, a hand up to a ball
     that comes. `examples/sim/ragdoll.py` (limp, held, braced) was
     looked at.
   - Left, and it is the hard part the plan names: balance is not
     footwork, it is a bounded hand on the hips; a step is a foot carried
     by a force, not a leg that swings; nothing gets up after a fall;
     no behaviour looks where it is going. Feet and hands are found by
     bone name (`foot`, `hand`, `head`).
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
