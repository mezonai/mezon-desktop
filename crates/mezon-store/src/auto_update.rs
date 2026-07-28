use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task};
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
const FIRST_POLL_DELAY: Duration = Duration::from_secs(5);
const AUTO_CHECK_MIN_GAP: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum AutoUpdateStatus {
    Idle,
    Checking,
    UpdateAvailable {
        version: SharedString,
    },
    Downloading {
        version: SharedString,
        progress: Option<f32>,
    },
    Installing {
        version: SharedString,
    },
    Updated {
        version: SharedString,
    },
    UpToDate,
    Errored {
        message: SharedString,
    },
}

enum UpdatePhase {
    Found { version: String },
    Downloading { fraction: Option<f32> },
    Installing,
}

enum CheckOutcome {
    UpToDate,
    Installed {
        version: String,
        outcome: mezon_updater::InstallOutcome,
    },
    ManualInstallAvailable {
        version: String,
    },
}

pub struct AutoUpdateStore {
    status: AutoUpdateStatus,
    base_url: String,
    last_auto_check: Option<std::time::SystemTime>,
    _poll_task: Option<Task<()>>,
    pending: Option<Task<()>>,
}

struct GlobalAutoUpdateStore(Entity<AutoUpdateStore>);
impl Global for GlobalAutoUpdateStore {}

fn auto_update_disabled() -> bool {
    std::env::var("MEZON_DISABLE_AUTO_UPDATE").is_ok()
}

fn running_from_cargo_target() -> bool {
    std::env::current_exe()
        .map(|exe| exe.components().any(|c| c.as_os_str() == "target"))
        .unwrap_or(true)
}

impl AutoUpdateStore {
    pub fn init(base_url: String, cx: &mut App) -> Entity<Self> {
        mezon_updater::cleanup_stale_update_artifacts();
        let entity = cx.new(|cx| {
            let poll_task = (!auto_update_disabled()
                && !cfg!(debug_assertions)
                && !running_from_cargo_target())
            .then(|| {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(FIRST_POLL_DELAY).await;
                    loop {
                        let alive = this
                            .update(cx, |this: &mut AutoUpdateStore, cx| this.check(false, cx))
                            .is_ok();
                        if !alive {
                            return;
                        }
                        cx.background_executor().timer(POLL_INTERVAL).await;
                    }
                })
            });
            AutoUpdateStore {
                status: AutoUpdateStatus::Idle,
                base_url,
                last_auto_check: None,
                _poll_task: poll_task,
                pending: None,
            }
        });
        cx.set_global(GlobalAutoUpdateStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAutoUpdateStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAutoUpdateStore>()
            .map(|g| g.0.clone())
    }

    pub fn status(&self) -> &AutoUpdateStatus {
        &self.status
    }

    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        if auto_update_disabled() {
            return;
        }
        if !manual {
            let now = std::time::SystemTime::now();
            let recently_checked = self.last_auto_check.is_some_and(|last| {
                now.duration_since(last)
                    .is_ok_and(|elapsed| elapsed < AUTO_CHECK_MIN_GAP)
            });
            if recently_checked {
                return;
            }
            self.last_auto_check = Some(now);
        }
        let current_version = match &self.status {
            AutoUpdateStatus::Updated { version } => version.to_string(),
            _ => env!("CARGO_PKG_VERSION").to_string(),
        };
        if let AutoUpdateStatus::Updated { .. } = self.status
            && !manual
        {
            return;
        }

        self.status = AutoUpdateStatus::Checking;
        cx.notify();

        let base_url = self.base_url.clone();
        let app_path = cx.app_path().ok();
        let (phase_tx, phase_rx) = flume::unbounded::<UpdatePhase>();

        let work = mezon_client::transport_runtime::handle().spawn(async move {
            let Some(manifest) =
                mezon_updater::check_for_updates_with_manifest(&base_url, &current_version).await?
            else {
                return anyhow::Ok(CheckOutcome::UpToDate);
            };
            let version = manifest.version.clone();
            if !manual && mezon_updater::needs_privileged_install() {
                return anyhow::Ok(CheckOutcome::ManualInstallAvailable { version });
            }
            let _ = phase_tx.send(UpdatePhase::Found {
                version: version.clone(),
            });
            let progress_tx = phase_tx.clone();
            let downloaded =
                mezon_updater::download_update(&base_url, &manifest, move |written, total| {
                    let fraction =
                        total.map(|total| (written as f32 / total as f32).clamp(0.0, 1.0));
                    let _ = progress_tx.send(UpdatePhase::Downloading { fraction });
                })
                .await?;
            let _ = phase_tx.send(UpdatePhase::Installing);
            let outcome = mezon_updater::install_update(&downloaded, app_path.as_deref()).await?;
            anyhow::Ok(CheckOutcome::Installed { version, outcome })
        });

        self.pending = Some(cx.spawn(async move |this, cx| {
            let mut version: SharedString = SharedString::default();
            while let Ok(phase) = phase_rx.recv_async().await {
                let ok = this
                    .update(cx, |this, cx| {
                        match phase {
                            UpdatePhase::Found { version: v } => {
                                version = SharedString::from(v);
                                this.status = AutoUpdateStatus::Downloading {
                                    version: version.clone(),
                                    progress: None,
                                };
                            }
                            UpdatePhase::Downloading { fraction } => {
                                this.status = AutoUpdateStatus::Downloading {
                                    version: version.clone(),
                                    progress: fraction,
                                };
                            }
                            UpdatePhase::Installing => {
                                this.status = AutoUpdateStatus::Installing {
                                    version: version.clone(),
                                };
                            }
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    return;
                }
            }

            let result = work
                .await
                .map_err(|e| anyhow::anyhow!("update task failed: {e}"))
                .and_then(|inner| inner);

            let _ = this.update(cx, |this, cx| {
                this.pending = None;
                this.status = match result {
                    Ok(CheckOutcome::Installed { version, outcome }) => {
                        if let Some(restart_path) = outcome.restart_path {
                            cx.set_restart_path(restart_path);
                        }
                        tracing::info!("update installed; restart to apply v{version}");
                        AutoUpdateStatus::Updated {
                            version: SharedString::from(version),
                        }
                    }
                    Ok(CheckOutcome::ManualInstallAvailable { version }) => {
                        tracing::info!("update v{version} available; waiting for user action");
                        AutoUpdateStatus::UpdateAvailable {
                            version: SharedString::from(version),
                        }
                    }
                    Ok(CheckOutcome::UpToDate) => {
                        if manual {
                            AutoUpdateStatus::UpToDate
                        } else {
                            AutoUpdateStatus::Idle
                        }
                    }
                    Err(error) => {
                        if manual {
                            tracing::error!("auto-update failed: {error:#}");
                            AutoUpdateStatus::Errored {
                                message: SharedString::from(format!("{error:#}")),
                            }
                        } else {
                            tracing::info!("auto-update check failed: {error:#}");
                            AutoUpdateStatus::Idle
                        }
                    }
                };
                cx.notify();
            });
        }));
    }
}
