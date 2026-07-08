use std::rc::Rc;

use gpui::{
    App, ClipboardItem, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, ListAlignment, ListState, MouseButton, MouseDownEvent, Render,
    SharedString, Subscription, Window, div, list, prelude::*, px, rgb, svg,
};
use mezon_store::{
    ChannelId, ChannelList, ClanId, ClanList, ClanMembersEvent, ClanMembersStore, InboxCategory,
    InboxEvent, InboxNotification, InboxStore, MessageId, MessagesStore, TopicBadgeEvent,
    TopicBadgeStore, TopicDiscussion, TopicsEvent, TopicsStore, UsersByUserEvent, UsersByUserStore,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::components::primitives::{h_flex, v_flex};
use crate::image_cache::{
    LruImageCache, MESSAGE_ENTRY_MAX_BYTES, MESSAGE_IMAGE_CACHE_BYTES, MESSAGE_IMAGE_CACHE_CAPACITY,
};

use crate::chat::inbox::row::{
    NotificationRowView, TopicRowView, build_notification_row_view, build_topic_row_view,
    notification_copy_text, render_notification_body, render_topic_body,
};
use crate::chat::inbox::{InboxTab, MESSAGE_ROW_HEIGHT};
use crate::components::primitives::{Icon, IconName};
use crate::router::{Route, navigate};
use crate::theme::{ActiveTheme, Theme};

const PANEL_WIDTH: f32 = 480.;
const LIST_BODY_HEIGHT: f32 = 520.;
const LIST_OVERDRAW: f32 = MESSAGE_ROW_HEIGHT + 40.;
const PREFETCH_THRESHOLD: usize = 5;
const EMPTY_PROTIP_COLOR: u32 = 0x2dc770;

pub struct InboxPopoverPanel {
    tab: InboxTab,
    clan_id: String,
    locale: SharedString,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    list_state: ListState,
    focus_handle: FocusHandle,
    cached_items: Rc<Vec<ListRow>>,
    avatar_image_cache: Entity<LruImageCache>,
    message_image_cache: Entity<LruImageCache>,
    _inbox_sub: Subscription,
    _topics_sub: Subscription,
    _members_sub: Subscription,
    _channel_obs: Subscription,
    _topic_badge_sub: Subscription,
    _users_sub: Subscription,
}

impl InboxPopoverPanel {
    pub fn new(
        clan_id: String,
        locale: String,
        inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list_state = ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW)).measure_all();
        let weak = cx.weak_entity();
        Self::attach_list_scroll_handler(&list_state, weak);
        let avatar_image_cache = crate::image_cache::shared_avatar_cache(cx);
        let message_image_cache = cx.new(|cx| {
            LruImageCache::message(
                "inbox-image",
                MESSAGE_IMAGE_CACHE_CAPACITY,
                MESSAGE_IMAGE_CACHE_BYTES,
                MESSAGE_ENTRY_MAX_BYTES,
                cx,
            )
        });

        let focus_handle = cx.focus_handle();
        cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(DismissEvent))
            .detach();

        let inbox_store = InboxStore::global(cx);
        let topics_store = TopicsStore::global(cx);
        let members_store = ClanMembersStore::global(cx);
        let channel_list = ChannelList::global(cx);
        let topic_badge_store = TopicBadgeStore::global(cx);
        let users_store = UsersByUserStore::global(cx);
        let panel_clan_id = clan_id.clone();

        UsersByUserStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(cx);
        });

        if let Ok(clan) = clan_id.parse::<ClanId>() {
            ClanMembersStore::global(cx).update(cx, |store, cx| {
                store.ensure_loaded(clan, cx);
            });
            ChannelList::global(cx).update(cx, |store, cx| {
                store.load_for_clan(clan, cx);
            });
        }

        if let Some(category) = InboxTab::Mentions.category() {
            inbox_store.update(cx, |store, cx| {
                store.fetch_if_empty(&clan_id, category, cx);
            });
        }

        let _inbox_sub = cx.subscribe(&inbox_store, |this, _, event, cx| {
            let InboxEvent::Updated { clan_id } = event;
            if clan_id.as_deref().is_some_and(|id| id != this.clan_id) {
                return;
            }
            this.sync_from_store(cx);
            cx.notify();
        });
        let _topics_sub = cx.subscribe(&topics_store, |this, _, event, cx| {
            if matches!(event, TopicsEvent::Updated) {
                this.sync_from_store(cx);
                cx.notify();
            }
        });
        let _members_sub = cx.subscribe(&members_store, |this, _, event, cx| {
            if let ClanMembersEvent::Changed { clan_id } = event
                && this.clan_id == clan_id.to_string()
            {
                cx.notify();
            }
        });
        let _channel_obs = cx.observe(&channel_list, move |_, _, cx| {
            if panel_clan_id.parse::<ClanId>().is_ok() {
                cx.notify();
            }
        });
        let _topic_badge_sub = cx.subscribe(&topic_badge_store, |this, _, event, cx| {
            let TopicBadgeEvent::Updated { clan_id } = event;
            if clan_id.as_deref().is_some_and(|id| id != this.clan_id) {
                return;
            }
            cx.notify();
        });
        let _users_sub = cx.subscribe(&users_store, |_, _, event, cx| {
            if matches!(event, UsersByUserEvent::Changed) {
                cx.notify();
            }
        });

        let mut this = Self {
            tab: InboxTab::Mentions,
            clan_id,
            locale: locale.into(),
            inbox_handle,
            list_state,
            focus_handle,
            cached_items: Rc::new(Vec::new()),
            avatar_image_cache,
            message_image_cache,
            _inbox_sub,
            _topics_sub,
            _members_sub,
            _channel_obs,
            _topic_badge_sub,
            _users_sub,
        };
        this.sync_from_store(cx);
        this
    }

    fn attach_list_scroll_handler(
        list_state: &ListState,
        weak: gpui::WeakEntity<InboxPopoverPanel>,
    ) {
        list_state.set_scroll_handler(move |event, _window, cx| {
            weak.update(cx, |panel, cx| {
                panel.maybe_load_more(event.visible_range.end, cx);
            })
            .ok();
        });
    }

    fn sync_list_state(&mut self, tab_changed: bool) {
        let count = self.cached_items.len();
        if self.list_state.item_count() != count {
            self.list_state.reset(count);
        } else if tab_changed && count > 0 {
            self.list_state.remeasure();
        }
        if tab_changed {
            self.list_state.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
        }
    }

    fn prefetch_context_for_tab(&self, cx: &mut Context<Self>) {
        if let Ok(clan) = self.clan_id.parse::<ClanId>() {
            ClanMembersStore::global(cx).update(cx, |store, cx| {
                store.ensure_loaded(clan, cx);
            });
            ChannelList::global(cx).update(cx, |store, cx| {
                store.load_for_clan(clan, cx);
            });
        }
        if self.tab == InboxTab::Topics {
            TopicsStore::global(cx).update(cx, |store, cx| {
                store.fetch_if_needed(&self.clan_id, cx);
            });
        } else if let Some(category) = self.tab.category() {
            InboxStore::global(cx).update(cx, |store, cx| {
                store.fetch_if_empty(&self.clan_id, category, cx);
            });
        }
    }

    fn sync_from_store(&mut self, cx: &App) {
        self.cached_items = Self::build_items(self.tab, &self.clan_id, &self.locale, cx);
        self.sync_list_state(false);
    }

    fn build_items(
        tab: InboxTab,
        clan_id: &str,
        locale: &SharedString,
        cx: &App,
    ) -> Rc<Vec<ListRow>> {
        if tab == InboxTab::Topics {
            let topics = TopicsStore::global(cx)
                .read(cx)
                .topics_for(clan_id)
                .to_vec();
            return Rc::new(
                topics
                    .into_iter()
                    .map(|topic| {
                        let view = build_topic_row_view(&topic, cx);
                        ListRow::Topic { topic, view }
                    })
                    .collect(),
            );
        }
        let Some(category) = tab.category() else {
            return Rc::new(Vec::new());
        };
        let items = InboxStore::global(cx)
            .read(cx)
            .items(clan_id, category)
            .to_vec();
        Rc::new(
            items
                .into_iter()
                .map(|notification| {
                    let view = build_notification_row_view(&notification, locale, cx);
                    ListRow::Notification {
                        notification: Box::new(notification),
                        view,
                    }
                })
                .collect(),
        )
    }

    fn select_tab(&mut self, tab: InboxTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.prefetch_context_for_tab(cx);
        self.sync_from_store(cx);
        self.sync_list_state(true);
        cx.notify();
    }

    fn delete_notification(&self, id: &str, category: InboxCategory, cx: &mut Context<Self>) {
        InboxStore::global(cx).update(cx, |store, cx| {
            store.delete(&self.clan_id, id, category, cx);
        });
    }

    fn copy_message(&self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    }

    fn maybe_load_more(&self, visible_end: usize, cx: &mut Context<Self>) {
        if self.tab == InboxTab::Topics {
            return;
        }
        let count = self.cached_items.len();
        if count == 0 || count.saturating_sub(visible_end) > PREFETCH_THRESHOLD {
            return;
        }
        if let Some(category) = self.tab.category() {
            InboxStore::global(cx).update(cx, |store, cx| {
                if store.has_more(&self.clan_id, category) {
                    store.fetch_more(&self.clan_id, category, cx);
                }
            });
        }
    }
}

#[derive(Clone)]
enum ListRow {
    Notification {
        notification: Box<InboxNotification>,
        view: NotificationRowView,
    },
    Topic {
        topic: TopicDiscussion,
        view: TopicRowView,
    },
}

impl EventEmitter<DismissEvent> for InboxPopoverPanel {}

impl Focusable for InboxPopoverPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InboxPopoverPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let theme_ref = theme.as_ref();
        let items = self.cached_items.clone();
        let locale = self.locale.clone();
        let inbox_handle = self.inbox_handle.clone();
        let active_tab = self.tab;
        let topic_badge = self.clan_id.parse::<ClanId>().ok().and_then(|_| {
            TopicBadgeStore::global(cx)
                .read(cx)
                .badge_label_for_clan(&self.clan_id)
        });
        let is_loading = if active_tab == InboxTab::Topics {
            TopicsStore::global(cx).read(cx).is_loading()
        } else if let Some(category) = active_tab.category() {
            InboxStore::global(cx)
                .read(cx)
                .is_loading(&self.clan_id, category)
        } else {
            false
        };

        let list_state = self.list_state.clone();
        let avatar_cache = self.avatar_image_cache.clone();
        let message_cache = self.message_image_cache.clone();
        let this = cx.weak_entity();

        let list_body: gpui::AnyElement = if items.is_empty() && is_loading {
            render_loading(theme_ref, &locale).into_any_element()
        } else if items.is_empty() {
            render_empty(theme_ref, &locale, active_tab)
        } else {
            let items_for_list = items;
            let locale_for_list = locale.clone();
            let inbox_handle_for_list = inbox_handle.clone();
            let panel_weak = this.clone();
            let theme_for_list = theme.clone();
            div()
                .size_full()
                .overflow_hidden()
                .child(
                    list(list_state, move |ix, _window, cx| {
                        let Some(row) = items_for_list.get(ix).cloned() else {
                            return div().into_any_element();
                        };
                        render_row(
                            theme_for_list.as_ref(),
                            &locale_for_list,
                            row,
                            active_tab,
                            avatar_cache.clone(),
                            message_cache.clone(),
                            panel_weak.clone(),
                            inbox_handle_for_list.clone(),
                            cx,
                        )
                    })
                    .size_full(),
                )
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&self.list_state),
                    window,
                    cx,
                )
                .into_any_element()
        };

        v_flex()
            .flex()
            .flex_col()
            .id("inbox-popover-panel")
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .w(px(PANEL_WIDTH))
            .max_h(px(LIST_BODY_HEIGHT + 120.))
            .bg(theme.bg_primary)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .overflow_hidden()
            .shadow_lg()
            .child(
                v_flex()
                    .px_3()
                    .py_2()
                    .child(render_header(theme_ref, &locale))
                    .child(render_tabs(
                        theme_ref,
                        &locale,
                        active_tab,
                        topic_badge,
                        this.clone(),
                    )),
            )
            .child(
                div()
                    .h(px(LIST_BODY_HEIGHT))
                    .w_full()
                    .overflow_hidden()
                    .child(list_body),
            )
    }
}

fn render_header(theme: &Theme, locale: &SharedString) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_2()
        .pb_2()
        .child(
            Icon::new(IconName::Inbox)
                .size(px(20.))
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .text_base()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(mezon_i18n::t(locale, "notifications.inbox")),
        )
}

fn render_tabs(
    theme: &Theme,
    locale: &SharedString,
    active: InboxTab,
    topic_badge: Option<String>,
    this: gpui::WeakEntity<InboxPopoverPanel>,
) -> impl IntoElement {
    h_flex()
        .gap_4()
        .py_3()
        .border_b_1()
        .border_color(theme.border)
        .children(InboxTab::all().into_iter().map(move |tab| {
            let is_active = tab == active;
            let label = mezon_i18n::t(locale, tab.label_key());
            let this = this.clone();
            div()
                .id(SharedString::from(format!("inbox-tab-{:?}", tab)))
                .relative()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .cursor_pointer()
                .when(is_active, |d| d.bg(theme.brand))
                .text_base()
                .font_weight(if is_active {
                    gpui::FontWeight::MEDIUM
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(if is_active {
                    gpui::rgb(0xffffff)
                } else {
                    theme.text_muted
                })
                .child(label)
                .when(tab == InboxTab::Topics && topic_badge.is_some(), |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-4.))
                            .right(px(-4.))
                            .px_1()
                            .min_w(px(16.))
                            .h(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme.mention_badge)
                            .text_xs()
                            .text_color(theme.bg_primary)
                            .child(topic_badge.clone().unwrap_or_default()),
                    )
                })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    this.update(cx, |panel, cx| panel.select_tab(tab, cx)).ok();
                })
        }))
}

fn render_loading(theme: &Theme, locale: &SharedString) -> impl IntoElement {
    let sk = theme.bg_hover;
    let row = || {
        div()
            .mx_3()
            .my_1()
            .p_3()
            .rounded(px(8.))
            .bg(theme.bg_secondary)
            .child(div().h(px(14.)).w(px(280.)).rounded(px(4.)).bg(sk))
            .child(div().mt_2().h(px(12.)).w(px(200.)).rounded(px(4.)).bg(sk))
    };
    v_flex()
        .size_full()
        .overflow_hidden()
        .child(row())
        .child(row())
        .child(row())
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme.text_muted)
                .child(mezon_i18n::t(locale, "channelTopbar.loading")),
        )
}

fn render_empty(theme: &Theme, locale: &SharedString, tab: InboxTab) -> gpui::AnyElement {
    match tab {
        InboxTab::Messages => render_empty_with_protip(
            theme,
            mezon_i18n::t(locale, "notifications.empty.messages.title").into(),
            mezon_i18n::t(locale, "notifications.empty.messages.protip")
                .to_uppercase()
                .into(),
            mezon_i18n::t(locale, "notifications.empty.messages.description").into(),
            IconName::EmptyUnread,
        )
        .into_any_element(),
        InboxTab::Mentions => render_empty_with_protip(
            theme,
            mezon_i18n::t(locale, "notifications.empty.mentions.title").into(),
            mezon_i18n::t(locale, "notifications.empty.mentions.protip")
                .to_uppercase()
                .into(),
            mezon_i18n::t(locale, "notifications.empty.mentions.description").into(),
            IconName::EmptyMention,
        )
        .into_any_element(),
        InboxTab::ForYou => render_empty_simple(
            theme,
            mezon_i18n::t(locale, "notifications.empty.forYou.title").into(),
            mezon_i18n::t(locale, "notifications.empty.forYou.description").into(),
            IconName::Inbox,
        )
        .into_any_element(),
        InboxTab::Topics => render_empty_simple(
            theme,
            mezon_i18n::t(locale, "notifications.empty.topics.title").into(),
            mezon_i18n::t(locale, "notifications.empty.topics.description").into(),
            IconName::EmptyMention,
        )
        .into_any_element(),
    }
}

fn render_empty_icon_cluster(theme: &Theme, icon: IconName) -> impl IntoElement {
    div()
        .relative()
        .mb_4()
        .p(px(22.))
        .rounded_full()
        .child(Icon::new(icon).size(px(36.)).text_color(theme.text_muted))
        .child(
            svg()
                .path(IconName::EmptyUnreadStyle.path())
                .absolute()
                .top_0()
                .left(px(-10.))
                .w(px(104.))
                .h(px(80.)),
        )
}

fn render_empty_with_protip(
    theme: &Theme,
    title: SharedString,
    protip: SharedString,
    description: SharedString,
    icon: IconName,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .py(px(80.))
        .px_4()
        .child(render_empty_icon_cluster(theme, icon))
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_center()
                .mb_2()
                .text_color(theme.text_primary)
                .child(title),
        )
        .child(
            div()
                .text_xs()
                .text_center()
                .max_w(px(360.))
                .text_color(theme.text_primary)
                .child(
                    h_flex()
                        .flex_wrap()
                        .justify_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(EMPTY_PROTIP_COLOR))
                                .child(protip),
                        )
                        .child(description),
                ),
        )
}

fn render_empty_simple(
    theme: &Theme,
    title: SharedString,
    description: SharedString,
    icon: IconName,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .child(Icon::new(icon).size(px(36.)).text_color(theme.text_muted))
        .child(
            div()
                .text_lg()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_center()
                .text_color(theme.text_muted)
                .max_w(px(360.))
                .child(description),
        )
}

fn render_row(
    theme: &Theme,
    locale: &SharedString,
    row: ListRow,
    tab: InboxTab,
    avatar_cache: Entity<LruImageCache>,
    message_cache: Entity<LruImageCache>,
    this: gpui::WeakEntity<InboxPopoverPanel>,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    cx: &App,
) -> gpui::AnyElement {
    match row {
        ListRow::Notification { notification, view } => render_notification_item(
            theme,
            locale,
            *notification,
            view,
            tab,
            avatar_cache,
            message_cache,
            this,
            inbox_handle,
            cx,
        ),
        ListRow::Topic { topic, view } => {
            render_topic_item(theme, locale, topic, view, avatar_cache, inbox_handle, cx)
        }
    }
}

fn schedule_inbox_jump(
    cx: &mut App,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    route: Route,
    _clan_id: String,
    channel_id: String,
    message_id: String,
) {
    let jump = channel_id
        .parse::<ChannelId>()
        .ok()
        .zip(message_id.parse::<MessageId>().ok());
    navigate(cx, route);
    if let Some((channel, target)) = jump {
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.request_jump(channel, target, cx);
        });
    }
    inbox_handle.hide(cx);
}

fn notification_jump_route(notification: &InboxNotification) -> Option<Route> {
    let message_id = notification
        .message
        .as_ref()
        .map(|m| m.message_id.as_str())
        .filter(|id| !id.is_empty())
        .unwrap_or("");
    if message_id.is_empty() {
        return None;
    }
    let channel_id = notification.effective_channel_id()?;
    let Ok(channel) = channel_id.parse::<ChannelId>() else {
        return None;
    };
    let clan_id = notification.effective_clan_id();
    if clan_id
        .as_deref()
        .is_none_or(|id| id.is_empty() || id == "0")
    {
        return Some(Route::DirectMessage {
            direct_id: channel,
            message_type: "3".into(),
        });
    }
    let Ok(clan) = clan_id.unwrap().parse::<ClanId>() else {
        return None;
    };
    Some(Route::Channel {
        clan_id: clan,
        channel_id: channel,
    })
}

fn schedule_notification_jump(
    cx: &mut App,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    notification: InboxNotification,
) {
    let Some(route) = notification_jump_route(&notification) else {
        return;
    };
    let message_id = notification
        .message
        .as_ref()
        .map(|m| m.message_id.clone())
        .filter(|id| !id.is_empty())
        .unwrap_or_default();
    schedule_inbox_jump(
        cx,
        inbox_handle,
        route,
        notification.effective_clan_id().unwrap_or_default(),
        notification.effective_channel_id().unwrap_or_default(),
        message_id,
    );
}

fn schedule_topic_jump(
    cx: &mut App,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    topic: TopicDiscussion,
) {
    if topic.message_id.is_empty() || topic.channel_id.is_empty() {
        return;
    }
    let Ok(clan) = topic.clan_id.parse::<ClanId>() else {
        return;
    };
    let Ok(channel) = topic.channel_id.parse::<ChannelId>() else {
        return;
    };
    schedule_inbox_jump(
        cx,
        inbox_handle,
        Route::Channel {
            clan_id: clan,
            channel_id: channel,
        },
        topic.clan_id,
        topic.channel_id,
        topic.message_id,
    );
}

fn render_notification_item(
    theme: &Theme,
    locale: &SharedString,
    notification: InboxNotification,
    view: NotificationRowView,
    tab: InboxTab,
    avatar_cache: Entity<LruImageCache>,
    message_cache: Entity<LruImageCache>,
    this: gpui::WeakEntity<InboxPopoverPanel>,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    cx: &App,
) -> gpui::AnyElement {
    let category = notification.category;
    let id: SharedString = notification.id.clone().into();
    let copy_text = notification_copy_text(&notification);
    let show_jump = tab == InboxTab::Mentions;
    let show_copy = tab == InboxTab::Messages && copy_text.is_some();
    let copy_text = copy_text.unwrap_or_default();
    let jump_notification = notification.clone();
    let this_delete = this.clone();
    let this_copy = this.clone();
    let inbox_handle_jump = inbox_handle.clone();
    let jump_label = mezon_i18n::t(locale, "channelTopbar.tooltips.jump");
    let outer_py = if tab == InboxTab::ForYou {
        px(4.)
    } else {
        px(8.)
    };

    div()
        .flex()
        .flex_col()
        .px_3()
        .py(outer_py)
        .w_full()
        .child(
            div()
                .w_full()
                .relative()
                .group("inbox-item")
                .p(px(8.))
                .rounded(px(8.))
                .bg(theme.bg_secondary)
                .child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(4.))
                        .w(px(20.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(theme.bg_hover)
                        .hover(|s| s.bg(theme.bg_primary))
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(12.))
                                .text_color(theme.text_muted),
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let id = id.clone();
                            move |_, _, cx| {
                                cx.stop_propagation();
                                this_delete
                                    .update(cx, |panel, cx| {
                                        panel.delete_notification(&id, category, cx);
                                    })
                                    .ok();
                            }
                        }),
                )
                .when(show_copy, |card| {
                    card.child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .right(px(28.))
                            .w(px(20.))
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .bg(theme.bg_hover)
                            .opacity(0.)
                            .group_hover("inbox-item", |s| s.opacity(1.))
                            .hover(|s| s.bg(theme.bg_primary))
                            .child(
                                Icon::new(IconName::CopyIcon)
                                    .size(px(12.))
                                    .text_color(theme.text_muted),
                            )
                            .on_mouse_down(MouseButton::Left, {
                                let copy_text = copy_text.clone();
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    this_copy
                                        .update(cx, |panel, cx| {
                                            panel.copy_message(&copy_text, cx);
                                        })
                                        .ok();
                                }
                            }),
                    )
                })
                .when(show_jump, |card| {
                    card.child(
                        div()
                            .absolute()
                            .bottom(px(10.))
                            .right(px(12.))
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .bg(theme.bg_hover)
                            .border_1()
                            .border_color(theme.border)
                            .text_xs()
                            .text_color(theme.text_primary)
                            .opacity(0.)
                            .group_hover("inbox-item", |s| s.opacity(1.))
                            .child(jump_label)
                            .on_mouse_down(MouseButton::Left, {
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    schedule_notification_jump(
                                        cx,
                                        inbox_handle_jump.clone(),
                                        jump_notification.clone(),
                                    );
                                }
                            }),
                    )
                })
                .child(
                    div()
                        .pr(if show_copy { px(52.) } else { px(28.) })
                        .when(show_jump, |content| content.pb(px(28.)))
                        .child(render_notification_body(
                            theme,
                            locale,
                            notification,
                            &view,
                            avatar_cache,
                            message_cache,
                            cx,
                        )),
                ),
        )
        .into_any_element()
}

fn render_topic_item(
    theme: &Theme,
    locale: &SharedString,
    topic: TopicDiscussion,
    view: TopicRowView,
    avatar_cache: Entity<LruImageCache>,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    cx: &App,
) -> gpui::AnyElement {
    let jump_topic = topic.clone();
    let inbox_handle_jump = inbox_handle.clone();
    let jump_label = mezon_i18n::t(locale, "channelTopbar.tooltips.jump");

    div()
        .flex()
        .flex_col()
        .px_3()
        .py(px(4.))
        .w_full()
        .child(
            div()
                .w_full()
                .relative()
                .group("inbox-topic")
                .p_2()
                .rounded(px(8.))
                .bg(theme.bg_secondary)
                .child(
                    div()
                        .absolute()
                        .bottom(px(10.))
                        .right(px(12.))
                        .px_2()
                        .py_1()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .bg(theme.bg_hover)
                        .border_1()
                        .border_color(theme.border)
                        .text_xs()
                        .text_color(theme.text_primary)
                        .opacity(0.)
                        .group_hover("inbox-topic", |s| s.opacity(1.))
                        .child(jump_label)
                        .on_mouse_down(MouseButton::Left, {
                            move |_, _, cx| {
                                cx.stop_propagation();
                                schedule_topic_jump(
                                    cx,
                                    inbox_handle_jump.clone(),
                                    jump_topic.clone(),
                                );
                            }
                        }),
                )
                .child(render_topic_body(
                    theme,
                    locale,
                    &topic,
                    &view,
                    avatar_cache,
                    cx,
                )),
        )
        .into_any_element()
}

pub fn clan_has_inbox_badge(clan_id: &str, cx: &App) -> bool {
    let Ok(clan_id) = clan_id.parse::<ClanId>() else {
        return false;
    };
    ClanList::global(cx)
        .read(cx)
        .clans
        .iter()
        .find(|c| c.id == clan_id)
        .is_some_and(|c| c.badge_count > 0 || c.has_unread)
}
