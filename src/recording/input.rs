use super::{
    model::{
        KeystrokeEvent, PointerCaptureFile, PointerPressEvent, PointerTravelKind,
        PointerTravelSample, PressPhase,
    },
    pointer::{sanitize_pointer_capture, PointerSanitizeOptions},
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    ffi::{c_char, c_int, c_uint, c_ulong, c_void, CString},
    fs,
    io::{self, BufWriter, Write},
    os::fd::{FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    ptr,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const CONTROL_FILE: &str = "screendrop-input-control.json";

#[derive(Clone, Copy, Debug)]
pub struct ActiveRange {
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct InputMapping {
    pub origin: Option<(f64, f64)>,
    pub size: (f64, f64),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlRequest {
    id: String,
    event_path: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    mono_us: u64,
    kind: String,
    x: Option<f64>,
    y: Option<f64>,
    button: Option<u8>,
    phase: Option<String>,
    key: Option<String>,
    #[serde(default)]
    modifiers: Vec<String>,
    window: Option<[f64; 4]>,
}

pub struct InputCapture {
    id: String,
    control_path: PathBuf,
    event_path: PathBuf,
    active: bool,
    extension_active: bool,
    x11_poller: Option<X11Poller>,
    pipewire_poller: Option<PipeWirePoller>,
    expects_pipewire_metadata: bool,
}

impl InputCapture {
    pub fn start(project_directory: &Path) -> Self {
        let id = Uuid::new_v4().simple().to_string();
        let event_path = project_directory.join("input.raw.jsonl");
        let control_path = runtime_directory().join(CONTROL_FILE);
        let request = ControlRequest {
            id: id.clone(),
            event_path: event_path.clone(),
        };
        let extension_active = serde_json::to_vec(&request)
            .ok()
            .and_then(|bytes| fs::write(&control_path, bytes).ok())
            .is_some()
            && wait_until_ready(&event_path, Duration::from_millis(450));
        if !extension_active {
            remove_owned_control(&control_path, &id);
        }
        let wayland = std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let x11_poller = (!extension_active && !wayland)
            .then(|| X11Poller::start(event_path.clone()))
            .flatten();
        let expects_pipewire_metadata = !extension_active && wayland;
        let active = extension_active || x11_poller.is_some() || expects_pipewire_metadata;
        Self {
            id,
            control_path,
            event_path,
            active,
            extension_active,
            x11_poller,
            pipewire_poller: None,
            expects_pipewire_metadata,
        }
    }

    pub fn uses_pipewire_metadata(&self) -> bool {
        self.expects_pipewire_metadata
    }

    pub fn attach_pipewire(
        &mut self,
        remote_fd: i32,
        node: u32,
        mapping: InputMapping,
    ) -> Result<(), String> {
        if !self.expects_pipewire_metadata {
            return Ok(());
        }
        self.pipewire_poller = Some(PipeWirePoller::start(
            self.event_path.clone(),
            remote_fd,
            node,
            mapping.origin.unwrap_or((0.0, 0.0)),
        )?);
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn finish(
        mut self,
        ranges: &[ActiveRange],
        mapping: InputMapping,
        destination: &Path,
    ) -> io::Result<bool> {
        if !self.active {
            return Ok(false);
        }
        if self.extension_active {
            remove_owned_control(&self.control_path, &self.id);
            thread::sleep(Duration::from_millis(140));
        }
        if let Some(poller) = self.x11_poller.take() {
            poller.stop();
        }
        if let Some(poller) = self.pipewire_poller.take() {
            poller.stop();
        }
        self.active = false;
        let input = fs::read_to_string(&self.event_path)?;
        let mut capture = PointerCaptureFile::default();
        for line in input.lines() {
            let Ok(event) = serde_json::from_str::<RawEvent>(line) else {
                continue;
            };
            let Some(time) = map_time(event.mono_us.saturating_mul(1_000), ranges) else {
                continue;
            };
            match event.kind.as_str() {
                "move" | "drag" => {
                    let Some((x, y)) = normalized_point(&event, mapping) else {
                        continue;
                    };
                    capture.travel.push(PointerTravelSample {
                        time,
                        x,
                        y,
                        kind: if event.kind == "drag" {
                            PointerTravelKind::Drag
                        } else {
                            PointerTravelKind::Move
                        },
                        artwork_id: None,
                    });
                }
                "button" => {
                    let Some((x, y)) = normalized_point(&event, mapping) else {
                        continue;
                    };
                    let Some(phase) = event.phase.as_deref().and_then(|phase| match phase {
                        "down" => Some(PressPhase::Down),
                        "up" => Some(PressPhase::Up),
                        _ => None,
                    }) else {
                        continue;
                    };
                    capture.presses.push(PointerPressEvent {
                        time,
                        x,
                        y,
                        button: event.button.unwrap_or(1).saturating_sub(1),
                        phase,
                        artwork_id: None,
                    });
                }
                "key" => {
                    if let Some(key) = event.key.filter(|key| !key.is_empty()) {
                        capture.keystrokes.push(KeystrokeEvent {
                            time,
                            modifiers: event.modifiers,
                            key,
                        });
                    }
                }
                _ => {}
            }
        }
        let sanitized = sanitize_pointer_capture(
            capture,
            PointerSanitizeOptions::for_recording(mapping.size.0, mapping.size.1),
        )
        .sanitized_capture;
        let has_pointer_motion = !sanitized.travel.is_empty();
        let result = write_json_atomic(destination, &sanitized);
        let _ = fs::remove_file(&self.event_path);
        result.map(|_| has_pointer_motion)
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        if self.active {
            remove_owned_control(&self.control_path, &self.id);
        }
        if let Some(poller) = self.x11_poller.take() {
            poller.stop();
        }
        if let Some(poller) = self.pipewire_poller.take() {
            poller.stop();
        }
    }
}

pub fn monotonic_ns() -> u64 {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value);
    }
    (value.tv_sec.max(0) as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(value.tv_nsec.max(0) as u64)
}

fn runtime_directory() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })))
}

fn wait_until_ready(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if fs::read_to_string(path)
            .map(|contents| contents.contains("\"kind\":\"ready\""))
            .unwrap_or(false)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn remove_owned_control(path: &Path, id: &str) {
    let owned = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ControlRequest>(&bytes).ok())
        .is_some_and(|request| request.id == id);
    if owned {
        let _ = fs::remove_file(path);
    }
}

fn map_time(event_ns: u64, ranges: &[ActiveRange]) -> Option<f64> {
    let mut preceding_ns = 0_u64;
    for range in ranges {
        if event_ns >= range.start_ns && event_ns <= range.end_ns {
            return Some(
                preceding_ns.saturating_add(event_ns - range.start_ns) as f64 / 1_000_000_000.0,
            );
        }
        preceding_ns = preceding_ns.saturating_add(range.end_ns.saturating_sub(range.start_ns));
    }
    None
}

fn normalized_point(event: &RawEvent, mapping: InputMapping) -> Option<(f64, f64)> {
    let (x, y) = (event.x?, event.y?);
    let (origin_x, origin_y, width, height) = match mapping.origin {
        Some((origin_x, origin_y)) => (origin_x, origin_y, mapping.size.0, mapping.size.1),
        None => event
            .window
            .map(|window| (window[0], window[1], window[2], window[3]))?,
    };
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(((x - origin_x) / width, (y - origin_y) / height))
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

struct PipeWirePoller {
    stop: Arc<AtomicBool>,
    worker: thread::JoinHandle<()>,
}

struct PipeWireEventWriter {
    writer: BufWriter<fs::File>,
    pointer: Option<(f64, f64)>,
    last_written: Option<(f64, f64)>,
    last_buttons: u8,
    last_heartbeat_us: u64,
}

// Mutter includes the cursor bitmap in SPA_META_Cursor even though Screendrop
// currently uses only its position.  Allocating just `spa_meta_cursor` causes
// Mutter to omit the metadata altogether because the advertised buffer is too
// small.  Match the allocation strategy used by PipeWire's video examples and
// OBS, with room for a large HiDPI cursor.
const MAX_CURSOR_BITMAP_EDGE: usize = 384;

fn cursor_meta_size() -> i32 {
    (std::mem::size_of::<libspa_sys::spa_meta_cursor>()
        + std::mem::size_of::<libspa_sys::spa_meta_bitmap>()
        + MAX_CURSOR_BITMAP_EDGE * MAX_CURSOR_BITMAP_EDGE * 4) as i32
}

impl PipeWirePoller {
    fn start(
        event_path: PathBuf,
        remote_fd: i32,
        node: u32,
        origin: (f64, f64),
    ) -> Result<Self, String> {
        let duplicated_fd = unsafe { libc::fcntl(remote_fd, libc::F_DUPFD_CLOEXEC, 3) };
        if duplicated_fd < 0 {
            return Err(format!(
                "could not duplicate the PipeWire portal connection: {}",
                io::Error::last_os_error()
            ));
        }
        let remote = unsafe { OwnedFd::from_raw_fd(duplicated_fd) };
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("screendrop-wayland-pointer".into())
            .spawn(move || {
                poll_pipewire_cursor(event_path, remote, node, origin, worker_stop, ready_tx)
            })
            .map_err(|error| format!("could not start Wayland pointer capture: {error}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self { stop, worker }),
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err("PipeWire cursor metadata did not become ready".into())
            }
        }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.worker.join();
    }
}

fn poll_pipewire_cursor(
    event_path: PathBuf,
    remote: OwnedFd,
    node: u32,
    origin: (f64, f64),
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let result = (|| -> Result<(), String> {
        pipewire::init();
        let file = fs::File::create(&event_path)
            .map_err(|error| format!("could not create pointer event stream: {error}"))?;
        let state = Arc::new(std::sync::Mutex::new(PipeWireEventWriter {
            writer: BufWriter::new(file),
            pointer: None,
            last_written: None,
            last_buttons: 0,
            last_heartbeat_us: 0,
        }));
        {
            let mut state = state.lock().expect("PipeWire pointer writer poisoned");
            write_raw_event(
                &mut state.writer,
                RawEvent {
                    mono_us: monotonic_ns() / 1_000,
                    kind: "ready".into(),
                    x: None,
                    y: None,
                    button: None,
                    phase: None,
                    key: None,
                    modifiers: Vec::new(),
                    window: None,
                },
            )
            .map_err(|error| error.to_string())?;
            state.writer.flush().map_err(|error| error.to_string())?;
        }

        let mainloop = pipewire::main_loop::MainLoopRc::new(None)
            .map_err(|error| format!("could not create PipeWire loop: {error}"))?;
        let context = pipewire::context::ContextRc::new(&mainloop, None)
            .map_err(|error| format!("could not create PipeWire context: {error}"))?;
        let core = context
            .connect_fd_rc(remote, None)
            .map_err(|error| format!("could not connect to the portal PipeWire remote: {error}"))?;
        let stream = pipewire::stream::StreamBox::new(
            &core,
            "Screendrop cursor metadata",
            pipewire::properties::properties! {
                *pipewire::keys::MEDIA_TYPE => "Video",
                *pipewire::keys::MEDIA_CATEGORY => "Capture",
                *pipewire::keys::MEDIA_ROLE => "Screen",
            },
        )
        .map_err(|error| format!("could not create PipeWire cursor stream: {error}"))?;

        let ready_slot = Rc::new(RefCell::new(Some(ready)));
        let listener_state = state.clone();
        let ready_for_state = ready_slot.clone();
        let _listener = stream
            .add_local_listener::<()>()
            .state_changed(move |_, _, _old, new| match new {
                pipewire::stream::StreamState::Streaming => {
                    if let Some(sender) = ready_for_state.borrow_mut().take() {
                        let _ = sender.send(Ok(()));
                    }
                }
                pipewire::stream::StreamState::Error(error) => {
                    if let Some(sender) = ready_for_state.borrow_mut().take() {
                        let _ = sender.send(Err(format!("PipeWire cursor stream failed: {error}")));
                    }
                }
                _ => {}
            })
            .param_changed(|stream, _, id, param| {
                if param.is_none() || id != pipewire::spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let object = pipewire::spa::pod::Object {
                    type_: pipewire::spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
                    id: pipewire::spa::param::ParamType::Meta.as_raw(),
                    properties: vec![
                        pipewire::spa::pod::Property::new(
                            libspa_sys::SPA_PARAM_META_type,
                            pipewire::spa::pod::Value::Id(pipewire::spa::utils::Id(
                                libspa_sys::SPA_META_Cursor,
                            )),
                        ),
                        pipewire::spa::pod::Property::new(
                            libspa_sys::SPA_PARAM_META_size,
                            pipewire::spa::pod::Value::Int(cursor_meta_size()),
                        ),
                    ],
                };
                let Ok((bytes, _)) = pipewire::spa::pod::serialize::PodSerializer::serialize(
                    std::io::Cursor::new(Vec::new()),
                    &pipewire::spa::pod::Value::Object(object),
                ) else {
                    return;
                };
                let bytes = bytes.into_inner();
                let Some(pod) = pipewire::spa::pod::Pod::from_bytes(&bytes) else {
                    return;
                };
                let _ = stream.update_params(&mut [pod]);
            })
            .process(move |stream, _| {
                let raw = unsafe { stream.dequeue_raw_buffer() };
                if raw.is_null() {
                    return;
                }
                unsafe {
                    let spa_buffer = (*raw).buffer;
                    if !spa_buffer.is_null() {
                        let cursor = libspa_sys::spa_buffer_find_meta_data(
                            spa_buffer,
                            libspa_sys::SPA_META_Cursor,
                            std::mem::size_of::<libspa_sys::spa_meta_cursor>(),
                        )
                            as *const libspa_sys::spa_meta_cursor;
                        // `spa_meta_cursor_is_valid` is a static inline in the
                        // SPA headers and is not emitted by every bindgen
                        // build; its definition is simply a non-zero id.
                        if !cursor.is_null() && (*cursor).id != 0 {
                            record_pipewire_position(
                                &listener_state,
                                origin.0 + f64::from((*cursor).position.x),
                                origin.1 + f64::from((*cursor).position.y),
                            );
                        }
                    }
                    stream.queue_raw_buffer(raw);
                }
            })
            .register()
            .map_err(|error| format!("could not listen to PipeWire cursor stream: {error}"))?;

        let mouse_state = state.clone();
        let mouse_stop = stop.clone();
        let mouse_worker = thread::Builder::new()
            .name("screendrop-wayland-buttons".into())
            .spawn(move || poll_pipewire_buttons(mouse_state, mouse_stop))
            .map_err(|error| format!("could not start pointer button capture: {error}"))?;

        let quit_loop = mainloop.clone();
        let timer_stop = stop.clone();
        let timer = mainloop.loop_().add_timer(move |_| {
            if timer_stop.load(Ordering::Acquire) {
                quit_loop.quit();
            }
        });
        let _ = timer.update_timer(
            Some(Duration::from_millis(20)),
            Some(Duration::from_millis(20)),
        );
        let format = pipewire::spa::pod::Object {
            type_: pipewire::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: pipewire::spa::param::ParamType::EnumFormat.as_raw(),
            properties: vec![
                pipewire::spa::pod::Property::new(
                    libspa_sys::SPA_FORMAT_mediaType,
                    pipewire::spa::pod::Value::Id(pipewire::spa::utils::Id(
                        pipewire::spa::param::format::MediaType::Video.as_raw(),
                    )),
                ),
                pipewire::spa::pod::Property::new(
                    libspa_sys::SPA_FORMAT_mediaSubtype,
                    pipewire::spa::pod::Value::Id(pipewire::spa::utils::Id(
                        pipewire::spa::param::format::MediaSubtype::Raw.as_raw(),
                    )),
                ),
            ],
        };
        let (format_bytes, _) = pipewire::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pipewire::spa::pod::Value::Object(format),
        )
        .map_err(|error| format!("could not describe the cursor video stream: {error}"))?;
        let format_bytes = format_bytes.into_inner();
        let format_pod = pipewire::spa::pod::Pod::from_bytes(&format_bytes)
            .ok_or_else(|| "could not create the cursor video format".to_string())?;
        stream
            .connect(
                pipewire::spa::utils::Direction::Input,
                Some(node),
                pipewire::stream::StreamFlags::AUTOCONNECT
                    | pipewire::stream::StreamFlags::MAP_BUFFERS,
                &mut [format_pod],
            )
            .map_err(|error| format!("could not connect PipeWire cursor stream: {error}"))?;
        mainloop.run();
        stop.store(true, Ordering::Release);
        let _ = mouse_worker.join();
        if let Ok(mut state) = state.lock() {
            let _ = state.writer.flush();
        }
        Ok(())
    })();
    if let Err(error) = result {
        // The receiver may already have observed a stream-state error.
        // Sending here covers failures during setup.
        // Ignore a disconnected receiver after successful startup.
        eprintln!("Screendrop Wayland pointer capture: {error}");
    }
}

fn record_pipewire_position(state: &Arc<std::sync::Mutex<PipeWireEventWriter>>, x: f64, y: f64) {
    let Ok(mut state) = state.lock() else {
        return;
    };
    let now_us = monotonic_ns() / 1_000;
    let changed = state.last_written != Some((x, y));
    let heartbeat = now_us.saturating_sub(state.last_heartbeat_us) >= 1_000_000;
    state.pointer = Some((x, y));
    if changed || heartbeat {
        let buttons = state.last_buttons;
        let _ = write_pointer_travel(&mut state.writer, now_us, x, y, buttons);
        let _ = state.writer.flush();
        state.last_written = Some((x, y));
        state.last_heartbeat_us = now_us;
    }
}

fn poll_pipewire_buttons(state: Arc<std::sync::Mutex<PipeWireEventWriter>>, stop: Arc<AtomicBool>) {
    let mouse_fd = unsafe {
        libc::open(
            c"/dev/input/mice".as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK,
        )
    };
    if mouse_fd < 0 {
        return;
    }
    let mut bytes = [0_u8; 96];
    let mut pending = Vec::with_capacity(96);
    while !stop.load(Ordering::Acquire) {
        let count = unsafe { libc::read(mouse_fd, bytes.as_mut_ptr().cast(), bytes.len()) };
        if count > 0 {
            pending.extend_from_slice(&bytes[..count as usize]);
        }
        let packet_count = pending.len() / 3;
        for packet in pending[..packet_count * 3].chunks_exact(3) {
            let buttons = mouse_buttons(packet[0]);
            let Ok(mut state) = state.lock() else {
                continue;
            };
            let changed = buttons ^ state.last_buttons;
            if changed != 0 {
                if let Some((x, y)) = state.pointer {
                    write_button_transitions(
                        &mut state.writer,
                        monotonic_ns() / 1_000,
                        x,
                        y,
                        buttons,
                        changed,
                    );
                    let _ = state.writer.flush();
                }
                state.last_buttons = buttons;
            }
        }
        if packet_count > 0 {
            pending.drain(..packet_count * 3);
        }
        thread::sleep(Duration::from_millis(2));
    }
    unsafe { libc::close(mouse_fd) };
}

struct X11Poller {
    stop: Arc<AtomicBool>,
    worker: thread::JoinHandle<()>,
}

impl X11Poller {
    fn start(event_path: PathBuf) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("screendrop-x11-pointer".into())
            .spawn(move || poll_x11_pointer(event_path, worker_stop, ready_tx))
            .ok()?;
        if ready_rx.recv_timeout(Duration::from_millis(300)).ok() != Some(true) {
            stop.store(true, Ordering::Release);
            let _ = worker.join();
            return None;
        }
        Some(Self { stop, worker })
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.worker.join();
    }
}

fn poll_x11_pointer(event_path: PathBuf, stop: Arc<AtomicBool>, ready: mpsc::Sender<bool>) {
    let display_name = std::env::var("DISPLAY")
        .ok()
        .and_then(|value| CString::new(value).ok());
    let display = unsafe {
        x_open_display(
            display_name
                .as_ref()
                .map(|value| value.as_ptr())
                .unwrap_or(ptr::null()),
        )
    };
    if display.is_null() {
        let _ = ready.send(false);
        return;
    }
    let root = unsafe { x_default_root_window(display) };
    let mouse_path = c"/dev/input/mice";
    let mouse_fd = unsafe { libc::open(mouse_path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
    if mouse_fd < 0 {
        unsafe { x_close_display(display) };
        let _ = ready.send(false);
        return;
    }
    let file = match fs::File::create(&event_path) {
        Ok(file) => file,
        Err(_) => {
            unsafe { libc::close(mouse_fd) };
            unsafe { x_close_display(display) };
            let _ = ready.send(false);
            return;
        }
    };
    let mut writer = BufWriter::new(file);
    let _ = write_raw_event(
        &mut writer,
        RawEvent {
            mono_us: monotonic_ns() / 1_000,
            kind: "ready".into(),
            x: None,
            y: None,
            button: None,
            phase: None,
            key: None,
            modifiers: Vec::new(),
            window: None,
        },
    );
    let _ = writer.flush();
    let _ = ready.send(true);

    let mut root_return = 0;
    let mut child_return = 0;
    let mut root_x = 0;
    let mut root_y = 0;
    let mut window_x = 0;
    let mut window_y = 0;
    let mut x11_state = 0;
    unsafe {
        x_query_pointer(
            display,
            root,
            &mut root_return,
            &mut child_return,
            &mut root_x,
            &mut root_y,
            &mut window_x,
            &mut window_y,
            &mut x11_state,
        );
    }
    let screen_width = unsafe { x_display_width(display, 0) }.max(1);
    let screen_height = unsafe { x_display_height(display, 0) }.max(1);
    let mut pointer_x = f64::from(root_x.clamp(0, screen_width - 1));
    let mut pointer_y = f64::from(root_y.clamp(0, screen_height - 1));
    let mut last_buttons = 0_u8;
    let mut last_heartbeat = 0_u64;
    let mut bytes = [0_u8; 192];
    let mut pending = Vec::with_capacity(192);
    while !stop.load(Ordering::Acquire) {
        let count =
            unsafe { libc::read(mouse_fd, bytes.as_mut_ptr().cast::<c_void>(), bytes.len()) };
        if count > 0 {
            pending.extend_from_slice(&bytes[..count as usize]);
        }
        let packet_count = pending.len() / 3;
        if packet_count > 0 {
            for packet in pending[..packet_count * 3].chunks_exact(3) {
                let dx = f64::from(packet[1] as i8);
                let dy = f64::from(packet[2] as i8);
                // `/dev/input/mice` reports unaccelerated device counts, not
                // desktop pixels. Accumulating those counts drifts rapidly
                // from the compositor cursor (especially with fractional
                // scaling or pointer acceleration). Use the packet only to
                // wake capture and ask XWayland for the absolute desktop
                // position; retain relative integration solely as a failure
                // fallback.
                if let Some((absolute_x, absolute_y)) =
                    query_pointer_position(display, root, screen_width, screen_height)
                {
                    pointer_x = absolute_x;
                    pointer_y = absolute_y;
                } else {
                    pointer_x = (pointer_x + dx).clamp(0.0, f64::from(screen_width - 1));
                    pointer_y = (pointer_y - dy).clamp(0.0, f64::from(screen_height - 1));
                }
                let buttons = mouse_buttons(packet[0]);
                let changed = buttons ^ last_buttons;
                let now_us = monotonic_ns() / 1_000;
                write_button_transitions(
                    &mut writer,
                    now_us,
                    pointer_x,
                    pointer_y,
                    buttons,
                    changed,
                );
                if dx != 0.0 || dy != 0.0 || changed != 0 {
                    let _ =
                        write_pointer_travel(&mut writer, now_us, pointer_x, pointer_y, buttons);
                    last_heartbeat = now_us;
                }
                last_buttons = buttons;
            }
            pending.drain(..packet_count * 3);
            let _ = writer.flush();
        }
        let now_us = monotonic_ns() / 1_000;
        if now_us.saturating_sub(last_heartbeat) >= 1_000_000 {
            let _ = write_pointer_travel(&mut writer, now_us, pointer_x, pointer_y, last_buttons);
            let _ = writer.flush();
            last_heartbeat = now_us;
        }
        thread::sleep(Duration::from_millis(4));
    }
    let _ = writer.flush();
    unsafe { libc::close(mouse_fd) };
    unsafe { x_close_display(display) };
}

fn query_pointer_position(
    display: *mut c_void,
    root: c_ulong,
    screen_width: c_int,
    screen_height: c_int,
) -> Option<(f64, f64)> {
    let mut root_return = 0;
    let mut child_return = 0;
    let mut root_x = 0;
    let mut root_y = 0;
    let mut window_x = 0;
    let mut window_y = 0;
    let mut state = 0;
    let success = unsafe {
        x_query_pointer(
            display,
            root,
            &mut root_return,
            &mut child_return,
            &mut root_x,
            &mut root_y,
            &mut window_x,
            &mut window_y,
            &mut state,
        )
    };
    (success != 0).then(|| {
        (
            f64::from(root_x.clamp(0, screen_width - 1)),
            f64::from(root_y.clamp(0, screen_height - 1)),
        )
    })
}

fn write_button_transitions(
    writer: &mut impl Write,
    now_us: u64,
    x: f64,
    y: f64,
    buttons: u8,
    changed: u8,
) {
    for button in 1..=5 {
        let bit = 1 << (button - 1);
        if changed & bit != 0 {
            let _ = write_raw_event(
                writer,
                RawEvent {
                    mono_us: now_us,
                    kind: "button".into(),
                    x: Some(x),
                    y: Some(y),
                    button: Some(button),
                    phase: Some(if buttons & bit != 0 { "down" } else { "up" }.into()),
                    key: None,
                    modifiers: Vec::new(),
                    window: None,
                },
            );
        }
    }
}

fn write_pointer_travel(
    writer: &mut impl Write,
    now_us: u64,
    x: f64,
    y: f64,
    buttons: u8,
) -> io::Result<()> {
    write_raw_event(
        writer,
        RawEvent {
            mono_us: now_us,
            kind: if buttons == 0 { "move" } else { "drag" }.into(),
            x: Some(x),
            y: Some(y),
            button: None,
            phase: None,
            key: None,
            modifiers: Vec::new(),
            window: None,
        },
    )
}

fn mouse_buttons(packet_header: u8) -> u8 {
    let mut buttons = 0_u8;
    if packet_header & 0x01 != 0 {
        buttons |= 1 << 0;
    }
    if packet_header & 0x04 != 0 {
        buttons |= 1 << 1;
    }
    if packet_header & 0x02 != 0 {
        buttons |= 1 << 2;
    }
    buttons
}

fn write_raw_event(writer: &mut impl Write, event: RawEvent) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, &event)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writer.write_all(b"\n")
}

#[link(name = "X11")]
unsafe extern "C" {
    #[link_name = "XOpenDisplay"]
    fn x_open_display(name: *const c_char) -> *mut c_void;
    #[link_name = "XDefaultRootWindow"]
    fn x_default_root_window(display: *mut c_void) -> c_ulong;
    #[link_name = "XQueryPointer"]
    fn x_query_pointer(
        display: *mut c_void,
        window: c_ulong,
        root_return: *mut c_ulong,
        child_return: *mut c_ulong,
        root_x_return: *mut c_int,
        root_y_return: *mut c_int,
        window_x_return: *mut c_int,
        window_y_return: *mut c_int,
        mask_return: *mut c_uint,
    ) -> c_int;
    #[link_name = "XCloseDisplay"]
    fn x_close_display(display: *mut c_void) -> c_int;
    #[link_name = "XDisplayWidth"]
    fn x_display_width(display: *mut c_void, screen_number: c_int) -> c_int;
    #[link_name = "XDisplayHeight"]
    fn x_display_height(display: *mut c_void, screen_number: c_int) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_gaps_are_removed_from_input_time() {
        let ranges = [
            ActiveRange {
                start_ns: 1_000,
                end_ns: 2_000,
            },
            ActiveRange {
                start_ns: 5_000,
                end_ns: 7_000,
            },
        ];
        assert_eq!(map_time(1_500, &ranges), Some(0.0000005));
        assert_eq!(map_time(3_000, &ranges), None);
        assert_eq!(map_time(6_000, &ranges), Some(0.000002));
    }

    #[test]
    fn window_mapping_uses_event_geometry() {
        let event = RawEvent {
            mono_us: 0,
            kind: "move".into(),
            x: Some(150.0),
            y: Some(250.0),
            button: None,
            phase: None,
            key: None,
            modifiers: Vec::new(),
            window: Some([100.0, 200.0, 200.0, 100.0]),
        };
        assert_eq!(
            normalized_point(
                &event,
                InputMapping {
                    origin: None,
                    size: (200.0, 100.0),
                }
            ),
            Some((0.25, 0.5))
        );
    }

    #[test]
    #[ignore = "requires a running X11 or XWayland display"]
    fn live_linux_fallback_writes_pointer_samples() {
        let event_path = std::env::temp_dir().join(format!(
            "screendrop-x11-pointer-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let poller = X11Poller::start(event_path.clone()).expect("X11 pointer polling unavailable");
        thread::sleep(Duration::from_millis(700));
        poller.stop();
        let contents = fs::read_to_string(&event_path).unwrap();
        let events: Vec<RawEvent> = contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert!(events
            .iter()
            .any(|event| { event.kind == "move" && event.x.is_some() && event.y.is_some() }));
        if std::env::var_os("SCREENDROP_REQUIRE_MOTION").is_some() {
            let mut points = events
                .iter()
                .filter(|event| event.kind == "move" || event.kind == "drag")
                .filter_map(|event| Some((event.x? as i32, event.y? as i32)));
            let first = points.next().expect("no pointer samples");
            assert!(points.any(|point| point != first), "pointer never moved");
        }
        let _ = fs::remove_file(event_path);
    }
}
