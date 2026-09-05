use crate::{ink, line, muted, RecordingAction, RecordingState, Studio};
use gpui::{div, prelude::*, px, rgb, svg, AnyElement, Context, FontWeight};

impl Studio {
    pub(crate) fn launcher_recording_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let paused = self.recording_state == RecordingState::Paused;
        let busy = self.recording_busy
            || matches!(
                self.recording_state,
                RecordingState::Starting | RecordingState::Finishing
            );
        let status = match self.recording_state {
            RecordingState::Starting => "Starting recording…",
            RecordingState::Finishing => "Saving recording…",
            RecordingState::Paused => "Recording paused",
            _ => "Recording in progress",
        };
        let action_button =
            |id: &'static str, label: &'static str, icon: &'static str, action, primary: bool| {
                div()
                    .id(id)
                    .h(px(48.0))
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .rounded_lg()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(if primary {
                        rgb(0xc92536)
                    } else {
                        rgb(0xffffff)
                    })
                    .text_color(if primary { gpui::white() } else { ink() })
                    .border_1()
                    .border_color(if primary {
                        rgb(0xc92536).into()
                    } else {
                        line()
                    })
                    .when(busy, |this| this.opacity(0.5))
                    .when(!busy, |this| {
                        this.cursor_pointer()
                            .hover(move |style| {
                                style.bg(if primary {
                                    rgb(0xab1e2d)
                                } else {
                                    rgb(0xf0f0f2)
                                })
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_recording_action(action, cx);
                                cx.notify();
                            }))
                    })
                    .child(svg().path(icon).size(px(17.0)))
                    .child(label)
            };
        div()
            .id("launcher-recording-panel")
            .flex_none()
            .px_4()
            .py_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(gpui::white())
            .border_b_1()
            .border_color(line())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(10.0)).rounded_full().bg(if paused || busy {
                        rgb(0x9c9fa4)
                    } else {
                        rgb(0xe33442)
                    }))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(status),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(44.0))
                            .line_height(px(50.0))
                            .font_family("monospace")
                            .font_weight(FontWeight::BOLD)
                            .child(self.recording_timecode()),
                    )
                    .when_some(self.camera_frame.clone(), |this, frame| {
                        this.child(
                            div()
                                .flex_none()
                                .flex()
                                .flex_col()
                                .items_center()
                                .child(
                                    gpui::img(frame)
                                        .size(px(88.0))
                                        .object_fit(gpui::ObjectFit::Contain),
                                )
                                .child(div().text_xs().text_color(muted()).child(if paused {
                                    "Camera paused"
                                } else {
                                    "Camera preview"
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(action_button(
                        "launcher-stop",
                        if self.recording_state == RecordingState::Finishing {
                            "Saving…"
                        } else {
                            "Stop recording"
                        },
                        "icons/stop.svg",
                        RecordingAction::Stop,
                        true,
                    ))
                    .child(action_button(
                        "launcher-pause",
                        if paused { "Resume" } else { "Pause" },
                        if paused {
                            "icons/play.svg"
                        } else {
                            "icons/pause.svg"
                        },
                        if paused {
                            RecordingAction::Resume
                        } else {
                            RecordingAction::Pause
                        },
                        false,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted())
                            .child("Stop to save and open the editor"),
                    )
                    .child(
                        div().flex().gap_3().children(
                            [
                                ("launcher-restart", "Restart", RecordingAction::Restart),
                                ("launcher-discard", "Discard", RecordingAction::Discard),
                            ]
                            .into_iter()
                            .map(|(id, label, action)| {
                                div()
                                    .id(id)
                                    .h(px(32.0))
                                    .flex()
                                    .items_center()
                                    .text_xs()
                                    .text_color(muted())
                                    .when(busy, |this| this.opacity(0.5))
                                    .when(!busy, |this| {
                                        this.cursor_pointer()
                                            .hover(|style| style.text_color(ink()))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.run_recording_action(action, cx);
                                                cx.notify();
                                            }))
                                    })
                                    .child(label)
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }
}
