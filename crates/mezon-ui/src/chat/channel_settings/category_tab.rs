use gpui::{
    App, Context, Entity, FontWeight, SharedString, Subscription, Task, Window, deferred, div,
    prelude::*, px,
};
use mezon_store::{ChannelId, ChannelList, ClanId, FAVOR_CATE_ID, Settings};

use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};

pub struct CategoryTab {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    dropdown_open: bool,
    moving: bool,
    _move_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl CategoryTab {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let channel_list = ChannelList::global(cx);
        let subs = vec![
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&channel_list, |_, _, cx| cx.notify()),
        ];
        Self {
            clan_id,
            channel_id,
            settings,
            dropdown_open: false,
            moving: false,
            _move_task: None,
            _subs: subs,
        }
    }

    fn channel_label(&self, cx: &App) -> SharedString {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| SharedString::from(channel.name.clone()))
            .unwrap_or_default()
    }

    fn current_category_name(&self, cx: &App) -> SharedString {
        let list = ChannelList::global(cx).read(cx);
        let Some(channel) = list.channel(self.clan_id, self.channel_id) else {
            return SharedString::default();
        };
        if !channel.category_name.is_empty() {
            return SharedString::from(channel.category_name.clone());
        }
        let Some(category_id) = channel.category_id.as_deref() else {
            return SharedString::default();
        };
        list.categories_for_clan(self.clan_id)
            .iter()
            .find(|category| category.id == category_id)
            .map(|category| SharedString::from(category.name.clone()))
            .unwrap_or_default()
    }

    fn other_categories(&self, cx: &App) -> Vec<(String, SharedString)> {
        let list = ChannelList::global(cx).read(cx);
        let current_id = list
            .channel(self.clan_id, self.channel_id)
            .and_then(|channel| channel.category_id.clone());
        list.categories_for_clan(self.clan_id)
            .iter()
            .filter(|category| {
                category.id != FAVOR_CATE_ID
                    && current_id.as_deref().is_none_or(|id| category.id != id)
            })
            .map(|category| {
                (
                    category.id.clone(),
                    SharedString::from(category.name.clone()),
                )
            })
            .collect()
    }

    fn move_to_category(&mut self, category_id: String, cx: &mut Context<Self>) {
        if self.moving {
            return;
        }
        self.dropdown_open = false;
        self.moving = true;
        cx.notify();

        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let task = ChannelList::global(cx).update(cx, |store, cx| {
            store.change_channel_category(clan_id, channel_id, category_id, cx)
        });
        self._move_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.moving = false;
                this._move_task = None;
                if result.is_ok() {
                    crate::router::navigate(
                        cx,
                        crate::router::Route::Channel {
                            clan_id,
                            channel_id,
                        },
                    );
                }
                cx.notify();
            });
        }));
    }

    fn render_category_dropdown(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let category_name = self.current_category_name(cx);
        let options = self.other_categories(cx);
        let open = self.dropdown_open && !self.moving;
        let has_options = !options.is_empty();

        let mut trigger = h_flex()
            .id("channel-category-trigger")
            .w_full()
            .h(px(48.))
            .items_center()
            .justify_between()
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(theme.tokens.border_theme_primary)
            .bg(theme.tokens.bg_input_secondary)
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.tokens.text_theme_message)
            .cursor_pointer()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .child(category_name.to_uppercase()),
            )
            .child(
                Icon::new(IconName::ArrowDownFill)
                    .size(px(16.))
                    .text_color(theme.tokens.text_theme_message),
            );

        if !self.moving && has_options {
            trigger = trigger.on_click(cx.listener(|this, _, _, cx| {
                this.dropdown_open = !this.dropdown_open;
                cx.notify();
            }));
        }

        div()
            .relative()
            .w_full()
            .child(trigger)
            .when(open && has_options, |this| {
                this.child(
                    deferred(
                        v_flex()
                            .id("channel-category-menu")
                            .absolute()
                            .top_full()
                            .left_0()
                            .right_0()
                            .mt(px(4.))
                            .p(px(4.))
                            .rounded_md()
                            .border_1()
                            .border_color(theme.tokens.border_theme_primary)
                            .bg(theme.tokens.theme_setting_primary)
                            .shadow_lg()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                if this.dropdown_open {
                                    this.dropdown_open = false;
                                    cx.notify();
                                }
                            }))
                            .children(options.into_iter().enumerate().map(
                                |(index, (id, name))| {
                                    let label = name.to_uppercase();
                                    div()
                                        .id(("channel-category-option", index))
                                        .w_full()
                                        .px_3()
                                        .py_2()
                                        .rounded(px(4.))
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.tokens.text_theme_primary)
                                        .truncate()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.tokens.bg_item_hover))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.move_to_category(id.clone(), cx);
                                        }))
                                        .child(label)
                                },
                            )),
                    )
                    .with_priority(1),
                )
            })
    }
}

impl Render for CategoryTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();

        v_flex()
            .w_full()
            .gap_4()
            .text_sm()
            .text_color(theme.tokens.text_theme_primary)
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(
                        &locale,
                        "channelSetting.categoryManagement.title",
                    )),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(
                                &locale,
                                "channelSetting.categoryManagement.channelName",
                            )),
                    )
                    .child(
                        div()
                            .w_full()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.tokens.border_theme_primary)
                            .bg(theme.tokens.bg_input_secondary)
                            .pl_3()
                            .py_2()
                            .text_sm()
                            .text_color(theme.tokens.text_theme_message)
                            .child(self.channel_label(cx)),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .mt_4()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(
                                &locale,
                                "channelSetting.categoryManagement.category",
                            )),
                    )
                    .child(self.render_category_dropdown(&theme, cx)),
            )
    }
}
