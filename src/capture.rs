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
        video::{load_or_rebuild_poster, probe_media},
        viewport::{synthesize_zoom_cues, ViewportTimeline},
    },
    scene_ui::SceneSelection,
};
use gpui::{AnyWindowHandle, Context, PathPromptOptions};
use std::{
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

/// Disk access, probing, decoding, and timeline construction happen off the UI thread.
pub(crate) struct PreparedVideoProject {
    session: RecordingSession,
    poster: image::RgbaImage,
    source_duration: f64,
    frame_rate: f64,
    recording_duration: f64,
    clip_timeline: RecordingClipTimeline,
    media_bank: Option<recording::media_bank::MediaBank>,
    pointer_capture: PointerCaptureFile,
    saved_style: Option<SceneStyle>,
    source_size: (u32, u32),
    pointer_timeline: PointerTimeline,
    zoom_cues: Vec<recording::viewport::ZoomCue>,
    viewport_timeline: ViewportTimeline,
    saved_extras: Option<RecordingExtras>,
    saved_annotations: Vec<AnnotationMark>,
    audio_clips: Vec<recording::media::AudioClip>,
    transcript: Option<recording::transcript::Transcript>,
    window_capture: bool,
    pointer_synthesized: bool,
    camera_path: Option<PathBuf>,
}

#[cfg(test)]
mod recording_lifecycle_tests {
    use super::*;
    use crate::{asset_directory, open_studio_window, Assets};
    use gpui::{AppContext, Application, Timer};

    #[test]
    #[ignore = "requires an isolated X11 process; synthetic audio only"]
    fn audio_preparation_is_project_scoped_and_does_not_lock_editing() {
        let root = std::env::temp_dir().join(format!("lahza-audio-jobs-{}", uuid::Uuid::new_v4()));
        let bad = RecordingSession::create_in(&root).unwrap();
        std::fs::write(bad.screen_path(), b"invalid source for a superseded job").unwrap();
        let good = RecordingSession::create_in(&root).unwrap();
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=blue:s=64x48:r=10:d=0.8",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=0.8", "-c:v", "ffv1", "-c:a", "pcm_s16le"])
            .arg(good.screen_path()).status().unwrap().success());
        let prepared = PreparedVideoProject::load(good.directory.clone()).unwrap();
        Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
            let handle = open_studio_window(cx, false, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            }).unwrap();
            let owner = handle.entity(cx).unwrap();
            cx.spawn(async move |cx| {
                owner.update(cx, |studio, cx| {
                    studio.video_project = Some(bad);
                    studio.set_video_noise_reduction(true, cx);
                    let superseded = studio.video_audio_job.as_ref().unwrap().control.clone();
                    studio.apply_video_project(prepared);
                    assert!(superseded.is_cancelled());
                    studio.set_video_noise_reduction(true, cx);
                    assert!(!studio.video_edit_busy, "audio work must leave timeline editing available");
                    studio.toggle_timeline_playback(cx);
                    assert!(studio.video_audio_play_when_ready);
                    studio.toggle_timeline_playback(cx);
                    assert!(!studio.video_audio_play_when_ready, "Pause must cancel queued playback");
                    studio.toggle_timeline_playback(cx);
                    assert!(studio.video_audio_play_when_ready);
                    studio.start_loading(crate::loading::LoadRequest::Image(
                        good.directory.join("missing-image.png")), false, cx);
                    assert!(!studio.video_audio_play_when_ready,
                        "opening another document must cancel the old project's queued playback");
                }).unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    Timer::after(Duration::from_millis(30)).await;
                    let ready = owner.update(cx, |studio, _| {
                        assert_eq!(studio.video_project.as_ref(), Some(&good));
                        assert!(studio.video_noise_reduction, "stale failure changed the new project's setting");
                        assert!(!studio.video_edit_busy);
                        assert!(!studio.video_playing, "old audio started behind the loading screen");
                        studio.video_audio_job.is_none()
                    }).unwrap();
                    if ready { break; }
                    assert!(Instant::now() < deadline, "audio preparation did not finish");
                }
                owner.update(cx, |studio, cx| {
                    assert!(studio.video_audio_error.is_none(), "{:?}", studio.video_audio_error);
                    assert!(recording::audio_processing::source_is_prepared(&good.screen_path(), true));
                    // Closing the editor must invalidate a new pending job,
                    // without changing unrelated busy/error state afterward.
                    studio.set_video_noise_reduction(false, cx);
                    let closing = studio.video_audio_job.as_ref().map(|job| job.control.clone());
                    studio.close_video_editor(cx);
                    if let Some(closing) = closing { assert!(closing.is_cancelled()); }
                }).unwrap();
                Timer::after(Duration::from_millis(150)).await;
                owner.update(cx, |studio, _| {
                    assert!(studio.video_project.is_none());
                    assert!(studio.video_audio_job.is_none());
                    assert!(studio.video_audio_error.is_none());
                    assert!(!studio.video_edit_busy);
                }).unwrap();
                let window = owner.update(cx, |studio, _| studio.window_handle).unwrap();
                let _ = window.update(cx, |_, window, _| window.remove_window());
                std::fs::remove_dir_all(root).unwrap();
                cx.update(|cx| cx.quit()).unwrap();
            }).detach();
        });
    }

    #[test]
    #[ignore = "requires a native display (run under Xvfb)"]
    fn external_stop_saves_recording_and_paused_takes_even_when_launcher_is_minimized() {
        let root = std::env::temp_dir().join(format!("lahza-external-stop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let sample = root.join("sample.mkv");
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=blue:s=64x48:r=10", "-t", "0.3", "-c:v", "libx264"])
            .arg(&sample).status().unwrap().success());
        Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
            let handle = open_studio_window(cx, false, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            }).unwrap();
            // The launcher is replaced by a larger editor window after save;
            // the same Studio entity owns both windows and its clock task.
            let owner = handle.entity(cx).unwrap();
            cx.spawn(async move |cx| {
                for state in [RecordingState::Recording, RecordingState::Paused] {
                    let session = RecordingSession::create_in(&root).unwrap();
                    std::fs::copy(&sample, session.screen_path()).unwrap();
                    let project = session.directory.clone();
                    owner.update(cx, |studio, _| {
                        let backend = NativeRecorder::completed_for_test(session.screen_path(), Duration::from_millis(300));
                        studio.recording_controller = Some(RecordingController::from_started_backend_for_test(backend, session));
                        studio.recording_state = state;
                        studio.recording_window_minimized = true;
                        studio.recording_busy = false;
                        studio.recording_elapsed = Duration::from_millis(300);
                        studio.recording_started_at = (state == RecordingState::Recording).then(Instant::now);
                        studio.recording_session_path = Some(project.clone());
                        studio.launcher_active = true;
                    }).unwrap();

                    // Exercise the actual recurring clock task, without calling
                    // Stop or the reconciliation method from the test.
                    let mut finished = false;
                    for _ in 0..120 {
                        Timer::after(Duration::from_millis(50)).await;
                        finished = owner.update(cx, |studio, _| {
                            studio.recording_state == RecordingState::Idle
                                && studio.video_project.as_ref().is_some_and(|session| session.directory == project)
                        }).unwrap();
                        if finished { break; }
                    }
                    assert!(finished, "the external stop must save and open the take from {state:?}");
                    owner.update(cx, |studio, _| {
                        assert!(!studio.recording_busy);
                        assert!(studio.recording_controller.is_none());
                        assert!(studio.recording_started_at.is_none());
                        assert_eq!(studio.recording_elapsed, Duration::ZERO);
                        assert!(!studio.recording_window_minimized);
                        assert!(!studio.launcher_active);
                        assert!(studio.recording_pill.is_none());
                        assert!(studio.recording_camera_window.is_none());
                    }).unwrap();
                    assert!(project.join("capture.json").exists());
                }
                let current_window = owner.update(cx, |studio, _| studio.window_handle).unwrap();
                let _ = current_window.update(cx, |_, window, _| window.remove_window());
                std::fs::remove_dir_all(root).unwrap();
                cx.update(|cx| cx.quit()).unwrap();
            }).detach();
        });
    }
}

impl PreparedVideoProject {
    pub(crate) fn load(directory: PathBuf) -> Result<Self, String> {
        let session = RecordingSession { directory };
        session.recover_interrupted_capture()
            .map_err(|error| format!("Could not recover recording: {error}"))?;
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
        let source = recording::media::source_path(&session).map_err(|error| error.to_string())?;
        let source_info = probe_media(&source).map_err(|error| error.to_string())?;
        let media_bank=recording::media_bank::MediaBank::load(&source).map_err(|error|error.to_string())?;
        let audio_clips = session.read_edit_field::<Vec<recording::media::AudioClip>>("audioClips")
            .map_err(|error| error.to_string())?.unwrap_or_default();
        for clip in &audio_clips {
            clip.validate().map_err(|error| error.to_string())?;
            recording::media::asset_path(&session, &clip.file).map_err(|error| error.to_string())?;
        }
        let source_duration = source_info.duration.max(0.0);
        let mut clip_timeline = session
            .effective_clip_timeline(source_duration)
            .map_err(|error| format!("Could not load recording edits: {error}"))?;
        for clip in &mut clip_timeline.segments {
            if clip.source_bounds.is_none() && clip.source_end <= manifest.duration {
                clip.source_bounds = Some([0.0, manifest.duration]);
            }
        }
        let pointer_capture = session.read_pointer_capture().unwrap_or_default();
        let saved_style = session
            .read_edit_field::<SceneStyle>("scene")
            .ok()
            .flatten();
        let source_size = if media.as_ref().is_some_and(|media| media.window_capture) {
            poster.dimensions()
        } else { (manifest.pixel_width.max(1), manifest.pixel_height.max(1)) };
        let mut pointer_timeline = PointerTimeline::build_with_clip_timeline(
            pointer_capture.clone(),
            source_duration,
            source_size.0 as f64,
            source_size.1 as f64,
            saved_style
                .as_ref()
                .map(|style| style.pointer)
                .unwrap_or_default()
                .timeline_options(),
            Some(&clip_timeline),
        );
        pointer_timeline.restrict_to_recording(&clip_timeline, manifest.duration);
        let generated_zoom_cues = synthesize_zoom_cues(&pointer_capture, manifest.duration);
        let zoom_cues = session
            .effective_zoom_cues()
            .map_err(|error| format!("Could not load zoom edits: {error}"))?
            .unwrap_or(generated_zoom_cues);
        let viewport_timeline = ViewportTimeline::build_for_viewport(
            &saved_style.as_ref().map(|style|style.animation).unwrap_or_default().cues(&zoom_cues),
            &pointer_timeline,
            &clip_timeline,
            &pointer_capture,
            saved_style.as_ref().cloned().unwrap_or_default().focus_bounds(source_size.0, source_size.1),
        );
        let saved_extras = session
            .read_edit_field::<RecordingExtras>("lahzaExtras")
            .ok()
            .flatten();
        let saved_annotations = session
            .read_edit_field::<Vec<AnnotationMark>>("annotations")
            .ok()
            .flatten()
            .unwrap_or_default();
        let camera_path = session.camera_path();
        let camera_path = camera_path.is_file().then_some(camera_path);
        // A damaged transcript must not block opening the recording.
        let transcript = session
            .read_edit_field::<recording::transcript::Transcript>("transcript")
            .ok()
            .flatten()
            .filter(|transcript| !transcript.words.is_empty() || !transcript.cues.is_empty());
        Ok(Self {
            frame_rate: source_info.frame_rate,
            window_capture: source_info.window_capture,
            recording_duration: manifest.duration,
            pointer_synthesized: manifest.pointer_synthesized,
            session, poster, source_duration, clip_timeline, media_bank, pointer_capture, saved_style,
            source_size, pointer_timeline, zoom_cues, viewport_timeline, saved_extras,
            saved_annotations, camera_path, audio_clips, transcript,
        })
    }
}

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

    /// A portal session can end without any Lahza button being pressed. Keep
    /// checking while minimized and paused, and use the same save path as Stop.
    pub(super) fn reconcile_recording_lifecycle(&mut self, cx: &mut Context<Self>) {
        if !self.recording_busy
            && matches!(self.recording_state, RecordingState::Recording | RecordingState::Paused)
            && self.recording_controller.as_ref().is_some_and(|controller| controller.has_stopped())
        {
            self.run_recording_action(RecordingAction::Stop, cx);
            cx.notify();
        }
    }

    pub(crate) fn restore_recording_window(&mut self, cx: &mut Context<Self>) {
        self.recording_window_minimized = false;
        let handle = self.window_handle;
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        });
    }

    pub(super) fn start_recording(&mut self, cx: &mut Context<Self>) {
        self.ensure_global_shortcuts(cx);
        if self.recording_state != RecordingState::Idle || self.recording_busy {
            return;
        }
        self.request_capture_access(crate::capture_access::CaptureAccess::Recording, cx);
    }

    pub(super) fn start_recording_with_access(&mut self, cx: &mut Context<Self>) {
        if self.recording_state != RecordingState::Idle || self.recording_busy {
            return;
        }
        let prompter_exclusion = match self.open_prompter(cx) {
            Ok(exclusion) => exclusion,
            Err(error) => {
                self.toast = Some(crate::Notification::error(error));
                cx.notify();
                return;
            }
        };
        // Starting a new capture from an open video project must stop its
        // synchronized playback before capture becomes the active media clock.
        self.pause_video_playback();
        self.recording_busy = true;
        self.recording_state = RecordingState::Starting;
        self.recording_elapsed = Duration::ZERO;
        self.recording_started_at = None;
        self.toast = Some(
            if self.record_area {
                "Choose a screen, then select the area to record…"
            } else {
                "Choose a screen or window to record…"
            }
            .into(),
        );
        let startup = Arc::new(recording::native::RecordingStartup::default());
        self.recording_startup = Some(startup.clone());
        let mut options = RecordingOptions {
            source: if self.record_area { recording::native::RecordingSource::Screen } else { self.recording_source },
            countdown_seconds: self.recording_countdown,
            area: None,
            capture_exclusions: prompter_exclusion.into_iter().collect(),
            system_audio: self.record_system_audio,
            microphone: self.record_microphone,
            microphone_device: self.microphone_device.clone(),
            camera: self.record_camera,
            camera_device: self.camera_device.clone(),
        };
        // The recorder opens the webcam itself; release the preview's handle.
        self.camera_preview = None;
        self.recording_camera_enabled = options.camera;
        let area_requests = self.record_area.then(|| self.area_selection_requests(cx));
        let camera_frames = self.camera_frames.clone();
        // Optional recording chrome must not make the GNOME helper a new
        // prerequisite for basic capture. Map and verify every overlay before
        // opening the source picker; otherwise use the existing launcher.
        cx.spawn(async move |weak, cx| {
            // Open after the initiating event releases its window borrow,
            // so overlay placement can query the launcher's actual display.
            let Ok((overlay_exclusions, opening_error)) = weak.update(cx, |this, cx| {
                match this.open_recording_overlays(cx) {
                    Ok(exclusions) => (exclusions, None),
                    Err(error) => {
                        this.close_recording_overlays(cx);
                        (Vec::new(), Some(error))
                    }
                }
            }) else { return };
            let protection = cx.background_executor().spawn(async move {
                let readiness = if let Some(error) = opening_error {
                    Err((error, "Floating windows could not open"))
                } else if overlay_exclusions.is_empty() {
                    Err((
                        "No recording overlay protection requests were created".into(),
                        "Floating windows could not open",
                    ))
                } else {
                    recording::capture_exclusion::CaptureExclusion::wait_until_all_protected(
                        &overlay_exclusions,
                        Duration::from_secs(3),
                    )
                    .map_err(|error| (error, "Capture Helper did not confirm protection"))
                };
                (overlay_exclusions, readiness)
            });
            let (overlay_exclusions, readiness) = protection.await;
            let Ok(overlay_failure) = weak.update(cx, |this, cx| {
                // A protection watchdog may have closed the windows while
                // the background wait completed. Only a visible, protected
                // pill can replace the launcher's recording controls.
                let pill_open = this.recording_pill.is_some();
                let camera_open = this.recording_camera_window.is_some();
                let protection_states: Vec<_> = overlay_exclusions
                    .iter()
                    .map(|request| (request, request.is_protected()))
                    .collect();
                let readiness = readiness.and_then(|()| {
                    if !pill_open || (this.record_camera && !camera_open) {
                        Err((
                            "A recording overlay closed while waiting for protection".into(),
                            "Floating windows closed during setup",
                        ))
                    } else if protection_states.iter().any(|(_, protected)| !protected) {
                        Err((
                            "Recording overlay protection was lost before capture setup".into(),
                            "Capture Helper lost protection",
                        ))
                    } else {
                        Ok(())
                    }
                });
                let failure = if let Err((error, summary)) = readiness {
                    eprintln!(
                        "Floating recording controls unavailable: {error}; controls_open={pill_open}, camera_requested={}, camera_open={camera_open}",
                        this.record_camera,
                    );
                    for (request, protected) in protection_states {
                        eprintln!(
                            "Recording overlay {:?}: protected={protected}, previously_protected={}",
                            request.title(),
                            request.has_been_protected(),
                        );
                    }
                    this.close_recording_overlays(cx);
                    Some(summary)
                } else {
                    this.recording_window_minimized = true;
                    let handle = this.window_handle;
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, _| window.minimize_window());
                    });
                    None
                };
                cx.notify();
                failure
            }) else {
                return;
            };
            if overlay_failure.is_none() {
                options.capture_exclusions.extend(overlay_exclusions);
            }
            // Let the compositor finish hiding the launcher or removing
            // unprotected overlays before any screen frames can be recorded.
            gpui::Timer::after(Duration::from_millis(400)).await;
            let task = cx.background_executor().spawn(async move {
                let mut recorder =
                    NativeRecorder::with_options(options).with_camera_preview(camera_frames).with_startup(startup);
                if let Some(requests) = area_requests {
                    recorder = recorder.with_area_selection(requests);
                }
                let mut controller = RecordingController::new(recorder);
                let result = controller
                    .start()
                    .map(|session| session.directory.clone())
                    .map_err(|error| error.to_string());
                Ok::<_, String>((controller, result))
            });
            let outcome = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.recording_busy = false;
                let cancelled = this.recording_startup.take().is_some_and(|startup| startup.is_cancelled());
                match outcome {
                    Ok((controller, Ok(_))) if cancelled => {
                        this.recording_controller = Some(controller);
                        this.run_recording_action(RecordingAction::Discard, cx);
                        cx.notify();
                        return;
                    }
                    Ok((controller, Ok(path))) => {
                        this.position_recording_overlays(controller.source_geometry(), cx);
                        this.recording_controller = Some(controller);
                        this.recording_state = RecordingState::Recording;
                        this.recording_started_at = Some(Instant::now());
                        this.recording_session_path = Some(path);
                        let mut message = format!("Recording with {}", NativeRecorder::description());
                        if let Some(reason) = overlay_failure {
                            message.push_str(&format!(" · Floating controls unavailable: {reason}; use the controls here"));
                        }
                        this.toast = Some(message.into());
                        // Discard speech heard during source selection while
                        // retaining the already-protected native window.
                        this.reset_prompter(cx);
                    }
                    Ok((controller, Err(error))) => {
                        this.close_prompter(cx);
                        this.close_recording_overlays(cx);
                        this.restore_recording_window(cx);
                        this.recording_state = controller.state();
                        this.recording_controller = None;
                        this.toast = Some(if cancelled { "Recording cancelled".into() } else { crate::Notification::error(format!("Recording could not start: {error}")) });
                    }
                    Err(error) => {
                        this.close_prompter(cx);
                        this.close_recording_overlays(cx);
                        this.restore_recording_window(cx);
                        this.recording_state = RecordingState::Idle;
                        this.toast = Some(crate::Notification::error(error));
                    }
                }
                if this.recording_close_requested && this.recording_state == RecordingState::Idle {
                    this.recording_close_requested = false;
                    let handle = this.window_handle;
                    cx.defer(move |cx| { let _ = handle.update(cx, |_, window, _| window.remove_window()); });
                } else {
                    this.sync_camera_preview(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// New captures use cinematic motion; existing projects keep their authored cues.
    fn save_recording_appearance(&self, directory: &std::path::Path) {
        let session = RecordingSession { directory: directory.to_owned() };
        let mut style = self.scene_style();
        style.camera.enabled &= self.recording_camera_enabled
            && session.read_manifest().is_ok_and(|manifest| manifest.includes_camera);
        let save = || -> std::io::Result<()> {
            session.write_edit_field("scene", &style)?;
            if session.effective_zoom_cues()?.is_none() {
                let capture = session.read_pointer_capture()?;
                let duration = session.read_manifest()?.duration;
                let cues = recording::viewport::synthesize_zoom_cues_with_style(
                    &capture, duration, recording::viewport::AutoZoomStyle::Cinematic,
                );
                session.write_edit_field("zoomCues", &cues)?;
            }
            Ok(())
        };
        if let Err(error) = save() { eprintln!("Could not save recording appearance: {error}"); }
    }

    pub(crate) fn recording_start_label(&self) -> String {
        match self.recording_startup.as_ref() {
            Some(startup) if startup.is_cancelled() => "Cancelling…".into(),
            Some(startup) => startup.countdown_remaining().map(|seconds| format!("Recording in {seconds}…"))
                .unwrap_or_else(|| "Choose your recording source…".into()),
            None => "Starting…".into(),
        }
    }

    pub(crate) fn cancel_recording_start(&mut self, cx: &mut Context<Self>) {
        if let Some(startup) = self.recording_startup.as_ref() {
            startup.cancel();
            self.toast = Some("Cancelling recording setup…".into());
            cx.notify();
        }
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
        if matches!(action, RecordingAction::Restart) {
            // Preserve the protected window across takes. Replacing it would
            // require another compositor acknowledgement before encoding.
            self.pause_prompter(true, cx);
        }
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
                // The prompter belongs to a live recording only.
                if matches!(action, RecordingAction::Stop | RecordingAction::Discard)
                    || controller.state() == RecordingState::Idle
                {
                    this.close_prompter(cx);
                    this.close_recording_overlays(cx);
                    this.restore_recording_window(cx);
                }
                this.recording_state = controller.state();
                match result {
                    Ok(path) => match action {
                        RecordingAction::Pause => {
                            this.pause_prompter(true, cx);
                        }
                        RecordingAction::Resume => {
                            this.pause_prompter(false, cx);
                            this.recording_started_at = Some(Instant::now());
                        }
                        RecordingAction::Restart => {
                            this.position_recording_overlays(controller.source_geometry(), cx);
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = Some(Instant::now());
                            this.recording_session_path = path;
                            // A new take reads the script from the top.
                            this.reset_prompter(cx);
                        }
                        RecordingAction::Stop => {
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = None;
                            this.recording_session_path = path.clone();
                            this.launcher_active = false;
                            if let Some(path) = path {
                                this.save_recording_appearance(&path);
                                this.start_loading(crate::loading::LoadRequest::Recording(path.clone()), false, cx);
                                let mut message = format!("Recording saved to {}", path.display());
                                if !warnings.is_empty() { message.push_str(&format!(" — {}", warnings.join(" "))); }
                                this.toast = Some(crate::Notification::saved(message, path));
                            }
                        }
                        RecordingAction::Discard => {
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = None;
                            this.recording_session_path = None;
                            this.toast = Some("Recording discarded".into());
                        }
                    },
                    Err(error) => {
                        if controller.state() == RecordingState::Recording && !controller.has_stopped() {
                            this.recording_started_at = Some(Instant::now());
                        }
                        this.toast = Some(crate::Notification::error(if controller.state() == RecordingState::Idle {
                            let recovery = controller.session().map(|session| {
                                this.recording_session_path = Some(session.directory.clone());
                                format!(" Files were preserved at {}.", session.directory.display())
                            }).unwrap_or_default();
                            format!("Recording ended, but could not be saved: {error}.{recovery}")
                        } else {
                            format!("Recording action failed: {error}")
                        }));
                        if matches!(action, RecordingAction::Restart)
                            && matches!(
                                controller.state(),
                                RecordingState::Recording | RecordingState::Paused
                            )
                        {
                            this.pause_prompter(controller.state() == RecordingState::Paused, cx);
                        }
                    }
                }
                if controller.state() == RecordingState::Idle {
                    this.recording_controller = None;
                } else {
                    this.recording_controller = Some(controller);
                }
                if this.recording_close_requested && this.recording_state == RecordingState::Idle {
                    this.recording_close_requested = false;
                    let handle = this.window_handle;
                    cx.defer(move |cx| { let _ = handle.update(cx, |_, window, _| window.remove_window()); });
                } else {
                    this.sync_camera_preview(cx);
                }
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
        if self.recording_state == RecordingState::Starting {
            self.recording_close_requested = true;
            self.cancel_recording_start(cx);
            return;
        }
        if self.recording_busy {
            self.toast = Some("Wait for the current recording operation to finish".into());
            cx.notify();
            return;
        }
        let Some(mut controller) = self.recording_controller.take() else {
            self.toast =
                Some(crate::Notification::error("Could not safely close: the recording controller is unavailable"));
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
                        this.close_prompter(cx);
                        this.close_recording_overlays(cx);
                        this.save_recording_appearance(&path);
                        this.recording_session_path = Some(path);
                        this.recording_controller = None;
                        for warning in warnings {
                            eprintln!("Recording finalized with warning: {warning}");
                        }
                    }
                    Err(error) => {
                        if controller.state() == RecordingState::Idle {
                            this.close_prompter(cx);
                            this.close_recording_overlays(cx);
                            this.restore_recording_window(cx);
                            let recovery = controller.session().map(|session| {
                                this.recording_session_path = Some(session.directory.clone());
                                format!(" Files were preserved at {}.", session.directory.display())
                            }).unwrap_or_default();
                            this.toast = Some(crate::Notification::error(format!("Recording ended, but could not be saved: {error}.{recovery}")));
                            this.recording_controller = None;
                            this.sync_camera_preview(cx);
                        } else {
                            if controller.state() == RecordingState::Recording && !controller.has_stopped() {
                                this.recording_started_at = Some(Instant::now());
                            }
                            this.toast = Some(crate::Notification::error(format!(
                                "Could not safely close; recording was preserved: {error}"
                            )));
                            this.recording_controller = Some(controller);
                        }
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
        self.cancel_audio_preparation();
        self.clear_timeline_preview();
        self.reset_timeline_thumbnails();
        self.reset_timeline_interactions();
        self.autosave_scene_style();
        self.leave_video_annotations();
        self.video_edit_busy = false;
        self.video_speed_draft = None;
        self.last_video_project = self.video_project.take().map(|session| session.directory);
        self.video_import_token = None;
        self.sync_camera_preview(cx);
        let frame = self.video_frame.take();
        self.retire_image(frame);
        // Keep the history so switching back to this recording can still undo.
        let undo = std::mem::take(&mut self.video_undo_stack);
        let redo = std::mem::take(&mut self.video_redo_stack);
        self.parked_video_history =
            self.last_video_project.clone().map(|directory| (directory, undo, redo));
        self.video_selected_clip = None;
        self.video_selected_zoom_cue = None;
        self.video_camera_path = None;
        self.camera_regions.clear();
        self.selected_camera_region = None;
        self.camera_region_drag = None;
        self.video_audio_clips.clear();
        self.camera_frame_rgba = None;
        // Recording motion must not leak into a later screenshot animation.
        self.video_zoom_cues.clear();
        self.video_viewport_timeline = ViewportTimeline::default();
        self.video_pointer_timeline = PointerTimeline::default();
        self.video_duration = 0.0;
        self.video_source_duration = 0.0;
        self.video_clip_timeline = RecordingClipTimeline::default();
        self.video_motion_clip_timeline=RecordingClipTimeline::default();
        self.video_media_bank=None;
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
                this.start_loading(crate::loading::LoadRequest::Recording(path), false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(super) fn open_video_project(&mut self, directory: PathBuf) -> Result<(), String> {
        self.apply_video_project(PreparedVideoProject::load(directory)?);
        Ok(())
    }

    pub(crate) fn apply_video_project(&mut self, prepared: PreparedVideoProject) {
        self.pause_video_playback();
        self.cancel_audio_preparation();
        self.video_audio_prepare_pending = true;
        self.camera_regions.clear();
        self.selected_camera_region = None;
        self.camera_region_drag = None;
        self.clear_timeline_preview();
        self.reset_timeline_thumbnails();
        self.reset_timeline_interactions();
        let PreparedVideoProject {
            session, poster, source_duration, frame_rate, clip_timeline, media_bank, pointer_capture, saved_style,
            source_size, pointer_timeline, zoom_cues, viewport_timeline, saved_extras,
            saved_annotations, window_capture, pointer_synthesized, camera_path, audio_clips, recording_duration,
            transcript,
        } = prepared;
        self.video_playback_generation.fetch_add(1, Ordering::SeqCst);
        if self.animation_active {
            self.exit_animation();
        }
        self.timeline_preview.frame_rate = (frame_rate.is_finite() && frame_rate > 0.0).then_some(frame_rate);
        self.video_window_capture = window_capture;
        self.video_source_size = source_size;
        self.motion_pick = MotionPick::Focus;
        if self.video_project.is_none() {
            self.screenshot_annotations = AnnotationWorkspace {
                marks: std::mem::take(&mut self.annotations),
                undo: std::mem::take(&mut self.undo_stack),
                redo: std::mem::take(&mut self.redo_stack),
            };
        }
        self.video_project = Some(session);
        self.video_import_token = None;
        self.set_video_frame(poster);
        self.video_pointer_timeline = pointer_timeline;
        self.video_viewport_timeline = viewport_timeline;
        self.video_pointer_synthesized = pointer_synthesized;
        self.video_source_duration = source_duration;
        self.video_recording_duration = recording_duration;
        self.video_duration = clip_timeline.duration();
        self.video_position = 0.0;
        self.video_playing = false;
        self.video_edit_busy = false;
        self.video_selected_clip = clip_timeline.segments.first().map(|clip| clip.id);
        self.video_motion_clip_timeline=media_bank.as_ref().map_or_else(||clip_timeline.clone(),|bank|bank.ranges(&clip_timeline));
        self.video_media_bank=media_bank;
        self.video_clip_timeline = clip_timeline;
        let reopened = self.video_project.as_ref().map(|session| session.directory.clone());
        match self.parked_video_history.take() {
            Some((directory, undo, redo)) if Some(&directory) == reopened.as_ref() => {
                self.video_undo_stack = undo;
                self.video_redo_stack = redo;
            }
            _ => {
                self.video_undo_stack.clear();
                self.video_redo_stack.clear();
            }
        }
        self.video_seek_drag = None;
        self.video_trim_drag = None;
        self.video_move_drag = None;
        self.video_zoom_cues = zoom_cues;
        self.video_selected_zoom_cue = None;
        self.video_zoom_drag = None;
        self.video_timeline_span = self.video_duration.max(0.001);
        self.video_timeline_extent = self.video_timeline_span;
        self.video_timeline_zoom = 1.0;
        self.video_timeline_scroll = 0.0;
        // Scene settings and Lahza extras saved with this project.
        self.video_keystrokes = pointer_capture.keystrokes.clone();
        self.video_transcript = transcript;
        self.video_transcript_revision += 1;
        self.transcript_open = false;
        self.transcript_selected_cue = None;
        self.transcript_selection = None;
        self.video_overlay_cache = None;
        self.video_press_times = pointer_capture
            .presses
            .iter()
            .filter(|press| press.phase == recording::model::PressPhase::Down)
            .map(|press| press.time)
            .collect();
        self.video_crop = CropRect::UNIT;
        self.crop_active = false;
        self.crop_drag = None;
        if let Some(style) = saved_style.as_ref() {
            self.video_crop = style.source_crop.validated();
            self.apply_scene_style(style);
        }
        self.persisted_scene_style = saved_style;
        // A newly opened project starts at rest, not easing from the last one.
        self.camera_handoff = None;
        self.camera_shown = None;
        let extras = saved_extras.clone().unwrap_or_default();
        self.video_audio_muted = extras.audio_muted;
        self.video_audio_clips = audio_clips;
        self.video_audio_mode = extras.effective_audio_mode();
        self.video_noise_reduction = self.video_audio_mode.requires_neural();
        self.video_removed_presses = extras.removed_press_times;
        self.persisted_extras = saved_extras;
        self.enter_video_annotations(saved_annotations);
        self.video_selected_press = None;
        self.video_audio_levels.clear();
        let thumbnails = self.video_thumbnails.drain(..).collect::<Vec<_>>();
        self.retired_images.extend(thumbnails);
        self.video_extras_pending = true;
        self.video_camera_path = camera_path;
        if self.video_camera_path.is_none() { self.camera_overlay.enabled = false; }
        self.camera_frame_rgba = None;
        self.camera_decoded_time = -1.0;
        self.scene_selection = SceneSelection::Scene;
        self.media_drag = None;
        let previous = std::mem::take(&mut self.preview_cache).frame;
        self.retire_image(previous.map(|(_, image)| image));
        if !self.video_removed_presses.is_empty() {
            self.rebuild_video_motion_timelines();
        }
    }

    pub(super) fn finish_capture_request(&mut self, result: Result<PathBuf, String>) {
        self.capturing = false;
        match result.and_then(|path| {
            image::open(&path).map(|image| (path, image.to_rgba8()))
                .map_err(|error| format!("Could not decode screenshot: {error}"))
        }) {
            Ok((path, image)) => self.apply_capture_image(path, image),
            Err(error) => self.toast = Some(crate::Notification::error(format!("Capture failed or was cancelled: {error}"))),
        }
    }

    pub(crate) fn apply_capture_image(&mut self, path: PathBuf, pixels: image::RgbaImage) {
        self.capturing = false;
        self.launcher_active = false;
        self.captured_dimensions = Some(pixels.dimensions());
        let image = self.displayed_capture_image.take();
        self.retired_images.extend(image);
        self.set_capture_image(pixels);
        self.scene_selection = SceneSelection::Scene;
        self.media_drag = None;
        self.captured_path = Some(path);
        self.processed_capture_path = None;
        self.annotations.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.original_capture = None;
        self.source_crop = CropRect::UNIT;
        self.crop_session = None;
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
        self.animation_image_start = 0.0;
        self.animation_image_end = self.animation_duration;
        let scenes = self.image_scenes.drain(..).map(|scene| scene.render);
        self.retired_images.extend(scenes);
        self.image_scene_index = 0;
        self.walkthrough_stops.clear();
        self.walkthrough_mode = false;
        self.animation_pointer_capture = PointerCaptureFile::default();
        self.video_pointer_timeline = PointerTimeline::default();
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
