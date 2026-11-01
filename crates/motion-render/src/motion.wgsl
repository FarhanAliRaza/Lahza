// Motion engine rasteriser. Mirrors raster.rs function for function so the
// CPU fallback draws the same picture. Colours are premultiplied floats.
// One 16×16 workgroup shades one tile from its binned primitive list.

struct Prim {
    kind: vec4<u32>,
    inv_x: vec4<f32>,
    inv_y: vec4<f32>,
    inv_w: vec4<f32>,
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
var<storage, read> b_segs: array<vec4<f32>>;
var<storage, read> b_footage: array<u32>;
var<storage, read> b_ops: array<vec4<f32>>;
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

fn sd_path(prim: Prim, l: vec2<f32>) -> f32 {
    let first = bitcast<u32>(prim.geom2.x);
    let count = bitcast<u32>(prim.geom2.y);
    let open = prim.geom2.z > 0.5;
    var best = 3.0e38;
    var winding = 0;
    for (var i = 0u; i < count; i++) {
        let s = b_segs[first + i];
        let e = s.zw - s.xy;
        let w = l - s.xy;
        let t = clamp01(dot(w, e) / max(dot(e, e), 1e-12));
        let q = w - e * t;
        best = min(best, dot(q, q));
        if !open && ((s.y <= l.y) != (s.w <= l.y)) {
            let x = s.x + (l.y - s.y) / (s.w - s.y) * e.x;
            if x > l.x {
                if s.w > s.y { winding += 1; } else { winding -= 1; }
            }
        }
    }
    let d = sqrt(best);
    if winding != 0 { return -d; }
    return d;
}

fn luma(c: vec4<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

fn shape_distance(prim: Prim, l: vec2<f32>) -> f32 {
    let g = prim.geom;
    switch prim.kind.y {
        case 6u: { return sd_path(prim, l); }
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
    if source == 2u { return unpack(b_footage[i]); }
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

fn shade(prim_in: Prim, p: vec2<f32>) -> vec4<f32> {
    var prim = prim_in;
    let w = prim.inv_w.x * p.x + prim.inv_w.y * p.y + prim.inv_w.z;
    if w <= 0.0 { return vec4<f32>(0.0); }
    let l = vec2<f32>(
        (prim.inv_x.x * p.x + prim.inv_x.y * p.y + prim.inv_x.z) / w,
        (prim.inv_y.x * p.x + prim.inv_y.y * p.y + prim.inv_y.z) / w,
    );
    if prim.inv_w.x != 0.0 || prim.inv_w.y != 0.0 {
        prim.inv_x.w = max(sqrt(abs(prim.inv_w.w) / (w * w * w)), 1e-9);
    }
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
        var c: vec4<f32>;
        if bitcast<u32>(prim.paint.w) > 0u { c = sample_post(prim, l); } else { c = sample_image(prim, l); }
        if prim.kind.x == 3u {
            var matte = prim;
            matte.geom2.x = prim.geom2.y;
            let m = sample_image(matte, l);
            var k: f32;
            switch u32(prim.geom2.z) {
                case 0u: { k = m.w; }
                case 1u: { k = 1.0 - m.w; }
                case 2u: { k = luma(m); }
                default: { k = 1.0 - luma(m); }
            }
            c = c * clamp01(k);
        } else if (prim.kind.w & 4u) != 0u {
            c = prim.fill * c.w;
        } else if (prim.kind.w & 8u) != 0u {
            c = c * prim.fill;
        }
        out = c * cov;
    }
    return out * opacity;
}

fn lum3(c: vec3<f32>) -> f32 { return 0.3 * c.x + 0.59 * c.y + 0.11 * c.z; }

fn clip_color(c_in: vec3<f32>) -> vec3<f32> {
    var c = c_in;
    let l = lum3(c);
    let n = min(min(c.x, c.y), c.z);
    let x = max(max(c.x, c.y), c.z);
    if n < 0.0 { c = vec3<f32>(l) + (c - vec3<f32>(l)) * l / max(l - n, 1e-6); }
    if x > 1.0 { c = vec3<f32>(l) + (c - vec3<f32>(l)) * (1.0 - l) / max(x - l, 1e-6); }
    return c;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(c + vec3<f32>(l - lum3(c)));
}

fn sat3(c: vec3<f32>) -> f32 { return max(max(c.x, c.y), c.z) - min(min(c.x, c.y), c.z); }

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let mx = max(max(c.x, c.y), c.z);
    let mn = min(min(c.x, c.y), c.z);
    if mx > mn { return (c - vec3<f32>(mn)) * s / (mx - mn); }
    return vec3<f32>(0.0);
}

fn soft_light_d(b: f32) -> f32 {
    if b <= 0.25 { return ((16.0 * b - 12.0) * b + 4.0) * b; }
    return sqrt(b);
}

fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 { return b * 2.0 * s; }
    let t = 2.0 * s - 1.0;
    return b + t - b * t;
}

fn blend_channel(b: f32, s: f32, mode: u32) -> f32 {
    switch mode {
        case 2u: { return b * s; }
        case 3u: { return b + s - b * s; }
        case 4u: { return hard_light(s, b); }
        case 5u: { return min(b, s); }
        case 6u: { return max(b, s); }
        case 7u: {
            if b <= 0.0 { return 0.0; }
            if s >= 1.0 { return 1.0; }
            return min(b / (1.0 - s), 1.0);
        }
        case 8u: {
            if b >= 1.0 { return 1.0; }
            if s <= 0.0 { return 0.0; }
            return 1.0 - min((1.0 - b) / s, 1.0);
        }
        case 9u: { return hard_light(b, s); }
        case 10u: {
            if s <= 0.5 { return b - (1.0 - 2.0 * s) * b * (1.0 - b); }
            return b + (2.0 * s - 1.0) * (soft_light_d(b) - b);
        }
        case 11u: { return abs(b - s); }
        case 12u: { return b + s - 2.0 * b * s; }
        case 13u: { return max(b - s, 0.0); }
        case 14u: {
            if s <= 0.0 { return 1.0; }
            return min(b / s, 1.0);
        }
        default: { return s; }
    }
}

fn blend(d: vec4<f32>, s: vec4<f32>, mode: u32) -> vec4<f32> {
    if mode == 0u { return s + d * (1.0 - s.w); }
    if mode == 1u { return min(s + d, vec4<f32>(1.0)); }
    let sa = s.w;
    let da = d.w;
    var cs = vec3<f32>(0.0);
    var cb = vec3<f32>(0.0);
    if sa > 0.0 { cs = clamp(s.xyz / sa, vec3<f32>(0.0), vec3<f32>(1.0)); }
    if da > 0.0 { cb = clamp(d.xyz / da, vec3<f32>(0.0), vec3<f32>(1.0)); }
    var m: vec3<f32>;
    switch mode {
        case 15u: { m = set_lum(set_sat(cs, sat3(cb)), lum3(cb)); }
        case 16u: { m = set_lum(set_sat(cb, sat3(cs)), lum3(cb)); }
        case 17u: { m = set_lum(cs, lum3(cb)); }
        case 18u: { m = set_lum(cb, lum3(cs)); }
        default: { m = vec3<f32>(blend_channel(cb.x, cs.x, mode), blend_channel(cb.y, cs.y, mode), blend_channel(cb.z, cs.z, mode)); }
    }
    let rgb = s.xyz * (1.0 - da) + d.xyz * (1.0 - sa) + sa * da * m;
    return vec4<f32>(rgb, sa + da - sa * da);
}

// ------------------------------------------------------------ post program

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    var h = u32(x) * 374761393u + u32(y) * 668265263u + seed * 1442695041u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    h = h ^ (h >> 16u);
    return f32(h >> 8u) / 16777216.0;
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let ix = floor(x);
    let iy = floor(y);
    let fx = x - ix;
    let fy = y - iy;
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash(i32(ix), i32(iy), seed);
    let b = hash(i32(ix) + 1, i32(iy), seed);
    let c = hash(i32(ix), i32(iy) + 1, seed);
    let d = hash(i32(ix) + 1, i32(iy) + 1, seed);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    return (top + (bottom - top) * sy) * 2.0 - 1.0;
}

fn luma3(c: vec3<f32>) -> f32 { return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z; }

fn apply_color(i: u32, c: vec3<f32>, l: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let h = b_ops[i];
    let op = h.x;
    if op == 1.0 { return c * exp2(h.z); }
    if op == 2.0 { return (c - vec3<f32>(0.5)) * (1.0 + h.w) + vec3<f32>(0.5 + h.z); }
    if op == 3.0 {
        let e = b_ops[i + 1u];
        let x = clamp((c - vec3<f32>(h.z)) / max(h.w - h.z, 1e-5), vec3<f32>(0.0), vec3<f32>(1.0));
        return vec3<f32>(e.y) + pow(x, vec3<f32>(1.0 / max(e.x, 1e-3))) * (e.z - e.y);
    }
    if op == 4.0 {
        let s = sin(radians(h.z));
        let k = cos(radians(h.z));
        let r = vec3<f32>(
            dot(vec3<f32>(0.213 + k * 0.787 - s * 0.213, 0.715 - k * 0.715 - s * 0.715, 0.072 - k * 0.072 + s * 0.928), c),
            dot(vec3<f32>(0.213 - k * 0.213 + s * 0.143, 0.715 + k * 0.285 + s * 0.140, 0.072 - k * 0.072 - s * 0.283), c),
            dot(vec3<f32>(0.213 - k * 0.213 - s * 0.787, 0.715 - k * 0.715 + s * 0.715, 0.072 + k * 0.928 + s * 0.072), c));
        let y = luma3(r);
        let r2 = vec3<f32>(y) + (r - vec3<f32>(y)) * (1.0 + h.w);
        let light = b_ops[i + 1u].x;
        if light >= 0.0 { return r2 + (vec3<f32>(1.0) - r2) * light; }
        return r2 * (1.0 + light);
    }
    if op == 5.0 {
        let y = clamp(luma3(c), 0.0, 1.0);
        let g = mix(b_ops[i + 1u].xyz, b_ops[i + 2u].xyz, y);
        return mix(c, g, h.z);
    }
    if op == 6.0 {
        let n = u32(h.w);
        let y = clamp(luma3(c), 0.0, 1.0);
        var g = b_ops[i + 2u].xyz;
        if y > b_ops[i + 1u].x {
            g = b_ops[i + 2u * n].xyz;
            for (var k = 0u; k + 1u < n; k++) {
                let oa = b_ops[i + 1u + k * 2u].x;
                let ob = b_ops[i + 3u + k * 2u].x;
                if y <= ob {
                    g = mix(b_ops[i + 2u + k * 2u].xyz, b_ops[i + 4u + k * 2u].xyz, clamp((y - oa) / max(ob - oa, 1e-6), 0.0, 1.0));
                    break;
                }
            }
        }
        return mix(c, g, h.z);
    }
    if op == 7.0 { return mix(c, vec3<f32>(1.0) - c, h.z); }
    if op == 9.0 { return mix(c, b_ops[i + 1u].xyz, h.z); }
    if op == 8.0 {
        let n = u32(h.w);
        if n < 2u { return c; }
        let k = f32(n - 1u);
        let p = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * k;
        let i0 = min(vec3<u32>(floor(p)), vec3<u32>(n - 2u));
        let f = p - vec3<f32>(i0);
        var out = vec3<f32>(0.0);
        for (var dr = 0u; dr < 2u; dr++) {
            for (var dg = 0u; dg < 2u; dg++) {
                for (var db = 0u; db < 2u; db++) {
                    let w = select(1.0 - f.x, f.x, dr == 1u) * select(1.0 - f.y, f.y, dg == 1u) * select(1.0 - f.z, f.z, db == 1u);
                    let e = b_ops[i + 1u + (i0.x + dr) + (i0.y + dg) * n + (i0.z + db) * n * n];
                    out += e.xyz * w;
                }
            }
        }
        return mix(c, out, h.z);
    }
    if op == 30.0 {
        let q = l / max(size, vec2<f32>(1e-6)) * 2.0 - vec2<f32>(1.0);
        let r = length(q) / 1.41421356;
        let inner = 1.0 - h.w;
        let t = clamp((r - inner) / max(1.0 - inner, 1e-4), 0.0, 1.0);
        let v = t * t * (3.0 - 2.0 * t) * h.z;
        return c * (1.0 - v);
    }
    if op == 31.0 {
        let sz = max(h.w, 1e-3);
        let n = hash(i32(floor(l.x / sz)), i32(floor(l.y / sz)), u32(b_ops[i + 1u].x)) - 0.5;
        return c + vec3<f32>(n * h.z);
    }
    return c;
}

fn tap(prim: Prim, p: vec2<f32>, blur: vec2<f32>, has_blur: bool) -> vec4<f32> {
    if !has_blur { return sample_image(prim, p); }
    var acc = vec4<f32>(0.0);
    for (var k = 0; k < 9; k++) {
        let t = f32(k) / 8.0 - 0.5;
        acc += sample_image(prim, p + blur * t) / 9.0;
    }
    return acc;
}

fn sample_post(prim: Prim, l: vec2<f32>) -> vec4<f32> {
    let start = bitcast<u32>(prim.paint.z);
    let end = start + bitcast<u32>(prim.paint.w);
    let size = prim.geom.xy;
    var uv = l;
    var blur = vec2<f32>(0.0);
    var has_blur = false;
    var split = vec2<f32>(0.0);
    var has_split = false;
    var i = start;
    while i < end {
        let h = b_ops[i];
        if h.x == 20.0 {
            let e = b_ops[i + 1u];
            let scale = max(h.w, 1e-3);
            let px = uv.x / scale + e.x;
            let py = uv.y / scale + e.x * 0.7;
            let seed = u32(e.y);
            uv = uv + h.z * vec2<f32>(value_noise(px, py, seed), value_noise(px, py, seed ^ 0x9e3779b9u));
        } else if h.x == 21.0 {
            blur = h.zw;
            has_blur = true;
        } else if h.x == 22.0 {
            split = h.zw;
            has_split = true;
        }
        i += u32(h.y);
    }
    var c = tap(prim, uv, blur, has_blur);
    if has_split {
        let r = tap(prim, uv + split, blur, has_blur);
        let b = tap(prim, uv - split, blur, has_blur);
        c = vec4<f32>(r.x, c.y, b.z, max(max(c.w, r.w), b.w));
    }
    let a = c.w;
    if a <= 0.0 { return c; }
    var rgb = c.xyz / a;
    i = start;
    while i < end {
        let h = b_ops[i];
        if h.x < 20.0 || h.x >= 30.0 {
            rgb = apply_color(i, rgb, l, size);
        }
        i += u32(h.y);
    }
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb * a, a);
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
        if u_blur.sigma.w > 0.5 {
            sx = clamp(sx, 0, i32(w) - 1);
            sy = clamp(sy, 0, i32(h) - 1);
        } else if sx < 0 || sy < 0 || sx >= i32(w) || sy >= i32(h) { continue; }
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
