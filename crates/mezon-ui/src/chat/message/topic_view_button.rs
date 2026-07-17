use super::context::RowCtx;
use super::parts::resolve_message_display_name;
use super::time::format_relative_time_from_seconds;
use crate::components::primitives::{Icon, IconName, avatar_color, name_initials};
use gpui::{AnyElement, App, ObjectFit, SharedString, div, img, prelude::*, px, rgb};
use mezon_store::{ClanMembersStore, Message, TopicsStore};

const AVATAR_SIZE: f32 = 28.0;
const AVATAR_ROUNDING: f32 = 6.0;
const MIN_WIDTH: f32 = 250.0;

pub fn render_topic_view_button(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;

    let display_name = resolve_message_display_name(msg, ctx, ctx.app);
    let (creator_name, creator_avatar) = msg
        .topic_creator_id
        .zip(ctx.clan_id)
        .and_then(|(user_id, clan_id)| {
            let store = ClanMembersStore::try_global(ctx.app)?;
            let store = store.read(ctx.app);
            store
                .member(clan_id, user_id)
                .map(|member| (member.name().to_string(), member.avatar().to_string()))
        })
        .unwrap_or_default();
    let creator_name = if creator_name.is_empty() {
        display_name.to_string()
    } else {
        creator_name
    };
    let creator_avatar = if creator_avatar.is_empty() {
        msg.avatar_url.to_string()
    } else {
        creator_avatar
    };

    let (reply_count, last_reply_timestamp) = msg
        .topic_id
        .map(|topic_id| {
            TopicsStore::try_global(ctx.app).map_or((0, None), |store| {
                store.read(ctx.app).topic_reply_summary(topic_id)
            })
        })
        .unwrap_or((0, None));

    let reply_label = (reply_count > 0).then(|| format_topic_reply_count(reply_count, ctx.locale));
    let time_label = last_reply_timestamp
        .map(|timestamp| format_relative_time_from_seconds(timestamp, ctx.locale, ctx.now))
        .filter(|label| !label.is_empty());

    let meta = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x_2()
        .flex_1()
        .min_w_0()
        .when_some(reply_label, |d, label| {
            d.child(div().text_color(theme.tokens.mention_color).child(label))
        })
        .when_some(time_label, |d, label| {
            d.child(
                div()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(label),
            )
        });

    let left = div()
        .flex()
        .items_center()
        .gap_2()
        .flex_1()
        .min_w_0()
        .text_size(px(14.))
        .child(creator_avatar_element(
            &creator_name,
            &creator_avatar,
            msg,
            ctx,
            ctx.app,
        ))
        .child(meta);

    let message_id = msg.id;
    let button = div()
        .id(("topic-view-button", msg.id.0 as usize))
        .flex()
        .items_center()
        .justify_between()
        .gap_1()
        .min_w(px(MIN_WIDTH))
        .my_1()
        .p_1()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.tokens.border_primary)
        .bg(theme.tokens.bg_item_theme_hover)
        .text_color(theme.tokens.text_theme_primary)
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            TopicsStore::global(cx).update(cx, |store, cx| {
                store.start_create_for_message(message_id, cx)
            });
        })
        .child(left)
        .child(
            Icon::new(IconName::ArrowRight)
                .size(px(16.))
                .text_color(theme.tokens.text_theme_primary),
        );

    div().flex().min_w_0().child(button).into_any_element()
}

fn format_topic_reply_count(rpl: i32, locale: &str) -> String {
    let number = if rpl > 99 {
        "99+".to_string()
    } else {
        rpl.to_string()
    };
    let key = if rpl == 1 {
        "message.reply"
    } else {
        "message.numberReplies"
    };
    mezon_i18n::t(locale, key).replace("{{number}}", &number)
}

fn creator_avatar_element(
    name: &str,
    avatar_url: &str,
    msg: &Message,
    ctx: &RowCtx,
    cx: &App,
) -> AnyElement {
    let size = px(AVATAR_SIZE);
    let base = div()
        .size(size)
        .flex_shrink_0()
        .rounded(px(AVATAR_ROUNDING))
        .overflow_hidden();

    let proxied = if !avatar_url.is_empty() {
        Some(crate::util::imgproxy::avatar_url(cx, avatar_url))
    } else if !msg.avatar_proxied.is_empty() {
        Some(msg.avatar_proxied.to_string())
    } else {
        None
    };

    if let Some(proxied) = proxied {
        base.image_cache(ctx.avatar_cache.clone())
            .child(
                img(SharedString::from(proxied))
                    .size(size)
                    .rounded(px(AVATAR_ROUNDING))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element()
    } else {
        base.flex()
            .items_center()
            .justify_center()
            .bg(avatar_color(name))
            .text_color(rgb(0xffffff))
            .text_size(px(12.))
            .child(name_initials(name))
            .into_any_element()
    }
}
