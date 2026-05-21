use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, FontWeight, SharedString, Task, Window, div, prelude::*};
use gpui_component::{Icon, IconName, h_flex, label::Label, v_flex};
use mezon_client::AppApi;
use mezon_proto::api::LogedDevice;

use crate::theme::Theme;
use crate::util::{check_connection, retry};

pub struct DevicePage {
    api: Arc<AppApi>,
    devices: Option<Vec<LogedDevice>>,
    device_error: Option<SharedString>,
    loading: bool,
    initial_loaded: bool,
    _fetch_task: Option<Task<()>>,
}

impl DevicePage {
    pub fn new(api: Arc<AppApi>, _cx: &mut Context<Self>) -> Self {
        Self {
            api,
            devices: None,
            device_error: None,
            loading: true,
            initial_loaded: false,
            _fetch_task: None,
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.initial_loaded {
            // Re-fetch data
            self.loading = true;
            self.device_error = None;
            cx.notify();
            self.fetch(cx);
        }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            if check_connection(cx.background_executor(), &api)
                .await
                .is_err()
            {
                this.update(cx, |this, view_cx| {
                    this.loading = false;
                    this.device_error = Some("Connection failed".into());
                    view_cx.notify();
                })
                .ok();
                return;
            }

            match retry(
                cx.background_executor(),
                5,
                Duration::from_millis(1000),
                || {
                    let api = api.clone();
                    async move { api.list_loged_device().await }
                },
            )
            .await
            {
                Ok(devices) => {
                    this.update(cx, |this, view_cx| {
                        this.devices = Some(devices);
                        this.device_error = None;
                        this.loading = false;
                        this.initial_loaded = true;
                        view_cx.notify();
                    })
                    .ok();
                }
                Err(_) => {
                    this.update(cx, |this, view_cx| {
                        this.device_error = Some(
                            "Failed to load devices after multiple attempts".into(),
                        );
                        this.loading = false;
                        view_cx.notify();
                    })
                    .ok();
                }
            }
        }));
    }
}

impl Render for DevicePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Trigger initial fetch on first render
        if !self.initial_loaded {
            self.fetch(cx);
        }

        let theme = Theme::dark();

        v_flex()
            .gap_4()
            .child(
                Label::new("Devices")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::BOLD),
            )
            .child(if let Some(error) = &self.device_error {
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(error.clone())
                    .into_any_element()
            } else if let Some(devices) = &self.devices {
                if devices.is_empty() {
                    div()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child("No devices found.")
                        .into_any_element()
                } else {
                    v_flex()
                        .gap_2()
                        .children(devices.iter().map(|device| {
                            let last_active = if device.last_active_seconds > 0 {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs() as u32;
                                let ago = now.saturating_sub(device.last_active_seconds);
                                if ago < 60 {
                                    format!("{}s ago", ago)
                                } else if ago < 3600 {
                                    format!("{}m ago", ago / 60)
                                } else if ago < 86400 {
                                    format!("{}h ago", ago / 3600)
                                } else {
                                    format!("{}d ago", ago / 86400)
                                }
                            } else {
                                String::from("Unknown")
                            };

                            let current_label = if device.is_current { " (current)" } else { "" };

                            h_flex()
                                .items_center()
                                .justify_between()
                                .px_4()
                                .py_3()
                                .rounded_lg()
                                .bg(theme.bg_secondary)
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .child(
                                            Icon::new(IconName::SquareTerminal)
                                                .size_5()
                                                .text_color(theme.text_secondary),
                                        )
                                        .child(
                                            v_flex()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(theme.text_primary)
                                                        .child(format!(
                                                            "{}{}",
                                                            device.device_name, current_label
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.text_muted)
                                                        .child(format!(
                                                            "{} · {}",
                                                            device.platform, device.ip
                                                        )),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(last_active),
                                )
                                .into_any_element()
                        }))
                        .into_any_element()
                }
            } else {
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child("Loading devices...")
                    .into_any_element()
            })
    }
}
