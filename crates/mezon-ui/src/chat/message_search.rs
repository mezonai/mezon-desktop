use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use chrono::Local;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Global, HighlightStyle,
    ListAlignment, ListState, MouseButton, MouseDownEvent, ObjectFit, Render, SharedString,
    StyledText, Transformation, WeakEntity, Window, deferred, div, img, list, prelude::*, px,
    radians,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanId, ClanMembersStore, MessageSearchStore,
    MessagesStore, RolesStore, SearchDropdownMode, SearchHit, SearchPageToken, autocomplete_needle,
    has_filter_options, search_content_highlight_terms, search_dropdown_mode, search_page_count,
    search_page_numbers, should_show_search_dropdown,
};
use ui::ScrollAxes;
use ui::Scrollbars;
use ui::WithScrollbar;

use crate::chat::layout::ChatLayout;
use crate::chat::member_list::{MentionMemberRaw, mention_member_pool};
use crate::chat::message::{DEFAULT_DISPLAY_NAME_COLOR, format_message_time};
use crate::components::primitives::{Icon, IconName, Input, InputState, Sizable, Size, Spinner};
use crate::image_cache::{
    LruImageCache, MESSAGE_ENTRY_MAX_BYTES, MESSAGE_IMAGE_CACHE_BYTES, MESSAGE_IMAGE_CACHE_CAPACITY,
};
use crate::router::{Route, Router, navigate};
use crate::theme::{ActiveTheme, Theme};

pub const MESSAGE_SEARCH_PANEL_WIDTH: f32 = 420.;
pub const SEARCH_BAR_WIDTH_COLLAPSED: f32 = 160.;
pub const SEARCH_BAR_WIDTH_EXPANDED: f32 = 320.;
pub const SEARCH_OPTIONS_WIDTH: f32 = 400.;
const SEARCH_VIRTUALIZE_THRESHOLD: usize = 30;
const SEARCH_LIST_OVERDRAW: f32 = 120.;
const DEFAULT_ROLE_COLOR: u32 = 0x99_aab5;

const SEARCH_PREFIX_OPTIONS: &[&str] = &[">", "~", "&"];

const KEY_CONTEXT: &str = "MessageSearchPanel";
const SEARCH_DROPDOWN_KEY_CONTEXT: &str = "MessageSearchDropdown";

struct GlobalChatLayout(WeakEntity<ChatLayout>);
impl Global for GlobalChatLayout {}

pub(crate) fn register_chat_layout(layout: WeakEntity<ChatLayout>, cx: &mut App) {
    cx.set_global(GlobalChatLayout(layout));
}

pub fn should_show_message_search_dropdown(query: &str) -> bool {
    should_show_search_dropdown(query)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchDropdownItem {
    Prefix(&'static str),
    Member {
        trigger: char,
        display: String,
        username: String,
        user_id: String,
    },
    Has {
        value: String,
    },
}

pub fn search_dropdown_item_count(query: &str, cx: &App) -> usize {
    search_dropdown_items(query, cx).len()
}

pub fn search_dropdown_items(query: &str, cx: &App) -> Vec<SearchDropdownItem> {
    let mode = search_dropdown_mode(query);
    let needle = autocomplete_needle(query);
    match mode {
        SearchDropdownMode::Options => SEARCH_PREFIX_OPTIONS
            .iter()
            .copied()
            .map(SearchDropdownItem::Prefix)
            .collect(),
        SearchDropdownMode::FromUser | SearchDropdownMode::PlainMembers => {
            filter_members_for_from(&mention_member_pool(cx), &needle, 3)
                .into_iter()
                .map(|member| SearchDropdownItem::Member {
                    trigger: '>',
                    display: member.username.clone(),
                    username: member.username.clone(),
                    user_id: member.user_id.clone(),
                })
                .collect()
        }
        SearchDropdownMode::Mentions => {
            filter_members_for_mentions(&mention_member_pool(cx), &needle, 3)
                .into_iter()
                .map(|member| SearchDropdownItem::Member {
                    trigger: '~',
                    display: member.display.clone(),
                    username: member.username.clone(),
                    user_id: member.user_id.clone(),
                })
                .collect()
        }
        SearchDropdownMode::Has => filtered_has_values(&needle)
            .into_iter()
            .map(|value| SearchDropdownItem::Has { value })
            .collect(),
        SearchDropdownMode::Hidden => Vec::new(),
    }
}

pub fn apply_search_dropdown_item(
    layout: &mut ChatLayout,
    item: &SearchDropdownItem,
    window: &mut Window,
    cx: &mut Context<ChatLayout>,
) {
    match item {
        SearchDropdownItem::Prefix(prefix) => {
            layout.insert_search_option_prefix(prefix, window, cx);
        }
        SearchDropdownItem::Member {
            trigger,
            username,
            user_id,
            ..
        } => {
            layout.insert_search_filter_markup(*trigger, username, user_id, window, cx);
        }
        SearchDropdownItem::Has { value } => {
            layout.insert_search_filter_markup('&', value, value, window, cx);
        }
    }
}

pub fn activate_search_dropdown_item(
    item: &SearchDropdownItem,
    layout: WeakEntity<ChatLayout>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(layout) = layout.upgrade() else {
        return;
    };
    layout.update(cx, |layout, cx| {
        apply_search_dropdown_item(layout, item, window, cx);
    });
}

#[derive(Clone)]
struct SearchListRow {
    channel_label: Option<SharedString>,
    show_group_header: bool,
    hit: SearchHit,
}

pub struct MessageSearchPanel {
    focus_handle: FocusHandle,
    layout: WeakEntity<ChatLayout>,
    channel_id: ChannelId,
    clan_id: ClanId,
    is_direct: bool,
    locale: SharedString,
    scroll: gpui::ScrollHandle,
    list_state: ListState,
    last_results_page: i32,
    avatar_image_cache: Entity<LruImageCache>,
    attachment_image_cache: Entity<LruImageCache>,
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
        clan_id: ClanId,
        is_direct: bool,
        locale: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            layout,
            channel_id,
            clan_id,
            is_direct,
            locale,
            scroll: gpui::ScrollHandle::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(SEARCH_LIST_OVERDRAW))
                .measure_all(),
            last_results_page: 0,
            avatar_image_cache: cx.new(|cx| {
                LruImageCache::avatar_thumbnail(
                    "message-search-avatars",
                    crate::image_cache::AVATAR_IMAGE_CACHE_CAPACITY,
                    crate::image_cache::AVATAR_IMAGE_CACHE_BYTES,
                    crate::image_cache::AVATAR_ENTRY_MAX_BYTES,
                    cx,
                )
            }),
            attachment_image_cache: cx.new(|cx| {
                LruImageCache::message(
                    "message-search-attachments",
                    MESSAGE_IMAGE_CACHE_CAPACITY,
                    MESSAGE_IMAGE_CACHE_BYTES,
                    MESSAGE_ENTRY_MAX_BYTES,
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

    fn set_results_page(&mut self, page: i32, cx: &mut Context<Self>) {
        if self.last_results_page != page {
            self.last_results_page = page;
            self.scroll.set_offset(gpui::point(px(0.), px(0.)));
            self.list_state.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
            cx.notify();
        }
    }

    fn go_to_page(&self, page: i32, cx: &mut App) {
        MessageSearchStore::global(cx).update(cx, |store, cx| {
            store.set_page(self.channel_id, self.clan_id, self.is_direct, page, cx);
        });
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
        let is_direct = self.is_direct;
        let state = MessageSearchStore::global(cx).read(cx).state(channel_id);
        let entity = cx.weak_entity();
        if !state.is_searching {
            self.set_results_page(state.current_page, cx);
        }
        let page_count = search_page_count(state.total);
        let show_searching_body = state.is_searching;

        let header_label: SharedString = if show_searching_body {
            mezon_i18n::t(&locale, "searchMessageChannel.searching").into()
        } else if state.has_error {
            mezon_i18n::t(&locale, "searchMessageChannel.searchFailed").into()
        } else if state.total < 1 {
            mezon_i18n::t(&locale, "searchMessageChannel.noResults").into()
        } else {
            mezon_i18n::t(&locale, "searchMessageChannel.resultsCount")
                .replace("{{count}}", &state.total.to_string())
                .into()
        };

        let body: gpui::AnyElement = if show_searching_body {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.bg_secondary)
                .child(
                    Spinner::new()
                        .with_size(Size::Large)
                        .color(theme.tokens.text_theme_primary.into()),
                )
                .into_any_element()
        } else if state.has_error {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px_6()
                .gap_2()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(mezon_i18n::t(&locale, "searchMessageChannel.searchFailed")),
                )
                .into_any_element()
        } else if state.results.is_empty() && !state.is_searching {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            img("icons/empty-search.svg")
                                .w(px(160.))
                                .h(px(160.))
                                .flex_none(),
                        )
                        .child(
                            div()
                                .mt(px(40.))
                                .w(px(280.))
                                .text_center()
                                .text_size(px(16.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.tokens.text_theme_primary)
                                .child(mezon_i18n::t(
                                    &locale,
                                    "searchMessageChannel.emptySearch.title",
                                )),
                        ),
                )
                .into_any_element()
        } else {
            let flat = build_search_list_rows(&state.results);
            let current_page = state.current_page;
            let panel = cx.weak_entity();
            let now = Local::now();
            let highlight_terms = search_content_highlight_terms(&state.query);
            let highlight_terms = Rc::new(highlight_terms);
            let use_virtual_list = flat.len() > SEARCH_VIRTUALIZE_THRESHOLD;
            if self.list_state.item_count() != flat.len() {
                self.list_state.reset(flat.len());
            } else if use_virtual_list {
                self.list_state.remeasure();
            }
            let list_state = self.list_state.clone();
            let flat_rows = Rc::new(flat);
            let avatar_cache = self.avatar_image_cache.clone();
            let attachment_cache = self.attachment_image_cache.clone();
            let scroll = self.scroll.clone();
            let pagination = (page_count > 1)
                .then(|| render_search_pagination(&theme, current_page, page_count, panel));
            let scroll_area = if use_virtual_list {
                let theme_for_list = theme.clone();
                let locale_for_list = locale.clone();
                let entity_for_list = entity.clone();
                let highlight_for_list = highlight_terms.clone();
                let is_direct_for_list = is_direct;
                let avatar_cache_for_list = avatar_cache.clone();
                let attachment_cache_for_list = attachment_cache.clone();
                div()
                    .id(format!("message-search-results-{current_page}"))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .custom_scrollbars(
                        Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&scroll),
                        window,
                        cx,
                    )
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div().flex_1().min_h_0().child(
                            list(list_state.clone(), move |ix, _window, cx| {
                                let Some(row) = flat_rows.get(ix) else {
                                    return div().into_any_element();
                                };
                                render_search_row(
                                    &theme_for_list,
                                    &row.hit,
                                    show_channel_label,
                                    row.channel_label.as_ref(),
                                    &locale_for_list,
                                    entity_for_list.clone(),
                                    row.show_group_header,
                                    now,
                                    &highlight_for_list,
                                    is_direct_for_list,
                                    avatar_cache_for_list.clone(),
                                    attachment_cache_for_list.clone(),
                                    cx,
                                )
                            })
                            .size_full(),
                        ),
                    )
                    .children(pagination)
                    .into_any_element()
            } else {
                let rows = flat_rows
                    .iter()
                    .map(|row| {
                        render_search_row(
                            &theme,
                            &row.hit,
                            show_channel_label,
                            row.channel_label.as_ref(),
                            &locale,
                            entity.clone(),
                            row.show_group_header,
                            now,
                            &highlight_terms,
                            is_direct,
                            avatar_cache.clone(),
                            attachment_cache.clone(),
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();
                div()
                    .id(format!("message-search-results-{current_page}"))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .custom_scrollbars(
                        Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&scroll),
                        window,
                        cx,
                    )
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .w_full()
                    .children(rows)
                    .children(pagination)
                    .into_any_element()
            };
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(scroll_area)
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
                    .image_cache(self.avatar_image_cache.clone())
                    .on_action(cx.listener(|this, _: &::menu::Cancel, _, cx| {
                        this.close_results(cx);
                    }))
                    .flex()
                    .flex_col()
                    .w(px(MESSAGE_SEARCH_PANEL_WIDTH))
                    .h_full()
                    .flex_shrink_0()
                    .bg(theme.bg_secondary)
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
                            .bg(theme.bg_secondary)
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

fn render_search_pagination(
    theme: &Theme,
    current_page: i32,
    page_count: i32,
    panel: WeakEntity<MessageSearchPanel>,
) -> gpui::AnyElement {
    let pages = search_page_numbers(current_page, page_count);
    let prev_disabled = current_page <= 1;
    let next_disabled = current_page >= page_count;

    div()
        .flex_shrink_0()
        .mt_4()
        .h(px(40.))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .child(render_pagination_nav_button(
            theme,
            true,
            prev_disabled,
            {
                let panel = panel.clone();
                let page = current_page - 1;
                move |_, _, cx| {
                    if let Some(panel) = panel.upgrade() {
                        panel.update(cx, |panel, cx| panel.go_to_page(page, cx));
                    }
                }
            },
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_center()
                .gap_2()
                .min_w_0()
                .children(pages.into_iter().map(|token| {
                    let panel = panel.clone();
                    match token {
                        SearchPageToken::Ellipsis => div()
                            .px_2()
                            .text_size(px(14.))
                            .text_color(theme.text_muted)
                            .child("...")
                            .into_any_element(),
                        SearchPageToken::Page(page) => {
                            let active = page == current_page;
                            render_pagination_page_button(theme, page, active, move |_, _, cx| {
                                if let Some(panel) = panel.upgrade() {
                                    panel.update(cx, |panel, cx| panel.go_to_page(page, cx));
                                }
                            })
                        }
                    }
                })),
        )
        .child(render_pagination_nav_button(
            theme,
            false,
            next_disabled,
            {
                let panel = panel.clone();
                let page = current_page + 1;
                move |_, _, cx| {
                    if let Some(panel) = panel.upgrade() {
                        panel.update(cx, |panel, cx| panel.go_to_page(page, cx));
                    }
                }
            },
        ))
        .into_any_element()
}

fn render_pagination_nav_button(
    theme: &Theme,
    previous: bool,
    disabled: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let icon_color = if disabled {
        theme.text_muted
    } else {
        theme.tokens.text_theme_primary
    };
    let mut icon = Icon::new(IconName::ArrowRight)
        .size(px(20.))
        .text_color(icon_color);
    if previous {
        icon = icon.with_transformation(Transformation::rotate(radians(std::f32::consts::PI)));
    }
    div()
        .flex_shrink_0()
        .w(px(40.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(theme.tokens.border_theme_primary)
        .when(disabled, |el| {
            el.bg(theme.tokens.bg_tertiary)
                .opacity(50.)
                .cursor_default()
        })
        .when(!disabled, |el| {
            el.cursor_pointer()
                .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .child(icon)
        .into_any_element()
}

fn render_pagination_page_button(
    theme: &Theme,
    page: i32,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .min_w(px(40.))
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(theme.tokens.border_theme_primary)
        .when(active, |el| {
            el.bg(theme.tokens.bg_active_button)
                .text_color(theme.tokens.color_text_active_button)
        })
        .when(!active, |el| {
            el.cursor_pointer()
                .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .text_size(px(14.))
        .text_color(theme.tokens.text_theme_primary)
        .child(page.to_string())
        .into_any_element()
}

fn build_search_list_rows(hits: &[SearchHit]) -> Vec<SearchListRow> {
    let groups = group_hits_by_channel(hits);
    let flat: Vec<(Option<SharedString>, SearchHit)> = groups
        .into_iter()
        .flat_map(|(label, group_hits)| {
            group_hits
                .into_iter()
                .map(move |hit| (label.clone(), hit))
                .collect::<Vec<_>>()
        })
        .collect();
    flat.iter()
        .enumerate()
        .map(|(ix, (channel_label, hit))| {
            let show_group_header = ix == 0
                || flat
                    .get(ix.wrapping_sub(1))
                    .map(|(l, h)| {
                        l.as_ref().map(|s| s.as_ref()) != channel_label.as_ref().map(|s| s.as_ref())
                            || h.channel_id != hit.channel_id
                    })
                    .unwrap_or(true);
            SearchListRow {
                channel_label: channel_label.clone(),
                show_group_header,
                hit: hit.clone(),
            }
        })
        .collect()
}

fn group_hits_by_channel(hits: &[SearchHit]) -> Vec<(Option<SharedString>, Vec<SearchHit>)> {
    let mut order: Vec<ChannelId> = Vec::new();
    let mut map: BTreeMap<ChannelId, (Option<SharedString>, Vec<SearchHit>)> = BTreeMap::new();
    for hit in hits {
        if !order.contains(&hit.channel_id) {
            order.push(hit.channel_id);
        }
        let entry = map
            .entry(hit.channel_id)
            .or_insert_with(|| (None, Vec::new()));
        if entry.0.is_none() && !hit.channel_label.is_empty() {
            entry.0 = Some(hit.channel_label.clone());
        }
        entry.1.push(hit.clone());
    }
    order.into_iter().filter_map(|id| map.remove(&id)).collect()
}

fn render_search_row(
    theme: &Theme,
    hit: &SearchHit,
    show_channel_label: bool,
    channel_label: Option<&SharedString>,
    locale: &SharedString,
    entity: WeakEntity<MessageSearchPanel>,
    show_group_header: bool,
    now: chrono::DateTime<Local>,
    highlight_terms: &[String],
    is_direct: bool,
    avatar_image_cache: Entity<LruImageCache>,
    attachment_image_cache: Entity<LruImageCache>,
    cx: &App,
) -> gpui::AnyElement {
    let hit_for_jump = hit.clone();
    let entity_for_jump = entity.clone();
    let jump_label = mezon_i18n::t(locale, "searchMessageChannel.jump");
    let time_label = format_message_time(&hit.time_hhmm, hit.local_date, locale, now);
    let has_text = !hit.content_preview.is_empty();
    let sender_color = resolve_search_sender_color(hit, is_direct, cx);
    let resolved_channel_label = resolve_search_channel_label(hit, channel_label, cx);
    let leading = if hit.avatar_proxied.is_empty() {
        div()
            .size(px(40.))
            .rounded_full()
            .flex_shrink_0()
            .bg(theme.tokens.bg_active_member_channel)
            .into_any_element()
    } else {
        div()
            .image_cache(avatar_image_cache)
            .child(
                img(hit.avatar_proxied.clone())
                    .size(px(40.))
                    .rounded_full()
                    .flex_shrink_0(),
            )
            .into_any_element()
    };

    let image_preview = hit.image_attachment.as_ref().and_then(|image| {
        if image.proxied_src.is_empty() {
            return None;
        }
        Some(
            div()
                .mt_1()
                .image_cache(attachment_image_cache)
                .child(
                    img(image.proxied_src.clone())
                        .w(px(image.display_width))
                        .h(px(image.display_height))
                        .max_w(px(280.))
                        .max_h(px(200.))
                        .rounded_md()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element(),
        )
    });

    let group_header =
        (show_channel_label && show_group_header && resolved_channel_label.is_some()).then(|| {
            div()
                .mb_2()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.tokens.text_theme_primary)
                .truncate()
                .child(format!("# {}", resolved_channel_label.unwrap()))
        });

    div()
        .id(format!("message-search-hit-{}", hit.message_id.get()))
        .w_full()
        .mb_2()
        .children(group_header)
        .child(
            div()
                .relative()
                .group("message-search-hit")
                .w_full()
                .px(px(5.))
                .pb_3()
                .rounded_md()
                .bg(theme.tokens.bg_active_member_channel)
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
                                .bg(theme.tokens.bg_secondary)
                                .text_size(px(10.))
                                .text_color(theme.tokens.text_theme_primary)
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(panel) = entity_for_jump.upgrade() {
                                        panel.update(cx, |panel, cx| {
                                            panel.jump_to_hit(&hit_for_jump, cx);
                                        });
                                    }
                                })
                                .child(jump_label),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .gap_2()
                        .pt_2()
                        .px_2()
                        .child(leading)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_baseline()
                                        .gap_2()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(px(16.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(sender_color)
                                                .child(hit.sender_name.to_string()),
                                        )
                                        .when(!time_label.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(theme.tokens.text_theme_primary)
                                                    .child(time_label),
                                            )
                                        }),
                                )
                                .when(has_text, |el| {
                                    el.child(
                                        div()
                                            .text_size(px(14.))
                                            .text_color(theme.tokens.text_theme_message)
                                            .line_clamp(3)
                                            .child(render_highlighted_content(
                                                hit.content_preview.as_ref(),
                                                highlight_terms,
                                                theme,
                                            )),
                                    )
                                })
                                .children(image_preview),
                        ),
                ),
        )
        .into_any_element()
}

fn resolve_search_channel_label(
    hit: &SearchHit,
    channel_label: Option<&SharedString>,
    cx: &App,
) -> Option<String> {
    if let Some(label) = channel_label.filter(|label| !label.is_empty()) {
        return Some(label.to_string());
    }
    if !hit.channel_label.is_empty() {
        return Some(hit.channel_label.to_string());
    }
    ChannelList::try_global(cx).and_then(|channels| {
        channels
            .read(cx)
            .channel(hit.clan_id, hit.channel_id)
            .map(|channel| channel.name.clone())
            .filter(|name| !name.is_empty())
    })
}

fn resolve_search_sender_color(hit: &SearchHit, is_direct: bool, cx: &App) -> gpui::Hsla {
    if is_direct {
        return gpui::Hsla::from(gpui::rgb(DEFAULT_DISPLAY_NAME_COLOR));
    }
    let Some(user_id) = hit.sender_id else {
        return gpui::Hsla::from(gpui::rgb(DEFAULT_ROLE_COLOR));
    };
    let role_ids = ClanMembersStore::global(cx)
        .read(cx)
        .member(hit.clan_id, user_id)
        .map(|member| member.role_ids.clone())
        .unwrap_or_default();
    if role_ids.is_empty() {
        return gpui::Hsla::from(gpui::rgb(DEFAULT_ROLE_COLOR));
    }
    let Some(roles_store) = RolesStore::try_global(cx) else {
        return gpui::Hsla::from(gpui::rgb(DEFAULT_ROLE_COLOR));
    };
    for role in roles_store.read(cx).roles_for(hit.clan_id, &role_ids) {
        if let Some(color) = parse_hex_role_color(&role.color) {
            return gpui::Hsla::from(color);
        }
    }
    gpui::Hsla::from(gpui::rgb(DEFAULT_ROLE_COLOR))
}

fn parse_hex_role_color(raw: &str) -> Option<gpui::Rgba> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(gpui::Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.,
        g: ((value >> 8) & 0xff) as f32 / 255.,
        b: (value & 0xff) as f32 / 255.,
        a: 1.,
    })
}

fn render_highlighted_content(text: &str, terms: &[String], theme: &Theme) -> gpui::AnyElement {
    let ranges = search_highlight_ranges(text, terms);
    if ranges.is_empty() {
        return div().child(text.to_string()).into_any_element();
    }
    let highlight_style = HighlightStyle {
        background_color: Some(theme.tokens.bg_item_theme_hover.into()),
        color: Some(theme.tokens.text_theme_message.into()),
        ..Default::default()
    };
    let highlights = ranges
        .into_iter()
        .map(|range| (range, highlight_style))
        .collect::<Vec<_>>();
    div()
        .child(StyledText::new(text).with_highlights(highlights))
        .into_any_element()
}

fn search_highlight_ranges(text: &str, terms: &[String]) -> Vec<Range<usize>> {
    if terms.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    let mut ranges = Vec::new();
    for term in terms {
        let needle = term.to_lowercase();
        if needle.len() < 2 {
            continue;
        }
        let mut start = 0;
        while let Some(rel) = lower[start..].find(&needle) {
            let begin = start + rel;
            let end = begin + needle.len();
            ranges.push(begin..end);
            start = end;
        }
    }
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged = Vec::with_capacity(ranges.len());
    merged.push(ranges[0].clone());
    for range in ranges.into_iter().skip(1) {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
    }
    merged
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
    let selected_index = layout
        .upgrade()
        .map(|layout| layout.read(cx).search_dropdown_index())
        .unwrap_or(0);

    let options_dropdown = show_options.then(|| {
        render_search_options(theme, locale, &query, layout_for_option, selected_index, cx)
    });

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
        .when(expanded, |el| {
            el.on_mouse_down_out(move |_: &MouseDownEvent, window, cx| {
                if let Some(layout) = layout_for_dismiss.upgrade() {
                    layout.update(cx, |layout, cx| {
                        layout.dismiss_message_search_options(window, cx);
                    });
                }
            })
        })
        .when(expanded && show_options, |el| {
            el.key_context(SEARCH_DROPDOWN_KEY_CONTEXT)
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
                    .top(px(40.))
                    .w(px(SEARCH_OPTIONS_WIDTH))
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
    selected_index: usize,
    cx: &App,
) -> gpui::AnyElement {
    let mode = search_dropdown_mode(query);
    let has_query = !query.trim().is_empty();
    let search_for_label = mezon_i18n::t(locale, "searchMessageChannel.searchFor");
    let enter_label = mezon_i18n::t(locale, "searchMessageChannel.enter");
    let needle = autocomplete_needle(query);
    let mut item_index = 0usize;

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

    let mut sections: Vec<gpui::AnyElement> = Vec::new();

    match mode {
        SearchDropdownMode::Options => {
            sections.push(render_default_search_options(
                theme,
                locale,
                layout.clone(),
                selected_index,
                &mut item_index,
            ));
        }
        SearchDropdownMode::FromUser | SearchDropdownMode::PlainMembers => {
            let pool = mention_member_pool(cx);
            let members = filter_members_for_from(&pool, &needle, 3);
            if !members.is_empty() {
                sections.push(render_member_suggestions(
                    theme,
                    locale,
                    "searchMessageChannel.fromUser",
                    "searchMessageChannel.prefixes.from",
                    '>',
                    members,
                    layout.clone(),
                    selected_index,
                    &mut item_index,
                ));
            }
        }
        SearchDropdownMode::Mentions => {
            let pool = mention_member_pool(cx);
            let members = filter_members_for_mentions(&pool, &needle, 3);
            if !members.is_empty() {
                sections.push(render_member_suggestions(
                    theme,
                    locale,
                    "searchMessageChannel.mentionsUser",
                    "searchMessageChannel.prefixes.mentions",
                    '~',
                    members,
                    layout.clone(),
                    selected_index,
                    &mut item_index,
                ));
            }
        }
        SearchDropdownMode::Has => {
            sections.push(render_has_suggestions(
                theme,
                locale,
                &needle,
                layout.clone(),
                selected_index,
                &mut item_index,
            ));
        }
        SearchDropdownMode::Hidden => {}
    }

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
                .children(sections),
        )
        .into_any_element()
}

fn render_default_search_options(
    theme: &Theme,
    locale: &str,
    layout: WeakEntity<ChatLayout>,
    selected_index: usize,
    item_index: &mut usize,
) -> gpui::AnyElement {
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

    div()
        .mx_3()
        .mt_3()
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
                    let selected = *item_index == selected_index;
                    *item_index += 1;
                    render_prefix_option_row(theme, prefix, &content, layout.clone(), selected)
                })
                .collect::<Vec<_>>(),
        )
        .into_any_element()
}

fn render_prefix_option_row(
    theme: &Theme,
    prefix: &str,
    content: &str,
    layout: WeakEntity<ChatLayout>,
    selected: bool,
) -> gpui::AnyElement {
    let prefix_label = prefix.to_string();
    let prefix_insert = prefix.to_string();
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
        .when(selected, |el| el.bg(theme.tokens.bg_item_theme_hover))
        .when(!selected, |el| {
            el.hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if let Some(layout) = layout.upgrade() {
                layout.update(cx, |layout, cx| {
                    layout.insert_search_option_prefix(&prefix_insert, window, cx);
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
                        .child(content.to_string()),
                ),
        )
        .into_any_element()
}

fn render_member_suggestions(
    theme: &Theme,
    locale: &str,
    group_key: &'static str,
    prefix_key: &'static str,
    trigger: char,
    members: Vec<&MentionMemberRaw>,
    layout: WeakEntity<ChatLayout>,
    selected_index: usize,
    item_index: &mut usize,
) -> gpui::AnyElement {
    let group_name = mezon_i18n::t(locale, group_key).to_string();
    let prefix_label = mezon_i18n::t(locale, prefix_key).to_string();
    div()
        .mx_3()
        .mt_3()
        .border_b_1()
        .border_color(theme.tokens.border_theme_primary)
        .pb_3()
        .child(
            div().flex().items_center().pb_2().px_2().child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(group_name.to_uppercase()),
            ),
        )
        .children(
            members
                .into_iter()
                .map(|member| {
                    let selected = *item_index == selected_index;
                    *item_index += 1;
                    let layout = layout.clone();
                    let user_id = member.user_id.clone();
                    let username_for_insert = member.username.clone();
                    let username_for_label = member.username.clone();
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .when(selected, |el| el.bg(theme.tokens.bg_item_theme_hover))
                        .when(!selected, |el| {
                            el.hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                        })
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            if let Some(layout) = layout.upgrade() {
                                layout.update(cx, |layout, cx| {
                                    layout.insert_search_filter_markup(
                                        trigger,
                                        &username_for_insert,
                                        &user_id,
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
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.interactive_active)
                                        .child(format!("{prefix_label} ")),
                                )
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(username_for_label),
                                ),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_any_element()
}

fn render_has_suggestions(
    theme: &Theme,
    locale: &str,
    needle: &str,
    layout: WeakEntity<ChatLayout>,
    selected_index: usize,
    item_index: &mut usize,
) -> gpui::AnyElement {
    let group_name = mezon_i18n::t(locale, "searchMessageChannel.hasContent").to_string();
    let prefix_label = mezon_i18n::t(locale, "searchMessageChannel.prefixes.has").to_string();
    let options: Vec<(String, String)> = filtered_has_options(locale, needle)
        .into_iter()
        .map(|option| (option.clone(), has_option_label(locale, &option)))
        .collect();

    div()
        .mx_3()
        .mt_3()
        .border_b_1()
        .border_color(theme.tokens.border_theme_primary)
        .pb_3()
        .child(
            div().flex().items_center().pb_2().px_2().child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(group_name.to_uppercase()),
            ),
        )
        .children(
            options
                .into_iter()
                .map(|(option, label)| {
                    let selected = *item_index == selected_index;
                    *item_index += 1;
                    let layout = layout.clone();
                    let option_id = option;
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .when(selected, |el| el.bg(theme.tokens.bg_item_theme_hover))
                        .when(!selected, |el| {
                            el.hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                        })
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            if let Some(layout) = layout.upgrade() {
                                layout.update(cx, |layout, cx| {
                                    layout.insert_search_filter_markup(
                                        '&', &option_id, &option_id, window, cx,
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
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.interactive_active)
                                        .child(format!("{prefix_label} ")),
                                )
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(label),
                                ),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_any_element()
}

fn filtered_has_values(needle: &str) -> Vec<String> {
    let needle_lc = needle.to_lowercase();
    has_filter_options()
        .iter()
        .filter(|option| needle.is_empty() || option.contains(&needle_lc))
        .map(|option| option.to_string())
        .collect()
}

fn filtered_has_options(locale: &str, needle: &str) -> Vec<String> {
    let needle_lc = needle.to_lowercase();
    has_filter_options()
        .iter()
        .filter(|option| {
            needle.is_empty()
                || option.contains(&needle_lc)
                || has_option_label(locale, option)
                    .to_lowercase()
                    .contains(&needle_lc)
        })
        .map(|option| option.to_string())
        .collect()
}

fn has_option_label(locale: &str, option: &str) -> String {
    let key = match option {
        "image" => "searchMessageChannel.hasOptions.image",
        "video" => "searchMessageChannel.hasOptions.video",
        "link" => "searchMessageChannel.hasOptions.link",
        _ => return option.to_string(),
    };
    mezon_i18n::t(locale, key).to_string()
}

fn filter_members_for_from<'a>(
    members: &'a [MentionMemberRaw],
    needle: &str,
    limit: usize,
) -> Vec<&'a MentionMemberRaw> {
    if needle.is_empty() {
        return members.iter().take(limit).collect();
    }
    let needle_lc = needle.to_lowercase();
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    for member in members {
        if member.username_lc == needle_lc {
            exact.push(member);
        } else if member.username_lc.contains(&needle_lc) {
            partial.push(member);
        }
    }
    exact.into_iter().chain(partial).take(limit).collect()
}

fn filter_members_for_mentions<'a>(
    members: &'a [MentionMemberRaw],
    needle: &str,
    limit: usize,
) -> Vec<&'a MentionMemberRaw> {
    if needle.is_empty() {
        return members.iter().take(limit).collect();
    }
    let needle_lc = needle.to_lowercase();
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    for member in members {
        if member.username_lc == needle_lc || member.display_lc == needle_lc {
            exact.push(member);
        } else if member.username_lc.contains(&needle_lc)
            || member.display_lc.contains(&needle_lc)
        {
            partial.push(member);
        }
    }
    exact.into_iter().chain(partial).take(limit).collect()
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
    gpui::actions!(
        mezon_message_search,
        [
            OpenMessageSearch,
            SelectNextSearchOption,
            SelectPrevSearchOption
        ]
    );
    cx.bind_keys([
        gpui::KeyBinding::new("secondary-f", OpenMessageSearch, None),
        gpui::KeyBinding::new("ctrl-f", OpenMessageSearch, None),
        gpui::KeyBinding::new("escape", ::menu::Cancel, Some(KEY_CONTEXT)),
        gpui::KeyBinding::new("up", SelectPrevSearchOption, None),
        gpui::KeyBinding::new("down", SelectNextSearchOption, None),
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
    cx.on_action(|_: &SelectNextSearchOption, cx: &mut App| {
        let Some(layout) = cx
            .try_global::<GlobalChatLayout>()
            .and_then(|global| global.0.upgrade())
        else {
            return;
        };
        layout.update(cx, |layout, cx| layout.select_next_search_dropdown_item(cx));
    });
    cx.on_action(|_: &SelectPrevSearchOption, cx: &mut App| {
        let Some(layout) = cx
            .try_global::<GlobalChatLayout>()
            .and_then(|global| global.0.upgrade())
        else {
            return;
        };
        layout.update(cx, |layout, cx| layout.select_prev_search_dropdown_item(cx));
    });
}
