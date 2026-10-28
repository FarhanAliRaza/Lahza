//! Capture launcher layout, source selectors, and recorder window lifecycle.

use super::{
    accent, accent_tint, brand, brand_wordmark, control_bg, control_hover, ink, library, line, muted, on_accent, open_studio_window, panel, recording,
    surface, TextRoles,
    RecordingOptions, RecordingState, Studio,
};
use crate::capture_access::CaptureAccess;
use gpui::{
    div, img, point, prelude::*, px, svg, AnyElement, App, ClickEvent, Context,
    FontWeight, IntoElement, ObjectFit, Window,
};

impl Studio {
    /// Device choice and capture state are separate controls, as in the editor.
    pub(super) fn launcher_source_row(
        &self,
        id: &'static str,
        icon: &'static str,
        title: &'static str,
        content: Option<AnyElement>,
        enabled: bool,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let editable = self.recording_state == RecordingState::Idle && !self.capture_access_busy;
        div().id(id).flex_none().p_3().flex().flex_col().gap_2()
            .rounded_lg().border_1().border_color(line()).bg(surface())
            .child(div().h(px(24.0)).flex().items_center().gap_2()
                .child(svg().path(icon).size(px(17.0)).flex_none()
                    .text_color(if enabled { ink() } else { muted() }))
                .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().id((id, 0usize)).h(px(28.0)).flex().items_center().gap_2()
                    .when(editable, |button| button.cursor_pointer().on_click(on_toggle))
                    .when(!editable, |button| button.opacity(0.5).cursor(gpui::CursorStyle::OperationNotAllowed))
                    .child(self.toggle(enabled))))
            .children(content)
            .into_any_element()
    }

    /// An outlined device field with a full-width, independently dismissible menu.
    pub(super) fn launcher_device_select(
        &self,
        id: &'static str,
        title: &'static str,
        icon: &'static str,
        window: &Window,
        devices: &[(String, String)],
        selected: &Option<String>,
        open: bool,
        set_open: impl Fn(&mut Self, bool, &mut Context<Self>) + 'static,
        pick: impl Fn(&mut Self, Option<String>, &mut Context<Self>) + 'static,
        refresh: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editable = self.recording_state == RecordingState::Idle;
        let width = (window.viewport_size().width - px(58.0)).max(px(200.0));
        let loading = self.capture_access_busy;
        let label = match selected {
            Some(selected) => devices.iter().find(|(name, _)| name == selected)
                .map(|(_, label)| label.clone()).unwrap_or_else(|| "Device unavailable · choose another".into()),
            None => "System default".into(),
        };
        let empty = devices.is_empty();
        let entries = std::iter::once((None, "System default".to_string()))
            .chain(devices.iter().map(|(name, label)| (Some(name.clone()), label.clone())))
            .collect::<Vec<_>>();
        let pick = std::rc::Rc::new(pick);
        let set_open = std::rc::Rc::new(set_open);
        let toggle = set_open.clone();
        let trigger_bounds = std::rc::Rc::new(std::cell::Cell::new(None::<gpui::Bounds<gpui::Pixels>>));
        let measure = trigger_bounds.clone();
        let clicked_bounds = trigger_bounds.clone();
        let anchor = self.launcher_device_anchor.unwrap_or(gpui::Bounds {
            origin: point(px(29.0), px(0.0)), size: gpui::size(width, px(38.0)),
        });
        let below = (window.viewport_size().height - anchor.bottom() - px(14.0)).max(px(0.0));
        let above = (anchor.top() - px(14.0)).max(px(0.0));
        let desired_height = px(76.0) + px(entries.len() as f32 * 42.0 + if empty { 36.0 } else { 0.0 }).min(px(220.0));
        let opens_below = below >= desired_height || below >= above;
        let list_height = ((if opens_below { below } else { above }) - px(76.0)).clamp(px(38.0), px(220.0));
        let menu_position = if opens_below {
            point(anchor.left(), anchor.bottom() + px(6.0))
        } else { point(anchor.left(), anchor.top() - px(6.0)) };
        div().id(id).relative().w_full().h(px(38.0)).flex_none()
            .flex().items_center().gap_2().pl_3().pr_2()
            .rounded_md().border_1().text_sm()
            .border_color(if open { accent() } else { line() })
            .bg(control_bg())
            .text_color(ink())
            .when(editable, |field| field.cursor_pointer()
                .hover(|field| field.border_color(muted()).bg(control_hover()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.focus_handle.focus(window);
                    this.launcher_device_anchor = clicked_bounds.get();
                    toggle(this, !open, cx);
                    cx.notify();
                })))
            .when(!editable, |field| field.opacity(0.5).cursor(gpui::CursorStyle::OperationNotAllowed))
            .tooltip(crate::timeline_ui::tooltip(format!("Choose {title}: {label}")))
            .child(gpui::canvas(move |bounds, _, _| measure.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0())
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(div().w(px(24.0)).h(px(24.0)).flex_none().flex().items_center().justify_center()
                .child(svg().path("icons/chevron-down.svg").size(px(16.0)).text_color(if open { ink() } else { muted() })))
            .when(open && editable, |field| {
                let close = set_open.clone();
                field.child(gpui::deferred(
                    gpui::anchored()
                        .position_mode(gpui::AnchoredPositionMode::Window)
                        .anchor(if opens_below { gpui::Corner::TopLeft } else { gpui::Corner::BottomLeft })
                        .position(menu_position)
                        .snap_to_window_with_margin(gpui::Edges { top: px(8.0), right: px(8.0), bottom: px(8.0), left: px(8.0) })
                        .child(div().id((id, 1usize)).w(width).rounded_lg()
                            .float()
                            .flex().flex_col().text_color(ink()).text_sm()
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                // Let the trigger's click close it; closing on mouse-down
                                // would otherwise reopen it on the subsequent mouse-up.
                                if trigger_bounds.get().is_some_and(|bounds| bounds.contains(&event.position)) { return; }
                                close(this, false, cx);
                                cx.notify();
                            }))
                            .child(div().px_3().pt_3().pb_2().text_xs().font_weight(FontWeight::SEMIBOLD)
                                .text_color(muted()).child(format!("Choose {title}")))
                            .child(div().id((id, 10000usize)).max_h(list_height).overflow_y_scroll().p_1()
                                .flex().flex_col().gap_1()
                                .children(entries.into_iter().enumerate().map(|(index, (name, entry))| {
                                    let is_selected = *selected == name;
                                    let pick = pick.clone();
                                    div().id((id, index + 2)).min_h(px(38.0)).flex_none().px_2().py_2()
                                        .rounded(px(crate::theme::inner_radius(crate::theme::RADIUS_LG, 4.0))).flex().items_center().gap_2()
                                        .when(!loading, |row| row.cursor_pointer())
                                        .when(loading, |row| row.opacity(0.5).cursor(gpui::CursorStyle::OperationNotAllowed))
                                        .when(is_selected, |row| row.bg(accent_tint()))
                                        .hover(|row| row.bg(control_bg()))
                                        .child(svg().path(icon).size(px(16.0)).flex_none()
                                            .text_color(if is_selected { ink() } else { muted() }))
                                        .child(div().flex_1().min_w_0().text_size(px(13.0)).line_height(px(18.0))
                                            .font_weight(if is_selected { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                                            .child(entry))
                                        .child(svg().path("icons/check.svg").size(px(16.0)).flex_none()
                                            .text_color(if is_selected { ink() } else { gpui::transparent_black() }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if loading { return; }
                                            pick(this, name.clone(), cx);
                                            cx.notify();
                                        }))
                                }))
                                .when(empty, |list| list.child(div().px_3().py_2().text_xs().text_color(muted())
                                    .child(if loading { "Looking for connected devices…" } else { "No devices found. Connect one, then refresh." }))))
                            .child(div().border_t_1().border_color(line()).p_1()
                                .child(div().id((id, 10001usize)).h(px(34.0)).px_2().rounded(px(crate::theme::inner_radius(crate::theme::RADIUS_LG, 4.0))).flex().items_center()
                                    .text_xs().text_color(if loading { muted() } else { ink() })
                                    .when(!loading, |button| button.cursor_pointer().hover(|button| button.bg(control_bg()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation(); refresh(this, cx); cx.notify();
                                        })))
                                    .child(if loading { "Refreshing devices…" } else { "Refresh devices" }))))
                ).with_priority(1))
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
        let record_area = self.record_area;
        let camera_overlay = self.camera_overlay;
        let aspect_ratio = self.aspect_ratio;
        let custom_canvas_dimensions = self.custom_canvas_dimensions;
        let options = RecordingOptions {
            source: self.recording_source,
            countdown_seconds: self.recording_countdown,
            area: None,
            capture_exclusions: Vec::new(),
            system_audio: self.record_system_audio,
            microphone: self.record_microphone,
            microphone_device: self.microphone_device.clone(),
            camera: self.record_camera,
            camera_device: self.camera_device.clone(),
        };
        match open_studio_window(cx, false, move |window_handle, cx| {
            cx.new(|cx| {
                let mut studio = Studio::new(window_handle, None, None, cx);
                studio.record_area = record_area;
                studio.recording_source = options.source;
                studio.recording_countdown = options.countdown_seconds;
                studio.camera_overlay = camera_overlay;
                studio.camera_overlay.enabled = true;
                studio.aspect_ratio = aspect_ratio;
                studio.custom_canvas_dimensions = custom_canvas_dimensions;
                studio.custom_canvas_draft = custom_canvas_dimensions;
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
            Err(error) => self.toast = Some(crate::Notification::error(format!("Could not open recorder: {error}"))),
        }
        cx.notify();
    }

    pub(super) fn render_launcher(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let menu_open = this.launcher_camera_menu_open || this.launcher_mic_menu_open;
                if event.keystroke.key == "escape" && menu_open {
                    this.launcher_camera_menu_open = false;
                    this.launcher_mic_menu_open = false;
                    cx.stop_propagation(); cx.notify();
                    return;
                }
                if this.native_text_focused(window, cx) { return; }
                if crate::controls::tab_navigation(event, window) {
                    cx.stop_propagation();
                    return;
                }
                // Enter records and S takes a screenshot from the Capture tab.
                if menu_open
                    || this.capture_access_prompt.is_some()
                    || event.keystroke.modifiers.modified()
                    || this.launcher_tab != 0
                    || this.recording_state != RecordingState::Idle
                {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "enter" => this.start_recording(cx),
                    "s" => this.begin_screen_capture(cx),
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
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
                                    .bg(if active { accent() } else { surface() })
                                    .text_color(if active { on_accent() } else { ink() })
                                    .border_1()
                                    .border_color(if active { accent() } else { line() })
                                    .cursor_pointer()
                                    .when(!active, |this| this.hover(|style| style.bg(control_bg())))
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
                                    crate::controls::keyboard_focusable(div().id("launcher-screenshot"))
                                        .h(px(82.0))
                                        .rounded_xl()
                                        .bg(surface())
                                        .border_1()
                                        .border_color(line())
                                        .hover(|s| s.bg(control_bg()))
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
                                                .text_color(ink()),
                                        )
                                        .child(launcher_tile_label("Screenshot", "S"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.begin_screen_capture(cx)
                                        })),
                                )
                                .child(
                                    crate::controls::keyboard_focusable(div().id("launcher-record"))
                                        .h(px(82.0))
                                        .rounded_xl()
                                        .bg(surface())
                                        .border_1()
                                        .border_color(line())
                                        .hover(|s| s.bg(brand().opacity(0.08)))
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
                                                .text_color(brand()),
                                        )
                                        .child(launcher_tile_label(
                                            if recording {
                                                "Recording…"
                                            } else if self.record_area {
                                                "Record area"
                                            } else if self.recording_source == recording::native::RecordingSource::Window {
                                                "Record window"
                                            } else {
                                                "Record screen"
                                            },
                                            "↵",
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if this.recording_state == RecordingState::Idle {
                                                this.start_recording(cx)
                                            }
                                        })),
                                ),
                        )
                    })
                    .when(!recording, |panel| panel
                        .child(self.segmented("recording-source-mode", &["Screen", "Window", "Area"],
                            if self.record_area { 2 } else if self.recording_source == recording::native::RecordingSource::Window { 1 } else { 0 },
                            |this, i| { this.record_area = i == 2; this.recording_source = if i == 1 { recording::native::RecordingSource::Window } else { recording::native::RecordingSource::Screen }; }, cx))
                        .child(div().text_xs().text_color(muted()).child("Countdown"))
                        .child(self.segmented("recording-countdown", &["Off", "3 s", "5 s", "10 s"], [0, 3, 5, 10].iter().position(|v| *v == self.recording_countdown).unwrap_or(1),
                            |this, i| this.recording_countdown = [0, 3, 5, 10][i], cx)))
                    .child(self.launcher_source_row(
                        "launcher-camera",
                        "icons/video.svg",
                        "Camera",
                        Some(self.launcher_device_select(
                            "launcher-camera-select", "camera", "icons/video.svg", window,
                            &self.camera_devices,
                            &self.camera_device,
                            self.launcher_camera_menu_open,
                            |this, open, cx| {
                                if this.recording_state != RecordingState::Idle { return; }
                                this.launcher_camera_menu_open = open;
                                if open {
                                    this.launcher_mic_menu_open = false;
                                    if this.camera_devices.is_empty() { this.request_capture_access(CaptureAccess::CameraPicker, cx); }
                                }
                            },
                            |this, device, cx| {
                                if this.recording_state != RecordingState::Idle || this.capture_access_busy { return; }
                                if this.camera_device != device {
                                    this.camera_device = device;
                                    // Restart the preview on the new device.
                                    this.camera_preview = None;
                                }
                                this.launcher_camera_menu_open = false;
                                this.request_capture_access(CaptureAccess::Camera, cx);
                            },
                            |this, cx| this.request_capture_access(CaptureAccess::CameraPicker, cx),
                            cx,
                        ).into_any_element()),
                        self.record_camera,
                        cx.listener(|this, _, _, cx| {
                            if this.recording_state != RecordingState::Idle { return; }
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
                                            .child("Camera preview"),
                                    ),
                            )
                        },
                    )
                    .child(self.launcher_source_row(
                        "launcher-mic",
                        "icons/microphone.svg",
                        "Microphone",
                        Some(self.launcher_device_select(
                            "launcher-mic-select", "microphone", "icons/microphone.svg", window,
                            &self.microphone_devices,
                            &self.microphone_device,
                            self.launcher_mic_menu_open,
                            |this, open, cx| {
                                if this.recording_state != RecordingState::Idle { return; }
                                this.launcher_mic_menu_open = open;
                                if open {
                                    this.launcher_camera_menu_open = false;
                                    if this.microphone_devices.is_empty() { this.request_capture_access(CaptureAccess::MicrophonePicker, cx); }
                                }
                            },
                            |this, device, cx| {
                                if this.recording_state != RecordingState::Idle || this.capture_access_busy { return; }
                                this.microphone_device = device;
                                this.launcher_mic_menu_open = false;
                                this.request_capture_access(CaptureAccess::Microphone, cx);
                            },
                            |this, cx| this.request_capture_access(CaptureAccess::MicrophonePicker, cx),
                            cx,
                        ).into_any_element()),
                        self.record_microphone,
                        cx.listener(|this, _, _, cx| {
                            if this.recording_state != RecordingState::Idle { return; }
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
                        None,
                        self.record_system_audio,
                        cx.listener(|this, _, _, cx| {
                            if this.recording_state != RecordingState::Idle { return; }
                            if this.record_system_audio {
                                this.record_system_audio = false;
                            } else {
                                this.request_capture_access(CaptureAccess::SystemAudio, cx);
                            }
                            cx.notify();
                        }),
                    ))
                    .child(self.launcher_teleprompter(recording, cx))
                    .child(div().text_xs().text_color(muted()).child("Appearance"))
                    .child(self.segmented(
                        "launcher-appearance",
                        &["System", "Light", "Dark"],
                        crate::theme::theme_preference() as usize,
                        |_, index| crate::theme::store_theme_preference(crate::theme::ThemePreference::ALL[index]),
                        cx,
                    ))
                    .child(crate::theme::credit_footer())
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
                    .child({
                        let folder = if self.launcher_tab == 1 {
                            recording::model::recordings_root()
                        } else {
                            library::screenshots_root()
                        };
                        let location = folder.display().to_string();
                        div()
                            .id("library-open-folder")
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .py_1()
                            .text_xs()
                            .text_color(muted())
                            .cursor_pointer()
                            .hover(|style| style.text_color(ink()))
                            .tooltip(crate::timeline_ui::tooltip(location.clone()))
                            .child(svg().path("icons/folder.svg").size(px(14.0)).flex_none().text_color(muted()))
                            .child(div().flex_1().min_w_0().truncate().child(location))
                            .child(div().flex_none().text_color(ink()).underline().child("Open folder ↗"))
                            .on_click(move |_, _, cx| {
                                let _ = std::fs::create_dir_all(&folder);
                                cx.open_url(&format!("file://{}", folder.display()));
                            })
                    })
                    .when(items.is_empty(), |this| {
                        let projects = self.launcher_tab == 1;
                        this.child(
                            div()
                                .h(px(220.0))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_3()
                                .child(
                                    svg()
                                        .path(if projects { "icons/video.svg" } else { "icons/capture.svg" })
                                        .size(px(28.0))
                                        .text_color(muted()),
                                )
                                .child(div().section_title().child(if self.library_state.loading {
                                    "Loading…"
                                } else if projects {
                                    "No recordings yet"
                                } else {
                                    "No screenshots yet"
                                }))
                                .when(!self.library_state.loading, |this| {
                                    this.child(
                                        crate::controls::button(
                                            "library-empty-capture",
                                            if projects { "Record screen" } else { "Take screenshot" },
                                            crate::controls::ButtonKind::Primary,
                                            crate::controls::ButtonSize::Md,
                                        )
                                        .icon(if projects { "icons/record.svg" } else { "icons/capture.svg" })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.launcher_tab = 0;
                                            if projects {
                                                if this.recording_state == RecordingState::Idle {
                                                    this.start_recording(cx);
                                                }
                                            } else {
                                                this.begin_screen_capture(cx);
                                            }
                                            cx.notify();
                                        })),
                                    )
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

/// Tile title with the key that triggers it.
fn launcher_tile_label(label: &'static str, key: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(label))
        .child(
            div()
                .min_w(px(18.0))
                .h(px(18.0))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(crate::theme::RADIUS_SM))
                .border_1()
                .border_color(line())
                .micro()
                .text_color(muted())
                .child(key),
        )
}
