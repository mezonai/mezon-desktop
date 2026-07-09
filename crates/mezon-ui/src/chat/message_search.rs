use std::collections::BTreeMap;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Global, MouseButton, MouseDownEvent,
    Render, SharedString, WeakEntity, Window, deferred, div, img, prelude::*, px,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanId, MessageSearchStore, MessagesStore, SearchHit,
    message_time::local_datetime,
};
use ui::ScrollAxes;
use ui::Scrollbars;
use ui::WithScrollbar;

use crate::chat::layout::ChatLayout;
use crate::components::primitives::{Icon, IconName, Input, InputState};
use crate::router::{Route, Router, navigate};
use crate::theme::{ActiveTheme, Theme};

pub const MESSAGE_SEARCH_PANEL_WIDTH: f32 = 420.;
pub const SEARCH_BAR_WIDTH_COLLAPSED: f32 = 160.;
pub const SEARCH_BAR_WIDTH_EXPANDED: f32 = 320.;
pub const SEARCH_OPTIONS_WIDTH: f32 = 400.;
const HEADER_HORIZONTAL_PADDING: f32 = 16.;

const KEY_CONTEXT: &str = "MessageSearchPanel";

struct GlobalChatLayout(WeakEntity<ChatLayout>);
impl Global for GlobalChatLayout {}

pub(crate) fn register_chat_layout(layout: WeakEntity<ChatLayout>, cx: &mut App) {
    cx.set_global(GlobalChatLayout(layout));
}

pub fn query_has_filter_prefix(query: &str) -> bool {
    query.contains('>') || query.contains('~') || query.contains('&')
}

pub struct MessageSearchPanel {
    focus_handle: FocusHandle,
    layout: WeakEntity<ChatLayout>,
    channel_id: ChannelId,
    is_direct: bool,
    locale: SharedString,
    scroll: gpui::UniformListScrollHandle,
    image_cache: Entity<crate::image_cache::LruImageCache>,
}

impl Focusable for MessageSearchPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MessageSearchPanel {
    pub fn new(
        layout: WeakEntity<ChatLayout>,
        channel_id: ChannelId,
        is_direct: bool,
        locale: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            layout,
            channel_id,
            is_direct,
            locale,
            scroll: gpui::UniformListScrollHandle::new(),
            image_cache: cx.new(|cx| {
                crate::image_cache::LruImageCache::avatar_thumbnail(
                    "message-search-avatars",
                    crate::image_cache::AVATAR_IMAGE_CACHE_CAPACITY,
                    crate::image_cache::AVATAR_IMAGE_CACHE_BYTES,
                    crate::image_cache::AVATAR_ENTRY_MAX_BYTES,
                    cx,
                )
            }),
        }
    }

    fn close_results(&self, cx: &mut App) {
        if let Some(layout) = self.layout.upgrade() {
            layout.update(cx, |layout, cx| layout.close_results_panel(cx));
        }
    }

    fn jump_to_hit(&self, hit: &SearchHit, cx: &mut App) {
        let route = jump_route_for_hit(hit);
        navigate(cx, route);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.request_jump(hit.channel_id, hit.message_id, cx);
        });
    }
}

impl Render for MessageSearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale.clone();
        let channel_id = self.channel_id;
        let show_channel_label = !self.is_direct;
        let state = MessageSearchStore::global(cx).read(cx).state(channel_id);
        let entity = cx.weak_entity();

        let header_label: SharedString = if state.is_searching {
            mezon_i18n::t(&locale, "searchMessageChannel.searching").into()
        } else if state.total < 1 {
            mezon_i18n::t(&locale, "searchMessageChannel.noResults").into()
        } else {
            mezon_i18n::t(&locale, "searchMessageChannel.resultsCount")
                .replace("{{count}}", &state.total.to_string())
                .into()
        };

        let body: gpui::AnyElement = if state.is_searching {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.tokens.bg_outside_footer)
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme.tokens.text_theme_primary)
                        .child(mezon_i18n::t(&locale, "searchMessageChannel.searching")),
                )
                .into_any_element()
        } else if state.results.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px_6()
                .gap_2()
                .bg(theme.tokens.bg_outside_footer)
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(mezon_i18n::t(
                            &locale,
                            "searchMessageChannel.emptySearch.title",
                        )),
                )
                .into_any_element()
        } else {
            let groups = group_hits_by_channel(&state.results);
            let flat: Vec<(Option<SharedString>, SearchHit)> = groups
                .into_iter()
                .flat_map(|(label, hits)| {
                    hits.into_iter()
                        .map(move |hit| (label.clone(), hit))
                        .collect::<Vec<_>>()
                })
                .collect();
            let count = flat.len();
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .bg(theme.tokens.bg_outside_footer)
                .px_4()
                .py_4()
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&self.scroll),
                    window,
                    cx,
                )
                .child(
                    gpui::uniform_list(
                        "message-search-results",
                        count,
                        move |range, _window, cx| {
                            let theme = cx.theme();
                            let flat = flat.clone();
                            let entity = entity.clone();
                            let locale = locale.clone();
                            range
                                .map(|ix| {
                                    let (channel_label, hit) = &flat[ix];
                                    render_search_row(
                                        theme,
                                        hit,
                                        show_channel_label,
                                        channel_label.as_ref(),
                                        &locale,
                                        entity.clone(),
                                        ix == 0
                                            || flat
                                                .get(ix.wrapping_sub(1))
                                                .map(|(l, h)| {
                                                    l.as_ref().map(|s| s.as_ref())
                                                        != channel_label
                                                            .as_ref()
                                                            .map(|s| s.as_ref())
                                                        || h.channel_id != hit.channel_id
                                                })
                                                .unwrap_or(true),
                                    )
                                })
                                .collect::<Vec<_>>()
                        },
                    )
                    .track_scroll(&self.scroll)
                    .w_full(),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_row()
            .h_full()
            .flex_shrink_0()
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .bg(theme.tokens.border_theme_primary),
            )
            .child(
                div()
                    .track_focus(&self.focus_handle)
                    .key_context(KEY_CONTEXT)
                    .image_cache(self.image_cache.clone())
                    .on_action(cx.listener(|this, _: &::menu::Cancel, _, cx| {
                        this.close_results(cx);
                    }))
                    .flex()
                    .flex_col()
                    .w(px(MESSAGE_SEARCH_PANEL_WIDTH))
                    .h_full()
                    .flex_shrink_0()
                    .bg(theme.tokens.bg_outside_footer)
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(56.))
                            .px_4()
                            .border_b_1()
                            .border_color(theme.tokens.border_theme_primary)
                            .bg(theme.bg_primary)
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(header_label),
                            ),
                    )
                    .child(body),
            )
    }
}

fn group_hits_by_channel(hits: &[SearchHit]) -> Vec<(Option<SharedString>, Vec<SearchHit>)> {
    let mut order: Vec<ChannelId> = Vec::new();
    let mut map: BTreeMap<ChannelId, (Option<SharedString>, Vec<SearchHit>)> = BTreeMap::new();
    for hit in hits {
        if !order.contains(&hit.channel_id) {
            order.push(hit.channel_id);
        }
        map.entry(hit.channel_id)
            .or_insert_with(|| {
                (
                    (!hit.channel_label.is_empty()).then(|| hit.channel_label.clone()),
                    Vec::new(),
                )
            })
            .1
            .push(hit.clone());
    }
    order.into_iter().filter_map(|id| map.remove(&id)).collect()
}

fn format_search_timestamp(timestamp: i64) -> String {
    let Some(dt) = local_datetime(timestamp) else {
        return String::new();
    };
    dt.format("%B %d, %Y %I:%M %p").to_string()
}

fn render_search_row(
    theme: &Theme,
    hit: &SearchHit,
    show_channel_label: bool,
    channel_label: Option<&SharedString>,
    locale: &SharedString,
    entity: WeakEntity<MessageSearchPanel>,
    show_group_header: bool,
) -> gpui::AnyElement {
    let hit_for_jump = hit.clone();
    let jump_label = mezon_i18n::t(locale, "searchMessageChannel.jump");
    let timestamp = format_search_timestamp(hit.create_time);
    let leading = if hit.avatar_proxied.is_empty() {
        div()
            .size(px(32.))
            .rounded_full()
            .flex_shrink_0()
            .bg(theme.tokens.bg_active_member_channel)
            .into_any_element()
    } else {
        img(hit.avatar_proxied.clone())
            .size(px(32.))
            .rounded_full()
            .flex_shrink_0()
            .into_any_element()
    };

    let group_header =
        (show_channel_label && show_group_header && channel_label.is_some_and(|l| !l.is_empty()))
            .then(|| {
                div()
                    .mb_2()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.tokens.text_theme_primary)
                    .truncate()
                    .child(format!("# {}", channel_label.unwrap()))
            });

    div()
        .id(format!("message-search-hit-{}", hit.message_id.get()))
        .w_full()
        .mb_3()
        .children(group_header)
        .child(
            div()
                .relative()
                .group("message-search-hit")
                .w_full()
                .px(px(5.))
                .pb_3()
                .rounded_md()
                .bg(theme.bg_tertiary)
                .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    if let Some(panel) = entity.upgrade() {
                        panel.update(cx, |panel, cx| panel.jump_to_hit(&hit_for_jump, cx));
                    }
                })
                .child(
                    div()
                        .absolute()
                        .top(px(10.))
                        .right_3()
                        .opacity(0.)
                        .group_hover("message-search-hit", |s| s.opacity(100.))
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(6.))
                                .bg(theme.bg_secondary)
                                .text_size(px(10.))
                                .text_color(theme.tokens.text_theme_primary)
                                .child(jump_label),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pt_2()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .child(leading)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(14.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(theme.interactive_active)
                                                .truncate()
                                                .child(hit.sender_name.to_string()),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(px(12.))
                                                .text_color(theme.text_muted)
                                                .child(timestamp),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .px_2()
                                .pb_2()
                                .text_size(px(13.))
                                .text_color(theme.tokens.text_theme_primary)
                                .line_clamp(3)
                                .child(hit.content_preview.to_string()),
                        ),
                ),
        )
        .into_any_element()
}

pub fn render_header_search_bar(
    theme: &Theme,
    locale: &str,
    search_input: Option<&Entity<InputState>>,
    expanded: bool,
    show_options: bool,
    layout: WeakEntity<ChatLayout>,
    cx: &mut App,
) -> gpui::AnyElement {
    let placeholder = mezon_i18n::t(locale, "searchMessageChannel.searchPlaceholder");
    let query = search_input
        .map(|input| input.read(cx).value().to_string())
        .unwrap_or_default();
    let has_query = !query.is_empty();
    let width = if expanded {
        SEARCH_BAR_WIDTH_EXPANDED
    } else {
        SEARCH_BAR_WIDTH_COLLAPSED
    };
    let layout_for_click = layout.clone();
    let layout_for_option = layout.clone();
    let layout_for_close = layout.clone();
    let layout_for_dismiss = layout.clone();
    let search_input_for_close = search_input.cloned();

    let options_dropdown =
        show_options.then(|| render_search_options(theme, locale, &query, layout_for_option));

    let input_or_placeholder: gpui::AnyElement = if expanded {
        if let Some(search_input) = search_input {
            Input::new(search_input)
                .w_full()
                .text_size(px(14.))
                .text_color(theme.tokens.text_theme_primary)
                .into_any_element()
        } else {
            div()
                .w_full()
                .text_size(px(14.))
                .text_color(theme.tokens.text_theme_primary)
                .child(placeholder)
                .into_any_element()
        }
    } else {
        div()
            .w_full()
            .text_size(px(14.))
            .text_color(theme.tokens.text_theme_primary)
            .child(placeholder)
            .into_any_element()
    };

    div()
        .relative()
        .flex_shrink_0()
        .when(expanded && show_options, |el| {
            el.on_mouse_down_out(move |_: &MouseDownEvent, window, cx| {
                if let Some(layout) = layout_for_dismiss.upgrade() {
                    layout.update(cx, |layout, cx| {
                        layout.dismiss_message_search_options(window, cx);
                    });
                }
            })
        })
        .child(
            div()
                .relative()
                .w(px(width))
                .h(px(32.))
                .flex()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .rounded_lg()
                .border_1()
                .border_color(theme.tokens.border_theme_primary)
                .bg(theme.tokens.bg_surface)
                .px_2()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    if let Some(layout) = layout_for_click.upgrade() {
                        layout.update(cx, |layout, cx| layout.expand_message_search(window, cx));
                    }
                })
                .child(div().flex_1().min_w_0().child(input_or_placeholder))
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(24.))
                        .h(px(24.))
                        .when(!has_query, |el| {
                            el.child(
                                Icon::new(IconName::Search)
                                    .size(px(16.))
                                    .mt(px(1.))
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                        })
                        .when(has_query, |el| {
                            el.cursor_pointer()
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    if let Some(search_input_for_close) =
                                        search_input_for_close.clone()
                                    {
                                        search_input_for_close.update(cx, |input, cx| {
                                            input.set_value("", window, cx);
                                        });
                                    }
                                    if let Some(layout) = layout_for_close.upgrade() {
                                        layout.update(cx, |layout, cx| {
                                            layout.on_search_input_cleared(window, cx);
                                        });
                                    }
                                })
                                .child(
                                    Icon::new(IconName::Close)
                                        .size(px(16.))
                                        .text_color(theme.tokens.text_theme_primary),
                                )
                        }),
                ),
        )
        .children(options_dropdown.map(|menu| {
            deferred(
                div()
                    .absolute()
                    .left_0()
                    .right(px(-HEADER_HORIZONTAL_PADDING))
                    .top(px(40.))
                    .occlude()
                    .child(menu),
            )
        }))
        .into_any_element()
}

fn render_search_options(
    theme: &Theme,
    locale: &str,
    query: &str,
    layout: WeakEntity<ChatLayout>,
) -> gpui::AnyElement {
    let has_query = !query.is_empty();
    let options = [
        (
            ">",
            mezon_i18n::t(
                locale,
                "searchMessageChannel.searchOptionsData.fromUserShort",
            ),
        ),
        (
            "~",
            mezon_i18n::t(
                locale,
                "searchMessageChannel.searchOptionsData.mentionsUserShort",
            ),
        ),
        (
            "&",
            mezon_i18n::t(
                locale,
                "searchMessageChannel.searchOptionsData.hasContentShort",
            ),
        ),
    ];
    let header = mezon_i18n::t(locale, "searchMessageChannel.searchOptions");
    let search_for_label = mezon_i18n::t(locale, "searchMessageChannel.searchFor");
    let enter_label = mezon_i18n::t(locale, "searchMessageChannel.enter");

    let search_for_row = has_query.then(|| {
        div()
            .p_3()
            .border_b_1()
            .border_color(theme.tokens.border_theme_primary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .mr_1()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(search_for_label.to_uppercase()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .mr(px(10.))
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.interactive_active)
                                    .truncate()
                                    .child(query.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .h(px(20.))
                            .w(px(40.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(theme.tokens.border_divider)
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(enter_label),
                    ),
            )
    });

    div()
        .relative()
        .w_full()
        .rounded_md()
        .overflow_hidden()
        .shadow_lg()
        .border_1()
        .border_color(theme.tokens.border_theme_primary)
        .text_color(theme.tokens.text_theme_primary)
        .child(
            div()
                .absolute()
                .inset_0()
                .bg(theme.tokens.bg_theme_contexify),
        )
        .child(div().absolute().inset_0().bg(theme.tokens.theme_base_color))
        .child(
            div()
                .relative()
                .when(has_query, |el| el.pt_0())
                .when(!has_query, |el| el.pt_3())
                .pb_3()
                .children(search_for_row)
                .child(
                    div()
                        .mx_3()
                        .when(!has_query, |el| el.mt_3())
                        .border_b_1()
                        .border_color(theme.tokens.border_theme_primary)
                        .pb_3()
                        .child(
                            div().flex().items_center().pb_2().px_2().child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(header.to_uppercase()),
                            ),
                        )
                        .children(
                            options
                                .into_iter()
                                .map(|(prefix, content)| {
                                    let layout = layout.clone();
                                    let prefix_label = prefix.to_string();
                                    let prefix_insert = prefix_label.clone();
                                    let prefix_weight = if prefix == "~" {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::SEMIBOLD
                                    };
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                            if let Some(layout) = layout.upgrade() {
                                                layout.update(cx, |layout, cx| {
                                                    layout.insert_search_option_prefix(
                                                        &prefix_insert,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        })
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_size(px(14.))
                                                        .font_weight(prefix_weight)
                                                        .text_color(theme.interactive_active)
                                                        .child(format!("{prefix_label} ")),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(14.))
                                                        .text_color(theme.tokens.text_theme_primary)
                                                        .child(content),
                                                ),
                                        )
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>(),
                        ),
                ),
        )
        .into_any_element()
}

fn jump_route_for_hit(hit: &SearchHit) -> Route {
    if hit.clan_id.is_zero() {
        Route::DirectMessage {
            direct_id: hit.channel_id,
            message_type: hit.channel_type.to_string(),
        }
    } else {
        Route::Channel {
            clan_id: hit.clan_id,
            channel_id: hit.channel_id,
        }
    }
}

pub fn message_search_available(cx: &App) -> Option<(ChannelId, ClanId, bool)> {
    let route = Router::global(cx).read(cx).route();
    match route {
        Route::DirectMessage { direct_id, .. } => Some((direct_id, ClanId(0), true)),
        Route::Channel {
            channel_id,
            clan_id,
            ..
        }
        | Route::Thread {
            channel_id,
            clan_id,
            ..
        } => {
            let channel = ChannelList::global(cx)
                .read(cx)
                .channel(clan_id, channel_id)?;
            if channel.channel_type == ChannelType::Voice {
                None
            } else {
                Some((channel_id, clan_id, false))
            }
        }
        _ => {
            let channel = ChannelList::global(cx).read(cx).active_channel()?;
            if channel.channel_type == ChannelType::Voice {
                None
            } else if channel.clan_id.is_zero() {
                Some((channel.id, ClanId(0), true))
            } else {
                Some((channel.id, channel.clan_id, false))
            }
        }
    }
}

pub fn try_open_message_search(window: &mut Window, cx: &mut App) {
    let Some(layout) = cx
        .try_global::<GlobalChatLayout>()
        .and_then(|global| global.0.upgrade())
    else {
        return;
    };
    layout.update(cx, |layout, cx| layout.expand_message_search(window, cx));
}

pub fn init(cx: &mut App) {
    gpui::actions!(mezon_message_search, [OpenMessageSearch]);
    cx.bind_keys([
        gpui::KeyBinding::new("secondary-f", OpenMessageSearch, None),
        gpui::KeyBinding::new("ctrl-f", OpenMessageSearch, None),
        gpui::KeyBinding::new("escape", ::menu::Cancel, Some(KEY_CONTEXT)),
    ]);
    cx.on_action(|_: &OpenMessageSearch, cx: &mut App| {
        let Some(window_handle) =
            crate::app::main_window::handle(cx).or_else(|| cx.active_window())
        else {
            return;
        };
        cx.defer(move |cx| {
            let _ = cx.update_window(window_handle, |_, window, cx| {
                try_open_message_search(window, cx);
            });
        });
    });
}
