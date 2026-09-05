//! Recording lifecycle, project loading, and screenshot capture requests.

use super::{
    capture_behind_window, AnnotationMark, AnnotationWorkspace, CropRect, RecordingAction,
    RecordingExtras, Studio,
};
use crate::{
    motion_ui::MotionPick,
    recording::{
        self,
        clips::RecordingClipTimeline,
        model::{PointerCaptureFile, RecordingSession},
        native::{NativeRecorder, RecordingOptions},
        pointer_timeline::PointerTimeline,
        scene::SceneStyle,
        session::{RecordingController, RecordingState},
        video::{load_or_rebuild_poster, probe_media, render_clip_preview},
        viewport::{synthesize_zoom_cues, ViewportTimeline},
    },
    scene_ui::SceneSelection,
};
use gpui::{AnyWindowHandle, Context, PathPromptOptions};
use std::{
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

impl Studio {
    pub(super) fn displayed_recording_elapsed(&self) -> Duration {
        self.recording_elapsed
            + self
                .recording_started_at
                .map(|started| started.elapsed())
                .unwrap_or_default()
    }

    pub(super) fn recording_timecode(&self) -> String {
        let total_seconds = self.displayed_recording_elapsed().as_secs();
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;
        if hours > 0 {
            format!("{hours:02}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes:02}:{seconds:02}")
        }
    }

    pub(super) fn freeze_recording_clock(&mut self) {
        if let Some(started) = self.recording_started_at.take() {
            self.recording_elapsed += started.elapsed();
        }
    }

    pub(super) fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording_state != RecordingState::Idle || self.recording_busy {
            return;
        }
        // Starting a new capture from an open video project must stop its
        // synchronized playback before capture becomes the active media clock.
        self.pause_video_playback();
        self.recording_busy = true;
        self.recording_state = RecordingState::Starting;
        self.recording_elapsed = Duration::ZERO;
        self.recording_started_at = None;
        self.toast = Some("Choose a screen or window to record…".into());
        let options = RecordingOptions {
            system_audio: self.record_system_audio,
            microphone: self.record_microphone,
            microphone_device: self.microphone_device.clone(),
            camera: self.record_camera,
            camera_device: self.camera_device.clone(),
        };
        // The recorder opens the webcam itself; release the preview's handle.
        self.camera_preview = None;
        self.recording_camera_enabled = options.camera;
        let camera_frames = self.camera_frames.clone();
        let task = cx.background_executor().spawn(async move {
            let mut controller = RecordingController::new(
                NativeRecorder::with_options(options).with_camera_preview(camera_frames),
            );
            let result = controller
                .start()
                .map(|session| session.directory.clone())
                .map_err(|error| error.to_string());
            Ok::<_, String>((controller, result))
        });
        cx.spawn(async move |weak, cx| {
            let outcome = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.recording_busy = false;
                match outcome {
                    Ok((controller, Ok(path))) => {
                        this.recording_controller = Some(controller);
                        this.recording_state = RecordingState::Recording;
                        this.recording_started_at = Some(Instant::now());
                        this.recording_session_path = Some(path);
                        this.toast = Some(
                            format!("Recording with {}", NativeRecorder::description()).into(),
                        );
                    }
                    Ok((controller, Err(error))) => {
                        this.recording_state = controller.state();
                        this.recording_controller = None;
                        this.toast = Some(format!("Recording could not start: {error}").into());
                    }
                    Err(error) => {
                        this.recording_state = RecordingState::Idle;
                        this.toast = Some(error.into());
                    }
                }
                this.sync_camera_preview(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn run_recording_action(&mut self, action: RecordingAction, cx: &mut Context<Self>) {
        if self.recording_busy {
            return;
        }
        let Some(mut controller) = self.recording_controller.take() else {
            self.toast = Some("There is no active recording".into());
            return;
        };
        self.recording_busy = true;
        self.freeze_recording_clock();
        if matches!(action, RecordingAction::Stop | RecordingAction::Discard) {
            self.recording_state = RecordingState::Finishing;
        }
        let task = cx.background_executor().spawn(async move {
            let result = match action {
                RecordingAction::Pause => controller.pause().map(|_| None),
                RecordingAction::Resume => controller.resume().map(|_| None),
                RecordingAction::Restart => controller
                    .restart()
                    .map(|session| Some(session.directory.clone())),
                RecordingAction::Stop => controller
                    .stop_and_save()
                    .map(|session| Some(session.directory)),
                RecordingAction::Discard => controller.discard().map(|_| None),
            }
            .map_err(|error| error.to_string());
            let warnings = controller.take_warnings();
            (controller, result, warnings)
        });
        cx.spawn(async move |weak, cx| {
            let (controller, result, warnings) = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.recording_busy = false;
                this.recording_state = controller.state();
                match result {
                    Ok(path) => match action {
                        RecordingAction::Pause => {
                            this.toast = Some("Recording paused".into());
                        }
                        RecordingAction::Resume => {
                            this.recording_started_at = Some(Instant::now());
                            this.toast = Some("Recording resumed".into());
                        }
                        RecordingAction::Restart => {
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = Some(Instant::now());
                            this.recording_session_path = path;
                            this.toast = Some("Recording restarted".into());
                        }
                        RecordingAction::Stop => {
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = None;
                            this.recording_session_path = path.clone();
                            this.launcher_active = false;
                            this.toast = path.and_then(|path| {
                                match this.open_video_project(path.clone()) {
                                    Ok(()) => {
                                        let mut message =
                                            format!("Recording saved to {}", path.display());
                                        if !warnings.is_empty() {
                                            message.push_str(&format!(" — {}", warnings.join(" ")));
                                        }
                                        Some(message.into())
                                    }
                                    Err(error) => Some(
                                        format!(
                                            "Recording saved to {}, but Studio could not open it: {error}",
                                            path.display()
                                        )
                                        .into(),
                                    ),
                                }
                            });
                        }
                        RecordingAction::Discard => {
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = None;
                            this.recording_session_path = None;
                            this.toast = Some("Recording discarded".into());
                        }
                    },
                    Err(error) => {
                        if controller.state() == RecordingState::Recording {
                            this.recording_started_at = Some(Instant::now());
                        }
                        this.toast = Some(format!("Recording action failed: {error}").into());
                    }
                }
                if controller.state() == RecordingState::Idle {
                    this.recording_controller = None;
                } else {
                    this.recording_controller = Some(controller);
                }
                this.sync_camera_preview(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn request_window_close(
        &mut self,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        if self.recording_state == RecordingState::Idle {
            let _ = window_handle.update(cx, |_, window, _| window.remove_window());
            return;
        }
        if self.recording_busy {
            self.toast = Some("Wait for the current recording operation to finish".into());
            cx.notify();
            return;
        }
        let Some(mut controller) = self.recording_controller.take() else {
            self.toast =
                Some("Could not safely close: the recording controller is unavailable".into());
            cx.notify();
            return;
        };
        self.recording_busy = true;
        self.recording_state = RecordingState::Finishing;
        self.freeze_recording_clock();
        self.toast = Some("Saving the recording before closing…".into());
        let task = cx.background_executor().spawn(async move {
            let result = controller
                .stop_and_save()
                .map(|session| session.directory)
                .map_err(|error| error.to_string());
            let warnings = controller.take_warnings();
            (controller, result, warnings)
        });
        cx.spawn(async move |weak, cx| {
            let (controller, result, warnings) = task.await;
            let close = result.is_ok();
            let _ = weak.update(cx, |this, cx| {
                this.recording_busy = false;
                this.recording_state = controller.state();
                match result {
                    Ok(path) => {
                        this.recording_session_path = Some(path);
                        this.recording_controller = None;
                        for warning in warnings {
                            eprintln!("Recording finalized with warning: {warning}");
                        }
                    }
                    Err(error) => {
                        this.toast = Some(
                            format!("Could not safely close; recording was preserved: {error}")
                                .into(),
                        );
                        this.recording_controller = Some(controller);
                    }
                }
                cx.notify();
            });
            if close {
                let _ = window_handle.update(cx, |_, window, _| window.remove_window());
            }
        })
        .detach();
    }

    /// Returns to the screenshot studio. Unsaved timeline edits stay in the
    /// project's draft file, so reopening the recording restores them.
    pub(super) fn close_video_editor(&mut self, cx: &mut Context<Self>) {
        self.pause_video_playback();
        self.autosave_scene_style();
        self.leave_video_annotations();
        self.video_preview_render_generation += 1;
        self.video_edit_busy = false;
        self.video_speed_draft = None;
        self.last_video_project = self.video_project.take().map(|session| session.directory);
        self.sync_camera_preview(cx);
        let frame = self.video_frame.take();
        self.retire_image(frame);
        self.video_preview_path = None;
        self.video_undo_stack.clear();
        self.video_redo_stack.clear();
        self.video_selected_clip = None;
        self.video_selected_zoom_cue = None;
        self.video_camera_path = None;
        self.camera_frame_rgba = None;
        // Recording motion must not leak into a later screenshot animation.
        self.video_zoom_cues.clear();
        self.video_viewport_timeline = ViewportTimeline::default();
        self.video_pointer_timeline = PointerTimeline::default();
        self.video_duration = 0.0;
        self.video_source_duration = 0.0;
        self.video_clip_timeline = RecordingClipTimeline::default();
        self.toast = None;
        cx.notify();
    }

    pub(super) fn open_video_project_dialog(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open recording".into()),
        });
        cx.spawn(async move |weak, cx| {
            let selected = match prompt.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = selected else {
                return;
            };
            let _ = weak.update(cx, |this, cx| {
                this.pause_video_playback();
                match this.open_video_project(path.clone()) {
                    Ok(()) => this.toast = None,
                    Err(error) => {
                        this.toast =
                            Some(format!("Could not open {}: {error}", path.display()).into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_video_project(&mut self, directory: PathBuf) -> Result<(), String> {
        let session = RecordingSession { directory };
        let mut manifest = session
            .read_manifest()
            .map_err(|error| format!("Could not open recording manifest: {error}"))?;
        let media = probe_media(&session.screen_path()).ok();
        if let Some(media) = media.as_ref() {
            if manifest.pixel_width != media.width
                || manifest.pixel_height != media.height
                || (manifest.duration - media.duration).abs() > 0.001
            {
                manifest.pixel_width = media.width;
                manifest.pixel_height = media.height;
                manifest.duration = media.duration;
                session
                    .write_manifest(&manifest)
                    .map_err(|error| format!("Could not repair recording manifest: {error}"))?;
            }
        }
        // A poster is a disposable cache, never a project validity
        // requirement. Repair it or decode directly from the master.
        let poster =
            load_or_rebuild_poster(&session.screen_path(), &session.poster_path(), 1280, 720)
                .map_err(|error| format!("Could not decode recording preview: {error}"))?;
        self.video_playback_generation
            .fetch_add(1, Ordering::SeqCst);
        // Drop any preview render still running for the previous project.
        self.video_preview_render_generation += 1;
        let source_duration = media
            .as_ref()
            .map(|media| media.duration)
            .unwrap_or(manifest.duration)
            .max(0.0);
        let clip_timeline = session
            .effective_clip_timeline(source_duration)
            .map_err(|error| format!("Could not load recording edits: {error}"))?;
        let pointer_capture = session.read_pointer_capture().unwrap_or_default();
        let saved_style = session
            .read_edit_field::<SceneStyle>("scene")
            .ok()
            .flatten();
        let pointer_timeline = PointerTimeline::build_with_clip_timeline(
            pointer_capture.clone(),
            source_duration,
            manifest.pixel_width as f64,
            manifest.pixel_height as f64,
            saved_style
                .as_ref()
                .map(|style| style.pointer)
                .unwrap_or_default()
                .timeline_options(),
            Some(&clip_timeline),
        );
        let generated_zoom_cues = synthesize_zoom_cues(&pointer_capture, source_duration);
        let zoom_cues = session
            .effective_zoom_cues()
            .map_err(|error| format!("Could not load zoom edits: {error}"))?
            .unwrap_or(generated_zoom_cues);
        let viewport_timeline = ViewportTimeline::build(
            &zoom_cues,
            &pointer_timeline,
            &clip_timeline,
            &pointer_capture,
        );
        let saved_extras = session
            .read_edit_field::<RecordingExtras>("lahzaExtras")
            .ok()
            .flatten();
        let preview_path = session.directory.join(".edit-preview.mkv");
        let edited_preview = if clip_timeline.is_unedited(source_duration) {
            None
        } else {
            let noise_reduction = saved_extras
                .as_ref()
                .is_some_and(|extras| extras.noise_reduction);
            let source = Self::media_source_for(&session, noise_reduction);
            render_clip_preview(&source, &preview_path, &clip_timeline)
                .map_err(|error| format!("Could not build edited preview: {error}"))?;
            Some(preview_path)
        };
        if self.animation_active {
            self.exit_animation();
        }
        self.video_source_size = (manifest.pixel_width.max(1), manifest.pixel_height.max(1));
        self.motion_pick = MotionPick::Focus;
        if self.video_project.is_none() {
            self.screenshot_annotations = AnnotationWorkspace {
                marks: std::mem::take(&mut self.annotations),
                undo: std::mem::take(&mut self.undo_stack),
                redo: std::mem::take(&mut self.redo_stack),
            };
        }
        self.video_project = Some(session);
        self.set_video_frame(poster);
        self.video_pointer_timeline = pointer_timeline;
        self.video_viewport_timeline = viewport_timeline;
        self.video_pointer_synthesized = manifest.pointer_synthesized;
        self.video_source_duration = source_duration;
        self.video_duration = clip_timeline.duration();
        self.video_position = 0.0;
        self.video_playing = false;
        self.video_edit_busy = false;
        self.video_selected_clip = clip_timeline.segments.first().map(|clip| clip.id);
        self.video_clip_timeline = clip_timeline;
        self.video_undo_stack.clear();
        self.video_redo_stack.clear();
        self.video_preview_path = edited_preview;
        self.video_seek_drag = None;
        self.video_trim_drag = None;
        self.video_move_drag = None;
        self.video_zoom_cues = zoom_cues;
        self.video_selected_zoom_cue = None;
        self.video_zoom_drag = None;
        self.video_timeline_zoom = 1.0;
        self.video_timeline_scroll = 0.0;
        // Scene settings and Lahza extras saved with this project.
        let session = self.video_project.clone().expect("project was just opened");
        let saved_annotations = session
            .read_edit_field::<Vec<AnnotationMark>>("annotations")
            .ok()
            .flatten()
            .unwrap_or_default();
        self.video_press_times = pointer_capture
            .presses
            .iter()
            .filter(|press| press.phase == recording::model::PressPhase::Down)
            .map(|press| press.time)
            .collect();
        if let Some(style) = saved_style.as_ref() {
            self.apply_scene_style(style);
        }
        self.persisted_scene_style = saved_style;
        let extras = saved_extras.clone().unwrap_or_default();
        self.video_audio_muted = extras.audio_muted;
        self.video_noise_reduction = extras.noise_reduction;
        self.video_removed_presses = extras.removed_press_times;
        self.persisted_extras = saved_extras;
        self.enter_video_annotations(saved_annotations);
        self.video_selected_press = None;
        self.video_audio_levels.clear();
        let thumbnails = self.video_thumbnails.drain(..).collect::<Vec<_>>();
        self.retired_images.extend(thumbnails);
        self.video_extras_pending = true;
        let camera_path = session.camera_path();
        self.video_camera_path = camera_path.is_file().then_some(camera_path);
        self.camera_frame_rgba = None;
        self.camera_decoded_time = -1.0;
        self.scene_selection = SceneSelection::Scene;
        self.media_drag = None;
        let previous = std::mem::take(&mut self.preview_cache).frame;
        self.retire_image(previous.map(|(_, image)| image));
        if !self.video_removed_presses.is_empty() {
            self.rebuild_video_motion_timelines();
        }
        Ok(())
    }

    pub(super) fn finish_capture_request(&mut self, result: Result<PathBuf, String>) {
        self.capturing = false;
        match result {
            Ok(path) => {
                self.launcher_active = false;
                self.captured_dimensions = image::image_dimensions(&path).ok();
                let image = self.displayed_capture_image.take();
                self.retired_images.extend(image);
                self.capture_rgba = None;
                if let Ok(image) = image::open(&path) {
                    self.set_capture_image(image.to_rgba8());
                }
                self.scene_selection = SceneSelection::Scene;
                self.media_drag = None;
                self.captured_path = Some(path);
                self.processed_capture_path = None;
                self.annotations.clear();
                self.undo_stack.clear();
                self.redo_stack.clear();
                self.crop_undo_stack.clear();
                self.crop_redo_stack.clear();
                self.crop_active = false;
                self.crop_rect = CropRect::UNIT;
                self.annotation_draft = None;
                self.selected_annotation = None;
                // A new capture starts static; its motion regions start fresh.
                if self.animation_active {
                    self.exit_animation();
                }
                self.video_zoom_cues.clear();
                self.animation_preset = None;
                let scenes = self.image_scenes.drain(..).map(|scene| scene.render);
                self.retired_images.extend(scenes);
                self.image_scene_index = 0;
                self.walkthrough_stops.clear();
                self.walkthrough_mode = false;
                self.animation_pointer_capture = PointerCaptureFile::default();
                self.video_pointer_timeline = PointerTimeline::default();
                self.toast = Some("Screenshot captured — editing controls are active".into());
            }
            Err(error) => {
                self.toast = Some(format!("Capture failed or was cancelled: {error}").into());
            }
        }
    }

    pub(super) fn begin_screen_capture(&mut self, cx: &mut Context<Self>) {
        if self.capturing {
            return;
        }
        self.capturing = true;
        self.toast = Some("Choose a screen, window, or area in the system picker".into());
        cx.notify();
        let window_handle = cx.active_window();
        cx.spawn(async move |weak, cx| {
            let result = capture_behind_window(window_handle, cx).await;
            weak.update(cx, |this, cx| {
                this.finish_capture_request(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
