// Independent tiled inference for the MIT-licensed Anime4K CNN x2 S model.
// Coefficients and attribution are in assets/upscaler; no vendor screen model.
struct UpscaleParams {
    sizes: vec4<u32>, // source width/height, output width/height
    region: vec4<u32>,
    tile: vec4<i32>, // scratch origin in source pixels, core width/height
    tile_output: vec4<u32>, // core origin in output-source region, scratch stride
    original: vec4<u32>, // original width/height, original region origin
    mapping: vec4<u32>, // original scale, residual strength in thousandths
}
var<storage, read> b_up_params: UpscaleParams;
var<storage, read> b_up_source: array<u32>;
var<storage, read> b_up_features: array<vec4<f32>>;
var<storage, read_write> b_up_result: array<vec4<f32>>;
var<storage, read> b_up_weights: array<vec4<f32>>;
var<storage, read> b_up_alpha: array<u32>;
var<storage, read_write> b_up_output: array<u32>;

fn unpack_rgba(v: u32) -> vec4<f32> {
    return vec4<f32>(vec4<u32>(v, v >> 8u, v >> 16u, v >> 24u) & vec4<u32>(255u)) / 255.0;
}
fn read_source(p: vec2<i32>) -> vec4<f32> {
    let q = vec2<u32>(clamp(p, vec2<i32>(0), vec2<i32>(b_up_params.sizes.xy) - vec2<i32>(1)));
    return unpack_rgba(b_up_source[q.y * b_up_params.sizes.x + q.x]);
}
fn read_features(p: vec2<i32>) -> vec4<f32> {
    // Clamp at the real image boundary at every convolution, not the tile edge.
    let world = clamp(p, vec2<i32>(0), vec2<i32>(b_up_params.sizes.xy) - vec2<i32>(1));
    let local = vec2<u32>(clamp(world - b_up_params.tile.xy, vec2<i32>(0), vec2<i32>(i32(b_up_params.tile_output.z) - 1)));
    return b_up_features[local.y * b_up_params.tile_output.z + local.x];
}
fn convolve(layer: u32, id: vec3<u32>) {
    let stride = b_up_params.tile_output.z;
    if id.x >= stride || id.y >= stride { return; }
    let p = clamp(b_up_params.tile.xy + vec2<i32>(id.xy), vec2<i32>(0), vec2<i32>(b_up_params.sizes.xy) - vec2<i32>(1));
    var result = vec4<f32>(0.0);
    let signs = select(2u, 1u, layer == 0u);
    for (var sign = 0u; sign < signs; sign++) {
        for (var tap = 0u; tap < 9u; tap++) {
            let offset = vec2<i32>(i32(tap / 3u) - 1, i32(tap % 3u) - 1);
            var value: vec4<f32>;
            if layer == 0u { value = read_source(p + offset); }
            else {
                value = read_features(p + offset);
                if sign == 1u { value = -value; }
                value = max(value, vec4<f32>(0.0));
            }
            let base = layer * 73u + (sign * 9u + tap) * 4u;
            let weights = mat4x4<f32>(b_up_weights[base], b_up_weights[base + 1u], b_up_weights[base + 2u], b_up_weights[base + 3u]);
            result += weights * value;
        }
    }
    b_up_result[id.y * stride + id.x] = result + b_up_weights[layer * 73u + 72u];
}
@compute @workgroup_size(8, 8) fn conv0(@builtin(global_invocation_id) id: vec3<u32>) { convolve(0u, id); }
@compute @workgroup_size(8, 8) fn conv1(@builtin(global_invocation_id) id: vec3<u32>) { convolve(1u, id); }
@compute @workgroup_size(8, 8) fn conv2(@builtin(global_invocation_id) id: vec3<u32>) { convolve(2u, id); }
@compute @workgroup_size(8, 8) fn conv3(@builtin(global_invocation_id) id: vec3<u32>) { convolve(3u, id); }

fn base_sample(p: vec2<f32>) -> vec4<f32> {
    let lo = vec2<i32>(floor(p)); let f = fract(p);
    return mix(mix(read_source(lo), read_source(lo + vec2<i32>(1, 0)), f.x),
        mix(read_source(lo + vec2<i32>(0, 1)), read_source(lo + vec2<i32>(1, 1)), f.x), f.y);
}
fn original_alpha(p: vec2<i32>) -> f32 {
    let q = vec2<u32>(clamp(p, vec2<i32>(0), vec2<i32>(b_up_params.original.xy) - vec2<i32>(1)));
    return f32(b_up_alpha[q.y * b_up_params.original.x + q.x] >> 24u) / 255.0;
}
@compute @workgroup_size(8, 8)
fn reconstruct(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u32(b_up_params.tile.z) * 2u || id.y >= u32(b_up_params.tile.w) * 2u { return; }
    let output = b_up_params.tile_output.xy * 2u + id.xy;
    let p = vec2<f32>(b_up_params.region.xy) + (vec2<f32>(output) + vec2<f32>(0.5)) * 0.5 - vec2<f32>(0.5);
    let local = id.xy / 2u + vec2<u32>(4u);
    let residual = b_up_features[local.y * b_up_params.tile_output.z + local.x][(id.y % 2u) * 2u + id.x % 2u];
    let rgb = base_sample(p).rgb + vec3<f32>(residual * f32(b_up_params.mapping.y) / 1000.0);
    // The neural luma residual never changes transparency, even in two-pass 4x.
    let alpha_p = vec2<f32>(b_up_params.original.zw) + (vec2<f32>(output) + vec2<f32>(0.5)) / f32(b_up_params.mapping.x) - vec2<f32>(0.5);
    let lo = vec2<i32>(floor(alpha_p)); let f = fract(alpha_p);
    let alpha = mix(mix(original_alpha(lo), original_alpha(lo + vec2<i32>(1, 0)), f.x),
        mix(original_alpha(lo + vec2<i32>(0, 1)), original_alpha(lo + vec2<i32>(1, 1)), f.x), f.y);
    let bytes = vec4<u32>(floor(clamp(vec4<f32>(rgb, alpha), vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0 + vec4<f32>(0.5)));
    b_up_output[output.y * b_up_params.sizes.z + output.x] = bytes.x | (bytes.y << 8u) | (bytes.z << 16u) | (bytes.w << 24u);
}
