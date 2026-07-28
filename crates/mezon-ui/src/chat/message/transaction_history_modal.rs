use gpui::{
    App, ClickEvent, Context, FocusHandle, Focusable, FontWeight, SharedString, Window, div,
    prelude::*, px, relative,
};
use mezon_store::WalletStore;

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName};
use crate::theme::ActiveTheme;

use mezon_store::TOKEN_DECIMAL_FACTOR as DECIMAL_FACTOR;
const PAGE_LIMIT: i64 = 50;
const FILTER_ALL: i32 = 0;
const FILTER_RECEIVED: i32 = 1;
const FILTER_SENT: i32 = 2;

const FILTER_TABS: [(i32, &str); 3] = [
    (FILTER_ALL, "token.history.all"),
    (FILTER_SENT, "token.history.sent"),
    (FILTER_RECEIVED, "token.history.received"),
];

#[derive(Clone)]
struct TxRow {
    sent: bool,
    amount: SharedString,
    counterparty: SharedString,
    note: SharedString,
    hash_short: SharedString,
}

pub struct TransactionHistoryModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    address: String,
    filter: i32,
    rows: Vec<TxRow>,
    loading: bool,
    error: Option<SharedString>,
    req_generation: u64,
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
            let mut this = Self {
                focus_handle: cx.focus_handle(),
                locale: locale.clone(),
                address: address.clone(),
                filter: FILTER_ALL,
                rows: Vec::new(),
                loading: !address.is_empty(),
                error: None,
                req_generation: 0,
            };
            if address.is_empty() {
                this.error = Some(mezon_i18n::t(&locale, "token.history.walletUnavailable").into());
            } else {
                this.fetch(cx);
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

    fn set_filter(&mut self, filter: i32, cx: &mut Context<Self>) {
        if self.filter == filter {
            return;
        }
        self.filter = filter;
        if !self.address.is_empty() {
            self.loading = true;
            self.error = None;
            self.fetch(cx);
        }
        cx.notify();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        self.req_generation = self.req_generation.wrapping_add(1);
        let generation = self.req_generation;
        let address = self.address.clone();
        let filter = self.filter;
        let task = WalletStore::global(cx).update(cx, |wallet, cx| {
            wallet.list_wallet_transactions(address, 1, PAGE_LIMIT, filter, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.req_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(transactions) => {
                        this.rows = transactions
                            .into_iter()
                            .map(|tx| TxRow {
                                sent: tx.sent,
                                amount: format_amount(&tx.value).into(),
                                counterparty: short_address(&tx.counterparty).into(),
                                note: tx.note.into(),
                                hash_short: short_address(&tx.hash).into(),
                            })
                            .collect();
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for TransactionHistoryModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let tk = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let active_filter = self.filter;

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p_4()
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(tk("token.history.title")),
            )
            .child(
                div()
                    .id("tx-history-close")
                    .size_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(18.))
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            );

        let mut tabs = div().flex().flex_row().items_center().gap_2().px_4().pb_2();
        for (filter, label_key) in FILTER_TABS {
            let is_active = filter == active_filter;
            tabs = tabs.child(
                div()
                    .id(("tx-history-tab", filter as usize))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .when(is_active, |d| {
                        d.bg(theme.tokens.bg_button_primary)
                            .text_color(theme.tokens.text_secondary)
                    })
                    .when(!is_active, |d| {
                        d.text_color(theme.tokens.text_theme_primary)
                            .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_filter(filter, cx);
                    }))
                    .child(tk(label_key)),
            );
        }

        let mut list = div()
            .id("tx-history-scroll")
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .pb_4()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();

        if self.loading {
            list = list.child(
                div()
                    .py_6()
                    .text_center()
                    .text_color(theme.tokens.text_secondary)
                    .child(tk("token.history.loading")),
            );
        } else if let Some(error) = &self.error {
            list = list.child(
                div()
                    .py_6()
                    .text_center()
                    .text_color(theme.tokens.text_secondary)
                    .child(error.clone()),
            );
        } else if self.rows.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .text_center()
                    .text_color(theme.tokens.text_secondary)
                    .child(tk("token.history.empty")),
            );
        } else {
            for (index, row) in self.rows.iter().enumerate() {
                let sign = if row.sent { "-" } else { "+" };
                let amount_color = if row.sent {
                    gpui::rgb(0xef4444)
                } else {
                    gpui::rgb(0x22c55e)
                };
                let direction = if row.sent { "To" } else { "From" };
                list = list.child(
                    div()
                        .id(("tx-history-row", index))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme.tokens.theme_setting_nav)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(format!("{direction} {}", row.counterparty)),
                                )
                                .when(!row.note.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.tokens.text_secondary)
                                            .child(row.note.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.tokens.text_secondary)
                                        .child(row.hash_short.clone()),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(amount_color)
                                .child(format!("{sign}{} ₫", row.amount)),
                        ),
                );
            }
        }

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_this, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .occlude()
            .w(px(420.))
            .max_h(relative(0.8))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(header)
            .child(tabs)
            .child(list)
    }
}

fn format_amount(value: &str) -> String {
    let scaled: i128 = value.trim().parse().unwrap_or(0);
    (scaled / DECIMAL_FACTOR).to_string()
}

fn short_address(address: &str) -> String {
    let chars: Vec<char> = address.chars().collect();
    if chars.len() <= 12 {
        return address.to_string();
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}
