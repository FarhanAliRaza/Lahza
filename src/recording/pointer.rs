use super::model::{
    KeystrokeEvent, PauseInterval, PointerArtwork, PointerCaptureFile, PointerTravelKind,
    PointerTravelSample, PressPhase,
};
use std::{cmp::Ordering, collections::HashSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerSanitizeOptions {
    pub recording_width: f64,
    pub recording_height: f64,
    pub minimum_travel: f64,
    pub max_sample_gap: f64,
}

impl PointerSanitizeOptions {
    pub fn for_recording(recording_width: f64, recording_height: f64) -> Self {
        Self {
            recording_width,
            recording_height,
            minimum_travel: 1.0,
            max_sample_gap: 0.25,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerStreamKind {
    Travel,
    Drag,
    Press,
    Release,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PointerStreamEvent {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub kind: PointerStreamKind,
    pub button: Option<u8>,
    pub artwork_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PointerStream {
    pub sanitized_capture: PointerCaptureFile,
    pub samples: Vec<PointerStreamEvent>,
    pub artwork: Vec<PointerArtwork>,
}

pub fn sanitize_pointer_capture(
    capture: PointerCaptureFile,
    options: PointerSanitizeOptions,
) -> PointerStream {
    if capture.is_sanitized {
        return build_stream(capture);
    }

    let artwork = valid_artwork(capture.artwork);
    let valid_artwork_ids: HashSet<_> = artwork
        .iter()
        .map(|item| item.artwork_id.as_str())
        .collect();
    let width = valid_dimension(options.recording_width);
    let height = valid_dimension(options.recording_height);

    let mut travel: Vec<_> = capture
        .travel
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut sample)| {
            let (x, y) = clamped_unit_point(sample.x, sample.y)?;
            if !sample.time.is_finite() {
                return None;
            }
            sample.time = sample.time.max(0.0);
            sample.x = x;
            sample.y = y;
            if !sample
                .artwork_id
                .as_deref()
                .is_some_and(|id| valid_artwork_ids.contains(id))
            {
                sample.artwork_id = None;
            }
            Some((index, sample))
        })
        .collect();
    travel.sort_by(|left, right| stable_time_order(left.1.time, left.0, right.1.time, right.0));
    let travel: Vec<_> = travel.into_iter().map(|(_, sample)| sample).collect();
    let travel = drop_isolated_spikes(travel, width, height);
    let travel = thin_travel_samples(
        travel,
        width,
        height,
        options.minimum_travel.max(0.0),
        options.max_sample_gap.max(1.0 / 120.0),
    );

    let mut presses: Vec<_> = capture
        .presses
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut press)| {
            let (x, y) = clamped_unit_point(press.x, press.y)?;
            if !press.time.is_finite() {
                return None;
            }
            press.time = press.time.max(0.0);
            press.x = x;
            press.y = y;
            if !press
                .artwork_id
                .as_deref()
                .is_some_and(|id| valid_artwork_ids.contains(id))
            {
                press.artwork_id = None;
            }
            Some((index, press))
        })
        .collect();
    presses.sort_by(|left, right| stable_time_order(left.1.time, left.0, right.1.time, right.0));

    let mut sanitized = PointerCaptureFile {
        format_version: PointerCaptureFile::CURRENT_FORMAT_VERSION,
        travel,
        presses: presses.into_iter().map(|(_, press)| press).collect(),
        keystrokes: sanitized_keystrokes(capture.keystrokes),
        artwork,
        pause_intervals: sanitized_pauses(capture.pause_intervals),
        is_sanitized: true,
    };
    sanitized.travel.shrink_to_fit();
    build_stream(sanitized)
}

fn build_stream(capture: PointerCaptureFile) -> PointerStream {
    let mut ordered = Vec::with_capacity(capture.travel.len() + capture.presses.len());
    for (index, sample) in capture.travel.iter().enumerate() {
        if !(sample.time.is_finite() && sample.x.is_finite() && sample.y.is_finite()) {
            continue;
        }
        ordered.push((
            PointerStreamEvent {
                time: sample.time,
                x: sample.x,
                y: sample.y,
                kind: if sample.kind == PointerTravelKind::Drag {
                    PointerStreamKind::Drag
                } else {
                    PointerStreamKind::Travel
                },
                button: None,
                artwork_id: sample.artwork_id.clone(),
            },
            index * 2,
        ));
    }
    let offset = capture.travel.len() * 2;
    for (index, press) in capture.presses.iter().enumerate() {
        if !(press.time.is_finite() && press.x.is_finite() && press.y.is_finite()) {
            continue;
        }
        ordered.push((
            PointerStreamEvent {
                time: press.time,
                x: press.x,
                y: press.y,
                kind: if press.phase == PressPhase::Down {
                    PointerStreamKind::Press
                } else {
                    PointerStreamKind::Release
                },
                button: Some(press.button),
                artwork_id: press.artwork_id.clone(),
            },
            offset + index * 2 + 1,
        ));
    }
    ordered.sort_by(|left, right| {
        left.0
            .time
            .total_cmp(&right.0.time)
            .then_with(|| stream_priority(left.0.kind).cmp(&stream_priority(right.0.kind)))
            .then_with(|| left.1.cmp(&right.1))
    });
    PointerStream {
        artwork: capture.artwork.clone(),
        sanitized_capture: capture,
        samples: ordered.into_iter().map(|(event, _)| event).collect(),
    }
}

fn thin_travel_samples(
    samples: Vec<PointerTravelSample>,
    width: f64,
    height: f64,
    minimum_travel: f64,
    max_sample_gap: f64,
) -> Vec<PointerTravelSample> {
    if samples.len() <= 2 {
        return samples;
    }
    let last = samples.last().cloned().expect("non-empty travel");
    let mut result = Vec::with_capacity(samples.len());
    result.push(samples[0].clone());
    for sample in samples.iter().skip(1).take(samples.len() - 2) {
        let previous = result.last().expect("first sample exists");
        let elapsed = sample.time - previous.time;
        let preserves_state =
            sample.kind != previous.kind || sample.artwork_id != previous.artwork_id;
        let moved = distance(previous, sample, width, height);
        if preserves_state
            || elapsed >= max_sample_gap
            || (elapsed >= 1.0 / 120.0 && moved >= minimum_travel)
        {
            result.push(sample.clone());
        }
    }
    if result.last() != Some(&last) {
        result.push(last);
    }
    result
}

fn drop_isolated_spikes(
    samples: Vec<PointerTravelSample>,
    width: f64,
    height: f64,
) -> Vec<PointerTravelSample> {
    if samples.len() <= 2 {
        return samples;
    }
    let diagonal = width.hypot(height);
    let spike_floor = 160.0_f64.max(diagonal * 0.24);
    let neighbor_ceiling = 16.0_f64.max(diagonal * 0.025);
    samples
        .iter()
        .enumerate()
        .filter(|(index, sample)| {
            if *index == 0 || *index + 1 == samples.len() {
                return true;
            }
            let previous = &samples[index - 1];
            let next = &samples[index + 1];
            if sample.kind != previous.kind
                || sample.kind != next.kind
                || sample.artwork_id != previous.artwork_id
                || sample.artwork_id != next.artwork_id
            {
                return true;
            }
            !(distance(previous, sample, width, height) > spike_floor
                && distance(sample, next, width, height) > spike_floor
                && distance(previous, next, width, height) < neighbor_ceiling)
        })
        .map(|(_, sample)| sample.clone())
        .collect()
}

fn sanitized_keystrokes(items: Vec<KeystrokeEvent>) -> Vec<KeystrokeEvent> {
    let mut ranked: Vec<_> = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut item)| {
            if !item.time.is_finite() || item.key.is_empty() {
                return None;
            }
            item.time = item.time.max(0.0);
            item.modifiers.retain(|modifier| !modifier.is_empty());
            Some((index, item))
        })
        .collect();
    ranked.sort_by(|left, right| stable_time_order(left.1.time, left.0, right.1.time, right.0));
    ranked.into_iter().map(|(_, item)| item).collect()
}

fn sanitized_pauses(items: Vec<PauseInterval>) -> Vec<PauseInterval> {
    let mut pauses: Vec<_> = items
        .into_iter()
        .filter_map(|mut item| {
            if !(item.start.is_finite() && item.end.is_finite()) {
                return None;
            }
            item.start = item.start.max(0.0);
            item.end = item.end.max(item.start);
            Some(item)
        })
        .collect();
    pauses.sort_by(|left, right| left.start.total_cmp(&right.start));
    pauses
}

fn valid_artwork(items: Vec<PointerArtwork>) -> Vec<PointerArtwork> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| {
            !item.artwork_id.is_empty()
                && seen.insert(item.artwork_id.clone())
                && !item.image_data_base64.is_empty()
                && item.anchor_point.x.is_finite()
                && item.anchor_point.y.is_finite()
                && item.reference_width.is_finite()
                && item.reference_height.is_finite()
                && item.reference_width > 0.0
                && item.reference_height > 0.0
        })
        .collect()
}

fn clamped_unit_point(x: f64, y: f64) -> Option<(f64, f64)> {
    if !(x.is_finite()
        && y.is_finite()
        && (-0.001..=1.001).contains(&x)
        && (-0.001..=1.001).contains(&y))
    {
        return None;
    }
    Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

fn valid_dimension(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        1000.0
    }
}

fn distance(
    left: &PointerTravelSample,
    right: &PointerTravelSample,
    width: f64,
    height: f64,
) -> f64 {
    ((right.x - left.x) * width).hypot((right.y - left.y) * height)
}

fn stable_time_order(
    left_time: f64,
    left_index: usize,
    right_time: f64,
    right_index: usize,
) -> Ordering {
    left_time
        .total_cmp(&right_time)
        .then_with(|| left_index.cmp(&right_index))
}

fn stream_priority(kind: PointerStreamKind) -> u8 {
    match kind {
        PointerStreamKind::Travel | PointerStreamKind::Drag => 0,
        PointerStreamKind::Press => 1,
        PointerStreamKind::Release => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn travel(time: f64, x: f64, y: f64) -> PointerTravelSample {
        PointerTravelSample {
            time,
            x,
            y,
            kind: PointerTravelKind::Move,
            artwork_id: None,
        }
    }

    #[test]
    fn sanitizer_matches_swift_order_clamping_spike_and_thinning_rules() {
        let mut capture = PointerCaptureFile::default();
        capture.travel = vec![
            travel(0.0, 0.1, 0.1),
            travel(0.01, 0.1001, 0.1),
            travel(0.02, 0.9, 0.9),
            travel(0.03, 0.1, 0.1),
            travel(0.4, 1.0005, -0.0005),
            travel(f64::NAN, 0.2, 0.2),
            travel(0.5, 1.2, 0.2),
        ];
        let stream = sanitize_pointer_capture(
            capture,
            PointerSanitizeOptions::for_recording(1920.0, 1080.0),
        );
        assert!(stream.sanitized_capture.is_sanitized);
        assert_eq!(stream.sanitized_capture.format_version, 1);
        assert_eq!(stream.sanitized_capture.travel.len(), 2);
        assert_eq!(stream.sanitized_capture.travel[0].x, 0.1);
        assert_eq!(stream.sanitized_capture.travel[1].x, 1.0);
        assert_eq!(stream.sanitized_capture.travel[1].y, 0.0);
    }

    #[test]
    fn stream_orders_travel_before_press_before_release_at_same_time() {
        let mut capture = PointerCaptureFile::default();
        capture.travel.push(travel(1.0, 0.5, 0.5));
        capture.presses.extend([
            crate::recording::model::PointerPressEvent {
                time: 1.0,
                x: 0.5,
                y: 0.5,
                button: 0,
                phase: PressPhase::Up,
                artwork_id: None,
            },
            crate::recording::model::PointerPressEvent {
                time: 1.0,
                x: 0.5,
                y: 0.5,
                button: 0,
                phase: PressPhase::Down,
                artwork_id: None,
            },
        ]);
        let stream = sanitize_pointer_capture(
            capture,
            PointerSanitizeOptions::for_recording(1000.0, 1000.0),
        );
        assert_eq!(
            stream
                .samples
                .iter()
                .map(|sample| sample.kind)
                .collect::<Vec<_>>(),
            vec![
                PointerStreamKind::Travel,
                PointerStreamKind::Press,
                PointerStreamKind::Release
            ]
        );
    }
}
