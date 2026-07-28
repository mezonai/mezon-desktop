#[cfg(target_os = "macos")]
mod macos;

use anyhow::Result;
use std::sync::Arc;
#[cfg(not(target_os = "macos"))]
use tray_icon::menu::MenuEvent;
#[cfg(not(target_os = "macos"))]
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuItem},
};

pub const SHOW_ID: &str = "show";
pub const UPDATE_ID: &str = "update";
pub const QUIT_ID: &str = "quit";
pub const VOICE_MIC_ID: &str = "voice-mic";
pub const VOICE_CAMERA_ID: &str = "voice-camera";
pub const VOICE_SCREEN_ID: &str = "voice-screen";
pub const VOICE_LEAVE_ID: &str = "voice-leave";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayVoiceState {
    pub in_call: bool,
    pub channel_line: String,
    pub mic_enabled: bool,
    pub camera_enabled: bool,
    pub screen_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayVoiceAction {
    ToggleMic,
    ToggleCamera,
    ToggleScreen,
    Leave,
}

impl TrayVoiceAction {
    pub fn from_menu_id(id: &str) -> Option<Self> {
        match id {
            VOICE_MIC_ID => Some(Self::ToggleMic),
            VOICE_CAMERA_ID => Some(Self::ToggleCamera),
            VOICE_SCREEN_ID => Some(Self::ToggleScreen),
            VOICE_LEAVE_ID => Some(Self::Leave),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TrayUiAction {
    Show,
    Update,
    Quit,
    Voice(TrayVoiceAction),
}

#[cfg(target_os = "linux")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/icons/trayicon-linux.png"
));

#[cfg(not(target_os = "linux"))]
pub(crate) const TRAY_ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/icons/trayicon.png"
));

pub struct MezonTray {
    #[cfg(target_os = "macos")]
    macos: macos::MacosTray,
    #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
    _icon: TrayIcon,
    voice_state: TrayVoiceState,
    locale: String,
    stop_tx: crossbeam_channel::Sender<()>,
}

impl MezonTray {
    pub fn new(
        locale: impl Into<String>,
        on_show: impl Fn() + Send + Sync + 'static,
        on_check_updates: impl Fn() + Send + Sync + 'static,
        on_quit: impl Fn() + Send + Sync + 'static,
        on_voice_action: impl Fn(TrayVoiceAction) + Send + Sync + 'static,
    ) -> Result<Self> {
        let locale = locale.into();
        let on_show: Arc<dyn Fn() + Send + Sync> = Arc::new(on_show);
        let on_check_updates: Arc<dyn Fn() + Send + Sync> = Arc::new(on_check_updates);
        let on_quit: Arc<dyn Fn() + Send + Sync> = Arc::new(on_quit);
        let on_voice_action: Arc<dyn Fn(TrayVoiceAction) + Send + Sync> = Arc::new(on_voice_action);

        let voice_state = TrayVoiceState::default();

        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);

        #[cfg(target_os = "macos")]
        let (panel_action_tx, panel_action_rx) = crossbeam_channel::unbounded();
        #[cfg(target_os = "macos")]
        let macos = macos::MacosTray::new(voice_state.clone(), locale.clone(), panel_action_tx)?;

        spawn_tray_event_loop(
            stop_rx,
            Arc::clone(&on_show),
            Arc::clone(&on_check_updates),
            Arc::clone(&on_quit),
            on_voice_action,
            #[cfg(target_os = "macos")]
            panel_action_rx,
        );

        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        let icon = build_menu_and_tray(&voice_state, &locale)?;

        #[cfg(target_os = "linux")]
        {
            let menu_state = voice_state.clone();
            let menu_locale = locale.clone();
            let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
            std::thread::Builder::new()
                .name("mezon-tray-gtk".into())
                .spawn(move || {
                    if let Err(e) = gtk::init() {
                        let _ = ready_tx.send(Err(format!("gtk init failed: {e}")));
                        return;
                    }
                    match build_menu_and_tray(&menu_state, &menu_locale) {
                        Ok(tray) => {
                            LINUX_TRAY.with(|cell| *cell.borrow_mut() = Some(tray));
                            let _ = ready_tx.send(Ok(()));
                            gtk::main();
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                        }
                    }
                })
                .map_err(|e| anyhow::anyhow!("Failed to spawn GTK tray thread: {e}"))?;

            match ready_rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(anyhow::anyhow!(e)),
                Err(e) => return Err(anyhow::anyhow!("GTK tray thread exited before init: {e}")),
            }
        }

        tracing::debug!("System tray created");
        Ok(Self {
            #[cfg(target_os = "macos")]
            macos,
            #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
            _icon: icon,
            voice_state,
            locale,
            stop_tx,
        })
    }

    pub fn update_voice_state(&mut self, state: TrayVoiceState) {
        if self.voice_state == state {
            return;
        }
        self.voice_state = state.clone();
        #[cfg(target_os = "macos")]
        self.macos.update_voice_state(state);
        #[cfg(not(target_os = "macos"))]
        self.refresh_tray_menu();
    }

    pub fn update_locale(&mut self, locale: String) {
        if self.locale == locale {
            return;
        }
        self.locale = locale;
        #[cfg(target_os = "macos")]
        self.macos.update_locale(self.locale.clone());
        #[cfg(not(target_os = "macos"))]
        self.refresh_tray_menu();
    }

    #[cfg(not(target_os = "macos"))]
    fn refresh_tray_menu(&mut self) {
        #[cfg(windows)]
        apply_tray_updates(&self._icon, &self.voice_state, &self.locale);
        #[cfg(target_os = "linux")]
        run_on_gtk_main_thread({
            let state = self.voice_state.clone();
            let locale = self.locale.clone();
            move || apply_linux_tray_updates(&state, &locale)
        });
    }
}

fn spawn_tray_event_loop(
    stop_rx: crossbeam_channel::Receiver<()>,
    on_show: Arc<dyn Fn() + Send + Sync>,
    on_check_updates: Arc<dyn Fn() + Send + Sync>,
    on_quit: Arc<dyn Fn() + Send + Sync>,
    on_voice_action: Arc<dyn Fn(TrayVoiceAction) + Send + Sync>,
    #[cfg(target_os = "macos")] panel_action_rx: crossbeam_channel::Receiver<TrayUiAction>,
) {
    #[cfg(not(target_os = "macos"))]
    {
        let menu_receiver = MenuEvent::receiver().clone();
        std::thread::spawn(move || {
            loop {
                crossbeam_channel::select! {
                    recv(menu_receiver) -> msg => match msg {
                        Ok(event) => dispatch_tray_action(
                            event.id().0.as_str(),
                            &on_show,
                            &on_check_updates,
                            &on_quit,
                            &on_voice_action,
                        ),
                        Err(_) => break,
                    },
                    recv(stop_rx) -> _ => break,
                }
            }
            tracing::debug!("Tray event-loop thread exiting");
        });
    }

    #[cfg(target_os = "macos")]
    {
        let tray_receiver = tray_icon::TrayIconEvent::receiver().clone();
        std::thread::spawn(move || {
            loop {
                crossbeam_channel::select! {
                    recv(tray_receiver) -> msg => match msg {
                        Ok(event) => {
                            if let tray_icon::TrayIconEvent::Click {
                                button: tray_icon::MouseButton::Left,
                                button_state: tray_icon::MouseButtonState::Up,
                                ..
                            } = event
                            {
                                macos::toggle_panel();
                            }
                        }
                        Err(_) => break,
                    },
                    recv(panel_action_rx) -> msg => match msg {
                        Ok(action) => dispatch_tray_action_from_ui(
                            action,
                            &on_show,
                            &on_check_updates,
                            &on_quit,
                            &on_voice_action,
                        ),
                        Err(_) => break,
                    },
                    recv(stop_rx) -> _ => break,
                }
            }
            tracing::debug!("Tray event-loop thread exiting");
        });
    }
}

#[cfg(target_os = "macos")]
fn dispatch_tray_action_from_ui(
    action: TrayUiAction,
    on_show: &Arc<dyn Fn() + Send + Sync>,
    on_check_updates: &Arc<dyn Fn() + Send + Sync>,
    on_quit: &Arc<dyn Fn() + Send + Sync>,
    on_voice_action: &Arc<dyn Fn(TrayVoiceAction) + Send + Sync>,
) {
    match action {
        TrayUiAction::Show => (on_show)(),
        TrayUiAction::Update => (on_check_updates)(),
        TrayUiAction::Quit => (on_quit)(),
        TrayUiAction::Voice(voice_action) => (on_voice_action)(voice_action),
    }
}

#[cfg(not(target_os = "macos"))]
fn dispatch_tray_action(
    action_id: &str,
    on_show: &Arc<dyn Fn() + Send + Sync>,
    on_check_updates: &Arc<dyn Fn() + Send + Sync>,
    on_quit: &Arc<dyn Fn() + Send + Sync>,
    on_voice_action: &Arc<dyn Fn(TrayVoiceAction) + Send + Sync>,
) {
    match action_id {
        SHOW_ID => (on_show)(),
        UPDATE_ID => (on_check_updates)(),
        QUIT_ID => (on_quit)(),
        id => {
            if let Some(action) = TrayVoiceAction::from_menu_id(id) {
                (on_voice_action)(action);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn build_menu_and_tray(state: &TrayVoiceState, locale: &str) -> Result<TrayIcon> {
    let menu = build_voice_menu(state, locale)?;
    let icon = build_tray_icon_rgba(false).unwrap_or_else(|_| build_fallback_icon());

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Mezon")
        .with_icon(icon)
        .build()
        .map_err(Into::into)
}

#[cfg(not(target_os = "macos"))]
fn build_voice_menu(state: &TrayVoiceState, locale: &str) -> Result<Menu> {
    let menu = Menu::new();

    if state.in_call {
        let status = if state.camera_enabled {
            mezon_i18n::t(locale, "channelVoice.videoConnected")
        } else {
            mezon_i18n::t(locale, "channelVoice.voiceConnected")
        };
        menu.append(&MenuItem::with_id("voice-status", status, false, None))?;

        if !state.channel_line.is_empty() {
            menu.append(&MenuItem::with_id(
                "voice-channel",
                &state.channel_line,
                false,
                None,
            ))?;
        }

        menu.append(&tray_icon::menu::PredefinedMenuItem::separator())?;

        let mic_label = if state.mic_enabled {
            mezon_i18n::t(locale, "channelVoice.turnOffMicrophone")
        } else {
            mezon_i18n::t(locale, "channelVoice.turnOnMicrophone")
        };
        menu.append(&MenuItem::with_id(VOICE_MIC_ID, mic_label, true, None))?;

        let camera_label = if state.camera_enabled {
            mezon_i18n::t(locale, "channelVoice.turnOffCamera")
        } else {
            mezon_i18n::t(locale, "channelVoice.turnOnCamera")
        };
        menu.append(&MenuItem::with_id(
            VOICE_CAMERA_ID,
            camera_label,
            true,
            None,
        ))?;

        let screen_label = if state.screen_enabled {
            mezon_i18n::t(locale, "channelVoice.stopScreenShare")
        } else {
            mezon_i18n::t(locale, "channelVoice.shareYourScreen")
        };
        menu.append(&MenuItem::with_id(
            VOICE_SCREEN_ID,
            screen_label,
            true,
            None,
        ))?;
        menu.append(&MenuItem::with_id(
            VOICE_LEAVE_ID,
            mezon_i18n::t(locale, "tray.leaveVoice"),
            true,
            None,
        ))?;
        menu.append(&tray_icon::menu::PredefinedMenuItem::separator())?;
    }

    menu.append(&MenuItem::with_id(
        SHOW_ID,
        mezon_i18n::t(locale, "tray.showMezon"),
        true,
        None,
    ))?;
    menu.append(&MenuItem::with_id(
        UPDATE_ID,
        mezon_i18n::t(locale, "tray.checkForUpdates"),
        true,
        None,
    ))?;
    menu.append(&tray_icon::menu::PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id(
        QUIT_ID,
        mezon_i18n::t(locale, "tray.quit"),
        true,
        None,
    ))?;

    Ok(menu)
}

#[cfg(target_os = "linux")]
thread_local! {
    static LINUX_TRAY: std::cell::RefCell<Option<TrayIcon>> = const { std::cell::RefCell::new(None) };
}

#[cfg(not(target_os = "macos"))]
fn apply_tray_updates(tray: &TrayIcon, state: &TrayVoiceState, locale: &str) {
    let Ok(menu) = build_voice_menu(state, locale) else {
        return;
    };
    let _ = tray.set_menu(Some(Box::new(menu)));
    if let Ok(icon) = build_tray_icon_rgba(state.in_call) {
        let _ = tray.set_icon(Some(icon));
    }
}

#[cfg(target_os = "linux")]
fn apply_linux_tray_updates(state: &TrayVoiceState, locale: &str) {
    LINUX_TRAY.with(|cell| {
        let mut tray_ref = cell.borrow_mut();
        let Some(tray) = tray_ref.as_mut() else {
            return;
        };
        apply_tray_updates(tray, state, locale);
    });
}

#[cfg(target_os = "linux")]
fn run_on_gtk_main_thread(f: impl FnOnce() + Send + 'static) {
    gtk::glib::MainContext::default().invoke(f);
}

impl Drop for MezonTray {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

pub(crate) fn load_base_icon_rgba(fallback_bytes: &[u8]) -> Result<image::RgbaImage> {
    for path in icon_search_paths() {
        if path.exists() {
            match image::open(&path) {
                Ok(img) => return Ok(img.into_rgba8()),
                Err(error) => {
                    tracing::warn!("Failed to load tray icon from {}: {error}", path.display());
                }
            }
        }
    }
    Ok(image::load_from_memory(fallback_bytes)?.into_rgba8())
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

#[cfg(not(target_os = "macos"))]
fn icon_to_tray_icon(mut img: image::RgbaImage, in_call: bool) -> Result<tray_icon::Icon> {
    if in_call {
        draw_call_indicator(&mut img);
    }
    let (width, height) = img.dimensions();
    Ok(tray_icon::Icon::from_rgba(img.into_raw(), width, height)?)
}

#[cfg(not(target_os = "macos"))]
fn build_tray_icon_rgba(in_call: bool) -> Result<tray_icon::Icon> {
    for path in &icon_search_paths() {
        if path.exists() {
            if let Ok(img) = image::open(path) {
                match icon_to_tray_icon(img.into_rgba8(), in_call) {
                    Ok(icon) => {
                        tracing::debug!("Loaded tray icon from {}", path.display());
                        return Ok(icon);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load tray icon from {}: {e}", path.display());
                    }
                }
            }
        }
    }
    icon_to_tray_icon(
        image::load_from_memory(TRAY_ICON_BYTES)?.into_rgba8(),
        in_call,
    )
}

#[cfg(not(target_os = "macos"))]
fn build_fallback_icon() -> tray_icon::Icon {
    const SIZE: u32 = 22;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        rgba.extend_from_slice(&[0x58, 0x65, 0xF2, 0xFF]);
    }
    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).expect("Failed to build fallback tray icon")
}

pub(crate) fn draw_call_indicator(icon: &mut image::RgbaImage) {
    let (width, height) = icon.dimensions();
    if width == 0 || height == 0 {
        return;
    }

    let center_x = width as i32 - 4;
    let center_y = 4;
    let radius = 3;
    let orange = [255u8, 149, 0, 255];

    for y in (center_y - radius)..=(center_y + radius) {
        for x in (center_x - radius)..=(center_x + radius) {
            if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                continue;
            }
            let dx = x - center_x;
            let dy = y - center_y;
            if dx * dx + dy * dy <= radius * radius {
                icon.put_pixel(x as u32, y as u32, image::Rgba(orange));
            }
        }
    }
}
