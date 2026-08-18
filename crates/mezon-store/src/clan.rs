use crate::channel::ChannelList;
use crate::config::AppConfig;
use crate::ids::{ChannelId, ClanId, UserId};
use crate::ogp::trusted_invite_id;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::transport::ApiClanDesc;
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::api::{self, SystemMessage, SystemMessageRequest, UpdateClanDescRequest};

use crate::realtime::{RealtimeDispatch, RealtimeKind};

pub const MAX_CLAN_LOGO_BYTES: u64 = 1_000_000;
pub const MAX_CLAN_BANNER_BYTES: u64 = 10_000_000;
pub const MAX_COMMUNITY_BANNER_BYTES: u64 = MAX_CLAN_BANNER_BYTES;
pub const MAX_COMMUNITY_ABOUT_CHARS: usize = 100;
pub const MAX_COMMUNITY_DESCRIPTION_CHARS: usize = 300;
pub const MAX_COMMUNITY_SHORT_URL_CHARS: usize = 50;
pub const MAX_COMMUNITY_BANNER_URL_BYTES: usize = 2048;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OnboardingAnswer {
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OnboardingContent {
    pub guide_type: i32,
    pub task_type: i32,
    pub channel_id: i64,
    pub title: String,
    pub content: String,
    pub image_url: String,
    pub answers: Vec<OnboardingAnswer>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OnboardingItem {
    pub id: i64,
    pub guide_type: i32,
    pub task_type: i32,
    pub channel_id: i64,
    pub title: String,
    pub content: String,
    pub image_url: String,
    pub answers: Vec<OnboardingAnswer>,
}

impl From<api::OnboardingAnswer> for OnboardingAnswer {
    fn from(answer: api::OnboardingAnswer) -> Self {
        Self {
            title: answer.title,
            description: answer.description,
        }
    }
}

impl From<OnboardingAnswer> for api::OnboardingAnswer {
    fn from(answer: OnboardingAnswer) -> Self {
        Self {
            title: answer.title,
            description: answer.description,
            ..Default::default()
        }
    }
}

impl From<api::OnboardingItem> for OnboardingItem {
    fn from(item: api::OnboardingItem) -> Self {
        Self {
            id: item.id,
            guide_type: item.guide_type,
            task_type: item.task_type,
            channel_id: item.channel_id,
            title: item.title,
            content: item.content,
            image_url: item.image_url,
            answers: item.answers.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<OnboardingContent> for api::OnboardingContent {
    fn from(content: OnboardingContent) -> Self {
        Self {
            guide_type: content.guide_type,
            task_type: content.task_type,
            channel_id: content.channel_id,
            title: content.title,
            content: content.content,
            image_url: content.image_url,
            answers: content.answers.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClanImageMimeType {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl ClanImageMimeType {
    pub const ALLOWED_EXTENSIONS: &'static [&'static str] = &["jpg", "jpeg", "png", "gif", "webp"];

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }

    pub fn is_allowed_extension(ext: &str) -> bool {
        Self::from_extension(ext).is_some()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

fn clan_image_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

pub fn validate_clan_image_file(path: &Path, max_bytes: u64) -> Result<ClanImageMimeType, String> {
    let ext =
        clan_image_extension(path).ok_or_else(|| "Unsupported image file type".to_string())?;
    let mime_type = ClanImageMimeType::from_extension(&ext)
        .ok_or_else(|| "Unsupported image file type".to_string())?;
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len == 0 {
        return Err("File is empty".into());
    }
    if len > max_bytes {
        return Err(format!("File exceeds {max_bytes}-byte limit ({len} bytes)"));
    }
    Ok(mime_type)
}

#[derive(Debug, Clone)]
pub struct Clan {
    pub id: ClanId,
    pub creator_id: UserId,
    pub name: String,
    pub avatar_url: Option<String>,
    pub banner_url: Option<String>,
    pub badge_count: u32,
    pub has_unread: bool,
    pub muted: bool,
    pub welcome_channel_id: Option<ChannelId>,
    pub status: i32,
    pub is_onboarding: bool,
    pub is_community: bool,
    pub prevent_anonymous: bool,
    pub community_banner: String,
    pub about: String,
    pub description: String,
    pub short_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClanInviteLink {
    pub id: i64,
    pub invite_link: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClanSystemMessage {
    pub channel_id: ChannelId,
    pub welcome_random: bool,
    pub welcome_sticker: bool,
    pub boost_message: bool,
    pub setup_tips: bool,
    pub hide_audit_log: bool,
}

impl ClanSystemMessage {
    pub fn from_proto(msg: SystemMessage) -> Self {
        Self {
            channel_id: ChannelId(msg.channel_id),
            welcome_random: msg.welcome_random == "1",
            welcome_sticker: msg.welcome_sticker == "1",
            boost_message: msg.boost_message == "1",
            setup_tips: msg.setup_tips == "1",
            hide_audit_log: msg.hide_audit_log,
        }
    }

    fn into_request(self, clan_id: ClanId) -> SystemMessageRequest {
        SystemMessageRequest {
            clan_id: clan_id.get(),
            channel_id: self.channel_id.get(),
            welcome_random: bool_flag(self.welcome_random),
            welcome_sticker: bool_flag(self.welcome_sticker),
            boost_message: bool_flag(self.boost_message),
            setup_tips: bool_flag(self.setup_tips),
            hide_audit_log: self.hide_audit_log,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommunityInfo {
    pub is_community: bool,
    pub community_banner: String,
    pub about: String,
    pub description: String,
    pub short_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommunityDraft {
    pub community_banner: String,
    pub about: String,
    pub description: String,
    pub short_url: String,
}

impl CommunityDraft {
    pub fn from_info(info: &CommunityInfo) -> Self {
        Self {
            community_banner: info.community_banner.clone(),
            about: info.about.clone(),
            description: info.description.clone(),
            short_url: info.short_url.clone(),
        }
    }

    pub fn has_changes(&self, saved: &Self) -> bool {
        self != saved
    }

    /// Normalize and bound user-controlled community fields before send/persist.
    pub fn sanitized(mut self) -> Self {
        self.about = truncate_chars(self.about.trim(), MAX_COMMUNITY_ABOUT_CHARS);
        self.description = truncate_chars(self.description.trim(), MAX_COMMUNITY_DESCRIPTION_CHARS);
        self.short_url = sanitize_community_short_url(&self.short_url);
        self.community_banner = sanitize_community_banner_url(&self.community_banner);
        self
    }

    pub fn is_valid(&self) -> bool {
        let draft = self.clone().sanitized();
        !draft.community_banner.is_empty()
            && !draft.about.is_empty()
            && !draft.description.is_empty()
            && !draft.short_url.is_empty()
    }
}

impl From<&Clan> for CommunityInfo {
    fn from(clan: &Clan) -> Self {
        Self {
            is_community: clan.is_community,
            community_banner: clan.community_banner.clone(),
            about: clan.about.clone(),
            description: clan.description.clone(),
            short_url: clan.short_url.clone(),
        }
    }
}

impl From<ApiClanDesc> for CommunityInfo {
    fn from(desc: ApiClanDesc) -> Self {
        Self {
            is_community: desc.is_community,
            community_banner: desc.community_banner,
            about: desc.about,
            description: desc.description,
            short_url: desc.short_url,
        }
    }
}

pub fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

pub fn sanitize_community_short_url(raw: &str) -> String {
    truncate_chars(
        &raw.to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>(),
        MAX_COMMUNITY_SHORT_URL_CHARS,
    )
}

fn sanitize_community_banner_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.len() > MAX_COMMUNITY_BANNER_URL_BYTES {
        return String::new();
    }
    // Only accept http(s) URLs produced by our upload/CDN path — reject schemes like javascript:.
    let Ok(url) = url::Url::parse(trimmed) else {
        return String::new();
    };
    if !matches!(url.scheme(), "https" | "http") {
        return String::new();
    }
    trimmed.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClanOverviewDraft {
    pub clan_name: String,
    pub logo: String,
    pub banner: String,
    pub welcome_channel_id: Option<ChannelId>,
    pub prevent_anonymous: bool,
}

impl ClanOverviewDraft {
    fn update_request(&self, clan_id: ClanId, clan: &Clan) -> UpdateClanDescRequest {
        UpdateClanDescRequest {
            clan_id: clan_id.get(),
            clan_name: self.clan_name.trim().to_string(),
            logo: Some(self.logo.clone()),
            banner: Some(self.banner.clone()),
            prevent_anonymous: self.prevent_anonymous,
            welcome_channel_id: proto_channel_id(
                self.welcome_channel_id.or(clan.welcome_channel_id),
            ),
            ..Default::default()
        }
    }

    fn clan_update(&self, clan: &Clan, name: String) -> ClanUpdate {
        ClanUpdate {
            name: Some(name),
            logo: self.logo.clone(),
            banner: self.banner.clone(),
            welcome_channel_id: self.welcome_channel_id.or(clan.welcome_channel_id),
            status: clan.status,
            is_onboarding: clan.is_onboarding,
            is_community: clan.is_community,
            prevent_anonymous: self.prevent_anonymous,
            // Overview save must not wipe community profile fields.
            community: None,
        }
    }
}

fn carry_live_badges(previous: &[Clan], next: &mut [Clan]) {
    if previous.is_empty() {
        return;
    }
    let live: std::collections::HashMap<ClanId, (u32, bool)> = previous
        .iter()
        .map(|clan| (clan.id, (clan.badge_count, clan.has_unread)))
        .collect();
    for clan in next {
        if let Some(&(badge_count, has_unread)) = live.get(&clan.id) {
            clan.badge_count = badge_count;
            clan.has_unread = has_unread;
        }
    }
}

fn proto_channel_id(id: Option<ChannelId>) -> i64 {
    id.map(|id| id.get())
        .filter(|id| *id != 0)
        .unwrap_or_default()
}

fn bool_flag(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

impl From<ApiClanDesc> for Clan {
    fn from(c: ApiClanDesc) -> Self {
        let avatar_url = (!c.logo.is_empty()).then_some(c.logo);
        let banner_url = (!c.banner.is_empty()).then_some(c.banner);
        let welcome_channel_id =
            (c.welcome_channel_id != 0).then_some(ChannelId(c.welcome_channel_id));
        Self {
            id: ClanId(c.clan_id),
            creator_id: UserId(c.creator_id),
            name: c.clan_name,
            avatar_url,
            banner_url,
            badge_count: 0,
            has_unread: false,
            muted: false,
            welcome_channel_id,
            status: c.status,
            is_onboarding: c.is_onboarding,
            is_community: c.is_community,
            prevent_anonymous: c.prevent_anonymous,
            community_banner: c.community_banner,
            about: c.about,
            description: c.description,
            short_url: c.short_url,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptInviteResult {
    pub clan_id: ClanId,
    pub channel_id: ChannelId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptInviteError {
    AlreadyJoining,
    InvalidLink,
    Failed(String),
}

/// Typed events emitted by [`ClanList`] — the analog of Zed's `ChannelEvent`
/// (`channel_store.rs:144`). Other stores/views `cx.subscribe` to react to specific changes.
#[derive(Debug, Clone)]
pub enum ClanEvent {
    /// The active clan changed (or was cleared).
    ActiveClanChanged(Option<ClanId>),
    /// A clan was removed (server push).
    Deleted(ClanId),
}

/// Clan store — owns the clan list, fetches it over REST, and self-subscribes to realtime
/// clan events.
///
/// Native analog of Zed's `ChannelStore` (`crates/channel/src/channel_store.rs`): registered as
/// a [`Global`] (`init`/`global`), an [`EventEmitter`] of [`ClanEvent`], reacting to server
/// pushes in `handle_event`, holding its subscription `Task` so it cancels on drop.
pub struct ClanList {
    pub clans: Vec<Clan>,
    pub active_clan_id: Option<ClanId>,
    api: Arc<AppApi>,
    loading: bool,
    badges_loaded: bool,
    reload_pending: bool,
    reset_generation: u64,
    saved_order: Vec<ClanId>,
    joining_invite_urls: HashSet<String>,
    invite_join_failed_urls: HashSet<String>,
    _connection_watch: Task<()>,
    _saved_order_load: Task<()>,
}

struct GlobalClanList(Entity<ClanList>);
impl Global for GlobalClanList {}

impl EventEmitter<ClanEvent> for ClanList {}

impl ClanList {
    /// Create the store and register it as the app-wide global. Cf. `ChannelStore::init`
    /// (`channel_store.rs:25`). Call once during app setup, before any view reads it.
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalClanList(entity.clone()));
        entity
    }

    /// The global clan store. Panics if [`ClanList::init`] hasn't run. Cf. `ChannelStore::global`.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalClanList>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalClanList>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.clans.clear();
        self.loading = false;
        self.badges_loaded = false;
        self.reload_pending = false;
        self.joining_invite_urls.clear();
        self.invite_join_failed_urls.clear();
        if self.active_clan_id.take().is_some() {
            cx.emit(ClanEvent::ActiveClanChanged(None));
        }
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let connection_watch = Self::spawn_connection_watch(api.clone(), cx);
        let saved_order_load = cx.spawn(async move |this, cx| {
            let order = cx
                .background_executor()
                .spawn(async { crate::Settings::load_sync().clan_order })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.saved_order = order;
                if this.apply_saved_order_internal() {
                    cx.notify();
                }
            });
        });
        Self {
            clans: Vec::new(),
            active_clan_id: None,
            api,
            loading: false,
            badges_loaded: false,
            reload_pending: false,
            reset_generation: 0,
            saved_order: Vec::new(),
            joining_invite_urls: HashSet::new(),
            invite_join_failed_urls: HashSet::new(),
            _connection_watch: connection_watch,
            _saved_order_load: saved_order_load,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ClanUpdated,
                RealtimeKind::ClanDeleted,
                RealtimeKind::AddClanUser,
                RealtimeKind::UserClanRemoved,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, cx| {
                tracing::warn!("ClanList realtime lagged — reloading clans");
                this.reload(cx);
            });
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
                    // Reconnected — realtime pushes were missed while offline, so the cached list
                    // is stale: always refetch (not just when empty).
                    if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.reload_pending = true;
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        let generation = self.reset_generation;
        let fetch_badges = !self.badges_loaded;
        cx.spawn(async move |this, cx| {
            const MAX_RETRIES: u32 = 3;
            let mut attempt = 0u32;
            let (clans, badges_result) = loop {
                let (clans_result, badges_result) = if fetch_badges {
                    let (clans, badges) =
                        tokio::join!(api.list_clan_descs(), api.list_clan_badge_count());
                    (clans, Some(badges))
                } else {
                    (api.list_clan_descs().await, None)
                };
                match clans_result {
                    Ok(c) => break (c, badges_result),
                    Err(e) if attempt < MAX_RETRIES => {
                        attempt += 1;
                        tracing::warn!("Failed to load clans (attempt {attempt}): {e}, retrying");
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2 * attempt as u64))
                            .await;
                    }
                    Err(e) => {
                        tracing::error!("Failed to load clans after {attempt} retries: {e}");
                        let _ = this.update(cx, |this, _| {
                            if this.reset_generation == generation {
                                this.loading = false;
                            }
                        });
                        return;
                    }
                }
            };
            let mut badges_fetched = false;
            let badge_map: std::collections::HashMap<String, (i32, bool)> = match badges_result {
                Some(Ok(list)) => {
                    badges_fetched = true;
                    list.into_iter()
                        .map(|(id, badge, has_unread)| (id, (badge, has_unread)))
                        .collect()
                }
                Some(Err(e)) => {
                    tracing::warn!("clan badge count fetch failed: {e}");
                    std::collections::HashMap::new()
                }
                None => std::collections::HashMap::new(),
            };
            let mapped: Vec<Clan> = clans
                .into_iter()
                .map(|c| {
                    let mut clan = Clan::from(c);
                    if let Some(&(badge, has_unread)) = badge_map.get(&clan.id.to_string()) {
                        clan.badge_count = badge.max(0) as u32;
                        clan.has_unread = has_unread;
                    }
                    clan
                })
                .collect();
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.loading = false;
                this.badges_loaded = this.badges_loaded || badges_fetched;
                this.update_clans(mapped, cx);
                if let Some(clan_id) = this.active_clan_id {
                    this.fire_join_clan_chat(clan_id, cx);
                } else if let Some(clan_id) = this.clans.first().map(|clan| clan.id) {
                    this.fire_join_clan_chat(clan_id, cx);
                }
                if this.reload_pending {
                    this.reload_pending = false;
                    this.reload(cx);
                }
            });
        })
        .detach();
    }

    /// Apply a server-pushed realtime event. Cf. `ChannelStore::handle_update_channels`.
    pub(crate) fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::ClanDeleted(e) => {
                self.drop_clan(ClanId(e.clan_id), cx);
            }
            RealtimeEvent::ClanUpdated(e) => {
                let name = (!e.clan_name.is_empty()).then_some(e.clan_name.clone());
                let welcome_channel_id =
                    (e.welcome_channel_id != 0).then_some(ChannelId(e.welcome_channel_id));
                let update = ClanUpdate {
                    name,
                    logo: e.logo.clone(),
                    banner: e.banner.clone(),
                    welcome_channel_id,
                    status: e.status,
                    is_onboarding: e.is_onboarding,
                    is_community: e.is_community,
                    prevent_anonymous: e.prevent_anonymous,
                    community: None,
                };
                if update_clan(&mut self.clans, ClanId(e.clan_id), update) {
                    cx.notify();
                }
            }
            RealtimeEvent::AddClanUser(e) => {
                let id = ClanId(e.clan_id);
                if !self.clans.iter().any(|c| c.id == id) {
                    self.reload(cx);
                }
            }
            RealtimeEvent::UserClanRemoved(e) => {
                let id = ClanId(e.clan_id);
                let me = crate::badge::BadgeService::try_global(cx)
                    .and_then(|badges| badges.read(cx).current_user_id(cx));
                if !crate::event_targets_user(&e.user_ids, me) {
                    return;
                }
                self.drop_clan(id, cx);
            }
            _ => {}
        }
    }

    fn drop_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let before = self.clans.len();
        self.clans.retain(|c| c.id != clan_id);
        if self.clans.len() == before {
            return;
        }
        if self.active_clan_id == Some(clan_id) {
            self.active_clan_id = None;
            cx.emit(ClanEvent::ActiveClanChanged(None));
        }
        cx.emit(ClanEvent::Deleted(clan_id));
        cx.notify();
    }

    pub fn leave_clan(
        &mut self,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let Some(user_id) = crate::badge::BadgeService::try_global(cx)
            .and_then(|badges| badges.read(cx).current_user_id(cx))
        else {
            return Task::ready(Err("no signed-in user".to_string()));
        };
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            api.remove_clan_users(clan_id.get(), vec![user_id.get().to_string()])
                .await
                .map_err(|e| e.to_string())?;
            this.update(cx, |this, cx| this.drop_clan(clan_id, cx))
                .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn delete_clan(
        &mut self,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            api.delete_clan_desc(clan_id.get())
                .await
                .map_err(|e| e.to_string())?;
            this.update(cx, |this, cx| this.drop_clan(clan_id, cx))
                .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn set_has_unread_message(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && !clan.muted
        {
            let was_unread = clan.has_unread;
            clan.has_unread = true;
            if !was_unread {
                cx.notify();
            }
        }
    }

    pub fn increment_clan_badge(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && !clan.muted
        {
            let was_badge = clan.badge_count;
            clan.badge_count = clan.badge_count.saturating_add(1);
            if was_badge != clan.badge_count {
                cx.notify();
            }
        }
    }

    pub fn set_has_unread(&mut self, clan_id: ClanId, has_unread: bool, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && !clan.muted
            && clan.has_unread != has_unread
        {
            clan.has_unread = has_unread;
            cx.notify();
        }
    }

    pub fn sync_has_unread_from_channels(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let channel_list = ChannelList::global(cx).read(cx);
        if !channel_list.is_clan_cache_loaded(clan_id) {
            return;
        }
        let has_unread = channel_list.clan_has_any_unread(clan_id);
        self.set_has_unread(clan_id, has_unread, cx);
    }

    pub fn decrement_badge(&mut self, clan_id: ClanId, amount: u32, cx: &mut Context<Self>) {
        if amount == 0 {
            return;
        }
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id) {
            let was_badge = clan.badge_count;
            clan.badge_count = clan.badge_count.saturating_sub(amount);
            if was_badge != clan.badge_count {
                cx.notify();
            }
        }
    }

    pub fn set_badge_count(&mut self, clan_id: ClanId, count: u32, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id) {
            let count = if clan.muted { 0 } else { count };
            if clan.badge_count != count {
                clan.badge_count = count;
                cx.notify();
            }
        }
    }

    pub fn apply_badge_read(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && (clan.badge_count > 0 || clan.has_unread)
        {
            clan.badge_count = 0;
            clan.has_unread = false;
            cx.notify();
        }
    }

    pub fn active_clan(&self) -> Option<&Clan> {
        self.active_clan_id
            .as_ref()
            .and_then(|id| self.clans.iter().find(|c| c.id == *id))
    }

    pub fn clan(&self, clan_id: ClanId) -> Option<&Clan> {
        self.clan_by_id(clan_id)
    }

    pub fn active_clan_banner(&self) -> Option<&str> {
        self.active_clan().and_then(|c| c.banner_url.as_deref())
    }

    pub fn is_active_clan(&self, clan_id: ClanId) -> bool {
        self.active_clan_id == Some(clan_id)
    }

    pub fn welcome_channel_id(&self, clan_id: ClanId) -> Option<ChannelId> {
        self.clans
            .iter()
            .find(|c| c.id == clan_id)
            .and_then(|c| c.welcome_channel_id)
    }

    fn fire_join_clan_chat(&self, clan_id: ClanId, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let id = clan_id.get();
        cx.spawn(async move |_, _| {
            if let Err(e) = api.join_clan_chat(id).await {
                tracing::error!("join_clan_chat failed for clan {id}: {e}");
            }
        })
        .detach();
    }

    pub fn subscribe_clan_realtime(&self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.fire_join_clan_chat(clan_id, cx);
    }

    pub fn is_joining_invite(&self, invite_url: &str) -> bool {
        self.joining_invite_urls.contains(invite_url)
    }

    pub fn has_invite_join_error(&self, invite_url: &str) -> bool {
        self.invite_join_failed_urls.contains(invite_url)
    }

    pub fn accept_invite_link(
        &mut self,
        invite_url: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<AcceptInviteResult, AcceptInviteError>> {
        if self.joining_invite_urls.contains(&invite_url) {
            return Task::ready(Err(AcceptInviteError::AlreadyJoining));
        }
        let internal_domain = AppConfig::try_global(cx).map(|cfg| cfg.domain_url.as_str());
        let Some(invite_id) = trusted_invite_id(&invite_url, internal_domain) else {
            return Task::ready(Err(AcceptInviteError::InvalidLink));
        };
        self.joining_invite_urls.insert(invite_url.clone());
        self.invite_join_failed_urls.remove(&invite_url);
        cx.notify();
        let api = self.api.clone();
        let generation = self.reset_generation;
        let invite_url_for_task = invite_url;
        cx.spawn(async move |this, cx| {
            let result = api
                .invite_user(invite_id)
                .await
                .map_err(|e| AcceptInviteError::Failed(e.to_string()))
                .and_then(|res| {
                    if res.clan_id == 0 {
                        Err(AcceptInviteError::Failed("cannot join clan".into()))
                    } else {
                        Ok(AcceptInviteResult {
                            clan_id: ClanId(res.clan_id),
                            channel_id: ChannelId(res.channel_id),
                        })
                    }
                });
            let _ = this.update(cx, |this, cx| {
                this.joining_invite_urls.remove(&invite_url_for_task);
                if this.reset_generation != generation {
                    return;
                }
                match &result {
                    Ok(accept) => {
                        this.invite_join_failed_urls.remove(&invite_url_for_task);
                        this.select_clan(accept.clan_id, cx);
                        this.fire_join_clan_chat(accept.clan_id, cx);
                        this.reload(cx);
                    }
                    Err(AcceptInviteError::AlreadyJoining) => {}
                    Err(_) => {
                        this.invite_join_failed_urls.insert(invite_url_for_task);
                    }
                }
                cx.notify();
            });
            result
        })
    }

    pub fn select_clan(&mut self, id: ClanId, cx: &mut Context<Self>) {
        if self.active_clan_id == Some(id) {
            return;
        }
        self.active_clan_id = Some(id);
        cx.emit(ClanEvent::ActiveClanChanged(self.active_clan_id));
        cx.notify();
    }

    pub fn clear_active_clan(&mut self, cx: &mut Context<Self>) {
        if self.active_clan_id.take().is_some() {
            cx.emit(ClanEvent::ActiveClanChanged(None));
            cx.notify();
        }
    }

    pub fn update_clans(&mut self, mut clans: Vec<Clan>, cx: &mut Context<Self>) {
        let prev_active = self.active_clan_id;
        carry_live_badges(&self.clans, &mut clans);
        self.clans = clans;
        self.apply_saved_order_internal();
        let active_missing = self
            .active_clan_id
            .is_some_and(|active| !self.clans.iter().any(|c| c.id == active));
        if active_missing {
            self.active_clan_id = None;
        }
        if self.active_clan_id != prev_active {
            cx.emit(ClanEvent::ActiveClanChanged(self.active_clan_id));
        }
        cx.notify();
    }

    pub fn create_clan(
        &mut self,
        name: String,
        logo: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, CreateClanError>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let trimmed = name.trim().to_string();
            let is_dup = api
                .check_duplicate_clan_name(&trimmed, "0")
                .await
                .map_err(|e| CreateClanError::Other(e.to_string()))?;
            if is_dup {
                return Err(CreateClanError::DuplicateName);
            }
            let desc = api
                .create_clan_desc(&trimmed, &logo, "")
                .await
                .map_err(|e| CreateClanError::Other(e.to_string()))?;
            let clan_id = desc.clan_id;
            this.update(cx, |this, cx| {
                apply_created_clan(&mut this.clans, desc);
                this.select_clan(ClanId(clan_id), cx);
            })
            .map_err(|_| CreateClanError::Other("store dropped".into()))?;
            Ok(clan_id.to_string())
        })
    }

    pub fn upload_clan_image(
        &self,
        path: &Path,
        max_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, String>> {
        let api = self.api.clone();
        let path = path.to_path_buf();
        let base_img_url = AppConfig::global(cx).base_img_url.clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .spawn(
                    async move { upload_image_to_cdn(&api, &base_img_url, &path, max_bytes).await },
                )
                .await
        })
    }

    pub fn reorder_clans(&mut self, order: Vec<ClanId>, cx: &mut Context<Self>) {
        apply_clan_order(&mut self.clans, &order);
        self.saved_order = order.clone();
        cx.notify();
        cx.background_executor()
            .spawn(async move {
                let mut settings = crate::Settings::load_sync();
                settings.clan_order = order;
                settings.save_sync();
            })
            .detach();
    }

    pub fn move_clan(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let Some(order) = moved_clan_order(&self.clans, from, to) else {
            return;
        };
        self.reorder_clans(order, cx);
    }

    pub fn apply_saved_order(&mut self, order: &[ClanId]) {
        self.saved_order = order.to_vec();
        apply_clan_order(&mut self.clans, order);
    }

    fn apply_saved_order_internal(&mut self) -> bool {
        if self.saved_order.is_empty() || self.clans.is_empty() {
            return false;
        }
        let order = std::mem::take(&mut self.saved_order);
        apply_clan_order(&mut self.clans, &order);
        self.saved_order = order;
        true
    }

    pub fn clan_by_id(&self, clan_id: ClanId) -> Option<&Clan> {
        self.clans.iter().find(|c| c.id == clan_id)
    }

    pub fn fetch_system_message(
        &self,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<ClanSystemMessage, String>> {
        let api = self.api.clone();
        let id = clan_id.get();
        cx.spawn(async move |_, _| {
            let msg = api
                .get_system_message_by_clan_id(id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(ClanSystemMessage::from_proto(msg))
        })
    }

    pub fn save_clan_overview(
        &mut self,
        clan_id: ClanId,
        draft: ClanOverviewDraft,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        let clan = self.clans.iter().find(|c| c.id == clan_id).cloned();
        let Some(clan) = clan else {
            return cx.spawn(async move |_, _| Err("clan not found".into()));
        };
        let request = draft.update_request(clan_id, &clan);
        let trimmed_name = draft.clan_name.trim().to_string();
        let previous_name = clan.name.clone();
        let local_update = draft.clan_update(&clan, trimmed_name.clone());
        cx.spawn(async move |this, cx| {
            if trimmed_name != previous_name.trim() {
                let is_duplicate = api
                    .check_duplicate_clan_name(&trimmed_name, "0")
                    .await
                    .map_err(|e| e.to_string())?;
                if is_duplicate {
                    return Err("Duplicate clan name".into());
                }
            }

            api.update_clan_desc(request)
                .await
                .map_err(|e| e.to_string())?;

            this.update(cx, |this, cx| {
                let _ = update_clan(&mut this.clans, clan_id, local_update);
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn save_system_message(
        &self,
        clan_id: ClanId,
        draft: ClanSystemMessage,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        let request = draft.into_request(clan_id);
        cx.spawn(async move |_, _| {
            api.update_system_message(request)
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn fetch_community_info(
        &self,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<CommunityInfo, String>> {
        // Prefer in-memory clan data from the initial list fetch — avoid re-listing all clans.
        if let Some(clan) = self.clans.iter().find(|c| c.id == clan_id) {
            let info = CommunityInfo::from(clan);
            return cx.spawn(async move |_, _| Ok(info));
        }
        let api = self.api.clone();
        let id = clan_id.get();
        cx.spawn(async move |_, _| {
            let descs = api.list_clan_descs().await.map_err(|e| e.to_string())?;
            descs
                .into_iter()
                .find(|desc| desc.clan_id == id)
                .map(CommunityInfo::from)
                .ok_or_else(|| "clan not found".into())
        })
    }

    pub fn fetch_onboarding(
        &self,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<OnboardingItem>, String>> {
        let api = self.api.clone();
        cx.spawn(async move |_, _| {
            api.list_onboarding(clan_id.get(), 100, 1)
                .await
                .map(|response| {
                    response
                        .list_onboarding
                        .into_iter()
                        .map(Into::into)
                        .collect()
                })
                .map_err(|error| error.to_string())
        })
    }

    pub fn create_onboarding_items(
        &self,
        clan_id: ClanId,
        contents: Vec<OnboardingContent>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<OnboardingItem>, String>> {
        let api = self.api.clone();
        cx.spawn(async move |_, _| {
            api.create_onboarding(
                clan_id.get(),
                contents.into_iter().map(Into::into).collect(),
            )
            .await
            .map(|response| {
                response
                    .list_onboarding
                    .into_iter()
                    .map(Into::into)
                    .collect()
            })
            .map_err(|error| error.to_string())
        })
    }

    pub fn update_onboarding_item(
        &self,
        clan_id: ClanId,
        id: i64,
        content: OnboardingContent,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        cx.spawn(async move |_, _| {
            api.update_onboarding(id, clan_id.get(), content.into())
                .await
                .map_err(|error| error.to_string())
        })
    }

    pub fn delete_onboarding_item(
        &self,
        clan_id: ClanId,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        cx.spawn(async move |_, _| {
            api.delete_onboarding(id, clan_id.get())
                .await
                .map_err(|error| error.to_string())
        })
    }

    pub fn set_onboarding_enabled(
        &mut self,
        clan_id: ClanId,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let Some(clan) = self.clans.iter().find(|clan| clan.id == clan_id).cloned() else {
            return cx.spawn(async move |_, _| Err("clan not found".into()));
        };
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let request = community_update_request(clan_id, &clan, |request| {
                request.is_onboarding = Some(enabled);
            });
            api.update_clan_desc(request)
                .await
                .map_err(|error| error.to_string())?;
            this.update(cx, |this, cx| {
                if let Some(clan) = this.clans.iter_mut().find(|clan| clan.id == clan_id) {
                    clan.is_onboarding = enabled;
                }
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn save_community_fields(
        &mut self,
        clan_id: ClanId,
        draft: CommunityDraft,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let draft = draft.sanitized();
        if !draft.is_valid() {
            return cx
                .spawn(async move |_, _| Err("community fields are incomplete or invalid".into()));
        }
        let Some(clan) = self.clans.iter().find(|c| c.id == clan_id).cloned() else {
            return cx.spawn(async move |_, _| Err("clan not found".into()));
        };
        let api = self.api.clone();
        let banner = draft.community_banner.clone();
        let about = draft.about.clone();
        let description = draft.description.clone();
        let short_url = draft.short_url.clone();
        cx.spawn(async move |this, cx| {
            let mut request = community_update_request(clan_id, &clan, |req| {
                req.is_community = Some(true);
            });
            request.community_banner = Some(banner.clone());
            request.about = Some(about.clone());
            request.description = Some(description.clone());
            request.short_url = Some(short_url.clone());
            api.update_clan_desc(request)
                .await
                .map_err(|e| e.to_string())?;
            this.update(cx, |this, cx| {
                if let Some(clan) = this.clans.iter_mut().find(|c| c.id == clan_id) {
                    clan.is_community = true;
                    clan.community_banner = banner;
                    clan.about = about;
                    clan.description = description;
                    clan.short_url = short_url;
                }
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn disable_community(
        &mut self,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let Some(clan) = self.clans.iter().find(|c| c.id == clan_id).cloned() else {
            return cx.spawn(async move |_, _| Err("clan not found".into()));
        };
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let request = community_update_request(clan_id, &clan, |req| {
                req.is_community = Some(false);
            });
            api.update_clan_desc(request)
                .await
                .map_err(|e| e.to_string())?;
            this.update(cx, |this, cx| {
                if let Some(clan) = this.clans.iter_mut().find(|c| c.id == clan_id) {
                    clan.is_community = false;
                }
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn check_clan_name_available(
        &self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool, String>> {
        let api = self.api.clone();
        let name = name.trim().to_string();
        cx.spawn(async move |_, _| {
            api.check_duplicate_clan_name(&name, "0")
                .await
                .map(|dup| !dup)
                .map_err(|e| e.to_string())
        })
    }

    pub fn create_invite_link(
        &self,
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        cx: &mut Context<Self>,
    ) -> Task<Result<ClanInviteLink, String>> {
        let api = self.api.clone();
        let clan = clan_id.get();
        let channel = channel_id.map(ChannelId::get).unwrap_or_default();
        cx.spawn(async move |_, _| {
            let link = api
                .create_link_invite_user(clan, channel, 10)
                .await
                .map_err(|e| e.to_string())?;
            Ok(ClanInviteLink {
                id: link.id,
                invite_link: link.invite_link,
            })
        })
    }
}

fn timestamped_upload_filename(original: &str) -> String {
    let sanitized = mezon_client::sanitize_upload_filename(original);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{ms}_{sanitized}")
}

fn image_dimensions(data: &[u8]) -> (i32, i32) {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .map(|(w, h)| {
            (
                i32::try_from(w).unwrap_or(i32::MAX),
                i32::try_from(h).unwrap_or(i32::MAX),
            )
        })
        .unwrap_or((0, 0))
}

fn read_clan_image_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len == 0 {
        return Err("File is empty".into());
    }
    if len > max_bytes {
        return Err(format!("File exceeds {max_bytes}-byte limit ({len} bytes)"));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

pub(crate) async fn upload_image_to_cdn(
    api: &AppApi,
    base_img_url: &str,
    path: &Path,
    max_bytes: u64,
) -> Result<String, String> {
    let path_buf = path.to_path_buf();
    let data = mezon_client::transport_runtime::handle()
        .spawn_blocking(move || read_clan_image_file(&path_buf, max_bytes))
        .await
        .map_err(|e| e.to_string())??;

    let raw_filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("avatar");
    let filename = timestamped_upload_filename(raw_filename);
    let ext =
        clan_image_extension(path).ok_or_else(|| "Unsupported image file type".to_string())?;
    let filetype = ClanImageMimeType::from_extension(&ext)
        .ok_or_else(|| "Unsupported image file type".to_string())?
        .as_str();
    let size = i32::try_from(data.len()).map_err(|_| "Image file is too large".to_string())?;
    let (width, height) = image_dimensions(&data);

    let upload = api
        .upload_attachment_file(&filename, filetype, size, width, height)
        .await
        .map_err(|e| e.to_string())?;
    mezon_client::transport_runtime::put_bytes_to_content_type(&upload.url, data, filetype)
        .await
        .map_err(|e| e.to_string())?;

    if upload.filename.is_empty() {
        return Err("UploadAttachmentFile returned empty filename".into());
    }
    let base = base_img_url.trim_end_matches('/');
    Ok(format!("{base}/{}", upload.filename))
}

fn community_update_request(
    clan_id: ClanId,
    clan: &Clan,
    patch: impl FnOnce(&mut UpdateClanDescRequest),
) -> UpdateClanDescRequest {
    let mut request = UpdateClanDescRequest {
        clan_id: clan_id.get(),
        clan_name: clan.name.clone(),
        welcome_channel_id: clan
            .welcome_channel_id
            .map(ChannelId::get)
            .unwrap_or_default(),
        prevent_anonymous: clan.prevent_anonymous,
        ..Default::default()
    };
    patch(&mut request);
    request
}

fn moved_clan_order(clans: &[Clan], from: usize, to: usize) -> Option<Vec<ClanId>> {
    if from == to || from >= clans.len() || to >= clans.len() {
        return None;
    }
    let mut ids: Vec<ClanId> = clans.iter().map(|clan| clan.id).collect();
    let moved = ids.remove(from);
    ids.insert(to, moved);
    Some(ids)
}

fn apply_clan_order(clans: &mut Vec<Clan>, order: &[ClanId]) {
    if order.is_empty() {
        return;
    }
    let mut ordered: Vec<Clan> = Vec::with_capacity(clans.len());
    for id in order {
        if let Some(pos) = clans.iter().position(|c| c.id == *id) {
            ordered.push(clans.remove(pos));
        }
    }
    ordered.append(clans);
    *clans = ordered;
}

struct ClanUpdate {
    name: Option<String>,
    logo: String,
    banner: String,
    welcome_channel_id: Option<ChannelId>,
    status: i32,
    is_onboarding: bool,
    is_community: bool,
    prevent_anonymous: bool,
    community: Option<CommunityDraft>,
}

fn update_clan(clans: &mut [Clan], clan_id: ClanId, update: ClanUpdate) -> bool {
    let Some(clan) = clans.iter_mut().find(|c| c.id == clan_id) else {
        return false;
    };
    if let Some(name) = update.name {
        clan.name = name;
    }
    clan.avatar_url = (!update.logo.is_empty()).then_some(update.logo);
    clan.banner_url = (!update.banner.is_empty()).then_some(update.banner);
    if let Some(wc) = update.welcome_channel_id {
        clan.welcome_channel_id = Some(wc);
    }
    clan.status = update.status;
    clan.is_onboarding = update.is_onboarding;
    clan.is_community = update.is_community;
    clan.prevent_anonymous = update.prevent_anonymous;
    if let Some(community) = update.community {
        clan.community_banner = community.community_banner;
        clan.about = community.about;
        clan.description = community.description;
        clan.short_url = community.short_url;
    }
    true
}

#[derive(Debug)]
pub enum CreateClanError {
    DuplicateName,
    Other(String),
}

impl std::fmt::Display for CreateClanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateName => write!(f, "A clan with that name already exists."),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

pub(crate) fn apply_created_clan(clans: &mut Vec<Clan>, desc: ApiClanDesc) {
    let clan = Clan::from(desc);
    if !clans.iter().any(|c| c.id == clan.id) {
        clans.push(clan);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_clan(id: i64, name: &str, avatar_url: Option<&str>) -> Clan {
        Clan {
            id: ClanId(id),
            creator_id: UserId(0),
            name: name.into(),
            avatar_url: avatar_url.map(|s| s.into()),
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

    fn make_update(name: Option<&str>, logo: &str) -> ClanUpdate {
        ClanUpdate {
            name: name.map(|s| s.into()),
            logo: logo.into(),
            banner: String::new(),
            welcome_channel_id: None,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
            community: None,
        }
    }

    fn clans() -> Vec<Clan> {
        vec![
            make_clan(1, "One", None),
            make_clan(2, "Two", Some("old.png")),
        ]
    }

    fn four_clans() -> Vec<Clan> {
        vec![
            make_clan(1, "A", None),
            make_clan(2, "B", None),
            make_clan(3, "C", None),
            make_clan(4, "D", None),
        ]
    }

    fn ids_after_move(from: usize, to: usize) -> Vec<i64> {
        let clans = four_clans();
        moved_clan_order(&clans, from, to)
            .expect("move")
            .into_iter()
            .map(|id| id.get())
            .collect()
    }

    #[test]
    fn dragging_a_clan_down_lands_it_after_the_target() {
        assert_eq!(ids_after_move(0, 2), vec![2, 3, 1, 4]);
    }

    #[test]
    fn dragging_a_clan_up_lands_it_before_the_target() {
        assert_eq!(ids_after_move(3, 1), vec![1, 4, 2, 3]);
    }

    #[test]
    fn moving_a_clan_onto_itself_or_out_of_range_is_a_no_op() {
        let clans = four_clans();
        assert!(moved_clan_order(&clans, 1, 1).is_none());
        assert!(moved_clan_order(&clans, 9, 1).is_none());
        assert!(moved_clan_order(&clans, 1, 9).is_none());
        assert!(moved_clan_order(&[], 0, 0).is_none());
    }

    #[test]
    fn a_saved_order_survives_a_clan_list_refetch() {
        let saved: Vec<ClanId> = [4, 3, 2, 1].into_iter().map(ClanId).collect();
        let mut clans = four_clans();

        apply_clan_order(&mut clans, &saved);

        let ids: Vec<i64> = clans.iter().map(|clan| clan.id.get()).collect();
        assert_eq!(ids, vec![4, 3, 2, 1]);
    }

    #[test]
    fn a_clan_missing_from_the_saved_order_keeps_its_place_at_the_end() {
        let saved: Vec<ClanId> = [3, 1].into_iter().map(ClanId).collect();
        let mut clans = four_clans();

        apply_clan_order(&mut clans, &saved);

        let ids: Vec<i64> = clans.iter().map(|clan| clan.id.get()).collect();
        assert_eq!(
            ids,
            vec![3, 1, 2, 4],
            "a clan joined after the order was saved must still be listed"
        );
    }

    #[test]
    fn update_clan_sets_name_and_logo() {
        let mut c = clans();
        assert!(update_clan(
            &mut c,
            ClanId(1),
            make_update(Some("NewName"), "logo.png")
        ));
        assert_eq!(c[0].name, "NewName");
        assert_eq!(c[0].avatar_url.as_deref(), Some("logo.png"));
    }

    #[test]
    fn update_clan_blank_name_keeps_name_and_empty_logo_clears_avatar() {
        let mut c = clans();
        assert!(update_clan(&mut c, ClanId(2), make_update(None, "")));
        assert_eq!(c[1].name, "Two");
        assert_eq!(c[1].avatar_url, None);
    }

    #[test]
    fn update_clan_unknown_is_noop() {
        let mut c = clans();
        assert!(!update_clan(
            &mut c,
            ClanId(999),
            make_update(Some("x"), "y")
        ));
    }

    #[test]
    fn update_clan_applies_all_fields() {
        let mut c = clans();
        let update = ClanUpdate {
            name: Some("NewName".into()),
            logo: "logo.png".into(),
            banner: "banner.png".into(),
            welcome_channel_id: Some(ChannelId(42)),
            status: 1,
            is_onboarding: true,
            is_community: true,
            prevent_anonymous: true,
            community: Some(CommunityDraft {
                community_banner: "https://cdn.example/community.png".into(),
                about: "about".into(),
                description: "desc".into(),
                short_url: "vanity".into(),
            }),
        };
        assert!(update_clan(&mut c, ClanId(1), update));
        assert_eq!(c[0].name, "NewName");
        assert_eq!(c[0].avatar_url.as_deref(), Some("logo.png"));
        assert_eq!(c[0].banner_url.as_deref(), Some("banner.png"));
        assert_eq!(c[0].welcome_channel_id, Some(ChannelId(42)));
        assert_eq!(c[0].status, 1);
        assert!(c[0].is_onboarding);
        assert!(c[0].is_community);
        assert!(c[0].prevent_anonymous);
        assert_eq!(c[0].community_banner, "https://cdn.example/community.png");
        assert_eq!(c[0].about, "about");
        assert_eq!(c[0].description, "desc");
        assert_eq!(c[0].short_url, "vanity");
    }

    #[test]
    fn community_draft_sanitizes_and_rejects_unsafe_banner() {
        let draft = CommunityDraft {
            community_banner: "javascript:alert(1)".into(),
            about: format!("  {}  ", "a".repeat(120)),
            description: "ok".into(),
            short_url: "Bad_URL!!".into(),
        }
        .sanitized();
        assert!(draft.community_banner.is_empty());
        assert_eq!(draft.about.len(), MAX_COMMUNITY_ABOUT_CHARS);
        assert_eq!(draft.short_url, "badurl");
        assert!(!draft.is_valid());

        let ok = CommunityDraft {
            community_banner: "https://cdn.example/b.png".into(),
            about: "about".into(),
            description: "desc".into(),
            short_url: "vanity".into(),
        }
        .sanitized();
        assert!(ok.is_valid());
    }

    #[test]
    fn clan_from_api_desc_zeroes_badge_and_muted() {
        use mezon_client::transport::ApiClanDesc;
        let desc = ApiClanDesc {
            clan_id: 42,
            clan_name: "Alpha".into(),
            creator_id: 0,
            logo: "logo.png".into(),
            banner: String::new(),
            welcome_channel_id: 0,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
            ..Default::default()
        };
        let clan = Clan::from(desc);
        assert_eq!(clan.badge_count, 0);
        assert!(!clan.has_unread);
        assert!(!clan.muted);
        assert_eq!(clan.avatar_url.as_deref(), Some("logo.png"));
        assert!(clan.welcome_channel_id.is_none());
    }

    #[test]
    fn clan_image_mime_type_matches_allowed_extensions() {
        for ext in ClanImageMimeType::ALLOWED_EXTENSIONS {
            assert!(
                ClanImageMimeType::from_extension(ext).is_some(),
                "missing mime mapping for .{ext}"
            );
        }
        assert!(ClanImageMimeType::from_extension("bmp").is_none());
    }

    #[test]
    fn validate_clan_image_file_rejects_unsupported_extension() {
        let dir =
            std::env::temp_dir().join(format!("mezon-clan-image-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bad.bmp");
        std::fs::write(&path, b"data").expect("write temp file");
        let err = validate_clan_image_file(&path, MAX_CLAN_LOGO_BYTES)
            .expect_err("unsupported extension must fail");
        assert_eq!(err, "Unsupported image file type");
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn validate_clan_image_file_rejects_oversized_file() {
        let dir =
            std::env::temp_dir().join(format!("mezon-clan-image-size-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("large.png");
        std::fs::write(&path, vec![0_u8; 32]).expect("write temp file");
        let err = validate_clan_image_file(&path, 16).expect_err("size limit must fail");
        assert!(err.contains("byte limit"));
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn clan_from_api_desc_maps_creator_id() {
        use mezon_client::transport::ApiClanDesc;
        let desc = ApiClanDesc {
            clan_id: 42,
            clan_name: "Alpha".into(),
            creator_id: 7,
            logo: String::new(),
            banner: String::new(),
            welcome_channel_id: 0,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
            ..Default::default()
        };
        assert_eq!(Clan::from(desc).creator_id, UserId(7));
    }

    #[test]
    fn refetched_clans_keep_live_badges() {
        let mut previous = clans();
        previous[0].badge_count = 4;
        previous[0].has_unread = true;
        let mut refetched = clans();
        carry_live_badges(&previous, &mut refetched);
        assert_eq!(refetched[0].badge_count, 4);
        assert!(refetched[0].has_unread);
        assert_eq!(refetched[1].badge_count, 0);
    }

    #[test]
    fn first_load_takes_server_badges() {
        let mut fresh = clans();
        fresh[0].badge_count = 7;
        fresh[0].has_unread = true;
        carry_live_badges(&[], &mut fresh);
        assert_eq!(fresh[0].badge_count, 7);
        assert!(fresh[0].has_unread);
    }

    #[test]
    fn newly_joined_clan_takes_server_badge() {
        let previous = vec![make_clan(1, "One", None)];
        let mut refetched = clans();
        refetched[1].badge_count = 9;
        carry_live_badges(&previous, &mut refetched);
        assert_eq!(refetched[0].badge_count, 0);
        assert_eq!(refetched[1].badge_count, 9);
    }

    #[test]
    fn badge_map_applies_to_clans_on_reload() {
        let mut c = clans();
        let badge_map: std::collections::HashMap<String, (i32, bool)> = [
            ("1".to_string(), (3_i32, true)),
            ("99".to_string(), (5_i32, false)),
        ]
        .into_iter()
        .collect();
        for clan in &mut c {
            if let Some(&(badge, has_unread)) = badge_map.get(&clan.id.to_string()) {
                clan.badge_count = badge.max(0) as u32;
                clan.has_unread = has_unread;
            }
        }
        assert_eq!(c[0].badge_count, 3);
        assert!(c[0].has_unread);
        assert_eq!(c[1].badge_count, 0);
        assert!(!c[1].has_unread);
    }

    #[test]
    fn set_has_unread_message_sets_flag() {
        let mut c = clans();
        if let Some(clan) = c.iter_mut().find(|c| c.id == ClanId(1))
            && !clan.muted
        {
            clan.has_unread = false;
        }
        if let Some(clan) = c.iter_mut().find(|c| c.id == ClanId(1))
            && !clan.muted
        {
            clan.has_unread = true;
        }
        assert!(c[0].has_unread);
        assert_eq!(c[0].badge_count, 0);
    }

    #[test]
    fn increment_clan_badge_when_not_muted() {
        let mut c = clans();
        if let Some(clan) = c.iter_mut().find(|c| c.id == ClanId(1))
            && !clan.muted
        {
            clan.badge_count = clan.badge_count.saturating_add(1);
        }
        assert_eq!(c[0].badge_count, 1);
    }

    #[test]
    fn increment_clan_badge_skipped_when_muted() {
        let mut c = clans();
        c[0].muted = true;
        if let Some(clan) = c.iter_mut().find(|cl| cl.id == ClanId(1))
            && !clan.muted
        {
            clan.badge_count = clan.badge_count.saturating_add(1);
        }
        assert_eq!(c[0].badge_count, 0);
    }

    #[test]
    fn mark_as_read_resets_badge_and_unread() {
        use mezon_proto::realtime;
        let mut c = clans();
        c[0].badge_count = 7;
        c[0].has_unread = true;
        let evt = realtime::MarkAsRead {
            clan_id: 1,
            ..Default::default()
        };
        if let Some(clan) = c.iter_mut().find(|cl| cl.id == ClanId(evt.clan_id))
            && (clan.badge_count > 0 || clan.has_unread)
        {
            clan.badge_count = 0;
            clan.has_unread = false;
        }
        assert_eq!(c[0].badge_count, 0);
        assert!(!c[0].has_unread);
    }

    #[test]
    fn mark_as_read_unknown_clan_is_noop() {
        use mezon_proto::realtime;
        let mut c = clans();
        c[0].badge_count = 3;
        let evt = realtime::MarkAsRead {
            clan_id: 999,
            ..Default::default()
        };
        if let Some(clan) = c.iter_mut().find(|cl| cl.id == ClanId(evt.clan_id)) {
            clan.badge_count = 0;
            clan.has_unread = false;
        }
        assert_eq!(c[0].badge_count, 3);
    }

    #[test]
    fn accept_invite_error_variants_are_distinct() {
        assert_ne!(
            AcceptInviteError::AlreadyJoining,
            AcceptInviteError::InvalidLink
        );
        assert_ne!(
            AcceptInviteError::InvalidLink,
            AcceptInviteError::Failed("x".into())
        );
    }

    #[test]
    fn apply_created_clan_inserts_new_clan() {
        use mezon_client::transport::ApiClanDesc;
        let mut clans = clans();
        let desc = ApiClanDesc {
            clan_id: 99,
            clan_name: "NewClan".into(),
            creator_id: 0,
            logo: "logo.png".into(),
            banner: String::new(),
            welcome_channel_id: 0,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
            ..Default::default()
        };
        apply_created_clan(&mut clans, desc);
        assert_eq!(clans.len(), 3);
        let inserted = clans.iter().find(|c| c.id == ClanId(99)).unwrap();
        assert_eq!(inserted.name, "NewClan");
        assert_eq!(inserted.avatar_url.as_deref(), Some("logo.png"));
        assert_eq!(inserted.badge_count, 0);
        assert!(!inserted.has_unread);
    }

    #[test]
    fn apply_created_clan_skips_duplicate_id() {
        use mezon_client::transport::ApiClanDesc;
        let mut clans = clans();
        let desc = ApiClanDesc {
            clan_id: 1,
            clan_name: "SameClan".into(),
            creator_id: 0,
            logo: String::new(),
            banner: String::new(),
            welcome_channel_id: 0,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
            ..Default::default()
        };
        apply_created_clan(&mut clans, desc);
        assert_eq!(clans.len(), 2);
        assert_eq!(clans[0].name, "One");
    }

    #[test]
    fn create_clan_error_display_duplicate_name() {
        let err = CreateClanError::DuplicateName;
        let msg = format!("{err}");
        assert!(msg.contains("already exists"));
    }

    const TEST_SELF: i64 = 42;
    const TEST_OTHER: i64 = 77;

    fn init_clan_list(cx: &mut App) -> Entity<ClanList> {
        let api = Arc::new(mezon_client::AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| {
            crate::AuthState::Authenticated(mezon_client::Session {
                user_id: TEST_SELF.to_string(),
                ..Default::default()
            })
        });
        crate::badge::BadgeService::init(auth_state, cx);
        ClanList::init(api, cx)
    }

    fn removed_event(clan_id: i64, user_ids: &[i64]) -> RealtimeEvent {
        RealtimeEvent::UserClanRemoved(mezon_proto::realtime::UserClanRemoved {
            clan_id,
            user_ids: user_ids.to_vec(),
        })
    }

    #[test]
    fn create_clan_error_display_other() {
        let err = CreateClanError::Other("network timeout".into());
        let msg = format!("{err}");
        assert_eq!(msg, "network timeout");
    }
    #[gpui::test]
    fn user_clan_removed_keeps_the_clan_when_someone_else_is_kicked(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let clan_list = init_clan_list(cx);
            clan_list.update(cx, |list, cx| {
                list.update_clans(clans(), cx);

                list.handle_event(&removed_event(1, &[TEST_OTHER]), cx);
                assert!(
                    list.clan_by_id(ClanId(1)).is_some(),
                    "kicking another member must not drop the clan locally"
                );

                list.handle_event(&removed_event(1, &[TEST_OTHER, TEST_SELF]), cx);
                assert!(list.clan_by_id(ClanId(1)).is_none());
                assert!(list.clan_by_id(ClanId(2)).is_some());
            });
        });
    }

    #[gpui::test]
    fn user_clan_removed_with_no_ids_is_a_no_op(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let clan_list = init_clan_list(cx);
            clan_list.update(cx, |list, cx| {
                list.update_clans(clans(), cx);
                list.handle_event(&removed_event(1, &[]), cx);
                assert!(list.clan_by_id(ClanId(1)).is_some());
            });
        });
    }

    #[gpui::test]
    fn dropping_the_active_clan_clears_the_active_clan(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let clan_list = init_clan_list(cx);
            clan_list.update(cx, |list, cx| {
                list.update_clans(clans(), cx);
                list.select_clan(ClanId(1), cx);

                list.handle_event(&removed_event(1, &[TEST_SELF]), cx);
                assert_eq!(
                    list.active_clan_id, None,
                    "leaving the clan you are viewing must not silently drop you into another one"
                );
                assert!(list.clan_by_id(ClanId(2)).is_some());
            });
        });
    }

    #[gpui::test]
    async fn leaving_without_a_signed_in_user_fails_before_calling_the_server(
        cx: &mut gpui::TestAppContext,
    ) {
        let clan_list = cx.update(|cx| {
            let api = Arc::new(mezon_client::AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            RealtimeDispatch::init(api.clone(), cx);
            let auth_state = cx.new(|_| crate::AuthState::NotAuthenticated);
            crate::badge::BadgeService::init(auth_state, cx);
            ClanList::init(api, cx)
        });
        let task = cx.update(|cx| {
            clan_list.update(cx, |list, cx| {
                list.update_clans(clans(), cx);
                list.leave_clan(ClanId(1), cx)
            })
        });
        assert!(task.await.is_err());
        clan_list.update(cx, |list, _| {
            assert!(
                list.clan_by_id(ClanId(1)).is_some(),
                "a rejected leave must not drop the clan locally"
            );
        });
    }
}
