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
        clip_filter, probe_media, VideoError, VideoFrameStream, NOISE_REDUCTION_FILTER,
    },
    viewport::ViewportTimeline,
};

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
    width: u32,
    height: u32,
    frame_rate: f64,
    duration: f64,
) -> u64 {
    let pixels_per_second = width as f64 * height as f64 * frame_rate.max(1.0);
    let bits_per_pixel = match format {
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

/// Produces the media-space overlay (e.g. timed annotations) for a frame
/// time; returning the same `Arc` for unchanged frames avoids re-rendering.
pub type OverlaySource = Box<dyn FnMut(f64) -> Option<Arc<RgbaImage>> + Send>;

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
    cancelled: AtomicBool,
    encoder: Mutex<Option<String>>,
    encoding_started: Mutex<Option<Instant>>,
}

impl ExportProgress {
    pub fn remaining_label(&self) -> String {
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

    fn reset(&self, total: u64) {
        *self.encoder.lock().unwrap() = None;
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
    pub overlay: Option<OverlaySource>,
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
    /// Keep the recording's audio track (when the format supports it).
    pub include_audio: bool,
    /// Suppress steady background noise in the exported audio.
    pub noise_reduction: bool,
    /// Per-frame media-space overlay, such as timed annotations.
    pub overlay: Option<OverlaySource>,
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
            include_audio: true,
            noise_reduction: false,
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
        audio: Option<(PathBuf, String)>,
        pointer: Option<PointerTimeline>,
        camera: Option<VideoFrameStream>,
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
            if let Some(camera) = camera.as_mut() {
                camera.stop();
            }
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

    let mut prepared = prepare_source(source, request, frame_rate)?;
    let result = encode(&mut prepared, request, frame_rate, main_frames, progress);
    prepared.cleanup();
    result
}

fn prepare_source(
    source: SceneSource,
    request: &SceneExportRequest,
    frame_rate: f64,
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
            let (width, height) = if request.style.source_crop != crate::CropRect::UNIT {
                (info.width, info.height)
            } else { (info.width.min(2560), info.height.min(1600)) };
            let stream = VideoFrameStream::open_timeline(&media, &clips, width, height, frame_rate)?;
            let initial_size = if info.window_capture {
                let frame = super::video::decode_frame(&media, clips.segments.first().map_or(0.0, |clip| clip.source_start), info.width, info.height)?;
                (frame.width, frame.height)
            } else { stream.dimensions() };
            let audio = (info.has_audio && request.include_audio && request.format.supports_audio())
                .then(|| (media, clip_filter(&info, &clips, 1, false, true)));
            let camera = camera.filter(|_| request.style.camera.enabled)
                .map(|path| VideoFrameStream::open_timeline(&path, &clips, 1280, 1280, frame_rate))
                .transpose()?;
            Ok(PreparedSource::Video {
                stream,
                initial_size,
                audio,
                pointer,
                camera,
            })
        }
    }
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

    let audio = match prepared {
        PreparedSource::Video { audio, .. } => audio.clone(),
        PreparedSource::Image { .. } => None,
    };
    let temporary = temporary_sibling(&request.destination, request.format.extension());
    let encoder = if request.format == ExportFormat::Mp4 {
        progress.set_encoder_label("Checking GPU");
        let encoder = super::export_encoder::select_mp4_encoder(
            canvas_width, canvas_height, frame_rate, progress,
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
    if let Some((path, filter)) = audio.as_ref() {
        command.arg("-i").arg(path);
        let filter = if request.noise_reduction {
            format!("{filter};[audio]{NOISE_REDUCTION_FILTER}[export_audio]")
        } else {
            format!("{filter};[audio]anull[export_audio]")
        };
        command.arg("-filter_complex").arg(filter);
    }
    match request.format {
        ExportFormat::Mp4 => {
            command.args(["-map", "0:v"]);
            if audio.is_some() {
                command.args(["-map", "[export_audio]", "-c:a", "aac", "-b:a", "192k", "-shortest"]);
            }
            encoder.as_ref().unwrap().configure_output(&mut command);
            command.args([
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
            ]);
        }
        ExportFormat::WebM => {
            command.args(["-map", "0:v"]);
            if audio.is_some() {
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
                ..
            } => {
                if let Some(camera) = camera.as_mut() {
                    if let Ok(Some(frame)) = camera.next_frame() {
                        if let Some(image) =
                            RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                        {
                            last_camera_frame = Some(image);
                        }
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
        let viewport = request.viewport.frame_at(time);
        let overlay = request.overlay.as_mut().and_then(|source| source(time));
        let canvas_overlay = request.canvas_overlay.as_mut().and_then(|source| source(time));
        let output = compositor.compose_layers(FrameInput {
            source: source_frame,
            overlay: overlay.as_deref(),
            viewport,
            pointer: pointer_overlay.as_ref(),
            camera: last_camera_frame.as_ref(),
        }, time >= request.media_start && request.media_end.is_none_or(|end| time < end), canvas_overlay.as_deref());
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
        let compositor = match SceneCompositor::new(
            &request.style,
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
            let pointer_overlay = segment
                .pointer
                .as_ref()
                .and_then(|pointer| pointer.frame_at(time))
                .map(|frame| PointerOverlay { frame });
            let overlay = segment.overlay.as_mut().and_then(|source| source(time));
            let canvas_overlay = segment.canvas_overlay.as_mut().and_then(|source| source(time));
            let output = compositor.compose_layers(FrameInput {
                source: &segment.image,
                overlay: overlay.as_deref(),
                viewport: segment.viewport.frame_at(time),
                pointer: pointer_overlay.as_ref(),
                camera: None,
            }, time >= segment.media_start && segment.media_end.is_none_or(|end| time < end), canvas_overlay.as_deref());
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
        assert!(progress.remaining_label().starts_with("About "));
        progress.reset(200);
        assert_eq!(progress.remaining_label(), "Preparing export…");
        assert_eq!(progress.fraction(), 0.0);
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
            let before = fs::read_dir(&root).unwrap().count();
            let mut prepared = prepare_source(source(), &request, 20.0).unwrap();
            assert_eq!(fs::read_dir(&root).unwrap().count(), before, "preparation created intermediate media");
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
        let small = estimate_size_bytes(ExportFormat::Mp4, 1280, 720, 30.0, 10.0);
        let large = estimate_size_bytes(ExportFormat::Mp4, 1920, 1080, 60.0, 10.0);
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
            let mut calls = 0usize;
            request.overlay = Some(Box::new(move |time: f64| {
                calls += 1;
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
        request.overlay = Some(Box::new(|time: f64| {
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
