// Path tracer (export). One invocation per group texel: `spp` paths through
// the scene's top-level structure, shaded with the raster's material model
// (Lambert + GGX/Smith, clearcoat, environment from sky/horizon/ground and
// lobes) plus what rasterising cannot give: shadows from every light by ray,
// bounced light, reflections of the scene, refraction through glass.
//
// Conventions shared with mesh3d.rs: positions in camera space (camera at
// the origin looking down +z, px units); light colours are irradiance, so a
// delta light adds f·π·colour·cos; the environment is radiance; output is
// premultiplied sRGB after the raster's soft shoulder.

struct Uniforms {
    size: vec4<u32>,      // group w, h, texel offset, spp
    opts: vec4<u32>,      // bounces, seed, card instance id, cards
    cam: vec4<f32>,       // focal, centre x, centre y, manual commit (1: see cast_ray)
    place: vec4<f32>,     // group origin x, y, factor x, y (target px)
    root0: vec4<f32>,
    root1: vec4<f32>,
    root2: vec4<f32>,
    inv0: vec4<f32>,
    inv1: vec4<f32>,
    inv2: vec4<f32>,
}

var<uniform> u: Uniforms;
var acc: acceleration_structure;
var<storage, read> b_inst: array<vec4<f32>>;
var<storage, read> b_attr: array<vec4<f32>>;
var<storage, read> b_idx: array<u32>;
var<storage, read> b_tag: array<u32>;
var<storage, read> b_mat: array<vec4<f32>>;
var<storage, read> b_light: array<vec4<f32>>;
var<storage, read> b_card: array<vec4<f32>>;
var<storage, read_write> b_groups: array<vec4<f32>>;
var<storage, read_write> b_guides: array<vec4<f32>>;

const PI: f32 = 3.14159265;
const FLAG_UNLIT: u32 = 1u;
const FLAG_FACETED: u32 = 2u;
const FLAG_SHADOW_CATCHER: u32 = 8u;
const FLAG_WIRE: u32 = 32u;
const TWO_SIDED: u32 = 0x80000000u;

// ---------------------------------------------------------------- random

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

struct Rng { s: u32 }

fn rnd(r: ptr<function, Rng>) -> f32 {
    (*r).s = pcg((*r).s);
    return f32((*r).s >> 8u) / 16777216.0;
}

// ---------------------------------------------------------------- helpers

fn luminance(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

fn encode(x_in: f32) -> f32 {
    let x = clamp(x_in, 0.0, 1.0);
    if x <= 0.0031308 { return x * 12.92; }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

fn decode(x_in: f32) -> f32 {
    let x = clamp(x_in, 0.0, 1.0);
    if x <= 0.04045 { return x / 12.92; }
    return pow((x + 0.055) / 1.055, 2.4);
}

fn shoulder(x: f32) -> f32 {
    if x < 0.8 { return x; }
    return 0.8 + 0.2 * (1.0 - exp(-(x - 0.8) / 0.2));
}

fn ggx_d(n_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-12);
}

fn smith(n_v: f32, n_l: f32, r: f32) -> f32 {
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    return n_v / (n_v * (1.0 - k) + k) * (n_l / (n_l * (1.0 - k) + k));
}

fn basis(n: vec3<f32>) -> mat3x3<f32> {
    var t = vec3<f32>(1.0, 0.0, 0.0);
    if abs(n.x) > 0.9 { t = vec3<f32>(0.0, 1.0, 0.0); }
    let b = normalize(cross(n, t));
    let tt = cross(b, n);
    return mat3x3<f32>(tt, b, n);
}

fn to_world(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(b_light[1].xyz, v), dot(b_light[2].xyz, v), dot(b_light[3].xyz, v));
}

// Environment radiance seen along camera-space direction `d` (sharp):
// sky/horizon/ground by height, plus the lobes, plus ambient lights (a
// uniform environment).
fn env(d: vec3<f32>) -> vec3<f32> {
    let w = to_world(d);
    let up = -w.y;
    let sky = b_light[4].xyz;
    let horizon = b_light[5].xyz;
    let ground = b_light[6].xyz;
    var c: vec3<f32>;
    if up >= 0.0 { c = mix(horizon, sky, smoothstep(0.0, 0.6, up)); } else { c = mix(horizon, ground, smoothstep(0.0, 0.2, -up)); }
    let lights = u32(b_light[0].x);
    let lobes = u32(b_light[0].y);
    let first = 7u + lights * 2u;
    for (var i = 0u; i < lobes; i++) {
        let l = b_light[first + i * 2u];
        let s2 = max(l.w * l.w, 1e-6);
        c += b_light[first + i * 2u + 1u].xyz * exp((dot(w, l.xyz) - 1.0) / s2);
    }
    for (var i = 0u; i < lights; i++) {
        if u32(b_light[7u + i * 2u].w) == 2u { c += b_light[7u + i * 2u + 1u].xyz; }
    }
    return c;
}

// ---------------------------------------------------------------- scene

struct Hit {
    found: bool,
    card: bool,
    t: f32,
    p: vec3<f32>,
    n: vec3<f32>,      // shading normal, facing the ray
    g: vec3<f32>,      // geometric normal, facing the ray
    front: bool,       // entered from the outside
    mat: u32,          // material row index (x4) or card index
    two_sided: bool,
    card_color: vec4<f32>,
}

fn card_at(off: i32, w: i32, h: i32, x: i32, y: i32) -> vec4<f32> {
    if x < 0 || y < 0 || x >= w || y >= h || u32(off + y * w + x) >= arrayLength(&b_groups) { return vec4<f32>(0.0); }
    return b_groups[u32(off + y * w + x)];
}

// A card's picture (premultiplied sRGB) at camera-space point `p`.
fn card_picture(card: u32, p: vec3<f32>) -> vec4<f32> {
    if card * 2u + 1u >= arrayLength(&b_card) { return vec4<f32>(0.0); }
    let r0 = b_card[card * 2u];
    let r1 = b_card[card * 2u + 1u];
    // Camera space → canvas → target, as the raster projects it.
    let s = vec3<f32>(u.cam.x * p.x / p.z + u.cam.y, u.cam.x * p.y / p.z + u.cam.z, 1.0);
    let q = vec3<f32>(dot(u.root0.xyz, s), dot(u.root1.xyz, s), dot(u.root2.xyz, s));
    let tp = q.xy / q.z;
    let g = vec2<f32>((tp.x - r0.x) / r0.z - 0.5, (tp.y - r0.y) / r0.w - 0.5);
    let off = i32(bitcast<u32>(r1.x));
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
    let c = card_at(off, w, h, xi, yi + 1);
    let d = card_at(off, w, h, xi + 1, yi + 1);
    return mix(mix(a, b, fx), mix(c, d, fx), fy);
}

fn cast_ray(o: vec3<f32>, d: vec3<f32>, tmax: f32, rng: ptr<function, Rng>) -> Hit {
    var rq: ray_query;
    // Manual commit: some drivers (Mesa lavapipe) report the committed hit
    // with the instance of the last box the ray entered, not of the
    // triangle it hit. There every triangle is a candidate, and the hit is
    // the last confirmed candidate, recorded here.
    let manual = u.cam.w > 0.5;
    var ray_flags = RAY_FLAG_NONE;
    if manual { ray_flags = RAY_FLAG_FORCE_NO_OPAQUE; }
    rayQueryInitialize(&rq, acc, RayDesc(ray_flags, 0xFFu, 0.0, tmax, o, d));
    // What the hit needs, recorded field by field: on lavapipe, reading the
    // committed intersection (or a whole copied candidate) crashes the JIT.
    var kind = RAY_QUERY_INTERSECTION_NONE;
    var hit_t = 0.0;
    var custom = 0u;
    var prim = 0u;
    var bary = vec2<f32>(0.0);
    var w2o = mat4x3<f32>();
    while rayQueryProceed(&rq) {
        let c = rayQueryGetCandidateIntersection(&rq);
        if c.kind != RAY_QUERY_INTERSECTION_TRIANGLE { continue; }
        var take = true;
        if c.instance_custom_data == u.opts.z {
            // Cards are not opaque: a candidate stands where its picture
            // does (stochastically by its alpha, so soft edges average out).
            let pic = card_picture(c.primitive_index / 2u, o + d * c.t);
            take = pic.w > 0.0 && pic.w >= rnd(rng);
        }
        if take {
            rayQueryConfirmIntersection(&rq);
            if manual {
                kind = c.kind;
                hit_t = c.t;
                custom = c.instance_custom_data;
                prim = c.primitive_index;
                bary = c.barycentrics;
                w2o = c.world_to_object;
            }
        }
    }
    if !manual {
        let h = rayQueryGetCommittedIntersection(&rq);
        kind = h.kind;
        hit_t = h.t;
        custom = h.instance_custom_data;
        prim = h.primitive_index;
        bary = h.barycentrics;
        w2o = h.world_to_object;
    }
    var hit: Hit;
    hit.found = kind != RAY_QUERY_INTERSECTION_NONE;
    if !hit.found { return hit; }
    hit.t = hit_t;
    hit.p = o + d * hit_t;
    if custom == u.opts.z {
        hit.card = true;
        hit.mat = prim / 2u;
        hit.card_color = card_picture(hit.mat, hit.p);
        hit.n = -d;
        hit.g = -d;
        hit.front = true;
        return hit;
    }
    // Every index is checked: a driver reporting a wrong instance or
    // primitive must not read past the buffers (that is a crash).
    let inst = custom * 4u;
    if inst + 3u >= arrayLength(&b_inst) { hit.found = false; return hit; }
    let r0 = b_inst[inst];
    let attr = bitcast<u32>(r0.x);
    let ib = bitcast<u32>(r0.y) + prim * 3u;
    let ti = bitcast<u32>(r0.z) + prim;
    if ib + 2u >= arrayLength(&b_idx) || ti >= arrayLength(&b_tag) { hit.found = false; return hit; }
    let tag = b_tag[ti];
    let i0 = b_idx[ib];
    let i1 = b_idx[ib + 1u];
    let i2 = b_idx[ib + 2u];
    let na = arrayLength(&b_attr);
    if (attr + max(i0, max(i1, i2))) * 2u + 1u >= na { hit.found = false; return hit; }
    let bw = vec3<f32>(1.0 - bary.x - bary.y, bary.x, bary.y);
    let p0 = b_attr[(attr + i0) * 2u].xyz;
    let p1 = b_attr[(attr + i1) * 2u].xyz;
    let p2 = b_attr[(attr + i2) * 2u].xyz;
    let n_obj = b_attr[(attr + i0) * 2u + 1u].xyz * bw.x + b_attr[(attr + i1) * 2u + 1u].xyz * bw.y + b_attr[(attr + i2) * 2u + 1u].xyz * bw.z;
    // Normals go through the inverse transpose: n · world_to_object.
    let m3 = mat3x3<f32>(w2o[0], w2o[1], w2o[2]);
    var g = normalize(cross(p1 - p0, p2 - p0) * m3);
    var n = normalize(n_obj * m3);
    // Material: part and segment from the tag.
    let part = tag & 0xffu;
    let seg = (tag >> 8u) & 0xffffu;
    let row = b_inst[inst + 1u + part / 2u];
    var base: u32;
    var count: u32;
    if part % 2u == 0u { base = bitcast<u32>(row.x); count = bitcast<u32>(row.y); } else { base = bitcast<u32>(row.z); count = bitcast<u32>(row.w); }
    hit.mat = (base + seg % max(count, 1u)) * 4u;
    if hit.mat + 3u >= arrayLength(&b_mat) { hit.found = false; return hit; }
    hit.two_sided = (tag & TWO_SIDED) != 0u;
    let flags = u32(b_mat[hit.mat + 2u].w);
    if (flags & FLAG_FACETED) != 0u { n = g; }
    // Which side was hit: by the geometric normal against the ray.
    hit.front = dot(g, d) < 0.0;
    if !hit.front { g = -g; }
    if dot(n, g) < 0.0 { n = -n; }
    hit.g = g;
    hit.n = n;
    return hit;
}

// Whether anything blocks the segment from `o` along `d` up to `tmax`.
fn occluded(o: vec3<f32>, d: vec3<f32>, tmax: f32, rng: ptr<function, Rng>) -> bool {
    let h = cast_ray(o, d, tmax, rng);
    if !h.found { return false; }
    if h.card { return true; }
    // Glass and see-through surfaces let some light by; the shadow catcher
    // casts nothing.
    let flags = u32(b_mat[h.mat + 2u].w);
    if (flags & FLAG_SHADOW_CATCHER) != 0u || (flags & FLAG_WIRE) != 0u { return false; }
    let opacity = b_mat[h.mat].w * (1.0 - b_mat[h.mat + 3u].x);
    return opacity >= rnd(rng);
}

fn offset(h: Hit, toward: vec3<f32>) -> vec3<f32> {
    let eps = 0.02 + h.t * 1e-5;
    if dot(toward, h.g) >= 0.0 { return h.p + h.g * eps; }
    return h.p - h.g * eps;
}

// ---------------------------------------------------------------- shading

struct Surface {
    base: vec3<f32>,
    metal: f32,
    rough: f32,
    coat: f32,
    coat_rough: f32,
    trans: f32,
}

// BRDF · cos for light from `l` seen from `v` (no transmission).
fn brdf_cos(s: Surface, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let n_l = dot(n, l);
    let n_v = max(dot(n, v), 1e-4);
    if n_l <= 0.0 { return vec3<f32>(0.0); }
    let h = normalize(l + v);
    let n_h = max(dot(n, h), 0.0);
    let v_h = max(dot(v, h), 0.0);
    let f0 = mix(vec3<f32>(0.04), s.base, s.metal);
    let fw = pow(1.0 - v_h, 5.0);
    let f = f0 + (1.0 - f0) * fw;
    let a = s.rough * s.rough;
    let spec = f * ggx_d(n_h, a) * smith(n_v, n_l, s.rough) / max(4.0 * n_v * n_l, 1e-4);
    let fc_v = 0.04 + 0.96 * pow(1.0 - n_v, 5.0);
    let coat = 1.0 - s.coat * fc_v;
    let diffuse = s.base * (1.0 - s.metal) * (1.0 - s.trans) * (vec3<f32>(1.0) - f) / PI;
    var out = (diffuse + spec) * coat * n_l;
    if s.coat > 0.0 {
        let fc = 0.04 + 0.96 * fw;
        let ca = s.coat_rough * s.coat_rough;
        out += vec3<f32>(fc * s.coat * ggx_d(n_h, ca) * smith(n_v, n_l, s.coat_rough) / max(4.0 * n_v * n_l, 1e-4) * n_l);
    }
    return out;
}

fn spec_weight(s: Surface, n_v: f32) -> f32 {
    let f0 = mix(vec3<f32>(0.04), s.base, s.metal);
    let fs = luminance(f0 + (1.0 - f0) * pow(1.0 - n_v, 5.0));
    let fd = luminance(s.base) * (1.0 - s.metal) * (1.0 - s.trans);
    return clamp(fs / max(fs + fd, 1e-4), 0.1, 0.9);
}

fn ggx_pdf(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, rough: f32) -> f32 {
    let h = normalize(l + v);
    let n_h = max(dot(n, h), 0.0);
    let v_h = max(dot(v, h), 1e-4);
    return ggx_d(n_h, rough * rough) * n_h / (4.0 * v_h);
}

// Samples a reflected direction: cosine-weighted diffuse or a GGX lobe,
// returning (direction, combined pdf of both strategies).
fn sample_brdf(s: Surface, n: vec3<f32>, v: vec3<f32>, rng: ptr<function, Rng>) -> vec4<f32> {
    let tbn = basis(n);
    let ps = spec_weight(s, max(dot(n, v), 1e-4));
    var l: vec3<f32>;
    let u1 = rnd(rng);
    let u2 = rnd(rng);
    if rnd(rng) < ps {
        let a = s.rough * s.rough;
        let theta = atan(a * sqrt(u1 / max(1.0 - u1, 1e-6)));
        let phi = 2.0 * PI * u2;
        let h = tbn * vec3<f32>(sin(theta) * cos(phi), sin(theta) * sin(phi), cos(theta));
        l = reflect(-v, h);
    } else {
        let r = sqrt(u1);
        let phi = 2.0 * PI * u2;
        l = tbn * vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(1.0 - u1, 0.0)));
    }
    let n_l = dot(n, l);
    if n_l <= 0.0 { return vec4<f32>(l, 0.0); }
    let pdf = ps * ggx_pdf(n, v, l, s.rough) + (1.0 - ps) * n_l / PI;
    return vec4<f32>(l, pdf);
}

// Light arriving at `h` from the delta lights (directional and point),
// with shadow rays where the light casts shadows.
fn direct(s: Surface, h: Hit, v: vec3<f32>, rng: ptr<function, Rng>) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        let row = b_light[7u + i * 2u];
        let col = b_light[7u + i * 2u + 1u];
        let kind = u32(row.w);
        if kind == 2u { continue; }
        var l: vec3<f32>;
        var dist = 1e30;
        if kind == 1u {
            let to = row.xyz - h.p;
            dist = length(to);
            l = to / max(dist, 1e-6);
        } else {
            l = normalize(row.xyz);
        }
        if dot(h.n, l) <= 0.0 { continue; }
        if col.w > 0.5 && occluded(offset(h, l), l, dist * 0.999, rng) { continue; }
        out += brdf_cos(s, h.n, v, l) * PI * col.xyz;
    }
    return out;
}

// What fraction of the shadow-casting light reaches a shadow catcher.
fn catcher_shadow(h: Hit, rng: ptr<function, Rng>) -> f32 {
    var blocked = 0.0;
    var total = 0.0;
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        let row = b_light[7u + i * 2u];
        let col = b_light[7u + i * 2u + 1u];
        if u32(row.w) == 2u || col.w < 0.5 { continue; }
        var l = normalize(row.xyz);
        var dist = 1e30;
        if u32(row.w) == 1u {
            let to = row.xyz - h.p;
            dist = length(to);
            l = to / max(dist, 1e-6);
        }
        let w = luminance(col.xyz);
        total += w;
        if occluded(offset(h, l), l, dist * 0.999, rng) { blocked += w; }
    }
    if total <= 0.0 { return 0.0; }
    return blocked / total;
}

// ---------------------------------------------------------------- paths

struct Sample {
    color: vec3<f32>,   // linear radiance, premultiplied by coverage
    alpha: f32,         // 1 where the path ends on something
    albedo: vec3<f32>,
    normal: vec3<f32>,
    depth: f32,
}

fn path(o_in: vec3<f32>, d_in: vec3<f32>, rng: ptr<function, Rng>) -> Sample {
    var out: Sample;
    var o = o_in;
    var d = d_in;
    var throughput = vec3<f32>(1.0);
    // Only see-through events so far (glass, opacity): a path that then
    // leaves the scene shows what is behind the layer list, not the sky.
    var through = true;
    let bounces = u.opts.x;
    for (var bounce = 0u; bounce <= bounces; bounce++) {
        let h = cast_ray(o, d, 1e30, rng);
        if !h.found {
            if !through {
                out.color += throughput * env(d);
                out.alpha = 1.0;
            }
            return out;
        }
        if bounce == 0u { out.depth = h.t; out.normal = h.n; }
        if h.card {
            let c = h.card_color;
            let straight = vec3<f32>(decode(c.x / c.w), decode(c.y / c.w), decode(c.z / c.w));
            out.color += throughput * straight;
            out.alpha = 1.0;
            if bounce == 0u { out.albedo = straight; }
            return out;
        }
        let m0 = b_mat[h.mat];
        let m1 = b_mat[h.mat + 1u];
        let m2 = b_mat[h.mat + 2u];
        let m3 = b_mat[h.mat + 3u];
        let flags = u32(m2.w);
        let v = -d;
        if bounce == 0u { out.albedo = m0.xyz; }
        if (flags & FLAG_SHADOW_CATCHER) != 0u {
            if through {
                // Only the shadows: their darkness is this sample's coverage.
                let a = catcher_shadow(h, rng) * m0.w;
                out.color += throughput * m0.xyz * a;
                out.alpha = a;
                return out;
            }
            o = offset(h, d);
            continue;
        }
        if (flags & FLAG_WIRE) != 0u {
            // Wires: their fill face if they have one (edges come later).
            if m1.w > 0.0 && rnd(rng) < m1.w {
                out.color += throughput * m1.xyz;
                out.alpha = 1.0;
                return out;
            }
            o = offset(h, d);
            continue;
        }
        // Partly opaque layers: pass through in proportion.
        if rnd(rng) >= m0.w {
            o = offset(h, d);
            continue;
        }
        if (flags & FLAG_UNLIT) != 0u {
            out.color += throughput * (m0.xyz + m2.xyz);
            out.alpha = 1.0;
            return out;
        }
        out.alpha = 1.0;
        var s: Surface;
        s.base = m0.xyz;
        s.metal = m1.x;
        s.rough = max(m1.y, 0.03);
        s.coat = m1.z;
        s.coat_rough = max(m1.w, 0.03);
        s.trans = m3.x;
        out.color += throughput * m2.xyz;
        // Glass: a dielectric boundary, reflect or refract by Fresnel.
        if s.trans > 0.0 && rnd(rng) < s.trans {
            let ior = max(m3.y, 1.0);
            var eta = 1.0 / ior;
            if !h.front { eta = ior; }
            var nn = h.n;
            if s.rough > 0.04 {
                // Frosted: a microfacet normal from the GGX distribution.
                let a = s.rough * s.rough;
                let u1 = rnd(rng);
                let theta = atan(a * sqrt(u1 / max(1.0 - u1, 1e-6)));
                let phi = 2.0 * PI * rnd(rng);
                nn = normalize(basis(h.n) * vec3<f32>(sin(theta) * cos(phi), sin(theta) * sin(phi), cos(theta)));
            }
            let cosi = clamp(dot(nn, v), 0.0, 1.0);
            let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
            let f0 = ((1.0 - eta) / (1.0 + eta)) * ((1.0 - eta) / (1.0 + eta));
            let fr = f0 + (1.0 - f0) * pow(1.0 - cosi, 5.0);
            if k < 0.0 || rnd(rng) < fr {
                d = reflect(d, nn);
                through = false;
            } else {
                d = normalize(eta * d + (eta * cosi - sqrt(k)) * nn);
                // Tinted by its colour on the way in.
                if h.front { throughput *= s.base; }
            }
            o = offset(h, d);
            continue;
        }
        through = false;
        out.color += throughput * direct(s, h, v, rng);
        let smp = sample_brdf(s, h.n, v, rng);
        if smp.w <= 0.0 { return out; }
        let l = smp.xyz;
        throughput *= brdf_cos(s, h.n, v, l) / smp.w;
        // Russian roulette after a few bounces.
        if bounce >= 3u {
            let q = clamp(max(throughput.x, max(throughput.y, throughput.z)), 0.05, 0.95);
            if rnd(rng) > q { return out; }
            throughput /= q;
        }
        d = l;
        o = offset(h, l);
    }
    return out;
}

@compute @workgroup_size(8, 8, 1)
fn trace(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u.size.x;
    let h = u.size.y;
    if id.x >= w || id.y >= h { return; }
    let spp = u.size.w;
    var rng: Rng;
    rng.s = pcg(id.x + pcg(id.y + pcg(u.opts.y)));
    var color = vec3<f32>(0.0);
    var alpha = 0.0;
    var first: Sample;
    for (var s = 0u; s < spp; s++) {
        // A point in the texel (stratified jitter), target → canvas → ray.
        let jx = (f32(s % 4u) + rnd(&rng)) / 4.0;
        let jy = (f32((s / 4u) % 4u) + rnd(&rng)) / 4.0;
        let tp = vec3<f32>(u.place.x + u.place.z * (f32(id.x) + jx), u.place.y + u.place.w * (f32(id.y) + jy), 1.0);
        let q = vec3<f32>(dot(u.inv0.xyz, tp), dot(u.inv1.xyz, tp), dot(u.inv2.xyz, tp));
        let c = q.xy / q.z;
        let dir = normalize(vec3<f32>((c.x - u.cam.y) / u.cam.x, (c.y - u.cam.z) / u.cam.x, 1.0));
        let smp = path(vec3<f32>(0.0), dir, &rng);
        color += smp.color;
        alpha += smp.alpha;
        if s == 0u { first = smp; }
    }
    color /= f32(spp);
    alpha /= f32(spp);
    // Highlights on glass over nothing still need coverage to show.
    let peak = min(max(color.x, max(color.y, color.z)), 1.0);
    let a = max(alpha, peak);
    var out = vec4<f32>(0.0);
    if a > 0.0 {
        let straight = color / a;
        out = vec4<f32>(encode(shoulder(straight.x)) * a, encode(shoulder(straight.y)) * a, encode(shoulder(straight.z)) * a, a);
    }
    let i = id.y * w + id.x;
    b_groups[u.size.z + i] = out;
    b_guides[i * 3u] = vec4<f32>(first.albedo, alpha);
    b_guides[i * 3u + 1u] = vec4<f32>(first.normal, 0.0);
    b_guides[i * 3u + 2u] = vec4<f32>(first.depth, 0.0, 0.0, 0.0);
}
