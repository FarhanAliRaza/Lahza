//! Named marker tracks (design section 4.2): words, beats, hits, clicks…

use crate::timeref::Selector;
use crate::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// Time in seconds on the timeline.
    pub t: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Strength (onsets, hits) or other numeric value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// Box in pixels [x, y, w, h] (OCR, clicks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MarkerTrack {
    pub name: String,
    /// Where the track came from ("music analysis of assets/track.wav").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub markers: Vec<Marker>,
}

impl MarkerTrack {
    pub fn from_times(name: &str, times: impl IntoIterator<Item = f64>) -> Self {
        Self {
            name: name.into(),
            source: None,
            markers: times.into_iter().map(|t| Marker { t, ..Default::default() }).collect(),
        }
    }
    pub fn sort(&mut self) {
        self.markers.sort_by(|a, b| a.t.total_cmp(&b.t));
    }
    /// Median spacing between markers.
    pub fn median_interval(&self) -> Option<f64> {
        let mut gaps: Vec<f64> = self.markers.windows(2).map(|w| w[1].t - w[0].t).filter(|g| *g > 0.0).collect();
        if gaps.is_empty() {
            return None;
        }
        gaps.sort_by(f64::total_cmp);
        Some(gaps[gaps.len() / 2])
    }
}

/// Lower-case letters, digits and apostrophes: "Borders," matches "borders".
pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'' || c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Markers {
    pub tracks: BTreeMap<String, MarkerTrack>,
}

impl Markers {
    pub fn insert(&mut self, mut track: MarkerTrack) {
        track.sort();
        self.tracks.insert(track.name.clone(), track);
    }
    pub fn get(&self, name: &str) -> Option<&MarkerTrack> {
        self.tracks.get(name)
    }

    pub fn select(&self, track: &str, selector: &Selector) -> Result<&Marker> {
        let Some(t) = self.tracks.get(track) else {
            let known: Vec<_> = self.tracks.keys().cloned().collect();
            bail!(
                "no marker track '{track}' (have: {}); music tracks come from audio_analyze, words from a transcript",
                if known.is_empty() { "none".into() } else { known.join(", ") }
            );
        };
        match selector {
            Selector::Index(n) => t.markers.get(n - 1).ok_or_else(|| {
                format!("{track} has {} markers, asked for #{n}", t.markers.len()).into()
            }),
            Selector::Text(text, occurrence) => {
                let wanted = normalize(text);
                let words = wanted.split(' ').count();
                let labels: Vec<String> = t
                    .markers
                    .iter()
                    .map(|m| normalize(m.label.as_deref().unwrap_or("")))
                    .collect();
                let mut found = 0;
                for i in 0..t.markers.len() {
                    // A label may hold a whole phrase (OCR) or one word; a
                    // multi-word query also matches consecutive word markers.
                    let matched = labels[i] == wanted
                        || words > 1
                            && labels[i].contains(&wanted)
                        || words > 1
                            && i + words <= labels.len()
                            && labels[i..i + words].join(" ") == wanted;
                    if matched {
                        found += 1;
                        if found == *occurrence {
                            return Ok(&t.markers[i]);
                        }
                    }
                }
                if found == 0 {
                    bail!("no '{text}' in {track}");
                }
                bail!("'{text}' occurs {found} times in {track}, asked for occurrence {occurrence}")
            }
        }
    }

    /// Markers of a track within [from, to).
    pub fn range(&self, track: &str, from: f64, to: f64) -> Vec<(usize, &Marker)> {
        self.tracks
            .get(track)
            .map(|t| {
                t.markers
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.t >= from && m.t < to)
                    .map(|(i, m)| (i + 1, m))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `word` and `sentence` tracks from timed words, such as a Lahza
    /// transcript (`crates/transcript`, whisper.cpp). Sentences end at
    /// terminal punctuation or a pause of 0.9 s or more.
    pub fn from_words(words: &[(String, f64, f64)], offset: f64) -> (MarkerTrack, MarkerTrack) {
        let mut word_track = MarkerTrack { name: "word".into(), source: Some("transcript".into()), markers: Vec::new() };
        let mut sentences = MarkerTrack { name: "sentence".into(), source: Some("transcript".into()), markers: Vec::new() };
        let mut current: Option<Marker> = None;
        let mut prev_end = f64::NEG_INFINITY;
        for (text, start, end) in words {
            let label = text.trim();
            if label.is_empty() || !start.is_finite() {
                continue;
            }
            let (start, end) = (start + offset, end.max(*start) + offset);
            word_track.markers.push(Marker { t: start, end: Some(end), label: Some(label.into()), ..Default::default() });
            if let Some(sentence) = &mut current {
                if start - prev_end >= 0.9 {
                    sentences.markers.push(current.take().unwrap());
                } else {
                    sentence.end = Some(end);
                    let joined = format!("{} {label}", sentence.label.as_deref().unwrap_or(""));
                    sentence.label = Some(joined);
                }
            }
            if current.is_none() {
                current = Some(Marker { t: start, end: Some(end), label: Some(label.into()), ..Default::default() });
            }
            if label.ends_with(['.', '!', '?']) {
                sentences.markers.extend(current.take());
            }
            prev_end = end;
        }
        sentences.markers.extend(current);
        (word_track, sentences)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_become_word_and_sentence_tracks() {
        let words: Vec<(String, f64, f64)> = [
            (" The", 0.0, 0.2),
            (" canal", 0.25, 0.6),
            (" closed.", 0.65, 1.0),
            (" Ships", 1.2, 1.5),
            (" waited", 1.55, 1.9),
            (" again", 3.0, 3.4),
        ]
        .iter()
        .map(|(w, s, e)| (w.to_string(), *s, *e))
        .collect();
        let (w, s) = Markers::from_words(&words, 10.0);
        assert_eq!(w.markers.len(), 6);
        assert_eq!(s.markers.len(), 3);
        assert_eq!(s.markers[0].label.as_deref(), Some("The canal closed."));
        assert_eq!(s.markers[1].end, Some(11.9));
        let mut m = Markers::default();
        m.insert(w);
        let canal = m.select("word", &Selector::Text("canal closed".into(), 1)).unwrap();
        assert_eq!(canal.t, 10.25);
    }
}
