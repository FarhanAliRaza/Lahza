# Lahza motion project format (version 1)

A project is a folder, conventionally named `*.lahza/`:

```
project.json          settings, audio, assets, scene order
scenes/<id>.json      one scene (its layers, camera, timing)
markers/<track>.json  marker tracks: beat, bar, word, hit, …
assets/               files the project uses
history/<n>.json      edit history (ops and their inverses)
```

All times are **timeline seconds** unless written as a reference string
(see `lahza://catalog` → `time_references`). Indices in references are
1-based. Unknown fields are rejected.

## project.json

```json
{
  "version": 1,
  "settings": {"width": 1920, "height": 1080, "fps": 30, "duration": 20,
               "background": "#0b0b0b", "style": "promo.kinetic",
               "motion_blur": {"shutter": 180, "samples": 8}},
  "audio": {"music": {"path": "assets/music.wav", "at": 0, "trim": 0, "gain_db": 0}},
  "assets": [{"path": "assets/music.wav", "source": "user", "licence": "user-supplied"}],
  "scenes": ["intro", "drop"]
}
```

`fps` is a number or a ratio string (`"30000/1001"`).
`renderer` picks how exports draw meshes: `raster` (default; what the
preview shows) or `path` (path traced, see Path tracing under mesh);
`samples` is the path tracer's paths per pixel (default 64; drafts trace a
quarter).

## scenes/<id>.json

```json
{
  "id": "intro",
  "start": "bar:1",
  "end": "bar:3",
  "camera": {"zoom": {"keys": [{"at": "bar:1", "v": 1}, {"at": "bar:3", "v": 1.08, "ease": "push"}]}},
  "layers": [ … bottom first … ]
}
```

A scene ends at its `end`, else `start + duration`, else the next scene's
start, else the project end. A cut on a marker leads it by one frame unless
`lead` says otherwise.

## Layers

Common fields: `id`, `kind`, `name`, `transform` (`position`, `anchor`,
`scale`, `rotation`, `opacity`), `time` (`start`, `end`, `duration`,
`step_fps`, `step_phase`, `offset`), `blend`, `effects` (`blur`, `shadow`,
`glow`), `matte` (`{"layer": "<sibling>", "mode": "alpha"}`), `parent`, `follow`, `hidden`,
`space` (`"world"`, the default, or `"screen"`: the layer ignores the scene
camera's zoom, orbit and dolly and stays where its transform puts it on
screen, drawn in layer order above a moving 3D camera; use it for titles,
captions and finishing layers in 3D scenes).

3D layers (`three_d`, and every `mesh`) also take `transform.z`,
`rotation_x`, `rotation_y` and `scale_z`. Space is x right, y down, z away
from the viewer; angles are degrees. A layer's point p goes to its
parent's space as

    position(x, y, z) · Ry(rotation_y) · Rx(rotation_x) · Rz(rotation) · S · (p − anchor)

so it is scaled first, then turned clockwise on screen (`rotation`), then
tilted (`rotation_x`: positive tips the top away), then turned
(`rotation_y`: positive turns the right side away), then moved; a parent's
transform applies after its child's. S scales x and y by `scale` and depth
by the mean of |scale x| and |scale y| times `scale_z` (default 1), so a
scale animation or a scaled parent scales depth too. To turn about another
axis first, parent the layer to a null and rotate that.

Kinds: `shape` (`circle`, `rect`, `capsule`, `ring`, `arc`, `polygon`,
`star`, `line`, `path` with SVG `d`, `field`), `text`, `image`, `video`, `comp`,
`instancer`, `mesh` (lit 3D objects, see [mesh](#mesh)).

```json
{"id": "route", "kind": "shape", "shape": "path", "d": "M -300 0 C -100 -200 100 200 300 0",
 "stroke": {"color": "#ffd23f", "width": 8},
 "trim": {"keys": [{"at": "word:route", "from": [0, 0], "v": [0, 1], "ease": "glide", "dur": "20f", "anchor": "start"}]}}
```

Path strokes take `cap` (`round`, `butt`, `square`), `join` (`round`,
`miter`, `bevel`), `miter_limit`, `dash` (`[dash, gap, …]` px),
`dash_offset` (animatable: key it linearly for marching ants), `feather` (px faded at the far end, so a stroke drawn on
with `trim` has a soft head) and `feather_start` (px faded at the start, a
fading tail):

```json
{"id": "guide", "kind": "shape", "shape": "path", "d": "M 0 450 H 1080",
 "stroke": {"color": "#3b35b0", "width": 3, "cap": "butt", "dash": [14, 12]}}
```

A path stroke's colour can run along its length: `gradient` replaces
`color`; offset 0 is the path's start and 1 its end, measured over the
full path, so `trim` reveals the colours without sliding them while it
draws on. It works with caps, joins, dashes and feathers (`along` is
`"length"`, the default). Other shapes: use a path primitive
(`"d": {"shape": "circle", "size": 200}`):

```json
{"id": "route", "kind": "shape", "shape": "path", "d": "M -300 0 C -100 -200 100 200 300 0",
 "stroke": {"width": 8, "gradient": {"stops": [[0, "#ff3d7f"], [1, "#ffd23f"]], "along": "length"}},
 "trim": {"keys": [{"at": 0, "v": [0, 0]}, {"at": 1, "v": [0, 1]}]}}
```

Gradient fills take `"units": "box"` to place `from`/`to` as fractions of
the shape's box ([0, 0] top-left, [1, 1] bottom-right; a path's box is its
bounds), so the gradient stretches with an animated `size`; radial ones
become ellipses. The default `"layer"` units are layer px:

```json
{"id": "bar", "kind": "shape", "shape": "rect", "size": {"keys": [{"at": 0, "v": [10, 40]}, {"at": 1, "v": [600, 40]}]},
 "fill": {"gradient": "linear", "units": "box", "from": [0, 0.5], "to": [1, 0.5], "stops": [[0, "#2d5bff"], [1, "#00d4a6"]]}}
```

`shape: "field"` fills its `size` box (corners rounded by `radius`) with a
smooth blend of coloured `points` (mesh / blob gradient backgrounds, one
GPU primitive). Each point has an animatable `position` (layer space,
centred like `d`), `color` and `radius` (px it holds its colour, default
200). `softness` (default 1; 0.25 crisp, 2 very soft) sets how far
colours blend; `warp` (px) and `warp_scale` (feature px, default 300) bend
the blend with noise:

```json
{"id": "bg", "kind": "shape", "shape": "field", "size": [1920, 1080], "transform": {"position": [960, 540]},
 "softness": 1, "warp": 40,
 "points": [{"position": {"keys": [{"at": 0, "v": [-600, -200]}, {"at": 6, "v": [-200, 100]}]}, "color": "#ff3355", "radius": 350},
            {"position": [500, 200], "color": "#7b61ff", "radius": 400},
            {"position": {"base": [0, 0], "add": [{"type": "wiggle", "hz": 0.2, "amp": [200, 120]}]}, "color": "#00d4a6"}]}
```

`follow` places a layer on a sibling path layer, evaluated at every
moment (motion-blur samples and stepped time included), from the path as
drawn: its morph, trim and full transform (parents and comps too). `at`
is `"head"` (the trim end, the default: a handle riding a stroke as it
draws on), `"tail"` (the trim start) or a fraction of the full length
(animatable, `{"keys": …}`). `rotate: true` also turns the layer along the
path's direction (added to its `rotation`); `offset` (px) moves it off the
point, along the path's direction frame when rotating. The follower's
`position` and `z` are not used, and it can't also have a `parent`:

```json
{"id": "pen", "kind": "shape", "shape": "ring", "size": 36, "thickness": 8, "fill": "#ffffff",
 "follow": {"layer": "route", "at": "head", "rotate": false, "offset": [0, 0]}}
```

An `instancer` (a cloner) draws its `layers`, the template, once per
point of its `layout`, each copy centred on its point in the instancer's
own space (its `position` is the layout's centre). Copies are cheap: use
one instancer, not hundreds of layers with their own keys.

```json
{"id": "dots", "kind": "instancer", "transform": {"position": [960, 540]},
 "layout": {"type": "grid", "columns": 48, "rows": 27, "spacing": [40, 40], "hex": true},
 "stagger": {"total": "20f", "order": "center"},
 "random": {"seed": 3, "scale": [0.6, 1.0]},
 "palette": ["#ffffff", "#ff3d7f"], "palette_order": "random",
 "fields": [{"type": "wave", "center": [0, 0], "at": 3.82, "speed": 900, "wavelength": 240, "scale": 1.5}],
 "layers": [{"id": "dot", "kind": "shape", "shape": "circle", "size": 12, "fill": "#ffffff"}]}
```

Layouts (`type`): `grid` (`columns`, `rows`, `spacing`, `hex`), `radial`
(`count`, `radius`, `start` and `sweep` in degrees from up, clockwise;
`align` turns each copy so its up points outward), `line` (`count`,
`from`, `to`, `align`), `random` (`count`, `size`, `area`: `box` or
`circle`, `seed`) and `path` (`count` spread along a sibling path layer
as drawn, `offset` slides them along it, `align`). `spacing`, `radius`,
`start`, `sweep`, `from`, `to`, `size` and `offset` are animatable.

`emitter` makes the template a particle: born `rate` per second over a
`size` box (or `area: "circle"`), launched `direction` (degrees, 0 = up)
± `spread`/2 at `speed` px/s (± `speed_jitter`), falling with `gravity`
[x, y] px/s² (default [0, 300]) and slowed by `drag` (fraction per s),
spinning up to `spin` °/s, living `lifetime` s (± `lifetime_jitter`).
`size_over_life`, `opacity_over_life` ([[life 0..1, value]]) and
`color_over_life` ([[life, colour]], tints the template) shape each
particle's life; the template's own animation plays from its birth.
Emission runs from `start` (layer time) to `stop`; `prewarm` starts full.
It is worked out per frame from the seed (scrub freely, frames repeat).

```json
{"id": "bubbles", "kind": "instancer", "transform": {"position": [960, 1000]},
 "layout": {"type": "emitter", "rate": 30, "lifetime": 3, "size": [600, 0], "speed": 180, "spread": 20,
            "gravity": [0, -60], "drag": 0.5, "size_over_life": [[0, 0.3], [0.2, 1], [1, 1.2]],
            "opacity_over_life": [[0, 0], [0.1, 0.9], [1, 0]]},
 "layers": [{"id": "bubble", "kind": "shape", "shape": "circle", "size": 18, "stroke": {"color": "#ffffff", "width": 2}}]}
```

Per copy:

- `stagger`: copy k sees time `t − delay`, its rank in `order`
  (`forward`, `reverse`, `center`: rings out from the middle, `edges`,
  `random`) times `each`, or the ranks spread over `total`; the template's
  keys and window start that much later.
- `random` (`seed`): `[min, max]` ranges for `scale` and `opacity`
  (multipliers), `rotation` (degrees) and `time` (extra delay, s);
  `position` a ± jitter in px; `color` a random mix of two colours.
- `palette` with `palette_order` (`cycle` or `random`) replaces the
  template's solid fill, stroke and text colours, keeping their alpha.

`fields` are effectors applied in order. Each gives every copy a value
0..1 at its layout point, then adds value × delta: `position` ([x, y]
px), `scale` (added to the copy's scale: 1 doubles it), `rotation`
(degrees), `opacity` (added: −1 hides) and `color` (mixed in). `ease`
reshapes the value (any curve), `invert` flips it, `amount` weights it.

| `type` | value | parameters |
|---|---|---|
| `radial` | 1 within `radius − falloff` of `center`, 0 from `radius` | `center`, `radius` (200), `falloff` (px, default the radius) |
| `linear` | ramps 0 → 1 over `range` px along `angle`, ½ at `center` | `center`, `angle` (degrees, 0 = +x), `range` (200) |
| `noise` | smooth noise | `size` (feature px, 200), `speed` (per second, 0.5), `seed` |
| `wave` | a ring leaving `center` at time `at` | `at` (default the layer start), `speed` (px/s, 600), `wavelength` (crest width px, 200), `cycles` (crests, 1; 0 = endless ripples), `decay` (px) |

Every field parameter and delta is animatable except `at`, `seed`,
`cycles` and `decay`. Copies whose opacity or scale reaches 0 are not
drawn. With opacity below 1, a blend mode, effects or a matte, the
instancer is drawn as one picture (like a comp).

Scenes may arrive with a transition centred on their cut:
`"transition": {"type": "whip", "dur": "8f", "direction": "left"}`
(`fade`, `push`, `slide`, `zoom`, `wipe`, `whip`).

Depth of field (2.5D): the scene `camera` takes `aperture`, the blur
radius in px per 100 px of depth away from the focus plane (0 = off), and
`focus`, the sharp plane as a depth read like a layer's `z` (0 = the point
of interest's plane, the default; animatable for a rack focus) or the id of
a top-level layer to keep sharp. Every 3D layer (and each child of a 3D
comp) blurs by its distance from that plane, up to an eighth of the frame's
longer side (beyond that a layer is a soft wash of its colour); 2D layers
are unaffected:

```json
"camera": {"aperture": 2, "focus": {"keys": [{"at": "bar:3", "v": 0}, {"at": "bar:4", "v": 600, "ease": "glide"}]}}
```

Shapes and text are centred on their origin: `position` places their centre.

```json
{"id": "title", "kind": "text", "text": "MOVE FAST", "size": 160,
 "transform": {"position": [960, 540]},
 "time": {"start": "beat:1"},
 "animators": [{"at": "beat:1", "stagger": "1f", "dur": "8f", "ease": "enter",
                "from": {"y": 80, "opacity": 0}}]}
```

Text styling comes from the glyphs' distance field, so it stays crisp at
any size and per-glyph animators still apply. `stroke` (`color`, `width`
px, `align`: `outside` (default, drawn under every fill so touching
letters share one outline), `center` or `inside`) outlines the glyphs;
a transparent `color` with a stroke gives outline-only type. `weight` (px)
moves the edges outward for a faux bold (negative thins). `edge` (`color`,
`width` px, `angle`: where the light comes from, 0 = above, clockwise)
lays a rim light inside the edges that face the light. All of these are
animatable; weight plus an outside stroke reach at most ~0.11 em past the
outline.

```json
{"id": "logo", "kind": "text", "text": "LAHZA", "size": 180, "color": "#1d2a6b", "weight": 2,
 "stroke": {"color": "#ffffff", "width": 4},
 "edge": {"color": "#ffe9a0", "width": 6, "angle": -40}}
```

An animator's `order` can also come from position: `{"axis": "x"}`
(`"y"`, or the sweep direction in degrees, 0 = rightward, clockwise; with
`"from": "start" | "end" | "center"`) or `{"radial": [0.5, 0.5]}` (a centre
in fractions of the text block). Each unit's delay is its position across
the text (0 for the first, 1 for the last) times `stagger × (units − 1)`.
`"feather": 0.3` turns the opacity change into a soft wipe 30% of the
text wide that travels through the glyphs over the whole animation;
position, scale and rotation still move per unit. One feathered animator
per text layer.

```json
{"id": "reveal", "kind": "text", "text": "SWEEP", "size": 140,
 "animators": [{"at": "beat:1", "order": {"axis": "x", "feather": 0.25},
                "stagger": "2f", "dur": "10f", "from": {"opacity": 0, "y": 20}}]}
```

```json
{"id": "dot", "kind": "shape", "shape": "circle", "size": 120, "fill": "#d7ff3a",
 "transform": {"scale": {"keys": [{"at": "beat:5", "from": 0, "v": 1, "ease": "pop", "anchor": "start"}],
                         "add": [{"type": "drift", "rate": 0.01, "per": "frame", "mode": "mul"}]}}}
```

### mesh

A lit 3D object rendered by the engine (no Blender). Always 3D: it sorts by
depth with other 3D layers, and mesh layers next to each other share one
depth buffer, so they intersect correctly. Space is the layer's: x right,
y down, z away from the viewer, centred on the layer origin.

| field | meaning |
|---|---|
| `mesh` | geometry, by `type` (below) |
| `material` | surface (below); default preset `glossy`, white |
| `parts` | `front`, `back`, `side`, `bevel`, `caps`, `inside`: a material over `material` for that part |
| `segments` | split rings, revolves and sweeps into this many pieces (default 1) |
| `spans` | relative sizes of the pieces, in turn (`[1, 3]`: a quarter and three quarters); default equal |
| `gap` | between pieces: degrees (sweeps: a fraction of the length); animatable |
| `colors` | one colour per piece, in turn (animatable); parts with their own colour keep it |
| `detail` | tessellation multiplier (default 1: smooth at the drawn size; meshes are tessellated for their on-screen size, so small profiles scaled up stay smooth) |

Geometry (numbers animatable unless noted):

| `type` | fields |
|---|---|
| `sphere` | `radius` (100) |
| `plane` | `size` [w, h] ([100, 100]): a flat rectangle, both sides (floors, walls, backdrops; `rotation_x: 90` lays it flat) |
| `box` | `size` [w, h] ([100, 100]), `depth` (100), `bevel` (edge radius) |
| `cylinder` | `radius` (50), `top` (top radius: cones), `height` along y (100), `bevel` |
| `capsule` | `radius` (50), `height` end to end along y (200) |
| `torus` | `radius` to the tube centre (100), `tube` (20), `angles` [from, to] degrees, 0 = up, clockwise ([0, 360]), `caps`: `flat` \| `round` (not animatable) |
| `extrude` | `d` closed outline (SVG path, `{"shape": …}` primitive or morph keys; holes kept), `depth` (40), `bevel` |
| `revolve` | `profile` path in (x = distance from the axis, y = along it), `angles`, `axis`: `y` (standing, default) \| `z` (facing the camera) |
| `sweep` | `d` path in the layer plane, `radius` (10), `profile`: `circle` \| `square` \| `ribbon`, `width` (ribbon, 40), `trim` [from, to] (0..1), `caps`: `round` (default) \| `flat`, `head` (radius of a leading ball) |

A torus faces the camera like a `ring` shape; tilt it with `rotation_x`.
Rings and revolves name their faces by the way they face: `front`
(toward −axis: the camera for rings, the top for standing revolves),
`back`, `side` (away from the axis) and `inside` (toward it); flat ends
are `caps`. Any part's material can take its own `colors` (one per piece,
in turn), e.g. a ring whose fronts cycle colours while its sides stay one
colour.
Depth scales with `scale` (times `transform.scale_z`, default 1).

Material: `preset` (`matte`, `plastic`, `glossy`, `coated`, `metal`,
`chrome`, `brushed`, `velvet`, `liquid`, `glass`, `emissive`, `flat`, `shadow`) then any of `color`,
`metallic` (0..1), `roughness` (0 mirror … 1 diffuse), `clearcoat` (0..1),
`clearcoat_roughness`, `emissive` (colour), `emissive_strength`, `opacity`,
`transmission` (0..1, glass), `ior` (glass: 1 no bending, 1.45 default,
2.4 diamond), `thickness` (glass: px the light travels inside, default 60),
`unlit` (bool), `faceted` (bool: one normal per triangle), `colors` (per
piece, in turn), `wireframe` (px: draw the edges instead of the surface,
unlit, in `color`; quad diagonals and cap fills are skipped) and `fill`
(wireframe face colour: hidden-line; without it the wires are see-through
and show the back edges). Preset `wireframe` is 1.5 px light grey wires.
Colours and numbers are animatable.

The model follows Blender's Principled BSDF, so materials carry over:

| field | meaning |
|---|---|
| `specular` | dielectric reflection, 0..1: 0.5 (default) reflects 4% head-on like most plastics and paints, 0 none at all |
| `specular_tint` | colour: tints a dielectric's reflection and a metal's edges (Blender's F82 tint: gold turning paler, copper redder at grazing angles) |
| `anisotropy` | 0..1 brushed or spun metal: the grain runs around the mesh's axis (y; z for rings and revolves about z), highlights and reflections stretch across it |
| `anisotropy_rotation` | degrees the grain turns (90: along the axis) |
| `sheen`, `sheen_color`, `sheen_roughness` | soft light at grazing angles (velvet, fabric, peach skin); default colour white, roughness 0.5 |
| `clearcoat_color` | the coat's tint: what is under it is seen through this colour (candy paint) |
| `dispersion` | glass: rainbow fringes as 1 / Abbe number (0.02 window glass, 0.05 flint, more for a stylised prism); path tracing only |

Surface detail (raster and path tracer alike):

| field | meaning |
|---|---|
| `noise` | procedural noise on the surface, no UVs needed: `{"type": "fbm" (default) \| "value" \| "cellular", "scale": px feature size (40), "octaves": 4, "seed": 0, "space": "object" (moves with the mesh) \| "world", "drift": [x, y, z] px/s the noise flows (ripples, boiling), "color": colour where the noise is high, "amount": 0..1 toward it (1), "roughness": ± roughness, "bump": px of apparent height}` (scale, colour, amount, roughness and bump animatable) |
| `map` | an image (project path) over the UVs, multiplying `color` |
| `normal_map` | a tangent-space normal map (green up) |
| `bump_map`, `bump` | a height image (bright = raised) and its height in px (2) |
| `roughness_map` | an image whose green channel scales `roughness` (glTF's metallic-roughness texture; a grey map works too): polished and dull patches on one surface |
| `metallic_map` | an image whose blue channel scales `metallic` (the same glTF texture) |
| `subsurface` | 0..1 of the diffuse light enters the surface and scatters inside before it leaves (Principled subsurface, Cycles' random walk: marble, milky quartz, wax, skin, milk); path tracing only |
| `subsurface_radius` | how far each colour travels inside, relative, `[r, g, b]` (default `[1, 0.2, 0.1]`, as Blender: red furthest) |
| `subsurface_scale` | the radius' scale in px (default 5); an imported model's comes from Blender's Subsurface Scale × `unit` |
| `subsurface_anisotropy` | -1..1 scattering inside: forward (+), back (−), even (0) |
| `uv` | `{"scale": [x, y] repeats (1, 1), "offset": [x, y], "rotation": degrees}` (animatable) |

UVs run 0..1 across each flat face of planes, boxes and extrusions,
around (u) and along (v) cylinders, capsules and revolves (caps planar),
over spheres equirectangularly, and around and across a ring's tube.
A mesh's `displace` pushes its surface out and in for real (the
silhouette changes): `{"type", "scale", "octaves", "seed", "amount": px, "drift": [x, y, z] px/s}`;
the mesh is subdivided to the noise's scale (up to 200k triangles) and
stays closed.

A liquid is a clear surface whose ripples flow: preset `liquid` (ior
1.33, a gentle ripple bump) with a drifting `noise` (and, for waves that
break the silhouette, a drifting `displace`):

```json
{"id": "acid", "kind": "mesh", "mesh": {"type": "plane", "size": [1600, 900]}, "transform": {"position": [960, 700], "rotation_x": 75},
 "material": {"preset": "liquid", "color": "#c8ffb0", "noise": {"scale": 60, "bump": 3, "drift": [30, 0, 25]}},
 "displace": {"scale": 140, "amount": 6, "drift": [0, 40, 0]}}
```

```json
{"id": "rock", "kind": "mesh", "mesh": {"type": "sphere", "radius": 120},
 "displace": {"type": "fbm", "scale": 60, "amount": 18},
 "material": {"preset": "matte", "color": "#8a7a68", "noise": {"type": "cellular", "scale": 20, "color": "#4a4038", "bump": 4}}}
```

A material's `volume` makes a closed mesh a volume of haze, smoke or a
beam instead of a surface: `{"density": light crossing 100 px keeps
e^−density of itself (1), "color": what scattered light takes on (white),
"anisotropy": −1 back … 1 forward scattering (0.3), "emission": colour of
a thick part's glow, "emission_strength": 1}` (all animatable). The path
tracer scatters light inside it (free flights, Henyey–Greenstein, shadow
rays to every light); the raster estimates single scattering along the
chord through the mesh's box (exact for boxes, the ellipsoid in the box
for other shapes). Volumes cast no shadows and never hide what is behind
them; a laser is a thin `cylinder` of glowing volume, a light column a
cone (`cylinder` with `top`) of thin haze lit from above.

```json
{"id": "laser", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 6, "height": 900}, "transform": {"position": [960, 400], "rotation": 90},
 "material": {"volume": {"density": 4, "color": "#000000", "emission": "#ff2a10", "emission_strength": 3}}}
```

Reflection keeps its energy at every roughness (rough metals stay bright,
as with multiple scattering), and the diffuse under a lobe gets only the
light that lobe leaves. Presets `brushed` (spun metal, anisotropy 0.8) and
`velvet` (matte with full sheen) start from these. With the path tracer
glass absorbs by depth: its `color` is what light keeps after `thickness`
px inside, so thick parts are deeper; a `plane` of glass is a thin pane.

```json
{"id": "cage", "kind": "mesh", "mesh": {"type": "torus", "radius": 200, "tube": 60},
 "material": {"preset": "wireframe", "color": "#ffb020", "fill": "#24262b"}}
```

Glass (`transmission` > 0) refracts what is drawn before it: the layers
behind are seen bent through it (by `ior` and `thickness`), tinted by its
colour, blurred by its `roughness` (frosted glass). The `shadow` preset is
invisible except for the shadows falling on it (`color` tints them,
`opacity` is their strength): a `plane` with it drops meshes' shadows onto
2D backgrounds, or, just in front of footage or another flat 3D layer in
the same list, onto that layer (a 3D object casting its shadow on a screen
recording).

Depth: mesh layers in the same layer list hide each other where nearer,
also when one has effects, a matte or depth of field. Meshes that let
what is behind show (opacity < 1, glass, a blend mode) are drawn after the
opaque ones and never hide them. Flat 3D layers (`three_d` shapes, text,
images, footage) in a list with meshes join the same depth test as cards
(their picture on a quad), and lists of two or more tilted 3D layers do
too, so planes cut through each other and through meshes. Cards are drawn
after the opaque meshes and blend by their picture's alpha, so their soft,
defocused and anti-aliased parts show the mesh behind them; a nearer
opaque mesh hides them. Coplanar layers keep their layer
order.

```json
{"id": "coin", "kind": "mesh", "mesh": {"type": "extrude", "d": {"shape": "circle", "size": 300}, "depth": 30, "bevel": 6},
 "material": {"preset": "metal", "color": "#ffc030"}, "parts": {"side": {"color": "#b07a10", "roughness": 0.5}},
 "transform": {"position": [960, 540], "rotation_y": {"keys": [{"at": 0, "v": 0}, {"at": 2, "v": 360, "ease": "glide"}]}}}
```

#### Path tracing

Exports can path-trace meshes instead of rasterising them: project
`settings.renderer: "path"`, or `lahza motion render --pt` (`--raster`
overrides the project; `--spp N` sets the samples). `lahza motion snapshot
--pt` previews a frame the same way (MCP: `pt` and `samples` on `render`
and `snapshot`). Each layer list with meshes becomes one traced picture
composited in the list's place, so 2D layers, effects on the list and
transitions around it are drawn as usual. The tracer shoots rays through
the same camera, so meshes stand exactly where the raster draws them, and
adds what rasterising cannot: shadows from every shadow-casting light by
ray, light bounced between surfaces, reflections of the scene, and glass
that refracts the meshes behind it. Flat 3D layers in the list are cards in
the traced scene. It needs a GPU with ray queries (Vulkan
`VK_KHR_ray_query`); elsewhere the frame is rasterised and a warning says
so. The same frame traces the same picture (samples are seeded by time).
Depth of field is a real thin lens (the camera's `aperture` and `focus`),
so defocus is exact per pixel, through glass and in reflections; motion
blur traces each shutter sub-frame with its share of the samples, so it
costs no more than a still frame. A denoiser cleans the picture: Intel
Open Image Denoise, as in Blender's Cycles, where it is installed (Lahza
finds the copy Blender ships, or `$LAHZA_OIDN`; it runs on CUDA on NVIDIA),
which also removes depth of field's grain, else an edge-aware à-trous
filter of the lighting (of the whole colour where the lens blurs, so
defocused areas are smooth too; `LAHZA_DENOISER=builtin` forces it; the render's
warnings say when it was used), so a few samples per pixel give a clean
frame (`--no-denoise` / `denoise: false` to see the raw samples); in an
export each frame also builds on the previous ones, reprojected along
each surface's motion (camera and objects), and rejects history that no
longer matches, so animation does not boil or ghost. Lists with what the tracer does not
draw yet (wireframes, effects, mattes, blend modes on their 3D layers)
are rasterised, and the render's warnings name the list's layer and why
(for depth of field, use the camera's `aperture`, not a blur effect).

```json
"settings": {"width": 1920, "height": 1080, "fps": 30, "duration": 20, "renderer": "path", "samples": 128}
```

A scene's (or the project's, `settings.look`) `look` sets how lit 3D
content becomes the picture: `{"tone": "standard" (default soft shoulder)
| "filmic" (Hable: toe, long shoulder) | "agx" (Blender's default: bright
colours desaturate toward white like film), "exposure": stops (animatable),
"bloom": {"threshold": 0.8 (display brightness), "radius": 24 px,
"intensity": 0.8}}` (bloom values animatable). Tone mapping applies to
lit meshes (raster and path tracer alike); flat 2D layers, flat 3D
layers among meshes (cards: footage, type, shapes) and unlit materials
keep their exact colours. Bloom blurs what is brighter than the
threshold across the whole scene (2D layers too) and adds it on top.

Scenes light meshes with `lights` and `environment`:

```json
"lights": [{"type": "directional", "from": [-1, -1, -1], "intensity": 1.1},
           {"type": "directional", "from": [1, -0.4, 1], "color": "#a0c8ff", "intensity": 0.7},
           {"type": "point", "position": [960, 300, -400], "intensity": 0.5},
           {"type": "ambient", "intensity": 0.05}],
"environment": {"preset": "studio", "intensity": 1, "rotation": 0}
```

`from` points toward a directional light (y down, z away: `[-1, -1, -1]`
is upper left, in front); `position` places a point light in world px.
`from`, `position`, colours and intensities are animatable (keys of
`[x, y, z]`), so lights can move. Without `lights` a scene has a key
from the upper left front and a rim from behind on the right. Lights cast
soft shadows from every mesh onto every other mesh of the layer list:
`shadow` (bool; default true for the first directional or point light,
false for the rest) and `softness` (0 sharp … 1 very soft, default 0.35;
animatable; the path tracer makes it the light's size: a sun up to 0.1 rad
across, a ball up to 40 px in radius). A point light's `range` (px,
animatable) fades it smoothly to nothing at that distance; without one it
lights near and far alike.

Area lights are glowing panels: `{"type": "area", "position": [x, y, z],
"aim": [x, y, z], "size": [w, h], "shape": "rect" | "disc"}` (`aim`
defaults to the canvas centre at z 0; position, aim and size animatable).
Their light is soft and their shadows soften with their size; `intensity`
1 lights a surface facing it 1000 px away like a directional light of
intensity 1, whatever its size, so resizing a softbox changes its shadows,
not its brightness. Glossy surfaces show the panel's shape (the path
tracer exactly, the raster approximately); the camera does not see it.
`range` fades it like a point light.

```json
"lights": [{"type": "area", "position": [400, 100, -600], "size": [600, 300], "intensity": 1.2},
           {"type": "area", "position": [1600, 500, -300], "aim": [960, 540, 0], "size": [200, 200], "shape": "disc", "color": "#a0c8ff"}]
``` The
environment (`studio` default, `soft`, `sunset`, `night`, `none`; `sky`,
`horizon`, `ground` override its colours; all three are animatable) is
what glossy surfaces reflect and gives soft ambient light. A scene's
`background` is animatable too.

`"environment": {"image": "assets/studio.hdr", "intensity": 1, "rotation": 30}`
lights the scene with an equirectangular image instead of the preset
(image-based lighting): an HDR (`.hdr`, `.exr`) gives real light and
sharp bright reflections; `.png`/`.jpg` are read as sRGB. The image's
centre faces away from the viewer, its top is straight up; `rotation`
turns it. Glossy surfaces reflect it (blurred by roughness), matte ones
take its light, and the path tracer samples its bright parts directly, so
a small sun in it gives clean light and sharp shadows. It lights and is
reflected but is not drawn behind the scene (the 2D layers are). A
missing image is reported and leaves the environment dark.

## Properties

Any animatable field is a plain value or

```json
{"keys": [ {"at": <time>, "v": <value>, "from"?, "ease"?, "anchor"?, "lead"?, "dur"?, "space"?} ],
 "add":  [ {"type": "drift" | "wiggle" | "sine", …} ],
 "step_fps"?: 12}
```

A key is a move that *arrives* at `v`. Without `dur` it runs from the
previous key's end to `at` (a classic keyframe). With `dur` or an `anchor`
other than `end`, it lasts `dur` (default: the ease's natural duration) and
is placed so its anchor moment (`start`, `peak` speed, `land` at 98%, or
`end`) falls on `at + lead`. A move that begins before the previous one
ends takes over from the current value, so motion stays continuous.

Values: numbers; `[x, y]` vectors (scale also takes one number); colours
`"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"`.

## Sounds

`audio` holds the `music`, the narration (`vo`) and sound effects (`sfx`):

```json
"audio": {
  "music": {"path": "assets/track.wav", "at": 0, "gain_db": -2},
  "sfx": [
    {"id": "whoosh1", "path": "assets/sfx/whoosh.wav", "at": "#card.start - 3f", "gain_db": -6},
    {"id": "pop1", "path": "assets/sfx/pop.wav", "at": "beat:9", "length": 0.1}
  ]
}
```

Fields: `path`, `at` (any time reference: an effect anchored to a layer,
`#card.start`, moves when the layer moves), `trim` (seconds skipped in the
file), `gain_db`, `length` (seconds to play), `fade_in` and `fade_out`
(seconds, at the ends of what plays), `name`, `id`. Add one with
`{"op": "add_sound", "path": …, "at": …}` (`"kind": "music"` or `"vo"` for
those); then `set` and `remove` work on its id (`whoosh1.at`), and music and
narration answer to `music` and `vo`. Built-in effects (`sfx_add`):
whoosh, whoosh-out, swipe, pop, click, tick, type, thud, hit, boom, riser,
ding, sparkle, glitch.

## Components

`{"id": "caps", "kind": "component", "component": "captions", "params": {…}}`
generates ordinary layers from parameters: `captions`, `card`, `callout`,
`counter`, `chart` and `map`. The catalog (`lahza://catalog` →
`components`) lists each one's params.

- **Children are addressed as `<id>/<child>`** (`caps/c3`, `map/IRN`);
  `timeline_get` lists them.
- **`set`, `key`, `anchor` and `remove` on a child** are stored in
  `overrides` as a merge patch for that child, keyframes included. They are
  re-applied every time the component regenerates, for example after its
  params change.
- **`{"op": "detach", "id": "caps"}`** replaces the component with a comp of
  plain layers (`caps-c3`), with the overrides applied.

## Time edits

`{"op": "insert_time", "at": 0, "duration": 5}` makes room at `at`:
every literal time at or after it (seconds or `"48f"`) moves later. That
covers:
- scene starts and ends, and layer `time.start` and `time.end`;
- keys, text animators and effects, read in each scene's and comp's own
  time (`pre_roll`, `time.offset`);
- sounds, the project length, user and transcript markers, and the
  narration;
- music analysis tracks, but only when the music itself moves.

References (`word:…`, `#id.end`, `beat:…`) are not rewritten: they move with
what they point at. A layer spanning `at` grows. The op undoes exactly.

`{"op": "shift", "ids": ["s07", "pop1"], "by": "-6f"}` moves scenes, layers
or sounds, with every literal time inside them, by `by`.

## Markers

`markers/<track>.json`:

```json
{"name": "beat", "source": "audio_analyze assets/music.wav",
 "markers": [{"t": 1.093}, {"t": 1.760, "value": 0.8}]}
```

Narration (`narration.json`, written by `narration_import`; not edited by
hand): clips with `id`, `path`, `text` (script with `[cue]` markup), `at`,
`duration` and aligned `words` (`text` as shown, `spoken` when it differs,
`start`/`end` in the clip, `loud` dB under the speech level). Its clips
appear in `audio.narration`; it derives the `line`, `sentence`, `word` and
`cue` tracks.

Marker fields: `t`, optional `end`, `label` (words, OCR text), `id` (a name it
also answers to: `line:hook`), `alt` (a word's spoken form), `value`
(strength), `bbox` ([x, y, w, h]).
