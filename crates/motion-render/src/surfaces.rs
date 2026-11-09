//! Slang materials in the renderers (`lahza_motion_core::surface`): the
//! material rows a Slang surface adds, and the GPU's raster and path
//! tracer shader variants that call the project's functions. The CPU calls
//! them in `mesh3d::slang_surface`.
//!
//! A Slang surface is never a volume, so it takes the volume's rows: 18
//! [dispatch id, CPU function address in three 16-bit parts], 19–20 the
//! params, 21–23 camera → object space (3 × 4), 24 [camera position in
//! the world, local seconds], 31–33 object → camera space's columns (the
//! normal's way back).

use crate::mesh3d::{affine_inverse, MAT_ROWS};
use lahza_motion_core::compile::{MaterialState, MeshState};
use lahza_motion_core::surface::Program;

/// Material flag: a Slang function shades it.
pub const FLAG_SLANG: u32 = 128;

/// Adds the rows of `mat`'s Slang surface; nothing unless it linked (its
/// errors are the project's).
pub(crate) fn rows(m: &MeshState, mat: &MaterialState, rows: &mut [[f32; 4]; MAT_ROWS]) {
    let Some(s) = &mat.slang else { return };
    let Some(address) = s.address() else { return };
    let mv = m.camera.view.then(m.world).0;
    let (Some(inv), Some(cam)) = (affine_inverse(mv), affine_inverse(m.camera.view.0)) else { return };
    // User-space addresses fit in 48 bits: three exact 16-bit floats.
    let a = address as usize as u64;
    rows[18] = [s.id as f32, (a & 0xffff) as f32, (a >> 16 & 0xffff) as f32, (a >> 32 & 0xffff) as f32];
    rows[19] = [s.params[0], s.params[1], s.params[2], s.params[3]];
    rows[20] = [s.params[4], s.params[5], s.params[6], s.params[7]];
    for r in 0..3 {
        rows[21 + r] = [inv[r * 4] as f32, inv[r * 4 + 1] as f32, inv[r * 4 + 2] as f32, inv[r * 4 + 3] as f32];
        rows[31 + r] = [mv[r] as f32, mv[4 + r] as f32, mv[8 + r] as f32, 0.0];
    }
    rows[24] = [cam[3] as f32, cam[7] as f32, cam[11] as f32, s.time as f32];
    rows[2][3] = (rows[2][3] as u32 | FLAG_SLANG) as f32;
}

/// The CPU function in rows 18 (0: none).
pub(crate) fn address(r18: [f32; 4]) -> u64 {
    r18[1] as u64 | (r18[2] as u64) << 16 | (r18[3] as u64) << 32
}

/// Lines of `shade_tri` in motion.wgsl the variant changes: the material
/// rows become variables the Slang surface writes, after the surface
/// detail.
const HOOKS: [(&str, &str); 4] = [
    ("    let m0 = b_mesh[mb + mat * MAT_ROWS];\n", "    var m0 = b_mesh[mb + mat * MAT_ROWS];\n"),
    ("    let m1 = b_mesh[mb + mat * MAT_ROWS + 1u];\n", "    var m1 = b_mesh[mb + mat * MAT_ROWS + 1u];\n"),
    ("    let m2 = b_mesh[mb + mat * MAT_ROWS + 2u];\n", "    var m2 = b_mesh[mb + mat * MAT_ROWS + 2u];\n"),
    (
        "    let det = surface_detail(mb + mat * MAT_ROWS, pos, nps, uvs, b, n, m0.xyz);\n",
        "    var det = surface_detail(mb + mat * MAT_ROWS, pos, nps, uvs, b, n, m0.xyz);\n    apply_slang_surface(mb + mat * MAT_ROWS, lb, p, v, uvs, b, &det, &m0, &m1, &m2);\n",
    ),
];

/// Calls the project's function on the material's values (mirrors
/// `mesh3d::slang_surface`); `DISPATCH` is the library's dispatch.
const GLUE: &str = r#"
// A Slang surface (surfaces.rs; mirrors slang_surface in mesh3d.rs): the
// material's values at p through the project's function, inputs in object
// space, the normal back in camera space.
fn apply_slang_surface(row: u32, lb: u32, p: vec3<f32>, v: vec3<f32>, uvs_in: array<vec2<f32>, 3>, b: vec3<f32>, det: ptr<function, Detail>, m0: ptr<function, vec4<f32>>, m1: ptr<function, vec4<f32>>, m2: ptr<function, vec4<f32>>) {
    if (u32((*m2).w) & 128u) == 0u { return; }
    let r18 = b_mesh[row + 18u];
    let r21 = b_mesh[row + 21u];
    let r22 = b_mesh[row + 22u];
    let r23 = b_mesh[row + 23u];
    let r24 = b_mesh[row + 24u];
    let n = (*det).n;
    let po = vec3<f32>(dot(r21.xyz, p) + r21.w, dot(r22.xyz, p) + r22.w, dot(r23.xyz, p) + r23.w);
    let no = normalize(vec3<f32>(dot(b_mesh[row + 31u].xyz, n), dot(b_mesh[row + 32u].xyz, n), dot(b_mesh[row + 33u].xyz, n)));
    let vo = normalize(vec3<f32>(dot(r21.xyz, v), dot(r22.xyz, v), dot(r23.xyz, v)));
    let world = to_world(lb, p) + r24.xyz;
    let wn = to_world(lb, n);
    // UVs across a wrapping seam joined, as surface_detail does.
    var uvs = uvs_in;
    let wraps = u32(b_mesh[row + 16u].y);
    for (var c = 0u; c < 2u; c++) {
        if (wraps & (1u << c)) == 0u { continue; }
        for (var k = 1u; k < 3u; k++) {
            if uvs[k][c] - uvs[0][c] > 0.5 { uvs[k][c] -= 1.0; }
            if uvs[k][c] - uvs[0][c] < -0.5 { uvs[k][c] += 1.0; }
        }
    }
    let uv = uvs[0] * b.x + uvs[1] * b.y + uvs[2] * b.z;
    let metal = (*m1).x * (*det).metal_mul;
    let rough = clamp((*m1).y * (*det).rough_mul + (*det).rough, 0.03, 1.0);
    var o0: vec4<f32>;
    var o1: vec4<f32>;
    var o2: vec4<f32>;
    DISPATCH(u32(r18.x), vec4<f32>(po, r24.w), vec4<f32>(world, 0.0), vec4<f32>(no, 0.0), vec4<f32>(vo, 0.0), vec4<f32>(wn, 0.0), vec4<f32>(uv, metal, 0.0), b_mesh[row + 19u], b_mesh[row + 20u], vec4<f32>((*det).base, rough), vec4<f32>((*m2).xyz * (*det).emit, (*m0).w), &o0, &o1, &o2);
    (*det).base = max(o0.xyz, vec3<f32>(0.0));
    (*m1).y = o0.w;
    (*det).rough_mul = 1.0;
    (*det).rough = 0.0;
    (*m1).x = clamp(o2.w, 0.0, 1.0);
    (*det).metal_mul = 1.0;
    (*m2) = vec4<f32>(max(o1.xyz, vec3<f32>(0.0)), (*m2).w);
    (*det).emit = vec3<f32>(1.0);
    (*m0).w = clamp(o1.w, 0.0, 1.0);
    let nc = r21.xyz * o2.x + r22.xyz * o2.y + r23.xyz * o2.z;
    if dot(nc, nc) > 1e-20 { (*det).n = normalize(nc); }
}
"#;

/// The raster shader with `program`'s functions: motion.wgsl with the
/// hooks in `shade_tri`, the functions and the glue. Errors name what
/// failed (slangc's, or the WGSL's own).
pub(crate) fn raster_source(program: &Program) -> Result<String, String> {
    splice(crate::gpu::SHADER, "motion.wgsl's shade_tri", &HOOKS, GLUE, program)
}

/// Lines of `path` and `surface` in pt.wgsl the variant changes: a hit's
/// material rows become variables the Slang surface writes (before the
/// opacity test, which reads its opacity), and `surface` and an unlit hit
/// take its detail. Each starts on a new line (indents differ).
const TRACE_HOOKS: [(&str, &str); 7] = [
    ("\n        let m0 = b_mat[h.mat];\n", "\n        var m0 = b_mat[h.mat];\n"),
    ("\n        let m1 = b_mat[h.mat + 1u];\n", "\n        var m1 = b_mat[h.mat + 1u];\n"),
    ("\n        let m2 = b_mat[h.mat + 2u];\n", "\n        var m2 = b_mat[h.mat + 2u];\n"),
    ("\n        if step == guide { out.albedo = m0.xyz; }\n", "\n        apply_slang_surface(h, v, &m0, &m1, &m2);\n        if step == guide { out.albedo = m0.xyz; }\n"),
    ("\n            let det = detail_at(h, h.n, m0.xyz);\n", "\n            let det = slang_detail(h, m0.xyz);\n"),
    ("\n    let m1 = b_mat[h.mat + 1u];\n", "\n    let m1 = slang_row1(h);\n"),
    ("\n    let det = detail_at(h, h.n, m0.xyz);\n", "\n    let det = slang_detail(h, m0.xyz);\n"),
];

/// The tracer's call of the project's function at a hit (mirrors `GLUE`),
/// kept for that hit's `surface` or unlit colour.
const TRACE_GLUE: &str = r#"
// The path step's Slang surface (apply_slang_surface): its detail and
// material row 1 with the function's roughness and metallic.
var<private> slang_hit: bool;
var<private> slang_det: Detail;
var<private> slang_m1: vec4<f32>;

fn slang_detail(h: Hit, base: vec3<f32>) -> Detail {
    if slang_hit { return slang_det; }
    return detail_at(h, h.n, base);
}

fn slang_row1(h: Hit) -> vec4<f32> {
    if slang_hit { return slang_m1; }
    return b_mat[h.mat + 1u];
}

// A Slang surface at a hit (surfaces.rs; as the raster's): the material's
// values with their detail through the project's function, inputs in
// object space, the normal back in camera space.
fn apply_slang_surface(h: Hit, v: vec3<f32>, m0: ptr<function, vec4<f32>>, m1: ptr<function, vec4<f32>>, m2: ptr<function, vec4<f32>>) {
    slang_hit = false;
    if (u32((*m2).w) & 128u) == 0u { return; }
    let row = h.mat;
    var det = detail_at(h, h.n, (*m0).xyz);
    let r18 = b_mat[row + 18u];
    let r21 = b_mat[row + 21u];
    let r22 = b_mat[row + 22u];
    let r23 = b_mat[row + 23u];
    let r24 = b_mat[row + 24u];
    let p = h.p;
    let n = det.n;
    let po = vec3<f32>(dot(r21.xyz, p) + r21.w, dot(r22.xyz, p) + r22.w, dot(r23.xyz, p) + r23.w);
    let no = normalize(vec3<f32>(dot(b_mat[row + 31u].xyz, n), dot(b_mat[row + 32u].xyz, n), dot(b_mat[row + 33u].xyz, n)));
    let vo = normalize(vec3<f32>(dot(r21.xyz, v), dot(r22.xyz, v), dot(r23.xyz, v)));
    let world = to_world(p) + r24.xyz;
    let wn = to_world(n);
    // UVs across a wrapping seam joined, as surface_detail does.
    let vs = array<u32, 3>(h.v0, h.v1, h.v2);
    var uvs: array<vec2<f32>, 3>;
    for (var k = 0u; k < 3u; k++) {
        uvs[k] = vec2<f32>(b_attr[vs[k] * 2u].w, b_attr[vs[k] * 2u + 1u].w);
    }
    let wraps = u32(b_mat[row + 16u].y);
    for (var c = 0u; c < 2u; c++) {
        if (wraps & (1u << c)) == 0u { continue; }
        for (var k = 1u; k < 3u; k++) {
            if uvs[k][c] - uvs[0][c] > 0.5 { uvs[k][c] -= 1.0; }
            if uvs[k][c] - uvs[0][c] < -0.5 { uvs[k][c] += 1.0; }
        }
    }
    let b = h.bary;
    let uv = uvs[0] * b.x + uvs[1] * b.y + uvs[2] * b.z;
    let metal = (*m1).x * det.metal_mul;
    let rough = clamp((*m1).y * det.rough_mul + det.rough, 0.03, 1.0);
    var o0: vec4<f32>;
    var o1: vec4<f32>;
    var o2: vec4<f32>;
    DISPATCH(u32(r18.x), vec4<f32>(po, r24.w), vec4<f32>(world, 0.0), vec4<f32>(no, 0.0), vec4<f32>(vo, 0.0), vec4<f32>(wn, 0.0), vec4<f32>(uv, metal, 0.0), b_mat[row + 19u], b_mat[row + 20u], vec4<f32>(det.base, rough), vec4<f32>((*m2).xyz * det.emit, (*m0).w), &o0, &o1, &o2);
    det.base = max(o0.xyz, vec3<f32>(0.0));
    (*m1).y = o0.w;
    det.rough_mul = 1.0;
    det.rough = 0.0;
    (*m1).x = clamp(o2.w, 0.0, 1.0);
    det.metal_mul = 1.0;
    (*m2) = vec4<f32>(max(o1.xyz, vec3<f32>(0.0)), (*m2).w);
    det.emit = vec3<f32>(1.0);
    (*m0).w = clamp(o1.w, 0.0, 1.0);
    let nc = r21.xyz * o2.x + r22.xyz * o2.y + r23.xyz * o2.z;
    if dot(nc, nc) > 1e-20 { det.n = normalize(nc); }
    slang_det = det;
    slang_m1 = *m1;
    slang_hit = true;
}
"#;

/// The path tracer's shader `base` (pt.wgsl, specialised) with
/// `program`'s functions: the hooks in `path` and `surface`, the functions
/// and the glue.
pub(crate) fn trace_source(base: &str, program: &Program) -> Result<String, String> {
    splice(base, "pt.wgsl's path or surface", &TRACE_HOOKS, TRACE_GLUE, program)
}

/// `base` with `hooks` applied, then `program`'s functions and `glue`
/// (`DISPATCH` naming the library's dispatch), checked by naga.
fn splice(base: &str, hooked: &str, hooks: &[(&str, &str)], glue: &str, program: &Program) -> Result<String, String> {
    let lib = program.wgsl()?;
    let mut source = base.to_string();
    for (from, to) in hooks {
        if source.matches(from).count() != 1 {
            return Err(format!("Slang materials: {hooked} changed ({})", from.trim()));
        }
        source = source.replacen(from, to, 1);
    }
    source.push_str("\n// ---- Slang surface functions (the project's shaders/) ----\n");
    source.push_str(&lib.wgsl);
    source.push_str(&glue.replace("DISPATCH", &lib.dispatch));
    // The WGSL's errors with their lines (the device's own say only that
    // it failed).
    let module = naga::front::wgsl::parse_str(&source).map_err(|e| format!("Slang materials' WGSL: {}", e.emit_to_string(&source)))?;
    let flags = naga::valid::ValidationFlags::all() ^ naga::valid::ValidationFlags::BINDINGS;
    naga::valid::Validator::new(flags, naga::valid::Capabilities::all()).validate(&module).map_err(|e| format!("Slang materials' WGSL: {}", e.emit_to_string(&source)))?;
    Ok(source)
}
