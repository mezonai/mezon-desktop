use crate::components::primitives::{Icon, IconName, Label, h_flex, v_flex};
use gpui::{Context, Entity, FontWeight, SharedString, Window, div, prelude::*, px};
use mezon_store::{AccountStore, AuthState, Settings};

use crate::theme::ActiveTheme;

pub struct DevicePage {
    settings: Entity<Settings>,
    auth_state: Entity<AuthState>,
}

impl DevicePage {
    pub fn new(
        settings: Entity<Settings>,
        auth_state: Entity<AuthState>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&AccountStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        AccountStore::global(cx).update(cx, |store, cx| store.ensure_devices(cx));
        Self {
            settings,
            auth_state,
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        AccountStore::global(cx).update(cx, |store, cx| store.fetch_devices(cx));
    }

    fn remove_device(&mut self, device_id: String, cx: &mut Context<Self>) {
        let auth = self.auth_state.read(cx).clone();
        let (token, refresh_token) = match auth {
            AuthState::Authenticated(s) | AuthState::Connecting(s) => (s.token, s.refresh_token),
            _ => return,
        };
        AccountStore::global(cx).update(cx, |store, cx| {
            store.remove_device(token, refresh_token, device_id, cx);
        });
    }
}

impl Render for DevicePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let store = AccountStore::global(cx).read(cx);

        let platform_label = |platform: &str| -> SharedString {
            if platform.trim().is_empty() {
                mezon_i18n::t(&locale, "setting.deviceSettings.platformDesktop")
                    .to_uppercase()
                    .into()
            } else {
                platform.to_uppercase().into()
            }
        };

        let device_icon = |platform: &str| {
            let normalized = platform.trim().to_ascii_lowercase();
            let is_mobile =
                normalized == "mobile" || normalized == "android" || normalized == "ios";
            div()
                .size(px(40.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(theme.bg_primary)
                .child(
                    Icon::new(if is_mobile {
                        IconName::DeviceMobileIcon
                    } else {
                        IconName::DeviceDesktopIcon
                    })
                    .size_5()
                    .text_color(theme.text_primary),
                )
        };

        v_flex()
            .gap_8()
            .child(
                v_flex()
                    .gap_4()
                    .child(
                        Label::new(mezon_i18n::t(
                            &locale,
                            "setting.deviceSettings.description1",
                        ))
                        .text_sm()
                        .text_color(theme.text_muted),
                    )
                    .child(
                        Label::new(mezon_i18n::t(
                            &locale,
                            "setting.deviceSettings.description2",
                        ))
                        .text_sm()
                        .text_color(theme.text_muted),
                    ),
            )
            .child(if let Some(error) = &store.devices_error {
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(error.clone())
                    .into_any_element()
            } else if store.devices_loading {
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(mezon_i18n::t(&locale, "setting.devices.loading"))
                    .into_any_element()
            } else if store.devices.is_empty() {
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(mezon_i18n::t(&locale, "setting.devices.none"))
                    .into_any_element()
            } else {
                let current: Vec<_> = store.devices.iter().filter(|d| d.is_current).collect();
                let others: Vec<_> = store.devices.iter().filter(|d| !d.is_current).collect();

                v_flex()
                    .gap_8()
                    .child(
                        v_flex()
                            .gap_4()
                            .p_4()
                            .rounded_lg()
                            .bg(theme.bg_secondary)
                            .shadow_sm()
                            .child(
                                Label::new(mezon_i18n::t(
                                    &locale,
                                    "setting.deviceSettings.currentDevice",
                                ))
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary),
                            )
                            .children(current.iter().map(|device| {
                                h_flex()
                                    .items_center()
                                    .gap_4()
                                    .py_4()
                                    .child(device_icon(&device.platform))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(platform_label(&device.platform)),
                                    )
                                    .into_any_element()
                            })),
                    )
                    .child(
                        v_flex()
                            .gap_4()
                            .p_4()
                            .rounded_lg()
                            .bg(theme.bg_secondary)
                            .shadow_sm()
                            .child(
                                Label::new(mezon_i18n::t(
                                    &locale,
                                    "setting.deviceSettings.otherDevices",
                                ))
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary),
                            )
                            .when(others.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.text_muted)
                                        .child(mezon_i18n::t(&locale, "setting.devices.noOther")),
                                )
                            })
                            .children(others.iter().map(|device| {
                                let device_id = device.device_id.clone();
                                let detail =
                                    match (device.location.trim(), device.last_active_label.trim())
                                    {
                                        ("", "") | ("", "Unknown") => None,
                                        ("", last_active) => Some(last_active.to_string()),
                                        (location, "") | (location, "Unknown") => {
                                            Some(location.to_string())
                                        }
                                        (location, last_active) => {
                                            Some(format!("{location} · {last_active}"))
                                        }
                                    };

                                h_flex()
                                    .items_center()
                                    .gap_4()
                                    .py_4()
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .child(device_icon(&device.platform))
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(theme.text_primary)
                                                    .child(platform_label(&device.platform)),
                                            )
                                            .when_some(detail, |content, detail| {
                                                content.child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(theme.text_muted)
                                                        .child(detail),
                                                )
                                            }),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .id(format!("remove-device-{}", device_id))
                                            .size(px(24.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_full()
                                            .cursor_pointer()
                                            .bg(theme.text_muted)
                                            .hover(move |style| style.bg(theme.danger))
                                            .child(
                                                Icon::new(IconName::Close)
                                                    .size(px(16.))
                                                    .text_color(theme.bg_primary),
                                            )
                                            .on_click(cx.listener(
                                                move |this, _event, _window, cx| {
                                                    this.remove_device(device_id.clone(), cx);
                                                },
                                            )),
                                    )
                                    .into_any_element()
                            })),
                    )
                    .into_any_element()
            })
    }
}
