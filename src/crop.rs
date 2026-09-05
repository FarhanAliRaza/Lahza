//! Crop geometry, canvas interaction, and crop undo/redo operations.

use super::annotations::{norm_to_screen, screen_to_norm};
use super::{CropDrag, CropHandle, CropRect, CropSnapshot, NormPoint, Studio, CROP_HANDLES};
use gpui::{hsla, point, px, quad, rgb, size, Bounds, Pixels, Point, Window};

fn normalized_aspect(aspect: usize, (width, height): (u32, u32)) -> Option<f32> {
    let pixel_ratio = match aspect {
        1 => return Some(1.0),
        2 => 1.0,
        3 => 16.0 / 9.0,
        4 => 9.0 / 16.0,
        5 => 4.0 / 3.0,
        6 => 3.0 / 2.0,
        _ => return None,
    };
    Some(pixel_ratio * height.max(1) as f32 / width.max(1) as f32)
}

fn select_aspect(rect: CropRect, aspect: Option<f32>) -> CropRect {
    // Presets always fit the source, never the previously selected preset.
    aspect.map_or(rect, |ratio| crop_rect_with_aspect(CropRect::UNIT, ratio))
}

fn remap_point(point: &mut NormPoint, from: CropRect, to: CropRect) {
    point.x = (from.x + point.x * from.width - to.x) / to.width;
    point.y = (from.y + point.y * from.height - to.y) / to.height;
}

fn crop_handle_point(handle: CropHandle, rect: CropRect) -> NormPoint {
    let (x, y) = match handle {
        CropHandle::TopLeft => (rect.x, rect.y),
        CropHandle::Top => (rect.x + rect.width * 0.5, rect.y),
        CropHandle::TopRight => (rect.right(), rect.y),
        CropHandle::Left => (rect.x, rect.y + rect.height * 0.5),
        CropHandle::Right => (rect.right(), rect.y + rect.height * 0.5),
        CropHandle::BottomLeft => (rect.x, rect.bottom()),
        CropHandle::Bottom => (rect.x + rect.width * 0.5, rect.bottom()),
        CropHandle::BottomRight => (rect.right(), rect.bottom()),
    };
    NormPoint { x, y }
}

pub(super) fn crop_rect_with_aspect(rect: CropRect, aspect: f32) -> CropRect {
    let mut width = rect.width;
    let mut height = rect.height;
    if width / height > aspect {
        width = height * aspect;
    } else {
        height = width / aspect;
    }
    let x = (rect.x + (rect.width - width) * 0.5).clamp(0.0, 1.0 - width);
    let y = (rect.y + (rect.height - height) * 0.5).clamp(0.0, 1.0 - height);
    CropRect {
        x,
        y,
        width,
        height,
    }
}

fn move_crop_rect(rect: CropRect, delta: NormPoint) -> CropRect {
    CropRect {
        x: (rect.x + delta.x).clamp(0.0, 1.0 - rect.width),
        y: (rect.y + delta.y).clamp(0.0, 1.0 - rect.height),
        ..rect
    }
}

fn resize_crop_rect(
    rect: CropRect,
    handle: CropHandle,
    point: NormPoint,
    aspect: Option<f32>,
    min_width: f32,
    min_height: f32,
) -> CropRect {
    let is_left = matches!(
        handle,
        CropHandle::TopLeft | CropHandle::Left | CropHandle::BottomLeft
    );
    let is_right = matches!(
        handle,
        CropHandle::TopRight | CropHandle::Right | CropHandle::BottomRight
    );
    let is_top = matches!(
        handle,
        CropHandle::TopLeft | CropHandle::Top | CropHandle::TopRight
    );
    let is_bottom = matches!(
        handle,
        CropHandle::BottomLeft | CropHandle::Bottom | CropHandle::BottomRight
    );
    let is_corner = (is_left || is_right) && (is_top || is_bottom);
    let mut left = rect.x;
    let mut right = rect.right();
    let mut top = rect.y;
    let mut bottom = rect.bottom();

    if is_corner {
        let anchor_x = if is_left { right } else { left };
        let anchor_y = if is_top { bottom } else { top };
        let mut width = (point.x - anchor_x).abs().max(min_width);
        let mut height = (point.y - anchor_y).abs().max(min_height);
        if let Some(aspect) = aspect {
            if width / height > aspect {
                width = height * aspect;
            } else {
                height = width / aspect;
            }
        }
        width = width.min(if is_left { anchor_x } else { 1.0 - anchor_x });
        height = height.min(if is_top { anchor_y } else { 1.0 - anchor_y });
        if let Some(aspect) = aspect {
            if width / height > aspect {
                width = height * aspect;
            } else {
                height = width / aspect;
            }
        }
        left = if is_left { anchor_x - width } else { anchor_x };
        right = if is_left { anchor_x } else { anchor_x + width };
        top = if is_top { anchor_y - height } else { anchor_y };
        bottom = if is_top { anchor_y } else { anchor_y + height };
    } else {
        if is_left {
            left = point.x.min(right - min_width);
        }
        if is_right {
            right = point.x.max(left + min_width);
        }
        if is_top {
            top = point.y.min(bottom - min_height);
        }
        if is_bottom {
            bottom = point.y.max(top + min_height);
        }
    }
    CropRect {
        x: left.clamp(0.0, 1.0),
        y: top.clamp(0.0, 1.0),
        width: (right - left).clamp(min_width, 1.0),
        height: (bottom - top).clamp(min_height, 1.0),
    }
}

pub(super) fn paint_crop_overlay(
    rect: CropRect,
    image: Bounds<Pixels>,
    aspect_locked: bool,
    window: &mut Window,
) {
    let top_left = norm_to_screen(
        NormPoint {
            x: rect.x,
            y: rect.y,
        },
        image,
    );
    let bottom_right = norm_to_screen(
        NormPoint {
            x: rect.right(),
            y: rect.bottom(),
        },
        image,
    );
    let crop = Bounds::from_corners(top_left, bottom_right);
    let dim = hsla(0.0, 0.0, 0.0, 0.55);
    let clear = hsla(0.0, 0.0, 0.0, 0.0);
    let image_right = image.origin.x + image.size.width;
    let image_bottom = image.origin.y + image.size.height;
    for bounds in [
        Bounds::from_corners(image.origin, point(image_right, crop.origin.y)),
        Bounds::from_corners(
            point(image.origin.x, crop.origin.y),
            point(crop.origin.x, crop.origin.y + crop.size.height),
        ),
        Bounds::from_corners(
            point(crop.origin.x + crop.size.width, crop.origin.y),
            point(image_right, crop.origin.y + crop.size.height),
        ),
        Bounds::from_corners(
            point(image.origin.x, crop.origin.y + crop.size.height),
            point(image_right, image_bottom),
        ),
    ] {
        if !bounds.is_empty() {
            window.paint_quad(quad(
                bounds,
                px(0.0),
                dim,
                px(0.0),
                clear,
                Default::default(),
            ));
        }
    }
    window.paint_quad(quad(
        crop,
        px(0.0),
        clear,
        px(1.5),
        rgb(0xffffff),
        Default::default(),
    ));
    for index in 1..=2 {
        let fraction = index as f32 / 3.0;
        let x = crop.origin.x + crop.size.width * fraction;
        let y = crop.origin.y + crop.size.height * fraction;
        let grid = hsla(0.0, 0.0, 1.0, 0.35);
        window.paint_quad(quad(
            Bounds::from_corners(
                point(x, crop.origin.y),
                point(x + px(1.0), crop.origin.y + crop.size.height),
            ),
            px(0.0),
            grid,
            px(0.0),
            clear,
            Default::default(),
        ));
        window.paint_quad(quad(
            Bounds::from_corners(
                point(crop.origin.x, y),
                point(crop.origin.x + crop.size.width, y + px(1.0)),
            ),
            px(0.0),
            grid,
            px(0.0),
            clear,
            Default::default(),
        ));
    }
    for handle in CROP_HANDLES {
        let corner = matches!(
            handle,
            CropHandle::TopLeft
                | CropHandle::TopRight
                | CropHandle::BottomLeft
                | CropHandle::BottomRight
        );
        if aspect_locked && !corner {
            continue;
        }
        let center = norm_to_screen(crop_handle_point(handle, rect), image);
        let size = if corner {
            size(px(13.0), px(13.0))
        } else if matches!(handle, CropHandle::Top | CropHandle::Bottom) {
            size(px(26.0), px(7.0))
        } else {
            size(px(7.0), px(26.0))
        };
        window.paint_quad(quad(
            Bounds {
                origin: point(center.x - size.width * 0.5, center.y - size.height * 0.5),
                size,
            },
            px(2.5),
            rgb(0xffffff),
            px(0.5),
            hsla(0.0, 0.0, 0.0, 0.25),
            Default::default(),
        ));
    }
}

impl Studio {
    pub(super) fn crop_normalized_aspect(&self) -> Option<f32> {
        normalized_aspect(self.crop_aspect, self.captured_dimensions?)
    }

    pub(super) fn begin_crop(&mut self) {
        if self.captured_path.is_none() {
            self.toast = Some("Capture an image first".into());
            return;
        }
        if self.crop_active {
            return;
        }
        self.stop_editing_text();
        self.selected_annotation = None;
        self.annotation_draft = None;
        let Some(current) = self.current_crop_snapshot() else {
            return;
        };
        let selection = self.source_crop;
        if let Some(mut original) = self.original_capture.clone() {
            original.annotations = self.annotations.clone();
            for mark in &mut original.annotations {
                let remap = |point: &mut NormPoint| {
                    remap_point(point, selection, CropRect::UNIT);
                };
                remap(&mut mark.start);
                remap(&mut mark.end);
                for point in &mut mark.points {
                    remap(point);
                }
            }
            if !self.restore_crop_snapshot(original) {
                return;
            }
        } else {
            self.original_capture = Some(current.clone());
        }
        self.crop_session = Some(current);
        self.crop_rect = selection;
        self.crop_aspect = 0;
        self.crop_drag = None;
        self.crop_active = true;
    }

    pub(super) fn cancel_crop(&mut self) {
        if let Some(snapshot) = self.crop_session.take() {
            self.restore_crop_snapshot(snapshot);
        }
        self.crop_active = false;
        self.crop_rect = CropRect::UNIT;
        self.crop_aspect = 0;
        self.crop_drag = None;
        self.pointer_is_down = false;
    }

    pub(super) fn set_crop_aspect(&mut self, aspect: usize) {
        self.crop_aspect = aspect;
        self.crop_rect = select_aspect(self.crop_rect, self.crop_normalized_aspect());
        self.crop_drag = None;
        self.pointer_is_down = false;
    }

    pub(super) fn reset_crop(&mut self) {
        self.crop_rect = CropRect::UNIT;
        self.crop_aspect = 0;
        self.crop_drag = None;
        self.pointer_is_down = false;
    }

    pub(super) fn crop_pointer_down(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) {
        if self.pointer_is_down {
            return;
        }
        self.pointer_is_down = true;
        let point = screen_to_norm(position, image);
        let handles: &[CropHandle] = if self.crop_aspect == 0 {
            &CROP_HANDLES
        } else {
            &[
                CropHandle::TopLeft,
                CropHandle::TopRight,
                CropHandle::BottomLeft,
                CropHandle::BottomRight,
            ]
        };
        let hit = handles.iter().copied().find(|handle| {
            let center = norm_to_screen(crop_handle_point(*handle, self.crop_rect), image);
            (center.x - position.x).abs() <= px(16.0) && (center.y - position.y).abs() <= px(16.0)
        });
        if let Some(handle) = hit {
            self.crop_drag = Some(CropDrag::Resize(handle));
        } else if point.x >= self.crop_rect.x
            && point.x <= self.crop_rect.right()
            && point.y >= self.crop_rect.y
            && point.y <= self.crop_rect.bottom()
        {
            self.crop_drag = Some(CropDrag::Move {
                start: point,
                rect: self.crop_rect,
            });
        }
    }

    pub(super) fn crop_pointer_move(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) {
        let Some(drag) = self.crop_drag else { return };
        let point = screen_to_norm(position, image);
        match drag {
            CropDrag::Move { start, rect } => {
                self.crop_rect = move_crop_rect(
                    rect,
                    NormPoint {
                        x: point.x - start.x,
                        y: point.y - start.y,
                    },
                );
            }
            CropDrag::Resize(handle) => {
                let (width, height) = self.captured_dimensions.unwrap_or((1200, 720));
                self.crop_rect = resize_crop_rect(
                    self.crop_rect,
                    handle,
                    point,
                    if matches!(
                        handle,
                        CropHandle::TopLeft
                            | CropHandle::TopRight
                            | CropHandle::BottomLeft
                            | CropHandle::BottomRight
                    ) {
                        self.crop_normalized_aspect()
                    } else {
                        None
                    },
                    (24.0 / width.max(1) as f32).clamp(0.01, 0.5),
                    (24.0 / height.max(1) as f32).clamp(0.01, 0.5),
                );
            }
        }
    }

    pub(super) fn apply_crop(&mut self) -> Result<(), String> {
        let source = self
            .captured_path
            .as_ref()
            .ok_or_else(|| "Capture an image first".to_string())?;
        let image = image::open(source)
            .map_err(|error| format!("Could not read capture: {error}"))?
            .to_rgba8();
        let old_width = image.width();
        let old_height = image.height();
        let rect = self.crop_rect;
        let left = (rect.x * old_width as f32)
            .floor()
            .clamp(0.0, old_width as f32 - 1.0) as u32;
        let top = (rect.y * old_height as f32)
            .floor()
            .clamp(0.0, old_height as f32 - 1.0) as u32;
        let right = (rect.right() * old_width as f32)
            .ceil()
            .clamp((left + 1) as f32, old_width as f32) as u32;
        let bottom = (rect.bottom() * old_height as f32)
            .ceil()
            .clamp((top + 1) as f32, old_height as f32) as u32;
        if left == 0 && top == 0 && right == old_width && bottom == old_height {
            if let Some(previous) = self.crop_session.take() {
                self.crop_undo_stack.push(previous);
                self.crop_redo_stack.clear();
            }
            self.crop_active = false;
            self.reset_crop();
            return Ok(());
        }
        let cropped =
            image::imageops::crop_imm(&image, left, top, right - left, bottom - top).to_image();
        let destination = std::env::temp_dir().join(format!(
            "lahza-crop-{}-{}.png",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        cropped
            .save(&destination)
            .map_err(|error| format!("Could not save crop: {error}"))?;
        let used = CropRect {
            x: left as f32 / old_width as f32,
            y: top as f32 / old_height as f32,
            width: (right - left) as f32 / old_width as f32,
            height: (bottom - top) as f32 / old_height as f32,
        };
        if let Some(previous) = self
            .crop_session
            .take()
            .or_else(|| self.current_crop_snapshot())
        {
            self.crop_undo_stack.push(previous);
        }
        self.source_crop = used;
        self.crop_redo_stack.clear();
        for mark in &mut self.annotations {
            let remap = |point: &mut NormPoint| {
                remap_point(point, CropRect::UNIT, used);
            };
            remap(&mut mark.start);
            remap(&mut mark.end);
            for point in &mut mark.points {
                remap(point);
            }
        }
        self.captured_path = Some(destination);
        self.captured_dimensions = Some((right - left, bottom - top));
        self.processed_capture_path = None;
        self.set_capture_image(cropped);
        self.crop_active = false;
        self.crop_rect = CropRect::UNIT;
        self.crop_aspect = 0;
        self.crop_drag = None;
        self.pointer_is_down = false;
        self.rebuild_redactions()?;
        Ok(())
    }

    fn current_crop_snapshot(&self) -> Option<CropSnapshot> {
        Some(CropSnapshot {
            source_crop: self.source_crop,
            path: self.captured_path.clone()?,
            dimensions: self.captured_dimensions?,
            annotations: self.annotations.clone(),
        })
    }

    fn restore_crop_snapshot(&mut self, snapshot: CropSnapshot) -> bool {
        let image = match image::open(&snapshot.path) {
            Ok(image) => image.to_rgba8(),
            Err(error) => {
                self.toast = Some(format!("Could not restore crop: {error}").into());
                return false;
            }
        };
        self.source_crop = snapshot.source_crop;
        self.captured_path = Some(snapshot.path);
        self.captured_dimensions = Some(snapshot.dimensions);
        self.annotations = snapshot.annotations;
        self.processed_capture_path = None;
        self.set_capture_image(image);
        self.selected_annotation = None;
        self.editing_text = None;
        let _ = self.rebuild_redactions();
        true
    }

    pub(super) fn undo_crop(&mut self) -> bool {
        let Some(previous) = self.crop_undo_stack.pop() else {
            return false;
        };
        let Some(current) = self.current_crop_snapshot() else {
            return false;
        };
        self.crop_redo_stack.push(current);
        self.restore_crop_snapshot(previous)
    }

    pub(super) fn redo_crop(&mut self) -> bool {
        let Some(next) = self.crop_redo_stack.pop() else {
            return false;
        };
        let Some(current) = self.current_crop_snapshot() else {
            return false;
        };
        self.crop_undo_stack.push(current);
        self.restore_crop_snapshot(next)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        crop_rect_with_aspect, move_crop_rect, resize_crop_rect, CropHandle, CropRect, NormPoint,
    };

    #[test]
    fn switching_presets_repeatedly_uses_the_full_source() {
        for dimensions in [(1600, 900), (900, 1600), (800, 800)] {
            let mut rect = CropRect::UNIT;
            for _ in 0..30 {
                for preset in [2, 3, 4, 5, 6, 1] {
                    let ratio = super::normalized_aspect(preset, dimensions).unwrap();
                    rect = super::select_aspect(rect, Some(ratio));
                    assert!((rect.width / rect.height - ratio).abs() < 0.0001);
                    assert!(rect.width == 1.0 || rect.height == 1.0);
                    assert!(rect.right() <= 1.0 && rect.bottom() <= 1.0);
                }
                assert_eq!(rect.width, 1.0);
                assert_eq!(rect.height, 1.0);
            }
        }
    }

    #[test]
    fn annotations_round_trip_between_original_and_successive_crops() {
        let crops = [
            CropRect {
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.6,
            },
            CropRect {
                x: 0.3,
                y: 0.1,
                width: 0.4,
                height: 0.3,
            },
        ];
        // Also retain marks outside the visible crop so reset reveals them.
        for original in [NormPoint { x: 0.4, y: 0.5 }, NormPoint { x: 0.95, y: 0.05 }] {
            let mut point = original;
            for crop in crops {
                super::remap_point(&mut point, CropRect::UNIT, crop);
                super::remap_point(&mut point, crop, CropRect::UNIT);
                assert!((point.x - original.x).abs() < 0.0001);
                assert!((point.y - original.y).abs() < 0.0001);
            }
        }
    }

    #[test]
    fn crop_aspect_and_handles_stay_inside_image() {
        let square = crop_rect_with_aspect(CropRect::UNIT, 0.5);
        assert!((square.width - 0.5).abs() < 0.0001);
        assert!((square.height - 1.0).abs() < 0.0001);
        assert!((square.x - 0.25).abs() < 0.0001);

        let resized = resize_crop_rect(
            square,
            CropHandle::BottomRight,
            NormPoint { x: 2.0, y: 2.0 },
            Some(0.5),
            0.01,
            0.01,
        );
        assert!(resized.x >= 0.0 && resized.y >= 0.0);
        assert!(resized.right() <= 1.0 && resized.bottom() <= 1.0);
        assert!((resized.width / resized.height - 0.5).abs() < 0.0001);
    }

    #[test]
    fn moving_crop_never_leaves_image() {
        let rect = CropRect {
            x: 0.2,
            y: 0.2,
            width: 0.4,
            height: 0.3,
        };
        let moved = move_crop_rect(rect, NormPoint { x: 5.0, y: -5.0 });
        assert!((moved.x - 0.6).abs() < 0.0001);
        assert_eq!(moved.y, 0.0);
    }
}
