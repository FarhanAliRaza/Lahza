//! Still-image canvas painting and video editor layout.

use super::{
    blue, fitted_image_bounds, gradient_layers, ink, line, muted, paint_annotation,
    paint_crop_overlay, paint_highlights, recording, scene_ui, timed, visible_rect, AnnotationMark,
    SceneSelection, Studio, Tool, GRADIENT_BACKGROUNDS, MOTION_ZOOM_SLIDER, SOLID_BACKGROUNDS,
};
use gpui::{
    canvas, div, hsla, img, point, prelude::*, px, quad, rgb, size, svg, AnyElement, Background,
    Bounds, BoxShadow, ContentMask, Context, CursorStyle, FontWeight, Hsla, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    RenderImage, ScrollWheelEvent, Window,
};
use std::sync::Arc;

impl Studio {
    pub(super) fn mock_capture(
        &self,
        cx: &mut Context<Self>,
        canvas_width: Pixels,
        canvas_height: Pixels,
        composited: Option<Arc<RenderImage>>,
    ) -> impl IntoElement {
        let solid = SOLID_BACKGROUNDS[self.color_index.min(SOLID_BACKGROUNDS.len() - 1)].1;
        let gradient =
            GRADIENT_BACKGROUNDS[self.gradient_index.min(GRADIENT_BACKGROUNDS.len() - 1)];
        let (gradient_base, gradient_overlay) = gradient_layers(gradient);
        let background_base: Background = match self.wallpaper_tab {
            0 => rgb(solid).into(),
            1 => gradient_base,
            _ => rgb(0x111214).into(),
        };
        let custom_wallpaper = self.custom_wallpaper.clone();
        let border_colors = [0xffc928, 0x22b45d, 0x22bfc2, 0x3678ef, 0x8c4ce8, 0xec3d87];
        let border_color = border_colors[self.border_color.min(border_colors.len() - 1)];
        // Swift stores border thickness as 0.2%...8% of the screenshot's
        // shortest edge. At this preview size the full range is about 0...48px.
        let border_width = if self.border {
            px(self.border_thickness as f32 * 0.48)
        } else {
            px(0.0)
        };
        // The original app maps 0...100% to a radius of 0...12% of the
        // screenshot's shortest edge. At this preview size that is about 64px.
        let corner_radius = px(self.corners as f32 * 0.64);
        let border_tint = Hsla::from(rgb(border_color)).opacity(self.border_opacity as f32 / 100.0);
        let strength = self.shadow as f32 / 100.0;
        let (radius_scale, offset_scale, opacity_scale) = match self.shadow_style {
            0 => (1.0, 0.3, 1.0),  // Soft
            1 => (1.2, 0.9, 0.85), // Long
            2 => (1.6, 0.0, 0.7),  // Glow
            _ => (0.8, 0.2, 1.1),  // Crisp
        };
        let shadow_radius = 85.0 * strength * radius_scale;
        let shadow_alpha = (0.08 + strength * 1.35)
            .min(0.35)
            .mul_add(opacity_scale, 0.0)
            .min(0.5);
        let shadow_layers = if self.shadow == 0 {
            Vec::new()
        } else {
            vec![BoxShadow {
                color: Hsla::from(rgb(self.shadow_color)).opacity(shadow_alpha),
                offset: point(px(0.0), px(shadow_radius * offset_scale)),
                blur_radius: px(shadow_radius),
                spread_radius: px(0.0),
            }]
        };
        let has_capture = self.captured_path.is_some();
        // `object-fit: contain` can place the bitmap somewhere inside its box.
        // Size the box to the fitted bitmap instead so its rounded clipping,
        // border, shadow, annotations, and pointer hit testing share one rect.
        let image_bounds = fitted_image_bounds(
            Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(canvas_width, canvas_height),
            },
            has_capture,
            self.captured_dimensions,
            self.padding,
            self.border,
            self.border_thickness,
        );
        let image_x = image_bounds.origin.x;
        let image_y = image_bounds.origin.y;
        let image_width = image_bounds.size.width;
        let image_height = image_bounds.size.height;
        let card_x = image_x - border_width;
        let card_y = image_y - border_width;
        let card_width = image_width + border_width * 2.0;
        let card_height = image_height + border_width * 2.0;
        let card_radius = corner_radius + border_width;
        let shadow_x = if self.border { card_x } else { image_x };
        let shadow_y = if self.border { card_y } else { image_y };
        let shadow_width = if self.border { card_width } else { image_width };
        let shadow_height = if self.border {
            card_height
        } else {
            image_height
        };
        let shadow_radius_for_card = if self.border {
            card_radius
        } else {
            corner_radius
        };
        let mut annotations = self.annotations.clone();
        if let Some(draft) = self.annotation_draft.clone() {
            annotations.push(draft);
        }
        let canvas_annotations = self.canvas_annotation_marks();
        let media_visible = self.image_visible_at(self.video_position);
        let committed_count = self.annotations.len();
        // Animated scenes paint each mark at its state for the playhead time.
        let selected_annotation = self.selected_annotation;
        let editing_text = self.editing_text;
        let (annotations, painted_indices): (Vec<AnnotationMark>, Vec<usize>) =
            if self.animation_active {
                let time = self.video_position;
                let mut marks = Vec::new();
                let mut indices = Vec::new();
                for (index, mark) in annotations.iter().enumerate() {
                    if mark.canvas || !media_visible { continue; }
                    if let Some(animated) = timed::editor_mark(
                        mark,
                        time,
                        selected_annotation == Some(index) || editing_text == Some(index),
                    ) {
                        marks.push(animated);
                        indices.push(index);
                    }
                }
                (marks, indices)
            } else {
                annotations.into_iter().enumerate().filter(|(_, mark)| !mark.canvas)
                    .map(|(index, mark)| (mark, index)).unzip()
            };
        let caret_visible = self.caret_visible;
        let crop_active = self.crop_active;
        let crop_rect = self.crop_rect;
        let crop_aspect_locked = self.crop_aspect != 0;
        let entity = cx.entity();
        let captured_dimensions = self.captured_dimensions;
        let padding = self.padding;
        let border = self.border;
        let border_thickness = self.border_thickness;
        let displayed_capture = self
            .processed_capture_path
            .clone()
            .or_else(|| self.captured_path.clone());
        let displayed_capture_image = self.displayed_capture_image.clone();
        let needs_path_fallback = displayed_capture_image.is_none();
        let animation_active = self.animation_active;
        // While animating, the still image is cropped by the same viewport
        // the exporter uses; annotations move with it.
        let (view_zoom, view_left, view_top) = if animation_active {
            let frame = self.video_viewport_timeline.frame_at(self.video_position);
            let (left, top, _) = visible_rect(frame);
            (frame.magnification.max(1.0) as f32, left as f32, top as f32)
        } else {
            (1.0, 0.0, 0.0)
        };
        let media_bounds_store = self.video_media_bounds.clone();
        let scene_bounds_store = self.scene_canvas_bounds.clone();
        // A composited preview lays the media out with the compositor's own
        // geometry, so annotations and hit testing must follow that rect.
        let composited_style = composited.is_some().then(|| self.scene_style());
        let composited_active = composited.is_some();
        // Under a 3D transform the compositor draws the annotations too.
        let paint_gpui_annotations = self.annotations_paint_flat();
        let select_tool = self.tool == Tool::Select;
        // Focus / pan-end markers of the selected motion region.
        let motion_markers = if animation_active && media_visible {
            let (_, projection) =
                self.preview_projection(f32::from(canvas_width), f32::from(canvas_height));
            self.motion_marker_points(&projection)
        } else {
            Vec::new()
        };
        let zoomed = move |bounds: Bounds<Pixels>| Bounds {
            origin: point(
                bounds.origin.x - bounds.size.width * view_zoom * view_left,
                bounds.origin.y - bounds.size.height * view_zoom * view_top,
            ),
            size: size(
                bounds.size.width * view_zoom,
                bounds.size.height * view_zoom,
            ),
        };
        div()
            .id("editable-canvas")
            .w(canvas_width)
            .h(canvas_height)
            .flex_none()
            .shadow_lg()
            .bg(background_base)
            .relative()
            .overflow_hidden()
            .when(self.wallpaper_tab == 1, |this| {
                this.child(div().absolute().inset_0().bg(gradient_overlay))
            })
            .when(
                self.wallpaper_tab == 2 && custom_wallpaper.is_none(),
                |this| {
                    this.child(
                        img(self.wallpaper_asset)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                },
            )
            .when_some(
                if self.wallpaper_tab == 2 {
                    custom_wallpaper
                } else {
                    None
                },
                |this, path| {
                    this.child(
                        img(path)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                },
            )
            .child(
                div()
                    .absolute()
                    .left(shadow_x)
                    .top(shadow_y)
                    .w(shadow_width)
                    .h(shadow_height)
                    .rounded(shadow_radius_for_card)
                    .shadow(shadow_layers),
            )
            .when(
                self.border && self.border_thickness > 0 && self.border_opacity > 0,
                |this| {
                    this.child(
                        div()
                            .absolute()
                            .left(card_x)
                            .top(card_y)
                            .w(card_width)
                            .h(card_height)
                            .rounded(card_radius)
                            .bg(border_tint),
                    )
                },
            )
            .child(
                div()
                    .absolute()
                    .left(image_x)
                    .top(image_y)
                    .w(image_width)
                    .h(image_height)
                    .bg(rgb(0xfafafa))
                    .border_1()
                    .border_color(rgb(0xd6dde6))
                    .overflow_hidden()
                    .rounded(corner_radius)
                    .when(has_capture, |this| {
                        this.child(
                            img("mock-capture.svg")
                                .size_full()
                                .object_fit(ObjectFit::Contain)
                                .rounded(corner_radius),
                        )
                    })
                    .when(!has_capture, |this| {
                        this.flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .child(
                                svg()
                                    .path("icons/capture.svg")
                                    .size(px(46.0))
                                    .text_color(hsla(220.0 / 360.0, 0.05, 0.78, 1.0)),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ink())
                                    .child("Nothing captured yet"),
                            )
                            .child(div().text_sm().text_color(muted()).child(
                                "Take a screenshot, record your screen, or open a saved recording",
                            ))
                            .child(
                                div()
                                    .mt_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("empty-take-screenshot")
                                            .px_4()
                                            .h(px(36.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded_lg()
                                            .bg(blue())
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xffffff))
                                            .cursor_pointer()
                                            .hover(|style| {
                                                style.bg(hsla(211.0 / 360.0, 0.88, 0.45, 1.0))
                                            })
                                            .child(
                                                svg()
                                                    .path("icons/capture.svg")
                                                    .size(px(16.0))
                                                    .text_color(rgb(0xffffff)),
                                            )
                                            .child("Take screenshot")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.begin_screen_capture(cx)
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("empty-record-video")
                                            .px_4()
                                            .h(px(36.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(line())
                                            .bg(rgb(0xffffff))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0xf0f1f3)))
                                            .child(
                                                svg()
                                                    .path("icons/record.svg")
                                                    .size(px(16.0))
                                                    .text_color(rgb(0xd92d3a)),
                                            )
                                            .child("Record video")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.start_recording(cx)
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("empty-open-recording")
                                            .px_4()
                                            .h(px(36.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(line())
                                            .bg(rgb(0xffffff))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0xf0f1f3)))
                                            .child(
                                                svg()
                                                    .path("icons/play.svg")
                                                    .size(px(16.0))
                                                    .text_color(ink()),
                                            )
                                            .child("Open recording")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.open_video_project_dialog(cx)
                                            })),
                                    ),
                            )
                    }),
            )
            .when_some(displayed_capture_image, |this, image| {
                this.child(
                    div()
                        .absolute()
                        .left(image_x)
                        .top(image_y)
                        .w(image_width)
                        .h(image_height)
                        .overflow_hidden()
                        .rounded(corner_radius)
                        .child(
                            img(image)
                                .absolute()
                                .left(-(image_width * view_zoom * view_left))
                                .top(-(image_height * view_zoom * view_top))
                                .w(image_width * view_zoom)
                                .h(image_height * view_zoom)
                                .object_fit(ObjectFit::Contain)
                                .rounded(corner_radius),
                        ),
                )
            })
            .when_some(
                if needs_path_fallback {
                    displayed_capture
                } else {
                    None
                },
                |this, path| {
                    this.child(
                        div()
                            .absolute()
                            .left(image_x)
                            .top(image_y)
                            .w(image_width)
                            .h(image_height)
                            .overflow_hidden()
                            .rounded(corner_radius)
                            .child(
                                img(path)
                                    .size_full()
                                    .object_fit(ObjectFit::Contain)
                                    .rounded(corner_radius),
                            ),
                    )
                },
            )
            .when_some(composited, |this, image| {
                // Match the canvas exactly even when the raster size was rounded.
                this.child(
                    img(image)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Fill),
                )
            })
            .child(
                canvas(
                    // The hitbox lets occluding overlays (dialogs) shadow the
                    // raw mouse listeners registered below.
                    move |bounds, window, _| {
                        (
                            annotations,
                            window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                        )
                    },
                    move |bounds, (annotations, hitbox), window, cx| {
                        let image_bounds = match composited_style.as_ref() {
                            Some(style) => {
                                let (source_width, source_height) =
                                    captured_dimensions.unwrap_or((1200, 720));
                                let media = recording::scene::SceneGeometry::layout(
                                    f64::from(bounds.size.width),
                                    f64::from(bounds.size.height),
                                    source_width as f64,
                                    source_height as f64,
                                    style,
                                )
                                .media;
                                Bounds {
                                    origin: point(
                                        bounds.origin.x + px(media.x as f32),
                                        bounds.origin.y + px(media.y as f32),
                                    ),
                                    size: size(px(media.width as f32), px(media.height as f32)),
                                }
                            }
                            None => fitted_image_bounds(
                                bounds,
                                has_capture,
                                captured_dimensions,
                                padding,
                                border,
                                border_thickness,
                            ),
                        };
                        if let Ok(mut stored) = media_bounds_store.lock() {
                            *stored = Some(image_bounds);
                        }
                        if let Ok(mut stored) = scene_bounds_store.lock() {
                            *stored = Some(bounds);
                        }
                        let paint_bounds = zoomed(image_bounds);
                        // While animating, drawing happens in the zoomed view.
                        let interaction_bounds = if animation_active {
                            paint_bounds
                        } else {
                            image_bounds
                        };
                        let painted_indices = painted_indices.clone();
                        let annotation_bounds = window.with_content_mask(
                            Some(ContentMask {
                                bounds: image_bounds,
                            }),
                            |window| {
                                if !paint_gpui_annotations {
                                    return Vec::new();
                                }
                                paint_highlights(&annotations, paint_bounds, window);
                                let mut annotation_bounds = Vec::with_capacity(annotations.len());
                                for (painted, mark) in annotations.iter().enumerate() {
                                    let index = painted_indices[painted];
                                    let rendered_bounds = paint_annotation(
                                        mark,
                                        if mark.pinned {
                                            image_bounds
                                        } else {
                                            paint_bounds
                                        },
                                        index >= committed_count,
                                        editing_text == Some(index) && caret_visible,
                                        window,
                                        cx,
                                    );
                                    annotation_bounds.push(rendered_bounds);
                                    if selected_annotation == Some(index) {
                                        let selected_bounds = rendered_bounds;
                                        window.paint_quad(quad(
                                            selected_bounds,
                                            px(3.0),
                                            hsla(0.0, 0.0, 0.0, 0.0),
                                            px(2.0),
                                            rgb(0x2997ff),
                                            Default::default(),
                                        ));
                                        window.paint_quad(quad(
                                            Bounds {
                                                origin: point(
                                                    selected_bounds.origin.x
                                                        + selected_bounds.size.width
                                                        - px(5.0),
                                                    selected_bounds.origin.y
                                                        + selected_bounds.size.height
                                                        - px(5.0),
                                                ),
                                                size: size(px(10.0), px(10.0)),
                                            },
                                            px(5.0),
                                            rgb(0xffffff),
                                            px(2.0),
                                            rgb(0x2997ff),
                                            Default::default(),
                                        ));
                                    }
                                }
                                annotation_bounds
                            },
                        );
                        let canvas_hits = scene_ui::paint_canvas_annotations(&canvas_annotations, selected_annotation, bounds, window, cx);
                        if !motion_markers.is_empty() {
                            window.with_content_mask(
                                Some(ContentMask {
                                    bounds,
                                }),
                                |window| {
                                    scene_ui::paint_motion_markers(&motion_markers, bounds, window)
                                },
                            );
                        }
                        if crop_active {
                            paint_crop_overlay(crop_rect, image_bounds, crop_aspect_locked, window);
                        }

                        window.on_mouse_event({
                            let entity = entity.clone();
                            let annotation_bounds = annotation_bounds.clone();
                            move |event: &MouseDownEvent, _, window, cx| {
                                let crop_hit_bounds = Bounds {
                                    origin: point(
                                        image_bounds.origin.x - px(18.0),
                                        image_bounds.origin.y - px(18.0),
                                    ),
                                    size: size(
                                        image_bounds.size.width + px(36.0),
                                        image_bounds.size.height + px(36.0),
                                    ),
                                };
                                if event.button != MouseButton::Left
                                    || !hitbox.is_hovered(window)
                                    || if crop_active {
                                        !crop_hit_bounds.contains(&event.position)
                                    } else {
                                        !bounds.contains(&event.position)
                                    }
                                {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    this.focus_handle.focus(window);
                                    if this.canvas_annotation_pointer_down(event.position, bounds, &canvas_hits) {
                                        cx.notify();
                                        return;
                                    }
                                    if !media_visible { return; }
                                    // Drawing through a projected preview lands where
                                    // the pointer is on the card, not on the canvas.
                                    let flat = if composited_active {
                                        this.flat_pointer_position(
                                            event.position,
                                            bounds,
                                            interaction_bounds,
                                        )
                                    } else {
                                        event.position
                                    };
                                    if animation_active && this.walkthrough_mode {
                                        if let Some(point) =
                                            this.media_point_at(event.position, bounds)
                                        {
                                            this.add_walkthrough_stop(point);
                                        }
                                    } else if animation_active
                                        && (select_tool || this.video_selected_zoom_cue.is_some())
                                    {
                                        // Motion mode: clicks choose the focus of the
                                        // selected region; otherwise they pick an
                                        // annotation first and fall back to the media.
                                        if this.video_selected_zoom_cue.is_none()
                                            && interaction_bounds.contains(&flat)
                                            && !this.annotations.is_empty()
                                        {
                                            this.pointer_down(
                                                flat,
                                                interaction_bounds,
                                                &annotation_bounds,
                                            );
                                        }
                                        if this.selected_annotation.is_some() {
                                            this.video_selected_press = None;
                                            this.scene_selection = SceneSelection::Scene;
                                        } else {
                                            this.toast = None;
                                            this.scene_pointer_down(
                                                event.position,
                                                bounds,
                                                &event.modifiers,
                                                event.click_count,
                                                cx,
                                            );
                                        }
                                    } else if animation_active {
                                        // Drawing tools place timed marks at the playhead.
                                        this.pause_video_playback();
                                        if interaction_bounds.contains(&flat) {
                                            this.pointer_down(
                                                flat,
                                                interaction_bounds,
                                                &annotation_bounds,
                                            );
                                        }
                                    } else if this.crop_active {
                                        this.crop_pointer_down(event.position, image_bounds);
                                    } else if !paint_gpui_annotations && select_tool {
                                        // Transformed media: select moves the card.
                                        this.scene_pointer_down(
                                            event.position,
                                            bounds,
                                            &event.modifiers,
                                            event.click_count,
                                            cx,
                                        );
                                    } else if select_tool {
                                        if image_bounds.contains(&event.position) {
                                            this.pointer_down(
                                                event.position,
                                                image_bounds,
                                                &annotation_bounds,
                                            );
                                        }
                                        if this.selected_annotation.is_none() {
                                            this.scene_pointer_down(
                                                event.position,
                                                bounds,
                                                &event.modifiers,
                                                event.click_count,
                                                cx,
                                            );
                                        }
                                    } else if interaction_bounds.contains(&flat) {
                                        this.pointer_down(
                                            flat,
                                            interaction_bounds,
                                            &annotation_bounds,
                                        );
                                    }
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseMoveEvent, _, _, cx| {
                                if !event.dragging() {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    if this.canvas_annotation_drag {
                                        this.pointer_move(event.position, bounds);
                                        cx.notify();
                                        return;
                                    }
                                    if this.focus_drag.is_some() {
                                        this.drag_motion_marker(event.position, bounds, cx);
                                        cx.notify();
                                        return;
                                    }
                                    if this.media_drag.is_some() {
                                        this.update_media_drag(event.position);
                                        cx.notify();
                                        return;
                                    }
                                    if this.crop_active {
                                        this.crop_pointer_move(event.position, image_bounds);
                                    } else {
                                        let flat = if composited_active {
                                            this.flat_pointer_position(
                                                event.position,
                                                bounds,
                                                interaction_bounds,
                                            )
                                        } else {
                                            event.position
                                        };
                                        this.pointer_move(flat, interaction_bounds);
                                    }
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                            if event.button != MouseButton::Left {
                                return;
                            }
                            entity.update(cx, |this, cx| {
                                if this.canvas_annotation_drag {
                                    this.pointer_up(event.position, bounds);
                                    this.canvas_annotation_drag = false;
                                    cx.notify();
                                    return;
                                }
                                this.focus_drag = None;
                                this.end_media_drag();
                                let flat = if composited_active {
                                    this.flat_pointer_position(
                                        event.position,
                                        bounds,
                                        interaction_bounds,
                                    )
                                } else {
                                    event.position
                                };
                                if this.crop_active {
                                    this.crop_drag = None;
                                    this.pointer_is_down = false;
                                } else if this.pointer_up(flat, interaction_bounds) {
                                    if let Err(error) = this.rebuild_redactions() {
                                        this.toast = Some(
                                            format!("Could not render redaction: {error}").into(),
                                        );
                                    }
                                }
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .when(
                self.tool == Tool::Text || self.editing_text.is_some(),
                |this| this.cursor(CursorStyle::IBeam),
            )
            .when(
                self.tool != Tool::Text && self.editing_text.is_none(),
                |this| this.cursor_crosshair(),
            )
            .when(self.scene_selection == SceneSelection::Media, |this| {
                this.cursor(CursorStyle::OpenHand)
            })
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                if this.scene_scroll(event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
    }

    pub(super) fn render_video(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.autosave_scene_style();
        if self.video_extras_pending {
            self.video_extras_pending = false;
            self.spawn_video_extras(cx);
        }
        self.ensure_camera_frame(cx);
        let (canvas_width, canvas_height) = self.canvas_budget(window.viewport_size());
        let video_canvas = self.scene_canvas(canvas_width, canvas_height, cx);
        let top_bar = self.top_bar(cx);
        let canvas_area = self.canvas_area(video_canvas, cx);
        let timeline = self.timeline_bar(cx);
        let sidebar = self.inspector_visible.then(|| self.sidebar(cx));
        let speed_dialog = self.video_speed_dialog(cx);

        div()
            .size_full()
            .min_w(px(980.0))
            .min_h(px(680.0))
            .bg(rgb(0xf3f3f4))
            .text_color(ink())
            .font_family("Inter")
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.native_text_focused(window, cx) { return; }
                if this.capture_access_prompt.is_some() {
                    cx.stop_propagation();
                    return;
                }
                if this.handle_video_key(event, cx) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let mut changed = this.update_slider_drag(event);
                if event.dragging() && this.annotation_drag.is_some() {
                    if this.update_annotation_drag(event.position.x) {
                        changed = true;
                    }
                } else if event.dragging() && this.media_drag.is_some() {
                    if this.update_media_drag(event.position) {
                        changed = true;
                    }
                } else if event.dragging() {
                    if let Some(drag) = this.video_move_drag.as_mut() {
                        drag.current_x = event.position.x;
                        // Reordering while a preview render is in flight is
                        // safe: the next apply supersedes it via the
                        // generation token, so don't gate on video_edit_busy.
                        if !drag.active && (drag.current_x - drag.start_x).abs() > px(6.0) {
                            drag.active = true;
                            this.video_seek_drag = None;
                        }
                        if drag.active {
                            changed = true;
                        }
                    }
                    if this.video_zoom_drag.is_some() {
                        this.update_video_zoom_drag(event.position.x);
                        changed = true;
                    } else if this.video_trim_drag.is_some() {
                        this.update_video_trim(event.position.x);
                        changed = true;
                    } else if let Some((start_x, start_position)) = this.video_seek_drag {
                        let delta = (event.position.x - start_x) / px(1.0);
                        let content_width =
                            this.video_timeline_viewport_width() * this.video_timeline_zoom;
                        this.video_position = (start_position
                            + delta as f64 / content_width * this.video_duration)
                            .clamp(0.0, this.video_duration);
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.end_motion_transform_drag() {
                        cx.notify();
                    }
                    if this.end_media_drag() {
                        cx.notify();
                    }
                    if this.end_annotation_drag() {
                        cx.notify();
                    }
                    if let Some(drag) = this.video_move_drag.take() {
                        if drag.active {
                            this.commit_video_move_drag(drag, cx);
                            this.slider_drag = None;
                            cx.notify();
                            return;
                        }
                    }
                    if this.video_zoom_drag.is_some() {
                        this.commit_video_zoom_drag(cx);
                    } else if this.video_trim_drag.is_some() {
                        this.commit_video_trim(cx);
                    } else if this.video_seek_drag.take().is_some() {
                        this.seek_video(this.video_position, cx);
                    }
                    if this
                        .slider_drag
                        .take()
                        .is_some_and(|drag| drag.slider_id == MOTION_ZOOM_SLIDER)
                    {
                        this.persist_video_zoom_cues(cx);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.end_motion_transform_drag() {
                        cx.notify();
                    }
                }),
            )
            .child(top_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(canvas_area)
                            .child(timeline),
                    )
                    .when_some(sidebar, |this, sidebar| this.child(sidebar)),
            )
            .when_some(speed_dialog, |this, dialog| {
                this.child(gpui::deferred(dialog).with_priority(1))
            })
            .into_any_element()
    }
}
