use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task};
use std::time::Duration;

use crate::auto_update::{AutoUpdateStatus, AutoUpdateStore, manual_install_command};

pub fn effective_update_status(cx: &App) -> Option<AutoUpdateStatus> {
    if let Some(store) = WinstoreUpdateStore::try_global(cx) {
        return Some(store.read(cx).status().clone());
    }
    AutoUpdateStore::try_global(cx).map(|store| store.read(cx).status().clone())
}

pub fn update_check_clicked(cx: &mut App) {
    if let Some(store) = WinstoreUpdateStore::try_global(cx) {
        store.update(cx, |store, cx| store.check(true, cx));
        return;
    }
    if let Some(store) = AutoUpdateStore::try_global(cx) {
        store.update(cx, |store, cx| store.check(true, cx));
    }
}

pub fn update_available_clicked(cx: &mut App) {
    if let Some(store) = WinstoreUpdateStore::try_global(cx) {
        store.update(cx, |store, cx| store.download(cx));
        return;
    }
    let Some(store) = AutoUpdateStore::try_global(cx) else {
        return;
    };
    if let Some(url) = store.read(cx).store_page_url() {
        cx.open_url(url);
    } else {
        store.update(cx, |store, cx| store.check(true, cx));
    }
}

pub fn update_error_clicked(cx: &mut App) {
    if WinstoreUpdateStore::try_global(cx).is_some() {
        cx.open_url("ms-windows-store://downloadsandupdates");
        return;
    }
    update_check_clicked(cx);
}

pub fn update_restart_clicked(cx: &mut App) {
    if let Some(store) = WinstoreUpdateStore::try_global(cx) {
        store.update(cx, |store, cx| store.install(cx));
        return;
    }
    cx.restart();
}

pub fn update_manual_install_clicked(cx: &mut App) {
    let Some(AutoUpdateStatus::ManualInstall { deb_path, .. }) = effective_update_status(cx) else {
        return;
    };
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(manual_install_command(
        &deb_path,
    )));
}

const FIRST_POLL_DELAY: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
const WINSTORE_FEED_URL: &str = "https://cdn.komu.vn/release/latest.yml";

fn current_version() -> &'static str {
    option_env!("MEZON_STORE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub struct WinstoreUpdateStore {
    status: AutoUpdateStatus,
    pending: Option<Task<()>>,
    _poll_task: Option<Task<()>>,
}

struct GlobalWinstoreUpdateStore(Entity<WinstoreUpdateStore>);
impl Global for GlobalWinstoreUpdateStore {}

fn enabled() -> bool {
    crate::running_from_windows_store() || std::env::var("MEZON_WINSTORE_UPDATE_TEST").is_ok()
}

fn auto_update_disabled() -> bool {
    std::env::var("MEZON_DISABLE_AUTO_UPDATE").is_ok()
}

impl WinstoreUpdateStore {
    pub fn init(cx: &mut App) {
        if !enabled() {
            return;
        }
        let entity = cx.new(|cx| {
            let poll_task = (!auto_update_disabled()).then(|| {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(FIRST_POLL_DELAY).await;
                    loop {
                        let alive = this
                            .update(cx, |this: &mut WinstoreUpdateStore, cx| {
                                this.check(false, cx)
                            })
                            .is_ok();
                        if !alive {
                            return;
                        }
                        cx.background_executor().timer(POLL_INTERVAL).await;
                    }
                })
            });
            WinstoreUpdateStore {
                status: AutoUpdateStatus::Idle,
                pending: None,
                _poll_task: poll_task,
            }
        });
        cx.set_global(GlobalWinstoreUpdateStore(entity));
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalWinstoreUpdateStore>()
            .map(|g| g.0.clone())
    }

    pub fn status(&self) -> &AutoUpdateStatus {
        &self.status
    }

    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        if !manual
            && !matches!(
                self.status,
                AutoUpdateStatus::Idle | AutoUpdateStatus::UpToDate
            )
        {
            return;
        }

        self.status = AutoUpdateStatus::Checking;
        cx.notify();

        let work = mezon_client::transport_runtime::handle().spawn(async {
            mezon_updater::check_store_feed(WINSTORE_FEED_URL, current_version()).await
        });

        self.pending = Some(cx.spawn(async move |this, cx| {
            let result = work
                .await
                .map_err(|e| anyhow::anyhow!("store check task failed: {e}"))
                .and_then(|inner| inner);

            let _ = this.update(cx, |this, cx| {
                this.pending = None;
                this.status = match result {
                    Ok(Some(version)) => AutoUpdateStatus::UpdateAvailable {
                        version: SharedString::from(version),
                    },
                    Ok(None) => {
                        if manual {
                            AutoUpdateStatus::UpToDate
                        } else {
                            AutoUpdateStatus::Idle
                        }
                    }
                    Err(error) => {
                        if manual {
                            tracing::error!("microsoft store update check failed: {error:#}");
                            AutoUpdateStatus::Errored {
                                message: SharedString::from(format!("{error:#}")),
                            }
                        } else {
                            tracing::info!("microsoft store update check failed: {error:#}");
                            AutoUpdateStatus::Idle
                        }
                    }
                };
                cx.notify();
            });
        }));
    }

    pub fn download(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let AutoUpdateStatus::UpdateAvailable { version } = &self.status else {
            return;
        };
        let version = version.clone();
        self.status = AutoUpdateStatus::Downloading {
            version: version.clone(),
            progress: None,
        };
        cx.notify();

        let (progress_tx, progress_rx) = flume::unbounded::<Option<f32>>();
        let work = cx.spawn(async move |_, _| {
            mezon_updater::winstore::download_updates(Box::new(move |fraction| {
                let _ = progress_tx.send(fraction);
            }))
            .await
        });

        self.pending = Some(cx.spawn(async move |this, cx| {
            while let Ok(fraction) = progress_rx.recv_async().await {
                let ok = this
                    .update(cx, |this, cx| {
                        this.status = AutoUpdateStatus::Downloading {
                            version: version.clone(),
                            progress: fraction,
                        };
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    return;
                }
            }

            let result = work.await;

            let _ = this.update(cx, |this, cx| {
                this.pending = None;
                this.status = match result {
                    Ok(()) => AutoUpdateStatus::Updated {
                        version: version.clone(),
                    },
                    Err(error) => {
                        tracing::error!("microsoft store download failed: {error:#}");
                        AutoUpdateStatus::Errored {
                            message: SharedString::from(format!("{error:#}")),
                        }
                    }
                };
                cx.notify();
            });
        }));
    }

    pub fn install(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let AutoUpdateStatus::Updated { version } = &self.status else {
            return;
        };
        let version = version.clone();
        self.status = AutoUpdateStatus::Installing {
            version: version.clone(),
        };
        cx.notify();

        self.pending = Some(cx.spawn(async move |this, cx| {
            let result = mezon_updater::winstore::install_updates().await;

            let _ = this.update(cx, |this, cx| {
                this.pending = None;
                this.status = match result {
                    Ok(()) => AutoUpdateStatus::Updated {
                        version: version.clone(),
                    },
                    Err(error) => {
                        tracing::error!("microsoft store install failed: {error:#}");
                        AutoUpdateStatus::Errored {
                            message: SharedString::from(format!("{error:#}")),
                        }
                    }
                };
                cx.notify();
            });
        }));
    }
}
