use std::sync::Arc;

use crate::chat::clan_management_page::{management_page, section_toolbar};
use crate::components::primitives::{Avatar, Icon, IconName, Input, InputEvent, InputState};
use crate::theme::ActiveTheme;
use gpui::{
    AnyElement, Context, Entity, FontWeight, Hsla, ListAlignment, ListOffset, ListState,
    MouseButton, Render, Subscription, Window, deferred, div, img, list, prelude::*, px, size,
};
use mezon_store::{ClanId, ClanMembersStore, Role, RoleId, RolesStore, Settings, UserId};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const PAGE_SIZES: [usize; 3] = [10, 50, 100];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemberSortField {
    Name,
    MemberSince,
    JoinedMezon,
    Roles,
}

pub struct ClanMembersPage {
    clan_id: ClanId,
    settings: Entity<Settings>,
    search_locale: String,
    search: Option<Entity<InputState>>,
    search_sub: Option<Subscription>,
    page: usize,
    page_size: usize,
    page_size_picker_open: bool,
    sort_field: MemberSortField,
    sort_descending: bool,
    cached_rows: Vec<MemberRow>,
    rows_dirty: bool,
    list_state: ListState,
}

#[derive(Clone)]
struct MemberRow {
    id: UserId,
    name: String,
    username: String,
    avatar: String,
    member_since: u32,
    joined_mezon: u32,
    role_ids: Vec<RoleId>,
    role_count: usize,
}

struct ExtraRolesTooltip {
    roles: Vec<Role>,
}

impl Render for ExtraRolesTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut roles = div().flex().flex_col().items_start().gap_1();
        for role in &self.roles {
            roles = roles.child(role_badge(role, true, cx.theme()));
        }
        roles
    }
}

impl ClanMembersPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&ClanMembersStore::global(cx), |this, _, cx| {
            this.rows_dirty = true;
            cx.notify();
        })
        .detach();
        cx.observe(&RolesStore::global(cx), |this, _, cx| {
            this.rows_dirty = true;
            cx.notify();
        })
        .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self {
            clan_id: ClanId(0),
            settings,
            search_locale: String::new(),
            search: None,
            search_sub: None,
            page: 0,
            page_size: PAGE_SIZES[0],
            page_size_picker_open: false,
            sort_field: MemberSortField::MemberSince,
            sort_descending: true,
            cached_rows: Vec::new(),
            rows_dirty: true,
            list_state: ListState::new(0, ListAlignment::Top, px(240.)),
        }
    }

    pub fn set_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.clan_id == clan_id {
            return;
        }
        self.clan_id = clan_id;
        self.page = 0;
        self.rows_dirty = true;
        self.page_size_picker_open = false;
        ClanMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        RolesStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        cx.notify();
    }

    fn ensure_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        if let Some(search) = &self.search {
            if self.search_locale != locale {
                let placeholder = tr(&locale, "memberTable.topBar.searchPlaceholder");
                search.update(cx, |input, cx| input.set_placeholder(placeholder, cx));
                self.search_locale = locale;
            }
            return;
        }
        let placeholder = tr(&locale, "memberTable.topBar.searchPlaceholder");
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

    fn rows(&mut self, cx: &Context<Self>) -> &[MemberRow] {
        if !self.rows_dirty {
            return &self.cached_rows;
        }
        let query = self
            .search
            .as_ref()
            .map(|input| normalize_search(input.read(cx).value().trim()))
            .unwrap_or_default();
        let mut rows: Vec<_> = ClanMembersStore::global(cx)
            .read(cx)
            .members(self.clan_id)
            .into_iter()
            .filter(|member| {
                query.is_empty()
                    || normalize_search(&member.user.username).contains(&query)
                    || normalize_search(&member.user.display_name).contains(&query)
                    || normalize_search(&member.clan_nick).contains(&query)
            })
            .map(|member| {
                let role_count = self.visible_roles(&member.role_ids, cx).len();
                MemberRow {
                    id: member.id(),
                    name: member.name().to_string(),
                    username: member.user.username.clone(),
                    avatar: member.avatar().to_string(),
                    member_since: member.user.join_time_seconds,
                    joined_mezon: member.user.create_time_seconds,
                    role_ids: member.role_ids.clone(),
                    role_count,
                }
            })
            .collect();
        match self.sort_field {
            MemberSortField::Name => {
                rows.sort_by_cached_key(|row| normalize_search(&row.name));
                if self.sort_descending {
                    rows.reverse();
                }
            }
            MemberSortField::MemberSince => rows.sort_by_cached_key(|row| {
                (
                    timestamp_sort_key(row.member_since, self.sort_descending),
                    normalize_search(&row.name),
                )
            }),
            MemberSortField::JoinedMezon => rows.sort_by_cached_key(|row| {
                (
                    timestamp_sort_key(row.joined_mezon, self.sort_descending),
                    normalize_search(&row.name),
                )
            }),
            MemberSortField::Roles => rows.sort_by_cached_key(|row| {
                let count = if self.sort_descending {
                    usize::MAX - row.role_count
                } else {
                    row.role_count
                };
                (count, normalize_search(&row.name))
            }),
        }
        self.cached_rows = rows;
        self.rows_dirty = false;
        &self.cached_rows
    }

    fn select_sort(&mut self, field: MemberSortField) {
        if self.sort_field == field {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_field = field;
            self.sort_descending = !matches!(field, MemberSortField::Name);
        }
        self.page = 0;
        self.rows_dirty = true;
        self.scroll_to_top();
    }

    fn format_date(seconds: u32) -> String {
        if seconds == 0 {
            return "-".into();
        }
        chrono::DateTime::from_timestamp(i64::from(seconds), 0)
            .map(|date| date.format("%b %d, %Y").to_string())
            .unwrap_or_else(|| "-".into())
    }

    fn visible_roles(&self, role_ids: &[RoleId], cx: &Context<Self>) -> Vec<Role> {
        let mut roles = RolesStore::global(cx)
            .read(cx)
            .roles_for(self.clan_id, role_ids)
            .into_iter()
            .filter(|role| !is_everyone(role))
            .collect::<Vec<_>>();
        roles.sort_by_key(|role| role.order);
        roles
    }

    fn role_cell(&self, row: &MemberRow, cx: &Context<Self>) -> AnyElement {
        let roles = self.visible_roles(&row.role_ids, cx);
        let extra = roles.len().saturating_sub(1);
        let user_id = row.id;
        let mut cell = div().relative().flex().items_center().gap_2().min_w_0();

        if let Some(role) = roles.first() {
            cell = cell.child(role_badge(role, false, cx.theme()));
        } else {
            cell = cell.child(div().text_color(cx.theme().text_secondary).child("-"));
        }

        if extra > 0 {
            let tooltip_roles = roles.iter().skip(1).cloned().collect::<Vec<_>>();
            let extra_roles = div()
                .id(format!("extra-roles-{}", user_id.get()))
                .relative()
                .cursor_default()
                .text_size(px(12.))
                .px_1()
                .child(format!("+{extra}"))
                .hoverable_tooltip(move |_window, cx| {
                    cx.new(|_| ExtraRolesTooltip {
                        roles: tooltip_roles.clone(),
                    })
                    .into()
                });
            cell = cell.child(extra_roles);
        }

        cell.into_any_element()
    }

    fn render_member_row(
        &self,
        row: &MemberRow,
        fill_available_height: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let roles = self.visible_roles(&row.role_ids, cx);
        let name_color = roles
            .first()
            .and_then(|role| parse_hex_color(&role.color))
            .map(Hsla::from)
            .unwrap_or_else(|| Hsla::from(theme.text_secondary));
        let subtitle = row.username.clone();
        table_row(theme, false, fill_available_height)
            .id(format!("member-row-{}", row.id.get()))
            .hover(|style| style.bg(theme.bg_hover))
            .child(name_column(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Avatar::new()
                            .src(row.avatar.clone())
                            .name(row.name.clone())
                            .size_px(px(36.)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .text_color(name_color)
                                    .child(row.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.text_secondary)
                                    .child(subtitle),
                            ),
                    )
                    .into_any_element(),
            ))
            .child(weighted_column(
                date_cell(Self::format_date(row.member_since), theme),
                1.,
                true,
            ))
            .child(weighted_column(
                date_cell(Self::format_date(row.joined_mezon), theme),
                1.,
                true,
            ))
            .child(weighted_column(self.role_cell(row, cx), 2., true))
            .into_any_element()
    }

    fn render_sort_header(
        &self,
        label: String,
        field: MemberSortField,
        centered: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let active = self.sort_field == field;
        let direction = if active && !self.sort_descending {
            std::f32::consts::PI
        } else {
            0.
        };
        div()
            .id(format!("member-sort-{field:?}"))
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .when(centered, |element| element.justify_center())
            .text_color(if active {
                cx.theme().text_primary
            } else {
                cx.theme().text_secondary
            })
            .child(label)
            .child(
                Icon::new(IconName::ArrowDown)
                    .size(px(14.))
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

    fn render_header(&self, locale: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        table_row(theme, true, false)
            .child(name_column(self.render_sort_header(
                tr(locale, "memberTable.headers.name").to_uppercase(),
                MemberSortField::Name,
                false,
                cx,
            )))
            .child(weighted_column(
                self.render_sort_header(
                    tr(locale, "memberTable.headers.memberSince").to_uppercase(),
                    MemberSortField::MemberSince,
                    true,
                    cx,
                ),
                1.,
                true,
            ))
            .child(weighted_column(
                self.render_sort_header(
                    tr(locale, "memberTable.headers.joinedMezon").to_uppercase(),
                    MemberSortField::JoinedMezon,
                    true,
                    cx,
                ),
                1.,
                true,
            ))
            .child(weighted_column(
                self.render_sort_header(
                    tr(locale, "memberTable.headers.roles").to_uppercase(),
                    MemberSortField::Roles,
                    true,
                    cx,
                ),
                2.,
                true,
            ))
            .into_any_element()
    }

    fn render_toolbar(&self, locale: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let controls = div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .id("member-search-field")
                    .relative()
                    .w(px(450.))
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
                                    .text_color(theme.text_secondary),
                            ),
                    )
                    .on_mouse_down_out(|_, window, _| window.blur()),
            )
            .child(
                div()
                    .id("sort-members")
                    .cursor_pointer()
                    .px_3()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .bg(theme.brand)
                    .text_color(gpui::white())
                    .gap_1()
                    .child(
                        Icon::new(IconName::ConvertAccount)
                            .size(px(18.))
                            .text_color(gpui::white())
                            .with_transformation(gpui::Transformation::rotate(gpui::radians(
                                std::f32::consts::FRAC_PI_2,
                            ))),
                    )
                    .child(tr(locale, "memberTable.topBar.sort"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.select_sort(MemberSortField::MemberSince);
                        cx.notify();
                    })),
            )
            .into_any_element();
        section_toolbar(
            tr(locale, "memberTable.topBar.recentMembers"),
            controls,
            theme,
        )
        .w_full()
        .into_any_element()
    }

    fn render_footer(
        &self,
        total: usize,
        pages: usize,
        locale: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        div()
            .w_full()
            .h(px(72.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.page_size_control(cx))
                    .text_color(theme.text_secondary)
                    .child(
                        tr(locale, "memberPage.membersOf").replace("{{count}}", &total.to_string()),
                    ),
            )
            .child(self.pagination(pages, cx))
            .into_any_element()
    }

    fn page_size_control(&self, cx: &Context<Self>) -> AnyElement {
        let open = self.page_size_picker_open;
        let locale = &self.settings.read(cx).language;
        let chevron_angle = if open { std::f32::consts::PI } else { 0. };
        let mut control = div().relative().w(px(68.)).child(
            div()
                .id("member-page-size")
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
                        .id(format!("member-page-size-{size}"))
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
            .id("member-page-size-control")
            .flex()
            .items_center()
            .gap_2()
            .child(tr(locale, "memberPage.show"))
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
        let current = self.page;
        let mut bar = div().flex().items_center().gap_2();
        bar = bar.child(
            page_arrow(true, "previous", current == 0, cx.theme()).on_click(cx.listener(
                |this, _, _, cx| {
                    if this.page > 0 {
                        this.page -= 1;
                        this.scroll_to_top();
                        cx.notify();
                    }
                },
            )),
        );
        for item in pagination_items(current, pages) {
            match item {
                Some(page) => {
                    let selected = page == current;
                    bar = bar.child(page_number(page + 1, selected, cx.theme()).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.page = page;
                            this.scroll_to_top();
                            cx.notify();
                        }),
                    ));
                }
                None => bar = bar.child(div().px_2().text_color(cx.theme().text_muted).child("…")),
            }
        }
        bar.child(
            page_arrow(false, "next", current + 1 >= pages, cx.theme()).on_click(cx.listener(
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

impl Render for ClanMembersPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_search(window, cx);
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let total = self.rows(cx).len();
        let pages = total.div_ceil(self.page_size).max(1);
        self.page = self.page.min(pages - 1);
        let page_start = self.page * self.page_size;
        let page_size = self.page_size;
        let visible = self
            .rows(cx)
            .iter()
            .skip(page_start)
            .take(page_size)
            .cloned()
            .collect::<Vec<_>>();

        let visible = Arc::new(visible);
        let item_count = visible.len();
        if self.list_state.item_count() != item_count {
            let old_count = self.list_state.item_count();
            self.list_state
                .splice_with_size_hint(0..old_count, item_count, size(px(0.), px(48.)));
        }
        let entity = cx.entity();
        let visible_for_list = visible.clone();
        let member_list = if self.page_size == 10 {
            let mut rows = div()
                .id("member-ten-row-list")
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .overflow_y_scroll();
            for row in visible.iter() {
                rows = rows.child(self.render_member_row(row, true, cx));
            }
            rows.into_any_element()
        } else {
            list(self.list_state.clone(), move |index, _window, cx| {
                entity.update(cx, |this, cx| {
                    visible_for_list
                        .get(index)
                        .map(|row| this.render_member_row(row, false, cx))
                        .unwrap_or_else(|| div().into_any_element())
                })
            })
            .size_full()
            .into_any_element()
        };
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .p_4()
            .text_color(theme.text_secondary)
            .child(self.render_toolbar(&locale, cx))
            .child(self.render_header(&locale, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(member_list),
            )
            .child(self.render_footer(total, pages, &locale, cx))
            .into_any_element();
        management_page(tr(&locale, "common.members"), body, theme)
    }
}

fn table_row(theme: &crate::theme::Theme, header: bool, fill_available_height: bool) -> gpui::Div {
    let row = div()
        .w_full()
        .flex()
        .items_center()
        .min_w_0()
        .px_4()
        .border_b_1()
        .border_color(theme.border);
    if header {
        row.h(px(48.))
            .text_size(px(12.))
            .font_weight(FontWeight::BOLD)
            .text_color(theme.text_secondary)
    } else if fill_available_height {
        row.flex_1().min_h(px(48.))
    } else {
        row.h(px(48.))
    }
}

fn name_column(content: AnyElement) -> gpui::Div {
    div()
        .flex_basis(px(0.))
        .flex_grow(3.)
        .min_w_0()
        .p_1()
        .child(content)
}

fn weighted_column(content: AnyElement, weight: f32, centered: bool) -> gpui::Div {
    div()
        .flex_basis(px(0.))
        .flex_grow(weight)
        .min_w_0()
        .p_1()
        .when(centered, |element| {
            element.flex().items_center().justify_center().text_center()
        })
        .child(content)
}

fn date_cell(value: String, theme: &crate::theme::Theme) -> AnyElement {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_secondary)
        .child(value)
        .into_any_element()
}
fn role_badge(role: &Role, emphasized: bool, theme: &crate::theme::Theme) -> AnyElement {
    let color = parse_hex_color(&role.color).unwrap_or_else(|| gpui::rgba(0x778899ff));
    let mut background = color;
    background.a = 0.31;
    div()
        .flex()
        .items_center()
        .gap_1()
        .max_w(px(90.))
        .p_1()
        .h(px(24.))
        .rounded(px(4.))
        .bg(background)
        .child(role_dot(role))
        .when(!role.icon.is_empty(), |element| {
            element.child(img(role.icon.clone()).size(px(12.)).flex_shrink_0())
        })
        .child(
            div()
                .min_w(px(0.))
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if emphasized {
                    gpui::white()
                } else {
                    Hsla::from(theme.text_secondary)
                })
                .child(role.name.clone()),
        )
        .into_any_element()
}

fn role_dot(role: &Role) -> AnyElement {
    div()
        .size(px(12.))
        .flex_shrink_0()
        .rounded_full()
        .bg(parse_hex_color(&role.color).unwrap_or_else(|| gpui::rgba(0x778899ff)))
        .into_any_element()
}

fn is_everyone(role: &Role) -> bool {
    role.slug
        .trim_start_matches('@')
        .eq_ignore_ascii_case("everyone")
        || role
            .name
            .trim_start_matches('@')
            .eq_ignore_ascii_case("everyone")
}

fn parse_hex_color(raw: &str) -> Option<gpui::Rgba> {
    let hex = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
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

fn page_arrow(
    left: bool,
    id: &'static str,
    disabled: bool,
    theme: &crate::theme::Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(format!("members-page-{id}"))
        .w(px(40.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.brand)
        .text_color(gpui::white())
        .when(disabled, |element| element.opacity(0.5))
        .when(!disabled, |element| element.cursor_pointer())
        .child(
            Icon::new(IconName::ArrowRight)
                .size(px(20.))
                .text_color(gpui::white())
                .when(left, |element| {
                    element.with_transformation(gpui::Transformation::rotate(gpui::radians(
                        std::f32::consts::PI,
                    )))
                }),
        )
}

fn page_number(
    page: usize,
    selected: bool,
    theme: &crate::theme::Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(format!("members-page-{page}"))
        .w(px(40.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .border_1()
        .border_color(if selected {
            theme.text_primary
        } else {
            theme.border
        })
        .bg(if selected {
            theme.tokens.bg_active_button
        } else {
            theme.brand
        })
        .text_color(theme.text_primary)
        .child(page.to_string())
}

fn tr(locale: &str, key: &'static str) -> String {
    mezon_i18n::t(locale, key).to_string()
}

fn timestamp_sort_key(value: u32, descending: bool) -> u32 {
    if value == 0 {
        u32::MAX
    } else if descending {
        u32::MAX - value
    } else {
        value
    }
}

fn normalize_search(value: &str) -> String {
    value
        .nfd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            '\u{0111}' => 'd',
            _ => character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_is_compact_for_many_pages() {
        assert_eq!(
            pagination_items(19, 39),
            vec![Some(0), None, Some(18), Some(19), Some(20), None, Some(38)]
        );
    }

    #[test]
    fn pagination_matches_react_at_the_edges() {
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
    fn everyone_role_is_hidden() {
        let role = Role {
            name: "Everyone".into(),
            color: String::new(),
            icon: String::new(),
            slug: "everyone".into(),
            max_level_permission: 0,
            order: 0,
        };
        assert!(is_everyone(&role));
    }

    #[test]
    fn search_matches_vietnamese_names_without_diacritics() {
        assert_eq!(normalize_search("Hiền"), "hien");
        assert_eq!(normalize_search("Đặng Văn"), "dang van");
    }

    #[test]
    fn missing_timestamps_always_sort_last() {
        assert!(timestamp_sort_key(0, false) > timestamp_sort_key(10, false));
        assert!(timestamp_sort_key(0, true) > timestamp_sort_key(10, true));
        assert!(timestamp_sort_key(20, false) > timestamp_sort_key(10, false));
        assert!(timestamp_sort_key(20, true) < timestamp_sort_key(10, true));
    }
}
