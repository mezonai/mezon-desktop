use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task};
use mezon_client::gotify::{BACKOFF_BASE, next_backoff, with_jitter};
use mezon_client::{AppApi, GotifyExtras, GotifyNotification, StreamEnd};

use crate::channel::ChannelList;
use crate::clan::ClanList;
use crate::config::AppConfig;
use crate::connection::ConnectionStore;
use crate::ids::ChannelId;
use crate::messages::MessagesStore;
use crate::platform::{DesktopNotification, NotificationTopicReply, PlatformStore};
use crate::presence::PresenceStore;
use crate::{AuthState, Settings};

const USER_STATUS_DO_NOT_DISTURB: &str = "Do Not Disturb";
const DEVICE_PLATFORM: &str = "desktop";
const SHOW_SENDER_AVATAR: bool = false;

/// Consumes the server-rendered Gotify notification stream (React `MezonNotificationService`)
/// and raises OS notifications through [`PlatformStore`]. This is the source of channel
/// notifications for non-mention messages: the server pushes whatever the channel's
/// notification level allows, and the client only suppresses DND / own-message /
/// currently-viewing-while-focused.
pub struct NotificationPushStore {
    auth_state: Entity<AuthState>,
    api: Arc<AppApi>,
    connection: Option<Task<()>>,
    connected_user: Option<String>,
    device_id: Option<String>,
    online: tokio::sync::watch::Sender<bool>,
    _auth_sub: Subscription,
    _connection_sub: Option<Subscription>,
}

struct GlobalNotificationPushStore(Entity<NotificationPushStore>);
impl Global for GlobalNotificationPushStore {}

impl NotificationPushStore {
    pub fn init(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| {
            let auth_sub = cx.observe(&auth_state, Self::on_auth_changed);
            let connection_store = ConnectionStore::try_global(cx);
            let starts_online = connection_store
                .as_ref()
                .map(|store| store.read(cx).is_online())
                .unwrap_or(true);
            let (online, _) = tokio::sync::watch::channel(starts_online);
            let connection_sub = connection_store.map(|store| {
                cx.observe(&store, |this: &mut Self, store, cx| {
                    let is_online = store.read(cx).is_online();
                    this.online.send_if_modified(|current| {
                        let changed = *current != is_online;
                        *current = is_online;
                        changed
                    });
                })
            });
            let mut this = Self {
                auth_state: auth_state.clone(),
                api,
                connection: None,
                connected_user: None,
                device_id: None,
                online,
                _auth_sub: auth_sub,
                _connection_sub: connection_sub,
            };
            this.sync_connection(cx);
            this
        });
        cx.set_global(GlobalNotificationPushStore(entity.clone()));
        entity
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalNotificationPushStore>()
            .map(|g| g.0.clone())
    }

    fn on_auth_changed(this: &mut Self, _auth: Entity<AuthState>, cx: &mut Context<Self>) {
        this.sync_connection(cx);
    }

    fn authenticated_user(&self, cx: &App) -> Option<String> {
        match self.auth_state.read(cx) {
            AuthState::Authenticated(session) => Some(session.user_id.clone()),
            _ => None,
        }
    }

    fn sync_connection(&mut self, cx: &mut Context<Self>) {
        let user = self.authenticated_user(cx);
        match user {
            Some(user_id) if self.connected_user.as_deref() != Some(user_id.as_str()) => {
                self.start(user_id, cx);
            }
            None if self.connection.is_some() => {
                self.connection = None;
                self.connected_user = None;
            }
            _ => {}
        }
    }

    fn start(&mut self, user_id: String, cx: &mut Context<Self>) {
        let Some(ws_base) = AppConfig::try_global(cx)
            .map(|c| c.notification_ws_url.clone())
            .filter(|u| !u.is_empty())
        else {
            return;
        };
        let permitted = PlatformStore::try_global(cx)
            .map(|p| p.read(cx).notifications_permitted())
            .unwrap_or(true);
        if !permitted {
            tracing::warn!("gotify: OS notifications denied; not connecting");
            return;
        }
        let api = self.api.clone();
        let mut online = self.online.subscribe();
        let timer = cx.background_executor().clone();
        self.connected_user = Some(user_id.clone());
        self.connection = Some(cx.spawn(async move |this, cx| {
            let mut backoff = BACKOFF_BASE;
            let mut token: Option<String> = None;

            loop {
                if !*online.borrow() {
                    tracing::debug!("gotify: offline, parking until the network returns");
                    if online.wait_for(|up| *up).await.is_err() {
                        return;
                    }
                    tracing::debug!("gotify: network back, reconnecting");
                    backoff = BACKOFF_BASE;
                }

                let stream_token = match token.clone() {
                    Some(token) => token,
                    None => {
                        let Ok(device_id) =
                            this.update(cx, |store, _| store.device_id.clone().unwrap_or_default())
                        else {
                            return;
                        };
                        match api
                            .regist_fcm_device_token(&device_id, "", DEVICE_PLATFORM)
                            .await
                        {
                            Ok((fresh, device_id)) if !fresh.trim().is_empty() => {
                                if this
                                    .update(cx, |store, _| {
                                        if !device_id.is_empty() {
                                            store.device_id = Some(device_id);
                                        }
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                                tracing::debug!("gotify: notification token registered");
                                let fresh = fresh.trim().to_string();
                                token = Some(fresh.clone());
                                fresh
                            }
                            Ok(_) => {
                                tracing::warn!("gotify: empty notification token; retrying");
                                timer.timer(with_jitter(backoff)).await;
                                backoff = next_backoff(backoff);
                                continue;
                            }
                            Err(e) => {
                                tracing::warn!("gotify: failed to register device token: {e}");
                                timer.timer(with_jitter(backoff)).await;
                                backoff = next_backoff(backoff);
                                continue;
                            }
                        }
                    }
                };

                tracing::debug!("gotify: opening stream");
                let (mut rx, ended) = api.spawn_gotify_stream(ws_base.clone(), stream_token);
                while let Some(notification) = rx.recv().await {
                    let prepared = this
                        .update(cx, |_, cx| {
                            note_dm_unread(cx, &user_id, &notification);
                            prepare(cx, &user_id, &notification)
                        })
                        .ok()
                        .flatten();
                    let Some(prepared) = prepared else {
                        if this.update(cx, |_, _| {}).is_err() {
                            return;
                        }
                        continue;
                    };
                    let icon_path = match prepared.icon_url.as_deref() {
                        Some(url) => api
                            .download_notification_icon(url)
                            .await
                            .ok()
                            .map(|p| p.to_string_lossy().into_owned()),
                        None => None,
                    };
                    if this
                        .update(cx, |_, cx| show_prepared(cx, prepared, icon_path))
                        .is_err()
                    {
                        return;
                    }
                }

                match ended.await.unwrap_or(StreamEnd::ConnectFailed) {
                    StreamEnd::ReceiverGone => return,
                    StreamEnd::Dropped => backoff = BACKOFF_BASE,
                    StreamEnd::Rejected => {
                        tracing::warn!("gotify: stream refused the token, re-registering");
                        token = None;
                    }
                    StreamEnd::ConnectFailed | StreamEnd::ClosedByServer => {}
                }

                timer.timer(with_jitter(backoff)).await;
                backoff = next_backoff(backoff);
            }
        }));
    }
}

struct PreparedNotification {
    title: String,
    body: String,
    channel_id: Option<String>,
    clan_id: Option<String>,
    link: Option<String>,
    topic_reply: Option<NotificationTopicReply>,
    icon_url: Option<String>,
}

fn note_dm_unread(cx: &mut App, user_id: &str, n: &GotifyNotification) {
    if n.sender_id == user_id {
        return;
    }
    let Ok(channel_id) = n.channel_id.parse::<i64>() else {
        return;
    };
    if channel_id == 0 {
        return;
    }
    let ts = chrono::DateTime::parse_from_rfc3339(&n.date)
        .map(|date| date.timestamp())
        .unwrap_or(0);
    if let Some(badge) = crate::badge::BadgeService::try_global(cx) {
        badge.update(cx, |badge, cx| {
            badge.note_dm_notification(ChannelId(channel_id), ts, cx);
        });
    }
}

const SENDER_AVATAR_PX: u32 = 64;

fn sender_avatar_url(cx: &App, image: &str) -> Option<String> {
    if !SHOW_SENDER_AVATAR || !image.starts_with("https://") {
        return None;
    }
    let proxied = AppConfig::try_global(cx)
        .map(|config| config.imgproxy_url(image, SENDER_AVATAR_PX, SENDER_AVATAR_PX, "fill"))
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| image.to_owned());
    Some(proxied)
}

/// Apply the suppressors and gather notification content on the foreground;
/// returns `None` when the notification should not be shown.
fn prepare(cx: &App, user_id: &str, n: &GotifyNotification) -> Option<PreparedNotification> {
    tracing::debug!(
        channel_id = %n.channel_id,
        sender_id = %n.sender_id,
        "gotify: received notification"
    );
    if n.sender_id == user_id {
        tracing::debug!("gotify: suppressed — own message");
        return None;
    }
    if is_do_not_disturb(cx, user_id) {
        tracing::debug!("gotify: suppressed — do not disturb");
        return None;
    }
    if is_viewing_while_focused(cx, &n.channel_id) {
        tracing::debug!("gotify: suppressed — viewing this channel while focused");
        return None;
    }
    let hide_content = Settings::try_global(cx)
        .map(|s| s.read(cx).notifications_hide_content)
        .unwrap_or(false);
    let (channel_id, clan_id) = route_ids(cx, n.effective_channel_id());
    Some(PreparedNotification {
        title: n.title.clone(),
        body: if hide_content {
            String::new()
        } else {
            n.message.clone()
        },
        channel_id,
        clan_id,
        link: Some(n.extras.link.clone()).filter(|l| !l.is_empty()),
        topic_reply: topic_reply(&n.extras),
        icon_url: sender_avatar_url(cx, &n.image),
    })
}

fn topic_reply(extras: &GotifyExtras) -> Option<NotificationTopicReply> {
    let present = |id: &str| !id.is_empty() && id != "0";
    (present(&extras.topic_id) && present(&extras.message_id)).then(|| NotificationTopicReply {
        topic_id: extras.topic_id.clone(),
        message_id: extras.message_id.clone(),
    })
}

fn show_prepared(cx: &App, p: PreparedNotification, icon_path: Option<String>) {
    let Some(platform) = PlatformStore::try_global(cx) else {
        tracing::warn!("gotify: no PlatformStore; cannot show notification");
        return;
    };
    tracing::debug!("gotify: delivering desktop notification");
    platform.read(cx).show_notification(DesktopNotification {
        title: p.title,
        body: p.body,
        channel_id: p.channel_id,
        clan_id: p.clan_id,
        link: p.link,
        topic_reply: p.topic_reply,
        icon_path,
    });
}

fn is_do_not_disturb(cx: &App, user_id: &str) -> bool {
    let Ok(uid) = user_id.parse::<i64>() else {
        return false;
    };
    PresenceStore::try_global(cx)
        .and_then(|p| {
            p.read(cx)
                .user_status(crate::ids::UserId(uid))
                .map(str::to_owned)
        })
        .as_deref()
        == Some(USER_STATUS_DO_NOT_DISTURB)
}

fn is_viewing_while_focused(cx: &App, channel_id: &str) -> bool {
    if cx.active_window().is_none() {
        return false;
    }
    let Ok(raw) = channel_id.parse::<i64>() else {
        return false;
    };
    MessagesStore::global(cx).read(cx).active_channel_id() == Some(ChannelId(raw))
}

/// Resolve the clan that owns a channel so a notification click can route to it;
/// a channel absent from the clan cache (e.g. a DM) yields `clan_id = None`.
fn route_ids(cx: &App, channel_id: &str) -> (Option<String>, Option<String>) {
    let Ok(raw) = channel_id.parse::<i64>() else {
        return (None, None);
    };
    let cid = ChannelId(raw);
    let clan_id = ClanList::global(cx)
        .read(cx)
        .clans
        .iter()
        .map(|clan| clan.id)
        .find(|clan_id| {
            ChannelList::global(cx)
                .read(cx)
                .channel(*clan_id, cid)
                .is_some()
        });
    (
        Some(channel_id.to_string()),
        clan_id.filter(|c| !c.is_zero()).map(|c| c.to_string()),
    )
}
