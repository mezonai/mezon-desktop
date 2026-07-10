use gpui::{
    AnyElement, App, MouseButton, MouseDownEvent, SharedString, div, prelude::*, px, rgb, rgba,
};
use mezon_store::{Message, MessageCode};

use super::call_log_card::render_call_log_card;
use super::coming_soon_toast;
use super::content::render_message_content;
use super::context::{
    AVATAR_LEFT, AVATAR_SIZE, CONTENT_INSET, CONTENT_RIGHT_PAD, DEFAULT_DISPLAY_NAME_COLOR, RowCtx,
};
use super::embed_card::render_embeds;
use super::message_actions_panel::render_actions_panel;
use super::ogp_embed::render_ogp_embed;
use super::parts::{
    avatar_element, render_attachments, render_head, render_hover_actions, render_reactions,
    render_reply,
};
use super::poll_card::render_poll_card;
use super::token_transaction_card::render_token_transaction_card;
use super::topic_view_button::render_topic_view_button;
use crate::chat::mention_input::MentionInput;
use crate::components::primitives::{Icon, IconName};

const GROUP_MARGIN_TOP: f32 = 10.;
const MESSAGE_ROW_MIN_HEIGHT: f32 = 30.;
const EPHEMERAL_BORDER: u32 = 0xa7_8b_fa;
const JUMP_HIGHLIGHT_RGBA: u32 = 0xea_b3_08_33;
const REPLY_HIGHLIGHT_RGBA: u32 = 0x94_9c_f7_14;
const TOPIC_CARD_MAX_WIDTH: f32 = 592.0;
const COMBINED_TIME_LEFT: f32 = 24.0;
const COMBINED_TIME_TOP: f32 = 4.0;

fn message_pad_top(combined: bool, has_reply: bool, code: MessageCode) -> bool {
    !combined || (has_reply && !matches!(code, MessageCode::CreatePin))
}

pub fn render_user_message(
    msg: &Message,
    combined: bool,
    is_different_day: bool,
    ctx: &RowCtx,
    cx: &App,
) -> AnyElement {
    let theme = ctx.theme;
    let has_reply = !msg.references.is_empty();
    let show_head = mezon_store::should_show_message_head(msg, combined);
    let row_key = msg.row_anchor_id.0;
    let ephemeral = msg.code == MessageCode::Ephemeral;
    let sending = msg.is_sending();
    let is_me = ctx.current_user_id == msg.sender_id.as_str();

    let mut body_column = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .mb_1()
        .pl(px(CONTENT_INSET))
        .pr(px(CONTENT_RIGHT_PAD));

    if show_head {
        body_column = body_column.child(render_head(msg, ctx, DEFAULT_DISPLAY_NAME_COLOR));
    }

    if msg.show_forwarded_label {
        body_column = body_column.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .w_full()
                .italic()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.tokens.text_theme_primary)
                .opacity(0.6)
                .child(
                    Icon::new(IconName::ForwardRightClick)
                        .size_4()
                        .text_color(theme.tokens.text_theme_primary),
                )
                .child(mezon_i18n::t(ctx.locale, "chat.forwarded")),
        );
    }

    let editing_input = (ctx.editing_id == Some(msg.id))
        .then(|| ctx.edit_input.clone())
        .flatten();
    let editing = editing_input.is_some();

    let content_element = if let Some(input) = editing_input {
        render_edit_box(msg.id, input, ctx)
    } else if let Some(call_log) = msg.call_log.as_ref() {
        render_call_log_card(msg, call_log, is_me, ctx)
    } else if msg.code == MessageCode::SendToken {
        render_token_transaction_card(msg, ctx)
    } else if msg.poll.is_some() {
        render_poll_card(msg, ctx)
    } else if sending {
        div()
            .w_full()
            .opacity(0.5)
            .child(render_message_content(msg, ctx))
            .into_any_element()
    } else {
        render_message_content(msg, ctx)
    };
    body_column = body_column.child(content_element);

    let shows_text_content =
        !editing && msg.call_log.is_none() && msg.code != MessageCode::SendToken;
    if ephemeral && shows_text_content {
        body_column = body_column.child(render_ephemeral_notice(ctx));
    }

    if let Some(ogp) = render_ogp_embed(msg, ctx) {
        body_column = body_column.child(ogp);
    }

    if let Some(attachments) = render_attachments(msg, ctx) {
        body_column = body_column.child(attachments);
    }

    if !msg.embeds.is_empty() {
        body_column = body_column.child(render_embeds(msg, ctx));
    }

    if msg.code == MessageCode::Topic {
        body_column = body_column.child(render_topic_view_button(msg, ctx));
    }

    if !msg.components.is_empty() {
        body_column = body_column.child(render_actions_panel(msg, ctx));
    }

    if !ephemeral
        && !sending
        && let Some(reactions) = render_reactions(msg, ctx)
    {
        body_column = body_column.child(reactions);
    }

    let body = div()
        .relative()
        .w_full()
        .when(msg.send_failed, |d| d.opacity(0.5))
        .when(msg.is_forwarded, |d| {
            d.child(
                div()
                    .absolute()
                    .left(px(58.))
                    .bottom_0()
                    .when(show_head, |b| b.top(px(50.)))
                    .when(!show_head, |b| b.top_0())
                    .w(px(4.))
                    .rounded(px(4.))
                    .bg(theme.tokens.text_theme_primary),
            )
        })
        .when_some(
            show_head.then(|| build_avatar_element(msg, ctx, cx)),
            |d, avatar_element| {
                d.child(
                    div()
                        .absolute()
                        .left(px(AVATAR_LEFT))
                        .top(px(2.))
                        .w(px(AVATAR_SIZE))
                        .h(px(AVATAR_SIZE))
                        .child(avatar_element),
                )
            },
        )
        .child(body_column);

    let hover_bg = theme.bg_hover;
    let highlighted = ctx.highlight_id.is_some_and(|id| id == msg.id);
    let reply_highlighted = !ephemeral && ctx.reply_highlight_id.is_some_and(|id| id == msg.id);
    let mentioned = !ephemeral
        && (msg.highlights_viewer_direct
            || mezon_store::message_row_highlight_roles(msg, ctx.current_role_ids));
    let mention_bg = theme.tokens.bg_highlight;
    let show_combined_time = !show_head && ctx.hovered_row == Some(msg.id);
    let combined_time = msg.time_hhmm.clone();
    let context_menu_id = msg.id;
    let context_menu_host = ctx.video_host.clone();
    let hover_host = ctx.video_host.clone();
    let hover_id = msg.id;
    let interactive = !ephemeral && !sending;
    div()
        .id(("msg-row", row_key as usize))
        .when(interactive, |d| {
            d.on_mouse_down(
                MouseButton::Right,
                move |event: &MouseDownEvent, _window, cx| {
                    let position = event.position;
                    let _ = context_menu_host.update(cx, |this, cx| {
                        this.open_context_menu(context_menu_id, position, cx);
                    });
                },
            )
        })
        .when(interactive && !ctx.suppress_hover, |d| {
            d.on_hover(move |hovered: &bool, _window, cx| {
                let entered = *hovered;
                let _ = hover_host.update(cx, |this, cx| this.set_row_hover(hover_id, entered, cx));
            })
        })
        .relative()
        .w_full()
        .min_h(px(MESSAGE_ROW_MIN_HEIGHT))
        .when(!combined, |d| d.mt(px(GROUP_MARGIN_TOP)))
        .when(message_pad_top(combined, has_reply, msg.code), |d| d.pt_3())
        .when(msg.is_card, |d| {
            d.max_w(px(TOPIC_CARD_MAX_WIDTH))
                .mx_2()
                .my_1()
                .p_3()
                .rounded(px(8.))
                .border_1()
                .border_color(theme.tokens.border_theme_primary)
                .bg(theme.tokens.bg_tertiary)
        })
        .when(ephemeral, |d| {
            d.bg(theme.tokens.bg_item_theme_hover)
                .border_l_4()
                .border_color(rgb(EPHEMERAL_BORDER))
        })
        .when(mentioned && !highlighted && !reply_highlighted, |d| {
            d.bg(mention_bg)
        })
        .when(reply_highlighted && !highlighted, |d| {
            d.bg(rgba(REPLY_HIGHLIGHT_RGBA))
        })
        .when(highlighted, |d| d.bg(rgba(JUMP_HIGHLIGHT_RGBA)))
        .when(interactive && !ctx.suppress_hover, |d| {
            d.hover(move |s| s.bg(hover_bg))
        })
        .when(show_combined_time, |d| {
            d.child(
                div()
                    .absolute()
                    .left(px(COMBINED_TIME_LEFT))
                    .top(px(COMBINED_TIME_TOP))
                    .text_size(px(11.))
                    .text_color(theme.tokens.text_theme_primary)
                    .child(combined_time),
            )
        })
        .when(has_reply && !ephemeral, |d| {
            d.child(render_reply(&msg.references[0], ctx))
        })
        .child(body)
        .when(interactive, |d| {
            d.child(render_hover_actions(
                msg,
                combined,
                has_reply,
                is_different_day,
                ctx,
            ))
        })
        .when(
            msg.is_card && !ephemeral && msg.code != MessageCode::Topic,
            |d| d.child(render_go_to_topic_footer(msg, ctx)),
        )
        .into_any_element()
}

fn render_ephemeral_notice(ctx: &RowCtx) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .mt_1()
        .mb_1()
        .italic()
        .text_size(px(12.))
        .text_color(ctx.theme.tokens.text_theme_primary)
        .opacity(0.6)
        .child(
            Icon::new(IconName::EyeClose)
                .size(px(12.))
                .text_color(ctx.theme.tokens.text_theme_primary),
        )
        .child(mezon_i18n::t(ctx.locale, "message.onlyVisibleToRecipient"))
        .into_any_element()
}

fn render_go_to_topic_footer(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;
    let hover_bg = theme.bg_hover;
    let locale = SharedString::from(ctx.locale);
    div()
        .id(("go-to-topic", msg.id.0 as usize))
        .flex()
        .items_center()
        .justify_between()
        .mt_3()
        .pt_3()
        .mx(px(-12.))
        .mb(px(-12.))
        .px_3()
        .py_2()
        .rounded_b(px(8.))
        .border_t_1()
        .border_color(theme.tokens.border_divider)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .on_click(move |_, _, cx| coming_soon_toast(&locale, cx))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(14.))
                .text_color(theme.tokens.text_theme_primary)
                .opacity(0.75)
                .child(
                    Icon::new(IconName::MessageSquareIcon)
                        .size(px(20.))
                        .text_color(theme.tokens.text_theme_primary),
                )
                .child(mezon_i18n::t(ctx.locale, "message.goToTopic")),
        )
        .child(
            Icon::new(IconName::ArrowRight)
                .size(px(16.))
                .text_color(theme.tokens.text_theme_primary)
                .opacity(0.5),
        )
        .into_any_element()
}

fn render_edit_box(
    message_id: mezon_store::MessageId,
    input: gpui::Entity<MentionInput>,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let save_host = ctx.video_host.clone();
    let cancel_host = ctx.video_host.clone();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .child(
            div()
                .w_full()
                .mt(px(5.))
                .rounded(px(8.))
                .border_1()
                .border_color(theme.tokens.border_theme_primary)
                .bg(theme.tokens.bg_surface)
                .child(input),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .text_xs()
                .text_color(theme.tokens.text_theme_primary)
                .child(div().pr(px(3.)).child("escape to"))
                .child(
                    div()
                        .id(("edit-cancel", message_id.0 as usize))
                        .pr(px(3.))
                        .cursor_pointer()
                        .text_color(gpui::rgb(0x3297ff))
                        .child("cancel")
                        .on_click(move |_, _, cx| {
                            let _ = cancel_host.update(cx, |this, cx| this.cancel_edit(cx));
                        }),
                )
                .child(div().pr(px(3.)).child("• enter to"))
                .child(
                    div()
                        .id(("edit-save", message_id.0 as usize))
                        .cursor_pointer()
                        .text_color(gpui::rgb(0x3297ff))
                        .child("save")
                        .on_click(move |_, window, cx| {
                            let _ = save_host.update(cx, |this, cx| this.save_edit(window, cx));
                        }),
                ),
        )
        .into_any_element()
}

fn build_avatar_element(msg: &Message, ctx: &RowCtx, cx: &App) -> AnyElement {
    let plain = avatar_element(msg, ctx, cx);
    let Some(profile_ctx) = ctx.profile_context else {
        return plain;
    };
    let Some(user_id) = msg.sender_user_id else {
        return plain;
    };
    let avatar_key = user_id.get() as usize;
    super::content::profile_popover_trigger(
        ("msg-avatar-trigger", avatar_key),
        user_id,
        profile_ctx,
        ctx,
        plain,
    )
    .size_full()
    .into_any_element()
}
