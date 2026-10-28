use crate::{
    brand, brand_hover, control_bg, danger, ink, line, muted, surface, RecordingAction,
    RecordingState, Studio, TextRoles,
};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use gpui::{div, prelude::*, px, svg, AnyElement, Context, FontWeight, Timer};
use std::time::Duration;

impl Studio {
    pub(crate) fn launcher_recording_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let paused = self.recording_state == RecordingState::Paused;
        let busy = self.recording_busy
            || matches!(
                self.recording_state,
                RecordingState::Starting | RecordingState::Finishing
            );
        let status = match self.recording_state {
            RecordingState::Starting => self.recording_start_label(),
            RecordingState::Finishing => "Saving recording…".into(),
            RecordingState::Paused => "Recording paused".into(),
            _ => "Recording in progress".into(),
        };
        let cancel_start = (self.recording_state == RecordingState::Starting).then(||
            self.small_button("launcher-cancel-start", "Cancel recording", true, cx, |this, cx| this.cancel_recording_start(cx)));
        let action_button =
            |id: &'static str, label: &'static str, icon: &'static str, action, primary: bool| {
                let shortcut = if primary { "stop" } else { "pause" };
                let hint = self.global_shortcut_hint(shortcut, label);
                crate::controls::keyboard_focusable(div().id(id))
                    .when_some(hint, |this, hint| this.tooltip(crate::timeline_ui::tooltip(hint)))
                    .h(px(48.0))
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .rounded_lg()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(if primary { brand() } else { surface() })
                    .text_color(if primary { gpui::white() } else { ink() })
                    .border_1()
                    .border_color(if primary { brand() } else { line() })
                    .when(busy, |this| this.opacity(0.5))
                    .when(!busy, |this| {
                        this.cursor_pointer()
                            .hover(move |style| {
                                style.bg(if primary { brand_hover() } else { control_bg() })
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
            .bg(surface())
            .border_b_1()
            .border_color(line())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(10.0)).rounded_full().bg(if paused || busy {
                        muted()
                    } else {
                        brand()
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
                            .tabular()
                            .font_weight(FontWeight::SEMIBOLD)
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
            .when_some(cancel_start, |panel, cancel| panel.child(cancel))
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
                    .justify_end()
                    .child(
                        div().flex().gap_3().children(
                            [
                                ("launcher-restart", "Restart", "Restart take?", RecordingAction::Restart),
                                ("launcher-discard", "Discard", "Discard take?", RecordingAction::Discard),
                            ]
                            .into_iter()
                            .map(|(id, label, confirm, action)| {
                                let armed = self.armed_recording_action == Some(id);
                                div()
                                    .id(id)
                                    .h(px(32.0))
                                    .flex()
                                    .items_center()
                                    .text_xs()
                                    .when(armed, |this| this.font_weight(FontWeight::SEMIBOLD))
                                    .text_color(if armed { danger() } else { muted() })
                                    .when(busy, |this| this.opacity(0.5))
                                    .when(!busy, |this| {
                                        this.cursor_pointer()
                                            .when(!armed, |this| this.hover(|style| style.text_color(ink())))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.confirm_recording_action(id, action, cx);
                                                cx.notify();
                                            }))
                                    })
                                    .child(if armed { confirm } else { label })
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    /// Destructive recording actions take two clicks: the first arms the
    /// button for three seconds, the second runs it.
    fn confirm_recording_action(
        &mut self,
        id: &'static str,
        action: RecordingAction,
        cx: &mut Context<Self>,
    ) {
        if self.armed_recording_action.take() == Some(id) {
            self.armed_recording_timer = None;
            self.run_recording_action(action, cx);
            return;
        }
        self.armed_recording_action = Some(id);
        self.armed_recording_timer = Some(cx.spawn(async move |weak, cx| {
            Timer::after(Duration::from_secs(3)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.armed_recording_action == Some(id) {
                    this.armed_recording_action = None;
                    cx.notify();
                }
            });
        }));
    }

    /// Tooltip text for a recording control with a desktop-wide shortcut.
    fn global_shortcut_hint(&self, id: &str, label: &str) -> Option<String> {
        self.global_shortcut_triggers
            .iter()
            .find(|(shortcut, trigger)| shortcut == id && !trigger.is_empty())
            .map(|(_, trigger)| format!("{label} ({trigger})"))
    }

    /// Binds Stop and Pause as desktop-wide shortcuts through the XDG
    /// GlobalShortcuts portal, once per window. The desktop may ask the user
    /// to confirm the bindings the first time; without the portal the
    /// in-app controls still work.
    pub(crate) fn ensure_global_shortcuts(&mut self, cx: &mut Context<Self>) {
        if self.global_shortcuts.is_some() {
            return;
        }
        self.global_shortcuts = Some(cx.spawn(async move |weak, cx| {
            let Ok(portal) = GlobalShortcuts::new().await else { return };
            let Ok(session) = portal.create_session().await else { return };
            let shortcuts = [
                NewShortcut::new("stop", "Stop recording").preferred_trigger("CTRL+ALT+S"),
                NewShortcut::new("pause", "Pause or resume recording")
                    .preferred_trigger("CTRL+ALT+P"),
            ];
            let Ok(request) = portal.bind_shortcuts(&session, &shortcuts, None).await else {
                return;
            };
            let Ok(bound) = request.response() else { return };
            let triggers = bound
                .shortcuts()
                .iter()
                .map(|shortcut| (shortcut.id().to_owned(), shortcut.trigger_description().to_owned()))
                .collect::<Vec<_>>();
            if weak
                .update(cx, |this, cx| {
                    this.global_shortcut_triggers = triggers;
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            let Ok(mut activated) = portal.receive_activated().await else { return };
            // `session` stays alive for as long as this loop listens.
            while let Some(event) = activated.next().await {
                let id = event.shortcut_id().to_owned();
                if weak.update(cx, |this, cx| this.run_global_shortcut(&id, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn run_global_shortcut(&mut self, id: &str, cx: &mut Context<Self>) {
        let action = match (id, self.recording_state) {
            ("stop", RecordingState::Recording | RecordingState::Paused) => RecordingAction::Stop,
            ("pause", RecordingState::Recording) => RecordingAction::Pause,
            ("pause", RecordingState::Paused) => RecordingAction::Resume,
            _ => return,
        };
        self.run_recording_action(action, cx);
        cx.notify();
    }
}
