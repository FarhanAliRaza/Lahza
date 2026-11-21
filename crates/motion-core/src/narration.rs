//! Narration timing contract (design section 4.2).
//!
//! Voices are made outside the engine by any provider (Kokoro, ElevenLabs,
//! Azure, OpenAI, a recording) and come in as a *manifest*: one clip per
//! line, each with its audio, its script text and the provider's timings
//! (`words` or `chars`, seconds from the clip's start). The engine never
//! listens for words: it aligns the script to the timings, builds the
//! `line`, `sentence`, `word` and `cue` marker tracks, and validates them.
//!
//! Script text is what is *shown* ("81", "U.S."); timings carry what was
//! *spoken* ("eighty-one", "U S"). Alignment tolerates the difference, and
//! markers keep the display text with the spoken form as an alternative.
//!
//! Cues name moments in the script so references survive edits:
//! `One bomb. Over [strike]Iran.` gives `cue:strike` (the cued word) and
//! `cue:strike.end`; `[map]Over Iran[/map]` spans words.

use crate::markers::{Marker, MarkerTrack};
use serde::{Deserialize, Serialize};

/// What an adapter writes (`tools/narration/*`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub clips: Vec<ManifestClip>,
    /// Where the first clip starts when it has no `at` (default 0.5 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<f64>,
    /// Silence between clips placed one after another (default 0.35 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestClip {
    /// Stable name (`line:hook`); defaults to `line<n>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Audio file, relative to the manifest.
    pub audio: String,
    /// Script text as shown, with `[cue]` markup.
    pub text: String,
    /// Timeline start; without it the clip follows the previous one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    /// Silence before this clip when it follows the previous one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<TimedWord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<Vec<TimedChar>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedChar {
    #[serde(alias = "character")]
    pub char: String,
    pub start: f64,
    pub end: f64,
}

/// The project's narration (`narration.json`): clips placed on the
/// timeline with their aligned words. Marker tracks are derived from it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Narration {
    pub clips: Vec<NarrationClip>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NarrationClip {
    pub id: String,
    /// Audio file in the project.
    pub path: String,
    pub text: String,
    /// Timeline start of the audio.
    pub at: f64,
    /// Audio length, seconds.
    pub duration: f64,
    #[serde(default)]
    pub gain_db: f64,
    pub words: Vec<Word>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<Cue>,
    /// Hash of the audio file's bytes, and of everything else the clip
    /// was made from (text, timings): unchanged clips are not re-imported.
    pub audio_hash: String,
    pub hash: String,
}

/// A script word with its time in the clip (seconds from the clip start).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// As shown ("81", "U.S.", "Iran.").
    pub text: String,
    /// As spoken, when that differs ("eighty-one").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spoken: Option<String>,
    /// None: the provider gave no time for this word.
    pub start: Option<f64>,
    pub end: Option<f64>,
    /// Loudest 10 ms of the word, dB relative to the clip's speech level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loud: Option<f64>,
}

/// `[name]` before word `from`; spans to `to` (exclusive) — one word, or
/// up to `[/name]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub name: String,
    pub from: usize,
    pub to: usize,
}

/// Loudness below the clip's speech level at which a word is silent.
pub const SILENT_DB: f64 = -30.0;

/// Splits script text into display words and cues.
pub fn parse_script(text: &str) -> Result<(Vec<String>, Vec<Cue>), String> {
    let mut words: Vec<String> = Vec::new();
    let mut cues: Vec<Cue> = Vec::new();
    let mut open: Vec<usize> = Vec::new(); // cues waiting for their end
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let flush = |current: &mut String, words: &mut Vec<String>| {
        if !current.is_empty() {
            if !normal(current).is_empty() {
                words.push(std::mem::take(current));
            } else if let Some(last) = words.last_mut() {
                // Punctuation alone ("—", "…") joins the previous word.
                last.push_str(current);
                current.clear();
            } else {
                current.clear();
            }
        }
    };
    while let Some(c) = chars.next() {
        if c == '[' {
            let mut name = String::new();
            for d in chars.by_ref() {
                if d == ']' {
                    break;
                }
                name.push(d);
            }
            let name = name.trim().to_string();
            if let Some(end) = name.strip_prefix('/') {
                flush(&mut current, &mut words);
                let i = open.iter().rposition(|&k| cues[k].name == end).ok_or_else(|| format!("'[/{end}]' closes no open [{end}]"))?;
                let k = open.remove(i);
                cues[k].to = words.len().max(cues[k].from + 1);
            } else {
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return Err(format!("cue '[{name}]': letters, digits, '-' and '_' only"));
                }
                flush(&mut current, &mut words);
                open.push(cues.len());
                // `to` is set at [/name], or to the next word only.
                cues.push(Cue { name, from: words.len(), to: usize::MAX });
            }
            continue;
        }
        if c.is_whitespace() {
            flush(&mut current, &mut words);
        } else {
            current.push(c);
        }
    }
    flush(&mut current, &mut words);
    for cue in &mut cues {
        if cue.to == usize::MAX {
            cue.to = cue.from + 1;
        }
    }
    // A cue without [/name] covers one word.
    let _ = open;
    Ok((words, cues))
}

/// Letters and digits, lower-case: the form words are compared in.
pub fn normal(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Characters (ElevenLabs `with-timestamps`) grouped into words at
/// whitespace.
pub fn chars_to_words(chars: &[TimedChar]) -> Vec<TimedWord> {
    let mut out = Vec::new();
    let mut cur: Option<TimedWord> = None;
    for c in chars {
        if c.char.chars().all(char::is_whitespace) {
            out.extend(cur.take());
            continue;
        }
        match &mut cur {
            Some(w) => {
                w.text.push_str(&c.char);
                w.end = c.end;
            }
            None => cur = Some(TimedWord { text: c.char.clone(), start: c.start, end: c.end }),
        }
    }
    out.extend(cur);
    out
}

const ONES: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
];
const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

fn number_words(n: u64) -> String {
    fn below_thousand(n: u64, out: &mut Vec<String>) {
        if n >= 100 {
            out.push(ONES[(n / 100) as usize].into());
            out.push("hundred".into());
        }
        let r = n % 100;
        if r == 0 {
            return;
        }
        if r < 20 {
            out.push(ONES[r as usize].into());
        } else {
            out.push(TENS[(r / 10) as usize].into());
            if r % 10 != 0 {
                out.push(ONES[(r % 10) as usize].into());
            }
        }
    }
    if n == 0 {
        return "zero".into();
    }
    let mut out = Vec::new();
    for (scale, name) in [(1_000_000_000_000u64, "trillion"), (1_000_000_000, "billion"), (1_000_000, "million"), (1_000, "thousand")] {
        if n >= scale && (n / scale) % 1000 > 0 {
            below_thousand((n / scale) % 1000, &mut out);
            out.push(name.into());
        }
    }
    below_thousand(n % 1000, &mut out);
    out.join(" ")
}

/// Ways a display word may be spoken, compact (letters and digits only).
pub fn spoken_forms(display: &str) -> Vec<String> {
    let mut out = vec![normal(display)];
    let core: String = display.trim_matches(|c: char| !c.is_alphanumeric() && c != '$' && c != '%').to_string();
    // Numbers: 81, 1,000, 3.5, 49%, $20, 1971, 5th.
    let money = core.starts_with('$');
    let percent = core.ends_with('%');
    let digits: String = core.trim_start_matches('$').trim_end_matches('%').replace(',', "");
    let (num, suffix) = match digits.find(|c: char| c.is_alphabetic()) {
        Some(i) => (&digits[..i], &digits[i..]),
        None => (&digits[..], ""),
    };
    if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit() || c == '.') && num.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        let (int, frac) = num.split_once('.').unwrap_or((num, ""));
        if let Ok(n) = int.parse::<u64>() {
            let mut forms = vec![number_words(n)];
            if (1000..2100).contains(&n) && frac.is_empty() {
                // Years: "nineteen seventy-one", "two thousand five".
                let (hi, lo) = (n / 100, n % 100);
                let lo_words = if lo == 0 { "hundred".to_string() } else if lo < 10 { format!("oh {}", ONES[lo as usize]) } else { number_words(lo) };
                forms.push(format!("{} {lo_words}", number_words(hi)));
            }
            if !frac.is_empty() {
                let tail: Vec<&str> = frac.chars().map(|d| ONES[d.to_digit(10).unwrap_or(0) as usize]).collect();
                forms = forms.into_iter().map(|f| format!("{f} point {}", tail.join(" "))).collect();
            }
            let ordinal = matches!(suffix.to_ascii_lowercase().as_str(), "st" | "nd" | "rd" | "th");
            for f in forms {
                let mut f = f;
                if ordinal {
                    for (a, b) in [("one", "first"), ("two", "second"), ("three", "third"), ("five", "fifth"), ("eight", "eighth"), ("nine", "ninth"), ("twelve", "twelfth")] {
                        if f.ends_with(a) {
                            f = format!("{}{b}", &f[..f.len() - a.len()]);
                        }
                    }
                    if !f.ends_with("st") && !f.ends_with("nd") && !f.ends_with("rd") && !f.ends_with("th") {
                        f = if f.ends_with('y') { format!("{}ieth", &f[..f.len() - 1]) } else { format!("{f}th") };
                    }
                }
                if percent {
                    f.push_str(" percent");
                }
                if money {
                    f.push_str(" dollars");
                }
                if !ordinal && !suffix.is_empty() {
                    // "20s", "5k": keep the letters.
                    f.push_str(suffix);
                }
                out.push(normal(&f));
            }
        }
    }
    // Symbols on their own.
    for (sym, word) in [("&", "and"), ("%", "percent"), ("+", "plus"), ("=", "equals"), ("@", "at")] {
        if display.trim() == sym {
            out.push(word.into());
        }
    }
    out.retain(|f| !f.is_empty());
    out.dedup();
    out
}

fn edit_distance(a: &[u8], b: &[u8]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// 0 (same) … 1 (nothing alike).
fn mismatch(a: &str, b: &str) -> f64 {
    if a == b {
        return 0.0;
    }
    let n = a.len().max(b.len()).max(1);
    edit_distance(a.as_bytes(), b.as_bytes()) as f64 / n as f64
}

/// Aligns display words to timed (spoken) words. A display word takes one
/// or more spoken words ("81" → "eighty", "one"), or none (no time); a
/// spoken word may be left over (a provider's extra token).
pub fn align(display: &[String], timed: &[TimedWord]) -> Vec<(Option<(f64, f64)>, Option<String>)> {
    const MAX_SPAN: usize = 5;
    const MISSING: f64 = 1.0;
    const EXTRA: f64 = 0.8;
    let timed: Vec<(String, &TimedWord)> = timed.iter().map(|w| (normal(&w.text), w)).filter(|(n, _)| !n.is_empty()).collect();
    let forms: Vec<Vec<String>> = display.iter().map(|d| spoken_forms(d)).collect();
    let (n, m) = (display.len(), timed.len());
    // cost[i][j]: best cost aligning display[..i] with timed[..j].
    let mut cost = vec![vec![f64::INFINITY; m + 1]; n + 1];
    let mut back = vec![vec![(0usize, 0usize); m + 1]; n + 1];
    cost[0][0] = 0.0;
    for i in 0..=n {
        for j in 0..=m {
            let c = cost[i][j];
            if !c.is_finite() {
                continue;
            }
            if i < n && c + MISSING < cost[i + 1][j] {
                cost[i + 1][j] = c + MISSING;
                back[i + 1][j] = (i, j);
            }
            if j < m && c + EXTRA < cost[i][j + 1] {
                cost[i][j + 1] = c + EXTRA;
                back[i][j + 1] = (i, j);
            }
            if i < n {
                let mut joined = String::new();
                for k in 1..=MAX_SPAN.min(m - j) {
                    joined.push_str(&timed[j + k - 1].0);
                    let d = forms[i].iter().map(|f| mismatch(f, &joined)).fold(f64::INFINITY, f64::min);
                    let step = d * 1.6 + 0.04 * (k - 1) as f64;
                    if step < MISSING + EXTRA * k as f64 && c + step < cost[i + 1][j + k] {
                        cost[i + 1][j + k] = c + step;
                        back[i + 1][j + k] = (i, j);
                    }
                }
            }
        }
    }
    let mut out = vec![(None, None); n];
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let (pi, pj) = back[i][j];
        if pi + 1 == i && pj < j {
            let span = &timed[pj..j];
            let spoken: Vec<&str> = span.iter().map(|(_, w)| w.text.trim()).collect();
            let spoken = spoken.join(" ");
            let same = normal(&spoken) == normal(&display[pi]);
            out[pi] = (Some((span[0].1.start, span[span.len() - 1].1.end)), (!same).then_some(spoken));
        }
        (i, j) = (pi, pj);
    }
    out
}

/// One clip from its script and timings (times relative to the clip).
pub fn build_clip(text: &str, timed: &[TimedWord]) -> Result<(Vec<Word>, Vec<Cue>), String> {
    let (display, cues) = parse_script(text)?;
    let aligned = align(&display, timed);
    let words = display
        .into_iter()
        .zip(aligned)
        .map(|(text, (time, spoken))| Word { text, spoken, start: time.map(|t| t.0), end: time.map(|t| t.1), loud: None })
        .collect();
    Ok((words, cues))
}

fn sentence_end(word: &str) -> bool {
    word.trim_end_matches(|c: char| matches!(c, '"' | '\'' | ')' | '”' | '’' | ']')).ends_with(['.', '!', '?', '…'])
}

impl Narration {
    /// Word times on the timeline; a word without a time sits between its
    /// neighbours (so references still resolve; `check` reports it).
    fn timeline_words(clip: &NarrationClip) -> Vec<(f64, f64)> {
        let n = clip.words.len();
        let mut out: Vec<(f64, f64)> = Vec::with_capacity(n);
        for i in 0..n {
            let w = &clip.words[i];
            let (s, e) = match (w.start, w.end) {
                (Some(s), Some(e)) => (s, e.max(s)),
                _ => {
                    let prev = out.last().map(|p| p.1 - clip.at).unwrap_or(0.0);
                    let next = clip.words[i + 1..].iter().find_map(|w| w.start).unwrap_or(prev);
                    (prev, next.max(prev))
                }
            };
            out.push((clip.at + s, clip.at + e));
        }
        out
    }

    /// `line`, `sentence`, `word` and `cue` tracks.
    pub fn tracks(&self) -> Vec<MarkerTrack> {
        let source = Some("narration".to_string());
        let mut line = MarkerTrack { name: "line".into(), source: source.clone(), markers: Vec::new() };
        let mut sentence = MarkerTrack { name: "sentence".into(), source: source.clone(), markers: Vec::new() };
        let mut word = MarkerTrack { name: "word".into(), source: source.clone(), markers: Vec::new() };
        let mut cue = MarkerTrack { name: "cue".into(), source, markers: Vec::new() };
        for clip in &self.clips {
            let times = Self::timeline_words(clip);
            let label = clip.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            let (t0, t1) = match (times.first(), times.last()) {
                (Some(a), Some(b)) => (a.0, b.1),
                _ => (clip.at, clip.at + clip.duration),
            };
            line.markers.push(Marker { t: t0, end: Some(t1), label: Some(label), id: Some(clip.id.clone()), ..Default::default() });
            let mut start = 0;
            for (i, w) in clip.words.iter().enumerate() {
                word.markers.push(Marker { t: times[i].0, end: Some(times[i].1), label: Some(w.text.clone()), alt: w.spoken.clone(), ..Default::default() });
                if sentence_end(&w.text) || i + 1 == clip.words.len() {
                    let text = clip.words[start..=i].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                    sentence.markers.push(Marker { t: times[start].0, end: Some(times[i].1), label: Some(text), ..Default::default() });
                    start = i + 1;
                }
            }
            for c in &clip.cues {
                let (t, end) = if c.from < times.len() {
                    (times[c.from].0, times[c.to.min(times.len()).max(c.from + 1) - 1].1)
                } else {
                    // A cue after the last word: the end of the line.
                    (t1, t1)
                };
                cue.markers.push(Marker { t, end: Some(end), label: Some(c.name.clone()), ..Default::default() });
            }
        }
        vec![line, sentence, word, cue]
    }

    /// Problems with the timings, found without listening: (clip id, word,
    /// message). All are errors: a reference on such a word is wrong.
    pub fn validate(&self) -> Vec<(String, Option<String>, String)> {
        let mut out = Vec::new();
        let mut prev_clip_end = f64::NEG_INFINITY;
        for clip in &self.clips {
            if clip.at < prev_clip_end - 1e-3 {
                out.push((clip.id.clone(), None, format!("starts at {:.2} s, before the previous line ends ({prev_clip_end:.2} s)", clip.at)));
            }
            prev_clip_end = clip.at + clip.duration;
            let mut last: Option<f64> = None;
            for (i, w) in clip.words.iter().enumerate() {
                let tag = Some(format!("{} (word {} of the line)", w.text, i + 1));
                let (Some(s), Some(e)) = (w.start, w.end) else {
                    out.push((clip.id.clone(), tag, "has no time: the voice's timings skip it (check the script against what was spoken)".into()));
                    continue;
                };
                if e < s || last.is_some_and(|l| s < l - 1e-3) {
                    out.push((clip.id.clone(), tag.clone(), format!("times out of order ({s:.3}–{e:.3} s)")));
                }
                if s < -0.02 || e > clip.duration + 0.05 {
                    out.push((clip.id.clone(), tag.clone(), format!("{s:.2}–{e:.2} s lies outside the clip (0–{:.2} s)", clip.duration)));
                }
                if let Some(l) = w.loud {
                    if l < SILENT_DB {
                        out.push((clip.id.clone(), tag, format!("lands in silence ({l:.0} dB under the speech): the timing is off")));
                    }
                }
                last = Some(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tw(words: &[(&str, f64, f64)]) -> Vec<TimedWord> {
        words.iter().map(|(t, s, e)| TimedWord { text: t.to_string(), start: *s, end: *e }).collect()
    }

    #[test]
    fn cues_and_display_words() {
        let (words, cues) = parse_script("One bomb. Over [strike]Iran. [map]Its 81 cities[/map] — gone.").unwrap();
        // Punctuation on its own joins the word before it.
        assert_eq!(words, ["One", "bomb.", "Over", "Iran.", "Its", "81", "cities—", "gone."].map(String::from));
        assert_eq!(cues[0], Cue { name: "strike".into(), from: 3, to: 4 });
        assert_eq!(cues[1], Cue { name: "map".into(), from: 4, to: 7 });
        assert!(parse_script("a [/x] b").is_err());
    }

    #[test]
    fn spoken_forms_cover_numbers_and_acronyms() {
        assert!(spoken_forms("81").contains(&"eightyone".to_string()));
        assert!(spoken_forms("1971").contains(&"nineteenseventyone".to_string()));
        assert!(spoken_forms("49%").contains(&"fortyninepercent".to_string()));
        assert!(spoken_forms("$20").contains(&"twentydollars".to_string()));
        assert!(spoken_forms("3.5").contains(&"threepointfive".to_string()));
        assert!(spoken_forms("1,000").contains(&"onethousand".to_string()));
        assert!(spoken_forms("2nd").contains(&"second".to_string()));
        assert_eq!(normal("U.S."), "us");
    }

    #[test]
    fn display_aligns_to_spoken_words() {
        let display: Vec<String> = ["The", "U.S.", "has", "81", "bases."].map(String::from).to_vec();
        let timed = tw(&[("The", 0.0, 0.1), ("U", 0.12, 0.3), ("S", 0.32, 0.5), ("has", 0.55, 0.7), ("eighty-one", 0.75, 1.2), ("bases", 1.25, 1.6)]);
        let a = align(&display, &timed);
        assert_eq!(a[1].0, Some((0.12, 0.5)));
        assert_eq!(a[1].1, None, "\"U S\" compares equal to \"U.S.\"");
        assert_eq!(a[3].1.as_deref(), Some("eighty-one"));
        assert_eq!(a[3].0, Some((0.75, 1.2)));
        assert_eq!(a[4].0, Some((1.25, 1.6)));
        assert_eq!(a[4].1, None, "same word, punctuation aside");
        // A word the voice skipped has no time; the rest still align.
        let a = align(&display, &tw(&[("The", 0.0, 0.1), ("U", 0.12, 0.3), ("S", 0.32, 0.5), ("eighty", 0.75, 0.9), ("one", 0.92, 1.2), ("bases", 1.25, 1.6)]));
        assert_eq!(a[2].0, None);
        assert_eq!(a[3].0, Some((0.75, 1.2)));
    }

    #[test]
    fn characters_group_into_words() {
        let chars: Vec<TimedChar> = "Hi you".chars().enumerate().map(|(i, c)| TimedChar { char: c.to_string(), start: i as f64 * 0.1, end: i as f64 * 0.1 + 0.1 }).collect();
        let w = chars_to_words(&chars);
        assert_eq!(w.len(), 2);
        assert_eq!(w[1].text, "you");
        assert!((w[1].start - 0.3).abs() < 1e-9 && (w[1].end - 0.6).abs() < 1e-9);
    }

    #[test]
    fn tracks_and_validation() {
        let (words, cues) = build_clip("One bomb. Over [strike]Iran.", &tw(&[("One", 0.0, 0.2), ("bomb", 0.25, 0.6), ("Over", 0.9, 1.1), ("Iran", 1.15, 1.6)])).unwrap();
        let n = Narration { clips: vec![NarrationClip { id: "hook".into(), path: "a.wav".into(), text: String::new(), at: 10.0, duration: 1.8, gain_db: 0.0, words, cues, audio_hash: String::new(), hash: String::new() }] };
        let tracks = n.tracks();
        let get = |name: &str| tracks.iter().find(|t| t.name == name).unwrap();
        assert_eq!(get("line").markers[0].id.as_deref(), Some("hook"));
        assert_eq!(get("sentence").markers.len(), 2);
        assert_eq!(get("cue").markers[0].t, 11.15);
        assert_eq!(get("cue").markers[0].end, Some(11.6));
        assert!(n.validate().is_empty());
        let mut bad = n.clone();
        bad.clips[0].words[1].start = None;
        bad.clips[0].words[1].end = None;
        bad.clips[0].words[3].end = Some(2.5);
        bad.clips[0].words[0].loud = Some(-45.0);
        let issues = bad.validate();
        assert_eq!(issues.len(), 3, "{issues:?}");
    }

    #[test]
    fn references_by_line_id_spoken_form_and_cue() {
        use crate::markers::Markers;
        use crate::time::{Fps, TimeBase};
        use crate::timeref::{NoElements, TimeRef};
        let (words, cues) = build_clip("It hit [hit]81 cities.", &tw(&[("It", 0.0, 0.1), ("hit", 0.15, 0.3), ("eighty-one", 0.35, 0.8), ("cities", 0.85, 1.2)])).unwrap();
        let n = Narration { clips: vec![NarrationClip { id: "s07".into(), path: String::new(), text: String::new(), at: 5.0, duration: 1.3, gain_db: 0.0, words, cues, audio_hash: String::new(), hash: String::new() }] };
        let mut m = Markers::default();
        for t in n.tracks() {
            m.insert(t);
        }
        let base = TimeBase::new(Fps::whole(30));
        let at = |r: &str| TimeRef::from(r).resolve(&m, &base, &mut NoElements).unwrap();
        assert_eq!(at("line:s07"), 5.0);
        assert_eq!(at("word:'81'"), 5.35, "numbers are quoted: word:81 is the 81st word");
        assert_eq!(at("word:eighty-one"), 5.35, "the spoken form finds the shown word");
        assert_eq!(at("cue:hit.end"), 5.8);
    }
}
