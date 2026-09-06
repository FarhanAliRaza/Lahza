//! Permission gates for optional capture devices in the confined Snap.
use super::*;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CaptureAccess {
    Camera,
    Microphone,
    SystemAudio,
    CameraPicker,
    MicrophonePicker,
    Recording,
}

#[derive(Clone)]
pub(crate) struct AccessPrompt {
    action: CaptureAccess,
    message: String,
}

fn required_interfaces(action: CaptureAccess, camera: bool, audio: bool) -> Vec<&'static str> {
    match action {
        CaptureAccess::Camera | CaptureAccess::CameraPicker => vec!["camera"],
        CaptureAccess::Microphone
        | CaptureAccess::MicrophonePicker
        | CaptureAccess::SystemAudio => {
            vec!["audio-record"]
        }
        CaptureAccess::Recording => {
            let mut interfaces = Vec::new();
            if camera {
                interfaces.push("camera");
            }
            if audio {
                interfaces.push("audio-record");
            }
            interfaces
        }
    }
}

fn missing_interfaces(
    interfaces: &[&'static str],
    mut check: impl FnMut(&str) -> Result<bool, String>,
) -> Result<Vec<&'static str>, String> {
    let mut missing = Vec::new();
    for interface in interfaces {
        if !check(interface)? {
            missing.push(*interface);
        }
    }
    Ok(missing)
}

fn interface_connected(interface: &str) -> Result<bool, String> {
    let status = Command::new("snapctl")
        .args(["is-connected", interface])
        .output()
        .map_err(|error| format!("Could not check device access: {error}"))?
        .status;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err("Could not check device access. Open Lahza’s permissions and try again.".into()),
    }
}

fn permission_message(missing: &[&str]) -> String {
    let access = match missing {
        ["camera"] => "camera access",
        ["audio-record"] => "audio recording access (microphone and system sound)",
        _ => "camera and audio recording access",
    };
    format!("Lahza needs {access}. Open Permissions in Lahza’s app settings and allow access, then return here and choose Try again.")
}

impl Studio {
    pub(crate) fn request_capture_access(&mut self, action: CaptureAccess, cx: &mut Context<Self>) {
        if self.capture_access_busy {
            return;
        }
        let interfaces = required_interfaces(
            action,
            self.record_camera,
            self.record_microphone || self.record_system_audio,
        );
        let confined = std::env::var_os("SNAP").is_some();
        self.capture_access_busy = true;
        let task = cx.background_executor().spawn(async move {
            let missing = if confined {
                missing_interfaces(&interfaces, interface_connected)?
            } else {
                Vec::new()
            };
            if !missing.is_empty() {
                return Err(permission_message(&missing));
            }
            let cameras = if matches!(action, CaptureAccess::Camera | CaptureAccess::CameraPicker) {
                Some(camera_devices())
            } else {
                None
            };
            let microphones = if matches!(
                action,
                CaptureAccess::Microphone | CaptureAccess::MicrophonePicker
            ) {
                Some(microphone_devices())
            } else {
                None
            };
            Ok((cameras, microphones))
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.capture_access_busy = false;
                match result {
                    Err(message) => {
                        this.launcher_camera_menu_open = false;
                        this.launcher_mic_menu_open = false;
                        this.capture_access_prompt = Some(AccessPrompt { action, message });
                    }
                    Ok((cameras, microphones)) => {
                        this.capture_access_prompt = None;
                        if let Some(devices) = cameras {
                            this.camera_devices = devices;
                        }
                        if let Some(devices) = microphones {
                            this.microphone_devices = devices;
                        }
                        match action {
                            CaptureAccess::Camera => {
                                this.camera_access_checked = true;
                                this.record_camera = true;
                            }
                            CaptureAccess::Microphone => this.record_microphone = true,
                            CaptureAccess::SystemAudio => this.record_system_audio = true,
                            CaptureAccess::CameraPicker => this.launcher_camera_menu_open = true,
                            CaptureAccess::MicrophonePicker => this.launcher_mic_menu_open = true,
                            CaptureAccess::Recording => this.start_recording_with_access(cx),
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_capture_permissions(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move {
            Command::new("xdg-open")
                .arg("snap://lahza")
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        });
        cx.spawn(async move |weak, cx| {
            if !task.await {
                let _ = weak.update(cx, |this, cx| {
                    if let Some(prompt) = &mut this.capture_access_prompt {
                        prompt.message = "Could not open app settings. Open your system’s software app, find Lahza, and enable camera or audio recording in Permissions. Then choose Try again.".into();
                    }
                    cx.notify();
                });
            }
        }).detach();
    }

    pub(crate) fn capture_access_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let prompt = self
            .capture_access_prompt
            .as_ref()
            .expect("permission prompt");
        let action = prompt.action;
        div()
            .id("capture-access-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(gpui::hsla(0.0, 0.0, 0.0, 0.35))
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .w_full()
                    .max_w(px(380.0))
                    .p_5()
                    .rounded_lg()
                    .bg(gpui::white())
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .text_color(ink())
                    .text_sm()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Allow device access"),
                    )
                    .child(div().whitespace_normal().child(prompt.message.clone()))
                    .child(
                        div()
                            .id("open-capture-permissions")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x18181b))
                            .text_color(gpui::white())
                            .cursor_pointer()
                            .child("Open permissions")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_capture_permissions(cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .id("cancel-capture-access")
                                    .px_3()
                                    .py_2()
                                    .cursor_pointer()
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.capture_access_busy {
                                            this.capture_access_prompt = None;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("retry-capture-access")
                                    .px_3()
                                    .py_2()
                                    .cursor_pointer()
                                    .child(if self.capture_access_busy {
                                        "Checking…"
                                    } else {
                                        "Try again"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.request_capture_access(action, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_only_permissions_needed_by_the_action() {
        assert_eq!(
            required_interfaces(CaptureAccess::CameraPicker, false, false),
            ["camera"]
        );
        assert_eq!(
            required_interfaces(CaptureAccess::Microphone, false, false),
            ["audio-record"]
        );
        assert_eq!(
            required_interfaces(CaptureAccess::SystemAudio, false, false),
            ["audio-record"]
        );
        assert!(required_interfaces(CaptureAccess::Recording, false, false).is_empty());
        assert_eq!(
            required_interfaces(CaptureAccess::Recording, true, true),
            ["camera", "audio-record"]
        );
    }

    #[test]
    fn retry_rechecks_permissions_and_does_not_treat_errors_as_grants() {
        let required = ["camera", "audio-record"];
        assert_eq!(
            missing_interfaces(&required, |name| Ok(name == "camera")).unwrap(),
            ["audio-record"]
        );
        assert!(missing_interfaces(&required, |_| Ok(true))
            .unwrap()
            .is_empty());
        assert!(missing_interfaces(&required, |_| Err("snapd unavailable".into())).is_err());
    }
}
