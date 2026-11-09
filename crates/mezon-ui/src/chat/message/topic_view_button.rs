use super::context::RowCtx;
use super::time::format_relative_time_from_seconds;
use crate::components::primitives::{Icon, IconName, default_user_avatar, unread_dot};
use gpui::{AnyElement, ObjectFit, SharedString, div, img, prelude::*, px};
use mezon_store::{ClanMembersStore, Message, TopicBadgeStore, TopicsStore};

const AVATAR_SIZE: f32 = 28.0;
const AVATAR_ROUNDING: f32 = 6.0;
const MIN_WIDTH: f32 = 250.0;

pub fn render_topic_view_button(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let theme = ctx.theme;

    let creator_avatar = msg
        .topic_creator_id
        .zip(ctx.clan_id)
        .and_then(|(user_id, clan_id)| {
            let store = ClanMembersStore::try_global(ctx.app)?;
            let store = store.read(ctx.app);
            store
                .member(clan_id, user_id)
                .map(|member| member.avatar().to_string())
        });
    let (creator_avatar, creator_proxy) = creator_avatar_sources(msg, creator_avatar);

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
    let topic_unread = msg.topic_id.is_some_and(|topic_id| {
        TopicBadgeStore::try_global(ctx.app)
            .is_some_and(|store| store.read(ctx.app).topic_has_unread(&topic_id.to_string()))
    });

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
        })
        .when(topic_unread, |d| d.child(unread_dot()));

    let left = div()
        .flex()
        .items_center()
        .gap_2()
        .flex_1()
        .min_w_0()
        .text_size(px(14.))
        .child(creator_avatar_element(&creator_avatar, creator_proxy, ctx))
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

fn creator_avatar_sources(
    msg: &Message,
    member_avatar: Option<String>,
) -> (String, Option<SharedString>) {
    if let Some(avatar) = member_avatar.filter(|avatar| !avatar.is_empty()) {
        return (avatar, None);
    }
    let sender_id = msg.sender_user_id.or_else(|| msg.sender_id.parse().ok());
    if msg
        .topic_creator_id
        .is_some_and(|creator| !creator.is_zero() && Some(creator) == sender_id)
    {
        return (
            msg.avatar_url.to_string(),
            (!msg.avatar_proxied.is_empty()).then(|| msg.avatar_proxied.clone()),
        );
    }
    (String::new(), None)
}

fn creator_avatar_element(
    avatar_url: &str,
    cached_proxy: Option<SharedString>,
    ctx: &RowCtx,
) -> AnyElement {
    let size = px(AVATAR_SIZE);
    let base = div()
        .size(size)
        .flex_shrink_0()
        .rounded(px(AVATAR_ROUNDING))
        .overflow_hidden();

    let proxied = cached_proxy.or_else(|| {
        (!avatar_url.is_empty())
            .then(|| crate::util::imgproxy::avatar_url(ctx.app, avatar_url).into())
    });
    let fallback = move || default_user_avatar(size, px(AVATAR_ROUNDING));

    if let Some(proxied) = proxied {
        let raw = SharedString::from(avatar_url.to_string());
        base.image_cache(ctx.avatar_cache.clone())
            .child(
                img(proxied.clone())
                    .id(("topic-creator-avatar", gpui::hash(&proxied)))
                    .size(size)
                    .rounded(px(AVATAR_ROUNDING))
                    .object_fit(ObjectFit::Cover)
                    .with_loading(fallback)
                    .with_fallback(move || {
                        if !raw.is_empty() && raw != proxied {
                            return img(raw.clone())
                                .id(("topic-creator-avatar-raw", gpui::hash(&raw)))
                                .size(size)
                                .rounded(px(AVATAR_ROUNDING))
                                .object_fit(ObjectFit::Cover)
                                .with_loading(fallback)
                                .with_fallback(fallback)
                                .into_any_element();
                        }
                        fallback()
                    }),
            )
            .into_any_element()
    } else {
        base.child(fallback()).into_any_element()
    }
}

#[cfg(test)]
mod avatar_tests {
    use super::creator_avatar_sources;
    use mezon_store::{ChannelId, Message, MessageId, UserId};

    fn topic(creator: Option<UserId>) -> Message {
        Message::new(MessageId(1), "hello", "10", "sender", 0)
            .with_avatar("https://cdn.example/sender.jpg")
            .with_avatar_proxied("https://proxy.example/sender.webp")
            .with_topic(Some(ChannelId(2)), creator)
    }

    #[test]
    fn another_creators_missing_avatar_never_uses_the_sender() {
        for member in [None, Some(String::new())] {
            let (raw, proxy) = creator_avatar_sources(&topic(Some(UserId(20))), member);
            assert!(raw.is_empty());
            assert!(proxy.is_none());
        }
    }

    #[test]
    fn unknown_creator_never_uses_the_sender() {
        for creator in [None, Some(UserId(0))] {
            assert_eq!(
                creator_avatar_sources(&topic(creator), None),
                (String::new(), None)
            );
        }
    }

    #[test]
    fn sender_avatar_is_used_when_sender_is_the_creator() {
        let message = topic(Some(UserId(10)));
        let (raw, proxy) = creator_avatar_sources(&message, None);
        assert_eq!(raw, message.avatar_url.as_ref());
        assert_eq!(proxy.as_ref(), Some(&message.avatar_proxied));
    }

    #[test]
    fn creator_profile_avatar_takes_priority_over_message_avatar() {
        for creator in [UserId(10), UserId(20)] {
            let avatar = "https://cdn.example/creator.jpg";
            let (raw, proxy) = creator_avatar_sources(&topic(Some(creator)), Some(avatar.into()));
            assert_eq!(raw, avatar);
            assert!(proxy.is_none());
        }
    }

    #[test]
    fn proxy_only_message_is_used_only_for_its_own_creator() {
        let mut message = topic(Some(UserId(20)));
        message.avatar_url = "".into();
        assert!(creator_avatar_sources(&message, None).1.is_none());
        message.topic_creator_id = Some(UserId(10));
        assert_eq!(
            creator_avatar_sources(&message, None).1,
            Some(message.avatar_proxied)
        );
    }
}
