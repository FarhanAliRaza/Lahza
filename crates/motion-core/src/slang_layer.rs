//! Shader layers: pictures an author's Slang function paints, one call per
//! pixel (docs/slang/shader-layers.md). The author writes
//!
//! ```slang
//! float4 sky(float2 uv, float2 px, float t /*, params in name order */)
//! ```
//!
//! returning straight colour (`float3` is opaque). Lahza writes the
//! callers: for the CPU an exported `lahza_pixel_*` function the CPU
//! renderer calls per pixel, for the GPU a compute kernel that fills the
//! layer's picture in the group buffer it is composited from.

use crate::slang::{self, Linked, Linker, Shader};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

/// Most arguments a layer passes after `t` (the GPU's uniform holds 256
/// bytes).
pub const MAX_PARAMS: usize = 12;
/// The GPU kernel's entry point.
pub const ENTRY: &str = "lahza_shader";

/// The GPU kernel's uniform (`u_shader_0` once Slang renames it): size =
/// [picture width, height, first texel in the group buffer, 0], time = [t,
/// 0, layer width, layer height], then the arguments, one `float4` each.
/// Its pixels go to `b_pixels_0` (premultiplied, row-major), 16 × 16
/// threads per group.
const KERNEL: &str = "
struct LahzaShaderPass { uint4 size; float4 time; float4 params[MAX]; };
ConstantBuffer<LahzaShaderPass> u_shader;
RWStructuredBuffer<float4> b_pixels;

[shader(\"compute\")]
[numthreads(16, 16, 1)]
void lahza_shader(uint3 id : SV_DispatchThreadID) {
    if (id.x >= u_shader.size.x || id.y >= u_shader.size.y) return;
    float2 uv = (float2(id.xy) + 0.5) / float2(u_shader.size.xy);
    float2 px = uv * u_shader.time.zw;
    float4 c = lahza_widen(CALL, 1.0);
    if (any(isnan(c)) || any(isinf(c))) c = float4(0.0);
    float a = saturate(c.w);
    b_pixels[u_shader.size.z + id.y * u_shader.size.x + id.x] = float4(c.xyz * a, a);
}
";

/// One pixel: `f(u, v, x, y, t, arguments, out)`, straight colour out.
pub type PixelFn = unsafe extern "C" fn(f32, f32, f32, f32, f32, *const [f32; 4], *mut [f32; 4]);

/// A shader function as layers call it (with these argument types), built
/// for both renderers. Layers calling it the same way share one.
pub struct Program {
    pub function: String,
    /// Components of each argument after `t`.
    pub components: Vec<usize>,
    /// The CPU caller; `None` when the function cannot be called (an error
    /// the project reports).
    cpu: Option<Linked>,
    /// The GPU kernel's Slang.
    module: String,
    /// Its WGSL, once the project links (`build_gpu`).
    wgsl: OnceLock<Arc<str>>,
    /// Content hash of the function, its argument types and the project's
    /// modules: the frame cache's key for the code.
    pub key: u64,
}

impl Program {
    /// Binds `function` with arguments of `components` (exporting its CPU
    /// caller to `linker`); Err (nothing exported) when it cannot be called.
    pub fn new(linker: &Linker, shaders: &[Shader], function: &str, components: Vec<usize>) -> (Self, Result<(), String>) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (function, &components, shaders.iter().map(|m| (&m.name, &m.source)).collect::<Vec<_>>()).hash(&mut h);
        let arg = |i: usize, n: usize, base: &str| match n {
            1 => format!(", {base}[{i}].x"),
            2 => format!(", {base}[{i}].xy"),
            3 => format!(", {base}[{i}].xyz"),
            _ => format!(", {base}[{i}]"),
        };
        let gpu_args: String = components.iter().enumerate().map(|(i, &n)| arg(i, n, "u_shader.params")).collect();
        let mut module = slang::imports(shaders);
        module.push_str(slang::PRELUDE);
        module.push_str(&KERNEL.replace("MAX", &MAX_PARAMS.to_string()).replace("CALL", &format!("{function}(uv, px, u_shader.time.x{gpu_args})")));
        let mut program = Program { function: function.into(), components, cpu: None, module, wgsl: OnceLock::new(), key: h.finish() };
        let check = if !slang::valid_name(function) {
            Err(format!("'{function}' is not a Slang function name"))
        } else if program.components.len() > MAX_PARAMS {
            Err(format!("at most {MAX_PARAMS} params"))
        } else if shaders.is_empty() {
            Err(format!("no Slang function '{function}': the project has no shaders/*.slang"))
        } else if !shaders.iter().any(|m| mentions(&m.source, function)) {
            let names: Vec<String> = shaders.iter().map(|m| format!("shaders/{}.slang", m.name)).collect();
            Err(format!("no Slang function '{function}' in {}", names.join(", ")))
        } else {
            Ok(())
        };
        if check.is_ok() {
            let digits: String = program.components.iter().map(|n| n.to_string()).collect();
            let symbol = format!("lahza_pixel_{function}_p{digits}");
            let cpu_args: String = program.components.iter().enumerate().map(|(i, &n)| arg(i, n, "p")).collect();
            program.cpu = Some(linker.export(&symbol, || {
                format!("export __extern_cpp void {symbol}(float u, float v, float x, float y, float t, float4* p, float4* out) {{ *out = lahza_widen({function}(float2(u, v), float2(x, y), t{cpu_args}), 1.0); }}\n")
            }));
        }
        (program, check)
    }

    /// The CPU caller; None until the project links (or when it cannot).
    pub fn pixel(&self) -> Option<PixelFn> {
        let a = self.cpu.as_ref()?.address()?;
        // SAFETY: `new` exported this symbol with exactly this signature,
        // and the runtime keeps its library loaded.
        Some(unsafe { std::mem::transmute::<*const (), PixelFn>(a) })
    }

    /// Compiles the GPU kernel (once the project linked: the same sources
    /// compile).
    pub fn build_gpu(&self, shaders: &[Shader]) -> Result<(), String> {
        if self.cpu.is_none() || self.wgsl.get().is_some() {
            return Ok(());
        }
        let wgsl = slang::wgsl(shaders, &self.module, ENTRY, "compute")?;
        let _ = self.wgsl.set(wgsl.into());
        Ok(())
    }

    /// The GPU kernel; None when it did not build (an error the project
    /// reports).
    pub fn wgsl(&self) -> Option<&Arc<str>> {
        self.wgsl.get()
    }
}

/// Whether `source` names `function` as a call or definition (`name(`): a
/// clear error for a misspelt function, before the compiler's.
fn mentions(source: &str, function: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    source.match_indices(function).any(|(i, _)| {
        let before = source[..i].chars().next_back().is_none_or(|c| !ident(c));
        let after = source[i + function.len()..].trim_start();
        before && after.starts_with('(')
    })
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "shader {}({:?})", self.function, self.components)
    }
}

impl PartialEq for Program {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.cpu == other.cpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_functions_by_name() {
        assert!(mentions("float4 sky(float2 uv) {}", "sky"));
        assert!(mentions("float4 sky (float2 uv) {}", "sky"));
        assert!(!mentions("float4 skyline(float2 uv) {}", "sky"));
        assert!(!mentions("float4 bluesky(float2 uv) {}", "sky"));
    }
}
