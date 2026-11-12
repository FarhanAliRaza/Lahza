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

use crate::motion_sound::{self, PeakCache, Player, Wave};
use crate::theme;
use gpui::{
    canvas, div, img, prelude::*, px, Bounds, Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels,
    Render, RenderImage, Timer, Window,
};
use lahza_motion_core::compile::{Compiled, Content, LayerState};
use lahza_motion_core::document::{Layer, Project};
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
    /// Why the last frame could not render (a missing or failed GPU).
    error: Option<String>,
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
                let mut renderer = Renderer::new(Some(&dir), RenderOptions { backend: lahza_motion_render::Backend::Auto, ..Default::default() });
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
                    let mut pixels = match renderer.try_render(&job.compiled, job.t) {
                        Ok(pixels) => pixels,
                        Err(e) => {
                            inner.0.lock().unwrap().error = Some(e);
                            continue;
                        }
                    };
                    // GPUI uploads BGRA.
                    for p in pixels.chunks_exact_mut(4) {
                        p.swap(0, 2);
                    }
                    let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
                    let mut mailbox = inner.0.lock().unwrap();
                    mailbox.done = Some((job.key, image));
                    mailbox.error = None;
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

    fn error(&self) -> Option<String> {
        self.shared.0.lock().unwrap().error.clone()
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

/// A layer moved on the canvas: the pointer at press and the layer's
/// position then.
#[derive(Clone)]
struct Drag {
    layer: String,
    start: (f32, f32),
    origin: Vec2,
}

/// The handle of a sound bar being dragged.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum SoundPart {
    Move,
    /// Leading edge: trims the start of the file (the sound starts later).
    Start,
    /// Trailing edge: how long it plays.
    End,
    FadeIn,
    FadeOut,
}

/// Where and how a sound plays, for drag previews.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct SoundShape {
    pub start: f64,
    trim: f64,
    pub length: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    source_duration: f64,
}

const MIN_SOUND: f64 = 0.05;

impl SoundShape {
    pub(crate) fn of(w: &Wave) -> Self {
        Self { start: w.start, trim: w.source_start, length: w.duration, fade_in: w.fade_in, fade_out: w.fade_out, source_duration: w.source_duration }
    }

    /// The shape after dragging `part` by `d` seconds.
    pub(crate) fn dragged(self, part: SoundPart, d: f64) -> Self {
        let mut s = self;
        match part {
            SoundPart::Move => s.start = (self.start + d).max(0.0),
            SoundPart::Start => {
                let d = d.max(-self.trim).max(-self.start).min(self.length - MIN_SOUND);
                s.start += d;
                s.trim += d;
                s.length -= d;
                s.fade_in = s.fade_in.min(s.length);
                s.fade_out = s.fade_out.min(s.length - s.fade_in);
            }
            SoundPart::End => {
                let longest = if self.source_duration > 0.0 { self.source_duration - self.trim } else { f64::INFINITY };
                s.length = (self.length + d).clamp(MIN_SOUND.min(longest), longest);
                s.fade_out = s.fade_out.min(s.length);
                s.fade_in = s.fade_in.min(s.length - s.fade_out);
            }
            SoundPart::FadeIn => s.fade_in = (self.fade_in + d).clamp(0.0, self.length - self.fade_out),
            SoundPart::FadeOut => s.fade_out = (self.fade_out - d).clamp(0.0, self.length - self.fade_in),
        }
        s
    }

    pub(crate) fn apply(self, w: &Wave) -> Wave {
        Wave { start: self.start, source_start: self.trim, duration: self.length, fade_in: self.fade_in, fade_out: self.fade_out, ..w.clone() }
    }
}

/// `at` moved by `d` seconds, keeping a reference (`#card.start + 0.12`).
fn shift_time(at: &Value, d: f64, fps: f64) -> Value {
    if let Some(n) = at.as_f64() {
        return json!(round((n + d).max(0.0)));
    }
    let Some(text) = at.as_str() else { return json!(round(d.max(0.0))) };
    let (base, mut offset) = split_offset(text, fps);
    offset += d;
    if offset.abs() < 5e-4 {
        json!(base)
    } else {
        json!(format!("{base} {} {:.3}", if offset < 0.0 { "-" } else { "+" }, offset.abs()))
    }
}

/// "#card.start + 4f" → ("#card.start", 4 frames in seconds).
fn split_offset(text: &str, fps: f64) -> (String, f64) {
    let at = text.rfind(" + ").or_else(|| text.rfind(" - "));
    if let Some(i) = at {
        let sign = if &text[i..i + 3] == " - " { -1.0 } else { 1.0 };
        let tail = text[i + 3..].trim();
        let value = match tail.strip_suffix('f') {
            Some(frames) => frames.trim().parse::<f64>().ok().map(|f| f / fps),
            None => tail.strip_suffix('s').unwrap_or(tail).trim().parse::<f64>().ok(),
        };
        if let Some(v) = value {
            return (text[..i].trim().to_string(), sign * v);
        }
    }
    (text.to_string(), 0.0)
}

/// A scrubbable inspector value: property path and vector component.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Field {
    pub path: &'static str,
    pub component: Option<usize>,
    pub label: &'static str,
    /// Value change per dragged pixel.
    pub step: f64,
}

pub(crate) const FIELDS: &[Field] = &[
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

pub(crate) struct KeyMark {
    pub path: String,
    pub index: usize,
    pub t: f64,
}

/// One layer on the timeline: its window and keys, in project seconds.
pub(crate) struct Row {
    pub id: String,
    pub kind: &'static str,
    pub depth: usize,
    pub start: f64,
    pub end: f64,
    pub keys: Vec<KeyMark>,
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
    /// The preview renderer's error, shown over the canvas.
    render_error: Option<String>,
    retired: Vec<Arc<RenderImage>>,
    request_key: u64,
    dirty: bool,
    selected: Option<String>,
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
    /// Decoded peaks per sound file.
    peaks: PeakCache,
}

/// Newest modification time of a motion project's files.
pub(crate) fn stamp(dir: &Path) -> SystemTime {
    let mut latest = SystemTime::UNIX_EPOCH;
    let mut look = |p: &Path| {
        if let Ok(m) = std::fs::metadata(p).and_then(|m| m.modified()) {
            latest = latest.max(m);
        }
    };
    look(&dir.join("project.json"));
    for sub in ["scenes", "markers", "shaders"] {
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
            render_error: None,
            retired: Vec::new(),
            request_key: 0,
            dirty: true,
            selected: None,
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
            peaks: PeakCache::default(),
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
        let error = self.worker.error();
        if error != self.render_error {
            self.render_error = error;
            changed = true;
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
                self.compiled = Arc::new(Compiled::new(&self.project));
                self.rows = layer_rows(&self.project, &self.compiled);
                self.stamp = stamp(&self.dir);
                self.dirty = true;
                self.refresh_waves();
                if self.player.is_some() {
                    self.start_sound();
                }
                if let Some(id) = &self.selected {
                    if find_layer(&self.project, id).is_none() && self.sound_clip(id).is_none() {
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
            self.compiled = Arc::new(Compiled::new(&a.project));
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


    pub(crate) fn seek(&mut self, t: f64) {
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
        let (project, compiled, dir, inbox, peaks) = (self.project.clone(), self.compiled.clone(), self.dir.clone(), self.wave_inbox.clone(), self.peaks.clone());
        std::thread::spawn(move || {
            let waves = motion_sound::waves(&project, &compiled, &dir, &peaks);
            *inbox.lock().unwrap() = Some((signature, waves));
        });
    }

    pub(crate) fn step(&mut self, frames: i64) {
        let fps = self.project.settings.fps;
        self.seek(fps.time_of(fps.nearest_frame(self.time) + frames));
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
        let linker = lahza_motion_core::slang::Linker::default();
        let mut ctx = Ctx { markers: &self.project.markers, base: self.compiled.base, elements: &mut elements, origin: 0.0, slang: &linker };
        match compile_prop(&prop, &mut ctx) {
            Ok(p) => {
                // A Slang track shows its value (the build is cached).
                if !linker.is_empty() {
                    let _ = linker.link(&lahza_motion_core::surface::modules(&self.project.shaders));
                }
                (p.at(self.time), animated)
            }
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
            (x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]).then(|| l.id.to_string())
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

    /// The canvas handles Delete; the studio owns playback, undo and the
    /// timeline keys.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !matches!(event.keystroke.key.as_str(), "delete" | "backspace") {
            return;
        }
        if let Some(id) = self.selected.take() {
            self.commit(vec![json!({"op": "remove", "id": id})]);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn toggle_play(&mut self, cx: &mut Context<Self>) {
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
        let Some(Drag { layer, start, origin }) = self.drag.clone() else { return };
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        if let (Some(a), Some(b)) = (self.project_point(start.0, start.1), self.project_point(x, y)) {
            self.move_layer(&layer, [origin.x() + b.0 - a.0, origin.y() + b.1 - a.1], false);
        }
        cx.notify();
    }

    /// Shows key `index` of `layer.path` at project time `t` (unsaved).
    pub(crate) fn drag_key(&mut self, layer: &str, path: &str, index: usize, t: f64) {
        let fps = self.project.settings.fps;
        let t = fps.time_of(fps.nearest_frame(t.max(0.0)));
        let ops = self.key_ops(layer, path, index, json!(round(t)));
        self.preview(&ops);
    }

    /// Saves key `index` of `layer.path` at project time `t`, moved from
    /// `from`; Shift snaps to the nearest beat and keeps the sync by
    /// writing the marker reference.
    pub(crate) fn drop_key(&mut self, layer: &str, path: &str, index: usize, from: f64, t: f64, shift: bool) {
        let fps = self.project.settings.fps;
        let t = fps.time_of(fps.nearest_frame(t.max(0.0)));
        let at = if shift { self.nearest_marker("beat", t).map(Value::from).unwrap_or(json!(round(t))) } else { json!(round(t)) };
        if (t - from).abs() > 1e-6 || shift {
            let ops = self.key_ops(layer, path, index, at);
            self.commit(ops);
        } else {
            // A click: drop the drag preview.
            self.compiled = Arc::new(Compiled::new(&self.project));
        }
    }

    /// Saves a dragged sound: `orig` moved by `d` seconds at `part`.
    /// `moved` says the pointer travelled far enough to mean it; `reach`
    /// is the snapping distance in seconds.
    pub(crate) fn drop_sound(&mut self, id: &str, part: SoundPart, orig: SoundShape, d: f64, moved: bool, reach: f64, shift: bool) {
        let new = orig.dragged(part, d);
        let set = |field: &str, value: Value| json!({"op": "set", "target": format!("{id}.{field}"), "value": value});
        let ops = match part {
            SoundPart::Move if moved || shift => vec![set("at", self.sound_anchor(new.start, reach, shift))],
            SoundPart::Start if moved => {
                let at = self.sound_clip(id).and_then(|c| serde_json::to_value(&c.at).ok()).unwrap_or(json!(0));
                let fps = self.project.settings.fps.as_f64();
                vec![set("at", shift_time(&at, new.start - orig.start, fps)), set("trim", json!(round(new.trim))), set("length", json!(round(new.length)))]
            }
            SoundPart::End if moved => vec![set("length", json!(round(new.length)))],
            SoundPart::FadeIn | SoundPart::FadeOut if moved => {
                vec![set("fade_in", json!(round(new.fade_in))), set("fade_out", json!(round(new.fade_out)))]
            }
            _ => Vec::new(),
        };
        if !ops.is_empty() {
            self.commit(ops);
        }
    }

    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The rows the timeline lists: layers alive at the playhead, so it
    /// shows what is on screen now (the comp you are in, as in After
    /// Effects) rather than every layer of every scene; the selected layer
    /// always stays listed.
    pub(crate) fn visible_rows(&self) -> Vec<&Row> {
        let t = self.time;
        self.rows.iter().filter(|r| (r.start <= t && t <= r.end) || self.selected.as_deref() == Some(r.id.as_str())).collect()
    }

    pub(crate) fn waves(&self) -> Arc<Vec<Wave>> {
        self.waves.clone()
    }

    /// The compiled project as it shows now, drag previews included.
    pub(crate) fn compiled(&self) -> Arc<Compiled> {
        self.compiled.clone()
    }

    pub(crate) fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub(crate) fn select(&mut self, id: Option<String>) {
        self.selected = id;
    }

    pub(crate) fn undo_edit(&mut self, redo: bool) {
        self.undo(redo);
    }

    /// Moves the editor's playhead (project seconds): values are read and
    /// keyed there.
    pub(crate) fn set_time(&mut self, t: f64) {
        let t = t.clamp(0.0, self.project.settings.duration);
        if (t - self.time).abs() > 1e-9 {
            self.time = t;
            self.dirty = true;
        }
    }

    pub(crate) fn layer(&self, id: &str) -> Option<&Layer> {
        find_layer(&self.project, id)
    }

    pub(crate) fn sound(&self, id: &str) -> Option<&lahza_motion_core::document::AudioClip> {
        self.sound_clip(id)
    }

    /// `field` of layer `id` at the playhead, and whether it is keyed.
    pub(crate) fn field_state(&self, id: &str, field: Field) -> (f64, bool) {
        let animated = if field.component.is_some() { self.eval::<Vec2>(id, field.path, Vec2::ZERO).1 } else { self.eval::<f64>(id, field.path, 0.0).1 };
        (self.field_value(id, field), animated)
    }

    /// Shows `field` of `layer` at `value` (unsaved).
    pub(crate) fn drag_value(&mut self, layer: &str, field: Field, value: f64) {
        let ops = self.field_ops(layer, field, value);
        self.preview(&ops);
    }

    /// Saves `field` of `layer` at `value`: a key at the playhead when the
    /// property is animated, otherwise a plain value.
    pub(crate) fn drop_value(&mut self, layer: &str, field: Field, value: f64) {
        let ops = self.field_ops(layer, field, value);
        self.commit(ops);
    }

    /// Drops any unsaved drag preview.
    pub(crate) fn cancel_preview(&mut self) {
        self.compiled = Arc::new(Compiled::new(&self.project));
        self.dirty = true;
    }

    /// Saves edit ops through the piece's history.
    pub(crate) fn commit_ops(&mut self, ops: Vec<Value>) {
        self.commit(ops);
    }

    pub(crate) fn status(&self) -> &str {
        &self.status
    }

    pub(crate) fn time(&self) -> f64 {
        self.time
    }

    pub(crate) fn playing(&self) -> bool {
        self.playing
    }

    pub(crate) fn project(&self) -> &Project {
        &self.project
    }

    pub(crate) fn frame_at_playhead(&self) -> i64 {
        self.project.settings.fps.nearest_frame(self.time)
    }

    /// The topmost layer under project point (`x`, `y`) at the playhead.
    pub(crate) fn hit_at(&mut self, x: f64, y: f64) -> Option<String> {
        self.hit(x, y)
    }

    /// The selected layer's box at the playhead, in project pixels.
    pub(crate) fn selection_bounds(&mut self) -> Option<[f64; 4]> {
        self.selection_rect()
    }

    /// `layer`'s position at the playhead.
    pub(crate) fn position_of(&self, layer: &str) -> Vec2 {
        self.eval::<Vec2>(layer, "transform.position", Vec2::ZERO).0
    }

    /// Moves `layer` to `p` (a key at the playhead when its position is
    /// animated): shown only, or saved with `commit`.
    pub(crate) fn move_layer(&mut self, layer: &str, p: [f64; 2], commit: bool) {
        let (_, animated) = self.eval::<Vec2>(layer, "transform.position", Vec2::ZERO);
        let ops = self.value_ops(layer, "transform.position", json!([round(p[0]), round(p[1])]), animated);
        if commit {
            self.commit(ops);
        } else {
            self.preview(&ops);
        }
    }

    fn key_ops(&self, layer: &str, path: &str, index: usize, at: Value) -> Vec<Value> {
        if index == usize::MAX {
            // Text animators: their own `at`.
            vec![json!({"op": "set", "target": format!("{layer}.{path}.at"), "value": at})]
        } else {
            vec![json!({"op": "anchor", "target": format!("{layer}.{path}"), "key": index + 1, "at": at})]
        }
    }

    fn mouse_up(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let Some(Drag { layer, start, origin }) = self.drag.take() else { return };
        match (self.project_point(start.0, start.1), self.project_point(x, y)) {
            (Some(a), Some(b)) if (b.0 - a.0).abs() + (b.1 - a.1).abs() > 0.5 => {
                self.move_layer(&layer, [origin.x() + b.0 - a.0, origin.y() + b.1 - a.1], true);
            }
            _ => self.cancel_preview(),
        }
        self.dirty = true;
        cx.notify();
    }

    /// Where a dragged sound lands: snapped (within 8 px) to a layer's start
    /// or end, written as an anchor so the sound follows that layer, or to a
    /// key; with Shift, to the nearest beat. Otherwise the frame.
    fn sound_anchor(&self, start: f64, reach: f64, shift: bool) -> Value {
        let fps = self.project.settings.fps;
        if shift {
            if let Some(beat) = self.nearest_marker("beat", start) {
                return json!(beat);
            }
        }
        let mut best: Option<(f64, Value)> = None;
        let mut consider = |t: f64, v: Value| {
            let d = (t - start).abs();
            if d <= reach && best.as_ref().is_none_or(|(b, _)| d < *b) {
                best = Some((d, v));
            }
        };
        for row in &self.rows {
            consider(row.start, json!(format!("#{}.start", row.id)));
            consider(row.end, json!(format!("#{}.end", row.id)));
            for k in &row.keys {
                consider(k.t, json!(round(k.t)));
            }
        }
        best.map(|(_, v)| v).unwrap_or_else(|| json!(round(fps.time_of(fps.nearest_frame(start)))))
    }

    /// The project sound with `id` ("music", "vo" or an effect's id).
    fn sound_clip(&self, id: &str) -> Option<&lahza_motion_core::document::AudioClip> {
        let a = &self.project.audio;
        match id {
            "music" if a.music.as_ref().is_some_and(|c| c.id.is_none()) => a.music.as_ref(),
            "vo" if a.vo.as_ref().is_some_and(|c| c.id.is_none()) => a.vo.as_ref(),
            _ => a.clips().find(|c| c.id.as_deref() == Some(id)),
        }
    }


    /// Adds a built-in effect at project time `t`; on the selected layer's
    /// start it anchors to that layer so it moves with it.
    pub(crate) fn add_sound_effect_at(&mut self, name: &str, t: f64) {
        let fps = self.project.settings.fps;
        let t = fps.time_of(fps.nearest_frame(t.max(0.0)));
        let anchored = self.selected.as_ref().and_then(|id| self.rows.iter().find(|r| &r.id == id)).filter(|r| (r.start - t).abs() < 0.1).map(|r| format!("#{}.start", r.id));
        let at = anchored.map(Value::from).unwrap_or(json!(round(t)));
        match self.toolbox.call("sfx_add", json!({"project": self.dir, "sound": name, "at": at})) {
            Ok(out) => {
                self.status = String::new();
                if let Some(id) = out.json["created"][0].as_str() {
                    self.selected = Some(id.to_string());
                }
            }
            Err(e) => self.status = e,
        }
        self.reload();
    }





    fn nearest_marker(&self, track: &str, t: f64) -> Option<String> {
        let tr = self.project.markers.get(track)?;
        let (i, _) = tr.markers.iter().enumerate().min_by(|a, b| (a.1.t - t).abs().total_cmp(&(b.1.t - t).abs()))?;
        Some(format!("{track}:{}", i + 1))
    }

    // ------------------------------------------------------------ views



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
                        this.drag = Some(Drag { layer: id, start: (x, y), origin });
                    }
                    cx.notify();
                }),
            );
        if let Some(frame) = self.frame.clone() {
            area = area.child(img(frame).absolute().inset_0().size_full().object_fit(ObjectFit::Contain));
        }
        if let Some(error) = &self.render_error {
            area = area.child(
                div()
                    .absolute()
                    .left_3()
                    .right_3()
                    .bottom_3()
                    .p_3()
                    .rounded(px(6.0))
                    .bg(theme::danger())
                    .text_sm()
                    .text_color(gpui::white())
                    .child(format!("Preview cannot render: {error}")),
            );
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


}

impl Render for MotionEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        div()
            .relative()
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
                cx.listener(|this, e: &gpui::MouseUpEvent, _, cx| this.mouse_up(f32::from(e.position.x), f32::from(e.position.y), cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseUpEvent, _, cx| this.mouse_up(f32::from(e.position.x), f32::from(e.position.y), cx)),
            )
            .child(div().flex().flex_1().min_h_0().child(self.preview_area(cx)))
    }
}


#[cfg(test)]
mod tests {

    #[test]
    fn sound_drags_trim_and_fade_within_the_file() {
        let shape = SoundShape { start: 1.0, trim: 0.0, length: 2.0, fade_in: 0.0, fade_out: 0.0, source_duration: 3.0 };
        // Leading trim: starts later in the file and later on the timeline.
        let t = shape.dragged(SoundPart::Start, 0.5);
        assert_eq!((t.start, t.trim, t.length), (1.5, 0.5, 1.5));
        // Cannot trim before the file's start or past zero.
        assert_eq!(shape.dragged(SoundPart::Start, -2.0).trim, 0.0);
        // Trailing trim stays inside the file and keeps a minimum.
        assert_eq!(shape.dragged(SoundPart::End, 5.0).length, 3.0);
        assert_eq!(shape.dragged(SoundPart::End, -5.0).length, MIN_SOUND);
        // Fades: the out handle is dragged left; fades never overlap.
        let f = shape.dragged(SoundPart::FadeIn, 0.4).dragged(SoundPart::FadeOut, -0.3);
        assert!((f.fade_in - 0.4).abs() < 1e-9 && (f.fade_out - 0.3).abs() < 1e-9);
        assert!((shape.dragged(SoundPart::FadeIn, 9.0).fade_in - 2.0).abs() < 1e-9);
        let both = SoundShape { fade_in: 1.5, ..shape }.dragged(SoundPart::FadeOut, -9.0);
        assert!((both.fade_out - 0.5).abs() < 1e-9);
        // Trimming shortens fades that no longer fit.
        let short = SoundShape { fade_in: 1.0, fade_out: 0.8, ..shape }.dragged(SoundPart::End, -1.5);
        assert!(short.fade_in + short.fade_out <= short.length + 1e-9);
    }

    #[test]
    fn trimming_an_anchored_sound_keeps_its_anchor() {
        assert_eq!(shift_time(&json!(1.0), 0.25, 30.0), json!(1.25));
        assert_eq!(shift_time(&json!("#card.start"), 0.12, 30.0), json!("#card.start + 0.120"));
        assert_eq!(shift_time(&json!("#card.start - 3f"), 0.1, 30.0), json!("#card.start + 0.000").as_str().map(|_| json!("#card.start")).unwrap());
        assert_eq!(shift_time(&json!("beat:9 + 0.5"), -0.75, 30.0), json!("beat:9 - 0.250"));
        assert_eq!(split_offset("#a.end + 6f", 30.0), ("#a.end".to_string(), 0.2));
    }
    use super::*;
    use gpui::Application;

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
            let bounds = gpui::Bounds::centered(None, gpui::size(px(1200.0), px(800.0)), cx);
            let options = gpui::WindowOptions { window_bounds: Some(gpui::WindowBounds::Windowed(bounds)), ..Default::default() };
            let dir = project2.clone();
            let handle = cx
                .open_window(options, move |_, cx| {
                    cx.new(|cx| {
                        let mut editor = MotionEditor::new(dir, cx).unwrap();
                        editor.start_ticking(cx);
                        editor
                    })
                })
                .unwrap();
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
                    .update(cx, |editor, _, _| {
                        // Hit-test the box at its first key position.
                        editor.seek(0.0);
                        assert_eq!(editor.hit_at(200.0, 180.0).as_deref(), Some("box"));
                        assert_eq!(editor.hit_at(10.0, 10.0), None);
                        // Scrub the title's opacity (constant → set).
                        let field = FIELDS[5];
                        editor.drag_value("title", field, 0.5);
                        editor.drop_value("title", field, 0.5);
                        assert!((editor.field_state("title", field).0 - 0.5).abs() < 1e-9, "opacity scrubbed to 0.5");
                        // Retime the box's second key onto beat 4 (Shift snaps).
                        editor.drop_key("box", "transform.position", 1, 1.5, 1.95, true);
                        let p = io::load(&editor.dir).unwrap();
                        let json = serde_json::to_value(find_layer(&p, "box").unwrap()).unwrap();
                        assert_eq!(json["transform"]["position"]["keys"][1]["at"], "beat:4", "status: {}", editor.status);
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
