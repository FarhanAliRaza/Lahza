# Slang materials

A mesh material can name a Slang function from the project's `shaders/`
that decides, at every point of the surface, its base colour, roughness,
metallic, emission, opacity and normal. Lahza then lights the result as it
lights any material (GGX highlights, clear coat, sheen, shadows, the
environment, tone mapping). The same function runs in GPU renders,
rasterised or path-traced (`--pt`, at every surface a path hits), and in
CPU (`--cpu`) renders.

## The material field

```json
"material": {
  "preset": "glossy",
  "slang": {"function": "marble", "params": [70, 4]}
}
```

- `function`: a function of a module in `shaders/` with the signature
  `Surface name(SurfaceInput s)`. `"slang": "marble"` is short for
  `{"function": "marble"}`.
- `params`: up to 8 numbers, each an animatable property (a constant,
  `{"keys": [...]}`, or anything else a number takes). The function reads
  them as `s.params[0]` … `s.params[7]`; params that are not given read 0.
- `see_through` (default false): the function's opacity lets other meshes
  behind show. The mesh is then drawn after the opaque meshes (as glass
  is). Without it, opacity below 1 only blends with what is behind the
  layer list (the background and 2D layers), not with other meshes.

The rest of the material still applies and comes in as `s.material`: the
preset, `color`, `roughness`, `metallic`, `emissive`, maps (`map`,
`normal_map`, `bump_map` …) and `noise` are worked out first. Clear coat,
sheen, specular, anisotropy, transmission and the other lobes stay as the
material sets them. Parts (`parts.front` …) can name different functions.

## The contract: `import lahza;`

Every project can import Lahza's built-in module `lahza` (do not name a
module of your own `lahza`):

```slang
struct Surface {
    float3 color;      // base colour, linear (not sRGB)
    float roughness;   // 0 mirror-sharp … 1 fully diffuse (clamped to 0.03 … 1)
    float metallic;    // 0 dielectric … 1 metal
    float3 emissive;   // light given off, linear, added to the lit colour
    float opacity;     // 0 … 1
    float3 normal;     // shading normal, object space, unit
};

struct SurfaceInput {
    float3 position;     // object space, px: sticks to the mesh as it moves
    float3 world;        // world space, px
    float3 normal;       // shading normal, object space, unit, facing the camera
    float3 view;         // direction toward the camera, object space, unit
    float3 world_normal; // the normal in world space (up is -y)
    float2 uv;           // the mesh's UVs, before the material's "uv" tiling
    float time;          // seconds since the layer started
    float params[8];     // the material's "params"
    Surface material;    // the material's own values here
};
```

Start from `s.material` and change what you need; whatever you leave keeps
the material's value. Object space is the mesh's own: a sphere of radius
190 spans -190 … 190 on each axis, centred on the layer's position, and
`position` stays put on the surface when the layer moves, turns or scales.
Object space y points down the screen, like the canvas.

Helpers in `lahza` (the same results on every renderer):

- `float hash(int3 cell)`: 0 … 1 per integer cell.
- `float value_noise(float3 p)`: smooth noise 0 … 1, one feature per unit.
- `float fbm(float3 p, int octaves)`: fractal noise 0 … 1.
- `float3 bump(SurfaceInput s, float3 gradient, float strength)`: the
  normal of a raised surface, from a height's change per px along object
  space (finite differences of a height function of `s.position`) and a
  strength (the height's scale in px).

Functions may call any other function of the project's modules. They
cannot use textures, buffers or other resources.

## Examples

`shaders/materials.slang`:

```slang
import lahza;

// Marble: dark veins through pale stone, warped by fractal noise.
// params: [feature size px, vein sharpness]
Surface marble(SurfaceInput s) {
    Surface o = s.material;
    float size = max(s.params[0], 1.0);
    float3 q = s.position / size;
    float warp = fbm(q, 5) * 7.0;
    float v = pow(0.5 + 0.5 * sin(q.x * 2.0 + q.y * 1.2 + warp), max(s.params[1], 1.0));
    o.color = lerp(float3(0.88, 0.86, 0.82), float3(0.10, 0.12, 0.16), v);
    o.roughness = lerp(0.12, 0.4, v);
    return o;
}

// Wood: growth rings around the mesh's y axis with a fine grain.
// params: [ring spacing px]
Surface wood(SurfaceInput s) {
    Surface o = s.material;
    float3 p = s.position;
    float r = length(p.xz) / max(s.params[0], 1.0) + fbm(p * float3(0.015, 0.003, 0.015), 4) * 2.5;
    float late = smoothstep(0.55, 0.95, frac(r));
    float grain = value_noise(p * float3(0.35, 0.015, 0.35));
    o.color = lerp(float3(0.62, 0.38, 0.18), float3(0.26, 0.13, 0.05), late * 0.9 + grain * 0.1);
    o.roughness = 0.45 + 0.25 * late;
    return o;
}

// Pulse: bands of light running up the mesh over time.
// params: [speed (radians per second), glow strength]
Surface pulse(SurfaceInput s) {
    Surface o = s.material;
    float band = 0.5 + 0.5 * sin(s.position.y * 0.09 - s.time * s.params[0]);
    o.emissive = float3(0.15, 0.85, 1.0) * pow(band, 10.0) * s.params[1];
    o.color = float3(0.03, 0.05, 0.07);
    o.metallic = 0.6;
    o.roughness = 0.3;
    return o;
}

// Hammered metal: dents from a noise height field (the normal only).
// params: [dent size px, depth px]
float dents(float3 p, float size) { return fbm(p / size, 3); }

Surface hammered(SurfaceInput s) {
    Surface o = s.material;
    float size = max(s.params[0], 1.0);
    float e = size * 0.05;
    float h = dents(s.position, size);
    float3 g = float3(dents(s.position + float3(e, 0, 0), size) - h,
                      dents(s.position + float3(0, e, 0), size) - h,
                      dents(s.position + float3(0, 0, e), size) - h) / e;
    o.normal = bump(s, g, s.params[1]);
    return o;
}
```

Layers using them:

```json
{"id": "bust", "kind": "mesh", "mesh": {"type": "sphere", "radius": 190},
 "material": {"preset": "glossy", "slang": {"function": "marble", "params": [70, 4]}}}

{"id": "ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 150, "tube": 55},
 "material": {"preset": "metal", "slang": {"function": "pulse", "params": [4, {"keys": [{"at": 0, "v": 0}, {"at": 1, "v": 3}]}]}}}

{"id": "bowl", "kind": "mesh", "mesh": {"type": "sphere", "radius": 170},
 "material": {"preset": "chrome", "color": "#d9b77a", "roughness": 0.18,
              "slang": {"function": "hammered", "params": [26, 14]}}}
```

## Errors

Nothing fails silently. `check` (and every render) reports:

- the compiler's errors as `shaders/<file>:<line>:<col>: <what>`, an
  unknown function among them;
- a function name that is not a Slang identifier, more than 8 params, a
  material that is also a `volume`, a module named `lahza`;
- `slangc` missing from PATH (or `LAHZA_SLANGC`).

A GPU render whose project's functions do not compile to WGSL fails with
that error (an automatic render then continues on the CPU and says so).

## Limits

- Path-traced subsurface scattering leaves the body with the material's
  own normal, not the function's.
- Wireframe materials and volumes ignore (volumes reject) a Slang
  function.
- Shadows are cast by the mesh's shape; a function's opacity does not cut
  holes in shadows.
- Changing a function rebuilds the project's library (cached by content)
  and, on the GPU, compiles a raster shader variant once (a second or two),
  and a path tracer variant once per traced scene's features.
