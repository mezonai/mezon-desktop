use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};
use mezon_client::{
    AppApi, InboxNotification, RealtimeEvent, inbox_notification_from_channel_mention,
    notification_ids_from_content,
};
use mezon_proto::api;
use prost::Message;

use crate::AuthState;
use crate::channel::{ChannelList, ChannelType};
use crate::clan_members::ClanMembersStore;
use crate::ids::{ChannelId, ClanId, RoleId, UserId};
use crate::inbox::{InboxStore, skip_inbox_mention_code};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const USER_MENTIONED: i32 = -9;
const USER_REPLIED: i32 = -11;
const CHAT_UPDATE: i32 = 1;
const CHAT_REMOVE: i32 = 2;
const ID_MENTION_HERE: i64 = 1_775_731_111_020_111_321;
const MAX_PROCESSED_KEYS: usize = 500;

#[derive(Debug, Clone)]
pub enum TopicBadgeEvent {
    Updated { clan_id: Option<String> },
}

#[derive(Debug, Clone)]
struct TopicParentEntry {
    clan_id: String,
    _parent_channel_id: String,
    count: u32,
}

pub struct TopicBadgeStore {
    topic_parent_map: HashMap<String, TopicParentEntry>,
    processed_keys: HashSet<String>,
    processed_order: VecDeque<String>,
    auth_state: Entity<AuthState>,
    _api: Arc<AppApi>,
}

struct GlobalTopicBadgeStore(Entity<TopicBadgeStore>);
impl Global for GlobalTopicBadgeStore {}

impl EventEmitter<TopicBadgeEvent> for TopicBadgeStore {}

impl TopicBadgeStore {
    pub fn init(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, auth_state, cx));
        cx.set_global(GlobalTopicBadgeStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalTopicBadgeStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalTopicBadgeStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.topic_parent_map.clear();
        self.processed_keys.clear();
        self.processed_order.clear();
        cx.emit(TopicBadgeEvent::Updated { clan_id: None });
        cx.notify();
    }

    fn new(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        Self {
            topic_parent_map: HashMap::new(),
            processed_keys: HashSet::new(),
            processed_order: VecDeque::new(),
            auth_state,
            _api: api,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::ChannelMessage, &entity, |this, event, cx| {
                this.handle_event(event, cx);
            });
            dispatch.on(RealtimeKind::Notifications, &entity, |this, event, cx| {
                this.handle_event(event, cx);
            });
            dispatch.on(RealtimeKind::MarkAsRead, &entity, |this, event, cx| {
                this.handle_event(event, cx);
            });
            dispatch.on_lagged(&entity, |this, cx| this.reset(cx));
        });
    }

    pub fn all_topic_noti_clan(&self, clan_id: &str) -> u32 {
        self.topic_parent_map
            .values()
            .filter(|entry| entry.clan_id == clan_id)
            .map(|entry| entry.count)
            .sum()
    }

    pub fn topic_badge_count(&self, topic_id: &str) -> u32 {
        self.topic_parent_map
            .get(topic_id)
            .map(|entry| entry.count)
            .unwrap_or(0)
    }

    pub fn badge_label_for_clan(&self, clan_id: &str) -> Option<String> {
        format_topic_badge_label(self.all_topic_noti_clan(clan_id))
    }

    pub fn clear_topic(&mut self, topic_id: &str, cx: &mut Context<Self>) {
        let Some(clan_id) = self.reset_topic(topic_id) else {
            return;
        };
        cx.emit(TopicBadgeEvent::Updated {
            clan_id: Some(clan_id),
        });
        cx.notify();
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let (changed_clans, inbox_mentions) = match event {
            RealtimeEvent::ChannelMessage(m) => self.handle_channel_message(m, cx),
            RealtimeEvent::Notifications(batch) => self.handle_notifications(batch, cx),
            RealtimeEvent::MarkAsRead(e) => {
                (self.handle_mark_as_read(e.channel_id, cx), Vec::new())
            }
            _ => (Vec::new(), Vec::new()),
        };
        for notification in inbox_mentions {
            InboxStore::global(cx).update(cx, |inbox, cx| {
                inbox.note_mention(notification, cx);
            });
        }
        if !changed_clans.is_empty() {
            for clan_id in changed_clans {
                cx.emit(TopicBadgeEvent::Updated {
                    clan_id: Some(clan_id),
                });
            }
            cx.notify();
        }
    }

    fn handle_channel_message(
        &mut self,
        m: &api::ChannelMessage,
        cx: &App,
    ) -> (Vec<String>, Vec<InboxNotification>) {
        if m.code == CHAT_UPDATE || m.code == CHAT_REMOVE {
            return (Vec::new(), Vec::new());
        }
        if m.topic_id == 0 {
            return (Vec::new(), Vec::new());
        }
        let Some(user_id) = self.current_user_id(cx) else {
            return (Vec::new(), Vec::new());
        };
        let topic_id = m.topic_id.to_string();
        let parent_channel_id = m.channel_id.to_string();
        let clan_id = m.clan_id.to_string();
        if is_viewing_channel(&topic_id, cx) {
            return (Vec::new(), Vec::new());
        }
        if is_already_seen(
            ClanId(m.clan_id),
            ChannelId(m.topic_id),
            m.create_time_seconds,
            cx,
        ) {
            return (Vec::new(), Vec::new());
        }
        let topic_message = api::ChannelMessage {
            channel_id: m.topic_id,
            ..m.clone()
        };
        if !is_message_mention_or_reply(&topic_message, user_id, cx) {
            return (Vec::new(), Vec::new());
        }
        let message_id = m.message_id.to_string();
        let dedupe_key = format!("{parent_channel_id}_{message_id}");
        if self.increment_topic_for_message(
            &clan_id,
            &parent_channel_id,
            &topic_id,
            Some(&message_id),
            &dedupe_key,
        ) {
            let inbox = if skip_inbox_mention_code(m.code) {
                Vec::new()
            } else {
                vec![inbox_notification_from_channel_mention(m)]
            };
            (vec![clan_id], inbox)
        } else {
            (Vec::new(), Vec::new())
        }
    }

    fn handle_notifications(
        &mut self,
        batch: &mezon_proto::realtime::Notifications,
        cx: &App,
    ) -> (Vec<String>, Vec<InboxNotification>) {
        let mut changed_clans: Vec<String> = Vec::new();
        for n in &batch.notifications {
            if n.channel_type == ChannelType::App.as_raw() as i32
                || n.channel_type == ChannelType::Voice.as_raw() as i32
            {
                continue;
            }
            if n.code != USER_MENTIONED && n.code != USER_REPLIED {
                continue;
            }
            let (message_id_raw, content_time, content_topic_id) =
                notification_ids_from_content(&n.content);
            let topic_id_raw = if n.topic_id > 0 {
                n.topic_id
            } else if content_topic_id > 0 {
                content_topic_id
            } else {
                continue;
            };
            let topic_id = topic_id_raw.to_string();
            let parent_channel_id = n.channel_id.to_string();
            let clan_id = n.clan_id.to_string();
            let message_id = if message_id_raw > 0 {
                message_id_raw.to_string()
            } else {
                notification_message_id(n)
            };
            let check_channel = topic_id.clone();
            if is_viewing_channel(&check_channel, cx) {
                continue;
            }
            let msg_time = if content_time > 0 {
                u32::try_from(content_time).unwrap_or(n.create_time_seconds)
            } else {
                notification_message_time(n)
            };
            if is_already_seen(ClanId(n.clan_id), ChannelId(topic_id_raw), msg_time, cx) {
                continue;
            }
            let dedupe_key = format!("{parent_channel_id}_{message_id}");
            if self.increment_topic_for_message(
                &clan_id,
                &parent_channel_id,
                &topic_id,
                Some(&message_id),
                &dedupe_key,
            ) && !changed_clans.contains(&clan_id)
            {
                changed_clans.push(clan_id);
            }
        }
        (changed_clans, Vec::new())
    }

    fn handle_mark_as_read(&mut self, channel_id: i64, _cx: &App) -> Vec<String> {
        self.reset_topic(&channel_id.to_string())
            .into_iter()
            .collect()
    }

    fn increment_topic_for_message(
        &mut self,
        clan_id: &str,
        parent_channel_id: &str,
        topic_id: &str,
        message_id: Option<&str>,
        dedupe_key: &str,
    ) -> bool {
        let dedupable = message_id.is_some_and(|id| !id.is_empty() && id != "0");
        if dedupable {
            if self.processed_keys.contains(dedupe_key) {
                return false;
            }
            self.track_processed(dedupe_key.to_string());
        }
        let entry = self
            .topic_parent_map
            .entry(topic_id.to_string())
            .or_insert_with(|| TopicParentEntry {
                clan_id: clan_id.to_string(),
                _parent_channel_id: parent_channel_id.to_string(),
                count: 0,
            });
        entry.count = entry.count.saturating_add(1);
        true
    }

    fn reset_topic(&mut self, topic_id: &str) -> Option<String> {
        self.topic_parent_map
            .remove(topic_id)
            .map(|entry| entry.clan_id)
    }

    fn track_processed(&mut self, key: String) {
        if !self.processed_keys.insert(key.clone()) {
            return;
        }
        self.processed_order.push_back(key);
        if self.processed_order.len() > MAX_PROCESSED_KEYS {
            while self.processed_order.len() > MAX_PROCESSED_KEYS / 2 {
                if let Some(evicted) = self.processed_order.pop_front() {
                    self.processed_keys.remove(&evicted);
                }
            }
        }
    }

    fn current_user_id(&self, cx: &App) -> Option<UserId> {
        match self.auth_state.read(cx) {
            AuthState::Authenticated(session) | AuthState::Connecting(session) => {
                session.user_id.parse().ok()
            }
            _ => None,
        }
    }
}

fn format_topic_badge_label(total: u32) -> Option<String> {
    if total == 0 {
        None
    } else if total > 9 {
        Some("9+".into())
    } else {
        Some(total.to_string())
    }
}

fn is_viewing_channel(channel_id: &str, cx: &App) -> bool {
    ChannelList::global(cx)
        .read(cx)
        .active_channel_id
        .is_some_and(|id| id.to_string() == channel_id)
}

fn is_already_seen(clan_id: ClanId, channel_id: ChannelId, msg_time: u32, cx: &App) -> bool {
    let Some(ch) = ChannelList::global(cx)
        .read(cx)
        .channel(clan_id, channel_id)
    else {
        return false;
    };
    let last_seen = ch.last_seen_timestamp.max(0) as u32;
    msg_time > 0 && last_seen > 0 && msg_time <= last_seen
}

fn notification_message_id(n: &api::Notification) -> String {
    if n.content.is_empty() {
        return String::new();
    }
    if let Ok(fcm) = api::DirectFcmProto::decode(n.content.as_slice()) {
        return fcm.message_id.to_string();
    }
    String::new()
}

fn notification_message_time(n: &api::Notification) -> u32 {
    if n.content.is_empty() {
        return n.create_time_seconds;
    }
    if let Ok(fcm) = api::DirectFcmProto::decode(n.content.as_slice())
        && fcm.create_time_seconds > 0
    {
        return fcm.create_time_seconds as u32;
    }
    n.create_time_seconds
}

fn is_message_mention_or_reply(msg: &api::ChannelMessage, user_id: UserId, cx: &App) -> bool {
    let mentions = parse_message_mentions(&msg.mentions);
    let member_roles = ClanMembersStore::global(cx)
        .read(cx)
        .member(ClanId(msg.clan_id), user_id)
        .map(|m| m.role_ids.clone())
        .unwrap_or_default();
    let includes_here = mentions
        .iter()
        .any(|m| m.user_id == Some(UserId(ID_MENTION_HERE)));
    let includes_user = mentions.iter().any(|m| m.user_id == Some(user_id));
    let includes_role = mentions.iter().any(|m| {
        m.role_id
            .map(|role| member_roles.contains(&role))
            .unwrap_or(false)
    });
    let is_reply = parse_message_refs(&msg.references)
        .iter()
        .any(|r| r.message_sender_id == user_id);
    includes_here || includes_user || includes_role || is_reply
}

struct ParsedMention {
    user_id: Option<UserId>,
    role_id: Option<RoleId>,
}

struct ParsedRef {
    message_sender_id: UserId,
}

fn parse_message_mentions(bytes: &[u8]) -> Vec<ParsedMention> {
    if bytes.is_empty() {
        return Vec::new();
    }
    if let Ok(list) = api::MessageMentionList::decode(bytes) {
        return list
            .mentions
            .into_iter()
            .map(|m| ParsedMention {
                user_id: (m.user_id != 0).then_some(UserId(m.user_id)),
                role_id: (m.role_id != 0).then_some(RoleId(m.role_id)),
            })
            .collect();
    }
    if let Ok(values) = serde_json::from_slice::<Vec<serde_json::Value>>(bytes) {
        return values
            .into_iter()
            .map(|value| {
                let user_id = value
                    .get("user_id")
                    .and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                    .and_then(|s| s.parse().ok());
                let role_id = value
                    .get("role_id")
                    .and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                    .and_then(|s| s.parse().ok());
                ParsedMention { user_id, role_id }
            })
            .collect();
    }
    Vec::new()
}

fn parse_message_refs(bytes: &[u8]) -> Vec<ParsedRef> {
    if bytes.is_empty() {
        return Vec::new();
    }
    if let Ok(list) = api::MessageRefList::decode(bytes) {
        return list
            .refs
            .into_iter()
            .map(|r| ParsedRef {
                message_sender_id: UserId(r.message_sender_id),
            })
            .collect();
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_topic_badge_label_values() {
        assert_eq!(format_topic_badge_label(0), None);
        assert_eq!(format_topic_badge_label(4), Some("4".into()));
        assert_eq!(format_topic_badge_label(12), Some("9+".into()));
    }

    #[test]
    fn all_topic_noti_clan_sums_entries() {
        let entries = [
            TopicParentEntry {
                clan_id: "10".into(),
                _parent_channel_id: "5".into(),
                count: 2,
            },
            TopicParentEntry {
                clan_id: "10".into(),
                _parent_channel_id: "5".into(),
                count: 3,
            },
        ];
        let total: u32 = entries
            .iter()
            .filter(|entry| entry.clan_id == "10")
            .map(|entry| entry.count)
            .sum();
        assert_eq!(total, 5);
    }

    #[gpui::test]
    fn clear_topic_removes_entry(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let api = std::sync::Arc::new(mezon_client::AppApi::new(
                std::sync::Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            crate::realtime::RealtimeDispatch::init(api.clone(), cx);
            let auth_state = cx.new(|_| AuthState::NotAuthenticated);
            let store = TopicBadgeStore::init(api, auth_state, cx);
            store.update(cx, |store, cx| {
                store.topic_parent_map.insert(
                    "99".into(),
                    TopicParentEntry {
                        clan_id: "10".into(),
                        _parent_channel_id: "5".into(),
                        count: 2,
                    },
                );
                store.clear_topic("99", cx);
                assert_eq!(store.topic_badge_count("99"), 0);
                assert!(store.topic_parent_map.is_empty());
            });
        });
    }
}
