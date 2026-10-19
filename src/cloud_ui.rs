//! Share to the cloud: server settings, and a job that renders the current
//! edit to MP4 and uploads it to the user's own upload worker, then copies
//! the share link.
use crate::recording::{
    cloud::{self, CloudSettings},
    export::{export_scene, ExportFormat, ExportProgress},
};
use crate::{blue, muted, Studio};
use gpui::{div, prelude::*, px, AnyElement, ClipboardItem, Context, FontWeight, SharedString, Timer};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

const STAGE_RENDER: u8 = 0;
const STAGE_UPLOAD: u8 = 1;

impl Studio {
    pub(crate) fn save_cloud_settings(&mut self) {
        self.cloud.worker_url = self.cloud.worker_url.trim().to_string();
        self.cloud.upload_token = self.cloud.upload_token.trim().to_string();
        if let Err(error) = self.cloud.save() {
            self.toast = Some(format!("Could not save sharing settings: {error}").into());
        }
    }

    pub(crate) fn share_recording(&mut self, cx: &mut Context<Self>) {
        if self.video_edit_busy || self.export_progress.is_some() {
            self.toast = Some("Wait for the current export to finish".into());
            cx.notify();
            return;
        }
        if !self.cloud.is_configured() {
            self.cloud_editing = true;
            cx.notify();
            return;
        }
        let Some(session) = self.video_project.clone() else {
            return;
        };
        if let Err(error) = session.commit_draft() {
            self.toast = Some(format!("Could not save recording edits: {error}").into());
            cx.notify();
            return;
        }
        let (source, mut request) = self.video_export_request(&session, ExportFormat::Mp4);
        let name = chrono::Local::now().format("Lahza-%Y-%m-%d_%H-%M-%S.mp4").to_string();
        let directory = std::env::temp_dir().join(format!("lahza-share-render-{}", uuid::Uuid::new_v4()));
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.toast = Some(format!("Could not prepare the upload: {error}").into());
            cx.notify();
            return;
        }
        request.destination = directory.join(&name);
        let settings = self.cloud.clone();
        let title = Some(self.share_title.trim().to_string()).filter(|title| !title.is_empty());
        let transcript = self
            .video_transcript
            .as_ref()
            .map(|transcript| transcript.in_editor_time(&self.video_clip_timeline));
        let progress = Arc::new(ExportProgress::default());
        let stage = Arc::new(AtomicU8::new(STAGE_RENDER));
        self.export_progress = Some(progress.clone());
        self.export_label = "Rendering for upload…".into();
        self.export_panel_collapsed = false;
        self.export_filename = name.clone().into();
        self.toast = None;
        let job = progress.clone();
        let job_stage = stage.clone();
        let task = cx.background_executor().spawn(async move {
            let render = export_scene(source, &mut request, &job);
            let result = render.and_then(|()| {
                job_stage.store(STAGE_UPLOAD, Ordering::Relaxed);
                job.begin_stage(1000);
                cloud::share_video(
                    &settings,
                    &request.destination,
                    title.as_deref(),
                    transcript.as_ref(),
                    || job.is_cancelled(),
                    |fraction| job.set_completed((fraction * 1000.0) as u64),
                )
            });
            let _ = std::fs::remove_dir_all(&directory);
            result.map_err(|error| error.to_string())
        });
        let ticker = progress.clone();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(120)).await;
            let active = weak
                .update(cx, |this, cx| {
                    if stage.load(Ordering::Relaxed) == STAGE_UPLOAD {
                        this.export_label = "Uploading…".into();
                    }
                    cx.notify();
                    this.export_progress.as_ref().is_some_and(|active| Arc::ptr_eq(active, &ticker))
                })
                .unwrap_or(false);
            if !active {
                break;
            }
        })
        .detach();
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.export_progress = None;
                this.toast = Some(match result {
                    Ok(upload) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(upload.url.clone()));
                        this.last_share_url = Some(upload.url.clone().into());
                        format!("Share link copied: {}", upload.url).into()
                    }
                    Err(error) if error == "export cancelled" => "Upload cancelled".into(),
                    Err(error) => format!("Share failed: {error}").into(),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Export → Share.
    pub(crate) fn share_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let configured = self.cloud.is_configured();
        let mut section = div().flex().flex_col().gap_2();
        if !configured || self.cloud_editing {
            return section
                .child(div().text_xs().text_color(muted()).child(
                    "Upload to your own Screendrop-compatible upload worker and get a share link. Settings stay on this computer.",
                ))
                .child(div().text_xs().text_color(muted()).child("Server"))
                .child(self.text_fields.cloud_url.clone())
                .child(div().text_xs().text_color(muted()).child("Upload token"))
                .child(self.text_fields.cloud_token.clone())
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(self.small_button("cloud-save", "Save", true, cx, |this, _| {
                            this.save_cloud_settings();
                            this.cloud_editing = !this.cloud.is_configured();
                        }))
                        .when(configured, |this| {
                            this.child(self.small_button("cloud-cancel", "Cancel", true, cx, |this, _| {
                                this.cloud = CloudSettings::load();
                                this.cloud_editing = false;
                            }))
                        }),
                )
                .into_any_element();
        }
        let host: SharedString = self
            .cloud
            .base_url()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
            .into();
        section = section
            .child(div().text_xs().text_color(muted()).child("Title"))
            .child(self.text_fields.share_title.clone())
            .child(self.scene_toggle_row(
                "cloud-social",
                "Allow comments & likes",
                self.cloud.social_enabled,
                cx,
                |this| {
                    this.cloud.social_enabled = !this.cloud.social_enabled;
                    this.save_cloud_settings();
                },
            ))
            .child(
                div()
                    .id("cloud-share")
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(blue())
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(gpui::white())
                    .opacity(if self.export_progress.is_none() { 1.0 } else { 0.45 })
                    .when(self.export_progress.is_none(), |this| {
                        this.cursor_pointer().on_click(cx.listener(|this, _, _, cx| this.share_recording(cx)))
                    })
                    .child("Upload & copy link"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(muted())
                    .child(format!("To {host}"))
                    .child(
                        div()
                            .id("cloud-change")
                            .cursor_pointer()
                            .text_color(blue())
                            .child("Change server")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cloud_editing = true;
                                cx.notify();
                            })),
                    ),
            );
        if let Some(url) = self.last_share_url.clone() {
            section = section.child(
                div()
                    .id("cloud-last-link")
                    .text_xs()
                    .text_color(blue())
                    .cursor_pointer()
                    .child(format!("Last link: {url} (click to copy)"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(url.to_string()));
                        this.toast = Some("Link copied".into());
                        cx.notify();
                    })),
            );
        }
        section.into_any_element()
    }
}
