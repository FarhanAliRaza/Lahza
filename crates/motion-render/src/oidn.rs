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

/// A committed "RT" filter for one image size, with its device buffers.
struct Sized {
    size: (usize, usize),
    filter: Handle,
    buffers: [Handle; 4],
}

/// The OIDN device and a filter for the last size denoised.
pub struct Denoiser {
    api: Api,
    device: Handle,
    sized: Option<Sized>,
}

// SAFETY: the handles are only used from the thread that owns the Denoiser
// (the renderer's), one call at a time.
unsafe impl Send for Denoiser {}

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
            let d = Denoiser { api, device, sized: None };
            d.check()?;
            Ok(d)
        }
    }

    fn check(&self) -> Result<(), String> {
        let mut msg: *const c_char = std::ptr::null();
        // SAFETY: the device is live; the message, when set, is a C string
        // owned by OIDN until the next call.
        let code = unsafe { (self.api.device_error)(self.device, &mut msg) };
        if code == 0 {
            return Ok(());
        }
        let text = if msg.is_null() { String::new() } else { unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned() };
        Err(format!("Open Image Denoise error {code}: {text}"))
    }

    fn release_sized(&mut self) {
        if let Some(s) = self.sized.take() {
            // SAFETY: handles from this device, not used after this.
            unsafe {
                (self.api.release_filter)(s.filter);
                for b in s.buffers {
                    (self.api.release_buffer)(b);
                }
            }
        }
    }

    /// Denoises `color` (linear, straight RGB) guided by `albedo` and
    /// `normal`, all `w × h` RGB f32 rows, into `out`.
    pub fn denoise(&mut self, w: usize, h: usize, color: &[f32], albedo: &[f32], normal: &[f32], out: &mut [f32]) -> Result<(), String> {
        let n = w * h * 3;
        if color.len() != n || albedo.len() != n || normal.len() != n || out.len() != n {
            return Err("denoise: image sizes differ".into());
        }
        let bytes = n * 4;
        if self.sized.as_ref().map(|s| s.size) != Some((w, h)) {
            self.release_sized();
            // SAFETY: OIDN C API calls on this device; names are C strings.
            unsafe {
                let buffers = [0; 4].map(|_| (self.api.new_buffer)(self.device, bytes));
                let filter = (self.api.new_filter)(self.device, c"RT".as_ptr());
                let image = |name: &str, b: Handle| {
                    let name = CString::new(name).unwrap();
                    (self.api.set_filter_image)(filter, name.as_ptr(), b, FORMAT_FLOAT3, w, h, 0, 0, 0);
                };
                image("color", buffers[0]);
                image("albedo", buffers[1]);
                image("normal", buffers[2]);
                image("output", buffers[3]);
                (self.api.set_filter_bool)(filter, c"hdr".as_ptr(), true);
                (self.api.commit_filter)(filter);
                self.sized = Some(Sized { size: (w, h), filter, buffers });
            }
            self.check()?;
        }
        let s = self.sized.as_ref().unwrap();
        // SAFETY: each buffer holds `bytes`; the slices are `n` f32 long.
        unsafe {
            (self.api.write_buffer)(s.buffers[0], 0, bytes, color.as_ptr() as *const c_void);
            (self.api.write_buffer)(s.buffers[1], 0, bytes, albedo.as_ptr() as *const c_void);
            (self.api.write_buffer)(s.buffers[2], 0, bytes, normal.as_ptr() as *const c_void);
            (self.api.execute_filter)(s.filter);
            (self.api.read_buffer)(s.buffers[3], 0, bytes, out.as_mut_ptr() as *mut c_void);
        }
        self.check()
    }
}

impl Drop for Denoiser {
    fn drop(&mut self) {
        self.release_sized();
        // SAFETY: the device is not used after this.
        unsafe { (self.api.release_device)(self.device) };
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
        let albedo = vec![0.8f32; w * h * 3];
        let normal: Vec<f32> = (0..w * h).flat_map(|_| [0.0f32, 0.0, 1.0]).collect();
        let mut out = vec![0.0f32; w * h * 3];
        d.denoise(w, h, &color, &albedo, &normal, &mut out).unwrap();
        let sd = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
        };
        assert!(sd(&out) < sd(&color) / 4.0, "noise {} → {}", sd(&color), sd(&out));
    }
}
