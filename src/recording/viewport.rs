use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    clips::RecordingClipTimeline,
    model::{NormalizedPoint, PointerCaptureFile, PressPhase},
    motion::{DampedSpring, SpringConstant},
    pointer_timeline::PointerTimeline,
};

/// Normalized rectangle of the media a viewport frame shows:
/// `(left, top, visible_fraction)`. Preview, focus picking, and export all
/// derive the crop from this single definition.
pub fn visible_rect(frame: ViewportFrame) -> (f64, f64, f64) {
    let visible = 1.0 / frame.magnification.max(1.0);
    let left = (frame.anchor.x - visible * 0.5).clamp(0.0, 1.0 - visible);
    let top = (frame.anchor.y - visible * 0.5).clamp(0.0, 1.0 - visible);
    (left, top, visible)
}

const STEP_RATE: f64 = 120.0;
const CLUSTER_WIDTH_RATIO: f64 = 0.5;
const CLUSTER_HEIGHT_RATIO: f64 = 0.7;
const MOTION_PROFILE: SpringConstant = SpringConstant {
    tension: 200.0,
    friction: 40.0,
    inertia: 2.25,
};

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
}

impl Default for ViewportFrame {
    fn default() -> Self {
        Self {
            magnification: 1.0,
            anchor: NormalizedPoint { x: 0.5, y: 0.5 },
            tilt: Tilt::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ViewportTimeline {
    frames: Vec<ViewportFrame>,
    duration: f64,
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
        let duration = clips.duration();
        if !duration.is_finite() || duration <= 0.0 {
            return Self::default();
        }
        let frame_count = ((duration * STEP_RATE).ceil() as usize + 1).max(2);
        let dt = 1.0 / STEP_RATE;
        let mut amount = DampedSpring::new(1.0);
        let mut anchor_x = DampedSpring::new(0.5);
        let mut anchor_y = DampedSpring::new(0.5);
        let mut tilt_x = DampedSpring::new(0.0);
        let mut tilt_y = DampedSpring::new(0.0);
        let mut tilt_z = DampedSpring::new(0.0);
        let mut previous_cue = None;
        let mut frames = Vec::with_capacity(frame_count);
        let cue_clusters: Vec<Vec<FocusCluster>> = cues
            .iter()
            .map(|cue| build_focus_clusters(cue, capture, clips, pointer))
            .collect();

        for frame_index in 0..frame_count {
            let editor_time = (frame_index as f64 * dt).min(duration);
            let source_time = clips.source_time_at(editor_time);
            let active = active_cue(source_time, cues);
            let progress = active
                .map(|cue| cue.progress_at(source_time))
                .unwrap_or(0.0);
            let target_magnification = active
                .map(|cue| cue.magnification_at(progress))
                .unwrap_or(1.0);
            let raw_target = active
                .and_then(|cue| {
                    let base = match cue.anchor_mode {
                        ZoomAnchorMode::PinnedAnchor => Some(cue.pinned_point),
                        ZoomAnchorMode::PointerAnchor | ZoomAnchorMode::SmartAnchor => {
                            let cue_index =
                                cues.iter().position(|candidate| candidate.id == cue.id)?;
                            cluster_center_at(&cue_clusters[cue_index], editor_time)
                                .or(Some(cue.pinned_point))
                        }
                    }?;
                    Some(cue.anchor_at(base, progress))
                })
                .unwrap_or(NormalizedPoint { x: 0.5, y: 0.5 });
            let target_anchor = bounded_anchor(
                raw_target,
                target_magnification,
                active
                    .map(|cue| cue.anchor_mode)
                    .unwrap_or(ZoomAnchorMode::PinnedAnchor),
                active.map(|cue| cue.bounds_bias).unwrap_or(0.0),
            );
            let target_tilt = active.map(|cue| cue.tilt_at(progress)).unwrap_or_default();
            let cue_id = active.map(|cue| cue.id);
            let changed = cue_id != previous_cue;
            let should_snap = changed
                && (active.is_some_and(|cue| cue.skips_easing)
                    || previous_cue
                        .and_then(|id| cues.iter().find(|cue| cue.id == id))
                        .is_some_and(|cue| cue.skips_easing));

            if should_snap {
                amount.snap(target_magnification);
                anchor_x.snap(target_anchor.x);
                anchor_y.snap(target_anchor.y);
                tilt_x.snap(target_tilt.x);
                tilt_y.snap(target_tilt.y);
                tilt_z.snap(target_tilt.z);
            } else if frame_index > 0 {
                tilt_x.step(target_tilt.x, MOTION_PROFILE, dt);
                tilt_y.step(target_tilt.y, MOTION_PROFILE, dt);
                tilt_z.step(target_tilt.z, MOTION_PROFILE, dt);
                // Cap's renderer pre-aims while scale is visually identity.
                // This prevents a late diagonal pan as an incoming zoom ramps.
                if amount.position <= 1.000_5 && target_magnification > 1.0 {
                    anchor_x.snap(target_anchor.x);
                    anchor_y.snap(target_anchor.y);
                } else {
                    anchor_x.step(target_anchor.x, MOTION_PROFILE, dt);
                    anchor_y.step(target_anchor.y, MOTION_PROFILE, dt);
                }
                amount.step(target_magnification, MOTION_PROFILE, dt);
            }

            let magnification = amount.position.max(1.0);
            frames.push(ViewportFrame {
                magnification,
                anchor: clamp_to_frame(
                    NormalizedPoint {
                        x: anchor_x.position,
                        y: anchor_y.position,
                    },
                    magnification,
                ),
                tilt: Tilt {
                    x: tilt_x.position,
                    y: tilt_y.position,
                    z: tilt_z.position,
                },
            });
            previous_cue = cue_id;
        }

        Self { frames, duration }
    }

    pub fn frame_at(&self, time: f64) -> ViewportFrame {
        let Some(first) = self.frames.first().copied() else {
            return ViewportFrame::default();
        };
        if self.frames.len() == 1 || self.duration <= 0.0 {
            return first;
        }
        let position = time.clamp(0.0, self.duration) * STEP_RATE;
        let index = position as usize;
        if index >= self.frames.len() - 1 {
            return *self.frames.last().unwrap_or(&first);
        }
        let fraction = position - index as f64;
        let left = self.frames[index];
        let right = self.frames[index + 1];
        ViewportFrame {
            magnification: lerp(left.magnification, right.magnification, fraction),
            anchor: NormalizedPoint {
                x: lerp(left.anchor.x, right.anchor.x, fraction),
                y: lerp(left.anchor.y, right.anchor.y, fraction),
            },
            tilt: Tilt {
                x: lerp(left.tilt.x, right.tilt.x, fraction),
                y: lerp(left.tilt.y, right.tilt.y, fraction),
                z: lerp(left.tilt.z, right.tilt.z, fraction),
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FocusCluster {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    editor_start: f64,
}

impl FocusCluster {
    fn new(point: NormalizedPoint, editor_start: f64) -> Self {
        Self {
            min_x: point.x,
            max_x: point.x,
            min_y: point.y,
            max_y: point.y,
            editor_start,
        }
    }

    fn can_add(self, point: NormalizedPoint, max_width: f64, max_height: f64) -> bool {
        self.max_x.max(point.x) - self.min_x.min(point.x) <= max_width
            && self.max_y.max(point.y) - self.min_y.min(point.y) <= max_height
    }

    fn add(&mut self, point: NormalizedPoint) {
        self.min_x = self.min_x.min(point.x);
        self.max_x = self.max_x.max(point.x);
        self.min_y = self.min_y.min(point.y);
        self.max_y = self.max_y.max(point.y);
    }

    fn center(self) -> NormalizedPoint {
        NormalizedPoint {
            x: (self.min_x + self.max_x) * 0.5,
            y: (self.min_y + self.max_y) * 0.5,
        }
    }
}

fn build_focus_clusters(
    cue: &ZoomCue,
    capture: &PointerCaptureFile,
    clips: &RecordingClipTimeline,
    pointer: &PointerTimeline,
) -> Vec<FocusCluster> {
    if cue.anchor_mode == ZoomAnchorMode::PinnedAnchor {
        return Vec::new();
    }
    let max_width = CLUSTER_WIDTH_RATIO / cue.zoom.max(1.0);
    let max_height = CLUSTER_HEIGHT_RATIO / cue.zoom.max(1.0);
    let mut samples: Vec<_> = capture
        .travel
        .iter()
        .filter(|sample| {
            sample.time.is_finite()
                && sample.x.is_finite()
                && sample.y.is_finite()
                && sample.time >= cue.start
                && sample.time <= cue.end
        })
        .filter_map(|sample| {
            clips.editor_time_for_event(sample.time).map(|editor_time| {
                (
                    editor_time,
                    NormalizedPoint {
                        x: sample.x,
                        y: sample.y,
                    }
                    .clamped(),
                )
            })
        })
        .collect();
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
            current.add(point);
        } else {
            clusters.push(current);
            current = FocusCluster::new(point, editor_time);
        }
    }
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

fn active_cue(time: f64, cues: &[ZoomCue]) -> Option<&ZoomCue> {
    cues.iter()
        .filter(|cue| cue.is_enabled && time >= cue.start && time <= cue.end)
        .max_by(|left, right| {
            cue_priority(left)
                .cmp(&cue_priority(right))
                .then_with(|| left.start.total_cmp(&right.start))
        })
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
    /// Optional 3D tilt of the media surface while the region is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt: Option<Tilt>,
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
            motion: MotionStyle::Hold,
            pan_to: None,
            easing: MotionEasing::Smooth,
            tilt: None,
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
        format!("{motion}{:.1}×{pan}{tilt}", self.zoom)
    }

    fn around_press(time: f64, point: NormalizedPoint, duration: f64) -> Option<Self> {
        // Keep automatic click zooms deliberately short. The cue's `end` is
        // the start of the camera's spring-driven release back to 1x; making
        // the post-roll too long leaves the recording looking permanently
        // zoomed after ordinary clicks.
        const PRE_ROLL: f64 = 0.6;
        const POST_ROLL: f64 = 1.6;
        const TRAILING_GUARD: f64 = 0.8;
        const EARLIEST_START: f64 = 0.001;

        let start = (time - PRE_ROLL).max(EARLIEST_START);
        let end = (time + POST_ROLL).min(duration - TRAILING_GUARD);
        (end > start).then(|| Self {
            id: Uuid::new_v4(),
            start,
            end,
            zoom: 1.5,
            anchor_mode: ZoomAnchorMode::PointerAnchor,
            pinned_point: point.clamped(),
            bounds_bias: 0.25,
            is_enabled: true,
            is_implicit: false,
            skips_easing: false,
            motion: MotionStyle::Hold,
            pan_to: None,
            easing: MotionEasing::Smooth,
            tilt: None,
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
}

impl MotionPreset {
    pub const ALL: [MotionPreset; 8] = [
        MotionPreset::SlowZoomIn,
        MotionPreset::SlowZoomOut,
        MotionPreset::PanLeft,
        MotionPreset::PanRight,
        MotionPreset::FocusCenter,
        MotionPreset::Sweep,
        MotionPreset::Tilt3D,
        MotionPreset::FloatingCard,
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
        }
    }
}

/// Down-clicks create editable cues using Cap's activity timing. Only
/// overlapping or genuinely adjacent click windows merge, leaving enough
/// identity time for the camera to visibly zoom back out between interactions.
pub fn synthesize_zoom_cues(capture: &PointerCaptureFile, duration: f64) -> Vec<ZoomCue> {
    const JOIN_TOLERANCE: f64 = 0.6;
    const TAIL_EXCLUSION: f64 = 1.0;

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
            )
        })
        .collect();
    candidates.sort_by(|a, b| a.start.total_cmp(&b.start));

    let mut merged: Vec<ZoomCue> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if let Some(previous) = merged.last_mut() {
            if candidate.start <= previous.end + JOIN_TOLERANCE {
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
            None,
            None,
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
            None,
            None,
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
            None,
            None,
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
            motion: MotionStyle::Hold,
            pan_to: None,
            easing: MotionEasing::Smooth,
            tilt: None,
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
        assert_eq!(cue.pan_to, None);
        let round_trip = serde_json::to_string(&cue).unwrap();
        assert!(!round_trip.contains("panTo"));
        assert!(round_trip.contains("\"motion\":\"hold\""));
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
        let (left, top, visible) = visible_rect(ViewportFrame {
            magnification: 2.0,
            anchor: NormalizedPoint { x: 0.0, y: 1.0 },
            tilt: Tilt::default(),
        });
        assert_eq!(visible, 0.5);
        assert_eq!(left, 0.0);
        assert_eq!(top, 0.5);
    }
}
