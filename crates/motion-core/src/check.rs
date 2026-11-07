//! `check` (design section 10): structure, timing (sync drift per anchor,
//! long holds), layout (title-safe, overlapping text, contrast) and
//! licensing. Sync is measured the way the studies measured references:
//! on the rendered frame grid, through stepped clocks and pre-roll, not
//! from the requested numbers.

use crate::catalog::style_pack;
use crate::compile::{Compiled, Content, EventKind, FrameState, LayerState, SyncKind, SyncRecord};
use crate::document::{walk_layers, Facing, Layer, LayerKind, Project, Role, Scene, ShapeKind, TransitionKind};
use crate::space::Proj;
use crate::timeref::Anchor;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Issue {
    pub severity: Severity,
    /// structure, timing, layout, licence
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SyncResult {
    pub element: String,
    pub what: String,
    pub reference: String,
    pub anchor: Anchor,
    pub requested: f64,
    pub achieved: f64,
    /// achieved − requested, in frames.
    pub drift_frames: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub issues: Vec<Issue>,
    pub syncs: Vec<SyncResult>,
    /// Largest |drift| in frames.
    pub max_drift_frames: f64,
    /// Issues grouped by cause (the same message for different elements).
    pub causes: Vec<Cause>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cause {
    pub severity: Severity,
    pub kind: &'static str,
    /// The message with numbers and quoted names generalised.
    pub what: String,
    pub count: usize,
    /// Up to five elements it applies to.
    pub elements: Vec<String>,
}

impl Report {
    /// Adds an issue found outside `check` (the renderer's fonts and
    /// images) and recounts.
    pub fn add(&mut self, severity: Severity, kind: &'static str, element: Option<&str>, at: Option<f64>, message: String) {
        self.issues.push(issue(severity, kind, element, at, message));
        self.issues.sort_by(|a, b| b.severity.cmp(&a.severity));
        self.errors = self.issues.iter().filter(|i| i.severity == Severity::Error).count();
        self.warnings = self.issues.iter().filter(|i| i.severity == Severity::Warning).count();
        self.ok = self.errors == 0;
        self.causes = causes(&self.issues);
    }
}

fn causes(issues: &[Issue]) -> Vec<Cause> {
    let general = |m: &str| -> String {
        let mut out = String::new();
        let mut quoted = false;
        let mut last_digit = false;
        for c in m.chars() {
            if c == '\'' {
                if !quoted {
                    out.push_str("'…'");
                }
                quoted = !quoted;
                continue;
            }
            if quoted {
                continue;
            }
            let digit = c.is_ascii_digit() || (last_digit && c == '.');
            if digit {
                if !last_digit {
                    out.push('#');
                }
            } else {
                out.push(c);
            }
            last_digit = digit;
        }
        out
    };
    let mut out: Vec<Cause> = Vec::new();
    for i in issues {
        let what = general(&i.message);
        match out.iter_mut().find(|c| c.severity == i.severity && c.kind == i.kind && c.what == what) {
            Some(c) => {
                c.count += 1;
                if let Some(e) = &i.element {
                    if c.elements.len() < 5 && !c.elements.contains(e) {
                        c.elements.push(e.clone());
                    }
                }
            }
            None => out.push(Cause { severity: i.severity, kind: i.kind, what, count: 1, elements: i.element.iter().cloned().collect() }),
        }
    }
    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(b.count.cmp(&a.count)));
    out
}

/// Frame layers with comps opened: children carry their parents'
/// transforms and opacity, in drawing order.
fn flatten(layers: &[LayerState]) -> Vec<LayerState> {
    let mut out = Vec::new();
    for l in layers {
        match &l.content {
            Content::Group(children) => {
                let kids: Vec<LayerState> = children
                    .iter()
                    .map(|c| LayerState { matrix: l.matrix.then(c.matrix), opacity: c.opacity * l.opacity, ..c.clone() })
                    .collect();
                out.extend(flatten(&kids));
            }
            _ => out.push(l.clone()),
        }
    }
    out
}

/// Measures a layer's on-canvas bounds [x0, y0, x1, y1]; the renderer
/// supplies text bounds (it owns font metrics).
pub type Measure<'a> = &'a dyn Fn(&LayerState) -> Option<[f64; 4]>;

pub struct Options<'a> {
    /// Project folder, for asset existence and licences.
    pub dir: Option<&'a Path>,
    pub measure: Option<Measure<'a>>,
    /// Allowed |drift| in frames before it is an error.
    pub tolerance_frames: f64,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self { dir: None, measure: None, tolerance_frames: 1.0 }
    }
}

/// Where a requested sync point lands on the frame grid (timeline seconds).
fn achieved(compiled: &Compiled, s: &SyncRecord) -> f64 {
    let fps = compiled.fps();
    let f = fps.as_f64();
    match &s.kind {
        SyncKind::Edge => ((s.requested * f - 1e-6).ceil()) / f,
        SyncKind::Move { curve, start, dur, step, shift } => {
            let p = |k: i64| {
                let tc = fps.time_of(k) + shift;
                let tl = step.map(|st| st.quantize(tc)).unwrap_or(tc);
                curve.progress(tl - start, *dur)
            };
            let lo = ((start - shift) * f).floor() as i64 - 2;
            let hi = ((start - shift + dur) * f).ceil() as i64 + 2;
            let k = match s.anchor {
                Anchor::Start => (lo..=hi).find(|&k| p(k) > 1e-6).map(|k| k - 1),
                Anchor::Peak => (lo..=hi).max_by(|a, b| {
                    let va = (p(a + 1) - p(a - 1)).abs();
                    let vb = (p(b + 1) - p(b - 1)).abs();
                    // Earliest of equals.
                    va.total_cmp(&vb).then(b.cmp(a))
                }),
                Anchor::Land => (lo..=hi).find(|&k| p(k) >= 0.98),
                Anchor::End => (lo..=hi).find(|&k| p(k) >= 1.0 - 1e-9),
            };
            k.map(|k| fps.time_of(k)).unwrap_or(s.requested)
        }
    }
}

pub fn check(project: &Project, compiled: &Compiled, options: &Options) -> Report {
    let mut issues = Vec::new();
    for e in &compiled.errors {
        issues.push(Issue { severity: Severity::Error, kind: "structure", element: e.split(['.', ':']).next().map(String::from), at: None, message: e.clone() });
    }
    if project.scenes.is_empty() {
        issues.push(issue(Severity::Warning, "structure", None, None, "the project has no scenes".into()));
    }
    // Sounds: their times must resolve (an anchored effect follows its layer).
    for clip in project.audio.clips() {
        let id = clip.id.clone().unwrap_or_else(|| clip.path.clone());
        match compiled.resolve(&project.markers, &clip.at) {
            Err(e) => issues.push(issue(Severity::Error, "sound", Some(&id), None, format!("sound '{id}' at: {e}"))),
            Ok(t) if t >= compiled.settings.duration => {
                issues.push(issue(Severity::Warning, "sound", Some(&id), Some(t), format!("sound '{id}' starts after the end ({t:.3} s)")))
            }
            Ok(_) => {}
        }
    }
    let fps = compiled.fps();
    let f = fps.as_f64();
    // Uncovered timeline.
    let mut covered: Vec<(f64, f64)> = compiled.scenes.iter().map(|s| (s.start, s.end)).collect();
    covered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cursor = 0.0;
    for (s, e) in &covered {
        if *s > cursor + fps.frame() {
            issues.push(issue(Severity::Warning, "structure", None, Some(cursor), format!("no scene covers {cursor:.3}–{s:.3} s (background only)")));
        }
        cursor = cursor.max(*e);
    }
    if cursor + fps.frame() < compiled.settings.duration && !covered.is_empty() {
        issues.push(issue(Severity::Warning, "structure", None, Some(cursor), format!("no scene covers {cursor:.3}–{:.3} s", compiled.settings.duration)));
    }

    // Sync drift.
    let mut syncs = Vec::new();
    for s in &compiled.syncs {
        let achieved = achieved(compiled, s);
        let drift = (achieved - s.requested) * f;
        // A stepped layer only changes on its steps: it can't land closer
        // than one step, so that is its tolerance.
        let tolerance = match &s.kind {
            SyncKind::Move { step: Some(st), .. } if st.fps > 0.0 => options.tolerance_frames.max(f / st.fps),
            _ => options.tolerance_frames,
        };
        if drift.abs() > tolerance + 1e-6 {
            issues.push(issue(
                Severity::Error,
                "timing",
                Some(&s.element),
                Some(s.requested),
                format!(
                    "{} {:?} on '{}' lands {:+.2} f off (requested {:.3} s, achieved {:.3} s){}",
                    s.what,
                    s.anchor,
                    s.reference,
                    drift,
                    s.requested,
                    achieved,
                    match &s.kind {
                        SyncKind::Move { step: Some(st), .. } if st.fps > 0.0 => format!("; the layer steps at {} fps", st.fps),
                        SyncKind::Move { shift, .. } if *shift != 0.0 => "; pre-roll or a comp offset shifts it".into(),
                        _ => String::new(),
                    }
                ),
            ));
        }
        syncs.push(SyncResult {
            element: s.element.clone(),
            what: s.what.clone(),
            reference: s.reference.clone(),
            anchor: s.anchor,
            requested: round(s.requested),
            achieved: round(achieved),
            drift_frames: (drift * 100.0).round() / 100.0,
        });
    }

    // Frames: holds, layout, contrast.
    let pack = compiled.settings.style.as_deref().and_then(style_pack);
    let hold_limit = pack.map(|p| p.max_hold).unwrap_or(4.0);
    let frames = compiled.frame_count();
    let mut prev: Option<FrameState> = None;
    let mut hold_start = 0.0;
    let mut reported_hold = false;
    let mut safe_reported = std::collections::HashSet::new();
    let mut overlap_reported = std::collections::HashSet::new();
    let mut contrast_reported = std::collections::HashSet::new();
    let mut mesh_reported = std::collections::HashSet::new();
    let (w, h) = (compiled.settings.width as f64, compiled.settings.height as f64);
    let safe = [w * 0.05, h * 0.05, w * 0.95, h * 0.95];
    let layout_every = (f / 4.0).round().max(1.0) as u64;
    let mut craft = Craft::new(compiled, layout_every as f64 / f);
    for n in 0..frames {
        let t = fps.time_of(n as i64);
        let frame = compiled.frame(t);
        // In a transition the frame is two scene pictures moving over
        // each other: text is compared within its own scene, and nothing
        // is judged against the title-safe area mid-move.
        let crossing = frame.layers.len() > 1 && frame.layers.iter().all(|l| matches!(l.content, Content::Group(_)) && compiled.scenes.iter().any(|s| s.id == l.id));
        craft.frame(n, t, &frame, crossing);
        let same = prev.as_ref().is_some_and(|p| same_picture(p, &frame));
        if !same {
            hold_start = t;
            reported_hold = false;
        } else if !reported_hold && t - hold_start > hold_limit && !frame.layers.is_empty() {
            issues.push(issue(Severity::Warning, "timing", frame.scene.as_deref(), Some(hold_start), format!(
                "the picture holds still for over {hold_limit} s from {hold_start:.2} s{}; add drift or a move (nothing stops dead)",
                pack.map(|p| format!(" (limit of style {})", p.name)).unwrap_or_default()
            )));
            reported_hold = true;
        }
        if n % layout_every == 0 {
            let mut text_boxes: Vec<(&str, [f64; 4], usize)> = Vec::new();
            // Every layer where it is on screen: children of comps with
            // their parents' transforms and opacity.
            let flat = flatten(&frame.layers);
            let scene_of: Vec<usize> = if crossing {
                frame.layers.iter().enumerate().flat_map(|(k, l)| std::iter::repeat_n(k, flatten(std::slice::from_ref(l)).len())).collect()
            } else {
                vec![0; flat.len()]
            };
            let boxes: Vec<Option<[f64; 4]>> = flat.iter().map(|l| bounds(l, options.measure)).collect();
            craft.sample(t, &frame, &flat, &boxes, crossing);
            for (i, layer) in flat.iter().enumerate() {
                if let Content::Mesh(m) = &layer.content {
                    for message in m.params.adjustments() {
                        if mesh_reported.insert(format!("{}:{message}", layer.id)) {
                            issues.push(issue(Severity::Warning, "mesh", Some(&layer.id), Some(t), message));
                        }
                    }
                }
                let Some(b) = boxes[i] else { continue };
                let visible = layer.opacity >= 0.5 && b[2] > 0.0 && b[3] > 0.0 && b[0] < w && b[1] < h;
                if !visible {
                    continue;
                }
                let is_text = matches!(layer.content, Content::Text(_));
                let fully_on = b[0] >= 0.0 && b[1] >= 0.0 && b[2] <= w && b[3] <= h;
                if is_text && !crossing && fully_on && (b[0] < safe[0] || b[1] < safe[1] || b[2] > safe[2] || b[3] > safe[3]) && safe_reported.insert(layer.id.to_string()) {
                    issues.push(issue(Severity::Warning, "layout", Some(&layer.id), Some(t), "text sits outside the title-safe area (5% margin)".into()));
                }
                if is_text {
                    for (other, ob, scene) in &text_boxes {
                        if *scene != scene_of[i] {
                            continue;
                        }
                        let overlap = b[0] < ob[2] && ob[0] < b[2] && b[1] < ob[3] && ob[1] < b[3];
                        let key = format!("{other}+{}", layer.id);
                        if overlap && overlap_reported.insert(key) {
                            issues.push(issue(Severity::Info, "layout", Some(&layer.id), Some(t), format!("text overlaps '{other}'")));
                        }
                    }
                    text_boxes.push((&layer.id, b, scene_of[i]));
                    if let Content::Text(tx) = &layer.content {
                        // Measured against what is right behind the text:
                        // a button or card under it, else the background.
                        let centre = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                        let below = &flat[..i];
                        let behind = match backdrop(below, centre, options.measure) {
                            Hit::Color(c) => Some((c, "the shape behind it")),
                            Hit::Miss => Some((frame.background, "the background")),
                            Hit::Unknown => None,
                        };
                        if let Some((under, what)) = behind {
                            let mut ratio = contrast(tx.color.luminance(), under.luminance());
                            // An outline or a solid shadow carries the text too.
                            if let Some((sc, sw, _)) = tx.stroke {
                                if sw >= tx.size * 0.03 && sc.0[3] >= 0.5 {
                                    ratio = ratio.max(contrast(tx.color.luminance(), sc.luminance()));
                                }
                            }
                            for e in &layer.effects {
                                if let crate::compile::EffectState::Shadow { color, sigma, .. } = e {
                                    if color.0[3] >= 0.5 && *sigma <= tx.size * 0.3 {
                                        ratio = ratio.max(contrast(tx.color.luminance(), color.luminance()));
                                    }
                                }
                            }
                            if ratio < 3.0 && contrast_reported.insert(layer.id.to_string()) {
                                issues.push(issue(Severity::Warning, "layout", Some(&layer.id), Some(t), format!("text contrast against {what} is {ratio:.1}:1 (aim for at least 3:1)")));
                            }
                        }
                    }
                }
            }
        }
        prev = Some(frame);
    }

    // Narration timings: wrong word times put every reference on them off.
    if let Some(narration) = &project.narration {
        for (clip, word, message) in narration.validate() {
            let at = narration.clips.iter().find(|c| c.id == clip).map(|c| c.at);
            let what = word.map(|w| format!("'{w}' ")).unwrap_or_default();
            issues.push(issue(Severity::Error, "narration", Some(&clip), at, format!("line {clip}: {what}{message}")));
        }
    }

    // Assets and licences.
    let mut used: Vec<(String, String)> = Vec::new();
    for scene in &project.scenes {
        walk_layers(&scene.layers, &mut |l| {
            if let LayerKind::Image(img) = &l.kind {
                used.push((l.id.clone(), img.src.clone()));
            }
            if let LayerKind::Video(v) = &l.kind {
                // Sequences are checked by their first frame's folder.
                if !v.src.contains('#') {
                    used.push((l.id.clone(), v.src.clone()));
                }
            }
        });
    }
    for clip in project.audio.clips() {
        used.push(("audio".into(), clip.path.clone()));
    }
    // Font files (bundled fonts are names: "Inter SemiBold"), LUTs and
    // environment images.
    for scene in &project.scenes {
        if let Some(image) = scene.environment.as_ref().and_then(|e| e.image.clone()) {
            used.push((format!("{}.environment", scene.id), image));
        }
        walk_layers(&scene.layers, &mut |l| {
            if let LayerKind::Text(t) = &l.kind {
                if t.font.contains('.') || t.font.contains('/') {
                    used.push((l.id.clone(), t.font.clone()));
                }
            }
            for e in &l.common.effects {
                if let crate::document::Effect::Lut { path, .. } = e {
                    used.push((l.id.clone(), path.clone()));
                }
            }
            // Mesh image maps.
            if let LayerKind::Mesh(m) = &l.kind {
                if let Some(src) = m.displace.as_ref().and_then(|d| d.src.clone()) {
                    used.push((l.id.clone(), src));
                }
                let parts = m.parts.list();
                for mat in std::iter::once(&m.material).chain(parts.iter().copied().flatten()) {
                    // A simulated grid's frames are checked by their pattern's source, not here.
                    if let Some(g) = mat.volume.as_ref().and_then(|v| v.grid.as_ref()).filter(|g| !g.src.contains("{frame")) {
                        used.push((l.id.clone(), g.src.clone()));
                    }
                }
                for mat in std::iter::once(&m.material).chain(parts.into_iter().flatten()) {
                    for path in [&mat.map, &mat.normal_map, &mat.bump_map, &mat.roughness_map, &mat.metallic_map, &mat.emissive_map].into_iter().flatten() {
                        used.push((l.id.clone(), path.clone()));
                    }
                }
            }
        });
    }
    for (element, path) in used {
        if let Some(dir) = options.dir {
            if !dir.join(&path).exists() {
                issues.push(issue(Severity::Error, "structure", Some(&element), None, format!("missing file '{path}'")));
            }
        }
        match project.assets.iter().find(|a| a.path == path) {
            None => issues.push(issue(Severity::Warning, "licence", Some(&element), None, format!("'{path}' has no asset entry with source and licence"))),
            Some(a) if a.licence.trim().is_empty() || a.licence == "unknown" => {
                issues.push(issue(Severity::Warning, "licence", Some(&element), None, format!("'{path}' has no licence; export needs a person's approval")))
            }
            _ => {}
        }
    }

    tracing(project, compiled, &mut issues);
    craft.finish(project, &mut issues);

    issues.sort_by(|a, b| b.severity.cmp(&a.severity));
    let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
    let warnings = issues.iter().filter(|i| i.severity == Severity::Warning).count();
    let max_drift_frames = syncs.iter().map(|s| s.drift_frames.abs()).fold(0.0, f64::max);
    let causes = causes(&issues);
    Report { ok: errors == 0, errors, warnings, issues, syncs, max_drift_frames, causes }
}

/// A scene with at least this many short-lived traced-looking path layers
/// warns (a flipbook of polygons).
pub const TRACE_FLASH_LAYERS: usize = 12;
/// "Short-lived": on screen for at most this many frames.
pub const TRACE_FLASH_FRAMES: f64 = 2.0;
/// A path whose `d` has at least this many keys, with a median gap of at
/// most [`TRACE_DENSE_GAP`] frames, is keyed on (nearly) every frame.
pub const TRACE_DENSE_KEYS: usize = 12;
pub const TRACE_DENSE_GAP: f64 = 1.5;
/// A scene warns with this many per-frame-keyed paths, or this many
/// per-frame keys among them.
pub const TRACE_DENSE_LAYERS: usize = 3;
pub const TRACE_DENSE_TOTAL_KEYS: usize = 48;

/// Geometry traced from a reference rather than built: many SVG path layers
/// that each live ≤ 2 frames (per-frame polygons), or path layers whose `d`
/// is keyed on (nearly) every frame (rotoscoped outlines). Paths made from
/// primitives (`{"shape": …}`), paths of fewer than 3 points (spark
/// streaks), instancer templates and ordinary morphs with a few keys don't
/// count.
fn tracing(project: &Project, compiled: &Compiled, issues: &mut Vec<Issue>) {
    use crate::document::{PathData, PathValue, ShapeKind};
    let fps = compiled.fps();
    let f = fps.as_f64();
    // A path drawn from SVG data with at least 3 points.
    let traced = |v: &PathValue| match v {
        PathValue::Svg(d) => {
            let d = d.replace('-', " -");
            let numbers = d.split(|c: char| c.is_whitespace() || c == ',' || (c.is_ascii_alphabetic() && c != 'e')).filter(|s| s.parse::<f64>().is_ok()).count();
            numbers >= 6
        }
        PathValue::Shape(_) => false,
    };
    fn paths<'a>(layers: &'a [crate::document::Layer], out: &mut Vec<&'a crate::document::Layer>) {
        for l in layers {
            match &l.kind {
                LayerKind::Shape(s) if s.shape == ShapeKind::Path => out.push(l),
                LayerKind::Comp(c) => paths(&c.layers, out),
                _ => {}
            }
        }
    }
    for scene in &project.scenes {
        let mut layers = Vec::new();
        paths(&scene.layers, &mut layers);
        let mut flashes: Vec<&str> = Vec::new();
        let mut dense: Vec<(&str, usize)> = Vec::new();
        for l in layers {
            let LayerKind::Shape(s) = &l.kind else { continue };
            match &s.d {
                Some(PathData::Static(v)) if traced(v) => {
                    if compiled.element_window(&l.id).is_some_and(|(a, b)| (b - a) * f <= TRACE_FLASH_FRAMES + 1e-6) {
                        flashes.push(&l.id);
                    }
                }
                Some(PathData::Keys(keys)) if keys.iter().any(|k| traced(&k.v)) => {
                    if compiled.element_window(&l.id).is_some_and(|(a, b)| (b - a) * f <= TRACE_FLASH_FRAMES + 1e-6) {
                        flashes.push(&l.id);
                    }
                    if keys.len() < TRACE_DENSE_KEYS {
                        continue;
                    }
                    let mut times: Vec<f64> = keys.iter().filter_map(|k| compiled.resolve(&project.markers, &k.at).ok()).collect();
                    times.sort_by(f64::total_cmp);
                    let mut gaps: Vec<f64> = times.windows(2).map(|w| (w[1] - w[0]) * f).collect();
                    if gaps.len() + 1 < TRACE_DENSE_KEYS {
                        continue;
                    }
                    gaps.sort_by(f64::total_cmp);
                    if gaps[gaps.len() / 2] <= TRACE_DENSE_GAP + 1e-6 {
                        dense.push((&l.id, keys.len()));
                    }
                }
                _ => {}
            }
        }
        let examples = |ids: &mut dyn Iterator<Item = &str>| ids.take(3).map(|i| format!("'{i}'")).collect::<Vec<_>>().join(", ");
        const BUILD: &str = "build the geometry from engine shapes, 3D meshes or Blender and use measurements of the reference only for timing, positions, colours, sizes and camera";
        if flashes.len() >= TRACE_FLASH_LAYERS {
            issues.push(issue(Severity::Warning, "tracing", Some(&scene.id), None, format!(
                "{} path layers in scene '{}' are each on screen for {} frames or less (e.g. {}): this looks like per-frame polygons traced from a reference; {BUILD}",
                flashes.len(), scene.id, TRACE_FLASH_FRAMES, examples(&mut flashes.iter().copied())
            )));
        }
        let dense_keys: usize = dense.iter().map(|d| d.1).sum();
        if dense.len() >= TRACE_DENSE_LAYERS || dense_keys >= TRACE_DENSE_TOTAL_KEYS {
            issues.push(issue(Severity::Warning, "tracing", Some(&scene.id), None, format!(
                "{} path layer(s) in scene '{}' key their shape on (nearly) every frame ({} keys, e.g. {}): this looks like outlines rotoscoped from a reference; {BUILD}. A morph needs a few eased keys",
                dense.len(), scene.id, dense_keys, examples(&mut dense.iter().map(|d| d.0))
            )));
        }
    }
}

// ------------------------------------------------- declared intent, craft

/// Check kinds a layer or scene may `allow` (break on purpose).
pub const ALLOWABLE: &[&str] = &[
    "depth_order",
    "moves_backwards",
    "text_below_floor",
    "text_crowded",
    "line_unread",
    "repeats_caption",
    "persistent_chrome",
    "world_crossfade",
    "draws_off_frame",
    "layout",
];
/// Readable text is at least this share of the frame's short side tall
/// on screen (28 px at 1080).
pub const TEXT_FLOOR: f64 = 0.026;
/// More words, or more text layers, than these on screen at once for a
/// second reads like a web page.
pub const CROWD_WORDS: usize = 12;
pub const CROWD_LAYERS: usize = 5;
/// Chrome sits in this outer share of the frame, on any side.
const EDGE_BAND: f64 = 0.15;
/// Opacity from which text and chrome count as seen (a faint HUD still
/// reads); covering a subject takes 0.5.
const SEEN: f64 = 0.25;

/// What a layer declares about itself, with its comp's role and allowed
/// checks passed down.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Intent {
    pub role: Option<Role>,
    pub heading: Option<Heading>,
    pub allow: Vec<String>,
}

/// A declared facing (`facing`, or `follow` with `rotate`: the path's way).
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    /// Degrees clockwise from +x in the layer's own space.
    pub degrees: f64,
    /// The layer that declared it: a comp's facing is measured on its
    /// first layer.
    pub by: Arc<str>,
    /// The layer whose space its motion is measured in (its follow path,
    /// its parent, or a sibling that does not move), so camera and comp
    /// moves don't count as its own.
    pub world: Option<Arc<str>>,
}

impl Intent {
    pub fn allows(&self, kind: &str) -> bool {
        self.allow.iter().any(|a| a == kind)
    }
}

/// The intent of every layer that declares or inherits one (components
/// expanded first).
pub fn intents(scenes: &[Scene]) -> HashMap<Arc<str>, Intent> {
    fn still(l: &Layer) -> bool {
        let (c, t) = (&l.common, &l.common.transform);
        l.children().is_empty()
            && !c.hidden
            && c.follow.is_none()
            && c.parent.is_none()
            && ![t.position.is_animated(), t.anchor.is_animated(), t.scale.is_animated()].contains(&true)
            && ![t.rotation.is_animated(), t.z.is_animated(), t.rotation_x.is_animated(), t.rotation_y.is_animated()].contains(&true)
    }
    fn walk(layers: &[Layer], role: Option<Role>, allow: &[String], mut handed: Option<Heading>, out: &mut HashMap<Arc<str>, Intent>) {
        for l in layers {
            let c = &l.common;
            let role = c.role.or(role);
            let allow: Vec<String> = allow.iter().chain(&c.allow).cloned().collect();
            let own = c.facing.map(Facing::degrees).or_else(|| c.follow.as_ref().filter(|f| f.rotate).map(|_| 0.0)).map(|degrees| Heading {
                degrees,
                by: l.id.as_str().into(),
                world: c.follow.as_ref().map(|f| f.layer.clone()).or_else(|| c.parent.clone()).or_else(|| layers.iter().find(|s| s.id != l.id && still(s)).map(|s| s.id.clone())).map(Arc::from),
            });
            // A comp hands its facing to its first layer.
            let mine = own.or(handed.take());
            let leaf = l.children().is_empty();
            if role.is_some() || !allow.is_empty() || (leaf && mine.is_some()) {
                out.insert(l.id.as_str().into(), Intent { role, heading: if leaf { mine.clone() } else { None }, allow: allow.clone() });
            }
            if !leaf {
                walk(l.children(), role, &allow, mine, out);
            }
        }
    }
    let mut out = HashMap::new();
    for s in scenes {
        walk(&s.layers, None, &[], None, &mut out);
    }
    out
}

/// A drawn layer with its on-canvas matrix and opacity (comps opened).
struct Leaf<'a> {
    layer: &'a LayerState,
    m: Proj,
    o: f64,
    /// Index of the top-level layer it is under (a scene in a transition).
    top: usize,
}

/// The drawn layers under `layers` that `keep` wants, in paint order,
/// without copying them.
fn leaves<'a>(layers: &'a [LayerState], parent: Option<(Proj, f64, usize)>, keep: &dyn Fn(&LayerState) -> bool, out: &mut Vec<Leaf<'a>>) {
    for (k, l) in layers.iter().enumerate() {
        let (m, o, top) = match parent {
            Some((pm, po, top)) => (pm.then(l.matrix), po * l.opacity, top),
            None => (l.matrix, l.opacity, k),
        };
        match &l.content {
            Content::Group(children) => leaves(children, Some((m, o, top)), keep, out),
            _ if keep(l) => out.push(Leaf { layer: l, m, o, top }),
            _ => {}
        }
    }
}

/// Text height on screen: one em (`size`) through the layer's matrix.
fn em_px(m: Proj, size: f64) -> f64 {
    let (a, b) = (m.apply(0.0, 0.0), m.apply(0.0, size));
    (b.0 - a.0).hypot(b.1 - a.1)
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

/// Seconds a line of `words` needs on screen to be read.
pub fn reading_time(words: usize) -> f64 {
    if words <= 2 {
        0.4
    } else {
        0.75 * (0.5 + 0.3 * words as f64)
    }
}

/// Lower-case words, punctuation dropped ("Earth." = "EARTH").
fn norm_words(text: &str) -> Vec<String> {
    let s: String = text.chars().filter(|c| !matches!(c, '\'' | '’')).flat_map(|c| if c.is_alphanumeric() { c.to_lowercase().collect::<Vec<_>>() } else { vec![' '] }).collect();
    s.split_whitespace().map(String::from).collect()
}

/// The longest run of consecutive words `a` and `b` share.
fn common_run<'w>(a: &'w [String], b: &[String]) -> &'w [String] {
    let mut best = (0, 0);
    let mut prev = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        let mut row = vec![0usize; b.len() + 1];
        for j in 1..=b.len() {
            if a[i - 1] == b[j - 1] {
                row[j] = prev[j - 1] + 1;
                if row[j] > best.0 {
                    best = (row[j], i);
                }
            }
        }
        prev = row;
    }
    &a[best.1 - best.0..best.1]
}

fn dir_word(v: (f64, f64)) -> &'static str {
    match (v.0.abs() >= v.1.abs(), v.0 >= 0.0, v.1 >= 0.0) {
        (true, true, _) => "right",
        (true, false, _) => "left",
        (false, _, true) => "down",
        (false, _, false) => "up",
    }
}

/// `items`, at most `n` of them, then how many more.
fn listed(items: &[String], n: usize) -> String {
    let mut s = items.iter().take(n).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > n {
        s.push_str(&format!(" and {} more", items.len() - n));
    }
    s
}

/// Items (scene, text, time) grouped by scene in first-seen order, each
/// group with its earliest time.
fn per_scene(items: Vec<(Arc<str>, String, f64)>) -> Vec<(Arc<str>, Vec<String>, f64)> {
    let mut out: Vec<(Arc<str>, Vec<String>, f64)> = Vec::new();
    for (scene, item, at) in items {
        match out.iter_mut().find(|g| g.0 == scene) {
            Some(g) => {
                g.1.push(item);
                g.2 = g.2.min(at);
            }
            None => out.push((scene, vec![item], at)),
        }
    }
    out
}

/// One stretch a text layer stays on screen.
struct Run {
    start: u64,
    last: u64,
    scene: Arc<str>,
    origin: (f64, f64),
    text: Arc<str>,
    words: usize,
}

/// A declared facing followed frame to frame.
#[derive(Default)]
struct Track {
    /// Frame, position and its world's matrix.
    prev: Option<(u64, (f64, f64), Option<Proj>)>,
    /// When it started moving against its facing, and last did.
    from: Option<f64>,
    last: f64,
    done: bool,
}

/// Something at one place on the frame's edge, sample after sample.
struct Chrome {
    origin: (f64, f64),
    size: f64,
    label: String,
    ids: Vec<Arc<str>>,
    scenes: Vec<Arc<str>>,
    samples: usize,
    last: usize,
}

/// A stretch with too much text on screen, and its busiest moment.
struct Crowd {
    scene: Arc<str>,
    from: f64,
    samples: usize,
    last: usize,
    words: usize,
    ids: Vec<Arc<str>>,
}

/// Declared-intent and measured-craft checks, fed every frame (`frame`)
/// and every layout sample (`sample`) of `check`'s one pass over the film.
struct Craft<'a> {
    compiled: &'a Compiled,
    w: f64,
    h: f64,
    short: f64,
    fps: f64,
    /// Seconds between layout samples.
    every: f64,
    samples: usize,
    out: Vec<Issue>,
    runs: HashMap<Arc<str>, Run>,
    /// Runs that ended, waiting a few frames to see what replaces them.
    left: Vec<(Arc<str>, Run)>,
    /// Text runs that started on the last two frames: (frame, origin, text).
    starts: Vec<(u64, (f64, f64), Arc<str>)>,
    unread: Vec<(Arc<str>, String, f64)>,
    words: HashMap<Arc<str>, Vec<String>>,
    pairs: HashSet<(Arc<str>, Arc<str>)>,
    repeats: Vec<(Arc<str>, String, f64)>,
    tracks: HashMap<Arc<str>, Track>,
    /// Layers some facing is measured in, and whether any layer has one.
    worlds: HashSet<Arc<str>>,
    headed: bool,
    /// Text under the floor: id → (scene, samples, sum of its sizes then,
    /// first seen).
    small: HashMap<Arc<str>, (Arc<str>, usize, f64, f64)>,
    crowd: Option<Crowd>,
    crowded: Vec<Crowd>,
    chrome: Vec<Chrome>,
    /// Chrome by a coarse grid cell of its origin.
    chrome_cells: HashMap<(bool, i64, i64), Vec<usize>>,
    /// (misplaced, subject) → (consecutive samples, last sample); 3 once
    /// reported.
    depth: HashMap<(Arc<str>, Arc<str>), (usize, usize)>,
    path_boxes: HashMap<(usize, usize), Option<[f64; 4]>>,
}

impl<'a> Craft<'a> {
    fn new(compiled: &'a Compiled, every: f64) -> Self {
        let (w, h) = (compiled.settings.width as f64, compiled.settings.height as f64);
        Craft {
            compiled,
            w,
            h,
            short: w.min(h),
            fps: compiled.fps().as_f64(),
            every,
            samples: 0,
            out: Vec::new(),
            runs: HashMap::new(),
            left: Vec::new(),
            starts: Vec::new(),
            unread: Vec::new(),
            words: HashMap::new(),
            pairs: HashSet::new(),
            repeats: Vec::new(),
            tracks: HashMap::new(),
            worlds: compiled.intents.values().filter_map(|i| i.heading.as_ref()?.world.clone()).collect(),
            headed: compiled.intents.values().any(|i| i.heading.is_some()),
            small: HashMap::new(),
            crowd: None,
            crowded: Vec::new(),
            chrome: Vec::new(),
            chrome_cells: HashMap::new(),
            depth: HashMap::new(),
            path_boxes: HashMap::new(),
        }
    }

    fn on_canvas(&self, p: (f64, f64)) -> bool {
        p.0 >= 0.0 && p.0 <= self.w && p.1 >= 0.0 && p.1 <= self.h
    }

    /// Every frame: text runs (`line_unread`), text said with the caption
    /// (`repeats_caption`) and declared facings (`moves_backwards`).
    fn frame(&mut self, n: u64, t: f64, frame: &FrameState, crossing: bool) {
        let compiled = self.compiled;
        let intents = &compiled.intents;
        let mut found = Vec::new();
        let (worlds, headed) = (&self.worlds, self.headed);
        leaves(&frame.layers, None, &|l| matches!(l.content, Content::Text(_)) || headed && (worlds.contains(&l.id) || intents.get(&l.id).is_some_and(|i| i.heading.is_some())), &mut found);
        // Where each layer that a facing is measured in is this frame.
        let mut world_m: Vec<(&Arc<str>, Proj)> = Vec::new();
        for leaf in found.iter().filter(|l| worlds.contains(&l.layer.id)) {
            if !world_m.iter().any(|w| *w.0 == leaf.layer.id) {
                world_m.push((&leaf.layer.id, leaf.m));
            }
        }
        let scene = |top: usize| -> Arc<str> {
            if crossing {
                frame.layers[top].id.clone()
            } else {
                frame.scene.clone().unwrap_or_else(|| "".into())
            }
        };
        let mut tracked: Vec<&Arc<str>> = Vec::new();
        let mut captions: Vec<(&Arc<str>, &str)> = Vec::new();
        let mut texts: Vec<(&Arc<str>, &str, Arc<str>)> = Vec::new();
        for leaf in &found {
            let id = &leaf.layer.id;
            let intent = intents.get(id);
            if let Some(h) = intent.and_then(|i| i.heading.as_ref()) {
                if leaf.o >= 0.5 && !tracked.contains(&&h.by) && !intent.is_some_and(|i| i.allows("moves_backwards")) {
                    tracked.push(&h.by);
                    let world = match &h.world {
                        None => Some(None),
                        Some(w) => world_m.iter().find(|m| m.0 == w).map(|m| Some(m.1)),
                    };
                    self.track(h, n, t, leaf.m, world);
                }
            }
            let Content::Text(tx) = &leaf.layer.content else { continue };
            if leaf.o < SEEN || tx.text.trim().is_empty() {
                continue;
            }
            // A line comes and goes by its own time and opacity, not as a
            // camera move frames it; read together means on screen together.
            let origin = leaf.m.apply(0.0, 0.0);
            let on = self.on_canvas(origin);
            match intent.and_then(|i| i.role) {
                Some(Role::Caption) if on => captions.push((id, tx.text.as_ref())),
                Some(Role::Caption | Role::Decoration) => {}
                _ => {
                    if on {
                        texts.push((id, tx.text.as_ref(), scene(leaf.top)));
                    }
                    if !intent.is_some_and(|i| i.allows("line_unread")) {
                        // The same words already on screen (the last scene's
                        // copy) keep their time.
                        let carried = (!self.runs.contains_key(id)).then(|| self.runs.values().chain(self.left.iter().map(|l| &l.1)).filter(|r| r.text == tx.text).map(|r| r.start).min()).flatten();
                        let run = self.runs.entry(id.clone()).or_insert_with(|| {
                            self.starts.push((n, origin, tx.text.clone()));
                            Run { start: carried.unwrap_or(n), last: n, scene: scene(leaf.top), origin, text: tx.text.clone(), words: word_count(&tx.text) }
                        });
                        run.last = n;
                        run.origin = origin;
                    }
                }
            }
        }
        // Lines that left, judged a few frames later, once whatever takes
        // their place has had time to appear.
        let mut left: Vec<Arc<str>> = self.runs.iter().filter(|(_, r)| r.last < n).map(|(id, _)| id.clone()).collect();
        left.sort();
        for id in left {
            let run = self.runs.remove(&id).unwrap();
            self.left.push((id, run));
        }
        let (due, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.left).into_iter().partition(|(_, r)| r.last + 3 <= n);
        self.left = wait;
        for (id, run) in due {
            self.end_run(&id, run, false);
        }
        self.starts.retain(|s| s.0 + 6 >= n);
        // Text that says what the caption says.
        for (cid, ctext) in &captions {
            for (xid, xtext, xscene) in &texts {
                if !self.pairs.insert(((*cid).clone(), (*xid).clone())) {
                    continue;
                }
                if intents.get(*xid).is_some_and(|i| i.allows("repeats_caption")) {
                    continue;
                }
                let cw = self.words.entry((*cid).clone()).or_insert_with(|| norm_words(ctext)).clone();
                let xw = self.words.entry((*xid).clone()).or_insert_with(|| norm_words(xtext));
                let shared = common_run(xw, &cw);
                if shared.len() >= 3 {
                    self.repeats.push((xscene.clone(), format!("'{xid}' says \"{}\" with the caption '{cid}'", shared.join(" ")), t));
                }
            }
        }
    }

    /// A line left the screen: was it there long enough to read? A number
    /// replaced at the same place (a count, a clock), or the same words
    /// carried on by the next scene, is not a line that left.
    fn end_run(&mut self, id: &Arc<str>, run: Run, last_line: bool) {
        let near = 0.03 * self.short;
        let (digits, all) = run.text.chars().filter(|c| c.is_alphanumeric()).fold((0, 0), |(d, a), c| (d + c.is_ascii_digit() as usize, a + 1));
        let number = all > 0 && digits * 2 >= all;
        let replaced = self.starts.iter().any(|(f, p, text)| *f + 1 >= run.last && *f <= run.last + 3 && (*text == run.text || number && (p.0 - run.origin.0).abs() <= near && (p.1 - run.origin.1).abs() <= near));
        if !last_line && (replaced || self.runs.values().any(|r| r.text == run.text)) {
            return;
        }
        let shown = (run.last - run.start + 1) as f64 / self.fps;
        let mut need = reading_time(run.words);
        if last_line {
            need = need.max(2.0);
        }
        if shown + 1e-6 < need {
            let words = if run.words == 1 { "1 word".to_string() } else { format!("{} words", run.words) };
            self.unread.push((run.scene, format!("'{id}' ({words}, {shown:.2} s of {need:.2} s)"), run.start as f64 / self.fps));
        }
    }

    /// Follows a declared facing: motion against it (dot < −0.3) faster
    /// than 3% of the short side per second for 0.3 s is an error. Motion
    /// is measured in the layer's world (`world`: that layer's matrix this
    /// frame; `Some(None)` measures on the canvas; `None`: not drawn, so
    /// not measured), so a camera or comp carrying it doesn't count.
    fn track(&mut self, h: &Heading, n: u64, t: f64, m: Proj, world: Option<Option<Proj>>) {
        let by = &h.by;
        let o = m.apply(0.0, 0.0);
        let (s, c) = h.degrees.to_radians().sin_cos();
        let q = m.apply(c * 10.0, s * 10.0);
        let face = (q.0 - o.0, q.1 - o.1);
        let (fps, slow) = (self.fps, 0.03 * self.short);
        let tr = self.tracks.entry(by.clone()).or_default();
        let Some(world) = world.filter(|_| o.0.is_finite() && o.1.is_finite()) else {
            tr.prev = None;
            return;
        };
        if let Some((pn, p, pw)) = tr.prev {
            // Where the last position sits in this frame's world.
            let p = match (world, pw.and_then(|pw| pw.inverse())) {
                (Some(w), Some(inv)) => {
                    let l = inv.apply(p.0, p.1);
                    w.apply(l.0, l.1)
                }
                _ => p,
            };
            if pn + 1 == n && !tr.done && p.0.is_finite() && p.1.is_finite() {
                let v = ((o.0 - p.0) * fps, (o.1 - p.1) * fps);
                let (speed, fl) = (v.0.hypot(v.1), face.0.hypot(face.1));
                if speed > slow && fl > 1e-9 {
                    if (v.0 * face.0 + v.1 * face.1) / (speed * fl) < -0.3 {
                        let from = *tr.from.get_or_insert(t);
                        tr.last = t;
                        if t - from >= 0.3 - 1e-6 {
                            tr.done = true;
                            self.out.push(issue(Severity::Error, "moves_backwards", Some(by), Some(from), format!(
                                "'{by}' faces {} on screen but moves {} from {from:.2} s: it travels backwards; turn or flip it (scale x −1) so it faces the way it goes, or set `facing` to the way its art points",
                                dir_word(face), dir_word(v)
                            )));
                        }
                    } else {
                        tr.from = None;
                    }
                } else if tr.from.is_some() && t - tr.last > 0.2 {
                    tr.from = None;
                }
            }
        }
        tr.prev = Some((n, o, world));
    }

    /// A path's screen box (its full outline, stroke included).
    fn path_box(&mut self, l: &LayerState) -> Option<[f64; 4]> {
        let Content::Shape(s) = &l.content else { return None };
        let p = s.path.as_ref()?;
        let mut b = self.local_box(&p.from)?;
        if let Some(to) = &p.to {
            if let Some(o) = self.local_box(to) {
                b = [b[0].min(o[0]), b[1].min(o[1]), b[2].max(o[2]), b[3].max(o[3])];
            }
        }
        let pad = s.stroke.map(|(_, w)| w / 2.0).unwrap_or(0.0);
        let corners = [(b[0] - pad, b[1] - pad), (b[2] + pad, b[1] - pad), (b[2] + pad, b[3] + pad), (b[0] - pad, b[3] + pad)].map(|(x, y)| l.matrix.apply(x, y));
        Some([
            corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min),
            corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min),
            corners.iter().map(|c| c.0).fold(f64::NEG_INFINITY, f64::max),
            corners.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max),
        ])
    }

    fn local_box(&mut self, d: &Arc<str>) -> Option<[f64; 4]> {
        let key = (Arc::as_ptr(d) as *const u8 as usize, d.len());
        *self.path_boxes.entry(key).or_insert_with(|| {
            let polys = crate::path::flatten(d, 1.0).ok()?;
            let pts = polys.iter().flat_map(|p| p.0.iter());
            let b = pts.fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]);
            (b[0] <= b[2]).then_some(b)
        })
    }

    /// Every layout sample: small text, crowded screens, chrome and the
    /// declared depth order. Transitions are skipped (two pictures moving).
    fn sample(&mut self, t: f64, frame: &FrameState, flat: &[LayerState], boxes: &[Option<[f64; 4]>], crossing: bool) {
        self.samples += 1;
        if crossing {
            self.end_crowd();
            return;
        }
        let seen = |l: &LayerState, role: Option<Role>| l.opacity >= if matches!(role, Some(Role::Background | Role::Subject | Role::Foreground)) { 0.5 } else { SEEN };
        let compiled = self.compiled;
        let intents = &compiled.intents;
        let scene: Arc<str> = frame.scene.clone().unwrap_or_else(|| "".into());
        let floor = TEXT_FLOOR * self.short;
        let depth = flat.iter().any(|l| l.opacity >= 0.5 && intents.get(&l.id).is_some_and(|i| i.role == Some(Role::Subject)));
        let (mut words, mut texts) = (0usize, Vec::new());
        let mut roles: Vec<(usize, Role, [f64; 4])> = Vec::new();
        for (i, l) in flat.iter().enumerate() {
            let intent = intents.get(&l.id);
            let role = intent.and_then(|i| i.role);
            if !seen(l, role) {
                continue;
            }
            let allows = |k: &str| intent.is_some_and(|i| i.allows(k));
            let positioned = matches!(role, Some(Role::Background | Role::Subject | Role::Foreground)) && depth && !allows("depth_order");
            let b = match (&l.content, boxes[i]) {
                (_, Some(b)) => Some(b),
                (Content::Text(_), None) => {
                    let (x, y) = l.matrix.apply(0.0, 0.0);
                    Some([x, y, x, y])
                }
                (Content::Shape(s), None) if s.shape == ShapeKind::Path && positioned => self.path_box(l),
                _ => None,
            };
            let Some(b) = b else { continue };
            if !b.iter().all(|v| v.is_finite()) || b[0] > self.w || b[2] < 0.0 || b[1] > self.h || b[3] < 0.0 {
                continue;
            }
            if let Content::Text(tx) = &l.content {
                if role != Some(Role::Decoration) {
                    let em = em_px(l.matrix, tx.size);
                    // (Under a pixel it is not drawn at all: a scale-in.)
                    if (1.0..floor).contains(&em) && !allows("text_below_floor") {
                        let e = self.small.entry(l.id.clone()).or_insert((scene.clone(), 0, 0.0, t));
                        e.1 += 1;
                        e.2 += em;
                    }
                    if role != Some(Role::Caption) && !allows("text_crowded") {
                        words += word_count(&tx.text);
                        texts.push(l.id.clone());
                    }
                }
            }
            if role != Some(Role::Caption) && !allows("persistent_chrome") {
                self.chrome(l, b, &scene);
            }
            if positioned {
                roles.push((i, role.unwrap(), b));
            }
        }
        // Too much to read at once.
        if texts.len() > CROWD_LAYERS || words > CROWD_WORDS {
            let samples = self.samples;
            match &mut self.crowd {
                Some(c) if c.scene == scene && c.last + 1 == samples => {
                    c.samples += 1;
                    c.last = samples;
                    if words > c.words {
                        (c.words, c.ids) = (words, texts);
                    }
                }
                _ => {
                    self.end_crowd();
                    self.crowd = Some(Crowd { scene: scene.clone(), from: t, samples: 1, last: samples, words, ids: texts });
                }
            }
        } else {
            self.end_crowd();
        }
        // A background in front of a subject, or a foreground behind one.
        let depth_of = |l: &LayerState| l.view.map(|v| v.apply(0.0, 0.0, 0.0)[2]);
        let front = |a: usize, b: usize| match (depth_of(&flat[a]), depth_of(&flat[b])) {
            (Some(da), Some(db)) => da < db,
            _ => a > b,
        };
        let area = |b: [f64; 4]| (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0);
        for s in roles.iter().filter(|r| r.1 == Role::Subject) {
            for o in roles.iter().filter(|r| r.1 != Role::Subject) {
                let ahead = front(o.0, s.0);
                let behind = match o.1 {
                    Role::Background if ahead => s,
                    Role::Foreground if !ahead => o,
                    _ => continue,
                };
                let inter = [s.2[0].max(o.2[0]), s.2[1].max(o.2[1]), s.2[2].min(o.2[2]), s.2[3].min(o.2[3])];
                let cover = area(inter) / area(behind.2).max(1e-9);
                if cover < 0.2 {
                    continue;
                }
                let (lo, so) = (&flat[o.0].id, &flat[s.0].id);
                let samples = self.samples;
                let e = self.depth.entry((lo.clone(), so.clone())).or_insert((0, 0));
                if e.0 > 2 {
                    continue;
                }
                e.0 = if e.1 + 1 == samples { e.0 + 1 } else { 1 };
                e.1 = samples;
                if e.0 < 2 {
                    continue;
                }
                e.0 = 3;
                let at = t - self.every;
                let (what, fix) = if o.1 == Role::Background {
                    (format!("'{lo}' (background) is drawn in front of '{so}' (subject) at {at:.2} s, covering {:.0}% of it", cover * 100.0), "a background belongs behind its subject: move it below the subject in the layer list (farther in z in 3D) or give it another role")
                } else {
                    (format!("'{lo}' (foreground) is drawn behind '{so}' (subject) at {at:.2} s, {:.0}% of it hidden", cover * 100.0), "a foreground belongs in front of its subject: move it above the subject in the layer list (nearer in z in 3D) or give it another role")
                };
                self.out.push(issue(Severity::Error, "depth_order", Some(lo), Some(at), format!("{what}: {fix}")));
            }
        }
    }

    fn end_crowd(&mut self) {
        let Some(c) = self.crowd.take() else { return };
        if c.samples as f64 * self.every < 1.0 - 1e-6 {
            return;
        }
        match self.crowded.iter_mut().find(|k| k.scene == c.scene) {
            Some(k) if c.words > k.words => *k = c,
            Some(_) => {}
            None => self.crowded.push(c),
        }
    }

    /// Text or a small shape at the frame's edge, grouped with what was at
    /// the same place (and size) before.
    fn chrome(&mut self, l: &LayerState, b: [f64; 4], scene: &Arc<str>) {
        let (w, h) = (self.w, self.h);
        if !(b[3] <= h * EDGE_BAND || b[1] >= h * (1.0 - EDGE_BAND) || b[2] <= w * EDGE_BAND || b[0] >= w * (1.0 - EDGE_BAND)) {
            return;
        }
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        let (text, size, label) = match &l.content {
            Content::Text(tx) => (true, em_px(l.matrix, tx.size), format!("'{}'", tx.text.replace('\n', " "))),
            Content::Shape(_) | Content::Image { .. } if bw * bh <= 0.05 * w * h => {
                let what = match &l.content {
                    Content::Shape(s) => format!("{:?}", s.shape).to_lowercase(),
                    _ => "image".into(),
                };
                (false, bw.max(bh), format!("a {bw:.0}×{bh:.0} px {what}"))
            }
            _ => return,
        };
        let origin = l.matrix.apply(0.0, 0.0);
        let tol = 0.01 * self.short;
        let cell = |v: f64| (v / tol).floor() as i64;
        let (cx, cy) = (cell(origin.0), cell(origin.1));
        let mut hit = None;
        'find: for dx in -1..=1 {
            for dy in -1..=1 {
                for &g in self.chrome_cells.get(&(text, cx + dx, cy + dy)).map(Vec::as_slice).unwrap_or(&[]) {
                    let c = &self.chrome[g];
                    if (c.origin.0 - origin.0).abs() <= tol && (c.origin.1 - origin.1).abs() <= tol && (c.size - size).abs() <= 0.1 * c.size.max(size) + 1.0 {
                        hit = Some(g);
                        break 'find;
                    }
                }
            }
        }
        let g = hit.unwrap_or_else(|| {
            self.chrome.push(Chrome { origin, size, label, ids: Vec::new(), scenes: Vec::new(), samples: 0, last: 0 });
            self.chrome_cells.entry((text, cx, cy)).or_default().push(self.chrome.len() - 1);
            self.chrome.len() - 1
        });
        let samples = self.samples;
        let c = &mut self.chrome[g];
        if c.last != samples {
            c.samples += 1;
            c.last = samples;
        }
        if !c.scenes.contains(scene) {
            c.scenes.push(scene.clone());
        }
        if !c.ids.contains(&l.id) && c.ids.len() < 32 {
            c.ids.push(l.id.clone());
        }
    }

    /// Reports everything, one finding per cause and scene.
    fn finish(mut self, project: &Project, issues: &mut Vec<Issue>) {
        let compiled = self.compiled;
        let allows = |id: &str, kind: &str| compiled.intents.get(id).is_some_and(|i| i.allows(kind));
        // The film's last lines need two seconds.
        for (id, run) in std::mem::take(&mut self.left) {
            self.end_run(&id, run, false);
        }
        let mut open: Vec<(Arc<str>, Run)> = self.runs.drain().collect();
        open.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, run) in open {
            self.end_run(&id, run, true);
        }
        self.end_crowd();
        issues.append(&mut self.out);
        let floor = TEXT_FLOOR * self.short;
        let mut small: Vec<_> = self.small.iter().filter(|(_, s)| s.1 as f64 * self.every >= 0.5 - 1e-6).collect();
        small.sort_by(|a, b| a.1 .3.total_cmp(&b.1 .3).then(a.0.cmp(b.0)));
        let small = per_scene(small.into_iter().map(|(id, s)| (s.0.clone(), format!("'{id}' ({:.1} px)", s.2 / s.1 as f64), s.3)).collect());
        for (scene, items, at) in small {
            issues.push(issue(Severity::Warning, "text_below_floor", Some(&scene), Some(at), format!(
                "{} text layer(s) in scene '{scene}' stay under {floor:.0} px tall on screen ({:.1}% of the frame's short side) for 0.5 s or more: {}; make them bigger, say it in the voice-over instead, or mark deliberate small print `role: decoration`",
                items.len(), TEXT_FLOOR * 100.0, listed(&items, 6)
            )));
        }
        for c in &self.crowded {
            let ids: Vec<String> = c.ids.iter().map(|i| format!("'{i}'")).collect();
            issues.push(issue(Severity::Warning, "text_crowded", Some(&c.scene), Some(c.from), format!(
                "scene '{}' shows {} text layers and {} words at once for {:.1} s from {:.2} s ({}): a screen that reads like a web page; keep one short idea on screen and let the voice carry the rest",
                c.scene, c.ids.len(), c.words, c.samples as f64 * self.every, c.from, listed(&ids, 8)
            )));
        }
        for (scene, items, at) in per_scene(std::mem::take(&mut self.unread)) {
            issues.push(issue(Severity::Warning, "line_unread", Some(&scene), Some(at), format!(
                "{} line(s) in scene '{scene}' leave before they can be read: {}; hold a line 0.75 × (0.5 s + 0.3 s per word), 0.4 s for one or two words and 2 s for the film's last line, or cut it",
                items.len(), listed(&items, 6)
            )));
        }
        for (scene, items, at) in per_scene(std::mem::take(&mut self.repeats)) {
            issues.push(issue(Severity::Warning, "repeats_caption", Some(&scene), Some(at), format!(
                "on-screen text in scene '{scene}' repeats the caption shown with it: {}; the viewer reads the same words twice; show what the words can't (a number, a place, a face) or drop one",
                listed(&items, 4)
            )));
        }
        // Chrome: the same thing at the same place on the edge across scenes.
        let duration = compiled.settings.duration;
        let chrome: Vec<&Chrome> = self.chrome.iter().filter(|c| c.scenes.len() >= 3 || c.samples as f64 * self.every >= 0.6 * duration).collect();
        if !chrome.is_empty() {
            let scenes: HashSet<&Arc<str>> = chrome.iter().flat_map(|c| &c.scenes).collect();
            // Alike things once, counted (the ticks of a timeline).
            let mut alike: Vec<(&str, &str, usize)> = Vec::new();
            for c in &chrome {
                match alike.iter_mut().find(|a| a.0 == c.label) {
                    Some(a) => a.2 += 1,
                    None => alike.push((&c.label, c.ids.first().map(|i| &**i).unwrap_or(""), 1)),
                }
            }
            let labels: Vec<String> = alike.iter().map(|(label, id, n)| if *n > 1 { format!("{n} × {label} ('{id}', …)") } else { format!("{label} ('{id}')") }).collect();
            let first = chrome[0].ids.first().cloned();
            issues.push(issue(Severity::Warning, "persistent_chrome", first.as_deref(), None, format!(
                "{} thing(s) stay at the same place on the frame's edge in {} scenes, like a web page's header and footer: {}; a film has no chrome (headers, frame counters, progress bars): take them out, or `allow` persistent_chrome on a deliberate watermark",
                chrome.len(), scenes.len(), listed(&labels, 8)
            )));
        }
        world_crossfade(project, compiled, issues);
        draws_off_frame(compiled, issues);
        // Allowed by declaration, and allows that name no check.
        for scene in &project.scenes {
            let mut bad: Vec<(String, String)> = scene.allow.iter().filter(|a| !ALLOWABLE.contains(&a.as_str())).map(|a| (scene.id.clone(), a.clone())).collect();
            walk_layers(&scene.layers, &mut |l| bad.extend(l.common.allow.iter().filter(|a| !ALLOWABLE.contains(&a.as_str())).map(|a| (l.id.clone(), a.clone()))));
            for (id, a) in bad {
                issues.push(issue(Severity::Warning, "structure", Some(&id), None, format!("'{id}' allows '{a}', which is not a check kind ({})", ALLOWABLE.join(", "))));
            }
        }
        issues.retain(|i| !(i.kind == "layout" && i.element.as_deref().is_some_and(|e| allows(e, "layout"))));
    }
}

/// Scenes that dissolve into a different scene: worlds never crossfade.
/// A fade from or to an empty scene (a colour) is fine.
fn world_crossfade(project: &Project, compiled: &Compiled, issues: &mut Vec<Issue>) {
    for (j, scene) in project.scenes.iter().enumerate().skip(1) {
        if !scene.transition.as_ref().is_some_and(|t| t.kind == TransitionKind::Fade) || scene.allow.iter().any(|a| a == "world_crossfade") {
            continue;
        }
        let Some(cut) = compiled.scenes.get(j).map(|s| s.start) else { continue };
        let frame = compiled.frame(cut);
        // Two scene pictures, each with more than its background.
        if frame.layers.len() != 2 || !frame.layers.iter().all(|l| matches!(&l.content, Content::Group(c) if c.len() > 1)) {
            continue;
        }
        let from = &project.scenes[j - 1].id;
        issues.push(issue(Severity::Warning, "world_crossfade", Some(&scene.id), Some(cut), format!(
            "scene '{}' dissolves in over '{from}' (a fade): two different worlds never crossfade; cut on a cause (a hit, a word), carry one element across, flood the frame with a colour or whip; if both are one place, `allow` world_crossfade on the scene",
            scene.id
        )));
    }
}

/// Strokes drawn on with `trim` whose head lands off the frame while their
/// start is on it: a line to a place the viewer cannot see. Measured when
/// each trim move ends, or on the stroke's last frame if it goes first,
/// in its own scene's picture (not mid-transition).
fn draws_off_frame(compiled: &Compiled, issues: &mut Vec<Issue>) {
    let fps = compiled.fps();
    let f = fps.as_f64();
    let (w, h) = (compiled.settings.width as f64, compiled.settings.height as f64);
    let margin = 0.01 * w.min(h);
    let last = compiled.frame_count() as i64 - 1;
    let mut done: HashSet<&str> = HashSet::new();
    let mut found = Vec::new();
    for e in compiled.events.iter().filter(|e| e.what == "trim") {
        if done.contains(e.element.as_str()) || compiled.intents.get(e.element.as_str()).is_some_and(|i| i.allows("draws_off_frame")) {
            continue;
        }
        let k = ((e.t * f - 1e-6).ceil() as i64).clamp(0, last.max(0));
        for j in (k - f.ceil() as i64..=k).rev().filter(|j| *j >= 0) {
            let t = fps.time_of(j);
            let frame = compiled.frame(t);
            if frame.layers.len() > 1 && frame.layers.iter().all(|l| matches!(l.content, Content::Group(_)) && compiled.scenes.iter().any(|s| s.id == l.id)) {
                continue;
            }
            let mut hit = Vec::new();
            leaves(&frame.layers, None, &|l| *l.id == *e.element, &mut hit);
            let Some(leaf) = hit.first() else { continue };
            let Content::Shape(s) = &leaf.layer.content else { break };
            let (a, b) = (s.trim.x(), s.trim.y());
            let Some(path) = s.path.as_ref().filter(|_| b - a > 0.02 && leaf.o >= 0.5) else { break };
            // Outlines (closed shapes) are traced, not sent somewhere.
            let Ok(polys) = crate::path::morph(path, crate::path::tolerance(leaf.m.max_scale())) else { break };
            if polys.iter().all(|p| p.1) {
                break;
            }
            let (Some((head, _)), Some((tail, _))) = (crate::path::point_at(&polys, b), crate::path::point_at(&polys, a)) else { break };
            let (hx, hy) = leaf.m.apply(head.0, head.1);
            let (tx, ty) = leaf.m.apply(tail.0, tail.1);
            let off = !(hx >= -margin && hx <= w + margin && hy >= -margin && hy <= h + margin);
            let tail_on = tx >= 0.0 && tx <= w && ty >= 0.0 && ty <= h;
            if off && tail_on {
                done.insert(e.element.as_str());
                let scene: Arc<str> = frame.scene.clone().unwrap_or_else(|| "".into());
                found.push((scene, format!("'{}' (head at {hx:.0}, {hy:.0} px at {t:.2} s)", e.element), t));
            }
            break;
        }
    }
    for (scene, items, at) in per_scene(found) {
        issues.push(issue(Severity::Warning, "draws_off_frame", Some(&scene), Some(at), format!(
            "{} stroke(s) in scene '{scene}' finish drawing with their head off the frame while they start on it: {}; the line goes somewhere the viewer cannot see: frame the destination (move the camera or the end point) before the draw lands",
            items.len(), listed(&items, 6)
        )));
    }
}

fn round(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

fn issue(severity: Severity, kind: &'static str, element: Option<&str>, at: Option<f64>, message: String) -> Issue {
    Issue { severity, kind, element: element.map(String::from), at: at.map(round), message }
}

fn contrast(a: f64, b: f64) -> f64 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// Two frames show the same picture (time aside).
fn same_picture(a: &FrameState, b: &FrameState) -> bool {
    a.scene == b.scene && a.background == b.background && a.layers == b.layers
}

/// Canvas bounds of shapes and images from their geometry; text through
/// the measure callback.
enum Hit {
    /// Nothing there: the background shows.
    Miss,
    /// A solid shape.
    Color(crate::value::Color),
    /// Footage, an image, a gradient… no single colour to measure against.
    Unknown,
}

/// What is painted at point `p` by the topmost of `layers` covering it.
fn backdrop(layers: &[LayerState], p: (f64, f64), measure: Option<Measure>) -> Hit {
    for layer in layers.iter().rev() {
        if layer.opacity < 0.5 {
            continue;
        }
        if let Content::Group(children) = &layer.content {
            let kids: Vec<LayerState> = children
                .iter()
                .map(|c| {
                    let mut c = c.clone();
                    c.matrix = layer.matrix.then(c.matrix);
                    c.opacity *= layer.opacity;
                    c
                })
                .collect();
            match backdrop(&kids, p, measure) {
                Hit::Miss => continue,
                hit => return hit,
            }
        }
        let Some(b) = bounds(layer, measure) else { continue };
        if !(b[0] <= p.0 && p.0 <= b[2] && b[1] <= p.1 && p.1 <= b[3]) {
            continue;
        }
        return match &layer.content {
            Content::Shape(s) => match &s.fill {
                Some(crate::compile::PaintState::Solid(c)) if c.0[3] >= 0.5 => Hit::Color(*c),
                Some(crate::compile::PaintState::Solid(_)) | None => continue,
                Some(_) => Hit::Unknown,
            },
            // Text behind text, footage, images, meshes: unknown colour.
            _ => Hit::Unknown,
        };
    }
    Hit::Miss
}

pub fn bounds(layer: &LayerState, measure: Option<Measure>) -> Option<[f64; 4]> {
    let (hw, hh) = match &layer.content {
        Content::Group(children) => {
            let mut b: Option<[f64; 4]> = None;
            for c in children {
                let mut c = c.clone();
                c.matrix = layer.matrix.then(c.matrix);
                if let Some(cb) = bounds(&c, measure) {
                    b = Some(match b {
                        None => cb,
                        Some(b) => [b[0].min(cb[0]), b[1].min(cb[1]), b[2].max(cb[2]), b[3].max(cb[3])],
                    });
                }
            }
            return b;
        }
        Content::Shape(s) if s.shape == crate::document::ShapeKind::Path => return None,
        Content::Shape(s) => {
            let stroke = s.stroke.map(|(_, w)| w / 2.0).unwrap_or(0.0);
            (s.size.x() / 2.0 + stroke, match s.shape {
                crate::document::ShapeKind::Ring | crate::document::ShapeKind::Arc | crate::document::ShapeKind::Polygon | crate::document::ShapeKind::Star => s.size.x() / 2.0,
                crate::document::ShapeKind::Line => s.thickness / 2.0,
                _ => s.size.y() / 2.0,
            } + stroke)
        }
        Content::Image { size: Some(size), .. } => (size.x() / 2.0, size.y() / 2.0),
        Content::Image { size: None, .. } => return None,
        Content::Video(v) => match v.size {
            Some(size) => (size.x() / 2.0, size.y() / 2.0),
            None => return None,
        },
        Content::Text(_) => return measure.and_then(|m| m(layer)),
        Content::Mesh(m) => return m.screen_bounds(),
    };
    let corners = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(x, y)| layer.matrix.apply(x, y));
    Some([
        corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min),
        corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min),
        corners.iter().map(|c| c.0).fold(f64::NEG_INFINITY, f64::max),
        corners.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max),
    ])
}

/// What `sync_check` looks at.
pub struct SyncOptions<'a> {
    /// Flag events further than this from their reference or any marker.
    pub frames: f64,
    /// Marker tracks to measure against (`sound` for sound effects); all
    /// when `None`.
    pub tracks: Option<&'a [String]>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SyncEvent {
    pub t: f64,
    pub frame: i64,
    pub kind: EventKind,
    pub element: String,
    pub what: String,
    /// The marker it was authored against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// For authored events: achieved − requested, in frames.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift_frames: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nearest: Option<Nearest>,
    /// `drift` (more than N frames off its reference) or `unsynced` (no
    /// marker within N frames).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Nearest {
    /// `track:n` (1-based), or `sound:<id>`.
    pub marker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub t: f64,
    /// Event − marker, in frames.
    pub frames: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SyncCheck {
    pub frames: f64,
    pub markers: usize,
    pub drift: usize,
    pub unsynced: usize,
    pub events: Vec<SyncEvent>,
}

/// Every visual event (cuts, layers appearing, keys landing, values
/// jumping) with the nearest marker. Events authored against a marker are
/// measured on the frame grid like `check` does and flagged `drift` when
/// more than `frames` off; the rest are flagged `unsynced` when no marker
/// (or sound effect) is within `frames`.
pub fn sync_check(project: &Project, compiled: &Compiled, options: &SyncOptions) -> SyncCheck {
    let f = compiled.fps().as_f64();
    let wanted = |track: &str| options.tracks.is_none_or(|ts| ts.iter().any(|t| t == track));
    let mut marks: Vec<(f64, String, Option<String>)> = Vec::new();
    for (name, track) in &project.markers.tracks {
        if wanted(name) {
            marks.extend(track.markers.iter().enumerate().map(|(i, m)| (m.t, format!("{name}:{}", i + 1), m.id.clone().or_else(|| m.label.clone()))));
        }
    }
    if wanted("sound") {
        for clip in &project.audio.sfx {
            if let Ok(t) = compiled.resolve(&project.markers, &clip.at) {
                let id = clip.id.clone().unwrap_or_else(|| clip.path.clone());
                marks.push((t, format!("sound:{id}"), None));
            }
        }
    }
    marks.sort_by(|a, b| a.0.total_cmp(&b.0));
    let nearest = |t: f64| -> Option<Nearest> {
        let i = marks.partition_point(|m| m.0 < t);
        [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|i| marks.get(i))
            .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
            .map(|(mt, name, label)| Nearest { marker: name.clone(), label: label.clone(), t: round(*mt), frames: ((t - mt) * f * 100.0).round() / 100.0 })
    };
    let cuts: Vec<f64> = compiled.events.iter().filter(|e| e.kind == EventKind::Cut).map(|e| e.t).collect();
    let half = 0.5 / f;
    let mut out = SyncCheck { frames: options.frames, markers: marks.len(), ..Default::default() };
    for e in &compiled.events {
        if e.t < -half || e.t > compiled.settings.duration + half {
            continue;
        }
        // The piece's start is not a sync point; layers appearing with a
        // cut are part of it, and a component's generated children
        // (`counter/v12`, one per displayed value) appearing is the
        // component working, not an event of its own.
        let generated = e.kind == EventKind::Appear && e.element.contains('/');
        if e.reference.is_none() && (generated || e.t < half || e.kind == EventKind::Appear && cuts.iter().any(|c| (c - e.t).abs() < half)) {
            continue;
        }
        // The authored point, measured as `check` measures it.
        let drift = e.reference.as_ref().and_then(|r| {
            let record = compiled.syncs.iter().find(|s| s.element == e.element && &s.reference == r && (s.what == e.what || e.kind == EventKind::Appear && s.what == "time.start" || e.kind == EventKind::Cut && s.what == "cut"))?;
            Some(((achieved(compiled, record) - record.requested) * f * 100.0).round() / 100.0)
        });
        let near = nearest(e.t);
        let flag = match drift {
            Some(d) if d.abs() > options.frames + 1e-6 => Some("drift"),
            Some(_) => None,
            None if e.reference.is_some() => None,
            None => (!marks.is_empty() && near.as_ref().is_none_or(|n| n.frames.abs() > options.frames + 1e-6)).then_some("unsynced"),
        };
        match flag {
            Some("drift") => out.drift += 1,
            Some(_) => out.unsynced += 1,
            None => {}
        }
        out.events.push(SyncEvent {
            t: round(e.t),
            frame: (e.t * f).round() as i64,
            kind: e.kind,
            element: e.element.clone(),
            what: e.what.clone(),
            reference: e.reference.clone(),
            drift_frames: drift,
            nearest: near,
            flag,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::MarkerTrack;
    use crate::time::Fps;

    fn project(step: Option<f64>) -> Project {
        let mut p = Project::new(1920, 1080, Fps::whole(30), 6.0);
        // 90 BPM from 1.093 s (the Motion Array grid).
        p.markers.insert(MarkerTrack::from_times("beat", (0..8).map(|i| 1.093 + i as f64 * 60.0 / 90.0)));
        let mut layer = serde_json::json!({
            "id": "dot", "kind": "shape", "shape": "circle", "size": 80, "fill": "#fff",
            "transform": {"position": {"keys": [
                {"at": 0, "v": [200, 540]},
                {"at": "beat:3", "v": [1700, 540], "ease": "gesture", "dur": "8f", "anchor": "peak", "lead": -1}
            ], "add": [{"type": "drift", "rate": [5, 0]}]}}
        });
        if let Some(s) = step {
            layer["time"] = serde_json::json!({"step_fps": s});
        }
        p.scenes.push(serde_json::from_value(serde_json::json!({"id": "s1", "start": "beat:1", "layers": [layer]})).unwrap());
        p.scenes[0].start = crate::timeref::TimeRef::Seconds(0.0);
        p
    }

    #[test]
    fn sync_check_lists_events_with_their_nearest_marker() {
        let mut p = Project::new(640, 360, Fps::whole(30), 4.0);
        p.markers.insert(MarkerTrack::from_times("hit", [1.0, 2.2]));
        p.scenes.push(
            serde_json::from_value(serde_json::json!({"id": "s1", "layers": [
                // Lands on hit 1 by hand: synced to it.
                {"id": "on", "kind": "shape", "shape": "rect", "fill": "#fff",
                 "transform": {"opacity": {"keys": [{"at": 0.5, "v": 0}, {"at": 1.0, "v": 1}]}}},
                // Lands 0.3 s after hit 2: nothing near.
                {"id": "late", "kind": "shape", "shape": "rect", "fill": "#fff",
                 "transform": {"position": {"keys": [{"at": 2.0, "v": [0, 0]}, {"at": 2.5, "v": [100, 0]}]}}},
                // Authored against hit 2 (a stepped clock is aligned to it).
                {"id": "stepped", "kind": "shape", "shape": "rect", "fill": "#fff", "time": {"step_fps": 2},
                 "transform": {"scale": {"keys": [{"at": 0, "v": [1, 1]}, {"at": "hit:2", "v": [2, 2], "dur": "6f", "anchor": "land"}]}}},
                // A counter: its values appearing are not events.
                {"id": "count", "kind": "component", "component": "counter", "params": {"from": 0, "to": 30, "at": 1.0, "dur": 1.0}},
                // Appears on hit 2; a colour jump (hold) at 3 s.
                {"id": "pop", "kind": "shape", "shape": "rect", "time": {"start": "hit:2"},
                 "fill": {"keys": [{"at": 2.2, "v": "#ff0000"}, {"at": 3.0, "v": "#00ff00", "ease": "hold"}]}}
            ]}))
            .unwrap(),
        );
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let r = sync_check(&p, &c, &SyncOptions { frames: 2.0, tracks: None });
        let find = |el: &str, kind: EventKind| r.events.iter().find(|e| e.element == el && e.kind == kind).unwrap_or_else(|| panic!("{el} {kind:?}: {r:#?}"));
        let on = find("on", EventKind::Land);
        assert_eq!((on.flag, on.nearest.as_ref().map(|n| n.marker.as_str())), (None, Some("hit:1")));
        let late = find("late", EventKind::Land);
        assert_eq!(late.flag, Some("unsynced"));
        assert_eq!(late.nearest.as_ref().unwrap().frames, 9.0);
        let stepped = find("stepped", EventKind::Land);
        assert_eq!(stepped.reference.as_deref(), Some("hit:2"));
        assert_eq!((stepped.drift_frames, stepped.flag), (Some(0.0), None));
        let pop = find("pop", EventKind::Appear);
        assert_eq!((pop.reference.as_deref(), pop.flag, pop.drift_frames), (Some("hit:2"), None, Some(0.0)));
        let jump = find("pop", EventKind::Jump);
        assert_eq!((jump.t, jump.flag), (3.0, Some("unsynced")));
        assert!(!r.events.iter().any(|e| e.element.starts_with("count/") && e.kind == EventKind::Appear), "{r:#?}");
        // Layers starting with the scene are part of its cut.
        assert!(!r.events.iter().any(|e| e.element == "on" && e.kind == EventKind::Appear));
        assert_eq!((r.drift, r.unsynced), (0, 2), "{:#?}", r.events.iter().filter(|e| e.flag.is_some()).collect::<Vec<_>>());
        // A peak on an 8 fps clock lands between frames: drift beyond a
        // tight tolerance.
        let p = project(Some(8.0));
        let c = Compiled::new(&p);
        let r = sync_check(&p, &c, &SyncOptions { frames: 0.25, tracks: None });
        let dot = r.events.iter().find(|e| e.element == "dot" && e.reference.is_some()).unwrap();
        assert!(dot.drift_frames.unwrap().abs() > 0.25 && dot.flag == Some("drift"), "{dot:?}");
        assert_eq!(r.drift, 1);
        // Only the tracks asked for count.
        let none = sync_check(&p, &c, &SyncOptions { frames: 2.0, tracks: Some(&["none".to_string()]) });
        assert_eq!((none.markers, none.unsynced), (0, 0));
    }

    #[test]
    fn smooth_layers_sync_within_half_a_frame() {
        let p = project(None);
        let c = Compiled::new(&p);
        let r = check(&p, &c, &Options::default());
        let s = r.syncs.iter().find(|s| s.element == "dot").unwrap();
        assert!(s.drift_frames.abs() <= 0.5, "{r:#?}");
        assert!(r.ok, "{r:#?}");
    }

    #[test]
    fn stepped_layers_report_their_drift() {
        let p = project(Some(8.0));
        let c = Compiled::new(&p);
        let r = check(&p, &c, &Options::default());
        let s = r.syncs.iter().find(|s| s.element == "dot").unwrap();
        // 8 fps holds of 4,4,4,3 frames move the measured peak.
        assert!(s.drift_frames.abs() > 0.0, "{s:?}");
        // Within one step (30/8 frames) that is as close as the layer can
        // land: not an error.
        assert!(s.drift_frames.abs() <= 30.0 / 8.0, "{s:?}");
        assert!(!r.issues.iter().any(|i| i.kind == "timing" && i.element.as_deref() == Some("dot")), "{:?}", r.issues);
    }

    #[test]
    fn holds_are_reported() {
        let mut p = Project::new(640, 360, Fps::whole(30), 8.0);
        p.scenes.push(serde_json::from_value(serde_json::json!({"id": "s1", "layers": [
            {"id": "still", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#fff", "transform": {"position": [320, 180]}}
        ]}))
        .unwrap());
        let c = Compiled::new(&p);
        let r = check(&p, &c, &Options::default());
        assert!(r.issues.iter().any(|i| i.message.contains("holds still")), "{r:#?}");
    }

    fn tracing_issues(layers: Vec<serde_json::Value>) -> Vec<Issue> {
        let mut p = Project::new(640, 360, Fps::whole(30), 4.0);
        p.scenes.push(serde_json::from_value(serde_json::json!({"id": "s1", "layers": layers})).unwrap());
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        check(&p, &c, &Options::default()).issues.into_iter().filter(|i| i.kind == "tracing").collect()
    }

    /// A quad roughly where frame `i` saw a blob in the reference.
    fn polygon(i: usize) -> String {
        let x = 100.0 + i as f64 * 7.0;
        format!("M {x} 100 L {} 96 L {} 160 L {} 171 Z", x + 52.0, x + 61.0, x - 4.0)
    }

    #[test]
    fn per_frame_polygons_warn_as_tracing() {
        // 14 polygons, each on screen for 2 frames: a flipbook traced from a reference.
        let flipbook = (0..14)
            .map(|i| serde_json::json!({"id": format!("blob-{i}"), "kind": "shape", "shape": "path", "d": polygon(i), "fill": "#e0a040",
                "time": {"start": i as f64 * 2.0 / 30.0, "end": (i as f64 * 2.0 + 2.0) / 30.0}}))
            .collect();
        let issues = tracing_issues(flipbook);
        assert_eq!(issues.len(), 1, "{issues:#?}");
        assert!(issues[0].severity == Severity::Warning && issues[0].message.contains("14 path layers"), "{issues:#?}");
        // Three outlines whose shape is keyed on every frame.
        let rotoscoped = (0..3)
            .map(|k| serde_json::json!({"id": format!("outline-{k}"), "kind": "shape", "shape": "path", "fill": "#3060c0",
                "d": {"keys": (0..20).map(|i| serde_json::json!({"at": i as f64 / 30.0, "v": polygon(i + k)})).collect::<Vec<_>>()}}))
            .collect();
        let issues = tracing_issues(rotoscoped);
        assert_eq!(issues.len(), 1, "{issues:#?}");
        assert!(issues[0].message.contains("every frame") && issues[0].message.contains("60 keys"), "{issues:#?}");
        // One outline keyed every frame for two seconds is enough.
        let one = vec![serde_json::json!({"id": "outline", "kind": "shape", "shape": "path", "fill": "#3060c0",
            "d": {"keys": (0..60).map(|i| serde_json::json!({"at": i as f64 / 30.0, "v": polygon(i)})).collect::<Vec<_>>()}})];
        assert_eq!(tracing_issues(one).len(), 1);
    }

    #[test]
    fn morphs_sparks_and_built_shapes_do_not_warn_as_tracing() {
        let mut layers = Vec::new();
        // An ordinary morph: three eased keys.
        layers.push(serde_json::json!({"id": "morph", "kind": "shape", "shape": "path", "fill": "#ffffff", "d": {"keys": [
            {"at": 0.5, "v": polygon(0)}, {"at": 1.0, "v": {"shape": "star", "size": 200}, "ease": "gesture"}, {"at": 1.6, "v": polygon(3), "ease": "glide"}]}}));
        // A 1-frame stagger of short-lived sparks built from primitives:
        // circles, primitive paths and two-point streaks, 2 frames each.
        for i in 0..16 {
            let time = serde_json::json!({"start": 2.0 + i as f64 / 30.0, "end": 2.0 + (i as f64 + 2.0) / 30.0});
            layers.push(serde_json::json!({"id": format!("spark-{i}"), "kind": "shape", "shape": "circle", "size": 8, "fill": "#ffd23f", "time": time}));
            layers.push(serde_json::json!({"id": format!("star-{i}"), "kind": "shape", "shape": "path", "d": {"shape": "star", "size": 30}, "fill": "#ffd23f", "time": time}));
            layers.push(serde_json::json!({"id": format!("streak-{i}"), "kind": "shape", "shape": "path", "d": "M 0 0 L 40 0", "stroke": {"color": "#ffffff", "width": 3}, "time": time}));
        }
        // Many hand-drawn polygons that stay on screen.
        for i in 0..20 {
            layers.push(serde_json::json!({"id": format!("shard-{i}"), "kind": "shape", "shape": "path", "d": polygon(i), "fill": "#ff3d7f", "time": {"start": i as f64 / 30.0}}));
        }
        // A few short-lived polygons, below the threshold.
        for i in 0..TRACE_FLASH_LAYERS - 1 {
            layers.push(serde_json::json!({"id": format!("flash-{i}"), "kind": "shape", "shape": "path", "d": polygon(i), "fill": "#ffffff",
                "time": {"start": 3.0 + i as f64 / 30.0, "end": 3.0 + (i as f64 + 1.0) / 30.0}}));
        }
        // A boil: a drawing redrawn on threes (12 keys, 3 frames apart).
        layers.push(serde_json::json!({"id": "boil", "kind": "shape", "shape": "path", "stroke": {"color": "#ffffff", "width": 4},
            "d": {"keys": (0..12).map(|i| serde_json::json!({"at": i as f64 * 3.0 / 30.0, "v": polygon(i % 3)})).collect::<Vec<_>>()}}));
        let issues = tracing_issues(layers);
        assert!(issues.is_empty(), "{issues:#?}");
    }

    #[test]
    fn text_contrast_is_measured_against_the_shape_behind_it() {
        let mut p = Project::new(640, 360, Fps::whole(30), 1.0);
        p.settings.background = crate::value::Color::parse("#15171c").unwrap();
        p.scenes.push(serde_json::from_value(serde_json::json!({"id": "s1", "background": "#15171c", "layers": [
            {"id": "button", "kind": "shape", "shape": "capsule", "size": [300, 80], "fill": "#ffd400", "transform": {"position": [320, 120]}},
            {"id": "cta", "kind": "text", "text": "Try free", "color": "#15171c", "transform": {"position": [320, 120]}},
            {"id": "lost", "kind": "text", "text": "dark on dark", "color": "#202228", "transform": {"position": [320, 260]}}
        ]}))
        .unwrap());
        let c = Compiled::new(&p);
        // Text boxes: 200 × 40 around each text's position.
        let measure = |l: &LayerState| {
            let (x, y) = l.matrix.apply(0.0, 0.0);
            Some([x - 100.0, y - 20.0, x + 100.0, y + 20.0])
        };
        let r = check(&p, &c, &Options { measure: Some(&measure), ..Default::default() });
        let contrast: Vec<_> = r.issues.iter().filter(|i| i.message.contains("contrast")).collect();
        assert!(!contrast.iter().any(|i| i.element.as_deref() == Some("cta")), "dark text on a yellow button reads fine: {contrast:?}");
        assert!(contrast.iter().any(|i| i.element.as_deref() == Some("lost") && i.message.contains("the background")), "{contrast:?}");
    }

    #[test]
    fn title_safe_uses_where_text_is_on_screen() {
        let mut p = Project::new(640, 360, Fps::whole(30), 2.0);
        // A label in a comp that a camera push brings in from off-frame,
        // and one that ends up hugging the edge.
        p.scenes.push(serde_json::from_value(serde_json::json!({"id": "s1", "layers": [
            {"id": "world", "kind": "comp", "transform": {"position": {"keys": [{"at": 0, "v": [900, 0]}, {"at": 1, "v": [0, 0]}]}}, "layers": [
                {"id": "label", "kind": "text", "text": "IRAN", "transform": {"position": [320, 180]}},
                {"id": "edge", "kind": "text", "text": "OMAN", "transform": {"position": [612, 180]}}]}
        ]}))
        .unwrap());
        let c = Compiled::new(&p);
        let measure = |l: &LayerState| {
            let (x, y) = l.matrix.apply(0.0, 0.0);
            Some([x - 20.0, y - 10.0, x + 20.0, y + 10.0])
        };
        let r = check(&p, &c, &Options { measure: Some(&measure), ..Default::default() });
        let safe: Vec<_> = r.issues.iter().filter(|i| i.message.contains("title-safe")).collect();
        assert!(!safe.iter().any(|i| i.element.as_deref() == Some("label")), "{safe:?}");
        assert!(safe.iter().any(|i| i.element.as_deref() == Some("edge")), "nested text is checked where it lands: {safe:?}");
    }

    #[test]
    fn scenes_crossing_in_a_transition_are_not_compared() {
        let mut p = Project::new(640, 360, Fps::whole(30), 2.0);
        let scene = |id: &str, start: f64, word: &str| {
            serde_json::from_value::<crate::document::Scene>(serde_json::json!({"id": id, "start": start, "transition": {"type": "fade", "dur": "10f"},
                "layers": [{"id": word, "kind": "text", "text": word, "transform": {"position": [320, 180]}}]})).unwrap()
        };
        p.scenes.push(scene("a", 0.0, "ONE"));
        p.scenes.push(scene("b", 1.0, "TWO"));
        let c = Compiled::new(&p);
        let measure = |l: &LayerState| {
            let (x, y) = l.matrix.apply(0.0, 0.0);
            Some([x - 40.0, y - 10.0, x + 40.0, y + 10.0])
        };
        let r = check(&p, &c, &Options { measure: Some(&measure), ..Default::default() });
        assert!(!r.issues.iter().any(|i| i.message.contains("overlaps")), "{:?}", r.issues);
    }

    // ------------------------------------------- declared intent, craft

    /// `kind` issues of a 1920×1080, 30 fps film of `scenes`; text is
    /// measured as 0.55 em per character by one em.
    fn kind_issues(duration: f64, scenes: serde_json::Value, kind: &str) -> Vec<Issue> {
        let mut p = Project::new(1920, 1080, Fps::whole(30), duration);
        for s in scenes.as_array().unwrap() {
            p.scenes.push(serde_json::from_value(s.clone()).unwrap());
        }
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let measure = |l: &LayerState| {
            let Content::Text(t) = &l.content else { return None };
            let (x, y) = l.matrix.apply(0.0, 0.0);
            let em = em_px(l.matrix, t.size);
            let w = t.text.chars().count() as f64 * em * 0.55;
            Some([x - w / 2.0, y - em / 2.0, x + w / 2.0, y + em / 2.0])
        };
        check(&p, &c, &Options { measure: Some(&measure), ..Default::default() }).issues.into_iter().filter(|i| i.kind == kind).collect()
    }

    fn text(id: &str, words: &str, size: f64, at: [f64; 2]) -> serde_json::Value {
        serde_json::json!({"id": id, "kind": "text", "text": words, "size": size, "transform": {"position": at}})
    }

    #[test]
    fn depth_order_catches_a_background_in_front_of_its_subject() {
        use serde_json::json;
        let person = json!({"id": "person", "kind": "shape", "shape": "capsule", "size": [120, 360], "fill": "#e0a040", "role": "subject", "transform": {"position": [900, 600]}});
        let pillar = json!({"id": "pillar", "kind": "shape", "shape": "rect", "size": [220, 1080], "fill": "#808080", "role": "background", "transform": {"position": [920, 540]}});
        let scene = |layers: Vec<serde_json::Value>| json!([{"id": "s1", "layers": layers}]);
        let issues = kind_issues(2.0, scene(vec![person.clone(), pillar.clone()]), "depth_order");
        assert_eq!(issues.len(), 1, "{issues:#?}");
        assert!(issues[0].severity == Severity::Error && issues[0].message.contains("'pillar' (background) is drawn in front of 'person' (subject)"), "{issues:#?}");
        // Fixed: the pillar below the person.
        assert!(kind_issues(2.0, scene(vec![pillar.clone(), person.clone()]), "depth_order").is_empty());
        // In 3D the camera's depth decides, not the list: a pillar nearer
        // the camera covers the person whatever its place in the list.
        let at_z = |l: &serde_json::Value, z: f64| {
            let mut l = l.clone();
            l["three_d"] = json!(true);
            l["transform"]["z"] = json!(z);
            l
        };
        assert_eq!(kind_issues(2.0, scene(vec![at_z(&pillar, -300.0), at_z(&person, 0.0)]), "depth_order").len(), 1);
        assert!(kind_issues(2.0, scene(vec![at_z(&person, 0.0), at_z(&pillar, 300.0)]), "depth_order").is_empty());
        // A foreground label behind its subject.
        let label = json!({"id": "tag", "kind": "shape", "shape": "rect", "size": [100, 60], "fill": "#ffffff", "role": "foreground", "transform": {"position": [900, 600]}});
        let issues = kind_issues(2.0, scene(vec![label.clone(), person.clone()]), "depth_order");
        assert!(issues.len() == 1 && issues[0].message.contains("'tag' (foreground) is drawn behind 'person'"), "{issues:#?}");
        assert!(kind_issues(2.0, scene(vec![person, label]), "depth_order").is_empty());
    }

    #[test]
    fn moves_backwards_catches_a_sprite_going_against_its_facing() {
        use serde_json::json;
        // A car whose art points right, driving right to left.
        let car = |facing: serde_json::Value, flip: f64| {
            json!({"id": "car", "kind": "shape", "shape": "rect", "size": [160, 60], "fill": "#c00000", "facing": facing,
                "transform": {"scale": [flip, 1], "position": {"keys": [{"at": 0, "v": [1600, 760]}, {"at": 2, "v": [300, 760]}]}}})
        };
        let road = json!({"id": "road", "kind": "shape", "shape": "rect", "size": [1920, 40], "fill": "#333333", "transform": {"position": [960, 800]}});
        let scene = |layers: Vec<serde_json::Value>| json!([{"id": "s1", "layers": layers}]);
        let issues = kind_issues(2.0, scene(vec![road.clone(), car(json!("right"), 1.0)]), "moves_backwards");
        assert!(issues.len() == 1 && issues[0].severity == Severity::Error && issues[0].message.contains("faces right on screen but moves left"), "{issues:#?}");
        // Fixed: the art faces left, or is flipped to.
        assert!(kind_issues(2.0, scene(vec![road.clone(), car(json!("left"), 1.0)]), "moves_backwards").is_empty());
        assert!(kind_issues(2.0, scene(vec![road.clone(), car(json!(0), -1.0)]), "moves_backwards").is_empty());
        // Parked in a world that pans left: the camera moves, not the car.
        let parked = json!({"id": "car", "kind": "shape", "shape": "rect", "size": [160, 60], "fill": "#c00000", "facing": "right", "transform": {"position": [1600, 760]}});
        let world = json!({"id": "world", "kind": "comp", "layers": [road.clone(), parked],
            "transform": {"position": {"keys": [{"at": 0, "v": [0, 0]}, {"at": 2, "v": [-1300, 0]}]}}});
        assert!(kind_issues(2.0, scene(vec![world]), "moves_backwards").is_empty());
        // Riding a path with `rotate` it faces the path's way; art drawn
        // facing left on it travels tail first.
        let route = json!({"id": "route", "kind": "shape", "shape": "path", "d": "M 1600 500 L 300 500", "stroke": {"color": "#ffffff", "width": 2}});
        let plane = |facing: Option<&str>| {
            let mut p = json!({"id": "plane", "kind": "shape", "shape": "polygon", "sides": 3, "size": 80, "fill": "#ffffff",
                "follow": {"layer": "route", "at": {"keys": [{"at": 0, "v": 0}, {"at": 2, "v": 1}]}, "rotate": true}});
            if let Some(f) = facing {
                p["facing"] = json!(f);
            }
            p
        };
        assert!(kind_issues(2.0, scene(vec![route.clone(), plane(None)]), "moves_backwards").is_empty());
        assert_eq!(kind_issues(2.0, scene(vec![route, plane(Some("left"))]), "moves_backwards").len(), 1);
    }

    #[test]
    fn text_below_floor_flags_small_type_on_screen() {
        use serde_json::json;
        // 28 px at 1080: 20 px print, and 40 px type in a comp scaled to half.
        let layers = json!([{"id": "s1", "layers": [
            text("headline", "Every frame", 80.0, [960.0, 300.0]),
            text("small", "source: someone", 20.0, [960.0, 700.0]),
            {"id": "card", "kind": "comp", "transform": {"scale": 0.5}, "layers": [text("shrunk", "a note", 40.0, [960.0, 1500.0])]},
            {"id": "credit", "kind": "text", "text": "music: someone", "size": 18, "role": "decoration", "transform": {"position": [960, 1000]}}
        ]}]);
        let issues = kind_issues(2.0, layers, "text_below_floor");
        assert_eq!(issues.len(), 1, "{issues:#?}");
        let m = &issues[0].message;
        assert!(m.contains("'small' (20.0 px)") && m.contains("'shrunk' (20.0 px)") && !m.contains("headline") && !m.contains("credit"), "{m}");
        let fine = json!([{"id": "s1", "layers": [text("headline", "Every frame", 80.0, [960.0, 300.0]), text("label", "a label", 30.0, [960.0, 700.0])]}]);
        assert!(kind_issues(2.0, fine, "text_below_floor").is_empty());
    }

    #[test]
    fn text_crowded_flags_a_screen_full_of_labels() {
        use serde_json::json;
        let labels = |n: usize, role: Option<&str>| {
            let layers: Vec<serde_json::Value> = (0..n)
                .map(|i| {
                    let mut t = text(&format!("l{i}"), "LABEL", 40.0, [960.0, 150.0 + i as f64 * 110.0]);
                    if let Some(r) = role {
                        t["role"] = json!(r);
                    }
                    t
                })
                .collect();
            json!([{"id": "s1", "layers": layers}])
        };
        let issues = kind_issues(2.0, labels(7, None), "text_crowded");
        assert!(issues.len() == 1 && issues[0].message.contains("7 text layers"), "{issues:#?}");
        assert!(kind_issues(2.0, labels(4, None), "text_crowded").is_empty());
        assert!(kind_issues(2.0, labels(7, Some("caption")), "text_crowded").is_empty());
        // Or one paragraph of more than 12 words.
        let paragraph = json!([{"id": "s1", "layers": [text("para", "one two three four five six seven eight nine ten eleven twelve thirteen", 40.0, [960.0, 540.0])]}]);
        assert_eq!(kind_issues(2.0, paragraph, "text_crowded").len(), 1);
    }

    #[test]
    fn line_unread_flags_lines_gone_before_they_are_read() {
        use serde_json::json;
        let timed = |id: &str, words: &str, start: f64, end: f64, y: f64| {
            let mut t = text(id, words, 60.0, [960.0, y]);
            t["time"] = json!({"start": start, "end": end});
            t
        };
        let film = json!([{"id": "s1", "layers": [
            // 4 words need 0.75 × (0.5 + 4 × 0.3) = 1.275 s.
            timed("quick", "this goes too fast", 0.5, 1.1, 200.0),
            timed("held", "this stays long enough", 0.5, 2.0, 400.0),
            timed("word", "BOOM", 0.5, 0.95, 600.0),
            // A count: numbers replaced at one place.
            timed("n1", "10", 1.0, 1.1, 800.0), timed("n2", "11", 1.1, 1.2, 800.0), timed("n3", "12", 1.2, 2.5, 800.0),
            {"id": "count", "kind": "component", "component": "counter", "params": {"from": 0, "to": 500, "at": 1.0, "dur": 1.0}, "transform": {"position": [960, 900]}},
            // On at the end: the film's last line needs 2 s.
            timed("end", "BYE", 3.0, 4.0, 1000.0)
        ]}]);
        let issues = kind_issues(4.0, film, "line_unread");
        assert_eq!(issues.len(), 1, "{issues:#?}");
        let m = &issues[0].message;
        assert!(m.contains("'quick' (4 words, 0.60 s of 1.27 s)") && m.contains("'end' (1 word, 1.00 s of 2.00 s)"), "{m}");
        assert!(!m.contains("held") && !m.contains("'word'") && !m.contains("'n1'") && !m.contains("count/"), "{m}");
    }

    #[test]
    fn repeats_caption_flags_text_saying_the_caption() {
        use serde_json::json;
        let film = |words: &str| {
            json!([{"id": "s1", "layers": [
                text("title", words, 80.0, [960.0, 300.0]),
                {"id": "cap", "kind": "text", "text": "FUEL PRICES EXPLODE EVERYWHERE ON EARTH.", "size": 60, "role": "caption", "transform": {"position": [960, 900]}}
            ]}])
        };
        let issues = kind_issues(2.0, film("Everywhere on Earth"), "repeats_caption");
        assert!(issues.len() == 1 && issues[0].message.contains("'title' says \"everywhere on earth\" with the caption 'cap'"), "{issues:#?}");
        assert!(kind_issues(2.0, film("+340%"), "repeats_caption").is_empty());
        assert!(kind_issues(2.0, film("Prices on Earth"), "repeats_caption").is_empty());
    }

    #[test]
    fn persistent_chrome_flags_a_header_on_every_scene() {
        use serde_json::json;
        let film = |header_in: usize| {
            let scenes: Vec<serde_json::Value> = (0..3)
                .map(|k| {
                    // Different words at the centre of each scene are content.
                    let mut layers = vec![text(&format!("t{k}"), ["ONE", "TWO", "THREE"][k], 120.0, [960.0, 540.0])];
                    if k < header_in {
                        layers.push(text(&format!("h{k}"), "LAHZA · ENGINE", 30.0, [200.0, 50.0]));
                        layers.push(json!({"id": format!("bar{k}"), "kind": "shape", "shape": "rect", "size": [1700, 4], "fill": "#ffffff", "transform": {"position": [960, 1040]}}));
                    }
                    json!({"id": format!("s{k}"), "start": k as f64 * 2.0, "layers": layers})
                })
                .collect();
            json!(scenes)
        };
        let issues = kind_issues(6.0, film(3), "persistent_chrome");
        assert_eq!(issues.len(), 1, "{issues:#?}");
        let m = &issues[0].message;
        assert!(m.contains("2 thing(s)") && m.contains("'LAHZA · ENGINE'") && m.contains("1700×4 px rect") && !m.contains("ONE"), "{m}");
        assert!(kind_issues(6.0, film(1), "persistent_chrome").is_empty());
    }

    #[test]
    fn world_crossfade_flags_a_dissolve_between_scenes() {
        use serde_json::json;
        let film = |transition: &str, allow: bool, empty: bool| {
            let mut b = json!({"id": "b", "start": 1.5, "transition": {"type": transition, "dur": "10f"}, "layers": [text("two", "TWO", 120.0, [960.0, 540.0])]});
            if allow {
                b["allow"] = json!(["world_crossfade"]);
            }
            let a = if empty { json!({"id": "a", "layers": []}) } else { json!({"id": "a", "layers": [text("one", "ONE", 120.0, [960.0, 540.0])]}) };
            json!([a, b])
        };
        let issues = kind_issues(3.0, film("fade", false, false), "world_crossfade");
        assert!(issues.len() == 1 && issues[0].element.as_deref() == Some("b") && issues[0].message.contains("dissolves in over 'a'"), "{issues:#?}");
        assert!(kind_issues(3.0, film("whip", false, false), "world_crossfade").is_empty());
        assert!(kind_issues(3.0, film("fade", true, false), "world_crossfade").is_empty());
        // Out of an empty scene (a colour) is a fade-in, not two worlds.
        assert!(kind_issues(3.0, film("fade", false, true), "world_crossfade").is_empty());
    }

    #[test]
    fn draws_off_frame_flags_a_stroke_to_a_place_off_screen() {
        use serde_json::json;
        let film = |d: &str| {
            json!([{"id": "s1", "layers": [{"id": "arc", "kind": "shape", "shape": "path", "d": d, "stroke": {"color": "#ff0000", "width": 6},
                "trim": {"keys": [{"at": 0.5, "from": [0, 0], "v": [0, 1], "ease": "glide", "dur": "20f", "anchor": "start"}]}}]}])
        };
        let issues = kind_issues(2.0, film("M 960 540 Q 1600 100 2300 400"), "draws_off_frame");
        assert!(issues.len() == 1 && issues[0].message.contains("'arc' (head at 2300, 400 px"), "{issues:#?}");
        assert!(kind_issues(2.0, film("M 960 540 Q 1400 100 1700 400"), "draws_off_frame").is_empty());
        // An outline traced around a shape that runs past the edge.
        assert!(kind_issues(2.0, film("M 960 540 L 2300 540 L 2300 900 Z"), "draws_off_frame").is_empty());
    }

    #[test]
    fn components_declare_roles_and_allows() {
        use serde_json::json;
        let mut p = Project::new(1080, 1920, Fps::whole(30), 4.0);
        p.markers.insert(MarkerTrack::from_times("word", [0.5, 1.0]));
        for (m, label) in p.markers.tracks.get_mut("word").unwrap().markers.iter_mut().zip(["hello", "world"]) {
            m.label = Some(label.into());
            m.end = Some(m.t + 0.3);
        }
        p.scenes.push(serde_json::from_value(json!({"id": "s1", "layers": [
            {"id": "map", "kind": "component", "component": "map", "params": {"camera": {"center": "Tehran", "zoom": 2}, "labels": ["IRN"], "places": ["Tehran"]}},
            {"id": "caps", "kind": "component", "component": "captions"},
            {"id": "n", "kind": "component", "component": "counter", "params": {"to": 9}},
            {"id": "deck", "kind": "comp", "role": "subject", "allow": ["layout"], "layers": [{"id": "deck-a", "kind": "shape", "shape": "rect"}]}
        ]})).unwrap());
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let role = |id: &str| c.intents.get(id).and_then(|i| i.role);
        assert_eq!((role("map/water"), role("map/IRN"), role("map/IRN-label"), role("map/p-tehran")), (Some(Role::Background), Some(Role::Background), None, None));
        assert_eq!(role("caps/c1"), Some(Role::Caption));
        assert!(c.intents.get("n/v0").is_some_and(|i| i.allows("line_unread")));
        // A comp's role and allows pass to its layers.
        assert!(c.intents.get("deck-a").is_some_and(|i| i.role == Some(Role::Subject) && i.allows("layout")));
    }
}
