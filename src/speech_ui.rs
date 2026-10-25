//! Shared on-device speech setup and processing preferences.
use crate::recording::{
    speech_runtime::{self, ProcessingMode, RuntimeStatus},
    speech_setup,
    teleprompter::whisper_stream_executable,
    transcription::{self, JobProgress, SpeechModel},
    video::VideoError,
};
use crate::{blue, ink, line, muted, RecordingState, Studio};
use gpui::{div, prelude::*, px, rgb, AnyElement, Context, FontWeight, Timer};
use std::{sync::Arc, time::Duration};

impl Studio {
    pub(crate) fn initialize_speech_processing(&mut self, cx: &mut Context<Self>) {
        if !self.speech_setup_needed() && self.speech_runtime_status == RuntimeStatus::Checking {
            self.start_speech_setup(false, cx);
        }
    }

    /// The launcher and editor can be separate windows, but their speech
    /// preference and active Whisper backend are shared.
    pub(crate) fn refresh_speech_processing(&mut self, cx: &mut Context<Self>) {
        let mode = speech_runtime::load_processing_mode();
        let status = speech_runtime::current_status();
        if mode != self.speech_processing || status != self.speech_runtime_status {
            if mode != self.speech_processing {
                self.speech_processing = mode;
                self.speech_processing_status = None;
            }
            self.speech_runtime_status = status;
            cx.notify();
        }
    }

    pub(crate) fn speech_setup_needed(&self) -> bool {
        transcription::whisper_executable().is_none()
            || whisper_stream_executable().is_none()
            || !SpeechModel::Base.is_installed()
    }

    fn speech_settings_available(&self) -> bool {
        self.recording_state == RecordingState::Idle
            && !self.recording_busy
            && !self.video_edit_busy
            && self.transcription_job.is_none()
            && self.speech_setup_job.is_none()
            && !speech_setup::is_busy()
    }

    fn select_speech_processing(&mut self, mode: ProcessingMode, cx: &mut Context<Self>) {
        if !self.speech_settings_available() || mode == self.speech_processing {
            return;
        }
        if let Err(error) = speech_runtime::save_processing_mode(mode) {
            self.toast = Some(format!("Could not save speech processing: {error}").into());
            cx.notify();
            return;
        }
        self.speech_processing = mode;
        self.speech_processing_status = None;
        self.speech_runtime_status = RuntimeStatus::Checking;
        speech_runtime::set_status(RuntimeStatus::Checking);
        if !self.speech_setup_needed() {
            self.start_speech_setup(false, cx);
        }
        cx.notify();
    }

    pub(crate) fn start_speech_setup(&mut self, install: bool, cx: &mut Context<Self>) {
        if !self.speech_settings_available() {
            return;
        }
        let progress = JobProgress::default();
        self.speech_setup_job = Some(progress.clone());
        self.speech_setup_installing = install;
        self.speech_processing_status = None;
        self.speech_runtime_status = RuntimeStatus::Checking;
        speech_runtime::set_status(RuntimeStatus::Checking);
        let job = progress.clone();
        let task = cx.background_executor().spawn(async move {
            if install {
                speech_setup::setup(&job)
            } else {
                speech_setup::check(&job)
            }
        });
        let ticker = progress.clone();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(200)).await;
            let active = weak
                .update(cx, |this, cx| {
                    cx.notify();
                    this.speech_setup_job
                        .as_ref()
                        .is_some_and(|job| Arc::ptr_eq(&job.fraction, &ticker.fraction))
                })
                .unwrap_or(false);
            if !active {
                break;
            }
        })
        .detach();
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.speech_setup_job = None;
                this.speech_runtime_status = speech_runtime::current_status();
                this.speech_processing_status = match result {
                    Ok(_) => None,
                    Err(VideoError::Cancelled) => Some("Voice setup cancelled.".into()),
                    Err(error) => Some(format!("Voice setup failed: {error}")),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Keep first-time setup and failures reachable while advanced controls
    /// are collapsed. Routine device checks do not add a status row.
    pub(crate) fn speech_setup_notice(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(progress) = &self.speech_setup_job {
            if !self.speech_setup_installing {
                return None;
            }
            return Some(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .text_color(muted())
                            .child(format!("Setting up… {:.0}%", progress.get() * 100.0)),
                    )
                    .child(
                        div()
                            .id("speech-notice-cancel")
                            .text_color(blue())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(job) = &this.speech_setup_job {
                                    job.cancel();
                                }
                                cx.notify();
                            }))
                            .child("Cancel"),
                    )
                    .into_any_element(),
            );
        }
        let install = self.speech_setup_needed();
        if !install && self.speech_processing_status.is_none() {
            return None;
        }
        let mut notice = div().id(id).flex().flex_col().gap_2().min_w_0();
        if let Some(status) = &self.speech_processing_status {
            notice = notice.child(
                div()
                    .id("speech-notice-status")
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(muted())
                    .child(status.clone())
                    .tooltip(crate::timeline_ui::tooltip(status.clone())),
            );
        }
        if install {
            let enabled = self.speech_settings_available();
            notice = notice.child(
                div()
                    .id("speech-notice-setup")
                    .h(px(28.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .border_1()
                    .border_color(line())
                    .bg(rgb(0xffffff))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .opacity(if enabled { 1.0 } else { 0.45 })
                    .when(enabled, |this| {
                        this.cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_speech_setup(true, cx);
                            }))
                    })
                    .child(self.speech_setup_label()),
            );
        }
        Some(notice.into_any_element())
    }

    fn speech_setup_label(&self) -> String {
        if !SpeechModel::Base.is_installed() {
            format!("Set up voice · {} MB", SpeechModel::Base.megabytes())
        } else {
            "Set up voice".into()
        }
    }

    pub(crate) fn speech_processing_controls(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.speech_settings_available();
        let mut modes = div().flex().gap_1();
        for (index, mode) in ProcessingMode::ALL.into_iter().enumerate() {
            let selected = self.speech_processing == mode;
            modes = modes.child(
                div()
                    .id(("speech-processing-mode", index))
                    .flex_1()
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .border_1()
                    .border_color(if selected { blue() } else { line() })
                    .bg(if selected {
                        rgb(0xeaf2ff)
                    } else {
                        rgb(0xffffff)
                    })
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if selected { blue() } else { ink() })
                    .opacity(if enabled { 1.0 } else { 0.45 })
                    .when(enabled, |this| {
                        this.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_speech_processing(mode, cx);
                            }))
                    })
                    .child(mode.label()),
            );
        }
        let mut panel = div()
            .id(id)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Processing"),
            )
            .child(modes);
        if let Some(progress) = self.speech_setup_job.as_ref() {
            let label = if self.speech_setup_installing {
                format!("Setting up… {:.0}%", progress.get() * 100.0)
            } else {
                "Checking…".into()
            };
            panel = panel
                .child(div().text_xs().text_color(muted()).child(label))
                .child(
                    div()
                        .id("speech-setup-cancel")
                        .text_xs()
                        .text_color(blue())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(job) = &this.speech_setup_job {
                                job.cancel();
                            }
                            cx.notify();
                        }))
                        .child("Cancel"),
                );
        } else {
            if let Some(status) = &self.speech_processing_status {
                panel = panel.child(div().text_xs().text_color(muted()).child(status.clone()));
            } else if self.speech_runtime_status != RuntimeStatus::Checking {
                panel = panel.child(
                    div().text_xs().text_color(muted()).child(
                        self.speech_runtime_status
                            .validate(self.speech_processing)
                            .err()
                            .unwrap_or_else(|| self.speech_runtime_status.label()),
                    ),
                );
            }
            let install = self.speech_setup_needed();
            let setup_label = if install {
                self.speech_setup_label()
            } else {
                "Check".into()
            };
            panel = panel.child(
                div()
                    .id("speech-setup")
                    .h(px(28.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .border_1()
                    .border_color(line())
                    .bg(rgb(0xffffff))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .opacity(if enabled { 1.0 } else { 0.45 })
                    .when(enabled, |this| {
                        this.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.start_speech_setup(install, cx);
                            }))
                    })
                    .child(setup_label),
            );
            if !install && speech_setup::can_install_runtime() {
                panel = panel.child(
                    div()
                        .id("speech-install-runtime")
                        .h(px(28.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .border_1()
                        .border_color(line())
                        .bg(rgb(0xffffff))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .opacity(if enabled { 1.0 } else { 0.45 })
                        .when(enabled, |this| {
                            this.cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_speech_setup(true, cx);
                                }))
                        })
                        .child("Install GPU support"),
                );
            }
        }
        panel.into_any_element()
    }
}
