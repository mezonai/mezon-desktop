use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task};
use mezon_client::{AppApi, GotifyNotification};

use crate::channel::ChannelList;
use crate::clan::ClanList;
use crate::config::AppConfig;
use crate::ids::ChannelId;
use crate::messages::MessagesStore;
use crate::platform::{DesktopNotification, PlatformStore};
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
    _auth_sub: Subscription,
}

struct GlobalNotificationPushStore(Entity<NotificationPushStore>);
impl Global for GlobalNotificationPushStore {}

impl NotificationPushStore {
    pub fn init(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| {
            let auth_sub = cx.observe(&auth_state, Self::on_auth_changed);
            let mut this = Self {
                auth_state: auth_state.clone(),
                api,
                connection: None,
                connected_user: None,
                device_id: None,
                _auth_sub: auth_sub,
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
        let cached_device_id = self.device_id.clone().unwrap_or_default();
        self.connected_user = Some(user_id.clone());
        self.connection = Some(cx.spawn(async move |this, cx| {
            let token = match api
                .regist_fcm_device_token(&cached_device_id, "", DEVICE_PLATFORM)
                .await
            {
                Ok((token, device_id)) if !token.trim().is_empty() => {
                    let _ = this.update(cx, |store, _| {
                        if !device_id.is_empty() {
                            store.device_id = Some(device_id);
                        }
                    });
                    token.trim().to_string()
                }
                Ok(_) => {
                    tracing::warn!("gotify: empty notification token; not connecting");
                    return;
                }
                Err(e) => {
                    tracing::warn!("gotify: failed to register device token: {e}");
                    return;
                }
            };
            tracing::debug!("gotify: token acquired, opening stream");
            let mut rx = api.spawn_gotify_stream(ws_base, token);
            while let Some(notification) = rx.recv().await {
                let prepared = this
                    .update(cx, |_, cx| prepare(cx, &user_id, &notification))
                    .ok()
                    .flatten();
                let Some(prepared) = prepared else {
                    if this.update(cx, |_, _| {}).is_err() {
                        break;
                    }
                    continue;
                };
                let icon_path = if SHOW_SENDER_AVATAR && notification.image.starts_with("https://")
                {
                    api.download_notification_icon(&notification.image)
                        .await
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned())
                } else {
                    None
                };
                let delivered = this.update(cx, |_, cx| show_prepared(cx, prepared, icon_path));
                if delivered.is_err() {
                    break;
                }
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
    let (channel_id, clan_id) = route_ids(cx, &n.channel_id);
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
