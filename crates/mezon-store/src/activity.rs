use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus};
use mezon_proto::api;

use crate::Freshness;
use crate::ids::UserId;

/// A user's current rich-presence activity, mirroring proto `UserActivity` and React `IActivity`.
/// `activity_type` groups the activity (React `ActivitiesType`): `1` = coding/work (Visual Studio
/// Code), `2` = music/live (Spotify), `3` = gaming/play (League of Legends).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserActivity {
    pub user_id: UserId,
    pub activity_type: i32,
    pub activity_name: String,
    pub activity_description: String,
}

/// Activity kinds used by the Friends activity sidebar, matching React's `activity_type` literals.
pub const ACTIVITY_TYPE_WORK: i32 = 1;
pub const ACTIVITY_TYPE_LIVE: i32 = 2;
pub const ACTIVITY_TYPE_PLAY: i32 = 3;

#[derive(Debug, Clone)]
pub enum ActivityEvent {
    Changed,
}

fn activity_from_api(a: api::UserActivity) -> UserActivity {
    UserActivity {
        user_id: UserId(a.user_id),
        activity_type: a.activity_type,
        activity_name: a.activity_name,
        activity_description: a.activity_description,
    }
}

struct GlobalActivityStore(Entity<ActivityStore>);
impl Global for GlobalActivityStore {}

/// Holds the list of user activities fetched from `ListActivity`. Mirrors React's `activitiesSlice`
/// (fetched by the `listActivities` thunk, cached for the session); the Friends sidebar filters it
/// by the current friend/DM user set and groups by [`UserActivity::activity_type`].
pub struct ActivityStore {
    activities: Vec<UserActivity>,
    loading: bool,
    freshness: Freshness,
    reset_generation: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

impl EventEmitter<ActivityEvent> for ActivityStore {}

impl ActivityStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalActivityStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalActivityStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalActivityStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            activities: Vec::new(),
            loading: false,
            freshness: Freshness::new(),
            reset_generation: 0,
            api,
            _conn_watch: conn_watch,
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.activities.clear();
        self.loading = false;
        self.freshness.mark_stale();
        cx.emit(ActivityEvent::Changed);
        cx.notify();
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

    pub fn activities(&self) -> &[UserActivity] {
        &self.activities
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx);
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.loading && !self.freshness.is_fresh(crate::CACHE_TTL) {
            self.fetch(cx);
        }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.list_activity().await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(list) => {
                        this.activities =
                            list.activities.into_iter().map(activity_from_api).collect();
                        this.freshness.mark_fetched();
                        cx.emit(ActivityEvent::Changed);
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_activity failed: {e}"),
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_from_api_maps_fields() {
        let a = activity_from_api(api::UserActivity {
            user_id: 42,
            activity_name: "Visual Studio Code".into(),
            activity_type: ACTIVITY_TYPE_WORK,
            activity_description: "Editing friends_page.rs".into(),
            start_time_seconds: 0,
            end_time_seconds: 0,
            application_id: 0,
            status: 0,
        });
        assert_eq!(a.user_id, UserId(42));
        assert_eq!(a.activity_type, ACTIVITY_TYPE_WORK);
        assert_eq!(a.activity_name, "Visual Studio Code");
        assert_eq!(a.activity_description, "Editing friends_page.rs");
    }
}
