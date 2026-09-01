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
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use super::{
    clips::RecordingClipTimeline,
    pointer_timeline::PointerTimeline,
    scene::{PointerOverlay, SceneCompositor, SceneStyle},
    video::{probe_media, render_clip_preview, VideoError, VideoFrameStream},
    viewport::ViewportTimeline,
};

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
}

impl ExportProgress {
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
        self.total.store(total, Ordering::Relaxed);
        self.completed.store(0, Ordering::Relaxed);
    }
}

/// Where the media frames come from.
pub enum SceneSource {
    Video {
        media: PathBuf,
        clips: RecordingClipTimeline,
        /// Reconstructed cursor in editor time, when the master is cursor-free.
        pointer: Option<PointerTimeline>,
    },
    Image(RgbaImage),
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
}

enum PreparedSource {
    Video {
        stream: VideoFrameStream,
        audio: Option<PathBuf>,
        pointer: Option<PointerTimeline>,
        temporary: Option<PathBuf>,
    },
    Image(RgbaImage),
}

impl PreparedSource {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            PreparedSource::Video { stream, .. } => stream.dimensions(),
            PreparedSource::Image(image) => (image.width(), image.height()),
        }
    }

    fn cleanup(&mut self) {
        if let PreparedSource::Video {
            stream, temporary, ..
        } = self
        {
            stream.stop();
            if let Some(path) = temporary.take() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Renders and encodes the scene. The destination is only replaced after
/// FFmpeg succeeds; cancelling removes every partial file.
pub fn export_scene(
    source: SceneSource,
    request: &SceneExportRequest,
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
    let total_frames = ((request.duration * frame_rate).ceil() as u64).max(1);
    progress.reset(total_frames);
    if progress.is_cancelled() {
        return Err(VideoError::Cancelled);
    }

    let mut prepared = prepare_source(source, request, frame_rate)?;
    let result = encode(&mut prepared, request, frame_rate, total_frames, progress);
    prepared.cleanup();
    result
}

fn prepare_source(
    source: SceneSource,
    request: &SceneExportRequest,
    frame_rate: f64,
) -> Result<PreparedSource, VideoError> {
    match source {
        SceneSource::Image(image) => {
            if image.width() == 0 || image.height() == 0 {
                return Err(VideoError::InvalidMedia("the image is empty".into()));
            }
            Ok(PreparedSource::Image(image))
        }
        SceneSource::Video {
            media,
            clips,
            pointer,
        } => {
            let info = probe_media(&media)?;
            let clips = clips.normalized(info.duration);
            let (playback, temporary) = if clips.is_unedited(info.duration) {
                (media.clone(), None)
            } else {
                let temporary = temporary_sibling(&request.destination, "edit.mkv");
                render_clip_preview(&media, &temporary, &clips)?;
                (temporary.clone(), Some(temporary))
            };
            // Decode close to native resolution so zoomed regions stay sharp,
            // but cap very large masters to keep the CPU compositor responsive.
            let stream = match VideoFrameStream::open_with_frame_rate(
                &playback,
                0.0,
                info.width.min(2560),
                info.height.min(1600),
                Some(frame_rate),
            ) {
                Ok(stream) => stream,
                Err(error) => {
                    if let Some(path) = temporary {
                        let _ = fs::remove_file(path);
                    }
                    return Err(error);
                }
            };
            let audio = (info.has_audio && request.format.supports_audio()).then(|| playback);
            Ok(PreparedSource::Video {
                stream,
                audio,
                pointer,
                temporary,
            })
        }
    }
}

fn encode(
    prepared: &mut PreparedSource,
    request: &SceneExportRequest,
    frame_rate: f64,
    total_frames: u64,
    progress: &ExportProgress,
) -> Result<(), VideoError> {
    let (source_width, source_height) = prepared.dimensions();
    let (canvas_width, canvas_height) =
        request
            .style
            .export_canvas_size(source_width, source_height, request.canvas_height);
    let compositor = SceneCompositor::new(
        &request.style,
        canvas_width,
        canvas_height,
        source_width,
        source_height,
    )
    .map_err(VideoError::InvalidMedia)?;

    let audio = match prepared {
        PreparedSource::Video { audio, .. } => audio.clone(),
        PreparedSource::Image(_) => None,
    };
    let temporary = temporary_sibling(&request.destination, request.format.extension());
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s",
        ])
        .arg(format!("{canvas_width}x{canvas_height}"))
        .arg("-framerate")
        .arg(format!("{frame_rate:.3}"))
        .args(["-i", "pipe:0"]);
    if let Some(audio) = audio.as_ref() {
        command.arg("-i").arg(audio);
    }
    match request.format {
        ExportFormat::Mp4 => {
            command.args(["-map", "0:v"]);
            if audio.is_some() {
                command.args(["-map", "1:a:0", "-c:a", "aac", "-b:a", "192k", "-shortest"]);
            }
            command.args([
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
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
                    "1:a:0",
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

    let mut last_video_frame: Option<RgbaImage> = None;
    let mut write_error: Option<VideoError> = None;
    for index in 0..total_frames {
        if progress.is_cancelled() {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&temporary);
            return Err(VideoError::Cancelled);
        }
        let time = index as f64 / frame_rate;
        let (source_frame, pointer_overlay): (&RgbaImage, Option<PointerOverlay>) = match prepared {
            PreparedSource::Image(image) => (&*image, None),
            PreparedSource::Video {
                stream, pointer, ..
            } => {
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
        let viewport = request.viewport.frame_at(time);
        let output = compositor.compose(source_frame, viewport, pointer_overlay.as_ref());
        if let Err(error) = stdin.write_all(output.as_raw()) {
            write_error = Some(VideoError::Decode(format!(
                "FFmpeg stopped accepting frames: {error}"
            )));
            break;
        }
        progress.completed.store(index + 1, Ordering::Relaxed);
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
            "screendrop-scene-export-{label}-{}",
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
        }
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
    fn cancelled_export_produces_no_file() {
        let root = test_root("cancel");
        let destination = root.join("cancelled.gif");
        let progress = ExportProgress::default();
        progress.cancel();
        let request = SceneExportRequest {
            destination: destination.clone(),
            format: ExportFormat::Gif,
            frame_rate: 10.0,
            canvas_height: 90,
            style: style(),
            viewport: ViewportTimeline::default(),
            duration: 1.0,
            loop_forever: true,
        };
        let image = RgbaImage::from_pixel(64, 36, Rgba([200, 30, 30, 255]));
        let result = export_scene(SceneSource::Image(image), &request, &progress);
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
            let request = SceneExportRequest {
                destination: destination.clone(),
                format,
                frame_rate: 10.0,
                canvas_height: 180,
                style: style(),
                viewport: viewport.clone(),
                duration: 1.0,
                loop_forever: true,
            };
            export_scene(SceneSource::Image(image.clone()), &request, &progress).unwrap();
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
        let request = SceneExportRequest {
            destination: destination.clone(),
            format: ExportFormat::Mp4,
            frame_rate: 12.0,
            canvas_height: 180,
            style: style(),
            viewport,
            duration: clips.duration(),
            loop_forever: false,
        };
        export_scene(
            SceneSource::Video {
                media,
                clips,
                pointer: None,
            },
            &request,
            &progress,
        )
        .unwrap();
        let info = probe_media(&destination).unwrap();
        assert_eq!((info.width, info.height), (320, 180));
        assert!(info.has_audio);
        assert!((info.duration - 1.0).abs() < 0.3, "{}", info.duration);
        // The gradient background is visible in the canvas corner.
        let frame = crate::recording::video::decode_frame(&destination, 0.5, 320, 180).unwrap();
        let corner = &frame.rgba[..4];
        assert!(corner[0] > 150 && corner[2] > 90, "{corner:?}");
        fs::remove_dir_all(root).unwrap();
    }
}
