use crate::components::primitives::{Icon, IconName, Label, h_flex, v_flex};
use gpui::{Context, Entity, FontWeight, Window, div, prelude::*};
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

        v_flex()
            .gap_4()
            .child(
                Label::new(mezon_i18n::t(&locale, "setting.devices.description"))
                    .text_sm()
                    .text_color(theme.text_muted),
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
                    .gap_6()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                Label::new(mezon_i18n::t(&locale, "setting.devices.currentDevice"))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_muted),
                            )
                            .children(current.iter().map(|device| {
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
                                                div().text_xs().text_color(theme.text_muted).child(
                                                    format!(
                                                        "{} · {} · {}",
                                                        device.platform,
                                                        device.ip,
                                                        mezon_i18n::t(
                                                            &locale,
                                                            "setting.devices.activeNow"
                                                        )
                                                    ),
                                                ),
                                            ),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        div().text_xs().text_color(theme.status_online).child(
                                            mezon_i18n::t(&locale, "setting.devices.activeNow"),
                                        ),
                                    )
                                    .into_any_element()
                            })),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                Label::new(mezon_i18n::t(&locale, "setting.devices.otherDevices"))
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
                                        .child(mezon_i18n::t(&locale, "setting.devices.noOther")),
                                )
                            })
                            .children(others.iter().map(|device| {
                                let last_active = device.last_active_label.clone();
                                let device_id = device.device_id.clone();
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
                                                div().text_xs().text_color(theme.text_muted).child(
                                                    format!(
                                                        "{} · {}",
                                                        device.platform, device.location
                                                    ),
                                                ),
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
                                            .child(
                                                Icon::new(IconName::Close)
                                                    .size_4()
                                                    .text_color(theme.status_dnd),
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
