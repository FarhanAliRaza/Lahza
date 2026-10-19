//! The teleprompter: launcher settings, and the floating prompter window
//! shown while recording. The prompter scrolls with the speaker's voice
//! when whisper.cpp is available, and by hand (mouse wheel, clicking a
//! word) always.
use crate::recording::teleprompter::{ScriptLayout, SpeechFollower, TeleprompterSettings};
use crate::recording::transcription::{self, JobProgress, SpeechModel};
use crate::{blue, line, muted, Studio};
use gpui::{
    div, point, prelude::*, px, rgb, size, AnyElement, Bounds, Context, FontWeight,
    MouseButton, PathPromptOptions, ScrollWheelEvent, SharedString, Timer, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowKind,
    WindowOptions,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const PROMPTER_WIDTH: f32 = 760.0;
const HEADER_HEIGHT: f32 = 28.0;
const PADDING: f32 = 14.0;

/// The prompter window's view.
pub(crate) struct Prompter {
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

    fn new(settings: &TeleprompterSettings, microphone: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            match SpeechFollower::start(&settings.script, position.clone()) {
                Ok(follower) => (Some(follower), "Following your voice".into()),
                Err(reason) => (None, format!("Voice follow unavailable: {reason} · scroll to advance").into()),
            }
        };
        let ticker = cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(33)).await;
            let alive = weak
                .update(cx, |this, cx| {
                    let target = this.layout.reading_line(this.position.load(Ordering::Relaxed)) as f32;
                    let delta = target - this.shown_line;
                    if delta.abs() > 0.002 {
                        // Ease toward the reading line rather than jumping.
                        this.shown_line += delta * 0.22;
                        if (target - this.shown_line).abs() < 0.002 {
                            this.shown_line = target;
                        }
                        cx.notify();
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
        });
        Self {
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
        let current = self.layout.reading_line(self.position.load(Ordering::Relaxed));
        let target = if lines > 0.0 {
            (current + 1).min(self.layout.lines.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        // Position at the start of the target line.
        let first_word = self.layout.lines[target].first().map_or(0, |word| word.index);
        self.position.store(first_word, Ordering::Relaxed);
        cx.notify();
    }
}

impl Render for Prompter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line_height = self.line_height();
        let position = self.position.load(Ordering::Relaxed);
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
                        .text_color(if index < position {
                            gpui::hsla(0.0, 0.0, 1.0, 0.38)
                        } else if index == position {
                            accent.into()
                        } else {
                            gpui::white()
                        })
                        .cursor_pointer()
                        .child(word.text.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.position.store(index, Ordering::Relaxed);
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
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                    .child("Teleprompter")
                    .child(self.status.clone()),
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
    /// Show the prompter for a recording that just started.
    pub(crate) fn open_prompter(&mut self, cx: &mut Context<Self>) {
        self.close_prompter(cx);
        let settings = self.teleprompter.clone();
        if !settings.enabled || !settings.has_script() {
            return;
        }
        let microphone = self.record_microphone;
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
            move |window, cx| cx.new(|cx| Prompter::new(&settings, microphone, window, cx)),
        );
        match result {
            Ok(handle) => self.prompter_window = Some(handle),
            Err(error) => self.toast = Some(format!("Could not open the teleprompter: {error}").into()),
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

    fn download_voice_model(&mut self, cx: &mut Context<Self>) {
        if self.voice_model_download.is_some() {
            return;
        }
        let progress = JobProgress::default();
        self.voice_model_download = Some(progress.clone());
        let job = progress.clone();
        let task = cx
            .background_executor()
            .spawn(async move { transcription::download_model(SpeechModel::Base, &job).map(|_| ()) });
        let ticker = progress.clone();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(200)).await;
            let active = weak
                .update(cx, |this, cx| {
                    cx.notify();
                    this.voice_model_download
                        .as_ref()
                        .is_some_and(|active| Arc::ptr_eq(&active.fraction, &ticker.fraction))
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
                this.voice_model_download = None;
                this.toast = Some(match result {
                    Ok(()) => "Voice model ready".into(),
                    Err(error) => format!("Could not download the voice model: {error}").into(),
                });
                cx.notify();
            });
        })
        .detach();
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
                this.teleprompter.enabled = !this.teleprompter.enabled;
                this.save_teleprompter();
                cx.notify();
            }),
        );
        if !settings.enabled || recording {
            return row;
        }
        let voice = if !settings.follow_voice {
            None
        } else if transcription::whisper_executable().is_none() {
            Some("Install whisper.cpp to scroll with your voice: sudo apt install whisper.cpp".to_string())
        } else if let Some(download) = self.voice_model_download.as_ref() {
            Some(format!("Downloading voice model… {:.0}%", download.get() * 100.0))
        } else if !SpeechModel::Base.is_installed() {
            Some(format!("Scrolling with your voice needs a {} MB model.", SpeechModel::Base.megabytes()))
        } else if !self.record_microphone {
            Some("Turn on the microphone to scroll with your voice.".into())
        } else {
            None
        };
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
                        let needs_model = transcription::whisper_executable().is_some()
                            && !SpeechModel::Base.is_installed()
                            && self.voice_model_download.is_none();
                        this.child(div().text_xs().text_color(muted()).child(message)).when(needs_model, |this| {
                            this.child(self.launcher_button(
                                "teleprompter-model",
                                format!("Download voice model ({} MB)", SpeechModel::Base.megabytes()),
                                true,
                                cx,
                                |this, cx| this.download_voice_model(cx),
                            ))
                        })
                    })
                    .child(div().text_xs().text_color(muted()).child(
                        "The prompter is a window on your screen: record a window or an area, or move it to another display, to keep it out of the video.",
                    ))
                    .child(div().h(px(1.0)).bg(line()))
                    .child(div().text_xs().text_color(blue()).child("Scroll the prompter or click a word to jump.")),
            )
            .into_any_element()
    }
}

/// The prompter window handle type, stored on the studio.
pub(crate) type PrompterHandle = WindowHandle<Prompter>;
