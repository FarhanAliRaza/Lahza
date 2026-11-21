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
    /// Sharing memory with the renderer's device (OIDN 2; absent in
    /// older libraries): the device's supported handle types, and a
    /// buffer over memory imported by file descriptor.
    device_int: Option<unsafe extern "C" fn(Handle, *const c_char) -> i32>,
    new_shared_from_fd: Option<unsafe extern "C" fn(Handle, i32, i32, usize) -> Handle>,
    /// Denoising without waiting for it, and waiting for it later.
    execute_async: Option<unsafe extern "C" fn(Handle)>,
    sync_device: Option<unsafe extern "C" fn(Handle)>,
}

/// `OIDN_EXTERNAL_MEMORY_TYPE_FLAG_OPAQUE_FD`: what Vulkan exports.
const EXTERNAL_OPAQUE_FD: i32 = 1;

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
                    device_int: lib.get(b"oidnGetDeviceInt\0").ok().map(|s| *s),
                    new_shared_from_fd: lib.get(b"oidnNewSharedBufferFromFD\0").ok().map(|s| *s),
                    execute_async: lib.get(b"oidnExecuteFilterAsync\0").ok().map(|s| *s),
                    sync_device: lib.get(b"oidnSyncDevice\0").ok().map(|s| *s),
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
    /// Filters over memory shared with the renderer's device (`share`).
    shared: std::collections::HashMap<u64, Shared>,
    next_shared: u64,
    /// The picture being denoised without being waited for (`start`,
    /// `start_shared`): one at a time.
    running: Option<Run>,
    /// Tests: the stand-in takes shared memory too (it reads and writes
    /// the descriptors as files, which Mesa's software Vulkan allows).
    #[cfg(test)]
    pub(crate) stand_in_shares: bool,
    /// Tests: the stand-in's shared pictures come out wrong (as memory
    /// imported from another device would).
    #[cfg(test)]
    pub(crate) stand_in_shares_badly: bool,
}

/// A picture `start` or `start_shared` began: the copied way's (the
/// caller's name for it), or a shared filter.
#[derive(Clone, Copy, PartialEq)]
enum Run {
    Copied(u64),
    Shared(u64),
}

/// A filter whose input and output are the renderer's own device memory,
/// imported by file descriptor: nothing is copied to denoise.
struct Shared {
    size: (usize, usize),
    filter: Handle,
    buffers: [Handle; 2],
    /// The stand-in's view of the two memories (tests).
    files: Option<[std::fs::File; 2]>,
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
            let d = Denoiser {
                api: Some(api),
                device,
                sized: None,
                runs: 0,
                shared: Default::default(),
                next_shared: 0,
                running: None,
                #[cfg(test)]
                stand_in_shares: false,
                #[cfg(test)]
                stand_in_shares_badly: false,
            };
            d.check()?;
            Ok(d)
        }
    }

    /// Tests: no library; `denoise` returns the colour unchanged, so what
    /// leads to it and back can be checked without OIDN.
    #[cfg(test)]
    pub(crate) fn pass_through() -> Self {
        Denoiser { api: None, device: std::ptr::null_mut(), sized: None, runs: 0, shared: Default::default(), next_shared: 0, running: None, stand_in_shares: false, stand_in_shares_badly: false }
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

    /// Waits for the picture begun with `start` or `start_shared`.
    fn wait(&mut self) {
        if let (Some(_), Some(api)) = (self.running.take(), &self.api) {
            if let Some(sync) = api.sync_device {
                // SAFETY: the device is live.
                unsafe { sync(self.device) };
            }
        }
    }

    /// Begins filter `filter` without waiting for it, where the library
    /// can and nothing else is under way.
    fn begin(&mut self, filter: Handle, run: Run) -> bool {
        let Some(api) = &self.api else { return false };
        let (Some(execute), Some(_), None) = (api.execute_async, api.sync_device, self.running) else { return false };
        // SAFETY: a committed filter of this device; its images are not
        // touched until `wait`.
        unsafe { execute(filter) };
        self.running = Some(run);
        true
    }

    fn release_sized(&mut self) {
        self.wait();
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
        // (A picture begun with `start` and not taken finishes first.)
        self.wait();
        self.size(w, h)?;
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

    /// Begins denoising a `w × h` picture (`input` as `denoise` takes it)
    /// without waiting; `picture` names it for `take`. False when it was
    /// not begun (`denoise` then does all of it): a library without the
    /// calls, another picture under way, the stand-in.
    pub fn start(&mut self, picture: u64, w: usize, h: usize, input: &[f32]) -> bool {
        if self.api.is_none() || self.running.is_some() || input.len() != w * h * INPUT_FLOATS || self.size(w, h).is_err() {
            return false;
        }
        let api = self.api.as_ref().unwrap();
        let s = self.sized.as_ref().unwrap();
        // SAFETY: the buffer holds the input's bytes; the slice is that long.
        unsafe { (api.write_buffer)(s.buffers[0], 0, input.len() * 4, input.as_ptr() as *const c_void) };
        let filter = s.filter;
        self.begin(filter, Run::Copied(picture))
    }

    /// Waits for the picture `start` began as `picture` and takes its
    /// colour (three floats a pixel). None when that picture is not the
    /// one under way: `denoise` it instead.
    pub fn take(&mut self, picture: u64, output: &mut [f32]) -> Option<Result<(), String>> {
        if self.running != Some(Run::Copied(picture)) {
            return None;
        }
        self.wait();
        self.runs += 1;
        let (api, s) = (self.api.as_ref()?, self.sized.as_ref()?);
        if output.len() != s.size.0 * s.size.1 * 3 {
            return Some(Err("denoise: image sizes differ".into()));
        }
        // SAFETY: the output buffer holds that many bytes; the slice is
        // that long.
        unsafe { (api.read_buffer)(s.buffers[1], 0, output.len() * 4, output.as_mut_ptr() as *mut c_void) };
        Some(self.check())
    }

    /// `start` for shared filter `id` (`denoise_shared` then waits for it).
    pub fn start_shared(&mut self, id: u64) -> bool {
        match self.shared.get(&id).map(|s| s.filter) {
            Some(filter) => self.begin(filter, Run::Shared(id)),
            None => false,
        }
    }

    /// Makes `sized` the filter and buffers for `w × h` pictures.
    fn size(&mut self, w: usize, h: usize) -> Result<(), String> {
        let (in_bytes, out_bytes) = (w * h * INPUT_FLOATS * 4, w * h * 12);
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
        Ok(())
    }
}

impl Denoiser {
    /// Whether the device can denoise memory the renderer exports by file
    /// descriptor (OIDN on the GPU; its CPU device cannot).
    pub fn shares_fd(&self) -> bool {
        match &self.api {
            // SAFETY: the device is live; the name is a C string.
            Some(api) => api.new_shared_from_fd.is_some() && api.device_int.is_some_and(|f| unsafe { f(self.device, c"externalMemoryTypes".as_ptr()) } & EXTERNAL_OPAQUE_FD != 0),
            #[cfg(test)]
            None => self.stand_in_shares,
            #[cfg(not(test))]
            None => false,
        }
    }

    /// A filter for `w × h` pictures over two memories the renderer
    /// exported as file descriptors: `input` (colour, albedo and normal,
    /// `INPUT_FLOATS` floats a pixel) and `output` (three). On success
    /// the descriptors are the denoiser's; on failure they are closed.
    /// Returns what `denoise_shared` and `release_shared` name it by.
    pub fn share(&mut self, w: usize, h: usize, input: i32, output: i32) -> Result<u64, String> {
        use std::os::fd::FromRawFd;
        let n = w * h;
        let (in_bytes, out_bytes) = (n * INPUT_FLOATS * 4, n * 12);
        // SAFETY: descriptors the caller hands over, owned from here.
        let close = |fd: i32| drop(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
        let shared = match &self.api {
            Some(api) => {
                let Some(import) = api.new_shared_from_fd else {
                    close(input);
                    close(output);
                    return Err("this Open Image Denoise cannot share memory".into());
                };
                // SAFETY: OIDN C API calls on this device; names are C strings.
                unsafe {
                    let a = import(self.device, EXTERNAL_OPAQUE_FD, input, in_bytes);
                    if a.is_null() {
                        close(input);
                        close(output);
                        return Err(self.check().err().unwrap_or_else(|| "Open Image Denoise did not take the shared input".into()));
                    }
                    let b = import(self.device, EXTERNAL_OPAQUE_FD, output, out_bytes);
                    if b.is_null() {
                        (api.release_buffer)(a);
                        close(output);
                        return Err(self.check().err().unwrap_or_else(|| "Open Image Denoise did not take the shared output".into()));
                    }
                    let filter = (api.new_filter)(self.device, c"RT".as_ptr());
                    let stride = INPUT_FLOATS * 4;
                    let image = |name: &CStr, buffer: Handle, offset: usize, stride: usize| (api.set_filter_image)(filter, name.as_ptr(), buffer, FORMAT_FLOAT3, w, h, offset, stride, stride * w);
                    image(c"color", a, 0, stride);
                    image(c"albedo", a, 12, stride);
                    image(c"normal", a, 24, stride);
                    image(c"output", b, 0, 12);
                    (api.set_filter_bool)(filter, c"hdr".as_ptr(), true);
                    (api.commit_filter)(filter);
                    let shared = Shared { size: (w, h), filter, buffers: [a, b], files: None };
                    if let Err(e) = self.check() {
                        (api.release_filter)(filter);
                        (api.release_buffer)(a);
                        (api.release_buffer)(b);
                        return Err(e);
                    }
                    shared
                }
            }
            // SAFETY: as `close`: the files own the descriptors.
            None => Shared { size: (w, h), filter: std::ptr::null_mut(), buffers: [std::ptr::null_mut(); 2], files: Some(unsafe { [std::fs::File::from_raw_fd(input), std::fs::File::from_raw_fd(output)] }) },
        };
        self.next_shared += 1;
        self.shared.insert(self.next_shared, shared);
        Ok(self.next_shared)
    }

    /// Denoises shared filter `id`'s input into its output, in place on
    /// the device. The renderer's work on both memories must be done.
    pub fn denoise_shared(&mut self, id: u64) -> Result<(), String> {
        // Begun with `start_shared`: wait for it.
        let started = self.running == Some(Run::Shared(id));
        self.wait();
        let s = self.shared.get(&id).ok_or("no such shared filter")?;
        self.runs += 1;
        match (&self.api, &s.files) {
            (Some(api), _) => {
                if !started {
                    // SAFETY: a committed filter of this device.
                    unsafe { (api.execute_filter)(s.filter) };
                }
                self.check()
            }
            (None, Some(_)) => {
                let (mut input, mut output) = (vec![0f32; s.size.0 * s.size.1 * INPUT_FLOATS], vec![0f32; s.size.0 * s.size.1 * 3]);
                self.read_shared(id, &mut input, &mut [])?;
                for (o, i) in output.chunks_exact_mut(3).zip(input.chunks_exact(INPUT_FLOATS)) {
                    o.copy_from_slice(&i[..3]);
                }
                #[cfg(test)]
                if self.stand_in_shares_badly {
                    output.iter_mut().for_each(|v| *v = 1.0 - *v);
                }
                use std::os::unix::fs::FileExt;
                let s = self.shared.get(&id).ok_or("no such shared filter")?;
                let file = &s.files.as_ref().unwrap()[1];
                file.write_all_at(bytemuck::cast_slice(&output), stand_in_offset(file, output.len() * 4)).map_err(|e| format!("shared output: {e}"))
            }
            (None, None) => Err("no denoiser".into()),
        }
    }

    /// Copies shared filter `id`'s input and output to the host (either
    /// may be empty: not read): to check the shared way against the
    /// copied one.
    pub fn read_shared(&self, id: u64, input: &mut [f32], output: &mut [f32]) -> Result<(), String> {
        let s = self.shared.get(&id).ok_or("no such shared filter")?;
        match (&self.api, &s.files) {
            (Some(api), _) => {
                // SAFETY: the buffers hold at least these many bytes (the
                // slices are checked against the filter's size).
                unsafe {
                    if !input.is_empty() {
                        assert_eq!(input.len(), s.size.0 * s.size.1 * INPUT_FLOATS);
                        (api.read_buffer)(s.buffers[0], 0, input.len() * 4, input.as_mut_ptr() as *mut c_void);
                    }
                    if !output.is_empty() {
                        assert_eq!(output.len(), s.size.0 * s.size.1 * 3);
                        (api.read_buffer)(s.buffers[1], 0, output.len() * 4, output.as_mut_ptr() as *mut c_void);
                    }
                }
                self.check()
            }
            (None, Some(files)) => {
                use std::os::unix::fs::FileExt;
                for (file, to) in files.iter().zip([input, output]) {
                    if !to.is_empty() {
                        let at = stand_in_offset(file, to.len() * 4);
                        file.read_exact_at(bytemuck::cast_slice_mut(to), at).map_err(|e| format!("shared memory: {e}"))?;
                    }
                }
                Ok(())
            }
            (None, None) => Err("no denoiser".into()),
        }
    }

    /// Lets go of shared filter `id` (before its memories are freed).
    pub fn release_shared(&mut self, id: u64) {
        self.wait();
        if let (Some(s), Some(api)) = (self.shared.remove(&id), &self.api) {
            // SAFETY: handles from this device, not used after this.
            unsafe {
                (api.release_filter)(s.filter);
                for b in s.buffers {
                    (api.release_buffer)(b);
                }
            }
        }
    }
}

/// The stand-in's way into memory exported by Mesa's software Vulkan,
/// the one device it is used with: the descriptor is a memory file whose
/// first page (and a little more) is the allocator's own, the memory
/// after it.
fn stand_in_offset(file: &std::fs::File, bytes: usize) -> u64 {
    let len = file.metadata().map_or(0, |m| m.len());
    len.saturating_sub(bytes as u64) / 4096 * 4096
}

impl Drop for Denoiser {
    fn drop(&mut self) {
        let ids: Vec<u64> = self.shared.keys().copied().collect();
        for id in ids {
            self.release_shared(id);
        }
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
