//! Scene export: renders the full composition (background, media surface,
//! camera motion, pointer) frame by frame through the shared
//! [`SceneCompositor`] and encodes it with FFmpeg.
//!
//! Recordings and animated screenshots use the same pipeline; only the frame
//! source differs.

use image::RgbaImage;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use super::{
    clips::RecordingClipTimeline,
    pointer_timeline::PointerTimeline,
    scene::{FrameInput, PointerOverlay, SceneCompositor, SceneStyle},
    video::{
        probe_media, VideoError, VideoFrameStream,
    },
    viewport::{ViewportFrame, ViewportTimeline},
};

pub use super::export_encoder::VideoCodec;

/// Output height presets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExportResolution {
    /// The media's own height, kept within 720p–4K.
    #[default]
    Original,
    Hd720,
    Hd1080,
    Qhd1440,
    Uhd2160,
}

impl ExportResolution {
    pub const ALL: [ExportResolution; 5] = [
        ExportResolution::Original,
        ExportResolution::Hd720,
        ExportResolution::Hd1080,
        ExportResolution::Qhd1440,
        ExportResolution::Uhd2160,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExportResolution::Original => "Original",
            ExportResolution::Hd720 => "720p",
            ExportResolution::Hd1080 => "1080p",
            ExportResolution::Qhd1440 => "1440p",
            ExportResolution::Uhd2160 => "4K",
        }
    }

    pub fn canvas_height(self, source_height: u32) -> u32 {
        let height = match self {
            ExportResolution::Original => source_height.clamp(720, 2160),
            ExportResolution::Hd720 => 720,
            ExportResolution::Hd1080 => 1080,
            ExportResolution::Qhd1440 => 1440,
            ExportResolution::Uhd2160 => 2160,
        };
        (height / 2) * 2
    }
}

/// Rough output size for the export panel, from typical bits per pixel of
/// each encoder at the settings used here.
pub fn estimate_size_bytes(
    format: ExportFormat,
    codec: VideoCodec,
    width: u32,
    height: u32,
    frame_rate: f64,
    duration: f64,
) -> u64 {
    let pixels_per_second = width as f64 * height as f64 * frame_rate.max(1.0);
    let bits_per_pixel = match format {
        ExportFormat::Mp4 if codec == VideoCodec::Hevc => 0.055,
        ExportFormat::Mp4 => 0.09,
        ExportFormat::WebM => 0.06,
        ExportFormat::Gif => 0.45,
    };
    let audio_bits = match format {
        ExportFormat::Mp4 => 192_000.0,
        ExportFormat::WebM => 128_000.0,
        ExportFormat::Gif => 0.0,
    };
    ((pixels_per_second * bits_per_pixel + audio_bits) * duration.max(0.0) / 8.0) as u64
}

pub fn format_size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1_073_741_824.0 {
        format!("{:.1} GB", bytes / 1_073_741_824.0)
    } else if bytes >= 1_048_576.0 {
        format!("{:.0} MB", bytes / 1_048_576.0)
    } else {
        format!("{:.0} KB", (bytes / 1024.0).max(1.0))
    }
}

/// Canvas overlays are evaluated once at the output frame time.
pub type OverlaySource = Box<dyn FnMut(f64) -> Option<Arc<RgbaImage>> + Send>;

/// Media overlays receive the compositor's effective viewport, including the
/// camera split's cover crop. Reusing an Arc avoids rasterizing unchanged marks.
pub type MediaOverlaySource =
    Box<dyn FnMut(f64, ViewportFrame) -> Option<Arc<RgbaImage>> + Send>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    Mp4,
    WebM,
    Gif,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [ExportFormat::Mp4, ExportFormat::WebM, ExportFormat::Gif];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Mp4 => "MP4",
            ExportFormat::WebM => "WebM",
            ExportFormat::Gif => "GIF",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Mp4 => "mp4",
            ExportFormat::WebM => "webm",
            ExportFormat::Gif => "gif",
        }
    }

    pub fn supports_audio(self) -> bool {
        !matches!(self, ExportFormat::Gif)
    }

    /// GIF frames are expensive and palette-limited; keep them lighter.
    pub fn default_frame_rate(self) -> f64 {
        match self {
            ExportFormat::Gif => 15.0,
            _ => 30.0,
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|format| format.extension() == extension)
    }

    /// Replaces or appends the extension so the file matches the format.
    pub fn apply_to_path(self, path: &Path) -> PathBuf {
        path.with_extension(self.extension())
    }
}

/// Shared between the exporting thread and the UI.
#[derive(Debug, Default)]
pub struct ExportProgress {
    completed: AtomicU64,
    total: AtomicU64,
    cancelled: Arc<AtomicBool>,
    audio_preparation: Mutex<Option<super::audio_processing::ProcessingControl>>,
    encoder: Mutex<Option<String>>,
    compositor: Mutex<Option<String>>,
    encoding_started: Mutex<Option<Instant>>,
}

impl ExportProgress {
    pub fn remaining_label(&self) -> String {
        if let Some(control) = self.audio_preparation.lock().unwrap().as_ref() {
            return format!("Preparing audio… {:.0}%", control.fraction() * 100.0);
        }
        let elapsed = self.encoding_started.lock().unwrap().map(|start| start.elapsed());
        remaining_label(
            self.completed.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
            elapsed,
        )
    }

    pub fn encoder_label(&self) -> Option<String> {
        self.encoder.lock().ok().and_then(|label| label.clone())
    }

    pub fn compositor_label(&self) -> Option<String> {
        self.compositor.lock().ok().and_then(|label| label.clone())
    }

    fn set_compositor_label(&self, label: &str) {
        let mut current = self.compositor.lock().unwrap();
        if current.as_deref() != Some(label) { *current = Some(label.to_owned()); }
    }

    fn set_encoder_label(&self, label: &str) {
        *self.encoder.lock().unwrap() = Some(label.to_owned());
    }

    pub fn fraction(&self) -> f64 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.completed.load(Ordering::Relaxed) as f64 / total as f64).clamp(0.0, 1.0)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Restart the bar for a follow-up stage such as uploading.
    pub fn begin_stage(&self, total: u64) {
        self.reset(total);
        *self.encoding_started.lock().unwrap() = Some(Instant::now());
    }

    pub fn set_completed(&self, completed: u64) {
        self.completed.store(completed, Ordering::Relaxed);
    }

    fn reset(&self, total: u64) {
        *self.encoder.lock().unwrap() = None;
        *self.compositor.lock().unwrap() = None;
        *self.encoding_started.lock().unwrap() = None;
        self.total.store(total, Ordering::Relaxed);
        self.completed.store(0, Ordering::Relaxed);
    }
}

/// Measure only frame production: preparing edited sources and probing the
/// GPU should not inflate the estimated time for every remaining frame.
fn remaining_label(completed: u64, total: u64, elapsed: Option<Duration>) -> String {
    let Some(elapsed) = elapsed.filter(|_| total > 0) else {
        return "Preparing export…".into();
    };
    if completed >= total {
        // FFmpeg still has buffered frames and container metadata to write.
        return "Finalizing file…".into();
    }
    if completed < 2 || elapsed < Duration::from_secs(2) {
        return "Estimating time remaining…".into();
    }
    let seconds = (elapsed.as_secs_f64() * (total - completed) as f64 / completed as f64)
        .ceil().max(1.0) as u64;
    let time = if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    };
    format!("About {time} remaining")
}

/// Where the media frames come from.
pub enum SceneSource {
    Video {
        media: PathBuf,
        clips: RecordingClipTimeline,
        /// Reconstructed cursor in editor time, when the master is cursor-free.
        pointer: Option<PointerTimeline>,
        /// Camera (webcam) clip recorded alongside the master, if any.
        camera: Option<PathBuf>,
    },
    Image {
        image: RgbaImage,
        /// Synthetic cursor walkthrough in editor time, if any.
        pointer: Option<PointerTimeline>,
    },
}

/// A further image scene encoded right after the main source, with its
/// own camera motion, duration, and timed overlay.
pub struct ImageSegment {
    pub image: RgbaImage,
    pub pointer: Option<PointerTimeline>,
    pub viewport: ViewportTimeline,
    pub duration: f64,
    pub overlay: Option<MediaOverlaySource>,
    pub canvas_overlay: Option<OverlaySource>,
    pub media_start: f64,
    pub media_end: Option<f64>,
}

pub struct SceneExportRequest {
    pub destination: PathBuf,
    pub format: ExportFormat,
    pub frame_rate: f64,
    /// Output height in pixels; width follows the scene aspect ratio.
    pub canvas_height: u32,
    pub style: SceneStyle,
    pub viewport: ViewportTimeline,
    /// Editor-time duration of the scene in seconds.
    pub duration: f64,
    /// GIF only: loop forever instead of playing once.
    pub loop_forever: bool,
    /// MP4 only: H.264 or HEVC.
    pub video_codec: VideoCodec,
    /// Average sub-frame camera states while the camera moves.
    pub motion_blur: bool,
    /// Keep the recording's audio track (when the format supports it).
    pub include_audio: bool,
    pub audio_clips: Vec<(PathBuf, super::media::AudioClip)>,
    pub recording_duration: Option<f64>,
    /// Keystroke captions, speed badge and subtitles above the main scene.
    pub studio_overlays: Option<super::studio_overlays::StudioOverlays>,
    /// Suppress steady background noise in the exported audio.
    pub noise_reduction: bool,
    pub audio_mode: Option<super::audio_mode::AudioMode>,
    /// Per-frame media-space overlay, such as timed annotations.
    pub overlay: Option<MediaOverlaySource>,
    pub canvas_overlay: Option<OverlaySource>,
    pub media_start: f64,
    pub media_end: Option<f64>,
    /// Image scenes that follow the main source back to back.
    pub followers: Vec<ImageSegment>,
}

impl SceneExportRequest {
    pub fn new(
        destination: PathBuf,
        format: ExportFormat,
        canvas_height: u32,
        style: SceneStyle,
        viewport: ViewportTimeline,
        duration: f64,
    ) -> Self {
        Self {
            destination,
            format,
            frame_rate: format.default_frame_rate(),
            canvas_height,
            style,
            viewport,
            duration,
            loop_forever: true,
            video_codec: VideoCodec::H264,
            motion_blur: false,
            include_audio: true,
            audio_clips: Vec::new(),
            recording_duration: None,
            studio_overlays: None,
            noise_reduction: false,
            audio_mode: None,
            overlay: None,
            canvas_overlay: None,
            media_start: 0.0,
            media_end: None,
            followers: Vec::new(),
        }
    }
}

fn frames_for(duration: f64, frame_rate: f64) -> u64 {
    ((duration * frame_rate).ceil() as u64).max(1)
}

enum PreparedSource {
    Video {
        stream: VideoFrameStream,
        initial_size: (u32, u32),
        audio: Option<(Vec<super::media_bank::MediaInput>, String)>,
        pointer: Option<PointerTimeline>,
        camera: Option<(super::camera_playback::CameraPlaybackDecoder, f64)>,
        clips: RecordingClipTimeline,
        temporal_source: super::motion_blur::VideoSource,
        temporal_camera: Option<super::motion_blur::VideoSource>,
    },
    Image {
        image: RgbaImage,
        pointer: Option<PointerTimeline>,
    },
}

impl PreparedSource {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            PreparedSource::Video { initial_size, .. } => *initial_size,
            PreparedSource::Image { image, .. } => (image.width(), image.height()),
        }
    }

    fn cleanup(&mut self) {
        if let PreparedSource::Video {
            stream,
            camera,
            ..
        } = self
        {
            stream.stop();
            *camera = None;
        }
    }
}

/// Renders and encodes the scene. The destination is only replaced after
/// FFmpeg succeeds; cancelling removes every partial file.
pub fn export_scene(
    source: SceneSource,
    request: &mut SceneExportRequest,
    progress: &ExportProgress,
) -> Result<(), VideoError> {
    let frame_rate = if request.frame_rate.is_finite() {
        request.frame_rate.clamp(1.0, 60.0)
    } else {
        request.format.default_frame_rate()
    };
    if !request.duration.is_finite() || request.duration <= 0.0 {
        return Err(VideoError::InvalidMedia(
            "the scene has no duration to export".into(),
        ));
    }
    let main_frames = frames_for(request.duration, frame_rate);
    let total_frames = main_frames
        + request
            .followers
            .iter()
            .map(|segment| frames_for(segment.duration, frame_rate))
            .sum::<u64>();
    progress.reset(total_frames);
    if progress.is_cancelled() {
        return Err(VideoError::Cancelled);
    }

    let mut prepared = prepare_source(source, request, frame_rate, progress)?;
    let result = encode(&mut prepared, request, frame_rate, main_frames, progress);
    prepared.cleanup();
    result
}

fn prepare_source(
    source: SceneSource,
    request: &SceneExportRequest,
    frame_rate: f64,
    progress: &ExportProgress,
) -> Result<PreparedSource, VideoError> {
    match source {
        SceneSource::Image { image, pointer } => {
            if image.width() == 0 || image.height() == 0 {
                return Err(VideoError::InvalidMedia("the image is empty".into()));
            }
            Ok(PreparedSource::Image { image, pointer })
        }
        SceneSource::Video {
            media,
            clips,
            pointer,
            camera,
        } => {
            let info = probe_media(&media)?;
            let clips = clips.normalized(info.duration);
            let clips=super::media_bank::MediaBank::load(&media)?.map_or_else(||clips.clone(),|bank|bank.ranges(&clips));
            // Output size belongs to the compositor. Decode the full source
            // so zoomed regions can use its detail even in a smaller export.
            let stream = VideoFrameStream::open_timeline(
                &media, &clips, info.width, info.height, frame_rate,
            )?;
            let initial_size = if info.window_capture {
                let frame = super::video::decode_frame(&media, clips.segments.first().map_or(0.0, |clip| clip.source_start), info.width, info.height)?;
                (frame.width, frame.height)
            } else { stream.dimensions() };
            let audio = (info.has_audio && request.include_audio && request.format.supports_audio())
                .then(|| source_audio_filter(&media, &clips, 1, request.audio_mode.unwrap_or_else(|| request.noise_reduction.into()), progress)).transpose()?;
            let camera = camera.filter(|_| request.style.has_camera_content())
                .map(|path| {
                    let camera_info=probe_media(&path)?;
                    let size=(camera_info.width,camera_info.height);
                    let duration = camera_info.duration.min(request.recording_duration.unwrap_or(f64::INFINITY));
                    let temporal=super::motion_blur::VideoSource {path:path.clone(),size,revision:0};
                    Ok::<_, VideoError>((super::camera_playback::CameraPlaybackDecoder::new_with_size(path,size), duration,temporal))
                }).transpose()?;
            let temporal_camera=camera.as_ref().map(|(_,_,source)|source.clone());
            let camera=camera.map(|(decoder,duration,_)|(decoder,duration));
            Ok(PreparedSource::Video {
                stream,
                initial_size,
                audio,
                pointer,
                camera,
                clips,
                temporal_source:super::motion_blur::VideoSource {path:media,size:(info.width,info.height),revision:0},
                temporal_camera,
            })
        }
    }
}

/// Prepare each original take once, then apply timeline edits to the same
/// lossless samples used by playback. Each retained range opens independently
/// so a late trim does not decode the entire beginning of a recording.
pub(super) fn source_audio_filter(
    path: &Path,
    timeline: &RecordingClipTimeline,
    first_input: usize,
    audio_mode: impl Into<super::audio_mode::AudioMode>,
    progress: &ExportProgress,
) -> Result<(Vec<super::media_bank::MediaInput>, String), VideoError> {
    let audio_mode = audio_mode.into();
    use super::{audio_processing, media_bank::{MediaBank, MediaInput}};
    let control = audio_processing::ProcessingControl::with_cancel_flag(progress.cancelled.clone());
    *progress.audio_preparation.lock().unwrap() = Some(control.clone());
    let result = (|| {
        audio_processing::prepare_source(path, audio_mode, &control)?;
        let bank = MediaBank::load(path)?;
        let ranges = bank.as_ref().map_or_else(|| timeline.clone(), |bank| bank.ranges(timeline));
        let sources: Vec<_> = if let Some(bank) = &bank {
            bank.entries.iter().enumerate().map(|(index, entry)|
                (bank.entry_path(path, index), entry.info.has_audio && !entry.still)).collect()
        } else {
            vec![(path.to_owned(), probe_media(path)?.has_audio)]
        };
        let mut prepared = Vec::with_capacity(sources.len());
        for (source, has_audio) in sources {
            if progress.is_cancelled() { return Err(VideoError::Cancelled); }
            prepared.push(if has_audio {
                Some(audio_processing::ensure_audio(&source, audio_mode, &control)?.unwrap_or(source))
            } else { None });
        }
        let mut inputs = Vec::new();
        let mut filter = String::new();
        let mut count = 0;
        for clip in ranges.playback_ranges() {
            if clip.gap_before > 0.0 {
                filter.push_str(&format!("anullsrc=r=48000:cl=stereo,atrim=duration={:.9}[a{count}];", clip.gap_before));
                count += 1;
            }
            let (index, offset) = bank.as_ref().map_or((0, 0.0), |bank| bank.locate(clip.source_start));
            let duration = clip.editor_duration();
            if let Some(source) = &prepared[index] {
                let input = first_input + inputs.len();
                inputs.push(MediaInput {
                    path: source.clone(), still_duration: None,
                    seek: (clip.source_start - offset).max(0.0), duration: Some(clip.duration()),
                });
                // FFmpeg's WSOLA filter changes samples even at rate 1.0.
                // An unchanged-speed cut must reuse the prepared waveform;
                // only actual speed edits need time stretching.
                let tempo = if clip.speed == 1.0 { String::new() }
                    else { format!("atempo={:.9},", clip.speed) };
                filter.push_str(&format!("[{input}:a:0]atrim=start=0:end={:.9},asetpts=PTS-STARTPTS,{tempo}aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,apad,atrim=duration={duration:.9}[a{count}];", clip.duration()));
            } else {
                filter.push_str(&format!("anullsrc=r=48000:cl=stereo,atrim=duration={duration:.9}[a{count}];"));
            }
            count += 1;
        }
        if count == 0 { return Err(VideoError::InvalidMedia("the audio edit is empty".into())); }
        for index in 0..count { filter.push_str(&format!("[a{index}]")); }
        filter.push_str(&format!("concat=n={count}:v=0:a=1[audio]"));
        Ok((inputs, filter))
    })();
    *progress.audio_preparation.lock().unwrap() = None;
    result
}

/// Add the recording audio and music layers as FFmpeg inputs starting at
/// input `first_input`, returning a filter graph that ends in
/// `[export_audio]`, or `None` when there is nothing to hear.
fn add_audio_graph(
    command: &mut Command,
    first_input: usize,
    recording: Option<&(Vec<super::media_bank::MediaInput>, String)>,
    audio_clips: &[(PathBuf, super::media::AudioClip)],
    editor_duration: f64,
    output_duration: f64,
) -> Result<Option<String>, VideoError> {
    let mut filters = Vec::new();
    let mut labels = Vec::new();
    let mut next_input = first_input;
    if let Some((inputs, filter)) = recording {
        super::media_bank::add_inputs(command, inputs);
        filters.push(format!("{filter};[audio]anull[recording_audio]"));
        labels.push("[recording_audio]".to_string());
        next_input += inputs.len();
    }
    for (index, (path, clip)) in audio_clips.iter().enumerate() {
        clip.validate()?;
        if clip.start >= editor_duration {
            continue;
        }
        command.arg("-i").arg(path);
        let label = format!("music{index}");
        filters.push(clip.filter(next_input, &label));
        labels.push(format!("[{label}]"));
        next_input += 1;
    }
    if labels.is_empty() {
        return Ok(None);
    }
    filters.push(format!(
        "{}amix=inputs={}:normalize=0:duration=longest,alimiter=limit=0.95:level=0:latency=1,apad,atrim=duration={output_duration:.9}[export_audio]",
        labels.join(""),
        labels.len()
    ));
    Ok(Some(filters.join(";")))
}

/// Audio-only deliverables.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AudioFormat {
    #[default]
    M4a,
    Wav,
}

impl AudioFormat {
    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::M4a => "m4a",
            AudioFormat::Wav => "wav",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::M4a => "M4A",
            AudioFormat::Wav => "WAV",
        }
    }
}

/// Everything an audio-only export needs: the edited recording audio and
/// the music layers, mixed exactly as the video export mixes them.
pub struct AudioExportRequest {
    pub destination: PathBuf,
    pub format: AudioFormat,
    pub media: PathBuf,
    pub clips: RecordingClipTimeline,
    pub include_recording: bool,
    pub noise_reduction: bool,
    pub audio_mode: Option<super::audio_mode::AudioMode>,
    pub audio_clips: Vec<(PathBuf, super::media::AudioClip)>,
}

pub fn export_audio(request: AudioExportRequest, progress: &ExportProgress) -> Result<(), VideoError> {
    let info = probe_media(&request.media)?;
    let clips = request.clips.normalized(info.duration);
    let duration = clips.duration();
    if duration <= 0.0 {
        return Err(VideoError::InvalidMedia("the edit is empty".into()));
    }
    let recording = (info.has_audio && request.include_recording)
        .then(|| source_audio_filter(&request.media, &clips, 0, request.audio_mode.unwrap_or_else(|| request.noise_reduction.into()), progress))
        .transpose()?;
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin", "-y", "-progress", "pipe:1", "-nostats"]);
    let Some(graph) = add_audio_graph(
        &mut command,
        0,
        recording.as_ref(),
        &request.audio_clips,
        duration,
        duration,
    )?
    else {
        return Err(VideoError::InvalidMedia("this edit has no audio to export".into()));
    };
    command.arg("-filter_complex").arg(graph).args(["-map", "[export_audio]"]);
    match request.format {
        AudioFormat::M4a => command.args(["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-f", "ipod"]),
        AudioFormat::Wav => command.args(["-c:a", "pcm_s16le", "-f", "wav"]),
    };
    let temporary = temporary_sibling(&request.destination, request.format.extension());
    progress.reset(1000);
    *progress.encoding_started.lock().unwrap() = Some(Instant::now());
    let mut child = command
        .arg(&temporary)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| VideoError::Decode(format!("could not start FFmpeg: {error}")))?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let total_micros = duration * 1_000_000.0;
    let per_mille = Arc::new(AtomicU64::new(0));
    let reader_per_mille = per_mille.clone();
    // FFmpeg reports `out_time_us=` lines on stdout with -progress.
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(value) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                reader_per_mille.store(((value / total_micros).clamp(0.0, 1.0) * 1000.0) as u64, Ordering::Relaxed);
            }
        }
    });
    loop {
        progress.completed.store(per_mille.load(Ordering::Relaxed), Ordering::Relaxed);
        if progress.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&temporary);
            return Err(VideoError::Cancelled);
        }
        if child.try_wait()?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = reader.join();
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(VideoError::Decode(String::from_utf8_lossy(&output.stderr).trim().into()));
    }
    progress.completed.store(1000, Ordering::Relaxed);
    fs::rename(&temporary, &request.destination)?;
    Ok(())
}

fn temporal_decode_error(error:String,progress:&ExportProgress)->VideoError {
    if progress.is_cancelled() {VideoError::Cancelled} else {VideoError::Decode(error)}
}

fn encode(
    prepared: &mut PreparedSource,
    request: &mut SceneExportRequest,
    frame_rate: f64,
    main_frames: u64,
    progress: &ExportProgress,
) -> Result<(), VideoError> {
    let (source_width, source_height) = prepared.dimensions();
    let (canvas_width, canvas_height) =
        request
            .style
            .export_canvas_size(source_width, source_height, request.canvas_height);
    let mut compositor = SceneCompositor::new(
        &request.style,
        canvas_width,
        canvas_height,
        source_width,
        source_height,
    )
    .map_err(VideoError::InvalidMedia)?;
    // Drawn at the export resolution, so caption text stays sharp at 4K.
    let mut overlay_layers = request
        .studio_overlays
        .take()
        .filter(|overlays| !overlays.is_empty())
        .map(|overlays| {
            super::studio_overlays::OverlayLayerCache::new(
                overlays,
                (canvas_width, canvas_height),
                compositor.geometry().card(),
            )
        });

    let mut renderer = super::scene::gpu::Renderer::shared();
    progress.set_compositor_label(renderer.label());

    let audio = match prepared {
        PreparedSource::Video { audio, .. } => audio.clone(),
        PreparedSource::Image { .. } => None,
    };
    let temporary = temporary_sibling(&request.destination, request.format.extension());
    let encoder = if request.format == ExportFormat::Mp4 {
        progress.set_encoder_label("Checking GPU");
        let encoder = super::export_encoder::select_mp4_encoder(
            canvas_width, canvas_height, frame_rate, request.video_codec, progress,
        );
        progress.set_encoder_label(encoder.label());
        Some(encoder)
    } else {
        None
    };
    if progress.is_cancelled() {
        return Err(VideoError::Cancelled);
    }
    let mut command = Command::new("ffmpeg");
    if let Some(encoder) = &encoder {
        encoder.configure_input(&mut command);
    }
    command
        .args([
            "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s",
        ])
        .arg(format!("{canvas_width}x{canvas_height}"))
        .arg("-framerate")
        .arg(format!("{frame_rate:.3}"))
        .args(["-i", "pipe:0"]);
    let audio_duration = (main_frames + request.followers.iter().map(|s| frames_for(s.duration, frame_rate)).sum::<u64>()) as f64 / frame_rate;
    let has_audio = match add_audio_graph(
        &mut command,
        1,
        audio.as_ref(),
        if request.format.supports_audio() { &request.audio_clips } else { &[] },
        request.duration,
        audio_duration,
    )? {
        Some(graph) => {
            command.arg("-filter_complex").arg(graph);
            true
        }
        None => false,
    };
    match request.format {
        ExportFormat::Mp4 => {
            command.args(["-map", "0:v"]);
            if has_audio {
                command.args(["-map", "[export_audio]", "-c:a", "aac", "-b:a", "192k", "-shortest"]);
            }
            encoder.as_ref().unwrap().configure_output(&mut command, request.video_codec);
            command.args([
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
            ]);
        }
        ExportFormat::WebM => {
            command.args(["-map", "0:v"]);
            if has_audio {
                command.args([
                    "-map",
                    "[export_audio]",
                    "-c:a",
                    "libopus",
                    "-b:a",
                    "128k",
                    "-shortest",
                ]);
            }
            command.args([
                "-c:v",
                "libvpx-vp9",
                "-crf",
                "32",
                "-b:v",
                "0",
                "-deadline",
                "good",
                "-cpu-used",
                "4",
                "-row-mt",
                "1",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "webm",
            ]);
        }
        ExportFormat::Gif => {
            command.args([
                "-filter_complex",
                "[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle",
                "-loop",
                if request.loop_forever { "0" } else { "-1" },
                "-f",
                "gif",
            ]);
        }
    }
    let mut child = command
        .arg(&temporary)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| VideoError::Decode(format!("could not start FFmpeg encoder: {error}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| VideoError::Decode("FFmpeg encoder did not accept frame input".into()))?;

    *progress.encoding_started.lock().unwrap() = Some(Instant::now());
    let mut compositor_source_size = (source_width, source_height);
    let (temporal_pointer,temporal_clips,temporal_source,temporal_camera)=match &*prepared {
        PreparedSource::Video {pointer,clips,temporal_source,temporal_camera,..} =>
            (pointer.clone(),Some(clips.clone()),Some(temporal_source.clone()),temporal_camera.clone()),
        PreparedSource::Image {pointer,..}=>(pointer.clone(),None,None,None),
    };
    let mut screen_frames=super::motion_blur::FrameCache::default();
    let mut camera_frames=super::motion_blur::FrameCache::default();
    let mut render_style=request.style.clone();
    if request.motion_blur && render_style.animation.motion_blur==0 {render_style.animation.motion_blur=100;}
    let mut last_video_frame: Option<RgbaImage> = None;
    let mut last_camera_frame: Option<RgbaImage> = None;
    let mut write_error: Option<VideoError> = None;
    for index in 0..main_frames {
        if progress.is_cancelled() {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&temporary);
            return Err(VideoError::Cancelled);
        }
        let time = index as f64 / frame_rate;
        let (source_frame, pointer_overlay): (&RgbaImage, Option<PointerOverlay>) = match prepared {
            PreparedSource::Image { image, pointer } => (
                &*image,
                pointer
                    .as_ref()
                    .and_then(|pointer| pointer.frame_at(time))
                    .map(|frame| PointerOverlay { frame }),
            ),
            PreparedSource::Video {
                stream,
                pointer,
                camera,
                clips,
                ..
            } => {
                if let Some((camera, duration)) = camera.as_mut() {
                    let source_time = clips.content_source_time_at(time).filter(|time| *time < *duration);
                    match camera.frame_at(source_time, || progress.is_cancelled()) {
                        Ok(frame) => last_camera_frame = frame.map(|(_, image)| (*image).clone()),
                        Err(error) => { write_error = Some(error); break; }
                    }
                }
                match stream.next_frame() {
                    Ok(Some(frame)) => {
                        if let Some(image) =
                            RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                        {
                            last_video_frame = Some(image);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        write_error = Some(error);
                        break;
                    }
                }
                let Some(frame) = last_video_frame.as_ref() else {
                    write_error = Some(VideoError::Decode(
                        "the recording produced no video frames".into(),
                    ));
                    break;
                };
                let overlay = pointer
                    .as_ref()
                    .and_then(|pointer| pointer.frame_at(time))
                    .map(|frame| PointerOverlay { frame });
                (frame, overlay)
            }
        };
        if source_frame.dimensions() != compositor_source_size {
            compositor_source_size = source_frame.dimensions();
            match compositor.rebuild(&request.style, canvas_width, canvas_height,
                compositor_source_size.0, compositor_source_size.1) {
                Ok(next) => compositor = next,
                Err(error) => { write_error = Some(VideoError::InvalidMedia(error)); break; }
            }
        }
        let camera_aspect=last_camera_frame.as_ref().map_or(1.0,|frame|frame.width() as f64/frame.height().max(1) as f64);
        let plan=super::motion_blur::plan(&render_style,&request.viewport,temporal_pointer.as_ref(),temporal_clips.as_ref(),
            time,request.duration,frame_rate,(canvas_width,canvas_height),source_frame.dimensions(),camera_aspect,
            super::motion_blur::Quality::High);
        let canvas_overlay = request.canvas_overlay.as_mut().and_then(|source| source(time));
        let canvas_overlay = match overlay_layers.as_mut() {
            Some(layers) => layers.layer(time, canvas_overlay),
            None => canvas_overlay,
        };
        let media_visible = time >= request.media_start && request.media_end.is_none_or(|end| time < end);
        let rendered=(||->Result<RgbaImage,VideoError>{
            use super::motion_blur::Accumulator;
            let temporal=plan.samples.len()>1;
            let mut sum:Option<Accumulator>=None;
            for sample in &plan.samples {
                if progress.is_cancelled(){return Err(VideoError::Cancelled);}
                let decoded=if temporal {match (&temporal_source,sample.source_time){
                    (Some(video),Some(time))=>Some(screen_frames.frame(video,time,&||progress.is_cancelled()).map_err(|error|temporal_decode_error(error,progress))?),
                    _=>None,
                }} else {None};
                let source=decoded.as_deref().unwrap_or(source_frame);
                let camera=if temporal {match (&temporal_camera,sample.source_time.filter(|t|*t<request.recording_duration.unwrap_or(f64::INFINITY))){
                    (Some(video),Some(time)) if sample.camera.opacity()>0.0 && camera_frames.contains_time(video,time)
                        .map_err(|error|temporal_decode_error(error,progress))? =>
                        Some(camera_frames.frame(video,time,&||progress.is_cancelled()).map_err(|error|temporal_decode_error(error,progress))?),
                    _=>None,
                }}else{None};
                let camera=if sample.camera.opacity()>0.0 && sample.source_time.is_some_and(|t|
                    t<request.recording_duration.unwrap_or(f64::INFINITY)) {
                    if temporal {camera.as_deref()} else {last_camera_frame.as_ref()}
                } else {None};
                let mut style=render_style.clone();style.camera=sample.camera;
                style.pointer.motion_blur_amount=Some(if temporal {0.0}else{style.animation.blur_amount()});
                if compositor.style()!=&style || compositor.source_size()!=source.dimensions(){
                    compositor=compositor.rebuild(&style,canvas_width,canvas_height,source.width(),source.height())
                        .map_err(VideoError::InvalidMedia)?;
                    compositor_source_size=source.dimensions();
                }
                let overlay=request.overlay.as_mut().and_then(|source|source(sample.time, compositor.camera_viewport(sample.viewport)));
                let frame=renderer.compose_with_quality(&compositor,FrameInput {
                    source,overlay:overlay.as_deref(),viewport:sample.viewport,
                    pointer:sample.pointer.as_ref().or(pointer_overlay.as_ref()),camera,
                },media_visible,canvas_overlay.as_deref(),super::scene::gpu::RenderQuality {
                    enhance:true,motion_pixels:plan.motion_pixels,trail_pixels:plan.trail_pixels,
                });
                if let Some(error)=renderer.take_camera_error() {return Err(VideoError::Decode(error));}
                if let Some(sum)=&mut sum {sum.add(&frame);}else{sum=Some(Accumulator::new(frame));}
            }
            sum.map(Accumulator::finish).ok_or_else(||VideoError::InvalidMedia("empty shutter plan".into()))
        })();
        let output=match rendered {Ok(frame)=>frame,Err(error)=>{write_error=Some(error);break;}};
        progress.set_compositor_label(renderer.label());
        if let Err(error) = stdin.write_all(output.as_raw()) {
            write_error = Some(VideoError::Decode(format!(
                "FFmpeg stopped accepting frames: {error}"
            )));
            break;
        }
        progress.completed.store(index + 1, Ordering::Relaxed);
    }
    // Follower scenes continue in the same stream; each gets a compositor
    // for its own media size on the shared canvas.
    let mut written = main_frames;
    'followers: for segment in request.followers.iter_mut() {
        if write_error.is_some() {
            break;
        }
        let mut compositor = match SceneCompositor::new(
            &render_style,
            canvas_width,
            canvas_height,
            segment.image.width(),
            segment.image.height(),
        ) {
            Ok(compositor) => compositor,
            Err(error) => {
                write_error = Some(VideoError::InvalidMedia(error));
                break;
            }
        };
        for index in 0..frames_for(segment.duration, frame_rate) {
            if progress.is_cancelled() {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&temporary);
                return Err(VideoError::Cancelled);
            }
            let time = index as f64 / frame_rate;
            let canvas_overlay = segment.canvas_overlay.as_mut().and_then(|source| source(time));
            let plan=super::motion_blur::plan(&render_style,&segment.viewport,segment.pointer.as_ref(),None,
                time,segment.duration,frame_rate,(canvas_width,canvas_height),segment.image.dimensions(),1.0,
                super::motion_blur::Quality::High);
            let mut style=render_style.clone();style.pointer.motion_blur_amount=Some(if plan.samples.len()>1 {0.0}else{style.animation.blur_amount()});
            if compositor.style()!=&style {
                compositor=match compositor.rebuild(&style,canvas_width,canvas_height,segment.image.width(),segment.image.height()) {
                    Ok(scene)=>scene,Err(error)=>{write_error=Some(VideoError::InvalidMedia(error));break 'followers;}
                };
            }
            let mut sum:Option<super::motion_blur::Accumulator>=None;
            for sample in &plan.samples {
                if progress.is_cancelled() {write_error=Some(VideoError::Cancelled);break 'followers;}
                let overlay=segment.overlay.as_mut().and_then(|source|source(sample.time, compositor.camera_viewport(sample.viewport)));
                let frame=renderer.compose_with_quality(&compositor,FrameInput {
                    source:&segment.image,overlay:overlay.as_deref(),viewport:sample.viewport,
                    pointer:sample.pointer.as_ref(),camera:None,
                },time>=segment.media_start && segment.media_end.is_none_or(|end|time<end),canvas_overlay.as_deref(),
                super::scene::gpu::RenderQuality {enhance:true,motion_pixels:plan.motion_pixels,trail_pixels:plan.trail_pixels});
                if let Some(sum)=&mut sum {sum.add(&frame);}else{sum=Some(super::motion_blur::Accumulator::new(frame));}
            }
            let output=sum.unwrap().finish();
            progress.set_compositor_label(renderer.label());
            if let Err(error) = stdin.write_all(output.as_raw()) {
                write_error = Some(VideoError::Decode(format!(
                    "FFmpeg stopped accepting frames: {error}"
                )));
                break 'followers;
            }
            written += 1;
            progress.completed.store(written, Ordering::Relaxed);
        }
    }
    drop(stdin);
    let output = child.wait_with_output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if let Some(error) = write_error {
        let _ = fs::remove_file(&temporary);
        return Err(match error {
            VideoError::Decode(message) if !stderr.is_empty() => {
                VideoError::Decode(format!("{message} ({stderr})"))
            }
            other => other,
        });
    }
    if !output.status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(VideoError::Decode(format!(
            "could not encode {}: {stderr}",
            request.format.label()
        )));
    }
    fs::rename(&temporary, &request.destination)?;
    Ok(())
}

/// Camera states averaged per moving frame when motion blur is on.
const MOTION_BLUR_SAMPLES: usize = 6;

/// Whether the virtual camera moves noticeably across a shutter interval.
fn camera_moved(early: super::viewport::ViewportFrame, late: super::viewport::ViewportFrame) -> bool {
    (early.magnification - late.magnification).abs() > 1e-4
        || (early.anchor.x - late.anchor.x).abs() > 2e-5
        || (early.anchor.y - late.anchor.y).abs() > 2e-5
        || (early.tilt.x - late.tilt.x).abs() > 0.01
        || (early.tilt.y - late.tilt.y).abs() > 0.01
        || (early.tilt.z - late.tilt.z).abs() > 0.01
        || early.transform != late.transform
        || early.loupe != late.loupe
}

fn temporary_sibling(destination: &Path, extension: &str) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("export");
    destination.with_file_name(format!(".{name}.{}.tmp.{extension}", uuid::Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{
        model::NormalizedPoint,
        scene::SceneBackground,
        viewport::{MotionPreset, ZoomCue},
    };
    use image::Rgba;

    #[test]
    fn remaining_time_handles_preparation_sampling_and_finalization() {
        assert_eq!(remaining_label(0, 9000, None), "Preparing export…");
        assert_eq!(remaining_label(0, 0, Some(Duration::ZERO)), "Preparing export…");
        assert_eq!(remaining_label(0, 9000, Some(Duration::from_secs(10))), "Estimating time remaining…");
        assert_eq!(remaining_label(30, 9000, Some(Duration::from_secs(1))), "Estimating time remaining…");
        assert_eq!(remaining_label(100, 1000, Some(Duration::from_secs(10))), "About 1m 30s remaining");
        assert_eq!(remaining_label(500, 1000, Some(Duration::from_secs(10))), "About 10s remaining");
        assert_eq!(remaining_label(100, 1000, Some(Duration::from_secs(400))), "About 1h 0m remaining");
        assert_eq!(remaining_label(999, 1000, Some(Duration::from_secs(10))), "About 1s remaining");
        assert_eq!(remaining_label(1000, 1000, Some(Duration::from_secs(10))), "Finalizing file…");
    }

    #[test]
    fn resetting_progress_clears_the_previous_export_estimate() {
        let progress = ExportProgress::default();
        progress.reset(100);
        *progress.encoding_started.lock().unwrap() = Some(Instant::now() - Duration::from_secs(10));
        progress.completed.store(50, Ordering::Relaxed);
        progress.set_compositor_label("Vulkan · test GPU");
        progress.set_encoder_label("GPU · NVIDIA");
        assert!(progress.remaining_label().starts_with("About "));
        progress.reset(200);
        assert_eq!(progress.remaining_label(), "Preparing export…");
        assert_eq!(progress.fraction(), 0.0);
        assert!(progress.compositor_label().is_none());
        assert!(progress.encoder_label().is_none());
    }

    fn ffmpeg_available() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "lahza-scene-export-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn uhd_source_detail_survives_preview_and_export_preparation() {
        use super::super::{clips::RecordingClipSegment, playback::TimelinePlaybackStream, video};
        if !ffmpeg_available() { return; }
        let root = test_root("native-detail");
        let still = root.join("fine-lines.png");
        let media = root.join("screen.mkv");
        let (width, height) = (3840, 2160);
        // Single-pixel stems disappear if the source is reduced before zoom.
        // Keep the rest uniform so this fixture encodes quickly and losslessly.
        let source = RgbaImage::from_fn(width, height, |x, y| {
            if (1500..1756).contains(&x) && (900..1028).contains(&y) {
                let level = if (x + y / 16) % 2 == 0 { 255 } else { 0 };
                Rgba([level, level, level, 255])
            } else {
                Rgba([23, 47, 83, 255])
            }
        });
        source.save(&still).unwrap();
        let encoded = Command::new("ffmpeg")
            .args(["-v", "error", "-loop", "1", "-framerate", "30", "-i"])
            .arg(&still)
            .args(["-frames:v", "9", "-c:v", "ffv1", "-level", "3", "-pix_fmt", "bgra", "-threads", "2", "-y"])
            .arg(&media).output().unwrap();
        assert!(encoded.status.success(), "{}", String::from_utf8_lossy(&encoded.stderr));
        let info = probe_media(&media).unwrap();
        assert!(!info.window_capture, "exercise ordinary monitor/imported video, not the window exemption");
        assert_eq!((info.width, info.height), (width, height));
        let assert_detail = |frame: &video::DecodedFrame| {
            assert_eq!((frame.width, frame.height), (width, height));
            for y in 910..930 {
                for x in 1510..1740 {
                    let offset = ((y * width + x) * 4) as usize;
                    assert_eq!(&frame.rgba[offset..offset + 4], &source.get_pixel(x, y).0,
                        "source pixel {x},{y} changed before composition");
                }
            }
        };
        assert_detail(&video::decode_frame(&media, 0.1, width, height).unwrap());

        // The paused decoder must replace a lightweight scrub frame with
        // original detail when the settled request asks for native dimensions.
        let mut proxy = crate::timeline_preview::PreviewDecoder::open(media.clone(), (1280, 720)).unwrap();
        let frame = proxy.frame(0.1, &|| false).unwrap();
        assert_eq!((frame.width, frame.height), (1280, 720));
        drop(frame);
        drop(proxy);
        let mut paused = crate::timeline_preview::PreviewDecoder::open(media.clone(), (width, height)).unwrap();
        assert_detail(&paused.frame(0.1, &|| false).unwrap());
        drop(paused);

        // Both direct and edited exports must keep UHD source detail even
        // when the selected output canvas is only 1080p.
        for clips in [
            RecordingClipTimeline::full(info.duration),
            RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.05, 0.25)]),
        ] {
            let request = SceneExportRequest::new(root.join("output.mp4"), ExportFormat::Mp4,
                1080, SceneStyle::default(), ViewportTimeline::default(), clips.duration());
            let mut prepared = prepare_source(SceneSource::Video {
                media: media.clone(), clips, pointer: None, camera: None,
            }, &request, 30.0, &ExportProgress::default()).unwrap();
            assert_eq!(prepared.dimensions(), (width, height));
            let PreparedSource::Video { stream, .. } = &mut prepared else { unreachable!() };
            assert_detail(&stream.next_frame().unwrap().unwrap());
            prepared.cleanup();
        }

        // Exercise real-time native decoding without any camera or capture
        // device. Timings are diagnostic; correctness is independent of load.
        let mut playback = TimelinePlaybackStream::open(&media,
            RecordingClipTimeline::full(info.duration), 0.0, width, height, true).unwrap();
        let started = Instant::now();
        let mut arrivals = Vec::new();
        while let Some(frame) = playback.next_frame(|| started.elapsed() > Duration::from_secs(10)).unwrap() {
            assert_detail(&frame);
            arrivals.push(started.elapsed().as_secs_f64());
        }
        playback.stop();
        assert!(arrivals.len() >= 8, "native playback must decode the complete fixture");
        let steady = arrivals.last().unwrap() - arrivals.first().unwrap();
        eprintln!("UHD native decode: {} frames, startup {:.3}s, steady {:.3}s for {:.3}s of media",
            arrivals.len(), arrivals[0], steady, (arrivals.len() - 1) as f64 / 30.0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "set LAHZA_EXPORT_PROJECT to benchmark 20 seconds of a saved recording"]
    fn benchmark_saved_recording_export() {
        use crate::recording::model::RecordingSession;
        let session = RecordingSession {
            directory: std::env::var_os("LAHZA_EXPORT_PROJECT").expect("recording directory").into(),
        };
        let document = session.effective_edit_document().unwrap().unwrap();
        let style: SceneStyle = serde_json::from_value(document["scene"].clone()).unwrap();
        let info = probe_media(&session.screen_path()).unwrap();
        let clips = session.effective_clip_timeline(info.duration).unwrap();
        let capture = session.read_pointer_capture().unwrap();
        let pointer = PointerTimeline::build_with_clip_timeline(
            capture.clone(), info.duration, info.width as f64, info.height as f64,
            style.pointer.timeline_options(), Some(&clips),
        );
        let cues: Vec<super::super::viewport::ZoomCue> = document.get("zoomCues").cloned()
            .map(|value| serde_json::from_value(value).unwrap()).unwrap_or_default();
        let viewport = ViewportTimeline::build(&cues, &pointer, &clips, &capture);
        let marks: Vec<crate::AnnotationMark> = document.get("annotations").cloned()
            .map(|value| serde_json::from_value(value).unwrap()).unwrap_or_default();
        let root = test_root("benchmark");
        let destination = root.join("sample.mp4");
        let duration = clips.duration().min(20.0);
        let mut request = SceneExportRequest::new(destination.clone(), ExportFormat::Mp4,
            1080, style.clone(), viewport.clone(), duration);
        request.frame_rate = 30.0;
        request.noise_reduction = document["lahzaExtras"]["noiseReduction"].as_bool().unwrap_or(false);
        request.audio_mode = serde_json::from_value(document["lahzaExtras"]["audioMode"].clone()).ok();
        request.include_audio = !document["lahzaExtras"]["audioMuted"].as_bool().unwrap_or(false);
        let (width, height) = style.export_canvas_size(info.width, info.height, 1080);
        request.canvas_overlay = crate::scene_ui::canvas_overlay_source_for(marks.clone(), width as f64 / height as f64);
        request.overlay = crate::scene_ui::overlay_source_for(marks, info.width, info.height);
        let progress = ExportProgress::default();
        let start = Instant::now();
        export_scene(SceneSource::Video {
            media: session.screen_path(), clips, pointer: Some(pointer),
            camera: session.camera_path().exists().then(|| session.camera_path()),
        }, &mut request, &progress).unwrap();
        let elapsed = start.elapsed().as_secs_f64();
        eprintln!("Saved project: {duration:.1}s at {width}x{height}/30fps exported in {elapsed:.2}s ({:.2}x realtime), compositor={:?}, encoder={:?}, output={}",
            duration / elapsed, progress.compositor_label(), progress.encoder_label(), destination.display());
        let output = probe_media(&destination).unwrap();
        assert_eq!((output.width, output.height), (width, height));
        assert!((output.duration - duration).abs() < 0.15);
    }

    fn test_video(root: &Path) -> PathBuf {
        let path = root.join("source.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=12:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=2",
                "-c:v",
                "ffv1",
                "-c:a",
                "pcm_s16le",
                "-shortest",
                "-y",
            ])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    }

    fn style() -> SceneStyle {
        SceneStyle {
            background: SceneBackground::Gradient {
                colors: [0xfa4f94, 0x6652f2, 0x4ad6cc],
                angle_degrees: 135.0,
            },
            padding: 12,
            corners: 20,
            shadow: 40,
            shadow_style: 0,
            border: true,
            border_thickness: 20,
            border_color: 0xffc928,
            border_opacity: 100,
            aspect: Some(16.0 / 9.0),
            ..SceneStyle::default()
        }
    }

    #[test]
    fn streamed_export_preserves_reordering_speed_gaps_and_audio_without_intermediates() {
        if !ffmpeg_available() { return; }
        use crate::recording::clips::RecordingClipSegment;
        let root = test_root("streamed-edits");
        let media = root.join("colors.mkv");
        let output = Command::new("ffmpeg").args([
            "-v", "error", "-y",
            "-f", "lavfi", "-i", "color=red:s=160x90:r=20:d=1",
            "-f", "lavfi", "-i", "color=green:s=160x90:r=20:d=1",
            "-f", "lavfi", "-i", "color=blue:s=160x90:r=20:d=1",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
            "-filter_complex", "[0:v][1:v][2:v]concat=n=3:v=1:a=0[v]",
            "-map", "[v]", "-map", "3:a", "-c:v", "ffv1", "-c:a", "pcm_s16le",
        ]).arg(&media).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let mut blue = RecordingClipSegment::new(2.0, 3.0);
        blue.speed = 2.0;
        let mut red = RecordingClipSegment::new(0.0, 1.0);
        red.speed = 0.5;
        red.gap_before = 0.25;
        let clips = RecordingClipTimeline::new(vec![blue, red]);
        for format in [ExportFormat::Mp4, ExportFormat::WebM] {
            let destination = root.join(format!("streamed.{}", format.extension()));
            let mut style = SceneStyle {
                background: SceneBackground::Solid(0), padding: 0, corners: 0,
                shadow: 0, border: false, aspect: Some(16.0 / 9.0), ..Default::default()
            };
            style.camera.enabled = false;
            let mut request = SceneExportRequest::new(destination.clone(), format, 90,
                style, ViewportTimeline::default(), clips.duration());
            request.frame_rate = 20.0;
            let source = || SceneSource::Video {
                media: media.clone(), clips: clips.clone(), pointer: None, camera: Some(media.clone()),
            };
            let media_count = || fs::read_dir(&root).unwrap().filter_map(Result::ok)
                .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.')).count();
            let before = media_count();
            let mut prepared = prepare_source(source(), &request, 20.0, &ExportProgress::default()).unwrap();
            assert_eq!(media_count(), before, "preparation created intermediate video media");
            prepared.cleanup();
            export_scene(source(), &mut request, &ExportProgress::default()).unwrap();
            let info = probe_media(&destination).unwrap();
            assert!(info.has_audio);
            assert!((info.duration - 2.75).abs() < 0.15, "{}", info.duration);
            for (time, expected) in [(0.2, [0, 0, 255]), (0.6, [0, 0, 0]), (1.5, [255, 0, 0])] {
                let frame = super::super::video::decode_frame(&destination, time, 160, 90).unwrap();
                for (value, expected) in frame.rgba[..3].iter().zip(expected) {
                    assert!((i32::from(*value) - expected).abs() < 25, "{time}: {:?}", &frame.rgba[..3]);
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn temporal_video_export_matches_shared_shutter_with_camera_region_over_hidden_base() {
        use super::super::{camera_layout::CameraLayout, camera_timeline::CameraRegion,
            motion_blur::{self,Accumulator,FrameCache,Quality,VideoSource}, scene::CameraOverlay};
        let root=test_root("temporal-camera");
        let media=test_video(&root);
        let destination=root.join("shutter.mp4");
        let clips=RecordingClipTimeline::full(0.6);
        let mut style=SceneStyle {background:SceneBackground::Solid(0x172033),padding:0,corners:0,
            shadow:0,border:false,aspect:Some(16.0/9.0),..Default::default()};
        style.camera.enabled=false;
        style.animation.motion_blur=100;
        style.camera_regions.push(CameraRegion {start:0.0,end:0.6,transition:0.35,
            overlay:CameraOverlay {enabled:true,layout:CameraLayout::Fullscreen,..Default::default()},..Default::default()});
        let viewport=ViewportTimeline::build_static(&[],0.6);
        let time=0.2;
        let plan=motion_blur::plan(&style,&viewport,None,Some(&clips),time,0.6,10.0,
            (320,180),(320,180),16.0/9.0,Quality::High);
        assert!(plan.samples.len()>1,"fixture must exercise temporal composition");
        let descriptor=VideoSource {path:media.clone(),size:(320,180),revision:0};
        let mut frames=FrameCache::default();let mut expected:Option<Accumulator>=None;
        for sample in &plan.samples {
            let image=frames.frame(&descriptor,sample.source_time.unwrap(),&||false).unwrap();
            let mut sampled=style.clone();sampled.camera=sample.camera;
            sampled.pointer.motion_blur_amount=Some(0.0);
            let scene=SceneCompositor::new(&sampled,320,180,320,180).unwrap();
            let output=scene.compose_layers(FrameInput {source:&image,camera:Some(&image),
                viewport:sample.viewport,pointer:None,overlay:None},true,None);
            if let Some(sum)=&mut expected {sum.add(&output);}else{expected=Some(Accumulator::new(output));}
        }
        let expected=expected.unwrap().finish();
        let mut request=SceneExportRequest::new(destination.clone(),ExportFormat::Mp4,180,style,viewport,0.6);
        request.frame_rate=10.0;request.recording_duration=Some(0.6);
        export_scene(SceneSource::Video {media:media.clone(),clips,pointer:None,camera:Some(media)},
            &mut request,&ExportProgress::default()).unwrap();
        let actual=super::super::video::decode_frame(&destination,time,320,180).unwrap();
        let mean=actual.rgba.chunks_exact(4).zip(expected.pixels()).map(|(a,b)|
            (0..3).map(|c|a[c].abs_diff(b[c]) as f64).sum::<f64>()).sum::<f64>()/(320.0*180.0*3.0);
        assert!(mean<6.0,"temporal export diverged from CPU shutter reference: {mean:.3}/255");
        drop(frames);fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn format_round_trips_through_paths() {
        assert_eq!(
            ExportFormat::from_path(Path::new("a/b.MP4")),
            Some(ExportFormat::Mp4)
        );
        assert_eq!(
            ExportFormat::from_path(Path::new("clip.webm")),
            Some(ExportFormat::WebM)
        );
        assert_eq!(ExportFormat::from_path(Path::new("clip.png")), None);
        assert_eq!(
            ExportFormat::Gif.apply_to_path(Path::new("out/clip.mp4")),
            PathBuf::from("out/clip.gif")
        );
    }

    #[test]
    fn resolution_presets_and_size_estimates_are_sane() {
        assert_eq!(ExportResolution::Original.canvas_height(900), 900);
        assert_eq!(ExportResolution::Original.canvas_height(300), 720);
        assert_eq!(ExportResolution::Uhd2160.canvas_height(300), 2160);
        let small = estimate_size_bytes(ExportFormat::Mp4, VideoCodec::H264, 1280, 720, 30.0, 10.0);
        let large = estimate_size_bytes(ExportFormat::Mp4, VideoCodec::H264, 1920, 1080, 60.0, 10.0);
        assert!(small > 0 && large > small);
        assert_eq!(format_size(2 * 1_048_576), "2 MB");
        assert_eq!(format_size(512), "1 KB");
    }

    #[test]
    fn cancelled_export_produces_no_file() {
        let root = test_root("cancel");
        let destination = root.join("cancelled.gif");
        let progress = ExportProgress::default();
        progress.cancel();
        let mut request = SceneExportRequest::new(
            destination.clone(),
            ExportFormat::Gif,
            90,
            style(),
            ViewportTimeline::default(),
            1.0,
        );
        request.frame_rate = 10.0;
        let image = RgbaImage::from_pixel(64, 36, Rgba([200, 30, 30, 255]));
        let result = export_scene(
            SceneSource::Image {
                image,
                pointer: None,
            },
            &mut request,
            &progress,
        );
        assert!(matches!(result, Err(VideoError::Cancelled)));
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn animated_screenshot_exports_as_gif_and_mp4() {
        if !ffmpeg_available() {
            eprintln!("FFmpeg unavailable; skipping animated screenshot export test");
            return;
        }
        let root = test_root("image");
        let image = RgbaImage::from_fn(160, 90, |x, y| {
            Rgba([(x * 255 / 160) as u8, (y * 255 / 90) as u8, 120, 255])
        });
        let cues = MotionPreset::SlowZoomIn.cues(1.0);
        let viewport = ViewportTimeline::build_static(&cues, 1.0);
        for (format, expected_width) in [(ExportFormat::Gif, 320), (ExportFormat::Mp4, 320)] {
            let destination = root.join(format!("animated.{}", format.extension()));
            let progress = ExportProgress::default();
            let mut request = SceneExportRequest::new(
                destination.clone(),
                format,
                180,
                style(),
                viewport.clone(),
                1.0,
            );
            request.frame_rate = 10.0;
            // A timed overlay: red square for the first half only.
            request.overlay = Some(Box::new(move |time: f64, _viewport| {
                (time < 0.5).then(|| {
                    let mut layer = RgbaImage::new(160, 90);
                    for y in 30..60 {
                        for x in 60..100 {
                            layer.put_pixel(x, y, Rgba([255, 0, 0, 255]));
                        }
                    }
                    Arc::new(layer)
                })
            }));
            export_scene(
                SceneSource::Image {
                    image: image.clone(),
                    pointer: None,
                },
                &mut request,
                &progress,
            )
            .unwrap();
            assert!((progress.fraction() - 1.0).abs() < 1e-9);
            let info = probe_media(&destination).unwrap();
            assert_eq!((info.width, info.height), (expected_width, 180));
            assert!((info.duration - 1.0).abs() < 0.25, "{}", info.duration);
            assert!(!info.has_audio);
        }
        // No temporary files are left behind.
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn canvas_text_exports_after_the_image_clip_ends() {
        if !ffmpeg_available() { return; }
        let root = test_root("late-canvas-text");
        let destination = root.join("late.mp4");
        let style = SceneStyle {
            background: SceneBackground::Solid(0),
            padding: 0, corners: 0, shadow: 0, border: false,
            aspect: Some(16.0 / 9.0),
            ..SceneStyle::default()
        };
        let mut request = SceneExportRequest::new(destination.clone(), ExportFormat::Mp4,
            180, style, ViewportTimeline::default(), 1.2);
        request.frame_rate = 10.0;
        request.media_start = 0.2;
        request.media_end = Some(0.4);
        request.canvas_overlay = crate::scene_ui::canvas_overlay_source_for(vec![crate::AnnotationMark {
            tool: crate::Tool::Text,
            text: "AFTER".into(),
            canvas: true,
            color: 0xff0000,
            font_size: 100.0,
            start: crate::NormPoint { x: 0.05, y: 0.25 },
            end: crate::NormPoint { x: 0.45, y: 0.5 },
            timing: Some(crate::timed::AnnotationTiming {
                start: 0.6, end: 1.2,
                entrance: crate::timed::EntranceEffect::None,
                exit: crate::timed::ExitEffect::None,
                ..Default::default()
            }),
            ..Default::default()
        }], 16.0 / 9.0);
        export_scene(SceneSource::Image {
            image: RgbaImage::from_pixel(160, 90, Rgba([0, 255, 0, 255])), pointer: None,
        }, &mut request, &ExportProgress::default()).unwrap();
        assert!((probe_media(&destination).unwrap().duration - 1.2).abs() < 0.15);
        let before = super::super::video::decode_frame(&destination, 0.0, 320, 180).unwrap();
        assert!(before.rgba.chunks_exact(4).all(|p| p[0] < 10 && p[1] < 10 && p[2] < 10), "image has not started");
        let early = super::super::video::decode_frame(&destination, 0.3, 320, 180).unwrap();
        assert!(early.rgba.chunks_exact(4).any(|p| p[1] > 180 && p[0] < 50));
        let late = super::super::video::decode_frame(&destination, 0.8, 320, 180).unwrap();
        assert!(!late.rgba.chunks_exact(4).any(|p| p[1] > 180 && p[0] < 50), "image has ended");
        assert!(late.rgba.chunks_exact(4).any(|p| p[0] > 150 && p[1] < 80), "later canvas text is visible");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_sequence_exports_scenes_back_to_back() {
        if !ffmpeg_available() {
            eprintln!("FFmpeg unavailable; skipping sequence export test");
            return;
        }
        let root = test_root("sequence");
        let first = RgbaImage::from_pixel(160, 90, Rgba([200, 40, 40, 255]));
        // A follower with a different size and aspect shares the canvas.
        let second = RgbaImage::from_pixel(90, 120, Rgba([40, 40, 200, 255]));
        let cues = MotionPreset::SlowZoomIn.cues(1.0);
        let destination = root.join("sequence.mp4");
        let progress = ExportProgress::default();
        let mut request = SceneExportRequest::new(
            destination.clone(),
            ExportFormat::Mp4,
            180,
            style(),
            ViewportTimeline::build_static(&cues, 1.0),
            1.0,
        );
        request.frame_rate = 10.0;
        let follower_cues = MotionPreset::PanRight.cues(1.5);
        request.followers.push(ImageSegment {
            image: second,
            pointer: None,
            viewport: ViewportTimeline::build_static(&follower_cues, 1.5),
            duration: 1.5,
            overlay: None,
            canvas_overlay: None,
            media_start: 0.0,
            media_end: None,
        });
        export_scene(
            SceneSource::Image {
                image: first,
                pointer: None,
            },
            &mut request,
            &progress,
        )
        .unwrap();
        assert!((progress.fraction() - 1.0).abs() < 1e-9);
        let info = probe_media(&destination).unwrap();
        assert_eq!((info.width, info.height), (320, 180));
        assert!((info.duration - 2.5).abs() < 0.25, "{}", info.duration);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn video_crop_export_keeps_only_selected_pixels_and_preserves_audio_and_source() {
        if !ffmpeg_available() { return; }
        let root = test_root("crop");
        let media = root.join("source.mkv");
        let result = Command::new("ffmpeg").args([
            "-v", "error", "-f", "lavfi", "-i",
            "color=red:s=160x96:r=10:d=1,drawbox=x=80:y=0:w=80:h=96:color=lime:t=fill",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=1",
            "-c:v", "ffv1", "-c:a", "pcm_s16le", "-y",
        ]).arg(&media).status().unwrap();
        assert!(result.success());
        let original = fs::read(&media).unwrap();
        let destination = root.join("cropped.mp4");
        let style = SceneStyle {
            source_crop: crate::CropRect { x: 0.5, y: 0.0, width: 0.5, height: 1.0 },
            padding: 0, corners: 0, shadow: 0, border: false,
            ..SceneStyle::default()
        };
        let mut request = SceneExportRequest::new(destination.clone(), ExportFormat::Mp4, 96, style, ViewportTimeline::default(), 0.6);
        request.frame_rate = 10.0;
        export_scene(SceneSource::Video {
            media: media.clone(),
            clips: RecordingClipTimeline::new(vec![crate::recording::clips::RecordingClipSegment::new(0.2, 0.8)]),
            pointer: None, camera: None,
        }, &mut request, &ExportProgress::default()).unwrap();
        let info = probe_media(&destination).unwrap();
        assert_eq!((info.width, info.height), (80, 96));
        assert!(info.has_audio);
        assert!((info.duration - 0.6).abs() < 0.15);
        let frame = crate::recording::video::decode_frame(&destination, 0.3, 80, 96).unwrap();
        assert!(frame.rgba.chunks_exact(4).all(|p| p[1] > 200 && p[0] < 30 && p[2] < 30));
        assert_eq!(fs::read(&media).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recording_export_composes_edited_clips_with_audio() {
        if !ffmpeg_available() {
            eprintln!("FFmpeg unavailable; skipping recording export test");
            return;
        }
        let root = test_root("video");
        let media = test_video(&root);
        // Keep the middle second only, so the edited intermediate is used.
        let clips =
            RecordingClipTimeline::new(vec![crate::recording::clips::RecordingClipSegment::new(
                0.5, 1.5,
            )]);
        let cue = ZoomCue::pinned(0.6, 1.4, 2.0, NormalizedPoint { x: 0.2, y: 0.2 });
        let viewport = ViewportTimeline::build(
            &[cue],
            &PointerTimeline::default(),
            &clips,
            &Default::default(),
        );
        let destination = root.join("scene.mp4");
        let progress = ExportProgress::default();
        let mut request = SceneExportRequest::new(
            destination.clone(),
            ExportFormat::Mp4,
            180,
            style(),
            viewport,
            clips.duration(),
        );
        request.noise_reduction = true;
        request.frame_rate = 12.0;
        let camera = root.join("camera.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=green:size=96x72:rate=12:duration=2",
                "-c:v",
                "ffv1",
                "-y",
            ])
            .arg(&camera)
            .status()
            .unwrap();
        assert!(status.success());
        request.style.camera.size = 40;
        request.style.camera.margin = 2;
        request.style.camera.shadow = false;
        // Timed annotations travel as an overlay: the whole media turns red
        // for the first half second only (so the check holds under zoom).
        request.overlay = Some(Box::new(|time: f64, _viewport| {
            (time < 0.5).then(|| Arc::new(RgbaImage::from_pixel(320, 180, Rgba([255, 0, 0, 255]))))
        }));
        export_scene(
            SceneSource::Video {
                media,
                clips,
                pointer: None,
                camera: Some(camera),
            },
            &mut request,
            &progress,
        )
        .unwrap();
        let info = probe_media(&destination).unwrap();
        assert_eq!((info.width, info.height), (320, 180));
        assert!(info.has_audio);
        // The green camera bubble sits in the bottom-right corner.
        let frame = crate::recording::video::decode_frame(&destination, 0.5, 320, 180).unwrap();
        let rect = request.style.camera.rect(320.0, 180.0);
        let cx = (rect.x + rect.width * 0.5) as usize;
        let cy = (rect.y + rect.height * 0.5) as usize;
        let pixel = &frame.rgba[(cy * 320 + cx) * 4..(cy * 320 + cx) * 4 + 3];
        // FFmpeg's "green" is (0, 128, 0).
        assert!(
            pixel[1] > 100 && pixel[0] < 60 && pixel[2] < 60,
            "{pixel:?}"
        );
        assert!((info.duration - 1.0).abs() < 0.3, "{}", info.duration);
        // The overlay lands on the media, cropped and projected like the frame.
        let media = crate::recording::scene::SceneGeometry::layout(
            320.0,
            180.0,
            320.0,
            180.0,
            &request.style,
        )
        .media;
        let frame = crate::recording::video::decode_frame(&destination, 0.2, 320, 180).unwrap();
        let ox = (media.x + media.width * 0.5) as usize;
        let oy = (media.y + media.height * 0.5) as usize;
        let pixel = &frame.rgba[(oy * 320 + ox) * 4..(oy * 320 + ox) * 4 + 3];
        assert!(
            pixel[0] > 180 && pixel[1] < 80 && pixel[2] < 80,
            "overlay missing at {ox},{oy}: {pixel:?}"
        );
        // The gradient background is visible in the canvas corner.
        let frame = crate::recording::video::decode_frame(&destination, 0.5, 320, 180).unwrap();
        let corner = &frame.rgba[..4];
        assert!(corner[0] > 150 && corner[2] > 90, "{corner:?}");
        fs::remove_dir_all(root).unwrap();
    }
}
