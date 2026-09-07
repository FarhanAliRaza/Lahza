//! Hardware encoding is selected by a real, bounded encode at the requested
//! size. An FFmpeg build listing an encoder does not imply a usable GPU.

use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::export::ExportProgress;

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

    pub fn configure_output(&self, command: &mut Command) {
        match self {
            Self::Nvenc => {
                command.args([
                    "-c:v",
                    "h264_nvenc",
                    "-preset",
                    "p4",
                    "-rc",
                    "vbr",
                    "-cq",
                    "18",
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
                    "h264_vaapi",
                    "-rc_mode",
                    "CQP",
                    "-qp",
                    "18",
                ]);
            }
            Self::Software => {
                command.args([
                    "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
                ]);
            }
        }
    }
}

pub fn select_mp4_encoder(
    width: u32,
    height: u32,
    frame_rate: f64,
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
        !progress.is_cancelled() && probe(candidate, width, height, frame_rate, progress)
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
    progress: &ExportProgress,
) -> bool {
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin"]);
    encoder.configure_input(&mut command);
    command.args(["-f", "lavfi", "-i"]).arg(format!(
        "color=c=black:s={width}x{height}:r={frame_rate:.3},format=rgba"
    ));
    encoder.configure_output(&mut command);
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
        encoder.configure_output(&mut command);
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
