//! `check` (design section 10): structure, timing (sync drift per anchor,
//! long holds), layout (title-safe, overlapping text, contrast) and
//! licensing. Sync is measured the way the studies measured references:
//! on the rendered frame grid, through stepped clocks and pre-roll, not
//! from the requested numbers.

use crate::catalog::style_pack;
use crate::compile::{Compiled, Content, FrameState, LayerState, SyncKind};
use crate::document::{walk_layers, LayerKind, Project};
use crate::timeref::Anchor;
use serde::Serialize;
use std::path::Path;

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

pub fn check(project: &Project, compiled: &Compiled, options: &Options) -> Report {
    let mut issues = Vec::new();
    for e in &compiled.errors {
        issues.push(Issue { severity: Severity::Error, kind: "structure", element: e.split(['.', ':']).next().map(String::from), at: None, message: e.clone() });
    }
    if project.scenes.is_empty() {
        issues.push(issue(Severity::Warning, "structure", None, None, "the project has no scenes".into()));
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
        let achieved = match &s.kind {
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
        };
        let drift = (achieved - s.requested) * f;
        if drift.abs() > options.tolerance_frames + 1e-6 {
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
    let (w, h) = (compiled.settings.width as f64, compiled.settings.height as f64);
    let safe = [w * 0.05, h * 0.05, w * 0.95, h * 0.95];
    let layout_every = (f / 4.0).round().max(1.0) as u64;
    for n in 0..frames {
        let t = fps.time_of(n as i64);
        let frame = compiled.frame(t);
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
            let mut text_boxes: Vec<(&str, [f64; 4])> = Vec::new();
            for layer in &frame.layers {
                let Some(b) = bounds(layer, options.measure) else { continue };
                let visible = layer.opacity >= 0.5 && b[2] > 0.0 && b[3] > 0.0 && b[0] < w && b[1] < h;
                if !visible {
                    continue;
                }
                let is_text = matches!(layer.content, Content::Text(_));
                let fully_on = b[0] >= 0.0 && b[1] >= 0.0 && b[2] <= w && b[3] <= h;
                if is_text && fully_on && (b[0] < safe[0] || b[1] < safe[1] || b[2] > safe[2] || b[3] > safe[3]) && safe_reported.insert(layer.id.to_string()) {
                    issues.push(issue(Severity::Warning, "layout", Some(&layer.id), Some(t), "text sits outside the title-safe area (5% margin)".into()));
                }
                if is_text {
                    for (other, ob) in &text_boxes {
                        let overlap = b[0] < ob[2] && ob[0] < b[2] && b[1] < ob[3] && ob[1] < b[3];
                        let key = format!("{other}+{}", layer.id);
                        if overlap && overlap_reported.insert(key) {
                            issues.push(issue(Severity::Info, "layout", Some(&layer.id), Some(t), format!("text overlaps '{other}'")));
                        }
                    }
                    text_boxes.push((&layer.id, b));
                    if let Content::Text(tx) = &layer.content {
                        let ratio = contrast(tx.color.luminance(), frame.background.luminance());
                        if ratio < 3.0 && contrast_reported.insert(layer.id.to_string()) {
                            issues.push(issue(Severity::Warning, "layout", Some(&layer.id), Some(t), format!("text contrast against the background is {ratio:.1}:1 (aim for at least 3:1)")));
                        }
                    }
                }
            }
        }
        prev = Some(frame);
    }

    // Assets and licences.
    let mut used: Vec<(String, String)> = Vec::new();
    for scene in &project.scenes {
        walk_layers(&scene.layers, &mut |l| {
            if let LayerKind::Image(img) = &l.kind {
                used.push((l.id.clone(), img.src.clone()));
            }
        });
    }
    for clip in project.audio.clips() {
        used.push(("audio".into(), clip.path.clone()));
    }
    for (element, path) in used {
        if let Some(dir) = options.dir {
            if !dir.join(&path).is_file() {
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

    issues.sort_by(|a, b| b.severity.cmp(&a.severity));
    let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
    let warnings = issues.iter().filter(|i| i.severity == Severity::Warning).count();
    let max_drift_frames = syncs.iter().map(|s| s.drift_frames.abs()).fold(0.0, f64::max);
    Report { ok: errors == 0, errors, warnings, issues, syncs, max_drift_frames }
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
pub fn bounds(layer: &LayerState, measure: Option<Measure>) -> Option<[f64; 4]> {
    let (hw, hh) = match &layer.content {
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
        Content::Text(_) => return measure.and_then(|m| m(layer)),
    };
    let corners = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(x, y)| layer.matrix.apply(x, y));
    Some([
        corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min),
        corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min),
        corners.iter().map(|c| c.0).fold(f64::NEG_INFINITY, f64::max),
        corners.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max),
    ])
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
}
