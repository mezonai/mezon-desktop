use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::Freshness;
use crate::ids::UserId;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

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
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::ListActivity, &entity, |this, event, cx| {
                this.handle_list_activity(event, cx);
            });
        });
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

    fn handle_list_activity(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::Unhandled(realtime::envelope::Message::ListActivity(list)) = event
        else {
            return;
        };
        let next = list
            .acts
            .iter()
            .cloned()
            .map(activity_from_api)
            .collect::<Vec<_>>();
        if next == self.activities {
            return;
        }
        self.activities = next;
        cx.emit(ActivityEvent::Changed);
        cx.notify();
    }

    fn apply_activities(&mut self, next: Vec<UserActivity>, cx: &mut Context<Self>) {
        self.freshness.mark_fetched();
        if next == self.activities {
            return;
        }
        self.activities = next;
        cx.emit(ActivityEvent::Changed);
        cx.notify();
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
                    Ok(list) => this.apply_activities(
                        list.activities.into_iter().map(activity_from_api).collect(),
                        cx,
                    ),
                    Err(e) => tracing::error!("list_activity failed: {e}"),
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn init_store(cx: &mut App) -> Entity<ActivityStore> {
        let api = Arc::new(mezon_client::AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        cx.new(|cx| ActivityStore::new(api, cx))
    }

    fn work(user_id: i64, name: &str) -> UserActivity {
        UserActivity {
            user_id: UserId(user_id),
            activity_type: ACTIVITY_TYPE_WORK,
            activity_name: name.into(),
            activity_description: String::new(),
        }
    }

    #[gpui::test]
    fn an_identical_refetch_emits_nothing(cx: &mut gpui::TestAppContext) {
        let emitted = Arc::new(AtomicUsize::new(0));
        let (store, _sub) = cx.update(|cx| {
            let store = init_store(cx);
            let counter = emitted.clone();
            let sub = cx.subscribe(&store, move |_, _: &ActivityEvent, _| {
                counter.fetch_add(1, Ordering::SeqCst);
            });
            (store, sub)
        });

        for _ in 0..2 {
            cx.update(|cx| {
                store.update(cx, |store, cx| {
                    store.apply_activities(vec![work(1, "Visual Studio Code")], cx);
                });
            });
        }

        assert_eq!(emitted.load(Ordering::SeqCst), 1);
    }

    #[gpui::test]
    fn a_changed_refetch_emits_and_replaces(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.apply_activities(vec![work(1, "Visual Studio Code")], cx);
                store.apply_activities(vec![work(1, "Spotify")], cx);
                assert_eq!(store.activities(), &[work(1, "Spotify")]);
            });
        });
    }

    #[gpui::test]
    fn a_pushed_list_activity_replaces_the_roster(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.apply_activities(vec![work(1, "Visual Studio Code")], cx);
                let event = RealtimeEvent::Unhandled(realtime::envelope::Message::ListActivity(
                    realtime::ListActivity {
                        acts: vec![api::UserActivity {
                            user_id: 2,
                            activity_name: "Spotify".into(),
                            activity_type: ACTIVITY_TYPE_WORK,
                            ..Default::default()
                        }],
                    },
                ));
                store.handle_list_activity(&event, cx);
                assert_eq!(store.activities(), &[work(2, "Spotify")]);
            });
        });
    }

    #[gpui::test]
    fn a_pushed_roster_does_not_satisfy_the_fetch_cache(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, _cx| {
                assert!(
                    !store.freshness.is_fresh(crate::CACHE_TTL),
                    "precondition: a new store is stale"
                );
            });
            store.update(cx, |store, cx| {
                let event = RealtimeEvent::Unhandled(realtime::envelope::Message::ListActivity(
                    realtime::ListActivity {
                        acts: vec![api::UserActivity {
                            user_id: 2,
                            activity_name: "Spotify".into(),
                            activity_type: ACTIVITY_TYPE_WORK,
                            ..Default::default()
                        }],
                    },
                ));
                store.handle_list_activity(&event, cx);
                assert!(
                    !store.freshness.is_fresh(crate::CACHE_TTL),
                    "a socket push must not stand in for a fetch — the push may be partial"
                );
            });
        });
    }

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
