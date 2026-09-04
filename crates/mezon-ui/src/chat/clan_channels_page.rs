use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, ListAlignment, ListOffset, ListState,
    MouseButton, MouseDownEvent, Pixels, Point, Render, Subscription, WeakEntity, Window, deferred,
    div, img, list, prelude::*, px, size,
};
use mezon_store::{
    BadgeService, ChannelId, ChannelList, ChannelSetting, ChannelSettingsStore, ChannelType,
    ClanId, ClanList, ClanMembersStore, PERMISSION_ADMINISTRATOR, PERMISSION_CLAN_OWNER,
    PERMISSION_MANAGE_CHANNEL, PERMISSION_MANAGE_CLAN, PermissionStore, Settings, UserId,
    archive_allowed_by_server, archive_menu_hidden, delete_allowed_by_server,
    manage_allowed_by_server,
};
use ui::Tooltip;

use crate::app::shell::Shell;
use crate::chat::clan_management_page::management_page;
use crate::chat::message::format_channel_setting_relative_time_from_seconds;
use crate::components::compositions::channel_row::channel_type_icon;
use crate::components::primitives::{
    Avatar, ContextMenu, Icon, IconName, Input, InputEvent, InputState, context_menu_at,
};
use crate::theme::ActiveTheme;
use crate::util::text_utils::normalize_diacritics;

const PAGE_SIZES: [usize; 3] = [10, 50, 100];
const CHANNEL_ROW_HEIGHT: f32 = 60.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortField {
    Name,
    Members,
    Messages,
    LastSent,
    Creator,
}

pub struct ClanChannelsPage {
    clan_id: ClanId,
    settings: Entity<Settings>,
    search: Option<Entity<InputState>>,
    search_locale: String,
    search_sub: Option<Subscription>,
    expanded: HashSet<ChannelId>,
    page: usize,
    page_size: usize,
    page_size_picker_open: bool,
    sort_field: Option<SortField>,
    sort_descending: bool,
    cached_rows: Vec<ChannelSetting>,
    rows_dirty: bool,
    visible_row_keys: Vec<VisibleRowKey>,
    list_state: ListState,
    open_menu: Option<ChannelListMenuState>,
}

#[derive(Clone)]
struct ChannelListMenuState {
    row: ChannelSetting,
    is_thread: bool,
    position: Point<Pixels>,
}

#[derive(Clone)]
enum VisibleRow {
    Channel(ChannelSetting, bool, bool),
    NoThreads(ChannelId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VisibleRowKey {
    Channel(ChannelId),
    NoThreads(ChannelId),
}

impl VisibleRow {
    fn key(&self) -> VisibleRowKey {
        match self {
            Self::Channel(channel, _, _) => VisibleRowKey::Channel(channel.id),
            Self::NoThreads(channel_id) => VisibleRowKey::NoThreads(*channel_id),
        }
    }
}

impl ClanChannelsPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&ChannelSettingsStore::global(cx), |this, _, cx| {
            this.rows_dirty = true;
            cx.notify();
        })
        .detach();
        cx.observe(&ClanMembersStore::global(cx), |this, _, cx| {
            if this.sort_field == Some(SortField::Creator) {
                this.rows_dirty = true;
            }
            cx.notify();
        })
        .detach();
        cx.observe(&PermissionStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self {
            clan_id: ClanId(0),
            settings,
            search: None,
            search_locale: String::new(),
            search_sub: None,
            expanded: HashSet::new(),
            page: 0,
            page_size: 10,
            page_size_picker_open: false,
            sort_field: None,
            sort_descending: true,
            cached_rows: Vec::new(),
            rows_dirty: true,
            visible_row_keys: Vec::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(60.))
                .smooth_line_scroll()
                .suppress_hover_while_scrolling(),
            open_menu: None,
        }
    }

    pub fn set_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.clan_id == clan_id {
            return;
        }
        self.clan_id = clan_id;
        self.reset_search(cx);
        self.expanded.clear();
        self.visible_row_keys.clear();
        self.rows_dirty = true;
        self.page_size_picker_open = false;
        self.open_menu = None;
        ChannelSettingsStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, ChannelId(0), cx)
        });
        ClanMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        PermissionStore::global(cx)
            .update(cx, |store, cx| store.load_clan_permissions(clan_id, cx));
        cx.notify();
    }

    pub fn reset_search(&mut self, cx: &mut Context<Self>) {
        self.search = None;
        self.search_sub = None;
        self.search_locale.clear();
        self.page = 0;
        self.rows_dirty = true;
        self.page_size_picker_open = false;
        self.open_menu = None;
        self.scroll_to_top();
        cx.notify();
    }

    pub fn deactivate(&mut self, cx: &mut Context<Self>) {
        if self.clan_id.get() != 0 {
            ChannelSettingsStore::global(cx)
                .update(cx, |store, cx| store.reset_clan(self.clan_id, cx));
            self.clan_id = ClanId(0);
        }
        self.reset_search(cx);
    }

    fn ensure_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let placeholder = tr(&locale, "setting.channelSetting.searchByChannelLabel");
        if let Some(input) = &self.search {
            if locale != self.search_locale {
                input.update(cx, |input, cx| input.set_placeholder(placeholder, cx));
                self.search_locale = locale;
            }
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(32.))
                .radius(px(8.))
                .padding_x(px(16.))
                .padding_right(px(38.))
        });
        self.search_sub = Some(cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.page = 0;
                this.rows_dirty = true;
                this.scroll_to_top();
                cx.notify();
            }
        }));
        self.search = Some(input);
        self.search_locale = locale;
    }

    fn member_name(&self, user_id: UserId, cx: &Context<Self>) -> String {
        ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, user_id)
            .map(|member| member.name().to_string())
            .unwrap_or_else(|| user_id.to_string())
    }

    fn rows(&mut self, cx: &Context<Self>) -> &[ChannelSetting] {
        if !self.rows_dirty {
            return &self.cached_rows;
        }
        let query = self
            .search
            .as_ref()
            .map(|input| normalize_diacritics(input.read(cx).value().trim()))
            .unwrap_or_default();
        let store = ChannelSettingsStore::global(cx);
        let mut rows = store.read(cx).rows(self.clan_id, ChannelId(0)).to_vec();
        let channel_list = ChannelList::global(cx);
        for row in &mut rows {
            if row.label.trim().is_empty()
                && let Some(label) = channel_list
                    .read(cx)
                    .channel_display_name(self.clan_id, row.id)
            {
                row.label = label;
            }
        }
        if !query.is_empty() {
            rows.retain(|row| normalize_diacritics(&row.label).contains(&query));
        }
        if let Some(field) = self.sort_field {
            match field {
                SortField::Name => rows.sort_by_cached_key(|row| normalize_diacritics(&row.label)),
                SortField::Members => rows.sort_by_cached_key(|row| row.user_ids.len()),
                SortField::Messages => rows.sort_by_cached_key(|row| row.message_count),
                SortField::LastSent => rows.sort_by_cached_key(|row| row.last_sent_seconds),
                SortField::Creator => rows.sort_by_cached_key(|row| {
                    normalize_diacritics(&self.member_name(row.creator_id, cx))
                }),
            }
            if self.sort_descending {
                rows.reverse();
            }
        }
        self.cached_rows = rows;
        self.rows_dirty = false;
        &self.cached_rows
    }

    fn select_sort(&mut self, field: SortField) {
        if self.sort_field == Some(field) {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_field = Some(field);
            self.sort_descending = !matches!(field, SortField::Name | SortField::Creator);
        }
        self.page = 0;
        self.rows_dirty = true;
        self.scroll_to_top();
    }

    fn toggle_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if !self.expanded.remove(&channel_id) {
            self.expanded.insert(channel_id);
            ChannelSettingsStore::global(cx).update(cx, |store, cx| {
                store.ensure_loaded(self.clan_id, channel_id, cx)
            });
        }
        cx.notify();
    }

    fn header_cell(
        &self,
        label: String,
        field: SortField,
        grow: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let active = self.sort_field == Some(field);
        let direction = if active && !self.sort_descending {
            std::f32::consts::PI
        } else {
            0.
        };
        div()
            .id(format!("channel-sort-{field:?}"))
            .flex_basis(px(0.))
            .flex_grow(grow)
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .text_color(if active {
                cx.theme().text_primary
            } else {
                cx.theme().text_secondary
            })
            .child(label)
            .child(
                Icon::new(IconName::ArrowDown)
                    .size(px(13.))
                    .text_color(if active {
                        cx.theme().text_primary
                    } else {
                        cx.theme().text_secondary
                    })
                    .with_transformation(gpui::Transformation::rotate(gpui::radians(direction))),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_sort(field);
                cx.notify();
            }))
            .into_any_element()
    }

    fn table_header(&self, locale: &str, cx: &Context<Self>) -> AnyElement {
        div()
            .h(px(48.))
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_size(px(12.))
            .font_weight(FontWeight::BOLD)
            .child(self.header_cell(key(locale, "name"), SortField::Name, 3., cx))
            .child(self.header_cell(key(locale, "members"), SortField::Members, 2., cx))
            .child(self.header_cell(key(locale, "messagesCount"), SortField::Messages, 2., cx))
            .child(self.header_cell(key(locale, "lastSent"), SortField::LastSent, 2., cx))
            .child(
                div()
                    .flex_basis(px(0.))
                    .flex_grow(1.)
                    .min_w_0()
                    .flex()
                    .justify_center()
                    .child(self.header_cell(key(locale, "creator"), SortField::Creator, 1., cx)),
            )
            .into_any_element()
    }

    fn member_avatar(
        &self,
        user_id: UserId,
        size: f32,
        tooltip: bool,
        slot_id: String,
        cx: &Context<Self>,
    ) -> AnyElement {
        let members = ClanMembersStore::global(cx);
        let member = members.read(cx).member(self.clan_id, user_id).cloned();
        let name = member
            .as_ref()
            .map(|member| member.name().to_string())
            .unwrap_or_else(|| user_id.to_string());
        let avatar = member
            .as_ref()
            .map(|member| member.avatar().to_string())
            .unwrap_or_default();
        div()
            .id(format!("channel-avatar-{slot_id}-{}", user_id.get()))
            .when(tooltip, |element| {
                element.tooltip(Tooltip::text(name.clone()))
            })
            .child(Avatar::new().src(avatar).name(name).size_px(px(size)))
            .into_any_element()
    }

    fn members_cell(&self, row: &ChannelSetting, locale: &str, cx: &Context<Self>) -> AnyElement {
        if !row.private && row.parent_id.get() == 0 {
            return div()
                .italic()
                .text_size(px(12.))
                .child(tr(locale, "channelSetting.table.members.allMembers"))
                .into_any_element();
        }
        let mut avatars = div().flex().items_center();
        for (index, user_id) in row.user_ids.iter().take(3).enumerate() {
            avatars = avatars.child(div().when(index > 0, |element| element.ml(px(-6.))).child(
                self.member_avatar(
                    *user_id,
                    24.,
                    true,
                    format!("{}-member-{index}", row.id.get()),
                    cx,
                ),
            ));
        }
        if row.user_ids.len() > 3 {
            avatars = avatars.child(format!("+{}", row.user_ids.len() - 3));
        }
        avatars.into_any_element()
    }

    fn render_row(
        &self,
        row: &ChannelSetting,
        is_thread: bool,
        show_separator: bool,
        locale: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let expanded = self.expanded.contains(&row.id);
        let channel_id = row.id;
        let menu_row = row.clone();
        let page = cx.entity().downgrade();
        let channel_type = ChannelType::from_raw(row.channel_type.max(0) as u32);
        let can_expand = !is_thread && !matches!(channel_type, ChannelType::Voice);
        let relative = format_channel_setting_relative_time_from_seconds(
            i64::from(row.last_sent_seconds),
            locale,
            chrono::Local::now(),
        );
        let channel_icon = if matches!(channel_type, ChannelType::Thread) && row.private {
            div()
                .relative()
                .flex_shrink_0()
                .size(px(if is_thread { 20. } else { 22. }))
                .child(
                    Icon::new(IconName::ThreadIcon)
                        .size(px(if is_thread { 20. } else { 22. }))
                        .text_color(cx.theme().text_secondary),
                )
                .child(
                    div().absolute().inset_0().child(
                        Icon::new(IconName::HashtagLock)
                            .size(px(if is_thread { 22. } else { 24. }))
                            .text_color(cx.theme().text_primary),
                    ),
                )
                .into_any_element()
        } else {
            Icon::new(channel_type_icon(channel_type, row.private))
                .size(px(if is_thread { 20. } else { 22. }))
                .text_color(cx.theme().text_secondary)
                .into_any_element()
        };
        div()
            .id(format!("channel-setting-row-{}", row.id.get()))
            .w_full()
            .min_w_0()
            .h(px(60.))
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .when(show_separator, |element| {
                element.border_b_1().border_color(cx.theme().border)
            })
            .hover(|style| style.bg(cx.theme().bg_hover))
            .on_mouse_down(
                MouseButton::Right,
                move |event: &MouseDownEvent, _window, cx| {
                    let _ = page.update(cx, |this, cx| {
                        this.open_menu = Some(ChannelListMenuState {
                            row: menu_row.clone(),
                            is_thread,
                            position: event.position,
                        });
                        cx.notify();
                    });
                },
            )
            .child(
                div()
                    .flex_basis(px(0.))
                    .flex_grow(3.)
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(is_thread, |element| element.pl(px(36.)))
                    .child(channel_icon)
                    .child(div().flex_1().min_w_0().truncate().child(row.label.clone())),
            )
            .child(
                div()
                    .flex_basis(px(0.))
                    .flex_grow(2.)
                    .min_w_0()
                    .child(self.members_cell(row, locale, cx)),
            )
            .child(
                div()
                    .flex_basis(px(0.))
                    .flex_grow(2.)
                    .font_weight(FontWeight::BOLD)
                    .child(compact_number(row.message_count)),
            )
            .child(
                div()
                    .flex_basis(px(0.))
                    .flex_grow(2.)
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(row.last_sent_seconds != 0, |element| {
                        element.child(if row.last_sender_id.get() != 0 {
                            self.member_avatar(
                                row.last_sender_id,
                                24.,
                                true,
                                format!("{}-last-sender", row.id.get()),
                                cx,
                            )
                        } else {
                            img("icons/avatar-user.svg")
                                .size(px(24.))
                                .flex_none()
                                .into_any_element()
                        })
                    })
                    .child(relative),
            )
            .child(
                div()
                    .flex_basis(px(0.))
                    .flex_grow(1.)
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.member_avatar(
                        row.creator_id,
                        30.,
                        true,
                        format!("{}-creator", row.id.get()),
                        cx,
                    ))
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .id(format!("expand-channel-{}", row.id.get()))
                            .w(px(24.))
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(can_expand, |element| {
                                element
                                    .cursor_pointer()
                                    .child(
                                        Icon::new(if expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .size(px(18.))
                                        .text_color(cx.theme().text_secondary),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_channel(channel_id, cx)
                                    }))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn flattened_rows(&self, parents: &[ChannelSetting], cx: &Context<Self>) -> Vec<VisibleRow> {
        let settings_store = ChannelSettingsStore::global(cx);
        let channel_list = ChannelList::global(cx);
        let mut result = Vec::new();
        for parent in parents {
            let expanded = self.expanded.contains(&parent.id);
            result.push(VisibleRow::Channel(parent.clone(), false, !expanded));
            if !expanded {
                continue;
            }
            let store = settings_store.read(cx);
            let threads = store.rows(self.clan_id, parent.id);
            if threads.is_empty() {
                if !store.is_loading(self.clan_id, parent.id) {
                    result.push(VisibleRow::NoThreads(parent.id));
                }
                continue;
            }
            let thread_count = threads.len();
            for (index, thread) in threads.iter().enumerate() {
                let mut thread = thread.clone();
                if thread.label.trim().is_empty()
                    && let Some(label) = channel_list
                        .read(cx)
                        .channel_display_name(self.clan_id, thread.id)
                {
                    thread.label = label;
                }
                result.push(VisibleRow::Channel(thread, true, index + 1 == thread_count));
            }
        }
        result
    }

    fn render_visible_row(&self, row: &VisibleRow, locale: &str, cx: &Context<Self>) -> AnyElement {
        match row {
            VisibleRow::Channel(channel, is_thread, show_separator) => {
                self.render_row(channel, *is_thread, *show_separator, locale, cx)
            }
            VisibleRow::NoThreads(_) => div()
                .w_full()
                .min_w_0()
                .h(px(60.))
                .flex()
                .items_center()
                .pl(px(52.))
                .border_b_1()
                .border_color(cx.theme().border)
                .text_color(cx.theme().text_primary)
                .child(tr(locale, "channelSetting.table.threads.noThreads"))
                .into_any_element(),
        }
    }

    fn page_size_control(&self, locale: &str, cx: &Context<Self>) -> AnyElement {
        let open = self.page_size_picker_open;
        let chevron_angle = if open { std::f32::consts::PI } else { 0. };
        let mut control = div().relative().w(px(68.)).child(
            div()
                .id("channel-page-size")
                .w_full()
                .h(px(32.))
                .px_2()
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(6.))
                .border_1()
                .border_color(cx.theme().border)
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().bg_hover))
                .child(self.page_size.to_string())
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(px(14.))
                        .text_color(cx.theme().text_secondary)
                        .with_transformation(gpui::Transformation::rotate(gpui::radians(
                            chevron_angle,
                        ))),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page_size_picker_open = !open;
                    cx.notify();
                })),
        );
        if open {
            let mut menu = div()
                .absolute()
                .bottom(px(36.))
                .left_0()
                .w(px(68.))
                .p_1()
                .rounded(px(6.))
                .bg(cx.theme().bg_floating)
                .border_1()
                .border_color(cx.theme().border)
                .shadow_lg()
                .occlude();
            for size in PAGE_SIZES {
                let selected = size == self.page_size;
                menu = menu.child(
                    div()
                        .id(format!("channel-page-size-{size}"))
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(selected, |style| style.bg(cx.theme().bg_hover))
                        .hover(|style| style.bg(cx.theme().bg_hover))
                        .child(size.to_string())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.page_size = size;
                                this.page = 0;
                                this.page_size_picker_open = false;
                                this.scroll_to_top();
                                cx.notify();
                            }),
                        ),
                );
            }
            control = control.child(deferred(menu));
        }
        div()
            .id("channel-page-size-control")
            .flex()
            .items_center()
            .gap_2()
            .child(tr(locale, "channelSetting.table.pagination.show"))
            .child(control)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.page_size_picker_open {
                    this.page_size_picker_open = false;
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn pagination(&self, pages: usize, cx: &Context<Self>) -> AnyElement {
        if pages <= 1 {
            return div().into_any_element();
        }
        let mut bar = div().flex().items_center().gap_2();
        bar = bar.child(
            page_button("‹", self.page == 0, false, cx).on_click(cx.listener(|this, _, _, cx| {
                if this.page > 0 {
                    this.page -= 1;
                    this.scroll_to_top();
                    cx.notify();
                }
            })),
        );
        for page in pagination_items(self.page, pages) {
            if let Some(page) = page {
                bar = bar.child(
                    page_button(&(page + 1).to_string(), false, page == self.page, cx).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.page = page;
                            this.scroll_to_top();
                            cx.notify();
                        }),
                    ),
                );
            } else {
                bar = bar.child(div().px_2().child("…"));
            }
        }
        bar.child(
            page_button("›", self.page + 1 >= pages, false, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    if this.page + 1 < pages {
                        this.page += 1;
                        this.scroll_to_top();
                        cx.notify();
                    }
                },
            )),
        )
        .into_any_element()
    }

    fn scroll_to_top(&self) {
        self.list_state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
    }
}

impl Render for ClanChannelsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_search(window, cx);
        let locale = self.settings.read(cx).language.clone();
        let can_view = PermissionStore::global(cx).read(cx).check_permission(
            self.clan_id,
            PERMISSION_ADMINISTRATOR,
            cx,
        );
        if !can_view {
            return management_page(
                tr(&locale, "channelTopbar.pageTitle.channels"),
                div().into_any_element(),
                cx.theme(),
            );
        }
        let has_search_query = self
            .search
            .as_ref()
            .is_some_and(|input| !input.read(cx).value().trim().is_empty());
        let total = self.rows(cx).len();
        let pages = total.div_ceil(self.page_size).max(1);
        self.page = self.page.min(pages - 1);
        let page_start = self.page * self.page_size;
        let page_size = self.page_size;
        let parents = self
            .rows(cx)
            .iter()
            .skip(page_start)
            .take(page_size)
            .cloned()
            .collect::<Vec<_>>();
        let visible = Arc::new(self.flattened_rows(&parents, cx));
        let visible_row_keys = visible.iter().map(VisibleRow::key).collect::<Vec<_>>();
        if self.visible_row_keys != visible_row_keys {
            let (old_range, new_count) = changed_range(&self.visible_row_keys, &visible_row_keys);
            self.list_state.splice_with_size_hint(
                old_range,
                new_count,
                size(px(0.), px(CHANNEL_ROW_HEIGHT)),
            );
            self.visible_row_keys = visible_row_keys;
        }
        let entity = cx.entity();
        let locale_for_list = Arc::<str>::from(locale.clone());
        let rows_for_list = visible.clone();
        let channel_list = if has_search_query && total == 0 {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().text_secondary)
                .child(tr(&locale, "setting.channelSetting.noSearchResults"))
                .into_any_element()
        } else {
            list(self.list_state.clone(), move |index, _window, cx| {
                entity.update(cx, |this, cx| {
                    rows_for_list
                        .get(index)
                        .map(|row| this.render_visible_row(row, &locale_for_list, cx))
                        .unwrap_or_else(|| div().into_any_element())
                })
            })
            .size_full()
            .into_any_element()
        };
        let search = div()
            .relative()
            .w(px(500.))
            .max_w_full()
            .child(Input::new(
                self.search.as_ref().expect("search initialized"),
            ))
            .child(
                div()
                    .absolute()
                    .right(px(10.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(18.))
                            .text_color(cx.theme().text_secondary),
                    ),
            )
            .on_mouse_down_out(|_, window, _| window.blur());
        let footer = div()
            .h(px(72.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.page_size_control(&locale, cx))
                    .child(format!(
                        "{} {}",
                        tr(&locale, "channelSetting.table.pagination.channelOf"),
                        total
                    )),
            )
            .child(self.pagination(pages, cx));
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .p_4()
            .text_color(cx.theme().text_secondary)
            .child(
                div()
                    .h(px(50.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(tr(&locale, "setting.channelSetting.recentChannels"))
                    .child(search),
            )
            .child(self.table_header(&locale, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .w_full()
                    .child(channel_list),
            )
            .child(footer)
            .into_any_element();
        let page = cx.entity().downgrade();
        let menu_overlay = self.open_menu.clone().map(|state| {
            let position = state.position;
            let menu = build_channel_list_menu(page, self.clan_id, state, locale.clone(), cx);
            (position, menu)
        });
        management_page(
            tr(&locale, "channelTopbar.pageTitle.channels"),
            body,
            cx.theme(),
        )
        .when_some(menu_overlay, |page, (position, menu)| {
            page.child(context_menu_at(position, menu))
        })
    }
}

fn build_channel_list_menu(
    page: WeakEntity<ClanChannelsPage>,
    clan_id: ClanId,
    state: ChannelListMenuState,
    locale: String,
    cx: &App,
) -> ContextMenu {
    let channel_id = state.row.id;
    let channel_type = ChannelType::from_raw(state.row.channel_type.max(0) as u32);
    let is_thread = state.is_thread;
    let is_welcome = ClanList::global(cx).read(cx).welcome_channel_id(clan_id) == Some(channel_id);
    let current_user_id =
        BadgeService::try_global(cx).and_then(|badges| badges.read(cx).current_user_id(cx));
    let is_creator = current_user_id == Some(state.row.creator_id);
    let permissions = PermissionStore::try_global(cx);
    let has_permission = |permission| {
        permissions
            .as_ref()
            .is_some_and(|permissions| permissions.read(cx).check(clan_id, None, permission, cx))
    };
    let has_owner = has_permission(PERMISSION_CLAN_OWNER);
    let has_admin = has_permission(PERMISSION_ADMINISTRATOR);
    let has_manage_clan = has_permission(PERMISSION_MANAGE_CLAN);
    let has_manage_channel = has_permission(PERMISSION_MANAGE_CHANNEL);
    let can_archive = state.row.active != 0
        && !archive_menu_hidden(channel_type, is_welcome)
        && archive_allowed_by_server(
            is_thread,
            is_creator,
            has_owner,
            has_admin,
            has_manage_clan,
            has_manage_channel,
        );
    let can_edit = manage_allowed_by_server(
        is_creator,
        has_owner,
        has_admin,
        has_manage_clan,
        has_manage_channel,
    );
    let can_delete = !is_welcome
        && delete_allowed_by_server(
            is_creator,
            has_owner,
            has_admin,
            has_manage_clan,
            has_manage_channel,
        );
    let dismiss = page.clone();
    let mut menu = ContextMenu::new().on_dismiss(move |_window, cx| {
        let _ = dismiss.update(cx, |this, cx| {
            this.open_menu = None;
            cx.notify();
        });
    });
    if can_archive {
        let locale = locale.clone();
        menu = menu.item(
            tr(
                &locale,
                if is_thread {
                    "channelMenu.menu.notification.archiveThread"
                } else {
                    "channelMenu.menu.notification.archiveChannel"
                },
            ),
            move |window, cx| {
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.confirm_archive_channel(
                        clan_id, channel_id, is_thread, &locale, window, cx,
                    )
                });
            },
        );
    }
    if can_edit {
        let label = tr(
            &locale,
            if is_thread {
                "channelMenu.menu.manageThreadMenu.editThread"
            } else {
                "channelMenu.menu.organizationMenu.edit"
            },
        );
        menu = menu.item(label, move |_window, cx| {
            crate::router::navigate(
                cx,
                crate::router::Route::ChannelSettings {
                    clan_id,
                    channel_id,
                    tab: crate::chat::channel_settings::ChannelSettingsTab::Overview,
                },
            );
        });
    }
    if can_delete {
        let locale_for_delete = locale.clone();
        let label = tr(
            &locale,
            if is_thread {
                "channelMenu.menu.manageThreadMenu.deleteThread"
            } else {
                "channelMenu.menu.organizationMenu.deleteChannel"
            },
        );
        menu = menu.danger_item(label, move |window, cx| {
            Shell::global(cx).update(cx, |shell, cx| {
                if is_thread {
                    shell.confirm_delete_thread(
                        clan_id,
                        channel_id,
                        &locale_for_delete,
                        window,
                        cx,
                    );
                } else {
                    shell.confirm_delete_channel(
                        clan_id,
                        channel_id,
                        &locale_for_delete,
                        window,
                        cx,
                    );
                }
            });
        });
    }
    menu
}

fn tr(locale: &str, key: &'static str) -> String {
    mezon_i18n::t(locale, key).to_string()
}

fn key(locale: &str, suffix: &str) -> String {
    let key = match suffix {
        "name" => "channelSetting.table.columnHeaders.name",
        "members" => "channelSetting.table.columnHeaders.members",
        "messagesCount" => "channelSetting.table.columnHeaders.messagesCount",
        "lastSent" => "channelSetting.table.columnHeaders.lastSent",
        "creator" => "channelSetting.table.columnHeaders.creator",
        _ => "channelSetting.table.columnHeaders.name",
    };
    tr(locale, key).to_uppercase()
}

fn compact_number(value: i64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.)
    } else {
        value.to_string()
    }
}

fn changed_range<T: PartialEq>(old: &[T], new: &[T]) -> (Range<usize>, usize) {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(old_item, new_item)| old_item == new_item)
        .count();
    let max_suffix = old.len().min(new.len()).saturating_sub(prefix);
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(old_item, new_item)| old_item == new_item)
        .count();

    (prefix..old.len() - suffix, new.len() - prefix - suffix)
}

fn pagination_items(current: usize, pages: usize) -> Vec<Option<usize>> {
    if pages <= 6 {
        return (0..pages).map(Some).collect();
    }
    if current <= 2 {
        let mut items = (0..5).map(Some).collect::<Vec<_>>();
        items.push(None);
        items.push(Some(pages - 1));
        return items;
    }
    if current >= pages - 3 {
        let mut items = vec![Some(0), None];
        items.extend(((pages - 6)..pages).map(Some));
        return items;
    }
    vec![
        Some(0),
        None,
        Some(current - 1),
        Some(current),
        Some(current + 1),
        None,
        Some(pages - 1),
    ]
}

fn page_button(
    label: &str,
    disabled: bool,
    selected: bool,
    cx: &Context<ClanChannelsPage>,
) -> gpui::Stateful<gpui::Div> {
    let is_arrow = label.parse::<usize>().is_err();
    let is_left = label == "‹" || label == "previous";
    div()
        .id(format!("channel-page-{label}-{selected}-{disabled}"))
        .w(px(40.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .border_color(if selected {
            cx.theme().text_primary
        } else {
            cx.theme().border
        })
        .bg(if disabled {
            cx.theme().brand
        } else if selected {
            cx.theme().tokens.bg_active_button
        } else {
            cx.theme().brand
        })
        .text_color(if is_arrow {
            gpui::white()
        } else {
            gpui::Hsla::from(cx.theme().text_primary)
        })
        .when(disabled, |element| element.opacity(0.5))
        .when(!disabled, |element| element.cursor_pointer())
        .when(is_arrow, |element| {
            element.child(
                Icon::new(IconName::ArrowRight)
                    .size(px(20.))
                    .text_color(gpui::white())
                    .when(is_left, |icon| {
                        icon.with_transformation(gpui::Transformation::rotate(gpui::radians(
                            std::f32::consts::PI,
                        )))
                    }),
            )
        })
        .when(!is_arrow, |element| element.child(label.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_matches_member_list_edges() {
        assert_eq!(
            pagination_items(5, 6),
            vec![Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]
        );
        assert_eq!(
            pagination_items(0, 10),
            vec![Some(0), Some(1), Some(2), Some(3), Some(4), None, Some(9)]
        );
        assert_eq!(
            pagination_items(9, 10),
            vec![
                Some(0),
                None,
                Some(4),
                Some(5),
                Some(6),
                Some(7),
                Some(8),
                Some(9)
            ]
        );
    }

    #[test]
    fn expanding_channel_only_splices_inserted_threads() {
        let old = [1, 2, 3, 4];
        let expanded = [1, 2, 20, 21, 3, 4];

        assert_eq!(changed_range(&old, &expanded), (2..2, 2));
        assert_eq!(changed_range(&expanded, &old), (2..4, 0));
    }

    #[test]
    fn search_normalization_removes_diacritics() {
        assert_eq!(normalize_diacritics("Hiền"), "hien");
        assert_eq!(normalize_diacritics("Đà Nẵng"), "da nang");
    }
}
