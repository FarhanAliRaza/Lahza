//! Slang shaders of a project (`shaders/*.slang`): code an author or agent
//! writes to drive the engine. Every module is compiled by `slangc` (on
//! PATH, or `LAHZA_SLANGC`) for the CPU into one native library, so the
//! same functions run for the CPU renderer and for anything evaluated on
//! the CPU (scene parameters); the GPU paths compile the same modules to
//! WGSL.
//!
//! Authors write plain functions; Lahza writes the entry points: a wrapper
//! module imports the project's modules and exports one C function per use
//! (`lahza_param_<slot>`), widening whatever the author returns (`float` …
//! `float4`) to four floats. Builds are cached by content under the user's
//! cache folder and loaded once per process.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// A module in `shaders/`: its name (the file stem, what `import` names)
/// and its source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shader {
    pub name: String,
    pub source: String,
}

/// Reads `dir/shaders/*.slang` in name order.
pub fn load(dir: &Path) -> std::io::Result<Vec<Shader>> {
    let folder = dir.join("shaders");
    if !folder.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&folder)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("slang") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        out.push(Shader { name: name.to_string(), source: std::fs::read_to_string(&path)? });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The Slang compiler: `LAHZA_SLANGC`, else `slangc` on PATH.
pub fn compiler() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("LAHZA_SLANGC") {
        return Ok(PathBuf::from(p));
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("slangc"))
        .find(|p| p.is_file())
        .ok_or_else(|| "Slang shaders need slangc on PATH (https://github.com/shader-slang/slang/releases)".to_string())
}

/// `slangc -v`, part of every cache key (a new compiler rebuilds).
fn compiler_version(slangc: &Path) -> String {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            std::process::Command::new(slangc)
                .arg("-v")
                .output()
                .map(|o| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).trim().to_string())
                .unwrap_or_default()
        })
        .clone()
}

/// Where builds are kept: `$XDG_CACHE_HOME/lahza/slang` (or `~/.cache`).
pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("lahza").join("slang")
}

/// Helpers every wrapper module has: `lahza_widen` turns whatever an
/// author returns (`float` … `float4`) into four floats; a scalar applies
/// to every component and missing components are `keep` (what leaves the
/// value as it is: 0 added, 1 multiplied).
pub const PRELUDE: &str = "\
float4 lahza_widen(float v, float keep) { return float4(v, v, v, v); }
float4 lahza_widen(float2 v, float keep) { return float4(v, keep, keep); }
float4 lahza_widen(float3 v, float keep) { return float4(v, keep); }
float4 lahza_widen(float4 v, float keep) { return v; }
";

/// Whether `source` names `function` as a call or definition (`name(`): a
/// clear error for a misspelt function, before the compiler's.
pub fn mentions(source: &str, function: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    source.match_indices(function).any(|(i, _)| {
        let before = source[..i].chars().next_back().is_none_or(|c| !ident(c));
        let after = source[i + function.len()..].trim_start();
        before && after.starts_with('(')
    })
}

/// Whether the project's modules declare `struct <name>`: how Lahza
/// tells that a function takes its params as a `<function>_params` struct
/// (filled by field name) rather than not at all or one by one.
pub fn declares_struct(shaders: &[Shader], name: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    shaders.iter().any(|m| {
        m.source.match_indices(name).any(|(i, _)| {
            let before = m.source[..i].trim_end();
            let after = m.source[i + name.len()..].chars().next();
            before.ends_with("struct") && before[..before.len() - 6].chars().next_back().is_none_or(|c| !ident(c)) && after.is_none_or(|c| !ident(c))
        })
    })
}

/// Why `function` cannot be called (no modules, or none names it), for an
/// error before the compiler's; None when a module names it.
pub fn missing(shaders: &[Shader], function: &str) -> Option<String> {
    if shaders.is_empty() {
        return Some(format!("no Slang function '{function}': the project has no shaders/*.slang"));
    }
    if shaders.iter().any(|m| mentions(&m.source, function)) {
        return None;
    }
    let names: Vec<String> = shaders.iter().map(|m| format!("shaders/{}.slang", m.name)).collect();
    Some(format!("no Slang function '{function}' in {}", names.join(", ")))
}

/// Whether `name` can be pasted into generated Slang as a function name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `import <module>;` for every project module (what a wrapper starts with).
pub fn imports(shaders: &[Shader]) -> String {
    shaders.iter().map(|m| format!("import {};\n", m.name)).collect()
}

/// A project's CPU library and the addresses of what it exports.
pub struct Runtime {
    // Keeps the code the addresses below point into loaded.
    _lib: libloading::Library,
    symbols: HashMap<String, usize>,
    /// The build's content hash (what a cache of its results keys on).
    key: u64,
}

impl Runtime {
    /// The address of exported `symbol` (one a `Linker::export` added).
    pub fn symbol(&self, symbol: &str) -> Option<*const ()> {
        self.symbols.get(symbol).map(|&a| a as *const ())
    }
}

/// Collects what a project's Slang uses export while the project compiles
/// (scene parameters, and whatever else calls an author's functions on the
/// CPU), then builds it all into one library (`link`). Each use holds a
/// `Linked` that finds its function once the library is loaded.
#[derive(Default)]
pub struct Linker {
    /// Wrapper functions: (symbol, Slang code), one per symbol.
    exports: Mutex<Vec<(String, String)>>,
    /// Uses that cannot be built (bad names), reported by `link`.
    errors: Mutex<Vec<String>>,
    /// Functions uses call that `link` looks for in the modules first
    /// (with what uses them), for an error that names the function and
    /// the files rather than the compiler's in Lahza's generated call.
    wanted: Mutex<Vec<(String, String)>>,
    runtime: Arc<OnceLock<Arc<Runtime>>>,
}

impl Linker {
    /// Exports `symbol`, defined by `code` (an `export __extern_cpp`
    /// function calling the project's functions; made once per symbol).
    pub fn export(&self, symbol: &str, code: impl FnOnce() -> String) -> Linked {
        let mut exports = self.exports.lock().unwrap();
        if !exports.iter().any(|(s, _)| s == symbol) {
            exports.push((symbol.to_string(), code()));
        }
        Linked { symbol: symbol.into(), runtime: self.runtime.clone() }
    }

    /// Reports a use that cannot be built (from `link`).
    pub fn error(&self, message: String) {
        self.errors.lock().unwrap().push(message);
    }

    /// Notes that `what` calls `function`: `link` reports it by name when
    /// no module has it.
    pub fn want(&self, function: &str, what: &str) {
        let mut wanted = self.wanted.lock().unwrap();
        if !wanted.iter().any(|(f, _)| f == function) {
            wanted.push((function.to_string(), what.to_string()));
        }
    }

    /// Tests: the wrapper exported as `symbol`, and the errors so far.
    #[cfg(test)]
    pub(crate) fn exported(&self, symbol: &str) -> Option<String> {
        self.exports.lock().unwrap().iter().find(|(s, _)| s.starts_with(symbol)).map(|(_, code)| code.clone())
    }

    #[cfg(test)]
    pub(crate) fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }

    /// A use of scene-parameter function `function`: `f(local seconds)`,
    /// added to the value or (`mul`) multiplying it.
    pub fn param(&self, function: &str, mul: bool) -> Param {
        if !valid_name(function) {
            self.error(format!("'{function}' is not a Slang function name"));
        }
        self.want(function, "a scene parameter");
        let symbol = format!("lahza_param_{function}_{}", if mul { "mul" } else { "add" });
        let keep = if mul { "1.0" } else { "0.0" };
        Param(self.export(&symbol, || format!("export __extern_cpp void {symbol}(float t, float4* out) {{ *out = lahza_widen({function}(t), {keep}); }}\n")))
    }

    /// Whether anything asked for a Slang function.
    pub fn is_empty(&self) -> bool {
        self.exports.lock().unwrap().is_empty() && self.errors.lock().unwrap().is_empty()
    }

    /// Builds (or reuses) the library for `shaders`; errors are the
    /// compiler's, with file and line.
    pub fn link(&self, shaders: &[Shader]) -> Result<Arc<Runtime>, String> {
        let mut errors = self.errors.lock().unwrap().clone();
        errors.extend(self.wanted.lock().unwrap().iter().filter(|(f, _)| valid_name(f)).filter_map(|(f, what)| missing(shaders, f).map(|e| format!("{e} ({what} calls it)"))));
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        let exports = self.exports.lock().unwrap().clone();
        let runtime = build(shaders, &exports)?;
        let _ = self.runtime.set(runtime.clone());
        Ok(runtime)
    }
}

/// A function the linker exports, found once the project links.
#[derive(Clone)]
pub struct Linked {
    symbol: Arc<str>,
    runtime: Arc<OnceLock<Arc<Runtime>>>,
}

impl Linked {
    /// Its address; None until the project links (or when it failed to,
    /// which the project reports as an error).
    pub fn address(&self) -> Option<*const ()> {
        self.runtime.get()?.symbol(&self.symbol)
    }

    /// The content hash of the library it is in (None until linked): the
    /// same code gives the same results.
    pub fn build(&self) -> Option<u64> {
        self.runtime.get().map(|r| r.key)
    }
}

impl std::fmt::Debug for Linked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "slang {}", self.symbol)
    }
}

impl PartialEq for Linked {
    fn eq(&self, other: &Self) -> bool {
        self.symbol == other.symbol && Arc::ptr_eq(&self.runtime, &other.runtime)
    }
}

/// A scene parameter bound to its function (see `Linker::param`).
#[derive(Clone, Debug, PartialEq)]
pub struct Param(Linked);

/// `f(local seconds, out four floats)`, as `Linker::param` exports it.
type ParamFn = unsafe extern "C" fn(f32, *mut [f32; 4]);

impl Param {
    /// The function at local time `t`; zeros until the project links.
    pub fn at(&self, t: f64) -> [f64; 4] {
        let mut out = [0f32; 4];
        if let Some(a) = self.0.address() {
            // SAFETY: `Linker::param` exported this symbol with exactly this
            // signature, and the runtime keeps its library loaded.
            let f: ParamFn = unsafe { std::mem::transmute(a) };
            unsafe { f(t as f32, &mut out) };
        }
        out.map(f64::from)
    }
}

/// Loaded libraries by build key: one load per process.
fn loaded() -> &'static Mutex<HashMap<u64, Arc<Runtime>>> {
    static LOADED: OnceLock<Mutex<HashMap<u64, Arc<Runtime>>>> = OnceLock::new();
    LOADED.get_or_init(Default::default)
}

/// Writes the project's modules under `dir/src` (what `-I` points at).
fn write_sources(dir: &Path, shaders: &[Shader]) -> Result<PathBuf, String> {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    for m in shaders {
        std::fs::write(src.join(format!("{}.slang", m.name)), &m.source).map_err(|e| e.to_string())?;
    }
    Ok(src)
}

/// A content hash of the compiler, the generated module and the sources.
fn build_key(slangc: &Path, generated: &str, shaders: &[Shader], target: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (compiler_version(slangc), target, generated, shaders.iter().map(|m| (&m.name, &m.source)).collect::<Vec<_>>()).hash(&mut h);
    h.finish()
}

fn build(shaders: &[Shader], exports: &[(String, String)]) -> Result<Arc<Runtime>, String> {
    let slangc = compiler()?;
    let mut wrap = imports(shaders);
    wrap.push_str(PRELUDE);
    for (_, code) in exports {
        wrap.push_str(code);
    }
    let key = build_key(&slangc, &wrap, shaders, "sharedlib");
    if let Some(r) = loaded().lock().unwrap().get(&key) {
        return Ok(r.clone());
    }
    let dir = cache_dir().join(format!("{key:016x}"));
    let lib = dir.join("lahza.so");
    if !lib.is_file() {
        let src = write_sources(&dir, shaders)?;
        let entry = dir.join("lahza_cpu.slang");
        std::fs::write(&entry, &wrap).map_err(|e| e.to_string())?;
        // Built beside the final name, then renamed: another process never
        // loads half a library.
        let tmp = dir.join(format!("lahza.{}.so", std::process::id()));
        let out = std::process::Command::new(&slangc)
            .arg(&entry)
            .arg("-I")
            .arg(&src)
            .args(["-target", "sharedlib", "-o"])
            .arg(&tmp)
            .output()
            .map_err(|e| format!("{}: {e}", slangc.display()))?;
        if !out.status.success() || !tmp.is_file() {
            return Err(diagnostics(&String::from_utf8_lossy(&out.stderr), &src, Some((&entry, &wrap))));
        }
        std::fs::rename(&tmp, &lib).map_err(|e| e.to_string())?;
    }
    // SAFETY: a library built from the project's shaders by slangc; its
    // initialisers are Slang's generated code.
    let library = unsafe { libloading::Library::new(&lib) }.map_err(|e| format!("{}: {e}", lib.display()))?;
    let mut symbols = HashMap::new();
    for (symbol, _) in exports {
        // SAFETY: only the address is taken here; each use calls it with
        // the signature it exported.
        let f = unsafe { library.get::<unsafe extern "C" fn()>(format!("{symbol}\0").as_bytes()) }.map_err(|e| format!("{symbol}: {e}"))?;
        symbols.insert(symbol.clone(), *f as usize);
    }
    let runtime = Arc::new(Runtime { _lib: library, symbols, key });
    loaded().lock().unwrap().insert(key, runtime.clone());
    Ok(runtime)
}

/// Compiles `module` (Lahza-written Slang: start it with `imports(shaders)`
/// to reach the project's modules) to WGSL for entry point `entry` of
/// `stage` ("compute", "vertex" or "fragment"), for the GPU renderer.
/// Cached by content like the CPU build.
pub fn wgsl(shaders: &[Shader], module: &str, entry: &str, stage: &str) -> Result<String, String> {
    let slangc = compiler()?;
    let key = build_key(&slangc, &format!("{module}\u{0}{entry}\u{0}{stage}"), shaders, "wgsl");
    let dir = cache_dir().join(format!("{key:016x}"));
    let out_path = dir.join("lahza.wgsl");
    if let Ok(text) = std::fs::read_to_string(&out_path) {
        return Ok(text);
    }
    let src = write_sources(&dir, shaders)?;
    let entry_file = dir.join("lahza_gpu.slang");
    std::fs::write(&entry_file, module).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("lahza.{}.wgsl", std::process::id()));
    let out = std::process::Command::new(&slangc)
        .arg(&entry_file)
        .arg("-I")
        .arg(&src)
        .args(["-target", "wgsl", "-entry", entry, "-stage", stage, "-o"])
        .arg(&tmp)
        .output()
        .map_err(|e| format!("{}: {e}", slangc.display()))?;
    if !out.status.success() || !tmp.is_file() {
        return Err(diagnostics(&String::from_utf8_lossy(&out.stderr), &src, Some((&entry_file, module))));
    }
    std::fs::rename(&tmp, &out_path).map_err(|e| e.to_string())?;
    std::fs::read_to_string(&out_path).map_err(|e| e.to_string())
}

/// The compiler's errors as `shaders/<file>:<line>:<col>: <what>`, one
/// per error (the follow-on "import failed" lines and the C++ compiler's
/// noise dropped). An error in `generated` (the file Lahza wrote to call
/// the author's functions, and its text) is given as the call it is in:
/// its place in a cache folder says nothing to the author, the call says
/// which function is used how.
fn diagnostics(stderr: &str, src: &Path, generated: Option<(&Path, &str)>) -> String {
    let src = format!("{}/", src.display());
    // `<generated file>:<line>:<col>` → the line Lahza wrote there.
    let call = |at: &str| -> Option<String> {
        let (file, text) = generated?;
        let rest = at.strip_prefix(&*file.to_string_lossy())?.strip_prefix(':')?;
        let line: usize = rest.split(':').next()?.trim().parse().ok()?;
        Some(text.lines().nth(line.checked_sub(1)?)?.trim().to_string())
    };
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<(String, Option<String>, Option<String>)> = None;
    let flush = |c: Option<(String, Option<String>, Option<String>)>, out: &mut Vec<String>| {
        if let Some((what, at, detail)) = c {
            if what.contains("import failed") || what.contains("compilation ceased") {
                return;
            }
            let what = detail.unwrap_or(what);
            out.push(match at {
                Some(at) => match call(&at) {
                    Some(line) => {
                        // What usually is behind it.
                        let hint = if what.contains("arguments") || what.contains("argument") {
                            ": the function's parameters are not what its use passes (see docs/slang for each use's signature; params go in a `<function>_params` struct)"
                        } else {
                            ""
                        };
                        format!("in Lahza's call `{line}`: {what}{hint}")
                    }
                    None => format!("{}: {what}", at.replace(&src, "shaders/")),
                },
                None => what,
            });
        }
    };
    for line in stderr.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("error").or_else(|| t.strip_prefix("fatal error")) {
            flush(current.take(), &mut out);
            let what = rest.split_once("]: ").map_or(rest.trim_start_matches(':'), |(_, m)| m).trim();
            current = Some((what.to_string(), None, None));
        } else if let Some(at) = t.strip_prefix("--> ") {
            if let Some(c) = current.as_mut() {
                c.1.get_or_insert(at.trim().to_string());
            }
        } else if t.starts_with('|') && t.contains('^') {
            if let Some(c) = current.as_mut() {
                let detail = t.rsplit_once('^').map(|(_, d)| d.trim()).unwrap_or("");
                if !detail.is_empty() {
                    c.2.get_or_insert(detail.trim_end_matches('.').to_string());
                }
            }
        } else if t.contains(": error") && !t.contains("#pragma once") {
            // The C++ compiler's own errors (`file(line): error : …`).
            flush(current.take(), &mut out);
            out.push(t.replace(&src, "shaders/"));
        }
    }
    flush(current.take(), &mut out);
    if out.is_empty() {
        format!("slangc failed: {}", stderr.trim())
    } else {
        format!("Slang: {}", out.join("; "))
    }
}

/// Checks one module on its own (syntax and types), for authoring tools.
pub fn check(shaders: &[Shader], name: &str) -> Result<(), String> {
    let slangc = compiler()?;
    let dir = std::env::temp_dir().join(format!("lahza-slang-check-{}", std::process::id()));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).map_err(|e| e.to_string())?;
    for m in shaders {
        std::fs::write(src.join(format!("{}.slang", m.name)), &m.source).map_err(|e| e.to_string())?;
    }
    let out = std::process::Command::new(&slangc)
        .arg(src.join(format!("{name}.slang")))
        .arg("-I")
        .arg(&src)
        .args(["-target", "cpp", "-o"])
        .arg(dir.join("check.cpp"))
        .output()
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_dir_all(&dir);
    let out = out?;
    if out.status.success() {
        Ok(())
    } else {
        Err(diagnostics(&String::from_utf8_lossy(&out.stderr), &src, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(source: &str) -> Vec<Shader> {
        vec![Shader { name: "fx".into(), source: source.into() }]
    }

    #[test]
    fn finds_functions_and_structs_by_name() {
        assert!(mentions("float4 sky(float2 uv) {}", "sky"));
        assert!(mentions("float4 sky (float2 uv) {}", "sky"));
        assert!(!mentions("float4 skyline(float2 uv) {}", "sky"));
        assert!(!mentions("float4 bluesky(float2 uv) {}", "sky"));
        assert!(declares_struct(&module("struct sky_params { float3 tint; }"), "sky_params"));
        assert!(declares_struct(&module("public struct\n  sky_params\n{ float3 tint; }"), "sky_params"));
        assert!(!declares_struct(&module("struct sky_params2 { float a; }"), "sky_params"));
        assert!(!declares_struct(&module("float4 sky(float2 uv, sky_params p) {}"), "sky_params"));
        assert!(!declares_struct(&module("mystruct sky_params;"), "sky_params"));
        assert_eq!(missing(&[], "sky").unwrap(), "no Slang function 'sky': the project has no shaders/*.slang");
        assert_eq!(missing(&module("float4 dusk(float2 uv) {}"), "sky").unwrap(), "no Slang function 'sky' in shaders/fx.slang");
        assert_eq!(missing(&module("float4 sky(float2 uv) {}"), "sky"), None);
    }

    /// A scene parameter whose function no module has is reported by name
    /// with the files, before the compiler runs.
    #[test]
    fn a_missing_scene_parameter_function_is_named() {
        let linker = Linker::default();
        linker.param("orbit", false);
        linker.param("breathe", true);
        let e = linker.link(&module("float breathe(float t) { return 1.0; }")).err().unwrap();
        assert_eq!(e, "no Slang function 'orbit' in shaders/fx.slang (a scene parameter calls it)");
    }

    /// Errors in the author's files keep their place as `shaders/…`;
    /// errors in the call Lahza generated are shown as that call.
    #[test]
    fn errors_in_generated_calls_are_shown_as_the_call() {
        let src = Path::new("/cache/abc/src");
        let wrapper = Path::new("/cache/abc/lahza_cpu.slang");
        let text = "import fx;\nfloat4 lahza_widen(float v, float keep) { return float4(v, v, v, v); }\nexport __extern_cpp void lahza_mesh_quad(uint v0, uint v1) {\n    for (uint i = v0; i < v1; i++) {\n        float3 v = quad_position(i, t);\n    }\n}\n";
        let stderr = "error[E30043]: not enough arguments to call (got 2, expected 3)\n  --> /cache/abc/lahza_cpu.slang:5:33\n   |\n 5 |         float3 v = quad_position(i, t);\n   |                                 ^\nerror[E30015]: undefined identifier 'siz'.\n  --> /cache/abc/src/fx.slang:3:40\n   |\n 3 | float3 quad_position(uint i) { return siz; }\n   |                                        ^^^ undefined identifier\n";
        let got = diagnostics(stderr, src, Some((wrapper, text)));
        assert_eq!(
            got,
            "Slang: in Lahza's call `float3 v = quad_position(i, t);`: not enough arguments to call (got 2, expected 3): the function's parameters are not what its use passes (see docs/slang for each use's signature; params go in a `<function>_params` struct); shaders/fx.slang:3:40: undefined identifier"
        );
        // Without the generated file's text (a module checked on its own).
        assert!(diagnostics(stderr, src, None).contains("/cache/abc/lahza_cpu.slang:5:33: not enough arguments"));
    }
}
