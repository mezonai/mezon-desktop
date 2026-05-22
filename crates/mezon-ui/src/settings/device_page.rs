use std::sync::Arc;
use std::time::Duration;

use gpui::{ClickEvent, Context, FontWeight, SharedString, Task, Window, div, prelude::*};
use gpui_component::{Icon, IconName, h_flex, label::Label, v_flex};
use mezon_client::AppApi;

use crate::theme::Theme;
use crate::util::{check_connection, retry};

#[derive(Debug, Clone)]
struct DeviceViewModel {
    device_id: String,
    device_name: String,
    platform: String,
    ip: String,
    location: String,
    is_current: bool,
    last_active_seconds: u32,
}

pub struct DevicePage {
    api: Arc<AppApi>,
    devices: Option<Vec<DeviceViewModel>>,
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
                    let view_models: Vec<DeviceViewModel> = devices
                        .into_iter()
                        .map(|d| DeviceViewModel {
                            device_id: d.device_id,
                            device_name: d.device_name,
                            platform: d.platform,
                            ip: d.ip,
                            location: d.location,
                            is_current: d.is_current,
                            last_active_seconds: d.last_active_seconds,
                        })
                        .collect();
                    this.update(cx, |this, view_cx| {
                        this.devices = Some(view_models);
                        this.device_error = None;
                        this.loading = false;
                        this.initial_loaded = true;
                        view_cx.notify();
                    })
                    .ok();
                }
                Err(_) => {
                    this.update(cx, |this, view_cx| {
                        this.device_error =
                            Some("Failed to load devices after multiple attempts".into());
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
            .child(
                Label::new("Manage the devices that have access to your account.")
                    .text_sm()
                    .text_color(theme.text_muted),
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
                    let current: Vec<&DeviceViewModel> = devices.iter().filter(|d| d.is_current).collect();
                    let others: Vec<&DeviceViewModel> = devices.iter().filter(|d| !d.is_current).collect();

                    v_flex()
                        .gap_6()
                        // Current Device Section
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    Label::new("CURRENT DEVICE")
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_muted),
                                )
                                .children(current.iter().map(|device| {
                                    let last_active = format_last_active(device.last_active_seconds);
                                    h_flex()
                                        .items_center()
                                        .gap_3()
                                        .px_4()
                                        .py_3()
                                        .rounded_lg()
                                        .bg(theme.bg_primary)
                                        .child(
                                            Icon::new(IconName::WindowMaximize)
                                                .size_5()
                                                .text_color(theme.status_online),
                                        )
                                        .child(
                                            v_flex()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(theme.text_primary)
                                                        .child(device.device_name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.text_muted)
                                                        .child(format!("{} · {} · Active now", device.platform, device.ip)),
                                                ),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.status_online)
                                                .child("Active now"),
                                        )
                                        .into_any_element()
                                })),
                        )
                        // Other Devices Section
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    Label::new("OTHER DEVICES")
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_muted),
                                )
                                .when(others.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.text_muted)
                                            .px_4()
                                            .child("No other devices."),
                                    )
                                })
                                .children(others.iter().map(|device| {
                                    let last_active = format_last_active(device.last_active_seconds);
                                    let device_id = device.device_id.clone();
                                    let api = self.api.clone();
                                    h_flex()
                                        .items_center()
                                        .gap_3()
                                        .px_4()
                                        .py_3()
                                        .rounded_lg()
                                        .bg(theme.bg_secondary)
                                        .child(
                                            Icon::new(IconName::Speaker)
                                                .size_5()
                                                .text_color(theme.text_secondary),
                                        )
                                        .child(
                                            v_flex()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(theme.text_primary)
                                                        .child(device.device_name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.text_muted)
                                                        .child(format!("{} · {}", device.platform, device.location)),
                                                ),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.text_muted)
                                                .child(last_active),
                                        )
                                        .child(
                                            div()
                                                .id(format!("remove-device-{}", device_id))
                                                .cursor_pointer()
                                                .text_color(theme.status_dnd)
                                                .child("✕")
                                                .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                                                    // TODO: Wire to actual logout_device call with session tokens
                                                    tracing::info!("Remove device: {}", device_id);
                                                }),
                                        )
                                        .into_any_element()
                                })),
                        )
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

fn format_last_active(seconds: u32) -> String {
    if seconds == 0 {
        return "Unknown".to_string();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    let ago = now.saturating_sub(seconds);
    if ago < 60 {
        format!("{}s ago", ago)
    } else if ago < 3600 {
        format!("{}m ago", ago / 60)
    } else if ago < 86400 {
        format!("{}h ago", ago / 3600)
    } else {
        format!("{}d ago", ago / 86400)
    }
}
