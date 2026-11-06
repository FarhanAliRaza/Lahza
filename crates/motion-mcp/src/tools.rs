//! The tools, independent of transport. The MCP server, the CLI and tests
//! all call [`Toolbox::call`]. Projects live on disk; every tool re-reads
//! files that changed since it last looked, so a person editing the same
//! project (or the Lahza editor) and the agent never diverge.

use base64::Engine as _;
use lahza_motion_core::catalog::style_pack;
use lahza_motion_core::check::{self, Options as CheckOptions};
use lahza_motion_core::compile::Compiled;
use lahza_motion_core::document::{Asset, AudioClip, Project, RendererKind};
use lahza_motion_core::edit;
use lahza_motion_core::io::{self, HistoryEntry};
use lahza_motion_core::markers::{Marker, MarkerTrack, Markers};
use lahza_motion_core::recording;
use lahza_motion_core::time::{Fps, TimeBase};
use lahza_motion_core::timeref::TimeRef;
use lahza_motion_core::value::Color;
use lahza_motion_render::export::{self, ExportOptions, ExportReport, Progress, Quality};
use lahza_motion_render::font::Fonts;
use lahza_motion_render::{sheet, Backend, RenderOptions, Renderer};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

pub type ToolResult = Result<Output, String>;

#[derive(Default, Debug)]
pub struct Output {
    pub json: Value,
    /// PNG images.
    pub images: Vec<Vec<u8>>,
}

impl Output {
    fn json(json: Value) -> Self {
        Self { json, images: Vec::new() }
    }
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
    pub read_only: bool,
}

const PROJECT: &str = "Project folder (*.lahza). Optional after project_open/project_create: defaults to the current project.";

fn obj(props: Value, required: &[&str]) -> Value {
    let mut props = props;
    props.as_object_mut().unwrap().insert("project".into(), json!({"type": "string", "description": PROJECT}));
    json!({"type": "object", "properties": props, "required": required})
}

pub fn tool_defs() -> Vec<ToolDef> {
    let time = json!({"description": "seconds or a time reference (\"beat:17\", \"word:canal\", \"#title.end + 0.25\")", "type": ["number", "string"]});
    vec![
        ToolDef {
            name: "project_create",
            description: "Create a motion project folder. Optionally copy a music file into assets/ and register it. Then add scenes and layers with edit_apply. With `recording` (a Lahza screen recording, *.lahzarec) the project is created inside it as a motion piece: sized to the recording, sharing its files (the raw recording is \"../../screen.mkv\"; media_import puts other files in the recording's shared media/). Place it on the video with recording_place.",
            schema: json!({"type": "object", "required": ["path"], "properties": {
                "path": {"type": "string", "description": "new folder; \".lahza\" is appended when missing. With `recording`, just a name (\"intro\")"},
                "recording": {"type": "string", "description": "a *.lahzarec folder to create the piece in"},
                "width": {"type": "integer", "default": 1920}, "height": {"type": "integer", "default": 1080},
                "fps": {"type": ["number", "string"], "default": 30}, "duration": {"type": "number", "default": 10},
                "style": {"type": "string", "description": "style pack (see lahza://styles)"},
                "background": {"type": "string", "description": "colour, default from the style"},
                "music": {"type": "string", "description": "audio file to copy in as the music track"},
                "licence": {"type": "string", "description": "licence of the music (default user-supplied)"},
                "overwrite": {"type": "boolean"}}}),
            read_only: false,
        },
        ToolDef {
            name: "sfx_add",
            description: "Add a sound effect at one or more times. `sound` is a built-in effect (whoosh, whoosh-out, swipe, pop, click, tick, type, thud, hit, boom, riser, ding, sparkle, glitch; see \"sounds\" in lahza://catalog) or an audio file. Anchor it to the animation it belongs to so it moves with it: \"#card.start - 3f\" (a whoosh leads its move), \"#title.end\", \"beat:9\", \"hit:2\". A riser should end where the reveal lands: at = \"#reveal.start - 1.6\". Each placement is a sound with an id (whoosh1, whoosh2…).",
            schema: obj(json!({
                "sound": {"type": "string"},
                "at": {"description": "a time, or a list of times", "type": ["number", "string", "array"]},
                "gain_db": {"type": "number", "default": 0},
                "length": {"type": "number", "description": "seconds to play"},
                "trim": {"type": "number", "description": "seconds skipped at the start of the file"},
                "fade_in": {"type": "number"}, "fade_out": {"type": "number"},
                "name": {"type": "string"}}), &["sound", "at"]),
            read_only: false,
        },
        ToolDef {
            name: "recording_sound_add",
            description: "Add sounds to a Lahza screen recording's own video timeline (its Music / SFX lane): a built-in effect or an audio file at seconds on the edited video. recording_info lists clicks, keystrokes and zooms in those seconds, e.g. a click on every click or a whoosh on every zoom. The person sees them live and can move, trim or delete them.",
            schema: obj(json!({
                "recording": {"type": "string"},
                "sound": {"type": "string"},
                "at": {"description": "seconds on the edited video, or a list", "type": ["number", "array"]},
                "gain_db": {"type": "number", "default": 0},
                "length": {"type": "number"}}), &["recording", "sound", "at"]),
            read_only: false,
        },
        ToolDef {
            name: "recording_info",
            description: "A Lahza screen recording (*.lahzarec): its size, the edited video's duration, transcript availability, shared media files (as paths usable from a piece inside it) and the motion pieces placed on its timeline.",
            schema: obj(json!({"recording": {"type": "string"}}), &["recording"]),
            read_only: true,
        },
        ToolDef {
            name: "recording_place",
            description: "Place the current motion piece (a project made with project_create {recording}) on its recording's video timeline, move it, or remove it. `overlay` draws it over the video (keep the background transparent, \"#00000000\"); `full` covers the frame like an inserted clip. The studio shows the change live; its sound joins the video's mix.",
            schema: obj(json!({
                "start": {"type": "number", "description": "seconds on the edited video"},
                "placement": {"type": "string", "enum": ["overlay", "full"], "default": "overlay"},
                "opacity": {"type": "number", "default": 1},
                "name": {"type": "string", "description": "label on the timeline"},
                "remove": {"type": "boolean"}}), &[]),
            read_only: false,
        },
        ToolDef { name: "project_open", description: "Open an existing project folder and make it current.", schema: json!({"type": "object", "required": ["path"], "properties": {"path": {"type": "string"}}}), read_only: true },
        ToolDef { name: "project_info", description: "Size, fps, duration, scenes with resolved times, marker tracks, assets with licences, audio, compile errors.", schema: obj(json!({}), &[]), read_only: true },
        ToolDef {
            name: "timeline_get",
            description: "Compact view of scenes and layers with ids, kinds, resolved times and which properties are animated. full=true returns the raw scene JSON (what edit_apply edits).",
            schema: obj(json!({"scene": {"type": "string"}, "from": {"type": "number"}, "to": {"type": "number"}, "full": {"type": "boolean"}}), &[]),
            read_only: true,
        },
        ToolDef {
            name: "markers_query",
            description: "Marker tracks (beat, bar, half, sixteenth, triplet, onset, onset.low/mid/high, hit, stop, drop, music.start/end, word, sentence, user). Without track: list tracks. Each marker comes with the reference to use in times, e.g. \"beat:17\".",
            schema: obj(json!({"track": {"type": ["string", "array"], "items": {"type": "string"}}, "from": {"type": "number"}, "to": {"type": "number"}, "text": {"type": "string", "description": "label contains (words, OCR)"}, "limit": {"type": "integer", "default": 200}}), &[]),
            read_only: true,
        },
        ToolDef {
            name: "markers_import",
            description: "Add marker tracks. kind=transcript reads word timestamps (a .lahzarec recording folder, a Lahza transcript JSON or whisper.cpp JSON) into word and sentence tracks, shifted by offset; kind=user stores markers you give (times or {t, label}) as a named track.",
            schema: obj(json!({"kind": {"type": "string", "enum": ["transcript", "user"]}, "path": {"type": "string"}, "offset": {"type": "number", "default": 0},
                "name": {"type": "string", "default": "user"}, "markers": {"type": "array", "items": {"type": ["number", "object"]}}}), &["kind"]),
            read_only: false,
        },
        ToolDef {
            name: "narration_import",
            description: "Import narration from a manifest: one clip per line, each {id, audio, text, at?, pause?, words: [{text, start, end}] | chars: [{char, start, end}]} with the voice provider's timings (seconds from the clip's start). Script text is as shown (\"81\", \"U.S.\") and may carry [cue] markup (\"Over [strike]Iran.\", \"[map]a span[/map]\"). Builds the narration audio and the line (line:<id>), sentence, word (word:iran, also matched by its spoken form) and cue (cue:strike, cue:strike.end) marker tracks. Clips without `at` follow the previous one after `pause` (default 0.35 s; the first starts at `lead`, default 0.5 s). Unchanged clips (same audio, text and timings) are reused; re-importing after a script change moves everything referenced to the narration's markers. Reports timing problems (also errors in check): words with no time, out of order, outside the clip, or in silence. Time narrated pieces with cue:/word:/line: references, never literal seconds.",
            schema: obj(json!({"manifest": {"type": "string", "description": "manifest JSON file"}}), &["manifest"]),
            read_only: false,
        },
        ToolDef {
            name: "edit_apply",
            description: "Apply a batch of edit ops atomically as one undoable step: add, add_scene, set, remove, move, key, anchor, group, ungroup, add_sound, insert_time {at, duration} (ripples every literal time at or after `at`: scenes, layers, keys, animators, sounds, markers, narration; references move on their own), shift {ids, by} (moves scenes, layers or sounds with everything in them) (see lahza://schema and lahza://catalog). Sounds have ids too: set \"whoosh1.at\", remove \"whoosh1\". Returns created ids, the change summary and compile errors.",
            schema: obj(json!({"ops": {"type": "array", "items": {"type": "object"}}, "author": {"type": "string", "default": "agent"}}), &["ops"]),
            read_only: false,
        },
        ToolDef {
            name: "check",
            description: "Structure, timing (measured sync drift per anchored key on the frame grid, long holds), layout (title-safe, overlapping text, contrast) and licences. ok=false means errors.",
            schema: obj(json!({"tolerance_frames": {"type": "number", "default": 1}, "verbose": {"type": "boolean"}}), &[]),
            read_only: true,
        },
        ToolDef {
            name: "sync_check",
            description: "Visual events (cuts, layers appearing, keys landing, fill/opacity/value jumps) on the frame grid, each with its nearest marker or sound effect. Flags `drift`: authored against a marker (word:, cue:, beat:, …) but lands more than `frames` off it; `unsynced`: no marker within `frames` (a hit with nothing to hit). Use it instead of eyeballing audio loudness to confirm a narrated or beat-cut piece lands; fix flags by keying to the marker (\"at\": \"word:iran\") rather than nudging seconds.",
            schema: obj(json!({"frames": {"type": "number", "default": 2, "description": "allowed distance in frames"},
                "tracks": {"type": "array", "items": {"type": "string"}, "description": "marker tracks to measure against (\"sound\" = sound effects); default all"},
                "element": {"type": "string", "description": "only this layer or scene id (and its children, <id>/…)"},
                "from": {"type": "number"}, "to": {"type": "number"},
                "flagged": {"type": "boolean", "description": "only flagged events (default false)"},
                "limit": {"type": "integer", "default": 200}}), &[]),
            read_only: true,
        },
        ToolDef {
            name: "snapshot",
            description: "Render frames as images: at given times/references, at a track's markers, or (default) at the project's sync points. sheet=true returns one labelled contact sheet.",
            schema: obj(json!({"at": {"type": "array", "items": time}, "track": {"type": "string"}, "from": {"type": "number"}, "to": {"type": "number"},
                "sheet": {"type": "boolean"}, "width": {"type": "integer", "description": "px per frame (default 960, 360 in sheets)"}, "save": {"type": "string", "description": "also write the PNG here"},
                "pt": {"type": "boolean", "description": "path-trace meshes, as a path-traced export draws them (default false: the preview's raster)"}, "samples": {"type": "integer", "description": "with pt: paths per pixel (default the project's samples, else 64)"},
                "denoise": {"type": "boolean", "description": "with pt: filter the noise (default true)"}}), &[]),
            read_only: true,
        },
        ToolDef {
            name: "fit_camera",
            description: "Solve the scene camera from marks: in each reference frame, 3D scene points (px, like a layer's position and z) and where they appear (px). Levenberg–Marquardt through the engine's own camera; returns per frame the camera (fov, orbit, dolly, position, shift) and its rms error, and `set` ops putting them on a scene's camera as keys (apply=true applies them). Fixed parameters (fix) stay at `start`; shift is fixed by default.",
            schema: obj(json!({"frames": {"type": "array", "items": {"type": "object", "properties": {"at": time, "marks": {"type": "array", "items": {"type": "object", "properties": {"world": {"type": "array", "items": {"type": "number"}}, "image": {"type": "array", "items": {"type": "number"}}}}}}}},
                "start": {"type": "object", "description": "camera to start from: fov, orbit, dolly, position, shift (default the default camera)"},
                "fix": {"type": "array", "items": {"type": "string", "enum": ["fov", "orbit", "dolly", "position", "shift"]}, "description": "parameters to keep (default [\"shift\"])"},
                "scene": {"type": "string", "description": "scene whose camera the ops set"}, "apply": {"type": "boolean"}}), &["frames"]),
            read_only: false,
        },
        ToolDef {
            name: "compare",
            description: "Compare the project with a reference video. Without `at` it compares every frame from first to last: per frame the difference and how much each side moves, a per-scene summary with a verdict (motion, look, missing/extra detail, and interior shading: shading_ratio is ours ÷ reference, \"ours is flatter\" when below 0.5 against a reference that has real shading), the worst stretches (with a sheet of them), a timeline graph, and a side-by-side video (ours | reference | difference) of the whole piece. With `at` it returns those moments side by side. The reference time is ours + offset.",
            schema: obj(json!({"against": {"type": "string", "description": "reference video or image"}, "at": {"type": "array", "items": time, "description": "only these moments (default: every frame)"},
                "from": {"type": "number"}, "to": {"type": "number"},
                "offset": {"type": "number", "description": "reference seconds = ours + offset (default 0)"},
                "width": {"type": "integer", "description": "px per frame (default 360 for moments, 270 for a full run)"},
                "video": {"type": "string", "description": "full run: side-by-side video path (default renders/compare.mp4)"},
                "save": {"type": "string", "description": "also write the first PNG here"}}), &["against"]),
            read_only: true,
        },
        ToolDef {
            name: "render",
            description: "Export video (mp4, webm or gif by extension) in the background; poll render_status. quality=draft renders half size without motion blur. pt=true (or settings.renderer \"path\") path-traces meshes on a GPU with ray queries (else rasterised, with a warning).",
            schema: obj(json!({"output": {"type": "string", "description": "default renders/<name>.mp4 in the project"}, "quality": {"type": "string", "enum": ["draft", "final"], "default": "final"},
                "from": {"type": "number"}, "to": {"type": "number"}, "audio": {"type": "boolean", "default": true}, "wait": {"type": "boolean"},
                "pt": {"type": "boolean", "description": "true: path-trace meshes; false: rasterise them; default the project's settings.renderer"}, "samples": {"type": "integer", "description": "path tracing: paths per pixel (default the project's samples, else 64; drafts trace a quarter)"},
                "denoise": {"type": "boolean", "description": "path tracing: filter the noise (default true; frames also build on the previous ones)"}}), &[]),
            read_only: false,
        },
        ToolDef { name: "render_status", description: "Progress and result of render jobs.", schema: json!({"type": "object", "properties": {"job": {"type": "string"}}}), read_only: true },
        ToolDef {
            name: "history",
            description: "Recent edits by person and agent; undo the last n steps or back to an entry; redo the last undo.",
            schema: obj(json!({"limit": {"type": "integer", "default": 20}, "undo": {"type": "integer"}, "undo_to": {"type": "integer"}, "redo": {"type": "boolean"}}), &[]),
            read_only: false,
        },
        ToolDef {
            name: "media_import",
            description: "Copy footage (video file, image, image sequence \"shot_####.png\" or a folder of frames) into the project's assets, record its source and licence, and report size, fps, duration, alpha and audio. Then add it with a {\"kind\": \"video\", \"src\": …} layer.",
            schema: obj(json!({"path": {"type": "string"}, "licence": {"type": "string", "description": "default user-supplied"}, "source": {"type": "string", "description": "where it came from (default user)"}, "copy": {"type": "boolean", "default": true}}), &["path"]),
            read_only: false,
        },
        ToolDef {
            name: "blender_run",
            description: "Run Blender headless to make 3D footage: give a bpy Python `script` (it can build or load a scene) and/or a `blend` file. Lahza sets resolution, fps, frame range, transparent film, a PNG RGBA sequence output, the GPU (OptiX/CUDA/HIP devices on, CPU when none) and the quality first; your script may override them. The script sees the dict LAHZA (out, width, height, fps, frame_start, frame_end, at, quality) with the project's timing: LAHZA[\"markers\"][\"beat\"] = {\"seconds\": […], \"frames\": […]} for every marker track (beat, bar, hit, word with \"labels\", …) and LAHZA[\"elements\"][\"title\"] = {start, end, start_frame, end_frame} for every scene and layer (by id and name), plus frame_of(seconds); frames count from frame_start at project time `at`, so key 3D moves on those frames to land on the timeline. quality \"preview\" renders a few frames (preview_frames) at half size, EEVEE (Cycles 8 samples without it), no motion blur, into assets/blender/<name>-preview/ and returns a sheet of them; \"final\" renders every frame with `samples`, denoising and motion blur into assets/blender/<name>/ and returns the footage src for a video layer. Reports seconds per frame. Long renders: wait=false and poll render_status.",
            schema: obj(json!({
                "name": {"type": "string", "description": "output name (letters, digits, - _)"},
                "script": {"type": "string", "description": "bpy Python run after the defaults are set"},
                "blend": {"type": "string", "description": ".blend file to open first"},
                "quality": {"type": "string", "enum": ["preview", "final"], "default": "final", "description": "preview: a few half-size frames to check the look and timing; final: the footage"},
                "preview_frames": {"type": "integer", "default": 6},
                "frames": {"type": "array", "items": {"type": "integer"}, "description": "[start, end], default [1, fps × project duration]"},
                "at": {"type": "number", "default": 0, "description": "project seconds shown by frame_start (where the footage layer starts); marker frames count from it"},
                "resolution": {"type": "array", "items": {"type": "integer"}, "description": "[w, h], default the project size"},
                "engine": {"type": "string", "enum": ["CYCLES", "BLENDER_EEVEE", "BLENDER_WORKBENCH"], "description": "default CYCLES for final, EEVEE for preview"},
                "samples": {"type": "integer", "default": 64, "description": "final quality (preview uses 8)"},
                "gpu": {"type": "boolean", "default": true, "description": "false renders on the CPU"},
                "transparent": {"type": "boolean", "default": true},
                "render": {"type": "boolean", "default": true, "description": "false only runs the script (e.g. to export a glTF)"},
                "timeout": {"type": "number", "description": "seconds, default 1800"},
                "wait": {"type": "boolean", "default": true}}), &["name"]),
            read_only: false,
        },
        ToolDef {
            name: "track",
            description: "Follow a target through a video (a reference) and measure it per frame: t, frame, x/y (centroid), bbox [x, y, w, h], area, angle (principal axis from image moments, degrees clockwise) and mean colour, in source pixels. Target: `color` \"#rrggbb\" ± `tolerance` (RGB distance, default 40; `region` [x, y, w, h] limits the search), or `point` [x, y] (the colour blob under it in the first frame, followed as the connected region nearest its last position), or `box` [x, y, w, h] (that picture matched frame to frame within `search` px; translation only, angle 0). With `keys` it also returns transform keys (position, scale from the area, rotation from the angle, one per frame) and, given keys.layer, edit_apply ops that set them. Use it instead of measuring frames by hand; feed a move's samples to fit_curve to get one eased key instead of per-frame keys.",
            schema: obj(json!({
                "video": {"type": "string", "description": "video file (absolute, or relative to the working folder or project)"},
                "color": {"type": "string"}, "tolerance": {"type": "number", "default": 40},
                "region": {"type": "array", "items": {"type": "number"}, "description": "[x, y, w, h] in source pixels"},
                "point": {"type": "array", "items": {"type": "number"}, "description": "[x, y] in source pixels on the first frame"},
                "box": {"type": "array", "items": {"type": "number"}, "description": "[x, y, w, h] in source pixels on the first frame"},
                "search": {"type": "number", "description": "box: pixels to search per frame (default the box size)"},
                "from": {"type": "number", "default": 0}, "to": {"type": "number"},
                "width": {"type": "integer", "description": "processing width (default source, at most 960)"},
                "keys": {"type": "object", "description": "{scale: canvas px per source px (default 1), offset: [x, y], at: timeline seconds of `from` (default 0), base_scale: 1, rotate: true, layer: id} → keys and ops"}}), &["video"]),
            read_only: true,
        },
        ToolDef {
            name: "fit_curve",
            description: "Fit the engine's eases to a measured move: `samples` [[t, value], …] with a number or [x, y] per sample (positions are fitted along the line of the move; e.g. track's x/y). Tries linear, power in/out/in-out (n), exp (tau), spring (zeta, hz), bezier, bounce and every named preset, searching start, duration and the ease's parameters; from/to by least squares. Returns `best` with ease, start, end, duration, from, to, rms_error, relative_error (of the travel) and `key` — {at, anchor: \"start\", dur, from, v, ease}, ready for a key op — plus the runners-up. A named preset or simpler ease wins when it fits about as well.",
            schema: obj(json!({
                "samples": {"type": "array", "items": {"type": "array"}, "description": "[[t, value], …], value a number or [x, y]"},
                "fps": {"type": "number", "description": "frame rate for frame-based eases (default the project's, else 30)"}}), &["samples"]),
            read_only: true,
        },
        ToolDef {
            name: "audio_analyze",
            description: "Analyse music into marker tracks: tempo (a tempo map when it changes), meter (3/4 or 4/4), first downbeat, beat/bar/half/sixteenth/triplet grids, onsets per band, hit, stop (silence→hit, or the music cutting to a reverb tail or ringing hit), drop (bass dropout before a downbeat), music.start/end. Reports the feel (straight, or triplet for a shuffle or compound swing) with feel_confidence, and tempo_confidence with the alternatives: other groupings of the same pulse (3:4, 2:3; e.g. 133 BPM in triplets vs 100 BPM in sixteenths) when the onsets cannot tell them apart. Defaults to the project's music.",
            schema: obj(json!({"path": {"type": "string", "description": "audio file; registered as the music when the project has none"}, "offset": {"type": "number", "description": "timeline time of the file's start"}}), &[]),
            read_only: false,
        },
    ]
}

struct Open {
    project: Project,
    stamp: SystemTime,
    compiled: Option<Arc<Compiled>>,
    renderer: Option<Renderer>,
    fonts: Option<Fonts>,
}

struct Job {
    project: PathBuf,
    output: PathBuf,
    progress: Arc<Progress>,
    started: Instant,
    handle: Option<std::thread::JoinHandle<Result<Value, String>>>,
    result: Option<Result<Value, String>>,
}

pub struct Toolbox {
    current: Option<PathBuf>,
    projects: HashMap<PathBuf, Open>,
    jobs: Vec<(String, Job)>,
    pub backend: Backend,
    /// Accepts relative paths against this folder.
    pub cwd: PathBuf,
}

impl Default for Toolbox {
    fn default() -> Self {
        Self::new()
    }
}

fn stamp(dir: &Path) -> SystemTime {
    let mut latest = SystemTime::UNIX_EPOCH;
    let mut look = |p: &Path| {
        if let Ok(m) = std::fs::metadata(p).and_then(|m| m.modified()) {
            latest = latest.max(m);
        }
    };
    look(&dir.join("project.json"));
    for sub in ["scenes", "markers"] {
        look(&dir.join(sub));
        if let Ok(rd) = std::fs::read_dir(dir.join(sub)) {
            for e in rd.flatten() {
                look(&e.path());
            }
        }
    }
    latest
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn get_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

/// `pt`: true path-traces meshes, false rasterises them, absent follows
/// the project.
fn renderer_arg(args: &Value) -> Result<Option<RendererKind>, String> {
    match args.get("pt") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(RendererKind::Path)),
        Some(Value::Bool(false)) => Ok(Some(RendererKind::Raster)),
        Some(v) => Err(format!("pt: expected true or false, got {v}")),
    }
}

/// `samples`: paths per pixel for the path tracer.
fn samples_arg(args: &Value) -> Result<Option<u32>, String> {
    match args.get("samples") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if (1..=65536).contains(&n) => Ok(Some(n as u32)),
            _ => Err(format!("samples: expected a whole number 1..65536, got {v}")),
        },
    }
}

fn round(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

impl Toolbox {
    pub fn new() -> Self {
        Self {
            current: None,
            projects: HashMap::new(),
            jobs: Vec::new(),
            backend: Backend::Auto,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    pub fn call(&mut self, name: &str, args: Value) -> ToolResult {
        let args = if args.is_null() { json!({}) } else { args };
        match name {
            "project_create" => self.project_create(&args),
            "project_open" => {
                let dir = self.path(get_str(&args, "path").ok_or("project_open needs \"path\"")?);
                self.open(&dir)?;
                self.project_info(&json!({"project": dir}))
            }
            "project_info" => self.project_info(&args),
            "recording_info" => self.recording_info(&args),
            "sfx_add" => self.sfx_add(&args),
            "recording_sound_add" => self.recording_sound_add(&args),
            "recording_place" => self.recording_place(&args),
            "timeline_get" => self.timeline_get(&args),
            "markers_query" => self.markers_query(&args),
            "markers_import" => self.markers_import(&args),
            "narration_import" => self.narration_import(&args),
            "edit_apply" => self.edit_apply(&args),
            "check" => self.check(&args),
            "sync_check" => self.sync_check(&args),
            "snapshot" => self.snapshot(&args),
            "compare" => self.compare(&args),
            "fit_camera" => self.fit_camera(&args),
            "render" => self.render(&args),
            "render_status" => self.render_status(&args),
            "history" => self.history(&args),
            "audio_analyze" => self.audio_analyze(&args),
            "media_import" => self.media_import(&args),
            "blender_run" => self.blender_run(&args),
            "track" => self.track(&args),
            "fit_curve" => self.fit_curve(&args),
            other => Err(format!("unknown tool '{other}'")),
        }
    }

    fn path(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        let p = if p.is_absolute() { p.to_path_buf() } else { self.cwd.join(p) };
        p.canonicalize().unwrap_or(p)
    }

    fn dir(&mut self, args: &Value) -> Result<PathBuf, String> {
        let dir = match get_str(args, "project") {
            Some(p) => self.path(p),
            None => self.current.clone().ok_or("no project is open: call project_open or project_create, or pass \"project\"")?,
        };
        self.open(&dir)?;
        Ok(dir)
    }

    /// Loads the project, or reloads it when files changed on disk.
    fn open(&mut self, dir: &Path) -> Result<(), String> {
        let now = stamp(dir);
        let stale = self.projects.get(dir).is_none_or(|o| now > o.stamp);
        if stale {
            let project = io::load(dir).map_err(|e| e.to_string())?;
            let (renderer, fonts) = match self.projects.remove(dir) {
                Some(o) => (o.renderer, o.fonts),
                None => (None, None),
            };
            self.projects.insert(dir.to_path_buf(), Open { project, stamp: now, compiled: None, renderer, fonts });
        }
        self.current = Some(dir.to_path_buf());
        Ok(())
    }

    fn state(&mut self, dir: &Path) -> &mut Open {
        self.projects.get_mut(dir).expect("opened")
    }

    fn compiled(&mut self, dir: &Path) -> Arc<Compiled> {
        let open = self.state(dir);
        if open.compiled.is_none() {
            open.compiled = Some(Arc::new(Compiled::new(&open.project)));
        }
        open.compiled.clone().unwrap()
    }

    fn save(&mut self, dir: &Path, mut project: Project) -> Result<(), String> {
        io::save(dir, &project).map_err(|e| e.to_string())?;
        let open = self.state(dir);
        // The narration lives in narration.json, owned by narration_import:
        // edits made from project JSON do not carry it.
        if project.narration.is_none() {
            project.narration = open.project.narration.take();
        }
        open.project = project;
        open.compiled = None;
        open.stamp = stamp(dir);
        Ok(())
    }

    /// Marker tracks and the narration, when an edit moved them
    /// (`insert_time`).
    fn save_markers(&mut self, dir: &Path, applied: &edit::Applied) -> Result<(), String> {
        if !applied.markers_changed {
            return Ok(());
        }
        for track in applied.project.markers.tracks.values() {
            io::save_track(dir, track).map_err(|e| e.to_string())?;
        }
        if let Some(n) = &applied.project.narration {
            io::save_narration(dir, n).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn renderer(&mut self, dir: &Path) -> &mut Renderer {
        let backend = self.backend;
        let open = self.projects.get_mut(dir).expect("opened");
        open.renderer.get_or_insert_with(|| Renderer::new(Some(dir), RenderOptions { backend, ..Default::default() }))
    }

    fn project_create(&mut self, args: &Value) -> ToolResult {
        let name = get_str(args, "path").ok_or("project_create needs \"path\"")?;
        let recording = match get_str(args, "recording") {
            Some(r) => {
                let r = self.path(r);
                if !recording::is_recording(&r) {
                    return Err(format!("{} is not a Lahza recording (*.lahzarec folder)", r.display()));
                }
                Some(r)
            }
            None => None,
        };
        let mut path = match &recording {
            Some(rec) => {
                let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("motion").to_string();
                if !lahza_motion_core::document::valid_id(&stem) {
                    return Err("with `recording`, path is a name: letters, digits, '-' and '_'".into());
                }
                rec.join(recording::MOTION_DIR).join(stem)
            }
            None => self.path(name),
        };
        if path.extension().and_then(|e| e.to_str()) != Some("lahza") {
            path = path.with_extension("lahza");
        }
        if io::is_project(&path) && !args.get("overwrite").and_then(Value::as_bool).unwrap_or(false) {
            return Err(format!("{} already exists (pass overwrite: true, or project_open it)", path.display()));
        }
        let fps = match args.get("fps") {
            Some(v) => serde_json::from_value::<Fps>(v.clone()).map_err(|e| e.to_string())?,
            None => Fps::whole(30),
        };
        // Inside a recording: 1920 on the long side, at the recording's shape.
        let (dw, dh) = recording.as_deref().and_then(recording_size).map(|(w, h)| fit_long_side(w, h, 1920)).unwrap_or((1920, 1080));
        let mut project = Project::new(
            args.get("width").and_then(Value::as_u64).unwrap_or(dw as u64) as u32,
            args.get("height").and_then(Value::as_u64).unwrap_or(dh as u64) as u32,
            fps,
            get_f64(args, "duration").unwrap_or(10.0),
        );
        if let Some(style) = get_str(args, "style") {
            let pack = style_pack(style).ok_or_else(|| format!("unknown style '{style}' (see lahza://styles)"))?;
            project.settings.style = Some(style.into());
            project.settings.background = Color::parse(pack.background).unwrap_or(Color::BLACK);
        }
        if let Some(bg) = get_str(args, "background") {
            project.settings.background = Color::parse(bg)?;
        }
        std::fs::create_dir_all(path.join("assets")).map_err(|e| e.to_string())?;
        if let Some(music) = get_str(args, "music") {
            let rel = import_asset(&path, &self.path(music))?;
            project.audio.music = Some(AudioClip { id: None, name: None, path: rel.clone(), at: TimeRef::Seconds(0.0), trim: 0.0, gain_db: 0.0, length: None, fade_in: 0.0, fade_out: 0.0, duck_db: None });
            project.assets.push(Asset { path: rel, source: "user".into(), licence: get_str(args, "licence").unwrap_or("user-supplied").into(), credit: None });
        }
        io::save(&path, &project).map_err(|e| e.to_string())?;
        self.open(&path)?;
        let mut info = self.project_info(&json!({"project": path}))?;
        info.json["next"] = json!(match (&recording, project.audio.music.is_some()) {
            (Some(_), _) => "add scenes and layers with edit_apply (\"../../screen.mkv\" is the recording), then recording_place it on the video",
            (None, true) => "run audio_analyze, then add scenes on bars with edit_apply",
            (None, false) => "add scenes and layers with edit_apply",
        });
        Ok(info)
    }

    fn project_info(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let compiled = self.compiled(&dir);
        let open = self.state(&dir);
        let p = &open.project;
        let markers: Map<String, Value> = p.markers.tracks.iter().map(|(k, t)| (k.clone(), json!(t.markers.len()))).collect();
        let scenes: Vec<Value> = compiled
            .scenes
            .iter()
            .zip(&p.scenes)
            .map(|(c, s)| json!({"id": s.id, "name": s.name, "start": round(c.start), "end": round(c.end), "layers": s.layers.len()}))
            .collect();
        Ok(Output::json(json!({
            "project": dir,
            "settings": p.settings,
            "frames": compiled.frame_count(),
            "scenes": scenes,
            "markers": markers,
            "audio": p.audio,
            "assets": p.assets,
            "errors": compiled.errors.iter().take(20).collect::<Vec<_>>(),
            "error_count": compiled.errors.len(),
            "renderer": open.renderer.as_ref().map(|r| r.backend_label()),
            "recording": recording::recording_of(&dir).map(|rec| json!({
                "path": rec,
                "placed": recording::read_pieces(&rec).into_iter().find(|p| rec.join(&p.folder) == dir),
            })),
        })))
    }

    fn recording_info(&mut self, args: &Value) -> ToolResult {
        let rec = self.path(get_str(args, "recording").ok_or("recording_info needs \"recording\"")?);
        if !recording::is_recording(&rec) {
            return Err(format!("{} is not a Lahza recording (*.lahzarec folder)", rec.display()));
        }
        let doc = recording::read_edit(&rec);
        let media: Vec<String> = std::fs::read_dir(rec.join("media"))
            .map(|rd| rd.flatten().map(|e| format!("../../media/{}", e.file_name().to_string_lossy())).collect())
            .unwrap_or_default();
        let pieces: Vec<Value> = recording::read_pieces(&rec)
            .into_iter()
            .map(|p| {
                let duration = io::load(&p.dir(&rec)).map(|m| m.settings.duration).ok();
                json!({"piece": p, "duration": duration})
            })
            .collect();
        let words = doc["transcript"]["words"].as_array().map(|w| w.len()).unwrap_or(0);
        // Moments worth a sound, in seconds on the edited video.
        let input: Value = std::fs::read_to_string(rec.join("input.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let on_video = |t: f64| recording::source_to_editor(&doc, t).map(round);
        let clicks: Vec<f64> = input["presses"].as_array().into_iter().flatten()
            .filter(|p| p["phase"] == "down").filter_map(|p| p["time"].as_f64()).filter_map(on_video).collect();
        let keys: Vec<f64> = input["keystrokes"].as_array().into_iter().flatten().filter_map(|k| k["time"].as_f64()).filter_map(on_video).collect();
        let zooms: Vec<Value> = doc["zoomCues"].as_array().into_iter().flatten()
            .filter(|z| z["isEnabled"].as_bool().unwrap_or(true))
            .filter_map(|z| Some(json!({"start": on_video(z["start"].as_f64()?)?, "end": z["end"].as_f64().and_then(on_video)})))
            .collect();
        let sounds: Vec<Value> = doc["audioClips"].as_array().into_iter().flatten()
            .map(|c| json!({"name": c["name"], "start": c["start"], "duration": c["duration"]})).collect();
        Ok(Output::json(json!({
            "recording": rec,
            "size": recording_size(&rec),
            "duration": edited_duration(&rec, &doc),
            "transcript_words": words,
            "clicks": clicks,
            "keystrokes": keys,
            "zooms": zooms,
            "sounds": sounds,
            "shared": {"recording": "../../screen.mkv", "media": media},
            "pieces": pieces,
            "next": "project_create {path: name, recording} → edit_apply → recording_place {start, placement}",
        })))
    }

    fn sfx_add(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let sound = get_str(args, "sound").ok_or("sfx_add needs \"sound\"")?;
        let rec = recording::recording_of(&dir);
        let path = if lahza_motion_audio::sfx::SOUNDS.iter().any(|s| s.0 == sound) {
            // Built-ins are generated once; inside a recording they are shared.
            match &rec {
                Some(rec) => {
                    let file = rec.join("media").join(format!("sfx-{sound}.wav"));
                    lahza_motion_audio::sfx::ensure(sound, &file)?;
                    recording::shared_path(&dir, rec, &file).ok_or("bad path")?
                }
                None => {
                    lahza_motion_audio::sfx::ensure(sound, &dir.join("assets/sfx").join(format!("{sound}.wav")))?;
                    format!("assets/sfx/{sound}.wav")
                }
            }
        } else {
            let abs = self.file_in(&dir, sound).map_err(|e| format!("sfx_add: '{sound}' is not a built-in sound (see lahza://sfx) and {e}"))?;
            match (abs.strip_prefix(&dir), &rec) {
                (Ok(r), _) => r.to_string_lossy().into_owned(),
                (Err(_), Some(rec)) if abs.starts_with(rec) => recording::shared_path(&dir, rec, &abs).ok_or("bad path")?,
                (Err(_), Some(rec)) => {
                    let dest = store_file(&rec.join("media"), &abs)?;
                    recording::shared_path(&dir, rec, &dest).ok_or("bad path")?
                }
                (Err(_), None) => import_asset(&dir, &abs)?,
            }
        };
        // Every sound file is an asset with its source and licence.
        let builtin = lahza_motion_audio::sfx::SOUNDS.iter().any(|s| s.0 == sound);
        let mut project = self.state(&dir).project.clone();
        if !project.assets.iter().any(|a| a.path == path) {
            project.assets.push(Asset {
                path: path.clone(),
                source: if builtin { "lahza built-in sound effect".into() } else { "user".into() },
                licence: if builtin { "generated by Lahza, no restrictions".into() } else { "user-supplied".into() },
                credit: None,
            });
            self.save(&dir, project)?;
        }
        let times: Vec<Value> = match args.get("at") {
            Some(Value::Array(a)) if !a.is_empty() => a.clone(),
            Some(v) if !v.is_array() && !v.is_null() => vec![v.clone()],
            _ => return Err("sfx_add needs \"at\": a time or a list of times".into()),
        };
        let ops: Vec<Value> = times
            .into_iter()
            .map(|at| {
                let mut op = json!({"op": "add_sound", "path": path, "at": at});
                for k in ["gain_db", "length", "name", "trim", "fade_in", "fade_out"] {
                    if let Some(v) = args.get(k) {
                        op[k] = v.clone();
                    }
                }
                op
            })
            .collect();
        let mut out = self.edit_apply(&json!({"project": dir, "ops": ops}))?;
        out.json["path"] = json!(path);
        Ok(out)
    }

    fn recording_sound_add(&mut self, args: &Value) -> ToolResult {
        let rec = self.path(get_str(args, "recording").ok_or("recording_sound_add needs \"recording\"")?);
        if !recording::is_recording(&rec) {
            return Err(format!("{} is not a Lahza recording (*.lahzarec folder)", rec.display()));
        }
        let sound = get_str(args, "sound").ok_or("recording_sound_add needs \"sound\"")?;
        let (file, name) = if lahza_motion_audio::sfx::SOUNDS.iter().any(|s| s.0 == sound) {
            let file = rec.join("media").join(format!("sfx-{sound}.wav"));
            lahza_motion_audio::sfx::ensure(sound, &file)?;
            (file, sound.to_string())
        } else {
            let abs = self.path(sound);
            let file = if abs.starts_with(&rec) { abs.clone() } else {
                let dest = store_file(&rec.join("media"), &abs)?;
                dest
            };
            (file, abs.file_stem().and_then(|s| s.to_str()).unwrap_or("sound").to_string())
        };
        let (source_duration, has_audio) = lahza_motion_render::footage::probe_sound(&file)?;
        if !has_audio || source_duration <= 0.0 {
            return Err(format!("{} has no sound", file.display()));
        }
        let times: Vec<f64> = match args.get("at") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_f64).collect(),
            Some(v) => v.as_f64().into_iter().collect(),
            None => Vec::new(),
        };
        if times.is_empty() || times.iter().any(|t| !(t.is_finite() && *t >= 0.0)) {
            return Err("at: seconds ≥ 0 on the edited video, or a list of them".into());
        }
        let duration = get_f64(args, "length").map_or(source_duration, |l| l.min(source_duration)).max(0.05);
        let volume = 10f64.powf(get_f64(args, "gain_db").unwrap_or(0.0) / 20.0).clamp(0.0, 2.0);
        let rel = file.strip_prefix(&rec).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        let mut clips = recording::read_edit(&rec)["audioClips"].as_array().cloned().unwrap_or_default();
        for t in &times {
            clips.push(json!({
                "id": uuid::Uuid::new_v4().to_string(), "file": rel, "name": name,
                "start": t, "sourceStart": 0.0, "duration": duration, "volume": volume,
                "fadeIn": 0.0, "fadeOut": 0.0, "sourceDuration": source_duration,
            }));
        }
        recording::write_key(&rec, "audioClips", Value::Array(clips))?;
        let mut info = self.recording_info(&json!({"recording": rec}))?;
        info.json["added"] = json!(times.len());
        Ok(info)
    }

    fn recording_place(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let rec = recording::recording_of(&dir).ok_or("this project is not inside a recording: create it with project_create {path, recording}")?;
        let folder = dir.strip_prefix(&rec).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        let mut pieces = recording::read_pieces(&rec);
        let existing = pieces.iter().position(|p| p.folder == folder);
        if args.get("remove").and_then(Value::as_bool).unwrap_or(false) {
            let i = existing.ok_or("this piece is not on the timeline")?;
            pieces.remove(i);
        } else {
            let placement = match get_str(args, "placement") {
                Some(p) => serde_json::from_value(json!(p)).map_err(|_| "placement: overlay or full")?,
                None => existing.map(|i| pieces[i].placement).unwrap_or_default(),
            };
            let start = get_f64(args, "start").or(existing.map(|i| pieces[i].start)).ok_or("recording_place needs \"start\"")?;
            if !(start.is_finite() && start >= 0.0) {
                return Err("start must be ≥ 0 seconds".into());
            }
            let stem = dir.file_stem().and_then(|s| s.to_str()).unwrap_or("motion").to_string();
            let piece = recording::MotionPiece {
                id: existing.map(|i| pieces[i].id.clone()).unwrap_or_else(|| format!("{stem}-{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0))),
                name: get_str(args, "name").map(str::to_string).or(existing.map(|i| pieces[i].name.clone())).unwrap_or(stem),
                folder,
                start,
                placement,
                opacity: get_f64(args, "opacity").or(existing.map(|i| pieces[i].opacity)).unwrap_or(1.0).clamp(0.0, 1.0),
                hidden: false,
            };
            match existing {
                Some(i) => pieces[i] = piece,
                None => pieces.push(piece),
            }
        }
        recording::write_pieces(&rec, &pieces)?;
        self.recording_info(&json!({"recording": rec}))
    }

    fn timeline_get(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let compiled = self.compiled(&dir);
        let p = &self.state(&dir).project;
        let from = get_f64(args, "from").unwrap_or(f64::NEG_INFINITY);
        let to = get_f64(args, "to").unwrap_or(f64::INFINITY);
        let full = args.get("full").and_then(Value::as_bool).unwrap_or(false);
        let mut scenes = Vec::new();
        for (s, c) in p.scenes.iter().zip(&compiled.scenes) {
            if get_str(args, "scene").is_some_and(|id| id != s.id) || c.end <= from || c.start >= to {
                continue;
            }
            if full {
                scenes.push(serde_json::to_value(s).unwrap_or_default());
                continue;
            }
            fn layers(list: &[lahza_motion_core::document::Layer], compiled: &Compiled, project: &Project) -> Vec<Value> {
                list.iter()
                    .map(|l| {
                        let (start, end) = compiled.element_window(&l.id).unwrap_or((0.0, 0.0));
                        let v = serde_json::to_value(l).unwrap_or_default();
                        let mut animated = Vec::new();
                        animated_paths(&v, "", &mut animated);
                        let mut out = json!({"id": l.id, "kind": l.kind.name(), "start": round(start), "end": round(end)});
                        if let Some(name) = &l.common.name {
                            out["name"] = json!(name);
                        }
                        match &l.kind {
                            lahza_motion_core::document::LayerKind::Text(t) => out["text"] = json!(t.text),
                            lahza_motion_core::document::LayerKind::Shape(s) => out["shape"] = json!(s.shape),
                            lahza_motion_core::document::LayerKind::Image(i) => out["src"] = json!(i.src),
                            lahza_motion_core::document::LayerKind::Video(v) => out["src"] = json!(v.src),
                            lahza_motion_core::document::LayerKind::Comp(_) => out["layers"] = json!(layers(l.children(), compiled, project)),
                            lahza_motion_core::document::LayerKind::Instancer(n) => {
                                out["layout"] = json!(n.layout);
                                out["layers"] = json!(layers(l.children(), compiled, project));
                            }
                            lahza_motion_core::document::LayerKind::Component(c) => {
                                // Its generated children, addressed as <id>/<child>.
                                out["component"] = json!(c.component);
                                if !c.overrides.is_empty() {
                                    out["overridden"] = json!(c.overrides.keys().collect::<Vec<_>>());
                                }
                                let ctx = lahza_motion_core::components::Ctx {
                                    width: project.settings.width as f64,
                                    height: project.settings.height as f64,
                                    fps: project.settings.fps,
                                    markers: &project.markers,
                                };
                                match lahza_motion_core::components::build_children(&l.id, c, &ctx) {
                                    Ok(children) => out["layers"] = json!(layers(&children, compiled, project)),
                                    Err(e) => out["error"] = json!(e),
                                }
                            }
                            lahza_motion_core::document::LayerKind::Mesh(m) => out["mesh"] = json!(m.mesh)["type"].clone(),
                        }
                        if !animated.is_empty() {
                            out["animated"] = json!(animated);
                        }
                        if let lahza_motion_core::document::LayerKind::Text(t) = &l.kind {
                            if !t.animators.is_empty() {
                                out["animators"] = json!(t.animators.len());
                            }
                        }
                        out
                    })
                    .collect()
            }
            scenes.push(json!({"id": s.id, "start": round(c.start), "end": round(c.end), "pre_roll": s.pre_roll, "camera_animated": !s.camera.is_default(), "layers": layers(&s.layers, &compiled, p)}));
        }
        Ok(Output::json(json!({"duration": p.settings.duration, "fps": p.settings.fps, "scenes": scenes})))
    }

    fn markers_query(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let markers = &self.state(&dir).project.markers;
        let tracks: Vec<String> = match args.get("track") {
            None => {
                let list: Vec<Value> = markers
                    .tracks
                    .values()
                    .map(|t| json!({"track": t.name, "count": t.markers.len(), "source": t.source,
                        "first": t.markers.first().map(|m| round(m.t)), "last": t.markers.last().map(|m| round(m.t)),
                        "median_interval": t.median_interval().map(round)}))
                    .collect();
                return Ok(Output::json(json!({"tracks": list})));
            }
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
            Some(_) => return Err("track must be a name or a list of names".into()),
        };
        let from = get_f64(args, "from").unwrap_or(f64::NEG_INFINITY);
        let to = get_f64(args, "to").unwrap_or(f64::INFINITY);
        let text = get_str(args, "text").map(lahza_motion_core::markers::normalize);
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
        let mut out = Map::new();
        for name in tracks {
            let Some(track) = markers.get(&name) else {
                return Err(format!("no track '{name}' (have: {})", markers.tracks.keys().cloned().collect::<Vec<_>>().join(", ")));
            };
            let list: Vec<Value> = track
                .markers
                .iter()
                .enumerate()
                .filter(|(_, m)| m.t >= from && m.t < to)
                .filter(|(_, m)| text.as_ref().is_none_or(|q| lahza_motion_core::markers::normalize(m.label.as_deref().unwrap_or("")).contains(q.as_str())))
                .take(limit)
                .map(|(i, m)| {
                    let mut v = json!({"ref": marker_ref(track, i), "t": round(m.t)});
                    if let Some(id) = &m.id {
                        v["id"] = json!(id);
                    }
                    if let Some(a) = &m.alt {
                        v["alt"] = json!(a);
                    }
                    if let Some(e) = m.end {
                        v["end"] = json!(round(e));
                    }
                    if let Some(l) = &m.label {
                        v["label"] = json!(l);
                    }
                    if let Some(x) = m.value {
                        v["value"] = json!(x);
                    }
                    if let Some(b) = m.bbox {
                        v["bbox"] = json!(b);
                    }
                    v
                })
                .collect();
            out.insert(name, Value::Array(list));
        }
        Ok(Output::json(Value::Object(out)))
    }

    fn narration_import(&mut self, args: &Value) -> ToolResult {
        use lahza_motion_core::narration::{self as nar, Manifest, Narration, NarrationClip};
        let dir = self.dir(args)?;
        let file = self.file_in(&dir, get_str(args, "manifest").ok_or("narration_import needs \"manifest\"")?)?;
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        let base = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let old = self.state(&dir).project.narration.clone().unwrap_or_default();
        let project = self.state(&dir).project.clone();
        let mut clips: Vec<NarrationClip> = Vec::new();
        let (mut imported, mut reused) = (Vec::new(), Vec::new());
        let mut cursor: Option<f64> = None;
        for (n, mc) in manifest.clips.iter().enumerate() {
            let id = mc.id.clone().unwrap_or_else(|| format!("line{}", n + 1));
            if !lahza_motion_core::document::valid_id(&id) {
                return Err(format!("clip id '{id}': letters, digits, '-' and '_' only"));
            }
            if clips.iter().any(|c| c.id == id) {
                return Err(format!("clip id '{id}' is used twice"));
            }
            let audio = if Path::new(&mc.audio).is_absolute() { PathBuf::from(&mc.audio) } else { base.join(&mc.audio) };
            let bytes = std::fs::read(&audio).map_err(|e| format!("line {id}: {}: {e}", audio.display()))?;
            let audio_hash = sha_hex(&bytes);
            let timed = match (&mc.words, &mc.chars) {
                (Some(w), _) => w.clone(),
                (None, Some(c)) => nar::chars_to_words(c),
                (None, None) => {
                    return Err(format!(
                        "line {id} has no timings: give \"words\" or \"chars\" from the voice provider (tools/narration has adapters; a voice without timings needs an external aligner)"
                    ))
                }
            };
            let hash = sha_hex(format!("{audio_hash}\n{}\n{}", mc.text, serde_json::to_string(&timed).unwrap_or_default()).as_bytes());
            let at = mc.at.unwrap_or_else(|| match cursor {
                None => manifest.lead.unwrap_or(0.5),
                Some(end) => end + mc.pause.or(manifest.pause).unwrap_or(0.35),
            });
            let gain_db = mc.gain_db.unwrap_or(0.0);
            let clip = match old.clips.iter().find(|c| c.id == id && c.hash == hash && dir.join(&c.path).is_file()) {
                Some(prev) => {
                    reused.push(id.clone());
                    NarrationClip { at, gain_db, ..prev.clone() }
                }
                None => {
                    let ext = audio.extension().and_then(|e| e.to_str()).unwrap_or("wav");
                    let rel = format!("assets/narration/{id}-{}.{ext}", &audio_hash[..12]);
                    let dest = dir.join(&rel);
                    if !dest.is_file() {
                        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
                        std::fs::write(&dest, &bytes).map_err(|e| format!("{}: {e}", dest.display()))?;
                    }
                    let (mut words, cues) = nar::build_clip(&mc.text, &timed).map_err(|e| format!("line {id}: {e}"))?;
                    let samples = lahza_motion_audio::decode(&dest).map_err(|e| format!("line {id}: {e}"))?;
                    let rate = lahza_motion_audio::SAMPLE_RATE as f64;
                    word_loudness(&samples, rate, &mut words);
                    imported.push(id.clone());
                    NarrationClip { id: id.clone(), path: rel, text: mc.text.clone(), at, duration: samples.len() as f64 / rate, gain_db, words, cues, audio_hash, hash }
                }
            };
            cursor = Some(clip.at + clip.duration);
            clips.push(clip);
        }
        let narration = Narration { clips };
        // Earlier takes of changed lines are no longer used.
        let keep: Vec<&str> = narration.clips.iter().map(|c| c.path.as_str()).collect();
        if let Ok(entries) = std::fs::read_dir(dir.join("assets/narration")) {
            for e in entries.flatten() {
                let rel = format!("assets/narration/{}", e.file_name().to_string_lossy());
                if !keep.contains(&rel.as_str()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        io::save_narration(&dir, &narration).map_err(|e| e.to_string())?;
        let tracks = narration.tracks();
        for t in &tracks {
            io::save_track(&dir, t).map_err(|e| e.to_string())?;
        }
        let mut project = project;
        project.audio.narration = narration
            .clips
            .iter()
            .map(|c| AudioClip { id: Some(c.id.clone()), name: None, path: c.path.clone(), at: TimeRef::Seconds(c.at), trim: 0.0, gain_db: c.gain_db, length: None, fade_in: 0.0, fade_out: 0.0, duck_db: None })
            .collect();
        project.assets.retain(|a| !a.path.starts_with("assets/narration/") || keep.contains(&a.path.as_str()));
        for c in &narration.clips {
            if !project.assets.iter().any(|a| a.path == c.path) {
                project.assets.push(Asset { path: c.path.clone(), source: "narration".into(), licence: "user-supplied".into(), credit: None });
            }
        }
        for t in tracks.clone() {
            project.markers.insert(t);
        }
        let issues: Vec<Value> = narration.validate().into_iter().map(|(clip, word, message)| json!({"line": clip, "word": word, "message": message})).collect();
        let end = narration.clips.last().map(|c| c.at + c.duration).unwrap_or(0.0);
        let summary = json!({
            "lines": narration.clips.len(),
            "imported": imported, "reused": reused,
            "words": narration.clips.iter().map(|c| c.words.len()).sum::<usize>(),
            "cues": tracks.iter().find(|t| t.name == "cue").map(|t| t.markers.iter().filter_map(|m| m.label.clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "narration_ends": (end * 1000.0).round() / 1000.0,
            "project_duration": project.settings.duration,
            "issues": issues,
        });
        project.narration = Some(narration);
        self.save(&dir, project)?;
        Ok(Output::json(summary))
    }

    fn markers_import(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let offset = get_f64(args, "offset").unwrap_or(0.0);
        let tracks = match get_str(args, "kind").unwrap_or("") {
            "transcript" => {
                let mut path = self.path(get_str(args, "path").ok_or("transcript import needs \"path\"")?);
                if path.is_dir() {
                    // A Lahza recording: the transcript lives in its edit
                    // document (the autosaved draft wins), on source time.
                    let draft = path.join("edit.draft.json");
                    path = if draft.is_file() { draft } else { path.join("edit.json") };
                }
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let value: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                let words = transcript_words(&value).ok_or("no words found: expected a Lahza transcript {\"words\": [{text, start, end}]} or whisper.cpp JSON")?;
                let (w, s) = Markers::from_words(&words, offset);
                vec![w, s]
            }
            "user" => {
                let name = get_str(args, "name").unwrap_or("user").to_string();
                let list = args.get("markers").and_then(Value::as_array).ok_or("user import needs \"markers\"")?;
                let mut track = MarkerTrack { name, source: Some("user".into()), markers: Vec::new() };
                for m in list {
                    track.markers.push(match m {
                        Value::Number(n) => Marker { t: n.as_f64().unwrap_or(0.0) + offset, ..Default::default() },
                        Value::Object(_) => {
                            let mut m: Marker = serde_json::from_value(m.clone()).map_err(|e| format!("marker: {e}"))?;
                            m.t += offset;
                            m
                        }
                        _ => return Err("markers are numbers or {t, label, end, value}".into()),
                    });
                }
                vec![track]
            }
            other => return Err(format!("unknown kind '{other}': use transcript or user")),
        };
        let mut summary = Vec::new();
        for track in &tracks {
            io::save_track(&dir, track).map_err(|e| e.to_string())?;
            summary.push(json!({"track": track.name, "count": track.markers.len()}));
        }
        let open = self.state(&dir);
        for t in tracks {
            open.project.markers.insert(t);
        }
        open.compiled = None;
        open.stamp = stamp(&dir);
        Ok(Output::json(json!({"imported": summary})))
    }

    fn edit_apply(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let ops = args.get("ops").and_then(Value::as_array).ok_or("edit_apply needs \"ops\": [...]")?.clone();
        let author = get_str(args, "author").unwrap_or("agent").to_string();
        let applied = edit::apply(&self.state(&dir).project, &ops).map_err(|e| e.to_string())?;
        let n = io::next_history_number(&dir).map_err(|e| e.to_string())?;
        self.save_markers(&dir, &applied)?;
        self.save(&dir, applied.project)?;
        io::write_history(&dir, &HistoryEntry {
            n,
            time: io::now(),
            author,
            summary: applied.summary.clone(),
            ops,
            inverse: applied.inverse,
            undone: false,
            undoes: Vec::new(),
        })
        .map_err(|e| e.to_string())?;
        let compiled = self.compiled(&dir);
        Ok(Output::json(json!({
            "history": n,
            "created": applied.created,
            "changed": applied.changed,
            "summary": applied.summary,
            "errors": compiled.errors,
        })))
    }

    fn check(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let compiled = self.compiled(&dir);
        let open = self.state(&dir);
        let fonts = std::cell::RefCell::new(open.fonts.take().unwrap_or_else(|| Fonts::new(Some(&dir))));
        let measure = |layer: &lahza_motion_core::compile::LayerState| -> Option<[f64; 4]> {
            let lahza_motion_core::compile::Content::Text(text) = &layer.content else { return None };
            fonts.borrow_mut().ink_bounds(text, layer.matrix)
        };
        let options = CheckOptions { dir: Some(&dir), measure: Some(&measure), tolerance_frames: get_f64(args, "tolerance_frames").unwrap_or(1.0) };
        let mut report = check::check(&open.project, &compiled, &options);
        let warnings = fonts.borrow().warnings.clone();
        open.fonts = Some(fonts.into_inner());
        let sync_count = report.syncs.len();
        if !args.get("verbose").and_then(Value::as_bool).unwrap_or(false) && report.syncs.len() > 40 {
            report.syncs.sort_by(|a, b| b.drift_frames.abs().total_cmp(&a.drift_frames.abs()));
            report.syncs.truncate(40);
        }
        // What the renderer would drop is an error, not a footnote: a font
        // that is not found, a frame whose images cannot all be held.
        for w in &warnings {
            report.add(check::Severity::Error, "structure", None, None, w.clone());
        }
        if let Some((t, need)) = image_overflow(&compiled, &dir) {
            report.add(
                check::Severity::Error,
                "structure",
                None,
                Some(t),
                format!(
                    "the images on screen at {t:.2} s need {:.0} M texels; the image pool holds {:.0} M, so some would not be drawn (use smaller images)",
                    need as f64 / 1e6,
                    lahza_motion_render::images::MAX_TEXELS as f64 / 1e6
                ),
            );
        }
        let mut v = serde_json::to_value(&report).unwrap_or_default();
        v["sync_count"] = json!(sync_count);
        Ok(Output::json(v))
    }

    fn sync_check(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let compiled = self.compiled(&dir);
        let project = &self.state(&dir).project;
        let tracks: Option<Vec<String>> = args.get("tracks").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect());
        let options = check::SyncOptions { frames: get_f64(args, "frames").unwrap_or(2.0), tracks: tracks.as_deref() };
        let mut r = check::sync_check(project, &compiled, &options);
        let from = get_f64(args, "from").unwrap_or(f64::NEG_INFINITY);
        let to = get_f64(args, "to").unwrap_or(f64::INFINITY);
        let element = get_str(args, "element");
        let flagged = args.get("flagged").and_then(Value::as_bool).unwrap_or(false);
        r.events.retain(|e| {
            e.t >= from && e.t <= to && (!flagged || e.flag.is_some()) && element.is_none_or(|id| e.element == id || e.element.starts_with(&format!("{id}/")))
        });
        let shown = r.events.len();
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
        if shown > limit {
            // Flagged first, then in time order.
            r.events.sort_by(|a, b| a.flag.is_none().cmp(&b.flag.is_none()).then(a.t.total_cmp(&b.t)));
            r.events.truncate(limit);
            r.events.sort_by(|a, b| a.t.total_cmp(&b.t));
        }
        let mut v = serde_json::to_value(&r).unwrap_or_default();
        v["matching"] = json!(shown);
        if r.markers == 0 {
            v["note"] = json!("no markers to measure against: import narration, analyse the music or add markers first");
        }
        Ok(Output::json(v))
    }

    fn resolve_time(&mut self, dir: &Path, v: &Value) -> Result<(String, f64), String> {
        let compiled = self.compiled(dir);
        let markers = &self.state(dir).project.markers;
        let r: TimeRef = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        let base: TimeBase = compiled.base;
        let mut elements: &Compiled = &compiled;
        let t = r.resolve(markers, &base, &mut elements).map_err(|e| e.to_string())?;
        Ok((r.text(), t))
    }

    fn fit_camera(&mut self, args: &Value) -> ToolResult {
        use lahza_motion_core::camfit::{fit, CameraParams, Free, Mark};
        let dir = self.dir(args)?;
        let project = self.state(&dir).project.clone();
        let (w, h) = (project.settings.width as f64, project.settings.height as f64);
        let frames = args.get("frames").and_then(Value::as_array).ok_or("fit_camera needs \"frames\"")?.clone();
        let mut start: CameraParams = match args.get("start") {
            Some(v) if !v.is_null() => serde_json::from_value(v.clone()).map_err(|e| format!("start: {e}"))?,
            _ => CameraParams::default(),
        };
        let fixed: Vec<String> = args.get("fix").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_else(|| vec!["shift".into()]);
        let free = Free {
            fov: !fixed.iter().any(|f| f == "fov"),
            orbit: !fixed.iter().any(|f| f == "orbit"),
            dolly: !fixed.iter().any(|f| f == "dolly"),
            position: !fixed.iter().any(|f| f == "position"),
            shift: !fixed.iter().any(|f| f == "shift"),
        };
        let mut rows = Vec::new();
        let mut solved = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            let (label, t) = self.resolve_time(&dir, f.get("at").ok_or(format!("frames.{i}: needs \"at\""))?)?;
            let marks: Vec<Mark> = serde_json::from_value(f.get("marks").cloned().unwrap_or(json!([]))).map_err(|e| format!("frames.{i}.marks: {e}"))?;
            if marks.len() < 3 {
                return Err(format!("frames.{i}: at least 3 marks (more pin the camera down better)"));
            }
            let (c, e) = fit(w, h, &marks, &start, free);
            start = c;
            rows.push(json!({"at": label, "t": round(t), "rms_px": round(e), "camera": c}));
            solved.push((t, c));
        }
        let mut out = json!({"frames": rows});
        if let Some(scene) = get_str(args, "scene") {
            let compiled = self.compiled(&dir);
            let sc = compiled.scenes.iter().find(|s| &*s.id == scene).ok_or(format!("no scene '{scene}'"))?;
            // Keys are in the scene's own time.
            let at = |t: f64| round(t - sc.start + sc.pre_roll);
            let keys = |v: &dyn Fn(&CameraParams) -> Value| -> Value {
                if solved.len() == 1 {
                    v(&solved[0].1)
                } else {
                    json!({"keys": solved.iter().map(|(t, c)| json!({"at": at(*t), "v": v(c)})).collect::<Vec<_>>()})
                }
            };
            let mut ops = Vec::new();
            let set = |name: &str, value: Value| json!({"op": "set", "target": format!("{scene}.camera.{name}"), "value": value});
            if free.fov {
                ops.push(set("fov", keys(&|c| json!(round(c.fov)))));
            }
            if free.orbit {
                ops.push(set("orbit", keys(&|c| json!([round(c.orbit[0]), round(c.orbit[1])]))));
            }
            if free.dolly {
                ops.push(set("dolly", keys(&|c| json!(round(c.dolly)))));
            }
            if free.position {
                ops.push(set("position", keys(&|c| json!(c.position.map(|p| [round(p[0]), round(p[1])])))));
            }
            if free.shift {
                ops.push(set("shift", keys(&|c| json!([round(c.shift[0]), round(c.shift[1])]))));
            }
            if args.get("apply").and_then(Value::as_bool).unwrap_or(false) {
                self.call("edit_apply", json!({"project": dir.display().to_string(), "ops": ops.clone()}))?;
                out["applied"] = json!(true);
            }
            out["ops"] = json!(ops);
        }
        Ok(Output { json: out, images: Vec::new() })
    }

    fn snapshot(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let compiled = self.compiled(&dir);
        let mut times: Vec<(String, f64)> = Vec::new();
        if let Some(list) = args.get("at").and_then(Value::as_array) {
            for v in list {
                times.push(self.resolve_time(&dir, v)?);
            }
        } else if let Some(track) = get_str(args, "track") {
            let from = get_f64(args, "from").unwrap_or(f64::NEG_INFINITY);
            let to = get_f64(args, "to").unwrap_or(f64::INFINITY);
            let markers = &self.state(&dir).project.markers;
            let t = markers.get(track).ok_or_else(|| format!("no track '{track}'"))?;
            times.extend(t.markers.iter().enumerate().filter(|(_, m)| m.t >= from && m.t < to).map(|(i, m)| (format!("{track}:{}", i + 1), m.t)));
        } else {
            let mut points: Vec<(String, f64)> = compiled.syncs.iter().map(|s| (s.reference.clone(), s.requested)).collect();
            points.sort_by(|a, b| a.1.total_cmp(&b.1));
            points.dedup_by(|a, b| (a.1 - b.1).abs() < compiled.fps().frame());
            if points.is_empty() {
                let d = compiled.settings.duration;
                points = (0..8).map(|i| (format!("{:.2}s", d * (i as f64 + 0.5) / 8.0), d * (i as f64 + 0.5) / 8.0)).collect();
            }
            times = points;
        }
        if times.is_empty() {
            return Err("no times to snapshot".into());
        }
        if times.len() > 48 {
            times.truncate(48);
        }
        let sheet = args.get("sheet").and_then(Value::as_bool).unwrap_or(times.len() > 1);
        let width = args.get("width").and_then(Value::as_u64).unwrap_or(if sheet { 360 } else { 960 }) as f64;
        let fps = compiled.fps();
        let pw = compiled.settings.width as f64;
        // Render at the size we return (sheets render at 2× thumbnail size).
        let scale = ((if sheet { width * 2.0 } else { width }) / pw).clamp(0.05, 1.0);
        let path = match renderer_arg(args)? {
            Some(RendererKind::Path) => export::trace_options(&compiled.settings, Quality::Final, Some(RendererKind::Path), samples_arg(args)?)
                .map(|o| lahza_motion_render::pt::TraceOptions { denoise: args.get("denoise").and_then(Value::as_bool).unwrap_or(true), ..o }),
            _ => None,
        };
        let renderer = self.renderer(&dir);
        renderer.set_scale(scale);
        renderer.set_path(path);
        renderer.clear_fallbacks();
        let mut frames = Vec::new();
        let mut listed = Vec::new();
        for (label, t) in &times {
            // The frame nearest the reference: where a cut or anchor on it lands.
            let n = fps.nearest_frame(*t).clamp(0, compiled.frame_count().saturating_sub(1) as i64);
            let ft = fps.time_of(n);
            let image = renderer.render(&compiled, ft);
            listed.push(json!({"at": label, "t": round(ft), "frame": n}));
            frames.push((format!("{label} · {ft:.2}s · f{n}"), image));
        }
        let backend = renderer.backend_label();
        let dropped = renderer.dropped();
        let fell_back: Vec<String> = renderer.warnings().into_iter().filter(|w| w.starts_with("GPU failed") || w.starts_with("path tracing needs")).collect();
        renderer.set_scale(1.0);
        renderer.set_path(None);
        let images: Vec<Vec<u8>> = if sheet {
            let columns = (frames.len() as f64).sqrt().ceil().max(1.0) as u32;
            vec![lahza_motion_render::encode_png(&sheet::contact_sheet(&frames, columns.min(4).max(1), width as u32))]
        } else {
            frames.iter().map(|(_, img)| lahza_motion_render::encode_png(img)).collect()
        };
        if let Some(save) = get_str(args, "save") {
            let path = self.path(save);
            std::fs::write(&path, &images[0]).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let mut out = json!({"frames": listed, "renderer": backend});
        // A GPU failure mid-snapshot is reported, not silent.
        if !fell_back.is_empty() {
            out["warnings"] = json!(fell_back);
        }
        if !dropped.is_empty() {
            // Not drawn as asked: the frames are wrong.
            out["ok"] = json!(false);
            out["errors"] = json!(dropped);
        }
        Ok(Output { json: out, images })
    }

    fn compare(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let against = self.path(get_str(args, "against").ok_or("compare needs \"against\" (a reference video)")?);
        if !against.exists() {
            return Err(format!("{} does not exist", against.display()));
        }
        let offset = get_f64(args, "offset").unwrap_or(0.0);
        let Some(at) = args.get("at").and_then(Value::as_array) else {
            return self.compare_run(&dir, &against, offset, args);
        };
        let mut times = Vec::new();
        for v in at {
            times.push(self.resolve_time(&dir, v)?);
        }
        if times.is_empty() || times.len() > 16 {
            return Err("compare takes 1 to 16 times".into());
        }
        let width = args.get("width").and_then(Value::as_u64).unwrap_or(360).clamp(64, 1280) as f64;
        let compiled = self.compiled(&dir);
        let fps = compiled.fps();
        let scale = (width * 2.0 / compiled.settings.width as f64).clamp(0.05, 1.0);
        let renderer = self.renderer(&dir);
        renderer.set_scale(scale);
        let mut cells = Vec::new();
        let mut rows = Vec::new();
        for (label, t) in &times {
            let n = fps.nearest_frame(*t).clamp(0, compiled.frame_count().saturating_sub(1) as i64);
            let ft = fps.time_of(n);
            let ours = renderer.render(&compiled, ft);
            let (w, h) = ours.dimensions();
            let rt = (ft + offset).max(0.0);
            let reference = reference_frame(&against, rt, w, h)?;
            let (diff, mean) = difference(&ours, &reference);
            let luma = |img: &image::RgbaImage| img.pixels().map(|p| 0.2126 * p.0[0] as f64 + 0.7152 * p.0[1] as f64 + 0.0722 * p.0[2] as f64).sum::<f64>() / (w as f64 * h as f64);
            let (missing, extra) = detail(&ours, &reference);
            let (so, sr) = (crate::measure::shading(&ours), crate::measure::shading(&reference));
            let ratio = crate::measure::shading_ratio(so, sr);
            let mut row = json!({"at": label, "t": round(ft), "reference_t": round(rt), "mean_difference": round(mean),
                             "missing_detail": round(missing), "extra_detail": round(extra),
                             "shading_ours": so.map(round), "shading_reference": sr.map(round), "shading_ratio": ratio.map(round),
                             "brightness_ours": round(luma(&ours)), "brightness_reference": round(luma(&reference))});
            if let Some(v) = crate::measure::flatness_verdict(ratio) {
                row["shading_verdict"] = json!(v);
            }
            rows.push(row);
            cells.push((format!("ours {label} · {ft:.2}s"), ours));
            cells.push((format!("reference {rt:.2}s"), reference));
            cells.push((format!("difference {mean:.1}"), diff));
        }
        renderer.set_scale(1.0);
        let png = lahza_motion_render::encode_png(&sheet::contact_sheet(&cells, 3, width as u32));
        if let Some(save) = get_str(args, "save") {
            let path = self.path(save);
            std::fs::write(&path, &png).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(Output { json: json!({"rows": rows, "against": against.display().to_string(), "offset": offset}), images: vec![png] })
    }

    /// Every frame from first to last against the reference.
    fn compare_run(&mut self, dir: &Path, against: &Path, offset: f64, args: &Value) -> ToolResult {
        let ffmpeg = which("ffmpeg").ok_or("compare needs ffmpeg")?;
        let compiled = self.compiled(dir);
        let fps = compiled.fps();
        let last = compiled.frame_count().saturating_sub(1) as i64;
        let first_n = get_f64(args, "from").map(|t| fps.nearest_frame(t)).unwrap_or(0).clamp(0, last);
        let last_n = get_f64(args, "to").map(|t| fps.nearest_frame(t)).unwrap_or(last).clamp(first_n, last);
        let width = args.get("width").and_then(Value::as_u64).unwrap_or(270).clamp(64, 960) as f64;
        let scale = (width / compiled.settings.width as f64).clamp(0.02, 1.0);
        let video = match get_str(args, "video") {
            Some(v) => self.path(v),
            None => dir.join("renders").join("compare.mp4"),
        };
        let save = get_str(args, "save").map(|p| self.path(p));
        let renderer = self.renderer(dir);
        renderer.set_scale(scale);
        let probe = renderer.render(&compiled, fps.time_of(first_n));
        let (w, h) = probe.dimensions();
        // Pair frames by index, not by resampled timestamps: our frame n
        // shows the reference frame on screen at n/fps + offset.
        let timing = ref_timing(against);
        let ref_index = |t: f64| timing.map(|(s, r)| ((t - s) * r + 1e-6).floor().max(0.0) as i64).unwrap_or(0);
        let first_ref = ref_index(fps.time_of(first_n) + offset);
        let filter = format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black");
        let mut reader = std::process::Command::new(&ffmpeg)
            .args(["-v", "error", "-ss", &format!("{:.6}", seek_before(timing, first_ref)), "-i"])
            .arg(against)
            .args(["-vf", &filter, "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("ffmpeg: {e}"))?;
        let mut ref_out = reader.stdout.take().ok_or("ffmpeg gave no output")?;
        // Side-by-side video: ours | reference | difference, even height.
        if let Some(parent) = video.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let (vw, vh) = (w * 3 + 8, h + (h % 2));
        let mut writer = std::process::Command::new(&ffmpeg)
            .args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{vw}x{vh}"), "-r", &format!("{}", fps.as_f64()), "-i", "-",
                   "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"])
            .arg(&video)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("ffmpeg: {e}"))?;
        let mut sink = writer.stdin.take().ok_or("ffmpeg gave no input")?;
        let frame_bytes = (w * h * 4) as usize;
        let mut ended_at = None;
        // [difference, motion ours, motion reference, missing detail, extra detail]
        let mut series: Vec<[f64; 5]> = Vec::new();
        // Interior shading (ours, reference); `None` where a frame has no interior.
        let mut shades: Vec<(Option<f64>, Option<f64>)> = Vec::new();
        let (mut prev_ours, mut prev_ref): (Option<image::RgbaImage>, Option<image::RgbaImage>) = (None, None);
        let black = image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
        // The last decoded reference frame and its index.
        let (mut held, mut held_index): (Option<image::RgbaImage>, i64) = (None, first_ref - 1);
        for n in first_n..=last_n {
            let ours = renderer.render(&compiled, fps.time_of(n));
            let want = ref_index(fps.time_of(n) + offset);
            while held_index < want && ended_at.is_none() {
                let mut buf = vec![0u8; frame_bytes];
                match std::io::Read::read_exact(&mut ref_out, &mut buf) {
                    Ok(()) => {
                        held = image::RgbaImage::from_raw(w, h, buf);
                        held_index += 1;
                    }
                    Err(_) => {
                        ended_at = Some(fps.time_of(n) + offset);
                        held = None;
                    }
                }
            }
            let reference = held.clone().unwrap_or_else(|| black.clone());
            let (diff, mean) = difference(&ours, &reference);
            let motion = |prev: &Option<image::RgbaImage>, cur: &image::RgbaImage| prev.as_ref().map(|p| difference(p, cur).1).unwrap_or(0.0);
            let (missing, extra) = detail(&ours, &reference);
            series.push([mean, motion(&prev_ours, &ours), motion(&prev_ref, &reference), missing, extra]);
            shades.push((crate::measure::shading(&ours), crate::measure::shading(&reference)));
            let mut frame = image::RgbaImage::from_pixel(vw, vh, image::Rgba([20, 20, 26, 255]));
            for (i, img) in [&ours, &reference, &diff].into_iter().enumerate() {
                image::imageops::replace(&mut frame, img, (i as u32 * (w + 4)) as i64, 0);
            }
            if std::io::Write::write_all(&mut sink, frame.as_raw()).is_err() {
                break;
            }
            prev_ours = Some(ours);
            prev_ref = Some(reference);
        }
        drop(sink);
        let _ = reader.kill();
        let _ = reader.wait();
        let written = writer.wait().map(|s| s.success()).unwrap_or(false);

        // Motion is smoothed over 5 frames so single-frame cuts don't dominate.
        let smooth = |k: usize| -> Vec<f64> {
            (0..series.len()).map(|i| {
                let (a, b) = (i.saturating_sub(2), (i + 3).min(series.len()));
                series[a..b].iter().map(|v| v[k]).sum::<f64>() / (b - a) as f64
            }).collect()
        };
        let (mo, mr, miss, extra) = (smooth(1), smooth(2), smooth(3), smooth(4));
        // Shading over the same 5 frames; a frame without interior counts as flat.
        let smooth_shade = |pick: fn(&(Option<f64>, Option<f64>)) -> Option<f64>| -> Vec<f64> {
            (0..shades.len()).map(|i| {
                let (a, b) = (i.saturating_sub(2), (i + 3).min(shades.len()));
                shades[a..b].iter().map(|s| pick(s).unwrap_or(0.0)).sum::<f64>() / (b - a) as f64
            }).collect()
        };
        let (so, sr) = (smooth_shade(|s| s.0), smooth_shade(|s| s.1));
        let flat = |i: usize| crate::measure::flatness_verdict(crate::measure::shading_ratio(Some(so[i]), Some(sr[i]))).is_some();
        // Shading over a set of frames: ours ÷ reference over the frames
        // where the reference has an interior to measure.
        let shading_of = |idx: &mut dyn Iterator<Item = usize>| -> (Option<f64>, Option<f64>, Option<f64>) {
            let (mut o, mut r, mut n) = (0.0, 0.0, 0usize);
            for i in idx {
                if let Some(rs) = shades[i].1 {
                    o += shades[i].0.unwrap_or(0.0);
                    r += rs;
                    n += 1;
                }
            }
            if n == 0 {
                return (None, None, None);
            }
            let (o, r) = (o / n as f64, r / n as f64);
            (Some(o), Some(r), crate::measure::shading_ratio(Some(o), Some(r)))
        };
        // Fixed scales, not relative to this video: motion is the mean
        // per-pixel change between frames, difference the mean per-pixel gap.
        let moves = |o: f64, r: f64| -> &'static str {
            if r > 0.8 && o < 0.5 * r {
                "the reference moves more"
            } else if o > 0.8 && r < 0.5 * o {
                "ours moves more"
            } else {
                "motion similar"
            }
        };
        let looks = |d: f64| -> &'static str {
            if d < 12.0 {
                "looks close"
            } else if d < 25.0 {
                "looks somewhat different"
            } else {
                "looks different"
            }
        };
        // Detail: edges in one picture with no edge near them in the other.
        // The mean difference barely notices thin lines; this does.
        const MISSING: f64 = 0.4;
        let details = |m: f64, x: f64| -> String {
            match (m > MISSING, x > MISSING) {
                (true, true) => format!("; different detail (missing {:.0}%, extra {:.0}%)", m * 100.0, x * 100.0),
                (true, false) => format!("; ours lacks detail the reference has ({:.0}% of its edges)", m * 100.0),
                (false, true) => format!("; ours has detail the reference lacks ({:.0}% of our edges)", x * 100.0),
                _ => String::new(),
            }
        };
        let verdict = |o: f64, r: f64, d: f64, m: f64, flat: bool| -> &'static str {
            match moves(o, r) {
                "motion similar" if m > MISSING => "ours lacks detail",
                "motion similar" if flat => "ours is flatter",
                "motion similar" if d >= 25.0 => "looks different",
                "motion similar" => "close",
                m => m,
            }
        };
        let time = |i: usize| fps.time_of(first_n + i as i64);
        // Stretches where a verdict other than "close" holds (gaps ≤ 3 f bridged).
        let mut segments: Vec<(usize, usize, &'static str)> = Vec::new();
        for i in 0..series.len() {
            let v = verdict(mo[i], mr[i], series[i][0], miss[i], flat(i));
            if v == "close" {
                continue;
            }
            match segments.last_mut() {
                Some(last) if last.2 == v && i - last.1 <= 4 => last.1 = i,
                _ => segments.push((i, i, v)),
            }
        }
        segments.retain(|s| s.1 - s.0 + 1 >= 4);
        let severity = |s: &(usize, usize, &str)| (s.0..=s.1).map(|i| series[i][0] + (mr[i] - mo[i]).abs() * 4.0).sum::<f64>();
        segments.sort_by(|a, b| severity(b).total_cmp(&severity(a)));
        segments.truncate(10);
        let seg_json: Vec<Value> = segments.iter().map(|s| {
            let n = (s.1 - s.0 + 1) as f64;
            json!({"from": round(time(s.0)), "to": round(time(s.1)), "frames": s.1 - s.0 + 1, "verdict": s.2,
                   "mean_difference": round((s.0..=s.1).map(|i| series[i][0]).sum::<f64>() / n),
                   "motion_ours": round((s.0..=s.1).map(|i| mo[i]).sum::<f64>() / n),
                   "motion_reference": round((s.0..=s.1).map(|i| mr[i]).sum::<f64>() / n),
                   "missing_detail": round((s.0..=s.1).map(|i| miss[i]).sum::<f64>() / n),
                   "shading_ratio": shading_of(&mut (s.0..=s.1)).2.map(round)})
        }).collect();
        let scenes: Vec<Value> = compiled.scenes.iter().filter_map(|sc| {
            let idx: Vec<usize> = (0..series.len()).filter(|&i| time(i) >= sc.start && time(i) < sc.end).collect();
            if idx.is_empty() {
                return None;
            }
            let n = idx.len() as f64;
            let avg = |v: &dyn Fn(usize) -> f64| idx.iter().map(|&i| v(i)).sum::<f64>() / n;
            let (d, o, r) = (avg(&|i| series[i][0]), avg(&|i| mo[i]), avg(&|i| mr[i]));
            let (m, x) = (avg(&|i| miss[i]), avg(&|i| extra[i]));
            let (shade_o, shade_r, ratio) = shading_of(&mut idx.iter().copied());
            let flatter = crate::measure::flatness_verdict(ratio).map(|v| format!("; {v}")).unwrap_or_default();
            Some(json!({"scene": sc.id.as_ref(), "from": round(sc.start), "to": round(sc.end), "mean_difference": round(d),
                        "motion_ours": round(o), "motion_reference": round(r), "missing_detail": round(m), "extra_detail": round(x),
                        "shading_ours": shade_o.map(round), "shading_reference": shade_r.map(round), "shading_ratio": ratio.map(round),
                        "verdict": format!("{}; {}{}{}", moves(o, r), looks(d), details(m, x), flatter)}))
        }).collect();

        // Timeline graph: difference (white), motion ours (orange), reference (cyan).
        let gw = (series.len() as u32 * 2).clamp(600, 1800);
        let gh = 260u32;
        let mut graph = image::RgbaImage::from_pixel(gw, gh, image::Rgba([14, 14, 20, 255]));
        let x_of = |i: usize| ((i as f64 + 0.5) / series.len() as f64 * gw as f64) as u32;
        for sc in &compiled.scenes {
            let i = (0..series.len()).find(|&i| time(i) >= sc.start);
            if let Some(i) = i {
                for y in 0..gh {
                    graph.put_pixel(x_of(i).min(gw - 1), y, image::Rgba([60, 60, 72, 255]));
                }
            }
        }
        let dmax = series.iter().map(|v| v[0]).fold(1.0f64, f64::max);
        let mmax = mo.iter().chain(mr.iter()).copied().fold(0.5f64, f64::max);
        let plot = |g: &mut image::RgbaImage, values: &[f64], max: f64, color: [u8; 4]| {
            let y_of = |v: f64| (gh as f64 - 6.0 - (v / max).clamp(0.0, 1.0) * (gh as f64 - 12.0)) as i64;
            for i in 1..values.len() {
                let (x0, x1) = (x_of(i - 1) as i64, x_of(i) as i64);
                let (y0, y1) = (y_of(values[i - 1]), y_of(values[i]));
                let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
                for k in 0..=steps {
                    let x = x0 + (x1 - x0) * k / steps;
                    let y = y0 + (y1 - y0) * k / steps;
                    for dy in 0..2 {
                        if x >= 0 && (x as u32) < gw && y + dy >= 0 && ((y + dy) as u32) < gh {
                            g.put_pixel(x as u32, (y + dy) as u32, image::Rgba(color));
                        }
                    }
                }
            }
        };
        let diffs: Vec<f64> = series.iter().map(|v| v[0]).collect();
        plot(&mut graph, &diffs, dmax, [235, 235, 240, 255]);
        plot(&mut graph, &mr, mmax, [80, 210, 255, 255]);
        plot(&mut graph, &mo, mmax, [255, 150, 40, 255]);
        let caption = format!("{:.2}–{:.2}s · difference (white, max {dmax:.0}) · motion: reference (cyan), ours (orange) · grey = cuts", time(0), time(series.len() - 1));
        let graph_png = lahza_motion_render::encode_png(&sheet::contact_sheet(&[(caption, graph)], 1, gw));
        // The worst stretches, at their middle frame.
        let mut cells = Vec::new();
        for s in &segments {
            let t = time((s.0 + s.1) / 2);
            let ours = renderer.render(&compiled, t);
            let reference = reference_frame(against, t + offset, w, h).unwrap_or_else(|_| image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255])));
            let (diff, _) = difference(&ours, &reference);
            cells.push((format!("ours {:.2}–{:.2}s", time(s.0), time(s.1)), ours));
            cells.push((format!("reference · {}", s.2), reference));
            cells.push(("difference".to_string(), diff));
        }
        renderer.set_scale(1.0);
        let mut images = vec![graph_png];
        if !cells.is_empty() {
            images.push(lahza_motion_render::encode_png(&sheet::contact_sheet(&cells, 3, 240)));
        }
        if let Some(path) = save {
            std::fs::write(&path, &images[0]).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(Output {
            json: json!({
                "frames": series.len(), "from": round(time(0)), "to": round(time(series.len() - 1)), "offset": offset,
                "mean_difference": round(diffs.iter().sum::<f64>() / diffs.len().max(1) as f64),
                "scenes": scenes, "worst": seg_json,
                "reference_ended_at": ended_at.map(round),
                "video": if written { Some(video.display().to_string()) } else { None },
            }),
            images,
        })
    }

    fn render(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let project = self.state(&dir).project.clone();
        let quality = match get_str(args, "quality").unwrap_or("final") {
            "draft" => Quality::Draft,
            "final" => Quality::Final,
            other => return Err(format!("quality '{other}': use draft or final")),
        };
        let name = dir.file_stem().and_then(|s| s.to_str()).unwrap_or("motion").to_string();
        let output = match get_str(args, "output") {
            Some(o) => self.path(o),
            None => dir.join("renders").join(format!("{name}{}.mp4", if quality == Quality::Draft { "-draft" } else { "" })),
        };
        let range = match (get_f64(args, "from"), get_f64(args, "to")) {
            (None, None) => None,
            (f, t) => Some((f.unwrap_or(0.0), t.unwrap_or(project.settings.duration))),
        };
        let options = ExportOptions {
            output: output.clone(),
            quality,
            range,
            audio: args.get("audio").and_then(Value::as_bool).unwrap_or(true),
            render: RenderOptions { backend: self.backend, ..Default::default() },
            renderer: renderer_arg(args)?,
            samples: samples_arg(args)?,
            denoise: args.get("denoise").and_then(Value::as_bool).unwrap_or(true),
        };
        let progress = Arc::new(Progress::default());
        let p2 = progress.clone();
        let dir2 = dir.clone();
        let handle = std::thread::spawn(move || {
            let compiled = Compiled::new(&project);
            export::export(&project, &compiled, &dir2, &options, &p2).map(|r: ExportReport| serde_json::to_value(r).unwrap_or_default())
        });
        let id = format!("render-{}", self.jobs.len() + 1);
        self.jobs.push((id.clone(), Job { project: dir, output, progress, started: Instant::now(), handle: Some(handle), result: None }));
        if args.get("wait").and_then(Value::as_bool).unwrap_or(false) {
            let job = &mut self.jobs.last_mut().unwrap().1;
            let result = job.handle.take().unwrap().join().unwrap_or_else(|_| Err("render panicked".into()));
            job.result = Some(result);
        }
        self.render_status(&json!({"job": id}))
    }

    fn render_status(&mut self, args: &Value) -> ToolResult {
        let wanted = get_str(args, "job");
        let mut list = Vec::new();
        for (id, job) in &mut self.jobs {
            if wanted.is_some_and(|w| w != id) {
                continue;
            }
            if job.result.is_none() && job.handle.as_ref().is_some_and(|h| h.is_finished()) {
                job.result = Some(job.handle.take().unwrap().join().unwrap_or_else(|_| Err("render panicked".into())));
            }
            let fraction = job.progress.fraction();
            let elapsed = job.started.elapsed().as_secs_f64();
            let mut v = json!({
                "job": id, "project": job.project, "output": job.output,
                "progress": round(fraction), "frames_done": job.progress.done.load(Ordering::Relaxed), "frames": job.progress.total.load(Ordering::Relaxed),
                "elapsed": round(elapsed), "status": *job.progress.status.lock().unwrap_or_else(|e| e.into_inner()),
            });
            match &job.result {
                None => {
                    v["state"] = json!("running");
                    if fraction > 0.02 {
                        v["eta_seconds"] = json!(round(elapsed * (1.0 - fraction) / fraction));
                    }
                }
                Some(Ok(report)) => {
                    v["state"] = json!("done");
                    v["report"] = report.clone();
                    if let Some(d) = report.get("dropped").filter(|d| d.as_array().is_some_and(|a| !a.is_empty())) {
                        // Rendered, but not as asked.
                        v["ok"] = json!(false);
                        v["errors"] = d.clone();
                    }
                }
                Some(Err(e)) => {
                    v["state"] = json!("failed");
                    v["error"] = json!(e);
                }
            }
            list.push(v);
        }
        if let Some(w) = wanted {
            return list.pop().map(Output::json).ok_or_else(|| format!("no render job '{w}'"));
        }
        Ok(Output::json(json!({"jobs": list})))
    }

    fn history(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let entries = io::read_history(&dir).map_err(|e| e.to_string())?;
        let undo_n = args.get("undo").and_then(Value::as_u64);
        let undo_to = args.get("undo_to").and_then(Value::as_u64);
        if args.get("redo").and_then(Value::as_bool).unwrap_or(false) {
            // Redo = revert the most recent undo that is still in effect.
            let undo = entries.iter().rev().find(|e| !e.undoes.is_empty() && !e.undone).ok_or("nothing to redo")?;
            let applied = edit::apply(&self.state(&dir).project, &undo.inverse).map_err(|e| format!("redo failed (the project changed in a conflicting way): {e}"))?;
            self.save_markers(&dir, &applied)?;
            self.save(&dir, applied.project)?;
            let mut u = undo.clone();
            u.undone = true;
            io::write_history(&dir, &u).map_err(|e| e.to_string())?;
            for e in entries.iter().filter(|e| undo.undoes.contains(&e.n)) {
                let mut e = e.clone();
                e.undone = false;
                io::write_history(&dir, &e).map_err(|e| e.to_string())?;
            }
            return Ok(Output::json(json!({"redone": undo.undoes})));
        }
        if undo_n.is_some() || undo_to.is_some() {
            let live: Vec<&HistoryEntry> = entries.iter().filter(|e| !e.undone && e.undoes.is_empty()).collect();
            let targets: Vec<&HistoryEntry> = match (undo_n, undo_to) {
                (Some(n), _) => live.iter().rev().take(n as usize).copied().collect(),
                (None, Some(to)) => live.iter().rev().filter(|e| e.n > to).copied().collect(),
                _ => unreachable!(),
            };
            if targets.is_empty() {
                return Err("nothing to undo".into());
            }
            let ops: Vec<Value> = targets.iter().flat_map(|e| e.inverse.iter().cloned()).collect();
            let applied = edit::apply(&self.state(&dir).project, &ops).map_err(|e| format!("undo failed (the project changed in a conflicting way): {e}"))?;
            let n = io::next_history_number(&dir).map_err(|e| e.to_string())?;
            self.save_markers(&dir, &applied)?;
            self.save(&dir, applied.project)?;
            for e in &targets {
                let mut e = (*e).clone();
                e.undone = true;
                io::write_history(&dir, &e).map_err(|e| e.to_string())?;
            }
            io::write_history(&dir, &HistoryEntry {
                n,
                time: io::now(),
                author: "undo".into(),
                summary: targets.iter().map(|e| format!("undid #{}: {}", e.n, e.summary.join(", "))).collect(),
                ops: ops.clone(),
                inverse: applied.inverse,
                undone: false,
                undoes: targets.iter().map(|e| e.n).collect(),
            })
            .map_err(|e| e.to_string())?;
            return Ok(Output::json(json!({"undone": targets.iter().map(|e| e.n).collect::<Vec<_>>(), "history": n})));
        }
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
        let list: Vec<Value> = entries
            .iter()
            .rev()
            .take(limit)
            .map(|e| json!({"n": e.n, "time": e.time, "author": e.author, "summary": e.summary, "undone": e.undone, "undoes": e.undoes}))
            .collect();
        Ok(Output::json(json!({"entries": list})))
    }

    fn audio_analyze(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let project = self.state(&dir).project.clone();
        let (rel, offset, register) = match get_str(args, "path") {
            Some(p) => {
                let abs = self.path(p);
                let abs = if abs.exists() { abs } else { dir.join(p) };
                let rel = match abs.strip_prefix(&dir) {
                    Ok(r) => r.to_string_lossy().into_owned(),
                    Err(_) => import_asset(&dir, &abs)?,
                };
                (rel, get_f64(args, "offset").unwrap_or(0.0), project.audio.music.is_none())
            }
            None => {
                let music = project.audio.music.as_ref().ok_or("the project has no music: pass \"path\"")?;
                let base = Compiled::new(&project).base;
                let at = music.at.resolve(&project.markers, &base, &mut lahza_motion_core::timeref::NoElements).unwrap_or(0.0);
                (music.path.clone(), get_f64(args, "offset").unwrap_or(at - music.trim), false)
            }
        };
        let analysis = lahza_motion_audio::analyze_file(&dir.join(&rel), offset)?;
        for t in &analysis.tracks {
            io::save_track(&dir, t).map_err(|e| e.to_string())?;
        }
        let mut next = project.clone();
        for t in analysis.tracks.iter().cloned() {
            next.markers.insert(t);
        }
        if register {
            next.audio.music = Some(AudioClip { id: None, name: None, path: rel.clone(), at: TimeRef::Seconds(offset), trim: 0.0, gain_db: 0.0, length: None, fade_in: 0.0, fade_out: 0.0, duck_db: None });
            if !next.assets.iter().any(|a| a.path == rel) {
                next.assets.push(Asset { path: rel.clone(), source: "user".into(), licence: "user-supplied".into(), credit: None });
            }
        }
        self.save(&dir, next)?;
        let preview = |name: &str, n: usize| -> Value {
            analysis
                .tracks
                .iter()
                .find(|t| t.name == name)
                .map(|t| t.markers.iter().take(n).enumerate().map(|(i, m)| json!({"ref": format!("{name}:{}", i + 1), "t": round(m.t), "label": m.label, "value": m.value})).collect::<Vec<_>>())
                .into()
        };
        let mut v = analysis.summary();
        v["registered_as_music"] = json!(register);
        v["bars"] = preview("bar", 12);
        v["hits"] = preview("hit", 24);
        v["stops"] = preview("stop", 24);
        v["drops"] = preview("drop", 24);
        Ok(Output::json(v))
    }
}

/// Copies a file into `<project>/assets/` and returns its relative path.
/// A free name for `file` in `folder` (`shot.png`, `shot-2.png`, …).
/// Copies `file` into `folder`, content-addressed and idempotent: it keeps
/// its name when that is free or already holds the same bytes (importing
/// twice is a no-op); a different file with the same name becomes
/// `<stem>-<hash>.<ext>`. Written through a temporary file and a rename,
/// so agents importing at once never see a half-written or clobbered file.
fn store_file(folder: &Path, file: &Path) -> Result<PathBuf, String> {
    if !file.is_file() {
        return Err(format!("{} does not exist", file.display()));
    }
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    if file.parent().and_then(|p| p.canonicalize().ok()) == folder.canonicalize().ok() {
        return Ok(file.to_path_buf());
    }
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let hash = sha_hex(&bytes);
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("media");
    let ext = file.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    for name in [format!("{stem}{ext}"), format!("{stem}-{}{ext}", &hash[..12])] {
        let dest = folder.join(&name);
        match std::fs::read(&dest) {
            Ok(existing) if existing == bytes => return Ok(dest),
            Ok(_) => continue,
            Err(_) => {
                let tmp = folder.join(format!(".{name}.{}.{}", std::process::id(), &hash[..8]));
                std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
                std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                return Ok(dest);
            }
        }
    }
    Err(format!("{}: could not store a copy", file.display()))
}

/// Pixel size of a recording's screen video.
fn recording_size(rec: &Path) -> Option<(u32, u32)> {
    let info = lahza_motion_render::footage::probe(&rec.join("screen.mkv")).ok()?;
    (info.width > 0 && info.height > 0).then_some((info.width, info.height))
}

/// `long` pixels on the long side, even dimensions, same shape.
fn fit_long_side(w: u32, h: u32, long: u32) -> (u32, u32) {
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if w >= h {
        (long, even(long as f64 * h as f64 / w as f64))
    } else {
        (even(long as f64 * w as f64 / h as f64), long)
    }
}

/// Length of the edited video: its clips in order, or the raw recording.
fn edited_duration(rec: &Path, doc: &Value) -> Option<f64> {
    if let Some(clips) = doc["clips"].as_array().filter(|c| !c.is_empty()) {
        let total = clips
            .iter()
            .map(|c| {
                let a = c["sourceStart"].as_f64().or(c["start"].as_f64()).unwrap_or(0.0);
                let b = c["sourceEnd"].as_f64().or(c["end"].as_f64()).unwrap_or(a);
                (b - a).max(0.0) / c["speed"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0) + c["gapBefore"].as_f64().unwrap_or(0.0)
            })
            .sum::<f64>();
        return Some(round(total));
    }
    lahza_motion_render::footage::probe_sound(&rec.join("screen.mkv")).ok().map(|(d, _)| round(d))
}

fn import_asset(dir: &Path, file: &Path) -> Result<String, String> {
    let assets = dir.join("assets");
    let dest = store_file(&assets, file)?;
    let name = dest.file_name().ok_or("bad file name")?.to_string_lossy().into_owned();
    Ok(format!("assets/{name}"))
}

/// JSON paths of properties that have keys.
fn animated_paths(v: &Value, prefix: &str, out: &mut Vec<String>) {
    if let Value::Object(m) = v {
        if m.contains_key("keys") || m.contains_key("add") {
            out.push(prefix.trim_start_matches('.').to_string());
            return;
        }
        for (k, child) in m {
            if k == "layers" || k == "animators" {
                continue;
            }
            animated_paths(child, &format!("{prefix}.{k}"), out);
        }
    }
}

/// Frames (sampled every half second) whose images together exceed the
/// image pool, with the texels they need. Reported once, at the first.
fn image_overflow(compiled: &Compiled, dir: &Path) -> Option<(f64, usize)> {
    use lahza_motion_core::compile::{Content, LayerState};
    let mut sizes: HashMap<String, usize> = HashMap::new();
    fn walk(layers: &[LayerState], out: &mut Vec<String>) {
        for l in layers {
            match &l.content {
                Content::Image { src, .. } => out.push(src.to_string()),
                Content::Group(c) => walk(c, out),
                _ => {}
            }
        }
    }
    let max_side = lahza_motion_render::images::MAX_SIDE as f64;
    let mut t = 0.0;
    while t < compiled.settings.duration {
        let frame = compiled.frame(t);
        let mut srcs = Vec::new();
        walk(&frame.layers, &mut srcs);
        srcs.sort();
        srcs.dedup();
        let need: usize = srcs
            .iter()
            .map(|s| {
                *sizes.entry(s.clone()).or_insert_with(|| {
                    image::image_dimensions(dir.join(s)).map(|(w, h)| {
                        let k = (max_side / w.max(h) as f64).min(1.0);
                        // With its mip chain: about a third more.
                        ((w as f64 * k) * (h as f64 * k) * 4.0 / 3.0) as usize
                    }).unwrap_or(0)
                })
            })
            .sum();
        if need > lahza_motion_render::images::MAX_TEXELS {
            return Some((t, need));
        }
        t += 0.5;
    }
    None
}

/// The reference to use for marker `i` of `track`: by name where that
/// survives edits (`line:hook`, `cue:strike`, `word:iran@2`), else by index.
fn marker_ref(track: &MarkerTrack, i: usize) -> String {
    let name = &track.name;
    let m = &track.markers[i];
    if let Some(id) = &m.id {
        return format!("{name}:{id}");
    }
    if matches!(name.as_str(), "cue" | "word") {
        if let Some(label) = &m.label {
            let key = lahza_motion_core::markers::normalize(label);
            if !key.is_empty() && !key.contains(' ') && key.chars().all(|c| c.is_alphanumeric()) && !key.chars().all(|c| c.is_ascii_digit()) {
                let n = track.markers[..=i].iter().filter(|o| o.label.as_deref().map(lahza_motion_core::markers::normalize).as_deref() == Some(key.as_str())).count();
                return if n == 1 { format!("{name}:{key}") } else { format!("{name}:{key}@{n}") };
            }
        }
    }
    format!("{name}#{}", i + 1)
}

fn sha_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Each word's loudest 10 ms, dB relative to the clip's speech level (the
/// 90th percentile of its 10 ms frames).
fn word_loudness(samples: &[f32], rate: f64, words: &mut [lahza_motion_core::narration::Word]) {
    let hop = (rate * 0.01).round().max(1.0) as usize;
    let db: Vec<f64> = samples
        .chunks(hop)
        .map(|c| {
            let rms = (c.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / c.len() as f64).sqrt();
            20.0 * rms.max(1e-6).log10()
        })
        .collect();
    if db.is_empty() {
        return;
    }
    let mut sorted = db.clone();
    sorted.sort_by(f64::total_cmp);
    let speech = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
    for w in words {
        if let (Some(s), Some(e)) = (w.start, w.end) {
            let a = ((s / 0.01).floor().max(0.0) as usize).min(db.len() - 1);
            let b = ((e / 0.01).ceil() as usize).clamp(a + 1, db.len());
            let peak = db[a..b].iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            w.loud = Some(((peak - speech) * 10.0).round() / 10.0);
        }
    }
}

/// Timed words from a Lahza transcript (`{"words": [{text, start, end}]}`,
/// possibly nested under "transcript") or whisper.cpp JSON output
/// (`{"transcription": [{"offsets": {"from", "to"}, "text"}]}`, ms; segments
/// with several words are split evenly).
fn transcript_words(v: &Value) -> Option<Vec<(String, f64, f64)>> {
    let words = v.get("words").or_else(|| v.get("transcript").and_then(|t| t.get("words")));
    if let Some(Value::Array(list)) = words {
        let out: Vec<(String, f64, f64)> = list
            .iter()
            .filter_map(|w| Some((w.get("text")?.as_str()?.to_string(), w.get("start")?.as_f64()?, w.get("end")?.as_f64()?)))
            .collect();
        return (!out.is_empty()).then_some(out);
    }
    let segments = v.get("transcription")?.as_array()?;
    let mut out = Vec::new();
    for s in segments {
        let text = s.get("text")?.as_str()?;
        let from = s.get("offsets")?.get("from")?.as_f64()? / 1000.0;
        let to = s.get("offsets")?.get("to")?.as_f64()? / 1000.0;
        let parts: Vec<&str> = text.split_whitespace().collect();
        let n = parts.len().max(1) as f64;
        for (i, w) in parts.iter().enumerate() {
            let a = from + (to - from) * i as f64 / n;
            let b = from + (to - from) * (i + 1) as f64 / n;
            out.push((w.to_string(), a, b));
        }
    }
    (!out.is_empty()).then_some(out)
}

pub fn png_base64(png: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(png)
}

const BLENDER_WRAPPER: &str = r#"
import bpy, json, os, sys, time, traceback
LAHZA = json.load(open(os.environ["LAHZA_BLENDER"]))
scene = bpy.context.scene
r = scene.render
preview = LAHZA["quality"] == "preview"
r.resolution_x, r.resolution_y = LAHZA["width"], LAHZA["height"]
r.resolution_percentage = 50 if preview else 100
r.fps = int(round(LAHZA["fps"]))
r.fps_base = r.fps / LAHZA["fps"]
scene.frame_start, scene.frame_end = LAHZA["frame_start"], LAHZA["frame_end"]
r.film_transparent = LAHZA["transparent"]
r.image_settings.file_format = "PNG"
r.image_settings.color_mode = "RGBA" if LAHZA["transparent"] else "RGB"
r.image_settings.color_depth = "8"
r.filepath = os.path.join(LAHZA["out"], LAHZA["name"] + "_####")
r.use_motion_blur = not preview

def set_engine(names):
    for name in names:
        try:
            r.engine = name
            return
        except Exception:
            pass

# Preview: EEVEE when this build has it (it renders headless on a GPU).
set_engine([LAHZA["engine"]] if LAHZA.get("engine") else ["BLENDER_EEVEE", "BLENDER_EEVEE_NEXT", "CYCLES"] if preview else ["CYCLES"])

def enable_gpu():
    """Turn on every device of the best GPU backend; CPU when none."""
    try:
        prefs = bpy.context.preferences.addons["cycles"].preferences
    except Exception:
        return "CPU", []
    for kind in ("OPTIX", "CUDA", "HIP", "ONEAPI", "METAL"):
        try:
            prefs.compute_device_type = kind
        except Exception:
            continue
        prefs.get_devices()
        found = [d for d in prefs.devices if d.type == kind]
        if found:
            for d in prefs.devices:
                d.use = d.type == kind
            return kind, [d.name for d in found]
    try:
        prefs.compute_device_type = "NONE"
    except Exception:
        pass
    return "CPU", []

device, devices = enable_gpu() if LAHZA["gpu"] else ("CPU", [])
scene.cycles.device = "CPU" if device == "CPU" else "GPU"
samples = 8 if preview else LAHZA["samples"]
scene.cycles.samples = samples
try:
    scene.eevee.taa_render_samples = samples
except Exception:
    pass
scene.cycles.use_denoising = not preview
if not preview:
    # Some builds (e.g. Ubuntu's) ship without OpenImageDenoise.
    # The flag isn't reliable either, so try the denoisers in turn.
    oidn = getattr(bpy.app.build_options, "openimagedenoise", True)
    choices = (["OPENIMAGEDENOISE"] if oidn else []) + (["OPTIX"] if device == "OPTIX" else [])
    for choice in choices:
        try:
            scene.cycles.denoiser = choice
            break
        except (TypeError, ValueError):
            pass
    else:
        scene.cycles.use_denoising = False

def frame_of(seconds):
    """Blender frame showing project time `seconds` (frame_start lands at LAHZA["at"])."""
    return LAHZA["frame_start"] + round((seconds - LAHZA["at"]) * LAHZA["fps"])

script = LAHZA.get("script")
if script:
    try:
        exec(compile(open(script).read(), script, "exec"), {"bpy": bpy, "LAHZA": LAHZA, "frame_of": frame_of, "__name__": "__main__"})
    except Exception:
        traceback.print_exc()
        sys.exit(3)

times = []
started = [0.0]
def before(scene, *_):
    started[0] = time.perf_counter()
def after(scene, *_):
    s = round(time.perf_counter() - started[0], 3)
    times.append({"frame": scene.frame_current, "seconds": s})
    print("LAHZA_FRAME %d %.3f" % (scene.frame_current, s), flush=True)
bpy.app.handlers.render_pre.append(before)
bpy.app.handlers.render_post.append(after)

if LAHZA["render"]:
    if preview:
        # A few frames spread over the range, for a look at the timing.
        fs, fe, n = scene.frame_start, scene.frame_end, max(1, LAHZA["preview_frames"])
        frames = sorted({round(fs + (fe - fs) * i / max(1, n - 1)) for i in range(n)})
        for f in frames:
            scene.frame_set(f)
            r.filepath = os.path.join(LAHZA["out"], "%s_%04d.png" % (LAHZA["name"], f))
            bpy.ops.render.render(write_still=True)
    else:
        bpy.ops.render.render(animation=True)
print("LAHZA_DONE " + json.dumps({"frame_start": scene.frame_start, "frame_end": scene.frame_end,
    "width": r.resolution_x * r.resolution_percentage // 100, "height": r.resolution_y * r.resolution_percentage // 100,
    "fps": r.fps / r.fps_base, "engine": r.engine, "device": device, "devices": devices,
    "samples": scene.cycles.samples if r.engine == "CYCLES" else samples, "motion_blur": r.use_motion_blur,
    "denoise": r.engine == "CYCLES" and scene.cycles.use_denoising, "frame_times": times}))
"#;

impl Toolbox {
    fn media_import(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let path = get_str(args, "path").ok_or("media_import needs \"path\"")?;
        let copy = args.get("copy").and_then(Value::as_bool).unwrap_or(true);
        let abs = if path.contains('#') { self.path(path) } else { self.file_in(&dir, path)? };
        // Sequences: "shot_####.png" or a folder of frames.
        let (rel, probe_src) = if path.contains('#') || abs.is_dir() {
            let folder = if abs.is_dir() { abs.clone() } else { abs.parent().map(Path::to_path_buf).unwrap_or_default() };
            let rel = match folder.strip_prefix(&dir) {
                Ok(r) => r.to_string_lossy().into_owned(),
                Err(_) if copy => {
                    let name = folder.file_name().ok_or("bad folder")?.to_string_lossy().into_owned();
                    std::fs::create_dir_all(dir.join("assets")).map_err(|e| e.to_string())?;
                    let dest = copy_dir(&folder, &dir.join("assets").join(&name))?;
                    format!("assets/{}", dest.file_name().unwrap_or_default().to_string_lossy())
                }
                Err(_) => return Err("sequences outside the project must be copied (copy: true)".into()),
            };
            let src = if abs.is_dir() { rel.clone() } else { format!("{rel}/{}", abs.file_name().unwrap().to_string_lossy()) };
            (src.clone(), src)
        } else {
            let shared = recording::recording_of(&dir);
            let rel = match (abs.strip_prefix(&dir), &shared) {
                (Ok(r), _) => r.to_string_lossy().into_owned(),
                // Files of the recording are used where they are.
                (Err(_), Some(rec)) if abs.starts_with(rec) => recording::shared_path(&dir, rec, &abs).ok_or("bad path")?,
                // Other files go to the recording's shared media folder.
                (Err(_), Some(rec)) if copy => {
                    let dest = store_file(&rec.join("media"), &abs)?;
                    recording::shared_path(&dir, rec, &dest).ok_or("bad path")?
                }
                (Err(_), _) if copy => import_asset(&dir, &abs)?,
                (Err(_), _) => return Err("files outside the project must be copied (copy: true)".into()),
            };
            (rel.clone(), rel)
        };
        let mut footage = lahza_motion_render::footage::Footage::new(Some(&dir));
        let audio_ext = Path::new(&probe_src)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "aiff", "aif"].contains(&e.to_ascii_lowercase().as_str()));
        let info = if audio_ext { None } else { footage.info(&probe_src) };
        let Some(info) = info else {
            // Audio-only files are sounds, not layers.
            let samples = lahza_motion_audio::decode(&dir.join(&probe_src))
                .map_err(|e| format!("cannot read '{probe_src}' as footage ({}) or as audio ({e})", footage.warnings.join("; ")))?;
            self.register_asset(&dir, &rel, args)?;
            let duration = samples.len() as f64 / lahza_motion_audio::SAMPLE_RATE as f64;
            return Ok(Output::json(json!({
                "src": rel, "kind": "audio", "info": {"duration": (duration * 1000.0).round() / 1000.0},
                "use": format!("audio is a sound, not a layer: sfx_add {{\"sound\": \"{rel}\", \"at\": …}}, or the add_sound / set_music edit ops with \"path\": \"{rel}\""),
                "op": {"op": "add_sound", "path": rel, "at": 0},
            })));
        };
        self.register_asset(&dir, &rel, args)?;
        Ok(Output::json(json!({
            "src": rel, "info": info,
            "layer": {"kind": "video", "src": rel, "transform": {"position": [self.state(&dir).project.settings.width / 2, self.state(&dir).project.settings.height / 2]}},
        })))
    }

    /// Records an imported file in the project's asset list (once).
    fn register_asset(&mut self, dir: &Path, rel: &str, args: &Value) -> Result<(), String> {
        let mut project = self.state(dir).project.clone();
        if !project.assets.iter().any(|a| a.path == rel) {
            project.assets.push(Asset {
                path: rel.to_string(),
                source: get_str(args, "source").unwrap_or("user").into(),
                licence: get_str(args, "licence").unwrap_or("user-supplied").into(),
                credit: None,
            });
            self.save(dir, project)?;
        }
        Ok(())
    }

    fn blender_run(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let name = get_str(args, "name").ok_or("blender_run needs \"name\"")?.to_string();
        if !lahza_motion_core::document::valid_id(&name) {
            return Err("name: letters, digits, '-' and '_' only".into());
        }
        let blender = which("blender").ok_or("Blender is not installed (Ubuntu: sudo apt install blender, or set LAHZA_BLENDER to its path)")?;
        let settings = self.state(&dir).project.settings.clone();
        let preview = match get_str(args, "quality").unwrap_or("final") {
            "preview" => true,
            "final" => false,
            other => return Err(format!("quality '{other}': use preview or final")),
        };
        // A preview never replaces the final frames.
        let folder = if preview { format!("{name}-preview") } else { name.clone() };
        let out = dir.join("assets").join("blender").join(&folder);
        // A fresh render replaces the previous one.
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let scripts = dir.join("blender");
        std::fs::create_dir_all(&scripts).map_err(|e| e.to_string())?;
        let script_path = match get_str(args, "script") {
            Some(code) => {
                let p = scripts.join(format!("{name}.py"));
                std::fs::write(&p, code).map_err(|e| e.to_string())?;
                Some(p)
            }
            None => None,
        };
        let blend = match get_str(args, "blend") {
            Some(b) => {
                let p = self.path(b);
                let p = if p.exists() { p } else { dir.join(b) };
                if !p.is_file() {
                    return Err(format!("{} does not exist", p.display()));
                }
                Some(p)
            }
            None => None,
        };
        if script_path.is_none() && blend.is_none() {
            return Err("give a bpy \"script\", a \"blend\" file, or both".into());
        }
        let fps = settings.fps.as_f64();
        let frames = args.get("frames").and_then(Value::as_array).filter(|a| a.len() == 2);
        let (fs, fe) = match frames {
            Some(a) => (a[0].as_i64().unwrap_or(1), a[1].as_i64().unwrap_or(1)),
            None => (1, (settings.duration * fps).round().max(1.0) as i64),
        };
        let res = args.get("resolution").and_then(Value::as_array).filter(|a| a.len() == 2);
        let (w, h) = match res {
            Some(a) => (a[0].as_u64().unwrap_or(1920), a[1].as_u64().unwrap_or(1080)),
            None => (settings.width as u64, settings.height as u64),
        };
        let at = get_f64(args, "at").unwrap_or(0.0);
        let config = json!({
            "out": out, "name": name, "width": w, "height": h, "fps": fps, "frame_start": fs, "frame_end": fe,
            "quality": if preview { "preview" } else { "final" }, "preview_frames": args.get("preview_frames").and_then(Value::as_u64).unwrap_or(6),
            "engine": get_str(args, "engine"), "samples": args.get("samples").and_then(Value::as_u64).unwrap_or(64),
            "gpu": args.get("gpu").and_then(Value::as_bool).unwrap_or(true),
            "transparent": args.get("transparent").and_then(Value::as_bool).unwrap_or(true),
            "render": args.get("render").and_then(Value::as_bool).unwrap_or(true),
            "script": script_path, "at": at,
            "markers": self.blender_markers(&dir, fps, fs, at),
            "elements": self.blender_elements(&dir, fps, fs, at),
        });
        let wrapper = scripts.join("_lahza_wrapper.py");
        std::fs::write(&wrapper, BLENDER_WRAPPER).map_err(|e| e.to_string())?;
        // A file, not the environment: marker lists outgrow an env variable.
        let config_path = scripts.join(format!("{name}.lahza.json"));
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap_or_default()).map_err(|e| e.to_string())?;
        let timeout = std::time::Duration::from_secs_f64(get_f64(args, "timeout").unwrap_or(1800.0).max(1.0));
        let rel_pattern = format!("assets/blender/{folder}/{name}_####.png");
        let source = match (&script_path, &blend) {
            (Some(s), _) => format!("blender script {}", s.strip_prefix(&dir).unwrap_or(s).display()),
            (None, Some(b)) => format!("blender file {}", b.display()),
            _ => unreachable!(),
        };
        let dir2 = dir.clone();
        let rel2 = rel_pattern.clone();
        let progress = Arc::new(Progress::default());
        let preview_count = args.get("preview_frames").and_then(Value::as_u64).unwrap_or(6).clamp(1, (fe - fs + 1).max(1) as u64);
        progress.total.store(if preview { preview_count } else { (fe - fs + 1).max(1) as u64 }, Ordering::Relaxed);
        let p2 = progress.clone();
        let out2 = out.clone();
        let handle = std::thread::spawn(move || -> Result<Value, String> {
            let mut cmd = std::process::Command::new(blender);
            cmd.args(["-b", "--factory-startup"]);
            if let Some(b) = &blend {
                cmd.arg(b);
            }
            cmd.arg("--python").arg(&wrapper).env("LAHZA_BLENDER", &config_path).current_dir(&dir2);
            // Blender prints a lot while rendering: write it to a log file
            // (an undrained pipe would block Blender once it fills).
            let log_path = dir2.join("blender").join(format!("{}.log", config["name"].as_str().unwrap_or("blender")));
            let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
            let log_err = log.try_clone().map_err(|e| e.to_string())?;
            cmd.stdout(log).stderr(log_err);
            let mut child = cmd.spawn().map_err(|e| format!("cannot start Blender: {e}"))?;
            let started = Instant::now();
            loop {
                if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                    let output = std::fs::read_to_string(&log_path).unwrap_or_default();
                    let tail = |s: &str| s.lines().rev().take(25).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                    let done = output.lines().find_map(|l| l.strip_prefix("LAHZA_DONE ")).and_then(|j| serde_json::from_str::<Value>(j).ok());
                    if !status.success() || done.is_none() {
                        return Err(format!("Blender failed ({status}); log {}:\n{}", log_path.display(), tail(&output)));
                    }
                    let count = std::fs::read_dir(&out2).map(|r| r.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "png")).count()).unwrap_or(0);
                    p2.done.store(count as u64, Ordering::Relaxed);
                    let mut done = done.unwrap_or_default();
                    let frame_times = done.as_object_mut().and_then(|d| d.remove("frame_times")).unwrap_or(json!([]));
                    let per_frame: Vec<f64> = frame_times.as_array().into_iter().flatten().filter_map(|f| f["seconds"].as_f64()).collect();
                    let mut report = json!({"src": rel2, "frames": count, "blender": done, "seconds": round(started.elapsed().as_secs_f64()),
                        "frame_times": frame_times,
                        "seconds_per_frame": if per_frame.is_empty() { Value::Null } else { json!(round(per_frame.iter().sum::<f64>() / per_frame.len() as f64)) },
                        "layer": {"kind": "video", "src": rel2, "fps": done.get("fps").cloned().unwrap_or(json!(fps))}});
                    if preview {
                        report["preview"] = json!(true);
                        report["note"] = json!("a few frames at half size for a look: render quality \"final\" for the footage");
                        report.as_object_mut().map(|r| r.remove("layer"));
                    }
                    return Ok(report);
                }
                if p2.cancel.load(Ordering::Relaxed) || started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(if started.elapsed() > timeout { "Blender timed out".into() } else { "cancelled".into() });
                }
                let count = std::fs::read_dir(&out2).map(|r| r.flatten().count()).unwrap_or(0);
                p2.done.store(count as u64, Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
        // Record the asset now; the files appear as Blender renders.
        let mut project = self.state(&dir).project.clone();
        if !preview && !project.assets.iter().any(|a| a.path == rel_pattern) {
            project.assets.push(Asset { path: rel_pattern.clone(), source, licence: "generated".into(), credit: None });
            self.save(&dir, project)?;
        }
        let id = format!("blender-{}", self.jobs.len() + 1);
        self.jobs.push((id.clone(), Job { project: dir, output: out.clone(), progress, started: Instant::now(), handle: Some(handle), result: None }));
        if args.get("wait").and_then(Value::as_bool).unwrap_or(true) {
            let job = &mut self.jobs.last_mut().unwrap().1;
            let result = job.handle.take().unwrap().join().unwrap_or_else(|_| Err("Blender job panicked".into()));
            job.result = Some(result);
        }
        let mut status = self.render_status(&json!({"job": id}))?;
        // A preview comes back as a sheet of its frames.
        if preview && status.json["state"] == "done" {
            let mut cells = Vec::new();
            let mut files: Vec<PathBuf> = std::fs::read_dir(&out).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png")).collect()).unwrap_or_default();
            files.sort();
            for f in &files {
                if let Ok(img) = image::open(f) {
                    let n: i64 = f.file_stem().and_then(|s| s.to_str()).and_then(|s| s.rsplit('_').next()).and_then(|s| s.parse().ok()).unwrap_or(0);
                    cells.push((format!("f{n} · {:.2}s", at + (n - fs) as f64 / fps), img.to_rgba8()));
                }
            }
            if !cells.is_empty() {
                let columns = (cells.len() as f64).sqrt().ceil().clamp(1.0, 4.0) as u32;
                status.images.push(lahza_motion_render::encode_png(&sheet::contact_sheet(&cells, columns, 320)));
            }
        }
        Ok(status)
    }

    /// Marker tracks for a Blender script: seconds, the Blender frame
    /// showing each (frame_start lands on `at`), and labels (words).
    fn blender_markers(&mut self, dir: &Path, fps: f64, frame_start: i64, at: f64) -> Value {
        let frame = |t: f64| frame_start + ((t - at) * fps).round() as i64;
        let tracks = &self.state(dir).project.markers.tracks;
        let mut out = Map::new();
        for (name, track) in tracks {
            let mut v = json!({
                "seconds": track.markers.iter().map(|m| round(m.t)).collect::<Vec<_>>(),
                "frames": track.markers.iter().map(|m| frame(m.t)).collect::<Vec<_>>(),
            });
            if track.markers.iter().any(|m| m.label.is_some()) {
                v["labels"] = json!(track.markers.iter().map(|m| m.label.clone().unwrap_or_default()).collect::<Vec<_>>());
            }
            out.insert(name.clone(), v);
        }
        Value::Object(out)
    }

    /// Resolved start and end of every scene and layer (by id, and by name
    /// when it has one), in seconds and Blender frames.
    fn blender_elements(&mut self, dir: &Path, fps: f64, frame_start: i64, at: f64) -> Value {
        let compiled = self.compiled(dir);
        let frame = |t: f64| frame_start + ((t - at) * fps).round() as i64;
        let window = |start: f64, end: f64| json!({"start": round(start), "end": round(end), "start_frame": frame(start), "end_frame": frame(end)});
        let mut out = Map::new();
        fn walk(list: &[lahza_motion_core::document::Layer], compiled: &Compiled, out: &mut Map<String, Value>, window: &dyn Fn(f64, f64) -> Value) {
            for l in list {
                if let Some((s, e)) = compiled.element_window(&l.id) {
                    out.insert(l.id.to_string(), window(s, e));
                    if let Some(name) = &l.common.name {
                        out.entry(name.clone()).or_insert_with(|| window(s, e));
                    }
                }
                walk(l.children(), compiled, out, window);
            }
        }
        let project = &self.state(dir).project;
        for (scene, c) in project.scenes.iter().zip(&compiled.scenes) {
            out.insert(scene.id.to_string(), window(c.start, c.end));
            walk(&scene.layers, &compiled, &mut out, &window);
        }
        Value::Object(out)
    }

    /// A file named by the agent: absolute, or relative to the working
    /// folder or (failing that) the current project.
    fn input_file(&self, p: &str) -> PathBuf {
        let abs = self.path(p);
        match &self.current {
            Some(dir) if !abs.exists() && dir.join(p).exists() => dir.join(p),
            _ => abs,
        }
    }

    /// A file named for project `dir`: absolute, relative to the working
    /// folder, or (when that does not exist) relative to the project.
    fn file_in(&self, dir: &Path, p: &str) -> Result<PathBuf, String> {
        let abs = self.path(p);
        if abs.exists() {
            return Ok(abs);
        }
        let inside = dir.join(p);
        if inside.exists() {
            return Ok(inside.canonicalize().unwrap_or(inside));
        }
        Err(format!("no file '{p}': looked in {} and in the project {}", self.cwd.display(), dir.display()))
    }

    fn track(&mut self, args: &Value) -> ToolResult {
        let video = self.input_file(get_str(args, "video").ok_or("track needs \"video\"")?);
        let target = crate::measure::Target::from_json(args)?;
        let from = get_f64(args, "from").unwrap_or(0.0);
        let width = args.get("width").and_then(Value::as_u64).map(|w| w as u32);
        let (info, samples) = crate::measure::track(&video, &target, from, get_f64(args, "to"), width)?;
        let found = samples.iter().filter(|s| s.found).count();
        let mut out = json!({
            "video": video.display().to_string(), "size": [info.width, info.height], "fps": round(info.fps),
            "frames": samples.len(), "found": found,
            "track": samples.iter().map(|s| s.to_json()).collect::<Vec<_>>(),
        });
        if let Some(k) = args.get("keys").filter(|k| k.is_object()) {
            let offset = k.get("offset").and_then(Value::as_array).map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]).unwrap_or([0.0, 0.0]);
            let rotate = k.get("rotate").and_then(Value::as_bool).unwrap_or(!matches!(target, crate::measure::Target::Box { .. }));
            let keys = crate::measure::keys(&samples, from, get_f64(k, "at").unwrap_or(0.0), get_f64(k, "scale").unwrap_or(1.0), offset, get_f64(k, "base_scale").unwrap_or(1.0), rotate);
            if let (Some(layer), Some(map)) = (get_str(k, "layer"), keys.as_object()) {
                out["ops"] = json!(map.iter().map(|(path, v)| json!({"op": "set", "target": format!("{layer}.{path}"), "value": v})).collect::<Vec<_>>());
            }
            out["keys"] = keys;
        }
        Ok(Output::json(out))
    }

    fn fit_curve(&mut self, args: &Value) -> ToolResult {
        let list = args.get("samples").and_then(Value::as_array).ok_or("fit_curve needs \"samples\": [[t, value], …]")?;
        let mut samples = Vec::with_capacity(list.len());
        for s in list {
            let pair = s.as_array().filter(|a| a.len() == 2).ok_or("each sample is [t, value]")?;
            let t = pair[0].as_f64().ok_or("sample times are numbers")?;
            let v: Vec<f64> = match &pair[1] {
                Value::Number(n) => vec![n.as_f64().unwrap_or(0.0)],
                Value::Array(a) => a.iter().map(|x| x.as_f64().ok_or("sample values are numbers or [x, y]")).collect::<Result<_, _>>()?,
                _ => return Err("sample values are numbers or [x, y]".into()),
            };
            samples.push((t, v));
        }
        let project_fps = match (get_str(args, "project"), &self.current) {
            (None, None) => None,
            _ => self.dir(args).ok().map(|dir| self.state(&dir).project.settings.fps),
        };
        let fps = match get_f64(args, "fps") {
            Some(f) => Fps::from_f64(f).map_err(|e| e.to_string())?,
            None => project_fps.unwrap_or(Fps::whole(30)),
        };
        crate::measure::fit_curve(&samples, fps).map(Output::json)
    }
}

/// One frame of a reference video (or an image) at `t`, fitted into w×h
/// on black so it lines up with our frame.
/// A video's first frame time and frame rate (None for stills or when
/// ffprobe can't tell).
fn ref_timing(path: &Path) -> Option<(f64, f64)> {
    let still = path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp"));
    if still {
        return None;
    }
    let out = std::process::Command::new(which("ffprobe")?)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=avg_frame_rate,r_frame_rate,start_time", "-of", "json"])
        .arg(path)
        .output()
        .ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let stream = v["streams"].get(0)?;
    let ratio = |s: &str| -> Option<f64> {
        let (a, b) = s.split_once('/')?;
        let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
        (a > 0.0 && b > 0.0).then_some(a / b)
    };
    let rate = stream["avg_frame_rate"].as_str().and_then(ratio).or_else(|| stream["r_frame_rate"].as_str().and_then(ratio))?;
    let start = stream["start_time"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    Some((start, rate))
}

/// A seek time that lands exactly on reference frame `index` (a quarter
/// frame before its timestamp, so float rounding can't skip it).
fn seek_before(timing: Option<(f64, f64)>, index: i64) -> f64 {
    timing.map(|(s, r)| (s + index as f64 / r - 0.25 / r).max(0.0)).unwrap_or(0.0)
}

fn reference_frame(path: &Path, t: f64, w: u32, h: u32) -> Result<image::RgbaImage, String> {
    let ffmpeg = which("ffmpeg").ok_or("compare needs ffmpeg")?;
    let filter = format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black");
    // The frame on screen at t; a still reference is the same at every time.
    let timing = ref_timing(path);
    let seek = seek_before(timing, timing.map(|(s, r)| ((t - s) * r + 1e-6).floor().max(0.0) as i64).unwrap_or(0));
    let out = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-ss", &format!("{seek:.6}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", &filter, "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    let expected = (w * h * 4) as usize;
    if !out.status.success() || out.stdout.len() < expected {
        return Err(format!("no reference frame at {t:.2}s in {}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    image::RgbaImage::from_raw(w, h, out.stdout[..expected].to_vec()).ok_or_else(|| "bad reference frame".into())
}

/// Fractions of the reference's edge pixels with no edge of ours within
/// 1 px (missing detail), and of ours with none of the reference's (extra).
pub(crate) fn detail(ours: &image::RgbaImage, reference: &image::RgbaImage) -> (f64, f64) {
    const EDGE: f32 = 60.0;
    let (a, w, h) = crate::measure::edges(ours);
    let (b, _, _) = crate::measure::edges(reference);
    let near = |e: &[f32], x: u32, y: u32| -> bool {
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx >= 0 && ny >= 0 && (nx as u32) < w && (ny as u32) < h && e[(ny as u32 * w + nx as u32) as usize] > EDGE * 0.5 {
                    return true;
                }
            }
        }
        false
    };
    let unmatched = |src: &[f32], other: &[f32]| -> f64 {
        let (mut total, mut lost) = (0u32, 0u32);
        for y in 0..h {
            for x in 0..w {
                if src[(y * w + x) as usize] > EDGE {
                    total += 1;
                    if !near(other, x, y) {
                        lost += 1;
                    }
                }
            }
        }
        if total < 20 { 0.0 } else { lost as f64 / total as f64 }
    };
    (unmatched(&b, &a), unmatched(&a, &b))
}

/// Per-pixel |ours − reference| (amplified 3× so small differences show)
/// and the mean absolute channel difference, 0–255.
fn difference(a: &image::RgbaImage, b: &image::RgbaImage) -> (image::RgbaImage, f64) {
    let (w, h) = a.dimensions();
    let mut out = image::RgbaImage::new(w, h);
    let mut total = 0u64;
    for ((pa, pb), po) in a.pixels().zip(b.pixels()).zip(out.pixels_mut()) {
        let mut c = [0u8, 0, 0, 255];
        for k in 0..3 {
            let d = (pa.0[k] as i32 - pb.0[k] as i32).unsigned_abs();
            total += d as u64;
            c[k] = (d * 3).min(255) as u8;
        }
        po.0 = c;
    }
    (out, total as f64 / (w as f64 * h as f64 * 3.0))
}

fn which(program: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LAHZA_BLENDER").filter(|_| program == "blender") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("PATH")?.to_str()?.split(':').map(|d| Path::new(d).join(program)).find(|p| p.is_file())
}

/// Copies a folder of frames; like `store_file`, an existing folder of the
/// same name is reused when it holds the same files, else the copy gets
/// `<name>-<hash>`. Returns the folder used.
fn copy_dir(from: &Path, to: &Path) -> Result<PathBuf, String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for e in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        if e.path().is_file() {
            files.push((e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).map_err(|e| e.to_string())?));
        }
    }
    files.sort();
    let mut all = Vec::new();
    for (n, b) in &files {
        all.extend_from_slice(n.as_bytes());
        all.extend_from_slice(&sha_hex(b).into_bytes());
    }
    let hash = sha_hex(&all);
    let same = |dir: &Path| files.iter().all(|(n, b)| std::fs::read(dir.join(n)).is_ok_and(|e| &e == b));
    let alt = to.with_file_name(format!("{}-{}", to.file_name().and_then(|n| n.to_str()).unwrap_or("frames"), &hash[..12]));
    for dest in [to.to_path_buf(), alt] {
        if dest.exists() {
            if same(&dest) {
                return Ok(dest);
            }
            continue;
        }
        let tmp = dest.with_file_name(format!(".{}.{}", dest.file_name().and_then(|n| n.to_str()).unwrap_or("frames"), std::process::id()));
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        for (n, b) in &files {
            std::fs::write(tmp.join(n), b).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        return Ok(dest);
    }
    Err(format!("{}: could not store a copy", from.display()))
}
