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
    ManualInstall {
        version: SharedString,
        deb_path: SharedString,
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

#[derive(Clone, Copy)]
struct StoreChannel {
    feed_url: &'static str,
    page_url: &'static str,
}

const MAC_APP_STORE: StoreChannel = StoreChannel {
    feed_url: "https://cdn.komu.vn/release/latest-mac.yml",
    page_url: "macappstore://itunes.apple.com/app/mezon-desktop/id6756601798",
};

const MICROSOFT_STORE: StoreChannel = StoreChannel {
    feed_url: "https://cdn.komu.vn/release/latest.yml",
    page_url: "ms-windows-store://pdp/?ProductId=9pf25lf1fj17",
};

fn store_channel() -> Option<StoreChannel> {
    if crate::running_in_app_sandbox() {
        Some(MAC_APP_STORE)
    } else if crate::running_from_windows_store() {
        Some(MICROSOFT_STORE)
    } else {
        None
    }
}

fn auto_update_disabled() -> bool {
    std::env::var("MEZON_DISABLE_AUTO_UPDATE").is_ok()
}

fn baked_store_version() -> Option<&'static str> {
    option_env!("MEZON_STORE_VERSION")
}

fn running_from_cargo_target() -> bool {
    std::env::current_exe()
        .map(|exe| exe.components().any(|c| c.as_os_str() == "target"))
        .unwrap_or(true)
}

pub fn manual_install_command(deb_path: &str) -> String {
    format!("sudo dpkg -i {deb_path}")
}

impl AutoUpdateStore {
    pub fn init(base_url: String, cx: &mut App) -> Entity<Self> {
        mezon_updater::cleanup_stale_update_artifacts();
        let entity = cx.new(|cx| {
            let poll_task = (!auto_update_disabled()
                && (store_channel().is_some()
                    || (!cfg!(debug_assertions) && !running_from_cargo_target())))
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

    pub fn store_page_url(&self) -> Option<&'static str> {
        if let Some(channel) = store_channel() {
            return Some(channel.page_url);
        }
        // A sideloaded Windows `.exe` has no store channel, but updates are still
        // delivered through the Microsoft Store, so point the user there.
        if cfg!(target_os = "windows") {
            return Some(MICROSOFT_STORE.page_url);
        }
        None
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
        if let Some(channel) = store_channel() {
            self.check_store(channel, manual, cx);
            return;
        }
        // A sideloaded Windows `.exe` tracks the store `latest.yml` for detection and
        // sends the user to the Microsoft Store to apply the update (`check_windows_exe`).
        // Every other platform keeps the original detect-download-install self-updater.
        if cfg!(target_os = "windows") {
            self.check_windows_exe(manual, cx);
        } else {
            self.check_native(manual, cx);
        }
    }

    fn check_native(&mut self, manual: bool, cx: &mut Context<Self>) {
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
                        if let Some(deb) = outcome.manual_deb {
                            tracing::info!("update v{version} ready; manual install required");
                            AutoUpdateStatus::ManualInstall {
                                version: SharedString::from(version),
                                deb_path: SharedString::from(deb.to_string_lossy().into_owned()),
                            }
                        } else {
                            if let Some(restart_path) = outcome.restart_path {
                                cx.set_restart_path(restart_path);
                            }
                            tracing::info!("update installed; restart to apply v{version}");
                            AutoUpdateStatus::Updated {
                                version: SharedString::from(version),
                            }
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

    /// Windows `.exe` (sideloaded, not installed from the Store): detect a newer version
    /// from the same `latest.yml` the Store build watches. The user-facing action opens
    /// the Microsoft Store page (see `store_page_url`) rather than self-installing, since
    /// the app is distributed through the Store and no downloadable artifact is hosted.
    fn check_windows_exe(&mut self, manual: bool, cx: &mut Context<Self>) {
        self.spawn_feed_check(
            MICROSOFT_STORE.feed_url,
            env!("CARGO_PKG_VERSION").to_string(),
            manual,
            cx,
        );
    }

    fn check_store(&mut self, channel: StoreChannel, manual: bool, cx: &mut Context<Self>) {
        let Some(current_version) = baked_store_version() else {
            tracing::warn!("store build has no baked MEZON_STORE_VERSION; skipping update check");
            return;
        };
        self.spawn_feed_check(channel.feed_url, current_version.to_string(), manual, cx);
    }

    /// Poll a store-style feed (version-only `latest.yml`) and surface a newer version as
    /// `UpdateAvailable`. How the update is applied is left to the caller: the Store build
    /// installs via the Store API, while a sideloaded `.exe` opens the Store page.
    fn spawn_feed_check(
        &mut self,
        feed_url: &'static str,
        current_version: String,
        manual: bool,
        cx: &mut Context<Self>,
    ) {
        self.status = AutoUpdateStatus::Checking;
        cx.notify();

        let work = mezon_client::transport_runtime::handle().spawn(async move {
            mezon_updater::check_store_feed(feed_url, &current_version).await
        });

        self.pending = Some(cx.spawn(async move |this, cx| {
            let result = work
                .await
                .map_err(|e| anyhow::anyhow!("update task failed: {e}"))
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
                            tracing::error!("store update check failed: {error:#}");
                            AutoUpdateStatus::Errored {
                                message: SharedString::from(format!("{error:#}")),
                            }
                        } else {
                            tracing::info!("store update check failed: {error:#}");
                            AutoUpdateStatus::Idle
                        }
                    }
                };
                cx.notify();
            });
        }));
    }
}
