use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_active_windows::{get_active_window, tracked_activity_from_window};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::Freshness;
use crate::Settings;
use crate::ids::UserId;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const ACTIVITY_POLL_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// A user's current rich-presence activity, mirroring proto `UserActivity` and React `IActivity`.
/// `activity_type` groups the activity (React `ActivitiesType`): `1` = coding (editors/dev tools),
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserActivity {
    pub user_id: UserId,
    pub activity_type: i32,
    pub activity_name: String,
    pub activity_description: String,
}

/// Activity kinds used by the Friends activity sidebar, matching React's `activity_type` literals.
pub use mezon_active_windows::{ACTIVITY_TYPE_LIVE, ACTIVITY_TYPE_PLAY, ACTIVITY_TYPE_WORK};

#[derive(Debug, Clone)]
pub enum ActivityEvent {
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedActivity {
    app_name: String,
    activity_type: i32,
}

fn activity_from_api(a: api::UserActivity) -> UserActivity {
    UserActivity {
        user_id: UserId(a.user_id),
        activity_type: a.activity_type,
        activity_name: a.activity_name,
        activity_description: a.activity_description,
    }
}

fn detect_tracked_activity(
    info: &mezon_active_windows::ActiveWindowInfo,
) -> Option<(String, String, i32)> {
    tracked_activity_from_window(info).map(|tracked| {
        (
            tracked.app_name,
            tracked.description,
            tracked.kind.as_type(),
        )
    })
}

fn clear_activity_request() -> api::CreateActivityRequest {
    api::CreateActivityRequest {
        activity_name: String::new(),
        activity_type: 0,
        status: 0,
        ..Default::default()
    }
}

fn publish_activity_request(
    app_name: &str,
    window_title: &str,
    activity_type: i32,
    start_time_seconds: u32,
) -> api::CreateActivityRequest {
    api::CreateActivityRequest {
        activity_name: app_name.to_string(),
        activity_type,
        activity_description: window_title.to_string(),
        start_time_seconds,
        application_id: 0,
        status: 1,
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
    publish_generation: u64,
    last_published: Option<PublishedActivity>,
    api: Arc<AppApi>,
    settings: Entity<Settings>,
    _conn_watch: Task<()>,
    _tracking_task: Task<()>,
}

impl EventEmitter<ActivityEvent> for ActivityStore {}

impl ActivityStore {
    pub fn init(api: Arc<AppApi>, settings: Entity<Settings>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, settings, cx));
        cx.set_global(GlobalActivityStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalActivityStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalActivityStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let tracking_task = Self::spawn_tracking_task(cx);
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::ListActivity, &entity, |this, event, cx| {
                this.handle_list_activity(event, cx);
            });
        });
        cx.observe(&settings, |this, settings, cx| {
            if settings.read(cx).activity_tracking {
                this.poll_active_window(cx);
            } else {
                this.clear_published_activity(cx);
            }
        })
        .detach();
        Self {
            activities: Vec::new(),
            loading: false,
            freshness: Freshness::new(),
            reset_generation: 0,
            publish_generation: 0,
            last_published: None,
            api,
            settings,
            _conn_watch: conn_watch,
            _tracking_task: tracking_task,
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.publish_generation = self.publish_generation.wrapping_add(1);
        self.clear_published_activity(cx);
        self.activities.clear();
        self.loading = false;
        self.freshness.mark_stale();
        cx.emit(ActivityEvent::Changed);
        cx.notify();
    }

    fn tracking_enabled(&self, cx: &App) -> bool {
        self.settings.read(cx).activity_tracking
    }

    fn is_connected(&self) -> bool {
        *self.api.status().borrow() == ConnectionStatus::Connected
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
                            this.poll_active_window(cx);
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

    fn spawn_tracking_task(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let should_poll = this
                    .update(cx, |this, cx| {
                        this.tracking_enabled(cx) && this.is_connected()
                    })
                    .unwrap_or(false);
                if should_poll {
                    let _ = this.update(cx, |this, cx| this.poll_active_window(cx));
                }
                cx.background_executor().timer(ACTIVITY_POLL_INTERVAL).await;
            }
        })
    }

    fn poll_active_window(&mut self, cx: &mut Context<Self>) {
        if !self.tracking_enabled(cx) || !self.is_connected() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { get_active_window() })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(info) => this.apply_active_window(info, cx),
                Err(error) => tracing::debug!("active window query failed: {error}"),
            });
        })
        .detach();
    }

    fn apply_active_window(
        &mut self,
        info: mezon_active_windows::ActiveWindowInfo,
        cx: &mut Context<Self>,
    ) {
        let Some((app_name, description, activity_type)) = detect_tracked_activity(&info) else {
            if self.last_published.is_some() {
                self.clear_published_activity(cx);
            }
            return;
        };
        let unchanged = self.last_published.as_ref().is_some_and(|published| {
            published.app_name == app_name && published.activity_type == activity_type
        });
        if unchanged {
            return;
        }
        let start_time_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as u32)
            .unwrap_or(0);
        self.last_published = Some(PublishedActivity {
            app_name: app_name.clone(),
            activity_type,
        });
        self.publish_activity(
            publish_activity_request(&app_name, &description, activity_type, start_time_seconds),
            cx,
        );
    }

    fn clear_published_activity(&mut self, cx: &mut Context<Self>) {
        if self.last_published.is_none() {
            return;
        }
        self.last_published = None;
        self.publish_activity(clear_activity_request(), cx);
    }

    fn publish_activity(&mut self, request: api::CreateActivityRequest, cx: &mut Context<Self>) {
        self.publish_generation = self.publish_generation.wrapping_add(1);
        let generation = self.publish_generation;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.create_activity(request).await;
            let _ = this.update(cx, |this, _cx| {
                if this.publish_generation != generation {
                    return;
                }
                if let Err(error) = result {
                    tracing::warn!("create_activity failed: {error}");
                }
            });
        })
        .detach();
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
        let settings = cx.new(|_| Settings::default());
        RealtimeDispatch::init(api.clone(), cx);
        cx.new(|cx| ActivityStore::new(api, settings, cx))
    }

    fn work(user_id: i64, name: &str) -> UserActivity {
        UserActivity {
            user_id: UserId(user_id),
            activity_type: ACTIVITY_TYPE_WORK,
            activity_name: name.into(),
            activity_description: String::new(),
        }
    }

    #[test]
    fn detect_tracked_activity_matches_code_and_game() {
        let code = mezon_active_windows::ActiveWindowInfo {
            os: "linux".into(),
            window_class: "Code".into(),
            window_name: "main.rs".into(),
            window_desktop: "0".into(),
            window_type: "0".into(),
            window_pid: "1".into(),
            idle_time: "0".into(),
        };
        let (app, title, activity_type) = detect_tracked_activity(&code).expect("code activity");
        assert_eq!(app, "Code");
        assert!(title.is_empty());
        assert_eq!(activity_type, ACTIVITY_TYPE_WORK);

        let lol = mezon_active_windows::ActiveWindowInfo {
            os: "windows".into(),
            window_class: "LeagueClientUx.exe".into(),
            window_name: String::new(),
            window_desktop: "0".into(),
            window_type: "0".into(),
            window_pid: "2".into(),
            idle_time: "0".into(),
        };
        let (_, _, activity_type) = detect_tracked_activity(&lol).expect("lol activity");
        assert_eq!(activity_type, ACTIVITY_TYPE_PLAY);
    }

    #[test]
    fn detect_tracked_activity_rejects_unlisted_apps() {
        let chrome = mezon_active_windows::ActiveWindowInfo {
            os: "linux".into(),
            window_class: "Google Chrome".into(),
            window_name: String::new(),
            window_desktop: "0".into(),
            window_type: "0".into(),
            window_pid: "1".into(),
            idle_time: "0".into(),
        };
        assert!(detect_tracked_activity(&chrome).is_none());
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
    fn clear_activity_request_matches_react_contract() {
        let request = clear_activity_request();
        assert_eq!(request.activity_name, "");
        assert_eq!(request.activity_type, 0);
        assert_eq!(request.status, 0);
    }

    #[test]
    fn publish_activity_request_is_process_only() {
        let request = publish_activity_request("Cursor", "", ACTIVITY_TYPE_WORK, 100);
        assert_eq!(request.activity_name, "Cursor");
        assert_eq!(request.activity_description, "");
        assert_eq!(request.activity_type, ACTIVITY_TYPE_WORK);
        assert_eq!(request.start_time_seconds, 100);
        assert_eq!(request.status, 1);
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
