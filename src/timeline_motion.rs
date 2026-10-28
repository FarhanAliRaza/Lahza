//! Motion ranges are drafted in editor time and committed once on release.
//! Source slices keep a drawn range correct across cuts, gaps and speed changes.

use crate::{
    motion_ui::MotionPick,
    recording::{
        clips::RecordingClipTimeline,
        model::NormalizedPoint,
        viewport::{ZoomAnchorMode, ZoomCue},
    },
    timeline_edit::snap_timeline_edges,
    Studio, VideoEditSnapshot,
};
use gpui::{div, hsla, prelude::*, px, AnyElement, Context, Pixels};
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct TimelineMotionState {
    pub(crate) creation: Option<MotionCreationDrag>,
}

#[derive(Clone, Copy)]
pub(crate) struct MotionCreationDrag {
    start_x: Pixels,
    start_scroll: f64,
    seconds_per_pixel: f64,
    start: f64,
    current: f64,
    active: bool,
    original_clip: Option<Uuid>,
    original_cue: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MotionCreationSlice {
    editor_start: f64,
    editor_end: f64,
    source_start: f64,
    source_end: f64,
}

/// A cue uses source time, so an editor range crossing disjoint/reordered
/// clips becomes separate cues. Empty workspace and clip gaps create none.
fn creation_slices(clips: &RecordingClipTimeline, from: f64, to: f64) -> Vec<MotionCreationSlice> {
    if !from.is_finite() || !to.is_finite() {
        return Vec::new();
    }
    let low = from.min(to).max(0.0);
    let high = from.max(to).min(clips.duration());
    let mut slot = 0.0;
    let mut slices: Vec<MotionCreationSlice> = Vec::new();
    for clip in &clips.segments {
        let clip_start = slot + clip.gap_before;
        let start = low.max(clip_start);
        let end = high.min(clip_start + clip.editor_duration());
        let source_start = clip.source_start + (start - clip_start) * clip.speed;
        let source_end = clip.source_start + (end - clip_start) * clip.speed;
        if end > start {
            let next = MotionCreationSlice {
                editor_start: start,
                editor_end: end,
                source_start,
                source_end,
            };
            // A plain split must not introduce a second easing envelope.
            if let Some(previous) = slices.last_mut().filter(|previous| {
                (previous.editor_end - start).abs() < 1e-9
                    && (previous.source_end - source_start).abs() < 1e-9
            }) {
                previous.editor_end = end;
                previous.source_end = source_end;
            } else {
                slices.push(next);
            }
        }
        slot += clip.slot_duration();
    }
    slices
        .retain(|slice| slice.source_end - slice.source_start >= ZoomCue::MINIMUM_DURATION - 1e-9);
    slices
}

impl Studio {
    pub(crate) fn begin_motion_creation(&mut self, pointer_x: Pixels, _cx: &mut Context<Self>) {
        if self.video_edit_busy || self.video_duration <= 0.0 {
            return;
        }
        let Some(time) = self.motion_timeline_time_at(pointer_x) else {
            return;
        };
        self.finish_annotation_interaction();
        self.pause_video_playback();
        self.video_seek_drag = None;
        self.video_trim_drag = None;
        self.video_move_drag = None;
        self.video_zoom_drag = None;
        self.timeline_ui.snap_guide = None;
        self.timeline_motion.creation = Some(MotionCreationDrag {
            start_x: pointer_x,
            start_scroll: self.video_timeline_scroll,
            seconds_per_pixel: self.video_timeline_seconds_per_pixel(),
            start: time,
            current: time,
            active: false,
            original_clip: self.video_selected_clip,
            original_cue: self.video_selected_zoom_cue,
        });
        self.video_selected_clip = None;
        self.video_selected_zoom_cue = None;
    }

    pub(crate) fn update_motion_creation(&mut self, pointer_x: Pixels) -> bool {
        let Some(mut drag) = self.timeline_motion.creation else {
            return false;
        };
        let pixels = f64::from((pointer_x - drag.start_x) / px(1.0)) + self.video_timeline_scroll
            - drag.start_scroll;
        drag.active |= pixels.abs() >= 4.0;
        let raw = (drag.start + pixels * drag.seconds_per_pixel)
            .clamp(0.0, self.video_timeline_duration());
        let (time, guide) = snap_timeline_edges(
            raw,
            0.0,
            &self.timeline_snap_targets(None, None, None),
            drag.seconds_per_pixel,
            self.timeline_ui.snap_enabled && drag.active,
        );
        drag.current = time.clamp(0.0, self.video_timeline_duration());
        self.timeline_ui.snap_guide = guide;
        self.timeline_motion.creation = Some(drag);
        true
    }

    pub(crate) fn finish_motion_creation(
        &mut self,
        pointer_x: Pixels,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.timeline_motion.creation.is_none() {
            return false;
        }
        self.update_motion_creation(pointer_x);
        let drag = self.timeline_motion.creation.take().unwrap();
        self.timeline_ui.snap_guide = None;
        if !drag.active {
            self.seek_video(drag.current, cx);
            return true;
        }
        let slices = creation_slices(&self.video_clip_timeline, drag.start, drag.current);
        if slices.is_empty() {
            self.toast = Some("Drag a longer range over a clip to add a zoom".into());
            cx.notify();
            return true;
        }
        let original = self.video_zoom_cues.clone();
        let follow_pointer = self.scene_has_pointer();
        let mut selected = None;
        for slice in slices {
            let point = self
                .video_pointer_timeline
                .location_at(slice.editor_start)
                .unwrap_or(NormalizedPoint { x: 0.5, y: 0.5 });
            let mut cue = ZoomCue::pinned(
                slice.source_start,
                slice.source_end,
                self.default_motion_zoom,
                point,
            );
            if follow_pointer {
                cue.anchor_mode = ZoomAnchorMode::PointerAnchor;
                cue.bounds_bias = 0.25;
            }
            selected.get_or_insert(cue.id);
            self.video_zoom_cues.push(cue);
        }
        self.video_zoom_cues
            .sort_by(|a, b| a.start.total_cmp(&b.start));
        self.video_selected_zoom_cue = selected;
        self.motion_pick = MotionPick::Focus;
        self.video_undo_stack
            .push(VideoEditSnapshot::Zoom(original));
        self.video_redo_stack.clear();
        self.rebuild_video_motion_timelines();
        self.persist_video_zoom_cues(cx);
        true
    }

    pub(crate) fn cancel_motion_creation(&mut self) -> bool {
        let Some(drag) = self.timeline_motion.creation.take() else {
            return false;
        };
        self.video_selected_clip = drag.original_clip;
        self.video_selected_zoom_cue = drag.original_cue;
        self.timeline_ui.snap_guide = None;
        true
    }

    pub(crate) fn motion_creation_times(&self) -> Option<(f64, f64)> {
        let drag = self.timeline_motion.creation.filter(|drag| drag.active)?;
        Some((drag.start.min(drag.current), drag.start.max(drag.current)))
    }

    pub(crate) fn motion_creation_is_active(&self) -> bool {
        self.timeline_motion
            .creation
            .is_some_and(|drag| drag.active)
    }

    /// The ghost shows only intervals that can actually become motion cues.
    pub(crate) fn motion_creation_preview(&self) -> Option<AnyElement> {
        let drag = self.timeline_motion.creation.filter(|drag| drag.active)?;
        let scale = self.video_timeline_seconds_per_pixel();
        let slices = creation_slices(&self.video_clip_timeline, drag.start, drag.current);
        let viewport = self.video_timeline_viewport_width();
        if slices.is_empty() {
            let left = (drag.start.min(drag.current) / scale - self.video_timeline_scroll).max(0.0);
            let right =
                (drag.start.max(drag.current) / scale - self.video_timeline_scroll).min(viewport);
            if right <= left {
                return None;
            }
            return Some(
                div()
                    .absolute()
                    .left(px(left as f32))
                    .top(px(3.0))
                    .w(px((right - left) as f32))
                    .h(px(24.0))
                    .rounded_md()
                    .border_2()
                    .border_dashed()
                    .border_color(crate::muted())
                    .bg(hsla(0.0, 0.0, 0.5, 0.1))
                    .overflow_hidden()
                    .when(right - left >= 120.0, |this| {
                        this.text_xs()
                            .text_color(crate::muted())
                            .child("Drag a range over a clip")
                    })
                    .into_any_element(),
            );
        }
        Some(
            div()
                .absolute()
                .size_full()
                .children(slices.into_iter().filter_map(|slice| {
                    let left = slice.editor_start / scale - self.video_timeline_scroll;
                    let right = slice.editor_end / scale - self.video_timeline_scroll;
                    if right < 0.0 || left > viewport {
                        return None;
                    }
                    Some(
                        div()
                            .absolute()
                            .left(px(left.max(0.0) as f32))
                            .top(px(3.0))
                            .w(px((right.min(viewport) - left.max(0.0)).max(1.0) as f32))
                            .h(px(24.0))
                            .rounded_md()
                            .border_2()
                            .border_dashed()
                            .border_color(crate::lane_zoom())
                            .bg(crate::lane_zoom().opacity(0.22))
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .text_xs()
                            .text_color(crate::ink())
                            .when(right - left >= 60.0, |this| {
                                this.child(crate::units::fmt_seconds(slice.editor_end - slice.editor_start))
                            }),
                    )
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::clips::RecordingClipSegment;

    #[test]
    fn reverse_creation_has_the_same_range_and_never_includes_gaps() {
        let mut second = RecordingClipSegment::new(12.0, 15.0);
        second.gap_before = 2.0;
        let clips = RecordingClipTimeline::new(vec![RecordingClipSegment::new(1.0, 4.0), second]);
        let expected = vec![
            MotionCreationSlice {
                editor_start: 2.0,
                editor_end: 3.0,
                source_start: 3.0,
                source_end: 4.0,
            },
            MotionCreationSlice {
                editor_start: 5.0,
                editor_end: 7.0,
                source_start: 12.0,
                source_end: 14.0,
            },
        ];
        assert_eq!(creation_slices(&clips, 2.0, 7.0), expected);
        assert_eq!(creation_slices(&clips, 7.0, 2.0), expected);
        assert!(creation_slices(&clips, 3.2, 4.8).is_empty());
    }

    #[test]
    fn creation_maps_reordered_and_sped_up_clips_without_covering_removed_source() {
        let mut first = RecordingClipSegment::new(20.0, 24.0);
        first.speed = 2.0;
        let clips = RecordingClipTimeline::new(vec![first, RecordingClipSegment::new(1.0, 3.0)]);
        assert_eq!(
            creation_slices(&clips, 1.0, 3.0),
            vec![
                MotionCreationSlice {
                    editor_start: 1.0,
                    editor_end: 2.0,
                    source_start: 22.0,
                    source_end: 24.0
                },
                MotionCreationSlice {
                    editor_start: 2.0,
                    editor_end: 3.0,
                    source_start: 1.0,
                    source_end: 2.0
                },
            ]
        );
    }

    #[test]
    fn creation_rejects_too_short_or_outside_ranges() {
        let clips = RecordingClipTimeline::full(5.0);
        assert!(creation_slices(&clips, 2.0, 2.1).is_empty());
        assert!(creation_slices(&clips, 6.0, 8.0).is_empty());
        assert!(creation_slices(&clips, f64::NAN, 3.0).is_empty());
        assert_eq!(creation_slices(&clips, 4.5, 9.0).len(), 1);
    }

    #[test]
    fn plain_splits_do_not_restart_motion_or_discard_short_fragments() {
        let clips = RecordingClipTimeline::new(vec![
            RecordingClipSegment::new(0.0, 1.0),
            RecordingClipSegment::new(1.0, 2.0),
        ]);
        assert_eq!(
            creation_slices(&clips, 0.7, 1.3),
            vec![MotionCreationSlice {
                editor_start: 0.7,
                editor_end: 1.3,
                source_start: 0.7,
                source_end: 1.3
            },]
        );
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::recording::model::RecordingSession;
    use crate::timed::AnnotationTiming;
    use crate::{asset_directory, open_studio_window, AnnotationMark, Assets};
    use gpui::{point, Application, Timer};
    use std::time::Duration;

    /// Native events exercise propagation from the lane through the editor,
    /// including Escape while a pointer button is still down.
    fn input(x: i32, y: i32, action: &str) {
        let status = std::process::Command::new("python3")
            .arg("-c")
            .arg(
                r#"
import ctypes, sys
x11 = ctypes.CDLL('libX11.so.6')
xtst = ctypes.CDLL('libXtst.so.6')
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XStringToKeysym.restype = ctypes.c_ulong
display = ctypes.c_void_p(x11.XOpenDisplay(None))
assert display.value
xtst.XTestFakeMotionEvent(display, -1, int(sys.argv[1]), int(sys.argv[2]), 0)
action = sys.argv[3]
if action == 'escape':
    code = x11.XKeysymToKeycode(display, x11.XStringToKeysym(b'Escape'))
    xtst.XTestFakeKeyEvent(display, code, True, 0)
    xtst.XTestFakeKeyEvent(display, code, False, 0)
elif action == 'scroll':
    xtst.XTestFakeButtonEvent(display, 5, True, 0)
    xtst.XTestFakeButtonEvent(display, 5, False, 0)
elif action != 'move':
    button = 3 if action.startswith('right-') else 1
    xtst.XTestFakeButtonEvent(display, button, action.endswith('down'), 0)
x11.XSync(display, False)
x11.XCloseDisplay(display)
"#,
            )
            .arg(x.to_string())
            .arg(y.to_string())
            .arg(action)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    #[ignore = "requires X11, python3 and libXtst (run under Xvfb)"]
    fn native_motion_creation_context_and_escape_restore_transactions() {
        let root = std::env::temp_dir().join(format!("lahza-motion-input-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        Application::new()
            .with_assets(Assets {
                base: asset_directory(),
            })
            .run(move |cx| {
                let handle = open_studio_window(cx, true, |handle, cx| {
                    cx.new(|cx| Studio::new(handle, None, None, cx))
                })
                .unwrap();
                handle
                    .update(cx, |studio, _, cx| {
                        studio.load_generation = studio.load_generation.wrapping_add(1);
                        studio.loading = None;
                        studio.launcher_active = false;
                        studio.launcher_window = false;
                        studio.video_project = Some(session);
                        studio.video_clip_timeline = RecordingClipTimeline::full(10.0);
                        studio.video_duration = 10.0;
                        studio.video_source_duration = 10.0;
                        studio.video_timeline_span = 10.0;
                        studio.video_timeline_extent = 10.0;
                        studio.video_position = 7.0;
                        studio.timeline_ui.snap_enabled = false;
                        studio.annotations = vec![AnnotationMark {
                            timing: Some(AnnotationTiming {
                                start: 1.0,
                                end: 3.0,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }];
                        studio.set_video_frame(image::RgbaImage::from_pixel(
                            160,
                            90,
                            image::Rgba([50, 70, 100, 255]),
                        ));
                        cx.notify();
                    })
                    .unwrap();
                cx.spawn(async move |cx| {
                    Timer::after(Duration::from_millis(250)).await;
                    let window_handle: gpui::AnyWindowHandle = handle.into();
                    window_handle
                        .update(cx, |_, window, cx| {
                            window.draw(cx).clear();
                            window.draw(cx).clear();
                        })
                        .unwrap();
                    let (origin, width, motion_y, annotation_y) = handle
                        .update(cx, |studio, window, _| {
                            let bounds = studio.video_timeline_bounds.lock().unwrap().unwrap();
                            let origin = window.bounds().origin;
                            (
                                point(origin.x + bounds.origin.x, origin.y + bounds.origin.y),
                                studio.video_timeline_viewport_width(),
                                f32::from(origin.y + bounds.bottom()) + 15.0,
                                f32::from(origin.y + bounds.bottom())
                                    + studio.motion_lane_height()
                                    + 19.0,
                            )
                        })
                        .unwrap();
                    let at = |fraction: f64| {
                        f32::from(origin.x) as i32 + (width * fraction).round() as i32
                    };
                    let y = motion_y.round() as i32;
                    input(at(0.2), y, "down");
                    Timer::after(Duration::from_millis(100)).await;
                    handle
                        .update(cx, |studio, _, _| {
                            assert!(
                                studio.timeline_motion.creation.is_some(),
                                "empty motion lane arms range creation"
                            );
                            assert!(studio.video_seek_drag.is_none());
                            assert_eq!(studio.video_position, 7.0);
                        })
                        .unwrap();
                    input(at(0.2), y, "scroll");
                    Timer::after(Duration::from_millis(100)).await;
                    input(at(0.5), y, "move");
                    Timer::after(Duration::from_millis(100)).await;
                    let draft_range = handle
                        .update(cx, |studio, _, _| {
                            assert!(
                                studio.video_timeline_scroll > 0.0,
                                "wheel pans the timeline during creation"
                            );
                            assert!(
                                studio.motion_creation_preview().is_some(),
                                "draft paints its ghost after panning"
                            );
                            let draft = studio.motion_creation_times().unwrap();
                            assert!(draft.1 > 5.0, "the dragged edge follows scrolled content");
                            assert!(
                                studio.video_zoom_cues.is_empty(),
                                "range remains a draft until release"
                            );
                            draft
                        })
                        .unwrap();
                    input(at(0.5), y, "up");
                    Timer::after(Duration::from_millis(100)).await;
                    let original = handle
                        .update(cx, |studio, _, cx| {
                            assert!(studio.timeline_motion.creation.is_none());
                            assert_eq!(studio.video_zoom_cues.len(), 1);
                            let cue = &studio.video_zoom_cues[0];
                            assert!(
                                (cue.start - draft_range.0).abs()
                                    < 2.0 * studio.video_timeline_seconds_per_pixel()
                            );
                            assert!(
                                (cue.end - draft_range.1).abs()
                                    < 2.0 * studio.video_timeline_seconds_per_pixel()
                            );
                            assert_eq!(
                                studio.video_position, 7.0,
                                "creating motion keeps the parked playhead"
                            );
                            assert_eq!(studio.video_undo_stack.len(), 1);
                            let cues = studio.video_zoom_cues.clone();
                            studio.undo_video_edit(cx);
                            assert!(studio.video_zoom_cues.is_empty());
                            studio.redo_video_edit(cx);
                            assert_eq!(studio.video_zoom_cues, cues);
                            studio.video_timeline_scroll = 0.0;
                            cx.notify();
                            cues
                        })
                        .unwrap();
                    input(at(0.6), y, "down");
                    Timer::after(Duration::from_millis(100)).await;
                    input(at(0.9), y, "move");
                    Timer::after(Duration::from_millis(100)).await;
                    input(at(0.9), y, "escape");
                    Timer::after(Duration::from_millis(100)).await;
                    input(at(0.9), y, "up");
                    Timer::after(Duration::from_millis(100)).await;
                    handle
                        .update(cx, |studio, _, _| {
                            assert!(studio.timeline_motion.creation.is_none());
                            assert_eq!(studio.video_zoom_cues, original);
                            assert_eq!(
                                studio.video_undo_stack.len(),
                                1,
                                "cancel adds no undo entry"
                            );
                        })
                        .unwrap();
                    input(at(0.35), y, "right-down");
                    input(at(0.35), y, "right-up");
                    Timer::after(Duration::from_millis(100)).await;
                    handle
                        .update(cx, |studio, _, cx| {
                            assert_eq!(studio.video_selected_zoom_cue, Some(original[0].id));
                            assert!(
                                studio.timeline_context_menu(cx).is_some(),
                                "right click opens motion options"
                            );
                        })
                        .unwrap();
                    input(at(0.35), y, "escape");
                    Timer::after(Duration::from_millis(100)).await;
                    let before = handle
                        .update(cx, |studio, _, cx| {
                            assert!(studio.timeline_context_menu(cx).is_none());
                            studio.annotations[0].timing
                        })
                        .unwrap();
                    let y = annotation_y.round() as i32;
                    input(at(0.15), y, "down");
                    Timer::after(Duration::from_millis(100)).await;
                    input(at(0.25), y, "move");
                    Timer::after(Duration::from_millis(100)).await;
                    handle
                        .update(cx, |studio, _, _| {
                            assert!(
                                studio.annotation_drag.is_some(),
                                "annotation block starts timing drag"
                            );
                            assert_ne!(studio.annotations[0].timing, before);
                            assert_eq!(
                                studio.video_undo_stack.len(),
                                1,
                                "annotation timing remains uncommitted during drag"
                            );
                        })
                        .unwrap();
                    input(at(0.25), y, "escape");
                    Timer::after(Duration::from_millis(100)).await;
                    input(at(0.25), y, "up");
                    Timer::after(Duration::from_millis(100)).await;
                    handle
                        .update(cx, |studio, _, _| {
                            assert!(studio.annotation_drag.is_none());
                            assert_eq!(studio.annotations[0].timing, before);
                            assert_eq!(studio.video_undo_stack.len(), 1);
                            assert!(studio.timeline_ui.snap_guide.is_none());
                        })
                        .unwrap();

                    // Releasing outside the editor takes the mouse_up_out
                    // route. It must commit the pending timing once and stop
                    // reacting to subsequent pointer movement.
                    input(at(0.15), y, "down");
                    Timer::after(Duration::from_millis(100)).await;
                    input(at(0.25), y, "move");
                    Timer::after(Duration::from_millis(100)).await;
                    input(0, 0, "move");
                    Timer::after(Duration::from_millis(100)).await;
                    let release_timing = handle
                        .update(cx, |studio, window, _| {
                            assert!(
                                window.bounds().origin.x > px(0.0)
                                    || window.bounds().origin.y > px(0.0),
                                "native release-out test needs a screen larger than the editor"
                            );
                            assert!(studio.annotation_drag.is_some());
                            assert_eq!(studio.video_undo_stack.len(), 1);
                            assert_ne!(studio.annotations[0].timing, before);
                            studio.annotations[0].timing
                        })
                        .unwrap();
                    input(0, 0, "up");
                    Timer::after(Duration::from_millis(100)).await;
                    handle
                        .update(cx, |studio, _, _| {
                            assert!(
                                studio.annotation_drag.is_none(),
                                "release outside the editor ends the annotation gesture"
                            );
                            assert_eq!(studio.annotations[0].timing, release_timing);
                            assert_eq!(
                                studio.video_undo_stack.len(),
                                2,
                                "release outside commits exactly one annotation undo entry"
                            );
                            assert!(studio.timeline_ui.snap_guide.is_none());
                        })
                        .unwrap();
                    input(at(0.4), y, "move");
                    Timer::after(Duration::from_millis(100)).await;
                    handle
                        .update(cx, |studio, _, cx| {
                            assert_eq!(
                                studio.annotations[0].timing, release_timing,
                                "released gesture no longer follows the pointer"
                            );
                            assert_eq!(studio.video_undo_stack.len(), 2);
                            studio.undo_video_edit(cx);
                            assert_eq!(studio.annotations[0].timing, before);
                            assert_eq!(studio.video_undo_stack.len(), 1);
                        })
                        .unwrap();
                    cx.update(|cx| cx.quit()).unwrap();
                    std::fs::remove_dir_all(&root).unwrap();
                })
                .detach();
            });
    }
}
