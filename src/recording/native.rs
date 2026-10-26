use super::area::{preview_frame, AreaRequest, RecordingArea};
use super::area_indicator::AreaIndicator;
use super::camera_preview::{attach_preview, preview_branch, CameraFrames};
use super::capture_exclusion::{CaptureExclusion, CAPTURE_MEMORY_CAPS};
use super::input::{monotonic_ns, ActiveRange, InputCapture, InputMapping};
use super::stream_geometry::{self, RecordingSize};
use ashpd::desktop::{
    screencast::{CursorMode, Screencast, SourceType},
    PersistMode,
};
use gst::prelude::*;
use gstreamer as gst;
use futures_util::{FutureExt, StreamExt};
use std::{
    fmt, fs, io,
    os::fd::{AsRawFd, OwnedFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub struct RecordStatus {
    pub active: bool,
    pub paused: bool,
    pub timecode: String,
    pub duration_ms: u64,
    pub output_bytes: u64,
}

/// Selected source bounds in compositor logical coordinates, not encoded pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureSourceGeometry {
    pub position: (i32, i32),
    pub size: (i32, i32),
    pub is_monitor: bool,
}

impl CaptureSourceGeometry {
    fn from_portal(
        position: Option<(i32, i32)>,
        size: Option<(i32, i32)>,
        source_type: Option<SourceType>,
    ) -> Option<Self> {
        let size = size.filter(|(width, height)| *width > 0 && *height > 0)?;
        Some(Self {
            position: position?,
            size,
            is_monitor: source_type == Some(SourceType::Monitor),
        })
    }
}

#[derive(Debug)]
pub enum RecorderError {
    Io(io::Error),
    Portal(String),
    Pipeline(String),
    State(String),
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Portal(error) => write!(f, "screen sharing failed: {error}"),
            Self::Pipeline(error) => write!(f, "video encoder failed: {error}"),
            Self::State(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RecorderError {}

impl From<io::Error> for RecorderError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub trait RecorderBackend {
    fn record_status(&mut self) -> Result<RecordStatus, RecorderError>;
    fn start_recording(&mut self, output: &Path) -> Result<(), RecorderError>;
    fn pause_recording(&mut self) -> Result<(), RecorderError>;
    fn resume_recording(&mut self) -> Result<(), RecorderError>;
    fn stop_recording(&mut self) -> Result<PathBuf, RecorderError>;

    /// Terminal shutdown has started; a paused recorder is not stopped.
    fn has_stopped(&self) -> bool {
        false
    }

    fn source_geometry(&self) -> Option<CaptureSourceGeometry> {
        None
    }

    fn includes_system_audio(&self) -> bool {
        false
    }

    fn includes_microphone(&self) -> bool {
        false
    }

    fn includes_camera(&self) -> bool {
        false
    }

    /// Recoverable capture failures reported after the screen was saved.
    fn capture_warnings(&self) -> Vec<String> {
        Vec::new()
    }

    fn pointer_synthesized(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RecordingSource {
    #[default]
    Any,
    Screen,
    Window,
}

/// Shared with the launcher before the background recorder has started.
#[derive(Default)]
pub struct RecordingStartup {
    remaining: AtomicU8,
    cancelled: AtomicBool,
    started: AtomicBool,
}

impl RecordingStartup {
    pub fn countdown_remaining(&self) -> Option<u8> {
        let remaining = self.remaining.load(Ordering::Acquire);
        (remaining > 0).then_some(remaining)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Dropping the pending portal operation lets the caller close its session
/// promptly when the launcher cancels an open source picker.
async fn during_startup<T>(
    startup: &RecordingStartup,
    operation: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if startup.is_cancelled() {
        return Err("Recording was cancelled before capture started.".into());
    }
    let cancelled = async {
        loop {
            if startup.is_cancelled() {
                return Err("Recording was cancelled before capture started.".into());
            }
            async_io::Timer::after(Duration::from_millis(50)).await;
        }
    };
    futures_util::pin_mut!(operation, cancelled);
    match futures_util::future::select(operation, cancelled).await {
        futures_util::future::Either::Left((result, _)) => result,
        futures_util::future::Either::Right((result, _)) => result,
    }
}

fn wait_for_countdown(startup: &RecordingStartup, seconds: u8, mut closed: impl FnMut() -> bool) -> Result<(), String> {
    // Restart reuses this recorder after its launcher has gone away. Only the
    // initial capture has a visible countdown and a startup cancellation UI.
    let seconds = if startup.started.load(Ordering::Acquire) { 0 } else { seconds };
    let deadline = Instant::now() + Duration::from_secs(u64::from(seconds.min(10)));
    let result = loop {
        if startup.is_cancelled() || closed() {
            break Err("Recording was cancelled before capture started.".into());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { break Ok(()); }
        startup.remaining.store(remaining.as_secs_f64().ceil() as u8, Ordering::Release);
        thread::sleep(remaining.min(Duration::from_millis(50)));
    };
    startup.remaining.store(0, Ordering::Release);
    result
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordingOptions {
    pub source: RecordingSource,
    pub countdown_seconds: u8,
    pub area: Option<RecordingArea>,
    /// Keeps every floating window's helper request alive and requires
    /// protected offscreen compositing before desktop/area recording.
    pub capture_exclusions: Vec<CaptureExclusion>,
    pub system_audio: bool,
    pub microphone: bool,
    /// PulseAudio/PipeWire source name to record from; `None` uses the default.
    pub microphone_device: Option<String>,
    /// Record a webcam alongside the screen into `camera.mkv`.
    pub camera: bool,
    /// V4L2 device node to record; `None` uses the first webcam found.
    pub camera_device: Option<String>,
}

#[derive(Clone, Debug)]
struct SharedState {
    active: bool,
    paused: bool,
    started_at: Option<Instant>,
    elapsed: Duration,
    output: Option<PathBuf>,
    failure: Option<String>,
    pointer_synthesized: bool,
    camera_recorded: bool,
    stopped: bool,
    completion: Option<Result<PathBuf, String>>,
    source_geometry: Option<CaptureSourceGeometry>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            active: false,
            paused: false,
            started_at: None,
            elapsed: Duration::ZERO,
            output: None,
            failure: None,
            pointer_synthesized: false,
            camera_recorded: false,
            stopped: false,
            completion: None,
            source_geometry: None,
        }
    }
}

enum WorkerCommand {
    Pause(mpsc::Sender<Result<(), String>>),
    Resume(mpsc::Sender<Result<(), String>>),
    Stop(mpsc::Sender<Result<PathBuf, String>>),
}

pub struct NativeRecorder {
    area_requests: Option<mpsc::Sender<AreaRequest>>,
    camera_frames: Option<Arc<CameraFrames>>,
    options: RecordingOptions,
    startup: Arc<RecordingStartup>,
    state: Arc<Mutex<SharedState>>,
    commands: Option<mpsc::Sender<WorkerCommand>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Default for NativeRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeRecorder {
    #[cfg(test)]
    pub(crate) fn completed_for_test(output: PathBuf, elapsed: Duration) -> Self {
        let recorder = Self::new();
        {
            let mut state = recorder.state.lock().unwrap();
            state.elapsed = elapsed;
            state.output = Some(output.clone());
            state.stopped = true;
            state.completion = Some(Ok(output));
        }
        recorder
    }

    pub fn new() -> Self {
        Self::with_options(RecordingOptions::default())
    }

    pub fn with_options(options: RecordingOptions) -> Self {
        Self {
            area_requests: None,
            camera_frames: None,
            options,
            startup: Arc::new(RecordingStartup::default()),
            state: Arc::new(Mutex::new(SharedState::default())),
            commands: None,
            worker: None,
        }
    }

    pub fn with_area_selection(mut self, requests: mpsc::Sender<AreaRequest>) -> Self {
        self.area_requests = Some(requests);
        self
    }

    pub fn with_camera_preview(mut self, frames: Arc<CameraFrames>) -> Self {
        self.camera_frames = Some(frames);
        self
    }

    pub fn with_startup(mut self, startup: Arc<RecordingStartup>) -> Self {
        self.startup = startup;
        self
    }

    pub fn countdown_remaining(&self) -> Option<u8> {
        self.startup.countdown_remaining()
    }

    pub fn description() -> &'static str {
        "PipeWire"
    }

    fn command<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, String>>) -> WorkerCommand,
    ) -> Result<T, RecorderError> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| RecorderError::State("there is no active native recording".into()))?;
        let (reply_tx, reply_rx) = mpsc::channel();
        commands
            .send(make(reply_tx))
            .map_err(|_| RecorderError::State("the native recorder stopped unexpectedly".into()))?;
        reply_rx
            .recv()
            .map_err(|_| RecorderError::State("the native recorder did not reply".into()))?
            .map_err(RecorderError::Pipeline)
    }
}

impl RecorderBackend for NativeRecorder {
    fn record_status(&mut self) -> Result<RecordStatus, RecorderError> {
        let state = self.state.lock().expect("native recorder state poisoned");
        if let Some(error) = &state.failure {
            return Err(RecorderError::Pipeline(error.clone()));
        }
        let elapsed = state.elapsed
            + state
                .started_at
                .filter(|_| state.active && !state.paused)
                .map(|started| started.elapsed())
                .unwrap_or_default();
        let millis = elapsed.as_millis() as u64;
        let hours = millis / 3_600_000;
        let minutes = millis / 60_000 % 60;
        let seconds = millis / 1_000 % 60;
        let fraction = millis % 1_000;
        let output_bytes = state
            .output
            .as_ref()
            .and_then(|path| path.metadata().ok())
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        Ok(RecordStatus {
            active: state.active,
            paused: state.paused,
            timecode: format!("{hours:02}:{minutes:02}:{seconds:02}.{fraction:03}"),
            duration_ms: millis,
            output_bytes,
        })
    }

    fn start_recording(&mut self, output: &Path) -> Result<(), RecorderError> {
        if self.commands.is_some() {
            return Err(RecorderError::State("a recording is already active".into()));
        }
        ensure_runtime()?;
        *self.state.lock().expect("native recorder state poisoned") = SharedState::default();
        let output = output.to_path_buf();
        let options = self.options.clone();
        let startup = self.startup.clone();
        let area_requests = self.area_requests.clone();
        let camera_frames = self.camera_frames.clone();
        let state = self.state.clone();
        let (commands_tx, commands_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("lahza-native-recorder".into())
            .spawn(move || {
                run_worker(
                    output,
                    options,
                    startup,
                    state,
                    commands_rx,
                    ready_tx,
                    camera_frames,
                    area_requests,
                )
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => {
                self.startup.started.store(true, Ordering::Release);
                self.commands = Some(commands_tx);
                self.worker = Some(worker);
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(RecorderError::Portal(error))
            }
            Err(_) => {
                let _ = worker.join();
                Err(RecorderError::Pipeline(
                    "the native recorder exited during setup".into(),
                ))
            }
        }
    }

    fn pause_recording(&mut self) -> Result<(), RecorderError> {
        self.command(WorkerCommand::Pause)
    }

    fn resume_recording(&mut self) -> Result<(), RecorderError> {
        self.command(WorkerCommand::Resume)
    }

    fn stop_recording(&mut self) -> Result<PathBuf, RecorderError> {
        // The portal can close before the UI sends Stop, or while it is being
        // sent. Joining and reading the cached result handles both races.
        let result = if self.has_stopped() {
            None
        } else {
            Some(self.command(WorkerCommand::Stop))
        };
        self.commands = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.state
            .lock()
            .expect("native recorder state poisoned")
            .completion
            .clone()
            .map(|result| result.map_err(RecorderError::Pipeline))
            .or(result)
            .unwrap_or_else(|| Err(RecorderError::State("there is no active native recording".into())))
    }

    fn has_stopped(&self) -> bool {
        self.state.lock().map(|state| state.stopped).unwrap_or(true)
    }

    fn source_geometry(&self) -> Option<CaptureSourceGeometry> {
        self.state.lock().ok().and_then(|state| state.source_geometry)
    }

    fn includes_system_audio(&self) -> bool {
        self.options.system_audio
    }

    fn includes_microphone(&self) -> bool {
        self.options.microphone
    }

    fn includes_camera(&self) -> bool {
        self.options.camera
            && self
                .state
                .lock()
                .map(|state| state.camera_recorded)
                .unwrap_or(false)
    }

    fn capture_warnings(&self) -> Vec<String> {
        let camera_was_lost = self.options.camera && self.state.lock().is_ok_and(|state| {
            matches!(state.completion, Some(Ok(_))) && !state.camera_recorded
        });
        if camera_was_lost {
            vec!["The screen recording was saved, but the webcam recording could not be finalized. Any recoverable camera segments remain in the project folder.".into()]
        } else {
            Vec::new()
        }
    }

    fn pointer_synthesized(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.pointer_synthesized)
            .unwrap_or(false)
    }
}

impl Drop for NativeRecorder {
    fn drop(&mut self) {
        if self.commands.is_some() {
            let _ = self.stop_recording();
        }
    }
}

fn ensure_runtime() -> Result<(), RecorderError> {
    for (program, version_arg, explanation) in [
        (
            "gst-launch-1.0",
            "--version",
            "GStreamer tools are required",
        ),
        (
            "ffmpeg",
            "-version",
            "FFmpeg is required to join paused recording segments",
        ),
    ] {
        let found = Command::new(program)
            .arg(version_arg)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !found {
            return Err(RecorderError::Pipeline(format!(
                "{explanation}, but `{program}` is unavailable"
            )));
        }
    }
    Ok(())
}

fn run_worker(
    output: PathBuf,
    mut options: RecordingOptions,
    startup: Arc<RecordingStartup>,
    state: Arc<Mutex<SharedState>>,
    commands: mpsc::Receiver<WorkerCommand>,
    ready: mpsc::Sender<Result<(), String>>,
    camera_frames: Option<Arc<CameraFrames>>,
    area_requests: Option<mpsc::Sender<AreaRequest>>,
) {
    let result = async_io::block_on(async {
        if startup.is_cancelled() { return Err("Recording was cancelled before capture started.".into()); }
        let input_destination = output.with_file_name("input.json");
        // Area capture embeds the compositor cursor. Do not collect unused
        // full-monitor input metadata or leave raw events in the project.
        let mut input_capture = (area_requests.is_none() && options.area.is_none())
            .then(|| InputCapture::start(output.parent().unwrap_or_else(|| Path::new("."))));
        let proxy = Screencast::new().await.map_err(|error| error.to_string())?;
        let session = proxy
            .create_session()
            .await
            .map_err(|error| error.to_string())?;
        // Subscribe before showing the picker so a fast external close cannot
        // be lost between receiving the stream and starting the worker loop.
        let mut closed = Box::pin(session.receive_closed().await.map_err(|error| error.to_string())?);
        let selection = during_startup(&startup, async { proxy
            .select_sources(
                &session,
                if area_requests.is_some() || options.area.is_some() {
                    // Crop the real cursor along with the screen. Synthesizing
                    // it from full-monitor metadata would misplace it.
                    CursorMode::Embedded
                } else if input_capture
                    .as_ref()
                    .is_some_and(InputCapture::uses_pipewire_metadata)
                {
                    CursorMode::Metadata
                } else if input_capture.as_ref().is_some_and(InputCapture::is_active) {
                    CursorMode::Hidden
                } else {
                    CursorMode::Embedded
                },
                if area_requests.is_some() || options.area.is_some() {
                    SourceType::Monitor.into()
                } else {
                    match options.source {
                        RecordingSource::Any => SourceType::Monitor | SourceType::Window,
                        RecordingSource::Screen => SourceType::Monitor.into(),
                        RecordingSource::Window => SourceType::Window.into(),
                    }
                },
                false,
                None,
                PersistMode::ExplicitlyRevoked,
            )
            .await
            .map_err(|error| error.to_string()) }).await;
        if let Err(error) = selection {
            let _ = session.close().await;
            return Err(error);
        }
        let response = during_startup(&startup, async { proxy
            .start(&session, None)
            .await
            .map_err(|error| error.to_string())?
            .response()
            .map_err(|error| error.to_string()) }).await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let _ = session.close().await;
                return Err(error);
            }
        };
        let stream = response
            .streams()
            .first()
            .ok_or_else(|| "no screen or window was selected".to_string())?;
        let node = stream.pipe_wire_node_id();
        state.lock().expect("native recorder state poisoned").source_geometry =
            CaptureSourceGeometry::from_portal(stream.position(), stream.size(), stream.source_type());
        let preserve_alpha = stream.source_type() == Some(SourceType::Window);
        let monitor_exclusions: &[CaptureExclusion] = if preserve_alpha {
            &[]
        } else {
            &options.capture_exclusions
        };
        if let Err(error) = CaptureExclusion::wait_until_all_protected(
            monitor_exclusions,
            Duration::from_secs(3),
        ) {
            let _ = session.close().await;
            return Err(error);
        }
        let input_mapping = InputMapping {
            origin: stream.position().map(|(x, y)| (f64::from(x), f64::from(y))),
            size: stream
                .size()
                .map(|(width, height)| (f64::from(width), f64::from(height)))
                .unwrap_or((1.0, 1.0)),
        };
        if let Some(requests) = area_requests.as_ref() {
            let remote = proxy
                .open_pipe_wire_remote(&session)
                .await
                .map_err(|error| error.to_string())?;

            let frame = match preview_frame(
                remote.as_raw_fd(),
                node,
                !options.capture_exclusions.is_empty(),
            ) {
                Ok(frame) => frame,
                Err(error) => {
                    let _ = session.close().await;
                    return Err(error);
                }
            };
            let (reply, response) = mpsc::channel();
            let selection = requests
                .send(AreaRequest { frame, reply })
                .map_err(|_| "area selection was cancelled".to_string())
                .and_then(|_| loop {
                    if startup.is_cancelled() || closed.next().now_or_never().is_some() {
                        break Err("area selection was cancelled".to_string());
                    }
                    match response.recv_timeout(Duration::from_millis(50)) {
                        Ok(Some(area)) => break Ok(area),
                        Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break Err("area selection was cancelled".into()),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                });
            match selection {
                Ok(area) => options.area = Some(area),
                Err(error) => {
                    let _ = session.close().await;
                    return Err(error);
                }
            }
            // Give the compositor time to remove the selection window before
            // the first encoded frame. Preview pixels never reach a file.
            thread::sleep(Duration::from_millis(250));
        }

        if let Err(error) = wait_for_countdown(&startup, options.countdown_seconds, || {
            closed.next().now_or_never().is_some()
        }) {
            let _ = session.close().await;
            return Err(error);
        }
        let camera = if options.camera {
            Some(match options.camera_device.clone() {
                Some(device) => device,
                None => default_camera_device()?,
            })
        } else {
            None
        };
        // Each GStreamer pipeline needs a fresh portal connection, including
        // after the area-selection preview has released its own pipeline.
        let remote = proxy
            .open_pipe_wire_remote(&session)
            .await
            .map_err(|error| error.to_string())?;
        // Area selection may have been open for a long time. Verify the
        // helper again immediately before the first frame can be encoded.
        if let Err(error) = CaptureExclusion::wait_until_all_protected(
            monitor_exclusions,
            Duration::from_secs(3),
        ) {
            let _ = session.close().await;
            return Err(error);
        }
        let recording_size = RecordingSize::default();
        let mut segments = Vec::new();
        if startup.is_cancelled() {
            let _ = session.close().await;
            return Err("Recording was cancelled before capture started.".into());
        }
        let mut child = Some(spawn_segment(
            &output,
            segments.len(),
            remote,
            node,
            &options,
            camera.as_deref(),
            camera_frames.as_ref(),
            recording_size.clone(),
            preserve_alpha,
        )?);
        if startup.is_cancelled() {
            drop(child.take());
            let _ = session.close().await;
            return Err("Recording was cancelled before capture started.".into());
        }
        // Negotiate and receive video before adding the cursor consumer.
        // Starting the metadata-only stream first can leave a window's video
        // consumer without any frames, even though caps and EOS succeed.
        let pointer_setup_error = if let Some(input_capture) = input_capture
            .as_mut()
            .filter(|input| input.uses_pipewire_metadata())
        {
            let attached = async {
                let pointer_remote = proxy
                    .open_pipe_wire_remote(&session)
                    .await
                    .map_err(|error| error.to_string())?;
                input_capture.attach_pipewire(pointer_remote.as_raw_fd(), node, input_mapping)
            }.await;
            attached.err()
        } else {
            None
        };
        if let Some(error) = pointer_setup_error {
            // The screen already has frames. Metadata is optional; if the
            // portal closed during setup the normal terminal loop below will
            // finalize the take rather than report a failed/empty setup.
            eprintln!("Recording pointer metadata unavailable: {error}");
            input_capture.take();
        }
        let mut indicator =
            options
                .area
                .and_then(|area| match AreaIndicator::start(area, input_mapping) {
                    Ok(indicator) => Some(indicator),
                    Err(error) => {
                        eprintln!("Recording area indicator unavailable: {error}");
                        None
                    }
                });
        segments.push(0);
        let mut active_ranges = Vec::new();
        let mut active_range_start = Some(monotonic_ns());
        {
            let mut shared = state.lock().expect("native recorder state poisoned");
            shared.active = true;
            shared.paused = false;
            shared.started_at = Some(Instant::now());
            shared.elapsed = Duration::ZERO;
            shared.output = Some(segment_path(&output, 0));
            shared.failure = None;
        }
        if startup.is_cancelled() {
            drop(child.take());
            let _ = session.close().await;
            return Err("Recording was cancelled before capture started.".into());
        }
        let _ = ready.send(Ok(()));

        let mut stop_reply = None;
        let mut terminal_error = None;
        loop {
            if let Some(indicator) = indicator.as_mut() {
                indicator.tick();
            }
            // Polling the D-Bus stream never blocks command handling. Its
            // connection receives signals independently of the capture loop.
            if closed.next().now_or_never().is_some() {
                break;
            }
            if let Some(end) = child.as_mut().and_then(SegmentPipeline::take_end) {
                if let PipelineEnd::Error(error) = end {
                    terminal_error = Some(error);
                }
                break;
            }
            let command = match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match command {
                WorkerCommand::Pause(reply) => {
                    let finishing_segment = child.is_some();
                    let result = if let Some(mut running) = child.take() {
                        if let Some(input_capture) = input_capture.as_mut() {
                            input_capture.detach_pipewire();
                        }
                        if let Some(start_ns) = active_range_start.take() {
                            active_ranges.push(ActiveRange {
                                start_ns,
                                end_ns: monotonic_ns(),
                            });
                        }
                        finalize_child(&mut running).map(|_| {
                            let mut shared = state.lock().expect("native recorder state poisoned");
                            if let Some(started) = shared.started_at.take() {
                                shared.elapsed += started.elapsed();
                            }
                            shared.paused = true;
                            if let Some(indicator) = indicator.as_mut() {
                                indicator.set_paused(true);
                            }
                        })
                    } else {
                        Err("recording is already paused".into())
                    };
                    let failed = finishing_segment && result.is_err();
                    if failed {
                        let error = result.as_ref().unwrap_err();
                        terminal_error = Some(error.clone());
                        mark_stopped(&state);
                    }
                    let _ = reply.send(result);
                    if failed {
                        break;
                    }
                }
                WorkerCommand::Resume(reply) => {
                    let result = if child.is_some() {
                        Err("recording is not paused".into())
                    } else {
                        let index = segments.len();
                        // The previous segment has consumed its connection.
                        let next = proxy
                            .open_pipe_wire_remote(&session)
                            .await
                            .map_err(|error| error.to_string())
                            .and_then(|next_remote| {
                                // Opening the portal remote may take time;
                                // require fresh protection just before encode.
                                CaptureExclusion::wait_until_all_protected(
                                    monitor_exclusions,
                                    Duration::from_secs(3),
                                )?;
                                spawn_segment(
                                    &output,
                                    index,
                                    next_remote,
                                    node,
                                    &options,
                                    camera.as_deref(),
                                    camera_frames.as_ref(),
                                    recording_size.clone(),
                                    preserve_alpha,
                                )
                            });
                        match next {
                            Ok(next) => {
                                if let Some(input_capture) = input_capture
                                    .as_mut()
                                    .filter(|input| input.uses_pipewire_metadata())
                                {
                                    let pointer_result = async {
                                        let pointer_remote = proxy
                                            .open_pipe_wire_remote(&session)
                                            .await
                                            .map_err(|error| error.to_string())?;
                                        input_capture.attach_pipewire(
                                            pointer_remote.as_raw_fd(),
                                            node,
                                            input_mapping,
                                        )
                                    }
                                    .await;
                                    if let Err(error) = pointer_result {
                                        // The segment is already encoding.
                                        // Keep it tracked and finalize instead
                                        // of dropping it and reusing its path.
                                        segments.push(index);
                                        child = Some(next);
                                        active_range_start = Some(monotonic_ns());
                                        terminal_error = Some(error.clone());
                                        mark_stopped(&state);
                                        let _ = reply.send(Err(error));
                                        break;
                                    }
                                }
                                let path = segment_path(&output, index);
                                segments.push(index);
                                child = Some(next);
                                active_range_start = Some(monotonic_ns());
                                let mut shared =
                                    state.lock().expect("native recorder state poisoned");
                                shared.paused = false;
                                shared.started_at = Some(Instant::now());
                                shared.output = Some(path);
                                if let Some(indicator) = indicator.as_mut() {
                                    indicator.set_paused(false);
                                }
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    };
                    let _ = reply.send(result);
                }
                WorkerCommand::Stop(reply) => {
                    stop_reply = Some(reply);
                    break;
                }
            }
        }
        mark_stopped(&state);
        drop(indicator.take());
        if let Some(start_ns) = active_range_start.take() {
            active_ranges.push(ActiveRange { start_ns, end_ns: monotonic_ns() });
        }
        if let Some(input_capture) = input_capture.as_mut() {
            input_capture.detach_pipewire();
        }
        let result = finalize_recording(
            &output, child.take(), &segments, camera.is_some(), terminal_error,
        );
        let pointer_synthesized = input_capture.take().is_some_and(|input_capture| {
            input_capture.finish(&active_ranges, input_mapping, &input_destination).unwrap_or(false)
        });
        {
            let mut shared = state.lock().expect("native recorder state poisoned");
            shared.pointer_synthesized = pointer_synthesized;
            if let Ok(camera_recorded) = result.as_ref() {
                shared.output = Some(output.clone());
                shared.camera_recorded = *camera_recorded;
            }
        }
        let completion = result.map(|_| output.clone());
        store_completion(&state, completion.clone());
        if let Some(reply) = stop_reply {
            let _ = reply.send(completion);
        }
        let _ = session.close().await;
        Ok::<(), String>(())
    });

    if let Err(error) = result {
        store_completion(&state, Err(error.clone()));
        let _ = ready.send(Err(error));
    }
}

fn segment_path(output: &Path, index: usize) -> PathBuf {
    output.with_file_name(format!("screen-segment-{index:04}.mkv"))
}

fn mark_stopped(state: &Arc<Mutex<SharedState>>) {
    let mut state = state.lock().expect("native recorder state poisoned");
    if let Some(started) = state.started_at.take() {
        state.elapsed += started.elapsed();
    }
    state.active = false;
    state.paused = false;
    state.stopped = true;
}

fn store_completion(state: &Arc<Mutex<SharedState>>, result: Result<PathBuf, String>) {
    mark_stopped(state);
    let mut state = state.lock().expect("native recorder state poisoned");
    state.failure = result.as_ref().err().cloned();
    state.completion = Some(result);
}

/// Every terminal path installs the same screen/camera output, including a
/// portal close while paused and a source that ends before Stop is clicked.
fn finalize_recording(
    output: &Path,
    mut child: Option<SegmentPipeline>,
    segments: &[usize],
    camera: bool,
    terminal_error: Option<String>,
) -> Result<bool, String> {
    let drain_error = child.as_mut().and_then(|child| finalize_child(child).err());
    drop(child);
    let screen_segments: Vec<_> = segments.iter().map(|index| segment_path(output, *index)).collect();
    let recovery_error = drain_error.or(terminal_error);
    if let Some(error) = recovery_error.as_ref() {
        // A disconnected source may leave valid frames in a container whose
        // final index was not written. Remux it before installing the project.
        for path in &screen_segments {
            recover_segment(path)?;
        }
        eprintln!("Recording source ended; preserving captured media: {error}");
    }
    finalize_segments(&screen_segments, output)?;
    let camera_recorded = camera && {
        let camera_segments: Vec<_> = segments.iter().map(|index| camera_segment_path(output, *index)).collect();
        let repair = if recovery_error.is_some() {
            camera_segments.iter().try_for_each(|path| recover_segment(path))
        } else {
            Ok(())
        };
        match repair.and_then(|_| finalize_segments(&camera_segments, &camera_path(output))) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("Webcam recording was dropped: {error}");
                false
            }
        }
    };
    Ok(camera_recorded)
}

fn recover_segment(path: &Path) -> Result<(), String> {
    let recovered = path.with_extension("recovered.mkv");
    remux_recovered_segment(path, &recovered)?;
    fs::rename(recovered, path).map_err(|error| format!("could not install recovered recording: {error}"))
}

fn remux_recovered_segment(path: &Path, recovered: &Path) -> Result<(), String> {
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-copyts", "-copytb", "1", "-i"])
        .arg(path)
        .args(["-map", "0", "-c", "copy"])
        .arg(recovered)
        .output()
        .map_err(|error| format!("could not recover recording: {error}"))?;
    if !result.status.success()
        || !super::video::probe_media(recovered).is_ok_and(|info| info.duration.is_finite() && info.duration > 0.0)
    {
        let _ = fs::remove_file(recovered);
        return Err(format!("could not finalize recording; captured segments remain at {}: {}",
            path.display(), String::from_utf8_lossy(&result.stderr).trim()));
    }
    Ok(())
}

fn interrupted_segments(directory: &Path, role: &str) -> Result<Vec<PathBuf>, String> {
    let prefix = format!("{role}-segment-");
    let mut segments = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue; };
        let Some(number) = name.strip_prefix(&prefix).and_then(|name| name.strip_suffix(".mkv")) else { continue; };
        let Ok(index) = number.parse::<usize>() else { continue; };
        if name != format!("{prefix}{index:04}.mkv") || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        segments.push((index, entry.path()));
    }
    segments.sort_by_key(|(index, _)| *index);
    if segments.iter().enumerate().any(|(expected, (actual, _))| expected != *actual) {
        return Err(format!("Some {role} recording segments are missing; the remaining files have been kept in {}", directory.display()));
    }
    Ok(segments.into_iter().map(|(_, path)| path).collect())
}

/// Reconstruct every ordered native segment, never a webcam or newest-only
/// substitute for the screen. Work on remuxed copies so a failed repair leaves
/// every original available for another recovery attempt.
pub(super) fn recover_interrupted_segments(output: &Path) -> Result<bool, String> {
    let directory = output.parent().ok_or("recording has no project directory")?;
    let screen_exists = output.metadata().is_ok_and(|meta| meta.len() > 0);
    // Older builds concatenated directly into screen.mkv. A crash could leave
    // a nonempty partial final file next to every original segment. Prefer the
    // complete contiguous source set even when that destination already exists.
    let screen = match interrupted_segments(directory, "screen") {
        Ok(segments) => segments,
        Err(error) if screen_exists && valid_finalized_media(output) => {
            // A crash after atomic installation can interrupt source cleanup.
            // An incomplete remainder cannot reconstruct the complete take.
            eprintln!("Keeping finalized screen recording: {error}");
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    let camera_output = camera_path(output);
    let camera = interrupted_segments(directory, "camera").unwrap_or_else(|error| {
        eprintln!("Webcam recovery: {error}");
        Vec::new()
    });
    if screen.is_empty() && (!screen_exists || camera.is_empty()) {
        return Ok(false);
    }
    let work = directory.join(format!(".recovery-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&work).map_err(|e| format!("could not create recovery workspace: {e}"))?;
    let repair = |sources: &[PathBuf], destination: &Path, role: &str| -> Result<(), String> {
        let copies: Vec<_> = sources.iter().enumerate()
            .map(|(index, _)| work.join(format!("{role}-{index:04}.mkv"))).collect();
        for (source, copy) in sources.iter().zip(&copies) {
            remux_recovered_segment(source, copy)?;
        }
        let completed = work.join(format!("{role}.mkv"));
        finalize_segments(&copies, &completed)?;
        let completed_duration = super::video::probe_media(&completed).map_err(|e| e.to_string())?.duration;
        if let Ok(existing) = super::video::probe_media(destination) {
            if existing.duration > completed_duration + 0.15 {
                return Err(format!("The remaining {role} segments are shorter than the existing recording; all originals were kept"));
            }
        }
        if role == "camera" {
            let screen_duration = super::video::probe_media(output).map_err(|e| e.to_string())?.duration;
            if (screen_duration - completed_duration).abs() > 0.15 {
                return Err("Recovered webcam duration does not match the screen; the original camera segments were kept".into());
            }
        }
        fs::rename(&completed, destination).map_err(|e| format!("could not install recovered {role}: {e}"))?;
        for source in sources {
            let _ = fs::remove_file(source);
        }
        Ok(())
    };
    if !screen.is_empty() {
        if let Err(error) = repair(&screen, output, "screen") {
            let _ = fs::remove_dir_all(&work);
            return Err(error);
        }
    }
    if !camera.is_empty() {
        if !screen.is_empty() && camera.len() != screen.len() {
            eprintln!("Webcam recovery: screen and camera segment counts differ; camera files remain in {}", directory.display());
        } else if let Err(error) = repair(&camera, &camera_output, "camera") {
            eprintln!("Webcam recovery: {error}");
        }
    }
    let _ = fs::remove_dir_all(&work);
    Ok(true)
}

fn camera_segment_path(output: &Path, index: usize) -> PathBuf {
    output.with_file_name(format!("camera-segment-{index:04}.mkv"))
}

fn camera_path(output: &Path) -> PathBuf {
    output.with_file_name(super::model::RecordingSession::CAMERA_FILE)
}

/// A microphone (or other capture source) the recorder can use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioSource {
    /// The PulseAudio/PipeWire node name passed to `pulsesrc device=`.
    pub name: String,
    /// The human-readable description shown in the picker.
    pub description: String,
    pub is_default: bool,
}

/// The capture sources GStreamer can record from, skipping output monitors.
pub fn audio_sources() -> Vec<AudioSource> {
    if gst::init().is_err() {
        return Vec::new();
    }
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Audio/Source"), None);
    if monitor.start().is_err() {
        return Vec::new();
    }
    // The PipeWire and Pulse providers each report every device, and only
    // the former flags the default; merge duplicates by node name.
    let mut sources: Vec<AudioSource> = Vec::new();
    for device in monitor.devices().iter() {
        let Some(properties) = device.properties() else {
            continue;
        };
        let Ok(name) = properties.get::<String>("node.name") else {
            continue;
        };
        let description = properties
            .get::<String>("node.description")
            .unwrap_or_else(|_| device.display_name().to_string());
        if name.ends_with(".monitor") || description.starts_with("Monitor of ") {
            continue;
        }
        let is_default = properties.get::<bool>("is-default").unwrap_or(false);
        match sources.iter_mut().find(|source| source.name == name) {
            Some(existing) => existing.is_default |= is_default,
            None => sources.push(AudioSource {
                name,
                description,
                is_default,
            }),
        }
    }
    monitor.stop();
    sources
}

/// The V4L2 device node of the first webcam GStreamer can capture from.
pub fn default_camera_device() -> Result<String, String> {
    camera_devices_result()?
        .into_iter()
        .next()
        .map(|(path, _)| path)
        .ok_or_else(|| "no webcam was found; unplug and replug it or turn the camera off".into())
}

/// Webcams GStreamer can capture from, as `(V4L2 device node, display name)`.
pub fn camera_devices() -> Vec<(String, String)> {
    camera_devices_result().unwrap_or_default()
}

pub fn camera_devices_result() -> Result<Vec<(String, String)>, String> {
    gst::init().map_err(|error| format!("could not initialize camera discovery: {error}"))?;
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    monitor.start().map_err(|error| format!("could not discover webcams: {error}"))?;
    let mut seen = std::collections::HashSet::new();
    let mut devices: Vec<_> = monitor
        .devices()
        .iter()
        .filter_map(|device| {
            let properties = device.properties()?;
            let path = ["device.path", "api.v4l2.path"]
                .iter()
                .find_map(|key| properties.get::<String>(*key).ok())?;
            seen.insert(path.clone())
                .then(|| (path, device.display_name().to_string()))
        })
        .collect();
    monitor.stop();
    devices.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(devices)
}

/// Microphones GStreamer can capture from, as `(source name, display name)`.
/// Monitor sources (system audio loopbacks) are excluded.
pub fn microphone_devices() -> Vec<(String, String)> {
    if gst::init().is_err() {
        return Vec::new();
    }
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Audio/Source"), None);
    if monitor.start().is_err() {
        return Vec::new();
    }
    // The PipeWire and PulseAudio providers both describe the same sources
    // (raw ALSA devices are skipped: `pulsesrc` cannot open them), so keep
    // the first entry per source name.
    let mut seen = std::collections::HashSet::new();
    let devices = monitor
        .devices()
        .iter()
        .filter_map(|device| {
            let properties = device.properties()?;
            if properties.get::<String>("device.class").ok().as_deref() == Some("monitor") {
                return None;
            }
            let name = properties.get::<String>("node.name").ok().or_else(|| {
                if properties.get::<String>("device.api").ok().as_deref() != Some("pulse") {
                    return None;
                }
                let element = device.create_element(None).ok()?;
                element
                    .has_property("device")
                    .then(|| element.property::<Option<String>>("device"))
                    .flatten()
            })?;
            seen.insert(name.clone())
                .then(|| (name, device.display_name().to_string()))
        })
        .collect();
    monitor.stop();
    devices
}

struct SegmentPipeline {
    pipeline: gst::Pipeline,
    failure: Option<String>,
    has_video: Arc<AtomicBool>,
    source_ended: Arc<AtomicBool>,
    pipeline_eos: bool,
    // Keep the fd identity alive until GStreamer releases its cached core.
    _portal_remote: Option<OwnedFd>,
}

enum PipelineEnd {
    Eos,
    Error(String),
}

impl SegmentPipeline {
    fn new(pipeline: gst::Pipeline) -> Result<Self, String> {
        let pad = pipeline
            .by_name("screen_encoder")
            .and_then(|encoder| encoder.static_pad("src"))
            .ok_or_else(|| "GStreamer pipeline has no screen encoder output".to_string())?;
        let has_video = Arc::new(AtomicBool::new(false));
        let source_ended = Arc::new(AtomicBool::new(false));
        if let Some(source_pad) = pipeline.by_name("screen_source").and_then(|source| source.static_pad("src")) {
            let ended = source_ended.clone();
            source_pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
                if matches!(info.data.as_ref(), Some(gst::PadProbeData::Event(event)) if event.type_() == gst::EventType::Eos) {
                    ended.store(true, Ordering::Release);
                }
                gst::PadProbeReturn::Ok
            });
        }
        let received = has_video.clone();
        // videobox initially negotiates 1x1 before the first PipeWire buffer
        // supplies its content rectangle. Audio can make matroskamux write
        // that provisional size into its permanent header before video arrives.
        // Keep only the latest caps, and deliver them with the first encoded
        // frame, after stream_geometry has established the recording size.
        let headers = Mutex::new(Some((None::<gst::Event>, Vec::<gst::Event>::new())));
        pad.add_probe(
            gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
            move |pad, info| {
                let mut headers = headers.lock().expect("screen headers poisoned");
                let Some((caps, events)) = headers.as_mut() else {
                    return gst::PadProbeReturn::Ok;
                };
                if let Some(gst::PadProbeData::Event(event)) = info.data.as_ref() {
                    match event.type_() {
                        gst::EventType::Caps => {
                            *caps = Some(event.clone());
                            return gst::PadProbeReturn::Drop;
                        }
                        gst::EventType::Segment | gst::EventType::Tag => {
                            // Sticky events can be retried while caps are held.
                            events.retain(|previous| previous.type_() != event.type_());
                            events.push(event.clone());
                            return gst::PadProbeReturn::Drop;
                        }
                        gst::EventType::Eos => {}
                        _ => return gst::PadProbeReturn::Ok,
                    }
                }
                let (caps, events) = headers.take().unwrap();
                drop(headers);
                // Pushing reenters this probe; the drained state passes these
                // events through. EOS also flushes headers for empty captures.
                if let Some(caps) = caps {
                    pad.push_event(caps);
                }
                for event in events {
                    pad.push_event(event);
                }
                if matches!(info.data, Some(gst::PadProbeData::Buffer(_))) {
                    received.store(true, Ordering::Release);
                }
                gst::PadProbeReturn::Remove
            },
        );
        Ok(Self {
            pipeline,
            failure: None,
            has_video,
            source_ended,
            pipeline_eos: false,
            _portal_remote: None,
        })
    }

    fn wait_for_video(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.has_video.load(Ordering::Acquire) {
                return Ok(());
            }
            if let Some(error) = self.take_failure() {
                // The first frame and source EOS can arrive in the same
                // poll. Preserve a real take for terminal finalization.
                if self.has_video.load(Ordering::Acquire) {
                    self.failure = Some(error);
                    return Ok(());
                }
                return Err(error);
            }
            if Instant::now() >= deadline {
                return Err("No video frames arrived from the selected screen or window. Keep it visible and try recording again.".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn take_failure(&mut self) -> Option<String> {
        self.take_end().map(|end| match end {
            PipelineEnd::Eos => "GStreamer stopped unexpectedly".into(),
            PipelineEnd::Error(error) => error,
        })
    }

    fn take_end(&mut self) -> Option<PipelineEnd> {
        if self.failure.is_none() {
            let bus = self.pipeline.bus()?;
            while let Some(message) = bus.pop() {
                match message.view() {
                    gst::MessageView::Error(error) => {
                        self.failure = Some(format!(
                            "GStreamer pipeline failed: {} ({})",
                            error.error(),
                            error.debug().unwrap_or_default()
                        ));
                        break;
                    }
                    gst::MessageView::Eos(..) => {
                        self.pipeline_eos = true;
                        return Some(PipelineEnd::Eos);
                    }
                    _ => {}
                }
            }
        }
        self.failure.take().map(PipelineEnd::Error).or_else(|| {
            (self.pipeline_eos || self.source_ended.load(Ordering::Acquire)).then_some(PipelineEnd::Eos)
        })
    }
}

impl Drop for SegmentPipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

pub(super) const WINDOW_ENCODER: &str =
    "video/x-raw,format=BGRA ! avenc_ffv1 name=screen_encoder threads=0";

/// Separate encoded streams retain source identity through pause/resume and
/// recovery. Cleaning a microphone must never filter desktop music or calls.
/// Both branches use the screen pipeline's running-time clock.
fn capture_audio_branch(source: &str, role: &str, stream_index: usize) -> String {
    format!(
        "{source} ! audio/x-raw,rate=48000 ! queue ! audioconvert ! audioresample ! \
         opusenc name=audio_encoder_{role} bitrate=160000 ! queue ! mux.audio_{stream_index} "
    )
}

fn tag_capture_audio(pipeline: &gst::Pipeline) {
    for (role, title) in [
        ("microphone", "Lahza-Microphone"),
        ("system", "Lahza-System-Audio"),
    ] {
        let Some(encoder) = pipeline.by_name(&format!("audio_encoder_{role}")) else {
            continue;
        };
        // Opus encoder TagSetter metadata is not forwarded as Matroska stream
        // tags. Send a stream-scoped tag before its first encoded buffer. Use
        // stable identifiers: some matroskamux versions serialize spaces with
        // quotes/escapes, making roles unreliable to identify with ffprobe.
        encoder.static_pad("src").unwrap().add_probe(gst::PadProbeType::BUFFER, move |pad, _| {
            let mut tags = gst::TagList::new();
            let tags_mut = tags.get_mut().unwrap();
            tags_mut.set_scope(gst::TagScope::Stream);
            tags_mut.add::<gst::tags::Title>(&title, gst::TagMergeMode::Replace);
            pad.push_event(gst::event::Tag::new(tags));
            gst::PadProbeReturn::Remove
        });
    }
}

fn build_segment_pipeline(
    output: &Path,
    index: usize,
    portal_fd: i32,
    node: u32,
    options: &RecordingOptions,
    camera_device: Option<&str>,
    camera_frames: Option<&Arc<CameraFrames>>,
    preserve_alpha: bool,
) -> Result<gst::Pipeline, String> {
    let path = segment_path(output, index);
    let _ = fs::remove_file(&path);
    gst::init().map_err(|error| format!("could not initialize GStreamer: {error}"))?;
    let crop_filter = options
        .area
        .map(RecordingArea::pipeline_filter)
        .unwrap_or_default();
    // Window buffers include transparent shadows and rounded corners. Keep
    // their alpha in the source recording so Studio can composite them.
    let encoder = if preserve_alpha {
        WINDOW_ENCODER
    } else {
        "vp8enc name=screen_encoder deadline=1 cpu-used=8 threads=4 target-bitrate=12000000 keyframe-max-dist=60"
    };
    // always-copy alone also accepts DMA-BUFs copied from the visible desktop.
    let capture_filter = if !options.capture_exclusions.is_empty() {
        format!("capsfilter name=screen_capture_memory caps=\"{CAPTURE_MEMORY_CAPS}\" ! ")
    } else {
        String::new()
    };
    let mut description = format!(
        "matroskamux name=mux ! filesink location=\"{}\" \
         pipewiresrc name=screen_source fd={} path={} always-copy=true \
         keepalive-time=1000 ! {capture_filter}{} ! {crop_filter}\
         queue max-size-buffers=4 leaky=downstream ! \
         {encoder} ! queue ! mux.video_0 ",
        path.display(),
        portal_fd,
        node,
        if preserve_alpha {
            stream_geometry::NATIVE_WINDOW_FILTER
        } else {
            stream_geometry::FILTER
        },
    );
    // Explicit pad names keep microphone first and system second when both
    // are enabled. Tags, rather than track positions, identify a single source.
    if options.microphone {
        description.push_str(&capture_audio_branch(
            "pulsesrc name=microphone_source", "microphone", 0,
        ));
    }
    if options.system_audio {
        description.push_str(&capture_audio_branch(
            "pulsesrc name=system_audio_source device=@DEFAULT_MONITOR@",
            "system", usize::from(options.microphone),
        ));
    }
    if camera_device.is_some() {
        // The webcam shares the screen pipeline's clock, so both files carry
        // the same running-time stamps and stay aligned in the editor.
        let camera_segment = camera_segment_path(output, index);
        let _ = fs::remove_file(&camera_segment);
        description.push_str(&format!(
            "v4l2src name=camera_source do-timestamp=true ! decodebin ! videoconvert ! tee name=camera_tee \
             camera_tee. ! queue max-size-buffers=4 leaky=downstream ! \
             vp8enc deadline=1 cpu-used=8 threads=2 target-bitrate=4000000 \
             keyframe-max-dist=60 ! queue ! matroskamux ! \
             filesink location=\"{}\" ",
            camera_segment.display()
        ));
    }

    if camera_device.is_some() && camera_frames.is_some() {
        description.push_str(&format!(
            "camera_tee. ! {}",
            preview_branch("recording_camera_preview")
        ));
    }

    let pipeline = gst::parse::launch(&description)
        .map_err(|error| format!("could not build GStreamer pipeline: {error}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "GStreamer did not create a pipeline".to_string())?;
    tag_capture_audio(&pipeline);
    if let Some(device) = options.microphone_device.as_deref().filter(|_| options.microphone) {
        pipeline.by_name("microphone_source").unwrap().set_property("device", device);
    }
    if let Some(device) = camera_device {
        pipeline.by_name("camera_source").ok_or_else(|| "GStreamer pipeline has no webcam source".to_string())?
            .set_property("device", device);
    }
    // Keep capture timestamps on a steady clock without relying on the
    // pipewiresrc `provide-clock` property, absent in the core24 Snap runtime.
    pipeline.use_clock(Some(&gst::SystemClock::obtain()));
    Ok(pipeline)
}

fn spawn_segment(
    output: &Path,
    index: usize,
    portal_remote: OwnedFd,
    node: u32,
    options: &RecordingOptions,
    camera_device: Option<&str>,
    camera_frames: Option<&Arc<CameraFrames>>,
    recording_size: RecordingSize,
    preserve_alpha: bool,
) -> Result<SegmentPipeline, String> {
    let pipeline = build_segment_pipeline(
        output,
        index,
        portal_remote.as_raw_fd(),
        node,
        options,
        camera_device,
        camera_frames,
        preserve_alpha,
    )?;
    stream_geometry::attach(&pipeline, recording_size, preserve_alpha)?;
    if preserve_alpha {
        fs::write(
            output.with_extension("window.json"),
            r#"{"version":1,"pixels":"native"}"#,
        )
        .map_err(|error| format!("could not save window capture metadata: {error}"))?;
    }
    if let Some(frames) = camera_frames.filter(|_| camera_device.is_some()) {
        attach_preview(&pipeline, "recording_camera_preview", frames.clone())?;
    }
    let source = pipeline
        .by_name("screen_source")
        .ok_or_else(|| "GStreamer pipeline has no screen source".to_string())?;
    let source_pad = source
        .static_pad("src")
        .ok_or_else(|| "GStreamer screen source has no output pad".to_string())?;
    let weak_pipeline = pipeline.downgrade();
    source_pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
        let Some(gst::PadProbeData::Buffer(buffer)) = info.data.as_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let Some(pipeline) = weak_pipeline.upgrade() else {
            return gst::PadProbeReturn::Remove;
        };
        // Mutter's metadata-mode buffers can carry repeated or zero header PTS.
        // Stamp frames at the instant GStreamer receives them so sparse desktop
        // damage and keepalive frames retain their actual wall-clock spacing.
        if let Some(running_time) = pipeline.current_running_time() {
            let buffer = buffer.make_mut();
            buffer.set_pts(running_time);
            buffer.set_dts(running_time);
            buffer.set_duration(gst::ClockTime::NONE);
        }
        gst::PadProbeReturn::Ok
    });
    let mut segment = SegmentPipeline::new(pipeline)?;
    segment._portal_remote = Some(portal_remote);
    segment
        .pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| format!("could not start GStreamer pipeline: {error}"))?;
    segment.wait_for_video(Duration::from_secs(5))?;
    Ok(segment)
}

fn finalize_child(child: &mut SegmentPipeline) -> Result<(), String> {
    if child.pipeline_eos {
        let _ = child.pipeline.set_state(gst::State::Null);
        return if child.has_video.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err("No video frames were recorded from the selected screen or window.".into())
        };
    }
    child
        .pipeline
        .send_event(gst::event::Eos::new())
        .then_some(())
        .ok_or_else(|| "GStreamer rejected the stop request".to_string())?;
    let bus = child
        .pipeline
        .bus()
        .ok_or_else(|| "GStreamer pipeline has no message bus".to_string())?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let result = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(message) = bus.timed_pop(gst::ClockTime::from_nseconds(remaining.as_nanos() as u64)) else {
            break Err("GStreamer did not finish the recording within 8 seconds".into());
        };
        match message.view() {
            gst::MessageView::Eos(..) => {
                child.pipeline_eos = true;
                break Ok(());
            }
            gst::MessageView::Error(error) => {
                break Err(format!(
                    "GStreamer pipeline failed: {} ({})",
                    error.error(),
                    error.debug().unwrap_or_default()
                ));
            }
            _ => {}
        }
    };
    let _ = child.pipeline.set_state(gst::State::Null);
    result?;
    if !child.has_video.load(Ordering::Acquire) {
        return Err("No video frames were recorded from the selected screen or window. Keep it visible and try recording again.".into());
    }
    Ok(())
}

fn finalize_segments(segments: &[PathBuf], output: &Path) -> Result<(), String> {
    let usable: Vec<_> = segments
        .iter()
        .filter(|path| path.metadata().map(|meta| meta.len() > 0).unwrap_or(false))
        .collect();
    if usable.is_empty() {
        return Err("the encoder produced no video".into());
    }
    // FFmpeg's concat demuxer can finish successfully after a bad later input,
    // leaving a playable but shortened file. Validate every source before
    // allowing installation or removal of any captured segment.
    if usable.len() != segments.len() || usable.iter().any(|path| !valid_finalized_media(path)) {
        return Err("Some captured segments are missing or incomplete; all original files were kept".into());
    }
    if usable.len() == 1 {
        fs::rename(usable[0], output)
            .or_else(|_| {
                let temporary = output.with_file_name(format!(".finalizing-{}.mkv", uuid::Uuid::new_v4()));
                if let Err(error) = fs::copy(usable[0], &temporary).and_then(|_| fs::rename(&temporary, output)) {
                    let _ = fs::remove_file(temporary);
                    return Err(error);
                }
                fs::remove_file(usable[0])
            })
            .map_err(|error| format!("could not install recording: {error}"))?;
        return Ok(());
    }

    let list = output.with_file_name(format!(".segments-{}.txt", uuid::Uuid::new_v4()));
    let contents = usable
        .iter()
        .map(|path| format!("file '{}'\n", path.to_string_lossy().replace('\'', "'\\''")))
        .collect::<String>();
    fs::write(&list, contents).map_err(|error| format!("could not list segments: {error}"))?;
    let temporary = output.with_file_name(format!(".finalizing-{}.mkv", uuid::Uuid::new_v4()));
    let result = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&list)
        .args(["-map", "0", "-c", "copy"])
        .arg(&temporary)
        .output()
        .map_err(|error| format!("could not start FFmpeg: {error}"))?;
    let _ = fs::remove_file(&list);
    if !result.status.success() || !valid_finalized_media(&temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "could not join paused recording segments: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    fs::rename(&temporary, output).map_err(|error| format!("could not install recording: {error}"))?;
    for path in usable {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn valid_finalized_media(path: &Path) -> bool {
    super::video::probe_media(path).is_ok_and(|info| {
        info.duration.is_finite() && info.duration > 0.0 && info.width > 0 && info.height > 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_separate_audio(path: &Path, microphone: bool, system: bool) {
        gst::init().unwrap();
        let mut description = format!(
            "matroskamux name=mux ! filesink location=\"{}\" \
             videotestsrc num-buffers=30 ! video/x-raw,width=96,height=64,framerate=30/1 ! \
             vp8enc deadline=1 ! queue ! mux.video_0 ", path.display(),
        );
        if microphone {
            description.push_str(&capture_audio_branch(
                "audiotestsrc num-buffers=100 samplesperbuffer=480 freq=440", "microphone", 0,
            ));
        }
        if system {
            description.push_str(&capture_audio_branch(
                "audiotestsrc num-buffers=100 samplesperbuffer=480 freq=880", "system", usize::from(microphone),
            ));
        }
        let pipeline = gst::parse::launch(&description).unwrap().downcast::<gst::Pipeline>().unwrap();
        tag_capture_audio(&pipeline);
        pipeline.use_clock(Some(&gst::SystemClock::obtain()));
        pipeline.set_state(gst::State::Playing).unwrap();
        let message = pipeline.bus().unwrap().timed_pop_filtered(
            gst::ClockTime::from_seconds(10), &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        pipeline.set_state(gst::State::Null).unwrap();
        match message.as_ref().map(|message| message.view()) {
            Some(gst::MessageView::Eos(_)) => {}
            other => panic!("synthetic capture did not finish: {other:?}"),
        }
    }

    fn audio_titles(path: &Path) -> Vec<String> {
        let probe = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream_tags=title", "-of", "json"])
            .arg(path).output().unwrap();
        assert!(probe.status.success(), "{}", String::from_utf8_lossy(&probe.stderr));
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        value["streams"].as_array().unwrap().iter().map(|stream| {
            stream["tags"].as_object().unwrap().iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("title"))
                .unwrap().1.as_str().unwrap().to_string()
        }).collect()
    }

    fn assert_isolated_tone(path: &Path, stream: usize, wanted: f64, unwanted: f64, start: f64) {
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-ss", &start.to_string(), "-i"])
            .arg(path).args(["-map", &format!("0:a:{stream}"), "-t", "0.25", "-ac", "1", "-ar", "48000", "-f", "f32le", "pipe:1"])
            .output().unwrap();
        assert!(decoded.status.success(), "{}", String::from_utf8_lossy(&decoded.stderr));
        let samples: Vec<_> = decoded.stdout.chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()) as f64).collect();
        assert!(samples.len() > 10_000, "audio disappeared from stream {stream}");
        let magnitude = |frequency: f64| {
            let (real, imaginary) = samples.iter().enumerate().fold((0.0, 0.0), |(real, imaginary), (index, sample)| {
                let phase = std::f64::consts::TAU * frequency * index as f64 / 48_000.0;
                (real + sample * phase.cos(), imaginary + sample * phase.sin())
            });
            real.hypot(imaginary) / samples.len() as f64
        };
        let wanted_level = magnitude(wanted);
        assert!(wanted_level > 0.2, "missing {wanted} Hz tone: {wanted_level}");
        assert!(magnitude(unwanted) < wanted_level * 0.03, "audio sources were mixed together");
    }

    #[test]
    fn isolated_audio_roles_survive_pause_join_and_recovery() {
        let root = std::env::temp_dir().join(format!("lahza-isolated-audio-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("screen.mkv");
        let segments: Vec<_> = (0..2).map(|index| segment_path(&output, index)).collect();
        let expected = vec!["Lahza-Microphone".to_string(), "Lahza-System-Audio".to_string()];
        for segment in &segments {
            synthetic_separate_audio(segment, true, true);
            assert_eq!(audio_titles(segment), expected);
        }
        recover_segment(&segments[0]).unwrap();
        assert_eq!(audio_titles(&segments[0]), expected, "source-loss recovery changed roles");
        finalize_segments(&segments, &output).unwrap();
        assert_eq!(audio_titles(&output), expected, "pause/resume dropped or reordered a source");
        assert!(segments.iter().all(|path| !path.exists()));
        let info = super::super::video::probe_media(&output).unwrap();
        assert!(info.duration > 1.9 && info.duration < 2.2, "{}", info.duration);
        for start in [0.25, 1.25] {
            assert_isolated_tone(&output, 0, 440.0, 880.0, start);
            assert_isolated_tone(&output, 1, 880.0, 440.0, start);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn single_audio_source_keeps_its_role_without_a_position_assumption() {
        let root = std::env::temp_dir().join(format!("lahza-single-audio-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("screen.mkv");
        for (microphone, system, title, frequency) in [
            (true, false, "Lahza-Microphone", 440.0),
            (false, true, "Lahza-System-Audio", 880.0),
        ] {
            let segment = segment_path(&output, 0);
            synthetic_separate_audio(&segment, microphone, system);
            finalize_segments(&[segment], &output).unwrap();
            assert_eq!(audio_titles(&output), vec![title]);
            assert_isolated_tone(&output, 0, frequency, if microphone { 880.0 } else { 440.0 }, 0.25);
        }
        synthetic_separate_audio(&output, false, false);
        assert!(audio_titles(&output).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_finalization_keeps_existing_output_and_every_source() {
        let root = std::env::temp_dir().join(format!("lahza-finalize-atomic-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("screen.mkv");
        synthetic_separate_audio(&output, true, true);
        let original_output = fs::read(&output).unwrap();
        let first = segment_path(&output, 0);
        fs::copy(&output, &first).unwrap();
        let last = segment_path(&output, 1);
        fs::write(&last, b"unfinished segment").unwrap();
        assert!(finalize_segments(&[first.clone(), last.clone()], &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), original_output);
        assert_eq!(fs::read(&first).unwrap(), original_output);
        assert_eq!(fs::read(&last).unwrap(), b"unfinished segment");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn countdown_publishes_remaining_time_and_cancels_before_capture() {
        let startup = Arc::new(RecordingStartup::default());
        let worker_startup = startup.clone();
        let worker = thread::spawn(move || wait_for_countdown(&worker_startup, 3, || false));
        let deadline = Instant::now() + Duration::from_secs(1);
        while startup.countdown_remaining().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(startup.countdown_remaining(), Some(3));
        let cancelled_at = Instant::now();
        startup.cancel();
        assert!(worker.join().unwrap().unwrap_err().contains("cancelled"));
        assert!(cancelled_at.elapsed() < Duration::from_millis(500));
        assert_eq!(startup.countdown_remaining(), None);
        assert!(wait_for_countdown(&RecordingStartup::default(), 0, || false).is_ok());
        assert!(wait_for_countdown(&RecordingStartup::default(), 3, || true).is_err());
    }

    #[test]
    fn source_picker_future_is_cancelled_without_waiting_for_its_response() {
        let startup = Arc::new(RecordingStartup::default());
        let worker_startup = startup.clone();
        let (result, response) = mpsc::channel();
        let (started, waiting) = mpsc::channel();
        let worker = thread::spawn(move || {
            let outcome = async_io::block_on(during_startup(&worker_startup, async move {
                started.send(()).unwrap();
                futures_util::future::pending::<Result<(), String>>().await
            }));
            result.send(outcome).unwrap();
        });
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        startup.cancel();
        assert!(response.recv_timeout(Duration::from_secs(1)).unwrap().unwrap_err().contains("cancelled"));
        worker.join().unwrap();
        let startup = RecordingStartup::default();
        assert_eq!(async_io::block_on(during_startup(&startup, async { Ok(42) })), Ok(42));
    }

    #[test]
    fn restart_skips_the_initial_countdown_but_still_honors_cancellation() {
        let startup = RecordingStartup::default();
        startup.started.store(true, Ordering::Release);
        let started = Instant::now();
        assert!(wait_for_countdown(&startup, 10, || false).is_ok());
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(startup.countdown_remaining(), None);
        startup.cancel();
        assert!(wait_for_countdown(&startup, 10, || false).is_err());
    }

    #[test]
    fn missing_webcam_warning_requires_a_successfully_saved_screen() {
        let recorder = NativeRecorder::with_options(RecordingOptions {
            camera: true,
            ..RecordingOptions::default()
        });
        assert!(recorder.capture_warnings().is_empty());
        recorder.state.lock().unwrap().completion = Some(Err("failed screen".into()));
        assert!(recorder.capture_warnings().is_empty());
        recorder.state.lock().unwrap().completion = Some(Ok(PathBuf::from("screen.mkv")));
        assert_eq!(recorder.capture_warnings().len(), 1);
        recorder.state.lock().unwrap().camera_recorded = true;
        assert!(recorder.capture_warnings().is_empty());
    }

    #[test]
    fn terminal_result_survives_a_closed_command_channel_and_repeated_stop() {
        let mut recorder = NativeRecorder::new();
        {
            let mut state = recorder.state.lock().unwrap();
            state.active = true;
            state.elapsed = Duration::from_millis(1200);
        }
        assert!(!recorder.has_stopped());
        recorder.state.lock().unwrap().paused = true;
        assert!(!recorder.has_stopped(), "pause must not trigger external-stop handling");
        mark_stopped(&recorder.state);
        assert!(recorder.has_stopped(), "UI can react before media finalization finishes");
        let shared = recorder.state.clone();
        let output = PathBuf::from("completed-screen.mkv");
        let expected = output.clone();
        recorder.worker = Some(thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            store_completion(&shared, Ok(output));
        }));
        assert_eq!(recorder.stop_recording().unwrap(), expected);
        assert_eq!(recorder.stop_recording().unwrap(), expected);
        let status = recorder.record_status().unwrap();
        assert!(!status.active && !status.paused);
        assert_eq!(status.duration_ms, 1200);
        store_completion(&recorder.state, Err("output could not be finalized".into()));
        assert!(recorder.has_stopped());
        assert!(recorder.stop_recording().unwrap_err().to_string().contains("output could not be finalized"));
        assert!(recorder.stop_recording().unwrap_err().to_string().contains("output could not be finalized"));
    }

    #[test]
    fn selected_source_geometry_preserves_logical_bounds_and_unknown_metadata() {
        let geometry = CaptureSourceGeometry::from_portal(
            Some((-1280, 120)), Some((1280, 720)), Some(SourceType::Monitor),
        ).unwrap();
        assert_eq!(geometry.position, (-1280, 120));
        assert_eq!(geometry.size, (1280, 720));
        assert!(geometry.is_monitor);
        assert!(!CaptureSourceGeometry::from_portal(
            Some((10, 20)), Some((640, 480)), Some(SourceType::Window),
        ).unwrap().is_monitor);
        assert!(CaptureSourceGeometry::from_portal(None, Some((1920, 1080)), Some(SourceType::Monitor)).is_none());
        assert!(CaptureSourceGeometry::from_portal(Some((0, 0)), Some((0, 1080)), Some(SourceType::Monitor)).is_none());
    }

    #[test]
    fn externally_ended_screen_drains_live_audio_and_installs_saved_output() {
        gst::init().unwrap();
        let root = std::env::temp_dir().join(format!("lahza-external-eos-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("screen.mkv");
        let pipeline = gst::parse::launch(&format!(
            "matroskamux name=mux ! filesink location=\"{}\" \
             videotestsrc name=screen_source is-live=true num-buffers=12 ! video/x-raw,width=96,height=64,framerate=30/1 ! \
             vp8enc name=screen_encoder deadline=1 ! queue ! mux. \
             audiotestsrc is-live=true ! audioconvert ! opusenc ! queue ! mux.",
            segment_path(&output, 0).display(),
        )).unwrap().downcast::<gst::Pipeline>().unwrap();
        let mut segment = SegmentPipeline::new(pipeline).unwrap();
        segment.pipeline.set_state(gst::State::Playing).unwrap();
        segment.wait_for_video(Duration::from_secs(3)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(end) = segment.take_end() {
                assert!(matches!(end, PipelineEnd::Eos));
                break;
            }
            assert!(Instant::now() < deadline, "screen EOS must be detected while audio remains live");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!segment.pipeline_eos, "live audio has not ended with the screen source");
        finalize_recording(&output, Some(segment), &[0], false, None).unwrap();
        let info = super::super::video::probe_media(&output).unwrap();
        assert!(info.duration > 0.2 && info.duration < 2.0, "{}", info.duration);
        assert!(info.has_audio);
        assert!(!segment_path(&output, 0).exists());

        // A paused take has no running child. Source loss still installs its
        // already-finalized screen and camera segments through the same path.
        fs::copy(&output, segment_path(&output, 0)).unwrap();
        fs::copy(&output, camera_segment_path(&output, 0)).unwrap();
        assert!(finalize_recording(&output, None, &[0], true, Some("source disconnected".into())).unwrap());
        assert!(super::super::video::probe_media(&camera_path(&output)).unwrap().duration > 0.2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn audio_cannot_commit_provisional_video_dimensions_to_the_recording_header() {
        gst::init().unwrap();
        let root = std::env::temp_dir().join(format!("lahza-recording-header-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for rect in [None, Some((7, 9, 32, 20))] {
            let path = root.join("screen.mkv");
            let pipeline = gst::parse::launch(&format!(
                "matroskamux name=mux ! filesink location=\"{}\" \
                 appsrc name=screen_source is-live=true format=time \
                 caps=video/x-raw,format=RGBA,width=64,height=48,framerate=30/1 ! \
                 {} ! queue ! vp8enc name=screen_encoder deadline=1 cpu-used=8 \
                 threads=4 target-bitrate=12000000 keyframe-max-dist=60 ! queue ! mux. \
                 audiotestsrc is-live=true samplesperbuffer=480 ! audioconvert ! \
                 audioresample ! opusenc name=audio_encoder ! queue ! mux.",
                path.display(), stream_geometry::FILTER,
            ))
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
            stream_geometry::attach(&pipeline, RecordingSize::default(), false).unwrap();
            let audio_frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let received = audio_frames.clone();
            pipeline.by_name("audio_encoder").unwrap().static_pad("src").unwrap()
                .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    received.fetch_add(1, Ordering::Release);
                    gst::PadProbeReturn::Ok
                });
            let video_pad = pipeline.by_name("screen_encoder").unwrap().static_pad("src").unwrap();
            let mut segment = SegmentPipeline::new(pipeline.clone()).unwrap();
            pipeline.set_state(gst::State::Playing).unwrap();
            // Let audio reach the mux while video has negotiated caps but has
            // not received the first buffer (and its possible crop metadata).
            let deadline = Instant::now() + Duration::from_secs(2);
            while audio_frames.load(Ordering::Acquire) < 6 || video_pad.current_caps().is_none() {
                assert!(Instant::now() < deadline, "initial negotiation must finish");
                thread::sleep(Duration::from_millis(5));
            }
            let source = pipeline.by_name("screen_source").unwrap();
            for frame in 0..3 {
                let mut buffer = gst::Buffer::from_mut_slice([20u8, 60, 100, 255].repeat(64 * 48));
                let buffer_ref = buffer.get_mut().unwrap();
                buffer_ref.set_pts(gst::ClockTime::from_mseconds(200 + frame * 34));
                buffer_ref.set_duration(gst::ClockTime::from_nseconds(33_333_333));
                if let Some(rect) = rect {
                    gstreamer_video::VideoCropMeta::add(buffer_ref, rect);
                }
                assert_eq!(source.emit_by_name::<gst::FlowReturn>("push-buffer", &[&buffer]), gst::FlowReturn::Ok);
                thread::sleep(Duration::from_millis(34));
            }
            segment.wait_for_video(Duration::from_secs(2)).unwrap();
            finalize_child(&mut segment).unwrap();
            // Inspect the stored header directly: video::probe's recovery for
            // old damaged recordings must not conceal a new capture regression.
            let output = Command::new("ffprobe").args([
                "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height",
                "-of", "csv=p=0:s=x",
            ]).arg(&path).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let expected = if rect.is_some() { "32x20" } else { "64x48" };
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recording_requires_video_before_reporting_ready() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch(
            "appsrc is-live=true format=time caps=video/x-raw,format=I420,width=64,height=64,framerate=30/1 ! \
             vp8enc name=screen_encoder deadline=1 ! matroskamux ! fakesink",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let mut segment = SegmentPipeline::new(pipeline.clone()).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let error = segment
            .wait_for_video(Duration::from_millis(80))
            .unwrap_err();
        assert!(error.contains("No video frames arrived"), "{error}");
        drop(segment);
        assert_eq!(pipeline.current_state(), gst::State::Null);
    }

    #[test]
    fn eos_without_video_is_not_a_successful_recording() {
        gst::init().unwrap();
        let root = std::env::temp_dir().join(format!("lahza-empty-video-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("empty.mkv");
        let pipeline = gst::parse::launch(&format!(
            "videotestsrc num-buffers=0 ! video/x-raw,width=64,height=64 ! \
             vp8enc name=screen_encoder deadline=1 ! matroskamux ! filesink location=\"{}\"",
            path.display()
        ))
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let mut segment = SegmentPipeline::new(pipeline.clone()).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let error = finalize_child(&mut segment).unwrap_err();
        assert!(error.contains("No video frames were recorded"), "{error}");
        assert_eq!(pipeline.current_state(), gst::State::Null);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires a live window node in LAHZA_TEST_PIPEWIRE_NODE"]
    fn live_window_cursor_capture_records_across_pause() {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        let node: u32 = std::env::var("LAHZA_TEST_PIPEWIRE_NODE")
            .unwrap()
            .parse()
            .unwrap();
        let root = std::env::temp_dir().join(format!("lahza-window-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let socket = std::env::var("XDG_RUNTIME_DIR").unwrap() + "/pipewire-0";
        let pointer_remote = UnixStream::connect(&socket).unwrap();
        let remote = UnixStream::connect(&socket).unwrap();
        let mut input = InputCapture::start(&root);
        let output = root.join("screen.mkv");
        let recording_size = RecordingSize::default();
        let mut segment = spawn_segment(
            &output,
            0,
            remote.into(),
            node,
            &RecordingOptions::default(),
            None,
            None,
            recording_size.clone(),
            true,
        )
        .unwrap();
        input
            .attach_pipewire(
                pointer_remote.as_raw_fd(),
                node,
                InputMapping {
                    origin: Some((0.0, 0.0)),
                    size: (640.0, 360.0),
                },
            )
            .unwrap();
        drop(pointer_remote);
        thread::sleep(Duration::from_secs(2));
        input.detach_pipewire();
        let cursor_before_pause = fs::read(root.join("input.cursor.jsonl")).unwrap();
        assert!(!cursor_before_pause.is_empty());
        finalize_child(&mut segment).unwrap();
        drop(segment);
        thread::sleep(Duration::from_millis(300));
        let resumed_remote = UnixStream::connect(&socket).unwrap();
        let mut resumed = spawn_segment(
            &output,
            1,
            resumed_remote.into(),
            node,
            &RecordingOptions::default(),
            None,
            None,
            recording_size.clone(),
            true,
        )
        .unwrap();
        let pointer_remote = UnixStream::connect(&socket).unwrap();
        input
            .attach_pipewire(
                pointer_remote.as_raw_fd(),
                node,
                InputMapping {
                    origin: Some((0.0, 0.0)),
                    size: (640.0, 360.0),
                },
            )
            .unwrap();
        drop(pointer_remote);
        thread::sleep(Duration::from_secs(2));
        finalize_child(&mut resumed).unwrap();
        for index in [0, 1] {
            let path = segment_path(&output, index);
            println!(
                "test recording: {} ({} bytes)",
                path.display(),
                path.metadata().unwrap().len()
            );
            let info = super::super::video::probe_media(&path).unwrap();
            assert!(info.duration > 1.0);
            assert_eq!((info.width, info.height), *recording_size.get().unwrap());
        }
        drop(input);
        let cursor_after_resume = fs::read(root.join("input.cursor.jsonl")).unwrap();
        assert!(cursor_after_resume.starts_with(&cursor_before_pause));
        assert!(cursor_after_resume.len() > cursor_before_pause.len());
        finalize_segments(
            &[segment_path(&output, 0), segment_path(&output, 1)],
            &output,
        )
        .unwrap();
        let info = super::super::video::probe_media(&output).unwrap();
        assert!(info.duration > 2.0);
        assert_eq!((info.width, info.height), *recording_size.get().unwrap());
        println!(
            "window recording dimensions: {}x{}",
            info.width, info.height
        );
        if let Some(destination) = std::env::var_os("LAHZA_TEST_RECORDING_COPY") {
            let destination = PathBuf::from(destination);
            fs::copy(&output, &destination).unwrap();
            fs::copy(
                output.with_extension("window.json"),
                destination.with_extension("window.json"),
            )
            .unwrap();
            let frame =
                super::super::video::decode_frame(&output, 0.2, info.width, info.height).unwrap();
            assert!(
                frame.rgba.chunks_exact(4).any(|pixel| pixel[3] == 0),
                "window margins must stay transparent"
            );
            let source = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba).unwrap();
            let compositor = super::super::scene::SceneCompositor::new(
                &super::super::scene::SceneStyle {
                    background: super::super::scene::SceneBackground::Solid(0x7bcbe8),
                    ..Default::default()
                },
                1000,
                700,
                frame.width,
                frame.height,
            )
            .unwrap();
            compositor
                .compose(super::super::scene::FrameInput {
                    source: &source,
                    overlay: None,
                    viewport: super::super::viewport::ViewportFrame::default(),
                    pointer: None,
                    camera: None,
                })
                .save(destination.with_extension("png"))
                .unwrap();
        }
        if std::env::var_os("LAHZA_TEST_RESIZE").is_some() {
            let first = super::super::video::decode_frame(&output, 0.2, 320, 180).unwrap();
            let wide = super::super::video::decode_frame(&output, 1.6, 320, 180).unwrap();
            println!(
                "native window sizes: {}x{} -> {}x{}",
                first.width, first.height, wide.width, wide.height
            );
            assert!(
                wide.width > 800 && wide.width > first.width,
                "growing the window must preserve its new physical resolution"
            );
            assert!(
                wide.width as f64 / wide.height as f64 > 1.4,
                "presentation must follow the wider aspect ratio"
            );
        }
        super::super::video::decode_frame(&output, info.duration * 0.75, 320, 180).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multiple_protected_windows_force_shared_memory_before_any_video_processing() {
        let root = std::env::temp_dir().join(format!("lahza-protected-pipeline-{}", uuid::Uuid::new_v4()));
        let exclusions = (0..3)
            .map(|_| CaptureExclusion::new_in(&root.join("requests")).unwrap())
            .collect();
        let options = RecordingOptions {
            capture_exclusions: exclusions,
            ..Default::default()
        };
        let output = root.join("screen.mkv");
        for preserve_alpha in [false, true] {
            for index in [0, 1] {
                let pipeline = build_segment_pipeline(
                    &output, index, -1, 0, &options, None, None, preserve_alpha,
                ).unwrap();
                let source = pipeline.by_name("screen_source").unwrap();
                let first_filter = source.static_pad("src").unwrap()
                    .peer().unwrap().parent_element().unwrap();
                assert_eq!(first_filter.name(), "screen_capture_memory");
                let caps = first_filter.property::<gst::Caps>("caps");
                let dmabuf: gst::Caps = "video/x-raw(memory:DMABuf),format=BGRA".parse().unwrap();
                let shared: gst::Caps = "video/x-raw(memory:SystemMemory),format=BGRA".parse().unwrap();
                assert!(
                    !caps.can_intersect(&dmabuf),
                    "DMA-BUF capture can include recording overlays",
                );
                assert!(caps.can_intersect(&shared));
            }
        }
        drop(options);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recording_pipeline_builds_with_installed_pipewire_plugin() {
        let root = std::env::temp_dir().join(format!("lahza-pipeline-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("screen.mkv");
        for (microphone, system_audio) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let options = RecordingOptions {
                area: None,
                capture_exclusions: Vec::new(),
                microphone,
                system_audio,
                microphone_device: None,
                camera: false,
                camera_device: None,
                ..Default::default()
            };
            // Build the real recording pipeline without opening the portal or
            // starting capture. Also run against the Snap's older plugin.
            let pipeline = build_segment_pipeline(&output, 0, -1, 0, &options, None, None, true)
                .expect("recording pipeline must support the installed PipeWire plugin");
            assert_eq!(pipeline.pipeline_clock(), gst::SystemClock::obtain());
            assert!(pipeline.by_name("screen_source").is_some());
            assert!(pipeline.by_name("screen_capture_memory").is_none());
            let cropped = RecordingOptions {
                area: RecordingArea::from_drag((0.25, 0.25), (0.75, 0.75), (1920, 1080)),
                ..options
            };
            for index in [0, 1] {
                // The same crop must build for both the initial segment and a
                // resumed segment, with every supported audio combination.
                build_segment_pipeline(&output, index, -1, 0, &cropped, None, None, false)
                    .expect("area recording pipeline must support pause/resume and audio");
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn segment_names_are_stable_and_stay_in_the_project() {
        let output = Path::new("/tmp/example.lahzarec/screen.mkv");
        assert_eq!(
            segment_path(output, 12),
            Path::new("/tmp/example.lahzarec/screen-segment-0012.mkv")
        );
    }

    #[test]
    #[ignore = "opens the desktop source picker and records a real PipeWire stream"]
    fn live_portal_pipewire_recording_produces_video() {
        let root =
            std::env::temp_dir().join(format!("lahza-native-smoke-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("screen.mkv");
        let mut recorder = NativeRecorder::with_options(RecordingOptions {
            area: None,
            capture_exclusions: Vec::new(),
            system_audio: true,
            microphone: true,
            microphone_device: None,
            camera: false,
            camera_device: None,
            ..Default::default()
        });
        recorder.start_recording(&output).unwrap();
        thread::sleep(Duration::from_secs(1));
        recorder.pause_recording().unwrap();
        assert!(recorder.record_status().unwrap().paused);
        thread::sleep(Duration::from_millis(400));
        recorder.resume_recording().unwrap();
        thread::sleep(Duration::from_secs(1));
        let finalized = recorder.stop_recording().unwrap();
        let info = crate::recording::video::probe_media(&finalized).unwrap();
        assert!(info.duration > 1.0);
        assert!(info.duration < 3.5);
        assert!(info.width > 0 && info.height > 0);
        assert!(info.has_audio);
        println!("native_recording={}", finalized.display());
        fs::remove_dir_all(root).unwrap();
    }
}
