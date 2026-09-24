use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::anyhow;

use gpui::{App, AppContext, Context, Entity, Global, Task};
use mezon_client::{AppApi, RealtimeEvent};
use mezon_stream::{StreamEvent, StreamSession, StreamSessionConfig, StreamTokenProvider};

use crate::AppConfig;
use crate::AuthState;
use crate::ChannelType;
use crate::ids::{ChannelId, ClanId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const STREAMING_CHANNEL_TYPE: i32 = 6;
const STREAM_MEMBER_STATE_ACTIVE: i32 = 1;
const STREAM_FETCH_LIMIT: i32 = 100;
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROLS_HIDE_DELAY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamMember {
    pub user_id: UserId,
    pub channel_id: ChannelId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamPhase {
    Idle,
    Joining {
        channel_id: ChannelId,
        clan_id: ClanId,
    },
    Joined {
        channel_id: ChannelId,
        clan_id: ClanId,
        is_live: bool,
    },
    Error(String),
}

pub struct StreamStore {
    api: Arc<AppApi>,
    phase: StreamPhase,
    members: HashMap<(ChannelId, UserId), StreamMember>,
    active_clan: Option<ClanId>,
    last_viewed_stream_channel: Option<ChannelId>,
    show_chat: bool,
    show_members: bool,
    controls_visible: bool,
    controls_hide_at: Option<Instant>,
    playback_blocked: bool,
    volume: f32,
    muted: bool,
    fullscreen: bool,
    error_message: Option<String>,
    session: Option<StreamSession>,
    session_generation: u64,
    session_channel_label: String,
    session_clan_name: String,
    session_user_id: Option<UserId>,
    join_started: Option<Instant>,
    _fetch_task: Option<Task<()>>,
    _token_task: Option<Task<()>>,
    _session_task: Option<Task<()>>,
    _join_timeout: Option<Task<()>>,
    _controls_hide_task: Option<Task<()>>,
}

struct GlobalStreamStore(Entity<StreamStore>);
impl Global for GlobalStreamStore {}

impl StreamStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalStreamStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalStreamStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalStreamStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        Self {
            api,
            phase: StreamPhase::Idle,
            members: HashMap::new(),
            active_clan: None,
            last_viewed_stream_channel: None,
            show_chat: false,
            show_members: true,
            controls_visible: true,
            controls_hide_at: None,
            playback_blocked: false,
            volume: 1.0,
            muted: false,
            fullscreen: false,
            error_message: None,
            session: None,
            session_generation: 0,
            session_channel_label: String::new(),
            session_clan_name: String::new(),
            session_user_id: None,
            join_started: None,
            _fetch_task: None,
            _token_task: None,
            _session_task: None,
            _join_timeout: None,
            _controls_hide_task: None,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::StreamingJoined, &entity, |this, event, cx| {
                this.handle_streaming_joined(event, cx)
            });
            dispatch.on(RealtimeKind::StreamingLeaved, &entity, |this, event, cx| {
                this.handle_streaming_leaved(event, cx)
            });
            dispatch.on(
                RealtimeKind::StreamingStarted,
                &entity,
                |this, event, cx| this.handle_streaming_started(event, cx),
            );
            dispatch.on(RealtimeKind::StreamingEnded, &entity, |this, event, cx| {
                this.handle_streaming_ended(event, cx)
            });
        });
    }

    pub fn phase(&self) -> &StreamPhase {
        &self.phase
    }

    pub fn is_joined(&self) -> bool {
        matches!(self.phase, StreamPhase::Joined { .. })
    }

    pub fn is_joining(&self) -> bool {
        matches!(self.phase, StreamPhase::Joining { .. })
    }

    pub fn session_channel_id(&self) -> Option<ChannelId> {
        match &self.phase {
            StreamPhase::Joining { channel_id, .. } | StreamPhase::Joined { channel_id, .. } => {
                Some(*channel_id)
            }
            _ => None,
        }
    }

    pub fn is_session_channel(&self, channel_id: ChannelId) -> bool {
        self.session_channel_id() == Some(channel_id)
    }

    pub fn sync_chat_for_active_channel(
        &mut self,
        active: Option<(ChannelType, ChannelId)>,
        cx: &mut Context<Self>,
    ) {
        if !self.show_chat {
            return;
        }
        let Some((ChannelType::Stream, channel_id)) = active else {
            return;
        };
        if self.is_session_channel(channel_id) && (self.is_joined() || self.is_joining()) {
            return;
        }
        self.show_chat = false;
        cx.notify();
    }

    pub fn show_chat(&self) -> bool {
        self.show_chat
    }

    pub fn show_members(&self) -> bool {
        self.show_members
    }

    pub fn controls_visible(&self) -> bool {
        self.controls_visible
    }

    pub fn bump_controls_visible(&mut self, cx: &mut Context<Self>) {
        let was_visible = self.controls_visible;
        self.controls_visible = true;
        self.controls_hide_at = Some(Instant::now() + CONTROLS_HIDE_DELAY);
        if self._controls_hide_task.is_none() {
            self._controls_hide_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    let wait = this
                        .update(cx, |this, _| {
                            this.controls_hide_at
                                .map(|at| at.saturating_duration_since(Instant::now()))
                        })
                        .ok()
                        .flatten();
                    let Some(wait) = wait else {
                        break;
                    };
                    if !wait.is_zero() {
                        cx.background_executor().timer(wait).await;
                    }
                    let hide = this
                        .update(cx, |this, _| {
                            this.controls_hide_at.is_none_or(|at| Instant::now() >= at)
                        })
                        .unwrap_or(true);
                    if hide {
                        this.update(cx, |this, cx| {
                            this.controls_visible = false;
                            this.controls_hide_at = None;
                            this._controls_hide_task = None;
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                }
            }));
        }
        if !was_visible {
            cx.notify();
        }
    }

    pub fn clear_error_on_active_channel_change(
        &mut self,
        active: Option<(ChannelType, ChannelId)>,
        cx: &mut Context<Self>,
    ) {
        let viewed = match active {
            Some((ChannelType::Stream, channel_id)) => Some(channel_id),
            _ => None,
        };
        if self.last_viewed_stream_channel == viewed {
            return;
        }
        self.last_viewed_stream_channel = viewed;
        if self.error_message.take().is_some() {
            if matches!(self.phase, StreamPhase::Error(_)) {
                self.phase = StreamPhase::Idle;
            }
            cx.notify();
        }
    }

    pub fn playback_blocked(&self) -> bool {
        self.playback_blocked
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    pub fn effective_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    pub fn session_channel_label(&self) -> &str {
        &self.session_channel_label
    }

    pub fn session_clan_name(&self) -> &str {
        &self.session_clan_name
    }

    pub fn members_for_channel(&self, channel_id: ChannelId) -> Vec<StreamMember> {
        self.members
            .values()
            .filter(|m| m.channel_id == channel_id)
            .cloned()
            .collect()
    }

    pub fn set_active_clan(&mut self, clan_id: Option<ClanId>, cx: &mut Context<Self>) {
        if self.active_clan == clan_id {
            return;
        }
        self.active_clan = clan_id;
        if let Some(clan_id) = clan_id {
            self.fetch_members(clan_id, cx);
        }
    }

    pub fn fetch_members(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let api = self.api.clone();
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            let result = api
                .list_streaming_channel_users(
                    clan_id.get(),
                    0,
                    STREAMING_CHANNEL_TYPE,
                    STREAM_MEMBER_STATE_ACTIVE,
                    STREAM_FETCH_LIMIT,
                )
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(list) => {
                        this.members.clear();
                        for user in list.streaming_channel_users {
                            let user_id = UserId(user.user_id);
                            let channel_id = ChannelId(user.channel_id);
                            let display_name = if user.participant.is_empty() {
                                user_id.to_string()
                            } else {
                                user.participant.clone()
                            };
                            this.members.insert(
                                (channel_id, user_id),
                                StreamMember {
                                    user_id,
                                    channel_id,
                                    display_name,
                                },
                            );
                        }
                        if let Some(store) = crate::ClanMembersStore::try_global(cx) {
                            store.update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
                        }
                        if let Some(store) = crate::UsersByUserStore::try_global(cx) {
                            store.update(cx, |store, cx| store.ensure_loaded(cx));
                        }
                    }
                    Err(err) => tracing::warn!("list_streaming_channel_users failed: {err:#}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn toggle_chat(&mut self, cx: &mut Context<Self>) {
        self.show_chat = !self.show_chat;
        cx.notify();
    }

    pub fn toggle_members(&mut self, cx: &mut Context<Self>) {
        self.show_members = !self.show_members;
        cx.notify();
    }

    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.volume = volume.clamp(0.0, 1.0);
        self.muted = self.volume <= 0.0;
        self.sync_audio_playback();
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.sync_audio_playback();
        cx.notify();
    }

    pub fn toggle_fullscreen(&mut self, cx: &mut Context<Self>) {
        self.fullscreen = !self.fullscreen;
        cx.notify();
    }

    pub fn clear_fullscreen(&mut self, cx: &mut Context<Self>) {
        if self.fullscreen {
            self.fullscreen = false;
            cx.notify();
        }
    }

    fn sync_audio_playback(&self) {
        if let Some(session) = &self.session
            && let Some(audio) = session.audio()
        {
            audio.set_volume(self.volume);
            audio.set_muted(self.muted);
        }
    }

    pub fn clear_playback_blocked(&mut self, cx: &mut Context<Self>) {
        self.playback_blocked = false;
        cx.notify();
    }

    pub fn join_stream(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        channel_label: &str,
        clan_name: &str,
        auth: &AuthState,
        config: &AppConfig,
        output_device_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let AuthState::Authenticated(session) = auth else {
            self.phase = StreamPhase::Error("Not authenticated".into());
            self.error_message = Some("Not authenticated".into());
            cx.notify();
            return;
        };
        if config.sfu_ws_url.is_empty() {
            self.phase = StreamPhase::Error("SFU server not configured".into());
            self.error_message = Some("SFU server not configured".into());
            cx.notify();
            return;
        }

        self.disconnect_session();
        let session_generation = self.session_generation;
        self.session_channel_label = channel_label.to_string();
        self.session_clan_name = clan_name.to_string();
        self.phase = StreamPhase::Joining {
            channel_id,
            clan_id,
        };
        self.join_started = Some(Instant::now());
        self.error_message = None;
        self.playback_blocked = false;
        self.session_user_id = session.user_id.parse::<i64>().ok().map(UserId);
        cx.notify();

        let api = self.api.clone();
        let room = channel_id.to_string();
        let ws_url = config.sfu_ws_url.clone();
        let provider_api = api.clone();
        let provider_room = room.clone();
        let provider: StreamTokenProvider = Arc::new(move || {
            let api = provider_api.clone();
            let room = provider_room.clone();
            Box::pin(async move {
                api.generate_meet_token(&room, &room, "")
                    .await
                    .map_err(|error| anyhow!("{error:#}"))
            })
        });
        let volume = self.volume;
        let muted = self.muted;
        let token_task = cx.spawn(async move |this, cx| {
            let token = api.generate_meet_token(&room, &room, "").await;
            let _ = this.update(cx, |this, cx| {
                if this.session_generation != session_generation
                    || !this.is_session_channel(channel_id)
                    || !this.is_joining()
                {
                    return;
                }
                let token = match token {
                    Ok(token) if !token.is_empty() => token,
                    Ok(_) => {
                        this.fail_stream("SFU returned an empty meet token".into(), cx);
                        return;
                    }
                    Err(error) => {
                        this.fail_stream(format!("Unable to obtain SFU meet token: {error:#}"), cx);
                        return;
                    }
                };
                let session_config = StreamSessionConfig {
                    ws_url,
                    token,
                    room,
                    token_provider: provider,
                };
                let stream_session =
                    StreamSession::start(session_config, output_device_id, volume, muted);
                let events = stream_session.events().clone();
                this.session = Some(stream_session);
                this._session_task = Some(cx.spawn(async move |this, cx| {
                    while let Ok(event) = events.recv_async().await {
                        let stop = this
                            .update(cx, |this, cx| {
                                this.handle_stream_event(
                                    session_generation,
                                    clan_id,
                                    channel_id,
                                    event,
                                    cx,
                                );
                                !this.is_joined() && !this.is_joining()
                            })
                            .unwrap_or(true);
                        if stop {
                            break;
                        }
                    }
                }));
            });
        });
        self._token_task = Some(token_task);

        self._join_timeout = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(JOIN_TIMEOUT).await;
            this.update(cx, |this, cx| {
                if this.session_generation == session_generation && this.is_joining() {
                    this.disconnect_session();
                    this.phase = StreamPhase::Error("Join timed out".into());
                    this.error_message = Some("Join timed out".into());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    pub fn leave_stream(&mut self, cx: &mut Context<Self>) {
        self.leave_stream_internal(false, cx);
    }

    fn leave_stream_internal(&mut self, clear_channel_members: bool, cx: &mut Context<Self>) {
        let channel_id = self.session_channel_id();
        let user_id = self.session_user_id;
        self.disconnect_session();
        self.phase = StreamPhase::Idle;
        self.error_message = None;
        self.fullscreen = false;
        self.controls_visible = false;
        self.controls_hide_at = None;
        self._controls_hide_task = None;
        self.show_chat = false;
        self.session_channel_label.clear();
        self.session_clan_name.clear();
        self.session_user_id = None;
        if clear_channel_members {
            if let Some(channel_id) = channel_id {
                self.members
                    .retain(|(member_channel_id, _), _| *member_channel_id != channel_id);
            }
        } else if let (Some(channel_id), Some(user_id)) = (channel_id, user_id) {
            self.members.remove(&(channel_id, user_id));
        }
        cx.notify();
    }

    fn disconnect_session(&mut self) {
        self.session_generation = self.session_generation.wrapping_add(1);
        self._token_task = None;
        if let Some(session) = self.session.take() {
            session.disconnect();
        }
        self._session_task = None;
        self._join_timeout = None;
        self.playback_blocked = false;
        self.join_started = None;
    }

    fn fail_stream(&mut self, message: String, cx: &mut Context<Self>) {
        self.disconnect_session();
        self.phase = StreamPhase::Error(message.clone());
        self.error_message = Some(message);
        cx.notify();
    }

    fn handle_stream_event(
        &mut self,
        session_generation: u64,
        clan_id: ClanId,
        channel_id: ChannelId,
        event: StreamEvent,
        cx: &mut Context<Self>,
    ) {
        if self.session_generation != session_generation {
            return;
        }
        match event {
            StreamEvent::Live => {
                self._join_timeout = None;
                self.phase = StreamPhase::Joined {
                    channel_id,
                    clan_id,
                    is_live: true,
                };
                self.bump_controls_visible(cx);
            }
            StreamEvent::NoBroadcast => {
                self._join_timeout = None;
                self.phase = StreamPhase::Joined {
                    channel_id,
                    clan_id,
                    is_live: false,
                };
                self.bump_controls_visible(cx);
            }
            StreamEvent::RemoteAudio(_) => {}
            StreamEvent::PlaybackBlocked => {
                self.playback_blocked = true;
            }
            StreamEvent::Error(message) => {
                self.fail_stream(message, cx);
                return;
            }
            StreamEvent::Disconnected => {
                if self.is_joined() || self.is_joining() {
                    self.leave_stream_internal(true, cx);
                    return;
                }
            }
        }
        self.sync_audio_playback();
        cx.notify();
    }

    fn handle_streaming_joined(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::StreamingJoined(e) = event else {
            return;
        };
        let channel_id = ChannelId(e.streaming_channel_id);
        let user_id = UserId(e.user_id);
        let display_name = if e.participant.is_empty() {
            user_id.to_string()
        } else {
            e.participant.clone()
        };
        self.members.insert(
            (channel_id, user_id),
            StreamMember {
                user_id,
                channel_id,
                display_name,
            },
        );
        cx.notify();
    }

    fn handle_streaming_leaved(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::StreamingLeaved(e) = event else {
            return;
        };
        let Ok(channel_id) = e.streaming_channel_id.parse::<i64>() else {
            return;
        };
        let channel_id = ChannelId(channel_id);
        let Ok(user_id) = e.streaming_user_id.parse::<i64>() else {
            return;
        };
        let user_id = UserId(user_id);
        self.members.remove(&(channel_id, user_id));

        let local_session_left = self.is_session_channel(channel_id)
            && (self.is_joined() || self.is_joining())
            && self.session_user_id == Some(user_id);
        if local_session_left {
            self.leave_stream_internal(true, cx);
            return;
        }
        cx.notify();
    }

    fn handle_streaming_started(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::StreamingStarted(e) = event else {
            return;
        };
        let channel_id = ChannelId(e.channel_id);
        if let Some(clan_id) = self.active_clan {
            self.fetch_members(clan_id, cx);
        }
        if let StreamPhase::Joined {
            channel_id: active,
            is_live,
            ..
        } = &mut self.phase
            && *active == channel_id
        {
            *is_live = e.is_streaming;
            cx.notify();
        }
    }

    fn handle_streaming_ended(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::StreamingEnded(e) = event else {
            return;
        };
        let channel_id = ChannelId(e.channel_id);
        self.members.retain(|(cid, _), _| *cid != channel_id);
        if let StreamPhase::Joined {
            channel_id: active,
            is_live,
            ..
        } = &mut self.phase
            && *active == channel_id
        {
            *is_live = false;
            cx.notify();
        } else {
            cx.notify();
        }
    }

    pub fn on_logout(&mut self, cx: &mut Context<Self>) {
        self.leave_stream(cx);
        self.members.clear();
        self.show_chat = false;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_member(channel_id: ChannelId, user_id: UserId, name: &str) -> StreamMember {
        StreamMember {
            user_id,
            channel_id,
            display_name: name.into(),
        }
    }

    fn init_store(cx: &mut gpui::TestAppContext) -> Entity<StreamStore> {
        cx.update(|cx| {
            let api = Arc::new(mezon_client::AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            RealtimeDispatch::init(api.clone(), cx);
            cx.new(|cx| StreamStore::new(api, cx))
        })
    }

    #[gpui::test]
    fn viewing_another_stream_channel_keeps_the_session(cx: &mut gpui::TestAppContext) {
        let joined = ChannelId(1);
        let other = ChannelId(2);
        let store = init_store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.phase = StreamPhase::Joined {
                    channel_id: joined,
                    clan_id: ClanId(9),
                    is_live: true,
                };
                store.show_chat = true;
                store.sync_chat_for_active_channel(Some((ChannelType::Stream, other)), cx);
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert!(store.is_joined());
            assert_eq!(store.session_channel_id(), Some(joined));
            assert!(store.is_session_channel(joined));
            assert!(!store.is_session_channel(other));
            assert!(!store.show_chat());
        });
    }

    #[gpui::test]
    fn viewing_the_session_channel_keeps_chat_open(cx: &mut gpui::TestAppContext) {
        let joined = ChannelId(1);
        let store = init_store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.phase = StreamPhase::Joined {
                    channel_id: joined,
                    clan_id: ClanId(9),
                    is_live: true,
                };
                store.show_chat = true;
                store.sync_chat_for_active_channel(Some((ChannelType::Stream, joined)), cx);
            });
        });

        assert!(cx.update(|cx| store.read(cx).show_chat()));
    }

    #[test]
    fn members_for_channel_filters_by_id() {
        let ch_a = ChannelId(1);
        let ch_b = ChannelId(2);
        let mut members = HashMap::new();
        members.insert((ch_a, UserId(10)), sample_member(ch_a, UserId(10), "a"));
        members.insert((ch_b, UserId(20)), sample_member(ch_b, UserId(20), "b"));
        let filtered: Vec<_> = members
            .values()
            .filter(|m| m.channel_id == ch_a)
            .cloned()
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].user_id, UserId(10));
    }

    #[gpui::test]
    fn local_stream_leave_resets_session_and_all_channel_members(cx: &mut gpui::TestAppContext) {
        let channel_id = ChannelId(1);
        let other_channel_id = ChannelId(2);
        let local_user_id = UserId(10);
        let other_user_id = UserId(20);
        let store = init_store(cx);
        let event = RealtimeEvent::StreamingLeaved(mezon_proto::realtime::StreamingLeavedEvent {
            streaming_channel_id: channel_id.to_string(),
            streaming_user_id: local_user_id.to_string(),
            ..Default::default()
        });

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.phase = StreamPhase::Joined {
                    channel_id,
                    clan_id: ClanId(9),
                    is_live: true,
                };
                store.session_user_id = Some(local_user_id);
                store.show_chat = true;
                store.members.insert(
                    (channel_id, local_user_id),
                    sample_member(channel_id, local_user_id, "local"),
                );
                store.members.insert(
                    (channel_id, other_user_id),
                    sample_member(channel_id, other_user_id, "other"),
                );
                store.members.insert(
                    (other_channel_id, other_user_id),
                    sample_member(other_channel_id, other_user_id, "other channel"),
                );

                store.handle_streaming_leaved(&event, cx);
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert_eq!(store.phase(), &StreamPhase::Idle);
            assert_eq!(store.session_channel_id(), None);
            assert!(!store.show_chat());
            assert!(store.members_for_channel(channel_id).is_empty());
            assert_eq!(store.members_for_channel(other_channel_id).len(), 1);
        });
    }

    #[gpui::test]
    fn another_user_stream_leave_only_removes_that_member(cx: &mut gpui::TestAppContext) {
        let channel_id = ChannelId(1);
        let local_user_id = UserId(10);
        let other_user_id = UserId(20);
        let store = init_store(cx);
        let event = RealtimeEvent::StreamingLeaved(mezon_proto::realtime::StreamingLeavedEvent {
            streaming_channel_id: channel_id.to_string(),
            streaming_user_id: other_user_id.to_string(),
            ..Default::default()
        });

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.phase = StreamPhase::Joined {
                    channel_id,
                    clan_id: ClanId(9),
                    is_live: true,
                };
                store.session_user_id = Some(local_user_id);
                store.members.insert(
                    (channel_id, local_user_id),
                    sample_member(channel_id, local_user_id, "local"),
                );
                store.members.insert(
                    (channel_id, other_user_id),
                    sample_member(channel_id, other_user_id, "other"),
                );

                store.handle_streaming_leaved(&event, cx);
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert!(store.is_joined());
            assert_eq!(store.session_channel_id(), Some(channel_id));
            assert_eq!(store.members_for_channel(channel_id).len(), 1);
            assert_eq!(
                store.members_for_channel(channel_id)[0].user_id,
                local_user_id
            );
        });
    }

    #[gpui::test]
    fn terminal_session_disconnect_resets_the_current_stream_channel(
        cx: &mut gpui::TestAppContext,
    ) {
        let channel_id = ChannelId(1);
        let other_channel_id = ChannelId(2);
        let store = init_store(cx);

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.phase = StreamPhase::Joined {
                    channel_id,
                    clan_id: ClanId(9),
                    is_live: true,
                };
                store.members.insert(
                    (channel_id, UserId(10)),
                    sample_member(channel_id, UserId(10), "current"),
                );
                store.members.insert(
                    (other_channel_id, UserId(20)),
                    sample_member(other_channel_id, UserId(20), "other channel"),
                );

                store.handle_stream_event(0, ClanId(9), channel_id, StreamEvent::Disconnected, cx);
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert_eq!(store.phase(), &StreamPhase::Idle);
            assert!(store.members_for_channel(channel_id).is_empty());
            assert_eq!(store.members_for_channel(other_channel_id).len(), 1);
        });
    }
}
