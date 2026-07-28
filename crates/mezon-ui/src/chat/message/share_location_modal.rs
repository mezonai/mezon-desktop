use gpui::{
    App, ClickEvent, Context, FocusHandle, Focusable, FontWeight, Render, SharedString, Task,
    Window, div, prelude::*, px,
};
use mezon_store::{MessagesStore, PlatformStore};

use crate::app::shell::Shell;
use crate::components::primitives::{Button, ButtonVariants};
use crate::theme::ActiveTheme;

pub struct ShareLocationModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    latitude: Option<f64>,
    longitude: Option<f64>,
    loading: bool,
    error: bool,
    _fetch_task: Option<Task<()>>,
}

impl Focusable for ShareLocationModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ShareLocationModal {
    pub fn open(locale: SharedString, window: &mut Window, cx: &mut App) {
        if Shell::global(cx).read(cx).has_modal() {
            return;
        }
        let view = cx.new(|cx| {
            let mut modal = Self {
                focus_handle: cx.focus_handle(),
                locale,
                latitude: None,
                longitude: None,
                loading: true,
                error: false,
                _fetch_task: None,
            };
            modal.start_fetch(window, cx);
            modal
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn start_fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fetch =
            PlatformStore::try_global(cx).and_then(|store| store.read(cx).current_location_fn());
        self._fetch_task = Some(cx.spawn_in(window, async move |this, cx| {
            let location = if let Some(fetch) = fetch {
                cx.background_executor().spawn(async move { fetch() }).await
            } else {
                Err(anyhow::anyhow!("current_location not registered"))
            };
            this.update(cx, |this, cx| {
                this.loading = false;
                match location {
                    Ok((lat, lng)) => {
                        this.latitude = Some(lat);
                        this.longitude = Some(lng);
                        this.error = false;
                    }
                    Err(error) => {
                        tracing::warn!("current location unavailable: {error}");
                        this.error = true;
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let Some(latitude) = self.latitude else {
            return;
        };
        let Some(longitude) = self.longitude else {
            return;
        };
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_location_message(latitude, longitude, cx);
        });
        Self::close(cx);
    }

    fn coordinate_label(&self) -> String {
        match (self.latitude, self.longitude) {
            (Some(lat), Some(lng)) => {
                let prefix = mezon_i18n::t(&self.locale, "message.shareLocationModal.coordinate");
                format!("{prefix} ({lat:.6}, {lng:.6})")
            }
            _ => String::new(),
        }
    }
}

impl Render for ShareLocationModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let title = mezon_i18n::t(&self.locale, "message.shareLocationModal.sendThisLocation");
        let send_label = mezon_i18n::t(&self.locale, "message.shareLocationModal.send");
        let cancel_label = mezon_i18n::t(&self.locale, "message.shareLocationModal.cancel");
        let body = if self.loading {
            mezon_i18n::t(&self.locale, "message.mapView.yourLocation").to_string()
        } else if self.error {
            mezon_i18n::t(
                &self.locale,
                "common.permissionNotification.locationPermissionDesc",
            )
            .to_string()
        } else {
            self.coordinate_label()
        };
        let can_send = !self.loading && !self.error && self.latitude.is_some();
        let entity = cx.entity();
        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_this, _: &::menu::Cancel, _window, cx| {
                Self::close(cx);
            }))
            .w(px(420.))
            .rounded(px(8.))
            .bg(theme.tokens.theme_setting_primary)
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(title),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("share-location-cancel")
                            .label(cancel_label)
                            .on_click(move |_: &ClickEvent, _window, cx| {
                                ShareLocationModal::close(cx);
                            }),
                    )
                    .child(
                        Button::new("share-location-send")
                            .primary()
                            .label(send_label)
                            .when(!can_send, |button| button.disabled(true))
                            .on_click(move |_: &ClickEvent, _window, cx| {
                                if can_send {
                                    entity.update(cx, |this, cx| this.send(cx));
                                }
                            }),
                    ),
            )
    }
}
