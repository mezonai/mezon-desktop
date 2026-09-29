use gpui::{
    AnyElement, FontWeight, HighlightStyle, Rgba, SharedString, StyledText, div, prelude::*, px,
    rgb,
};
use mezon_store::{CommandInvocation, CommandStatus, Message, MessageId, MessagesStore};

use super::context::RowCtx;
use super::user_row::EPHEMERAL_BORDER;
use crate::components::primitives::{Icon, IconName};

const CARD_MAX_WIDTH: f32 = 520.;
const CARD_RADIUS: f32 = 8.;
const MONO: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};
const STATUS_INDENT: f32 = 20.;

pub fn render_command_card(msg: &Message, command: &CommandInvocation, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let card = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .w_full()
        .max_w(px(CARD_MAX_WIDTH))
        .pl(px(12.))
        .pr(px(6.))
        .pt(px(6.))
        .pb(px(8.))
        .rounded(px(CARD_RADIUS))
        .bg(theme.tokens.bg_markdown_code)
        .border_1()
        .border_color(theme.tokens.border_primary)
        .child(render_command_line(msg, command, ctx))
        .child(render_status_line(msg, command, ctx))
        .child(render_footer(ctx));
    div()
        .id(("command-card", msg.row_anchor_id.0 as usize))
        .w_full()
        .mt_1()
        .child(card)
        .into_any_element()
}

fn render_command_line(msg: &Message, command: &CommandInvocation, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let message_id = msg.id;
    let hover_bg = theme.bg_hover;
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(8.))
        .min_w_0()
        .font_family(MONO)
        .text_size(px(13.5))
        .child(
            div().flex_none().h(px(20.)).flex().items_center().child(
                Icon::new(IconName::ChevronRight)
                    .size(px(14.))
                    .text_color(theme.tokens.mention_color),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .line_height(px(20.))
                .text_color(theme.tokens.text_theme_message)
                .child(command_text(command, ctx)),
        )
        .child(
            div()
                .id(("command-dismiss", msg.row_anchor_id.0 as usize))
                .flex_none()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .child(
                    Icon::new(IconName::Close)
                        .size(px(12.))
                        .text_color(theme.text_muted),
                )
                .on_click(move |_, _, cx| {
                    MessagesStore::global(cx)
                        .update(cx, |store, cx| store.dismiss_local_message(message_id, cx));
                }),
        )
        .into_any_element()
}

fn command_text(command: &CommandInvocation, ctx: &RowCtx) -> StyledText {
    let name = format!("/{}", command.menu_name);
    let name_range = 0..name.len();
    let text = if command.arguments.is_empty() {
        name
    } else {
        format!("{name} {}", command.arguments)
    };
    StyledText::new(text).with_highlights(vec![(
        name_range,
        HighlightStyle {
            color: Some(ctx.theme.tokens.mention_color.into()),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        },
    )])
}

enum StatusGlyph {
    Text(&'static str),
    Icon(IconName),
}

fn render_status_line(msg: &Message, command: &CommandInvocation, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let bot = if command.bot_name.is_empty() {
        mezon_i18n::t(ctx.locale, "message.command.theBot").to_string()
    } else {
        command.bot_name.to_string()
    };
    let capitalize = command.bot_name.is_empty();
    let text = |key: &'static str| -> SharedString {
        let label = mezon_i18n::t(ctx.locale, key).replace("{{bot}}", &bot);
        if capitalize {
            capitalize_first(&label).into()
        } else {
            label.into()
        }
    };
    let row_key = msg.row_anchor_id.0 as usize;
    let message_id = msg.id;
    let (glyph, color, label) = match command.status {
        CommandStatus::Waiting => (
            StatusGlyph::Text("…"),
            rgb(EPHEMERAL_BORDER),
            text("message.command.waiting"),
        ),
        CommandStatus::Answered(_) => (
            StatusGlyph::Icon(IconName::Check),
            theme.status_online,
            text("message.command.answered"),
        ),
        CommandStatus::NoResponse => (
            StatusGlyph::Text("!"),
            theme.status_idle,
            text("message.command.noResponse"),
        ),
        CommandStatus::Failed => (
            StatusGlyph::Icon(IconName::Close),
            theme.danger_text,
            text("message.command.failed"),
        ),
    };
    let label_color = if matches!(command.status, CommandStatus::Waiting) {
        theme.text_muted
    } else {
        theme.tokens.text_theme_message
    };
    let action = match command.status {
        CommandStatus::Answered(reply_id) => Some(
            render_terminal_button(
                ("command-view-reply", row_key),
                text("message.command.viewReply"),
                Some(IconName::ArrowDown),
                ctx,
            )
            .on_click(move |_, _, cx| {
                MessagesStore::global(cx)
                    .update(cx, |store, cx| store.jump_to_message(reply_id, None, cx));
            })
            .into_any_element(),
        ),
        CommandStatus::NoResponse | CommandStatus::Failed if command.resendable => {
            let key = if matches!(command.status, CommandStatus::Failed) {
                "message.command.retry"
            } else {
                "message.command.resend"
            };
            Some(render_resend_button(message_id, row_key, text(key), ctx))
        }
        _ => None,
    };
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
        .pl(px(STATUS_INDENT))
        .font_family(MONO)
        .text_size(px(13.))
        .child(render_glyph(glyph, color))
        .child(div().min_w_0().text_color(label_color).child(label))
        .children(action)
        .into_any_element()
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn render_glyph(glyph: StatusGlyph, color: Rgba) -> AnyElement {
    let slot = div().flex_none().w(px(12.)).flex().justify_center();
    match glyph {
        StatusGlyph::Text(glyph) => slot
            .font_weight(FontWeight::BOLD)
            .text_color(color)
            .child(glyph),
        StatusGlyph::Icon(icon) => slot.child(Icon::new(icon).size(px(12.)).text_color(color)),
    }
    .into_any_element()
}

fn render_terminal_button(
    id: (&'static str, usize),
    label: SharedString,
    icon: Option<IconName>,
    ctx: &RowCtx,
) -> gpui::Stateful<gpui::Div> {
    let theme = ctx.theme;
    let hover_bg = theme.bg_hover;
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .px(px(7.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.tokens.border_primary)
        .text_size(px(12.5))
        .text_color(theme.tokens.text_theme_message)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(label)
        .when_some(icon, |d, icon| {
            d.child(
                Icon::new(icon)
                    .size(px(11.))
                    .text_color(theme.tokens.text_theme_message),
            )
        })
}

fn render_resend_button(
    message_id: MessageId,
    row_key: usize,
    label: SharedString,
    ctx: &RowCtx,
) -> AnyElement {
    render_terminal_button(("command-resend", row_key), label, None, ctx)
        .on_click(move |_, _, cx| {
            MessagesStore::global(cx).update(cx, |store, cx| store.resend_command(message_id, cx));
        })
        .into_any_element()
}

fn render_footer(ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    div()
        .flex()
        .items_center()
        .gap_1()
        .pt(px(2.))
        .text_size(px(11.5))
        .text_color(theme.text_muted)
        .child(
            Icon::new(IconName::EyeClose)
                .size(px(11.))
                .text_color(theme.text_muted),
        )
        .child(mezon_i18n::t(ctx.locale, "message.onlyVisibleToYou"))
        .into_any_element()
}
