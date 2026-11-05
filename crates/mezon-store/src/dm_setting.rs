use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, Global, Task};
use mezon_client::{ApiStatusError, AppApi, ConnectionStatus, api_status_from_error};

pub struct DmSettingStore {
    only_friends_dm: bool,
    save_generation: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalDmSettingStore(Entity<DmSettingStore>);

impl Global for GlobalDmSettingStore {}

impl DmSettingStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalDmSettingStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            only_friends_dm: false,
            save_generation: 0,
            api,
            _conn_watch: conn_watch,
        }
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalDmSettingStore>()
            .map(|global| global.0.clone())
    }

    pub fn only_friends_dm(&self) -> bool {
        self.only_friends_dm
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.save_generation = self.save_generation.wrapping_add(1);
        self.only_friends_dm = false;
        cx.notify();
    }

    pub fn set_only_friends(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.only_friends_dm == enabled {
            return;
        }
        let previous = self.only_friends_dm;
        self.only_friends_dm = enabled;
        self.save_generation = self.save_generation.wrapping_add(1);
        let generation = self.save_generation;
        cx.notify();
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.update_dm_setting(enabled).await;
            let _ = this.update(cx, |this, cx| {
                if this.save_generation != generation {
                    return;
                }
                if let Err(error) = result {
                    tracing::warn!("update dm setting failed: {error}");
                    this.only_friends_dm = previous;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            loop {
                if *status_rx.borrow() == ConnectionStatus::Connected {
                    let Ok(generation) = this.update(cx, |this, _| this.save_generation) else {
                        break;
                    };
                    let fetched = api.get_dm_setting().await;
                    if this
                        .update(cx, |this, cx| {
                            if this.save_generation != generation {
                                return;
                            }
                            match fetched {
                                Ok(enabled) => {
                                    this.only_friends_dm = enabled;
                                    cx.notify();
                                }
                                Err(error) => {
                                    tracing::warn!("get dm setting failed: {error}");
                                }
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                if status_rx.changed().await.is_err() {
                    break;
                }
            }
        })
    }
}

#[derive(Debug)]
struct DmSendStep;

impl std::fmt::Display for DmSendStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("direct message send step")
    }
}

pub(crate) fn mark_dm_send_step(err: anyhow::Error) -> anyhow::Error {
    err.context(DmSendStep)
}

pub fn is_new_dm_denied(err: &anyhow::Error) -> bool {
    err.downcast_ref::<DmSendStep>().is_none()
        && api_status_from_error(err).is_some_and(ApiStatusError::is_permission_denied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_denied_marks_a_new_dm_as_denied() {
        let denied: anyhow::Error = ApiStatusError {
            code: ApiStatusError::PERMISSION_DENIED,
        }
        .into();
        let other: anyhow::Error = ApiStatusError {
            code: ApiStatusError::INTERNAL,
        }
        .into();
        assert!(is_new_dm_denied(&denied));
        assert!(!is_new_dm_denied(&other));
        assert!(!is_new_dm_denied(&anyhow::anyhow!("offline")));
    }

    #[test]
    fn permission_denied_from_the_send_step_is_not_a_new_dm_denial() {
        let denied: anyhow::Error = ApiStatusError {
            code: ApiStatusError::PERMISSION_DENIED,
        }
        .into();
        let marked = mark_dm_send_step(denied);
        assert!(!is_new_dm_denied(&marked));
        assert_eq!(
            api_status_from_error(&marked).map(|status| status.code),
            Some(ApiStatusError::PERMISSION_DENIED)
        );
    }
}
