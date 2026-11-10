# Slang in Lahza

A project's `shaders/<name>.slang` files are [Slang](https://shader-slang.org)
modules an author or agent writes to drive the engine with code. Write them
with the `shader_write` tool (it compiles and returns errors as
`shaders/<file>:<line>:<col>: <what>`), check them with `shader_check`, and
`check` reports errors of every function the project uses.

Authors write plain functions. Lahza writes the entry points, compiles the
modules with `slangc` (on PATH, or `LAHZA_SLANGC`) for the CPU into one
native library and for the GPU to WGSL, and calls the same functions on
both renderers. Modules reach each other with `import <module>;`, and every
project can `import lahza;` (types and helpers: noise, hashes, bumps; see
Materials). Nothing falls back silently: a module that does not compile, an
unknown function or a missing `slangc` is an error.

Four uses:

| Use | Where | Author writes |
|---|---|---|
| Scene parameters | any animatable property's `add` list | `T f(float t)` |
| Shader layers | `{"kind": "shader", "function": …}` | `float4 f(float2 uv, float2 px, float t, …params)` |
| Geometry generators and deformers | `"mesh": {"type": "slang", …}`, `"deform": {…}` | `float3 f_position(uint i, float t)`, `uint3 f_triangle(uint j, float t)`, … |
| Materials | `"material": {"slang": "f"}` | `Surface f(SurfaceInput s)` |

## Scene parameters

Any animatable property takes a Slang track in its `add` list:

```json
"position": {"base": [960, 540], "add": [{"type": "slang", "function": "orbit"}]},
"scale": {"base": [1, 1], "add": [{"type": "slang", "function": "breathe", "mode": "mul"}]}
```

```slang
float2 orbit(float t)   { float a = t * 1.5707963; return float2(cos(a), sin(a)) * 120.0; }
float  breathe(float t) { return 1.0 + 0.25 * sin(t * 6.2831853); }
```

- `t` is local seconds since `from` (default: the layer start); `until`
  ends the track (after it the track adds nothing). Before `from` the
  track holds its first value, `f(0)`: a function that starts away from
  rest does not pop in when the track starts, and a cut that lands a
  fraction of a frame before the marker a track starts on already shows
  where the track starts.
- The function returns `float` … `float4`, one component per value
  component. A `float` applies to every component; missing components leave
  the value as it is (0 added, 1 multiplied).
- `mode`: `add` (default) adds to the keyed value; `mul` multiplies it.
- Works on the camera (position, zoom, rotation, fov, orbit, dolly, shift),
  transforms, lights, materials and every other keyable value, and runs
  identically for GPU and CPU renders.

The other three uses follow.
