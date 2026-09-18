use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::MezonClient;

use crate::config::AppConfig;
use crate::ids::{ChannelId, ClanId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteDetails {
    pub clan_id: ClanId,
    pub channel_id: ChannelId,
    pub clan_name: String,
    pub clan_logo: String,
    pub member_count: i32,
    pub user_joined: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InviteState {
    #[default]
    Idle,
    Loading,
    Loaded(InviteDetails),
    Failed,
}

#[derive(Debug, Clone)]
pub enum InviteEvent {
    Loaded(InviteDetails),
    LoadFailed,
}

pub struct InviteStore {
    invite_id: String,
    state: InviteState,
    client: Arc<MezonClient>,
    _fetch: Option<Task<()>>,
}

struct GlobalInviteStore(Entity<InviteStore>);
impl Global for GlobalInviteStore {}

impl EventEmitter<InviteEvent> for InviteStore {}

impl InviteStore {
    pub fn init(client: Arc<MezonClient>, cx: &mut App) -> Entity<Self> {
        let store = cx.new(|_| Self {
            invite_id: String::new(),
            state: InviteState::Idle,
            client,
            _fetch: None,
        });
        cx.set_global(GlobalInviteStore(store.clone()));
        store
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalInviteStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalInviteStore>().map(|g| g.0.clone())
    }

    pub fn state(&self) -> &InviteState {
        &self.state
    }

    pub fn ensure_invite(&mut self, invite_id: String, cx: &mut Context<Self>) {
        if self.invite_id == invite_id
            && matches!(self.state, InviteState::Loading | InviteState::Loaded(_))
        {
            return;
        }
        self.invite_id = invite_id.clone();
        self.state = InviteState::Loading;
        cx.notify();

        let Some(gw_base) = AppConfig::try_global(cx).map(|config| config.client_base_url()) else {
            self.state = InviteState::Failed;
            cx.emit(InviteEvent::LoadFailed);
            cx.notify();
            return;
        };
        let client = self.client.clone();
        self._fetch = Some(cx.spawn(async move |this, cx| {
            let result = client.get_link_invite(&gw_base, &invite_id).await;
            let _ = this.update(cx, |this, cx| {
                if this.invite_id != invite_id {
                    return;
                }
                match result {
                    Ok(res) => {
                        let details = InviteDetails {
                            clan_id: ClanId(res.clan_id),
                            channel_id: ChannelId(res.channel_id),
                            clan_name: res.clan_name,
                            clan_logo: res.clan_logo,
                            member_count: res.member_count,
                            user_joined: res.user_joined,
                        };
                        this.state = InviteState::Loaded(details.clone());
                        cx.emit(InviteEvent::Loaded(details));
                    }
                    Err(error) => {
                        tracing::warn!("invite lookup failed: {error}");
                        this.state = InviteState::Failed;
                        cx.emit(InviteEvent::LoadFailed);
                    }
                }
                cx.notify();
            });
        }));
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.invite_id.clear();
        self.state = InviteState::Idle;
        self._fetch = None;
        cx.notify();
    }
}
