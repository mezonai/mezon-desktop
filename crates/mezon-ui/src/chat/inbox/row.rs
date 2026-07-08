use chrono::{Local, TimeZone};
use gpui::{App, Entity, FontWeight, SharedString, div, img, prelude::*, px, rgb};
use mezon_store::{
    Channel, ChannelId, ChannelList, ChannelType, ClanId, ClanList, InboxCategory,
    InboxMentionSpan, InboxNotification, ProfileContext, TopicDiscussion, UserId, UsersByUserStore,
    attachment_link_is_image, message_content_is_attachment, resolve_user_profile,
};

use crate::components::primitives::{Avatar, Sizable, Size, h_flex, v_flex};
use crate::image_cache::LruImageCache;
use crate::theme::Theme;

const DISPLAY_NAME_COLOR: u32 = 0x17ac86;
pub const FOR_YOU_ROW_HEIGHT: f32 = 76.;
pub const MENTION_ROW_HEIGHT: f32 = 120.;
pub const MESSAGE_ROW_HEIGHT: f32 = 128.;
pub const TOPIC_ROW_HEIGHT: f32 = 96.;
pub const ROW_HEIGHT: f32 = MENTION_ROW_HEIGHT;

#[derive(Clone)]
pub(crate) struct MentionBreadcrumb {
    clan_name: SharedString,
    category_name: SharedString,
    channel_label: SharedString,
    thread_label: Option<SharedString>,
}

#[derive(Clone)]
pub(crate) struct ForYouLine {
    display_name: SharedString,
    subject_suffix: SharedString,
}

#[derive(Clone)]
pub(crate) struct NotificationRowView {
    sender_name: SharedString,
    avatar_url: Option<SharedString>,
    time_label: SharedString,
    mention_breadcrumb: Option<MentionBreadcrumb>,
    show_direct_message: bool,
    messages_clan_name: Option<SharedString>,
    for_you_line: Option<ForYouLine>,
    body_text: SharedString,
    body_is_attachment: bool,
    mention_spans: Vec<InboxMentionSpan>,
    attachment_link: String,
    attachment_type: String,
    has_more_attachment: bool,
}

#[derive(Clone)]
pub(crate) struct TopicRowView {
    avatar_name: SharedString,
    avatar_url: Option<SharedString>,
    reply_preview: SharedString,
    reply_is_attachment: bool,
}

fn format_inbox_time(ts: u32, locale: &SharedString) -> String {
    if ts == 0 {
        return String::new();
    }
    let Some(dt) = Local.timestamp_opt(ts as i64, 0).single() else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let date = dt.date_naive();
    let time = dt.format("%H:%M").to_string();
    if date == today {
        mezon_i18n::t(locale, "common.todayAtTime").replace("{{time}}", &time)
    } else if date == today.pred_opt().unwrap_or(today) {
        mezon_i18n::t(locale, "common.yesterdayAtTime").replace("{{time}}", &time)
    } else {
        dt.format("%d/%m/%Y, %H:%M").to_string()
    }
}

fn resolve_for_you_profile(
    notification: &InboxNotification,
    sender_id: &str,
    cx: &App,
) -> (String, String, Option<SharedString>, String) {
    let message = notification.message.as_ref();
    let user_id = sender_id.parse::<UserId>().ok();
    let store_user = user_id.and_then(|uid| UsersByUserStore::global(cx).read(cx).user(uid));
    let clan_profile = notification
        .clan_id
        .parse::<ClanId>()
        .ok()
        .and_then(|clan| {
            user_id.and_then(|uid| resolve_user_profile(uid, ProfileContext::Clan(clan), cx))
        });

    let username = message
        .map(|m| m.username.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| store_user.map(|user| user.username.clone()))
        .or_else(|| {
            clan_profile
                .as_ref()
                .map(|profile| profile.username.clone())
        })
        .unwrap_or_default();

    let display_name = store_user
        .map(|user| {
            if user.display_name.is_empty() {
                user.username.clone()
            } else {
                user.display_name.clone()
            }
        })
        .or_else(|| message.map(sender_display_name))
        .or_else(|| {
            clan_profile.as_ref().map(|profile| {
                if profile.display_name.is_empty() {
                    profile.username.clone()
                } else {
                    profile.display_name.clone()
                }
            })
        })
        .filter(|name| !name.is_empty())
        .or_else(|| (!username.is_empty()).then(|| username.clone()))
        .unwrap_or_default();

    let avatar = message
        .and_then(|m| (!m.avatar.is_empty()).then(|| m.avatar.clone()))
        .or_else(|| (!notification.avatar_url.is_empty()).then(|| notification.avatar_url.clone()))
        .or_else(|| {
            store_user
                .map(|user| user.avatar_url.clone())
                .filter(|url| !url.is_empty())
        })
        .or_else(|| {
            clan_profile
                .map(|profile| profile.avatar_url)
                .filter(|url| !url.is_empty())
        });

    let subject_suffix = if !username.is_empty() && notification.subject.starts_with(&username) {
        notification.subject[username.len()..].to_string()
    } else {
        notification.subject.clone()
    };

    (
        display_name,
        username,
        avatar_from(avatar.as_deref().unwrap_or("")),
        subject_suffix,
    )
}

fn find_mention_channel(clan_id: ClanId, channel_id: ChannelId, cx: &App) -> Option<Channel> {
    let list = ChannelList::global(cx).read(cx);
    if let Some(channel) = list.channel(clan_id, channel_id) {
        return Some(channel.clone());
    }
    if let Some(channel) = list.find_channel_in_active_clan(channel_id) {
        return Some(channel.clone());
    }
    list.clan_id_for_channel(channel_id)
        .and_then(|resolved_clan| list.channel(resolved_clan, channel_id).cloned())
}

fn clan_name(clan_id: ClanId, cx: &App) -> Option<String> {
    ClanList::global(cx)
        .read(cx)
        .clans
        .iter()
        .find(|c| c.id == clan_id)
        .map(|c| c.name.clone())
}

fn resolve_sender(
    clan_id: ClanId,
    sender_id: &str,
    fallback_avatar: &str,
    fallback_name: &str,
    cx: &App,
) -> (SharedString, Option<SharedString>, bool) {
    let Ok(user_id) = sender_id.parse::<UserId>() else {
        return (fallback_name.into(), avatar_from(fallback_avatar), false);
    };
    if let Some(profile) = resolve_user_profile(user_id, ProfileContext::Clan(clan_id), cx) {
        let name = if profile.display_name.is_empty() {
            profile.username.clone()
        } else {
            profile.display_name.clone()
        };
        let avatar = if profile.avatar_url.is_empty() {
            avatar_from(fallback_avatar)
        } else {
            Some(profile.avatar_url.into())
        };
        return (name.into(), avatar, !profile.role_ids.is_empty());
    }
    (fallback_name.into(), avatar_from(fallback_avatar), false)
}

fn sender_display_name(message: &mezon_store::InboxMessagePreview) -> String {
    if !message.display_name.is_empty() {
        message.display_name.clone()
    } else if !message.username.is_empty() {
        message.username.clone()
    } else {
        String::new()
    }
}

fn avatar_from(url: &str) -> Option<SharedString> {
    if url.is_empty() {
        None
    } else {
        Some(url.into())
    }
}

fn is_direct_message_mention(notification: &InboxNotification, cx: &App) -> bool {
    match notification
        .effective_clan_id()
        .and_then(|id| id.parse::<ClanId>().ok())
    {
        None => true,
        Some(clan_id) => clan_name(clan_id, cx).is_none(),
    }
}

fn build_mention_breadcrumb(
    notification: &InboxNotification,
    cx: &App,
) -> Option<MentionBreadcrumb> {
    let clan_id = notification
        .effective_clan_id()
        .and_then(|id| id.parse::<ClanId>().ok())?;
    let channel_id = notification
        .effective_channel_id()
        .and_then(|id| id.parse::<ChannelId>().ok())?;
    let clan = clan_name(clan_id, cx)?;
    let Some(channel) = find_mention_channel(clan_id, channel_id, cx) else {
        return Some(MentionBreadcrumb {
            clan_name: clan.to_uppercase().into(),
            category_name: SharedString::default(),
            channel_label: SharedString::default(),
            thread_label: None,
        });
    };
    let is_channel = channel.channel_type != ChannelType::Thread;
    if is_channel {
        return Some(MentionBreadcrumb {
            clan_name: clan.to_uppercase().into(),
            category_name: channel.category_name.to_uppercase().into(),
            channel_label: format!("#{}", channel.name).into(),
            thread_label: None,
        });
    }
    let parent = channel
        .parent_id
        .and_then(|parent_id| find_mention_channel(clan_id, parent_id, cx))?;
    Some(MentionBreadcrumb {
        clan_name: clan.to_uppercase().into(),
        category_name: parent.category_name.to_uppercase().into(),
        channel_label: format!("#{}", parent.name).into(),
        thread_label: Some(channel.name.clone().into()),
    })
}

pub(crate) fn build_notification_row_view(
    notification: &InboxNotification,
    locale: &SharedString,
    cx: &App,
) -> NotificationRowView {
    let clan_id = notification
        .effective_clan_id()
        .and_then(|id| id.parse::<ClanId>().ok());
    let sender_id = notification
        .message
        .as_ref()
        .map(|m| m.sender_id.as_str())
        .filter(|id| !id.is_empty() && *id != "0")
        .unwrap_or(notification.sender_id.as_str());
    let fallback_avatar = notification
        .message
        .as_ref()
        .map(|m| m.avatar.as_str())
        .filter(|a| !a.is_empty())
        .unwrap_or(notification.avatar_url.as_str());

    let fallback_name = notification
        .message
        .as_ref()
        .map(sender_display_name)
        .unwrap_or_default();

    let (mut sender_name, mut avatar_url, _) = clan_id
        .map(|clan| resolve_sender(clan, sender_id, fallback_avatar, &fallback_name, cx))
        .unwrap_or((
            fallback_name.clone().into(),
            avatar_from(fallback_avatar),
            false,
        ));
    if sender_name.is_empty() && !fallback_name.is_empty() {
        sender_name = fallback_name.into();
    }

    let message_ts = notification.message_timestamp();
    let time_label = format_inbox_time(message_ts, locale).into();

    let message_preview = notification.message.as_ref();
    let body_is_attachment = message_preview.is_some_and(|m| {
        message_content_is_attachment(&m.raw_content) || message_content_is_attachment(&m.content)
    });
    let body_text = if body_is_attachment {
        SharedString::default()
    } else {
        message_preview
            .map(|m| m.body_text())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| notification.preview_text())
            .into()
    };
    let mention_spans = message_preview
        .map(|m| m.mention_spans_for_render())
        .unwrap_or_default();
    let attachment_link = message_preview
        .map(|m| m.attachment_link.clone())
        .unwrap_or_default();
    let attachment_type = message_preview
        .map(|m| m.attachment_type.clone())
        .unwrap_or_default();
    let has_more_attachment = message_preview.is_some_and(|m| m.has_more_attachment);

    let mention_breadcrumb = if notification.category == InboxCategory::Mentions {
        build_mention_breadcrumb(notification, cx)
    } else {
        None
    };
    let show_direct_message = notification.category == InboxCategory::Mentions
        && is_direct_message_mention(notification, cx);

    let messages_clan_name = match notification.category {
        InboxCategory::Messages => clan_id.and_then(|id| clan_name(id, cx)).map(Into::into),
        _ => None,
    };

    let for_you_line = if notification.category == InboxCategory::ForYou {
        let (display_name, _, avatar, subject_suffix) =
            resolve_for_you_profile(notification, sender_id, cx);
        if avatar.is_some() {
            avatar_url = avatar;
        }
        if !display_name.is_empty() {
            sender_name = display_name.clone().into();
        }
        Some(ForYouLine {
            display_name: display_name.into(),
            subject_suffix: subject_suffix.into(),
        })
    } else {
        None
    };

    NotificationRowView {
        sender_name,
        avatar_url,
        time_label,
        mention_breadcrumb,
        show_direct_message,
        messages_clan_name,
        for_you_line,
        body_text,
        body_is_attachment,
        mention_spans,
        attachment_link,
        attachment_type,
        has_more_attachment,
    }
}

pub(crate) fn build_topic_row_view(topic: &TopicDiscussion, cx: &App) -> TopicRowView {
    let clan_id = topic.clan_id.parse::<ClanId>().ok();
    let sender_id = if topic.last_sender_id.is_empty() {
        topic.creator_id.as_str()
    } else {
        topic.last_sender_id.as_str()
    };
    let (avatar_name, avatar_url, _) = clan_id
        .map(|clan| resolve_sender(clan, sender_id, "", "", cx))
        .unwrap_or((SharedString::default(), None, false));
    let reply_is_attachment = topic.reply_is_attachment();
    let reply_preview = if reply_is_attachment {
        SharedString::default()
    } else {
        topic.reply_preview_text().into()
    };
    TopicRowView {
        avatar_name,
        avatar_url,
        reply_preview,
        reply_is_attachment,
    }
}

fn render_avatar(
    name: &SharedString,
    url: Option<&SharedString>,
    size: Size,
    avatar_cache: Entity<LruImageCache>,
) -> impl IntoElement {
    let mut avatar = Avatar::new()
        .name(name.clone())
        .with_size(size)
        .image_cache(avatar_cache);
    if let Some(src) = url.filter(|s| !s.is_empty()) {
        avatar = avatar.src(src.clone());
    }
    avatar
}

fn render_mention_breadcrumb(theme: &Theme, breadcrumb: &MentionBreadcrumb) -> impl IntoElement {
    let has_category = !breadcrumb.category_name.is_empty();
    let has_channel = !breadcrumb.channel_label.is_empty();
    v_flex()
        .gap(px(2.))
        .child(
            h_flex()
                .gap_1()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(
                    div()
                        .max_w(px(120.))
                        .overflow_hidden()
                        .child(breadcrumb.clan_name.clone()),
                )
                .when(has_category, |row| {
                    row.child(">").child(
                        div()
                            .max_w(px(130.))
                            .overflow_hidden()
                            .child(breadcrumb.category_name.clone()),
                    )
                }),
        )
        .when(has_channel, |col| {
            col.child(
                h_flex()
                    .gap_1()
                    .text_sm()
                    .text_color(theme.text_primary)
                    .child(
                        div()
                            .max_w(px(120.))
                            .overflow_hidden()
                            .child(breadcrumb.channel_label.clone()),
                    )
                    .when_some(breadcrumb.thread_label.clone(), |row, thread| {
                        row.child(">")
                            .child(div().max_w(px(130.)).overflow_hidden().child(thread))
                    }),
            )
        })
}

fn utf16_offset_to_byte(text: &str, utf16_offset: usize) -> usize {
    let mut utf16_count = 0;
    for (byte_idx, ch) in text.char_indices() {
        if utf16_count >= utf16_offset {
            return byte_idx;
        }
        utf16_count += ch.len_utf16();
    }
    text.len()
}

fn mention_byte_range(text: &str, start: i32, end: i32) -> Option<(usize, usize)> {
    let start = start.max(0) as usize;
    let end = end.max(start as i32) as usize;
    let byte_start = utf16_offset_to_byte(text, start);
    let byte_end = utf16_offset_to_byte(text, end);
    (byte_start <= byte_end && byte_end <= text.len()).then_some((byte_start, byte_end))
}

fn render_message_content(
    theme: &Theme,
    text: &SharedString,
    spans: &[InboxMentionSpan],
) -> impl IntoElement {
    if text.is_empty() {
        return div().into_any_element();
    }
    if spans.is_empty() {
        return div()
            .text_sm()
            .text_color(theme.tokens.text_theme_message)
            .overflow_hidden()
            .child(text.clone())
            .into_any_element();
    }
    let text_str = text.to_string();
    let mut children: Vec<gpui::AnyElement> = Vec::new();
    let mut cursor = 0usize;
    let mut ordered = spans.to_vec();
    ordered.sort_by_key(|span| span.start);
    for span in ordered {
        let Some((start, end)) = mention_byte_range(&text_str, span.start, span.end) else {
            continue;
        };
        if start > cursor {
            children.push(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_message)
                    .child(SharedString::from(text_str[cursor..start].to_string()))
                    .into_any_element(),
            );
        }
        if end > start {
            let mention_text = SharedString::from(text_str[start..end].to_string());
            if span.is_role {
                children.push(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.tokens.color_mention_evryone)
                        .bg(theme.tokens.bg_mention_evryone)
                        .rounded(px(2.))
                        .px(px(1.6))
                        .child(mention_text)
                        .into_any_element(),
                );
            } else {
                children.push(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.tokens.mention_color)
                        .bg(theme.tokens.mention_primary)
                        .rounded(px(2.))
                        .px(px(1.6))
                        .child(mention_text)
                        .into_any_element(),
                );
            }
            cursor = end;
        }
    }
    if cursor < text_str.len() {
        children.push(
            div()
                .text_sm()
                .text_color(theme.tokens.text_theme_message)
                .child(SharedString::from(text_str[cursor..].to_string()))
                .into_any_element(),
        );
    }
    h_flex()
        .flex_wrap()
        .overflow_hidden()
        .children(children)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_byte_range_handles_vietnamese_utf16_offsets() {
        let text = "Thứ tư, @Cù Mạnh Tuấn Tài";
        let (start, end) = mention_byte_range(text, 8, 26).expect("valid range");
        assert_eq!(&text[start..end], "@Cù Mạnh Tuấn Tài");
    }

    #[test]
    fn mention_byte_range_handles_emoji_utf16_offsets() {
        let text = "hello 📢 world";
        let (start, end) = mention_byte_range(text, 6, 8).expect("valid range");
        assert_eq!(&text[start..end], "📢");
    }
}

fn render_attachment_preview(
    theme: &Theme,
    locale: &SharedString,
    link: &str,
    filetype: &str,
    has_more: bool,
    image_cache: Entity<LruImageCache>,
) -> impl IntoElement {
    let more_files = mezon_i18n::t(locale, "channelTopbar.moreFiles");
    v_flex()
        .gap_1()
        .child(if attachment_link_is_image(link, filetype) {
            div()
                .max_w(px(150.))
                .max_h(px(150.))
                .overflow_hidden()
                .rounded(px(8.))
                .child(
                    img(link)
                        .image_cache(&image_cache)
                        .w_full()
                        .h_full()
                        .object_fit(gpui::ObjectFit::Cover),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(150.))
                .h(px(80.))
                .rounded(px(8.))
                .bg(theme.bg_tertiary)
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .text_color(theme.text_muted)
                .child(SharedString::from(
                    filetype
                        .split('/')
                        .next_back()
                        .unwrap_or("file")
                        .to_string(),
                ))
                .into_any_element()
        })
        .when(has_more, |col| {
            col.child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(more_files),
            )
        })
}

fn render_message_head(
    theme: &Theme,
    sender_name: &SharedString,
    time_label: &SharedString,
) -> impl IntoElement {
    h_flex()
        .items_baseline()
        .gap_1()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(DISPLAY_NAME_COLOR))
                .child(sender_name.clone()),
        )
        .when(!time_label.is_empty(), |row| {
            row.child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.tokens.text_secondary)
                    .child(time_label.clone()),
            )
        })
}

pub fn render_notification_body(
    theme: &Theme,
    locale: &SharedString,
    notification: InboxNotification,
    view: &NotificationRowView,
    avatar_cache: Entity<LruImageCache>,
    image_cache: Entity<LruImageCache>,
    _cx: &App,
) -> impl IntoElement {
    let is_for_you = notification.category == InboxCategory::ForYou;
    let is_mentions = notification.category == InboxCategory::Mentions;
    let is_messages = notification.category == InboxCategory::Messages;
    let attachment_label = mezon_i18n::t(locale, "message.clickToSeeAttachment");
    let direct_message_label = mezon_i18n::t(locale, "channelTopbar.directMessage");

    h_flex()
        .gap_4()
        .items_start()
        .p_1()
        .child(render_avatar(
            &view.sender_name,
            view.avatar_url.as_ref(),
            Size::Small,
            avatar_cache.clone(),
        ))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .when_some(view.mention_breadcrumb.as_ref(), |col, breadcrumb| {
                    col.child(render_mention_breadcrumb(theme, breadcrumb))
                })
                .when(view.show_direct_message, |col| {
                    col.child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(direct_message_label),
                    )
                })
                .when_some(view.messages_clan_name.clone(), |col, clan| {
                    col.child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(clan),
                    )
                })
                .when(is_for_you, |col| {
                    if let Some(for_you) = &view.for_you_line {
                        col.child(
                            div().text_sm().text_color(theme.text_primary).child(
                                h_flex()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .child(for_you.display_name.clone()),
                                    )
                                    .child(for_you.subject_suffix.clone()),
                            ),
                        )
                        .when(!view.time_label.is_empty(), |c| {
                            c.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(view.time_label.clone()),
                            )
                        })
                    } else {
                        col
                    }
                })
                .when(is_mentions || is_messages, |col| {
                    col.child(render_message_head(
                        theme,
                        &view.sender_name,
                        &view.time_label,
                    ))
                    .when(view.body_is_attachment, |c| {
                        c.child(
                            div()
                                .text_sm()
                                .text_color(theme.text_muted)
                                .child(attachment_label),
                        )
                    })
                    .when(
                        !view.body_is_attachment && !view.body_text.is_empty(),
                        |c| {
                            c.child(render_message_content(
                                theme,
                                &view.body_text,
                                &view.mention_spans,
                            ))
                        },
                    )
                    .when(!view.attachment_link.is_empty(), |c| {
                        c.child(render_attachment_preview(
                            theme,
                            locale,
                            &view.attachment_link,
                            &view.attachment_type,
                            view.has_more_attachment,
                            image_cache.clone(),
                        ))
                    })
                }),
        )
}

pub fn render_topic_body(
    theme: &Theme,
    locale: &SharedString,
    _topic: &TopicDiscussion,
    view: &TopicRowView,
    avatar_cache: Entity<LruImageCache>,
    _cx: &App,
) -> impl IntoElement {
    let topic_title = mezon_i18n::t(locale, "notification.topicAndYou");
    let replied_label = mezon_i18n::t(locale, "notification.repliedTo");
    let attachment_label: SharedString =
        mezon_i18n::t(locale, "message.clickToSeeAttachment").into();
    let reply_text = if view.reply_is_attachment {
        attachment_label
    } else if view.reply_preview.is_empty() {
        SharedString::from("—")
    } else {
        view.reply_preview.clone()
    };

    h_flex()
        .gap_4()
        .items_start()
        .p_1()
        .pt_1()
        .child(render_avatar(
            &view.avatar_name,
            view.avatar_url.as_ref(),
            Size::Medium,
            avatar_cache,
        ))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child(topic_title),
                )
                .child(
                    div().text_xs().text_color(theme.text_muted).child(
                        h_flex()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(replied_label))
                            .child(": ")
                            .child(reply_text),
                    ),
                ),
        )
}

pub fn notification_copy_text(notification: &InboxNotification) -> Option<String> {
    notification.message.as_ref().and_then(|m| {
        if !m.raw_content.is_empty() {
            Some(m.raw_content.clone())
        } else if !m.content.is_empty() {
            Some(m.content.clone())
        } else {
            None
        }
    })
}
