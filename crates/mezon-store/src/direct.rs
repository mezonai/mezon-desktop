use crate::ids::{ChannelId, UserId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::transport::{ApiChannelDesc, ApiDirectChannel, build_send_content};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};

use crate::Freshness;
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::users_by_user::UsersByUserStore;

const STREAM_MODE_GROUP: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectKind {
    Dm,
    Group,
}

impl DirectKind {
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            2 => DirectKind::Group,
            _ => DirectKind::Dm,
        }
    }

    /// `ChannelStreamMode` used when sending (DM = 4, GROUP = 3).
    pub fn stream_mode(self) -> i32 {
        match self {
            DirectKind::Dm => 4,
            DirectKind::Group => 3,
        }
    }

    /// `ChannelType` used when joining the socket room (DM = 3, GROUP = 2).
    pub fn channel_type(self) -> i32 {
        match self {
            DirectKind::Dm => 3,
            DirectKind::Group => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DirectChannel {
    pub id: ChannelId,
    pub label: String,
    pub kind: DirectKind,
    pub avatar: String,
    pub peer_user_id: Option<UserId>,
    pub peer_username: String,
    pub creator_id: Option<UserId>,
    pub online: bool,
    pub member_count: u32,
    pub unread_count: u32,
    pub last_sent_timestamp: i64,
    pub last_seen_timestamp: i64,
}

impl DirectChannel {
    pub fn is_unread(&self) -> bool {
        self.unread_count > 0
            || (self.last_sent_timestamp > 0 && self.last_seen_timestamp < self.last_sent_timestamp)
    }

    pub fn counts_toward_unread_badge(&self, muted: bool) -> bool {
        dm_counts_toward_unread_badge(self.unread_count, muted)
    }
}

pub fn dm_counts_toward_unread_badge(unread_count: u32, muted: bool) -> bool {
    unread_count > 0 && !muted
}

#[derive(Debug, Clone, Copy)]
pub enum DirectEvent {
    /// `channel_id` is `Some` for a single-conversation update (incoming
    /// message, read-state change) so consumers can skip irrelevant channels;
    /// `None` means a bulk change (fetch page, reset) — treat as "anything".
    Changed { channel_id: Option<ChannelId> },
}

const DM_PAGE_SIZE: i32 = 500;
const MESSAGE_CODE_SEND_TOKEN: i32 = 11;
/// Incoming DM traffic bumps last-message/unread state per message; the
/// sidebar and unread rail only need to converge, not animate every packet,
/// so per-message `Changed` notifies are coalesced into one flush.
const CHANGED_NOTIFY_COALESCE: Duration = Duration::from_millis(100);

#[derive(Default)]
struct DirectChannelList {
    channels: Vec<DirectChannel>,
    by_id: HashMap<ChannelId, usize>,
    pending_created: Option<ChannelId>,
}

impl DirectChannelList {
    fn reindex(&mut self) {
        self.by_id.clear();
        self.by_id.reserve(self.channels.len());
        for (i, c) in self.channels.iter().enumerate() {
            self.by_id.insert(c.id, i);
        }
    }

    fn as_slice(&self) -> &[DirectChannel] {
        &self.channels
    }

    fn find(&self, id: ChannelId) -> Option<&DirectChannel> {
        let idx = *self.by_id.get(&id)?;
        self.channels.get(idx)
    }

    fn find_mut(&mut self, id: ChannelId) -> Option<&mut DirectChannel> {
        let idx = *self.by_id.get(&id)?;
        self.channels.get_mut(idx)
    }

    fn sort_by_recent(&mut self) {
        sort_by_recent(&mut self.channels);
        self.reindex();
    }

    fn resort_after_bump(&mut self, id: ChannelId) {
        let Some(&idx) = self.by_id.get(&id) else {
            return;
        };
        let new_ts = self.channels[idx].last_sent_timestamp;
        let mut target = idx;
        while target > 0 && self.channels[target - 1].last_sent_timestamp < new_ts {
            target -= 1;
        }
        while target + 1 < self.channels.len()
            && self.channels[target + 1].last_sent_timestamp > new_ts
        {
            target += 1;
        }
        if target == idx {
            return;
        }
        if target < idx {
            self.channels[target..=idx].rotate_right(1);
        } else {
            self.channels[idx..=target].rotate_left(1);
        }
        let (lo, hi) = (target.min(idx), target.max(idx));
        for i in lo..=hi {
            self.by_id.insert(self.channels[i].id, i);
        }
    }

    fn replace(
        &mut self,
        mut channels: Vec<DirectChannel>,
        badge_map: &HashMap<ChannelId, DmBadgeInfo>,
    ) {
        let local_counts: HashMap<ChannelId, u32> = self
            .channels
            .iter()
            .map(|c| (c.id, c.unread_count))
            .collect();
        if let Some(id) = self.pending_created {
            if channels.iter().any(|remote| remote.id == id) {
                self.pending_created = None;
            } else if let Some(local) = self.find(id).cloned() {
                channels.push(local);
            }
        }
        merge_dm_unread_counts(&mut channels, badge_map, &local_counts);
        self.channels = channels;
        self.sort_by_recent();
    }

    fn upsert(&mut self, channel: DirectChannel) {
        if let Some(existing) = self.find_mut(channel.id) {
            if !channel.label.is_empty() {
                existing.label = channel.label;
            }
            if !channel.avatar.is_empty() {
                existing.avatar = channel.avatar;
            }
            if channel.peer_user_id.is_some() {
                existing.peer_user_id = channel.peer_user_id;
            }
            if !channel.peer_username.is_empty() {
                existing.peer_username = channel.peer_username;
            }
            self.sort_by_recent();
        } else {
            self.channels.push(channel);
            self.sort_by_recent();
        }
    }

    fn upsert_created(&mut self, channel: DirectChannel) {
        let id = channel.id;
        self.upsert(channel);
        self.pending_created = Some(id);
    }

    fn push_new(&mut self, channel: DirectChannel) -> bool {
        if self.by_id.contains_key(&channel.id) {
            return false;
        }
        self.channels.push(channel);
        self.sort_by_recent();
        true
    }

    fn extend_new(&mut self, channels: Vec<DirectChannel>) {
        for channel in channels {
            if !self.by_id.contains_key(&channel.id) {
                self.by_id.insert(channel.id, self.channels.len());
                self.channels.push(channel);
            }
        }
        self.sort_by_recent();
    }
}

/// Holds the user's direct-message / group conversations (clan_id = 0). Self-subscribes to the
/// realtime broadcast (cf. `ChannelStore`): fetches the list on connect and keeps it ordered by
/// most-recent activity.
pub struct DirectMessageStore {
    channels: DirectChannelList,
    loading: bool,
    freshness: Freshness,
    has_more: bool,
    current_page: u32,
    current: Option<(ChannelId, i32)>,
    pending_changed: Vec<ChannelId>,
    changed_notify_task: Option<Task<()>>,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalDirectMessageStore(Entity<DirectMessageStore>);
impl Global for GlobalDirectMessageStore {}

impl EventEmitter<DirectEvent> for DirectMessageStore {}

impl DirectMessageStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalDirectMessageStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalDirectMessageStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalDirectMessageStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.channels = DirectChannelList::default();
        self.loading = false;
        self.freshness.mark_stale();
        self.has_more = true;
        self.current_page = 1;
        self.current = None;
        self.pending_changed.clear();
        self.changed_notify_task = None;
        cx.emit(DirectEvent::Changed { channel_id: None });
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            channels: DirectChannelList::default(),
            loading: false,
            freshness: Freshness::new(),
            has_more: true,
            current_page: 1,
            current: None,
            pending_changed: Vec::new(),
            changed_notify_task: None,
            api,
            _conn_watch: conn_watch,
        }
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [RealtimeKind::UserChannelAdded, RealtimeKind::ChannelUpdated] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, cx| this.refresh(cx));
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
                    if this
                        .update(cx, |this, cx| {
                            this.freshness.mark_stale();
                            this.fetch(cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn channels(&self) -> &[DirectChannel] {
        self.channels.as_slice()
    }

    pub fn find(&self, id: ChannelId) -> Option<&DirectChannel> {
        self.channels.find(id)
    }

    pub fn create_dm_with_user(
        &self,
        user_id: UserId,
        member_label: String,
        member_avatar: String,
        member_username: String,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<(ChannelId, i32)>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let desc = api.create_direct_channel(&[user_id.0]).await?;
            let channel_id = ChannelId(desc.channel_id);
            let channel_type = desc.channel_type as i32;
            this.update(cx, |this, cx| {
                let (peer_username, peer_avatar) =
                    if !member_username.is_empty() && !member_avatar.is_empty() {
                        (member_username.clone(), member_avatar.clone())
                    } else {
                        UsersByUserStore::try_global(cx)
                            .and_then(|store| store.read(cx).user(user_id))
                            .map(|user| {
                                (
                                    if user.username.is_empty() {
                                        member_username.clone()
                                    } else {
                                        user.username.clone()
                                    },
                                    if user.avatar_url.is_empty() {
                                        member_avatar.clone()
                                    } else {
                                        user.avatar_url.clone()
                                    },
                                )
                            })
                            .unwrap_or((member_username.clone(), member_avatar.clone()))
                    };
                let label = if !member_label.is_empty() {
                    member_label
                } else if !peer_username.is_empty() {
                    peer_username.clone()
                } else {
                    desc.channel_label.clone()
                };
                let channel =
                    direct_from_created(&desc, user_id, &label, &peer_avatar, &peer_username);
                this.channels.upsert_created(channel);
                this.freshness.mark_fetched();
                cx.emit(DirectEvent::Changed { channel_id: None });
                cx.notify();
            })?;
            Ok((channel_id, channel_type))
        })
    }

    pub fn create_group_with_users(
        &self,
        user_ids: Vec<UserId>,
        group_label: String,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<(ChannelId, i32)>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let ids: Vec<i64> = user_ids.iter().map(|id| id.0).collect();
            let fallback_member_count = (user_ids.len() + 1) as u32;
            let desc = api.create_direct_channel(&ids).await?;
            let channel_id = ChannelId(desc.channel_id);
            let channel_type = desc.channel_type as i32;
            this.update(cx, |this, cx| {
                let channel = direct_group_from_created(&desc, &group_label, fallback_member_count);
                this.channels.upsert_created(channel);
                this.freshness.mark_fetched();
                cx.emit(DirectEvent::Changed { channel_id: None });
                cx.notify();
            })?;
            Ok((channel_id, channel_type))
        })
    }

    /// `label`/`avatar` are `Some` only for the fields the user actually edited;
    /// `Some("")` on `avatar` means "remove it".
    pub fn update_group(
        &mut self,
        channel_id: ChannelId,
        label: Option<String>,
        avatar: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let api = self.api.clone();
        let current = self.channels.find(channel_id);
        let (sent_label, sent_avatar) = group_desc_payload(
            current.map(|ch| (ch.label.as_str(), ch.avatar.as_str())),
            label.as_deref(),
            avatar.as_deref(),
        );
        cx.spawn(async move |this, cx| {
            api.update_channel_desc(
                0,
                channel_id.0,
                mezon_client::UpdateChannelDescParams {
                    channel_label: sent_label,
                    channel_avatar: sent_avatar,
                    ..Default::default()
                },
            )
            .await?;
            this.update(cx, |this, cx| {
                if let Some(ch) = this.channels.find_mut(channel_id) {
                    if let Some(label) = label {
                        ch.label = label;
                    }
                    if let Some(avatar) = avatar {
                        ch.avatar = avatar;
                    }
                    cx.emit(DirectEvent::Changed { channel_id: None });
                    cx.notify();
                }
            })?;
            Ok(())
        })
    }

    pub fn send_direct_text_to_target(
        &self,
        user_id: Option<UserId>,
        existing_channel: Option<(ChannelId, i32)>,
        member_label: String,
        member_avatar: String,
        member_username: String,
        body: DirectMessageBody,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let api = self.api.clone();
        let existing = existing_channel.or_else(|| {
            user_id.and_then(|user_id| {
                self.channels
                    .as_slice()
                    .iter()
                    .find(|channel| {
                        channel.kind == DirectKind::Dm && channel.peer_user_id == Some(user_id)
                    })
                    .map(|channel| (channel.id, channel.kind.stream_mode()))
            })
        });

        cx.spawn(async move |this, cx| {
            let (channel_id, mode) = match existing {
                Some(existing) => existing,
                None => {
                    let user_id = user_id
                        .ok_or_else(|| anyhow::anyhow!("invite target has no user or channel"))?;
                    let desc = api.create_direct_channel(&[user_id.0]).await?;
                    let channel_id = ChannelId(desc.channel_id);
                    let kind = DirectKind::from_raw(desc.channel_type);
                    let mode = kind.stream_mode();
                    this.update(cx, |this, cx| {
                        let channel = direct_from_created(
                            &desc,
                            user_id,
                            &member_label,
                            &member_avatar,
                            &member_username,
                        );
                        this.channels.upsert_created(channel);
                        this.freshness.mark_fetched();
                        cx.emit(DirectEvent::Changed { channel_id: None });
                        cx.notify();
                    })?;
                    (channel_id, mode)
                }
            };

            let content_json = body.into_content_json();
            let sent = api
                .send_channel_message_structured(channel_id.get(), &content_json, mode)
                .await?;
            this.update(cx, |this, cx| {
                this.note_message(channel_id, sent.create_time, true, false, cx);
            })?;
            Ok(())
        })
    }

    pub fn create_dm_and_send_text(
        &self,
        user_id: UserId,
        member_label: String,
        member_avatar: String,
        member_username: String,
        body: DirectMessageBody,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<(ChannelId, i32)>> {
        self.create_dm_and_send_with_code(
            user_id,
            member_label,
            member_avatar,
            member_username,
            body,
            0,
            cx,
        )
    }

    pub fn create_dm_and_send_token_card(
        &self,
        user_id: UserId,
        member_username: String,
        text: String,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<(ChannelId, i32)>> {
        self.create_dm_and_send_with_code(
            user_id,
            String::new(),
            String::new(),
            member_username,
            DirectMessageBody::Text(text),
            MESSAGE_CODE_SEND_TOKEN,
            cx,
        )
    }

    fn create_dm_and_send_with_code(
        &self,
        user_id: UserId,
        member_label: String,
        member_avatar: String,
        member_username: String,
        body: DirectMessageBody,
        message_code: i32,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<(ChannelId, i32)>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let desc = api.create_direct_channel(&[user_id.0]).await?;
            let channel_id = ChannelId(desc.channel_id);
            let channel_type = desc.channel_type as i32;
            let send_mode = DirectKind::Dm.stream_mode();

            this.update(cx, |this, cx| {
                let (peer_username, peer_avatar) =
                    if !member_username.is_empty() && !member_avatar.is_empty() {
                        (member_username.clone(), member_avatar.clone())
                    } else {
                        UsersByUserStore::try_global(cx)
                            .and_then(|store| store.read(cx).user(user_id))
                            .map(|user| {
                                (
                                    if user.username.is_empty() {
                                        member_username.clone()
                                    } else {
                                        user.username.clone()
                                    },
                                    if user.avatar_url.is_empty() {
                                        member_avatar.clone()
                                    } else {
                                        user.avatar_url.clone()
                                    },
                                )
                            })
                            .unwrap_or((member_username.clone(), member_avatar.clone()))
                    };
                let label = if !member_label.is_empty() {
                    member_label
                } else if !peer_username.is_empty() {
                    peer_username.clone()
                } else {
                    desc.channel_label.clone()
                };
                let channel =
                    direct_from_created(&desc, user_id, &label, &peer_avatar, &peer_username);
                this.channels.upsert_created(channel);
                this.freshness.mark_fetched();
                cx.emit(DirectEvent::Changed { channel_id: None });
                cx.notify();
            })?;

            if let Err(e) = api
                .join_chat(0, channel_id.get(), channel_type, false)
                .await
            {
                tracing::warn!("join_chat after create DM failed: {e}");
            }

            let content_json = body.into_content_json();
            let sent = api
                .send_channel_message_structured_with_code(
                    channel_id.get(),
                    &content_json,
                    send_mode,
                    message_code,
                )
                .await?;
            this.update(cx, |this, cx| {
                this.note_message(channel_id, sent.create_time, true, false, cx);
            })?;
            Ok((channel_id, channel_type))
        })
    }

    pub fn set_current(&mut self, id: ChannelId, channel_type: i32) {
        self.current = Some((id, channel_type));
    }

    pub fn current(&self) -> Option<(ChannelId, i32)> {
        self.current
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx);
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.loading && !self.freshness.is_fresh(crate::CACHE_TTL) {
            self.fetch(cx);
        }
    }

    pub fn fetch_more(&mut self, cx: &mut Context<Self>) {
        if self.loading || !self.has_more {
            return;
        }
        let next_page = self.current_page + 1;
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let (result, badges) = tokio::join!(
                api.list_dm_channels(next_page as i32),
                api.list_channel_badge_counts(0),
            );
            let badge_map = badge_map_from_descs(badges.unwrap_or_default());
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(list) => {
                        let has_more = list.len() >= DM_PAGE_SIZE as usize;
                        let mut new_channels: Vec<DirectChannel> =
                            list.into_iter().map(direct_from_api).collect();
                        let local_counts: HashMap<ChannelId, u32> = this
                            .channels
                            .as_slice()
                            .iter()
                            .map(|c| (c.id, c.unread_count))
                            .collect();
                        merge_dm_unread_counts(&mut new_channels, &badge_map, &local_counts);
                        this.channels.extend_new(new_channels);
                        this.has_more = has_more;
                        this.current_page = next_page;
                        cx.emit(DirectEvent::Changed { channel_id: None });
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_dm_channels page {next_page} failed: {e}"),
                }
            });
        })
        .detach();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let (result, badges) =
                tokio::join!(api.list_dm_channels(1), api.list_channel_badge_counts(0),);
            let badge_map = badge_map_from_descs(badges.unwrap_or_default());
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(list) => {
                        tracing::info!("DirectMessageStore: fetched {} DM channels", list.len());
                        let has_more = list.len() >= DM_PAGE_SIZE as usize;
                        let channels: Vec<DirectChannel> =
                            list.into_iter().map(direct_from_api).collect();
                        this.channels.replace(channels, &badge_map);
                        this.freshness.mark_fetched();
                        this.has_more = has_more;
                        this.current_page = 1;
                        cx.emit(DirectEvent::Changed { channel_id: None });
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_dm_channels failed: {e}"),
                }
            });
        })
        .detach();
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::ChannelUpdated(e) => {
                let id = ChannelId(e.channel_id);
                let Some(ch) = self.channels.find_mut(id) else {
                    return;
                };
                if !e.channel_label.is_empty() {
                    ch.label = e.channel_label.clone();
                }
                if !e.channel_avatar.is_empty() {
                    ch.avatar = e.channel_avatar.clone();
                }
                cx.emit(DirectEvent::Changed { channel_id: None });
                cx.notify();
            }
            RealtimeEvent::UserChannelAdded(e) => {
                let Some(ref desc) = e.channel_desc else {
                    return;
                };
                let channel_type = desc.r#type as u32;
                if channel_type != 2 && channel_type != 3 {
                    return;
                }
                let mut api_ch = direct_from_channel_desc(desc);
                if e.create_time_seconds > 0 {
                    api_ch.last_sent_timestamp = i64::from(e.create_time_seconds);
                }
                let mut channel = direct_from_api(api_ch);
                let self_id = crate::badge::BadgeService::try_global(cx)
                    .and_then(|badge| badge.read(cx).current_user_id(cx));
                enrich_direct_from_event_users(&mut channel, &e.users, self_id);
                let inserted = self.channels.push_new(channel);
                tracing::info!(
                    "user_channel_added: direct channel {} inserted={inserted}",
                    desc.channel_id
                );
                if !inserted {
                    return;
                }
                cx.emit(DirectEvent::Changed { channel_id: None });
                cx.notify();
            }
            _ => {}
        }
    }

    pub fn note_message(
        &mut self,
        channel_id: ChannelId,
        ts: i64,
        from_me: bool,
        increment_unread: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(channel) = self.channels.find_mut(channel_id) else {
            return false;
        };
        if ts > channel.last_sent_timestamp {
            channel.last_sent_timestamp = ts;
        }
        if from_me {
            if ts > channel.last_seen_timestamp {
                channel.last_seen_timestamp = ts;
            }
        } else if increment_unread {
            channel.unread_count = channel.unread_count.saturating_add(1);
        }
        self.channels.resort_after_bump(channel_id);
        self.schedule_changed_notify(channel_id, cx);
        true
    }

    /// Mirrors mezon-react's `addDirectByMessageWS`: a DM/group message arriving for a
    /// conversation not in the list (stranger DM, first message ever) synthesizes the entry
    /// from the message itself so the conversation shows up immediately. The next full fetch
    /// replaces it with the server record.
    pub fn insert_from_message(
        &mut self,
        m: &mezon_proto::api::ChannelMessage,
        from_me: bool,
        increment_unread: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .channels
            .push_new(direct_from_message(m, from_me, increment_unread))
        {
            return false;
        }
        cx.emit(DirectEvent::Changed { channel_id: None });
        cx.notify();
        true
    }

    pub fn note_read(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) -> bool {
        let Some(ch) = self.channels.find_mut(channel_id) else {
            return false;
        };
        ch.unread_count = 0;
        ch.last_seen_timestamp = ch.last_sent_timestamp;
        cx.emit(DirectEvent::Changed {
            channel_id: Some(channel_id),
        });
        cx.notify();
        true
    }

    pub fn mark_as_read(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if channel_id.is_zero() {
            return;
        }
        self.note_read(channel_id, cx);
        let api = self.api.clone();
        let id = channel_id.get();
        cx.spawn(async move |_, _| {
            if let Err(e) = api.mark_as_read(id, 0, 0).await {
                tracing::error!("mark dm as read failed for channel {id}: {e}");
            }
        })
        .detach();
    }

    fn schedule_changed_notify(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if !self.pending_changed.contains(&channel_id) {
            self.pending_changed.push(channel_id);
        }
        if self.changed_notify_task.is_some() {
            return;
        }
        self.changed_notify_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(CHANGED_NOTIFY_COALESCE)
                .await;
            let _ = this.update(cx, |store, cx| {
                store.changed_notify_task = None;
                for channel_id in std::mem::take(&mut store.pending_changed) {
                    cx.emit(DirectEvent::Changed {
                        channel_id: Some(channel_id),
                    });
                }
                cx.notify();
            });
        }));
    }
}

pub enum DirectMessageBody {
    Text(String),
    ContentJson(String),
}

impl DirectMessageBody {
    fn into_content_json(self) -> String {
        match self {
            Self::Text(text) => build_send_content(&text, &[], &[], &[]).json,
            Self::ContentJson(json) => json,
        }
    }
}

fn sort_by_recent(channels: &mut [DirectChannel]) {
    channels.sort_by_key(|c| std::cmp::Reverse(c.last_sent_timestamp));
}

#[derive(Debug, Clone, Copy)]
struct DmBadgeInfo {
    badge_count: i32,
    last_sent_timestamp: i64,
    last_seen_timestamp: i64,
}

fn badge_map_from_descs(
    descs: Vec<mezon_client::transport::ApiChannelDesc>,
) -> HashMap<ChannelId, DmBadgeInfo> {
    descs
        .into_iter()
        .map(|d| {
            (
                ChannelId(d.channel_id),
                DmBadgeInfo {
                    badge_count: d.badge_count,
                    last_sent_timestamp: d.last_sent_timestamp,
                    last_seen_timestamp: d.last_seen_timestamp,
                },
            )
        })
        .collect()
}

/// Merge server badge counts + last-sent/seen timestamps with in-memory WS increments (React
/// seeds its DM sort + unread state from `ListChannelBadgeCount`, not just `ListChannelDescs` —
/// both timestamps must come from the same record or `is_unread` desyncs from reality).
fn merge_dm_unread_counts(
    channels: &mut [DirectChannel],
    badge_map: &HashMap<ChannelId, DmBadgeInfo>,
    local_counts: &HashMap<ChannelId, u32>,
) {
    for ch in channels.iter_mut() {
        let badge = badge_map.get(&ch.id);
        let from_list = ch.unread_count;
        let from_badges = badge.map(|b| b.badge_count).unwrap_or(0).max(0) as u32;
        let from_local = local_counts.get(&ch.id).copied().unwrap_or(0);
        ch.unread_count = from_list.max(from_badges).max(from_local);
        if let Some(b) = badge {
            ch.last_sent_timestamp = b.last_sent_timestamp;
            ch.last_seen_timestamp = b.last_seen_timestamp;
        }
    }
}

fn direct_from_channel_desc(desc: &mezon_proto::api::ChannelDescription) -> ApiDirectChannel {
    let last_seen_timestamp = desc
        .last_seen_message
        .as_ref()
        .map(|m| i64::from(m.timestamp_seconds))
        .unwrap_or(0);
    let last_sent_timestamp = desc
        .last_sent_message
        .as_ref()
        .map(|m| i64::from(m.timestamp_seconds))
        .unwrap_or(0);
    ApiDirectChannel {
        channel_id: desc.channel_id,
        channel_label: desc.channel_label.clone(),
        channel_type: desc.r#type as u32,
        channel_avatar: desc.channel_avatar.clone(),
        avatars: desc.avatars.clone(),
        usernames: desc.usernames.clone(),
        display_names: desc.display_names.clone(),
        user_ids: desc.user_ids.clone(),
        onlines: desc.onlines.clone(),
        member_count: desc.member_count,
        count_mess_unread: desc.count_mess_unread,
        last_sent_timestamp,
        last_seen_timestamp,
        creator_id: desc.creator_id,
    }
}

fn direct_from_created(
    desc: &ApiChannelDesc,
    peer_user_id: UserId,
    label: &str,
    peer_avatar: &str,
    peer_username: &str,
) -> DirectChannel {
    let kind = DirectKind::from_raw(desc.channel_type);
    DirectChannel {
        id: ChannelId(desc.channel_id),
        label: label.to_string(),
        kind,
        avatar: peer_avatar.to_string(),
        peer_user_id: Some(peer_user_id),
        peer_username: peer_username.to_string(),
        creator_id: if desc.creator_id == 0 {
            None
        } else {
            Some(UserId(desc.creator_id))
        },
        online: false,
        member_count: desc.member_count.max(0) as u32,
        unread_count: desc.count_mess_unread.max(0) as u32,
        last_sent_timestamp: desc.last_sent_timestamp,
        last_seen_timestamp: desc.last_seen_timestamp,
    }
}

fn direct_group_from_created(
    desc: &ApiChannelDesc,
    label: &str,
    fallback_member_count: u32,
) -> DirectChannel {
    let member_count = if desc.member_count > 0 {
        desc.member_count as u32
    } else {
        fallback_member_count
    };
    DirectChannel {
        id: ChannelId(desc.channel_id),
        label: if label.is_empty() {
            desc.channel_label.clone()
        } else {
            label.to_string()
        },
        kind: DirectKind::from_raw(desc.channel_type),
        avatar: String::new(),
        peer_user_id: None,
        peer_username: String::new(),
        creator_id: if desc.creator_id == 0 {
            None
        } else {
            Some(UserId(desc.creator_id))
        },
        online: false,
        member_count,
        unread_count: desc.count_mess_unread.max(0) as u32,
        last_sent_timestamp: desc.last_sent_timestamp,
        last_seen_timestamp: desc.last_seen_timestamp,
    }
}

fn dm_peer_index(c: &ApiDirectChannel) -> usize {
    c.avatars
        .iter()
        .enumerate()
        .rfind(|(_, avatar)| !avatar.is_empty())
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// Builds the `UpdateChannelDesc` label/avatar pair for a group edit.
///
/// `edited_*` carries only what the user actually changed. The field they left
/// alone is refilled from `current` rather than sent as `None`, mirroring
/// React's `updateDmGroup` (`direct.slice.ts`): omitting a field there is not
/// trusted to mean "leave it", so a name-only edit can't clear the avatar.
/// An empty string stands in for React's default-avatar placeholder and is
/// never sent as a carry-over — but an empty `edited_avatar` is an explicit
/// removal and must reach the server.
fn group_desc_payload(
    current: Option<(&str, &str)>,
    edited_label: Option<&str>,
    edited_avatar: Option<&str>,
) -> (Option<String>, Option<String>) {
    let carry = |value: Option<&str>| value.filter(|value| !value.is_empty()).map(str::to_string);
    let label = match edited_label {
        Some(label) => Some(label.to_string()),
        None => carry(current.map(|(label, _)| label)),
    };
    let avatar = match edited_avatar {
        Some(avatar) => Some(avatar.to_string()),
        None => carry(current.map(|(_, avatar)| avatar)),
    };
    (label, avatar)
}

fn enrich_direct_from_event_users(
    channel: &mut DirectChannel,
    users: &[mezon_proto::realtime::UserProfileRedis],
    self_id: Option<UserId>,
) {
    let others: Vec<&mezon_proto::realtime::UserProfileRedis> = users
        .iter()
        .filter(|u| u.user_id != 0)
        .filter(|u| self_id.is_none_or(|me| UserId(u.user_id) != me))
        .collect();
    let others_label = others
        .iter()
        .map(|u| {
            if u.display_name.is_empty() {
                u.username.clone()
            } else {
                u.display_name.clone()
            }
        })
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    match channel.kind {
        DirectKind::Dm => {
            if !others_label.is_empty() {
                channel.label = others_label;
            }
            if let Some(peer) = others.first() {
                channel.peer_user_id = Some(UserId(peer.user_id));
                channel.peer_username = peer.username.clone();
                if !peer.avatar.is_empty() {
                    channel.avatar = peer.avatar.clone();
                }
                channel.online = channel.online || peer.online;
            }
        }
        DirectKind::Group => {
            if channel.label.is_empty() {
                channel.label = others_label;
            }
        }
    }
    if channel.member_count == 0 {
        channel.member_count = users.len() as u32;
    }
}

fn direct_from_message(
    m: &mezon_proto::api::ChannelMessage,
    from_me: bool,
    increment_unread: bool,
) -> DirectChannel {
    let kind = if m.mode == STREAM_MODE_GROUP {
        DirectKind::Group
    } else {
        DirectKind::Dm
    };
    let label = if !m.display_name.is_empty() {
        m.display_name.clone()
    } else {
        m.username.clone()
    };
    let ts = if m.create_time_seconds > 0 {
        i64::from(m.create_time_seconds)
    } else {
        0
    };
    let (peer_user_id, peer_username) = match kind {
        DirectKind::Dm => (
            (m.sender_id != 0).then_some(UserId(m.sender_id)),
            m.username.clone(),
        ),
        DirectKind::Group => (None, String::new()),
    };
    DirectChannel {
        id: ChannelId(m.channel_id),
        label,
        kind,
        avatar: m.avatar.clone(),
        peer_user_id,
        peer_username,
        creator_id: (m.sender_id != 0).then_some(UserId(m.sender_id)),
        online: true,
        member_count: 0,
        unread_count: u32::from(increment_unread),
        last_sent_timestamp: ts,
        last_seen_timestamp: if from_me { ts } else { ts.saturating_sub(1) },
    }
}

fn direct_from_api(c: ApiDirectChannel) -> DirectChannel {
    let kind = DirectKind::from_raw(c.channel_type);
    let (avatar, peer_user_id, peer_username, online) = match kind {
        DirectKind::Group => (c.channel_avatar.clone(), None, String::new(), false),
        DirectKind::Dm => {
            let peer_idx = dm_peer_index(&c);
            (
                c.avatars
                    .get(peer_idx)
                    .filter(|avatar| !avatar.is_empty())
                    .cloned()
                    .unwrap_or_default(),
                c.user_ids.get(peer_idx).copied().map(UserId),
                c.usernames
                    .get(peer_idx)
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_default(),
                c.onlines.get(peer_idx).copied().unwrap_or(false),
            )
        }
    };
    let label = if !c.channel_label.is_empty() {
        c.channel_label.clone()
    } else if !c.display_names.is_empty() {
        c.display_names.join(", ")
    } else {
        c.usernames.join(", ")
    };
    DirectChannel {
        id: ChannelId(c.channel_id),
        label,
        kind,
        avatar,
        peer_user_id,
        peer_username,
        creator_id: (c.creator_id != 0).then_some(UserId(c.creator_id)),
        online,
        member_count: c.member_count.max(0) as u32,
        unread_count: c.count_mess_unread.max(0) as u32,
        last_sent_timestamp: c.last_sent_timestamp,
        last_seen_timestamp: c.last_seen_timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_client::transport::ApiMessageContent;

    #[test]
    fn group_desc_payload_refills_the_untouched_field() {
        let current = Some(("Team", "team.png"));

        // Name-only edit still carries the avatar, so the server cannot read the
        // missing field as "clear it".
        assert_eq!(
            group_desc_payload(current, Some("Team v2"), None),
            (Some("Team v2".into()), Some("team.png".into()))
        );
        // ...and the same the other way around.
        assert_eq!(
            group_desc_payload(current, None, Some("new.png")),
            (Some("Team".into()), Some("new.png".into()))
        );
    }

    #[test]
    fn group_desc_payload_sends_empty_avatar_only_when_removed() {
        // Explicit removal must reach the server as an empty string.
        assert_eq!(
            group_desc_payload(Some(("Team", "team.png")), None, Some("")),
            (Some("Team".into()), Some(String::new()))
        );
        // A group that never had an avatar carries nothing.
        assert_eq!(
            group_desc_payload(Some(("Team", "")), Some("Team v2"), None),
            (Some("Team v2".into()), None)
        );
    }

    #[test]
    fn group_desc_payload_without_a_cached_channel_sends_only_edits() {
        assert_eq!(
            group_desc_payload(None, Some("Team v2"), None),
            (Some("Team v2".into()), None)
        );
    }

    fn api_dm(id: i64, label: &str, ty: u32) -> ApiDirectChannel {
        ApiDirectChannel {
            channel_id: id,
            channel_label: label.into(),
            channel_type: ty,
            channel_avatar: String::new(),
            avatars: vec!["peer.png".into()],
            usernames: vec!["peer".into()],
            display_names: vec!["Peer".into()],
            user_ids: vec![42],
            onlines: vec![true],
            member_count: 2,
            count_mess_unread: 0,
            last_sent_timestamp: 0,
            last_seen_timestamp: 0,
            creator_id: 0,
        }
    }

    #[test]
    fn direct_from_api_maps_group_creator_id() {
        let mut api = api_dm(1, "Group", 2);
        api.creator_id = 99;
        assert_eq!(direct_from_api(api).creator_id, Some(UserId(99)));
    }

    #[test]
    fn direct_from_api_zero_creator_is_none() {
        let api = api_dm(1, "Group", 2);
        assert_eq!(direct_from_api(api).creator_id, None);
    }

    fn incoming_dm_message() -> mezon_proto::api::ChannelMessage {
        mezon_proto::api::ChannelMessage {
            channel_id: 77,
            sender_id: 42,
            username: "stranger".into(),
            display_name: "Stranger Danger".into(),
            avatar: "stranger.png".into(),
            mode: 4,
            create_time_seconds: 1000,
            ..Default::default()
        }
    }

    #[test]
    fn stranger_dm_message_synthesizes_unread_conversation() {
        let dm = direct_from_message(&incoming_dm_message(), false, true);
        assert_eq!(dm.id, ChannelId(77));
        assert_eq!(dm.kind, DirectKind::Dm);
        assert_eq!(dm.label, "Stranger Danger");
        assert_eq!(dm.avatar, "stranger.png");
        assert_eq!(dm.peer_user_id, Some(UserId(42)));
        assert_eq!(dm.peer_username, "stranger");
        assert_eq!(dm.unread_count, 1);
        assert_eq!(dm.last_sent_timestamp, 1000);
        assert!(dm.is_unread());
    }

    #[test]
    fn own_message_synthesizes_seen_conversation() {
        let dm = direct_from_message(&incoming_dm_message(), true, false);
        assert_eq!(dm.unread_count, 0);
        assert_eq!(dm.last_seen_timestamp, dm.last_sent_timestamp);
        assert!(!dm.is_unread());
    }

    #[test]
    fn user_channel_added_dm_label_overrides_server_garbage() {
        let mut channel = DirectChannel {
            id: ChannelId(9),
            label: "dev.mezon".into(),
            kind: DirectKind::Dm,
            avatar: String::new(),
            peer_user_id: Some(UserId(999)),
            peer_username: "wrong".into(),
            creator_id: None,
            online: false,
            member_count: 0,
            unread_count: 0,
            last_sent_timestamp: 100,
            last_seen_timestamp: 0,
        };
        let users = vec![
            mezon_proto::realtime::UserProfileRedis {
                user_id: 1,
                username: "me".into(),
                ..Default::default()
            },
            mezon_proto::realtime::UserProfileRedis {
                user_id: 2,
                username: "potinoc605".into(),
                ..Default::default()
            },
        ];
        enrich_direct_from_event_users(&mut channel, &users, Some(UserId(1)));
        assert_eq!(channel.label, "potinoc605");
        assert_eq!(channel.peer_user_id, Some(UserId(2)));
        assert_eq!(channel.peer_username, "potinoc605");
    }

    #[test]
    fn user_channel_added_enriches_blank_dm_from_event_users() {
        let mut channel = DirectChannel {
            id: ChannelId(9),
            label: String::new(),
            kind: DirectKind::Dm,
            avatar: String::new(),
            peer_user_id: None,
            peer_username: String::new(),
            creator_id: None,
            online: false,
            member_count: 0,
            unread_count: 0,
            last_sent_timestamp: 100,
            last_seen_timestamp: 0,
        };
        let users = vec![
            mezon_proto::realtime::UserProfileRedis {
                user_id: 1,
                username: "me".into(),
                display_name: "Me".into(),
                ..Default::default()
            },
            mezon_proto::realtime::UserProfileRedis {
                user_id: 2,
                username: "newbie".into(),
                display_name: "New Bie".into(),
                avatar: "newbie.png".into(),
                online: true,
                ..Default::default()
            },
        ];
        enrich_direct_from_event_users(&mut channel, &users, Some(UserId(1)));
        assert_eq!(channel.label, "New Bie");
        assert_eq!(channel.peer_user_id, Some(UserId(2)));
        assert_eq!(channel.peer_username, "newbie");
        assert_eq!(channel.avatar, "newbie.png");
        assert!(channel.online);
        assert_eq!(channel.member_count, 2);
    }

    #[test]
    fn group_message_synthesizes_group_conversation() {
        let mut m = incoming_dm_message();
        m.mode = 3;
        m.display_name = String::new();
        let dm = direct_from_message(&m, false, true);
        assert_eq!(dm.kind, DirectKind::Group);
        assert_eq!(dm.label, "stranger");
        assert_eq!(dm.peer_user_id, None);
    }

    #[test]
    fn dm_maps_peer_avatar_and_id() {
        let dm = direct_from_api(api_dm(1, "Peer", 3));
        assert_eq!(dm.kind, DirectKind::Dm);
        assert_eq!(dm.avatar, "peer.png");
        assert_eq!(dm.peer_user_id, Some(UserId(42)));
        assert!(dm.online);
    }

    #[test]
    fn dm_uses_last_nonempty_avatar_when_multiple() {
        let mut api = api_dm(1, "Peer", 3);
        api.avatars = vec!["".into(), "peer.png".into()];
        api.user_ids = vec![1, 42];
        api.onlines = vec![true, false];
        let dm = direct_from_api(api);
        assert_eq!(dm.avatar, "peer.png");
        assert_eq!(dm.peer_user_id, Some(UserId(42)));
        assert!(!dm.online);
    }

    #[test]
    fn dm_skips_trailing_empty_avatar() {
        let mut api = api_dm(1, "Peer", 3);
        api.avatars = vec!["peer.png".into(), String::new()];
        let dm = direct_from_api(api);
        assert_eq!(dm.avatar, "peer.png");
    }

    #[test]
    fn dm_maps_unread_count_from_count_mess_unread() {
        let mut api = api_dm(1, "Peer", 3);
        api.count_mess_unread = 7;
        let dm = direct_from_api(api);
        assert_eq!(dm.unread_count, 7);
    }

    #[test]
    fn dm_is_unread_when_unread_count_nonzero() {
        let mut api = api_dm(1, "Peer", 3);
        api.count_mess_unread = 3;
        let dm = direct_from_api(api);
        assert!(dm.is_unread());
    }

    #[test]
    fn unread_dm_counts_toward_badge_when_not_muted() {
        let mut api = api_dm(1, "Peer", 3);
        api.count_mess_unread = 4;
        let dm = direct_from_api(api);
        assert!(dm.counts_toward_unread_badge(false));
    }

    #[test]
    fn muted_dm_does_not_count_even_with_unread() {
        let mut api = api_dm(1, "Peer", 3);
        api.count_mess_unread = 4;
        let dm = direct_from_api(api);
        assert!(!dm.counts_toward_unread_badge(true));
    }

    #[test]
    fn read_dm_never_counts_toward_badge() {
        let dm = direct_from_api(api_dm(1, "Peer", 3));
        assert_eq!(dm.unread_count, 0);
        assert!(!dm.counts_toward_unread_badge(false));
        assert!(!dm.counts_toward_unread_badge(true));
    }

    #[test]
    fn dm_counts_predicate_matches_unread_and_unmuted() {
        assert!(dm_counts_toward_unread_badge(1, false));
        assert!(!dm_counts_toward_unread_badge(1, true));
        assert!(!dm_counts_toward_unread_badge(0, false));
        assert!(!dm_counts_toward_unread_badge(0, true));
    }

    #[test]
    fn dm_is_unread_when_last_sent_after_seen() {
        let api = ApiDirectChannel {
            last_sent_timestamp: 200,
            last_seen_timestamp: 100,
            ..api_dm(1, "Peer", 3)
        };
        let dm = direct_from_api(api);
        assert!(dm.is_unread());
    }

    #[test]
    fn has_more_is_true_when_page_full() {
        let channels: Vec<ApiDirectChannel> = (0..DM_PAGE_SIZE)
            .map(|i| api_dm(i64::from(i), "x", 3))
            .collect();
        let has_more = channels.len() >= DM_PAGE_SIZE as usize;
        assert!(has_more);
    }

    #[test]
    fn has_more_is_false_when_page_partial() {
        let channels: Vec<ApiDirectChannel> =
            (0..10).map(|i| api_dm(i64::from(i), "x", 3)).collect();
        let has_more = channels.len() >= DM_PAGE_SIZE as usize;
        assert!(!has_more);
    }

    #[test]
    fn group_uses_channel_avatar_and_no_peer() {
        let mut api = api_dm(2, "Group", 2);
        api.channel_avatar = "group.png".into();
        let group = direct_from_api(api);
        assert_eq!(group.kind, DirectKind::Group);
        assert_eq!(group.avatar, "group.png");
        assert_eq!(group.peer_user_id, None);
    }

    #[test]
    fn empty_label_falls_back_to_display_names() {
        let api = api_dm(3, "", 3);
        let dm = direct_from_api(api);
        assert_eq!(dm.label, "Peer");
    }

    #[test]
    fn stream_mode_and_channel_type_match_react() {
        assert_eq!(DirectKind::Dm.stream_mode(), 4);
        assert_eq!(DirectKind::Group.stream_mode(), 3);
        assert_eq!(DirectKind::Dm.channel_type(), 3);
        assert_eq!(DirectKind::Group.channel_type(), 2);
    }

    fn api_channel_desc(
        id: i64,
        label: &str,
        ty: u32,
        member_count: i32,
        creator_id: i64,
    ) -> ApiChannelDesc {
        ApiChannelDesc {
            channel_id: id,
            channel_label: label.into(),
            channel_type: ty,
            clan_id: 0,
            category_name: String::new(),
            category_id: 0,
            channel_private: 1,
            count_mess_unread: 0,
            member_count,
            parent_id: 0,
            is_mute: false,
            last_seen_message_id: 0,
            last_seen_timestamp: 0,
            last_sent_message_id: 0,
            last_sent_timestamp: 0,
            badge_count: 0,
            creator_id,
            clan_name: String::new(),
            channel_avatar: String::new(),
            topic: String::new(),
            age_restricted: 0,
            e2ee: 0,
            app_id: 0,
            active: 1,
        }
    }

    #[test]
    fn group_from_created_marks_group_without_peer() {
        let desc = api_channel_desc(9, "server-label", 2, 4, 7);
        let group = direct_group_from_created(&desc, "phat, nghia", 3);
        assert_eq!(group.id, ChannelId(9));
        assert_eq!(group.kind, DirectKind::Group);
        assert_eq!(group.peer_user_id, None);
        assert_eq!(group.creator_id, Some(UserId(7)));
        assert_eq!(group.label, "phat, nghia");
        assert_eq!(group.member_count, 4);
    }

    #[test]
    fn group_from_created_falls_back_when_server_omits_count_and_label() {
        let desc = api_channel_desc(9, "server-label", 2, 0, 0);
        let group = direct_group_from_created(&desc, "", 3);
        assert_eq!(group.member_count, 3);
        assert_eq!(group.label, "server-label");
        assert_eq!(group.creator_id, None);
    }

    #[test]
    fn sort_orders_most_recent_first() {
        let mut chans = vec![
            DirectChannel {
                id: ChannelId(1),
                label: "a".into(),
                kind: DirectKind::Dm,
                avatar: String::new(),
                peer_user_id: None,
                peer_username: String::new(),
                creator_id: None,
                online: false,
                member_count: 0,
                unread_count: 0,
                last_sent_timestamp: 100,
                last_seen_timestamp: 0,
            },
            DirectChannel {
                id: ChannelId(2),
                label: "b".into(),
                kind: DirectKind::Dm,
                avatar: String::new(),
                peer_user_id: None,
                peer_username: String::new(),
                creator_id: None,
                online: false,
                member_count: 0,
                unread_count: 0,
                last_sent_timestamp: 200,
                last_seen_timestamp: 0,
            },
        ];
        sort_by_recent(&mut chans);
        assert_eq!(chans[0].id, ChannelId(2));
        assert_eq!(chans[1].id, ChannelId(1));
    }

    fn make_user_channel_added(channel_id: i64, channel_type: i32) -> RealtimeEvent {
        use mezon_proto::{api, realtime};
        RealtimeEvent::UserChannelAdded(realtime::UserChannelAdded {
            channel_desc: Some(api::ChannelDescription {
                channel_id,
                r#type: channel_type,
                channel_label: "dm-label".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    fn upsert_user_channel_added(channels: &mut DirectChannelList, event: &RealtimeEvent) {
        let RealtimeEvent::UserChannelAdded(e) = event else {
            return;
        };
        let Some(ref desc) = e.channel_desc else {
            return;
        };
        let channel_type = desc.r#type as u32;
        if channel_type != 2 && channel_type != 3 {
            return;
        }
        let api_ch = direct_from_channel_desc(desc);
        channels.push_new(direct_from_api(api_ch));
    }

    fn list_from(channels: Vec<DirectChannel>) -> DirectChannelList {
        let mut list = DirectChannelList::default();
        list.replace(channels, &HashMap::new());
        list
    }

    fn assert_index_consistent(list: &DirectChannelList) {
        assert_eq!(list.by_id.len(), list.channels.len());
        for (i, c) in list.channels.iter().enumerate() {
            assert_eq!(list.by_id.get(&c.id), Some(&i));
        }
    }

    #[test]
    fn user_channel_added_dm_inserts_new_channel() {
        let mut channels = DirectChannelList::default();
        let event = make_user_channel_added(99, 3);
        upsert_user_channel_added(&mut channels, &event);
        assert_eq!(channels.as_slice().len(), 1);
        assert_eq!(channels.as_slice()[0].id, ChannelId(99));
        assert_eq!(channels.as_slice()[0].kind, DirectKind::Dm);
        assert!(channels.find(ChannelId(99)).is_some());
        assert_index_consistent(&channels);
    }

    #[test]
    fn user_channel_added_group_inserts_new_channel() {
        let mut channels = DirectChannelList::default();
        let event = make_user_channel_added(42, 2);
        upsert_user_channel_added(&mut channels, &event);
        assert_eq!(channels.as_slice().len(), 1);
        assert_eq!(channels.as_slice()[0].id, ChannelId(42));
        assert_eq!(channels.as_slice()[0].kind, DirectKind::Group);
    }

    #[test]
    fn user_channel_added_skips_duplicate() {
        let mut channels = list_from(vec![direct_from_api(api_dm(99, "existing", 3))]);
        let event = make_user_channel_added(99, 3);
        upsert_user_channel_added(&mut channels, &event);
        assert_eq!(channels.as_slice().len(), 1);
        assert_index_consistent(&channels);
    }

    #[test]
    fn user_channel_added_ignores_non_dm_types() {
        let mut channels = DirectChannelList::default();
        let event = make_user_channel_added(55, 1);
        upsert_user_channel_added(&mut channels, &event);
        assert!(channels.as_slice().is_empty());
    }

    #[test]
    fn index_stays_consistent_after_sort_and_dedup() {
        let mut list = list_from(vec![
            DirectChannel {
                last_sent_timestamp: 100,
                ..direct_from_api(api_dm(1, "a", 3))
            },
            DirectChannel {
                last_sent_timestamp: 300,
                ..direct_from_api(api_dm(2, "b", 3))
            },
        ]);
        assert_eq!(list.as_slice()[0].id, ChannelId(2));
        assert_index_consistent(&list);
        list.extend_new(vec![
            DirectChannel {
                last_sent_timestamp: 200,
                ..direct_from_api(api_dm(3, "c", 3))
            },
            DirectChannel {
                last_sent_timestamp: 999,
                ..direct_from_api(api_dm(1, "dup", 3))
            },
        ]);
        let ids: Vec<ChannelId> = list.as_slice().iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![ChannelId(2), ChannelId(3), ChannelId(1)]);
        assert_index_consistent(&list);
        assert_eq!(list.find(ChannelId(3)).unwrap().label, "c");
    }

    #[test]
    fn merge_dm_unread_counts_takes_max_of_list_badges_and_local() {
        let mut channels = vec![DirectChannel {
            id: ChannelId(1),
            unread_count: 1,
            last_sent_timestamp: 100,
            ..direct_from_api(api_dm(1, "a", 3))
        }];
        let mut badge_map = HashMap::new();
        badge_map.insert(
            ChannelId(1),
            DmBadgeInfo {
                badge_count: 5,
                last_sent_timestamp: 100,
                last_seen_timestamp: 0,
            },
        );
        let mut local = HashMap::new();
        local.insert(ChannelId(1), 3);
        merge_dm_unread_counts(&mut channels, &badge_map, &local);
        assert_eq!(channels[0].unread_count, 5);
    }

    #[test]
    fn merge_dm_unread_counts_adopts_badge_last_sent_timestamp() {
        let mut channels = vec![DirectChannel {
            id: ChannelId(1),
            last_sent_timestamp: 100,
            ..direct_from_api(api_dm(1, "a", 3))
        }];
        let mut badge_map = HashMap::new();
        badge_map.insert(
            ChannelId(1),
            DmBadgeInfo {
                badge_count: 0,
                last_sent_timestamp: 900,
                last_seen_timestamp: 0,
            },
        );
        merge_dm_unread_counts(&mut channels, &badge_map, &HashMap::new());
        assert_eq!(channels[0].last_sent_timestamp, 900);
    }

    #[test]
    fn merge_dm_unread_counts_leaves_timestamp_when_channel_missing_from_badge_map() {
        let mut channels = vec![DirectChannel {
            id: ChannelId(1),
            last_sent_timestamp: 100,
            ..direct_from_api(api_dm(1, "a", 3))
        }];
        merge_dm_unread_counts(&mut channels, &HashMap::new(), &HashMap::new());
        assert_eq!(channels[0].last_sent_timestamp, 100);
    }

    #[test]
    fn merge_dm_unread_counts_adopts_badge_last_seen_timestamp_keeping_unread_state_consistent() {
        let mut channels = vec![DirectChannel {
            id: ChannelId(1),
            last_sent_timestamp: 100,
            last_seen_timestamp: 0,
            ..direct_from_api(api_dm(1, "a", 3))
        }];
        let mut badge_map = HashMap::new();
        badge_map.insert(
            ChannelId(1),
            DmBadgeInfo {
                badge_count: 0,
                last_sent_timestamp: 900,
                last_seen_timestamp: 900,
            },
        );
        merge_dm_unread_counts(&mut channels, &badge_map, &HashMap::new());
        assert_eq!(channels[0].last_sent_timestamp, 900);
        assert_eq!(channels[0].last_seen_timestamp, 900);
        assert!(!channels[0].is_unread());
    }

    fn dm(id: i64, ts: i64) -> DirectChannel {
        DirectChannel {
            last_sent_timestamp: ts,
            ..direct_from_api(api_dm(id, "x", 3))
        }
    }

    #[test]
    fn resort_after_bump_matches_full_sort() {
        let base = vec![dm(1, 500), dm(2, 400), dm(3, 400), dm(4, 300), dm(5, 200)];
        let scenarios = [
            (ChannelId(5), 600),
            (ChannelId(5), 400),
            (ChannelId(1), 100),
            (ChannelId(3), 400),
            (ChannelId(3), 450),
            (ChannelId(2), 350),
        ];
        for (id, new_ts) in scenarios {
            let mut list = list_from(base.clone());
            let mut expected = list.channels.clone();
            if let Some(c) = expected.iter_mut().find(|c| c.id == id) {
                c.last_sent_timestamp = new_ts;
            }
            sort_by_recent(&mut expected);
            if let Some(c) = list.find_mut(id) {
                c.last_sent_timestamp = new_ts;
            }
            list.resort_after_bump(id);
            let got: Vec<ChannelId> = list.channels.iter().map(|c| c.id).collect();
            let want: Vec<ChannelId> = expected.iter().map(|c| c.id).collect();
            assert_eq!(got, want, "id={id:?} new_ts={new_ts}");
            assert_index_consistent(&list);
        }
    }

    #[test]
    fn replace_carries_only_pending_created_dm() {
        let mut list = list_from(vec![
            dm(1, 100),
            dm(2, 200),
            DirectChannel {
                unread_count: 7,
                ..dm(3, 300)
            },
        ]);
        list.pending_created = Some(ChannelId(3));

        let remote = vec![dm(1, 150)];
        list.replace(remote, &HashMap::new());

        let ids: Vec<ChannelId> = list.channels.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![ChannelId(3), ChannelId(1)]);
        assert_eq!(list.find(ChannelId(3)).unwrap().unread_count, 7);
        assert_eq!(list.pending_created, Some(ChannelId(3)));
        assert_index_consistent(&list);
    }

    #[test]
    fn replace_clears_pending_created_when_server_returns_it() {
        let mut list = list_from(vec![dm(1, 100), dm(99, 50)]);
        list.pending_created = Some(ChannelId(99));
        list.replace(vec![dm(1, 100), dm(99, 80)], &HashMap::new());
        assert_eq!(list.pending_created, None);
        assert_eq!(list.channels.len(), 2);
        assert_index_consistent(&list);
    }

    #[test]
    fn replace_does_not_retain_unrelated_local_only_dms() {
        let mut list = list_from(vec![dm(1, 100), dm(2, 200)]);
        list.replace(vec![dm(1, 150)], &HashMap::new());
        let ids: Vec<ChannelId> = list.channels.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![ChannelId(1)]);
        assert_index_consistent(&list);
    }

    #[test]
    fn direct_body_text_is_wrapped_into_content_json() {
        let json = DirectMessageBody::Text("hello".into()).into_content_json();
        let parsed: ApiMessageContent = serde_json::from_str(&json).expect("json");
        assert_eq!(parsed.t, "hello");
    }

    #[test]
    fn direct_body_text_that_looks_like_json_is_still_wrapped() {
        let json = DirectMessageBody::Text(r#"{"t":"","foo":1}"#.into()).into_content_json();
        let parsed: ApiMessageContent = serde_json::from_str(&json).expect("json");
        assert_eq!(parsed.t, r#"{"t":"","foo":1}"#);
    }

    #[test]
    fn direct_body_content_json_passes_through() {
        let structured = build_send_content("hello", &[], &[], &[]).json;
        assert_eq!(
            DirectMessageBody::ContentJson(structured.clone()).into_content_json(),
            structured
        );
    }
}
