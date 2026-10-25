//! Teleprompter: the script model, the fuzzy matcher that tracks how far the
//! speaker has read, the fixed line layout the prompter scrolls through, and
//! live speech following through whisper.cpp's `whisper-stream`.
//!
//! Following is best effort. Without whisper.cpp, a speech model or a
//! microphone the script still shows and can be scrolled by hand; it never
//! interferes with the recording.

use super::transcription::{whisper_executable, SpeechModel};
use super::speech_runtime::{self, ProcessingMode, RuntimeDiagnostics, RuntimeStatus};
use super::speech_diagnostics::VoiceLog;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Saved prompter settings and script.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TeleprompterSettings {
    pub enabled: bool,
    pub script: String,
    /// Lines visible at once.
    pub lines: u8,
    /// Script text size in points.
    pub font_size: f32,
    /// Advance with the speaker's voice when whisper.cpp is available.
    pub follow_voice: bool,
    /// Pin a known language instead of guessing again on each short chunk.
    pub voice_language: String,
}

impl Default for TeleprompterSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            script: String::new(),
            lines: 3,
            font_size: 26.0,
            follow_voice: true,
            voice_language: "auto".into(),
        }
    }
}

impl TeleprompterSettings {
    const FILE_NAME: &'static str = "teleprompter.json";

    fn path() -> PathBuf {
        super::presets::config_root().join(Self::FILE_NAME)
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(temporary, path)
    }

    pub fn has_script(&self) -> bool {
        !self.script.trim().is_empty()
    }

    /// Whether an enabled script is ready to display for a recording.
    pub fn is_active(&self) -> bool {
        self.enabled && self.has_script()
    }
}

/// Whitespace-separated display words, punctuation kept.
pub fn display_words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

/// Lower-case letters and digits only, so "Hello," matches "hello" and
/// "it's" matches "its".
pub fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn normalized_words(text: &str) -> Vec<String> {
    display_words(text)
        .into_iter()
        .flat_map(|word| word.split(['-', '‐', '‑', '–', '—']))
        .map(normalize)
        .filter(|word| !word.is_empty())
        .collect()
}

/// Align a recognition hypothesis near the current script position.
/// Skipped or misheard words cost evidence, so a repeated common word
/// cannot pull the reader into a later clause on its own.
#[derive(Clone, Debug, Default)]
pub struct ScriptMatcher {
    entries: Vec<(String, usize)>,
    script_words: Vec<String>,
}

impl ScriptMatcher {
    const LOOKAHEAD: usize = 8;
    const RECOVERY_LOOKAHEAD: usize = 64;
    const MAX_HYPOTHESIS_WORDS: usize = 128;

    pub fn new(script: &str) -> Self {
        Self {
            script_words: display_words(script).into_iter().map(str::to_owned).collect(),
            entries: display_words(script)
                .into_iter()
                .enumerate()
                .flat_map(|(index, word)| normalized_words(word).into_iter().map(move |word| (word, index)))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn spoken_count(&self, spoken: &[String]) -> usize {
        self.spoken_count_from(spoken, 0)
    }

    fn spoken_count_from(&self, spoken: &[String], start: usize) -> usize {
        let first = self.entries.partition_point(|(_, index)| *index < start);
        let spoken = &spoken[..spoken.len().min(Self::MAX_HYPOTHESIS_WORDS)];
        // whisper-stream retains 200 ms across committed chunks. A repeated
        // word from that overlap must not penalize the next real word: e.g.
        // "it away" when "it" was already committed and the anchor is "away".
        // Prefer the current anchor for repeated phrases such as "hello world
        // hello world", where the same prefix is also valid new speech.
        let overlap = if self.entries.get(first).zip(spoken.first())
            .is_some_and(|((reference, _), word)| comparable_number(reference) == comparable_number(word))
        {
            0
        } else {
            (1..=first.min(spoken.len()).min(2)).rev().find(|&count| {
                self.entries[first - count..first].iter().zip(&spoken[..count])
                    .all(|((reference, _), word)| comparable_number(reference) == comparable_number(word))
            }).unwrap_or(0)
        };
        let spoken = &spoken[overlap..];
        let end = (first + spoken.len() + Self::LOOKAHEAD).min(self.entries.len());
        let script = &self.entries[first..end];
        // Anchor the alignment to the current chunk: skipping script words
        // costs more than an extra recognition word. Only an endpoint that
        // actually matches can advance the reader; trailing guesses cannot.
        let mut previous: Vec<i32> = (0..=script.len()).map(|index| -2 * index as i32).collect();
        let mut best_score = 0;
        let mut best_end = 0;
        for (heard_index, word) in spoken.iter().enumerate() {
            let mut current = vec![0; script.len() + 1];
            current[0] = -(heard_index as i32 + 1);
            for (script_index, (reference, _)) in script.iter().enumerate() {
                let column = script_index + 1;
                let matches = comparable_number(word) == comparable_number(reference);
                let diagonal = previous[column - 1] + if matches { 3 } else { -2 };
                current[column] = diagonal.max(previous[column] - 1).max(current[column - 1] - 2);
                if matches && self.completes_word(first + script_index) && (diagonal > best_score
                    || (diagonal == best_score && column < best_end))
                {
                    best_score = diagonal;
                    best_end = column;
                }
            }
            previous = current;
        }
        // One exact word at the anchor is enough; an isolated common word
        // farther ahead is not. Equal evidence prefers the nearer endpoint.
        let anchored = if best_score >= 3 {
            (script[best_end - 1].1 + 1).max(start)
        } else {
            start
        };
        // A small match at the old anchor must not hide stronger evidence
        // that the speaker has continued after a misrecognized phrase.
        anchored.max(self.recover_phrase(spoken, first).unwrap_or(start))
    }

    fn completes_word(&self, entry: usize) -> bool {
        self.entries.get(entry + 1).is_none_or(|next| next.1 != self.entries[entry].1)
    }

    /// Recover within the next 64 display words. A nearby exact phrase can
    /// use two distinct substantive words; farther matches need three. One
    /// recognition error is allowed only with four exact matches, including
    /// three distinct substantive words. Common words alone cannot jump ahead.
    fn recover_phrase(&self, spoken: &[String], first: usize) -> Option<usize> {
        let start_word = self.entries.get(first)?.1;
        let end = self.entries.partition_point(|(_, index)| *index < start_word + Self::RECOVERY_LOOKAHEAD);
        // (heard end, heard start, script start, display-word end)
        let mut best: Option<(usize, usize, usize, usize)> = None;
        for script_start in first..end {
            let nearby = self.entries[script_start].1 - start_word <= Self::LOOKAHEAD;
            for heard_start in 0..spoken.len() {
                if let Some((heard_end, word_end)) = self.recover_from(
                    spoken, script_start, end, heard_start, 0, Vec::new(), false, nearby,
                ) {
                    // Follow the latest recognizable speech in this rolling
                    // hypothesis, not a strong but stale prefix. For the same
                    // heard span, keep the nearest script occurrence.
                    if best.is_none_or(|previous| heard_end > previous.0
                        || (heard_end == previous.0 && heard_start < previous.1)
                        || (heard_end == previous.0 && heard_start == previous.1
                            && script_start < previous.2))
                    {
                        best = Some((heard_end, heard_start, script_start, word_end));
                    }
                }
            }
        }
        best.map(|(_, _, _, word_end)| word_end)
    }

    fn recover_from<'a>(
        &'a self, spoken: &[String], mut script: usize, end: usize, mut heard: usize,
        mut matches: usize, mut substantive: Vec<&'a str>, edited: bool, nearby: bool,
    ) -> Option<(usize, usize)> {
        let mut best_end: Option<(usize, usize)> = None;
        while script < end && heard < spoken.len() {
            let reference = &self.entries[script].0;
            if comparable_number(reference) != comparable_number(&spoken[heard]) {
                // Start and end on recognized words. Try only one changed,
                // omitted, or extra word inside an otherwise matching phrase.
                if !edited && matches > 0 {
                    for (skip_script, skip_heard) in [(1, 1), (1, 0), (0, 1)] {
                        if let Some(matched) = self.recover_from(
                            spoken, script + skip_script, end, heard + skip_heard,
                            matches, substantive.clone(), true, nearby,
                        ) {
                            if best_end.is_none_or(|previous| matched.0 > previous.0
                                || (matched.0 == previous.0 && matched.1 < previous.1))
                            {
                                best_end = Some(matched);
                            }
                        }
                    }
                }
                break;
            }
            matches += 1;
            let word = comparable_number(reference);
            if substantive_word(reference) && !substantive.contains(&word) {
                substantive.push(word);
            }
            let enough = if edited {
                matches >= 4 && substantive.len() >= 3
            } else {
                matches >= 3 && (substantive.len() >= 3 || (nearby && substantive.len() >= 2))
            };
            if enough && self.completes_word(script) {
                best_end = Some((heard + 1, self.entries[script].1 + 1));
            }
            script += 1;
            heard += 1;
        }
        best_end
    }

}

fn substantive_word(word: &str) -> bool {
    word.len() >= 3 && !matches!(word,
        "the" | "and" | "are" | "was" | "were" | "for" | "from" | "with" |
        "this" | "that" | "these" | "those" | "you" | "your" | "they" | "their" |
        "them" | "his" | "her" | "our" | "its" | "has" | "had" | "have" |
        "will" | "would" | "could" | "should" | "can" | "but" | "not" |
        "what" | "which" | "when" | "where" | "who" | "how" | "there" | "here")
}

/// Whisper commonly writes spoken numbers as digits even when the script
/// spells them out (or vice versa). Keep other words distinct: "to" is not 2.
fn comparable_number(word: &str) -> &str {
    match word {
        "zero" => "0", "one" => "1", "two" => "2", "three" => "3",
        "four" => "4", "five" => "5", "six" => "6", "seven" => "7",
        "eight" => "8", "nine" => "9", "ten" => "10", "eleven" => "11",
        "twelve" => "12", "thirteen" => "13", "fourteen" => "14", "fifteen" => "15",
        "sixteen" => "16", "seventeen" => "17", "eighteen" => "18", "nineteen" => "19",
        "twenty" => "20", "thirty" => "30", "forty" => "40", "fifty" => "50",
        "sixty" => "60", "seventy" => "70", "eighty" => "80", "ninety" => "90",
        _ => word,
    }
}

/// One display word placed on a prompter line.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutWord {
    /// Index into the script's display words (the matcher's counting).
    pub index: usize,
    pub text: String,
}

/// The script wrapped once into fixed lines, so the prompter scrolls line
/// by line and knows which line holds the active word. Paragraph breaks are
/// hard line breaks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptLayout {
    pub lines: Vec<Vec<LayoutWord>>,
    line_of_word: Vec<usize>,
}

impl ScriptLayout {
    pub fn new(script: &str, maximum_width: f32, measure: impl Fn(&str) -> f32) -> Self {
        let space = measure(" ");
        let mut lines = Vec::new();
        let mut line_of_word = Vec::new();
        let mut index = 0;
        for paragraph in script.lines() {
            let mut line: Vec<LayoutWord> = Vec::new();
            let mut width = 0.0;
            for word in display_words(paragraph) {
                let word_width = measure(word);
                let joined = if line.is_empty() { word_width } else { width + space + word_width };
                if !line.is_empty() && joined > maximum_width {
                    lines.push(std::mem::take(&mut line));
                    width = word_width;
                } else {
                    width = joined;
                }
                line.push(LayoutWord { index, text: word.to_string() });
                line_of_word.push(lines.len());
                index += 1;
            }
            if !line.is_empty() {
                lines.push(line);
            }
        }
        Self { lines, line_of_word }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn word_count(&self) -> usize {
        self.line_of_word.len()
    }

    pub fn line_of(&self, word: usize) -> usize {
        self.line_of_word
            .get(word.min(self.line_of_word.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    }

    /// The line the reader should be on after `spoken` words: once a line's
    /// last word is spoken their eyes already need the next one.
    pub fn reading_line(&self, spoken: usize) -> usize {
        if spoken == 0 || self.line_of_word.is_empty() {
            return 0;
        }
        let last = spoken - 1;
        let line = self.line_of(last);
        let closes_line = last + 1 >= self.line_of_word.len() || self.line_of_word[last + 1] != line;
        if closes_line {
            (line + 1).min(self.lines.len().saturating_sub(1))
        } else {
            line
        }
    }
}

/// Find the streaming executable independently of the batch transcription CLI.
pub fn whisper_stream_executable() -> Option<PathBuf> {
    find_stream_executable(
        std::env::var_os("LAHZA_WHISPER_STREAM").map(PathBuf::from),
        whisper_executable(),
        std::env::var_os("PATH").as_deref(),
    )
}

fn find_stream_executable(
    override_path: Option<PathBuf>,
    cli: Option<PathBuf>,
    search_path: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return path.is_file().then_some(path);
    }
    cli.map(|path| path.with_file_name("whisper-stream"))
        .into_iter()
        .chain(search_path.into_iter().flat_map(std::env::split_paths)
            .map(|directory| directory.join("whisper-stream")))
        .find(|path| path.is_file())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpeechStatus {
    Starting,
    Listening(String),
    Paused(String),
    Failed(String),
}

fn stream_command(stream: &Path, model: &Path, microphone_device: Option<&str>, language: &str, mode: ProcessingMode) -> Command {
    let threads = std::thread::available_parallelism()
        .map(|count| (count.get() / 2).clamp(2, 8))
        .unwrap_or(4);
    let mut command = speech_runtime::command_with_mode(stream, mode);
    command.arg("-m").arg(model)
        // Temperature fallback can spend seconds retrying silence or a bad
        // partial guess, during which whisper-stream discards incoming audio.
        .args(["--step", "800", "--length", "5000", "--keep", "200", "-nf", "-l", language, "-t"])
        .arg(threads.to_string());
    if let Some(device) = microphone_device.filter(|device| !device.is_empty()) {
        // Recorder device IDs are Pulse/PipeWire source names, not SDL's
        // numeric capture indices. Route the child through the same source.
        command.env("SDL_AUDIODRIVER", "pulseaudio").env("PULSE_SOURCE", device);
    }
    command
}

/// Live speech following: runs `whisper-stream` on the selected microphone
/// and publishes the (monotonic) number of script words spoken.
pub struct SpeechFollower {
    child: Child,
    paused: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
    diagnostics: Arc<Mutex<Vec<u8>>>,
    stderr_reader: std::thread::JoinHandle<()>,
    failure: Option<String>,
    runtime: Arc<Mutex<RuntimeDiagnostics>>,
    mode: ProcessingMode,
    started: std::time::Instant,
    cpu_retry: Option<CpuRetry>,
    fallback_reason: Option<String>,
    trace: Arc<VoiceLog>,
    spoken: Arc<AtomicUsize>,
    last_output_ms: Arc<AtomicU64>,
    last_progress: (usize, std::time::Instant),
    last_status: Option<SpeechStatus>,
    last_log: std::time::Instant,
}

struct CpuRetry {
    command: Command,
    matcher: ScriptMatcher,
    spoken: Arc<AtomicUsize>,
}

impl SpeechFollower {
    /// Start following when whisper.cpp and the speech model are present.
    pub fn start(script: &str, spoken: Arc<AtomicUsize>, microphone_device: Option<&str>, language: &str) -> Result<Self, String> {
        let matcher = ScriptMatcher::new(script);
        if matcher.is_empty() {
            return Err("the script has no words".into());
        }
        let stream = whisper_stream_executable().ok_or("whisper-stream is not installed")?;
        let model = SpeechModel::Base;
        if !model.is_installed() {
            return Err("the speech model is not downloaded".into());
        }
        let mode = speech_runtime::load_processing_mode();
        let cpu_retry = (mode == ProcessingMode::Auto).then(|| CpuRetry {
            command: stream_command(&stream, &model.path(), microphone_device, language, ProcessingMode::Cpu),
            matcher: matcher.clone(),
            spoken: spoken.clone(),
        });
        let mut follower = Self::spawn_with_log(
            stream_command(&stream, &model.path(), microphone_device, language, mode), matcher, spoken, mode,
            false, VoiceLog::new(),
        )?;
        follower.cpu_retry = cpu_retry;
        Ok(follower)
    }

    #[cfg(test)]
    fn spawn(command: Command, matcher: ScriptMatcher, spoken: Arc<AtomicUsize>, mode: ProcessingMode) -> Result<Self, String> {
        Self::spawn_with_log(command, matcher, spoken, mode, false, VoiceLog::disabled())
    }

    fn spawn_with_log(mut command: Command, matcher: ScriptMatcher, spoken: Arc<AtomicUsize>, mode: ProcessingMode, initially_paused: bool, trace: Arc<VoiceLog>) -> Result<Self, String> {
        let child_setting = |key: &str| {
            command.get_envs().find(|(name, _)| *name == key)
                .map(|(_, value)| value.map(std::ffi::OsStr::to_owned))
                .unwrap_or_else(|| std::env::var_os(key))
        };
        let startup = format!(
            "[voice-follow] starting executable={:?} args={:?} audio_driver={:?} microphone={:?} backend={:?}",
            command.get_program(), command.get_args().collect::<Vec<_>>(),
            child_setting("SDL_AUDIODRIVER"), child_setting("PULSE_SOURCE"),
            child_setting("GGML_BACKEND_PATH"),
        );
        eprintln!("{startup}");
        trace.write(&startup);
        trace.write(&format!("script_words={} processing={mode:?}", matcher.script_words.len()));
        if let Some(path) = trace.path() {
            eprintln!("[voice-follow] saved diagnostics: {}", path.display());
        }
        let mut child = command.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                let error = format!("could not start whisper-stream: {error}");
                trace.write(&error);
                error
            })?;
        let started = std::time::Instant::now();
        let mut stdout = child.stdout.take().ok_or("whisper-stream has no output")?;
        let stderr = child.stderr.take().ok_or("whisper-stream has no diagnostics")?;
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let reader_diagnostics = diagnostics.clone();
        let runtime = Arc::new(Mutex::new(RuntimeDiagnostics::new(mode)));
        let reader_runtime = runtime.clone();
        let stderr_trace = trace.clone();
        let stderr_reader = std::thread::spawn(move || {
            let mut stderr = BufReader::new(stderr);
            let mut buffer = Vec::new();
            let mut previous_line = String::new();
            while let Ok(count) = stderr.read_until(b'\n', &mut buffer) {
                if count == 0 { break; }
                {
                    let mut tail = reader_diagnostics.lock().unwrap();
                    tail.extend_from_slice(&buffer);
                    let excess = tail.len().saturating_sub(8192);
                    tail.drain(..excess);
                }
                for line in String::from_utf8_lossy(&buffer).lines() {
                    let line = line.trim_end();
                    reader_runtime.lock().unwrap().observe(line);
                    if !line.is_empty() && line != previous_line {
                        eprintln!("[voice-follow] whisper: {line}");
                        stderr_trace.write(&format!("whisper: {line}"));
                        previous_line = line.to_owned();
                    }
                }
                buffer.clear();
            }
            stderr_trace.write("Whisper diagnostics stream closed");
        });
        let paused = Arc::new(AtomicBool::new(initially_paused));
        let reader_paused = paused.clone();
        let ready = Arc::new(AtomicBool::new(false));
        let reader_ready = ready.clone();
        let stdout_runtime = runtime.clone();
        let stdout_trace = trace.clone();
        let follower_spoken = spoken.clone();
        let last_output_ms = Arc::new(AtomicU64::new(0));
        let reader_last_output_ms = last_output_ms.clone();
        std::thread::spawn(move || {
            let mut parser = StreamParser::default();
            let mut buffer = [0u8; 4096];
            let mut progress = SpeechProgress::new(matcher);
            let mut log = SpeechLog::default();
            while let Ok(count) = stdout.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                reader_last_output_ms.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                // A manual click/scroll establishes a new matching anchor.
                let position = spoken.load(Ordering::Relaxed);
                if position != progress.reported {
                    stdout_trace.write(&format!("manual_jump matched={position}"));
                    progress.seek(position);
                    parser.discard_speech();
                }
                parser.push(&buffer[..count]);
                let hypotheses = std::mem::take(&mut parser.hypotheses);
                if parser.ready {
                    reader_ready.store(true, Ordering::Relaxed);
                }
                // An explicit GPU request cannot silently advance the script
                // with CPU results while the status poll notices the failure.
                if mode == ProcessingMode::Gpu && !stdout_runtime.lock().unwrap().status().is_gpu() {
                    parser.discard_speech();
                    progress.seek(position);
                    continue;
                }
                if reader_paused.load(Ordering::Relaxed) {
                    for hypothesis in &hypotheses {
                        log.print_to(&stdout_trace, hypothesis, &progress.matcher, position, true);
                    }
                    parser.discard_speech();
                    progress.seek(position);
                    continue;
                }
                let next = progress.update(&mut parser);
                // Do not overwrite a manual jump made while this chunk was
                // being matched. The next update will adopt its position.
                if let Err(manual_position) = spoken.compare_exchange(
                    position, next, Ordering::Relaxed, Ordering::Relaxed,
                ) {
                    progress.seek(manual_position);
                    parser.discard_speech();
                }
                for hypothesis in &hypotheses {
                    log.print_to(&stdout_trace, hypothesis, &progress.matcher, spoken.load(Ordering::Relaxed), false);
                }
            }
            stdout_trace.write("Whisper recognition stream closed");
        });
        let initial_position = follower_spoken.load(Ordering::Relaxed);
        Ok(Self {
            child, paused, ready, diagnostics, stderr_reader, failure: None,
            runtime, mode, started, cpu_retry: None, fallback_reason: None,
            trace, spoken: follower_spoken, last_output_ms,
            last_progress: (initial_position, started), last_status: None, last_log: started,
        })
    }

    /// Spawning succeeds before SDL opens the microphone or Whisper loads
    /// the model. Only announce listening after its ready marker, and keep
    /// checking for failures throughout the take.
    pub fn status(&mut self) -> SpeechStatus {
        let status = self.check_status();
        let now = std::time::Instant::now();
        let position = self.spoken.load(Ordering::Relaxed);
        if position != self.last_progress.0 {
            self.last_progress = (position, now);
        }
        if self.last_status.as_ref() != Some(&status)
            || now.duration_since(self.last_log).as_secs() >= 5
        {
            let output_age = self.started.elapsed().as_secs_f64()
                - self.last_output_ms.load(Ordering::Relaxed) as f64 / 1000.0;
            self.trace.write(&format!(
                "status={status:?} matched={position} output_age={output_age:.2}s progress_age={:.2}s",
                now.duration_since(self.last_progress.1).as_secs_f64(),
            ));
            self.last_status = Some(status.clone());
            self.last_log = now;
        }
        status
    }

    fn check_status(&mut self) -> SpeechStatus {
        if let Some(reason) = &self.failure {
            return SpeechStatus::Failed(reason.clone());
        }
        match self.child.try_wait() {
            Ok(Some(status)) if self.stderr_reader.is_finished() => {
                let diagnostics = self.diagnostics.lock().unwrap().clone();
                let text = String::from_utf8_lossy(&diagnostics);
                let detail = text.lines().rev().find(|line| {
                    let lower = line.to_ascii_lowercase();
                    ["error", "failed", "cannot", "couldn't", "could not"]
                        .iter().any(|word| lower.contains(word))
                }).or_else(|| text.lines().rev().find(|line| !line.trim().is_empty()));
                let mut reason = format!("whisper-stream stopped ({status})");
                if let Some(detail) = detail {
                    reason.push_str(": ");
                    reason.extend(detail.trim().chars().take(240));
                }
                return self.startup_failed(reason);
            }
            Err(error) => {
                let reason = format!("could not check whisper-stream: {error}");
                self.failure = Some(reason.clone());
                return SpeechStatus::Failed(reason);
            }
            _ => {}
        }
        if !self.ready.load(Ordering::Relaxed) && self.started.elapsed() > std::time::Duration::from_secs(60) {
            return self.startup_failed("Whisper did not become ready within 60 seconds".into());
        }
        let mut runtime = self.runtime.lock().unwrap().status();
        if self.mode == ProcessingMode::Auto && self.cpu_retry.is_some() {
            if let RuntimeStatus::Cpu { reason } = &runtime {
                if reason.contains("software renderer") {
                    return self.startup_failed(reason.clone());
                }
            }
        }
        if self.mode == ProcessingMode::Gpu && self.ready.load(Ordering::Relaxed) {
            if runtime == RuntimeStatus::Checking {
                if self.started.elapsed() > std::time::Duration::from_secs(60) {
                    return self.startup_failed("Whisper did not confirm GPU processing".into());
                }
                return SpeechStatus::Starting;
            }
            if let RuntimeStatus::Cpu { .. } = &runtime {
                speech_runtime::set_status(runtime.clone());
                let reason = runtime.validate(self.mode).unwrap_err();
                let _ = self.child.kill();
                let _ = self.child.wait();
                self.failure = Some(reason.clone());
                return SpeechStatus::Failed(reason);
            }
        }
        if let (RuntimeStatus::Cpu { reason }, Some(fallback)) = (&mut runtime, &self.fallback_reason) {
            *reason = format!("automatic startup failed: {fallback}");
        }
        speech_runtime::set_status(runtime.clone());
        if self.paused.load(Ordering::Relaxed) {
            SpeechStatus::Paused(runtime.label())
        } else if self.ready.load(Ordering::Relaxed) {
            SpeechStatus::Listening(runtime.label())
        } else {
            SpeechStatus::Starting
        }
    }

    fn startup_failed(&mut self, reason: String) -> SpeechStatus {
        let software_renderer = matches!(self.runtime.lock().unwrap().status(), RuntimeStatus::Cpu { reason } if reason.contains("software renderer"));
        if !self.ready.load(Ordering::Relaxed) || software_renderer {
            if let Some(retry) = self.cpu_retry.take() {
                let paused = self.paused.load(Ordering::Relaxed);
                let _ = self.child.kill();
                let _ = self.child.wait();
                eprintln!("[voice-follow] automatic startup failed; retrying CPU: {reason}");
                self.trace.write(&format!("automatic startup failed; retrying CPU: {reason}"));
                match Self::spawn_with_log(retry.command, retry.matcher, retry.spoken, ProcessingMode::Cpu, paused, self.trace.clone()) {
                    Ok(mut follower) => {
                        follower.fallback_reason = Some(reason);
                        follower.set_paused(paused);
                        *self = follower;
                        return SpeechStatus::Starting;
                    }
                    Err(cpu_error) => {
                        let reason = format!("{reason}; CPU fallback failed: {cpu_error}");
                        self.failure = Some(reason.clone());
                        return SpeechStatus::Failed(reason);
                    }
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.failure = Some(reason.clone());
        SpeechStatus::Failed(reason)
    }

    pub fn set_paused(&self, paused: bool) {
        self.trace.write(if paused { "paused" } else { "resumed" });
        self.paused.store(paused, Ordering::Relaxed);
    }
}

impl Drop for SpeechFollower {
    fn drop(&mut self) {
        self.trace.write(&format!("stopping process pid={} matched={}", self.child.id(), self.spoken.load(Ordering::Relaxed)));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Keep the model's actual hypotheses visible alongside the position sent
/// to the UI. Repeated repaint text and blank terminal clears are omitted.
#[derive(Default)]
struct SpeechLog {
    previous_hypothesis: String,
}

impl SpeechLog {
    fn message(&mut self, hypothesis: &str, matcher: &ScriptMatcher, spoken: usize, paused: bool) -> Option<String> {
        let hypothesis = hypothesis.trim();
        if hypothesis.is_empty() || hypothesis == self.previous_hypothesis {
            return None;
        }
        self.previous_hypothesis = hypothesis.to_owned();
        let matched_word = spoken.checked_sub(1)
            .and_then(|index| matcher.script_words.get(index))
            .map_or("<none>", String::as_str);
        Some(format!(
            "[voice-follow] hypothesis={hypothesis:?} matched={spoken}/{} matched_word={matched_word:?} paused={paused}",
            matcher.script_words.len(),
        ))
    }

    fn print(&mut self, hypothesis: &str, matcher: &ScriptMatcher, spoken: usize, paused: bool) {
        if let Some(message) = self.message(hypothesis, matcher, spoken, paused) {
            eprintln!("{message}");
        }
    }

    fn print_to(&mut self, trace: &VoiceLog, hypothesis: &str, matcher: &ScriptMatcher, spoken: usize, paused: bool) {
        if let Some(message) = self.message(hypothesis, matcher, spoken, paused) {
            eprintln!("{message}");
            trace.write(&message);
        }
    }
}

struct SpeechProgress {
    matcher: ScriptMatcher,
    reported: usize,
    chunk_start: usize,
}

impl SpeechProgress {
    fn new(matcher: ScriptMatcher) -> Self {
        Self { matcher, reported: 0, chunk_start: 0 }
    }

    fn update(&mut self, parser: &mut StreamParser) -> usize {
        for line in std::mem::take(&mut parser.committed) {
            self.follow(&line);
            // Whisper may revise away words we already followed. Preserve
            // that progress when a new audio chunk begins instead of matching
            // all subsequent speech against the script's first eight words.
            self.chunk_start = self.reported;
        }
        self.follow(&parser.current);
        self.reported
    }

    fn follow(&mut self, text: &str) {
        let heard = normalized_words(&strip_annotations(text));
        self.reported = self.reported.max(self.matcher.spoken_count_from(&heard, self.chunk_start));
    }

    fn seek(&mut self, position: usize) {
        self.reported = position;
        // A manual jump highlights this word so it can be read again. The
        // published count includes it, but matching must still start on it.
        self.chunk_start = position.saturating_sub(1);
    }
}

/// Splits whisper-stream's terminal output into committed lines (ended by a
/// newline) and the volatile line it keeps rewriting after carriage returns.
#[derive(Default)]
struct StreamParser {
    committed: Vec<String>,
    current: String,
    hypotheses: Vec<String>,
    escape: bool,
    ready: bool,
}

impl StreamParser {
    fn remember_hypothesis(&mut self) {
        let text = self.current.trim();
        if !text.is_empty() && self.hypotheses.last().is_none_or(|last| last != text) {
            self.hypotheses.push(text.to_owned());
        }
    }

    fn discard_speech(&mut self) {
        self.committed.clear();
        self.current.clear();
        self.hypotheses.clear();
    }

    fn push(&mut self, bytes: &[u8]) {
        for character in String::from_utf8_lossy(bytes).chars() {
            if self.escape {
                // CSI sequences end with a letter, e.g. "\x1b[2K".
                if character.is_ascii_alphabetic() {
                    self.escape = false;
                }
                continue;
            }
            match character {
                '\x1b' => self.escape = true,
                '\r' => {
                    self.remember_hypothesis();
                    self.current.clear();
                }
                '\n' => {
                    self.remember_hypothesis();
                    let line = std::mem::take(&mut self.current);
                    if line.trim() == "[Start speaking]" {
                        self.ready = true;
                    }
                    if !line.trim().is_empty() {
                        self.committed.push(line);
                    }
                }
                _ => self.current.push(character),
            }
        }
        self.remember_hypothesis();
    }

    /// Everything heard so far, without non-speech annotations such as
    /// "[Start speaking]" or "(music)".
    #[cfg(test)]
    fn heard_words(&self) -> Vec<String> {
        self.committed
            .iter()
            .chain(std::iter::once(&self.current))
            .flat_map(|line| normalized_words(&strip_annotations(line)))
            .collect()
    }
}

fn strip_annotations(text: &str) -> String {
    let mut result = String::new();
    let mut depth = 0usize;
    for character in text.chars() {
        match character {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => result.push(character),
            _ => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_enabled_nonempty_scripts_are_active() {
        for (enabled, script, active) in [
            (false, "Read this", false),
            (true, "", false),
            (true, " \n\t ", false),
            (true, "Read this", true),
        ] {
            let settings = TeleprompterSettings {
                enabled,
                script: script.into(),
                ..Default::default()
            };
            assert_eq!(settings.is_active(), active);
        }
    }

    #[test]
    fn matcher_skips_misheard_words_and_never_rewinds() {
        let matcher = ScriptMatcher::new("Hello, world! Today we ship it.");
        let heard = normalized_words("hello world today wee ship");
        assert_eq!(matcher.spoken_count(&heard), 5);
        assert_eq!(matcher.spoken_count(&normalized_words("unrelated noise")), 0);
    }

    #[test]
    fn matcher_keeps_repeated_phrases_near_the_anchor_and_matches_spoken_numbers() {
        let matcher = ScriptMatcher::new(
            "1. The hook (5 to 30 seconds) The hook has twenty seconds to do three things",
        );
        assert_eq!(matcher.spoken_count(&normalized_words("one")), 1);
        assert_eq!(matcher.spoken_count(&normalized_words("one the hook from the hook")), 3);
        assert_eq!(matcher.spoken_count(&normalized_words("one the hook five to thirty")), 6);
        assert_eq!(matcher.spoken_count(&normalized_words("one the hook 5 to 30 seconds")), 7);
        assert_eq!(matcher.spoken_count_from(&normalized_words("the hook has 20 seconds to do 3 things"), 7), 16);
    }

    #[test]
    fn matcher_uses_phrase_evidence_instead_of_jumping_to_a_later_common_word() {
        let matcher = ScriptMatcher::new("the viewer is in the right place raise the stakes right now");
        assert_eq!(matcher.spoken_count(&normalized_words("you are in the right place")), 7);
        assert_eq!(matcher.spoken_count(&normalized_words("you are in the right place raise")), 8);
        let matcher = ScriptMatcher::new("First explain a surprising result before showing the next step");
        assert_eq!(matcher.spoken_count(&normalized_words("the")), 0);
        assert_eq!(matcher.spoken_count(&normalized_words("unrelated noise the")), 0);
        assert_eq!(matcher.spoken_count(&normalized_words("first explain the result")), 2);
    }

    #[test]
    fn chunk_overlap_does_not_stall_on_the_next_word() {
        let mut progress = SpeechProgress::new(ScriptMatcher::new(
            "hint at the grand payoff without giving it away use this three-part formula",
        ));
        let mut parser = StreamParser::default();
        parser.push(b"hint at the grand payoff without giving it\n");
        assert_eq!(progress.update(&mut parser), 8);
        // Actual stream shape: the retained audio repeats "it", then the
        // rest is misheard. The correctly recognized "away" must still win.
        parser.push(b"\r it away. You will have three portfolio.");
        assert_eq!(progress.update(&mut parser), 9);
        parser.push(b"\n\r use this 3 part formula");
        assert_eq!(progress.update(&mut parser), 13);
    }

    #[test]
    fn compound_alignment_keeps_display_word_indices_and_waits_for_whole_word() {
        let matcher = ScriptMatcher::new("use this three-part formula then lean-in");
        assert_eq!(matcher.spoken_count(&normalized_words("use this three")), 2);
        assert_eq!(matcher.spoken_count(&normalized_words("use this 3 part formula")), 4);
        assert_eq!(matcher.spoken_count_from(&normalized_words("then lean in"), 4), 6);
    }

    #[test]
    fn strong_later_phrase_recovers_after_skipped_words_but_common_words_do_not() {
        let script = "confirm the viewer is in the right place raise the stakes why this matters right now \
            and open the first curiosity loop hint at the grand payoff without giving it away \
            use this three-part formula";
        let matcher = ScriptMatcher::new(script);
        let start = 8; // "place" was the last word recognized.
        let away = script.split_whitespace().position(|word| word == "away").unwrap() + 1;
        assert_eq!(matcher.spoken_count_from(&normalized_words("without giving it away"), start), away);
        assert_eq!(matcher.spoken_count_from(&normalized_words("the first"), start), start);
        assert_eq!(matcher.spoken_count_from(&normalized_words("and the"), start), start);

        let repeated = ScriptMatcher::new("missed intro and some other words bright green apples \
            extra words bright green apples");
        assert_eq!(repeated.spoken_count_from(&normalized_words("bright green apples"), 0), 9);
        let far = format!("{} bright green apples", "unheard ".repeat(65));
        assert_eq!(ScriptMatcher::new(&far).spoken_count(&normalized_words("bright green apples")), 0);
    }

    #[test]
    fn continuation_recovers_after_misheard_formula_without_manual_seek() {
        let mut progress = SpeechProgress::new(ScriptMatcher::new(
            "it away. Use this three-part formula. Context lean-in. Establish what the video is about",
        ));
        let mut parser = StreamParser::default();
        parser.push(b"it\n\r it away. You will have three portfolio.\n");
        assert_eq!(progress.update(&mut parser), 2);
        parser.push(b"\r Context lean in");
        assert_eq!(progress.update(&mut parser), 8);
        parser.push(b"\n\r Establish what this video is about");
        assert_eq!(progress.update(&mut parser), 14);
        parser.push(b"\n\r [BLANK_AUDIO]\n");
        assert_eq!(progress.update(&mut parser), 14, "silence must not advance the script");
    }

    #[test]
    fn recovery_tolerates_one_wrong_missing_or_extra_word() {
        let matcher = ScriptMatcher::new(
            "Use this three-part formula. Context lean-in. Establish what the video is about",
        );
        for heard in [
            "Establish what this video is about",
            "Establish what video is about",
            "Establish really what the video is about",
        ] {
            assert_eq!(matcher.spoken_count(&normalized_words(heard)), 12, "{heard}");
        }
        assert_eq!(matcher.spoken_count(&normalized_words("Establish really what this radio is about")), 0);
    }

    #[test]
    fn small_anchor_match_does_not_hide_a_strong_later_phrase() {
        let matcher = ScriptMatcher::new(
            "Use this three-part formula. Context lean-in. Establish what the video is about \
            and connect it to something the viewer already cares about. \
            The right viewers will recognise themselves in it.",
        );
        assert_eq!(matcher.spoken_count(&normalized_words(
            "Use this. Something the viewer already cares about."
        )), 22);
        assert_eq!(matcher.spoken_count(&normalized_words(
            "Use this. The right viewers will recognise themselves in it."
        )), 30);
    }

    #[test]
    fn strong_earlier_phrase_does_not_hide_later_speech_in_the_same_chunk() {
        let script = format!("{}without giving it away. Use this three-part formula. Context lean-in. \
            Establish what the video is about and connect it to something the viewer already cares about. \
            Extra words. Something the viewer already cares about.", "unheard ".repeat(47));
        let mut progress = SpeechProgress {
            matcher: ScriptMatcher::new(&script), reported: 47, chunk_start: 47,
        };
        let mut parser = StreamParser::default();
        parser.push(b"\r without giving it away");
        assert_eq!(progress.update(&mut parser), 51);
        for _ in 0..3 {
            parser.push(b"\r without giving it away. You will have three portfolio. \
                Something the viewer already cares about.");
            assert_eq!(progress.update(&mut parser), 73,
                "later speech should win without a newline or skipping to the repeated occurrence");
        }
    }

    #[test]
    fn weak_recovery_cannot_jump_far_or_count_repeated_words_as_more_evidence() {
        let far_pair = format!("{} the right place", "missed ".repeat(12));
        assert_eq!(ScriptMatcher::new(&far_pair).spoken_count(&normalized_words("the right place")), 0);
        let repeated = format!("{} bright bright bright", "missed ".repeat(12));
        assert_eq!(ScriptMatcher::new(&repeated).spoken_count(&normalized_words("bright bright bright")), 0);
        let matcher = ScriptMatcher::new("Use this three-part formula. Context lean-in.");
        assert_eq!(matcher.spoken_count(&normalized_words("unrelated chatter and the")), 0);
        assert_eq!(matcher.spoken_count(&[]), 0);
    }

    #[test]
    fn overlap_and_recovery_do_not_replay_a_repeated_phrase_in_the_same_chunk() {
        let mut progress = SpeechProgress::new(ScriptMatcher::new("hello world again hello world again"));
        let mut parser = StreamParser::default();
        for _ in 0..3 {
            parser.push(b"\r hello world again");
            assert_eq!(progress.update(&mut parser), 3);
        }
        parser.push(b"\n\r hello world again");
        assert_eq!(progress.update(&mut parser), 6);
    }

    #[test]
    fn stream_parser_commits_lines_and_drops_annotations() {
        let mut parser = StreamParser::default();
        parser.push(b"[Start speaking]\n\x1b[2K\r          \x1b[2K\r Hello wor");
        parser.push(b"\x1b[2K\r Hello world\n\x1b[2K\r today");
        assert_eq!(parser.heard_words(), ["hello", "world", "today"]);
    }

    #[test]
    fn voice_logs_keep_revised_hypotheses_and_show_the_published_word() {
        let mut parser = StreamParser::default();
        parser.push(b"\x1b[2K\r Hello wor\x1b[2K\r Hello, world!\n\x1b[2K\r [BLANK_AUDIO]");
        assert_eq!(parser.hypotheses, ["Hello wor", "Hello, world!", "[BLANK_AUDIO]"]);

        let matcher = ScriptMatcher::new("Hello, world! Again —");
        let mut log = SpeechLog::default();
        assert_eq!(log.message(" Hello, world! ", &matcher, 2, false).unwrap(),
            "[voice-follow] hypothesis=\"Hello, world!\" matched=2/4 matched_word=\"world!\" paused=false");
        assert!(log.message("Hello, world!", &matcher, 2, false).is_none());
        assert!(log.message("   ", &matcher, 2, false).is_none());
        let paused = log.message("[BLANK_AUDIO]", &matcher, 0, true).unwrap();
        assert!(paused.contains("hypothesis=\"[BLANK_AUDIO]\""));
        assert!(paused.contains("matched=0/4 matched_word=\"<none>\" paused=true"));
    }

    #[test]
    fn voice_progress_continues_after_whisper_revises_a_completed_chunk() {
        let mut progress = SpeechProgress::new(ScriptMatcher::new(
            "one two three four five six seven eight nine ten eleven twelve",
        ));
        let mut parser = StreamParser::default();
        parser.push(b"\x1b[2K\r one two three four five six seven eight nine");
        assert_eq!(progress.update(&mut parser), 9);
        // whisper-stream rewrites its current hypothesis before committing it.
        // The words already followed must still anchor the next audio chunk.
        parser.push(b"\x1b[2K\r [BLANK_AUDIO]\n");
        assert_eq!(progress.update(&mut parser), 9);
        parser.push(b"\x1b[2K\r ten eleven twelve");
        assert_eq!(progress.update(&mut parser), 12);
    }

    #[test]
    fn revised_hypotheses_do_not_replay_repeated_words_or_undo_manual_jumps() {
        let mut progress = SpeechProgress::new(ScriptMatcher::new(
            "hello world hello world this is the second sentence",
        ));
        let mut parser = StreamParser::default();
        for _ in 0..3 {
            parser.push(b"\r hello world");
            assert_eq!(progress.update(&mut parser), 2);
        }
        parser.push(b"\n\r hello world");
        assert_eq!(progress.update(&mut parser), 4);
        progress.seek(7);
        parser.discard_speech();
        parser.push(b"\r second sentence");
        assert_eq!(progress.update(&mut parser), 9);
        progress.seek(0);
        parser.discard_speech();
        parser.push(b"\r hello world");
        assert_eq!(progress.update(&mut parser), 2);
    }

    #[test]
    fn clicking_a_repeated_word_reanchors_on_that_occurrence() {
        let mut progress = SpeechProgress::new(ScriptMatcher::new("hello world hello world"));
        let mut parser = StreamParser::default();
        // The highlighted first word is represented by a spoken count of 1.
        progress.seek(1);
        parser.push(b"\r hello");
        assert_eq!(progress.update(&mut parser), 1);
        parser.push(b"\r hello world");
        assert_eq!(progress.update(&mut parser), 2);
        parser.push(b"\n\r hello world");
        assert_eq!(progress.update(&mut parser), 4);
    }

    fn await_status(follower: &mut SpeechFollower, accept: impl Fn(&SpeechStatus) -> bool) -> SpeechStatus {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let status = follower.status();
            if accept(&status) { return status; }
            assert!(std::time::Instant::now() < deadline, "unexpected voice status: {status:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn voice_status_waits_for_readiness_and_reports_microphone_failure() {
        let spoken = Arc::new(AtomicUsize::new(0));
        let mut loading = Command::new("sh");
        loading.args(["-c", "exec sleep 10"]);
        let mut follower = SpeechFollower::spawn(loading, ScriptMatcher::new("hello"), spoken.clone(), ProcessingMode::Auto).unwrap();
        assert_eq!(follower.status(), SpeechStatus::Starting);
        drop(follower);

        let mut failing = Command::new("sh");
        failing.args(["-c", "printf 'audio.init() failed: microphone unavailable\\n' >&2; exit 7"]);
        let mut follower = SpeechFollower::spawn(failing, ScriptMatcher::new("hello"), spoken, ProcessingMode::Auto).unwrap();
        let status = await_status(&mut follower, |status| matches!(status, SpeechStatus::Failed(_)));
        let SpeechStatus::Failed(reason) = &status else { unreachable!() };
        assert!(reason.contains("microphone unavailable"), "{reason}");
        assert!(reason.contains('7'), "{reason}");
        assert_eq!(follower.status(), status, "failure remains visible after the child exits");
    }

    #[test]
    fn selected_microphone_reaches_the_stream_and_live_words_reach_the_ui_position() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("lahza-voice-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let stream = root.join("whisper-stream");
        std::fs::write(&stream, "#!/bin/sh\n\
            test \"$SDL_AUDIODRIVER\" = pulseaudio || exit 11\n\
            test \"$PULSE_SOURCE\" = selected-microphone || exit 12\n\
            printf '[Start speaking]\\n\\033[2K\\r hello world'\n\
            exec sleep 10\n").unwrap();
        std::fs::set_permissions(&stream, std::fs::Permissions::from_mode(0o700)).unwrap();
        // A standalone streaming installation needs no batch CLI beside it.
        let path = std::env::join_paths([&root]).unwrap();
        assert_eq!(find_stream_executable(None, Some(root.join("elsewhere/whisper-cli")), Some(&path)), Some(stream.clone()));
        assert_eq!(find_stream_executable(Some(root.join("missing")), None, Some(&path)), None);
        let spoken = Arc::new(AtomicUsize::new(0));
        let trace = VoiceLog::in_directory(&root.join("logs")).unwrap();
        let trace_path = trace.path().unwrap().to_owned();
        let mut follower = SpeechFollower::spawn_with_log(
            stream_command(&stream, &root.join("model.bin"), Some("selected-microphone"), "en", ProcessingMode::Auto),
            ScriptMatcher::new("hello world"), spoken.clone(), ProcessingMode::Auto,
            false, trace,
        ).unwrap();
        await_status(&mut follower, |status| matches!(status, SpeechStatus::Listening(_)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while spoken.load(Ordering::Relaxed) != 2 {
            assert!(std::time::Instant::now() < deadline, "stream did not publish recognized words");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        follower.set_paused(true);
        assert!(matches!(follower.status(), SpeechStatus::Paused(_)));
        follower.set_paused(false);
        assert!(matches!(follower.status(), SpeechStatus::Listening(_)));
        drop(follower);
        let saved = loop {
            let saved = std::fs::read_to_string(&trace_path).unwrap();
            if saved.contains("hypothesis=\"hello world\"") { break saved; }
            assert!(std::time::Instant::now() < deadline, "recognized words were not saved to diagnostics");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(saved.contains("selected-microphone"));
        assert!(saved.contains("matched=2/2"));
        assert!(saved.contains("output_age="));
        assert!(saved.contains("progress_age="));
        assert!(saved.contains("paused"));
        assert!(saved.contains("resumed"));
        assert!(saved.contains("stopping process"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_gpu_rejects_cpu_stream_without_publishing_words() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'whisper_backend_init_gpu: no GPU found\\n' >&2; printf '[Start speaking]\\n\\r hello world'; exec sleep 10"]);
        let spoken = Arc::new(AtomicUsize::new(0));
        let mut follower = SpeechFollower::spawn(command, ScriptMatcher::new("hello world"), spoken.clone(), ProcessingMode::Gpu).unwrap();
        let status = await_status(&mut follower, |status| matches!(status, SpeechStatus::Failed(_)));
        let SpeechStatus::Failed(reason) = status else { unreachable!() };
        assert!(reason.contains("GPU processing is unavailable"), "{reason}");
        assert_eq!(spoken.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn failed_auto_start_retries_cpu_once_and_keeps_pause_and_reason() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'Vulkan initialization failed\\n' >&2; exit 2"]);
        let mut retry = Command::new("sh");
        retry.args(["-c", "printf 'whisper_model_load: CPU total size = 147.37 MB\\n' >&2; printf '[Start speaking]\\n\\r hello world'; exec sleep 10"]);
        let spoken = Arc::new(AtomicUsize::new(0));
        let matcher = ScriptMatcher::new("hello world");
        let mut follower = SpeechFollower::spawn(command, matcher.clone(), spoken.clone(), ProcessingMode::Auto).unwrap();
        follower.cpu_retry = Some(CpuRetry { command: retry, matcher, spoken: spoken.clone() });
        follower.set_paused(true);
        let status = await_status(&mut follower, |status| matches!(status, SpeechStatus::Paused(label) if label.contains("Using CPU")));
        let SpeechStatus::Paused(label) = status else { unreachable!() };
        assert!(label.contains("Vulkan initialization failed"), "{label}");
        assert_eq!(follower.mode, ProcessingMode::Cpu);
        assert!(follower.cpu_retry.is_none());
        assert_eq!(spoken.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn layout_moves_to_the_next_line_after_its_last_word() {
        let layout = ScriptLayout::new("one two three\nfour", 9.0, |word| word.len() as f32);
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(layout.reading_line(1), 0);
        assert_eq!(layout.reading_line(2), 1);
        assert_eq!(layout.reading_line(3), 2);
    }

    #[test]
    #[ignore = "set LAHZA_VOICE_REPLAY_EVENTS and LAHZA_VOICE_REPLAY_CONFIG to replay saved local Whisper output"]
    fn replay_saved_whisper_output_against_local_script() {
        let events = std::env::var_os("LAHZA_VOICE_REPLAY_EVENTS").expect("set LAHZA_VOICE_REPLAY_EVENTS");
        let config = std::env::var_os("LAHZA_VOICE_REPLAY_CONFIG").expect("set LAHZA_VOICE_REPLAY_CONFIG");
        let settings: TeleprompterSettings = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
        let events: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(events).unwrap()).unwrap();
        let mut progress = SpeechProgress::new(ScriptMatcher::new(&settings.script));
        let mut parser = StreamParser::default();
        let mut log = SpeechLog::default();
        for event in events {
            parser.push(event["text"].as_str().unwrap().as_bytes());
            let position = progress.update(&mut parser);
            for hypothesis in std::mem::take(&mut parser.hypotheses) {
                if let Some(message) = log.message(&hypothesis, &progress.matcher, position, false) {
                    eprintln!("{:.3}s {message}", event["seconds"].as_f64().unwrap_or_default());
                }
            }
        }
        if let Ok(expected) = std::env::var("LAHZA_VOICE_REPLAY_EXPECTED") {
            assert_eq!(progress.reported, expected.parse::<usize>().unwrap());
        }
    }
}
