use super::{
    clips::RecordingClipTimeline,
    model::{NormalizedPoint, PointerArtwork, PointerCaptureFile},
    motion::{DampedSpring, SpringConstant},
    pointer::{
        sanitize_pointer_capture, PointerSanitizeOptions, PointerStreamEvent, PointerStreamKind,
    },
};
use std::collections::HashMap;

const STEP_RATE: f64 = 120.0;
const ANTICIPATION_WINDOW: f64 = 0.5;
const INTERCEPT_WINDOW: f64 = 0.175;
const PULSE_DURATION: f64 = 0.4;
const TILT_SAMPLE_WINDOW: f64 = 0.4;
const TILT_GAIN: f64 = 0.03;
const TILT_WEIGHT: f64 = 0.5;
const REVEAL_LEAD_WINDOW: f64 = 0.25;
const IDLE_GAP_THRESHOLD: f64 = 4.0 / 60.0;
const LEAD_SMOOTHING: f64 = 0.12;

const GLIDE: SpringConstant = SpringConstant {
    tension: 470.0,
    friction: 70.0,
    inertia: 3.0,
};
const INTERCEPT: SpringConstant = SpringConstant {
    tension: 538.0,
    friction: 40.0,
    inertia: 1.0,
};
const TRACK: SpringConstant = SpringConstant {
    tension: 1000.0,
    friction: 40.0,
    inertia: 1.0,
};
const SETTLE: SpringConstant = SpringConstant {
    tension: 300.0,
    friction: 30.0,
    inertia: 0.3,
};

#[derive(Clone, Debug, PartialEq)]
pub struct PointerPressFrame {
    pub location: NormalizedPoint,
    pub progress: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PointerFrame {
    pub location: NormalizedPoint,
    pub artwork_id: Option<String>,
    pub magnification: f64,
    pub tilt_degrees: f64,
    pub opacity: f64,
    pub blur_radius: f64,
    pub press: Option<PointerPressFrame>,
}

#[derive(Clone, Debug, Default)]
pub struct PointerTimeline {
    frames: Vec<PointerFrame>,
    duration: f64,
    artwork_by_id: HashMap<String, PointerArtwork>,
    fallback_artwork: Option<PointerArtwork>,
}

impl PointerTimeline {
    pub const STEP_RATE: f64 = STEP_RATE;

    pub fn build(
        capture: PointerCaptureFile,
        duration: f64,
        recording_width: f64,
        recording_height: f64,
        fallback_artwork: Option<PointerArtwork>,
        hide_after_inactivity: Option<f64>,
    ) -> Self {
        Self::build_with_clip_timeline(
            capture,
            duration,
            recording_width,
            recording_height,
            fallback_artwork,
            hide_after_inactivity,
            None,
        )
    }

    pub fn build_with_clip_timeline(
        capture: PointerCaptureFile,
        duration: f64,
        recording_width: f64,
        recording_height: f64,
        fallback_artwork: Option<PointerArtwork>,
        hide_after_inactivity: Option<f64>,
        clip_timeline: Option<&RecordingClipTimeline>,
    ) -> Self {
        if !duration.is_finite() || duration <= 0.0 {
            return Self::default();
        }
        let timeline_duration = clip_timeline
            .map(RecordingClipTimeline::duration)
            .unwrap_or(duration);
        if !timeline_duration.is_finite() || timeline_duration <= 0.0 {
            return Self::default();
        }
        let width = valid_dimension(recording_width);
        let height = valid_dimension(recording_height);
        let stream = sanitize_pointer_capture(
            capture,
            PointerSanitizeOptions::for_recording(width, height),
        );
        let source_samples: Vec<_> = stream
            .samples
            .into_iter()
            .filter(|sample| {
                sample.time.is_finite() && sample.x.is_finite() && sample.y.is_finite()
            })
            .collect();
        let samples = timeline_samples(source_samples, clip_timeline);
        let Some(first) = samples.first().cloned() else {
            return Self::default();
        };
        let press_samples: Vec<_> = samples
            .iter()
            .filter(|sample| sample.kind == PointerStreamKind::Press)
            .cloned()
            .collect();
        let travel_samples: Vec<_> = samples
            .iter()
            .filter(|sample| {
                matches!(
                    sample.kind,
                    PointerStreamKind::Travel | PointerStreamKind::Drag
                )
            })
            .cloned()
            .collect();
        let artwork_by_id = stream
            .artwork
            .into_iter()
            .map(|artwork| (artwork.artwork_id.clone(), artwork))
            .collect();

        let frame_count = ((timeline_duration * STEP_RATE).ceil() as usize + 1).max(2);
        let dt = 1.0 / STEP_RATE;
        let mut x_spring = DampedSpring::new(first.x);
        let mut y_spring = DampedSpring::new(first.y);
        let mut opacity_spring = DampedSpring::new(1.0);
        let mut blur_spring = DampedSpring::new(0.0);
        let mut sample_index: isize = -1;
        let mut travel_index: isize = -1;
        let mut latest_press_index: isize = -1;
        let mut current_artwork_id = first.artwork_id.clone();
        let mut phase_lead = GLIDE.friction / GLIDE.tension;
        let mut frames = Vec::with_capacity(frame_count);

        for frame_index in 0..frame_count {
            let time = (frame_index as f64 * dt).min(timeline_duration);
            while (sample_index + 1) < samples.len() as isize
                && samples[(sample_index + 1) as usize].time <= time
            {
                sample_index += 1;
                current_artwork_id = samples[sample_index as usize].artwork_id.clone();
            }
            while (travel_index + 1) < travel_samples.len() as isize
                && travel_samples[(travel_index + 1) as usize].time <= time
            {
                travel_index += 1;
            }
            while (latest_press_index + 1) < press_samples.len() as isize
                && press_samples[(latest_press_index + 1) as usize].time <= time
            {
                latest_press_index += 1;
            }
            let latest = if sample_index >= 0 {
                &samples[sample_index as usize]
            } else {
                &first
            };
            let mut target = (latest.x, latest.y);
            let mut approaching_press = false;
            let upcoming_press_index = latest_press_index + 1;
            if upcoming_press_index < press_samples.len() as isize {
                let press = &press_samples[upcoming_press_index as usize];
                let remaining = press.time - time;
                if (0.0..=ANTICIPATION_WINDOW).contains(&remaining) {
                    target = (press.x, press.y);
                    approaching_press = remaining <= INTERCEPT_WINDOW;
                }
            }
            let motion = if latest.kind == PointerStreamKind::Drag {
                TRACK
            } else if approaching_press {
                INTERCEPT
            } else {
                GLIDE
            };
            let desired_lead = motion.friction / motion.tension.max(0.000_001);
            phase_lead += (desired_lead - phase_lead) * LEAD_SMOOTHING;
            if !approaching_press {
                if let Some(interpolated) =
                    interpolated_travel_position(&travel_samples, time + phase_lead)
                {
                    target = interpolated;
                }
            }
            x_spring.step(target.0, motion, dt);
            y_spring.step(target.1, motion, dt);

            let hidden = hide_after_inactivity
                .filter(|value| *value > 0.0)
                .is_some_and(|threshold| {
                    if travel_index < 0 {
                        return false;
                    }
                    let previous = &travel_samples[travel_index as usize];
                    let next = travel_samples.get(travel_index as usize + 1);
                    let will_move_soon = next
                        .map(|sample| sample.time - time <= REVEAL_LEAD_WINDOW)
                        .unwrap_or(false);
                    time - previous.time > threshold && !will_move_soon
                });
            opacity_spring.step(if hidden { 0.0 } else { 1.0 }, SETTLE, dt);
            blur_spring.step(if hidden { 5.0 } else { 0.0 }, SETTLE, dt);

            let press = if latest_press_index >= 0 {
                let event = &press_samples[latest_press_index as usize];
                let elapsed = time - event.time;
                (elapsed <= PULSE_DURATION).then(|| PointerPressFrame {
                    location: NormalizedPoint {
                        x: event.x,
                        y: event.y,
                    },
                    progress: (elapsed / PULSE_DURATION).clamp(0.0, 1.0),
                })
            } else {
                None
            };
            frames.push(PointerFrame {
                location: NormalizedPoint {
                    x: x_spring.position,
                    y: y_spring.position,
                },
                artwork_id: current_artwork_id.clone(),
                // Cap treats click feedback as an independent 130ms curve,
                // not another target on the cursor movement spring.
                magnification: click_scale_at(&samples, time),
                tilt_degrees: 0.0,
                opacity: opacity_spring.position.clamp(0.0, 1.0),
                blur_radius: blur_spring.position.max(0.0),
                press,
            });
        }

        let tilt_offset = ((TILT_SAMPLE_WINDOW * STEP_RATE).round() as usize).max(1);
        for index in 0..frames.len() {
            let previous = frames[index.saturating_sub(tilt_offset)].location;
            let current = frames[index].location;
            let delta_points = (current.x - previous.x) * width;
            frames[index].tilt_degrees =
                (delta_points * TILT_GAIN * TILT_WEIGHT).clamp(-20.0, 20.0);
        }

        Self {
            frames,
            duration: timeline_duration,
            artwork_by_id,
            fallback_artwork,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn frame_at(&self, time: f64) -> Option<PointerFrame> {
        let first = self.frames.first()?.clone();
        if self.frames.len() == 1 || self.duration <= 0.0 {
            return Some(first);
        }
        let position = time.clamp(0.0, self.duration) * STEP_RATE;
        let index = position as usize;
        if index >= self.frames.len() - 1 {
            return self.frames.last().cloned();
        }
        let fraction = position - index as f64;
        let left = &self.frames[index];
        let right = &self.frames[index + 1];
        Some(PointerFrame {
            location: NormalizedPoint {
                x: lerp(left.location.x, right.location.x, fraction),
                y: lerp(left.location.y, right.location.y, fraction),
            },
            artwork_id: if fraction < 0.5 {
                left.artwork_id.clone()
            } else {
                right.artwork_id.clone()
            },
            magnification: lerp(left.magnification, right.magnification, fraction),
            tilt_degrees: lerp(left.tilt_degrees, right.tilt_degrees, fraction),
            opacity: lerp(left.opacity, right.opacity, fraction),
            blur_radius: lerp(left.blur_radius, right.blur_radius, fraction),
            press: if fraction < 0.5 {
                left.press.clone()
            } else {
                right.press.clone()
            },
        })
    }

    pub fn location_at(&self, time: f64) -> Option<NormalizedPoint> {
        self.frame_at(time).map(|frame| frame.location)
    }

    pub fn artwork(&self, id: Option<&str>) -> Option<&PointerArtwork> {
        id.and_then(|id| self.artwork_by_id.get(id))
            .or(self.fallback_artwork.as_ref())
    }
}

fn interpolated_travel_position(samples: &[PointerStreamEvent], time: f64) -> Option<(f64, f64)> {
    let first = samples.first()?;
    let index = samples.partition_point(|sample| sample.time <= time);
    if index == 0 {
        return Some((first.x, first.y));
    }
    let previous = &samples[index - 1];
    let Some(next) = samples.get(index) else {
        return Some((previous.x, previous.y));
    };
    let elapsed = next.time - previous.time;
    if !elapsed.is_finite() || elapsed <= 0.0 || elapsed > IDLE_GAP_THRESHOLD {
        return Some((previous.x, previous.y));
    }
    let fraction = ((time - previous.time) / elapsed).clamp(0.0, 1.0);
    Some((
        lerp(previous.x, next.x, fraction),
        lerp(previous.y, next.y, fraction),
    ))
}

fn click_scale_at(samples: &[PointerStreamEvent], time: f64) -> f64 {
    const CLICK_DURATION: f64 = 0.13;
    const SHRINK_SIZE: f64 = 0.8;
    let is_click = |sample: &&PointerStreamEvent| {
        matches!(
            sample.kind,
            PointerStreamKind::Press | PointerStreamKind::Release
        )
    };
    let next = samples
        .iter()
        .filter(is_click)
        .find(|sample| sample.time > time);
    let click_t = if let Some(next) = next {
        if next.kind == PointerStreamKind::Press && next.time - time <= CLICK_DURATION {
            smoothstep(0.0, CLICK_DURATION, next.time - time)
        } else if next.kind == PointerStreamKind::Release {
            0.0
        } else {
            1.0
        }
    } else if let Some(previous) = samples
        .iter()
        .rev()
        .filter(is_click)
        .find(|sample| sample.time <= time)
    {
        if previous.kind == PointerStreamKind::Press {
            0.0
        } else if time - previous.time <= CLICK_DURATION {
            smoothstep(0.0, CLICK_DURATION, time - previous.time)
        } else {
            1.0
        }
    } else {
        1.0
    };
    click_t + (1.0 - click_t) * SHRINK_SIZE
}

fn smoothstep(low: f64, high: f64, value: f64) -> f64 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone)]
struct EditedPointerEvent {
    event: PointerStreamEvent,
    boundary_seed: bool,
    order: usize,
}

/// Port of Swift's cut-aware event remapping. Events removed by a cut are
/// discarded before spring integration, while a travel seed at each retained
/// clip boundary gives cursor motion a deterministic starting point.
fn timeline_samples(
    source_samples: Vec<PointerStreamEvent>,
    clip_timeline: Option<&RecordingClipTimeline>,
) -> Vec<PointerStreamEvent> {
    let Some(clip_timeline) = clip_timeline else {
        return source_samples;
    };
    if source_samples.is_empty() || clip_timeline.segments.is_empty() {
        return Vec::new();
    }

    let mut ranked = Vec::with_capacity(source_samples.len() + clip_timeline.segments.len());
    for (index, sample) in source_samples.iter().enumerate() {
        let Some(editor_time) = clip_timeline.editor_time_for_event(sample.time) else {
            continue;
        };
        let mut mapped = sample.clone();
        mapped.time = editor_time;
        ranked.push(EditedPointerEvent {
            event: mapped,
            boundary_seed: false,
            order: index,
        });
    }

    let mut slot_start = 0.0;
    for (index, segment) in clip_timeline.segments.iter().enumerate() {
        if let Some(mut seed) = source_sample_at(segment.source_start, &source_samples) {
            seed.time = slot_start + segment.gap_before;
            seed.kind = PointerStreamKind::Travel;
            seed.button = None;
            ranked.push(EditedPointerEvent {
                event: seed,
                boundary_seed: true,
                order: index,
            });
        }
        slot_start += segment.slot_duration();
    }

    ranked.sort_by(|left, right| {
        left.event
            .time
            .total_cmp(&right.event.time)
            .then_with(|| right.boundary_seed.cmp(&left.boundary_seed))
            .then_with(|| left.order.cmp(&right.order))
    });
    ranked.into_iter().map(|item| item.event).collect()
}

fn source_sample_at(time: f64, samples: &[PointerStreamEvent]) -> Option<PointerStreamEvent> {
    let first = samples.first()?;
    let upper = samples.partition_point(|sample| sample.time <= time);
    Some(if upper > 0 {
        samples[upper - 1].clone()
    } else {
        first.clone()
    })
}

fn valid_dimension(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        1000.0
    }
}

fn lerp(left: f64, right: f64, fraction: f64) -> f64 {
    left + (right - left) * fraction
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::clips::{RecordingClipSegment, RecordingClipTimeline};
    use crate::recording::model::{
        PointerPressEvent, PointerTravelKind, PointerTravelSample, PressPhase,
    };

    fn travel(time: f64, x: f64, y: f64, kind: PointerTravelKind) -> PointerTravelSample {
        PointerTravelSample {
            time,
            x,
            y,
            kind,
            artwork_id: None,
        }
    }

    fn press(time: f64, x: f64, phase: PressPhase) -> PointerPressEvent {
        PointerPressEvent {
            time,
            x,
            y: 0.5,
            button: 0,
            phase,
            artwork_id: None,
        }
    }

    fn stream_event(time: f64, x: f64, kind: PointerStreamKind) -> PointerStreamEvent {
        PointerStreamEvent {
            time,
            x,
            y: 0.5,
            kind,
            button: matches!(kind, PointerStreamKind::Press | PointerStreamKind::Release)
                .then_some(0),
            artwork_id: None,
        }
    }

    #[test]
    fn click_feedback_uses_exact_recorded_target() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.1, 0.5, PointerTravelKind::Move)],
            presses: vec![press(0.5, 0.9, PressPhase::Down)],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(capture, 2.0, 1920.0, 1080.0, None, None);
        let frame = timeline.frame_at(0.5).expect("frame");
        let effect = frame.press.expect("press effect");
        assert!((effect.location.x - 0.9).abs() < 1e-12);
        assert!(frame.location.x < effect.location.x);
        assert_eq!(effect.progress, 0.0);
    }

    #[test]
    fn press_anticipation_shrinks_cursor_before_down_event() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.5, 0.5, PointerTravelKind::Move)],
            presses: vec![
                press(0.5, 0.5, PressPhase::Down),
                press(0.7, 0.5, PressPhase::Up),
            ],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(capture, 1.5, 1000.0, 1000.0, None, None);
        assert!(timeline.frame_at(0.45).unwrap().magnification < 1.0);
        assert!(timeline.frame_at(1.2).unwrap().magnification > 0.99);
    }

    #[test]
    fn inactivity_hides_and_upcoming_travel_reveals_cursor() {
        let capture = PointerCaptureFile {
            travel: vec![
                travel(0.0, 0.2, 0.5, PointerTravelKind::Move),
                travel(2.0, 0.8, 0.5, PointerTravelKind::Move),
            ],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(capture, 3.0, 1000.0, 1000.0, None, Some(0.5));
        assert!(timeline.frame_at(1.0).unwrap().opacity < 0.1);
        assert!(timeline.frame_at(1.9).unwrap().opacity > 0.5);
    }

    #[test]
    fn interpolation_and_time_clamping_are_stable() {
        let capture = PointerCaptureFile {
            travel: vec![
                travel(0.0, 0.2, 0.5, PointerTravelKind::Move),
                travel(0.5, 0.8, 0.5, PointerTravelKind::Drag),
            ],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(capture, 1.0, 1000.0, 1000.0, None, None);
        assert_eq!(timeline.frame_at(-1.0), timeline.frame_at(0.0));
        assert_eq!(timeline.frame_at(2.0), timeline.frame_at(1.0));
        assert!(timeline.frame_at(0.75).unwrap().location.x > 0.2);
    }

    #[test]
    fn cut_events_are_removed_and_clip_boundaries_are_seeded() {
        let source = vec![
            stream_event(0.0, 0.1, PointerStreamKind::Travel),
            stream_event(2.0, 0.2, PointerStreamKind::Travel),
            stream_event(3.0, 0.9, PointerStreamKind::Press),
            stream_event(4.0, 0.4, PointerStreamKind::Travel),
        ];
        let timeline = RecordingClipTimeline::new(vec![
            RecordingClipSegment::new(0.0, 2.0),
            RecordingClipSegment::new(4.0, 5.0),
        ]);
        let mapped = timeline_samples(source, Some(&timeline));

        assert!(!mapped
            .iter()
            .any(|event| event.kind == PointerStreamKind::Press));
        assert!(mapped.iter().any(|event| {
            event.time == 2.0 && event.kind == PointerStreamKind::Travel && event.x == 0.4
        }));
        assert!(!mapped
            .iter()
            .any(|event| event.time == 2.0 && event.x == 0.2));
    }

    #[test]
    fn clip_speed_remaps_pointer_events_before_integration() {
        let source = vec![
            stream_event(0.0, 0.1, PointerStreamKind::Travel),
            stream_event(4.0, 0.8, PointerStreamKind::Travel),
        ];
        let mut segment = RecordingClipSegment::new(0.0, 6.0);
        segment.speed = 2.0;
        let timeline = RecordingClipTimeline::new(vec![segment]);
        let mapped = timeline_samples(source, Some(&timeline));

        assert!(mapped
            .iter()
            .any(|event| event.time == 2.0 && event.x == 0.8));
    }
}
