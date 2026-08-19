use gpui::{AnyElement, FontWeight, SharedString, div, prelude::*, px, rgb};
use mezon_store::{CallLog, CallLogType, Message};

use super::content::{SelectableSectionCursor, SelectableTextContext};
use super::context::RowCtx;
use crate::chat::call_actions::call_current_dm;
use crate::components::primitives::{Icon, IconName};

const RED_500: u32 = 0xef_44_44;
const BLUE_500: u32 = 0x3b_82_f6;
const CARD_MAX_WIDTH: f32 = 520.0;

pub fn render_call_log_card(
    msg: &Message,
    call_log: &CallLog,
    is_me: bool,
    ctx: &RowCtx,
    base: usize,
    selection_context: &SelectableTextContext,
) -> AnyElement {
    let theme = ctx.theme;
    let (icon, title, title_is_red) = call_log_header(msg, call_log, is_me, ctx.locale);
    let sub_message = call_log_sub_message(msg, call_log, is_me, ctx.locale);
    let mut cursor = SelectableSectionCursor::new(base);
    let title_range = cursor.section(&title);
    let sub_message_range = cursor.section(&sub_message);

    let mut header = div()
        .w_full()
        .border_b_1()
        .border_color(theme.tokens.border_color_primary)
        .px(px(20.))
        .py(px(16.))
        .flex()
        .flex_col()
        .gap(px(16.));

    let mut title_span = div()
        .cursor(gpui::CursorStyle::IBeam)
        .font_weight(FontWeight::SEMIBOLD)
        .when_some(title_range, |div, range| {
            div.child(selection_context.text_node(&title, range))
        });
    if title_is_red {
        title_span = title_span.text_color(rgb(RED_500));
    }
    header = header.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(title_span),
    );
    header = header.child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Icon::new(icon)
                    .size(px(24.))
                    .text_color(theme.tokens.text_theme_primary),
            )
            .child(
                div()
                    .cursor(gpui::CursorStyle::IBeam)
                    .text_size(px(14.))
                    .when_some(sub_message_range, |div, range| {
                        div.child(selection_context.text_node(&sub_message, range))
                    }),
            ),
    );

    let mut card = div()
        .max_w(px(CARD_MAX_WIDTH))
        .border_1()
        .border_color(theme.tokens.border_primary)
        .bg(theme.tokens.bg_active_member_channel)
        .rounded(px(8.))
        .shadow_lg()
        .child(header);

    if call_log.show_call_back {
        let label = mezon_i18n::t(ctx.locale, "message.callLog.CALL_BACK").to_uppercase();
        let video = call_log.is_video;
        let selection = ctx.selection.clone();
        card = card.child(
            div()
                .id(("call-log-callback", msg.id.0 as usize))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .p_3()
                .cursor_pointer()
                .text_color(rgb(BLUE_500))
                .child(label)
                .on_click(move |_, _, cx| {
                    if !selection.borrow().has_selection() {
                        call_current_dm(video, cx);
                    }
                }),
        );
    }

    div()
        .flex()
        .w_full()
        .relative()
        .mt_2()
        .rounded(px(8.))
        .text_color(theme.tokens.text_theme_primary)
        .child(card)
        .into_any_element()
}

fn call_log_header(
    msg: &Message,
    call_log: &CallLog,
    is_me: bool,
    locale: &str,
) -> (IconName, SharedString, bool) {
    let key = |name: &'static str| SharedString::from(mezon_i18n::t(locale, name));
    match (call_log.log_type, is_me) {
        (CallLogType::TimeoutCall, true) => (
            IconName::OutGoingCall,
            key("message.callLog.OUTGOING_CALL"),
            true,
        ),
        (CallLogType::TimeoutCall, false) => {
            (IconName::MissedCall, key("message.callLog.MISSED"), true)
        }
        (CallLogType::FinishCall, true) => (
            IconName::OutGoingCall,
            key("message.callLog.OUTGOING_CALL"),
            false,
        ),
        (CallLogType::FinishCall, false) => (
            IconName::IncomingCall,
            key("message.callLog.INCOMING_CALL"),
            false,
        ),
        (CallLogType::RejectCall, true) => (
            IconName::CancelCall,
            key("message.callLog.receiverRejected"),
            true,
        ),
        (CallLogType::RejectCall, false) => (
            IconName::CancelCall,
            key("message.callLog.youRejected"),
            true,
        ),
        (CallLogType::CancelCall, true) => {
            (IconName::CancelCall, key("message.callLog.cancel"), true)
        }
        (CallLogType::CancelCall, false) => {
            (IconName::MissedCall, key("message.callLog.MISSED"), true)
        }
        _ => (
            IconName::OutGoingCall,
            started_call_title(msg, call_log),
            false,
        ),
    }
}

fn started_call_title(msg: &Message, call_log: &CallLog) -> SharedString {
    let kind = if call_log.is_video {
        "a video"
    } else {
        "an audio"
    };
    format!("{} started {} call", msg.sender_name, kind).into()
}

fn call_log_sub_message(
    msg: &Message,
    call_log: &CallLog,
    is_me: bool,
    locale: &str,
) -> SharedString {
    match call_log.log_type {
        CallLogType::FinishCall => msg.content.clone().into(),
        CallLogType::TimeoutCall if is_me => "0 mins 0 secs".into(),
        _ => {
            let key = if call_log.is_video {
                "message.callLog.VIDEO_CALL"
            } else {
                "message.callLog.VOICE_CALL"
            };
            mezon_i18n::t(locale, key).into()
        }
    }
}

pub(crate) fn selectable_call_log_text(
    msg: &Message,
    call_log: &CallLog,
    is_me: bool,
    locale: &str,
) -> String {
    let (_, title, _) = call_log_header(msg, call_log, is_me, locale);
    let sub_message = call_log_sub_message(msg, call_log, is_me, locale);
    if sub_message.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n{sub_message}")
    }
}
