use gpui::{AnyElement, FontWeight, SharedString, div, prelude::*, px, rgb};
use mezon_store::{CallLog, CallLogType, Message};

use super::coming_soon_toast;
use super::context::RowCtx;
use crate::components::primitives::{Icon, IconName};

const RED_500: u32 = 0xef_44_44;
const BLUE_500: u32 = 0x3b_82_f6;
const CARD_MAX_WIDTH: f32 = 520.0;

pub fn render_call_log_card(
    msg: &Message,
    call_log: &CallLog,
    is_me: bool,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let (icon, title, title_is_red) = call_log_header(msg, call_log, is_me, ctx);
    let sub_message = call_log_sub_message(msg, call_log, is_me, ctx);

    let mut header = div()
        .w_full()
        .border_b_1()
        .border_color(theme.tokens.border_color_primary)
        .px(px(20.))
        .py(px(16.))
        .flex()
        .flex_col()
        .gap(px(16.));

    let mut title_span = div().font_weight(FontWeight::SEMIBOLD).child(title);
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
            .child(div().text_size(px(14.)).child(sub_message)),
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
        let locale = SharedString::from(ctx.locale);
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
                .on_click(move |_, _, cx| coming_soon_toast(&locale, cx)),
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
    ctx: &RowCtx,
) -> (IconName, SharedString, bool) {
    let key = |name: &'static str| SharedString::from(mezon_i18n::t(ctx.locale, name));
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
    ctx: &RowCtx,
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
            mezon_i18n::t(ctx.locale, key).into()
        }
    }
}
