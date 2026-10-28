//! Locate a matched Whisper runtime and prepare on-device speech recognition.
//! Release packages contain the runtime; source installs can fetch the exact
//! application release's runtime, checked against its published SHA256SUMS.
use super::speech_runtime::{self, ProcessingMode};
use super::transcription::{self, JobProgress, SpeechModel};
use super::video::VideoError;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn runtime_roots(executable: Option<&Path>, data_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(prefix) = executable.and_then(Path::parent).and_then(Path::parent) {
        roots.push(prefix.join("lib/lahza/whisper"));
    }
    roots.push(data_root.join("whisper"));
    roots
}

pub fn bundled_executable(name: &str) -> Option<PathBuf> {
    runtime_roots(
        std::env::current_exe().ok().as_deref(),
        &speech_runtime::data_root(),
    )
    .into_iter()
    .map(|root| root.join("bin").join(name))
    .find(|path| path.is_file())
}

/// Existing source installs may have a CPU-only system Whisper. Offer the
/// matched release runtime without replacing a deliberate binary override.
pub fn can_install_runtime() -> bool {
    std::env::var_os("LAHZA_WHISPER_CLI").is_none()
        && std::env::var_os("LAHZA_WHISPER_STREAM").is_none()
        && bundled_executable("whisper-cli").is_none()
}

fn invalid(message: impl Into<String>) -> VideoError {
    VideoError::InvalidMedia(message.into())
}

static SETUP_BUSY: AtomicBool = AtomicBool::new(false);
pub fn is_busy() -> bool {
    SETUP_BUSY.load(Ordering::Relaxed)
}
struct SetupGuard;
impl SetupGuard {
    fn acquire() -> Result<Self, VideoError> {
        SETUP_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| invalid("Voice setup is already running in another window."))
    }
}
impl Drop for SetupGuard {
    fn drop(&mut self) {
        SETUP_BUSY.store(false, Ordering::Release);
    }
}

pub fn setup(progress: &JobProgress) -> Result<String, VideoError> {
    let _guard = SetupGuard::acquire()?;
    if can_install_runtime()
        || transcription::whisper_executable().is_none()
        || super::teleprompter::whisper_stream_executable().is_none()
    {
        install_runtime(progress)?;
    }
    transcription::download_model(SpeechModel::Base, progress)?;
    check_inner(progress)
}

pub fn check(progress: &JobProgress) -> Result<String, VideoError> {
    let _guard = SetupGuard::acquire()?;
    check_inner(progress)
}

fn check_inner(progress: &JobProgress) -> Result<String, VideoError> {
    if progress.is_cancelled() {
        return Err(VideoError::Cancelled);
    }
    let executable = transcription::whisper_executable()
        .ok_or_else(|| invalid("Set up the speech model to install Whisper."))?;
    let model = SpeechModel::Base;
    if !model.is_installed() {
        return Err(invalid(
            "Download the speech model to transcribe.",
        ));
    }
    progress.set(0.0);
    let status = speech_runtime::probe(
        &executable,
        &model.path(),
        speech_runtime::load_processing_mode(),
        progress,
    )
    .map_err(|error| {
        if progress.is_cancelled() {
            VideoError::Cancelled
        } else {
            invalid(error)
        }
    })?;
    if progress.is_cancelled() {
        return Err(VideoError::Cancelled);
    }
    speech_runtime::set_status(status.clone());
    progress.set(1.0);
    Ok(status.label())
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(parent: &Path) -> Result<Self, VideoError> {
        std::fs::create_dir_all(parent)?;
        let path = parent.join(format!(".speech-setup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run a bounded, cancellable setup command, draining output to files so a
/// failing downloader/extractor can never block on a full pipe.
fn run(command: &mut Command, work: &Path, progress: &JobProgress) -> Result<String, VideoError> {
    let out = work.join("command.stdout");
    let err = work.join("command.stderr");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out)?)
        .stderr(std::fs::File::create(&err)?)
        .spawn()
        .map_err(|error| invalid(format!("Could not run voice setup: {error}")))?;
    let started = Instant::now();
    loop {
        if progress.is_cancelled() || started.elapsed() > Duration::from_secs(300) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if progress.is_cancelled() {
                VideoError::Cancelled
            } else {
                invalid("Voice setup timed out. Check your connection and try again.")
            });
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let detail = std::fs::read_to_string(&err).unwrap_or_default();
                return Err(invalid(format!(
                    "Voice setup failed: {}",
                    detail.trim().chars().take(700).collect::<String>()
                )));
            }
            return Ok(std::fs::read_to_string(&out).unwrap_or_default());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn download(
    url: &str,
    destination: &Path,
    work: &Path,
    progress: &JobProgress,
) -> Result<(), VideoError> {
    run(
        Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "20",
                "--max-time",
                "280",
                "--output",
            ])
            .arg(destination)
            .arg(url),
        work,
        progress,
    )
    .map(|_| ())
}

fn expected_checksum(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let file = fields.next()?.trim_start_matches('*');
        (file == asset && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

fn install_runtime(progress: &JobProgress) -> Result<(), VideoError> {
    if std::env::var_os("LAHZA_WHISPER_CLI").is_some()
        || std::env::var_os("LAHZA_WHISPER_STREAM").is_some()
    {
        return Err(invalid(
            "The custom Whisper path is unavailable. Correct the Whisper override before setup.",
        ));
    }
    if std::env::consts::OS != "linux" || std::env::consts::ARCH != "x86_64" {
        return Err(invalid("Automatic Whisper installation currently supports Linux x86_64. Install whisper.cpp with whisper-stream for this platform."));
    }
    let root = speech_runtime::data_root();
    let work = Scratch::new(&root)?;
    let version = env!("CARGO_PKG_VERSION");
    let asset = format!("lahza-whisper-{version}-linux-x86_64.tar.gz");
    let base = format!("https://github.com/FarhanAliRaza/lahza/releases/download/v{version}");
    let sums = work.0.join("SHA256SUMS");
    let archive = work.0.join(&asset);
    download(&format!("{base}/SHA256SUMS"), &sums, &work.0, progress)?;
    let expected = expected_checksum(&std::fs::read_to_string(sums)?, &asset)
        .ok_or_else(|| invalid("This release has no downloadable voice runtime. Install the latest Lahza package, which includes Whisper."))?;
    progress.set(0.1);
    download(&format!("{base}/{asset}"), &archive, &work.0, progress)?;
    progress.set(0.65);
    let actual = run(Command::new("sha256sum").arg(&archive), &work.0, progress)?;
    if actual.split_whitespace().next() != Some(expected.as_str()) {
        return Err(invalid(
            "Voice runtime checksum did not match. Please retry setup.",
        ));
    }
    // Python's data filter rejects paths/symlinks that escape the destination.
    run(Command::new("python3").arg("-c").arg(
        "import sys,tarfile\nwith tarfile.open(sys.argv[1], 'r:gz') as t:\n for m in t.getmembers():\n  if m.name.split('/')[0] != 'whisper': raise ValueError('Unexpected runtime archive path')\n t.extractall(sys.argv[2], filter='data')"
    ).arg(&archive).arg(&work.0), &work.0, progress)?;
    let staged = work.0.join("whisper");
    for binary in ["whisper-cli", "whisper-stream"] {
        let path = staged.join("bin").join(binary);
        if !path.is_file() {
            return Err(invalid("The downloaded voice runtime is incomplete."));
        }
        run(
            speech_runtime::command_with_mode(&path, ProcessingMode::Cpu).arg("--help"),
            &work.0,
            progress,
        )?;
    }
    let destination = root.join("whisper");
    let previous = work.0.join("previous");
    if destination.exists() {
        std::fs::rename(&destination, &previous)?;
    }
    if let Err(error) = std::fs::rename(staged, &destination) {
        if previous.exists() {
            let _ = std::fs::rename(previous, destination);
        }
        return Err(error.into());
    }
    progress.set(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_runtime_for_system_local_and_relocated_bundle_installs() {
        for prefix in ["/usr", "/home/person/.local", "/opt/portable-lahza"] {
            let exe = Path::new(prefix).join("bin/lahza");
            assert_eq!(
                runtime_roots(Some(&exe), Path::new("/data")),
                vec![
                    Path::new(prefix).join("lib/lahza/whisper"),
                    PathBuf::from("/data/whisper")
                ]
            );
        }
    }
    #[test]
    fn checksums_require_the_exact_asset_and_full_digest() {
        let hash = "a".repeat(64);
        assert_eq!(
            expected_checksum(&format!("{hash}  runtime.tar.gz\n"), "runtime.tar.gz"),
            Some(hash)
        );
        assert_eq!(
            expected_checksum("abcd runtime.tar.gz", "runtime.tar.gz"),
            None
        );
        assert_eq!(
            expected_checksum(
                &format!("{} other.tar.gz", "a".repeat(64)),
                "runtime.tar.gz"
            ),
            None
        );
    }

    /// Exercise actual inference through both modes without recording audio
    /// or writing preferences. Set LAHZA_TEST_RUNTIME to test a relocated bundle.
    #[test]
    #[ignore = "requires installed speech model and Whisper runtime"]
    fn probe_installed_processing_modes() {
        let executable = std::env::var_os("LAHZA_TEST_RUNTIME")
            .map(PathBuf::from)
            .unwrap_or_else(|| transcription::whisper_executable().expect("Whisper installed"));
        let model = SpeechModel::Base.path();
        let progress = JobProgress::default();
        let cpu =
            speech_runtime::probe(&executable, &model, ProcessingMode::Cpu, &progress).unwrap();
        assert!(matches!(cpu, speech_runtime::RuntimeStatus::Cpu { .. }));
        eprintln!("CPU: {}", cpu.label());
        let auto =
            speech_runtime::probe(&executable, &model, ProcessingMode::Auto, &progress).unwrap();
        eprintln!("Auto: {}", auto.label());
        let gpu = speech_runtime::probe(&executable, &model, ProcessingMode::Gpu, &progress);
        if std::env::var("LAHZA_TEST_EXPECT_GPU").as_deref() == Ok("1") {
            assert!(auto.is_gpu());
            assert!(gpu.unwrap().is_gpu());
        } else if std::env::var("LAHZA_TEST_EXPECT_GPU").as_deref() == Ok("0") {
            assert!(!auto.is_gpu());
            assert!(gpu.is_err());
        }
        let cancelled = JobProgress::default();
        cancelled.cancel();
        assert!(
            speech_runtime::probe(&executable, &model, ProcessingMode::Auto, &cancelled).is_err()
        );
    }
}
