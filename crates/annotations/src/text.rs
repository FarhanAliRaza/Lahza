//! Shared line breaking and font metrics for canvas text and SVG output.
use crate::AnnotationMark;
use resvg::usvg::fontdb::{Family, Query, Style, Weight};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub fn family(mark: &AnnotationMark) -> &'static str {
    match mark.font_family {
        1 => "DejaVu Sans Condensed",
        2 => "Ubuntu",
        3 => crate::fonts::HANDWRITTEN_FAMILY,
        _ => "Noto Sans",
    }
}
pub fn font(mark: &AnnotationMark) -> gpui::Font {
    let mut font = gpui::font(family(mark));
    if mark.bold {
        font.weight = gpui::FontWeight::BOLD;
    }
    if mark.italic {
        font = font.italic();
    }
    font
}
#[derive(Clone, Debug)]
pub struct TextLine {
    pub range: Range<usize>,
    pub width: f32,
}
#[derive(Clone, Debug)]
pub struct TextLayout {
    pub lines: Vec<TextLine>,
    pub width: f32,
    pub height: f32,
    pub line_height: f32,
    pub baseline: f32,
}
impl TextLayout {
    pub fn x(&self, line: &TextLine, alignment: u8) -> f32 {
        match alignment {
            1 => (self.width - line.width) * 0.5,
            2 => self.width - line.width,
            _ => 0.,
        }
    }
}
pub fn layout(mark: &AnnotationMark, box_width: f32) -> TextLayout {
    type Key = (String, u32, u8, bool, bool, bool, u32);
    thread_local! {static CACHE: std::cell::RefCell<std::collections::VecDeque<(Key,TextLayout)>>=const {std::cell::RefCell::new(std::collections::VecDeque::new())};}
    let key = (
        mark.text.clone(),
        mark.font_size.to_bits(),
        mark.font_family,
        mark.bold,
        mark.italic,
        mark.text_auto_width,
        if mark.text_auto_width {
            0
        } else {
            box_width.to_bits()
        },
    );
    if let Some(value) = CACHE.with(|c| {
        c.borrow()
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, v)| v.clone())
    }) {
        return value;
    }
    let result = measure_layout(mark, box_width);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 128 {
            c.pop_front();
        }
        c.push_back((key, result.clone()));
    });
    result
}
fn measure_layout(mark: &AnnotationMark, box_width: f32) -> TextLayout {
    let db = crate::fonts::shared_fontdb();
    let families = [
        Family::Name(family(mark)),
        Family::Name("DejaVu Sans"),
        Family::SansSerif,
    ];
    let id = db.query(&Query {
        families: &families,
        weight: Weight(if mark.bold { 700 } else { 400 }),
        style: if mark.italic {
            Style::Italic
        } else {
            Style::Normal
        },
        ..Default::default()
    });
    let size = mark.font_size.max(1.);
    let make = |data: &[u8], index: u32| {
        let face = rustybuzz::Face::from_slice(data, index);
        let measure = |text: &str| {
            if let Some(face) = &face {
                let mut b = rustybuzz::UnicodeBuffer::new();
                b.push_str(text);
                b.guess_segment_properties();
                let shaped = rustybuzz::shape(face, &[], b);
                shaped
                    .glyph_positions()
                    .iter()
                    .map(|p| p.x_advance as f32)
                    .sum::<f32>()
                    .abs()
                    * size
                    / face.units_per_em() as f32
            } else {
                text.graphemes(true).count() as f32 * size * 0.6
            }
        };
        let baseline = face
            .as_ref()
            .map(|f| f.ascender() as f32 * size / f.units_per_em() as f32)
            .unwrap_or(size);
        let descent = face
            .as_ref()
            .map(|f| -(f.descender() as f32) * size / f.units_per_em() as f32)
            .unwrap_or(size * 0.2);
        let line_height = (size * 1.35).max(baseline + descent);
        let baseline = baseline + (line_height - baseline - descent) * 0.5;
        let wrap = (!mark.text_auto_width).then_some(box_width.max(size));
        let lines = break_lines(&mark.text, wrap, measure);
        let width =
            wrap.unwrap_or_else(|| lines.iter().map(|l| l.width).fold(size * 0.5, f32::max));
        TextLayout {
            height: lines.len() as f32 * line_height,
            width,
            line_height,
            baseline,
            lines,
        }
    };
    id.and_then(|id| db.with_face_data(id, make))
        .unwrap_or_else(|| make(&[], 0))
}
fn break_lines(text: &str, wrap: Option<f32>, measure: impl Fn(&str) -> f32) -> Vec<TextLine> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for paragraph in text.split('\n') {
        if let Some(limit) = wrap {
            let boundaries: Vec<usize> = paragraph
                .grapheme_indices(true)
                .map(|(i, g)| i + g.len())
                .collect();
            let mut start = 0;
            while start < paragraph.len() {
                let mut end = start;
                let mut word_break = None;
                for &next in boundaries.iter().filter(|i| **i > start) {
                    if end > start && measure(&paragraph[start..next]) > limit {
                        break;
                    }
                    if paragraph[end..next].chars().all(char::is_whitespace) {
                        word_break = Some(next);
                    }
                    end = next;
                }
                if end < paragraph.len() {
                    end = word_break.unwrap_or(end);
                }
                lines.push(TextLine {
                    range: offset + start..offset + end,
                    width: measure(&paragraph[start..end]),
                });
                start = end;
            }
            if paragraph.is_empty() {
                lines.push(TextLine {
                    range: offset..offset,
                    width: 0.,
                });
            }
        } else {
            lines.push(TextLine {
                range: offset..offset + paragraph.len(),
                width: measure(paragraph),
            });
        }
        offset += paragraph.len() + 1;
    }
    lines
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrapping_keeps_unicode_offsets_and_empty_paragraphs() {
        let text = "hello world\n\n👨‍👩‍👧‍👦 abc";
        let lines = break_lines(text, Some(7.), |s| s.graphemes(true).count() as f32);
        assert_eq!(&text[lines[0].range.clone()], "hello ");
        assert!(lines.iter().any(|l| l.range.is_empty()));
        assert!(lines.iter().all(|l| text.get(l.range.clone()).is_some()));
    }
    #[test]
    fn metrics_measure_glyphs_not_character_counts() {
        let mut m = AnnotationMark::default();
        m.text = "iiii".into();
        let narrow = layout(&m, 500.).width;
        m.text = "WWWW".into();
        assert!(layout(&m, 500.).width > narrow * 1.5);
        m.text = "hello\nworld".into();
        let l = layout(&m, 500.);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.height, l.line_height * 2.);
    }
}
