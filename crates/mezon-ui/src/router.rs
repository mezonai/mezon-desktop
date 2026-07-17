use gpui::{App, AppContext, Entity, Global};
use mezon_store::{ChannelId, ClanId};

use crate::clan::settings::ClanSettingsPage;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Chat,
    Direct,
    Friends,
    DirectMessage {
        direct_id: ChannelId,
        message_type: String,
    },
    Channel {
        clan_id: ClanId,
        channel_id: ChannelId,
    },
    Thread {
        clan_id: ClanId,
        channel_id: ChannelId,
        thread_id: ChannelId,
    },
    Canvas {
        clan_id: ClanId,
        channel_id: ChannelId,
        canvas_id: ChannelId,
    },
    AddFriend {
        username: String,
    },
    Invite {
        invite_id: String,
    },
    SettingsAccount,
    SettingsProfile,
    SettingsDevices,
    SettingsAppearance,
    SettingsActivity,
    SettingsNotifications,
    SettingsLanguage,
    SettingsVoice,
    SettingsAdvanced,
    ClanSettings {
        clan_id: ClanId,
        page: ClanSettingsPage,
    },
    NotFound {
        path: String,
    },
}

impl Route {
    pub fn to_path(&self) -> String {
        match self {
            Route::Chat => "/chat".to_string(),
            Route::Direct => "/chat/direct".to_string(),
            Route::Friends => "/chat/direct/friends".to_string(),
            Route::DirectMessage {
                direct_id,
                message_type,
            } => format!("/chat/direct/message/{direct_id}/{message_type}"),
            Route::Channel {
                clan_id,
                channel_id,
            } => format!("/chat/clans/{clan_id}/channels/{channel_id}"),
            Route::Thread {
                clan_id,
                channel_id,
                thread_id,
            } => format!("/chat/clans/{clan_id}/channels/{channel_id}/threads/{thread_id}"),
            Route::Canvas {
                clan_id,
                channel_id,
                canvas_id,
            } => format!("/chat/clans/{clan_id}/channels/{channel_id}/canvas/{canvas_id}"),
            Route::AddFriend { username } => format!("/chat/{username}"),
            Route::Invite { invite_id } => format!("/invite/{invite_id}"),
            Route::SettingsAccount => "/settings/account".to_string(),
            Route::SettingsProfile => "/settings/profile".to_string(),
            Route::SettingsDevices => "/settings/devices".to_string(),
            Route::SettingsAppearance => "/settings/appearance".to_string(),
            Route::SettingsActivity => "/settings/activity".to_string(),
            Route::SettingsNotifications => "/settings/notifications".to_string(),
            Route::SettingsLanguage => "/settings/language".to_string(),
            Route::SettingsVoice => "/settings/voice".to_string(),
            Route::SettingsAdvanced => "/settings/advanced".to_string(),
            Route::ClanSettings { clan_id, page } => {
                format!("/chat/clans/{}/settings/{}", clan_id.get(), page.slug())
            }
            Route::NotFound { path } => path.clone(),
        }
    }

    pub fn from_path(path: &str) -> Route {
        let normalized = normalize_path(path);
        let segments = normalized
            .trim_start_matches('/')
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();

        Self::route_from_segments(&segments).unwrap_or(Route::NotFound { path: normalized })
    }

    /// Map URL segments to a [`Route`]. Snowflake ids are parsed with `.parse().ok()?`, so a
    /// malformed (non-numeric) id in an untrusted deep link yields `None` → `NotFound` — never a
    /// panic.
    fn route_from_segments(segments: &[&str]) -> Option<Route> {
        Some(match *segments {
            ["chat"] => Route::Chat,
            ["chat", "direct"] => Route::Direct,
            ["chat", "direct", "friends"] => Route::Friends,
            ["chat", "direct", "message", direct_id, message_type] => Route::DirectMessage {
                direct_id: direct_id.parse().ok()?,
                message_type: message_type.to_string(),
            },
            ["chat", "clans", clan_id, "channels", channel_id] => Route::Channel {
                clan_id: clan_id.parse().ok()?,
                channel_id: channel_id.parse().ok()?,
            },
            [
                "chat",
                "clans",
                clan_id,
                "channels",
                channel_id,
                "threads",
                thread_id,
            ] => Route::Thread {
                clan_id: clan_id.parse().ok()?,
                channel_id: channel_id.parse().ok()?,
                thread_id: thread_id.parse().ok()?,
            },
            [
                "chat",
                "clans",
                clan_id,
                "channels",
                channel_id,
                "canvas",
                canvas_id,
            ] => Route::Canvas {
                clan_id: clan_id.parse().ok()?,
                channel_id: channel_id.parse().ok()?,
                canvas_id: canvas_id.parse().ok()?,
            },
            ["chat", username] if !matches!(username, "direct" | "clans") => Route::AddFriend {
                username: username.to_string(),
            },
            ["invite", invite_id] => Route::Invite {
                invite_id: invite_id.to_string(),
            },
            ["settings"] | ["settings", "account"] => Route::SettingsAccount,
            ["settings", "profile"] => Route::SettingsProfile,
            ["settings", "devices"] => Route::SettingsDevices,
            ["settings", "appearance"] => Route::SettingsAppearance,
            ["settings", "activity"] => Route::SettingsActivity,
            ["settings", "notifications"] => Route::SettingsNotifications,
            ["settings", "language"] => Route::SettingsLanguage,
            ["settings", "voice"] => Route::SettingsVoice,
            ["settings", "advanced"] => Route::SettingsAdvanced,
            ["chat", "clans", clan_id, "settings"] => Route::ClanSettings {
                clan_id: clan_id.parse().ok()?,
                page: ClanSettingsPage::Overview,
            },
            ["chat", "clans", clan_id, "settings", page] => Route::ClanSettings {
                clan_id: clan_id.parse().ok()?,
                page: ClanSettingsPage::from_slug(page)?,
            },
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Router {
    current: Route,
    backward: VecDeque<Route>,
    forward: VecDeque<Route>,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    const MAX_HISTORY: usize = 64;

    pub fn new() -> Self {
        Self {
            current: Route::Direct,
            backward: VecDeque::new(),
            forward: VecDeque::new(),
        }
    }

    pub fn route(&self) -> Route {
        self.current.clone()
    }

    pub fn conversation_channel_id(&self) -> Option<ChannelId> {
        match &self.current {
            Route::Channel { channel_id, .. }
            | Route::Thread { channel_id, .. }
            | Route::DirectMessage {
                direct_id: channel_id,
                ..
            } => Some(*channel_id),
            _ => None,
        }
    }

    pub fn current_path(&self) -> String {
        self.current.to_path()
    }

    pub fn navigate(&mut self, route: Route) {
        if route == self.current {
            return;
        }
        self.backward
            .push_back(std::mem::replace(&mut self.current, route));
        self.forward.clear();
        while self.backward.len() > Self::MAX_HISTORY {
            self.backward.pop_front();
        }
    }

    pub fn replace(&mut self, route: Route) {
        self.current = route;
    }

    pub fn go_back(&mut self) {
        if let Some(prev) = self.backward.pop_back() {
            self.forward
                .push_back(std::mem::replace(&mut self.current, prev));
            while self.forward.len() > Self::MAX_HISTORY {
                self.forward.pop_front();
            }
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(next) = self.forward.pop_back() {
            self.backward
                .push_back(std::mem::replace(&mut self.current, next));
            while self.backward.len() > Self::MAX_HISTORY {
                self.backward.pop_front();
            }
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.backward.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

struct GlobalRouter(Entity<Router>);
impl Global for GlobalRouter {}

impl Router {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Router::new());
        cx.set_global(GlobalRouter(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalRouter>().0.clone()
    }
}

pub fn parse_link(url: &str) -> Option<Route> {
    let rest = url.trim().strip_prefix("mezonapp://")?;
    if rest.starts_with("callback") {
        return None;
    }
    Some(Route::from_path(&format!(
        "/{}",
        rest.trim_start_matches('/')
    )))
}

pub fn navigate(cx: &mut App, route: Route) {
    Router::global(cx).update(cx, |router, cx| {
        router.navigate(route);
        cx.notify();
    });
}

pub fn replace(cx: &mut App, route: Route) {
    Router::global(cx).update(cx, |router, cx| {
        router.replace(route);
        cx.notify();
    });
}

pub fn go_back(cx: &mut App) {
    Router::global(cx).update(cx, |router, cx| {
        router.go_back();
        cx.notify();
    });
}

pub fn go_forward(cx: &mut App) {
    Router::global(cx).update(cx, |router, cx| {
        router.go_forward();
        cx.notify();
    });
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/chat".to_string();
    }

    let without_trailing = trimmed.trim_end_matches('/');
    if without_trailing.starts_with('/') {
        without_trailing.to_string()
    } else {
        format!("/{without_trailing}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_path_channel_route() {
        let route = Route::from_path("/chat/clans/1/channels/42");
        assert_eq!(
            route,
            Route::Channel {
                clan_id: ClanId(1),
                channel_id: ChannelId(42),
            }
        );
    }

    #[test]
    fn from_path_settings_profile() {
        assert_eq!(
            Route::from_path("/settings/profile"),
            Route::SettingsProfile
        );
    }

    #[test]
    fn from_path_unknown_becomes_not_found() {
        assert_eq!(
            Route::from_path("/nope/xyz"),
            Route::NotFound {
                path: "/nope/xyz".into()
            }
        );
    }

    #[test]
    fn parse_link_strips_mezonapp_scheme() {
        assert_eq!(
            parse_link("mezonapp://chat/clans/9/channels/3"),
            Some(Route::Channel {
                clan_id: ClanId(9),
                channel_id: ChannelId(3),
            })
        );
    }

    #[test]
    fn parse_link_callback_is_none() {
        assert_eq!(parse_link("mezonapp://callback?token=secret"), None);
    }

    #[test]
    fn to_path_roundtrip_channel() {
        let route = Route::Channel {
            clan_id: ClanId(7),
            channel_id: ChannelId(99),
        };
        assert_eq!(route.to_path(), "/chat/clans/7/channels/99");
        assert_eq!(Route::from_path(&route.to_path()), route);
    }

    #[test]
    fn default_route_is_dm_view() {
        assert_eq!(Router::new().route(), Route::Direct);
    }

    #[test]
    fn from_path_friends() {
        assert_eq!(Route::from_path("/chat/direct/friends"), Route::Friends);
    }

    #[test]
    fn to_path_roundtrip_friends() {
        assert_eq!(Route::Friends.to_path(), "/chat/direct/friends");
        assert_eq!(Route::from_path(&Route::Friends.to_path()), Route::Friends);
    }

    #[test]
    fn from_path_thread() {
        let route = Route::from_path("/chat/clans/1/channels/2/threads/3");
        assert_eq!(
            route,
            Route::Thread {
                clan_id: ClanId(1),
                channel_id: ChannelId(2),
                thread_id: ChannelId(3),
            }
        );
    }

    #[test]
    fn to_path_roundtrip_thread() {
        let route = Route::Thread {
            clan_id: ClanId(1),
            channel_id: ChannelId(2),
            thread_id: ChannelId(3),
        };
        assert_eq!(route.to_path(), "/chat/clans/1/channels/2/threads/3");
        assert_eq!(Route::from_path(&route.to_path()), route);
    }

    #[test]
    fn from_path_canvas() {
        let route = Route::from_path("/chat/clans/1/channels/2/canvas/4");
        assert_eq!(
            route,
            Route::Canvas {
                clan_id: ClanId(1),
                channel_id: ChannelId(2),
                canvas_id: ChannelId(4),
            }
        );
    }

    #[test]
    fn to_path_roundtrip_canvas() {
        let route = Route::Canvas {
            clan_id: ClanId(1),
            channel_id: ChannelId(2),
            canvas_id: ChannelId(4),
        };
        assert_eq!(route.to_path(), "/chat/clans/1/channels/2/canvas/4");
        assert_eq!(Route::from_path(&route.to_path()), route);
    }

    #[test]
    fn from_path_add_friend() {
        let route = Route::from_path("/chat/alice");
        assert_eq!(
            route,
            Route::AddFriend {
                username: "alice".into(),
            }
        );
    }

    #[test]
    fn to_path_roundtrip_add_friend() {
        let route = Route::AddFriend {
            username: "alice".into(),
        };
        assert_eq!(route.to_path(), "/chat/alice");
        assert_eq!(Route::from_path(&route.to_path()), route);
    }

    #[test]
    fn from_path_clan_settings_overview() {
        let route = Route::from_path("/chat/clans/42/settings/overview");
        assert_eq!(
            route,
            Route::ClanSettings {
                clan_id: ClanId(42),
                page: ClanSettingsPage::Overview,
            }
        );
    }

    #[test]
    fn to_path_roundtrip_clan_settings() {
        let route = Route::ClanSettings {
            clan_id: ClanId(7),
            page: ClanSettingsPage::Roles,
        };
        assert_eq!(route.to_path(), "/chat/clans/7/settings/roles");
        assert_eq!(Route::from_path(&route.to_path()), route);
    }

    #[test]
    fn from_path_add_friend_skips_reserved_segments() {
        assert_eq!(Route::from_path("/chat/direct"), Route::Direct);
        assert_eq!(
            Route::from_path("/chat/clans"),
            Route::NotFound {
                path: "/chat/clans".into(),
            }
        );
    }

    #[test]
    fn from_path_invite() {
        let route = Route::from_path("/invite/abc123");
        assert_eq!(
            route,
            Route::Invite {
                invite_id: "abc123".into(),
            }
        );
    }

    #[test]
    fn to_path_roundtrip_invite() {
        let route = Route::Invite {
            invite_id: "abc123".into(),
        };
        assert_eq!(route.to_path(), "/invite/abc123");
        assert_eq!(Route::from_path(&route.to_path()), route);
    }

    #[test]
    fn parse_link_friends() {
        assert_eq!(
            parse_link("mezonapp://chat/direct/friends"),
            Some(Route::Friends)
        );
    }

    #[test]
    fn parse_link_invite() {
        assert_eq!(
            parse_link("mezonapp://invite/abc123"),
            Some(Route::Invite {
                invite_id: "abc123".into(),
            })
        );
    }

    #[test]
    fn go_back_at_boundary_is_noop() {
        let mut router = Router::new();
        router.go_back();
        assert_eq!(router.route(), Route::Direct);
    }

    #[test]
    fn go_forward_at_boundary_is_noop() {
        let mut router = Router::new();
        router.go_forward();
        assert_eq!(router.route(), Route::Direct);
    }

    #[test]
    fn navigate_then_back_then_forward() {
        let mut router = Router::new();
        router.navigate(Route::Chat);
        router.navigate(Route::Friends);
        assert_eq!(router.route(), Route::Friends);
        router.go_back();
        assert_eq!(router.route(), Route::Chat);
        router.go_back();
        assert_eq!(router.route(), Route::Direct);
        router.go_forward();
        assert_eq!(router.route(), Route::Chat);
    }

    #[test]
    fn normalize_path_removes_trailing_slash() {
        assert_eq!(Route::from_path("/chat/direct/"), Route::Direct);
    }

    #[test]
    fn normalize_path_empty_becomes_chat() {
        assert_eq!(Route::from_path(""), Route::Chat);
    }
}
