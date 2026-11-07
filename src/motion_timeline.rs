//! Motion editing on the studio timeline. In the Motion tab the selected
//! piece opens into a row per layer (its bar and keys) and a row per sound,
//! in video time, edited through the same model as the motion editor.

use crate::controls::{button, ButtonKind, ButtonSize};
use crate::motion_editor::{Field, MotionEditor, SoundPart, SoundShape, FIELDS};
use crate::shell_ui::EditorMode;
use crate::theme;
use crate::Studio;
use gpui::{canvas, div, prelude::*, px, size, AnyElement, App, Bounds, Context, Entity, FontWeight, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Window};
use lahza_motion_core::recording::Placement;
use serde_json::{json, Value};

/// Height of one layer or sound row.
pub(crate) const LAYER_ROW: f32 = 22.0;

/// The piece open for editing on the timeline.
pub(crate) struct MotionDoc {
    pub piece: String,
    pub editor: Entity<MotionEditor>,
}

/// A key or a sound being dragged on the timeline.
pub(crate) enum LayerDrag {
    Key { layer: String, path: String, index: usize, from: f64 },
    /// `grab`: video time under the pointer at press.
    Sound { id: String, part: SoundPart, grab: f64, orig: SoundShape, start_x: Pixels, preview: Option<SoundShape> },
    /// A property scrubbed in the inspector.
    Value { layer: String, field: Field, start_x: Pixels, origin: f64 },
    /// A layer moved on the preview: the piece point under the pointer at
    /// press, the layer's position then, and the preview canvas.
    Move { layer: String, grab: (f64, f64), origin: [f64; 2], canvas: Bounds<Pixels>, moved: bool },
}

impl Studio {
    /// The Motion tab of a recording, or a motion project of its own:
    /// the timeline edits motion.
    pub(crate) fn motion_editing(&self) -> bool {
        self.motion_project.is_some() || (self.video_project.is_some() && self.displayed_mode() == EditorMode::Motion)
    }

    /// Opens a motion project on its own in the Motion tab.
    pub(crate) fn open_motion_project(&mut self, dir: std::path::PathBuf) {
        self.launcher_active = false;
        self.launcher_window = false;
        self.motion_project = Some(dir);
        self.inspector_visible = true;
        self.select_inspector_tab(crate::shell_ui::InspectorTab::Motion);
    }

    pub(crate) fn close_motion_project(&mut self) {
        self.motion_project = None;
        self.motion_doc = None;
        self.motion_layer_drag = None;
        self.motion_scrub = false;
    }

    /// Video time where the open piece starts (0 for a motion project).
    fn motion_doc_start(&self) -> Option<f64> {
        if self.motion_project.is_some() {
            return Some(0.0);
        }
        let doc = self.motion_doc.as_ref()?;
        Some(self.motion_pieces.as_ref()?.get(&doc.piece)?.piece.start)
    }

    /// The open piece's or project's name.
    fn motion_doc_name(&self) -> Option<String> {
        if let Some(dir) = &self.motion_project {
            return Some(dir.file_stem().and_then(|s| s.to_str()).unwrap_or("motion").to_string());
        }
        let doc = self.motion_doc.as_ref()?;
        Some(self.motion_pieces.as_ref()?.get(&doc.piece)?.piece.name.clone())
    }

    /// The open piece's time at the playhead.
    fn motion_doc_time(&self, cx: &App) -> Option<f64> {
        let doc = self.motion_doc.as_ref()?;
        if self.motion_project.is_some() {
            return Some(doc.editor.read(cx).time());
        }
        Some(self.video_position - self.motion_doc_start()?)
    }

    /// Keeps the selected piece open for editing while the Motion tab
    /// shows (the first piece when none is selected); closes it otherwise.
    pub(crate) fn ensure_motion_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = if let Some(dir) = self.motion_project.clone() {
            Some(("project".to_string(), dir))
        } else if self.motion_editing() {
            if self.selected_motion_piece.is_none() {
                self.selected_motion_piece = self.motion_pieces.as_ref().and_then(|m| m.items.first()).map(|p| p.piece.id.clone());
            }
            self.selected_motion_piece.clone().and_then(|id| Some((id.clone(), self.motion_pieces.as_ref()?.get(&id)?.dir.clone())))
        } else {
            None
        };
        match want {
            Some((id, _)) if self.motion_doc.as_ref().is_some_and(|d| d.piece == id) => {}
            Some((id, dir)) => {
                self.motion_layer_drag = None;
                if let Err(e) = lahza_motion_core::io::load(&dir) {
                    self.toast = Some(crate::Notification::error(format!("Could not open the motion design: {e}")));
                    self.motion_doc = None;
                    return;
                }
                let editor = cx.new(|cx| {
                    let mut editor = MotionEditor::new(dir.clone(), cx).unwrap_or_else(|e| panic!("cannot open {}: {e}", dir.display()));
                    editor.start_ticking(cx);
                    cx.on_release_in(window, |this, window, _| this.release_images(window)).detach();
                    editor
                });
                // The editor reloads from disk on its own; redraw the rows.
                cx.observe(&editor, |_, _, cx| cx.notify()).detach();
                self.motion_doc = Some(MotionDoc { piece: id, editor });
            }
            None => {
                self.motion_doc = None;
                self.motion_layer_drag = None;
            }
        }
        // The inspector reads and keys values at the video's playhead (a
        // motion project keeps its own clock).
        if let Some(doc) = self.motion_doc.as_ref().filter(|_| self.motion_project.is_none()) {
            if let Some(start) = self.motion_pieces.as_ref().and_then(|m| m.get(&doc.piece)).map(|p| p.piece.start) {
                let t = self.video_position - start;
                doc.editor.update(cx, |e, _| e.set_time(t));
            }
        }
    }

    /// The open piece's placement and size, when it plays at the playhead.
    fn open_piece_shape(&self) -> Option<(Placement, (f64, f64))> {
        let p = self.motion_pieces.as_ref()?.get(&self.motion_doc.as_ref()?.piece)?;
        p.active_at(self.timeline_display_time()).then(|| (p.piece.placement, (p.project.settings.width as f64, p.project.settings.height as f64)))
    }

    /// The open piece's point (project pixels) under a preview position.
    fn piece_point_at(&self, position: Point<Pixels>, canvas: Bounds<Pixels>) -> Option<(f64, f64)> {
        let (placement, piece) = self.open_piece_shape()?;
        match placement {
            Placement::Overlay => {
                let m = self.media_point_at(position, canvas)?;
                let (w, h) = self.media_dimensions().map(|(w, h)| (w as f64, h as f64))?;
                let (k, ox, oy) = crate::motion_pieces::fit(placement, piece, (w, h));
                Some(((m.x * w - ox) / k, (m.y * h - oy) / k))
            }
            Placement::Full => {
                let (w, h) = (f64::from(canvas.size.width), f64::from(canvas.size.height));
                let (k, ox, oy) = crate::motion_pieces::fit(placement, piece, (w, h));
                Some(((f64::from(position.x - canvas.origin.x) - ox) / k, (f64::from(position.y - canvas.origin.y) - oy) / k))
            }
        }
    }

    /// Where a point of the open piece shows on a `width × height` preview
    /// canvas (canvas pixels).
    fn piece_to_canvas(&self, (x, y): (f64, f64), width: f32, height: f32) -> Option<(f64, f64)> {
        let (placement, piece) = self.open_piece_shape()?;
        match placement {
            Placement::Overlay => {
                let (w, h) = self.media_dimensions().map(|(w, h)| (w as f64, h as f64))?;
                let (k, ox, oy) = crate::motion_pieces::fit(placement, piece, (w, h));
                let (nx, ny) = ((ox + x * k) / w, (oy + y * k) / h);
                let (style, geometry, viewport) = self.preview_geometry_frame(width, height);
                let projection = geometry.projection(style.transform.with_motion(viewport));
                let (left, top, vx, vy) = style.source_crop.visible_rect(viewport);
                Some(projection.project((nx - left) / vx, (ny - top) / vy))
            }
            Placement::Full => {
                let (k, ox, oy) = crate::motion_pieces::fit(placement, piece, (width as f64, height as f64));
                Some((ox + x * k, oy + y * k))
            }
        }
    }

    /// In the Motion tab a press on the preview picks the open piece's
    /// layer under it and starts moving it. False when no layer is there.
    pub(crate) fn motion_canvas_pointer_down(&mut self, position: Point<Pixels>, canvas: Bounds<Pixels>, cx: &mut Context<Self>) -> bool {
        if !self.motion_editing() || self.motion_isolated() {
            return false;
        }
        let Some(editor) = self.motion_doc.as_ref().map(|d| d.editor.clone()) else { return false };
        let Some(grab) = self.piece_point_at(position, canvas) else { return false };
        let Some(layer) = editor.update(cx, |e, _| e.hit_at(grab.0, grab.1)) else { return false };
        self.pause_video_playback();
        let origin = editor.update(cx, |e, _| {
            e.select(Some(layer.clone()));
            e.position_of(&layer)
        });
        self.motion_layer_drag = Some(LayerDrag::Move { layer, grab, origin: [origin.x(), origin.y()], canvas, moved: false });
        true
    }

    /// The selected layer's outline over the preview canvas.
    pub(crate) fn motion_selection_outline(&self, width: Pixels, height: Pixels, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.motion_editing() || self.motion_isolated() {
            return None;
        }
        let editor = self.motion_doc.as_ref()?.editor.clone();
        let r = editor.update(cx, |e, _| e.selection_bounds())?;
        let (w, h) = (f32::from(width), f32::from(height));
        let corners: Vec<(f64, f64)> = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])].iter().filter_map(|&p| self.piece_to_canvas(p, w, h)).collect();
        if corners.len() < 4 || corners.iter().any(|c| !c.0.is_finite() || !c.1.is_finite()) {
            return None;
        }
        let (x0, y0) = corners.iter().fold((f64::INFINITY, f64::INFINITY), |a, c| (a.0.min(c.0), a.1.min(c.1)));
        let (x1, y1) = corners.iter().fold((f64::NEG_INFINITY, f64::NEG_INFINITY), |a, c| (a.0.max(c.0), a.1.max(c.1)));
        Some(
            div()
                .absolute()
                .left(px(x0 as f32))
                .top(px(y0 as f32))
                .w(px(((x1 - x0) as f32).max(2.0)))
                .h(px(((y1 - y0) as f32).max(2.0)))
                .border_1()
                .border_color(theme::accent())
                .into_any_element(),
        )
    }

    /// Isolate: the preview shows only the open piece, edited in place.
    pub(crate) fn motion_isolated(&self) -> bool {
        self.motion_doc.is_some() && (self.motion_project.is_some() || (self.motion_isolate && self.motion_editing()))
    }

    pub(crate) fn toggle_motion_isolate(&mut self, cx: &mut Context<Self>) {
        self.motion_isolate = !self.motion_isolate;
        cx.notify();
    }

    /// The isolate preview: the piece alone, as the motion editor draws it.
    pub(crate) fn motion_isolate_canvas(&self, width: Pixels, height: Pixels) -> Option<AnyElement> {
        self.motion_isolated().then(|| div().w(width).h(height).flex().child(self.motion_doc.as_ref().unwrap().editor.clone()).into_any_element())
    }

    /// Height of the open piece's rows, or None when none is open.
    pub(crate) fn motion_layer_lane_height(&self, cx: &App) -> Option<f32> {
        let editor = self.motion_doc.as_ref()?.editor.read(cx);
        let rows = editor.rows().len() + editor.waves().len();
        (rows > 0).then(|| rows as f32 * LAYER_ROW)
    }

    /// The open piece's row names, beside its rows.
    pub(crate) fn motion_layer_labels(&self, width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let doc = self.motion_doc.as_ref()?;
        let editor = doc.editor.read(cx);
        let selected = editor.selected().map(str::to_string);
        let mut column = div().w(px(width)).flex_none().flex().flex_col();
        let label = |text: String, sub: String, is_sel: bool| {
            div()
                .h(px(LAYER_ROW))
                .pr_3()
                .flex()
                .items_center()
                .gap_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_xs()
                .text_color(if is_sel { theme::ink() } else { theme::muted() })
                .child(text)
                .child(div().text_color(theme::muted()).opacity(0.7).child(sub))
        };
        for (i, wave) in editor.waves().iter().enumerate() {
            let is_sel = wave.id.is_some() && wave.id.as_deref() == selected.as_deref();
            let id = wave.id.clone();
            column = column.child(
                label(format!("♪ {}", wave.name), String::new(), is_sel)
                    .id(("motion-sound-label", i))
                    .when(id.is_some(), |d| d.cursor_pointer())
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if id.is_some() {
                            this.select_motion_layer(id.clone(), cx);
                        }
                    })),
            );
        }
        for (i, row) in editor.rows().iter().enumerate() {
            let id = row.id.clone();
            column = column.child(
                label(format!("{}{}", "  ".repeat(row.depth), row.id), row.kind.to_string(), selected.as_deref() == Some(row.id.as_str()))
                    .id(("motion-layer-label", i))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.select_motion_layer(Some(id.clone()), cx))),
            );
        }
        Some(column.into_any_element())
    }

    fn select_motion_layer(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if let Some(doc) = &self.motion_doc {
            doc.editor.update(cx, |e, _| e.select(id));
        }
        cx.notify();
    }

    /// The open piece's rows: a bar per layer where it plays, its keys,
    /// and a bar per sound, at the piece's place on the video timeline.
    pub(crate) fn motion_layer_lane(&self, scroll: f64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let height = self.motion_layer_lane_height(cx)?;
        let doc = self.motion_doc.as_ref()?;
        let offset = self.motion_doc_start()?;
        let spp = self.video_timeline_seconds_per_pixel();
        let x_of = move |t: f64| ((offset + t) / spp - scroll) as f32;
        let editor = doc.editor.read(cx);
        let selected = editor.selected().map(str::to_string);
        let mut lane = div().relative().w_full().flex_none().h(px(height)).overflow_hidden().rounded_lg().bg(theme::control_bg());
        let rows = editor.rows();
        let waves = editor.waves();
        for (ri, row) in rows.iter().enumerate() {
            let top = (waves.len() + ri) as f32 * LAYER_ROW;
            let is_sel = selected.as_deref() == Some(row.id.as_str());
            let id = row.id.clone();
            lane = lane.child(
                div()
                    .id(("motion-layer-row", ri))
                    .absolute()
                    .top(px(top))
                    .left_0()
                    .right_0()
                    .h(px(LAYER_ROW))
                    .when(is_sel, |d| d.bg(theme::selection_fill()))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.select_motion_layer(Some(id.clone()), cx))),
            );
            let (x0, x1) = (x_of(row.start), x_of(row.end));
            let colour = match row.kind {
                "text" => theme::lane_zoom(),
                "video" | "image" => theme::lane_video(),
                "comp" => theme::lane_camera(),
                _ => theme::lane_audio(),
            };
            lane = lane.child(div().absolute().top(px(top + 5.0)).left(px(x0)).w(px((x1 - x0).max(2.0))).h(px(LAYER_ROW - 10.0)).rounded(px(3.0)).bg(colour).opacity(if is_sel { 0.9 } else { 0.55 }));
            for (ki, k) in row.keys.iter().enumerate() {
                let (layer, path, index, from) = (row.id.clone(), k.path.clone(), k.index, k.t);
                lane = lane.child(
                    div()
                        .id(("motion-layer-key", ri * 10_000 + ki))
                        .absolute()
                        .left(px(x_of(k.t) - 5.0))
                        .top(px(top + 2.0))
                        .text_size(px(12.0))
                        .text_color(if is_sel { gpui::white() } else { theme::ink() })
                        .cursor_ew_resize()
                        .child("◆")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.pause_video_playback();
                                this.select_motion_layer(Some(layer.clone()), cx);
                                this.motion_layer_drag = Some(LayerDrag::Key { layer: layer.clone(), path: path.clone(), index, from });
                            }),
                        ),
                );
            }
        }
        let drag_preview = match &self.motion_layer_drag {
            Some(LayerDrag::Sound { id, preview: Some(shape), .. }) => Some((id.clone(), *shape)),
            _ => None,
        };
        for (wi, wave) in waves.iter().enumerate() {
            let top = wi as f32 * LAYER_ROW;
            let shape = match (&drag_preview, &wave.id) {
                (Some((pid, shape)), Some(id)) if pid == id => *shape,
                _ => SoundShape::of(wave),
            };
            let shown = shape.apply(wave);
            let (x0, x1) = (x_of(shape.start), x_of(shape.start + shape.length));
            let is_sel = wave.id.is_some() && wave.id.as_deref() == selected.as_deref();
            let (zoom, colour) = (1.0 / spp, theme::lane_audio());
            let mut bar = div()
                .id(("motion-sound-bar", wi))
                .absolute()
                .top(px(top + 3.0))
                .left(px(x0))
                .w(px((x1 - x0).max(3.0)))
                .h(px(LAYER_ROW - 6.0))
                .rounded(px(3.0))
                .bg(colour.opacity(if is_sel { 0.35 } else { 0.18 }))
                .border_1()
                .border_color(if is_sel { theme::accent() } else { colour.opacity(0.5) })
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                            let mut x = 0.0f32;
                            while x < bw {
                                let t0 = shown.start + f64::from(x) / zoom;
                                let peak = shown.peak(t0, t0 + 2.0 / zoom);
                                if peak > 0.0 {
                                    let h = (peak * (bh - 2.0)).max(1.0);
                                    window.paint_quad(gpui::fill(Bounds::new(gpui::point(bounds.origin.x + px(x), bounds.origin.y + px((bh - h) / 2.0)), size(px(1.5), px(h))), colour));
                                }
                                x += 2.0;
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                );
            if let Some(id) = wave.id.clone() {
                let press = |part: SoundPart, id: String| {
                    cx.listener(move |this: &mut Self, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.pause_video_playback();
                        this.select_motion_layer(Some(id.clone()), cx);
                        if let Some(grab) = this.motion_timeline_time_at(e.position.x) {
                            this.motion_layer_drag = Some(LayerDrag::Sound { id: id.clone(), part, grab, orig: shape, start_x: e.position.x, preview: None });
                        }
                    })
                };
                bar = bar.cursor_grab().on_mouse_down(MouseButton::Left, press(SoundPart::Move, id.clone()));
                // Trim edges: the outer 5 px of each end.
                for (part, left) in [(SoundPart::Start, true), (SoundPart::End, false)] {
                    let edge = div().id(SharedString::from(format!("motion-sound-trim-{id}-{left}"))).absolute().top_0().bottom_0().w(px(5.0)).cursor_ew_resize();
                    let edge = if left { edge.left_0() } else { edge.right_0() };
                    bar = bar.child(edge.hover(|s| s.bg(theme::accent().opacity(0.35))).on_mouse_down(MouseButton::Left, press(part, id.clone())));
                }
                // Fade handles on the top edge, at the end of each fade.
                let bar_w = (x1 - x0).max(3.0);
                if bar_w >= 24.0 {
                    let fin = (shape.fade_in / spp) as f32;
                    let fout = bar_w - (shape.fade_out / spp) as f32;
                    for (part, x, tip) in [(SoundPart::FadeIn, fin.clamp(6.0, bar_w - 6.0), "in"), (SoundPart::FadeOut, fout.clamp(6.0, bar_w - 6.0), "out")] {
                        bar = bar.child(
                            div()
                                .id(SharedString::from(format!("motion-sound-fade-{id}-{tip}")))
                                .absolute()
                                .top(px(-5.0))
                                .left(px(x - 6.0))
                                .size(px(12.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_ew_resize()
                                .child(div().size(px(8.0)).rounded_full().bg(gpui::white()).border_1().border_color(theme::ink().opacity(0.6)))
                                .on_mouse_down(MouseButton::Left, press(part, id.clone())),
                        );
                    }
                }
            }
            lane = lane.child(bar);
        }
        Some(lane.into_any_element())
    }

    /// Follows a key or sound drag; the preview shows the change live.
    pub(crate) fn update_motion_layer_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let x = position.x;
        let Some(editor) = self.motion_doc.as_ref().map(|d| d.editor.clone()) else { return false };
        if let Some(LayerDrag::Move { layer, grab, origin, canvas, .. }) = &self.motion_layer_drag {
            let (layer, grab, origin, canvas) = (layer.clone(), *grab, *origin, *canvas);
            let Some(now) = self.piece_point_at(position, canvas) else { return false };
            if let Some(LayerDrag::Move { moved, .. }) = &mut self.motion_layer_drag {
                *moved |= (now.0 - grab.0).abs() + (now.1 - grab.1).abs() > 0.5;
            }
            editor.update(cx, |e, _| e.move_layer(&layer, [origin[0] + now.0 - grab.0, origin[1] + now.1 - grab.1], false));
            self.show_motion_doc(cx);
            return true;
        }
        if let Some(LayerDrag::Value { layer, field, start_x, origin }) = &self.motion_layer_drag {
            let (layer, field) = (layer.clone(), *field);
            let value = origin + f64::from((x - *start_x) / px(1.0)) * field.step;
            editor.update(cx, |e, _| e.drag_value(&layer, field, value));
            self.show_motion_doc(cx);
            return true;
        }
        let Some(t) = self.motion_timeline_time_at(x) else { return false };
        let Some(offset) = self.motion_doc_start() else { return false };
        match self.motion_layer_drag.as_mut() {
            Some(LayerDrag::Key { layer, path, index, .. }) => {
                let (layer, path, index) = (layer.clone(), path.clone(), *index);
                editor.update(cx, |e, _| e.drag_key(&layer, &path, index, t - offset));
                self.show_motion_doc(cx);
                true
            }
            Some(LayerDrag::Sound { part, grab, orig, preview, .. }) => {
                *preview = Some(orig.dragged(*part, t - *grab));
                true
            }
            Some(LayerDrag::Value { .. } | LayerDrag::Move { .. }) | None => false,
        }
    }

    /// Ends a key or sound drag: saves it through the piece's history.
    pub(crate) fn finish_motion_layer_drag(&mut self, position: Point<Pixels>, shift: bool, cx: &mut Context<Self>) -> bool {
        let x = position.x;
        let Some(drag) = self.motion_layer_drag.take() else { return false };
        if let LayerDrag::Move { layer, grab, origin, canvas, moved } = &drag {
            let now = self.piece_point_at(position, *canvas);
            let editor = self.motion_doc.as_ref().map(|d| d.editor.clone());
            if let Some(editor) = editor {
                match now.filter(|_| *moved) {
                    Some(now) => editor.update(cx, |e, _| e.move_layer(layer, [origin[0] + now.0 - grab.0, origin[1] + now.1 - grab.1], true)),
                    None => editor.update(cx, |e, _| e.cancel_preview()),
                }
            }
            self.show_motion_doc(cx);
            cx.notify();
            return true;
        }
        let t = self.motion_timeline_time_at(x);
        let Some(editor) = self.motion_doc.as_ref().map(|d| d.editor.clone()) else { return true };
        if let LayerDrag::Value { layer, field, start_x, origin } = &drag {
            let moved = (x - *start_x).abs() > px(0.5);
            let value = origin + f64::from((x - *start_x) / px(1.0)) * field.step;
            editor.update(cx, |e, _| if moved { e.drop_value(layer, *field, value) } else { e.cancel_preview() });
            self.show_motion_doc(cx);
            cx.notify();
            return true;
        }
        let Some(offset) = self.motion_doc_start() else { return true };
        let spp = self.video_timeline_seconds_per_pixel();
        match drag {
            LayerDrag::Key { layer, path, index, from } => {
                let t = t.map_or(from, |t| t - offset);
                editor.update(cx, |e, _| e.drop_key(&layer, &path, index, from, t, shift));
            }
            LayerDrag::Sound { id, part, grab, orig, start_x, .. } => {
                if let Some(t) = t {
                    let moved = (x - start_x).abs() >= px(2.0);
                    editor.update(cx, |e, _| e.drop_sound(&id, part, orig, t - grab, moved, 8.0 * spp, shift));
                }
            }
            LayerDrag::Value { .. } | LayerDrag::Move { .. } => {}
        }
        self.show_motion_doc(cx);
        cx.notify();
        true
    }

    /// The video preview draws the open piece as the editor has it now.
    fn show_motion_doc(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.motion_doc.as_ref() else { return };
        let compiled = doc.editor.read(cx).compiled();
        let piece = doc.piece.clone();
        if let Some(pieces) = self.motion_pieces.as_mut() {
            pieces.show_compiled(&piece, compiled);
        }
    }

    /// Adds a built-in effect to the open piece at the playhead, when the
    /// playhead is inside it.
    pub(crate) fn add_sound_effect_to_piece(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.motion_doc.as_ref().map(|d| d.editor.clone()) else { return false };
        let Some(t) = self.motion_doc_time(cx) else { return false };
        if !(0.0..editor.read(cx).project().settings.duration).contains(&t) {
            return false;
        }
        editor.update(cx, |e, _| e.add_sound_effect_at(name, t));
        cx.notify();
        true
    }

    /// Saves `ops` to the open piece and shows the result.
    fn motion_commit(&mut self, ops: Vec<Value>, cx: &mut Context<Self>) {
        let Some(editor) = self.motion_doc.as_ref().map(|d| d.editor.clone()) else { return };
        editor.update(cx, |e, _| e.commit_ops(ops));
        self.show_motion_doc(cx);
        cx.notify();
    }

    /// The Motion panel for the layer or sound selected on the open
    /// piece's rows; None when nothing is selected there.
    pub(crate) fn motion_layer_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.motion_editing() {
            return None;
        }
        let piece_name = self.motion_doc_name()?;
        let editor = self.motion_doc.as_ref()?.editor.read(cx);
        let id = editor.selected()?.to_string();
        let back = button("motion-layer-back", piece_name, ButtonKind::Ghost, ButtonSize::Sm)
            .icon("icons/chevron-left.svg")
            .tooltip("Back to the motion design")
            .on_click(cx.listener(|this, _, _, cx| this.select_motion_layer(None, cx)));
        let title = |text: String| div().text_sm().font_weight(FontWeight::SEMIBOLD).child(text);
        let note = |text: String| div().text_xs().text_color(theme::muted()).child(text);
        let mut panel = div().flex().flex_col().gap_3().child(div().flex().child(back));
        let status = editor.status().to_string();
        if !status.is_empty() {
            panel = panel.child(note(status));
        }

        if let Some(clip) = editor.sound(&id).cloned() {
            let wave = editor.waves().iter().find(|w| w.id.as_deref() == Some(id.as_str())).cloned();
            let at = serde_json::to_value(&clip.at).ok().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
            let gain = clip.gain_db;
            let set = |id: &str, field: &str, value: Value| json!({"op": "set", "target": format!("{id}.{field}"), "value": value});
            let step_button = |key: &'static str, label: &'static str, enabled: bool, ops: Vec<Value>| {
                button(SharedString::from(format!("motion-sound-{key}")), label, ButtonKind::Secondary, ButtonSize::Sm)
                    .disabled(!enabled)
                    .on_click(cx.listener(move |this, _, _, cx| this.motion_commit(ops.clone(), cx)))
            };
            let row = |label: String| div().flex().items_center().gap_2().child(div().text_xs().w(px(120.0)).child(label));
            panel = panel
                .child(title(format!("♪ {}", wave.as_ref().map(|w| w.name.clone()).unwrap_or_else(|| id.clone()))))
                .child(note(format!("{} · starts at {at}", clip.path)))
                .child(
                    row(format!("Gain {gain:+.1} dB"))
                        .child(step_button("gain-down", "−3 dB", gain > -60.0, vec![set(&id, "gain_db", json!((gain - 3.0).max(-60.0)))]))
                        .child(step_button("gain-up", "+3 dB", gain < 12.0, vec![set(&id, "gain_db", json!((gain + 3.0).min(12.0)))])),
                );
            if let Some(w) = &wave {
                let round = |v: f64| (v * 1000.0).round() / 1000.0;
                for (label, field, value, most) in [("Fade in", "fade_in", clip.fade_in, w.duration - w.fade_out), ("Fade out", "fade_out", clip.fade_out, w.duration - w.fade_in)] {
                    panel = panel.child(
                        row(format!("{label} {value:.2} s"))
                            .child(step_button(if field == "fade_in" { "fade-in-down" } else { "fade-out-down" }, "−", value > 0.0, vec![set(&id, field, json!(round((value - 0.05).max(0.0))))]))
                            .child(step_button(if field == "fade_in" { "fade-in-up" } else { "fade-out-up" }, "+", value < most - 1e-6, vec![set(&id, field, json!(round((value + 0.05).min(most.max(0.0)))))])),
                    );
                }
                panel = panel.child(note(format!("Plays {:.2} s of the file's {:.2} s, from {:.2} s. Drag the bar to move it, its ends to trim.", w.duration, w.source_duration, w.source_start)));
            }
            if clip.trim != 0.0 || clip.length.is_some() || clip.fade_in != 0.0 || clip.fade_out != 0.0 {
                let ops = vec![set(&id, "trim", json!(0.0)), json!({"op": "set", "target": format!("{id}.length")}), set(&id, "fade_in", json!(0.0)), set(&id, "fade_out", json!(0.0))];
                panel = panel.child(div().flex().child(step_button("reset", "Reset trim and fades", true, ops)));
            }
            let remove = id.clone();
            panel = panel.child(div().flex().justify_end().child(
                button("motion-sound-delete", "Delete sound", ButtonKind::Danger, ButtonSize::Sm).icon("icons/trash.svg").on_click(cx.listener(move |this, _, _, cx| {
                    this.select_motion_layer(None, cx);
                    this.motion_commit(vec![json!({"op": "remove", "id": remove})], cx);
                })),
            ));
            return Some(panel.into_any_element());
        }

        let layer = editor.layer(&id)?.clone();
        panel = panel.child(title(id.clone())).child(note(format!(
            "{} · blend {:?}{}",
            layer.kind.name(),
            layer.common.blend,
            if layer.common.three_d || layer.kind.name() == "mesh" { " · 3D" } else { "" }
        )));
        if let lahza_motion_core::document::LayerKind::Text(t) = &layer.kind {
            panel = panel.child(div().text_xs().child(format!("“{}”", t.text)));
        }
        let three_d = layer.common.three_d || layer.kind.name() == "mesh";
        let mut fields = div().flex().flex_col().gap_1();
        for (i, field) in FIELDS.iter().enumerate() {
            let depth_only = field.path.contains("_x") || field.path.contains("_y") || field.path.ends_with(".z");
            if depth_only && !three_d {
                continue;
            }
            let (value, animated) = editor.field_state(&id, *field);
            let (lid, f) = (id.clone(), *field);
            fields = fields.child(
                div()
                    .id(("motion-layer-field", i))
                    .flex()
                    .justify_between()
                    .items_center()
                    .h(px(28.0))
                    .px_2()
                    .rounded_md()
                    .bg(theme::control_bg())
                    .hover(|s| s.bg(theme::control_hover()))
                    .cursor_ew_resize()
                    .child(div().text_xs().text_color(theme::muted()).child(format!("{}{}", f.label, if animated { "  ◆" } else { "" })))
                    .child(div().text_xs().child(format!("{value:.2}")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.pause_video_playback();
                            let origin = this.motion_doc.as_ref().map(|d| d.editor.read(cx).field_state(&lid, f).0).unwrap_or(0.0);
                            this.motion_layer_drag = Some(LayerDrag::Value { layer: lid.clone(), field: f, start_x: e.position.x, origin });
                        }),
                    ),
            );
        }
        let effects: Vec<String> = serde_json::to_value(&layer.common.effects)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| e.get("type").and_then(Value::as_str).map(String::from))
            .collect();
        let keys = editor.rows().iter().find(|r| r.id == id).map_or(0, |r| r.keys.len());
        let remove = id.clone();
        panel = panel
            .child(fields)
            .child(note("Drag a value sideways to change it. Values marked ◆ are animated: a change adds a key at the playhead.".into()))
            .when(!effects.is_empty(), |d| d.child(note(format!("Effects: {}", effects.join(" · ")))))
            .child(note(format!("{keys} keys")))
            .child(div().flex().justify_end().child(
                button("motion-layer-delete", "Delete layer", ButtonKind::Danger, ButtonSize::Sm).icon("icons/trash.svg").on_click(cx.listener(move |this, _, _, cx| {
                    this.select_motion_layer(None, cx);
                    this.motion_commit(vec![json!({"op": "remove", "id": remove})], cx);
                })),
            ));
        Some(panel.into_any_element())
    }

    /// Undo and redo in the Motion tab go to the open piece's history.
    pub(crate) fn undo_motion_doc(&mut self, redo: bool, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.motion_doc.as_ref().map(|d| d.editor.clone()) else { return false };
        editor.update(cx, |e, _| e.undo_edit(redo));
        self.show_motion_doc(cx);
        cx.notify();
        true
    }
}

// ------------------------------------------------------------------
// A motion project on its own
// ------------------------------------------------------------------

impl Studio {
    /// A motion project opened on its own: the isolated preview, a
    /// timeline of its sounds and layers, and the Motion inspector.
    pub(crate) fn render_motion_project(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_motion_doc(window, cx);
        let Some(editor) = self.motion_doc.as_ref().map(|d| d.editor.clone()) else {
            return div().size_full().bg(theme::canvas_well()).into_any_element();
        };
        // The studio's timeline geometry (seconds per pixel, x → time) runs
        // on the project's length.
        self.video_duration = editor.read(cx).project().settings.duration.max(0.1);
        let top_bar = self.top_bar(cx);
        let (width, height) = self.canvas_budget(window.viewport_size());
        let canvas = self.canvas_area(self.motion_isolate_canvas(width, height).unwrap_or_else(|| div().into_any_element()));
        let timeline = self.motion_project_timeline(&editor, cx);
        let inspector = self.inspector_visible.then(|| {
            let content = self.motion_layer_inspector(cx).unwrap_or_else(|| self.motion_project_summary(&editor, cx));
            div()
                .id("motion-project-inspector")
                .w(px(320.0))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .border_l_1()
                .border_color(theme::line())
                .bg(theme::panel())
                .p_4()
                .child(content)
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas_well())
            .text_color(theme::ink())
            .font_family("Inter")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| this.motion_project_key(e, cx)))
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                if !e.dragging() {
                    return;
                }
                if this.motion_scrub {
                    this.motion_project_seek(e.position.x, cx);
                } else if this.motion_layer_drag.is_some() && this.update_motion_layer_drag(e.position, cx) {
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &gpui::MouseUpEvent, _, cx| this.end_motion_project_pointer(e, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, e: &gpui::MouseUpEvent, _, cx| this.end_motion_project_pointer(e, cx)))
            .child(top_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().h_full().flex().flex_col().child(canvas).child(timeline))
                    .children(inspector),
            )
            .into_any_element()
    }

    fn end_motion_project_pointer(&mut self, e: &gpui::MouseUpEvent, cx: &mut Context<Self>) {
        self.motion_scrub = false;
        self.finish_motion_layer_drag(e.position, e.modifiers.shift, cx);
        cx.notify();
    }

    fn motion_project_seek(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let (Some(t), Some(editor)) = (self.motion_timeline_time_at(x), self.motion_doc.as_ref().map(|d| d.editor.clone())) else { return };
        editor.update(cx, |e, _| e.seek(t));
        cx.notify();
    }

    fn motion_project_key(&mut self, e: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let Some(editor) = self.motion_doc.as_ref().map(|d| d.editor.clone()) else { return };
        let k = &e.keystroke;
        let command = k.modifiers.control || k.modifiers.platform;
        match k.key.as_str() {
            "space" => editor.update(cx, |e, cx| e.toggle_play(cx)),
            "left" => editor.update(cx, |e, _| e.step(if k.modifiers.shift { -10 } else { -1 })),
            "right" => editor.update(cx, |e, _| e.step(if k.modifiers.shift { 10 } else { 1 })),
            "home" => editor.update(cx, |e, _| e.seek(0.0)),
            "end" => editor.update(cx, |e, _| e.seek(f64::MAX)),
            "z" if command => {
                self.undo_motion_doc(k.modifiers.shift, cx);
            }
            "y" if command => {
                self.undo_motion_doc(true, cx);
            }
            "delete" | "backspace" => {
                let Some(id) = editor.read(cx).selected().map(str::to_string) else { return };
                self.select_motion_layer(None, cx);
                self.motion_commit(vec![json!({"op": "remove", "id": id})], cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The project's settings, when nothing is selected.
    fn motion_project_summary(&self, editor: &Entity<MotionEditor>, cx: &mut Context<Self>) -> AnyElement {
        let p = editor.read(cx).project();
        let s = &p.settings;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(self.motion_doc_name().unwrap_or_default()))
            .child(div().text_xs().text_color(theme::muted()).child(format!("{} × {} · {} fps · {:.2} s · {} scenes", s.width, s.height, s.fps.as_f64(), s.duration, p.scenes.len())))
            .child(div().text_xs().text_color(theme::muted()).child("Click a layer in the preview or on the timeline to edit it. Space plays; Ctrl+Z undoes."))
            .into_any_element()
    }

    /// Transport, ruler and the project's rows, on the studio's timeline
    /// geometry.
    fn motion_project_timeline(&mut self, editor: &Entity<MotionEditor>, cx: &mut Context<Self>) -> AnyElement {
        const LABELS: f32 = 168.0;
        let (time, frame, playing, duration) = {
            let e = editor.read(cx);
            (e.time(), e.frame_at_playhead(), e.playing(), e.project().settings.duration)
        };
        let spp = self.video_timeline_seconds_per_pixel();
        let scroll = self.video_timeline_scroll;
        let x_of = |t: f64| (t / spp - scroll) as f32;
        let sound_open = matches!(self.timeline_ui.context, Some((crate::timeline_ui::TimelineContextTarget::SoundEffects, _)));
        let to_start = {
            let editor = editor.clone();
            cx.listener(move |_, _: &gpui::ClickEvent, _, cx| editor.update(cx, |e, _| e.seek(0.0)))
        };
        let to_end = {
            let editor = editor.clone();
            cx.listener(move |_, _: &gpui::ClickEvent, _, cx| editor.update(cx, |e, _| e.seek(f64::MAX)))
        };
        let menu = if sound_open { self.render_timeline_context_menu(cx) } else { None };
        let play = {
            let editor = editor.clone();
            cx.listener(move |_, _: &gpui::ClickEvent, _, cx| editor.update(cx, |e, cx| e.toggle_play(cx)))
        };
        let transport = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(44.0))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(
                        button("motion-project-sound", "+ Sound", ButtonKind::Secondary, ButtonSize::Sm)
                            .selected(sound_open)
                            .tooltip("Add a sound effect at the playhead")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.timeline_ui.context = (!sound_open).then_some((crate::timeline_ui::TimelineContextTarget::SoundEffects, Default::default()));
                                cx.notify();
                            })),
                    )
                    .children(menu),
            )
            .child(div().flex_1())
            .child(crate::controls::icon_button("motion-project-start", "icons/skip-back.svg", ButtonKind::Ghost, ButtonSize::Sm).tooltip("Start (Home)").on_click(to_start))
            .child(crate::controls::icon_button("motion-project-play", if playing { "icons/pause.svg" } else { "icons/play.svg" }, ButtonKind::Secondary, ButtonSize::Sm).tooltip("Play / pause (Space)").on_click(play))
            .child(crate::controls::icon_button("motion-project-end", "icons/skip-forward.svg", ButtonKind::Ghost, ButtonSize::Sm).tooltip("End (End)").on_click(to_end))
            .child(div().flex_1())
            .child(div().text_xs().text_color(theme::muted()).child(format!("{time:.2} s · f{frame} / {duration:.2} s")));
        // Ruler: ticks every "nice" step of about 80 px; press or drag seeks.
        let bounds_store = self.video_timeline_bounds.clone();
        let width = self.video_timeline_bounds.lock().ok().and_then(|b| *b).map_or(800.0, |b| f32::from(b.size.width));
        let step = [0.04, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| *s >= 80.0 * spp).unwrap_or(60.0);
        let mut ruler = div()
            .id("motion-project-ruler")
            .relative()
            .flex_1()
            .h(px(24.0))
            .overflow_hidden()
            .cursor_text()
            .child(
                canvas(
                    move |bounds, window, _| {
                        if let Ok(mut stored) = bounds_store.lock() {
                            if *stored != Some(bounds) {
                                *stored = Some(bounds);
                                window.refresh();
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                this.motion_scrub = true;
                this.motion_project_seek(e.position.x, cx);
            }));
        let mut t = (scroll * spp / step).floor() * step;
        while t <= duration + 1e-9 && x_of(t) <= width {
            let x = x_of(t);
            if x >= 0.0 {
                ruler = ruler
                    .child(div().absolute().left(px(x)).top(px(14.0)).h(px(10.0)).w(px(1.0)).bg(theme::line()))
                    .child(div().absolute().left(px(x + 3.0)).top(px(2.0)).text_size(px(10.0)).text_color(theme::muted()).child(format!("{t:.2}")));
            }
            t += step;
        }
        let rows = div()
            .id("motion-project-rows")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .child(div().pl_3().flex_none().children(self.motion_layer_labels(LABELS - 12.0, cx)))
                    .child(div().flex_1().min_w_0().children(self.motion_layer_lane(scroll, cx))),
            );
        let playhead = LABELS + x_of(time);
        div()
            .relative()
            .h(px(300.0))
            .flex_none()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme::line())
            .bg(theme::panel())
            .child(transport)
            .child(div().flex().child(div().w(px(LABELS)).flex_none()).child(ruler))
            .child(rows)
            .child(div().absolute().top(px(44.0)).bottom_0().left(px(playhead - 1.0)).w(px(2.0)).bg(theme::brand()))
            .into_any_element()
    }
}
