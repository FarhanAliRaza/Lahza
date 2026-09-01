use serde::Deserialize;
use std::{
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
};

use super::clips::RecordingClipTimeline;

#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub has_audio: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedFrame {
    pub time: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug)]
pub enum VideoError {
    Io(io::Error),
    Probe(String),
    Decode(String),
    InvalidMedia(String),
    Cancelled,
}

impl std::fmt::Display for VideoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Probe(error) => write!(formatter, "could not inspect video: {error}"),
            Self::Decode(error) => write!(formatter, "could not decode video: {error}"),
            Self::InvalidMedia(error) => error.fmt(formatter),
            Self::Cancelled => write!(formatter, "export cancelled"),
        }
    }
}

impl std::error::Error for VideoError {}

impl From<io::Error> for VideoError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Deserialize)]
struct ProbeOutput {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: String,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: String,
}

pub fn probe_media(path: &Path) -> Result<MediaInfo, VideoError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height,avg_frame_rate",
            "-of",
            "json",
        ])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(VideoError::Probe(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let probe: ProbeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| VideoError::Probe(error.to_string()))?;
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type == "video")
        .ok_or_else(|| VideoError::InvalidMedia("recording contains no video stream".into()))?;
    let duration = probe
        .format
        .duration
        .parse::<f64>()
        .map_err(|_| VideoError::InvalidMedia("recording has no valid duration".into()))?;
    let width = video
        .width
        .filter(|value| *value > 0)
        .ok_or_else(|| VideoError::InvalidMedia("recording has no valid width".into()))?;
    let height = video
        .height
        .filter(|value| *value > 0)
        .ok_or_else(|| VideoError::InvalidMedia("recording has no valid height".into()))?;
    Ok(MediaInfo {
        duration,
        width,
        height,
        frame_rate: video
            .avg_frame_rate
            .as_deref()
            .and_then(parse_frame_rate)
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(30.0),
        has_audio: probe
            .streams
            .iter()
            .any(|stream| stream.codec_type == "audio"),
    })
}

pub fn decode_frame(
    path: &Path,
    time: f64,
    maximum_width: u32,
    maximum_height: u32,
) -> Result<DecodedFrame, VideoError> {
    let info = probe_media(path)?;
    let (width, height) = fitted_dimensions(info.width, info.height, maximum_width, maximum_height);
    let selected_time = time.clamp(0.0, info.duration);
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-ss"])
        .arg(format!("{selected_time:.6}"))
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-an", "-vf"])
        .arg(format!("scale={width}:{height}:flags=lanczos,format=rgba"))
        .args(["-f", "rawvideo", "pipe:1"])
        .output()?;
    if !output.status.success() {
        return Err(VideoError::Decode(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    validate_frame_bytes(&output.stdout, width, height)?;
    Ok(DecodedFrame {
        time: selected_time,
        width,
        height,
        rgba: output.stdout,
    })
}

pub fn write_poster(
    video_path: &Path,
    destination: &Path,
    maximum_width: u32,
    maximum_height: u32,
) -> Result<(), VideoError> {
    let info = probe_media(video_path)?;
    let frame = decode_frame(
        video_path,
        (info.duration * 0.1).min(1.0),
        maximum_width,
        maximum_height,
    )?;
    let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
        .ok_or_else(|| VideoError::Decode("decoded poster had an invalid byte count".into()))?;
    let image = image::DynamicImage::ImageRgba8(image).to_rgb8();
    let temporary = temporary_sibling(destination);
    image
        .save_with_format(&temporary, image::ImageFormat::Jpeg)
        .map_err(|error| VideoError::Decode(error.to_string()))?;
    fs::rename(&temporary, destination)?;
    Ok(())
}

pub fn load_or_rebuild_poster(
    video_path: &Path,
    poster_path: &Path,
    maximum_width: u32,
    maximum_height: u32,
) -> Result<image::RgbaImage, VideoError> {
    if let Ok(poster) = image::open(poster_path) {
        return Ok(poster.to_rgba8());
    }
    if write_poster(video_path, poster_path, maximum_width, maximum_height).is_ok() {
        if let Ok(poster) = image::open(poster_path) {
            return Ok(poster.to_rgba8());
        }
    }
    let frame = decode_frame(video_path, 0.1, maximum_width, maximum_height)?;
    image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
        .ok_or_else(|| VideoError::Decode("decoded recording preview had invalid pixels".into()))
}

/// Builds the exact non-destructive clip composition used by Studio preview.
/// Video PTS and audio PTS are changed together for each clip, then the
/// resulting streams are concatenated in editor order. The source is never
/// modified and the destination is replaced only after FFmpeg succeeds.
pub fn render_clip_preview(
    source: &Path,
    destination: &Path,
    timeline: &RecordingClipTimeline,
) -> Result<(), VideoError> {
    render_clip_timeline(source, destination, timeline, "matroska")
}

/// Renders the same non-destructive clip composition into an MP4 suitable for
/// sharing outside Screendrop.
pub fn export_clip_timeline(
    source: &Path,
    destination: &Path,
    timeline: &RecordingClipTimeline,
) -> Result<(), VideoError> {
    render_clip_timeline(source, destination, timeline, "mp4")
}

fn render_clip_timeline(
    source: &Path,
    destination: &Path,
    timeline: &RecordingClipTimeline,
    container: &str,
) -> Result<(), VideoError> {
    let info = probe_media(source)?;
    let timeline = timeline.normalized(info.duration);
    if timeline.segments.is_empty() {
        return Err(VideoError::InvalidMedia(
            "recording edit contains no playable clips".into(),
        ));
    }

    // Gaps between clips render as black video with silent audio. Every
    // chain is normalized to one pixel/sample format so concat accepts the
    // generated fillers alongside the source segments.
    use std::fmt::Write as _;
    let frame_rate = if info.frame_rate.is_finite() && info.frame_rate > 0.0 {
        info.frame_rate
    } else {
        30.0
    };
    let mut filter = String::new();
    let mut chains: Vec<usize> = Vec::new();
    let mut label = 0usize;
    for clip in timeline.segments.iter() {
        if clip.gap_before > 0.0 {
            write!(
                filter,
                "color=c=black:s={}x{}:r={:.6}:d={:.9},format=yuv420p,setsar=1[v{label}];",
                info.width, info.height, frame_rate, clip.gap_before
            )
            .expect("writing to String cannot fail");
            if info.has_audio {
                write!(
                    filter,
                    "anullsrc=channel_layout=stereo:sample_rate=48000,atrim=0:{:.9}[a{label}];",
                    clip.gap_before
                )
                .expect("writing to String cannot fail");
            }
            chains.push(label);
            label += 1;
        }
        write!(
            filter,
            "[0:v]trim=start={:.9}:end={:.9},setpts=(PTS-STARTPTS)/{:.9},format=yuv420p,setsar=1[v{label}];",
            clip.source_start, clip.source_end, clip.speed
        )
        .expect("writing to String cannot fail");
        if info.has_audio {
            write!(
                filter,
                "[0:a]atrim=start={:.9}:end={:.9},asetpts=PTS-STARTPTS,atempo={:.9},aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[a{label}];",
                clip.source_start, clip.source_end, clip.speed
            )
            .expect("writing to String cannot fail");
        }
        chains.push(label);
        label += 1;
    }
    for index in &chains {
        write!(filter, "[v{index}]").expect("writing to String cannot fail");
        if info.has_audio {
            write!(filter, "[a{index}]").expect("writing to String cannot fail");
        }
    }
    write!(
        filter,
        "concat=n={}:v=1:a={}[video]{}",
        chains.len(),
        u8::from(info.has_audio),
        if info.has_audio { "[audio]" } else { "" }
    )
    .expect("writing to String cannot fail");

    let temporary = temporary_video_sibling(destination);
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-i"]).arg(source).args([
        "-filter_complex",
        &filter,
        "-map",
        "[video]",
    ]);
    if info.has_audio {
        command.args(["-map", "[audio]"]);
    }
    command.args([
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "18",
        "-pix_fmt",
        "yuv420p",
    ]);
    if info.has_audio {
        command.args(["-c:a", "aac", "-b:a", "192k"]);
    }
    if container == "mp4" {
        command.args(["-movflags", "+faststart"]);
    }
    let output = command
        .args(["-f", container, "-y"])
        .arg(&temporary)
        .output()?;
    if !output.status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(VideoError::Decode(format!(
            "could not build edited preview: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    fs::rename(&temporary, destination)?;
    Ok(())
}

/// Root-mean-square loudness per bucket across the whole file (0..1), for
/// the timeline's audio lane. Files without audio produce an empty list.
pub fn audio_levels(path: &Path, buckets: usize) -> Result<Vec<f32>, VideoError> {
    let info = probe_media(path)?;
    if !info.has_audio || buckets == 0 || info.duration <= 0.0 {
        return Ok(Vec::new());
    }
    const SAMPLE_RATE: usize = 8_000;
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "8000", "-f", "s16le", "pipe:1"])
        .output()?;
    if !output.status.success() {
        return Err(VideoError::Decode(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let samples: Vec<f32> = output
        .stdout
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / i16::MAX as f32)
        .collect();
    let expected = (info.duration * SAMPLE_RATE as f64) as usize;
    let total = samples.len().max(expected).max(1);
    let mut levels = vec![0.0f32; buckets];
    for (index, level) in levels.iter_mut().enumerate() {
        let start = index * total / buckets;
        let end = ((index + 1) * total / buckets).min(samples.len());
        if end > start && start < samples.len() {
            let slice = &samples[start..end];
            let energy: f32 = slice.iter().map(|value| value * value).sum();
            *level = (energy / slice.len() as f32).sqrt();
        }
    }
    let peak = levels.iter().cloned().fold(0.0f32, f32::max);
    if peak > 0.0 {
        for level in &mut levels {
            *level = (*level / peak).clamp(0.0, 1.0);
        }
    }
    Ok(levels)
}

/// `count` evenly spaced thumbnails at `height` pixels, for the clip lane.
pub fn decode_thumbnails(
    path: &Path,
    count: usize,
    height: u32,
) -> Result<Vec<image::RgbaImage>, VideoError> {
    let info = probe_media(path)?;
    if count == 0 || info.duration <= 0.0 {
        return Ok(Vec::new());
    }
    let height = (height.max(8) / 2) * 2;
    let width = ((height as f64 * info.width as f64 / info.height as f64).round() as u32).max(2);
    let width = (width / 2) * 2;
    let interval = info.duration / count as f64;
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-an", "-vf"])
        .arg(format!(
            "fps=1/{interval:.6}:start_time=0,scale={width}:{height}:flags=area,format=rgba"
        ))
        .args(["-frames:v"])
        .arg(count.to_string())
        .args(["-f", "rawvideo", "pipe:1"])
        .output()?;
    if !output.status.success() {
        return Err(VideoError::Decode(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let frame_bytes = frame_byte_count(width, height)?;
    Ok(output
        .stdout
        .chunks_exact(frame_bytes)
        .filter_map(|chunk| image::RgbaImage::from_raw(width, height, chunk.to_vec()))
        .collect())
}

pub struct VideoFrameStream {
    child: Child,
    stdout: ChildStdout,
    width: u32,
    height: u32,
    frame_rate: f64,
    next_frame_index: u64,
    start_time: f64,
}

/// Runtime-only GStreamer transport. `fdsink sync=true` and playbin's audio
/// sink share one pipeline clock, so the bytes handed to GPUI are scheduled
/// against the audio the user hears. Pause and seek intentionally stop and
/// recreate this process at the last frame timestamp; a suspended process
/// would leave the pipeline clock running and resume late.
pub struct SynchronizedPlaybackStream {
    child: Child,
    stdout: ChildStdout,
    width: u32,
    height: u32,
    frame_rate: f64,
    next_frame_index: u64,
    start_time: f64,
}

impl SynchronizedPlaybackStream {
    pub fn open(
        path: &Path,
        start_time: f64,
        maximum_width: u32,
        maximum_height: u32,
    ) -> Result<Self, VideoError> {
        let info = probe_media(path)?;
        let (width, height) =
            fitted_dimensions(info.width, info.height, maximum_width, maximum_height);
        let start_time = start_time.clamp(0.0, info.duration);
        let frame_rate = info.frame_rate.round().clamp(1.0, 120.0);
        let sink = format!(
            "videoconvert ! videoscale method=lanczos ! videorate ! video/x-raw,format=RGBA,width={width},height={height},framerate={frame_rate:.0}/1,pixel-aspect-ratio=1/1 ! fdsink fd=1 sync=true async=false"
        );
        let mut child = Command::new("gst-play-1.0")
            .args([
                "--quiet",
                "--no-position",
                "--no-interactive",
                "--accurate-seeks",
                "--start-position",
            ])
            .arg(format!("{start_time:.6}"))
            .arg("--videosink")
            .arg(sink)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                VideoError::Decode(format!(
                    "could not start synchronized GStreamer playback: {error}"
                ))
            })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            VideoError::Decode("GStreamer did not provide preview frame output".into())
        })?;
        Ok(Self {
            child,
            stdout,
            width,
            height,
            frame_rate,
            next_frame_index: 0,
            start_time,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn frame_rate(&self) -> f64 {
        self.frame_rate
    }

    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, VideoError> {
        let expected = frame_byte_count(self.width, self.height)?;
        let mut rgba = vec![0; expected];
        let mut read = 0;
        while read < expected {
            match self.stdout.read(&mut rgba[read..]) {
                Ok(0) if read == 0 => return Ok(None),
                Ok(0) => {
                    return Err(VideoError::Decode(format!(
                        "GStreamer ended halfway through a frame ({read}/{expected} bytes)"
                    )));
                }
                Ok(count) => read += count,
                Err(error) => return Err(error.into()),
            }
        }
        let time = self.start_time + self.next_frame_index as f64 / self.frame_rate;
        self.next_frame_index += 1;
        Ok(Some(DecodedFrame {
            time,
            width: self.width,
            height: self.height,
            rgba,
        }))
    }

    pub fn position(&self) -> f64 {
        self.start_time + self.next_frame_index as f64 / self.frame_rate
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for SynchronizedPlaybackStream {
    fn drop(&mut self) {
        self.stop();
    }
}

impl VideoFrameStream {
    pub fn open(
        path: &Path,
        start_time: f64,
        maximum_width: u32,
        maximum_height: u32,
    ) -> Result<Self, VideoError> {
        Self::open_with_frame_rate(path, start_time, maximum_width, maximum_height, None)
    }

    /// Streams frames at a constant `frame_rate` (FFmpeg duplicates or drops
    /// source frames), so frame `n` is exactly at `start_time + n / rate`.
    pub fn open_with_frame_rate(
        path: &Path,
        start_time: f64,
        maximum_width: u32,
        maximum_height: u32,
        frame_rate: Option<f64>,
    ) -> Result<Self, VideoError> {
        let info = probe_media(path)?;
        let (width, height) =
            fitted_dimensions(info.width, info.height, maximum_width, maximum_height);
        let start_time = start_time.clamp(0.0, info.duration);
        let frame_rate = frame_rate
            .filter(|rate| rate.is_finite() && *rate > 0.0)
            .unwrap_or(info.frame_rate)
            .clamp(1.0, 120.0);
        let mut child = Command::new("ffmpeg")
            .args(["-v", "error", "-ss"])
            .arg(format!("{start_time:.6}"))
            .arg("-i")
            .arg(path)
            .args(["-an", "-vf"])
            .arg(format!(
                "fps={frame_rate:.6},scale={width}:{height}:flags=lanczos,format=rgba"
            ))
            .args(["-f", "rawvideo", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| VideoError::Decode("FFmpeg did not provide frame output".into()))?;
        Ok(Self {
            child,
            stdout,
            width,
            height,
            frame_rate,
            next_frame_index: 0,
            start_time,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn frame_rate(&self) -> f64 {
        self.frame_rate
    }

    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, VideoError> {
        let expected = frame_byte_count(self.width, self.height)?;
        let mut rgba = vec![0; expected];
        let mut read = 0;
        while read < expected {
            match self.stdout.read(&mut rgba[read..]) {
                Ok(0) if read == 0 => return Ok(None),
                Ok(0) => {
                    return Err(VideoError::Decode(format!(
                        "FFmpeg ended halfway through a frame ({read}/{expected} bytes)"
                    )));
                }
                Ok(count) => read += count,
                Err(error) => return Err(error.into()),
            }
        }
        let time = self.start_time + self.next_frame_index as f64 / self.frame_rate;
        self.next_frame_index += 1;
        Ok(Some(DecodedFrame {
            time,
            width: self.width,
            height: self.height,
            rgba,
        }))
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for VideoFrameStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn fitted_dimensions(
    source_width: u32,
    source_height: u32,
    maximum_width: u32,
    maximum_height: u32,
) -> (u32, u32) {
    let maximum_width = maximum_width.max(2);
    let maximum_height = maximum_height.max(2);
    let scale = (maximum_width as f64 / source_width as f64)
        .min(maximum_height as f64 / source_height as f64)
        .min(1.0);
    let even = |value: f64| ((value.floor() as u32).max(2) / 2) * 2;
    (
        even(source_width as f64 * scale),
        even(source_height as f64 * scale),
    )
}

fn parse_frame_rate(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    (denominator != 0.0).then_some(numerator / denominator)
}

fn frame_byte_count(width: u32, height: u32) -> Result<usize, VideoError> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| VideoError::InvalidMedia("decoded frame dimensions overflow".into()))
}

fn validate_frame_bytes(bytes: &[u8], width: u32, height: u32) -> Result<(), VideoError> {
    let expected = frame_byte_count(width, height)?;
    if bytes.len() != expected {
        return Err(VideoError::Decode(format!(
            "decoded frame contained {} bytes; expected {expected}",
            bytes.len()
        )));
    }
    Ok(())
}

fn temporary_sibling(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("poster.jpg");
    destination.with_file_name(format!(".{name}.{}.tmp.jpg", uuid::Uuid::new_v4()))
}

fn temporary_video_sibling(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("edit-preview.mkv");
    destination.with_file_name(format!(".{name}.{}.tmp.mkv", uuid::Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_video() -> Option<(PathBuf, PathBuf)> {
        let root =
            std::env::temp_dir().join(format!("screendrop-video-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).ok()?;
        let path = root.join("test.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=12:duration=1",
                "-c:v",
                "ffv1",
                "-y",
            ])
            .arg(&path)
            .status()
            .ok()?;
        status.success().then_some((root, path))
    }

    fn test_video_with_audio() -> Option<(PathBuf, PathBuf)> {
        let root = std::env::temp_dir().join(format!(
            "screendrop-video-audio-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).ok()?;
        let path = root.join("test.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=12:duration=3",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=3",
                "-c:v",
                "ffv1",
                "-c:a",
                "pcm_s16le",
                "-shortest",
                "-y",
            ])
            .arg(&path)
            .status()
            .ok()?;
        status.success().then_some((root, path))
    }

    #[test]
    fn probes_and_decodes_real_video_frames() {
        let Some((root, path)) = test_video() else {
            eprintln!("FFmpeg unavailable; skipping decoder integration test");
            return;
        };
        let info = probe_media(&path).unwrap();
        assert_eq!((info.width, info.height), (320, 180));
        assert!((info.frame_rate - 12.0).abs() < 0.001);
        let frame = decode_frame(&path, 0.5, 160, 100).unwrap();
        assert_eq!((frame.width, frame.height), (160, 90));
        assert_eq!(frame.rgba.len(), 160 * 90 * 4);
        assert!(frame.rgba.iter().any(|value| *value != 0));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stream_returns_consecutive_fixed_size_frames() {
        let Some((root, path)) = test_video() else {
            eprintln!("FFmpeg unavailable; skipping decoder integration test");
            return;
        };
        let mut stream = VideoFrameStream::open(&path, 0.25, 160, 100).unwrap();
        let first = stream.next_frame().unwrap().unwrap();
        let second = stream.next_frame().unwrap().unwrap();
        assert_eq!((first.width, first.height), stream.dimensions());
        assert_eq!(first.rgba.len(), second.rgba.len());
        assert!(second.time > first.time);
        stream.stop();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn poster_is_written_atomically_as_jpeg() {
        let Some((root, path)) = test_video() else {
            eprintln!("FFmpeg unavailable; skipping decoder integration test");
            return;
        };
        let poster = root.join("poster.jpg");
        write_poster(&path, &poster, 200, 200).unwrap();
        assert_eq!(image::image_dimensions(&poster).unwrap(), (200, 112));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_poster_is_a_recoverable_cache_miss() {
        let Some((root, path)) = test_video() else {
            eprintln!("FFmpeg unavailable; skipping poster recovery test");
            return;
        };
        let poster = root.join("missing-poster.jpg");
        let image = load_or_rebuild_poster(&path, &poster, 200, 200).unwrap();
        assert!(poster.exists());
        assert_eq!(image.dimensions(), (200, 112));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn edited_preview_applies_cuts_and_speed_to_video_and_audio() {
        let Some((root, path)) = test_video_with_audio() else {
            eprintln!("FFmpeg unavailable; skipping clip preview integration test");
            return;
        };
        let mut fast = crate::recording::clips::RecordingClipSegment::new(2.0, 3.0);
        fast.speed = 2.0;
        let timeline = RecordingClipTimeline::new(vec![
            crate::recording::clips::RecordingClipSegment::new(0.0, 1.0),
            fast,
        ]);
        let preview = root.join("edited.mkv");
        render_clip_preview(&path, &preview, &timeline).unwrap();
        let info = probe_media(&preview).unwrap();
        assert!(info.has_audio);
        // Container/audio packet padding may add up to two source frames; the
        // editor continues to use the exact mathematical clip duration.
        assert!((info.duration - 1.5).abs() < 0.2, "{}", info.duration);
        let final_frame = decode_frame(&preview, 1.4, 160, 90).unwrap();
        assert!(final_frame.rgba.iter().any(|value| *value != 0));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn edited_preview_renders_reordered_clips() {
        let Some((root, path)) = test_video_with_audio() else {
            eprintln!("FFmpeg unavailable; skipping reorder preview integration test");
            return;
        };
        // Play the tail of the recording before its head.
        let timeline = RecordingClipTimeline::new(vec![
            crate::recording::clips::RecordingClipSegment::new(2.0, 3.0),
            crate::recording::clips::RecordingClipSegment::new(0.0, 1.5),
        ]);
        assert_eq!(timeline.normalized(3.0), timeline);
        let preview = root.join("reordered.mkv");
        render_clip_preview(&path, &preview, &timeline).unwrap();
        let info = probe_media(&preview).unwrap();
        assert!((info.duration - 2.5).abs() < 0.2, "{}", info.duration);
        let final_frame = decode_frame(&preview, 2.3, 160, 90).unwrap();
        assert!(final_frame.rgba.iter().any(|value| *value != 0));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn edited_preview_renders_gaps_as_black_filler() {
        let Some((root, path)) = test_video_with_audio() else {
            eprintln!("FFmpeg unavailable; skipping gap preview integration test");
            return;
        };
        let mut spaced = crate::recording::clips::RecordingClipSegment::new(2.0, 3.0);
        spaced.gap_before = 1.0;
        let timeline = RecordingClipTimeline::new(vec![
            crate::recording::clips::RecordingClipSegment::new(0.0, 1.0),
            spaced,
        ]);
        assert!((timeline.duration() - 3.0).abs() < 1e-9);
        let preview = root.join("gapped.mkv");
        render_clip_preview(&path, &preview, &timeline).unwrap();
        let info = probe_media(&preview).unwrap();
        assert!(info.has_audio);
        assert!((info.duration - 3.0).abs() < 0.2, "{}", info.duration);
        // The gap between one and two seconds decodes as pure black.
        let gap_frame = decode_frame(&preview, 1.5, 160, 90).unwrap();
        let max_luma = gap_frame
            .rgba
            .chunks(4)
            .flat_map(|pixel| pixel[..3].iter())
            .copied()
            .max()
            .unwrap_or(0);
        assert!(max_luma < 32, "gap frame not black (max {max_luma})");
        // Content after the gap still decodes.
        let content_frame = decode_frame(&preview, 2.5, 160, 90).unwrap();
        assert!(content_frame.rgba.iter().any(|value| *value > 64));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn audio_levels_and_thumbnails_describe_the_media() {
        let Some((root, path)) = test_video_with_audio() else {
            eprintln!("FFmpeg unavailable; skipping audio level test");
            return;
        };
        let levels = audio_levels(&path, 12).unwrap();
        assert_eq!(levels.len(), 12);
        assert!(levels.iter().any(|level| *level > 0.5));
        assert!(levels.iter().all(|level| (0.0..=1.0).contains(level)));
        let thumbnails = decode_thumbnails(&path, 4, 36).unwrap();
        assert!(!thumbnails.is_empty() && thumbnails.len() <= 4);
        assert_eq!(thumbnails[0].height(), 36);
        assert!(thumbnails[0].pixels().any(|pixel| pixel[0] > 0));
        let silent = test_video().unwrap();
        assert!(audio_levels(&silent.1, 8).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(silent.0).unwrap();
    }

    #[test]
    fn gstreamer_transport_outputs_clocked_preview_frames() {
        if Command::new("gst-play-1.0")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| !status.success())
            .unwrap_or(true)
        {
            eprintln!("GStreamer playback unavailable; skipping transport integration test");
            return;
        }
        let Some((root, path)) = test_video() else {
            eprintln!("FFmpeg unavailable; skipping transport integration test");
            return;
        };
        let mut stream = SynchronizedPlaybackStream::open(&path, 0.25, 160, 100).unwrap();
        let first = stream.next_frame().unwrap().unwrap();
        let second = stream.next_frame().unwrap().unwrap();
        assert_eq!((first.width, first.height), (160, 90));
        assert_eq!(first.rgba.len(), 160 * 90 * 4);
        assert!(second.time > first.time);
        assert!(stream.position() > 0.25);
        stream.stop();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "plays frames from the newest persistent recording"]
    fn live_gstreamer_transport_decodes_recording_package() {
        let session = crate::recording::model::recoverable_sessions()
            .unwrap()
            .into_iter()
            .next()
            .expect("a persistent recording package");
        let mut stream = SynchronizedPlaybackStream::open(&session.screen_path(), 0.25, 640, 360)
            .expect("open synchronized playback");
        let first = stream.next_frame().unwrap().expect("first frame");
        let second = stream.next_frame().unwrap().expect("second frame");
        assert_eq!(first.rgba.len(), (first.width * first.height * 4) as usize);
        assert!(first.rgba.iter().any(|value| *value != 0));
        assert!(second.time > first.time);
        stream.stop();
        println!("playback_session={}", session.directory.display());
    }
}
