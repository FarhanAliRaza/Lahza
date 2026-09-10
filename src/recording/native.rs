use super::area::{preview_frame, AreaRequest, RecordingArea};
use super::area_indicator::AreaIndicator;
use super::camera_preview::{attach_preview, preview_branch, CameraFrames};
use super::input::{monotonic_ns, ActiveRange, InputCapture, InputMapping};
use super::stream_geometry::{self, RecordingSize};
use ashpd::desktop::{
    screencast::{CursorMode, Screencast, SourceType},
    PersistMode,
};
use gst::prelude::*;
use gstreamer as gst;
use std::{
    fmt, fs, io,
    os::fd::{AsRawFd, OwnedFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
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

    fn includes_system_audio(&self) -> bool {
        false
    }

    fn includes_microphone(&self) -> bool {
        false
    }

    fn includes_camera(&self) -> bool {
        false
    }

    fn pointer_synthesized(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordingOptions {
    pub area: Option<RecordingArea>,
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
    pub fn new() -> Self {
        Self::with_options(RecordingOptions::default())
    }

    pub fn with_options(options: RecordingOptions) -> Self {
        Self {
            area_requests: None,
            camera_frames: None,
            options,
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
        let output = output.to_path_buf();
        let options = self.options.clone();
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
                    state,
                    commands_rx,
                    ready_tx,
                    camera_frames,
                    area_requests,
                )
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => {
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
        let result = self.command(WorkerCommand::Stop);
        self.commands = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        result
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
    state: Arc<Mutex<SharedState>>,
    commands: mpsc::Receiver<WorkerCommand>,
    ready: mpsc::Sender<Result<(), String>>,
    camera_frames: Option<Arc<CameraFrames>>,
    area_requests: Option<mpsc::Sender<AreaRequest>>,
) {
    let result = async_io::block_on(async {
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
        proxy
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
                if area_requests.is_some() {
                    SourceType::Monitor.into()
                } else {
                    SourceType::Monitor | SourceType::Window
                },
                false,
                None,
                PersistMode::ExplicitlyRevoked,
            )
            .await
            .map_err(|error| error.to_string())?;
        let response = proxy
            .start(&session, None)
            .await
            .map_err(|error| error.to_string())?
            .response()
            .map_err(|error| error.to_string())?;
        let stream = response
            .streams()
            .first()
            .ok_or_else(|| "no screen or window was selected".to_string())?;
        let node = stream.pipe_wire_node_id();
        let preserve_alpha = stream.source_type() == Some(SourceType::Window);
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

            let frame = match preview_frame(remote.as_raw_fd(), node) {
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
                .and_then(|_| {
                    response
                        .recv()
                        .ok()
                        .flatten()
                        .ok_or_else(|| "area selection was cancelled".to_string())
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
        let recording_size = RecordingSize::default();
        let mut segments = Vec::new();
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
        // Negotiate and receive video before adding the cursor consumer.
        // Starting the metadata-only stream first can leave a window's video
        // consumer without any frames, even though caps and EOS succeed.
        if let Some(input_capture) = input_capture
            .as_mut()
            .filter(|input| input.uses_pipewire_metadata())
        {
            let pointer_remote = proxy
                .open_pipe_wire_remote(&session)
                .await
                .map_err(|error| error.to_string())?;
            input_capture.attach_pipewire(pointer_remote.as_raw_fd(), node, input_mapping)?;
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
        let _ = ready.send(Ok(()));

        loop {
            if let Some(indicator) = indicator.as_mut() {
                indicator.tick();
            }
            if let Some(error) = child.as_mut().and_then(SegmentPipeline::take_failure) {
                return Err(error);
            }
            let command = match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if let Some(mut running) = child.take() {
                        let _ = finalize_child(&mut running);
                    }
                    break;
                }
            };
            match command {
                WorkerCommand::Pause(reply) => {
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
                    let _ = reply.send(result);
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
                                        let _ = reply.send(Err(error));
                                        continue;
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
                    drop(indicator.take());
                    let result = (|| {
                        if let Some(start_ns) = active_range_start.take() {
                            active_ranges.push(ActiveRange {
                                start_ns,
                                end_ns: monotonic_ns(),
                            });
                        }
                        if let Some(mut running) = child.take() {
                            finalize_child(&mut running)?;
                        }
                        let screen_segments: Vec<_> = segments
                            .iter()
                            .map(|index| segment_path(&output, *index))
                            .collect();
                        finalize_segments(&screen_segments, &output)?;
                        let camera_recorded = if camera.is_some() {
                            let camera_output = camera_path(&output);
                            let camera_segments: Vec<_> = segments
                                .iter()
                                .map(|index| camera_segment_path(&output, *index))
                                .collect();
                            // A webcam that produced no frames must not fail
                            // the screen recording; the editor just omits it.
                            match finalize_segments(&camera_segments, &camera_output) {
                                Ok(()) => true,
                                Err(error) => {
                                    eprintln!("Webcam recording was dropped: {error}");
                                    false
                                }
                            }
                        } else {
                            false
                        };
                        let pointer_synthesized =
                            input_capture.take().is_some_and(|input_capture| {
                                input_capture
                                    .finish(&active_ranges, input_mapping, &input_destination)
                                    .unwrap_or(false)
                            });
                        let mut shared = state.lock().expect("native recorder state poisoned");
                        if let Some(started) = shared.started_at.take() {
                            shared.elapsed += started.elapsed();
                        }
                        shared.active = false;
                        shared.paused = false;
                        shared.output = Some(output.clone());
                        shared.pointer_synthesized = pointer_synthesized;
                        shared.camera_recorded = camera_recorded;
                        Ok(output.clone())
                    })();
                    let _ = reply.send(result);
                    break;
                }
            }
        }
        let _ = session.close().await;
        Ok::<(), String>(())
    });

    if let Err(error) = result {
        let mut shared = state.lock().expect("native recorder state poisoned");
        shared.active = false;
        shared.failure = Some(error.clone());
        let _ = ready.send(Err(error));
    }
}

fn segment_path(output: &Path, index: usize) -> PathBuf {
    output.with_file_name(format!("screen-segment-{index:04}.mkv"))
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
    camera_devices()
        .into_iter()
        .next()
        .map(|(path, _)| path)
        .ok_or_else(|| "no webcam was found; unplug and replug it or turn the camera off".into())
}

/// Webcams GStreamer can capture from, as `(V4L2 device node, display name)`.
pub fn camera_devices() -> Vec<(String, String)> {
    if gst::init().is_err() {
        return Vec::new();
    }
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    if monitor.start().is_err() {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let devices = monitor
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
    devices
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
    // Keep the fd identity alive until GStreamer releases its cached core.
    _portal_remote: Option<OwnedFd>,
}

impl SegmentPipeline {
    fn new(pipeline: gst::Pipeline) -> Result<Self, String> {
        let pad = pipeline
            .by_name("screen_encoder")
            .and_then(|encoder| encoder.static_pad("src"))
            .ok_or_else(|| "GStreamer pipeline has no screen encoder output".to_string())?;
        let has_video = Arc::new(AtomicBool::new(false));
        let received = has_video.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            received.store(true, Ordering::Release);
            gst::PadProbeReturn::Remove
        });
        Ok(Self {
            pipeline,
            failure: None,
            has_video,
            _portal_remote: None,
        })
    }

    fn wait_for_video(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(error) = self.take_failure() {
                return Err(error);
            }
            if self.has_video.load(Ordering::Acquire) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("No video frames arrived from the selected screen or window. Keep it visible and try recording again.".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn take_failure(&mut self) -> Option<String> {
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
                        self.failure = Some("GStreamer stopped unexpectedly".into());
                        break;
                    }
                    _ => {}
                }
            }
        }
        self.failure.take()
    }
}

impl Drop for SegmentPipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

pub(super) const WINDOW_ENCODER: &str =
    "video/x-raw,format=BGRA ! avenc_ffv1 name=screen_encoder threads=0";

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
    let mut description = format!(
        "matroskamux name=mux ! filesink location=\"{}\" \
         pipewiresrc name=screen_source fd={} path={} always-copy=true \
         keepalive-time=1000 ! {} ! {crop_filter}\
         queue max-size-buffers=4 leaky=downstream ! \
         {encoder} ! queue ! mux. ",
        path.display(),
        portal_fd,
        node,
        if preserve_alpha {
            stream_geometry::NATIVE_WINDOW_FILTER
        } else {
            stream_geometry::FILTER
        },
    );
    // Sources capture at the device's native 48 kHz so PipeWire does not
    // resample for the client. A lone source feeds the encoder directly:
    // `audiomixer` is a live element with a deadline that discards buffers
    // arriving late and pads silence, which under encoder load clicks at
    // every 10 ms output block. Mixing two sources needs it, so give it a
    // latency budget large enough to absorb those stalls.
    const ENCODE: &str = "audioconvert ! audioresample ! opusenc bitrate=160000 ! queue ! mux. ";
    let audio_sources = [
        options
            .system_audio
            .then_some("device=@DEFAULT_MONITOR@".to_string()),
        options.microphone.then(|| {
            options
                .microphone_device
                .as_deref()
                .map(|name| format!("device=\"{name}\""))
                .unwrap_or_default()
        }),
    ];
    let audio_sources: Vec<String> = audio_sources.into_iter().flatten().collect();
    match audio_sources.as_slice() {
        [] => {}
        [source] => description.push_str(&format!(
            "pulsesrc {source} ! audio/x-raw,rate=48000 ! queue ! {ENCODE}"
        )),
        sources => {
            description.push_str(&format!(
                "audiomixer name=audio_mix latency=1000000000 ! {ENCODE}"
            ));
            for source in sources {
                description.push_str(&format!(
                    "pulsesrc {source} ! audio/x-raw,rate=48000 ! queue ! audio_mix. "
                ));
            }
        }
    }
    if let Some(device) = camera_device {
        // The webcam shares the screen pipeline's clock, so both files carry
        // the same running-time stamps and stay aligned in the editor.
        let camera_segment = camera_segment_path(output, index);
        let _ = fs::remove_file(&camera_segment);
        description.push_str(&format!(
            "v4l2src device=\"{}\" do-timestamp=true ! videoconvert ! tee name=camera_tee \
             camera_tee. ! queue max-size-buffers=4 leaky=downstream ! \
             vp8enc deadline=1 cpu-used=8 threads=2 target-bitrate=4000000 \
             keyframe-max-dist=60 ! queue ! matroskamux ! \
             filesink location=\"{}\" ",
            device,
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
    child
        .pipeline
        .send_event(gst::event::Eos::new())
        .then_some(())
        .ok_or_else(|| "GStreamer rejected the stop request".to_string())?;
    let bus = child
        .pipeline
        .bus()
        .ok_or_else(|| "GStreamer pipeline has no message bus".to_string())?;
    let result = loop {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_seconds(8)) else {
            break Err("GStreamer did not finish the recording within 8 seconds".into());
        };
        match message.view() {
            gst::MessageView::Eos(..) => break Ok(()),
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
    let _ = fs::remove_file(output);
    if usable.len() == 1 {
        fs::rename(usable[0], output)
            .or_else(|_| {
                fs::copy(usable[0], output)?;
                fs::remove_file(usable[0])
            })
            .map_err(|error| format!("could not install recording: {error}"))?;
        return Ok(());
    }

    let list = output.with_file_name("segments.txt");
    let contents = usable
        .iter()
        .map(|path| format!("file '{}'\n", path.display()))
        .collect::<String>();
    // Session paths are generated by Lahza and contain no quotes.
    fs::write(&list, contents).map_err(|error| format!("could not list segments: {error}"))?;
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
        .args(["-c", "copy"])
        .arg(output)
        .output()
        .map_err(|error| format!("could not start FFmpeg: {error}"))?;
    let _ = fs::remove_file(&list);
    if !result.status.success() {
        return Err(format!(
            "could not join paused recording segments: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    for path in usable {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn recording_pipeline_builds_with_installed_pipewire_plugin() {
        let root = std::env::temp_dir().join(format!("lahza-pipeline-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("screen.mkv");
        for (microphone, system_audio) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let options = RecordingOptions {
                area: None,
                microphone,
                system_audio,
                microphone_device: None,
                camera: false,
                camera_device: None,
            };
            // Build the real recording pipeline without opening the portal or
            // starting capture. Also run against the Snap's older plugin.
            let pipeline = build_segment_pipeline(&output, 0, -1, 0, &options, None, None, true)
                .expect("recording pipeline must support the installed PipeWire plugin");
            assert_eq!(pipeline.pipeline_clock(), gst::SystemClock::obtain());
            assert!(pipeline.by_name("screen_source").is_some());
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
            system_audio: true,
            microphone: true,
            microphone_device: None,
            camera: false,
            camera_device: None,
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
