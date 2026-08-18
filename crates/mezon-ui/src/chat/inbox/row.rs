use std::ops::Range;

use chrono::{Local, TimeZone};
use gpui::{
    App, ClickEvent, Entity, FontWeight, HighlightStyle, Hsla, ObjectFit, SharedString, StyledText,
    UnderlineStyle, Window, div, img, prelude::*, px, rgb,
};
use mezon_store::{
    AppConfig, AttachmentSeedInput, Channel, ChannelAttachment, ChannelId, ChannelList,
    ChannelType, ClanId, ClanList, ClanMembersStore, DirectMessageStore, InboxCategory,
    InboxMentionSpan, InboxNotification, MessageId, MessageSpan, ProfileContext, RolesStore,
    Settings, TopicDiscussion, TopicReplyPreview, UserId, UsersByUserStore,
    attachment_link_is_image, attachment_link_is_video, format_file_size, inbox_spans_from_raw,
    message_content_is_attachment, resolve_user_profile,
};

use crate::chat::file_type_icon::file_type_icon_for;
use crate::chat::message::parts::FILE_NAME_COLOR;
use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size, h_flex, v_flex};
use crate::image_cache::LruImageCache;
use crate::image_viewer::{OpenViewerRequest, open_image_viewer, resolve_channel_label};
use crate::router::Route;
use crate::theme::Theme;
use crate::util::download::save_with_progress_toast;

const DEFAULT_ROLE_COLOR: u32 = 0x99_aab5;
pub const FOR_YOU_ROW_HEIGHT: f32 = 76.;
pub const MENTION_ROW_HEIGHT: f32 = 120.;
pub const MESSAGE_ROW_HEIGHT: f32 = 128.;
pub const TOPIC_ROW_HEIGHT: f32 = 150.;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InboxInlineHighlight {
    Mention { is_role: bool },
    Link,
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
    body_spans: Vec<MessageSpan>,
    body_link_ranges: Vec<Range<usize>>,
    body_inline_span_ranges: Vec<(Range<usize>, InboxInlineHighlight)>,
    mention_spans: Vec<InboxMentionSpan>,
    sender_name_color: Hsla,
    attachment_link: String,
    attachment_type: String,
    attachment_filename: String,
    attachment_size: u64,
    attachment_thumbnail: String,
    has_more_attachment: bool,
    media: Option<InboxMediaOpen>,
    pub(crate) can_jump: bool,
}

#[derive(Clone)]
pub(crate) struct TopicRowView {
    avatar_name: SharedString,
    avatar_url: Option<SharedString>,
    reply_preview: TopicReplyPreview,
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

fn parse_hex_role_color(raw: &str) -> Option<gpui::Rgba> {
    mezon_store::parse_role_color(raw)
}

fn resolve_inbox_sender_color(clan_id: ClanId, sender_id: UserId, cx: &App) -> Hsla {
    let role_ids = ClanMembersStore::global(cx)
        .read(cx)
        .member(clan_id, sender_id)
        .map(|member| member.role_ids.clone())
        .unwrap_or_default();
    if role_ids.is_empty() {
        return Hsla::from(rgb(DEFAULT_ROLE_COLOR));
    }
    let Some(roles_store) = RolesStore::try_global(cx) else {
        return Hsla::from(rgb(DEFAULT_ROLE_COLOR));
    };
    let store = roles_store.read(cx);
    let matched_role = store
        .roles_in_clan(clan_id)
        .into_iter()
        .find(|(id, _)| role_ids.contains(id))
        .map(|(_, role)| role);
    if let Some(role) = matched_role
        && let Some(color) = (!role.color.is_empty())
            .then(|| parse_hex_role_color(&role.color))
            .flatten()
    {
        return Hsla::from(color);
    }
    Hsla::from(rgb(DEFAULT_ROLE_COLOR))
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

pub(crate) fn notification_jump_route(notification: &InboxNotification, cx: &App) -> Option<Route> {
    let channel_id_str = notification.effective_channel_id()?;
    if channel_id_str.is_empty() || channel_id_str == "0" {
        return None;
    }
    let channel_id = channel_id_str.parse::<ChannelId>().ok()?;
    let clan_id = notification
        .effective_clan_id()
        .and_then(|id| id.parse::<ClanId>().ok())
        .filter(|id| !id.is_zero())
        .or_else(|| {
            ChannelList::global(cx)
                .read(cx)
                .clan_id_for_channel(channel_id)
                .filter(|id| !id.is_zero())
        });
    match clan_id {
        Some(clan_id) => Some(Route::Channel {
            clan_id,
            channel_id,
        }),
        None => Some(Route::DirectMessage {
            direct_id: channel_id,
            message_type: DirectMessageStore::global(cx)
                .read(cx)
                .find(channel_id)
                .map(|dm| dm.kind.channel_type().to_string())
                .unwrap_or_else(|| "3".into()),
        }),
    }
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

    let is_mentions = notification.category == InboxCategory::Mentions;
    let is_messages = notification.category == InboxCategory::Messages;
    let (resolved_name, mut avatar_url, _) = clan_id
        .map(|clan| resolve_sender(clan, sender_id, fallback_avatar, &fallback_name, cx))
        .unwrap_or((
            fallback_name.clone().into(),
            avatar_from(fallback_avatar),
            false,
        ));
    let mut sender_name = if (is_mentions || is_messages) && !fallback_name.is_empty() {
        fallback_name.into()
    } else if !resolved_name.is_empty() {
        resolved_name
    } else if !fallback_name.is_empty() {
        fallback_name.into()
    } else {
        SharedString::default()
    };

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
    let body_spans = message_preview
        .and_then(|m| inbox_spans_from_raw(&m.raw_content))
        .unwrap_or_default();
    let body_text_str = body_text.to_string();
    let body_raw_content = message_preview
        .map(|m| m.raw_content.as_str())
        .unwrap_or("");
    let body_link_ranges = inbox_link_ranges(&body_text_str, body_raw_content);
    let body_inline_span_ranges = inbox_inline_span_ranges(&body_text_str, &body_spans);
    let attachment_link = message_preview
        .map(|m| m.attachment_link.clone())
        .unwrap_or_default();
    let attachment_type = message_preview
        .map(|m| m.attachment_type.clone())
        .unwrap_or_default();
    let attachment_filename = message_preview
        .map(|m| m.attachment_filename.clone())
        .unwrap_or_default();
    let attachment_size = message_preview
        .map(|m| m.attachment_size)
        .unwrap_or_default();
    let attachment_thumbnail = message_preview
        .map(|m| m.attachment_thumbnail.clone())
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

    let sender_name_color = clan_id
        .zip(sender_id.parse::<UserId>().ok())
        .map(|(clan, user_id)| resolve_inbox_sender_color(clan, user_id, cx))
        .unwrap_or_else(|| Hsla::from(rgb(DEFAULT_ROLE_COLOR)));

    let can_jump = notification.effective_message_id().is_some()
        && notification_jump_route(notification, cx).is_some();
    let media = (!attachment_link.is_empty()).then(|| {
        inbox_media_open(
            notification,
            &attachment_link,
            &attachment_filename,
            &attachment_type,
        )
    });

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
        body_spans,
        body_link_ranges,
        body_inline_span_ranges,
        mention_spans,
        sender_name_color,
        attachment_link,
        attachment_type,
        attachment_filename,
        attachment_size,
        attachment_thumbnail,
        has_more_attachment,
        media,
        can_jump,
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
    TopicRowView {
        avatar_name,
        avatar_url,
        reply_preview: topic.reply_preview(),
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
    let clan_line = if has_category {
        format!("{} > {}", breadcrumb.clan_name, breadcrumb.category_name)
    } else {
        breadcrumb.clan_name.to_string()
    };
    let channel_line = if !has_channel {
        None
    } else if let Some(thread) = breadcrumb.thread_label.as_ref() {
        Some(format!("{} > {}", breadcrumb.channel_label, thread))
    } else {
        Some(breadcrumb.channel_label.to_string())
    };
    v_flex()
        .w_full()
        .min_w_0()
        .gap(px(2.))
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(clan_line),
        )
        .when_some(channel_line, |col, line| {
            col.child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .text_color(theme.text_primary)
                    .child(line),
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

fn mention_highlight_style(theme: &Theme, is_role: bool) -> HighlightStyle {
    let (color, bg) = if is_role {
        (
            theme.tokens.color_mention_evryone.into(),
            theme.tokens.bg_mention_evryone.into(),
        )
    } else {
        (
            theme.tokens.mention_color.into(),
            theme.tokens.mention_primary.into(),
        )
    };
    HighlightStyle {
        color: Some(color),
        background_color: Some(bg),
        font_weight: Some(FontWeight::MEDIUM),
        ..Default::default()
    }
}

fn link_highlight_style(theme: &Theme) -> HighlightStyle {
    let link_color: Hsla = theme.tokens.mention_color.into();
    HighlightStyle {
        color: Some(link_color),
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(link_color),
            wavy: false,
        }),
        ..Default::default()
    }
}

fn is_link_highlight(style: &HighlightStyle) -> bool {
    style.underline.is_some()
}

fn clip_highlights(
    highlights: &[(Range<usize>, HighlightStyle)],
    start: usize,
    end: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    if start >= end {
        return Vec::new();
    }
    highlights
        .iter()
        .filter_map(|(range, style)| {
            let s = range.start.max(start);
            let e = range.end.min(end);
            if e <= s {
                return None;
            }
            Some((s - start..e - start, *style))
        })
        .collect()
}

fn span_ranges_in_text(
    text: &str,
    spans: &[MessageSpan],
) -> Vec<(Range<usize>, InboxInlineHighlight)> {
    let mut cursor = 0usize;
    let mut out = Vec::new();
    for span in spans {
        if matches!(span, MessageSpan::CodeBlock { .. }) {
            continue;
        }
        let Some(piece) = span_inline_text(span) else {
            continue;
        };
        if piece.is_empty() {
            continue;
        }
        if !text[cursor..].starts_with(piece) {
            break;
        }
        let start = cursor;
        let end = start + piece.len();
        match span {
            MessageSpan::Mention { role_id, .. } => {
                let is_role = role_id.as_deref().is_some_and(|id| !id.is_empty());
                out.push((start..end, InboxInlineHighlight::Mention { is_role }));
            }
            MessageSpan::Link { .. } => {
                out.push((start..end, InboxInlineHighlight::Link));
            }
            _ => {}
        }
        cursor = end;
    }
    out
}

fn inbox_inline_span_ranges(
    text: &str,
    body_spans: &[MessageSpan],
) -> Vec<(Range<usize>, InboxInlineHighlight)> {
    span_ranges_in_text(text, body_spans)
}

fn inbox_link_ranges(text: &str, raw_content: &str) -> Vec<Range<usize>> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw_content.trim()) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for key in ["lk", "vk", "lky"] {
        let Some(items) = value.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        for item in items {
            let Some(start) = item.get("s").and_then(|v| v.as_i64()) else {
                continue;
            };
            let Some(end) = item.get("e").and_then(|v| v.as_i64()) else {
                continue;
            };
            let Some((byte_start, byte_end)) = mention_byte_range(text, start as i32, end as i32)
            else {
                continue;
            };
            if byte_end > byte_start {
                ranges.push(byte_start..byte_end);
            }
        }
    }
    ranges
}

fn ranges_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn merge_sorted_highlights(
    text: &str,
    tokens: Vec<(Range<usize>, HighlightStyle)>,
    body_style: HighlightStyle,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let text_len = text.len();
    let mut mentions = Vec::new();
    let mut links = Vec::new();
    for (range, style) in tokens {
        if range.start >= range.end || range.end > text_len {
            continue;
        }
        if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            continue;
        }
        if style.background_color.is_some() {
            mentions.push((range, style));
        } else if is_link_highlight(&style) {
            links.push((range, style));
        }
    }
    mentions.sort_by_key(|(range, _)| range.start);
    links.sort_by_key(|(range, _)| range.start);
    let mut picked = mentions;
    for (link_range, link_style) in links {
        if !picked
            .iter()
            .any(|(range, _)| ranges_overlap(range, &link_range))
        {
            picked.push((link_range, link_style));
        }
    }
    picked.sort_by_key(|(range, _)| range.start);
    let mut highlights = Vec::with_capacity(picked.len() * 2 + 1);
    let mut cursor = 0usize;
    for (range, style) in picked {
        if range.start < cursor {
            continue;
        }
        if cursor < range.start {
            highlights.push((cursor..range.start, body_style));
        }
        highlights.push((range.clone(), style));
        cursor = range.end;
    }
    if cursor < text_len {
        highlights.push((cursor..text_len, body_style));
    }
    if highlights.is_empty() && text_len > 0 {
        highlights.push((0..text_len, body_style));
    }
    highlights
}

fn inbox_content_highlights(
    theme: &Theme,
    text: &str,
    mention_spans: &[InboxMentionSpan],
    body_link_ranges: &[Range<usize>],
    body_inline_span_ranges: &[(Range<usize>, InboxInlineHighlight)],
) -> Vec<(Range<usize>, HighlightStyle)> {
    let body_color: Hsla = theme.tokens.text_theme_message.into();
    let body_style = HighlightStyle {
        color: Some(body_color),
        ..Default::default()
    };
    let mut tokens: Vec<(Range<usize>, HighlightStyle)> = Vec::new();

    for span in mention_spans {
        let Some((start, end)) = mention_byte_range(text, span.start, span.end) else {
            continue;
        };
        if end > start {
            tokens.push((start..end, mention_highlight_style(theme, span.is_role)));
        }
    }

    for (range, kind) in body_inline_span_ranges {
        let style = match kind {
            InboxInlineHighlight::Mention { is_role } => mention_highlight_style(theme, *is_role),
            InboxInlineHighlight::Link => link_highlight_style(theme),
        };
        tokens.push((range.clone(), style));
    }

    for range in body_link_ranges {
        if range.end > range.start {
            tokens.push((range.clone(), link_highlight_style(theme)));
        }
    }
    tokens.extend(inbox_auto_link_highlights(theme, text, &tokens));
    merge_sorted_highlights(text, tokens, body_style)
}

fn next_char_index(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let Some(ch) = text[index..].chars().next() else {
        return text.len();
    };
    index + ch.len_utf8()
}

fn char_boundary_range(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start >= range.end || range.end > text.len() {
        return None;
    }
    if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
        return None;
    }
    Some(range)
}

fn inbox_auto_link_highlights(
    theme: &Theme,
    text: &str,
    occupied: &[(Range<usize>, HighlightStyle)],
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < text.len() {
        if !text.is_char_boundary(index) {
            index = next_char_index(text, index);
            continue;
        }
        let rest = &text[index..];
        let scheme_len = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            index = next_char_index(text, index);
            continue;
        };
        let start = index;
        let mut end = index + scheme_len;
        while end < text.len() {
            if !text.is_char_boundary(end) {
                break;
            }
            let Some(ch) = text[end..].chars().next() else {
                break;
            };
            if ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '(' | '[') {
                break;
            }
            if matches!(ch, ')' | ']') {
                break;
            }
            end += ch.len_utf8();
        }
        while end > start + scheme_len {
            if !text.is_char_boundary(end) {
                break;
            }
            let Some(ch) = text[..end].chars().last() else {
                break;
            };
            if matches!(ch, '.' | ',' | ';' | '!' | '?' | ')' | ']') {
                end -= ch.len_utf8();
            } else {
                break;
            }
        }
        let range = start..end;
        if end > start + scheme_len
            && char_boundary_range(text, range.clone()).is_some()
            && !occupied
                .iter()
                .any(|(occupied_range, _)| ranges_overlap(occupied_range, &range))
            && !out
                .iter()
                .any(|(existing, _)| ranges_overlap(existing, &range))
        {
            out.push((range, link_highlight_style(theme)));
            index = end;
        } else {
            index = next_char_index(text, index);
        }
    }
    out
}

fn render_inbox_styled_body(
    theme: &Theme,
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> gpui::AnyElement {
    if text.is_empty() {
        return div().into_any_element();
    }
    let code_style = inline_code_highlight_style(theme);
    let (text, highlights) = strip_inline_code_markers(text, highlights, code_style);
    let (display_text, display_highlights) = inject_link_word_joiners(&text, highlights);
    div()
        .w_full()
        .min_w_0()
        .text_sm()
        .child(StyledText::new(display_text).with_highlights(display_highlights))
        .into_any_element()
}

fn backtick_run_len(bytes: &[u8], at: usize) -> usize {
    bytes[at..].iter().take_while(|&&b| b == b'`').count()
}

fn find_backtick_code_pairs(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut pairs = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let run = backtick_run_len(bytes, i);
        if run != 1 {
            i += run;
            continue;
        }
        let content_start = i + 1;
        match text[content_start..].find('`') {
            Some(rel) => {
                let close = content_start + rel;
                if backtick_run_len(bytes, close) == 1 {
                    pairs.push((i, close));
                    i = close + 1;
                } else {
                    i = close + backtick_run_len(bytes, close);
                }
            }
            None => i += 1,
        }
    }
    pairs
}

fn strip_inline_code_markers(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    code_style: HighlightStyle,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let pairs = find_backtick_code_pairs(text);
    if pairs.is_empty() {
        return (text.to_string(), highlights);
    }
    let mut boundaries: Vec<usize> = vec![0, text.len()];
    for (open, close) in &pairs {
        boundaries.push(*open);
        boundaries.push(open + 1);
        boundaries.push(*close);
        boundaries.push(close + 1);
    }
    for (range, _) in &highlights {
        boundaries.push(range.start);
        boundaries.push(range.end);
    }
    boundaries.retain(|b| *b <= text.len());
    boundaries.sort_unstable();
    boundaries.dedup();

    let style_at = |pos: usize| -> HighlightStyle {
        highlights
            .iter()
            .find(|(range, _)| range.start <= pos && pos < range.end)
            .map(|(_, style)| *style)
            .unwrap_or_default()
    };
    let is_backtick_byte = |pos: usize| pairs.iter().any(|(o, c)| pos == *o || pos == *c);
    let in_code_inner = |pos: usize| pairs.iter().any(|(o, c)| pos > *o && pos < *c);

    let mut display_text = String::with_capacity(text.len());
    let mut raw_spans: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut cursor = 0usize;
    for window in boundaries.windows(2) {
        let (start, end) = (window[0], window[1]);
        if start >= end {
            continue;
        }
        if end == start + 1 && is_backtick_byte(start) {
            continue;
        }
        display_text.push_str(&text[start..end]);
        let style = if in_code_inner(start) {
            code_style
        } else {
            style_at(start)
        };
        let len = end - start;
        raw_spans.push((cursor..cursor + len, style));
        cursor += len;
    }
    let mut display_highlights: Vec<(Range<usize>, HighlightStyle)> =
        Vec::with_capacity(raw_spans.len());
    for (range, style) in raw_spans {
        if let Some(last) = display_highlights.last_mut()
            && last.1 == style
            && last.0.end == range.start
        {
            last.0.end = range.end;
            continue;
        }
        display_highlights.push((range, style));
    }
    (display_text, display_highlights)
}

const LINK_WORD_JOINER: char = '\u{2060}';

fn inject_link_word_joiners(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let mut break_after = std::collections::BTreeSet::new();
    for (range, style) in &highlights {
        if !is_link_highlight(style) {
            continue;
        }
        let Some(range) = char_boundary_range(text, range.clone()) else {
            continue;
        };
        let mut index = range.start;
        while index < range.end.min(text.len()) {
            let Some(ch) = text[index..].chars().next() else {
                break;
            };
            let next = index + ch.len_utf8();
            if next < range.end {
                break_after.insert(next);
            }
            index = next;
        }
    }
    if break_after.is_empty() {
        return (text.to_string(), highlights);
    }
    let mut display = String::new();
    let mut orig_to_new = vec![0usize; text.len() + 1];
    for (index, ch) in text.char_indices() {
        orig_to_new[index] = display.len();
        display.push(ch);
        let next = index + ch.len_utf8();
        if break_after.contains(&next) {
            display.push(LINK_WORD_JOINER);
        }
    }
    orig_to_new[text.len()] = display.len();
    let remapped = highlights
        .into_iter()
        .filter_map(|(range, style)| {
            let range = char_boundary_range(text, range)?;
            Some((orig_to_new[range.start]..orig_to_new[range.end], style))
        })
        .collect();
    (display, remapped)
}

fn span_inline_text(span: &MessageSpan) -> Option<&str> {
    match span {
        MessageSpan::Text(text)
        | MessageSpan::Bold(text)
        | MessageSpan::Code(text)
        | MessageSpan::Link { text, .. }
        | MessageSpan::Mention { display: text, .. }
        | MessageSpan::Hashtag { display: text, .. }
        | MessageSpan::Heading { text, .. } => Some(text.as_ref()),
        _ => None,
    }
}

fn inline_code_highlight_style(theme: &Theme) -> HighlightStyle {
    HighlightStyle {
        color: Some(theme.tokens.text_secondary.into()),
        background_color: Some(theme.tokens.bg_active_member_channel.into()),
        ..Default::default()
    }
}

fn take_inbox_inline_element(
    theme: &Theme,
    text: &str,
    batch: &mut Vec<MessageSpan>,
    offset: &mut usize,
    full_highlights: &[(Range<usize>, HighlightStyle)],
) -> Option<gpui::AnyElement> {
    if batch.is_empty() {
        return None;
    }
    let batch_text: String = batch.iter().filter_map(span_inline_text).collect();
    if batch_text.is_empty() {
        batch.clear();
        return None;
    }
    let Some(rel) = text[*offset..].find(&batch_text) else {
        batch.clear();
        return None;
    };
    let start = *offset + rel;
    let end = start + batch_text.len();
    let clipped = clip_highlights(full_highlights, start, end);
    *offset = end;
    batch.clear();
    Some(render_inbox_styled_body(theme, &text[start..end], clipped))
}

fn render_inbox_message_spans(
    theme: &Theme,
    text: &str,
    mention_spans: &[InboxMentionSpan],
    body_link_ranges: &[Range<usize>],
    body_inline_span_ranges: &[(Range<usize>, InboxInlineHighlight)],
    body_spans: &[MessageSpan],
) -> gpui::AnyElement {
    if body_spans.is_empty() {
        return div().into_any_element();
    }
    let full_highlights = inbox_content_highlights(
        theme,
        text,
        mention_spans,
        body_link_ranges,
        body_inline_span_ranges,
    );
    let mut children: Vec<gpui::AnyElement> = Vec::new();
    let mut inline_batch: Vec<MessageSpan> = Vec::new();
    let mut offset = 0usize;

    for span in body_spans {
        if matches!(span, MessageSpan::CodeBlock { .. }) {
            if let Some(element) = take_inbox_inline_element(
                theme,
                text,
                &mut inline_batch,
                &mut offset,
                &full_highlights,
            ) {
                children.push(element);
            }
            if let MessageSpan::CodeBlock {
                text: code_text, ..
            } = span
            {
                if let Some(rel) = text[offset..].find(code_text.as_ref()) {
                    offset += rel + code_text.len();
                }
                children.push(render_inbox_code_block(theme, code_text));
            }
            continue;
        }
        inline_batch.push(span.clone());
    }
    if let Some(element) = take_inbox_inline_element(
        theme,
        text,
        &mut inline_batch,
        &mut offset,
        &full_highlights,
    ) {
        children.push(element);
    }

    v_flex()
        .w_full()
        .min_w_0()
        .gap_1()
        .children(children)
        .into_any_element()
}

fn render_message_content(
    theme: &Theme,
    text: &SharedString,
    mention_spans: &[InboxMentionSpan],
    body_link_ranges: &[Range<usize>],
    body_inline_span_ranges: &[(Range<usize>, InboxInlineHighlight)],
    body_spans: &[MessageSpan],
) -> impl IntoElement {
    let text_str = text.to_string();
    if text_str.is_empty() {
        return div().into_any_element();
    }
    let has_code_block = body_spans
        .iter()
        .any(|span| matches!(span, MessageSpan::CodeBlock { .. }));
    if has_code_block {
        return render_inbox_message_spans(
            theme,
            &text_str,
            mention_spans,
            body_link_ranges,
            body_inline_span_ranges,
            body_spans,
        );
    }
    let highlights = inbox_content_highlights(
        theme,
        &text_str,
        mention_spans,
        body_link_ranges,
        body_inline_span_ranges,
    );
    render_inbox_styled_body(theme, &text_str, highlights)
}

fn render_inbox_code_block(theme: &Theme, text: &SharedString) -> gpui::AnyElement {
    div()
        .w_full()
        .min_w_0()
        .my_1()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.tokens.border_primary)
        .bg(theme.tokens.bg_markdown_code)
        .text_size(px(14.))
        .text_color(theme.tokens.text_theme_message)
        .child(text.clone())
        .into_any_element()
}

#[derive(Clone)]
struct InboxMediaOpen {
    clan_id: ClanId,
    channel_id: ChannelId,
    message_id: MessageId,
    uploader_id: UserId,
    create_time_seconds: u32,
    url: String,
    filename: String,
    filetype: String,
}

fn inbox_media_open(
    notification: &InboxNotification,
    url: &str,
    filename: &str,
    filetype: &str,
) -> InboxMediaOpen {
    let clan_id = notification
        .effective_clan_id()
        .and_then(|id| id.parse().ok())
        .filter(|id: &ClanId| !id.is_zero())
        .unwrap_or(ClanId(0));
    let channel_id = notification
        .effective_channel_id()
        .and_then(|id| id.parse().ok())
        .filter(|id: &ChannelId| !id.is_zero())
        .unwrap_or(ChannelId(0));
    let message_id = notification
        .effective_message_id()
        .and_then(|id| id.parse().ok())
        .unwrap_or(MessageId(0));
    let uploader_id = notification
        .message
        .as_ref()
        .map(|m| m.sender_id.as_str())
        .filter(|id| !id.is_empty() && *id != "0")
        .unwrap_or(notification.sender_id.as_str())
        .parse()
        .unwrap_or(UserId(0));
    InboxMediaOpen {
        clan_id,
        channel_id,
        message_id,
        uploader_id,
        create_time_seconds: notification.message_timestamp(),
        url: url.to_string(),
        filename: filename.to_string(),
        filetype: filetype.to_string(),
    }
}

fn open_inbox_media_viewer(media: InboxMediaOpen, window: &Window, cx: &mut App) {
    if media.url.is_empty() {
        return;
    }
    let Some(settings) = Settings::try_global(cx) else {
        return;
    };
    let seed = ChannelAttachment::seed_from_message(
        &AttachmentSeedInput {
            url: media.url.clone(),
            filename: media.filename,
            filetype: media.filetype,
            width: 0,
            height: 0,
            presign_pending: false,
        },
        media.message_id,
        media.uploader_id,
        media.create_time_seconds,
        media.channel_id,
        media.clan_id,
        AppConfig::try_global(cx),
    );
    let url = SharedString::from(seed.url.clone());
    let anchor = media.create_time_seconds.saturating_add(86_400);
    open_image_viewer(
        OpenViewerRequest {
            clan_id: media.clan_id,
            channel_id: media.channel_id,
            channel_label: resolve_channel_label(
                media.clan_id,
                media.channel_id,
                SharedString::default(),
                cx,
            ),
            settings,
            attachments: vec![seed],
            selected_index: 0,
            selected_url: Some(url),
            anchor_before: (media.create_time_seconds > 0).then_some(anchor),
        },
        window,
        cx,
    );
}

fn render_inbox_image(
    link: &str,
    image_cache: Entity<LruImageCache>,
    media: Option<InboxMediaOpen>,
) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!("inbox-image-{link}")))
        .max_w(px(150.))
        .max_h(px(150.))
        .overflow_hidden()
        .rounded(px(8.))
        .cursor_pointer()
        .child(
            img(link)
                .image_cache(&image_cache)
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        )
        .on_click(move |_: &ClickEvent, window, cx| {
            cx.stop_propagation();
            if let Some(media) = media.clone() {
                open_inbox_media_viewer(media, window, cx);
            }
        })
        .into_any_element()
}

fn render_inbox_video(
    theme: &Theme,
    link: &str,
    thumbnail: &str,
    image_cache: Entity<LruImageCache>,
    media: Option<InboxMediaOpen>,
) -> gpui::AnyElement {
    let poster = if thumbnail.is_empty() {
        None
    } else {
        Some(SharedString::from(thumbnail.to_string()))
    };
    div()
        .id(SharedString::from(format!("inbox-video-{link}")))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(150.))
        .h(px(150.))
        .overflow_hidden()
        .rounded(px(8.))
        .bg(theme.bg_tertiary)
        .cursor_pointer()
        .when_some(poster, |el, poster| {
            el.child(
                img(poster)
                    .image_cache(&image_cache)
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Cover),
            )
        })
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::Rgba {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.3,
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(40.))
                        .h(px(40.))
                        .rounded_full()
                        .bg(gpui::Rgba {
                            r: 0.,
                            g: 0.,
                            b: 0.,
                            a: 0.5,
                        })
                        .child(
                            Icon::new(IconName::PlayButton)
                                .size(px(18.))
                                .text_color(gpui::white()),
                        ),
                ),
        )
        .on_click(move |_: &ClickEvent, window, cx| {
            cx.stop_propagation();
            if let Some(media) = media.clone() {
                open_inbox_media_viewer(media, window, cx);
            }
        })
        .into_any_element()
}

fn render_inbox_file_card(
    theme: &Theme,
    link: &str,
    filetype: &str,
    filename: &str,
    size: u64,
) -> gpui::AnyElement {
    let display_name = if filename.is_empty() {
        SharedString::from("Attachment")
    } else {
        SharedString::from(filename.to_string())
    };
    let size_line = SharedString::from(format!("size: {}", format_file_size(size)));
    let icon = file_type_icon_for(filetype, filename);
    let download_url = SharedString::from(link.to_string());
    let download_name = display_name.clone();
    let body_url = download_url.clone();
    let body_name = download_name.clone();
    let group_name = SharedString::from(format!("inbox-file-{link}"));
    let body_id = SharedString::from(format!("inbox-file-body-{link}"));
    div()
        .id(group_name.clone())
        .group(group_name.clone())
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .w_full()
        .max_w_full()
        .min_w_0()
        .mt(px(10.))
        .p_3()
        .rounded_lg()
        .bg(theme.tokens.bg_item_theme_hover)
        .border_1()
        .border_color(theme.tokens.border_primary)
        .overflow_hidden()
        .child(
            div()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .w(px(32.))
                .h(px(40.))
                .child(img(icon.path()).w(px(32.)).h(px(40.)).flex_none()),
        )
        .child(
            div()
                .id(body_id)
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .cursor_pointer()
                .on_click({
                    let body_url = body_url.clone();
                    let body_name = body_name.clone();
                    move |_: &ClickEvent, _, cx| {
                        save_with_progress_toast(body_url.clone(), body_name.clone(), cx);
                    }
                })
                .child(
                    div()
                        .truncate()
                        .text_size(px(16.))
                        .text_color(gpui::rgb(FILE_NAME_COLOR))
                        .hover(|s| s.underline())
                        .child(display_name),
                )
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme.tokens.text_theme_primary)
                        .child(size_line),
                ),
        )
        .child(
            div()
                .absolute()
                .right(px(12.))
                .flex()
                .items_center()
                .opacity(0.)
                .group_hover(group_name, |s| s.opacity(1.))
                .child(
                    div()
                        .id(SharedString::from(format!("inbox-file-dl-{link}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(28.))
                        .rounded_md()
                        .bg(theme.tokens.bg_theme_contexify)
                        .border_1()
                        .border_color(theme.tokens.border_primary)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.8))
                        .on_click(move |_: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            save_with_progress_toast(
                                download_url.clone(),
                                download_name.clone(),
                                cx,
                            );
                        })
                        .child(
                            Icon::new(IconName::Download)
                                .size(px(14.))
                                .text_color(theme.tokens.text_theme_primary),
                        ),
                ),
        )
        .into_any_element()
}

fn render_attachment_preview(
    theme: &Theme,
    locale: &SharedString,
    view: &NotificationRowView,
    image_cache: Entity<LruImageCache>,
) -> impl IntoElement {
    let more_files = mezon_i18n::t(locale, "channelTopbar.moreFiles");
    let link = view.attachment_link.as_str();
    let filetype = view.attachment_type.as_str();
    let media = view.media.clone();
    let preview = if attachment_link_is_image(link, filetype) {
        render_inbox_image(link, image_cache, media)
    } else if attachment_link_is_video(link, filetype) {
        render_inbox_video(theme, link, &view.attachment_thumbnail, image_cache, media)
    } else {
        render_inbox_file_card(
            theme,
            link,
            filetype,
            &view.attachment_filename,
            view.attachment_size,
        )
    };
    v_flex()
        .gap_1()
        .w_full()
        .min_w_0()
        .child(preview)
        .when(view.has_more_attachment, |col| {
            col.child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(more_files),
            )
        })
}

pub(crate) fn render_message_head(
    theme: &Theme,
    sender_name: &SharedString,
    time_label: &SharedString,
    sender_name_color: Hsla,
) -> impl IntoElement {
    if time_label.is_empty() {
        return div()
            .w_full()
            .min_w_0()
            .text_size(px(16.))
            .line_height(px(20.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(sender_name_color)
            .child(sender_name.clone())
            .into_any_element();
    }
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w_0()
        .child(
            div()
                .flex_none()
                .text_size(px(16.))
                .line_height(px(20.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(sender_name_color)
                .child(sender_name.clone()),
        )
        .child(
            div()
                .flex_none()
                .ml_1()
                .pt(px(4.))
                .text_size(px(10.))
                .line_height(px(20.))
                .text_color(theme.tokens.text_secondary)
                .child(time_label.clone()),
        )
        .into_any_element()
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
                                    .w_full()
                                    .min_w_0()
                                    .flex_wrap()
                                    .child(
                                        div()
                                            .max_w_full()
                                            .min_w_0()
                                            .font_weight(FontWeight::BOLD)
                                            .child(for_you.display_name.clone()),
                                    )
                                    .child(
                                        div()
                                            .max_w_full()
                                            .min_w_0()
                                            .child(for_you.subject_suffix.clone()),
                                    ),
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
                    col.child(
                        div().w_full().min_w_0().child(
                            v_flex()
                                .gap(px(2.))
                                .child(render_message_head(
                                    theme,
                                    &view.sender_name,
                                    &view.time_label,
                                    view.sender_name_color,
                                ))
                                .when(
                                    view.body_is_attachment && view.attachment_link.is_empty(),
                                    |c| {
                                        c.child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.text_muted)
                                                .child(attachment_label),
                                        )
                                    },
                                )
                                .when(!view.body_text.is_empty(), |c| {
                                    c.child(render_message_content(
                                        theme,
                                        &view.body_text,
                                        &view.mention_spans,
                                        &view.body_link_ranges,
                                        &view.body_inline_span_ranges,
                                        &view.body_spans,
                                    ))
                                })
                                .when(!view.attachment_link.is_empty(), |c| {
                                    c.child(render_attachment_preview(
                                        theme,
                                        locale,
                                        view,
                                        image_cache.clone(),
                                    ))
                                }),
                        ),
                    )
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
    let reply_text: SharedString = match &view.reply_preview {
        TopicReplyPreview::Text(text) => text.clone().into(),
        TopicReplyPreview::Contact => mezon_i18n::t(locale, "notification.contactMessage").into(),
        TopicReplyPreview::Attachment => {
            mezon_i18n::t(locale, "notification.attachmentMessage").into()
        }
        TopicReplyPreview::Interactive => {
            mezon_i18n::t(locale, "notification.interactiveMessage").into()
        }
    };
    let combined = format!("{replied_label}{reply_text}");
    let label_end = replied_label.len();
    let muted: Hsla = theme.text_muted.into();
    let highlights = vec![
        (
            0..label_end,
            HighlightStyle {
                color: Some(muted),
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            },
        ),
        (
            label_end..combined.len(),
            HighlightStyle {
                color: Some(muted),
                ..Default::default()
            },
        ),
    ];

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
                    div()
                        .w_full()
                        .min_w_0()
                        .text_xs()
                        .line_clamp(5)
                        .text_ellipsis()
                        .child(StyledText::new(combined).with_highlights(highlights)),
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

    #[test]
    fn inbox_mention_highlights_preserve_plain_text_gaps() {
        let theme = Theme::dark();
        let text = "- line @VINH tail @user.name end";
        let spans = vec![
            InboxMentionSpan {
                start: 7,
                end: 12,
                user_id: String::new(),
                role_id: "1".into(),
                is_role: true,
            },
            InboxMentionSpan {
                start: 18,
                end: 28,
                user_id: "2".into(),
                role_id: String::new(),
                is_role: false,
            },
        ];
        let highlights = inbox_content_highlights(&theme, text, &spans, &[], &[]);
        assert!(highlights.iter().any(|(range, _)| range == &(0..7)));
        assert!(highlights.iter().any(|(range, style)| {
            range == &(7..12) && style.font_weight == Some(FontWeight::MEDIUM)
        }));
        assert!(highlights.iter().any(|(range, _)| range == &(12..18)));
        assert!(
            highlights
                .iter()
                .any(|(range, _)| range == &(28..text.len()))
        );
    }

    #[test]
    fn clip_highlights_skips_ranges_before_clip_window() {
        let theme = Theme::dark();
        let style = link_highlight_style(&theme);
        let highlights = vec![(0..5, style)];
        let clipped = clip_highlights(&highlights, 10, 20);
        assert!(clipped.is_empty());
    }

    #[test]
    fn inbox_auto_link_highlights_detect_plain_urls() {
        let theme = Theme::dark();
        let text = "see https://checkin.nccsoft.vn and http://example.com ok";
        let highlights = inbox_content_highlights(&theme, text, &[], &[], &[]);
        assert!(highlights.iter().any(|(range, style)| {
            is_link_highlight(style) && &text[range.clone()] == "https://checkin.nccsoft.vn"
        }));
        assert!(highlights.iter().any(|(range, style)| {
            is_link_highlight(style) && &text[range.clone()] == "http://example.com"
        }));
    }

    #[test]
    fn inbox_content_highlights_handle_vietnamese_plain_text() {
        let theme = Theme::dark();
        let text = "thua nên hơi buồn a, không muốn kéo dài nỗi đau";
        let highlights = inbox_content_highlights(&theme, text, &[], &[], &[]);
        assert!(highlights.iter().all(|(range, _)| {
            text.is_char_boundary(range.start) && text.is_char_boundary(range.end)
        }));
        assert_eq!(
            highlights.last().map(|(range, _)| range.end),
            Some(text.len())
        );
    }

    #[test]
    fn strip_inline_code_markers_removes_backticks_and_styles_inner_text() {
        let theme = Theme::dark();
        let text = "Bạn đã đặt `Matcha latte xoài` !!!";
        let body_style = HighlightStyle {
            color: Some(theme.tokens.text_theme_message.into()),
            ..Default::default()
        };
        let code_style = inline_code_highlight_style(&theme);
        let highlights = vec![(0..text.len(), body_style)];
        let (display_text, display_highlights) =
            strip_inline_code_markers(text, highlights, code_style);
        assert_eq!(display_text, "Bạn đã đặt Matcha latte xoài !!!");
        assert!(!display_text.contains('`'));
        let code_range = display_text.find("Matcha latte xoài").unwrap();
        let code_end = code_range + "Matcha latte xoài".len();
        assert!(display_highlights.iter().any(|(range, style)| {
            range == &(code_range..code_end)
                && style.background_color == code_style.background_color
        }));
    }

    #[test]
    fn strip_inline_code_markers_ignores_triple_backtick_fences() {
        let text = "before ```block``` after";
        let highlights = vec![(0..text.len(), HighlightStyle::default())];
        let (display_text, _) =
            strip_inline_code_markers(text, highlights, HighlightStyle::default());
        assert_eq!(display_text, text);
    }

    #[test]
    fn strip_inline_code_markers_leaves_plain_text_unchanged() {
        let text = "no backticks here";
        let highlights = vec![(0..text.len(), HighlightStyle::default())];
        let (display_text, display_highlights) =
            strip_inline_code_markers(text, highlights.clone(), HighlightStyle::default());
        assert_eq!(display_text, text);
        assert_eq!(display_highlights, highlights);
    }

    #[test]
    fn parse_hex_role_color_accepts_shorthand_and_ignores_alpha() {
        let rgb = parse_hex_role_color("#fff").expect("3-digit hex");
        assert_eq!(rgb.r, 1.);
        assert_eq!(rgb.g, 1.);
        assert_eq!(rgb.b, 1.);
        let rgb = parse_hex_role_color("#ff000080").expect("8-digit hex");
        assert_eq!(rgb.r, 1.);
        assert_eq!(rgb.g, 0.);
        assert_eq!(rgb.b, 0.);
    }

    #[test]
    fn inbox_inline_span_ranges_use_sequential_alignment() {
        let text = "hello @user world";
        let spans = vec![
            MessageSpan::Text("hello ".into()),
            MessageSpan::Mention {
                display: "@user".into(),
                user_id: Some("1".into()),
                role_id: None,
            },
            MessageSpan::Text(" world".into()),
        ];
        let ranges = inbox_inline_span_ranges(text, &spans);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].0, 6..11);
        assert_eq!(
            ranges[0].1,
            InboxInlineHighlight::Mention { is_role: false }
        );
    }
}
