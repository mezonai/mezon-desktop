use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task};
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
const FIRST_POLL_DELAY: Duration = Duration::from_secs(5);
const AUTO_CHECK_MIN_GAP: Duration = Duration::from_secs(5 * 60);
const DEFAULT_BACKGROUND_APPLY_DELAY_SECS: u64 = 60;
const START_HIDDEN_MARKER_MAX_AGE_SECS: u64 = 300;

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
    window_hidden: bool,
    apply_task: Option<Task<()>>,
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

fn background_apply_delay() -> Duration {
    let secs = std::env::var("MEZON_UPDATE_APPLY_DELAY_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(DEFAULT_BACKGROUND_APPLY_DELAY_SECS);
    Duration::from_secs(secs)
}

fn app_busy(cx: &App) -> bool {
    let voice_busy = crate::voice::VoiceStore::try_global(cx).is_some_and(|store| {
        let voice = store.read(cx);
        voice.connection().active_channel_id().is_some()
            || voice.recording_state() != crate::voice::RecordingState::Idle
    });
    if voice_busy {
        return true;
    }
    let call_busy = crate::call::CallStore::try_global(cx)
        .is_some_and(|store| store.read(cx).phase() != crate::call::CallPhase::Idle);
    if call_busy {
        return true;
    }
    if crate::platform::downloads_in_flight() {
        return true;
    }
    crate::messages::MessagesStore::try_global(cx)
        .is_some_and(|store| store.read(cx).has_running_uploads())
}

fn start_hidden_marker_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("mezon")
        .join("update_start_hidden")
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_marker_at(path: &std::path::Path, now: u64) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(path, now.to_string()) {
        tracing::warn!("failed to write update start-hidden marker: {error}");
    }
}

fn write_start_hidden_marker() {
    write_marker_at(&start_hidden_marker_path(), unix_now_secs());
}

fn take_marker_at(path: &std::path::Path, now: u64) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let _ = std::fs::remove_file(path);
    let Ok(written) = raw.trim().parse::<u64>() else {
        return false;
    };
    now.saturating_sub(written) <= START_HIDDEN_MARKER_MAX_AGE_SECS
}

pub fn take_start_hidden_marker() -> bool {
    take_marker_at(&start_hidden_marker_path(), unix_now_secs())
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
                window_hidden: false,
                apply_task: None,
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
        store_channel().map(|channel| channel.page_url)
    }

    pub fn set_main_window_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        self.window_hidden = hidden;
        if hidden {
            self.schedule_background_apply(cx);
        } else {
            self.apply_task = None;
        }
    }

    fn schedule_background_apply(&mut self, cx: &mut Context<Self>) {
        if !self.window_hidden
            || !matches!(self.status, AutoUpdateStatus::Updated { .. })
            || self.apply_task.is_some()
        {
            return;
        }
        let delay = background_apply_delay();
        self.apply_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(delay).await;
                let keep_waiting = this
                    .update(cx, |this, cx| {
                        if !this.window_hidden
                            || !matches!(this.status, AutoUpdateStatus::Updated { .. })
                        {
                            this.apply_task = None;
                            return false;
                        }
                        if app_busy(cx) {
                            return true;
                        }
                        this.apply_task = None;
                        this.relaunch_hidden(cx);
                        false
                    })
                    .unwrap_or(false);
                if !keep_waiting {
                    return;
                }
            }
        }));
    }

    fn relaunch_hidden(&self, cx: &mut Context<Self>) {
        let AutoUpdateStatus::Updated { version } = &self.status else {
            return;
        };
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::process::CommandExt as _;

            let Some(bundle_path) = cx
                .app_path()
                .ok()
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
            else {
                return;
            };
            write_start_hidden_marker();
            tracing::info!("restarting hidden to apply update v{version}");
            let pid = std::process::id().to_string();
            let script = r#"while kill -0 "$0" 2>/dev/null; do sleep 0.1; done; open -g -j "$1""#;
            match std::process::Command::new("/bin/bash")
                .arg("-c")
                .arg(script)
                .arg(pid)
                .arg(bundle_path)
                .process_group(0)
                .spawn()
            {
                Ok(_) => cx.quit(),
                Err(error) => {
                    let _ = std::fs::remove_file(start_hidden_marker_path());
                    tracing::error!("failed to spawn hidden restart script: {error}");
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            write_start_hidden_marker();
            tracing::info!("restarting hidden to apply update v{version}");
            cx.restart();
        }
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
                if matches!(this.status, AutoUpdateStatus::Updated { .. }) {
                    this.schedule_background_apply(cx);
                }
                cx.notify();
            });
        }));
    }

    fn check_store(&mut self, channel: StoreChannel, manual: bool, cx: &mut Context<Self>) {
        let Some(current_version) = baked_store_version() else {
            tracing::warn!("store build has no baked MEZON_STORE_VERSION; skipping update check");
            return;
        };

        self.status = AutoUpdateStatus::Checking;
        cx.notify();

        let work = mezon_client::transport_runtime::handle().spawn(async move {
            mezon_updater::check_store_feed(channel.feed_url, current_version).await
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

#[cfg(test)]
mod tests {
    use super::{AutoUpdateStatus, AutoUpdateStore, take_marker_at, write_marker_at};
    use gpui::{App, AppContext, Entity};

    fn scratch_marker_path(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "mezon-update-marker-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("update_start_hidden");
        (dir, path)
    }

    fn new_test_store(status: AutoUpdateStatus, cx: &mut App) -> Entity<AutoUpdateStore> {
        cx.new(|_| AutoUpdateStore {
            status,
            base_url: String::new(),
            last_auto_check: None,
            _poll_task: None,
            pending: None,
            window_hidden: false,
            apply_task: None,
        })
    }

    #[test]
    fn fresh_marker_is_taken_once() {
        let (dir, path) = scratch_marker_path("fresh");
        write_marker_at(&path, 1_000);
        assert!(take_marker_at(&path, 1_010));
        assert!(!take_marker_at(&path, 1_010));
        write_marker_at(&path, 1_000);
        assert!(take_marker_at(&path, 1_300));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn marker_older_than_300s_returns_false() {
        let (dir, path) = scratch_marker_path("stale");
        write_marker_at(&path, 1_000);
        assert!(!take_marker_at(&path, 1_301));
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_marker_returns_false() {
        let (dir, path) = scratch_marker_path("missing");
        assert!(!take_marker_at(&path, 1_000));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[gpui::test]
    fn schedules_background_apply_only_when_updated_and_hidden(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let updated = new_test_store(
                AutoUpdateStatus::Updated {
                    version: "2.2.0".into(),
                },
                cx,
            );
            updated.update(cx, |store, cx| {
                assert!(store.apply_task.is_none());
                store.set_main_window_hidden(true, cx);
                assert!(store.apply_task.is_some());
                store.set_main_window_hidden(false, cx);
                assert!(store.apply_task.is_none());
            });

            let idle = new_test_store(AutoUpdateStatus::Idle, cx);
            idle.update(cx, |store, cx| {
                store.set_main_window_hidden(true, cx);
                assert!(store.apply_task.is_none());
            });
        });
    }
}
