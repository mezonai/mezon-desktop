use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Window, deferred, div, prelude::*, px,
};
use mezon_store::notification_setting::{
    NOTIFICATION_ALL_MESSAGE, NOTIFICATION_MENTION_MESSAGE, NOTIFICATION_NOTHING_MESSAGE,
};
use mezon_store::{ChannelId, ChannelList, ClanId, NotificationSettingStore, Settings};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Icon, IconName, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::theme::ActiveTheme;

const LEVELS: [(i32, &str); 3] = [
    (
        NOTIFICATION_ALL_MESSAGE,
        "channelMenu.menu.notification.all",
    ),
    (
        NOTIFICATION_MENTION_MESSAGE,
        "channelMenu.menu.notification.onlyMention",
    ),
    (
        NOTIFICATION_NOTHING_MESSAGE,
        "channelMenu.menu.notification.nothing",
    ),
];

#[derive(Clone)]
struct AddCandidate {
    id: String,
    label: String,
    is_category: bool,
}

pub struct NotificationSettingModal {
    clan_id: ClanId,
    clan_name: String,
    settings: Entity<Settings>,
    channel_list: Entity<ChannelList>,
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    add_open: bool,
    _input_sub: Subscription,
    _store_sub: Subscription,
}

impl Focusable for NotificationSettingModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl NotificationSettingModal {
    pub fn new(
        clan_id: ClanId,
        clan_name: String,
        settings: Entity<Settings>,
        channel_list: Entity<ChannelList>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = NotificationSettingStore::global(cx);
        store.update(cx, |store, cx| {
            store.fetch_overrides(clan_id, cx);
            store.prefetch_clan(clan_id, cx);
        });
        let store_sub = cx.observe(&store, |_, _, cx| cx.notify());

        let locale = settings.read(cx).language.clone();
        let placeholder = mezon_i18n::t(&locale, "notificationSetting.selectChannelOrCategory");
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let input_sub = cx.subscribe(
            &search_input,
            |_this: &mut Self, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        );

        Self {
            clan_id,
            clan_name,
            settings,
            channel_list,
            focus_handle: cx.focus_handle(),
            search_input,
            add_open: false,
            _input_sub: input_sub,
            _store_sub: store_sub,
        }
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn add_candidates(&self, cx: &App, already: &[i64], query: &str) -> Vec<AddCandidate> {
        let channels = self.channel_list.read(cx);
        let mut out = Vec::new();
        for category in channels.categories_for_clan(self.clan_id) {
            let cat_raw = category.id.parse::<i64>().unwrap_or(0);
            if cat_raw != 0 && !already.contains(&cat_raw) {
                out.push(AddCandidate {
                    id: category.id.clone(),
                    label: category.name.clone(),
                    is_category: true,
                });
            }
            for channel in &category.channels {
                if !already.contains(&channel.id.get()) {
                    out.push(AddCandidate {
                        id: channel.id.get().to_string(),
                        label: channel.name.clone(),
                        is_category: false,
                    });
                }
            }
        }
        if !query.is_empty() {
            out.retain(|c| c.label.to_lowercase().contains(query));
        }
        out.truncate(50);
        out
    }
}

pub(crate) fn radio_dot(selected: bool, border: Hsla, fill: Hsla) -> impl IntoElement {
    div()
        .w(px(18.))
        .h(px(18.))
        .rounded_full()
        .border_2()
        .border_color(if selected { fill } else { border })
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |d| {
            d.child(div().w(px(9.)).h(px(9.)).rounded_full().bg(fill))
        })
}

pub(crate) fn checkbox(checked: bool, border: Hsla, fill: Hsla) -> impl IntoElement {
    div()
        .w(px(18.))
        .h(px(18.))
        .rounded(px(4.))
        .border_2()
        .border_color(if checked { fill } else { border })
        .when(checked, |d| d.bg(fill))
        .flex()
        .items_center()
        .justify_center()
        .when(checked, |d| {
            d.child(
                Icon::new(IconName::Check)
                    .size(px(12.))
                    .text_color(gpui::white()),
            )
        })
}

impl Render for NotificationSettingModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tokens = &theme.tokens;
        let locale = self.settings.read(cx).language.clone();
        let t = |key: &'static str| mezon_i18n::t(&locale, key);

        let border: Hsla = tokens.border_primary.into();
        let fill: Hsla = theme.brand.into();
        let text_primary: Hsla = tokens.text_theme_primary.into();
        let text_secondary: Hsla = tokens.text_secondary.into();
        let hover = tokens.bg_item_hover;

        let store = NotificationSettingStore::global(cx);
        let clan_default = store.read(cx).clan_default(self.clan_id);
        let mut overrides = store.read(cx).overrides(self.clan_id);
        overrides.sort_by_key(|o| o.label.to_lowercase());

        let clan_id = self.clan_id;

        let mut clan_section = v_flex().gap(px(4.));
        for (value, key) in LEVELS {
            let selected = clan_default == Some(value);
            clan_section = clan_section.child(
                h_flex()
                    .id(("clan-level", value as usize))
                    .items_center()
                    .gap(px(12.))
                    .p(px(12.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(hover))
                    .child(radio_dot(selected, border, fill))
                    .child(
                        div()
                            .text_sm()
                            .text_color(text_primary)
                            .child(t(key).to_string()),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        NotificationSettingStore::global(cx)
                            .update(cx, |store, cx| store.set_clan_level(clan_id, value, cx));
                    })),
            );
        }

        let header_cell = |label: SharedString| {
            div()
                .w(px(80.))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(text_primary)
                .child(label)
        };

        let table_header = h_flex()
            .w_full()
            .items_center()
            .px(px(10.))
            .pb(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(text_primary)
                    .child(t("notificationSetting.headers.channelOrCategory").to_string()),
            )
            .child(header_cell(t("notificationSetting.headers.all").into()))
            .child(header_cell(
                t("notificationSetting.headers.mentions").into(),
            ))
            .child(header_cell(t("notificationSetting.headers.nothing").into()))
            .child(header_cell(t("notificationSetting.headers.mute").into()));

        let mut table = v_flex().w_full().gap(px(4.));
        for o in &overrides {
            let is_category = o.is_category;
            let id_str = o.id.to_string();
            let channel_id = ChannelId(o.id);
            let (level, muted) = {
                let s = store.read(cx);
                if is_category {
                    (
                        s.category_default(&id_str).unwrap_or(0),
                        s.category_is_time_muted(&id_str),
                    )
                } else {
                    (s.level(channel_id), s.is_time_muted(channel_id))
                }
            };
            let group_name = SharedString::from(format!("noti-ovr-{}", o.id));

            let mut row = h_flex()
                .id(("noti-ovr", o.id as usize))
                .group(group_name.clone())
                .w_full()
                .items_center()
                .p(px(10.))
                .rounded(px(4.))
                .hover(|s| s.bg(hover))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(text_primary)
                        .child(o.label.clone()),
                );

            for (value, _key) in LEVELS {
                let selected = level == value;
                let id_for = id_str.clone();
                row = row.child(
                    div()
                        .w(px(80.))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id(("ovr-level", (o.id as usize) * 8 + value as usize))
                                .cursor_pointer()
                                .child(radio_dot(selected, border, fill))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    let id_for = id_for.clone();
                                    NotificationSettingStore::global(cx).update(cx, |store, cx| {
                                        if is_category {
                                            store.set_category_level(id_for, clan_id, value, cx);
                                        } else {
                                            store.set_level(channel_id, clan_id, value, cx);
                                        }
                                    });
                                })),
                        ),
                );
            }

            let id_for_mute = id_str.clone();
            let id_for_del = id_str.clone();
            row = row.child(
                div()
                    .w(px(80.))
                    .flex_shrink_0()
                    .relative()
                    .flex()
                    .justify_center()
                    .items_center()
                    .child(
                        div()
                            .id(("ovr-mute", o.id as usize))
                            .cursor_pointer()
                            .child(checkbox(muted, border, fill))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let id_for_mute = id_for_mute.clone();
                                NotificationSettingStore::global(cx).update(cx, |store, cx| {
                                    if is_category {
                                        if store.category_is_time_muted(&id_for_mute) {
                                            store.unmute_category(id_for_mute, clan_id, cx);
                                        } else {
                                            store.mute_category_forever(id_for_mute, clan_id, cx);
                                        }
                                    } else if store.is_time_muted(channel_id) {
                                        store.unmute(channel_id, clan_id, cx);
                                    } else {
                                        store.mute_forever(channel_id, clan_id, cx);
                                    }
                                });
                            })),
                    )
                    .child(
                        div()
                            .id(("ovr-del", o.id as usize))
                            .absolute()
                            .right(px(0.))
                            .invisible()
                            .group_hover(group_name.clone(), |s| s.visible())
                            .w(px(20.))
                            .h(px(20.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(gpui::rgba(0xef_44_44_26))
                            .cursor_pointer()
                            .hover(|s| s.bg(gpui::rgba(0xef_44_44_40)))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(10.))
                                    .text_color(gpui::rgb(0xef_44_44)),
                            )
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let id_for_del = id_for_del.clone();
                                NotificationSettingStore::global(cx).update(cx, |store, cx| {
                                    if is_category {
                                        store.delete_category_override(id_for_del, clan_id, cx);
                                    } else {
                                        store.delete_channel_override(channel_id, clan_id, cx);
                                    }
                                });
                            })),
                    ),
            );

            table = table.child(row);
        }

        let already: Vec<i64> = overrides.iter().map(|o| o.id).collect();
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let add_open = self.add_open;

        let mut add_field = div().id("noti-add-field").relative().w_full().child(
            div()
                .id("noti-add-input")
                .w_full()
                .child(Input::new(&self.search_input).text_color(text_primary))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.add_open = true;
                    cx.notify();
                })),
        );

        if add_open {
            let candidates = self.add_candidates(cx, &already, &query);
            let mut list = v_flex()
                .id("noti-add-list")
                .max_h(px(240.))
                .overflow_y_scroll()
                .min_h_0()
                .w_full()
                .p(px(4.))
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(tokens.theme_setting_primary)
                .shadow_lg();
            for c in candidates {
                let is_category = c.is_category;
                let id = c.id.clone();
                let label = c.label.clone();
                let prefix = if is_category { "" } else { "# " };
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!("noti-add-{}", c.id)))
                        .w_full()
                        .items_center()
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(hover))
                        .child(
                            div()
                                .text_sm()
                                .text_color(text_primary)
                                .child(format!("{prefix}{label}")),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = id.clone();
                            let label = label.clone();
                            NotificationSettingStore::global(cx).update(cx, |store, cx| {
                                if is_category {
                                    store.add_category_override(id, clan_id, label, cx);
                                } else {
                                    store.add_channel_override(
                                        ChannelId(id.parse().unwrap_or_default()),
                                        clan_id,
                                        label,
                                        cx,
                                    );
                                }
                            });
                            this.add_open = false;
                            cx.notify();
                        })),
                );
            }
            add_field = add_field.child(deferred(
                div()
                    .absolute()
                    .bottom_full()
                    .left_0()
                    .w_full()
                    .mb_1()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this.add_open {
                            this.add_open = false;
                            cx.notify();
                        }
                    }))
                    .child(list),
            ));
        }

        let body = div()
            .id("noti-modal-body")
            .flex()
            .flex_col()
            .px(px(20.))
            .py(px(16.))
            .max_h(px(500.))
            .overflow_y_scroll()
            .min_h_0()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(text_primary)
                    .mb(px(8.))
                    .child(t("notificationSetting.clanNotificationSettings").to_string()),
            )
            .child(clan_section)
            .child(div().my(px(16.)).h(px(1.)).w_full().bg(border))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(text_primary)
                    .mb(px(8.))
                    .child(t("notificationSetting.notificationOverrides").to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(text_secondary)
                    .mb(px(8.))
                    .child(t("notificationSetting.addChannelOverride").to_string()),
            )
            .child(add_field.mb(px(20.)))
            .child(table_header)
            .child(table);

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .occlude()
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .w(px(620.))
            .max_w(px(620.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .bg(tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .p(px(16.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        v_flex()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_primary)
                                    .child(t("notificationSetting.title").to_string()),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .text_color(text_secondary)
                                    .child(self.clan_name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("noti-modal-close")
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|s| s.bg(hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(16.))
                                    .text_color(text_secondary),
                            )
                            .on_click(cx.listener(|_, _, _, cx| Self::close(cx))),
                    ),
            )
            .child(body)
    }
}
