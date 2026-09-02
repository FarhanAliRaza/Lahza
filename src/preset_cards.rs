//! Animated preview cards for motion presets: a stylised app window run
//! through the preset's real viewport timeline, looping like the exported GIF.

use std::sync::OnceLock;
use std::time::Instant;

use gpui::{
    canvas, point, prelude::*, px, AnyElement, Bounds, ContentMask, Hsla, PathBuilder, Pixels,
    Window,
};

use crate::recording::viewport::{visible_rect, MotionPreset, ViewportFrame, ViewportTimeline};

/// Length of the looping preview; a short hold at the end reads as the loop.
const PREVIEW_DURATION: f64 = 3.0;
const PREVIEW_HOLD: f64 = 0.5;

/// Normalized rectangles making up the fake window, with their colours.
/// (x, y, w, h, hue, saturation, lightness)
const WINDOW_PARTS: [(f64, f64, f64, f64, f32, f32, f32); 9] = [
    (0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.98),      // window body
    (0.0, 0.0, 1.0, 0.14, 0.0, 0.0, 0.90),     // title bar
    (0.04, 0.045, 0.045, 0.05, 0.0, 0.0, 0.6), // window dots
    (0.10, 0.045, 0.045, 0.05, 0.0, 0.0, 0.6),
    (0.16, 0.045, 0.045, 0.05, 0.0, 0.0, 0.6),
    (0.0, 0.14, 0.26, 0.86, 0.0, 0.0, 0.93),      // sidebar
    (0.34, 0.26, 0.58, 0.34, 0.0667, 0.95, 0.55), // hero card (orange)
    (0.34, 0.68, 0.50, 0.07, 0.0, 0.0, 0.78),     // text lines
    (0.34, 0.80, 0.36, 0.07, 0.0, 0.0, 0.85),
];

fn timelines() -> &'static [ViewportTimeline] {
    static TIMELINES: OnceLock<Vec<ViewportTimeline>> = OnceLock::new();
    TIMELINES.get_or_init(|| {
        MotionPreset::ALL
            .iter()
            .map(|preset| {
                ViewportTimeline::build_static(&preset.cues(PREVIEW_DURATION), PREVIEW_DURATION)
            })
            .collect()
    })
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Camera frame for `preset` at the current wall-clock loop position.
fn frame_now(preset: MotionPreset) -> ViewportFrame {
    let index = MotionPreset::ALL
        .iter()
        .position(|candidate| *candidate == preset)
        .unwrap_or(0);
    let elapsed = epoch().elapsed().as_secs_f64();
    let time = (elapsed % (PREVIEW_DURATION + PREVIEW_HOLD)).min(PREVIEW_DURATION);
    timelines()[index].frame_at(time)
}

/// Projects a normalized window point into card pixels through the frame's
/// crop and tilt, mirroring the scene projection's rotation order.
fn project(u: f64, v: f64, frame: ViewportFrame, bounds: Bounds<Pixels>) -> gpui::Point<Pixels> {
    let width = f64::from(f32::from(bounds.size.width));
    let height = f64::from(f32::from(bounds.size.height));
    let (left, top, visible) = visible_rect(frame);
    let x = (u - left - visible * 0.5) / visible * width;
    let y = (v - top - visible * 0.5) / visible * height;
    let camera = (width * width + height * height).sqrt() * 2.0;
    let (sx, cx) = frame.tilt.x.to_radians().sin_cos();
    let (sy, cy) = frame.tilt.y.to_radians().sin_cos();
    let (sz, cz) = frame.tilt.z.to_radians().sin_cos();
    let (x, y) = (x * cz - y * sz, x * sz + y * cz);
    let (y, z) = (y * cx, y * sx);
    let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
    let factor = camera / (camera - z).max(camera * 0.05);
    point(
        bounds.origin.x + px((width * 0.5 + x * factor) as f32),
        bounds.origin.y + px((height * 0.5 + y * factor) as f32),
    )
}

fn paint_preview(preset: MotionPreset, bounds: Bounds<Pixels>, window: &mut Window) {
    let frame = frame_now(preset);
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (x, y, w, h, hue, saturation, lightness) in WINDOW_PARTS {
            let mut builder = PathBuilder::fill();
            builder.move_to(project(x, y, frame, bounds));
            builder.line_to(project(x + w, y, frame, bounds));
            builder.line_to(project(x + w, y + h, frame, bounds));
            builder.line_to(project(x, y + h, frame, bounds));
            builder.close();
            if let Ok(path) = builder.build() {
                window.paint_path(
                    path,
                    Hsla {
                        h: hue,
                        s: saturation,
                        l: lightness,
                        a: 1.0,
                    },
                );
            }
        }
    });
    // Keep looping while the cards are on screen.
    window.request_animation_frame();
}

/// The animated preview area of a preset card; size it from the caller.
pub(crate) fn preset_preview(preset: MotionPreset) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| paint_preview(preset, bounds, window),
    )
    .size_full()
    .into_any_element()
}
