/// System tray icon with context menu.
///
/// Menu items:
///   • Show Mezon         — brings the main window to front via callback
///   • Check for Updates  — calls `mezon_updater::check_for_updates` asynchronously
///   • ─────────────────
///   • Quit               — terminates the process
use anyhow::Result;
use std::sync::Arc;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};

const SHOW_ID: &str = "show";
const UPDATE_ID: &str = "update";
const QUIT_ID: &str = "quit";

pub struct MezonTray {
    /// Keep-alive handle — dropped → tray icon removed
    _icon: TrayIcon,
}

impl MezonTray {
    /// Create and register the system tray icon.
    ///
    /// - `on_show`    — called when the user clicks "Show Mezon"
    /// - `on_quit`    — called when the user clicks "Quit"
    /// - `rt_handle`  — tokio runtime handle used to spawn the async update check
    ///
    /// The returned `MezonTray` must be kept alive for the tray to remain visible.
    pub fn new(
        on_show: impl Fn() + Send + Sync + 'static,
        on_quit: impl Fn() + Send + Sync + 'static,
        rt_handle: Arc<tokio::runtime::Handle>,
    ) -> Result<Self> {
        let menu = Menu::new();

        let item_show = MenuItem::with_id(SHOW_ID, "Show Mezon", true, None);
        let item_update = MenuItem::with_id(UPDATE_ID, "Check for Updates", true, None);
        let item_quit = MenuItem::with_id(QUIT_ID, "Quit Mezon", true, None);

        menu.append(&item_show)?;
        menu.append(&item_update)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&item_quit)?;

        let icon = build_tray_icon();

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Mezon")
            .with_icon(icon)
            .build()?;

        let on_show = Arc::new(on_show);
        let on_quit = Arc::new(on_quit);
        let receiver = MenuEvent::receiver().clone();

        std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                match event.id().0.as_str() {
                    SHOW_ID => (on_show)(),
                    UPDATE_ID => {
                        let handle = rt_handle.clone();
                        handle.spawn(async {
                            match mezon_updater::check_for_updates(env!("CARGO_PKG_VERSION"))
                                .await
                            {
                                Ok(Some(version)) => {
                                    tracing::info!(
                                        "Update available: v{version} — \
                                         download from https://mezon.ai/download"
                                    );
                                    // Stage 3+: show an in-app update banner instead.
                                    let _ = open::that("https://mezon.ai/download");
                                }
                                Ok(None) => {
                                    tracing::info!("Mezon is up to date");
                                }
                                Err(e) => {
                                    tracing::warn!("Update check failed: {e}");
                                }
                            }
                        });
                    }
                    QUIT_ID => (on_quit)(),
                    _ => {}
                }
            }
        });

        tracing::debug!("System tray created");
        Ok(Self { _icon: tray })
    }
}

/// Build a small RGBA tray icon.
/// Loads the embedded PNG asset if available; falls back to a generated solid-colour icon.
fn build_tray_icon() -> tray_icon::Icon {
    for path in &icon_search_paths() {
        if path.exists() {
            match load_icon_from_path(path) {
                Ok(icon) => {
                    tracing::debug!("Loaded tray icon from {}", path.display());
                    return icon;
                }
                Err(e) => {
                    tracing::warn!("Failed to load tray icon from {}: {e}", path.display());
                }
            }
        }
    }
    tracing::debug!("Using generated fallback tray icon");
    build_fallback_icon()
}

fn icon_search_paths() -> Vec<std::path::PathBuf> {
    let mut paths = vec![];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            paths.push(parent.join("assets/icons/trayicon.png"));
            paths.push(parent.join("../../../assets/icons/trayicon.png"));
            paths.push(parent.join("../../../../assets/icons/trayicon.png"));
        }
    }
    paths.push(std::path::PathBuf::from("assets/icons/trayicon.png"));
    paths
}

fn load_icon_from_path(path: &std::path::Path) -> Result<tray_icon::Icon> {
    let img = image::open(path)?.into_rgba8();
    let (w, h) = img.dimensions();
    let rgba = img.into_raw();
    Ok(tray_icon::Icon::from_rgba(rgba, w, h)?)
}

/// Generates a 22×22 solid `#5865F2` brand-blue icon as fallback.
fn build_fallback_icon() -> tray_icon::Icon {
    const SIZE: u32 = 22;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        rgba.extend_from_slice(&[0x58, 0x65, 0xF2, 0xFF]);
    }
    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).expect("Failed to build fallback tray icon")
}
