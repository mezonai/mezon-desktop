use gpui::{App, AppContext, ClipboardItem, Entity, Global, Image, ImageFormat, SharedString};
use std::sync::Arc;

pub enum DownloadEvent {
    Started,
    Progress { written: u64, total: Option<u64> },
    Finished,
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
            Ok(()) => DownloadEvent::Finished,
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

pub struct DesktopNotification {
    pub title: String,
    pub body: String,
    pub channel_id: Option<String>,
}

pub struct PlatformStore {
    open_url: Option<OpenUrlFn>,
    save_attachment: Option<SaveAttachmentFn>,
    notifier: Option<NotifyFn>,
}

impl PlatformStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            open_url: None,
            save_attachment: None,
            notifier: None,
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
}

struct GlobalPlatformStore(Entity<PlatformStore>);
impl Global for GlobalPlatformStore {}
