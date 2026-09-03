use super::camera_preview::{self, CameraFrames};
use super::input::{monotonic_ns, ActiveRange, InputCapture, InputMapping};
use ashpd::desktop::{
    screencast::{CursorMode, Screencast, SourceType},
    PersistMode,
};
use gst::prelude::*;
use gstreamer as gst;
use std::{
    fmt, fs, io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
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
    pub system_audio: bool,
    pub microphone: bool,
    /// PulseAudio/PipeWire node name of the microphone; `None` records the
    /// system default source.
    pub microphone_device: Option<String>,
    /// Record the default webcam alongside the screen into `camera.mkv`.
    pub camera: bool,
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
    options: RecordingOptions,
    /// Receives downscaled webcam frames while recording with a camera.
    camera_preview: Option<Arc<CameraFrames>>,
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
            options,
            camera_preview: None,
            state: Arc::new(Mutex::new(SharedState::default())),
            commands: None,
            worker: None,
        }
    }

    /// Mirror the recorded webcam into `frames` for a live preview.
    pub fn with_camera_preview(mut self, frames: Arc<CameraFrames>) -> Self {
        self.camera_preview = Some(frames);
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
        let camera_preview = self.camera_preview.clone();
        let state = self.state.clone();
        let (commands_tx, commands_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("screendrop-native-recorder".into())
            .spawn(move || {
                run_worker(
                    output,
                    options,
                    camera_preview,
                    state,
                    commands_rx,
                    ready_tx,
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
    options: RecordingOptions,
    camera_preview: Option<Arc<CameraFrames>>,
    state: Arc<Mutex<SharedState>>,
    commands: mpsc::Receiver<WorkerCommand>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let result = async_io::block_on(async {
        let input_destination = output.with_file_name("input.json");
        let mut input_capture =
            InputCapture::start(output.parent().unwrap_or_else(|| Path::new(".")));
        let proxy = Screencast::new().await.map_err(|error| error.to_string())?;
        let session = proxy
            .create_session()
            .await
            .map_err(|error| error.to_string())?;
        proxy
            .select_sources(
                &session,
                if input_capture.uses_pipewire_metadata() {
                    CursorMode::Metadata
                } else if input_capture.is_active() {
                    CursorMode::Hidden
                } else {
                    CursorMode::Embedded
                },
                SourceType::Monitor | SourceType::Window,
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
        let input_mapping = InputMapping {
            origin: stream.position().map(|(x, y)| (f64::from(x), f64::from(y))),
            size: stream
                .size()
                .map(|(width, height)| (f64::from(width), f64::from(height)))
                .unwrap_or((1.0, 1.0)),
        };
        let pointer_remote = if input_capture.uses_pipewire_metadata() {
            Some(
                proxy
                    .open_pipe_wire_remote(&session)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let remote = proxy
            .open_pipe_wire_remote(&session)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(pointer_remote) = pointer_remote.as_ref() {
            input_capture.attach_pipewire(pointer_remote.as_raw_fd(), node, input_mapping)?;
        }

        let camera = if options.camera {
            Some(default_camera_device()?)
        } else {
            None
        };
        let mut segments = Vec::new();
        let mut child = Some(spawn_segment(
            &output,
            segments.len(),
            remote.as_raw_fd(),
            node,
            &options,
            camera.as_deref(),
            camera_preview.as_ref(),
        )?);
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
                        match spawn_segment(
                            &output,
                            index,
                            remote.as_raw_fd(),
                            node,
                            &options,
                            camera.as_deref(),
                            camera_preview.as_ref(),
                        ) {
                            Ok(next) => {
                                let path = segment_path(&output, index);
                                segments.push(index);
                                child = Some(next);
                                active_range_start = Some(monotonic_ns());
                                let mut shared =
                                    state.lock().expect("native recorder state poisoned");
                                shared.paused = false;
                                shared.started_at = Some(Instant::now());
                                shared.output = Some(path);
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    };
                    let _ = reply.send(result);
                }
                WorkerCommand::Stop(reply) => {
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
                        let pointer_synthesized = input_capture
                            .finish(&active_ranges, input_mapping, &input_destination)
                            .unwrap_or(false);
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
    gst::init().map_err(|error| format!("could not initialize GStreamer: {error}"))?;
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    monitor
        .start()
        .map_err(|_| "could not enumerate webcams".to_string())?;
    let device = monitor.devices().iter().find_map(|device| {
        let properties = device.properties()?;
        ["device.path", "api.v4l2.path"]
            .iter()
            .find_map(|key| properties.get::<String>(*key).ok())
    });
    monitor.stop();
    device.ok_or_else(|| "no webcam was found; unplug and replug it or turn the camera off".into())
}

struct SegmentPipeline {
    pipeline: gst::Pipeline,
    failure: Option<String>,
}

impl SegmentPipeline {
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

fn spawn_segment(
    output: &Path,
    index: usize,
    portal_fd: i32,
    node: u32,
    options: &RecordingOptions,
    camera_device: Option<&str>,
    camera_preview: Option<&Arc<CameraFrames>>,
) -> Result<SegmentPipeline, String> {
    let path = segment_path(output, index);
    let _ = fs::remove_file(&path);
    gst::init().map_err(|error| format!("could not initialize GStreamer: {error}"))?;
    let mut description = format!(
        "matroskamux name=mux ! filesink location=\"{}\" \
         pipewiresrc name=screen_source fd={} path={} always-copy=true \
         provide-clock=false keepalive-time=1000 ! videoconvert ! \
         queue max-size-buffers=4 leaky=downstream ! \
         vp8enc deadline=1 cpu-used=8 threads=4 target-bitrate=12000000 \
         keyframe-max-dist=60 ! queue ! mux. ",
        path.display(),
        portal_fd,
        node
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
            "v4l2src device=\"{}\" do-timestamp=true ! tee name=camera_tee ! \
             videoconvert ! queue max-size-buffers=4 leaky=downstream ! \
             vp8enc deadline=1 cpu-used=8 threads=2 target-bitrate=4000000 \
             keyframe-max-dist=60 ! queue ! matroskamux ! \
             filesink location=\"{}\" ",
            device,
            camera_segment.display()
        ));
        if camera_preview.is_some() {
            description.push_str("camera_tee. ! ");
            description.push_str(&camera_preview::preview_branch("camera_preview"));
        }
    }

    let pipeline = gst::parse::launch(&description)
        .map_err(|error| format!("could not build GStreamer pipeline: {error}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "GStreamer did not create a pipeline".to_string())?;
    if let Some(frames) = camera_preview.filter(|_| camera_device.is_some()) {
        camera_preview::attach_preview(&pipeline, "camera_preview", frames.clone())?;
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
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| format!("could not start GStreamer pipeline: {error}"))?;
    thread::sleep(Duration::from_millis(120));
    let mut segment = SegmentPipeline {
        pipeline,
        failure: None,
    };
    if let Some(error) = segment.take_failure() {
        let _ = segment.pipeline.set_state(gst::State::Null);
        return Err(error);
    }
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
    result
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
    // Session paths are generated by Screendrop and contain no quotes.
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
    fn segment_names_are_stable_and_stay_in_the_project() {
        let output = Path::new("/tmp/example.screendroprec/screen.mkv");
        assert_eq!(
            segment_path(output, 12),
            Path::new("/tmp/example.screendroprec/screen-segment-0012.mkv")
        );
    }

    #[test]
    #[ignore = "opens the desktop source picker and records a real PipeWire stream"]
    fn live_portal_pipewire_recording_produces_video() {
        let root =
            std::env::temp_dir().join(format!("screendrop-native-smoke-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("screen.mkv");
        let mut recorder = NativeRecorder::with_options(RecordingOptions {
            system_audio: true,
            microphone: true,
            microphone_device: None,
            camera: false,
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
