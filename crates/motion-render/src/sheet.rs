//! Contact sheets: frames at sync points in a labelled grid, drawn by the
//! engine itself (thumbnails as image layers, labels as text layers).

use crate::{Backend, RenderOptions, Renderer};
use image::RgbaImage;
use lahza_motion_core::compile::{Affine, Content, FrameState, LayerState, TextState};
use lahza_motion_core::document::{Align, Blend};
use lahza_motion_core::value::{Color, Vec2};
use std::sync::Arc;

const LABEL: f64 = 30.0;
const GAP: f64 = 10.0;

/// Lays `frames` (label, image) out in `columns`, each `thumb_width` wide.
pub fn contact_sheet(frames: &[(String, RgbaImage)], columns: u32, thumb_width: u32) -> RgbaImage {
    let columns = columns.clamp(1, 12).min(frames.len().max(1) as u32);
    let rows = (frames.len() as u32).div_ceil(columns).max(1);
    let (fw, fh) = frames.first().map(|(_, f)| f.dimensions()).unwrap_or((16, 9));
    let tw = thumb_width.max(32) as f64;
    let th = (tw * fh as f64 / fw.max(1) as f64).round();
    let width = (GAP + columns as f64 * (tw + GAP)).round() as u32;
    let height = (GAP + rows as f64 * (th + LABEL + GAP)).round() as u32;
    let mut renderer = Renderer::new(None, RenderOptions { backend: Backend::Cpu, motion_blur: false, ..Default::default() });
    let mut layers = Vec::new();
    for (i, (label, frame)) in frames.iter().enumerate() {
        let thumb = image::imageops::resize(frame, tw as u32, th as u32, image::imageops::FilterType::Triangle);
        let name = format!("frame-{i}");
        renderer.images.insert_named(&name, &thumb);
        let col = (i as u32 % columns) as f64;
        let row = (i as u32 / columns) as f64;
        let x = GAP + col * (tw + GAP);
        let y = GAP + row * (th + LABEL + GAP);
        layers.push(LayerState {
            id: name.as_str().into(),
            matrix: Affine::translate(x + tw / 2.0, y + th / 2.0).into(),
            opacity: 1.0,
            blend: Blend::Normal,
            effects: Vec::new(),
            matte: None,
            content: Content::Image { src: name.as_str().into(), size: Some(Vec2::new(tw, th)) },
            view: None,
        });
        let text: Arc<str> = label.as_str().into();
        let count = text.chars().count();
        layers.push(LayerState {
            id: format!("label-{i}").into(),
            matrix: Affine::translate(x + 4.0, y + th + LABEL / 2.0).into(),
            opacity: 1.0,
            blend: Blend::Normal,
            effects: Vec::new(),
            matte: None,
            content: Content::Text(TextState {
                text,
                font: "Inter Medium".into(),
                size: 15.0,
                color: Color::parse("#e8e8ee").unwrap(),
                tracking: 0.0,
                line_height: 1.2,
                align: Align::Left,
                glyphs: Vec::new(),
                pivots: vec![u32::MAX; count].into(),
                weight: 0.0,
                stroke: None,
                edge: None,
                sweeps: Vec::new(),
                units: Vec::new().into(),
            }),
            view: None,
        });
    }
    let state = FrameState { t: 0.0, scene: None, background: Color::parse("#18181c").unwrap(), width, height, layers };
    renderer.render_states(&[state])
}
