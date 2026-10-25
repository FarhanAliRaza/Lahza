// Byte-domain math deliberately matches the CPU renderer (no sRGB conversion).
struct Draw {
    kind: vec4<f32>, rect: vec4<f32>, anchor: vec4<f32>,
    smear: vec4<f32>, color: vec4<f32>, bounds: vec4<f32>,
}
struct Params {
    canvas: vec4<u32>, images: array<vec4<u32>, 8>, inverse: array<vec4<f32>, 3>,
    viewport: vec4<f32>, loupe_shape: vec4<f32>, loupe_optics: vec4<f32>,
    sample_bounds: vec4<f32>, geometry: vec4<f32>,
    projection: vec4<f32>, media_bounds: vec4<u32>, camera_rect: vec4<f32>, camera_info: vec4<f32>, camera_crop: vec4<f32>,
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
// Recompose a screen pixel without overlays above the glass. Bilinear taps
// below reproduce sampling the immutable CPU screen pass, including pointer.
fn screen_pixel(position: vec2<u32>) -> vec4<f32> {
    let index = position.y * b_params.canvas.x + position.x;
    let p = vec2<f32>(position) + vec2<f32>(0.5);
    var color = unpack(b_background[index]);
    if b_params.canvas.z != 0u {
        color = media(color, position, p);
        for (var i = 0u; i < b_params.canvas.w; i++) { color = pointer(color, b_params.draws[i], p); }
    }
    return color;
}
fn screen_sample(point: vec2<f32>) -> vec4<f32> {
    let p = clamp(point - vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(b_params.canvas.xy - vec2<u32>(1u)));
    let lo = vec2<u32>(floor(p));
    let hi = min(lo + vec2<u32>(1u), b_params.canvas.xy - vec2<u32>(1u));
    let f = p - vec2<f32>(lo);
    return mix(mix(screen_pixel(lo), screen_pixel(vec2<u32>(hi.x, lo.y)), f.x),
        mix(screen_pixel(vec2<u32>(lo.x, hi.y)), screen_pixel(hi), f.x), f.y);
}
fn loupe(base: vec4<f32>, p: vec2<f32>) -> vec4<f32> {
    let shape = b_params.loupe_shape;
    let optics = b_params.loupe_optics;
    if optics.z <= 0.0 || shape.z <= 0.0 { return base; }
    let delta = p - shape.xy;
    let radius = max(shape.z, 1.0);
    let distance_squared = dot(delta, delta);
    let reach = radius * 1.37 + 0.5;
    if distance_squared > reach * reach { return base; }
    let inner_radius = min(min(1.0 - max(optics.x, 0.001), 0.965) * radius, radius - 0.5);
    let interior = inner_radius >= 0.0 && distance_squared <= inner_radius * inner_radius;
    // Most lens pixels have no bevel, reflection, dispersion, or rim light.
    // Recompose four bilinear taps once, rather than four sets of four taps.
    if interior && optics.z >= 1.0 {
        return quantize(screen_sample(shape.xy + delta * (1.0 / shape.w)));
    }
    let light_delta = vec2<f32>(f32(b_params.canvas.x) * 0.5, -f32(b_params.canvas.y) * 0.5) - shape.xy;
    let light = light_delta / max(length(light_delta), 0.001);
    if interior {
        let shadow_r = length(delta + light * radius * 0.09) / radius;
        let shadow = (1.0 - smoothstep(0.93, 1.28, shadow_r)) * 0.35 * optics.z;
        let outside = vec4<f32>(base.xyz * (1.0 - shadow), base.w);
        return quantize(mix(outside, screen_sample(shape.xy + delta * (1.0 / shape.w)), optics.z));
    }
    let distance = sqrt(distance_squared);
    let r = distance / radius;
    let coverage = clamp(radius - distance + 0.5, 0.0, 1.0);
    var shadow = 0.0;
    if coverage < 1.0 || optics.z < 1.0 {
        let shadow_r = length(delta + light * radius * 0.09) / radius;
        shadow = (1.0 - smoothstep(0.93, 1.28, shadow_r)) * 0.35 * optics.z;
    }
    var outside = vec4<f32>(base.xyz * (1.0 - shadow), base.w);
    if coverage <= 0.0 && (r >= 1.12 || optics.x == 0.0) { return quantize(outside); }
    let normal = delta / max(distance, 0.001);
    let lighting = dot(normal, light);
    let sheen = (1.0 - smoothstep(1.0, 1.12, r)) * smoothstep(0.98, 1.01, r)
        * pow(max(-lighting, 0.0), 3.0) * optics.x * optics.z * 0.7;
    let spectrum_phase = (r * 2.0 + lighting) * 6.283185307179586;
    let spectrum = cos(vec3<f32>(spectrum_phase, spectrum_phase - 2.0943952, spectrum_phase + 2.0943952));
    let tint = vec3<f32>(255.0) * (vec3<f32>(1.0) - optics.y * 0.35 * (vec3<f32>(0.5) + spectrum * 0.5));
    if sheen > 0.0 { outside = vec4<f32>(mix(outside.xyz, tint, sheen), outside.w); }
    if coverage <= 0.0 { return quantize(outside); }
    let bevel = optics.x;
    let shell = smoothstep(1.0 - max(bevel, 0.001), 1.0, r);
    let bend = sin(shell * 3.141592653589793) * bevel * 0.22;
    let q = shape.xy + delta * (1.0 / shape.w - bend);
    let dispersion = optics.y * bevel * radius * 0.055 * shell;
    var glass = screen_sample(q);
    if dispersion != 0.0 {
        glass.x = screen_sample(q + normal * dispersion).x;
        glass.z = screen_sample(q - normal * dispersion).z;
    }
    let reflection_mix = shell * bevel * 0.8;
    if reflection_mix > 0.0 {
        let reflection = screen_sample(shape.xy + normal * radius * (1.0 + (1.0 - r) * 0.65));
        glass = vec4<f32>(mix(glass.xyz, reflection.xyz, reflection_mix), glass.w);
    }
    let rim = smoothstep(0.965, 0.997, r) * (1.0 - smoothstep(0.997, 1.006, r));
    let glint = pow(max(lighting, 0.0), 12.0) + pow(max(-lighting, 0.0), 20.0) * 0.45;
    let highlight = (shell * glint * 0.24 + rim * (0.12 + glint * 0.62)) * min(bevel * 5.0, 1.0);
    let inner_shade = shell * bevel * 0.24 * (1.0 - max(lighting, 0.0));
    glass = vec4<f32>(glass.xyz * (1.0 - inner_shade), glass.w);
    glass = vec4<f32>(mix(glass.xyz, tint, clamp(highlight, 0.0, 1.0)), glass.w);
    return quantize(mix(outside, glass, coverage * optics.z));
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
    var uv = local / rect.zw;
    if b_params.camera_info.y != 0.0 { uv.x = 1.0 - uv.x; }
    let xy = (b_params.camera_crop.xy + uv * b_params.camera_crop.zw) * size - vec2<f32>(0.5);
    let s = sample_image(3u, xy, false);
    return over(result, s.xyz, coverage * s.w / 255.0);
}
@compute @workgroup_size(8, 8, 1)
fn compose(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= b_params.canvas.xy) { return; }
    let index = id.y * b_params.canvas.x + id.x;
    let p = vec2<f32>(id.xy) + vec2<f32>(0.5);
    var color = loupe(screen_pixel(id.xy), p);
    color = quantize(mix(camera(color, index, p), color, b_params.loupe_optics.z));
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
