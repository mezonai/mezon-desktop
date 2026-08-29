use gpui::{App, SharedString};
use mezon_store::{DownloadEvent, Settings};

use crate::app::shell::Shell;
use crate::components::primitives::ToastKind;

/// Save `url` to disk via the native folder picker, surfacing a live progress
/// toast that resolves into a success/error toast on completion.
pub fn save_with_progress_toast(url: SharedString, filename: SharedString, cx: &mut App) {
    let locale = Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_default();
    let key = url.clone();
    let name = if filename.is_empty() {
        SharedString::from("file")
    } else {
        filename.clone()
    };
    mezon_store::download_url_with_dialog(
        url,
        filename,
        move |event, cx| {
            let key = key.clone();
            let name = name.clone();
            let locale = locale.clone();
            Shell::global(cx).update(cx, move |shell, cx| match event {
                DownloadEvent::Started => {
                    let message =
                        mezon_i18n::t(&locale, "download.downloading").replace("{{name}}", &name);
                    shell.progress_toast(key, message, 0., cx);
                }
                DownloadEvent::Progress { written, total } => {
                    let progress = total.map_or(0., |total| written as f32 / total as f32);
                    let message =
                        mezon_i18n::t(&locale, "download.downloading").replace("{{name}}", &name);
                    shell.progress_toast(key, message, progress, cx);
                }
                DownloadEvent::Finished { path } => {
                    cx.reveal_path(&path);
                    let message =
                        mezon_i18n::t(&locale, "download.saved").replace("{{name}}", &name);
                    shell.finish_toast(key, ToastKind::Success, message, cx);
                }
                DownloadEvent::Failed => {
                    let message = mezon_i18n::t(&locale, "download.failed").to_string();
                    shell.finish_toast(key, ToastKind::Error, message, cx);
                }
            });
        },
        cx,
    );
}
