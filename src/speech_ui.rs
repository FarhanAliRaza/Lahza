//! Shared on-device speech setup and processing preferences.
use crate::recording::{
    speech_runtime::{self, ProcessingMode, RuntimeStatus},
    speech_setup,
    teleprompter::whisper_stream_executable,
    transcription::{self, JobProgress, SpeechModel},
    video::VideoError,
};
use crate::controls::{button, option_button, segmented_track, ButtonKind, ButtonSize};
use crate::{muted, RecordingState, Studio, TextRoles};
use gpui::{div, prelude::*, AnyElement, Context, Timer};
use std::{sync::Arc, time::Duration};

/// Which speech tools are on disk. Finding them walks `$PATH` and stats
/// model files, so the result is cached on `Studio` and refreshed after
/// setup or transcription jobs and when the window regains focus.
#[derive(Clone, Copy)]
pub(crate) struct SpeechAvailability {
    pub(crate) whisper: bool,
    stream: bool,
    /// Indexed like `SpeechModel::ALL`.
    models: [bool; SpeechModel::ALL.len()],
}

impl SpeechAvailability {
    fn probe() -> Self {
        Self {
            whisper: transcription::whisper_executable().is_some(),
            stream: whisper_stream_executable().is_some(),
            models: SpeechModel::ALL.map(SpeechModel::is_installed),
        }
    }

    pub(crate) fn model_installed(&self, model: SpeechModel) -> bool {
        SpeechModel::ALL
            .iter()
            .position(|candidate| *candidate == model)
            .is_some_and(|index| self.models[index])
    }
}

impl Studio {
    pub(crate) fn speech_availability(&self) -> SpeechAvailability {
        if let Some(availability) = self.speech_availability.get() {
            return availability;
        }
        let availability = SpeechAvailability::probe();
        self.speech_availability.set(Some(availability));
        availability
    }

    /// Forget the cached tool check; the next read probes again.
    pub(crate) fn refresh_speech_availability(&self) {
        self.speech_availability.set(None);
    }

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
        let available = self.speech_availability();
        !available.whisper || !available.stream || !available.model_installed(SpeechModel::Base)
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
            self.toast = Some(crate::Notification::error(format!("Could not save speech processing: {error}")));
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
                this.refresh_speech_availability();
                this.speech_runtime_status = speech_runtime::current_status();
                this.speech_processing_status = match result {
                    // Only failures leave a status; it shows as an inline error.
                    Ok(_) | Err(VideoError::Cancelled) => None,
                    Err(error) => Some(format!("Speech model setup failed: {error}")),
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
                            .text_color(crate::ink())
                            .underline()
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
            notice = notice.child(crate::controls::inline_error(status.clone()));
        }
        if install {
            let enabled = self.speech_settings_available();
            notice = notice.child(
                button("speech-notice-setup", self.speech_setup_label(), ButtonKind::Secondary, ButtonSize::Sm)
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_speech_setup(true, cx);
                    })),
            );
        }
        Some(notice.into_any_element())
    }

    fn speech_setup_label(&self) -> String {
        if !self.speech_availability().model_installed(SpeechModel::Base) {
            format!("Download speech model · {} MB", SpeechModel::Base.megabytes())
        } else {
            "Set up speech model".into()
        }
    }

    pub(crate) fn speech_processing_controls(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.speech_settings_available();
        let mut modes = segmented_track();
        for (index, mode) in ProcessingMode::ALL.into_iter().enumerate() {
            let selected = self.speech_processing == mode;
            modes = modes.child(
                option_button(("speech-processing-mode", index), selected)
                    .flex_1()
                    .when(!enabled, |this| this.opacity(0.45).cursor_default())
                    .when(enabled, |this| {
                        this.on_click(cx.listener(move |this, _, _, cx| {
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
                div().field_label().child("Processing"),
            )
            .child(modes);
        if let Some(progress) = self.speech_setup_job.as_ref() {
            let label = if self.speech_setup_installing {
                format!("Setting up… {:.0}%", progress.get() * 100.0)
            } else {
                "Checking…".into()
            };
            panel = panel
                .child(div().hint().child(label))
                .child(
                    div()
                        .id("speech-setup-cancel")
                        .text_xs()
                        .text_color(crate::ink())
                        .underline()
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
                panel = panel.child(crate::controls::inline_error(status.clone()));
            } else if self.speech_runtime_status != RuntimeStatus::Checking {
                panel = panel.child(
                    div().hint().child(
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
                button("speech-setup", setup_label, ButtonKind::Secondary, ButtonSize::Sm)
                    .disabled(!enabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_speech_setup(install, cx);
                    })),
            );
            if !install && speech_setup::can_install_runtime() {
                panel = panel.child(
                    button("speech-install-runtime", "Install GPU support", ButtonKind::Secondary, ButtonSize::Sm)
                        .disabled(!enabled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_speech_setup(true, cx);
                        })),
                );
            }
        }
        panel.into_any_element()
    }
}
