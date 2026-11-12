//! Time references (design section 4.3). Any time field takes a number of
//! seconds or a reference string:
//!
//! ```text
//! 2.5                      seconds on the timeline
//! "48f"                    frame 48
//! "word:borders"           first occurrence of a word (case-insensitive)
//! "word:borders@2"         second occurrence
//! "word#42"                42nd word (indices are 1-based everywhere)
//! "sentence#3.end"         end of the third sentence
//! "beat:17", "bar:5", "triplet:12", "hit:3", "stop:2", "click:4"
//! "ocr:'Deploy'"           quoted text match
//! "#title.end + 0.25"      relative to another layer or scene
//! "bar:5 - 2f", "beat:9 + 0.5b"
//! ```

use crate::markers::Markers;
use crate::time::{Duration, TimeBase};
use crate::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Which moment of a move lands on its time reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    Start,
    /// Maximum speed.
    Peak,
    /// First reaches 98% (contact for springs and bounces).
    Land,
    #[default]
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TimeRef {
    Seconds(f64),
    Expr(String),
}

impl Default for TimeRef {
    fn default() -> Self {
        TimeRef::Seconds(0.0)
    }
}

impl From<f64> for TimeRef {
    fn from(value: f64) -> Self {
        TimeRef::Seconds(value)
    }
}

impl From<&str> for TimeRef {
    fn from(value: &str) -> Self {
        TimeRef::Expr(value.into())
    }
}

impl Serialize for TimeRef {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            TimeRef::Seconds(v) => s.serialize_f64(*v),
            TimeRef::Expr(e) => s.serialize_str(e),
        }
    }
}

impl<'de> Deserialize<'de> for TimeRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Number(n) => Ok(TimeRef::Seconds(n.as_f64().unwrap_or(0.0))),
            serde_json::Value::String(s) => {
                parse(&s).map_err(serde::de::Error::custom)?;
                Ok(TimeRef::Expr(s))
            }
            other => Err(serde::de::Error::custom(format!("expected a time or reference, got {other}"))),
        }
    }
}

/// How a marker is selected inside its track.
#[derive(Clone, Debug, PartialEq)]
pub enum Selector {
    /// 1-based index.
    Index(usize),
    /// Text match (case-insensitive, punctuation ignored), with 1-based
    /// occurrence.
    Text(String, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Base {
    Seconds(f64),
    Duration(Duration),
    Marker { track: String, selector: Selector, edge: Edge },
    Element { id: String, edge: Edge },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub base: Base,
    pub offsets: Vec<Duration>,
}

impl Parsed {
    /// The marker track this reference depends on, if any.
    pub fn track(&self) -> Option<&str> {
        match &self.base {
            Base::Marker { track, .. } => Some(track),
            _ => None,
        }
    }
}

pub fn parse(text: &str) -> Result<Parsed> {
    let text = text.trim();
    if text.is_empty() {
        bail!("empty time reference");
    }
    // Offsets follow whitespace: "beat:3 + 2f", "#title.end - 0.25". Quoted
    // marker text (after ":'") is protected, so it may contain spaces.
    let protected = match text.find(":'") {
        Some(open) => {
            let close = text[open + 2..].find('\'').map(|c| open + 2 + c).unwrap_or(text.len());
            open..close + 1
        }
        None => 0..0,
    };
    let mut parts = Vec::new();
    let mut signs = Vec::new();
    let mut last = 0;
    let bytes = text.as_bytes();
    for i in 1..bytes.len() {
        let c = bytes[i];
        if (c == b'+' || c == b'-') && bytes[i - 1].is_ascii_whitespace() && !protected.contains(&i) {
            parts.push(text[last..i].to_string());
            signs.push(if c == b'+' { 1.0 } else { -1.0 });
            last = i + 1;
        }
    }
    parts.push(text[last..].to_string());
    let base = parse_base(parts[0].trim())?;
    let mut offsets = Vec::new();
    for (sign, part) in signs.iter().zip(&parts[1..]) {
        let d = Duration::parse(part.trim())
            .map_err(|e| format!("in '{text}': {e}"))?;
        offsets.push(match d {
            Duration::Seconds(v) => Duration::Seconds(v * sign),
            Duration::Frames(v) => Duration::Frames(v * sign),
            Duration::Beats(v) => Duration::Beats(v * sign),
            Duration::Bars(v) => Duration::Bars(v * sign),
        });
    }
    Ok(Parsed { base, offsets })
}

fn split_edge(text: &str) -> (&str, Edge) {
    if let Some(rest) = text.strip_suffix(".end") {
        (rest, Edge::End)
    } else if let Some(rest) = text.strip_suffix(".start") {
        (rest, Edge::Start)
    } else {
        (text, Edge::Start)
    }
}

fn parse_base(text: &str) -> Result<Base> {
    if let Ok(v) = text.parse::<f64>() {
        return Ok(Base::Seconds(v));
    }
    if text.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-') {
        return Duration::parse(text)
            .map(Base::Duration)
            .map_err(|e| format!("bad time '{text}': {e}").into());
    }
    if let Some(rest) = text.strip_prefix('#') {
        let (id, edge) = split_edge(rest);
        if id.is_empty() {
            bail!("'{text}': expected #<id>, #<id>.start or #<id>.end");
        }
        return Ok(Base::Element { id: id.to_string(), edge });
    }
    // Marker: track then ':' selector or '#' index.
    let sep = text
        .find([':', '#'])
        .ok_or_else(|| format!("bad time reference '{text}': expected seconds, \"48f\", \"beat:17\", \"word:hello\", \"word#3\" or \"#layer.end\""))?;
    let track = text[..sep].trim();
    if track.is_empty() || !track.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
        bail!("bad marker track name in '{text}'");
    }
    let rest = &text[sep + 1..];
    let (selector_text, edge) = if rest.starts_with('\'') {
        let close = rest[1..]
            .find('\'')
            .ok_or_else(|| format!("unclosed quote in '{text}'"))?
            + 1;
        let tail = &rest[close + 1..];
        let edge = match tail {
            "" => Edge::Start,
            ".end" => Edge::End,
            ".start" => Edge::Start,
            other if other.starts_with('@') => {
                let (n, edge) = split_edge(&other[1..]);
                let occurrence = n.parse::<usize>().map_err(|_| format!("bad occurrence in '{text}'"))?;
                return Ok(Base::Marker {
                    track: track.into(),
                    selector: Selector::Text(rest[1..close].to_string(), occurrence.max(1)),
                    edge,
                });
            }
            _ => bail!("unexpected '{tail}' after quoted text in '{text}'"),
        };
        return Ok(Base::Marker {
            track: track.into(),
            selector: Selector::Text(rest[1..close].to_string(), 1),
            edge,
        });
    } else {
        split_edge(rest)
    };
    let selector = if let Ok(n) = selector_text.parse::<usize>() {
        if n == 0 {
            bail!("'{text}': marker indices start at 1");
        }
        Selector::Index(n)
    } else if text.as_bytes()[sep] == b'#' {
        bail!("'{text}': '#' takes a 1-based index; use ':' for text");
    } else if let Some((word, n)) = selector_text.rsplit_once('@') {
        let occurrence = n.parse::<usize>().map_err(|_| format!("bad occurrence in '{text}'"))?;
        Selector::Text(word.to_string(), occurrence.max(1))
    } else {
        if selector_text.is_empty() {
            bail!("'{text}': empty marker selector");
        }
        Selector::Text(selector_text.to_string(), 1)
    };
    Ok(Base::Marker { track: track.into(), selector, edge })
}

/// Resolves element references (`#id.start`) during compilation.
pub trait ElementTimes {
    fn element_time(&mut self, id: &str, edge: Edge) -> Result<f64>;
}

pub struct NoElements;

impl ElementTimes for NoElements {
    fn element_time(&mut self, id: &str, _: Edge) -> Result<f64> {
        bail!("reference to #{id} is not available here")
    }
}

impl TimeRef {
    pub fn parsed(&self) -> Result<Parsed> {
        match self {
            TimeRef::Seconds(v) => Ok(Parsed { base: Base::Seconds(*v), offsets: Vec::new() }),
            TimeRef::Expr(e) => parse(e),
        }
    }

    /// The reference as written.
    pub fn text(&self) -> String {
        match self {
            TimeRef::Seconds(s) => format!("{s}"),
            TimeRef::Expr(e) => e.clone(),
        }
    }

    pub fn is_marker(&self) -> bool {
        self.parsed().ok().is_some_and(|p| matches!(p.base, Base::Marker { .. }))
    }

    pub fn resolve(
        &self,
        markers: &Markers,
        base: &TimeBase,
        elements: &mut dyn ElementTimes,
    ) -> Result<f64> {
        let parsed = self.parsed()?;
        let mut t = match &parsed.base {
            Base::Seconds(v) => *v,
            Base::Duration(d) => d.seconds(base)?,
            Base::Marker { track, selector, edge } => {
                let marker = markers.select(track, selector).map_err(|e| {
                    let text = match self {
                        TimeRef::Expr(e) => e.clone(),
                        TimeRef::Seconds(v) => v.to_string(),
                    };
                    format!("'{text}': {e}")
                })?;
                match edge {
                    Edge::Start => marker.t,
                    Edge::End => marker.end.unwrap_or(marker.t),
                }
            }
            Base::Element { id, edge } => elements.element_time(id, *edge)?,
        };
        for offset in &parsed.offsets {
            t += offset.seconds(base)?;
        }
        if !t.is_finite() {
            bail!("time reference resolved to a non-finite time");
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::{Marker, MarkerTrack};
    use crate::time::Fps;

    fn markers() -> Markers {
        let mut m = Markers::default();
        m.insert(MarkerTrack::from_times("beat", (0..32).map(|i| 0.5 + i as f64 * 0.5)));
        m.insert(MarkerTrack {
            name: "word".into(),
            source: None,
            markers: ["The", "borders,", "moved", "borders"]
                .iter()
                .enumerate()
                .map(|(i, w)| Marker { t: i as f64, end: Some(i as f64 + 0.6), label: Some(w.to_string()), ..Default::default() })
                .collect(),
        });
        m
    }

    fn at(text: &str) -> f64 {
        let base = TimeBase { fps: Fps::whole(25), beat: Some(0.5), bar: Some(2.0) };
        TimeRef::Expr(text.into()).resolve(&markers(), &base, &mut NoElements).unwrap()
    }

    #[test]
    fn references_resolve() {
        assert_eq!(at("beat:1"), 0.5);
        assert_eq!(at("beat:17"), 8.5);
        assert_eq!(at("word:borders"), 1.0);
        assert_eq!(at("word:Borders@2"), 3.0);
        assert_eq!(at("word#3.end"), 2.6);
        assert_eq!(at("word:'moved'.end"), 2.6);
        assert_eq!(at("beat:3 + 0.25"), 1.75);
        assert!((at("beat:3 - 2f") - 1.42).abs() < 1e-12);
        assert_eq!(at("beat:3 + 1b"), 2.0);
        assert_eq!(at("50f"), 2.0);
        assert_eq!(at("-1.5"), -1.5);
    }

    #[test]
    fn element_references_parse() {
        let p = parse("#title.end + 0.25").unwrap();
        assert_eq!(p.base, Base::Element { id: "title".into(), edge: Edge::End });
        assert_eq!(p.offsets, vec![Duration::Seconds(0.25)]);
        let p = parse("onset.low:3").unwrap();
        assert_eq!(p.track(), Some("onset.low"));
        let p = parse("ocr:'canal revenue'").unwrap();
        assert!(matches!(p.base, Base::Marker { selector: Selector::Text(ref s, 1), .. } if s == "canal revenue"));
    }

    #[test]
    fn bad_references_say_why() {
        assert!(parse("beat:0").is_err());
        assert!(parse("word#hello").is_err());
        assert!(parse("nonsense").is_err());
        let base = TimeBase::new(Fps::whole(25));
        let err = TimeRef::from("hit:3").resolve(&markers(), &base, &mut NoElements).unwrap_err();
        assert!(err.0.contains("hit"), "{err}");
        let err = TimeRef::from("beat:99").resolve(&markers(), &base, &mut NoElements).unwrap_err();
        assert!(err.0.contains("32"), "{err}");
    }
}
