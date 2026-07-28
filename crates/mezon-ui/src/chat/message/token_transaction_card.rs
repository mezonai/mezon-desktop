use gpui::{AnyElement, FontWeight, SharedString, div, prelude::*, px, rgb};
use mezon_store::Message;

use super::content::{SelectableSectionCursor, SelectableTextContext};
use super::context::RowCtx;
use crate::components::primitives::{Icon, IconName};

const GREEN_600: u32 = 0x16_a3_4a;
const BLUE_500: u32 = 0x3b_82_f6;
const CARD_WIDTH: f32 = 300.0;
const DETAIL_MAX_WIDTH: f32 = 200.0;

pub fn render_token_transaction_card(
    msg: &Message,
    ctx: &RowCtx,
    base: usize,
    selection_context: &SelectableTextContext,
) -> AnyElement {
    let theme = ctx.theme;
    let (title, detail) = match msg.token_transaction.as_deref() {
        Some(tx) => (tx.title.clone(), tx.detail.clone()),
        None => msg.token_transaction_parts(),
    };
    let mut cursor = SelectableSectionCursor::new(base);
    let title_range = cursor.section(&title);
    let detail_range = cursor.section(&detail);

    let body = div()
        .p_3()
        .flex()
        .gap_2()
        .w_full()
        .child(
            div().w(px(50.)).child(
                Icon::new(IconName::Transaction)
                    .size(px(50.))
                    .text_color(rgb(GREEN_600)),
            ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .cursor(gpui::CursorStyle::IBeam)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .when_some(title_range, |div, range| {
                            div.child(selection_context.text_node(&title, range))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(div().mr(px(4.)).text_color(rgb(BLUE_500)).child("Detail:"))
                        .child(
                            div()
                                .cursor(gpui::CursorStyle::IBeam)
                                .truncate()
                                .max_w(px(DETAIL_MAX_WIDTH))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.tokens.text_theme_primary)
                                .when_some(detail_range, |div, range| {
                                    div.child(
                                        selection_context.end_truncated_text_node(&detail, range),
                                    )
                                }),
                        ),
                ),
        );

    let selection = ctx.selection.clone();
    let history_locale = SharedString::from(ctx.locale);
    let footer = div()
        .p_3()
        .flex()
        .justify_center()
        .bg(theme.tokens.theme_setting_nav)
        .rounded_b(px(6.))
        .child(
            div()
                .id(("token-transfer-history", msg.id.0 as usize))
                .cursor_pointer()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(15.))
                .text_color(rgb(BLUE_500))
                .child("Mezon transfer")
                .on_click(move |_, window, cx| {
                    if !selection.borrow().has_selection() {
                        super::TransactionHistoryModal::open(history_locale.clone(), window, cx);
                    }
                }),
        );

    div()
        .py_2()
        .w_full()
        .child(
            div()
                .w(px(CARD_WIDTH))
                .border_1()
                .border_color(theme.tokens.border_primary)
                .rounded(px(6.))
                .bg(theme.tokens.theme_setting_primary)
                .shadow_md()
                .child(body)
                .child(footer),
        )
        .into_any_element()
}

pub(crate) fn selectable_token_transaction_text(msg: &Message) -> String {
    let (title, detail) = match msg.token_transaction.as_deref() {
        Some(tx) => (tx.title.as_ref(), tx.detail.as_ref()),
        None => {
            let (title, detail) = msg.token_transaction_parts();
            return if detail.is_empty() {
                title.to_string()
            } else {
                format!("{title}\n{detail}")
            };
        }
    };
    if detail.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n{detail}")
    }
}
