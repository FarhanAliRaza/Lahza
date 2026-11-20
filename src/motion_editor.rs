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

use crate::motion_preview::{self, Ask};
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
use lahza_motion_core::value::{Animatable, Color, Vec2};
use lahza_motion_core::{edit, io};
use lahza_motion_mcp::tools::Toolbox;
use lahza_motion_render::font::Fonts;
use lahza_motion_render::{RenderOptions, Renderer};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};


// ------------------------------------------------------------ render worker

struct Job {
    compiled: Arc<Compiled>,
    t: f64,
    scale: f64,
    /// Rasterised, with the sky, or traced fast (`motion_preview`).
    ask: Ask,
    playing: bool,
    key: u64,
}

#[derive(Default)]
struct Mailbox {
    job: Option<Job>,
    done: Option<(u64, Arc<RenderImage>)>,
    /// Why the last frame could not render (a missing or failed GPU).
    error: Option<String>,
    /// The last frame was asked traced on a GPU that cannot.
    cannot_trace: bool,
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
                // The last frame drawn: what a traced frame builds on.
                let mut last = None;
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
                    let (mut pixels, cannot_trace) = match motion_preview::draw(&mut renderer, &mut last, &job.compiled, job.t, job.scale, job.ask, job.playing) {
                        Ok(drawn) => drawn,
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
                    mailbox.cannot_trace = cannot_trace;
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

    fn cannot_trace(&self) -> bool {
        self.shared.0.lock().unwrap().cannot_trace
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
#[derive(Clone, PartialEq)]
pub(crate) struct Field {
    pub path: Cow<'static, str>,
    pub component: Option<usize>,
    pub label: Cow<'static, str>,
    /// Value change per dragged pixel.
    pub step: f64,
    /// The value when the layer's JSON has none.
    pub default: f64,
}

const fn field(path: &'static str, component: Option<usize>, label: &'static str, step: f64, default: f64) -> Field {
    Field { path: Cow::Borrowed(path), component, label: Cow::Borrowed(label), step, default }
}

pub(crate) const FIELDS: &[Field] = &[
    field("transform.position", Some(0), "X", 1.0, 0.0),
    field("transform.position", Some(1), "Y", 1.0, 0.0),
    field("transform.scale", Some(0), "Scale X", 0.005, 1.0),
    field("transform.scale", Some(1), "Scale Y", 0.005, 1.0),
    field("transform.rotation", None, "Rotation", 0.5, 0.0),
    field("transform.opacity", None, "Opacity", 0.005, 1.0),
    field("transform.z", None, "Z", 2.0, 0.0),
    field("transform.rotation_x", None, "Tilt X", 0.5, 0.0),
    field("transform.rotation_y", None, "Turn Y", 0.5, 0.0),
];

/// Effects the inspector can add, with the values they start from (what
/// is not given takes the format's default). Not here:
/// `lut`, which needs a file.
pub(crate) const EFFECTS: &[(&str, &str)] = &[
    ("Blur", r#"{"type": "blur", "radius": 8}"#),
    ("Shadow", r##"{"type": "shadow", "color": "#00000080", "radius": 12, "offset": [0, 8]}"##),
    ("Glow", r#"{"type": "glow", "radius": 12, "intensity": 1}"#),
    ("Exposure", r#"{"type": "exposure", "stops": 0.5}"#),
    ("Contrast", r#"{"type": "brightness_contrast", "brightness": 0, "contrast": 0.2}"#),
    ("Levels", r#"{"type": "levels"}"#),
    ("Colour", r#"{"type": "hue_saturation", "hue": 0, "saturation": 0.2, "lightness": 0}"#),
    ("Gradient map", r##"{"type": "gradient_map", "stops": [[0, "#101030"], [1, "#ffd27a"]]}"##),
    ("Tint", r#"{"type": "tint"}"#),
    ("Fill", r##"{"type": "fill", "color": "#ffffff"}"##),
    ("Invert", r#"{"type": "invert"}"#),
    ("Motion blur", r#"{"type": "directional_blur"}"#),
    ("RGB split", r#"{"type": "rgb_split"}"#),
    ("Displace", r#"{"type": "displace"}"#),
    ("Vignette", r#"{"type": "vignette"}"#),
    ("Grain", r#"{"type": "grain"}"#),
];

/// Colours offered for a colour value (any other is typed as #rrggbb).
pub(crate) const SWATCHES: &[&str] = &["#ffffff", "#000000", "#8a8f98", "#ff3355", "#ffc928", "#22b45d", "#22bfc2", "#3678ef", "#8c4ce8", "#ec3d87"];

/// Values of a layer's own kind or of one effect that the inspector edits:
/// numbers (two-part values as two), and colours by their path.
#[derive(Default)]
pub(crate) struct Values {
    pub name: String,
    pub fields: Vec<Field>,
    /// Label and path of each colour.
    pub colors: Vec<(String, String)>,
}

/// Reads the editable values under `prefix` out of a JSON object: what is
/// a number, a pair of numbers or a colour, constant or keyed.
fn values_in(object: &Value, prefix: &str, name: String) -> Values {
    // A keyed value is judged by its first key.
    let shown = |v: &Value| v.get("keys").and_then(|k| k.get(0)).and_then(|k| k.get("v")).cloned().unwrap_or_else(|| v.clone());
    let mut out = Values { name, ..Default::default() };
    for (key, value) in object.as_object().into_iter().flatten() {
        let (path, label) = (format!("{prefix}{key}"), key.replace('_', " "));
        let step = match key.as_str() {
            "radius" | "size" | "distance" | "width" | "length" | "offset" => 0.25,
            "hue" | "angle" => 1.0,
            "stops" => 0.02,
            _ => 0.01,
        };
        match shown(value) {
            Value::Number(_) if key != "seed" => out.fields.push(Field { path: path.into(), component: None, label: label.into(), step, default: 0.0 }),
            Value::Array(pair) if pair.len() == 2 && pair.iter().all(Value::is_number) => {
                for (c, axis) in ["X", "Y"].into_iter().enumerate() {
                    out.fields.push(Field { path: path.clone().into(), component: Some(c), label: format!("{label} {axis}").into(), step, default: 0.0 });
                }
            }
            Value::String(text) if text.starts_with('#') => out.colors.push((label, path)),
            // A gradient's stops: each one's colour.
            Value::Array(stops) if key == "stops" => {
                for (i, stop) in stops.iter().enumerate() {
                    if stop.get(1).and_then(Value::as_str).is_some_and(|c| c.starts_with('#')) {
                        out.colors.push((format!("stop {} (at {})", i + 1, stop.get(0).and_then(Value::as_f64).unwrap_or(0.0)), format!("{path}.{i}.1")));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// What the inspector edits beyond the transform: a text's size, tracking
/// and colour, and each effect's values.
pub(crate) fn extra_values(layer: &Layer) -> (Values, Vec<Values>) {
    let mut own = Values::default();
    if let lahza_motion_core::document::LayerKind::Text(_) = &layer.kind {
        own.fields.push(Field { path: "size".into(), component: None, label: "Size".into(), step: 0.5, default: 96.0 });
        own.fields.push(Field { path: "tracking".into(), component: None, label: "Tracking".into(), step: 0.1, default: 0.0 });
        own.fields.push(Field { path: "line_height".into(), component: None, label: "Line height".into(), step: 0.005, default: 1.15 });
        own.colors.push(("Colour".into(), "color".into()));
        // The outline, when it has one: its width and colour.
        let stroke = serde_json::to_value(layer).ok().and_then(|v| v.get("stroke").cloned()).filter(Value::is_object);
        if let Some(stroke) = stroke {
            let outline = values_in(&stroke, "stroke.", String::new());
            own.fields.extend(outline.fields.into_iter().map(|f| Field { label: format!("Outline {}", f.label).into(), ..f }));
            own.colors.extend(outline.colors.into_iter().map(|(label, path)| (format!("Outline {label}"), path)));
        }
    }
    let json = serde_json::to_value(&layer.common.effects).unwrap_or_default();
    let effects = json
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, effect)| {
            let name = effect.get("type").and_then(Value::as_str).unwrap_or("effect").replace('_', " ");
            let mut values = values_in(effect, &format!("effects.{i}."), name);
            values.fields.retain(|f| !f.path.ends_with(".type"));
            values
        })
        .collect();
    (own, effects)
}

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
    /// The saved project compiled: what a drag's tick compiles one scene
    /// again from, and what a cancelled drag goes back to.
    saved_compiled: Arc<Compiled>,
    /// The one layer this drag's ticks have changed in `preview_project`
    /// (None once they changed anything else).
    preview_layer: Option<String>,
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
    /// The project as a drag shows it (unsaved): a copy made at the drag's
    /// first tick, in which each tick replaces the one layer it changes.
    preview_project: Option<Project>,
    /// The preview shows a request's "before" (`compare`), not a drag.
    comparing: bool,
    /// The preview quality the last frame was asked at.
    quality: motion_preview::Quality,
    cannot_trace: bool,
    /// Scenes whose simulation needs baking, found off the UI thread
    /// after each load (the newest answer wins).
    sim_needs: Vec<SimNeed>,
    sim_generation: u64,
    sim_inbox: Arc<Mutex<Option<(u64, Vec<SimNeed>)>>>,
    /// A bake is running; its result arrives here.
    baking: bool,
    bake_started: Option<Instant>,
    /// Whole seconds of the bake last shown.
    bake_shown: u64,
    bake_inbox: Arc<Mutex<Option<Result<String, String>>>>,
    font_names: Vec<String>,
    /// The selected layer as JSON, saved and as the drag shows it: values
    /// are read from these, so a draw does not serialise the layer again.
    saved_json: RefCell<Option<(String, Value)>>,
    preview_json: RefCell<Option<(String, Value)>>,
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
    // (`sim`: a bake made elsewhere shows without an edit.)
    for sub in ["scenes", "markers", "shaders", "sim"] {
        look(&dir.join(sub));
        if let Ok(rd) = std::fs::read_dir(dir.join(sub)) {
            for e in rd.flatten() {
                look(&e.path());
            }
        }
    }
    latest
}

/// The bundled fonts, then the project's font files (`assets/` and its
/// folders), as a text's `font` names them.
fn font_names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = lahza_motion_render::font::BUNDLED.iter().map(|f| f.to_string()).collect();
    let mut files = Vec::new();
    let mut pending = vec![(dir.join("assets"), 0)];
    while let Some((folder, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 2 {
                pending.push((path, depth + 1));
            } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf")) {
                if let Ok(rel) = path.strip_prefix(dir) {
                    files.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    files.sort();
    out.extend(files);
    out
}

/// A scene whose simulation is not what is drawn: `check`'s `simulation`
/// issues, for the editor.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SimNeed {
    /// Bodies and no bake: they stand where their keys put them.
    Unbaked { scene: String, bodies: usize },
    /// The bake was made from something else: the old simulation is drawn.
    Stale { scene: String, bodies: usize },
    /// It cannot be baked as it is.
    Broken { scene: String, error: String },
}

impl SimNeed {
    /// For the person, in a line.
    pub(crate) fn message(&self) -> String {
        match self {
            SimNeed::Unbaked { scene, bodies } => format!("Scene {scene}: {bodies} bodies are not simulated yet (they stand where their keys put them)."),
            SimNeed::Stale { scene, bodies } => format!("Scene {scene}: its {bodies} bodies or their settings changed since the bake; the old simulation is drawn."),
            SimNeed::Broken { scene, error } => format!("Scene {scene} cannot be simulated: {error}"),
        }
    }

    /// Whether a bake would settle it.
    pub(crate) fn bakeable(&self) -> bool {
        !matches!(self, SimNeed::Broken { .. })
    }
}

/// What needs baking in a project. Gathers every body (0.3 s at 500
/// bodies): call it off the UI thread.
pub(crate) fn sim_needs(compiled: &Compiled, dir: &Path) -> Vec<SimNeed> {
    use lahza_motion_core::compile::SimStatus;
    let mut out = Vec::new();
    for (i, scene) in compiled.scenes.iter().enumerate() {
        let scene = scene.id.to_string();
        match compiled.sim_status(i, Some(dir)) {
            SimStatus::None | SimStatus::Fresh { .. } => {}
            SimStatus::Unbaked { bodies } => out.push(SimNeed::Unbaked { scene, bodies: bodies as usize }),
            SimStatus::Stale { bodies } => out.push(SimNeed::Stale { scene, bodies: bodies as usize }),
            SimStatus::Broken(errors) => out.push(SimNeed::Broken { scene, error: errors.join("; ") }),
        }
    }
    out
}

/// Bakes every scene that needs it, through the `sim_bake` tool (a
/// toolbox of its own, so it can run off the UI thread). Says what was
/// baked.
pub(crate) fn bake_simulations(dir: &Path) -> Result<String, String> {
    let out = Toolbox::new().call("sim_bake", json!({"project": dir}))?.json;
    let baked: Vec<String> = out["scenes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["status"] == "baked")
        .map(|s| format!("{} ({} bodies, {} frames, {:.2} s)", s["scene"].as_str().unwrap_or(""), s["bodies"], s["frames"], s["seconds"].as_f64().unwrap_or(0.0)))
        .collect();
    Ok(if baked.is_empty() { "The simulation was up to date".into() } else { format!("Baked {}", baked.join(", ")) })
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
            saved_compiled: compiled.clone(),
            preview_layer: None,
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
            preview_project: None,
            comparing: false,
            quality: motion_preview::quality(),
            cannot_trace: false,
            sim_needs: Vec::new(),
            sim_generation: 0,
            sim_inbox: Arc::default(),
            baking: false,
            bake_started: None,
            bake_shown: 0,
            bake_inbox: Arc::default(),
            font_names: Vec::new(),
            saved_json: RefCell::default(),
            preview_json: RefCell::default(),
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
        editor.font_names = font_names(&editor.dir);
        editor.refresh_waves();
        editor.refresh_sim();
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
        let found = self.sim_inbox.lock().unwrap().take();
        if let Some((generation, needs)) = found {
            if generation == self.sim_generation && needs != self.sim_needs {
                self.sim_needs = needs;
                changed = true;
            }
        }
        let baked = self.bake_inbox.lock().unwrap().take();
        // A running bake: the seconds it has taken, as they pass.
        if self.baking && self.bake_seconds() != self.bake_shown {
            self.bake_shown = self.bake_seconds();
            changed = true;
        }
        if let Some(result) = baked {
            self.baking = false;
            self.bake_started = None;
            self.reload();
            self.status = result.unwrap_or_else(|e| format!("The simulation could not be baked: {e}"));
            changed = true;
        }
        self.poll_ticks += 1;
        if self.poll_ticks % 30 == 0 && !self.dragging() && stamp(&self.dir) > self.stamp {
            self.reload();
            self.status = "Updated from disk".into();
            changed = true;
        }
        if self.quality != motion_preview::quality() {
            self.dirty = true;
        }
        if self.dirty {
            self.request_frame();
        }
        let cannot_trace = self.worker.cannot_trace();
        if cannot_trace != self.cannot_trace {
            self.cannot_trace = cannot_trace;
            changed = true;
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
        // Half-size proxy while moving; full preview size when still, drawn
        // as the preview's quality says. A drag is always the raster proxy.
        self.quality = motion_preview::quality();
        let ask = motion_preview::ask(self.quality, self.playing, self.dragging());
        let scale = (fit * if ask.proxy { 0.5 } else { 1.0 }).max(0.05);
        self.request_key += 1;
        self.worker.request(Job { compiled: self.compiled.clone(), t: self.time, scale, ask, playing: self.playing, key: self.request_key });
        self.dirty = false;
    }

    /// Asks, off the UI thread, which scenes' simulations need baking.
    fn refresh_sim(&mut self) {
        self.sim_generation += 1;
        if !(0..self.compiled.scenes.len()).any(|i| self.compiled.has_simulation(i)) {
            self.sim_needs.clear();
            return;
        }
        let (compiled, dir, inbox, generation) = (self.compiled.clone(), self.dir.clone(), self.sim_inbox.clone(), self.sim_generation);
        std::thread::spawn(move || {
            let needs = sim_needs(&compiled, &dir);
            *inbox.lock().unwrap() = Some((generation, needs));
        });
    }

    /// Scenes whose simulation needs baking (as of the last load).
    pub(crate) fn sim_needs(&self) -> &[SimNeed] {
        &self.sim_needs
    }

    pub(crate) fn baking(&self) -> bool {
        self.baking
    }

    /// Seconds the running bake has taken so far.
    pub(crate) fn bake_seconds(&self) -> u64 {
        self.bake_started.filter(|_| self.baking).map_or(0, |t| t.elapsed().as_secs())
    }

    /// Bakes what needs it, off the UI thread; the result shows when it
    /// is done (`tick`).
    pub(crate) fn bake_simulation(&mut self) {
        if std::mem::replace(&mut self.baking, true) {
            return;
        }
        self.status = "Baking the simulation…".into();
        self.bake_started = Some(Instant::now());
        self.bake_shown = 0;
        let (dir, inbox) = (self.dir.clone(), self.bake_inbox.clone());
        std::thread::spawn(move || {
            let result = bake_simulations(&dir);
            *inbox.lock().unwrap() = Some(result);
        });
    }

    /// Whether anything is being dragged: on the canvas, or a value, a
    /// layer or a key from the studio (an unsaved preview shows).
    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some() || (self.preview_project.is_some() && !self.comparing)
    }

    /// The project as it shows now: a drag's unsaved copy, or the saved one.
    fn shown(&self) -> &Project {
        self.preview_project.as_ref().unwrap_or(&self.project)
    }

    fn drop_preview(&mut self) {
        self.comparing = false;
        self.preview_layer = None;
        self.preview_project = None;
        *self.preview_json.borrow_mut() = None;
    }

    fn reload(&mut self) {
        match io::load(&self.dir) {
            Ok(p) => {
                self.drop_preview();
                *self.saved_json.borrow_mut() = None;
                self.font_names = font_names(&self.dir);
                self.project = p;
                self.compiled = Arc::new(Compiled::new(&self.project));
                self.saved_compiled = self.compiled.clone();
                self.rows = layer_rows(&self.project, &self.compiled);
                self.stamp = stamp(&self.dir);
                self.dirty = true;
                self.refresh_waves();
                self.refresh_sim();
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
    /// A drag's ticks change one layer's fields: only that layer goes
    /// through JSON (`edit::apply_to_layer`), into a copy of the project
    /// made once a drag. Anything else takes `edit::apply`.
    fn preview(&mut self, ops: &[Value]) {
        // The layer the copy differs from the saved project in, when it is
        // one layer: then only its scene is compiled again
        // (`with_layer_changed`, which gives up whenever that could differ
        // from compiling everything).
        let mut one = None;
        match edit::apply_to_layer(&self.project, ops) {
            Some(Ok((id, layer))) => {
                let first = self.preview_project.is_none();
                let copy = self.preview_project.get_or_insert_with(|| self.project.clone());
                edit::replace_layer(copy, layer);
                if first || self.preview_layer.as_deref() == Some(id.as_str()) {
                    one = Some(id);
                }
            }
            Some(Err(_)) => return,
            None => match edit::apply(&self.project, ops) {
                Ok(a) => self.preview_project = Some(a.project),
                Err(_) => return,
            },
        }
        self.preview_layer = one.clone();
        *self.preview_json.borrow_mut() = None;
        let compiled = one.and_then(|id| self.saved_compiled.with_layer_changed(self.shown(), &id)).unwrap_or_else(|| Compiled::new(self.shown()));
        self.compiled = Arc::new(compiled);
        self.dirty = true;
    }

    fn undo(&mut self, redo: bool) {
        // An agent's request is one step.
        let args = if redo { json!({"project": self.dir, "redo": true}) } else { json!({"project": self.dir, "undo": 1, "by_request": true}) };
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

    /// `path` of layer `id` as JSON, from the saved project or (`shown`)
    /// from what a drag shows. The layer is serialised once and kept until
    /// it changes.
    fn layer_value(&self, id: &str, path: &str, shown: bool) -> Option<Value> {
        let shown = shown && self.preview_project.is_some();
        let (cell, project) = if shown { (&self.preview_json, self.shown()) } else { (&self.saved_json, &self.project) };
        let mut cache = cell.borrow_mut();
        if cache.as_ref().is_none_or(|(cached, _)| cached != id) {
            *cache = Some((id.to_string(), serde_json::to_value(find_layer(project, id)?).ok()?));
        }
        cache.as_ref()?.1.pointer(&format!("/{}", path.replace('.', "/"))).cloned()
    }

    /// The saved value (what ops are made from).
    fn eval<T: Animatable>(&self, id: &str, path: &str, default: T) -> (T, bool) {
        self.eval_in(id, path, default, false)
    }

    /// `path` at the playhead and whether it is keyed; `shown`: as a drag
    /// shows it.
    fn eval_in<T: Animatable>(&self, id: &str, path: &str, default: T, shown: bool) -> (T, bool) {
        let v = self.layer_value(id, path, shown);
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

    fn field_ops(&self, id: &str, field: &Field, value: f64) -> Vec<Value> {
        match field.component {
            Some(c) => {
                let (mut v, animated) = self.eval::<Vec2>(id, &field.path, Vec2([field.default; 2]));
                v.0[c] = value;
                self.value_ops(id, &field.path, json!([round(v.0[0]), round(v.0[1])]), animated)
            }
            None => {
                let (_, animated) = self.eval::<f64>(id, &field.path, field.default);
                self.value_ops(id, &field.path, json!(round(value)), animated)
            }
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
            self.cancel_preview();
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
    /// One evaluation; during a drag, the value the drag shows.
    pub(crate) fn field_state(&self, id: &str, field: &Field) -> (f64, bool) {
        match field.component {
            Some(c) => {
                let (v, animated) = self.eval_in::<Vec2>(id, &field.path, Vec2([field.default; 2]), true);
                (v.0[c], animated)
            }
            None => self.eval_in::<f64>(id, &field.path, field.default, true),
        }
    }

    /// Shows `field` of `layer` at `value` (unsaved).
    pub(crate) fn drag_value(&mut self, layer: &str, field: &Field, value: f64) {
        let ops = self.field_ops(layer, field, value);
        self.preview(&ops);
    }

    /// Saves `field` of `layer` at `value`: a key at the playhead when the
    /// property is animated, otherwise a plain value.
    pub(crate) fn drop_value(&mut self, layer: &str, field: &Field, value: f64) {
        let ops = self.field_ops(layer, field, value);
        self.commit(ops);
    }

    /// The colour at `path` of layer `id` at the playhead, as #rrggbb.
    pub(crate) fn color_state(&self, id: &str, path: &str) -> String {
        self.eval_in::<Color>(id, path, Color::WHITE, true).0.to_hex()
    }

    /// Saves a colour: a key at the playhead when it is animated.
    pub(crate) fn set_color(&mut self, id: &str, path: &str, hex: &str) {
        if Color::parse(hex).is_err() {
            self.status = format!("'{hex}' is not a colour: use #rrggbb");
            return;
        }
        let (_, animated) = self.eval::<Color>(id, path, Color::WHITE);
        let ops = self.value_ops(id, path, json!(hex), animated);
        self.commit(ops);
    }

    /// Fonts a text can take: the bundled ones and the font files in the
    /// project's assets.
    pub(crate) fn fonts(&self) -> &[String] {
        &self.font_names
    }

    /// Shows edit ops without saving them (text as it is typed).
    pub(crate) fn preview_ops(&mut self, ops: &[Value]) {
        self.preview(ops);
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The preview was asked traced (or with the sky) on a GPU that cannot.
    pub(crate) fn cannot_trace(&self) -> bool {
        self.cannot_trace
    }

    /// The scene at the playhead.
    pub(crate) fn scene_at_playhead(&self) -> Option<String> {
        self.compiled.scene_at(self.time).map(|s| s.id.to_string())
    }

    /// The frame showing now, as a PNG (what an agent is sent to look at).
    pub(crate) fn frame_png(&self) -> Option<Vec<u8>> {
        let frame = self.frame.as_ref()?;
        let size = frame.size(0);
        let (w, h) = (size.width.0 as u32, size.height.0 as u32);
        // The frame is BGRA (GPUI's upload order).
        let mut pixels = frame.as_bytes(0)?.to_vec();
        for p in pixels.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        let mut png = Vec::new();
        image::RgbaImage::from_raw(w, h, pixels)?.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
        Some(png)
    }

    /// Takes back every edit made for `request` (one undo; redo puts them
    /// back).
    pub(crate) fn undo_request(&mut self, request: &str) -> Result<(), String> {
        let result = self.toolbox.call("history", json!({"project": self.dir, "undo_request": request})).map(|_| ());
        self.status = match &result {
            Ok(()) => "Undone".into(),
            Err(e) => e.clone(),
        };
        self.reload();
        result
    }

    /// Before and after: while `request` is given the preview shows the
    /// project without that request's edits (nothing is saved); None
    /// shows it as it is.
    pub(crate) fn compare(&mut self, request: Option<&str>) {
        let Some(request) = request else {
            if std::mem::take(&mut self.comparing) {
                self.cancel_preview();
            }
            return;
        };
        let Ok(history) = io::read_history(&self.dir) else { return };
        let ops: Vec<Value> = history.iter().rev().filter(|e| e.request.as_deref() == Some(request) && !e.undone).flat_map(|e| e.inverse.iter().cloned()).collect();
        if let Ok(before) = edit::apply(&self.project, &ops) {
            self.preview_project = Some(before.project);
            *self.preview_json.borrow_mut() = None;
            self.comparing = true;
            self.compiled = Arc::new(Compiled::new(self.shown()));
            self.dirty = true;
        }
    }

    /// Drops any unsaved drag preview.
    pub(crate) fn cancel_preview(&mut self) {
        self.drop_preview();
        self.compiled = self.saved_compiled.clone();
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
    fn a_missing_or_stale_bake_is_found_and_baked() {
        let dir = std::env::temp_dir().join(format!("lahza-motion-editor-sim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut tb = Toolbox::new();
        tb.cwd = dir.clone();
        tb.call("project_create", json!({"path": "drop", "duration": 2, "width": 640, "height": 360})).unwrap();
        tb.call("edit_apply", json!({"ops": [
            {"op": "add_scene", "id": "s1", "simulation": {"ground": {"y": 330}}},
            {"op": "add", "id": "crate", "kind": "mesh", "mesh": {"type": "box", "size": [60, 60], "depth": 60}, "transform": {"position": [320, 60], "rotation": 12}, "body": {"bounce": 0.3}},
            {"op": "add_scene", "id": "s2", "start": 1},
            {"op": "add", "scene": "s2", "id": "plain", "kind": "shape", "shape": "rect"}
        ]}))
        .unwrap();
        let project = dir.join("drop.lahza");
        let needs = |project: &Path| sim_needs(&Compiled::new(&io::load(project).unwrap()), project);
        assert_eq!(needs(&project), [SimNeed::Unbaked { scene: "s1".into(), bodies: 1 }]);
        assert!(needs(&project)[0].bakeable() && needs(&project)[0].message().contains("not simulated yet"));
        // A bake made elsewhere changes what the editor polls.
        let before = stamp(&project);
        std::thread::sleep(Duration::from_millis(20));
        let said = bake_simulations(&project).unwrap();
        assert!(said.starts_with("Baked s1 (1 bodies, "), "{said}");
        assert!(stamp(&project) > before, "the editor reloads after a bake");
        assert!(needs(&project).is_empty());
        assert_eq!(bake_simulations(&project).unwrap(), "The simulation was up to date");
        // The body dragged somewhere else: the bake is stale.
        tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "crate.transform.position", "value": [200, 40]}]})).unwrap();
        assert_eq!(needs(&project), [SimNeed::Stale { scene: "s1".into(), bodies: 1 }]);
        bake_simulations(&project).unwrap();
        assert!(needs(&project).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn every_effect_offered_is_one_the_format_takes() {
        use lahza_motion_core::document::Effect;
        for (label, json) in EFFECTS {
            let effect: Effect = serde_json::from_str(json).unwrap_or_else(|e| panic!("{label}: {e}"));
            // And each shows something to change.
            let values = values_in(&serde_json::to_value(&effect).unwrap(), "effects.0.", label.to_string());
            assert!(!values.fields.is_empty() || !values.colors.is_empty(), "{label} has no values");
        }
        // A shadow: its numbers, its offset as two, its colour by path.
        let shadow: Effect = serde_json::from_str(EFFECTS[1].1).unwrap();
        let values = values_in(&serde_json::to_value(&shadow).unwrap(), "effects.2.", "shadow".into());
        let labels: Vec<(&str, Option<usize>)> = values.fields.iter().map(|f| (&*f.label, f.component)).collect();
        assert_eq!(labels, [("offset X", Some(0)), ("offset Y", Some(1)), ("radius", None)]);
        assert_eq!(values.fields[0].path, "effects.2.offset");
        assert_eq!(values.colors, [("color".to_string(), "effects.2.color".to_string())]);
        for hex in SWATCHES {
            Color::parse(hex).unwrap();
        }
        // A gradient map: a colour for each stop, by its place in the list.
        let map: Effect = serde_json::from_str(EFFECTS.iter().find(|(label, _)| *label == "Gradient map").unwrap().1).unwrap();
        let values = values_in(&serde_json::to_value(&map).unwrap(), "effects.0.", "gradient map".into());
        assert_eq!(values.colors.iter().map(|(_, path)| path.as_str()).collect::<Vec<_>>(), ["effects.0.stops.0.1", "effects.0.stops.1.1"]);
        // A text: its line height, and its outline once it has one.
        let text: Layer = serde_json::from_value(json!({"id": "t", "kind": "text", "text": "Hi"})).unwrap();
        let (own, _) = extra_values(&text);
        assert_eq!(own.fields.iter().map(|f| &*f.label).collect::<Vec<_>>(), ["Size", "Tracking", "Line height"]);
        let outlined: Layer = serde_json::from_value(json!({"id": "t", "kind": "text", "text": "Hi", "stroke": {"color": "#000000", "width": 4}})).unwrap();
        let (own, _) = extra_values(&outlined);
        assert!(own.fields.iter().any(|f| f.label == "Outline width" && f.path == "stroke.width"), "{:?}", own.fields.iter().map(|f| &*f.label).collect::<Vec<_>>());
        assert!(own.colors.contains(&("Outline color".to_string(), "stroke.color".to_string())));
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
                // On a software GPU the first frame builds its
                // pipelines: tens of seconds, cold.
                for _ in 0..3600 {
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
                        let field = &FIELDS[5];
                        editor.drag_value("title", field, 0.25);
                        // Mid-drag: the proxy is on and the number follows.
                        assert!(editor.dragging(), "a value drag is a drag");
                        assert!((editor.field_state("title", field).0 - 0.25).abs() < 1e-9, "the number at once");
                        editor.drag_value("title", field, 0.5);
                        editor.drop_value("title", field, 0.5);
                        assert!(!editor.dragging());
                        // A keyed value dragged twice leaves one new key, not two.
                        editor.move_layer("box", [210.0, 190.0], false);
                        editor.move_layer("box", [220.0, 200.0], false);
                        assert_eq!(editor.position_of("box").x(), 200.0, "ops are made from the saved project");
                        let shown = serde_json::to_value(find_layer(editor.shown(), "box").unwrap()).unwrap();
                        assert_eq!(shown["transform"]["position"]["keys"].as_array().unwrap().len(), 3);
                        editor.cancel_preview();
                        assert!(!editor.dragging());
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
