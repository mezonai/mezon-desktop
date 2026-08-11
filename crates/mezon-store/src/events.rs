use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::api;

use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::{ChannelId, ChannelList, ClanId, UserId};

const CACHE_TTL: Duration = Duration::from_secs(20 * 60);
const FETCH_RETRY_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventAction {
    Created,
    Updated,
    Deleted,
    Interested,
    Uninterested,
    Unknown,
}

impl From<i32> for EventAction {
    fn from(value: i32) -> Self {
        match value {
            1 => Self::Created,
            2 => Self::Updated,
            3 => Self::Deleted,
            4 => Self::Interested,
            5 => Self::Uninterested,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
enum EventStatus {
    Completed = 3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClanEventItem {
    pub id: i64,
    pub title: String,
    pub logo: String,
    pub description: String,
    pub creator_id: UserId,
    pub channel_voice_id: Option<ChannelId>,
    pub address: String,
    pub start_time_seconds: u32,
    pub end_time_seconds: u32,
    pub user_ids: Vec<UserId>,
    pub channel_id: Option<ChannelId>,
    pub event_status: i32,
    pub is_private: bool,
    pub external_link: String,
}

impl ClanEventItem {
    fn from_api(event: api::EventManagement) -> Self {
        Self {
            id: event.id,
            title: event.title,
            logo: event.logo,
            description: event.description,
            creator_id: UserId(event.creator_id),
            channel_voice_id: nonzero_channel(event.channel_voice_id),
            address: event.address,
            start_time_seconds: event.start_time_seconds,
            end_time_seconds: event.end_time_seconds,
            user_ids: event.user_ids.into_iter().map(UserId).collect(),
            channel_id: nonzero_channel(event.channel_id),
            event_status: event.event_status,
            is_private: event.is_private,
            external_link: event
                .meet_room
                .map(|room| room.external_link)
                .unwrap_or_default(),
        }
    }

    fn from_realtime(event: &api::CreateEventRequest) -> Self {
        Self {
            id: event.event_id,
            title: event.title.clone(),
            logo: event.logo.clone(),
            description: event.description.clone(),
            creator_id: UserId(event.creator_id),
            channel_voice_id: nonzero_channel(event.channel_voice_id),
            address: event.address.clone(),
            start_time_seconds: event.start_time_seconds,
            end_time_seconds: event.end_time_seconds,
            user_ids: vec![UserId(event.creator_id)],
            channel_id: nonzero_channel(event.channel_id),
            event_status: event.event_status,
            is_private: event.is_private,
            external_link: event
                .meet_room
                .as_ref()
                .map(|room| room.external_link.clone())
                .unwrap_or_default(),
        }
    }

    fn apply_realtime(&mut self, event: &api::CreateEventRequest) {
        self.title.clone_from(&event.title);
        self.logo.clone_from(&event.logo);
        self.description.clone_from(&event.description);
        self.creator_id = UserId(event.creator_id);
        self.channel_voice_id = nonzero_channel(event.channel_voice_id);
        self.address.clone_from(&event.address);
        self.start_time_seconds = event.start_time_seconds;
        self.end_time_seconds = event.end_time_seconds;
        self.channel_id = nonzero_channel(event.channel_id);
        if event.event_status != 0 {
            self.event_status = event.event_status;
        }
        self.is_private = event.is_private;
        if let Some(room) = &event.meet_room {
            self.external_link.clone_from(&room.external_link);
        }
    }
}

fn nonzero_channel(id: i64) -> Option<ChannelId> {
    (id != 0).then_some(ChannelId(id))
}

#[derive(Clone, Debug)]
pub enum EventsEvent {
    Changed { clan_id: ClanId },
}

#[derive(Clone, Debug, Default)]
pub struct CreateEventDraft {
    pub title: String,
    pub logo: String,
    pub description: String,
    pub clan_id: ClanId,
    pub channel_voice_id: Option<ChannelId>,
    pub address: String,
    pub start_time_seconds: u32,
    pub end_time_seconds: u32,
    pub channel_id: Option<ChannelId>,
    pub repeat_type: i32,
    pub is_private: bool,
}

pub struct EventsStore {
    events: HashMap<ClanId, Vec<ClanEventItem>>,
    loaded: HashSet<ClanId>,
    loading: HashSet<ClanId>,
    fetched_at: HashMap<ClanId, Instant>,
    failed_at: HashMap<ClanId, Instant>,
    api: Arc<AppApi>,
    _connection_watch: Task<()>,
    transition_task: Option<Task<()>>,
    transition_generation: u64,
}

struct GlobalEventsStore(Entity<EventsStore>);
impl Global for GlobalEventsStore {}
impl EventEmitter<EventsEvent> for EventsStore {}

impl EventsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalEventsStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let connection_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            events: HashMap::new(),
            loaded: HashSet::new(),
            loading: HashSet::new(),
            fetched_at: HashMap::new(),
            failed_at: HashMap::new(),
            api,
            _connection_watch: connection_watch,
            transition_task: None,
            transition_generation: 0,
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalEventsStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalEventsStore>()
            .map(|store| store.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.events.clear();
        self.loaded.clear();
        self.loading.clear();
        self.fetched_at.clear();
        self.failed_at.clear();
        self.transition_generation = self.transition_generation.wrapping_add(1);
        self.transition_task = None;
        cx.notify();
    }

    pub fn create_event(
        &mut self,
        draft: CreateEventDraft,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let api = self.api.clone();
        let requested_clan_id = draft.clan_id;
        let request = api::CreateEventRequest {
            title: draft.title,
            logo: draft.logo,
            description: draft.description,
            clan_id: draft.clan_id.0,
            channel_voice_id: draft.channel_voice_id.map_or(0, |id| id.0),
            address: draft.address,
            start_time_seconds: draft.start_time_seconds,
            end_time_seconds: draft.end_time_seconds,
            channel_id: draft.channel_id.map_or(0, |id| id.0),
            repeat_type: draft.repeat_type,
            is_private: draft.is_private,
            ..Default::default()
        };
        cx.spawn(async move |this, cx| {
            let event = api.create_event(request).await?;
            this.update(cx, |this, cx| {
                let clan_id = if event.clan_id == 0 {
                    requested_clan_id
                } else {
                    ClanId(event.clan_id)
                };
                let item = ClanEventItem::from_api(event);
                let events = this.events.entry(clan_id).or_default();
                if let Some(existing) = events.iter_mut().find(|existing| existing.id == item.id) {
                    *existing = item;
                } else {
                    events.push(item);
                }
                this.schedule_next_transition(cx);
                cx.emit(EventsEvent::Changed { clan_id });
                cx.notify();
            })?;
            Ok(())
        })
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(
                RealtimeKind::ClanEventCreated,
                &entity,
                |this, event, cx| this.handle_realtime(event, cx),
            );
            dispatch.on_lagged(&entity, |this, cx| this.refresh_loaded(cx));
        });
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status = api.status();
            let mut connected_before = *status.borrow() == ConnectionStatus::Connected;
            loop {
                if status.changed().await.is_err() {
                    break;
                }
                let connected = *status.borrow() == ConnectionStatus::Connected;
                if connected
                    && !connected_before
                    && this.update(cx, |this, cx| this.refresh_loaded(cx)).is_err()
                {
                    break;
                }
                connected_before = connected;
            }
        })
    }

    fn schedule_next_transition(&mut self, cx: &mut Context<Self>) {
        self.transition_generation = self.transition_generation.wrapping_add(1);
        let generation = self.transition_generation;
        self.transition_task = None;

        let now = chrono::Utc::now().timestamp().max(0) as u32;
        let next_transition = self
            .events
            .values()
            .flatten()
            .flat_map(|event| [event.start_time_seconds, event.end_time_seconds])
            .filter(|timestamp| *timestamp > now)
            .min();
        let Some(next_transition) = next_transition else {
            return;
        };

        let delay = Duration::from_secs(next_transition.saturating_sub(now).max(1) as u64);
        self.transition_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                if this.transition_generation != generation {
                    return;
                }
                this.transition_task = None;
                let changed = this.remove_expired();
                Self::emit_changed(changed, cx);
                cx.notify();
                this.schedule_next_transition(cx);
            });
        }));
    }

    fn emit_changed(changed: Vec<ClanId>, cx: &mut Context<Self>) {
        for clan_id in changed.into_iter().collect::<HashSet<_>>() {
            cx.emit(EventsEvent::Changed { clan_id });
        }
    }

    fn remove_expired(&mut self) -> Vec<ClanId> {
        let now = chrono::Utc::now().timestamp().max(0) as u32;
        let mut changed = Vec::new();
        for (clan_id, events) in &mut self.events {
            let old_len = events.len();
            events.retain(|event| {
                event.event_status != EventStatus::Completed as i32
                    && (event.end_time_seconds == 0 || event.end_time_seconds > now)
            });
            if events.len() != old_len {
                changed.push(*clan_id);
            }
        }
        changed
    }

    fn handle_realtime(&mut self, realtime: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::ClanEventCreated(event) = realtime else {
            return;
        };
        let clan_id = ClanId(event.clan_id);
        let events = self.events.entry(clan_id).or_default();
        match EventAction::from(event.action) {
            EventAction::Created => {
                if let Some(existing) = events.iter_mut().find(|item| item.id == event.event_id) {
                    existing.apply_realtime(event);
                } else {
                    events.push(ClanEventItem::from_realtime(event));
                }
            }
            EventAction::Updated => {
                if event.event_status == EventStatus::Completed as i32 {
                    events.retain(|item| item.id != event.event_id);
                } else if let Some(existing) =
                    events.iter_mut().find(|item| item.id == event.event_id)
                {
                    existing.apply_realtime(event);
                } else {
                    events.push(ClanEventItem::from_realtime(event));
                }
            }
            EventAction::Deleted => events.retain(|item| item.id != event.event_id),
            EventAction::Interested => {
                if let Some(existing) = events.iter_mut().find(|item| item.id == event.event_id) {
                    let user = UserId(event.user_id);
                    if !existing.user_ids.contains(&user) {
                        existing.user_ids.push(user);
                    }
                }
            }
            EventAction::Uninterested => {
                if let Some(existing) = events.iter_mut().find(|item| item.id == event.event_id) {
                    existing.user_ids.retain(|user| user.0 != event.user_id);
                }
            }
            EventAction::Unknown => self.fetch(clan_id, cx),
        }
        let mut changed = self.remove_expired();
        changed.push(clan_id);
        Self::emit_changed(changed, cx);
        self.schedule_next_transition(cx);
        cx.notify();
    }

    pub fn events(&self, clan_id: ClanId) -> &[ClanEventItem] {
        self.events.get(&clan_id).map_or(&[], Vec::as_slice)
    }

    pub fn ensure_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self.loading.contains(&clan_id)
            || self
                .fetched_at
                .get(&clan_id)
                .is_some_and(|instant| now.duration_since(*instant) < CACHE_TTL)
            || self
                .failed_at
                .get(&clan_id)
                .is_some_and(|instant| now.duration_since(*instant) < FETCH_RETRY_BACKOFF)
        {
            return;
        }
        self.fetch(clan_id, cx);
    }

    fn refresh_loaded(&mut self, cx: &mut Context<Self>) {
        let clans: Vec<_> = self.loaded.iter().copied().collect();
        for clan_id in clans {
            self.fetch(clan_id, cx);
        }
    }

    fn fetch(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.loading.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_events(clan_id.0).await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&clan_id);
                match result {
                    Ok(items) => {
                        let items = items
                            .into_iter()
                            .map(ClanEventItem::from_api)
                            .filter(|event| event.event_status != EventStatus::Completed as i32)
                            .collect();
                        this.events.insert(clan_id, items);
                        this.loaded.insert(clan_id);
                        this.fetched_at.insert(clan_id, Instant::now());
                        this.failed_at.remove(&clan_id);
                    }
                    Err(error) => {
                        this.failed_at.insert(clan_id, Instant::now());
                        tracing::warn!(%error, %clan_id, "failed to load events");
                    }
                }
                let mut changed = this.remove_expired();
                changed.push(clan_id);
                Self::emit_changed(changed, cx);
                this.schedule_next_transition(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Rows the viewer may see in `clan_id`. `visible_events` and
    /// `visible_event_count` must agree exactly — the sidebar count is rendered
    /// next to the list the modal shows — so both go through this one predicate.
    ///
    /// Keeps parity with React's `useEventManagementQuantity`: private events are
    /// only visible to their creator, and an event pinned to a channel is hidden
    /// unless that channel is in the clan the viewer can see.
    fn visible_in_clan<'a>(
        &'a self,
        clan_id: ClanId,
        current_user: Option<UserId>,
        cx: &'a App,
    ) -> impl Iterator<Item = &'a ClanEventItem> + 'a {
        let channels = ChannelList::global(cx).read(cx);
        let now = chrono::Utc::now().timestamp().max(0) as u32;
        self.events(clan_id).iter().filter(move |event| {
            event.event_status != EventStatus::Completed as i32
                && (event.end_time_seconds == 0 || event.end_time_seconds > now)
                && (!event.is_private || Some(event.creator_id) == current_user)
                && event
                    .channel_id
                    .is_none_or(|channel_id| channels.channel_in_clan(clan_id, channel_id))
        })
    }

    pub fn visible_events(
        &self,
        clan_id: ClanId,
        current_user: Option<UserId>,
        cx: &App,
    ) -> Vec<ClanEventItem> {
        self.visible_in_clan(clan_id, current_user, cx)
            .cloned()
            .collect()
    }

    pub fn visible_event_count(
        &self,
        clan_id: ClanId,
        current_user: Option<UserId>,
        cx: &App,
    ) -> usize {
        self.visible_in_clan(clan_id, current_user, cx).count()
    }
}
