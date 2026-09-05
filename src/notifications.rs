use super::*;

/// A fresh identity makes repeated messages restart their lifetime too.
#[derive(Clone)]
pub(crate) struct Notification {
    id: u64,
    message: SharedString,
    expires_at: Instant,
    error: bool,
    open_path: Option<PathBuf>,
}

impl From<String> for Notification {
    fn from(message: String) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let lower = message.to_lowercase();
        let error = ["failed", "could not", "error"]
            .iter()
            .any(|word| lower.contains(word));
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            message: message.into(),
            expires_at: Instant::now() + Duration::from_secs(if error { 8 } else { 5 }),
            error,
            open_path: None,
        }
    }
}

impl Notification {
    pub(crate) fn exported(message: String, path: PathBuf) -> Self {
        let mut toast = Self::from(message);
        toast.error = false;
        toast.expires_at = Instant::now() + Duration::from_secs(8);
        toast.open_path = Some(path);
        toast
    }
}

impl From<&str> for Notification {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

fn dismiss_notification(current: &mut Option<Notification>, id: u64) -> bool {
    if current.as_ref().is_some_and(|toast| toast.id == id) {
        *current = None;
        true
    } else {
        false
    }
}

impl Studio {
    fn sync_notification_timer(&mut self, cx: &mut Context<Self>) {
        let id = self.toast.as_ref().map(|toast| toast.id);
        if self.toast_timer_id == id {
            return;
        }
        self.toast_timer = None;
        self.toast_timer_id = id;
        if let Some(toast) = &self.toast {
            let id = toast.id;
            let delay = toast.expires_at.saturating_duration_since(Instant::now());
            self.toast_timer = Some(cx.spawn(async move |weak, cx| {
                Timer::after(delay).await;
                let _ = weak.update(cx, |this, cx| {
                    if dismiss_notification(&mut this.toast, id) {
                        cx.notify();
                    }
                });
            }));
        }
    }

    fn notification_card(&self, toast: Notification, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("notification")
            .absolute()
            .bottom(px(20.0))
            .right(px(20.0))
            .w(px(356.0))
            .p_4()
            .flex()
            .items_start()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0xe4e4e7))
            .bg(rgb(0xffffff))
            .shadow_lg()
            .font_family("Inter")
            .text_sm()
            .text_color(rgb(0x18181b))
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(if toast.error { 0xdc2626 } else { 0x2563eb }))
                    .child(if toast.error { "!" } else { "ℹ" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .child(toast.message.replace('/', "/\u{200b}")),
            )
            .when_some(toast.open_path, |card, path| {
                card.child(
                    div()
                        .id("open-export")
                        .flex_shrink_0()
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0x18181b))
                        .text_color(rgb(0xffffff))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x3f3f46)))
                        .child("Open")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let path = path.clone();
                            let task = cx.background_executor().spawn(async move {
                                std::process::Command::new("xdg-open")
                                    .arg(&path)
                                    .status()
                                    .map_err(|error| error.to_string())
                                    .and_then(|status| {
                                        if status.success() {
                                            Ok(())
                                        } else {
                                            Err(format!("default viewer exited with {status}"))
                                        }
                                    })
                            });
                            this.toast = None;
                            cx.notify();
                            cx.spawn(async move |weak, cx| {
                                if let Err(error) = task.await {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.toast =
                                            Some(format!("Could not open export: {error}").into());
                                        cx.notify();
                                    });
                                }
                            })
                            .detach();
                        })),
                )
            })
            .child(
                div()
                    .id("dismiss-notification")
                    .flex_shrink_0()
                    .size(px(20.0))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0xf4f4f5)))
                    .child(
                        svg()
                            .path("icons/close.svg")
                            .size(px(14.0))
                            .text_color(rgb(0x71717a)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toast = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_notification_timer(cx);
        let content = self.render_content(window, cx);
        let toast = self.toast.clone();
        div()
            .relative()
            .size_full()
            .child(content)
            .when_some(toast, |element, toast| {
                element.child(self.notification_card(toast, cx))
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
        assert_eq!(toast.open_path, Some(path));
        assert!(!toast.error);
        assert!(Notification::from("Export failed").open_path.is_none());
        assert!(Notification::from("Export cancelled").open_path.is_none());
    }

    #[test]
    fn repeated_notifications_have_independent_lifetimes() {
        let first = Notification::from("Wallpaper selected");
        let second = Notification::from("Wallpaper selected");
        assert_ne!(first.id, second.id);
        assert!(second.expires_at >= first.expires_at);
        assert!(!first.error);
        let mut current = Some(second.clone());
        assert!(!dismiss_notification(&mut current, first.id));
        assert_eq!(current.as_ref().unwrap().id, second.id);
        assert!(dismiss_notification(&mut current, second.id));
        assert!(current.is_none());
        assert!(!dismiss_notification(&mut current, second.id));
    }

    #[test]
    fn errors_have_a_longer_but_finite_timeout() {
        let start = Instant::now();
        let info = Notification::from("Exported image");
        let error = Notification::from("Export failed: disk full");
        assert!(error.error);
        assert!(info.expires_at >= start + Duration::from_secs(5));
        assert!(error.expires_at >= start + Duration::from_secs(8));
        assert!(error.expires_at < Instant::now() + Duration::from_secs(9));
    }
}
