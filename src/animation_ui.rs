//! Project animation controls and hover demonstrations driven by real timelines.
use crate::recording::{
    animation::AnimationSettings,
    clips::RecordingClipTimeline,
    model::{NormalizedPoint, PointerCaptureFile, PointerTravelKind, PointerTravelSample},
    pointer_timeline::{PointerMotion, PointerTimeline, PointerTimelineOptions},
    viewport::{CameraMotion, ViewportTimeline, ZoomCue},
};
use crate::scene_ui::SceneSlider;
use crate::{blue, ink, muted, Studio, VideoEditSnapshot};
use gpui::{div, prelude::*, px, rgb, svg, AnyElement, Context, FontWeight, Timer};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AnimationDemoKind {
    Screen(CameraMotion),
    Cursor(PointerMotion),
}

pub(crate) struct AnimationHover {
    kind: AnimationDemoKind,
    started: Instant,
    samples: Vec<f32>,
}

/// Small synthetic trajectories use exactly the selected production spring.
/// No recording, media decode, or physical input is involved.
fn demo_samples(kind: AnimationDemoKind) -> Vec<f32> {
    match kind {
        AnimationDemoKind::Screen(motion) => {
            let mut cue = ZoomCue::pinned(0.2, 0.95, 2.0, NormalizedPoint { x: 0.75, y: 0.5 });
            cue.camera_motion = motion;
            let timeline = ViewportTimeline::build_static(&[cue], 1.8);
            (0..=216)
                .map(|i| (timeline.frame_at(i as f64 / 120.0).magnification - 1.0) as f32)
                .collect()
        }
        AnimationDemoKind::Cursor(motion) => {
            let capture = PointerCaptureFile {
                travel: [
                    (0.0, 0.15),
                    (0.25, 0.85),
                    (0.7, 0.85),
                    (0.95, 0.15),
                    (1.4, 0.15),
                ]
                .into_iter()
                .map(|(time, x)| PointerTravelSample {
                    time,
                    x,
                    y: 0.5,
                    kind: PointerTravelKind::Move,
                    artwork_id: None,
                })
                .collect(),
                ..Default::default()
            };
            let timeline = PointerTimeline::build(
                capture,
                1.8,
                1000.0,
                1000.0,
                PointerTimelineOptions {
                    motion,
                    hide_after_inactivity: None,
                    hide_when_typing: false,
                    ..Default::default()
                },
            );
            (0..=216)
                .map(|i| {
                    timeline
                        .frame_at(i as f64 / 120.0)
                        .map_or(0.15, |frame| frame.location.x as f32)
                })
                .collect()
        }
    }
}

fn screen_label(motion: CameraMotion) -> &'static str {
    match motion {
        CameraMotion::Focused => "Focused",
        CameraMotion::Smooth => "Smooth",
        CameraMotion::Standard => "Standard",
    }
}

/// Resolve an inactive image scene at export time from its authored data. Its
/// cached timelines may predate project preferences changed in another scene.
pub(crate) fn image_scene_timelines(
    cues: &[ZoomCue],
    capture: &PointerCaptureFile,
    duration: f64,
    dimensions: (u32, u32),
    settings: AnimationSettings,
    pointer_options: PointerTimelineOptions,
) -> (ViewportTimeline, Option<PointerTimeline>) {
    let cues = settings.cues(cues);
    if capture.presses.is_empty() {
        return (ViewportTimeline::build_static(&cues, duration), None);
    }
    let clips = RecordingClipTimeline::full(duration);
    let pointer = PointerTimeline::build_with_clip_timeline(
        capture.clone(),
        duration,
        f64::from(dimensions.0),
        f64::from(dimensions.1),
        pointer_options,
        Some(&clips),
    );
    let viewport = ViewportTimeline::build(&cues, &pointer, &clips, capture);
    (viewport, Some(pointer))
}

impl Studio {
    pub(crate) fn animation_edit_snapshot(&self) -> VideoEditSnapshot {
        VideoEditSnapshot::AnimationPreferences {
            settings: self.animation_settings,
            cursor_motion: self.pointer_style.motion,
        }
    }

    pub(crate) fn record_animation_edit(&mut self) {
        self.video_undo_stack.push(self.animation_edit_snapshot());
        self.video_redo_stack.clear();
    }

    pub(crate) fn restore_animation_preferences(
        &mut self,
        settings: AnimationSettings,
        cursor_motion: PointerMotion,
    ) {
        self.animation_settings = settings;
        self.pointer_style.motion = cursor_motion;
        self.rebuild_video_motion_timelines();
        self.autosave_scene_style();
    }

    pub(crate) fn set_screen_animation(&mut self, motion: Option<CameraMotion>) {
        if self.video_edit_busy || self.animation_settings.screen_motion == motion {
            return;
        }
        self.record_animation_edit();
        self.animation_settings.screen_motion = motion;
        self.rebuild_video_motion_timelines();
        self.autosave_scene_style();
    }

    pub(crate) fn set_cursor_animation(&mut self, motion: PointerMotion) {
        if self.video_edit_busy || self.pointer_style.motion == motion {
            return;
        }
        self.record_animation_edit();
        self.pointer_style.motion = motion;
        self.rebuild_video_motion_timelines();
        self.autosave_scene_style();
    }

    pub(crate) fn project_early_enabled(&self) -> bool {
        self.animation_settings.start_early.unwrap_or_else(|| {
            !self.video_zoom_cues.is_empty()
                && self
                    .video_zoom_cues
                    .iter()
                    .all(|cue| cue.start_animation_early)
        })
    }

    pub(crate) fn set_project_early(&mut self, early: Option<bool>) {
        if self.video_edit_busy || self.animation_settings.start_early == early {
            return;
        }
        self.record_animation_edit();
        self.animation_settings.start_early = early;
        self.rebuild_video_motion_timelines();
        self.autosave_scene_style();
    }

    fn hover_animation_preset(
        &mut self,
        kind: AnimationDemoKind,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if !hovered {
            if self
                .animation_hover
                .as_ref()
                .is_some_and(|hover| hover.kind == kind)
            {
                self.animation_hover = None;
                cx.notify();
            }
            return;
        }
        let started = Instant::now();
        self.animation_hover = Some(AnimationHover {
            kind,
            started,
            samples: demo_samples(kind),
        });
        cx.notify();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(16)).await;
            let active = weak
                .update(cx, |this, cx| {
                    let active = this.animation_hover.as_ref().is_some_and(|hover| {
                        hover.started == started && hover.started.elapsed() < Duration::from_secs(6)
                    });
                    if !active
                        && this
                            .animation_hover
                            .as_ref()
                            .is_some_and(|hover| hover.started == started)
                    {
                        this.animation_hover = None;
                        cx.notify();
                    }
                    if active {
                        cx.notify();
                    }
                    active
                })
                .unwrap_or(false);
            if !active {
                break;
            }
        })
        .detach();
    }

    fn animation_demonstration(&self, kind: AnimationDemoKind) -> AnyElement {
        let value = self
            .animation_hover
            .as_ref()
            .filter(|hover| hover.kind == kind)
            .map_or(0.0, |hover| {
                let position = (hover.started.elapsed().as_secs_f64() % 1.8) * 120.0;
                let index = position as usize;
                let a = hover.samples[index.min(hover.samples.len() - 1)];
                let b = hover.samples[(index + 1).min(hover.samples.len() - 1)];
                a + (b - a) * position.fract() as f32
            });
        let background = div()
            .relative()
            .w_full()
            .h(px(31.0))
            .overflow_hidden()
            .rounded_sm()
            .bg(rgb(0xeaf0f8));
        match kind {
            AnimationDemoKind::Screen(_) => background
                .child(
                    div()
                        .absolute()
                        .left(gpui::relative(0.22 - value.clamp(0.0, 1.2) * 0.11))
                        .top(px(8.0 - value * 3.0))
                        .w(gpui::relative(0.56 + value.clamp(0.0, 1.2) * 0.3))
                        .h(px(17.0 + value * 9.0))
                        .rounded_sm()
                        .bg(rgb(0xffffff))
                        .border_1()
                        .border_color(rgb(0x8facd6))
                        .child(div().m_1().h(px(3.0)).w(px(18.0)).bg(rgb(0xb6c9e4))),
                )
                .into_any_element(),
            AnimationDemoKind::Cursor(_) => background
                .child(
                    svg()
                        .path("icons/mouse-pointer.svg")
                        .absolute()
                        .left(gpui::relative(0.05 + value.clamp(0.0, 1.0) * 0.7))
                        .top(px(8.0))
                        .size(px(15.0))
                        .text_color(ink()),
                )
                .into_any_element(),
        }
    }

    pub(crate) fn screen_animation_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.animation_settings.screen_motion;
        let legacy = self.video_zoom_cues.first().map(|cue| cue.camera_motion);
        let mixed = legacy.is_some_and(|first| {
            self.video_zoom_cues
                .iter()
                .any(|cue| cue.camera_motion != first)
        });
        let status = match selected {
            Some(motion) => format!("{} · every zoom", screen_label(motion)),
            None if mixed => "Per-region · mixed responses".into(),
            None => format!(
                "Per-region · {}",
                screen_label(legacy.unwrap_or(CameraMotion::Standard))
            ),
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Screen animation"),
                    )
                    .child(self.small_button(
                        "animation-screen-reset",
                        "Use region settings",
                        selected.is_some(),
                        cx,
                        |this, cx| {
                            this.set_screen_animation(None);
                            cx.notify();
                        },
                    )),
            )
            .child(
                div().flex().gap_1().children(
                    [
                        CameraMotion::Focused,
                        CameraMotion::Smooth,
                        CameraMotion::Standard,
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, motion)| {
                        let kind = AnimationDemoKind::Screen(motion);
                        div()
                            .id(("animation-screen-preset", index))
                            .flex_1()
                            .min_w_0()
                            .p_1()
                            .rounded_md()
                            .border_2()
                            .border_color(if selected == Some(motion) {
                                blue()
                            } else {
                                rgb(0xe0e4eb).into()
                            })
                            .flex()
                            .flex_col()
                            .gap_1()
                            .cursor_pointer()
                            .child(self.animation_demonstration(kind))
                            .child(div().text_xs().text_center().child(screen_label(motion)))
                            .on_hover(cx.listener(move |this, hover: &bool, _, cx| {
                                this.hover_animation_preset(kind, *hover, cx)
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_screen_animation(Some(motion));
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(div().text_xs().text_color(muted()).child(status))
            .into_any_element()
    }

    pub(crate) fn cursor_animation_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.pointer_style.motion;
        let description = self
            .animation_hover
            .as_ref()
            .and_then(|hover| {
                if let AnimationDemoKind::Cursor(motion) = hover.kind {
                    Some(motion)
                } else {
                    None
                }
            })
            .unwrap_or(selected);
        let text = match description {
            PointerMotion::None => "Recorded movement without smoothing.",
            PointerMotion::Smooth => "Soft, relaxed cursor movement.",
            PointerMotion::Medium => "Balanced smoothing and response.",
            PointerMotion::Rapid => "Fast response with a light spring.",
            _ => "Saved legacy response. Choose a preset to update it.",
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Cursor animation"),
                    )
                    .child(self.small_button(
                        "animation-cursor-reset",
                        "Reset",
                        selected != PointerMotion::Smooth,
                        cx,
                        |this, cx| {
                            this.set_cursor_animation(PointerMotion::Smooth);
                            cx.notify();
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .children(
                        PointerMotion::ALL
                            .into_iter()
                            .enumerate()
                            .map(|(index, motion)| {
                                let kind = AnimationDemoKind::Cursor(motion);
                                div()
                                    .id(("pointer-motion", index))
                                    .flex_1()
                                    .min_w_0()
                                    .p_1()
                                    .rounded_md()
                                    .border_2()
                                    .border_color(if selected == motion {
                                        blue()
                                    } else {
                                        rgb(0xe0e4eb).into()
                                    })
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .cursor_pointer()
                                    .child(self.animation_demonstration(kind))
                                    .child(div().text_xs().text_center().child(motion.label()))
                                    .on_hover(cx.listener(move |this, hover: &bool, _, cx| {
                                        this.hover_animation_preset(kind, *hover, cx)
                                    }))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_cursor_animation(motion);
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .child(div().text_xs().text_color(muted()).child(text))
            .into_any_element()
    }

    pub(crate) fn project_early_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.scene_toggle_row(
                "motion-early",
                "Start 350 ms early",
                self.project_early_enabled(),
                cx,
                |this| this.set_project_early(Some(!this.project_early_enabled())),
            ))
            .child(div().text_xs().text_color(muted()).child(
                if self.animation_settings.start_early.is_some() {
                    "Applies to every zoom in this project."
                } else {
                    "Per-region; changing applies to every zoom."
                },
            ))
            .when(self.animation_settings.start_early.is_some(), |panel| {
                panel.child(self.small_button(
                    "animation-early-reset",
                    "Use region settings",
                    true,
                    cx,
                    |this, cx| {
                        this.set_project_early(None);
                        cx.notify();
                    },
                ))
            })
            .into_any_element()
    }

    pub(crate) fn project_animation_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.screen_animation_controls(cx))
            .child(self.cursor_animation_controls(cx))
            .child(self.scene_slider_row(SceneSlider::MotionBlurAmount, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(muted())
                    .child("Motion blur applies in preview and export. Set 0% for crisp motion."),
            )
            .child(self.project_early_controls(cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_demonstrations_use_distinct_production_timelines() {
        let none = demo_samples(AnimationDemoKind::Cursor(PointerMotion::None));
        let smooth = demo_samples(AnimationDemoKind::Cursor(PointerMotion::Smooth));
        let rapid = demo_samples(AnimationDemoKind::Cursor(PointerMotion::Rapid));
        assert!(none[31] > 0.8 && smooth[31] < 0.4);
        assert!(rapid[40] > smooth[40]);
        let focused = demo_samples(AnimationDemoKind::Screen(CameraMotion::Focused));
        let gentle = demo_samples(AnimationDemoKind::Screen(CameraMotion::Smooth));
        assert!(focused[45] > gentle[45]);
        assert!(smooth.iter().chain(&focused).all(|value| value.is_finite()));
    }

    #[test]
    fn inactive_image_scenes_resolve_current_project_preferences_and_walkthrough() {
        let cue = ZoomCue::pinned(2.0, 3.0, 2.0, NormalizedPoint { x: 0.7, y: 0.5 });
        let authored = vec![cue];
        let settings = AnimationSettings {
            screen_motion: Some(CameraMotion::Focused),
            start_early: Some(true),
            motion_blur: 65,
        };
        let (legacy, _) = image_scene_timelines(
            &authored,
            &PointerCaptureFile::default(),
            4.0,
            (640, 360),
            AnimationSettings::default(),
            PointerTimelineOptions::default(),
        );
        let (updated, pointer) = image_scene_timelines(
            &authored,
            &PointerCaptureFile::default(),
            4.0,
            (640, 360),
            settings,
            PointerTimelineOptions::default(),
        );
        assert!(pointer.is_none());
        assert_eq!(legacy.frame_at(1.8).magnification, 1.0);
        assert!(updated.frame_at(1.8).magnification > 1.0);
        assert!(
            !authored[0].start_animation_early,
            "export must not overwrite authored legacy data"
        );

        let capture = crate::recording::viewport::walkthrough_capture(
            &[
                NormalizedPoint { x: 0.2, y: 0.5 },
                NormalizedPoint { x: 0.8, y: 0.5 },
            ],
            4.0,
        );
        let (_, direct) = image_scene_timelines(
            &authored,
            &capture,
            4.0,
            (640, 360),
            settings,
            PointerTimelineOptions {
                motion: PointerMotion::None,
                ..Default::default()
            },
        );
        let (view, smoothed) = image_scene_timelines(
            &authored,
            &capture,
            4.0,
            (640, 360),
            settings,
            PointerTimelineOptions {
                motion: PointerMotion::Smooth,
                ..Default::default()
            },
        );
        assert!(view.frame_at(1.8).magnification > 1.0);
        let direct = direct.unwrap();
        let smoothed = smoothed.unwrap();
        assert!(
            (1..240).any(|frame| {
                let time = frame as f64 / 60.0;
                (direct.frame_at(time).unwrap().location.x
                    - smoothed.frame_at(time).unwrap().location.x)
                    .abs()
                    > 0.01
            }),
            "the inactive walkthrough must use the new cursor preset"
        );
    }
}
