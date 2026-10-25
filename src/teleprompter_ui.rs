//! The teleprompter: launcher settings, and the floating prompter window
//! shown while recording. The prompter scrolls with the speaker's voice
//! when whisper.cpp is available, and by hand (mouse wheel, clicking a
//! word) always.
use crate::recording::teleprompter::{
    ScriptLayout, SpeechFollower, SpeechStatus, TeleprompterSettings,
};
use crate::recording::capture_exclusion::CaptureExclusion;
use crate::recording_overlay_drag::OverlayDrag;
use crate::{blue, line, muted, Studio};
use gpui::{
    div, point, prelude::*, px, rgb, size, AnyElement, Bounds, Context, FontWeight,
    MouseButton, PathPromptOptions, ScrollWheelEvent, SharedString, Timer, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowKind,
    WindowOptions,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROMPTER_WIDTH: f32 = 760.0;
const HEADER_HEIGHT: f32 = 28.0;
const PADDING: f32 = 14.0;

fn voice_status_text(status: SpeechStatus) -> SharedString {
    match status {
        SpeechStatus::Starting => "Starting voice follow…".into(),
        SpeechStatus::Listening(device) => format!("Following your voice · {device}").into(),
        SpeechStatus::Paused(device) => format!("Voice follow paused · {device}").into(),
        SpeechStatus::Failed(reason) => format!("Voice follow stopped: {reason} · scroll to advance").into(),
    }
}

/// Recognition publishes a count, so the active word is the last one
/// matched. Before speech starts, cue the first word of a nonempty script.
fn highlighted_word(layout: &ScriptLayout, spoken: usize) -> Option<usize> {
    let last = layout.word_count().checked_sub(1)?;
    Some(spoken.saturating_sub(1).min(last))
}

fn highlighted_line(layout: &ScriptLayout, spoken: usize) -> usize {
    highlighted_word(layout, spoken).map_or(0, |word| layout.line_of(word))
}

/// Speech updates arrive outside GPUI. Invalidate the view for each new
/// word, including words that do not move the current line.
fn update_reading_position(
    layout: &ScriptLayout,
    position: usize,
    previous_position: &mut usize,
    shown_line: &mut f32,
) -> bool {
    let mut changed = position != *previous_position;
    *previous_position = position;
    let target = highlighted_line(layout, position) as f32;
    let delta = target - *shown_line;
    if delta.abs() > 0.002 {
        // Ease toward the reading line rather than jumping.
        *shown_line += delta * 0.22;
        if (target - *shown_line).abs() < 0.002 {
            *shown_line = target;
        }
        changed = true;
    }
    changed
}

/// The prompter window's view.
pub(crate) struct Prompter {
    exclusion: CaptureExclusion,
    drag: OverlayDrag,
    layout: ScriptLayout,
    font_size: f32,
    /// Script words read so far; written by the follower and by hand.
    position: Arc<AtomicUsize>,
    /// Smoothly animated top line.
    shown_line: f32,
    follower: Option<SpeechFollower>,
    status: SharedString,
    _ticker: gpui::Task<()>,
}

impl Prompter {
    fn line_height(&self) -> f32 {
        (self.font_size * 1.45).round()
    }

    fn new(
        settings: &TeleprompterSettings,
        microphone: bool,
        microphone_device: Option<&str>,
        exclusion: CaptureExclusion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let font_size = settings.font_size.clamp(16.0, 48.0);
        let text_width = PROMPTER_WIDTH - PADDING * 2.0;
        let font = gpui::font("Inter");
        let layout = ScriptLayout::new(&settings.script, text_width, |word| {
            let run = gpui::TextRun {
                len: word.len(),
                font: gpui::Font { weight: FontWeight::SEMIBOLD, ..font.clone() },
                color: gpui::white(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            f32::from(window.text_system().shape_line(word.to_string().into(), px(font_size), &[run], None).width)
        });
        let position = Arc::new(AtomicUsize::new(0));
        let (follower, status) = if !settings.follow_voice {
            (None, "Scroll to advance".into())
        } else if !microphone {
            (None, "Microphone is off · scroll to advance".into())
        } else {
            match SpeechFollower::start(
                &settings.script, position.clone(), microphone_device, &settings.voice_language,
            ) {
                Ok(follower) => (Some(follower), voice_status_text(SpeechStatus::Starting)),
                Err(reason) => (None, format!("Voice follow unavailable: {reason} · scroll to advance").into()),
            }
        };
        let window_handle = window.window_handle();
        let mut checked_protection = Instant::now();
        let mut previous_position = 0;
        let ticker = cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(33)).await;
            let keep_open = weak
                .update(cx, |this, cx| {
                    if checked_protection.elapsed() >= Duration::from_millis(250) {
                        checked_protection = Instant::now();
                        let protected = this.exclusion.is_protected();
                        if this.exclusion.has_been_protected() && !protected {
                            // The helper keeps the exclusion effect attached
                            // until this window is destroyed, preventing any
                            // exposed frames while we notice its disappearance.
                            return false;
                        }
                        if let Some(follower) = this.follower.as_mut() {
                            let status = voice_status_text(follower.status());
                            if status != this.status {
                                this.status = status;
                                cx.notify();
                            }
                        }
                    }
                    if update_reading_position(
                        &this.layout,
                        this.position.load(Ordering::Relaxed),
                        &mut previous_position,
                        &mut this.shown_line,
                    ) {
                        cx.notify();
                    }
                    true
                })
                .unwrap_or(false);
            if !keep_open {
                let _ = window_handle.update(cx, |_, window, _| window.remove_window());
                break;
            }
        });
        Self {
            exclusion,
            drag: OverlayDrag::default(),
            layout,
            font_size,
            position,
            shown_line: 0.0,
            follower,
            status,
            _ticker: ticker,
        }
    }

    fn window_height(lines: u8, font_size: f32) -> f32 {
        HEADER_HEIGHT + PADDING * 2.0 + lines.clamp(2, 6) as f32 * (font_size.clamp(16.0, 48.0) * 1.45).round()
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        if let Some(follower) = self.follower.as_ref() {
            follower.set_paused(paused);
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let lines = match event.delta {
            gpui::ScrollDelta::Lines(delta) => -delta.y,
            gpui::ScrollDelta::Pixels(delta) => -f32::from(delta.y) / self.line_height(),
        };
        if lines.abs() < 0.3 || self.layout.is_empty() {
            return;
        }
        let current = highlighted_line(&self.layout, self.position.load(Ordering::Relaxed));
        let target = if lines > 0.0 {
            (current + 1).min(self.layout.lines.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        // Store a spoken-word count so the target line's first word is active.
        let first_word = self.layout.lines[target].first().map_or(0, |word| word.index);
        self.position.store(first_word + 1, Ordering::Relaxed);
        cx.notify();
    }
}

impl Render for Prompter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line_height = self.line_height();
        let position = self.position.load(Ordering::Relaxed);
        let active_word = highlighted_word(&self.layout, position);
        let offset = self.shown_line * line_height;
        let accent = rgb(0xffd60a);
        let mut column = div().absolute().left_0().right_0().top(px(-offset)).flex().flex_col();
        for line in &self.layout.lines {
            let mut row = div().h(px(line_height)).flex().items_center().gap(px(self.font_size * 0.28));
            for word in line {
                let index = word.index;
                row = row.child(
                    div()
                        .id(("prompter-word", index))
                        .text_size(px(self.font_size))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if Some(index) == active_word {
                            accent.into()
                        } else if index < position {
                            gpui::hsla(0.0, 0.0, 1.0, 0.38)
                        } else {
                            gpui::white()
                        })
                        .cursor_pointer()
                        .child(word.text.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.position.store(index + 1, Ordering::Relaxed);
                            cx.notify();
                        })),
                );
            }
            column = column.child(row);
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family("Inter")
            .bg(rgb(0x111214))
            .text_color(gpui::white())
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag.end()))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.drag.end()))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.scroll(event, cx)))
            .child(
                div()
                    .id("prompter-header")
                    .h(px(HEADER_HEIGHT))
                    .flex_none()
                    .px(px(PADDING))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(gpui::hsla(0.0, 0.0, 1.0, 0.55))
                    .cursor(gpui::CursorStyle::OpenHand)
                    .tooltip(crate::timeline_ui::tooltip("Drag to move teleprompter"))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, event, window, cx| {
                        this.drag.begin(this.exclusion.title(), event, window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        div().flex().items_center().gap_2()
                            .child(div().text_size(px(16.0)).child("⠿"))
                            .child("Teleprompter"),
                    )
                    .child(div().id("teleprompter-runtime-status").min_w_0().ml_3().truncate().child(self.status.clone())
                        .tooltip(crate::timeline_ui::tooltip(self.status.clone()))),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .mx(px(PADDING))
                    .mb(px(PADDING))
                    .overflow_hidden()
                    .child(column),
            )
    }
}

impl Studio {
    /// Open before capture starts so the compositor can protect the exact
    /// window before the first encoded frame. Window capture needs no helper.
    pub(crate) fn open_prompter(&mut self, cx: &mut Context<Self>) -> Result<Option<CaptureExclusion>, String> {
        self.close_prompter(cx);
        let settings = self.teleprompter.clone();
        if !settings.is_active() {
            return Ok(None);
        }
        let exclusion = CaptureExclusion::new()?;
        let window_exclusion = exclusion.clone();
        let microphone = self.record_microphone;
        let microphone_device = self.microphone_device.clone();
        let width = px(PROMPTER_WIDTH);
        let height = px(Prompter::window_height(settings.lines, settings.font_size));
        // Top center of the main screen, close to where a webcam sits.
        let display = cx.primary_display();
        let origin = display
            .as_ref()
            .map(|display| {
                let bounds = display.bounds();
                point(bounds.origin.x + (bounds.size.width - width) / 2.0, bounds.origin.y + px(36.0))
            })
            .unwrap_or_else(|| point(px(200.0), px(36.0)));
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(width, height) })),
                titlebar: None,
                focus: false,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: true,
                is_resizable: false,
                is_minimizable: false,
                display_id: display.as_ref().map(|display| display.id()),
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some("com.lahza.Lahza".into()),
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            move |window, cx| {
                window.set_window_title(window_exclusion.title());
                cx.new(|cx| Prompter::new(
                    &settings, microphone, microphone_device.as_deref(), window_exclusion, window, cx,
                ))
            },
        );
        match result {
            Ok(handle) => {
                self.prompter_window = Some(handle);
                self.pause_prompter(true, cx);
                Ok(Some(exclusion))
            }
            Err(error) => Err(format!("Could not open the teleprompter: {error}")),
        }
    }

    /// Reset the take without replacing the protected native window.
    pub(crate) fn reset_prompter(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.prompter_window {
            let settings = self.teleprompter.clone();
            let microphone = self.record_microphone;
            let microphone_device = self.microphone_device.clone();
            let _ = handle.update(cx, |prompter, window, cx| {
                let exclusion = prompter.exclusion.clone();
                drop(prompter.follower.take());
                *prompter = Prompter::new(
                    &settings, microphone, microphone_device.as_deref(), exclusion, window, cx,
                );
            });
        }
    }

    pub(crate) fn close_prompter(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.prompter_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    pub(crate) fn pause_prompter(&mut self, paused: bool, cx: &mut Context<Self>) {
        if let Some(handle) = self.prompter_window {
            let _ = handle.update(cx, |prompter, _, _| prompter.set_paused(paused));
        }
    }

    fn save_teleprompter(&mut self) {
        if let Err(error) = self.teleprompter.save() {
            self.toast = Some(format!("Could not save the teleprompter script: {error}").into());
        }
    }

    fn set_teleprompter_script(&mut self, script: String) {
        // A file picker can finish after recording has started. Do not let
        // its result enable the prompter on an existing monitor capture.
        if self.recording_state != crate::RecordingState::Idle || self.recording_busy {
            self.toast = Some("Finish recording before changing the script".into());
            return;
        }
        let script = script.replace("\r\n", "\n");
        if script.trim().is_empty() {
            self.toast = Some("That script is empty".into());
            return;
        }
        self.teleprompter.script = script;
        self.teleprompter.enabled = true;
        self.save_teleprompter();
    }

    fn open_teleprompter_script(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open script".into()),
        });
        cx.spawn(async move |weak, cx| {
            let Some(path) = prompt.await.ok().and_then(Result::ok).flatten().and_then(|paths| paths.into_iter().next()) else {
                return;
            };
            let text = std::fs::read_to_string(&path);
            let _ = weak.update(cx, |this, cx| {
                match text {
                    Ok(text) => this.set_teleprompter_script(text),
                    Err(error) => this.toast = Some(format!("Could not read {}: {error}", path.display()).into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn paste_teleprompter_script(&mut self, cx: &mut Context<Self>) {
        match cx.read_from_clipboard().and_then(|item| item.text()) {
            Some(text) => self.set_teleprompter_script(text),
            None => self.toast = Some("The clipboard has no text".into()),
        }
    }

    fn launcher_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        enabled: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Studio, &mut Context<Studio>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .h(px(28.0))
            .px_3()
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(line())
            .bg(gpui::white())
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .opacity(if enabled { 1.0 } else { 0.45 })
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(rgb(0xf0f0f2)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_click(this, cx);
                        cx.notify();
                    }))
            })
            .child(label.into())
            .into_any_element()
    }

    /// Launcher row and settings for the teleprompter.
    pub(crate) fn launcher_teleprompter(&self, recording: bool, cx: &mut Context<Self>) -> AnyElement {
        let settings = &self.teleprompter;
        let words = settings.script.split_whitespace().count();
        let subtitle = if settings.has_script() {
            format!("{words} words · shows above your screen while recording")
        } else {
            "Add a script to read while recording".into()
        };
        let row = self.launcher_source_row(
            "launcher-teleprompter",
            "icons/text.svg",
            "Teleprompter",
            subtitle,
            settings.enabled,
            cx.listener(|this, _, _, cx| {
                // The capture source is fixed for this take. Enabling a
                // script after monitor capture starts would record it.
                if this.recording_state != crate::RecordingState::Idle || this.recording_busy {
                    this.toast = Some("Teleprompter settings can be changed after recording".into());
                    cx.notify();
                    return;
                }
                this.teleprompter.enabled = !this.teleprompter.enabled;
                this.save_teleprompter();
                cx.notify();
            }),
        );
        if !settings.enabled || recording {
            return row;
        }
        let voice = (settings.follow_voice && !self.record_microphone)
            .then_some("Microphone off");
        let preview: String = settings
            .script
            .split_whitespace()
            .take(24)
            .collect::<Vec<_>>()
            .join(" ");
        let lines_index = (settings.lines.clamp(2, 4) - 2) as usize;
        let size_index = match settings.font_size as u32 {
            0..=22 => 0,
            23..=30 => 1,
            _ => 2,
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(row)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(line())
                    .when(settings.has_script(), |this| {
                        this.child(div().text_xs().text_color(muted()).child(format!(
                            "{preview}{}",
                            if words > 24 { "…" } else { "" }
                        )))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(self.launcher_button("teleprompter-open", "Open script…", true, cx, |this, cx| {
                                this.open_teleprompter_script(cx)
                            }))
                            .child(self.launcher_button("teleprompter-paste", "Paste script", true, cx, |this, cx| {
                                this.paste_teleprompter_script(cx)
                            })),
                    )
                    .child(self.segmented(
                        "teleprompter-lines",
                        &["2 lines", "3 lines", "4 lines"],
                        lines_index,
                        |this, index| {
                            this.teleprompter.lines = index as u8 + 2;
                            this.save_teleprompter();
                        },
                        cx,
                    ))
                    .child(self.segmented(
                        "teleprompter-size",
                        &["Small", "Medium", "Large"],
                        size_index,
                        |this, index| {
                            this.teleprompter.font_size = [20.0, 26.0, 34.0][index];
                            this.save_teleprompter();
                        },
                        cx,
                    ))
                    .child(self.scene_toggle_row(
                        "teleprompter-voice",
                        "Scroll with my voice",
                        settings.follow_voice,
                        cx,
                        |this| {
                            this.teleprompter.follow_voice = !this.teleprompter.follow_voice;
                            this.save_teleprompter();
                        },
                    ))
                    .when_some(voice, |this, message| {
                        this.child(div().text_xs().text_color(muted()).child(message))
                    })
                    .when(settings.follow_voice, |this| {
                        this.child(
                            div()
                                .id("teleprompter-advanced")
                                .flex().items_center().gap_1().py_1()
                                .text_xs().text_color(muted()).cursor_pointer()
                                .hover(|style| style.text_color(blue()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.teleprompter_advanced_open = !this.teleprompter_advanced_open;
                                    cx.notify();
                                }))
                                .child(gpui::svg().path(if self.teleprompter_advanced_open {
                                    "icons/chevron-down.svg"
                                } else {
                                    "icons/chevron-right.svg"
                                }).size(px(12.0)).text_color(muted()))
                                .child("Advanced"),
                        )
                        .when(self.teleprompter_advanced_open, |this| {
                            this.child(div().text_xs().text_color(muted()).child("Language"))
                                .child(self.segmented(
                                    "teleprompter-language",
                                    &["Auto", "English"],
                                    if settings.voice_language == "en" { 1 } else { 0 },
                                    |this, index| {
                                        this.teleprompter.voice_language = if index == 1 { "en" } else { "auto" }.into();
                                        this.save_teleprompter();
                                    },
                                    cx,
                                ))
                                .child(self.speech_processing_controls("teleprompter-processing", cx))
                        })
                        .when(!self.teleprompter_advanced_open, |this| {
                            this.children(self.speech_setup_notice("teleprompter-setup-notice", cx))
                        })
                    }),
            )
            .into_any_element()
    }
}

/// The prompter window handle type, stored on the studio.
pub(crate) type PrompterHandle = WindowHandle<Prompter>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_stays_on_the_matched_word_and_its_line_at_boundaries() {
        let layout = ScriptLayout::new("one two three\nfour five six", 100.0, |word| word.len() as f32);
        for (spoken, word, line) in [
            (0, 0, 0), // First-word cue before recognition starts.
            (1, 0, 0),
            (2, 1, 0),
            (3, 2, 0), // Keep the last matched word visible at the line end.
            (4, 3, 1),
            (6, 5, 1), // The final spoken word stays highlighted.
            (usize::MAX, 5, 1),
        ] {
            assert_eq!(highlighted_word(&layout, spoken), Some(word));
            assert_eq!(highlighted_line(&layout, spoken), line);
        }
        assert_eq!(highlighted_word(&ScriptLayout::default(), 0), None);
        assert_eq!(highlighted_line(&ScriptLayout::default(), 0), 0);

        // A clicked word or a scroll target uses the same count convention.
        for selected in [0, 2, 3, 5] {
            let position = selected + 1;
            assert_eq!(highlighted_word(&layout, position), Some(selected));
            assert_eq!(highlighted_line(&layout, position), layout.line_of(selected));
        }
    }

    #[test]
    fn speech_redraws_each_word_before_scrolling_and_manual_rewind_still_works() {
        let layout = ScriptLayout::new("one two three\nfour five six", 100.0, |word| word.len() as f32);
        let mut previous_position = 0;
        let mut shown_line = 0.0;
        assert!(!update_reading_position(&layout, 0, &mut previous_position, &mut shown_line));

        // Speech advances the highlight even while all words share a line.
        for position in [1, 2, 3] {
            assert!(update_reading_position(&layout, position, &mut previous_position, &mut shown_line));
            assert_eq!(shown_line, 0.0);
            assert!(!update_reading_position(&layout, position, &mut previous_position, &mut shown_line));
        }

        // Speaking the next line's first word starts smooth scrolling, which
        // keeps repainting without requiring another recognized word.
        assert!(update_reading_position(&layout, 4, &mut previous_position, &mut shown_line));
        assert!(shown_line > 0.0 && shown_line < 1.0);
        assert!(update_reading_position(&layout, 4, &mut previous_position, &mut shown_line));
        for _ in 0..40 {
            update_reading_position(&layout, 4, &mut previous_position, &mut shown_line);
        }
        assert_eq!(shown_line, 1.0);
        assert!(!update_reading_position(&layout, 4, &mut previous_position, &mut shown_line));

        // Clicking or scrolling back uses the same invalidation path.
        assert!(update_reading_position(&layout, 1, &mut previous_position, &mut shown_line));
        assert!(shown_line < 1.0);
    }
}
