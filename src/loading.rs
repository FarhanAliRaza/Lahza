//! Responsive startup and document loading, before the editor is ready to paint.
use crate::{capture::PreparedVideoProject, Studio};
use gpui::{canvas, div, point, prelude::*, px, rgb, AnyElement, Context, FontWeight, PathBuilder, Window};
use std::{path::PathBuf, time::Instant};

#[derive(Clone)]
pub(crate) enum LoadRequest {
    Launch,
    Image(PathBuf),
    Recording(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    async fn settle(handle: gpui::WindowHandle<Studio>, cx: &mut gpui::AsyncApp) {
        for _ in 0..250 {
            let ready = handle.update(cx, |studio, _, _| {
                studio.loading.as_ref().is_none_or(|loading| loading.error.is_some())
            }).unwrap();
            if ready { return; }
            Timer::after(Duration::from_millis(20)).await;
        }
        panic!("document loading did not settle");
    }

    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn loading_keeps_existing_content_on_failure_and_can_retry_or_cancel() {
        let dir = std::env::temp_dir().join(format!("lahza-loading-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let first = dir.join("first.png");
        let second = dir.join("second.png");
        let broken = dir.join("broken.png");
        image::RgbaImage::new(48, 32).save(&first).unwrap();
        image::RgbaImage::new(96, 64).save(&second).unwrap();
        std::fs::write(&broken, b"not an image").unwrap();
        Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
            let handle = open_studio_window(cx, true, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            }).unwrap();
            handle.update(cx, |studio, _, cx| {
                studio.start_loading(LoadRequest::Image(first.clone()), false, cx);
                assert!(studio.loading.is_some());
                assert!(studio.captured_path.is_none(), "loading must yield before applying a document");
            }).unwrap();
            cx.spawn(async move |cx| {
                settle(handle, cx).await;
                handle.update(cx, |studio, _, cx| {
                    assert_eq!(studio.captured_dimensions, Some((48, 32)));
                    assert_eq!(studio.captured_path.as_ref(), Some(&first));
                    studio.start_loading(LoadRequest::Image(broken.clone()), false, cx);
                }).unwrap();
                settle(handle, cx).await;
                handle.update(cx, |studio, _, cx| {
                    assert!(studio.loading.as_ref().unwrap().error.is_some());
                    assert_eq!(studio.captured_path.as_ref(), Some(&first));
                    // Fix the source and retry the original request.
                    std::fs::copy(&second, &broken).unwrap();
                    let request = studio.loading.as_ref().unwrap().request.clone();
                    studio.start_loading(request, false, cx);
                }).unwrap();
                settle(handle, cx).await;
                handle.update(cx, |studio, _, cx| {
                    assert_eq!(studio.captured_dimensions, Some((96, 64)));
                    assert_eq!(studio.captured_path.as_ref(), Some(&broken));
                    studio.start_loading(LoadRequest::Image(first), false, cx);
                    studio.dismiss_loading(cx);
                }).unwrap();
                Timer::after(Duration::from_millis(250)).await;
                handle.update(cx, |studio, _, _| {
                    assert!(studio.loading.is_none());
                    assert_eq!(studio.captured_path.as_ref(), Some(&broken), "cancelled loads cannot replace the editor");
                }).unwrap();
                std::fs::remove_dir_all(dir).unwrap();
                cx.update(|cx| cx.quit()).unwrap();
            }).detach();
        });
    }
}

impl LoadRequest {
    fn title(&self) -> &'static str {
        match self {
            Self::Launch => "Starting Lahza",
            Self::Image(_) => "Opening image",
            Self::Recording(_) => "Opening recording",
        }
    }

    fn filename(&self) -> Option<String> {
        match self {
            Self::Launch => None,
            Self::Image(path) | Self::Recording(path) => path.file_name().map(|name| name.to_string_lossy().into_owned()),
        }
    }
}

enum PreparedOpen {
    Launch,
    Image(PathBuf, image::RgbaImage),
    Recording(PreparedVideoProject),
}

pub(crate) struct LoadingState {
    request: LoadRequest,
    started: Instant,
    error: Option<String>,
}

impl Studio {
    pub(crate) fn start_loading(&mut self, request: LoadRequest, discover_devices: bool, cx: &mut Context<Self>) {
        if self.video_playing { self.pause_video_playback(); }
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        self.loading = Some(LoadingState { request: request.clone(), started: Instant::now(), error: None });
        self.toast = None;
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let devices = discover_devices.then(|| {
                (crate::microphone_devices(), crate::camera_devices())
            });
            // Warm the export/measurement font database without blocking first paint.
            if !matches!(request, LoadRequest::Launch) { let _ = crate::fonts::shared_fontdb(); }
            let result = match request {
                LoadRequest::Launch => Ok(PreparedOpen::Launch),
                LoadRequest::Image(path) => image::open(&path)
                    .map(|image| PreparedOpen::Image(path, image.to_rgba8()))
                    .map_err(|error| format!("Could not open image: {error}")),
                LoadRequest::Recording(path) => PreparedVideoProject::load(path).map(PreparedOpen::Recording),
            };
            (devices, result)
        });
        cx.spawn(async move |weak, cx| {
            let (devices, result) = task.await;
            let _ = weak.update(cx, |this, cx| {
                if let Some((microphones, cameras)) = devices {
                    this.microphone_devices = microphones;
                    this.camera_devices = cameras;
                }
                if this.load_generation != generation { return; }
                match result {
                    Ok(prepared) => {
                        match prepared {
                            PreparedOpen::Launch => {},
                            PreparedOpen::Image(path, image) => {
                                if this.video_project.is_some() { this.close_video_editor(cx); }
                                this.apply_capture_image(path, image);
                                this.toast = None;
                            },
                            PreparedOpen::Recording(project) => {
                                this.apply_video_project(project);
                                this.launcher_active = false;
                            },
                        }
                        this.loading = None;
                    },
                    Err(error) => {
                        if let Some(loading) = &mut this.loading { loading.error = Some(error); }
                    },
                }
                cx.notify();
            });
        }).detach();
    }

    fn dismiss_loading(&mut self, cx: &mut Context<Self>) {
        self.load_generation = self.load_generation.wrapping_add(1);
        self.loading = None;
        if self.video_project.is_none() && self.captured_path.is_none() { self.launcher_active = true; }
        cx.notify();
    }

    pub(crate) fn render_loading(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.loading.as_ref().expect("loading screen requires a load");
        let error = loading.error.clone();
        let request = loading.request.clone();
        let phase = loading.started.elapsed().as_secs_f32() * 4.5;
        if error.is_none() { window.request_animation_frame(); }
        div().size_full().bg(rgb(0xf7f8fa)).text_color(crate::ink()).font_family("Inter")
            .flex().flex_col().items_center().justify_center().p_6()
            .child(div().w(px(340.)).max_w_full().flex().flex_col().items_center().gap_5()
                .child(crate::brand_wordmark_latin(180., 36.))
                .child(div().h(px(1.)).w(px(40.)).bg(rgb(0xe1e4e9)))
                .when(error.is_none(), |this| this.child(
                    canvas(|bounds, _, _| bounds, move |_, bounds, window, _| {
                        let center = bounds.center();
                        for (start, length, color) in [(0., std::f32::consts::TAU, rgb(0xe1e4e9)), (phase, 4.0, rgb(0xd03734))] {
                            let mut path = PathBuilder::stroke(px(2.5));
                            for step in 0..=48 {
                                let angle = start + length * step as f32 / 48.;
                                let p = point(center.x + px(angle.cos() * 12.), center.y + px(angle.sin() * 12.));
                                if step == 0 { path.move_to(p); } else { path.line_to(p); }
                            }
                            if let Ok(path) = path.build() { window.paint_path(path, color); }
                        }
                    }).size(px(32.))
                ))
                .child(div().text_lg().font_weight(FontWeight::MEDIUM)
                    .child(if error.is_some() { "Couldn't finish opening" } else { loading.request.title() }))
                .when_some(loading.request.filename(), |this, name| this.child(
                    div().w_full().text_center().text_sm().text_color(crate::muted()).truncate().child(name)
                ))
                .when(error.is_none() && !matches!(loading.request, LoadRequest::Launch), |this| this.child(
                    div().id("loading-cancel").px_4().py_2().rounded_md().text_sm().text_color(crate::muted())
                        .cursor_pointer().hover(|style| style.bg(rgb(0xe9ebef))).child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_loading(cx)))
                ))
                .when_some(error, |this, error| this
                    .child(div().w_full().text_sm().text_center().text_color(crate::muted()).child(error))
                    .child(div().flex().gap_3()
                        .child(div().id("loading-back").px_4().py_2().rounded_md().bg(rgb(0xe9ebef)).cursor_pointer().child("Go back")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dismiss_loading(cx);
                            })))
                        .child(div().id("loading-retry").px_4().py_2().rounded_md().bg(crate::blue()).text_color(rgb(0xffffff)).cursor_pointer().child("Try again")
                            .on_click(cx.listener(move |this, _, _, cx| this.start_loading(request.clone(), false, cx))))
                    )
                )
            ).into_any_element()
    }
}
