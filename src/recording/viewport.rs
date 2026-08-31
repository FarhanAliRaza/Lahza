use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    clips::RecordingClipTimeline,
    model::{NormalizedPoint, PointerCaptureFile, PressPhase},
    motion::{DampedSpring, SpringConstant},
    pointer_timeline::PointerTimeline,
};

const STEP_RATE: f64 = 120.0;
const CLUSTER_WIDTH_RATIO: f64 = 0.5;
const CLUSTER_HEIGHT_RATIO: f64 = 0.7;
const MOTION_PROFILE: SpringConstant = SpringConstant {
    tension: 200.0,
    friction: 40.0,
    inertia: 2.25,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportFrame {
    pub magnification: f64,
    pub anchor: NormalizedPoint,
}

impl Default for ViewportFrame {
    fn default() -> Self {
        Self {
            magnification: 1.0,
            anchor: NormalizedPoint { x: 0.5, y: 0.5 },
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ViewportTimeline {
    frames: Vec<ViewportFrame>,
    duration: f64,
}

impl ViewportTimeline {
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
            let target_magnification = active.map(|cue| cue.zoom.max(1.0)).unwrap_or(1.0);
            let raw_target = active
                .and_then(|cue| match cue.anchor_mode {
                    ZoomAnchorMode::PinnedAnchor => Some(cue.pinned_point),
                    ZoomAnchorMode::PointerAnchor | ZoomAnchorMode::SmartAnchor => {
                        let cue_index = cues.iter().position(|candidate| candidate.id == cue.id)?;
                        cluster_center_at(&cue_clusters[cue_index], editor_time)
                            .or(Some(cue.pinned_point))
                    }
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
            } else if frame_index > 0 {
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
}

impl ZoomCue {
    pub const MINIMUM_DURATION: f64 = 0.5;

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
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::model::{
        PointerCaptureFile, PointerPressEvent, PointerTravelKind, PointerTravelSample,
    };

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
        };
        let viewport = ViewportTimeline::build(&[cue], &pointer, &clips, &capture);
        let before_dead_zone_exit = viewport.frame_at(1.5).anchor.x;
        let after_dead_zone_exit = viewport.frame_at(3.0).anchor.x;
        assert!(before_dead_zone_exit < 0.45);
        assert!(after_dead_zone_exit > before_dead_zone_exit + 0.15);
    }
}
