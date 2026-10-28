//! Keystroke captions, the speed badge and subtitles in the recording
//! editor: the cached overlay model, its preview painting, and the
//! inspector controls. Layout comes from `recording::studio_overlays`, which
//! the exporter rasterizes, so the preview and the file agree.
use crate::recording::{
    clips::RecordingClipTimeline,
    scene::Rect,
    studio_overlays::{InputOverlayStyle, OverlayPlacement, OverlayShape, StudioOverlays, SubtitleStyle},
};
use crate::Studio;
use gpui::{
    div, point, prelude::*, px, quad, size, AnyElement, App, Bounds, Context, Hsla, PathBuilder,
    Pixels, Point, Rgba, TextRun, Window,
};
use std::sync::Arc;

/// Everything the derived overlays depend on; a change rebuilds them.
#[derive(Clone, PartialEq)]
pub(crate) struct OverlayKey {
    clips: RecordingClipTimeline,
    input: InputOverlayStyle,
    subtitles: SubtitleStyle,
    transcript: u64,
    keystrokes: usize,
}

impl Studio {
    /// The overlay model for the current edit, rebuilt only when its inputs
    /// change (clips, settings, transcript edits).
    pub(crate) fn studio_overlays(&mut self) -> Arc<StudioOverlays> {
        let key = OverlayKey {
            clips: self.video_clip_timeline.clone(),
            input: self.input_overlays,
            subtitles: self.subtitle_style,
            transcript: self.video_transcript_revision,
            keystrokes: self.video_keystrokes.len(),
        };
        if let Some((cached, overlays)) = &self.video_overlay_cache {
            if *cached == key {
                return overlays.clone();
            }
        }
        let overlays = Arc::new(StudioOverlays::new(
            self.input_overlays,
            self.subtitle_style,
            &self.video_keystrokes,
            self.video_transcript.as_ref(),
            &self.video_clip_timeline,
        ));
        self.video_overlay_cache = Some((key, overlays.clone()));
        overlays
    }

    /// Overlay shapes for the preview canvas at the playhead, in canvas
    /// pixels relative to the canvas origin.
    pub(crate) fn preview_overlay_shapes(&mut self, canvas: (f64, f64), card: Rect) -> Vec<OverlayShape> {
        if self.video_project.is_none() {
            return Vec::new();
        }
        let overlays = self.studio_overlays();
        if overlays.is_empty() {
            return Vec::new();
        }
        overlays.shapes_at(self.video_position, canvas, card)
    }

    fn has_speed_changes(&self) -> bool {
        self.video_clip_timeline
            .segments
            .iter()
            .any(|clip| clip.speed > 1.001)
    }

    fn placement_rows(
        &self,
        id: &'static str,
        selected: OverlayPlacement,
        cx: &mut Context<Self>,
        set: fn(&mut Studio, OverlayPlacement),
    ) -> AnyElement {
        const PLACEMENTS: [OverlayPlacement; 6] = [
            OverlayPlacement::TopLeft,
            OverlayPlacement::TopCenter,
            OverlayPlacement::TopRight,
            OverlayPlacement::BottomLeft,
            OverlayPlacement::BottomCenter,
            OverlayPlacement::BottomRight,
        ];
        let control_id = match id {
            "keystroke" => "keystroke-position",
            _ => "speed-position",
        };
        crate::controls::field(
            "Position",
            self.position_grid(
                control_id,
                &["Top left", "Top center", "Top right", "Bottom left", "Bottom center", "Bottom right"],
                3,
                PLACEMENTS.iter().position(|item| *item == selected).unwrap_or(usize::MAX),
                move |this, index| set(this, PLACEMENTS[index]),
                cx,
            ),
        )
        .into_any_element()
    }

    /// Record → Keystrokes: typed-key captions from the recording's input log.
    pub(crate) fn keystroke_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.input_overlays;
        self.placement_rows("keystroke", style.keystroke_placement, cx, |this, placement| {
            this.input_overlays.keystroke_placement = placement
        })
    }

    /// Record → Speed badge: a "2× ›››" pill while sped-up clips play.
    pub(crate) fn speed_badge_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.input_overlays;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when(!self.has_speed_changes(), |this| {
                this.child(crate::controls::hint(
                    "Shows while a clip plays faster than 1×. Set a clip's speed from the timeline.",
                ))
            })
            .when(style.speed_badge, |this| {
                this.child(self.placement_rows("speed", style.speed_badge_placement, cx, |this, placement| {
                    this.input_overlays.speed_badge_placement = placement
                }))
            })
            .into_any_element()
    }
}

fn color(rgb: [u8; 3], alpha: f64) -> Hsla {
    Rgba {
        r: rgb[0] as f32 / 255.0,
        g: rgb[1] as f32 / 255.0,
        b: rgb[2] as f32 / 255.0,
        a: alpha.clamp(0.0, 1.0) as f32,
    }
    .into()
}

/// Paint overlay shapes whose coordinates are relative to `origin`.
pub(crate) fn paint_overlay_shapes(
    shapes: &[OverlayShape],
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let at = |x: f64, y: f64| point(origin.x + px(x as f32), origin.y + px(y as f32));
    for shape in shapes {
        match shape {
            OverlayShape::Pill { rect, radius, alpha } => {
                window.paint_quad(quad(
                    Bounds::new(at(rect.x, rect.y), size(px(rect.width as f32), px(rect.height as f32))),
                    px(*radius as f32),
                    color([0, 0, 0], *alpha),
                    px(0.0),
                    gpui::transparent_black(),
                    Default::default(),
                ));
            }
            OverlayShape::Text { x, y, size: font_size, line_height, text, color: rgb, alpha, .. } => {
                let mut font = gpui::font("Noto Sans");
                font.weight = gpui::FontWeight::BOLD;
                let run = TextRun {
                    len: text.len(),
                    font,
                    color: color(*rgb, *alpha),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    text.clone().into(),
                    px(*font_size as f32),
                    &[run],
                    None,
                );
                let _ = shaped.paint(at(*x, *y), px(*line_height as f32), window, cx);
            }
            OverlayShape::Chevron { points, width, alpha } => {
                let mut builder = PathBuilder::stroke(px(*width as f32));
                builder.move_to(at(points[0].0, points[0].1));
                builder.line_to(at(points[1].0, points[1].1));
                builder.line_to(at(points[2].0, points[2].1));
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color([255, 255, 255], *alpha));
                }
            }
        }
    }
}
