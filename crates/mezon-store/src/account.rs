use crate::ids::ClanId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::transport::ApiAccount;
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent, RegistrationPasswordError};
use serde::{Deserialize, Serialize};

use crate::Freshness;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

#[derive(Debug, Clone)]
pub struct UserAccount {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub phone_number: Option<String>,
    pub about_me: Option<String>,
    pub password_setted: bool,
    pub logo: Option<String>,
    pub status: String,
    pub user_status: String,
}

#[derive(Debug, Clone)]
pub struct LoggedDevice {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub ip: String,
    pub location: String,
    pub is_current: bool,
    pub last_active_seconds: u32,
    pub last_active_label: String,
}

#[derive(Debug, Clone)]
pub struct UserClanProfile {
    pub clan_id: ClanId,
    pub nick_name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AccountEvent {
    AccountLoaded,
    AccountLoadFailed,
    DevicesLoaded,
    DevicesLoadFailed,
    AccountSaved,
    AccountSaveFailed(String),
    PasswordSaved,
    PasswordSaveFailed(PasswordSaveError),
    UserAvatarUploaded(String),
    ClanAvatarUploaded(ClanId, String),
    UserAvatarUploadFailed(String),
    ClanAvatarUploadFailed(String),
    DirectMessageIconUploaded(String),
    DirectMessageIconUploadFailed(String),
    ClanProfileLoaded,
    ClanProfileLoadFailed(String),
    ClanProfileSaved,
    ClanProfileSaveFailed(String),
    NicknameDuplicateChecked(bool),
    StatusUpdated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordSaveError {
    IncorrectCurrentPassword,
    UpdateFailed,
    CreateFailed,
}

pub struct AccountStore {
    pub account: Option<UserAccount>,
    pub account_loading: bool,
    pub account_error: bool,
    pub devices: Vec<LoggedDevice>,
    pub devices_loading: bool,
    pub devices_error: Option<String>,
    pub clan_profile: Option<UserClanProfile>,
    pub clan_profile_loading: bool,
    pub nickname_duplicate: bool,
    account_freshness: Freshness,
    devices_freshness: Freshness,
    reset_generation: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalAccountStore(Entity<AccountStore>);
impl Global for GlobalAccountStore {}

impl EventEmitter<AccountEvent> for AccountStore {}

impl AccountStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalAccountStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            account: load_cached_account(),
            account_loading: false,
            account_error: false,
            devices: Vec::new(),
            devices_loading: false,
            devices_error: None,
            clan_profile: None,
            clan_profile_loading: false,
            nickname_duplicate: false,
            account_freshness: Freshness::new(),
            devices_freshness: Freshness::new(),
            reset_generation: 0,
            api,
            _conn_watch: conn_watch,
        }
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
                            this.account_freshness.mark_stale();
                            this.devices_freshness.mark_stale();
                            this.ensure_account(cx);
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

    /// Register realtime handlers with the central dispatcher (cf. `add_message_handler`).
    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(
                RealtimeKind::ClanProfileUpdated,
                &entity,
                |this, event, cx| this.handle_event(event, cx),
            );
            dispatch.on(
                RealtimeKind::UserProfileUpdated,
                &entity,
                |this, event, cx| this.handle_event(event, cx),
            );
            dispatch.on_lagged(&entity, |this, cx| {
                tracing::warn!("AccountStore realtime lagged — refetching account");
                if this.account.is_some() || this.account_loading {
                    this.fetch_account(cx);
                }
            });
        });
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        if let RealtimeEvent::UserProfileUpdated(e) = event {
            tracing::debug!(
                user_id = e.user_id,
                "user profile realtime received; refreshing signed-in account"
            );
            let mut updated_current_account = false;
            if let Some(account) = &mut self.account
                && account.user_id == e.user_id
            {
                // Match the React reducer: sparse empty display/avatar fields do
                // not erase existing values, while about_me is authoritative.
                if !e.display_name.is_empty() {
                    account.display_name = e.display_name.clone();
                }
                if !e.avatar.is_empty() {
                    account.avatar_url = Some(e.avatar.clone());
                }
                account.about_me = Some(e.about_me.clone());
                updated_current_account = true;
            }
            if updated_current_account {
                if let Some(account) = &self.account {
                    Self::spawn_persist_cache(account, cx);
                }
                cx.emit(AccountEvent::AccountLoaded);
                cx.notify();
                // The event has no `logo` field, so always refetch as well; this is
                // what makes direct-message icon changes propagate across clients.
                self.account_freshness.mark_stale();
                self.fetch_account(cx);
            }
            return;
        }
        if let RealtimeEvent::ClanProfileUpdated(e) = event {
            let clan_id = ClanId(e.clan_id);
            if self
                .clan_profile
                .as_ref()
                .is_some_and(|p| p.clan_id == clan_id)
            {
                if let Some(profile) = &mut self.clan_profile {
                    profile.nick_name = e.clan_nick.clone();
                    profile.avatar_url = (!e.clan_avatar.is_empty()).then(|| e.clan_avatar.clone());
                }
                cx.emit(AccountEvent::ClanProfileLoaded);
                cx.notify();
            }
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAccountStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAccountStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.account = None;
        self.account_loading = false;
        self.account_error = false;
        self.devices.clear();
        self.devices_loading = false;
        self.devices_error = None;
        self.clan_profile = None;
        self.clan_profile_loading = false;
        self.nickname_duplicate = false;
        self.account_freshness.mark_stale();
        self.devices_freshness.mark_stale();
        cx.notify();
    }

    fn spawn_persist_cache(account: &UserAccount, cx: &App) {
        let account = account.clone();
        cx.background_executor()
            .spawn(async move { save_cached_account(&account) })
            .detach();
    }

    pub fn ensure_account(&mut self, cx: &mut Context<Self>) {
        if !self.account_loading && !self.account_freshness.is_fresh(crate::CACHE_TTL) {
            self.fetch_account(cx);
        }
    }

    pub fn fetch_account(&mut self, cx: &mut Context<Self>) {
        if self.account_loading {
            return;
        }
        self.account_loading = true;
        self.account_error = false;
        cx.notify();

        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| match api.get_account().await {
            Ok(acct) => {
                let account = user_account_from_api(acct);
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation != generation {
                        return;
                    }
                    Self::spawn_persist_cache(&account, cx);
                    this.account = Some(account);
                    this.account_freshness.mark_fetched();
                    this.account_loading = false;
                    this.account_error = false;
                    cx.emit(AccountEvent::AccountLoaded);
                    cx.notify();
                });
            }
            Err(e) => {
                tracing::error!("Failed to load account: {e}");
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation != generation {
                        return;
                    }
                    this.account_loading = false;
                    this.account_error = true;
                    cx.emit(AccountEvent::AccountLoadFailed);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn ensure_devices(&mut self, cx: &mut Context<Self>) {
        if !self.devices_loading && !self.devices_freshness.is_fresh(crate::CACHE_TTL) {
            self.fetch_devices(cx);
        }
    }

    pub fn fetch_devices(&mut self, cx: &mut Context<Self>) {
        if self.devices_loading {
            return;
        }
        self.devices_loading = true;
        self.devices_error = None;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| match api.list_loged_device().await {
            Ok(devices) => {
                let mapped: Vec<LoggedDevice> =
                    devices.into_iter().map(logged_device_from_proto).collect();
                let _ = this.update(cx, |this, cx| {
                    this.devices = mapped;
                    this.devices_freshness.mark_fetched();
                    this.devices_loading = false;
                    this.devices_error = None;
                    cx.emit(AccountEvent::DevicesLoaded);
                    cx.notify();
                });
            }
            Err(e) => {
                tracing::error!("Failed to load devices: {e}");
                let _ = this.update(cx, |this, cx| {
                    this.devices_loading = false;
                    this.devices_error = Some("Failed to load devices".into());
                    cx.emit(AccountEvent::DevicesLoadFailed);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn save_account(
        &mut self,
        display_name: String,
        avatar_url: Option<String>,
        about_me: String,
        logo_url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            match api
                .update_account(
                    Some(&display_name),
                    Some(avatar_url.as_deref().unwrap_or_default()),
                    Some(&about_me),
                    logo_url.as_deref(),
                )
                .await
            {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(account) = &mut this.account {
                            account.display_name = display_name;
                            account.avatar_url = avatar_url;
                            account.about_me = Some(about_me);
                            account.logo = logo_url.filter(|url| !url.is_empty());
                        }
                        if let Some(account) = &this.account {
                            Self::spawn_persist_cache(account, cx);
                        }
                        cx.emit(AccountEvent::AccountSaved);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |_, cx| {
                        cx.emit(AccountEvent::AccountSaveFailed(e.to_string()));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub fn save_password(
        &mut self,
        email: String,
        password: String,
        old_password: String,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        let generation = self.reset_generation;
        let user_id = self.account.as_ref().map(|account| account.user_id);
        let changing = !old_password.is_empty();
        cx.spawn(async move |this, cx| {
            let result = api
                .registration_password(&email, &password, &old_password)
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(rotated) => {
                    if this.reset_generation != generation
                        || this.account.as_ref().map(|account| account.user_id) != user_id
                        || user_id.is_none_or(|id| rotated.user_id != 0 && rotated.user_id != id)
                    {
                        return;
                    }
                    let auth_state = crate::login::LoginStore::global(cx).read(cx).auth_state();
                    let applied = auth_state.update(cx, |state, cx| {
                        let session = match state {
                            crate::AuthState::Authenticated(session)
                            | crate::AuthState::Connecting(session) => session,
                            _ => return None,
                        };
                        if user_id.is_some_and(|id| session.user_id != id.to_string()) {
                            return None;
                        }
                        session.apply_refresh(
                            &rotated.token,
                            &rotated.refresh_token,
                            &rotated.session_id,
                            "",
                        );
                        cx.notify();
                        Some(session.clone())
                    });
                    let Some(session) = applied else {
                        return;
                    };
                    cx.background_executor()
                        .spawn(async move {
                            if let Err(error) = crate::login::LoginStore::persist_session(&session)
                            {
                                tracing::warn!(
                                    "Failed to persist password-rotated session: {error}"
                                );
                            }
                        })
                        .detach();
                    if let Some(account) = &mut this.account {
                        account.password_setted = true;
                    }
                    if let Some(account) = &this.account {
                        Self::spawn_persist_cache(account, cx);
                    }
                    cx.emit(AccountEvent::PasswordSaved);
                    cx.notify();
                }
                Err(error) => {
                    if this.reset_generation != generation
                        || this.account.as_ref().map(|account| account.user_id) != user_id
                    {
                        return;
                    }
                    let error = match error {
                        RegistrationPasswordError::IncorrectCurrentPassword if changing => {
                            PasswordSaveError::IncorrectCurrentPassword
                        }
                        _ if changing => PasswordSaveError::UpdateFailed,
                        _ => PasswordSaveError::CreateFailed,
                    };
                    cx.emit(AccountEvent::PasswordSaveFailed(error));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn set_status(
        &mut self,
        status: String,
        minutes: i32,
        until_turn_on: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(account) = &mut self.account else {
            return;
        };
        if account.status != status {
            account.status = status.clone();
            if let Some(account) = &self.account {
                Self::spawn_persist_cache(account, cx);
            }
            cx.emit(AccountEvent::StatusUpdated);
            cx.notify();
        }
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api.update_user_status(status, minutes, until_turn_on).await {
                tracing::warn!("Failed to update user status: {e}");
            }
        })
        .detach();
    }

    pub fn set_custom_status(
        &mut self,
        text: String,
        minutes: i32,
        until_turn_on: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(account) = &mut self.account {
            account.user_status = text.clone();
        } else {
            return;
        }
        if let Some(account) = &self.account {
            Self::spawn_persist_cache(account, cx);
        }
        cx.emit(AccountEvent::StatusUpdated);
        cx.notify();
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .update_user_custom_status(text, minutes, until_turn_on)
                .await
            {
                tracing::warn!("Failed to update custom status: {e}");
            }
        })
        .detach();
    }

    pub fn upload_user_avatar(&mut self, path: &Path, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let upload_api = api.clone();
            let result = cx
                .background_executor()
                .spawn(async move { upload_api.upload_avatar(&path).await })
                .await;
            match result {
                Ok(url) => {
                    let _ = this.update(cx, |_, cx| {
                        cx.emit(AccountEvent::UserAvatarUploaded(url));
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |_, cx| {
                        cx.emit(AccountEvent::UserAvatarUploadFailed(e.to_string()));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub fn upload_clan_avatar(&mut self, clan_id: ClanId, path: &Path, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.upload_avatar(&path).await })
                .await;
            let _ = this.update(cx, |_, cx| {
                match result {
                    Ok(url) => cx.emit(AccountEvent::ClanAvatarUploaded(clan_id, url)),
                    Err(error) => cx.emit(AccountEvent::ClanAvatarUploadFailed(error.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn upload_direct_message_icon(&mut self, path: &Path, cx: &mut Context<Self>) {
        if path
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(u64::MAX)
            > 1024 * 1024
        {
            cx.emit(AccountEvent::DirectMessageIconUploadFailed(
                "image exceeds the 1 MB direct-message icon limit".into(),
            ));
            cx.notify();
            return;
        }
        let api = self.api.clone();
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let upload_api = api.clone();
            let result = cx
                .background_executor()
                .spawn(async move { upload_api.upload_avatar(&path).await })
                .await;
            match result {
                Ok(url) => {
                    let _ = this.update(cx, |_, cx| {
                        // Keep the uploaded URL as a draft. Save Changes sends
                        // it with the rest of the account fields.
                        cx.emit(AccountEvent::DirectMessageIconUploaded(url));
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |_, cx| {
                        cx.emit(AccountEvent::DirectMessageIconUploadFailed(
                            error.to_string(),
                        ));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub fn logout(&mut self, token: String, refresh_token: String, cx: &mut Context<Self>) {
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            let _ = api.session_logout(&token, &refresh_token).await;
        })
        .detach();
        cx.notify();
    }

    pub fn remove_device(
        &mut self,
        token: String,
        refresh_token: String,
        device_id: String,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        let device_id_clone = device_id.clone();
        cx.spawn(async move |this, cx| {
            match api
                .logout_device(&token, &refresh_token, &device_id_clone)
                .await
            {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.devices.retain(|d| d.device_id != device_id_clone);
                        cx.emit(AccountEvent::DevicesLoaded);
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to remove device {device_id_clone}: {e}");
                    let _ = this.update(cx, |_, cx| {
                        cx.emit(AccountEvent::DevicesLoadFailed);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub fn fetch_clan_profile(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.clan_profile_loading = true;
        self.nickname_duplicate = false;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(
            async move |this, cx| match api.get_user_clan_profile(clan_id.get()).await {
                Ok(profile) => {
                    let _ = this.update(cx, |this, cx| {
                        this.clan_profile = Some(UserClanProfile {
                            clan_id,
                            nick_name: profile.nick_name,
                            avatar_url: (!profile.avatar.is_empty()).then_some(profile.avatar),
                        });
                        this.clan_profile_loading = false;
                        cx.emit(AccountEvent::ClanProfileLoaded);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.clan_profile.as_ref().map(|p| p.clan_id) != Some(clan_id) {
                            this.clan_profile = None;
                        }
                        this.clan_profile_loading = false;
                        cx.emit(AccountEvent::ClanProfileLoadFailed(e.to_string()));
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    pub fn save_clan_profile(
        &mut self,
        clan_id: ClanId,
        nick_name: String,
        avatar_url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            match api
                .update_user_clan_profile(
                    clan_id.get(),
                    &nick_name,
                    Some(avatar_url.as_deref().unwrap_or_default()),
                )
                .await
            {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.clan_profile = Some(UserClanProfile {
                            clan_id,
                            nick_name: nick_name.clone(),
                            avatar_url: avatar_url.clone(),
                        });
                        cx.emit(AccountEvent::ClanProfileSaved);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |_, cx| {
                        cx.emit(AccountEvent::ClanProfileSaveFailed(e.to_string()));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub fn check_clan_nickname(
        &mut self,
        clan_id: ClanId,
        nick_name: &str,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        let nick_name = nick_name.to_string();
        cx.spawn(async move |this, cx| {
            let is_dup = api
                .check_duplicate_clan_nickname(clan_id.get(), &nick_name)
                .await
                .unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.nickname_duplicate = is_dup;
                cx.emit(AccountEvent::NicknameDuplicateChecked(is_dup));
                cx.notify();
            });
        })
        .detach();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAccount {
    #[serde(default)]
    user_id: i64,
    username: String,
    display_name: String,
    #[serde(default)]
    avatar_url: Option<String>,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    user_status: String,
}

impl PersistedAccount {
    fn from_account(account: &UserAccount) -> Self {
        Self {
            user_id: account.user_id,
            username: account.username.clone(),
            display_name: account.display_name.clone(),
            avatar_url: account.avatar_url.clone(),
            logo: account.logo.clone(),
            status: account.status.clone(),
            user_status: account.user_status.clone(),
        }
    }

    fn into_account(self) -> UserAccount {
        UserAccount {
            user_id: self.user_id,
            username: self.username,
            display_name: self.display_name,
            email: None,
            avatar_url: self.avatar_url,
            phone_number: None,
            about_me: None,
            password_setted: false,
            logo: self.logo,
            status: self.status,
            user_status: self.user_status,
        }
    }
}

fn account_cache_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mezon")
        .join("account.json")
}

fn load_cached_account() -> Option<UserAccount> {
    let data = std::fs::read_to_string(account_cache_path()).ok()?;
    let cached: PersistedAccount = serde_json::from_str(&data).ok()?;
    Some(cached.into_account())
}

fn save_cached_account(account: &UserAccount) {
    let path = account_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = match serde_json::to_string_pretty(&PersistedAccount::from_account(account)) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Failed to serialize account cache: {e}");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &data) {
        tracing::warn!("Failed to write account cache: {e}");
        return;
    }
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

pub(crate) fn clear_cached_account() {
    let _ = std::fs::remove_file(account_cache_path());
}

fn user_account_from_api(acct: ApiAccount) -> UserAccount {
    let display = acct
        .display_name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| acct.username.clone());
    UserAccount {
        user_id: acct.user_id,
        username: acct.username,
        display_name: display,
        email: acct.email,
        avatar_url: acct.avatar_url,
        phone_number: acct.phone_number,
        about_me: acct.about_me,
        password_setted: acct.password_setted,
        logo: acct.logo,
        status: acct.status,
        user_status: acct.user_status,
    }
}

fn logged_device_from_proto(d: mezon_proto::api::LogedDevice) -> LoggedDevice {
    LoggedDevice {
        device_id: d.device_id,
        device_name: d.device_name,
        platform: d.platform,
        ip: d.ip,
        location: d.location,
        is_current: d.is_current,
        last_active_seconds: d.last_active_seconds,
        last_active_label: format_device_last_active(d.last_active_seconds),
    }
}

fn format_device_last_active(seconds: u32) -> String {
    if seconds == 0 {
        return "Unknown".to_string();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    let ago = now.saturating_sub(seconds);
    if ago < 60 {
        format!("{}s ago", ago)
    } else if ago < 3600 {
        format!("{}m ago", ago / 60)
    } else if ago < 86400 {
        format!("{}h ago", ago / 3600)
    } else {
        format!("{}d ago", ago / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_account_roundtrip_drops_sensitive_fields() {
        let account = UserAccount {
            user_id: 42,
            username: "alice".into(),
            display_name: "Alice".into(),
            email: Some("a@b.c".into()),
            avatar_url: Some("https://cdn/x.png".into()),
            phone_number: Some("+123".into()),
            about_me: Some("hi".into()),
            password_setted: true,
            logo: Some("https://cdn/logo.webp".into()),
            status: String::new(),
            user_status: String::new(),
        };
        let json = serde_json::to_string(&PersistedAccount::from_account(&account)).unwrap();
        let restored = serde_json::from_str::<PersistedAccount>(&json)
            .unwrap()
            .into_account();

        assert_eq!(restored.user_id, 42);
        assert_eq!(restored.username, "alice");
        assert_eq!(restored.display_name, "Alice");
        assert_eq!(restored.avatar_url.as_deref(), Some("https://cdn/x.png"));
        assert_eq!(restored.logo.as_deref(), Some("https://cdn/logo.webp"));
        assert_eq!(restored.email, None);
        assert_eq!(restored.phone_number, None);
        assert_eq!(restored.about_me, None);
        assert!(!restored.password_setted);
    }

    #[test]
    fn user_account_from_api_uses_username_when_display_empty() {
        let acct = user_account_from_api(ApiAccount {
            user_id: 1,
            username: "alice".into(),
            email: Some("a@b.c".into()),
            display_name: None,
            avatar_url: None,
            about_me: None,
            phone_number: None,
            password_setted: true,
            logo: None,
            status: String::new(),
            user_status: String::new(),
        });
        assert_eq!(acct.user_id, 1);
        assert_eq!(acct.display_name, "alice");
    }
}
