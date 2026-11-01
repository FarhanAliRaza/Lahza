// Motion engine rasteriser. Mirrors raster.rs function for function so the
// CPU fallback draws the same picture. Colours are premultiplied floats.
// One 16×16 workgroup shades one tile from its binned primitive list.

struct Prim {
    kind: vec4<u32>,
    inv_x: vec4<f32>,
    inv_y: vec4<f32>,
    geom: vec4<f32>,
    geom2: vec4<f32>,
    fill: vec4<f32>,
    stroke: vec4<f32>,
    paint: vec4<f32>,
    paint2: vec4<f32>,
}

struct Pass {
    // width, height, tiles_x, range_base
    size: vec4<u32>,
    // target offset (texels), mode (0 replace, 1 accumulate, 2 replace weighted)
    dest: vec4<u32>,
    background: vec4<f32>,
    // x: accumulation weight
    weight: vec4<f32>,
}

struct BlurPass {
    // width, height, offset, horizontal
    size: vec4<u32>,
    // sigma, radius, 1/norm
    sigma: vec4<f32>,
}

struct ResolvePass {
    size: vec4<u32>,
}

var<uniform> u_pass: Pass;
var<storage, read> b_prims: array<Prim>;
var<storage, read> b_ranges: array<u32>;
var<storage, read> b_indices: array<u32>;
var<storage, read> b_stops: array<vec4<f32>>;
var<storage, read> b_atlas: array<u32>;
var<storage, read> b_images: array<u32>;
var<storage, read> b_groups: array<vec4<f32>>;
var<storage, read_write> b_target: array<vec4<f32>>;

var<uniform> u_blur: BlurPass;
var<storage, read> b_blur_src: array<vec4<f32>>;
var<storage, read_write> b_blur_dst: array<vec4<f32>>;

var<uniform> u_resolve: ResolvePass;
var<storage, read> b_accum: array<vec4<f32>>;
var<storage, read_write> b_output: array<u32>;

const ATLAS_SIZE: u32 = 2048u;
const SPREAD: f32 = 12.0;
const PI: f32 = 3.14159265358979;

fn clamp01(v: f32) -> f32 { return clamp(v, 0.0, 1.0); }

fn sd_ellipse(p: vec2<f32>, r_in: vec2<f32>) -> f32 {
    if abs(r_in.x - r_in.y) < 1e-4 { return length(p) - r_in.x; }
    let r = max(r_in, vec2<f32>(1e-4));
    let k0 = length(p / r);
    let k1 = length(p / (r * r));
    if k1 < 1e-9 { return -min(r.x, r.y); }
    return k0 * (k0 - 1.0) / k1;
}

fn sd_rect(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn sd_capsule(p: vec2<f32>, b: vec2<f32>) -> f32 {
    let r = min(b.x, b.y);
    if b.x >= b.y {
        return length(vec2<f32>(max(abs(p.x) - (b.x - r), 0.0), p.y)) - r;
    }
    return length(vec2<f32>(p.x, max(abs(p.y) - (b.y - r), 0.0))) - r;
}

fn sd_arc(p: vec2<f32>, g: vec4<f32>, full: bool) -> f32 {
    let rc = g.x;
    let ht = g.y;
    if full { return abs(length(p) - rc) - ht; }
    let s = sin(g.z);
    let c = cos(g.z);
    let x = p.x * c + p.y * s;
    let y = p.y * c - p.x * s;
    let q = vec2<f32>(abs(x), -y);
    let sc = vec2<f32>(sin(g.w), cos(g.w));
    var d: f32;
    if sc.y * q.x > sc.x * q.y {
        d = length(q - sc * rc);
    } else {
        d = abs(length(q) - rc);
    }
    return d - ht;
}

fn sd_line(p: vec2<f32>, g: vec4<f32>) -> f32 {
    let dx = max(max(g.x - p.x, p.x - g.y), 0.0);
    return length(vec2<f32>(dx, p.y)) - g.z;
}

fn sd_star(p: vec2<f32>, g: vec4<f32>) -> f32 {
    let n = max(g.z, 3.0);
    let round = g.w;
    let r = max(g.x - round, 1e-4);
    let k = g.y;
    let an = PI / n;
    let theta = atan2(p.x, -p.y);
    var b = theta + an;
    b = b - 2.0 * an * floor(b / (2.0 * an));
    b = abs(b - an);
    let l = length(p);
    let q = vec2<f32>(l * cos(b), l * sin(b));
    let a = vec2<f32>(r, 0.0);
    let bb = vec2<f32>(k * r * cos(an), k * r * sin(an));
    let e = bb - a;
    let w = q - a;
    let t = clamp01(dot(w, e) / max(dot(e, e), 1e-9));
    let dist = length(w - e * t);
    let cross = e.x * w.y - e.y * w.x;
    var sd = dist;
    if cross > 0.0 { sd = -dist; }
    return sd - round;
}

fn shape_distance(prim: Prim, l: vec2<f32>) -> f32 {
    let g = prim.geom;
    switch prim.kind.y {
        case 0u: { return sd_ellipse(l, g.xy); }
        case 1u: { return sd_rect(l, g.xy, g.z); }
        case 2u: { return sd_capsule(l, g.xy); }
        case 3u: { return sd_arc(l, g, prim.geom2.x > 0.5); }
        case 4u: { return sd_line(l, g); }
        default: { return sd_star(l, g); }
    }
}

fn gradient(prim: Prim, l: vec2<f32>) -> vec4<f32> {
    let origin = prim.paint2.xy;
    let d = prim.paint2.zw - origin;
    let dd = max(dot(d, d), 1e-9);
    var t: f32;
    if prim.paint.y < 1.5 {
        t = dot(l - origin, d) / dd;
    } else {
        t = length(l - origin) / sqrt(dd);
    }
    let base = u32(prim.paint.z) * 2u;
    let count = u32(prim.paint.w);
    let o0 = b_stops[base].x;
    if t <= o0 || count == 1u { return b_stops[base + 1u]; }
    for (var i = 0u; i + 1u < count; i++) {
        let oa = b_stops[base + i * 2u].x;
        let ob = b_stops[base + i * 2u + 2u].x;
        if t <= ob {
            return mix(b_stops[base + i * 2u + 1u], b_stops[base + i * 2u + 3u], clamp01((t - oa) / max(ob - oa, 1e-6)));
        }
    }
    return b_stops[base + (count - 1u) * 2u + 1u];
}

fn atlas_texel(x: i32, y: i32) -> f32 {
    let i = u32(y) * ATLAS_SIZE + u32(x);
    return f32((b_atlas[i >> 2u] >> ((i & 3u) * 8u)) & 255u) / 255.0;
}

fn sample_atlas(l: vec2<f32>, r: vec4<f32>) -> f32 {
    let s = l - vec2<f32>(0.5);
    let x0 = floor(s.x);
    let y0 = floor(s.y);
    let fx = s.x - x0;
    let fy = s.y - y0;
    let lo = vec2<i32>(i32(r.x), i32(r.y));
    let hi = vec2<i32>(i32(r.z) - 1, i32(r.w) - 1);
    let ax = clamp(i32(x0), lo.x, hi.x);
    let bx = clamp(i32(x0 + 1.0), lo.x, hi.x);
    let ay = clamp(i32(y0), lo.y, hi.y);
    let by = clamp(i32(y0 + 1.0), lo.y, hi.y);
    let a = atlas_texel(ax, ay);
    let b = atlas_texel(bx, ay);
    let c = atlas_texel(ax, by);
    let d = atlas_texel(bx, by);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    return top + (bottom - top) * fy;
}

fn unpack(v: u32) -> vec4<f32> {
    return vec4<f32>(vec4<u32>(v, v >> 8u, v >> 16u, v >> 24u) & vec4<u32>(255u)) / 255.0;
}

fn image_texel(source: u32, offset: u32, w: i32, h: i32, x: i32, y: i32) -> vec4<f32> {
    let i = u32(i32(offset) + clamp(y, 0, h - 1) * w + clamp(x, 0, w - 1));
    if source == 1u { return b_groups[i]; }
    return unpack(b_images[i]);
}

fn sample_image(prim: Prim, l: vec2<f32>) -> vec4<f32> {
    let source = prim.kind.y;
    var offset = bitcast<u32>(prim.geom2.x);
    var w = i32(prim.geom.x);
    var h = i32(prim.geom.y);
    let levels = i32(prim.geom.z);
    let aa = prim.inv_x.w;
    let lod = clamp(i32(floor(log2(max(aa, 1.0)))), 0, levels - 1);
    for (var i = 0; i < lod; i++) {
        offset += u32(w * h);
        w = max(w / 2, 1);
        h = max(h / 2, 1);
    }
    let k = f32(1u << u32(lod));
    let s = l / k - vec2<f32>(0.5);
    let fx0 = floor(s.x);
    let fy0 = floor(s.y);
    let fx = s.x - fx0;
    let fy = s.y - fy0;
    let x0 = i32(fx0);
    let y0 = i32(fy0);
    let a = image_texel(source, offset, w, h, x0, y0);
    let b = image_texel(source, offset, w, h, x0 + 1, y0);
    let c = image_texel(source, offset, w, h, x0, y0 + 1);
    let d = image_texel(source, offset, w, h, x0 + 1, y0 + 1);
    return mix(mix(a, b, fx), mix(c, d, fx), fy);
}

fn shade(prim: Prim, p: vec2<f32>) -> vec4<f32> {
    let l = vec2<f32>(
        prim.inv_x.x * p.x + prim.inv_x.y * p.y + prim.inv_x.z,
        prim.inv_y.x * p.x + prim.inv_y.y * p.y + prim.inv_y.z,
    );
    let aa = prim.inv_x.w;
    let opacity = prim.inv_y.w;
    var out = vec4<f32>(0.0);
    if prim.kind.x == 0u {
        let d = shape_distance(prim, l);
        if (prim.kind.w & 1u) != 0u {
            let cov = clamp01(0.5 - d / aa);
            if cov > 0.0 {
                var c = prim.fill;
                if prim.paint.y > 0.5 { c = gradient(prim, l); }
                out = c * cov;
            }
        }
        if (prim.kind.w & 2u) != 0u {
            let cov = clamp01(0.5 - (abs(d) - prim.paint.x * 0.5) / aa);
            if cov > 0.0 {
                let s = prim.stroke * cov;
                out = s + out * (1.0 - s.w);
            }
        }
    } else if prim.kind.x == 1u {
        let r = prim.geom;
        if l.x < r.x || l.y < r.y || l.x > r.z || l.y > r.w { return out; }
        let v = sample_atlas(l, r);
        let d = (0.5 - v) * 2.0 * SPREAD;
        let cov = clamp01(0.5 - d / aa);
        out = prim.fill * cov;
    } else {
        let w = prim.geom.x;
        let h = prim.geom.y;
        let edge = max(max(-l.x, l.x - w), max(-l.y, l.y - h));
        let cov = clamp01(0.5 - edge / max(aa, 1e-6));
        if cov <= 0.0 { return out; }
        out = sample_image(prim, l) * cov;
    }
    return out * opacity;
}

fn blend(d: vec4<f32>, s: vec4<f32>, mode: u32) -> vec4<f32> {
    switch mode {
        case 1u: { return min(s + d, vec4<f32>(1.0)); }
        case 2u: { return s * d + s * (1.0 - d.w) + d * (1.0 - s.w); }
        case 3u: { return s + d - s * d; }
        default: { return s + d * (1.0 - s.w); }
    }
}

@compute @workgroup_size(16, 16, 1)
fn raster(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let size = u_pass.size;
    let x = wg.x * 16u + lid.x;
    let y = wg.y * 16u + lid.y;
    if x >= size.x || y >= size.y { return; }
    let tile = size.w + wg.y * size.z + wg.x;
    let start = b_ranges[tile * 2u];
    let count = b_ranges[tile * 2u + 1u];
    let p = vec2<f32>(f32(x) + 0.5, f32(y) + 0.5);
    var c = u_pass.background;
    for (var k = 0u; k < count; k++) {
        let prim = b_prims[b_indices[start + k]];
        let s = shade(prim, p);
        if s.w > 0.0 {
            c = blend(c, s, prim.kind.z);
        }
    }
    let o = u_pass.dest.x + y * size.x + x;
    if u_pass.dest.y == 1u {
        b_target[o] = b_target[o] + c * u_pass.weight.x;
    } else if u_pass.dest.y == 2u {
        b_target[o] = c * u_pass.weight.x;
    } else {
        b_target[o] = c;
    }
}

@compute @workgroup_size(16, 16, 1)
fn blur(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u_blur.size.x;
    let h = u_blur.size.y;
    if id.x >= w || id.y >= h { return; }
    let base = u_blur.size.z;
    let horizontal = u_blur.size.w == 1u;
    let sigma = u_blur.sigma.x;
    let radius = i32(u_blur.sigma.y);
    let inv_norm = u_blur.sigma.z;
    var acc = vec4<f32>(0.0);
    for (var d = -radius; d <= radius; d++) {
        var sx = i32(id.x);
        var sy = i32(id.y);
        if horizontal { sx += d; } else { sy += d; }
        if sx < 0 || sy < 0 || sx >= i32(w) || sy >= i32(h) { continue; }
        let wt = exp(-f32(d * d) / (2.0 * sigma * sigma)) * inv_norm;
        acc += b_blur_src[base + u32(sy) * w + u32(sx)] * wt;
    }
    b_blur_dst[base + id.y * w + id.x] = acc;
}

@compute @workgroup_size(16, 16, 1)
fn resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u_resolve.size.x;
    if id.x >= w || id.y >= u_resolve.size.y { return; }
    let i = id.y * w + id.x;
    let c = b_accum[i];
    let a = clamp(c.w, 0.0, 1.0);
    var k = 0.0;
    if a > 0.0 { k = 1.0 / a; }
    let rgb = vec3<u32>(clamp(c.xyz * k, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + vec3<f32>(0.5));
    let alpha = u32(a * 255.0 + 0.5);
    b_output[i] = rgb.x | (rgb.y << 8u) | (rgb.z << 16u) | (alpha << 24u);
}
