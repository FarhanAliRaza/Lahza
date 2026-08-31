use super::model::KeystrokeEvent;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPressEffectGeometry {
    pub impact_radius: f64,
    pub impact_opacity: f64,
    pub ripple_radius: f64,
    pub ripple_opacity: f64,
    pub ripple_line_width: f64,
}

pub const POINTER_PRESS_EFFECT_DURATION: f64 = 0.4;
pub const POINTER_PRESS_EFFECT_COLOR: (f64, f64, f64) = (0.0, 122.0 / 255.0, 1.0);

pub fn pointer_press_effect_geometry(
    progress: f64,
    reference_height: f64,
    cursor_scale: f64,
) -> PointerPressEffectGeometry {
    const IMPACT_DURATION: f64 = 0.12;
    const RIPPLE_DELAY: f64 = 0.06;
    let age = progress.clamp(0.0, 1.0) * POINTER_PRESS_EFFECT_DURATION;
    let base = reference_height * (21.0 / 1080.0) * cursor_scale;
    let impact_progress = (age / IMPACT_DURATION).clamp(0.0, 1.0);
    let impact_ease = ease_out_cubic(impact_progress);
    let ripple_progress =
        ((age - RIPPLE_DELAY) / (POINTER_PRESS_EFFECT_DURATION - RIPPLE_DELAY)).clamp(0.0, 1.0);
    let ripple_ease = ease_out_cubic(ripple_progress);
    PointerPressEffectGeometry {
        impact_radius: base * (0.38 + 0.34 * impact_ease),
        impact_opacity: if age <= IMPACT_DURATION {
            0.38 * (1.0 - impact_ease)
        } else {
            0.0
        },
        ripple_radius: base * (0.62 + 0.93 * ripple_ease),
        ripple_opacity: if age >= RIPPLE_DELAY {
            0.44 * (1.0 - ripple_ease)
        } else {
            0.0
        },
        ripple_line_width: (base * (0.14 - 0.07 * ripple_ease)).max(1.0),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeystrokeCaptionFrame {
    pub modifiers: Vec<String>,
    pub key: String,
    pub opacity: f64,
    pub scale: f64,
}

#[derive(Clone, Debug, Default)]
pub struct KeystrokeCaptionTimeline {
    events: Vec<KeystrokeEvent>,
}

impl KeystrokeCaptionTimeline {
    const POP_IN_DURATION: f64 = 0.16;
    const HOLD_DURATION: f64 = 1.1;
    const POP_OUT_DURATION: f64 = 0.3;

    pub fn new(events: Vec<KeystrokeEvent>) -> Self {
        let mut ranked: Vec<_> = events
            .into_iter()
            .enumerate()
            .filter(|(_, event)| event.time.is_finite() && !event.key.is_empty())
            .collect();
        ranked.sort_by(|left, right| {
            left.1
                .time
                .total_cmp(&right.1.time)
                .then_with(|| left.0.cmp(&right.0))
        });
        Self {
            events: ranked.into_iter().map(|(_, event)| event).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn frame_at(&self, time: f64) -> Option<KeystrokeCaptionFrame> {
        if self.events.is_empty() || !time.is_finite() {
            return None;
        }
        let index = self.events.partition_point(|event| event.time <= time);
        let index = index.checked_sub(1)?;
        let event = &self.events[index];
        let natural_end =
            event.time + Self::POP_IN_DURATION + Self::HOLD_DURATION + Self::POP_OUT_DURATION;
        let next_start = self
            .events
            .get(index + 1)
            .map(|event| event.time)
            .unwrap_or(f64::MAX);
        if time >= natural_end.min(next_start) {
            return None;
        }

        let previous_end = index.checked_sub(1).map(|previous| {
            self.events[previous].time
                + Self::POP_IN_DURATION
                + Self::HOLD_DURATION
                + Self::POP_OUT_DURATION
        });
        let continues_previous = previous_end.is_some_and(|end| end > event.time);
        let mut opacity: f64 = 1.0;
        let mut scale: f64 = 1.0;
        if !continues_previous && time - event.time < Self::POP_IN_DURATION {
            let progress = ((time - event.time) / Self::POP_IN_DURATION).clamp(0.0, 1.0);
            let eased = ease_out_cubic(progress);
            opacity = eased;
            scale = 0.92 + 0.08 * eased;
        }
        if natural_end <= next_start && time > natural_end - Self::POP_OUT_DURATION {
            let progress = ((natural_end - time) / Self::POP_OUT_DURATION).clamp(0.0, 1.0);
            let eased = ease_out_cubic(progress);
            opacity = opacity.min(eased);
            scale = scale.min(0.97 + 0.03 * eased);
        }
        (opacity > 0.005).then(|| KeystrokeCaptionFrame {
            modifiers: event.modifiers.clone(),
            key: event.key.clone(),
            opacity,
            scale,
        })
    }
}

fn ease_out_cubic(progress: f64) -> f64 {
    let clamped = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - clamped).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time: f64, value: &str) -> KeystrokeEvent {
        KeystrokeEvent {
            time,
            modifiers: vec!["Ctrl".into()],
            key: value.into(),
        }
    }

    #[test]
    fn press_geometry_matches_swift_phases() {
        let start = pointer_press_effect_geometry(0.0, 1080.0, 1.0);
        let middle = pointer_press_effect_geometry(0.5, 1080.0, 1.0);
        let end = pointer_press_effect_geometry(1.0, 1080.0, 1.0);
        assert!((start.impact_radius - 7.98).abs() < 0.001);
        assert!((start.impact_opacity - 0.38).abs() < 0.001);
        assert!(middle.ripple_radius > start.ripple_radius);
        assert_eq!(end.impact_opacity, 0.0);
        assert_eq!(end.ripple_opacity, 0.0);
    }

    #[test]
    fn caption_pops_in_holds_and_fades() {
        let timeline = KeystrokeCaptionTimeline::new(vec![key(1.0, "K")]);
        assert!(timeline.frame_at(0.99).is_none());
        assert!(timeline.frame_at(1.01).unwrap().opacity < 1.0);
        assert_eq!(timeline.frame_at(1.5).unwrap().opacity, 1.0);
        assert!(timeline.frame_at(2.5).unwrap().opacity < 1.0);
        assert!(timeline.frame_at(2.57).is_none());
    }

    #[test]
    fn consecutive_chord_swaps_without_second_pop() {
        let timeline = KeystrokeCaptionTimeline::new(vec![key(1.0, "K"), key(1.5, "C")]);
        let frame = timeline.frame_at(1.5).unwrap();
        assert_eq!(frame.key, "C");
        assert_eq!(frame.opacity, 1.0);
        assert_eq!(frame.scale, 1.0);
    }
}
