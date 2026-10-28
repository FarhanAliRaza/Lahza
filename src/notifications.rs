use super::*;
use gpui::{Animation, AnimationExt as _};
use std::path::Path;
use std::rc::Rc;

/// A fresh identity makes repeated messages restart their lifetime too.
#[derive(Clone)]
pub(crate) struct Notification {
    id: u64,
    message: SharedString,
    /// The raw error behind a plain-language message, shown as a tooltip.
    detail: Option<SharedString>,
    /// How long the toast stays up; `None` waits for the user.
    lifetime: Option<Duration>,
    error: bool,
    open_path: Option<PathBuf>,
    reveal_path: Option<PathBuf>,
    action: Option<NotificationAction>,
}

/// A labelled button on the toast, e.g. Undo.
#[derive(Clone)]
pub(crate) struct NotificationAction {
    label: SharedString,
    run: Rc<dyn Fn(&mut Studio, &mut Context<Studio>)>,
}

/// Long enough to read: 60 ms a character, never under `floor`.
fn reading_time(message: &str, floor: Duration) -> Duration {
    floor.max(Duration::from_millis(60 * message.chars().count() as u64))
}

impl Notification {
    fn new(message: String, error: bool) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        // Errors from `recording` read as sentences; toasts drop the period.
        let message = match message.strip_suffix('.') {
            Some(trimmed) if !trimmed.ends_with('.') => trimmed.to_owned(),
            _ => message,
        };
        let floor = Duration::from_secs(if error { 8 } else { 5 });
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            lifetime: Some(reading_time(&message, floor)),
            message: message.into(),
            detail: None,
            error,
            open_path: None,
            reveal_path: None,
            action: None,
        }
    }

    pub(crate) fn info(message: impl Into<String>) -> Self {
        Self::new(message.into(), false)
    }

    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self::new(message.into(), true)
    }

    /// Attach the raw error behind a plain-language message.
    pub(crate) fn with_detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A saved export: Open plus Show in folder.
    pub(crate) fn exported(message: String, path: PathBuf) -> Self {
        let mut toast = Self::info(message);
        toast.lifetime = toast.lifetime.map(|lifetime| lifetime.max(Duration::from_secs(8)));
        toast.reveal_path = Some(path.clone());
        toast.open_path = Some(path);
        toast
    }

    /// A file written somewhere the user may want to find: Show in folder.
    pub(crate) fn saved(message: String, path: PathBuf) -> Self {
        let mut toast = Self::info(message);
        toast.lifetime = toast.lifetime.map(|lifetime| lifetime.max(Duration::from_secs(8)));
        toast.reveal_path = Some(path);
        toast
    }

    /// Info toast with one action button; it stays up long enough to use it.
    pub(crate) fn with_action(
        message: impl Into<String>,
        label: impl Into<SharedString>,
        run: impl Fn(&mut Studio, &mut Context<Studio>) + 'static,
    ) -> Self {
        Self::info(message).action(label, run)
    }

    /// Add a button. An error with a way to recover waits for the user.
    pub(crate) fn action(
        mut self,
        label: impl Into<SharedString>,
        run: impl Fn(&mut Studio, &mut Context<Studio>) + 'static,
    ) -> Self {
        self.lifetime = if self.error {
            None
        } else {
            self.lifetime.map(|lifetime| lifetime.max(Duration::from_secs(8)))
        };
        self.action = Some(NotificationAction { label: label.into(), run: Rc::new(run) });
        self
    }
}

impl From<String> for Notification {
    fn from(message: String) -> Self {
        Self::info(message)
    }
}

impl From<&str> for Notification {
    fn from(message: &str) -> Self {
        Self::info(message)
    }
}

/// Plain-language wording for an export failure. `context` leads the
/// fallback ("Export failed"); the raw error stays in the tooltip.
pub(crate) fn failure(context: &str, raw: &str, directory: Option<&Path>) -> Notification {
    let lower = raw.to_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    let message = if any(&["no space left", "disk full", "enospc", "os error 28", "quota exceeded"]) {
        match directory {
            Some(directory) => format!("Not enough disk space in {}.", directory.display()),
            None => "Not enough disk space.".to_string(),
        }
    } else {
        return Notification::error(format!("{context}: {raw}"));
    };
    Notification::error(message).with_detail(raw.to_string())
}

fn dismiss_notification(current: &mut Option<Notification>, id: u64) -> Option<Notification> {
    if current.as_ref().is_some_and(|toast| toast.id == id) {
        current.take()
    } else {
        None
    }
}

const TOAST_ENTER: Duration = Duration::from_millis(160);
const TOAST_EXIT: Duration = Duration::from_millis(120);

/// Ask the file manager to select `path`; fall back to opening its folder.
async fn show_in_folder(path: PathBuf) -> Result<(), String> {
    let shown = async {
        let uri = ashpd::url::Url::from_file_path(&path).map_err(|()| "not an absolute path".to_string())?;
        let connection = ashpd::zbus::Connection::session().await.map_err(|error| error.to_string())?;
        connection
            .call_method(
                Some("org.freedesktop.FileManager1"),
                "/org/freedesktop/FileManager1",
                Some("org.freedesktop.FileManager1"),
                "ShowItems",
                &(vec![uri.as_str()], ""),
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    };
    if shown.await.is_ok() {
        return Ok(());
    }
    let folder = path.parent().unwrap_or(&path).to_path_buf();
    open_with_default_app(folder.as_os_str())
}

fn open_with_default_app(target: &std::ffi::OsStr) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(target)
        .status()
        .map_err(|error| error.to_string())
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("default viewer exited with {status}"))
            }
        })
}

impl Studio {
    fn sync_notification_timer(&mut self, cx: &mut Context<Self>) {
        // Hovering holds the toast; leaving restarts its full lifetime.
        let id = self
            .toast
            .as_ref()
            .map(|toast| toast.id)
            .filter(|id| self.toast_hovered != Some(*id));
        if self.toast_timer_id == id {
            return;
        }
        self.toast_timer = None;
        self.toast_timer_id = id;
        if let Some((id, delay)) = id.zip(self.toast.as_ref().and_then(|toast| toast.lifetime)) {
            self.toast_timer = Some(cx.spawn(async move |weak, cx| {
                Timer::after(delay).await;
                let _ = weak.update(cx, |this, cx| this.dismiss_toast(id, cx));
            }));
        }
    }

    /// Fade the toast out, then drop it.
    fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(toast) = dismiss_notification(&mut self.toast, id) else { return };
        if crate::theme::animations_enabled() {
            self.toast_exiting = Some(toast);
            cx.spawn(async move |weak, cx| {
                Timer::after(TOAST_EXIT).await;
                let _ = weak.update(cx, |this, cx| {
                    if this.toast_exiting.as_ref().is_some_and(|toast| toast.id == id) {
                        this.toast_exiting = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn run_in_background(
        &self,
        failure: &'static str,
        job: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let task = cx.background_executor().spawn(job);
        cx.spawn(async move |weak, cx| {
            if let Err(error) = task.await {
                let _ = weak.update(cx, |this, cx| {
                    this.toast = Some(Notification::error(format!("{failure}: {error}")));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn notification_card(&self, toast: Notification, exiting: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = toast.id;
        let card = div()
            .id(("notification", id))
            .relative()
            .w(px(356.0))
            .p_3()
            .flex()
            .items_start()
            .gap_3()
            .rounded_lg()
            .float()
            .font_family("Inter")
            .text_sm()
            .text_color(ink())
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let hovered = hovered.then_some(id);
                if this.toast_hovered != hovered {
                    this.toast_hovered = hovered;
                    cx.notify();
                }
            }))
            .child(
                svg()
                    .path(if toast.error { "icons/alert.svg" } else { "icons/info.svg" })
                    .size(px(16.0))
                    .mt(px(2.0))
                    .flex_none()
                    .text_color(if toast.error { danger() } else { muted() }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .id(("notification-message", id))
                            .whitespace_normal()
                            .line_height(px(20.0))
                            .child(toast.message.replace('/', "/\u{200b}"))
                            .when_some(toast.detail.clone(), |this, detail| {
                                this.tooltip(crate::timeline_ui::tooltip(detail))
                            }),
                    )
                    .when(
                        toast.open_path.is_some()
                            || toast.reveal_path.is_some()
                            || toast.action.is_some(),
                        |this| this.child(self.notification_actions(&toast, cx)),
                    ),
            )
            .child(
                div()
                    .id(("dismiss-notification", id))
                    .flex_none()
                    .size(px(24.0))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(control_bg()))
                    .tooltip(crate::timeline_ui::tooltip("Dismiss"))
                    .child(svg().path("icons/close.svg").size(px(14.0)).text_color(muted()))
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss_toast(id, cx))),
            );
        if !crate::theme::animations_enabled() {
            return card.into_any_element();
        }
        if exiting {
            card.with_animation(
                ("notification-exit", id),
                Animation::new(TOAST_EXIT).with_easing(gpui::ease_in_out),
                |card, delta| card.opacity(1.0 - delta).top(px(6.0 * delta)),
            )
            .into_any_element()
        } else {
            card.with_animation(
                ("notification-enter", id),
                Animation::new(TOAST_ENTER).with_easing(gpui::ease_out_quint()),
                |card, delta| card.opacity(delta).top(px(6.0 * (1.0 - delta))),
            )
            .into_any_element()
        }
    }

    fn notification_actions(&self, toast: &Notification, cx: &mut Context<Self>) -> AnyElement {
        use crate::controls::{button, ButtonKind, ButtonSize};
        let id = toast.id;
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .when_some(toast.action.clone(), |row, action| {
                let run = action.run.clone();
                row.child(
                    button(("notification-action", id), action.label, ButtonKind::Secondary, ButtonSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toast = None;
                            run(this, cx);
                            cx.notify();
                        })),
                )
            })
            .when_some(toast.open_path.clone(), |row, path| {
                row.child(
                    button(("notification-open", id), "Open", ButtonKind::Secondary, ButtonSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let path = path.clone();
                            this.dismiss_toast(id, cx);
                            this.run_in_background(
                                "Could not open export",
                                async move { open_with_default_app(path.as_os_str()) },
                                cx,
                            );
                        })),
                )
            })
            .when_some(toast.reveal_path.clone(), |row, path| {
                row.child(
                    button(("notification-reveal", id), "Show in folder", ButtonKind::Secondary, ButtonSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let path = path.clone();
                            this.dismiss_toast(id, cx);
                            this.run_in_background(
                                "Could not show the folder",
                                show_in_folder(path),
                                cx,
                            );
                        })),
                )
            })
            .into_any_element()
    }

    /// Toasts stacked above the export panel in one bottom-right column.
    fn notification_stack(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let export = self.export_status_overlay(cx);
        if self.toast.is_none() && self.toast_exiting.is_none() && export.is_none() {
            return None;
        }
        Some(
            div()
                .absolute()
                .bottom(px(crate::theme::SPACE_4))
                .right(px(crate::theme::SPACE_4))
                .flex()
                .flex_col()
                .items_end()
                .gap(px(crate::theme::SPACE_2))
                .when_some(self.toast_exiting.clone(), |stack, toast| {
                    stack.child(self.notification_card(toast, true, cx))
                })
                .when_some(self.toast.clone(), |stack, toast| {
                    stack.child(self.notification_card(toast, false, cx))
                })
                .when_some(export, |stack, panel| stack.child(panel))
                .into_any_element(),
        )
    }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_notification_timer(cx);
        let content = self.render_content(window, cx);
        let stack = self.notification_stack(cx);
        div()
            .relative()
            .size_full()
            .child(content)
            .when_some(stack, |element, stack| element.child(stack))
            .when(self.capture_access_prompt.is_some(), |element| {
                element.child(self.capture_access_dialog(cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_action_keeps_the_exact_path_separate_from_the_message() {
        let path = PathBuf::from("/tmp/error reports/image & video #1.png");
        let toast = Notification::exported(format!("Exported to {}", path.display()), path.clone());
        assert_eq!(toast.open_path, Some(path.clone()));
        assert_eq!(toast.reveal_path, Some(path));
        assert!(!toast.error);
        assert!(Notification::error("Export failed").open_path.is_none());
        assert!(Notification::from("Export cancelled").open_path.is_none());
    }

    #[test]
    fn repeated_notifications_have_independent_lifetimes() {
        let first = Notification::from("Wallpaper selected");
        let second = Notification::from("Wallpaper selected");
        assert_ne!(first.id, second.id);
        assert!(!first.error);
        let mut current = Some(second.clone());
        assert!(dismiss_notification(&mut current, first.id).is_none());
        assert_eq!(current.as_ref().unwrap().id, second.id);
        assert!(dismiss_notification(&mut current, second.id).is_some());
        assert!(current.is_none());
        assert!(dismiss_notification(&mut current, second.id).is_none());
    }

    #[test]
    fn lifetimes_follow_length_and_severity() {
        let info = Notification::info("Exported image");
        let error = Notification::error("Export failed: disk full");
        assert!(error.error && !info.error);
        assert_eq!(info.lifetime, Some(Duration::from_secs(5)));
        assert_eq!(error.lifetime, Some(Duration::from_secs(8)));
        let long = Notification::info("x".repeat(200));
        assert_eq!(long.lifetime, Some(Duration::from_millis(12_000)));
        let recoverable = Notification::error("Upload failed").action("Retry", |_, _| {});
        assert_eq!(recoverable.lifetime, None);
    }

    #[test]
    fn toasts_drop_a_trailing_period() {
        assert_eq!(Notification::error("Audio preparation paused.").message.as_ref(), "Audio preparation paused");
        assert_eq!(Notification::info("Loading...").message.as_ref(), "Loading...");
    }

    #[test]
    fn failures_read_in_plain_language() {
        let full = failure("Export failed", "No space left on device (os error 28)", Some(Path::new("/home/me/Videos")));
        assert_eq!(full.message.as_ref(), "Not enough disk space in /home/me/Videos");
        assert_eq!(full.detail.as_ref().map(|detail| detail.as_ref()), Some("No space left on device (os error 28)"));
        let other = failure("Export failed", "encoder crashed", None);
        assert_eq!(other.message.as_ref(), "Export failed: encoder crashed");
        assert!(other.error);
    }
}
