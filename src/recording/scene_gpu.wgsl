// Byte-domain math deliberately matches the CPU renderer (no sRGB conversion).
struct Draw {
    kind: vec4<f32>, rect: vec4<f32>, anchor: vec4<f32>,
    smear: vec4<f32>, color: vec4<f32>, bounds: vec4<f32>,
}
struct Params {
    canvas: vec4<u32>, images: array<vec4<u32>, 8>, inverse: array<vec4<f32>, 3>,
    viewport: vec4<f32>, sample_bounds: vec4<f32>, geometry: vec4<f32>,
    projection: vec4<f32>, media_bounds: vec4<u32>, camera_rect: vec4<f32>, camera_info: vec4<f32>,
    draws: array<Draw, 4>,
}
var<storage, read> b_params: Params;
var<storage, read> b_background: array<u32>;
var<storage, read> b_source: array<u32>;
var<storage, read> b_overlay: array<u32>;
var<storage, read> b_camera: array<u32>;
var<storage, read> b_cursor: array<u32>;
var<storage, read> b_shadow: array<f32>;
var<storage, read> b_caption: array<u32>;
var<storage, read> b_watermark: array<u32>;
var<storage, read_write> b_output: array<u32>;

fn unpack(pixel: u32) -> vec4<f32> {
    return vec4<f32>(vec4<u32>(pixel, pixel >> 8u, pixel >> 16u, pixel >> 24u) & vec4<u32>(255u));
}
fn quantize(v: vec4<f32>) -> vec4<f32> { return floor(clamp(v, vec4<f32>(0.0), vec4<f32>(255.0)) + vec4<f32>(0.5)); }
fn pack(pixel: vec4<f32>) -> u32 {
    let p = vec4<u32>(quantize(pixel));
    return p.x | (p.y << 8u) | (p.z << 16u) | (p.w << 24u);
}
fn pixel(layer: u32, index: u32) -> vec4<f32> {
    switch layer {
        case 1u: { return unpack(b_source[index]); }
        case 2u: { return unpack(b_overlay[index]); }
        case 3u: { return unpack(b_camera[index]); }
        case 4u: { return unpack(b_cursor[index]); }
        default: { return vec4<f32>(0.0); }
    }
}
fn sample_image(layer: u32, xy: vec2<f32>, fixed: bool) -> vec4<f32> {
    let size = b_params.images[layer].xy;
    let p = clamp(xy, vec2<f32>(0.0), vec2<f32>(size - vec2<u32>(1u)));
    let lo = vec2<u32>(floor(p));
    let hi = min(lo + vec2<u32>(1u), size - vec2<u32>(1u));
    var f = p - vec2<f32>(lo);
    if fixed { f = floor(f * 256.0 + vec2<f32>(0.5)) / 256.0; }
    return quantize(mix(mix(pixel(layer, lo.y * size.x + lo.x), pixel(layer, lo.y * size.x + hi.x), f.x),
        mix(pixel(layer, hi.y * size.x + lo.x), pixel(layer, hi.y * size.x + hi.x), f.x), f.y));
}
fn over(dst: vec4<f32>, color: vec3<f32>, alpha: f32) -> vec4<f32> {
    return quantize(mix(dst, vec4<f32>(color, 255.0), clamp(alpha, 0.0, 1.0)));
}
fn unproject(p: vec2<f32>) -> vec2<f32> {
    let q = vec3<f32>(p, 1.0);
    var w = dot(b_params.inverse[2].xyz, q);
    if abs(w) < 1e-12 { w = 1e-12; }
    return vec2<f32>(dot(b_params.inverse[0].xyz, q), dot(b_params.inverse[1].xyz, q)) / w;
}
fn pixel_size(p: vec2<f32>, uv: vec2<f32>) -> f32 {
    if b_params.projection.x > 0.0 { return b_params.projection.x; }
    let dx = (unproject(p + vec2<f32>(1.0, 0.0)) - uv) * b_params.geometry.xy;
    let dy = (unproject(p + vec2<f32>(0.0, 1.0)) - uv) * b_params.geometry.xy;
    return max(sqrt(abs(dx.x * dy.y - dx.y * dy.x)), 1e-6);
}
fn rounded_distance(local: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let r = clamp(radius, 0.0, min(size.x, size.y) * 0.5);
    let d = abs(local - size * 0.5) - (size * 0.5 - vec2<f32>(r));
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}
fn media(dst: vec4<f32>, position: vec2<u32>, p: vec2<f32>) -> vec4<f32> {
    if b_params.images[1].z == 0u { return dst; }
    if any(position < b_params.media_bounds.xy) || any(position >= b_params.media_bounds.zw) { return dst; }
    let uv = unproject(p);
    if any(uv < vec2<f32>(-0.05)) || any(uv > vec2<f32>(1.05)) { return dst; }
    let size = b_params.geometry.xy;
    let title = b_params.geometry.w;
    let ps = pixel_size(p, uv);
    let surface_size = size + vec2<f32>(0.0, title);
    let distance = max(rounded_distance(uv * size + vec2<f32>(0.0, title), surface_size, b_params.geometry.z), -uv.y * size.y);
    let coverage = clamp(0.5 - distance / ps, 0.0, 1.0);
    if coverage <= 0.0 { return dst; }
    let band = b_params.geometry.z + 2.0 * ps + 1.0;
    let fixed = b_params.projection.z > 0.0 && uv.y * size.y > ps + 1.0
        && uv.y * size.y + title < surface_size.y - band && uv.x * size.x > band && uv.x * size.x < size.x - band;
    let source_uv = clamp(b_params.viewport.xy + clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * b_params.viewport.zw,
        b_params.sample_bounds.xy, b_params.sample_bounds.zw);
    var s = sample_image(1u, source_uv * vec2<f32>(b_params.images[1].xy) - vec2<f32>(0.5), fixed);
    if b_params.images[2].z != 0u {
        let annotation = sample_image(2u, source_uv * vec2<f32>(b_params.images[2].xy) - vec2<f32>(0.5), fixed);
        s = over(s, annotation.xyz, annotation.w / 255.0);
    }
    return over(dst, s.xyz, select(coverage, 1.0, fixed) * s.w / 255.0);
}
fn sprite_sample(xy: vec2<f32>) -> vec4<f32> {
    let base = floor(xy - vec2<f32>(0.5));
    let f = xy - vec2<f32>(0.5) - base;
    var sum = vec4<f32>(0.0);
    for (var y = 0u; y < 2u; y++) {
        for (var x = 0u; x < 2u; x++) {
            let p = base + vec2<f32>(f32(x), f32(y));
            if all(p >= vec2<f32>(0.0)) && all(p < vec2<f32>(b_params.images[4].xy)) {
                let weight = select(1.0 - f.x, f.x, x == 1u) * select(1.0 - f.y, f.y, y == 1u);
                sum += pixel(4u, u32(p.y) * b_params.images[4].x + u32(p.x)) * weight;
            }
        }
    }
    return sum;
}
fn pointer(dst: vec4<f32>, d: Draw, p: vec2<f32>) -> vec4<f32> {
    if d.kind.y <= 0.0 || any(p < d.bounds.xy) || any(p >= d.bounds.zw) { return dst; }
    if d.kind.x < 3.0 {
        if d.rect.z <= 0.0 { return dst; }
        let distance = length(p - d.rect.xy);
        var coverage = clamp(d.rect.z - distance + 0.5, 0.0, 1.0);
        if d.kind.x == 2.0 { coverage *= clamp(distance - max(d.rect.z - d.rect.w, 0.0) + 0.5, 0.0, 1.0); }
        return over(dst, d.color.xyz, coverage * d.kind.y);
    }
    let delta = p - d.rect.xy;
    let local = vec2<f32>(delta.x * d.anchor.w + delta.y * d.anchor.z, -delta.x * d.anchor.z + delta.y * d.anchor.w) + d.anchor.xy;
    var sum = vec4<f32>(0.0);
    let taps = u32(d.kind.w);
    for (var i = 0u; i < taps; i++) {
        let t = f32(i) / f32(max(taps - 1u, 1u));
        let q = local + d.smear.xy * t;
        if any(q < vec2<f32>(-1.0)) || any(q > d.rect.zw + vec2<f32>(1.0)) { continue; }
        sum += sprite_sample(q * vec2<f32>(b_params.images[4].xy) / d.rect.zw);
    }
    sum /= f32(taps);
    let coverage = sum.w / 255.0;
    if coverage <= 0.002 { return dst; }
    let color = select(quantize(sum / coverage).xyz, vec3<f32>(0.0), d.kind.z != 0.0);
    return over(dst, color, coverage * d.kind.y);
}
fn camera(dst: vec4<f32>, index: u32, p: vec2<f32>) -> vec4<f32> {
    if b_params.images[3].z == 0u { return dst; }
    var result = dst;
    if b_params.camera_info.z != 0.0 { result = over(result, vec3<f32>(0.0), b_shadow[index]); }
    let rect = b_params.camera_rect;
    if any(p < floor(rect.xy)) || any(p >= ceil(rect.xy + rect.zw)) { return result; }
    let local = p - rect.xy;
    let coverage = clamp(0.5 - rounded_distance(local, rect.zw, b_params.camera_info.x), 0.0, 1.0);
    if coverage <= 0.0 { return result; }
    let size = vec2<f32>(b_params.images[3].xy);
    let scale = max(rect.z / size.x, rect.w / size.y);
    let offset = (rect.zw - size * scale) * 0.5;
    var xy = (local - offset) / scale - vec2<f32>(0.5);
    if b_params.camera_info.y != 0.0 { xy.x = size.x - 1.0 - xy.x; }
    let s = sample_image(3u, xy, false);
    return over(result, s.xyz, coverage * s.w / 255.0);
}
@compute @workgroup_size(8, 8, 1)
fn compose(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= b_params.canvas.xy) { return; }
    let index = id.y * b_params.canvas.x + id.x;
    let p = vec2<f32>(id.xy) + vec2<f32>(0.5);
    var color = unpack(b_background[index]);
    if b_params.canvas.z != 0u {
        color = media(color, id.xy, p);
        for (var i = 0u; i < b_params.canvas.w; i++) { color = pointer(color, b_params.draws[i], p); }
    }
    color = camera(color, index, p);
    if b_params.images[7].z != 0u {
        let s = unpack(b_watermark[index]);
        color = over(color, s.xyz, s.w / 255.0);
    }
    if b_params.projection.y > 0.0 {
        let center = vec2<f32>(b_params.canvas.xy) * 0.5;
        let r = length(p - center) / max(length(center), 1.0);
        let t = clamp((r - 0.3) / 0.7, 0.0, 1.0);
        let factor = 1.0 - b_params.projection.y * 0.85 * t * t * (3.0 - 2.0 * t);
        color = vec4<f32>(floor(color.xyz * factor + vec3<f32>(0.5)), color.w);
    }
    if b_params.images[6].z != 0u {
        let s = unpack(b_caption[index]);
        color = over(color, s.xyz, s.w / 255.0);
    }
    b_output[index] = pack(color);
}

// Same separable Lanczos3 filter, support and normalization as image::resize.
// This runs only when caption pixels or canvas dimensions change.
var<storage, read> b_resize_source: array<u32>;
var<storage, read_write> b_resize_temp: array<vec4<f32>>;
var<storage, read_write> b_resize_output: array<u32>;
fn lanczos(x: f32) -> f32 {
    if abs(x) >= 3.0 { return 0.0; }
    if abs(x) < 1e-7 { return 1.0; }
    let p = x * 3.141592653589793;
    return (sin(p) / p) * (sin(p / 3.0) / (p / 3.0));
}
@compute @workgroup_size(8, 8, 1)
fn resize_vertical(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = b_params.images[6].xy;
    if id.x >= size.x || id.y >= b_params.canvas.y { return; }
    let ratio = f32(size.y) / f32(b_params.canvas.y);
    let scale = max(ratio, 1.0);
    let center = (f32(id.y) + 0.5) * ratio;
    let lo = u32(clamp(floor(center - 3.0 * scale), 0.0, f32(size.y - 1u)));
    let hi = u32(clamp(ceil(center + 3.0 * scale), f32(lo + 1u), f32(size.y)));
    var weight = 0.0;
    var color = vec4<f32>(0.0);
    for (var i = lo; i < hi; i++) {
        let w = lanczos((f32(i) - (center - 0.5)) / scale);
        color += unpack(b_resize_source[i * size.x + id.x]) * w;
        weight += w;
    }
    b_resize_temp[id.y * size.x + id.x] = color / weight;
}
@compute @workgroup_size(8, 8, 1)
fn resize_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = b_params.images[6].xy;
    if any(id.xy >= b_params.canvas.xy) { return; }
    let ratio = f32(size.x) / f32(b_params.canvas.x);
    let scale = max(ratio, 1.0);
    let center = (f32(id.x) + 0.5) * ratio;
    let lo = u32(clamp(floor(center - 3.0 * scale), 0.0, f32(size.x - 1u)));
    let hi = u32(clamp(ceil(center + 3.0 * scale), f32(lo + 1u), f32(size.x)));
    var weight = 0.0;
    var color = vec4<f32>(0.0);
    for (var i = lo; i < hi; i++) {
        let w = lanczos((f32(i) - (center - 0.5)) / scale);
        color += b_resize_temp[id.y * size.x + i] * w;
        weight += w;
    }
    b_resize_output[id.y * b_params.canvas.x + id.x] = pack(color / weight);
}
