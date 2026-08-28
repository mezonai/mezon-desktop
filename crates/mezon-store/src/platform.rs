use gpui::{App, AppContext, ClipboardItem, Entity, Global, Image, ImageFormat, SharedString};
use std::sync::Arc;

pub enum DownloadEvent {
    Started,
    Progress { written: u64, total: Option<u64> },
    Finished { path: std::path::PathBuf },
    Failed,
}

pub fn download_url_with_dialog(
    url: SharedString,
    filename: SharedString,
    on_event: impl Fn(DownloadEvent, &mut App) + 'static,
    cx: &mut App,
) {
    let directory = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let suggested = filename.to_string();
    let receiver = cx.prompt_for_new_path(&directory, Some(suggested.as_str()));
    let url = url.to_string();
    cx.spawn(async move |cx| {
        let path = match receiver.await {
            Ok(Ok(Some(path))) => path,
            _ => return,
        };
        let finished_path = path.clone();
        cx.update(|cx| on_event(DownloadEvent::Started, cx));
        let (tx, rx) = flume::unbounded::<(u64, Option<u64>)>();
        let download = cx.background_executor().spawn(async move {
            mezon_client::transport_runtime::download_to(&url, path, move |written, total| {
                let _ = tx.send((written, total));
            })
            .await
        });
        while let Ok((written, total)) = rx.recv_async().await {
            cx.update(|cx| on_event(DownloadEvent::Progress { written, total }, cx));
        }
        let event = match download.await {
            Ok(()) => DownloadEvent::Finished {
                path: finished_path,
            },
            Err(error) => {
                tracing::error!("attachment download failed: {error}");
                DownloadEvent::Failed
            }
        };
        cx.update(|cx| on_event(event, cx));
    })
    .detach();
}

const CLIPBOARD_IMAGE_MAX_DIMENSION: u32 = 16_384;
const CLIPBOARD_IMAGE_MAX_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

pub fn copy_image_url_to_clipboard(
    url: SharedString,
    on_result: impl FnOnce(bool, &mut App) + 'static,
    cx: &mut App,
) {
    let url = url.to_string();
    if url.is_empty() {
        return;
    }
    cx.spawn(async move |cx| {
        let png = cx
            .background_executor()
            .spawn(async move {
                let (bytes, _content_type) =
                    mezon_client::transport_runtime::fetch_bytes(&url).await?;
                let mut reader =
                    image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?;
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(CLIPBOARD_IMAGE_MAX_DIMENSION);
                limits.max_image_height = Some(CLIPBOARD_IMAGE_MAX_DIMENSION);
                limits.max_alloc = Some(CLIPBOARD_IMAGE_MAX_ALLOC_BYTES);
                reader.limits(limits);
                let decoded = reader.decode()?;
                let mut buffer = std::io::Cursor::new(Vec::new());
                decoded.write_to(&mut buffer, image::ImageFormat::Png)?;
                anyhow::Ok(buffer.into_inner())
            })
            .await;
        let ok = match &png {
            Ok(_) => true,
            Err(error) => {
                tracing::error!("copy image to clipboard failed: {error}");
                false
            }
        };
        cx.update(|cx| {
            if let Ok(png) = png {
                cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                    ImageFormat::Png,
                    png,
                )));
            }
            on_result(ok, cx);
        });
    })
    .detach();
}

pub type OpenUrlFn = Arc<dyn Fn(&str) -> anyhow::Result<()> + Send + Sync>;
/// Download `url` and save it locally under the given suggested filename.
pub type SaveAttachmentFn = Arc<dyn Fn(&str, &str) -> anyhow::Result<()> + Send + Sync>;
pub type NotifyFn = Arc<dyn Fn(DesktopNotification) + Send + Sync>;
pub type CliInstallVisibleFn = Arc<dyn Fn() -> bool + Send + Sync>;
pub type CliInstallStateFn = Arc<dyn Fn() -> bool + Send + Sync>;
pub type CliInstallToggleFn = Arc<dyn Fn() -> anyhow::Result<bool> + Send + Sync>;
pub type McpStatusFn = Arc<dyn Fn() -> McpServerStatus + Send + Sync>;
pub type McpStartFn = Arc<dyn Fn(bool) -> anyhow::Result<McpServerStatus> + Send + Sync>;
pub type McpStopFn = Arc<dyn Fn() -> anyhow::Result<McpServerStatus> + Send + Sync>;
/// Returns whether the OS permits desktop notifications (false only when explicitly denied).
pub type NotificationPermitFn = Arc<dyn Fn() -> bool + Send + Sync>;
pub type CurrentLocationFn = Arc<dyn Fn() -> anyhow::Result<(f64, f64)> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpServerStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub read_only: bool,
    pub url: Option<String>,
}

pub struct McpServerHooks {
    pub status: McpStatusFn,
    pub start: McpStartFn,
    pub stop: McpStopFn,
}

pub struct CliInstallHooks {
    pub visible: CliInstallVisibleFn,
    pub installed: CliInstallStateFn,
    pub toggle: CliInstallToggleFn,
}

pub struct DesktopNotification {
    pub title: String,
    pub body: String,
    pub channel_id: Option<String>,
    pub clan_id: Option<String>,
    pub link: Option<String>,
    /// Local path to a downloaded sender-avatar image, attached as the icon.
    pub icon_path: Option<String>,
}

pub struct PlatformStore {
    open_url: Option<OpenUrlFn>,
    open_url_app_window: Option<OpenUrlFn>,
    save_attachment: Option<SaveAttachmentFn>,
    notifier: Option<NotifyFn>,
    cli_install: Option<CliInstallHooks>,
    mcp_server: Option<McpServerHooks>,
    notification_permit: Option<NotificationPermitFn>,
    current_location: Option<CurrentLocationFn>,
}

impl PlatformStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            open_url: None,
            open_url_app_window: None,
            save_attachment: None,
            notifier: None,
            cli_install: None,
            mcp_server: None,
            notification_permit: None,
            current_location: None,
        });
        cx.set_global(GlobalPlatformStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPlatformStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalPlatformStore>().map(|g| g.0.clone())
    }

    pub fn set_open_url(entity: &Entity<Self>, f: OpenUrlFn, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.open_url = Some(f);
            cx.notify();
        });
    }

    pub fn set_save_attachment(entity: &Entity<Self>, f: SaveAttachmentFn, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.save_attachment = Some(f);
            cx.notify();
        });
    }

    pub fn set_open_url_app_window(entity: &Entity<Self>, f: OpenUrlFn, cx: &mut App) {
        entity.update(cx, |store, _| {
            store.open_url_app_window = Some(f);
        });
    }

    /// Hand back the toolbar-less-window opener, falling back to the normal-tab
    /// one when the platform layer has not registered it.
    ///
    /// This returns the closure instead of calling it because opening an app
    /// window probes the OS for the default browser, which blocks; callers must
    /// run it on a background thread, not on the foreground/UI thread.
    pub fn app_window_opener(&self) -> Option<OpenUrlFn> {
        self.open_url_app_window
            .clone()
            .or_else(|| self.open_url.clone())
    }

    pub fn open_app_window(url: String, cx: &App) {
        let Some(platform) = Self::try_global(cx) else {
            tracing::warn!("app window launch skipped: platform store is unavailable");
            return;
        };
        let Some(open) = platform.read(cx).app_window_opener() else {
            tracing::warn!("app window launch skipped: no URL opener is registered");
            return;
        };
        cx.background_spawn(async move {
            if let Err(error) = open(&url) {
                tracing::warn!("app window launch failed: {error:#}");
            }
        })
        .detach();
    }

    pub fn open_url_external(&self, url: &str) -> anyhow::Result<()> {
        match &self.open_url {
            Some(f) => f(url),
            None => Err(anyhow::anyhow!("open_url not registered")),
        }
    }

    pub fn save_attachment(&self, url: &str, filename: &str) -> anyhow::Result<()> {
        match &self.save_attachment {
            Some(f) => f(url, filename),
            None => Err(anyhow::anyhow!("save_attachment not registered")),
        }
    }

    pub fn set_notifier(entity: &Entity<Self>, f: NotifyFn, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.notifier = Some(f);
            cx.notify();
        });
    }

    pub fn show_notification(&self, notification: DesktopNotification) {
        if let Some(f) = &self.notifier {
            f(notification);
        }
    }

    pub fn set_cli_install(entity: &Entity<Self>, hooks: CliInstallHooks, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.cli_install = Some(hooks);
            cx.notify();
        });
    }

    pub fn cli_install_visible(&self) -> bool {
        self.cli_install
            .as_ref()
            .is_some_and(|hooks| (hooks.visible)())
    }

    pub fn cli_install_installed(&self) -> bool {
        self.cli_install
            .as_ref()
            .is_some_and(|hooks| (hooks.installed)())
    }

    pub fn cli_install_toggle(&self) -> anyhow::Result<bool> {
        match &self.cli_install {
            Some(hooks) => (hooks.toggle)(),
            None => Err(anyhow::anyhow!("cli install not available")),
        }
    }

    pub fn cli_install_toggle_fn(&self) -> Option<CliInstallToggleFn> {
        self.cli_install
            .as_ref()
            .map(|hooks| Arc::clone(&hooks.toggle))
    }

    pub fn set_mcp_server(entity: &Entity<Self>, hooks: McpServerHooks, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.mcp_server = Some(hooks);
            cx.notify();
        });
    }

    pub fn mcp_server_available(&self) -> bool {
        self.mcp_server.is_some()
    }

    pub fn mcp_server_status(&self) -> McpServerStatus {
        self.mcp_server
            .as_ref()
            .map(|hooks| (hooks.status)())
            .unwrap_or_default()
    }

    pub fn mcp_server_start_fn(&self) -> Option<McpStartFn> {
        self.mcp_server
            .as_ref()
            .map(|hooks| Arc::clone(&hooks.start))
    }

    pub fn mcp_server_stop_fn(&self) -> Option<McpStopFn> {
        self.mcp_server
            .as_ref()
            .map(|hooks| Arc::clone(&hooks.stop))
    }

    pub fn set_notification_permit(entity: &Entity<Self>, f: NotificationPermitFn, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.notification_permit = Some(f);
            cx.notify();
        });
    }

    /// Whether OS notifications are permitted; `true` when unknown (no callback yet),
    /// so a pending authorization does not block the notification stream.
    pub fn notifications_permitted(&self) -> bool {
        self.notification_permit.as_ref().is_none_or(|f| f())
    }

    pub fn set_current_location(entity: &Entity<Self>, f: CurrentLocationFn, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.current_location = Some(f);
            cx.notify();
        });
    }

    pub fn current_location(&self) -> anyhow::Result<(f64, f64)> {
        match &self.current_location {
            Some(f) => f(),
            None => Err(anyhow::anyhow!("current_location not registered")),
        }
    }

    pub fn current_location_fn(&self) -> Option<CurrentLocationFn> {
        self.current_location.clone()
    }
}

struct GlobalPlatformStore(Entity<PlatformStore>);
impl Global for GlobalPlatformStore {}
