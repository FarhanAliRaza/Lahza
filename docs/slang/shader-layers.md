# Shader layers

A shader layer is a picture a Slang function paints, one call per pixel.
It composites like any layer: transform, opacity, blend, effects, mattes,
3D and comps all apply to it.

```json
{"id": "bg", "kind": "shader", "function": "plasma", "size": [1920, 1080],
 "transform": {"position": [960, 540]},
 "params": {"speed": {"keys": [{"at": 0, "v": 1}, {"at": 2, "v": 4}]}}}
```

| Field      | Meaning |
|------------|---------|
| `function` | A function in the project's `shaders/*.slang` modules. |
| `size`     | The picture's size in pixels, centred on the layer's position like an image. Animatable. Default: the frame's size. Layers sit at position [0, 0] by default, so a full-frame background also needs `"transform": {"position": [960, 540]}` (half the frame). |
| `params`   | Optional named arguments. Each one is a property, so you can key it or drive it with `add` tracks (including a Slang `add`). |

## The function

```slang
float4 name(float2 uv, float2 px, float t /*, params in name order */)
```

| Argument | Meaning |
|----------|---------|
| `uv` | Position in the picture, 0..1. (0, 0) is the top-left corner and y points down. Each call gets a pixel centre, so `uv` is never exactly 0 or 1. |
| `px` | The same position in layer pixels, 0..`size` (`px = uv * size`). The layer's size is `px / uv`. |
| `t`  | Seconds since the layer started (the layer's own clock, so `step_fps` and comp time offsets apply). |
| then the `params` | Passed in **name order (alphabetical)**, not the order you wrote them in. A number is a `float`. `[x, y]` is a `float2` and `[x, y, z]` is a `float3`. `[x, y, z, w]` or a colour (`"#ff8800"`, `"#ff880080"`) is a `float4`. A colour is passed as its 0..1 sRGB components. A keyed param takes its type from its first key's `v`. A param with only `add` tracks needs a `"base"`. You can pass at most 12 params. |

The function returns **straight (not premultiplied) colour** in the
display's 0..1 sRGB values, the same values a `"#rrggbb"` colour has:

- `float4(rgb, alpha)`: Lahza clamps the alpha to 0..1 and premultiplies.
- `float3`: the result is opaque.

Lahza does not clamp the colour channels; the output clamps them. A NaN
or infinite result paints a transparent pixel.

Lahza evaluates the function once per pixel of the picture. A picture
that is shown at its own size gets one pixel per canvas pixel. A scaled
layer gets about one pixel per canvas pixel at its centre, so a scaled-up
layer stays sharp. The limit is 4K-frame worth of pixels per layer:
past that, a zoomed-in picture is drawn coarser and stretched.

The same function runs on the GPU (compiled to WGSL) and on the CPU
renderer (`--cpu`, compiled to native code). Both use f32 maths, so the
results match closely, but not to the bit: `sin`, `pow` and similar
functions differ in their last bits.

Hash noise with integer maths, not with the common
`frac(sin(dot(p, k)) * 43758.5)` hash. GPUs compute `sin` of large
arguments with low precision, so that hash looks blocky on the GPU and
differs from the CPU result. This integer hash is exact on both:

```slang
float hash(float2 p) {
    uint2 q = uint2(int2(p) + 4096);
    uint h = (q.x * 1597334677u) ^ (q.y * 3812015801u);
    h = h * 747796405u + 2891336453u;
    h ^= h >> 16; h *= 2246822519u; h ^= h >> 13;
    return float(h & 0xffffffu) / 16777216.0;
}
```

## Examples

A plasma background whose speed you can key:

```slang
// shaders/plasma.slang
float4 plasma(float2 uv, float2 px, float t, float speed) {
    float s = t * speed;
    float2 size = px / uv;
    float v = sin(px.x * 0.012 + s) + sin(px.y * 0.017 - s * 1.3)
            + sin(length(px - size * 0.5) * 0.015 - s * 2.0);
    return float4(0.5 + 0.5 * cos(v * 1.6 + float3(0.0, 2.1, 4.2)), 1.0);
}
```

```json
{"id": "bg", "kind": "shader", "function": "plasma", "transform": {"position": [960, 540]},
 "params": {"speed": {"keys": [{"at": 0, "v": 1}, {"at": 2, "v": 4}]}}}
```

Soft rings with alpha, drawn over other layers with a blend mode:

```slang
// shaders/rings.slang
float4 rings(float2 uv, float2 px, float t, float count, float4 tint) {
    float r = length(uv - 0.5) * 2.0;
    float band = 0.5 + 0.5 * sin(r * count * 6.2831853 - t * 4.0);
    return float4(tint.rgb, tint.a * band * smoothstep(1.0, 0.85, r));
}
```

```json
{"id": "rings", "kind": "shader", "function": "rings", "size": [500, 500],
 "params": {"count": 6, "tint": "#ffcc66"},
 "blend": "screen", "transform": {"opacity": 0.85, "rotation": 15, "position": [1400, 540]}}
```

The params are passed in name order: `count`, then `tint`. That order
matches the function's argument order.

An opaque gradient (`float3`, no params):

```slang
float3 sky(float2 uv, float2 px, float t) {
    return lerp(float3(0.08, 0.16, 0.42), float3(0.85, 0.45, 0.30), uv.y);
}
```

## Errors

`check` reports these errors:

- An unknown function, with the layer: `bg: no Slang function 'plasm' in shaders/plasma.slang`.
- A compile error in a module, with its place:
  `Slang: shaders/plasma.slang:4:12: undefined identifier 'spd'`.
- Arguments that do not match the function: for example, a missing
  param, or params given in another order than name order. The compiler
  reports these errors against Lahza's generated caller (`lahza_cpu.slang`).
  To fix one, compare the function's arguments after `t` with the layer's
  params in name order.
- A missing `slangc`.

A layer whose function did not build is drawn transparent, and the error
says why.
