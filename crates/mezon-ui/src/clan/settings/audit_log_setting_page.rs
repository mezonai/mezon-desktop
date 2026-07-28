use chrono::{Local, NaiveDate};
use gpui::{
    App, Context, Entity, FontWeight, SharedString, Subscription, UniformListScrollHandle, Window,
    deferred, div, prelude::*, px, size, uniform_list,
};
use mezon_store::{
    ALL_ACTION_INDEX, AUDIT_ACTION_OPTIONS, AuditLogEntry, AuditLogQuery, AuditLogStateView,
    AuditLogStore, ClanId, ClanMembersStore, Settings, UserId, audit_action_i18n_key,
    audit_action_index_for_api_log,
};
use std::sync::Arc;

use crate::components::primitives::{
    Avatar, DatePicker, DatePickerEvent, Icon, IconName, InputState, Sizable, Size, Spinner,
    h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::imgproxy;
use mezon_store::message_time::local_datetime;

const AVATAR_SIZE: f32 = 40.0;
const CARD_PADDING: f32 = 10.0;
const CARD_GAP: f32 = 16.0;
const ROW_HEIGHT: f32 = 68.0;
const FILTER_MENU_MAX_HEIGHT: f32 = 280.0;
const FILTER_CHIP_HEIGHT: f32 = 36.0;
const FILTER_MENU_ROW_HEIGHT: f32 = 40.0;
const FILTER_DROPDOWN_INNER_WIDTH: f32 = 284.0;

fn filter_menu_list_height(row_count: usize) -> gpui::Pixels {
    px((row_count as f32 * FILTER_MENU_ROW_HEIGHT).min(FILTER_MENU_MAX_HEIGHT))
}

fn localized_audit_action(locale: &str, action_log: &str) -> String {
    if let Some(index) = audit_action_index_for_api_log(action_log) {
        return mezon_i18n::t(locale, audit_action_i18n_key(index)).to_string();
    }
    action_log.trim().to_lowercase()
}

#[derive(Clone, PartialEq)]
struct UserFilterRow {
    user_id: Option<UserId>,
    label: SharedString,
}

pub struct AuditLogSettingPage {
    clan_id: ClanId,
    settings: Entity<Settings>,
    selected_action: usize,
    selected_user_id: Option<UserId>,
    selected_user_label: SharedString,
    selected_date: NaiveDate,
    user_menu_open: bool,
    action_menu_open: bool,
    user_search: Option<Entity<InputState>>,
    action_search: Option<Entity<InputState>>,
    date_picker: Entity<DatePicker>,
    list_scroll: UniformListScrollHandle,
    filter_menu_scroll: UniformListScrollHandle,
    _user_search_sub: Option<Subscription>,
    _action_search_sub: Option<Subscription>,
    _date_picker_sub: Subscription,
    _audit_log_sub: Subscription,
}

fn render_date_filter(date_picker: Entity<DatePicker>) -> impl IntoElement {
    div()
        .id("audit-log-date-filter")
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .w_full()
        .h(px(FILTER_CHIP_HEIGHT))
        .child(div().w_full().h_full().child(date_picker))
}

fn filter_chip(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    active: bool,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut chip = h_flex()
        .id(id)
        .w_full()
        .h_full()
        .min_h(px(FILTER_CHIP_HEIGHT))
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .px(px(12.0))
        .rounded_md()
        .border_1()
        .border_color(if active {
            theme.tokens.text_theme_primary
        } else {
            theme.border
        })
        .bg(if active {
            theme.bg_hover
        } else {
            theme.tokens.bg_tertiary
        })
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .child(
            Icon::new(icon)
                .size(px(16.0))
                .flex_shrink_0()
                .text_color(if active {
                    theme.tokens.text_theme_primary
                } else {
                    theme.text_muted
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .overflow_hidden()
                .text_ellipsis()
                .child(label.into()),
        )
        .child(
            Icon::new(IconName::ArrowDownFill)
                .size(px(12.0))
                .flex_shrink_0()
                .text_color(theme.text_muted),
        );
    chip.interactivity().on_click(on_click);
    chip
}

fn filter_dropdown_panel(
    theme: &Theme,
    on_dismiss: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
    search: Option<Entity<InputState>>,
    list: impl IntoElement,
) -> impl IntoElement {
    v_flex()
        .absolute()
        .top_full()
        .left_0()
        .mt(px(6.0))
        .w(px(300.0))
        .p(px(8.0))
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.tokens.bg_theme_input_primary)
        .shadow_lg()
        .occlude()
        .on_mouse_down_out(on_dismiss)
        .when_some(search, |panel, search| {
            panel.child(div().mb(px(8.0)).child(search))
        })
        .child(list)
}

fn filter_menu_row(
    id: impl Into<gpui::ElementId>,
    label: SharedString,
    selected: bool,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut row = h_flex()
        .id(id)
        .w_full()
        .items_center()
        .px(px(12.0))
        .py(px(10.0))
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_hover))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .text_color(if selected {
                    theme.text_primary
                } else {
                    theme.text_secondary
                })
                .child(label),
        );
    if selected {
        row = row.child(
            Icon::new(IconName::Check)
                .size(px(16.0))
                .text_color(theme.tokens.text_theme_primary),
        );
    }
    row.interactivity().on_click(on_click);
    row
}

fn render_user_filter_menu_row(
    index: usize,
    row: &UserFilterRow,
    selected_user_id: Option<UserId>,
    theme: &Theme,
    entity: Entity<AuditLogSettingPage>,
) -> impl IntoElement {
    let selected = row.user_id == selected_user_id;
    let user_id = row.user_id;
    let label = row.label.clone();
    filter_menu_row(
        ("audit-log-user-filter-item", index),
        row.label.clone(),
        selected,
        theme,
        move |_, _, cx| {
            entity.update(cx, |this, cx| {
                this.set_user_filter(user_id, label.clone(), cx);
            });
        },
    )
}

fn render_action_filter_menu_row(
    index: usize,
    action_index: usize,
    label: SharedString,
    selected_action: usize,
    theme: &Theme,
    entity: Entity<AuditLogSettingPage>,
) -> impl IntoElement {
    let selected = action_index == selected_action;
    filter_menu_row(
        ("audit-log-action-filter-item", index),
        label,
        selected,
        theme,
        move |_, _, cx| {
            entity.update(cx, |this, cx| {
                this.set_action_filter(action_index, cx);
            });
        },
    )
}

fn render_log_action_line(
    entry: &AuditLogEntry,
    username: &str,
    locale: &str,
    theme: &Theme,
) -> impl IntoElement {
    let action = localized_audit_action(locale, &entry.action_log);
    let target = audit_log_entity_label(entry);
    let has_channel = !entry.channel_id.is_zero();
    let in_channel = mezon_i18n::t(locale, "auditLog.auditLogItem.inChannel");

    h_flex()
        .w_full()
        .min_w(px(0.0))
        .overflow_hidden()
        .items_center()
        .gap_1()
        .child(
            div()
                .flex_shrink_0()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(username.to_string()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_sm()
                .text_color(theme.text_muted)
                .child(format!(" {action} ")),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(target),
        )
        .when(has_channel, |row| {
            row.child(
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(format!(" {in_channel} ")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(format!("#{}", entry.channel_label)),
            )
        })
}

fn format_audit_log_time(timestamp_sec: u32, locale: &str) -> String {
    if timestamp_sec == 0 {
        return String::new();
    }
    let Some(dt) = local_datetime(i64::from(timestamp_sec)) else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let date = dt.date_naive();
    let time = dt.format("%H:%M").to_string();
    if date == today {
        mezon_i18n::t(locale, "common.todayAtTime").replace("{{time}}", &time)
    } else if date == today.pred_opt().unwrap_or(today) {
        mezon_i18n::t(locale, "common.yesterdayAtTime").replace("{{time}}", &time)
    } else {
        dt.format("%d/%m/%Y, %H:%M").to_string()
    }
}

fn audit_log_entity_label(entry: &AuditLogEntry) -> String {
    let name = entry.entity_name.trim();
    if name.is_empty() {
        entry.entity_id.to_string()
    } else {
        name.to_string()
    }
}

fn render_audit_log_row(
    entry: &AuditLogEntry,
    clan_id: ClanId,
    locale: &str,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let members = ClanMembersStore::global(cx).read(cx);
    let member = members.member(clan_id, entry.user_id);
    let username: String = member
        .map(|member| member.name().to_string())
        .unwrap_or_else(|| entry.user_id.get().to_string());
    let avatar_raw = member.map(|member| member.avatar()).unwrap_or("");
    let avatar_src = if avatar_raw.is_empty() {
        SharedString::default()
    } else {
        imgproxy::avatar_url(cx, avatar_raw).into()
    };
    let time_label = format_audit_log_time(entry.time_log_seconds, locale);

    div()
        .id(("audit-log-row", entry.id as u64))
        .w_full()
        .h(px(ROW_HEIGHT + CARD_GAP))
        .child(
            h_flex()
                .w_full()
                .h(px(ROW_HEIGHT))
                .items_center()
                .gap_3()
                .p(px(CARD_PADDING))
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.tokens.bg_tertiary)
                .child(div().flex_shrink_0().child({
                    let mut avatar = Avatar::new()
                        .name(username.clone())
                        .size_px(px(AVATAR_SIZE));
                    if !avatar_src.is_empty() {
                        avatar = avatar.src(avatar_src);
                    }
                    avatar
                }))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(3.0))
                        .child(render_log_action_line(entry, &username, locale, theme))
                        .when(!time_label.is_empty(), |col| {
                            col.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(time_label),
                            )
                        }),
                ),
        )
}

impl AuditLogSettingPage {
    fn audit_log_state<'a>(&self, cx: &'a App) -> AuditLogStateView<'a> {
        AuditLogStore::global(cx).read(cx).state(self.clan_id)
    }

    fn audit_log_query(&self) -> AuditLogQuery {
        AuditLogQuery {
            action_index: self.selected_action,
            user_id: self.selected_user_id,
            date: self.selected_date,
        }
    }

    pub fn new(clan_id: ClanId, settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        ClanMembersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });

        let today = Local::now().date_naive();
        let date_picker = cx.new(|cx| {
            let mut picker = DatePicker::new(cx);
            picker.set_field_height(FILTER_CHIP_HEIGHT, cx);
            picker.set_max(Some(today), cx);
            picker.set_selected_silent(Some(today), cx);
            picker
        });
        let date_picker_entity = date_picker.clone();
        let date_picker_sub = cx.subscribe(&date_picker_entity, |this, _, event, cx| {
            if let DatePickerEvent::Change(Some(date)) = event
                && this.selected_date != *date
            {
                this.selected_date = *date;
                this.fetch_logs(cx);
            }
            if matches!(event, DatePickerEvent::Opened) {
                this.user_menu_open = false;
                this.action_menu_open = false;
                cx.notify();
            }
        });

        let audit_log_sub = cx.observe(&AuditLogStore::global(cx), |_, _, cx| cx.notify());

        let mut this = Self {
            clan_id,
            settings,
            selected_action: ALL_ACTION_INDEX,
            selected_user_id: None,
            selected_user_label: SharedString::default(),
            selected_date: today,
            user_menu_open: false,
            action_menu_open: false,
            user_search: None,
            action_search: None,
            date_picker,
            list_scroll: UniformListScrollHandle::new(),
            filter_menu_scroll: UniformListScrollHandle::new(),
            _user_search_sub: None,
            _action_search_sub: None,
            _date_picker_sub: date_picker_sub,
            _audit_log_sub: audit_log_sub,
        };

        this.fetch_logs(cx);
        this
    }

    pub fn release(&mut self, cx: &mut Context<Self>) {
        AuditLogStore::global(cx).update(cx, |store, _| store.cancel(self.clan_id));
        self.user_search = None;
        self.action_search = None;
        self._user_search_sub = None;
        self._action_search_sub = None;
    }

    fn ensure_user_search(&mut self, locale: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.user_search.is_some() {
            return;
        }
        let placeholder = mezon_i18n::t(locale, "auditLog.filterUserAuditLog.placeholder");
        let user_search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        self._user_search_sub = Some(cx.subscribe(&user_search, |_, _, _, cx| cx.notify()));
        self.user_search = Some(user_search);
    }

    fn ensure_action_search(&mut self, locale: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.action_search.is_some() {
            return;
        }
        let placeholder = mezon_i18n::t(locale, "auditLog.filterActionAuditLog.placeholder");
        let action_search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        self._action_search_sub = Some(cx.subscribe(&action_search, |_, _, _, cx| cx.notify()));
        self.action_search = Some(action_search);
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn fetch_logs(&mut self, cx: &mut Context<Self>) {
        let query = self.audit_log_query();
        AuditLogStore::global(cx).update(cx, |store, cx| {
            store.fetch(self.clan_id, query, cx);
        });
    }

    fn set_user_filter(
        &mut self,
        user_id: Option<UserId>,
        label: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.selected_user_id = user_id;
        self.selected_user_label = label;
        self.user_menu_open = false;
        self.fetch_logs(cx);
    }

    fn set_action_filter(&mut self, action_index: usize, cx: &mut Context<Self>) {
        self.selected_action = action_index;
        self.action_menu_open = false;
        self.fetch_logs(cx);
    }

    fn user_filter_rows(&self, locale: &str, cx: &App) -> Vec<UserFilterRow> {
        let query = self
            .user_search
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let mut rows = vec![UserFilterRow {
            user_id: None,
            label: mezon_i18n::t(locale, "auditLogSearch.all").into(),
        }];
        let mut members: Vec<UserFilterRow> = ClanMembersStore::global(cx)
            .read(cx)
            .members(self.clan_id)
            .into_iter()
            .filter_map(|member| {
                let label = member.name().to_string();
                if !query.is_empty() && !label.to_lowercase().contains(&query) {
                    return None;
                }
                Some(UserFilterRow {
                    user_id: Some(member.id()),
                    label: label.into(),
                })
            })
            .collect();
        members.sort_by_cached_key(|member| member.label.to_lowercase());
        rows.extend(members);
        rows
    }

    fn action_filter_rows(&self, locale: &str, cx: &App) -> Vec<(usize, SharedString)> {
        let query = self
            .action_search
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        AUDIT_ACTION_OPTIONS
            .iter()
            .enumerate()
            .filter_map(|(index, option)| {
                let label = mezon_i18n::t(locale, option.i18n_key);
                if !query.is_empty() && !label.to_lowercase().contains(&query) {
                    return None;
                }
                Some((index, label.into()))
            })
            .collect()
    }

    fn render_user_filter(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.user_menu_open {
            self.ensure_user_search(locale, window, cx);
        }
        let label = if self.selected_user_id.is_some() {
            self.selected_user_label.clone()
        } else {
            mezon_i18n::t(locale, "auditLog.filterUserAuditLog.allUsers").into()
        };
        let open = self.user_menu_open;
        let selected_user_id = self.selected_user_id;
        let user_active = self.selected_user_id.is_some();
        let entity = cx.entity();
        let filter_menu_scroll = self.filter_menu_scroll.clone();

        div()
            .id("audit-log-user-filter")
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .w_full()
            .h(px(FILTER_CHIP_HEIGHT))
            .child(div().w_full().h_full().child(filter_chip(
                "audit-log-user-filter-chip",
                IconName::MemberIcon,
                label,
                user_active,
                theme,
                cx.listener(|this, _, _, cx| {
                    this.user_menu_open = !this.user_menu_open;
                    this.action_menu_open = false;
                    this.date_picker.update(cx, |picker, cx| picker.close(cx));
                    cx.notify();
                }),
            )))
            .when(open, |menu| {
                let rows: Arc<[UserFilterRow]> = self.user_filter_rows(locale, cx).into();
                let row_count = rows.len();
                let rows_for_list = Arc::clone(&rows);
                let list = uniform_list(
                    "audit-log-user-filter-menu",
                    row_count,
                    move |range, _window, cx| {
                        let theme = cx.theme();
                        range
                            .map(|index| {
                                rows_for_list.get(index).map_or_else(
                                    || div().h(px(FILTER_MENU_ROW_HEIGHT)).into_any_element(),
                                    |row| {
                                        render_user_filter_menu_row(
                                            index,
                                            row,
                                            selected_user_id,
                                            theme,
                                            entity.clone(),
                                        )
                                        .into_any_element()
                                    },
                                )
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .with_item_size(size(
                    px(FILTER_DROPDOWN_INNER_WIDTH),
                    px(FILTER_MENU_ROW_HEIGHT),
                ))
                .track_scroll(&filter_menu_scroll)
                .h(filter_menu_list_height(row_count))
                .w_full();
                menu.child(deferred(filter_dropdown_panel(
                    theme,
                    cx.listener(|this, _, _, cx| {
                        if this.user_menu_open {
                            this.user_menu_open = false;
                            cx.notify();
                        }
                    }),
                    self.user_search.clone(),
                    list,
                )))
            })
    }

    fn render_action_filter(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.action_menu_open {
            self.ensure_action_search(locale, window, cx);
        }
        let label: SharedString =
            mezon_i18n::t(locale, audit_action_i18n_key(self.selected_action)).into();
        let open = self.action_menu_open;
        let selected_action = self.selected_action;
        let action_active = self.selected_action != ALL_ACTION_INDEX;
        let entity = cx.entity();
        let filter_menu_scroll = self.filter_menu_scroll.clone();

        div()
            .id("audit-log-action-filter")
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .w_full()
            .h(px(FILTER_CHIP_HEIGHT))
            .child(div().w_full().h_full().child(filter_chip(
                "audit-log-action-filter-chip",
                IconName::FiltersIcon,
                label,
                action_active,
                theme,
                cx.listener(|this, _, _, cx| {
                    this.action_menu_open = !this.action_menu_open;
                    this.user_menu_open = false;
                    this.date_picker.update(cx, |picker, cx| picker.close(cx));
                    cx.notify();
                }),
            )))
            .when(open, |menu| {
                let rows: Arc<[(usize, SharedString)]> = self.action_filter_rows(locale, cx).into();
                let row_count = rows.len();
                let rows_for_list = Arc::clone(&rows);
                let list = uniform_list(
                    "audit-log-action-filter-menu",
                    row_count,
                    move |range, _window, cx| {
                        let theme = cx.theme();
                        range
                            .map(|index| {
                                rows_for_list.get(index).map_or_else(
                                    || div().h(px(FILTER_MENU_ROW_HEIGHT)).into_any_element(),
                                    |(action_index, label)| {
                                        render_action_filter_menu_row(
                                            index,
                                            *action_index,
                                            label.clone(),
                                            selected_action,
                                            theme,
                                            entity.clone(),
                                        )
                                        .into_any_element()
                                    },
                                )
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .with_item_size(size(
                    px(FILTER_DROPDOWN_INNER_WIDTH),
                    px(FILTER_MENU_ROW_HEIGHT),
                ))
                .track_scroll(&filter_menu_scroll)
                .h(filter_menu_list_height(row_count))
                .w_full();
                menu.child(deferred(filter_dropdown_panel(
                    theme,
                    cx.listener(|this, _, _, cx| {
                        if this.action_menu_open {
                            this.action_menu_open = false;
                            cx.notify();
                        }
                    }),
                    self.action_search.clone(),
                    list,
                )))
            })
    }

    fn render_filter_toolbar(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("audit-log-filter-toolbar")
            .w_full()
            .h(px(FILTER_CHIP_HEIGHT))
            .items_stretch()
            .gap_3()
            .child(render_date_filter(self.date_picker.clone()))
            .child(self.render_user_filter(locale, theme, window, cx))
            .child(self.render_action_filter(locale, theme, window, cx))
    }

    fn render_log_list(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let audit_state = self.audit_log_state(cx);
        if audit_state.loading {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new().with_size(Size::Small))
                .into_any_element();
        }

        if audit_state.fetch_failed {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .px(px(24.0))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.status_dnd)
                        .text_center()
                        .child(mezon_i18n::t(locale, "auditLog.fetchFailed")),
                )
                .into_any_element();
        }

        if audit_state.logs.is_empty() {
            return v_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .items_center()
                .justify_start()
                .pt(px(40.0))
                .child(
                    v_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::SearchIcon)
                                .size(px(28.0))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_muted)
                                .text_center()
                                .child(mezon_i18n::t(locale, "auditLog.emptyAuditLog.noLogsYet")),
                        ),
                )
                .into_any_element();
        }

        let clan_id = self.clan_id;
        let locale_owned = locale.to_string();
        let row_count = audit_state.logs.len();
        uniform_list("audit-log-list", row_count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let logs = AuditLogStore::global(cx).read(cx).logs(clan_id);
            range
                .map(|index| {
                    logs.get(index).map_or_else(
                        || div().h(px(ROW_HEIGHT + CARD_GAP)).into_any_element(),
                        |entry| {
                            render_audit_log_row(entry, clan_id, &locale_owned, &theme, cx)
                                .into_any_element()
                        },
                    )
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.list_scroll)
        .w_full()
        .with_item_size(size(px(740.0), px(ROW_HEIGHT + CARD_GAP)))
        .flex_1()
        .min_h_0()
        .into_any_element()
    }
}

impl Render for AuditLogSettingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale(cx);
        self.date_picker.update(cx, |picker, _| {
            picker.set_locale(locale.clone());
        });

        v_flex()
            .id("audit-log-setting-page")
            .w_full()
            .h_full()
            .min_h_0()
            .flex_1()
            .gap_5()
            .child(
                div()
                    .flex_shrink_0()
                    .w_full()
                    .child(self.render_filter_toolbar(&locale, &theme, window, cx)),
            )
            .child(
                v_flex()
                    .id("audit-log-list-panel")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.render_log_list(&locale, &theme, cx)),
            )
    }
}
