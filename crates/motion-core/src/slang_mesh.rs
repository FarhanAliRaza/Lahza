//! Geometry from the project's Slang: meshes an author's functions
//! generate (`{"type": "slang"}` mesh layers) and deformers that move the
//! points of any mesh (a mesh layer's `deform`). See
//! `docs/slang/geometry.md` for the contract.
//!
//! Lahza exports one C function per use (`lahza_mesh_…`) that loops over
//! points and triangles in native code; the CPU renderer, the GPU's mesh
//! pool and the path tracer all take the mesh from that one generation.
//! A mesh depends only on its parameters (the library's build, counts,
//! param values and, when animated, the time), so it is cached by them
//! like every other mesh.

use crate::mesh::{self, Mesh, PART_FRONT, TWO_SIDED};
use crate::slang::{valid_name, Linked, Linker};
use std::sync::{Arc, OnceLock};

/// Most points and triangles one generated mesh may have (the GPU's mesh
/// pool holds 256 MB: this is half of it).
pub const MAX_VERTICES: u32 = 2_000_000;
pub const MAX_TRIANGLES: u32 = 4_000_000;
/// Most times a deformed mesh is subdivided, and the triangles that stops
/// at.
pub const MAX_SUBDIVIDE: u32 = 4;
const MAX_DEFORM_TRIS: usize = 500_000;

/// What a generator's author wrote besides `_position` and `_triangle`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Outputs {
    pub normals: bool,
    pub uvs: bool,
    pub segments: bool,
}

/// A mesh made by the author's `<function>_*` functions (exported by
/// `generator`).
#[derive(Clone, Debug, PartialEq)]
pub struct SlangShape {
    pub function: String,
    pub call: Linked,
    pub vertices: u32,
    pub triangles: u32,
    /// Values of the `<function>_params` fields, in name order.
    pub params: Vec<f32>,
    /// Local seconds the functions see (0 unless animated).
    pub time: f32,
    pub animated: bool,
    pub outputs: Outputs,
    pub two_sided: bool,
    pub bounds: Option<[[f64; 3]; 2]>,
    /// The mesh, generated once for these parameters: the bounds and the
    /// renderer share it.
    pub generated: Generated,
}

/// A deformer bound to its function (see `deformer`).
#[derive(Clone, Debug, PartialEq)]
pub struct Deform {
    pub function: String,
    pub call: Linked,
    pub params: Vec<f32>,
    pub time: f32,
    pub animated: bool,
    pub subdivide: u32,
    /// px the function may move a point (pads the bounds).
    pub reach: f64,
}

/// A generated mesh kept with its parameters. Not part of their identity:
/// equal parameters are equal whether or not one has generated yet.
#[derive(Clone, Default)]
pub struct Generated(Arc<OnceLock<Result<Mesh, String>>>);

impl PartialEq for Generated {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for Generated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", if self.0.get().is_some() { "generated" } else { "not generated" })
    }
}

/// A short stable hash (symbols and keys must not change between runs).
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Code filling `<function>_params p` from `params` by field name, and
/// the argument that passes it: where the author's modules declare the
/// struct (`takes`), whether or not the layer gives params (fields it does
/// not give keep the struct's defaults); nothing where they do not.
fn params_code(function: &str, names: &[String], takes: bool) -> (String, &'static str) {
    if !takes {
        return (String::new(), "");
    }
    let mut code = format!("    {function}_params p = {{}};\n");
    for (k, n) in names.iter().enumerate() {
        code.push_str(&format!("    p.{n} = params[{k}];\n"));
    }
    (code, ", p")
}

/// Reports names that cannot be pasted into Slang (the link fails on them).
fn names_ok(linker: &Linker, what: &str, function: &str, names: &[String]) -> bool {
    let mut ok = true;
    for n in std::iter::once(function).chain(names.iter().map(String::as_str)) {
        if !valid_name(n) {
            linker.error(format!("{what}: '{n}' is not a Slang name"));
            ok = false;
        }
    }
    ok
}

/// `(first vertex, end, first triangle, end, t, params, positions,
/// normals, uvs, triangles, segments)`, as `generator` exports it.
type GenerateFn = unsafe extern "C" fn(u32, u32, u32, u32, f32, *const f32, *mut f32, *mut f32, *mut f32, *mut u32, *mut u32);
/// `(first point, end, t, params, positions (in place), normals)`, as
/// `deformer` exports it.
type DeformFn = unsafe extern "C" fn(u32, u32, f32, *const f32, *mut f32, *const f32);

/// Whether `function` takes a `<function>_params` struct (the modules
/// declare one), reporting params given to a function that has none: the
/// compiler would say so about Lahza's generated call, not the layer.
fn takes_params(linker: &Linker, shaders: &[crate::slang::Shader], what: &str, function: &str, names: &[String]) -> bool {
    let takes = crate::slang::declares_struct(shaders, &format!("{function}_params"));
    if !takes && !names.is_empty() && !shaders.is_empty() {
        linker.error(format!("{what}: params ({}) are given, but shaders/ has no `struct {function}_params` for them (the function takes it as its last argument)", names.join(", ")));
    }
    takes
}

/// Exports a generator: one function writing every output of a range of
/// points and triangles. `names`: the params, in the order passed.
/// `shaders`: the project's modules (which functions and structs exist).
pub fn generator(linker: &Linker, shaders: &[crate::slang::Shader], what: &str, function: &str, names: &[String], out: Outputs) -> Linked {
    names_ok(linker, what, function, names);
    let takes = takes_params(linker, shaders, what, function, names);
    for (wanted, suffix) in [(true, "position"), (true, "triangle"), (out.normals, "normal"), (out.uvs, "uv"), (out.segments, "segment")] {
        if wanted {
            linker.want(&format!("{function}_{suffix}"), what);
        }
    }
    let shape = format!("{names:?}{out:?}{}", if takes { "" } else { "plain" });
    let symbol = format!("lahza_mesh_{function}_{:016x}", fnv(&shape));
    linker.export(&symbol, || {
        let (fill, p) = params_code(function, names, takes);
        let f = function;
        let mut code = format!(
            "export __extern_cpp void {symbol}(uint v0, uint v1, uint t0, uint t1, float t, float* params, float* pos, float* nor, float* uv, uint* tri, uint* seg) {{\n{fill}    for (uint i = v0; i < v1; i++) {{\n        float3 v = {f}_position(i, t{p});\n        pos[i * 3] = v.x; pos[i * 3 + 1] = v.y; pos[i * 3 + 2] = v.z;\n"
        );
        if out.normals {
            code.push_str(&format!("        float3 n = {f}_normal(i, t{p});\n        nor[i * 3] = n.x; nor[i * 3 + 1] = n.y; nor[i * 3 + 2] = n.z;\n"));
        }
        if out.uvs {
            code.push_str(&format!("        float2 w = {f}_uv(i, t{p});\n        uv[i * 2] = w.x; uv[i * 2 + 1] = w.y;\n"));
        }
        code.push_str(&format!("    }}\n    for (uint j = t0; j < t1; j++) {{\n        uint3 k = {f}_triangle(j, t{p});\n        tri[j * 3] = k.x; tri[j * 3 + 1] = k.y; tri[j * 3 + 2] = k.z;\n"));
        if out.segments {
            code.push_str(&format!("        seg[j] = {f}_segment(j, t{p});\n"));
        }
        code.push_str("    }\n}\n");
        code
    })
}

/// Exports a deformer: `function(p, n, t[, params])` over a range of points.
pub fn deformer(linker: &Linker, shaders: &[crate::slang::Shader], what: &str, function: &str, names: &[String]) -> Linked {
    names_ok(linker, what, function, names);
    let takes = takes_params(linker, shaders, what, function, names);
    linker.want(function, what);
    let symbol = format!("lahza_mesh_deform_{function}_{:016x}", fnv(&format!("{names:?}{}", if takes { "" } else { "plain" })));
    linker.export(&symbol, || {
        let (fill, p) = params_code(function, names, takes);
        format!(
            "export __extern_cpp void {symbol}(uint i0, uint i1, float t, float* params, float* pos, float* nor) {{\n{fill}    for (uint i = i0; i < i1; i++) {{\n        float3 q = {function}(float3(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]), float3(nor[i * 3], nor[i * 3 + 1], nor[i * 3 + 2]), t{p});\n        pos[i * 3] = q.x; pos[i * 3 + 1] = q.y; pos[i * 3 + 2] = q.z;\n    }}\n}}\n"
        )
    })
}

/// Splits `n` items over the threads worth using: (first, end) each.
fn chunks(n: usize, work: usize) -> Vec<(u32, u32)> {
    let threads = if work < 32_768 { 1 } else { std::thread::available_parallelism().map_or(1, |t| t.get()).min(16) };
    let size = n.div_ceil(threads).max(1);
    (0..threads).map(|k| ((k * size).min(n) as u32, ((k + 1) * size).min(n) as u32)).collect()
}

/// Raw pointers handed to the worker threads (each writes its own range).
#[derive(Clone, Copy)]
struct Ptr<T>(*mut T);
// SAFETY: the threads write disjoint ranges and are joined before the
// buffers are read or dropped.
unsafe impl<T> Send for Ptr<T> {}
unsafe impl<T> Sync for Ptr<T> {}

impl<T> Ptr<T> {
    fn of(v: &mut [T]) -> Self {
        Ptr(if v.is_empty() { std::ptr::null_mut() } else { v.as_mut_ptr() })
    }
}

/// The first point of `values` (in `width`s) that is not a number.
fn not_finite(values: &[f32], width: usize) -> Option<(usize, &[f32])> {
    values.chunks(width).enumerate().find(|(_, v)| v.iter().any(|x| !x.is_finite()))
}

impl SlangShape {
    /// The mesh (generated the first time it is asked for).
    pub fn mesh(&self) -> Result<&Mesh, String> {
        self.generated.0.get_or_init(|| self.generate()).as_ref().map_err(Clone::clone)
    }

    /// Local bounds [min, max]: as given, else measured.
    pub fn local_bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        if let Some([lo, hi]) = self.bounds {
            return Some((lo, hi));
        }
        let m = self.mesh().ok()?;
        if m.positions.is_empty() {
            return None;
        }
        let (lo, hi) = m.bounds();
        Some((lo.map(f64::from), hi.map(f64::from)))
    }

    fn generate(&self) -> Result<Mesh, String> {
        let f = &self.function;
        let address = self.call.address().ok_or_else(|| format!("slang mesh '{f}': the project's Slang did not build"))?;
        let (nv, nt) = (self.vertices as usize, self.triangles as usize);
        if nv == 0 || nt == 0 {
            // Nothing yet (counts that grow over time start here).
            return Ok(Mesh::default());
        }
        let mut pos = vec![0f32; nv * 3];
        let mut nor = vec![0f32; if self.outputs.normals { nv * 3 } else { 0 }];
        let mut uv = vec![0f32; if self.outputs.uvs { nv * 2 } else { 0 }];
        let mut tri = vec![0u32; nt * 3];
        let mut seg = vec![0u32; if self.outputs.segments { nt } else { 0 }];
        // SAFETY: `generator` exported this symbol with exactly this
        // signature, and the runtime keeps its library loaded.
        let call: GenerateFn = unsafe { std::mem::transmute(address) };
        let (p, n, u, k, s) = (Ptr::of(&mut pos), Ptr::of(&mut nor), Ptr::of(&mut uv), Ptr::of(&mut tri), Ptr::of(&mut seg));
        let (time, params) = (self.time, Ptr(self.params.as_ptr() as *mut f32));
        let (vs, ts) = (chunks(nv, nv + nt), chunks(nt, nv + nt));
        std::thread::scope(|scope| {
            for (v, t) in vs.into_iter().zip(ts) {
                scope.spawn(move || {
                    let (p, n, u, k, s, params) = (p, n, u, k, s, params);
                    // SAFETY: each thread writes only its own ranges of the
                    // buffers, sized for every point and triangle above.
                    unsafe { call(v.0, v.1, t.0, t.1, time, params.0, p.0, n.0, u.0, k.0, s.0) };
                });
            }
        });
        if let Some((i, v)) = not_finite(&pos, 3) {
            return Err(format!("slang mesh '{f}': {f}_position({i}) is not a number: {v:?}"));
        }
        if let Some((i, v)) = not_finite(&nor, 3) {
            return Err(format!("slang mesh '{f}': {f}_normal({i}) is not a number: {v:?}"));
        }
        if let Some((i, v)) = not_finite(&uv, 2) {
            return Err(format!("slang mesh '{f}': {f}_uv({i}) is not a number: {v:?}"));
        }
        if let Some((j, t)) = tri.chunks(3).enumerate().find(|(_, t)| t.iter().any(|&i| i as usize >= nv)) {
            return Err(format!("slang mesh '{f}': {f}_triangle({j}) is {t:?}, past the {nv} vertices"));
        }
        let mut mesh = Mesh {
            positions: pos.chunks(3).map(|v| [v[0], v[1], v[2]]).collect(),
            normals: if self.outputs.normals {
                nor.chunks(3)
                    .map(|v| {
                        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                        if l > 1e-12 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0; 3] }
                    })
                    .collect()
            } else {
                vec![[0.0; 3]; nv]
            },
            tris: tri.chunks(3).map(|t| [t[0], t[1], t[2]]).collect(),
            tags: (0..nt).map(|j| mesh::tag(PART_FRONT, seg.get(j).copied().unwrap_or(0)) | if self.two_sided { TWO_SIDED } else { 0 }).collect(),
            edges: vec![7; nt],
            uvs: uv.chunks(2).map(|w| [w[0], w[1]]).collect(),
        };
        if !self.outputs.normals {
            mesh::face_normals(&mut mesh);
        }
        Ok(mesh)
    }
}

/// Moves every point of `mesh` by the deformer (subdividing first as it
/// asks), then shades it from its new faces. Copies of one point (hard
/// edges, UV seams) get the same normal, the average of theirs, so they
/// move together and nothing cracks.
pub fn deform(mesh: &mut Mesh, d: &Deform) -> Result<(), String> {
    let f = &d.function;
    let address = d.call.address().ok_or_else(|| format!("deform '{f}': the project's Slang did not build"))?;
    if mesh.uvs.len() != mesh.positions.len() {
        mesh.uvs.resize(mesh.positions.len(), [0.0; 2]);
    }
    for _ in 0..d.subdivide.min(MAX_SUBDIVIDE) {
        if mesh.tris.len() * 4 > MAX_DEFORM_TRIS {
            break;
        }
        mesh::subdivide(mesh);
    }
    let key = |p: [f32; 3]| [(p[0] * 64.0).round() as i64, (p[1] * 64.0).round() as i64, (p[2] * 64.0).round() as i64];
    let mut welded: std::collections::HashMap<[i64; 3], [f32; 3]> = std::collections::HashMap::new();
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let w = welded.entry(key(*p)).or_insert([0.0; 3]);
        *w = [w[0] + n[0], w[1] + n[1], w[2] + n[2]];
    }
    let mut nor: Vec<f32> = Vec::with_capacity(mesh.positions.len() * 3);
    for p in &mesh.positions {
        let w = welded[&key(*p)];
        let l = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
        nor.extend(if l > 1e-12 { [w[0] / l, w[1] / l, w[2] / l] } else { [0.0; 3] });
    }
    let mut pos: Vec<f32> = mesh.positions.iter().flatten().copied().collect();
    let n = mesh.positions.len();
    // SAFETY: `deformer` exported this symbol with exactly this signature,
    // and the runtime keeps its library loaded.
    let call: DeformFn = unsafe { std::mem::transmute(address) };
    let (p, q, time, params) = (Ptr::of(&mut pos), Ptr(nor.as_ptr() as *mut f32), d.time, Ptr(d.params.as_ptr() as *mut f32));
    std::thread::scope(|scope| {
        for (i0, i1) in chunks(n, n) {
            scope.spawn(move || {
                let (p, q, params) = (p, q, params);
                // SAFETY: each thread moves only its own range of points.
                unsafe { call(i0, i1, time, params.0, p.0, q.0) };
            });
        }
    });
    if let Some((i, v)) = not_finite(&pos, 3) {
        return Err(format!("deform '{f}': point {i} moved to a value that is not a number: {v:?}"));
    }
    mesh.positions = pos.chunks(3).map(|v| [v[0], v[1], v[2]]).collect();
    mesh::face_normals(mesh);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::compile::{Compiled, Content, MeshState};
    use crate::document::Project;
    use crate::time::Fps;
    use serde_json::json;

    const SHADER: &str = "
struct quad_params { float size = 10; }
float3 quad_position(uint i, float t, quad_params p) { return float3(float(i & 1u) * p.size + t, float(i >> 1) * p.size, 0); }
uint3 quad_triangle(uint j, float t, quad_params p) { return j == 0 ? uint3(0, 1, 2) : uint3(1, 3, 2); }
float3 bad_position(uint i, float t) { return float3(sqrt(-1.0 - t), 0, 0); }
uint3 bad_triangle(uint j, float t) { return uint3(0, 1, 2); }
float3 lift(float3 p, float3 n, float t) { return p + float3(0, -5, 0) * (1 + t); }
";

    /// None without slangc (nothing to test against).
    fn compiled(layers: serde_json::Value) -> Option<Compiled> {
        crate::slang::compiler().ok()?;
        let mut p = Project::new(640, 360, Fps::whole(30), 2.0);
        p.shaders.push(crate::slang::Shader { name: "geo".into(), source: SHADER.into() });
        p.scenes.push(serde_json::from_value(json!({"id": "s1", "layers": layers})).unwrap());
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        Some(c)
    }

    fn mesh(c: &Compiled, t: f64, id: &str) -> MeshState {
        c.frame(t)
            .layers
            .iter()
            .find_map(|l| match &l.content {
                Content::Mesh(m) if &*l.id == id => Some((**m).clone()),
                _ => None,
            })
            .unwrap()
    }

    /// A generator whose modules declare `<function>_params` is passed
    /// the struct whether or not the layer gives params (its defaults
    /// then stand); one without is not, and params given to it are an
    /// error that names the layer, not the generated call. A function no
    /// module has is named with the files. (The generated Slang's text:
    /// no compiler needed.)
    #[test]
    fn generated_calls_follow_what_the_modules_declare() {
        use crate::slang::{Linker, Shader};
        let modules = vec![Shader { name: "geo".into(), source: SHADER.into() }];
        let linker = Linker::default();
        super::generator(&linker, &modules, "wall.mesh", "quad", &[], Default::default());
        let code = linker.exported("lahza_mesh_quad_").unwrap();
        assert!(code.contains("    quad_params p = {};\n    for (uint i = v0; i < v1; i++) {\n        float3 v = quad_position(i, t, p);"), "{code}");
        assert!(code.contains("uint3 k = quad_triangle(j, t, p);"), "{code}");
        assert!(linker.errors().is_empty(), "{:?}", linker.errors());
        // With a param: filled by name.
        let linker = Linker::default();
        super::generator(&linker, &modules, "wall.mesh", "quad", &["size".into()], Default::default());
        assert!(linker.exported("lahza_mesh_quad_").unwrap().contains("    quad_params p = {};\n    p.size = params[0];\n"));
        // `bad` has no struct: called without, and params for it are an error.
        let linker = Linker::default();
        super::generator(&linker, &modules, "blob.mesh", "bad", &[], Default::default());
        assert!(linker.exported("lahza_mesh_bad_").unwrap().contains("float3 v = bad_position(i, t);"));
        assert!(linker.errors().is_empty());
        super::generator(&linker, &modules, "blob.mesh", "bad", &["size".into()], Default::default());
        assert_eq!(linker.errors(), ["blob.mesh: params (size) are given, but shaders/ has no `struct bad_params` for them (the function takes it as its last argument)"]);
        // Functions the modules lack: by name, with what calls them.
        let linker = Linker::default();
        super::generator(&linker, &modules, "ring.mesh", "ring", &[], super::Outputs { normals: true, ..Default::default() });
        super::deformer(&linker, &modules, "ring.deform", "lift", &[]);
        let e = linker.link(&modules).err().unwrap();
        assert_eq!(e, "no Slang function 'ring_position' in shaders/geo.slang (ring.mesh calls it); no Slang function 'ring_triangle' in shaders/geo.slang (ring.mesh calls it); no Slang function 'ring_normal' in shaders/geo.slang (ring.mesh calls it)");
    }

    /// `vertices` and `triangles` can change over time: a mesh that grows
    /// is asked for the points and triangles it has so far (none yet is
    /// an empty mesh, not an error).
    #[test]
    fn counts_follow_their_keys() {
        let mut p = Project::new(640, 360, Fps::whole(30), 2.0);
        p.shaders.push(crate::slang::Shader { name: "geo".into(), source: SHADER.into() });
        p.scenes.push(serde_json::from_value(json!({"id": "s1", "layers": [
            {"id": "grow", "kind": "mesh", "mesh": {"type": "slang", "function": "quad",
                "vertices": {"keys": [{"at": 0, "v": 0, "ease": "linear"}, {"at": 1, "v": 400}]},
                "triangles": {"keys": [{"at": 0, "v": 0, "ease": "linear"}, {"at": 1, "v": 200}]}}},
            {"id": "still", "kind": "mesh", "mesh": {"type": "slang", "function": "quad", "vertices": 4, "triangles": 2}}
        ]})).unwrap());
        // (Without slangc the project reports that it did not build; the
        // counts are the layers' own.)
        let c = Compiled::new(&p);
        let counts = |t: f64, id: &str| match &mesh(&c, t, id).params.shape {
            crate::mesh::MeshShape::Slang(s) => (s.vertices, s.triangles),
            other => panic!("{other:?}"),
        };
        assert_eq!(counts(0.0, "grow"), (0, 0));
        assert_eq!(counts(0.5, "grow"), (200, 100));
        assert_eq!(counts(1.5, "grow"), (400, 200));
        assert_eq!(counts(0.5, "still"), (4, 2));
        assert_ne!(mesh(&c, 0.25, "grow").params.key(), mesh(&c, 0.5, "grow").params.key(), "another mesh as it grows");
        // A constant count out of range is still an error.
        let mut bad = Project::new(640, 360, Fps::whole(30), 2.0);
        bad.scenes.push(serde_json::from_value(json!({"id": "s1", "layers": [{"id": "none", "kind": "mesh", "mesh": {"type": "slang", "function": "quad", "vertices": 0, "triangles": 2}}]})).unwrap());
        assert!(Compiled::new(&bad).errors.iter().any(|e| e.contains("none.mesh.vertices: 0 must be 1 to")), "{:?}", Compiled::new(&bad).errors);
    }

    /// A static generator is one mesh for the whole layer (one cache key);
    /// an animated one is new every frame, and its points follow `t`.
    #[test]
    fn generated_meshes_change_only_when_animated() {
        let Some(c) = compiled(json!([
            {"id": "still", "kind": "mesh", "mesh": {"type": "slang", "function": "quad", "vertices": 4, "triangles": 2, "params": {"size": 20}}},
            {"id": "moving", "kind": "mesh", "mesh": {"type": "slang", "function": "quad", "vertices": 4, "triangles": 2, "animated": true}},
            {"id": "lifted", "kind": "mesh", "mesh": {"type": "plane", "size": [10, 10]}, "deform": {"function": "lift"}}]))
        else {
            return;
        };
        let (a, b) = (mesh(&c, 0.0, "still"), mesh(&c, 1.0, "still"));
        assert_eq!(a.params.key(), b.params.key());
        assert!(!a.params.animated());
        let m = crate::mesh::generate(&a.params).unwrap();
        assert_eq!(m.tris, vec![[0, 1, 2], [1, 3, 2]]);
        assert_eq!(m.positions[3], [20.0, 20.0, 0.0]);
        assert_eq!(a.params.local_bounds(), Some(([0.0; 3], [20.0, 20.0, 0.0])));
        let (a, b) = (mesh(&c, 0.0, "moving"), mesh(&c, 1.0, "moving"));
        assert_ne!(a.params.key(), b.params.key());
        assert!(b.params.animated());
        assert_eq!(crate::mesh::generate(&b.params).unwrap().positions[0], [1.0, 0.0, 0.0]);
        let l = mesh(&c, 0.0, "lifted");
        let mut m = crate::mesh::generate(&l.params).unwrap();
        let before = m.positions.clone();
        super::deform(&mut m, l.params.deform.as_ref().unwrap()).unwrap();
        assert!(m.positions.iter().zip(&before).all(|(p, q)| p[1] == q[1] - 5.0));
    }

    /// Points that are not numbers fail the mesh, naming the function.
    #[test]
    fn not_a_number_is_an_error() {
        let Some(c) = compiled(json!([{"id": "bad", "kind": "mesh", "mesh": {"type": "slang", "function": "bad", "vertices": 3, "triangles": 1}}])) else { return };
        let e = crate::mesh::generate(&mesh(&c, 0.0, "bad").params).unwrap_err();
        assert!(e.contains("bad_position(0) is not a number"), "{e}");
    }
}
