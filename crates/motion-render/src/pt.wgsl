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
    cam: vec4<f32>,       // focal, centre x, centre y, unused
    place: vec4<f32>,     // group origin x, y, factor x, y (target px)
    root0: vec4<f32>,
    root1: vec4<f32>,
    root2: vec4<f32>,
    inv0: vec4<f32>,
    inv1: vec4<f32>,
    inv2: vec4<f32>,
    lens: vec4<f32>,      // aperture radius, focus depth (camera px)
    denoise: vec4<u32>,   // denoiser: step (px), temporal (1), 0, 0
    prev_cam: vec4<f32>,  // previous frame: focal, centre x, y, valid
    prev_place: vec4<f32>,
    prev_root0: vec4<f32>,
    prev_root1: vec4<f32>,
    prev_root2: vec4<f32>,
    band: vec4<u32>,      // rows the trace pass covers: first, end; profiling density repeats
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
// The environment image (envmap.rs): [w, h, sampling w, sampling h], the
// map, the marginal CDF, then per sampling texel [row CDF, probability].
var<storage, read> b_env: array<vec4<f32>>;
// Denoising (per texel two rows: [lighting / albedo (straight), coverage],
// [variance of its luminance, 0, 0, 0]); passes read b_hdr, write b_tmp.
var<storage, read_write> b_hdr: array<vec4<f32>>;
var<storage, read_write> b_tmp: array<vec4<f32>>;
// Temporal history (three rows per texel: [lighting, coverage], [frames,
// camera z, 0, 0], [normal, 0]): last frame's, and this frame's.
var<storage, read_write> b_hist: array<vec4<f32>>;
var<storage, read_write> b_next: array<vec4<f32>>;
// The image pool (premultiplied sRGB RGBA8): mesh image maps.
var<storage, read> b_images: array<u32>;
// Profiling (denoise.z = 1): [ray queries, candidates, empty density
// lookups, 0], counted on one pixel in 64 (profile.rs); then per 8 × 4
// pixel block (the lanes that run in lockstep), every pixel's density
// lookups summed and their maximum; then per material: ray hits, density
// lookups (one pixel in 64).
var<storage, read_write> b_prof: array<atomic<u32>>;
// Noise and grid volumes' density bounds: per volume (material row 30.w =
// offset + 1) BOUNDS_N³ cells over its box, each the most density (a share
// of sigma) found in it, baked each frame by `bounds` from the jobs
// [material row, offset].
var<storage, read_write> b_bounds: array<f32>;
var<storage, read> b_jobs: array<u32>;
const BOUNDS_N: u32 = 32u;
var<private> prof_on: bool;
// The profiling build of this shader sets this true (pt.rs `shader`); with
// it false the compiler drops all counting, so renders carry none of it.
const PROFILE: bool = false;
// Features this build traces (pt.rs `specialise`): each scene compiles
// only what its materials and lights use, so the kernel stays small. All
// on is the general build.
const F_VOLUME: bool = true;        // any volume
const F_VOL_PHYSICAL: bool = true;  // volumes walked exactly (even, physical)
const F_GLASS: bool = true;         // transmission (and dispersion)
const F_SSS: bool = true;           // subsurface walks
const F_CARDS: bool = true;         // flat 3D layers as cards
const F_CATCHER: bool = true;       // shadow catchers
const F_AREA: bool = true;          // area lights
const F_ENV_IMAGE: bool = true;     // an environment image
const F_DETAIL: bool = true;        // surface noise and image maps
// This pixel's density lookups (profiling, every pixel).
var<private> px_lookups: u32;
// Volume scattering events so far on this path (0..3, 3 = three or more):
// header slots 4..7 count ray queries and 8..11 density lookups by it.
var<private> vol_depth: u32;
const PROF_HEADER: u32 = 12u;

// Where the per-material counters start: after the header and the blocks.
fn prof_materials() -> u32 {
    return PROF_HEADER + ((u.size.x + 7u) / 8u) * ((u.size.y + 3u) / 4u) * 2u;
}

fn prof_count(i: u32) {
    if PROFILE && prof_on && i < arrayLength(&b_prof) { atomicAdd(&b_prof[i], 1u); }
}

fn unpack(v: u32) -> vec4<f32> {
    return vec4<f32>(vec4<u32>(v, v >> 8u, v >> 16u, v >> 24u) & vec4<u32>(255u)) / 255.0;
}

const PI: f32 = 3.14159265;
// Rows per material (mesh3d::MAT_ROWS).
const MAT_ROWS: u32 = 34u;
// Rows per instance (pt.rs INST_ROWS).
const INST_ROWS: u32 = 13u;
// Rows per light (mesh3d::LROWS).
const LROWS: u32 = 5u;
const FLAG_UNLIT: u32 = 1u;
const FLAG_FACETED: u32 = 2u;
const FLAG_SHADOW_CATCHER: u32 = 8u;
const FLAG_WIRE: u32 = 32u;
const FLAG_VOLUME: u32 = 64u;
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

// Lit colour → display: exposure, then the scene's curve (0 the standard
// shoulder, 1 Hable filmic, 2 AgX). The light block's rotation rows carry
// exposure (row 1 w) and the curve (row 2 w).
fn hable(v: f32) -> f32 {
    return ((v * (0.15 * v + 0.05) + 0.004) / (v * (0.15 * v + 0.5) + 0.06)) - 0.02 / 0.3;
}

fn agx(c: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(0.84247906, 0.04232824, 0.042375654, 0.0784336, 0.8784686, 0.0784336, 0.079223745, 0.07916613, 0.87914297);
    let outset = mat3x3<f32>(1.196879, -0.05289685, -0.052971635, -0.09802088, 1.1519031, -0.09804345, -0.09902974, -0.09896118, 1.1510737);
    let lo = -12.47393;
    let hi = 4.026069;
    var v = inset * c;
    v = (clamp(log2(max(v, vec3<f32>(1e-10))), vec3<f32>(lo), vec3<f32>(hi)) - vec3<f32>(lo)) / (hi - lo);
    let x2 = v * v;
    let x4 = x2 * x2;
    v = 15.5 * x4 * x2 - 40.14 * x4 * v + 31.96 * x4 - 6.868 * x2 * v + 0.4298 * x2 + 0.1191 * v - vec3<f32>(0.00232);
    return min(pow(max(outset * v, vec3<f32>(0.0)), vec3<f32>(2.2)), vec3<f32>(1.0));
}

fn tone_curve(x: vec3<f32>, kind: u32) -> vec3<f32> {
    if kind == 1u {
        let white = hable(11.2);
        return clamp(vec3<f32>(hable(max(x.x, 0.0) * 2.0), hable(max(x.y, 0.0) * 2.0), hable(max(x.z, 0.0) * 2.0)) / white, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if kind == 2u { return agx(x); }
    return vec3<f32>(shoulder(x.x), shoulder(x.y), shoulder(x.z));
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
// the smooth part (sky/horizon/ground by height, plus ambient lights, a
// uniform environment) and the lobes (softboxes, the sun), which are
// also sampled directly (see env_lobe_*).
fn has_image() -> bool {
    return F_ENV_IMAGE && b_light[0].z > 0.5;
}

// Where world direction w falls on the environment image (u, v in 0..1).
fn env_uv(w: vec3<f32>) -> vec2<f32> {
    let phi = atan2(w.x, w.z) + b_light[4].w;
    return vec2<f32>(fract(0.5 + phi / (2.0 * PI)), acos(clamp(-w.y, -1.0, 1.0)) / PI);
}

// The image's radiance toward camera-space d (bilinear, sharp).
fn env_image(d: vec3<f32>) -> vec3<f32> {
    let uv = env_uv(to_world(d));
    let w = u32(b_env[0].x);
    let h = u32(b_env[0].y);
    let x = uv.x * f32(w) - 0.5;
    let y = clamp(uv.y * f32(h) - 0.5, 0.0, f32(h - 1u));
    let xf = floor(x);
    let fx = x - xf;
    let y0 = u32(floor(y));
    let y1 = min(y0 + 1u, h - 1u);
    let fy = y - floor(y);
    let x0 = u32(i32(xf) + i32(w)) % w;
    let x1 = (x0 + 1u) % w;
    let a = mix(b_env[1u + y0 * w + x0].xyz, b_env[1u + y0 * w + x1].xyz, fx);
    let b = mix(b_env[1u + y1 * w + x0].xyz, b_env[1u + y1 * w + x1].xyz, fx);
    return mix(a, b, fy) * b_light[0].w;
}

// Sampling the image by brightness: the table's place and its pdf.
fn env_image_pdf(d: vec3<f32>) -> f32 {
    let uv = env_uv(to_world(d));
    let gw = u32(b_env[0].z);
    let gh = u32(b_env[0].w);
    let x = min(u32(uv.x * f32(gw)), gw - 1u);
    let y = min(u32(uv.y * f32(gh)), gh - 1u);
    let base = 1u + u32(b_env[0].x) * u32(b_env[0].y);
    let p = b_env[base + gh + y * gw + x].y;
    let st = sin(uv.y * PI);
    if st <= 1e-6 { return 0.0; }
    return p * f32(gw * gh) / (2.0 * PI * PI * st);
}

fn env_image_sample(rng: ptr<function, Rng>) -> vec4<f32> {
    let gw = u32(b_env[0].z);
    let gh = u32(b_env[0].w);
    let base = 1u + u32(b_env[0].x) * u32(b_env[0].y);
    // Row by the marginal CDF, then the texel by its row's CDF.
    let u1 = rnd(rng);
    var lo = 0u;
    var hi = gh - 1u;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if b_env[base + mid].x < u1 { lo = mid + 1u; } else { hi = mid; }
    }
    let y = lo;
    let u2 = rnd(rng);
    let row = base + gh + y * gw;
    lo = 0u;
    hi = gw - 1u;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if b_env[row + mid].x < u2 { lo = mid + 1u; } else { hi = mid; }
    }
    let x = lo;
    let u = (f32(x) + rnd(rng)) / f32(gw);
    let v = (f32(y) + rnd(rng)) / f32(gh);
    let theta = v * PI;
    let phi = (u - 0.5) * 2.0 * PI - b_light[4].w;
    let ww = vec3<f32>(sin(theta) * sin(phi), -cos(theta), sin(theta) * cos(phi));
    let d = normalize(b_light[1].xyz * ww.x + b_light[2].xyz * ww.y + b_light[3].xyz * ww.z);
    return vec4<f32>(d, 1.0);
}

// Ambient lights: a uniform environment, added to the image or the sky.
fn env_ambient() -> vec3<f32> {
    var c = vec3<f32>(0.0);
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        if u32(b_light[7u + i * LROWS].w) == 2u { c += b_light[7u + i * LROWS + 1u].xyz; }
    }
    return c;
}

fn env_smooth(d: vec3<f32>) -> vec3<f32> {
    if has_image() { return env_image(d) + env_ambient(); }
    let w = to_world(d);
    let up = -w.y;
    let sky = b_light[4].xyz;
    let horizon = b_light[5].xyz;
    let ground = b_light[6].xyz;
    var c: vec3<f32>;
    if up >= 0.0 { c = mix(horizon, sky, smoothstep(0.0, 0.6, up)); } else { c = mix(horizon, ground, smoothstep(0.0, 0.2, -up)); }
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        if u32(b_light[7u + i * LROWS].w) == 2u { c += b_light[7u + i * LROWS + 1u].xyz; }
    }
    return c;
}

fn env_lobes(d: vec3<f32>) -> vec3<f32> {
    let w = to_world(d);
    var c = vec3<f32>(0.0);
    let lights = u32(b_light[0].x);
    let lobes = u32(b_light[0].y);
    let first = 7u + lights * LROWS;
    for (var i = 0u; i < lobes; i++) {
        let l = b_light[first + i * 2u];
        let s2 = max(l.w * l.w, 1e-6);
        c += b_light[first + i * 2u + 1u].xyz * exp((dot(w, l.xyz) - 1.0) / s2);
    }
    return c;
}

fn env(d: vec3<f32>) -> vec3<f32> {
    return env_smooth(d) + env_lobes(d);
}

// A lobe exp((cos − 1) / s²) is a von Mises–Fisher lobe with κ = 1 / s²:
// sampled exactly. Lobes are picked by their power (colour × s²).
fn lobe_weight(i: u32) -> f32 {
    let first = 7u + u32(b_light[0].x) * LROWS;
    let l = b_light[first + i * 2u];
    return luminance(b_light[first + i * 2u + 1u].xyz) * max(l.w * l.w, 1e-6);
}

fn vmf_pdf(cos_t: f32, k: f32) -> f32 {
    return k * exp(k * (cos_t - 1.0)) / (2.0 * PI * (1.0 - exp(-2.0 * k)));
}

// The pdf of env_lobe_sample choosing camera-space direction d.
fn env_lobe_pdf(d: vec3<f32>) -> f32 {
    let lobes = u32(b_light[0].y);
    if lobes == 0u { return 0.0; }
    let w = to_world(d);
    let first = 7u + u32(b_light[0].x) * LROWS;
    var total = 0.0;
    var pdf = 0.0;
    for (var i = 0u; i < lobes; i++) {
        let lw = lobe_weight(i);
        total += lw;
        let l = b_light[first + i * 2u];
        pdf += lw * vmf_pdf(dot(w, l.xyz), 1.0 / max(l.w * l.w, 1e-6));
    }
    if total <= 0.0 { return 0.0; }
    return pdf / total;
}

// A camera-space direction toward the lobes (w: 0 when there are none).
fn env_lobe_sample(rng: ptr<function, Rng>) -> vec4<f32> {
    let lobes = u32(b_light[0].y);
    if lobes == 0u { return vec4<f32>(0.0); }
    var total = 0.0;
    for (var i = 0u; i < lobes; i++) { total += lobe_weight(i); }
    if total <= 0.0 { return vec4<f32>(0.0); }
    var pick = rnd(rng) * total;
    var chosen = lobes - 1u;
    for (var i = 0u; i < lobes; i++) {
        pick -= lobe_weight(i);
        if pick <= 0.0 { chosen = i; break; }
    }
    let first = 7u + u32(b_light[0].x) * LROWS;
    let l = b_light[first + chosen * 2u];
    let k = 1.0 / max(l.w * l.w, 1e-6);
    let u1 = rnd(rng);
    let u2 = rnd(rng);
    let c = clamp(1.0 + log(u1 + (1.0 - u1) * exp(-2.0 * k)) / k, -1.0, 1.0);
    let sn = sqrt(max(1.0 - c * c, 0.0));
    let f = basis(normalize(l.xyz));
    let ww = normalize(f[0] * (sn * cos(2.0 * PI * u2)) + f[1] * (sn * sin(2.0 * PI * u2)) + f[2] * c);
    // World → camera: the rows of camera→world are its columns.
    let d = normalize(b_light[1].xyz * ww.x + b_light[2].xyz * ww.y + b_light[3].xyz * ww.z);
    return vec4<f32>(d, 1.0);
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
    prev: vec3<f32>,   // where this point was on the previous frame (camera space)
    // The triangle, for surface detail: instance row, its vertices' rows
    // (attribute base + index), barycentrics.
    inst: u32,
    v0: u32,
    v1: u32,
    v2: u32,
    bary: vec3<f32>,
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
    prof_count(0u);
    prof_count(4u + vol_depth);
    var rq: ray_query;
    // Every hit is recorded by hand from the candidates, on every device:
    // every triangle is a candidate (no opaque fast path) and the hit is the
    // last confirmed one (confirming shortens the ray, so it is the nearest).
    // The committed intersection is never read:
    // - naga 25's SPIR-V writer makes one helper for both intersection
    //   reads, so after a candidate read the committed one returns the
    //   candidate: on NVIDIA every miss came back as a triangle at t = 0
    //   (black frames, everything shadowed);
    // - Mesa lavapipe reports the committed hit with the instance of the
    //   last box the ray entered;
    // - on lavapipe, reading the committed intersection (or a whole copied
    //   candidate) crashes the JIT, so the fields are copied one by one.
    rayQueryInitialize(&rq, acc, RayDesc(RAY_FLAG_FORCE_NO_OPAQUE, 0xFFu, 0.0, tmax, o, d));
    var kind = RAY_QUERY_INTERSECTION_NONE;
    var hit_t = 0.0;
    var custom = 0u;
    var prim = 0u;
    var bary = vec2<f32>(0.0);
    var w2o = mat4x3<f32>();
    while rayQueryProceed(&rq) {
        let c = rayQueryGetCandidateIntersection(&rq);
        prof_count(1u);
        if c.kind != RAY_QUERY_INTERSECTION_TRIANGLE { continue; }
        var take = true;
        if F_CARDS && c.instance_custom_data == u.opts.z {
            // Cards are not opaque: a candidate stands where its picture
            // does (stochastically by its alpha, so soft edges average out).
            let pic = card_picture(c.primitive_index / 2u, o + d * c.t);
            take = pic.w > 0.0 && pic.w >= rnd(rng);
        }
        if take {
            rayQueryConfirmIntersection(&rq);
            kind = c.kind;
            hit_t = c.t;
            custom = c.instance_custom_data;
            prim = c.primitive_index;
            bary = c.barycentrics;
            w2o = c.world_to_object;
        }
    }
    var hit: Hit;
    hit.found = kind != RAY_QUERY_INTERSECTION_NONE;
    if !hit.found { return hit; }
    hit.t = hit_t;
    hit.p = o + d * hit_t;
    if F_CARDS && custom == u.opts.z {
        hit.card = true;
        hit.mat = prim / 2u;
        hit.card_color = card_picture(hit.mat, hit.p);
        hit.prev = hit.p;
        hit.n = -d;
        hit.g = -d;
        hit.front = true;
        return hit;
    }
    // Every index is checked: a driver reporting a wrong instance or
    // primitive must not read past the buffers (that is a crash).
    let inst = custom * INST_ROWS;
    if inst + INST_ROWS - 1u >= arrayLength(&b_inst) { hit.found = false; return hit; }
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
    // The same point of the object on the previous frame.
    let obj = vec4<f32>(p0 * bw.x + p1 * bw.y + p2 * bw.z, 1.0);
    hit.prev = vec3<f32>(dot(b_inst[inst + 4u], obj), dot(b_inst[inst + 5u], obj), dot(b_inst[inst + 6u], obj));
    hit.inst = inst;
    hit.v0 = attr + i0;
    hit.v1 = attr + i1;
    hit.v2 = attr + i2;
    hit.bary = bw;
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
    hit.mat = (base + seg % max(count, 1u)) * MAT_ROWS;
    if hit.mat + MAT_ROWS - 1u >= arrayLength(&b_mat) { hit.found = false; return hit; }
    prof_count(prof_materials() + hit.mat / MAT_ROWS * 2u);
    hit.two_sided = (tag & TWO_SIDED) != 0u;
    let flags = u32(b_mat[hit.mat + 2u].w);
    if (flags & FLAG_FACETED) != 0u { n = g; }
    // Which side was hit: by the mesh's outward (vertex) normal against
    // the ray (triangle winding is not consistently outward); then both
    // normals face the ray.
    hit.front = dot(normalize(n_obj * m3), d) < 0.0;
    if dot(g, d) > 0.0 { g = -g; }
    if dot(n, g) < 0.0 { n = -n; }
    hit.g = g;
    hit.n = n;
    return hit;
}

// Whether anything blocks the segment from `o` along `d` up to `tmax`.
fn occluded(o_in: vec3<f32>, d: vec3<f32>, tmax: f32, rng: ptr<function, Rng>) -> bool {
    var o = o_in;
    var left = tmax;
    // Volumes' boundaries let light through: look past them. A noise
    // volume (clouds, smoke) may block over its stretch of the ray: from
    // where the ray entered it, or from the start when it began inside.
    // Each blocks independently, so overlapping ones multiply.
    var entered = array<u32, 4>(0u, 0u, 0u, 0u);
    var einst = array<u32, 4>(0u, 0u, 0u, 0u);
    var at = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    var n = 0u;
    var dist = 0.0;
    for (var k = 0u; k < 16u; k++) {
        let h = cast_ray(o, d, left, rng);
        if !h.found {
            // Still inside the volumes entered: they block up to the end.
            for (var i = 0u; i < n; i++) {
                if volume_shadow(entered[i], o_in + d * at[i], d, tmax - at[i], rng) { return true; }
            }
            return false;
        }
        if F_CARDS && h.card { return true; }
        // Glass and see-through surfaces let some light by; the shadow
        // catcher casts nothing.
        let flags = u32(b_mat[h.mat + 2u].w);
        if F_VOLUME && (flags & FLAG_VOLUME) != 0u {
            let here = dist + h.t;
            if b_mat[h.mat + 30u].z > 0.5 {
                if h.front {
                    if n < 4u {
                        entered[n] = h.mat;
                        einst[n] = h.inst;
                        at[n] = here;
                        n++;
                    }
                } else {
                    var start = 0.0;
                    for (var i = 0u; i < n; i++) {
                        if einst[i] == h.inst {
                            start = at[i];
                            n--;
                            entered[i] = entered[n];
                            einst[i] = einst[n];
                            at[i] = at[n];
                            break;
                        }
                    }
                    if volume_shadow(h.mat, o_in + d * start, d, here - start, rng) { return true; }
                }
            }
            o = offset(h, d);
            left = left - h.t;
            dist = here;
            continue;
        }
        if (F_CATCHER && (flags & FLAG_SHADOW_CATCHER) != 0u) || (flags & FLAG_WIRE) != 0u { return false; }
        let opacity = b_mat[h.mat].w * (1.0 - b_mat[h.mat + 3u].x);
        return opacity >= rnd(rng);
    }
    return false;
}


// A density grid's value at uvw (0..1 across the box; mirrors mesh3d.rs):
// trilinear, between two of its tiled z slices.
fn grid_density(row: u32, uvw: vec3<f32>) -> f32 {
    let r31 = b_mat[row + 31u];
    if r31.w < 0.5 { return 0.0; }
    let n = b_mat[row + 32u];
    let x = clamp(uvw.x * n.x - 0.5, 0.0, n.x - 1.0);
    let y = clamp(uvw.y * n.y - 0.5, 0.0, n.y - 1.0);
    let z = clamp(uvw.z * n.z - 0.5, 0.0, n.z - 1.0);
    let z0 = floor(z);
    let z1 = min(z0 + 1.0, n.z - 1.0);
    return mix(grid_slice(r31, n, z0, x, y), grid_slice(r31, n, z1, x, y), z - z0);
}

// Slice k of a grid image (n: cells x, y, z, tiles per row), bilinear
// inside its tile.
fn grid_slice(r31: vec4<f32>, n: vec4<f32>, k: f32, x: f32, y: f32) -> f32 {
    let ox = i32((k % n.w) * n.x);
    let oy = i32(floor(k / n.w) * n.y);
    let x0 = floor(x);
    let y0 = floor(y);
    let x1 = min(x0 + 1.0, n.x - 1.0);
    let y1 = min(y0 + 1.0, n.y - 1.0);
    let a = mix(map_texel(r31, ox + i32(x0), oy + i32(y0)).x, map_texel(r31, ox + i32(x1), oy + i32(y0)).x, x - x0);
    let b = mix(map_texel(r31, ox + i32(x0), oy + i32(y1)).x, map_texel(r31, ox + i32(x1), oy + i32(y1)).x, x - x0);
    return mix(a, b, y - y0);
}
// A volume's density at camera-space p, as a share of its sigma (mirrors
// `volume_density` in motion.wgsl).
fn volume_density(row: u32, p: vec3<f32>) -> f32 {
    if b_mat[row + 30u].z < 0.5 { return 1.0; }
    if !PROFILE { return density_at(row, p); }
    // Profiling: count the lookup, and whether it found nothing.
    prof_count(prof_materials() + row / MAT_ROWS * 2u + 1u);
    px_lookups += 1u;
    prof_count(8u + vol_depth);
    let v = density_at(row, p);
    if v <= 0.0 { prof_count(2u); }
    return v;
}

fn density_at(row: u32, p: vec3<f32>) -> f32 {
    return density_local(row, vol_local(row, p));
}

// A camera-space point on a volume's own centred box (rows 21..23).
fn vol_local(row: u32, p: vec3<f32>) -> vec3<f32> {
    let r0 = b_mat[row + 21u];
    let r1 = b_mat[row + 22u];
    let r2 = b_mat[row + 23u];
    return vec3<f32>(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w);
}

// A camera-space direction in a volume's box: how its local point moves
// per unit along the ray.
fn vol_dir(row: u32, d: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(b_mat[row + 21u].xyz, d), dot(b_mat[row + 22u].xyz, d), dot(b_mat[row + 23u].xyz, d));
}

// The density at local point q of a noise or grid volume.
fn density_local(row: u32, q: vec3<f32>) -> f32 {
    let r30 = b_mat[row + 30u];
    let s = q / max(b_mat[row + 24u].xyz, vec3<f32>(1e-3));
    if r30.z > 1.5 {
        let uvw = s * 0.5 + vec3<f32>(0.5);
        if any(uvw < vec3<f32>(0.0)) || any(uvw > vec3<f32>(1.0)) { return 0.0; }
        return grid_density(row, uvw);
    }
    var e = length(s);
    if b_mat[row + 18u].z < 0.5 { e = max(max(abs(s.x), abs(s.y)), abs(s.z)); }
    let fall = clamp((1.0 - e) / 0.35, 0.0, 1.0);
    if fall <= 0.0 { return 0.0; }
    let r9 = b_mat[row + 9u];
    let at = (q + b_mat[row + 17u].xyz) * r9.y;
    let n = n_sample(u32(r9.x + 0.5), at, u32(r9.z), bitcast<u32>(r9.w));
    if PROFILE {
        // Cost experiment (profiling builds only; denoise.w = k): the noise
        // k more times, nothing else changed, so the trace's growth is what
        // the density work costs in place.
        var extra = 0.0;
        for (var i = 0u; i < u.band.z; i++) {
            extra += n_sample(u32(r9.x + 0.5), at + vec3<f32>(f32(i + 1u) * 1e-3), u32(r9.z), bitcast<u32>(r9.w));
        }
        if extra < -1.0e30 { return 0.0; }
    }
    return shaped_density(row, s, at, n, fall);
}

// A noise volume's density from its noise n at noise-space point `at`,
// box position s (-1..1) and edge fade (mirrors mesh3d.rs). Cumulus (row
// 33.w = 1): round billows from cellular noise twice the noise's size
// over the noise, a flat base at 18% of the height, a rounded top, and
// coverage that erodes faster with height (cauliflower heads).
fn shaped_density(row: u32, s: vec3<f32>, at: vec3<f32>, n: f32, fall: f32) -> f32 {
    let r30 = b_mat[row + 30u];
    let r33 = b_mat[row + 33u];
    if r33.w < 0.5 { return clamp((n * fall - (1.0 - r30.x)) / max(r30.y, 1e-3), 0.0, 1.0); }
    let t = clamp(dot(s, r33.xyz) * 0.5 + 0.5, 0.0, 1.0);
    let profile = smoothstep(0.18, 0.24, t) * (1.0 - smoothstep(0.6, 1.0, t));
    let billow = 1.0 - n_cellular(at * 0.5, bitcast<u32>(b_mat[row + 9u].w) ^ 0x5bd1u);
    let v = (0.6 * billow + 0.4 * n) * fall * profile;
    return clamp((v - (1.0 - r30.x) - 0.25 * t) / max(r30.y, 1e-3), 0.0, 1.0);
}

struct Cell {
    bound: f32,  // the most density in the cell (a share of sigma)
    exit: f32,   // how much further along the ray the cell lasts
}

// The bounds cell of a volume at local point q, for a ray moving dq per
// unit: its bound and where the ray leaves it. Without baked bounds (even
// volumes): 1, for ever. Outside the box there is no medium until the ray
// reaches it.
fn bounds_cell(row: u32, q: vec3<f32>, dq: vec3<f32>) -> Cell {
    var c: Cell;
    c.bound = 1.0;
    c.exit = 1e30;
    let off = bitcast<u32>(b_mat[row + 30u].w);
    if off == 0u { return c; }
    let h = max(b_mat[row + 24u].xyz, vec3<f32>(1e-3));
    let n = f32(BOUNDS_N);
    let size = 2.0 * h / n;
    let u = (q + h) / size;
    if any(u < vec3<f32>(0.0)) || any(u >= vec3<f32>(n)) {
        // Outside: empty up to where the ray enters the box (slab test).
        c.bound = 0.0;
        var enter = 0.0;
        let lo = (-h - q) / dq;
        let hi = (h - q) / dq;
        let near = min(lo, hi);
        let far = max(lo, hi);
        enter = max(max(near.x, near.y), max(near.z, 0.0));
        if enter >= min(min(far.x, far.y), far.z) { return c; }
        c.exit = enter + 1e-3 * min(size.x, min(size.y, size.z)) / max(length(dq), 1e-12);
        return c;
    }
    let i = floor(u);
    let ii = vec3<u32>(i);
    c.bound = b_bounds[off - 1u + ii.x + BOUNDS_N * (ii.y + BOUNDS_N * ii.z)];
    // The nearest of the cell's faces ahead.
    let ahead = select(i, i + vec3<f32>(1.0), dq > vec3<f32>(0.0)) * size - h;
    let tx = select(1e30, (ahead.x - q.x) / dq.x, abs(dq.x) > 1e-12);
    let ty = select(1e30, (ahead.y - q.y) / dq.y, abs(dq.y) > 1e-12);
    let tz = select(1e30, (ahead.z - q.z) / dq.z, abs(dq.z) > 1e-12);
    // Just past the face, so the next lookup lands in the next cell.
    c.exit = max(min(tx, min(ty, tz)), 0.0) + 1e-3 * min(size.x, min(size.y, size.z)) / max(length(dq), 1e-12);
    return c;
}

// Bakes the density bounds: one invocation per cell (x) of a volume (y,
// a job). Sixteen samples spread over the cell grown by half a cell each
// way, so a wisp near a face counts for both cells, and half as much again
// for what the samples missed (as Cycles does for estimated bounds).
@compute @workgroup_size(64, 1, 1)
fn bounds(@builtin(global_invocation_id) id: vec3<u32>) {
    let cells = BOUNDS_N * BOUNDS_N * BOUNDS_N;
    if id.x >= cells || id.y * 2u + 1u >= arrayLength(&b_jobs) { return; }
    let row = b_jobs[id.y * 2u];
    let off = b_jobs[id.y * 2u + 1u];
    let h = max(b_mat[row + 24u].xyz, vec3<f32>(1e-3));
    let size = 2.0 * h / f32(BOUNDS_N);
    let cell = vec3<f32>(f32(id.x % BOUNDS_N), f32((id.x / BOUNDS_N) % BOUNDS_N), f32(id.x / (BOUNDS_N * BOUNDS_N)));
    let lo = -h + cell * size;
    var rng: Rng;
    rng.s = pcg(id.x + pcg(id.y + pcg(u.opts.y)));
    var most = 0.0;
    for (var k = 0u; k < 16u; k++) {
        let r = vec3<f32>(rnd(&rng), rnd(&rng), rnd(&rng)) * 2.0 - vec3<f32>(0.5);
        most = max(most, density_local(row, lo + r * size));
    }
    if most > 0.0 { most = min(1.0, most * 1.5); }
    if off + id.x < arrayLength(&b_bounds) { b_bounds[off + id.x] = most; }
}

// Artistic volumes (24.w = 1): marched as the preview draws them.
fn vol_artistic(row: u32) -> bool {
    return b_mat[row + 24u].w > 0.5;
}

// The preview's self-shadow tap: a quarter of the box's mean half size,
// in camera px.
fn vol_reach(row: u32) -> f32 {
    let h = b_mat[row + 24u].xyz;
    let k = (length(b_mat[row + 21u].xyz) + length(b_mat[row + 22u].xyz) + length(b_mat[row + 23u].xyz)) / 3.0;
    return 0.25 * (h.x + h.y + h.z) / 3.0 / max(k, 1e-6);
}

// Light an artistic volume scatters toward -d at p (as the preview's
// `volume_lights`): each light through one density tap toward it, with
// light scattered many times in three octaves (each shadowed half as much,
// half as anisotropic), plus the sky from one random direction, unshadowed.
fn artistic_lights(row: u32, p: vec3<f32>, d: vec3<f32>, rng: ptr<function, Rng>) -> vec3<f32> {
    let v18 = b_mat[row + 18u];
    let reach = vol_reach(row);
    var out = vec3<f32>(0.0);
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        let kind = u32(b_light[7u + i * LROWS].w);
        if kind == 2u { continue; }
        let ls = sample_light(i, p, rng);
        if dot(ls.col, ls.col) <= 0.0 { continue; }
        let tau = v18.x * (density_at(row, p + ls.l * (0.5 * reach)) + density_at(row, p + ls.l * (1.5 * reach))) * reach;
        var ms = 0.0;
        var k = 1.0;
        for (var o = 0u; o < 3u; o++) {
            ms += k * exp(-tau * k) * hg_phase(v18.y * k, dot(ls.l, d));
            k *= 0.5;
        }
        if F_AREA && kind == 3u {
            let row_l = 7u + i * LROWS;
            let pdf = area_pdf(row_l, p, ls.l, ls.dist);
            if pdf > 0.0 { out += ms * area_radiance(row_l) * ls.col / pdf; }
        } else {
            out += ms * PI * ls.col;
        }
    }
    // The sky (and ambient lights), one direction per step.
    let z = 1.0 - 2.0 * rnd(rng);
    let r = sqrt(max(1.0 - z * z, 0.0));
    let phi = 2.0 * PI * rnd(rng);
    let w = vec3<f32>(r * cos(phi), r * sin(phi), z);
    out += env(w) * hg_phase(v18.y, dot(w, d)) * 4.0 * PI;
    return out;
}

// Whether a shadow ray is blocked crossing `len` px of a noise volume: an
// artistic one by its marched transmittance (blocks with probability
// 1 − T), a physical one by delta tracking.
fn volume_shadow(row: u32, o: vec3<f32>, d: vec3<f32>, len: f32, rng: ptr<function, Rng>) -> bool {
    if F_VOL_PHYSICAL && !vol_artistic(row) { return volume_blocks(row, o, d, len, rng); }
    let sigma = b_mat[row + 18u].x;
    let dt = len / 8.0;
    let j = rnd(rng);
    var tau = 0.0;
    for (var s = 0u; s < 8u; s++) {
        tau += sigma * volume_density(row, o + d * ((f32(s) + j) * dt)) * dt;
    }
    return rnd(rng) >= exp(-tau);
}

// Whether the medium blocks a ray crossing `len` px of a noise volume:
// delta tracking, cell by cell through its bounds (empty cells skipped,
// each other cell at its own bound × sigma); unbiased while the bounds
// hold: blocks with probability one minus the transmittance.
fn volume_blocks(row: u32, o: vec3<f32>, d: vec3<f32>, len: f32, rng: ptr<function, Rng>) -> bool {
    let sigma = max(b_mat[row + 18u].x, 1e-6);
    let q0 = vol_local(row, o);
    let dq = vol_dir(row, d);
    var t = 0.0;
    for (var k = 0u; k < 1024u; k++) {
        let c = bounds_cell(row, q0 + dq * t, dq);
        let until = min(len, t + c.exit);
        let major = sigma * c.bound;
        if major > 0.0 {
            let step = -log(max(1.0 - rnd(rng), 1e-7)) / major;
            if t + step < until {
                t += step;
                if rnd(rng) * c.bound < volume_density(row, o + d * t) { return true; }
                continue;
            }
        }
        if until >= len { return false; }
        t = until;
    }
    return true;
}

fn hg_phase(g: f32, c: f32) -> f32 {
    let k = 1.0 + g * g - 2.0 * g * c;
    return (1.0 - g * g) / (4.0 * PI * max(k * sqrt(max(k, 0.0)), 1e-6));
}

// A direction scattered from travel direction d by Henyey–Greenstein g.
fn sample_hg(d: vec3<f32>, g: f32, u1: f32, u2: f32) -> vec3<f32> {
    var c = 2.0 * u1 - 1.0;
    if abs(g) > 1e-3 {
        let q = (1.0 - g * g) / (1.0 - g + 2.0 * g * u1);
        c = (1.0 + g * g - q * q) / (2.0 * g);
    }
    c = clamp(c, -1.0, 1.0);
    let sn = sqrt(max(1.0 - c * c, 0.0));
    let phi = 2.0 * PI * u2;
    let f = basis(d);
    return normalize(f[0] * (sn * cos(phi)) + f[1] * (sn * sin(phi)) + f[2] * c);
}

// Light reaching a point inside a volume (travelling along d), times the
// phase toward the viewer: delta and area lights, with shadow rays.
fn volume_direct(p: vec3<f32>, d: vec3<f32>, g: f32, rng: ptr<function, Rng>) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        let kind = u32(b_light[7u + i * LROWS].w);
        if kind == 2u { continue; }
        let ls = sample_light(i, p, rng);
        if dot(ls.col, ls.col) <= 0.0 { continue; }
        if ls.shadow && occluded(p, ls.l, ls.dist * 0.999, rng) { continue; }
        // Light travels −l, scatters toward −d: cos between them is l·d.
        let ph = hg_phase(g, dot(ls.l, d));
        if F_AREA && kind == 3u {
            let row = 7u + i * LROWS;
            let pdf = area_pdf(row, p, ls.l, ls.dist);
            if pdf > 0.0 { out += ph * area_radiance(row) * ls.col / pdf; }
        } else {
            out += ph * PI * ls.col;
        }
    }
    return out;
}

fn offset(h: Hit, toward: vec3<f32>) -> vec3<f32> {
    let eps = 0.02 + h.t * 1e-5;
    if dot(toward, h.g) >= 0.0 { return h.p + h.g * eps; }
    return h.p - h.g * eps;
}

// ---------------------------------------------------------------- shading

// ------------------------------------------------------------ surface detail
// Seeded noise (noise.rs, exactly): PCG lattice hash, quintic value noise,
// fbm octaves, cellular F1.

fn n_pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn n_hash(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let h = n_pcg((bitcast<u32>(x) * 73856093u) ^ (bitcast<u32>(y) * 19349663u) ^ (bitcast<u32>(z) * 83492791u) ^ n_pcg(seed));
    return f32(h >> 8u) / 16777216.0;
}

fn n_fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn n_value(q: vec3<f32>, seed: u32) -> f32 {
    let f = floor(q);
    let x = i32(f.x);
    let y = i32(f.y);
    let z = i32(f.z);
    let tx = n_fade(q.x - f.x);
    let ty = n_fade(q.y - f.y);
    let tz = n_fade(q.z - f.z);
    let a = mix(mix(n_hash(x, y, z, seed), n_hash(x + 1, y, z, seed), tx), mix(n_hash(x, y + 1, z, seed), n_hash(x + 1, y + 1, z, seed), tx), ty);
    let b = mix(mix(n_hash(x, y, z + 1, seed), n_hash(x + 1, y, z + 1, seed), tx), mix(n_hash(x, y + 1, z + 1, seed), n_hash(x + 1, y + 1, z + 1, seed), tx), ty);
    return mix(a, b, tz);
}

fn n_fbm(q: vec3<f32>, octaves: u32, seed: u32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var total = 0.0;
    var f = 1.0;
    for (var o = 0u; o < clamp(octaves, 1u, 8u); o++) {
        sum += amp * n_value(q * f, seed + o * 1013u);
        total += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    return sum / total;
}

fn n_cellular(q: vec3<f32>, seed: u32) -> f32 {
    let f = floor(q);
    var best = 4.0;
    for (var dz = -1; dz <= 1; dz++) {
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                let x = i32(f.x) + dx;
                let y = i32(f.y) + dy;
                let z = i32(f.z) + dz;
                let p = vec3<f32>(f32(x) + n_hash(x, y, z, seed), f32(y) + n_hash(x, y, z, seed ^ 0x9e37u), f32(z) + n_hash(x, y, z, seed ^ 0x7f4au));
                let d = p - q;
                best = min(best, dot(d, d));
            }
        }
    }
    return min(sqrt(best), 1.0);
}

// kind: 1 value, 2 fbm, 3 cellular (material row 9.x).
fn n_sample(kind: u32, q: vec3<f32>, octaves: u32, seed: u32) -> f32 {
    if kind == 1u { return n_value(q, seed); }
    if kind == 3u { return n_cellular(q, seed); }
    return n_fbm(q, octaves, seed);
}

// An image map texel (wrapping: maps tile), bilinear, level 0. Raw
// values (premultiplied sRGB for colour, data for normal and bump maps).
fn map_texel(r: vec4<f32>, x: i32, y: i32) -> vec4<f32> {
    let w = i32(bitcast<u32>(r.y));
    let h = i32(bitcast<u32>(r.z));
    let xx = ((x % w) + w) % w;
    let yy = ((y % h) + h) % h;
    return unpack(b_images[bitcast<u32>(r.x) + u32(yy * w + xx)]);
}

fn map_sample(r: vec4<f32>, t: vec2<f32>) -> vec4<f32> {
    let s = t * vec2<f32>(f32(bitcast<u32>(r.y)), f32(bitcast<u32>(r.z))) - vec2<f32>(0.5);
    let f = floor(s);
    let fx = s.x - f.x;
    let fy = s.y - f.y;
    let x = i32(f.x);
    let y = i32(f.y);
    return mix(mix(map_texel(r, x, y), map_texel(r, x + 1, y), fx), mix(map_texel(r, x, y + 1), map_texel(r, x + 1, y + 1), fx), fy);
}

fn map_height(r: vec4<f32>, t: vec2<f32>) -> f32 {
    let c = map_sample(r, t);
    return dot(c.xyz, vec3<f32>(0.2126, 0.7152, 0.0722));
}

struct Detail {
    base: vec3<f32>,
    rough: f32,
    n: vec3<f32>,
    rough_mul: f32,  // roughness map (green)
    metal_mul: f32,  // metallic map (blue)
    emit: vec3<f32>, // emissive map (multiplies the emission)
}

// A material's surface detail at barycentric b of a triangle (camera-space
// positions pos, noise-space positions nps, UVs uvs), shading normal n:
// its colour, roughness change and bent normal.
fn surface_detail(row: u32, pos: array<vec3<f32>, 3>, nps: array<vec3<f32>, 3>, uvs_in: array<vec2<f32>, 3>, b: vec3<f32>, n_in: vec3<f32>, base_in: vec3<f32>) -> Detail {
    var d: Detail;
    d.base = base_in;
    d.rough = 0.0;
    d.n = n_in;
    d.rough_mul = 1.0;
    d.metal_mul = 1.0;
    d.emit = vec3<f32>(1.0);
    let r9 = b_mat[row + 9u];
    let r12 = b_mat[row + 12u];
    let r13 = b_mat[row + 13u];
    let r14 = b_mat[row + 14u];
    let r25 = b_mat[row + 25u];
    let r26 = b_mat[row + 26u];
    let r29 = b_mat[row + 29u];
    let pbr_maps = r25.w > 0.5 || r26.w > 0.5 || r29.w > 0.5;
    if r9.x < 0.5 && r12.w < 0.5 && r13.w < 0.5 && r14.w == 0.0 && !pbr_maps { return d; }
    let e1 = pos[1] - pos[0];
    let e2 = pos[2] - pos[0];
    var n = n_in;
    if r12.w > 0.5 || r13.w > 0.5 || r14.w != 0.0 || pbr_maps {
        // UVs: across a wrap seam, bring the corners together; then tile,
        // offset and turn (about the middle).
        var uvs = uvs_in;
        let wraps = u32(b_mat[row + 16u].y);
        for (var k = 1u; k < 3u; k++) {
            if (wraps & 1u) != 0u {
                if uvs[k].x - uvs[0].x > 0.5 { uvs[k].x -= 1.0; }
                if uvs[k].x - uvs[0].x < -0.5 { uvs[k].x += 1.0; }
            }
            if (wraps & 2u) != 0u {
                if uvs[k].y - uvs[0].y > 0.5 { uvs[k].y -= 1.0; }
                if uvs[k].y - uvs[0].y < -0.5 { uvs[k].y += 1.0; }
            }
        }
        let r15 = b_mat[row + 15u];
        let ang = b_mat[row + 16u].x;
        let cs = cos(ang);
        let sn = sin(ang);
        for (var k = 0u; k < 3u; k++) {
            let c = uvs[k] - vec2<f32>(0.5);
            uvs[k] = vec2<f32>(c.x * cs - c.y * sn, c.x * sn + c.y * cs) * r15.xy + vec2<f32>(0.5) + r15.zw;
        }
        let t = uvs[0] * b.x + uvs[1] * b.y + uvs[2] * b.z;
        // How the surface runs along u and v.
        let d1 = uvs[1] - uvs[0];
        let d2 = uvs[2] - uvs[0];
        let det = d1.x * d2.y - d2.x * d1.y;
        var dpdu = vec3<f32>(0.0);
        var dpdv = vec3<f32>(0.0);
        if abs(det) > 1e-12 {
            dpdu = (e1 * d2.y - e2 * d1.y) / det;
            dpdv = (e2 * d1.x - e1 * d2.x) / det;
        }
        if r12.w > 0.5 {
            let c = map_sample(r12, t);
            var straight = vec3<f32>(0.0);
            if c.w > 0.0 { straight = c.xyz / c.w; }
            d.base = d.base * vec3<f32>(decode(straight.x), decode(straight.y), decode(straight.z));
        }
        // Linear data, as glTF packs it: roughness in green, metallic in blue.
        if r25.w > 0.5 {
            let c = map_sample(r25, t);
            if c.w > 0.0 { d.rough_mul = c.y / c.w; }
        }
        if r26.w > 0.5 {
            let c = map_sample(r26, t);
            if c.w > 0.0 { d.metal_mul = c.z / c.w; }
        }
        if r29.w > 0.5 {
            let c = map_sample(r29, t);
            d.emit = vec3<f32>(0.0);
            if c.w > 0.0 { d.emit = vec3<f32>(decode(c.x / c.w), decode(c.y / c.w), decode(c.z / c.w)); }
        }
        let lu = length(dpdu);
        let lv = length(dpdv);
        if lu > 1e-9 && lv > 1e-9 {
            let tu = normalize(dpdu - n * dot(n, dpdu));
            let tv = normalize(dpdv - n * dot(n, dpdv) - tu * dot(tu, dpdv));
            if r13.w > 0.5 {
                // Tangent space, green up (against v, which runs down the image).
                let m = map_sample(r13, t).xyz * 2.0 - vec3<f32>(1.0);
                n = normalize(tu * m.x - tv * m.y + n * max(m.z, 0.05));
            }
            if r14.w != 0.0 {
                let su = 1.0 / f32(bitcast<u32>(r14.y));
                let sv = 1.0 / f32(bitcast<u32>(r14.z));
                let dhdu = (map_height(r14, t + vec2<f32>(su, 0.0)) - map_height(r14, t - vec2<f32>(su, 0.0))) / (2.0 * su);
                let dhdv = (map_height(r14, t + vec2<f32>(0.0, sv)) - map_height(r14, t - vec2<f32>(0.0, sv))) / (2.0 * sv);
                n = normalize(n - r14.w * (tu * (dhdu / lu) + tv * (dhdv / lv)));
            }
        }
    }
    if r9.x > 0.5 {
        let kind = u32(r9.x + 0.5);
        let inv = r9.y;
        let octaves = u32(r9.z);
        let seed = bitcast<u32>(r9.w);
        let q = (nps[0] * b.x + nps[1] * b.y + nps[2] * b.z + b_mat[row + 17u].xyz) * inv;
        let v = n_sample(kind, q, octaves, seed);
        let r10 = b_mat[row + 10u];
        let r11 = b_mat[row + 11u];
        d.base = mix(d.base, r10.xyz, r10.w * v);
        d.rough = r11.x * (2.0 * v - 1.0);
        if r11.y != 0.0 {
            // Bump: the noise's slope along the surface (two directions in
            // the triangle, carried into noise space by its Jacobian).
            let f1 = nps[1] - nps[0];
            let f2 = nps[2] - nps[0];
            let g11 = dot(e1, e1);
            let g12 = dot(e1, e2);
            let g22 = dot(e2, e2);
            let gd = g11 * g22 - g12 * g12;
            if abs(gd) > 1e-12 {
                let t1 = normalize(e1 - n_in * dot(n_in, e1));
                let t2 = cross(n_in, t1);
                let h = 0.15 / max(inv, 1e-6);
                var grad = vec3<f32>(0.0);
                for (var k = 0u; k < 2u; k++) {
                    var tk = t1;
                    if k == 1u { tk = t2; }
                    let c1 = dot(e1, tk);
                    let c2 = dot(e2, tk);
                    let a = (c1 * g22 - c2 * g12) / gd;
                    let bb = (c2 * g11 - c1 * g12) / gd;
                    let step = (f1 * a + f2 * bb) * (h * inv);
                    let dv = (n_sample(kind, q + step, octaves, seed) - v) / h;
                    grad += tk * dv;
                }
                n = normalize(n - r11.y * grad);
            }
        }
    }
    d.n = n;
    return d;
}

// Surface detail at a mesh hit: the triangle's corners in camera and
// noise space with their UVs, then `surface_detail` as the raster.
fn detail_at(h: Hit, n: vec3<f32>, base: vec3<f32>) -> Detail {
    if !F_DETAIL {
        var plain: Detail;
        plain.base = base;
        plain.rough = 0.0;
        plain.n = n;
        plain.rough_mul = 1.0;
        plain.metal_mul = 1.0;
        plain.emit = vec3<f32>(1.0);
        return plain;
    }
    var pos: array<vec3<f32>, 3>;
    var nps: array<vec3<f32>, 3>;
    var uvs: array<vec2<f32>, 3>;
    let world = b_mat[h.mat + 11u].z > 0.5;
    let vs = array<u32, 3>(h.v0, h.v1, h.v2);
    for (var k = 0u; k < 3u; k++) {
        let a = b_attr[vs[k] * 2u];
        let o = vec4<f32>(a.xyz, 1.0);
        pos[k] = vec3<f32>(dot(b_inst[h.inst + 7u], o), dot(b_inst[h.inst + 8u], o), dot(b_inst[h.inst + 9u], o));
        nps[k] = a.xyz;
        if world { nps[k] = vec3<f32>(dot(b_inst[h.inst + 10u], o), dot(b_inst[h.inst + 11u], o), dot(b_inst[h.inst + 12u], o)); }
        uvs[k] = vec2<f32>(a.w, b_attr[vs[k] * 2u + 1u].w);
    }
    return surface_detail(h.mat, pos, nps, uvs, h.bary, n, base);
}

// The material at a hit, as the raster's model (mesh3d::material_rows):
// GGX base with F82-tinted Fresnel and energy compensation, Lambert
// diffuse, Charlie sheen, a clear coat (tinting what is under it).
struct Surface {
    albedo: vec3<f32>,   // the surface's colour with its detail (denoiser guide)
    n: vec3<f32>,
    t: vec3<f32>,
    b: vec3<f32>,
    at: f32,             // GGX alpha along t
    ab: f32,             // and along b (across the grain)
    rough: f32,
    f0: vec3<f32>,
    f82: vec3<f32>,
    f90: f32,
    comp: vec3<f32>,     // given back for multiple scattering
    kd: vec3<f32>,       // diffuse albedo left after the lobes above it
    sheen: vec3<f32>,
    sheen_a: f32,
    coat: f32,
    coat_a: f32,
    coat_tint: vec3<f32>,
    trans: f32,
    // Lobe choice when sampling: coat, specular (the rest diffuse).
    p_coat: f32,
    p_spec: f32,
    emit: vec3<f32>,     // the emissive map's colour (multiplies the emission)
}

fn pow5(x: f32) -> f32 {
    let x2 = x * x;
    return x2 * x2 * x;
}

fn env_brdf(r: f32, n_v: f32) -> vec2<f32> {
    let rr = r * vec4<f32>(-1.0, -0.0275, -0.572, 0.022) + vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let a004 = min(rr.x * rr.x, exp2(-9.28 * n_v)) * rr.x + rr.y;
    return vec2<f32>(-1.04 * a004 + rr.z, 1.04 * a004 + rr.w);
}

fn fresnel_f82(f0: vec3<f32>, f82: vec3<f32>, f90: f32, mu: f32) -> vec3<f32> {
    let m = clamp(mu, 0.0, 1.0);
    let s = f0 + (vec3<f32>(f90) - f0) * pow5(1.0 - m);
    let sb = f0 + (vec3<f32>(f90) - f0) * 0.46266437;
    let a = sb * (vec3<f32>(1.0) - f82) / 0.0566527;
    let x = 1.0 - m;
    return max(s - a * (m * x * x * x * x * x * x), vec3<f32>(0.0));
}

// Exact Fresnel of a dielectric boundary; eta = n_incident / n_transmitted.
fn fresnel_dielectric(cosi: f32, eta: f32) -> f32 {
    let s2 = eta * eta * (1.0 - cosi * cosi);
    if s2 >= 1.0 { return 1.0; }
    let cost = sqrt(1.0 - s2);
    let rs = (eta * cosi - cost) / (eta * cosi + cost);
    let rp = (cosi - eta * cost) / (cosi + eta * cost);
    return 0.5 * (rs * rs + rp * rp);
}

fn charlie(n_h: f32, a: f32) -> f32 {
    let inv = 1.0 / a;
    let s2 = max(1.0 - n_h * n_h, 0.0078125);
    return (2.0 + inv) * pow(s2, inv * 0.5) / (2.0 * PI);
}

fn neubelt(n_v: f32, n_l: f32) -> f32 {
    return 1.0 / max(4.0 * (n_l + n_v - n_l * n_v), 1e-4);
}

const SHEEN_E = array<f32, 64>(
    1.0000, 0.1364, 0.0009, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 1.0000, 0.6862, 0.2519, 0.0837, 0.0229, 0.0044, 0.0004, 0.0000,
    1.0000, 0.6499, 0.3698, 0.2068, 0.1081, 0.0498, 0.0177, 0.0026, 0.8886, 0.5849, 0.3937, 0.2658, 0.1739, 0.1065, 0.0570, 0.0217,
    0.7538, 0.5358, 0.3933, 0.2921, 0.2137, 0.1504, 0.0981, 0.0545, 0.6738, 0.5014, 0.3874, 0.3044, 0.2377, 0.1814, 0.1323, 0.0885,
    0.6227, 0.4772, 0.3811, 0.3105, 0.2530, 0.2033, 0.1589, 0.1180, 0.5882, 0.4599, 0.3757, 0.3138, 0.2631, 0.2190, 0.1790, 0.1418);

fn sheen_albedo(n_v: f32, r: f32) -> f32 {
    let x = clamp(n_v, 0.0, 1.0) * 7.0;
    let y = clamp((r - 0.05) / 0.95, 0.0, 1.0) * 7.0;
    let x0 = min(u32(x), 6u);
    let y0 = min(u32(y), 6u);
    let fx = x - f32(x0);
    let fy = y - f32(y0);
    var e = SHEEN_E;
    let a = mix(e[y0 * 8u + x0], e[y0 * 8u + x0 + 1u], fx);
    let b = mix(e[(y0 + 1u) * 8u + x0], e[(y0 + 1u) * 8u + x0 + 1u], fx);
    return mix(a, b, fy);
}

// Single-scattering albedo of this GGX (white, height-correlated Smith),
// by roughness 0..1 (rows) and cos 0..1 (columns), 16 × 16, measured by
// sampling visible normals: what multiple scattering would add back is
// 1 / E − 1 of it.
const E_SS = array<f32, 256>(
    0.9989, 0.9999, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000,
    0.9709, 0.9976, 0.9993, 0.9997, 0.9998, 0.9999, 0.9999, 0.9999, 0.9999, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000, 1.0000,
    0.8907, 0.9604, 0.9890, 0.9952, 0.9973, 0.9983, 0.9988, 0.9990, 0.9993, 0.9994, 0.9995, 0.9995, 0.9996, 0.9995, 0.9996, 0.9996,
    0.9182, 0.9005, 0.9511, 0.9752, 0.9855, 0.9905, 0.9930, 0.9948, 0.9958, 0.9965, 0.9972, 0.9974, 0.9978, 0.9979, 0.9983, 0.9983,
    0.9446, 0.8888, 0.9053, 0.9353, 0.9566, 0.9696, 0.9778, 0.9830, 0.9861, 0.9886, 0.9904, 0.9915, 0.9924, 0.9933, 0.9938, 0.9943,
    0.9594, 0.9007, 0.8799, 0.8958, 0.9163, 0.9345, 0.9486, 0.9580, 0.9654, 0.9713, 0.9746, 0.9782, 0.9807, 0.9825, 0.9839, 0.9852,
    0.9657, 0.9115, 0.8742, 0.8699, 0.8792, 0.8933, 0.9074, 0.9209, 0.9310, 0.9405, 0.9472, 0.9531, 0.9586, 0.9623, 0.9655, 0.9679,
    0.9676, 0.9176, 0.8733, 0.8543, 0.8508, 0.8560, 0.8645, 0.8763, 0.8869, 0.8974, 0.9060, 0.9143, 0.9213, 0.9275, 0.9330, 0.9374,
    0.9669, 0.9170, 0.8704, 0.8426, 0.8270, 0.8218, 0.8239, 0.8296, 0.8370, 0.8439, 0.8517, 0.8619, 0.8704, 0.8769, 0.8820, 0.8892,
    0.9639, 0.9113, 0.8597, 0.8248, 0.8034, 0.7909, 0.7839, 0.7819, 0.7838, 0.7861, 0.7917, 0.7977, 0.8056, 0.8116, 0.8194, 0.8242,
    0.9589, 0.9009, 0.8449, 0.8059, 0.7764, 0.7568, 0.7428, 0.7335, 0.7280, 0.7254, 0.7257, 0.7271, 0.7285, 0.7329, 0.7380, 0.7425,
    0.9535, 0.8882, 0.8252, 0.7791, 0.7443, 0.7184, 0.6989, 0.6835, 0.6703, 0.6630, 0.6562, 0.6520, 0.6506, 0.6499, 0.6497, 0.6486,
    0.9464, 0.8726, 0.7999, 0.7485, 0.7094, 0.6765, 0.6512, 0.6305, 0.6126, 0.5993, 0.5888, 0.5783, 0.5698, 0.5642, 0.5580, 0.5554,
    0.9389, 0.8540, 0.7749, 0.7163, 0.6691, 0.6323, 0.6022, 0.5750, 0.5545, 0.5357, 0.5178, 0.5021, 0.4900, 0.4826, 0.4709, 0.4630,
    0.9303, 0.8356, 0.7452, 0.6792, 0.6272, 0.5844, 0.5485, 0.5201, 0.4938, 0.4721, 0.4523, 0.4357, 0.4163, 0.4050, 0.3926, 0.3806,
    0.9210, 0.8161, 0.7147, 0.6420, 0.5838, 0.5383, 0.4995, 0.4639, 0.4366, 0.4109, 0.3890, 0.3696, 0.3504, 0.3358, 0.3195, 0.3071);

fn e_ss(r: f32, n_v: f32) -> f32 {
    let x = clamp(n_v, 0.0, 1.0) * 15.0;
    let y = clamp(r, 0.0, 1.0) * 15.0;
    let x0 = min(u32(x), 14u);
    let y0 = min(u32(y), 14u);
    let fx = x - f32(x0);
    let fy = y - f32(y0);
    var e = E_SS;
    let a = mix(e[y0 * 16u + x0], e[y0 * 16u + x0 + 1u], fx);
    let b = mix(e[(y0 + 1u) * 16u + x0], e[(y0 + 1u) * 16u + x0 + 1u], fx);
    return mix(a, b, fy);
}

// GGX in the (t, b, n) frame; w is a direction in that frame.
fn ggx_aniso(h: vec3<f32>, at: f32, ab: f32) -> f32 {
    let x = h.x / at;
    let y = h.y / ab;
    let d = x * x + y * y + h.z * h.z;
    return 1.0 / max(PI * at * ab * d * d, 1e-12);
}

fn smith_lambda(w: vec3<f32>, at: f32, ab: f32) -> f32 {
    let z2 = max(w.z * w.z, 1e-8);
    return (-1.0 + sqrt(1.0 + (at * at * w.x * w.x + ab * ab * w.y * w.y) / z2)) * 0.5;
}

// Visible normals (Heitz 2018): a microfacet normal seen from w.
fn sample_vndf(w: vec3<f32>, at: f32, ab: f32, u1: f32, u2: f32) -> vec3<f32> {
    let vh = normalize(vec3<f32>(at * w.x, ab * w.y, w.z));
    let lensq = vh.x * vh.x + vh.y * vh.y;
    var t1 = vec3<f32>(1.0, 0.0, 0.0);
    if lensq > 0.0 { t1 = vec3<f32>(-vh.y, vh.x, 0.0) / sqrt(lensq); }
    let t2v = cross(vh, t1);
    let r = sqrt(u1);
    let phi = 2.0 * PI * u2;
    let p1 = r * cos(phi);
    let s = 0.5 * (1.0 + vh.z);
    let p2 = (1.0 - s) * sqrt(max(1.0 - p1 * p1, 0.0)) + s * r * sin(phi);
    let nh = p1 * t1 + p2 * t2v + sqrt(max(0.0, 1.0 - p1 * p1 - p2 * p2)) * vh;
    return normalize(vec3<f32>(at * nh.x, ab * nh.y, max(0.0, nh.z)));
}

fn local(s: Surface, w: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(w, s.t), dot(w, s.b), dot(w, s.n));
}

fn surface(h: Hit, v: vec3<f32>) -> Surface {
    let m0 = b_mat[h.mat];
    let m1 = b_mat[h.mat + 1u];
    let m3 = b_mat[h.mat + 3u];
    let m4 = b_mat[h.mat + 4u];
    let m5 = b_mat[h.mat + 5u];
    let m6 = b_mat[h.mat + 6u];
    let m7 = b_mat[h.mat + 7u];
    let m8 = b_mat[h.mat + 8u];
    var s: Surface;
    let det = detail_at(h, h.n, m0.xyz);
    s.emit = det.emit;
    s.n = det.n;
    // Bent past the viewer: keep it just facing (no light from behind).
    if dot(s.n, v) < 0.02 { s.n = normalize(s.n + v * (0.02 - dot(s.n, v))); }
    let base = det.base;
    s.albedo = base;
    let metal = m1.x * det.metal_mul;
    s.rough = clamp(m1.y * det.rough_mul + det.rough, 0.03, 1.0);
    s.trans = m3.x;
    s.f0 = mix(vec3<f32>(m3.w) * m4.xyz, base, metal);
    s.f82 = mix(vec3<f32>(1.0), m4.xyz, metal);
    s.f90 = clamp(dot(s.f0, vec3<f32>(16.5)), 0.0, 1.0);
    let n_v = max(dot(s.n, v), 1e-4);
    let ab = env_brdf(s.rough, n_v);
    s.comp = vec3<f32>(1.0) + s.f0 * (1.0 / max(e_ss(s.rough, n_v), 1e-3) - 1.0);
    let e_spec = min((s.f0 * ab.x + vec3<f32>(s.f90 * ab.y)) * s.comp, vec3<f32>(1.0));
    s.sheen = m5.xyz;
    s.sheen_a = m5.w * m5.w;
    let e_sheen = sheen_albedo(n_v, m5.w) * max(max(s.sheen.x, s.sheen.y), s.sheen.z);
    s.kd = base * ((1.0 - metal) * (1.0 - s.trans) * (1.0 - e_sheen)) * (vec3<f32>(1.0) - e_spec);
    s.coat = m1.z;
    let cr = max(m1.w, 0.03);
    s.coat_a = cr * cr;
    s.coat_tint = mix(vec3<f32>(1.0), m6.xyz, s.coat);
    // The grain: around the mesh's axis, turned (as the raster).
    let alpha = s.rough * s.rough;
    s.at = alpha;
    s.ab = alpha;
    var t = vec3<f32>(0.0);
    let aniso = m4.w;
    if aniso > 0.0 {
        t = cross(m7.xyz, h.p - m8.xyz);
        t = t - s.n * dot(s.n, t);
    }
    if dot(t, t) < 1e-12 { t = basis(s.n)[0]; }
    t = normalize(t);
    if aniso > 0.0 {
        t = t * cos(m7.w) + cross(s.n, t) * sin(m7.w);
        let aspect = sqrt(1.0 - 0.9 * aniso);
        s.at = alpha * aspect;
        s.ab = alpha / aspect;
    }
    s.t = t;
    s.b = cross(s.n, t);
    // Sample each lobe by roughly what it reflects.
    let fc_v = 0.04 + 0.96 * pow5(1.0 - n_v);
    let w_coat = s.coat * fc_v;
    let under = (1.0 - w_coat) * luminance(s.coat_tint);
    let w_spec = luminance(e_spec) * under;
    let w_diff = (luminance(s.kd) + luminance(s.sheen) * 0.5) * under;
    let total = max(w_coat + w_spec + w_diff, 1e-6);
    s.p_coat = w_coat / total;
    s.p_spec = w_spec / total;
    if w_diff <= 0.0 && w_coat + w_spec <= 0.0 { s.p_spec = 1.0; }
    return s;
}

// BSDF · cos for light from `l` seen from `v` (reflection only).
fn bsdf_cos(s: Surface, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let lv = local(s, v);
    let ll = local(s, l);
    if ll.z <= 0.0 || lv.z <= 0.0 { return vec3<f32>(0.0); }
    let n_v = max(lv.z, 1e-4);
    let n_l = ll.z;
    let hl = normalize(lv + ll);
    let v_h = max(dot(lv, hl), 0.0);
    let d = ggx_aniso(hl, s.at, s.ab);
    let g2 = 1.0 / (1.0 + smith_lambda(lv, s.at, s.ab) + smith_lambda(ll, s.at, s.ab));
    let f = fresnel_f82(s.f0, s.f82, s.f90, v_h);
    let spec = f * s.comp * (d * g2 / (4.0 * n_v * n_l));
    var sheen = vec3<f32>(0.0);
    if s.sheen_a > 0.0 && dot(s.sheen, s.sheen) > 0.0 {
        sheen = s.sheen * (charlie(hl.z, s.sheen_a) * neubelt(n_v, n_l));
    }
    let fc_v = 0.04 + 0.96 * pow5(1.0 - n_v);
    var out = (s.kd / PI + spec + sheen) * s.coat_tint * (1.0 - s.coat * fc_v);
    if s.coat > 0.0 {
        let dc = ggx_aniso(hl, s.coat_a, s.coat_a);
        let gc = 1.0 / (1.0 + smith_lambda(lv, s.coat_a, s.coat_a) + smith_lambda(ll, s.coat_a, s.coat_a));
        let fc = 0.04 + 0.96 * pow5(1.0 - v_h);
        out += vec3<f32>(s.coat * fc * dc * gc / (4.0 * n_v * n_l));
    }
    return out * n_l;
}

// The pdf of sampling `l` from `v` (all lobes, as sample_bsdf chooses).
fn bsdf_pdf(s: Surface, v: vec3<f32>, l: vec3<f32>) -> f32 {
    let lv = local(s, v);
    let ll = local(s, l);
    if ll.z <= 0.0 || lv.z <= 0.0 { return 0.0; }
    let hl = normalize(lv + ll);
    let n_v = max(lv.z, 1e-4);
    var pdf = (1.0 - s.p_coat - s.p_spec) * ll.z / PI;
    let g1 = 1.0 / (1.0 + smith_lambda(lv, s.at, s.ab));
    pdf += s.p_spec * g1 * ggx_aniso(hl, s.at, s.ab) / (4.0 * n_v);
    if s.p_coat > 0.0 {
        let gc = 1.0 / (1.0 + smith_lambda(lv, s.coat_a, s.coat_a));
        pdf += s.p_coat * gc * ggx_aniso(hl, s.coat_a, s.coat_a) / (4.0 * n_v);
    }
    return pdf;
}

// Samples a reflected direction from one lobe; returns (direction, pdf of
// all lobes together).
fn sample_bsdf(s: Surface, v: vec3<f32>, rng: ptr<function, Rng>) -> vec4<f32> {
    let lv = local(s, v);
    let u0 = rnd(rng);
    let u1 = rnd(rng);
    let u2 = rnd(rng);
    var ll: vec3<f32>;
    if u0 < s.p_coat {
        let m = sample_vndf(lv, s.coat_a, s.coat_a, u1, u2);
        ll = reflect(-lv, m);
    } else if u0 < s.p_coat + s.p_spec {
        let m = sample_vndf(lv, s.at, s.ab, u1, u2);
        ll = reflect(-lv, m);
    } else {
        let r = sqrt(u1);
        let phi = 2.0 * PI * u2;
        ll = vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(1.0 - u1, 0.0)));
    }
    let l = s.t * ll.x + s.b * ll.y + s.n * ll.z;
    if ll.z <= 0.0 { return vec4<f32>(l, 0.0); }
    return vec4<f32>(l, bsdf_pdf(s, v, l));
}

// A point light's fade to its range (0: none), as the raster.
fn range_window(dist: f32, range: f32) -> f32 {
    if range <= 0.0 { return 1.0; }
    let x = dist / range;
    let w = clamp(1.0 - x * x * x * x, 0.0, 1.0);
    return w * w;
}

// Area lights (rows: [centre, 3], [colour, shadow], [range, ..],
// [u half extent, disc], [v half extent, area]); they shine along u × v.
fn area_point(row: u32, u1: f32, u2: f32) -> vec3<f32> {
    let c = b_light[row].xyz;
    let u = b_light[row + 3u];
    let w = b_light[row + 4u];
    if u.w > 0.5 {
        let r = sqrt(u1);
        let phi = 2.0 * PI * u2;
        return c + u.xyz * (r * cos(phi)) + w.xyz * (r * sin(phi));
    }
    return c + u.xyz * (2.0 * u1 - 1.0) + w.xyz * (2.0 * u2 - 1.0);
}

// Radiance per unit colour: a light of intensity 1 gives a surface facing
// it 1000 px away the irradiance of a directional light of intensity 1
// (π × colour, as light colours are irradiance / π), whatever its size.
fn area_radiance(row: u32) -> f32 {
    return PI * 1e6 / max(b_light[row + 4u].w, 1.0);
}

// Solid-angle pdf of sampling the panel uniformly, toward l at dist.
fn area_pdf(row: u32, p: vec3<f32>, l: vec3<f32>, dist: f32) -> f32 {
    let nl = normalize(cross(b_light[row + 3u].xyz, b_light[row + 4u].xyz));
    let cos_l = -dot(l, nl);
    if cos_l <= 1e-6 { return 0.0; }
    return dist * dist / (cos_l * max(b_light[row + 4u].w, 1.0));
}

// Where a ray from o along d meets the front of the panel (t, or −1).
fn area_hit(row: u32, o: vec3<f32>, d: vec3<f32>) -> f32 {
    let c = b_light[row].xyz;
    let u = b_light[row + 3u];
    let w = b_light[row + 4u];
    let nl = normalize(cross(u.xyz, w.xyz));
    let den = dot(d, nl);
    if den >= -1e-6 { return -1.0; }
    let t = dot(c - o, nl) / den;
    if t <= 0.0 { return -1.0; }
    let rel = o + d * t - c;
    let x = dot(rel, u.xyz) / max(dot(u.xyz, u.xyz), 1e-6);
    let y = dot(rel, w.xyz) / max(dot(w.xyz, w.xyz), 1e-6);
    if u.w > 0.5 {
        if x * x + y * y > 1.0 { return -1.0; }
    } else if abs(x) > 1.0 || abs(y) > 1.0 {
        return -1.0;
    }
    return t;
}

// Light from the area panels a ray (o, d) meets before tmax, found by
// sampling a surface with pdf `pdf` (0: a mirror bounce, no MIS).
fn area_emission(o: vec3<f32>, d: vec3<f32>, tmax: f32, pdf: f32) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        let row = 7u + i * LROWS;
        if u32(b_light[row].w) != 3u { continue; }
        let t = area_hit(row, o, d);
        if t <= 0.0 || t >= tmax { continue; }
        var w = 1.0;
        if pdf > 0.0 {
            let lp = area_pdf(row, o, d, t);
            w = pdf / (pdf + lp);
        }
        let col = b_light[row + 1u].xyz * range_window(length(b_light[row].xyz - o), b_light[row + 2u].x);
        out += col * area_radiance(row) * w;
    }
    return out;
}

struct LightSample {
    l: vec3<f32>,      // toward the light
    dist: f32,         // to it (1e30: directional)
    col: vec3<f32>,    // irradiance it gives facing it
    shadow: bool,      // casts shadows
}

// One light (directional or point) seen from `p`: its `softness` sizes
// it, so shadows soften as the raster's do (directional: a disc of sky up
// to 0.1 rad across; point: a ball up to 40 px in radius).
fn sample_light(i: u32, p: vec3<f32>, rng: ptr<function, Rng>) -> LightSample {
    let row = b_light[7u + i * LROWS];
    let col = b_light[7u + i * LROWS + 1u];
    let extra = b_light[7u + i * LROWS + 2u];
    var ls: LightSample;
    ls.shadow = col.w > 0.5;
    ls.col = col.xyz;
    let soft = extra.y;
    let u1 = rnd(rng);
    let u2 = rnd(rng);
    let r = sqrt(u1);
    let phi = 2.0 * PI * u2;
    let disc = vec2<f32>(r * cos(phi), r * sin(phi));
    if u32(row.w) == 3u {
        let q = area_point(7u + i * LROWS, u1, u2);
        let tt = q - p;
        ls.dist = length(tt);
        ls.l = tt / max(ls.dist, 1e-6);
        ls.col *= range_window(length(row.xyz - p), extra.x);
    } else if u32(row.w) == 1u {
        let to = row.xyz - p;
        let d = length(to);
        let c = to / max(d, 1e-6);
        let f = basis(c);
        let aim = row.xyz + (f[0] * disc.x + f[1] * disc.y) * (soft * 40.0);
        let tt = aim - p;
        ls.dist = length(tt);
        ls.l = tt / max(ls.dist, 1e-6);
        ls.col *= range_window(d, extra.x);
    } else {
        let c = normalize(row.xyz);
        let f = basis(c);
        ls.l = normalize(c + (f[0] * disc.x + f[1] * disc.y) * (soft * 0.1));
        ls.dist = 1e30;
    }
    return ls;
}

// Light arriving at `h` from the lights (directional and point), with
// shadow rays where the light casts shadows.
fn direct(s: Surface, h: Hit, v: vec3<f32>, rng: ptr<function, Rng>) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        let kind = u32(b_light[7u + i * LROWS].w);
        if kind == 2u { continue; }
        let ls = sample_light(i, h.p, rng);
        if dot(h.n, ls.l) <= 0.0 || dot(ls.col, ls.col) <= 0.0 { continue; }
        if F_AREA && kind == 3u {
            // A point on the panel: radiance / pdf, weighted against
            // finding it by sampling the surface (MIS, balance).
            let row = 7u + i * LROWS;
            let pdf = area_pdf(row, h.p, ls.l, ls.dist);
            if pdf <= 0.0 { continue; }
            if ls.shadow && occluded(offset(h, ls.l), ls.l, ls.dist * 0.999, rng) { continue; }
            let w = pdf / (pdf + bsdf_pdf(s, v, ls.l));
            out += bsdf_cos(s, v, ls.l) * area_radiance(row) * ls.col * (w / pdf);
            continue;
        }
        if ls.shadow && occluded(offset(h, ls.l), ls.l, ls.dist * 0.999, rng) { continue; }
        out += bsdf_cos(s, v, ls.l) * PI * ls.col;
    }
    // The environment's bright parts (an image by its brightness, or the
    // lobes), sampled directly (MIS with the BSDF).
    if has_image() {
        let e = env_image_sample(rng);
        if dot(h.n, e.xyz) > 0.0 {
            let pdf = env_image_pdf(e.xyz);
            if pdf > 0.0 && !occluded(offset(h, e.xyz), e.xyz, 1e30, rng) {
                let w = pdf / (pdf + bsdf_pdf(s, v, e.xyz));
                out += bsdf_cos(s, v, e.xyz) * env_image(e.xyz) * (w / pdf);
            }
        }
        return out;
    }
    let e = env_lobe_sample(rng);
    if e.w > 0.0 && dot(h.n, e.xyz) > 0.0 {
        let pdf = env_lobe_pdf(e.xyz);
        if pdf > 0.0 && !occluded(offset(h, e.xyz), e.xyz, 1e30, rng) {
            let w = pdf / (pdf + bsdf_pdf(s, v, e.xyz));
            out += bsdf_cos(s, v, e.xyz) * env_lobes(e.xyz) * (w / pdf);
        }
    }
    return out;
}

// Index of refraction for colour channel c (0 r, 1 g, 2 b) with dispersion
// as 1 / Abbe number (Cauchy's law through the d line).
fn ior_at(ior: f32, dispersion: f32, c: u32) -> f32 {
    if dispersion <= 0.0 { return ior; }
    let bb = (ior - 1.0) * dispersion / (1.0 / (0.4861 * 0.4861) - 1.0 / (0.6563 * 0.6563));
    let aa = ior - bb / (0.5876 * 0.5876);
    var lambda = 0.55;
    if c == 0u { lambda = 0.61; } else if c == 2u { lambda = 0.465; }
    return aa + bb / (lambda * lambda);
}

// What fraction of the shadow-casting light reaches a shadow catcher.
fn catcher_shadow(h: Hit, rng: ptr<function, Rng>) -> f32 {
    var blocked = 0.0;
    var total = 0.0;
    let lights = u32(b_light[0].x);
    for (var i = 0u; i < lights; i++) {
        let kind = u32(b_light[7u + i * LROWS].w);
        if kind == 2u { continue; }
        let ls = sample_light(i, h.p, rng);
        if !ls.shadow { continue; }
        let w = luminance(ls.col);
        total += w;
        if occluded(offset(h, ls.l), ls.l, ls.dist * 0.999, rng) { blocked += w; }
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
    prev: vec3<f32>,
    // Flat colour (cards, unlit materials) the camera sees first (a shadow
    // catcher is looked through), premultiplied: it skips the look (tone
    // curve, exposure), as in the raster, so it is kept apart from `color`;
    // seen through glass it is lit picture, denoised with the rest.
    flat: vec3<f32>,
    // How much of the flat colour shadow catchers in front let through
    // (1: none in the way); denoised apart from it, so a soft shadow on
    // footage is smooth while the footage keeps its exact colour.
    vis: f32,
}

// A plain diffuse surface facing n (albedo 1): where a subsurface walk
// leaves, its light is lit and continues as from a Lambert surface.
fn lambert_surface(n: vec3<f32>) -> Surface {
    var s: Surface;
    s.albedo = vec3<f32>(1.0);
    s.n = n;
    s.t = basis(n)[0];
    s.b = cross(n, s.t);
    s.at = 1.0;
    s.ab = 1.0;
    s.rough = 1.0;
    s.f0 = vec3<f32>(0.0);
    s.f82 = vec3<f32>(1.0);
    s.f90 = 0.0;
    s.comp = vec3<f32>(1.0);
    s.kd = vec3<f32>(1.0);
    s.sheen = vec3<f32>(0.0);
    s.sheen_a = 0.0;
    s.coat = 0.0;
    s.coat_a = 1.0;
    s.coat_tint = vec3<f32>(1.0);
    s.trans = 0.0;
    s.p_coat = 0.0;
    s.p_spec = 0.0;
    return s;
}

// The single-scattering albedo that makes a random walk (anisotropy g)
// reflect colour c in all: van de Hulst's relation (d'Eon, "A Hitchhiker's
// Guide to Multiple Scattering", Eq. 53.7), as Cycles' random walk.
fn walk_albedo(c: f32, g: f32) -> f32 {
    let s = 4.20863 * c - sqrt(9.59217 + 41.6808 * c + 17.7126 * c * c) + 4.09712;
    let s2 = s * s;
    let den = 1.0 - g * s2;
    if abs(den) < 1e-8 { return 0.0; }
    return clamp((1.0 - s2) / den, 0.0, 0.999999);
}

// Henyey–Greenstein phase function for the cosine between directions.
fn hg_eval(cos_t: f32, g: f32) -> f32 {
    let d = 1.0 + g * g - 2.0 * g * cos_t;
    return (1.0 - g * g) / (4.0 * PI * d * sqrt(max(d, 1e-8)));
}

fn max3(v: vec3<f32>) -> f32 { return max(v.x, max(v.y, v.z)); }
fn min3(v: vec3<f32>) -> f32 { return min(v.x, min(v.y, v.z)); }

// The walk's free-flight pdf per channel: of flying t and stopping
// (scattering), or of flying past t (leaving).
fn flight_pdf(sigma: vec3<f32>, t: f32, leaves: bool) -> vec3<f32> {
    let tr = exp(-sigma * t);
    if leaves { return tr; }
    return sigma * tr;
}

struct Walk {
    found: bool,
    h: Hit,               // where it leaves, its normals turned outward
    weight: vec3<f32>,
}

// Subsurface scattering, Cycles' random walk: the light enters below
// the surface (cosine-weighted), flies through a medium whose mean free
// path is radius × scale per colour and whose albedo makes the whole walk
// reflect the surface's colour, scattering at each stop, until it crosses
// the surface again. As Cycles, steps are guided back toward the surface
// (Dwivedi sampling; toward the far side when a thin object's is near) and
// weighed against classic sampling and across the colour channels (MIS),
// so pale materials that scatter long get out within the step budget.
fn sss_walk(h_in: Hit, n_in: vec3<f32>, base: vec3<f32>, rng: ptr<function, Rng>) -> Walk {
    var w: Walk;
    w.found = false;
    let r27 = b_mat[h_in.mat + 27u];
    let r28 = b_mat[h_in.mat + 28u];
    let g0 = r27.z;
    let a = clamp(base, vec3<f32>(0.0), vec3<f32>(1.0));
    var alpha = vec3<f32>(walk_albedo(a.x, g0), walk_albedo(a.y, g0), walk_albedo(a.z, g0));
    let sigma_t0 = 1.0 / max(r28.xyz * r27.y, vec3<f32>(1e-3));
    var weight = vec3<f32>(1.0);
    // Very dark: a floor on the albedo, the weight making up for it.
    for (var k = 0u; k < 3u; k++) {
        if alpha[k] < 0.2 {
            weight[k] *= alpha[k] / 0.2;
            alpha[k] = 0.2;
        }
    }
    let sigma_s0 = sigma_t0 * alpha;
    let dl = 1.0 / sqrt(max(1.0 - pow(max3(alpha), 2.44294 - 0.0215813 * max3(alpha) + 0.578637 / max3(alpha)), 1e-12));
    if dl <= 1.0 + 1e-6 { return w; }
    let phase_log = log((dl + 1.0) / (dl - 1.0));
    let guided0 = 1.0 - max(0.5, pow(abs(g0), 0.125));
    // Past a few bounces: isotropic with the similar (reduced) medium.
    let sigma_s_star = sigma_s0 * (1.0 - g0);
    let sigma_t_star = sigma_t0 - sigma_s0 + sigma_s_star;
    // The surface where it entered: its outward shading normal, with its
    // detail (normal map, bump), as Cycles.
    let n = n_in;
    let p0 = h_in.p;
    let fin = basis(-n);
    let u1 = rnd(rng);
    let rr = sqrt(u1);
    let phi = 2.0 * PI * rnd(rng);
    var d = normalize(fin[0] * (rr * cos(phi)) + fin[1] * (rr * sin(phi)) + fin[2] * sqrt(max(1.0 - u1, 0.0)));
    var o = offset(h_in, d);
    let fn_ = basis(n);
    var have_opposite = false;
    var opposite = 0.0;
    for (var bounce = 0u; bounce < 256u; bounce++) {
        var g = g0;
        var guided_fraction = guided0;
        var sigma_t = sigma_t0;
        var sigma_s = sigma_s0;
        if bounce > 9u {
            g = 0.0;
            guided_fraction = 0.75;
            sigma_t = sigma_t_star;
            sigma_s = sigma_s_star;
        }
        // A colour channel by albedo × weight (balance heuristic over them).
        let cw = alpha * weight;
        let cs = cw.x + cw.y + cw.z;
        if !(cs > 0.0) { return w; }
        let channel_pdf = cw / cs;
        let uc = rnd(rng);
        var c = 2u;
        if uc < channel_pdf.x { c = 0u; } else if uc < channel_pdf.x + channel_pdf.y { c = 1u; }
        var sample_sigma = sigma_t[c];
        var back_fraction = 0.0;
        var fwd_factor = 0.0;
        var back_factor = 0.0;
        var fwd_stretch = 1.0;
        var back_stretch = 1.0;
        if bounce > 0u {
            let guided = rnd(rng) < guided_fraction;
            var backward = false;
            if have_opposite {
                let x = clamp(dot(o - p0, -n), 0.0, opposite);
                back_fraction = 1.0 / (1.0 + exp((opposite - 2.0 * x) / dl));
                backward = rnd(rng) < back_fraction;
            }
            var cos_t: f32;
            var hg_pdf: f32;
            if guided {
                cos_t = dl - (dl + 1.0) * exp(-rnd(rng) * phase_log);
                if backward { cos_t = -cos_t; }
                let sn = sqrt(max(1.0 - cos_t * cos_t, 0.0));
                let ph = 2.0 * PI * rnd(rng);
                let nd = normalize(fn_[0] * (sn * cos(ph)) + fn_[1] * (sn * sin(ph)) + n * cos_t);
                hg_pdf = hg_eval(dot(d, nd), g);
                d = nd;
            } else {
                let nd = sample_hg(d, g, rnd(rng), rnd(rng));
                hg_pdf = hg_eval(dot(d, nd), g);
                cos_t = dot(nd, n);
                d = nd;
            }
            fwd_factor = 1.0 / (2.0 * PI * (dl - cos_t) * phase_log * hg_pdf);
            back_factor = 1.0 / (2.0 * PI * (dl + cos_t) * phase_log * hg_pdf);
            fwd_stretch = 1.0 - cos_t / dl;
            back_stretch = 1.0 + cos_t / dl;
            if guided {
                if backward { sample_sigma *= back_stretch; } else { sample_sigma *= fwd_stretch; }
            }
        }
        var t = -log(max(1.0 - rnd(rng), 1e-7)) / sample_sigma;
        // The first flight also looks further, for a thin object's far side.
        var reach_t = t;
        if bounce == 0u { reach_t = max(t, 10.0 / min3(sigma_t)); }
        let h = cast_ray(o, d, reach_t, rng);
        var hit = h.found && h.t < reach_t;
        if bounce == 0u {
            if hit {
                have_opposite = true;
                opposite = dot(o + d * h.t - p0, -n);
            }
            hit = hit && h.t < t;
        }
        if hit { t = h.t; }
        var pdf = flight_pdf(sigma_t, t, hit);
        if bounce > 0u {
            var gpdf = flight_pdf(sigma_t * fwd_stretch, t, hit);
            if have_opposite {
                let bpdf = flight_pdf(sigma_t * back_stretch, t, hit);
                gpdf = mix(gpdf * fwd_factor, bpdf * back_factor, back_fraction);
            } else {
                gpdf *= fwd_factor;
            }
            pdf = mix(pdf, gpdf, guided_fraction);
        }
        let tr = exp(-sigma_t * t);
        var f = sigma_s * tr;
        if hit { f = tr; }
        weight *= f / max(dot(channel_pdf, pdf), 1e-12);
        if hit {
            var hx = h;
            hx.n = -h.n;
            hx.g = -h.g;
            w.found = true;
            w.h = hx;
            w.weight = weight;
            return w;
        }
        o = o + d * t;
    }
    return w;
}

fn path(o_in: vec3<f32>, d_in: vec3<f32>, rng: ptr<function, Rng>) -> Sample {
    var out: Sample;
    out.vis = 1.0;
    var o = o_in;
    var d = d_in;
    var throughput = vec3<f32>(1.0);
    // Only see-through events so far (glass, opacity): a path that then
    // leaves the scene shows what is behind the layer list, not the sky.
    var through = true;
    // Inside glass: what it absorbs per px (Beer–Lambert).
    var inside = false;
    var sigma = vec3<f32>(0.0);
    // Dispersion: the colour channel this path carries (3: all of them).
    var hero = 3u;
    let bounces = u.opts.x;
    // Scattering off surfaces counts against `bounces`; passing through
    // glass and see-through layers has its own, larger allowance.
    var bounce = 0u;
    if PROFILE { vol_depth = 0u; }
    // The pdf of the last bounce (0: a mirror-like event), for MIS on the
    // area lights it reaches.
    var last_pdf = 0.0;
    // The step whose surface gives the denoiser its guides: the first one
    // the camera sees (a shadow catcher is looked through).
    var guide = 0u;
    // The volumes the path is inside: material rows and, to find them
    // again on the way out, instances (a mesh's faces have a row per part).
    // Overlapping clouds are several at once.
    var vols = array<u32, 4>(0u, 0u, 0u, 0u);
    var vinst = array<u32, 4>(0u, 0u, 0u, 0u);
    var nvol = 0u;
    for (var step = 0u; step < bounces + 16u; step++) {
        let h = cast_ray(o, d, 1e30, rng);
        if F_VOLUME && nvol > 0u {
            // Free flight to a scattering point, or through to the next
            // surface. Glow adds along the way.
            var seg = 1e30;
            if h.found { seg = h.t; }
            var scatters = false;
            var p = o;
            var sv = vols[0];
            // Whether single scattering was gathered along this stretch (an
            // even medium): a scatter then adds no direct light of its own.
            var gathered = false;
            var artistic = false;
            for (var i = 0u; i < nvol; i++) { artistic = artistic || vol_artistic(vols[i]); }
            if artistic {
                // Marched as the preview draws it: 16 jittered steps to the
                // next surface, light gathered on the way, the path going on
                // behind the medium dimmed by what it let through.
                if seg < 1e29 {
                    let dt = seg / 16.0;
                    let j = rnd(rng);
                    var tr = 1.0;
                    var lit = vec3<f32>(0.0);
                    for (var s = 0u; s < 16u; s++) {
                        let q = o + d * ((f32(s) + j) * dt);
                        var ext = 0.0;
                        var col = vec3<f32>(0.0);
                        for (var i = 0u; i < nvol; i++) {
                            let rho = volume_density(vols[i], q);
                            if rho <= 0.0 { continue; }
                            let sig = b_mat[vols[i] + 18u].x * rho;
                            ext += sig;
                            col += sig * (b_mat[vols[i] + 19u].xyz * artistic_lights(vols[i], q, d, rng) + b_mat[vols[i] + 20u].xyz);
                        }
                        if ext <= 0.0 { continue; }
                        let a = 1.0 - exp(-ext * dt);
                        lit += tr * a * col / ext;
                        tr *= 1.0 - a;
                    }
                    out.color += throughput * lit;
                    throughput *= tr;
                    out.alpha = max(out.alpha, 1.0 - tr);
                }
            } else if !F_VOL_PHYSICAL {
                // This build has no physical volumes.
            } else if nvol == 1u && b_mat[vols[0] + 30u].z < 0.5 {
                // One even medium.
                let sigma = max(b_mat[sv + 18u].x, 1e-6);
                let s_free = -log(max(1.0 - rnd(rng), 1e-7)) / sigma;
                out.color += throughput * b_mat[sv + 20u].xyz * (sigma * min(s_free, seg));
                // Light scattered once toward the camera: gathered on every
                // path through the medium, at a point drawn along this
                // stretch by its transmittance, weighted by the chance of
                // scattering in it (1 − T). A path that does scatter below
                // carries only the light of further scattering, so nothing
                // is counted twice and thin haze is smooth instead of specks.
                if bounce < bounces {
                    let tr = exp(-sigma * min(seg, 1e6));
                    let ts = -log(max(1.0 - rnd(rng) * (1.0 - tr), 1e-7)) / sigma;
                    out.color += throughput * b_mat[sv + 19u].xyz * (1.0 - tr) * volume_direct(o + d * ts, d, b_mat[sv + 18u].y, rng);
                    gathered = true;
                }
                scatters = s_free < seg;
                p = o + d * s_free;
            } else {
                // Noise or overlapping media: delta tracking, piecewise
                // through each medium's bounds cells (Cycles' octree idea
                // on a grid): empty cells are skipped and inside one the
                // walk steps at the media's summed cell bounds × sigma.
                // Each tentative collision adds their glow (collision
                // estimator); a real one picks the medium that scatters by
                // its share.
                var q0: array<vec3<f32>, 4>;
                var dq: array<vec3<f32>, 4>;
                for (var i = 0u; i < nvol; i++) {
                    q0[i] = vol_local(vols[i], o);
                    dq[i] = vol_dir(vols[i], d);
                }
                var t = 0.0;
                for (var k = 0u; k < 1024u; k++) {
                    // The majorant up to the nearest cell face.
                    var major = 0.0;
                    var until = seg;
                    for (var i = 0u; i < nvol; i++) {
                        let c = bounds_cell(vols[i], q0[i] + dq[i] * t, dq[i]);
                        major += max(b_mat[vols[i] + 18u].x, 1e-6) * c.bound;
                        until = min(until, t + c.exit);
                    }
                    var step = 1e30;
                    if major > 0.0 { step = -log(max(1.0 - rnd(rng), 1e-7)) / major; }
                    if t + step >= until {
                        if until >= seg { break; }
                        t = until;
                        continue;
                    }
                    t += step;
                    let q = o + d * t;
                    var w = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
                    var real = 0.0;
                    for (var i = 0u; i < nvol; i++) {
                        w[i] = max(b_mat[vols[i] + 18u].x, 1e-6) * volume_density(vols[i], q);
                        out.color += throughput * b_mat[vols[i] + 20u].xyz * (w[i] / major);
                        real += w[i];
                    }
                    var pick = rnd(rng) * major;
                    if pick < real {
                        for (var i = 0u; i < nvol; i++) {
                            sv = vols[i];
                            if pick < w[i] { break; }
                            pick -= w[i];
                        }
                        scatters = true;
                        p = q;
                        break;
                    }
                }
            }
            if F_VOL_PHYSICAL && scatters {
                if PROFILE { vol_depth = min(vol_depth + 1u, 3u); }
                let v18 = b_mat[sv + 18u];
                throughput *= b_mat[sv + 19u].xyz;
                // Noise and overlapping media gather nothing on the way:
                // light reaching the scattering point, here.
                if !gathered { out.color += throughput * volume_direct(p, d, v18.y, rng); }
                through = false;
                out.alpha = 1.0;
                bounce++;
                if bounce > bounces { return out; }
                if bounce >= 3u {
                    let q = clamp(max(throughput.x, max(throughput.y, throughput.z)), 0.05, 0.95);
                    if rnd(rng) > q { return out; }
                    throughput /= q;
                }
                d = sample_hg(d, v18.y, rnd(rng), rnd(rng));
                o = p;
                last_pdf = 0.0;
                continue;
            }
        }
        if !through {
            // Area lights are seen in reflections, not by the camera.
            var tmax = 1e30;
            if h.found { tmax = h.t; }
            if F_AREA { out.color += throughput * area_emission(o, d, tmax, last_pdf); }
        }
        if !h.found {
            if !through {
                // Lobes were also sampled directly at the last surface:
                // weigh this sample against that (MIS).
                var w = 1.0;
                if has_image() {
                    if last_pdf > 0.0 { w = last_pdf / (last_pdf + env_image_pdf(d)); }
                    out.color += throughput * (env_image(d) * w + env_ambient());
                } else {
                    if last_pdf > 0.0 { w = last_pdf / (last_pdf + env_lobe_pdf(d)); }
                    out.color += throughput * (env_smooth(d) + env_lobes(d) * w);
                }
                out.alpha = 1.0;
            }
            return out;
        }
        if inside { throughput *= exp(-sigma * h.t); }
        if step == guide { out.depth = h.t; out.normal = h.n; out.prev = h.prev; }
        if F_CARDS && h.card {
            let c = h.card_color;
            let straight = vec3<f32>(decode(c.x / c.w), decode(c.y / c.w), decode(c.z / c.w));
            if step == guide { out.flat += straight; } else { out.color += throughput * straight; }
            out.alpha = 1.0;
            if step == guide { out.albedo = straight; }
            return out;
        }
        let m0 = b_mat[h.mat];
        let m1 = b_mat[h.mat + 1u];
        let m2 = b_mat[h.mat + 2u];
        let m3 = b_mat[h.mat + 3u];
        let flags = u32(m2.w);
        let v = -d;
        if F_VOLUME && (flags & FLAG_VOLUME) != 0u {
            // A volume's boundary: entering or leaving its medium.
            if step == guide { out.albedo = vec3<f32>(1.0); }
            if h.front {
                if nvol < 4u {
                    vols[nvol] = h.mat;
                    vinst[nvol] = h.inst;
                    nvol++;
                }
            } else {
                for (var i = 0u; i < nvol; i++) {
                    if vinst[i] == h.inst {
                        nvol--;
                        vols[i] = vols[nvol];
                        vinst[i] = vinst[nvol];
                        break;
                    }
                }
            }
            o = offset(h, d);
            continue;
        }
        if step == guide { out.albedo = m0.xyz; }
        if F_CATCHER && (flags & FLAG_SHADOW_CATCHER) != 0u {
            if through {
                // Only the shadows: their darkness covers what is behind
                // (cards, meshes) or, over nothing, is the coverage.
                let a = catcher_shadow(h, rng) * m0.w;
                out.color += throughput * m0.xyz * a;
                out.alpha = max(out.alpha, a);
                throughput *= 1.0 - a;
                if step == guide {
                    guide = step + 1u;
                    out.vis *= 1.0 - a;
                }
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
            let det = detail_at(h, h.n, m0.xyz);
            let c = det.base + m2.xyz * det.emit;
            if step == guide { out.flat += c; } else { out.color += throughput * c; }
            out.alpha = 1.0;
            return out;
        }
        out.alpha = 1.0;
        var s = surface(h, v);
        out.color += throughput * m2.xyz * s.emit;
        if step == guide { out.albedo = s.albedo; }
        // Glass: a dielectric boundary (rough: on a visible microfacet),
        // reflecting or refracting by exact Fresnel.
        if F_GLASS && s.trans > 0.0 && rnd(rng) < s.trans {
            let m6 = b_mat[h.mat + 6u];
            if m6.w > 0.0 && hero == 3u {
                // Dispersion: carry one colour from here on.
                hero = min(u32(rnd(rng) * 3.0), 2u);
                var keep = vec3<f32>(0.0);
                keep[hero] = 3.0;
                throughput *= keep;
            }
            var ior = max(m3.y, 1.0);
            if hero < 3u { ior = ior_at(ior, m6.w, hero); }
            var eta = 1.0 / ior;
            if !h.front { eta = ior; }
            let lv = local(s, v);
            var m = vec3<f32>(0.0, 0.0, 1.0);
            let glass_a = s.rough * s.rough;
            if s.rough > 0.04 { m = sample_vndf(lv, glass_a, glass_a, rnd(rng), rnd(rng)); }
            let cosi = clamp(dot(lv, m), 0.0, 1.0);
            let fr = fresnel_dielectric(cosi, eta);
            let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
            var ll: vec3<f32>;
            var refracted = false;
            if k < 0.0 || rnd(rng) < fr {
                ll = reflect(-lv, m);
                through = false;
            } else {
                ll = normalize(-lv * eta + m * (eta * cosi - sqrt(k)));
                refracted = true;
            }
            if s.rough > 0.04 {
                // Visible-normal sampling leaves the outgoing shadowing.
                let lw = vec3<f32>(ll.x, ll.y, abs(ll.z));
                throughput *= 1.0 / (1.0 + smith_lambda(lw, glass_a, glass_a));
            }
            d = s.t * ll.x + s.b * ll.y + s.n * ll.z;
            last_pdf = 0.0;
            if refracted {
                if h.two_sided {
                    // A thin pane: straight through, tinted once.
                    d = -v;
                    throughput *= m0.xyz;
                } else if h.front {
                    inside = true;
                    sigma = -log(max(m0.xyz, vec3<f32>(1e-4))) / max(m3.z, 1.0);
                } else {
                    inside = false;
                }
            }
            o = offset(h, d);
            continue;
        }
        through = false;
        // Subsurface: that share of the diffuse light goes into the body
        // (a random walk) instead of bouncing here.
        let sss = b_mat[h.mat + 27u].x;
        var kd_in = vec3<f32>(0.0);
        if sss > 0.0 {
            kd_in = s.kd * sss;
            s.kd *= 1.0 - sss;
        }
        out.color += throughput * direct(s, h, v, rng);
        bounce++;
        if bounce > bounces { return out; }
        // The walk is chosen as often as the diffuse lobe it replaces.
        let p_sss = sss * (1.0 - s.p_coat - s.p_spec);
        if F_SSS && p_sss > 0.0 && rnd(rng) < p_sss {
            let wk = sss_walk(h, s.n, s.albedo, rng);
            if !wk.found { return out; }
            // The walk reflects about the base colour; kd_in is that colour
            // times what the layers above it leave.
            throughput *= kd_in / max(s.albedo, vec3<f32>(1e-4)) * wk.weight / p_sss;
            // It leaves as from a diffuse surface, shaded with the exit's
            // detail (normal map, bump) like the entry.
            var nx = detail_at(wk.h, wk.h.n, vec3<f32>(1.0)).n;
            if dot(nx, wk.h.g) < 0.02 { nx = wk.h.n; }
            let sx = lambert_surface(nx);
            out.color += throughput * direct(sx, wk.h, nx, rng);
            bounce++;
            if bounce > bounces { return out; }
            let sm = sample_bsdf(sx, nx, rng);
            if sm.w <= 0.0 { return out; }
            throughput *= bsdf_cos(sx, nx, sm.xyz) / sm.w;
            last_pdf = sm.w;
            if bounce >= 3u {
                let q = clamp(max(throughput.x, max(throughput.y, throughput.z)), 0.05, 0.95);
                if rnd(rng) > q { return out; }
                throughput /= q;
            }
            d = sm.xyz;
            o = offset(wk.h, d);
            continue;
        }
        let smp = sample_bsdf(s, v, rng);
        if smp.w <= 0.0 { return out; }
        let l = smp.xyz;
        throughput *= bsdf_cos(s, v, l) / (smp.w * (1.0 - p_sss));
        last_pdf = smp.w * (1.0 - p_sss);
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
fn trace(@builtin(global_invocation_id) id_in: vec3<u32>) {
    let w = u.size.x;
    let h = u.size.y;
    // Rows come in bands (one dispatch and submission each, see `trace`
    // in pt.rs): this band covers rows band.x up to band.y.
    let py = id_in.y + u.band.x;
    let id = vec3<u32>(id_in.x, py, id_in.z);
    if id.x >= w || py >= h || py >= u.band.y { return; }
    prof_on = PROFILE && (id.x & 7u) == 0u && (py & 7u) == 0u;
    let spp = u.size.w;
    var rng: Rng;
    rng.s = pcg(id.x + pcg(py + pcg(u.opts.y)));
    var color = vec3<f32>(0.0);
    var alpha = 0.0;
    var albedo = vec3<f32>(0.0);
    var normal = vec3<f32>(0.0);
    var depth = 0.0;
    var hits = 0.0;
    var m1 = 0.0;
    var m2 = 0.0;
    var camz = 0.0;
    var prev = vec4<f32>(0.0);
    var flat = vec3<f32>(0.0);
    var vis = 0.0;
    for (var s = 0u; s < spp; s++) {
        // A point in the texel (stratified jitter), target → canvas → ray.
        let jx = (f32(s % 4u) + rnd(&rng)) / 4.0;
        let jy = (f32((s / 4u) % 4u) + rnd(&rng)) / 4.0;
        let tp = vec3<f32>(u.place.x + u.place.z * (f32(id.x) + jx), u.place.y + u.place.w * (f32(py) + jy), 1.0);
        let q = vec3<f32>(dot(u.inv0.xyz, tp), dot(u.inv1.xyz, tp), dot(u.inv2.xyz, tp));
        let c = q.xy / q.z;
        var dir = normalize(vec3<f32>((c.x - u.cam.y) / u.cam.x, (c.y - u.cam.z) / u.cam.x, 1.0));
        var origin = vec3<f32>(0.0);
        if u.lens.x > 0.0 {
            // Thin lens: a point on the aperture, aimed at where the pinhole
            // ray meets the focus plane.
            let focus = dir * (u.lens.y / dir.z);
            let r = u.lens.x * sqrt(rnd(&rng));
            let phi = 2.0 * PI * rnd(&rng);
            origin = vec3<f32>(r * cos(phi), r * sin(phi), 0.0);
            dir = normalize(focus - origin);
        }
        let smp = path(origin, dir, &rng);
        color += smp.color;
        flat += smp.flat;
        vis += smp.vis;
        alpha += smp.alpha;
        let l = luminance(smp.color);
        m1 += l;
        m2 += l * l;
        if s == 0u && smp.depth > 0.0 {
            // Motion: where the first surface was last frame, in this
            // group's texels.
            camz = (origin + dir * smp.depth).z;
            if u.prev_cam.w > 0.5 && smp.prev.z > 1.0 {
                let sp = vec3<f32>(u.prev_cam.x * smp.prev.x / smp.prev.z + u.prev_cam.y, u.prev_cam.x * smp.prev.y / smp.prev.z + u.prev_cam.z, 1.0);
                let tq = vec3<f32>(dot(u.prev_root0.xyz, sp), dot(u.prev_root1.xyz, sp), dot(u.prev_root2.xyz, sp));
                let tp = tq.xy / tq.z;
                prev = vec4<f32>((tp.x - u.prev_place.x) / u.prev_place.z - 0.5, (tp.y - u.prev_place.y) / u.prev_place.w - 0.5, smp.prev.z, 1.0);
            }
        }
        if smp.depth > 0.0 {
            albedo += smp.albedo;
            normal += smp.normal;
            depth += smp.depth;
            hits += 1.0;
        }
    }
    let n = f32(spp);
    color /= n;
    flat /= n;
    vis /= n;
    alpha /= n;
    let i = py * w + id.x;
    // Guides: what the samples hit, on average.
    var a = vec3<f32>(1.0);
    if hits > 0.0 {
        a = max(albedo / hits, vec3<f32>(0.02));
        normal = normalize(normal + vec3<f32>(0.0, 0.0, -1e-6));
        depth /= hits;
    }
    // Out of focus by more than a texel (the lens's circle of confusion
    // at this depth): the samples saw a blur of different surfaces, so
    // their average colour is noise, not texture. Filter the whole colour
    // there (a white guide) and let the filter reach across the blur.
    var blur = 0.0;
    if u.lens.x > 0.0 && camz > 0.0 {
        let coc = u.cam.x * u.lens.x * abs(camz - u.lens.y) / (camz * max(u.lens.y, 1.0)) / max(u.place.z, 1e-6);
        blur = clamp(coc - 1.0, 0.0, 1.0);
    }
    a = mix(a, vec3<f32>(1.0), blur);
    b_guides[i * 4u] = vec4<f32>(a, alpha);
    b_guides[i * 4u + 1u] = vec4<f32>(normal, camz);
    b_guides[i * 4u + 2u] = vec4<f32>(depth, flat);
    b_guides[i * 4u + 3u] = prev;
    // The lighting, straight (not times coverage) and with the surface's
    // colour divided out, so the filter smooths light, not texture.
    // Light with little coverage (glowing volumes, highlights on glass
    // over nothing) still needs coverage to show.
    alpha = max(alpha, min(max(color.x, max(color.y, color.z)), 1.0));
    var straight = vec3<f32>(0.0);
    if alpha > 0.0 { straight = color / alpha; }
    let var_l = max(m2 / n - (m1 / n) * (m1 / n), 0.0) / n;
    let al = max(luminance(a), 0.02);
    b_hdr[i * 2u] = vec4<f32>(straight / a, alpha);
    b_hdr[i * 2u + 1u] = vec4<f32>(var_l / (al * al) / max(alpha * alpha, 1e-4), blur, vis, 0.0);
    if PROFILE {
        // The lockstep block: its lanes' lookups summed, and the most any did.
        let blk = PROF_HEADER + ((id.y / 4u) * ((w + 7u) / 8u) + id.x / 8u) * 2u;
        if blk + 1u < arrayLength(&b_prof) {
            atomicAdd(&b_prof[blk], px_lookups);
            atomicMax(&b_prof[blk + 1u], px_lookups);
        }
    }
}

// Temporal accumulation: last frame's lighting at where this surface was
// (its motion, camera and object), if it was the same surface there (camera
// depth and normal agree), clamped to what this frame's neighbourhood
// allows (no ghosts when light changes), blended in by how long it has
// been seen. Keeps animation from boiling.
@compute @workgroup_size(8, 8, 1)
fn temporal(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u.size.x;
    let h = u.size.y;
    if id.x >= w || id.y >= h { return; }
    let i = id.y * w + id.x;
    let c = b_hdr[i * 2u];
    let var_c = b_hdr[i * 2u + 1u].x;
    let g1 = b_guides[i * 4u + 1u];
    let mv = b_guides[i * 4u + 3u];
    var out = c.xyz;
    var var_o = var_c;
    var frames = 1.0;
    if c.w > 0.0 && mv.w > 0.5 && u.denoise.y == 1u {
        // This frame's neighbourhood: mean and spread of the lighting.
        var m1 = vec3<f32>(0.0);
        var m2 = vec3<f32>(0.0);
        var k = 0.0;
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                let x = i32(id.x) + dx;
                let y = i32(id.y) + dy;
                if x < 0 || y < 0 || x >= i32(w) || y >= i32(h) { continue; }
                let q = b_hdr[(u32(y) * w + u32(x)) * 2u];
                if q.w <= 0.0 { continue; }
                m1 += q.xyz;
                m2 += q.xyz * q.xyz;
                k += 1.0;
            }
        }
        m1 /= k;
        let sd = sqrt(max(m2 / k - m1 * m1, vec3<f32>(0.0)));
        let px = i32(round(mv.x));
        let py = i32(round(mv.y));
        if px >= 0 && py >= 0 && px < i32(w) && py < i32(h) {
            let j = u32(py) * w + u32(px);
            let hz = b_hist[j * 3u + 1u];
            let same = hz.x > 0.0 && abs(hz.y - mv.z) < 0.03 * max(mv.z, 1.0) && dot(b_hist[j * 3u + 2u].xyz, g1.xyz) > 0.8;
            if same {
                let hist = clamp(b_hist[j * 3u].xyz, m1 - 1.5 * sd, m1 + 1.5 * sd);
                frames = min(hz.x + 1.0, 32.0);
                let a = max(1.0 / frames, 0.15);
                out = mix(hist, c.xyz, a);
                var_o = var_c * a;
            }
        }
    }
    b_tmp[i * 2u] = vec4<f32>(out, c.w);
    b_tmp[i * 2u + 1u] = vec4<f32>(var_o, b_hdr[i * 2u + 1u].y, b_hdr[i * 2u + 1u].z, 0.0);
    var seen = frames;
    if c.w <= 0.0 { seen = 0.0; }
    b_next[i * 3u] = vec4<f32>(out, c.w);
    b_next[i * 3u + 1u] = vec4<f32>(seen, g1.w, 0.0, 0.0);
    b_next[i * 3u + 2u] = vec4<f32>(g1.xyz, 0.0);
}

// One à-trous pass (SVGF): a 5 × 5 B-spline kernel `step` px apart, each
// tap weighed by how alike its surface is (depth, normal, coverage) and
// its lighting (against the noise expected there).
@compute @workgroup_size(8, 8, 1)
fn atrous(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u.size.x;
    let h = u.size.y;
    if id.x >= w || id.y >= h { return; }
    let i = id.y * w + id.x;
    let c = b_hdr[i * 2u];
    let cv = b_hdr[i * 2u + 1u].x;
    let blur = b_hdr[i * 2u + 1u].y;
    if c.w <= 0.0 {
        b_tmp[i * 2u] = c;
        b_tmp[i * 2u + 1u] = b_hdr[i * 2u + 1u];
        return;
    }
    let n0 = b_guides[i * 4u + 1u].xyz;
    let z0 = b_guides[i * 4u + 2u].x;
    let l0 = luminance(c.xyz);
    let step = i32(u.denoise.x);
    let sigma_l = 4.0 * sqrt(max(cv, 0.0)) + 1e-4;
    var kern = array<f32, 3>(0.375, 0.25, 0.0625);
    var sum = vec3<f32>(0.0);
    var wsum = 0.0;
    var vsum = 0.0;
    var zsum = 0.0;
    for (var dy = -2; dy <= 2; dy++) {
        for (var dx = -2; dx <= 2; dx++) {
            let x = i32(id.x) + dx * step;
            let y = i32(id.y) + dy * step;
            if x < 0 || y < 0 || x >= i32(w) || y >= i32(h) { continue; }
            let j = u32(y) * w + u32(x);
            let q = b_hdr[j * 2u];
            if q.w <= 0.0 { continue; }
            let k = kern[abs(dx)] * kern[abs(dy)];
            let wz = exp(-abs(z0 - b_guides[j * 4u + 2u].x) / ((0.01 * max(z0, 1.0) * f32(step) + 1.0) * (1.0 + 30.0 * blur)));
            let wn = pow(max(dot(n0, b_guides[j * 4u + 1u].xyz), 0.0), mix(64.0, 1.0, blur));
            let wl = exp(-abs(l0 - luminance(q.xyz)) / sigma_l);
            let wa = exp(-abs(c.w - q.w) * 8.0);
            let wt = k * wz * wn * wl * wa;
            sum += q.xyz * wt;
            wsum += wt;
            vsum += wt * wt * b_hdr[j * 2u + 1u].x;
            zsum += wt * b_hdr[j * 2u + 1u].z;
        }
    }
    // denoise.w: the lighting is denoised already (OIDN); only smooth
    // the shadow catchers' visibility.
    var lighting = sum / max(wsum, 1e-8);
    if u.denoise.w == 1u { lighting = c.xyz; }
    b_tmp[i * 2u] = vec4<f32>(lighting, c.w);
    b_tmp[i * 2u + 1u] = vec4<f32>(vsum / max(wsum * wsum, 1e-12), blur, zsum / max(wsum, 1e-8), 0.0);
}

// The picture: lighting × albedo × coverage, tone-mapped as the raster,
// premultiplied sRGB into the group.
@compute @workgroup_size(8, 8, 1)
fn resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u.size.x;
    let h = u.size.y;
    if id.x >= w || id.y >= h { return; }
    let i = id.y * w + id.x;
    let c = b_hdr[i * 2u];
    let alpha = c.w;
    let color = c.xyz * b_guides[i * 4u].xyz * alpha;
    // Cards and unlit materials seen directly: not tone-mapped.
    let flat = b_guides[i * 4u + 2u].yzw * b_hdr[i * 2u + 1u].z;
    // Highlights on glass over nothing still need coverage to show.
    let lit = color + flat;
    let peak = min(max(lit.x, max(lit.y, lit.z)), 1.0);
    let a = max(alpha, peak);
    var out = vec4<f32>(0.0);
    if a > 0.0 {
        var e = b_light[1].w;
        if e <= 0.0 { e = 1.0; }
        let tm = (tone_curve(color / a * e, u32(b_light[2].w)) * a + flat) / a;
        out = vec4<f32>(encode(tm.x) * a, encode(tm.y) * a, encode(tm.z) * a, a);
    }
    b_groups[u.size.z + i] = out;
}
