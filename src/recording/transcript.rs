//! Narration transcript of a recording: timed words on the source timeline
//! and the readable subtitle cues derived from them. Words drive karaoke
//! highlighting and transcript-based cutting; cues drive the subtitle bar
//! and are the editable source of truth for its text.

use super::clips::RecordingClipTimeline;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One spoken word on the source timeline. `text` keeps the recognizer's
/// spacing and punctuation so concatenation reconstructs the transcript.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

impl TranscriptWord {
    /// The word as shown in the transcript editor.
    pub fn display(&self) -> &str {
        self.text.trim()
    }

    /// Lower-case letters and digits only, for filler matching.
    pub fn normalized(&self) -> String {
        self.text
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '\'')
            .flat_map(char::to_lowercase)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubtitleCue {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Transcript {
    pub words: Vec<TranscriptWord>,
    pub cues: Vec<SubtitleCue>,
    /// Language the recognizer detected or was told to use.
    pub language: Option<String>,
}

impl Transcript {
    const MAXIMUM_CUE_CHARACTERS: usize = 42;
    const MAXIMUM_CUE_DURATION: f64 = 4.0;
    const CUE_GAP_THRESHOLD: f64 = 0.9;
    const MINIMUM_CUE_DURATION: f64 = 0.8;

    pub fn from_words(mut words: Vec<TranscriptWord>, language: Option<String>) -> Self {
        words.retain(|word| {
            word.start.is_finite() && word.end.is_finite() && !word.text.trim().is_empty()
        });
        words.sort_by(|a, b| a.start.total_cmp(&b.start));
        for word in &mut words {
            word.end = word.end.max(word.start);
        }
        let cues = Self::make_cues(&words);
        Self { words, cues, language }
    }

    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    /// Chunk words into readable cues: break on length, a pause, or duration.
    pub fn make_cues(words: &[TranscriptWord]) -> Vec<SubtitleCue> {
        let mut cues = Vec::new();
        let mut text = String::new();
        let mut start = 0.0;
        let mut end: f64 = 0.0;
        let flush = |text: &mut String, start: f64, end: f64, cues: &mut Vec<SubtitleCue>| {
            let trimmed = text.trim().to_string();
            text.clear();
            if !trimmed.is_empty() {
                cues.push(SubtitleCue {
                    id: Uuid::new_v4(),
                    start,
                    end: end.max(start + Self::MINIMUM_CUE_DURATION),
                    text: trimmed,
                });
            }
        };
        for word in words {
            let length = word.text.trim().chars().count();
            if !text.is_empty() {
                let long = text.trim().chars().count() + 1 + length > Self::MAXIMUM_CUE_CHARACTERS;
                let pause = word.start - end > Self::CUE_GAP_THRESHOLD;
                let lengthy = word.end - start > Self::MAXIMUM_CUE_DURATION;
                if long || pause || lengthy {
                    flush(&mut text, start, end, &mut cues);
                }
            }
            if text.is_empty() {
                start = word.start;
                end = word.end;
            }
            if !text.is_empty() && !word.text.starts_with(char::is_whitespace) {
                text.push(' ');
            }
            text.push_str(&word.text);
            end = end.max(word.end);
        }
        flush(&mut text, start, end, &mut cues);
        // A minimum-duration cue must not run into the next one.
        for index in 1..cues.len() {
            let next_start = cues[index].start;
            let previous = &mut cues[index - 1];
            if previous.end > next_start {
                previous.end = next_start.max(previous.start + 0.05);
            }
        }
        cues
    }

    /// The transcript on the edited timeline: words and cues follow cuts,
    /// speed changes and reordering. A cue that spans a cut appears on both
    /// sides of it; content that was cut away disappears.
    pub fn in_editor_time(&self, clips: &RecordingClipTimeline) -> Transcript {
        let mut words: Vec<TranscriptWord> = self
            .words
            .iter()
            .flat_map(|word| {
                clips
                    .slices_overlapping(word.start, word.end.max(word.start + 0.001))
                    .into_iter()
                    .take(1)
                    .map(|slice| TranscriptWord {
                        text: word.text.clone(),
                        start: slice.editor_start,
                        end: slice.editor_end,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        words.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut cues: Vec<SubtitleCue> = self
            .cues
            .iter()
            .flat_map(|cue| {
                clips
                    .slices_overlapping(cue.start, cue.end)
                    .into_iter()
                    .filter(|slice| slice.editor_end - slice.editor_start >= 0.15)
                    .map(|slice| SubtitleCue {
                        id: cue.id,
                        start: slice.editor_start,
                        end: slice.editor_end,
                        text: cue.text.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        cues.sort_by(|a, b| a.start.total_cmp(&b.start));
        Transcript {
            words,
            cues,
            language: self.language.clone(),
        }
    }

    /// SubRip text for the edited timeline.
    pub fn to_srt(&self, clips: &RecordingClipTimeline) -> String {
        let edited = self.in_editor_time(clips);
        let stamp = |time: f64| {
            let millis = (time.max(0.0) * 1000.0).round() as u64;
            format!(
                "{:02}:{:02}:{:02},{:03}",
                millis / 3_600_000,
                millis / 60_000 % 60,
                millis / 1000 % 60,
                millis % 1000
            )
        };
        let mut output = String::new();
        for (index, cue) in edited.cues.iter().enumerate() {
            output.push_str(&format!(
                "{}\n{} --> {}\n{}\n\n",
                index + 1,
                stamp(cue.start),
                stamp(cue.end),
                cue.text
            ));
        }
        output
    }
}

/// The cue covering a time, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubtitleTimeline {
    cues: Vec<SubtitleCue>,
}

impl SubtitleTimeline {
    pub fn new(mut cues: Vec<SubtitleCue>) -> Self {
        cues.retain(|cue| cue.start.is_finite() && cue.end > cue.start && !cue.text.trim().is_empty());
        cues.sort_by(|a, b| a.start.total_cmp(&b.start));
        Self { cues }
    }

    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    pub fn cue_at(&self, time: f64) -> Option<&SubtitleCue> {
        if !time.is_finite() {
            return None;
        }
        let index = self.cues.partition_point(|cue| cue.start <= time).checked_sub(1)?;
        let cue = &self.cues[index];
        (time < cue.end).then_some(cue)
    }
}

/// Word-level state of the subtitle bar at a moment.
#[derive(Clone, Debug, PartialEq)]
pub struct KaraokeLine {
    pub words: Vec<String>,
    /// The word being spoken; none between words and after the last one.
    pub active: Option<usize>,
    /// Words that have started; they stay fully lit.
    pub spoken: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct KaraokeCue {
    start: f64,
    end: f64,
    words: Vec<String>,
    timings: Vec<(f64, f64)>,
}

/// Groups timed words under the cue they fall in, but displays the cue's
/// (possibly hand-edited) text with timings mapped across by index, so text
/// edits keep working when word counts drift.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KaraokeTimeline {
    cues: Vec<KaraokeCue>,
}

impl KaraokeTimeline {
    pub fn new(cues: &[SubtitleCue], words: &[TranscriptWord]) -> Self {
        let mut sorted: Vec<&SubtitleCue> = cues
            .iter()
            .filter(|cue| cue.start.is_finite() && cue.end > cue.start)
            .collect();
        sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut timed: Vec<Vec<&TranscriptWord>> = vec![Vec::new(); sorted.len()];
        let mut words: Vec<&TranscriptWord> = words.iter().collect();
        words.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut index = 0;
        for word in words {
            while index + 1 < sorted.len() && word.start >= sorted[index + 1].start - 0.001 {
                index += 1;
            }
            if let Some(bucket) = timed.get_mut(index) {
                if word.start < sorted[index].end + 0.001 {
                    bucket.push(word);
                }
            }
        }
        let cues = sorted
            .into_iter()
            .zip(timed)
            .filter_map(|(cue, timed)| {
                let display: Vec<String> = cue.text.split_whitespace().map(str::to_string).collect();
                if timed.is_empty() || display.is_empty() {
                    return None;
                }
                let timings = (0..display.len())
                    .map(|index| {
                        let word = timed[(index * timed.len() / display.len()).min(timed.len() - 1)];
                        (word.start, word.end)
                    })
                    .collect();
                Some(KaraokeCue {
                    start: cue.start,
                    end: cue.end,
                    words: display,
                    timings,
                })
            })
            .collect();
        Self { cues }
    }

    pub fn line_at(&self, time: f64) -> Option<KaraokeLine> {
        if !time.is_finite() {
            return None;
        }
        let index = self.cues.partition_point(|cue| cue.start <= time).checked_sub(1)?;
        let cue = &self.cues[index];
        if time >= cue.end {
            return None;
        }
        let mut active = None;
        let mut spoken = 0;
        if let Some(last) = cue.timings.iter().rposition(|(start, _)| *start <= time) {
            spoken = last + 1;
            // Ride each word until the next starts; bridge the last word briefly.
            if last + 1 < cue.timings.len() || time <= cue.timings[last].1 + 0.25 {
                active = Some(last);
            }
        }
        Some(KaraokeLine {
            words: cue.words.clone(),
            active,
            spoken,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::clips::RecordingClipSegment;

    fn word(text: &str, start: f64, end: f64) -> TranscriptWord {
        TranscriptWord { text: text.into(), start, end }
    }

    #[test]
    fn cues_break_on_pauses_and_follow_cuts() {
        let transcript = Transcript::from_words(
            vec![
                word(" Hello", 0.0, 0.4),
                word(" world.", 0.5, 0.9),
                word(" Next", 3.0, 3.3),
                word(" part", 3.4, 3.8),
            ],
            None,
        );
        assert_eq!(transcript.cues.len(), 2);
        assert_eq!(transcript.cues[0].text, "Hello world.");
        let clips = RecordingClipTimeline::new(vec![RecordingClipSegment::new(2.0, 5.0)]);
        let edited = transcript.in_editor_time(&clips);
        assert_eq!(edited.cues.len(), 1);
        assert!((edited.cues[0].start - 1.0).abs() < 1e-9);
        assert_eq!(edited.words.len(), 2);
        assert!(transcript.to_srt(&clips).contains("00:00:01,000 --> "));
    }

    #[test]
    fn karaoke_highlights_the_spoken_word_of_edited_text() {
        let words = vec![word(" one", 0.0, 0.3), word(" two", 0.4, 0.7)];
        let cues = vec![SubtitleCue {
            id: Uuid::new_v4(),
            start: 0.0,
            end: 1.5,
            text: "One two".into(),
        }];
        let timeline = KaraokeTimeline::new(&cues, &words);
        assert_eq!(timeline.line_at(0.1).unwrap().active, Some(0));
        assert_eq!(timeline.line_at(0.5).unwrap().active, Some(1));
        let after = timeline.line_at(1.2).unwrap();
        assert_eq!((after.active, after.spoken), (None, 2));
    }
}
