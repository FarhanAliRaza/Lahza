#[path = "source_sampling.rs"]
mod sampling;

use image::{Rgba, RgbaImage};

#[path = "scene_gpu.rs"]
pub mod gpu;

pub use super::scene_model::*;

use super::{
    cursor_assets::{self, CursorFamily},
    model::NormalizedPoint,
    overlays::pointer_press_effect_geometry,
    pointer_timeline::PointerBitmap,
    viewport::ViewportFrame,
};
#[cfg(test)]
use super::pointer_timeline::PointerFrame;

/// Per-frame inputs to [`SceneCompositor::compose`].
#[derive(Clone, Copy)]
pub struct FrameInput<'a> {
    /// Full media (screenshot or decoded video frame).
    pub source: &'a RgbaImage,
    /// Optional straight-alpha layer in media space (same aspect as
    /// `source`), e.g. flattened annotations for the current time.
    pub overlay: Option<&'a RgbaImage>,
    pub viewport: ViewportFrame,
    pub pointer: Option<&'a PointerOverlay>,
    /// Camera (webcam) frame for this time, drawn as picture-in-picture.
    pub camera: Option<&'a RgbaImage>,
}

#[derive(PartialEq)]
struct MediaShadow {
    alpha: image::GrayImage,
    viewport: (f64, f64, f64, f64),
    sample_bounds: (f64, f64, f64, f64),
}

impl MediaShadow {
    fn from_source(source: &RgbaImage, viewport: (f64, f64, f64, f64), crop: crate::viewport::CropRect) -> Option<Self> {
        if source.pixels().all(|pixel| pixel[3] == 255) {
            return None;
        }
        Some(Self {
            alpha: image::GrayImage::from_raw(
                source.width(),
                source.height(),
                source.pixels().map(|pixel| pixel[3]).collect(),
            )
            .expect("one alpha value per source pixel"),
            viewport,
            sample_bounds: crop.sample_bounds(source.width(), source.height()),
        })
    }

    fn sample(&self, u: f64, v: f64) -> f64 {
        let (left, top, visible_x, visible_y) = self.viewport;
        let x = tap(
            (left + u.clamp(0.0, 1.0) * visible_x).clamp(self.sample_bounds.0, self.sample_bounds.2) * f64::from(self.alpha.width()) - 0.5,
            self.alpha.width(),
        );
        let y = tap(
            (top + v.clamp(0.0, 1.0) * visible_y).clamp(self.sample_bounds.1, self.sample_bounds.3) * f64::from(self.alpha.height()) - 0.5,
            self.alpha.height(),
        );
        let x1 = (x.index + 1).min(self.alpha.width() as usize - 1);
        let y1 = (y.index + 1).min(self.alpha.height() as usize - 1);
        let pixels = self.alpha.as_raw();
        let width = self.alpha.width() as usize;
        let row = |y: usize| {
            u32::from(pixels[y * width + x.index]) * (256 - x.weight)
                + u32::from(pixels[y * width + x1]) * x.weight
        };
        f64::from(row(y.index) * (256 - y.weight) + row(y1) * y.weight) / (255.0 * 65_536.0)
    }
}

struct CardLayer {
    transform: SceneTransform,
    shadow_source: Option<MediaShadow>,
    pixels: RgbaImage,
}

/// CPU compositor with the static layers (background, effects, watermark)
/// precomputed once; the card is cached per transform and alpha silhouette.
pub struct SceneCompositor {
    style: SceneStyle,
    geometry: SceneGeometry,
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    background: RgbaImage,
    card: std::cell::RefCell<Option<CardLayer>>,
    camera_shadow: std::cell::RefCell<Option<CameraShadow>>,
    canvas_overlay: std::cell::RefCell<Option<(RgbaImage, RgbaImage)>>,
    vignette: Option<Vec<f32>>,
    watermark: Option<RgbaImage>,
}

impl SceneCompositor {
    pub fn new(
        style: &SceneStyle,
        canvas_width: u32,
        canvas_height: u32,
        source_width: u32,
        source_height: u32,
    ) -> Result<Self, String> {
        Self::build(
            style,
            canvas_width,
            canvas_height,
            source_width,
            source_height,
            None,
        )
    }

    /// Like [`new`](Self::new), but reuses the rendered background, vignette
    /// and watermark layers of `previous` when their inputs did not change,
    /// so interactive edits of one setting only redo that setting's work.
    pub fn rebuild(
        &self,
        style: &SceneStyle,
        canvas_width: u32,
        canvas_height: u32,
        source_width: u32,
        source_height: u32,
    ) -> Result<Self, String> {
        Self::build(
            style,
            canvas_width,
            canvas_height,
            source_width,
            source_height,
            Some(self),
        )
    }

    fn build(
        style: &SceneStyle,
        canvas_width: u32,
        canvas_height: u32,
        source_width: u32,
        source_height: u32,
        previous: Option<&Self>,
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
        let previous = previous
            .filter(|previous| previous.width == canvas_width && previous.height == canvas_height);
        let background = match previous.filter(|previous| {
            previous.style.background == style.background
                && previous.style.background_blur == style.background_blur
                && previous.style.background_noise == style.background_noise
                && previous.geometry.ui_scale == geometry.ui_scale
        }) {
            Some(previous) => previous.background.clone(),
            None => {
                let mut background =
                    render_background(&style.background, canvas_width, canvas_height)?;
                if style.background_blur > 0 {
                    let sigma = style.background_blur as f64 / 100.0 * 40.0 * geometry.ui_scale;
                    blur_image(&mut background, sigma);
                }
                if style.background_noise > 0 {
                    apply_noise(&mut background, style.background_noise as f64 / 100.0);
                }
                background
            }
        };
        let vignette = match previous.filter(|previous| previous.style.vignette == style.vignette) {
            Some(previous) => previous.vignette.clone(),
            None => (style.vignette > 0)
                .then(|| vignette_map(canvas_width, canvas_height, style.vignette as f64 / 100.0)),
        };
        let watermark =
            match previous.filter(|previous| previous.style.watermark == style.watermark) {
                Some(previous) => previous.watermark.clone(),
                None => style
                    .watermark
                    .as_ref()
                    .filter(|watermark| !watermark.text.trim().is_empty())
                    .and_then(|watermark| {
                        render_watermark(watermark, canvas_width, canvas_height).ok()
                    }),
            };
        Ok(Self {
            style: style.clone(),
            geometry,
            width: canvas_width,
            height: canvas_height,
            source_width,
            source_height,
            background,
            card: std::cell::RefCell::new(None),
            camera_shadow: std::cell::RefCell::new(None),
            canvas_overlay: std::cell::RefCell::new(None),
            vignette,
            watermark,
        })
    }

    pub fn geometry(&self) -> SceneGeometry {
        self.geometry
    }

    pub fn style(&self) -> &SceneStyle {
        &self.style
    }

    pub fn canvas_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn source_size(&self) -> (u32, u32) {
        (self.source_width, self.source_height)
    }

    /// Projection for the authored transform plus a frame's tilt.
    pub fn projection(&self, viewport: ViewportFrame) -> MediaProjection {
        self.geometry
            .projection(self.style.transform.with_motion(viewport))
    }

    pub fn loupe_geometry(
        &self,
        viewport: ViewportFrame,
    ) -> Option<super::loupe::LoupeGeometry> {
        let lens = viewport.loupe?;
        let projection = self.projection(viewport);
        let (left, top, width, height) = self.style.source_crop.visible_rect(viewport);
        let u = (lens.center.x - left) / width;
        let v = (lens.center.y - top) / height;
        let center = projection.project(u, v);
        // Exact homography derivative keeps lens size proportional across
        // preview/export resolutions, even when the media is tilted.
        let m = projection.forward;
        let denominator = m[2][0] * u + m[2][1] * v + m[2][2];
        let dxdu = (m[0][0] - center.0 * m[2][0]) / denominator;
        let dxdv = (m[0][1] - center.0 * m[2][1]) / denominator;
        let dydu = (m[1][0] - center.1 * m[2][0]) / denominator;
        let dydv = (m[1][1] - center.1 * m[2][1]) / denominator;
        let scale = ((dxdu * dydv - dxdv * dydu).abs()
            / (self.geometry.media.width * self.geometry.media.height)).sqrt();
        let radius = lens.radius * self.geometry.media.width.max(self.geometry.media.height)
            * scale;
        super::loupe::LoupeGeometry::new(lens, center, radius)
    }

    /// Composes one output frame.
    /// Cover-fit a full split screen cell without stretching screen pixels.
    /// Assigns an absolute fill fraction, so worker planning and renderer
    /// dispatch may safely both call this helper.
    pub fn camera_viewport(&self, viewport: ViewportFrame) -> ViewportFrame {
        self.geometry.camera_viewport(viewport, &self.style, (self.source_width,self.source_height))
    }

    pub fn compose(&self, input: FrameInput<'_>) -> RgbaImage {
        self.compose_layers(input, true, None)
    }

    /// Canvas overlays and the background outlive the optional media layer.
    pub fn compose_layers(
        &self,
        input: FrameInput<'_>,
        media_visible: bool,
        canvas_overlay: Option<&RgbaImage>,
    ) -> RgbaImage {
        let input = FrameInput { viewport: self.camera_viewport(input.viewport), ..input };
        let transform = self.style.transform.with_motion(input.viewport);
        let mut output = if media_visible {
            self.card_layer(transform, input.source, input.viewport)
        } else {
            self.background.clone()
        };
        let projection = self.geometry.projection(transform);
        let lens = media_visible
            .then(|| self.loupe_geometry(input.viewport))
            .flatten();
        // Keep only the card/background pass immutable. Lens pixels sample
        // original media at fractional coordinates, before canvas reduction.
        let lens_background = lens.map(|_| output.clone());
        let source_mips = sampling::Pyramid::cropped(input.source,self.style.source_crop.sample_bounds(input.source.width(),input.source.height()));
        if media_visible {
            self.paint_media(
                &mut output,
                &projection,
                input.source,
                &source_mips,
                input.overlay,
                input.viewport,
            );
            if let Some(pointer) = input.pointer {
                self.paint_pointer(&mut output, &projection, pointer, input.viewport);
            }
        }
        // The camera stays on the stage beneath the glass; the lens magnifies
        // only the card and media it samples, never the camera.
        if let Some(camera) = input.camera.filter(|_| self.style.camera.enabled) {
            self.paint_camera(&mut output, camera, input.viewport.magnification);
        }
        if media_visible {
            if let (Some(lens), Some(background)) = (lens, lens_background.as_ref()) {
                let (left, top, visible_x, visible_y) =
                    self.style.source_crop.visible_rect(input.viewport);
                let painter = MediaPainter {
                    geometry: self.geometry,
                    projection: &projection,
                    source: input.source,
                    source_mips: &source_mips,
                    magnification: lens.magnification as f64,
                    overlay: input
                        .overlay
                        .filter(|image| image.width() > 0 && image.height() > 0),
                    left,
                    top,
                    visible_x,
                    visible_y,
                    sample_bounds: self
                        .style
                        .source_crop
                        .sample_bounds(input.source.width(), input.source.height()),
                    x0: 0,
                    x1: self.width,
                };
                let commands = input
                    .pointer
                    .map(|pointer| self.pointer_commands(&projection, pointer, input.viewport))
                    .unwrap_or_default();
                let pointers: Vec<_> = commands.iter().map(ContinuousPointer::new).collect();
                let axis = projection.axis_aligned();
                let (width, height) = (self.width, self.height);
                super::loupe::paint_with_sampler(&mut output, lens, &|point| {
                    let point = [
                        point[0].clamp(0.5, width as f32 - 0.5),
                        point[1].clamp(0.5, height as f32 - 0.5),
                    ];
                    let mut color = painter.sample_continuous(background, point, axis);
                    for pointer in &pointers {
                        color = pointer.sample(color, point);
                    }
                    color
                });
            }
        }
        if let Some(watermark) = self.watermark.as_ref() {
            blend_layer(&mut output, watermark);
        }
        if let Some(vignette) = self.vignette.as_ref() {
            for (index, pixel) in output.pixels_mut().enumerate() {
                let factor = vignette[index];
                pixel[0] = (pixel[0] as f32 * factor).round() as u8;
                pixel[1] = (pixel[1] as f32 * factor).round() as u8;
                pixel[2] = (pixel[2] as f32 * factor).round() as u8;
            }
        }
        if let Some(layer) = canvas_overlay {
            if layer.dimensions() == output.dimensions() {
                blend_layer(&mut output, layer);
            } else {
                // Timed annotation sources reuse unchanged pixels. Resample
                // those pixels once, rather than once per video frame. Own
                // the key so mutating/reusing an input allocation is safe.
                let mut cache = self.canvas_overlay.borrow_mut();
                if !cache.as_ref().is_some_and(|(source, _)| source == layer) {
                    let resized = image::imageops::resize(
                        layer, self.width, self.height,
                        image::imageops::FilterType::Lanczos3,
                    );
                    *cache = Some((layer.clone(), resized));
                }
                blend_layer(&mut output, &cache.as_ref().unwrap().1);
            }
        }
        output
    }

    /// Cache the card while its transform and visible alpha silhouette match.
    /// Window resizes and viewport crops can change the shadow inside a fixed
    /// recording canvas, even when the card transform stays the same.
    fn card_layer(
        &self,
        transform: SceneTransform,
        source: &RgbaImage,
        viewport: ViewportFrame,
    ) -> RgbaImage {
        let shadow_source = self
            .geometry
            .shadow
            .and_then(|_| MediaShadow::from_source(source, self.style.source_crop.visible_rect(viewport), self.style.source_crop));
        let mut cache = self.card.borrow_mut();
        if let Some(layer) = cache.as_ref() {
            if layer.transform == transform && layer.shadow_source == shadow_source {
                return layer.pixels.clone();
            }
        }
        let mut pixels = self.background.clone();
        let projection = self.geometry.projection(transform);
        if let Some(shadow) = self.geometry.shadow {
            paint_shadow(
                &mut pixels,
                &projection,
                self.geometry,
                shadow,
                shadow_source.as_ref(),
            );
        }
        if let Some(bar) = self.style.window_frame.bar_color() {
            if self.geometry.title_height > 0.0 {
                self.paint_title_bar(&mut pixels, &projection, bar);
            }
        }
        if self.geometry.border_width > 0.0 && self.style.border_opacity > 0 {
            self.paint_border(&mut pixels, &projection);
        }
        *cache = Some(CardLayer {
            transform,
            shadow_source,
            pixels: pixels.clone(),
        });
        pixels
    }

    /// Signed distance (media pixels) from `(u, v)` to the media area: the
    /// card surface cut off at the title bar, negative inside.
    fn media_distance(&self, u: f64, v: f64) -> f64 {
        self.geometry
            .surface_distance(u, v)
            .max(-v * self.geometry.media.height)
    }

    fn pixel_range(&self, projection: &MediaProjection, margin: f64) -> (u32, u32, u32, u32) {
        let bounds = self.geometry.surface_bounds(projection);
        let x0 = (bounds.x - margin).floor().max(0.0) as u32;
        let y0 = (bounds.y - margin).floor().max(0.0) as u32;
        let x1 = ((bounds.right() + margin).ceil().max(0.0) as u32).min(self.width);
        let y1 = ((bounds.bottom() + margin).ceil().max(0.0) as u32).min(self.height);
        (x0, y0, x1, y1)
    }

    /// Title bar with traffic lights above the media, inside the card outline.
    fn paint_title_bar(&self, output: &mut RgbaImage, projection: &MediaProjection, bar: [u8; 3]) {
        const LIGHTS: [[u8; 3]; 3] = [[0xff, 0x5f, 0x57], [0xfe, 0xbc, 0x2e], [0x28, 0xc8, 0x40]];
        let geometry = self.geometry;
        let title = geometry.title_height;
        let light_radius = title * 0.2;
        let light_spacing = title * 0.67;
        let light_start = title * 0.7;
        let top = geometry.title_top();
        let (x0, y0, x1, y1) = self.pixel_range(projection, 2.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let (u, v) = projection.unproject(px, py);
                if !(-0.05..=1.05).contains(&u) || v < top - 0.05 || v > 0.05 {
                    continue;
                }
                let pixel_size = projection.pixel_size_at(px, py);
                let distance = geometry
                    .surface_distance(u, v)
                    .max(v * geometry.media.height);
                let coverage = (0.5 - distance / pixel_size).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                blend_pixel(output, x, y, bar, coverage);
                let local_x = u * geometry.media.width;
                let local_y = v * geometry.media.height + title * 0.5;
                for (index, color) in LIGHTS.into_iter().enumerate() {
                    let dx = local_x - (light_start + light_spacing * index as f64);
                    let light = (dx * dx + local_y * local_y).sqrt() - light_radius;
                    let light_coverage = (0.5 - light / pixel_size).clamp(0.0, 1.0);
                    if light_coverage > 0.0 {
                        blend_pixel(output, x, y, color, light_coverage * coverage);
                    }
                }
            }
        }
    }

    fn paint_border(&self, output: &mut RgbaImage, projection: &MediaProjection) {
        let tint = unpack(self.style.border_color);
        let alpha = self.style.border_opacity as f64 / 100.0;
        let border = self.geometry.border_width;
        let (x0, y0, x1, y1) = self.pixel_range(projection, border * 2.0 + 2.0);
        let top = self.geometry.title_top() - 0.3;
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let (u, v) = projection.unproject(px, py);
                if !(-0.3..=1.3).contains(&u) || !(top..=1.3).contains(&v) {
                    continue;
                }
                let pixel_size = projection.pixel_size_at(px, py);
                let distance = self.geometry.surface_distance(u, v);
                let outer = (0.5 - (distance - border) / pixel_size).clamp(0.0, 1.0);
                let inner = (0.5 - distance / pixel_size).clamp(0.0, 1.0);
                let coverage = (outer - inner).max(0.0);
                if coverage > 0.0 {
                    blend_pixel(output, x, y, tint, coverage * alpha);
                }
            }
        }
    }

    fn paint_media(
        &self,
        output: &mut RgbaImage,
        projection: &MediaProjection,
        source: &RgbaImage,
        source_mips: &sampling::Pyramid,
        overlay: Option<&RgbaImage>,
        viewport: ViewportFrame,
    ) {
        if source.width() == 0 || source.height() == 0 {
            return;
        }
        let overlay = overlay.filter(|layer| layer.width() > 0 && layer.height() > 0);
        let (left, top, visible_x, visible_y) = self.style.source_crop.visible_rect(viewport);
        let (x0, y0, x1, y1) = self.pixel_range(projection, 2.0);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let painter = MediaPainter {
            geometry: self.geometry,
            projection,
            source,
            source_mips,
            magnification: 1.0,
            overlay,
            left,
            top,
            visible_x,
            visible_y,
            sample_bounds: self.style.source_crop.sample_bounds(source.width(), source.height()),
            x0,
            x1,
        };
        // Every row is independent, so the rows are painted in parallel.
        let stride = output.width() as usize * 4;
        let buffer: &mut [u8] = output.as_mut();
        let region = &mut buffer[y0 as usize * stride..y1 as usize * stride];
        let threads = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .clamp(1, 8);
        let rows_per_chunk = ((y1 - y0) as usize).div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (index, chunk) in region.chunks_mut(rows_per_chunk * stride).enumerate() {
                let first_row = y0 + (index * rows_per_chunk) as u32;
                let painter = &painter;
                scope.spawn(move || painter.paint_rows(chunk, stride, first_row));
            }
        });
    }

    fn paint_camera(&self, output: &mut RgbaImage, camera: &RgbaImage, zoom: f64) {
        if camera.width() == 0 || camera.height() == 0 {
            return;
        }
        let mut overlay = self.style.camera;
        let rect = overlay.rect_for_frame(self.width as f64, self.height as f64, camera.width() as f64 / camera.height() as f64, zoom);
        let shadow_strength=overlay.shadow_strength();
        if shadow_strength > 0.0 {
            let mut cache = self.camera_shadow.borrow_mut();
            if cache.as_ref().is_none_or(|shadow| shadow.rect != rect || shadow.radius != overlay.radius(rect)) {
                *cache = Some(CameraShadow::new(self.width, self.height, overlay, rect));
            }
            let shadow = cache.as_ref().expect("camera shadow just prepared");
            shadow.paint(output, overlay.opacity()*shadow_strength);
            overlay.shadow = false;
        }
        paint_camera_overlay(output, camera, overlay, rect);
    }

    fn paint_pointer(&self, output: &mut RgbaImage, projection: &MediaProjection,
        pointer: &PointerOverlay, viewport: ViewportFrame) {
        let clip = Rect { x: 0.0, y: 0.0, width: self.width as f64, height: self.height as f64 };
        for command in self.pointer_commands(projection, pointer, viewport) {
            match command {
                PointerCommand::Disc { x, y, radius, line_width, color, opacity } => {
                    if let Some(line_width) = line_width {
                        paint_ring(output, clip, x, y, radius, line_width, color, opacity);
                    } else {
                        paint_disc(output, clip, x, y, radius, color, opacity);
                    }
                }
                PointerCommand::Bitmap { bitmap, placement, opacity, shadow_only } => {
                    paint_cursor_bitmap(output, clip, &bitmap, placement, opacity, shadow_only);
                }
            }
        }
    }

    fn pointer_commands(
        &self,
        projection: &MediaProjection,
        pointer: &PointerOverlay,
        viewport: ViewportFrame,
    ) -> Vec<PointerCommand> {
        let mut commands = Vec::with_capacity(4);
        let style = self.style.pointer;
        if !style.visible {
            return commands;
        }
        let frame = &pointer.frame;
        let (left, top, visible_x, visible_y) = self.style.source_crop.visible_rect(viewport);
        let to_canvas = |point: NormalizedPoint| {
            let u = (point.x - left) / visible_x;
            let v = (point.y - top) / visible_y;
            (projection.project(u, v), u, v)
        };
        let pointer_scale = style.scale as f64 / 100.0;
        if style.click_effects {
            if let Some(press) = frame.press.as_ref() {
                let ((x, y), u, v) = to_canvas(press.location);
                if (-0.02..=1.02).contains(&u) && (-0.02..=1.02).contains(&v) {
                    let local_scale = projection.screen_scale_at(u, v);
                    let geometry = pointer_press_effect_geometry(
                        press.progress,
                        self.geometry.media.height * local_scale,
                        viewport.magnification.max(1.0) * pointer_scale,
                    );
                    let color = unpack(style.click_color);
                    commands.push(PointerCommand::Disc { x, y, radius: geometry.impact_radius,
                        line_width: None, color, opacity: geometry.impact_opacity });
                    commands.push(PointerCommand::Disc { x, y, radius: geometry.ripple_radius,
                        line_width: Some(geometry.ripple_line_width), color, opacity: geometry.ripple_opacity });
                }
            }
        }
        // The timeline already applies the independent idle/typing options.
        // Overriding its opacity here also disabled typing hide when idle hide
        // was off, and revealed the cursor before its first recorded event.
        let opacity = frame.opacity;
        if opacity <= 0.0 {
            return commands;
        }
        let ((x, y), u, v) = to_canvas(frame.location);
        if !(-0.02..=1.02).contains(&u) || !(-0.02..=1.02).contains(&v) {
            return commands;
        }
        let local_scale = projection.screen_scale_at(u, v);
        // Canvas pixels per normalized media unit at this point.
        let unit_scale = (
            self.geometry.media.width * local_scale / visible_x,
            self.geometry.media.height * local_scale / visible_y,
        );
        let bitmap = frame
            .bitmap
            .as_ref()
            .filter(|_| style.family == CursorFamily::Recorded);
        if let Some(bitmap) = bitmap {
            // Captured cursors keep their on-screen proportion to the
            // recording, boosted so 100% reads like the vector arrow.
            let media_scale =
                local_scale * pointer_scale * frame.magnification * CAPTURED_CURSOR_BOOST;
            let width = bitmap.reference_width * self.geometry.media.width * media_scale / visible_x;
            let height = bitmap.reference_height * self.geometry.media.height * media_scale / visible_y;
            if width < 1.0 || height < 1.0 {
                return commands;
            }
            let placement = BitmapPlacement {
                hotspot_x: x,
                hotspot_y: y,
                width,
                height,
                tilt_degrees: frame.tilt_degrees,
                smear: cursor_smear(frame.velocity, unit_scale, width, style.motion_blur_amount.unwrap_or(1.0)),
            };
            let (bitmap, placement) = blurred_cursor(bitmap.clone(), placement, frame.blur_radius);
            if style.shadow {
                let shadow = BitmapPlacement {
                    hotspot_x: x + height * 0.05,
                    hotspot_y: y + height * 0.07,
                    ..placement
                };
                commands.push(PointerCommand::Bitmap { bitmap: bitmap.clone(), placement: shadow, opacity: opacity * 0.35, shadow_only: true });
            }
            commands.push(PointerCommand::Bitmap { bitmap: bitmap.clone(), placement, opacity, shadow_only: false });
            return commands;
        }
        // Styled cursors follow Cap: a fixed height relative to the media,
        // in the chosen family's rendition of the recorded shape.  The
        // artwork carries its own soft shadow.
        let family = match style.family {
            CursorFamily::Recorded => CursorFamily::MacOs,
            family => family,
        };
        let shape = frame
            .bitmap
            .as_deref()
            .and_then(|bitmap| bitmap.shape)
            .unwrap_or_default();
        let height = ASSET_CURSOR_HEIGHT * self.geometry.media.height * local_scale / visible_y
            * pointer_scale
            * frame.magnification;
        if height < 1.0 {
            return commands;
        }
        // Rasterize at the next coarse step above the drawn size and let
        // the painter resample to the exact fractional height: zoom and
        // click animations scale smoothly instead of stepping a pixel at a
        // time, and a zoom no longer fills the cache with one sprite per
        // integer height.
        let raster_height = (height / CURSOR_RASTER_STEP).ceil() * CURSOR_RASTER_STEP;
        let Some(asset) = cursor_assets::rasterize(family, shape, raster_height as u32) else {
            return commands;
        };
        let width = height * f64::from(asset.image.width()) / f64::from(asset.image.height());
        let placement = BitmapPlacement {
            hotspot_x: x,
            hotspot_y: y,
            width,
            height,
            tilt_degrees: frame.tilt_degrees,
            smear: cursor_smear(frame.velocity, unit_scale, width, style.motion_blur_amount.unwrap_or(1.0)),
        };
        let (asset, placement) = blurred_cursor(asset, placement, frame.blur_radius);
        commands.push(PointerCommand::Bitmap { bitmap: asset, placement, opacity, shadow_only: false });
        commands
    }
}

enum PointerCommand {
    Disc { x: f64, y: f64, radius: f64, line_width: Option<f64>, color: [u8; 3], opacity: f64 },
    Bitmap { bitmap: std::sync::Arc<PointerBitmap>, placement: BitmapPlacement, opacity: f64, shadow_only: bool },
}

#[inline]
fn continuous_over(base: [f32; 4], rgb: [f32; 3], alpha: f32) -> [f32; 4] {
    let alpha = alpha.clamp(0.0, 1.0);
    let target = [rgb[0], rgb[1], rgb[2], 255.0];
    std::array::from_fn(|i| {
        (base[i] + (target[i] - base[i]) * alpha)
            .clamp(0.0, 255.0)
            .round()
    })
}

/// Frame-constant pointer transforms, shared by every optical tap. Sampling
/// the artwork directly preserves its hotspot, alpha, rotation and smear
/// without first baking the cursor into a smaller preview bitmap.
struct ContinuousPointer<'a> {
    command: &'a PointerCommand,
    bounds: [f64; 4],
    sin: f64,
    cos: f64,
    anchor: (f64, f64),
    smear: (f64, f64),
    taps: usize,
}

impl<'a> ContinuousPointer<'a> {
    fn new(command: &'a PointerCommand) -> Self {
        let mut result = Self {
            command,
            bounds: [0.0; 4],
            sin: 0.0,
            cos: 1.0,
            anchor: (0.0, 0.0),
            smear: (0.0, 0.0),
            taps: 1,
        };
        match command {
            PointerCommand::Disc { x, y, radius, .. } => {
                result.bounds = [
                    (x - radius - 1.0).floor(),
                    (y - radius - 1.0).floor(),
                    (x + radius + 1.0).ceil(),
                    (y + radius + 1.0).ceil(),
                ];
            }
            PointerCommand::Bitmap {
                bitmap,
                placement: p,
                ..
            } => {
                (result.sin, result.cos) = p.tilt_degrees.to_radians().sin_cos();
                result.anchor = (bitmap.anchor.x * p.width, bitmap.anchor.y * p.height);
                result.smear = (
                    p.smear.0 * result.cos + p.smear.1 * result.sin,
                    -p.smear.0 * result.sin + p.smear.1 * result.cos,
                );
                result.taps = if p.smear.0.hypot(p.smear.1) < 0.5 {
                    1
                } else {
                    CURSOR_SMEAR_TAPS
                };
                let corners = [
                    (0.0, 0.0),
                    (p.width, 0.0),
                    (0.0, p.height),
                    (p.width, p.height),
                ]
                .map(|(x, y)| {
                    let (x, y) = (x - result.anchor.0, y - result.anchor.1);
                    (
                        p.hotspot_x + x * result.cos - y * result.sin,
                        p.hotspot_y + x * result.sin + y * result.cos,
                    )
                });
                result.bounds = [
                    (corners.iter().map(|p| p.0).fold(f64::INFINITY, f64::min)
                        - 1.0
                        - p.smear.0.max(0.0))
                    .floor(),
                    (corners.iter().map(|p| p.1).fold(f64::INFINITY, f64::min)
                        - 1.0
                        - p.smear.1.max(0.0))
                    .floor(),
                    (corners
                        .iter()
                        .map(|p| p.0)
                        .fold(f64::NEG_INFINITY, f64::max)
                        + 1.0
                        - p.smear.0.min(0.0))
                    .ceil(),
                    (corners
                        .iter()
                        .map(|p| p.1)
                        .fold(f64::NEG_INFINITY, f64::max)
                        + 1.0
                        - p.smear.1.min(0.0))
                    .ceil(),
                ];
            }
        }
        result
    }

    #[inline]
    fn sample(&self, base: [f32; 4], point: [f32; 2]) -> [f32; 4] {
        let (px, py) = (f64::from(point[0]), f64::from(point[1]));
        if px < self.bounds[0]
            || py < self.bounds[1]
            || px >= self.bounds[2]
            || py >= self.bounds[3]
        {
            return base;
        }
        match self.command {
            PointerCommand::Disc {
                x,
                y,
                radius,
                line_width,
                color,
                opacity,
            } => {
                if *opacity <= 0.0 || *radius <= 0.0 {
                    return base;
                }
                let distance = (px - x).hypot(py - y);
                let mut coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
                if let Some(width) = line_width {
                    coverage *= (distance - (radius - width).max(0.0) + 0.5).clamp(0.0, 1.0);
                }
                continuous_over(base, color.map(f32::from), (coverage * opacity) as f32)
            }
            PointerCommand::Bitmap {
                bitmap,
                placement: p,
                opacity,
                shadow_only,
            } => {
                if *opacity <= 0.0 || p.width <= 0.0 || p.height <= 0.0 {
                    return base;
                }
                let (dx, dy) = (px - p.hotspot_x, py - p.hotspot_y);
                let (lx, ly) = (
                    dx * self.cos + dy * self.sin + self.anchor.0,
                    -dx * self.sin + dy * self.cos + self.anchor.1,
                );
                let source = &bitmap.image;
                let mut sum = [0.0; 4];
                for i in 0..self.taps {
                    let t = i as f64 / self.taps.saturating_sub(1).max(1) as f64;
                    let (tx, ty) = (lx + self.smear.0 * t, ly + self.smear.1 * t);
                    if tx < -1.0 || ty < -1.0 || tx > p.width + 1.0 || ty > p.height + 1.0 {
                        continue;
                    }
                    let sx = tx * f64::from(source.width()) / p.width - 0.5;
                    let sy = ty * f64::from(source.height()) / p.height - 0.5;
                    let (ix, iy) = (sx.floor(), sy.floor());
                    let (fx, fy) = (sx - ix, sy - iy);
                    for (x, y, weight) in [
                        (ix, iy, (1.0 - fx) * (1.0 - fy)),
                        (ix + 1.0, iy, fx * (1.0 - fy)),
                        (ix, iy + 1.0, (1.0 - fx) * fy),
                        (ix + 1.0, iy + 1.0, fx * fy),
                    ] {
                        if x < 0.0
                            || y < 0.0
                            || x >= f64::from(source.width())
                            || y >= f64::from(source.height())
                        {
                            continue;
                        }
                        let pixel = source.get_pixel(x as u32, y as u32);
                        for channel in 0..4 {
                            sum[channel] += f64::from(pixel[channel]) * weight;
                        }
                    }
                }
                let sum = sum.map(|value| value / self.taps as f64);
                let coverage = sum[3] / 255.0;
                if coverage <= 0.002 {
                    return base;
                }
                let color = if *shadow_only {
                    [0.0; 3]
                } else {
                    [sum[0], sum[1], sum[2]]
                        .map(|value| (value / coverage).round().clamp(0.0, 255.0) as f32)
                };
                continuous_over(base, color, (coverage * opacity) as f32)
            }
        }
    }
}

/// Paints the media surface into a band of output rows.  Untransformed
/// media takes an integer bilinear fast path away from the surface edge;
/// the edge band and every projected pixel use the exact per-pixel path.
struct MediaPainter<'a> {
    geometry: SceneGeometry,
    projection: &'a MediaProjection,
    source: &'a RgbaImage,
    source_mips: &'a sampling::Pyramid,
    magnification: f64,
    overlay: Option<&'a RgbaImage>,
    left: f64,
    top: f64,
    visible_x: f64,
    visible_y: f64,
    sample_bounds: (f64, f64, f64, f64),
    x0: u32,
    x1: u32,
}

/// One axis of a bilinear tap: the lower texel and the 8-bit weight of the
/// next one.
#[derive(Clone, Copy)]
struct Tap {
    index: usize,
    weight: u32,
}

fn tap(position: f64, size: u32) -> Tap {
    let max = f64::from(size - 1);
    let position = position.clamp(0.0, max);
    let index = position.floor();
    Tap {
        index: index as usize,
        weight: ((position - index) * 256.0).round() as u32,
    }
}

/// Bilinear sample in 8-bit fixed point, matching `sample_bilinear`.
fn sample_fixed(image: &RgbaImage, x: Tap, y: Tap) -> [u8; 4] {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let x1 = (x.index + 1).min(width - 1);
    let y1 = (y.index + 1).min(height - 1);
    let raw = image.as_raw();
    let row0 = y.index * width * 4;
    let row1 = y1 * width * 4;
    let p00 = &raw[row0 + x.index * 4..row0 + x.index * 4 + 4];
    let p10 = &raw[row0 + x1 * 4..row0 + x1 * 4 + 4];
    let p01 = &raw[row1 + x.index * 4..row1 + x.index * 4 + 4];
    let p11 = &raw[row1 + x1 * 4..row1 + x1 * 4 + 4];
    let (fx, fy) = (x.weight, y.weight);
    let mut out = [0u8; 4];
    for channel in 0..4 {
        let top = u32::from(p00[channel]) * (256 - fx) + u32::from(p10[channel]) * fx;
        let bottom = u32::from(p01[channel]) * (256 - fx) + u32::from(p11[channel]) * fx;
        out[channel] = ((top * (256 - fy) + bottom * fy + 32_768) >> 16) as u8;
    }
    out
}

impl MediaPainter<'_> {
    fn filtered(&self, px: f64, py: f64, u: f64, v: f64) -> Option<[u8;4]> {
        let at=self.projection.unproject(px,py);
        let m=self.projection.inverse;
        let w=m[2][0]*px+m[2][1]*py+m[2][2];
        let w=if w.abs()<1e-12 {1e-12}else{w};
        let dx=((m[0][0]-at.0*m[2][0])/w,(m[1][0]-at.1*m[2][0])/w);
        let dy=((m[0][1]-at.0*m[2][1])/w,(m[1][1]-at.1*m[2][1])/w);
        let sx=self.visible_x*self.source.width() as f64/self.magnification;
        let sy=self.visible_y*self.source.height() as f64/self.magnification;
        self.source_mips.sample(self.source,(u,v),(dx.0*sx,dx.1*sy),(dy.0*sx,dy.1*sy),self.sample_bounds)
    }

    /// Reconstruct one continuous canvas position from the original source.
    /// Unlike the raster pass, the lens never snaps its source weights to 8.8
    /// or samples four already-reduced canvas pixels.
    fn sample_continuous(
        &self,
        background: &RgbaImage,
        p: [f32; 2],
        axis: Option<(f64, f64, f64, f64)>,
    ) -> [f32; 4] {
        if self.source.width() == 0 || self.source.height() == 0 {
            return super::loupe::sample(background, p);
        }
        let (px, py) = (f64::from(p[0]), f64::from(p[1]));
        let (u, v) = axis.map_or_else(
            || self.projection.unproject(px, py),
            |(a, b, c, d)| (a * px + b, c * py + d),
        );
        if !(-0.05..=1.05).contains(&u) || !(-0.05..=1.05).contains(&v) {
            return super::loupe::sample(background, p);
        }
        let ps = self.projection.pixel_size_at(px, py);
        let media = self.geometry.media;
        let band = self.geometry.radius + 2.0 * ps + 1.0;
        let interior = axis.is_some()
            && v * media.height > ps + 1.0
            && v * media.height < media.height - band
            && u * media.width > band
            && u * media.width < media.width - band;
        let coverage = if interior {
            1.0
        } else {
            (0.5 - self.media_distance(u, v) / ps).clamp(0.0, 1.0)
        };
        if coverage <= 0.0 {
            return super::loupe::sample(background, p);
        }
        let source_u = (self.left + u.clamp(0.0, 1.0) * self.visible_x)
            .clamp(self.sample_bounds.0, self.sample_bounds.2);
        let source_v = (self.top + v.clamp(0.0, 1.0) * self.visible_y)
            .clamp(self.sample_bounds.1, self.sample_bounds.3);
        let mut color = self.filtered(px,py,source_u,source_v).unwrap_or_else(|| sample_bilinear(
            self.source,
            source_u * f64::from(self.source.width()) - 0.5,
            source_v * f64::from(self.source.height()) - 0.5,
        ))
        .map(f32::from);
        if let Some(overlay) = self.overlay {
            let annotation = sample_bilinear(
                overlay,
                source_u * f64::from(overlay.width()) - 0.5,
                source_v * f64::from(overlay.height()) - 0.5,
            );
            color = continuous_over(
                color,
                [annotation[0], annotation[1], annotation[2]].map(f32::from),
                f32::from(annotation[3]) / 255.0,
            );
        }
        if coverage >= 1.0 && color[3] >= 255.0 {
            return color;
        }
        continuous_over(
            super::loupe::sample(background, p),
            [color[0], color[1], color[2]],
            coverage as f32 * color[3] / 255.0,
        )
    }

    fn media_distance(&self, u: f64, v: f64) -> f64 {
        self.geometry
            .surface_distance(u, v)
            .max(-v * self.geometry.media.height)
    }

    fn paint_rows(&self, rows: &mut [u8], stride: usize, first_row: u32) {
        let axis_aligned = self.projection.axis_aligned();
        let source_width = f64::from(self.source.width());
        // Column tables for the axis-aligned path: media `u` and the
        // source tap are the same for every row.
        let columns: Vec<(f64, Tap, Option<Tap>)> = axis_aligned
            .map(|(a, b, _, _)| {
                (self.x0..self.x1)
                    .map(|x| {
                        let u = a * (f64::from(x) + 0.5) + b;
                        let media_u = (self.left + u.clamp(0.0, 1.0) * self.visible_x).clamp(self.sample_bounds.0, self.sample_bounds.2);
                        (
                            u,
                            tap(media_u * source_width - 0.5, self.source.width()),
                            self.overlay.map(|layer| {
                                tap(media_u * f64::from(layer.width()) - 0.5, layer.width())
                            }),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let media = self.geometry.media;
        let pixel_size = self.projection.affine_pixel_size.unwrap_or(1.0);
        // Pixels this close (in media pixels) to the surface outline take
        // the exact path so corners and edge coverage are unchanged.
        let band = self.geometry.radius + 2.0 * pixel_size + 1.0;
        let surface_height = media.height + self.geometry.title_height;

        for (row_index, row) in rows.chunks_mut(stride).enumerate() {
            let y = first_row + row_index as u32;
            let py = f64::from(y) + 0.5;
            let row_taps = axis_aligned.map(|(_, _, c, d)| {
                let v = c * py + d;
                let media_v = (self.top + v.clamp(0.0, 1.0) * self.visible_y).clamp(self.sample_bounds.1, self.sample_bounds.3);
                let surface_y = v * media.height + self.geometry.title_height;
                let interior_row =
                    v * media.height > pixel_size + 1.0 && surface_y < surface_height - band;
                (
                    v,
                    interior_row,
                    tap(
                        media_v * f64::from(self.source.height()) - 0.5,
                        self.source.height(),
                    ),
                    self.overlay.map(|layer| {
                        tap(media_v * f64::from(layer.height()) - 0.5, layer.height())
                    }),
                )
            });
            for x in self.x0..self.x1 {
                let pixel = &mut row[x as usize * 4..x as usize * 4 + 4];
                if let Some((v, interior_row, source_y, overlay_y)) = row_taps {
                    let (u, source_x, overlay_x) = columns[(x - self.x0) as usize];
                    if !(-0.05..=1.05).contains(&u) || !(-0.05..=1.05).contains(&v) {
                        continue;
                    }
                    let px = u * media.width;
                    if interior_row && px > band && px < media.width - band {
                        let media_u=(self.left+u.clamp(0.0,1.0)*self.visible_x).clamp(self.sample_bounds.0,self.sample_bounds.2);
                        let media_v=(self.top+v.clamp(0.0,1.0)*self.visible_y).clamp(self.sample_bounds.1,self.sample_bounds.3);
                        let mut sample = self.filtered(x as f64+0.5,py,media_u,media_v).unwrap_or_else(|| sample_fixed(self.source, source_x, source_y));
                        if let (Some(layer), Some(ox), Some(oy)) =
                            (self.overlay, overlay_x, overlay_y)
                        {
                            composite_over(&mut sample, sample_fixed(layer, ox, oy));
                        }
                        blend_fixed(pixel, sample, 256);
                        continue;
                    }
                }
                self.paint_exact(pixel, f64::from(x) + 0.5, py);
            }
        }
    }

    /// The original per-pixel path: exact projection, edge coverage and
    /// floating-point sampling.
    fn paint_exact(&self, pixel: &mut [u8], px: f64, py: f64) {
        let (u, v) = self.projection.unproject(px, py);
        if !(-0.05..=1.05).contains(&u) || !(-0.05..=1.05).contains(&v) {
            return;
        }
        let pixel_size = self.projection.pixel_size_at(px, py);
        let coverage = (0.5 - self.media_distance(u, v) / pixel_size).clamp(0.0, 1.0);
        if coverage <= 0.0 {
            return;
        }
        let media_u = (self.left + u.clamp(0.0, 1.0) * self.visible_x).clamp(self.sample_bounds.0, self.sample_bounds.2);
        let media_v = (self.top + v.clamp(0.0, 1.0) * self.visible_y).clamp(self.sample_bounds.1, self.sample_bounds.3);
        let mut sample = self.filtered(px,py,media_u,media_v).unwrap_or_else(|| sample_bilinear(
            self.source,
            media_u * f64::from(self.source.width()) - 0.5,
            media_v * f64::from(self.source.height()) - 0.5,
        ));
        if let Some(layer) = self.overlay {
            let over = sample_bilinear(
                layer,
                media_u * f64::from(layer.width()) - 0.5,
                media_v * f64::from(layer.height()) - 0.5,
            );
            let alpha = f64::from(over[3]) / 255.0;
            for channel in 0..3 {
                sample[channel] = (f64::from(sample[channel])
                    + (f64::from(over[channel]) - f64::from(sample[channel])) * alpha)
                    .round() as u8;
            }
            sample[3] =
                (f64::from(sample[3]) + (255.0 - f64::from(sample[3])) * alpha).round() as u8;
        }
        blend_slice(
            pixel,
            [sample[0], sample[1], sample[2]],
            coverage * f64::from(sample[3]) / 255.0,
        );
    }
}

/// Straight-alpha "over" of `over` onto `under`, in place.
fn composite_over(under: &mut [u8; 4], over: [u8; 4]) {
    let alpha = u32::from(over[3]);
    for channel in 0..3 {
        under[channel] =
            ((u32::from(under[channel]) * (255 - alpha) + u32::from(over[channel]) * alpha + 127)
                / 255) as u8;
    }
    under[3] = ((u32::from(under[3]) * (255 - alpha) + 255 * alpha + 127) / 255) as u8;
}

/// Blends a straight-alpha sample onto a pixel with an 8.8 fixed-point
/// coverage (256 = full).
fn blend_fixed(pixel: &mut [u8], sample: [u8; 4], coverage: u32) {
    let alpha = u32::from(sample[3]) * coverage;
    if alpha >= 255 * 256 {
        pixel[..4].copy_from_slice(&sample);
        return;
    }
    if alpha == 0 {
        return;
    }
    let scale = 255 * 256;
    for channel in 0..3 {
        pixel[channel] = ((u32::from(pixel[channel]) * (scale - alpha)
            + u32::from(sample[channel]) * alpha
            + scale / 2)
            / scale) as u8;
    }
    pixel[3] = ((u32::from(pixel[3]) * (scale - alpha) + 255 * alpha + scale / 2) / scale) as u8;
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Enlarged framing preview using the exact crop, mask, mirror, and shadow
/// painter used for the recording. Placement and size remain editor settings.
pub fn camera_framing_preview(camera: &RgbaImage, overlay: CameraOverlay) -> RgbaImage {
    camera_framing_preview_in_canvas(camera, overlay, 1.0)
}

pub fn camera_framing_preview_in_canvas(camera: &RgbaImage, overlay: CameraOverlay, canvas_aspect: f64) -> RgbaImage {
    // Framing previews run on a worker; retain one plan instead of rebuilding
    // the network each time a camera frame or edge setting changes.
    static MATTING: std::sync::Mutex<Option<super::camera_matting::CameraMatting>> = std::sync::Mutex::new(None);
    let cutout = if overlay.cutout_strength() > 0.0 {
        let mut cache = MATTING.lock().unwrap_or_else(|error| error.into_inner());
        match cache.get_or_insert_with(super::camera_matting::CameraMatting::new).process(camera, overlay) {
            Ok(image) => Some(image),
            Err(error) => { eprintln!("Camera framing: {error}"); None }
        }
    } else { None };
    let camera = cutout.as_deref().unwrap_or(camera);
    let mut output = RgbaImage::from_pixel(200, 200, Rgba([251, 251, 251, 255]));
    let composed = overlay.rect_for_frame(canvas_aspect, 1.0,
        camera.width() as f64 / camera.height().max(1) as f64, 1.0);
    let rect = super::camera_layout::fit_rect(
        Rect { x: 12.0, y: 12.0, width: 176.0, height: 176.0 },
        composed.width / composed.height,
    );
    paint_camera_overlay(&mut output, camera, overlay, rect);
    output
}

/// Only nonzero shadow pixels are retained. Geometry stays fixed for the
/// lifetime of a compositor; changing webcam frames must not repeat the blur.
struct CameraShadow {
    rect: Rect,
    radius: f64,
    pixels: Vec<(usize, f64)>,
}

impl CameraShadow {
    fn new(width: u32, height: u32, overlay: CameraOverlay, rect: Rect) -> Self {
        let radius = overlay.radius(rect);
        let width = width as usize;
        let height = height as usize;
        let mut mask = vec![0.0f32; width * height];
        let offset = rect.height * 0.06;
        let x0 = (rect.x - 2.0).floor().max(0.0) as usize;
        let y0 = (rect.y + offset - 2.0).floor().max(0.0) as usize;
        let x1 = ((rect.right() + 2.0).ceil().max(0.0) as usize).min(width);
        let y1 = ((rect.bottom() + offset + 2.0).ceil().max(0.0) as usize).min(height);
        for y in y0..y1 {
            for x in x0..x1 {
                let u = (x as f64 + 0.5 - rect.x) / rect.width;
                let v = (y as f64 + 0.5 - offset - rect.y) / rect.height;
                let distance = rounded_rect_distance(u, v, rect.width, rect.height, radius);
                mask[y * width + x] = (0.5 - distance).clamp(0.0, 1.0) as f32;
            }
        }
        blur_plane(&mut mask, width, height, rect.height * 0.05 * 0.5 + 1.0);
        let pixels = mask.into_iter().enumerate().filter_map(|(index, value)| {
            let alpha = f64::from(value) * 0.35;
            (alpha > 0.001).then_some((index * 4, alpha))
        }).collect();
        Self { rect, radius, pixels }
    }

    fn paint(&self, output: &mut RgbaImage, opacity: f64) {
        let raw: &mut [u8] = output.as_mut();
        for &(offset, alpha) in &self.pixels {
            blend_slice(&mut raw[offset..offset + 4], [0, 0, 0], alpha * opacity);
        }
    }
}

/// Shared cover-fit camera renderer for the launcher, editor, and export.
fn paint_camera_overlay(output: &mut RgbaImage, camera: &RgbaImage, overlay: CameraOverlay, rect: Rect) {
    if camera.width() == 0 || camera.height() == 0 || overlay.opacity() <= 0.0 {
        return;
    }
    let radius = overlay.radius(rect);
    if overlay.shadow_strength() > 0.0 {
        CameraShadow::new(output.width(), output.height(), overlay, rect).paint(output, overlay.opacity()*overlay.shadow_strength());
    }
    let frame_width = camera.width() as f64;
    let frame_height = camera.height() as f64;
    let crop = super::camera_layout::source_crop(overlay, frame_width / frame_height, rect);
    let x0 = rect.x.floor().max(0.0) as u32;
    let y0 = rect.y.floor().max(0.0) as u32;
    let x1 = (rect.right().ceil().max(0.0) as u32).min(output.width());
    let y1 = (rect.bottom().ceil().max(0.0) as u32).min(output.height());
    for y in y0..y1 {
        for x in x0..x1 {
            let local_x = x as f64 + 0.5 - rect.x;
            let local_y = y as f64 + 0.5 - rect.y;
            let u = local_x / rect.width;
            let v = local_y / rect.height;
            let coverage = (0.5 - rounded_rect_distance(u, v, rect.width, rect.height, radius))
                .clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let source_u = if overlay.mirror { 1.0 - u } else { u };
            let sx = (crop.x + source_u * crop.width) * frame_width - 0.5;
            let sy = (crop.y + v * crop.height) * frame_height - 0.5;
            let sample = sample_bilinear(camera, sx, sy);
            blend_pixel(
                output,
                x,
                y,
                [sample[0], sample[1], sample[2]],
                coverage * sample[3] as f64 / 255.0 * overlay.opacity(),
            );
        }
    }
}


fn blend_pixel(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 3], alpha: f64) {
    if x >= image.width() || y >= image.height() {
        return;
    }
    blend_slice(&mut image.get_pixel_mut(x, y).0, color, alpha);
}

fn blend_slice(pixel: &mut [u8], color: [u8; 3], alpha: f64) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    for channel in 0..3 {
        pixel[channel] = (pixel[channel] as f64
            + (color[channel] as f64 - pixel[channel] as f64) * alpha)
            .round() as u8;
    }
    pixel[3] = (pixel[3] as f64 + (255.0 - pixel[3] as f64) * alpha).round() as u8;
}

/// Straight-alpha `layer` over `image` (same size).
fn blend_layer(image: &mut RgbaImage, layer: &RgbaImage) {
    if layer.dimensions() != image.dimensions() {
        return;
    }
    for (target, source) in image.pixels_mut().zip(layer.pixels()) {
        let alpha = source[3] as f64 / 255.0;
        if alpha <= 0.0 {
            continue;
        }
        for channel in 0..3 {
            target[channel] = (target[channel] as f64
                + (source[channel] as f64 - target[channel] as f64) * alpha)
                .round() as u8;
        }
        target[3] = (target[3] as f64 + (255.0 - target[3] as f64) * alpha).round() as u8;
    }
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

/// Gaussian-like blur (three box passes) of an opaque RGBA image.
fn blur_image(image: &mut RgbaImage, sigma: f64) {
    if sigma <= 0.5 {
        return;
    }
    let width = image.width() as usize;
    let height = image.height() as usize;
    let factor = blur_downsample_factor(sigma);
    if factor == 1 {
        let mut channels: Vec<Vec<f32>> = (0..3)
            .map(|channel| {
                image
                    .pixels()
                    .map(|pixel| pixel[channel] as f32)
                    .collect::<Vec<f32>>()
            })
            .collect();
        for channel in channels.iter_mut() {
            blur_plane(channel, width, height, sigma);
        }
        for (index, pixel) in image.pixels_mut().enumerate() {
            pixel[0] = channels[0][index].round().clamp(0.0, 255.0) as u8;
            pixel[1] = channels[1][index].round().clamp(0.0, 255.0) as u8;
            pixel[2] = channels[2][index].round().clamp(0.0, 255.0) as u8;
        }
        return;
    }
    // Large sigma: average blocks straight out of the RGBA buffer, blur the
    // small planes, and write the upsampled result straight back.
    let small_width = width.div_ceil(factor);
    let small_height = height.div_ceil(factor);
    let mut small = vec![vec![0.0f32; small_width * small_height]; 3];
    let raw = image.as_raw();
    for sy in 0..small_height {
        for sx in 0..small_width {
            let x_end = ((sx + 1) * factor).min(width);
            let y_end = ((sy + 1) * factor).min(height);
            let mut sum = [0.0f32; 3];
            for y in sy * factor..y_end {
                for x in sx * factor..x_end {
                    let offset = (y * width + x) * 4;
                    sum[0] += raw[offset] as f32;
                    sum[1] += raw[offset + 1] as f32;
                    sum[2] += raw[offset + 2] as f32;
                }
            }
            let count = ((x_end - sx * factor) * (y_end - sy * factor)) as f32;
            for channel in 0..3 {
                small[channel][sy * small_width + sx] = sum[channel] / count;
            }
        }
    }
    for plane in small.iter_mut() {
        blur_plane(plane, small_width, small_height, sigma / factor as f64);
    }
    let raw: &mut [u8] = image.as_mut();
    for channel in 0..3 {
        upsample_bilinear(
            &small[channel],
            small_width,
            small_height,
            factor,
            width,
            height,
            |index, value| raw[index * 4 + channel] = value.round().clamp(0.0, 255.0) as u8,
        );
    }
}

fn box_radius_for(sigma: f64) -> usize {
    // Three box passes of width w approximate sigma² = 3 (w² − 1) / 12.
    let box_width = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    (((box_width - 1.0) / 2.0).round() as usize).max(1)
}

/// Deterministic film grain.
fn apply_noise(image: &mut RgbaImage, amount: f64) {
    let strength = (amount.clamp(0.0, 1.0) * 48.0) as f32;
    let width = image.width() as usize;
    let height = image.height() as usize;
    let raw: &mut [u8] = image.as_mut();
    for y in 0..height {
        let row = &mut raw[y * width * 4..(y + 1) * width * 4];
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let noise = (hash_noise(x as u64, y as u64) as f32 * 2.0 - 1.0) * strength;
            for channel in pixel.iter_mut().take(3) {
                *channel = (*channel as f32 + noise).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn hash_noise(x: u64, y: u64) -> f64 {
    let mut h = x.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ y.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 29;
    (h & 0xFFFF) as f64 / 65535.0
}

fn vignette_map(width: u32, height: u32, amount: f64) -> Vec<f32> {
    let mut map = vec![1.0f32; (width * height) as usize];
    let cx = width as f64 * 0.5;
    let cy = height as f64 * 0.5;
    let corner = (cx * cx + cy * cy).sqrt().max(1.0);
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let r = (dx * dx + dy * dy).sqrt() / corner;
            let t = ((r - 0.3) / 0.7).clamp(0.0, 1.0);
            let falloff = t * t * (3.0 - 2.0 * t);
            map[(y * width + x) as usize] = (1.0 - amount * 0.85 * falloff) as f32;
        }
    }
    map
}

fn render_watermark(watermark: &Watermark, width: u32, height: u32) -> Result<RgbaImage, String> {
    let font_size = height as f64 * (0.02 + watermark.size as f64 / 100.0 * 0.06);
    let margin = height as f64 * 0.035;
    let (x, anchor) = match watermark.position {
        WatermarkPosition::TopLeft | WatermarkPosition::BottomLeft => (margin, "start"),
        WatermarkPosition::TopRight | WatermarkPosition::BottomRight => {
            (width as f64 - margin, "end")
        }
    };
    let y = match watermark.position {
        WatermarkPosition::TopLeft | WatermarkPosition::TopRight => margin + font_size,
        WatermarkPosition::BottomLeft | WatermarkPosition::BottomRight => height as f64 - margin,
    };
    let opacity = watermark.opacity as f64 / 100.0;
    let text = xml_escape(watermark.text.trim());
    let stroke_opacity = opacity * 0.35;
    let stroke_width = font_size * 0.06;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><text x="{x:.2}" y="{y:.2}" text-anchor="{anchor}" font-family="sans-serif" font-weight="600" font-size="{font_size:.2}" fill="#ffffff" fill-opacity="{opacity:.3}" stroke="#000000" stroke-opacity="{stroke_opacity:.3}" stroke-width="{stroke_width:.2}" paint-order="stroke">{text}</text></svg>"##
    );
    render_svg_layer(&svg, width, height)
}

pub use lahza_annotations::fonts::shared_fontdb;
pub use lahza_annotations::svg::{render_svg_layer, xml_escape};

fn paint_shadow(
    image: &mut RgbaImage,
    projection: &MediaProjection,
    geometry: SceneGeometry,
    shadow: ShadowSpec,
    source: Option<&MediaShadow>,
) {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let mut mask = vec![0.0f32; width * height];
    let border = geometry.border_width;
    let margin = border * 2.0 + 2.0;
    let bounds = geometry.surface_bounds(projection);
    let x0 = (bounds.x - margin).floor().max(0.0) as usize;
    let y0 = (bounds.y - margin + shadow.offset_y).floor().max(0.0) as usize;
    let x1 = ((bounds.right() + margin).ceil().max(0.0) as usize).min(width);
    let y1 = ((bounds.bottom() + margin + shadow.offset_y).ceil().max(0.0) as usize).min(height);
    let top = geometry.title_top() - 0.3;
    for y in y0..y1 {
        for x in x0..x1 {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5 - shadow.offset_y;
            let (u, v) = projection.unproject(px, py);
            if !(-0.3..=1.3).contains(&u) || !(top..=1.3).contains(&v) {
                continue;
            }
            let pixel_size = projection.pixel_size_at(px, py);
            let distance = geometry.surface_distance(u, v);
            let outer = (0.5 - (distance - border) / pixel_size).clamp(0.0, 1.0);
            let coverage = if let Some(source) = source {
                let inner = (0.5 - distance / pixel_size).clamp(0.0, 1.0);
                let alpha = if v < 0.0 && geometry.title_height > 0.0 {
                    1.0
                } else {
                    source.sample(u, v)
                };
                // Keep the optional frame/border's own shadow, but let the
                // media cast a shadow only where this frame has visible pixels.
                (outer - inner).max(0.0) + inner * alpha
            } else {
                outer
            };
            mask[y * width + x] = coverage as f32;
        }
    }
    // A CSS blur radius corresponds to a Gaussian with sigma = radius / 2.
    blur_plane(&mut mask, width, height, shadow.blur_radius * 0.5);
    for y in 0..height {
        for x in 0..width {
            let alpha = mask[y * width + x] as f64 * shadow.opacity;
            if alpha > 0.001 {
                blend_pixel(image, x as u32, y as u32, shadow.color, alpha);
            }
        }
    }
}

/// Gaussian-like blur (three box passes) of one f32 plane. Large sigmas are
/// blurred on a downsampled copy and bilinearly upsampled back: the result is
/// visually identical (the blur removes everything finer than the
/// downsample) and the cost stops growing with sigma.
fn blur_plane(plane: &mut [f32], width: usize, height: usize, sigma: f64) {
    if sigma <= 0.5 || width == 0 || height == 0 {
        return;
    }
    let factor = blur_downsample_factor(sigma);
    if factor == 1 {
        let box_radius = box_radius_for(sigma);
        let mut scratch = vec![0.0f32; width * height];
        for _ in 0..3 {
            box_blur_horizontal(plane, &mut scratch, width, height, box_radius);
            box_blur_vertical(&scratch, plane, width, height, box_radius);
        }
        return;
    }
    let small_width = width.div_ceil(factor);
    let small_height = height.div_ceil(factor);
    let mut small = vec![0.0f32; small_width * small_height];
    for sy in 0..small_height {
        for sx in 0..small_width {
            let x_end = ((sx + 1) * factor).min(width);
            let y_end = ((sy + 1) * factor).min(height);
            let mut sum = 0.0f32;
            for y in sy * factor..y_end {
                sum += plane[y * width + sx * factor..y * width + x_end]
                    .iter()
                    .sum::<f32>();
            }
            let count = ((x_end - sx * factor) * (y_end - sy * factor)) as f32;
            small[sy * small_width + sx] = sum / count;
        }
    }
    blur_plane(&mut small, small_width, small_height, sigma / factor as f64);
    upsample_bilinear(
        &small,
        small_width,
        small_height,
        factor,
        width,
        height,
        |index, value| plane[index] = value,
    );
}

/// Downsampling applied before blurring with `sigma`, keeping the effective
/// sigma on the small image around 2.5 px or more.
fn blur_downsample_factor(sigma: f64) -> usize {
    ((sigma / 2.5).floor() as usize).clamp(1, 16)
}

/// Bilinearly stretches `small` (a `factor`-times downsampled plane) back to
/// `width` × `height`, handing each output pixel's index and value to `write`.
fn upsample_bilinear(
    small: &[f32],
    small_width: usize,
    small_height: usize,
    factor: usize,
    width: usize,
    height: usize,
    mut write: impl FnMut(usize, f32),
) {
    let scale = 1.0 / factor as f32;
    let max_x = (small_width - 1) as f32;
    let max_y = (small_height - 1) as f32;
    // Horizontal weights repeat per column, so compute them once.
    let columns: Vec<(usize, usize, f32)> = (0..width)
        .map(|x| {
            let fx = ((x as f32 + 0.5) * scale - 0.5).clamp(0.0, max_x);
            let x0 = fx.floor() as usize;
            (x0, (x0 + 1).min(small_width - 1), fx - x0 as f32)
        })
        .collect();
    for y in 0..height {
        let fy = ((y as f32 + 0.5) * scale - 0.5).clamp(0.0, max_y);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(small_height - 1);
        let ty = fy - y0 as f32;
        let row0 = &small[y0 * small_width..(y0 + 1) * small_width];
        let row1 = &small[y1 * small_width..(y1 + 1) * small_width];
        for (x, &(x0, x1, tx)) in columns.iter().enumerate() {
            let top = row0[x0] + (row0[x1] - row0[x0]) * tx;
            let bottom = row1[x0] + (row1[x1] - row1[x0]) * tx;
            write(y * width + x, top + (bottom - top) * ty);
        }
    }
}

/// Box blur along a row/column with clamp-to-edge sampling, so borders keep
/// their brightness instead of fading toward black.
fn box_blur_horizontal(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = (radius * 2 + 1) as f32;
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        let last = width - 1;
        let mut sum: f32 =
            (0..=radius).map(|dx| row[dx.min(last)]).sum::<f32>() + row[0] * radius as f32;
        for x in 0..width {
            dst[y * width + x] = sum / window;
            sum += row[(x + radius + 1).min(last)];
            sum -= row[x.saturating_sub(radius)];
        }
    }
}

fn box_blur_vertical(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = (radius * 2 + 1) as f32;
    let last = height - 1;
    for x in 0..width {
        let mut sum: f32 = (0..=radius)
            .map(|dy| src[dy.min(last) * width + x])
            .sum::<f32>()
            + src[x] * radius as f32;
        for y in 0..height {
            dst[y * width + x] = sum / window;
            sum += src[(y + radius + 1).min(last) * width + x];
            sum -= src[y.saturating_sub(radius) * width + x];
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

/// Captured cursors are drawn larger than life so 100% reads like an
/// editor cursor rather than a 24 px system pointer.
const CAPTURED_CURSOR_BOOST: f64 = 2.5;
/// Height of a styled cursor as a fraction of the media height: Cap's 60 px
/// on a 1080 px tall screen.
const ASSET_CURSOR_HEIGHT: f64 = 60.0 / 1080.0;
/// Motion smear length as a fraction of the distance the cursor travels in
/// one 60 Hz frame (Cap's default amount).
const CURSOR_SMEAR_AMOUNT: f64 = 1.0;
/// Longest smear, in cursor widths; only pathological jumps hit it.
const CURSOR_SMEAR_MAX_WIDTHS: f64 = 4.0;
const CURSOR_SMEAR_TAPS: usize = 21;
/// Styled cursor artwork is rasterized in steps of this many pixels and
/// drawn downscaled to the exact size.
const CURSOR_RASTER_STEP: f64 = 16.0;

/// Canvas-space trail for a cursor moving at `velocity` (normalized media
/// units per second) drawn `width` pixels wide.
fn cursor_smear(velocity: (f64, f64), scale: (f64, f64), width: f64, amount: f64) -> (f64, f64) {
    let amount = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 1.0 };
    let smear = (
        velocity.0 * scale.0 / 60.0 * CURSOR_SMEAR_AMOUNT * amount,
        velocity.1 * scale.1 / 60.0 * CURSOR_SMEAR_AMOUNT * amount,
    );
    let length = smear.0.hypot(smear.1);
    let limit = CURSOR_SMEAR_MAX_WIDTHS * width;
    if !length.is_finite() || length < 0.5 {
        (0.0, 0.0)
    } else if length > limit {
        (smear.0 * limit / length, smear.1 * limit / length)
    } else {
        smear
    }
}

/// Where a cursor bitmap lands on the canvas: hotspot position, drawn size
/// and rotation about the hotspot.
#[derive(Clone, Copy)]
struct BitmapPlacement {
    hotspot_x: f64,
    hotspot_y: f64,
    width: f64,
    height: f64,
    tilt_degrees: f64,
    /// Motion trail in canvas pixels: the cursor is smeared this far behind
    /// its position.
    smear: (f64, f64),
}

/// Blur premultiplied artwork once, before either CPU or GPU composition.
/// Transparent padding preserves the complete halo and the transformed hotspot.
/// Radius is in the same 60-pixel reference units as the normal cursor size,
/// so zoom/output scaling need not regenerate a sprite. The cache retains its
/// source Arc, preventing allocator address reuse from matching another image.
fn blurred_cursor(
    bitmap: std::sync::Arc<PointerBitmap>,
    mut placement: BitmapPlacement,
    radius: f64,
) -> (std::sync::Arc<PointerBitmap>, BitmapPlacement) {
    use std::{collections::VecDeque, sync::{Arc, Mutex, OnceLock}};
    struct Entry { source: Arc<PointerBitmap>, radius: u16, blurred: Arc<PointerBitmap> }
    static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    let sigma = if radius.is_finite() {
        (radius.clamp(0.0, 10.0) * bitmap.image.height() as f64 / 60.0).min(128.0)
    } else { 0.0 };
    let quantized = (sigma * 8.0).round() as u16;
    if quantized == 0 { return (bitmap, placement); }
    let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    let hit = {
        let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
        cache.iter().position(|entry| entry.radius == quantized && Arc::ptr_eq(&entry.source, &bitmap))
            .and_then(|index| cache.remove(index))
            .map(|entry| { let image = entry.blurred.clone(); cache.push_back(entry); image })
    };
    let blurred = hit.unwrap_or_else(|| {
        let sigma = quantized as f64 / 8.0;
        let padding = (sigma * 3.0).ceil() as u32 + 2;
        let (width, height) = (bitmap.image.width() + padding * 2, bitmap.image.height() + padding * 2);
        let mut image = RgbaImage::new(width, height);
        for (x, y, pixel) in bitmap.image.enumerate_pixels() {
            image.put_pixel(x + padding, y + padding, *pixel);
        }
        // Blur alpha and RGB together: straight-alpha filtering would create
        // dark fringes or brighten transparent pixels.
        for channel in 0..4 {
            let mut plane: Vec<f32> = image.pixels().map(|pixel| pixel[channel] as f32).collect();
            blur_plane(&mut plane, width as usize, height as usize, sigma);
            for (pixel, value) in image.pixels_mut().zip(plane) {
                pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
            }
        }
        let blurred = Arc::new(PointerBitmap {
            id: format!("{}:idle-blur:{quantized}", bitmap.id), image,
            anchor: NormalizedPoint {
                x: (bitmap.anchor.x * bitmap.image.width() as f64 + padding as f64) / width as f64,
                y: (bitmap.anchor.y * bitmap.image.height() as f64 + padding as f64) / height as f64,
            },
            reference_width: bitmap.reference_width * width as f64 / bitmap.image.width() as f64,
            reference_height: bitmap.reference_height * height as f64 / bitmap.image.height() as f64,
            shape: bitmap.shape,
        });
        let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
        // Both entry count and bytes are bounded for recordings with many
        // large cursor images. A single over-budget image is usable uncached.
        const BYTE_LIMIT: usize = 8 * 1024 * 1024;
        if blurred.image.as_raw().len() + bitmap.image.as_raw().len() <= BYTE_LIMIT {
            cache.push_back(Entry { source: bitmap.clone(), radius: quantized, blurred: blurred.clone() });
            while cache.len() > 16 || cache.iter().map(|entry|
                entry.source.image.as_raw().len() + entry.blurred.image.as_raw().len()).sum::<usize>() > BYTE_LIMIT {
                cache.pop_front();
            }
        }
        blurred
    });
    placement.width *= blurred.image.width() as f64 / bitmap.image.width() as f64;
    placement.height *= blurred.image.height() as f64 / bitmap.image.height() as f64;
    (blurred, placement)
}

/// Draws a captured cursor image with its hotspot at the pointer location,
/// resampling bilinearly in premultiplied space so scaled edges stay clean.
fn paint_cursor_bitmap(
    image: &mut RgbaImage,
    clip: Rect,
    bitmap: &PointerBitmap,
    placement: BitmapPlacement,
    opacity: f64,
    shadow_only: bool,
) {
    let source = &bitmap.image;
    let (source_width, source_height) = (source.width() as f64, source.height() as f64);
    let anchor_x = bitmap.anchor.x * placement.width;
    let anchor_y = bitmap.anchor.y * placement.height;
    let (sin, cos) = placement.tilt_degrees.to_radians().sin_cos();
    let corners = [
        (0.0, 0.0),
        (placement.width, 0.0),
        (0.0, placement.height),
        (placement.width, placement.height),
    ]
    .map(|(lx, ly)| {
        let (lx, ly) = (lx - anchor_x, ly - anchor_y);
        (
            placement.hotspot_x + lx * cos - ly * sin,
            placement.hotspot_y + lx * sin + ly * cos,
        )
    });
    // A pixel shows the sprite averaged along the trail behind it, so the
    // painted box extends opposite the motion.
    let (smear_x, smear_y) = placement.smear;
    let smear_length = smear_x.hypot(smear_y);
    let taps = if smear_length < 0.5 {
        1
    } else {
        CURSOR_SMEAR_TAPS
    };
    // The trail in the sprite's local (unrotated) frame.
    let local_smear = (
        smear_x * cos + smear_y * sin,
        -smear_x * sin + smear_y * cos,
    );
    let min_x = corners.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - 1.0 - smear_x.max(0.0);
    let max_x = corners
        .iter()
        .map(|p| p.0)
        .fold(f64::NEG_INFINITY, f64::max)
        + 1.0
        - smear_x.min(0.0);
    let min_y = corners.iter().map(|p| p.1).fold(f64::INFINITY, f64::min) - 1.0 - smear_y.max(0.0);
    let max_y = corners
        .iter()
        .map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max)
        + 1.0
        - smear_y.min(0.0);
    let x0 = min_x.floor().max(clip.x) as i64;
    let y0 = min_y.floor().max(clip.y) as i64;
    let x1 = max_x.ceil().min(clip.right()) as i64;
    let y1 = max_y.ceil().min(clip.bottom()) as i64;
    let scale_x = source_width / placement.width;
    let scale_y = source_height / placement.height;
    let sample = |sx: f64, sy: f64| -> [f64; 4] {
        // Bilinear tap in premultiplied space; outside the image is
        // transparent.
        let fx = sx - 0.5;
        let fy = sy - 0.5;
        let ix = fx.floor();
        let iy = fy.floor();
        let tx = fx - ix;
        let ty = fy - iy;
        let mut out = [0.0; 4];
        for (dx, dy, weight) in [
            (0.0, 0.0, (1.0 - tx) * (1.0 - ty)),
            (1.0, 0.0, tx * (1.0 - ty)),
            (0.0, 1.0, (1.0 - tx) * ty),
            (1.0, 1.0, tx * ty),
        ] {
            let px = ix + dx;
            let py = iy + dy;
            if weight <= 0.0 || px < 0.0 || py < 0.0 || px >= source_width || py >= source_height {
                continue;
            }
            let pixel = source.get_pixel(px as u32, py as u32);
            for channel in 0..4 {
                out[channel] += f64::from(pixel[channel]) * weight;
            }
        }
        out
    };
    for y in y0..y1 {
        for x in x0..x1 {
            // Map the pixel centre back into the unrotated artwork box.
            let dx = x as f64 + 0.5 - placement.hotspot_x;
            let dy = y as f64 + 0.5 - placement.hotspot_y;
            let lx = dx * cos + dy * sin + anchor_x;
            let ly = -dx * sin + dy * cos + anchor_y;
            let mut accumulated = [0.0; 4];
            for tap_index in 0..taps {
                let t = if taps == 1 {
                    0.0
                } else {
                    tap_index as f64 / (taps - 1) as f64
                };
                let tx = lx + local_smear.0 * t;
                let ty = ly + local_smear.1 * t;
                if tx < -1.0
                    || ty < -1.0
                    || tx > placement.width + 1.0
                    || ty > placement.height + 1.0
                {
                    continue;
                }
                let tapped = sample(tx * scale_x, ty * scale_y);
                for channel in 0..4 {
                    accumulated[channel] += tapped[channel];
                }
            }
            let [r, g, b, a] = accumulated.map(|value| value / taps as f64);
            let coverage = a / 255.0;
            if coverage <= 0.002 {
                continue;
            }
            if shadow_only {
                blend_pixel(image, x as u32, y as u32, [0, 0, 0], coverage * opacity);
                continue;
            }
            let color = [
                (r / coverage).round().clamp(0.0, 255.0) as u8,
                (g / coverage).round().clamp(0.0, 255.0) as u8,
                (b / coverage).round().clamp(0.0, 255.0) as u8,
            ];
            blend_pixel(image, x as u32, y as u32, color, coverage * opacity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewport::Tilt;
    use crate::pointer_timeline::PointerPressFrame;

    #[test]
    #[ignore = "manual export compositor benchmark"]
    fn benchmark_export_camera() {
        let source = checker(1920, 1080);
        let camera = checker(640, 480);
        let style = SceneStyle::default();
        let compositor = SceneCompositor::new(&style, 1920, 1080, 1920, 1080).unwrap();
        let mut output = source.clone();
        compositor.paint_camera(&mut output, &camera, 1.0);
        let start = std::time::Instant::now();
        for _ in 0..60 {
            let pixels: &mut [u8] = output.as_mut();
            pixels.copy_from_slice(source.as_raw());
            compositor.paint_camera(&mut output, &camera, 1.0);
            std::hint::black_box(&output);
        }
        eprintln!("1080p camera: {:.2} ms/frame", start.elapsed().as_secs_f64() * 1000.0 / 60.0);
        let start = std::time::Instant::now();
        for _ in 0..60 {
            std::hint::black_box(compositor.compose(FrameInput {
                source: &source, overlay: None, viewport: ViewportFrame::default(),
                pointer: None, camera: Some(&camera),
            }));
        }
        eprintln!("1080p composition: {:.2} ms/frame", start.elapsed().as_secs_f64() * 1000.0 / 60.0);
        // Canvas captions are rendered at an 800px reference height. Include
        // the layer even when transparent to catch work outside its timing.
        let mut caption = RgbaImage::new(1422, 800);
        for visible in [false, true] {
            if visible {
                for y in 600..640 {
                    for x in 400..900 {
                        caption.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                    }
                }
            }
            let start = std::time::Instant::now();
            for _ in 0..60 {
                std::hint::black_box(compositor.compose_layers(FrameInput {
                    source: &source, overlay: None, viewport: ViewportFrame::default(),
                    pointer: None, camera: Some(&camera),
                }, true, Some(&caption)));
            }
            eprintln!("1080p composition + canvas caption (visible={visible}): {:.2} ms/frame",
                start.elapsed().as_secs_f64() * 1000.0 / 60.0);
        }
    }

    #[test]
    fn cached_canvas_overlay_matches_resampling_after_changes_and_rebuilds() {
        let style = flat_style(0x345678);
        let compositor = SceneCompositor::new(&style, 96, 64, 48, 32).unwrap();
        let source = checker(48, 32);
        let mut caption = RgbaImage::new(48, 32);
        let check = |compositor: &SceneCompositor, caption: &RgbaImage| {
            let mut expected = compose(compositor, &source);
            let resized = image::imageops::resize(caption, expected.width(), expected.height(),
                image::imageops::FilterType::Lanczos3);
            blend_layer(&mut expected, &resized);
            let actual = compositor.compose_layers(FrameInput {
                source: &source, overlay: None, viewport: Default::default(),
                pointer: None, camera: None,
            }, true, Some(caption));
            assert_eq!(actual, expected);
        };
        check(&compositor, &caption);
        caption.put_pixel(12, 20, Rgba([255, 0, 0, 180]));
        check(&compositor, &caption);
        check(&compositor, &caption);
        // Mutate the same allocation, then change its dimensions, then seek
        // back to blank. None of these may reuse stale caption pixels.
        caption.put_pixel(12, 20, Rgba([0, 255, 0, 90]));
        check(&compositor, &caption);
        check(&compositor, &checker(32, 48));
        check(&compositor, &RgbaImage::new(48, 32));
        let rebuilt = compositor.rebuild(&style, 128, 96, 48, 32).unwrap();
        check(&rebuilt, &caption);
    }

    #[test]
    fn cached_camera_shadow_matches_uncached_and_rebuilds_for_new_geometry() {
        let camera = checker(96, 72);
        for shape in [CameraShape::Circle, CameraShape::Squircle, CameraShape::Square] {
            let mut style = SceneStyle::default();
            style.camera.shape = shape;
            let compositor = SceneCompositor::new(&style, 320, 180, 320, 180).unwrap();
            assert!(compositor.camera_shadow.borrow().is_none());
            for background in [Rgba([90, 160, 230, 255]), Rgba([230, 50, 100, 100])] {
                let mut cached = RgbaImage::from_pixel(320, 180, background);
                let mut reference = cached.clone();
                compositor.paint_camera(&mut cached, &camera, 1.0);
                paint_camera_overlay(&mut reference, &camera, style.camera, style.camera.rect(320.0, 180.0));
                assert_eq!(cached, reference);
            }
            assert!(compositor.camera_shadow.borrow().is_some());
            style.camera.position = WatermarkPosition::TopLeft;
            style.camera.size = 40;
            let rebuilt = compositor.rebuild(&style, 400, 300, 320, 180).unwrap();
            assert!(rebuilt.camera_shadow.borrow().is_none());
            let mut cached = RgbaImage::from_pixel(400, 300, Rgba([200, 200, 200, 255]));
            let mut reference = cached.clone();
            rebuilt.paint_camera(&mut cached, &camera, 1.0);
            paint_camera_overlay(&mut reference, &camera, style.camera, style.camera.rect(400.0, 300.0));
            assert_eq!(cached, reference);
        }
    }

    #[test]
    fn responsive_webcam_shadow_and_crop_match_uncached_composition() {
        let camera = checker(96, 72);
        for (width, height) in [(320, 180), (180, 320), (240, 240)] {
            let mut style = SceneStyle::default();
            style.camera.shape = CameraShape::Squircle;
            style.camera.aspect = CameraAspect::Wide;
            style.camera.zoomed_size = Some(12);
            style.camera.crop_center = (0.2, 0.8);
            style.camera.crop_zoom = 150;
            let compositor = SceneCompositor::new(&style, width, height, 320, 180).unwrap();
            for zoom in [1.0, 1.25, 1.5, 1.0] {
                let mut cached = RgbaImage::from_pixel(width, height, Rgba([90, 160, 230, 255]));
                let mut reference = cached.clone();
                let rect = style.camera.rect_for_frame(width as f64, height as f64, 96.0 / 72.0, zoom);
                compositor.paint_camera(&mut cached, &camera, zoom);
                paint_camera_overlay(&mut reference, &camera, style.camera, rect);
                assert_eq!(cached, reference, "{width}x{height}, zoom={zoom}");
            }
        }
    }

    #[test]
    fn split_camera_layout_places_the_screen_inside_its_reserved_area() {
        for (width, height) in [(320.0, 180.0), (180.0, 320.0), (240.0, 240.0)] {
            for flip in [false, true] {
                let mut style = SceneStyle::default();
                style.camera.layout = CameraLayout::Split;
                style.camera.split_flip = flip;
                let geometry = SceneGeometry::layout(width, height, 1920.0, 1080.0, &style);
                let area = style.camera.screen_area(width, height);
                assert!(geometry.media.x >= area.x && geometry.media.y >= area.y);
                assert!(geometry.media.right() <= area.right() + 1e-9);
                assert!(geometry.media.bottom() <= area.bottom() + 1e-9);
            }
        }
    }

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

    fn flat_style(background: u32) -> SceneStyle {
        SceneStyle {
            background: SceneBackground::Solid(background),
            padding: 0,
            corners: 0,
            shadow: 0,
            border: false,
            aspect: Some(1.0),
            ..SceneStyle::default()
        }
    }

    fn compose(compositor: &SceneCompositor, source: &RgbaImage) -> RgbaImage {
        compositor.compose(FrameInput {
            source,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: None,
        })
    }

    #[test]
    fn shadow_color_defaults_for_old_projects_and_round_trips() {
        let style = SceneStyle {
            shadow_color: 0x8c4ce8,
            ..SceneStyle::default()
        };
        let mut json = serde_json::to_value(&style).unwrap();
        assert_eq!(
            serde_json::from_value::<SceneStyle>(json.clone()).unwrap(),
            style
        );
        json.as_object_mut().unwrap().remove("shadowColor");
        assert_eq!(
            serde_json::from_value::<SceneStyle>(json)
                .unwrap()
                .shadow_color,
            0x000000
        );
    }

    #[test]
    fn all_shadow_styles_render_the_selected_color_and_zero_disables_it() {
        let source = RgbaImage::from_pixel(80, 80, Rgba([255, 255, 255, 255]));
        for shadow_style in 0..4 {
            let mut style = SceneStyle {
                padding: 40,
                shadow: 60,
                shadow_style,
                ..flat_style(0xffffff)
            };
            let black = compose(
                &SceneCompositor::new(&style, 200, 200, 80, 80).unwrap(),
                &source,
            );
            style.shadow_color = 0xff0000;
            let red = compose(
                &SceneCompositor::new(&style, 200, 200, 80, 80).unwrap(),
                &source,
            );
            assert!(black
                .pixels()
                .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2]));
            assert!(
                red.pixels()
                    .any(|pixel| pixel[0] > pixel[1] && pixel[1] == pixel[2]),
                "style {shadow_style}"
            );
            assert_eq!(red.get_pixel(100, 100), source.get_pixel(40, 40));
            assert_ne!(black, red);
            style.shadow = 0;
            let disabled = compose(
                &SceneCompositor::new(&style, 200, 200, 80, 80).unwrap(),
                &source,
            );
            assert!(disabled
                .pixels()
                .all(|pixel| pixel.0 == [255, 255, 255, 255]));
        }
    }

    #[test]
    fn window_frame_draws_a_title_bar_above_the_media() {
        let source = RgbaImage::from_pixel(80, 80, Rgba([10, 200, 30, 255]));
        let style = SceneStyle {
            window_frame: WindowFrame::Light,
            ..flat_style(0x000000)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 80, 80).unwrap();
        let geometry = compositor.geometry();
        assert!(geometry.title_height > 0.0);
        assert!((geometry.card().y - (geometry.media.y - geometry.title_height)).abs() < 1e-9);
        let output = compose(&compositor, &source);
        let bar_y = (geometry.media.y - geometry.title_height * 0.5) as u32;
        let media_y = (geometry.media.y + 2.0) as u32;
        let x = (geometry.media.x + geometry.media.width * 0.8) as u32;
        assert_eq!(output.get_pixel(x, bar_y).0[..3], [0xe9, 0xe9, 0xeb]);
        assert_eq!(output.get_pixel(x, media_y).0[..3], [10, 200, 30]);
        let light_x = (geometry.media.x + geometry.title_height * 0.7) as u32;
        assert_eq!(output.get_pixel(light_x, bar_y).0[..3], [0xff, 0x5f, 0x57]);
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
        assert!(large.media.right() <= 1200.0 && large.media.bottom() <= 1200.0);
    }

    #[test]
    fn export_canvas_follows_aspect_and_stays_even() {
        let style = SceneStyle {
            aspect: Some(16.0 / 9.0),
            ..SceneStyle::default()
        };
        assert_eq!(style.export_canvas_size(1000, 1000, 1080), (1920, 1080));
        let portrait = SceneStyle {
            aspect: Some(9.0 / 16.0),
            ..SceneStyle::default()
        };
        assert_eq!(portrait.export_canvas_size(1920, 1080, 1920), (1080, 1920));
        assert_eq!(portrait.export_canvas_size(1920, 1080, 1080), (608, 1080));
        let auto = SceneStyle {
            aspect: None,
            ..SceneStyle::default()
        };
        assert_eq!(auto.export_canvas_size(1001, 1000, 719), (718, 718));
    }

    #[test]
    fn style_round_trips_through_json_with_defaults() {
        let style = SceneStyle {
            background: SceneBackground::Gradient {
                colors: [1, 2, 3],
                angle_degrees: 45.0,
            },
            transform: SceneTransform {
                rotation_y: 12.0,
                ..SceneTransform::IDENTITY
            },
            watermark: Some(Watermark {
                text: "demo".into(),
                ..Watermark::default()
            }),
            ..SceneStyle::default()
        };
        let json = serde_json::to_string(&style).unwrap();
        let parsed: SceneStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, style);
        // Older documents without the new fields still load.
        let minimal: SceneStyle = serde_json::from_str(r#"{"padding":5}"#).unwrap();
        assert_eq!(minimal.padding, 5);
        assert_eq!(minimal.transform, SceneTransform::IDENTITY);
    }

    #[test]
    fn compositor_paints_background_outside_and_media_inside() {
        let style = SceneStyle {
            padding: 20,
            ..flat_style(0x102030)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 100, 100).unwrap();
        let output = compose(&compositor, &checker(100, 100));
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
    #[ignore = "set LAHZA_SHADOW_RECORDING to a resized window recording for visual verification"]
    fn render_resized_window_shadow_preview() {
        let root = std::path::PathBuf::from(std::env::var_os("LAHZA_SHADOW_RECORDING").unwrap());
        let edit: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("edit.draft.json")).unwrap()).unwrap();
        let style: SceneStyle = serde_json::from_value(edit["scene"].clone()).unwrap();
        let path = root.join("screen.mkv");
        let frame = super::super::video::decode_frame(&path, 0.2, 1920, 1080).unwrap();
        let compositor =
            SceneCompositor::new(&style, 1000, 700, frame.width, frame.height).unwrap();
        let source = RgbaImage::from_raw(frame.width, frame.height, frame.rgba).unwrap();
        compose(&compositor, &source);
        let time = std::env::var("LAHZA_SHADOW_TIME")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(12.0);
        let frame = super::super::video::decode_frame(&path, time, 1920, 1080).unwrap();
        eprintln!("presented window at {time}s: {}x{}", frame.width, frame.height);
        let source = RgbaImage::from_raw(frame.width, frame.height, frame.rgba).unwrap();
        let compositor = compositor.rebuild(&style, 1000, 700, source.width(), source.height()).unwrap();
        let output = compose(&compositor, &source);
        let media = compositor.geometry.media;
        assert!((media.width / media.height - source.width() as f64 / source.height() as f64).abs() < 1e-6);
        let destination = std::env::var_os("LAHZA_SHADOW_PREVIEW").unwrap();
        output.save(destination).unwrap();
    }

    #[test]
    fn window_shadow_follows_resizes_and_invalidates_the_cached_silhouette() {
        let style = SceneStyle {
            shadow: 40,
            ..flat_style(0xffffff)
        };
        let compositor = SceneCompositor::new(&style, 400, 300, 400, 300).unwrap();
        let window = |left, right| {
            let mut image = RgbaImage::new(400, 300);
            for y in 40..260 {
                for x in left..right {
                    image.put_pixel(x, y, Rgba([20, 60, 100, 255]));
                }
            }
            image
        };
        let wide = window(40, 360);
        let narrow = window(160, 240);
        compose(&compositor, &wide);
        let resized = compose(&compositor, &narrow);
        let fresh = SceneCompositor::new(&style, 400, 300, 400, 300).unwrap();
        assert_eq!(
            resized,
            compose(&fresh, &narrow),
            "resizing must refresh the cached shadow"
        );
        assert_eq!(
            resized.get_pixel(80, 150).0,
            [255; 4],
            "old window footprint must reveal the background"
        );
        assert!(
            resized.get_pixel(155, 150)[0] < 255,
            "the resized window must still cast a shadow"
        );
        let blank = compose(&compositor, &RgbaImage::new(400, 300));
        assert!(
            blank.pixels().all(|pixel| pixel.0 == [255; 4]),
            "invisible media must not cast a rectangular shadow"
        );
        assert_eq!(
            compose(&compositor, &wide),
            compose(&fresh, &wide),
            "growing the window must also refresh its shadow"
        );
    }

    #[test]
    fn window_shadow_tracks_viewport_crops_and_preserves_opaque_card_shadows() {
        let style = SceneStyle {
            shadow: 40,
            ..flat_style(0xffffff)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let mut source = RgbaImage::new(200, 200);
        for y in 50..150 {
            for x in 80..120 {
                source.put_pixel(x, y, Rgba([20, 60, 100, 255]));
            }
        }
        compose(&compositor, &source);
        let viewport = ViewportFrame {
            magnification: 2.0,
            ..ViewportFrame::default()
        };
        let render = |compositor: &SceneCompositor| {
            compositor.compose(FrameInput {
                source: &source,
                overlay: None,
                viewport,
                pointer: None,
                camera: None,
            })
        };
        let zoomed = render(&compositor);
        let fresh = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        assert_eq!(
            zoomed,
            render(&fresh),
            "zoom must refresh the alpha shadow in canvas space"
        );
        let opaque = RgbaImage::from_pixel(200, 200, Rgba([20, 60, 100, 255]));
        compose(&compositor, &opaque);
        assert!(compositor
            .card
            .borrow()
            .as_ref()
            .unwrap()
            .shadow_source
            .is_none());
        assert_eq!(compose(&compositor, &opaque), compose(&fresh, &opaque));
    }

    #[test]
    fn transparent_window_margins_reveal_the_scene_background() {
        let compositor = SceneCompositor::new(&flat_style(0x4080c0), 100, 100, 100, 100).unwrap();
        let mut source = RgbaImage::new(100, 100);
        for y in 20..80 {
            for x in 20..80 {
                source.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        source.put_pixel(10, 50, Rgba([0, 0, 0, 128]));
        let output = compose(&compositor, &source);
        assert_eq!(output.get_pixel(5, 50).0, [64, 128, 192, 255]);
        assert_eq!(output.get_pixel(50, 50).0, [0, 0, 0, 255]);
        let shadow = output.get_pixel(10, 50);
        assert!(shadow[0] > 0 && shadow[0] < 64);
    }

    #[test]
    fn viewport_zoom_crops_the_media() {
        let compositor = SceneCompositor::new(&flat_style(0), 100, 100, 100, 100).unwrap();
        let viewport = ViewportFrame {
            magnification: 2.0,
            anchor: NormalizedPoint { x: 0.25, y: 0.25 },
            tilt: Tilt::default(),
            ..ViewportFrame::default()
        };
        let output = compositor.compose(FrameInput {
            source: &checker(100, 100),
            overlay: None,
            viewport,
            pointer: None,
            camera: None,
        });
        assert_eq!(output.get_pixel(5, 5).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(94, 94).0, [255, 0, 0, 255]);
    }

    #[test]
    fn rounded_corners_reveal_the_background() {
        let style = SceneStyle {
            corners: 100,
            ..flat_style(0xffffff)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let output = compose(&compositor, &checker(200, 200));
        assert_eq!(output.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(output.get_pixel(100, 5).0, [0, 255, 0, 255]);
    }

    #[test]
    fn video_crop_matches_physical_crop_for_pixels_layout_and_decoration() {
        let mut source = RgbaImage::from_pixel(128, 128, Rgba([255, 0, 0, 255]));
        let crop = crate::viewport::CropRect { x: 0.25, y: 0.125, width: 0.5, height: 0.75 };
        let body = RgbaImage::from_pixel(64, 96, Rgba([0, 100, 240, 255]));
        image::imageops::overlay(&mut source, &body, 32, 16);
        for decorated in [false, true] {
            let style = SceneStyle {
                source_crop: crop,
                aspect: None,
                padding: if decorated { 20 } else { 0 },
                corners: if decorated { 80 } else { 0 },
                border: decorated,
                shadow: if decorated { 40 } else { 0 },
                ..flat_style(0xffffff)
            };
            assert_eq!(style.export_canvas_size(128, 128, 192), (128, 192));
            let cropped = SceneCompositor::new(&style, 128, 192, 128, 128).unwrap();
            let reference = SceneCompositor::new(&SceneStyle { source_crop: crate::viewport::CropRect::UNIT, ..style }, 128, 192, 64, 96).unwrap();
            assert_eq!(cropped.geometry(), reference.geometry());
            let actual = compose(&cropped, &source);
            let expected = compose(&reference, &body);
            let largest = actual.as_raw().iter().zip(expected.as_raw()).map(|(a,b)| a.abs_diff(*b)).max().unwrap();
            assert!(largest <= 1, "outside-crop pixels leaked into the frame: {largest}");
        }
    }

    #[test]
    fn video_crop_persists_and_old_scene_settings_keep_the_full_frame() {
        let style = SceneStyle { source_crop: crate::viewport::CropRect { x: 0.25, y: 0.0, width: 0.5, height: 1.0 }, ..Default::default() };
        let decoded: SceneStyle = serde_json::from_str(&serde_json::to_string(&style).unwrap()).unwrap();
        assert_eq!(decoded.source_crop, style.source_crop);
        assert_eq!(serde_json::from_str::<SceneStyle>("{}").unwrap().source_crop, crate::viewport::CropRect::UNIT);
    }

    #[test]
    fn window_corners_border_and_shadow_use_body_bounds() {
        let body = checker(200, 200);
        let mut storage = RgbaImage::new(240, 240);
        for y in 10..230 {
            for x in 10..230 {
                storage.put_pixel(x, y, Rgba([0, 0, 0, 28]));
            }
        }
        image::imageops::overlay(&mut storage, &body, 20, 20);
        let presented = super::super::video::present_frame(
            super::super::video::DecodedFrame {
                time: 0.0,
                width: 240,
                height: 240,
                rgba: storage.into_raw(),
            },
            true,
        );
        let source = RgbaImage::from_raw(presented.width, presented.height, presented.rgba).unwrap();
        for shadow in [0, 40] {
            let style = SceneStyle {
                padding: 20,
                corners: 100,
                border: true,
                border_thickness: 20,
                shadow,
                ..flat_style(0xffffff)
            };
            let actual = SceneCompositor::new(&style, 300, 300, source.width(), source.height()).unwrap();
            let expected = SceneCompositor::new(&style, 300, 300, body.width(), body.height()).unwrap();
            assert_eq!(compose(&actual, &source), compose(&expected, &body));
        }
    }

    #[test]
    fn scale_and_position_move_the_media() {
        let style = SceneStyle {
            transform: SceneTransform {
                scale: 0.5,
                position_x: 0.5,
                ..SceneTransform::IDENTITY
            },
            ..flat_style(0x000000)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let output = compose(&compositor, &checker(200, 200));
        // Half-size media centred at x = 150: canvas 100..200 horizontally.
        assert_eq!(output.get_pixel(20, 100).0, [0, 0, 0, 255]);
        assert_eq!(output.get_pixel(110, 60).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(190, 140).0, [255, 255, 0, 255]);
        let projection = compositor.projection(ViewportFrame::default());
        assert!(projection.contains(150.0, 100.0));
        assert!(!projection.contains(50.0, 100.0));
    }

    #[test]
    fn rotation_projects_a_perspective_quad_and_round_trips() {
        let style = SceneStyle {
            transform: SceneTransform {
                rotation_y: 35.0,
                rotation_x: 10.0,
                perspective: 0.8,
                ..SceneTransform::IDENTITY
            },
            ..flat_style(0x000000)
        };
        let compositor = SceneCompositor::new(&style, 300, 200, 300, 200).unwrap();
        let projection = compositor.projection(ViewportFrame::default());
        // The rotated side recedes: the right edge is shorter than the left.
        let left_height = (projection.quad[3].1 - projection.quad[0].1).abs();
        let right_height = (projection.quad[2].1 - projection.quad[1].1).abs();
        assert!(right_height < left_height, "{right_height} {left_height}");
        for (u, v) in [(0.1, 0.2), (0.5, 0.5), (0.9, 0.8)] {
            let (x, y) = projection.project(u, v);
            let (back_u, back_v) = projection.unproject(x, y);
            assert!((back_u - u).abs() < 1e-6 && (back_v - v).abs() < 1e-6);
        }
        let output = compose(&compositor, &checker(300, 200));
        let (x, y) = projection.project(0.25, 0.25);
        assert_eq!(output.get_pixel(x as u32, y as u32).0, [255, 0, 0, 255]);
        let (x, y) = projection.project(0.75, 0.75);
        assert_eq!(output.get_pixel(x as u32, y as u32).0, [255, 255, 0, 255]);
        // Outside the quad (right of the receding edge) the background shows.
        assert!(projection.bounds.right() < 296.0, "{:?}", projection.bounds);
        assert_eq!(output.get_pixel(296, 100).0, [0, 0, 0, 255]);
    }

    #[test]
    fn canvas_overlay_survives_hidden_and_transformed_media() {
        let compositor = SceneCompositor::new(&flat_style(0), 100, 100, 100, 100).unwrap();
        let source = checker(100, 100);
        let mut overlay = RgbaImage::new(100, 100);
        for y in 20..40 {
            for x in 5..25 { overlay.put_pixel(x, y, Rgba([255, 255, 255, 255])); }
        }
        let render = |visible, transform| compositor.compose_layers(FrameInput {
            source: &source,
            overlay: None,
            viewport: ViewportFrame { transform, ..ViewportFrame::default() },
            pointer: None,
            camera: None,
        }, visible, Some(&overlay));
        let hidden = render(false, SceneTransform::IDENTITY);
        assert_eq!(hidden.get_pixel(10, 30).0, [255, 255, 255, 255]);
        assert_eq!(hidden.get_pixel(90, 90).0, [0, 0, 0, 255]);
        let moved = render(true, SceneTransform { position_x: 2.0, rotation_z: 30.0, ..SceneTransform::IDENTITY });
        assert_eq!(moved.get_pixel(10, 30), hidden.get_pixel(10, 30));
    }

    #[test]
    fn card_can_slide_fully_offscreen_and_back_from_each_edge() {
        let compositor = SceneCompositor::new(&flat_style(0), 100, 100, 100, 100).unwrap();
        let source = checker(100, 100);
        for (x, y) in [(6.0, 0.0), (-6.0, 0.0), (0.0, 6.0), (0.0, -6.0)] {
            let motion = crate::viewport::TransformMotion {
                keep_end_state: false,
                start: SceneTransform {
                    scale: SceneTransform::MAX_SCALE,
                    position_x: x,
                    position_y: y,
                    ..SceneTransform::IDENTITY
                },
                end: SceneTransform::IDENTITY,
            };
            let render = |progress| compositor.compose(FrameInput {
                source: &source,
                overlay: None,
                viewport: ViewportFrame {
                    transform: motion.at(progress),
                    ..ViewportFrame::default()
                },
                pointer: None,
                camera: None,
            });
            assert!(render(0.0).pixels().all(|pixel| pixel.0 == [0, 0, 0, 255]));
            assert!(render(0.75).pixels().any(|pixel| pixel.0 != [0, 0, 0, 255]));
            assert_eq!(render(1.0), compose(&compositor, &source));
        }
    }

    #[test]
    fn animated_card_transform_moves_the_full_media_and_restores_cached_frame() {
        let compositor = SceneCompositor::new(&flat_style(0), 100, 100, 100, 100).unwrap();
        let source = checker(100, 100);
        let original = compose(&compositor, &source);
        let viewport = ViewportFrame {
            transform: SceneTransform {
                scale: 0.5,
                position_x: 0.2,
                ..SceneTransform::IDENTITY
            },
            ..ViewportFrame::default()
        };
        let output = compositor.compose(FrameInput {
            source: &source,
            overlay: None,
            viewport,
            pointer: None,
            camera: None,
        });
        // The scaled card spans x=35..85 and y=25..75. Both opposite
        // source corners survive; unlike camera zoom, scaling never crops.
        assert_eq!(output.get_pixel(40, 30).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(80, 70).0, [255, 255, 0, 255]);
        assert_ne!(output.as_raw(), original.as_raw());
        let (x, y) = compositor.projection(viewport).project(0.1, 0.1);
        assert!((x - 40.0).abs() < 1e-6 && (y - 30.0).abs() < 1e-6);
        assert_eq!(compose(&compositor, &source).as_raw(), original.as_raw());
    }

    #[test]
    fn motion_transform_combines_with_authored_layout_and_tilt() {
        let authored = SceneTransform {
            scale: 0.8, position_x: 0.1, rotation_z: 10.0,
            ..SceneTransform::IDENTITY
        };
        let result = authored.with_motion(ViewportFrame {
            transform: SceneTransform {
                scale: 1.5, position_x: 0.2, rotation_z: 20.0,
                ..SceneTransform::IDENTITY
            },
            tilt: Tilt { x: 5.0, y: -8.0, z: 3.0 },
            ..ViewportFrame::default()
        });
        assert!((result.scale - 1.2).abs() < 1e-9);
        assert!((result.position_x - 0.3).abs() < 1e-9);
        assert_eq!(result.rotation_x, 5.0);
        assert_eq!(result.rotation_y, -8.0);
        assert_eq!(result.rotation_z, 33.0);
    }

    #[test]
    fn tilt_is_applied_per_frame_without_disturbing_the_cache() {
        let compositor = SceneCompositor::new(&flat_style(0), 120, 120, 120, 120).unwrap();
        let flat = compose(&compositor, &checker(120, 120));
        let tilted = compositor.compose(FrameInput {
            source: &checker(120, 120),
            overlay: None,
            viewport: ViewportFrame {
                tilt: Tilt {
                    x: 0.0,
                    y: 40.0,
                    z: 0.0,
                },
                ..ViewportFrame::default()
            },
            pointer: None,
            camera: None,
        });
        assert_ne!(flat.as_raw(), tilted.as_raw());
        let flat_again = compose(&compositor, &checker(120, 120));
        assert_eq!(flat.as_raw(), flat_again.as_raw());
    }

    #[test]
    fn gradient_shadow_border_effects_render() {
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
            background_blur: 40,
            background_noise: 30,
            vignette: 50,
            watermark: Some(Watermark {
                text: "Lahza".into(),
                ..Watermark::default()
            }),
            ..SceneStyle::default()
        };
        let compositor = SceneCompositor::new(&style, 320, 180, 64, 36).unwrap();
        let output = compose(&compositor, &checker(64, 36));
        let geometry = compositor.geometry();
        let border_x = (geometry.media.x - geometry.border_width * 0.5) as u32;
        let border_y = (geometry.media.y + geometry.media.height * 0.5) as u32;
        let border = output.get_pixel(border_x, border_y).0;
        assert!(
            border[0] > 180 && border[1] > 130 && border[2] < 120,
            "{border:?}"
        );
        // Vignette darkens the corner more than the edge midpoint.
        let corner = output.get_pixel(0, 0).0;
        let edge = output.get_pixel(160, 0).0;
        let sum = |pixel: [u8; 4]| pixel[0] as u32 + pixel[1] as u32 + pixel[2] as u32;
        assert!(sum(corner) < sum(edge));
    }

    #[test]
    fn captured_cursor_bitmap_paints_with_hotspot_on_the_pointer() {
        use crate::model::PointerArtwork;
        use crate::pointer_timeline::PointerBitmap;
        use base64::Engine;
        // 8x8 red square, hotspot at the centre.
        let square = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        square.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let artwork = PointerArtwork {
            artwork_id: "square".into(),
            image_data_base64: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
            anchor_point: NormalizedPoint { x: 0.5, y: 0.5 },
            reference_width: 0.1,
            reference_height: 0.1,
            shape: None,
        };
        let bitmap = std::sync::Arc::new(PointerBitmap::decode(&artwork).unwrap());
        let style = SceneStyle {
            pointer: PointerStyle {
                shadow: false,
                click_effects: false,
                ..PointerStyle::default()
            },
            ..flat_style(0)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let white = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.5, y: 0.5 },
                artwork_id: Some("square".into()),
                bitmap: Some(bitmap),
                magnification: 1.0,
                tilt_degrees: 0.0,
                opacity: 1.0,
                blur_radius: 0.0,
                velocity: (0.0, 0.0),
                press: None,
            },
        };
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        });
        let geometry = compositor.geometry();
        let centre_x = (geometry.media.x + geometry.media.width * 0.5) as u32;
        let centre_y = (geometry.media.y + geometry.media.height * 0.5) as u32;
        // The square is centred on the pointer: red at the centre, and the
        // half-size is 0.05 * media * boost, so 2.5x that is white again.
        // Bilinear upscaling feathers each edge over one source pixel.
        assert_eq!(output.get_pixel(centre_x, centre_y).0, [255, 0, 0, 255]);
        let half = (0.05 * geometry.media.width * CAPTURED_CURSOR_BOOST) as u32;
        assert_eq!(
            output.get_pixel(centre_x + half - 4, centre_y).0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            output.get_pixel(centre_x - half + 4, centre_y).0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            output.get_pixel(centre_x + half + 3, centre_y).0,
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn overlay_blends_in_media_space() {
        let compositor = SceneCompositor::new(&flat_style(0), 100, 100, 100, 100).unwrap();
        let white = RgbaImage::from_pixel(100, 100, Rgba([255, 255, 255, 255]));
        let mut overlay = RgbaImage::new(100, 100);
        for y in 40..60 {
            for x in 40..60 {
                overlay.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: Some(&overlay),
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: None,
        });
        assert_eq!(output.get_pixel(50, 50).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(10, 10).0, [255, 255, 255, 255]);
    }

    #[test]
    fn launcher_camera_crop_matches_recording_for_every_shape_and_mirror_setting() {
        let camera = RgbaImage::from_fn(320, 180, |x, y| {
            Rgba([(x % 256) as u8, y as u8, 80, 255])
        });
        for shape in CameraShape::ALL {
            for mirror in [false, true] {
                let overlay = CameraOverlay {
                    position: WatermarkPosition::TopLeft,
                    size: 44,
                    margin: 3,
                    shape,
                    mirror,
                    shadow: false,
                    ..CameraOverlay::default()
                };
                let style = SceneStyle { camera: overlay, ..flat_style(0xfbfbfb) };
                let compositor = SceneCompositor::new(&style, 400, 400, 400, 400).unwrap();
                let source = RgbaImage::from_pixel(400, 400, Rgba([251, 251, 251, 255]));
                let recording = compositor.compose(FrameInput {
                    source: &source,
                    overlay: None,
                    viewport: ViewportFrame::default(),
                    pointer: None,
                    camera: Some(&camera),
                });
                let preview = camera_framing_preview(&camera, overlay);
                for (x, y, pixel) in preview.enumerate_pixels() {
                    assert_eq!(pixel, recording.get_pixel(x, y), "{shape:?}, mirror={mirror}, ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn camera_overlay_is_cover_fitted_mirrored_and_masked() {
        let style = SceneStyle {
            camera: CameraOverlay {
                enabled: true,
                position: WatermarkPosition::BottomRight,
                size: 40,
                shape: CameraShape::Circle,
                mirror: true,
                margin: 5,
                shadow: true,
                center: None,
                ..Default::default()
            },
            ..flat_style(0x000000)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let white = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        // Camera frame: left half red, right half green (wider than tall).
        let camera = RgbaImage::from_fn(160, 90, |x, _| {
            if x < 80 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 255, 0, 255])
            }
        });
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&PointerOverlay {
                frame: PointerFrame {
                    location: NormalizedPoint { x: 0.1, y: 0.1 },
                    artwork_id: None,
                    bitmap: None,
                    magnification: 1.0,
                    tilt_degrees: 0.0,
                    opacity: 0.0,
                    blur_radius: 0.0,
                    velocity: (0.0, 0.0),
                    press: None,
                },
            }),
            camera: Some(&camera),
        });
        let rect = style.camera.rect(200.0, 200.0);
        let cx = rect.x + rect.width * 0.5;
        let cy = rect.y + rect.height * 0.5;
        // Mirrored: the camera's left half (red) appears on the right.
        assert_eq!(
            output
                .get_pixel((cx + rect.width * 0.3) as u32, cy as u32)
                .0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            output
                .get_pixel((cx - rect.width * 0.3) as u32, cy as u32)
                .0,
            [0, 255, 0, 255]
        );
        // Circle mask: the square's corner is not camera colour.
        let corner = output
            .get_pixel((rect.x + 2.0) as u32, (rect.y + 2.0) as u32)
            .0;
        assert!(
            corner != [255, 0, 0, 255] && corner != [0, 255, 0, 255],
            "{corner:?}"
        );
        // Disabled camera paints nothing.
        let disabled = SceneStyle {
            camera: CameraOverlay {
                enabled: false,
                ..style.camera
            },
            ..style.clone()
        };
        let compositor = SceneCompositor::new(&disabled, 200, 200, 200, 200).unwrap();
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: Some(&camera),
        });
        assert_eq!(
            output.get_pixel(cx as u32, cy as u32).0,
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn pointer_and_click_ring_paint_inside_the_media() {
        let style = SceneStyle {
            padding: 10,
            ..flat_style(0)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let white = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.5, y: 0.5 },
                artwork_id: None,
                bitmap: None,
                magnification: 1.0,
                tilt_degrees: 0.0,
                opacity: 1.0,
                blur_radius: 0.0,
                velocity: (0.0, 0.0),
                press: Some(PointerPressFrame {
                    location: NormalizedPoint { x: 0.5, y: 0.5 },
                    progress: 0.5,
                }),
            },
        };
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        });
        let media = compositor.geometry().media;
        let center = (
            (media.x + media.width * 0.5) as u32,
            (media.y + media.height * 0.5) as u32,
        );
        let mut painted = false;
        for dy in 0..12 {
            for dx in 0..12 {
                if output.get_pixel(center.0 + dx, center.1 + dy).0 != [255, 255, 255, 255] {
                    painted = true;
                }
            }
        }
        assert!(painted);
        // Hidden pointer paints nothing.
        let hidden = SceneStyle {
            pointer: PointerStyle {
                visible: false,
                click_effects: false,
                ..PointerStyle::default()
            },
            ..style
        };
        let compositor = SceneCompositor::new(&hidden, 200, 200, 200, 200).unwrap();
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        });
        assert_eq!(
            output.get_pixel(center.0 + 3, center.1 + 3).0,
            [255, 255, 255, 255]
        );
    }
}

#[cfg(test)]
mod cursor_smear_tests {
    use super::*;

    #[test]
    fn idle_blur_preserves_premultiplication_hotspot_and_cache_identity() {
        use std::sync::Arc;
        let bitmap = Arc::new(PointerBitmap {
            id: "synthetic-opaque-cursor".into(),
            image: RgbaImage::from_pixel(16, 16, Rgba([100, 50, 25, 128])),
            anchor: NormalizedPoint { x: 0.125, y: 0.25 },
            reference_width: 0.016, reference_height: 0.016, shape: None,
        });
        let placement = BitmapPlacement { hotspot_x: 37.0, hotspot_y: 53.0,
            width: 32.0, height: 32.0, tilt_degrees: 27.0, smear: (0.0, 0.0) };
        let (blurred, padded) = blurred_cursor(bitmap.clone(), placement, 10.0);
        assert!(blurred.image.width() > bitmap.image.width());
        let padding = (blurred.image.width() - bitmap.image.width()) / 2;
        assert!(blurred.image.get_pixel(padding - 2, padding + 8)[3] > 0,
            "blur must extend beyond the old artwork box");
        assert!(blurred.image.pixels().all(|pixel| pixel[0] <= pixel[3]
            && pixel[1] <= pixel[3] && pixel[2] <= pixel[3]));
        let mass = |image: &RgbaImage| image.pixels().map(|pixel| u64::from(pixel[3])).sum::<u64>();
        assert!((mass(&blurred.image) as f64 / mass(&bitmap.image) as f64 - 1.0).abs() < 0.03);
        let padding_canvas = padding as f64 * placement.width / bitmap.image.width() as f64;
        assert!((blurred.anchor.x * padded.width - padding_canvas - bitmap.anchor.x * placement.width).abs() < 1e-9);
        assert!((blurred.anchor.y * padded.height - padding_canvas - bitmap.anchor.y * placement.height).abs() < 1e-9);
        assert_eq!((padded.hotspot_x, padded.hotspot_y), (placement.hotspot_x, placement.hotspot_y));
        assert!(Arc::ptr_eq(&blurred, &blurred_cursor(bitmap.clone(), placement, 10.0).0));
        assert!(Arc::ptr_eq(&bitmap, &blurred_cursor(bitmap.clone(), placement, 0.0).0));
    }

    #[test]
    fn typing_hide_is_rendered_when_idle_hide_is_disabled() {
        use super::super::model::{KeystrokeEvent, PointerCaptureFile, PointerTravelKind, PointerTravelSample};
        let style = SceneStyle { pointer: PointerStyle { hide_when_idle: false,
            hide_when_typing: true, click_effects: false, ..Default::default() }, ..Default::default() };
        let capture = PointerCaptureFile {
            travel: vec![PointerTravelSample { time: 0.0, x: 0.5, y: 0.5,
                kind: PointerTravelKind::Move, artwork_id: None }],
            keystrokes: [0.1, 0.2, 0.3].into_iter().map(|time|
                KeystrokeEvent { time, key: "Space".into(), modifiers: vec![] }).collect(),
            ..Default::default()
        };
        let timeline = super::super::pointer_timeline::PointerTimeline::build(capture,
            2.0, 400.0, 300.0, style.pointer.timeline_options());
        let frame = timeline.frame_at(1.0).unwrap();
        assert!(frame.opacity < 0.05);
        let compositor = SceneCompositor::new(&style, 400, 300, 400, 300).unwrap();
        let source = RgbaImage::from_pixel(400, 300, Rgba([255, 255, 255, 255]));
        let render = |pointer| compositor.compose(FrameInput { source: &source, overlay: None,
            viewport: ViewportFrame::default(), pointer, camera: None });
        let plain = render(None);
        let hidden_pointer = PointerOverlay { frame: frame.clone() };
        let visible_pointer = PointerOverlay { frame: PointerFrame { opacity: 1.0,
            blur_radius: 0.0, ..frame } };
        let hidden = render(Some(&hidden_pointer));
        let visible = render(Some(&visible_pointer));
        let difference = |image: &RgbaImage| image.as_raw().iter().zip(plain.as_raw())
            .map(|(a,b)| u64::from(a.abs_diff(*b))).sum::<u64>();
        assert!(difference(&visible) > 1000);
        assert!(difference(&hidden) * 10 < difference(&visible), "typing hide must reach final pixels independently");
    }

    #[test]
    fn temporal_compositor_can_disable_or_scale_the_cursor_smear() {
        assert_eq!(cursor_smear((2.0, 1.0), (500.0, 400.0), 60.0, 0.0), (0.0, 0.0));
        let full = cursor_smear((2.0, 1.0), (500.0, 400.0), 60.0, 1.0);
        let half = cursor_smear((2.0, 1.0), (500.0, 400.0), 60.0, 0.5);
        assert_eq!(half, (full.0 * 0.5, full.1 * 0.5));
    }

    fn compose_with_velocity(velocity: (f64, f64)) -> (RgbaImage, u32, u32) {
        let style = SceneStyle {
            pointer: PointerStyle {
                click_effects: false,
                hide_when_idle: false,
                ..PointerStyle::default()
            },
            ..SceneStyle::default()
        };
        let compositor = SceneCompositor::new(&style, 400, 300, 400, 300).unwrap();
        let white = RgbaImage::from_pixel(400, 300, Rgba([255, 255, 255, 255]));
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.5, y: 0.5 },
                artwork_id: None,
                bitmap: None,
                magnification: 1.0,
                tilt_degrees: 0.0,
                opacity: 1.0,
                blur_radius: 0.0,
                velocity,
                press: None,
            },
        };
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        });
        let media = compositor.geometry().media;
        (
            output,
            (media.x + media.width * 0.5) as u32,
            (media.y + media.height * 0.5) as u32,
        )
    }

    #[test]
    fn fast_cursor_leaves_a_trail_behind_its_motion() {
        let (still, x, y) = compose_with_velocity((0.0, 0.0));
        // Ten media widths per second: a long streak to the left.
        let (moving, _, _) = compose_with_velocity((10.0, 0.0));
        let at = |image: &RgbaImage, dx: i64| image.get_pixel((x as i64 + dx) as u32, y + 4).0;
        // Left of the arrow is plain media when still, streaked when moving.
        assert_eq!(at(&still, -30), at(&still, -120));
        assert_ne!(at(&moving, -30), at(&still, -30));
    }
}

#[cfg(test)]
mod box_blur_edge_tests {
    use super::*;

    #[test]
    fn box_blur_keeps_flat_image_flat_at_edges() {
        let (w, h) = (40usize, 30usize);
        let src = vec![200.0f32; w * h];
        let mut tmp = vec![0.0f32; w * h];
        let mut out = vec![0.0f32; w * h];
        box_blur_horizontal(&src, &mut tmp, w, h, 7);
        box_blur_vertical(&tmp, &mut out, w, h, 7);
        for v in out {
            assert!((v - 200.0).abs() < 0.01, "edge darkened: {v}");
        }
    }
}

#[cfg(test)]
mod loupe_composition_tests {
    use super::*;
    use crate::viewport::LoupeFrame;

    fn lens() -> LoupeFrame {
        LoupeFrame { center: NormalizedPoint { x:0.5,y:0.5 }, radius:0.35,
            magnification:2.0, bevel:0.2, chromatic_aberration:0.5, presence:1.0 }
    }

    #[test]
    fn lens_preserves_native_one_pixel_strokes_and_glyphs_after_downscaling() {
        let mut source = RgbaImage::from_pixel(1024, 512, Rgba([255, 255, 255, 255]));
        // A one-pixel stripe chart beside tiny bitmap letters. A 4:1 canvas
        // pass discards these strokes; a 4× lens must recover original pixels.
        for y in 240..272 {
            for x in 480..512 {
                if x % 2 == 0 {
                    source.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                }
            }
        }
        let glyphs = [
            "1000101110",
            "1000100100",
            "1000100100",
            "1111100100",
            "1000100100",
            "1000100100",
            "1000101110",
        ];
        for (row, line) in glyphs.iter().enumerate() {
            for (col, value) in line.bytes().enumerate() {
                if value == b'1' {
                    source.put_pixel(520 + col as u32, 248 + row as u32, Rgba([0, 0, 0, 255]));
                }
            }
        }
        for cropped in [false, true] {
            let style = SceneStyle {
                padding: 0,
                corners: 0,
                shadow: 0,
                source_crop: if cropped {
                    crate::viewport::CropRect {
                        x: 0.25,
                        y: 0.25,
                        width: 0.5,
                        height: 0.5,
                    }
                } else {
                    crate::viewport::CropRect::UNIT
                },
                ..Default::default()
            };
            let scene = SceneCompositor::new(&style, 256, 128, 1024, 512).unwrap();
            let frame = LoupeFrame {
                magnification: if cropped { 2.0 } else { 4.0 },
                bevel: 0.0,
                chromatic_aberration: 0.0,
                ..lens()
            };
            let input = FrameInput {
                source: &source,
                overlay: None,
                viewport: ViewportFrame {
                    loupe: Some(frame),
                    ..Default::default()
                },
                pointer: None,
                camera: None,
            };
            let sharp = scene.compose(input);
            let mut reduced = scene.compose(FrameInput {
                viewport: ViewportFrame::default(),
                ..input
            });
            super::super::loupe::paint(&mut reduced, scene.loupe_geometry(input.viewport).unwrap());
            let mut direct_error = 0u64;
            let mut reduced_error = 0u64;
            for y in 48..80 {
                for x in 96..160 {
                    let expected = source.get_pixel(384 + x, 192 + y)[0];
                    direct_error += u64::from(sharp.get_pixel(x, y)[0].abs_diff(expected));
                    reduced_error += u64::from(reduced.get_pixel(x, y)[0].abs_diff(expected));
                }
            }
            assert!(
                direct_error < 2048,
                "native strokes softened with crop={cropped}: {direct_error}"
            );
            assert!(
                reduced_error > 30 * 2048,
                "fixture must detect the old reduced-canvas sampling"
            );
            eprintln!("native lens crop={cropped}: mean error {:.3}, reduced-canvas mean error {:.3}",
                direct_error as f64 / 2048.0, reduced_error as f64 / 2048.0);
            if let Some(directory) = std::env::var_os("LAHZA_SHARPNESS_ARTIFACTS") {
                let directory = std::path::PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                source.save(directory.join("original-one-pixel-text.png")).unwrap();
                sharp.save(directory.join(format!("native-loupe-crop-{cropped}.png"))).unwrap();
                reduced.save(directory.join(format!("reduced-canvas-loupe-crop-{cropped}.png"))).unwrap();
            }
        }
    }

    #[test]
    fn unit_lens_preserves_alpha_annotations_and_rotated_smeared_pointer() {
        use crate::pointer_timeline::PointerPressFrame;
        let style = SceneStyle {
            padding: 0,
            corners: 0,
            shadow: 0,
            background: SceneBackground::Solid(0x204060),
            pointer: PointerStyle {
                family: CursorFamily::Recorded,
                ..Default::default()
            },
            ..Default::default()
        };
        let scene = SceneCompositor::new(&style, 256, 160, 256, 160).unwrap();
        let source = RgbaImage::from_fn(256, 160, |x, y| {
            Rgba([x as u8, y as u8, 120, if x % 3 == 0 { 100 } else { 220 }])
        });
        let overlay = RgbaImage::from_fn(256, 160, |x, y| {
            Rgba([230, 30, 120, if (x + y) % 3 == 0 { 150 } else { 0 }])
        });
        let bitmap = std::sync::Arc::new(PointerBitmap {
            id: "lens-alpha-fixture".into(),
            image: RgbaImage::from_pixel(12, 18, Rgba([100, 40, 20, 128])),
            anchor: NormalizedPoint { x: 0.3, y: 0.2 },
            reference_width: 0.08,
            reference_height: 0.12,
            shape: None,
        });
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.5, y: 0.5 },
                artwork_id: None,
                bitmap: Some(bitmap),
                magnification: 1.0,
                tilt_degrees: 23.0,
                opacity: 0.7,
                blur_radius: 0.0,
                velocity: (0.9, 0.4),
                press: Some(PointerPressFrame {
                    location: NormalizedPoint { x: 0.5, y: 0.5 },
                    progress: 0.3,
                }),
            },
        };
        let input = FrameInput {
            source: &source,
            overlay: Some(&overlay),
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        };
        let plain = scene.compose(input);
        let optical = scene.compose(FrameInput {
            viewport: ViewportFrame {
                loupe: Some(LoupeFrame {
                    magnification: 1.0,
                    bevel: 0.0,
                    chromatic_aberration: 0.0,
                    ..lens()
                }),
                ..Default::default()
            },
            ..input
        });
        for y in 40..120 {
            for x in 88..168 {
                assert!(
                    plain
                        .get_pixel(x, y)
                        .0
                        .into_iter()
                        .zip(optical.get_pixel(x, y).0)
                        .all(|(a, b)| a.abs_diff(b) <= 1),
                    "unit lens changed layered pixel({x},{y})"
                );
            }
        }
    }

    #[test]
    #[ignore = "manual original-source lens performance regression; no capture devices"]
    fn benchmark_original_source_lens() {
        let source = RgbaImage::from_fn(3840, 2160, |x, y| {
            Rgba([
                ((x * 13 + y * 5) % 256) as u8,
                ((x * 3 + y * 7) % 256) as u8,
                ((x + y) % 256) as u8,
                255,
            ])
        });
        for (width, height) in [(1280, 720), (1600, 900)] {
            let style = SceneStyle {
                padding: 8,
                shadow: 0,
                background: SceneBackground::Solid(0x204060),
                ..Default::default()
            };
            let scene = SceneCompositor::new(&style, width, height, 3840, 2160).unwrap();
            let input = FrameInput {
                source: &source,
                overlay: None,
                viewport: ViewportFrame {
                    loupe: Some(LoupeFrame {
                        magnification: 3.0,
                        ..lens()
                    }),
                    ..Default::default()
                },
                pointer: None,
                camera: None,
            };
            let _ = scene.compose(input);
            let mut times = Vec::new();
            for _ in 0..9 {
                let start = std::time::Instant::now();
                std::hint::black_box(scene.compose(input));
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "original source 3840×2160 lens to {width}×{height}: median={:.2}ms p95={:.2}ms",
                times[4], times[8]
            );
            assert!(
                times[4] < 250.0,
                "lens composition must stay bounded for interactive previews"
            );
        }
    }

    #[test]
    fn lens_tracks_source_crop_projection_and_output_resolution() {
        let mut style = SceneStyle::default();
        style.source_crop = crate::viewport::CropRect { x:0.2,y:0.1,width:0.6,height:0.8 };
        style.transform.rotation_y = 18.0;
        let viewport = ViewportFrame { loupe:Some(lens()), ..Default::default() };
        let small = SceneCompositor::new(&style,320,240,160,100).unwrap();
        let large = SceneCompositor::new(&style,640,480,160,100).unwrap();
        let a = small.loupe_geometry(viewport).unwrap();
        let b = large.loupe_geometry(viewport).unwrap();
        assert!((b.radius - a.radius * 2.0).abs() < 1e-3);
        assert!((b.center[0] - a.center[0] * 2.0).abs() < 1e-3);
        assert!((b.center[1] - a.center[1] * 2.0).abs() < 1e-3);
        let expected = small.projection(viewport).project(0.5,0.5);
        assert!((a.center[0] as f64 - expected.0).abs() < 1e-3);
        assert!((a.center[1] as f64 - expected.1).abs() < 1e-3);
    }

    #[test]
    fn lens_keeps_camera_on_stage_and_canvas_overlays_above_glass() {
        let style = SceneStyle::default();
        let scene = SceneCompositor::new(&style,240,160,120,80).unwrap();
        let source = RgbaImage::from_pixel(120,80,Rgba([30,60,90,255]));
        let camera = RgbaImage::from_pixel(60,60,Rgba([240,30,10,255]));
        let input = FrameInput {source:&source,overlay:None,
            viewport:ViewportFrame {loupe:Some(lens()),..Default::default()}, pointer:None,camera:None};
        let expected = scene.compose(input);
        // The camera stays visible beside the lens, which never magnifies it.
        let with_camera = scene.compose(FrameInput {camera:Some(&camera),..input});
        let rect = style.camera.rect(240.0,160.0);
        let center = ((rect.x+rect.width*0.5) as u32, (rect.y+rect.height*0.5) as u32);
        assert_eq!(with_camera.get_pixel(center.0,center.1).0, [240,30,10,255]);
        assert_eq!(with_camera.get_pixel(120,80), expected.get_pixel(120,80));
        let caption = RgbaImage::from_pixel(240,160,Rgba([20,230,40,255]));
        let composited = scene.compose_layers(input,true,Some(&caption));
        assert!(composited.pixels().all(|p| p.0 == [20,230,40,255]));
        assert_eq!(scene.compose_layers(input,false,None),
            scene.compose_layers(FrameInput {viewport:ViewportFrame::default(),..input},false,None));
    }

    #[test]
    #[ignore = "writes /tmp/lahza-loupe-gallery.png for visual inspection"]
    fn render_loupe_gallery() {
        let source = RgbaImage::from_fn(640,400,|x,y| {
            let color = if y < 32 { [35,43,62,255] }
                else if x < 124 { if y % 42 < 4 { [58,69,92,255] } else { [27,34,51,255] } }
                else if x > 144 && x < 616 && y > 60 && y < 116 { [41,165,157,255] }
                else if x > 144 && x < 616 && y > 140 && y < 360 {
                    if (x-144) % 80 < 2 || (y-140) % 40 < 2 { [169,183,205,255] }
                    else if (x/20+y/18)%5 == 0 { [104,124,156,255] } else { [222,230,240,255] }
                } else { [243,246,251,255] };
            Rgba(color)
        });
        let style = SceneStyle { background:SceneBackground::Solid(0x151b2d),
            padding:22,corners:12,shadow:25,..Default::default() };
        let scene = SceneCompositor::new(&style,640,440,640,400).unwrap();
        let mut gallery = RgbaImage::new(1920,1760);
        for (row,presence) in [0.2,0.6,1.0,0.35].into_iter().enumerate() {
            for column in 0..3 {
                let lens = (column > 0).then_some(LoupeFrame {
                    center:NormalizedPoint {x:0.62,y:0.54}, radius:0.3,
                    magnification:1.0 + presence, presence,
                    bevel:if column == 2 {0.2*presence} else {0.0},
                    chromatic_aberration:if column == 2 {0.7*presence} else {0.0},
                });
                let image = scene.compose(FrameInput {source:&source,overlay:None,
                    viewport:ViewportFrame {loupe:lens,..Default::default()},pointer:None,camera:None});
                image::imageops::replace(&mut gallery,&image,column*640,row as i64*440);
            }
        }
        gallery.save("/tmp/lahza-loupe-gallery.png").unwrap();
    }
}
