use gpui::{AnyElement, App, div, prelude::*};
use mezon_store::{Message, MessageCode};

use super::context::RowCtx;
use super::parts::render_date_divider;
use super::system_row::{
    render_onboard_welcome, render_system_message, render_unread_break, render_welcome,
};
use super::time::format_date_divider;
use super::user_row::render_user_message;

pub fn render_message_item(messages: &[Message], ix: usize, ctx: &RowCtx, cx: &App) -> AnyElement {
    let Some(msg) = messages.get(ix) else {
        return div().into_any_element();
    };
    let prev = ix.checked_sub(1).and_then(|p| messages.get(p));
    let show_separator = prev.is_some_and(|p| p.day_label.as_str() != msg.day_label.as_str());
    let combined = msg.combined_with_prev;
    let show_unread_break = ctx.unread_boundary_id.is_some_and(|id| id == msg.id);

    let row = match msg.code {
        MessageCode::Indicator => {
            let mut col = div().flex().flex_col().w_full();
            if ctx.onboarding.is_some() {
                col = col.child(render_onboard_welcome(ctx));
            }
            col.child(render_welcome(msg, ctx)).into_any_element()
        }
        code if code.is_system() => render_system_message(msg, ctx),
        _ => render_user_message(msg, combined, show_separator, ctx, cx),
    };

    if !show_separator && !show_unread_break {
        return row;
    }

    let mut stack = div().flex().flex_col().w_full();
    if show_separator {
        let label = format_date_divider(msg.create_time, ctx.locale, ctx.now);
        stack = stack.child(render_date_divider(ctx.theme, &label));
    }
    if show_unread_break {
        stack = stack.child(render_unread_break(ctx.locale));
    }
    stack.child(row).into_any_element()
}
