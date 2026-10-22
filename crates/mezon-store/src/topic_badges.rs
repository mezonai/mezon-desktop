use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};
use mezon_client::{
    AppApi, InboxNotification, RealtimeEvent, TopicDiscussion,
    inbox_notification_from_channel_mention, notification_ids_from_content,
};
use mezon_proto::api;
use prost::Message;

use crate::AuthState;
use crate::channel::{ChannelList, ChannelType};
use crate::clan::ClanList;
use crate::clan_members::ClanMembersStore;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use crate::inbox::{InboxStore, skip_inbox_mention_code};
use crate::messages::MessagesStore;
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::topics::TopicsStore;

const USER_MENTIONED: i32 = -9;
const USER_REPLIED: i32 = -11;
const CHAT_UPDATE: i32 = 1;
const CHAT_REMOVE: i32 = 2;
const MAX_PROCESSED_KEYS: usize = 500;

#[derive(Debug, Clone)]
pub enum TopicBadgeEvent {
    Updated { clan_id: Option<String> },
}

#[derive(Debug, Clone)]
struct TopicParentEntry {
    clan_id: String,
    parent_channel_id: String,
    unread: bool,
    count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopicListing {
    reset_generation: u64,
    clear_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckedTopic {
    clan_id: String,
    parent_channel_id: String,
}

pub struct TopicBadgeStore {
    topic_parent_map: HashMap<String, TopicParentEntry>,
    checked_unread: HashMap<String, CheckedTopic>,
    pending_checks: usize,
    clear_generation: u64,
    cleared_during_checks: HashMap<String, u64>,
    reset_generation: u64,
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
        cx.subscribe(&entity, |_, event: &TopicBadgeEvent, cx| {
            let TopicBadgeEvent::Updated {
                clan_id: Some(clan_id),
            } = event
            else {
                return;
            };
            let (Ok(clan_id), Some(clans)) = (clan_id.parse::<ClanId>(), ClanList::try_global(cx))
            else {
                return;
            };
            clans.update(cx, |clans, cx| {
                clans.sync_has_unread_from_channels(clan_id, cx);
            });
        })
        .detach();
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
        self.checked_unread.clear();
        self.pending_checks = 0;
        self.cleared_during_checks.clear();
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.processed_keys.clear();
        self.processed_order.clear();
        cx.emit(TopicBadgeEvent::Updated { clan_id: None });
        cx.notify();
    }

    fn new(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        Self {
            topic_parent_map: HashMap::new(),
            checked_unread: HashMap::new(),
            pending_checks: 0,
            clear_generation: 0,
            cleared_during_checks: HashMap::new(),
            reset_generation: 0,
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
            dispatch.on(RealtimeKind::LastSeenUpdated, &entity, |this, event, cx| {
                this.handle_event(event, cx);
            });
            dispatch.on_lagged(&entity, |this, cx| this.reset(cx));
        });
    }

    pub fn topic_badge_count(&self, topic_id: &str) -> u32 {
        self.topic_parent_map
            .get(topic_id)
            .map(|entry| entry.count)
            .unwrap_or(0)
    }

    pub fn unread_parent_ids(&self, clan_id: ClanId) -> HashSet<ChannelId> {
        let clan_key = clan_id.to_string();
        self.topic_parent_map
            .values()
            .filter(|entry| entry.unread && entry.clan_id == clan_key)
            .filter_map(|entry| entry.parent_channel_id.parse().ok())
            .collect()
    }

    pub fn any_topic_unread(&self, clan_id: ClanId) -> bool {
        let clan_key = clan_id.to_string();
        self.clan_has_unread_topic(clan_id)
            || self
                .checked_unread
                .values()
                .any(|topic| topic.clan_id == clan_key)
    }

    pub fn clan_has_unread_topic(&self, clan_id: ClanId) -> bool {
        let clan_key = clan_id.to_string();
        self.topic_parent_map
            .values()
            .any(|entry| entry.unread && entry.clan_id == clan_key)
    }

    pub fn clear_parents(
        &mut self,
        clan_id: ClanId,
        parent_ids: &[ChannelId],
        cx: &mut Context<Self>,
    ) {
        let clan_key = clan_id.to_string();
        let parents: HashSet<String> = parent_ids.iter().map(ChannelId::to_string).collect();
        let before = self.topic_parent_map.len() + self.checked_unread.len();
        self.topic_parent_map.retain(|_, entry| {
            entry.clan_id != clan_key || !parents.contains(&entry.parent_channel_id)
        });
        self.checked_unread.retain(|_, topic| {
            topic.clan_id != clan_key || !parents.contains(&topic.parent_channel_id)
        });
        if self.topic_parent_map.len() + self.checked_unread.len() != before {
            cx.emit(TopicBadgeEvent::Updated {
                clan_id: Some(clan_key),
            });
            cx.notify();
        }
    }

    pub fn clear_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let clan_key = clan_id.to_string();
        let before = self.topic_parent_map.len() + self.checked_unread.len();
        self.topic_parent_map
            .retain(|_, entry| entry.clan_id != clan_key);
        self.checked_unread
            .retain(|_, topic| topic.clan_id != clan_key);
        if self.topic_parent_map.len() + self.checked_unread.len() != before {
            cx.emit(TopicBadgeEvent::Updated {
                clan_id: Some(clan_key),
            });
            cx.notify();
        }
    }

    pub fn topic_has_unread(&self, topic_id: &str) -> bool {
        self.checked_unread.contains_key(topic_id)
            || self
                .topic_parent_map
                .get(topic_id)
                .is_some_and(|entry| entry.unread)
    }

    pub fn channel_has_unread_topic(&self, clan_id: ClanId, channel_id: ChannelId) -> bool {
        let clan_key = clan_id.to_string();
        let channel_key = channel_id.to_string();
        self.checked_unread
            .values()
            .any(|topic| topic.clan_id == clan_key && topic.parent_channel_id == channel_key)
            || self.topic_parent_map.values().any(|entry| {
                entry.unread && entry.clan_id == clan_key && entry.parent_channel_id == channel_key
            })
    }

    pub fn begin_topic_listing(&mut self) -> TopicListing {
        self.pending_checks += 1;
        TopicListing {
            reset_generation: self.reset_generation,
            clear_generation: self.clear_generation,
        }
    }

    pub fn end_topic_listing(
        &mut self,
        clan_id: ClanId,
        listed: &[TopicDiscussion],
        listing: TopicListing,
        cx: &mut Context<Self>,
    ) {
        if listing.reset_generation != self.reset_generation {
            return;
        }
        self.pending_checks = self.pending_checks.saturating_sub(1);
        self.apply_listed_topics(clan_id, listed, listing.clear_generation, cx);
        if self.pending_checks == 0 {
            self.cleared_during_checks.clear();
        }
    }

    fn apply_listed_topics(
        &mut self,
        clan_id: ClanId,
        listed: &[TopicDiscussion],
        clear_generation: u64,
        cx: &mut Context<Self>,
    ) {
        let clan_key = clan_id.to_string();
        let mut changed = false;
        for topic in listed {
            let unread = topic.unread_count > 0
                && self
                    .cleared_during_checks
                    .get(&topic.id)
                    .is_none_or(|cleared| *cleared <= clear_generation)
                && !is_viewing_channel(&topic.id, cx);
            if unread {
                let checked = CheckedTopic {
                    clan_id: clan_key.clone(),
                    parent_channel_id: topic.channel_id.clone(),
                };
                changed |= self
                    .checked_unread
                    .insert(topic.id.clone(), checked.clone())
                    != Some(checked);
            } else {
                changed |= self.checked_unread.remove(&topic.id).is_some();
            }
        }
        if changed {
            cx.emit(TopicBadgeEvent::Updated {
                clan_id: Some(clan_key),
            });
            cx.notify();
        }
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
            RealtimeEvent::LastSeenUpdated(e) => (
                self.reset_topic(&e.channel_id.to_string())
                    .into_iter()
                    .collect(),
                Vec::new(),
            ),
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
        if m.sender_id == user_id.get() {
            return (Vec::new(), Vec::new());
        }
        let topic_id = m.topic_id.to_string();
        let parent_channel_id = m.channel_id.to_string();
        let clan_id = m.clan_id.to_string();
        if is_viewing_channel(&topic_id, cx) {
            return (Vec::new(), Vec::new());
        }
        if is_already_seen(
            ClanId(m.clan_id),
            ChannelId(m.topic_id),
            MessageId(m.message_id),
            m.create_time_seconds,
            cx,
        ) {
            return (Vec::new(), Vec::new());
        }
        let topic_message = api::ChannelMessage {
            channel_id: m.topic_id,
            ..m.clone()
        };
        let badge_mention = is_message_mention_or_reply(&topic_message, user_id, cx);
        let message_id = m.message_id.to_string();
        let dedupe_key = format!("{parent_channel_id}_{message_id}");
        if self.increment_topic_for_message(
            &clan_id,
            &parent_channel_id,
            &topic_id,
            Some(&message_id),
            &dedupe_key,
            badge_mention,
        ) {
            let inbox = if !badge_mention
                || skip_inbox_mention_code(m.code)
                || !is_message_inbox_mention_or_reply(&topic_message, user_id, cx)
            {
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
            if is_already_seen(
                ClanId(n.clan_id),
                ChannelId(topic_id_raw),
                MessageId(message_id.parse().unwrap_or_default()),
                msg_time,
                cx,
            ) {
                continue;
            }
            let dedupe_key = format!("{parent_channel_id}_{message_id}");
            if self.increment_topic_for_message(
                &clan_id,
                &parent_channel_id,
                &topic_id,
                Some(&message_id),
                &dedupe_key,
                true,
            ) && !changed_clans.contains(&clan_id)
            {
                changed_clans.push(clan_id);
            }
        }
        (changed_clans, Vec::new())
    }

    fn handle_mark_as_read(&mut self, channel_id: i64, _cx: &App) -> Vec<String> {
        if channel_id == 0 {
            return Vec::new();
        }
        let channel_key = channel_id.to_string();
        self.note_cleared(&channel_key);
        let mut cleared: Vec<String> = Vec::new();
        self.topic_parent_map.retain(|topic_id, entry| {
            let read = *topic_id == channel_key || entry.parent_channel_id == channel_key;
            if read && !cleared.contains(&entry.clan_id) {
                cleared.push(entry.clan_id.clone());
            }
            !read
        });
        self.checked_unread.retain(|topic_id, topic| {
            let read = *topic_id == channel_key || topic.parent_channel_id == channel_key;
            if read && !cleared.contains(&topic.clan_id) {
                cleared.push(topic.clan_id.clone());
            }
            !read
        });
        cleared
    }

    fn increment_topic_for_message(
        &mut self,
        clan_id: &str,
        parent_channel_id: &str,
        topic_id: &str,
        message_id: Option<&str>,
        dedupe_key: &str,
        increment_badge: bool,
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
                parent_channel_id: parent_channel_id.to_string(),
                unread: false,
                count: 0,
            });
        entry.unread = true;
        if increment_badge {
            entry.count = entry.count.saturating_add(1);
        }
        true
    }

    fn reset_topic(&mut self, topic_id: &str) -> Option<String> {
        self.note_cleared(topic_id);
        let checked = self
            .checked_unread
            .remove(topic_id)
            .map(|topic| topic.clan_id);
        self.topic_parent_map
            .remove(topic_id)
            .map(|entry| entry.clan_id)
            .or(checked)
    }

    fn note_cleared(&mut self, topic_id: &str) {
        if self.pending_checks == 0 {
            return;
        }
        self.clear_generation = self.clear_generation.wrapping_add(1);
        self.cleared_during_checks
            .insert(topic_id.to_string(), self.clear_generation);
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

fn is_viewing_channel(channel_id: &str, cx: &App) -> bool {
    let Ok(topic_id) = channel_id.parse::<i64>() else {
        return false;
    };
    if cx.active_window().is_none() {
        return false;
    }
    let topics = TopicsStore::global(cx);
    let topics = topics.read(cx);
    topics.is_panel_open()
        && topics.active_topic_id() == Some(topic_id)
        && !MessagesStore::global(cx).read(cx).topic_has_more_bottom()
}

fn is_already_seen(
    clan_id: ClanId,
    channel_id: ChannelId,
    message_id: MessageId,
    msg_time: u32,
    cx: &App,
) -> bool {
    if MessagesStore::global(cx)
        .read(cx)
        .has_read_message(channel_id, message_id)
    {
        return true;
    }
    let last_seen = ChannelList::global(cx)
        .read(cx)
        .channel(clan_id, channel_id)
        .map(|ch| ch.last_seen_timestamp.max(0) as u32)
        .unwrap_or(0);
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

fn is_message_inbox_mention_or_reply(msg: &api::ChannelMessage, user_id: UserId, cx: &App) -> bool {
    let member_roles: Vec<i64> = ClanMembersStore::global(cx)
        .read(cx)
        .member(ClanId(msg.clan_id), user_id)
        .map(|member| member.role_ids.iter().map(|role| role.get()).collect())
        .unwrap_or_default();
    mezon_client::transport::is_inbox_mention_or_reply(
        &msg.content,
        &msg.references,
        &msg.mentions,
        user_id.get(),
        &member_roles,
    )
}

fn is_message_mention_or_reply(msg: &api::ChannelMessage, user_id: UserId, cx: &App) -> bool {
    let member_roles: Vec<i64> = ClanMembersStore::global(cx)
        .read(cx)
        .member(ClanId(msg.clan_id), user_id)
        .map(|member| member.role_ids.iter().map(|role| role.get()).collect())
        .unwrap_or_default();
    mezon_client::transport::is_mention_or_reply(
        &msg.content,
        &msg.references,
        &msg.mentions,
        user_id.get(),
        &member_roles,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
                        parent_channel_id: "5".into(),
                        unread: true,
                        count: 2,
                    },
                );
                store.clear_topic("99", cx);
                assert_eq!(store.topic_badge_count("99"), 0);
                assert!(store.topic_parent_map.is_empty());
            });
        });
    }

    fn entry(clan_id: &str, parent_channel_id: &str, unread: bool) -> TopicParentEntry {
        TopicParentEntry {
            clan_id: clan_id.into(),
            parent_channel_id: parent_channel_id.into(),
            unread,
            count: 0,
        }
    }

    fn init_store(cx: &mut App) -> Entity<TopicBadgeStore> {
        let api = std::sync::Arc::new(mezon_client::AppApi::new(
            std::sync::Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        crate::realtime::RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| AuthState::NotAuthenticated);
        TopicBadgeStore::init(api, auth_state, cx)
    }

    #[gpui::test]
    fn unread_topics_mark_their_parent_and_clan_unread(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store
                    .topic_parent_map
                    .insert("91".into(), entry("10", "5", true));
                store
                    .topic_parent_map
                    .insert("92".into(), entry("10", "6", false));
                store
                    .topic_parent_map
                    .insert("93".into(), entry("11", "7", true));

                assert_eq!(
                    store.unread_parent_ids(ClanId(10)),
                    HashSet::from([ChannelId(5)])
                );
                assert!(store.clan_has_unread_topic(ClanId(10)));
                assert!(store.clan_has_unread_topic(ClanId(11)));
                assert!(!store.clan_has_unread_topic(ClanId(12)));

                store.clear_topic("91", cx);
                assert!(store.unread_parent_ids(ClanId(10)).is_empty());
                assert!(!store.clan_has_unread_topic(ClanId(10)));
            });
        });
    }

    #[gpui::test]
    fn marking_a_channel_read_clears_its_topics_and_the_topic_itself(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store
                    .topic_parent_map
                    .insert("91".into(), entry("10", "5", true));
                store
                    .topic_parent_map
                    .insert("92".into(), entry("10", "5", true));
                store
                    .topic_parent_map
                    .insert("93".into(), entry("10", "6", true));

                assert_eq!(store.handle_mark_as_read(5, cx), vec!["10".to_string()]);
                assert_eq!(store.topic_parent_map.len(), 1);
                assert!(store.topic_parent_map.contains_key("93"));

                assert_eq!(store.handle_mark_as_read(93, cx), vec!["10".to_string()]);
                assert!(store.topic_parent_map.is_empty());
                assert!(store.handle_mark_as_read(0, cx).is_empty());
            });
        });
    }

    #[gpui::test]
    fn clearing_parents_or_a_clan_stays_inside_that_clan(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store
                    .topic_parent_map
                    .insert("91".into(), entry("10", "5", true));
                store
                    .topic_parent_map
                    .insert("92".into(), entry("11", "5", true));
                store
                    .topic_parent_map
                    .insert("93".into(), entry("11", "6", true));

                store.clear_parents(ClanId(10), &[ChannelId(5)], cx);
                assert!(!store.topic_parent_map.contains_key("91"));
                assert!(store.topic_parent_map.contains_key("92"));

                store.clear_clan(ClanId(11), cx);
                assert!(store.topic_parent_map.is_empty());
            });
        });
    }

    fn listed(id: &str, channel_id: &str, unread_count: u32) -> TopicDiscussion {
        TopicDiscussion {
            id: id.into(),
            message_id: String::new(),
            clan_id: String::new(),
            channel_id: channel_id.into(),
            creator_id: String::new(),
            last_sender_id: String::new(),
            content: String::new(),
            last_message_content: String::new(),
            last_message_attachments: Vec::new(),
            last_message_timestamp: 0,
            unread_count,
        }
    }

    #[gpui::test]
    fn listed_topics_dot_their_channel_but_never_its_label(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                let listing = store.begin_topic_listing();
                store.end_topic_listing(
                    ClanId(10),
                    &[
                        listed("91", "5", 2),
                        listed("92", "5", 1),
                        listed("93", "6", 0),
                    ],
                    listing,
                    cx,
                );
                assert!(store.topic_has_unread("91"));
                assert!(store.topic_has_unread("92"));
                assert!(!store.topic_has_unread("93"));
                assert!(store.channel_has_unread_topic(ClanId(10), ChannelId(5)));
                assert!(!store.channel_has_unread_topic(ClanId(10), ChannelId(6)));
                assert!(!store.channel_has_unread_topic(ClanId(11), ChannelId(5)));
                assert!(store.any_topic_unread(ClanId(10)));
                assert!(store.unread_parent_ids(ClanId(10)).is_empty());
                assert!(!store.clan_has_unread_topic(ClanId(10)));
                assert_eq!(store.pending_checks, 0);

                let listing = store.begin_topic_listing();
                store.end_topic_listing(ClanId(10), &[listed("91", "5", 0)], listing, cx);
                assert!(!store.topic_has_unread("91"));
                assert!(store.topic_has_unread("92"));

                store.clear_topic("92", cx);
                assert!(!store.channel_has_unread_topic(ClanId(10), ChannelId(5)));
                assert!(!store.any_topic_unread(ClanId(10)));
            });
        });
    }

    #[gpui::test]
    fn a_topic_read_while_the_list_loads_stays_read(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                let listing = store.begin_topic_listing();
                store.clear_topic("91", cx);
                store.end_topic_listing(
                    ClanId(10),
                    &[listed("91", "5", 1), listed("92", "5", 1)],
                    listing,
                    cx,
                );
                assert!(!store.topic_has_unread("91"));
                assert!(store.topic_has_unread("92"));
                assert!(store.cleared_during_checks.is_empty());
            });
        });
    }

    #[gpui::test]
    fn a_listing_from_before_a_reset_is_dropped(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                let listing = store.begin_topic_listing();
                store.reset(cx);
                store.end_topic_listing(ClanId(10), &[listed("91", "5", 1)], listing, cx);
                assert!(!store.topic_has_unread("91"));
                assert_eq!(store.pending_checks, 0);
            });
        });
    }

    #[gpui::test]
    fn marking_read_clears_listed_topics(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                let listing = store.begin_topic_listing();
                store.end_topic_listing(
                    ClanId(10),
                    &[listed("91", "5", 1), listed("93", "6", 1)],
                    listing,
                    cx,
                );
                let listing = store.begin_topic_listing();
                store.end_topic_listing(ClanId(11), &[listed("94", "7", 1)], listing, cx);

                assert_eq!(store.handle_mark_as_read(5, cx), vec!["10".to_string()]);
                assert!(!store.topic_has_unread("91"));
                assert!(store.topic_has_unread("93"));

                store.clear_parents(ClanId(10), &[ChannelId(6)], cx);
                assert!(!store.topic_has_unread("93"));
                assert!(store.topic_has_unread("94"));

                store.clear_clan(ClanId(11), cx);
                assert!(!store.any_topic_unread(ClanId(11)));
            });
        });
    }
}
