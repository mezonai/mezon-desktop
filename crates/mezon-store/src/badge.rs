use std::collections::{HashSet, VecDeque};

use gpui::{App, AppContext, Context, Entity, Global};
use mezon_client::RealtimeEvent;
use mezon_proto::api::{ChannelMessage, Notification};

use crate::AuthState;
use crate::Settings;
use crate::channel::ChannelList;
use crate::clan::ClanList;
use crate::clan_members::ClanMembersStore;
use crate::direct::DirectMessageStore;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use crate::message::MessageCode;
use crate::messages::MessagesStore;
use crate::platform::{DesktopNotification, PlatformStore};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const STREAM_MODE_GROUP: i32 = 3;
const STREAM_MODE_DM: i32 = 4;
const MAX_BADGE_DEDUP: usize = 500;
const NOTIFICATION_USER_MENTIONED: i32 = -9;
const NOTIFICATION_USER_REPLIED: i32 = -11;
const NOTIFICATION_CHANNEL_TYPE_THREAD: i32 = 7;
const NOTIFICATION_CHANNEL_TYPE_APP: i32 = 8;
const NOTIFICATION_CHANNEL_TYPE_VOICE: i32 = 10;

fn mark_badge_processed(
    seen: &mut HashSet<(ChannelId, MessageId)>,
    order: &mut VecDeque<(ChannelId, MessageId)>,
    key: (ChannelId, MessageId),
) -> bool {
    if key.1.is_zero() {
        return true;
    }
    if !seen.insert(key) {
        return false;
    }
    order.push_back(key);
    if order.len() > MAX_BADGE_DEDUP {
        while order.len() > MAX_BADGE_DEDUP / 2 {
            if let Some(evicted) = order.pop_front() {
                seen.remove(&evicted);
            }
        }
    }
    true
}

fn is_dm_message(m: &ChannelMessage) -> bool {
    m.mode == STREAM_MODE_GROUP || m.mode == STREAM_MODE_DM || m.clan_id == 0
}

fn badge_channel_id(m: &ChannelMessage) -> ChannelId {
    if m.topic_id != 0 {
        ChannelId(m.topic_id)
    } else {
        ChannelId(m.channel_id)
    }
}

fn is_content_mutation(m: &ChannelMessage) -> bool {
    matches!(
        MessageCode::from_raw(m.code),
        MessageCode::ChatUpdate
            | MessageCode::ChatRemove
            | MessageCode::UpdateEphemeralMsg
            | MessageCode::DeleteEphemeralMsg
    )
}

fn is_chat_remove(m: &ChannelMessage) -> bool {
    matches!(MessageCode::from_raw(m.code), MessageCode::ChatRemove)
}

fn deleted_message_timestamp(m: &ChannelMessage) -> i64 {
    if m.update_time_seconds > 0 {
        i64::from(m.update_time_seconds)
    } else {
        i64::from(m.create_time_seconds)
    }
}

/// Matches React `ChatContext` `isNotCurrentDirect` before `badgeService.incrementDm`.
fn should_increment_dm_unread(cx: &App, channel_id: ChannelId, from_me: bool) -> bool {
    if from_me {
        return false;
    }
    let messages = MessagesStore::global(cx).read(cx);
    let app_focused = cx.active_window().is_some();
    let viewing_this_dm =
        messages.is_dm() && messages.active_channel_id() == Some(channel_id) && app_focused;
    !viewing_this_dm
}

fn is_clan_message_seen(cx: &App, m: &ChannelMessage, from_me: bool) -> bool {
    if from_me {
        return true;
    }
    if cx.active_window().is_none() {
        return false;
    }
    let messages = MessagesStore::global(cx).read(cx);
    if messages.is_dm() {
        return false;
    }
    let Some(active) = messages.active_channel_id() else {
        return false;
    };
    let badge_id = badge_channel_id(m);
    let parent_id = ChannelId(m.channel_id);
    active == badge_id || active == parent_id
}

fn is_viewing_channel(cx: &App, channel_id: ChannelId) -> bool {
    if cx.active_window().is_none() {
        return false;
    }
    let messages = MessagesStore::global(cx).read(cx);
    if messages.is_dm() {
        return false;
    }
    messages.active_channel_id() == Some(channel_id)
}

fn maybe_show_desktop_notification(cx: &App, notif: &Notification, channel_id: ChannelId) {
    if cx.active_window().is_some() {
        return;
    }
    let Some(settings) = Settings::try_global(cx) else {
        return;
    };
    let (enabled, hide_content) = {
        let s = settings.read(cx);
        (s.notifications_enabled, s.notifications_hide_content)
    };
    if !enabled {
        return;
    }
    let Some(platform) = PlatformStore::try_global(cx) else {
        return;
    };
    let title = notif
        .channel
        .as_ref()
        .map(|c| c.channel_label.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mezon".to_string());
    let body = if hide_content || notif.subject.is_empty() {
        "New message".to_string()
    } else {
        notif.subject.clone()
    };
    platform.read(cx).show_notification(DesktopNotification {
        title,
        body,
        channel_id: Some(channel_id.to_string()),
    });
}

fn dm_message_preview(m: &ChannelMessage) -> String {
    let text = serde_json::from_str::<mezon_client::transport::ApiMessageContent>(&m.content)
        .map(|c| c.t)
        .unwrap_or_default();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "New message".to_string()
    } else {
        trimmed.chars().take(140).collect()
    }
}

fn is_message_already_seen(
    cx: &App,
    clan_id: ClanId,
    channel_id: ChannelId,
    msg_time: i64,
) -> bool {
    if msg_time <= 0 {
        return false;
    }
    let last_seen = ChannelList::global(cx)
        .read(cx)
        .channel(clan_id, channel_id)
        .map(|channel| channel.last_seen_timestamp)
        .unwrap_or(0);
    last_seen > 0 && msg_time <= last_seen
}

pub struct BadgeService {
    auth_state: Entity<AuthState>,
    processed_badge_messages: HashSet<(ChannelId, MessageId)>,
    processed_badge_order: VecDeque<(ChannelId, MessageId)>,
}

struct GlobalBadgeService(Entity<BadgeService>);
impl Global for GlobalBadgeService {}

impl BadgeService {
    pub fn init(auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(auth_state, cx));
        cx.set_global(GlobalBadgeService(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalBadgeService>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalBadgeService>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.processed_badge_messages.clear();
        self.processed_badge_order.clear();
        cx.notify();
    }

    pub fn current_user_id(&self, cx: &App) -> Option<UserId> {
        self.current_user_id_raw(cx).map(UserId)
    }

    fn new(auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ChannelMessage,
                RealtimeKind::MarkAsRead,
                RealtimeKind::LastSeenUpdated,
                RealtimeKind::Notifications,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
        });
        Self {
            auth_state,
            processed_badge_messages: HashSet::new(),
            processed_badge_order: VecDeque::new(),
        }
    }

    fn current_user_id_raw(&self, cx: &App) -> Option<i64> {
        match self.auth_state.read(cx) {
            AuthState::Authenticated(session) | AuthState::Connecting(session) => {
                session.user_id.parse::<i64>().ok()
            }
            _ => None,
        }
    }

    fn is_mention(
        &self,
        content: &str,
        references: &[u8],
        mention_bytes: &[u8],
        user_id: i64,
        clan_id: ClanId,
        cx: &App,
    ) -> bool {
        let role_ids: Vec<i64> = ClanMembersStore::global(cx)
            .read(cx)
            .member(clan_id, UserId(user_id))
            .map(|member| member.role_ids.iter().map(|role| role.get()).collect())
            .unwrap_or_default();
        mezon_client::transport::is_mention_or_reply(
            content,
            references,
            mention_bytes,
            user_id,
            &role_ids,
        )
    }

    fn maybe_show_dm_notification(
        &mut self,
        cx: &App,
        m: &ChannelMessage,
        channel_id: ChannelId,
        message_id: MessageId,
    ) {
        if cx.active_window().is_some() {
            return;
        }
        let Some(settings) = Settings::try_global(cx) else {
            return;
        };
        let (enabled, hide_content) = {
            let s = settings.read(cx);
            (s.notifications_enabled, s.notifications_hide_content)
        };
        if !enabled {
            return;
        }
        if !mark_badge_processed(
            &mut self.processed_badge_messages,
            &mut self.processed_badge_order,
            (channel_id, message_id),
        ) {
            return;
        }
        let Some(platform) = PlatformStore::try_global(cx) else {
            return;
        };
        let title = if !m.display_name.is_empty() {
            m.display_name.clone()
        } else if !m.username.is_empty() {
            m.username.clone()
        } else if !m.channel_label.is_empty() {
            m.channel_label.clone()
        } else {
            "Mezon".to_string()
        };
        let body = if hide_content {
            "New message".to_string()
        } else {
            dm_message_preview(m)
        };
        platform.read(cx).show_notification(DesktopNotification {
            title,
            body,
            channel_id: Some(channel_id.to_string()),
        });
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::ChannelMessage(m) => {
                let channel_id = badge_channel_id(m);
                let ts = if m.create_time_seconds > 0 {
                    i64::from(m.create_time_seconds)
                } else {
                    0
                };
                let user_id = self.current_user_id_raw(cx);
                let from_me = matches!(user_id, Some(uid) if uid == m.sender_id);
                let message_id = MessageId(m.message_id);

                if is_dm_message(m) {
                    if !is_content_mutation(m) {
                        let increment_unread = should_increment_dm_unread(cx, channel_id, from_me);
                        DirectMessageStore::global(cx).update(cx, |dm, cx| {
                            dm.note_message(channel_id, ts, from_me, increment_unread, cx);
                        });
                        ChannelList::global(cx).update(cx, |cl, cx| {
                            cl.note_user_channel_dm_message(
                                channel_id,
                                ts,
                                from_me,
                                increment_unread,
                                message_id,
                                cx,
                            );
                        });
                        if !from_me && increment_unread {
                            self.maybe_show_dm_notification(cx, m, channel_id, message_id);
                        }
                    }
                } else {
                    let clan_id = ClanId(m.clan_id);
                    let is_new_message = !is_content_mutation(m);
                    let seen = is_clan_message_seen(cx, m, from_me);
                    let already_seen = is_message_already_seen(cx, clan_id, channel_id, ts);
                    let removed_by_other = is_chat_remove(m) && !from_me;
                    let needs_mention_check =
                        (is_new_message && !seen && !already_seen) || removed_by_other;
                    let mentions_me = needs_mention_check
                        && user_id
                            .map(|uid| {
                                self.is_mention(
                                    &m.content,
                                    &m.references,
                                    &m.mentions,
                                    uid,
                                    clan_id,
                                    cx,
                                )
                            })
                            .unwrap_or(false);
                    let is_mention = is_new_message && !seen && !already_seen && mentions_me;
                    let badge_mention = is_mention
                        && mark_badge_processed(
                            &mut self.processed_badge_messages,
                            &mut self.processed_badge_order,
                            (channel_id, message_id),
                        );
                    ChannelList::global(cx).update(cx, |cl, cx| {
                        cl.note_channel_message(
                            clan_id,
                            channel_id,
                            badge_mention,
                            seen,
                            ts,
                            message_id,
                            cx,
                        )
                    });
                    if seen {
                        MessagesStore::global(cx).update(cx, |store, _| {
                            store.set_last_read_message(channel_id, message_id);
                        });
                    }
                    if !from_me && is_new_message {
                        ClanList::global(cx).update(cx, |cls, cx| {
                            cls.set_has_unread_message(clan_id, cx);
                        });
                    }
                    if !from_me && is_new_message && !seen && badge_mention {
                        ClanList::global(cx).update(cx, |cls, cx| {
                            cls.increment_clan_badge(clan_id, cx);
                        });
                        if m.topic_id != 0 {
                            let parent_id = ChannelId(m.channel_id);
                            ChannelList::global(cx).update(cx, |cl, cx| {
                                cl.increment_channel_for_topic(clan_id, parent_id, channel_id, cx);
                            });
                        }
                    }
                    if removed_by_other && mentions_me {
                        let message_ts = deleted_message_timestamp(m);
                        let deleted_channel = ChannelId(m.channel_id);
                        let decremented = ChannelList::global(cx).update(cx, |cl, cx| {
                            cl.decrement_channel_on_delete(clan_id, deleted_channel, message_ts, cx)
                        });
                        if decremented {
                            ClanList::global(cx).update(cx, |cls, cx| {
                                cls.decrement_badge(clan_id, 1, cx);
                            });
                        }
                    }
                }
            }
            RealtimeEvent::MarkAsRead(e) => {
                let clan_id = ClanId(e.clan_id);
                if clan_id.is_zero() {
                    let channel_id = ChannelId(e.channel_id);
                    DirectMessageStore::global(cx).update(cx, |dm, cx| {
                        dm.note_read(channel_id, cx);
                    });
                } else if e.category_id == 0 {
                    ChannelList::global(cx).update(cx, |cl, cx| {
                        cl.apply_mark_as_read_clan(clan_id, cx);
                    });
                    ClanList::global(cx).update(cx, |cls, cx| cls.apply_badge_read(clan_id, cx));
                } else if e.channel_id == 0 {
                    ChannelList::global(cx).update(cx, |cl, cx| {
                        cl.apply_mark_as_read_category(clan_id, e.category_id, cx);
                    });
                } else {
                    let channel_id = ChannelId(e.channel_id);
                    ChannelList::global(cx).update(cx, |cl, cx| {
                        cl.apply_read(clan_id, channel_id, cx);
                        cl.apply_topic_read(channel_id, cx);
                    });
                }
            }
            RealtimeEvent::LastSeenUpdated(e) => {
                let channel_id = ChannelId(e.channel_id);
                let clan_id = ClanId(e.clan_id);
                let new_badge = e.badge_count.max(0) as u32;
                let seen_ts = i64::from(e.timestamp_seconds);
                let seen_message_id = MessageId(e.message_id);
                if clan_id.is_zero() {
                    DirectMessageStore::global(cx).update(cx, |dm, cx| {
                        dm.note_read(channel_id, cx);
                    });
                } else {
                    ChannelList::global(cx).update(cx, |cl, cx| {
                        cl.apply_last_seen(
                            clan_id,
                            channel_id,
                            new_badge,
                            seen_ts,
                            seen_message_id,
                            cx,
                        );
                        cl.apply_topic_read(channel_id, cx);
                    });
                }
                if !seen_message_id.is_zero() {
                    MessagesStore::global(cx).update(cx, |store, _| {
                        store.set_last_read_message(channel_id, seen_message_id);
                    });
                }
            }
            RealtimeEvent::Notifications(n) => {
                tracing::debug!(count = n.notifications.len(), "badge: notifications event");
                for notif in &n.notifications {
                    self.handle_notification(notif, cx);
                }
            }
            _ => {}
        }
    }

    fn handle_notification(&mut self, notif: &Notification, cx: &mut Context<Self>) {
        tracing::debug!(
            code = notif.code,
            channel_type = notif.channel_type,
            clan_id = notif.clan_id,
            channel_id = notif.channel_id,
            topic_id = notif.topic_id,
            content_len = notif.content.len(),
            "badge: handle notification"
        );
        if notif.code != NOTIFICATION_USER_MENTIONED && notif.code != NOTIFICATION_USER_REPLIED {
            tracing::debug!(
                code = notif.code,
                "badge: skip notification, code not mention/reply"
            );
            return;
        }
        if notif.channel_type == NOTIFICATION_CHANNEL_TYPE_APP
            || notif.channel_type == NOTIFICATION_CHANNEL_TYPE_VOICE
        {
            tracing::debug!(
                channel_type = notif.channel_type,
                "badge: skip notification, app/voice channel"
            );
            return;
        }
        let clan_id = ClanId(notif.clan_id);
        if clan_id.is_zero() {
            tracing::debug!("badge: skip notification, no clan_id");
            return;
        }
        let channel_id = ChannelId(notif.channel_id);
        if is_viewing_channel(cx, channel_id) {
            tracing::debug!(
                channel_id = notif.channel_id,
                "badge: skip notification, viewing channel"
            );
            return;
        }
        if notif.channel_type == NOTIFICATION_CHANNEL_TYPE_THREAD
            && let Some(desc) = notif.channel.as_ref()
        {
            let parent_id = ChannelId(desc.parent_id);
            let label = desc.channel_label.clone();
            ChannelList::global(cx).update(cx, |cl, cx| {
                cl.ensure_thread_with_parent(channel_id, parent_id, clan_id, label, cx);
            });
        }
        let (message_id_raw, content_time) =
            mezon_client::transport::parse_notification_content(&notif.content);
        tracing::debug!(
            message_id = message_id_raw,
            content_time,
            content_len = notif.content.len(),
            content_first_byte = notif.content.first().copied(),
            "badge: parsed notification content"
        );
        let message_id = MessageId(message_id_raw);
        if message_id.is_zero() {
            tracing::debug!(
                content_len = notif.content.len(),
                "badge: skip notification, no message_id in content"
            );
            return;
        }
        let msg_time = if content_time > 0 {
            content_time
        } else {
            i64::from(notif.create_time_seconds)
        };
        let badge_channel = if notif.topic_id != 0 {
            ChannelId(notif.topic_id)
        } else {
            channel_id
        };
        if is_message_already_seen(cx, clan_id, badge_channel, msg_time) {
            tracing::debug!(
                channel_id = badge_channel.get(),
                msg_time,
                "badge: skip notification, already seen"
            );
            return;
        }
        if !mark_badge_processed(
            &mut self.processed_badge_messages,
            &mut self.processed_badge_order,
            (badge_channel, message_id),
        ) {
            tracing::debug!(
                channel_id = badge_channel.get(),
                message_id = message_id.get(),
                "badge: skip notification, duplicate"
            );
            return;
        }
        tracing::debug!(
            clan_id = notif.clan_id,
            channel_id = badge_channel.get(),
            message_id = message_id.get(),
            "badge: apply notification increment"
        );
        maybe_show_desktop_notification(cx, notif, badge_channel);
        ChannelList::global(cx).update(cx, |cl, cx| {
            cl.note_channel_message(
                clan_id,
                badge_channel,
                true,
                false,
                msg_time,
                message_id,
                cx,
            );
        });
        ClanList::global(cx).update(cx, |cls, cx| {
            cls.increment_clan_badge(clan_id, cx);
        });
        if notif.topic_id != 0 {
            ChannelList::global(cx).update(cx, |cl, cx| {
                cl.increment_channel_for_topic(clan_id, channel_id, badge_channel, cx);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_badge_processed_allows_first_skips_duplicate() {
        let mut seen = HashSet::new();
        let mut order = VecDeque::new();
        let key = (ChannelId(7), MessageId(42));
        assert!(mark_badge_processed(&mut seen, &mut order, key));
        assert!(!mark_badge_processed(&mut seen, &mut order, key));
        assert!(mark_badge_processed(
            &mut seen,
            &mut order,
            (ChannelId(7), MessageId(43))
        ));
    }

    #[test]
    fn mark_badge_processed_keys_on_channel_and_message() {
        let mut seen = HashSet::new();
        let mut order = VecDeque::new();
        assert!(mark_badge_processed(
            &mut seen,
            &mut order,
            (ChannelId(1), MessageId(99))
        ));
        assert!(mark_badge_processed(
            &mut seen,
            &mut order,
            (ChannelId(2), MessageId(99))
        ));
    }

    #[test]
    fn mark_badge_processed_zero_message_id_is_never_recorded() {
        let mut seen = HashSet::new();
        let mut order = VecDeque::new();
        let key = (ChannelId(7), MessageId(0));
        assert!(mark_badge_processed(&mut seen, &mut order, key));
        assert!(mark_badge_processed(&mut seen, &mut order, key));
        assert!(seen.is_empty());
        assert!(order.is_empty());
    }

    #[test]
    fn mark_badge_processed_evicts_oldest_over_capacity() {
        let mut seen = HashSet::new();
        let mut order = VecDeque::new();
        for i in 1..=(MAX_BADGE_DEDUP as i64 + 1) {
            assert!(mark_badge_processed(
                &mut seen,
                &mut order,
                (ChannelId(1), MessageId(i))
            ));
        }
        assert!(seen.len() <= MAX_BADGE_DEDUP);
        assert_eq!(seen.len(), order.len());
        assert!(!seen.contains(&(ChannelId(1), MessageId(1))));
        assert!(seen.contains(&(ChannelId(1), MessageId(MAX_BADGE_DEDUP as i64 + 1))));
    }
}
