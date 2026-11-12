//! NVIDIA DLSS Ray Reconstruction, through Lahza's shim over the NGX SDK
//! (packaging/ngx, `just ngx`): a traced frame of few samples at half
//! size becomes a clean frame at full size on the GPU. The shim is looked
//! for at `LAHZA_NGX`, then in ~/.local/lib/lahza; NVIDIA's own library
//! sits beside it. Without it (or off NVIDIA) there is no fast mode.

use blade_graphics as gpu;
use std::ffi::{c_char, c_void, CString};

/// An image as the shim takes it (`LahzaNgxImage`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Image {
    pub image: u64,
    pub view: u64,
    /// `VkFormat`.
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub pad: u32,
}

/// A frame to reconstruct (`LahzaNgxFrame`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Frame {
    pub color: Image,
    pub diffuse: Image,
    pub specular: Image,
    pub normals: Image,
    pub depth: Image,
    pub motion: Image,
    pub output: Image,
    /// Where in the texel the frame's samples went, from its centre.
    pub jitter: [f32; 2],
    pub width: u32,
    pub height: u32,
    pub reset: u32,
    pub pad: u32,
}

pub(crate) struct Ngx {
    _lib: libloading::Library,
    device: u64,
    create: unsafe extern "C" fn(u64, u64, u32, u32, u32, u32) -> *mut c_void,
    evaluate: unsafe extern "C" fn(u64, *mut c_void, *const Frame) -> i32,
    release: unsafe extern "C" fn(*mut c_void),
    shutdown: unsafe extern "C" fn(u64),
}

/// A reconstruction's state (its history of frames) in the SDK.
#[derive(Clone, Copy)]
pub(crate) struct Feature(*mut c_void);

// SAFETY: an opaque handle the SDK gives; it is only used by the tracer
// that owns it, one thread at a time (as the device's own handles).
unsafe impl Send for Feature {}

impl Ngx {
    /// Loads the shim and starts NGX on `context`'s device.
    pub(crate) fn load(context: &gpu::Context) -> Result<Self, String> {
        let handles = context.ngx_handles().ok_or("this GPU has no NVIDIA Ray Reconstruction")?;
        let path = std::env::var_os("LAHZA_NGX")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".local/lib/lahza/liblahza_ngx.so")))
            .filter(|p| p.exists())
            .ok_or("the Ray Reconstruction shim is not installed (just ngx)")?;
        let dir = CString::new(path.parent().unwrap_or(std::path::Path::new(".")).to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: the shim's C functions, with the signatures it declares
        // (packaging/ngx/lahza_ngx.cpp), on this context's live handles.
        unsafe {
            let lib = libloading::Library::new(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get($name).map_err(|e| format!("{}: {e}", path.display()))?
                };
            }
            let init: unsafe extern "C" fn(u64, u64, u64, *const c_void, *const c_void, *const c_char) -> i32 = sym!(b"lahza_ngx_init\0");
            let ngx = Ngx { device: handles[2], create: sym!(b"lahza_ngx_create\0"), evaluate: sym!(b"lahza_ngx_evaluate\0"), release: sym!(b"lahza_ngx_release\0"), shutdown: sym!(b"lahza_ngx_shutdown\0"), _lib: lib };
            match init(handles[0], handles[1], handles[2], handles[3] as *const c_void, handles[4] as *const c_void, dir.as_ptr()) {
                0 => Ok(ngx),
                code => Err(format!("NVIDIA Ray Reconstruction is not available here (code {code})")),
            }
        }
    }

    /// A reconstruction from `low` to `full` texels, recorded into `cmd`.
    pub(crate) fn create(&self, cmd: u64, low: (u32, u32), full: (u32, u32)) -> Option<Feature> {
        // SAFETY: `cmd` is the encoder's open command buffer on this device.
        let handle = unsafe { (self.create)(self.device, cmd, low.0, low.1, full.0, full.1) };
        (!handle.is_null()).then_some(Feature(handle))
    }

    /// Records `frame`'s reconstruction into `cmd`.
    pub(crate) fn evaluate(&self, cmd: u64, feature: Feature, frame: &Frame) -> bool {
        // SAFETY: the feature is live; the frame's images are this device's.
        unsafe { (self.evaluate)(cmd, feature.0, frame) == 0 }
    }

    /// (Once the GPU is done with the commands that used it.)
    pub(crate) fn release(&self, feature: Feature) {
        // SAFETY: a feature `create` made, released once.
        unsafe { (self.release)(feature.0) }
    }

    /// Before the device goes.
    pub(crate) fn shutdown(&self) {
        // SAFETY: every feature was released; the device is still live.
        unsafe { (self.shutdown)(self.device) }
    }
}
