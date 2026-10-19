//! Import controls shared with the recording's existing clip and annotation editors.
use crate::{
    recording::{
        clips::RecordingClipSegment,
        media::{self, AudioClip},
    },
    AnnotationMark, NormPoint, Studio, Tool, VideoEditSnapshot,
};
use base64::Engine;
use gpui::{div, prelude::*, px, AnyElement, Context};

#[derive(Clone, Copy)]
pub(crate) enum ImportKind {
    Clip,
    Image,
    Audio,
}

enum Imported {
    Clip(std::path::PathBuf, f64, f64),
    Image(String, u32, u32),
    Audio(AudioClip),
}

impl Studio {
    pub(crate) fn import_timeline_media(&mut self, kind: ImportKind, cx: &mut Context<Self>) {
        if self.video_edit_busy {
            return;
        }
        let Some(session) = self.video_project.clone() else {
            return;
        };
        self.pause_video_playback();
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let token = uuid::Uuid::new_v4();
        cx.spawn(async move |weak, cx| {
            let Some(source) = (match prompt.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            }) else {
                return;
            };
            let start = weak
                .update(cx, |this, cx| {
                    if this.video_project.as_ref() != Some(&session) || this.video_edit_busy {
                        return None;
                    }
                    this.pause_video_playback();
                    this.video_edit_busy = true;
                    this.video_import_token = Some(token);
                    this.toast = Some("Importing media…".into());
                    cx.notify();
                    Some(this.video_position)
                })
                .ok()
                .flatten();
            let Some(start) = start else {
                return;
            };
            let project = session.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match kind {
                        ImportKind::Clip => media::append_media(&project, &source)
                            .map(|(file, start, end)| Imported::Clip(file, start, end))
                            .map_err(|e| e.to_string()),
                        ImportKind::Audio => media::import_audio(&project, &source, start)
                            .map(Imported::Audio)
                            .map_err(|e| e.to_string()),
                        ImportKind::Image => {
                            let image = image::open(&source).map_err(|e| e.to_string())?;
                            // Bound embedded overlay size while retaining transparent pixels.
                            let image = image.thumbnail(2560, 2560);
                            let mut bytes = std::io::Cursor::new(Vec::new());
                            image
                                .write_to(&mut bytes, image::ImageFormat::Png)
                                .map_err(|e| e.to_string())?;
                            Ok(Imported::Image(
                                base64::engine::general_purpose::STANDARD
                                    .encode(bytes.into_inner()),
                                image.width(),
                                image.height(),
                            ))
                        }
                    }
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.video_project.as_ref() != Some(&session)
                    || this.video_import_token != Some(token)
                {
                    return;
                }
                this.video_import_token = None;
                this.video_edit_busy = false;
                this.toast = None;
                match result {
                    Ok(Imported::Clip(file, start, end)) => {
                        let mut timeline = this.video_clip_timeline.clone();
                        let mut clip = RecordingClipSegment::new(start, end);
                        clip.source_bounds = Some([start, end]);
                        let id = clip.id;
                        timeline.segments.push(clip);
                        if let Err(error) = session.write_edit_field("mediaSource", &file) {
                            this.toast = Some(format!("Could not save import: {error}").into());
                        } else {
                            this.video_source_duration = end;
                            let position = this.video_duration;
                            this.apply_video_clip_timeline(timeline, Some(id), true, cx);
                            this.video_extras_pending = true;
                            this.seek_video(position, cx);
                            if this.video_noise_reduction {
                                this.set_video_noise_reduction(true, cx);
                            }
                        }
                    }
                    Ok(Imported::Audio(clip)) => {
                        this.video_undo_stack
                            .push(VideoEditSnapshot::Audio(this.video_audio_clips.clone()));
                        this.video_redo_stack.clear();
                        this.video_audio_clips.push(clip);
                        this.save_audio_clips(cx);
                        this.select_inspector_tab(crate::shell_ui::InspectorTab::Record);
                        this.open_sections.insert("audio");
                    }
                    Ok(Imported::Image(data, width, height)) => {
                        this.record_annotation_undo();
                        this.finish_annotation_interaction();
                        let (sw, sh) = this.video_source_size;
                        let w = 0.4_f32;
                        let h = (w * sw as f32 / sh.max(1) as f32 * height as f32
                            / width.max(1) as f32)
                            .min(0.8);
                        let w =
                            h * sh as f32 / sw.max(1) as f32 * width as f32 / height.max(1) as f32;
                        let start = start.min((this.video_duration - 0.12).max(0.0));
                        this.annotations.push(AnnotationMark {
                            tool: Tool::Image,
                            image_png: Some(data.into()),
                            start: NormPoint {
                                x: 0.5 - w / 2.0,
                                y: 0.5 - h / 2.0,
                            },
                            end: NormPoint {
                                x: 0.5 + w / 2.0,
                                y: 0.5 + h / 2.0,
                            },
                            timing: Some(crate::timed::AnnotationTiming {
                                start,
                                end: (start + 5.0).min(this.video_duration),
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                        this.selected_annotation = Some(this.annotations.len() - 1);
                        this.annotation_selection = vec![this.annotations.len() - 1];
                        this.tool = Tool::Select;
                        this.seek_video(start, cx);
                        this.select_annotation_for_timing(this.annotations.len() - 1);
                        this.autosave_scene_style();
                    }
                    Err(error) => {
                        this.toast = Some(format!("Could not import media: {error}").into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn save_audio_clips(&mut self, cx: &mut Context<Self>) {
        self.pause_video_playback();
        if let Some(session) = &self.video_project {
            if let Err(error) = session.write_edit_field("audioClips", &self.video_audio_clips) {
                self.toast = Some(format!("Could not save audio: {error}").into());
            }
        }
        cx.notify();
    }

    fn edit_audio(
        &mut self,
        id: uuid::Uuid,
        edit: impl FnOnce(&mut AudioClip),
        cx: &mut Context<Self>,
    ) {
        if self.video_edit_busy {
            return;
        }
        let Some(index) = self.video_audio_clips.iter().position(|clip| clip.id == id) else {
            return;
        };
        self.video_undo_stack
            .push(VideoEditSnapshot::Audio(self.video_audio_clips.clone()));
        self.video_redo_stack.clear();
        edit(&mut self.video_audio_clips[index]);
        self.save_audio_clips(cx);
    }

    pub(crate) fn imported_audio_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div().flex().flex_col().gap_2()
            .child(self.small_button("add-background-audio", "Add music / sound effect", !self.video_edit_busy, cx, |this, cx| this.import_timeline_media(ImportKind::Audio, cx)))
            .child(div().text_xs().text_color(crate::muted()).child("Audio starts at the playhead and mixes with the recording. Ends at the video’s end."));
        for clip in &self.video_audio_clips {
            let id = clip.id;
            let name = clip.name.clone();
            let button = |suffix: &str,
                          label: &str,
                          action: fn(&mut Self, uuid::Uuid, &mut Context<Self>),
                          cx: &mut Context<Self>| {
                div()
                    .id(gpui::SharedString::from(format!("audio-{id}-{suffix}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(gpui::rgb(0xf1f1f2))
                    .text_xs()
                    .cursor_pointer()
                    .child(label.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| action(this, id, cx)))
            };
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(crate::line())
                    .rounded_md()
                    .child(div().text_xs().child(name))
                    .child(div().text_xs().text_color(crate::muted()).child(format!(
                        "{:.1}s – {:.1}s · {}% · trim {:.1}s",
                        clip.start,
                        clip.start + clip.duration,
                        (clip.volume * 100.0).round(),
                        clip.source_start
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(button(
                                "place",
                                "At playhead",
                                |this, id, cx| {
                                    let start = this.video_position;
                                    this.edit_audio(id, |clip| clip.start = start, cx);
                                },
                                cx,
                            ))
                            .child(button(
                                "quieter",
                                "− volume",
                                |this, id, cx| {
                                    this.edit_audio(
                                        id,
                                        |clip| clip.volume = (clip.volume - 0.1).max(0.0),
                                        cx,
                                    )
                                },
                                cx,
                            ))
                            .child(button(
                                "louder",
                                "+ volume",
                                |this, id, cx| {
                                    this.edit_audio(
                                        id,
                                        |clip| clip.volume = (clip.volume + 0.1).min(2.0),
                                        cx,
                                    )
                                },
                                cx,
                            ))
                            .child(button(
                                "trim-start",
                                "Trim start here",
                                |this, id, cx| {
                                    let t = this.video_position;
                                    this.edit_audio(
                                        id,
                                        |clip| {
                                            let delta = (t - clip.start)
                                                .clamp(0.0, (clip.duration - 0.12).max(0.0));
                                            clip.source_start += delta;
                                            clip.duration -= delta;
                                            clip.start += delta;
                                        },
                                        cx,
                                    );
                                },
                                cx,
                            ))
                            .child(button(
                                "trim-end",
                                "End here",
                                |this, id, cx| {
                                    let t = this.video_position;
                                    this.edit_audio(
                                        id,
                                        |clip| {
                                            clip.duration = (t - clip.start)
                                                .clamp(0.12_f64.min(clip.duration), clip.duration)
                                        },
                                        cx,
                                    );
                                },
                                cx,
                            ))
                            .child(button(
                                "fade",
                                if clip.fade_in > 0.0 {
                                    "Fades: on"
                                } else {
                                    "Fades: off"
                                },
                                |this, id, cx| {
                                    this.edit_audio(
                                        id,
                                        |clip| {
                                            let fade = if clip.fade_in > 0.0 {
                                                0.0
                                            } else {
                                                1.0_f64.min(clip.duration / 2.0)
                                            };
                                            clip.fade_in = fade;
                                            clip.fade_out = fade;
                                        },
                                        cx,
                                    )
                                },
                                cx,
                            ))
                            .child(button(
                                "delete",
                                "Remove",
                                |this, id, cx| {
                                    if this.video_edit_busy {
                                        return;
                                    }
                                    this.video_undo_stack.push(VideoEditSnapshot::Audio(
                                        this.video_audio_clips.clone(),
                                    ));
                                    this.video_redo_stack.clear();
                                    this.video_audio_clips.retain(|clip| clip.id != id);
                                    this.save_audio_clips(cx);
                                },
                                cx,
                            )),
                    ),
            );
        }
        list.into_any_element()
    }

    pub(crate) fn imported_audio_lane(
        &self,
        width: f64,
        scroll: f64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut lane = div()
            .relative()
            .h(px(self.video_audio_clips.len() as f32 * 24.0))
            .overflow_hidden();
        for (index, clip) in self.video_audio_clips.iter().enumerate() {
            let end = (clip.start + clip.duration).min(self.video_duration);
            if end <= clip.start {
                continue;
            }
            lane = lane.child(
                div()
                    .absolute()
                    .top(px(index as f32 * 24.0))
                    .left(px(
                        (clip.start / self.video_duration * width - scroll) as f32
                    ))
                    .w(px(((end - clip.start) / self.video_duration * width) as f32))
                    .h(px(22.0))
                    .rounded_sm()
                    .bg(gpui::rgb(0xccebe5))
                    .px_2()
                    .text_xs()
                    .overflow_hidden()
                    .child(format!(
                        "{} · {}%",
                        clip.name,
                        (clip.volume * 100.0).round()
                    ))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, event, _, cx| {
                            this.timeline_seek_down(event);
                            this.select_inspector_tab(crate::shell_ui::InspectorTab::Record);
                            this.open_sections.insert("audio");
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            );
        }
        lane.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn imported_image_and_audio_render_with_undo_and_project_reopen() {
        use crate::*;
        use base64::Engine;
        let root = std::env::temp_dir().join(format!("lahza-import-ui-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let session = RecordingSession {
            directory: root.clone(),
        };
        assert!(std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=160x90:r=10:d=2",
                "-c:v",
                "ffv1",
                "-y"
            ])
            .arg(session.screen_path())
            .status()
            .unwrap()
            .success());
        session
            .write_manifest(&recording::model::CaptureManifest::default())
            .unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(40, 40, image::Rgba([20, 200, 80, 180]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let still = root.join("appended.png");
        std::fs::write(&still, png.get_ref()).unwrap();
        let (file, start, end) = recording::media::append_media(&session, &still).unwrap();
        session.write_edit_field("mediaSource", &file).unwrap();
        session.write_clip_timeline_draft(&recording::clips::RecordingClipTimeline::new(vec![
            recording::clips::RecordingClipSegment::new(0.0, 2.0),
            recording::clips::RecordingClipSegment::new(start, end),
        ])).unwrap();
        session
            .write_edit_field(
                "annotations",
                &vec![AnnotationMark {
                    tool: Tool::Image,
                    image_png: Some(
                        base64::engine::general_purpose::STANDARD
                            .encode(png.into_inner())
                            .into(),
                    ),
                    start: NormPoint { x: 0.2, y: 0.2 },
                    end: NormPoint { x: 0.6, y: 0.6 },
                    timing: Some(timed::AnnotationTiming {
                        start: 0.0,
                        end: 2.0,
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
            )
            .unwrap();
        let project = root.clone();
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
                        studio.open_video_project(project.clone()).unwrap();
                        assert!((studio.video_source_duration - 7.0).abs() < 0.01);
                        assert!((studio.video_duration - 7.0).abs() < 0.01);
                        assert_eq!(studio.video_clip_timeline.segments.len(), 2);
                        studio.launcher_active = false;
                        studio.video_position = 1.0;
                        studio.select_annotation_for_timing(0);
                        let clip = recording::media::AudioClip {
                            id: Uuid::new_v4(),
                            file: "media/music.wav".into(),
                            name: "Music".into(),
                            start: 0.2,
                            source_start: 0.0,
                            duration: 1.0,
                            volume: 0.5,
                            fade_in: 0.2,
                            fade_out: 0.2,
                        };
                        studio.video_audio_clips.push(clip.clone());
                        studio.edit_audio(clip.id, |clip| clip.volume = 0.2, cx);
                        assert_eq!(studio.video_audio_clips[0].volume, 0.2);
                        studio.undo_video_edit(cx);
                        assert_eq!(studio.video_audio_clips[0].volume, 0.5);
                        studio.redo_video_edit(cx);
                        assert_eq!(studio.video_audio_clips[0].volume, 0.2);
                        studio.autosave_scene_style();
                        studio.open_video_project(project).unwrap();
                        assert_eq!(studio.annotations[0].tool, Tool::Image);
                        assert_eq!(studio.video_audio_clips[0].volume, 0.2);
                        studio.video_playing = true;
                        for effect in [timed::EntranceEffect::Fade, timed::EntranceEffect::Pop, timed::EntranceEffect::SlideLeft, timed::EntranceEffect::SlideUp] {
                            studio.annotations[0].timing.as_mut().unwrap().entrance = effect;
                            let mut export = studio.annotation_overlay_source().unwrap();
                            for time in [0.1, 0.2, 0.3, 1.0, 1.8] {
                                studio.video_position = time;
                                studio.scene_preview_image(px(480.0), px(270.0)).unwrap();
                                let (signature, preview) = studio.preview_cache.overlay.as_ref().expect("flat image overlays must be composited, not painted as snapping sprites");
                                assert_ne!(*signature, 0);
                                assert_eq!(preview.as_ref(), export(time).unwrap().as_ref(), "{effect:?} at {time}: preview and export must agree");
                            }
                        }
                        studio.video_playing = false;
                        studio.video_position = 1.0;
                        studio.select_inspector_tab(shell_ui::InspectorTab::Record);
                        studio.open_sections.insert("audio");
                        cx.notify();
                    })
                    .unwrap();
                cx.spawn(async move |cx| {
                    Timer::after(Duration::from_millis(300)).await;
                    handle
                        .update(cx, |studio, _, cx| {
                            studio.select_annotation_for_timing(0);
                            studio.video_position = 0.1;
                            studio.annotation_edit_preview_pending = false;
                            cx.notify();
                        })
                        .unwrap();
                    Timer::after(Duration::from_millis(300)).await;
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
            });
        std::fs::remove_dir_all(root).unwrap();
    }
}
