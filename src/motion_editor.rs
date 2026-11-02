//! Motion editor window: preview, transport, timeline (scenes, markers,
//! layers, keys) and an inspector for `*.lahza` motion projects.
//!
//! Every edit goes through the same `Toolbox` the MCP server uses, so a
//! person and an agent share one history and undo stack. Changes an agent
//! makes on disk appear live (the project folder is polled).
//!
//! Frames render on a worker thread that owns a motion `Renderer` (GPU
//! when available). It keeps only the newest request; playback and drags
//! render a half-size proxy, a paused frame renders at full preview size
//! with motion blur. GPU images are released with `Window::drop_image`.

use crate::controls::{button, icon_button, ButtonKind, ButtonSize};
use crate::motion_sound::{self, Player, Wave};
use crate::theme::{self, TextRoles};
use gpui::{
    canvas, div, img, prelude::*, px, size, App, Bounds, Context, FocusHandle, Hsla, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ObjectFit, Pixels, Render, RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Timer, TitlebarOptions,
    Window, WindowBounds, WindowDecorations, WindowOptions,
};
use lahza_motion_core::compile::{Compiled, Content, LayerState};
use lahza_motion_core::document::{Layer, LayerKind, Project};
use lahza_motion_core::property::{compile as compile_prop, Ctx, Property};
use lahza_motion_core::timeref::TimeRef;
use lahza_motion_core::value::{Animatable, Vec2};
use lahza_motion_core::{edit, io};
use lahza_motion_mcp::tools::Toolbox;
use lahza_motion_render::font::Fonts;
use lahza_motion_render::{RenderOptions, Renderer};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

const LABEL_WIDTH: f32 = 168.0;
const ROW: f32 = 24.0;
const INSPECTOR_WIDTH: f32 = 280.0;

// ------------------------------------------------------------ render worker

struct Job {
    compiled: Arc<Compiled>,
    t: f64,
    scale: f64,
    blur: bool,
    key: u64,
}

#[derive(Default)]
struct Mailbox {
    job: Option<Job>,
    done: Option<(u64, Arc<RenderImage>)>,
    label: String,
    quit: bool,
}

/// One thread with a motion renderer; the newest request replaces any
/// waiting one, so a busy renderer drops frames instead of queueing.
struct Worker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    fn new(dir: PathBuf) -> Self {
        let shared: Arc<(Mutex<Mailbox>, Condvar)> = Arc::default();
        let inner = shared.clone();
        let thread = std::thread::Builder::new()
            .name("lahza-motion-preview".into())
            .spawn(move || {
                let mut renderer = Renderer::new(Some(&dir), RenderOptions::default());
                inner.0.lock().unwrap().label = renderer.backend_label();
                loop {
                    let job = {
                        let mut mailbox = inner.0.lock().unwrap();
                        while mailbox.job.is_none() && !mailbox.quit {
                            mailbox = inner.1.wait(mailbox).unwrap();
                        }
                        if mailbox.quit {
                            return;
                        }
                        mailbox.job.take().unwrap()
                    };
                    renderer.set_scale(job.scale);
                    renderer.set_motion_blur(job.blur);
                    let mut pixels = renderer.render(&job.compiled, job.t);
                    // GPUI uploads BGRA.
                    for p in pixels.chunks_exact_mut(4) {
                        p.swap(0, 2);
                    }
                    let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
                    inner.0.lock().unwrap().done = Some((job.key, image));
                }
            })
            .expect("preview thread");
        Self { shared, thread: Some(thread) }
    }

    fn request(&self, job: Job) {
        self.shared.0.lock().unwrap().job = Some(job);
        self.shared.1.notify_one();
    }

    fn take(&self) -> Option<(u64, Arc<RenderImage>)> {
        self.shared.0.lock().unwrap().done.take()
    }

    fn label(&self) -> String {
        self.shared.0.lock().unwrap().label.clone()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap().quit = true;
        self.shared.1.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ------------------------------------------------------------------ model

#[derive(Clone)]
enum Drag {
    Scrub,
    Key { layer: String, path: String, index: usize, from: f64 },
    Move { layer: String, start: (f32, f32), origin: Vec2 },
    Value { layer: String, field: Field, start_x: f32, origin: f64 },
}

/// A scrubbable inspector value: property path and vector component.
#[derive(Clone, Copy, PartialEq)]
struct Field {
    path: &'static str,
    component: Option<usize>,
    label: &'static str,
    /// Value change per dragged pixel.
    step: f64,
}

const FIELDS: &[Field] = &[
    Field { path: "transform.position", component: Some(0), label: "X", step: 1.0 },
    Field { path: "transform.position", component: Some(1), label: "Y", step: 1.0 },
    Field { path: "transform.scale", component: Some(0), label: "Scale X", step: 0.005 },
    Field { path: "transform.scale", component: Some(1), label: "Scale Y", step: 0.005 },
    Field { path: "transform.rotation", component: None, label: "Rotation", step: 0.5 },
    Field { path: "transform.opacity", component: None, label: "Opacity", step: 0.005 },
    Field { path: "transform.z", component: None, label: "Z", step: 2.0 },
    Field { path: "transform.rotation_x", component: None, label: "Tilt X", step: 0.5 },
    Field { path: "transform.rotation_y", component: None, label: "Turn Y", step: 0.5 },
];

struct KeyMark {
    path: String,
    index: usize,
    t: f64,
}

struct Row {
    id: String,
    kind: &'static str,
    depth: usize,
    start: f64,
    end: f64,
    keys: Vec<KeyMark>,
}

pub(crate) struct MotionEditor {
    dir: PathBuf,
    toolbox: Toolbox,
    project: Project,
    compiled: Arc<Compiled>,
    rows: Vec<Row>,
    stamp: SystemTime,
    time: f64,
    playing: bool,
    generation: Arc<AtomicU64>,
    worker: Worker,
    frame: Option<Arc<RenderImage>>,
    retired: Vec<Arc<RenderImage>>,
    request_key: u64,
    dirty: bool,
    selected: Option<String>,
    /// Timeline pixels per second and left edge (seconds).
    zoom: f64,
    scroll: f64,
    track_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    canvas_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    drag: Option<Drag>,
    focus: FocusHandle,
    fonts: Fonts,
    status: String,
    poll_ticks: u32,
    /// Playhead in seconds (`f64::to_bits`), read by the sound thread.
    clock: Arc<AtomicU64>,
    /// Plays the project's sound while the editor plays.
    player: Option<Player>,
    sound_error: Arc<Mutex<Option<String>>>,
    waves: Arc<Vec<Wave>>,
    wave_signature: String,
    wave_inbox: Arc<Mutex<Option<(String, Vec<Wave>)>>>,
    /// Shown inside the studio window, with a way back to the video.
    pub(crate) embedded: bool,
    /// The recording under an over-the-video piece while it is edited.
    guide: Option<Guide>,
}

/// A reference picture under the design: never saved, never selectable.
#[derive(Clone, Debug)]
pub(crate) struct Guide {
    /// Footage path as seen from the project (`../../screen.mkv`).
    pub src: String,
    /// Footage time at the project's time zero.
    pub source_start: f64,
}

const GUIDE_ID: &str = "__guide";

/// Compiles `project`, with `guide` as a muted footage layer under every
/// scene when there is one.
fn compile_with_guide(project: &Project, guide: Option<&Guide>) -> Compiled {
    let Some(guide) = guide else { return Compiled::new(project) };
    let plain = Compiled::new(project);
    let mut with = project.clone();
    let (w, h) = (project.settings.width as f64, project.settings.height as f64);
    for (scene, c) in with.scenes.iter_mut().zip(&plain.scenes) {
        let layer = json!({"id": GUIDE_ID, "kind": "video", "src": guide.src, "size": [w, h], "mute": true,
            "trim": round(guide.source_start + c.start), "transform": {"position": [w / 2.0, h / 2.0]}});
        if let Ok(layer) = serde_json::from_value::<Layer>(layer) {
            scene.layers.insert(0, layer);
        }
    }
    Compiled::new(&with)
}

/// Emitted by an embedded editor's "Back to video".
pub(crate) struct CloseSpace;

impl gpui::EventEmitter<CloseSpace> for MotionEditor {}

/// Newest modification time of a motion project's files.
pub(crate) fn stamp(dir: &Path) -> SystemTime {
    let mut latest = SystemTime::UNIX_EPOCH;
    let mut look = |p: &Path| {
        if let Ok(m) = std::fs::metadata(p).and_then(|m| m.modified()) {
            latest = latest.max(m);
        }
    };
    look(&dir.join("project.json"));
    for sub in ["scenes", "markers"] {
        if let Ok(rd) = std::fs::read_dir(dir.join(sub)) {
            for e in rd.flatten() {
                look(&e.path());
            }
        }
    }
    latest
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn find_layer<'a>(project: &'a Project, id: &str) -> Option<&'a Layer> {
    fn walk<'a>(layers: &'a [Layer], id: &str) -> Option<&'a Layer> {
        layers.iter().find_map(|l| if l.id == id { Some(l) } else { walk(l.children(), id) })
    }
    project.scenes.iter().find_map(|s| walk(&s.layers, id))
}

/// Paths and indices of every key in a layer's JSON (properties, morphs and
/// text animators), with their times resolved.
fn key_marks(layer: &Layer, compiled: &Compiled, project: &Project) -> Vec<KeyMark> {
    let mut out = Vec::new();
    let v = serde_json::to_value(layer).unwrap_or_default();
    fn walk(v: &Value, path: &str, out: &mut Vec<(String, usize, Value)>) {
        if let Value::Object(m) = v {
            if let Some(Value::Array(keys)) = m.get("keys") {
                for (i, k) in keys.iter().enumerate() {
                    if let Some(at) = k.get("at").or(k.get("t")) {
                        out.push((path.trim_start_matches('.').to_string(), i, at.clone()));
                    }
                }
                return;
            }
            for (k, child) in m {
                if k == "layers" {
                    continue;
                }
                walk(child, &format!("{path}.{k}"), out);
            }
        } else if let Value::Array(items) = v {
            for (i, item) in items.iter().enumerate() {
                if path.ends_with("animators") {
                    if let Some(at) = item.get("at") {
                        out.push((format!("{}.{i}", path.trim_start_matches('.')), usize::MAX, at.clone()));
                        continue;
                    }
                }
                walk(item, &format!("{path}.{i}"), out);
            }
        }
    }
    let mut raw = Vec::new();
    walk(&v, "", &mut raw);
    for (path, index, at) in raw {
        let Ok(r) = serde_json::from_value::<TimeRef>(at) else { continue };
        let mut elements: &Compiled = compiled;
        if let Ok(t) = r.resolve(&project.markers, &compiled.base, &mut elements) {
            out.push(KeyMark { path, index, t });
        }
    }
    out
}

fn layer_rows(project: &Project, compiled: &Compiled) -> Vec<Row> {
    fn walk(layers: &[Layer], depth: usize, project: &Project, compiled: &Compiled, out: &mut Vec<Row>) {
        for l in layers {
            let (start, end) = compiled.element_window(&l.id).unwrap_or((0.0, 0.0));
            out.push(Row { id: l.id.clone(), kind: l.kind.name(), depth, start, end, keys: key_marks(l, compiled, project) });
            walk(l.children(), depth + 1, project, compiled, out);
        }
    }
    let mut out = Vec::new();
    for s in &project.scenes {
        walk(&s.layers, 0, project, compiled, &mut out);
    }
    out
}

/// The layer state with `id` at this frame (searching inside groups), with
/// its matrix made absolute.
fn find_state(layers: &[LayerState], id: &str) -> Option<LayerState> {
    for l in layers {
        if &*l.id == id {
            return Some(l.clone());
        }
        if let Content::Group(children) = &l.content {
            if let Some(mut c) = find_state(children, id) {
                c.matrix = l.matrix.then(c.matrix);
                return Some(c);
            }
        }
    }
    None
}

fn all_states(layers: &[LayerState], parent: lahza_motion_core::space::Proj, out: &mut Vec<LayerState>) {
    for l in layers {
        let mut l = l.clone();
        l.matrix = parent.then(l.matrix);
        if let Content::Group(children) = &l.content {
            all_states(children, l.matrix, out);
        }
        out.push(l);
    }
}

impl MotionEditor {
    pub(crate) fn new(dir: PathBuf, cx: &mut Context<Self>) -> Result<Self, String> {
        let project = io::load(&dir).map_err(|e| e.to_string())?;
        let compiled = Arc::new(Compiled::new(&project));
        let rows = layer_rows(&project, &compiled);
        let mut toolbox = Toolbox::new();
        toolbox.cwd = dir.parent().map(Path::to_path_buf).unwrap_or_default();
        toolbox.call("project_open", json!({"path": dir})).map_err(|e| e.to_string())?;
        let duration = project.settings.duration.max(0.1);
        let editor = Self {
            worker: Worker::new(dir.clone()),
            fonts: Fonts::new(Some(&dir)),
            stamp: stamp(&dir),
            dir,
            toolbox,
            project,
            compiled,
            rows,
            time: 0.0,
            playing: false,
            generation: Arc::default(),
            frame: None,
            retired: Vec::new(),
            request_key: 0,
            dirty: true,
            selected: None,
            zoom: 900.0 / duration,
            scroll: 0.0,
            track_bounds: Arc::default(),
            canvas_bounds: Arc::default(),
            drag: None,
            focus: cx.focus_handle(),
            status: String::new(),
            poll_ticks: 0,
            clock: Arc::default(),
            player: None,
            sound_error: Arc::default(),
            waves: Arc::default(),
            wave_signature: String::new(),
            wave_inbox: Arc::default(),
            embedded: false,
            guide: None,
        };
        let mut editor = editor;
        editor.refresh_waves();
        Ok(editor)
    }

    /// Releases this editor's GPU images (call when it goes away).
    pub(crate) fn release_images(&mut self, window: &mut Window) {
        self.player = None;
        for image in self.retired.drain(..).chain(self.frame.take()) {
            let _ = window.drop_image(image);
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Shows `guide` under the design (over-the-video pieces).
    pub(crate) fn set_guide(&mut self, guide: Option<Guide>) {
        self.guide = guide;
        self.compiled = Arc::new(compile_with_guide(&self.project, self.guide.as_ref()));
        self.dirty = true;
    }

    /// Ticks at 60 Hz: playback, finished frames, and live reload.
    pub(crate) fn start_ticking(&mut self, cx: &mut Context<Self>) {
        let mut last = Instant::now();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(16)).await;
            let now = Instant::now();
            let elapsed = now.duration_since(last).as_secs_f64();
            last = now;
            if weak.update(cx, |this, cx| this.tick(elapsed, cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn tick(&mut self, elapsed: f64, cx: &mut Context<Self>) {
        let mut changed = false;
        if self.playing {
            let d = self.project.settings.duration;
            self.time += elapsed;
            if self.time >= d {
                self.time = 0.0;
            }
            self.follow_playhead();
            self.dirty = true;
            changed = true;
        }
        self.clock.store(self.time.to_bits(), Ordering::SeqCst);
        if let Some(message) = self.sound_error.lock().unwrap().take() {
            self.status = message;
            self.player = None;
            changed = true;
        }
        if let Some((signature, waves)) = self.wave_inbox.lock().unwrap().take() {
            if signature == self.wave_signature {
                self.waves = Arc::new(waves);
                changed = true;
            }
        }
        self.poll_ticks += 1;
        if self.poll_ticks % 30 == 0 && self.drag.is_none() && stamp(&self.dir) > self.stamp {
            self.reload();
            self.status = "Updated from disk".into();
            changed = true;
        }
        if self.dirty {
            self.request_frame();
        }
        if let Some((key, image)) = self.worker.take() {
            if key <= self.request_key {
                if let Some(old) = self.frame.replace(image) {
                    self.retired.push(old);
                }
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn request_frame(&mut self) {
        let Some(bounds) = *self.canvas_bounds.lock().unwrap() else { return };
        let (w, h) = (self.project.settings.width as f64, self.project.settings.height as f64);
        let fit = (f64::from(bounds.size.width) / w).min(f64::from(bounds.size.height) / h).min(1.0);
        // Half-size proxy while moving; full preview size when still.
        let proxy = self.playing || self.drag.is_some();
        let scale = (fit * if proxy { 0.5 } else { 1.0 }).max(0.05);
        self.request_key += 1;
        self.worker.request(Job { compiled: self.compiled.clone(), t: self.time, scale, blur: !proxy, key: self.request_key });
        self.dirty = false;
    }

    fn reload(&mut self) {
        match io::load(&self.dir) {
            Ok(p) => {
                self.project = p;
                self.compiled = Arc::new(compile_with_guide(&self.project, self.guide.as_ref()));
                self.rows = layer_rows(&self.project, &self.compiled);
                self.stamp = stamp(&self.dir);
                self.dirty = true;
                self.refresh_waves();
                if self.player.is_some() {
                    self.start_sound();
                }
                if let Some(id) = &self.selected {
                    if find_layer(&self.project, id).is_none() {
                        self.selected = None;
                    }
                }
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Applies ops for real: saved, in the shared history, undoable.
    fn commit(&mut self, ops: Vec<Value>) {
        match self.toolbox.call("edit_apply", json!({"project": self.dir, "ops": ops, "author": "person"})) {
            Ok(out) => {
                let errors = out.json["errors"].as_array().map(|a| a.len()).unwrap_or(0);
                self.status = if errors > 0 { format!("{errors} problems — see check") } else { String::new() };
            }
            Err(e) => self.status = e,
        }
        self.reload();
    }

    /// Shows ops without saving them (live drags).
    fn preview(&mut self, ops: &[Value]) {
        if let Ok(a) = edit::apply(&self.project, ops) {
            self.compiled = Arc::new(compile_with_guide(&a.project, self.guide.as_ref()));
            self.dirty = true;
        }
    }

    fn undo(&mut self, redo: bool) {
        let args = if redo { json!({"project": self.dir, "redo": true}) } else { json!({"project": self.dir, "undo": 1}) };
        match self.toolbox.call("history", args) {
            Ok(_) => self.status = if redo { "Redone".into() } else { "Undone".into() },
            Err(e) => self.status = e,
        }
        self.reload();
    }

    fn follow_playhead(&mut self) {
        let Some(b) = *self.track_bounds.lock().unwrap() else { return };
        let span = f64::from(b.size.width) / self.zoom;
        if self.time < self.scroll || self.time > self.scroll + span * 0.95 {
            self.scroll = (self.time - span * 0.1).max(0.0);
        }
    }

    fn seek(&mut self, t: f64) {
        let fps = self.project.settings.fps;
        let n = fps.nearest_frame(t.clamp(0.0, self.project.settings.duration)).clamp(0, self.compiled.frame_count().saturating_sub(1) as i64);
        self.time = fps.time_of(n);
        self.clock.store(self.time.to_bits(), Ordering::SeqCst);
        self.dirty = true;
    }

    /// (Re)starts the sound from the saved project at the playhead.
    fn start_sound(&mut self) {
        self.clock.store(self.time.to_bits(), Ordering::SeqCst);
        self.player = Some(Player::start(self.project.clone(), self.compiled.clone(), self.dir.clone(), self.clock.clone(), self.sound_error.clone()));
    }

    /// Recomputes the waveform lane off the UI thread when the sound's
    /// placement changed.
    fn refresh_waves(&mut self) {
        let signature = motion_sound::signature(&self.project, &self.compiled);
        if signature == self.wave_signature {
            return;
        }
        self.wave_signature = signature.clone();
        let (project, compiled, dir, inbox) = (self.project.clone(), self.compiled.clone(), self.dir.clone(), self.wave_inbox.clone());
        std::thread::spawn(move || {
            let waves = motion_sound::waves(&project, &compiled, &dir);
            *inbox.lock().unwrap() = Some((signature, waves));
        });
    }

    fn step(&mut self, frames: i64) {
        let fps = self.project.settings.fps;
        self.seek(fps.time_of(fps.nearest_frame(self.time) + frames));
    }

    fn time_at(&self, x: f32) -> Option<f64> {
        let b = (*self.track_bounds.lock().unwrap())?;
        Some(self.scroll + f64::from(x - f32::from(b.origin.x)) / self.zoom)
    }

    /// Screen point → project pixel, through the fitted preview.
    fn project_point(&self, x: f32, y: f32) -> Option<(f64, f64)> {
        let b = (*self.canvas_bounds.lock().unwrap())?;
        let (w, h) = (self.project.settings.width as f64, self.project.settings.height as f64);
        let (bw, bh) = (f64::from(b.size.width), f64::from(b.size.height));
        let k = (bw / w).min(bh / h);
        let (ox, oy) = (f64::from(b.origin.x) + (bw - w * k) / 2.0, f64::from(b.origin.y) + (bh - h * k) / 2.0);
        Some(((x as f64 - ox) / k, (y as f64 - oy) / k))
    }

    fn layer_json(&self, id: &str) -> Option<Value> {
        find_layer(&self.project, id).and_then(|l| serde_json::to_value(l).ok())
    }

    fn eval<T: Animatable>(&self, id: &str, path: &str, default: T) -> (T, bool) {
        let v = self.layer_json(id).and_then(|j| j.pointer(&format!("/{}", path.replace('.', "/"))).cloned());
        let Some(v) = v else { return (default, false) };
        let Ok(prop) = serde_json::from_value::<Property<T>>(v) else { return (default, false) };
        let animated = prop.is_animated();
        let mut elements: &Compiled = &self.compiled;
        let mut ctx = Ctx { markers: &self.project.markers, base: self.compiled.base, elements: &mut elements, origin: 0.0 };
        match compile_prop(&prop, &mut ctx) {
            Ok(p) => (p.at(self.time), animated),
            Err(_) => (default, animated),
        }
    }

    /// Ops that make `path` show `value` now: a key when it is animated,
    /// otherwise a plain set.
    fn value_ops(&self, id: &str, path: &str, value: Value, animated: bool) -> Vec<Value> {
        if animated {
            vec![json!({"op": "key", "target": format!("{id}.{path}"), "at": round(self.time), "v": value})]
        } else {
            vec![json!({"op": "set", "target": format!("{id}.{path}"), "value": value})]
        }
    }

    fn field_ops(&self, id: &str, field: Field, value: f64) -> Vec<Value> {
        match field.component {
            Some(c) => {
                let default = if field.path == "transform.scale" { Vec2::ONE } else { Vec2::ZERO };
                let (mut v, animated) = self.eval::<Vec2>(id, field.path, default);
                v.0[c] = value;
                self.value_ops(id, field.path, json!([round(v.0[0]), round(v.0[1])]), animated)
            }
            None => {
                let (_, animated) = self.eval::<f64>(id, field.path, 0.0);
                self.value_ops(id, field.path, json!(round(value)), animated)
            }
        }
    }

    fn field_value(&self, id: &str, field: Field) -> f64 {
        match field.component {
            Some(c) => {
                let default = if field.path == "transform.scale" { Vec2::ONE } else { Vec2::ZERO };
                self.eval::<Vec2>(id, field.path, default).0 .0[c]
            }
            None => self.eval::<f64>(id, field.path, if field.path == "transform.opacity" { 1.0 } else { 0.0 }).0,
        }
    }

    fn hit(&mut self, x: f64, y: f64) -> Option<String> {
        let frame = self.compiled.frame(self.time);
        let mut states = Vec::new();
        all_states(&frame.layers, lahza_motion_core::space::Proj::IDENTITY, &mut states);
        let fonts = &mut self.fonts;
        states.iter().rev().find_map(|l| {
            let b = match &l.content {
                Content::Text(t) => {
                    let layout = fonts.layout(t);
                    let [x0, y0, x1, y1] = layout.bounds;
                    let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(a, b)| l.matrix.apply(a, b));
                    Some([
                        pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min),
                        pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
                        pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max),
                        pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
                    ])
                }
                Content::Group(_) => None,
                _ => lahza_motion_core::check::bounds(l, None),
            }?;
            (&*l.id != GUIDE_ID && x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]).then(|| l.id.to_string())
        })
    }

    fn selection_rect(&mut self) -> Option<[f64; 4]> {
        let id = self.selected.clone()?;
        let frame = self.compiled.frame(self.time);
        let state = find_state(&frame.layers, &id)?;
        match &state.content {
            Content::Text(t) => {
                let layout = self.fonts.layout(t);
                let [x0, y0, x1, y1] = layout.bounds;
                let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(a, b)| state.matrix.apply(a, b));
                Some([
                    pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min),
                    pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
                    pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max),
                    pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
                ])
            }
            _ => lahza_motion_core::check::bounds(&state, None),
        }
    }

    // ------------------------------------------------------------ input

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let command = k.modifiers.control || k.modifiers.platform;
        match k.key.as_str() {
            "space" => self.toggle_play(cx),
            "left" => self.step(if k.modifiers.shift { -10 } else { -1 }),
            "right" => self.step(if k.modifiers.shift { 10 } else { 1 }),
            "home" => self.seek(0.0),
            "end" => self.seek(self.project.settings.duration),
            "z" if command => self.undo(k.modifiers.shift),
            "y" if command => self.undo(true),
            "delete" | "backspace" => {
                if let Some(id) = self.selected.take() {
                    self.commit(vec![json!({"op": "remove", "id": id})]);
                }
            }
            "=" | "+" => self.zoom = (self.zoom * 1.25).min(4000.0),
            "-" => self.zoom = (self.zoom / 1.25).max(5.0),
            _ => return,
        }
        // Inside the studio, keys must not also drive the video behind.
        cx.stop_propagation();
        cx.notify();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        self.playing = !self.playing;
        self.generation.fetch_add(1, Ordering::SeqCst);
        if self.playing {
            self.start_sound();
        } else {
            self.player = None;
        }
        self.dirty = true;
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.clone() else { return };
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        match drag {
            Drag::Scrub => {
                if let Some(t) = self.time_at(x) {
                    self.seek(t);
                }
            }
            Drag::Key { layer, path, index, .. } => {
                if let Some(t) = self.time_at(x) {
                    let t = self.project.settings.fps.time_of(self.project.settings.fps.nearest_frame(t.max(0.0)));
                    let ops = self.key_ops(&layer, &path, index, json!(round(t)));
                    self.preview(&ops);
                }
            }
            Drag::Move { layer, start, origin } => {
                if let (Some(a), Some(b)) = (self.project_point(start.0, start.1), self.project_point(x, y)) {
                    let p = [round(origin.x() + b.0 - a.0), round(origin.y() + b.1 - a.1)];
                    let (_, animated) = self.eval::<Vec2>(&layer, "transform.position", Vec2::ZERO);
                    let ops = self.value_ops(&layer, "transform.position", json!(p), animated);
                    self.preview(&ops);
                }
            }
            Drag::Value { layer, field, start_x, origin } => {
                let v = origin + (x - start_x) as f64 * field.step;
                let ops = self.field_ops(&layer, field, v);
                self.preview(&ops);
            }
        }
        cx.notify();
    }

    fn key_ops(&self, layer: &str, path: &str, index: usize, at: Value) -> Vec<Value> {
        if index == usize::MAX {
            // Text animators: their own `at`.
            vec![json!({"op": "set", "target": format!("{layer}.{path}.at"), "value": at})]
        } else {
            vec![json!({"op": "anchor", "target": format!("{layer}.{path}"), "key": index + 1, "at": at})]
        }
    }

    fn mouse_up(&mut self, x: f32, y: f32, shift: bool, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        match drag {
            Drag::Scrub => {}
            Drag::Key { layer, path, index, from } => {
                if let Some(t) = self.time_at(x) {
                    let fps = self.project.settings.fps;
                    let t = fps.time_of(fps.nearest_frame(t.max(0.0)));
                    // Shift snaps to the nearest beat and keeps the sync
                    // by writing the marker reference.
                    let at = if shift { self.nearest_marker("beat", t).map(Value::from).unwrap_or(json!(round(t))) } else { json!(round(t)) };
                    if (t - from).abs() > 1e-6 || shift {
                        let ops = self.key_ops(&layer, &path, index, at);
                        self.commit(ops);
                    }
                }
            }
            Drag::Move { layer, start, origin } => {
                if let (Some(a), Some(b)) = (self.project_point(start.0, start.1), self.project_point(x, y)) {
                    if (b.0 - a.0).abs() + (b.1 - a.1).abs() > 0.5 {
                        let p = [round(origin.x() + b.0 - a.0), round(origin.y() + b.1 - a.1)];
                        let (_, animated) = self.eval::<Vec2>(&layer, "transform.position", Vec2::ZERO);
                        let ops = self.value_ops(&layer, "transform.position", json!(p), animated);
                        self.commit(ops);
                    }
                }
            }
            Drag::Value { layer, field, start_x, origin } => {
                if (x - start_x).abs() > 0.5 {
                    let ops = self.field_ops(&layer, field, origin + (x - start_x) as f64 * field.step);
                    self.commit(ops);
                }
            }
        }
        let _ = y;
        self.dirty = true;
        cx.notify();
    }

    fn nearest_marker(&self, track: &str, t: f64) -> Option<String> {
        let tr = self.project.markers.get(track)?;
        let (i, _) = tr.markers.iter().enumerate().min_by(|a, b| (a.1.t - t).abs().total_cmp(&(b.1.t - t).abs()))?;
        Some(format!("{track}:{}", i + 1))
    }

    // ------------------------------------------------------------ views

    fn x_of(&self, t: f64) -> f32 {
        ((t - self.scroll) * self.zoom) as f32
    }

    fn top_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let fps = self.project.settings.fps;
        let frame = fps.nearest_frame(self.time);
        let name = self.dir.file_stem().and_then(|s| s.to_str()).unwrap_or("motion").to_string();
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(44.0))
            .px_3()
            .border_b_1()
            .border_color(theme::line())
            .when(self.embedded, |d| {
                d.child(
                    button("motion-space-back", "Video", ButtonKind::Secondary, ButtonSize::Sm)
                        .icon("icons/chevron-left.svg")
                        .tooltip("Back to the video timeline")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.player = None;
                            this.playing = false;
                            cx.emit(CloseSpace);
                        })),
                )
            })
            .child(div().panel_title().child(name))
            .child(div().w(px(16.0)))
            .child(icon_button("motion-back", "icons/skip-back.svg", ButtonKind::Ghost, ButtonSize::Sm).tooltip("Start (Home)").on_click(cx.listener(|this, _, _, cx| {
                this.seek(0.0);
                cx.notify();
            })))
            .child(
                icon_button("motion-play", if self.playing { "icons/pause.svg" } else { "icons/play.svg" }, ButtonKind::Secondary, ButtonSize::Sm)
                    .tooltip("Play / pause (Space)")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx))),
            )
            .child(icon_button("motion-end", "icons/skip-forward.svg", ButtonKind::Ghost, ButtonSize::Sm).tooltip("End (End)").on_click(cx.listener(|this, _, _, cx| {
                let d = this.project.settings.duration;
                this.seek(d);
                cx.notify();
            })))
            .child(div().row_label().child(format!("{:>6.2} s  ·  f{frame}", self.time)))
            .child(div().flex_1())
            .child(div().text_size(px(12.0)).text_color(theme::muted()).child(SharedString::from(self.status.clone())))
            .child(icon_button("motion-undo", "icons/undo.svg", ButtonKind::Ghost, ButtonSize::Sm).tooltip("Undo (Ctrl+Z)").on_click(cx.listener(|this, _, _, cx| {
                this.undo(false);
                cx.notify();
            })))
            .child(icon_button("motion-redo", "icons/redo.svg", ButtonKind::Ghost, ButtonSize::Sm).tooltip("Redo (Ctrl+Shift+Z)").on_click(cx.listener(|this, _, _, cx| {
                this.undo(true);
                cx.notify();
            })))
            .child(div().text_size(px(11.0)).text_color(theme::muted()).child(self.worker.label()))
    }

    fn preview_area(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds_store = self.canvas_bounds.clone();
        let (w, h) = (self.project.settings.width as f64, self.project.settings.height as f64);
        let selection = self.selection_rect();
        let canvas_bounds = *self.canvas_bounds.lock().unwrap();
        let mut area = div()
            .id("motion-canvas")
            .relative()
            .flex_1()
            .m_3()
            .bg(theme::canvas_well())
            .rounded(px(6.0))
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, _| {
                        let mut stored = bounds_store.lock().unwrap();
                        if *stored != Some(bounds) {
                            *stored = Some(bounds);
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
                    let Some((px_, py_)) = this.project_point(x, y) else { return };
                    this.selected = this.hit(px_, py_);
                    if let Some(id) = this.selected.clone() {
                        let (origin, _) = this.eval::<Vec2>(&id, "transform.position", Vec2::ZERO);
                        this.drag = Some(Drag::Move { layer: id, start: (x, y), origin });
                    }
                    cx.notify();
                }),
            );
        if let Some(frame) = self.frame.clone() {
            area = area.child(img(frame).absolute().inset_0().size_full().object_fit(ObjectFit::Contain));
        }
        if let (Some(r), Some(b)) = (selection, canvas_bounds) {
            let (bw, bh) = (f64::from(b.size.width), f64::from(b.size.height));
            let k = (bw / w).min(bh / h);
            let (ox, oy) = ((bw - w * k) / 2.0, (bh - h * k) / 2.0);
            if r.iter().all(|v| v.is_finite()) {
                area = area.child(
                    div()
                        .absolute()
                        .left(px((ox + r[0] * k) as f32))
                        .top(px((oy + r[1] * k) as f32))
                        .w(px(((r[2] - r[0]) * k).max(2.0) as f32))
                        .h(px(((r[3] - r[1]) * k).max(2.0) as f32))
                        .border_1()
                        .border_color(theme::accent()),
                );
            }
        }
        area
    }

    fn inspector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = div().w(px(INSPECTOR_WIDTH)).h_full().border_l_1().border_color(theme::line()).p_3().flex().flex_col().gap_2();
        let Some(id) = self.selected.clone() else {
            let s = &self.project.settings;
            return panel
                .child(div().panel_title().child("Project"))
                .child(div().row_label().child(format!("{} × {} · {} fps · {:.2} s", s.width, s.height, s.fps.as_f64(), s.duration)))
                .child(div().text_size(px(12.0)).text_color(theme::muted()).child(format!(
                    "{} scenes · {} marker tracks · style {}",
                    self.project.scenes.len(),
                    self.project.markers.tracks.len(),
                    s.style.clone().unwrap_or_else(|| "none".into())
                )))
                .child(div().text_size(px(12.0)).text_color(theme::muted()).child("Click a layer in the preview or the timeline. Drag values to change them; drag keys to retime (Shift snaps to beats)."));
        };
        let Some(layer) = find_layer(&self.project, &id).cloned() else { return panel };
        panel = panel
            .child(div().panel_title().child(id.clone()))
            .child(div().text_size(px(12.0)).text_color(theme::muted()).child(format!("{} · blend {:?}{}", layer.kind.name(), layer.common.blend, if layer.common.three_d { " · 3D" } else { "" })));
        if let LayerKind::Text(t) = &layer.kind {
            panel = panel.child(div().row_label().child(format!("“{}”", t.text)));
        }
        for (i, field) in FIELDS.iter().enumerate() {
            if field.path.contains("_x") || field.path.contains("_y") || field.path.ends_with(".z") {
                if !layer.common.three_d {
                    continue;
                }
            }
            let value = self.field_value(&id, *field);
            let (_, animated) = if field.component.is_some() { let (v, a) = self.eval::<Vec2>(&id, field.path, Vec2::ZERO); (v.0[0], a) } else { self.eval::<f64>(&id, field.path, 0.0) };
            let lid = id.clone();
            let f = *field;
            panel = panel.child(
                div()
                    .id(("motion-field", i))
                    .flex()
                    .justify_between()
                    .items_center()
                    .h(px(26.0))
                    .px_2()
                    .rounded(px(4.0))
                    .bg(theme::control_bg())
                    .hover(|s| s.bg(theme::control_hover()))
                    .cursor_ew_resize()
                    .child(div().text_size(px(12.0)).text_color(theme::muted()).child(format!("{}{}", f.label, if animated { "  ◆" } else { "" })))
                    .child(div().text_size(px(12.0)).text_color(theme::ink()).child(format!("{value:.2}")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            let origin = this.field_value(&lid, f);
                            this.drag = Some(Drag::Value { layer: lid.clone(), field: f, start_x: f32::from(e.position.x), origin });
                            cx.notify();
                        }),
                    ),
            );
        }
        let effects: Vec<String> = serde_json::to_value(&layer.common.effects)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| e.get("type").and_then(Value::as_str).map(String::from))
            .collect();
        if !effects.is_empty() {
            panel = panel.child(div().section_title().child("Effects")).child(div().text_size(px(12.0)).text_color(theme::muted()).child(effects.join(" · ")));
        }
        let keys = self.rows.iter().find(|r| r.id == id).map(|r| r.keys.len()).unwrap_or(0);
        panel.child(div().text_size(px(12.0)).text_color(theme::muted()).child(format!("{keys} keys · Delete removes the layer")))
    }

    fn timeline(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let track_store = self.track_bounds.clone();
        let duration = self.project.settings.duration;
        let width = self.track_bounds.lock().unwrap().map(|b| f32::from(b.size.width)).unwrap_or(800.0);
        // Ruler ticks every "nice" step of about 80 px.
        let target = 80.0 / self.zoom;
        let step = [0.04, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| *s >= target).unwrap_or(60.0);
        let mut ruler = div()
            .id("motion-ruler")
            .relative()
            .flex_1()
            .h(px(ROW))
            .overflow_hidden()
            .bg(theme::surface())
            .cursor_text()
            .child(
                canvas(
                    move |bounds, window, _| {
                        let mut s = track_store.lock().unwrap();
                        if *s != Some(bounds) {
                            *s = Some(bounds);
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.drag = Some(Drag::Scrub);
                    if let Some(t) = this.time_at(f32::from(e.position.x)) {
                        this.seek(t);
                    }
                    cx.notify();
                }),
            );
        let mut t = (self.scroll / step).floor() * step;
        while t <= duration && self.x_of(t) < width {
            let x = self.x_of(t);
            if x >= 0.0 {
                ruler = ruler.child(div().absolute().left(px(x)).top_0().h_full().w(px(1.0)).bg(theme::line())).child(
                    div().absolute().left(px(x + 3.0)).top(px(4.0)).text_size(px(10.0)).text_color(theme::muted()).child(format!("{t:.2}")),
                );
            }
            t += step;
        }
        // Marker lane: bars bold, beats thin, hits as dots.
        let mut markers = div().relative().flex_1().h(px(18.0)).overflow_hidden();
        for (track, colour, w) in [("beat", theme::muted(), 1.0f32), ("bar", theme::ink_secondary(), 2.0), ("hit", theme::brand(), 3.0)] {
            if let Some(tr) = self.project.markers.get(track) {
                for m in &tr.markers {
                    let x = self.x_of(m.t);
                    if x < 0.0 || x > width {
                        continue;
                    }
                    markers = markers.child(div().absolute().left(px(x)).top(px(if track == "hit" { 12.0 } else { 3.0 })).w(px(w)).h(px(if track == "hit" { 3.0 } else { 12.0 })).bg(colour));
                }
            }
        }
        if let Some(words) = self.project.markers.get("word") {
            for m in &words.markers {
                let x = self.x_of(m.t);
                if x >= 0.0 && x < width {
                    markers = markers.child(div().absolute().left(px(x)).top_0().text_size(px(9.0)).text_color(theme::muted()).child(m.label.clone().unwrap_or_default()));
                }
            }
        }
        // Sound: peaks of every clip, two pixels per bar.
        let (waves, zoom, scroll, colour) = (self.waves.clone(), self.zoom, self.scroll, theme::lane_audio());
        let sound = div().relative().flex_1().h(px(30.0)).overflow_hidden().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    let mut x = 0.0f32;
                    while x < w {
                        let t0 = scroll + f64::from(x) / zoom;
                        let peak = waves.iter().map(|wave| wave.peak(t0, t0 + 2.0 / zoom)).fold(0.0, f32::max);
                        if peak > 0.0 {
                            let bar = (peak * (h - 6.0)).max(1.0);
                            window.paint_quad(gpui::fill(
                                Bounds::new(gpui::point(bounds.origin.x + px(x), bounds.origin.y + px((h - bar) / 2.0)), size(px(1.5), px(bar))),
                                colour,
                            ));
                        }
                        x += 2.0;
                    }
                },
            )
            .absolute()
            .size_full(),
        );
        // Scenes.
        let mut scenes = div().relative().flex_1().h(px(ROW)).overflow_hidden();
        for s in &self.compiled.scenes {
            let (x0, x1) = (self.x_of(s.start).max(0.0), self.x_of(s.end).min(width));
            if x1 <= x0 {
                continue;
            }
            scenes = scenes.child(
                div()
                    .absolute()
                    .left(px(x0))
                    .w(px(x1 - x0 - 1.0))
                    .top(px(3.0))
                    .h(px(ROW - 6.0))
                    .rounded(px(3.0))
                    .bg(theme::lane_clip())
                    .px_1()
                    .text_size(px(11.0))
                    .text_color(theme::ink())
                    .overflow_hidden()
                    .child(s.id.to_string()),
            );
        }
        let lane = |label: String, track: gpui::AnyElement| -> gpui::Div {
            div()
                .flex()
                .border_b_1()
                .border_color(theme::line())
                .child(div().w(px(LABEL_WIDTH)).px_2().flex().items_center().text_size(px(11.0)).text_color(theme::muted()).child(label))
                .child(track)
        };
        let mut rows = div().id("motion-rows").flex().flex_col().overflow_y_scroll().flex_1();
        let selected = self.selected.clone();
        for (ri, row) in self.rows.iter().enumerate() {
            let is_sel = selected.as_deref() == Some(row.id.as_str());
            let (x0, x1) = (self.x_of(row.start).max(0.0), self.x_of(row.end).min(width));
            let mut track = div().relative().flex_1().h(px(ROW)).overflow_hidden();
            if x1 > x0 {
                let colour: Hsla = match row.kind {
                    "text" => theme::lane_zoom(),
                    "video" | "image" => theme::lane_video(),
                    "comp" => theme::lane_camera(),
                    _ => theme::lane_audio(),
                };
                track = track.child(div().absolute().left(px(x0)).w(px(x1 - x0)).top(px(5.0)).h(px(ROW - 10.0)).rounded(px(3.0)).bg(colour).opacity(if is_sel { 0.9 } else { 0.55 }));
            }
            for (ki, k) in row.keys.iter().enumerate() {
                let x = self.x_of(k.t);
                if x < -6.0 || x > width + 6.0 {
                    continue;
                }
                let (layer, path, index, from) = (row.id.clone(), k.path.clone(), k.index, k.t);
                track = track.child(
                    div()
                        .id(("motion-key", ri * 10_000 + ki))
                        .absolute()
                        .left(px(x - 5.0))
                        .top(px(3.0))
                        .text_size(px(12.0))
                        .text_color(if is_sel { theme::accent() } else { theme::ink() })
                        .cursor_ew_resize()
                        .child("◆")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.selected = Some(layer.clone());
                                this.drag = Some(Drag::Key { layer: layer.clone(), path: path.clone(), index, from });
                                cx.notify();
                            }),
                        ),
                );
            }
            let id = row.id.clone();
            let label = format!("{}{} {}", "  ".repeat(row.depth), row.id, row.kind);
            rows = rows.child(
                div()
                    .id(("motion-row", ri))
                    .flex()
                    .border_b_1()
                    .border_color(theme::line())
                    .bg(if is_sel { theme::selection_fill() } else { theme::panel() })
                    .child(div().w(px(LABEL_WIDTH)).px_2().flex().items_center().text_size(px(12.0)).text_color(theme::ink()).overflow_hidden().child(label))
                    .child(track)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus);
                            this.selected = Some(id.clone());
                            cx.notify();
                        }),
                    ),
            );
        }
        let playhead = LABEL_WIDTH + self.x_of(self.time);
        div()
            .id("motion-timeline")
            .relative()
            .h(px(260.0))
            .border_t_1()
            .border_color(theme::line())
            .flex()
            .flex_col()
            .bg(theme::panel())
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                let (dx, dy) = match e.delta {
                    ScrollDelta::Pixels(d) => (f64::from(d.x), f64::from(d.y)),
                    ScrollDelta::Lines(d) => (d.x as f64 * 16.0, d.y as f64 * 16.0),
                };
                if e.modifiers.control {
                    this.zoom = (this.zoom * 2f64.powf(-dy / 220.0)).clamp(5.0, 4000.0);
                } else if dx.abs() > 0.0 || e.modifiers.shift {
                    let d = if dx.abs() > 0.0 { dx } else { dy };
                    this.scroll = (this.scroll - d / this.zoom).max(0.0);
                } else {
                    return;
                }
                cx.notify();
            }))
            .child(lane("Time".into(), ruler.into_any_element()))
            .child(lane("Markers".into(), markers.into_any_element()))
            .children((!self.waves.is_empty()).then(|| lane("Sound".into(), sound.into_any_element())))
            .child(lane("Scenes".into(), scenes.into_any_element()))
            .child(rows)
            .child(div().absolute().top_0().bottom_0().left(px(playhead)).w(px(2.0)).bg(theme::brand()))
    }
}

impl Render for MotionEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .text_color(theme::ink())
            .font_family("Inter")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.mouse_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseUpEvent, _, cx| this.mouse_up(f32::from(e.position.x), f32::from(e.position.y), e.modifiers.shift, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseUpEvent, _, cx| this.mouse_up(f32::from(e.position.x), f32::from(e.position.y), e.modifiers.shift, cx)),
            )
            .child(self.top_bar(cx))
            .child(div().flex().flex_1().min_h(px(200.0)).child(self.preview_area(cx)).child(self.inspector(cx)))
            .child(self.timeline(cx))
    }
}

/// Opens a motion project in its own window.
pub(crate) fn open(dir: PathBuf, cx: &mut App) -> Result<gpui::WindowHandle<MotionEditor>, String> {
    let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
    let name = dir.file_stem().and_then(|s| s.to_str()).unwrap_or("motion").to_string();
    let result = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(980.0), px(640.0))),
            titlebar: Some(TitlebarOptions { title: Some(format!("{name} — Lahza motion").into()), ..Default::default() }),
            app_id: Some("com.lahza.Lahza".into()),
            window_decorations: Some(WindowDecorations::Server),
            ..Default::default()
        },
        move |window, cx| {
            theme::follow_appearance(window);
            let editor = cx.new(|cx| {
                let mut e = MotionEditor::new(dir.clone(), cx).unwrap_or_else(|err| panic!("cannot open {}: {err}", dir.display()));
                e.start_ticking(cx);
                // Release GPU textures however the window closes.
                cx.on_release_in(window, |this, window, _| {
                    for image in this.retired.drain(..).chain(this.frame.take()) {
                        let _ = window.drop_image(image);
                    }
                })
                .detach();
                e
            });
            window.focus(&editor.read(cx).focus);
            editor
        },
    );
    cx.activate(true);
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, Application, Modifiers};

    fn sample(dir: &Path) -> PathBuf {
        let mut tb = Toolbox::new();
        tb.cwd = dir.to_path_buf();
        tb.backend = lahza_motion_render::Backend::Cpu;
        tb.call("project_create", json!({"path": "edit", "width": 640, "height": 360, "duration": 3})).unwrap();
        tb.call("markers_import", json!({"kind": "user", "name": "beat", "markers": [0.5, 1.0, 1.5, 2.0, 2.5]})).unwrap();
        tb.call("edit_apply", json!({"ops": [
            {"op": "add_scene", "id": "s1"},
            {"op": "add", "id": "box", "kind": "shape", "shape": "rect", "size": [120, 80], "fill": "#ff3355",
             "transform": {"position": {"keys": [{"at": 0, "v": [200, 180]}, {"at": "beat:3", "v": [440, 180], "ease": "glide", "dur": "10f", "anchor": "start"}]}}},
            {"op": "add", "id": "title", "kind": "text", "text": "Hello", "size": 48, "transform": {"position": [320, 80]},
             "animators": [{"at": "beat:1", "from": {"opacity": 0, "y": 20}}]}
        ]})).unwrap();
        dir.join("edit.lahza")
    }

    #[test]
    fn rows_and_keys_resolve_marker_times() {
        let dir = std::env::temp_dir().join(format!("lahza-motion-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = io::load(&sample(&dir)).unwrap();
        let compiled = Compiled::new(&project);
        let rows = layer_rows(&project, &compiled);
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["box", "title"]);
        let keys: Vec<(String, usize, f64)> = rows[0].keys.iter().map(|k| (k.path.clone(), k.index, k.t)).collect();
        assert_eq!(keys, vec![("transform.position".into(), 0, 0.0), ("transform.position".into(), 1, 1.5)]);
        assert_eq!(rows[1].keys[0].path, "animators.0");
        assert_eq!(rows[1].keys[0].t, 0.5);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Opens the real window, waits for a rendered frame, and drives the
    /// editor: hit-testing, value scrubbing, key retiming (with beat snap),
    /// undo/redo and live reload of an agent's edit.
    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn editor_window_edits_and_reloads() {
        let dir = std::env::temp_dir().join(format!("lahza-motion-editor-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let project = sample(&dir);
        let project2 = project.clone();
        Application::new().with_assets(crate::Assets { base: crate::asset_directory() }).run(move |cx| {
            let handle = open(project2.clone(), cx).unwrap();
            let project = project2.clone();
            cx.spawn(async move |cx| {
                let wait = |ms| Timer::after(Duration::from_millis(ms));
                // Lay out, then wait for the worker's first frame.
                let any: gpui::AnyWindowHandle = handle.into();
                let draw = |cx: &mut gpui::AsyncApp| any.update(cx, |_, window, cx| window.draw(cx).clear()).unwrap();
                let mut framed = false;
                for _ in 0..200 {
                    wait(50).await;
                    draw(cx);
                    framed = handle.update(cx, |editor, _, _| editor.frame.is_some()).unwrap();
                    if framed {
                        break;
                    }
                }
                assert!(framed, "the preview rendered a frame");
                handle
                    .update(cx, |editor, _, cx| {
                        // Hit-test the box at its first key position.
                        editor.seek(0.0);
                        assert_eq!(editor.hit(200.0, 180.0).as_deref(), Some("box"));
                        assert_eq!(editor.hit(10.0, 10.0), None);
                        // Scrub the title's opacity (constant → set).
                        editor.selected = Some("title".into());
                        let field = FIELDS[5];
                        editor.drag = Some(Drag::Value { layer: "title".into(), field, start_x: 100.0, origin: 1.0 });
                        let moved = gpui::MouseMoveEvent { position: point(px(0.0), px(0.0)), pressed_button: Some(MouseButton::Left), modifiers: Modifiers::default() };
                        editor.mouse_move(&moved, cx);
                        editor.mouse_up(0.0, 0.0, false, cx);
                        assert!((editor.field_value("title", field) - 0.5).abs() < 1e-9, "opacity scrubbed to 0.5");
                        // Retime the box's second key onto beat 4 (Shift snaps).
                        let b = editor.track_bounds.lock().unwrap().expect("timeline laid out");
                        let x = f32::from(b.origin.x) + ((1.95 - editor.scroll) * editor.zoom) as f32;
                        editor.drag = Some(Drag::Key { layer: "box".into(), path: "transform.position".into(), index: 1, from: 1.5 });
                        editor.mouse_up(x, 0.0, true, cx);
                        let p = io::load(&editor.dir).unwrap();
                        let json = serde_json::to_value(find_layer(&p, "box").unwrap()).unwrap();
                        assert_eq!(json["transform"]["position"]["keys"][1]["at"], "beat:4", "status: {} (x {x}, bounds {b:?})", editor.status);
                        // Undo the retime, then redo it.
                        editor.undo(false);
                        let json = serde_json::to_value(find_layer(&editor.project, "box").unwrap()).unwrap();
                        assert_eq!(json["transform"]["position"]["keys"][1]["at"], "beat:3");
                        editor.undo(true);
                        let json = serde_json::to_value(find_layer(&editor.project, "box").unwrap()).unwrap();
                        assert_eq!(json["transform"]["position"]["keys"][1]["at"], "beat:4");
                    })
                    .unwrap();
                // An agent edits the project on disk; the editor picks it up.
                let mut agent = Toolbox::new();
                agent.call("edit_apply", json!({"project": project, "ops": [{"op": "add", "scene": "s1", "id": "dot", "kind": "shape", "shape": "circle", "fill": "#ffffff"}]})).unwrap();
                let mut seen = false;
                for _ in 0..60 {
                    wait(50).await;
                    draw(cx);
                    seen = handle.update(cx, |editor, _, _| editor.rows.iter().any(|r| r.id == "dot")).unwrap();
                    if seen {
                        break;
                    }
                }
                assert!(seen, "live reload of the agent's edit");
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
        std::fs::remove_dir_all(&dir).ok();
    }
}
