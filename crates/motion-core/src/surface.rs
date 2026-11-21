//! Slang materials: a material names a Slang function (`"slang":
//! {"function": "marble", "params": [...]}`) that turns what the material
//! shows at a point (its colour, roughness, metallic, emission, opacity and
//! normal) into what Lahza then lights. The contract is the built-in module
//! `lahza` (`MODULE`, imported as `import lahza;`); docs/slang/materials.md
//! has it with examples.
//!
//! The CPU renderer calls each function through an export of the project's
//! library (`lahza_material_<function>`); the GPU renderer compiles the
//! project's functions with a dispatch on the material's id to WGSL
//! (`Program::wgsl`) and splices it into a variant of its raster shader.

use crate::slang::{Linked, Linker, Shader};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

/// The built-in module every project can import (`import lahza;`). Packed
/// inputs and outputs (`lahza_surface_input`, `lahza_surface_output`) are
/// the layout the renderers fill and read: keep them in step with
/// `motion-render/src/surfaces.rs`.
pub const MODULE: &str = r#"// Lahza's surface interface (docs/slang/materials.md).
module lahza;

// What a surface shows at a point; Lahza lights it (GGX, shadows,
// environment).
public struct Surface {
    // Base colour, linear.
    public float3 color;
    // 0 mirror-sharp … 1 fully diffuse.
    public float roughness;
    // 0 dielectric … 1 metal.
    public float metallic;
    // Light given off, linear (added to the lit colour).
    public float3 emissive;
    // 0 … 1 (see-through meshes need "see_through": true).
    public float opacity;
    // Shading normal, object space, unit.
    public float3 normal;
};

// A point of the surface.
public struct SurfaceInput {
    // Object space, px: sticks to the mesh as it moves.
    public float3 position;
    // World space, px.
    public float3 world;
    // Shading normal, object space, unit, facing the camera.
    public float3 normal;
    // Toward the camera, object space, unit.
    public float3 view;
    // The normal in world space (up is -y).
    public float3 world_normal;
    // The mesh's UVs (before the material's "uv" tiling).
    public float2 uv;
    // Seconds since the layer started.
    public float time;
    // The material's "params", in order (0 past the given ones).
    public float params[8];
    // The material's own values here (maps and noise applied): return it
    // changed, or build a new Surface.
    public Surface material;
};

public SurfaceInput lahza_surface_input(float4 a0, float4 a1, float4 a2, float4 a3, float4 a4, float4 a5, float4 a6, float4 a7, float4 a8, float4 a9) {
    SurfaceInput s;
    s.position = a0.xyz;
    s.time = a0.w;
    s.world = a1.xyz;
    s.normal = a2.xyz;
    s.view = a3.xyz;
    s.world_normal = a4.xyz;
    s.uv = a5.xy;
    s.params[0] = a6.x;
    s.params[1] = a6.y;
    s.params[2] = a6.z;
    s.params[3] = a6.w;
    s.params[4] = a7.x;
    s.params[5] = a7.y;
    s.params[6] = a7.z;
    s.params[7] = a7.w;
    s.material.color = a8.xyz;
    s.material.roughness = a8.w;
    s.material.metallic = a5.z;
    s.material.emissive = a9.xyz;
    s.material.opacity = a9.w;
    s.material.normal = a2.xyz;
    return s;
}

public void lahza_surface_output(Surface o, out float4 o0, out float4 o1, out float4 o2) {
    o0 = float4(o.color, o.roughness);
    o1 = float4(o.emissive, o.opacity);
    o2 = float4(o.normal, o.metallic);
}

// 0 … 1 for an integer cell: the same on every renderer.
public float hash(int3 c) {
    uint h = uint(c.x) * 0x8da6b343u ^ uint(c.y) * 0xd8163841u ^ uint(c.z) * 0xcb1ab31fu;
    h ^= h >> 16;
    h *= 0x7feb352du;
    h ^= h >> 15;
    h *= 0x846ca68bu;
    h ^= h >> 16;
    return float(h >> 8) * (1.0 / 16777216.0);
}

// Smooth value noise, 0 … 1, one feature per unit.
public float value_noise(float3 p) {
    float3 f = floor(p);
    int3 c = int3(f);
    float3 t = p - f;
    float3 u = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    float x00 = lerp(hash(c), hash(c + int3(1, 0, 0)), u.x);
    float x10 = lerp(hash(c + int3(0, 1, 0)), hash(c + int3(1, 1, 0)), u.x);
    float x01 = lerp(hash(c + int3(0, 0, 1)), hash(c + int3(1, 0, 1)), u.x);
    float x11 = lerp(hash(c + int3(0, 1, 1)), hash(c + int3(1, 1, 1)), u.x);
    return lerp(lerp(x00, x10, u.y), lerp(x01, x11, u.y), u.z);
}

// Fractal value noise, 0 … 1: `octaves` layers, each twice as fine and
// half as strong.
public float fbm(float3 p, int octaves) {
    float sum = 0.0;
    float amp = 0.5;
    float norm = 0.0;
    for (int i = 0; i < octaves; i++) {
        sum += amp * value_noise(p);
        norm += amp;
        p = p * 2.03 + float3(17.1, 31.7, 7.3);
        amp *= 0.5;
    }
    return sum / max(norm, 1e-6);
}

// The normal of a raised surface: `gradient` is the height's change per
// px along object space (as finite differences of a height function of
// `s.position`), `strength` scales it.
public float3 bump(SurfaceInput s, float3 gradient, float strength) {
    float3 n = s.normal;
    float3 g = gradient - n * dot(gradient, n);
    return normalize(n - g * strength);
}
"#;

/// The built-in module's name (what `import` names; no project module may
/// take it).
pub const MODULE_NAME: &str = "lahza";

/// Params a material passes (`SurfaceInput::params`).
pub const MAX_PARAMS: usize = 8;

/// The project's modules plus the built-in `lahza` one: what every Slang
/// build of a project compiles against.
pub fn modules(shaders: &[Shader]) -> Vec<Shader> {
    let mut all = shaders.to_vec();
    if !all.iter().any(|m| m.name == MODULE_NAME) {
        all.push(Shader { name: MODULE_NAME.into(), source: MODULE.into() });
    }
    all
}

/// The surface functions a project's materials use, gathered while it
/// compiles (`Surfaces::add`) and fixed once it links (`Surfaces::finish`).
#[derive(Default)]
pub struct Surfaces {
    functions: Vec<String>,
    program: Arc<OnceLock<Arc<Program>>>,
}

impl Surfaces {
    /// A material's use of `function`: its id (1-based, the GPU's dispatch)
    /// and its CPU export.
    pub fn add(&mut self, linker: &Linker, function: &str) -> (u32, Linked) {
        if !crate::slang::valid_name(function) {
            linker.error(format!("'{function}' is not a Slang function name"));
        }
        let id = match self.functions.iter().position(|f| f == function) {
            Some(i) => i + 1,
            None => {
                self.functions.push(function.to_string());
                self.functions.len()
            }
        };
        let symbol = format!("lahza_material_{function}");
        let linked = linker.export(&symbol, || {
            format!(
                "export __extern_cpp void {symbol}(float4* i, float4* o) {{ lahza_surface_output({function}(lahza_surface_input(i[0], i[1], i[2], i[3], i[4], i[5], i[6], i[7], i[8], i[9])), o[0], o[1], o[2]); }}\n"
            )
        });
        (id as u32, linked)
    }

    /// Where the program will be (materials keep it).
    pub fn handle(&self) -> Arc<OnceLock<Arc<Program>>> {
        self.program.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Fixes the program once the project linked: `shaders` with the
    /// built-in module (`modules`). A project module named `lahza` is an
    /// error.
    pub fn finish(&self, project: &[Shader]) -> Result<(), String> {
        if project.iter().any(|m| m.name == MODULE_NAME) {
            return Err(format!("shaders/{MODULE_NAME}.slang: the name is Lahza's built-in module (rename it)"));
        }
        let shaders = modules(project);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (shaders.iter().map(|m| (&m.name, &m.source)).collect::<Vec<_>>(), &self.functions).hash(&mut h);
        let _ = self.program.set(Arc::new(Program { key: h.finish(), shaders, functions: self.functions.clone() }));
        Ok(())
    }
}

/// A project's surface functions for the GPU.
#[derive(Debug, PartialEq)]
pub struct Program {
    /// Content hash (sources and functions): which shader variant.
    pub key: u64,
    /// The project's modules and the built-in one.
    pub shaders: Vec<Shader>,
    /// By id − 1.
    pub functions: Vec<String>,
}

/// The function a WGSL library exports for the GPU: `fn <name>(id: u32,
/// a0 … a9: vec4<f32>, o0, o1, o2: ptr<function, vec4<f32>>)`, inputs and
/// outputs as `lahza_surface_input` and `lahza_surface_output` pack them.
pub struct Library {
    pub wgsl: String,
    pub dispatch: String,
}

/// Prefix of every module-scope name of a WGSL library (never one of the
/// raster shader's).
const PREFIX: &str = "slang_";

impl Program {
    /// The functions as WGSL with a dispatch on the id, every module-scope
    /// name prefixed: ready to paste into another WGSL module. Cached by
    /// content (`slang::wgsl`).
    pub fn wgsl(&self) -> Result<Library, String> {
        let mut src = crate::slang::imports(&self.shaders);
        let args = (0..10).map(|k| format!("float4 a{k}")).collect::<Vec<_>>().join(", ");
        src.push_str(&format!("void lahza_dispatch(uint id, {args}, out float4 o0, out float4 o1, out float4 o2) {{\n"));
        src.push_str("    SurfaceInput s = lahza_surface_input(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9);\n    Surface r = s.material;\n    switch (id) {\n");
        for (i, f) in self.functions.iter().enumerate() {
            src.push_str(&format!("    case {}: r = {f}(s); break;\n", i + 1));
        }
        src.push_str("    default: break;\n    }\n    lahza_surface_output(r, o0, o1, o2);\n}\n");
        // An entry point only so slangc emits the dispatch; `library`
        // drops it.
        src.push_str(
            "RWStructuredBuffer<float4> lahza_io;\n[numthreads(1, 1, 1)]\nvoid lahza_main() {\n    float4 o0, o1, o2;\n    lahza_dispatch(uint(lahza_io[10].x), lahza_io[0], lahza_io[1], lahza_io[2], lahza_io[3], lahza_io[4], lahza_io[5], lahza_io[6], lahza_io[7], lahza_io[8], lahza_io[9], o0, o1, o2);\n    lahza_io[0] = o0;\n    lahza_io[1] = o1;\n    lahza_io[2] = o2;\n}\n",
        );
        let out = crate::slang::wgsl(&self.shaders, &src, "lahza_main", "compute")?;
        library(&out, "lahza_main", "lahza_dispatch")
    }
}

/// Slang's WGSL as a library: the entry point `entry` and its resource
/// globals dropped, every module-scope name prefixed. `dispatch`: the
/// Slang name of the function to find.
fn library(wgsl: &str, entry: &str, dispatch: &str) -> Result<Library, String> {
    // Drop the entry point (its attributes and body) and the bindings.
    let mut kept = String::new();
    let mut lines = wgsl.lines().peekable();
    let mut attrs: Vec<&str> = Vec::new();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if t.starts_with('@') && !t.contains(" var") && !t.starts_with("@binding") && !t.starts_with("@group") {
            attrs.push(line);
            continue;
        }
        if t.starts_with("@binding") || t.starts_with("@group") {
            let name = t.split_once(" var").and_then(|(_, r)| r.split_once('>').map(|x| x.1).or(Some(r))).map(|r| r.trim().split([' ', ':']).next().unwrap_or("")).unwrap_or("");
            if !name.starts_with("lahza_io") {
                return Err(format!("Slang materials cannot use textures or buffers ('{name}')"));
            }
            continue;
        }
        if t.starts_with("fn ") && t[3..].trim_start().starts_with(entry) {
            attrs.clear();
            // Skip to the end of its body.
            let mut depth = line.matches('{').count() as i64 - line.matches('}').count() as i64;
            let mut opened = depth > 0;
            for l in lines.by_ref() {
                depth += l.matches('{').count() as i64 - l.matches('}').count() as i64;
                opened |= l.contains('{');
                if opened && depth <= 0 {
                    break;
                }
            }
            continue;
        }
        for a in attrs.drain(..) {
            kept.push_str(a);
            kept.push('\n');
        }
        kept.push_str(line);
        kept.push('\n');
    }
    // Module-scope names (Slang writes each declaration at the start of a
    // line).
    let mut names: Vec<String> = Vec::new();
    for line in kept.lines() {
        let mut words = line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| !w.is_empty());
        let first = words.next();
        let name = match first {
            Some("fn" | "struct" | "const" | "alias" | "override") if !line.starts_with(' ') => words.next(),
            Some("var") if !line.starts_with(' ') => line.split_once('>').map_or(line[3..].trim(), |x| x.1.trim()).split([' ', ':']).next(),
            _ => None,
        };
        if let Some(n) = name {
            names.push(n.to_string());
        }
    }
    let renamed = rename(&kept, &names);
    let found = names.iter().find(|n| n.strip_prefix(dispatch).and_then(|r| r.strip_prefix('_')).is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())));
    let dispatch = found.map(|n| format!("{PREFIX}{n}")).ok_or("slangc's WGSL has no surface dispatch")?;
    Ok(Library { wgsl: renamed, dispatch })
}

/// `text` with every identifier in `names` prefixed.
fn rename(text: &str, names: &[String]) -> String {
    let set: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut out = String::with_capacity(text.len() + names.len() * 16);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            if set.contains(word) {
                out.push_str(PREFIX);
            }
            out.push_str(word);
        } else if c.is_ascii_digit() {
            // Numbers (with suffixes like 1u, 2.0f) are not names.
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_') {
                i += 1;
            }
            out.push_str(&text[start..i]);
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// A material's surface function at one moment (`MaterialState::slang`).
#[derive(Clone, Debug)]
pub struct SurfaceState {
    pub function: Arc<str>,
    /// The GPU's dispatch id (1-based).
    pub id: u32,
    pub params: [f32; MAX_PARAMS],
    /// Seconds since the layer started.
    pub time: f64,
    /// Its opacity lets what is behind show (drawn as a see-through mesh).
    pub see_through: bool,
    pub linked: Linked,
    pub program: Arc<OnceLock<Arc<Program>>>,
}

impl SurfaceState {
    /// The CPU function: `f(10 input float4s, 3 output float4s)`; None
    /// until (or unless) the project links.
    pub fn address(&self) -> Option<*const ()> {
        self.linked.address()
    }

    /// The GPU program; None unless the project linked.
    pub fn program(&self) -> Option<Arc<Program>> {
        self.program.get().cloned()
    }
}

impl PartialEq for SurfaceState {
    fn eq(&self, o: &Self) -> bool {
        self.function == o.function && self.id == o.id && self.params == o.params && self.time == o.time && self.see_through == o.see_through && self.linked == o.linked && Arc::ptr_eq(&self.program, &o.program)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_drops_the_entry_and_prefixes_names() {
        let wgsl = "@binding(0) @group(0) var<storage, read_write> lahza_io_0 : array<vec4<f32>>;\n\nstruct Surface_0\n{\n     color_0 : vec3<f32>,\n};\n\nfn lahza_dispatch_0( id_0 : u32,  o_0 : ptr<function, vec4<f32>>)\n{\n    var s_0 : Surface_0;\n    (*o_0) = vec4<f32>(1.0f);\n    return;\n}\n\n@compute\n@workgroup_size(1, 1, 1)\nfn lahza_main()\n{\n    var o_1 : vec4<f32>;\n    lahza_dispatch_0(1u, &(o_1));\n    lahza_io_0[i32(0)] = o_1;\n    return;\n}\n";
        let lib = library(wgsl, "lahza_main", "lahza_dispatch").unwrap();
        assert_eq!(lib.dispatch, "slang_lahza_dispatch_0");
        assert!(!lib.wgsl.contains("lahza_main") && !lib.wgsl.contains("@binding") && !lib.wgsl.contains("@compute"));
        assert!(lib.wgsl.contains("struct slang_Surface_0") && lib.wgsl.contains("var s_0 : slang_Surface_0;"));
    }
}
