use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
};

use super::clips::RecordingClipTimeline;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub has_audio: bool,
    pub window_capture: bool,
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
    codec_name: Option<String>,
    pix_fmt: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: String,
}

pub fn probe_media(path: &Path) -> Result<MediaInfo, VideoError> {
    probe_media_with_recovery(path).map(|(info, _)| info)
}

/// Also reports whether an old placeholder header required decoded dimensions.
fn probe_media_with_recovery(path: &Path) -> Result<(MediaInfo, bool), VideoError> {
    if let Some(bank) = super::media_bank::MediaBank::load(path)? { return Ok((bank.info(), false)); }
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,codec_name,pix_fmt,width,height,avg_frame_rate",
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
    // Older PipeWire recordings could mux an initial 1x1 negotiation into
    // the container header while encoding full-size frames. Scaling from
    // that header reduces otherwise intact recordings to a blurry 2x2 image.
    let (width, height, recovered) = match (video.width, video.height) {
        (Some(width), Some(height)) if width > 1 && height > 1 => (width, height, false),
        _ => {
            let (width, height) = probe_frame_dimensions(path)?;
            (width, height, true)
        }
    };
    Ok((MediaInfo {
        duration,
        width,
        height,
        frame_rate: video
            .avg_frame_rate
            .as_deref()
            .and_then(parse_frame_rate)
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(30.0),
        window_capture: path.with_extension("window.json").is_file()
            || (path
                .parent()
                .and_then(Path::extension)
                .is_some_and(|ext| ext == "lahzarec")
                && video.codec_name.as_deref() == Some("ffv1")
                && video.pix_fmt.as_deref() == Some("bgra")),
        has_audio: probe
            .streams
            .iter()
            .any(|stream| stream.codec_type == "audio"),
    }, recovered))
}

fn probe_frame_dimensions(path: &Path) -> Result<(u32, u32), VideoError> {
    #[derive(Deserialize)]
    struct FrameDimensions {
        width: u32,
        height: u32,
    }
    #[derive(Deserialize)]
    struct Frames {
        frames: Vec<FrameDimensions>,
    }
    // Decode only the first video packet, not the whole recording. Stream
    // metadata alone can keep reporting the muxer's placeholder dimensions.
    let output = Command::new("ffprobe")
        .args([
            "-v", "error", "-select_streams", "v:0", "-read_intervals", "%+#1",
            "-show_entries", "frame=width,height", "-of", "json",
        ])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(VideoError::Probe(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let probe: Frames = serde_json::from_slice(&output.stdout)
        .map_err(|error| VideoError::Probe(error.to_string()))?;
    probe.frames.into_iter()
        .find(|frame| frame.width > 0 && frame.height > 0)
        .map(|frame| (frame.width, frame.height))
        .ok_or_else(|| VideoError::InvalidMedia("recording has no valid frame dimensions".into()))
}

pub fn decode_frame(
    path: &Path,
    time: f64,
    maximum_width: u32,
    maximum_height: u32,
) -> Result<DecodedFrame, VideoError> {
    if let Some(bank) = super::media_bank::MediaBank::load(path)? {
        return bank.decode_frame(path, time, maximum_width, maximum_height);
    }
    let info = probe_media(path)?;
    let (width, height) = decode_dimensions(&info, maximum_width, maximum_height);
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
    Ok(present_frame(
        DecodedFrame {
            time: selected_time,
            width,
            height,
            rgba: output.stdout,
        },
        info.window_capture,
    ))
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
    let image = image::DynamicImage::ImageRgba8(image);
    // Ordinary frames are already fitted by decode_frame. Only native
    // window frames bypass that scaling to preserve their alpha crop.
    let image = if info.window_capture {
        image.thumbnail(maximum_width, maximum_height)
    } else {
        image
    };
    let (image, format) = if destination.extension().is_some_and(|ext| ext == "png") {
        (image, image::ImageFormat::Png)
    } else {
        (
            image::DynamicImage::ImageRgb8(image.to_rgb8()),
            image::ImageFormat::Jpeg,
        )
    };
    let temporary = temporary_sibling(destination);
    image
        .save_with_format(&temporary, format)
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
    let (info, recovered_dimensions) = probe_media_with_recovery(video_path)?;
    if info.window_capture {
        let frame = decode_frame(video_path, 0.1, maximum_width, maximum_height)?;
        return image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
            .ok_or_else(|| VideoError::Decode("invalid window preview pixels".into()));
    }
    let expected_size = decode_dimensions(&info, maximum_width, maximum_height);
    if let Ok(poster) = image::open(poster_path) {
        // A tiny decoded frame may have been enlarged before caching. For
        // square recordings even its poster dimensions can look correct.
        if !recovered_dimensions && (poster.width(), poster.height()) == expected_size {
            return Ok(poster.to_rgba8());
        }
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

/// Shared edit graph for direct export streams and materialized previews.
/// Audio and video can be requested separately without decoding the other.
pub fn clip_filter(
    info: &MediaInfo,
    timeline: &RecordingClipTimeline,
    input: usize,
    video: bool,
    audio: bool,
) -> String {
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
    for clip in timeline.playback_ranges().iter() {
        if clip.gap_before > 0.0 {
            if video {
                write!(
                    filter,
                    "color=c=black:s={}x{}:r={:.6}:d={:.9},format=rgba,setsar=1[v{label}];",
                    info.width, info.height, frame_rate, clip.gap_before
                )
                .expect("writing to String cannot fail");
            }
            if audio {
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
        if video {
            write!(
            filter,
            "[{input}:v]trim=start={:.9}:end={:.9},setpts=(PTS-STARTPTS)/{:.9},format=rgba,setsar=1[v{label}];",
            clip.source_start, clip.source_end, clip.speed
        )
        .expect("writing to String cannot fail");
        }
        if audio {
            write!(
                filter,
                "[{input}:a]atrim=start={:.9}:end={:.9},asetpts=PTS-STARTPTS,atempo={:.9},aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[a{label}];",
                clip.source_start, clip.source_end, clip.speed
            )
            .expect("writing to String cannot fail");
        }
        chains.push(label);
        label += 1;
    }
    for index in &chains {
        if video {
            write!(filter, "[v{index}]").expect("writing to String cannot fail");
        }
        if audio {
            write!(filter, "[a{index}]").expect("writing to String cannot fail");
        }
    }
    write!(
        filter,
        "concat=n={}:v={}:a={}{}{}",
        chains.len(),
        u8::from(video),
        u8::from(audio),
        if video { "[video]" } else { "" },
        if audio { "[audio]" } else { "" }
    )
    .expect("writing to String cannot fail");

    filter
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
    if info.window_capture || super::media_bank::MediaBank::load(path)?.is_some() {
        return (0..count)
            .map(|index| {
                let frame = decode_frame(
                    path,
                    info.duration * index as f64 / count as f64,
                    info.width,
                    info.height,
                )?;
                let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                    .ok_or_else(|| VideoError::Decode("invalid window thumbnail".into()))?;
                Ok(image::DynamicImage::ImageRgba8(image)
                    .resize(
                        u32::MAX,
                        height.max(8),
                        image::imageops::FilterType::Triangle,
                    )
                    .to_rgba8())
            })
            .collect();
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
    window_capture: bool,
}

// Our sink handles conversion and fitting. Native video prevents playbin
// from inserting its own scaler with opaque aspect-ratio padding first.
pub const PLAYBACK_FLAGS: &str =
    "video+audio+text+soft-volume+native-video+deinterlace+soft-colorbalance";

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
    window_capture: bool,
}

impl SynchronizedPlaybackStream {
    pub fn open(
        path: &Path,
        start_time: f64,
        maximum_width: u32,
        maximum_height: u32,
    ) -> Result<Self, VideoError> {
        let info = probe_media(path)?;
        let (width, height) = decode_dimensions(&info, maximum_width, maximum_height);
        let start_time = start_time.clamp(0.0, info.duration);
        // Variable-rate screen captures report absurd nominal rates; the
        // preview never needs more than 60 frames a second.
        let frame_rate = info.frame_rate.round().clamp(1.0, 60.0);
        let sink = format!(
            "videoconvert ! videoscale method=lanczos add-borders=false ! videorate ! video/x-raw,format=RGBA,width={width},height={height},framerate={frame_rate:.0}/1,pixel-aspect-ratio=1/1 ! fdsink fd=1 sync=true async=false"
        );
        let mut child = Command::new("gst-play-1.0")
            .args([
                "--quiet",
                "--no-position",
                "--no-interactive",
                "--accurate-seeks",
                "--flags",
                PLAYBACK_FLAGS,
                "--start-position",
            ])
            .arg(format!("{start_time:.6}"))
            .arg("--videosink")
            .arg(sink)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
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
            window_capture: info.window_capture,
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
        Ok(Some(present_frame(
            DecodedFrame {
                time,
                width: self.width,
                height: self.height,
                rgba,
            },
            self.window_capture,
        )))
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
        if super::media_bank::MediaBank::load(path)?.is_some() {
            let timeline = RecordingClipTimeline::new(vec![super::clips::RecordingClipSegment::new(start_time.max(0.0), info.duration)]);
            let mut stream = Self::open_timeline(path, &timeline, maximum_width, maximum_height, frame_rate.unwrap_or(info.frame_rate))?;
            stream.start_time = start_time;
            return Ok(stream);
        }
        let (width, height) = decode_dimensions(&info, maximum_width, maximum_height);
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
            window_capture: info.window_capture,
        })
    }

    /// Apply the edit directly to decoded frames, without an intermediate
    /// video encode or a second decode of that temporary file.
    pub fn open_timeline(
        path: &Path,
        timeline: &RecordingClipTimeline,
        maximum_width: u32,
        maximum_height: u32,
        frame_rate: f64,
    ) -> Result<Self, VideoError> {
        let info = probe_media(path)?;
        let timeline = timeline.normalized(info.duration);
        if timeline.is_unedited(info.duration) && super::media_bank::MediaBank::load(path)?.is_none() {
            return Self::open_with_frame_rate(
                path,
                0.0,
                maximum_width,
                maximum_height,
                Some(frame_rate),
            );
        }
        let (width, height) = decode_dimensions(&info, maximum_width, maximum_height);
        let (inputs, mut filter) = super::media_bank::source_filter(path, &timeline, 0, true, false)?;
        filter.push_str(&format!(
            ";[video]fps={frame_rate:.6},scale={width}:{height}:flags=lanczos,format=rgba[frames]"
        ));
        let mut command = Command::new("ffmpeg");
        command.args(["-v", "error"]);
        super::media_bank::add_inputs(&mut command, &inputs);
        let mut child = command.args([
                "-filter_complex",
                &filter,
                "-map",
                "[frames]",
                "-an",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
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
            start_time: 0.0,
            window_capture: info.window_capture,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn frame_rate(&self) -> f64 {
        self.frame_rate
    }

    pub fn next_time(&self) -> f64 {
        self.start_time + self.next_frame_index as f64 / self.frame_rate
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
        Ok(Some(present_frame(
            DecodedFrame {
                time,
                width: self.width,
                height: self.height,
                rgba,
            },
            self.window_capture,
        )))
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

pub fn decode_dimensions(
    info: &MediaInfo,
    maximum_width: u32,
    maximum_height: u32,
) -> (u32, u32) {
    if info.window_capture
        || (maximum_width >= info.width && maximum_height >= info.height)
    {
        // Native RGB frames can have odd dimensions. Do not resize them just
        // to satisfy encoder alignment; the output encoder handles its own
        // canvas and source sampling must retain every input pixel.
        (info.width, info.height)
    } else {
        fitted_dimensions(info.width, info.height, maximum_width, maximum_height)
    }
}

/// Find a sharp body edge in each half of an alpha profile. Desktop shadows
/// fade gradually; window edges introduce a much larger opacity change.
/// Separate halves also handle a translucent body with an opaque title bar.
fn window_body_edges(profile: &[u8], start: u32, end: u32) -> Option<(u32, u32)> {
    let middle = start + (end - start) / 2;
    let edge = |reverse: bool| {
        let mut best = None;
        let mut largest = 0;
        let range = if reverse { middle..end } else { start..middle };
        for index in range {
            let outside = if reverse {
                profile.get(index as usize + 1).copied().unwrap_or(0)
            } else {
                index
                    .checked_sub(1)
                    .map(|i| profile[i as usize])
                    .unwrap_or(0)
            };
            let inside = profile[index as usize];
            let jump = inside.saturating_sub(outside) as u16;
            if jump >= 8 && jump * 5 >= inside as u16 * 2 && jump > largest {
                largest = jump;
                best = Some(index);
            }
        }
        best
    };
    Some((edge(false)?, edge(true)? + 1))
}

/// Presentation geometry follows the window body, not its storage or OS shadow.
/// Crop before any scaling so resizing never throws away captured detail.
pub fn present_frame(frame: DecodedFrame, window_capture: bool) -> DecodedFrame {
    if !window_capture || frame.width == 0 || frame.height == 0 {
        return frame;
    }
    let (mut left, mut top, mut right, mut bottom) = (frame.width, frame.height, 0, 0);
    let mut columns = vec![0; frame.width as usize];
    let mut rows = vec![0; frame.height as usize];
    for (index, pixel) in frame.rgba.chunks_exact(4).enumerate() {
        if pixel[3] != 0 {
            let x = index as u32 % frame.width;
            let y = index as u32 / frame.width;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
            columns[x as usize] = columns[x as usize].max(pixel[3]);
            rows[y as usize] = rows[y as usize].max(pixel[3]);
        }
    }
    // Only remove a soft margin when all four body edges are identifiable.
    // Keep unusual/translucent shapes intact when the alpha signal is ambiguous.
    if right > left && bottom > top {
        if let (Some((body_left, body_right)), Some((body_top, body_bottom))) = (
            window_body_edges(&columns, left, right),
            window_body_edges(&rows, top, bottom),
        ) {
            (left, top, right, bottom) = (body_left, body_top, body_right, body_bottom);
        }
    }
    if right <= left
        || bottom <= top
        || (left == 0 && top == 0 && right == frame.width && bottom == frame.height)
    {
        return frame;
    }
    let (width, height) = (right - left, bottom - top);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in top..bottom {
        let start = ((y * frame.width + left) * 4) as usize;
        rgba.extend_from_slice(&frame.rgba[start..start + width as usize * 4]);
    }
    DecodedFrame {
        time: frame.time,
        width,
        height,
        rgba,
    }
}

pub fn fitted_dimensions(
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

pub fn temporary_video_sibling(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("edit-preview.mkv");
    destination.with_file_name(format!(".{name}.{}.tmp.mkv", uuid::Uuid::new_v4()))
}

/// Capture the samples after production audio filters without a sound device.
#[doc(hidden)]
pub fn test_audio_sink(samples: std::sync::Arc<std::sync::Mutex<Vec<f32>>>) -> gstreamer::Bin {
    let sink = gstreamer::parse::bin_from_description(
        "audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate=48000,channels=1 ! fakesink name=audio sync=true signal-handoffs=true", true).unwrap();
    use gstreamer::prelude::*;
    sink.by_name("audio").unwrap().connect("handoff", false, move |values| {
        let buffer = values[1].get::<gstreamer::Buffer>().unwrap();
        let map = buffer.map_readable().unwrap();
        samples.lock().unwrap().extend(map.chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())));
        None
    });
    sink
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_decode_keeps_odd_source_edges_instead_of_resizing() {
        let info = MediaInfo {
            duration: 1.0,
            width: 3841,
            height: 2161,
            frame_rate: 30.0,
            has_audio: false,
            window_capture: false,
        };
        assert_eq!(decode_dimensions(&info, 3841, 2161), (3841, 2161));
        assert_eq!(decode_dimensions(&info, 7680, 4320), (3841, 2161));
        let proxy = decode_dimensions(&info, 1280, 720);
        assert!(proxy.0 <= 1280 && proxy.1 <= 720);
        assert!(proxy.0 < info.width && proxy.1 < info.height);
        let window = MediaInfo { window_capture: true, ..info };
        assert_eq!(decode_dimensions(&window, 1280, 720), (3841, 2161));
    }

    fn shadowed_window(width: u32, height: u32, body: &image::RgbaImage) -> image::RgbaImage {
        let (left, top) = ((width - body.width()) / 2, (height - body.height()) / 2);
        let mut frame = image::RgbaImage::new(width, height);
        for (x, y, pixel) in frame.enumerate_pixels_mut() {
            let dx = left
                .saturating_sub(x)
                .max((x + 1).saturating_sub(left + body.width()));
            let dy = top
                .saturating_sub(y)
                .max((y + 1).saturating_sub(top + body.height()));
            let distance = dx.max(dy);
            if distance == 0 {
                *pixel = *body.get_pixel(x - left, y - top);
            } else {
                *pixel = image::Rgba([
                    0,
                    0,
                    0,
                    [55, 39, 28, 16, 8, 4, 2, 1]
                        .get(distance as usize - 1)
                        .copied()
                        .unwrap_or(0),
                ]);
            }
        }
        frame
    }

    #[test]
    fn window_presentation_removes_soft_shadow_without_changing_body_pixels() {
        for opacity in [100, 180, 255] {
            let mut body = image::RgbaImage::from_pixel(40, 30, image::Rgba([0, 0, 0, opacity]));
            // Opaque title bar, translucent content, and antialiased native corners.
            for y in 0..5 {
                for x in 0..40 {
                    body.put_pixel(x, y, image::Rgba([20, 40, 60, 255]));
                }
            }
            body.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
            body.put_pixel(1, 0, image::Rgba([20, 40, 60, 128]));
            let storage = shadowed_window(80, 70, &body);
            let frame = DecodedFrame {
                time: 3.0,
                width: 80,
                height: 70,
                rgba: storage.into_raw(),
            };
            let presented = present_frame(frame, true);
            assert_eq!(
                (presented.width, presented.height, presented.time),
                (40, 30, 3.0)
            );
            assert_eq!(presented.rgba, body.into_raw());
        }
    }

    #[test]
    fn ambiguous_window_alpha_and_non_window_media_keep_their_bounds() {
        for window_capture in [true, false] {
            let source = image::RgbaImage::from_pixel(40, 30, image::Rgba([10, 20, 30, 4]));
            let frame = DecodedFrame {
                time: 0.0,
                width: 40,
                height: 30,
                rgba: source.clone().into_raw(),
            };
            let presented = present_frame(frame, window_capture);
            assert_eq!((presented.width, presented.height), (40, 30));
            assert_eq!(presented.rgba, source.into_raw());
        }
        let blank = present_frame(
            DecodedFrame {
                time: 0.0,
                width: 40,
                height: 30,
                rgba: vec![0; 40 * 30 * 4],
            },
            true,
        );
        assert_eq!((blank.width, blank.height), (40, 30));
    }

    fn test_video() -> Option<(PathBuf, PathBuf)> {
        let root = std::env::temp_dir().join(format!("lahza-video-test-{}", uuid::Uuid::new_v4()));
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

}
