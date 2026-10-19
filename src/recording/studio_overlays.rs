//! Input feedback and narration drawn above the finished scene: keystroke
//! captions, the speed badge, and the subtitle bar. Layout lives here and is
//! shared verbatim by the GPUI preview (which paints the shapes directly) and
//! the exporter (which rasterizes the same shapes through SVG), so both agree
//! at any canvas size. Every metric derives from the card or canvas size.

use super::clips::RecordingClipTimeline;
use super::model::KeystrokeEvent;
use super::overlays::KeystrokeCaptionTimeline;
use super::scene::Rect;
use super::transcript::{KaraokeTimeline, SubtitleTimeline, Transcript};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::sync::Arc;

/// Where a corner-pinned overlay sits on the recording card.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayPlacement {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

impl OverlayPlacement {
    pub const TOP: [OverlayPlacement; 3] = [Self::TopLeft, Self::TopCenter, Self::TopRight];
    pub const BOTTOM: [OverlayPlacement; 3] =
        [Self::BottomLeft, Self::BottomCenter, Self::BottomRight];

    pub fn is_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopCenter | Self::TopRight)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft | Self::BottomLeft => "Left",
            Self::TopCenter | Self::BottomCenter => "Center",
            Self::TopRight | Self::BottomRight => "Right",
        }
    }

    /// Top-left of a `size` box pinned inside `card` with `margin`.
    fn origin(self, size: (f64, f64), card: Rect, margin: f64) -> (f64, f64) {
        let x = match self {
            Self::TopLeft | Self::BottomLeft => card.x + margin,
            Self::TopCenter | Self::BottomCenter => card.x + (card.width - size.0) * 0.5,
            Self::TopRight | Self::BottomRight => card.right() - margin - size.0,
        };
        let y = if self.is_top() {
            card.y + margin
        } else {
            card.bottom() - margin - size.1
        };
        (x, y)
    }
}

/// Keystroke caption and speed badge settings, saved with the scene.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InputOverlayStyle {
    pub keystrokes: bool,
    pub keystroke_placement: OverlayPlacement,
    pub speed_badge: bool,
    pub speed_badge_placement: OverlayPlacement,
}

impl Default for InputOverlayStyle {
    fn default() -> Self {
        Self {
            keystrokes: true,
            keystroke_placement: OverlayPlacement::BottomCenter,
            speed_badge: false,
            speed_badge_placement: OverlayPlacement::TopRight,
        }
    }
}

/// Subtitle bar appearance. The bar is always centered horizontally; only
/// its height on the canvas and its text size are adjustable.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubtitleStyle {
    pub visible: bool,
    /// Center of the bar along the canvas height, 0 top to 1 bottom.
    pub vertical_position: f64,
    /// Multiplier on the default text size; paddings follow it.
    pub font_scale: f64,
    /// Highlight the word being spoken (needs word timings).
    pub karaoke: bool,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            visible: true,
            vertical_position: 0.92,
            font_scale: 1.0,
            karaoke: false,
        }
    }
}

impl SubtitleStyle {
    pub const VERTICAL_RANGE: (f64, f64) = (0.1, 0.95);
    pub const FONT_SCALE_RANGE: (f64, f64) = (0.6, 1.8);

    fn vertical(self) -> f64 {
        finite_or(self.vertical_position, 0.92).clamp(Self::VERTICAL_RANGE.0, Self::VERTICAL_RANGE.1)
    }

    fn font_scale(self) -> f64 {
        finite_or(self.font_scale, 1.0).clamp(Self::FONT_SCALE_RANGE.0, Self::FONT_SCALE_RANGE.1)
    }
}

/// One primitive of an overlay, in canvas pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayShape {
    /// Rounded rectangle filled with black.
    Pill { rect: Rect, radius: f64, alpha: f64 },
    /// A run of bold caption text. `y` is the top of its line box.
    Text {
        x: f64,
        y: f64,
        size: f64,
        line_height: f64,
        baseline: f64,
        text: String,
        color: [u8; 3],
        alpha: f64,
    },
    /// An open chevron pointing right, stroked in white.
    Chevron { points: [(f64, f64); 3], width: f64, alpha: f64 },
}

const BACKGROUND_ALPHA: f64 = 0.62;
const MODIFIER_ALPHA: f64 = 0.7;
const SUBTITLE_BACKGROUND_ALPHA: f64 = 0.78;
const KARAOKE_ACCENT: [u8; 3] = [255, 214, 10];
const KARAOKE_UPCOMING_ALPHA: f64 = 0.55;
const WHITE: [u8; 3] = [255, 255, 255];
/// Swift's 11 pt floor at 1080p, kept proportional so the preview matches export.
const MINIMUM_FONT_FRACTION: f64 = 11.0 / 1080.0;

/// Text metrics from the same shaper the annotation text uses.
#[derive(Clone, Copy, Debug)]
struct Metrics {
    width: f64,
    line_height: f64,
    baseline: f64,
}

fn measure(text: &str, size: f64) -> Metrics {
    let mark = lahza_annotations::AnnotationMark {
        tool: lahza_annotations::Tool::Text,
        text: text.to_string(),
        font_size: size as f32,
        bold: true,
        text_auto_width: true,
        ..Default::default()
    };
    let layout = lahza_annotations::text::layout(&mark, 0.0);
    let width = if text.is_empty() {
        0.0
    } else {
        layout.lines.iter().map(|line| line.width).fold(0.0, f32::max) as f64
    };
    Metrics {
        width,
        line_height: layout.line_height as f64,
        baseline: layout.baseline as f64,
    }
}

/// Shared pill metrics for the keystroke caption and the speed badge.
struct PillMetrics {
    font_size: f64,
    padding_x: f64,
    padding_y: f64,
    radius: f64,
    margin: f64,
}

impl PillMetrics {
    fn new(card: Rect, canvas_height: f64) -> Self {
        let font_size = (card.height * 0.033).max(canvas_height * MINIMUM_FONT_FRACTION);
        Self {
            font_size,
            padding_x: font_size * 0.72,
            padding_y: font_size * 0.46,
            radius: font_size * 0.55,
            margin: card.height * 0.045,
        }
    }
}

/// Sped-up stretches of the edited timeline. Neighbouring clips at the same
/// speed merge, so a split does not make the badge blink.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeedBadgeTimeline {
    spans: Vec<(f64, f64, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedBadgeFrame {
    pub speed: f64,
    /// Entrance and exit, 0-1.
    pub presence: f64,
    pub time: f64,
}

impl SpeedBadgeFrame {
    const SWEEP_PERIOD: f64 = 0.9;

    /// Brightness of chevron `index`: a highlight sweeps left to right.
    pub fn chevron_alpha(&self, index: usize) -> f64 {
        let phase = self.time / Self::SWEEP_PERIOD;
        let local = (phase - index as f64 * 0.2).rem_euclid(1.0);
        let window = 0.45;
        let bump = if local < window {
            (std::f64::consts::PI * local / window).sin()
        } else {
            0.0
        };
        0.32 + 0.68 * bump
    }
}

impl SpeedBadgeTimeline {
    const FADE_IN: f64 = 0.22;
    const FADE_OUT: f64 = 0.18;

    pub fn new(clips: &RecordingClipTimeline) -> Self {
        let mut spans: Vec<(f64, f64, f64)> = Vec::new();
        let mut slot = 0.0;
        for segment in &clips.segments {
            let start = slot + segment.gap_before;
            let end = start + segment.editor_duration();
            slot += segment.slot_duration();
            if segment.speed <= 1.001 || end <= start {
                continue;
            }
            match spans.last_mut() {
                Some(last) if (last.2 - segment.speed).abs() < 0.001 && (last.1 - start).abs() < 0.0001 => {
                    last.1 = end;
                }
                _ => spans.push((start, end, segment.speed)),
            }
        }
        Self { spans }
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub fn frame_at(&self, time: f64) -> Option<SpeedBadgeFrame> {
        let &(start, end, speed) = self
            .spans
            .iter()
            .find(|(start, end, _)| time >= *start && time < *end)?;
        let linear = ((time - start) / Self::FADE_IN)
            .min((end - time) / Self::FADE_OUT)
            .clamp(0.0, 1.0);
        Some(SpeedBadgeFrame {
            speed,
            presence: linear * linear * (3.0 - 2.0 * linear),
            time,
        })
    }
}

/// "2×", "1.5×": the badge and clip-lane title for a speed.
pub fn speed_title(speed: f64) -> String {
    let rounded = (speed * 100.0).round() / 100.0;
    let mut text = format!("{rounded:.2}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    format!("{text}×")
}

/// Everything drawn above the scene for one project, built once per edit and
/// sampled per frame in editor time.
#[derive(Clone, Debug, Default)]
pub struct StudioOverlays {
    input: InputOverlayStyle,
    subtitle_style: SubtitleStyle,
    keystrokes: KeystrokeCaptionTimeline,
    speed: SpeedBadgeTimeline,
    subtitles: SubtitleTimeline,
    karaoke: KaraokeTimeline,
}

impl StudioOverlays {
    /// `keystrokes` and `transcript` are on the source timeline; they are
    /// mapped through `clips` so cuts, speed changes and reordering apply.
    pub fn new(
        input: InputOverlayStyle,
        subtitle_style: SubtitleStyle,
        keystrokes: &[KeystrokeEvent],
        transcript: Option<&Transcript>,
        clips: &RecordingClipTimeline,
    ) -> Self {
        let keystrokes = if input.keystrokes {
            KeystrokeCaptionTimeline::new(
                keystrokes
                    .iter()
                    .filter_map(|event| {
                        Some(KeystrokeEvent {
                            time: clips.editor_time_for_event(event.time)?,
                            ..event.clone()
                        })
                    })
                    .collect(),
            )
        } else {
            KeystrokeCaptionTimeline::default()
        };
        let speed = if input.speed_badge {
            SpeedBadgeTimeline::new(clips)
        } else {
            SpeedBadgeTimeline::default()
        };
        let (subtitles, karaoke) = match transcript.filter(|_| subtitle_style.visible) {
            Some(transcript) => {
                let edited = transcript.in_editor_time(clips);
                (
                    SubtitleTimeline::new(edited.cues.clone()),
                    if subtitle_style.karaoke {
                        KaraokeTimeline::new(&edited.cues, &edited.words)
                    } else {
                        KaraokeTimeline::default()
                    },
                )
            }
            None => Default::default(),
        };
        Self {
            input,
            subtitle_style,
            keystrokes,
            speed,
            subtitles,
            karaoke,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keystrokes.is_empty() && self.speed.is_empty() && self.subtitles.is_empty()
    }

    /// The shapes visible at `time` on a `canvas`, with the recording card at
    /// `card` (its untransformed rect, so zooms do not move the captions).
    pub fn shapes_at(&self, time: f64, canvas: (f64, f64), card: Rect) -> Vec<OverlayShape> {
        let mut shapes = Vec::new();
        if !time.is_finite() || canvas.0 <= 0.0 || canvas.1 <= 0.0 {
            return shapes;
        }
        let metrics = PillMetrics::new(card, canvas.1);
        let mut keystroke_box = None;
        if let Some(frame) = self.keystrokes.frame_at(time) {
            keystroke_box = Some(self.keystroke_shapes(&frame, &metrics, card, &mut shapes));
        }
        if let Some(frame) = self.speed.frame_at(time) {
            self.speed_badge_shapes(frame, &metrics, card, keystroke_box, &mut shapes);
        }
        self.subtitle_shapes(time, canvas, &mut shapes);
        shapes
    }

    fn keystroke_shapes(
        &self,
        frame: &super::overlays::KeystrokeCaptionFrame,
        metrics: &PillMetrics,
        card: Rect,
        shapes: &mut Vec<OverlayShape>,
    ) -> Rect {
        let modifiers = frame.modifiers.join(" ");
        let modifiers = if modifiers.is_empty() {
            modifiers
        } else {
            modifiers + " "
        };
        let size = metrics.font_size;
        let lead = measure(&modifiers, size);
        let key = measure(&frame.key, size);
        let width = lead.width + key.width + metrics.padding_x * 2.0;
        let height = key.line_height + metrics.padding_y * 2.0;
        let (x, y) = self
            .input
            .keystroke_placement
            .origin((width, height), card, metrics.margin);
        let rect = Rect { x, y, width, height };
        let mut local = vec![OverlayShape::Pill {
            rect,
            radius: metrics.radius.min(height * 0.5),
            alpha: BACKGROUND_ALPHA,
        }];
        let text_y = y + metrics.padding_y;
        if !modifiers.is_empty() {
            local.push(OverlayShape::Text {
                x: x + metrics.padding_x,
                y: text_y,
                size,
                line_height: key.line_height,
                baseline: key.baseline,
                text: modifiers,
                color: WHITE,
                alpha: MODIFIER_ALPHA,
            });
        }
        local.push(OverlayShape::Text {
            x: x + metrics.padding_x + lead.width,
            y: text_y,
            size,
            line_height: key.line_height,
            baseline: key.baseline,
            text: frame.key.clone(),
            color: WHITE,
            alpha: 1.0,
        });
        shapes.extend(transform(local, rect, frame.scale, frame.opacity));
        rect
    }

    fn speed_badge_shapes(
        &self,
        frame: SpeedBadgeFrame,
        metrics: &PillMetrics,
        card: Rect,
        keystroke_box: Option<Rect>,
        shapes: &mut Vec<OverlayShape>,
    ) {
        let size = metrics.font_size;
        let title = speed_title(frame.speed);
        let text = measure(&title, size);
        let gap = size * 0.42;
        let chevron_height = size * 0.62;
        let chevron_width = chevron_height * 0.5;
        let advance = chevron_width + size * 0.14;
        let chevrons = advance * 2.0 + chevron_width;
        let width = metrics.padding_x * 2.0 + text.width + gap + chevrons;
        let height = text.line_height + metrics.padding_y * 2.0;
        let placement = self.input.speed_badge_placement;
        let (x, mut y) = placement.origin((width, height), card, metrics.margin);
        // Sharing a corner with a visible keystroke caption: stack inward.
        if let Some(other) = keystroke_box.filter(|_| placement == self.input.keystroke_placement) {
            let step = other.height + metrics.margin * 0.35;
            y += if placement.is_top() { step } else { -step };
        }
        let rect = Rect { x, y, width, height };
        let mut local = vec![
            OverlayShape::Pill {
                rect,
                radius: metrics.radius.min(height * 0.5),
                alpha: BACKGROUND_ALPHA,
            },
            OverlayShape::Text {
                x: x + metrics.padding_x,
                y: y + metrics.padding_y,
                size,
                line_height: text.line_height,
                baseline: text.baseline,
                text: title,
                color: WHITE,
                alpha: 1.0,
            },
        ];
        let first = x + metrics.padding_x + text.width + gap;
        let mid = y + height * 0.5;
        for index in 0..3 {
            let left = first + advance * index as f64;
            local.push(OverlayShape::Chevron {
                points: [
                    (left, mid - chevron_height * 0.5),
                    (left + chevron_width, mid),
                    (left, mid + chevron_height * 0.5),
                ],
                width: (size * 0.13).max(1.0),
                alpha: frame.chevron_alpha(index),
            });
        }
        let scale = 0.85 + 0.15 * frame.presence;
        shapes.extend(transform(local, rect, scale, frame.presence));
    }

    fn subtitle_shapes(&self, time: f64, canvas: (f64, f64), shapes: &mut Vec<OverlayShape>) {
        let Some(cue) = self.subtitles.cue_at(time) else {
            return;
        };
        let karaoke = self.subtitle_style.karaoke.then(|| self.karaoke.line_at(time)).flatten();
        let words: Vec<String> = match &karaoke {
            Some(line) => line.words.clone(),
            None => cue.text.split_whitespace().map(str::to_string).collect(),
        };
        if words.is_empty() {
            return;
        }
        let (width, height) = canvas;
        let reference = width.min(height);
        let floor = height * MINIMUM_FONT_FRACTION;
        let mut size = (reference * 0.032 * self.subtitle_style.font_scale()).max(floor);
        let mut lines;
        let mut widths;
        let mut space;
        let mut word_metrics;
        // Wrap into centered lines; shrink only when three lines cannot hold it.
        loop {
            let padding_x = size * 0.85;
            let limit = (width * 0.92 - padding_x * 2.0).max(24.0);
            space = measure(" ", size).width;
            word_metrics = words.iter().map(|word| measure(word, size)).collect::<Vec<_>>();
            lines = Vec::<std::ops::Range<usize>>::new();
            widths = Vec::new();
            let mut start = 0;
            let mut line_width = 0.0;
            for (index, metrics) in word_metrics.iter().enumerate() {
                let next = if index == start {
                    metrics.width
                } else {
                    line_width + space + metrics.width
                };
                if index > start && next > limit {
                    lines.push(start..index);
                    widths.push(line_width);
                    start = index;
                    line_width = metrics.width;
                } else {
                    line_width = next;
                }
            }
            lines.push(start..words.len());
            widths.push(line_width);
            if lines.len() <= 3 || size <= floor {
                break;
            }
            size = (size * 3.0 / lines.len() as f64).max(floor);
        }
        let line = measure("Ag", size);
        let padding_x = size * 0.85;
        let padding_y = size * 0.5;
        let advance = line.line_height * 1.12;
        let text_height = line.line_height + advance * (lines.len() - 1) as f64;
        let bar_width = widths.iter().copied().fold(0.0, f64::max) + padding_x * 2.0;
        let bar_height = text_height + padding_y * 2.0;
        let center_y = height * self.subtitle_style.vertical();
        let bar = Rect {
            x: (width - bar_width) * 0.5,
            y: center_y - bar_height * 0.5,
            width: bar_width,
            height: bar_height,
        };
        shapes.push(OverlayShape::Pill {
            rect: bar,
            radius: (size * 0.6).min(bar_height * 0.5),
            alpha: SUBTITLE_BACKGROUND_ALPHA,
        });
        for (row, range) in lines.iter().enumerate() {
            let y = bar.y + padding_y + advance * row as f64;
            let mut x = width * 0.5 - widths[row] * 0.5;
            match &karaoke {
                None => shapes.push(OverlayShape::Text {
                    x,
                    y,
                    size,
                    line_height: line.line_height,
                    baseline: line.baseline,
                    text: words[range.clone()].join(" "),
                    color: WHITE,
                    alpha: 1.0,
                }),
                Some(state) => {
                    for index in range.clone() {
                        let (color, alpha) = if state.active == Some(index) {
                            (KARAOKE_ACCENT, 1.0)
                        } else if index < state.spoken {
                            (WHITE, 1.0)
                        } else {
                            (WHITE, KARAOKE_UPCOMING_ALPHA)
                        };
                        shapes.push(OverlayShape::Text {
                            x,
                            y,
                            size,
                            line_height: line.line_height,
                            baseline: line.baseline,
                            text: words[index].clone(),
                            color,
                            alpha,
                        });
                        x += word_metrics[index].width + space;
                    }
                }
            }
        }
    }
}

/// Scale `shapes` about the center of `around` and fade them by `opacity`.
fn transform(shapes: Vec<OverlayShape>, around: Rect, scale: f64, opacity: f64) -> Vec<OverlayShape> {
    let cx = around.x + around.width * 0.5;
    let cy = around.y + around.height * 0.5;
    let at = |x: f64, y: f64| (cx + (x - cx) * scale, cy + (y - cy) * scale);
    shapes
        .into_iter()
        .map(|shape| match shape {
            OverlayShape::Pill { rect, radius, alpha } => {
                let (x, y) = at(rect.x, rect.y);
                OverlayShape::Pill {
                    rect: Rect {
                        x,
                        y,
                        width: rect.width * scale,
                        height: rect.height * scale,
                    },
                    radius: radius * scale,
                    alpha: alpha * opacity,
                }
            }
            OverlayShape::Text {
                x,
                y,
                size,
                line_height,
                baseline,
                text,
                color,
                alpha,
            } => {
                let (x, y) = at(x, y);
                OverlayShape::Text {
                    x,
                    y,
                    size: size * scale,
                    line_height: line_height * scale,
                    baseline: baseline * scale,
                    text,
                    color,
                    alpha: alpha * opacity,
                }
            }
            OverlayShape::Chevron { points, width, alpha } => OverlayShape::Chevron {
                points: points.map(|(x, y)| at(x, y)),
                width: width * scale,
                alpha: alpha * opacity,
            },
        })
        .collect()
}

/// Font stack matching the preview's bold caption face.
const FONT_FAMILY: &str = "Noto Sans, Inter, DejaVu Sans, Liberation Sans, Cantarell, sans-serif";

/// The shapes as an SVG document whose origin is `origin` on the canvas.
fn svg_document(shapes: &[OverlayShape], origin: (f64, f64), width: u32, height: u32) -> String {
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><g transform=\"translate({:.3} {:.3})\">",
        -origin.0, -origin.1
    );
    for shape in shapes {
        match shape {
            OverlayShape::Pill { rect, radius, alpha } => {
                let _ = write!(
                    svg,
                    "<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" rx=\"{radius:.3}\" fill=\"#000\" fill-opacity=\"{alpha:.4}\"/>",
                    rect.x, rect.y, rect.width, rect.height
                );
            }
            OverlayShape::Text {
                x,
                y,
                size,
                baseline,
                text,
                color,
                alpha,
                ..
            } => {
                let _ = write!(
                    svg,
                    "<text x=\"{x:.3}\" y=\"{:.3}\" xml:space=\"preserve\" font-family=\"{FONT_FAMILY}\" font-weight=\"700\" font-size=\"{size:.3}\" fill=\"#{:02x}{:02x}{:02x}\" fill-opacity=\"{alpha:.4}\">{}</text>",
                    y + baseline,
                    color[0],
                    color[1],
                    color[2],
                    super::scene::xml_escape(text)
                );
            }
            OverlayShape::Chevron { points, width, alpha } => {
                let _ = write!(
                    svg,
                    "<polyline points=\"{:.3},{:.3} {:.3},{:.3} {:.3},{:.3}\" fill=\"none\" stroke=\"#fff\" stroke-opacity=\"{alpha:.4}\" stroke-width=\"{width:.3}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
                    points[0].0, points[0].1, points[1].0, points[1].1, points[2].0, points[2].1
                );
            }
        }
    }
    svg.push_str("</g></svg>");
    svg
}

/// Canvas-pixel bounds covering every shape, padded for strokes and glyph
/// overhang.
fn bounds(shapes: &[OverlayShape]) -> Option<Rect> {
    let mut min = (f64::MAX, f64::MAX);
    let mut max = (f64::MIN, f64::MIN);
    let mut include = |x: f64, y: f64| {
        min = (min.0.min(x), min.1.min(y));
        max = (max.0.max(x), max.1.max(y));
    };
    for shape in shapes {
        match shape {
            OverlayShape::Pill { rect, .. } => {
                include(rect.x, rect.y);
                include(rect.right(), rect.bottom());
            }
            OverlayShape::Text { x, y, size, line_height, text, .. } => {
                include(*x - size * 0.2, *y);
                include(*x + measure(text, *size).width + size * 0.2, *y + line_height);
            }
            OverlayShape::Chevron { points, width, .. } => {
                for (x, y) in points {
                    include(x - width, y - width);
                    include(x + width, y + width);
                }
            }
        }
    }
    (max.0 > min.0 && max.1 > min.1).then(|| Rect {
        x: min.0.floor() - 2.0,
        y: min.1.floor() - 2.0,
        width: (max.0 - min.0).ceil() + 4.0,
        height: (max.1 - min.1).ceil() + 4.0,
    })
}

/// Draw `shapes` over a straight-alpha canvas layer, rasterizing only the
/// region they cover.
pub fn paint_over(layer: &mut RgbaImage, shapes: &[OverlayShape]) {
    let Some(area) = bounds(shapes) else {
        return;
    };
    let left = area.x.max(0.0) as u32;
    let top = area.y.max(0.0) as u32;
    let right = (area.right().max(0.0) as u32).min(layer.width());
    let bottom = (area.bottom().max(0.0) as u32).min(layer.height());
    if right <= left || bottom <= top {
        return;
    }
    let (width, height) = (right - left, bottom - top);
    let svg = svg_document(shapes, (left as f64, top as f64), width, height);
    let Ok(patch) = super::scene::render_svg_layer(&svg, width, height) else {
        return;
    };
    for y in 0..height {
        for x in 0..width {
            let over = *patch.get_pixel(x, y);
            if over[3] == 0 {
                continue;
            }
            let under = layer.get_pixel_mut(left + x, top + y);
            *under = image::Rgba(blend_straight(under.0, over.0));
        }
    }
}

/// Straight-alpha source-over.
fn blend_straight(under: [u8; 4], over: [u8; 4]) -> [u8; 4] {
    let sa = over[3] as f32 / 255.0;
    let da = under[3] as f32 / 255.0;
    let out = sa + da * (1.0 - sa);
    if out <= 0.0 {
        return [0, 0, 0, 0];
    }
    let mut result = [0u8; 4];
    for channel in 0..3 {
        let value = (over[channel] as f32 * sa + under[channel] as f32 * da * (1.0 - sa)) / out;
        result[channel] = value.round().clamp(0.0, 255.0) as u8;
    }
    result[3] = (out * 255.0).round() as u8;
    result
}

/// Per-frame export layer: the canvas annotations (if any) with the studio
/// overlays drawn on top at the export canvas resolution. Unchanged frames
/// reuse the previous layer.
pub struct OverlayLayerCache {
    overlays: StudioOverlays,
    canvas: (u32, u32),
    card: Rect,
    last: Option<(Option<usize>, Vec<OverlayShape>, Arc<RgbaImage>)>,
    resized: Option<(usize, Arc<RgbaImage>)>,
}

impl OverlayLayerCache {
    pub fn new(overlays: StudioOverlays, canvas: (u32, u32), card: Rect) -> Self {
        Self {
            overlays,
            canvas,
            card,
            last: None,
            resized: None,
        }
    }

    pub fn layer(&mut self, time: f64, base: Option<Arc<RgbaImage>>) -> Option<Arc<RgbaImage>> {
        let shapes = self.overlays.shapes_at(
            time,
            (self.canvas.0 as f64, self.canvas.1 as f64),
            self.card,
        );
        if shapes.is_empty() {
            return base;
        }
        let key = base.as_ref().map(|image| Arc::as_ptr(image) as usize);
        if let Some((cached_key, cached_shapes, layer)) = &self.last {
            if *cached_key == key && *cached_shapes == shapes {
                return Some(layer.clone());
            }
        }
        let mut layer = match base {
            Some(base) if base.dimensions() == self.canvas => (*base).clone(),
            Some(base) => {
                let pointer = Arc::as_ptr(&base) as usize;
                if !self.resized.as_ref().is_some_and(|(cached, _)| *cached == pointer) {
                    let resized = image::imageops::resize(
                        base.as_ref(),
                        self.canvas.0,
                        self.canvas.1,
                        image::imageops::FilterType::Lanczos3,
                    );
                    self.resized = Some((pointer, Arc::new(resized)));
                }
                (*self.resized.as_ref().unwrap().1).clone()
            }
            None => RgbaImage::new(self.canvas.0, self.canvas.1),
        };
        paint_over(&mut layer, &shapes);
        let layer = Arc::new(layer);
        self.last = Some((key, shapes, layer.clone()));
        Some(layer)
    }
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::clips::RecordingClipSegment;

    fn card() -> Rect {
        Rect { x: 100.0, y: 60.0, width: 1720.0, height: 960.0 }
    }

    #[test]
    fn speed_badge_merges_adjacent_spans_and_fades() {
        let mut fast = RecordingClipSegment::new(0.0, 4.0);
        fast.speed = 2.0;
        let mut faster = RecordingClipSegment::new(4.0, 8.0);
        faster.speed = 2.0;
        let timeline = SpeedBadgeTimeline::new(&RecordingClipTimeline::new(vec![
            fast,
            faster,
            RecordingClipSegment::new(8.0, 10.0),
        ]));
        assert_eq!(timeline.spans, vec![(0.0, 4.0, 2.0)]);
        assert_eq!(timeline.frame_at(0.0).unwrap().presence, 0.0);
        assert_eq!(timeline.frame_at(2.0).unwrap().presence, 1.0);
        assert!(timeline.frame_at(4.5).is_none());
        assert_eq!(speed_title(1.5), "1.5×");
        assert_eq!(speed_title(2.0), "2×");
    }

    #[test]
    fn keystrokes_follow_edits_and_pin_to_the_card() {
        let clips = RecordingClipTimeline::new(vec![RecordingClipSegment::new(2.0, 10.0)]);
        let events = [
            KeystrokeEvent { time: 1.0, modifiers: vec![], key: "A".into() },
            KeystrokeEvent { time: 3.0, modifiers: vec!["Ctrl".into()], key: "S".into() },
        ];
        let overlays = StudioOverlays::new(
            InputOverlayStyle::default(),
            SubtitleStyle::default(),
            &events,
            None,
            &clips,
        );
        // The cut-away key never shows; the kept one moves to editor time 1.
        assert!(overlays.shapes_at(0.5, (1920.0, 1080.0), card()).is_empty());
        let shapes = overlays.shapes_at(1.5, (1920.0, 1080.0), card());
        let OverlayShape::Pill { rect, .. } = &shapes[0] else { panic!("pill first") };
        assert!((rect.x + rect.width * 0.5 - 960.0).abs() < 0.5);
        assert!(rect.bottom() <= card().bottom() - card().height * 0.045 + 0.5);
        let texts: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match shape {
                OverlayShape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Ctrl ", "S"]);
    }

    #[test]
    fn painted_layer_is_resolution_independent() {
        let clips = RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.0, 4.0)]);
        let events = [KeystrokeEvent { time: 0.0, modifiers: vec![], key: "Enter".into() }];
        let overlays = StudioOverlays::new(
            InputOverlayStyle::default(),
            SubtitleStyle::default(),
            &events,
            None,
            &clips,
        );
        let coverage = |scale: f64| {
            let canvas = ((1280.0 * scale) as u32, (720.0 * scale) as u32);
            let card = Rect { x: 0.0, y: 0.0, width: canvas.0 as f64, height: canvas.1 as f64 };
            let mut cache = OverlayLayerCache::new(overlays.clone(), canvas, card);
            let layer = cache.layer(0.5, None).unwrap();
            layer.pixels().filter(|pixel| pixel[3] > 0).count() as f64 / (scale * scale)
        };
        let small = coverage(1.0);
        let large = coverage(2.0);
        assert!(small > 0.0);
        assert!((small - large).abs() / small < 0.08, "{small} vs {large}");
    }
}
