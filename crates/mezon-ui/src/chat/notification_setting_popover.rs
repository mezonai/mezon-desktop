use gpui::{
    Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Styled, Subscription, Window, div, prelude::*, px, relative,
};
use mezon_store::notification_setting::{
    MUTE_FOR_1_HOUR_SEC, MUTE_FOR_3_HOURS_SEC, MUTE_FOR_8_HOURS_SEC, MUTE_FOR_15_MINUTES_SEC,
    MUTE_FOR_24_HOURS_SEC, MUTE_FOREVER, NOTIFICATION_ALL_MESSAGE, NOTIFICATION_DEFAULT,
    NOTIFICATION_MENTION_MESSAGE, NOTIFICATION_NOTHING_MESSAGE,
};
use mezon_store::{ChannelId, ClanId, NotificationSettingStore, Settings};

use crate::components::primitives::{Icon, IconName, Label, h_flex, v_flex};
use crate::theme::ActiveTheme;

const MUTE_OPTIONS: &[(i32, &str)] = &[
    (
        MUTE_FOR_15_MINUTES_SEC,
        "channelTopbar.notificationSetting.muteOptions.for15Minutes",
    ),
    (
        MUTE_FOR_1_HOUR_SEC,
        "channelTopbar.notificationSetting.muteOptions.for1Hour",
    ),
    (
        MUTE_FOR_3_HOURS_SEC,
        "channelTopbar.notificationSetting.muteOptions.for3Hours",
    ),
    (
        MUTE_FOR_8_HOURS_SEC,
        "channelTopbar.notificationSetting.muteOptions.for8Hours",
    ),
    (
        MUTE_FOR_24_HOURS_SEC,
        "channelTopbar.notificationSetting.muteOptions.for24Hours",
    ),
    (
        MUTE_FOREVER,
        "channelTopbar.notificationSetting.muteOptions.untilTurnedBackOn",
    ),
];

const LEVEL_OPTIONS: &[(i32, &str)] = &[
    (
        NOTIFICATION_DEFAULT,
        "notificationSetting.bottomSheet.labelOptions.categoryDefault",
    ),
    (
        NOTIFICATION_ALL_MESSAGE,
        "notificationSetting.bottomSheet.labelOptions.allMessage",
    ),
    (
        NOTIFICATION_MENTION_MESSAGE,
        "notificationSetting.bottomSheet.labelOptions.mentionMessage",
    ),
    (
        NOTIFICATION_NOTHING_MESSAGE,
        "notificationSetting.bottomSheet.labelOptions.notThingMessage",
    ),
];

pub struct NotificationSettingPanel {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
    mute_submenu_open: bool,
    _blur_sub: Subscription,
}

impl EventEmitter<DismissEvent> for NotificationSettingPanel {}

impl Focusable for NotificationSettingPanel {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl NotificationSettingPanel {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = NotificationSettingStore::global(cx);
        store.update(cx, |store, cx| {
            store.ensure_channel(clan_id, channel_id, cx)
        });
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        let focus_handle = cx.focus_handle();
        let blur_sub = cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(DismissEvent));
        Self {
            clan_id,
            channel_id,
            settings,
            focus_handle,
            mute_submenu_open: false,
            _blur_sub: blur_sub,
        }
    }
}

impl Render for NotificationSettingPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tokens = &theme.tokens;
        let locale = self.settings.read(cx).language.clone();
        let store = NotificationSettingStore::global(cx);
        let (muted, level, muted_until) = {
            let store = store.read(cx);
            (
                store.is_time_muted(self.channel_id),
                store.level(self.channel_id),
                store.muted_until_ms(self.channel_id),
            )
        };
        let t = |key: &'static str| mezon_i18n::t(&locale, key);

        let channel_id = self.channel_id;
        let clan_id = self.clan_id;

        let mute_header = if muted {
            t("channelTopbar.notificationSetting.unmuteChannel")
        } else {
            t("channelTopbar.notificationSetting.muteChannel")
        };

        let mut mute_row = v_flex().w_full().child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(Label::new(mute_header).text_color(tokens.text_theme_primary))
                .when(!muted, |row| {
                    row.child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(14.))
                            .text_color(tokens.text_secondary),
                    )
                }),
        );
        if let Some(until_ms) = muted_until {
            mute_row = mute_row.child(
                div().ml(px(8.)).mb(px(4.)).child(
                    Label::new(
                        t("channelTopbar.notificationSetting.mutedUntil")
                            .replace("{{date}}", &format_muted_until(until_ms)),
                    )
                    .text_xs()
                    .text_color(tokens.text_theme_primary),
                ),
            );
        }

        let submenu_open = self.mute_submenu_open && !muted;
        let mut mute_entry = div()
            .id("noti-mute-toggle")
            .relative()
            .px(px(8.))
            .py(px(6.))
            .rounded(px(2.))
            .cursor_pointer()
            .hover(|s| s.bg(tokens.bg_item_hover))
            .child(mute_row)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.mute_submenu_open = true;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |_, _, _, cx| {
                NotificationSettingStore::global(cx).update(cx, |store, cx| {
                    if store.is_time_muted(channel_id) {
                        store.unmute(channel_id, clan_id, cx);
                    } else {
                        store.mute_forever(channel_id, clan_id, cx);
                    }
                });
            }));

        if submenu_open {
            let mut submenu = v_flex()
                .id("noti-mute-submenu")
                .absolute()
                .top(px(-6.))
                .left(relative(1.))
                .ml(px(4.))
                .w(px(200.))
                .p(px(6.))
                .rounded_md()
                .border_1()
                .border_color(tokens.border_primary)
                .bg(tokens.theme_setting_primary)
                .shadow_lg()
                .occlude();
            for (seconds, key) in MUTE_OPTIONS {
                let seconds = *seconds;
                submenu = submenu.child(
                    div()
                        .id(*key)
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(2.))
                        .cursor_pointer()
                        .hover(|s| s.bg(tokens.bg_item_hover))
                        .child(
                            Label::new(t(key))
                                .text_sm()
                                .text_color(tokens.text_theme_primary),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            NotificationSettingStore::global(cx).update(cx, |store, cx| {
                                store.set_mute(channel_id, clan_id, seconds, cx);
                            });
                        })),
                );
            }
            mute_entry = mute_entry.child(submenu);
        }

        let mut root = v_flex()
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .w(px(202.))
            .py(px(6.))
            .px(px(8.))
            .rounded_md()
            .border_1()
            .border_color(tokens.border_primary)
            .bg(tokens.theme_setting_primary)
            .text_color(tokens.text_theme_message)
            .child(
                div()
                    .w_full()
                    .pb(px(4.))
                    .mb(px(4.))
                    .border_b_1()
                    .border_color(tokens.border_primary)
                    .child(mute_entry),
            );

        for (option_level, key) in LEVEL_OPTIONS {
            let option_level = *option_level;
            let selected = level == option_level;
            root = root.child(
                h_flex()
                    .id(*key)
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(tokens.bg_item_hover))
                    .child(crate::chat::notification_setting_modal::radio_dot(
                        selected,
                        tokens.border_primary.into(),
                        theme.brand.into(),
                    ))
                    .child(
                        Label::new(t(key))
                            .text_sm()
                            .text_color(tokens.text_theme_primary),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        NotificationSettingStore::global(cx).update(cx, |store, cx| {
                            store.set_level(channel_id, clan_id, option_level, cx);
                        });
                    })),
            );
        }

        root
    }
}

pub fn format_muted_until(epoch_ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(epoch_ms).single() {
        Some(dt) => dt.format("%d/%m, %H:%M").to_string(),
        None => String::new(),
    }
}
