use crate::app::shell::Shell;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyView, App, Context, Entity, ExternalPaths, FontWeight, ObjectFit, SharedString,
    StyleRefinement, Subscription, Task, Window, div, img, prelude::*, px, rgb, rgba,
};
use mezon_store::{
    BannedUsersStore, ChannelId, ChannelList, ChannelPermissionsStore, ClanId, ClanList,
    DirectEvent, DirectKind, DirectMessageStore, FriendEvent, FriendStore, InVoiceInfo, MessageId,
    MessagesEvent, MessagesStore, OnboardingStore, PERMISSION_SEND_MESSAGE, PinnedMessagesStore,
    ProfileContext, Settings, TopicBadgeStore, TopicDiscussion, TopicsEvent, TopicsStore, UserId,
    resolve_user_profile,
};
use ui::PopoverMenuHandle;

use crate::chat::CanvasPopoverPanel;
use crate::chat::ReplyTarget;
use crate::chat::channel_app_bar::{ChannelAppBarTarget, render_channel_app_bar};
use crate::chat::channel_header::ChatHeader;
use crate::chat::channel_typing::ChannelTyping;
use crate::chat::file_type_icon::file_type_icon_for;
use crate::chat::inbox::InboxPopoverPanel;
use crate::chat::input_bar::{InputBar, ReplyClearSource};
use crate::chat::media_channel::MediaChannelPanel;
use crate::chat::member_list::{MemberListPanel, MemberSource};
use crate::chat::mention_input::{MentionInput, MentionInputEvent};
use crate::chat::message::{ChannelMessages, ChannelMessagesEvent};
use crate::chat::message_search::{MESSAGE_SEARCH_PANEL_WIDTH, MessageSearchPanel};
use crate::chat::pinned_popover::PinnedPopoverPanel;
use crate::chat::user_profile_popover::UserProfilePopover;
use crate::components::compositions::channel_row::ChannelIcon;
use crate::components::primitives::{Avatar, Icon, IconName, InputState};
use crate::image_cache::LruImageCache;
use crate::router::{Route, Router, navigate};
use crate::theme::ActiveTheme;

pub struct ChatArea {
    pub(crate) timeline: Entity<ChannelMessages>,
    pub(crate) mention_input: Option<Entity<MentionInput>>,
    input_bar: Option<Entity<InputBar>>,
    member_panel: Option<Entity<MemberListPanel>>,
    dm_profile_panel: Option<(ChannelId, Entity<UserProfilePopover>)>,
    member_source: Option<MemberSource>,
    member_avatar_cache: Entity<LruImageCache>,
    settings: Entity<Settings>,
    header: Entity<ChatHeader>,
    typing: Entity<ChannelTyping>,
    activity_strip: Entity<LatestActivityStripView>,
    media_channel_panel: Option<Entity<MediaChannelPanel>>,
    media_channel_context: Option<(ClanId, ChannelId)>,
    replying_to: Option<ReplyTarget>,
    send_permission_key: Option<(ClanId, ChannelId)>,
    send_permission_live: Option<bool>,
    can_send_message: Option<bool>,
    dm_blocked: bool,
    no_permission_label: Option<(SharedString, bool, SharedString)>,
    _submit_sub: Option<Subscription>,
    _reply_sub: Option<Subscription>,
    _edit_closed_sub: Option<Subscription>,
    _send_permission_sub: Subscription,
    _send_permission_channel_sub: Subscription,
    _send_permission_direct_sub: Subscription,
    _send_permission_friend_sub: Subscription,
    _send_permission_debounce: Option<Task<()>>,
    drop_title_cache: Option<(SharedString, SharedString, SharedString)>,
    drop_body_cache: Option<(SharedString, SharedString)>,
}

const SEND_PERMISSION_DEBOUNCE: Duration = Duration::from_millis(500);

/// Replaces the composer while the signed-in user is banned from the channel. `remaining` is
/// `None` when the server gave no expiry, i.e. the ban does not lift on its own.
fn banned_notice(locale: &str, remaining: Option<i64>, cx: &App) -> gpui::AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .flex_shrink_0()
        .h(px(48.))
        .ml(px(16.))
        .mr(px(14.))
        .mb(px(16.))
        .px(px(12.))
        .rounded(px(4.))
        .opacity(0.8)
        .bg(theme.tokens.bg_tertiary)
        .text_color(theme.tokens.text_theme_primary)
        .overflow_hidden()
        .child(
            Icon::new(IconName::TriangleAlert)
                .size(px(24.))
                .flex_none()
                .text_color(theme.danger_text),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_secondary)
                        .child(mezon_i18n::t(locale, "common.timeout")),
                )
                .child(
                    div()
                        .text_xs()
                        .truncate()
                        .child(mezon_i18n::t(locale, "common.timeoutDesc")),
                ),
        )
        .children(remaining.map(|left| {
            div()
                .flex_none()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tokens.text_secondary)
                .child(crate::util::time_ago::remaining(locale, left))
        }))
        .into_any_element()
}

/// The strip React parks directly on top of the composer while a member still has onboarding
/// missions left: the next mission, and clicking it does the mission.
fn onboarding_mission_banner(locale: &str, cx: &mut App) -> Option<gpui::AnyElement> {
    let clan_id = ClanList::global(cx).read(cx).active_clan_id?;
    let store = OnboardingStore::global(cx);
    let store = store.read(cx);
    // This runs on every chat render, so the clan with no missions — every clan, for most
    // people — bails on one hash lookup, before the scan `ClanList::clan` costs.
    if store.mission_total(clan_id) == 0 {
        return None;
    }
    let clan_enabled = ClanList::global(cx)
        .read(cx)
        .clan(clan_id)
        .is_some_and(|clan| clan.is_onboarding);
    if !store.show_progress(clan_id, clan_enabled) {
        return None;
    }
    let index = store.mission_progress(clan_id);
    let mission = store.current_mission(clan_id)?;
    let title: SharedString = mission.title.clone().into();
    let task_type = mission.task_type;
    let channel_id = ChannelId(mission.channel_id);
    let channel_name = ChannelList::global(cx)
        .read(cx)
        .channel(clan_id, channel_id)
        .map(|channel| SharedString::from(format!("#{}", channel.name)));
    let theme = cx.theme();
    Some(
        div()
            .flex_none()
            .w_full()
            .px_3()
            .child(
                div()
                    .id("onboarding-mission-banner")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .h(px(56.))
                    .px_4()
                    .rounded_t(px(6.))
                    .cursor_pointer()
                    .bg(theme.tokens.bg_tertiary)
                    .hover(|style| style.bg(theme.tokens.bg_item_hover))
                    .child(
                        Icon::new(IconName::Hashtag)
                            .size(px(20.))
                            .text_color(theme.tokens.text_theme_primary),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.tokens.text_secondary)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_1()
                                    .text_size(px(10.))
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(crate::chat::clan_guide_page::mission_summary(
                                        locale, task_type,
                                    ))
                                    .children(channel_name.map(|name| {
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.tokens.text_secondary)
                                            .child(name)
                                    })),
                            ),
                    )
                    .on_click(move |_, _, cx| {
                        crate::chat::clan_guide_page::start_mission(
                            clan_id, channel_id, task_type, index, cx,
                        );
                    }),
            )
            .into_any_element(),
    )
}

/// Compact activity rail owned by the message column (never the member/topic sidebars).
fn latest_activity_strip(locale: &str, clan_id: &str, cx: &mut App) -> gpui::AnyElement {
    let topics_store = TopicsStore::global(cx);
    let messages_store = MessagesStore::global(cx);
    let topics = topics_store.read(cx);
    let messages = messages_store.read(cx);
    let topic_syncing = topics.is_loading();
    let active_channel_id = messages.active_channel_id();
    let active_channel_key = active_channel_id.map(|channel_id| channel_id.to_string());

    // The list endpoint is the richest source. While it is still loading (or when an older topic
    // only exists in the current channel buffer), synthesize the same row from the origin message
    // and its realtime topic metadata so an existing topic never renders as "empty".
    let latest_topic = (!topic_syncing)
        .then(|| {
            active_channel_key
                .as_deref()
                .and_then(|channel_id| topics.latest_topic_for_channel(clan_id, channel_id))
                .cloned()
                .or_else(|| {
                    let clan_id = messages.active_clan_id()?;
                    let channel_id = messages.active_channel_id()?;
                    messages
                        .messages()
                        .iter()
                        .filter_map(|origin| {
                            let topic_id = origin.topic_id?;
                            let meta = topics.topic_meta_for_topic(topic_id)?;
                            Some((meta.lsnt, origin, topic_id))
                        })
                        .max_by_key(|(timestamp, _, _)| *timestamp)
                        .map(|(timestamp, origin, topic_id)| {
                            let last_visible_message =
                                messages.messages_in_channel(topic_id).iter().rev().find(
                                    |message| {
                                        !message.content.trim().is_empty()
                                            || !message.attachments.is_empty()
                                    },
                                );
                            TopicDiscussion {
                                id: topic_id.to_string(),
                                message_id: origin.id.to_string(),
                                clan_id: clan_id.to_string(),
                                channel_id: channel_id.to_string(),
                                creator_id: origin.sender_id.clone(),
                                last_sender_id: origin.sender_id.clone(),
                                content: origin.content.clone(),
                                last_message_content: last_visible_message
                                    .map(|message| message.content.clone())
                                    .unwrap_or_default(),
                                last_message_attachments: last_visible_message
                                    .map(|message| {
                                        message
                                            .attachments
                                            .iter()
                                            .map(|attachment| {
                                                mezon_client::transport::ApiAttachment {
                                                    url: attachment.url.clone(),
                                                    filename: attachment.filename.clone(),
                                                    filetype: attachment.filetype.clone(),
                                                    width: attachment.width as i32,
                                                    height: attachment.height as i32,
                                                    thumbnail: attachment.thumbnail.clone(),
                                                    duration: attachment.duration,
                                                    size: attachment.size.min(i32::MAX as u64)
                                                        as i32,
                                                }
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                last_message_timestamp: timestamp.clamp(0, i64::from(u32::MAX))
                                    as u32,
                            }
                        })
                })
        })
        .flatten();
    let plain_topic_content = |content: &str| match mezon_client::topic_reply_preview(content) {
        mezon_client::TopicReplyPreview::Text(text) => Some(text),
        _ => None,
    };
    let topic_content_kind = |content: &str| match mezon_client::topic_reply_preview(content) {
        mezon_client::TopicReplyPreview::Contact => {
            format!("[{}]", mezon_i18n::t(locale, "message.attachments.contact"))
        }
        mezon_client::TopicReplyPreview::Interactive => format!(
            "[{}]",
            mezon_i18n::t(locale, "notification.interactiveMessage")
        ),
        mezon_client::TopicReplyPreview::Attachment => format!(
            "[{}]",
            mezon_i18n::t(locale, "message.attachments.attachment")
        ),
        mezon_client::TopicReplyPreview::Text(_) => {
            mezon_i18n::t(locale, "channelTopbar.topic").to_string()
        }
    };
    let topic_title = latest_topic
        .as_ref()
        .and_then(|topic| {
            let message_id = topic.message_id.parse::<MessageId>().ok()?;
            messages
                .messages()
                .iter()
                .find(|message| message.id == message_id)
                .and_then(|message| {
                    plain_topic_content(&message.content).or_else(|| {
                        message.attachments.first().map(|attachment| {
                            if attachment.is_image() {
                                format!("[{}]", mezon_i18n::t(locale, "message.attachments.image"))
                            } else if attachment.is_video() {
                                format!("[{}]", mezon_i18n::t(locale, "message.attachments.video"))
                            } else if !attachment.filename.is_empty() {
                                format!(
                                    "[{}] {}",
                                    mezon_i18n::t(locale, "message.attachments.file"),
                                    attachment.filename
                                )
                            } else {
                                format!("[{}]", mezon_i18n::t(locale, "message.attachments.file"))
                            }
                        })
                    })
                })
        })
        .or_else(|| {
            latest_topic
                .as_ref()
                .and_then(|topic| plain_topic_content(&topic.content))
        });
    let topic_live_last_message = latest_topic.as_ref().and_then(|topic| {
        let topic_id = topic.id.parse::<ChannelId>().ok()?;
        messages
            .messages_in_channel(topic_id)
            .iter()
            .rev()
            .find(|message| !message.content.trim().is_empty() || !message.attachments.is_empty())
    });
    let topic_preview = topic_live_last_message
        .and_then(|message| plain_topic_content(&message.content))
        .or_else(|| {
            latest_topic
                .as_ref()
                .map(|topic| topic.reply_preview_text())
        })
        .filter(|text| !text.trim().is_empty());
    let topic_live_attachment = latest_topic.as_ref().and_then(|topic| {
        let topic_id = topic.id.parse::<ChannelId>().ok()?;
        let latest = topic_live_last_message?;
        latest.attachments.first().cloned().or_else(|| {
            messages
                .messages_in_channel(topic_id)
                .iter()
                .rev()
                .find(|message| {
                    message.create_time == latest.create_time && !message.attachments.is_empty()
                })
                .and_then(|message| message.attachments.first())
                .cloned()
        })
    });
    let topic_attachment = latest_topic.as_ref().and_then(|topic| {
        topic_live_attachment.or_else(|| {
            topic
                .last_message_attachments
                .first()
                .cloned()
                .map(|attachment| {
                    mezon_store::MessageAttachment::from_api(
                        attachment,
                        mezon_store::AppConfig::try_global(cx),
                    )
                })
        })
    });
    let topic_sender_name = latest_topic.as_ref().and_then(|topic| {
        let clan_id = topic.clan_id.parse::<ClanId>().ok()?;
        let sender_id = topic.last_sender_id.parse::<UserId>().ok()?;
        resolve_user_profile(sender_id, ProfileContext::Clan(clan_id), cx)
            .map(|profile| profile.display_name)
            .filter(|name| !name.trim().is_empty())
    });
    let pinned_store = PinnedMessagesStore::global(cx);
    let pinned = pinned_store.read(cx);
    let pin_syncing = pinned.is_loading();
    let active_clan_id = clan_id.parse::<ClanId>().ok();
    let latest_pin = (pinned.clan_id() == active_clan_id
        && pinned.channel_id() == active_channel_id)
        .then(|| {
            pinned
                .pinned()
                .iter()
                .max_by_key(|pin| pin.create_time)
                .cloned()
        })
        .flatten();

    let theme = cx.theme();
    let hover = theme.bg_hover;

    let topic_title: SharedString = if topic_syncing {
        "".into()
    } else {
        topic_title
            .map(|text| SharedString::from(text.split_whitespace().collect::<Vec<_>>().join(" ")))
            .or_else(|| {
                latest_topic
                    .as_ref()
                    .map(|topic| SharedString::from(topic_content_kind(&topic.content)))
            })
            .unwrap_or_else(|| mezon_i18n::t(locale, "notifications.empty.topics.title").into())
    };
    let topic_has_attachment = topic_attachment.is_some()
        || latest_topic
            .as_ref()
            .is_some_and(TopicDiscussion::reply_is_attachment);
    let topic_preview: SharedString = if topic_syncing {
        "".into()
    } else {
        topic_preview
            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .map(|text| {
                if let Some(sender_name) = topic_sender_name.as_deref() {
                    mezon_i18n::t(locale, "chat.activityStrip.messageFrom")
                        .replace("{{name}}", sender_name)
                        .replace("{{message}}", &text)
                } else {
                    text
                }
            })
            .map(SharedString::from)
            .unwrap_or_else(|| {
                if topic_has_attachment {
                    topic_sender_name
                        .as_deref()
                        .map(|sender_name| {
                            mezon_i18n::t(locale, "chat.activityStrip.messageFrom")
                                .replace("{{name}}", sender_name)
                                .replace("{{message}}", "")
                        })
                        .unwrap_or_default()
                        .into()
                } else {
                    mezon_i18n::t(locale, "notifications.empty.topics.description").into()
                }
            })
    };
    let topic_media = topic_attachment.as_ref().map(|attachment| {
        if attachment.is_image() {
            if let Some(path) = attachment.local_source.clone() {
                return div()
                    .flex_none()
                    .size(px(42.))
                    .rounded(px(7.))
                    .overflow_hidden()
                    .child(img(path).size_full().object_fit(ObjectFit::Cover))
                    .into_any_element();
            }
            let source = if !attachment.thumbnail_proxied.is_empty() {
                attachment.thumbnail_proxied.clone()
            } else if !attachment.proxied_src.is_empty() {
                attachment.proxied_src.clone()
            } else if !attachment.thumbnail.is_empty() {
                SharedString::from(attachment.thumbnail.clone())
            } else {
                SharedString::from(attachment.url.clone())
            };
            if !source.is_empty() {
                return div()
                    .flex_none()
                    .size(px(42.))
                    .rounded(px(7.))
                    .overflow_hidden()
                    .child(img(source).size_full().object_fit(ObjectFit::Cover))
                    .into_any_element();
            }
        }
        let filename: SharedString = if attachment.filename.trim().is_empty() {
            mezon_i18n::t(locale, "message.attachments.attachment").into()
        } else {
            attachment.filename.clone().into()
        };
        let size_label: SharedString = mezon_store::format_file_size(attachment.size).into();
        let file_icon = file_type_icon_for(&attachment.filetype, &attachment.filename);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .w(px(145.))
            .h(px(46.))
            .px_2()
            .rounded(px(7.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_hover)
            .child(img(file_icon.path()).w(px(26.)).h(px(34.)).flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .text_color(theme.interactive_active)
                            .child(filename),
                    )
                    .child(
                        div()
                            .truncate()
                            .whitespace_nowrap()
                            .text_size(px(9.))
                            .text_color(theme.text_muted)
                            .child(size_label),
                    ),
            )
            .into_any_element()
    });
    let topic_cell = div()
        .id("latest-topic-activity")
        .flex()
        .items_center()
        .gap_3()
        .flex_1()
        .min_w_0()
        .h_full()
        .px_3()
        .rounded(px(9.))
        .overflow_hidden()
        .when(latest_topic.is_some() && !topic_syncing, |cell| {
            cell.cursor_pointer().hover(move |style| style.bg(hover))
        })
        // A stable semantic icon avoids remounting an image while clan/topic data resolves.
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(36.))
                .child(
                    Icon::new(IconName::TopicIcon)
                        .size(px(30.))
                        .text_color(theme.interactive_active),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .h(px(18.))
                        .truncate()
                        .whitespace_nowrap()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(topic_title),
                )
                .child(
                    div()
                        .flex_none()
                        .h(px(16.))
                        .truncate()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(topic_preview),
                ),
        )
        .children(topic_media)
        .when_some(
            (!topic_syncing).then_some(latest_topic).flatten(),
            |cell, topic| cell.on_click(move |_, _, cx| open_latest_topic(topic.clone(), cx)),
        );

    let pin_attachment = latest_pin
        .as_ref()
        .and_then(|pin| pin.attachments.first())
        .cloned();
    let pin_profile = latest_pin.as_ref().and_then(|pin| {
        let clan_id = active_clan_id?;
        let sender_id = pin.sender_id.parse::<UserId>().ok()?;
        resolve_user_profile(sender_id, ProfileContext::Clan(clan_id), cx)
    });
    let pin_title: SharedString = if pin_syncing && latest_pin.is_none() {
        "".into()
    } else {
        latest_pin
            .as_ref()
            .and_then(|pin| {
                pin_profile
                    .as_ref()
                    .map(|profile| profile.display_name.trim())
                    .filter(|name| !name.is_empty())
                    .or_else(|| {
                        (!pin.sender_name.trim().is_empty()).then(|| pin.sender_name.trim())
                    })
            })
            .map(|name| SharedString::from(name.to_string()))
            .unwrap_or_else(|| {
                mezon_i18n::t(locale, "channelTopbar.pinnedMessages.emptyTitle").into()
            })
    };
    let pin_time: SharedString = latest_pin
        .as_ref()
        .and_then(|pin| chrono::DateTime::from_timestamp(pin.create_time, 0))
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%d/%m/%Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
        .into();
    let pin_preview: SharedString = if pin_syncing && latest_pin.is_none() {
        "".into()
    } else {
        latest_pin
            .as_ref()
            .and_then(|pin| {
                pin.poll
                    .as_ref()
                    .map(|poll| poll.question.trim())
                    .filter(|question| !question.is_empty())
                    .map(|question| {
                        format!(
                            "{}: {question}",
                            mezon_i18n::t(locale, "message.poll.pollLabel")
                        )
                    })
                    .or_else(|| Some(pin.content.trim().to_string()))
            })
            .filter(|text| !text.trim().is_empty())
            // Pinned code blocks and long text commonly contain newlines. A compact rail must
            // remain one line; otherwise wrapping pushes the label/name/time out of the card and
            // appears to change position when a sidebar opens.
            .map(|text| SharedString::from(text.split_whitespace().collect::<Vec<_>>().join(" ")))
            .unwrap_or_else(|| {
                if latest_pin.is_some() {
                    "".into()
                } else {
                    mezon_i18n::t(locale, "channelTopbar.pinnedMessages.emptyDescription").into()
                }
            })
    };
    let pin_message_id = latest_pin
        .as_ref()
        .and_then(|pin| pin.message_id.parse::<MessageId>().ok());
    let pin_media = pin_attachment.as_ref().map(|attachment| {
        if attachment.is_image() {
            let source = if !attachment.thumbnail_proxied.is_empty() {
                attachment.thumbnail_proxied.clone()
            } else if !attachment.proxied_src.is_empty() {
                attachment.proxied_src.clone()
            } else {
                SharedString::from(attachment.url.clone())
            };
            if !source.is_empty() {
                return div()
                    .flex_none()
                    .size(px(42.))
                    .rounded(px(7.))
                    .overflow_hidden()
                    .child(img(source).size_full().object_fit(ObjectFit::Cover))
                    .into_any_element();
            }
        }
        let filename: SharedString = if attachment.filename.trim().is_empty() {
            mezon_i18n::t(locale, "message.attachments.attachment").into()
        } else {
            attachment.filename.clone().into()
        };
        let size = if attachment.size_label.is_empty() {
            mezon_store::format_file_size(attachment.size)
        } else {
            attachment.size_label.to_string()
        };
        let size_label: SharedString = size.into();
        let file_icon = file_type_icon_for(&attachment.filetype, &attachment.filename);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .w(px(145.))
            .h(px(46.))
            .px_2()
            .rounded(px(7.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_hover)
            .child(img(file_icon.path()).w(px(26.)).h(px(34.)).flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .text_color(theme.interactive_active)
                            .child(filename),
                    )
                    .child(
                        div()
                            .truncate()
                            .whitespace_nowrap()
                            .text_size(px(9.))
                            .text_color(theme.text_muted)
                            .child(size_label),
                    ),
            )
            .into_any_element()
    });
    let pin_avatar = latest_pin.as_ref().map(|pin| {
        let display_name = pin_profile
            .as_ref()
            .map(|profile| profile.display_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| pin.sender_name.clone());
        let mut avatar = Avatar::new().name(display_name).size_px(px(32.));
        let raw_source = pin_profile
            .as_ref()
            .map(|profile| profile.avatar_url.as_str())
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .or_else(|| (!pin.avatar_url.is_empty()).then(|| pin.avatar_url.clone()));
        if let Some(raw_source) = raw_source {
            avatar = avatar
                .src(crate::util::imgproxy::avatar_url(cx, &raw_source))
                .fallback_src(raw_source);
        } else if !pin.avatar_proxied.is_empty() {
            avatar = avatar.src(pin.avatar_proxied.clone());
        }
        avatar.into_any_element()
    });
    let has_pin_time = !pin_time.is_empty();
    let pin_cell = div()
        .id("latest-pinned-message")
        .flex()
        .items_center()
        .gap_3()
        .flex_1()
        .min_w_0()
        .h_full()
        .px_3()
        .rounded(px(9.))
        .overflow_hidden()
        .when(pin_message_id.is_some(), |cell| {
            cell.cursor_pointer().hover(move |style| style.bg(hover))
        })
        .child(pin_avatar.unwrap_or_else(|| {
            Icon::new(IconName::PinRight)
                .size(px(20.))
                .text_color(theme.text_muted)
                .into_any_element()
        }))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .h(px(18.))
                        .min_w_0()
                        .overflow_hidden()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .whitespace_nowrap()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(pin_title)
                                .when(pin_message_id.is_some(), |title| {
                                    title.flex_none().max_w(px(180.))
                                })
                                .when(pin_message_id.is_none(), |title| title.flex_1()),
                        )
                        .when(has_pin_time, |header| {
                            header.child(
                                div()
                                    .flex_none()
                                    .text_sm()
                                    .text_color(theme.text_muted)
                                    .child(pin_time),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .h(px(16.))
                        .truncate()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(pin_preview),
                ),
        )
        .children(pin_media)
        .when_some(pin_message_id, |cell, message_id| {
            cell.on_click(move |_, _, cx| {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.jump_to_message(message_id, None, cx);
                });
            })
        });

    div()
        .id("channel-latest-activity-strip")
        .flex()
        .flex_row()
        .flex_none()
        .w_full()
        .h(px(70.))
        .p_1()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.bg_primary)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .size_full()
                .rounded(px(12.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg_secondary)
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .size_full()
                        .child(topic_cell)
                        .child(
                            div()
                                .flex_none()
                                .w(px(2.))
                                .h(px(42.))
                                .rounded_full()
                                .bg(theme.text_muted),
                        )
                        .child(pin_cell),
                ),
        )
        .into_any_element()
}

struct LatestActivityStripView {
    settings: Entity<Settings>,
    _topics_sub: Subscription,
    _pinned_sub: Subscription,
    _messages_sub: Subscription,
    _settings_sub: Subscription,
}

impl LatestActivityStripView {
    fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let topics_sub = cx.subscribe(&TopicsStore::global(cx), |_, _, event, cx| {
            if matches!(event, TopicsEvent::Updated) {
                cx.notify();
            }
        });
        let pinned_sub = cx.subscribe(&PinnedMessagesStore::global(cx), |_, _, event, cx| {
            if matches!(event, mezon_store::PinnedEvent::Updated) {
                cx.notify();
            }
        });
        let messages_sub = cx.subscribe(&MessagesStore::global(cx), |_, _, event, cx| {
            if matches!(
                event,
                MessagesEvent::Reset { .. } | MessagesEvent::TopicUpdated { .. }
            ) {
                cx.notify();
            }
        });
        let settings_sub = cx.observe(&settings, |_, _, cx| cx.notify());
        Self {
            settings,
            _topics_sub: topics_sub,
            _pinned_sub: pinned_sub,
            _messages_sub: messages_sub,
            _settings_sub: settings_sub,
        }
    }
}

impl Render for LatestActivityStripView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let clan_id = MessagesStore::global(cx)
            .read(cx)
            .active_clan_id()
            .filter(|clan_id| !clan_id.is_zero())
            .map(|clan_id| clan_id.to_string());
        clan_id
            .map(|clan_id| latest_activity_strip(&locale, &clan_id, cx))
            .unwrap_or_else(|| div().hidden().into_any_element())
    }
}

fn open_latest_topic(topic: TopicDiscussion, cx: &mut App) {
    let (Ok(clan_id), Ok(channel_id), Ok(topic_id), Ok(message_id)) = (
        topic.clan_id.parse::<ClanId>(),
        topic.channel_id.parse::<ChannelId>(),
        topic.id.parse::<i64>(),
        topic.message_id.parse::<MessageId>(),
    ) else {
        return;
    };
    navigate(
        cx,
        Route::Channel {
            clan_id,
            channel_id,
        },
    );
    TopicBadgeStore::global(cx).update(cx, |store, cx| {
        store.clear_topic(&topic.id, cx);
    });
    ChannelList::global(cx).update(cx, |store, cx| {
        store.apply_topic_read(ChannelId(topic_id), cx);
    });
    TopicsStore::global(cx).update(cx, |store, cx| {
        store.begin_inbox_topic_jump_to_origin(channel_id, topic_id, message_id, message_id, cx);
    });
}

fn leave_removed_conversation(channel_id: ChannelId, cx: &mut App) {
    let viewing = route_targets_conversation(&Router::global(cx).read(cx).route(), channel_id);
    if viewing {
        navigate(cx, Route::Friends);
    }
    // After the navigate, so the entry `navigate` just pushed goes too: a conversation the
    // store dropped must not be reachable through Back or Forward either.
    Router::global(cx).update(cx, |router, _| router.forget_conversation(channel_id));
}

fn route_targets_conversation(route: &Route, channel_id: ChannelId) -> bool {
    matches!(route, Route::DirectMessage { direct_id, .. } if *direct_id == channel_id)
}

impl ChatArea {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<crate::ChatLayout>) -> Self {
        let timeline = cx.new({
            let settings = settings.clone();
            move |cx| ChannelMessages::new(settings, cx)
        });
        ChannelMessages::register_as_active_timeline(&timeline, cx);
        let layout = cx.weak_entity();
        let header = cx.new(|cx| ChatHeader::new(layout, &settings, cx));
        let typing = cx.new(|cx| ChannelTyping::new(&settings, cx));
        let activity_strip = cx.new({
            let settings = settings.clone();
            move |cx| LatestActivityStripView::new(settings, cx)
        });
        let member_avatar_cache = crate::image_cache::shared_avatar_cache(cx);
        let send_permission_sub = cx.subscribe(
            &ChannelPermissionsStore::global(cx),
            |this: &mut crate::ChatLayout, _, _event: &mezon_store::ChannelPermissionsEvent, cx| {
                this.chat_area.sync_send_permission(cx);
            },
        );
        let send_permission_channel_sub = cx.subscribe(
            &MessagesStore::global(cx),
            |this: &mut crate::ChatLayout, _, event: &mezon_store::MessagesEvent, cx| {
                if matches!(event, mezon_store::MessagesEvent::Reset { .. }) {
                    this.chat_area.sync_send_permission(cx);
                }
            },
        );
        let send_permission_direct_sub = cx.subscribe(
            &DirectMessageStore::global(cx),
            |this: &mut crate::ChatLayout, _, event: &DirectEvent, cx| {
                let channel_id = match event {
                    DirectEvent::Changed { channel_id } => *channel_id,
                    DirectEvent::Removed { channel_id } => {
                        leave_removed_conversation(*channel_id, cx);
                        Some(*channel_id)
                    }
                };
                let active_channel = MessagesStore::global(cx).read(cx).active_channel_id();
                if channel_id.is_none() || channel_id == active_channel {
                    this.chat_area.sync_send_permission(cx);
                }
            },
        );
        let send_permission_friend_sub = cx.subscribe(
            &FriendStore::global(cx),
            |this: &mut crate::ChatLayout, _, event: &FriendEvent, cx| {
                if matches!(event, FriendEvent::Changed) {
                    this.chat_area.sync_send_permission(cx);
                }
            },
        );
        Self {
            timeline,
            mention_input: None,
            input_bar: None,
            member_panel: None,
            dm_profile_panel: None,
            member_source: None,
            member_avatar_cache,
            settings,
            header,
            typing,
            activity_strip,
            media_channel_panel: None,
            media_channel_context: None,
            replying_to: None,
            send_permission_key: None,
            send_permission_live: None,
            can_send_message: None,
            dm_blocked: false,
            no_permission_label: None,
            _submit_sub: None,
            _reply_sub: None,
            _edit_closed_sub: None,
            _send_permission_sub: send_permission_sub,
            _send_permission_channel_sub: send_permission_channel_sub,
            _send_permission_direct_sub: send_permission_direct_sub,
            _send_permission_friend_sub: send_permission_friend_sub,
            _send_permission_debounce: None,
            drop_title_cache: None,
            drop_body_cache: None,
        }
    }

    pub fn sync_send_permission(&mut self, cx: &mut Context<crate::ChatLayout>) {
        let (key, dm_peer) = {
            let messages = MessagesStore::global(cx).read(cx);
            if messages.is_dm() {
                let peer = messages.active_channel_id().and_then(|channel_id| {
                    let direct_store = DirectMessageStore::try_global(cx)?;
                    let direct_store = direct_store.read(cx);
                    let direct = direct_store.find(channel_id)?;
                    (direct.kind == DirectKind::Dm).then_some(direct.peer_user_id)?
                });
                (None, peer)
            } else {
                (
                    messages
                        .active_clan_id()
                        .filter(|clan_id| !clan_id.is_zero())
                        .zip(messages.active_channel_id()),
                    None,
                )
            }
        };
        let dm_blocked = dm_peer.is_some_and(|peer| {
            FriendStore::try_global(cx)
                .is_some_and(|store| store.read(cx).is_user_blocked_by_me(peer, cx))
        });
        let mut notify = self.dm_blocked != dm_blocked;
        self.dm_blocked = dm_blocked;
        let live = key.and_then(|(clan_id, channel_id)| {
            ChannelPermissionsStore::try_global(cx).and_then(|store| {
                store
                    .read(cx)
                    .permission_value(PERMISSION_SEND_MESSAGE, clan_id, channel_id)
            })
        });
        if key == self.send_permission_key && live == self.send_permission_live {
            if notify {
                cx.notify();
            }
            return;
        }
        let switched = key != self.send_permission_key;
        self.send_permission_key = key;
        self.send_permission_live = live;
        if switched {
            self._send_permission_debounce = None;
            if self.can_send_message != live {
                self.can_send_message = live;
                notify = true;
            }
            if notify {
                cx.notify();
            }
            return;
        }
        if notify {
            cx.notify();
        }
        self._send_permission_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(SEND_PERMISSION_DEBOUNCE)
                .await;
            let _ = this.update(cx, |this, cx| {
                let live = this.chat_area.send_permission_live;
                if this.chat_area.can_send_message != live {
                    this.chat_area.can_send_message = live;
                    cx.notify();
                }
            });
        }));
    }

    pub(crate) fn send_denied(&self) -> bool {
        self.dm_blocked || self.can_send_message == Some(false)
    }

    pub fn bind_channel_members(&mut self, cx: &mut Context<crate::ChatLayout>) {
        self.set_member_source(Some(MemberSource::Channel), cx);
    }

    pub fn bind_group_members(&mut self, cx: &mut Context<crate::ChatLayout>) {
        self.set_member_source(Some(MemberSource::Group), cx);
    }

    pub fn clear_member_panel(&mut self) {
        self.member_source = None;
        self.member_panel = None;
        self.dm_profile_panel = None;
    }

    pub fn ensure_dm_profile_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<crate::ChatLayout>,
    ) {
        if !matches!(
            Router::global(cx).read(cx).route(),
            Route::DirectMessage { .. }
        ) {
            self.dm_profile_panel = None;
            return;
        }
        let Some((channel_id, user_id)) = DirectMessageStore::try_global(cx).and_then(|store| {
            let store = store.read(cx);
            let channel_id = store.current()?.0;
            let dm = store.find(channel_id)?;
            (dm.kind == DirectKind::Dm).then_some((channel_id, dm.peer_user_id?))
        }) else {
            self.dm_profile_panel = None;
            return;
        };
        if self
            .dm_profile_panel
            .as_ref()
            .is_some_and(|(id, _)| *id == channel_id)
        {
            return;
        }
        let settings = self.settings.clone();
        let avatar_cache = self.member_avatar_cache.clone();
        let panel = cx.new(|cx| {
            UserProfilePopover::new_embedded(
                user_id,
                mezon_store::ProfileContext::Direct(channel_id),
                settings,
                avatar_cache,
                window,
                cx,
            )
        });
        self.dm_profile_panel = Some((channel_id, panel));
    }

    fn set_member_source(
        &mut self,
        source: Option<MemberSource>,
        cx: &mut Context<crate::ChatLayout>,
    ) {
        self.dm_profile_panel = None;
        if self.member_source == source {
            return;
        }
        self.member_source = source;
        self.member_panel = source.map(|source| {
            let settings = self.settings.clone();
            let avatar_cache = self.member_avatar_cache.clone();
            cx.new(move |cx| MemberListPanel::new(source, settings, avatar_cache, cx))
        });
    }

    pub fn bind_window(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        self.timeline
            .update(cx, |timeline, cx| timeline.bind_window(window, cx));
        if let Some(panel) = self.media_channel_panel.clone() {
            panel.update(cx, |panel, cx| panel.bind_window(window, cx));
        }
    }

    pub fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        if self.mention_input.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let placeholder = mezon_i18n::t(&locale, "messageBox.placeholder");
            let settings = self.settings.clone();
            let mention_input = cx.new(|cx| MentionInput::new(placeholder, settings, window, cx));
            MentionInput::register_as_active_composer(&mention_input, cx);
            let submit_sub = cx.subscribe_in(
                &mention_input,
                window,
                |this: &mut crate::ChatLayout, _, event: &MentionInputEvent, window, cx| match event
                {
                    MentionInputEvent::Submit => this.send_current_message(window, cx),
                    MentionInputEvent::Cancel => {
                        MessagesStore::global(cx).update(cx, |store, cx| store.clear_reply(cx));
                    }
                    MentionInputEvent::SendSticker { url, filename } => {
                        this.send_sticker(url.clone(), filename.clone(), cx)
                    }
                    MentionInputEvent::SendGif { url, width, height } => {
                        this.send_gif(url.clone(), *width, *height, cx)
                    }
                    MentionInputEvent::SendSound { url, filename } => {
                        this.send_sound(url.clone(), filename.clone(), cx)
                    }
                    MentionInputEvent::EditLastMessage => {
                        let timeline = this.chat_area.timeline.clone();
                        timeline.update(cx, |timeline, cx| {
                            timeline.edit_last_own_message(window, cx)
                        });
                    }
                },
            );
            self._submit_sub = Some(submit_sub);
            self.input_bar = Some(cx.new(|cx| {
                InputBar::new(
                    mention_input.clone(),
                    SharedString::from(locale.clone()),
                    &self.settings,
                    false,
                    cx,
                )
            }));
            self.mention_input = Some(mention_input);
            self.replying_to = MessagesStore::global(cx)
                .read(cx)
                .reply_target()
                .map(|draft| ReplyTarget {
                    sender_name: SharedString::from(draft.sender_name.clone()),
                });

            let reply_sub = cx.subscribe_in(
                &MessagesStore::global(cx),
                window,
                |this: &mut crate::ChatLayout, store, event: &MessagesEvent, window, cx| {
                    if matches!(event, MessagesEvent::SendFailedWithoutRow) {
                        let locale = this.chat_area.settings.read(cx).language.clone();
                        let message =
                            SharedString::from(mezon_i18n::t(&locale, "message.toast.sendFailed"));
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                        return;
                    }
                    if matches!(event, MessagesEvent::ReplyTargetChanged) {
                        let replying_to = store.read(cx).reply_target().map(|draft| ReplyTarget {
                            sender_name: SharedString::from(draft.sender_name.clone()),
                        });
                        if replying_to.is_some()
                            && let Some(input) = this.chat_area.mention_input.clone()
                        {
                            window.defer(cx, move |window, cx| {
                                input.update(cx, |input, cx| input.focus_input(window, cx));
                            });
                        }
                        this.chat_area.replying_to = replying_to;
                        cx.notify();
                    }
                },
            );
            self._reply_sub = Some(reply_sub);

            let edit_closed_sub = cx.subscribe_in(
                &self.timeline,
                window,
                |this: &mut crate::ChatLayout, _, event: &ChannelMessagesEvent, window, cx| {
                    let ChannelMessagesEvent::EditClosed = event;
                    if this.chat_area.send_denied() {
                        return;
                    }
                    if let Some(input) = this.chat_area.mention_input.clone() {
                        window.defer(cx, move |window, cx| {
                            input.update(cx, |input, cx| input.focus_input(window, cx));
                        });
                    }
                },
            );
            self._edit_closed_sub = Some(edit_closed_sub);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_canvas(
        &mut self,
        locale: &str,
        channel_name: Option<&str>,
        channel_icon: Option<ChannelIcon>,
        channel_id: Option<ChannelId>,
        show_members_button: bool,
        show_member_panel: bool,
        show_inbox: bool,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
        show_search_bar: bool,
        search_expanded: bool,
        show_search_options: bool,
        search_input: Option<Entity<InputState>>,
        canvas_body: gpui::AnyElement,
        cx: &mut Context<crate::ChatLayout>,
    ) -> gpui::AnyElement {
        self.render_panel_body(
            locale,
            channel_name,
            channel_icon,
            channel_id,
            show_members_button,
            show_member_panel,
            show_inbox,
            inbox_handle,
            clan_id,
            pin_handle,
            canvas_handle,
            show_search_bar,
            search_expanded,
            show_search_options,
            search_input,
            canvas_body,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_panel_body(
        &mut self,
        locale: &str,
        channel_name: Option<&str>,
        channel_icon: Option<ChannelIcon>,
        channel_id: Option<ChannelId>,
        show_members_button: bool,
        show_member_panel: bool,
        show_inbox: bool,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
        show_search_bar: bool,
        search_expanded: bool,
        show_search_options: bool,
        search_input: Option<Entity<InputState>>,
        panel_body: gpui::AnyElement,
        cx: &mut Context<crate::ChatLayout>,
    ) -> gpui::AnyElement {
        self.header.update(cx, |header, cx| {
            header.sync(
                channel_name,
                channel_icon,
                false,
                None,
                show_members_button,
                show_member_panel,
                show_inbox,
                inbox_handle,
                clan_id,
                pin_handle,
                canvas_handle,
                false,
                false,
                show_search_bar,
                search_expanded,
                show_search_options,
                search_input,
                false,
                Some(locale),
                cx,
            );
        });

        self.typing
            .update(cx, |typing, cx| typing.sync(channel_id, cx));

        let header = AnyView::from(self.header.clone()).cached(
            StyleRefinement::default()
                .w_full()
                .h(px(crate::app::window_controls::APP_HEADER_HEIGHT))
                .flex_shrink_0(),
        );

        let panel_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(panel_body);

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .child(panel_column)
            .when(show_member_panel, |row| match &self.member_panel {
                Some(panel) => row.child(
                    AnyView::from(panel.clone()).cached(
                        StyleRefinement::default()
                            .w(px(245.))
                            .h_full()
                            .flex_shrink_0(),
                    ),
                ),
                None => row.child(div().w(px(245.)).h_full().flex_shrink_0()),
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .child(header)
            .child(body)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        locale: &str,
        channel_name: Option<&str>,
        channel_icon: Option<ChannelIcon>,
        is_dm: bool,
        in_voice: Option<InVoiceInfo>,
        channel_id: Option<ChannelId>,
        show_members_button: bool,
        show_member_panel: bool,
        show_inbox: bool,
        timeline_action: bool,
        timeline_active: bool,
        media_channel_view: bool,
        media_clan_id: Option<ClanId>,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        canvas_handle: Option<PopoverMenuHandle<CanvasPopoverPanel>>,
        show_search_bar: bool,
        search_expanded: bool,
        show_search_options: bool,
        search_input: Option<Entity<InputState>>,
        show_results_panel: bool,
        message_search_panel: Option<Entity<MessageSearchPanel>>,
        app_channel_bar: Option<ChannelAppBarTarget>,
        stream_sidebar: bool,
        cx: &mut Context<crate::ChatLayout>,
    ) -> gpui::AnyElement {
        let (input_bar, mention_input) = if media_channel_view {
            (None, None)
        } else {
            match (self.input_bar.clone(), self.mention_input.clone()) {
                (Some(input_bar), Some(mention_input)) => (Some(input_bar), Some(mention_input)),
                _ => {
                    return div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .into_any_element();
                }
            }
        };

        let activity_strip = (!is_dm && !stream_sidebar && !media_channel_view)
            .then(|| AnyView::from(self.activity_strip.clone()));

        self.header.update(cx, |header, cx| {
            header.sync(
                channel_name,
                channel_icon,
                is_dm,
                in_voice,
                show_members_button,
                show_member_panel,
                show_inbox,
                inbox_handle,
                clan_id,
                pin_handle,
                canvas_handle,
                timeline_action,
                timeline_active,
                show_search_bar,
                search_expanded,
                show_search_options,
                search_input,
                stream_sidebar,
                Some(locale),
                cx,
            );
        });

        self.typing
            .update(cx, |typing, cx| typing.sync(channel_id, cx));

        if media_channel_view && let (Some(clan_id), Some(channel_id)) = (media_clan_id, channel_id)
        {
            let needs_new = self.media_channel_context != Some((clan_id, channel_id));
            if needs_new || self.media_channel_panel.is_none() {
                self.media_channel_context = Some((clan_id, channel_id));
                self.media_channel_panel = Some(cx.new(|cx| {
                    MediaChannelPanel::new(clan_id, channel_id, self.settings.clone(), cx)
                }));
                if let Some(panel) = self.media_channel_panel.clone() {
                    panel.update(cx, |panel, cx| panel.on_enter(cx));
                }
            } else if let Some(panel) = self.media_channel_panel.clone() {
                panel.update(cx, |panel, cx| {
                    panel.sync_channel(clan_id, channel_id, cx);
                });
            }
        } else {
            self.media_channel_panel = None;
            self.media_channel_context = None;
        }

        if let Some(input_bar) = input_bar.clone() {
            input_bar.update(cx, |input_bar, cx| {
                input_bar.sync(
                    locale,
                    self.replying_to.clone(),
                    ReplyClearSource::Messages,
                    cx,
                )
            });
        }

        let header = AnyView::from(self.header.clone()).cached(
            StyleRefinement::default()
                .w_full()
                .h(px(crate::app::window_controls::APP_HEADER_HEIGHT))
                .flex_shrink_0(),
        );

        let channel_label = channel_name.unwrap_or_default();
        let drop_title = match &self.drop_title_cache {
            Some((cached_locale, cached_channel, title))
                if cached_locale.as_ref() == locale && cached_channel.as_ref() == channel_label =>
            {
                title.clone()
            }
            _ => {
                let title: SharedString = mezon_i18n::t(locale, "common.uploadToChannel")
                    .replace("{{channelName}}", channel_label)
                    .into();
                self.drop_title_cache = Some((
                    SharedString::from(locale.to_string()),
                    SharedString::from(channel_label.to_string()),
                    title.clone(),
                ));
                title
            }
        };
        let drop_body = match &self.drop_body_cache {
            Some((cached_locale, body)) if cached_locale.as_ref() == locale => body.clone(),
            _ => {
                let body: SharedString = mezon_i18n::t(locale, "common.uploadInstructions").into();
                self.drop_body_cache = Some((SharedString::from(locale.to_string()), body.clone()));
                body
            }
        };
        let drop_overlay = if media_channel_view {
            None
        } else {
            Some(
                div()
                    .absolute()
                    .inset_0()
                    .invisible()
                    .group_drag_over::<ExternalPaths>("chat-drop-zone", |style| style.visible())
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x000000e6))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(16.))
                            .w(px(400.))
                            .h(px(240.))
                            .rounded(px(8.))
                            .border_2()
                            .border_dashed()
                            .border_color(rgb(0xffffff))
                            .bg(rgb(0x5865f2))
                            .child(
                                Icon::new(IconName::FileAndFolder)
                                    .size(px(48.))
                                    .text_color(rgb(0xffffff)),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(18.))
                                    .text_color(rgb(0xffffff))
                                    .child(drop_title),
                            )
                            .child(
                                div()
                                    .px(px(24.))
                                    .text_center()
                                    .text_size(px(14.))
                                    .text_color(rgb(0xffffff))
                                    .child(drop_body),
                            ),
                    ),
            )
        };

        let send_denied = !media_channel_view && self.send_denied();
        let no_permission_notice = if send_denied {
            let label = match &self.no_permission_label {
                Some((cached_locale, cached_dm_blocked, label))
                    if cached_locale.as_ref() == locale
                        && *cached_dm_blocked == self.dm_blocked =>
                {
                    label.clone()
                }
                _ => {
                    let key = if self.dm_blocked {
                        "message.blockedUserMessage"
                    } else {
                        "common.noPermissionToSendMessage"
                    };
                    let label: SharedString = mezon_i18n::t(locale, key).into();
                    self.no_permission_label = Some((
                        SharedString::from(locale.to_string()),
                        self.dm_blocked,
                        label.clone(),
                    ));
                    label
                }
            };
            let theme = cx.theme();
            div()
                .flex_shrink_0()
                .h(px(44.))
                .ml(px(16.))
                .mr(px(14.))
                .mb(px(16.))
                .py(px(8.))
                .pl(px(8.))
                .rounded(px(4.))
                .opacity(0.8)
                .bg(theme.tokens.bg_tertiary)
                .text_color(theme.tokens.text_theme_primary)
                .overflow_hidden()
                .child(label)
                .into_any_element()
        } else {
            div().into_any_element()
        };

        // A ban replaces the composer outright, the way the web client does — the moderator list
        // only says *who* is banned, so the countdown has to come from `IsBanned`.
        //
        // A denied send wins over it: the server answers `IsBanned` with `EXISTS ucns:<user>:<channel>`,
        // and that key is also what revoking send-message on a channel writes — without a TTL — so
        // every member of a bot-only channel reads back as banned (KOMU #checkin: is_banned=1,
        // ttl=-1, send-message active=0). Trust the permission over the ban when they disagree.
        let ban_notice = if !is_dm
            && !media_channel_view
            && !send_denied
            && let Some(channel_id) = channel_id
        {
            let store = BannedUsersStore::global(cx);
            store.update(cx, |store, cx| store.ensure_self_ban(channel_id, cx));
            let remaining = store.read(cx).self_ban_remaining(channel_id);
            remaining.map(|left| banned_notice(locale, left, cx))
        } else {
            None
        };
        let banned = ban_notice.is_some();

        let onboarding_mission = if !is_dm && !media_channel_view && !send_denied && !banned {
            onboarding_mission_banner(locale, cx)
        } else {
            None
        };

        let timeline_popover_open = self.timeline.read(cx).profile_popover_open();
        let member_popover_open = self
            .member_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).profile_popover_open());

        let message_column = div()
            .relative()
            .group("chat-drop-zone")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .children(activity_strip)
            .when(!media_channel_view, |col| {
                let drop_input = mention_input;
                let input_visible = !send_denied && !banned;
                col.on_drop(
                    move |paths: &ExternalPaths, window: &mut Window, cx: &mut App| {
                        if let Some(drop_input) = drop_input.clone()
                            && input_visible
                        {
                            let dropped: Vec<PathBuf> = paths.paths().to_vec();
                            drop_input.update(cx, |input, cx| {
                                input.focus_input(window, cx);
                                input.add_dropped_paths(dropped, window, cx)
                            });
                        }
                    },
                )
            })
            .when(media_channel_view, |col| {
                if let Some(panel) = self.media_channel_panel.clone() {
                    col.child(div().size_full().child(AnyView::from(panel)))
                } else {
                    col
                }
            })
            .when(!media_channel_view, |col| {
                col.child(div().flex_1().min_h_0().overflow_hidden().child(
                    if timeline_popover_open {
                        div()
                            .size_full()
                            .child(AnyView::from(self.timeline.clone()))
                            .into_any_element()
                    } else {
                        AnyView::from(self.timeline.clone())
                            .cached(StyleRefinement::default().size_full())
                            .into_any_element()
                    },
                ))
                .when_some(ban_notice, |col, notice| col.child(notice))
                .when(send_denied, |col| col.child(no_permission_notice))
                .when(!banned && !send_denied, |col| {
                    col.children(onboarding_mission)
                        .when_some(input_bar.clone(), |col, input_bar| col.child(input_bar))
                        .when_some(app_channel_bar.as_ref(), |col, target| {
                            col.child(render_channel_app_bar(locale, target.clone(), cx.theme()))
                        })
                        .child(
                            AnyView::from(self.typing.clone()).cached(
                                StyleRefinement::default()
                                    .w_full()
                                    .h(px(16.))
                                    .flex_shrink_0(),
                            ),
                        )
                })
                .when_some(drop_overlay, |col, overlay| col.child(overlay))
            });

        let has_search_panel = show_results_panel && message_search_panel.is_some();
        let member_visible = show_member_panel && !has_search_panel && !media_channel_view;
        let dm_profile_panel = self.dm_profile_panel.as_ref().and_then(|(id, panel)| {
            matches!(
                Router::global(cx).read(cx).route_ref(),
                Route::DirectMessage { direct_id, .. } if *direct_id == *id
            )
            .then(|| panel.clone())
        });
        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .child(message_column)
            .when_some(message_search_panel, |row, panel| {
                row.child(
                    AnyView::from(panel).cached(
                        StyleRefinement::default()
                            .w(px(MESSAGE_SEARCH_PANEL_WIDTH))
                            .h_full()
                            .flex_shrink_0(),
                    ),
                )
            })
            .when_some(self.member_panel.clone(), |row, panel| {
                row.child(
                    div()
                        .h_full()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .when(member_visible, |slot| slot.w(px(245.)))
                        .when(!member_visible, |slot| slot.w(px(0.)).invisible())
                        .child(if member_popover_open {
                            div()
                                .w(px(245.))
                                .h_full()
                                .child(AnyView::from(panel))
                                .into_any_element()
                        } else {
                            AnyView::from(panel)
                                .cached(StyleRefinement::default().w(px(245.)).h_full())
                                .into_any_element()
                        }),
                )
            })
            .when_some(dm_profile_panel, |row, panel| {
                row.child(
                    div()
                        .h_full()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .border_l_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().surfaces.direct_message.ramp())
                        .when(member_visible, |slot| slot.w(px(320.)))
                        .when(!member_visible, |slot| slot.w(px(0.)).invisible())
                        .child(AnyView::from(panel)),
                )
            });

        let hide_header = is_dm
            && channel_id.is_some_and(|cid| {
                crate::chat::call_window::call_panel_active_for_dm(cid.get(), cx)
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .h_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .when(!hide_header, |this| this.child(header))
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod removed_conversation_tests {
    use super::{ChannelId, Route, route_targets_conversation};

    fn direct(id: i64) -> Route {
        Route::DirectMessage {
            direct_id: ChannelId(id),
            message_type: "2".to_string(),
        }
    }

    #[test]
    fn the_open_conversation_has_to_be_left() {
        assert!(route_targets_conversation(&direct(7), ChannelId(7)));
    }

    #[test]
    fn another_conversation_stays_put() {
        assert!(!route_targets_conversation(&direct(7), ChannelId(8)));
    }

    #[test]
    fn a_route_outside_direct_messages_stays_put() {
        assert!(!route_targets_conversation(&Route::Friends, ChannelId(7)));
    }
}
