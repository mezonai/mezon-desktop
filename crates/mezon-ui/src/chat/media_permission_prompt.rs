use gpui::{
    AnyElement, App, Context, Entity, FontWeight, Hsla, Pixels, SharedString, Subscription, Window,
    deferred, div, img, prelude::*, px, rgb,
};
use mezon_store::{MediaDevice, MediaPermissionPrompt, MediaPermissionStore, Settings};

use crate::components::primitives::{Button, ButtonVariants, Icon, IconName, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};
use crate::util::assets::APP_ICON;

const BADGE_TEXT: u32 = 0x1e1f22;

pub fn media_access_missing(device: MediaDevice, cx: &App) -> bool {
    MediaPermissionStore::try_global(cx).is_some_and(|store| !store.read(cx).is_granted(device))
}

fn media_access_flags(cx: &App) -> (bool, bool) {
    (
        media_access_missing(MediaDevice::Microphone, cx),
        media_access_missing(MediaDevice::Camera, cx),
    )
}

pub fn observe_media_access<V: 'static>(cx: &mut Context<V>) -> Option<Subscription> {
    let store = MediaPermissionStore::try_global(cx)?;
    let mut shown = media_access_flags(cx);
    Some(cx.observe(&store, move |_, _, cx| {
        let flags = media_access_flags(cx);
        if flags != shown {
            shown = flags;
            cx.notify();
        }
    }))
}

pub fn media_access_needed_label(device: MediaDevice, locale: &str) -> &'static str {
    mezon_i18n::t(
        locale,
        match device {
            MediaDevice::Microphone => "channelVoice.mediaPermission.needed.microphone",
            MediaDevice::Camera => "channelVoice.mediaPermission.needed.camera",
        },
    )
}

pub fn media_permission_badge(theme: &Theme, size: Pixels, ring: impl Into<Hsla>) -> gpui::Div {
    div()
        .absolute()
        .flex()
        .items_center()
        .justify_center()
        .size(size)
        .rounded_full()
        .border_2()
        .border_color(ring.into())
        .bg(theme.status_idle)
        .text_color(rgb(BADGE_TEXT))
        .text_size(size * 0.6)
        .font_weight(FontWeight::EXTRA_BOLD)
        .line_height(size)
        .child("!")
}

pub struct MediaPermissionOverlay {
    store: Option<Entity<MediaPermissionStore>>,
    _observe: Option<Subscription>,
    _window_activation: Option<Subscription>,
}

impl MediaPermissionOverlay {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = MediaPermissionStore::try_global(cx);
        let observe = store
            .as_ref()
            .map(|store| cx.observe(store, |_, _, cx| cx.notify()));
        Self {
            store,
            _observe: observe,
            _window_activation: None,
        }
    }

    fn observe_window_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._window_activation.is_some() || self.store.is_none() {
            return;
        }
        self._window_activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                return;
            }
            if let Some(store) = &this.store {
                store.update(cx, |store, cx| store.refresh(cx));
            }
        }));
    }
}

impl Render for MediaPermissionOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.observe_window_activation(window, cx);
        let Some(store) = self.store.clone() else {
            return div().into_any_element();
        };
        let Some(prompt) = store.read(cx).prompt() else {
            return div().into_any_element();
        };
        let locale = Settings::try_global(cx)
            .map(|s| s.read(cx).language.clone())
            .unwrap_or_default();
        let card = match prompt {
            MediaPermissionPrompt::Request { device, requesting } => {
                request_card(&store, device, requesting, &locale, cx)
            }
            MediaPermissionPrompt::Blocked(device) => blocked_card(&store, device, &locale, cx),
        };
        deferred(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x000000b3))
                .occlude()
                .child(card),
        )
        .into_any_element()
    }
}

fn close_button(store: &Entity<MediaPermissionStore>, theme: &Theme) -> AnyElement {
    let store = store.clone();
    let hover = theme.bg_hover;
    div()
        .id("media-permission-close")
        .absolute()
        .top(px(12.))
        .right(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded_full()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(
            Icon::new(IconName::Close)
                .size(px(16.))
                .text_color(theme.tokens.text_secondary),
        )
        .on_click(move |_, _, cx| store.update(cx, |store, cx| store.dismiss(cx)))
        .into_any_element()
}

fn card_shell(theme: &Theme, width: Pixels) -> gpui::Div {
    div()
        .relative()
        .w(width)
        .max_w_full()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg_floating)
        .shadow_lg()
}

fn device_icon(device: MediaDevice) -> IconName {
    match device {
        MediaDevice::Microphone => IconName::VoiceMicIcon,
        MediaDevice::Camera => IconName::VoiceCameraIcon,
    }
}

fn request_card(
    store: &Entity<MediaPermissionStore>,
    device: MediaDevice,
    requesting: bool,
    locale: &str,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (title_key, body_key) = match device {
        MediaDevice::Microphone => (
            "channelVoice.mediaPermission.requestTitle.microphone",
            "channelVoice.mediaPermission.requestBody.microphone",
        ),
        MediaDevice::Camera => (
            "channelVoice.mediaPermission.requestTitle.camera",
            "channelVoice.mediaPermission.requestBody.camera",
        ),
    };
    let dismiss_store = store.clone();
    let allow_store = store.clone();
    card_shell(theme, px(400.))
        .child(
            v_flex()
                .items_center()
                .gap_4()
                .px(px(28.))
                .pt(px(32.))
                .pb(px(24.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(64.))
                        .rounded_full()
                        .bg(theme.brand)
                        .child(
                            Icon::new(device_icon(device))
                                .size(px(28.))
                                .text_color(rgb(0xffffff)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_center()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(mezon_i18n::t(locale, title_key)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(theme.tokens.text_secondary)
                        .child(mezon_i18n::t(locale, body_key)),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .pt_2()
                        .child(
                            div().flex_1().child(
                                Button::new("media-permission-later")
                                    .label(mezon_i18n::t(locale, "channelVoice.later"))
                                    .ghost()
                                    .w_full()
                                    .on_click(move |_, _, cx| {
                                        dismiss_store.update(cx, |store, cx| store.dismiss(cx))
                                    }),
                            ),
                        )
                        .child(
                            div().flex_1().child(
                                Button::new("media-permission-allow")
                                    .label(mezon_i18n::t(
                                        locale,
                                        "channelVoice.mediaPermission.allow",
                                    ))
                                    .primary()
                                    .w_full()
                                    .disabled(requesting)
                                    .loading(requesting)
                                    .on_click(move |_, _, cx| {
                                        allow_store.update(cx, |store, cx| store.request_access(cx))
                                    }),
                            ),
                        ),
                ),
        )
        .child(close_button(store, theme))
        .into_any_element()
}

fn blocked_steps(device: MediaDevice, locale: &str) -> [&'static str; 2] {
    let (open_key, enable_key) = if cfg!(target_os = "windows") {
        match device {
            MediaDevice::Microphone => (
                "channelVoice.mediaPermission.windowsStepOpen.microphone",
                "channelVoice.mediaPermission.windowsStepEnable.microphone",
            ),
            MediaDevice::Camera => (
                "channelVoice.mediaPermission.windowsStepOpen.camera",
                "channelVoice.mediaPermission.windowsStepEnable.camera",
            ),
        }
    } else {
        match device {
            MediaDevice::Microphone => (
                "channelVoice.mediaPermission.macStepOpen.microphone",
                "channelVoice.mediaPermission.macStepEnable",
            ),
            MediaDevice::Camera => (
                "channelVoice.mediaPermission.macStepOpen.camera",
                "channelVoice.mediaPermission.macStepEnable",
            ),
        }
    };
    [
        mezon_i18n::t(locale, open_key),
        mezon_i18n::t(locale, enable_key),
    ]
}

fn blocked_card(
    store: &Entity<MediaPermissionStore>,
    device: MediaDevice,
    locale: &str,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let title_key = match device {
        MediaDevice::Microphone => "channelVoice.mediaPermission.blockedTitle.microphone",
        MediaDevice::Camera => "channelVoice.mediaPermission.blockedTitle.camera",
    };
    let steps = blocked_steps(device, locale)
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            h_flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .w(px(16.))
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(SharedString::from(format!("{}.", index + 1))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(text),
                )
        });
    let open_store = store.clone();
    card_shell(theme, px(600.))
        .child(
            h_flex()
                .items_stretch()
                .gap_6()
                .p(px(24.))
                .child(settings_illustration(theme, device, locale))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .justify_center()
                        .gap_4()
                        .pr(px(20.))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.tokens.text_theme_primary)
                                .child(mezon_i18n::t(locale, title_key)),
                        )
                        .child(v_flex().gap_2().children(steps))
                        .child(
                            h_flex().pt_1().child(
                                Button::new("media-permission-open-settings")
                                    .label(mezon_i18n::t(locale, "channelVoice.openSettings"))
                                    .primary()
                                    .on_click(move |_, _, cx| {
                                        if let Some(url) = open_store.read(cx).settings_url() {
                                            cx.open_url(url);
                                        }
                                    }),
                            ),
                        )
                        .when(cfg!(target_os = "macos"), |column| {
                            column.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.tokens.text_secondary)
                                    .child(mezon_i18n::t(
                                        locale,
                                        "screenShare.permissionRestartHint",
                                    )),
                            )
                        }),
                ),
        )
        .child(close_button(store, theme))
        .into_any_element()
}

fn settings_illustration(theme: &Theme, device: MediaDevice, locale: &str) -> AnyElement {
    let dot = |color: u32| {
        div()
            .size(px(7.))
            .rounded_full()
            .bg(if cfg!(target_os = "macos") {
                rgb(color).into()
            } else {
                Hsla::from(theme.tokens.text_secondary).opacity(0.4)
            })
    };
    let device_label = mezon_i18n::t(
        locale,
        match device {
            MediaDevice::Microphone => "channelVoice.mediaPermission.microphone",
            MediaDevice::Camera => "channelVoice.mediaPermission.camera",
        },
    );
    let toggle = div()
        .flex()
        .items_center()
        .justify_end()
        .w(px(28.))
        .h(px(16.))
        .p(px(2.))
        .rounded_full()
        .bg(theme.status_online)
        .child(div().size(px(12.)).rounded_full().bg(rgb(0xffffff)));
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(200.))
        .min_h(px(168.))
        .p_4()
        .rounded_lg()
        .bg(theme.bg_tertiary)
        .child(
            v_flex()
                .w_full()
                .overflow_hidden()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg_floating)
                .shadow_md()
                .child(
                    h_flex()
                        .gap(px(4.))
                        .px_2()
                        .py(px(6.))
                        .bg(theme.bg_secondary)
                        .child(dot(0xff5f57))
                        .child(dot(0xfebc2e))
                        .child(dot(0x28c840)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .py_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            Icon::new(device_icon(device))
                                .size(px(14.))
                                .text_color(theme.tokens.text_secondary),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.tokens.text_theme_primary)
                                .child(device_label),
                        ),
                )
                .child(
                    h_flex()
                        .justify_between()
                        .m_1()
                        .px_2()
                        .py(px(6.))
                        .rounded_md()
                        .border_1()
                        .border_color(theme.brand)
                        .child(
                            h_flex().gap_2().child(img(APP_ICON).size(px(16.))).child(
                                div()
                                    .text_xs()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child("Mezon"),
                            ),
                        )
                        .child(toggle),
                ),
        )
        .into_any_element()
}
