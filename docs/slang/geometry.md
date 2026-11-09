# Slang geometry

A project's Slang (`shaders/*.slang`) can generate meshes and move the
points of any mesh. The functions run on the CPU, in native code built
from the project's modules, so the GPU renderer, the CPU renderer
(`--cpu`) and the path tracer all draw the same triangles.

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
| `vertices`, `triangles` | how many points and triangles (1–2,000,000 and 1–4,000,000) |
| `params` | `{name: number or animatable property}`: fields of the author's `<function>_params` struct, passed as a last argument |
| `animated` | `t` is the layer's local seconds (since its start); default false: `t` is 0 and the mesh is made once |
| `normals` | the author wrote `<function>_normal`; default: smooth normals from the triangles |
| `uvs` | the author wrote `<function>_uv`; default: planar UVs by the facing axis |
| `segments` | the author wrote `<function>_segment`: per triangle, which of the layer's `colors` it takes |
| `one_sided` | the mesh is closed: back faces are culled (a triangle's outside is where its normals point); default: seen from both sides |
| `bounds` | `[[x, y, z], [x, y, z]]` local box in px; default: measured from the generated mesh |

The functions (`i` a point, `j` a triangle; with `params`, every function
takes `<function>_params p` as a last argument):

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

## Time and caching

A mesh is cached by its parameters: the built library (editing a shader
makes new meshes), the counts, the `params` values and `t`. So:

- **static** (`animated` false, constant params): generated once, uploaded
  to the GPU once, however long the layer plays or the camera moves;
- **animated params** (keyframed `params`): a new mesh only on frames where
  a value changes;
- **animated** (`animated` true): a new mesh every frame. The renderer
  keeps only the frame being drawn (CPU cache, GPU mesh pool, path
  tracer), so memory stays at one copy of the mesh.

Prefer params with keys over `animated` when the motion is a few values
changing; use `animated` for motion that only `t` can express.

## Errors

- The Slang does not build (a missing function, a wrong signature, a
  `params` field the struct does not have): `check` and the render list
  the compiler's errors as `shaders/<file>:<line>:<col>: <what>`.
- `vertices` or `triangles` 0 or past the limits, `subdivide` past 4:
  `check` errors.
- A triangle naming a point past `vertices`, or a position, normal or UV
  that is not a number (NaN, infinity): the mesh is not drawn and the
  render warns, naming the function and index, e.g.
  `slang mesh 'terrain': terrain_triangle(19999) is [10200, 10301, 10201], past the 10201 vertices`.
