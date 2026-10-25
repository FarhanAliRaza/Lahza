//! Teleprompter: the script model, the fuzzy matcher that tracks how far the
//! speaker has read, the fixed line layout the prompter scrolls through, and
//! live speech following through whisper.cpp's `whisper-stream`.
//!
//! Following is best effort. Without whisper.cpp, a speech model or a
//! microphone the script still shows and can be scrolled by hand; it never
//! interferes with the recording.

use super::transcription::{whisper_executable, SpeechModel};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

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
}

impl Default for TeleprompterSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            script: String::new(),
            lines: 3,
            font_size: 26.0,
            follow_voice: true,
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
        .map(normalize)
        .filter(|word| !word.is_empty())
        .collect()
}

/// How many script display words have been spoken, given everything the
/// recognizer has heard. Greedy with a short lookahead, so a misheard or
/// skipped word never stalls the prompter.
#[derive(Clone, Debug, Default)]
pub struct ScriptMatcher {
    entries: Vec<(String, usize)>,
}

impl ScriptMatcher {
    const LOOKAHEAD: usize = 8;

    pub fn new(script: &str) -> Self {
        Self {
            entries: display_words(script)
                .into_iter()
                .enumerate()
                .filter_map(|(index, word)| {
                    let normalized = normalize(word);
                    (!normalized.is_empty()).then_some((normalized, index))
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn spoken_count(&self, spoken: &[String]) -> usize {
        let mut position = 0;
        for word in spoken {
            if position >= self.entries.len() {
                break;
            }
            let end = (position + Self::LOOKAHEAD).min(self.entries.len());
            if let Some(found) = (position..end).find(|&index| self.entries[index].0 == *word) {
                position = found + 1;
            }
        }
        position.checked_sub(1).map_or(0, |last| self.entries[last].1 + 1)
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

/// Live speech following: runs `whisper-stream` on the default microphone
/// and publishes the (monotonic) number of script words spoken.
pub struct SpeechFollower {
    child: Child,
    paused: Arc<AtomicBool>,
}

impl SpeechFollower {
    /// Start following when whisper.cpp and the speech model are present.
    pub fn start(script: &str, spoken: Arc<AtomicUsize>) -> Result<Self, String> {
        let matcher = ScriptMatcher::new(script);
        if matcher.is_empty() {
            return Err("the script has no words".into());
        }
        let whisper = whisper_executable().ok_or("whisper.cpp is not installed")?;
        let stream = whisper.with_file_name("whisper-stream");
        if !stream.is_file() {
            return Err("whisper-stream is not installed".into());
        }
        let model = SpeechModel::Base;
        if !model.is_installed() {
            return Err("the speech model is not downloaded".into());
        }
        let threads = std::thread::available_parallelism()
            .map(|count| (count.get() / 2).clamp(2, 8))
            .unwrap_or(4);
        let mut child = Command::new(stream)
            .arg("-m")
            .arg(model.path())
            .args(["--step", "800", "--length", "5000", "--keep", "200", "-l", "auto", "-t"])
            .arg(threads.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start whisper-stream: {error}"))?;
        let mut stdout = child.stdout.take().ok_or("whisper-stream has no output")?;
        let paused = Arc::new(AtomicBool::new(false));
        let reader_paused = paused.clone();
        std::thread::spawn(move || {
            let mut parser = StreamParser::default();
            let mut buffer = [0u8; 4096];
            let mut reported = 0;
            while let Ok(count) = stdout.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                parser.push(&buffer[..count]);
                if reader_paused.load(Ordering::Relaxed) {
                    continue;
                }
                let heard = parser.heard_words();
                let count = matcher.spoken_count(&heard);
                // Progress only moves forward: a revised guess must never
                // scroll back while someone is mid-sentence.
                if count > reported {
                    reported = count;
                    spoken.fetch_max(count, Ordering::Relaxed);
                }
            }
        });
        Ok(Self { child, paused })
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

impl Drop for SpeechFollower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Splits whisper-stream's terminal output into committed lines (ended by a
/// newline) and the volatile line it keeps rewriting after carriage returns.
#[derive(Default)]
struct StreamParser {
    committed: Vec<String>,
    current: String,
    escape: bool,
}

impl StreamParser {
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
                '\r' => self.current.clear(),
                '\n' => {
                    let line = std::mem::take(&mut self.current);
                    if !line.trim().is_empty() {
                        self.committed.push(line);
                    }
                }
                _ => self.current.push(character),
            }
        }
    }

    /// Everything heard so far, without non-speech annotations such as
    /// "[Start speaking]" or "(music)".
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
    fn stream_parser_commits_lines_and_drops_annotations() {
        let mut parser = StreamParser::default();
        parser.push(b"[Start speaking]\n\x1b[2K\r          \x1b[2K\r Hello wor");
        parser.push(b"\x1b[2K\r Hello world\n\x1b[2K\r today");
        assert_eq!(parser.heard_words(), ["hello", "world", "today"]);
    }

    #[test]
    fn layout_moves_to_the_next_line_after_its_last_word() {
        let layout = ScriptLayout::new("one two three\nfour", 9.0, |word| word.len() as f32);
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(layout.reading_line(1), 0);
        assert_eq!(layout.reading_line(2), 1);
        assert_eq!(layout.reading_line(3), 2);
    }
}
