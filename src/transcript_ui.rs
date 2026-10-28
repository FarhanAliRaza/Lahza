//! Narration transcript in the recording editor: on-device transcription,
//! the subtitle settings, the transcript panel (caption text editing and
//! transcript-driven cuts), and SubRip export.
use crate::recording::{
    clips::RecordingClipTimeline,
    model::RecordingSession,
    transcript::planner,
    transcription::{self, JobProgress, SpeechModel},
};
use crate::scene_ui::SceneSlider;
use crate::controls::{button, ButtonKind, ButtonSize};
use crate::{accent, brand, control_bg, ink, line, muted, Studio, TextRoles, VideoEditSnapshot};
use gpui::{
    div, prelude::*, px, AnyElement, Context, SharedString, Timer,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// What the transcript panel derives from the transcript and the clip
/// timeline, rebuilt only when either changes so playback frames only
/// look up the speaking word.
pub(crate) struct TranscriptLayout {
    revision: u64,
    clips: RecordingClipTimeline,
    fillers: usize,
    pauses: usize,
    /// Per word: still in the edit, and its editor start time.
    words: Vec<(bool, Option<f64>)>,
    /// (editor start, editor end, word index) of present words, by start.
    spoken: Vec<(f64, f64, usize)>,
    cues: Vec<CueLayout>,
}

struct CueLayout {
    /// Index into `Transcript::cues`.
    cue: usize,
    words: std::ops::Range<usize>,
    stamp: SharedString,
    /// The caption text was edited away from its words.
    edited: bool,
}

impl TranscriptLayout {
    fn build(
        transcript: &crate::recording::transcript::Transcript,
        clips: &RecordingClipTimeline,
        source_duration: f64,
        revision: u64,
    ) -> Self {
        let still_present = |(start, end): &(f64, f64)| !clips.slices_overlapping(*start, *end).is_empty();
        let fillers = planner::filler_cut_ranges(&transcript.words, source_duration)
            .iter()
            .filter(|range| still_present(range))
            .count();
        let pauses = planner::silence_cut_ranges(&transcript.words, source_duration)
            .iter()
            .filter(|range| still_present(range))
            .count();
        let words: Vec<_> = transcript
            .words
            .iter()
            .map(|word| {
                let present = clips.editor_time_for_event((word.start + word.end) * 0.5).is_some();
                (present, clips.editor_time_for_event(word.start))
            })
            .collect();
        let mut spoken: Vec<_> = transcript
            .words
            .iter()
            .zip(&words)
            .enumerate()
            .filter_map(|(index, (word, (present, start)))| {
                let start = start.filter(|_| *present)?;
                Some((start, start + (word.end - word.start).max(0.05), index))
            })
            .collect();
        spoken.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Words grouped under the cue they start in.
        let mut order: Vec<_> = (0..transcript.cues.len()).collect();
        order.sort_by(|a, b| transcript.cues[*a].start.total_cmp(&transcript.cues[*b].start));
        let mut word_index = 0;
        let mut cues = Vec::with_capacity(order.len());
        for (position, &cue_index) in order.iter().enumerate() {
            let cue = &transcript.cues[cue_index];
            let next_start = order
                .get(position + 1)
                .map_or(f64::INFINITY, |next| transcript.cues[*next].start);
            let start_index = word_index;
            while word_index < transcript.words.len()
                && transcript.words[word_index].start < next_start - 0.001
            {
                word_index += 1;
            }
            let editor_start = clips.editor_time_for_event(cue.start).or_else(|| {
                clips.slices_overlapping(cue.start, cue.end).first().map(|slice| slice.editor_start)
            });
            let stamp = match editor_start {
                Some(time) => format!("{}:{:02}", (time / 60.0) as u64, (time % 60.0) as u64),
                None => "cut".into(),
            };
            let joined = transcript.words[start_index..word_index]
                .iter()
                .map(|word| word.display())
                .collect::<Vec<_>>()
                .join(" ");
            cues.push(CueLayout {
                cue: cue_index,
                words: start_index..word_index,
                stamp: stamp.into(),
                edited: cue.text.trim() != joined,
            });
        }
        Self { revision, clips: clips.clone(), fillers, pauses, words, spoken, cues }
    }

    /// The present word under the playhead, by binary search.
    fn speaking(&self, playhead: f64) -> Option<usize> {
        let after = self.spoken.partition_point(|(start, _, _)| *start <= playhead);
        self.spoken[..after]
            .iter()
            .rev()
            .take(2)
            .find(|(_, end, _)| playhead < *end)
            .map(|(_, _, index)| *index)
    }
}

/// A transcription running in the background.
pub(crate) struct TranscriptionJob {
    progress: JobProgress,
    /// True while the speech model downloads, false once whisper runs.
    downloading: Arc<AtomicBool>,
    session: RecordingSession,
}

impl Studio {
    // ------------------------------------------------------------------
    // Transcription job
    // ------------------------------------------------------------------

    pub(crate) fn start_transcription(&mut self, cx: &mut Context<Self>) {
        if self.transcription_job.is_some() || self.video_edit_busy || self.speech_setup_job.is_some() {
            return;
        }
        let Some(session) = self.video_project.clone() else {
            return;
        };
        // Starting a job is rare; check the disk rather than the cache.
        self.refresh_speech_availability();
        if !self.speech_availability().whisper {
            self.toast = Some(crate::notifications::Notification::with_action(
                "Download the speech model to transcribe",
                "Download",
                |this, cx| this.start_speech_setup(true, cx),
            ));
            cx.notify();
            return;
        }
        self.transcription_error = None;
        let model = self.speech_model;
        let source = Self::media_source_for(&session);
        let duration = self.video_source_duration;
        let progress = JobProgress::default();
        let downloading = Arc::new(AtomicBool::new(!model.is_installed()));
        self.transcription_job = Some(TranscriptionJob {
            progress: progress.clone(),
            downloading: downloading.clone(),
            session: session.clone(),
        });
        let job_progress = progress.clone();
        let task = cx.background_executor().spawn(async move {
            let model_path = if model.is_installed() {
                model.path()
            } else {
                let path = transcription::download_model(model, &progress)?;
                downloading.store(false, Ordering::Relaxed);
                progress.set(0.0);
                path
            };
            transcription::transcribe(&source, duration, &model_path, &progress)
        });
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(150)).await;
            let active = weak
                .update(cx, |this, cx| {
                    cx.notify();
                    this.transcription_job
                        .as_ref()
                        .is_some_and(|job| Arc::ptr_eq(&job.progress.fraction, &job_progress.fraction))
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
                // The job may have downloaded a model.
                this.refresh_speech_availability();
                let Some(job) = this.transcription_job.take() else {
                    return;
                };
                if this.video_project.as_ref() != Some(&job.session) {
                    return;
                }
                match result {
                    Ok(transcript) if transcript.is_empty() => {
                        this.toast = Some("No speech was found in this recording".into());
                    }
                    Ok(transcript) => {
                        this.video_undo_stack
                            .push(VideoEditSnapshot::Transcript(this.video_transcript.clone()));
                        this.video_redo_stack.clear();
                        this.video_transcript = Some(transcript);
                        this.subtitle_style.visible = true;
                        this.transcript_selection = None;
                        this.transcript_selected_cue = None;
                        this.transcript_changed(cx);
                        this.toast = Some("Transcript ready".into());
                    }
                    Err(crate::recording::video::VideoError::Cancelled) => {
                        this.toast = Some("Transcription cancelled".into());
                    }
                    Err(error) => {
                        this.transcription_error = Some(format!("Transcription failed: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn cancel_transcription(&mut self) {
        if let Some(job) = self.transcription_job.as_ref() {
            job.progress.cancel();
        }
    }

    /// Persist the transcript and rebuild everything derived from it.
    pub(crate) fn transcript_changed(&mut self, cx: &mut Context<Self>) {
        self.video_transcript_revision += 1;
        if let Some(session) = self.video_project.as_ref() {
            if let Err(error) = session.write_edit_field("transcript", &self.video_transcript) {
                self.toast = Some(crate::Notification::error(format!("Could not autosave transcript: {error}")));
            }
        }
        cx.notify();
    }

    fn remove_transcript(&mut self, cx: &mut Context<Self>) {
        if self.video_edit_busy || self.video_transcript.is_none() {
            return;
        }
        self.video_undo_stack
            .push(VideoEditSnapshot::Transcript(self.video_transcript.take()));
        self.video_redo_stack.clear();
        self.transcript_open = false;
        self.transcript_selection = None;
        self.transcript_selected_cue = None;
        self.transcript_changed(cx);
    }

    /// Apply an undo/redo snapshot and return its inverse.
    pub(crate) fn restore_transcript_snapshot(
        &mut self,
        snapshot: VideoEditSnapshot,
        cx: &mut Context<Self>,
    ) -> VideoEditSnapshot {
        self.transcript_selection = None;
        match snapshot {
            VideoEditSnapshot::TranscriptCut { timeline, annotations, transcript } => {
                let inverse = VideoEditSnapshot::TranscriptCut {
                    timeline: self.video_clip_timeline.clone(),
                    annotations: self.annotations.clone(),
                    transcript: self.video_transcript.clone(),
                };
                let selected = self.video_selected_clip;
                self.apply_video_clip_timeline(timeline, selected, false, cx);
                self.annotations = annotations;
                self.finish_annotation_interaction();
                let _ = self.rebuild_redactions();
                self.video_transcript = transcript;
                self.transcript_changed(cx);
                inverse
            }
            VideoEditSnapshot::Transcript(transcript) => {
                let inverse = VideoEditSnapshot::Transcript(std::mem::replace(&mut self.video_transcript, transcript));
                if self.video_transcript.is_none() {
                    self.transcript_open = false;
                }
                self.transcript_selected_cue = None;
                self.transcript_changed(cx);
                inverse
            }
            other => other,
        }
    }

    // ------------------------------------------------------------------
    // Transcript-driven cuts
    // ------------------------------------------------------------------

    /// Cut source ranges from the clips and drop the cut words from captions,
    /// as one undo step.
    fn cut_source_ranges(&mut self, ranges: Vec<(f64, f64)>, cx: &mut Context<Self>) {
        if self.video_edit_busy {
            return;
        }
        let merged = RecordingClipTimeline::merged_ranges(&ranges);
        let Some(next) = (!merged.is_empty())
            .then(|| self.video_clip_timeline.removing_source_ranges(&merged))
            .flatten()
            .map(|timeline| timeline.normalized(self.video_source_duration))
        else {
            self.toast = Some("That would remove the whole video".into());
            cx.notify();
            return;
        };
        if next == self.video_clip_timeline {
            self.toast = Some("Already removed".into());
            cx.notify();
            return;
        }
        self.video_undo_stack.push(VideoEditSnapshot::TranscriptCut {
            timeline: self.video_clip_timeline.clone(),
            annotations: self.annotations.clone(),
            transcript: self.video_transcript.clone(),
        });
        self.video_redo_stack.clear();
        if let Some(transcript) = self.video_transcript.as_ref() {
            self.video_transcript = Some(transcript.removing_cut_words(&merged));
        }
        self.transcript_selection = None;
        let selected = self.video_selected_clip;
        self.apply_video_clip_timeline(next, selected, false, cx);
        self.transcript_changed(cx);
    }

    fn cut_selected_words(&mut self, cx: &mut Context<Self>) {
        let (Some(transcript), Some((anchor, head))) = (self.video_transcript.as_ref(), self.transcript_selection) else {
            return;
        };
        if let Some(range) =
            planner::cut_range(&transcript.words, anchor.min(head), anchor.max(head), self.video_source_duration)
        {
            self.cut_source_ranges(vec![range], cx);
        }
    }

    fn remove_fillers(&mut self, cx: &mut Context<Self>) {
        let Some(transcript) = self.video_transcript.as_ref() else { return };
        let ranges = planner::filler_cut_ranges(&transcript.words, self.video_source_duration);
        self.cut_source_ranges(ranges, cx);
    }

    fn trim_pauses(&mut self, cx: &mut Context<Self>) {
        let Some(transcript) = self.video_transcript.as_ref() else { return };
        let ranges = planner::silence_cut_ranges(&transcript.words, self.video_source_duration);
        self.cut_source_ranges(ranges, cx);
    }

    /// The panel's derived layout, rebuilt when the transcript or the clip
    /// timeline changed since the last render.
    fn transcript_layout(&self) -> Option<std::cell::Ref<'_, TranscriptLayout>> {
        let transcript = self.video_transcript.as_ref()?;
        let stale = self.transcript_layout.borrow().as_ref().is_none_or(|layout| {
            layout.revision != self.video_transcript_revision || layout.clips != self.video_clip_timeline
        });
        if stale {
            *self.transcript_layout.borrow_mut() = Some(TranscriptLayout::build(
                transcript,
                &self.video_clip_timeline,
                self.video_source_duration,
                self.video_transcript_revision,
            ));
        }
        std::cell::Ref::filter_map(self.transcript_layout.borrow(), Option::as_ref).ok()
    }

    fn click_transcript_word(&mut self, index: usize, extend: bool, cx: &mut Context<Self>) {
        let Some(word) = self.video_transcript.as_ref().and_then(|t| t.words.get(index)) else {
            return;
        };
        let editor_time = self.video_clip_timeline.editor_time_for_event(word.start);
        self.transcript_selection = Some(match self.transcript_selection {
            Some((anchor, _)) if extend => (anchor, index),
            _ => (index, index),
        });
        if let Some(time) = editor_time {
            self.pause_video_playback();
            self.seek_video(time, cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Caption text editing (through the shared `cue` text field)
    // ------------------------------------------------------------------

    pub(crate) fn selected_cue_text(&self) -> Option<(Uuid, String)> {
        let id = self.transcript_selected_cue?;
        let cue = self.video_transcript.as_ref()?.cues.iter().find(|cue| cue.id == id)?;
        Some((id, cue.text.clone()))
    }

    pub(crate) fn begin_cue_text_edit(&mut self) {
        self.pause_video_playback();
        self.video_undo_stack
            .push(VideoEditSnapshot::Transcript(self.video_transcript.clone()));
        self.video_redo_stack.clear();
    }

    pub(crate) fn set_cue_text(&mut self, id: Uuid, text: &str, persist: bool, cx: &mut Context<Self>) {
        let Some(cue) = self
            .video_transcript
            .as_mut()
            .and_then(|transcript| transcript.cues.iter_mut().find(|cue| cue.id == id))
        else {
            return;
        };
        if cue.text != text {
            cue.text = text.to_string();
            self.video_transcript_revision += 1;
        }
        if persist {
            self.transcript_changed(cx);
        }
    }

    // ------------------------------------------------------------------
    // Subtitle export
    // ------------------------------------------------------------------

    fn export_subtitles(&mut self, cx: &mut Context<Self>) {
        let Some(transcript) = self.video_transcript.clone() else {
            return;
        };
        let srt = transcript.to_srt(&self.video_clip_timeline);
        let directory = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
        let name = chrono::Local::now()
            .format("Lahza-%Y-%m-%d_%H-%M-%S.srt")
            .to_string();
        let prompt = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |weak, cx| {
            let selected = prompt.await.ok().and_then(Result::ok).flatten();
            let _ = weak.update(cx, |this, cx| {
                this.toast = Some(match selected {
                    Some(path) => match std::fs::write(&path, srt) {
                        Ok(()) => crate::notifications::Notification::exported(
                            format!("Saved captions to {}", path.display()),
                            path,
                        ),
                        Err(error) => crate::Notification::error(format!("Could not save captions: {error}")),
                    },
                    None => "Export cancelled".into(),
                });
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Inspector
    // ------------------------------------------------------------------

    fn text_button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        enabled: bool,
        primary: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Studio, &mut Context<Studio>) + 'static,
    ) -> AnyElement {
        button(
            gpui::ElementId::Name(id.into()),
            label,
            if primary { ButtonKind::Primary } else { ButtonKind::Secondary },
            ButtonSize::Sm,
        )
        .disabled(!enabled)
        .on_click(cx.listener(move |this, _, _, cx| {
            on_click(this, cx);
            cx.notify();
        }))
        .into_any_element()
    }

    /// Record → Subtitles.
    pub(crate) fn subtitle_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut section = div().flex().flex_col().gap(px(crate::theme::SPACE_3))
            .child(
                div().id("subtitle-speech-advanced")
                    .flex().items_center().gap_1().py_1()
                    .text_xs().text_color(muted()).cursor_pointer()
                    .hover(|style| style.text_color(ink()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.subtitle_speech_advanced_open = !this.subtitle_speech_advanced_open;
                        cx.notify();
                    }))
                    .child(gpui::svg().path(if self.subtitle_speech_advanced_open {
                        "icons/chevron-down.svg"
                    } else {
                        "icons/chevron-right.svg"
                    }).size(px(12.0)).text_color(muted()))
                    .child("Advanced"),
            )
            .when(self.subtitle_speech_advanced_open, |section| {
                section.child(self.speech_processing_controls("subtitle-processing", cx))
            })
            .when(!self.subtitle_speech_advanced_open, |section| {
                section.children(self.speech_setup_notice("subtitle-setup-notice", cx))
            });
        if let Some(job) = self.transcription_job.as_ref() {
            let percent = (job.progress.get() * 100.0).round();
            let label = if job.downloading.load(Ordering::Relaxed) {
                format!("Downloading speech model… {percent:.0}%")
            } else {
                format!("Transcribing… {percent:.0}%")
            };
            return section
                .child(div().row_label().child(label))
                .child(
                    div()
                        .h(px(4.0))
                        .w_full()
                        .rounded_full()
                        .bg(control_bg())
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(accent())
                                .w(gpui::relative(job.progress.get() as f32)),
                        ),
                )
                .child(self.text_button("transcribe-cancel", "Cancel", true, false, cx, |this, _| {
                    this.cancel_transcription()
                }))
                .into_any_element();
        }
        let Some(transcript) = self.video_transcript.as_ref() else {
            let available = self.speech_availability();
            let installed = available.whisper;
            let model = self.speech_model;
            section = section.child(div().hint().child(
                "Transcribe speech into captions on this computer. Nothing is uploaded.",
            ))
            .when_some(self.transcription_error.clone(), |section, error| {
                section.child(crate::controls::inline_error(error).child(self.small_button(
                    "transcribe-retry",
                    "Retry",
                    !self.video_edit_busy && self.speech_setup_job.is_none(),
                    cx,
                    |this, cx| this.start_transcription(cx),
                )))
            });
            if !installed {
                return section
                    .child(div().hint().child(
                        "Download the speech model above to transcribe this recording.",
                    ))
                    .into_any_element();
            }
            let selected = SpeechModel::ALL.iter().position(|m| *m == model).unwrap_or(0);
            let label = if available.model_installed(model) {
                "Transcribe".to_string()
            } else {
                format!("Download model ({} MB) and transcribe", model.megabytes())
            };
            return section
                .child(self.segmented(
                    "speech-model",
                    &["Fast", "Accurate"],
                    selected,
                    |this, index| this.speech_model = SpeechModel::ALL[index],
                    cx,
                ))
                .child(div().hint().child(match model {
                    SpeechModel::Base => "Quick and good for clear speech.",
                    SpeechModel::Small => "Slower, better with accents and noise.",
                }))
                .child(self.text_button(
                    "transcribe-start",
                    label,
                    !self.video_edit_busy && self.speech_setup_job.is_none(),
                    true,
                    cx,
                    |this, cx| this.start_transcription(cx),
                ))
                .into_any_element();
        };
        let cue_count = transcript.cues.len();
        let style = self.subtitle_style;
        section
            .child(div().hint().child(format!(
                "{cue_count} caption{}{}",
                if cue_count == 1 { "" } else { "s" },
                transcript
                    .language
                    .as_deref()
                    .map(|language| format!(" · {}", language.to_uppercase()))
                    .unwrap_or_default()
            )))
            .child(self.scene_toggle_row("subtitles-visible", "Show captions", style.visible, cx, |this| {
                this.subtitle_style.visible = !this.subtitle_style.visible
            }))
            .when(style.visible, |this| {
                this.child(self.scene_toggle_row(
                    "subtitles-karaoke",
                    "Highlight spoken word",
                    style.karaoke,
                    cx,
                    |this| this.subtitle_style.karaoke = !this.subtitle_style.karaoke,
                ))
                .child(self.scene_slider_row(SceneSlider::SubtitlePosition, cx))
                .child(self.scene_slider_row(SceneSlider::SubtitleSize, cx))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.text_button("transcript-open", "Edit transcript", true, true, cx, |this, _| {
                        this.transcript_open = true
                    }))
                    .child(self.text_button("transcript-srt", "Export .srt…", true, false, cx, |this, cx| {
                        this.export_subtitles(cx)
                    }))
                    .child(self.text_button(
                        "transcript-again",
                        "Transcribe again",
                        !self.video_edit_busy,
                        false,
                        cx,
                        |this, cx| this.start_transcription(cx),
                    ))
                    .child(self.text_button("transcript-remove", "Remove", !self.video_edit_busy, false, cx, |this, cx| {
                        this.remove_transcript(cx)
                    })),
            )
            .into_any_element()
    }

    /// The transcript editor, shown in place of the Record tab: caption text
    /// editing and cutting footage by words.
    pub(crate) fn transcript_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(transcript), Some(layout)) = (self.video_transcript.as_ref(), self.transcript_layout()) else {
            return div().into_any_element();
        };
        let (fillers, pauses) = (layout.fillers, layout.pauses);
        let selection = self
            .transcript_selection
            .map(|(anchor, head)| (anchor.min(head), anchor.max(head)));
        let speaking_word = layout.speaking(self.video_position);
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(div().section_title().child("Transcript"))
            .child(self.text_button("transcript-done", "Done", true, false, cx, |this, _| {
                this.transcript_open = false;
                this.transcript_selected_cue = None;
            }));
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(self.text_button(
                "transcript-cut",
                match selection {
                    Some((first, last)) if last > first => format!("Cut {} words", last - first + 1),
                    _ => "Cut word".into(),
                },
                selection.is_some() && !self.video_edit_busy,
                true,
                cx,
                |this, cx| this.cut_selected_words(cx),
            ))
            .child(self.text_button(
                "transcript-fillers",
                format!("Remove fillers ({fillers})"),
                fillers > 0 && !self.video_edit_busy,
                false,
                cx,
                |this, cx| this.remove_fillers(cx),
            ))
            .child(self.text_button(
                "transcript-pauses",
                format!("Trim pauses ({pauses})"),
                pauses > 0 && !self.video_edit_busy,
                false,
                cx,
                |this, cx| this.trim_pauses(cx),
            ));
        let hint = div().hint().child(
            "Click a word to jump to it; Shift-click to select a range. Click a time to edit that caption's text.",
        );
        let words = &transcript.words;
        let mut list = div().flex().flex_col().gap_2();
        for (cue_index, cue_layout) in layout.cues.iter().enumerate() {
            let cue = &transcript.cues[cue_layout.cue];
            let cue_id = cue.id;
            let editing = self.transcript_selected_cue == Some(cue_id);
            let stamp = cue_layout.stamp.clone();
            let mut flow = div().flex().flex_wrap().gap(px(2.0));
            for index in cue_layout.words.clone() {
                let word = &words[index];
                let present = layout.words[index].0;
                let selected = selection.is_some_and(|(first, last)| index >= first && index <= last);
                let speaking = speaking_word == Some(index);
                let filler = planner::is_filler(word);
                flow = flow.child(
                    div()
                        .id(("transcript-word", index))
                        .px(px(3.0))
                        .rounded_sm()
                        .text_sm()
                        .cursor_pointer()
                        .when(!present, |this| this.line_through().text_color(muted()).opacity(0.6))
                        .when(present && filler, |this| this.text_color(crate::theme::warning()))
                        .when(speaking, |this| this.bg(brand().opacity(0.16)))
                        .when(selected, |this| this.bg(crate::selection_fill()))
                        .hover(|style| style.bg(control_bg()))
                        .child(word.display().to_string())
                        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            this.click_transcript_word(index, event.modifiers().shift, cx);
                        })),
                );
            }
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if editing { accent() } else { line() })
                    .child(
                        div()
                            .id(("transcript-cue", cue_index))
                            .text_xs()
                            .text_color(muted())
                            .cursor_pointer()
                            .hover(|style| style.text_color(ink()))
                            .child(stamp)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.transcript_selected_cue = Some(cue_id);
                                if let Some(time) = this
                                    .video_transcript
                                    .as_ref()
                                    .and_then(|t| t.cues.iter().find(|cue| cue.id == cue_id))
                                    .and_then(|cue| this.video_clip_timeline.editor_time_for_event(cue.start))
                                {
                                    this.pause_video_playback();
                                    this.seek_video(time, cx);
                                }
                                cx.notify();
                            })),
                    )
                    .when(editing, |this| this.child(self.text_fields.cue.clone()))
                    .when(!editing && cue_layout.edited, |this| {
                        // The caption was edited; show what the bar displays.
                        this.child(div().text_xs().italic().text_color(muted()).child(format!("Caption: {}", cue.text)))
                    })
                    .child(flow),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(header)
            .child(actions)
            .child(hint)
            .child(list)
            .into_any_element()
    }
}
