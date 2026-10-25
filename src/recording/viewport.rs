use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    clips::RecordingClipTimeline,
    model::{NormalizedPoint, PointerCaptureFile, PressPhase},
    motion::{DampedSpring, SpringConstant},
    pointer_timeline::PointerTimeline,
    scene::SceneTransform,
};

/// Normalized rectangle of the media a viewport frame shows:
/// `(left, top, visible_fraction)`. Preview, focus picking, and export all
/// derive the crop from this single definition.
/// The media window `(left, top, width, height)` a frame shows, as fractions
/// of the (cropped) media. Fill mode narrows one axis to the card's aspect.
pub fn visible_rect(frame: ViewportFrame) -> (f64, f64, f64, f64) {
    let magnification = frame.magnification.max(1.0);
    let visible_x = (frame.fill.0.clamp(0.01, 1.0) / magnification).min(1.0);
    let visible_y = (frame.fill.1.clamp(0.01, 1.0) / magnification).min(1.0);
    let left = (frame.anchor.x - visible_x * 0.5).clamp(0.0, 1.0 - visible_x);
    let top = (frame.anchor.y - visible_y * 0.5).clamp(0.0, 1.0 - visible_y);
    (left, top, visible_x, visible_y)
}

const STEP_RATE: f64 = 120.0;
const CLUSTER_WIDTH_RATIO: f64 = 0.5;
const CLUSTER_HEIGHT_RATIO: f64 = 0.5;
const MOTION_PROFILE: SpringConstant = SpringConstant {
    tension: 200.0,
    friction: 40.0,
    inertia: 2.25,
};

/// Camera response, independent of the authored ramp's easing curve.
/// Standard preserves the response of projects saved before this control.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraMotion {
    #[default]
    Standard,
    Smooth,
    Focused,
}

impl CameraMotion {
    pub const ALL: [Self; 3] = [Self::Standard, Self::Smooth, Self::Focused];

    fn spring(self) -> SpringConstant {
        match self {
            Self::Standard => MOTION_PROFILE,
            Self::Smooth => SpringConstant { tension: 170.0, friction: 50.0, inertia: 3.0 },
            Self::Focused => SpringConstant { tension: 250.0, friction: 40.0, inertia: 2.25 },
        }
    }

    fn step(self, spring: &mut DampedSpring, target: f64, dt: f64) {
        let mut constant = self.spring();
        if self != Self::Standard {
            // Brake fast approaches using critical damping. This retains
            // velocity through target changes while reducing overshoot.
            let displacement = target - spring.position;
            let natural_frequency = (constant.tension / constant.inertia).sqrt();
            if spring.velocity * displacement > 0.0
                && spring.velocity.abs() > displacement.abs() * natural_frequency
            {
                constant.friction = constant.friction.max(
                    2.0 * (constant.inertia * constant.tension).sqrt(),
                );
            }
        }
        spring.step(target, constant, dt);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ZoomPresentation {
    #[default]
    Classic,
    GlassLoupe,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoupeOptions {
    /// Radius as a fraction of the media's longer edge.
    pub radius: f64,
    pub bevel: f64,
    pub chromatic_aberration: f64,
    pub glass: bool,
}

impl Default for LoupeOptions {
    fn default() -> Self {
        Self { radius: 0.35, bevel: 0.2, chromatic_aberration: 0.5, glass: true }
    }
}

impl LoupeOptions {
    pub fn clamped(self) -> Self {
        let finite = |value: f64, fallback: f64| if value.is_finite() { value } else { fallback };
        Self {
            radius: finite(self.radius, 0.35).clamp(0.25, 0.75),
            bevel: finite(self.bevel, 0.2).clamp(0.0, 0.5),
            chromatic_aberration: finite(self.chromatic_aberration, 0.5).clamp(0.0, 1.0),
            glass: self.glass,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoupeFrame {
    pub center: NormalizedPoint,
    pub radius: f64,
    pub magnification: f64,
    pub bevel: f64,
    pub chromatic_aberration: f64,
    pub presence: f64,
}

impl LoupeFrame {
    fn interpolate(self, other: Self, fraction: f64) -> Self {
        Self {
            center: NormalizedPoint {
                x: lerp(self.center.x, other.center.x, fraction),
                y: lerp(self.center.y, other.center.y, fraction),
            },
            radius: lerp(self.radius, other.radius, fraction),
            magnification: lerp(self.magnification, other.magnification, fraction),
            bevel: lerp(self.bevel, other.bevel, fraction),
            chromatic_aberration: lerp(self.chromatic_aberration, other.chromatic_aberration, fraction),
            presence: lerp(self.presence, other.presence, fraction),
        }
    }
}

/// Animated 3D rotation (degrees) added to the authored scene transform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Tilt {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Tilt {
    pub fn is_zero(&self) -> bool {
        self.x.abs() < 1e-9 && self.y.abs() < 1e-9 && self.z.abs() < 1e-9
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportFrame {
    pub magnification: f64,
    pub anchor: NormalizedPoint,
    pub tilt: Tilt,
    pub transform: SceneTransform,
    /// Fraction of the media visible on each axis at magnification 1. Below
    /// 1 when Fill crops the media to the card's aspect ratio.
    pub fill: (f64, f64),
    /// Local magnification over the screen; independent of the classic camera.
    pub loupe: Option<LoupeFrame>,
}

impl Default for ViewportFrame {
    fn default() -> Self {
        Self {
            magnification: 1.0,
            anchor: NormalizedPoint { x: 0.5, y: 0.5 },
            tilt: Tilt::default(),
            transform: SceneTransform::IDENTITY,
            fill: (1.0, 1.0),
            loupe: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ViewportTimeline {
    frames: Vec<ViewportSample>,
    duration: f64,
    cues: Vec<ZoomCue>,
    cue_clusters: Vec<Vec<FocusCluster>>,
    focus_bounds: (f64, f64),
    clips: RecordingClipTimeline,
    reframe: Option<std::sync::Arc<Reframe>>,
}

/// Fill mode's follow camera: the media is cropped to the card's aspect and
/// the crop drifts calmly after the pointer. Zoom cues bring their own
/// camera, so the crop hands over to them as they ramp in.
#[derive(Clone, Debug, PartialEq)]
pub struct Reframe {
    /// Visible media fraction per axis at magnification 1.
    pub fill: (f64, f64),
    /// What the track was built for, so callers can tell when it is stale.
    pub key: ReframeKey,
    /// Crop center per step, in source-normalized coordinates.
    anchors: Vec<NormalizedPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReframeKey {
    pub fill: (f64, f64),
    pub crop: crate::CropRect,
}

impl Reframe {
    /// Share of the visible crop the pointer roams before the camera moves.
    const DEAD_ZONE: f64 = 0.28;
    /// Low-pass time constant for camera moves, in seconds.
    const SMOOTHING_TAU: f64 = 0.45;
    /// Magnification over which the crop hands over to a zoom cue's anchor.
    const ZOOM_HANDOFF: f64 = 0.2;

    fn anchor_at(&self, time: f64) -> Option<NormalizedPoint> {
        let first = *self.anchors.first()?;
        let position = time.max(0.0) * STEP_RATE;
        let index = position as usize;
        if index + 1 >= self.anchors.len() {
            return Some(*self.anchors.last().unwrap_or(&first));
        }
        let fraction = position - index as f64;
        let (a, b) = (self.anchors[index], self.anchors[index + 1]);
        Some(NormalizedPoint {
            x: lerp(a.x, b.x, fraction),
            y: lerp(a.y, b.y, fraction),
        })
    }
}

/// Framing is a proportion of the available crop travel, not a source-space
/// crop center. Its valid range stays 0..1 at every magnification.
#[derive(Clone, Copy, Debug)]
struct ViewportSample {
    magnification: f64,
    framing: NormalizedPoint,
    tilt: Tilt,
    loupe: Option<LoupeFrame>,
}

impl ViewportSample {
    fn frame(self, transform: SceneTransform) -> ViewportFrame {
        let half = 0.5 / self.magnification.max(1.0);
        let travel = 1.0 - 2.0 * half;
        ViewportFrame {
            magnification: self.magnification,
            anchor: NormalizedPoint {
                x: half + self.framing.x.clamp(0.0, 1.0) * travel,
                y: half + self.framing.y.clamp(0.0, 1.0) * travel,
            },
            tilt: self.tilt,
            transform,
            fill: (1.0, 1.0),
            loupe: self.loupe.filter(|frame| frame.presence > 0.0001),
        }
    }
}

impl ViewportTimeline {
    /// Viewport motion for media without pointer data (animated screenshots).
    pub fn build_static(cues: &[ZoomCue], duration: f64) -> Self {
        Self::build(
            cues,
            &PointerTimeline::default(),
            &RecordingClipTimeline::full(duration),
            &PointerCaptureFile::default(),
        )
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn build(
        cues: &[ZoomCue],
        pointer: &PointerTimeline,
        clips: &RecordingClipTimeline,
        capture: &PointerCaptureFile,
    ) -> Self {
        Self::build_for_viewport(cues, pointer, clips, capture, (1.0, 1.0))
    }

    /// Visible source fraction before zoom (crop multiplied by Fill). Camera
    /// activity groups must fit the actual viewport, including portrait crops.
    pub fn build_for_viewport(
        cues: &[ZoomCue],
        pointer: &PointerTimeline,
        clips: &RecordingClipTimeline,
        capture: &PointerCaptureFile,
        visible_source: (f64, f64),
    ) -> Self {
        let sanitize = |value: f64| if value.is_finite() { value.clamp(0.001, 1.0) } else { 1.0 };
        let focus_bounds = (sanitize(visible_source.0), sanitize(visible_source.1));
        let duration = clips.duration();
        if !duration.is_finite() || duration <= 0.0 {
            return Self::default();
        }
        // A removed animation cannot leave a held end pose on later footage.
        // Keep authored cues untouched so undo restores them with the clip.
        let retained_cues: Vec<_> = cues.iter()
            .filter(|cue| !clips.slices_overlapping(cue.start, cue.end).is_empty())
            .cloned().collect();
        let cues = retained_cues.as_slice();
        let frame_count = ((duration * STEP_RATE).ceil() as usize + 1).max(2);
        let dt = 1.0 / STEP_RATE;
        let mut amount = DampedSpring::new(1.0);
        let mut framing_x = DampedSpring::new(0.5);
        let mut framing_y = DampedSpring::new(0.5);
        let mut held_framing = NormalizedPoint { x: 0.5, y: 0.5 };
        let mut tilt_x = DampedSpring::new(0.0);
        let mut tilt_y = DampedSpring::new(0.0);
        let mut tilt_z = DampedSpring::new(0.0);
        let mut previous_cue = None;
        let mut camera_motion = CameraMotion::default();
        let mut loupe_motion = LoupeMotion::default();
        let mut frames = Vec::with_capacity(frame_count);
        let cue_clusters: Vec<Vec<FocusCluster>> = cues
            .iter()
            .map(|cue| build_focus_clusters_for_viewport(cue, capture, clips, pointer, focus_bounds))
            .collect();

        for frame_index in 0..frame_count {
            let editor_time = (frame_index as f64 * dt).min(duration);
            let source_time = clips.source_time_at(editor_time);
            let active = active_zoom_cue(source_time, cues, ZoomPresentation::Classic);
            let progress = active
                .map(|cue| cue.progress_at(source_time))
                .unwrap_or(0.0);
            let target_magnification = active
                .map(|cue| cue.magnification_at(progress))
                .unwrap_or(1.0);
            let raw_target = active
                .and_then(|cue| {
                    let base = cue_focus(cue, cues, &cue_clusters, editor_time);
                    Some(cue.anchor_at(base, progress))
                })
                .unwrap_or(NormalizedPoint { x: 0.5, y: 0.5 });
            if let Some(cue) = active {
                // Keep the outgoing response during release as well.
                camera_motion = cue.camera_motion;
                // Preserve the authored framing at the cue's full zoom. Using
                // the changing ramp amount here would move the pivot itself.
                held_framing = framing_for_focus(
                    raw_target,
                    cue.zoom,
                    cue.anchor_mode,
                    cue.bounds_bias,
                );
            }
            // Zoom-out keeps the outgoing focus; scale alone reveals the
            // full frame, without a second pan toward the middle.
            let target_framing = held_framing;
            let target_tilt = active.map(|cue| cue.tilt_at(progress)).unwrap_or_default();
            let cue_id = active.map(|cue| cue.id);
            let changed = cue_id != previous_cue;
            let instant_window = in_instant_window(source_time, cues);
            let should_snap = instant_window
                || (changed && previous_cue
                        .and_then(|id| cues.iter().find(|cue| cue.id == id))
                        .is_some_and(|cue| cue.skips_easing));

            if should_snap {
                amount.snap(target_magnification);
                framing_x.snap(target_framing.x);
                framing_y.snap(target_framing.y);
                tilt_x.snap(target_tilt.x);
                tilt_y.snap(target_tilt.y);
                tilt_z.snap(target_tilt.z);
            } else {
                // At identity all framing coordinates show the full image,
                // so pre-aim without producing any visible movement.
                if amount.position <= 1.000_5 {
                    framing_x.snap(target_framing.x);
                    framing_y.snap(target_framing.y);
                } else if frame_index > 0 {
                    camera_motion.step(&mut framing_x, target_framing.x, dt);
                    camera_motion.step(&mut framing_y, target_framing.y, dt);
                }
                if frame_index > 0 {
                    camera_motion.step(&mut tilt_x, target_tilt.x, dt);
                    camera_motion.step(&mut tilt_y, target_tilt.y, dt);
                    camera_motion.step(&mut tilt_z, target_tilt.z, dt);
                    camera_motion.step(&mut amount, target_magnification, dt);
                }
            }

            if amount.position < 1.0 {
                // Stop at the full frame, including velocity, so spring
                // undershoot cannot cause a small second zoom after release.
                amount.snap(1.0);
            }
            let loupe_cue = active_zoom_cue(source_time, cues, ZoomPresentation::GlassLoupe);
            let loupe_target = loupe_cue.map(|cue| {
                let base = cue_focus(cue, cues, &cue_clusters, editor_time);
                cue.anchor_at(base, cue.progress_at(source_time))
            });
            let loupe = loupe_motion.advance(
                loupe_cue, loupe_target, source_time, if frame_index == 0 { 0.0 } else { dt }, instant_window,
            );
            frames.push(ViewportSample {
                magnification: amount.position,
                framing: NormalizedPoint {
                    x: framing_x.position.clamp(0.0, 1.0),
                    y: framing_y.position.clamp(0.0, 1.0),
                },
                tilt: Tilt {
                    x: tilt_x.position,
                    y: tilt_y.position,
                    z: tilt_z.position,
                },
                loupe,
            });
            previous_cue = cue_id;
        }

        Self { frames, duration, cues: cues.to_vec(), cue_clusters, focus_bounds, clips: clips.clone(), reframe: None }
    }

    pub fn focus_bounds(&self) -> (f64, f64) { self.focus_bounds }

    pub fn reframe(&self) -> Option<&Reframe> {
        self.reframe.as_deref()
    }

    /// Change only the fill window of an existing camera (cheap, for live
    /// slider drags); the key stays stale so the next sync rebuilds it.
    pub fn reshape_reframe(&mut self, fill: (f64, f64)) -> bool {
        let Some(reframe) = self.reframe.as_mut() else {
            return false;
        };
        std::sync::Arc::make_mut(reframe).fill = fill;
        true
    }

    /// Drop Fill's follow camera (Fit, or an unconstrained canvas).
    pub fn clear_reframe(&mut self) {
        self.reframe = None;
    }

    /// Build Fill's follow camera for a `fill` window inside `crop`, chasing
    /// `focus` (source-normalized, usually the smoothed pointer).
    pub fn set_reframe(
        &mut self,
        fill: (f64, f64),
        crop: crate::CropRect,
        focus: impl Fn(f64) -> Option<NormalizedPoint>,
    ) {
        let crop = crop.validated();
        let (cx, cy, cw, ch) = (crop.x as f64, crop.y as f64, crop.width as f64, crop.height as f64);
        let to_crop = |p: NormalizedPoint| NormalizedPoint { x: (p.x - cx) / cw, y: (p.y - cy) / ch };
        let steps = ((self.duration.max(0.0) * STEP_RATE).round() as usize + 1).max(2);
        let smoothing = 1.0 - (-(1.0 / STEP_RATE) / Reframe::SMOOTHING_TAU).exp();
        let mut center = focus(0.0).map(to_crop).unwrap_or(NormalizedPoint { x: 0.5, y: 0.5 });
        let mut anchors = Vec::with_capacity(steps);
        for step in 0..steps {
            let time = step as f64 / STEP_RATE;
            let base = self.base_frame_at(time);
            let magnification = base.magnification.max(1.0);
            let visible_x = (fill.0 / magnification).min(1.0);
            let visible_y = (fill.1 / magnification).min(1.0);
            let zoom_anchor = to_crop(base.anchor);
            let point = focus(time).map(to_crop).unwrap_or(zoom_anchor);
            let (window_x, window_y) = (visible_x * Reframe::DEAD_ZONE, visible_y * Reframe::DEAD_ZONE);
            let mut target = center;
            if point.x > center.x + window_x {
                target.x = point.x - window_x;
            } else if point.x < center.x - window_x {
                target.x = point.x + window_x;
            }
            if point.y > center.y + window_y {
                target.y = point.y - window_y;
            } else if point.y < center.y - window_y {
                target.y = point.y + window_y;
            }
            center.x += (target.x - center.x) * smoothing;
            center.y += (target.y - center.y) * smoothing;
            let handoff = ((magnification - 1.0) / Reframe::ZOOM_HANDOFF).clamp(0.0, 1.0);
            let mut anchor = NormalizedPoint {
                x: center.x + (zoom_anchor.x - center.x) * handoff,
                y: center.y + (zoom_anchor.y - center.y) * handoff,
            };
            anchor.x = if visible_x >= 1.0 { 0.5 } else { anchor.x.clamp(visible_x * 0.5, 1.0 - visible_x * 0.5) };
            anchor.y = if visible_y >= 1.0 { 0.5 } else { anchor.y.clamp(visible_y * 0.5, 1.0 - visible_y * 0.5) };
            // Stay continuous through zoom handoffs.
            center = anchor;
            anchors.push(NormalizedPoint { x: cx + anchor.x * cw, y: cy + anchor.y * ch });
        }
        self.reframe = Some(std::sync::Arc::new(Reframe {
            fill,
            key: ReframeKey { fill, crop },
            anchors,
        }));
    }

    pub fn frame_at(&self, time: f64) -> ViewportFrame {
        let mut frame = self.base_frame_at(time);
        // Instant regions are evaluated at the requested source timestamp,
        // not interpolated across a baked sample's entry/exit boundary.
        let source_time = self.clips.source_time_at(time.clamp(0.0, self.duration.max(0.0)));
        for presentation in [ZoomPresentation::Classic, ZoomPresentation::GlassLoupe] {
            let Some(cue) = active_zoom_cue(source_time, &self.cues, presentation)
                .filter(|cue| cue.skips_easing || in_instant_window(source_time, &self.cues)) else { continue; };
            let progress = cue.progress_at(source_time);
            let base = cue_focus(cue, &self.cues, &self.cue_clusters, time);
            let center = cue.anchor_at(base, progress);
            if presentation == ZoomPresentation::Classic {
                let sample = ViewportSample {
                    magnification: cue.magnification_at(progress),
                    framing: framing_for_focus(center, cue.zoom, cue.anchor_mode, cue.bounds_bias),
                    tilt: cue.tilt_at(progress), loupe: frame.loupe,
                }.frame(frame.transform);
                frame.magnification = sample.magnification;
                frame.anchor = sample.anchor;
                frame.tilt = sample.tilt;
            } else {
                let options = cue.loupe.clamped();
                frame.loupe = Some(LoupeFrame {
                    center: loupe_drift(center, source_time), radius: options.radius,
                    magnification: cue.magnification_at(progress), presence: 1.0,
                    bevel: if options.glass { options.bevel } else { 0.0 },
                    chromatic_aberration: if options.glass { options.chromatic_aberration } else { 0.0 },
                });
            }
        }
        if let Some(reframe) = self.reframe.as_deref() {
            frame.fill = reframe.fill;
            if let Some(anchor) = reframe.anchor_at(time.clamp(0.0, self.duration.max(0.0))) {
                frame.anchor = anchor;
            }
        }
        frame
    }

    fn base_frame_at(&self, time: f64) -> ViewportFrame {
        // Evaluate authored transforms at the requested time. Interpolating
        // cached frames across a cue boundary blends with the reset pose and
        // makes the image flash back before the animation has actually ended.
        let source_time = self.clips.source_time_at(time.clamp(0.0, self.duration));
        let transform = transform_at(source_time, &self.cues);
        let Some(first) = self.frames.first().copied() else {
            return ViewportFrame::default();
        };
        if self.frames.len() == 1 || self.duration <= 0.0 {
            return first.frame(transform);
        }
        let position = time.clamp(0.0, self.duration) * STEP_RATE;
        let index = position as usize;
        if index >= self.frames.len() - 1 {
            return self.frames.last().unwrap_or(&first).frame(transform);
        }
        let fraction = position - index as f64;
        let left = self.frames[index];
        let right = self.frames[index + 1];
        let left_time = self.clips.source_time_at(index as f64 / STEP_RATE);
        let right_time = self.clips.source_time_at(((index + 1) as f64 / STEP_RATE).min(self.duration));
        let boundary_fraction = |presentation| {
            let a = active_zoom_cue(left_time, &self.cues, presentation);
            let b = active_zoom_cue(right_time, &self.cues, presentation);
            if a.map(|c| c.id) != b.map(|c| c.id)
                && (a.is_some_and(|c| c.skips_easing) || b.is_some_and(|c| c.skips_easing)) {
                let current = active_zoom_cue(source_time, &self.cues, presentation).map(|c| c.id);
                if current == b.map(|c| c.id) { 1.0 } else { 0.0 }
            } else { fraction }
        };
        let classic_fraction = boundary_fraction(ZoomPresentation::Classic);
        let loupe_fraction = boundary_fraction(ZoomPresentation::GlassLoupe);
        // Interpolate in framing space, then derive the source crop. Lerp of
        // crop centers would reintroduce pivot drift between cached samples.
        let left_framing = if left.magnification == 1.0 {
            right.framing
        } else {
            left.framing
        };
        let right_framing = if right.magnification == 1.0 {
            left_framing
        } else {
            right.framing
        };
        ViewportSample {
            magnification: lerp(left.magnification, right.magnification, classic_fraction),
            framing: NormalizedPoint {
                x: lerp(left_framing.x, right_framing.x, classic_fraction),
                y: lerp(left_framing.y, right_framing.y, classic_fraction),
            },
            tilt: Tilt {
                x: lerp(left.tilt.x, right.tilt.x, classic_fraction),
                y: lerp(left.tilt.y, right.tilt.y, classic_fraction),
                z: lerp(left.tilt.z, right.tilt.z, classic_fraction),
            },
            loupe: interpolate_loupe(left.loupe, right.loupe, loupe_fraction),
        }
        .frame(transform)
    }
}

#[derive(Clone, Copy, Debug)]
struct FocusCluster {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    editor_start: f64,
    weighted_x: f64,
    weighted_y: f64,
    weight: f64,
    last_point: NormalizedPoint,
    last_time: f64,
}

impl FocusCluster {
    fn new(point: NormalizedPoint, editor_start: f64) -> Self {
        Self {
            min_x: point.x,
            max_x: point.x,
            min_y: point.y,
            max_y: point.y,
            editor_start,
            weighted_x: 0.0,
            weighted_y: 0.0,
            weight: 0.0,
            last_point: point,
            last_time: editor_start,
        }
    }

    fn can_add(self, point: NormalizedPoint, max_width: f64, max_height: f64) -> bool {
        self.max_x.max(point.x) - self.min_x.min(point.x) <= max_width
            && self.max_y.max(point.y) - self.min_y.min(point.y) <= max_height
    }

    fn finish(&mut self, editor_end: f64) {
        let weight = (editor_end - self.last_time).max(0.0);
        self.weighted_x += self.last_point.x * weight;
        self.weighted_y += self.last_point.y * weight;
        self.weight += weight;
        self.last_time = editor_end;
    }

    fn add(&mut self, point: NormalizedPoint, editor_time: f64) {
        self.finish(editor_time);
        self.last_point = point;
        self.min_x = self.min_x.min(point.x);
        self.max_x = self.max_x.max(point.x);
        self.min_y = self.min_y.min(point.y);
        self.max_y = self.max_y.max(point.y);
    }

    fn center(self) -> NormalizedPoint {
        if self.weight <= f64::EPSILON { return self.last_point; }
        NormalizedPoint { x: self.weighted_x / self.weight, y: self.weighted_y / self.weight }
    }
}

#[cfg(test)]
fn build_focus_clusters(
    cue: &ZoomCue,
    capture: &PointerCaptureFile,
    clips: &RecordingClipTimeline,
    pointer: &PointerTimeline,
) -> Vec<FocusCluster> {
    build_focus_clusters_for_viewport(cue, capture, clips, pointer, (1.0, 1.0))
}

fn build_focus_clusters_for_viewport(
    cue: &ZoomCue,
    capture: &PointerCaptureFile,
    clips: &RecordingClipTimeline,
    pointer: &PointerTimeline,
    visible_source: (f64, f64),
) -> Vec<FocusCluster> {
    if cue.anchor_mode == ZoomAnchorMode::PinnedAnchor {
        return Vec::new();
    }
    let max_width = CLUSTER_WIDTH_RATIO * visible_source.0 / cue.zoom.max(1.0);
    let max_height = CLUSTER_HEIGHT_RATIO * visible_source.1 / cue.zoom.max(1.0);
    // Keep click/drag events and remove sub-pixel travel noise before grouping.
    // Centres are weighted by time spent at each position, so brief excursions
    // do not outweigh the place where the cursor rests.
    let mut source: Vec<_> = capture.travel.iter().map(|sample| {
        (sample.time, NormalizedPoint { x: sample.x, y: sample.y },
            sample.kind == super::model::PointerTravelKind::Drag)
    }).chain(capture.presses.iter().map(|press| {
        (press.time, NormalizedPoint { x: press.x, y: press.y }, true)
    })).filter(|(time, point, _)| time.is_finite() && point.x.is_finite() && point.y.is_finite())
        .collect();
    source.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (width, height) = pointer.recording_size();
    let mut previous = None;
    source.retain(|(_, point, important)| {
        let retain = *important || width <= 0.0 || height <= 0.0 || previous.is_none_or(|last: NormalizedPoint| {
            ((point.x - last.x) * width).hypot((point.y - last.y) * height) >= 10.0
        });
        if retain { previous = Some(*point); }
        retain
    });
    let mut samples = Vec::new();
    let slices = clips.slices_overlapping(cue.start, cue.end);
    for slice in &slices {
        if let Some((_, point, _)) = source.iter().rev().find(|(time, _, _)| *time <= slice.source_start) {
            samples.push((slice.editor_start, point.clamped()));
        }
        for (time, point, _) in &source {
            if *time > slice.source_start && *time <= slice.source_end {
                let t = (*time - slice.source_start) / (slice.source_end - slice.source_start).max(1e-9);
                samples.push((lerp(slice.editor_start, slice.editor_end, t), point.clamped()));
            }
        }
    }
    samples.sort_by(|left, right| left.0.total_cmp(&right.0));

    if samples.is_empty() {
        let editor_start = clips.editor_time_for_source(cue.start).unwrap_or(0.0);
        let point = pointer
            .location_at(editor_start)
            .unwrap_or(cue.pinned_point)
            .clamped();
        return vec![FocusCluster::new(point, editor_start)];
    }

    let mut clusters = Vec::new();
    let mut current = FocusCluster::new(samples[0].1, samples[0].0);
    for (editor_time, point) in samples.into_iter().skip(1) {
        if current.can_add(point, max_width, max_height) {
            current.add(point, editor_time);
        } else {
            current.finish(editor_time);
            clusters.push(current);
            current = FocusCluster::new(point, editor_time);
        }
    }
    current.finish(slices.last().map(|slice| slice.editor_end).unwrap_or(current.last_time + 0.1));
    clusters.push(current);
    clusters
}

fn cluster_center_at(clusters: &[FocusCluster], editor_time: f64) -> Option<NormalizedPoint> {
    clusters
        .iter()
        .rev()
        .find(|cluster| cluster.editor_start <= editor_time)
        .or_else(|| clusters.first())
        .copied()
        .map(FocusCluster::center)
}

/// Card motion is independent of camera zoom. A later transform replaces a
/// held pose; a completed reset region must not resurrect an older held pose.
pub fn transform_at(time: f64, cues: &[ZoomCue]) -> SceneTransform {
    let transforms = || cues.iter().filter(|cue| cue.is_enabled && cue.transform.is_some());
    let active = transforms()
        .filter(|cue| time >= cue.start && time <= cue.end)
        .max_by(|left, right| cue_priority(left).cmp(&cue_priority(right))
            .then_with(|| left.start.total_cmp(&right.start)));
    if let Some(cue) = active {
        return cue.transform.unwrap().at(cue.easing.apply(cue.progress_at(time)));
    }
    transforms()
        .filter(|cue| time > cue.end)
        .max_by(|left, right| left.start.total_cmp(&right.start))
        .and_then(|cue| cue.transform)
        .filter(|motion| motion.keep_end_state)
        .map(|motion| motion.end.clamped())
        .unwrap_or(SceneTransform::IDENTITY)
}

fn active_cue(time: f64, cues: &[ZoomCue]) -> Option<&ZoomCue> {
    cues.iter()
        .filter(|cue| cue.is_enabled && time >= cue.start && time <= cue.end)
        .max_by(|left, right| {
            cue_priority(left)
                .cmp(&cue_priority(right))
                .then_with(|| left.start.total_cmp(&right.start))
        })
}

fn active_zoom_cue(
    time: f64,
    cues: &[ZoomCue],
    presentation: ZoomPresentation,
) -> Option<&ZoomCue> {
    let eligible = |cue: &&ZoomCue| cue.is_enabled && cue.presentation == presentation;
    let active = cues.iter().filter(eligible)
        .filter(|cue| time >= cue.start && time <= cue.end)
        .max_by(|a, b| cue_priority(a).cmp(&cue_priority(b))
            .then_with(|| a.start.total_cmp(&b.start)));
    if active.is_some() || active_cue(time, cues).is_some() {
        return active;
    }
    cues.iter().filter(eligible)
        .filter(|cue| cue.start_animation_early && !cue.skips_easing
            && time >= (cue.start - 0.35).max(0.0) && time < cue.start)
        .min_by(|a, b| a.start.total_cmp(&b.start))
}

fn cue_focus(
    cue: &ZoomCue,
    cues: &[ZoomCue],
    clusters: &[Vec<FocusCluster>],
    editor_time: f64,
) -> NormalizedPoint {
    if cue.anchor_mode == ZoomAnchorMode::PinnedAnchor {
        return cue.pinned_point;
    }
    cues.iter().position(|candidate| candidate.id == cue.id)
        .and_then(|index| cluster_center_at(&clusters[index], editor_time))
        .unwrap_or(cue.pinned_point)
}

fn interpolate_loupe(a: Option<LoupeFrame>, b: Option<LoupeFrame>, t: f64) -> Option<LoupeFrame> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.interpolate(b, t)),
        (Some(a), None) => Some(a.interpolate(LoupeFrame { presence: 0.0, magnification: 1.0, ..a }, t)),
        (None, Some(b)) => Some(LoupeFrame { presence: 0.0, magnification: 1.0, ..b }.interpolate(b, t)),
        (None, None) => None,
    }
}

fn loupe_drift(mut center: NormalizedPoint, time: f64) -> NormalizedPoint {
    center.x += (time * std::f64::consts::TAU / 12.0).sin() * 0.0125;
    center.y += ((time + 7.5) * std::f64::consts::TAU / 18.0).cos() * 0.0125;
    center
}

fn in_instant_window(time: f64, cues: &[ZoomCue]) -> bool {
    cues.iter().any(|cue| cue.is_enabled && cue.skips_easing
        && time >= cue.start && time <= cue.end + 0.05)
}

struct LoupeMotion {
    x: DampedSpring,
    y: DampedSpring,
    radius: DampedSpring,
    amount: DampedSpring,
    presence: DampedSpring,
    bevel: DampedSpring,
    chroma: DampedSpring,
    response: CameraMotion,
    previous: Option<(Uuid, bool)>,
}

impl Default for LoupeMotion {
    fn default() -> Self {
        Self {
            x: DampedSpring::new(0.5), y: DampedSpring::new(0.5),
            radius: DampedSpring::new(0.35), amount: DampedSpring::new(1.0),
            presence: DampedSpring::new(0.0), bevel: DampedSpring::new(0.0),
            chroma: DampedSpring::new(0.0), response: CameraMotion::default(), previous: None,
        }
    }
}

impl LoupeMotion {
    fn advance(
        &mut self,
        cue: Option<&ZoomCue>,
        center: Option<NormalizedPoint>,
        time: f64,
        dt: f64,
        instant_window: bool,
    ) -> Option<LoupeFrame> {
        let mut target_radius = self.radius.position;
        let mut target_center = NormalizedPoint { x: self.x.position, y: self.y.position };
        let (mut amount, mut presence, mut bevel, mut chroma) = (1.0, 0.0, 0.0, 0.0);
        if let Some(cue) = cue {
            let options = cue.loupe.clamped();
            self.response = cue.camera_motion;
            target_radius = options.radius;
            target_center = center.unwrap_or(cue.pinned_point).clamped();
            // A slow, small drift makes the glass feel suspended over the
            // source. It is based on source time, so seeking is deterministic.
            target_center = loupe_drift(target_center, time);
            amount = cue.magnification_at(cue.progress_at(time));
            presence = 1.0;
            if options.glass {
                bevel = options.bevel;
                chroma = options.chromatic_aberration;
            }
            if self.presence.position <= 0.0001 && self.previous.is_none() {
                self.x.snap(target_center.x);
                self.y.snap(target_center.y);
                self.radius.snap(target_radius);
            }
        }
        let identity = cue.map(|cue| (cue.id, cue.skips_easing));
        let instant = instant_window || identity.is_some_and(|(_, instant)| instant)
            || (identity != self.previous && self.previous.is_some_and(|(_, instant)| instant));
        let mut spring = self.response.spring();
        spring.tension *= 1.5;
        for (state, target) in [
            (&mut self.x, target_center.x), (&mut self.y, target_center.y),
            (&mut self.radius, target_radius), (&mut self.amount, amount),
            (&mut self.presence, presence), (&mut self.bevel, bevel), (&mut self.chroma, chroma),
        ] {
            if instant { state.snap(target); } else if dt > 0.0 { state.step(target, spring, dt); }
        }
        self.previous = identity;
        if self.presence.position < 0.0 {
            self.presence.snap(0.0);
            self.amount.snap(1.0);
        }
        (self.presence.position > 0.0001).then_some(LoupeFrame {
            center: NormalizedPoint { x: self.x.position, y: self.y.position },
            radius: self.radius.position.clamp(0.01, 0.75),
            magnification: self.amount.position.max(1.0),
            bevel: self.bevel.position.clamp(0.0, 0.5),
            chromatic_aberration: self.chroma.position.clamp(0.0, 1.0),
            presence: self.presence.position.clamp(0.0, 1.0),
        })
    }
}

fn cue_priority(cue: &ZoomCue) -> u8 {
    if cue.is_implicit {
        return 0;
    }
    match cue.anchor_mode {
        ZoomAnchorMode::PointerAnchor => 1,
        ZoomAnchorMode::SmartAnchor => 2,
        ZoomAnchorMode::PinnedAnchor => 3,
    }
}

fn framing_for_focus(
    point: NormalizedPoint,
    magnification: f64,
    mode: ZoomAnchorMode,
    bounds_bias: f64,
) -> NormalizedPoint {
    let half = 0.5 / magnification.max(1.0);
    let travel = 1.0 - 2.0 * half;
    if travel <= f64::EPSILON {
        return point.clamped();
    }
    let anchor = bounded_anchor(point, magnification, mode, bounds_bias);
    NormalizedPoint {
        x: ((anchor.x - half) / travel).clamp(0.0, 1.0),
        y: ((anchor.y - half) / travel).clamp(0.0, 1.0),
    }
}

fn bounded_anchor(
    point: NormalizedPoint,
    magnification: f64,
    mode: ZoomAnchorMode,
    bounds_bias: f64,
) -> NormalizedPoint {
    let point = point.clamped();
    if mode == ZoomAnchorMode::PinnedAnchor {
        return clamp_to_frame(point, magnification);
    }
    let half_extent = 1.0 / (2.0 * magnification.max(1.0));
    let preserving = NormalizedPoint {
        x: half_extent + point.x * (1.0 - 2.0 * half_extent),
        y: half_extent + point.y * (1.0 - 2.0 * half_extent),
    };
    let bias = bounds_bias.clamp(0.0, 1.0);
    clamp_to_frame(
        NormalizedPoint {
            x: lerp(point.x, preserving.x, bias),
            y: lerp(point.y, preserving.y, bias),
        },
        magnification,
    )
}

fn clamp_to_frame(point: NormalizedPoint, magnification: f64) -> NormalizedPoint {
    let half_extent = 1.0 / (2.0 * magnification.max(1.0));
    NormalizedPoint {
        x: point.x.clamp(half_extent, 1.0 - half_extent),
        y: point.y.clamp(half_extent, 1.0 - half_extent),
    }
}

fn lerp(left: f64, right: f64, fraction: f64) -> f64 {
    left + (right - left) * fraction.clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ZoomAnchorMode {
    #[default]
    PointerAnchor,
    SmartAnchor,
    PinnedAnchor,
}

/// How a motion region's magnification evolves across its time range.
///
/// `Hold` is the classic click zoom: the camera springs to the target at the
/// region start and springs back when it ends. `ZoomIn`/`ZoomOut` ramp the
/// target across the whole region for slow, cinematic moves.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MotionStyle {
    #[default]
    Hold,
    ZoomIn,
    ZoomOut,
}

impl MotionStyle {
    pub const ALL: [MotionStyle; 3] =
        [MotionStyle::Hold, MotionStyle::ZoomIn, MotionStyle::ZoomOut];

    pub fn label(self) -> &'static str {
        match self {
            MotionStyle::Hold => "Hold",
            MotionStyle::ZoomIn => "Zoom in",
            MotionStyle::ZoomOut => "Zoom out",
        }
    }
}

/// Shape of the zoom/pan ramps inside a region. `Hold` regions always use
/// the camera spring; easing only affects animated ramps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MotionEasing {
    #[default]
    Smooth,
    Linear,
    Snappy,
    Cinematic,
}

impl MotionEasing {
    pub const ALL: [MotionEasing; 4] = [
        MotionEasing::Smooth,
        MotionEasing::Linear,
        MotionEasing::Snappy,
        MotionEasing::Cinematic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MotionEasing::Smooth => "Smooth",
            MotionEasing::Linear => "Linear",
            MotionEasing::Snappy => "Snappy",
            MotionEasing::Cinematic => "Cinematic",
        }
    }

    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            MotionEasing::Smooth => t * t * (3.0 - 2.0 * t),
            MotionEasing::Linear => t,
            MotionEasing::Snappy => 1.0 - (1.0 - t).powi(4),
            MotionEasing::Cinematic => {
                if t < 0.5 {
                    16.0 * t.powi(5)
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(5) / 2.0
                }
            }
        }
    }
}

/// Card transforms are relative to the scene's authored placement. Outside
/// the region the card either keeps its end pose or returns to layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TransformMotion {
    pub start: SceneTransform,
    pub end: SceneTransform,
    /// False for legacy projects and presets; new manual transforms opt in.
    pub keep_end_state: bool,
}

impl TransformMotion {
    pub fn at(self, progress: f64) -> SceneTransform {
        let start = self.start.clamped();
        let end = self.end.clamped();
        SceneTransform {
            scale: lerp(start.scale, end.scale, progress),
            position_x: lerp(start.position_x, end.position_x, progress),
            position_y: lerp(start.position_y, end.position_y, progress),
            rotation_x: lerp(start.rotation_x, end.rotation_x, progress),
            rotation_y: lerp(start.rotation_y, end.rotation_y, progress),
            rotation_z: lerp(start.rotation_z, end.rotation_z, progress),
            ..SceneTransform::IDENTITY
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoomCue {
    pub id: Uuid,
    pub start: f64,
    pub end: f64,
    pub zoom: f64,
    pub anchor_mode: ZoomAnchorMode,
    pub pinned_point: NormalizedPoint,
    pub bounds_bias: f64,
    pub is_enabled: bool,
    pub is_implicit: bool,
    pub skips_easing: bool,
    #[serde(default)]
    pub presentation: ZoomPresentation,
    #[serde(default)]
    pub loupe: LoupeOptions,
    /// Anticipate this region by 350ms, without changing its timeline bounds.
    #[serde(default)]
    pub start_animation_early: bool,
    /// Magnification envelope. Older projects omit it and behave as `Hold`.
    #[serde(default)]
    pub motion: MotionStyle,
    /// Optional pan destination: the anchor glides from its base target to
    /// this point across the region (Ken Burns style).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan_to: Option<NormalizedPoint>,
    /// Ramp shape for zoom in/out and pan.
    #[serde(default)]
    pub easing: MotionEasing,
    /// Response for entry, following, and release; absent in older projects.
    #[serde(default)]
    pub camera_motion: CameraMotion,
    /// Optional 3D tilt of the media surface while the region is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt: Option<Tilt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformMotion>,
}

impl ZoomCue {
    pub const MINIMUM_DURATION: f64 = 0.5;
    pub const MINIMUM_ZOOM: f64 = 1.0;
    pub const MAXIMUM_ZOOM: f64 = 4.0;

    /// A pinned, held region: the standard building block of manual motion.
    pub fn pinned(start: f64, end: f64, zoom: f64, point: NormalizedPoint) -> Self {
        Self {
            id: Uuid::new_v4(),
            start,
            end,
            zoom: zoom.clamp(Self::MINIMUM_ZOOM, Self::MAXIMUM_ZOOM),
            anchor_mode: ZoomAnchorMode::PinnedAnchor,
            pinned_point: point.clamped(),
            bounds_bias: 0.0,
            is_enabled: true,
            is_implicit: false,
            skips_easing: false,
            presentation: ZoomPresentation::Classic,
            loupe: LoupeOptions::default(),
            start_animation_early: false,
            motion: MotionStyle::Hold,
            pan_to: None,
            easing: MotionEasing::Smooth,
            camera_motion: CameraMotion::default(),
            tilt: None,
            transform: None,
        }
    }

    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    /// 0 at the region start, 1 at its end (in source time).
    pub fn progress_at(&self, source_time: f64) -> f64 {
        let duration = self.duration();
        if duration <= 0.0 {
            return 1.0;
        }
        ((source_time - self.start) / duration).clamp(0.0, 1.0)
    }

    /// Target magnification for the given progress through the region.
    pub fn magnification_at(&self, progress: f64) -> f64 {
        let zoom = self.zoom.max(1.0);
        match self.motion {
            MotionStyle::Hold => zoom,
            MotionStyle::ZoomIn => 1.0 + (zoom - 1.0) * self.easing.apply(progress),
            MotionStyle::ZoomOut => zoom - (zoom - 1.0) * self.easing.apply(progress),
        }
    }

    /// Tilt target while the region is active. Ramped regions ease the tilt
    /// in alongside the zoom so the card settles as the move completes.
    pub fn tilt_at(&self, progress: f64) -> Tilt {
        let Some(tilt) = self.tilt else {
            return Tilt::default();
        };
        let t = match self.motion {
            MotionStyle::Hold => 1.0,
            MotionStyle::ZoomIn => self.easing.apply(progress),
            MotionStyle::ZoomOut => 1.0 - self.easing.apply(progress),
        };
        Tilt {
            x: tilt.x * t,
            y: tilt.y * t,
            z: tilt.z * t,
        }
    }

    /// Target anchor for the given progress, applying an optional pan.
    pub fn anchor_at(&self, base: NormalizedPoint, progress: f64) -> NormalizedPoint {
        match self.pan_to {
            None => base,
            Some(destination) => {
                let t = self.easing.apply(progress);
                NormalizedPoint {
                    x: lerp(base.x, destination.x, t),
                    y: lerp(base.y, destination.y, t),
                }
                .clamped()
            }
        }
    }

    pub fn has_camera_motion(&self) -> bool {
        self.presentation == ZoomPresentation::GlassLoupe || self.transform.is_none()
            || self.zoom > 1.0 || self.pan_to.is_some() || self.tilt.is_some()
    }

    pub fn summary(&self) -> String {
        let motion = match self.motion {
            MotionStyle::Hold => "",
            MotionStyle::ZoomIn => "In ",
            MotionStyle::ZoomOut => "Out ",
        };
        let pan = if self.pan_to.is_some() { " Pan" } else { "" };
        let tilt = if self.tilt.is_some_and(|tilt| !tilt.is_zero()) {
            " 3D"
        } else {
            ""
        };
        let presentation = if self.presentation == ZoomPresentation::GlassLoupe { "Loupe " } else { "" };
        let mut label = format!("{presentation}{motion}{:.1}×{pan}{tilt}", self.zoom);
        if let Some(transform) = self.transform {
            let a = transform.start;
            let b = transform.end;
            let mut properties = Vec::new();
            if a.scale != 1.0 || b.scale != 1.0 { properties.push("Scale"); }
            if a.position_x != 0.0 || b.position_x != 0.0 || a.position_y != 0.0 || b.position_y != 0.0 { properties.push("Move"); }
            if a.has_rotation() || b.has_rotation() { properties.push("Rotate"); }
            if properties.is_empty() { properties.push("Transform"); }
            if self.zoom == 1.0 && self.pan_to.is_none() && self.tilt.is_none() {
                label = properties.join(" + ");
            } else {
                label.push_str(&format!(" · {}", properties.join(" + ")));
            }
        }
        label
    }

    fn around_press(
        time: f64,
        point: NormalizedPoint,
        duration: f64,
        style: AutoZoomStyle,
    ) -> Option<Self> {
        const TRAILING_GUARD: f64 = 0.8;
        const EARLIEST_START: f64 = 0.001;

        let (lead, hold, zoom, camera_motion) = match style {
            AutoZoomStyle::Compact => (0.6, 1.6, 1.5, CameraMotion::Standard),
            AutoZoomStyle::Cinematic => (0.3, 2.5, 2.0, CameraMotion::Focused),
        };
        let start = (time - lead).max(EARLIEST_START);
        let end = (time + hold).min(duration - TRAILING_GUARD);
        (end > start).then(|| Self {
            id: Uuid::new_v4(),
            start,
            end,
            zoom,
            anchor_mode: ZoomAnchorMode::PointerAnchor,
            pinned_point: point.clamped(),
            bounds_bias: 0.25,
            is_enabled: true,
            is_implicit: false,
            skips_easing: false,
            presentation: ZoomPresentation::Classic,
            loupe: LoupeOptions::default(),
            start_animation_early: false,
            motion: MotionStyle::Hold,
            pan_to: None,
            easing: MotionEasing::Smooth,
            camera_motion,
            tilt: None,
            transform: None,
        })
    }
}

/// One-click motion recipes for animated screenshots and quick recording
/// polish. Every preset expands into ordinary editable regions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotionPreset {
    SlowZoomIn,
    SlowZoomOut,
    PanLeft,
    PanRight,
    FocusCenter,
    Sweep,
    Tilt3D,
    FloatingCard,
    CornerReveal,
    TiltedScroll,
}

impl MotionPreset {
    pub const ALL: [MotionPreset; 10] = [
        MotionPreset::SlowZoomIn,
        MotionPreset::SlowZoomOut,
        MotionPreset::PanLeft,
        MotionPreset::PanRight,
        MotionPreset::FocusCenter,
        MotionPreset::Sweep,
        MotionPreset::Tilt3D,
        MotionPreset::FloatingCard,
        MotionPreset::CornerReveal,
        MotionPreset::TiltedScroll,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MotionPreset::SlowZoomIn => "Slow zoom in",
            MotionPreset::SlowZoomOut => "Slow zoom out",
            MotionPreset::PanLeft => "Pan left",
            MotionPreset::PanRight => "Pan right",
            MotionPreset::FocusCenter => "Focus",
            MotionPreset::Sweep => "Sweep",
            MotionPreset::Tilt3D => "3D tilt",
            MotionPreset::FloatingCard => "Floating card",
            MotionPreset::CornerReveal => "Corner reveal",
            MotionPreset::TiltedScroll => "Tilted scroll",
        }
    }

    /// Regions covering a scene of `duration` seconds.
    pub fn cues(self, duration: f64) -> Vec<ZoomCue> {
        if !duration.is_finite() || duration < ZoomCue::MINIMUM_DURATION {
            return Vec::new();
        }
        let center = NormalizedPoint { x: 0.5, y: 0.5 };
        let lead = (duration * 0.06).min(0.4);
        match self {
            MotionPreset::SlowZoomIn => {
                let mut cue = ZoomCue::pinned(0.0, duration, 1.6, center);
                cue.motion = MotionStyle::ZoomIn;
                vec![cue]
            }
            MotionPreset::SlowZoomOut => {
                let mut cue = ZoomCue::pinned(0.0, duration, 1.6, center);
                cue.motion = MotionStyle::ZoomOut;
                vec![cue]
            }
            MotionPreset::PanLeft => {
                let mut cue =
                    ZoomCue::pinned(0.0, duration, 1.4, NormalizedPoint { x: 0.75, y: 0.5 });
                cue.pan_to = Some(NormalizedPoint { x: 0.25, y: 0.5 });
                cue.skips_easing = true;
                vec![cue]
            }
            MotionPreset::PanRight => {
                let mut cue =
                    ZoomCue::pinned(0.0, duration, 1.4, NormalizedPoint { x: 0.25, y: 0.5 });
                cue.pan_to = Some(NormalizedPoint { x: 0.75, y: 0.5 });
                cue.skips_easing = true;
                vec![cue]
            }
            MotionPreset::FocusCenter => {
                let start = (duration * 0.25).max(lead);
                let end = (duration * 0.8)
                    .max(start + ZoomCue::MINIMUM_DURATION)
                    .min(duration);
                vec![ZoomCue::pinned(start, end, 1.8, center)]
            }
            MotionPreset::Sweep => {
                let mut cue =
                    ZoomCue::pinned(0.0, duration, 1.7, NormalizedPoint { x: 0.3, y: 0.3 });
                cue.pan_to = Some(NormalizedPoint { x: 0.7, y: 0.7 });
                cue.skips_easing = true;
                vec![cue]
            }
            MotionPreset::Tilt3D => {
                // Start tilted away, settle flat while easing in slightly.
                let mut cue = ZoomCue::pinned(0.0, duration, 1.15, center);
                cue.motion = MotionStyle::ZoomOut;
                cue.easing = MotionEasing::Cinematic;
                cue.tilt = Some(Tilt {
                    x: 10.0,
                    y: -24.0,
                    z: 0.0,
                });
                cue.skips_easing = true;
                vec![cue]
            }
            MotionPreset::FloatingCard => {
                // Alternate gentle tilts so the card appears to float.
                let segments = ((duration / 2.5).round() as usize).clamp(2, 6);
                let step = duration / segments as f64;
                (0..segments)
                    .map(|index| {
                        let sign = if index % 2 == 0 { 1.0 } else { -1.0 };
                        let start = index as f64 * step;
                        let end = if index + 1 == segments {
                            duration
                        } else {
                            start + step
                        };
                        let mut cue = ZoomCue::pinned(start, end, 1.08, center);
                        cue.tilt = Some(Tilt {
                            x: 4.0 * sign,
                            y: 9.0 * sign,
                            z: 1.5 * sign,
                        });
                        cue
                    })
                    .collect()
            }
            MotionPreset::CornerReveal => {
                // Open tight on the top-left corner, hold, then pull back to
                // reveal the whole frame while gliding to the centre.
                let corner = NormalizedPoint { x: 0.22, y: 0.22 };
                let hold_end = (duration * 0.3).max(ZoomCue::MINIMUM_DURATION);
                let hold_end = hold_end.min(duration - ZoomCue::MINIMUM_DURATION);
                if hold_end <= 0.0 {
                    let mut cue = ZoomCue::pinned(0.0, duration, 2.2, corner);
                    cue.motion = MotionStyle::ZoomOut;
                    cue.easing = MotionEasing::Cinematic;
                    cue.pan_to = Some(center);
                    cue.skips_easing = true;
                    return vec![cue];
                }
                let mut hold = ZoomCue::pinned(0.0, hold_end, 2.2, corner);
                hold.skips_easing = true;
                let mut reveal = ZoomCue::pinned(hold_end, duration, 2.2, corner);
                reveal.motion = MotionStyle::ZoomOut;
                reveal.easing = MotionEasing::Cinematic;
                reveal.pan_to = Some(center);
                reveal.skips_easing = true;
                vec![hold, reveal]
            }
            MotionPreset::TiltedScroll => {
                // A window turned in perspective while the camera scrolls
                // down its content.
                let mut cue =
                    ZoomCue::pinned(0.0, duration, 1.5, NormalizedPoint { x: 0.5, y: 0.2 });
                cue.pan_to = Some(NormalizedPoint { x: 0.5, y: 0.8 });
                cue.tilt = Some(Tilt {
                    x: 8.0,
                    y: 22.0,
                    z: 0.0,
                });
                cue.skips_easing = true;
                vec![cue]
            }
        }
    }
}

/// Down-clicks create editable cues using the compact activity recipe.
/// Short gaps merge while separated interactions let the camera zoom out.
pub fn synthesize_zoom_cues(capture: &PointerCaptureFile, duration: f64) -> Vec<ZoomCue> {
    synthesize_zoom_cues_with_style(capture, duration, AutoZoomStyle::Compact)
}

/// Generation recipes expand into ordinary editable cues. They do not change
/// saved regions or the default behavior of existing capture callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoZoomStyle {
    /// Short holds and the existing default response.
    Compact,
    /// Longer activity groups with closer framing and a focused response.
    Cinematic,
}

pub fn synthesize_zoom_cues_with_style(
    capture: &PointerCaptureFile,
    duration: f64,
    style: AutoZoomStyle,
) -> Vec<ZoomCue> {
    const TAIL_EXCLUSION: f64 = 1.0;
    let join_tolerance = match style {
        AutoZoomStyle::Compact => 0.6,
        AutoZoomStyle::Cinematic => 2.5,
    };

    if !duration.is_finite() || duration <= 0.0 {
        return Vec::new();
    }
    let latest_eligible_press = duration - TAIL_EXCLUSION;
    let mut candidates: Vec<_> = capture
        .presses
        .iter()
        .filter(|press| {
            press.phase == PressPhase::Down
                && press.time.is_finite()
                && press.time < latest_eligible_press
                && (0.0..=1.0).contains(&press.x)
                && (0.0..=1.0).contains(&press.y)
        })
        .filter_map(|press| {
            ZoomCue::around_press(
                press.time,
                NormalizedPoint {
                    x: press.x,
                    y: press.y,
                },
                duration,
                style,
            )
        })
        .collect();
    candidates.sort_by(|a, b| a.start.total_cmp(&b.start));

    let mut merged: Vec<ZoomCue> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if let Some(previous) = merged.last_mut() {
            if candidate.start <= previous.end + join_tolerance {
                previous.end = previous.end.max(candidate.end);
                continue;
            }
        }
        merged.push(candidate);
    }
    merged
}

/// Builds a synthetic pointer capture for an animated screenshot: the cursor
/// glides between the given stops (evenly spaced across `duration`) and
/// clicks at each one, so the usual click-zoom synthesis and cursor
/// reconstruction apply to a still image.
pub fn walkthrough_capture(stops: &[NormalizedPoint], duration: f64) -> PointerCaptureFile {
    use crate::recording::model::{PointerPressEvent, PointerTravelKind, PointerTravelSample};
    let mut capture = PointerCaptureFile::default();
    if stops.is_empty() || !duration.is_finite() || duration <= 0.0 {
        return capture;
    }
    const SAMPLE_RATE: f64 = 60.0;
    let stop_times: Vec<f64> = (0..stops.len())
        .map(|index| duration * (index as f64 + 1.0) / (stops.len() as f64 + 1.0))
        .collect();
    let sample = |time: f64, point: NormalizedPoint| PointerTravelSample {
        time,
        x: point.x,
        y: point.y,
        kind: PointerTravelKind::Move,
        artwork_id: None,
    };
    // Appear a little before the first stop, already heading toward it.
    let lead_in = (stop_times[0] * 0.6).max(0.15);
    let entry = NormalizedPoint {
        x: (stops[0].x - 0.08).clamp(0.0, 1.0),
        y: (stops[0].y + 0.10).clamp(0.0, 1.0),
    };
    let mut segments: Vec<(f64, NormalizedPoint, f64, NormalizedPoint)> =
        vec![(stop_times[0] - lead_in, entry, stop_times[0], stops[0])];
    for index in 1..stops.len() {
        // Dwell on the previous stop, then travel to the next one.
        let dwell = (stop_times[index] - stop_times[index - 1]) * 0.45;
        segments.push((
            stop_times[index - 1] + dwell,
            stops[index - 1],
            stop_times[index],
            stops[index],
        ));
    }
    for (start, from, end, to) in segments {
        let steps = ((end - start) * SAMPLE_RATE).ceil().max(2.0) as usize;
        for step in 0..=steps {
            let t = step as f64 / steps as f64;
            let eased = t * t * (3.0 - 2.0 * t);
            capture.travel.push(sample(
                start + (end - start) * t,
                NormalizedPoint {
                    x: lerp(from.x, to.x, eased),
                    y: lerp(from.y, to.y, eased),
                }
                .clamped(),
            ));
        }
    }
    for (index, stop) in stops.iter().enumerate() {
        for (phase, offset) in [(PressPhase::Down, 0.0), (PressPhase::Up, 0.09)] {
            capture.presses.push(PointerPressEvent {
                time: (stop_times[index] + offset).min(duration),
                x: stop.x,
                y: stop.y,
                button: 0,
                phase,
                artwork_id: None,
            });
        }
    }
    capture.is_sanitized = true;
    capture
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::model::{
        PointerCaptureFile, PointerPressEvent, PointerTravelKind, PointerTravelSample,
    };
    use crate::recording::pointer_timeline::PointerTimelineOptions;

    #[test]
    fn card_transform_animates_endpoints_with_easing_and_random_seeks() {
        let mut cue = ZoomCue::pinned(1.0, 3.0, 1.0, NormalizedPoint { x: 0.5, y: 0.5 });
        let motion = TransformMotion {
            keep_end_state: false,
            start: SceneTransform { scale: 0.8, rotation_z: -20.0, ..SceneTransform::IDENTITY },
            end: SceneTransform {
                scale: 1.4,
                position_x: 0.4,
                position_y: -0.2,
                rotation_x: 30.0,
                rotation_y: -40.0,
                rotation_z: 60.0,
                ..SceneTransform::IDENTITY
            },
        };
        cue.transform = Some(motion);
        for easing in MotionEasing::ALL {
            cue.easing = easing;
            let timeline = ViewportTimeline::build_static(&[cue.clone()], 4.0);
            for time in [3.0, 1.0, 2.0, 1.5, 2.5, 0.0, 4.0, 1.5] {
                let frame = timeline.frame_at(time);
                let expected = if (1.0..=3.0).contains(&time) {
                    motion.at(easing.apply((time - 1.0) / 2.0))
                } else { SceneTransform::IDENTITY };
                assert_eq!(frame.transform, expected);
                assert_eq!(frame.magnification, 1.0, "card scaling must not crop the source");
            }
        }
        cue.is_enabled = false;
        assert_eq!(ViewportTimeline::build_static(&[cue], 4.0).frame_at(2.0).transform, SceneTransform::IDENTITY);
    }

    #[test]
    fn transform_boundaries_do_not_blend_with_reset_or_neighboring_cues() {
        let mut cue = ZoomCue::pinned(0.013, 2.473, 1.0, NormalizedPoint { x: 0.5, y: 0.5 });
        let pose = SceneTransform { position_x: 6.0, scale: 0.8, ..SceneTransform::IDENTITY };
        cue.transform = Some(TransformMotion { start: pose, end: pose, ..Default::default() });
        let mut next = cue.clone();
        next.id = Uuid::new_v4();
        next.start = cue.end;
        next.end = 3.01;
        let next_pose = SceneTransform { position_x: -6.0, ..SceneTransform::IDENTITY };
        next.transform = Some(TransformMotion { start: next_pose, end: next_pose, ..Default::default() });
        for cues in [vec![cue.clone()], vec![cue.clone(), next]] {
            let timeline = ViewportTimeline::build_static(&cues, 4.0);
            // Non-grid-aligned boundaries, in random seek order.
            for time in [2.472999, 0.013, 2.47, 0.013001] {
                assert_eq!(timeline.frame_at(time).transform, pose);
            }
            assert_eq!(timeline.frame_at(0.012999).transform, SceneTransform::IDENTITY);
            let after = if cues.len() == 1 { SceneTransform::IDENTITY } else { next_pose };
            assert_eq!(timeline.frame_at(2.473001).transform, after);
        }
    }

    #[test]
    fn held_pose_survives_camera_motion_and_is_replaced_by_later_transforms() {
        let pose = SceneTransform { position_x: 1.5, scale: 0.8, rotation_z: 20.0, ..SceneTransform::IDENTITY };
        let mut first = ZoomCue::pinned(0.0, 1.0, 1.0, NormalizedPoint { x: 0.5, y: 0.5 });
        first.transform = Some(TransformMotion { end: pose, keep_end_state: true, ..Default::default() });
        let camera = ZoomCue::pinned(1.5, 2.5, 2.0, NormalizedPoint { x: 0.5, y: 0.5 });
        let mut later = ZoomCue::pinned(3.0, 4.0, 1.0, NormalizedPoint { x: 0.5, y: 0.5 });
        later.transform = Some(TransformMotion { start: pose, end: SceneTransform::IDENTITY, ..Default::default() });
        let mut cues = vec![first, camera, later];
        let timeline = ViewportTimeline::build_static(&cues, 6.0);
        for time in [2.0, 1.0001, 3.0, 2.99] {
            assert_eq!(timeline.frame_at(time).transform, pose);
        }
        assert_eq!(timeline.frame_at(4.001).transform, SceneTransform::IDENTITY);
        assert_eq!(timeline.frame_at(5.0).transform, SceneTransform::IDENTITY);
        cues[2].is_enabled = false;
        assert_eq!(ViewportTimeline::build_static(&cues, 6.0).frame_at(5.0).transform, pose);
        cues[0].transform.as_mut().unwrap().keep_end_state = false;
        assert_eq!(ViewportTimeline::build_static(&cues, 6.0).frame_at(2.0).transform, SceneTransform::IDENTITY);
    }

    #[test]
    fn legacy_transform_json_resets_and_keep_end_state_round_trips() {
        let legacy: TransformMotion = serde_json::from_str(r#"{"start":{},"end":{}}"#).unwrap();
        assert!(!legacy.keep_end_state);
        let held = TransformMotion { keep_end_state: true, ..legacy };
        let json = serde_json::to_string(&held).unwrap();
        assert_eq!(serde_json::from_str::<TransformMotion>(&json).unwrap(), held);
    }

    #[test]
    fn transform_motion_round_trips_and_uses_edited_clip_time() {
        let mut cue = ZoomCue::pinned(0.0, 4.0, 1.0, NormalizedPoint { x: 0.5, y: 0.5 });
        cue.easing = MotionEasing::Linear;
        cue.transform = Some(TransformMotion {
            end: SceneTransform { scale: 2.0, ..SceneTransform::IDENTITY },
            ..TransformMotion::default()
        });
        let restored: ZoomCue = serde_json::from_str(&serde_json::to_string(&cue).unwrap()).unwrap();
        assert_eq!(restored, cue);
        let mut clips = RecordingClipTimeline::full(4.0);
        clips.segments[0].speed = 2.0;
        let timeline = ViewportTimeline::build(&[restored], &PointerTimeline::default(), &clips, &PointerCaptureFile::default());
        assert!((timeline.frame_at(1.0).transform.scale - 1.5).abs() < 1e-9);
    }

    #[test]
    fn walkthrough_capture_visits_stops_in_order_with_clicks() {
        let stops = [
            NormalizedPoint { x: 0.2, y: 0.3 },
            NormalizedPoint { x: 0.8, y: 0.4 },
            NormalizedPoint { x: 0.5, y: 0.8 },
        ];
        let capture = walkthrough_capture(&stops, 6.0);
        let downs: Vec<_> = capture
            .presses
            .iter()
            .filter(|press| press.phase == PressPhase::Down)
            .collect();
        assert_eq!(downs.len(), 3);
        assert!((downs[0].time - 1.5).abs() < 1e-9 && (downs[2].time - 4.5).abs() < 1e-9);
        assert!(capture
            .travel
            .windows(2)
            .all(|pair| pair[1].time >= pair[0].time));
        let at_second_stop = capture
            .travel
            .iter()
            .min_by(|a, b| (a.time - 3.0).abs().total_cmp(&(b.time - 3.0).abs()))
            .unwrap();
        assert!((at_second_stop.x - 0.8).abs() < 0.02 && (at_second_stop.y - 0.4).abs() < 0.02);
        let cues = synthesize_zoom_cues(&capture, 6.0);
        assert!(!cues.is_empty());
        assert!(walkthrough_capture(&[], 6.0).travel.is_empty());
    }

    #[test]
    #[ignore = "set LAHZA_ZOOM_RECORDING to a recording package to print its automatic zoom camera"]
    fn real_recording_auto_zoom_camera_report() {
        let root = std::path::PathBuf::from(
            std::env::var_os("LAHZA_ZOOM_RECORDING").expect("recording package"),
        );
        let capture: PointerCaptureFile =
            serde_json::from_slice(&std::fs::read(root.join("input.json")).unwrap()).unwrap();
        let meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("capture.json")).unwrap()).unwrap();
        let duration = meta["duration"].as_f64().unwrap();
        let width = meta["pixelWidth"].as_f64().unwrap();
        let height = meta["pixelHeight"].as_f64().unwrap();
        let cues = synthesize_zoom_cues(&capture, duration);
        let clips = RecordingClipTimeline::new(vec![
            crate::recording::clips::RecordingClipSegment::new(0.0, duration),
        ]);
        let pointer = PointerTimeline::build(
            capture.clone(),
            duration,
            width,
            height,
            PointerTimelineOptions::default(),
        );
        for (index, cue) in cues.iter().enumerate() {
            let clusters = build_focus_clusters(cue, &capture, &clips, &pointer);
            eprintln!("cue {index}: {:.2}s..{:.2}s zoom {:.2}", cue.start, cue.end, cue.zoom);
            for cluster in &clusters {
                eprintln!(
                    "  cluster from {:.2}s x {:.3}..{:.3} y {:.3}..{:.3} center ({:.3},{:.3})",
                    cluster.editor_start, cluster.min_x, cluster.max_x, cluster.min_y,
                    cluster.max_y, cluster.center().x, cluster.center().y
                );
            }
        }
        let viewport = ViewportTimeline::build(&cues, &pointer, &clips, &capture);
        for press in capture.presses.iter().filter(|press| press.phase == PressPhase::Down) {
            let frame = viewport.frame_at(press.time);
            eprintln!(
                "click {:.2}s at ({:.3},{:.3}): magnification {:.2} anchor ({:.3},{:.3})",
                press.time, press.x, press.y, frame.magnification, frame.anchor.x, frame.anchor.y
            );
        }
    }

    fn down(time: f64, x: f64, y: f64) -> PointerPressEvent {
        PointerPressEvent {
            time,
            x,
            y,
            button: 0,
            phase: PressPhase::Down,
            artwork_id: None,
        }
    }

    #[test]
    fn adjacent_click_cues_merge_but_separated_clicks_release() {
        let capture = PointerCaptureFile {
            presses: vec![
                down(2.0, 0.2, 0.2),
                down(4.0, 0.5, 0.5),
                down(8.0, 0.8, 0.8),
            ],
            ..Default::default()
        };
        let cues = synthesize_zoom_cues(&capture, 20.0);
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start - 1.4).abs() < 0.0001);
        assert!((cues[0].end - 5.6).abs() < 0.0001);
        assert!((cues[1].start - 7.4).abs() < 0.0001);
        assert!((cues[1].end - 9.6).abs() < 0.0001);
        assert_eq!(cues[0].zoom, 1.5);
    }

    #[test]
    fn cinematic_clicks_hold_related_activity_and_release_between_groups() {
        let capture = PointerCaptureFile {
            presses: vec![
                down(14.0, 0.8, 0.8),
                down(2.0, 0.2, 0.2),
                down(7.2, 0.5, 0.5),
            ],
            ..Default::default()
        };
        let cinematic =
            synthesize_zoom_cues_with_style(&capture, 20.0, AutoZoomStyle::Cinematic);
        assert_eq!(cinematic.len(), 2);
        assert!((cinematic[0].start - 1.7).abs() < 1e-9);
        assert!((cinematic[0].end - 9.7).abs() < 1e-9);
        assert!((cinematic[1].start - 13.7).abs() < 1e-9);
        assert!((cinematic[1].end - 16.5).abs() < 1e-9);
        for cue in &cinematic {
            assert_eq!(cue.zoom, 2.0);
            assert_eq!(cue.camera_motion, CameraMotion::Focused);
            assert!(!cue.skips_easing);
        }
        // The existing recipe still lets the camera pull back between these
        // clicks; selecting the new recipe is the only behavior change.
        let compact = synthesize_zoom_cues(&capture, 20.0);
        assert_eq!(compact.len(), 3);
        assert!(compact.iter().all(|cue| cue.zoom == 1.5
            && cue.camera_motion == CameraMotion::Standard));
    }

    #[test]
    fn cinematic_clicks_respect_recording_edges_and_tail_exclusion() {
        let capture = PointerCaptureFile {
            presses: vec![
                down(0.0, 0.2, 0.2),
                down(8.9, 0.8, 0.8),
                down(9.0, 0.5, 0.5),
            ],
            ..Default::default()
        };
        let cues = synthesize_zoom_cues_with_style(&capture, 10.0, AutoZoomStyle::Cinematic);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start, 0.001);
        assert_eq!(cues[0].end, 2.5);
        assert!((cues[1].start - 8.6).abs() < 1e-9);
        assert_eq!(cues[1].end, 9.2);
        assert_eq!(cues[1].pinned_point, NormalizedPoint { x: 0.8, y: 0.8 });
        let final_click = PointerCaptureFile {
            presses: vec![down(9.0, 0.5, 0.5)],
            ..Default::default()
        };
        assert!(synthesize_zoom_cues_with_style(
            &final_click, 10.0, AutoZoomStyle::Cinematic,
        ).is_empty());
        for duration in [0.5, 0.0, f64::NAN, f64::INFINITY] {
            assert!(synthesize_zoom_cues_with_style(
                &capture, duration, AutoZoomStyle::Cinematic,
            ).is_empty());
        }
    }

    #[test]
    fn corner_reveal_opens_zoomed_on_the_corner_and_ends_on_the_full_frame() {
        let cues = MotionPreset::CornerReveal.cues(5.0);
        let timeline = ViewportTimeline::build_static(&cues, 5.0);
        let opening = timeline.frame_at(0.0);
        assert!((opening.magnification - 2.2).abs() < 1e-6);
        let (left, top, _, _) = visible_rect(opening);
        assert!(
            left < 1e-6 && top < 1e-6,
            "should open on the top-left corner"
        );
        assert!((timeline.frame_at(1.0).magnification - 2.2).abs() < 1e-6);
        assert!(timeline.frame_at(5.0).magnification < 1.01);
    }

    #[test]
    fn tilted_scroll_keeps_the_tilt_while_panning_down() {
        let cues = MotionPreset::TiltedScroll.cues(5.0);
        let timeline = ViewportTimeline::build_static(&cues, 5.0);
        let (start, end) = (timeline.frame_at(0.0), timeline.frame_at(5.0));
        assert!((start.tilt.y - 22.0).abs() < 1e-6);
        assert!((end.tilt.y - 22.0).abs() < 1e-6);
        assert!(end.anchor.y > start.anchor.y + 0.3);
    }

    #[test]
    fn automatic_click_zoom_springs_back_to_identity_after_the_cue() {
        let capture = PointerCaptureFile {
            travel: vec![PointerTravelSample {
                time: 2.0,
                x: 0.7,
                y: 0.4,
                kind: PointerTravelKind::Move,
                artwork_id: None,
            }],
            presses: vec![down(2.0, 0.7, 0.4)],
            ..Default::default()
        };
        let clips = RecordingClipTimeline::full(8.0);
        let pointer = PointerTimeline::build_with_clip_timeline(
            capture.clone(),
            8.0,
            1920.0,
            1080.0,
            PointerTimelineOptions::default(),
            Some(&clips),
        );
        let cues = synthesize_zoom_cues(&capture, 8.0);
        assert_eq!(cues.len(), 1);
        assert!(viewport_magnification(&cues, &pointer, &clips, &capture, 3.4) > 1.35);
        assert!(viewport_magnification(&cues, &pointer, &clips, &capture, 4.6) < 1.02);
    }

    fn viewport_magnification(
        cues: &[ZoomCue],
        pointer: &PointerTimeline,
        clips: &RecordingClipTimeline,
        capture: &PointerCaptureFile,
        time: f64,
    ) -> f64 {
        ViewportTimeline::build(cues, pointer, clips, capture)
            .frame_at(time)
            .magnification
    }

    #[test]
    fn swift_parity_ignores_clicks_in_final_second() {
        let capture = PointerCaptureFile {
            presses: vec![down(9.2, 0.5, 0.5)],
            ..Default::default()
        };
        assert!(synthesize_zoom_cues(&capture, 10.0).is_empty());
    }

    #[test]
    fn zoom_in_keeps_its_pivot_fixed_between_simulation_samples() {
        for mode in [ZoomAnchorMode::PinnedAnchor, ZoomAnchorMode::PointerAnchor] {
            let mut cue = ZoomCue::pinned(
                0.5, 2.0, 1.5, NormalizedPoint { x: 0.65, y: 0.35 },
            );
            cue.anchor_mode = mode;
            cue.bounds_bias = 0.25;
            let viewport = ViewportTimeline::build_static(&[cue], 4.0);
            // These pivots produce the existing settled framing at 1.5x.
            let pivot = match mode {
                ZoomAnchorMode::PinnedAnchor => NormalizedPoint { x: 0.95, y: 0.05 },
                _ => NormalizedPoint { x: 0.875, y: 0.125 },
            };
            for time in [0.501, 0.537, 0.613, 0.729, 0.947, 1.313] {
                assert_pivot_stationary(viewport.frame_at(time), pivot);
            }
        }
    }

    #[test]
    fn zoom_out_preserves_the_outgoing_framing() {
        let cue = ZoomCue::pinned(
            0.5, 2.0, 1.5, NormalizedPoint { x: 0.625, y: 0.375 },
        );
        let viewport = ViewportTimeline::build_static(&[cue], 4.0);
        let pivot = NormalizedPoint { x: 0.875, y: 0.125 };
        for time in [2.017, 2.113, 2.229, 2.347, 2.513, 3.173] {
            assert_pivot_stationary(viewport.frame_at(time), pivot);
        }
        assert_eq!(viewport.frame_at(4.0).magnification, 1.0);
    }

    #[test]
    fn authored_zoom_ramps_keep_a_fixed_pivot_and_support_random_seeks() {
        for motion in [MotionStyle::ZoomIn, MotionStyle::ZoomOut] {
            let mut cue = ZoomCue::pinned(
                0.0, 2.5, 1.5, NormalizedPoint { x: 0.6, y: 0.4 },
            );
            cue.motion = motion;
            cue.skips_easing = motion == MotionStyle::ZoomOut;
            let viewport = ViewportTimeline::build_static(&[cue], 4.0);
            let pivot = NormalizedPoint { x: 0.8, y: 0.2 };
            let times = [0.003, 0.113, 0.729, 1.347, 2.113, 2.497, 2.613, 3.719];
            let sequential: Vec<_> = times.iter().map(|&t| viewport.frame_at(t)).collect();
            for index in [7, 0, 4, 2, 6, 1, 5, 3] {
                let frame = viewport.frame_at(times[index]);
                assert_eq!(frame, sequential[index]);
                assert_pivot_stationary(frame, pivot);
            }
        }
    }

    fn assert_pivot_stationary(frame: ViewportFrame, pivot: NormalizedPoint) {
        let (left, top, visible, _) = visible_rect(frame);
        let projected = NormalizedPoint {
            x: (pivot.x - left) / visible,
            y: (pivot.y - top) / visible,
        };
        assert!(
            (projected.x - pivot.x).abs() < 1e-9
                && (projected.y - pivot.y).abs() < 1e-9,
            "zoom pivot drifted: {pivot:?} -> {projected:?} at {frame:?}"
        );
    }

    #[test]
    fn viewport_uses_a_precomputed_spring_without_frame_jumps() {
        let capture = PointerCaptureFile {
            travel: vec![
                PointerTravelSample {
                    time: 0.0,
                    x: 0.2,
                    y: 0.4,
                    kind: PointerTravelKind::Move,
                    artwork_id: None,
                },
                PointerTravelSample {
                    time: 1.0,
                    x: 0.8,
                    y: 0.6,
                    kind: PointerTravelKind::Move,
                    artwork_id: None,
                },
            ],
            presses: vec![down(1.0, 0.8, 0.6)],
            ..Default::default()
        };
        let clips = RecordingClipTimeline::full(5.0);
        let pointer = PointerTimeline::build_with_clip_timeline(
            capture.clone(),
            5.0,
            1920.0,
            1080.0,
            PointerTimelineOptions::default(),
            Some(&clips),
        );
        let cues = synthesize_zoom_cues(&capture, 5.0);
        let viewport = ViewportTimeline::build(&cues, &pointer, &clips, &capture);
        assert_eq!(viewport.frame_at(0.0).magnification, 1.0);
        assert!(viewport.frame_at(1.2).magnification > 1.0);

        let mut previous = viewport.frame_at(0.0);
        for index in 1..=(5.0 * STEP_RATE) as usize {
            let current = viewport.frame_at(index as f64 / STEP_RATE);
            assert!((current.magnification - previous.magnification).abs() < 0.04);
            assert!((current.anchor.x - previous.anchor.x).abs() < 0.04);
            assert!((current.anchor.y - previous.anchor.y).abs() < 0.04);
            previous = current;
        }
    }

    #[test]
    fn auto_zoom_camera_uses_cap_dead_zone_clusters_instead_of_chasing_cursor() {
        let capture = PointerCaptureFile {
            travel: vec![
                PointerTravelSample {
                    time: 0.2,
                    x: 0.20,
                    y: 0.40,
                    kind: PointerTravelKind::Move,
                    artwork_id: None,
                },
                PointerTravelSample {
                    time: 1.0,
                    x: 0.30,
                    y: 0.45,
                    kind: PointerTravelKind::Move,
                    artwork_id: None,
                },
                PointerTravelSample {
                    time: 2.0,
                    x: 0.90,
                    y: 0.45,
                    kind: PointerTravelKind::Move,
                    artwork_id: None,
                },
            ],
            ..Default::default()
        };
        let clips = RecordingClipTimeline::full(4.0);
        let pointer = PointerTimeline::build_with_clip_timeline(
            capture.clone(),
            4.0,
            1920.0,
            1080.0,
            PointerTimelineOptions::default(),
            Some(&clips),
        );
        let cue = ZoomCue {
            id: Uuid::new_v4(),
            start: 0.1,
            end: 3.5,
            zoom: 2.0,
            anchor_mode: ZoomAnchorMode::PointerAnchor,
            pinned_point: NormalizedPoint { x: 0.5, y: 0.5 },
            bounds_bias: 0.25,
            is_enabled: true,
            is_implicit: false,
            skips_easing: false,
            presentation: ZoomPresentation::Classic,
            loupe: LoupeOptions::default(),
            start_animation_early: false,
            motion: MotionStyle::Hold,
            pan_to: None,
            easing: MotionEasing::Smooth,
            camera_motion: CameraMotion::default(),
            tilt: None,
            transform: None,
        };
        let viewport = ViewportTimeline::build(&[cue], &pointer, &clips, &capture);
        let before_dead_zone_exit = viewport.frame_at(1.5).anchor.x;
        let after_dead_zone_exit = viewport.frame_at(3.0).anchor.x;
        assert!(before_dead_zone_exit < 0.45);
        assert!(after_dead_zone_exit > before_dead_zone_exit + 0.15);
    }

    #[test]
    fn older_projects_without_motion_fields_still_load() {
        let json = r#"{"id":"8f5b4a0e-8f1e-4f4d-9c33-2b6b4f2f1a11","start":1.0,"end":3.0,"zoom":2.0,
            "anchorMode":"pinnedAnchor","pinnedPoint":{"x":0.2,"y":0.3},"boundsBias":0.0,
            "isEnabled":true,"isImplicit":false,"skipsEasing":false}"#;
        let cue: ZoomCue = serde_json::from_str(json).unwrap();
        assert_eq!(cue.motion, MotionStyle::Hold);
        assert_eq!(cue.camera_motion, CameraMotion::Standard);
        assert_eq!(cue.presentation, ZoomPresentation::Classic);
        assert_eq!(cue.loupe, LoupeOptions::default());
        assert_eq!(cue.loupe.radius, 0.35);
        assert_eq!(cue.loupe.clamped().radius, 0.35);
        assert!(!cue.start_animation_early);
        assert_eq!(cue.pan_to, None);
        assert_eq!(cue.transform, None);
        let round_trip = serde_json::to_string(&cue).unwrap();
        assert!(!round_trip.contains("panTo"));
        assert!(round_trip.contains("\"motion\":\"hold\""));
    }

    #[test]
    fn focused_camera_settles_faster_than_smooth_on_entry_and_release() {
        let mut cue = ZoomCue::pinned(0.5, 2.5, 2.0, NormalizedPoint { x: 0.7, y: 0.3 });
        cue.camera_motion = CameraMotion::Smooth;
        let smooth = ViewportTimeline::build_static(&[cue.clone()], 5.0);
        cue.camera_motion = CameraMotion::Focused;
        let focused = ViewportTimeline::build_static(&[cue], 5.0);
        assert!(focused.frame_at(0.85).magnification > smooth.frame_at(0.85).magnification + 0.1);
        assert!(focused.frame_at(2.85).magnification < smooth.frame_at(2.85).magnification - 0.1);
        for timeline in [&smooth, &focused] {
            // A hold should settle without a second zoom pulse; releasing
            // must return to the full image and stay there.
            let mut previous = 1.0;
            for index in 60..=299 {
                let zoom = timeline.frame_at(index as f64 / STEP_RATE).magnification;
                assert!(zoom >= previous - 1e-5 && zoom <= 2.000_01, "{zoom} after {previous}");
                previous = zoom;
            }
            previous = timeline.frame_at(2.51).magnification;
            for index in 302..=600 {
                let zoom = timeline.frame_at(index as f64 / STEP_RATE).magnification;
                assert!(zoom >= 1.0 && zoom <= previous + 1e-5, "{zoom} after {previous}");
                previous = zoom;
            }
            assert!((timeline.frame_at(5.0).magnification - 1.0).abs() < 0.001);
        }
    }

    #[test]
    fn changing_camera_response_preserves_continuity_and_seek_determinism() {
        let mut first = ZoomCue::pinned(0.5, 1.5, 2.0, NormalizedPoint { x: 0.2, y: 0.5 });
        first.camera_motion = CameraMotion::Smooth;
        let mut second = ZoomCue::pinned(1.5, 3.0, 3.0, NormalizedPoint { x: 0.8, y: 0.5 });
        second.camera_motion = CameraMotion::Focused;
        let viewport = ViewportTimeline::build_static(&[first, second], 5.0);
        let before = viewport.frame_at(1.5 - 1.0 / STEP_RATE);
        let at = viewport.frame_at(1.5);
        assert!((at.magnification - before.magnification).abs() < 0.02);
        assert!((at.anchor.x - before.anchor.x).abs() < 0.02);
        let expected = viewport.frame_at(2.25);
        let _ = viewport.frame_at(4.5);
        let _ = viewport.frame_at(0.75);
        assert_eq!(expected, viewport.frame_at(2.25));
    }

    #[test]
    fn zoom_in_region_ramps_smoothly_across_its_duration() {
        let cues = MotionPreset::SlowZoomIn.cues(5.0);
        let viewport = ViewportTimeline::build_static(&cues, 5.0);
        let early = viewport.frame_at(0.5).magnification;
        let middle = viewport.frame_at(2.5).magnification;
        let late = viewport.frame_at(4.9).magnification;
        assert!(early < middle && middle < late, "{early} {middle} {late}");
        assert!(early < 1.15, "{early}");
        assert!(late > 1.5, "{late}");
        let mut previous = viewport.frame_at(0.0).magnification;
        for index in 1..=(5.0 * STEP_RATE) as usize {
            let current = viewport.frame_at(index as f64 / STEP_RATE).magnification;
            assert!((current - previous).abs() < 0.02);
            previous = current;
        }
    }

    #[test]
    fn pan_region_glides_the_anchor_between_its_points() {
        let cues = MotionPreset::PanRight.cues(4.0);
        let viewport = ViewportTimeline::build_static(&cues, 4.0);
        let start = viewport.frame_at(0.05).anchor.x;
        let end = viewport.frame_at(3.95).anchor.x;
        assert!(start < 0.45, "{start}");
        assert!(end > 0.55, "{end}");
        let mut previous = viewport.frame_at(0.0).anchor.x;
        for index in 1..=(4.0 * STEP_RATE) as usize {
            let current = viewport.frame_at(index as f64 / STEP_RATE).anchor.x;
            assert!((current - previous).abs() < 0.02);
            previous = current;
        }
    }

    #[test]
    fn tilt_presets_animate_rotation_smoothly() {
        let cues = MotionPreset::Tilt3D.cues(4.0);
        let viewport = ViewportTimeline::build_static(&cues, 4.0);
        let start = viewport.frame_at(0.0).tilt;
        let end = viewport.frame_at(4.0).tilt;
        assert!(start.y < -15.0, "{start:?}");
        assert!(end.y.abs() < 2.0, "{end:?}");
        let mut previous = viewport.frame_at(0.0).tilt.y;
        for index in 1..=(4.0 * STEP_RATE) as usize {
            let current = viewport.frame_at(index as f64 / STEP_RATE).tilt.y;
            assert!((current - previous).abs() < 0.5);
            previous = current;
        }
        let floating = MotionPreset::FloatingCard.cues(6.0);
        assert!(floating.len() >= 2);
        assert!(floating.iter().all(|cue| cue.tilt.is_some()));
    }

    #[test]
    fn easing_curves_are_monotonic_and_bounded() {
        for easing in MotionEasing::ALL {
            let mut previous = easing.apply(0.0);
            assert!(previous.abs() < 1e-9);
            for step in 1..=100 {
                let current = easing.apply(step as f64 / 100.0);
                assert!(current >= previous - 1e-9, "{easing:?}");
                previous = current;
            }
            assert!((previous - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn visible_rect_is_clamped_inside_the_media() {
        let (left, top, visible, _) = visible_rect(ViewportFrame {
            magnification: 2.0,
            anchor: NormalizedPoint { x: 0.0, y: 1.0 },
            ..ViewportFrame::default()
        });
        assert_eq!(visible, 0.5);
        assert_eq!(left, 0.0);
        assert_eq!(top, 0.5);
    }
}

#[cfg(test)]
mod deleted_motion_tests {
    use super::*;

    #[test]
    fn deleted_motion_cannot_hold_its_end_pose_on_retained_video() {
        let mut cue = ZoomCue::pinned(1.0, 2.0, 1.0, NormalizedPoint { x: 0.5, y: 0.5 });
        let mut pose = SceneTransform::default();
        pose.scale = 1.5;
        cue.transform = Some(TransformMotion { end: pose, keep_end_state: true, ..Default::default() });
        let clips = RecordingClipTimeline::new(vec![super::super::clips::RecordingClipSegment::new(3.0, 5.0)]);
        let viewport = ViewportTimeline::build(&[cue], &PointerTimeline::default(), &clips, &PointerCaptureFile::default());
        assert_eq!(viewport.frame_at(1.0).transform, SceneTransform::default());
    }
}

#[cfg(test)]
mod loupe_tests {
    use super::*;
    use crate::recording::{clips::RecordingClipSegment, model::{PointerTravelKind, PointerTravelSample}};

    fn cue() -> ZoomCue {
        let mut cue = ZoomCue::pinned(1.0, 2.5, 2.0, NormalizedPoint { x: 0.3, y: 0.6 });
        cue.presentation = ZoomPresentation::GlassLoupe;
        cue.camera_motion = CameraMotion::Focused;
        cue
    }

    #[test]
    fn lens_fades_in_and_out_without_scaling_the_screen_or_shrinking_radius() {
        let timeline = ViewportTimeline::build_static(&[cue()], 5.0);
        assert!(timeline.frame_at(0.8).loupe.is_none());
        let entering = timeline.frame_at(1.1).loupe.unwrap();
        let settled = timeline.frame_at(2.0).loupe.unwrap();
        let leaving = timeline.frame_at(2.6).loupe.unwrap();
        assert!(entering.presence > 0.0 && entering.presence < 0.8, "{entering:?}");
        assert!(settled.presence > 0.99 && settled.magnification > 1.99, "{settled:?}");
        assert!(leaving.presence > 0.0 && leaving.presence < settled.presence);
        assert!((entering.radius - 0.35).abs() < 1e-9);
        assert!((leaving.radius - 0.35).abs() < 1e-9);
        assert!((settled.center.x - 0.3).abs() < 0.02);
        assert!((settled.center.y - 0.6).abs() < 0.02);
        assert!(timeline.frame_at(4.9).loupe.is_none());
        for time in [0.0, 0.9, 1.1, 2.0, 2.6, 4.9] {
            assert_eq!(timeline.frame_at(time).magnification, 1.0);
        }
    }

    #[test]
    fn early_onset_respects_active_regions_and_instant_intervals_snap() {
        for presentation in [ZoomPresentation::Classic, ZoomPresentation::GlassLoupe] {
            let mut cue = cue();
            cue.presentation = presentation;
            cue.start_animation_early = true;
            let timeline = ViewportTimeline::build_static(&[cue.clone()], 4.0);
            let visible = |frame: ViewportFrame| frame.magnification > 1.001 || frame.loupe.is_some();
            assert!(!visible(timeline.frame_at(0.6)));
            assert!(visible(timeline.frame_at(0.85)));
            let prior = ZoomCue::pinned(0.1, 0.95, 1.0, NormalizedPoint { x: 0.5, y: 0.5 });
            assert_ne!(active_zoom_cue(0.8, &[prior, cue.clone()], presentation).map(|c| c.id), Some(cue.id));
            cue.skips_easing = true;
            let timeline = ViewportTimeline::build_static(&[cue], 4.0);
            assert!(!visible(timeline.frame_at(0.85)));
            let frame = timeline.frame_at(1.0);
            let zoom = frame.loupe.map_or(frame.magnification, |lens| lens.magnification);
            assert_eq!(zoom, 2.0);
            assert!(visible(timeline.frame_at(2.49)));
            assert!(!visible(timeline.frame_at(2.51)));
        }
    }

    #[test]
    fn loupe_and_classic_camera_have_independent_tracks_and_seek_deterministically() {
        let classic = ZoomCue::pinned(0.2, 3.0, 1.5, NormalizedPoint { x: 0.5, y: 0.5 });
        let mut glass = cue();
        glass.loupe.radius = 0.75;
        let cues = [classic, glass];
        let timeline = ViewportTimeline::build_static(&cues, 5.0);
        let sample = timeline.frame_at(2.1);
        assert!((sample.magnification - 1.5).abs() < 0.01);
        assert!((sample.loupe.unwrap().magnification - 2.0).abs() < 0.01);
        assert_eq!(sample.loupe.unwrap().radius, 0.75);
        let times = [0.01, 0.85, 1.001, 1.125, 2.1, 2.555, 3.45, 4.99];
        let sequential: Vec<_> = times.map(|time| timeline.frame_at(time)).into();
        for i in [7, 1, 4, 0, 5, 2, 6, 3] { assert_eq!(timeline.frame_at(times[i]), sequential[i]); }
        let json = serde_json::to_string(&cues).unwrap();
        let restored = serde_json::from_str::<Vec<ZoomCue>>(&json).unwrap();
        assert_eq!(restored, cues);
        assert_eq!(restored[1].loupe.radius, 0.75);
        assert_eq!(restored[1].loupe.clamped().radius, 0.75);
        let mut oversized = restored[1].clone();
        oversized.loupe.radius = 1.2;
        oversized.skips_easing = true;
        assert_eq!(oversized.loupe.clamped().radius, 0.75);
        assert_eq!(ViewportTimeline::build_static(&[oversized], 5.0)
            .frame_at(1.5).loupe.unwrap().radius, 0.75);
    }

    #[test]
    fn instant_lens_boundaries_do_not_crossfade_between_baked_samples() {
        let mut cue = cue();
        cue.start = 1.003;
        cue.end = 2.473;
        cue.skips_easing = true;
        let timeline = ViewportTimeline::build_static(&[cue], 4.0);
        assert!(timeline.frame_at(1.002999).loupe.is_none());
        assert_eq!(timeline.frame_at(1.003).loupe.unwrap().presence, 1.0);
        assert_eq!(timeline.frame_at(2.472999).loupe.unwrap().presence, 1.0);
        assert!(timeline.frame_at(2.473001).loupe.is_none());
    }

    #[test]
    fn lens_uses_clip_time_and_removed_regions_cannot_appear() {
        let mut segment = RecordingClipSegment::new(0.5, 3.5);
        segment.speed = 2.0;
        let clips = RecordingClipTimeline::new(vec![segment]);
        let mut cue = cue();
        cue.skips_easing = true;
        let timeline = ViewportTimeline::build(&[cue.clone()], &PointerTimeline::default(), &clips,
            &PointerCaptureFile::default());
        assert!(timeline.frame_at(0.1).loupe.is_none());
        assert_eq!(timeline.frame_at(0.3).loupe.unwrap().magnification, 2.0);
        assert!(timeline.frame_at(1.1).loupe.is_none());
        let removed = RecordingClipTimeline::new(vec![RecordingClipSegment::new(3.0, 5.0)]);
        let timeline = ViewportTimeline::build(&[cue], &PointerTimeline::default(), &removed,
            &PointerCaptureFile::default());
        assert!(timeline.frame_at(0.5).loupe.is_none());
    }

    #[test]
    fn cluster_focus_is_weighted_by_dwell_and_not_sample_count() {
        let sample = |time, x, y| PointerTravelSample { time, x, y,
            kind: PointerTravelKind::Move, artwork_id: None };
        let capture = PointerCaptureFile { travel: vec![sample(0.0, 0.4, 0.5),
            sample(1.8, 0.5, 0.5), sample(1.9, 0.6, 0.5)], ..Default::default() };
        let mut cue = ZoomCue::pinned(0.0, 2.0, 2.0, NormalizedPoint { x:0.5,y:0.5 });
        cue.anchor_mode = ZoomAnchorMode::SmartAnchor;
        let clusters = build_focus_clusters(&cue, &capture, &RecordingClipTimeline::full(2.0),
            &PointerTimeline::default());
        assert_eq!(clusters.len(), 1);
        assert!((clusters[0].center().x - 0.415).abs() < 1e-9);
        let cropped = build_focus_clusters_for_viewport(&cue, &capture,
            &RecordingClipTimeline::full(2.0), &PointerTimeline::default(), (0.3, 1.0));
        assert_eq!(cropped.len(), 3, "narrow crops need smaller activity groups");
        // The same limit applies on both axes, so a vertical excursion forms
        // a new viewing region just like horizontal travel.
        let capture = PointerCaptureFile { travel: vec![sample(0.0,0.4,0.2), sample(1.0,0.4,0.5)], ..Default::default() };
        assert_eq!(build_focus_clusters(&cue, &capture, &RecordingClipTimeline::full(2.0),
            &PointerTimeline::default()).len(), 2);
    }
}
