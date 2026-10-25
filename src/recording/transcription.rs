//! On-device narration transcription through whisper.cpp's `whisper-cli`
//! (Ubuntu: `sudo apt install whisper.cpp`), the same way FFmpeg is used:
//! as a runtime tool, never linked. Speech models are downloaded once into
//! the user's data directory.

use super::clips::RecordingClipTimeline;
use super::speech_runtime::{self, ProcessingMode, RuntimeDiagnostics, RuntimeStatus};
use super::transcript::{Transcript, TranscriptWord};
use super::video::VideoError;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A whisper.cpp model the editor offers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpeechModel {
    /// Multilingual base model: fast, good for clear narration.
    #[default]
    Base,
    /// Multilingual small model: slower, noticeably more accurate.
    Small,
}

impl SpeechModel {
    pub const ALL: [SpeechModel; 2] = [SpeechModel::Base, SpeechModel::Small];

    pub fn file_name(self) -> &'static str {
        match self {
            SpeechModel::Base => "ggml-base.bin",
            SpeechModel::Small => "ggml-small.bin",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SpeechModel::Base => "Fast",
            SpeechModel::Small => "Accurate",
        }
    }

    /// Exact download size, used for progress and to detect partial files.
    pub fn bytes(self) -> u64 {
        match self {
            SpeechModel::Base => 147_951_465,
            SpeechModel::Small => 487_601_967,
        }
    }

    pub fn megabytes(self) -> u64 {
        (self.bytes() as f64 / 1_000_000.0).round() as u64
    }

    fn url(self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.file_name()
        )
    }

    pub fn path(self) -> PathBuf {
        models_directory().join(self.file_name())
    }

    pub fn is_installed(self) -> bool {
        std::fs::metadata(self.path()).is_ok_and(|meta| meta.len() == self.bytes())
    }
}

fn models_directory() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("lahza/models")
}

/// The `whisper-cli` executable: `LAHZA_WHISPER_CLI`, else the first known
/// name on `PATH`.
pub fn whisper_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LAHZA_WHISPER_CLI").map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    if let Some(path) = super::speech_setup::bundled_executable("whisper-cli") {
        return Some(path);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|directory| {
            ["whisper-cli", "whisper-cpp", "whisper.cpp"].map(|name| directory.join(name))
        })
        .find(|candidate| candidate.is_file())
}

/// Shared between the UI and a background job.
#[derive(Clone, Default)]
pub struct JobProgress {
    /// Per-mille of the current stage.
    pub fraction: Arc<AtomicU32>,
    pub cancelled: Arc<AtomicBool>,
}

impl JobProgress {
    pub fn set(&self, fraction: f64) {
        self.fraction
            .store((fraction.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }

    pub fn get(&self) -> f64 {
        self.fraction.load(Ordering::Relaxed) as f64 / 1000.0
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// Download `model` with progress; a finished file is renamed into place so
/// an interrupted download never looks installed.
pub fn download_model(model: SpeechModel, progress: &JobProgress) -> Result<PathBuf, VideoError> {
    let destination = model.path();
    if model.is_installed() {
        return Ok(destination);
    }
    std::fs::create_dir_all(models_directory())?;
    let partial = destination.with_extension("part");
    let _ = std::fs::remove_file(&partial);
    let mut child = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error", "--output"])
        .arg(&partial)
        .arg(model.url())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            VideoError::InvalidMedia(format!("could not start curl to download the speech model: {error}"))
        })?;
    loop {
        if progress.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&partial);
            return Err(VideoError::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                let mut message = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut message);
                }
                let _ = std::fs::remove_file(&partial);
                return Err(VideoError::InvalidMedia(format!(
                    "could not download the speech model: {}",
                    message.trim()
                )));
            }
            break;
        }
        let downloaded = std::fs::metadata(&partial).map(|meta| meta.len()).unwrap_or(0);
        progress.set(downloaded as f64 / model.bytes() as f64);
        std::thread::sleep(Duration::from_millis(150));
    }
    let size = std::fs::metadata(&partial)?.len();
    if size != model.bytes() {
        let _ = std::fs::remove_file(&partial);
        return Err(VideoError::InvalidMedia(format!(
            "the speech model download was incomplete ({size} of {} bytes)",
            model.bytes()
        )));
    }
    std::fs::rename(&partial, &destination)?;
    progress.set(1.0);
    Ok(destination)
}

/// Render the whole project source (every imported asset, in source order)
/// to the 16 kHz mono WAV whisper expects.
fn extract_narration(source: &Path, duration: f64, destination: &Path) -> Result<(), VideoError> {
    let timeline = RecordingClipTimeline::full(duration);
    let (inputs, filter) = super::media_bank::source_filter(source, &timeline, 0, false, true)?;
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin"]);
    super::media_bank::add_inputs(&mut command, &inputs);
    command
        .args(["-filter_complex", &filter, "-map", "[audio]"])
        .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", "-y"])
        .arg(destination);
    let output = command.output()?;
    if !output.status.success() {
        return Err(VideoError::Decode(String::from_utf8_lossy(&output.stderr).trim().into()));
    }
    Ok(())
}

/// Transcribe the project's narration on its source timeline.
pub fn transcribe(
    source: &Path,
    source_duration: f64,
    model: &Path,
    progress: &JobProgress,
) -> Result<Transcript, VideoError> {
    let whisper = whisper_executable().ok_or_else(|| {
        VideoError::InvalidMedia("Set up speech processing before transcribing".into())
    })?;
    let info = super::video::probe_media(source)?;
    if !info.has_audio {
        return Err(VideoError::InvalidMedia("this recording has no audio to transcribe".into()));
    }
    let work = WorkDirectory::new()?;
    let audio = work.0.join("narration.wav");
    progress.set(0.02);
    extract_narration(source, source_duration, &audio)?;
    if progress.is_cancelled() {
        return Err(VideoError::Cancelled);
    }
    progress.set(0.05);
    let threads = std::thread::available_parallelism()
        .map(|count| count.get().saturating_sub(1).clamp(1, 16))
        .unwrap_or(4);
    let output_base = work.0.join("words");
    let mode = speech_runtime::load_processing_mode();
    let checked = speech_runtime::probe(&whisper, model, mode, progress).map_err(|error| {
        if progress.is_cancelled() { VideoError::Cancelled } else { VideoError::InvalidMedia(error) }
    })?;
    speech_runtime::set_status(checked.clone());
    let actual_mode = if matches!(checked, RuntimeStatus::Cpu { .. }) { ProcessingMode::Cpu } else { mode };
    let first = transcribe_audio(&whisper, model, &audio, &output_base, threads, actual_mode, &checked, progress);
    let transcript = match first {
        Err(error) if mode == ProcessingMode::Auto && actual_mode != ProcessingMode::Cpu && !progress.is_cancelled() => {
            let fallback = RuntimeStatus::Cpu { reason: format!("GPU transcription failed: {}", error.to_string().chars().take(240).collect::<String>()) };
            eprintln!("[transcription] retrying on CPU: {error}");
            transcribe_audio(&whisper, model, &audio, &output_base, threads, ProcessingMode::Cpu, &fallback, progress)?
        }
        result => result?,
    };
    progress.set(1.0);
    Ok(transcript)
}

fn transcribe_audio(
    whisper: &Path,
    model: &Path,
    audio: &Path,
    output_base: &Path,
    threads: usize,
    mode: ProcessingMode,
    checked: &RuntimeStatus,
    progress: &JobProgress,
) -> Result<Transcript, VideoError> {
    if progress.is_cancelled() { return Err(VideoError::Cancelled); }
    // One segment per word gives word-level timing for karaoke and cutting.
    let mut child = speech_runtime::command_with_mode(whisper, mode)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&audio)
        // Keep diagnostics enabled so the actual backend can be verified;
        // stdout is already discarded, so `-np` is unnecessary here.
        .args(["-l", "auto", "-ml", "1", "-sow", "-oj", "-pp", "-t"])
        .arg(threads.to_string())
        .arg("-of")
        .arg(&output_base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| VideoError::InvalidMedia(format!("could not start whisper.cpp: {error}")))?;
    let stderr = child.stderr.take().expect("stderr was piped");
    let reader_progress = progress.clone();
    let checked = checked.clone();
    let reader = std::thread::spawn(move || {
        let mut tail = Vec::new();
        let mut diagnostics = RuntimeDiagnostics::new(mode);
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            diagnostics.observe(&line);
            let mut runtime = diagnostics.status();
            if let (RuntimeStatus::Cpu { reason }, RuntimeStatus::Cpu { reason: checked_reason }) = (&mut runtime, &checked) {
                *reason = checked_reason.clone();
            }
            if !matches!(runtime, RuntimeStatus::Checking) { speech_runtime::set_status(runtime); }
            if let Some(percent) = line
                .split("progress =")
                .nth(1)
                .and_then(|rest| rest.trim().trim_end_matches('%').trim().parse::<f64>().ok())
            {
                reader_progress.set(0.05 + 0.93 * percent / 100.0);
            } else if !line.contains("backend") {
                tail.push(line);
                if tail.len() > 20 {
                    tail.remove(0);
                }
            }
        }
        (diagnostics, tail.join("\n"))
    });
    let status = loop {
        if progress.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(VideoError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {},
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(error.into());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let (diagnostics, messages) = reader.join().map_err(|_| VideoError::InvalidMedia("Whisper's output reader stopped".into()))?;
    if !status.success() {
        return Err(VideoError::InvalidMedia(format!("whisper.cpp failed: {}", messages.trim())));
    }
    diagnostics.validate().map_err(VideoError::InvalidMedia)?;
    let json = std::fs::read_to_string(output_base.with_extension("json"))?;
    let transcript = parse_whisper_json(&json)?;
    Ok(transcript)
}

/// A scratch directory removed when the job ends, however it ends.
struct WorkDirectory(PathBuf);

impl WorkDirectory {
    fn new() -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("lahza-transcribe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for WorkDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(serde::Deserialize)]
struct WhisperOutput {
    #[serde(default)]
    result: Option<WhisperResult>,
    transcription: Vec<WhisperSegment>,
}

#[derive(serde::Deserialize)]
struct WhisperResult {
    language: Option<String>,
}

#[derive(serde::Deserialize)]
struct WhisperSegment {
    offsets: WhisperOffsets,
    text: String,
}

#[derive(serde::Deserialize)]
struct WhisperOffsets {
    from: i64,
    to: i64,
}

/// Recognized words whose timestamps can be longer than the speech itself.
const MAXIMUM_WORD_DURATION: f64 = 1.5;

fn parse_whisper_json(json: &str) -> Result<Transcript, VideoError> {
    let output: WhisperOutput = serde_json::from_str(json)
        .map_err(|error| VideoError::InvalidMedia(format!("could not read the transcript: {error}")))?;
    let mut words: Vec<TranscriptWord> = Vec::new();
    for segment in output.transcription {
        let text = segment.text;
        let trimmed = text.trim();
        // Non-speech annotations such as "[BLANK_AUDIO]" or "(music)".
        if trimmed.is_empty()
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
            || (trimmed.starts_with('(') && trimmed.ends_with(')'))
        {
            continue;
        }
        let start = segment.offsets.from.max(0) as f64 / 1000.0;
        let end = segment.offsets.to.max(0) as f64 / 1000.0;
        // Punctuation split from its word belongs to the previous word.
        if !trimmed.chars().any(char::is_alphanumeric) {
            if let Some(previous) = words.last_mut() {
                previous.text.push_str(trimmed);
            }
            continue;
        }
        // Token pieces of one word arrive without a leading space.
        if !text.starts_with(char::is_whitespace) {
            if let Some(previous) = words.last_mut() {
                previous.text.push_str(&text);
                previous.end = previous.end.max(end);
                continue;
            }
        }
        words.push(TranscriptWord {
            text,
            start,
            end: end.max(start),
        });
    }
    for index in 0..words.len() {
        let next_start = words.get(index + 1).map(|word| word.start).unwrap_or(f64::INFINITY);
        let word = &mut words[index];
        word.end = word
            .end
            .min(word.start + MAXIMUM_WORD_DURATION)
            .min(next_start.max(word.start));
    }
    Ok(Transcript::from_words(
        words,
        output.result.and_then(|result| result.language),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_gpu_request_rejects_cpu_output_and_cpu_choice_succeeds() {
        use std::os::unix::fs::PermissionsExt;
        let work = WorkDirectory::new().unwrap();
        let executable = work.0.join("whisper-cli");
        std::fs::write(&executable, r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
    if [ "$1" = "-np" ]; then echo 'backend diagnostics were disabled' >&2; exit 9; fi
    if [ "$1" = "-of" ]; then shift; output="$1"; fi
    shift
done
printf 'whisper_model_load: CPU total size = 147.37 MB\nwhisper_backend_init_gpu: no GPU found\n' >&2
printf '{"result":{"language":"en"},"transcription":[{"offsets":{"from":0,"to":500},"text":" Hello"}]}' > "${output}.json"
"#).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = work.0.join("words");
        let progress = JobProgress::default();
        let error = transcribe_audio(&executable, Path::new("model.bin"), Path::new("audio.wav"), &output, 1,
            ProcessingMode::Gpu, &RuntimeStatus::Checking, &progress).unwrap_err();
        assert!(error.to_string().contains("GPU processing is unavailable"));
        let transcript = transcribe_audio(&executable, Path::new("model.bin"), Path::new("audio.wav"), &output, 1,
            ProcessingMode::Cpu, &RuntimeStatus::Cpu { reason: "selected in Processing".into() }, &progress).unwrap();
        assert_eq!(transcript.words.len(), 1);
    }

    #[test]
    fn whisper_words_merge_punctuation_and_cap_long_timestamps() {
        let json = r#"{"result":{"language":"en"},"transcription":[
            {"offsets":{"from":0,"to":580},"text":""},
            {"offsets":{"from":580,"to":1990},"text":" Hello"},
            {"offsets":{"from":1990,"to":2000},"text":","},
            {"offsets":{"from":2000,"to":2500},"text":" [BLANK_AUDIO]"},
            {"offsets":{"from":2500,"to":2700},"text":" wor"},
            {"offsets":{"from":2700,"to":9000},"text":"ld"}
        ]}"#;
        let transcript = parse_whisper_json(json).unwrap();
        let words: Vec<_> = transcript.words.iter().map(|word| word.text.as_str()).collect();
        assert_eq!(words, [" Hello,", " world"]);
        assert!((transcript.words[1].end - 4.0).abs() < 1e-9);
        assert_eq!(transcript.cues[0].text, "Hello, world");
        assert_eq!(transcript.language.as_deref(), Some("en"));
    }
}
