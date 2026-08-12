use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, TimeZone as _};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, ClipboardItem, Context, FocusHandle,
    Focusable, FontWeight, ListAlignment, ListState, SharedString, Transformation, WeakEntity,
    Window, div, ease_in_out, img, linear_color_stop, linear_gradient, list, percentage,
    prelude::*, px, relative, rgb, rgba,
};
use mezon_store::{TransactionCursor, UserId, UsersByUserStore, WalletStore, WalletTransaction};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName, Sizable, Size, Spinner};
use crate::theme::{ActiveTheme, Theme};

use mezon_store::{TOKEN_DECIMAL_FACTOR as DECIMAL_FACTOR, TOKEN_DECIMALS as DECIMALS};

const FILTER_ALL: i32 = 0;
const FILTER_RECEIVED: i32 = 1;
const FILTER_SENT: i32 = 2;

const CARD_WIDTH: f32 = 800.;
const LIST_HEIGHT: f32 = 450.;
const LIST_OVERDRAW: f32 = 200.;
const ROW_HEIGHT: f32 = 84.;
const SKELETON_ROWS: usize = 6;
const ID_SUFFIX_LEN: usize = 8;
const PREFETCH_THRESHOLD: usize = 3;
const SPIN_PERIOD: Duration = Duration::from_millis(800);

const SENT_COLOR: u32 = 0xf87171;
const SENT_SURFACE: u32 = 0x7f1d1d33;
const RECEIVED_COLOR: u32 = 0x4ade80;
const RECEIVED_SURFACE: u32 = 0x14532d33;
const TAB_ALL_COLOR: u32 = 0x60a5fa;
const TAB_ALL_SURFACE: u32 = 0x1e3a8a4d;
const TAB_SENT_SURFACE: u32 = 0x7f1d1d4d;
const TAB_RECEIVED_SURFACE: u32 = 0x14532d4d;
const HEADER_GRADIENT_FROM: u32 = 0x2563eb;
const HEADER_GRADIENT_TO: u32 = 0x7e22ce;
const HEADER_ICON_COLOR: u32 = 0xffffff;

const FILTER_TABS: [(i32, &str, u32, u32); 3] = [
    (
        FILTER_ALL,
        "transactionHistory.filters.all",
        TAB_ALL_COLOR,
        TAB_ALL_SURFACE,
    ),
    (
        FILTER_SENT,
        "transactionHistory.filters.sent",
        SENT_COLOR,
        TAB_SENT_SURFACE,
    ),
    (
        FILTER_RECEIVED,
        "transactionHistory.filters.received",
        RECEIVED_COLOR,
        TAB_RECEIVED_SURFACE,
    ),
];

#[derive(Clone)]
struct TxRow {
    hash: SharedString,
    id_label: SharedString,
    amount: SharedString,
    kind_label: SharedString,
    date: SharedString,
    sent: bool,
}

#[derive(Clone)]
struct TxDetail {
    hash: SharedString,
    sender: SharedString,
    amount: SharedString,
    receiver: SharedString,
    note: SharedString,
    created: SharedString,
}

pub struct TransactionHistoryModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    address: String,
    filter: i32,
    rows: Rc<Vec<TxRow>>,
    list_state: ListState,
    cursor: Option<TransactionCursor>,
    has_more: bool,
    loading: bool,
    loading_more: bool,
    error: Option<SharedString>,
    expanded: Option<SharedString>,
    detail: Option<TxDetail>,
    detail_loading: bool,
    req_generation: u64,
    detail_generation: u64,
}

impl Focusable for TransactionHistoryModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TransactionHistoryModal {
    pub fn open(locale: SharedString, window: &mut Window, cx: &mut App) {
        if Shell::global(cx).read(cx).has_modal() {
            return;
        }
        let address = WalletStore::try_global(cx)
            .and_then(|w| w.read(cx).address().map(|a| a.to_string()))
            .unwrap_or_default();
        let view = cx.new(|cx| {
            let list_state = ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW))
                .measure_all()
                .smooth_line_scroll()
                .suppress_hover_while_scrolling();
            Self::attach_scroll_handler(&list_state, cx.weak_entity());
            let mut this = Self {
                focus_handle: cx.focus_handle(),
                locale: locale.clone(),
                address: address.clone(),
                filter: FILTER_ALL,
                rows: Rc::new(Vec::new()),
                list_state,
                cursor: None,
                has_more: false,
                loading: !address.is_empty(),
                loading_more: false,
                error: None,
                expanded: None,
                detail: None,
                detail_loading: false,
                req_generation: 0,
                detail_generation: 0,
            };
            if address.is_empty() {
                this.error = Some(mezon_i18n::t(&locale, "token.history.walletUnavailable").into());
            } else {
                this.fetch(false, cx);
            }
            this
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn attach_scroll_handler(list_state: &ListState, weak: WeakEntity<Self>) {
        list_state.set_scroll_handler(move |event, _window, cx| {
            let visible_end = event.visible_range.end;
            weak.update(cx, |this, cx| this.maybe_load_more(visible_end, cx))
                .ok();
        });
    }

    fn set_filter(&mut self, filter: i32, cx: &mut Context<Self>) {
        if self.filter == filter || self.loading {
            return;
        }
        self.filter = filter;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.address.is_empty() {
            return;
        }
        self.loading = true;
        self.error = None;
        self.expanded = None;
        self.detail = None;
        self.fetch(false, cx);
        cx.notify();
    }

    fn maybe_load_more(&mut self, visible_end: usize, cx: &mut Context<Self>) {
        if !self.has_more || self.loading || self.loading_more || self.rows.is_empty() {
            return;
        }
        if self.rows.len().saturating_sub(visible_end) > PREFETCH_THRESHOLD {
            return;
        }
        self.loading_more = true;
        self.fetch(true, cx);
        cx.notify();
    }

    fn fetch(&mut self, load_more: bool, cx: &mut Context<Self>) {
        self.req_generation = self.req_generation.wrapping_add(1);
        let generation = self.req_generation;
        let address = self.address.clone();
        let filter = self.filter;
        let cursor = if load_more { self.cursor.clone() } else { None };
        let locale = self.locale.clone();
        let task = WalletStore::global(cx).update(cx, |wallet, cx| {
            wallet.load_wallet_transactions(address.clone(), filter, cursor, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.req_generation != generation {
                    return;
                }
                this.loading = false;
                this.loading_more = false;
                match result {
                    Ok(page) => {
                        let rows = page
                            .transactions
                            .iter()
                            .map(|tx| build_row(tx, &locale))
                            .collect::<Vec<_>>();
                        let next_cursor =
                            page.transactions.last().and_then(TransactionCursor::after);
                        if next_cursor.is_some() || !load_more {
                            this.cursor = next_cursor;
                        }
                        this.has_more = page.has_more;
                        this.error = None;
                        if load_more {
                            let previous = this.rows.len();
                            Rc::make_mut(&mut this.rows).extend(rows);
                            let added = this.rows.len() - previous;
                            if added > 0 {
                                this.list_state.splice(previous..previous, added);
                            }
                        } else {
                            this.rows = Rc::new(rows);
                            this.list_state.reset(this.rows.len());
                        }
                    }
                    Err(error) => {
                        if load_more {
                            this.has_more = false;
                        } else {
                            this.rows = Rc::new(Vec::new());
                            this.list_state.reset(0);
                        }
                        this.error = Some(error.into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_detail(&mut self, hash: SharedString, index: usize, cx: &mut Context<Self>) {
        if self.expanded.as_ref() == Some(&hash) {
            self.expanded = None;
            self.detail = None;
            self.detail_loading = false;
        } else {
            self.expanded = Some(hash.clone());
            self.detail = None;
            self.detail_loading = true;
            self.fetch_detail(hash, cx);
        }
        self.list_state.splice(index..index + 1, 1);
        cx.notify();
    }

    fn fetch_detail(&mut self, hash: SharedString, cx: &mut Context<Self>) {
        self.detail_generation = self.detail_generation.wrapping_add(1);
        let generation = self.detail_generation;
        let address = self.address.clone();
        let locale = self.locale.clone();
        let task = WalletStore::global(cx).update(cx, |wallet, cx| {
            wallet.wallet_transaction_detail(hash.to_string(), address, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.detail_generation != generation || this.expanded.as_ref() != Some(&hash) {
                    return;
                }
                this.detail_loading = false;
                if let Ok(transaction) = result {
                    this.detail = Some(build_detail(&transaction, &locale, cx));
                }
                if let Some(index) = this.rows.iter().position(|row| row.hash == hash) {
                    this.list_state.splice(index..index + 1, 1);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_header(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let locale = self.locale.clone();
        let tk = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let reload = Icon::new(IconName::ReloadIcon)
            .size(px(20.))
            .text_color(theme.tokens.text_theme_primary);
        let reload = if self.loading && window.is_window_active() {
            reload
                .with_animation(
                    "tx-history-refresh-spin",
                    Animation::new(SPIN_PERIOD)
                        .repeat()
                        .with_easing(ease_in_out),
                    |icon, delta| {
                        icon.with_transformation(Transformation::rotate(percentage(delta)))
                    },
                )
                .into_any_element()
        } else {
            reload.into_any_element()
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p_6()
            .border_b_1()
            .border_color(theme.tokens.border_primary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(48.))
                            .flex_none()
                            .rounded(px(12.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .shadow_lg()
                            .bg(linear_gradient(
                                135.,
                                linear_color_stop(rgb(HEADER_GRADIENT_FROM), 0.),
                                linear_color_stop(rgb(HEADER_GRADIENT_TO), 1.),
                            ))
                            .child(
                                Icon::new(IconName::HistoryTransaction)
                                    .size(px(36.))
                                    .text_color(rgb(HEADER_ICON_COLOR)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(tk("transactionHistory.header.title")),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.tokens.text_secondary)
                                    .child(tk("transactionHistory.header.subtitle")),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("tx-history-refresh")
                            .p_2()
                            .rounded(px(8.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.refresh(cx);
                            }))
                            .child(reload),
                    )
                    .child(
                        div()
                            .id("tx-history-close")
                            .p_2()
                            .rounded(px(8.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                            .on_click(|_: &ClickEvent, _window, cx| Self::close(cx))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(20.))
                                    .text_color(theme.tokens.text_theme_primary),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let locale = self.locale.clone();
        let active = self.filter;
        let mut tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .pb_4()
            .border_b_1()
            .border_color(theme.tokens.border_primary);
        for (filter, label_key, color, surface) in FILTER_TABS {
            let is_active = filter == active;
            tabs = tabs.child(
                div()
                    .id(("tx-history-tab", filter as usize))
                    .px_4()
                    .py_2()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .when(is_active, |d| d.bg(rgba(surface)).text_color(rgb(color)))
                    .when(!is_active, |d| {
                        d.text_color(theme.tokens.text_theme_primary)
                            .hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_filter(filter, cx);
                    }))
                    .child(mezon_i18n::t(&locale, label_key).to_string()),
            );
        }
        div().px_6().pt_4().child(tabs).into_any_element()
    }

    fn render_body(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let container = div()
            .flex()
            .flex_col()
            .px_6()
            .pb_2()
            .mt_4()
            .h(px(LIST_HEIGHT));

        if self.rows.is_empty() && self.loading {
            return container.child(render_skeletons(theme)).into_any_element();
        }
        if self.rows.is_empty() {
            return container
                .child(render_placeholder(
                    theme,
                    &self.locale,
                    self.filter,
                    &self.error,
                ))
                .into_any_element();
        }

        let rows = self.rows.clone();
        let expanded = self.expanded.clone();
        let detail = self.detail.clone();
        let detail_loading = self.detail_loading;
        let theme_for_rows = cx.theme().clone();
        let locale = self.locale.clone();
        let weak = cx.weak_entity();
        let suppress_hover = self.list_state.is_scroll_hover_suppressed();

        container
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        list(self.list_state.clone(), move |index, _window, _cx| {
                            let Some(row) = rows.get(index) else {
                                return div().into_any_element();
                            };
                            let is_expanded = expanded.as_ref() == Some(&row.hash);
                            render_row(
                                theme_for_rows.as_ref(),
                                &locale,
                                row,
                                index,
                                is_expanded,
                                if is_expanded { detail.as_ref() } else { None },
                                is_expanded && detail_loading,
                                suppress_hover,
                                weak.clone(),
                            )
                        })
                        .size_full(),
                    )
                    .custom_scrollbars(
                        Scrollbars::always_visible(ScrollAxes::Vertical)
                            .tracked_scroll_handle(&self.list_state),
                        window,
                        cx,
                    ),
            )
            .into_any_element()
    }

    fn render_footer(&self, theme: &Theme) -> AnyElement {
        let locale = self.locale.clone();
        let mut content = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .h(px(40.))
            .text_xs()
            .text_color(theme.tokens.text_secondary);

        if self.loading || self.loading_more {
            content = content.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(Spinner::new().with_size(Size::Small))
                    .child(
                        mezon_i18n::t(&locale, "transactionHistory.footers.fetching").to_string(),
                    ),
            );
        } else if self.has_more {
            content = content.child(
                Icon::new(IconName::ArrowDown)
                    .size(px(16.))
                    .text_color(theme.tokens.text_secondary),
            );
        } else if !self.rows.is_empty() {
            content = content.child(
                mezon_i18n::t(&locale, "transactionHistory.footers.noti")
                    .replace("{{count}}", &self.rows.len().to_string()),
            );
        }

        div()
            .px_6()
            .py_3()
            .border_t_1()
            .border_color(theme.tokens.border_primary)
            .child(content)
            .into_any_element()
    }
}

impl Render for TransactionHistoryModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_this, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .occlude()
            .w(px(CARD_WIDTH))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(theme.surfaces.surface)
            .shadow_lg()
            .child(self.render_header(&theme, window, cx))
            .child(self.render_tabs(&theme, cx))
            .child(self.render_body(&theme, window, cx))
            .child(self.render_footer(&theme))
    }
}

#[allow(clippy::too_many_arguments)]
fn render_row(
    theme: &Theme,
    locale: &SharedString,
    row: &TxRow,
    index: usize,
    is_expanded: bool,
    detail: Option<&TxDetail>,
    detail_loading: bool,
    suppress_hover: bool,
    weak: WeakEntity<TransactionHistoryModal>,
) -> AnyElement {
    let accent = if row.sent {
        rgb(SENT_COLOR)
    } else {
        rgb(RECEIVED_COLOR)
    };
    let surface = if row.sent {
        rgba(SENT_SURFACE)
    } else {
        rgba(RECEIVED_SURFACE)
    };
    let arrow = Icon::new(if is_expanded {
        IconName::ArrowDown
    } else {
        IconName::ArrowRight
    })
    .size(px(16.))
    .text_color(accent);
    let arrow = if is_expanded {
        arrow
            .with_transformation(Transformation::rotate(percentage(0.5)))
            .into_any_element()
    } else {
        arrow.into_any_element()
    };
    let hash = row.hash.clone();

    let summary = div()
        .p_4()
        .h(px(ROW_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .gap_4()
        .child(
            div()
                .w(px(225.))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded_full()
                        .bg(surface)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(arrow),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(accent)
                                .child(row.amount.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.tokens.text_secondary)
                                .child(row.kind_label.clone()),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .items_start()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(row.id_label.clone()),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(theme.tokens.text_secondary)
                        .child(row.date.clone()),
                ),
        );

    div()
        .pb_4()
        .child(
            div()
                .id(row.hash.clone())
                .cursor_pointer()
                .rounded(px(8.))
                .overflow_hidden()
                .bg(theme.tokens.bg_active_member_channel)
                .border_1()
                .border_color(theme.tokens.border_primary)
                .when(!suppress_hover, |el| {
                    el.hover(|s| s.border_color(theme.tokens.border_hover))
                })
                .on_click(move |_: &ClickEvent, _window, cx| {
                    weak.update(cx, |this, cx| this.toggle_detail(hash.clone(), index, cx))
                        .ok();
                })
                .child(summary)
                .when(is_expanded, |el| {
                    el.child(render_detail(
                        theme,
                        locale,
                        detail,
                        detail_loading,
                        suppress_hover,
                    ))
                }),
        )
        .into_any_element()
}

fn render_detail(
    theme: &Theme,
    locale: &SharedString,
    detail: Option<&TxDetail>,
    loading: bool,
    suppress_hover: bool,
) -> AnyElement {
    let tk = |key: &'static str| mezon_i18n::t(locale, key).to_uppercase();
    let placeholder = SharedString::from("");
    let value = |get: fn(&TxDetail) -> SharedString| match detail {
        Some(detail) if !loading => get(detail),
        _ => placeholder.clone(),
    };

    let left = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_4()
        .child(detail_field(
            theme,
            IconName::Transaction,
            tk("transactionHistory.transactionDetail.fields.transactionId"),
            value(|d| d.hash.clone()),
            true,
            suppress_hover,
            locale.clone(),
        ))
        .child(detail_field(
            theme,
            IconName::DollarIcon,
            tk("transactionHistory.transactionDetail.fields.amount"),
            value(|d| d.amount.clone()),
            false,
            suppress_hover,
            locale.clone(),
        ))
        .child(detail_field(
            theme,
            IconName::PenEdit,
            tk("transactionHistory.transactionDetail.fields.note"),
            value(|d| d.note.clone()),
            false,
            suppress_hover,
            locale.clone(),
        ));

    let right = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_4()
        .child(detail_field(
            theme,
            IconName::UserIcon,
            tk("transactionHistory.transactionDetail.fields.sender"),
            value(|d| d.sender.clone()),
            false,
            suppress_hover,
            locale.clone(),
        ))
        .child(detail_field(
            theme,
            IconName::UserIcon,
            tk("transactionHistory.transactionDetail.fields.receiver"),
            value(|d| d.receiver.clone()),
            false,
            suppress_hover,
            locale.clone(),
        ))
        .child(detail_field(
            theme,
            IconName::ClockIcon,
            tk("transactionHistory.transactionDetail.fields.created"),
            value(|d| d.created.clone()),
            false,
            suppress_hover,
            locale.clone(),
        ));

    div()
        .p_4()
        .bg(theme.tokens.bg_active_member_channel)
        .text_color(theme.tokens.text_theme_primary)
        .flex()
        .flex_row()
        .gap_4()
        .child(left)
        .child(right)
        .into_any_element()
}

fn field_icon(theme: &Theme, icon: IconName) -> AnyElement {
    match icon {
        IconName::DollarIcon | IconName::UserIcon => img(icon.path())
            .size(px(12.))
            .flex_none()
            .into_any_element(),
        _ => Icon::new(icon)
            .size(px(12.))
            .text_color(theme.tokens.text_theme_primary)
            .into_any_element(),
    }
}

fn detail_field(
    theme: &Theme,
    icon: IconName,
    label: String,
    value: SharedString,
    copyable: bool,
    suppress_hover: bool,
    locale: SharedString,
) -> AnyElement {
    let copy_value = value.clone();
    let mut header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(field_icon(theme, icon))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.tokens.text_theme_primary)
                .child(label),
        );

    if copyable && !copy_value.is_empty() {
        header = header.child(
            div()
                .id("tx-detail-copy")
                .p_1()
                .rounded(px(4.))
                .cursor_pointer()
                .when(!suppress_hover, |el| {
                    el.hover(|s| s.bg(theme.tokens.bg_item_theme_hover))
                })
                .on_click(move |_: &ClickEvent, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_value.to_string()));
                    let message =
                        mezon_i18n::t(&locale, "token.historyTransaction.copied").to_string();
                    Shell::global(cx).update(cx, |shell, cx| shell.success(message, cx));
                })
                .child(
                    Icon::new(IconName::CopyIcon)
                        .size(px(12.))
                        .text_color(theme.tokens.text_theme_primary),
                ),
        );
    }

    let body = if value.is_empty() {
        div()
            .ml_5()
            .h(px(20.))
            .w(px(128.))
            .rounded(px(4.))
            .bg(theme.bg_hover)
            .into_any_element()
    } else {
        div()
            .pl_5()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .child(value)
            .into_any_element()
    };

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(header)
        .child(body)
        .into_any_element()
}

fn render_skeletons(theme: &Theme) -> AnyElement {
    let fill = theme.bg_hover;
    let mut container = div().flex().flex_col().size_full().overflow_hidden();
    for _ in 0..SKELETON_ROWS {
        container = container.child(
            div().pb_4().child(
                div()
                    .h(px(ROW_HEIGHT))
                    .p_4()
                    .rounded(px(8.))
                    .bg(theme.tokens.bg_active_member_channel)
                    .border_1()
                    .border_color(theme.tokens.border_primary)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_4()
                    .child(div().size(px(32.)).flex_none().rounded_full().bg(fill))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(div().h(px(16.)).w(relative(0.75)).rounded(px(4.)).bg(fill))
                            .child(div().h(px(12.)).w(relative(0.5)).rounded(px(4.)).bg(fill)),
                    ),
            ),
        );
    }
    container.into_any_element()
}

fn render_placeholder(
    theme: &Theme,
    locale: &SharedString,
    filter: i32,
    error: &Option<SharedString>,
) -> AnyElement {
    let (title, description) = match error {
        Some(error) => (error.to_string(), String::new()),
        None if filter == FILTER_ALL => (
            mezon_i18n::t(
                locale,
                "transactionHistory.emptyStates.noTransactions.title",
            )
            .to_string(),
            mezon_i18n::t(
                locale,
                "transactionHistory.emptyStates.noTransactions.description",
            )
            .to_string(),
        ),
        None => (
            mezon_i18n::t(
                locale,
                "transactionHistory.emptyStates.noFilteredTransactions.title",
            )
            .to_string(),
            mezon_i18n::t(
                locale,
                "transactionHistory.emptyStates.noFilteredTransactions.description",
            )
            .to_string(),
        ),
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .py_12()
        .child(
            div()
                .size(px(64.))
                .flex_none()
                .mb_4()
                .rounded_full()
                .bg(theme.bg_hover)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(IconName::FileThumbEmpty.path())
                        .w(px(30.))
                        .h(px(40.))
                        .flex_none(),
                ),
        )
        .child(
            div()
                .mb_2()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tokens.text_theme_primary)
                .child(title),
        )
        .when(!description.is_empty(), |el| {
            el.child(
                div()
                    .max_w(px(384.))
                    .text_sm()
                    .text_center()
                    .text_color(theme.tokens.text_secondary)
                    .child(description),
            )
        })
        .into_any_element()
}

fn build_row(transaction: &WalletTransaction, locale: &SharedString) -> TxRow {
    let sign = if transaction.sent { "-" } else { "+" };
    let symbol = mezon_i18n::t(locale, "transactionHistory.currency.symbol");
    let kind_key = if transaction.sent {
        "transactionHistory.transactionTypes.sent"
    } else {
        "transactionHistory.transactionTypes.received"
    };
    let prefix = mezon_i18n::t(locale, "transactionHistory.transactionItem.idPrefix");
    TxRow {
        hash: transaction.hash.clone().into(),
        id_label: format!("{prefix}{}", id_suffix(&transaction.hash)).into(),
        amount: format!("{sign} {} {symbol}", format_amount(&transaction.value)).into(),
        kind_label: mezon_i18n::t(locale, kind_key).into(),
        date: format_date(transaction.timestamp).into(),
        sent: transaction.sent,
    }
}

fn build_detail(transaction: &WalletTransaction, locale: &SharedString, cx: &App) -> TxDetail {
    let symbol = mezon_i18n::t(locale, "transactionHistory.currency.symbol");
    let unknown = mezon_i18n::t(locale, "transactionHistory.transactionDetail.unknownUser");
    let default_note = mezon_i18n::t(locale, "transactionHistory.transactionDetail.defaultNote");
    let note = if transaction.note.is_empty() {
        default_note.to_string()
    } else {
        transaction.note.clone()
    };
    TxDetail {
        hash: transaction.hash.clone().into(),
        sender: resolve_username(transaction.sender_user_id.as_deref(), unknown, cx).into(),
        amount: format!("{} {symbol}", format_amount(&transaction.value)).into(),
        receiver: resolve_username(transaction.receiver_user_id.as_deref(), unknown, cx).into(),
        note: note.into(),
        created: format_date(transaction.timestamp).into(),
    }
}

fn resolve_username(user_id: Option<&str>, fallback: &'static str, cx: &App) -> String {
    let Some(store) = UsersByUserStore::try_global(cx) else {
        return fallback.to_string();
    };
    user_id
        .and_then(|id| id.parse::<UserId>().ok())
        .and_then(|id| store.read(cx).user(id).map(|user| user.username.clone()))
        .unwrap_or_else(|| fallback.to_string())
}

fn id_suffix(hash: &str) -> String {
    let chars: Vec<char> = hash.chars().collect();
    if chars.len() <= ID_SUFFIX_LEN {
        return hash.to_string();
    }
    chars[chars.len() - ID_SUFFIX_LEN..].iter().collect()
}

fn format_date(timestamp: i64) -> String {
    match Local.timestamp_opt(timestamp, 0).single() {
        Some(date) => date.format("%d/%m/%Y %H:%M").to_string(),
        None => String::new(),
    }
}

fn format_amount(value: &str) -> String {
    let scaled: i128 = value.trim().parse().unwrap_or(0);
    let integer = scaled / DECIMAL_FACTOR;
    if integer != 0 {
        return format_thousands(integer);
    }
    let fraction = (scaled % DECIMAL_FACTOR).unsigned_abs();
    if fraction == 0 {
        return integer.to_string();
    }
    let digits = format!("{fraction:0>width$}", width = DECIMALS as usize);
    let trimmed = digits.trim_end_matches('0');
    if trimmed.is_empty() {
        integer.to_string()
    } else {
        format!("{integer},{trimmed}")
    }
}

fn format_thousands(value: i128) -> String {
    let digits = value.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    if value < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_whole_token_amounts_with_thousand_separators() {
        assert_eq!(format_amount("1000000"), "1");
        assert_eq!(format_amount("1234567000000"), "1,234,567");
        assert_eq!(format_amount("0"), "0");
    }

    #[test]
    fn formats_sub_token_amounts_with_decimal_comma() {
        assert_eq!(format_amount("500000"), "0,5");
        assert_eq!(format_amount("1"), "0,000001");
    }

    #[test]
    fn id_suffix_takes_last_eight_characters() {
        assert_eq!(id_suffix("0123456789abcdef"), "89abcdef");
        assert_eq!(id_suffix("abc"), "abc");
    }
}
