//! Area selection on a preview of the monitor authorized by the portal.

use crate::{
    cached_render_image,
    recording::area::{AreaPreset, AreaRequest, RecordingArea},
    theme, Studio,
};
use gpui::{
    canvas, div, img, prelude::*, px, size, App, Bounds, Context, FocusHandle, KeyDownEvent,
    MouseButton, ObjectFit, Pixels, Point, Render, RenderImage, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};
use std::sync::{mpsc, Arc, Mutex};

impl Studio {
    pub(super) fn area_selection_requests(
        &self,
        cx: &mut Context<Self>,
    ) -> mpsc::Sender<AreaRequest> {
        let (sender, mut receiver) = mpsc::channel::<AreaRequest>();
        cx.spawn(async move |weak, cx| loop {
            let (returned, request) = cx
                .background_executor()
                .spawn(async move {
                    let request = receiver.recv();
                    (receiver, request)
                })
                .await;
            receiver = returned;
            let Ok(request) = request else { break };
            if weak
                .update(cx, |_, cx| {
                    if let Err(error) = open_picker(request, cx) {
                        eprintln!("Could not open recording area picker: {error}");
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        sender
    }
}

fn open_picker(request: AreaRequest, cx: &mut App) -> gpui::Result<gpui::WindowHandle<AreaPicker>> {
    let bounds = Bounds::centered(None, size(px(1000.0), px(720.0)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(640.0), px(480.0))),
            titlebar: Some(TitlebarOptions {
                title: Some("Lahza — Record area".into()),
                ..Default::default()
            }),
            app_id: Some("com.lahza.Lahza".into()),
            ..Default::default()
        },
        move |window, cx| {
            let picker = cx.new(|cx| AreaPicker::new(request, cx));
            window.focus(&picker.read(cx).focus);
            let weak = picker.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                let _ = weak.update(cx, |picker, _| picker.finish(None, window));
                true
            });
            picker
        },
    )?;
    cx.activate(true);
    Ok(handle)
}

#[derive(Clone, Copy)]
enum AreaDrag {
    Draw {
        start: (f32, f32),
    },
    Move {
        start: (f32, f32),
        area: RecordingArea,
    },
}

const RATIOS: &[(&str, AreaPreset)] = &[
    ("Free", AreaPreset::Free),
    ("16:9", AreaPreset::Ratio(16, 9)),
    ("9:16", AreaPreset::Ratio(9, 16)),
    ("1:1", AreaPreset::Ratio(1, 1)),
    ("4:3", AreaPreset::Ratio(4, 3)),
];
const SIZES: &[(&str, AreaPreset)] = &[
    ("720p", AreaPreset::Size(1280, 720)),
    ("1080p", AreaPreset::Size(1920, 1080)),
    ("1440p", AreaPreset::Size(2560, 1440)),
    ("4K", AreaPreset::Size(3840, 2160)),
];

struct AreaPicker {
    image: Option<Arc<RenderImage>>,
    source_size: (u32, u32),
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    selection: Option<RecordingArea>,
    preset: AreaPreset,
    dragging: Option<AreaDrag>,
    reply: Option<mpsc::Sender<Option<RecordingArea>>>,
    focus: FocusHandle,
}

impl AreaPicker {
    fn new(request: AreaRequest, cx: &mut Context<Self>) -> Self {
        let frame = request.frame;
        let source_size = (frame.width, frame.height);
        let pixels = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
            .expect("area preview has complete RGBA pixels");
        // A single bounded texture is enough to choose coordinates; retain
        // the original stream dimensions for the crop rather than UI pixels.
        let preview = image::DynamicImage::ImageRgba8(pixels)
            .thumbnail(1600, 1200)
            .to_rgba8();
        Self {
            image: Some(cached_render_image(preview)),
            source_size,
            bounds: Arc::new(Mutex::new(None)),
            selection: None,
            preset: AreaPreset::Free,
            dragging: None,
            reply: Some(request.reply),
            focus: cx.focus_handle(),
        }
    }

    fn normalized(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = (*self.bounds.lock().ok()?)?;
        Some((
            ((position.x - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0),
            ((position.y - bounds.origin.y) / bounds.size.height).clamp(0.0, 1.0),
        ))
    }

    fn area(&self) -> Option<RecordingArea> {
        self.selection
    }

    fn select_preset(&mut self, preset: AreaPreset) {
        if !preset.fits(self.source_size) {
            return;
        }
        self.selection = preset.select(self.selection, self.source_size);
        self.preset = preset;
        self.dragging = None;
    }

    fn begin_drag(&mut self, point: (f32, f32)) {
        if let Some(area) = self.selection.filter(|area| area.contains(point)) {
            self.dragging = Some(AreaDrag::Move { start: point, area });
        } else if matches!(self.preset, AreaPreset::Size(..)) {
            if let Some(area) = self.selection {
                let area = area.at(
                    point.0 * self.source_size.0 as f32 - area.width as f32 / 2.0,
                    point.1 * self.source_size.1 as f32 - area.height as f32 / 2.0,
                );
                self.selection = Some(area);
                self.dragging = Some(AreaDrag::Move { start: point, area });
            }
        } else {
            self.selection = None;
            self.dragging = Some(AreaDrag::Draw { start: point });
        }
    }

    fn update_drag(&mut self, end: (f32, f32)) {
        self.selection = match self.dragging {
            Some(AreaDrag::Draw { start }) => self.preset.draw(start, end, self.source_size),
            Some(AreaDrag::Move { start, area }) => Some(area.at(
                area.left as f32 + (end.0 - start.0) * self.source_size.0 as f32,
                area.top as f32 + (end.1 - start.1) * self.source_size.1 as f32,
            )),
            None => self.selection,
        };
    }

    fn preset_row(
        &self,
        id: &'static str,
        label: &'static str,
        options: &'static [(&'static str, AreaPreset)],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(80.0))
                    .text_sm()
                    .text_color(theme::muted())
                    .child(label),
            )
            .children(options.iter().enumerate().map(|(index, &(label, preset))| {
                let enabled = preset.fits(self.source_size);
                let active = self.preset == preset;
                div()
                    .id((id, index))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .border_1()
                    .border_color(if active { theme::blue() } else { theme::line() })
                    .bg(if active {
                        gpui::hsla(0.58, 0.8, 0.95, 1.0)
                    } else {
                        gpui::white()
                    })
                    .text_color(if active {
                        theme::blue()
                    } else if enabled {
                        theme::ink()
                    } else {
                        theme::muted()
                    })
                    .when(!enabled, |this| this.opacity(0.45))
                    .when(enabled, |this| this.cursor_pointer())
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_preset(preset);
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn finish(&mut self, area: Option<RecordingArea>, window: &mut Window) {
        if let Some(image) = self.image.take() {
            let _ = window.drop_image(image);
        }
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(area);
        }
    }
}

impl Render for AreaPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let aspect = self.source_size.0 as f32 / self.source_size.1 as f32;
        let width = (viewport.width / px(1.0) - 48.0)
            .max(1.0)
            .min((viewport.height / px(1.0) - 310.0).max(1.0) * aspect);
        let height = width / aspect;
        let bounds_store = self.bounds.clone();
        let area = self.area();
        let label = area
            .map(|a| format!("{} × {} px", a.width, a.height))
            .unwrap_or_else(|| "Drag to select the recording area".into());
        div()
            .size_full()
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .bg(theme::panel())
            .text_color(theme::ink())
            .font_family("Inter")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    this.finish(None, window);
                    window.remove_window();
                } else if event.keystroke.key == "enter" {
                    if let Some(area) = this.area() {
                        this.finish(Some(area), window);
                        window.remove_window();
                    }
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.dragging.is_some() {
                    if let Some(end) = this.normalized(event.position) {
                        this.update_drag(end);
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    if this.dragging.is_some() {
                        if let Some(end) = this.normalized(event.position) {
                            this.update_drag(end);
                        }
                        this.dragging = None;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    if this.dragging.is_some() {
                        if let Some(end) = this.normalized(event.position) {
                            this.update_drag(end);
                        }
                        this.dragging = None;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Select recording area"),
            )
            .child(div().text_sm().text_color(theme::muted()).child(
                if matches!(self.preset, AreaPreset::Size(..)) {
                    "Drag to position the selected size on your screen."
                } else {
                    "Drag inside to move. Shift-drag to draw a new area."
                },
            ))
            .child(self.preset_row("area-ratio", "Shape", RATIOS, cx))
            .child(self.preset_row("area-size", "Size", SIZES, cx))
            .child(
                div()
                    .w_full()
                    .text_xs()
                    .text_color(theme::muted())
                    .flex_none()
                    .child(format!(
                        "Screen: {} × {} px. Sizes larger than this screen are unavailable.",
                        self.source_size.0, self.source_size.1
                    )),
            )
            .child(
                div()
                    .id("recording-area-preview")
                    .relative()
                    .w(px(width))
                    .h(px(height))
                    .flex_none()
                    .cursor(gpui::CursorStyle::Crosshair)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            if let Some(start) = this.normalized(event.position) {
                                if event.modifiers.shift
                                    && !matches!(this.preset, AreaPreset::Size(..))
                                {
                                    this.selection = None;
                                }
                                this.begin_drag(start);
                                cx.notify();
                            }
                        }),
                    )
                    .when_some(self.image.clone(), |this, image| {
                        this.child(img(image).size_full().object_fit(ObjectFit::Fill))
                    })
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                *bounds_store.lock().expect("area bounds poisoned") = Some(bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .when_some(area, |this, area| {
                        let x = area.left as f32 / area.source_width as f32 * width;
                        let y = area.top as f32 / area.source_height as f32 * height;
                        let w = area.width as f32 / area.source_width as f32 * width;
                        let h = area.height as f32 / area.source_height as f32 * height;
                        this.child(
                            div()
                                .absolute()
                                .left(px(x))
                                .top(px(y))
                                .w(px(w))
                                .h(px(h))
                                .border_2()
                                .border_color(theme::blue())
                                .bg(gpui::hsla(0.58, 0.8, 0.5, 0.15)),
                        )
                    }),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().text_sm().child(label))
                    .child(
                        div()
                            .id("cancel-recording-area")
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .bg(gpui::white())
                            .cursor_pointer()
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, window, _| {
                                this.finish(None, window);
                                window.remove_window();
                            })),
                    )
                    .child(
                        div()
                            .id("confirm-recording-area")
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .bg(if area.is_some() {
                                theme::blue()
                            } else {
                                theme::muted()
                            })
                            .text_color(gpui::white())
                            .when(area.is_some(), |this| this.cursor_pointer())
                            .child("Start recording")
                            .on_click(cx.listener(|this, _, window, _| {
                                if let Some(area) = this.area() {
                                    this.finish(Some(area), window);
                                    window.remove_window();
                                }
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Application, Timer};
    use std::time::Duration;

    #[test]
    #[ignore = "interactive picker check: requires a display; free drag, 720p move, portrait drag, then cancel"]
    fn area_picker_drag_confirm_and_cancel() {
        Application::new().run(|cx| {
            cx.spawn(async move |cx| {
                for mode in ["free", "720p", "portrait", "cancel"] {
                    let (reply, response) = mpsc::channel();
                    let handle = cx
                        .update(|cx| {
                            open_picker(
                                AreaRequest {
                                    frame: crate::recording::camera_preview::CameraFrame {
                                        width: 1920,
                                        height: 1080,
                                        rgba: vec![255; 1920 * 1080 * 4],
                                    },
                                    reply,
                                },
                                cx,
                            )
                            .unwrap()
                        })
                        .unwrap();
                    Timer::after(Duration::from_millis(500)).await;
                    handle
                        .update(cx, |picker, window, _| {
                            let b = picker.bounds.lock().unwrap().unwrap();
                            assert!(
                                b.origin.y + b.size.height + px(78.0)
                                    <= window.viewport_size().height,
                                "picker footer must fit inside the window"
                            );
                            println!(
                                "AREA_PICKER_READY {} {} {} {} {}",
                                mode,
                                b.origin.x / px(1.0),
                                b.origin.y / px(1.0),
                                b.size.width / px(1.0),
                                b.size.height / px(1.0)
                            );
                        })
                        .unwrap();
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            response
                                .recv_timeout(Duration::from_secs(30))
                                .expect("picker must unblock recorder")
                        })
                        .await;
                    if mode == "cancel" {
                        assert!(result.is_none());
                    } else {
                        let area =
                            result.expect("drag must select an area through the rendered hitbox");
                        // Pointer injection is in whole display pixels.
                        match mode {
                            "720p" => {
                                assert_eq!((area.width, area.height), (1280, 720));
                                assert!((area.left as i32 - 512).abs() <= 4);
                                assert!((area.top as i32 - 288).abs() <= 4);
                            }
                            "portrait" => assert_eq!(area.width * 16, area.height * 9),
                            _ => {
                                assert!((area.left as i32 - 480).abs() <= 4);
                                assert!((area.top as i32 - 270).abs() <= 4);
                                assert!((area.width as i32 - 960).abs() <= 4);
                                assert!((area.height as i32 - 540).abs() <= 4);
                            }
                        }
                    }
                }
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
    }
}
