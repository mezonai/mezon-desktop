use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::transport::{ApiCategoryDesc, ApiChannelDesc};
use mezon_client::{ApiChannelApp, AppApi, ConnectionStatus, RealtimeEvent};

use crate::KeyedCache;
use crate::clan::{ClanEvent, ClanList};
use crate::messages::MessagesStore;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

pub const FAVOR_CATE_ID: &str = "favorCate";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelType {
    Text,
    Voice,
    Stream,
    Thread,
    App,
    Forum,
    Announcement,
    Unknown(u32),
}

impl ChannelType {
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            1 => ChannelType::Text,
            5 => ChannelType::Forum,
            6 => ChannelType::Stream,
            7 => ChannelType::Thread,
            8 => ChannelType::App,
            9 => ChannelType::Announcement,
            10 => ChannelType::Voice,
            other => ChannelType::Unknown(other),
        }
    }

    pub fn as_raw(&self) -> u32 {
        match self {
            ChannelType::Text => 1,
            ChannelType::Forum => 5,
            ChannelType::Stream => 6,
            ChannelType::Thread => 7,
            ChannelType::App => 8,
            ChannelType::Announcement => 9,
            ChannelType::Voice => 10,
            ChannelType::Unknown(raw) => *raw,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceMember {
    pub user_id: UserId,
    pub display_name: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppChannel {
    pub app_id: String,
    pub app_name: String,
    pub app_logo: Option<String>,
    pub app_url: String,
    pub channel_id: ChannelId,
}

impl From<ApiChannelApp> for AppChannel {
    fn from(a: ApiChannelApp) -> Self {
        Self {
            app_id: a.app_id,
            app_name: a.app_name,
            app_logo: a.app_logo,
            app_url: a.app_url,
            channel_id: ChannelId(a.channel_id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    pub channel_type: ChannelType,
    pub private: bool,
    pub clan_id: ClanId,
    pub clan_name: String,
    pub category_name: String,
    pub category_id: Option<String>,
    pub member_count: u32,
    pub badge_count: u32,
    pub muted: bool,
    pub parent_id: Option<ChannelId>,
    pub last_seen_message_id: MessageId,
    pub last_seen_timestamp: i64,
    pub last_sent_message_id: MessageId,
    pub last_sent_timestamp: i64,
    pub voice_members: Vec<VoiceMember>,
    pub is_favorite: bool,
    pub creator_id: UserId,
}

impl Channel {
    pub fn is_unread(&self) -> bool {
        self.badge_count > 0 || self.last_seen_timestamp < self.last_sent_timestamp
    }
}

#[derive(Debug, Clone)]
pub struct Category {
    pub id: String,
    pub clan_id: ClanId,
    pub name: String,
    pub order: i32,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone)]
pub enum ChannelEvent {
    ActiveChannelChanged(Option<ChannelId>),
    Unread(ChannelId),
}

fn collapse_state_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("mezon")
        .join("collapse_state.json")
}

fn previous_channels_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("mezon")
        .join("previous_channels.json")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TopicParentBadge {
    clan_id: ClanId,
    parent_id: ChannelId,
    count: u32,
}

pub struct ChannelList {
    cache: KeyedCache<ClanId, Vec<Category>>,
    app_channels_cache: HashMap<ClanId, Vec<AppChannel>>,
    topic_parent_badges: HashMap<ChannelId, TopicParentBadge>,
    pending_channel_badges: HashMap<ChannelId, u32>,
    user_channels: HashMap<ChannelId, Channel>,
    user_channels_loading: bool,
    loading: HashSet<ClanId>,
    active_clan_id: Option<ClanId>,
    pub active_channel_id: Option<ChannelId>,
    remembered_channels: HashMap<ClanId, ChannelId>,
    previous_channels: HashMap<ClanId, Vec<ChannelId>>,
    api: Arc<AppApi>,
    collapsed: HashSet<(String, String)>,
    show_empty_categories: HashSet<ClanId>,
    channel_index: RefCell<ChannelLocationCache>,
    reset_generation: u64,
    _clan_sub: Subscription,
    _conn_watch: Task<()>,
}

#[derive(Default)]
struct ChannelLocationCache {
    by_clan: HashMap<ClanId, HashMap<ChannelId, (usize, usize)>>,
    clan_of: HashMap<ChannelId, ClanId>,
    clan_of_built: bool,
}

impl ChannelLocationCache {
    fn invalidate(&mut self, clan_id: ClanId) {
        self.by_clan.remove(&clan_id);
        self.clan_of_built = false;
    }

    fn invalidate_all(&mut self) {
        self.by_clan.clear();
        self.clan_of_built = false;
    }

    fn location(
        &mut self,
        clan_id: ClanId,
        categories: &[Category],
        channel_id: ChannelId,
    ) -> Option<(usize, usize)> {
        self.by_clan
            .entry(clan_id)
            .or_insert_with(|| build_clan_channel_index(categories))
            .get(&channel_id)
            .copied()
    }

    fn clan_for(
        &mut self,
        cache: &KeyedCache<ClanId, Vec<Category>>,
        channel_id: ChannelId,
    ) -> Option<ClanId> {
        if !self.clan_of_built {
            self.clan_of.clear();
            for (clan_id, categories) in cache.iter() {
                for category in categories {
                    for channel in &category.channels {
                        self.clan_of.entry(channel.id).or_insert(*clan_id);
                    }
                }
            }
            self.clan_of_built = true;
        }
        self.clan_of.get(&channel_id).copied()
    }
}

fn build_clan_channel_index(categories: &[Category]) -> HashMap<ChannelId, (usize, usize)> {
    let mut index = HashMap::new();
    for (cat_idx, category) in categories.iter().enumerate() {
        for (ch_idx, channel) in category.channels.iter().enumerate() {
            index.entry(channel.id).or_insert((cat_idx, ch_idx));
        }
    }
    index
}

struct GlobalChannelList(Entity<ChannelList>);
impl Global for GlobalChannelList {}

impl EventEmitter<ChannelEvent> for ChannelList {}

impl ChannelList {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalChannelList(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalChannelList>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalChannelList>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.cache.clear();
        self.app_channels_cache.clear();
        self.topic_parent_badges.clear();
        self.pending_channel_badges.clear();
        self.user_channels.clear();
        self.user_channels_loading = false;
        self.loading.clear();
        self.remembered_channels.clear();
        self.previous_channels.clear();
        self.persist_previous_channels(cx);
        self.invalidate_channel_index_all();
        self.active_clan_id = None;
        if self.active_channel_id.take().is_some() {
            cx.emit(ChannelEvent::ActiveChannelChanged(None));
        }
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let clan_sub = cx.subscribe(&ClanList::global(cx), |this, _clan, event, cx| {
            if let ClanEvent::ActiveClanChanged(active) = event {
                match active {
                    Some(clan_id) => {
                        this.active_clan_id = Some(*clan_id);
                        this.load_for_clan(*clan_id, cx);
                        cx.notify();
                    }
                    None => {
                        this.active_clan_id = None;
                        if this.active_channel_id.is_some() {
                            this.active_channel_id = None;
                            cx.emit(ChannelEvent::ActiveChannelChanged(None));
                        }
                        cx.notify();
                    }
                }
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        cx.spawn(async move |this, cx| {
            let (collapsed, previous_channels) = cx
                .background_executor()
                .spawn(async {
                    (
                        load_collapse_state(),
                        load_previous_channels(),
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let mut changed = false;
                if !collapsed.is_empty() {
                    this.collapsed = collapsed;
                    changed = true;
                }
                if !previous_channels.is_empty() {
                    this.previous_channels = previous_channels;
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();

        Self {
            cache: KeyedCache::new(None),
            app_channels_cache: HashMap::new(),
            topic_parent_badges: HashMap::new(),
            pending_channel_badges: HashMap::new(),
            user_channels: HashMap::new(),
            user_channels_loading: false,
            loading: HashSet::new(),
            active_clan_id: None,
            active_channel_id: None,
            remembered_channels: HashMap::new(),
            previous_channels: HashMap::new(),
            api,
            collapsed: HashSet::new(),
            show_empty_categories: HashSet::new(),
            channel_index: RefCell::new(ChannelLocationCache::default()),
            reset_generation: 0,
            _clan_sub: clan_sub,
            _conn_watch: conn_watch,
        }
    }

    fn invalidate_channel_index(&mut self, clan_id: ClanId) {
        self.channel_index.get_mut().invalidate(clan_id);
    }

    fn invalidate_channel_index_all(&mut self) {
        self.channel_index.get_mut().invalidate_all();
    }

    fn channel_location(&self, clan_id: ClanId, channel_id: ChannelId) -> Option<(usize, usize)> {
        let categories = self.cache.get(&clan_id)?;
        self.channel_index
            .borrow_mut()
            .location(clan_id, categories, channel_id)
    }

    fn merge_pending_badges(&mut self, categories: &mut [Category]) {
        merge_pending_badges_into(&mut self.pending_channel_badges, categories);
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ChannelCreated,
                RealtimeKind::ChannelUpdated,
                RealtimeKind::ChannelDeleted,
                RealtimeKind::VoiceJoined,
                RealtimeKind::VoiceLeaved,
                RealtimeKind::UserChannelAdded,
                RealtimeKind::UserChannelRemoved,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, cx| this.resync(cx));
        });
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = false;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected && !was_connected {
                    was_connected = true;
                    if this.update(cx, |this, cx| this.resync(cx)).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn load_for_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.cache.is_fresh(&clan_id, crate::CACHE_TTL) {
            return;
        }
        self.fetch_clan(clan_id, cx);
    }

    pub fn refresh_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.fetch_clan(clan_id, cx);
    }

    fn fetch_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.loading.contains(&clan_id) {
            return;
        }
        self.loading.insert(clan_id);
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = Self::fetch_clan_data(&api, clan_id).await;
            match result {
                Ok((mut categories, app_channels, last_messages)) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.reset_generation != generation {
                            return;
                        }
                        this.loading.remove(&clan_id);
                        this.app_channels_cache.insert(clan_id, app_channels);
                        this.merge_pending_badges(&mut categories);
                        this.cache.insert(clan_id, categories, None);
                        this.invalidate_channel_index(clan_id);
                        if let Some(store) = MessagesStore::try_global(cx) {
                            store.update(cx, |store, cx| {
                                store.set_many_last_messages(last_messages);
                                cx.notify();
                            });
                        }
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to load channels for clan {clan_id}: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.loading.remove(&clan_id);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn fetch_user_channels(&mut self, cx: &mut Context<Self>) {
        if self.user_channels_loading {
            return;
        }
        self.user_channels_loading = true;
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.list_channel_by_user_id().await;
            let _ = this.update(cx, |this, cx| {
                this.user_channels_loading = false;
                if this.reset_generation != generation {
                    return;
                }
                match result {
                    Ok(descs) => {
                        this.user_channels = descs
                            .into_iter()
                            .map(|d| {
                                let channel = channel_from_desc(d, 0, Vec::new(), false);
                                (channel.id, channel)
                            })
                            .collect();
                        cx.notify();
                    }
                    Err(e) => {
                        tracing::warn!("list_channel_by_user_id failed: {e}");
                    }
                }
            });
        })
        .detach();
    }

    pub fn user_channel(&self, channel_id: ChannelId) -> Option<&Channel> {
        self.user_channels.get(&channel_id)
    }

    pub fn user_channels(&self) -> impl Iterator<Item = &Channel> + '_ {
        self.user_channels.values()
    }

    pub fn ensure_user_channels_loaded(&mut self, cx: &mut Context<Self>) {
        if self.user_channels.is_empty() && !self.user_channels_loading {
            self.fetch_user_channels(cx);
        }
    }

    async fn fetch_clan_data(
        api: &AppApi,
        clan_id: ClanId,
    ) -> anyhow::Result<(Vec<Category>, Vec<AppChannel>, Vec<(ChannelId, MessageId)>)> {
        let (channels_res, categories_res, badges_res, voice_res, favorites_res, apps_res) = tokio::join!(
            api.list_channel_descs(clan_id.get(), 1),
            api.list_categories_typed(clan_id.get()),
            api.list_channel_badge_counts(clan_id.get()),
            api.list_voice_channel_users(clan_id.get()),
            api.list_favorite_channels(clan_id.get()),
            api.list_channel_apps(clan_id.get()),
        );

        let api_channels = channels_res?;
        let api_categories = categories_res?;
        let badge_descs = badges_res.unwrap_or_else(|e| {
            tracing::warn!("list_channel_badge_counts failed: {e}");
            Vec::new()
        });
        let voice_users = voice_res.unwrap_or_else(|e| {
            tracing::warn!("list_voice_channel_users failed: {e}");
            Vec::new()
        });
        let favorite_ids: HashSet<ChannelId> = favorites_res
            .unwrap_or_else(|e| {
                tracing::warn!("list_favorite_channels failed: {e}");
                Vec::new()
            })
            .into_iter()
            .filter_map(|s| s.parse::<ChannelId>().ok())
            .collect();
        let app_channels: Vec<AppChannel> = apps_res
            .unwrap_or_else(|e| {
                tracing::warn!("list_channel_apps failed: {e}");
                Vec::new()
            })
            .into_iter()
            .map(AppChannel::from)
            .collect();

        let badge_map: HashMap<ChannelId, ApiChannelDesc> = badge_descs
            .into_iter()
            .filter(|d| {
                !matches!(
                    ChannelType::from_raw(d.channel_type),
                    ChannelType::App | ChannelType::Voice
                )
            })
            .map(|d| (ChannelId(d.channel_id), d))
            .collect();

        let voice_map: HashMap<ChannelId, Vec<UserId>> = voice_users
            .into_iter()
            .map(|v| {
                (
                    ChannelId(v.channel_id),
                    v.user_ids.into_iter().map(UserId).collect(),
                )
            })
            .collect();

        let mut channels: Vec<Channel> = api_channels
            .into_iter()
            .map(|mut c| {
                let cid = ChannelId(c.channel_id);
                let badge = if let Some(b) = badge_map.get(&cid) {
                    if b.last_seen_timestamp > 0 {
                        c.last_seen_timestamp = b.last_seen_timestamp;
                        c.last_seen_message_id = b.last_seen_message_id;
                    }
                    if b.last_sent_timestamp > 0 {
                        c.last_sent_timestamp = b.last_sent_timestamp;
                        c.last_sent_message_id = b.last_sent_message_id;
                    }
                    b.badge_count
                } else {
                    c.badge_count
                };
                let badge = badge.max(0) as u32;
                let voice_ids = voice_map.get(&cid).cloned().unwrap_or_default();
                let is_favorite = favorite_ids.contains(&cid);
                channel_from_desc(c, badge, voice_ids, is_favorite)
            })
            .collect();

        let last_messages: Vec<(ChannelId, MessageId)> = channels
            .iter()
            .filter(|ch| !ch.last_sent_message_id.is_zero())
            .map(|ch| (ch.id, ch.last_sent_message_id))
            .collect();

        let categories = build_categories(api_categories, &mut channels);
        Ok((
            assemble_with_favorites(categories, clan_id),
            app_channels,
            last_messages,
        ))
    }

    fn notify_channel_list(&self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.active_clan_id == Some(clan_id) {
            cx.notify();
        }
    }

    pub fn channel_badge_count(&self, clan_id: ClanId, channel_id: ChannelId) -> u32 {
        self.channel(clan_id, channel_id)
            .map(|ch| ch.badge_count)
            .unwrap_or(0)
    }

    pub fn is_clan_cache_loaded(&self, clan_id: ClanId) -> bool {
        self.cache.contains(&clan_id)
    }

    pub fn clan_has_any_unread(&self, clan_id: ClanId) -> bool {
        self.cache.get(&clan_id).is_some_and(|categories| {
            categories
                .iter()
                .flat_map(|category| &category.channels)
                .any(|ch| ch.is_unread())
        })
    }

    fn mark_channels_read(channels: &mut [Channel]) -> u32 {
        let mut cleared_badge = 0;
        for ch in channels {
            cleared_badge += ch.badge_count;
            ch.badge_count = 0;
            ch.last_seen_timestamp = ch.last_sent_timestamp;
            ch.last_seen_message_id = ch.last_sent_message_id;
        }
        cleared_badge
    }

    fn clan_total_badge(&self, clan_id: ClanId) -> Option<u32> {
        self.cache.get(&clan_id).map(|categories| {
            let mut seen = HashSet::new();
            categories
                .iter()
                .flat_map(|category| &category.channels)
                .filter(|ch| seen.insert(ch.id))
                .map(|ch| ch.badge_count)
                .sum()
        })
    }

    fn sync_clan_after_read(&self, clan_id: ClanId, cleared_badge: u32, cx: &mut Context<Self>) {
        let has_unread = self.clan_has_any_unread(clan_id);
        let total = self.clan_total_badge(clan_id);
        ClanList::global(cx).update(cx, |cls, cx| {
            match total {
                Some(count) => cls.set_badge_count(clan_id, count, cx),
                None if cleared_badge > 0 => cls.decrement_badge(clan_id, cleared_badge, cx),
                None => {}
            }
            cls.set_has_unread(clan_id, has_unread, cx);
        });
    }

    pub fn apply_mark_as_read_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let mut cleared_badge = 0;
        let mut should_notify = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for category in categories.iter_mut() {
                for ch in &category.channels {
                    should_notify = should_notify || ch.is_unread();
                }
                cleared_badge += Self::mark_channels_read(&mut category.channels);
            }
        }
        clear_topic_badges_for_clan(&mut self.topic_parent_badges, clan_id);
        if should_notify {
            self.notify_channel_list(clan_id, cx);
        }
        self.sync_clan_after_read(clan_id, cleared_badge, cx);
    }

    pub fn apply_mark_as_read_category(
        &mut self,
        clan_id: ClanId,
        category_id: i64,
        cx: &mut Context<Self>,
    ) {
        let category_key = category_id.to_string();
        let mut cleared_badge = 0;
        let mut should_notify = false;
        let mut parent_ids = Vec::new();
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for category in categories.iter_mut().filter(|c| c.id == category_key) {
                for ch in &category.channels {
                    should_notify = should_notify || ch.is_unread();
                    parent_ids.push(ch.id);
                }
                cleared_badge += Self::mark_channels_read(&mut category.channels);
            }
            for ch in categories
                .iter_mut()
                .flat_map(|c| c.channels.iter_mut())
                .filter(|ch| parent_ids.contains(&ch.id))
            {
                ch.badge_count = 0;
                ch.last_seen_timestamp = ch.last_sent_timestamp;
                ch.last_seen_message_id = ch.last_sent_message_id;
            }
        }
        self.topic_parent_badges
            .retain(|_, tracked| !parent_ids.contains(&tracked.parent_id));
        if should_notify {
            self.notify_channel_list(clan_id, cx);
        }
        self.sync_clan_after_read(clan_id, cleared_badge, cx);
    }

    pub fn note_channel_message(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        is_mention: bool,
        seen: bool,
        ts: i64,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        let mut visible_changed = false;
        let mut found = false;
        let mut updated_channel = None;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for ch in categories
                .iter_mut()
                .flat_map(|c| c.channels.iter_mut())
                .filter(|ch| ch.id == channel_id)
            {
                found = true;
                let was_unread = ch.is_unread();
                let was_badge = ch.badge_count;
                if ts > 0 {
                    ch.last_sent_timestamp = ts;
                    if !message_id.is_zero() {
                        ch.last_sent_message_id = message_id;
                    }
                    if seen {
                        ch.last_seen_timestamp = ts;
                        if !message_id.is_zero() {
                            ch.last_seen_message_id = message_id;
                        }
                    }
                }
                if is_mention && !seen {
                    ch.badge_count = ch.badge_count.saturating_add(1);
                }
                visible_changed =
                    visible_changed || was_unread != ch.is_unread() || was_badge != ch.badge_count;
                updated_channel = Some(ch.clone());
            }
        }
        if !found {
            if is_mention && !seen {
                *self.pending_channel_badges.entry(channel_id).or_default() += 1;
            }
            self.patch_user_channel_message(channel_id, is_mention, seen, ts, message_id, cx);
        } else if let Some(channel) = updated_channel {
            self.sync_user_channel_from(&channel, cx);
        }
        if visible_changed {
            self.notify_channel_list(clan_id, cx);
        }
    }

    pub fn palette_channel_unread(&self, channel: &Channel) -> (u32, i64, i64) {
        if let Some(live) = self.channel(channel.clan_id, channel.id) {
            return (
                live.badge_count,
                live.last_sent_timestamp,
                live.last_seen_timestamp,
            );
        }
        let pending = self
            .pending_channel_badges
            .get(&channel.id)
            .copied()
            .unwrap_or(0);
        (
            channel.badge_count.max(pending),
            channel.last_sent_timestamp,
            channel.last_seen_timestamp,
        )
    }

    fn sync_user_channel_from(&mut self, source: &Channel, cx: &mut Context<Self>) {
        let Some(user_channel) = self.user_channels.get_mut(&source.id) else {
            return;
        };
        let was_unread = user_channel.is_unread();
        let was_badge = user_channel.badge_count;
        copy_channel_unread_fields(user_channel, source);
        if was_unread != user_channel.is_unread() || was_badge != user_channel.badge_count {
            cx.notify();
        }
    }

    fn patch_user_channel_message(
        &mut self,
        channel_id: ChannelId,
        is_mention: bool,
        seen: bool,
        ts: i64,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        let Some(user_channel) = self.user_channels.get_mut(&channel_id) else {
            return;
        };
        let was_unread = user_channel.is_unread();
        let was_badge = user_channel.badge_count;
        if ts > 0 {
            user_channel.last_sent_timestamp = ts;
            if !message_id.is_zero() {
                user_channel.last_sent_message_id = message_id;
            }
            if seen {
                user_channel.last_seen_timestamp = ts;
                if !message_id.is_zero() {
                    user_channel.last_seen_message_id = message_id;
                }
            }
        }
        if is_mention && !seen {
            user_channel.badge_count = user_channel.badge_count.saturating_add(1);
        }
        if was_unread != user_channel.is_unread() || was_badge != user_channel.badge_count {
            cx.notify();
        }
    }

    fn bump_user_channel_badge(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        let Some(user_channel) = self.user_channels.get_mut(&channel_id) else {
            return;
        };
        user_channel.badge_count = user_channel.badge_count.saturating_add(1);
        cx.notify();
    }

    pub fn note_user_channel_dm_message(
        &mut self,
        channel_id: ChannelId,
        ts: i64,
        from_me: bool,
        increment_unread: bool,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        let Some(user_channel) = self.user_channels.get_mut(&channel_id) else {
            return;
        };
        if !Self::is_user_channel_dm_type(user_channel.channel_type, user_channel.clan_id) {
            return;
        }
        let was_unread = user_channel.is_unread();
        let was_badge = user_channel.badge_count;
        if ts > 0 {
            user_channel.last_sent_timestamp = ts;
            if !message_id.is_zero() {
                user_channel.last_sent_message_id = message_id;
            }
        }
        if from_me {
            if ts > 0 {
                user_channel.last_seen_timestamp = ts;
                if !message_id.is_zero() {
                    user_channel.last_seen_message_id = message_id;
                }
            }
        } else if increment_unread {
            user_channel.badge_count = user_channel.badge_count.saturating_add(1);
        }
        if was_unread != user_channel.is_unread() || was_badge != user_channel.badge_count {
            cx.notify();
        }
    }

    fn is_user_channel_dm_type(channel_type: ChannelType, clan_id: ClanId) -> bool {
        clan_id.is_zero()
            && matches!(
                channel_type,
                ChannelType::Unknown(2) | ChannelType::Unknown(3)
            )
    }

    pub fn decrement_channel_on_delete(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        message_ts: i64,
        cx: &mut Context<Self>,
    ) -> bool {
        let decremented = match self.cache.get_mut(&clan_id) {
            Some(categories) => {
                decrement_channel_badge_on_delete(categories, channel_id, message_ts)
            }
            None => false,
        };
        if decremented {
            self.notify_channel_list(clan_id, cx);
        }
        decremented
    }

    pub fn apply_read(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        let mut cleared_badge = 0;
        let mut should_notify = false;
        let mut found = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for ch in categories
                .iter_mut()
                .flat_map(|c| c.channels.iter_mut())
                .filter(|ch| ch.id == channel_id)
            {
                if !found {
                    found = true;
                    cleared_badge = ch.badge_count;
                    should_notify = ch.is_unread() || cleared_badge > 0;
                }
                ch.badge_count = 0;
                ch.last_seen_timestamp = ch.last_sent_timestamp;
                ch.last_seen_message_id = ch.last_sent_message_id;
            }
        }
        let overlaid = self.pending_channel_badges.remove(&channel_id);
        if !found {
            cleared_badge = overlaid.unwrap_or(0);
        }
        clear_topic_badges_for_parent(&mut self.topic_parent_badges, channel_id);
        if let Some(user_channel) = self.user_channels.get_mut(&channel_id) {
            user_channel.badge_count = 0;
            user_channel.last_seen_timestamp = user_channel.last_sent_timestamp;
            user_channel.last_seen_message_id = user_channel.last_sent_message_id;
            cx.notify();
        }
        if should_notify {
            self.notify_channel_list(clan_id, cx);
        }
        self.sync_clan_after_read(clan_id, cleared_badge, cx);
    }

    pub fn apply_clan_read(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let mut changed = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for category in categories.iter_mut() {
                for ch in &category.channels {
                    if ch.badge_count > 0 || ch.is_unread() {
                        changed = true;
                    }
                }
                Self::mark_channels_read(&mut category.channels);
            }
        }
        if changed {
            self.notify_channel_list(clan_id, cx);
        }
    }

    pub fn mark_clan_as_read(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.apply_clan_read(clan_id, cx);
        ClanList::global(cx).update(cx, |cl, cx| cl.apply_badge_read(clan_id, cx));
        let api = self.api.clone();
        let id = clan_id.get();
        cx.spawn(async move |_, _| {
            if let Err(e) = api.mark_as_read(0, 0, id).await {
                tracing::error!("mark clan as read failed for clan {id}: {e}");
            }
        })
        .detach();
    }

    pub fn is_show_empty_category(&self, clan_id: ClanId) -> bool {
        self.show_empty_categories.contains(&clan_id)
    }

    pub fn set_show_empty_category(&mut self, clan_id: ClanId, show: bool, cx: &mut Context<Self>) {
        if show {
            self.show_empty_categories.insert(clan_id);
        } else {
            self.show_empty_categories.remove(&clan_id);
        }
        cx.notify();
    }

    pub fn apply_last_seen(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        new_badge: u32,
        seen_ts: i64,
        seen_message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        let mut visible_changed = false;
        let mut badge_delta = 0;
        let mut computed = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for ch in categories
                .iter_mut()
                .flat_map(|c| c.channels.iter_mut())
                .filter(|ch| ch.id == channel_id)
            {
                let was_unread = ch.is_unread();
                let was_badge = ch.badge_count;
                ch.badge_count = new_badge;
                if seen_ts > ch.last_seen_timestamp {
                    ch.last_seen_timestamp = seen_ts;
                }
                if !seen_message_id.is_zero() {
                    ch.last_seen_message_id = seen_message_id;
                }
                if !computed {
                    computed = true;
                    visible_changed = was_unread != ch.is_unread() || was_badge != ch.badge_count;
                    if was_badge > new_badge {
                        badge_delta = was_badge - new_badge;
                    }
                }
            }
        }
        clear_topic_badges_for_parent(&mut self.topic_parent_badges, channel_id);
        if let Some(live) = self.channel(clan_id, channel_id).cloned() {
            self.sync_user_channel_from(&live, cx);
        }
        if visible_changed {
            self.notify_channel_list(clan_id, cx);
        }
        if badge_delta > 0 || visible_changed {
            self.sync_clan_after_read(clan_id, badge_delta, cx);
        }
    }

    pub fn increment_channel_for_topic(
        &mut self,
        clan_id: ClanId,
        parent_id: ChannelId,
        topic_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let changed = match self.cache.get_mut(&clan_id) {
            Some(categories) => add_channel_badge(categories, parent_id),
            None => {
                *self.pending_channel_badges.entry(parent_id).or_default() += 1;
                false
            }
        };
        if changed {
            record_topic_parent_badge(&mut self.topic_parent_badges, clan_id, parent_id, topic_id);
            self.notify_channel_list(clan_id, cx);
        }
        if let Some(parent) = self.channel(clan_id, parent_id).cloned() {
            self.sync_user_channel_from(&parent, cx);
        } else {
            self.bump_user_channel_badge(parent_id, cx);
        }
    }

    pub fn apply_topic_read(&mut self, topic_id: ChannelId, cx: &mut Context<Self>) {
        let Some(tracked) = self.topic_parent_badges.remove(&topic_id) else {
            return;
        };
        let cleared = match self.cache.get_mut(&tracked.clan_id) {
            Some(categories) => {
                subtract_channel_badge(categories, tracked.parent_id, tracked.count)
            }
            None => 0,
        };
        if cleared > 0 {
            self.notify_channel_list(tracked.clan_id, cx);
            self.sync_clan_after_read(tracked.clan_id, cleared, cx);
        }
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::ChannelCreated(e) => {
                let clan_id = ClanId(e.clan_id);
                if self.cache.contains(&clan_id) {
                    let channel = Channel {
                        id: ChannelId(e.channel_id),
                        name: e.channel_label.clone(),
                        channel_type: ChannelType::from_raw(e.channel_type as u32),
                        private: e.channel_private != 0,
                        clan_id,
                        clan_name: String::new(),
                        category_name: String::new(),
                        category_id: Some(e.category_id.to_string())
                            .filter(|s| !s.is_empty() && s != "0"),
                        member_count: 0,
                        badge_count: 0,
                        muted: false,
                        parent_id: Some(ChannelId(e.parent_id)).filter(|c| !c.is_zero()),
                        last_seen_message_id: MessageId(0),
                        last_seen_timestamp: 0,
                        last_sent_message_id: MessageId(0),
                        last_sent_timestamp: 0,
                        voice_members: Vec::new(),
                        is_favorite: false,
                        creator_id: UserId(e.creator_id),
                    };
                    let inserted = if let Some(cats) = self.cache.get_mut(&clan_id) {
                        insert_channel(cats, channel)
                    } else {
                        false
                    };
                    if inserted {
                        self.invalidate_channel_index(clan_id);
                        cx.notify();
                    }
                }
            }
            RealtimeEvent::ChannelUpdated(e) => {
                let id = ChannelId(e.channel_id);
                let label = (!e.channel_label.is_empty()).then_some(e.channel_label.clone());
                let mut changed = false;
                for cats in self.cache.values_mut() {
                    if update_channel(cats, id, label.clone(), e.channel_private) {
                        changed = true;
                        break;
                    }
                }
                if changed {
                    cx.notify();
                }
            }
            RealtimeEvent::ChannelDeleted(e) => {
                let id = ChannelId(e.channel_id);
                let mut removed = false;
                for cats in self.cache.values_mut() {
                    removed |= remove_channel(cats, id);
                }
                if removed {
                    self.invalidate_channel_index_all();
                    if self.active_channel_id == Some(id) {
                        self.active_channel_id = None;
                        cx.emit(ChannelEvent::ActiveChannelChanged(None));
                    }
                    cx.notify();
                }
            }
            RealtimeEvent::VoiceJoined(e) => {
                let clan_id = ClanId(e.clan_id);
                let channel_id = ChannelId(e.voice_channel_id);
                let user_id = UserId(e.user_id);
                let display_name = if !e.participant.is_empty() {
                    e.participant.clone()
                } else {
                    user_id.to_string()
                };
                let member = VoiceMember {
                    user_id,
                    display_name,
                    avatar_url: String::new(),
                };
                let mut changed = false;
                if let Some(cats) = self.cache.get_mut(&clan_id) {
                    for ch in cats
                        .iter_mut()
                        .flat_map(|c| c.channels.iter_mut())
                        .filter(|ch| ch.id == channel_id)
                    {
                        if !ch.voice_members.iter().any(|m| m.user_id == user_id) {
                            ch.voice_members.push(member.clone());
                            changed = true;
                        }
                    }
                }
                if changed {
                    cx.notify();
                }
            }
            RealtimeEvent::VoiceLeaved(e) => {
                let clan_id = ClanId(e.clan_id);
                let channel_id = ChannelId(e.voice_channel_id);
                let user_id = UserId(e.voice_user_id);
                let mut changed = false;
                if let Some(cats) = self.cache.get_mut(&clan_id) {
                    for ch in cats
                        .iter_mut()
                        .flat_map(|c| c.channels.iter_mut())
                        .filter(|ch| ch.id == channel_id)
                    {
                        let before = ch.voice_members.len();
                        ch.voice_members.retain(|m| m.user_id != user_id);
                        if ch.voice_members.len() != before {
                            changed = true;
                        }
                    }
                }
                if changed {
                    cx.notify();
                }
            }
            RealtimeEvent::UserChannelAdded(e) => {
                let Some(ref desc) = e.channel_desc else {
                    return;
                };
                let channel_type = desc.r#type as u32;
                if channel_type == 2 || channel_type == 3 {
                    return;
                }
                let clan_id = ClanId(e.clan_id);
                let channel_id = ChannelId(desc.channel_id);
                let Some(cats) = self.cache.get_mut(&clan_id) else {
                    return;
                };
                let already_exists = cats
                    .iter()
                    .flat_map(|c| &c.channels)
                    .any(|ch| ch.id == channel_id);
                if already_exists {
                    return;
                }
                let channel = Channel {
                    id: channel_id,
                    name: desc.channel_label.clone(),
                    channel_type: ChannelType::from_raw(channel_type),
                    private: desc.channel_private != 0,
                    clan_id,
                    clan_name: desc.clan_name.clone(),
                    category_name: String::new(),
                    category_id: Some(desc.category_id.to_string())
                        .filter(|s| !s.is_empty() && s != "0"),
                    member_count: 0,
                    badge_count: 0,
                    muted: false,
                    parent_id: Some(ChannelId(desc.parent_id)).filter(|c| !c.is_zero()),
                    last_seen_message_id: MessageId(0),
                    last_seen_timestamp: 0,
                    last_sent_message_id: MessageId(0),
                    last_sent_timestamp: 0,
                    voice_members: Vec::new(),
                    is_favorite: false,
                    creator_id: UserId(desc.creator_id),
                };
                let inserted = insert_channel(cats, channel);
                if inserted {
                    self.invalidate_channel_index(clan_id);
                    cx.notify();
                }
            }
            RealtimeEvent::UserChannelRemoved(e) => {
                let channel_id = ChannelId(e.channel_id);
                let channel_type = e.channel_type as u32;
                if channel_type == 2 || channel_type == 3 {
                    return;
                }
                let mut removed = false;
                for cats in self.cache.values_mut() {
                    removed |= remove_channel(cats, channel_id);
                }
                if removed {
                    self.invalidate_channel_index_all();
                    if self.active_channel_id == Some(channel_id) {
                        self.active_channel_id = None;
                        cx.emit(ChannelEvent::ActiveChannelChanged(None));
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn resync(&mut self, cx: &mut Context<Self>) {
        tracing::info!("ChannelList resync — invalidating channel cache");
        self.cache.mark_all_stale();
        self.invalidate_channel_index_all();
        self.fetch_user_channels(cx);
        if let Some(clan_id) = self.active_clan_id {
            self.load_for_clan(clan_id, cx);
        }
    }

    pub fn active_channel(&self) -> Option<&Channel> {
        self.active_channel_id
            .as_ref()
            .and_then(|id| self.find_channel_in_active_clan(*id))
    }

    pub fn categories_for_clan(&self, clan_id: ClanId) -> &[Category] {
        self.cache.get(&clan_id).map_or(&[], Vec::as_slice)
    }

    pub fn is_loading_clan(&self, clan_id: ClanId) -> bool {
        self.loading.contains(&clan_id)
    }

    pub fn app_channels_for_clan(&self, clan_id: ClanId) -> &[AppChannel] {
        self.app_channels_cache
            .get(&clan_id)
            .map_or(&[], Vec::as_slice)
    }

    pub fn channel_in_clan(&self, clan_id: ClanId, channel_id: ChannelId) -> bool {
        self.categories_for_clan(clan_id)
            .iter()
            .flat_map(|category| &category.channels)
            .any(|channel| channel.id == channel_id)
    }

    pub fn remembered_channel(&self, clan_id: ClanId) -> Option<ChannelId> {
        self.remembered_channels.get(&clan_id).copied()
    }

    pub fn previous_channels_for_clan(&self, clan_id: ClanId) -> &[ChannelId] {
        self.previous_channels
            .get(&clan_id)
            .map(|channels| channels.as_slice())
            .unwrap_or(&[])
    }

    pub fn channel_display_name(&self, clan_id: ClanId, channel_id: ChannelId) -> Option<String> {
        self.user_channels
            .get(&channel_id)
            .map(|channel| channel.name.clone())
            .or_else(|| {
                self.channel(clan_id, channel_id)
                    .map(|channel| channel.name.clone())
            })
    }

    pub fn record_previous_channel(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let entry = self.previous_channels.entry(clan_id).or_default();
        entry.retain(|id| *id != channel_id);
        entry.insert(0, channel_id);
        entry.truncate(5);
        self.persist_previous_channels(cx);
    }

    fn persist_previous_channels(&self, cx: &mut Context<Self>) {
        let snapshot = self.previous_channels.clone();
        cx.background_executor()
            .spawn(async move { save_previous_channels(snapshot) })
            .detach();
    }

    pub fn reset_user_channel_unread(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        let Some(ch) = self.user_channels.get_mut(&channel_id) else {
            return;
        };
        if ch.badge_count == 0 && ch.last_seen_timestamp >= ch.last_sent_timestamp {
            return;
        }
        ch.badge_count = 0;
        ch.last_seen_timestamp = ch.last_sent_timestamp;
        cx.notify();
    }

    pub fn default_channel_id(&self, clan_id: ClanId) -> Option<ChannelId> {
        self.categories_for_clan(clan_id)
            .iter()
            .filter(|cat| cat.id != FAVOR_CATE_ID)
            .flat_map(|category| &category.channels)
            .find(|channel| channel.channel_type == ChannelType::Text)
            .map(|channel| channel.id)
    }

    pub fn select_channel(&mut self, id: ChannelId, cx: &mut Context<Self>) {
        if self.active_channel_id == Some(id) {
            return;
        }
        self.active_channel_id = Some(id);
        if let Some(clan_id) = self.clan_id_for_channel(id) {
            self.remembered_channels.insert(clan_id, id);
            self.apply_read(clan_id, id, cx);
        } else {
            self.mark_read(id);
        }
        cx.emit(ChannelEvent::ActiveChannelChanged(self.active_channel_id));
        cx.notify();
    }

    pub fn mark_read(&mut self, id: ChannelId) {
        if let Some(ch) = self
            .cache
            .values_mut()
            .flat_map(|cats| cats.iter_mut().flat_map(|c| &mut c.channels))
            .find(|ch| ch.id == id)
        {
            ch.badge_count = 0;
            ch.last_seen_timestamp = ch.last_sent_timestamp;
        }
    }

    pub fn channel(&self, clan_id: ClanId, channel_id: ChannelId) -> Option<&Channel> {
        let (cat_idx, ch_idx) = self.channel_location(clan_id, channel_id)?;
        self.cache.get(&clan_id)?.get(cat_idx)?.channels.get(ch_idx)
    }

    pub fn find_channel_in_active_clan(&self, channel_id: ChannelId) -> Option<&Channel> {
        self.channel(self.active_clan_id?, channel_id)
    }

    pub fn ensure_thread_channel(
        &mut self,
        thread_id: ChannelId,
        label: String,
        cx: &mut Context<Self>,
    ) -> Option<ClanId> {
        if let Some(channel) = self.find_channel_in_active_clan(thread_id) {
            return Some(channel.clan_id);
        }
        let clan_id = self.active_clan_id?;
        let parent_id = self.active_channel_id?;
        let channel = thread_channel_from_context(thread_id, label, clan_id, parent_id);
        let inserted = if let Some(categories) = self.cache.get_mut(&clan_id) {
            insert_channel(categories, channel)
        } else {
            false
        };
        if inserted {
            self.invalidate_channel_index(clan_id);
            cx.notify();
        }
        Some(clan_id)
    }

    pub fn ensure_thread_with_parent(
        &mut self,
        thread_id: ChannelId,
        parent_id: ChannelId,
        clan_id: ClanId,
        label: String,
        cx: &mut Context<Self>,
    ) {
        if self.channel(clan_id, thread_id).is_some() {
            return;
        }
        let channel = thread_channel_from_context(thread_id, label, clan_id, parent_id);
        let inserted = if let Some(categories) = self.cache.get_mut(&clan_id) {
            insert_channel(categories, channel)
        } else {
            false
        };
        if inserted {
            self.invalidate_channel_index(clan_id);
            cx.notify();
        }
    }

    pub fn clan_id_for_channel(&self, channel_id: ChannelId) -> Option<ClanId> {
        self.channel_index
            .borrow_mut()
            .clan_for(&self.cache, channel_id)
    }

    pub fn is_category_collapsed(&self, clan_id: ClanId, cat_id: &str) -> bool {
        self.collapsed
            .contains(&(clan_id.to_string(), cat_id.to_string()))
    }

    pub fn toggle_category(&mut self, clan_id: ClanId, cat_id: &str, cx: &mut Context<Self>) {
        let key = (clan_id.to_string(), cat_id.to_string());
        if self.collapsed.contains(&key) {
            self.collapsed.remove(&key);
        } else {
            self.collapsed.insert(key);
        }
        cx.notify();
        let snapshot: Vec<(String, String)> = self.collapsed.iter().cloned().collect();
        cx.background_executor()
            .spawn(async move { save_collapse_state(snapshot) })
            .detach();
    }

    pub fn add_channel_favorite(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        if let Some(cats) = self.cache.get_mut(&clan_id) {
            let channel = cats
                .iter()
                .flat_map(|c| c.channels.iter())
                .find(|ch| ch.id == channel_id)
                .cloned();
            if let Some(mut ch) = channel {
                ch.is_favorite = true;
                if let Some(favor_cat) = cats.iter_mut().find(|c| c.id == FAVOR_CATE_ID) {
                    if !favor_cat.channels.iter().any(|c| c.id == ch.id) {
                        favor_cat.channels.push(ch.clone());
                    }
                } else {
                    let favor_cate = Category {
                        id: FAVOR_CATE_ID.to_string(),
                        clan_id,
                        name: "favoriteChannel".to_string(),
                        order: i32::MIN,
                        channels: vec![ch.clone()],
                    };
                    cats.insert(0, favor_cate);
                }
                for cat in cats.iter_mut() {
                    for existing in cat.channels.iter_mut() {
                        if existing.id == channel_id {
                            existing.is_favorite = true;
                        }
                    }
                }
                changed = true;
            }
        }
        if changed {
            self.invalidate_channel_index(clan_id);
            cx.notify();
        }

        let api = self.api.clone();
        let cid = channel_id;
        let clid = clan_id;
        cx.spawn(async move |_, _| {
            if let Err(e) = api.add_channel_favorite(cid.get(), clid.get()).await {
                tracing::error!("add_channel_favorite failed: {e}");
            }
        })
        .detach();
    }

    pub fn remove_channel_favorite(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        if let Some(cats) = self.cache.get_mut(&clan_id) {
            for cat in cats.iter_mut() {
                for ch in cat.channels.iter_mut() {
                    if ch.id == channel_id {
                        ch.is_favorite = false;
                    }
                }
            }
            if let Some(favor_cat) = cats.iter_mut().find(|c| c.id == FAVOR_CATE_ID) {
                favor_cat.channels.retain(|ch| ch.id != channel_id);
            }
            changed = true;
        }
        if changed {
            self.invalidate_channel_index(clan_id);
            cx.notify();
        }

        let api = self.api.clone();
        let cid = channel_id;
        let clid = clan_id;
        cx.spawn(async move |_, _| {
            if let Err(e) = api.remove_channel_favorite(cid.get(), clid.get()).await {
                tracing::error!("remove_channel_favorite failed: {e}");
            }
        })
        .detach();
    }
}

fn thread_channel_from_context(
    thread_id: ChannelId,
    label: String,
    clan_id: ClanId,
    parent_id: ChannelId,
) -> Channel {
    Channel {
        id: thread_id,
        name: label,
        channel_type: ChannelType::Thread,
        private: false,
        clan_id,
        clan_name: String::new(),
        category_name: String::new(),
        category_id: None,
        member_count: 0,
        badge_count: 0,
        muted: false,
        parent_id: Some(parent_id),
        last_seen_message_id: MessageId(0),
        last_seen_timestamp: 0,
        last_sent_message_id: MessageId(0),
        last_sent_timestamp: 0,
        voice_members: Vec::new(),
        is_favorite: false,
        creator_id: UserId(0),
    }
}

fn channel_from_desc(
    c: ApiChannelDesc,
    badge_count: u32,
    voice_ids: Vec<UserId>,
    is_favorite: bool,
) -> Channel {
    let voice_members = voice_ids
        .into_iter()
        .map(|uid| VoiceMember {
            display_name: uid.to_string(),
            avatar_url: String::new(),
            user_id: uid,
        })
        .collect();
    Channel {
        id: ChannelId(c.channel_id),
        name: c.channel_label,
        channel_type: ChannelType::from_raw(c.channel_type),
        private: c.channel_private != 0,
        clan_id: ClanId(c.clan_id),
        clan_name: c.clan_name,
        category_name: c.category_name,
        category_id: Some(c.category_id.to_string()).filter(|s| !s.is_empty() && s != "0"),
        member_count: c.member_count.max(0) as u32,
        badge_count,
        muted: c.is_mute,
        parent_id: Some(ChannelId(c.parent_id)).filter(|p| !p.is_zero()),
        last_seen_message_id: MessageId(c.last_seen_message_id),
        last_seen_timestamp: c.last_seen_timestamp,
        last_sent_message_id: MessageId(c.last_sent_message_id),
        last_sent_timestamp: c.last_sent_timestamp,
        voice_members,
        is_favorite,
        creator_id: UserId(c.creator_id),
    }
}

fn assemble_with_favorites(mut categories: Vec<Category>, clan_id: ClanId) -> Vec<Category> {
    let favor_channels: Vec<Channel> = categories
        .iter()
        .flat_map(|cat| cat.channels.iter())
        .filter(|ch| ch.is_favorite)
        .cloned()
        .collect();
    let favor_clan_id = favor_channels
        .first()
        .map(|ch| ch.clan_id)
        .unwrap_or(clan_id);
    categories.insert(
        0,
        Category {
            id: FAVOR_CATE_ID.to_string(),
            clan_id: favor_clan_id,
            name: "favoriteChannel".to_string(),
            order: i32::MIN,
            channels: favor_channels,
        },
    );
    categories
}

fn flatten_parents_with_threads(
    mut parents: Vec<Channel>,
    thread_groups: &mut HashMap<ChannelId, Vec<Channel>>,
) -> Vec<Channel> {
    parents.sort_by_key(|a| a.id);
    let mut ordered: Vec<Channel> = Vec::with_capacity(parents.len() * 2);
    for parent in parents {
        let threads = thread_groups.remove(&parent.id);
        ordered.push(parent);
        if let Some(mut ts) = threads {
            ts.sort_by_key(|a| a.id);
            ordered.extend(ts);
        }
    }
    ordered
}

fn build_categories(
    api_categories: Vec<ApiCategoryDesc>,
    channels: &mut Vec<Channel>,
) -> Vec<Category> {
    let mut parents_by_cat: HashMap<String, Vec<Channel>> = HashMap::new();
    let mut thread_groups: HashMap<ChannelId, Vec<Channel>> = HashMap::new();

    for ch in std::mem::take(channels) {
        if let Some(pid) = ch.parent_id {
            thread_groups.entry(pid).or_default().push(ch);
        } else {
            let cat_id = ch.category_id.clone().unwrap_or_else(|| "0".to_string());
            parents_by_cat.entry(cat_id).or_default().push(ch);
        }
    }

    let mut result: Vec<Category> = Vec::with_capacity(api_categories.len());

    for c in api_categories {
        let cat_id = c.category_id.to_string();
        let parents = parents_by_cat.remove(&cat_id).unwrap_or_default();
        result.push(Category {
            id: cat_id,
            clan_id: ClanId(c.clan_id),
            name: c.category_name,
            order: c.category_order,
            channels: flatten_parents_with_threads(parents, &mut thread_groups),
        });
    }

    for (cat_id, parents) in parents_by_cat {
        let Some(clan_id) = parents.first().map(|ch| ch.clan_id) else {
            continue;
        };
        result.push(Category {
            id: cat_id,
            clan_id,
            name: "General".to_string(),
            order: i32::MAX,
            channels: flatten_parents_with_threads(parents, &mut thread_groups),
        });
    }

    result
}

fn insert_channel(categories: &mut Vec<Category>, mut channel: Channel) -> bool {
    if categories
        .iter()
        .flat_map(|c| &c.channels)
        .any(|c| c.id == channel.id)
    {
        return false;
    }
    let clan_id = channel.clan_id;

    if let Some(parent_id) = channel.parent_id {
        let cat_id = categories
            .iter()
            .flat_map(|c| c.channels.iter())
            .find(|ch| ch.id == parent_id)
            .and_then(|p| p.category_id.clone());

        let target_cat_id = cat_id.unwrap_or_else(|| "0".to_string());
        let cat_name = categories
            .iter()
            .find(|c| c.id == target_cat_id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "General".to_string());
        channel.category_name = cat_name;

        if let Some(cat) = categories
            .iter_mut()
            .find(|c| c.id != FAVOR_CATE_ID && c.id == target_cat_id)
        {
            let insert_pos = cat
                .channels
                .iter()
                .position(|ch| ch.parent_id == Some(parent_id))
                .map(|first_thread_pos| {
                    let mut end = first_thread_pos;
                    while end < cat.channels.len() && cat.channels[end].parent_id == Some(parent_id)
                    {
                        end += 1;
                    }
                    end
                })
                .or_else(|| {
                    cat.channels
                        .iter()
                        .position(|ch| ch.id == parent_id)
                        .map(|p| p + 1)
                })
                .unwrap_or(cat.channels.len());
            cat.channels.insert(insert_pos, channel);
            return true;
        }
    }

    let (cat_id, cat_name) = channel
        .category_id
        .as_ref()
        .and_then(|cid| {
            categories
                .iter()
                .find(|c| c.id != FAVOR_CATE_ID && c.id == *cid)
                .map(|c| (c.id.clone(), c.name.clone()))
        })
        .unwrap_or_else(|| ("0".to_string(), "General".to_string()));
    channel.category_name = cat_name.clone();

    if let Some(cat) = categories
        .iter_mut()
        .find(|c| c.id != FAVOR_CATE_ID && c.id == cat_id)
    {
        let insert_pos = cat
            .channels
            .iter()
            .position(|ch| ch.id > channel.id)
            .unwrap_or(cat.channels.len());
        cat.channels.insert(insert_pos, channel);
    } else {
        categories.push(Category {
            id: cat_id,
            clan_id,
            name: cat_name,
            order: i32::MAX,
            channels: vec![channel],
        });
    }
    true
}

fn remove_channel(categories: &mut [Category], channel_id: ChannelId) -> bool {
    let mut removed = false;
    for cat in categories.iter_mut() {
        if cat.id == FAVOR_CATE_ID {
            continue;
        }
        let before = cat.channels.len();
        cat.channels.retain(|ch| ch.id != channel_id);
        removed |= cat.channels.len() != before;
    }
    if let Some(favor) = categories.iter_mut().find(|c| c.id == FAVOR_CATE_ID) {
        favor.channels.retain(|ch| ch.id != channel_id);
    }
    removed
}

fn update_channel(
    categories: &mut [Category],
    channel_id: ChannelId,
    label: Option<String>,
    private: bool,
) -> bool {
    let mut found = false;
    for cat in categories.iter_mut() {
        for ch in cat.channels.iter_mut() {
            if ch.id == channel_id {
                if let Some(ref label) = label {
                    ch.name = label.clone();
                }
                ch.private = private;
                found = true;
            }
        }
    }
    found
}

fn copy_channel_unread_fields(target: &mut Channel, source: &Channel) {
    target.badge_count = source.badge_count;
    target.last_sent_timestamp = source.last_sent_timestamp;
    target.last_seen_timestamp = source.last_seen_timestamp;
    target.last_sent_message_id = source.last_sent_message_id;
    target.last_seen_message_id = source.last_seen_message_id;
}

fn merge_pending_badges_into(pending: &mut HashMap<ChannelId, u32>, categories: &mut [Category]) {
    if pending.is_empty() {
        return;
    }
    let mut applied: Vec<ChannelId> = Vec::new();
    for category in categories.iter_mut() {
        for ch in category.channels.iter_mut() {
            if let Some(&overlay) = pending.get(&ch.id) {
                ch.badge_count = ch.badge_count.max(overlay);
                applied.push(ch.id);
            }
        }
    }
    for id in applied {
        pending.remove(&id);
    }
}

fn add_channel_badge(categories: &mut [Category], channel_id: ChannelId) -> bool {
    let mut changed = false;
    let mut computed = false;
    for ch in categories
        .iter_mut()
        .flat_map(|category| category.channels.iter_mut())
        .filter(|ch| ch.id == channel_id)
    {
        let was_unread = ch.is_unread();
        let was_badge = ch.badge_count;
        ch.badge_count = ch.badge_count.saturating_add(1);
        if !computed {
            computed = true;
            changed = was_badge != ch.badge_count || was_unread != ch.is_unread();
        }
    }
    changed
}

fn subtract_channel_badge(categories: &mut [Category], channel_id: ChannelId, amount: u32) -> u32 {
    let mut cleared = 0;
    let mut computed = false;
    for ch in categories
        .iter_mut()
        .flat_map(|category| category.channels.iter_mut())
        .filter(|ch| ch.id == channel_id)
    {
        let was_badge = ch.badge_count;
        ch.badge_count = ch.badge_count.saturating_sub(amount);
        if !computed {
            computed = true;
            cleared = was_badge - ch.badge_count;
        }
    }
    cleared
}

fn decrement_channel_badge_on_delete(
    categories: &mut [Category],
    channel_id: ChannelId,
    message_ts: i64,
) -> bool {
    let mut changed = false;
    for ch in categories
        .iter_mut()
        .flat_map(|category| category.channels.iter_mut())
        .filter(|ch| ch.id == channel_id)
    {
        let last_seen = ch.last_seen_timestamp;
        if last_seen > 0 && message_ts > last_seen && ch.badge_count > 0 {
            ch.badge_count = ch.badge_count.saturating_sub(1);
            changed = true;
        }
    }
    changed
}

fn record_topic_parent_badge(
    topic_badges: &mut HashMap<ChannelId, TopicParentBadge>,
    clan_id: ClanId,
    parent_id: ChannelId,
    topic_id: ChannelId,
) {
    let entry = topic_badges
        .entry(topic_id)
        .or_insert_with(|| TopicParentBadge {
            clan_id,
            parent_id,
            count: 0,
        });
    entry.count = entry.count.saturating_add(1);
}

fn clear_topic_badges_for_parent(
    topic_badges: &mut HashMap<ChannelId, TopicParentBadge>,
    parent_id: ChannelId,
) {
    topic_badges.retain(|_, tracked| tracked.parent_id != parent_id);
}

fn clear_topic_badges_for_clan(
    topic_badges: &mut HashMap<ChannelId, TopicParentBadge>,
    clan_id: ClanId,
) {
    topic_badges.retain(|_, tracked| tracked.clan_id != clan_id);
}

fn load_collapse_state() -> HashSet<(String, String)> {
    let path = collapse_state_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return HashSet::new(),
    };
    let pairs: Vec<(String, String)> = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => return HashSet::new(),
    };
    pairs.into_iter().collect()
}

fn save_collapse_state(pairs: Vec<(String, String)>) {
    let path = collapse_state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string(&pairs) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, data) {
                tracing::warn!("Failed to save collapse state: {e}");
            }
        }
        Err(e) => tracing::warn!("Failed to serialize collapse state: {e}"),
    }
}

fn load_previous_channels() -> HashMap<ClanId, Vec<ChannelId>> {
    let path = previous_channels_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return HashMap::new(),
    };
    match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => HashMap::new(),
    }
}

fn save_previous_channels(channels: HashMap<ClanId, Vec<ChannelId>>) {
    let path = previous_channels_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string(&channels) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, data) {
                tracing::warn!("Failed to save previous channels: {e}");
            }
        }
        Err(e) => tracing::warn!("Failed to serialize previous channels: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_state_roundtrip() {
        let mut collapsed: HashSet<(String, String)> = HashSet::new();
        collapsed.insert(("clan1".into(), "cat1".into()));
        collapsed.insert(("clan1".into(), "cat2".into()));

        let snapshot: Vec<(String, String)> = collapsed.iter().cloned().collect();
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: Vec<(String, String)> = serde_json::from_str(&json).unwrap();
        let restored: HashSet<(String, String)> = parsed.into_iter().collect();

        assert!(restored.contains(&("clan1".into(), "cat1".into())));
        assert!(restored.contains(&("clan1".into(), "cat2".into())));
        assert!(!restored.contains(&("clan2".into(), "cat1".into())));
    }

    #[test]
    fn previous_channels_roundtrip() {
        let mut channels: HashMap<ClanId, Vec<ChannelId>> = HashMap::new();
        channels.insert(
            ClanId(1),
            vec![ChannelId(10), ChannelId(20), ChannelId(30)],
        );
        channels.insert(ClanId(0), vec![ChannelId(99)]);

        let json = serde_json::to_string(&channels).unwrap();
        let restored: HashMap<ClanId, Vec<ChannelId>> = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.get(&ClanId(1)).map(Vec::as_slice), Some(&[ChannelId(10), ChannelId(20), ChannelId(30)][..]));
        assert_eq!(restored.get(&ClanId(0)).map(Vec::as_slice), Some(&[ChannelId(99)][..]));
    }

    #[test]
    fn is_category_collapsed_defaults_to_false() {
        let collapsed: HashSet<(String, String)> = HashSet::new();
        let is_collapsed =
            |clan: &str, cat: &str| collapsed.contains(&(clan.to_string(), cat.to_string()));
        assert!(!is_collapsed("clan1", "cat1"));
    }

    fn make_channel(id: i64, name: &str, cat_id: &str) -> Channel {
        Channel {
            id: ChannelId(id),
            name: name.into(),
            channel_type: ChannelType::Text,
            private: false,
            clan_id: ClanId(1),
            clan_name: String::new(),
            category_name: "General".into(),
            category_id: Some(cat_id.into()),
            member_count: 0,
            badge_count: 0,
            muted: false,
            parent_id: None,
            last_seen_message_id: MessageId(0),
            last_seen_timestamp: 0,
            last_sent_message_id: MessageId(0),
            last_sent_timestamp: 0,
            voice_members: Vec::new(),
            is_favorite: false,
            creator_id: UserId(0),
        }
    }

    fn make_thread(id: i64, parent_id: i64, cat_id: &str) -> Channel {
        let mut ch = make_channel(id, &id.to_string(), cat_id);
        ch.parent_id = Some(ChannelId(parent_id));
        ch
    }

    fn categories() -> Vec<Category> {
        vec![Category {
            id: "cat1".into(),
            clan_id: ClanId(1),
            name: "General".into(),
            order: 0,
            channels: vec![
                make_channel(10, "alpha", "cat1"),
                make_channel(11, "beta", "cat1"),
            ],
        }]
    }

    #[test]
    fn clan_channel_index_maps_first_occurrence() {
        let favorited = make_channel(10, "alpha", "cat1");
        let cats = vec![
            Category {
                id: FAVOR_CATE_ID.into(),
                clan_id: ClanId(1),
                name: "favoriteChannel".into(),
                order: i32::MIN,
                channels: vec![favorited.clone()],
            },
            Category {
                id: "cat1".into(),
                clan_id: ClanId(1),
                name: "General".into(),
                order: 0,
                channels: vec![make_channel(9, "z", "cat1"), favorited],
            },
        ];
        let index = build_clan_channel_index(&cats);
        assert_eq!(index.get(&ChannelId(10)), Some(&(0, 0)));
        assert_eq!(index.get(&ChannelId(9)), Some(&(1, 0)));
        assert_eq!(index.get(&ChannelId(999)), None);
    }

    #[test]
    fn clan_channel_index_lookup_matches_linear_scan() {
        let cats = categories();
        let index = build_clan_channel_index(&cats);
        for category in &cats {
            for channel in &category.channels {
                let (cat_idx, ch_idx) = index.get(&channel.id).copied().unwrap();
                let linear = cats
                    .iter()
                    .flat_map(|c| &c.channels)
                    .find(|c| c.id == channel.id)
                    .unwrap();
                assert_eq!(cats[cat_idx].channels[ch_idx].id, linear.id);
            }
        }
    }

    #[test]
    fn thread_channel_from_context_builds_thread_under_parent() {
        let thread = thread_channel_from_context(
            ChannelId(500),
            "my-thread".into(),
            ClanId(1),
            ChannelId(10),
        );
        assert_eq!(thread.id, ChannelId(500));
        assert_eq!(thread.channel_type, ChannelType::Thread);
        assert_eq!(thread.parent_id, Some(ChannelId(10)));
        assert_eq!(thread.clan_id, ClanId(1));
        assert!(!thread.private);
    }

    #[test]
    fn synthesized_thread_inserts_nested_after_parent() {
        let mut cats = categories();
        let thread = thread_channel_from_context(
            ChannelId(500),
            "my-thread".into(),
            ClanId(1),
            ChannelId(10),
        );
        assert!(insert_channel(&mut cats, thread));
        let ids: Vec<ChannelId> = cats[0].channels.iter().map(|c| c.id).collect();
        let parent_pos = ids.iter().position(|id| *id == ChannelId(10)).unwrap();
        assert_eq!(ids[parent_pos + 1], ChannelId(500));
        let inserted = cats
            .iter()
            .flat_map(|c| &c.channels)
            .find(|c| c.id == ChannelId(500))
            .unwrap();
        assert_eq!(inserted.channel_type, ChannelType::Thread);
        assert_eq!(inserted.parent_id, Some(ChannelId(10)));
    }

    #[test]
    fn channel_type_from_raw_maps_all_known() {
        assert_eq!(ChannelType::from_raw(1), ChannelType::Text);
        assert_eq!(ChannelType::from_raw(10), ChannelType::Voice);
        assert_eq!(ChannelType::from_raw(6), ChannelType::Stream);
        assert_eq!(ChannelType::from_raw(7), ChannelType::Thread);
        assert_eq!(ChannelType::from_raw(8), ChannelType::App);
        assert_eq!(ChannelType::from_raw(5), ChannelType::Forum);
        assert_eq!(ChannelType::from_raw(9), ChannelType::Announcement);
        assert!(matches!(
            ChannelType::from_raw(99),
            ChannelType::Unknown(99)
        ));
    }

    #[test]
    fn channel_is_unread_uses_badge_count_and_timestamps() {
        let mut ch = make_channel(1, "test", "cat1");
        assert!(!ch.is_unread());
        ch.badge_count = 5;
        assert!(ch.is_unread());
        ch.badge_count = 0;
        ch.last_sent_timestamp = 100;
        ch.last_seen_timestamp = 50;
        assert!(ch.is_unread());
        ch.last_seen_timestamp = 100;
        assert!(!ch.is_unread());
    }

    #[test]
    fn remove_channel_keeps_empty_category() {
        let mut c = categories();
        assert!(remove_channel(&mut c, ChannelId(10)));
        assert_eq!(c[0].channels.len(), 1);
        assert!(remove_channel(&mut c, ChannelId(11)));
        assert_eq!(c.len(), 1);
        assert!(c[0].channels.is_empty());
    }

    #[test]
    fn remove_channel_unknown_is_noop() {
        let mut c = categories();
        assert!(!remove_channel(&mut c, ChannelId(999)));
        assert_eq!(c[0].channels.len(), 2);
    }

    #[test]
    fn update_channel_renames_and_sets_private() {
        let mut c = categories();
        assert!(update_channel(
            &mut c,
            ChannelId(10),
            Some("renamed".into()),
            true
        ));
        assert_eq!(c[0].channels[0].name, "renamed");
        assert!(c[0].channels[0].private);
    }

    #[test]
    fn update_channel_blank_label_keeps_name() {
        let mut c = categories();
        assert!(update_channel(&mut c, ChannelId(11), None, true));
        assert_eq!(c[0].channels[1].name, "beta");
        assert!(c[0].channels[1].private);
    }

    #[test]
    fn update_channel_unknown_is_noop() {
        let mut c = categories();
        assert!(!update_channel(
            &mut c,
            ChannelId(999),
            Some("x".into()),
            true
        ));
    }

    #[test]
    fn build_categories_preserves_api_response_order() {
        let api_cats = vec![
            ApiCategoryDesc {
                category_id: 1,
                category_name: "Bravo".into(),
                clan_id: 1,
                category_order: 2,
            },
            ApiCategoryDesc {
                category_id: 2,
                category_name: "Alpha".into(),
                clan_id: 1,
                category_order: 1,
            },
        ];
        let mut channels = vec![make_channel(10, "ch1", "1"), make_channel(11, "ch2", "2")];
        let cats = build_categories(api_cats, &mut channels);
        assert_eq!(cats[0].name, "Bravo");
        assert_eq!(cats[1].name, "Alpha");
    }

    #[test]
    fn build_categories_emits_empty_categories() {
        let api_cats = vec![
            ApiCategoryDesc {
                category_id: 1,
                category_name: "Has".into(),
                clan_id: 1,
                category_order: 0,
            },
            ApiCategoryDesc {
                category_id: 2,
                category_name: "Empty".into(),
                clan_id: 1,
                category_order: 1,
            },
        ];
        let mut channels = vec![make_channel(10, "ch1", "1")];
        let cats = build_categories(api_cats, &mut channels);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[1].name, "Empty");
        assert!(cats[1].channels.is_empty());
    }

    #[test]
    fn insert_channel_new_category_appends_without_reordering() {
        let mut cats = vec![
            Category {
                id: "a".into(),
                clan_id: ClanId(1),
                name: "A".into(),
                order: 5,
                channels: vec![make_channel(1, "x", "a")],
            },
            Category {
                id: "b".into(),
                clan_id: ClanId(1),
                name: "B".into(),
                order: 2,
                channels: vec![make_channel(2, "y", "b")],
            },
        ];
        assert!(insert_channel(&mut cats, make_channel(3, "z", "new")));
        assert_eq!(cats.len(), 3);
        assert_eq!(cats[0].id, "a");
        assert_eq!(cats[1].id, "b");
        assert_eq!(cats[2].channels[0].id, ChannelId(3));
    }

    #[test]
    fn badge_update_increments_and_mark_read_resets() {
        let mut c = categories();
        let ch = &mut c[0].channels[0];
        ch.badge_count = 3;
        ch.badge_count = ch.badge_count.saturating_add(1);
        assert_eq!(ch.badge_count, 4);
        ch.badge_count = 0;
        assert_eq!(ch.badge_count, 0);
    }

    #[test]
    fn badge_map_excludes_app_and_voice_channel_types() {
        use mezon_client::transport::ApiChannelDesc;

        let make_desc = |id: i64, label: &str, ch_type: u32, badge: i32| ApiChannelDesc {
            channel_id: id,
            channel_label: label.into(),
            channel_type: ch_type,
            clan_id: 1,
            category_name: String::new(),
            category_id: 0,
            channel_private: 0,
            count_mess_unread: 0,
            member_count: 0,
            parent_id: 0,
            is_mute: false,
            last_seen_message_id: 0,
            last_seen_timestamp: 0,
            last_sent_message_id: 0,
            last_sent_timestamp: 0,
            badge_count: badge,
            creator_id: 0,
            clan_name: String::new(),
        };

        let badge_descs = vec![
            make_desc(1, "text_ch", 1, 5),
            make_desc(2, "app_ch", 8, 99),
            make_desc(3, "voice_ch", 10, 77),
        ];

        let badge_map: HashMap<i64, i32> = badge_descs
            .into_iter()
            .filter(|d| {
                !matches!(
                    ChannelType::from_raw(d.channel_type),
                    ChannelType::App | ChannelType::Voice
                )
            })
            .map(|d| (d.channel_id, d.badge_count))
            .collect();

        assert_eq!(badge_map.get(&1), Some(&5));
        assert!(!badge_map.contains_key(&2));
        assert!(!badge_map.contains_key(&3));
    }

    #[test]
    fn voice_join_leave_updates_member_list() {
        let mut c = categories();
        let ch = &mut c[0].channels[0];
        ch.voice_members.push(VoiceMember {
            user_id: UserId(1),
            display_name: "u1".into(),
            avatar_url: String::new(),
        });
        assert!(ch.voice_members.iter().any(|m| m.user_id == UserId(1)));
        ch.voice_members.retain(|m| m.user_id != UserId(1));
        assert!(ch.voice_members.is_empty());
    }

    #[test]
    fn build_categories_threads_nested_after_parent() {
        let api_cats = vec![ApiCategoryDesc {
            category_id: 1,
            category_name: "General".into(),
            clan_id: 1,
            category_order: 0,
        }];
        let mut channels = vec![
            make_channel(20, "parent-b", "1"),
            make_channel(10, "parent-a", "1"),
            make_thread(15, 10, "1"),
            make_thread(25, 20, "1"),
            make_thread(12, 10, "1"),
        ];
        let cats = build_categories(api_cats, &mut channels);
        assert_eq!(cats.len(), 1);
        let ids: Vec<i64> = cats[0].channels.iter().map(|c| c.id.get()).collect();
        assert_eq!(ids, vec![10, 12, 15, 20, 25]);
    }

    #[test]
    fn build_categories_channel_id_ordering_within_category() {
        let api_cats = vec![ApiCategoryDesc {
            category_id: 1,
            category_name: "General".into(),
            clan_id: 1,
            category_order: 0,
        }];
        let mut channels = vec![
            make_channel(30, "z", "1"),
            make_channel(10, "a", "1"),
            make_channel(20, "m", "1"),
        ];
        let cats = build_categories(api_cats, &mut channels);
        let ids: Vec<i64> = cats[0].channels.iter().map(|c| c.id.get()).collect();
        assert_eq!(ids, vec![10, 20, 30]);
    }

    #[test]
    fn build_categories_threads_do_not_appear_as_top_level_siblings() {
        let api_cats = vec![ApiCategoryDesc {
            category_id: 1,
            category_name: "General".into(),
            clan_id: 1,
            category_order: 0,
        }];
        let mut channels = vec![make_channel(10, "parent", "1"), make_thread(11, 10, "1")];
        let cats = build_categories(api_cats, &mut channels);
        let top_level: Vec<i64> = cats[0]
            .channels
            .iter()
            .filter(|ch| ch.parent_id.is_none())
            .map(|c| c.id.get())
            .collect();
        assert_eq!(top_level, vec![10]);
        let thread_row = cats[0]
            .channels
            .iter()
            .find(|c| c.id == ChannelId(11))
            .unwrap();
        assert_eq!(thread_row.parent_id, Some(ChannelId(10)));
    }

    #[test]
    fn favorites_mapping_sets_is_favorite_flag() {
        let ch = Channel {
            id: ChannelId(42),
            name: "fav".into(),
            channel_type: ChannelType::Text,
            private: false,
            clan_id: ClanId(1),
            clan_name: String::new(),
            category_name: "General".into(),
            category_id: Some("c1".into()),
            member_count: 0,
            badge_count: 0,
            muted: false,
            parent_id: None,
            last_seen_message_id: MessageId(0),
            last_seen_timestamp: 0,
            last_sent_message_id: MessageId(0),
            last_sent_timestamp: 0,
            voice_members: Vec::new(),
            is_favorite: true,
            creator_id: UserId(0),
        };
        assert!(ch.is_favorite);
    }

    #[test]
    fn synthetic_favor_cate_is_element_zero() {
        let channels_all = vec![
            Channel {
                id: ChannelId(1),
                name: "general".into(),
                channel_type: ChannelType::Text,
                private: false,
                clan_id: ClanId(1),
                clan_name: String::new(),
                category_name: "Main".into(),
                category_id: Some("cat1".into()),
                member_count: 0,
                badge_count: 0,
                muted: false,
                parent_id: None,
                last_seen_message_id: MessageId(0),
                last_seen_timestamp: 0,
                last_sent_message_id: MessageId(0),
                last_sent_timestamp: 0,
                voice_members: Vec::new(),
                is_favorite: false,
                creator_id: UserId(0),
            },
            Channel {
                id: ChannelId(2),
                name: "fav-ch".into(),
                channel_type: ChannelType::Text,
                private: false,
                clan_id: ClanId(1),
                clan_name: String::new(),
                category_name: "Main".into(),
                category_id: Some("cat1".into()),
                member_count: 0,
                badge_count: 0,
                muted: false,
                parent_id: None,
                last_seen_message_id: MessageId(0),
                last_seen_timestamp: 0,
                last_sent_message_id: MessageId(0),
                last_sent_timestamp: 0,
                voice_members: Vec::new(),
                is_favorite: true,
                creator_id: UserId(0),
            },
        ];

        let favor_channels: Vec<Channel> = channels_all
            .iter()
            .filter(|ch| ch.is_favorite)
            .cloned()
            .collect();

        let mut categories = vec![Category {
            id: "cat1".into(),
            clan_id: ClanId(1),
            name: "Main".into(),
            order: 0,
            channels: channels_all,
        }];

        if !favor_channels.is_empty() {
            categories.insert(
                0,
                Category {
                    id: FAVOR_CATE_ID.into(),
                    clan_id: ClanId(1),
                    name: "favoriteChannel".into(),
                    order: i32::MIN,
                    channels: favor_channels,
                },
            );
        }

        assert_eq!(categories[0].id, FAVOR_CATE_ID);
        assert_eq!(categories[0].channels.len(), 1);
        assert_eq!(categories[0].channels[0].id, ChannelId(2));
        assert_eq!(categories[1].id, "cat1");
    }

    #[test]
    fn voice_member_resolution_fallback_to_user_id() {
        let vm = VoiceMember {
            user_id: UserId(42),
            display_name: "42".into(),
            avatar_url: String::new(),
        };
        assert_eq!(vm.display_name, vm.user_id.to_string());
        assert!(vm.avatar_url.is_empty());
    }

    #[test]
    fn assemble_with_favorites_yields_favor_cate_as_element_zero_on_first_load() {
        let api_cats = vec![ApiCategoryDesc {
            category_id: 1,
            category_name: "General".into(),
            clan_id: 1,
            category_order: 0,
        }];
        let mut channels = vec![
            {
                let mut ch = make_channel(1, "normal", "1");
                ch.clan_id = ClanId(1);
                ch
            },
            {
                let mut ch = make_channel(2, "fav-ch", "1");
                ch.clan_id = ClanId(1);
                ch.is_favorite = true;
                ch
            },
        ];
        let categories = build_categories(api_cats, &mut channels);
        let result = assemble_with_favorites(categories, ClanId(1));

        assert_eq!(result[0].id, FAVOR_CATE_ID);
        assert_eq!(result[0].channels.len(), 1);
        assert_eq!(result[0].channels[0].id, ChannelId(2));
        assert_eq!(result[1].id, "1");
    }

    #[test]
    fn assemble_with_favorites_always_inserts_favor_cate_even_when_empty() {
        let api_cats = vec![ApiCategoryDesc {
            category_id: 1,
            category_name: "General".into(),
            clan_id: 1,
            category_order: 0,
        }];
        let mut channels = vec![make_channel(1, "normal", "1")];
        let categories = build_categories(api_cats, &mut channels);
        let result = assemble_with_favorites(categories, ClanId(1));

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, FAVOR_CATE_ID);
        assert!(result[0].channels.is_empty());
        assert_eq!(result[1].id, "1");
    }

    const PARENT_CHANNEL: ChannelId = ChannelId(10);
    const OTHER_CHANNEL: ChannelId = ChannelId(11);
    const TOPIC: ChannelId = ChannelId(500);

    #[test]
    fn topic_mention_increments_parent_channel_badge_not_the_topic() {
        let mut categories = categories();
        let mut topic_badges = HashMap::new();

        assert!(!add_channel_badge(&mut categories, TOPIC));
        assert!(add_channel_badge(&mut categories, PARENT_CHANNEL));
        record_topic_parent_badge(&mut topic_badges, ClanId(1), PARENT_CHANNEL, TOPIC);

        assert_eq!(categories[0].channels[0].badge_count, 1);
        assert_eq!(categories[0].channels[1].badge_count, 0);
        let tracked = &topic_badges[&TOPIC];
        assert_eq!(tracked.parent_id, PARENT_CHANNEL);
        assert_eq!(tracked.count, 1);
    }

    #[test]
    fn overlay_merge_applies_when_server_badge_zero() {
        let mut categories = categories();
        let mut pending: HashMap<ChannelId, u32> = HashMap::from([(ChannelId(10), 1)]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[0].badge_count, 1);
        assert!(pending.is_empty());
    }

    #[test]
    fn overlay_merge_takes_max_of_server_and_overlay() {
        let mut categories = categories();
        categories[0].channels[0].badge_count = 2;
        let mut pending: HashMap<ChannelId, u32> = HashMap::from([(ChannelId(10), 1)]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[0].badge_count, 2);

        categories[0].channels[1].badge_count = 1;
        let mut pending: HashMap<ChannelId, u32> = HashMap::from([(ChannelId(11), 3)]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[1].badge_count, 3);
    }

    #[test]
    fn overlay_merge_consumes_entry_so_refetch_does_not_readd() {
        let mut categories = categories();
        let mut pending: HashMap<ChannelId, u32> = HashMap::from([(ChannelId(10), 1)]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[0].badge_count, 1);

        categories[0].channels[0].badge_count = 0;
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[0].badge_count, 0);
    }

    #[test]
    fn overlay_merge_keeps_entries_for_channels_in_another_clan() {
        let mut categories = categories();
        let mut pending: HashMap<ChannelId, u32> = HashMap::from([(ChannelId(999), 4)]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(pending.get(&ChannelId(999)), Some(&4));
    }

    #[test]
    fn topic_read_decrements_parent_and_clears_tracking() {
        let mut categories = categories();
        let mut topic_badges = HashMap::new();
        add_channel_badge(&mut categories, PARENT_CHANNEL);
        record_topic_parent_badge(&mut topic_badges, ClanId(1), PARENT_CHANNEL, TOPIC);

        let tracked = topic_badges.remove(&TOPIC).expect("topic tracked");
        let cleared = subtract_channel_badge(&mut categories, tracked.parent_id, tracked.count);

        assert_eq!(cleared, 1);
        assert_eq!(categories[0].channels[0].badge_count, 0);
        assert!(topic_badges.is_empty());
    }

    #[test]
    fn multiple_topic_mentions_accumulate_then_clear_together() {
        let mut categories = categories();
        let mut topic_badges = HashMap::new();
        for _ in 0..3 {
            add_channel_badge(&mut categories, PARENT_CHANNEL);
            record_topic_parent_badge(&mut topic_badges, ClanId(1), PARENT_CHANNEL, TOPIC);
        }
        assert_eq!(categories[0].channels[0].badge_count, 3);
        assert_eq!(topic_badges[&TOPIC].count, 3);

        let tracked = topic_badges.remove(&TOPIC).expect("topic tracked");
        let cleared = subtract_channel_badge(&mut categories, tracked.parent_id, tracked.count);

        assert_eq!(cleared, 3);
        assert_eq!(categories[0].channels[0].badge_count, 0);
    }

    #[test]
    fn subtract_channel_badge_saturates_and_reports_actual_cleared() {
        let mut categories = categories();
        add_channel_badge(&mut categories, PARENT_CHANNEL);
        let cleared = subtract_channel_badge(&mut categories, PARENT_CHANNEL, 5);
        assert_eq!(cleared, 1);
        assert_eq!(categories[0].channels[0].badge_count, 0);
        let missing = subtract_channel_badge(&mut categories, ChannelId(999), 3);
        assert_eq!(missing, 0);
    }

    #[test]
    fn delete_decrements_when_unread_mention_newer_than_last_seen() {
        let mut categories = categories();
        categories[0].channels[0].badge_count = 2;
        categories[0].channels[0].last_seen_timestamp = 100;
        assert!(decrement_channel_badge_on_delete(
            &mut categories,
            PARENT_CHANNEL,
            150
        ));
        assert_eq!(categories[0].channels[0].badge_count, 1);
    }

    #[test]
    fn delete_does_not_decrement_when_badge_zero() {
        let mut categories = categories();
        categories[0].channels[0].badge_count = 0;
        categories[0].channels[0].last_seen_timestamp = 100;
        assert!(!decrement_channel_badge_on_delete(
            &mut categories,
            PARENT_CHANNEL,
            150
        ));
    }

    #[test]
    fn delete_does_not_decrement_when_message_already_seen() {
        let mut categories = categories();
        categories[0].channels[0].badge_count = 2;
        categories[0].channels[0].last_seen_timestamp = 200;
        assert!(!decrement_channel_badge_on_delete(
            &mut categories,
            PARENT_CHANNEL,
            150
        ));
        assert_eq!(categories[0].channels[0].badge_count, 2);
    }

    #[test]
    fn delete_does_not_decrement_when_last_seen_unknown() {
        let mut categories = categories();
        categories[0].channels[0].badge_count = 2;
        categories[0].channels[0].last_seen_timestamp = 0;
        assert!(!decrement_channel_badge_on_delete(
            &mut categories,
            PARENT_CHANNEL,
            150
        ));
        assert_eq!(categories[0].channels[0].badge_count, 2);
    }

    #[test]
    fn delete_decrement_missing_channel_is_false() {
        let mut categories = categories();
        assert!(!decrement_channel_badge_on_delete(
            &mut categories,
            ChannelId(999),
            150
        ));
    }

    #[test]
    fn clear_topic_badges_for_parent_keeps_other_parents() {
        let mut topic_badges = HashMap::new();
        record_topic_parent_badge(&mut topic_badges, ClanId(1), PARENT_CHANNEL, TOPIC);
        record_topic_parent_badge(&mut topic_badges, ClanId(1), OTHER_CHANNEL, ChannelId(501));

        clear_topic_badges_for_parent(&mut topic_badges, PARENT_CHANNEL);

        assert!(!topic_badges.contains_key(&TOPIC));
        assert!(topic_badges.contains_key(&ChannelId(501)));
    }

    #[test]
    fn clear_topic_badges_for_clan_keeps_other_clans() {
        let mut topic_badges = HashMap::new();
        record_topic_parent_badge(&mut topic_badges, ClanId(1), PARENT_CHANNEL, TOPIC);
        record_topic_parent_badge(&mut topic_badges, ClanId(2), ChannelId(20), ChannelId(600));

        clear_topic_badges_for_clan(&mut topic_badges, ClanId(1));

        assert!(!topic_badges.contains_key(&TOPIC));
        assert!(topic_badges.contains_key(&ChannelId(600)));
    }

    #[test]
    fn parent_read_clears_tracking_so_later_topic_read_does_not_overcount() {
        let mut categories = categories();
        let mut topic_badges = HashMap::new();
        add_channel_badge(&mut categories, PARENT_CHANNEL);
        record_topic_parent_badge(&mut topic_badges, ClanId(1), PARENT_CHANNEL, TOPIC);

        categories[0].channels[0].badge_count = 0;
        clear_topic_badges_for_parent(&mut topic_badges, PARENT_CHANNEL);
        assert!(topic_badges.is_empty());

        add_channel_badge(&mut categories, PARENT_CHANNEL);
        add_channel_badge(&mut categories, PARENT_CHANNEL);
        add_channel_badge(&mut categories, PARENT_CHANNEL);
        record_topic_parent_badge(&mut topic_badges, ClanId(1), PARENT_CHANNEL, TOPIC);

        let tracked = topic_badges.remove(&TOPIC).expect("topic tracked");
        let cleared = subtract_channel_badge(&mut categories, tracked.parent_id, tracked.count);

        assert_eq!(cleared, 1);
        assert_eq!(categories[0].channels[0].badge_count, 2);
    }
}
