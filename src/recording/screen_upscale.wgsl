alias Math = f32;
// Independent tiled inference for the Lahza UI CNN v2 models.
// Training data, coefficient layout and provenance: assets/upscaler.
struct UpscaleParams {
    sizes: vec4<u32>, // source width/height, output width/height
    region: vec4<u32>,
    tile: vec4<i32>, // scratch origin in source pixels, core width/height
    tile_output: vec4<u32>, // core origin in region, scratch stride, scale
    layer: vec4<u32>, // weight offset in vec4s, input groups, output groups, kind
}
// Must match HALO in screen_upscale.rs.
const HALO: u32 = 8u;
const HEAD: u32 = 0u;
const BODY: u32 = 1u;
const TAIL: u32 = 2u;
var<storage, read> b_up_params: UpscaleParams;
var<storage, read> b_up_source: array<u32>;
var<storage, read> b_up_features: array<vec4<f32>>;
var<storage, read> b_up_skip: array<vec4<f32>>;
var<storage, read_write> b_up_result: array<vec4<f32>>;
var<storage, read> b_up_weights: array<vec4<f32>>;
var<storage, read_write> b_up_output: array<u32>;

fn unpack_rgba(v: u32) -> vec4<f32> {
    return vec4<f32>(vec4<u32>(v, v >> 8u, v >> 16u, v >> 24u) & vec4<u32>(255u)) / 255.0;
}
fn read_source(p: vec2<i32>) -> vec4<f32> {
    let q = vec2<u32>(clamp(p, vec2<i32>(0), vec2<i32>(b_up_params.sizes.xy) - vec2<i32>(1)));
    return unpack_rgba(b_up_source[q.y * b_up_params.sizes.x + q.x]);
}
// Clamp at the real image boundary at every convolution, not the tile edge.
fn feature_index(p: vec2<i32>, group: u32, groups: u32) -> u32 {
    let world = clamp(p, vec2<i32>(0), vec2<i32>(b_up_params.sizes.xy) - vec2<i32>(1));
    let local = vec2<u32>(clamp(world - b_up_params.tile.xy, vec2<i32>(0), vec2<i32>(i32(b_up_params.tile_output.z) - 1)));
    return (local.y * b_up_params.tile_output.z + local.x) * groups + group;
}

// Largest output group count of any layer (the 4× tail: 48 channels).
const MAX_GROUPS: u32 = 12u;
// One invocation computes every output group of a pixel, so each input is
// read once per tap. Constant loop bounds keep the accumulators in registers.
@compute @workgroup_size(8, 8, 1)
fn conv(@builtin(global_invocation_id) id: vec3<u32>) {
    let stride = b_up_params.tile_output.z;
    let layer = b_up_params.layer;
    let inputs = layer.y;
    let outputs = layer.z;
    let kind = layer.w;
    if id.x >= stride || id.y >= stride { return; }
    let p = clamp(b_up_params.tile.xy + vec2<i32>(id.xy), vec2<i32>(0), vec2<i32>(b_up_params.sizes.xy) - vec2<i32>(1));
    let biases = layer.x + outputs * 9u * inputs * 4u;
    var result: array<vec4<f32>, MAX_GROUPS>;
    for (var group = 0u; group < MAX_GROUPS; group++) {
        if group < outputs { result[group] = b_up_weights[biases + group]; }
    }
    for (var tap = 0u; tap < 9u; tap++) {
        let q = p + vec2<i32>(i32(tap % 3u) - 1, i32(tap / 3u) - 1);
        for (var input = 0u; input < inputs; input++) {
            var value: vec4<f32>;
            if kind == HEAD {
                value = vec4<f32>(read_source(q).rgb, 0.0);
            } else {
                let index = feature_index(q, input, inputs);
                value = b_up_features[index];
                // The tail sees the long skip from the head features.
                if kind == TAIL { value += b_up_skip[index]; }
            }
            let half = vec4<Math>(value);
            for (var group = 0u; group < MAX_GROUPS; group++) {
                if group >= outputs { break; }
                let base = layer.x + ((group * 9u + tap) * inputs + input) * 4u;
                // Preserve source and reconstruction precision; hidden
                // convolutions use native half products with f32 accumulation.
                if kind == BODY {
                    let w = mat4x4<Math>(vec4<Math>(b_up_weights[base]), vec4<Math>(b_up_weights[base + 1u]),
                        vec4<Math>(b_up_weights[base + 2u]), vec4<Math>(b_up_weights[base + 3u]));
                    result[group] += vec4<f32>(w[0] * half.x) + vec4<f32>(w[1] * half.y)
                        + vec4<f32>(w[2] * half.z) + vec4<f32>(w[3] * half.w);
                } else {
                    let w = mat4x4<f32>(b_up_weights[base], b_up_weights[base + 1u],
                        b_up_weights[base + 2u], b_up_weights[base + 3u]);
                    result[group] += w * value;
                }
            }
        }
    }
    let out = (id.y * stride + id.x) * outputs;
    for (var group = 0u; group < MAX_GROUPS; group++) {
        if group >= outputs { break; }
        b_up_result[out + group] = select(max(result[group], vec4<f32>(0.0)), result[group], kind == TAIL);
    }
}

fn base_sample(p: vec2<f32>) -> vec4<f32> {
    let lo = vec2<i32>(floor(p)); let f = fract(p);
    return mix(mix(read_source(lo), read_source(lo + vec2<i32>(1, 0)), f.x),
        mix(read_source(lo + vec2<i32>(0, 1)), read_source(lo + vec2<i32>(1, 1)), f.x), f.y);
}
fn residual(pixel: u32, channel: u32) -> f32 {
    return b_up_features[pixel + channel / 4u][channel % 4u];
}
@compute @workgroup_size(8, 8, 1)
fn reconstruct(@builtin(global_invocation_id) id: vec3<u32>) {
    let scale = b_up_params.tile_output.w;
    if id.x >= u32(b_up_params.tile.z) * scale || id.y >= u32(b_up_params.tile.w) * scale { return; }
    let output = b_up_params.tile_output.xy * scale + id.xy;
    let p = vec2<f32>(b_up_params.region.xy) + (vec2<f32>(output) + vec2<f32>(0.5)) / f32(scale) - vec2<f32>(0.5);
    let local = id.xy / scale + vec2<u32>(HALO);
    let pixel = (local.y * b_up_params.tile_output.z + local.x) * b_up_params.layer.z;
    // Pixel-shuffle order: channel c of sub-pixel (x, y) is c·s² + y·s + x.
    let sub = (id.y % scale) * scale + id.x % scale;
    let area = scale * scale;
    let base = base_sample(p);
    // The learned RGB residual never changes transparency.
    let rgb = base.rgb + vec3<f32>(residual(pixel, sub), residual(pixel, area + sub), residual(pixel, 2u * area + sub));
    let bytes = vec4<u32>(floor(clamp(vec4<f32>(rgb, base.a), vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0 + vec4<f32>(0.5)));
    b_up_output[output.y * b_up_params.sizes.z + output.x] = bytes.x | (bytes.y << 8u) | (bytes.z << 16u) | (bytes.w << 24u);
}
