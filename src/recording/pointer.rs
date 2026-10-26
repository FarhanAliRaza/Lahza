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
    Hover,
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
    let travel = drop_hand_shake(travel);
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

    // Target transitions are accuracy constraints, not travel samples: keep
    // stationary transitions and never pass them through movement thinning.
    let mut hovers: Vec<_> = capture.hovers.into_iter().enumerate().filter_map(|(index, mut hover)| {
        let (x, y) = clamped_unit_point(hover.x, hover.y)?;
        if !hover.time.is_finite() { return None; }
        hover.time = hover.time.max(0.0);
        hover.x = x;
        hover.y = y;
        if !hover.artwork_id.as_deref().is_some_and(|id| valid_artwork_ids.contains(id)) {
            hover.artwork_id = None;
        }
        Some((index, hover))
    }).collect();
    hovers.sort_by(|left, right| stable_time_order(left.1.time, left.0, right.1.time, right.0));

    let mut sanitized = PointerCaptureFile {
        format_version: PointerCaptureFile::CURRENT_FORMAT_VERSION,
        travel,
        presses: presses.into_iter().map(|(_, press)| press).collect(),
        hovers: hovers.into_iter().map(|(_, hover)| hover).collect(),
        keystrokes: sanitized_keystrokes(capture.keystrokes),
        artwork,
        pause_intervals: sanitized_pauses(capture.pause_intervals),
        is_sanitized: true,
    };
    sanitized.travel.shrink_to_fit();
    build_stream(sanitized)
}

fn build_stream(capture: PointerCaptureFile) -> PointerStream {
    let mut ordered = Vec::with_capacity(capture.travel.len() + capture.presses.len() + capture.hovers.len());
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
    let offset = (capture.travel.len() + capture.presses.len()) * 2;
    for (index, hover) in capture.hovers.iter().enumerate() {
        if !(hover.time.is_finite() && hover.x.is_finite() && hover.y.is_finite()) { continue; }
        ordered.push((PointerStreamEvent {
            time: hover.time, x: hover.x, y: hover.y, kind: PointerStreamKind::Hover,
            button: None, artwork_id: hover.artwork_id.clone(),
        }, offset + index));
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

/// Largest normalized displacement that still counts as a shake.
const SHAKE_THRESHOLD: f64 = 0.015;
/// Three samples closer together than this can form a shake.
const SHAKE_WINDOW: f64 = 0.1;

/// Removes the middle sample of small, rapid direction reversals so a
/// hesitant hand reads as one deliberate glide (Cap's shake filter).
/// Only ordinary moves are filtered. Drag samples and transitions into or
/// out of a drag stay intact; clicks are stored separately.
fn drop_hand_shake(samples: Vec<PointerTravelSample>) -> Vec<PointerTravelSample> {
    if samples.len() < 3 {
        return samples;
    }
    let last = samples.last().cloned().expect("non-empty travel");
    let mut result = Vec::with_capacity(samples.len());
    result.push(samples[0].clone());
    for (index, current) in samples.iter().enumerate().skip(1).take(samples.len() - 2) {
        let previous = result.last().expect("first sample exists");
        let next = &samples[index + 1];
        let same_state = current.kind == PointerTravelKind::Move
            && current.kind == previous.kind
            && current.kind == next.kind
            && current.artwork_id == previous.artwork_id
            && current.artwork_id == next.artwork_id;
        if same_state && next.time - previous.time <= SHAKE_WINDOW {
            let to_current = (current.x - previous.x, current.y - previous.y);
            let to_next = (next.x - current.x, next.y - current.y);
            let reversal = to_current.0 * to_next.0 + to_current.1 * to_next.1 < 0.0;
            let small = to_current.0.hypot(to_current.1) < SHAKE_THRESHOLD
                && to_next.0.hypot(to_next.1) < SHAKE_THRESHOLD;
            if reversal && small {
                continue;
            }
        }
        result.push(current.clone());
    }
    result.push(last);
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
        PointerStreamKind::Hover => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stationary_hover_constraints_survive_thinning_and_invalid_data_is_removed() {
        use super::super::model::PointerHoverEvent;
        let hover = |time, x| PointerHoverEvent { time, x, y: 0.5, artwork_id: Some("unknown".into()) };
        let mut capture = PointerCaptureFile::default();
        capture.travel = vec![travel(0.0, 0.5, 0.5), travel(1.0, 0.5, 0.5)];
        capture.hovers = vec![hover(0.4, 0.5), hover(0.2, 0.5), hover(f64::NAN, 0.5),
            hover(0.3, 1.0001), hover(0.6, 1.5)];
        let stream = sanitize_pointer_capture(capture, PointerSanitizeOptions::for_recording(1920.0, 1080.0));
        let hovers: Vec<_> = stream.samples.iter().filter(|event| event.kind == PointerStreamKind::Hover).collect();
        assert_eq!(hovers.iter().map(|event| event.time).collect::<Vec<_>>(), vec![0.2, 0.3, 0.4]);
        assert_eq!(hovers[1].x, 1.0);
        assert!(hovers.iter().all(|event| event.artwork_id.is_none() && event.button.is_none()));
        assert_eq!(stream.sanitized_capture.hovers.len(), 3);
        let again = sanitize_pointer_capture(stream.sanitized_capture.clone(), PointerSanitizeOptions::for_recording(1920.0, 1080.0));
        assert_eq!(again.samples, stream.samples);
    }

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
        assert_eq!(stream.sanitized_capture.format_version, PointerCaptureFile::CURRENT_FORMAT_VERSION);
        assert_eq!(stream.sanitized_capture.travel.len(), 2);
        assert_eq!(stream.sanitized_capture.travel[0].x, 0.1);
        assert_eq!(stream.sanitized_capture.travel[1].x, 1.0);
        assert_eq!(stream.sanitized_capture.travel[1].y, 0.0);
    }

    #[test]
    fn small_rapid_reversals_are_dropped_but_deliberate_ones_stay() {
        let mut capture = PointerCaptureFile::default();
        capture.travel = vec![
            travel(0.0, 0.5, 0.5),
            // A jitter step back and forth within 100ms.
            travel(0.02, 0.505, 0.5),
            travel(0.04, 0.5, 0.5),
            // A deliberate reversal: too large to be a shake.
            travel(0.06, 0.6, 0.5),
            travel(0.08, 0.5, 0.5),
            // A slow reversal outside the shake window.
            travel(0.3, 0.505, 0.5),
            travel(0.5, 0.5, 0.5),
        ];
        let stream = sanitize_pointer_capture(
            capture,
            PointerSanitizeOptions::for_recording(1920.0, 1080.0),
        );
        let times: Vec<_> = stream
            .sanitized_capture
            .travel
            .iter()
            .map(|sample| sample.time)
            .collect();
        assert!(!times.contains(&0.02), "{times:?}");
        assert!(times.contains(&0.06), "{times:?}");
        assert!(times.contains(&0.3), "{times:?}");
    }

    #[test]
    fn sanitizer_preserves_small_rapid_drag_reversals() {
        let samples: Vec<_> = [(0.0, 0.5), (0.02, 0.505), (0.04, 0.5)]
            .into_iter()
            .map(|(time, x)| PointerTravelSample {
                kind: PointerTravelKind::Drag,
                ..travel(time, x, 0.5)
            })
            .collect();
        let capture = PointerCaptureFile {
            travel: samples.clone(),
            ..Default::default()
        };
        let stream = sanitize_pointer_capture(
            capture,
            PointerSanitizeOptions::for_recording(1920.0, 1080.0),
        );
        assert_eq!(stream.sanitized_capture.travel, samples);
        assert!(stream.samples.iter().all(|sample| sample.kind == PointerStreamKind::Drag));
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
