//! Shader layers: pictures an author's Slang function paints, one call per
//! pixel (docs/slang/shader-layers.md). The author writes
//!
//! ```slang
//! struct sky_params { float3 tint = float3(1, 1, 1); float speed = 1; }
//! float4 sky(float2 uv, float2 px, float t, sky_params p)
//! ```
//!
//! returning straight colour (`float3` is opaque). The layer's `params`
//! fill the struct's fields by name (those it does not give keep their
//! defaults). A function without a `<function>_params` struct takes its
//! params one by one after `t`, in name order. Lahza writes the
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
FILL    float4 c = lahza_widen(CALL, 1.0);
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
    ///
    /// `params`: the layer's params in the order their values are passed
    /// (name order), each with its components. Where the modules declare
    /// `struct <function>_params` they fill its fields by name and the
    /// struct is the function's last argument; else each is an argument.
    pub fn new(linker: &Linker, shaders: &[Shader], function: &str, params: &[(String, usize)]) -> (Self, Result<(), String>) {
        let components: Vec<usize> = params.iter().map(|p| p.1).collect();
        let named = slang::declares_struct(shaders, &format!("{function}_params"));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (function, &components, shaders.iter().map(|m| (&m.name, &m.source)).collect::<Vec<_>>()).hash(&mut h);
        if named {
            params.iter().map(|p| &p.0).collect::<Vec<_>>().hash(&mut h);
        }
        let part = |n: usize| match n {
            1 => ".x",
            2 => ".xy",
            3 => ".xyz",
            _ => "",
        };
        // The arguments after `t`, and what fills the struct before the
        // call, reading the values from `base`.
        let pass = |base: &str| -> (String, String) {
            if named {
                let mut fill = format!("    {function}_params q = {{}};\n");
                for (i, (name, n)) in params.iter().enumerate() {
                    fill.push_str(&format!("    q.{name} = {base}[{i}]{};\n", part(*n)));
                }
                (fill, ", q".to_string())
            } else {
                (String::new(), params.iter().enumerate().map(|(i, (_, n))| format!(", {base}[{i}]{}", part(*n))).collect())
            }
        };
        let (gpu_fill, gpu_args) = pass("u_shader.params");
        let mut module = slang::imports(shaders);
        module.push_str(slang::PRELUDE);
        module.push_str(&KERNEL.replace("MAX", &MAX_PARAMS.to_string()).replace("FILL", &gpu_fill).replace("CALL", &format!("{function}(uv, px, u_shader.time.x{gpu_args})")));
        let mut program = Program { function: function.into(), components, cpu: None, module, wgsl: OnceLock::new(), key: h.finish() };
        let check = if !slang::valid_name(function) {
            Err(format!("'{function}' is not a Slang function name"))
        } else if let Some((bad, _)) = params.iter().find(|(name, _)| named && !slang::valid_name(name)) {
            Err(format!("param '{bad}' is not a Slang field name"))
        } else if program.components.len() > MAX_PARAMS {
            Err(format!("at most {MAX_PARAMS} params"))
        } else if let Some(e) = slang::missing(shaders, function) {
            Err(e)
        } else {
            Ok(())
        };
        if check.is_ok() {
            let digits: String = program.components.iter().map(|n| n.to_string()).collect();
            let symbol = if named {
                let names: Vec<&str> = params.iter().map(|p| p.0.as_str()).collect();
                format!("lahza_pixel_{function}_n{digits}_{:016x}", names.join(",").bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)))
            } else {
                format!("lahza_pixel_{function}_p{digits}")
            };
            let (cpu_fill, cpu_args) = pass("p");
            program.cpu = Some(linker.export(&symbol, || {
                format!("export __extern_cpp void {symbol}(float u, float v, float x, float y, float t, float4* p, float4* out) {{\n{cpu_fill}    *out = lahza_widen({function}(float2(u, v), float2(x, y), t{cpu_args}), 1.0);\n}}\n")
            }));
        }
        (program, check)
    }

    /// Tests: the GPU kernel's Slang.
    #[cfg(test)]
    pub(crate) fn kernel(&self) -> &str {
        &self.module
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

    /// A function with a `<function>_params` struct takes the layer's
    /// params by field name, on both renderers; one without takes them
    /// one by one in name order, as before.
    #[test]
    fn params_fill_a_struct_by_name_where_the_author_declares_one() {
        let params = [("speed".to_string(), 1), ("tint".to_string(), 3)];
        let named = vec![Shader { name: "fx".into(), source: "struct sky_params { float3 tint = float3(1, 1, 1); float speed = 1; }\nfloat4 sky(float2 uv, float2 px, float t, sky_params p) { return float4(p.tint, 1); }".into() }];
        let linker = Linker::default();
        let (program, check) = Program::new(&linker, &named, "sky", &params);
        assert_eq!(check, Ok(()));
        let kernel = program.kernel();
        assert!(kernel.contains("    sky_params q = {};\n    q.speed = u_shader.params[0].x;\n    q.tint = u_shader.params[1].xyz;\n    float4 c = lahza_widen(sky(uv, px, u_shader.time.x, q), 1.0);"), "{kernel}");
        let cpu = linker.exported("lahza_pixel_sky_n13_").expect("the CPU caller");
        assert!(cpu.contains("float4* p, float4* out) {\n    sky_params q = {};\n    q.speed = p[0].x;\n    q.tint = p[1].xyz;\n    *out = lahza_widen(sky(float2(u, v), float2(x, y), t, q), 1.0);\n}"), "{cpu}");
        // No params given: the struct still goes, with its defaults.
        let (program, check) = Program::new(&linker, &named, "sky", &[]);
        assert_eq!(check, Ok(()));
        assert!(program.kernel().contains("    sky_params q = {};\n    float4 c = lahza_widen(sky(uv, px, u_shader.time.x, q), 1.0);"));
        // No struct: one by one, in the order given (name order).
        let plain = vec![Shader { name: "fx".into(), source: "float4 sky(float2 uv, float2 px, float t, float speed, float3 tint) { return float4(tint, 1); }".into() }];
        let linker = Linker::default();
        let (program, check) = Program::new(&linker, &plain, "sky", &params);
        assert_eq!(check, Ok(()));
        assert!(program.kernel().contains("    float4 c = lahza_widen(sky(uv, px, u_shader.time.x, u_shader.params[0].x, u_shader.params[1].xyz), 1.0);"), "{}", program.kernel());
        assert!(!program.kernel().contains("FILL"));
        let cpu = linker.exported("lahza_pixel_sky_p13").expect("the CPU caller");
        assert!(cpu.contains("*out = lahza_widen(sky(float2(u, v), float2(x, y), t, p[0].x, p[1].xyz), 1.0);"), "{cpu}");
        // A function no module has: named with the files.
        let (_, check) = Program::new(&linker, &plain, "dusk", &[]);
        assert_eq!(check, Err("no Slang function 'dusk' in shaders/fx.slang".into()));
    }
}
