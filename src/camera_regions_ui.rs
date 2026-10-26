//! Source-time webcam layout regions and their editor interactions.
use crate::{
    muted,
    recording::{camera_timeline::CameraRegion, scene::CameraOverlay},
    Studio, VideoEditSnapshot, VideoZoomDragKind,
};
use gpui::{
    div, hsla, prelude::*, px, rgb, AnyElement, Context, CursorStyle, FontWeight, IntoElement,
    MouseButton, MouseDownEvent, Pixels,
};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct CameraRegionDrag {
    original: Vec<CameraRegion>,
    region: CameraRegion,
    kind: VideoZoomDragKind,
    start_x: Pixels,
    start_scroll: f64,
    editor_start: f64,
    editor_end: f64,
    seconds_per_pixel: f64,
    snap_targets: Vec<f64>,
}

impl Studio {
    pub(crate) fn camera_edit_snapshot(&self) -> VideoEditSnapshot {
        VideoEditSnapshot::CameraLayouts {
            base: self.camera_overlay,
            regions: self.camera_regions.clone(),
            selected: self.selected_camera_region,
        }
    }

    pub(crate) fn record_camera_edit(&mut self) {
        self.video_undo_stack.push(self.camera_edit_snapshot());
        self.video_redo_stack.clear();
    }

    pub(crate) fn camera_edit_overlay(&self) -> CameraOverlay {
        self.selected_camera_region
            .and_then(|id| self.camera_regions.iter().find(|r| r.id == id))
            .map_or(self.camera_overlay, |r| r.overlay)
    }

    pub(crate) fn edit_camera_overlay(&mut self, edit: impl FnOnce(&mut CameraOverlay)) {
        let before = self.camera_edit_overlay();
        let mut after = before;
        edit(&mut after);
        after.pose = None;
        if before == after {
            return;
        }
        // Sliders record the pre-gesture state on mouse-down; steps and
        // segmented controls each create one undoable edit here.
        if self.slider_drag.is_none() && self.camera_drag.is_none() {
            self.record_camera_edit();
        }
        if let Some(region) = self
            .selected_camera_region
            .and_then(|id| self.camera_regions.iter_mut().find(|r| r.id == id))
        {
            region.overlay = after;
        } else {
            self.camera_overlay = after;
        }
    }

    pub(crate) fn select_camera_region(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.pause_video_playback();
        self.selected_camera_region = Some(id);
        self.video_selected_zoom_cue = None;
        self.video_selected_clip = None;
        self.finish_annotation_interaction();
        self.select_inspector_tab(crate::shell_ui::InspectorTab::Camera);
        cx.notify();
    }

    pub(crate) fn add_camera_region(&mut self, editor_time: f64, cx: &mut Context<Self>) {
        if self.video_camera_path.is_none() || self.video_edit_busy {
            return;
        }
        let Some(location) = self.video_clip_timeline.location_at(editor_time) else {
            return;
        };
        let start = location.source_time;
        if let Some(region) = self
            .camera_regions
            .iter()
            .find(|r| r.valid() && start >= r.start && start < r.end)
        {
            self.select_camera_region(region.id, cx);
            return;
        }
        let limit = self.video_clip_timeline.segments[location.segment_index].source_end;
        let next = self
            .camera_regions
            .iter()
            .filter(|r| r.valid() && r.start > start)
            .map(|r| r.start)
            .min_by(f64::total_cmp)
            .unwrap_or(limit);
        let end = (start + 3.0).min(next).min(limit);
        if end - start < CameraRegion::MINIMUM_DURATION {
            return;
        }
        self.record_camera_edit();
        let mut overlay = self.camera_edit_overlay();
        overlay.pose = None;
        let region = CameraRegion {
            start,
            end,
            overlay,
            ..Default::default()
        };
        let id = region.id;
        self.camera_regions.push(region);
        self.select_camera_region(id, cx);
        self.autosave_scene_style();
    }

    pub(crate) fn remove_selected_camera_region(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.selected_camera_region else {
            return false;
        };
        if !self.camera_regions.iter().any(|r| r.id == id) {
            self.selected_camera_region = None;
            return false;
        }
        self.record_camera_edit();
        self.camera_regions.retain(|r| r.id != id);
        self.selected_camera_region = None;
        self.autosave_scene_style();
        cx.notify();
        true
    }

    fn duplicate_camera_region(&mut self, cx: &mut Context<Self>) {
        let Some(mut copy) = self
            .selected_camera_region
            .and_then(|id| self.camera_regions.iter().find(|r| r.id == id))
            .cloned()
        else {
            return;
        };
        let length = copy.end - copy.start;
        let mut start = copy.end;
        let mut occupied: Vec<_> = self.camera_regions.iter().filter(|r| r.valid()).collect();
        occupied.sort_by(|a, b| a.start.total_cmp(&b.start));
        for region in occupied {
            if start < region.end && start + length > region.start {
                start = region.end;
            }
        }
        if start + CameraRegion::MINIMUM_DURATION > self.video_source_duration {
            return;
        }
        self.record_camera_edit();
        copy.id = Uuid::new_v4();
        copy.start = start;
        copy.end = (start + length).min(self.video_source_duration);
        self.selected_camera_region = Some(copy.id);
        self.camera_regions.push(copy);
        self.autosave_scene_style();
        cx.notify();
    }

    fn change_camera_transition(&mut self, seconds: f64, cx: &mut Context<Self>) {
        let Some(id) = self.selected_camera_region else {
            return;
        };
        let Some(region) = self.camera_regions.iter().find(|r| r.id == id) else {
            return;
        };
        let duration = seconds
            .clamp(0.0, 2.0)
            .min((region.end - region.start) * 0.5);
        if (region.transition - duration).abs() < 1e-9 {
            return;
        }
        self.record_camera_edit();
        self.camera_regions
            .iter_mut()
            .find(|r| r.id == id)
            .unwrap()
            .transition = duration;
        self.autosave_scene_style();
        cx.notify();
    }

    pub(crate) fn camera_regions_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self
            .selected_camera_region
            .and_then(|id| self.camera_regions.iter().find(|r| r.id == id));
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Camera layouts"),
            )
            .child(self.small_button(
                "camera-layout-add",
                "Add layout at playhead",
                self.video_camera_path.is_some(),
                cx,
                |this, cx| this.add_camera_region(this.video_position, cx),
            ))
            .child(self.small_button(
                "camera-layout-base",
                if selected.is_some() {
                    "Edit project default"
                } else {
                    "Editing project default"
                },
                true,
                cx,
                |this, cx| {
                    this.selected_camera_region = None;
                    cx.notify();
                },
            ));
        if let Some(region) = selected {
            let duration = region.transition;
            panel=panel.child(div().rounded_lg().p_2().bg(rgb(0xf2eafa)).flex().flex_col().gap_2()
                .child(div().text_xs().child(format!("Selected layout · {:.2}s–{:.2}s",region.start,region.end)))
                .child(self.segmented("camera-layout-transition", &["Instant","Quick","Smooth"],
                    if duration<=0.0 {0}else if duration<=0.2 {1}else{2},
                    |this,index| {
                        if let Some(id)=this.selected_camera_region {
                            this.record_camera_edit();
                            if let Some(region)=this.camera_regions.iter_mut().find(|r|r.id==id) {
                                region.transition=[0.0_f64,0.18,0.35][index].min((region.end-region.start)*0.5);
                            }
                        }
                    },cx))
                .child(div().flex().gap_1()
                    .child(self.small_button("camera-transition-less","−",duration>0.0,cx,move|this,cx|this.change_camera_transition(duration-0.05,cx)))
                    .child(div().flex_1().text_xs().child(format!("Transition {:.2}s",duration)))
                    .child(self.small_button("camera-transition-more","+",duration<2.0,cx,move|this,cx|this.change_camera_transition(duration+0.05,cx))))
                .child(div().flex().gap_2()
                    .child(self.small_button("camera-layout-copy","Duplicate",true,cx,|this,cx|this.duplicate_camera_region(cx)))
                    .child(self.small_button("camera-layout-delete","Remove",true,cx,|this,cx|{this.remove_selected_camera_region(cx);}))))
                .child(div().text_xs().text_color(muted()).child("These controls affect the selected purple region. Drag its middle to move it or its edges to trim."));
        } else {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(muted())
                    .child("The project default applies outside layout regions."),
            );
        }
        for (index, region) in self.camera_regions.iter().enumerate() {
            let id = region.id;
            let start = region.start;
            panel = panel.child(
                div()
                    .id(("camera-layout-row", index))
                    .rounded_md()
                    .p_2()
                    .cursor_pointer()
                    .bg(if self.selected_camera_region == Some(id) {
                        rgb(0xe9dbf7)
                    } else {
                        rgb(0xf1f1f2)
                    })
                    .hover(|s| s.bg(rgb(0xe9dbf7)))
                    .text_xs()
                    .child(format!(
                        "{} · {:.2}–{:.2}s",
                        region.overlay.layout.label(),
                        region.start,
                        region.end
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_camera_region(id, cx);
                        if let Some(time) = this
                            .video_clip_timeline
                            .editor_time_for_event(start + 0.001)
                        {
                            this.seek_video_frame(time, cx);
                        }
                    })),
            );
        }
        panel.into_any_element()
    }

    fn begin_camera_region_drag(
        &mut self,
        id: Uuid,
        kind: VideoZoomDragKind,
        x: Pixels,
        start: f64,
        end: f64,
        cx: &mut Context<Self>,
    ) {
        let Some(region) = self.camera_regions.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        self.select_camera_region(id, cx);
        self.video_seek_drag = None;
        self.video_zoom_drag = None;
        self.video_trim_drag = None;
        self.video_move_drag = None;
        let mut snap_targets = self.timeline_snap_targets(None, None, None);
        for other in self
            .camera_regions
            .iter()
            .filter(|other| other.id != id && other.valid())
        {
            for slice in self
                .video_clip_timeline
                .slices_overlapping(other.start, other.end)
            {
                snap_targets.extend([slice.editor_start, slice.editor_end]);
            }
        }
        snap_targets.sort_by(f64::total_cmp);
        snap_targets.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        self.camera_region_drag = Some(CameraRegionDrag {
            original: self.camera_regions.clone(),
            region,
            kind,
            start_x: x,
            start_scroll: self.video_timeline_scroll,
            editor_start: start,
            editor_end: end,
            seconds_per_pixel: self.video_timeline_seconds_per_pixel(),
            snap_targets,
        });
    }

    pub(crate) fn update_camera_region_drag(&mut self, x: Pixels) {
        let Some(drag) = self.camera_region_drag.clone() else {
            return;
        };
        let delta = (f64::from((x - drag.start_x) / px(1.0)) + self.video_timeline_scroll
            - drag.start_scroll)
            * drag.seconds_per_pixel;
        let duration = drag.editor_end - drag.editor_start;
        let (edge, length) = match drag.kind {
            VideoZoomDragKind::Move => (drag.editor_start + delta, duration),
            VideoZoomDragKind::Leading => (drag.editor_start + delta, 0.0),
            VideoZoomDragKind::Trailing => (drag.editor_end + delta, 0.0),
        };
        let (snapped, guide) = crate::timeline_edit::snap_timeline_edges(
            edge,
            length,
            &drag.snap_targets,
            drag.seconds_per_pixel,
            self.timeline_ui.snap_enabled,
        );
        let mut region = drag.region.clone();
        match drag.kind {
            VideoZoomDragKind::Move => {
                let source = self
                    .video_clip_timeline
                    .source_time_at(snapped.clamp(0.0, (self.video_duration - duration).max(0.0)));
                let length = region.end - region.start;
                region.start = source.clamp(0.0, (self.video_source_duration - length).max(0.0));
                region.end = region.start + length;
            }
            VideoZoomDragKind::Leading => {
                region.start = self
                    .video_clip_timeline
                    .source_time_at(snapped.clamp(0.0, drag.editor_end - 1e-6))
                    .clamp(0.0, region.end - CameraRegion::MINIMUM_DURATION)
            }
            VideoZoomDragKind::Trailing => {
                region.end = self
                    .video_clip_timeline
                    .source_time_at(snapped.clamp(drag.editor_start + 1e-6, self.video_duration))
                    .clamp(
                        region.start + CameraRegion::MINIMUM_DURATION,
                        self.video_source_duration,
                    )
            }
        }
        // Prevent a drag from silently covering another layout. Loaded overlaps
        // still resolve deterministically, but authored lanes stay unambiguous.
        for other in drag
            .original
            .iter()
            .filter(|r| r.id != region.id && r.valid())
        {
            if region.start < other.end && region.end > other.start {
                self.timeline_ui.snap_guide = None;
                return;
            }
        }
        self.timeline_ui.snap_guide = guide;
        if let Some(current) = self.camera_regions.iter_mut().find(|r| r.id == region.id) {
            *current = region;
        }
    }

    pub(crate) fn finish_camera_region_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.camera_region_drag.take() else {
            return false;
        };
        if drag.original != self.camera_regions {
            self.video_undo_stack
                .push(VideoEditSnapshot::CameraLayouts {
                    base: self.camera_overlay,
                    regions: drag.original,
                    selected: self.selected_camera_region,
                });
            self.video_redo_stack.clear();
            self.autosave_scene_style();
        }
        self.timeline_ui.snap_guide = None;
        cx.notify();
        true
    }

    pub(crate) fn cancel_camera_region_drag(&mut self) -> bool {
        let Some(drag) = self.camera_region_drag.take() else {
            return false;
        };
        self.camera_regions = drag.original;
        self.timeline_ui.snap_guide = None;
        true
    }

    pub(crate) fn camera_region_lane(
        &self,
        scroll: f64,
        width: f64,
        progress: f64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.video_camera_path.as_ref()?;
        let mut lane = div()
            .id("camera-layout-lane")
            .relative()
            .w_full()
            .h(px(28.0))
            .overflow_hidden()
            .rounded_lg()
            .bg(rgb(0xeeeaf3))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        if let Some(time) = this.motion_timeline_time_at(event.position.x) {
                            this.add_camera_region(time, cx);
                        }
                        cx.stop_propagation();
                    }
                }),
            );
        for (index, region) in self.camera_regions.iter().enumerate() {
            for (slice_index, slice) in self
                .video_clip_timeline
                .slices_overlapping(region.start, region.end)
                .iter()
                .enumerate()
            {
                let (id, start, end) = (region.id, slice.editor_start, slice.editor_end);
                let selected = self.selected_camera_region == Some(id);
                let mut item = div()
                    .id(("camera-region", index * 4096 + slice_index))
                    .absolute()
                    .left(px(
                        (start / self.video_timeline_duration() * width - scroll) as f32,
                    ))
                    .top(px(3.0))
                    .w(px(
                        ((end - start) / self.video_timeline_duration() * width) as f32
                    ))
                    .h(px(22.0))
                    .min_w(px(8.0))
                    .rounded_md()
                    .overflow_hidden()
                    .bg(if selected {
                        rgb(0x8842c4)
                    } else {
                        rgb(0xa36ad0)
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(0x5d2090)
                    } else {
                        rgb(0xb886df)
                    })
                    .cursor(CursorStyle::OpenHand)
                    .text_xs()
                    .text_color(rgb(0xffffff))
                    .px_2()
                    .flex()
                    .items_center()
                    .child(region.overlay.layout.label())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.begin_camera_region_drag(
                                id,
                                VideoZoomDragKind::Move,
                                event.position.x,
                                start,
                                end,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    );
                for (trailing, kind) in [
                    (false, VideoZoomDragKind::Leading),
                    (true, VideoZoomDragKind::Trailing),
                ] {
                    item = item.child(
                        div()
                            .id((
                                "camera-region-edge",
                                (index * 4096 + slice_index) * 2 + usize::from(trailing),
                            ))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .w(px(7.0))
                            .when(trailing, |d| d.right_0())
                            .when(!trailing, |d| d.left_0())
                            .cursor(CursorStyle::ResizeLeftRight)
                            .hover(|s| s.bg(hsla(0.0, 0.0, 1.0, 0.25)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    this.begin_camera_region_drag(
                                        id,
                                        kind,
                                        event.position.x,
                                        start,
                                        end,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            ),
                    );
                }
                lane = lane.child(item);
            }
        }
        if self.camera_regions.is_empty() {
            lane = lane.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(muted())
                    .child("Camera · double-click to add a layout"),
            );
        }
        Some(
            lane.child(
                div()
                    .absolute()
                    .left(px((width * progress - scroll) as f32 - 1.0))
                    .top_0()
                    .w(px(2.0))
                    .h_full()
                    .bg(rgb(0x30343b)),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a display (run under Xvfb); uses synthetic frames, never captures devices"]
    fn camera_layout_edit_move_trim_undo_and_reopen_workflow() {
        use crate::{
            asset_directory, open_studio_window,
            recording::{
                camera_layout::CameraLayout, clips::RecordingClipTimeline, model::RecordingSession,
                scene::SceneStyle,
            },
            Assets,
        };
        let root = std::env::temp_dir().join(format!("lahza-camera-workflow-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        gpui::Application::new()
            .with_assets(Assets {
                base: asset_directory(),
            })
            .run(move |cx| {
                let handle = open_studio_window(cx, true, |handle, cx| {
                    cx.new(|cx| Studio::new(handle, None, None, cx))
                })
                .unwrap();
                handle
                    .update(cx, |studio, _, cx| {
                        studio.load_generation = studio.load_generation.wrapping_add(1);
                        studio.loading = None;
                        studio.launcher_active = false;
                        studio.launcher_window = false;
                        studio.record_camera = false;
                        studio.video_project = Some(session.clone());
                        studio.video_camera_path = Some(session.camera_path());
                        studio.camera_decode_in_flight = true;
                        studio.video_clip_timeline = RecordingClipTimeline::full(12.0);
                        studio.video_duration = 12.0;
                        studio.video_source_duration = 12.0;
                        studio.video_recording_duration = 12.0;
                        studio.video_timeline_span = 12.0;
                        studio.video_timeline_extent = 12.0;
                        studio.set_video_frame(image::RgbaImage::from_pixel(
                            320,
                            180,
                            image::Rgba([40, 60, 90, 255]),
                        ));
                        studio.camera_frame_rgba = Some(std::sync::Arc::new(
                            image::RgbaImage::from_pixel(80, 60, image::Rgba([200, 100, 80, 255])),
                        ));
                        studio.add_camera_region(2.0, cx);
                        let id = studio.selected_camera_region.unwrap();
                        assert_eq!(
                            (studio.camera_regions[0].start, studio.camera_regions[0].end),
                            (2.0, 5.0)
                        );
                        studio.edit_camera_overlay(|camera| {
                            camera.layout = CameraLayout::Split;
                            camera.split_full = true;
                        });
                        assert_eq!(studio.camera_overlay.layout, CameraLayout::Overlay);
                        assert_eq!(studio.camera_regions[0].overlay.layout, CameraLayout::Split);
                        studio.undo_video_edit(cx);
                        assert_eq!(
                            studio.camera_regions[0].overlay.layout,
                            CameraLayout::Overlay
                        );
                        studio.redo_video_edit(cx);
                        assert!(studio.camera_regions[0].overlay.split_full);
                        studio.timeline_ui.snap_enabled = false;
                        let scale = studio.video_timeline_seconds_per_pixel();
                        studio.begin_camera_region_drag(
                            id,
                            VideoZoomDragKind::Move,
                            px(0.0),
                            2.0,
                            5.0,
                            cx,
                        );
                        studio.update_camera_region_drag(px((1.0 / scale) as f32));
                        studio.finish_camera_region_drag(cx);
                        assert!((studio.camera_regions[0].start - 3.0).abs() < 1e-5);
                        studio.begin_camera_region_drag(
                            id,
                            VideoZoomDragKind::Trailing,
                            px(0.0),
                            3.0,
                            6.0,
                            cx,
                        );
                        studio.update_camera_region_drag(px((-1.0 / scale) as f32));
                        studio.finish_camera_region_drag(cx);
                        assert!((studio.camera_regions[0].end - 5.0).abs() < 1e-5);
                        studio.undo_video_edit(cx);
                        assert!((studio.camera_regions[0].end - 6.0).abs() < 1e-5);
                        studio.redo_video_edit(cx);
                        studio.duplicate_camera_region(cx);
                        assert_eq!(studio.camera_regions.len(), 2);
                        studio.remove_selected_camera_region(cx);
                        assert_eq!(studio.camera_regions.len(), 1);
                        studio.undo_video_edit(cx);
                        assert_eq!(studio.camera_regions.len(), 2);
                        studio.autosave_scene_style();
                        let saved = session
                            .read_edit_field::<SceneStyle>("scene")
                            .unwrap()
                            .unwrap();
                        let expected = studio.camera_regions.clone();
                        studio.camera_regions.clear();
                        studio.selected_camera_region = None;
                        studio.apply_scene_style(&saved);
                        assert_eq!(studio.camera_regions, expected);
                        assert_eq!(
                            studio
                                .scene_style()
                                .camera_at(4.0, (1920, 1080), 4.0 / 3.0, 1.0)
                                .layout,
                            CameraLayout::Split
                        );
                        studio.video_source_size = (1600, 900);
                        studio.video_position = 4.0;
                        let (resolved, geometry, viewport) =
                            studio.preview_geometry_frame(960.0, 540.0);
                        assert!(resolved.camera.full_bleed_strength() > 0.99);
                        let projection =
                            geometry.projection(resolved.transform.with_motion(viewport));
                        let (x, y) = projection.project(0.0, 0.5);
                        let canvas = gpui::Bounds {
                            origin: gpui::point(px(0.0), px(0.0)),
                            size: gpui::size(px(960.0), px(540.0)),
                        };
                        let source_point = studio
                            .media_point_at(gpui::point(px(x as f32 + 0.01), px(y as f32)), canvas)
                            .unwrap();
                        assert!(
                            source_point.x > 0.2 && source_point.x < 0.3,
                            "full split picking must use the cover crop"
                        );
                        studio.video_camera_path = None;
                        studio.video_project = None;
                    })
                    .unwrap();
                cx.spawn(async move |cx| {
                    async_io::Timer::after(std::time::Duration::from_millis(30)).await;
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
            });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires X11/Xvfb and ffmpeg; offline synthetic screen and public-domain portrait, no device capture"]
    fn timed_camera_cutout_and_blur_playback_stays_responsive() {
        use crate::{
            asset_directory,
            capture::PreparedVideoProject,
            open_studio_window,
            recording::{
                camera_layout::CameraLayout,
                model::{CaptureManifest, RecordingSession},
                scene::SceneStyle,
            },
            Assets,
        };
        use std::time::{Duration, Instant};
        let root = std::env::temp_dir().join(format!("lahza-camera-playback-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        let run = |args: &[&str], output: &std::path::Path| {
            let result = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(args)
                .args([
                    "-an",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                ])
                .arg(output)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=30:duration=8",
            ],
            &session.screen_path(),
        );
        run(
            &[
                "-loop",
                "1",
                "-i",
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/astronaut.png"),
                "-vf",
                "crop=480:480:16+12*sin(n/5):16,scale=320:320",
                "-t",
                "8",
                "-r",
                "30",
            ],
            &session.camera_path(),
        );
        session
            .write_manifest(&CaptureManifest {
                duration: 8.0,
                pixel_width: 640,
                pixel_height: 360,
                includes_camera: true,
                ..Default::default()
            })
            .unwrap();
        let mut style = SceneStyle::default();
        style.animation.motion_blur = 70;
        style.camera.enabled = false;
        for (start, end, layout) in [
            (0.0, 1.6, CameraLayout::Cutout),
            (1.6, 2.4, CameraLayout::Split),
            (2.4, 3.2, CameraLayout::Hidden),
            (3.2, 4.0, CameraLayout::Fullscreen),
            (4.0, 7.5, CameraLayout::Cutout),
        ] {
            style.camera_regions.push(CameraRegion {
                start,
                end,
                transition: 0.25,
                overlay: CameraOverlay {
                    enabled: true,
                    layout,
                    split_full: true,
                    size: 45,
                    ..Default::default()
                },
                ..Default::default()
            });
        }
        session.write_edit_field("scene", &style).unwrap();
        let prepared = PreparedVideoProject::load(session.directory.clone()).unwrap();
        let cleanup = root.clone();
        gpui::Application::new().with_assets(Assets {base:asset_directory()}).run(move|cx| {
            let handle=open_studio_window(cx,true,|handle,cx|cx.new(|cx|Studio::new(handle,None,None,cx))).unwrap();
            handle.update(cx,|studio,window,cx| {
                studio.load_generation=studio.load_generation.wrapping_add(1);studio.loading=None;
                studio.launcher_active=false;studio.launcher_window=false;studio.record_camera=false;
                studio.apply_video_project(prepared);studio.video_position=0.4;
                studio.select_inspector_tab(crate::shell_ui::InspectorTab::Camera);
                window.activate_window();window.refresh();cx.notify();
            }).unwrap();
            cx.activate(true);
            cx.spawn(async move|cx| {
                async_io::Timer::after(Duration::from_millis(200)).await;
                handle.update(cx,|studio,_,cx|{studio.start_video_playback(cx);cx.notify();}).unwrap();
                let warmup=Instant::now();
                loop {
                    async_io::Timer::after(Duration::from_millis(25)).await;
                    let ready=handle.update(cx,|studio,_,_| {
                        assert!(studio.video_playing,"camera transport stopped during startup");
                        studio.preview_cache.frame.as_ref().is_some_and(|(key,_)|!key.high_quality&&key.time()>0.7)
                    }).unwrap();
                    if ready {break;}
                    assert!(warmup.elapsed()<Duration::from_secs(5),"camera preview failed to start");
                }
                let mut frames=Vec::new();let mut max_lag=0.0_f64;let mut max_wake=Duration::ZERO;
                let mut saw_cutout=false;let mut saw_hidden=false;let mut saw_split=false;let mut canvas=(0,0);
                let mut lag_samples=Vec::new();
                let start=Instant::now();
                while start.elapsed()<Duration::from_secs(5) {
                    let scheduled=Instant::now();async_io::Timer::after(Duration::from_millis(25)).await;
                    max_wake=max_wake.max(scheduled.elapsed());
                    handle.update(cx,|studio,_,_| {
                        assert!(studio.camera_preview.is_none(),"offline playback must not capture a webcam");
                        if let Some((key,_))=&studio.preview_cache.frame {
                            if !key.high_quality && key.time()>0.4 {
                                frames.push(key.time());canvas=key.canvas;
                                max_lag=max_lag.max(studio.video_position-key.time());
                                lag_samples.push((studio.video_position-key.time(),key.time(),key.motion.as_ref().map_or(1,|plan|plan.samples.len())));
                                let source=studio.video_clip_timeline.content_source_time_at(key.time()).unwrap();
                                let overlay=style.camera_at(source,key.canvas,1.0,1.0);
                                saw_cutout|=overlay.cutout_strength()>0.99;
                                saw_hidden|=overlay.opacity()<0.01;
                                saw_split|=overlay.full_bleed_strength()>0.99;
                            }
                        }
                    }).unwrap();
                }
                frames.dedup_by(|a,b|a.to_bits()==b.to_bits());
                lag_samples.sort_by(|a,b|b.0.total_cmp(&a.0));
                eprintln!("camera playback canvas {canvas:?}: {} distinct sampled frames, max lag {max_lag:.3}s, max25ms timer {max_wake:?}, worst {:?}",frames.len(),&lag_samples[..lag_samples.len().min(5)]);
                assert!(frames.len()>60,"camera preview stopped advancing: {} distinct frames",frames.len());
                assert!(saw_cutout&&saw_hidden&&saw_split,"timed layouts were not presented");
                assert!(max_lag<0.25,"camera preview lagged by {max_lag}s");
                assert!(max_wake<Duration::from_millis(500),"UI executor stalled for {max_wake:?}");
                eprintln!("camera playback canvas {canvas:?}: {} distinct sampled frames, max lag {max_lag:.3}s, max25ms timer {max_wake:?}",frames.len());
                handle.update(cx,|studio,_,cx| {studio.pause_video_playback();studio.video_project=None;studio.video_camera_path=None;cx.notify();}).unwrap();
                cx.update(|cx|cx.quit()).unwrap();
                std::fs::remove_dir_all(cleanup).unwrap();
            }).detach();
        });
    }
}
