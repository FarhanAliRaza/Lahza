use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

/// Shared between the exporting thread and the UI.
#[derive(Debug, Default)]
pub struct ExportProgress {
    pub completed: AtomicU64,
    pub total: AtomicU64,
    pub cancelled: Arc<AtomicBool>,
    pub audio_preparation: Mutex<Option<super::audio_processing::ProcessingControl>>,
    pub encoder: Mutex<Option<String>>,
    pub compositor: Mutex<Option<String>>,
    pub encoding_started: Mutex<Option<Instant>>,
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

    pub fn set_compositor_label(&self, label: &str) {
        let mut current = self.compositor.lock().unwrap();
        if current.as_deref() != Some(label) { *current = Some(label.to_owned()); }
    }

    pub fn set_encoder_label(&self, label: &str) {
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

    pub fn reset(&self, total: u64) {
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

/// The video codec inside an MP4.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoCodec {
    /// Plays everywhere.
    #[default]
    H264,
    /// About 40% smaller at the same quality; newer players only.
    Hevc,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 2] = [VideoCodec::H264, VideoCodec::Hevc];

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mp4Encoder {
    Nvenc,
    Vaapi(PathBuf),
    Software,
}

impl Mp4Encoder {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Nvenc => "GPU · NVIDIA",
            Self::Vaapi(_) => "GPU · VAAPI",
            Self::Software => "CPU",
        }
    }

    pub fn configure_input(&self, command: &mut Command) {
        if let Self::Vaapi(device) = self {
            command.arg("-vaapi_device").arg(device);
        }
    }

    pub fn configure_output(&self, command: &mut Command, codec: VideoCodec) {
        let hevc = codec == VideoCodec::Hevc;
        // HEVC holds the same visual quality at a higher quantizer.
        let quality = if hevc { "22" } else { "18" };
        match self {
            Self::Nvenc => {
                command.args([
                    "-c:v",
                    if hevc { "hevc_nvenc" } else { "h264_nvenc" },
                    "-preset",
                    "p4",
                    "-rc",
                    "vbr",
                    "-cq",
                    quality,
                    "-b:v",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                ]);
            }
            Self::Vaapi(_) => {
                command.args([
                    "-vf",
                    "format=nv12,hwupload",
                    "-c:v",
                    if hevc { "hevc_vaapi" } else { "h264_vaapi" },
                    "-rc_mode",
                    "CQP",
                    "-qp",
                    quality,
                ]);
            }
            Self::Software if hevc => {
                command.args([
                    "-c:v", "libx265", "-preset", "fast", "-crf", quality, "-pix_fmt", "yuv420p",
                    "-x265-params", "log-level=error",
                ]);
            }
            Self::Software => {
                command.args([
                    "-c:v", "libx264", "-preset", "veryfast", "-crf", quality, "-pix_fmt", "yuv420p",
                ]);
            }
        }
        if hevc {
            // The hvc1 tag lets QuickTime and Safari play the file.
            command.args(["-tag:v", "hvc1"]);
        }
    }
}

pub fn select_mp4_encoder(
    width: u32,
    height: u32,
    frame_rate: f64,
    codec: VideoCodec,
    progress: &ExportProgress,
) -> Mp4Encoder {
    let mut candidates = vec![Mp4Encoder::Nvenc];
    // Try every render node, including a discrete GPU on hybrid systems.
    let mut devices: Vec<_> = fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("renderD"))
        .map(|entry| entry.path())
        .collect();
    devices.sort();
    candidates.extend(devices.into_iter().map(Mp4Encoder::Vaapi));
    select_from(candidates, |candidate| {
        !progress.is_cancelled() && probe(candidate, width, height, frame_rate, codec, progress)
    })
}

fn select_from(
    candidates: impl IntoIterator<Item = Mp4Encoder>,
    mut usable: impl FnMut(&Mp4Encoder) -> bool,
) -> Mp4Encoder {
    candidates
        .into_iter()
        .find(|encoder| usable(encoder))
        .unwrap_or(Mp4Encoder::Software)
}

fn probe(
    encoder: &Mp4Encoder,
    width: u32,
    height: u32,
    frame_rate: f64,
    codec: VideoCodec,
    progress: &ExportProgress,
) -> bool {
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin"]);
    encoder.configure_input(&mut command);
    command.args(["-f", "lavfi", "-i"]).arg(format!(
        "color=c=black:s={width}x{height}:r={frame_rate:.3},format=rgba"
    ));
    encoder.configure_output(&mut command, codec);
    command.args(["-frames:v", "3", "-an", "-f", "null", "-"]);
    probe_command(command, Duration::from_secs(5), || progress.is_cancelled())
}

fn probe_command(mut command: Command, timeout: Duration, cancelled: impl Fn() -> bool) -> bool {
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let start = Instant::now();
    loop {
        if cancelled() || start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn falls_back_and_tries_other_gpus() {
        let devices = vec![
            Mp4Encoder::Nvenc,
            Mp4Encoder::Vaapi("/dev/dri/renderD129".into()),
        ];
        assert_eq!(
            select_from(devices.clone(), |_| false),
            Mp4Encoder::Software
        );
        assert_eq!(select_from(devices.clone(), |_| true), Mp4Encoder::Nvenc);
        assert_eq!(
            select_from(devices.clone(), |e| matches!(e, Mp4Encoder::Vaapi(_))),
            devices[1]
        );
        assert_eq!(select_from([], |_| true), Mp4Encoder::Software);
    }

    #[test]
    fn probe_failure_timeout_and_cancellation_are_bounded() {
        let mut failure = Command::new("sh");
        failure.args(["-c", "exit 1"]);
        assert!(!probe_command(failure, Duration::from_secs(1), || false));
        let start = Instant::now();
        let mut sleeper = Command::new("sleep");
        sleeper.arg("10");
        assert!(!probe_command(sleeper, Duration::from_millis(40), || false));
        assert!(start.elapsed() < Duration::from_secs(2));
        let mut sleeper = Command::new("sleep");
        sleeper.arg("10");
        assert!(!probe_command(sleeper, Duration::from_secs(5), || true));
    }

    #[test]
    fn vaapi_uploads_frames_to_the_selected_device() {
        let encoder = Mp4Encoder::Vaapi("/dev/dri/renderD129".into());
        let mut command = Command::new("ffmpeg");
        encoder.configure_input(&mut command);
        command.args(["-i", "pipe:0"]);
        encoder.configure_output(&mut command, VideoCodec::H264);
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &args[..4],
            &["-vaapi_device", "/dev/dri/renderD129", "-i", "pipe:0"]
        );
        assert!(args
            .windows(2)
            .any(|a| a == ["-vf", "format=nv12,hwupload"]));
        assert!(!args.iter().any(|a| a == "libx264"));
    }
}
