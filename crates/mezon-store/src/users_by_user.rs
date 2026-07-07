use crate::ids::UserId;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};

use crate::Freshness;
use crate::clan_members::{User, user_from_api};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

#[derive(Debug, Clone)]
pub enum UsersByUserEvent {
    Changed,
}

pub struct UsersByUserStore {
    by_id: HashMap<UserId, User>,
    loading: bool,
    freshness: Freshness,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalUsersByUserStore(Entity<UsersByUserStore>);
impl Global for GlobalUsersByUserStore {}

impl EventEmitter<UsersByUserEvent> for UsersByUserStore {}

impl UsersByUserStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalUsersByUserStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalUsersByUserStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalUsersByUserStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.by_id.clear();
        self.loading = false;
        self.freshness.mark_stale();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            by_id: HashMap::new(),
            loading: false,
            freshness: Freshness::new(),
            api,
            _conn_watch: conn_watch,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::AddClanUser, &entity, |this, event, cx| {
                this.handle_event(event, cx)
            });
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
                    if this.update(cx, |this, _| this.invalidate()).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn user(&self, user_id: UserId) -> Option<&User> {
        self.by_id.get(&user_id)
    }

    pub fn users(&self) -> impl Iterator<Item = &User> + '_ {
        self.by_id.values()
    }

    pub fn count(&self) -> usize {
        self.by_id.len()
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.freshness.is_fresh(crate::CACHE_TTL) && !self.loading {
            self.fetch(cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx);
    }

    fn invalidate(&mut self) {
        self.freshness.mark_stale();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_user_clans_by_user().await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(users) => {
                        this.freshness.mark_fetched();
                        for user in users {
                            if let Some(user) = user_from_api(user) {
                                this.by_id.insert(user.id, user);
                            }
                        }
                        tracing::info!("UsersByUserStore: loaded {} users", this.by_id.len());
                        cx.emit(UsersByUserEvent::Changed);
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_user_clans_by_user failed: {e}"),
                }
            });
        })
        .detach();
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        if let RealtimeEvent::AddClanUser(e) = event {
            let Some(redis) = e.user.as_ref() else {
                return;
            };
            if redis.user_id == 0 {
                return;
            }
            let user = User {
                id: UserId(redis.user_id),
                username: redis.username.clone(),
                display_name: redis.display_name.clone(),
                avatar_url: redis.avatar.clone(),
                about_me: String::new(),
                create_time_seconds: redis.create_time_second,
            };
            self.by_id.insert(user.id, user);
            cx.emit(UsersByUserEvent::Changed);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_proto::api;

    #[test]
    fn user_from_api_maps_fields() {
        let user = user_from_api(api::User {
            id: 5,
            username: "carol".into(),
            display_name: "Carol".into(),
            avatar_url: "c.png".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(user.id, UserId(5));
        assert_eq!(user.username, "carol");
        assert_eq!(user.display_name, "Carol");
        assert_eq!(user.avatar_url, "c.png");
    }

    #[test]
    fn user_from_api_skips_zero_id() {
        assert!(user_from_api(api::User::default()).is_none());
    }
}
