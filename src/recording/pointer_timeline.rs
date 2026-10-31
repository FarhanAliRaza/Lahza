use super::{
    clips::RecordingClipTimeline,
    cursor_assets::CursorShape,
    cursor_planner::{advance_scalar, CursorEventKind, CursorPath, CursorSample},
    model::{NormalizedPoint, PointerArtwork, PointerCaptureFile},
    motion::{DampedSpring, SpringConstant},
    pointer::{
        sanitize_pointer_capture, PointerSanitizeOptions, PointerStreamEvent, PointerStreamKind,
    },
};
use base64::Engine;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const STEP_RATE: f64 = 120.0;
const PULSE_DURATION: f64 = 0.4;
const REVEAL_LEAD_WINDOW: f64 = 0.25;
const GLIDE: SpringConstant = SpringConstant {
    tension: 470.0,
    friction: 70.0,
    inertia: 3.0,
};
const MEDIUM: SpringConstant = SpringConstant {
    tension: 340.0,
    friction: 60.0,
    inertia: 3.0,
};
const RAPID: SpringConstant = SpringConstant {
    tension: 530.0,
    friction: 40.0,
    inertia: 1.0,
};
const TYPING_FADE: SpringConstant = SpringConstant {
    tension: 600.0,
    friction: 50.0,
    inertia: 1.0,
};

/// Old serialized choices remain readable; the picker exposes the four current
/// styles. Existing Quick/Default/Slow projects retain their spring constants.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PointerMotion {
    None,
    #[default]
    Smooth,
    Medium,
    Rapid,
    Quick,
    Default,
    Slow,
}
impl PointerMotion {
    pub const ALL: [Self; 4] = [Self::None, Self::Smooth, Self::Medium, Self::Rapid];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Smooth | Self::Default => "Smooth",
            Self::Medium => "Medium",
            Self::Rapid => "Rapid",
            Self::Quick => "Quick",
            Self::Slow => "Slow",
        }
    }
    pub fn current_index(self) -> usize {
        match self {
            Self::None => 0,
            Self::Smooth | Self::Default | Self::Slow => 1,
            Self::Medium | Self::Quick => 2,
            Self::Rapid => 3,
        }
    }
    fn glide(self) -> Option<SpringConstant> {
        Some(match self {
            Self::None => return None,
            Self::Smooth | Self::Default => GLIDE,
            Self::Medium => MEDIUM,
            Self::Rapid => RAPID,
            Self::Quick => SpringConstant {
                tension: 720.0,
                friction: 78.0,
                inertia: 3.0,
            },
            Self::Slow => SpringConstant {
                tension: 260.0,
                friction: 54.0,
                inertia: 3.0,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PointerRotation {
    Off,
    #[default]
    Velocity,
    Directional,
}
impl PointerRotation {
    pub const ALL: [Self; 3] = [Self::Off, Self::Velocity, Self::Directional];
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Velocity => "Tilt",
            Self::Directional => "Direction",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PointerTimelineOptions {
    pub fallback_artwork: Option<PointerArtwork>,
    pub hide_after_inactivity: Option<f64>,
    pub motion: PointerMotion,
    pub loop_to_start: bool,
    pub loop_return_duration: f64,
    pub rotation: PointerRotation,
    pub rotation_strength: f64,
    pub hide_when_typing: bool,
}
impl Default for PointerTimelineOptions {
    fn default() -> Self {
        Self {
            fallback_artwork: None,
            hide_after_inactivity: None,
            motion: PointerMotion::Smooth,
            loop_to_start: false,
            loop_return_duration: 1.0,
            rotation: PointerRotation::Velocity,
            rotation_strength: 0.5,
            hide_when_typing: true,
        }
    }
}

/// Decoded, premultiplied cursor image ready to paint.
#[derive(Debug)]
pub struct PointerBitmap {
    pub id: String,
    /// Premultiplied RGBA pixels.
    pub image: RgbaImage,
    /// Hotspot as a fraction of the image size.
    pub anchor: NormalizedPoint,
    /// Size of the cursor as it appeared on screen, as a fraction of the
    /// recording width/height (independent of the image resolution).
    pub reference_width: f64,
    pub reference_height: f64,
    pub shape: Option<CursorShape>,
}

impl PointerBitmap {
    pub fn decode(artwork: &PointerArtwork) -> Option<Self> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(artwork.image_data_base64.as_bytes())
            .ok()?;
        let mut image = image::load_from_memory(&bytes).ok()?.into_rgba8();
        if image.width() == 0 || image.height() == 0 {
            return None;
        }
        for pixel in image.pixels_mut() {
            let alpha = u32::from(pixel[3]);
            for channel in &mut pixel.0[..3] {
                *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
        Some(Self {
            id: artwork.artwork_id.clone(),
            image,
            anchor: artwork.anchor_point.clamped(),
            reference_width: artwork.reference_width,
            reference_height: artwork.reference_height,
            shape: artwork.shape,
        })
    }
}

impl PartialEq for PointerBitmap {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PointerPressFrame {
    pub location: NormalizedPoint,
    pub progress: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PointerFrame {
    pub location: NormalizedPoint,
    pub artwork_id: Option<String>,
    /// Captured cursor image for this frame, if the recording has one.
    pub bitmap: Option<Arc<PointerBitmap>>,
    pub magnification: f64,
    pub tilt_degrees: f64,
    pub opacity: f64,
    pub blur_radius: f64,
    /// Smoothed cursor velocity in normalized media units per second; the
    /// painter smears the cursor along it as a motion trail.
    pub velocity: (f64, f64),
    pub press: Option<PointerPressFrame>,
}

#[derive(Clone, Debug, Default)]
pub struct PointerTimeline {
    frames: Vec<PointerFrame>,
    duration: f64,
    artwork_by_id: HashMap<String, Arc<PointerBitmap>>,
    fallback_artwork: Option<Arc<PointerBitmap>>,
    path: CursorPath,
    recording_size: (f64, f64),
    artwork_changes: Vec<(f64, Option<String>)>,
    instant_clicks: Option<Vec<(f64, bool)>>,
}

impl PointerTimeline {
    pub const STEP_RATE: f64 = STEP_RATE;

    pub fn build(
        capture: PointerCaptureFile,
        duration: f64,
        recording_width: f64,
        recording_height: f64,
        options: PointerTimelineOptions,
    ) -> Self {
        Self::build_with_clip_timeline(
            capture,
            duration,
            recording_width,
            recording_height,
            options,
            None,
        )
    }

    pub fn build_with_clip_timeline(
        capture: PointerCaptureFile,
        duration: f64,
        recording_width: f64,
        recording_height: f64,
        options: PointerTimelineOptions,
        clip_timeline: Option<&RecordingClipTimeline>,
    ) -> Self {
        let PointerTimelineOptions {
            fallback_artwork,
            hide_after_inactivity,
            motion,
            loop_to_start,
            loop_return_duration,
            rotation,
            rotation_strength,
            hide_when_typing,
        } = options;
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
        let mut typing_times: Vec<_> = stream
            .sanitized_capture
            .keystrokes
            .iter()
            .flat_map(|key| timeline_event_times(key.time, clip_timeline))
            .collect();
        typing_times.sort_by(f64::total_cmp);
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
        let artwork_by_id = stream
            .artwork
            .iter()
            .filter_map(|artwork| {
                PointerBitmap::decode(artwork)
                    .map(|bitmap| (artwork.artwork_id.clone(), Arc::new(bitmap)))
            })
            .collect();
        let fallback_artwork = fallback_artwork
            .as_ref()
            .and_then(PointerBitmap::decode)
            .map(Arc::new);
        let cuts = cut_times(clip_timeline);
        let mut points: Vec<_> = samples
            .iter()
            .enumerate()
            .map(|(index, sample)| CursorSample {
                time: sample.time,
                x: sample.x * width,
                y: sample.y * height,
                kind: match sample.kind {
                    PointerStreamKind::Travel => CursorEventKind::Travel,
                    PointerStreamKind::Drag => CursorEventKind::Drag,
                    PointerStreamKind::Press => CursorEventKind::Press,
                    PointerStreamKind::Release => CursorEventKind::Release,
                    PointerStreamKind::Hover => CursorEventKind::Hover,
                },
                artwork_change: index > 0 && sample.artwork_id != samples[index - 1].artwork_id,
            })
            .collect();
        let return_duration = if loop_return_duration.is_finite() {
            loop_return_duration.clamp(0.1, 4.0)
        } else {
            1.0
        };
        let loop_start = loop_to_start.then_some((timeline_duration - return_duration).max(0.0));
        if let Some(start) = loop_start {
            let source = source_sample_at(start, &samples).unwrap_or(first.clone());
            points.retain(|p| p.time < start);
            let steps = ((timeline_duration - start) * STEP_RATE).ceil().max(1.0) as usize;
            for index in 0..=steps {
                let fraction = index as f64 / steps as f64;
                let eased = smoothstep(0.0, 1.0, fraction);
                points.push(CursorSample {
                    time: start + (timeline_duration - start) * fraction,
                    x: lerp(source.x, first.x, eased) * width,
                    y: lerp(source.y, first.y, eased) * height,
                    kind: CursorEventKind::Travel,
                    artwork_change: false,
                });
            }
            // The last sample is a precision target so the loop closes even
            // with the slowest spring, without resetting position at playback.
            if let Some(last) = points.last_mut() {
                last.artwork_change = true;
            }
        }
        let mut artwork_changes = Vec::new();
        let mut click_states = Vec::new();
        let mut buttons = HashSet::new();
        let mut inferred_drag = false;
        for sample in &samples {
            if artwork_changes
                .last()
                .is_none_or(|(_, id)| *id != sample.artwork_id)
            {
                artwork_changes.push((sample.time, sample.artwork_id.clone()));
            }
            match sample.kind {
                PointerStreamKind::Press => {
                    buttons.insert(sample.button.unwrap_or(0));
                }
                PointerStreamKind::Release => {
                    buttons.remove(&sample.button.unwrap_or(0));
                    inferred_drag = false;
                }
                PointerStreamKind::Travel => { buttons.clear(); inferred_drag = false; }
                PointerStreamKind::Drag => inferred_drag = true,
                PointerStreamKind::Hover => {}
            }
            let pressed = !buttons.is_empty() || inferred_drag;
            if click_states
                .last()
                .is_none_or(|(_, previous)| *previous != pressed)
            {
                click_states.push((sample.time, pressed));
            }
        }
        let instant_clicks = (motion == PointerMotion::None).then_some(click_states);
        let path = CursorPath::build(&points, timeline_duration, motion.glide(), &cuts);
        let frame_count = ((timeline_duration * STEP_RATE).ceil() as usize + 1).max(2);
        let dt = 1.0 / STEP_RATE;
        let mut opacity = DampedSpring::new(if first.time <= 0.0 { 1.0 } else { 0.0 });
        let mut blur = DampedSpring::new(0.0);
        let mut click = DampedSpring::new(1.0);
        let mut angle = DampedSpring::new(0.0);
        let mut velocity_ema = 0.0;
        let mut angular_velocity: f64 = 0.0;
        let mut directional: f64 = 0.0;
        let mut sample_index = 0;
        let mut pressed_buttons = HashSet::new();
        let mut inferred_drag = false;
        let mut current_artwork_id = first.artwork_id.clone();
        let mut cut_index = 0;
        let mut latest_press = None;
        let mut frames = Vec::with_capacity(frame_count);
        let strength = if rotation_strength.is_finite() {
            rotation_strength.clamp(0.0, 1.0)
        } else {
            0.5
        };
        for index in 0..frame_count {
            let time = (index as f64 * dt).min(timeline_duration);
            if cuts.get(cut_index).is_some_and(|v| *v <= time) {
                while cuts.get(cut_index).is_some_and(|v| *v <= time) {
                    cut_index += 1;
                }
                pressed_buttons.clear();
                inferred_drag = false;
                click.snap(1.0);
                angle.snap(0.0);
                velocity_ema = 0.0;
                directional = 0.0;
                angular_velocity = 0.0;
                latest_press = None;
            }
            while sample_index < samples.len() && samples[sample_index].time <= time {
                let sample = &samples[sample_index];
                current_artwork_id = sample.artwork_id.clone();
                match sample.kind {
                    PointerStreamKind::Press => {
                        pressed_buttons.insert(sample.button.unwrap_or(0));
                        latest_press = Some(sample);
                    }
                    PointerStreamKind::Release => {
                        pressed_buttons.remove(&sample.button.unwrap_or(0));
                        inferred_drag = false;
                    }
                    PointerStreamKind::Travel => {
                        pressed_buttons.clear();
                        inferred_drag = false;
                    }
                    PointerStreamKind::Drag => inferred_drag = true,
                    PointerStreamKind::Hover => {}
                }
                sample_index += 1;
            }
            let latest = &samples[sample_index.saturating_sub(1)];
            let state = path.at(time);
            let returning = loop_start.is_some_and(|start| time >= start);
            let hidden = hide_after_inactivity
                .filter(|v| v.is_finite() && *v > 0.0)
                .is_some_and(|threshold| {
                    !returning
                        && time - latest.time > threshold
                        && !samples
                            .get(sample_index)
                            .is_some_and(|s| s.time - time <= REVEAL_LEAD_WINDOW)
                });
            let upper = typing_times.partition_point(|v| *v <= time);
            let lower = typing_times.partition_point(|v| *v < time - 1.8);
            let typing = hide_when_typing
                && upper.saturating_sub(lower) >= 3
                && typing_times
                    .get(upper.saturating_sub(1))
                    .is_some_and(|key| *key > latest.time)
                && !returning;
            advance_scalar(
                &mut opacity,
                if hidden || typing || time < first.time {
                    0.0
                } else {
                    1.0
                },
                if typing { TYPING_FADE } else { MEDIUM },
                dt,
            );
            advance_scalar(
                &mut blur,
                if hidden || typing { 10.0 } else { 0.0 },
                MEDIUM,
                dt,
            );
            let pressed = !pressed_buttons.is_empty() || inferred_drag;
            let click_target = if pressed { 0.9 } else { 1.0 };
            if let Some(spring) = motion.glide() {
                advance_scalar(&mut click, click_target, spring, dt);
            } else {
                click.snap(click_target);
            }
            velocity_ema += (state.velocity_x / 1000.0 - velocity_ema) * (1.0 - (-dt / 0.12).exp());
            let target_angle = match rotation {
                PointerRotation::Off => 0.0,
                PointerRotation::Velocity => (velocity_ema * 12.0 * strength).clamp(-20.0, 20.0),
                PointerRotation::Directional => {
                    let speed = state.velocity_x.hypot(state.velocity_y) / 1000.0;
                    if speed > 0.025 {
                        let heading = state.velocity_y.atan2(state.velocity_x).to_degrees() + 112.5;
                        let difference = (heading - directional + 180.0).rem_euclid(360.0) - 180.0;
                        angular_velocity += 0.00005
                            * speed.min(3.0)
                            * difference.to_radians().sin()
                            * (dt * 1000.0);
                    }
                    angular_velocity *= (-dt / 0.1).exp();
                    angular_velocity =
                        angular_velocity.clamp(-0.4_f64.to_radians(), 0.4_f64.to_radians());
                    directional = (directional + angular_velocity.to_degrees() * dt * 1000.0)
                        .clamp(-135.0, 135.0);
                    directional * strength
                }
            };
            let rotation_spring = if rotation == PointerRotation::Directional {
                SpringConstant {
                    tension: 320.0,
                    friction: 28.0,
                    inertia: 1.0,
                }
            } else {
                SpringConstant {
                    tension: 120.0,
                    friction: 8.0,
                    inertia: 1.0,
                }
            };
            if motion == PointerMotion::None || rotation == PointerRotation::Off {
                angle.snap(0.0);
            } else {
                advance_scalar(&mut angle, target_angle, rotation_spring, dt);
            }
            let press = latest_press.and_then(|event| {
                let elapsed = time - event.time;
                (elapsed <= PULSE_DURATION).then_some(PointerPressFrame {
                    location: NormalizedPoint {
                        x: event.x,
                        y: event.y,
                    },
                    progress: (elapsed / PULSE_DURATION).clamp(0.0, 1.0),
                })
            });
            frames.push(PointerFrame {
                location: NormalizedPoint {
                    x: state.x / width,
                    y: state.y / height,
                },
                artwork_id: current_artwork_id.clone(),
                bitmap: None,
                magnification: click.position.clamp(0.85, 1.05),
                tilt_degrees: angle.position,
                opacity: opacity.position.clamp(0.0, 1.0),
                blur_radius: blur.position.max(0.0),
                velocity: (state.velocity_x / width, state.velocity_y / height),
                press,
            });
        }
        Self {
            frames,
            duration: timeline_duration,
            artwork_by_id,
            fallback_artwork,
            path,
            recording_size: (width, height),
            artwork_changes,
            instant_clicks,
        }
    }

    pub fn recording_size(&self) -> (f64, f64) {
        self.recording_size
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Imported media has no captured pointer, even when it is moved between
    /// recording clips. Clear the cursor and click pulse over those intervals.
    pub fn restrict_to_recording(&mut self, clips: &RecordingClipTimeline, duration: f64) {
        for (index, frame) in self.frames.iter_mut().enumerate() {
            let time = index as f64 / STEP_RATE;
            if clips
                .content_source_time_at(time)
                .is_none_or(|source| source >= duration)
            {
                frame.opacity = 0.0;
                frame.press = None;
            }
        }
    }

    pub fn frame_at(&self, time: f64) -> Option<PointerFrame> {
        let time = if time.is_finite() {
            time.clamp(0.0, self.duration)
        } else {
            0.0
        };
        let mut frame = self.interpolated_frame(time)?;
        let state = self.path.at(time);
        frame.location = NormalizedPoint {
            x: state.x / self.recording_size.0,
            y: state.y / self.recording_size.1,
        };
        frame.velocity = (
            state.velocity_x / self.recording_size.0,
            state.velocity_y / self.recording_size.1,
        );
        let artwork_index = self.artwork_changes.partition_point(|(at, _)| *at <= time);
        if artwork_index > 0 {
            frame.artwork_id = self.artwork_changes[artwork_index - 1].1.clone();
        }
        if let Some(clicks) = &self.instant_clicks {
            let index = clicks.partition_point(|(at, _)| *at <= time);
            frame.magnification = if index > 0 && clicks[index - 1].1 {
                0.9
            } else {
                1.0
            };
        }
        frame.bitmap = self.artwork(frame.artwork_id.as_deref()).cloned();
        Some(frame)
    }

    fn interpolated_frame(&self, time: f64) -> Option<PointerFrame> {
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
            bitmap: None,
            magnification: lerp(left.magnification, right.magnification, fraction),
            tilt_degrees: lerp(left.tilt_degrees, right.tilt_degrees, fraction),
            opacity: lerp(left.opacity, right.opacity, fraction),
            blur_radius: lerp(left.blur_radius, right.blur_radius, fraction),
            velocity: (
                lerp(left.velocity.0, right.velocity.0, fraction),
                lerp(left.velocity.1, right.velocity.1, fraction),
            ),
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

    pub fn artwork(&self, id: Option<&str>) -> Option<&Arc<PointerBitmap>> {
        id.and_then(|id| self.artwork_by_id.get(id))
            .or(self.fallback_artwork.as_ref())
    }
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
        for editor_time in timeline_event_times(sample.time, Some(clip_timeline)) {
            let mut mapped = sample.clone();
            mapped.time = editor_time;
            ranked.push(EditedPointerEvent {
                event: mapped,
                boundary_seed: false,
                order: index,
            });
        }
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

fn timeline_event_times(time: f64, clips: Option<&RecordingClipTimeline>) -> Vec<f64> {
    let Some(clips) = clips else {
        return vec![time];
    };
    let mut result = Vec::new();
    let mut start = 0.0;
    for segment in &clips.segments {
        if time >= segment.source_start && time < segment.source_end {
            result.push(
                start + segment.gap_before + (time - segment.source_start) / segment.speed.max(0.5),
            );
        }
        start += segment.slot_duration();
    }
    result
}

fn cut_times(clips: Option<&RecordingClipTimeline>) -> Vec<f64> {
    let Some(clips) = clips else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let mut time = 0.0;
    for (index, segment) in clips.segments.iter().enumerate() {
        let previous = index.checked_sub(1).map(|i| &clips.segments[i]);
        if segment.gap_before > 0.0 {
            result.push(time);
            result.push(time + segment.gap_before);
        } else if previous.is_some_and(|previous| {
            (previous.source_end - segment.source_start).abs() > 1e-6
                || (previous.speed - segment.speed).abs() > 1e-6
                || previous.source_bounds != segment.source_bounds
        }) {
            result.push(time);
        }
        time += segment.slot_duration();
    }
    result
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
        let timeline = PointerTimeline::build(
            capture,
            2.0,
            1920.0,
            1080.0,
            PointerTimelineOptions::default(),
        );
        let frame = timeline.frame_at(0.5).expect("frame");
        let effect = frame.press.expect("press effect");
        assert!((effect.location.x - 0.9).abs() < 1e-12);
        assert!((frame.location.x - effect.location.x).abs() * 1920.0 < 2.0);
        assert_eq!(effect.progress, 0.0);
    }

    #[test]
    fn press_planner_arrives_at_recorded_target_with_bounded_anticipation() {
        let mut travel: Vec<_> = (0..=11)
            .map(|i| travel(f64::from(i) * 0.1, 0.1, 0.1, PointerTravelKind::Move))
            .collect();
        travel.push(self::travel(1.2, 0.9, 0.9, PointerTravelKind::Move));
        let capture = PointerCaptureFile {
            travel,
            presses: vec![press(1.2, 0.9, PressPhase::Down)],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(
            capture,
            1.5,
            1000.0,
            1000.0,
            PointerTimelineOptions::default(),
        );
        let x_at = |time: f64| timeline.frame_at(time).unwrap().location.x;
        // A 1.2s click permits up to 600ms of look-ahead, never more.
        assert!(
            x_at(0.59) < 0.12,
            "moved before the window opened: {}",
            x_at(0.59)
        );
        assert!(x_at(1.19) > 0.8, "not approaching click: {}", x_at(1.19));
        assert!(
            (x_at(1.2) - 0.9).abs() * 1000.0 < 2.0,
            "missed recorded click: {}",
            x_at(1.2)
        );
    }

    #[test]
    fn spring_click_feedback_follows_press_and_recovers() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.5, 0.5, PointerTravelKind::Move)],
            presses: vec![
                press(0.5, 0.5, PressPhase::Down),
                press(0.7, 0.5, PressPhase::Up),
            ],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(
            capture,
            1.5,
            1000.0,
            1000.0,
            PointerTimelineOptions::default(),
        );
        assert_eq!(timeline.frame_at(0.45).unwrap().magnification, 1.0);
        assert!(timeline.frame_at(0.65).unwrap().magnification < 0.95);
        assert!(timeline.frame_at(1.2).unwrap().magnification > 0.99);
    }

    fn click_timeline(events: Vec<PointerPressEvent>) -> PointerTimeline {
        PointerTimeline::build(
            PointerCaptureFile {
                travel: vec![travel(0.0, 0.5, 0.5, PointerTravelKind::Move)],
                presses: events,
                ..Default::default()
            },
            3.0,
            1000.0,
            1000.0,
            PointerTimelineOptions::default(),
        )
    }

    #[test]
    fn every_click_release_recovers_even_with_later_clicks() {
        let first = vec![
            press(0.5, 0.5, PressPhase::Down),
            press(0.7, 0.5, PressPhase::Up),
        ];
        let mut repeated = first.clone();
        repeated.extend([
            press(2.0, 0.5, PressPhase::Down),
            press(2.2, 0.5, PressPhase::Up),
        ]);
        let baseline = click_timeline(first);
        let actual = click_timeline(repeated);
        for step in 70..120 {
            let time = step as f64 / 100.0;
            assert_eq!(
                actual.frame_at(time).unwrap().magnification,
                baseline.frame_at(time).unwrap().magnification
            );
        }
        assert!(
            actual.frame_at(0.8).unwrap().magnification
                > actual.frame_at(0.7).unwrap().magnification
        );
        assert!(actual.frame_at(1.4).unwrap().magnification > 0.999);
    }

    #[test]
    fn rapid_clicks_preserve_continuous_size_and_recover() {
        let timeline = click_timeline(vec![
            press(0.5, 0.5, PressPhase::Down),
            press(0.6, 0.5, PressPhase::Up),
            press(0.66, 0.5, PressPhase::Down),
            press(0.8, 0.5, PressPhase::Up),
        ]);
        for boundary in [0.5, 0.6, 0.66, 0.8] {
            let before = timeline.frame_at(boundary - 1e-6).unwrap().magnification;
            let after = timeline.frame_at(boundary + 1e-6).unwrap().magnification;
            assert!((before - after).abs() < 1e-4);
        }
        assert!(timeline.frame_at(0.75).unwrap().magnification < 0.99);
        assert!(timeline.frame_at(1.5).unwrap().magnification > 0.999);
    }

    #[test]
    fn held_drag_stays_pressed_until_release_then_recovers() {
        let timeline = click_timeline(vec![
            press(0.2, 0.5, PressPhase::Down),
            press(1.2, 0.5, PressPhase::Up),
            press(2.0, 0.5, PressPhase::Down),
        ]);
        assert!((timeline.frame_at(1.1).unwrap().magnification - 0.9).abs() < 0.001);
        assert!(timeline.frame_at(1.5).unwrap().magnification > 0.99);
    }

    #[test]
    fn future_click_does_not_pull_cursor_away_from_a_held_drag() {
        let capture = PointerCaptureFile {
            travel: (0..=100)
                .map(|step| {
                    travel(
                        step as f64 / 100.0,
                        0.2,
                        0.5,
                        if step >= 10 {
                            PointerTravelKind::Drag
                        } else {
                            PointerTravelKind::Move
                        },
                    )
                })
                .collect(),
            presses: vec![
                press(0.1, 0.2, PressPhase::Down),
                press(1.0, 0.2, PressPhase::Up),
            ],
            ..Default::default()
        };
        let mut with_future_click = capture.clone();
        with_future_click
            .presses
            .push(press(1.1, 0.9, PressPhase::Down));
        let build = |capture| {
            PointerTimeline::build(
                capture,
                1.5,
                1000.0,
                1000.0,
                PointerTimelineOptions::default(),
            )
        };
        let baseline = build(capture);
        let actual = build(with_future_click);
        for step in 12..120 {
            let time = step as f64 / STEP_RATE;
            assert_eq!(
                actual.frame_at(time).unwrap().location,
                baseline.frame_at(time).unwrap().location,
                "drag changed at {time}"
            );
        }
        assert!(
            actual.frame_at(1.1).unwrap().location.x > 0.5,
            "the next click should still be anticipated after release"
        );
    }

    #[test]
    fn cutting_out_a_release_does_not_leave_the_next_clip_dragging() {
        let capture = PointerCaptureFile {
            travel: vec![
                travel(0.0, 0.2, 0.5, PointerTravelKind::Move),
                travel(0.1, 0.2, 0.5, PointerTravelKind::Drag),
                travel(0.7, 0.2, 0.5, PointerTravelKind::Move),
                travel(0.9, 0.2, 0.5, PointerTravelKind::Move),
                travel(1.4, 0.9, 0.5, PointerTravelKind::Move),
            ],
            presses: vec![
                press(0.1, 0.2, PressPhase::Down),
                press(0.7, 0.2, PressPhase::Up),
                press(1.4, 0.9, PressPhase::Down),
                press(1.5, 0.9, PressPhase::Up),
            ],
            ..Default::default()
        };
        let clips = RecordingClipTimeline::new(vec![
            RecordingClipSegment::new(0.0, 0.5),
            RecordingClipSegment::new(0.9, 2.0),
        ]);
        let timeline = PointerTimeline::build_with_clip_timeline(
            capture,
            2.0,
            1000.0,
            1000.0,
            PointerTimelineOptions::default(),
            Some(&clips),
        );
        // The second clip starts at 0.5s; its cursor should recover after
        // the removed mouse-up and anticipate its click at editor time 1s.
        assert!(timeline.frame_at(0.65).unwrap().magnification > 0.99);
        assert!((timeline.frame_at(1.0).unwrap().location.x - 0.9).abs() * 1000.0 < 2.0);
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
        let timeline = PointerTimeline::build(
            capture,
            3.0,
            1000.0,
            1000.0,
            PointerTimelineOptions {
                hide_after_inactivity: Some(0.5),
                ..Default::default()
            },
        );
        assert!(timeline.frame_at(1.0).unwrap().opacity < 0.1);
        assert!(timeline.frame_at(1.9).unwrap().opacity > 0.5);
    }

    #[test]
    fn loop_to_start_returns_cursor_to_first_position_before_the_end() {
        let capture = PointerCaptureFile {
            travel: vec![
                travel(0.0, 0.1, 0.1, PointerTravelKind::Move),
                travel(0.5, 0.9, 0.9, PointerTravelKind::Move),
            ],
            ..Default::default()
        };
        let plain = PointerTimeline::build(
            capture.clone(),
            3.0,
            1000.0,
            1000.0,
            PointerTimelineOptions::default(),
        );
        let looped = PointerTimeline::build(
            capture,
            3.0,
            1000.0,
            1000.0,
            PointerTimelineOptions {
                loop_to_start: true,
                hide_after_inactivity: Some(0.5),
                ..Default::default()
            },
        );
        assert!(plain.frame_at(3.0).unwrap().location.x > 0.85);
        let end = looped.frame_at(3.0).unwrap();
        assert!((end.location.x - 0.1).abs() < 0.03);
        assert!((end.location.y - 0.1).abs() < 0.03);
        // The return glide is treated as motion so the cursor is visible.
        assert!(end.opacity > 0.5);
        assert!(looped.frame_at(1.5).unwrap().opacity < 0.1);
    }

    #[test]
    fn motion_styles_order_how_quickly_the_cursor_follows() {
        let capture = PointerCaptureFile {
            travel: vec![
                travel(0.0, 0.0, 0.5, PointerTravelKind::Move),
                travel(0.05, 1.0, 0.5, PointerTravelKind::Move),
            ],
            ..Default::default()
        };
        let progress = |motion| {
            PointerTimeline::build(
                capture.clone(),
                1.0,
                1000.0,
                1000.0,
                PointerTimelineOptions {
                    motion,
                    ..Default::default()
                },
            )
            .frame_at(0.12)
            .unwrap()
            .location
            .x
        };
        let rapid = progress(PointerMotion::Rapid);
        let quick = progress(PointerMotion::Quick);
        let default = progress(PointerMotion::Default);
        let slow = progress(PointerMotion::Slow);
        assert!(
            rapid > quick && quick > default && default > slow,
            "{rapid} {quick} {default} {slow}"
        );
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
        let timeline = PointerTimeline::build(
            capture,
            1.0,
            1000.0,
            1000.0,
            PointerTimelineOptions::default(),
        );
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
    #[test]
    fn new_and_legacy_motion_values_round_trip() {
        for value in [
            "none", "smooth", "medium", "rapid", "quick", "default", "slow",
        ] {
            let json = format!("\"{value}\"");
            let motion: PointerMotion = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&motion).unwrap(), json);
            assert!(motion.current_index() < PointerMotion::ALL.len());
        }
    }

    #[test]
    fn none_has_exact_event_timing_and_no_rotation() {
        let capture = PointerCaptureFile {
            travel: vec![
                travel(0.0, 0.2, 0.5, PointerTravelKind::Move),
                travel(0.02, 0.8, 0.5, PointerTravelKind::Move),
            ],
            presses: vec![
                press(0.5037, 0.8, PressPhase::Down),
                press(0.7, 0.8, PressPhase::Up),
            ],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(
            capture,
            1.0,
            1000.0,
            1000.0,
            PointerTimelineOptions {
                motion: PointerMotion::None,
                ..Default::default()
            },
        );
        assert!((timeline.frame_at(0.01).unwrap().location.x - 0.5).abs() < 1e-5);
        assert_eq!(timeline.frame_at(0.50369).unwrap().magnification, 1.0);
        assert_eq!(timeline.frame_at(0.5037).unwrap().magnification, 0.9);
        assert_eq!(timeline.frame_at(0.2).unwrap().tilt_degrees, 0.0);
    }

    #[test]
    fn hover_does_not_release_an_inferred_drag_and_survives_clip_mapping() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.2, 0.5, PointerTravelKind::Move),
                travel(0.5, 0.6, 0.5, PointerTravelKind::Drag),
                travel(0.9, 0.7, 0.5, PointerTravelKind::Move)],
            hovers: vec![super::super::model::PointerHoverEvent {
                time: 0.6, x: 0.6, y: 0.5, artwork_id: None,
            }],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(capture.clone(), 1.2, 1000.0, 1000.0,
            PointerTimelineOptions { motion: PointerMotion::None, ..Default::default() });
        assert_eq!(timeline.frame_at(0.7).unwrap().magnification, 0.9);
        assert_eq!(timeline.frame_at(0.95).unwrap().magnification, 1.0);
        let stream = sanitize_pointer_capture(capture, PointerSanitizeOptions::for_recording(1000.0, 1000.0));
        let clips = RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.4, 0.8),
            RecordingClipSegment::new(0.4, 0.8)]);
        let mapped = timeline_samples(stream.samples, Some(&clips));
        let times: Vec<_> = mapped.iter().filter(|event| event.kind == PointerStreamKind::Hover)
            .map(|event| event.time).collect();
        assert_eq!(times.len(), 2);
        assert!((times[0] - 0.2).abs() < 1e-9 && (times[1] - 0.6).abs() < 1e-9);
    }

    #[test]
    fn typing_fades_cursor_until_pointer_activity_and_can_be_disabled() {
        let capture = PointerCaptureFile {
            travel: vec![
                travel(0.0, 0.2, 0.5, PointerTravelKind::Move),
                travel(1.2, 0.3, 0.5, PointerTravelKind::Move),
            ],
            keystrokes: [0.1, 0.2, 0.3]
                .into_iter()
                .map(|time| super::super::model::KeystrokeEvent {
                    time,
                    key: "Space".into(),
                    modifiers: vec![],
                })
                .collect(),
            ..Default::default()
        };
        let hidden = PointerTimeline::build(
            capture.clone(),
            2.0,
            1000.0,
            1000.0,
            PointerTimelineOptions::default(),
        );
        let visible = PointerTimeline::build(
            capture,
            2.0,
            1000.0,
            1000.0,
            PointerTimelineOptions {
                hide_when_typing: false,
                ..Default::default()
            },
        );
        assert!(hidden.frame_at(0.8).unwrap().opacity < 0.05);
        assert!(hidden.frame_at(1.8).unwrap().opacity > 0.95);
        assert_eq!(visible.frame_at(0.8).unwrap().opacity, 1.0);
    }

    #[test]
    fn rotation_modes_are_distinct_finite_and_seek_stable() {
        let capture = PointerCaptureFile {
            travel: (0..=100)
                .map(|i| {
                    travel(
                        i as f64 / 100.0,
                        0.1 + i as f64 * 0.007,
                        0.5,
                        PointerTravelKind::Move,
                    )
                })
                .collect(),
            ..Default::default()
        };
        let build = |rotation| {
            PointerTimeline::build(
                capture.clone(),
                2.0,
                1920.0,
                1080.0,
                PointerTimelineOptions {
                    rotation,
                    ..Default::default()
                },
            )
        };
        let off = build(PointerRotation::Off);
        let tilt = build(PointerRotation::Velocity);
        let direction = build(PointerRotation::Directional);
        assert_eq!(off.frame_at(0.8).unwrap().tilt_degrees, 0.0);
        assert!(tilt.frame_at(0.8).unwrap().tilt_degrees > 1.0);
        assert!(
            direction.frame_at(0.8).unwrap().tilt_degrees
                > tilt.frame_at(0.8).unwrap().tilt_degrees
        );
        let expected = direction.frame_at(0.6).unwrap();
        direction.frame_at(1.9);
        direction.frame_at(0.0);
        assert_eq!(direction.frame_at(0.6).unwrap(), expected);
        assert_eq!(tilt.recording_size(), (1920.0, 1080.0));
    }

    #[test]
    fn repeated_source_segments_repeat_pointer_and_key_events() {
        let clips = RecordingClipTimeline::new(vec![
            RecordingClipSegment::new(1.0, 2.0),
            RecordingClipSegment::new(1.0, 2.0),
        ]);
        assert_eq!(timeline_event_times(1.25, Some(&clips)), vec![0.25, 1.25]);
        let mapped = timeline_samples(
            vec![stream_event(1.25, 0.5, PointerStreamKind::Press)],
            Some(&clips),
        );
        assert_eq!(
            mapped
                .iter()
                .filter(|e| e.kind == PointerStreamKind::Press)
                .map(|e| e.time)
                .collect::<Vec<_>>(),
            vec![0.25, 1.25]
        );
    }
}
