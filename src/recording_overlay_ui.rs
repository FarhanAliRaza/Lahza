//! Protected, recording-scoped controls and a webcam monitor. Moving either
//! window only moves the monitor; the saved camera composition stays intact.
use crate::theme::TextRoles;
use crate::recording::camera_preview::{CameraFrame, CameraFrames};
use crate::recording::capture_exclusion::CaptureExclusion;
use crate::recording::native::CaptureSourceGeometry;
use crate::recording::scene::{CameraOverlay, Rect};
use crate::recording_overlay_drag::OverlayDrag;
use crate::{cached_render_image, RecordingAction, RecordingState, Studio};
use gpui::{
    div, point, prelude::*, px, rgb, size, svg, AnyElement, Bounds, Context, FontWeight, MouseButton,
    Pixels, RenderImage, Timer, WeakEntity, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowKind, WindowOptions,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PILL_WIDTH: f32 = 292.0;
const PILL_HEIGHT: f32 = 56.0;
const CAMERA_SIZE: f32 = 216.0;
const EDGE_MARGIN: f32 = 32.0;
const STALE_CAMERA: Duration = Duration::from_secs(1);

#[cfg(test)]
#[path = "recording_overlay_drag_tests.rs"]
mod drag_tests;

#[cfg(test)]
#[path = "recording_overlay_probe_tests.rs"]
mod visibility_probe_tests;

fn monitor_pixels(frame: CameraFrame, overlay: CameraOverlay, canvas_aspect: f64) -> Option<image::RgbaImage> {
    let pixels = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)?;
    if frame.width == 0 || frame.height == 0 { return None; }
    let source_aspect = frame.width as f64 / frame.height as f64;
    let composed = overlay.rect_for_frame(canvas_aspect, 1.0, source_aspect, 1.0);
    let crop = crate::recording::camera_layout::source_crop(overlay, source_aspect, composed);
    let width = (crop.width * frame.width as f64).round().max(1.0) as u32;
    let height = (crop.height * frame.height as f64).round().max(1.0) as u32;
    let x = (crop.x * frame.width as f64).round() as u32;
    let y = (crop.y * frame.height as f64).round() as u32;
    let mut pixels = image::imageops::crop_imm(&pixels,
        x.min(frame.width - width), y.min(frame.height - height), width, height).to_image();
    if overlay.mirror { image::imageops::flip_horizontal_in_place(&mut pixels); }
    Some(pixels)
}

#[derive(Clone, Debug, PartialEq)]
struct RecordingSnapshot {
    state: RecordingState,
    busy: bool,
    has_controller: bool,
    timecode: String,
    start_label: String,
}

impl RecordingSnapshot {
    fn from_studio(studio: &Studio) -> Self {
        Self {
            state: studio.recording_state,
            busy: studio.recording_busy,
            has_controller: studio.recording_controller.is_some(),
            timecode: studio.recording_startup.as_ref()
                .and_then(|startup| startup.countdown_remaining())
                .filter(|_| studio.recording_state == RecordingState::Starting)
                .map(|remaining| remaining.to_string())
                .unwrap_or_else(|| studio.recording_timecode()),
            start_label: studio.recording_start_label(),
        }
    }

    fn can_control(&self) -> bool {
        self.has_controller
            && !self.busy
            && matches!(
                self.state,
                RecordingState::Recording | RecordingState::Paused
            )
    }

    fn label(&self) -> &str {
        match self.state {
            RecordingState::Idle => "Starting…",
            RecordingState::Starting => &self.start_label,
            RecordingState::Finishing => "Saving…",
            RecordingState::Paused if self.busy => "Resuming…",
            RecordingState::Recording if self.busy => "Updating…",
            RecordingState::Paused => "Paused",
            RecordingState::Recording => "Recording",
        }
    }
}

/// The compact, always-available recording controls.
pub(crate) struct RecordingPill {
    studio: WeakEntity<Studio>,
    exclusion: CaptureExclusion,
    snapshot: RecordingSnapshot,
    positioned_source: Option<CaptureSourceGeometry>,
    drag: OverlayDrag,
    _ticker: gpui::Task<()>,
}

fn protection_lost(exclusion: &CaptureExclusion, checked: &mut Instant) -> bool {
    if checked.elapsed() < Duration::from_millis(250) {
        return false;
    }
    *checked = Instant::now();
    let protected = exclusion.is_protected();
    exclusion.has_been_protected() && !protected
}

fn restore_controls(studio: &WeakEntity<Studio>, cx: &mut gpui::AsyncApp) {
    let _ = studio.update(cx, |studio, cx| {
        studio.close_recording_overlays(cx);
        studio.restore_recording_window(cx);
        studio.toast = Some("Recording controls closed so they would not appear in the video".into());
        cx.notify();
    });
}

impl RecordingPill {
    fn new(
        studio: WeakEntity<Studio>,
        snapshot: RecordingSnapshot,
        exclusion: CaptureExclusion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owner = studio.clone();
        let handle = window.window_handle();
        let request = exclusion.clone();
        let mut checked = Instant::now();
        let ticker = cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(100)).await;
            let Ok(snapshot) =
                owner.read_with(cx, |studio, _| RecordingSnapshot::from_studio(studio))
            else {
                let _ = handle.update(cx, |_, window, _| {
                    window.remove_window();
                    request.retire_closed_window();
                });
                break;
            };
            let Ok(lost) = weak.update(cx, |this, cx| {
                if this.snapshot != snapshot {
                    this.snapshot = snapshot;
                    cx.notify();
                }
                protection_lost(&this.exclusion, &mut checked)
            }) else {
                break;
            };
            if lost {
                restore_controls(&owner, cx);
                break;
            }
        });
        // Native requests are kept for the entire take. A window-manager
        // close is the same as Stop, never an unprotected hidden recorder.
        let owner = studio.clone();
        window.on_window_should_close(cx, move |_, cx| {
            let owner = owner.clone();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |studio, cx| {
                    if studio.recording_state == RecordingState::Starting {
                        studio.cancel_recording_start(cx);
                    } else if RecordingSnapshot::from_studio(studio).can_control() {
                        studio.run_recording_action(RecordingAction::Stop, cx);
                        cx.notify();
                    }
                });
            });
            false
        });
        Self {
            studio,
            exclusion,
            snapshot,
            positioned_source: None,
            drag: OverlayDrag::default(),
            _ticker: ticker,
        }
    }

    fn action(&mut self, action: RecordingAction, cx: &mut Context<Self>) {
        // The displayed snapshot can be up to 100 ms old: check the live
        // state again so double clicks cannot race an in-flight operation.
        let _ = self.studio.update(cx, |studio, cx| {
            if matches!(action, RecordingAction::Stop) && studio.recording_state == RecordingState::Starting {
                studio.cancel_recording_start(cx);
            } else if RecordingSnapshot::from_studio(studio).can_control() {
                studio.run_recording_action(action, cx);
                cx.notify();
            }
        });
    }

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        action: RecordingAction,
        stop: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.snapshot.can_control() || (stop && self.snapshot.state == RecordingState::Starting);
        div()
            .id(id)
            .size(px(36.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(if stop { crate::brand() } else { rgb(0x32343b).into() })
            .text_color(gpui::white())
            .tooltip(crate::timeline_ui::tooltip(label))
            .when(!enabled, |this| this.opacity(0.4))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(if stop { crate::brand_hover() } else { rgb(0x494c55).into() }))
                    .on_click(cx.listener(move |this, _, _, cx| this.action(action, cx)))
            })
            .child(if stop {
                div()
                    .size(px(12.0))
                    .rounded(px(2.0))
                    .bg(gpui::white())
                    .into_any_element()
            } else if self.snapshot.state == RecordingState::Paused {
                svg()
                    .path("icons/play.svg")
                    .size(px(14.0))
                    .text_color(gpui::white())
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .gap(px(4.0))
                    .child(
                        div()
                            .w(px(3.0))
                            .h(px(13.0))
                            .rounded(px(1.0))
                            .bg(gpui::white()),
                    )
                    .child(
                        div()
                            .w(px(3.0))
                            .h(px(13.0))
                            .rounded(px(1.0))
                            .bg(gpui::white()),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }
}

impl Render for RecordingPill {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let paused = self.snapshot.state == RecordingState::Paused;
        div()
            .size_full()
            .p(px(3.0))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag.end()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag.end()),
            )
            .child(
                div()
                    .id("recording-pill")
                    .size_full()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(0x42444c))
                    .bg(rgb(0x191a20))
                    .text_color(gpui::white())
                    .font_family("Inter")
                    .child(
                        div()
                            .id("recording-pill-drag")
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .gap(px(9.0))
                            .cursor(gpui::CursorStyle::OpenHand)
                            .tooltip(crate::timeline_ui::tooltip(
                                "Drag to move recording controls",
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event, window, cx| {
                                    this.drag.begin(this.exclusion.title(), event, window, cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0x777b87))
                                    .text_size(px(16.0))
                                    .child("⠿"),
                            )
                            .child(div().size(px(7.0)).rounded_full().bg(if paused {
                                gpui::Hsla::from(rgb(0xf0b963))
                            } else if self.snapshot.state == RecordingState::Recording {
                                crate::brand()
                            } else {
                                gpui::Hsla::from(rgb(0x999eab))
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .line_height(px(16.0))
                                            .tabular()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.snapshot.timecode.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .line_height(px(12.0))
                                            .text_color(rgb(0xb5b8c3))
                                            .child(self.snapshot.label().to_owned()),
                                    ),
                            ),
                    )
                    .child(self.button(
                        "recording-pill-pause",
                        if paused {
                            "Resume recording"
                        } else {
                            "Pause recording"
                        },
                        if paused {
                            RecordingAction::Resume
                        } else {
                            RecordingAction::Pause
                        },
                        false,
                        cx,
                    ))
                    .child(self.button(
                        "recording-pill-stop",
                        if self.snapshot.state == RecordingState::Starting { "Cancel recording" } else { "Stop and save recording" },
                        RecordingAction::Stop,
                        true,
                        cx,
                    )),
            )
    }
}

/// A preview from the recorder's existing camera stream; never owns a device.
pub(crate) struct RecordingCamera {
    exclusion: CaptureExclusion,
    snapshot: RecordingSnapshot,
    overlay: CameraOverlay,
    generation: u64,
    image: Option<Arc<RenderImage>>,
    image_aspect: f64,
    last_frame: Option<Instant>,
    waiting: bool,
    positioned_source: Option<CaptureSourceGeometry>,
    drag: OverlayDrag,
    _ticker: gpui::Task<()>,
}

impl RecordingCamera {
    fn new(
        studio: WeakEntity<Studio>,
        frames: Arc<CameraFrames>,
        snapshot: RecordingSnapshot,
        overlay: CameraOverlay,
        exclusion: CaptureExclusion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Skip an idle preview left in the slot; the recording tee must
        // publish a fresh frame before this monitor calls it live.
        let generation = frames
            .newer_than(0)
            .map(|(generation, _)| generation)
            .unwrap_or(0);
        let handle = window.window_handle();
        let request = exclusion.clone();
        let mut checked = Instant::now();
        let ticker = cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(67)).await;
            let Ok((snapshot, overlay, canvas_aspect)) = studio.read_with(cx, |studio, _| {
                let source_aspect = studio.recording_controller.as_ref()
                    .and_then(|controller| controller.source_geometry())
                    .map(|geometry| geometry.size.0 as f64 / geometry.size.1.max(1) as f64);
                let canvas_aspect = if studio.aspect_ratio == 0 {
                    source_aspect.unwrap_or(studio.selected_canvas_ratio() as f64)
                } else { studio.selected_canvas_ratio() as f64 };
                (
                    RecordingSnapshot::from_studio(studio),
                    studio.camera_overlay,
                    canvas_aspect,
                )
            }) else {
                let _ = handle.update(cx, |root, window, cx| {
                    if let Ok(view) = root.downcast::<RecordingCamera>() {
                        view.update(cx, |this, _| {
                            if let Some(image) = this.image.take() {
                                let _ = window.drop_image(image);
                            }
                        });
                    }
                    window.remove_window();
                    request.retire_closed_window();
                });
                break;
            };
            let Ok((lost, seen, live)) = weak.update(cx, |this, cx| {
                let waiting = this
                    .last_frame
                    .is_none_or(|last| last.elapsed() >= STALE_CAMERA);
                if this.snapshot != snapshot || this.overlay != overlay || this.waiting != waiting {
                    this.snapshot = snapshot;
                    this.overlay = overlay;
                    this.waiting = waiting;
                    cx.notify();
                }
                (
                    protection_lost(&this.exclusion, &mut checked),
                    this.generation,
                    this.snapshot.state == RecordingState::Recording && !this.snapshot.busy,
                )
            }) else {
                break;
            };
            if lost {
                restore_controls(&studio, cx);
                break;
            }
            if !live {
                continue;
            }
            let Some((generation, frame)) = frames.newer_than(seen) else {
                continue;
            };
            // Bounded, one-at-a-time work off the UI thread. A slow producer
            // simply replaces the shared slot instead of queueing frames.
            let pixels = cx
                .background_executor()
                .spawn(async move { monitor_pixels(frame, overlay, canvas_aspect) })
                .await;
            let Some(pixels) = pixels else { continue };
            if handle
                .update(cx, |root, window, cx| {
                    let Ok(view) = root.downcast::<RecordingCamera>() else {
                        return;
                    };
                    view.update(cx, |this, cx| {
                        this.generation = generation;
                        this.image_aspect = pixels.width() as f64 / pixels.height().max(1) as f64;
                        let previous = this.image.replace(cached_render_image(pixels));
                        if let Some(previous) = previous {
                            let _ = window.drop_image(previous);
                        }
                        this.last_frame = Some(Instant::now());
                        this.waiting = false;
                        cx.notify();
                    });
                })
                .is_err()
            {
                break;
            }
        });
        // Release the final texture even when a native window is closed by
        // its window manager, rather than by close_recording_overlays.
        cx.on_release_in(window, |this, window, _| {
            if let Some(image) = this.image.take() {
                let _ = window.drop_image(image);
            }
        })
        .detach();
        window.on_window_should_close(cx, |_, _| false);
        Self {
            exclusion,
            snapshot,
            overlay,
            generation,
            image: None,
            image_aspect: overlay.aspect_ratio(1.0),
            last_frame: None,
            waiting: true,
            positioned_source: None,
            drag: OverlayDrag::default(),
            _ticker: ticker,
        }
    }

    fn badge(&self) -> Option<&str> {
        match self.snapshot.state {
            RecordingState::Paused => Some("Camera paused"),
            RecordingState::Finishing => Some("Saving recording…"),
            RecordingState::Starting if self.snapshot.start_label != "Starting…" => Some(&self.snapshot.start_label),
            RecordingState::Idle | RecordingState::Starting => Some("Starting camera…"),
            RecordingState::Recording if self.waiting => Some("Waiting for camera…"),
            RecordingState::Recording if self.snapshot.busy => Some("Updating…"),
            RecordingState::Recording => None,
        }
    }
}

impl Render for RecordingCamera {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rect = crate::recording::camera_layout::fit_rect(
            Rect { x: 0.0, y: 0.0, width: (CAMERA_SIZE - 6.0) as f64, height: (CAMERA_SIZE - 6.0) as f64 },
            self.image_aspect,
        );
        let radius = self.overlay.radius(rect) as f32;
        let badge = self.badge().map(str::to_owned);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(3.0))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag.end()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag.end()),
            )
            .child(
                div()
                    .id("recording-camera-monitor")
                    .relative()
                    .w(px(rect.width as f32))
                    .h(px(rect.height as f32))
                    .rounded(px(radius))
                    .overflow_hidden()
                    .border_2()
                    .border_color(rgb(0x545761))
                    .bg(rgb(0x191a20))
                    .cursor(gpui::CursorStyle::OpenHand)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| {
                            this.drag.begin(this.exclusion.title(), event, window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .when_some(self.image.clone(), |this, image| {
                        this.child(
                            gpui::img(image)
                                .size_full()
                                .rounded(px(radius))
                                .object_fit(gpui::ObjectFit::Cover)
                                .when(badge.is_some(), |this| this.opacity(0.45)),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .top(px(12.0))
                            .left_0()
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .w(px(26.0))
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(gpui::hsla(0.0, 0.0, 1.0, 0.65)),
                            ),
                    )
                    .when_some(badge, |this, label| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .rounded_full()
                                        .bg(rgb(0x252730))
                                        .text_color(gpui::white())
                                        .text_size(px(11.0))
                                        .child(label),
                                ),
                        )
                    }),
            )
    }
}

fn overlay_bounds(display: Bounds<Pixels>, camera: bool) -> Bounds<Pixels> {
    let width = px(if camera { CAMERA_SIZE } else { PILL_WIDTH });
    let height = px(if camera { CAMERA_SIZE } else { PILL_HEIGHT });
    let horizontal_space = (display.size.width - width).max(px(0.0));
    let vertical_space = (display.size.height - height).max(px(0.0));
    let x = if camera {
        (horizontal_space - px(EDGE_MARGIN)).max(px(0.0))
    } else {
        horizontal_space / 2.0
    };
    let y = (vertical_space - px(EDGE_MARGIN)).max(px(0.0));
    Bounds {
        origin: display.origin + point(x, y),
        size: size(width, height),
    }
}

fn overlay_options(bounds: Bounds<Pixels>, display_id: Option<gpui::DisplayId>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("com.lahza.Lahza".into()),
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DesktopRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn monitor_containing_window(window: DesktopRect, monitors: &[DesktopRect]) -> Option<DesktopRect> {
    monitors.iter().copied().filter_map(|monitor| {
        let width = ((window.x as i64 + window.width as i64)
            .min(monitor.x as i64 + monitor.width as i64)
            - (window.x as i64).max(monitor.x as i64)).max(0);
        let height = ((window.y as i64 + window.height as i64)
            .min(monitor.y as i64 + monitor.height as i64)
            - (window.y as i64).max(monitor.y as i64)).max(0);
        let area = width * height;
        (area > 0).then_some((area, monitor))
    }).max_by_key(|(area, _)| *area).map(|(_, monitor)| monitor)
}

fn desktop_rect(bounds: Bounds<Pixels>, scale: f32) -> DesktopRect {
    DesktopRect {
        x: (f32::from(bounds.origin.x) * scale).round() as i32,
        y: (f32::from(bounds.origin.y) * scale).round() as i32,
        width: (f32::from(bounds.size.width) * scale).round().max(0.0) as u32,
        height: (f32::from(bounds.size.height) * scale).round().max(0.0) as u32,
    }
}

fn monitor_bounds(monitor: DesktopRect, scale: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(monitor.x as f32 / scale), px(monitor.y as f32 / scale)),
        size: size(px(monitor.width as f32 / scale), px(monitor.height as f32 / scale)),
    }
}

/// GPUI's X11 display covers the entire desktop, including gaps between
/// unequal monitors. Anchor both windows to one real monitor before mapping
/// them, including while the portal is open and when recording a window.
fn initial_overlay_monitor(launcher: Option<DesktopRect>) -> PlacementResult<DesktopRect> {
    use x11rb::{connection::Connection, protocol::{randr::ConnectionExt, xproto::ConnectionExt as _}};
    let (connection, index) = x11rb::connect(None)?;
    let root = connection.setup().roots[index].root;
    let info = connection.randr_get_monitors(root, true)?.reply()?;
    let monitors: Vec<_> = info.monitors.iter().filter(|monitor| monitor.width > 0 && monitor.height > 0)
        .map(|monitor| DesktopRect {
            x: monitor.x.into(), y: monitor.y.into(),
            width: monitor.width.into(), height: monitor.height.into(),
        }).collect();
    if let Some(monitor) = launcher.and_then(|launcher| monitor_containing_window(launcher, &monitors)) {
        return Ok(monitor);
    }
    if let Ok(pointer) = connection.query_pointer(root)?.reply() {
        let pointer = DesktopRect { x: pointer.root_x.into(), y: pointer.root_y.into(), width: 1, height: 1 };
        if let Some(monitor) = monitor_containing_window(pointer, &monitors) {
            return Ok(monitor);
        }
    }
    info.monitors.iter().find(|monitor| monitor.primary && monitor.width > 0 && monitor.height > 0)
        .map(|monitor| DesktopRect {
            x: monitor.x.into(), y: monitor.y.into(),
            width: monitor.width.into(), height: monitor.height.into(),
        })
        .or_else(|| monitors.first().copied())
        .ok_or_else(|| "no active recording overlay display".into())
}

/// Portal positions are compositor coordinates, whereas RandR reports the
/// X11/XWayland desktop. Accept only an unambiguous position/scale match.
fn capture_monitor(source: CaptureSourceGeometry, monitors: &[DesktopRect]) -> Option<DesktopRect> {
    if !source.is_monitor || source.size.0 <= 0 || source.size.1 <= 0 {
        return None;
    }
    let valid: Vec<_> = monitors
        .iter()
        .copied()
        .filter(|monitor| monitor.width > 0 && monitor.height > 0)
        .collect();
    if valid.len() == 1 {
        return Some(valid[0]);
    }
    if let Some(exact) = valid.iter().find(|monitor| {
        (monitor.x, monitor.y) == source.position
            && (monitor.width as i32, monitor.height as i32) == source.size
    }) {
        return Some(*exact);
    }
    let matches: Vec<_> = valid
        .into_iter()
        .filter(|monitor| {
            let sx = monitor.width as f64 / source.size.0 as f64;
            let sy = monitor.height as f64 / source.size.1 as f64;
            let close = |a: f64, b: f64| (a - b).abs() < 3.0;
            let scaled_origin = close(monitor.x as f64, source.position.0 as f64 * sx)
                && close(monitor.y as f64, source.position.1 as f64 * sy);
            let same_origin = close(monitor.x as f64, source.position.0 as f64)
                && close(monitor.y as f64, source.position.1 as f64);
            (sx - sy).abs() < 0.01 && (scaled_origin || same_origin)
        })
        .collect();
    (matches.len() == 1).then(|| matches[0])
}

fn desktop_overlay_origin(
    monitor: DesktopRect,
    width: u32,
    height: u32,
    camera: bool,
) -> (i32, i32) {
    let scale = width as f64
        / if camera {
            CAMERA_SIZE as f64
        } else {
            PILL_WIDTH as f64
        };
    let margin = (EDGE_MARGIN as f64 * scale).round() as i32;
    let horizontal = (monitor.width as i64 - width as i64).max(0) as i32;
    let vertical = (monitor.height as i64 - height as i64).max(0) as i32;
    (
        monitor.x
            + if camera {
                (horizontal - margin).max(0)
            } else {
                horizontal / 2
            },
        monitor.y + (vertical - margin).max(0),
    )
}

#[derive(Clone)]
struct OverlayPlacementTarget {
    title: String,
    camera: bool,
}

type PlacementResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn move_native_overlays(
    connection: &x11rb::rust_connection::RustConnection,
    root: u32,
    monitor: DesktopRect,
    targets: &[OverlayPlacementTarget],
) -> PlacementResult<()> {
    use x11rb::{connection::Connection, protocol::xproto::*};
    let atom = |name: &[u8]| -> PlacementResult<u32> {
        Ok(connection.intern_atom(false, name)?.reply()?.atom)
    };
    let pid_atom = atom(b"_NET_WM_PID")?;
    let title_atom = atom(b"_NET_WM_NAME")?;
    let clients_atom = atom(b"_NET_CLIENT_LIST")?;
    let move_atom = atom(b"_NET_MOVERESIZE_WINDOW")?;
    let supported_atom = atom(b"_NET_SUPPORTED")?;
    let managed_move = connection
        .get_property(false, root, supported_atom, AtomEnum::ATOM, 0, 4096)?
        .reply()?
        .value32()
        .is_some_and(|mut atoms| atoms.any(|atom| atom == move_atom));
    let mut candidates: Vec<_> = connection
        .get_property(false, root, clients_atom, AtomEnum::WINDOW, 0, 4096)?
        .reply()?
        .value32()
        .map(|windows| windows.collect())
        .unwrap_or_default();
    // The client list handles reparenting window managers. The tree also
    // covers notification windows and bare Xvfb, which have no client list.
    let mut level = vec![root];
    for _ in 0..3 {
        let mut next = Vec::new();
        for parent in level {
            if let Ok(reply) = connection.query_tree(parent)?.reply() {
                next.extend(reply.children);
            }
        }
        candidates.extend(next.iter().copied());
        level = next;
    }
    candidates.sort_unstable();
    candidates.dedup();
    for candidate in candidates {
        let Ok(pid) = connection
            .get_property(false, candidate, pid_atom, AtomEnum::CARDINAL, 0, 1)?
            .reply()
        else {
            continue;
        };
        if pid.value32().and_then(|mut values| values.next()) != Some(std::process::id()) {
            continue;
        }
        let Ok(title) = connection
            .get_property(false, candidate, title_atom, AtomEnum::ANY, 0, 1024)?
            .reply()
        else {
            continue;
        };
        let Some(target) = targets
            .iter()
            .find(|target| target.title.as_bytes() == title.value.as_slice())
        else {
            continue;
        };
        let geometry = connection.get_geometry(candidate)?.reply()?;
        let (x, y) = desktop_overlay_origin(
            monitor,
            geometry.width.into(),
            geometry.height.into(),
            target.camera,
        );
        if managed_move
            && !connection
                .get_window_attributes(candidate)?
                .reply()?
                .override_redirect
        {
            // NorthWest gravity, x/y fields present, source=normal app. Let
            // the WM move its frame as well as the client's content window.
            let event = ClientMessageEvent::new(
                32,
                candidate,
                move_atom,
                [
                    1 | (1 << 8) | (1 << 9) | (1 << 12),
                    x as u32,
                    y as u32,
                    0,
                    0,
                ],
            );
            connection
                .send_event(
                    false,
                    root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    event,
                )?
                .check()?;
        } else {
            connection
                .configure_window(candidate, &ConfigureWindowAux::new().x(x).y(y))?
                .check()?;
        }
    }
    connection.flush()?;
    Ok(())
}

fn position_native_overlays(
    source: CaptureSourceGeometry,
    targets: &[OverlayPlacementTarget],
) -> PlacementResult<()> {
    use x11rb::{connection::Connection, protocol::randr::ConnectionExt};
    let (connection, index) = x11rb::connect(None)?;
    let root = connection.setup().roots[index].root;
    let monitors: Vec<_> = connection
        .randr_get_monitors(root, true)?
        .reply()?
        .monitors
        .into_iter()
        .map(|monitor| DesktopRect {
            x: monitor.x.into(),
            y: monitor.y.into(),
            width: monitor.width.into(),
            height: monitor.height.into(),
        })
        .collect();
    let monitor = capture_monitor(source, &monitors)
        .ok_or("could not map the selected recording display to the desktop")?;
    move_native_overlays(&connection, root, monitor, targets)
}

impl Studio {
    /// Called after the portal has selected a source, never on a playback or
    /// pause tick. Reuse the protected native windows and retain user drags
    /// if an already-positioned source is reported again.
    pub(crate) fn position_recording_overlays(
        &mut self,
        source: Option<CaptureSourceGeometry>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = source.filter(|source| source.is_monitor) else {
            return;
        };
        let mut targets = Vec::new();
        if let Some(pill) = self.recording_pill {
            if let Ok(Some(title)) = pill.update(cx, |pill, _, _| {
                if pill.positioned_source == Some(source) {
                    return None;
                }
                pill.positioned_source = Some(source);
                Some(pill.exclusion.title().to_owned())
            }) {
                targets.push(OverlayPlacementTarget {
                    title,
                    camera: false,
                });
            }
        }
        if let Some(camera) = self.recording_camera_window {
            if let Ok(Some(title)) = camera.update(cx, |camera, _, _| {
                if camera.positioned_source == Some(source) {
                    return None;
                }
                camera.positioned_source = Some(source);
                Some(camera.exclusion.title().to_owned())
            }) {
                targets.push(OverlayPlacementTarget {
                    title,
                    camera: true,
                });
            }
        }
        if targets.is_empty() {
            return;
        }
        // The app intentionally runs through XWayland on GNOME (main.rs).
        // Its display is a whole X screen, not an individual RandR monitor;
        // GPUI display_id therefore cannot place these on the portal source.
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = position_native_overlays(source, &targets) {
                    eprintln!("Could not position recording overlays: {error}");
                }
            })
            .detach();
    }

    pub(crate) fn open_recording_overlays(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<Vec<CaptureExclusion>, String> {
        self.close_recording_overlays(cx);
        let launcher = self
            .window_handle
            .update(cx, |_, window, cx| (window.bounds(), window.scale_factor(), window.display(cx)))
            .ok();
        let scale = launcher.as_ref().map(|(_, scale, _)| *scale).unwrap_or(1.0);
        let display = launcher.as_ref().and_then(|(_, _, display)| display.clone())
            .or_else(|| cx.primary_display());
        let fallback_bounds = display
            .as_ref()
            .map(|display| display.bounds())
            .unwrap_or_else(|| Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(1280.0), px(720.0)),
            });
        let bounds = match initial_overlay_monitor(launcher.map(|(bounds, scale, _)| desktop_rect(bounds, scale))) {
            Ok(monitor) => monitor_bounds(monitor, scale),
            Err(error) => {
                eprintln!("Could not locate recording overlay monitor: {error}");
                fallback_bounds
            }
        };
        let display_id = display.as_ref().map(|display| display.id());
        let owner = cx.entity().downgrade();
        let snapshot = RecordingSnapshot::from_studio(self);
        let pill_exclusion = CaptureExclusion::for_window("Recording Controls")?;
        // Prepare both requests before opening either window, so an error
        // cannot leave one visible overlay without a returned protection gate.
        let camera_exclusion = self
            .record_camera
            .then(|| CaptureExclusion::for_window("Camera Preview"))
            .transpose()?;
        let exclusion = pill_exclusion.clone();
        let studio = owner.clone();
        let initial = snapshot.clone();
        let pill = cx
            .open_window(
                overlay_options(overlay_bounds(bounds, false), display_id),
                move |window, cx| {
                    window.set_window_title(exclusion.title());
                    crate::theme::follow_appearance(window);
                    cx.new(|cx| RecordingPill::new(studio, initial, exclusion, window, cx))
                },
            )
            .map_err(|error| format!("Could not open recording controls: {error}"))?;
        self.recording_pill = Some(pill);
        let mut exclusions = vec![pill_exclusion];
        if let Some(camera_exclusion) = camera_exclusion {
            let exclusion = camera_exclusion.clone();
            let frames = self.camera_frames.clone();
            let overlay = self.camera_overlay;
            let camera = cx.open_window(
                overlay_options(overlay_bounds(bounds, true), display_id),
                move |window, cx| {
                    window.set_window_title(exclusion.title());
                    cx.new(|cx| {
                        RecordingCamera::new(
                            owner, frames, snapshot, overlay, exclusion, window, cx,
                        )
                    })
                },
            );
            match camera {
                Ok(camera) => self.recording_camera_window = Some(camera),
                Err(error) => {
                    self.close_recording_overlays(cx);
                    return Err(format!("Could not open camera preview: {error}"));
                }
            }
            exclusions.push(camera_exclusion);
        }
        Ok(exclusions)
    }

    pub(crate) fn close_recording_overlays(&mut self, cx: &mut Context<Self>) {
        if let Some(camera) = self.recording_camera_window.take() {
            let _ = camera.update(cx, |camera, window, _| {
                if let Some(image) = camera.image.take() {
                    let _ = window.drop_image(image);
                }
                window.remove_window();
                camera.exclusion.retire_closed_window();
            });
        }
        if let Some(pill) = self.recording_pill.take() {
            let _ = pill.update(cx, |pill, window, _| {
                window.remove_window();
                pill.exclusion.retire_closed_window();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_overlays_share_the_launcher_monitor_in_an_uneven_desktop() {
        let wide = DesktopRect { x: 0, y: 0, width: 3440, height: 1440 };
        let portrait = DesktopRect { x: 3440, y: 0, width: 1080, height: 1920 };
        let monitors = [wide, portrait];
        for (launcher, expected) in [
            (DesktopRect { x: 1500, y: 500, width: 430, height: 610 }, wide),
            (DesktopRect { x: 3700, y: 500, width: 430, height: 610 }, portrait),
            (DesktopRect { x: 3300, y: 500, width: 430, height: 610 }, portrait),
        ] {
            let monitor = monitor_containing_window(launcher, &monitors).unwrap();
            assert_eq!(monitor, expected);
            let bounds = monitor_bounds(monitor, 1.0);
            for camera in [false, true] {
                let overlay = desktop_rect(overlay_bounds(bounds, camera), 1.0);
                assert!(overlay.x >= monitor.x && overlay.y >= monitor.y);
                assert!(overlay.x as i64 + overlay.width as i64 <= monitor.x as i64 + monitor.width as i64);
                assert!(overlay.y as i64 + overlay.height as i64 <= monitor.y as i64 + monitor.height as i64);
            }
        }
        // The old whole-desktop placement put the pill in this gap.
        let gap = DesktopRect { x: 2116, y: 1832, width: 292, height: 56 };
        assert_eq!(monitor_containing_window(gap, &monitors), None);
    }

    #[test]
    fn initial_overlay_monitor_conversion_preserves_negative_and_scaled_bounds() {
        let left = DesktopRect { x: -2560, y: -120, width: 2560, height: 1440 };
        for scale in [1.0, 1.25, 2.0] {
            let logical = monitor_bounds(left, scale);
            assert_eq!(desktop_rect(logical, scale), left);
            let launcher = Bounds {
                origin: point(px(-900.0 / scale), px(200.0 / scale)),
                size: size(px(430.0 / scale), px(610.0 / scale)),
            };
            assert_eq!(monitor_containing_window(desktop_rect(launcher, scale), &[left]), Some(left));
        }
    }

    #[test]
    fn exact_capture_monitor_wins_over_an_ambiguous_scaled_match() {
        let primary = DesktopRect { x: 0, y: 0, width: 1920, height: 1080 };
        let next = DesktopRect { x: 1920, ..primary };
        let scaled = DesktopRect { x: 3840, y: 0, width: 3840, height: 2160 };
        let source = CaptureSourceGeometry { position: (1920, 0), size: (1920, 1080), is_monitor: true };
        assert_eq!(capture_monitor(source, &[primary, next, scaled]), Some(next));
    }

    #[test]
    fn selected_ultrawide_or_portrait_monitor_controls_overlay_placement() {
        let wide = DesktopRect {
            x: 0,
            y: 0,
            width: 3440,
            height: 1440,
        };
        let portrait = DesktopRect {
            x: 3440,
            y: 0,
            width: 1080,
            height: 1920,
        };
        let source = |position, size| CaptureSourceGeometry {
            position,
            size,
            is_monitor: true,
        };
        assert_eq!(
            capture_monitor(source((0, 0), (3440, 1440)), &[wide, portrait]),
            Some(wide)
        );
        assert_eq!(
            capture_monitor(source((3440, 0), (1080, 1920)), &[wide, portrait]),
            Some(portrait)
        );
        assert_eq!(desktop_overlay_origin(wide, 292, 56, false), (1574, 1352));
        assert_eq!(desktop_overlay_origin(wide, 216, 216, true), (3192, 1192));
        assert_eq!(
            desktop_overlay_origin(portrait, 292, 56, false),
            (3834, 1832)
        );
        assert_eq!(
            desktop_overlay_origin(portrait, 216, 216, true),
            (4272, 1672)
        );
    }

    #[test]
    fn portal_logical_coordinates_map_to_scaled_monitors_without_guessing() {
        let primary = DesktopRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let scaled = DesktopRect {
            x: 3840,
            y: 0,
            width: 5120,
            height: 2880,
        };
        let source = CaptureSourceGeometry {
            position: (1920, 0),
            size: (2560, 1440),
            is_monitor: true,
        };
        assert_eq!(capture_monitor(source, &[primary, scaled]), Some(scaled));
        let mixed = DesktopRect { x: 1920, ..scaled };
        assert_eq!(capture_monitor(source, &[primary, mixed]), Some(mixed));
        let left = DesktopRect {
            x: -2560,
            y: -120,
            width: 2560,
            height: 1440,
        };
        assert_eq!(
            capture_monitor(
                CaptureSourceGeometry {
                    position: (-2560, -120),
                    ..source
                },
                &[primary, left]
            ),
            Some(left)
        );
        assert_eq!(capture_monitor(source, &[primary]), Some(primary));
        assert_eq!(capture_monitor(source, &[primary, scaled, mixed]), None);
        assert_eq!(
            capture_monitor(
                CaptureSourceGeometry {
                    is_monitor: false,
                    ..source
                },
                &[primary]
            ),
            None
        );
        assert_eq!(
            capture_monitor(
                CaptureSourceGeometry {
                    size: (0, 1440),
                    ..source
                },
                &[primary]
            ),
            None
        );
    }

    #[test]
    #[ignore = "requires X11 or XWayland; run under Xvfb"]
    fn native_overlay_placement_moves_only_exact_owned_windows_to_capture_monitor() {
        use x11rb::{
            connection::Connection,
            protocol::xproto::{ConnectionExt as _, *},
            wrapper::ConnectionExt as _,
        };
        let (connection, index) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[index].root;
        let pid_atom = connection
            .intern_atom(false, b"_NET_WM_PID")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let title_atom = connection
            .intern_atom(false, b"_NET_WM_NAME")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let utf8_atom = connection
            .intern_atom(false, b"UTF8_STRING")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let pill_title = format!("Lahza placement pill {}", uuid::Uuid::new_v4());
        let camera_title = format!("Lahza placement camera {}", uuid::Uuid::new_v4());
        let create = |title: &str, pid: u32, width, height| {
            let window = connection.generate_id().unwrap();
            connection
                .create_window(
                    x11rb::COPY_DEPTH_FROM_PARENT,
                    window,
                    root,
                    20,
                    30,
                    width,
                    height,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    0,
                    &CreateWindowAux::new().override_redirect(1),
                )
                .unwrap()
                .check()
                .unwrap();
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    pid_atom,
                    AtomEnum::CARDINAL,
                    &[pid],
                )
                .unwrap();
            connection
                .change_property8(
                    PropMode::REPLACE,
                    window,
                    title_atom,
                    utf8_atom,
                    title.as_bytes(),
                )
                .unwrap();
            connection.map_window(window).unwrap();
            window
        };
        let pill = create(&pill_title, std::process::id(), 292, 56);
        let camera = create(&camera_title, std::process::id(), 216, 216);
        let unrelated = create("Another Lahza window", std::process::id(), 292, 56);
        let other_process = create(&pill_title, std::process::id() + 1, 292, 56);
        let selected = DesktopRect {
            x: 800,
            y: 0,
            width: 800,
            height: 1000,
        };
        move_native_overlays(
            &connection,
            root,
            selected,
            &[
                OverlayPlacementTarget {
                    title: pill_title,
                    camera: false,
                },
                OverlayPlacementTarget {
                    title: camera_title,
                    camera: true,
                },
            ],
        )
        .unwrap();
        let origin = |window| {
            let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
            (i32::from(geometry.x), i32::from(geometry.y))
        };
        assert_eq!(origin(pill), (1054, 912));
        assert_eq!(origin(camera), (1352, 752));
        assert_eq!(origin(unrelated), (20, 30));
        assert_eq!(origin(other_process), (20, 30));
        for window in [pill, camera, unrelated, other_process] {
            connection.destroy_window(window).unwrap().check().unwrap();
        }
    }

    #[test]
    fn monitor_center_crops_before_mirroring_and_rounding() {
        let pixels =
            image::RgbaImage::from_fn(4, 2, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        let frame = CameraFrame {
            width: 4,
            height: 2,
            rgba: pixels.into_raw(),
        };
        let normal = monitor_pixels(frame.clone(), CameraOverlay::default(), 16.0 / 9.0).unwrap();
        let mirrored = monitor_pixels(frame, CameraOverlay { mirror: true, ..Default::default() }, 16.0 / 9.0).unwrap();
        assert_eq!(normal.dimensions(), (2, 2));
        assert_eq!(normal.get_pixel(0, 0).0, [1, 0, 0, 255]);
        assert_eq!(normal.get_pixel(1, 1).0, [2, 1, 0, 255]);
        assert_eq!(mirrored.get_pixel(0, 0), normal.get_pixel(1, 0));
        assert_eq!(mirrored.get_pixel(1, 1), normal.get_pixel(0, 1));
    }

    #[test]
    fn camera_only_monitor_uses_the_selected_canvas_crop() {
        let pixels = image::RgbaImage::from_fn(160, 90, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        let frame = CameraFrame { width: 160, height: 90, rgba: pixels.into_raw() };
        let overlay = CameraOverlay { layout: crate::recording::camera_layout::CameraLayout::Fullscreen, ..Default::default() };
        let wide = monitor_pixels(frame.clone(), overlay, 16.0 / 9.0).unwrap();
        let portrait = monitor_pixels(frame, overlay, 9.0 / 16.0).unwrap();
        assert_eq!(wide.dimensions(), (160, 90));
        assert_eq!(portrait.dimensions(), (51, 90));
        assert_eq!(portrait.get_pixel(0, 0).0[0], 55);
    }

    #[test]
    #[ignore = "requires X11 or XWayland; run under Xvfb"]
    fn native_managed_overlay_placement_sends_window_manager_move_request() {
        use x11rb::{
            connection::Connection,
            protocol::{
                xproto::{ConnectionExt as _, *},
                Event,
            },
            wrapper::ConnectionExt as _,
        };
        let (connection, index) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[index].root;
        let atom = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let pid_atom = atom(b"_NET_WM_PID");
        let title_atom = atom(b"_NET_WM_NAME");
        let supported_atom = atom(b"_NET_SUPPORTED");
        let move_atom = atom(b"_NET_MOVERESIZE_WINDOW");
        let utf8_atom = atom(b"UTF8_STRING");
        // A private parent emulates the WM's root. Its advertised EWMH
        // support and event listener never alter the real desktop's root.
        let desktop = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                desktop,
                root,
                0,
                0,
                1600,
                1000,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .event_mask(EventMask::SUBSTRUCTURE_NOTIFY),
            )
            .unwrap()
            .check()
            .unwrap();
        connection
            .change_property32(
                PropMode::REPLACE,
                desktop,
                supported_atom,
                AtomEnum::ATOM,
                &[move_atom],
            )
            .unwrap();
        let camera = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                camera,
                desktop,
                20,
                30,
                216,
                216,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )
            .unwrap()
            .check()
            .unwrap();
        let title = format!("Lahza managed camera {}", uuid::Uuid::new_v4());
        connection
            .change_property32(
                PropMode::REPLACE,
                camera,
                pid_atom,
                AtomEnum::CARDINAL,
                &[std::process::id()],
            )
            .unwrap();
        connection
            .change_property8(
                PropMode::REPLACE,
                camera,
                title_atom,
                utf8_atom,
                title.as_bytes(),
            )
            .unwrap();
        let focus = connection.get_input_focus().unwrap().reply().unwrap().focus;
        move_native_overlays(
            &connection,
            desktop,
            DesktopRect {
                x: 800,
                y: 0,
                width: 800,
                height: 1000,
            },
            &[OverlayPlacementTarget {
                title,
                camera: true,
            }],
        )
        .unwrap();
        assert_eq!(
            connection.get_input_focus().unwrap().reply().unwrap().focus,
            focus
        );
        let mut requested = None;
        while let Some(event) = connection.poll_for_event().unwrap() {
            if let Event::ClientMessage(message) = event {
                if message.type_ == move_atom {
                    requested = Some(message);
                }
            }
        }
        let requested = requested.expect("managed overlays request a compositor move");
        assert_eq!(requested.window, camera);
        assert_eq!(
            requested.data.as_data32(),
            [1 | (1 << 8) | (1 << 9) | (1 << 12), 1352, 752, 0, 0]
        );
        let geometry = connection.get_geometry(camera).unwrap().reply().unwrap();
        assert_eq!(
            (geometry.x, geometry.y),
            (20, 30),
            "the WM owns managed-window positioning"
        );
        connection.destroy_window(desktop).unwrap().check().unwrap();
    }

    #[test]
    fn only_ready_active_recordings_accept_controls() {
        let mut snapshot = RecordingSnapshot {
            state: RecordingState::Starting,
            busy: false,
            has_controller: true,
            timecode: "00:00".into(),
            start_label: "Starting…".into(),
        };
        for state in [
            RecordingState::Idle,
            RecordingState::Starting,
            RecordingState::Finishing,
        ] {
            snapshot.state = state;
            assert!(!snapshot.can_control());
        }
        for state in [RecordingState::Recording, RecordingState::Paused] {
            snapshot.state = state;
            assert!(snapshot.can_control());
            snapshot.busy = true;
            assert!(!snapshot.can_control());
            snapshot.busy = false;
            snapshot.has_controller = false;
            assert!(!snapshot.can_control());
            snapshot.has_controller = true;
        }
    }

    #[test]
    fn floating_windows_stay_on_the_selected_display() {
        let display = Bounds {
            origin: point(px(-1920.0), px(120.0)),
            size: size(px(1920.0), px(1080.0)),
        };
        let pill = overlay_bounds(display, false);
        let camera = overlay_bounds(display, true);
        assert_eq!(pill.origin.x + pill.size.width / 2.0, px(-960.0));
        assert_eq!(camera.origin.x + camera.size.width, px(-EDGE_MARGIN));
        assert_eq!(
            camera.origin.y + camera.size.height,
            px(1200.0 - EDGE_MARGIN)
        );
        let tiny = Bounds {
            origin: point(px(100.0), px(-200.0)),
            size: size(px(150.0), px(150.0)),
        };
        assert_eq!(overlay_bounds(tiny, true).origin, tiny.origin);
    }

    /// Exercises real native windows and GPU image replacement without a
    /// camera device, portal picker, capture pipeline, or GNOME extension.
    #[test]
    #[ignore = "requires a native display (run under Xvfb)"]
    fn native_recording_overlays_share_frames_and_restore_controls_on_protection_loss() {
        use crate::recording::camera_preview::CameraFrame;
        use crate::{
            asset_directory, open_studio_window, Assets, NativeRecorder, RecordingController,
        };
        use gpui::Application;

        Application::new()
            .with_assets(Assets {
                base: asset_directory(),
            })
            .run(|cx| {
                let launcher = open_studio_window(cx, true, |handle, cx| {
                    cx.new(|cx| Studio::new(handle, None, None, cx))
                })
                .unwrap();
                let owner = launcher.entity(cx).unwrap();
                let (pill, camera, frames, exclusions, original_overlay) =
                    owner.update(cx, |studio, cx| {
                        studio.load_generation = studio.load_generation.wrapping_add(1);
                        studio.loading = None;
                        studio.record_camera = true;
                        studio.recording_camera_enabled = true;
                        studio.recording_state = RecordingState::Starting;
                        studio.recording_busy = true;
                        // An idle backend is enough to make controls available in
                        // this fixture; no enabled action is sent to the backend.
                        studio.recording_controller =
                            Some(RecordingController::new(NativeRecorder::new()));
                        let exclusions = studio.open_recording_overlays(cx).unwrap();
                        assert_eq!(exclusions.len(), 2);
                        assert_ne!(exclusions[0].title(), exclusions[1].title());
                        (
                            studio.recording_pill.unwrap(),
                            studio.recording_camera_window.unwrap(),
                            studio.camera_frames.clone(),
                            exclusions,
                            studio.camera_overlay,
                        )
                    });
                cx.spawn(async move |cx| {
                    Timer::after(Duration::from_millis(200)).await;
                    pill.update(cx, |pill, window, _| {
                        assert_eq!(pill.snapshot.label(), "Starting…");
                        assert!(!pill.snapshot.can_control());
                        assert_eq!(window.bounds().size.width, px(PILL_WIDTH));
                    })
                    .unwrap();
                    camera
                        .update(cx, |camera, _, _| {
                            assert!(camera.image.is_none());
                            assert_eq!(camera.badge(), Some("Starting camera…"));
                        })
                        .unwrap();
                    owner
                        .update(cx, |studio, cx| {
                            studio.recording_state = RecordingState::Recording;
                            studio.recording_busy = false;
                            studio.recording_elapsed = Duration::from_secs(65);
                            cx.notify();
                        })
                        .unwrap();
                    let publish = |color| {
                        frames.publish(CameraFrame {
                            width: 320,
                            height: 180,
                            rgba: image::RgbaImage::from_pixel(320, 180, image::Rgba(color))
                                .into_raw(),
                        })
                    };
                    publish([45, 145, 205, 255]);
                    Timer::after(Duration::from_millis(200)).await;
                    pill.update(cx, |pill, _, _| {
                        assert_eq!(pill.snapshot.timecode, "01:05");
                        assert_eq!(pill.snapshot.label(), "Recording");
                        assert!(pill.snapshot.can_control());
                    })
                    .unwrap();
                    let first = camera
                        .update(cx, |camera, _, _| {
                            assert!(camera.badge().is_none());
                            assert_eq!(camera.generation, 1);
                            camera.image.clone().unwrap()
                        })
                        .unwrap();
                    publish([205, 125, 45, 255]);
                    Timer::after(Duration::from_millis(200)).await;
                    camera
                        .update(cx, |camera, _, _| {
                            assert_eq!(camera.generation, 2);
                            assert!(!Arc::ptr_eq(&first, camera.image.as_ref().unwrap()));
                        })
                        .unwrap();
                    if let Some(path) = std::env::var_os("LAHZA_RECORDING_OVERLAY_SCREENSHOT") {
                        let camera_window: gpui::AnyWindowHandle = camera.into();
                        let capture_size = camera_window
                            .update(cx, |_, window, cx| {
                                window.draw(cx).clear();
                                let bounds = window.display(cx).unwrap().bounds();
                                format!(
                                    "{}x{}",
                                    f32::from(bounds.size.width) as u32,
                                    f32::from(bounds.size.height) as u32
                                )
                            })
                            .unwrap();
                        let pill_window: gpui::AnyWindowHandle = pill.into();
                        pill_window
                            .update(cx, |_, window, cx| window.draw(cx).clear())
                            .unwrap();
                        // draw constructs the scene; the next native frame
                        // presents it. Let that callback run before x11grab.
                        Timer::after(Duration::from_millis(150)).await;
                        let captured = std::process::Command::new("ffmpeg")
                            .args([
                                "-hide_banner",
                                "-loglevel",
                                "error",
                                "-f",
                                "x11grab",
                                "-video_size",
                                &capture_size,
                                "-i",
                                &std::env::var("DISPLAY").unwrap(),
                                "-frames:v",
                                "1",
                                "-y",
                            ])
                            .arg(path)
                            .status()
                            .unwrap();
                        assert!(captured.success());
                    }
                    owner
                        .update(cx, |studio, cx| {
                            studio.recording_state = RecordingState::Paused;
                            cx.notify();
                        })
                        .unwrap();
                    Timer::after(Duration::from_millis(150)).await;
                    publish([105, 185, 75, 255]);
                    Timer::after(Duration::from_millis(150)).await;
                    camera
                        .update(cx, |camera, _, _| {
                            assert_eq!(camera.generation, 2, "a paused take must not appear live");
                            assert_eq!(camera.badge(), Some("Camera paused"));
                        })
                        .unwrap();
                    owner
                        .update(cx, |studio, cx| {
                            studio.recording_busy = true;
                            cx.notify();
                        })
                        .unwrap();
                    Timer::after(Duration::from_millis(150)).await;
                    pill.update(cx, |pill, _, cx| {
                        assert!(!pill.snapshot.can_control());
                        pill.action(RecordingAction::Stop, cx);
                    })
                    .unwrap();
                    owner
                        .update(cx, |studio, cx| {
                            assert_eq!(studio.recording_state, RecordingState::Paused);
                            assert!(studio.recording_controller.is_some());
                            assert_eq!(
                                studio.camera_overlay, original_overlay,
                                "monitor placement never edits the saved composition"
                            );
                            studio.recording_busy = false;
                            studio.recording_state = RecordingState::Recording;
                            studio.recording_window_minimized = true;
                            cx.notify();
                        })
                        .unwrap();
                    Timer::after(Duration::from_millis(150)).await;

                    // Simulate the helper's exact-title acknowledgement, then
                    // its loss. Only these test-created IDs are touched.
                    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                        .filter(|value| !value.is_empty())
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| {
                            format!("/run/user/{}", unsafe { libc::getuid() }).into()
                        });
                    let ready_files: Vec<_> =
                        exclusions
                            .iter()
                            .map(|exclusion| {
                                let id = exclusion.title().rsplit(' ').next().unwrap();
                                let path = runtime
                                    .join("lahza-prompter")
                                    .join(format!("{id}.ready.json"));
                                std::fs::write(&path, serde_json::to_vec(&serde_json::json!({
                        "id": id, "state": "protected",
                        "heartbeatUs": crate::recording::input::monotonic_ns() / 1_000,
                    })).unwrap()).unwrap();
                                assert!(exclusion.is_protected());
                                path
                            })
                            .collect();
                    for path in ready_files {
                        std::fs::remove_file(path).unwrap();
                    }
                    Timer::after(Duration::from_millis(400)).await;
                    owner
                        .update(cx, |studio, _| {
                            assert!(studio.recording_pill.is_none());
                            assert!(studio.recording_camera_window.is_none());
                            assert!(!studio.recording_window_minimized);
                            assert!(studio.toast.is_some());
                            assert_eq!(studio.recording_state, RecordingState::Recording);
                        })
                        .unwrap();
                    assert!(pill.update(cx, |_, _, _| ()).is_err());
                    assert!(camera.update(cx, |_, _, _| ()).is_err());
                    CaptureExclusion::wait_until_all_protected(&exclusions, Duration::ZERO)
                        .unwrap();
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
            });
    }
}
