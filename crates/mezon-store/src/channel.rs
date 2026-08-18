use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use regex::Regex;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::FutureExt as _;
use futures::future::Shared;
use gpui::{
    App, AppContext, BackgroundExecutor, Context, Entity, EventEmitter, Global, Subscription, Task,
};
use mezon_client::transport::{ApiCategoryDesc, ApiChannelDesc, is_channel_limit_api_error};
use mezon_client::{
    ApiChannelApp, AppApi, ChannelAppLaunchParams, ConnectionStatus, RealtimeEvent,
    build_channel_app_url,
};

use crate::KeyedCache;
use crate::badge::BadgeService;
use crate::clan::{ClanEvent, ClanList};
use crate::compose::ComposeStore;
use crate::event_targets_user;
use crate::messages::MessagesStore;
use crate::permissions::{
    PERMISSION_ADMINISTRATOR, PERMISSION_CLAN_OWNER, PERMISSION_MANAGE_CHANNEL,
    PERMISSION_MANAGE_CLAN, PermissionStore,
};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::threads::CHANNEL_TYPE_THREAD;

pub const FAVOR_CATE_ID: &str = "favorCate";
pub const CATEGORY_NAME_MAX_CHARS: usize = 64;
const CATEGORY_EVENT_CREATED: i32 = 1;
const CATEGORY_EVENT_UPDATED: i32 = 2;
const CATEGORY_EVENT_DELETED: i32 = 3;

const PREVIOUS_CHANNELS_PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);
const BADGE_SEED_MAX_ATTEMPTS: u32 = 3;
const BADGE_SEED_RETRY_BACKOFF: Duration = Duration::from_millis(400);
const FAVORITES_PAINT_BUDGET: Duration = Duration::from_millis(1500);
const CHANNEL_DETAIL_MAX_ATTEMPTS: u32 = 3;
const CHANNEL_DETAIL_RETRY_BACKOFF: Duration = Duration::from_secs(1);
const THREAD_ARCHIVE_DURATION_SECONDS: i64 = 7 * 24 * 60 * 60;
const ADDED_THREAD_UNREAD_WINDOW_SECONDS: i64 = 1000;

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
    pub sharing_screen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InVoiceInfo {
    pub clan_id: ClanId,
    pub channel_id: ChannelId,
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

pub const CHANNEL_ACTIVE_ARCHIVED: i32 = 0;
pub const CHANNEL_ACTIVE_JOINED: i32 = 1;
pub const ARCHIVE_ERR_IN_PROGRESS: &str = "archive already in progress";
pub const ARCHIVE_ERR_PERMISSION: &str = "permission denied";
pub const DELETE_ERR_IN_PROGRESS: &str = "delete already in progress";
pub const DELETE_ERR_PERMISSION: &str = "permission denied";
pub const DELETE_ERR_SYSTEM_CHANNEL: &str = "system channel";
pub const STREAM_MODE_THREAD: i32 = 6;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedChannelDesc {
    pub channel_id: i64,
    pub channel_label: String,
    pub channel_private: bool,
    pub last_active_timestamp: Option<i64>,
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
    pub active: i32,
    pub avatar_url: String,
    pub topic: String,
    pub age_restricted: i32,
    pub e2ee: i32,
    pub app_id: i64,
}

impl Channel {
    pub fn is_thread(&self) -> bool {
        self.channel_type == ChannelType::Thread || self.parent_id.is_some()
    }

    pub fn is_unread(&self) -> bool {
        self.badge_count > 0 || self.last_seen_timestamp < self.last_sent_timestamp
    }

    pub fn is_archived(&self) -> bool {
        self.parent_id.is_some() && self.active == CHANNEL_ACTIVE_ARCHIVED
    }

    pub fn visible_in_sidebar(&self) -> bool {
        !self.is_archived()
    }

    /// A voice channel already holding a conversation, which the command
    /// palette and the hashtag suggestions both mark as `(busy)`. One person
    /// sitting in a channel is someone waiting, not a call to interrupt.
    pub fn voice_busy(&self) -> bool {
        self.channel_type == ChannelType::Voice && self.voice_members.len() >= 2
    }
}

pub fn archive_menu_hidden(channel_type: ChannelType, is_welcome_channel: bool) -> bool {
    matches!(
        channel_type,
        ChannelType::Voice | ChannelType::Stream | ChannelType::App
    ) || is_welcome_channel
}

pub fn archive_allowed_by_server(
    is_thread: bool,
    is_creator: bool,
    has_owner: bool,
    has_administrator: bool,
    has_manage_clan: bool,
    has_manage_channel: bool,
) -> bool {
    if is_creator || has_owner || has_administrator {
        return true;
    }
    if is_thread {
        has_manage_channel
    } else {
        has_manage_clan
    }
}

pub fn overview_duplicate_thread_parent_id(channel: &Channel) -> Option<String> {
    if !channel.is_thread() {
        return None;
    }
    channel
        .parent_id
        .filter(|parent| !parent.is_zero())
        .map(|parent| parent.get().to_string())
}

pub fn delete_allowed_by_server(
    is_creator: bool,
    has_owner: bool,
    has_administrator: bool,
    has_manage_clan: bool,
    has_manage_channel: bool,
) -> bool {
    is_creator || has_owner || has_administrator || has_manage_clan || has_manage_channel
}

pub fn can_archive_channel(clan_id: ClanId, channel_id: ChannelId, cx: &App) -> bool {
    ChannelList::global(cx)
        .read(cx)
        .can_archive_channel_for(clan_id, channel_id, cx)
}

pub fn can_delete_channel(clan_id: ClanId, channel_id: ChannelId, cx: &App) -> bool {
    ChannelList::global(cx)
        .read(cx)
        .can_delete_channel_for(clan_id, channel_id, cx)
}

fn archive_permission_for(
    channel_list: &ChannelList,
    clan_id: ClanId,
    channel_id: ChannelId,
    cx: &App,
) -> bool {
    let Some(channel) = channel_list.channel(clan_id, channel_id) else {
        return false;
    };
    let is_welcome = ClanList::global(cx).read(cx).welcome_channel_id(clan_id) == Some(channel_id);
    if archive_menu_hidden(channel.channel_type, is_welcome) {
        return false;
    }
    let is_thread = channel.parent_id.is_some();
    let is_creator = BadgeService::try_global(cx)
        .and_then(|badges| badges.read(cx).current_user_id(cx))
        .is_some_and(|me| me == channel.creator_id);
    let Some(permissions) = PermissionStore::try_global(cx) else {
        return is_creator;
    };
    let permissions = permissions.read(cx);
    archive_allowed_by_server(
        is_thread,
        is_creator,
        permissions.check(clan_id, None, PERMISSION_CLAN_OWNER, cx),
        permissions.check(clan_id, None, PERMISSION_ADMINISTRATOR, cx),
        permissions.check(clan_id, None, PERMISSION_MANAGE_CLAN, cx),
        permissions.check(clan_id, None, PERMISSION_MANAGE_CHANNEL, cx),
    )
}

fn delete_permission_for(
    channel_list: &ChannelList,
    clan_id: ClanId,
    channel_id: ChannelId,
    cx: &App,
) -> bool {
    if ClanList::global(cx).read(cx).welcome_channel_id(clan_id) == Some(channel_id) {
        return false;
    }
    let Some(channel) = channel_list.channel(clan_id, channel_id) else {
        return false;
    };
    let is_creator = BadgeService::try_global(cx)
        .and_then(|badges| badges.read(cx).current_user_id(cx))
        .is_some_and(|me| me == channel.creator_id);
    let Some(permissions) = PermissionStore::try_global(cx) else {
        return is_creator;
    };
    let permissions = permissions.read(cx);
    delete_allowed_by_server(
        is_creator,
        permissions.check(clan_id, None, PERMISSION_CLAN_OWNER, cx),
        permissions.check(clan_id, None, PERMISSION_ADMINISTRATOR, cx),
        permissions.check(clan_id, None, PERMISSION_MANAGE_CLAN, cx),
        permissions.check(clan_id, None, PERMISSION_MANAGE_CHANNEL, cx),
    )
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
    InVoiceChanged,
    ClanChannelsLoaded(ClanId),
    UserChannelsLoaded,
    ArchivedByAdministrator { is_thread: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateCategoryError {
    InvalidName,
    DuplicateName,
    Other(String),
}

static EMOJI_PRESENTATION_CHAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\p{Emoji_Presentation}$").expect("emoji presentation regex"));

fn is_allowed_category_name_char(c: char) -> bool {
    if c == '\'' {
        return false;
    }
    c.is_alphanumeric()
        || matches!(c, '_' | '-' | ' ')
        || EMOJI_PRESENTATION_CHAR.is_match(&c.to_string())
}

pub fn validate_category_name(name: &str) -> Result<String, CreateCategoryError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > CATEGORY_NAME_MAX_CHARS {
        return Err(CreateCategoryError::InvalidName);
    }
    let mut chars = trimmed.chars();
    let first = chars.next().expect("non-empty");
    if matches!(first, '_' | '-' | ' ') || !is_allowed_category_name_char(first) {
        return Err(CreateCategoryError::InvalidName);
    }
    if !chars.all(is_allowed_category_name_char) {
        return Err(CreateCategoryError::InvalidName);
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateChannelError {
    InvalidName,
    DuplicateName,
    ChannelLimitExceeded,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateChannelOverviewError {
    InvalidName,
    DuplicateName,
    Other(String),
}

pub const MAX_CHANNEL_TOPIC_CHARS: usize = 1024;

pub fn validate_channel_name(name: &str) -> Result<String, CreateChannelError> {
    validate_category_name(name).map_err(|err| match err {
        CreateCategoryError::InvalidName => CreateChannelError::InvalidName,
        CreateCategoryError::DuplicateName => CreateChannelError::DuplicateName,
        CreateCategoryError::Other(msg) => CreateChannelError::Other(msg),
    })
}

fn map_create_channel_api_error(err: anyhow::Error) -> CreateChannelError {
    if is_channel_limit_api_error(&err) {
        return CreateChannelError::ChannelLimitExceeded;
    }
    CreateChannelError::Other(err.to_string())
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

struct ClanExtras {
    voice_map: Option<HashMap<ChannelId, Vec<VoiceMember>>>,
    app_channels: Option<Vec<AppChannel>>,
}

impl ClanExtras {
    fn is_complete(&self) -> bool {
        self.voice_map.is_some() && self.app_channels.is_some()
    }
}

#[derive(Debug, Clone, Copy)]
struct ChannelUnreadSeed {
    badge_count: u32,
    last_seen_timestamp: i64,
    last_seen_message_id: MessageId,
    last_sent_timestamp: i64,
    last_sent_message_id: MessageId,
}

#[derive(Default, Clone, Copy)]
struct PendingBadge {
    count: u32,
    last_sent_timestamp: i64,
    last_sent_message_id: MessageId,
}

pub struct ChannelList {
    cache: KeyedCache<ClanId, Vec<Category>>,
    app_channels_cache: HashMap<ClanId, Vec<AppChannel>>,
    favorites: HashMap<ClanId, HashSet<ChannelId>>,
    topic_parent_badges: HashMap<ChannelId, TopicParentBadge>,
    pending_channel_badges: HashMap<ChannelId, PendingBadge>,
    pending_badge_seed: HashMap<ClanId, HashMap<ChannelId, ChannelUnreadSeed>>,
    want_extras: HashSet<ClanId>,
    extras_loaded: HashSet<ClanId>,
    extras_loading: HashSet<ClanId>,
    badge_seeding: HashSet<ClanId>,
    badge_seeded: HashSet<ClanId>,
    forgotten_clans: HashSet<ClanId>,
    user_channels: HashMap<ChannelId, Channel>,
    user_channels_order: Vec<ChannelId>,
    user_channels_loading: bool,
    in_voice: HashMap<UserId, InVoiceInfo>,
    user_channels_loaded: bool,
    loading: HashMap<ClanId, Shared<Task<()>>>,
    active_clan_id: Option<ClanId>,
    pub active_channel_id: Option<ChannelId>,
    remembered_channels: HashMap<ClanId, ChannelId>,
    previous_channels: HashMap<ClanId, Vec<ChannelId>>,
    api: Arc<AppApi>,
    collapsed: HashSet<(String, String)>,
    show_empty_categories: HashSet<ClanId>,
    channel_index: RefCell<ChannelLocationCache>,
    reset_generation: u64,
    reactivating: HashSet<ChannelId>,
    archiving: HashSet<ChannelId>,
    deleting: HashSet<ChannelId>,
    archived_channel_ids: HashSet<ChannelId>,
    archived_cascade_children: HashMap<ChannelId, Vec<ChannelId>>,
    archived_channel_parents: HashMap<ChannelId, ChannelId>,
    deleted_channel_ids: HashSet<ChannelId>,
    deleted_channel_parents: HashMap<ChannelId, ChannelId>,
    channel_detail_pending: HashSet<ChannelId>,
    channel_detail_failed: HashSet<ChannelId>,
    _previous_channels_persist: Task<()>,
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

    pub fn can_archive_channel_for(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &App,
    ) -> bool {
        archive_permission_for(self, clan_id, channel_id, cx)
    }

    pub fn can_delete_channel_for(&self, clan_id: ClanId, channel_id: ChannelId, cx: &App) -> bool {
        delete_permission_for(self, clan_id, channel_id, cx)
    }

    pub fn fetch_channel_app_url(
        &self,
        app_id: i64,
        app_url: String,
        clan_id: ClanId,
        clan_name: String,
        cx: &mut Context<Self>,
    ) -> Task<Option<String>> {
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            let hash = match api.generate_hash_channel_apps(app_id).await {
                Ok(hash) => hash,
                Err(error) => {
                    tracing::warn!("generate_hash_channel_apps failed: {error:#}");
                    return None;
                }
            };
            if hash.web_app_data.is_empty() {
                tracing::warn!("generate_hash_channel_apps returned empty web_app_data");
                return None;
            }
            match build_channel_app_url(
                &app_url,
                ChannelAppLaunchParams {
                    web_app_data: &hash.web_app_data,
                    clan_id: &clan_id.0.to_string(),
                    clan_name: Some(&clan_name),
                },
            ) {
                Ok(url) => Some(url),
                Err(error) => {
                    tracing::warn!("build_channel_app_url failed: {error:#}");
                    None
                }
            }
        })
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.cache.clear();
        self.app_channels_cache.clear();
        self.favorites.clear();
        self.topic_parent_badges.clear();
        self.pending_channel_badges.clear();
        self.pending_badge_seed.clear();
        self.want_extras.clear();
        self.extras_loaded.clear();
        self.extras_loading.clear();
        self.badge_seeding.clear();
        self.badge_seeded.clear();
        self.forgotten_clans.clear();
        self.user_channels.clear();
        self.user_channels_order.clear();
        self.user_channels_loading = false;
        self.in_voice.clear();
        self.user_channels_loaded = false;
        self.loading.clear();
        self.show_empty_categories.clear();
        self.remembered_channels.clear();
        self.previous_channels.clear();
        self.persist_previous_channels(cx);
        self.invalidate_channel_index_all();
        self.reactivating.clear();
        self.archiving.clear();
        self.deleting.clear();
        self.archived_channel_ids.clear();
        self.archived_cascade_children.clear();
        self.archived_channel_parents.clear();
        self.deleted_channel_ids.clear();
        self.deleted_channel_parents.clear();
        self.channel_detail_pending.clear();
        self.channel_detail_failed.clear();
        self.active_clan_id = None;
        if self.active_channel_id.take().is_some() {
            cx.emit(ChannelEvent::ActiveChannelChanged(None));
        }
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let clan_sub = cx.subscribe(
            &ClanList::global(cx),
            |this, _clan, event, cx| match event {
                ClanEvent::ActiveClanChanged(active) => match active {
                    Some(clan_id) => {
                        this.active_clan_id = Some(*clan_id);
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
                },
                ClanEvent::Deleted(clan_id) => this.forget_clan(*clan_id, cx),
            },
        );

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        cx.spawn(async move |this, cx| {
            let (collapsed, previous_channels) = cx
                .background_executor()
                .spawn(async { (load_collapse_state(), load_previous_channels()) })
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
            favorites: HashMap::new(),
            topic_parent_badges: HashMap::new(),
            pending_channel_badges: HashMap::new(),
            pending_badge_seed: HashMap::new(),
            want_extras: HashSet::new(),
            extras_loaded: HashSet::new(),
            extras_loading: HashSet::new(),
            badge_seeding: HashSet::new(),
            badge_seeded: HashSet::new(),
            forgotten_clans: HashSet::new(),
            user_channels: HashMap::new(),
            user_channels_order: Vec::new(),
            user_channels_loading: false,
            in_voice: HashMap::new(),
            user_channels_loaded: false,
            loading: HashMap::new(),
            active_clan_id: None,
            active_channel_id: None,
            remembered_channels: HashMap::new(),
            previous_channels: HashMap::new(),
            api,
            collapsed: HashSet::new(),
            show_empty_categories: HashSet::new(),
            channel_index: RefCell::new(ChannelLocationCache::default()),
            reset_generation: 0,
            reactivating: HashSet::new(),
            archiving: HashSet::new(),
            deleting: HashSet::new(),
            archived_channel_ids: HashSet::new(),
            archived_cascade_children: HashMap::new(),
            archived_channel_parents: HashMap::new(),
            deleted_channel_ids: HashSet::new(),
            deleted_channel_parents: HashMap::new(),
            channel_detail_pending: HashSet::new(),
            channel_detail_failed: HashSet::new(),
            _previous_channels_persist: Task::ready(()),
            _clan_sub: clan_sub,
            _conn_watch: conn_watch,
        }
    }

    pub fn forget_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.forgotten_clans.insert(clan_id);
        let channel_ids: Vec<ChannelId> = self
            .cache
            .get(&clan_id)
            .map(|categories| {
                categories
                    .iter()
                    .flat_map(|category| category.channels.iter().map(|channel| channel.id))
                    .collect()
            })
            .unwrap_or_default();

        self.cache.remove(&clan_id);
        self.app_channels_cache.remove(&clan_id);
        self.favorites.remove(&clan_id);
        self.pending_badge_seed.remove(&clan_id);
        self.want_extras.remove(&clan_id);
        self.extras_loaded.remove(&clan_id);
        self.extras_loading.remove(&clan_id);
        self.badge_seeding.remove(&clan_id);
        self.badge_seeded.remove(&clan_id);
        self.loading.remove(&clan_id);
        self.show_empty_categories.remove(&clan_id);
        self.remembered_channels.remove(&clan_id);
        if self.previous_channels.remove(&clan_id).is_some() {
            self.persist_previous_channels(cx);
        }

        for channel_id in &channel_ids {
            self.user_channels.remove(channel_id);
            self.topic_parent_badges.remove(channel_id);
            self.pending_channel_badges.remove(channel_id);
            self.reactivating.remove(channel_id);
            self.archiving.remove(channel_id);
            self.deleting.remove(channel_id);
            self.archived_channel_ids.remove(channel_id);
            self.archived_cascade_children.remove(channel_id);
            self.archived_channel_parents.remove(channel_id);
            self.deleted_channel_ids.remove(channel_id);
            self.deleted_channel_parents.remove(channel_id);
            self.channel_detail_pending.remove(channel_id);
            self.channel_detail_failed.remove(channel_id);
        }

        self.user_channels_order
            .retain(|channel_id| self.user_channels.contains_key(channel_id));
        self.in_voice.retain(|_, info| info.clan_id != clan_id);

        self.invalidate_channel_index(clan_id);
        if self.active_clan_id == Some(clan_id) {
            self.active_clan_id = None;
        }
        if self
            .active_channel_id
            .is_some_and(|active| channel_ids.contains(&active))
        {
            self.active_channel_id = None;
            cx.emit(ChannelEvent::ActiveChannelChanged(None));
        }
        cx.notify();
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

    fn consume_badge_seed(&mut self, clan_id: ClanId, categories: &mut [Category]) {
        if let Some(seed) = self.pending_badge_seed.get(&clan_id) {
            let applied = apply_unread_seed_into(seed, categories);
            tracing::debug!(
                target: "clan_load",
                clan_id = clan_id.get(),
                seeded = seed.len(),
                applied,
                "consumed parked badge seed on structure insert"
            );
        }
    }

    fn drop_seeded_badge(&mut self, clan_id: ClanId, channel_id: ChannelId) {
        if let Some(seed) = self.pending_badge_seed.get_mut(&clan_id) {
            seed.remove(&channel_id);
        }
    }

    pub fn cancel_badge_seed(&mut self, clan_id: ClanId) {
        self.badge_seeding.remove(&clan_id);
    }

    pub fn seed_badges(&mut self, clan_id: ClanId, cx: &mut Context<Self>) -> Task<()> {
        if self.forgotten_clans.contains(&clan_id)
            || self.badge_seeded.contains(&clan_id)
            || !self.badge_seeding.insert(clan_id)
        {
            return Task::ready(());
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        let structure_ready = self.clan_structure_ready(clan_id);
        cx.spawn(async move |this, cx| {
            structure_ready.await;
            let still_current = this
                .update(cx, |this, _| {
                    this.reset_generation == generation && !this.forgotten_clans.contains(&clan_id)
                })
                .unwrap_or(false);
            if !still_current {
                return;
            }
            if let Err(e) = api.join_clan_chat(clan_id.get()).await {
                tracing::error!(target: "clan_load", "join_clan_chat failed for clan {clan_id}: {e}");
            }
            let mut attempt = 0u32;
            let descs = loop {
                match api.list_channel_badge_counts(clan_id.get()).await {
                    Ok(descs) => break Some(descs),
                    Err(e) => {
                        attempt += 1;
                        if attempt >= BADGE_SEED_MAX_ATTEMPTS {
                            tracing::warn!(
                                "list_channel_badge_counts failed for clan {clan_id} after {attempt} attempts: {e}"
                            );
                            break None;
                        }
                        tracing::warn!(
                            "list_channel_badge_counts failed for clan {clan_id} (attempt {attempt}): {e}, retrying"
                        );
                        cx.background_executor()
                            .timer(BADGE_SEED_RETRY_BACKOFF * attempt)
                            .await;
                    }
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.badge_seeding.remove(&clan_id);
                if this.reset_generation != generation {
                    return;
                }
                let Some(descs) = descs else {
                    return;
                };
                this.badge_seeded.insert(clan_id);
                let desc_count = descs.len();
                let seed = unread_seed_from_descs(descs);
                let last_messages: Vec<(ChannelId, MessageId)> = seed
                    .iter()
                    .filter(|(_, s)| !s.last_sent_message_id.is_zero())
                    .map(|(id, s)| (*id, s.last_sent_message_id))
                    .collect();
                let applied = match this.cache.get_mut(&clan_id) {
                    Some(categories) => apply_unread_seed_into(&seed, categories),
                    None => false,
                };
                if tracing::enabled!(target: "clan_load", tracing::Level::DEBUG) {
                    let badged_rows: Vec<String> = seed
                        .iter()
                        .filter(|(_, s)| s.badge_count > 0)
                        .map(|(id, _)| match this.channel(clan_id, *id) {
                            Some(ch) => format!(
                                "{}#{} type={:?} badge={} muted={} thread={}",
                                ch.name,
                                id.get(),
                                ch.channel_type,
                                ch.badge_count,
                                ch.muted,
                                ch.parent_id.is_some()
                            ),
                            None => format!("MISSING#{}", id.get()),
                        })
                        .collect();
                    tracing::debug!(
                        target: "clan_load",
                        clan_id = clan_id.get(),
                        desc_count,
                        seeded = seed.len(),
                        with_badge = badged_rows.len(),
                        cached = this.cache.contains(&clan_id),
                        applied,
                        active = this.active_clan_id == Some(clan_id),
                        rows = ?badged_rows,
                        "seeded badge counts"
                    );
                }
                this.pending_badge_seed.insert(clan_id, seed);
                if applied {
                    this.notify_channel_list(clan_id, cx);
                }
                if !last_messages.is_empty()
                    && let Some(store) = MessagesStore::try_global(cx)
                {
                    store.update(cx, |store, cx| {
                        store.set_many_last_messages(last_messages);
                        cx.notify();
                    });
                }
            });
        })
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ChannelCreated,
                RealtimeKind::ChannelUpdated,
                RealtimeKind::ChannelDeleted,
                RealtimeKind::CategoryEvent,
                RealtimeKind::VoiceJoined,
                RealtimeKind::VoiceLeaved,
                RealtimeKind::ScreenShare,
                RealtimeKind::UserChannelAdded,
                RealtimeKind::UserChannelRemoved,
                RealtimeKind::ChannelArchive,
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
        self.want_extras.insert(clan_id);
        if self.cache.is_fresh(&clan_id, crate::CACHE_TTL) {
            self.ensure_extras(clan_id, cx);
            return;
        }
        self.fetch_clan(clan_id, cx);
    }

    pub fn load_structure_for_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.cache.is_fresh(&clan_id, crate::CACHE_TTL) {
            return;
        }
        self.fetch_clan(clan_id, cx);
    }

    fn ensure_extras(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.extras_loaded.contains(&clan_id) || !self.extras_loading.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let extras = Self::fetch_clan_extras(&api, clan_id).await;
            let _ = this.update(cx, |this, cx| {
                this.extras_loading.remove(&clan_id);
                if this.reset_generation != generation {
                    return;
                }
                this.finish_extras(clan_id, extras, cx);
            });
        })
        .detach();
    }

    fn finish_extras(&mut self, clan_id: ClanId, extras: ClanExtras, cx: &mut Context<Self>) {
        let complete = extras.is_complete();
        let applied = self.apply_clan_extras(clan_id, extras, cx);
        if applied && complete {
            self.extras_loaded.insert(clan_id);
        }
    }

    pub fn refresh_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.want_extras.insert(clan_id);
        self.extras_loaded.remove(&clan_id);
        self.fetch_clan(clan_id, cx);
    }

    pub fn fetch_archived_channels(
        &self,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<ArchivedChannelDesc>, String>> {
        let api = self.api.clone();
        let id = clan_id.get();
        cx.spawn(async move |_, _| {
            let descs = api
                .list_archived_channel_descs(id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(descs
                .into_iter()
                .map(|d| ArchivedChannelDesc {
                    channel_id: d.channel_id,
                    channel_label: d.channel_label,
                    channel_private: d.channel_private != 0,
                    last_active_timestamp: d
                        .last_sent_message
                        .filter(|m| m.timestamp_seconds > 0)
                        .map(|m| i64::from(m.timestamp_seconds)),
                })
                .collect())
        })
    }

    pub fn restore_archived_channel(
        &self,
        clan_id: ClanId,
        channel_id: i64,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        let clan_id_raw = clan_id.get();
        cx.spawn(async move |_, _| {
            api.restore_archived_channel(clan_id_raw, channel_id)
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn archive_channel(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        cx.spawn(async move |this, cx| {
            let prep: Result<(ChannelId, Arc<AppApi>), String> = this
                .update(cx, |this, cx| {
                    if !this.can_archive_channel_for(clan_id, channel_id, cx) {
                        return Err(ARCHIVE_ERR_PERMISSION.into());
                    }
                    if !this.begin_archiving(channel_id) {
                        return Err(ARCHIVE_ERR_IN_PROGRESS.into());
                    }
                    let parent_id = this
                        .channel(clan_id, channel_id)
                        .and_then(|ch| ch.parent_id)
                        .unwrap_or(ChannelId(0));
                    Ok((parent_id, this.api.clone()))
                })
                .unwrap_or_else(|_| Err("archive channel store unavailable".into()));
            let (parent_id, api) = match prep {
                Ok(value) => value,
                Err(err) => return Err(err),
            };

            let result = api
                .archive_channel(clan_id.get(), channel_id.get())
                .await
                .map_err(|e| e.to_string());

            if result.is_ok() {
                let _ = this.update(cx, |this, cx| {
                    this.apply_local_archive(clan_id, channel_id, parent_id, cx);
                });
            }

            let _ = this.update(cx, |this, _| {
                this.finish_archiving(channel_id);
            });

            result
        })
    }

    pub fn delete_channel(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        cx.spawn(async move |this, cx| {
            let prep: Result<(ChannelId, Arc<AppApi>), String> = this
                .update(cx, |this, cx| {
                    if ClanList::global(cx).read(cx).welcome_channel_id(clan_id) == Some(channel_id)
                    {
                        return Err(DELETE_ERR_SYSTEM_CHANNEL.into());
                    }
                    if !this.can_delete_channel_for(clan_id, channel_id, cx) {
                        return Err(DELETE_ERR_PERMISSION.into());
                    }
                    if !this.begin_deleting(channel_id) {
                        return Err(DELETE_ERR_IN_PROGRESS.into());
                    }
                    let parent_id = this
                        .channel(clan_id, channel_id)
                        .and_then(|ch| ch.parent_id)
                        .unwrap_or(ChannelId(0));
                    Ok((parent_id, this.api.clone()))
                })
                .unwrap_or_else(|_| Err("delete channel store unavailable".into()));
            let (parent_id, api) = match prep {
                Ok(value) => value,
                Err(err) => return Err(err),
            };

            let result = api
                .delete_channel(clan_id.get(), channel_id.get())
                .await
                .map_err(|e| e.to_string());

            if result.is_ok() {
                let _ = this.update(cx, |this, cx| {
                    this.apply_local_delete(clan_id, channel_id, parent_id, cx);
                });
            }

            let _ = this.update(cx, |this, _| {
                this.finish_deleting(channel_id);
            });

            result
        })
    }

    #[cfg(test)]
    pub(crate) fn seed_clan_channels_for_test(
        &mut self,
        clan_id: ClanId,
        categories: Vec<Category>,
    ) {
        self.active_clan_id = Some(clan_id);
        self.cache.insert(clan_id, categories, None);
        self.invalidate_channel_index(clan_id);
    }

    fn apply_clan_structure(
        &mut self,
        clan_id: ClanId,
        mut categories: Vec<Category>,
        favorite_ids: Option<HashSet<ChannelId>>,
        cx: &mut Context<Self>,
    ) {
        if self.forgotten_clans.contains(&clan_id) {
            return;
        }
        let favorites_missing = favorite_ids.is_none();
        let favorites_len = favorite_ids.as_ref().map(HashSet::len);
        if let Some(favorite_ids) = favorite_ids {
            self.favorites.insert(clan_id, favorite_ids);
        }
        self.consume_badge_seed(clan_id, &mut categories);
        self.merge_pending_badges(&mut categories);
        let previous_badges = collect_channel_badges(&self.cache, clan_id);
        carry_live_channel_badges(&mut categories, &previous_badges);
        tracing::debug!(
            target: "badge_flow",
            clan = clan_id.get(),
            carried_channels = previous_badges.len(),
            carried_badged = previous_badges.values().filter(|s| s.badge_count > 0).count(),
            "apply_clan_structure carried live badges"
        );
        let previous_voice = collect_voice_members(&self.cache, clan_id);
        merge_previous_voice_members(&mut categories, &previous_voice);
        let mut categories = rebuild_favorites(categories, clan_id, self.favorites.get(&clan_id));
        for cat in categories.iter_mut() {
            cat.channels.retain(|ch| {
                !self.archived_channel_ids.contains(&ch.id)
                    && !self.deleted_channel_ids.contains(&ch.id)
            });
        }
        if seed_in_voice_from_categories(&mut self.in_voice, clan_id, &categories) {
            cx.emit(ChannelEvent::InVoiceChanged);
        }
        self.sync_user_channels_from_clan_structure(&categories, cx);
        self.cache.insert(clan_id, categories, None);
        if favorites_missing {
            self.cache.mark_stale(&clan_id);
        }
        tracing::debug!(
            target: "clan_load",
            clan_id = clan_id.get(),
            favorites = favorites_len,
            stale = favorites_missing,
            "clan structure applied"
        );
        self.invalidate_channel_index(clan_id);
        self.channel_detail_failed.clear();
        self.sync_clan_after_read(clan_id, 0, cx);
        cx.emit(ChannelEvent::ClanChannelsLoaded(clan_id));
        cx.notify();
        if self.want_extras.contains(&clan_id) {
            self.ensure_extras(clan_id, cx);
        }
    }

    fn fetch_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.loading.contains_key(&clan_id) {
            return;
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        let executor = cx.background_executor().clone();
        let task = cx
            .spawn(async move |this, cx| {
                let result = Self::fetch_clan_structure(&api, clan_id, executor).await;
                match result {
                    Ok((categories, favorite_ids)) => {
                        let _ = this.update(cx, |this, cx| {
                            if this.reset_generation != generation {
                                return;
                            }
                            this.apply_clan_structure(clan_id, categories, favorite_ids, cx);
                            this.loading.remove(&clan_id);
                        });
                    }
                    Err(e) => {
                        tracing::error!("Failed to load channels for clan {clan_id}: {e}");
                        let _ = this.update(cx, |this, cx| {
                            if this.reset_generation != generation {
                                return;
                            }
                            this.loading.remove(&clan_id);
                            cx.notify();
                        });
                    }
                }
            })
            .shared();
        self.loading.insert(clan_id, task);
    }

    fn clan_structure_ready(&self, clan_id: ClanId) -> Shared<Task<()>> {
        self.loading
            .get(&clan_id)
            .cloned()
            .unwrap_or_else(|| Task::ready(()).shared())
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
                        this.merge_user_channels_from_api_descs(descs, cx);
                        this.user_channels_loaded = true;
                        cx.emit(ChannelEvent::UserChannelsLoaded);
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

    pub fn in_voice_status(&self, user_id: UserId) -> Option<InVoiceInfo> {
        self.in_voice.get(&user_id).copied()
    }

    pub fn user_channels(&self) -> impl Iterator<Item = &Channel> + '_ {
        self.user_channels_order
            .iter()
            .filter_map(|id| self.user_channels.get(id))
            .filter(|channel| !channel.is_archived())
    }

    pub fn ensure_user_channels_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.user_channels_loaded && !self.user_channels_loading {
            self.fetch_user_channels(cx);
        }
    }

    pub fn is_locally_archived(&self, channel_id: ChannelId) -> bool {
        self.archived_channel_ids.contains(&channel_id)
    }

    pub fn is_locally_deleted(&self, channel_id: ChannelId) -> bool {
        self.deleted_channel_ids.contains(&channel_id)
    }

    pub fn deleted_channel_parent(&self, channel_id: ChannelId) -> Option<ChannelId> {
        self.deleted_channel_parents.get(&channel_id).copied()
    }

    pub fn is_resolving_channel_detail(&self, channel_id: ChannelId) -> bool {
        self.channel_detail_pending.contains(&channel_id)
    }

    /// Mirrors mezon-react's `addThreadToChannels` (channelLoader): when a route targets a
    /// channel/thread absent from the clan structure, fetch its detail and insert it. Returns
    /// `true` while the channel is present or still being resolved (caller should wait), and
    /// `false` once the fetch has definitively failed (caller may fall back).
    pub fn ensure_channel_in_clan(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_locally_archived(channel_id) || self.is_locally_deleted(channel_id) {
            return false;
        }
        if self.channel_in_clan(clan_id, channel_id) {
            return true;
        }
        if !self.cache.contains(&clan_id) {
            return true;
        }
        if self.channel_detail_failed.remove(&channel_id) {
            return false;
        }
        if !self.channel_detail_pending.insert(channel_id) {
            return true;
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let mut attempt = 0u32;
            let desc = loop {
                match api.list_channel_detail(channel_id.get()).await {
                    Ok(desc) => break Some(desc),
                    Err(e) => {
                        attempt += 1;
                        if attempt >= CHANNEL_DETAIL_MAX_ATTEMPTS {
                            tracing::warn!(
                                "list_channel_detail failed for {channel_id} after {attempt} attempts: {e}"
                            );
                            break None;
                        }
                        cx.background_executor()
                            .timer(CHANNEL_DETAIL_RETRY_BACKOFF * attempt)
                            .await;
                    }
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.channel_detail_pending.remove(&channel_id);
                if this.reset_generation != generation {
                    return;
                }
                match desc {
                    Some(desc) => this.apply_channel_detail(clan_id, desc, cx),
                    None => {
                        this.channel_detail_failed.insert(channel_id);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        true
    }

    fn apply_channel_detail(
        &mut self,
        clan_id: ClanId,
        desc: ApiChannelDesc,
        cx: &mut Context<Self>,
    ) {
        let channel_id = ChannelId(desc.channel_id);
        if self.is_locally_archived(channel_id) || self.is_locally_deleted(channel_id) {
            return;
        }
        let badge = desc.badge_count.max(0) as u32;
        let mut channel = channel_from_desc(desc, badge, Vec::new(), false);
        if !channel.visible_in_sidebar() {
            self.channel_detail_failed.insert(channel_id);
            return;
        }
        channel.clan_id = clan_id;
        let Some(categories) = self.cache.get_mut(&clan_id) else {
            return;
        };
        if insert_channel(categories, channel) {
            self.invalidate_channel_index(clan_id);
            cx.emit(ChannelEvent::ClanChannelsLoaded(clan_id));
            cx.notify();
        }
    }

    async fn fetch_clan_structure(
        api: &AppApi,
        clan_id: ClanId,
        executor: BackgroundExecutor,
    ) -> anyhow::Result<(Vec<Category>, Option<HashSet<ChannelId>>)> {
        let (channels_res, categories_res, favorite_ids) = tokio::join!(
            api.list_channel_descs(clan_id.get(), 1),
            api.list_categories_typed(clan_id.get()),
            fetch_favorites_within_paint_budget(api, clan_id, &executor),
        );

        let api_channels = channels_res?;
        let api_categories = categories_res?;

        let mut channels: Vec<Channel> = api_channels
            .into_iter()
            .map(|c| {
                let badge = c.badge_count.max(0) as u32;
                channel_from_desc(c, badge, Vec::new(), false)
            })
            .collect();

        let omitted_active = channels
            .iter()
            .filter(|ch| ch.parent_id.is_none() && ch.active == CHANNEL_ACTIVE_ARCHIVED)
            .count();
        if omitted_active > 0 {
            tracing::info!(
                target: "socket",
                "list_channel_descs: clan={} channels={} with_active_unset={omitted_active}",
                clan_id,
                channels.len()
            );
        }

        Ok((
            build_categories(api_categories, &mut channels),
            favorite_ids,
        ))
    }

    async fn fetch_clan_extras(api: &AppApi, clan_id: ClanId) -> ClanExtras {
        let (voice_res, apps_res) = tokio::join!(
            api.list_voice_channel_users(clan_id.get()),
            api.list_channel_apps(clan_id.get()),
        );

        let voice_users = voice_res
            .inspect_err(|e| tracing::warn!("list_voice_channel_users failed: {e}"))
            .ok();
        let app_channels: Option<Vec<AppChannel>> = apps_res
            .inspect_err(|e| tracing::warn!("list_channel_apps failed: {e}"))
            .ok()
            .map(|apps| apps.into_iter().map(AppChannel::from).collect());

        let voice_map: Option<HashMap<ChannelId, Vec<VoiceMember>>> = voice_users.map(|users| {
            users
                .into_iter()
                .map(|v| {
                    let sharing: HashSet<UserId> =
                        v.share_screen_ids.into_iter().map(UserId).collect();
                    let members = v
                        .user_ids
                        .into_iter()
                        .map(|uid| {
                            let user_id = UserId(uid);
                            VoiceMember {
                                user_id,
                                display_name: user_id.to_string(),
                                avatar_url: String::new(),
                                sharing_screen: sharing.contains(&user_id),
                            }
                        })
                        .collect();
                    (ChannelId(v.channel_id), members)
                })
                .collect()
        });

        ClanExtras {
            voice_map,
            app_channels,
        }
    }

    fn apply_clan_extras(
        &mut self,
        clan_id: ClanId,
        extras: ClanExtras,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut app_channels_changed = false;
        if let Some(app_channels) = extras.app_channels {
            app_channels_changed = self.app_channels_cache.get(&clan_id) != Some(&app_channels);
            self.app_channels_cache.insert(clan_id, app_channels);
        }

        let Some(slot) = self.cache.get_mut(&clan_id) else {
            cx.notify();
            return false;
        };

        let mut owned = std::mem::take(slot);
        owned.retain(|category| category.id != FAVOR_CATE_ID);

        let mut changed = app_channels_changed;
        if let Some(voice_map) = extras.voice_map.as_ref() {
            for ch in owned
                .iter_mut()
                .flat_map(|category| category.channels.iter_mut())
            {
                let members = voice_map.get(&ch.id).cloned().unwrap_or_default();
                if ch.voice_members != members {
                    ch.voice_members = members;
                    changed = true;
                }
            }
        }

        let rebuilt = rebuild_favorites(owned, clan_id, self.favorites.get(&clan_id));
        let in_voice_changed = seed_in_voice_from_categories(&mut self.in_voice, clan_id, &rebuilt);
        if let Some(slot) = self.cache.get_mut(&clan_id) {
            *slot = rebuilt;
        }
        self.invalidate_channel_index(clan_id);

        if in_voice_changed {
            cx.emit(ChannelEvent::InVoiceChanged);
        }
        tracing::debug!(
            target: "clan_load",
            clan_id = clan_id.get(),
            voice_channels = extras.voice_map.as_ref().map(HashMap::len),
            changed,
            "extras patched into cached clan structure"
        );
        if changed || in_voice_changed {
            cx.notify();
        }
        true
    }

    fn notify_channel_list(&self, clan_id: ClanId, cx: &mut Context<Self>) {
        if self.active_clan_id == Some(clan_id) {
            cx.notify();
        }
    }

    fn apply_category_upsert(
        &mut self,
        clan_id: ClanId,
        mut category: Category,
        cx: &mut Context<Self>,
    ) {
        if category.clan_id.is_zero() {
            category.clan_id = clan_id;
        }
        if !self.cache.contains(&clan_id) {
            return;
        }
        let category_id = category.id.clone();
        let category_name = category.name.clone();
        let changed = self
            .cache
            .get_mut(&clan_id)
            .is_some_and(|categories| upsert_category(categories, category));
        let names_changed = self.sync_channel_category_names(clan_id, &category_id, &category_name);
        if changed || names_changed {
            self.invalidate_channel_index(clan_id);
            self.notify_channel_list(clan_id, cx);
        }
    }

    fn sync_channel_category_names(
        &mut self,
        clan_id: ClanId,
        category_id: &str,
        name: &str,
    ) -> bool {
        let mut changed = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for channel in categories
                .iter_mut()
                .flat_map(|category| category.channels.iter_mut())
                .filter(|channel| channel.category_id.as_deref() == Some(category_id))
            {
                if channel.category_name != name {
                    channel.category_name = name.to_string();
                    changed = true;
                }
            }
        }
        changed
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
                .filter(|ch| ch.visible_in_sidebar())
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
                .filter(|ch| ch.visible_in_sidebar())
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
        tracing::debug!(target: "badge_flow", clan = clan_id.get(), "apply_mark_as_read_clan");
        let mut cleared_badge = 0;
        let mut should_notify = false;
        self.pending_badge_seed.remove(&clan_id);
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for category in categories.iter_mut() {
                for ch in &category.channels {
                    should_notify = should_notify || ch.is_unread();
                }
                cleared_badge += Self::mark_channels_read(&mut category.channels);
            }
        }
        self.drop_pending_badges_for_clan(clan_id);
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
        tracing::debug!(
            target: "badge_flow",
            clan = clan_id.get(),
            category = category_id,
            "apply_mark_as_read_category"
        );
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
        for &channel_id in &parent_ids {
            self.drop_seeded_badge(clan_id, channel_id);
            self.pending_channel_badges.remove(&channel_id);
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
                if ts > ch.last_sent_timestamp {
                    ch.last_sent_timestamp = ts;
                    if !message_id.is_zero() {
                        ch.last_sent_message_id = message_id;
                    }
                }
                if seen && ts > ch.last_seen_timestamp {
                    ch.last_seen_timestamp = ts;
                    if !message_id.is_zero() {
                        ch.last_seen_message_id = message_id;
                    }
                }
                let counts_badge = !matches!(
                    ch.channel_type,
                    ChannelType::App | ChannelType::Voice | ChannelType::Stream
                );
                if is_mention && !seen && counts_badge {
                    ch.badge_count = ch.badge_count.saturating_add(1);
                }
                visible_changed =
                    visible_changed || was_unread != ch.is_unread() || was_badge != ch.badge_count;
                updated_channel = Some(ch.clone());
            }
        }
        if !found {
            if !seen && (is_mention || ts > 0) {
                let overlay = self.pending_channel_badges.entry(channel_id).or_default();
                if is_mention {
                    overlay.count += 1;
                }
                if ts > overlay.last_sent_timestamp {
                    overlay.last_sent_timestamp = ts;
                    if !message_id.is_zero() {
                        overlay.last_sent_message_id = message_id;
                    }
                }
            }
            self.patch_user_channel_message(channel_id, is_mention, seen, ts, message_id, cx);
        } else if let Some(channel) = updated_channel {
            self.sync_user_channel_from(&channel, cx);
        }
        tracing::debug!(
            target: "badge_flow",
            clan = clan_id.get(),
            channel = channel_id.get(),
            is_mention,
            seen,
            ts,
            found,
            visible_changed,
            badge = self.channel_badge_count(clan_id, channel_id),
            "note_channel_message applied"
        );
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
            .unwrap_or_default();
        (
            channel.badge_count.max(pending.count),
            channel.last_sent_timestamp.max(pending.last_sent_timestamp),
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

    fn merge_user_channels_from_api_descs(
        &mut self,
        descs: Vec<mezon_client::transport::ApiChannelDesc>,
        cx: &mut Context<Self>,
    ) {
        self.user_channels.clear();
        self.user_channels_order.clear();
        for d in descs {
            let channel = channel_from_desc(d, 0, Vec::new(), false);
            upsert_user_channel(
                &mut self.user_channels,
                &mut self.user_channels_order,
                channel,
            );
        }
        self.sync_user_channels_from_all_loaded_caches(cx);
    }

    fn sync_user_channels_from_all_loaded_caches(&mut self, _cx: &mut Context<Self>) {
        let clan_ids: Vec<ClanId> = self.cache.iter().map(|(id, _)| *id).collect();
        for clan_id in clan_ids {
            let batch: Vec<Channel> = self
                .cache
                .get(&clan_id)
                .into_iter()
                .flat_map(|categories| categories.iter())
                .flat_map(|category| category.channels.iter())
                .filter(|channel| should_sync_channel_to_user_list(channel))
                .cloned()
                .collect();
            for channel in batch {
                upsert_user_channel(
                    &mut self.user_channels,
                    &mut self.user_channels_order,
                    channel,
                );
            }
        }
    }

    fn sync_user_channels_from_clan_structure(
        &mut self,
        categories: &[Category],
        _cx: &mut Context<Self>,
    ) {
        for channel in categories
            .iter()
            .flat_map(|category| category.channels.iter())
            .filter(|channel| should_sync_channel_to_user_list(channel))
        {
            upsert_user_channel(
                &mut self.user_channels,
                &mut self.user_channels_order,
                channel.clone(),
            );
        }
    }

    fn upsert_user_channel_from_cache(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = self.channel(clan_id, channel_id).cloned() else {
            return;
        };
        if !should_sync_channel_to_user_list(&channel) {
            return;
        }
        if upsert_user_channel(
            &mut self.user_channels,
            &mut self.user_channels_order,
            channel,
        ) {
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
        self.drop_seeded_badge(clan_id, channel_id);
        if !found {
            cleared_badge = overlaid.map(|o| o.count).unwrap_or(0);
        }
        tracing::debug!(
            target: "badge_flow",
            clan = clan_id.get(),
            channel = channel_id.get(),
            found,
            cleared_badge,
            "apply_read"
        );
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
        self.pending_badge_seed.remove(&clan_id);
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
        self.drop_pending_badges_for_clan(clan_id);
        if changed {
            self.notify_channel_list(clan_id, cx);
        }
    }

    fn drop_pending_badges_for_clan(&mut self, clan_id: ClanId) {
        let Some(categories) = self.cache.get(&clan_id) else {
            return;
        };
        let ids: Vec<ChannelId> = categories
            .iter()
            .flat_map(|category| &category.channels)
            .map(|ch| ch.id)
            .collect();
        for id in ids {
            self.pending_channel_badges.remove(&id);
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

    fn apply_channel_read_with_threads(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let thread_ids = self
            .cache
            .get(&clan_id)
            .map(|categories| thread_ids_of(categories, channel_id))
            .unwrap_or_default();
        self.apply_read(clan_id, channel_id, cx);
        for thread_id in thread_ids {
            self.apply_read(clan_id, thread_id, cx);
        }
    }

    pub fn mark_channel_as_read(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        if channel_id.is_zero() {
            return;
        }
        let category_id = self
            .cache
            .get(&clan_id)
            .map(|categories| mark_as_read_category_id(categories, channel_id))
            .unwrap_or(0);
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .mark_as_read(channel_id.get(), category_id, clan_id.get())
                .await
            {
                tracing::error!("mark channel as read failed for channel {channel_id}: {e}");
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.apply_channel_read_with_threads(clan_id, channel_id, cx);
            });
        })
        .detach();
    }

    pub fn mark_category_as_read(
        &mut self,
        clan_id: ClanId,
        category_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Ok(category_id) = category_id.parse::<i64>() else {
            return;
        };
        if category_id == 0 {
            return;
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            if let Err(e) = api.mark_as_read(0, category_id, clan_id.get()).await {
                tracing::error!("mark category as read failed for category {category_id}: {e}");
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.apply_mark_as_read_category(clan_id, category_id, cx);
            });
        })
        .detach();
    }

    pub fn collapse_all_categories(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let Some(categories) = self.cache.get(&clan_id) else {
            return;
        };
        let clan_key = clan_id.to_string();
        let keys: Vec<(String, String)> = categories
            .iter()
            .map(|category| (clan_key.clone(), category.id.clone()))
            .collect();
        let mut changed = false;
        for key in keys {
            changed |= self.collapsed.insert(key);
        }
        if !changed {
            return;
        }
        cx.notify();
        let snapshot: Vec<(String, String)> = self.collapsed.iter().cloned().collect();
        cx.background_executor()
            .spawn(async move { save_collapse_state(snapshot) })
            .detach();
    }

    pub fn category_name_exists(&self, clan_id: ClanId, name: &str) -> bool {
        self.category_name_exists_excluding(clan_id, name, "")
    }

    pub fn category_name_exists_excluding(
        &self,
        clan_id: ClanId,
        name: &str,
        exclude_id: &str,
    ) -> bool {
        let normalized = name.trim().to_lowercase();
        self.cache.get(&clan_id).is_some_and(|categories| {
            categories.iter().any(|category| {
                category.id != FAVOR_CATE_ID
                    && category.id != exclude_id
                    && category.name.trim().to_lowercase() == normalized
            })
        })
    }

    pub fn category_name(&self, clan_id: ClanId, category_id: &str) -> Option<&str> {
        self.cache.get(&clan_id).and_then(|categories| {
            categories
                .iter()
                .find(|category| category.id == category_id)
                .map(|category| category.name.as_str())
        })
    }

    pub fn rename_category(
        &mut self,
        clan_id: ClanId,
        category_id: String,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CreateCategoryError>> {
        let trimmed = match validate_category_name(&name) {
            Ok(trimmed) => trimmed,
            Err(err) => return Task::ready(Err(err)),
        };
        if self.category_name_exists_excluding(clan_id, &trimmed, &category_id) {
            return Task::ready(Err(CreateCategoryError::DuplicateName));
        }
        let Ok(category_id_num) = category_id.parse::<i64>() else {
            return Task::ready(Err(CreateCategoryError::Other(
                "category is not editable".into(),
            )));
        };
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            api.update_category(clan_id.get(), category_id_num, &trimmed)
                .await
                .map_err(|e| CreateCategoryError::Other(e.to_string()))?;
            this.update(cx, |this, cx| {
                this.apply_category_rename(clan_id, &category_id, trimmed, cx);
            })
            .map_err(|_| CreateCategoryError::Other("store dropped".into()))?;
            Ok(())
        })
    }

    fn apply_category_rename(
        &mut self,
        clan_id: ClanId,
        category_id: &str,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for category in categories
                .iter_mut()
                .filter(|category| category.id == category_id)
            {
                if category.name != name {
                    category.name = name.clone();
                    changed = true;
                }
            }
        }
        changed |= self.sync_channel_category_names(clan_id, category_id, &name);
        if changed {
            self.notify_channel_list(clan_id, cx);
        }
    }

    pub fn delete_category(
        &self,
        clan_id: ClanId,
        category_id: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let category_id_num = category_id
                .parse::<i64>()
                .map_err(|_| "category is not deletable".to_string())?;
            api.delete_category(clan_id.get(), category_id_num)
                .await
                .map_err(|e| e.to_string())?;
            let _ = this.update(cx, |this, cx| {
                this.apply_category_removed(clan_id, &category_id, cx);
            });
            Ok(())
        })
    }

    fn apply_category_removed(
        &mut self,
        clan_id: ClanId,
        category_id: &str,
        cx: &mut Context<Self>,
    ) {
        if category_id == FAVOR_CATE_ID {
            return;
        }
        let doomed: Vec<(ChannelId, ChannelId)> = self
            .cache
            .get(&clan_id)
            .into_iter()
            .flatten()
            .filter(|category| category.id == category_id)
            .flat_map(|category| category.channels.iter())
            .map(|ch| (ch.id, ch.parent_id.unwrap_or(ChannelId(0))))
            .collect();
        for (channel_id, parent_id) in doomed {
            self.apply_local_delete(clan_id, channel_id, parent_id, cx);
        }
        let mut removed = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            let before = categories.len();
            categories.retain(|category| category.id != category_id);
            removed = categories.len() != before;
        }
        self.collapsed
            .remove(&(clan_id.to_string(), category_id.to_string()));
        if removed {
            self.invalidate_channel_index(clan_id);
            self.notify_channel_list(clan_id, cx);
        }
    }

    pub fn update_categories_order(
        &mut self,
        clan_id: ClanId,
        category_ids: &[i64],
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        if category_ids.is_empty() {
            return Task::ready(Err("no categories to reorder".into()));
        }
        let payload: Vec<(i32, i64)> = category_ids
            .iter()
            .enumerate()
            .map(|(index, category_id)| ((index + 1) as i32, *category_id))
            .collect();
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for (index, category_id) in category_ids.iter().enumerate() {
                let order = (index + 1) as i32;
                let id = category_id.to_string();
                if let Some(category) = categories.iter_mut().find(|c| c.id == id) {
                    category.order = order;
                }
            }
            categories.sort_by_key(|c| {
                if c.id == FAVOR_CATE_ID {
                    i32::MIN
                } else {
                    c.order
                }
            });
            cx.notify();
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            match api.update_category_order(clan_id.get(), &payload).await {
                Ok(()) => this
                    .update(cx, |this, cx| {
                        this.refresh_clan(clan_id, cx);
                    })
                    .map_err(|_| "store dropped".to_string()),
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.refresh_clan(clan_id, cx);
                    });
                    Err(error.to_string())
                }
            }
        })
    }

    pub fn create_category(
        &mut self,
        clan_id: ClanId,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CreateCategoryError>> {
        let trimmed = match validate_category_name(&name) {
            Ok(trimmed) => trimmed,
            Err(err) => return Task::ready(Err(err)),
        };
        if self.category_name_exists(clan_id, &trimmed) {
            return Task::ready(Err(CreateCategoryError::DuplicateName));
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let desc = api
                .create_category(clan_id.get(), &trimmed)
                .await
                .map_err(|e| CreateCategoryError::Other(e.to_string()))?;
            let category = category_from_desc(desc);
            this.update(cx, |this, cx| {
                this.apply_category_upsert(clan_id, category, cx);
            })
            .map_err(|_| CreateCategoryError::Other("store dropped".into()))?;
            Ok(())
        })
    }

    pub fn channel_name_exists_in_category(
        &self,
        clan_id: ClanId,
        category_id: &str,
        name: &str,
    ) -> bool {
        self.cache.get(&clan_id).is_some_and(|categories| {
            channel_name_exists_in_categories(categories, category_id, name)
        })
    }

    pub fn create_channel(
        &mut self,
        clan_id: ClanId,
        category_id: String,
        name: String,
        channel_type: ChannelType,
        private: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(ChannelId, ChannelType), CreateChannelError>> {
        let label = match validate_channel_name(&name) {
            Ok(label) => label,
            Err(err) => return Task::ready(Err(err)),
        };
        if self.channel_name_exists_in_category(clan_id, &category_id, &label) {
            return Task::ready(Err(CreateChannelError::DuplicateName));
        }
        let channel_private = if private && channel_type == ChannelType::Text {
            1
        } else {
            0
        };
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if api
                .check_duplicate_channel_name(&label, &category_id)
                .await
                .map_err(|e| CreateChannelError::Other(e.to_string()))?
            {
                return Err(CreateChannelError::DuplicateName);
            }
            let category_id_num = category_id.parse::<i64>().ok();
            let mut desc = api
                .create_channel(
                    clan_id.get(),
                    &label,
                    channel_type.as_raw(),
                    category_id_num,
                    None,
                    channel_private,
                )
                .await
                .map_err(map_create_channel_api_error)?;
            desc.category_id = effective_category_id(desc.category_id, category_id_num);
            let channel_id = ChannelId(desc.channel_id);
            let created_type = ChannelType::from_raw(desc.channel_type);
            this.update(cx, |this, cx| {
                if let Some(categories) = this.cache.get_mut(&clan_id) {
                    let channel = channel_from_desc(desc, 0, Vec::new(), false);
                    if insert_channel(categories, channel) {
                        this.invalidate_channel_index(clan_id);
                        cx.notify();
                    }
                }
            })
            .map_err(|_| CreateChannelError::Other("store dropped".into()))?;
            Ok((channel_id, created_type))
        })
    }

    pub fn change_channel_category(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        new_category_id: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        if new_category_id.is_empty() || new_category_id == FAVOR_CATE_ID {
            return Task::ready(Err("invalid category".into()));
        }
        let Some(channel) = self.channel(clan_id, channel_id).cloned() else {
            return Task::ready(Err("channel not found".into()));
        };
        if channel.category_id.as_deref() == Some(new_category_id.as_str()) {
            return Task::ready(Ok(()));
        }
        let Some(new_category_name) = self
            .categories_for_clan(clan_id)
            .iter()
            .find(|category| category.id == new_category_id)
            .map(|category| category.name.clone())
        else {
            return Task::ready(Err("category not found".into()));
        };
        let Ok(new_category_id_num) = new_category_id.parse::<i64>() else {
            return Task::ready(Err("invalid category id".into()));
        };

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            api.change_channel_category(clan_id.get(), channel_id.get(), new_category_id_num)
                .await
                .map_err(|e| e.to_string())?;

            this.update(cx, |this, cx| {
                let favorites = this.favorites.get(&clan_id).cloned();
                if let Some(mut categories) = this.cache.remove(&clan_id) {
                    if move_channel_to_category(
                        &mut categories,
                        channel_id,
                        &new_category_id,
                        &new_category_name,
                    ) {
                        let rebuilt = rebuild_favorites(categories, clan_id, favorites.as_ref());
                        this.cache.insert(clan_id, rebuilt, None);
                        this.invalidate_channel_index(clan_id);
                        cx.notify();
                    } else {
                        this.cache.insert(clan_id, categories, None);
                    }
                }
            })
            .map_err(|_| "store dropped".to_string())?;

            Ok(())
        })
    }

    pub fn patch_channel_overview_detail(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        topic: String,
        age_restricted: i32,
        e2ee: i32,
        app_id: i64,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = self.channel(clan_id, channel_id).cloned() else {
            return;
        };
        let mut changed = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            changed = update_channel(
                categories,
                channel_id,
                None,
                Some(topic),
                Some(age_restricted),
                channel.private,
            );
            if changed {
                for cat in categories.iter_mut() {
                    for ch in cat.channels.iter_mut() {
                        if ch.id == channel_id {
                            ch.e2ee = e2ee;
                            ch.app_id = app_id;
                        }
                    }
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub fn update_channel_overview(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        label: String,
        topic: String,
        age_restricted: i32,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), UpdateChannelOverviewError>> {
        let validated = match validate_channel_name(&label) {
            Ok(label) => label,
            Err(CreateChannelError::InvalidName) => {
                return Task::ready(Err(UpdateChannelOverviewError::InvalidName));
            }
            Err(CreateChannelError::DuplicateName) => {
                return Task::ready(Err(UpdateChannelOverviewError::DuplicateName));
            }
            Err(CreateChannelError::ChannelLimitExceeded) => {
                return Task::ready(Err(UpdateChannelOverviewError::Other(
                    "channel limit exceeded".into(),
                )));
            }
            Err(CreateChannelError::Other(msg)) => {
                return Task::ready(Err(UpdateChannelOverviewError::Other(msg)));
            }
        };
        let topic = crate::truncate_chars(&topic, MAX_CHANNEL_TOPIC_CHARS);

        let Some(channel) = self.channel(clan_id, channel_id).cloned() else {
            return Task::ready(Err(UpdateChannelOverviewError::Other(
                "channel not found".into(),
            )));
        };

        if channel.name == validated
            && channel.topic == topic
            && channel.age_restricted == age_restricted
        {
            return Task::ready(Ok(()));
        }

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if channel.name != validated {
                let duplicate = if let Some(parent) = overview_duplicate_thread_parent_id(&channel)
                {
                    api.check_duplicate_thread_name(&validated, &parent)
                        .await
                        .map_err(|e| UpdateChannelOverviewError::Other(e.to_string()))?
                } else {
                    let category_id = channel.category_id.clone().unwrap_or_default();
                    api.check_duplicate_channel_name(&validated, &category_id)
                        .await
                        .map_err(|e| UpdateChannelOverviewError::Other(e.to_string()))?
                };
                if duplicate {
                    return Err(UpdateChannelOverviewError::DuplicateName);
                }
            }

            let category_id = channel
                .category_id
                .as_deref()
                .and_then(|id| id.parse().ok())
                .unwrap_or(0);

            let params = mezon_client::UpdateChannelDescParams {
                channel_label: Some(validated.clone()),
                category_id,
                topic: topic.clone(),
                age_restricted,
                e2ee: channel.e2ee,
                app_id: channel.app_id,
                channel_avatar: None,
            };

            api.update_channel_desc(clan_id.get(), channel_id.get(), params)
                .await
                .map_err(|e| UpdateChannelOverviewError::Other(e.to_string()))?;

            this.update(cx, |this, cx| {
                if let Some(categories) = this.cache.get_mut(&clan_id)
                    && update_channel(
                        categories,
                        channel_id,
                        Some(validated),
                        Some(topic),
                        Some(age_restricted),
                        channel.private,
                    )
                {
                    this.invalidate_channel_index(clan_id);
                    cx.notify();
                }
            })
            .map_err(|_| UpdateChannelOverviewError::Other("store dropped".into()))?;

            Ok(())
        })
    }

    pub fn is_show_empty_category(&self, clan_id: ClanId) -> bool {
        !self.show_empty_categories.contains(&clan_id)
    }

    pub fn set_show_empty_category(&mut self, clan_id: ClanId, show: bool, cx: &mut Context<Self>) {
        if show {
            self.show_empty_categories.remove(&clan_id);
        } else {
            self.show_empty_categories.insert(clan_id);
        }
        cx.notify();
    }

    pub fn muted(&self, clan_id: ClanId, channel_id: ChannelId) -> bool {
        self.channel(clan_id, channel_id)
            .map(|ch| ch.muted)
            .unwrap_or(false)
    }

    pub fn set_channel_muted(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        muted: bool,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        if let Some(categories) = self.cache.get_mut(&clan_id) {
            for ch in categories
                .iter_mut()
                .flat_map(|c| c.channels.iter_mut())
                .filter(|ch| ch.id == channel_id)
            {
                if ch.muted != muted {
                    ch.muted = muted;
                    changed = true;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub fn set_channel_muted_any_clan(
        &mut self,
        channel_id: ChannelId,
        muted: bool,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for categories in self.cache.values_mut() {
            for ch in categories
                .iter_mut()
                .flat_map(|c| c.channels.iter_mut())
                .filter(|ch| ch.id == channel_id)
            {
                if ch.muted != muted {
                    ch.muted = muted;
                    changed = true;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub fn apply_last_seen(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cleared_badge: u32,
        seen_ts: i64,
        seen_message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!(
            target: "badge_flow",
            clan = clan_id.get(),
            channel = channel_id.get(),
            cleared_badge,
            seen_ts,
            "apply_last_seen (realtime LastSeenUpdated)"
        );
        self.pending_channel_badges.remove(&channel_id);
        self.drop_seeded_badge(clan_id, channel_id);
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
                ch.badge_count = 0;
                if seen_ts > ch.last_seen_timestamp {
                    ch.last_seen_timestamp = seen_ts;
                }
                if !seen_message_id.is_zero() {
                    ch.last_seen_message_id = seen_message_id;
                }
                if !computed {
                    computed = true;
                    visible_changed = was_unread != ch.is_unread() || was_badge != ch.badge_count;
                    badge_delta = was_badge;
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
                self.pending_channel_badges
                    .entry(parent_id)
                    .or_default()
                    .count += 1;
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
            RealtimeEvent::CategoryEvent(e) => {
                if e.id == 0 {
                    return;
                }
                let clan_id = ClanId(e.clan_id);
                match e.status {
                    CATEGORY_EVENT_DELETED => {
                        self.apply_category_removed(clan_id, &e.id.to_string(), cx);
                    }
                    CATEGORY_EVENT_CREATED | CATEGORY_EVENT_UPDATED => {
                        let name = e.category_name.trim();
                        if name.is_empty() {
                            return;
                        }
                        let id = e.id.to_string();
                        let order = self
                            .cache
                            .get(&clan_id)
                            .and_then(|categories| {
                                categories
                                    .iter()
                                    .find(|category| category.id == id)
                                    .map(|category| category.order)
                            })
                            .unwrap_or(0);
                        let category = Category {
                            id,
                            clan_id,
                            name: name.to_string(),
                            order,
                            channels: Vec::new(),
                        };
                        self.apply_category_upsert(clan_id, category, cx);
                    }
                    _ => {}
                }
            }
            RealtimeEvent::ChannelCreated(e) => {
                let clan_id = ClanId(e.clan_id);
                let channel_id = ChannelId(e.channel_id);
                if self.is_locally_archived(channel_id) {
                    return;
                }
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
                        active: CHANNEL_ACTIVE_JOINED,
                        avatar_url: String::new(),
                        topic: String::new(),
                        age_restricted: 0,
                        e2ee: 0,
                        app_id: 0,
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
                let topic = (!e.topic.is_empty()).then_some(e.topic.clone());
                let age_restricted = (!e.topic.is_empty()).then_some(e.age_restricted);
                let mut changed = false;
                for cats in self.cache.values_mut() {
                    if update_channel(
                        cats,
                        id,
                        label.clone(),
                        topic.clone(),
                        age_restricted,
                        e.channel_private,
                    ) {
                        changed = true;
                        break;
                    }
                }
                if changed {
                    cx.notify();
                }
            }
            RealtimeEvent::ChannelDeleted(e) => {
                let clan_id = ClanId(e.clan_id);
                let channel_id = ChannelId(e.channel_id);
                let parent_id = ChannelId(e.parent_id);
                self.apply_local_delete(clan_id, channel_id, parent_id, cx);
            }
            RealtimeEvent::VoiceJoined(e) => {
                let clan_id = ClanId(e.clan_id);
                let channel_id = ChannelId(e.voice_channel_id);
                let user_id = UserId(e.user_id);
                let member = VoiceMember {
                    user_id,
                    display_name: e.participant.clone(),
                    avatar_url: String::new(),
                    sharing_screen: false,
                };
                let clan_cached = self.cache.contains(&clan_id);
                let mut channel_found = false;
                let mut changed = false;
                if let Some(cats) = self.cache.get_mut(&clan_id) {
                    for ch in cats
                        .iter_mut()
                        .flat_map(|c| c.channels.iter_mut())
                        .filter(|ch| ch.id == channel_id)
                    {
                        channel_found = true;
                        if !ch.voice_members.iter().any(|m| m.user_id == user_id) {
                            ch.voice_members.push(member.clone());
                            changed = true;
                        }
                    }
                }
                tracing::debug!(
                    %clan_id,
                    %channel_id,
                    %user_id,
                    clan_cached,
                    channel_found,
                    added = changed,
                    "realtime VoiceJoined"
                );
                let in_voice_changed = apply_in_voice_joined(
                    &mut self.in_voice,
                    user_id,
                    InVoiceInfo {
                        clan_id,
                        channel_id,
                    },
                );
                notify_in_voice_change(changed, in_voice_changed, cx);
            }
            RealtimeEvent::VoiceLeaved(e) => {
                let clan_id = ClanId(e.clan_id);
                let channel_id = ChannelId(e.voice_channel_id);
                let user_id = UserId(e.voice_user_id);
                let clan_cached = self.cache.contains(&clan_id);
                let mut channel_found = false;
                let mut changed = false;
                if let Some(cats) = self.cache.get_mut(&clan_id) {
                    for ch in cats
                        .iter_mut()
                        .flat_map(|c| c.channels.iter_mut())
                        .filter(|ch| ch.id == channel_id)
                    {
                        channel_found = true;
                        let before = ch.voice_members.len();
                        ch.voice_members.retain(|m| m.user_id != user_id);
                        if ch.voice_members.len() != before {
                            changed = true;
                        }
                    }
                }
                tracing::debug!(
                    %clan_id,
                    %channel_id,
                    %user_id,
                    clan_cached,
                    channel_found,
                    removed = changed,
                    "realtime VoiceLeaved"
                );
                let in_voice_changed =
                    apply_in_voice_leaved(&mut self.in_voice, user_id, channel_id);
                notify_in_voice_change(changed, in_voice_changed, cx);
            }
            RealtimeEvent::ScreenShare(e) => {
                let clan_id = ClanId(e.clan_id);
                let channel_id = ChannelId(e.voice_channel_id);
                let user_id = UserId(e.user_id);
                let is_sharing = e.is_sharing;
                let mut changed = false;
                if let Some(cats) = self.cache.get_mut(&clan_id) {
                    for ch in cats
                        .iter_mut()
                        .flat_map(|c| c.channels.iter_mut())
                        .filter(|ch| ch.id == channel_id)
                    {
                        if let Some(member) =
                            ch.voice_members.iter_mut().find(|m| m.user_id == user_id)
                            && member.sharing_screen != is_sharing
                        {
                            member.sharing_screen = is_sharing;
                            changed = true;
                        }
                    }
                }
                tracing::debug!(
                    %clan_id,
                    %channel_id,
                    %user_id,
                    is_sharing,
                    updated = changed,
                    "realtime ScreenShare"
                );
                if changed {
                    cx.notify();
                }
            }
            RealtimeEvent::UserChannelAdded(e) => {
                let Some(ref desc) = e.channel_desc else {
                    return;
                };
                let channel_id = ChannelId(desc.channel_id);
                if self.is_locally_archived(channel_id) {
                    return;
                }
                let channel_type = desc.r#type as u32;
                if channel_type == 2 || channel_type == 3 {
                    return;
                }
                let me = BadgeService::try_global(cx)
                    .and_then(|badges| badges.read(cx).current_user_id(cx));
                let added_ids = e.users.iter().map(|u| u.user_id).collect::<Vec<_>>();
                if !event_targets_user(&added_ids, me) {
                    return;
                }
                let clan_id = ClanId(e.clan_id);
                let last_sent = desc.last_sent_message.as_ref();
                let (last_sent_message_id, last_sent_timestamp) = match last_sent {
                    Some(header) => (
                        MessageId(header.id),
                        i64::from(header.timestamp_seconds).max(0),
                    ),
                    None => (MessageId(0), 0),
                };
                let is_thread = channel_type == CHANNEL_TYPE_THREAD;
                let (last_sent_timestamp, last_seen_timestamp) = if is_thread {
                    seed_added_thread_unread(last_sent_timestamp)
                } else {
                    let seen = desc
                        .last_seen_message
                        .as_ref()
                        .map(|header| i64::from(header.timestamp_seconds).max(0))
                        .unwrap_or(0);
                    (last_sent_timestamp, seen)
                };
                let channel = Channel {
                    id: channel_id,
                    name: desc.channel_label.clone(),
                    channel_type: ChannelType::from_raw(channel_type),
                    private: desc.channel_private != 0,
                    clan_id,
                    clan_name: desc.clan_name.clone(),
                    category_name: desc.category_name.clone(),
                    category_id: Some(desc.category_id.to_string())
                        .filter(|s| !s.is_empty() && s != "0"),
                    member_count: desc.member_count.max(0) as u32,
                    badge_count: 0,
                    muted: desc.is_mute,
                    parent_id: Some(ChannelId(desc.parent_id)).filter(|c| !c.is_zero()),
                    last_seen_message_id: desc
                        .last_seen_message
                        .as_ref()
                        .map(|header| MessageId(header.id))
                        .unwrap_or(MessageId(0)),
                    last_seen_timestamp,
                    last_sent_message_id,
                    last_sent_timestamp,
                    voice_members: Vec::new(),
                    is_favorite: false,
                    creator_id: UserId(desc.creator_id),
                    active: CHANNEL_ACTIVE_JOINED,
                    avatar_url: desc.channel_avatar.clone(),
                    topic: desc.topic.clone(),
                    age_restricted: desc.age_restricted,
                    e2ee: desc.e2ee,
                    app_id: desc.app_id,
                };
                let inserted = self
                    .cache
                    .get_mut(&clan_id)
                    .is_some_and(|cats| insert_channel(cats, channel.clone()));
                let listed = upsert_user_channel(
                    &mut self.user_channels,
                    &mut self.user_channels_order,
                    channel,
                );
                if inserted {
                    self.invalidate_channel_index(clan_id);
                }
                if inserted || listed {
                    cx.notify();
                }
            }
            RealtimeEvent::UserChannelRemoved(e) => {
                let channel_id = ChannelId(e.channel_id);
                let channel_type = e.channel_type as u32;
                if channel_type == 2 || channel_type == 3 {
                    return;
                }
                let me = BadgeService::try_global(cx)
                    .and_then(|badges| badges.read(cx).current_user_id(cx));
                if !event_targets_user(&e.user_ids, me) {
                    return;
                }
                self.apply_self_removed_from_channel(channel_id, cx);
            }
            RealtimeEvent::ChannelArchive(e) => {
                self.apply_channel_archive_event(e, cx);
            }
            _ => {}
        }
    }

    fn resync(&mut self, cx: &mut Context<Self>) {
        tracing::info!("ChannelList resync — invalidating channel cache");
        self.cache.mark_all_stale();
        self.invalidate_channel_index_all();
        self.badge_seeded.clear();
        self.extras_loaded.clear();
        self.fetch_user_channels(cx);
        if let Some(clan_id) = self.active_clan_id {
            self.load_for_clan(clan_id, cx);
            self.seed_badges(clan_id, cx).detach();
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
        self.loading.contains_key(&clan_id)
    }

    pub fn app_channels_for_clan(&self, clan_id: ClanId) -> &[AppChannel] {
        self.app_channels_cache
            .get(&clan_id)
            .map_or(&[], Vec::as_slice)
    }

    pub fn app_channel_for_id(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
    ) -> Option<&AppChannel> {
        self.app_channels_cache
            .get(&clan_id)?
            .iter()
            .find(|app| app.channel_id == channel_id)
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
        self.schedule_persist_previous_channels(cx);
    }

    fn schedule_persist_previous_channels(&mut self, cx: &mut Context<Self>) {
        self._previous_channels_persist = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(PREVIOUS_CHANNELS_PERSIST_DEBOUNCE)
                .await;
            let snapshot = this
                .update(cx, |this, _cx| this.previous_channels.clone())
                .ok();
            if let Some(snapshot) = snapshot {
                cx.background_executor()
                    .spawn(async move { save_previous_channels(snapshot) })
                    .await;
            }
        });
    }

    fn persist_previous_channels(&mut self, cx: &mut Context<Self>) {
        self._previous_channels_persist = Task::ready(());
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
        tracing::debug!(target: "badge_flow", channel = id.get(), "select_channel (no badge mutation)");
        self.active_channel_id = Some(id);
        if let Some(clan_id) = self.clan_id_for_channel(id) {
            self.remembered_channels.insert(clan_id, id);
        }
        cx.emit(ChannelEvent::ActiveChannelChanged(self.active_channel_id));
        cx.notify();
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
        self.ensure_thread_channel_with_active(thread_id, label, CHANNEL_ACTIVE_JOINED, false, cx)
    }

    pub fn ensure_thread_channel_with_active(
        &mut self,
        thread_id: ChannelId,
        label: String,
        active: i32,
        active_confirmed: bool,
        cx: &mut Context<Self>,
    ) -> Option<ClanId> {
        if let Some(clan_id) = self.active_clan_id
            && let Some(existing) = self.channel_mut(clan_id, thread_id)
        {
            if sync_thread_active_status(existing, active, active_confirmed) {
                cx.notify();
            }
            return Some(clan_id);
        }
        let clan_id = self.active_clan_id?;
        let parent_id = self.active_channel_id?;
        let channel = thread_channel_from_context(thread_id, label, clan_id, parent_id, active);
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
        self.ensure_thread_with_parent_active(
            thread_id,
            parent_id,
            clan_id,
            label,
            CHANNEL_ACTIVE_JOINED,
            false,
            cx,
        );
    }

    pub fn ensure_thread_with_parent_active(
        &mut self,
        thread_id: ChannelId,
        parent_id: ChannelId,
        clan_id: ClanId,
        label: String,
        active: i32,
        active_confirmed: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(existing) = self.channel_mut(clan_id, thread_id) {
            if sync_thread_active_status(existing, active, active_confirmed) {
                if !label.is_empty() {
                    existing.name = label;
                }
                cx.notify();
            } else if !label.is_empty() && existing.name != label {
                existing.name = label;
                cx.notify();
            }
            return;
        }
        let channel = thread_channel_from_context(thread_id, label, clan_id, parent_id, active);
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

    pub fn begin_reactivate_for_send(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        mode: i32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.reactivate_state_for_send(channel_id, clan_id, mode, cx) else {
            return false;
        };
        if !state.should_reactivate {
            return false;
        }
        if state.sync_cached_archived
            && let Some(existing) = self.channel_mut(clan_id, channel_id)
        {
            let _ = sync_thread_active_status(existing, CHANNEL_ACTIVE_ARCHIVED, true);
        }
        self.reactivating.insert(channel_id)
    }

    pub fn apply_thread_reactivated(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        label: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let parent_id = self
            .channel(clan_id, channel_id)
            .and_then(|ch| ch.parent_id)
            .or_else(|| self.archived_channel_parents.get(&channel_id).copied());
        let existing_label = self
            .channel(clan_id, channel_id)
            .map(|ch| ch.name.clone())
            .unwrap_or_default();
        let name = label.filter(|s| !s.is_empty()).unwrap_or(existing_label);

        if let Some(ch) = self.channel_mut(clan_id, channel_id) {
            ch.active = CHANNEL_ACTIVE_JOINED;
            if !name.is_empty() {
                ch.name = name;
            }
        } else if let Some(parent_id) = parent_id {
            let channel = thread_channel_from_context(
                channel_id,
                name,
                clan_id,
                parent_id,
                CHANNEL_ACTIVE_JOINED,
            );
            if let Some(cats) = self.cache.get_mut(&clan_id)
                && insert_channel(cats, channel)
            {
                self.invalidate_channel_index(clan_id);
            }
        }

        self.reactivating.remove(&channel_id);
        self.archived_channel_ids.remove(&channel_id);
        self.archived_channel_parents.remove(&channel_id);
        self.upsert_user_channel_from_cache(clan_id, channel_id, cx);
        cx.notify();
        crate::threads::ThreadsStore::global(cx).update(cx, |store, cx| {
            store.mark_thread_active(&channel_id.to_string(), cx);
        });
    }

    pub fn finish_reactivating(&mut self, channel_id: ChannelId) {
        self.reactivating.remove(&channel_id);
    }

    pub fn begin_archiving(&mut self, channel_id: ChannelId) -> bool {
        self.archiving.insert(channel_id)
    }

    pub fn finish_archiving(&mut self, channel_id: ChannelId) {
        self.archiving.remove(&channel_id);
    }

    pub fn is_archiving(&self, channel_id: ChannelId) -> bool {
        self.archiving.contains(&channel_id)
    }

    pub fn begin_deleting(&mut self, channel_id: ChannelId) -> bool {
        self.deleting.insert(channel_id)
    }

    pub fn finish_deleting(&mut self, channel_id: ChannelId) {
        self.deleting.remove(&channel_id);
    }

    pub fn is_deleting(&self, channel_id: ChannelId) -> bool {
        self.deleting.contains(&channel_id)
    }

    pub fn should_persist_compose_draft(&self, channel_id: ChannelId) -> bool {
        !self.archiving.contains(&channel_id)
            && !self.deleting.contains(&channel_id)
            && !self.is_locally_archived(channel_id)
            && !self.is_locally_deleted(channel_id)
    }

    pub fn clear_compose_draft(&self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if let Some(store) = ComposeStore::try_global(cx) {
            store.update(cx, |store, _| store.clear_draft(channel_id));
        }
    }

    pub fn apply_local_delete(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        parent_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.deleted_channel_ids.insert(channel_id);
        if !parent_id.is_zero() {
            self.deleted_channel_parents.insert(channel_id, parent_id);
        }
        self.channel_detail_pending.remove(&channel_id);
        self.channel_detail_failed.remove(&channel_id);
        let leaving_badge = self
            .channel(clan_id, channel_id)
            .map(|ch| ch.badge_count)
            .unwrap_or(0);
        let mut removed = false;
        let mut swept_all = false;
        if let Some(cats) = self.cache.get_mut(&clan_id) {
            removed = remove_channel(cats, channel_id);
        } else {
            swept_all = true;
            for cats in self.cache.values_mut() {
                removed |= remove_channel(cats, channel_id);
            }
        }
        removed |= remove_user_channel(
            &mut self.user_channels,
            &mut self.user_channels_order,
            channel_id,
        );
        let in_voice_changed = remove_in_voice_in_channel(&mut self.in_voice, channel_id);
        if removed {
            if swept_all {
                self.invalidate_channel_index_all();
            } else {
                self.invalidate_channel_index(clan_id);
            }
        }
        let mut changed = removed || in_voice_changed;
        if self.active_channel_id == Some(channel_id) {
            let redirect = (!parent_id.is_zero())
                .then_some(parent_id)
                .filter(|parent| self.channel_in_clan(clan_id, *parent));
            if let Some(parent_id) = redirect {
                self.remembered_channels.insert(clan_id, parent_id);
            }
            self.active_channel_id = redirect;
            cx.emit(ChannelEvent::ActiveChannelChanged(redirect));
            changed = true;
        }
        if !parent_id.is_zero() && removed {
            crate::threads::ThreadsStore::global(cx).update(cx, |store, cx| {
                store.remove_thread_locally(&channel_id.to_string(), cx);
            });
        }
        if removed || in_voice_changed {
            if leaving_badge > 0 {
                self.sync_clan_after_read(clan_id, leaving_badge, cx);
            }
            notify_in_voice_change(removed, in_voice_changed, cx);
        }
        if parent_id.is_zero() {
            self.clear_compose_draft(channel_id, cx);
            self.remove_deleted_child_threads_of_channel(clan_id, channel_id, cx);
            self.remove_favorite_locally(channel_id, clan_id, cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub fn apply_local_archive(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        parent_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let leaving_badge = self
            .channel(clan_id, channel_id)
            .map(|ch| ch.badge_count)
            .unwrap_or(0);
        self.archived_channel_ids.insert(channel_id);
        if !parent_id.is_zero() {
            self.archived_channel_parents.insert(channel_id, parent_id);
        }
        self.channel_detail_pending.remove(&channel_id);
        self.channel_detail_failed.remove(&channel_id);
        let mut removed = false;
        let mut swept_all = false;
        if let Some(cats) = self.cache.get_mut(&clan_id) {
            removed = remove_channel(cats, channel_id);
        } else {
            swept_all = true;
            for cats in self.cache.values_mut() {
                removed |= remove_channel(cats, channel_id);
            }
        }
        if removed {
            if swept_all {
                self.invalidate_channel_index_all();
            } else {
                self.invalidate_channel_index(clan_id);
            }
        }
        remove_user_channel(
            &mut self.user_channels,
            &mut self.user_channels_order,
            channel_id,
        );
        if self.active_channel_id == Some(channel_id) {
            let redirect = (!parent_id.is_zero()).then_some(parent_id);
            self.active_channel_id = redirect;
            cx.emit(ChannelEvent::ActiveChannelChanged(redirect));
        }
        if leaving_badge > 0 {
            self.sync_clan_after_read(clan_id, leaving_badge, cx);
        }
        cx.notify();
        crate::threads::ThreadsStore::global(cx).update(cx, |store, cx| {
            store.mark_thread_archived(&channel_id.to_string(), cx);
        });
        let draft_key = if parent_id.is_zero() {
            channel_id
        } else {
            parent_id
        };
        self.clear_compose_draft(draft_key, cx);
        if parent_id.is_zero() {
            self.remove_child_threads_of_channel(clan_id, channel_id, cx);
            self.remove_favorite_locally(channel_id, clan_id, cx);
        }
    }

    fn remove_child_threads_of_channel(
        &mut self,
        clan_id: ClanId,
        parent_channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let child_ids = self.child_thread_ids(clan_id, parent_channel_id);
        if child_ids.is_empty() {
            return;
        }
        self.archived_cascade_children
            .insert(parent_channel_id, child_ids.clone());
        let mut removed_any = false;
        if let Some(cats) = self.cache.get_mut(&clan_id) {
            for child_id in &child_ids {
                removed_any |= remove_channel(cats, *child_id);
            }
        }
        for child_id in child_ids {
            self.archived_channel_ids.insert(child_id);
            self.channel_detail_pending.remove(&child_id);
            self.channel_detail_failed.remove(&child_id);
            if self.active_channel_id == Some(child_id) {
                self.active_channel_id = Some(parent_channel_id);
                cx.emit(ChannelEvent::ActiveChannelChanged(Some(parent_channel_id)));
            }
            remove_user_channel(
                &mut self.user_channels,
                &mut self.user_channels_order,
                child_id,
            );
            crate::threads::ThreadsStore::global(cx).update(cx, |store, cx| {
                store.mark_thread_archived(&child_id.to_string(), cx);
            });
            self.clear_compose_draft(parent_channel_id, cx);
        }
        if removed_any {
            self.invalidate_channel_index(clan_id);
            cx.notify();
        }
    }

    fn child_thread_ids(&self, clan_id: ClanId, parent_channel_id: ChannelId) -> Vec<ChannelId> {
        self.cache
            .get(&clan_id)
            .map(|cats| {
                cats.iter()
                    .flat_map(|c| &c.channels)
                    .filter(|ch| ch.parent_id == Some(parent_channel_id))
                    .map(|ch| ch.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn remove_deleted_child_threads_of_channel(
        &mut self,
        clan_id: ClanId,
        parent_channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let child_ids = self.child_thread_ids(clan_id, parent_channel_id);
        if let Some(threads) = crate::threads::ThreadsStore::try_global(cx) {
            threads.update(cx, |store, cx| {
                store.remove_threads_of_parent(&parent_channel_id.to_string(), cx);
            });
        }
        if child_ids.is_empty() {
            return;
        }
        let mut removed_any = false;
        if let Some(cats) = self.cache.get_mut(&clan_id) {
            for child_id in &child_ids {
                removed_any |= remove_channel(cats, *child_id);
            }
        }
        let mut active_cleared = false;
        for child_id in child_ids {
            self.deleted_channel_ids.insert(child_id);
            self.deleted_channel_parents
                .insert(child_id, parent_channel_id);
            self.channel_detail_pending.remove(&child_id);
            self.channel_detail_failed.remove(&child_id);
            if self.active_channel_id == Some(child_id) {
                self.active_channel_id = None;
                active_cleared = true;
            }
            remove_user_channel(
                &mut self.user_channels,
                &mut self.user_channels_order,
                child_id,
            );
            self.clear_compose_draft(child_id, cx);
        }
        if active_cleared {
            cx.emit(ChannelEvent::ActiveChannelChanged(None));
        }
        if removed_any {
            self.invalidate_channel_index(clan_id);
        }
        self.sync_clan_after_read(clan_id, 0, cx);
        cx.notify();
    }

    pub fn apply_self_removed_from_channel(
        &mut self,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) -> bool {
        let leaving = self
            .cache
            .iter()
            .flat_map(|(clan_id, cats)| {
                cats.iter()
                    .flat_map(|c| &c.channels)
                    .map(move |ch| (*clan_id, ch))
            })
            .find(|(_, ch)| ch.id == channel_id)
            .map(|(clan_id, ch)| {
                (
                    clan_id,
                    ch.badge_count,
                    ch.parent_id.filter(|parent_id| !parent_id.is_zero()),
                )
            });
        let parent_redirect = leaving
            .and_then(|(clan_id, _, parent_id)| parent_id.map(|parent_id| (clan_id, parent_id)));
        let mut removed = false;
        for cats in self.cache.values_mut() {
            removed |= remove_channel(cats, channel_id);
        }
        removed |= remove_user_channel(
            &mut self.user_channels,
            &mut self.user_channels_order,
            channel_id,
        );
        let in_voice_changed = remove_in_voice_in_channel(&mut self.in_voice, channel_id);
        if removed {
            self.invalidate_channel_index_all();
            if self.active_channel_id == Some(channel_id) {
                let redirect = parent_redirect
                    .filter(|(clan_id, parent_id)| self.channel_in_clan(*clan_id, *parent_id));
                if let Some((clan_id, parent_id)) = redirect {
                    self.remembered_channels.insert(clan_id, parent_id);
                }
                let target = redirect.map(|(_, parent_id)| parent_id);
                self.active_channel_id = target;
                cx.emit(ChannelEvent::ActiveChannelChanged(target));
            }
            if let Some((clan_id, badge_count, _)) = leaving {
                self.sync_clan_after_read(clan_id, badge_count, cx);
            }
        }
        notify_in_voice_change(removed, in_voice_changed, cx);
        removed
    }

    fn apply_channel_archive_event(
        &mut self,
        e: &mezon_proto::realtime::ChannelArchiveEvent,
        cx: &mut Context<Self>,
    ) {
        let clan_id = ClanId(e.clan_id);
        if clan_id.is_zero() {
            return;
        }
        let channel_id = ChannelId(e.channel_id);
        let parent_id = ChannelId(e.parent_id);
        let is_archive = e.active == CHANNEL_ACTIVE_ARCHIVED;

        if is_archive {
            let was_viewing = self.active_channel_id == Some(channel_id);
            let is_thread = !parent_id.is_zero();
            let archived_by_other = crate::badge::BadgeService::try_global(cx)
                .and_then(|badges| badges.read(cx).current_user_id(cx))
                .is_some_and(|me| me.get() != e.creator_id);

            self.apply_local_archive(clan_id, channel_id, parent_id, cx);

            if was_viewing && archived_by_other {
                cx.emit(ChannelEvent::ArchivedByAdministrator { is_thread });
            }
            return;
        }

        self.archived_channel_ids.remove(&channel_id);
        self.archived_channel_parents.remove(&channel_id);
        if parent_id.is_zero()
            && let Some(children) = self.archived_cascade_children.remove(&channel_id)
        {
            for child_id in children {
                self.archived_channel_ids.remove(&child_id);
            }
        }
        let label = e.channel_label.clone();
        if !parent_id.is_zero() {
            self.ensure_thread_with_parent_active(
                channel_id,
                parent_id,
                clan_id,
                label.clone(),
                CHANNEL_ACTIVE_JOINED,
                true,
                cx,
            );
            if let Some(ch) = self.channel_mut(clan_id, channel_id) {
                if !label.is_empty() {
                    ch.name = label;
                }
                ch.private = e.channel_private;
                if e.category_id != 0 {
                    ch.category_id = Some(e.category_id.to_string());
                }
            }
        } else if let Some(ch) = self.channel_mut(clan_id, channel_id) {
            ch.active = CHANNEL_ACTIVE_JOINED;
            if !label.is_empty() {
                ch.name = label;
            }
        } else if self.cache.contains(&clan_id) {
            let channel = Channel {
                id: channel_id,
                name: label,
                channel_type: ChannelType::from_raw(e.channel_type as u32),
                private: e.channel_private,
                clan_id,
                clan_name: String::new(),
                category_name: String::new(),
                category_id: Some(e.category_id.to_string()).filter(|s| !s.is_empty() && s != "0"),
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
                creator_id: UserId(e.creator_id),
                active: CHANNEL_ACTIVE_JOINED,
                avatar_url: String::new(),
                topic: String::new(),
                age_restricted: 0,
                e2ee: 0,
                app_id: 0,
            };
            if let Some(cats) = self.cache.get_mut(&clan_id)
                && insert_channel(cats, channel)
            {
                self.invalidate_channel_index(clan_id);
            }
        }
        if !parent_id.is_zero() {
            self.upsert_user_channel_from_cache(clan_id, channel_id, cx);
        }
        self.reactivating.remove(&channel_id);
        cx.notify();
        crate::threads::ThreadsStore::global(cx).update(cx, |store, cx| {
            store.mark_thread_active(&channel_id.to_string(), cx);
        });
    }

    fn channel_mut(&mut self, clan_id: ClanId, channel_id: ChannelId) -> Option<&mut Channel> {
        let (cat_idx, ch_idx) = self.channel_location(clan_id, channel_id)?;
        self.cache
            .get_mut(&clan_id)?
            .get_mut(cat_idx)?
            .channels
            .get_mut(ch_idx)
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

    pub fn is_channel_favorite(&self, clan_id: ClanId, channel_id: ChannelId) -> bool {
        self.favorites
            .get(&clan_id)
            .is_some_and(|ids| ids.contains(&channel_id))
    }

    pub fn add_channel_favorite(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        self.apply_favorite_locally(channel_id, clan_id, true, cx);

        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .add_channel_favorite(channel_id.get(), clan_id.get())
                .await
            {
                tracing::error!("add_channel_favorite failed: {e}");
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation == generation {
                        this.apply_favorite_locally(channel_id, clan_id, false, cx);
                    }
                });
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
        self.apply_favorite_locally(channel_id, clan_id, false, cx);

        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .remove_channel_favorite(channel_id.get(), clan_id.get())
                .await
            {
                tracing::error!("remove_channel_favorite failed: {e}");
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation == generation {
                        this.apply_favorite_locally(channel_id, clan_id, true, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn apply_favorite_locally(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        favorite: bool,
        cx: &mut Context<Self>,
    ) {
        if favorite {
            self.add_favorite_locally(channel_id, clan_id, cx);
        } else {
            self.remove_favorite_locally(channel_id, clan_id, cx);
        }
    }

    fn add_favorite_locally(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        self.favorites
            .entry(clan_id)
            .or_default()
            .insert(channel_id);
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
    }

    fn remove_favorite_locally(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        if let Some(ids) = self.favorites.get_mut(&clan_id) {
            ids.remove(&channel_id);
        }
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
    }
}

fn should_reactivate_thread_after_send(mode: i32, channel: &Channel, archived: bool) -> bool {
    mode == STREAM_MODE_THREAD && channel.is_thread() && archived
}

fn thread_needs_reactivate(channel: &Channel) -> bool {
    channel.is_archived() || thread_is_stale(channel)
}

fn seed_added_thread_unread(last_sent_timestamp: i64) -> (i64, i64) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let last_sent = if last_sent_timestamp > 0 {
        last_sent_timestamp
    } else {
        now
    };
    (last_sent, (now - ADDED_THREAD_UNREAD_WINDOW_SECONDS).max(0))
}

fn thread_is_stale(channel: &Channel) -> bool {
    if channel.last_sent_timestamp <= 0 {
        return false;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    now - channel.last_sent_timestamp > THREAD_ARCHIVE_DURATION_SECONDS
}

fn sync_thread_active_status(existing: &mut Channel, active: i32, confirmed: bool) -> bool {
    if existing.active == active {
        return false;
    }
    if active != CHANNEL_ACTIVE_ARCHIVED && existing.is_archived() && !confirmed {
        return false;
    }
    if active == CHANNEL_ACTIVE_ARCHIVED && !confirmed {
        return false;
    }
    existing.active = active;
    true
}

struct ThreadReactivateState {
    should_reactivate: bool,
    sync_cached_archived: bool,
}

impl ChannelList {
    fn reactivate_state_for_send(
        &self,
        channel_id: ChannelId,
        clan_id: ClanId,
        mode: i32,
        cx: &App,
    ) -> Option<ThreadReactivateState> {
        let channel = self.channel(clan_id, channel_id)?.clone();
        let threads_archived = crate::threads::ThreadsStore::global(cx)
            .read(cx)
            .thread_active(&channel_id.to_string())
            == Some(CHANNEL_ACTIVE_ARCHIVED);
        let archived = threads_archived || thread_needs_reactivate(&channel);
        Some(ThreadReactivateState {
            should_reactivate: should_reactivate_thread_after_send(mode, &channel, archived),
            sync_cached_archived: threads_archived && !channel.is_archived(),
        })
    }
}

fn thread_channel_from_context(
    thread_id: ChannelId,
    label: String,
    clan_id: ClanId,
    parent_id: ChannelId,
    active: i32,
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
        active,
        avatar_url: String::new(),
        topic: String::new(),
        age_restricted: 0,
        e2ee: 0,
        app_id: 0,
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
            display_name: String::new(),
            avatar_url: String::new(),
            user_id: uid,
            sharing_screen: false,
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
        active: c.active,
        avatar_url: c.channel_avatar,
        topic: c.topic,
        age_restricted: c.age_restricted,
        e2ee: c.e2ee,
        app_id: c.app_id,
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
        if let Some(ts) = threads {
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

fn category_from_desc(c: ApiCategoryDesc) -> Category {
    Category {
        id: c.category_id.to_string(),
        clan_id: ClanId(c.clan_id),
        name: c.category_name,
        order: c.category_order,
        channels: Vec::new(),
    }
}

fn upsert_category(categories: &mut Vec<Category>, category: Category) -> bool {
    if category.id == FAVOR_CATE_ID {
        return false;
    }

    if let Some(existing) = categories
        .iter_mut()
        .find(|existing| existing.id == category.id)
    {
        let mut changed = false;
        if existing.name != category.name {
            existing.name = category.name;
            changed = true;
        }
        if existing.order != category.order {
            existing.order = category.order;
            changed = true;
        }
        if existing.clan_id != category.clan_id {
            existing.clan_id = category.clan_id;
            changed = true;
        }
        return changed;
    }

    categories.push(category);
    true
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
            .position(|ch| ch.parent_id.is_none() && ch.id > channel.id)
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

fn move_channel_to_category(
    categories: &mut Vec<Category>,
    channel_id: ChannelId,
    new_category_id: &str,
    new_category_name: &str,
) -> bool {
    let Some(channel) = categories
        .iter()
        .filter(|category| category.id != FAVOR_CATE_ID)
        .flat_map(|category| category.channels.iter())
        .find(|channel| channel.id == channel_id)
        .cloned()
    else {
        return false;
    };
    if !remove_channel(categories, channel_id) {
        return false;
    }
    let mut moved = channel.clone();
    moved.category_id = Some(new_category_id.to_string());
    moved.category_name = new_category_name.to_string();
    if insert_channel(categories, moved) {
        return true;
    }
    insert_channel(categories, channel);
    false
}

fn notify_in_voice_change(
    channels_changed: bool,
    in_voice_changed: bool,
    cx: &mut Context<ChannelList>,
) {
    if in_voice_changed {
        cx.emit(ChannelEvent::InVoiceChanged);
    }
    if channels_changed || in_voice_changed {
        cx.notify();
    }
}

fn apply_in_voice_joined(
    in_voice: &mut HashMap<UserId, InVoiceInfo>,
    user_id: UserId,
    info: InVoiceInfo,
) -> bool {
    if user_id.is_zero() {
        return false;
    }
    in_voice.insert(user_id, info) != Some(info)
}

fn apply_in_voice_leaved(
    in_voice: &mut HashMap<UserId, InVoiceInfo>,
    user_id: UserId,
    channel_id: ChannelId,
) -> bool {
    if in_voice
        .get(&user_id)
        .is_some_and(|info| info.channel_id == channel_id)
    {
        in_voice.remove(&user_id);
        return true;
    }
    false
}

fn should_sync_channel_to_user_list(channel: &Channel) -> bool {
    if channel.clan_id.is_zero() {
        let raw = channel.channel_type.as_raw();
        return raw != 2 && raw != 3;
    }
    !matches!(channel.channel_type, ChannelType::App | ChannelType::Voice)
}

fn upsert_user_channel(
    user_channels: &mut HashMap<ChannelId, Channel>,
    order: &mut Vec<ChannelId>,
    channel: Channel,
) -> bool {
    let channel_id = channel.id;
    if let Some(existing) = user_channels.get_mut(&channel_id) {
        merge_user_channel_from_structure(existing, &channel)
    } else {
        user_channels.insert(channel_id, channel);
        order.push(channel_id);
        true
    }
}

fn merge_user_channel_from_structure(target: &mut Channel, source: &Channel) -> bool {
    let mut changed = false;
    if target.name != source.name {
        target.name = source.name.clone();
        changed = true;
    }
    if target.channel_type != source.channel_type {
        target.channel_type = source.channel_type;
        changed = true;
    }
    if target.private != source.private {
        target.private = source.private;
        changed = true;
    }
    if target.clan_id != source.clan_id {
        target.clan_id = source.clan_id;
        changed = true;
    }
    if target.clan_name != source.clan_name {
        target.clan_name = source.clan_name.clone();
        changed = true;
    }
    if target.category_name != source.category_name {
        target.category_name = source.category_name.clone();
        changed = true;
    }
    if target.category_id != source.category_id {
        target.category_id = source.category_id.clone();
        changed = true;
    }
    if target.parent_id != source.parent_id {
        target.parent_id = source.parent_id;
        changed = true;
    }
    if target.member_count != source.member_count {
        target.member_count = source.member_count;
        changed = true;
    }
    if target.muted != source.muted {
        target.muted = source.muted;
        changed = true;
    }
    if target.is_favorite != source.is_favorite {
        target.is_favorite = source.is_favorite;
        changed = true;
    }
    if target.creator_id != source.creator_id {
        target.creator_id = source.creator_id;
        changed = true;
    }
    if target.active != source.active {
        target.active = source.active;
        changed = true;
    }
    if target.avatar_url != source.avatar_url {
        target.avatar_url = source.avatar_url.clone();
        changed = true;
    }
    if source.last_sent_timestamp >= target.last_sent_timestamp {
        let was_unread = target.is_unread();
        let was_badge = target.badge_count;
        copy_channel_unread_fields(target, source);
        changed |= was_unread != target.is_unread() || was_badge != target.badge_count;
    }
    changed
}

fn remove_user_channel(
    user_channels: &mut HashMap<ChannelId, Channel>,
    order: &mut Vec<ChannelId>,
    channel_id: ChannelId,
) -> bool {
    if user_channels.remove(&channel_id).is_none() {
        return false;
    }
    order.retain(|id| *id != channel_id);
    true
}

fn remove_in_voice_in_channel(
    in_voice: &mut HashMap<UserId, InVoiceInfo>,
    channel_id: ChannelId,
) -> bool {
    let before = in_voice.len();
    in_voice.retain(|_, info| info.channel_id != channel_id);
    in_voice.len() != before
}

fn collect_voice_members(
    cache: &KeyedCache<ClanId, Vec<Category>>,
    clan_id: ClanId,
) -> HashMap<ChannelId, Vec<VoiceMember>> {
    cache
        .get(&clan_id)
        .map(|categories| {
            categories
                .iter()
                .flat_map(|c| &c.channels)
                .filter(|ch| !ch.voice_members.is_empty())
                .map(|ch| (ch.id, ch.voice_members.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn merge_previous_voice_members(
    categories: &mut [Category],
    previous: &HashMap<ChannelId, Vec<VoiceMember>>,
) {
    for ch in categories.iter_mut().flat_map(|c| c.channels.iter_mut()) {
        let Some(prev) = previous.get(&ch.id) else {
            continue;
        };
        for member in prev {
            if !ch.voice_members.iter().any(|m| m.user_id == member.user_id) {
                ch.voice_members.push(member.clone());
            }
        }
    }
}

struct LiveUnreadState {
    badge_count: u32,
    last_seen_timestamp: i64,
    last_seen_message_id: MessageId,
    last_sent_timestamp: i64,
    last_sent_message_id: MessageId,
}

fn collect_channel_badges(
    cache: &KeyedCache<ClanId, Vec<Category>>,
    clan_id: ClanId,
) -> HashMap<ChannelId, LiveUnreadState> {
    cache
        .get(&clan_id)
        .map(|categories| {
            categories
                .iter()
                .flat_map(|c| &c.channels)
                .map(|ch| {
                    (
                        ch.id,
                        LiveUnreadState {
                            badge_count: ch.badge_count,
                            last_seen_timestamp: ch.last_seen_timestamp,
                            last_seen_message_id: ch.last_seen_message_id,
                            last_sent_timestamp: ch.last_sent_timestamp,
                            last_sent_message_id: ch.last_sent_message_id,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn suppress_read_badge(ch: &mut Channel) {
    if ch.badge_count > 0
        && ch.last_sent_timestamp > 0
        && ch.last_seen_timestamp >= ch.last_sent_timestamp
    {
        ch.badge_count = 0;
    }
}

fn carry_live_channel_badges(
    categories: &mut [Category],
    previous: &HashMap<ChannelId, LiveUnreadState>,
) {
    for ch in categories.iter_mut().flat_map(|c| c.channels.iter_mut()) {
        let Some(live) = previous.get(&ch.id) else {
            continue;
        };
        ch.badge_count = ch.badge_count.max(live.badge_count);
        if live.last_seen_timestamp > ch.last_seen_timestamp
            || (live.last_seen_timestamp == ch.last_seen_timestamp
                && ch.last_seen_message_id.is_zero())
        {
            ch.last_seen_timestamp = live.last_seen_timestamp;
            ch.last_seen_message_id = live.last_seen_message_id;
        }
        if live.last_sent_timestamp > ch.last_sent_timestamp
            || (live.last_sent_timestamp == ch.last_sent_timestamp
                && ch.last_sent_message_id.is_zero())
        {
            ch.last_sent_timestamp = live.last_sent_timestamp;
            ch.last_sent_message_id = live.last_sent_message_id;
        }
        suppress_read_badge(ch);
    }
}

async fn fetch_favorites_within_paint_budget(
    api: &AppApi,
    clan_id: ClanId,
    executor: &BackgroundExecutor,
) -> Option<HashSet<ChannelId>> {
    let favorites = std::pin::pin!(api.list_favorite_channels(clan_id.get()));
    let budget = std::pin::pin!(executor.timer(FAVORITES_PAINT_BUDGET));
    match futures::future::select(favorites, budget).await {
        futures::future::Either::Left((result, _)) => parse_favorite_ids(result, clan_id),
        futures::future::Either::Right(_) => {
            tracing::warn!(
                target: "clan_load",
                clan_id = clan_id.get(),
                budget_ms = FAVORITES_PAINT_BUDGET.as_millis(),
                "list_favorite_channels outran the paint budget, painting the sidebar without it"
            );
            None
        }
    }
}

fn parse_favorite_ids(
    result: anyhow::Result<Vec<String>>,
    clan_id: ClanId,
) -> Option<HashSet<ChannelId>> {
    result
        .inspect_err(|e| tracing::warn!("list_favorite_channels failed for clan {clan_id}: {e}"))
        .ok()
        .map(|ids| {
            ids.into_iter()
                .filter_map(|s| match s.parse::<ChannelId>() {
                    Ok(id) => Some(id),
                    Err(e) => {
                        tracing::warn!(
                            "list_favorite_channels returned an unparseable id {s:?} for clan {clan_id}: {e}"
                        );
                        None
                    }
                })
                .collect()
        })
}

fn rebuild_favorites(
    mut categories: Vec<Category>,
    clan_id: ClanId,
    favorite_ids: Option<&HashSet<ChannelId>>,
) -> Vec<Category> {
    categories.retain(|category| category.id != FAVOR_CATE_ID);
    for ch in categories
        .iter_mut()
        .flat_map(|category| category.channels.iter_mut())
    {
        ch.is_favorite = favorite_ids.is_some_and(|ids| ids.contains(&ch.id));
    }
    assemble_with_favorites(categories, clan_id)
}

fn seed_in_voice_from_categories(
    in_voice: &mut HashMap<UserId, InVoiceInfo>,
    clan_id: ClanId,
    categories: &[Category],
) -> bool {
    let before = clan_in_voice_snapshot(in_voice, clan_id);

    in_voice.retain(|_, info| info.clan_id != clan_id);

    for channel in categories.iter().flat_map(|c| &c.channels) {
        for member in &channel.voice_members {
            apply_in_voice_joined(
                in_voice,
                member.user_id,
                InVoiceInfo {
                    clan_id,
                    channel_id: channel.id,
                },
            );
        }
    }

    clan_in_voice_snapshot(in_voice, clan_id) != before
}

fn clan_in_voice_snapshot(
    in_voice: &HashMap<UserId, InVoiceInfo>,
    clan_id: ClanId,
) -> HashMap<UserId, InVoiceInfo> {
    in_voice
        .iter()
        .filter(|(_, info)| info.clan_id == clan_id)
        .map(|(user, info)| (*user, *info))
        .collect()
}

fn update_channel(
    categories: &mut [Category],
    channel_id: ChannelId,
    label: Option<String>,
    topic: Option<String>,
    age_restricted: Option<i32>,
    private: bool,
) -> bool {
    let mut found = false;
    for cat in categories.iter_mut() {
        for ch in cat.channels.iter_mut() {
            if ch.id == channel_id {
                if let Some(ref label) = label {
                    ch.name = label.clone();
                }
                if let Some(ref topic) = topic {
                    ch.topic = topic.clone();
                }
                if let Some(age_restricted) = age_restricted {
                    ch.age_restricted = age_restricted;
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

fn unread_seed_from_descs(descs: Vec<ApiChannelDesc>) -> HashMap<ChannelId, ChannelUnreadSeed> {
    descs
        .into_iter()
        .filter(|d| {
            !matches!(
                ChannelType::from_raw(d.channel_type),
                ChannelType::App | ChannelType::Voice
            )
        })
        .map(|d| {
            (
                ChannelId(d.channel_id),
                ChannelUnreadSeed {
                    badge_count: d.badge_count.max(0) as u32,
                    last_seen_timestamp: d.last_seen_timestamp,
                    last_seen_message_id: MessageId(d.last_seen_message_id),
                    last_sent_timestamp: d.last_sent_timestamp,
                    last_sent_message_id: MessageId(d.last_sent_message_id),
                },
            )
        })
        .collect()
}

fn apply_unread_seed_into(
    seed: &HashMap<ChannelId, ChannelUnreadSeed>,
    categories: &mut [Category],
) -> bool {
    let mut changed = false;
    for ch in categories
        .iter_mut()
        .flat_map(|category| category.channels.iter_mut())
    {
        let Some(s) = seed.get(&ch.id) else {
            continue;
        };
        let was_unread = ch.is_unread();
        let was_badge = ch.badge_count;
        if s.last_sent_timestamp >= ch.last_sent_timestamp {
            ch.badge_count = s.badge_count;
        } else {
            ch.badge_count = ch.badge_count.max(s.badge_count);
        }
        if s.last_seen_timestamp > ch.last_seen_timestamp {
            ch.last_seen_timestamp = s.last_seen_timestamp;
            ch.last_seen_message_id = s.last_seen_message_id;
        }
        if s.last_sent_timestamp > ch.last_sent_timestamp {
            ch.last_sent_timestamp = s.last_sent_timestamp;
            ch.last_sent_message_id = s.last_sent_message_id;
        }
        suppress_read_badge(ch);
        changed = changed || was_badge != ch.badge_count || was_unread != ch.is_unread();
    }
    changed
}

fn merge_pending_badges_into(
    pending: &mut HashMap<ChannelId, PendingBadge>,
    categories: &mut [Category],
) {
    if pending.is_empty() {
        return;
    }
    let mut applied: Vec<ChannelId> = Vec::new();
    for category in categories.iter_mut() {
        for ch in category.channels.iter_mut() {
            if let Some(&overlay) = pending.get(&ch.id) {
                if overlay.last_sent_timestamp == 0
                    || overlay.last_sent_timestamp > ch.last_seen_timestamp
                {
                    ch.badge_count = ch.badge_count.max(overlay.count);
                }
                if overlay.last_sent_timestamp > ch.last_sent_timestamp {
                    ch.last_sent_timestamp = overlay.last_sent_timestamp;
                    if !overlay.last_sent_message_id.is_zero() {
                        ch.last_sent_message_id = overlay.last_sent_message_id;
                    }
                }
                suppress_read_badge(ch);
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

fn find_channel_in(categories: &[Category], channel_id: ChannelId) -> Option<&Channel> {
    categories
        .iter()
        .flat_map(|category| category.channels.iter())
        .find(|channel| channel.id == channel_id)
}

fn numeric_category_id(channel: &Channel) -> Option<i64> {
    channel
        .category_id
        .as_deref()
        .and_then(|id| id.parse::<i64>().ok())
        .filter(|id| *id != 0)
}

fn mark_as_read_category_id(categories: &[Category], channel_id: ChannelId) -> i64 {
    let Some(channel) = find_channel_in(categories, channel_id) else {
        return 0;
    };
    numeric_category_id(channel)
        .or_else(|| {
            channel
                .parent_id
                .and_then(|parent_id| find_channel_in(categories, parent_id))
                .and_then(numeric_category_id)
        })
        .unwrap_or(0)
}

fn thread_ids_of(categories: &[Category], parent_id: ChannelId) -> Vec<ChannelId> {
    let mut ids: Vec<ChannelId> = categories
        .iter()
        .flat_map(|category| category.channels.iter())
        .filter(|channel| channel.parent_id == Some(parent_id))
        .map(|channel| channel.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
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
    let Ok(data) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
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

fn channel_name_exists_in_categories(
    categories: &[Category],
    category_id: &str,
    name: &str,
) -> bool {
    let normalized = name.trim().to_lowercase();
    categories
        .iter()
        .filter(|category| category.id != FAVOR_CATE_ID && category.id == category_id)
        .flat_map(|category| category.channels.iter())
        .any(|channel| {
            channel.parent_id.is_none() && channel.name.trim().to_lowercase() == normalized
        })
}

fn effective_category_id(desc_category_id: i64, requested: Option<i64>) -> i64 {
    if desc_category_id == 0 {
        requested.unwrap_or(desc_category_id)
    } else {
        desc_category_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_create_channel_api_error_maps_out_of_range_to_limit() {
        use mezon_client::ApiStatusError;
        let err = anyhow::Error::from(ApiStatusError {
            code: ApiStatusError::OUT_OF_RANGE,
        });
        assert_eq!(
            map_create_channel_api_error(err),
            CreateChannelError::ChannelLimitExceeded
        );
        let err = anyhow::Error::from(ApiStatusError { code: 13 });
        assert!(matches!(
            map_create_channel_api_error(err),
            CreateChannelError::Other(_)
        ));
    }

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
        channels.insert(ClanId(1), vec![ChannelId(10), ChannelId(20), ChannelId(30)]);
        channels.insert(ClanId(0), vec![ChannelId(99)]);

        let json = serde_json::to_string(&channels).unwrap();
        let restored: HashMap<ClanId, Vec<ChannelId>> = serde_json::from_str(&json).unwrap();

        assert_eq!(
            restored.get(&ClanId(1)).map(Vec::as_slice),
            Some(&[ChannelId(10), ChannelId(20), ChannelId(30)][..])
        );
        assert_eq!(
            restored.get(&ClanId(0)).map(Vec::as_slice),
            Some(&[ChannelId(99)][..])
        );
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
            active: CHANNEL_ACTIVE_JOINED,
            avatar_url: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
        }
    }

    #[test]
    fn overview_duplicate_for_thread_uses_parent_id() {
        let mut thread = make_channel(9, "thread", "1");
        thread.channel_type = ChannelType::Thread;
        thread.parent_id = Some(ChannelId(77));
        assert_eq!(
            overview_duplicate_thread_parent_id(&thread).as_deref(),
            Some("77")
        );

        let mut orphan_thread = make_channel(10, "orphan", "1");
        orphan_thread.channel_type = ChannelType::Thread;
        assert_eq!(overview_duplicate_thread_parent_id(&orphan_thread), None);

        let mut zero_parent = make_channel(11, "zero-parent", "1");
        zero_parent.channel_type = ChannelType::Thread;
        zero_parent.parent_id = Some(ChannelId(0));
        assert_eq!(overview_duplicate_thread_parent_id(&zero_parent), None);

        let mut text_with_parent = make_channel(2, "text", "1");
        text_with_parent.parent_id = Some(ChannelId(77));
        assert_eq!(
            overview_duplicate_thread_parent_id(&text_with_parent).as_deref(),
            Some("77")
        );

        let channel = make_channel(1, "general", "1");
        assert_eq!(overview_duplicate_thread_parent_id(&channel), None);
    }

    #[test]
    fn a_voice_channel_is_busy_only_once_someone_has_company() {
        let member = |id: i64| VoiceMember {
            user_id: UserId(id),
            display_name: format!("user{id}"),
            avatar_url: String::new(),
            sharing_screen: false,
        };

        let mut voice = make_channel(1, "General Voice", "cat");
        voice.channel_type = ChannelType::Voice;
        assert!(!voice.voice_busy(), "an empty voice channel is free");

        voice.voice_members = vec![member(1)];
        assert!(
            !voice.voice_busy(),
            "one person in a voice channel is someone waiting, not a call to interrupt"
        );

        voice.voice_members.push(member(2));
        assert!(voice.voice_busy());

        let mut text = make_channel(2, "general", "cat");
        text.voice_members = vec![member(1), member(2)];
        assert!(
            !text.voice_busy(),
            "only voice channels can be busy, whatever a text channel carries"
        );
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
    fn channel_name_exists_in_category_matches_case_insensitively() {
        let cats = categories();
        assert!(channel_name_exists_in_categories(&cats, "cat1", "alpha"));
        assert!(channel_name_exists_in_categories(
            &cats,
            "cat1",
            "  ALPHA  "
        ));
        assert!(!channel_name_exists_in_categories(&cats, "cat1", "gamma"));
    }

    #[test]
    fn channel_name_exists_in_category_is_scoped_to_that_category() {
        let cats = categories();
        assert!(!channel_name_exists_in_categories(&cats, "cat2", "alpha"));
    }

    #[test]
    fn channel_name_exists_in_category_ignores_favorites_and_threads() {
        let mut cats = categories();
        cats.push(Category {
            id: FAVOR_CATE_ID.into(),
            clan_id: ClanId(1),
            name: "favoriteChannel".into(),
            order: i32::MIN,
            channels: vec![make_channel(20, "starred", FAVOR_CATE_ID)],
        });
        cats[0].channels.push(make_thread(30, 10, "cat1"));
        assert!(!channel_name_exists_in_categories(
            &cats,
            FAVOR_CATE_ID,
            "starred"
        ));
        assert!(!channel_name_exists_in_categories(&cats, "cat1", "30"));
    }

    #[test]
    fn effective_category_id_falls_back_to_requested_when_zero() {
        assert_eq!(effective_category_id(0, Some(42)), 42);
        assert_eq!(effective_category_id(0, None), 0);
        assert_eq!(effective_category_id(7, Some(42)), 7);
    }

    #[test]
    fn merge_previous_voice_members_preserves_realtime_join() {
        let mut fresh = categories();
        fresh[0].channels[0].voice_members = vec![VoiceMember {
            user_id: UserId(1),
            display_name: "u1".into(),
            avatar_url: String::new(),
            sharing_screen: false,
        }];

        let mut previous: HashMap<ChannelId, Vec<VoiceMember>> = HashMap::new();
        previous.insert(
            ChannelId(10),
            vec![
                VoiceMember {
                    user_id: UserId(1),
                    display_name: "u1".into(),
                    avatar_url: String::new(),
                    sharing_screen: false,
                },
                VoiceMember {
                    user_id: UserId(2),
                    display_name: "u2".into(),
                    avatar_url: String::new(),
                    sharing_screen: true,
                },
            ],
        );

        merge_previous_voice_members(&mut fresh, &previous);

        let members = &fresh[0].channels[0].voice_members;
        assert_eq!(members.len(), 2);
        assert!(members.iter().any(|m| m.user_id == UserId(2)));
        assert_eq!(members.iter().filter(|m| m.user_id == UserId(1)).count(), 1);
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

    fn category(id: &str, channels: Vec<Channel>) -> Category {
        Category {
            id: id.into(),
            clan_id: ClanId(1),
            name: "General".into(),
            order: 0,
            channels,
        }
    }

    fn favorites_category(channels: Vec<Channel>) -> Category {
        let mut cat = category(FAVOR_CATE_ID, channels);
        cat.name = "favoriteChannel".into();
        cat.order = i32::MIN;
        cat
    }

    #[test]
    fn mark_as_read_category_id_resolves_through_the_favorites_copy() {
        let mut favorited = make_channel(10, "alpha", "77");
        favorited.is_favorite = true;
        let cats = assemble_with_favorites(vec![category("77", vec![favorited])], ClanId(1));
        assert_eq!(cats[0].id, FAVOR_CATE_ID);
        assert_eq!(mark_as_read_category_id(&cats, ChannelId(10)), 77);
    }

    #[test]
    fn mark_as_read_category_id_falls_back_to_the_parent_channel() {
        let mut thread = make_thread(500, 10, "77");
        thread.category_id = None;
        let cats = vec![category(
            "77",
            vec![make_channel(10, "alpha", "77"), thread],
        )];
        assert_eq!(mark_as_read_category_id(&cats, ChannelId(500)), 77);
    }

    #[test]
    fn mark_as_read_category_id_is_zero_when_unresolvable() {
        let cats = categories();
        assert_eq!(mark_as_read_category_id(&cats, ChannelId(999)), 0);
        assert_eq!(mark_as_read_category_id(&cats, ChannelId(10)), 0);
    }

    #[test]
    fn mark_as_read_of_a_zero_channel_would_request_a_clan_wide_read() {
        let cats = vec![category("77", vec![make_channel(10, "alpha", "77")])];
        assert_eq!(mark_as_read_category_id(&cats, ChannelId(0)), 0);
    }

    #[test]
    fn thread_ids_of_collects_each_child_once() {
        let cats = vec![
            category(
                "77",
                vec![
                    make_channel(10, "alpha", "77"),
                    make_thread(501, 10, "77"),
                    make_thread(500, 10, "77"),
                    make_thread(600, 11, "77"),
                ],
            ),
            favorites_category(vec![make_thread(500, 10, "77")]),
        ];
        assert_eq!(
            thread_ids_of(&cats, ChannelId(10)),
            vec![ChannelId(500), ChannelId(501)]
        );
        assert!(thread_ids_of(&cats, ChannelId(999)).is_empty());
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
            CHANNEL_ACTIVE_JOINED,
        );
        assert_eq!(thread.id, ChannelId(500));
        assert_eq!(thread.channel_type, ChannelType::Thread);
        assert_eq!(thread.parent_id, Some(ChannelId(10)));
        assert_eq!(thread.clan_id, ClanId(1));
        assert!(!thread.private);
        assert_eq!(thread.active, CHANNEL_ACTIVE_JOINED);
    }

    #[test]
    fn synthesized_thread_inserts_nested_after_parent() {
        let mut cats = categories();
        let thread = thread_channel_from_context(
            ChannelId(500),
            "my-thread".into(),
            ClanId(1),
            ChannelId(10),
            CHANNEL_ACTIVE_JOINED,
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
    fn channel_detail_desc_inserts_thread_under_parent() {
        let mut cats = categories();
        let desc = ApiChannelDesc {
            channel_id: 500,
            channel_label: "my-thread".into(),
            channel_type: 7,
            clan_id: 0,
            category_name: String::new(),
            category_id: 0,
            channel_private: 0,
            count_mess_unread: 0,
            member_count: 0,
            parent_id: 10,
            is_mute: false,
            last_seen_message_id: 0,
            last_seen_timestamp: 0,
            last_sent_message_id: 0,
            last_sent_timestamp: 0,
            badge_count: 2,
            active: CHANNEL_ACTIVE_JOINED,
            creator_id: 0,
            clan_name: String::new(),
            channel_avatar: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
        };
        let badge = desc.badge_count.max(0) as u32;
        let mut channel = channel_from_desc(desc, badge, Vec::new(), false);
        channel.clan_id = ClanId(1);
        assert!(insert_channel(&mut cats, channel));
        let ids: Vec<ChannelId> = cats[0].channels.iter().map(|c| c.id).collect();
        let parent_pos = ids.iter().position(|id| *id == ChannelId(10)).unwrap();
        assert_eq!(ids[parent_pos + 1], ChannelId(500));
        let inserted = cats
            .iter()
            .flat_map(|c| &c.channels)
            .find(|c| c.id == ChannelId(500))
            .unwrap();
        assert_eq!(inserted.channel_type, ChannelType::Thread);
        assert_eq!(inserted.clan_id, ClanId(1));
        assert_eq!(inserted.badge_count, 2);
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
    fn move_channel_to_category_updates_membership_and_fields() {
        let mut cats = categories();
        cats.push(Category {
            id: "cat2".into(),
            clan_id: ClanId(1),
            name: "Category Two".into(),
            order: 1,
            channels: vec![make_channel(20, "gamma", "cat2")],
        });
        assert!(move_channel_to_category(
            &mut cats,
            ChannelId(10),
            "cat2",
            "Category Two"
        ));
        assert!(
            !cats
                .iter()
                .find(|c| c.id == "cat1")
                .expect("cat1")
                .channels
                .iter()
                .any(|ch| ch.id == ChannelId(10))
        );
        let moved = cats
            .iter()
            .find(|c| c.id == "cat2")
            .expect("cat2")
            .channels
            .iter()
            .find(|ch| ch.id == ChannelId(10))
            .expect("moved channel");
        assert_eq!(moved.category_id.as_deref(), Some("cat2"));
        assert_eq!(moved.category_name, "Category Two");
    }

    #[test]
    fn update_channel_patches_topic_and_age_restricted() {
        let mut c = categories();
        assert!(update_channel(
            &mut c,
            ChannelId(10),
            None,
            Some("rules channel".into()),
            Some(1),
            false,
        ));
        assert_eq!(c[0].channels[0].topic, "rules channel");
        assert_eq!(c[0].channels[0].age_restricted, 1);
        assert_eq!(c[0].channels[0].name, "alpha");
    }

    #[test]
    fn update_channel_renames_and_sets_private() {
        let mut c = categories();
        assert!(update_channel(
            &mut c,
            ChannelId(10),
            Some("renamed".into()),
            None,
            None,
            true
        ));
        assert_eq!(c[0].channels[0].name, "renamed");
        assert!(c[0].channels[0].private);
    }

    #[test]
    fn update_channel_blank_label_keeps_name() {
        let mut c = categories();
        assert!(update_channel(
            &mut c,
            ChannelId(11),
            None,
            None,
            None,
            true
        ));
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
            None,
            None,
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
            active: CHANNEL_ACTIVE_JOINED,
            creator_id: 0,
            clan_name: String::new(),
            channel_avatar: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
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
            sharing_screen: false,
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
        assert_eq!(ids, vec![10, 15, 12, 20, 25]);
    }

    #[test]
    fn build_categories_keeps_server_order_within_a_thread_group() {
        let api_cats = vec![ApiCategoryDesc {
            category_id: 1,
            category_name: "General".into(),
            clan_id: 1,
            category_order: 0,
        }];
        let mut zulu = make_thread(11, 10, "1");
        zulu.name = "zulu".into();
        let mut alpha = make_thread(12, 10, "1");
        alpha.name = "alpha".into();
        let mut mike = make_thread(13, 10, "1");
        mike.name = "mike".into();
        let mut channels = vec![make_channel(10, "parent", "1"), zulu, alpha, mike];
        let cats = build_categories(api_cats, &mut channels);
        let names: Vec<&str> = cats[0].channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["parent", "zulu", "alpha", "mike"]);
    }

    #[test]
    fn insert_channel_does_not_split_a_thread_block() {
        let mut cats = vec![Category {
            id: "cat1".into(),
            clan_id: ClanId(1),
            name: "General".into(),
            order: 0,
            channels: vec![
                make_channel(10, "xfans", "cat1"),
                make_thread(15, 10, "cat1"),
                make_thread(40, 10, "cat1"),
                make_channel(50, "later", "cat1"),
            ],
        }];

        assert!(insert_channel(
            &mut cats,
            make_channel(20, "yi-crycemi", "cat1")
        ));

        let ids: Vec<i64> = cats[0].channels.iter().map(|c| c.id.get()).collect();
        assert_eq!(ids, vec![10, 15, 40, 20, 50]);
    }

    #[test]
    fn insert_channel_keeps_top_level_id_order() {
        let mut cats = vec![Category {
            id: "cat1".into(),
            clan_id: ClanId(1),
            name: "General".into(),
            order: 0,
            channels: vec![
                make_channel(10, "a", "cat1"),
                make_thread(90, 10, "cat1"),
                make_channel(30, "c", "cat1"),
            ],
        }];

        assert!(insert_channel(&mut cats, make_channel(20, "b", "cat1")));

        let ids: Vec<i64> = cats[0].channels.iter().map(|c| c.id.get()).collect();
        assert_eq!(ids, vec![10, 90, 20, 30]);
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
            active: CHANNEL_ACTIVE_JOINED,
            avatar_url: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
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
                active: CHANNEL_ACTIVE_JOINED,
                avatar_url: String::new(),
                topic: String::new(),
                age_restricted: 0,
                e2ee: 0,
                app_id: 0,
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
                active: CHANNEL_ACTIVE_JOINED,
                avatar_url: String::new(),
                topic: String::new(),
                age_restricted: 0,
                e2ee: 0,
                app_id: 0,
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
    fn channel_from_desc_leaves_voice_member_name_unresolved() {
        let desc = ApiChannelDesc {
            channel_id: 10,
            channel_label: "voice".into(),
            channel_type: 10,
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
            badge_count: 0,
            active: CHANNEL_ACTIVE_JOINED,
            creator_id: 0,
            clan_name: String::new(),
            channel_avatar: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
        };
        let channel = channel_from_desc(desc, 0, vec![UserId(42)], false);
        let vm = &channel.voice_members[0];
        assert_eq!(vm.user_id, UserId(42));
        assert!(vm.display_name.is_empty());
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

    fn structure_with_two_channels() -> Vec<Category> {
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
                ch
            },
        ];
        build_categories(api_cats, &mut channels)
    }

    fn favor_ids(ids: &[ChannelId]) -> Option<HashSet<ChannelId>> {
        Some(ids.iter().copied().collect())
    }

    fn complete_extras() -> ClanExtras {
        ClanExtras {
            voice_map: Some(HashMap::new()),
            app_channels: Some(Vec::new()),
        }
    }

    const VOICE_CHANNEL: ChannelId = ChannelId(2);
    const VOICE_USER: UserId = UserId(7);

    fn voice_extras(voice_map: HashMap<ChannelId, Vec<VoiceMember>>) -> ClanExtras {
        ClanExtras {
            voice_map: Some(voice_map),
            app_channels: Some(Vec::new()),
        }
    }

    fn one_user_in_voice() -> HashMap<ChannelId, Vec<VoiceMember>> {
        [(
            VOICE_CHANNEL,
            vec![VoiceMember {
                user_id: VOICE_USER,
                display_name: VOICE_USER.to_string(),
                avatar_url: String::new(),
                sharing_screen: false,
            }],
        )]
        .into_iter()
        .collect()
    }

    fn voice_members_of(channels: &ChannelList, category_ix: usize) -> Vec<UserId> {
        channels.categories_for_clan(ClanId(1))[category_ix]
            .channels
            .iter()
            .find(|ch| ch.id == VOICE_CHANNEL)
            .map(|ch| ch.voice_members.iter().map(|m| m.user_id).collect())
            .unwrap_or_default()
    }

    fn assert_voice_member_on_both_copies(channels: &ChannelList, context: &str) {
        assert_eq!(
            voice_members_of(channels, 0),
            vec![VOICE_USER],
            "{context}: favorite copy lost its voice member"
        );
        assert_eq!(
            voice_members_of(channels, 1),
            vec![VOICE_USER],
            "{context}: source channel lost its voice member"
        );
        assert_eq!(
            channels.in_voice_status(VOICE_USER).map(|i| i.channel_id),
            Some(VOICE_CHANNEL),
            "{context}: in_voice index lost the user"
        );
    }

    #[gpui::test]
    fn extras_patch_voice_members_onto_the_favorite_copy_too(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[VOICE_CHANNEL]),
                    cx,
                );
                channels.apply_clan_extras(ClanId(1), voice_extras(one_user_in_voice()), cx);

                assert_voice_member_on_both_copies(channels, "extras apply");
            });
        });
    }

    #[gpui::test]
    fn failed_voice_fetch_keeps_the_members_it_already_knows(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[VOICE_CHANNEL]),
                    cx,
                );
                channels.apply_clan_extras(ClanId(1), voice_extras(one_user_in_voice()), cx);

                channels.apply_clan_extras(
                    ClanId(1),
                    ClanExtras {
                        voice_map: None,
                        app_channels: None,
                    },
                    cx,
                );

                assert_voice_member_on_both_copies(channels, "failed voice fetch");
            });
        });
    }

    #[gpui::test]
    fn an_empty_voice_response_clears_the_members(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[VOICE_CHANNEL]),
                    cx,
                );
                channels.apply_clan_extras(ClanId(1), voice_extras(one_user_in_voice()), cx);

                channels.apply_clan_extras(ClanId(1), voice_extras(HashMap::new()), cx);

                assert!(voice_members_of(channels, 0).is_empty());
                assert!(voice_members_of(channels, 1).is_empty());
                assert!(channels.in_voice_status(VOICE_USER).is_none());
            });
        });
    }

    #[gpui::test]
    fn realtime_voice_join_survives_a_structure_refetch(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[VOICE_CHANNEL]),
                    cx,
                );
                channels.apply_clan_extras(ClanId(1), voice_extras(HashMap::new()), cx);

                channels.handle_event(
                    &RealtimeEvent::VoiceJoined(mezon_proto::realtime::VoiceJoinedEvent {
                        clan_id: 1,
                        user_id: VOICE_USER.get(),
                        voice_channel_id: VOICE_CHANNEL.get(),
                        participant: "someone".into(),
                        ..Default::default()
                    }),
                    cx,
                );
                assert_voice_member_on_both_copies(channels, "realtime join");

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert_voice_member_on_both_copies(channels, "structure refetch");
            });
        });
    }

    #[gpui::test]
    fn realtime_screen_share_updates_voice_member_status(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                channels.apply_clan_extras(ClanId(1), voice_extras(one_user_in_voice()), cx);

                let sharing = |channels: &ChannelList| {
                    channels.categories_for_clan(ClanId(1))[1]
                        .channels
                        .iter()
                        .find(|ch| ch.id == VOICE_CHANNEL)
                        .and_then(|ch| {
                            ch.voice_members
                                .iter()
                                .find(|m| m.user_id == VOICE_USER)
                                .map(|m| m.sharing_screen)
                        })
                };

                assert_eq!(sharing(channels), Some(false));

                channels.handle_event(
                    &RealtimeEvent::ScreenShare(mezon_proto::realtime::ScreenShareEvent {
                        clan_id: 1,
                        voice_channel_id: VOICE_CHANNEL.get(),
                        user_id: VOICE_USER.get(),
                        is_sharing: true,
                    }),
                    cx,
                );
                assert_eq!(sharing(channels), Some(true));

                channels.handle_event(
                    &RealtimeEvent::ScreenShare(mezon_proto::realtime::ScreenShareEvent {
                        clan_id: 1,
                        voice_channel_id: VOICE_CHANNEL.get(),
                        user_id: VOICE_USER.get(),
                        is_sharing: false,
                    }),
                    cx,
                );
                assert_eq!(sharing(channels), Some(false));
            });
        });
    }

    #[test]
    fn rebuild_favorites_derives_the_favor_cate_from_the_id_set() {
        let ids: HashSet<ChannelId> = [ChannelId(2)].into_iter().collect();
        let result = rebuild_favorites(structure_with_two_channels(), ClanId(1), Some(&ids));

        assert_eq!(result[0].id, FAVOR_CATE_ID);
        assert_eq!(result[0].channels.len(), 1);
        assert_eq!(result[0].channels[0].id, ChannelId(2));
        let source = result[1].channels.iter().find(|ch| ch.id == ChannelId(2));
        assert!(source.is_some_and(|ch| ch.is_favorite));
    }

    #[test]
    fn rebuild_favorites_keeps_a_single_favor_cate_when_run_twice() {
        let once = rebuild_favorites(structure_with_two_channels(), ClanId(1), None);
        let twice = rebuild_favorites(once, ClanId(1), None);

        assert_eq!(
            twice.iter().filter(|c| c.id == FAVOR_CATE_ID).count(),
            1,
            "assembling twice must not stack favor categories"
        );
        assert!(twice[0].channels.is_empty());
    }

    fn init_channel_list(cx: &mut App) -> Entity<ChannelList> {
        let api = Arc::new(mezon_client::AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| crate::AuthState::NotAuthenticated);
        crate::badge::BadgeService::init(auth_state, cx);
        crate::clan::ClanList::init(api.clone(), cx);
        cx.new(|cx| ChannelList::new(api, cx))
    }

    const REMOVED_SELF: i64 = 77;

    fn init_authenticated_channel_list(cx: &mut App) -> Entity<ChannelList> {
        let api = Arc::new(mezon_client::AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| {
            crate::AuthState::Authenticated(mezon_client::Session {
                user_id: REMOVED_SELF.to_string(),
                ..Default::default()
            })
        });
        crate::badge::BadgeService::init(auth_state, cx);
        crate::clan::ClanList::init(api.clone(), cx);
        cx.new(|cx| ChannelList::new(api, cx))
    }

    fn test_clan(id: ClanId, name: &str) -> crate::clan::Clan {
        crate::clan::Clan {
            id,
            creator_id: UserId(0),
            name: name.into(),
            avatar_url: None,
            banner_url: None,
            badge_count: 0,
            has_unread: false,
            muted: false,
            welcome_channel_id: None,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
            community_banner: String::new(),
            about: String::new(),
            description: String::new(),
            short_url: String::new(),
        }
    }

    #[gpui::test]
    fn leaving_a_clan_forgets_its_channels(cx: &mut gpui::TestAppContext) {
        let channels = cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            crate::clan::ClanList::global(cx).update(cx, |clans, cx| {
                clans.update_clans(vec![test_clan(ClanId(1), "One")], cx);
            });
            channels.update(cx, |channels, cx| {
                channels.seed_clan_channels_for_test(ClanId(1), categories());
                channels.show_empty_categories.insert(ClanId(1));
                channels
                    .remembered_channels
                    .insert(ClanId(1), ChannelId(10));
                channels.select_channel(ChannelId(10), cx);
            });
            channels
        });

        cx.update(|cx| {
            crate::clan::ClanList::global(cx).update(cx, |clans, cx| {
                clans.handle_event(
                    &RealtimeEvent::UserClanRemoved(mezon_proto::realtime::UserClanRemoved {
                        clan_id: 1,
                        user_ids: vec![REMOVED_SELF],
                    }),
                    cx,
                );
            });
        });

        cx.update(|cx| {
            let channels = channels.read(cx);
            assert!(
                channels.cache.get(&ClanId(1)).is_none(),
                "a clan you left must not keep its channel structure cached"
            );
            assert!(!channels.show_empty_categories.contains(&ClanId(1)));
            assert!(!channels.remembered_channels.contains_key(&ClanId(1)));
            assert_eq!(channels.active_channel_id, None);
        });
    }

    fn structure_with_a_thread() -> Vec<Category> {
        let api_cats = vec![ApiCategoryDesc {
            category_id: 1,
            category_name: "General".into(),
            clan_id: 1,
            category_order: 0,
        }];
        let mut channels = vec![
            {
                let mut ch = make_channel(1, "parent", "1");
                ch.clan_id = ClanId(1);
                ch
            },
            {
                let mut ch = make_channel(9, "thread", "1");
                ch.clan_id = ClanId(1);
                ch.parent_id = Some(ChannelId(1));
                ch.channel_type = ChannelType::Thread;
                ch
            },
        ];
        build_categories(api_cats, &mut channels)
    }

    fn structure_with_parent_and_two_threads() -> Vec<Category> {
        let api_cats = vec![ApiCategoryDesc {
            category_id: 1,
            category_name: "General".into(),
            clan_id: 1,
            category_order: 0,
        }];
        let mut channels = vec![
            {
                let mut ch = make_channel(1, "parent", "1");
                ch.clan_id = ClanId(1);
                ch
            },
            {
                let mut ch = make_channel(9, "thread-a", "1");
                ch.clan_id = ClanId(1);
                ch.parent_id = Some(ChannelId(1));
                ch.channel_type = ChannelType::Thread;
                ch
            },
            {
                let mut ch = make_channel(10, "thread-b", "1");
                ch.clan_id = ClanId(1);
                ch.parent_id = Some(ChannelId(1));
                ch.channel_type = ChannelType::Thread;
                ch
            },
        ];
        build_categories(api_cats, &mut channels)
    }

    fn make_clan(id: i64) -> crate::clan::Clan {
        crate::clan::Clan {
            id: ClanId(id),
            creator_id: UserId(0),
            name: format!("clan-{id}"),
            avatar_url: None,
            banner_url: None,
            badge_count: 0,
            has_unread: false,
            muted: false,
            welcome_channel_id: None,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
            community_banner: String::new(),
            about: String::new(),
            description: String::new(),
            short_url: String::new(),
        }
    }

    fn set_channel_badge(
        channels: &mut ChannelList,
        clan_id: ClanId,
        channel_id: ChannelId,
        count: u32,
    ) {
        let Some(cats) = channels.cache.get_mut(&clan_id) else {
            return;
        };
        for category in cats.iter_mut() {
            for channel in category.channels.iter_mut() {
                if channel.id == channel_id {
                    channel.badge_count = count;
                }
            }
        }
    }

    fn clan_badge(cx: &App, clan_id: ClanId) -> Option<u32> {
        crate::clan::ClanList::global(cx)
            .read(cx)
            .clan(clan_id)
            .map(|clan| clan.badge_count)
    }

    fn removed_from_channel(channel_id: i64, user_ids: &[i64]) -> RealtimeEvent {
        RealtimeEvent::UserChannelRemoved(mezon_proto::realtime::UserChannelRemoved {
            channel_id,
            user_ids: user_ids.to_vec(),
            channel_type: crate::threads::CHANNEL_TYPE_THREAD as i32,
            ..Default::default()
        })
    }

    fn added_to_channel(channel_id: i64, parent_id: i64, user_ids: &[i64]) -> RealtimeEvent {
        RealtimeEvent::UserChannelAdded(mezon_proto::realtime::UserChannelAdded {
            channel_desc: Some(mezon_proto::api::ChannelDescription {
                channel_id,
                parent_id,
                clan_id: 1,
                channel_label: "added".into(),
                r#type: crate::threads::CHANNEL_TYPE_THREAD as i32,
                channel_private: 1,
                ..Default::default()
            }),
            clan_id: 1,
            users: user_ids
                .iter()
                .map(|id| mezon_proto::realtime::UserProfileRedis {
                    user_id: *id,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    #[gpui::test]
    fn list_channel_by_user_id_merge_keeps_clan_structure_threads(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                let mut structure = structure_with_a_thread();
                if let Some(thread) = structure[0]
                    .channels
                    .iter_mut()
                    .find(|ch| ch.id == ChannelId(9))
                {
                    thread.name = "Tes Private thread".into();
                    thread.private = true;
                }
                channels.apply_clan_structure(ClanId(1), structure, None, cx);
                channels.merge_user_channels_from_api_descs(vec![], cx);

                let thread = channels.user_channel(ChannelId(9)).expect("private thread");
                assert_eq!(thread.name, "Tes Private thread");
                assert!(!thread.is_archived());
            });
        });
    }

    #[gpui::test]
    fn reactivated_thread_updates_user_channels_for_palette(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                let mut structure = structure_with_a_thread();
                if let Some(thread) = structure[0]
                    .channels
                    .iter_mut()
                    .find(|ch| ch.id == ChannelId(9))
                {
                    thread.name = "Tes Private thread".into();
                    thread.private = true;
                    thread.active = CHANNEL_ACTIVE_ARCHIVED;
                }
                channels.apply_clan_structure(ClanId(1), structure, None, cx);

                let archived = channels
                    .user_channel(ChannelId(9))
                    .expect("archived thread");
                assert!(archived.is_archived());

                if let Some(thread) = channels.channel_mut(ClanId(1), ChannelId(9)) {
                    thread.active = CHANNEL_ACTIVE_JOINED;
                }
                channels.upsert_user_channel_from_cache(ClanId(1), ChannelId(9), cx);

                let joined = channels
                    .user_channel(ChannelId(9))
                    .expect("reactivated thread");
                assert!(!joined.is_archived());
                assert_eq!(joined.active, CHANNEL_ACTIVE_JOINED);
            });
        });
    }

    #[gpui::test]
    fn being_added_also_lands_in_the_user_channel_list(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                assert!(channels.user_channel(ChannelId(42)).is_none());

                channels.handle_event(&added_to_channel(42, 1, &[REMOVED_SELF]), cx);

                assert!(
                    channels.user_channel(ChannelId(42)).is_some(),
                    "hashtag suggestions / forward picker / palette read user_channels, \
                     which is only fetched once per session"
                );
                assert!(channels.user_channels().any(|ch| ch.id == ChannelId(42)));
            });
        });
    }

    #[gpui::test]
    fn being_added_to_an_already_visible_channel_still_lists_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert!(channels.user_channel(ChannelId(9)).is_none());

                channels.handle_event(&added_to_channel(9, 1, &[REMOVED_SELF]), cx);

                assert!(
                    channels.user_channel(ChannelId(9)).is_some(),
                    "UserChannelAdded must upsert threads missing from clan structure"
                );
            });
        });
    }

    #[gpui::test]
    fn being_added_to_a_thread_marks_it_unread(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);

                channels.handle_event(&added_to_channel(42, 1, &[REMOVED_SELF]), cx);

                let thread = channels.user_channel(ChannelId(42)).expect("thread");
                assert!(
                    thread.is_unread(),
                    "react seeds lastSeen behind lastSent so a freshly added thread is unread"
                );
            });
        });
    }

    #[gpui::test]
    fn being_added_carries_the_channel_description_fields(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);

                let mut event = added_to_channel(42, 1, &[REMOVED_SELF]);
                if let RealtimeEvent::UserChannelAdded(ref mut e) = event
                    && let Some(desc) = e.channel_desc.as_mut()
                {
                    desc.member_count = 4;
                    desc.category_name = "General".into();
                    desc.is_mute = true;
                    desc.last_sent_message = Some(mezon_proto::api::ChannelMessageHeader {
                        id: 555,
                        timestamp_seconds: 1_700_000_000,
                        ..Default::default()
                    });
                }
                channels.handle_event(&event, cx);

                let thread = channels.user_channel(ChannelId(42)).expect("thread");
                assert_eq!(thread.member_count, 4);
                assert_eq!(thread.category_name, "General");
                assert!(thread.muted);
                assert_eq!(thread.last_sent_message_id, MessageId(555));
                assert_eq!(thread.last_sent_timestamp, 1_700_000_000);
            });
        });
    }

    #[test]
    fn seeding_an_added_thread_keeps_recent_activity_unread() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();

        let (sent, seen) = seed_added_thread_unread(0);
        assert!(sent > seen);

        let (sent, seen) = seed_added_thread_unread(now - 10);
        assert!(sent > seen);

        let (sent, seen) = seed_added_thread_unread(now - 5_000);
        assert!(sent < seen);
    }

    #[gpui::test]
    fn leaving_a_thread_drops_its_badge_from_the_clan_total(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            let clan_list = crate::clan::ClanList::global(cx);
            clan_list.update(cx, |clans, cx| {
                clans.update_clans(vec![make_clan(1)], cx);
            });
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                set_channel_badge(channels, ClanId(1), ChannelId(9), 3);
                channels.sync_clan_after_read(ClanId(1), 0, cx);
                assert_eq!(
                    clan_badge(cx, ClanId(1)),
                    Some(3),
                    "the thread's unread count must reach the clan badge first"
                );

                channels.apply_self_removed_from_channel(ChannelId(9), cx);
            });
            assert_eq!(
                clan_badge(cx, ClanId(1)),
                Some(0),
                "react decrements the clan badge by the leaving channel's unread count"
            );
        });
    }

    #[gpui::test]
    fn user_channel_list_add_and_remove_stay_symmetric(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);

                channels.handle_event(&added_to_channel(42, 1, &[REMOVED_SELF]), cx);
                channels.handle_event(&removed_from_channel(42, &[REMOVED_SELF]), cx);

                assert!(channels.user_channel(ChannelId(42)).is_none());
                assert!(!channels.user_channels().any(|ch| ch.id == ChannelId(42)));
            });
        });
    }

    #[gpui::test]
    fn removal_from_an_open_thread_falls_back_to_its_parent(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(9), cx);

                channels.handle_event(&removed_from_channel(9, &[REMOVED_SELF]), cx);

                assert_eq!(channels.active_channel_id, Some(ChannelId(1)));
                assert_eq!(
                    channels.remembered_channel(ClanId(1)),
                    Some(ChannelId(1)),
                    "the parent must also become the remembered channel so the router follows"
                );
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
            });
        });
    }

    fn channel_archive_event(
        clan_id: i64,
        channel_id: i64,
        parent_id: i64,
        active: i32,
    ) -> mezon_proto::realtime::ChannelArchiveEvent {
        mezon_proto::realtime::ChannelArchiveEvent {
            clan_id,
            channel_id,
            parent_id,
            active,
            status: 1,
            ..Default::default()
        }
    }

    fn channel_deleted_event(
        clan_id: i64,
        channel_id: i64,
        parent_id: i64,
    ) -> mezon_proto::realtime::ChannelDeletedEvent {
        mezon_proto::realtime::ChannelDeletedEvent {
            clan_id,
            channel_id,
            parent_id,
            ..Default::default()
        }
    }

    fn init_channel_list_with_threads(cx: &mut App) -> Entity<ChannelList> {
        let api = Arc::new(mezon_client::AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| {
            crate::AuthState::Authenticated(mezon_client::Session {
                user_id: REMOVED_SELF.to_string(),
                ..Default::default()
            })
        });
        crate::badge::BadgeService::init(auth_state, cx);
        crate::clan::ClanList::init(api.clone(), cx);
        let channels = ChannelList::init(api.clone(), cx);
        crate::threads::ThreadsStore::init(api, cx);
        channels
    }

    #[gpui::test]
    fn begin_archiving_blocks_duplicate_calls(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, _| {
                assert!(channels.begin_archiving(ChannelId(9)));
                assert!(!channels.begin_archiving(ChannelId(9)));
                assert!(channels.is_archiving(ChannelId(9)));
                channels.finish_archiving(ChannelId(9));
                assert!(!channels.is_archiving(ChannelId(9)));
                assert!(channels.begin_archiving(ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn channel_deleted_socket_event_applies_local_delete(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(9), cx);

                channels.handle_event(
                    &RealtimeEvent::ChannelDeleted(channel_deleted_event(1, 9, 1)),
                    cx,
                );

                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert!(channels.is_locally_deleted(ChannelId(9)));
                assert_eq!(channels.active_channel_id, Some(ChannelId(1)));
                assert_eq!(channels.remembered_channel(ClanId(1)), Some(ChannelId(1)));
            });
        });
    }

    #[gpui::test]
    fn apply_local_delete_is_idempotent(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(9), cx);
                channels.apply_local_delete(ClanId(1), ChannelId(9), ChannelId(1), cx);
                channels.apply_local_delete(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert!(channels.is_locally_deleted(ChannelId(9)));
                assert_eq!(channels.active_channel_id, Some(ChannelId(1)));
            });
        });
    }

    #[gpui::test]
    fn begin_deleting_blocks_duplicate_calls(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, _| {
                assert!(channels.begin_deleting(ChannelId(9)));
                assert!(!channels.begin_deleting(ChannelId(9)));
                assert!(channels.is_deleting(ChannelId(9)));
                channels.finish_deleting(ChannelId(9));
                assert!(!channels.is_deleting(ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn apply_local_archive_removes_thread_and_redirects_to_parent(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(9), cx);

                channels.apply_local_archive(ClanId(1), ChannelId(9), ChannelId(1), cx);

                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert_eq!(channels.active_channel_id, Some(ChannelId(1)));
            });
        });
    }

    #[gpui::test]
    fn channel_archive_socket_event_applies_local_archive(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(9), cx);

                channels.handle_event(
                    &RealtimeEvent::ChannelArchive(channel_archive_event(1, 9, 1, 0)),
                    cx,
                );

                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert_eq!(channels.active_channel_id, Some(ChannelId(1)));
            });
        });
    }

    #[gpui::test]
    fn apply_local_archive_is_idempotent(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.apply_local_archive(ClanId(1), ChannelId(9), ChannelId(1), cx);
                channels.apply_local_archive(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn parent_archive_cascade_removes_child_threads(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_parent_and_two_threads(),
                    None,
                    cx,
                );
                channels.apply_local_archive(ClanId(1), ChannelId(1), ChannelId(0), cx);
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(1)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(10)));
            });
        });
    }

    #[gpui::test]
    fn parent_delete_cascade_removes_child_threads(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_parent_and_two_threads(),
                    favor_ids(&[ChannelId(1)]),
                    cx,
                );
                channels.select_channel(ChannelId(9), cx);
                channels.apply_local_delete(ClanId(1), ChannelId(1), ChannelId(0), cx);
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(1)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(10)));
                assert!(channels.is_locally_deleted(ChannelId(1)));
                assert!(channels.is_locally_deleted(ChannelId(9)));
                assert!(channels.is_locally_deleted(ChannelId(10)));
                assert_eq!(
                    channels.deleted_channel_parent(ChannelId(9)),
                    Some(ChannelId(1))
                );
                assert!(channels.active_channel_id.is_none());
                assert!(
                    channels
                        .categories_for_clan(ClanId(1))
                        .iter()
                        .find(|cat| cat.id == FAVOR_CATE_ID)
                        .is_none_or(|cat| !cat.channels.iter().any(|ch| ch.id == ChannelId(1)))
                );
            });
        });
    }

    #[gpui::test]
    fn channel_deleted_socket_parent_cascades_children(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_parent_and_two_threads(),
                    favor_ids(&[ChannelId(1)]),
                    cx,
                );
                crate::threads::ThreadsStore::global(cx).update(cx, |store, cx| {
                    store.seed_threads_for_test(
                        "1",
                        vec![
                            crate::threads::ThreadSummary {
                                channel_id: "9".into(),
                                channel_label: "t1".into(),
                                clan_id: "1".into(),
                                parent_id: "1".into(),
                                channel_private: 0,
                                active: crate::threads::THREAD_STATUS_JOINED,
                                creator_id: "1".into(),
                                last_message_content: String::new(),
                                last_message_sender_id: String::new(),
                                last_message_sender_name: String::new(),
                                last_message_sender_avatar: String::new(),
                                last_sent_timestamp: 0,
                                member_count: 0,
                            },
                            crate::threads::ThreadSummary {
                                channel_id: "99".into(),
                                channel_label: "orphan-only-in-threads".into(),
                                clan_id: "1".into(),
                                parent_id: "1".into(),
                                channel_private: 0,
                                active: crate::threads::THREAD_STATUS_JOINED,
                                creator_id: "1".into(),
                                last_message_content: String::new(),
                                last_message_sender_id: String::new(),
                                last_message_sender_name: String::new(),
                                last_message_sender_avatar: String::new(),
                                last_sent_timestamp: 0,
                                member_count: 0,
                            },
                            crate::threads::ThreadSummary {
                                channel_id: "50".into(),
                                channel_label: "other-parent".into(),
                                clan_id: "1".into(),
                                parent_id: "2".into(),
                                channel_private: 0,
                                active: crate::threads::THREAD_STATUS_JOINED,
                                creator_id: "1".into(),
                                last_message_content: String::new(),
                                last_message_sender_id: String::new(),
                                last_message_sender_name: String::new(),
                                last_message_sender_avatar: String::new(),
                                last_sent_timestamp: 0,
                                member_count: 0,
                            },
                        ],
                        cx,
                    );
                });
                channels.handle_event(
                    &RealtimeEvent::ChannelDeleted(channel_deleted_event(1, 1, 0)),
                    cx,
                );
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(1)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(10)));
                assert!(channels.is_locally_deleted(ChannelId(9)));
                assert!(channels.is_locally_deleted(ChannelId(10)));
                assert!(!channels.ensure_channel_in_clan(ClanId(1), ChannelId(9), cx));
                assert!(
                    channels
                        .categories_for_clan(ClanId(1))
                        .iter()
                        .find(|cat| cat.id == FAVOR_CATE_ID)
                        .is_none_or(|cat| !cat.channels.iter().any(|ch| ch.id == ChannelId(1)))
                );
                let threads = crate::threads::ThreadsStore::global(cx).read(cx);
                assert!(threads.threads().iter().all(|t| t.parent_id != "1"));
                assert_eq!(threads.threads().len(), 1);
                assert_eq!(threads.threads()[0].channel_id, "50");
            });
        });
    }

    #[gpui::test]
    fn channel_deleted_socket_does_not_emit_admin_archive_toast_event(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::cell::Cell;
        use std::rc::Rc;

        let seen = Rc::new(Cell::new(false));
        let sink = seen.clone();
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            cx.subscribe(&channels, move |_, event, _| {
                if matches!(event, ChannelEvent::ArchivedByAdministrator { .. }) {
                    sink.set(true);
                }
            })
            .detach();
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_parent_and_two_threads(),
                    None,
                    cx,
                );
                channels.handle_event(
                    &RealtimeEvent::ChannelDeleted(channel_deleted_event(1, 1, 0)),
                    cx,
                );
            });
        });
        assert!(!seen.get());
    }

    #[gpui::test]
    fn archive_socket_cascade_removes_child_threads(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_parent_and_two_threads(),
                    None,
                    cx,
                );
                channels.handle_event(
                    &RealtimeEvent::ChannelArchive(channel_archive_event(1, 1, 0, 0)),
                    cx,
                );
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(1)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(10)));
            });
        });
    }

    #[gpui::test]
    fn archive_socket_emits_admin_event_for_passive_viewer(cx: &mut gpui::TestAppContext) {
        use std::cell::Cell;
        use std::rc::Rc;

        let seen = Rc::new(Cell::new(false));
        let sink = seen.clone();
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            cx.subscribe(&channels, move |_, event, _| {
                if let ChannelEvent::ArchivedByAdministrator { is_thread } = event {
                    assert!(*is_thread);
                    sink.set(true);
                }
            })
            .detach();
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(9), cx);
                let mut event = channel_archive_event(1, 9, 1, 0);
                event.creator_id = REMOVED_SELF + 1;
                channels.handle_event(&RealtimeEvent::ChannelArchive(event), cx);
            });
        });
        assert!(seen.get());
    }

    #[gpui::test]
    fn archive_socket_skips_admin_event_for_self_initiated_archive(cx: &mut gpui::TestAppContext) {
        use std::cell::Cell;
        use std::rc::Rc;

        let seen = Rc::new(Cell::new(false));
        let sink = seen.clone();
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            cx.subscribe(&channels, move |_, event, _| {
                if matches!(event, ChannelEvent::ArchivedByAdministrator { .. }) {
                    sink.set(true);
                }
            })
            .detach();
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(9), cx);
                let mut event = channel_archive_event(1, 9, 1, 0);
                event.creator_id = REMOVED_SELF;
                channels.handle_event(&RealtimeEvent::ChannelArchive(event), cx);
            });
        });
        assert!(!seen.get());
    }

    #[gpui::test]
    fn archive_channel_returns_error_when_already_in_flight(cx: &mut gpui::TestAppContext) {
        use std::sync::{Arc, Mutex};

        let outcome: Arc<Mutex<Option<Result<(), String>>>> = Arc::new(Mutex::new(None));
        let sink = outcome.clone();
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                if let Some(ch) = channels.channel_mut(ClanId(1), ChannelId(9)) {
                    ch.creator_id = UserId(REMOVED_SELF);
                }
                assert!(channels.begin_archiving(ChannelId(9)));
            });
            let task = channels.update(cx, |channels, cx| {
                channels.archive_channel(ClanId(1), ChannelId(9), cx)
            });
            cx.background_executor()
                .spawn(async move {
                    *sink.lock().expect("archive outcome lock") = Some(task.await);
                })
                .detach();
        });
        cx.run_until_parked();
        let result = outcome
            .lock()
            .expect("archive outcome lock")
            .clone()
            .expect("task should finish");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ARCHIVE_ERR_IN_PROGRESS);
        cx.update(|cx| {
            let channels = ChannelList::global(cx);
            channels.update(cx, |channels, _| {
                assert!(channels.is_archiving(ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn removal_from_a_thread_you_are_not_viewing_keeps_the_active_channel(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(1), cx);

                channels.handle_event(&removed_from_channel(9, &[REMOVED_SELF]), cx);

                assert_eq!(channels.active_channel_id, Some(ChannelId(1)));
            });
        });
    }

    #[gpui::test]
    fn removal_targeting_another_user_leaves_the_thread_in_place(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.select_channel(ChannelId(9), cx);

                channels.handle_event(&removed_from_channel(9, &[REMOVED_SELF + 1]), cx);

                assert_eq!(channels.active_channel_id, Some(ChannelId(9)));
                assert!(channels.channel_in_clan(ClanId(1), ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn refetching_a_clan_structure_keeps_favorites_without_refetching_them(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[ChannelId(2)]),
                    cx,
                );
                channels.apply_clan_extras(ClanId(1), complete_extras(), cx);
                channels.extras_loaded.insert(ClanId(1));
                assert_eq!(channels.categories_for_clan(ClanId(1))[0].channels.len(), 1);

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);

                let favor = &channels.categories_for_clan(ClanId(1))[0];
                assert_eq!(favor.id, FAVOR_CATE_ID);
                assert_eq!(
                    favor.channels.len(),
                    1,
                    "a TTL-expiry refetch must not empty the favorites category"
                );
                assert!(
                    channels.extras_loaded.contains(&ClanId(1)),
                    "the cached favorite ids survive a refetch, so no extra API call is needed"
                );
            });
        });
    }

    #[gpui::test]
    fn toggling_a_favorite_survives_a_structure_refetch(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[]),
                    cx,
                );

                channels.add_channel_favorite(ChannelId(1), ClanId(1), cx);
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert_eq!(
                    channels.categories_for_clan(ClanId(1))[0].channels.len(),
                    1,
                    "an optimistic favorite must survive a refetch"
                );

                channels.remove_channel_favorite(ChannelId(1), ClanId(1), cx);
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert!(
                    channels.categories_for_clan(ClanId(1))[0]
                        .channels
                        .is_empty(),
                    "an unfavorited channel must not come back on a refetch"
                );
            });
        });
    }

    #[gpui::test]
    fn a_rolled_back_favorite_does_not_survive_a_structure_refetch(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[]),
                    cx,
                );
                channels.extras_loaded.insert(ClanId(1));

                channels.apply_favorite_locally(ChannelId(1), ClanId(1), true, cx);
                assert_eq!(channels.categories_for_clan(ClanId(1))[0].channels.len(), 1);

                channels.apply_favorite_locally(ChannelId(1), ClanId(1), false, cx);
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);

                assert!(
                    channels.categories_for_clan(ClanId(1))[0]
                        .channels
                        .is_empty(),
                    "a favorite whose api call failed must not be re-applied from the local set"
                );
            });
        });
    }

    #[gpui::test]
    fn a_rolled_back_unfavorite_is_restored_on_a_structure_refetch(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[ChannelId(2)]),
                    cx,
                );
                channels.extras_loaded.insert(ClanId(1));

                channels.apply_favorite_locally(ChannelId(2), ClanId(1), false, cx);
                assert!(
                    channels.categories_for_clan(ClanId(1))[0]
                        .channels
                        .is_empty()
                );

                channels.apply_favorite_locally(ChannelId(2), ClanId(1), true, cx);
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);

                assert_eq!(
                    channels.categories_for_clan(ClanId(1))[0].channels.len(),
                    1,
                    "an unfavorite whose api call failed must leave the favorite in place"
                );
            });
        });
    }

    #[gpui::test]
    fn the_first_structure_apply_already_carries_favorites(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[ChannelId(2)]),
                    cx,
                );

                let favor = &channels.categories_for_clan(ClanId(1))[0];
                assert_eq!(favor.id, FAVOR_CATE_ID);
                assert_eq!(
                    favor.channels.len(),
                    1,
                    "favorites ride along with the structure, so the first paint is already correct"
                );
            });
        });
    }

    #[gpui::test]
    fn failed_favorites_fetch_keeps_favorites(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[ChannelId(2)]),
                    cx,
                );

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);

                let favor = &channels.categories_for_clan(ClanId(1))[0];
                assert_eq!(
                    favor.channels.len(),
                    1,
                    "a failed list_favorite_channels must not wipe known favorites"
                );
                assert!(
                    !channels.cache.is_fresh(&ClanId(1), crate::CACHE_TTL),
                    "a failed fetch must leave the gate open so the next clan entry retries"
                );
            });
        });
    }

    #[gpui::test]
    fn a_structure_carrying_favorites_is_stamped_fresh(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[ChannelId(2)]),
                    cx,
                );

                assert!(
                    channels.cache.is_fresh(&ClanId(1), crate::CACHE_TTL),
                    "a complete structure must be pinned for the TTL"
                );
            });
        });
    }

    #[gpui::test]
    fn favorites_carry_a_parsed_id_and_drop_an_unparseable_one(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                let parsed = parse_favorite_ids(
                    Ok(vec!["2".to_string(), "not-an-id".to_string()]),
                    ClanId(1),
                );
                assert_eq!(parsed, Some([ChannelId(2)].into_iter().collect()));

                assert_eq!(
                    parse_favorite_ids(Err(anyhow::anyhow!("socket error")), ClanId(1)),
                    None,
                    "a favorites error must map to None so the structure still paints"
                );

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), parsed, cx);
                assert_eq!(channels.categories_for_clan(ClanId(1))[0].channels.len(), 1);
            });
        });
    }

    #[gpui::test]
    fn re_entering_a_clan_after_a_failed_extras_fetch_starts_a_new_one(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.load_for_clan(ClanId(1), cx);
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[]),
                    cx,
                );
                channels.finish_extras(
                    ClanId(1),
                    ClanExtras {
                        voice_map: None,
                        app_channels: None,
                    },
                    cx,
                );
                channels.extras_loading.clear();

                assert!(channels.cache.is_fresh(&ClanId(1), crate::CACHE_TTL));
                channels.load_for_clan(ClanId(1), cx);
                assert!(
                    channels.extras_loading.contains(&ClanId(1)),
                    "re-entering the clan must issue the extras calls again"
                );

                channels.extras_loading.clear();
                channels.finish_extras(ClanId(1), complete_extras(), cx);
                channels.load_for_clan(ClanId(1), cx);
                assert!(
                    channels.extras_loading.is_empty(),
                    "once loaded, re-entering must not re-issue them"
                );
            });
        });
    }

    #[gpui::test]
    fn seed_badges_waits_on_the_in_flight_structure_fetch(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.load_for_clan(ClanId(1), cx);
                assert!(
                    channels.is_loading_clan(ClanId(1)),
                    "an in-flight structure fetch must report the clan as loading"
                );

                let listing = channels.clan_structure_ready(ClanId(1));
                assert!(
                    listing.clone().now_or_never().is_none(),
                    "seed_badges must not reach join_clan_chat while the listing is in flight"
                );

                channels.refresh_clan(ClanId(1), cx);
                assert_eq!(
                    channels.loading.len(),
                    1,
                    "a second load must reuse the in-flight fetch, not spawn a duplicate"
                );
                assert!(
                    Shared::ptr_eq(&listing, &channels.clan_structure_ready(ClanId(1))),
                    "every waiter must observe the same listing fetch"
                );
            });
        });
    }

    #[gpui::test]
    fn parked_badge_seed_survives_more_than_one_structure_refetch(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.pending_badge_seed.insert(
                    ClanId(1),
                    HashMap::from([(
                        ChannelId(1),
                        ChannelUnreadSeed {
                            badge_count: 4,
                            last_seen_timestamp: 0,
                            last_seen_message_id: MessageId(0),
                            last_sent_timestamp: 100,
                            last_sent_message_id: MessageId(5),
                        },
                    )]),
                );

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert_eq!(
                    channels.channel(ClanId(1), ChannelId(1)).unwrap().badge_count,
                    4,
                    "the first structure insert (list_channel_descs always reports 0) must pick up the parked seed"
                );
                assert!(
                    channels
                        .pending_badge_seed
                        .get(&ClanId(1))
                        .is_some_and(|seed| seed.contains_key(&ChannelId(1))),
                    "the parked seed must be retained after being applied, not consumed"
                );

                for ch in channels
                    .cache
                    .get_mut(&ClanId(1))
                    .unwrap()
                    .iter_mut()
                    .flat_map(|c| c.channels.iter_mut())
                {
                    ch.badge_count = 0;
                }
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert_eq!(
                    channels.channel(ClanId(1), ChannelId(1)).unwrap().badge_count,
                    4,
                    "a later refetch (e.g. after the clan's CACHE_TTL expires) must reapply the \
                     seed itself — the zeroed live rows prove the carry cannot mask this"
                );
            });
        });
    }

    #[gpui::test]
    fn last_seen_echo_never_repaints_the_badge_it_just_cleared(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                channels.note_channel_message(
                    ClanId(1),
                    ChannelId(1),
                    true,
                    false,
                    100,
                    MessageId(9),
                    cx,
                );
                channels.apply_read(ClanId(1), ChannelId(1), cx);
                assert_eq!(
                    channels
                        .channel(ClanId(1), ChannelId(1))
                        .unwrap()
                        .badge_count,
                    0
                );

                channels.apply_last_seen(ClanId(1), ChannelId(1), 1, 100, MessageId(9), cx);
                assert_eq!(
                    channels
                        .channel(ClanId(1), ChannelId(1))
                        .unwrap()
                        .badge_count,
                    0,
                    "LastSeenUpdated carries the badge count the writer CLEARED (React overrides \
                     lastSeenMess.badge_count with the local count for exactly this reason), so \
                     echoing our own read back must never repaint the row"
                );
                assert!(
                    !channels
                        .channel(ClanId(1), ChannelId(1))
                        .unwrap()
                        .is_unread(),
                    "the row must stay read after its own echo"
                );
            });
        });
    }

    #[gpui::test]
    fn live_channel_badges_survive_a_structure_refetch_without_any_parked_seed(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                channels.note_channel_message(ClanId(1), ChannelId(1), true, false, 100, MessageId(9), cx);
                assert_eq!(
                    channels.channel(ClanId(1), ChannelId(1)).unwrap().badge_count,
                    1,
                    "a realtime mention must bump the live badge"
                );

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert_eq!(
                    channels.channel(ClanId(1), ChannelId(1)).unwrap().badge_count,
                    1,
                    "a structure refetch (list_channel_descs always reports 0) must carry the live \
                     badge forward instead of resetting it, exactly like ClanList::carry_live_badges \
                     does for the clan-icon aggregate"
                );
            });
        });
    }

    #[gpui::test]
    fn carry_keeps_the_unread_dot_and_respects_a_fresher_server_read_state(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                {
                    let cats = channels.cache.get_mut(&ClanId(1)).unwrap();
                    for ch in cats.iter_mut().flat_map(|c| c.channels.iter_mut()) {
                        if ch.id == ChannelId(1) {
                            ch.last_sent_timestamp = 600;
                            ch.last_sent_message_id = MessageId(60);
                            ch.last_seen_timestamp = 500;
                            ch.last_seen_message_id = MessageId(50);
                        }
                    }
                }

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                let ch = channels.channel(ClanId(1), ChannelId(1)).unwrap();
                assert_eq!(
                    (ch.last_sent_timestamp, ch.last_seen_timestamp),
                    (600, 500),
                    "a refetch whose descs report zero timestamps must not erase the live \
                     unread-dot state (last_sent > last_seen)"
                );

                let mut fresher = structure_with_two_channels();
                for ch in fresher.iter_mut().flat_map(|c| c.channels.iter_mut()) {
                    if ch.id == ChannelId(1) {
                        ch.last_seen_timestamp = 700;
                        ch.last_seen_message_id = MessageId(70);
                    }
                }
                channels.apply_clan_structure(ClanId(1), fresher, None, cx);
                let ch = channels.channel(ClanId(1), ChannelId(1)).unwrap();
                assert_eq!(
                    ch.last_seen_timestamp, 700,
                    "a server-reported read state newer than the local one (read on another \
                     device, realtime missed) must win over the stale live value"
                );
            });
        });
    }

    #[gpui::test]
    fn a_mention_arriving_before_the_structure_survives_a_fresh_ts_stale_badge_seed(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.note_channel_message(
                    ClanId(1),
                    ChannelId(1),
                    true,
                    false,
                    1000,
                    MessageId(90),
                    cx,
                );

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                let ch = channels.channel(ClanId(1), ChannelId(1)).unwrap();
                assert_eq!(
                    ch.badge_count, 1,
                    "the parked mention overlay must land on the freshly inserted structure"
                );
                assert_eq!(
                    ch.last_sent_timestamp, 1000,
                    "the overlay must carry the mention's activity timestamp onto the structure, \
                     or a later seed with the same server-side timestamp outranks local knowledge"
                );

                let cats = channels.cache.get_mut(&ClanId(1)).unwrap();
                let server_row = HashMap::from([(
                    ChannelId(1),
                    ChannelUnreadSeed {
                        badge_count: 0,
                        last_seen_timestamp: 400,
                        last_seen_message_id: MessageId(40),
                        last_sent_timestamp: 900,
                        last_sent_message_id: MessageId(80),
                    },
                )]);
                apply_unread_seed_into(&server_row, cats);
                let ch = channels.channel(ClanId(1), ChannelId(1)).unwrap();
                assert_eq!(
                    ch.badge_count, 1,
                    "a seed snapshot that predates the client-witnessed mention (its last_sent \
                     is older than the overlay-carried activity) must not wipe the badge"
                );
            });
        });
    }

    #[gpui::test]
    fn a_stale_badge_seed_cannot_wipe_a_fresher_realtime_mention(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                channels.note_channel_message(
                    ClanId(1),
                    ChannelId(1),
                    true,
                    false,
                    1000,
                    MessageId(90),
                    cx,
                );
                assert_eq!(
                    channels
                        .channel(ClanId(1), ChannelId(1))
                        .unwrap()
                        .badge_count,
                    1
                );

                channels.pending_badge_seed.insert(
                    ClanId(1),
                    HashMap::from([(
                        ChannelId(1),
                        ChannelUnreadSeed {
                            badge_count: 0,
                            last_seen_timestamp: 400,
                            last_seen_message_id: MessageId(40),
                            last_sent_timestamp: 500,
                            last_sent_message_id: MessageId(50),
                        },
                    )]),
                );
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                let ch = channels.channel(ClanId(1), ChannelId(1)).unwrap();
                assert_eq!(
                    ch.badge_count, 1,
                    "a server badge snapshot older than the realtime mention (stale List* cache) \
                     must not zero the live badge"
                );
                assert_eq!(
                    ch.last_sent_timestamp, 1000,
                    "the fresher realtime activity timestamp must survive the stale seed"
                );

                let cats = channels.cache.get_mut(&ClanId(1)).unwrap();
                let stale = HashMap::from([(
                    ChannelId(1),
                    ChannelUnreadSeed {
                        badge_count: 0,
                        last_seen_timestamp: 400,
                        last_seen_message_id: MessageId(40),
                        last_sent_timestamp: 500,
                        last_sent_message_id: MessageId(50),
                    },
                )]);
                apply_unread_seed_into(&stale, cats);
                let ch = channels.channel(ClanId(1), ChannelId(1)).unwrap();
                assert_eq!(
                    ch.badge_count, 1,
                    "seed_badges direct-apply of a stale snapshot must not wipe the live mention"
                );

                let cats = channels.cache.get_mut(&ClanId(1)).unwrap();
                let equal_ts = HashMap::from([(
                    ChannelId(1),
                    ChannelUnreadSeed {
                        badge_count: 0,
                        last_seen_timestamp: 1000,
                        last_seen_message_id: MessageId(90),
                        last_sent_timestamp: 1000,
                        last_sent_message_id: MessageId(90),
                    },
                )]);
                apply_unread_seed_into(&equal_ts, cats);
                let ch = channels.channel(ClanId(1), ChannelId(1)).unwrap();
                assert_eq!(
                    ch.badge_count, 0,
                    "a seed that knows the same activity tail as the client (read on another \
                     device while offline, no newer messages) is authoritative — the fixed \
                     ListChannelBadgeCount backend counts every mention it has seen"
                );

                let cats = channels.cache.get_mut(&ClanId(1)).unwrap();
                let authoritative = HashMap::from([(
                    ChannelId(1),
                    ChannelUnreadSeed {
                        badge_count: 0,
                        last_seen_timestamp: 1100,
                        last_seen_message_id: MessageId(95),
                        last_sent_timestamp: 1100,
                        last_sent_message_id: MessageId(95),
                    },
                )]);
                apply_unread_seed_into(&authoritative, cats);
                let ch = channels.channel(ClanId(1), ChannelId(1)).unwrap();
                assert_eq!(
                    ch.badge_count, 0,
                    "a seed that knows strictly newer activity than the client stays \
                     authoritative and clears the badge"
                );
            });
        });
    }

    #[gpui::test]
    fn mark_as_read_category_evicts_the_parked_seed_so_a_refetch_cannot_resurrect_it(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.pending_badge_seed.insert(
                    ClanId(1),
                    HashMap::from([(
                        ChannelId(1),
                        ChannelUnreadSeed {
                            badge_count: 7,
                            last_seen_timestamp: 0,
                            last_seen_message_id: MessageId(0),
                            last_sent_timestamp: 100,
                            last_sent_message_id: MessageId(5),
                        },
                    )]),
                );
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert_eq!(
                    channels
                        .channel(ClanId(1), ChannelId(1))
                        .unwrap()
                        .badge_count,
                    7
                );

                channels.apply_mark_as_read_category(ClanId(1), 1, cx);
                assert_eq!(
                    channels
                        .channel(ClanId(1), ChannelId(1))
                        .unwrap()
                        .badge_count,
                    0,
                    "marking the category read must clear the live badge"
                );
                assert!(
                    !channels
                        .pending_badge_seed
                        .get(&ClanId(1))
                        .is_some_and(|seed| seed.contains_key(&ChannelId(1))),
                    "marking the category read must evict the channel from the parked seed"
                );

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert_eq!(
                    channels
                        .channel(ClanId(1), ChannelId(1))
                        .unwrap()
                        .badge_count,
                    0,
                    "a later structure refetch must not resurrect a badge whose category was \
                     already explicitly marked as read"
                );
            });
        });
    }

    #[gpui::test]
    fn collapse_all_categories_collapses_every_category_of_the_clan(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[ChannelId(2)]),
                    cx,
                );
                assert!(!channels.is_category_collapsed(ClanId(1), "1"));
                assert!(!channels.is_category_collapsed(ClanId(1), FAVOR_CATE_ID));

                channels.collapse_all_categories(ClanId(1), cx);

                assert!(channels.is_category_collapsed(ClanId(1), "1"));
                assert!(channels.is_category_collapsed(ClanId(1), FAVOR_CATE_ID));
                assert!(!channels.is_category_collapsed(ClanId(2), "1"));
            });
        });
    }

    #[gpui::test]
    fn category_name_exists_excluding_ignores_the_renamed_category_itself(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);

                assert!(channels.category_name_exists(ClanId(1), "general"));
                assert!(!channels.category_name_exists_excluding(ClanId(1), "general", "1"));
                assert!(channels.category_name_exists_excluding(ClanId(1), "general", "2"));
            });
        });
    }

    #[gpui::test]
    fn apply_category_removed_drops_the_category_and_its_channels(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[ChannelId(2)]),
                    cx,
                );
                channels.toggle_category(ClanId(1), "1", cx);
                assert!(channels.is_category_collapsed(ClanId(1), "1"));

                channels.apply_category_removed(ClanId(1), "1", cx);

                assert!(
                    channels
                        .categories_for_clan(ClanId(1))
                        .iter()
                        .all(|category| category.id != "1")
                );
                assert!(channels.channel(ClanId(1), ChannelId(1)).is_none());
                assert!(
                    channels.channel(ClanId(1), ChannelId(2)).is_none(),
                    "the favourites copy of a deleted channel must go too"
                );
                assert!(!channels.is_category_collapsed(ClanId(1), "1"));
            });
        });
    }

    fn category_event(clan_id: i64, id: i64, name: &str, status: i32) -> RealtimeEvent {
        RealtimeEvent::CategoryEvent(mezon_proto::realtime::CategoryEvent {
            clan_id,
            id,
            category_name: name.to_string(),
            status,
            ..Default::default()
        })
    }

    #[gpui::test]
    fn category_event_delete_drops_the_category_and_redirects_the_open_channel(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                channels.select_channel(ChannelId(1), cx);

                channels.handle_event(&category_event(1, 1, "", CATEGORY_EVENT_DELETED), cx);

                assert!(
                    channels
                        .categories_for_clan(ClanId(1))
                        .iter()
                        .all(|category| category.id != "1")
                );
                assert!(channels.channel(ClanId(1), ChannelId(1)).is_none());
                assert_eq!(
                    channels.active_channel_id, None,
                    "deleting the category of the open channel must clear the selection"
                );
            });
        });
    }

    #[gpui::test]
    fn category_event_update_renames_the_category_on_its_channels_too(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);

                channels.handle_event(&category_event(1, 1, "Renamed", CATEGORY_EVENT_UPDATED), cx);

                assert_eq!(
                    channels.category_name(ClanId(1), "1"),
                    Some("Renamed"),
                    "the sidebar header must follow a remote rename"
                );
                assert_eq!(
                    channels
                        .channel(ClanId(1), ChannelId(1))
                        .map(|ch| ch.category_name.as_str()),
                    Some("Renamed"),
                    "channel.category_name is read by the channel settings tabs and mention rows"
                );
            });
        });
    }

    #[gpui::test]
    fn seed_badges_does_not_wait_when_no_listing_is_in_flight(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[]),
                    cx,
                );
                channels.load_for_clan(ClanId(1), cx);

                assert!(
                    !channels.is_loading_clan(ClanId(1)),
                    "a fresh cache must not open the loading gate"
                );
                assert!(
                    channels
                        .clan_structure_ready(ClanId(1))
                        .now_or_never()
                        .is_some(),
                    "with the listing already cached the gate must resolve immediately"
                );
            });
        });
    }

    #[gpui::test]
    fn a_successful_extras_fetch_closes_the_gate(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                channels.finish_extras(ClanId(1), complete_extras(), cx);

                assert!(
                    channels.extras_loaded.contains(&ClanId(1)),
                    "a complete fetch must not be repeated on the next clan entry"
                );
            });
        });
    }

    #[gpui::test]
    fn extras_arriving_before_the_structure_stay_unloaded(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list(cx);
            channels.update(cx, |channels, cx| {
                channels.finish_extras(ClanId(1), complete_extras(), cx);
                assert!(
                    !channels.extras_loaded.contains(&ClanId(1)),
                    "extras that could not be applied must be re-requested"
                );

                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                channels.finish_extras(ClanId(1), complete_extras(), cx);
                assert!(
                    channels.extras_loaded.contains(&ClanId(1)),
                    "the retry lands once the structure is there to patch"
                );
            });
        });
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
        let mut pending: HashMap<ChannelId, PendingBadge> = HashMap::from([(
            ChannelId(10),
            PendingBadge {
                count: 1,
                ..Default::default()
            },
        )]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[0].badge_count, 1);
        assert!(pending.is_empty());
    }

    #[test]
    fn overlay_merge_takes_max_of_server_and_overlay() {
        let mut categories = categories();
        categories[0].channels[0].badge_count = 2;
        let mut pending: HashMap<ChannelId, PendingBadge> = HashMap::from([(
            ChannelId(10),
            PendingBadge {
                count: 1,
                ..Default::default()
            },
        )]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[0].badge_count, 2);

        categories[0].channels[1].badge_count = 1;
        let mut pending: HashMap<ChannelId, PendingBadge> = HashMap::from([(
            ChannelId(11),
            PendingBadge {
                count: 3,
                ..Default::default()
            },
        )]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[1].badge_count, 3);
    }

    #[test]
    fn overlay_merge_consumes_entry_so_refetch_does_not_readd() {
        let mut categories = categories();
        let mut pending: HashMap<ChannelId, PendingBadge> = HashMap::from([(
            ChannelId(10),
            PendingBadge {
                count: 1,
                ..Default::default()
            },
        )]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[0].badge_count, 1);

        categories[0].channels[0].badge_count = 0;
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(categories[0].channels[0].badge_count, 0);
    }

    fn seed_entry(badge: u32, last_seen: i64, last_sent: i64) -> ChannelUnreadSeed {
        ChannelUnreadSeed {
            badge_count: badge,
            last_seen_timestamp: last_seen,
            last_seen_message_id: MessageId(if last_seen > 0 { 100 } else { 0 }),
            last_sent_timestamp: last_sent,
            last_sent_message_id: MessageId(if last_sent > 0 { 200 } else { 0 }),
        }
    }

    #[test]
    fn favorites_rebuild_survives_a_later_extras_patch() {
        let mut categories = assemble_with_favorites(categories(), ClanId(1));
        assert_eq!(categories[0].id, FAVOR_CATE_ID);
        assert!(categories[0].channels.is_empty());

        let seed = HashMap::from([(ChannelId(10), seed_entry(4, 0, 0))]);
        apply_unread_seed_into(&seed, &mut categories);

        categories.retain(|category| category.id != FAVOR_CATE_ID);
        for ch in categories
            .iter_mut()
            .flat_map(|category| category.channels.iter_mut())
        {
            ch.is_favorite = ch.id == ChannelId(10);
        }
        let rebuilt = assemble_with_favorites(categories, ClanId(1));

        assert_eq!(rebuilt[0].id, FAVOR_CATE_ID);
        assert_eq!(rebuilt[0].channels.len(), 1);
        assert_eq!(rebuilt[0].channels[0].id, ChannelId(10));
        assert_eq!(rebuilt[0].channels[0].badge_count, 4);
        let original = rebuilt[1].channels.iter().find(|c| c.id == ChannelId(10));
        assert_eq!(original.map(|c| c.badge_count), Some(4));
    }

    #[test]
    fn unread_seed_applies_badge_and_timestamps() {
        let mut categories = categories();
        let seed = HashMap::from([(ChannelId(10), seed_entry(3, 50, 90))]);
        assert!(apply_unread_seed_into(&seed, &mut categories));

        let ch = &categories[0].channels[0];
        assert_eq!(ch.badge_count, 3);
        assert_eq!(ch.last_seen_timestamp, 50);
        assert_eq!(ch.last_seen_message_id, MessageId(100));
        assert_eq!(ch.last_sent_timestamp, 90);
        assert_eq!(ch.last_sent_message_id, MessageId(200));
        assert!(ch.is_unread());
    }

    #[test]
    fn unread_seed_marks_white_unread_without_mention() {
        let mut categories = categories();
        let seed = HashMap::from([(ChannelId(10), seed_entry(0, 50, 90))]);
        apply_unread_seed_into(&seed, &mut categories);

        let ch = &categories[0].channels[0];
        assert_eq!(ch.badge_count, 0);
        assert!(ch.is_unread());
    }

    #[test]
    fn unread_seed_ignores_zero_timestamps() {
        let mut categories = categories();
        categories[0].channels[0].last_seen_timestamp = 42;
        categories[0].channels[0].last_sent_timestamp = 77;
        let seed = HashMap::from([(ChannelId(10), seed_entry(1, 0, 0))]);
        apply_unread_seed_into(&seed, &mut categories);

        let ch = &categories[0].channels[0];
        assert_eq!(ch.badge_count, 1);
        assert_eq!(ch.last_seen_timestamp, 42);
        assert_eq!(ch.last_sent_timestamp, 77);
    }

    #[test]
    fn unread_seed_from_descs_drops_app_and_voice_channels() {
        let desc = |id: i64, channel_type: u32| ApiChannelDesc {
            channel_id: id,
            channel_label: String::new(),
            channel_type,
            clan_id: 1,
            category_name: String::new(),
            category_id: 0,
            channel_private: 0,
            count_mess_unread: 2,
            member_count: 0,
            parent_id: 0,
            is_mute: false,
            last_seen_message_id: 0,
            last_seen_timestamp: 0,
            last_sent_message_id: 0,
            last_sent_timestamp: 0,
            badge_count: 2,
            active: CHANNEL_ACTIVE_JOINED,
            creator_id: 0,
            clan_name: String::new(),
            channel_avatar: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
        };
        let seed = unread_seed_from_descs(vec![desc(10, 1), desc(11, 8), desc(12, 10)]);

        assert_eq!(seed.len(), 1);
        assert_eq!(seed[&ChannelId(10)].badge_count, 2);
        assert!(!seed.contains_key(&ChannelId(11)));
        assert!(!seed.contains_key(&ChannelId(12)));
    }

    #[test]
    fn overlay_merge_keeps_entries_for_channels_in_another_clan() {
        let mut categories = categories();
        let mut pending: HashMap<ChannelId, PendingBadge> = HashMap::from([(
            ChannelId(999),
            PendingBadge {
                count: 4,
                ..Default::default()
            },
        )]);
        merge_pending_badges_into(&mut pending, &mut categories);
        assert_eq!(pending.get(&ChannelId(999)).map(|o| o.count), Some(4));
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

    #[test]
    fn validate_category_name_trims_and_accepts_64_char_boundary() {
        let boundary = "a".repeat(CATEGORY_NAME_MAX_CHARS);

        assert_eq!(validate_category_name(&boundary).unwrap(), boundary);
        assert_eq!(
            validate_category_name("  test_2026-9-7  ").unwrap(),
            "test_2026-9-7"
        );
    }

    #[test]
    fn validate_category_name_accepts_emoji_and_vietnamese() {
        assert_eq!(validate_category_name("🎮 Games").unwrap(), "🎮 Games");
        assert_eq!(validate_category_name("Kênh chung").unwrap(), "Kênh chung");
    }

    #[test]
    fn validate_category_name_rejects_empty_long_apostrophe_or_leading_space() {
        let too_long = "a".repeat(CATEGORY_NAME_MAX_CHARS + 1);

        assert_eq!(
            validate_category_name("   "),
            Err(CreateCategoryError::InvalidName)
        );
        assert_eq!(
            validate_category_name(&too_long),
            Err(CreateCategoryError::InvalidName)
        );
        assert_eq!(
            validate_category_name("it's"),
            Err(CreateCategoryError::InvalidName)
        );
        assert_eq!(
            validate_category_name("_hidden"),
            Err(CreateCategoryError::InvalidName)
        );
    }

    #[test]
    fn validate_channel_name_accepts_and_trims() {
        assert_eq!(validate_channel_name("  general  ").unwrap(), "general");
        assert_eq!(validate_channel_name("Kênh chung").unwrap(), "Kênh chung");
    }

    #[test]
    fn validate_channel_name_rejects_invalid() {
        let too_long = "a".repeat(CATEGORY_NAME_MAX_CHARS + 1);
        assert_eq!(
            validate_channel_name(""),
            Err(CreateChannelError::InvalidName)
        );
        assert_eq!(
            validate_channel_name(&too_long),
            Err(CreateChannelError::InvalidName)
        );
        assert_eq!(
            validate_channel_name("it's"),
            Err(CreateChannelError::InvalidName)
        );
        assert_eq!(
            validate_channel_name("-lead"),
            Err(CreateChannelError::InvalidName)
        );
    }

    #[test]
    fn upsert_category_appends_new_category_without_sorting_by_order() {
        let mut cats = vec![
            Category {
                id: FAVOR_CATE_ID.into(),
                clan_id: ClanId(1),
                name: "favoriteChannel".into(),
                order: i32::MIN,
                channels: Vec::new(),
            },
            Category {
                id: "a".into(),
                clan_id: ClanId(1),
                name: "A".into(),
                order: 10,
                channels: Vec::new(),
            },
            Category {
                id: "b".into(),
                clan_id: ClanId(1),
                name: "B".into(),
                order: 1,
                channels: Vec::new(),
            },
        ];

        assert!(upsert_category(
            &mut cats,
            Category {
                id: "new".into(),
                clan_id: ClanId(1),
                name: "New".into(),
                order: 0,
                channels: Vec::new(),
            },
        ));

        let ids: Vec<&str> = cats.iter().map(|category| category.id.as_str()).collect();
        assert_eq!(ids, vec![FAVOR_CATE_ID, "a", "b", "new"]);
    }

    #[test]
    fn upsert_category_updates_existing_without_moving_or_clearing_channels() {
        let mut cats = vec![
            Category {
                id: "a".into(),
                clan_id: ClanId(1),
                name: "A".into(),
                order: 0,
                channels: vec![make_channel(10, "testa", "a")],
            },
            Category {
                id: "b".into(),
                clan_id: ClanId(1),
                name: "B".into(),
                order: 1,
                channels: Vec::new(),
            },
        ];

        assert!(upsert_category(
            &mut cats,
            Category {
                id: "a".into(),
                clan_id: ClanId(1),
                name: "Renamed".into(),
                order: 42,
                channels: Vec::new(),
            },
        ));

        let ids: Vec<&str> = cats.iter().map(|category| category.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(cats[0].name, "Renamed");
        assert_eq!(cats[0].order, 42);
        assert_eq!(cats[0].channels.len(), 1);
    }

    fn in_voice(clan: i64, channel: i64) -> InVoiceInfo {
        InVoiceInfo {
            clan_id: ClanId(clan),
            channel_id: ChannelId(channel),
        }
    }

    #[test]
    fn in_voice_joined_inserts_and_moves_between_channels() {
        let mut map = HashMap::new();
        assert!(apply_in_voice_joined(&mut map, UserId(7), in_voice(1, 10)));
        assert_eq!(map.get(&UserId(7)), Some(&in_voice(1, 10)));

        assert!(!apply_in_voice_joined(&mut map, UserId(7), in_voice(1, 10)));

        assert!(apply_in_voice_joined(&mut map, UserId(7), in_voice(2, 20)));
        assert_eq!(map.get(&UserId(7)), Some(&in_voice(2, 20)));
    }

    #[test]
    fn in_voice_joined_ignores_zero_user_id() {
        let mut map = HashMap::new();
        assert!(!apply_in_voice_joined(&mut map, UserId(0), in_voice(1, 10)));
        assert!(map.is_empty());
    }

    #[test]
    fn in_voice_leaved_removes_only_matching_channel() {
        let mut map = HashMap::new();
        apply_in_voice_joined(&mut map, UserId(7), in_voice(1, 10));

        assert!(!apply_in_voice_leaved(&mut map, UserId(7), ChannelId(99)));
        assert_eq!(map.get(&UserId(7)), Some(&in_voice(1, 10)));

        assert!(apply_in_voice_leaved(&mut map, UserId(7), ChannelId(10)));
        assert!(map.is_empty());

        assert!(!apply_in_voice_leaved(&mut map, UserId(7), ChannelId(10)));
    }

    #[test]
    fn remove_in_voice_in_channel_clears_all_users_of_channel() {
        let mut map = HashMap::new();
        apply_in_voice_joined(&mut map, UserId(7), in_voice(1, 10));
        apply_in_voice_joined(&mut map, UserId(8), in_voice(1, 10));
        apply_in_voice_joined(&mut map, UserId(9), in_voice(1, 11));

        assert!(remove_in_voice_in_channel(&mut map, ChannelId(10)));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&UserId(9)), Some(&in_voice(1, 11)));

        assert!(!remove_in_voice_in_channel(&mut map, ChannelId(10)));
    }

    #[test]
    fn user_channel_removed_only_drops_the_channel_for_the_removed_user() {
        let me = Some(UserId(7));
        assert!(event_targets_user(&[7], me));
        assert!(event_targets_user(&[8, 7, 9], me));
        assert!(!event_targets_user(&[8], me));
        assert!(!event_targets_user(&[8, 9], me));
        assert!(!event_targets_user(&[], me));
    }

    #[test]
    fn user_channel_removed_is_ignored_when_the_session_has_no_user() {
        assert!(!event_targets_user(&[7], None));
        assert!(!event_targets_user(&[], None));
    }

    #[test]
    fn remove_user_channel_drops_it_from_the_palette_source_too() {
        let mut channels = HashMap::new();
        let mut order = Vec::new();
        for id in [10, 11, 12] {
            channels.insert(ChannelId(id), make_channel(id, "general", "cat"));
            order.push(ChannelId(id));
        }

        assert!(remove_user_channel(
            &mut channels,
            &mut order,
            ChannelId(11)
        ));
        assert!(!channels.contains_key(&ChannelId(11)));
        assert_eq!(order, vec![ChannelId(10), ChannelId(12)]);

        assert!(!remove_user_channel(
            &mut channels,
            &mut order,
            ChannelId(11)
        ));
        assert_eq!(order, vec![ChannelId(10), ChannelId(12)]);
    }

    #[test]
    fn seed_in_voice_from_categories_indexes_voice_members() {
        let mut cats = categories();
        cats[0].channels[0].voice_members = vec![
            VoiceMember {
                user_id: UserId(7),
                display_name: "seven".into(),
                avatar_url: String::new(),
                sharing_screen: false,
            },
            VoiceMember {
                user_id: UserId(0),
                display_name: "zero".into(),
                avatar_url: String::new(),
                sharing_screen: false,
            },
        ];

        let mut map = HashMap::new();
        assert!(seed_in_voice_from_categories(&mut map, ClanId(1), &cats));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&UserId(7)), Some(&in_voice(1, 10)));

        assert!(!seed_in_voice_from_categories(&mut map, ClanId(1), &cats));
    }

    #[test]
    fn archived_thread_hidden_from_sidebar() {
        let mut thread = make_thread(50, 10, "cat1");
        thread.active = CHANNEL_ACTIVE_ARCHIVED;
        assert!(thread.is_archived());
        assert!(!thread.visible_in_sidebar());
        thread.active = CHANNEL_ACTIVE_JOINED;
        assert!(thread.visible_in_sidebar());
    }

    #[test]
    fn should_reactivate_only_for_archived_thread_mode() {
        let mut thread = make_thread(50, 10, "cat1");
        thread.channel_type = ChannelType::Thread;
        thread.active = CHANNEL_ACTIVE_ARCHIVED;
        assert!(should_reactivate_thread_after_send(
            6,
            &thread,
            thread_needs_reactivate(&thread)
        ));
        assert!(!should_reactivate_thread_after_send(
            2,
            &thread,
            thread_needs_reactivate(&thread)
        ));
        thread.active = CHANNEL_ACTIVE_JOINED;
        assert!(!should_reactivate_thread_after_send(
            6,
            &thread,
            thread_needs_reactivate(&thread)
        ));
        assert!(should_reactivate_thread_after_send(6, &thread, true));
        let parent = make_channel(10, "parent", "cat1");
        assert!(!should_reactivate_thread_after_send(
            6,
            &parent,
            thread_needs_reactivate(&parent)
        ));
    }

    #[test]
    fn stale_thread_reactivates_even_when_active_flag_is_joined() {
        let mut thread = make_thread(50, 10, "cat1");
        thread.channel_type = ChannelType::Thread;
        thread.active = CHANNEL_ACTIVE_JOINED;
        thread.last_sent_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("unix time")
            .as_secs() as i64
            - THREAD_ARCHIVE_DURATION_SECONDS
            - 1;
        assert!(thread_needs_reactivate(&thread));
        assert!(should_reactivate_thread_after_send(
            6,
            &thread,
            thread_needs_reactivate(&thread)
        ));
    }

    #[test]
    fn plain_channel_stays_visible_when_server_omits_active() {
        let desc = ApiChannelDesc {
            channel_id: 11,
            channel_label: "general".into(),
            channel_type: ChannelType::Text.as_raw(),
            clan_id: 1,
            category_name: String::new(),
            category_id: 7,
            channel_private: 0,
            count_mess_unread: 0,
            member_count: 0,
            parent_id: 0,
            is_mute: false,
            last_seen_message_id: 0,
            last_seen_timestamp: 0,
            last_sent_message_id: 0,
            last_sent_timestamp: 0,
            badge_count: 0,
            active: 0,
            creator_id: 0,
            clan_name: String::new(),
            channel_avatar: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
        };
        let channel = channel_from_desc(desc, 0, Vec::new(), false);
        assert!(!channel.is_archived());
        assert!(channel.visible_in_sidebar());
    }

    #[test]
    fn channel_from_desc_preserves_active_state() {
        let desc = ApiChannelDesc {
            channel_id: 10,
            channel_label: "thread".into(),
            channel_type: ChannelType::Thread.as_raw(),
            clan_id: 1,
            category_name: String::new(),
            category_id: 0,
            channel_private: 0,
            count_mess_unread: 0,
            member_count: 0,
            parent_id: 1,
            is_mute: false,
            last_seen_message_id: 0,
            last_seen_timestamp: 0,
            last_sent_message_id: 0,
            last_sent_timestamp: 0,
            badge_count: 0,
            active: CHANNEL_ACTIVE_ARCHIVED,
            creator_id: 0,
            clan_name: String::new(),
            channel_avatar: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
        };
        let channel = channel_from_desc(desc, 0, Vec::new(), false);
        assert_eq!(channel.active, CHANNEL_ACTIVE_ARCHIVED);
        assert!(channel.is_archived());
    }

    #[test]
    fn sync_thread_active_confirmed_allows_joined_to_archived() {
        let mut thread = make_thread(50, 10, "cat1");
        thread.active = CHANNEL_ACTIVE_JOINED;
        assert!(!sync_thread_active_status(
            &mut thread,
            CHANNEL_ACTIVE_ARCHIVED,
            false
        ));
        assert_eq!(thread.active, CHANNEL_ACTIVE_JOINED);
        assert!(sync_thread_active_status(
            &mut thread,
            CHANNEL_ACTIVE_ARCHIVED,
            true
        ));
        assert_eq!(thread.active, CHANNEL_ACTIVE_ARCHIVED);
        assert!(!sync_thread_active_status(
            &mut thread,
            CHANNEL_ACTIVE_ARCHIVED,
            true
        ));
        assert!(sync_thread_active_status(
            &mut thread,
            CHANNEL_ACTIVE_JOINED,
            true
        ));
        assert_eq!(thread.active, CHANNEL_ACTIVE_JOINED);
    }

    #[test]
    fn reopening_thread_already_joined_syncs_down_to_archived_when_confirmed() {
        let mut thread = make_thread(50, 10, "cat1");
        thread.channel_type = ChannelType::Thread;
        thread.active = CHANNEL_ACTIVE_JOINED;
        assert!(!thread.is_archived());
        assert!(!should_reactivate_thread_after_send(
            6,
            &thread,
            thread_needs_reactivate(&thread)
        ));

        assert!(sync_thread_active_status(
            &mut thread,
            CHANNEL_ACTIVE_ARCHIVED,
            true
        ));
        assert!(thread.is_archived());
        assert!(!thread.visible_in_sidebar());
        assert!(should_reactivate_thread_after_send(
            6,
            &thread,
            thread_needs_reactivate(&thread)
        ));
        assert!(should_reactivate_thread_after_send(6, &thread, true));
    }

    #[test]
    fn insert_archived_thread_then_reactivate_makes_visible() {
        let mut cats = categories();
        let archived = thread_channel_from_context(
            ChannelId(500),
            "old".into(),
            ClanId(1),
            ChannelId(10),
            CHANNEL_ACTIVE_ARCHIVED,
        );
        assert!(insert_channel(&mut cats, archived));
        let archived_row = cats[0]
            .channels
            .iter()
            .find(|c| c.id == ChannelId(500))
            .unwrap();
        assert!(!archived_row.visible_in_sidebar());

        if let Some(ch) = cats[0].channels.iter_mut().find(|c| c.id == ChannelId(500)) {
            ch.active = CHANNEL_ACTIVE_JOINED;
        }
        let joined = cats[0]
            .channels
            .iter()
            .find(|c| c.id == ChannelId(500))
            .unwrap();
        assert!(joined.visible_in_sidebar());
    }

    #[test]
    fn remove_channel_on_archive_drops_thread_row() {
        let mut cats = categories();
        let thread = thread_channel_from_context(
            ChannelId(500),
            "t".into(),
            ClanId(1),
            ChannelId(10),
            CHANNEL_ACTIVE_JOINED,
        );
        assert!(insert_channel(&mut cats, thread));
        assert!(remove_channel(&mut cats, ChannelId(500)));
        assert!(!cats[0].channels.iter().any(|c| c.id == ChannelId(500)));
    }

    #[test]
    fn begin_archiving_rejects_duplicate_in_flight() {
        let mut archiving = HashSet::new();
        assert!(archiving.insert(ChannelId(1)));
        assert!(!archiving.insert(ChannelId(1)));
        assert!(archiving.insert(ChannelId(2)));
    }

    #[test]
    fn archive_allowed_top_level_needs_manage_clan_not_manage_channel() {
        assert!(!archive_allowed_by_server(
            false, false, false, false, false, true
        ));
        assert!(archive_allowed_by_server(
            false, false, false, false, true, false
        ));
        assert!(archive_allowed_by_server(
            false, true, false, false, false, false
        ));
    }

    #[test]
    fn delete_allowed_accepts_manage_channel_for_top_level() {
        assert!(delete_allowed_by_server(false, false, false, false, true));
        assert!(delete_allowed_by_server(false, false, false, true, false));
        assert!(delete_allowed_by_server(true, false, false, false, false));
        assert!(!delete_allowed_by_server(false, false, false, false, false));
    }

    #[test]
    fn archive_allowed_thread_needs_manage_channel_not_creator_only() {
        assert!(archive_allowed_by_server(
            true, false, false, false, true, true
        ));
        assert!(!archive_allowed_by_server(
            true, false, false, false, false, false
        ));
        assert!(archive_allowed_by_server(
            true, true, false, false, false, false
        ));
    }

    #[test]
    fn archive_menu_hidden_for_voice_and_welcome() {
        assert!(archive_menu_hidden(ChannelType::Voice, false));
        assert!(archive_menu_hidden(ChannelType::Text, true));
        assert!(!archive_menu_hidden(ChannelType::Text, false));
    }

    #[gpui::test]
    fn parent_delete_structure_refetch_does_not_resurrect(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_parent_and_two_threads(),
                    None,
                    cx,
                );
                channels.apply_local_delete(ClanId(1), ChannelId(1), ChannelId(0), cx);
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_parent_and_two_threads(),
                    None,
                    cx,
                );
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(1)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(10)));
                assert!(channels.is_locally_deleted(ChannelId(1)));
                assert!(channels.is_locally_deleted(ChannelId(9)));
                assert!(channels.is_locally_deleted(ChannelId(10)));
            });
        });
    }

    #[gpui::test]
    fn ensure_channel_in_clan_skips_locally_deleted_thread(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.apply_local_delete(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(!channels.ensure_channel_in_clan(ClanId(1), ChannelId(9), cx));
                assert!(!channels.channel_detail_pending.contains(&ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn apply_channel_detail_after_delete_does_not_reinsert_thread(cx: &mut gpui::TestAppContext) {
        use mezon_client::transport::ApiChannelDesc;

        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.apply_local_delete(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                channels.apply_channel_detail(
                    ClanId(1),
                    ApiChannelDesc {
                        channel_id: 9,
                        channel_label: "Rust 1".into(),
                        channel_type: CHANNEL_TYPE_THREAD,
                        clan_id: 1,
                        category_name: String::new(),
                        category_id: 0,
                        channel_private: 0,
                        count_mess_unread: 0,
                        member_count: 0,
                        parent_id: 1,
                        is_mute: false,
                        last_seen_message_id: 0,
                        last_seen_timestamp: 0,
                        last_sent_message_id: 0,
                        last_sent_timestamp: 0,
                        badge_count: 0,
                        active: CHANNEL_ACTIVE_JOINED,
                        creator_id: 0,
                        clan_name: String::new(),
                        channel_avatar: String::new(),
                        topic: String::new(),
                        age_restricted: 0,
                        e2ee: 0,
                        app_id: 0,
                    },
                    cx,
                );
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn delete_thread_preserves_parent_compose_draft(cx: &mut gpui::TestAppContext) {
        use crate::compose::ComposeDraft;

        cx.update(|cx| {
            ComposeStore::init(cx);
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                ComposeStore::global(cx).update(cx, |store, _| {
                    store.set_draft(
                        ChannelId(1),
                        ComposeDraft {
                            text: "parent draft".into(),
                            ..Default::default()
                        },
                    );
                });
                channels.apply_local_delete(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(
                    ComposeStore::global(cx)
                        .read(cx)
                        .draft(ChannelId(1))
                        .is_some()
                );
            });
        });
    }

    #[gpui::test]
    fn ensure_channel_in_clan_skips_locally_archived_thread(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.apply_local_archive(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(!channels.ensure_channel_in_clan(ClanId(1), ChannelId(9), cx));
                assert!(!channels.channel_detail_pending.contains(&ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn apply_channel_detail_after_archive_does_not_reinsert_thread(cx: &mut gpui::TestAppContext) {
        use mezon_client::transport::ApiChannelDesc;

        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.apply_local_archive(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                channels.apply_channel_detail(
                    ClanId(1),
                    ApiChannelDesc {
                        channel_id: 9,
                        channel_label: "Rust 1".into(),
                        channel_type: CHANNEL_TYPE_THREAD,
                        clan_id: 1,
                        category_name: String::new(),
                        category_id: 0,
                        channel_private: 0,
                        count_mess_unread: 0,
                        member_count: 0,
                        parent_id: 1,
                        is_mute: false,
                        last_seen_message_id: 0,
                        last_seen_timestamp: 0,
                        last_sent_message_id: 0,
                        last_sent_timestamp: 0,
                        badge_count: 0,
                        active: CHANNEL_ACTIVE_JOINED,
                        creator_id: 0,
                        clan_name: String::new(),
                        channel_avatar: String::new(),
                        topic: String::new(),
                        age_restricted: 0,
                        e2ee: 0,
                        app_id: 0,
                    },
                    cx,
                );
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn archive_clears_compose_draft(cx: &mut gpui::TestAppContext) {
        use crate::compose::ComposeDraft;

        cx.update(|cx| {
            ComposeStore::init(cx);
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                ComposeStore::global(cx).update(cx, |store, _| {
                    store.set_draft(
                        ChannelId(1),
                        ComposeDraft {
                            text: "draft before archive".into(),
                            ..Default::default()
                        },
                    );
                });
                channels.apply_local_archive(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(
                    ComposeStore::global(cx)
                        .read(cx)
                        .draft(ChannelId(1))
                        .is_none()
                );
                assert!(channels.is_locally_archived(ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn should_persist_compose_draft_keeps_dm_drafts(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_authenticated_channel_list(cx);
            channels.update(cx, |channels, _| {
                let dm = Channel {
                    id: ChannelId(42),
                    name: "dm".into(),
                    channel_type: ChannelType::Unknown(2),
                    private: true,
                    clan_id: ClanId(0),
                    clan_name: String::new(),
                    category_name: String::new(),
                    category_id: None,
                    member_count: 2,
                    badge_count: 0,
                    muted: false,
                    parent_id: None,
                    last_seen_message_id: MessageId(0),
                    last_seen_timestamp: 0,
                    last_sent_message_id: MessageId(0),
                    last_sent_timestamp: 0,
                    voice_members: Vec::new(),
                    is_favorite: false,
                    creator_id: UserId(1),
                    active: CHANNEL_ACTIVE_JOINED,
                    avatar_url: String::new(),
                    topic: String::new(),
                    age_restricted: 0,
                    e2ee: 0,
                    app_id: 0,
                };
                channels.user_channels.insert(ChannelId(42), dm);
                channels.user_channels_order.push(ChannelId(42));
                assert!(channels.should_persist_compose_draft(ChannelId(42)));
            });
        });
    }

    #[gpui::test]
    fn apply_thread_reactivated_clears_local_archive_tombstone(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_a_thread(), None, cx);
                channels.apply_local_archive(ClanId(1), ChannelId(9), ChannelId(1), cx);
                assert!(channels.is_locally_archived(ChannelId(9)));
                channels.apply_thread_reactivated(ClanId(1), ChannelId(9), None, cx);
                assert!(!channels.is_locally_archived(ChannelId(9)));
                assert!(channels.channel_in_clan(ClanId(1), ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn apply_channel_detail_marks_server_archived_as_failed(cx: &mut gpui::TestAppContext) {
        use mezon_client::transport::ApiChannelDesc;

        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(ClanId(1), structure_with_two_channels(), None, cx);
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                channels.apply_channel_detail(
                    ClanId(1),
                    ApiChannelDesc {
                        channel_id: 9,
                        channel_label: "Rust 1".into(),
                        channel_type: CHANNEL_TYPE_THREAD,
                        clan_id: 1,
                        category_name: String::new(),
                        category_id: 0,
                        channel_private: 0,
                        count_mess_unread: 0,
                        member_count: 0,
                        parent_id: 1,
                        is_mute: false,
                        last_seen_message_id: 0,
                        last_seen_timestamp: 0,
                        last_sent_message_id: 0,
                        last_sent_timestamp: 0,
                        badge_count: 0,
                        active: CHANNEL_ACTIVE_ARCHIVED,
                        creator_id: 0,
                        clan_name: String::new(),
                        channel_avatar: String::new(),
                        topic: String::new(),
                        age_restricted: 0,
                        e2ee: 0,
                        app_id: 0,
                    },
                    cx,
                );
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(9)));
                assert!(channels.channel_detail_failed.contains(&ChannelId(9)));
            });
        });
    }

    #[gpui::test]
    fn top_level_archive_removes_favorite(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let channels = init_channel_list_with_threads(cx);
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    ClanId(1),
                    structure_with_two_channels(),
                    favor_ids(&[ChannelId(1)]),
                    cx,
                );
                assert!(
                    channels
                        .categories_for_clan(ClanId(1))
                        .iter()
                        .any(|cat| cat.id == FAVOR_CATE_ID
                            && cat.channels.iter().any(|ch| ch.id == ChannelId(1)))
                );
                channels.apply_local_archive(ClanId(1), ChannelId(1), ChannelId(0), cx);
                assert!(!channels.channel_in_clan(ClanId(1), ChannelId(1)));
                assert!(
                    channels
                        .categories_for_clan(ClanId(1))
                        .iter()
                        .find(|cat| cat.id == FAVOR_CATE_ID)
                        .is_none_or(|cat| !cat.channels.iter().any(|ch| ch.id == ChannelId(1)))
                );
            });
        });
    }
}
