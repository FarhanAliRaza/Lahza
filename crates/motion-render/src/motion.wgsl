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
var<storage, read> b_mesh: array<vec4<f32>>;
var<storage, read> b_bins: array<u32>;
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
    var first = bitcast<u32>(prim.geom2.x);
    let count = bitcast<u32>(prim.geom2.y);
    let open = prim.geom2.z > 0.5;
    var best = 3.0e38;
    var winding = 0;
    if prim.geom2.w > 0.5 {
        // A tile of a split path: its row's winding from the segments
        // wholly to its right (see list.rs split_path).
        let h = b_segs[first];
        let r = u32(clamp(round((l.y - h.x) / h.y), 0.0, 15.0));
        winding = i32(b_segs[first + 1u + r / 4u][r % 4u]);
        first += 5u;
    }
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

fn along(prim: Prim, l: vec2<f32>) -> f32 {
    let first = bitcast<u32>(prim.paint2.x);
    let count = bitcast<u32>(prim.paint2.y);
    var best = 3.0e38;
    var u = 0.0;
    for (var i = 0u; i < count; i++) {
        let s = b_segs[first + i];
        let e = s.zw - s.xy;
        let w = l - s.xy;
        let t = clamp01(dot(w, e) / max(dot(e, e), 1e-12));
        let q = w - e * t;
        let d = dot(q, q);
        if d < best {
            best = d;
            let m = b_segs[first + count + i];
            u = m.x + (m.y - m.x) * t;
        }
    }
    return u;
}

fn field_lx(p: vec2<f32>, pt: vec4<f32>) -> f32 {
    let d = p - pt.xy;
    return log2(1.0 + dot(d, d) * pt.z);
}

fn field(prim: Prim, l: vec2<f32>) -> vec4<f32> {
    let warp = prim.paint2.x;
    let power = prim.paint2.z;
    var p = l;
    if warp != 0.0 {
        let s = max(prim.paint2.y, 1e-3);
        let q = l / s;
        p = l + warp * vec2<f32>(value_noise(q.x, q.y, 7u), value_noise(q.x + 31.7, q.y + 17.3, 13u));
    }
    let base = u32(prim.paint.z) * 2u;
    let count = u32(prim.paint.w);
    var lo = 0.0;
    var acc = vec4<f32>(0.0);
    var sum = 0.0;
    for (var i = 0u; i < count; i++) {
        let x = field_lx(p, b_stops[base + i * 2u]);
        if i == 0u {
            lo = x;
        } else if x < lo {
            let k = exp2(-power * (lo - x));
            acc *= k;
            sum *= k;
            lo = x;
        }
        let w = exp2(-power * (x - lo));
        acc += b_stops[base + i * 2u + 1u] * w;
        sum += w;
    }
    return acc / max(sum, 1e-30);
}

fn gradient(prim: Prim, l: vec2<f32>) -> vec4<f32> {
    let kind = u32(prim.paint.y);
    if kind == 5u { return field(prim, l); }
    let origin = prim.paint2.xy;
    let d = prim.paint2.zw - origin;
    let dd = max(dot(d, d), 1e-9);
    var t: f32;
    if kind == 4u {
        t = along(prim, l);
    } else if kind == 3u {
        t = length((l - origin) / max(prim.paint2.zw, vec2<f32>(1e-6)));
    } else if kind == 1u {
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

fn edge_light(prim: Prim, l: vec2<f32>, d: f32, c: vec4<f32>) -> vec4<f32> {
    let r = prim.geom;
    let gx = sample_atlas(l + vec2<f32>(1.0, 0.0), r) - sample_atlas(l - vec2<f32>(1.0, 0.0), r);
    let gy = sample_atlas(l + vec2<f32>(0.0, 1.0), r) - sample_atlas(l - vec2<f32>(0.0, 1.0), r);
    let n = sqrt(gx * gx + gy * gy);
    if n <= 1e-6 { return c; }
    let facing = clamp01(-(gx * prim.geom2.z + gy * prim.geom2.w) / n);
    let rim = facing * clamp01(1.0 + d / prim.paint.w);
    let e = prim.stroke;
    return e * rim + c * (1.0 - e.w * rim);
}

fn wipe(prim: Prim, l: vec2<f32>) -> f32 {
    let q = prim.paint2;
    var r: f32;
    if prim.kind.y == 3u {
        r = length(l - q.xy) * q.z;
    } else {
        r = q.x * l.x + q.y * l.y + q.z;
        if prim.kind.y == 2u { r = abs(r); }
    }
    let m = clamp01(q.w - r);
    return clamp01(prim.geom2.x + (prim.geom2.y - prim.geom2.x) * m);
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
                var sc = prim.stroke;
                if u32(prim.paint.y) == 4u { sc = gradient(prim, l); }
                let s = sc * cov;
                out = s + out * (1.0 - s.w);
            }
        }
    } else if prim.kind.x == 1u {
        let r = prim.geom;
        if l.x < r.x || l.y < r.y || l.x > r.z || l.y > r.w { return out; }
        let v = sample_atlas(l, r);
        let d = (0.5 - v) * 2.0 * SPREAD - prim.paint.x;
        if (prim.kind.w & 2u) != 0u {
            let mid = (prim.paint.y + prim.paint.z) * 0.5;
            let half = (prim.paint.z - prim.paint.y) * 0.5;
            out = prim.fill * clamp01(0.5 - (abs(d - mid) - half) / aa);
        } else {
            var c = prim.fill;
            if (prim.kind.w & 16u) != 0u { c = edge_light(prim, l, d, c); }
            out = c * clamp01(0.5 - d / aa);
        }
        if prim.kind.y != 0u { out = out * wipe(prim, l); }
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

// ------------------------------------------------------------------ meshes
// Mirrors mesh3d.rs: nearest triangle at 8 samples, each distinct winner
// shaded once at the centroid of its samples (GGX, clear coat, fresnel,
// procedural environment, linear light encoded to sRGB).

const SAMPLE_X = array<f32, 8>(1.0, -1.0, 5.0, -3.0, -5.0, -7.0, 3.0, 7.0);
const SAMPLE_Y = array<f32, 8>(-3.0, 3.0, 1.0, -5.0, 5.0, -1.0, 7.0, -7.0);
const NO_TRI: u32 = 0xffffffffu;

fn env_light(lb: u32, d: vec3<f32>, r: f32) -> vec3<f32> {
    let up = -d.y;
    let sky = b_mesh[lb + 4u].xyz;
    let horizon = b_mesh[lb + 5u].xyz;
    let ground = b_mesh[lb + 6u].xyz;
    var c: vec3<f32>;
    if up >= 0.0 {
        c = mix(horizon, sky, smoothstep(0.0, 0.6 + r, up));
    } else {
        c = mix(horizon, ground, smoothstep(0.0, 0.2 + r, -up));
    }
    let lights = u32(b_mesh[lb].x);
    let lobes = u32(b_mesh[lb].y);
    let first = lb + 7u + lights * 2u;
    for (var i = 0u; i < lobes; i++) {
        let l = b_mesh[first + i * 2u];
        let s2 = l.w * l.w;
        let w2 = s2 + r * r * 1.2;
        let k = exp((dot(d, l.xyz) - 1.0) / w2) * s2 / w2;
        c = c + b_mesh[first + i * 2u + 1u].xyz * k;
    }
    return c;
}

const SHADOW_TAP_X = array<f32, 8>(-0.613, 0.170, -0.299, 0.645, -0.651, 0.421, -0.084, 0.874);
const SHADOW_TAP_Y = array<f32, 8>(0.617, -0.040, -0.792, 0.493, -0.075, -0.718, 0.290, -0.130);
const NO_SURFACE: f32 = -1.0e30;

fn shadow_lit(s: f32, c: f32, bias: f32, point: bool) -> f32 {
    if s <= NO_SURFACE * 0.5 { return 1.0; }
    var lit: bool;
    if point { lit = s <= 0.0 || 1.0 / s >= c * (1.0 - bias); } else { lit = s <= c + bias; }
    if lit { return 1.0; }
    return 0.0;
}

fn shadow_texel(base: u32, size: i32, i: i32, j: i32, c: f32, bias: f32, point: bool) -> f32 {
    if i < 0 || j < 0 || i >= size || j >= size { return 1.0; }
    let k = u32(j * size + i);
    return shadow_lit(b_mesh[base + 6u + k / 4u][k % 4u], c, bias, point);
}

// Mirrors `shadow` in mesh3d.rs.
fn shadow(base: u32, p: vec3<f32>, n_in: vec3<f32>) -> f32 {
    let h0 = b_mesh[base];
    let h1 = b_mesh[base + 1u];
    let u = b_mesh[base + 2u];
    let v = b_mesh[base + 3u];
    let a = b_mesh[base + 4u];
    let pos = b_mesh[base + 5u];
    let point = h0.x > 0.5;
    let size = i32(h0.y);
    var offset = u.w;
    if point { offset = offset * max(dot(p - pos.xyz, a.xyz), 1.0); }
    var tw = a.xyz;
    if point { tw = pos.xyz - p; }
    let toward = tw / max(sqrt(dot(tw, tw)), 1e-20);
    var n = n_in;
    if dot(n, toward) < 0.0 { n = -n; }
    let po = p + n * offset;
    let cs = max(dot(n, toward), 0.05);
    let slope = min(sqrt(max(1.0 - cs * cs, 0.0)) / cs, 4.0);
    let bias = h1.z + u.w * h1.w * slope;
    var tu: f32;
    var tv: f32;
    var c: f32;
    if point {
        let q = po - pos.xyz;
        let z = dot(q, a.xyz);
        if z < 1.0 { return 1.0; }
        tu = dot(q, u.xyz) / z;
        tv = dot(q, v.xyz) / z;
        c = z;
    } else {
        tu = dot(po, u.xyz);
        tv = dot(po, v.xyz);
        c = dot(po, a.xyz);
    }
    let x = (tu - h0.z) * h1.x - 0.5;
    let y = (tv - h0.w) * h1.y - 0.5;
    var sum = 0.0;
    for (var t = 0u; t < 8u; t++) {
        let sx = x + SHADOW_TAP_X[t] * h1.w;
        let sy = y + SHADOW_TAP_Y[t] * h1.w;
        let fx = floor(sx);
        let fy = floor(sy);
        let ax = sx - fx;
        let ay = sy - fy;
        let i = i32(fx);
        let j = i32(fy);
        let t00 = shadow_texel(base, size, i, j, c, bias, point);
        let t10 = shadow_texel(base, size, i + 1, j, c, bias, point);
        let t01 = shadow_texel(base, size, i, j + 1, c, bias, point);
        let t11 = shadow_texel(base, size, i + 1, j + 1, c, bias, point);
        let top = t00 + (t10 - t00) * ax;
        let bottom = t01 + (t11 - t01) * ax;
        sum += top + (bottom - top) * ay;
    }
    return sum / 8.0;
}

// The shadow map linked from light i's colour row (0: none).
fn shadow_of(lb: u32, i: u32) -> i32 {
    return bitcast<i32>(b_mesh[lb + 7u + i * 2u + 1u].w);
}

fn to_world(lb: u32, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(b_mesh[lb + 1u].xyz, v), dot(b_mesh[lb + 2u].xyz, v), dot(b_mesh[lb + 3u].xyz, v));
}

fn ggx(n_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-12);
}

fn smith(n_v: f32, n_l: f32, r: f32) -> f32 {
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    return n_v / (n_v * (1.0 - k) + k) * (n_l / (n_l * (1.0 - k) + k));
}

fn env_brdf(r: f32, n_v: f32) -> vec2<f32> {
    let rr = r * vec4<f32>(-1.0, -0.0275, -0.572, 0.022) + vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let a004 = min(rr.x * rr.x, exp2(-9.28 * n_v)) * rr.x + rr.y;
    return vec2<f32>(-1.04 * a004 + rr.z, 1.04 * a004 + rr.w);
}

fn shoulder(x: f32) -> f32 {
    if x < 0.8 { return x; }
    return 0.8 + 0.2 * (1.0 - exp(-(x - 0.8) / 0.2));
}

fn encode(x_in: f32) -> f32 {
    let x = clamp(x_in, 0.0, 1.0);
    if x <= 0.0031308 { return x * 12.92; }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

fn pow5(x: f32) -> f32 {
    let x2 = x * x;
    return x2 * x2 * x;
}

// Edge function of a→b at q with the endpoints in a fixed order, so two
// triangles sharing an edge compute it identically (watertight).
fn edge_fn(a: vec2<f32>, b: vec2<f32>, q: vec2<f32>) -> f32 {
    if a.x < b.x || (a.x == b.x && a.y < b.y) {
        return (b.x - a.x) * (q.y - a.y) - (b.y - a.y) * (q.x - a.x);
    }
    return -((a.x - b.x) * (q.y - b.y) - (a.y - b.y) * (q.x - b.x));
}

fn owns(d: vec2<f32>) -> bool {
    return d.y > 0.0 || (d.y == 0.0 && d.x < 0.0);
}

struct Cover {
    e: vec3<f32>,
    inside: bool,
}

fn cover(r0: vec4<f32>, r1: vec4<f32>, q: vec2<f32>) -> Cover {
    var p = array<vec2<f32>, 3>(r0.xy, r0.zw, r1.xy);
    let sign = r1.z;
    var e = array<f32, 3>(0.0, 0.0, 0.0);
    var inside = true;
    for (var k = 0u; k < 3u; k++) {
        let a = p[(k + 1u) % 3u];
        let b = p[(k + 2u) % 3u];
        e[k] = edge_fn(a, b, q) * sign;
        if e[k] < 0.0 || (e[k] == 0.0 && !owns((b - a) * sign)) {
            inside = false;
        }
    }
    return Cover(vec3<f32>(e[0], e[1], e[2]), inside);
}

fn srgb_decode(x_in: f32) -> f32 {
    let x = clamp(x_in, 0.0, 1.0);
    if x <= 0.04045 { return x / 12.92; }
    return pow((x + 0.055) / 1.055, 2.4);
}

fn group_at(off: i32, w: i32, h: i32, x: i32, y: i32) -> vec4<f32> {
    return b_groups[u32(off + clamp(y, 0, h - 1) * w + clamp(x, 0, w - 1))];
}

// Mirrors `sample_group` in mesh3d.rs.
fn sample_group(prim: Prim, g: vec2<f32>) -> vec4<f32> {
    let off = i32(bitcast<u32>(prim.paint.x));
    let w = i32(bitcast<u32>(prim.paint.y));
    let h = i32(bitcast<u32>(prim.paint.z));
    let s = g - vec2<f32>(0.5);
    let x0 = floor(s.x);
    let y0 = floor(s.y);
    let fx = s.x - x0;
    let fy = s.y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = group_at(off, w, h, xi, yi);
    let b = group_at(off, w, h, xi + 1, yi);
    let c = group_at(off, w, h, xi, yi + 1);
    let d = group_at(off, w, h, xi + 1, yi + 1);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    return top + (bottom - top) * fy;
}

const TIE: f32 = 2e-5;

// Mirrors `wire_distance` in mesh3d.rs.
fn wire_distance(r0: vec4<f32>, r1: vec4<f32>, e: vec3<f32>, mask: u32) -> f32 {
    var p = array<vec2<f32>, 3>(r0.xy, r0.zw, r1.xy);
    let ev = array<f32, 3>(e.x, e.y, e.z);
    var d = 3.4e38;
    for (var k = 0u; k < 3u; k++) {
        if (mask & (1u << k)) != 0u {
            let a = p[(k + 1u) % 3u];
            let b = p[(k + 2u) % 3u];
            let len = max(sqrt((b.x - a.x) * (b.x - a.x) + (b.y - a.y) * (b.y - a.y)), 1e-6);
            d = min(d, ev[k] / len);
        }
    }
    return d;
}

fn wire_stands(material: u32, r0: vec4<f32>, r1: vec4<f32>, tf: u32, q: vec2<f32>) -> bool {
    if b_mesh[material + 1u].w > 0.0 { return true; }
    let c = cover(r0, r1, q);
    return wire_distance(r0, r1, c.e, tf >> 2u) <= b_mesh[material + 3u].w * 0.5;
}

fn card_at(off: i32, w: i32, h: i32, x: i32, y: i32) -> vec4<f32> {
    if x < 0 || y < 0 || x >= w || y >= h { return vec4<f32>(0.0); }
    return b_groups[u32(off + y * w + x)];
}

// Mirrors `card_sample` in mesh3d.rs: a card's picture at target point q.
fn card_sample(prim: Prim, material: u32, q: vec2<f32>) -> vec4<f32> {
    let r0 = b_mesh[material];
    let r1 = b_mesh[material + 1u];
    let c = vec2<f32>(prim.fill.x + prim.fill.z * q.x, prim.fill.y + prim.fill.w * q.y);
    let g = vec2<f32>((c.x - r0.x) / r0.z - 0.5, (c.y - r0.y) / r0.w - 0.5);
    let off = i32(bitcast<u32>(prim.stroke.z) + bitcast<u32>(r1.x));
    let w = i32(bitcast<u32>(r1.y));
    let h = i32(bitcast<u32>(r1.z));
    let x0 = floor(g.x);
    let y0 = floor(g.y);
    let fx = g.x - x0;
    let fy = g.y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = card_at(off, w, h, xi, yi);
    let b = card_at(off, w, h, xi + 1, yi);
    let cc = card_at(off, w, h, xi, yi + 1);
    let d = card_at(off, w, h, xi + 1, yi + 1);
    let top = a + (b - a) * fx;
    let bottom = cc + (d - cc) * fx;
    return top + (bottom - top) * fy;
}

fn shade_tri(prim: Prim, tri: u32, q: vec2<f32>) -> vec4<f32> {
    let tb = bitcast<u32>(prim.geom2.x) + tri * 4u;
    let vb = bitcast<u32>(prim.geom2.y);
    let mb = bitcast<u32>(prim.geom2.z);
    let lb = bitcast<u32>(prim.geom2.w);
    let mat_flags = u32(b_mesh[mb + bitcast<u32>(b_mesh[tb + 1u].w) * 4u + 2u].w);
    if (mat_flags & 4u) != 0u {
        // A depth-only occluder: another batch draws this surface.
        return vec4<f32>(0.0);
    }
    if (mat_flags & 16u) != 0u {
        return card_sample(prim, mb + bitcast<u32>(b_mesh[tb + 1u].w) * 4u, q);
    }
    if (mat_flags & 32u) != 0u {
        let row = mb + bitcast<u32>(b_mesh[tb + 1u].w) * 4u;
        let wc = cover(b_mesh[tb], b_mesh[tb + 1u], q);
        let on_wire = wire_distance(b_mesh[tb], b_mesh[tb + 1u], wc.e, u32(b_mesh[tb + 2u].w) >> 2u) <= b_mesh[row + 3u].w * 0.5;
        var c = b_mesh[row + 1u];
        if on_wire { c = b_mesh[row]; }
        return vec4<f32>(encode(c.x) * c.w, encode(c.y) * c.w, encode(c.z) * c.w, c.w);
    }
    let cv = cover(b_mesh[tb], b_mesh[tb + 1u], q);
    let ev = array<f32, 3>(cv.e.x, cv.e.y, cv.e.z);
    let ids = b_mesh[tb + 2u];
    let idv = array<u32, 3>(bitcast<u32>(ids.x), bitcast<u32>(ids.y), bitcast<u32>(ids.z));
    var w: array<f32, 3>;
    var pos: array<vec3<f32>, 3>;
    var nrm: array<vec3<f32>, 3>;
    for (var k = 0u; k < 3u; k++) {
        let v = idv[k];
        let a = b_mesh[vb + v * 2u];
        pos[k] = a.xyz;
        nrm[k] = b_mesh[vb + v * 2u + 1u].xyz;
        w[k] = max(ev[k], 0.0) * a.w;
    }
    let s = max(w[0] + w[1] + w[2], 1e-30);
    let b = vec3<f32>(w[0] / s, w[1] / s, w[2] / s);
    let p = pos[0] * b.x + pos[1] * b.y + pos[2] * b.z;
    let navg = nrm[0] * b.x + nrm[1] * b.y + nrm[2] * b.z;
    let mat = bitcast<u32>(b_mesh[tb + 1u].w);
    let m0 = b_mesh[mb + mat * 4u];
    let m1 = b_mesh[mb + mat * 4u + 1u];
    let m2 = b_mesh[mb + mat * 4u + 2u];
    let m3 = b_mesh[mb + mat * 4u + 3u];
    let flags = u32(m2.w);
    var n = navg / max(sqrt(dot(navg, navg)), 1e-20);
    if (flags & 2u) != 0u {
        let gc = cross(pos[1] - pos[0], pos[2] - pos[0]);
        var g = gc / max(sqrt(dot(gc, gc)), 1e-20);
        if dot(g, navg) < 0.0 { g = -g; }
        n = g;
    }
    let vv = -p;
    let v = vv / max(sqrt(dot(vv, vv)), 1e-20);
    if dot(n, v) < 0.0 { n = -n; }
    let base = m0.xyz;
    let opacity = m0.w;
    let emissive = m2.xyz;
    if (flags & 8u) != 0u {
        var blocked = 0.0;
        var total = 0.0;
        for (var i = 0u; i < u32(b_mesh[lb].x); i++) {
            let off = shadow_of(lb, i);
            if off != 0 {
                let w = dot(b_mesh[lb + 7u + i * 2u + 1u].xyz, vec3<f32>(0.2126, 0.7152, 0.0722));
                blocked += w * (1.0 - shadow(u32(i32(lb) + off), p, n));
                total += w;
            }
        }
        var sa = 0.0;
        if total > 0.0 { sa = blocked / total * opacity; }
        return vec4<f32>(encode(base.x) * sa, encode(base.y) * sa, encode(base.z) * sa, sa);
    }
    if (flags & 1u) != 0u {
        let c = base + emissive;
        return vec4<f32>(encode(c.x) * opacity, encode(c.y) * opacity, encode(c.z) * opacity, opacity);
    }
    let metal = m1.x;
    let rough = max(m1.y, 0.03);
    let cc = m1.z;
    let ccr = max(m1.w, 0.03);
    let trans = m3.x;
    let f0 = mix(vec3<f32>(0.04), base, metal);
    let diffuse = base * ((1.0 - metal) * (1.0 - trans));
    let n_v = max(dot(n, v), 1e-4);
    let fc_v = 0.04 + 0.96 * pow5(1.0 - n_v);
    let coat = 1.0 - cc * fc_v;
    var d_light = vec3<f32>(0.0);
    var s_light = vec3<f32>(0.0);
    let lights = u32(b_mesh[lb].x);
    for (var i = 0u; i < lights; i++) {
        let row = b_mesh[lb + 7u + i * 2u];
        var col = b_mesh[lb + 7u + i * 2u + 1u].xyz;
        let kind = u32(row.w);
        if kind == 2u {
            d_light = d_light + diffuse * col;
            continue;
        }
        var l = row.xyz;
        if kind == 1u {
            let dl = row.xyz - p;
            l = dl / max(sqrt(dot(dl, dl)), 1e-20);
        }
        let n_l = dot(n, l);
        if n_l <= 0.0 { continue; }
        let soff = shadow_of(lb, i);
        if soff != 0 { col = col * shadow(u32(i32(lb) + soff), p, n); }
        let hh = l + v;
        let h = hh / max(sqrt(dot(hh, hh)), 1e-20);
        let n_h = max(dot(n, h), 0.0);
        let v_h = max(dot(v, h), 0.0);
        let fw = pow5(1.0 - v_h);
        let f = f0 + (vec3<f32>(1.0) - f0) * fw;
        let spec = ggx(n_h, rough * rough) * smith(n_v, n_l, rough) / max(4.0 * n_v * n_l, 1e-4) * PI;
        let kd = vec3<f32>(1.0) - f;
        d_light = d_light + (kd * diffuse) * (col * (n_l * coat));
        s_light = s_light + (f * (spec * n_l * coat)) * col;
        if cc > 0.0 {
            let fc = 0.04 + 0.96 * fw;
            let sc = ggx(n_h, ccr * ccr) * smith(n_v, n_l, ccr) / max(4.0 * n_v * n_l, 1e-4) * PI;
            s_light = s_light + col * (sc * fc * cc * n_l);
        }
    }
    let nw = to_world(lb, n);
    let r_view = n * (2.0 * dot(n, v)) - v;
    let rw = to_world(lb, r_view);
    d_light = d_light + (diffuse * env_light(lb, nw, 1.0)) * coat;
    let ab = env_brdf(rough, n_v);
    let spec_env = env_light(lb, rw, rough) * (f0 * ab.x + vec3<f32>(ab.y));
    s_light = s_light + spec_env * coat;
    if cc > 0.0 {
        s_light = s_light + env_light(lb, rw, ccr) * (fc_v * cc);
    }
    if trans > 0.0 && prim.paint.w > 0.5 {
        let ior = max(m3.y, 1.0);
        let i = -v;
        let eta = 1.0 / ior;
        let cosi = clamp(dot(n, v), 0.0, 1.0);
        let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
        var t: vec3<f32>;
        if k < 0.0 { t = i - n * (2.0 * dot(n, i)); } else { t = i * eta + n * (eta * cosi - sqrt(k)); }
        let exit = p + t * m3.z;
        let z = max(exit.z, 1.0);
        let d = vec2<f32>(prim.stroke.x * (exit.x / z - p.x / p.z), prim.stroke.y * (exit.y / z - p.y / p.z));
        let g = vec2<f32>(prim.paint2.x + prim.paint2.z * (q.x + d.x), prim.paint2.y + prim.paint2.w * (q.y + d.y));
        let behind = sample_group(prim, g);
        let gf0 = ((ior - 1.0) / (ior + 1.0)) * ((ior - 1.0) / (ior + 1.0));
        let fr = gf0 + (1.0 - gf0) * pow5(1.0 - n_v);
        var seen = vec3<f32>(0.0);
        if behind.w > 0.0 {
            seen = vec3<f32>(srgb_decode(behind.x / behind.w) * base.x, srgb_decode(behind.y / behind.w) * base.y, srgb_decode(behind.z / behind.w) * base.z);
        }
        let through = trans * (1.0 - fr) * behind.w;
        let glit = seen * through + (d_light + emissive) + s_light;
        let gpeak = min(max(max(s_light.x, s_light.y), s_light.z), 1.0);
        let ga = clamp(through + (1.0 - trans) + max(1.0 - through - (1.0 - trans), 0.0) * gpeak, 0.0, 1.0);
        if ga <= 0.0 { return vec4<f32>(0.0); }
        let gc = vec3<f32>(encode(shoulder(glit.x / ga)), encode(shoulder(glit.y / ga)), encode(shoulder(glit.z / ga)));
        let gao = ga * opacity;
        return vec4<f32>(gc * gao, gao);
    }
    let body = opacity * (1.0 - trans * (1.0 - fc_v));
    let sl = s_light * opacity;
    let lit = (d_light + emissive) * body + sl;
    let peak = min(max(max(sl.x, sl.y), sl.z), 1.0);
    let a = clamp(body + (1.0 - body) * peak, 0.0, 1.0);
    if a <= 0.0 { return vec4<f32>(0.0); }
    let c = vec3<f32>(encode(shoulder(lit.x / a)), encode(shoulder(lit.y / a)), encode(shoulder(lit.z / a)));
    return vec4<f32>(c * a, a);
}

fn shade_mesh(prim: Prim, p: vec2<f32>) -> vec4<f32> {
    let tx = u32(p.x) / 16u;
    let ty = u32(p.y) / 16u;
    let tx0 = bitcast<u32>(prim.geom.y);
    let ty0 = bitcast<u32>(prim.geom.z);
    let tw = bitcast<u32>(prim.geom.w);
    if tx < tx0 || ty < ty0 || tx - tx0 >= tw { return vec4<f32>(0.0); }
    let base = bitcast<u32>(prim.geom.x);
    let k = (ty - ty0) * tw + (tx - tx0);
    let start = base + b_bins[base + k * 2u];
    let count = b_bins[base + k * 2u + 1u];
    let tb = bitcast<u32>(prim.geom2.x);
    var best: array<u32, 8>;
    var depth: array<f32, 8>;
    for (var s = 0u; s < 8u; s++) {
        best[s] = NO_TRI;
        depth[s] = 0.0;
    }
    for (var j = 0u; j < count; j++) {
        let tri = b_bins[start + j];
        let r = tb + tri * 4u;
        let r0 = b_mesh[r];
        let r1 = b_mesh[r + 1u];
        let d = b_mesh[r + 3u];
        let c = cover(r0, r1, p);
        let ca = array<f32, 3>(c.e.x, c.e.y, c.e.z);
        var pts = array<vec2<f32>, 3>(r0.xy, r0.zw, r1.xy);
        var reject = false;
        for (var k = 0u; k < 3u; k++) {
            let ea = pts[(k + 1u) % 3u];
            let eb = pts[(k + 2u) % 3u];
            if ca[k] < -(abs(eb.x - ea.x) + abs(eb.y - ea.y)) * 0.4375 - 1e-3 { reject = true; }
        }
        if reject { continue; }
        for (var s = 0u; s < 8u; s++) {
            let q = vec2<f32>(p.x + SAMPLE_X[s] / 16.0, p.y + SAMPLE_Y[s] / 16.0);
            if cover(r0, r1, q).inside {
                let z = d.x * q.x + d.y * q.y + d.z;
                // Later surfaces win ties; cards only where their picture is.
                if z >= depth[s] * (1.0 - TIE) {
                    var stands = true;
                    let tf = u32(b_mesh[r + 2u].w);
                    let material = bitcast<u32>(prim.geom2.z) + bitcast<u32>(r1.w) * 4u;
                    if (tf & 1u) != 0u {
                        stands = card_sample(prim, material, q).w >= 1.0 / 512.0; // CARD_STANDS
                    } else if (tf & 2u) != 0u {
                        stands = wire_stands(material, r0, r1, tf, q);
                    }
                    if stands {
                        depth[s] = z;
                        best[s] = tri;
                    }
                }
            }
        }
    }
    var out = vec4<f32>(0.0);
    for (var s = 0u; s < 8u; s++) {
        let tri = best[s];
        if tri == NO_TRI { continue; }
        var seen = false;
        for (var j = 0u; j < s; j++) {
            if best[j] == tri { seen = true; }
        }
        if seen { continue; }
        var n = 0.0;
        var q = vec2<f32>(0.0);
        for (var j = s; j < 8u; j++) {
            if best[j] == tri {
                n += 1.0;
                q = q + vec2<f32>(SAMPLE_X[j], SAMPLE_Y[j]);
            }
        }
        let centroid = vec2<f32>(p.x + q.x / (16.0 * n), p.y + q.y / (16.0 * n));
        out = out + shade_tri(prim, tri, centroid) * (n / 8.0);
    }
    return out * prim.inv_y.w;
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
        var s: vec4<f32>;
        if prim.kind.x == 4u { s = shade_mesh(prim, p); } else { s = shade(prim, p); }
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
