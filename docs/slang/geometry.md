# Slang geometry

A project's Slang (`shaders/*.slang`) can generate meshes and move the
points of any mesh. The functions are built from the project's modules
twice: into native code for the CPU, and into a compute kernel for the
GPU. The CPU renderer (`--cpu`) takes the mesh from the CPU. The GPU
renderer and the path tracer generate a mesh on the GPU, in its own
memory, when the layer gives `bounds`, and move a mesh's points there
when it has a deformer (see
[Where a mesh is generated](#where-a-mesh-is-generated)); the triangles
are the same either way.

Two forms:

- a **generator**: a mesh layer whose `mesh` is `{"type": "slang", ...}`;
  the author's functions give every point and triangle;
- a **deformer**: a mesh layer's `deform`, moving every point of its mesh
  (any type: sphere, plane, extrude, glTF, a generator).

Space is the layer's, in px: x right, **y down** (negative y is up), z away
from the viewer, centred on the layer's position.

## Generator

```json
{"id": "terrain", "kind": "mesh",
 "mesh": {"type": "slang", "function": "terrain", "vertices": 10201, "triangles": 20000,
          "params": {"size": 900, "height": 60, "cells": 100}, "uvs": true},
 "material": {"preset": "matte", "color": "#7f9a64"},
 "transform": {"position": [640, 560], "z": 250}}
```

| field | meaning |
|---|---|
| `function` | name prefix of the author's functions |
| `vertices`, `triangles` | how many points and triangles (1–2,000,000 and 1–4,000,000). Animatable (rounded): key them, and a mesh that grows over time is asked only for the points and triangles it has so far; 0 at a moment is an empty mesh |
| `params` | `{name: number or animatable property}`: fields of the author's `<function>_params` struct, passed as a last argument. Fields not given keep the struct's defaults; a function whose module declares the struct is always passed it |
| `animated` | `t` is the layer's local seconds (since its start); default false: `t` is 0 and the mesh is made once |
| `normals` | the author wrote `<function>_normal`; default: smooth normals from the triangles |
| `uvs` | the author wrote `<function>_uv`; default: planar UVs by the facing axis |
| `segments` | the author wrote `<function>_segment`: per triangle, which of the layer's `colors` it takes |
| `one_sided` | the mesh is closed: back faces are culled (a triangle's outside is where its normals point); default: seen from both sides |
| `bounds` | `[[x, y, z], [x, y, z]]` local box in px that holds every point, at every time; default: measured from the generated mesh. With it the GPU renderer generates the mesh itself, and planar UVs spread over this box |

The functions (`i` a point, `j` a triangle; where the module declares
`struct <function>_params`, every function takes `<function>_params p` as
a last argument):

```slang
float3 terrain_position(uint i, float t);   // required
uint3  terrain_triangle(uint j, float t);   // required: three point indices
float3 terrain_normal(uint i, float t);     // with "normals": true
float2 terrain_uv(uint i, float t);         // with "uvs": true
uint   terrain_segment(uint j, float t);    // with "segments": true
```

Every triangle is part `front` (so `parts.front` styles it) and every
edge is real for wireframes.

### Example: terrain (static, params)

```slang
struct terrain_params { float size = 900; float height = 70; float cells = 100; }

float terrain_height(float x, float z, terrain_params p) {
    float h = sin(x * 0.011) * cos(z * 0.013) + 0.45 * sin(x * 0.031 + z * 0.027);
    return -p.height * h;   // y down: negative raises
}

float3 terrain_position(uint i, float t, terrain_params p) {
    uint n = uint(p.cells) + 1;
    float x = (float(i % n) / p.cells - 0.5) * p.size;
    float z = (float(i / n) / p.cells - 0.5) * p.size;
    return float3(x, terrain_height(x, z, p), z);
}

float2 terrain_uv(uint i, float t, terrain_params p) {
    uint n = uint(p.cells) + 1;
    return float2(float(i % n), float(i / n)) / p.cells;
}

uint3 terrain_triangle(uint j, float t, terrain_params p) {
    uint c = uint(p.cells), n = c + 1, q = j / 2;
    uint a = (q / c) * n + q % c;
    return (j % 2 == 0) ? uint3(a, a + n, a + 1) : uint3(a + 1, a + n, a + n + 1);
}
```

`cells` 100 gives `vertices` 101² = 10201 and `triangles` 2 × 100² = 20000:
the counts in the JSON must match what the functions index. Unset struct
fields keep their defaults (`= 900`), so `params` may name only some.

### Example: particles as geometry (animated, coloured)

One small octahedron per particle (6 points, 8 triangles); 3000 particles
are `"vertices": 18000, "triangles": 24000`, with `"animated": true,
"normals": true, "segments": true, "one_sided": true` and four `colors`.

```slang
float hash(uint x) { x ^= x >> 16; x *= 0x7feb352du; x ^= x >> 15; x *= 0x846ca68bu; x ^= x >> 16; return float(x) / 4294967295.0; }

float3 swarm_centre(uint k, float t) {
    float r = 70.0 + 230.0 * sqrt(hash(k * 3u + 1u));
    float a = hash(k) * 6.2831853 + t * 60.0 / r;      // orbits, faster inside
    return float3(cos(a) * r, (hash(k * 5u + 2u) - 0.5) * 40.0, sin(a) * r);
}
float3 corner(uint c) {                                 // ±x, ±y, ±z
    float s = (c & 1u) == 0u ? 1.0 : -1.0; uint axis = c / 2u;
    return float3(axis == 0u ? s : 0.0, axis == 1u ? s : 0.0, axis == 2u ? s : 0.0);
}
float3 swarm_position(uint i, float t) { return swarm_centre(i / 6u, t) + corner(i % 6u) * 5.0; }
float3 swarm_normal(uint i, float t)   { return corner(i % 6u); }
uint3  swarm_triangle(uint j, float t) {
    uint base = j / 8u * 6u, f = j % 8u;
    return uint3(base + (f & 1u), base + 2u + ((f >> 1) & 1u), base + 4u + ((f >> 2) & 1u));
}
uint   swarm_segment(uint j, float t)  { return (j / 8u) % 4u; }
```

## Deformer

```json
{"id": "blob", "kind": "mesh", "mesh": {"type": "sphere", "radius": 110},
 "deform": {"function": "wave", "animated": true, "reach": 14}}
```

```slang
// Where point p (normal n) goes at local time t.
float3 wave(float3 p, float3 n, float t) {
    return p + n * 14.0 * sin(p.y * 0.07 - t * 5.0);
}
```

| field | meaning |
|---|---|
| `function` | `float3 f(float3 p, float3 n, float t)` (with `params`: `, f_params prm` last) |
| `params` | as for generators: fields of `<function>_params` |
| `animated` | `t` is the layer's local seconds; default false (`t` = 0, made once) |
| `subdivide` | 0–4: split every triangle into four this many times first (stops near 500k triangles), so boxes and planes bend |
| `reach` | px the function may move a point: pads the layer's bounds for layout checks and occlusion |

The deformer runs last: after the mesh is generated or loaded and after
`displace`. Copies of one point (hard edges, UV seams) get the same `n`
(their average), so they move together and the mesh does not crack.
Normals are then recomputed from the moved faces; UVs are kept.

## Where a mesh is generated

On the CPU, Lahza calls the functions on every core, checks what they
return, and hands the mesh to whoever draws it. The GPU renderer then
uploads it: 32 bytes a point and 16 a triangle, every frame for an
`animated` mesh.

The GPU renderer skips all of that for a generator whose layer gives
`bounds`. It runs the functions as a compute kernel, one thread per point
and per triangle, and they write the mesh where the GPU draws it from. A
frame of animated geometry costs the CPU a few numbers (the counts, `t`,
the params), however many points the mesh has. The GPU also makes the
smooth normals and planar UVs of a mesh whose author gives none.

`bounds` is what makes this possible: the CPU no longer sees the points,
and it still has to know where the mesh is (which part of the frame it
covers, what it may hide, how far its shadow reaches). So with `bounds`:

- The box must hold every point at every time. A point outside it may be
  cut off, because the renderer draws the mesh only where the box is on
  screen.
- Planar UVs (a mesh without `uvs`) spread over the box, not over the
  points' own extent at the moment, on every renderer. A picture mapped
  on an animated mesh stays in place while the points move.

A deformer runs on the GPU too. The mesh before the move (generated or
loaded, displaced, subdivided) is made on the CPU once and kept on the
GPU with the normals its points move along; each frame the deformer runs
there as a compute kernel, one thread per point, and the GPU shades the
moved mesh from its faces. The CPU still runs the deformer over the
points each frame, for their box alone: it places and culls the mesh and
frames its shadow as the CPU's own mesh would, and a point moved to a
value that is not a number has the mesh refused, as there.

A generated mesh with a deformer is the GPU's all the way where the
layer gives `bounds`: generated there, subdivided there as the deformer
asks, readied for it there (copies of one point found and given their
shared normal), and moved. The CPU has none of its points, so the layer's
`bounds`, padded by `reach`, stand for the moved ones: `reach` must be as
far as the function moves any point, or the mesh may be cut off. (The
CPU cannot have the moved points' own box before the GPU has made them.)
A point moved to a value that is not a number has the mesh refused there
too: it is not drawn, and the render says so once the frame is back.

The path tracer takes such meshes from the GPU's memory as they are
(their acceleration structures are built there), so a traced frame of
animated geometry costs the CPU no more than a rasterised one.

The CPU still makes the mesh when:

- a generator's layer gives no `bounds`, or has a `displace`: how far
  displacement subdivides a mesh follows from the length of its edges,
  which the CPU would not have, and it must say how many points the mesh
  has before the GPU makes them (a deformer then moves, on the GPU, the
  mesh the CPU generated);
- the function has more than 32 `params`;
- the frame is rendered with `--cpu`;
- the frame is rasterised on a GPU with no hardware visibility pass (the
  renderer then draws meshes from display lists the CPU builds).

`LAHZA_SLANG_MESH_CPU=1` makes the CPU generate every mesh, to compare.
`--profile` reports how many meshes the GPU generated and how many were
uploaded.

A mesh the GPU generates smooths its normals in f32 and in no fixed
order, where the CPU sums in f64: normals without `normals: true` can
differ in their last bits between the two and from run to run. Pictures
differ by less than one 8-bit step.

## Time and caching

A mesh is cached by its parameters: the built library (editing a shader
makes new meshes), the counts, the `params` values and `t`. So:

- **static** (`animated` false, constant params): generated once (on the
  CPU and uploaded once, or once on the GPU), however long the layer
  plays or the camera moves;
- **animated params** (keyframed `params`): a new mesh only on frames where
  a value changes;
- **animated** (`animated` true): a new mesh every frame. The renderer
  keeps only the frame being drawn (CPU cache, GPU mesh pool, path
  tracer), so memory stays at one copy of the mesh.

Prefer params with keys over `animated` when the motion is a few values
changing; use `animated` for motion that only `t` can express.

## Errors

- A function the modules do not have is named with the layer that
  calls it: `no Slang function 'ring_position' in shaders/geo.slang
  (ring.mesh calls it)`. `params` given to a function without a
  `<function>_params` struct are an error on the layer.
- The Slang does not build (a wrong signature, a `params` field the
  struct does not have): `check` and the render list the compiler's
  errors as `shaders/<file>:<line>:<col>: <what>`, or, when the error is
  in the call Lahza generates, with that call:
  ``in Lahza's call `float3 v = quad_position(i, t, p);`: too many arguments``.
- A fixed `vertices` or `triangles` of 0 or past the limits, `subdivide`
  past 4: `check` errors. (Keyed counts are held to the limits.)
- A triangle naming a point past `vertices`, or a position, normal or UV
  that is not a number (NaN, infinity): the mesh is not drawn and the
  render warns, naming the function and index, e.g.
  `slang mesh 'terrain': terrain_triangle(19999) is [10200, 10301, 10201], past the 10201 vertices`.
- The same mistakes in a mesh the GPU generates are errors too, reported
  once the frame is back from the GPU, with counts. The rest of the mesh
  is drawn: points that are not numbers sit at the origin, and triangles
  naming points the mesh does not have are left out.
  `slang mesh 'terrain': 3 of its triangles name points it does not have (the first is terrain_triangle(19999), past the 10201 vertices); they are not drawn`.
- A generator whose GPU kernel does not build (Slang that the CPU target
  takes and WGSL does not, such as `double`) is generated on the CPU, and
  the render warns once.
