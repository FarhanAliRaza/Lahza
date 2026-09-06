//! Capture launcher layout, source selectors, and recorder window lifecycle.

use super::{
    blue, brand_wordmark, ink, library, line, muted,
    open_studio_window, panel, recording, RecordingOptions, RecordingState, Studio,
};
use crate::capture_access::CaptureAccess;
use gpui::{
    div, hsla, img, point, prelude::*, px, rgb, svg, AnyElement, App, ClickEvent, Context,
    FontWeight, IntoElement, ObjectFit, Window,
};

impl Studio {
    pub(super) fn launcher_source_row(
        &self,
        id: &'static str,
        icon: &'static str,
        title: &'static str,
        subtitle: impl IntoElement,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .h(px(52.0))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(if enabled { blue() } else { line() })
            .bg(if enabled {
                hsla(211.0 / 360.0, 0.9, 0.96, 1.0)
            } else {
                gpui::white()
            })
            .cursor_pointer()
            .on_click(on_click)
            .child(svg().path(icon).size(px(17.0)).text_color(if enabled {
                blue()
            } else {
                muted()
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(div().text_xs().text_color(muted()).child(subtitle)),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_full()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(if enabled { blue() } else { line() })
                    .text_color(if enabled { gpui::white() } else { ink() })
                    .child(if enabled { "On" } else { "Off" }),
            )
            .into_any_element()
    }

    /// A web-style device select for a launcher row: shows the chosen entry,
    /// opens a popup on click, and closes on a pick or a click outside.
    pub(super) fn launcher_device_select(
        &self,
        id: &'static str,
        devices: &[(String, String)],
        selected: &Option<String>,
        open: bool,
        set_open: impl Fn(&mut Self, bool, &mut Context<Self>) + 'static,
        pick: impl Fn(&mut Self, Option<String>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = selected
            .as_ref()
            .and_then(|selected| {
                devices
                    .iter()
                    .find(|(name, _)| name == selected)
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| "System default".to_string());
        let entries = std::iter::once((None, "System default".to_string()))
            .chain(
                devices
                    .iter()
                    .map(|(name, label)| (Some(name.clone()), label.clone())),
            )
            .collect::<Vec<_>>();
        let pick = std::rc::Rc::new(pick);
        let set_open = std::rc::Rc::new(set_open);
        let toggle = set_open.clone();
        div()
            .id(id)
            .relative()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .text_color(if open { blue() } else { muted() })
            .hover(|s| s.text_color(ink()))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                toggle(this, !open, cx);
                cx.notify();
            }))
            .child(
                div()
                    .max_w(px(220.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label),
            )
            .child(svg().path("icons/chevron-down.svg").size(px(12.0)))
            .when(open, |this| {
                let close = set_open.clone();
                this.child(
                    gpui::deferred(
                        gpui::anchored()
                            .position_mode(gpui::AnchoredPositionMode::Local)
                            .position(point(px(0.0), px(22.0)))
                            .snap_to_window_with_margin(gpui::Edges {
                                top: px(8.0),
                                right: px(8.0),
                                bottom: px(8.0),
                                left: px(8.0),
                            })
                            .child(
                                div()
                                    .id((id, 1usize))
                                    .w(px(280.0))
                                    .max_h(px(240.0))
                                    .overflow_y_scroll()
                                    .py_1()
                                    .rounded_lg()
                                    .bg(gpui::white())
                                    .border_1()
                                    .border_color(line())
                                    .shadow_md()
                                    .flex()
                                    .flex_col()
                                    .text_color(ink())
                                    .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                                        close(this, false, cx);
                                        cx.notify();
                                    }))
                                    .children(entries.into_iter().enumerate().map(
                                        |(index, (name, entry))| {
                                            let is_selected = *selected == name;
                                            let pick = pick.clone();
                                            div()
                                                .id((id, index + 2))
                                                .h(px(30.0))
                                                .flex_none()
                                                .px_2()
                                                .mx_1()
                                                .rounded_md()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .cursor_pointer()
                                                .hover(|s| {
                                                    s.bg(hsla(220.0 / 360.0, 0.08, 0.95, 1.0))
                                                })
                                                .child(
                                                    svg()
                                                        .path("icons/check.svg")
                                                        .size(px(13.0))
                                                        .text_color(if is_selected {
                                                            blue()
                                                        } else {
                                                            gpui::transparent_black()
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .font_weight(if is_selected {
                                                            FontWeight::SEMIBOLD
                                                        } else {
                                                            FontWeight::NORMAL
                                                        })
                                                        .child(entry),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    pick(this, name.clone(), cx);
                                                    cx.notify();
                                                }))
                                        },
                                    )),
                            ),
                    )
                    .with_priority(1),
                )
            })
            .into_any_element()
    }

    /// A separate studio owns the capture so opening or finishing it cannot
    /// replace the project currently being edited in this window.
    pub(super) fn open_recorder_window(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.recorder_window {
            let activated = handle.update(cx, |studio, window, _| {
                if !studio.launcher_active {
                    return false;
                }
                window.activate_window();
                true
            });
            if activated.unwrap_or(false) {
                cx.activate(true);
                return;
            }
        }
        self.pause_video_playback();
        self.finish_annotation_interaction();
        let options = RecordingOptions {
            system_audio: self.record_system_audio,
            microphone: self.record_microphone,
            microphone_device: self.microphone_device.clone(),
            camera: self.record_camera,
            camera_device: self.camera_device.clone(),
        };
        match open_studio_window(cx, false, move |window_handle, cx| {
            cx.new(|cx| {
                let mut studio = Studio::new(window_handle, None, None, cx);
                studio.record_system_audio = options.system_audio;
                studio.record_microphone = options.microphone;
                studio.microphone_device = options.microphone_device;
                studio.record_camera = options.camera;
                studio.camera_device = options.camera_device;
                studio
            })
        }) {
            Ok(handle) => {
                self.recorder_window = Some(handle);
                cx.activate(true);
            }
            Err(error) => self.toast = Some(format!("Could not open recorder: {error}").into()),
        }
        cx.notify();
    }

    pub(super) fn render_launcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let items = if self.launcher_tab == 1 {
            &self.recent_projects
        } else {
            &self.recent_screenshots
        };
        let recording = self.recording_state != RecordingState::Idle;
        div()
            .size_full()
            .bg(panel())
            .text_color(ink())
            .font_family("Inter")
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(54.0))
                    .flex_none()
                    .relative()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_b_1()
                    .border_color(line())
                    .child(brand_wordmark(100.0, 32.0)),
            )
            .when(recording, |this| {
                this.child(self.launcher_recording_panel(cx))
            })
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .gap_2()
                    .children(
                        [("Capture", 0usize), ("Projects", 1), ("Screenshots", 2)]
                            .into_iter()
                            .map(|(label, tab)| {
                                let active = self.launcher_tab == tab;
                                div()
                                    .id(("launcher-tab", tab))
                                    .flex_1()
                                    .h(px(34.0))
                                    .rounded_lg()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .bg(if active { ink() } else { gpui::white() })
                                    .text_color(if active { gpui::white() } else { ink() })
                                    .border_1()
                                    .border_color(if active { ink() } else { line() })
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.launcher_tab = tab;
                                        this.refresh_library(cx);
                                        cx.notify();
                                    }))
                                    .child(label)
                            }),
                    ),
            )
            .child(if self.launcher_tab == 0 {
                div()
                    .id("launcher-capture")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(!recording, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .grid()
                                .grid_cols(2)
                                .gap_2()
                                .child(
                                    div()
                                        .id("launcher-screenshot")
                                        .h(px(82.0))
                                        .rounded_xl()
                                        .bg(gpui::white())
                                        .border_1()
                                        .border_color(line())
                                        .hover(|s| s.bg(rgb(0xe5f2ff)))
                                        .cursor_pointer()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(
                                            svg()
                                                .path("icons/capture.svg")
                                                .size(px(24.0))
                                                .text_color(blue()),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::BOLD)
                                                .child("Screenshot"),
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.begin_screen_capture(cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id("launcher-record")
                                        .h(px(82.0))
                                        .rounded_xl()
                                        .bg(gpui::white())
                                        .border_1()
                                        .border_color(line())
                                        .hover(|s| s.bg(rgb(0xffe9eb)))
                                        .cursor_pointer()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(
                                            svg()
                                                .path("icons/record.svg")
                                                .size(px(24.0))
                                                .text_color(rgb(0xe33442)),
                                        )
                                        .child(div().text_sm().font_weight(FontWeight::BOLD).child(
                                            if recording {
                                                "Recording…"
                                            } else {
                                                "Record screen"
                                            },
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if this.recording_state == RecordingState::Idle {
                                                this.start_recording(cx)
                                            }
                                        })),
                                ),
                        )
                    })
                    .child(self.launcher_source_row(
                        "launcher-camera",
                        "icons/video.svg",
                        "Camera",
                        self.launcher_device_select(
                            "launcher-camera-select",
                            &self.camera_devices,
                            &self.camera_device,
                            self.launcher_camera_menu_open,
                            |this, open, cx| {
                                if open {
                                    this.request_capture_access(CaptureAccess::CameraPicker, cx);
                                } else {
                                    this.launcher_camera_menu_open = false;
                                }
                            },
                            |this, device, cx| {
                                if this.camera_device != device {
                                    this.camera_device = device;
                                    // Restart the preview on the new device.
                                    this.camera_preview = None;
                                }
                                this.request_capture_access(CaptureAccess::Camera, cx);
                                this.launcher_camera_menu_open = false;
                            },
                            cx,
                        ),
                        self.record_camera,
                        cx.listener(|this, _, _, cx| {
                            if this.record_camera {
                                this.record_camera = false;
                            } else {
                                this.request_capture_access(CaptureAccess::Camera, cx);
                            }
                            cx.notify();
                        }),
                    ))
                    .when_some(
                        self.camera_frame.clone().filter(|_| !recording),
                        |this, frame| {
                            this.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        img(frame).size(px(200.0)).object_fit(ObjectFit::Contain),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted())
                                            .child("Camera crop · enlarged preview"),
                                    ),
                            )
                        },
                    )
                    .child(self.launcher_source_row(
                        "launcher-mic",
                        "icons/microphone.svg",
                        "Microphone",
                        self.launcher_device_select(
                            "launcher-mic-select",
                            &self.microphone_devices,
                            &self.microphone_device,
                            self.launcher_mic_menu_open,
                            |this, open, cx| {
                                if open {
                                    this.request_capture_access(CaptureAccess::MicrophonePicker, cx);
                                } else {
                                    this.launcher_mic_menu_open = false;
                                }
                            },
                            |this, device, cx| {
                                this.microphone_device = device;
                                this.request_capture_access(CaptureAccess::Microphone, cx);
                                this.launcher_mic_menu_open = false;
                            },
                            cx,
                        ),
                        self.record_microphone,
                        cx.listener(|this, _, _, cx| {
                            if this.record_microphone {
                                this.record_microphone = false;
                            } else {
                                this.request_capture_access(CaptureAccess::Microphone, cx);
                            }
                            cx.notify();
                        }),
                    ))
                    .child(self.launcher_source_row(
                        "launcher-system-audio",
                        "icons/volume.svg",
                        "System audio",
                        "Sound playing on this computer",
                        self.record_system_audio,
                        cx.listener(|this, _, _, cx| {
                            if this.record_system_audio {
                                this.record_system_audio = false;
                            } else {
                                this.request_capture_access(CaptureAccess::SystemAudio, cx);
                            }
                            cx.notify();
                        }),
                    ))
                    .into_any_element()
            } else {
                div()
                    .id("launcher-recent")
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div().text_xs().text_color(muted()).child(
                            if self.launcher_tab == 1 {
                                recording::model::recordings_root()
                            } else {
                                library::screenshots_root()
                            }
                            .display()
                            .to_string(),
                        ),
                    )
                    .when(items.is_empty(), |this| {
                        this.child(
                            div()
                                .h(px(180.0))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .text_color(muted())
                                .child(
                                    svg()
                                        .path("icons/folder.svg")
                                        .size(px(28.0))
                                        .text_color(muted()),
                                )
                                .child(if self.library_state.loading {
                                    "Loading…"
                                } else {
                                    "No saved items yet"
                                }),
                        )
                    })
                    .when(!items.is_empty(), |this| {
                        this.child(
                            gpui::uniform_list(
                                if self.launcher_tab == 1 {
                                    "project-library"
                                } else {
                                    "screenshot-library"
                                },
                                items.len(),
                                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                    range.map(|index| this.library_row(index, cx)).collect()
                                }),
                            )
                            .flex_1()
                            .min_h_0(),
                        )
                    })
                    .into_any_element()
            })
            .into_any_element()
    }
}
