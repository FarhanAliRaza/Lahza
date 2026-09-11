//! Annotation SVG fragments and raster layers shared by preview and export.
use crate::fonts::shared_fontdb;
use crate::{AnnotationMark, Tool};
use gpui::{point, px, size, Bounds};
use image::RgbaImage;
use std::fmt::Write as _;

/// SVG fragment with every visible annotation in `marks`, positioned
/// relative to a capture drawn at (`x`, `y`) with the given pixel size. The
/// fragment is self-contained; callers own any surrounding groups.
pub fn annotations_svg(
    marks: &[AnnotationMark],
    x: f32,
    y: f32,
    capture_width: u32,
    capture_height: u32,
    stroke_scale: f32,
) -> String {
    {
        let mut svg = String::new();
        let highlights: Vec<_> = marks
            .iter()
            .filter(|mark| mark.tool == Tool::Highlight)
            .collect();
        if !highlights.is_empty() {
            let _ = write!(svg, "<path fill=\"black\" fill-opacity=\"0.55\" fill-rule=\"evenodd\" d=\"M{x},{y}h{capture_width}v{capture_height}h-{capture_width}z");
            for mark in highlights {
                let hx = x + mark.start.x.min(mark.end.x) * capture_width as f32;
                let hy = y + mark.start.y.min(mark.end.y) * capture_height as f32;
                let hw = (mark.end.x - mark.start.x).abs() * capture_width as f32;
                let hh = (mark.end.y - mark.start.y).abs() * capture_height as f32;
                let _ = write!(svg, " M{hx},{hy}v{hh}h{hw}v-{hh}z");
            }
            svg.push_str("\"/>");
        }

        for mark in marks.iter().filter(|mark| {
            !matches!(
                mark.tool,
                Tool::Select | Tool::Blur | Tool::Pixelate | Tool::Highlight
            )
        }) {
            if mark.opacity < 0.999 {
                let _ = write!(svg, "<g opacity=\"{:.3}\">", mark.opacity.clamp(0.0, 1.0));
            }
            let sx = x + mark.start.x * capture_width as f32;
            let sy = y + mark.start.y * capture_height as f32;
            let ex = x + mark.end.x * capture_width as f32;
            let ey = y + mark.end.y * capture_height as f32;
            let left = sx.min(ex);
            let top = sy.min(ey);
            let width = (ex - sx).abs();
            let height = (ey - sy).abs();
            let color = mark.color;
            let stroke = (mark.stroke_width * stroke_scale).max(1.0);
            match mark.tool {
                tool if matches!(tool, Tool::Line | Tool::Arrow | Tool::Pen)
                    || (mark.hand_drawn && crate::geometry::supports_drawn(tool)) =>
                {
                    let mut scaled = mark.clone();
                    scaled.scale_stroke_width(stroke_scale);
                    scaled.stroke_width = stroke;
                    let geometry = crate::geometry::geometry(
                        &scaled,
                        Bounds::new(
                            point(px(x), px(y)),
                            size(px(capture_width as f32), px(capture_height as f32)),
                        ),
                    );
                    for path in geometry.paths {
                        let fill = if path.filled {
                            format!("#{color:06x}")
                        } else {
                            "none".into()
                        };
                        let outline = if path.filled {
                            "none".into()
                        } else {
                            format!("#{color:06x}")
                        };
                        let _ = write!(svg, "<path d=\"{}\" fill=\"{fill}\" stroke=\"{outline}\" stroke-width=\"{stroke}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>", path.svg());
                    }
                }
                Tool::Rectangle => {
                    let _ = write!(svg, "<rect x=\"{left}\" y=\"{top}\" width=\"{width}\" height=\"{height}\" rx=\"2\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\"/>");
                }
                Tool::FilledRectangle => {
                    let _ = write!(svg, "<rect x=\"{left}\" y=\"{top}\" width=\"{width}\" height=\"{height}\" rx=\"2\" fill=\"#{color:06x}\"/>");
                }
                Tool::Ellipse => {
                    let _ = write!(svg, "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\"/>", left + width/2.0, top + height/2.0, width/2.0, height/2.0);
                }
                Tool::Number => {
                    let cx = left + width / 2.0;
                    let cy = top + height / 2.0;
                    let r = width.min(height) / 2.0;
                    let _ = write!(svg, "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"#{color:06x}\"/><text x=\"{cx}\" y=\"{}\" text-anchor=\"middle\" font-family=\"sans-serif\" font-weight=\"700\" font-size=\"{}\" fill=\"white\">{}</text>", cy+r*0.36, r, mark.number);
                }
                Tool::Text if !mark.text.is_empty() => {
                    let weight = if mark.bold { "700" } else { "400" };
                    let style = if mark.italic { "italic" } else { "normal" };
                    let decoration = if mark.underline { "underline" } else { "none" };
                    // Fallbacks keep the export sans-serif on machines
                    // without the preferred face installed.
                    let family = match mark.font_family {
                        1 => {
                            "DejaVu Sans Condensed, DejaVu Sans, Liberation Sans Narrow, sans-serif"
                        }
                        2 => "Ubuntu, Cantarell, Noto Sans, DejaVu Sans, sans-serif",
                        3 => crate::fonts::HANDWRITTEN_FAMILY,
                        _ => {
                            "Noto Sans, Inter, DejaVu Sans, Liberation Sans, Cantarell, sans-serif"
                        }
                    };
                    let mut scaled = mark.clone();
                    scaled.font_size *= stroke_scale;
                    let layout = crate::text::layout(&scaled, width);
                    for (row, line) in layout.lines.iter().enumerate() {
                        let text_x = left + layout.x(line, mark.text_alignment);
                        let text_y = top + row as f32 * layout.line_height + layout.baseline;
                        let value = xml_escape(&mark.text[line.range.clone()]);
                        let _ = write!(svg, "<text x=\"{text_x}\" y=\"{text_y}\" xml:space=\"preserve\" font-family=\"{family}\" font-weight=\"{weight}\" font-style=\"{style}\" text-decoration=\"{decoration}\" font-size=\"{}\" fill=\"#{color:06x}\">{value}</text>", scaled.font_size);
                    }
                }
                _ => {}
            }
            if mark.opacity < 0.999 {
                svg.push_str("</g>");
            }
        }
        svg
    }
}

/// Renders SVG markup to a straight-alpha RGBA layer.
pub fn render_svg_layer(svg: &str, width: u32, height: u32) -> Result<RgbaImage, String> {
    let mut options = resvg::usvg::Options::default();
    options.fontdb = shared_fontdb();
    let tree = resvg::usvg::Tree::from_str(svg, &options)
        .map_err(|error| format!("could not parse overlay: {error}"))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "overlay dimensions are too large".to_string())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let mut data = pixmap.take();
    // tiny-skia stores premultiplied alpha; the compositor expects straight.
    for pixel in data.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for channel in pixel.iter_mut().take(3) {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width, height, data)
        .ok_or_else(|| "overlay had an invalid byte count".to_string())
}

pub fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Renders annotation marks (no capture) to a transparent layer.
pub fn render_annotations(marks: &[AnnotationMark], width: u32, height: u32) -> Option<RgbaImage> {
    let stroke_scale = width.min(height) as f32 / 800.0;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><g>"#
    );
    svg.push_str(&annotations_svg(
        marks,
        0.0,
        0.0,
        width,
        height,
        stroke_scale,
    ));
    svg.push_str("</g></svg>");
    render_svg_layer(&svg, width, height).ok()
}
