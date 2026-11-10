//! Intel Open Image Denoise, the denoiser Blender's Cycles uses, loaded at
//! run time (no build dependency): `$LAHZA_OIDN` (the library's path), the
//! system's `libOpenImageDenoise.so.2`, or the copy Blender ships in its
//! `lib/` folder. Its "RT" filter is trained on path-traced renders: it
//! denoises the full colour guided by albedo and normal, so the grain of
//! depth of field and other noise in the surface colour goes too (the
//! à-trous filter smooths the light, and the whole colour only where the
//! lens blurs by more than a texel).
//! OIDN picks its fastest device itself (CUDA on NVIDIA, else the CPU).

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;

type Handle = *mut c_void;

const DEVICE_DEFAULT: i32 = 0;
const FORMAT_FLOAT3: i32 = 3;

/// The C API functions the denoiser calls (OIDN 2).
struct Api {
    _lib: libloading::Library,
    new_device: unsafe extern "C" fn(i32) -> Handle,
    commit_device: unsafe extern "C" fn(Handle),
    release_device: unsafe extern "C" fn(Handle),
    device_error: unsafe extern "C" fn(Handle, *mut *const c_char) -> i32,
    new_buffer: unsafe extern "C" fn(Handle, usize) -> Handle,
    write_buffer: unsafe extern "C" fn(Handle, usize, usize, *const c_void),
    read_buffer: unsafe extern "C" fn(Handle, usize, usize, *mut c_void),
    release_buffer: unsafe extern "C" fn(Handle),
    new_filter: unsafe extern "C" fn(Handle, *const c_char) -> Handle,
    set_filter_image: unsafe extern "C" fn(Handle, *const c_char, Handle, i32, usize, usize, usize, usize, usize),
    set_filter_bool: unsafe extern "C" fn(Handle, *const c_char, bool),
    commit_filter: unsafe extern "C" fn(Handle),
    execute_filter: unsafe extern "C" fn(Handle),
    release_filter: unsafe extern "C" fn(Handle),
}

/// Where to look for the library, in order.
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("LAHZA_OIDN") {
        out.push(PathBuf::from(p));
    }
    out.push(PathBuf::from("libOpenImageDenoise.so.2"));
    out.push(PathBuf::from("libOpenImageDenoise.so"));
    // Blender bundles it next to its binary: <install>/lib.
    let mut blenders: Vec<PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let b = dir.join("blender");
            if let Ok(real) = std::fs::canonicalize(&b) {
                blenders.push(real);
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let opt = PathBuf::from(home).join(".local/opt");
        if let Ok(rd) = std::fs::read_dir(&opt) {
            for e in rd.flatten() {
                blenders.push(e.path().join("blender"));
            }
        }
    }
    for b in blenders {
        if let Some(dir) = b.parent() {
            out.push(dir.join("lib/libOpenImageDenoise.so.2"));
        }
    }
    out
}

impl Api {
    fn load() -> Result<Self, String> {
        let mut tried = Vec::new();
        for path in candidates() {
            // SAFETY: loading a shared library runs its initialisers; OIDN's
            // only set up its own state.
            let Ok(lib) = (unsafe { libloading::Library::new(&path) }) else {
                tried.push(path.display().to_string());
                continue;
            };
            // SAFETY: the symbols are OIDN 2's C API with these signatures.
            unsafe {
                macro_rules! sym {
                    ($name:literal) => {
                        *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("{}: {e}", $name))?
                    };
                }
                return Ok(Api {
                    new_device: sym!("oidnNewDevice"),
                    commit_device: sym!("oidnCommitDevice"),
                    release_device: sym!("oidnReleaseDevice"),
                    device_error: sym!("oidnGetDeviceError"),
                    new_buffer: sym!("oidnNewBuffer"),
                    write_buffer: sym!("oidnWriteBuffer"),
                    read_buffer: sym!("oidnReadBuffer"),
                    release_buffer: sym!("oidnReleaseBuffer"),
                    new_filter: sym!("oidnNewFilter"),
                    set_filter_image: sym!("oidnSetFilterImage"),
                    set_filter_bool: sym!("oidnSetFilterBool"),
                    commit_filter: sym!("oidnCommitFilter"),
                    execute_filter: sym!("oidnExecuteFilter"),
                    release_filter: sym!("oidnReleaseFilter"),
                    _lib: lib,
                });
            }
        }
        Err(format!("Open Image Denoise not found (set LAHZA_OIDN, install it, or install Blender); tried {}", tried.join(", ")))
    }
}

/// A committed "RT" filter for one image size, with its device buffers:
/// the input (colour, albedo and normal, nine floats per pixel) and the
/// output (three).
struct Sized {
    size: (usize, usize),
    filter: Handle,
    buffers: [Handle; 2],
}

/// The OIDN device and a filter for the last size denoised.
pub struct Denoiser {
    /// None: a stand-in that returns the colour it is given (tests).
    api: Option<Api>,
    device: Handle,
    sized: Option<Sized>,
    /// Pictures denoised (tests: once per traced list and frame).
    pub(crate) runs: u64,
}

// SAFETY: the handles are only used from the thread that owns the Denoiser
// (the renderer's), one call at a time.
unsafe impl Send for Denoiser {}

/// Floats per pixel of the denoiser's input: colour, albedo, normal.
pub const INPUT_FLOATS: usize = 9;

impl Denoiser {
    pub fn new() -> Result<Self, String> {
        let api = Api::load()?;
        // SAFETY: OIDN C API calls on handles it returned.
        unsafe {
            let device = (api.new_device)(DEVICE_DEFAULT);
            if device.is_null() {
                return Err("Open Image Denoise: no device".into());
            }
            (api.commit_device)(device);
            let d = Denoiser { api: Some(api), device, sized: None, runs: 0 };
            d.check()?;
            Ok(d)
        }
    }

    /// Tests: no library; `denoise` returns the colour unchanged, so what
    /// leads to it and back can be checked without OIDN.
    #[cfg(test)]
    pub(crate) fn pass_through() -> Self {
        Denoiser { api: None, device: std::ptr::null_mut(), sized: None, runs: 0 }
    }

    fn check(&self) -> Result<(), String> {
        let Some(api) = &self.api else { return Ok(()) };
        let mut msg: *const c_char = std::ptr::null();
        // SAFETY: the device is live; the message, when set, is a C string
        // owned by OIDN until the next call.
        let code = unsafe { (api.device_error)(self.device, &mut msg) };
        if code == 0 {
            return Ok(());
        }
        let text = if msg.is_null() { String::new() } else { unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned() };
        Err(format!("Open Image Denoise error {code}: {text}"))
    }

    fn release_sized(&mut self) {
        if let (Some(s), Some(api)) = (self.sized.take(), &self.api) {
            // SAFETY: handles from this device, not used after this.
            unsafe {
                (api.release_filter)(s.filter);
                for b in s.buffers {
                    (api.release_buffer)(b);
                }
            }
        }
    }

    /// Denoises a `w × h` picture. `input` holds per pixel the colour
    /// (linear, straight RGB), the albedo and the normal that guide it
    /// (`INPUT_FLOATS` floats, as the tracer's pack pass writes them);
    /// `output` takes the colour, three floats per pixel. The images are
    /// read in place from the one input buffer (their offsets and the
    /// pixel stride say where), so nothing is repacked on the way.
    pub fn denoise(&mut self, w: usize, h: usize, input: &[f32], output: &mut [f32]) -> Result<(), String> {
        let n = w * h;
        if input.len() != n * INPUT_FLOATS || output.len() != n * 3 {
            return Err("denoise: image sizes differ".into());
        }
        self.runs += 1;
        if self.api.is_none() {
            for (o, i) in output.chunks_exact_mut(3).zip(input.chunks_exact(INPUT_FLOATS)) {
                o.copy_from_slice(&i[..3]);
            }
            return Ok(());
        }
        let (in_bytes, out_bytes) = (n * INPUT_FLOATS * 4, n * 12);
        if self.sized.as_ref().map(|s| s.size) != Some((w, h)) {
            self.release_sized();
            let api = self.api.as_ref().unwrap();
            // SAFETY: OIDN C API calls on this device; names are C strings.
            unsafe {
                let buffers = [(api.new_buffer)(self.device, in_bytes), (api.new_buffer)(self.device, out_bytes)];
                let filter = (api.new_filter)(self.device, c"RT".as_ptr());
                let stride = INPUT_FLOATS * 4;
                let image = |name: &str, b: Handle, offset: usize, stride: usize| {
                    let name = CString::new(name).unwrap();
                    (api.set_filter_image)(filter, name.as_ptr(), b, FORMAT_FLOAT3, w, h, offset, stride, stride * w);
                };
                image("color", buffers[0], 0, stride);
                image("albedo", buffers[0], 12, stride);
                image("normal", buffers[0], 24, stride);
                image("output", buffers[1], 0, 12);
                (api.set_filter_bool)(filter, c"hdr".as_ptr(), true);
                (api.commit_filter)(filter);
                self.sized = Some(Sized { size: (w, h), filter, buffers });
            }
            self.check()?;
        }
        let api = self.api.as_ref().unwrap();
        let s = self.sized.as_ref().unwrap();
        // SAFETY: the buffers hold `in_bytes` and `out_bytes`; the slices
        // are that long.
        unsafe {
            (api.write_buffer)(s.buffers[0], 0, in_bytes, input.as_ptr() as *const c_void);
            (api.execute_filter)(s.filter);
            (api.read_buffer)(s.buffers[1], 0, out_bytes, output.as_mut_ptr() as *mut c_void);
        }
        self.check()
    }
}

impl Drop for Denoiser {
    fn drop(&mut self) {
        self.release_sized();
        if let Some(api) = &self.api {
            // SAFETY: the device is not used after this.
            unsafe { (api.release_device)(self.device) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noisy flat grey becomes nearly flat (skipped without OIDN).
    #[test]
    fn oidn_removes_noise_from_a_flat_image() {
        let Ok(mut d) = Denoiser::new() else {
            eprintln!("skipping: no Open Image Denoise");
            return;
        };
        let (w, h) = (64usize, 64usize);
        let mut seed = 7u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let color: Vec<f32> = (0..w * h * 3).map(|_| 0.5 + (rnd() - 0.5) * 0.4).collect();
        let input: Vec<f32> = color.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 0.8, 0.8, 0.8, 0.0, 0.0, 1.0]).collect();
        let mut out = vec![0.0f32; w * h * 3];
        d.denoise(w, h, &input, &mut out).unwrap();
        let sd = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
        };
        assert!(sd(&out) < sd(&color) / 4.0, "noise {} → {}", sd(&color), sd(&out));
    }
}
