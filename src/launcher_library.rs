use crate::{blue, cached_render_image, ink, library, line, muted, recording, Studio};
use gpui::{div, img, prelude::*, px, svg, AnyElement, Context, ObjectFit, RenderImage};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
};

#[derive(Default)]
pub(crate) struct LibraryState {
    thumbnails: VecDeque<(PathBuf, Option<Arc<RenderImage>>)>,
    pending: HashSet<PathBuf>,
    generation: u64,
    pub loading: bool,
}

impl Studio {
    pub(crate) fn refresh_library(&mut self, cx: &mut Context<Self>) {
        if self.launcher_tab == 0 {
            return;
        }
        let projects = self.launcher_tab == 1;
        self.library_state.generation += 1;
        let generation = self.library_state.generation;
        self.library_state.loading = true;
        let task = cx.background_executor().spawn(async move {
            let root = if projects {
                recording::model::recordings_root()
            } else {
                library::screenshots_root()
            };
            library::saved_items(&root, projects)
        });
        cx.spawn(async move |weak, cx| {
            let items = task.await;
            let _ = weak.update(cx, |this, cx| {
                if generation != this.library_state.generation {
                    return;
                }
                if projects {
                    this.recent_projects = items;
                } else {
                    this.recent_screenshots = items;
                }
                this.library_state.loading = false;
                // Refresh modified images and retry failed previews on a tab refresh.
                for (_, image) in this.library_state.thumbnails.drain(..) {
                    this.retired_images.extend(image);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn library_thumbnail(
        &mut self,
        path: &PathBuf,
        projects: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut preview = None;
        if let Some(index) = self
            .library_state
            .thumbnails
            .iter()
            .position(|(key, _)| key == path)
        {
            let entry = self.library_state.thumbnails.remove(index).unwrap();
            preview = entry.1.clone();
            self.library_state.thumbnails.push_back(entry);
        } else if self.library_state.pending.len() < 2
            && self.library_state.pending.insert(path.clone())
        {
            let path = path.clone();
            let worker_path = path.clone();
            let generation = self.library_state.generation;
            let task = cx
                .background_executor()
                .spawn(async move { load_thumbnail(&worker_path, projects) });
            cx.spawn(async move |weak, cx| {
                let pixels = task.await;
                let _ = weak.update(cx, |this, cx| {
                    this.library_state.pending.remove(&path);
                    if generation == this.library_state.generation {
                        this.library_state
                            .thumbnails
                            .push_back((path, pixels.map(cached_render_image)));
                        while this.library_state.thumbnails.len() > 64 {
                            if let Some((_, image)) = this.library_state.thumbnails.pop_front() {
                                this.retire_image(image);
                            }
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        div()
            .w(px(96.0))
            .h(px(60.0))
            .flex_none()
            .rounded_md()
            .overflow_hidden()
            .bg(gpui::rgb(0xeeeeef))
            .flex()
            .items_center()
            .justify_center()
            .child(if let Some(image) = preview {
                img(image)
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element()
            } else {
                svg()
                    .path(if projects {
                        "icons/video.svg"
                    } else {
                        "icons/capture.svg"
                    })
                    .size(px(20.0))
                    .text_color(muted())
                    .into_any_element()
            })
            .into_any_element()
    }

    pub(crate) fn library_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let projects = self.launcher_tab == 1;
        let path = if projects {
            &self.recent_projects[index]
        } else {
            &self.recent_screenshots[index]
        }
        .clone();
        let thumbnail = self.library_thumbnail(&path, projects, cx);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled")
            .to_string();
        div()
            .h(px(88.0))
            .pb_2()
            .child(
                div()
                    .id(("library-item", index))
                    .h_full()
                    .px_2()
                    .rounded_lg()
                    .bg(gpui::white())
                    .border_1()
                    .border_color(line())
                    .hover(|style| style.bg(gpui::rgb(0xf3f3f5)))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(thumbnail)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().text_color(ink()).truncate().child(name))
                            .child(div().text_xs().text_color(muted()).child(if projects {
                                "Video project"
                            } else {
                                "Screenshot"
                            })),
                    )
                    .child(
                        svg()
                            .path("icons/chevron-right.svg")
                            .size(px(15.0))
                            .text_color(blue()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if projects {
                            if let Err(error) = this.open_video_project(path.clone()) {
                                this.toast = Some(error.into());
                            } else {
                                this.launcher_active = false;
                            }
                        } else {
                            this.finish_capture_request(Ok(path.clone()));
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

/// Decode off the UI thread and retain only a display-sized image.
fn load_thumbnail(path: &std::path::Path, projects: bool) -> Option<image::RgbaImage> {
    let source = if projects {
        path.join("poster.jpg")
    } else {
        path.to_path_buf()
    };
    if let Ok(image) = image::open(source) {
        return Some(image.thumbnail(192, 120).to_rgba8());
    }
    if !projects {
        return None;
    }
    let frame = recording::video::decode_frame(&path.join("screen.mkv"), 0.1, 192, 120).ok()?;
    image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_previews_preserve_aspect_ratio_and_handle_invalid_images() {
        let root = std::env::temp_dir().join(format!("lahza-thumbnails-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("screenshot.png");
        image::RgbaImage::new(1200, 800).save(&path).unwrap();
        let preview = load_thumbnail(&path, false).unwrap();
        assert_eq!(preview.dimensions(), (180, 120));
        std::fs::write(&path, b"broken image").unwrap();
        assert!(load_thumbnail(&path, false).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_poster_is_used_without_needing_video() {
        let root = std::env::temp_dir().join(format!("lahza-poster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        image::RgbImage::new(1280, 720)
            .save(root.join("poster.jpg"))
            .unwrap();
        assert_eq!(
            load_thumbnail(&root, true).unwrap().dimensions(),
            (192, 108)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_without_poster_decodes_video_frame() {
        let root =
            std::env::temp_dir().join(format!("lahza-video-thumbnail-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=320x180:d=0.2",
                "-c:v",
                "ffv1",
            ])
            .arg(root.join("screen.mkv"))
            .status()
            .unwrap();
        assert!(status.success());
        let preview = load_thumbnail(&root, true).unwrap();
        assert_eq!(preview.dimensions(), (192, 108));
        assert!(preview.get_pixel(0, 0)[0] > 200);
        assert!(!root.join("poster.jpg").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
