use std::sync::Arc;

use anyhow::Result;
use gpui::{App, AppContext, BackgroundExecutor, Context, Entity, Global};
use mezon_client::{AppApi, MezonClient, QrLoginId, Session, keychain};

use crate::AuthState;

pub fn token_from_oauth_callback_url(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned())
}

pub struct LoginStore {
    client: Arc<MezonClient>,
    api: Arc<AppApi>,
    auth_state: Entity<AuthState>,
}

struct GlobalLoginStore(Entity<LoginStore>);
impl Global for GlobalLoginStore {}

impl LoginStore {
    pub fn init(
        client: Arc<MezonClient>,
        api: Arc<AppApi>,
        auth_state: Entity<AuthState>,
        cx: &mut App,
    ) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            client,
            api,
            auth_state,
        });
        cx.set_global(GlobalLoginStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalLoginStore>().0.clone()
    }

    pub fn client(&self) -> Arc<MezonClient> {
        self.client.clone()
    }

    pub async fn request_otp(&self, email: &str) -> Result<String> {
        self.client.request_otp(email).await
    }

    pub async fn confirm_otp(&self, req_id: &str, otp_code: &str) -> Result<Session> {
        self.client.confirm_otp(req_id, otp_code).await
    }

    pub async fn authenticate_email(&self, email: &str, password: &str) -> Result<Session> {
        self.client.authenticate_email(email, password).await
    }

    pub async fn confirm_oauth_callback(&self, token: &str) -> Result<Session> {
        self.client.authenticate_mezon(token).await
    }

    pub async fn request_sms_otp(&self, phone: &str) -> Result<String> {
        self.client.request_sms_otp(phone).await
    }

    pub async fn confirm_otp_sms(&self, req_id: &str, otp_code: &str) -> Result<Session> {
        self.client.confirm_otp_sms(req_id, otp_code).await
    }

    pub async fn create_qr_login(&self) -> Result<QrLoginId> {
        self.client.create_qr_login().await
    }

    pub async fn confirm_qr_login(&self, login_id: &str, is_remember: bool) -> Result<Session> {
        self.client.confirm_qr_login(login_id, is_remember).await
    }

    pub fn persist_session(session: &Session) -> Result<()> {
        keychain::save_session(session)
    }

    pub fn logout(&self, cx: &mut Context<Self>) {
        let credentials = cx.read_entity(&self.auth_state, |state, _| session_credentials(state));

        spawn_session_logout(self.api.clone(), credentials, cx.background_executor());

        self.auth_state.update(cx, |state, cx| {
            *state = AuthState::NotAuthenticated;
            cx.notify();
        });

        Self::reset_all_user_stores(cx);
    }

    pub fn reset_all_user_stores(cx: &mut App) {
        if let Some(e) = crate::account::AccountStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::presence::PresenceStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::clan_members::ClanMembersStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::channel_members::ChannelMembersStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::group_members::GroupMembersStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::roles::RolesStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::sticker::StickerStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::emoji::EmojiStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::pinned::PinnedMessagesStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::badge::BadgeService::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::users_by_user::UsersByUserStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::direct::DirectMessageStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::friend::FriendStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::activity::ActivityStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::messages::MessagesStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::inbox::InboxStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::topic_badges::TopicBadgeStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::topics::TopicsStore::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::clan::ClanList::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::channel::ChannelList::try_global(cx) {
            e.update(cx, |s, cx| s.reset(cx));
        }
        if let Some(e) = crate::voice::VoiceStore::try_global(cx) {
            e.update(cx, |s, cx| s.logout_teardown(cx));
        }
    }
}

pub(crate) fn session_credentials(state: &AuthState) -> Option<(String, String)> {
    match state {
        AuthState::Authenticated(s) | AuthState::Connecting(s) => {
            Some((s.token.clone(), s.refresh_token.clone()))
        }
        _ => None,
    }
}

pub(crate) fn spawn_session_logout(
    api: Arc<AppApi>,
    credentials: Option<(String, String)>,
    background: &BackgroundExecutor,
) {
    background
        .spawn(async move {
            if let Some((token, refresh_token)) = credentials
                && let Err(e) = api.session_logout(&token, &refresh_token).await
            {
                tracing::warn!("session_logout request failed (ignored): {e}");
            }
            if let Err(e) = keychain::clear_session() {
                tracing::warn!("Failed to clear keychain session: {e}");
            }
            crate::account::clear_cached_account();
        })
        .detach();
}

#[cfg(test)]
mod oauth_url_tests {
    use super::token_from_oauth_callback_url;

    #[test]
    fn parses_token_from_callback_url() {
        let url = "mezonapp://callback?token=abc.def.ghi";
        assert_eq!(
            token_from_oauth_callback_url(url).as_deref(),
            Some("abc.def.ghi")
        );
    }

    #[test]
    fn decodes_percent_encoded_token() {
        let url = "mezonapp://callback?token=abc%2Bdef%3D";
        assert_eq!(
            token_from_oauth_callback_url(url).as_deref(),
            Some("abc+def=")
        );
    }

    #[test]
    fn missing_token_returns_none() {
        assert!(token_from_oauth_callback_url("mezonapp://callback").is_none());
    }
}
