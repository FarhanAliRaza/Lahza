//! Shared scene composition for screenshots and recordings.
//!
//! A scene is a canvas (background + effects) with one media surface (the
//! screenshot or a decoded video frame) placed on it. The same geometry and
//! the same CPU compositor are used for animated-screenshot export and for
//! recording export, and the GPUI preview derives its layout from the same
//! [`SceneGeometry`] so what the user sees is what the file contains.

use image::{Rgba, RgbaImage};
use std::path::PathBuf;

use super::{
    model::NormalizedPoint,
    overlays::pointer_press_effect_geometry,
    pointer_timeline::PointerFrame,
    viewport::{visible_rect, ViewportFrame},
};

/// The preview canvas height every scene dimension is expressed against.
/// Padding, border thickness, corner radius, and shadow spread scale linearly
/// with the actual canvas height so the export matches the preview.
pub const REFERENCE_CANVAS_HEIGHT: f64 = 600.0;

#[derive(Clone, Debug, PartialEq)]
pub enum SceneBackground {
    Solid(u32),
    Gradient {
        colors: [u32; 3],
        angle_degrees: f64,
    },
    Wallpaper(PathBuf),
}

impl Default for SceneBackground {
    fn default() -> Self {
        Self::Solid(0x111214)
    }
}

/// Everything that styles a scene except the media itself.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneStyle {
    pub background: SceneBackground,
    /// 0-100, matches the inspector slider.
    pub padding: u8,
    /// 0-100, matches the inspector slider.
    pub corners: u8,
    /// 0-100 shadow strength.
    pub shadow: u8,
    /// 0 soft, 1 long, 2 glow, 3 crisp.
    pub shadow_style: usize,
    pub border: bool,
    pub border_thickness: u8,
    pub border_color: u32,
    pub border_opacity: u8,
    /// Canvas aspect ratio (width / height). `None` follows the media.
    pub aspect: Option<f64>,
}

impl Default for SceneStyle {
    fn default() -> Self {
        Self {
            background: SceneBackground::default(),
            padding: 20,
            corners: 12,
            shadow: 40,
            shadow_style: 0,
            border: false,
            border_thickness: 20,
            border_color: 0x3678ef,
            border_opacity: 100,
            aspect: None,
        }
    }
}

impl SceneStyle {
    /// Canvas size for an export at the requested height. Width follows the
    /// scene aspect ratio (or the media's own ratio) and both are even so
    /// every video encoder accepts them.
    pub fn export_canvas_size(
        &self,
        source_width: u32,
        source_height: u32,
        canvas_height: u32,
    ) -> (u32, u32) {
        let ratio = self
            .aspect
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
            .unwrap_or_else(|| {
                if source_height == 0 {
                    16.0 / 9.0
                } else {
                    source_width as f64 / source_height as f64
                }
            });
        let height = (canvas_height.max(2) / 2) * 2;
        let width = ((height as f64 * ratio).round() as u32).max(2);
        (width / 2 * 2, height)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    fn inset(&self, amount: f64) -> Rect {
        Rect {
            x: self.x + amount,
            y: self.y + amount,
            width: (self.width - amount * 2.0).max(0.0),
            height: (self.height - amount * 2.0).max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSpec {
    pub blur_radius: f64,
    pub offset_y: f64,
    pub opacity: f64,
}

/// Resolved layout of one scene at one canvas size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneGeometry {
    pub canvas_width: f64,
    pub canvas_height: f64,
    /// Rect the media occupies (inside the border).
    pub media: Rect,
    /// Corner radius of the media surface.
    pub radius: f64,
    /// Border thickness in canvas pixels (0 when the border is off).
    pub border_width: f64,
    pub shadow: Option<ShadowSpec>,
    /// Multiplier applied to every reference-size dimension.
    pub ui_scale: f64,
}

impl SceneGeometry {
    pub fn layout(
        canvas_width: f64,
        canvas_height: f64,
        source_width: f64,
        source_height: f64,
        style: &SceneStyle,
    ) -> Self {
        let ui_scale = (canvas_height / REFERENCE_CANVAS_HEIGHT).max(0.05);
        let border_width = if style.border {
            style.border_thickness as f64 * 0.48 * ui_scale
        } else {
            0.0
        };
        // Zero padding means the media reaches the canvas edge; only the
        // border adds space beyond the user's padding setting.
        let inset = style.padding as f64 * 2.0 * ui_scale + border_width;
        let available_width = (canvas_width - inset * 2.0).max(1.0);
        let available_height = (canvas_height - inset * 2.0).max(1.0);
        let source_width = if source_width > 0.0 {
            source_width
        } else {
            1200.0
        };
        let source_height = if source_height > 0.0 {
            source_height
        } else {
            720.0
        };
        let scale = (available_width / source_width).min(available_height / source_height);
        let width = source_width * scale;
        let height = source_height * scale;
        let media = Rect {
            x: inset + (available_width - width) * 0.5,
            y: inset + (available_height - height) * 0.5,
            width,
            height,
        };
        let strength = style.shadow as f64 / 100.0;
        let (radius_scale, offset_scale, opacity_scale) = match style.shadow_style {
            0 => (1.0, 0.3, 1.0),
            1 => (1.2, 0.9, 0.85),
            2 => (1.6, 0.0, 0.7),
            _ => (0.8, 0.2, 1.1),
        };
        let shadow_radius = 85.0 * strength * radius_scale * ui_scale;
        let shadow = (style.shadow > 0).then(|| ShadowSpec {
            blur_radius: shadow_radius,
            offset_y: shadow_radius * offset_scale,
            opacity: ((0.08 + strength * 1.35).min(0.35) * opacity_scale).min(0.5),
        });
        Self {
            canvas_width,
            canvas_height,
            media,
            radius: style.corners as f64 * 0.64 * ui_scale,
            border_width,
            shadow,
            ui_scale,
        }
    }

    /// The card rect: media plus border.
    pub fn card(&self) -> Rect {
        self.media.inset(-self.border_width)
    }

    pub fn card_radius(&self) -> f64 {
        self.radius + self.border_width
    }
}

/// Pointer state drawn on top of the media surface.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerOverlay {
    pub frame: PointerFrame,
}

/// CPU compositor with the static layers (background, shadow, border, mask)
/// precomputed once so per-frame work is only the media sample and overlays.
pub struct SceneCompositor {
    geometry: SceneGeometry,
    width: u32,
    height: u32,
    background: RgbaImage,
    media_x0: u32,
    media_y0: u32,
    media_w: u32,
    media_h: u32,
    media_mask: Vec<f32>,
}

impl SceneCompositor {
    pub fn new(
        style: &SceneStyle,
        canvas_width: u32,
        canvas_height: u32,
        source_width: u32,
        source_height: u32,
    ) -> Result<Self, String> {
        if canvas_width == 0 || canvas_height == 0 {
            return Err("scene canvas must be at least one pixel".into());
        }
        let geometry = SceneGeometry::layout(
            canvas_width as f64,
            canvas_height as f64,
            source_width as f64,
            source_height as f64,
            style,
        );
        let mut background = render_background(&style.background, canvas_width, canvas_height)?;
        let card = geometry.card();
        let card_radius = geometry.card_radius();
        if let Some(shadow) = geometry.shadow {
            paint_shadow(&mut background, card, card_radius, shadow);
        }
        if geometry.border_width > 0.0 && style.border_opacity > 0 {
            let tint = unpack(style.border_color);
            let alpha = style.border_opacity as f64 / 100.0;
            let x0 = card.x.floor().max(0.0) as u32;
            let y0 = card.y.floor().max(0.0) as u32;
            let x1 = (card.right().ceil() as u32).min(canvas_width);
            let y1 = (card.bottom().ceil() as u32).min(canvas_height);
            for y in y0..y1 {
                for x in x0..x1 {
                    let coverage =
                        rounded_rect_coverage(x as f64 + 0.5, y as f64 + 0.5, card, card_radius);
                    if coverage > 0.0 {
                        blend_pixel(&mut background, x, y, tint, coverage * alpha);
                    }
                }
            }
        }
        let media = geometry.media;
        let media_x0 = media.x.floor().max(0.0) as u32;
        let media_y0 = media.y.floor().max(0.0) as u32;
        let media_x1 = (media.right().ceil().max(0.0) as u32).min(canvas_width);
        let media_y1 = (media.bottom().ceil().max(0.0) as u32).min(canvas_height);
        let media_w = media_x1.saturating_sub(media_x0);
        let media_h = media_y1.saturating_sub(media_y0);
        let mut media_mask = vec![0.0; (media_w * media_h) as usize];
        for y in 0..media_h {
            for x in 0..media_w {
                media_mask[(y * media_w + x) as usize] = rounded_rect_coverage(
                    (media_x0 + x) as f64 + 0.5,
                    (media_y0 + y) as f64 + 0.5,
                    media,
                    geometry.radius,
                ) as f32;
            }
        }
        Ok(Self {
            geometry,
            width: canvas_width,
            height: canvas_height,
            background,
            media_x0,
            media_y0,
            media_w,
            media_h,
            media_mask,
        })
    }

    pub fn geometry(&self) -> SceneGeometry {
        self.geometry
    }

    pub fn canvas_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Composes one output frame. `source` is the full media (screenshot or
    /// decoded video frame); `viewport` selects the visible part of it.
    pub fn compose(
        &self,
        source: &RgbaImage,
        viewport: ViewportFrame,
        pointer: Option<&PointerOverlay>,
    ) -> RgbaImage {
        let mut output = self.background.clone();
        let (left, top, visible) = visible_rect(viewport);
        let media = self.geometry.media;
        let source_width = source.width() as f64;
        let source_height = source.height() as f64;
        if source_width > 0.0 && source_height > 0.0 && media.width > 0.0 && media.height > 0.0 {
            for y in 0..self.media_h {
                let canvas_y = self.media_y0 + y;
                let v = top + ((canvas_y as f64 + 0.5 - media.y) / media.height) * visible;
                for x in 0..self.media_w {
                    let coverage = self.media_mask[(y * self.media_w + x) as usize] as f64;
                    if coverage <= 0.0 {
                        continue;
                    }
                    let canvas_x = self.media_x0 + x;
                    let u = left + ((canvas_x as f64 + 0.5 - media.x) / media.width) * visible;
                    let sample =
                        sample_bilinear(source, u * source_width - 0.5, v * source_height - 0.5);
                    blend_pixel(
                        &mut output,
                        canvas_x,
                        canvas_y,
                        [sample[0], sample[1], sample[2]],
                        coverage * sample[3] as f64 / 255.0,
                    );
                }
            }
        }
        if let Some(pointer) = pointer {
            self.paint_pointer(
                &mut output,
                pointer,
                left,
                top,
                visible,
                viewport.magnification,
            );
        }
        output
    }

    fn paint_pointer(
        &self,
        output: &mut RgbaImage,
        pointer: &PointerOverlay,
        left: f64,
        top: f64,
        visible: f64,
        magnification: f64,
    ) {
        let frame = &pointer.frame;
        let media = self.geometry.media;
        let to_canvas = |point: NormalizedPoint| {
            (
                media.x + (point.x - left) / visible * media.width,
                media.y + (point.y - top) / visible * media.height,
            )
        };
        let clip = Rect {
            x: self.media_x0 as f64,
            y: self.media_y0 as f64,
            width: self.media_w as f64,
            height: self.media_h as f64,
        };
        if let Some(press) = frame.press.as_ref() {
            let (x, y) = to_canvas(press.location);
            let geometry =
                pointer_press_effect_geometry(press.progress, media.height, magnification);
            let color = [0, 122, 255];
            paint_disc(
                output,
                clip,
                x,
                y,
                geometry.impact_radius,
                color,
                geometry.impact_opacity,
            );
            paint_ring(
                output,
                clip,
                x,
                y,
                geometry.ripple_radius,
                geometry.ripple_line_width,
                color,
                geometry.ripple_opacity,
            );
        }
        if frame.opacity <= 0.0 {
            return;
        }
        let (x, y) = to_canvas(frame.location);
        // Preview draws the cursor icon at 25px on a reference-height canvas.
        let size = (25.0 * frame.magnification).clamp(15.0, 34.0) * self.geometry.ui_scale;
        paint_cursor(output, clip, x, y, size, frame.tilt_degrees, frame.opacity);
    }
}

fn unpack(color: u32) -> [u8; 3] {
    [
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    ]
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn blend_pixel(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 3], alpha: f64) {
    if x >= image.width() || y >= image.height() {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let pixel = image.get_pixel_mut(x, y);
    for channel in 0..3 {
        pixel[channel] = (pixel[channel] as f64
            + (color[channel] as f64 - pixel[channel] as f64) * alpha)
            .round() as u8;
    }
    pixel[3] = (pixel[3] as f64 + (255.0 - pixel[3] as f64) * alpha).round() as u8;
}

fn sample_bilinear(image: &RgbaImage, x: f64, y: f64) -> [u8; 4] {
    let max_x = image.width() as f64 - 1.0;
    let max_y = image.height() as f64 - 1.0;
    let x = x.clamp(0.0, max_x);
    let y = y.clamp(0.0, max_y);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;
    let p00 = image.get_pixel(x0, y0).0;
    let p10 = image.get_pixel(x1, y0).0;
    let p01 = image.get_pixel(x0, y1).0;
    let p11 = image.get_pixel(x1, y1).0;
    let mut out = [0u8; 4];
    for channel in 0..4 {
        let top = lerp(p00[channel] as f64, p10[channel] as f64, fx);
        let bottom = lerp(p01[channel] as f64, p11[channel] as f64, fx);
        out[channel] = lerp(top, bottom, fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Anti-aliased coverage of a rounded rectangle at a pixel center.
fn rounded_rect_coverage(px: f64, py: f64, rect: Rect, radius: f64) -> f64 {
    let radius = radius.clamp(0.0, rect.width.min(rect.height) * 0.5);
    let cx = rect.x + rect.width * 0.5;
    let cy = rect.y + rect.height * 0.5;
    let half_w = rect.width * 0.5 - radius;
    let half_h = rect.height * 0.5 - radius;
    let dx = (px - cx).abs() - half_w;
    let dy = (py - cy).abs() - half_h;
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.0);
    let distance = outside + inside - radius;
    (0.5 - distance).clamp(0.0, 1.0)
}

fn render_background(
    background: &SceneBackground,
    width: u32,
    height: u32,
) -> Result<RgbaImage, String> {
    match background {
        SceneBackground::Solid(color) => {
            let [r, g, b] = unpack(*color);
            Ok(RgbaImage::from_pixel(width, height, Rgba([r, g, b, 255])))
        }
        SceneBackground::Gradient {
            colors,
            angle_degrees,
        } => {
            let mut image = RgbaImage::new(width, height);
            let first = unpack(colors[0]);
            let middle = unpack(colors[1]);
            let last = unpack(colors[2]);
            // CSS angles: 0deg points up, 90deg points right.
            let radians = angle_degrees.to_radians();
            let dir = (radians.sin(), -radians.cos());
            let w = width as f64;
            let h = height as f64;
            let half_length = (w * dir.0.abs() + h * dir.1.abs()) * 0.5;
            for y in 0..height {
                for x in 0..width {
                    let px = x as f64 + 0.5 - w * 0.5;
                    let py = y as f64 + 0.5 - h * 0.5;
                    let t = if half_length > 0.0 {
                        ((px * dir.0 + py * dir.1) / half_length * 0.5 + 0.5).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let mut pixel = [0u8; 3];
                    for channel in 0..3 {
                        pixel[channel] =
                            lerp(first[channel] as f64, middle[channel] as f64, t).round() as u8;
                    }
                    // Second layer: transparent middle colour fading into
                    // the final stop from 35% onward, matching the preview.
                    let overlay = ((t - 0.35) / 0.65).clamp(0.0, 1.0);
                    for channel in 0..3 {
                        pixel[channel] = lerp(pixel[channel] as f64, last[channel] as f64, overlay)
                            .round() as u8;
                    }
                    image.put_pixel(x, y, Rgba([pixel[0], pixel[1], pixel[2], 255]));
                }
            }
            Ok(image)
        }
        SceneBackground::Wallpaper(path) => {
            let wallpaper = image::open(path)
                .map_err(|error| format!("could not open wallpaper {}: {error}", path.display()))?
                .to_rgba8();
            let mut image = RgbaImage::new(width, height);
            let ww = wallpaper.width() as f64;
            let wh = wallpaper.height() as f64;
            if ww <= 0.0 || wh <= 0.0 {
                return Err("wallpaper image is empty".into());
            }
            let scale = (width as f64 / ww).max(height as f64 / wh);
            let scaled_w = ww * scale;
            let scaled_h = wh * scale;
            let offset_x = (width as f64 - scaled_w) * 0.5;
            let offset_y = (height as f64 - scaled_h) * 0.5;
            for y in 0..height {
                for x in 0..width {
                    let sx = (x as f64 + 0.5 - offset_x) / scale - 0.5;
                    let sy = (y as f64 + 0.5 - offset_y) / scale - 0.5;
                    let sample = sample_bilinear(&wallpaper, sx, sy);
                    image.put_pixel(x, y, Rgba([sample[0], sample[1], sample[2], 255]));
                }
            }
            Ok(image)
        }
    }
}

fn paint_shadow(image: &mut RgbaImage, card: Rect, radius: f64, shadow: ShadowSpec) {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let mut mask = vec![0.0f32; width * height];
    let shifted = Rect {
        y: card.y + shadow.offset_y,
        ..card
    };
    let x0 = shifted.x.floor().max(0.0) as usize;
    let y0 = shifted.y.floor().max(0.0) as usize;
    let x1 = (shifted.right().ceil().max(0.0) as usize).min(width);
    let y1 = (shifted.bottom().ceil().max(0.0) as usize).min(height);
    for y in y0..y1 {
        for x in x0..x1 {
            mask[y * width + x] =
                rounded_rect_coverage(x as f64 + 0.5, y as f64 + 0.5, shifted, radius) as f32;
        }
    }
    // A CSS blur radius corresponds to a Gaussian with sigma = radius / 2;
    // three box passes approximate that Gaussian closely.
    let sigma = shadow.blur_radius * 0.5;
    if sigma > 0.5 {
        let box_width = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
        let box_radius = (((box_width - 1.0) / 2.0).round() as usize).max(1);
        let mut scratch = vec![0.0f32; width * height];
        for _ in 0..3 {
            box_blur_horizontal(&mask, &mut scratch, width, height, box_radius);
            box_blur_vertical(&scratch, &mut mask, width, height, box_radius);
        }
    }
    for y in 0..height {
        for x in 0..width {
            let alpha = mask[y * width + x] as f64 * shadow.opacity;
            if alpha > 0.001 {
                blend_pixel(image, x as u32, y as u32, [0, 0, 0], alpha);
            }
        }
    }
}

fn box_blur_horizontal(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = (radius * 2 + 1) as f32;
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        let mut sum: f32 = row.iter().take(radius + 1).sum();
        for x in 0..width {
            dst[y * width + x] = sum / window;
            if x + radius + 1 < width {
                sum += row[x + radius + 1];
            }
            if x >= radius {
                sum -= row[x - radius];
            }
        }
    }
}

fn box_blur_vertical(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = (radius * 2 + 1) as f32;
    for x in 0..width {
        let mut sum: f32 = (0..(radius + 1).min(height))
            .map(|y| src[y * width + x])
            .sum();
        for y in 0..height {
            dst[y * width + x] = sum / window;
            if y + radius + 1 < height {
                sum += src[(y + radius + 1) * width + x];
            }
            if y >= radius {
                sum -= src[(y - radius) * width + x];
            }
        }
    }
}

fn paint_disc(
    image: &mut RgbaImage,
    clip: Rect,
    cx: f64,
    cy: f64,
    radius: f64,
    color: [u8; 3],
    opacity: f64,
) {
    if opacity <= 0.0 || radius <= 0.0 {
        return;
    }
    let x0 = (cx - radius - 1.0).floor().max(clip.x) as i64;
    let y0 = (cy - radius - 1.0).floor().max(clip.y) as i64;
    let x1 = (cx + radius + 1.0).ceil().min(clip.right()) as i64;
    let y1 = (cy + radius + 1.0).ceil().min(clip.bottom()) as i64;
    for y in y0..y1 {
        for x in x0..x1 {
            let distance = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            blend_pixel(image, x as u32, y as u32, color, coverage * opacity);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_ring(
    image: &mut RgbaImage,
    clip: Rect,
    cx: f64,
    cy: f64,
    radius: f64,
    line_width: f64,
    color: [u8; 3],
    opacity: f64,
) {
    if opacity <= 0.0 || radius <= 0.0 {
        return;
    }
    let outer = radius;
    let inner = (radius - line_width).max(0.0);
    let x0 = (cx - outer - 1.0).floor().max(clip.x) as i64;
    let y0 = (cy - outer - 1.0).floor().max(clip.y) as i64;
    let x1 = (cx + outer + 1.0).ceil().min(clip.right()) as i64;
    let y1 = (cy + outer + 1.0).ceil().min(clip.bottom()) as i64;
    for y in y0..y1 {
        for x in x0..x1 {
            let distance = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let coverage =
                (outer - distance + 0.5).clamp(0.0, 1.0) * (distance - inner + 0.5).clamp(0.0, 1.0);
            blend_pixel(image, x as u32, y as u32, color, coverage * opacity);
        }
    }
}

/// Arrow cursor matching `icons/mouse-pointer.svg` (24-unit art box). The
/// tip of the arrow lands exactly on the pointer location.
const CURSOR_OUTLINE: [(f64, f64); 8] = [
    (4.2, 2.7),
    (4.2, 17.8),
    (8.3, 13.7),
    (11.55, 20.8),
    (14.6, 19.4),
    (11.45, 12.5),
    (17.3, 12.5),
    (4.2, 2.7),
];

fn paint_cursor(
    image: &mut RgbaImage,
    clip: Rect,
    tip_x: f64,
    tip_y: f64,
    size: f64,
    tilt_degrees: f64,
    opacity: f64,
) {
    const SUPERSAMPLE: usize = 3;
    let scale = size / 24.0;
    let stroke = 1.7 * scale * 0.5;
    let (sin, cos) = tilt_degrees.to_radians().sin_cos();
    let points: Vec<(f64, f64)> = CURSOR_OUTLINE
        .iter()
        .map(|(x, y)| {
            let lx = (x - CURSOR_OUTLINE[0].0) * scale;
            let ly = (y - CURSOR_OUTLINE[0].1) * scale;
            (tip_x + lx * cos - ly * sin, tip_y + lx * sin + ly * cos)
        })
        .collect();
    let min_x = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - stroke - 1.0;
    let max_x = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + stroke + 1.0;
    let min_y = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min) - stroke - 1.0;
    let max_y = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) + stroke + 1.0;
    let x0 = min_x.floor().max(clip.x) as i64;
    let y0 = min_y.floor().max(clip.y) as i64;
    let x1 = max_x.ceil().min(clip.right()) as i64;
    let y1 = max_y.ceil().min(clip.bottom()) as i64;
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let mut fill = 0.0;
            let mut outline = 0.0;
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let px = x as f64 + (sx as f64 + 0.5) / SUPERSAMPLE as f64;
                    let py = y as f64 + (sy as f64 + 0.5) / SUPERSAMPLE as f64;
                    let distance = polygon_edge_distance(&points, px, py);
                    let inside = point_in_polygon(&points, px, py);
                    if distance <= stroke {
                        outline += 1.0;
                    } else if inside {
                        fill += 1.0;
                    }
                }
            }
            if fill > 0.0 {
                blend_pixel(
                    image,
                    x as u32,
                    y as u32,
                    [255, 255, 255],
                    fill / samples * opacity,
                );
            }
            if outline > 0.0 {
                blend_pixel(
                    image,
                    x as u32,
                    y as u32,
                    [17, 17, 17],
                    outline / samples * opacity,
                );
            }
        }
    }
}

fn point_in_polygon(points: &[(f64, f64)], px: f64, py: f64) -> bool {
    let mut inside = false;
    let count = points.len();
    let mut j = count - 1;
    for i in 0..count {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn polygon_edge_distance(points: &[(f64, f64)], px: f64, py: f64) -> f64 {
    let mut best = f64::INFINITY;
    for pair in points.windows(2) {
        let (ax, ay) = pair[0];
        let (bx, by) = pair[1];
        let dx = bx - ax;
        let dy = by - ay;
        let length_squared = dx * dx + dy * dy;
        let t = if length_squared > 0.0 {
            (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cx = ax + dx * t;
        let cy = ay + dy * t;
        best = best.min(((px - cx).powi(2) + (py - cy).powi(2)).sqrt());
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::pointer_timeline::PointerPressFrame;

    fn checker(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let left = x < width / 2;
                let top = y < height / 2;
                let color = match (left, top) {
                    (true, true) => [255, 0, 0, 255],
                    (false, true) => [0, 255, 0, 255],
                    (true, false) => [0, 0, 255, 255],
                    (false, false) => [255, 255, 0, 255],
                };
                image.put_pixel(x, y, Rgba(color));
            }
        }
        image
    }

    #[test]
    fn layout_scales_with_canvas_height() {
        let style = SceneStyle {
            padding: 10,
            border: true,
            border_thickness: 50,
            ..SceneStyle::default()
        };
        let small = SceneGeometry::layout(600.0, 600.0, 1600.0, 900.0, &style);
        let large = SceneGeometry::layout(1200.0, 1200.0, 1600.0, 900.0, &style);
        assert!((large.border_width - small.border_width * 2.0).abs() < 1e-9);
        assert!((large.media.x - small.media.x * 2.0).abs() < 1e-6);
        assert!((large.media.width - small.media.width * 2.0).abs() < 1e-6);
        assert!((large.radius - small.radius * 2.0).abs() < 1e-9);
        // The media never leaves the canvas.
        assert!(large.media.right() <= 1200.0 && large.media.bottom() <= 1200.0);
    }

    #[test]
    fn export_canvas_follows_aspect_and_stays_even() {
        let style = SceneStyle {
            aspect: Some(16.0 / 9.0),
            ..SceneStyle::default()
        };
        assert_eq!(style.export_canvas_size(1000, 1000, 1080), (1920, 1080));
        let auto = SceneStyle {
            aspect: None,
            ..SceneStyle::default()
        };
        assert_eq!(auto.export_canvas_size(1001, 1000, 719), (718, 718));
    }

    #[test]
    fn compositor_paints_background_outside_and_media_inside() {
        let style = SceneStyle {
            background: SceneBackground::Solid(0x102030),
            padding: 20,
            corners: 0,
            shadow: 0,
            border: false,
            aspect: Some(1.0),
            ..SceneStyle::default()
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 100, 100).unwrap();
        let output = compositor.compose(&checker(100, 100), ViewportFrame::default(), None);
        assert_eq!(output.get_pixel(2, 2).0, [0x10, 0x20, 0x30, 255]);
        let media = compositor.geometry().media;
        let inside_x = (media.x + 5.0) as u32;
        let inside_y = (media.y + 5.0) as u32;
        assert_eq!(output.get_pixel(inside_x, inside_y).0, [255, 0, 0, 255]);
        let far_x = (media.right() - 5.0) as u32;
        let far_y = (media.bottom() - 5.0) as u32;
        assert_eq!(output.get_pixel(far_x, far_y).0, [255, 255, 0, 255]);
    }

    #[test]
    fn viewport_zoom_crops_the_media() {
        let style = SceneStyle {
            background: SceneBackground::Solid(0x000000),
            padding: 0,
            corners: 0,
            shadow: 0,
            border: false,
            aspect: Some(1.0),
            ..SceneStyle::default()
        };
        let compositor = SceneCompositor::new(&style, 100, 100, 100, 100).unwrap();
        // Zoom 2x toward the top-left: the whole card shows the red quadrant.
        let viewport = ViewportFrame {
            magnification: 2.0,
            anchor: NormalizedPoint { x: 0.25, y: 0.25 },
        };
        let output = compositor.compose(&checker(100, 100), viewport, None);
        assert_eq!(output.get_pixel(5, 5).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(94, 94).0, [255, 0, 0, 255]);
    }

    #[test]
    fn rounded_corners_reveal_the_background() {
        let style = SceneStyle {
            background: SceneBackground::Solid(0xffffff),
            padding: 0,
            corners: 100,
            shadow: 0,
            border: false,
            aspect: Some(1.0),
            ..SceneStyle::default()
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let output = compositor.compose(&checker(200, 200), ViewportFrame::default(), None);
        assert_eq!(output.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(output.get_pixel(100, 5).0, [0, 255, 0, 255]);
    }

    #[test]
    fn gradient_and_shadow_and_border_render_without_panicking() {
        let style = SceneStyle {
            background: SceneBackground::Gradient {
                colors: [0xfa4f94, 0x6652f2, 0x4ad6cc],
                angle_degrees: 135.0,
            },
            padding: 30,
            corners: 40,
            shadow: 80,
            shadow_style: 1,
            border: true,
            border_thickness: 40,
            border_color: 0xffc928,
            border_opacity: 100,
            aspect: Some(16.0 / 9.0),
        };
        let compositor = SceneCompositor::new(&style, 320, 180, 64, 36).unwrap();
        let output = compositor.compose(&checker(64, 36), ViewportFrame::default(), None);
        let geometry = compositor.geometry();
        // Border ring sits just outside the media.
        let border_x = (geometry.media.x - geometry.border_width * 0.5) as u32;
        let border_y = (geometry.media.y + geometry.media.height * 0.5) as u32;
        assert_eq!(
            output.get_pixel(border_x, border_y).0,
            [0xff, 0xc9, 0x28, 255]
        );
        // Shadow darkens the area below the card compared to the far corner.
        let below = output
            .get_pixel(160, (geometry.card().bottom() + 3.0) as u32)
            .0;
        let corner = output.get_pixel(160, 0).0;
        let sum = |pixel: [u8; 4]| pixel[0] as u32 + pixel[1] as u32 + pixel[2] as u32;
        assert!(sum(below) < sum(corner));
    }

    #[test]
    fn pointer_and_click_ring_paint_inside_the_media() {
        let style = SceneStyle {
            background: SceneBackground::Solid(0x000000),
            padding: 10,
            corners: 0,
            shadow: 0,
            border: false,
            aspect: Some(1.0),
            ..SceneStyle::default()
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let white = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.5, y: 0.5 },
                artwork_id: None,
                magnification: 1.0,
                tilt_degrees: 0.0,
                opacity: 1.0,
                blur_radius: 0.0,
                press: Some(PointerPressFrame {
                    location: NormalizedPoint { x: 0.5, y: 0.5 },
                    progress: 0.5,
                }),
            },
        };
        let output = compositor.compose(&white, ViewportFrame::default(), Some(&pointer));
        let media = compositor.geometry().media;
        let center = (
            (media.x + media.width * 0.5) as u32,
            (media.y + media.height * 0.5) as u32,
        );
        // Something other than the flat white surface was painted near the pointer.
        let mut painted = false;
        for dy in 0..12 {
            for dx in 0..12 {
                if output.get_pixel(center.0 + dx, center.1 + dy).0 != [255, 255, 255, 255] {
                    painted = true;
                }
            }
        }
        assert!(painted);
    }
}
