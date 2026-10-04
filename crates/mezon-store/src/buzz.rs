use std::collections::{HashMap, HashSet, VecDeque};

use gpui::{App, AppContext, Context, Entity, Global};
use mezon_client::RealtimeEvent;
use mezon_proto::api::ChannelMessage;
use mezon_proto::realtime::MarkAsRead;

use crate::channel::ChannelList;
use crate::ids::{ChannelId, ClanId, MessageId};
use crate::message::MessageCode;
use crate::messages::{MessagesStore, snowflake_seq, viewer_user_id};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MAX_PROCESSED_BUZZES: usize = 200;

#[derive(Default)]
struct BuzzMark {
    clan_id: ClanId,
    in_channel: Option<MessageId>,
    topics: HashMap<ChannelId, MessageId>,
}

impl BuzzMark {
    fn is_empty(&self) -> bool {
        self.in_channel.is_none() && self.topics.is_empty()
    }
}

#[derive(Default)]
pub struct BuzzStore {
    marks: HashMap<ChannelId, BuzzMark>,
    processed: HashSet<(ChannelId, MessageId)>,
    processed_order: VecDeque<(ChannelId, MessageId)>,
}

struct GlobalBuzzStore(Entity<BuzzStore>);
impl Global for GlobalBuzzStore {}

impl BuzzStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(Self::new);
        cx.set_global(GlobalBuzzStore(entity.clone()));
        entity
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ChannelMessage,
                RealtimeKind::MarkAsRead,
                RealtimeKind::LastSeenUpdated,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
        });
        Self::default()
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalBuzzStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalBuzzStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.marks.clear();
        self.processed.clear();
        self.processed_order.clear();
        cx.notify();
    }

    pub fn has_buzz(&self, channel_id: ChannelId) -> bool {
        self.marks.contains_key(&channel_id)
    }

    pub fn clear_opened(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.marks.remove(&channel_id).is_some() {
            cx.notify();
        }
    }

    pub fn clear_seen(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.unmark_channel(channel_id) {
            cx.notify();
        }
    }

    pub fn clear_topic_seen(&mut self, topic_id: ChannelId, cx: &mut Context<Self>) {
        if self.unmark_topic(topic_id) {
            cx.notify();
        }
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::ChannelMessage(m) if self.receive_buzz(m, cx) => {
                MessagesStore::global(cx).update(cx, |store, cx| store.play_buzz_sound(cx));
            }
            RealtimeEvent::MarkAsRead(read) if self.unmark_read(read, cx) => cx.notify(),
            RealtimeEvent::LastSeenUpdated(seen)
                if self.unmark_seen(ChannelId(seen.channel_id), MessageId(seen.message_id)) =>
            {
                cx.notify()
            }
            _ => {}
        }
    }

    fn receive_buzz(&mut self, m: &ChannelMessage, cx: &mut Context<Self>) -> bool {
        if MessageCode::from_raw(m.code) != MessageCode::MessageBuzz {
            return false;
        }
        if viewer_user_id(cx).is_some_and(|uid| uid.get() == m.sender_id) {
            return false;
        }
        let (channel_id, topic_id) = buzz_target(m);
        let message_id = MessageId(m.message_id);
        if !self.note_processed(topic_id.unwrap_or(channel_id), message_id) {
            return false;
        }
        if !is_buzz_on_screen(channel_id, topic_id, cx)
            && self.mark(ClanId(m.clan_id), channel_id, topic_id, message_id)
        {
            cx.notify();
        }
        true
    }

    fn note_processed(&mut self, storage_id: ChannelId, message_id: MessageId) -> bool {
        if message_id.is_zero() {
            return true;
        }
        let key = (storage_id, message_id);
        if !self.processed.insert(key) {
            return false;
        }
        self.processed_order.push_back(key);
        while self.processed_order.len() > MAX_PROCESSED_BUZZES {
            if let Some(evicted) = self.processed_order.pop_front() {
                self.processed.remove(&evicted);
            }
        }
        true
    }

    fn mark(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        topic_id: Option<ChannelId>,
        message_id: MessageId,
    ) -> bool {
        let mark = self.marks.entry(channel_id).or_default();
        mark.clan_id = clan_id;
        let previous = match topic_id {
            Some(topic_id) => mark.topics.get(&topic_id).copied(),
            None => mark.in_channel,
        };
        let latest = previous
            .filter(|previous| snowflake_seq(*previous) > snowflake_seq(message_id))
            .unwrap_or(message_id);
        match topic_id {
            Some(topic_id) => {
                mark.topics.insert(topic_id, latest);
            }
            None => mark.in_channel = Some(latest),
        }
        previous.is_none()
    }

    fn unmark_channel(&mut self, channel_id: ChannelId) -> bool {
        let Some(mark) = self.marks.get_mut(&channel_id) else {
            return false;
        };
        if mark.in_channel.take().is_none() {
            return false;
        }
        if mark.is_empty() {
            self.marks.remove(&channel_id);
        }
        true
    }

    fn unmark_topic(&mut self, topic_id: ChannelId) -> bool {
        let mut changed = false;
        self.marks.retain(|_, mark| {
            changed |= mark.topics.remove(&topic_id).is_some();
            !mark.is_empty()
        });
        changed
    }

    fn unmark_seen(&mut self, channel_id: ChannelId, seen_up_to: MessageId) -> bool {
        let seen = |buzz: MessageId| snowflake_seq(seen_up_to) >= snowflake_seq(buzz);
        let mut changed = false;
        self.marks.retain(|id, mark| {
            if *id == channel_id && mark.in_channel.is_some_and(seen) {
                mark.in_channel = None;
                changed = true;
            }
            if mark.topics.get(&channel_id).copied().is_some_and(seen) {
                mark.topics.remove(&channel_id);
                changed = true;
            }
            !mark.is_empty()
        });
        changed
    }

    fn unmark_read(&mut self, read: &MarkAsRead, cx: &App) -> bool {
        let clan_id = ClanId(read.clan_id);
        let read_channel = ChannelId(read.channel_id);
        let read_category = read.category_id.to_string();
        let channels = ChannelList::global(cx).read(cx);
        let mut changed = false;
        self.marks.retain(|id, mark| {
            let read_row = if !read_channel.is_zero() {
                *id == read_channel
                    || channels
                        .channel(mark.clan_id, *id)
                        .is_some_and(|row| row.parent_id == Some(read_channel))
            } else if read.category_id != 0 {
                mark.clan_id == clan_id
                    && category_of(channels, clan_id, *id) == Some(read_category.as_str())
            } else {
                !clan_id.is_zero() && mark.clan_id == clan_id
            };
            if !read_channel.is_zero() {
                changed |= mark.topics.remove(&read_channel).is_some();
            }
            changed |= read_row;
            !read_row && !mark.is_empty()
        });
        changed
    }
}

fn category_of(channels: &ChannelList, clan_id: ClanId, channel_id: ChannelId) -> Option<&str> {
    let row = channels.channel(clan_id, channel_id)?;
    match row.parent_id {
        Some(parent_id) => channels.channel(clan_id, parent_id)?.category_id.as_deref(),
        None => row.category_id.as_deref(),
    }
}

fn buzz_target(m: &ChannelMessage) -> (ChannelId, Option<ChannelId>) {
    let topic_id = (m.topic_id != 0).then_some(ChannelId(m.topic_id));
    (ChannelId(m.channel_id), topic_id)
}

fn is_buzz_on_screen(channel_id: ChannelId, topic_id: Option<ChannelId>, cx: &App) -> bool {
    if cx.active_window().is_none() {
        return false;
    }
    let messages = MessagesStore::global(cx).read(cx);
    match topic_id {
        Some(topic_id) => messages.active_topic_id() == Some(topic_id),
        None => messages.active_channel_id() == Some(channel_id),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mezon_client::AppApi;
    use mezon_proto::realtime::LastSeenMessageEvent;

    use super::*;
    use crate::channel::{CHANNEL_ACTIVE_JOINED, Category, Channel, ChannelType};
    use crate::ids::UserId;

    const VIEWER: i64 = 77;
    const OTHER: i64 = 88;
    const STREAM_MODE_CHANNEL: i32 = 2;
    const STREAM_MODE_GROUP: i32 = 3;
    const STREAM_MODE_DM: i32 = 4;
    const STREAM_MODE_THREAD: i32 = 6;
    const GROUP_CHANNEL_TYPE: i32 = 2;
    const DM_CHANNEL_TYPE: i32 = 3;
    const CLAN: ClanId = ClanId(1);
    const OTHER_CLAN: ClanId = ClanId(2);
    const DM_SPACE: ClanId = ClanId(0);
    const GENERAL: i64 = 10;
    const GENERAL_THREAD: i64 = 11;
    const DM: i64 = 12;
    const GROUP: i64 = 13;
    const OFF_TOPIC: i64 = 30;
    const OFF_TOPIC_THREAD: i64 = 31;
    const ELSEWHERE: i64 = 40;
    const TOPIC: i64 = 99;
    const TEXT_CATEGORY: i64 = 5;
    const OTHER_CATEGORY: i64 = 6;

    fn channel(id: i64) -> ChannelId {
        ChannelId(id)
    }

    fn message(seq: i64) -> MessageId {
        MessageId(seq << 22)
    }

    fn row(id: i64, category: i64, parent_id: Option<i64>) -> Channel {
        Channel {
            id: channel(id),
            name: id.to_string(),
            channel_type: ChannelType::Text,
            private: false,
            clan_id: CLAN,
            clan_name: String::new(),
            category_name: String::new(),
            category_id: parent_id.is_none().then(|| category.to_string()),
            member_count: 0,
            badge_count: 0,
            muted: false,
            parent_id: parent_id.map(channel),
            last_seen_message_id: MessageId(0),
            last_seen_timestamp: 0,
            last_sent_message_id: MessageId(0),
            last_sent_timestamp: 0,
            voice_members: Vec::new(),
            is_favorite: false,
            creator_id: UserId(0),
            active: CHANNEL_ACTIVE_JOINED,
            avatar_url: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
        }
    }

    fn category(id: i64, channels: Vec<Channel>) -> Category {
        Category {
            id: id.to_string(),
            clan_id: CLAN,
            name: id.to_string(),
            order: 0,
            channels,
        }
    }

    fn init_stores(cx: &mut App) -> (Entity<BuzzStore>, Entity<MessagesStore>, Arc<AppApi>) {
        let api = Arc::new(AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| {
            crate::AuthState::Authenticated(mezon_client::Session {
                user_id: VIEWER.to_string(),
                ..Default::default()
            })
        });
        crate::badge::BadgeService::init(auth_state, cx);
        crate::clan::ClanList::init(api.clone(), cx);
        crate::direct::DirectMessageStore::init(api.clone(), cx);
        ChannelList::init(api.clone(), cx).update(cx, |channels, _| {
            channels.seed_clan_channels_for_test(
                CLAN,
                vec![
                    category(
                        TEXT_CATEGORY,
                        vec![
                            row(GENERAL, TEXT_CATEGORY, None),
                            row(GENERAL_THREAD, TEXT_CATEGORY, Some(GENERAL)),
                        ],
                    ),
                    category(
                        OTHER_CATEGORY,
                        vec![
                            row(OFF_TOPIC, OTHER_CATEGORY, None),
                            row(OFF_TOPIC_THREAD, OTHER_CATEGORY, Some(OFF_TOPIC)),
                        ],
                    ),
                ],
            );
        });
        let messages = MessagesStore::init(api.clone(), cx);
        (BuzzStore::init(cx), messages, api)
    }

    struct Incoming {
        clan_id: ClanId,
        channel_id: i64,
        topic_id: i64,
        mode: i32,
        code: i32,
        sender_id: i64,
        message_id: MessageId,
    }

    impl Incoming {
        fn buzz(clan_id: ClanId, channel_id: i64, mode: i32, seq: i64) -> Self {
            Self {
                clan_id,
                channel_id,
                topic_id: 0,
                mode,
                code: mezon_client::transport::MESSAGE_BUZZ_CODE,
                sender_id: OTHER,
                message_id: message(seq),
            }
        }

        fn in_topic(self) -> Self {
            Self {
                topic_id: TOPIC,
                ..self
            }
        }

        fn message(self) -> ChannelMessage {
            ChannelMessage {
                clan_id: self.clan_id.get(),
                channel_id: self.channel_id,
                topic_id: self.topic_id,
                mode: self.mode,
                code: self.code,
                sender_id: self.sender_id,
                message_id: self.message_id.get(),
                ..Default::default()
            }
        }
    }

    fn deliver(store: &Entity<BuzzStore>, incoming: Incoming, cx: &mut App) -> bool {
        let message = incoming.message();
        store.update(cx, |store, cx| store.receive_buzz(&message, cx))
    }

    fn marked(store: &Entity<BuzzStore>, cx: &gpui::TestAppContext) -> Vec<i64> {
        cx.read(|cx| {
            let mut rows: Vec<i64> = store.read(cx).marks.keys().map(|id| id.get()).collect();
            rows.sort_unstable();
            rows
        })
    }

    fn mark_all(store: &Entity<BuzzStore>, cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store.mark(CLAN, channel(GENERAL), None, message(1));
                store.mark(CLAN, channel(GENERAL_THREAD), None, message(1));
                store.mark(CLAN, channel(OFF_TOPIC), None, message(1));
                store.mark(CLAN, channel(OFF_TOPIC_THREAD), None, message(1));
                store.mark(OTHER_CLAN, channel(ELSEWHERE), None, message(1));
                store.mark(DM_SPACE, channel(DM), None, message(1));
            });
        });
    }

    fn publish(api: &AppApi, event: RealtimeEvent, cx: &mut gpui::TestAppContext) {
        api.publish_event(event);
        cx.run_until_parked();
    }

    fn mark_as_read(clan_id: ClanId, channel_id: i64, category_id: i64) -> RealtimeEvent {
        RealtimeEvent::MarkAsRead(MarkAsRead {
            clan_id: clan_id.get(),
            channel_id,
            category_id,
        })
    }

    fn last_seen(clan_id: ClanId, channel_id: i64, seq: i64) -> RealtimeEvent {
        RealtimeEvent::LastSeenUpdated(LastSeenMessageEvent {
            clan_id: clan_id.get(),
            channel_id,
            message_id: message(seq).get(),
            ..Default::default()
        })
    }

    #[gpui::test]
    fn a_buzz_from_someone_else_marks_its_row_in_every_stream_mode(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (store, _, _) = init_stores(cx);
            assert!(deliver(
                &store,
                Incoming::buzz(CLAN, GENERAL, STREAM_MODE_CHANNEL, 1),
                cx
            ));
            assert!(deliver(
                &store,
                Incoming::buzz(CLAN, GENERAL_THREAD, STREAM_MODE_THREAD, 2),
                cx
            ));
            assert!(deliver(
                &store,
                Incoming::buzz(DM_SPACE, DM, STREAM_MODE_DM, 3),
                cx
            ));
            assert!(deliver(
                &store,
                Incoming::buzz(DM_SPACE, GROUP, STREAM_MODE_GROUP, 4),
                cx
            ));
            assert!(deliver(
                &store,
                Incoming::buzz(CLAN, OFF_TOPIC, STREAM_MODE_CHANNEL, 5).in_topic(),
                cx
            ));
            assert!(!deliver(
                &store,
                Incoming::buzz(CLAN, GENERAL, STREAM_MODE_CHANNEL, 1),
                cx
            ));
            let store = store.read(cx);
            for row in [GENERAL, GENERAL_THREAD, DM, GROUP, OFF_TOPIC] {
                assert!(
                    store.has_buzz(channel(row)),
                    "row {row} should carry the buzz"
                );
            }
            assert!(!store.has_buzz(channel(TOPIC)));
        });
    }

    #[gpui::test]
    fn own_buzzes_and_plain_messages_mark_nothing(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (store, _, _) = init_stores(cx);
            assert!(!deliver(
                &store,
                Incoming {
                    sender_id: VIEWER,
                    ..Incoming::buzz(DM_SPACE, DM, STREAM_MODE_DM, 1)
                },
                cx,
            ));
            assert!(!deliver(
                &store,
                Incoming {
                    code: 0,
                    ..Incoming::buzz(DM_SPACE, GROUP, STREAM_MODE_GROUP, 2)
                },
                cx,
            ));
            assert!(!deliver(
                &store,
                Incoming {
                    code: 0,
                    ..Incoming::buzz(CLAN, GENERAL, STREAM_MODE_CHANNEL, 3)
                },
                cx,
            ));
            assert!(store.read(cx).marks.is_empty());
        });
    }

    #[gpui::test]
    fn opening_a_group_clears_its_buzz_and_a_seen_tail_clears_a_later_one(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let (store, messages, _) = init_stores(cx);
            deliver(
                &store,
                Incoming::buzz(DM_SPACE, GROUP, STREAM_MODE_GROUP, 1),
                cx,
            );
            deliver(&store, Incoming::buzz(DM_SPACE, DM, STREAM_MODE_DM, 2), cx);
            messages.update(cx, |messages, cx| {
                messages.open_direct(channel(GROUP), GROUP_CHANNEL_TYPE, cx)
            });
            assert!(!store.read(cx).has_buzz(channel(GROUP)));
            assert!(store.read(cx).has_buzz(channel(DM)));

            deliver(
                &store,
                Incoming::buzz(DM_SPACE, GROUP, STREAM_MODE_GROUP, 3),
                cx,
            );
            assert!(store.read(cx).has_buzz(channel(GROUP)));
            messages.update(cx, |messages, cx| {
                messages.note_viewport_seen(message(3), 1, false, cx)
            });
            assert!(store.read(cx).has_buzz(channel(GROUP)));
            messages.update(cx, |messages, cx| {
                messages.note_viewport_seen(message(3), 1, true, cx)
            });
            assert!(!store.read(cx).has_buzz(channel(GROUP)));
            assert!(store.read(cx).has_buzz(channel(DM)));
        });
    }

    #[gpui::test]
    fn seeing_a_topic_clears_the_buzz_it_left_on_its_channel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (store, messages, _) = init_stores(cx);
            messages.update(cx, |messages, cx| {
                messages.open_direct(channel(DM), DM_CHANNEL_TYPE, cx)
            });
            deliver(
                &store,
                Incoming::buzz(CLAN, OFF_TOPIC, STREAM_MODE_CHANNEL, 1).in_topic(),
                cx,
            );
            assert!(store.read(cx).has_buzz(channel(OFF_TOPIC)));
            messages.update(cx, |messages, cx| {
                messages.note_topic_viewport_seen(channel(TOPIC), message(1), 1, true, cx)
            });
            assert!(!store.read(cx).has_buzz(channel(OFF_TOPIC)));
        });
    }

    #[gpui::test]
    fn marking_a_channel_read_anywhere_clears_it_and_its_threads(cx: &mut gpui::TestAppContext) {
        let (store, _, api) = cx.update(init_stores);
        cx.run_until_parked();
        mark_all(&store, cx);
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store.mark(CLAN, channel(OFF_TOPIC), Some(channel(TOPIC)), message(1));
            });
        });
        publish(&api, mark_as_read(CLAN, GENERAL, TEXT_CATEGORY), cx);
        assert_eq!(
            marked(&store, cx),
            vec![DM, OFF_TOPIC, OFF_TOPIC_THREAD, ELSEWHERE]
        );
        publish(&api, mark_as_read(CLAN, TOPIC, 0), cx);
        cx.read(|cx| {
            assert!(store.read(cx).marks[&channel(OFF_TOPIC)].topics.is_empty());
        });
        publish(&api, mark_as_read(DM_SPACE, DM, 0), cx);
        assert_eq!(
            marked(&store, cx),
            vec![OFF_TOPIC, OFF_TOPIC_THREAD, ELSEWHERE]
        );
    }

    #[gpui::test]
    fn marking_a_category_or_clan_read_clears_only_its_rows(cx: &mut gpui::TestAppContext) {
        let (store, _, api) = cx.update(init_stores);
        cx.run_until_parked();
        mark_all(&store, cx);
        publish(&api, mark_as_read(CLAN, 0, OTHER_CATEGORY), cx);
        assert_eq!(
            marked(&store, cx),
            vec![GENERAL, GENERAL_THREAD, DM, ELSEWHERE]
        );
        publish(&api, mark_as_read(CLAN, 0, 0), cx);
        assert_eq!(marked(&store, cx), vec![DM, ELSEWHERE]);
    }

    #[gpui::test]
    fn a_read_on_another_session_clears_only_the_buzzes_it_covers(cx: &mut gpui::TestAppContext) {
        let (store, _, api) = cx.update(init_stores);
        cx.run_until_parked();
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store.mark(CLAN, channel(GENERAL), None, message(20));
                store.mark(CLAN, channel(OFF_TOPIC), Some(channel(TOPIC)), message(30));
                store.mark(DM_SPACE, channel(DM), None, message(40));
            });
        });
        publish(&api, last_seen(CLAN, GENERAL, 19), cx);
        publish(&api, last_seen(CLAN, TOPIC, 29), cx);
        publish(&api, last_seen(DM_SPACE, DM, 39), cx);
        assert_eq!(marked(&store, cx), vec![GENERAL, DM, OFF_TOPIC]);
        publish(&api, last_seen(CLAN, GENERAL, 20), cx);
        publish(&api, last_seen(CLAN, TOPIC, 31), cx);
        publish(&api, last_seen(DM_SPACE, DM, 40), cx);
        assert!(marked(&store, cx).is_empty());
    }

    #[test]
    fn channel_buzz_is_cleared_once_the_channel_tail_is_seen() {
        let mut store = BuzzStore::default();
        store.mark(CLAN, channel(1), None, message(1));
        assert!(store.has_buzz(channel(1)));
        assert!(store.unmark_channel(channel(1)));
        assert!(!store.has_buzz(channel(1)));
    }

    #[test]
    fn topic_buzz_marks_its_channel_until_the_topic_is_seen() {
        let mut store = BuzzStore::default();
        store.mark(CLAN, channel(1), Some(channel(TOPIC)), message(1));
        assert!(store.has_buzz(channel(1)));
        assert!(!store.unmark_channel(channel(1)));
        assert!(store.has_buzz(channel(1)));
        assert!(store.unmark_topic(channel(TOPIC)));
        assert!(!store.has_buzz(channel(1)));
    }

    #[test]
    fn seeing_the_channel_keeps_a_pending_topic_buzz() {
        let mut store = BuzzStore::default();
        store.mark(CLAN, channel(1), None, message(1));
        store.mark(CLAN, channel(1), Some(channel(TOPIC)), message(2));
        assert!(store.unmark_channel(channel(1)));
        assert!(store.has_buzz(channel(1)));
        assert!(!store.unmark_topic(channel(8)));
        assert!(store.unmark_topic(channel(TOPIC)));
        assert!(!store.has_buzz(channel(1)));
    }

    #[test]
    fn a_repeated_buzz_notifies_once_and_keeps_the_newest_id() {
        let mut store = BuzzStore::default();
        assert!(store.mark(CLAN, channel(1), None, message(10)));
        assert!(!store.mark(CLAN, channel(1), None, message(20)));
        assert!(!store.mark(CLAN, channel(1), None, message(5)));
        assert!(!store.unmark_seen(channel(1), message(15)));
        assert!(store.unmark_seen(channel(1), message(20)));
        assert!(store.mark(CLAN, channel(1), Some(channel(TOPIC)), message(1)));
        assert!(!store.mark(CLAN, channel(1), Some(channel(TOPIC)), message(1)));
    }

    #[test]
    fn a_redelivered_buzz_is_processed_once() {
        let mut store = BuzzStore::default();
        assert!(store.note_processed(channel(1), MessageId(5)));
        assert!(!store.note_processed(channel(1), MessageId(5)));
        assert!(store.note_processed(channel(2), MessageId(5)));
    }

    #[test]
    fn processed_buzzes_stay_bounded() {
        let mut store = BuzzStore::default();
        for id in 1..=(MAX_PROCESSED_BUZZES as i64 + 10) {
            store.note_processed(channel(1), MessageId(id));
        }
        assert_eq!(store.processed.len(), MAX_PROCESSED_BUZZES);
        assert!(store.note_processed(channel(1), MessageId(1)));
    }

    #[test]
    fn buzz_target_splits_topic_from_channel() {
        let in_channel = ChannelMessage {
            channel_id: 7,
            ..Default::default()
        };
        assert_eq!(buzz_target(&in_channel), (channel(7), None));
        let in_topic = ChannelMessage {
            channel_id: 7,
            topic_id: 9,
            ..Default::default()
        };
        assert_eq!(buzz_target(&in_topic), (channel(7), Some(channel(9))));
    }
}
