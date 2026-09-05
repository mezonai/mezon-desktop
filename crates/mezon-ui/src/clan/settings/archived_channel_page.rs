use gpui::{
    AnyElement, Context, Entity, FontWeight, MouseButton, Render, ScrollHandle, SharedString,
    Subscription, Task, Window, deferred, div, point, prelude::*, px,
};
use mezon_store::{ChannelList, ClanId, ClanMembersStore, Settings, UserId};
use ui::utils::{DateTimeType, format_distance_from_now};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, PaginationButton,
    Sizable, Size, h_flex, pagination_button, pagination_items, v_flex,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::text_utils::normalize_diacritics;

#[derive(Clone, PartialEq, Eq)]
struct ArchivedChannelRow {
    channel_id: i64,
    channel_label: SharedString,
    channel_private: bool,
    category_id: i64,
    creator_id: i64,
    age_restricted: bool,
    last_active_timestamp: Option<i64>,
}

const PAGE_SIZES: [usize; 3] = [10, 50, 100];

pub struct ArchivedChannelPage {
    clan_id: ClanId,
    channel_list: Entity<ChannelList>,
    settings: Entity<Settings>,
    channels: Vec<ArchivedChannelRow>,
    search: Option<Entity<InputState>>,
    search_locale: String,
    _search_sub: Option<Subscription>,
    _member_sub: Subscription,
    filtered_indices: Vec<usize>,
    rows_dirty: bool,
    list_scroll: ScrollHandle,
    page: usize,
    page_size: usize,
    page_size_picker_open: bool,
    loading: bool,
    fetch_failed: bool,
    restoring: Option<i64>,
    _fetch_task: Option<Task<()>>,
    _restore_task: Option<Task<()>>,
}

impl ArchivedChannelPage {
    pub fn new(
        clan_id: ClanId,
        channel_list: Entity<ChannelList>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        ClanMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        let member_sub = cx.observe(&ClanMembersStore::global(cx), |_, _, cx| cx.notify());
        let mut this = Self {
            clan_id,
            channel_list,
            settings,
            channels: Vec::new(),
            search: None,
            search_locale: String::new(),
            _search_sub: None,
            _member_sub: member_sub,
            filtered_indices: Vec::new(),
            rows_dirty: true,
            list_scroll: ScrollHandle::new(),
            page: 0,
            page_size: PAGE_SIZES[0],
            page_size_picker_open: false,
            loading: true,
            fetch_failed: false,
            restoring: None,
            _fetch_task: None,
            _restore_task: None,
        };
        this.fetch_archived_channels(cx);
        this
    }

    pub fn release(&mut self) {
        self._fetch_task.take();
        self._restore_task.take();
    }

    fn fetch_archived_channels(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.fetch_failed = false;
        cx.notify();

        let clan_id = self.clan_id;
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            let task = this
                .update(cx, |this, cx| {
                    this.channel_list
                        .update(cx, |store, cx| store.fetch_archived_channels(clan_id, cx))
                })
                .ok();
            let Some(task) = task else {
                return;
            };
            let fetched = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match fetched {
                    Ok(descs) => {
                        this.fetch_failed = false;
                        this.channels = descs
                            .into_iter()
                            .map(|desc| ArchivedChannelRow {
                                channel_id: desc.channel_id,
                                channel_label: desc.channel_label.into(),
                                channel_private: desc.channel_private,
                                category_id: desc.category_id,
                                creator_id: desc.creator_id,
                                age_restricted: desc.age_restricted,
                                last_active_timestamp: desc.last_active_timestamp,
                            })
                            .collect();
                        this.page = 0;
                        this.rows_dirty = true;
                        this.scroll_to_top();
                    }
                    Err(err) => {
                        tracing::error!("fetch archived channels failed: {err}");
                        this.channels.clear();
                        this.fetch_failed = true;
                    }
                }
                cx.notify();
            });
        }));
    }

    fn restore_channel(&mut self, channel_id: i64, cx: &mut Context<Self>) {
        if self.restoring.is_some() {
            return;
        }
        self.restoring = Some(channel_id);
        cx.notify();

        let clan_id = self.clan_id;
        let locale = self.settings.read(cx).language.clone();
        self._restore_task = Some(cx.spawn(async move |this, cx| {
            let task = this
                .update(cx, |this, cx| {
                    this.channel_list.update(cx, |store, cx| {
                        store.restore_archived_channel(clan_id, channel_id, cx)
                    })
                })
                .ok();
            let Some(task) = task else {
                return;
            };
            let result = task.await;
            let success = result.is_ok();
            let _ = this.update(cx, |this, cx| {
                this.restoring = None;
                if let Ok(()) = result {
                    this.channels.retain(|row| row.channel_id != channel_id);
                    this.rows_dirty = true;
                    this.scroll_to_top();
                    this.channel_list
                        .update(cx, |store, cx| store.refresh_clan(clan_id, cx));
                } else if let Err(err) = &result {
                    tracing::error!("restore archived channel failed: {err}");
                }
                cx.notify();
            });
            cx.update(|cx| {
                Shell::global(cx).update(cx, |shell, cx| {
                    if success {
                        let message =
                            mezon_i18n::t(&locale, "clanSettings.archivedChannels.restoreSuccess")
                                .to_string();
                        shell.success(message, cx);
                    } else {
                        let message =
                            mezon_i18n::t(&locale, "clanSettings.archivedChannels.restoreFailed")
                                .to_string();
                        shell.error(message, cx);
                    }
                });
            });
        }));
    }

    fn format_active_subtitle(timestamp_sec: Option<i64>, locale: &str) -> SharedString {
        let active_label =
            mezon_i18n::t(locale, "clanSettings.archivedChannels.archived").to_uppercase();
        let Some(timestamp_sec) = timestamp_sec.filter(|&t| t > 0) else {
            return active_label.into();
        };
        let now = chrono::Local::now().timestamp();
        let diff = now.saturating_sub(timestamp_sec);
        let time_ago = if diff <= 1 {
            mezon_i18n::t(locale, "common.justNow").to_string()
        } else {
            let Some(utc) = chrono::DateTime::from_timestamp(timestamp_sec, 0) else {
                return active_label.into();
            };
            let naive = utc.with_timezone(&chrono::Local).naive_local();
            format_distance_from_now(DateTimeType::Naive(naive), false, true, false)
        };
        format!("{active_label} {time_ago}").into()
    }

    fn ensure_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let placeholder =
            mezon_i18n::t(&locale, "clanSettings.archivedChannels.searchPlaceholder").to_string();
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
                .height(px(40.0))
                .radius(px(10.0))
                .padding_x(px(16.0))
                .padding_right(px(42.0))
        });
        self._search_sub = Some(cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.page = 0;
                this.page_size_picker_open = false;
                this.rows_dirty = true;
                this.scroll_to_top();
                cx.notify();
            }
        }));
        self.search = Some(input);
        self.search_locale = locale;
    }

    fn filtered_indices(&mut self, cx: &Context<Self>) -> &[usize] {
        if !self.rows_dirty {
            return &self.filtered_indices;
        }
        let query = self
            .search
            .as_ref()
            .map(|input| normalize_diacritics(input.read(cx).value().trim()))
            .unwrap_or_default();
        self.filtered_indices = self
            .channels
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                query.is_empty() || normalize_diacritics(&row.channel_label).contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        self.rows_dirty = false;
        &self.filtered_indices
    }

    fn scroll_to_top(&self) {
        self.list_scroll.set_offset(point(px(0.0), px(0.0)));
    }

    fn category_name(&self, category_id: i64, cx: &Context<Self>) -> Option<String> {
        let category_id = category_id.to_string();
        self.channel_list
            .read(cx)
            .categories_for_clan(self.clan_id)
            .iter()
            .find(|category| category.id == category_id)
            .map(|category| category.name.clone())
            .filter(|name| !name.is_empty())
    }

    fn creator_name(&self, creator_id: i64, cx: &Context<Self>) -> Option<String> {
        if creator_id <= 0 {
            return None;
        }
        ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, UserId(creator_id))
            .map(|member| member.name().to_string())
            .filter(|name| !name.is_empty())
    }

    fn metadata_chip(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
        div()
            .max_w(px(190.0))
            .px(px(8.0))
            .py(px(3.0))
            .rounded_full()
            .bg(theme.bg_hover)
            .text_xs()
            .text_color(theme.text_secondary)
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(text.into())
    }

    fn render_pagination(&self, pages: usize, cx: &mut Context<Self>) -> AnyElement {
        if pages <= 1 {
            return div().into_any_element();
        }
        let mut bar = h_flex().items_center().gap_2();
        bar = bar.child(
            pagination_button(
                "archived-channels",
                PaginationButton::Previous,
                self.page == 0,
                false,
                cx.theme(),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.page > 0 {
                    this.page -= 1;
                    this.scroll_to_top();
                    cx.notify();
                }
            })),
        );
        for item in pagination_items(self.page, pages) {
            if let Some(page) = item {
                bar = bar.child(
                    pagination_button(
                        "archived-channels",
                        PaginationButton::Page(page + 1),
                        false,
                        page == self.page,
                        cx.theme(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.page = page;
                        this.scroll_to_top();
                        cx.notify();
                    })),
                );
            } else {
                bar = bar.child(
                    div()
                        .px(px(4.0))
                        .text_color(cx.theme().text_secondary)
                        .child("…"),
                );
            }
        }
        bar.child(
            pagination_button(
                "archived-channels",
                PaginationButton::Next,
                self.page + 1 >= pages,
                false,
                cx.theme(),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.page + 1 < pages {
                    this.page += 1;
                    this.scroll_to_top();
                    cx.notify();
                }
            })),
        )
        .into_any_element()
    }

    fn page_size_control(&self, locale: &str, cx: &mut Context<Self>) -> AnyElement {
        let open = self.page_size_picker_open;
        let chevron_angle = if open { std::f32::consts::PI } else { 0.0 };
        let mut control = div().relative().w(px(68.0)).child(
            div()
                .id("archived-page-size")
                .w_full()
                .h(px(32.0))
                .px_2()
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(6.0))
                .border_1()
                .border_color(cx.theme().border)
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().bg_hover))
                .child(self.page_size.to_string())
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(px(14.0))
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
                .bottom(px(36.0))
                .left_0()
                .w(px(68.0))
                .p_1()
                .rounded(px(6.0))
                .bg(cx.theme().bg_floating)
                .border_1()
                .border_color(cx.theme().border)
                .shadow_lg()
                .occlude();
            for size in PAGE_SIZES {
                let selected = size == self.page_size;
                menu = menu.child(
                    div()
                        .id(format!("archived-page-size-{size}"))
                        .h(px(28.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(4.0))
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
        h_flex()
            .id("archived-page-size-control")
            .items_center()
            .gap_2()
            .child(mezon_i18n::t(
                locale,
                "channelSetting.table.pagination.show",
            ))
            .child(control)
            .child(mezon_i18n::t(
                locale,
                "channelSetting.table.pagination.channelOf",
            ))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.page_size_picker_open {
                    this.page_size_picker_open = false;
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn render_empty_state(locale: &str, theme: &Theme) -> impl IntoElement {
        v_flex()
            .items_center()
            .justify_center()
            .py(px(48.0))
            .child(
                div().opacity(0.6).child(
                    Icon::new(IconName::Hashtag)
                        .size(px(48.0))
                        .text_color(theme.text_primary),
                ),
            )
            .child(
                div()
                    .mt(px(12.0))
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .opacity(0.6)
                    .child(mezon_i18n::t(
                        locale,
                        "clanSettings.archivedChannels.emptyState",
                    )),
            )
    }

    fn render_fetch_error(locale: &str, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .py(px(48.0))
            .child(
                div()
                    .text_base()
                    .text_color(theme.status_dnd)
                    .text_center()
                    .child(mezon_i18n::t(
                        locale,
                        "clanSettings.archivedChannels.fetchFailed",
                    )),
            )
            .child(
                Button::new("archived-channels-retry")
                    .label(mezon_i18n::t(locale, "channelVoice.retry"))
                    .with_size(Size::Medium)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.fetch_archived_channels(cx);
                    })),
            )
    }

    fn render_channel_row(
        &self,
        index: usize,
        row: &ArchivedChannelRow,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let channel_id = row.channel_id;
        let icon = if row.channel_private {
            IconName::HashtagLocked
        } else {
            IconName::Hashtag
        };
        let subtitle = Self::format_active_subtitle(row.last_active_timestamp, locale);
        let category_name = self.category_name(row.category_id, cx);
        let creator_name = self.creator_name(row.creator_id, cx);
        let restoring = self.restoring == Some(channel_id);

        h_flex()
            .id(("archived-channel-row", index))
            .w_full()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(12.0))
            .rounded_lg()
            .bg(theme.tokens.bg_item_theme_hover)
            .shadow_sm()
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(icon)
                            .size(px(20.0))
                            .text_color(theme.tokens.bg_icon_theme),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(row.channel_label.clone()),
                    )
                    .child(
                        h_flex()
                            .mt(px(5.0))
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(theme.text_primary)
                                    .whitespace_nowrap()
                                    .child(subtitle),
                            )
                            .when_some(category_name, |el, category_name| {
                                el.child(Self::metadata_chip(
                                    format!(
                                        "{}: {category_name}",
                                        mezon_i18n::t(
                                            locale,
                                            "channelSetting.categoryManagement.category"
                                        )
                                    ),
                                    theme,
                                ))
                            })
                            .when_some(creator_name, |el, creator_name| {
                                el.child(Self::metadata_chip(
                                    format!(
                                        "{}{creator_name}",
                                        mezon_i18n::t(locale, "eventMenu.detail.createdBy")
                                    ),
                                    theme,
                                ))
                            })
                            .when(row.age_restricted, |el| {
                                el.child(Self::metadata_chip(
                                    mezon_i18n::t(locale, "ageRestricted.title"),
                                    theme,
                                ))
                            }),
                    ),
            )
            .child(
                Button::new(format!("archived-channel-restore-{channel_id}"))
                    .label(mezon_i18n::t(
                        locale,
                        "clanSettings.archivedChannels.restore",
                    ))
                    .primary()
                    .with_size(Size::Medium)
                    .disabled(restoring)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.restore_channel(channel_id, cx);
                    })),
            )
    }
}

impl Render for ArchivedChannelPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_search(window, cx);
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let filtered_indices = self.filtered_indices(cx).to_vec();
        let total = filtered_indices.len();
        let pages = total.div_ceil(self.page_size).max(1);
        self.page = self.page.min(pages - 1);
        let visible = filtered_indices
            .into_iter()
            .skip(self.page * self.page_size)
            .take(self.page_size)
            .filter_map(|index| self.channels.get(index).cloned())
            .collect::<Vec<_>>();

        v_flex()
            .h_full()
            .min_h_0()
            .w_full()
            .pt(px(8.0))
            .child(
                div()
                    .mb(px(24.0))
                    .max_w(px(672.0))
                    .text_base()
                    .text_color(theme.text_secondary)
                    .child(mezon_i18n::t(
                        &locale,
                        "clanSettings.archivedChannels.description",
                    )),
            )
            .when(!self.loading && !self.fetch_failed, |el| {
                el.child(
                    div()
                        .relative()
                        .w_full()
                        .mb(px(20.0))
                        .child(Input::new(
                            self.search.as_ref().expect("search initialized"),
                        ))
                        .child(
                            div()
                                .absolute()
                                .right(px(12.0))
                                .top_0()
                                .bottom_0()
                                .flex()
                                .items_center()
                                .child(
                                    Icon::new(IconName::Search)
                                        .size(px(18.0))
                                        .text_color(theme.text_secondary),
                                ),
                        ),
                )
            })
            .when(self.loading, |el| el.child(div().h(px(48.0))))
            .when(!self.loading && self.fetch_failed, |el| {
                el.child(Self::render_fetch_error(&locale, &theme, cx))
            })
            .when(
                !self.loading && !self.fetch_failed && self.channels.is_empty(),
                |el| el.child(Self::render_empty_state(&locale, &theme)),
            )
            .when(
                !self.loading && !self.fetch_failed && !self.channels.is_empty() && total == 0,
                |el| {
                    el.child(
                        v_flex()
                            .items_center()
                            .py(px(48.0))
                            .text_color(theme.text_secondary)
                            .child(mezon_i18n::t(
                                &locale,
                                "clanSettings.archivedChannels.noSearchResults",
                            )),
                    )
                },
            )
            .when(!self.loading && !self.fetch_failed && total > 0, |el| {
                el.child(
                    div()
                        .id("archived-channel-list-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.list_scroll)
                        .pr(px(4.0))
                        .child(
                            v_flex()
                                .gap(px(12.0))
                                .children(visible.iter().enumerate().map(|(index, row)| {
                                    self.render_channel_row(index, row, &locale, &theme, cx)
                                        .into_any_element()
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .w_full()
                        .h(px(72.0))
                        .items_center()
                        .justify_between()
                        .border_t_1()
                        .border_color(theme.border)
                        .text_sm()
                        .text_color(theme.text_secondary)
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(self.page_size_control(&locale, cx))
                                .child(total.to_string()),
                        )
                        .child(self.render_pagination(pages, cx)),
                )
            })
    }
}
