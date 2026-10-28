use anyhow::Result;

#[cfg(target_os = "linux")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/icons/trayicon-linux.png"
));

#[cfg(not(target_os = "linux"))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/icons/trayicon.png"
));

struct TrayPixels {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn tray_pixels() -> TrayPixels {
    for path in &icon_search_paths() {
        if path.exists() {
            match decode_path(path) {
                Ok(pixels) => {
                    tracing::debug!("Loaded tray icon from {}", path.display());
                    return pixels;
                }
                Err(e) => {
                    tracing::warn!("Failed to load tray icon from {}: {e}", path.display());
                }
            }
        }
    }
    match decode_bytes(TRAY_ICON_BYTES) {
        Ok(pixels) => {
            tracing::debug!("Loaded embedded tray icon");
            return pixels;
        }
        Err(e) => tracing::warn!("Failed to load embedded tray icon: {e}"),
    }
    tracing::debug!("Using generated fallback tray icon");
    fallback_pixels()
}

fn decode_bytes(data: &[u8]) -> Result<TrayPixels> {
    let img = image::load_from_memory(data)?.into_rgba8();
    let (width, height) = img.dimensions();
    Ok(TrayPixels {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn decode_path(path: &std::path::Path) -> Result<TrayPixels> {
    let img = image::open(path)?.into_rgba8();
    let (width, height) = img.dimensions();
    Ok(TrayPixels {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn icon_search_paths() -> Vec<std::path::PathBuf> {
    let mut paths = vec![];
    if cfg!(target_os = "linux") {
        paths.push(std::path::PathBuf::from("assets/icons/trayicon-linux.png"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        paths.push(parent.join("assets/icons/trayicon-linux.png"));
        paths.push(parent.join("assets/icons/trayicon.png"));
        paths.push(parent.join("../../../assets/icons/trayicon.png"));
        paths.push(parent.join("../../../../assets/icons/trayicon.png"));
    }
    paths.push(std::path::PathBuf::from("assets/icons/trayicon.png"));
    paths
}

fn fallback_pixels() -> TrayPixels {
    const SIZE: u32 = 22;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        rgba.extend_from_slice(&[0x58, 0x65, 0xF2, 0xFF]);
    }
    TrayPixels {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod sni {
    use super::{TrayPixels, tray_pixels};
    use anyhow::Result;
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::{MenuItem, StandardItem};
    use std::sync::Arc;

    type Callback = Arc<dyn Fn() + Send + Sync>;

    struct SniTray {
        icon: Vec<ksni::Icon>,
        on_show: Callback,
        on_check_updates: Callback,
        on_quit: Callback,
    }

    impl ksni::Tray for SniTray {
        fn id(&self) -> String {
            "mezon".into()
        }

        fn title(&self) -> String {
            "Mezon".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icon.clone()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Mezon".into(),
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.on_show)();
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                StandardItem {
                    label: "Show Mezon".into(),
                    activate: Box::new(|this: &mut Self| (this.on_show)()),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Check for updates".into(),
                    activate: Box::new(|this: &mut Self| (this.on_check_updates)()),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit Mezon".into(),
                    activate: Box::new(|this: &mut Self| (this.on_quit)()),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub struct MezonTray {
        _handle: Handle<SniTray>,
    }

    impl MezonTray {
        pub fn new(
            on_show: impl Fn() + Send + Sync + 'static,
            on_check_updates: impl Fn() + Send + Sync + 'static,
            on_quit: impl Fn() + Send + Sync + 'static,
        ) -> Result<Self> {
            let tray = SniTray {
                icon: vec![argb_icon(tray_pixels())],
                on_show: Arc::new(on_show),
                on_check_updates: Arc::new(on_check_updates),
                on_quit: Arc::new(on_quit),
            };
            let handle = tray.spawn()?;
            tracing::debug!("StatusNotifierItem tray registered");
            Ok(Self { _handle: handle })
        }
    }

    fn argb_icon(pixels: TrayPixels) -> ksni::Icon {
        let mut data = Vec::with_capacity(pixels.rgba.len());
        for px in pixels.rgba.chunks_exact(4) {
            data.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
        }
        ksni::Icon {
            width: pixels.width as i32,
            height: pixels.height as i32,
            data,
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use sni::MezonTray;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod desktop {
    use super::tray_pixels;
    use anyhow::Result;
    use std::sync::Arc;
    use tray_icon::{
        TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    const SHOW_ID: &str = "show";
    const UPDATE_ID: &str = "update";
    const QUIT_ID: &str = "quit";

    pub struct MezonTray {
        _icon: TrayIcon,
        stop_tx: crossbeam_channel::Sender<()>,
    }

    impl MezonTray {
        pub fn new(
            on_show: impl Fn() + Send + Sync + 'static,
            on_check_updates: impl Fn() + Send + Sync + 'static,
            on_quit: impl Fn() + Send + Sync + 'static,
        ) -> Result<Self> {
            let on_show = Arc::new(on_show);
            let on_check_updates = Arc::new(on_check_updates);
            let on_quit = Arc::new(on_quit);
            let receiver = MenuEvent::receiver().clone();
            let icon_receiver = TrayIconEvent::receiver().clone();

            let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);

            std::thread::spawn(move || {
                loop {
                    crossbeam_channel::select! {
                        recv(receiver) -> msg => match msg {
                            Ok(event) => match event.id().0.as_str() {
                                SHOW_ID => (on_show)(),
                                UPDATE_ID => (on_check_updates)(),
                                QUIT_ID => (on_quit)(),
                                _ => {}
                            },
                            Err(_) => break,
                        },
                        recv(icon_receiver) -> msg => match msg {
                            Ok(event) => {
                                if is_show_request(&event) {
                                    (on_show)();
                                }
                            }
                            Err(_) => break,
                        },
                        recv(stop_rx) -> _ => break,
                    }
                }
                tracing::debug!("Tray event-loop thread exiting");
            });

            let icon = build_menu_and_tray()?;

            tracing::debug!("System tray created");
            Ok(Self {
                _icon: icon,
                stop_tx,
            })
        }
    }

    #[cfg(target_os = "windows")]
    fn is_show_request(event: &TrayIconEvent) -> bool {
        use tray_icon::{MouseButton, MouseButtonState};

        matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
        )
    }

    #[cfg(not(target_os = "windows"))]
    fn is_show_request(_event: &TrayIconEvent) -> bool {
        false
    }

    fn build_menu_and_tray() -> Result<TrayIcon> {
        let menu = Menu::new();

        let item_show = MenuItem::with_id(SHOW_ID, "Show Mezon", true, None);
        let item_update = MenuItem::with_id(UPDATE_ID, "Check for updates", true, None);
        let item_quit = MenuItem::with_id(QUIT_ID, "Quit Mezon", true, None);

        menu.append(&item_show)?;
        menu.append(&item_update)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&item_quit)?;

        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Mezon")
            .with_icon(build_tray_icon());

        #[cfg(target_os = "windows")]
        let builder = builder.with_menu_on_left_click(false);

        Ok(builder.build()?)
    }

    fn build_tray_icon() -> tray_icon::Icon {
        #[cfg(target_os = "windows")]
        {
            if let Ok(handle) = crate::window_icon::load_tray_icon_handle() {
                return tray_icon::Icon::from_handle(handle.0 as isize);
            }
            tracing::warn!("Failed to load tray icon from embedded app.ico resource");
        }

        let pixels = tray_pixels();
        match tray_icon::Icon::from_rgba(pixels.rgba, pixels.width, pixels.height) {
            Ok(icon) => icon,
            Err(e) => {
                tracing::warn!("Failed to build tray icon from pixels: {e}");
                let fallback = super::fallback_pixels();
                tray_icon::Icon::from_rgba(fallback.rgba, fallback.width, fallback.height)
                    .expect("Failed to build fallback tray icon")
            }
        }
    }

    impl Drop for MezonTray {
        fn drop(&mut self) {
            let _ = self.stop_tx.send(());
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use desktop::MezonTray;
