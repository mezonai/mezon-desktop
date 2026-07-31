#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod mcp;

use anyhow::Result;
use futures::StreamExt;
use gpui::{App, AppContext, AsyncApp, Bounds, Entity, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;
use mezon_client::{AppApi, MezonClient, TransportClient};
use mezon_native::instance::SingleInstance;
use mezon_store::{AppConfig, AuthState, Settings};
use mezon_ui::app::main_window::{activate_main_window, register_main_window};
use mezon_ui::app::window_controls::{linux_app_id, main_window_decorations, window_title_options};
use mezon_ui::{RootView, TitleBar, init as init_ui};
use std::borrow::Cow;
use std::sync::Arc;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

const IMAGE_CLIENT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

#[cfg(feature = "tracy")]
#[global_allocator]
static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
    tracy_client::ProfiledAllocator::new(std::alloc::System, 16);

#[cfg(feature = "tracy")]
#[derive(Default)]
struct TracyConfig {
    fmt: tracing_subscriber::fmt::format::DefaultFields,
}

#[cfg(feature = "tracy")]
impl tracing_tracy::Config for TracyConfig {
    type Formatter = tracing_subscriber::fmt::format::DefaultFields;

    fn formatter(&self) -> &Self::Formatter {
        &self.fmt
    }

    fn stack_depth(&self, _: &tracing::Metadata<'_>) -> u16 {
        16
    }

    fn format_fields_in_zone_name(&self) -> bool {
        true
    }

    fn on_error(&self, client: &tracy_client::Client, error: &'static str) {
        client.color_message(error, 0xFF000000, 0);
    }
}

#[cfg(target_os = "linux")]
fn configure_linux_session() {
    if std::env::var_os("DISPLAY").is_none() {
        return;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty()) {
        mezon_store::record_wayland_session();
    }
    unsafe {
        std::env::set_var("GDK_BACKEND", "x11");
        std::env::remove_var("WAYLAND_DISPLAY");
        let ctype = std::env::var("LC_CTYPE").unwrap_or_default();
        if ctype.is_empty() || ctype == "C" || ctype == "POSIX" {
            let lang = std::env::var("LANG").unwrap_or_default();
            let fallback = if !lang.is_empty() && lang != "C" && lang != "POSIX" {
                lang
            } else {
                "en_US.UTF-8".into()
            };
            std::env::set_var("LC_CTYPE", fallback);
        }
    }
    ensure_fcitx_xim_on_the_spot();
}

#[cfg(target_os = "linux")]
fn ensure_fcitx_xim_on_the_spot() {
    let uses_fcitx = std::env::var("XMODIFIERS")
        .map(|v| v.contains("fcitx"))
        .unwrap_or(false);
    if !uses_fcitx {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let conf_dir = std::path::PathBuf::from(home).join(".config/fcitx5/conf");
    let conf_path = conf_dir.join("xim.conf");
    let existing = std::fs::read_to_string(&conf_path).unwrap_or_default();
    let already_on = existing
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("UseOnTheSpot=True"));
    if already_on {
        return;
    }
    if let Err(error) = std::fs::create_dir_all(&conf_dir) {
        tracing::warn!("fcitx5 XIM OnTheSpot: create conf dir failed: {error}");
        return;
    }
    let next = if existing.trim().is_empty() {
        "UseOnTheSpot=True\n".to_string()
    } else if existing
        .lines()
        .any(|line| line.trim().starts_with("UseOnTheSpot="))
    {
        existing
            .lines()
            .map(|line| {
                if line.trim().starts_with("UseOnTheSpot=") {
                    "UseOnTheSpot=True"
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    } else {
        format!("{}\nUseOnTheSpot=True\n", existing.trim_end())
    };
    if let Err(error) = std::fs::write(&conf_path, next) {
        tracing::warn!("fcitx5 XIM OnTheSpot: write conf failed: {error}");
        return;
    }
    tracing::info!(
        "Enabled fcitx5 XIM UseOnTheSpot for inline preedit; restarting fcitx5 once \
         (the xim addon only reads this setting at startup)"
    );
    match std::process::Command::new("fcitx5").args(["-rd"]).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            std::thread::sleep(std::time::Duration::from_millis(1200));
        }
        Err(error) => tracing::debug!("fcitx5 not available for restart: {error}"),
    }
}

#[cfg(target_os = "linux")]
fn log_linux_session_notes() {
    match std::env::var("MEZON_LINUX_SESSION")
        .ok()
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("wayland") => {
            tracing::warn!("MEZON_LINUX_SESSION=wayland: channel app webviews require X11/XWayland")
        }
        Some(other) if other != "x11" => {
            tracing::warn!("Unknown MEZON_LINUX_SESSION={other}; expected x11 or wayland")
        }
        _ => {}
    }
}

fn main() -> Result<()> {
    #[cfg(target_os = "linux")]
    configure_linux_session();

    install_panic_hook();
    if let Some(exit_code) = mezon_cli::try_run(std::env::args())? {
        std::process::exit(exit_code);
    }

    init_logging();

    #[cfg(target_os = "linux")]
    log_linux_session_notes();

    tracing::info!("Starting Mezon desktop app v{}", env!("CARGO_PKG_VERSION"));

    // Check if a mezonapp:// deep link URL was passed as argv[1].
    let deep_link_url: Option<String> = std::env::args()
        .nth(1)
        .filter(|a| a.starts_with("mezonapp://"));

    // Single instance guard — forward deep link URL to an existing instance if running.
    let lock_result = match deep_link_url.as_deref() {
        Some(url) => SingleInstance::try_acquire_or_forward(url)?,
        None => SingleInstance::try_acquire()?,
    };

    match lock_result {
        None => {
            tracing::info!("Another instance is already running — exiting");
            return Ok(());
        }
        Some(lock) => run_app(lock, deep_link_url),
    }

    Ok(())
}

/// Directory for rotated log files (alongside the app's config dir).
fn log_dir() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("mezon")
        .join("logs")
}

/// Initialise tracing to stdout **and** a daily-rotated log file. Uses a blocking file writer
/// (not `non_blocking`) so a panic is flushed to disk before the process aborts.
fn init_logging() {
    let default_filter = if cfg!(debug_assertions) {
        "mezon=debug,info"
    } else {
        "mezon=info,warn"
    };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let dir = log_dir();
    let dir_err = std::fs::create_dir_all(&dir).err();
    let file_layer = dir_err.is_none().then(|| {
        let file_appender = tracing_appender::rolling::daily(&dir, "mezon.log");
        fmt::layer().with_ansi(false).with_writer(file_appender)
    });

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(std::io::stdout))
        .with(file_layer);

    #[cfg(feature = "tracy")]
    let subscriber = subscriber.with(tracing_tracy::TracyLayer::new(TracyConfig::default()));

    subscriber.init();

    if let Some(e) = dir_err {
        tracing::warn!(
            "File logging disabled (cannot create {}): {e}",
            dir.display()
        );
    }
}

/// Log panics (location + backtrace) before delegating to the default hook, so a crash leaves
/// a record in the log file instead of vanishing on stderr (invisible in a bundled `.app`).
/// Distinguishes a hard hang from a crash in field reports: a foreground task
/// bumps a heartbeat every second; a detached OS thread checks it and logs
/// (synchronously, so it survives a later abort) when the foreground thread has
/// not run for [`WATCHDOG_STALL_SECS`]. A crashed process stops both sides;
/// a wedged UI thread keeps the watchdog logging while the app is frozen.
fn install_foreground_watchdog(cx: &mut gpui::App) {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    const WATCHDOG_STALL_SECS: u64 = 20;

    static START: OnceLock<Instant> = OnceLock::new();
    static HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
    let start = *START.get_or_init(Instant::now);

    const WATCHDOG_HEARTBEAT_SECS: u64 = 5;

    let timer_executor = cx.background_executor().clone();
    cx.spawn(async move |_| {
        loop {
            timer_executor
                .timer(Duration::from_secs(WATCHDOG_HEARTBEAT_SECS))
                .await;
            HEARTBEAT_MS.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
    })
    .detach();

    std::thread::Builder::new()
        .name("mezon-fg-watchdog".into())
        .spawn(move || {
            let mut stale_checks = 0u32;
            let mut reported_stall = false;
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let now_ms = start.elapsed().as_millis() as u64;
                let last = HEARTBEAT_MS.load(Ordering::Relaxed);
                let stalled_ms = now_ms.saturating_sub(last);
                if stalled_ms >= WATCHDOG_STALL_SECS * 1000 {
                    // Two consecutive stale checks are required so a wake from
                    // system sleep (heartbeat catches up within one interval)
                    // never logs a false stall; one line per stall episode.
                    stale_checks += 1;
                    if stale_checks >= 2 && !reported_stall {
                        reported_stall = true;
                        let line = format!(
                            "[watchdog] foreground thread has not run for {}s",
                            stalled_ms / 1000
                        );
                        eprintln!("{line}");
                        tracing::error!("{line}");
                    }
                } else {
                    stale_checks = 0;
                    if reported_stall {
                        reported_stall = false;
                        tracing::warn!("[watchdog] foreground thread recovered after a stall");
                    }
                }
            }
        })
        .map_err(|e| tracing::error!("failed to start the foreground watchdog thread: {e}"))
        .ok();
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic at {location}: {message}\n{backtrace}");
        default_hook(info);
    }));
}

fn run_app(lock: SingleInstance, initial_url: Option<String>) {
    let settings = Settings::load_sync();

    tracing::debug!(
        "Settings: theme={}, zoom={}, auto_start={}",
        settings.theme,
        settings.zoom_factor,
        settings.auto_start
    );

    // ── Determine initial auth state from keychain ────────────────────────────
    let app_config = Arc::new(AppConfig::from_env());
    tracing::debug!(
        "App config: rest={}:{} secure={} (gw host; api_host={})",
        app_config.client_host(),
        app_config.client_port(),
        app_config.api_secure,
        app_config.api_host,
    );
    mezon_updater::configure_endpoints(mezon_updater::UpdaterEndpoints {
        manifest_base_url: app_config.update_url.clone(),
        download_url: format!("{}/download", app_config.domain_url.trim_end_matches('/')),
    });
    let client = MezonClient::new(
        app_config.client_host(),
        app_config.client_port(),
        app_config.api_secure,
        &app_config.api_key,
    );
    let client = Arc::new(client);
    let transport = Arc::new(TransportClient::new(String::new()));
    let api = Arc::new(AppApi::new(
        transport.clone(),
        app_config.base_img_url.clone(),
    ));
    let initial_auth_state = mezon_store::resolve_initial_auth_state();

    // Subscribe to screen lock/unlock events.
    let (wake_tx, mut wake_rx) = futures::channel::mpsc::unbounded::<()>();
    mezon_native::power::subscribe(Box::new(move |event| match event {
        mezon_native::power::PowerEvent::ScreenLocked => {
            tracing::info!("Screen locked");
        }
        mezon_native::power::PowerEvent::ScreenUnlocked => {
            tracing::info!("Screen unlocked");
            let _ = wake_tx.unbounded_send(());
        }
    }));

    let (url_tx, mut url_rx) = futures::channel::mpsc::unbounded::<String>();
    let (mcp_cmd_tx, mcp_cmd_rx) = futures::channel::mpsc::unbounded::<mezon_mcp::McpCommand>();

    let mcp_runtime = match mcp::McpRuntime::start(api.clone(), mcp_cmd_tx) {
        Ok(runtime) => Some((runtime, mcp_cmd_rx)),
        Err(error) => {
            tracing::warn!("MCP control server disabled: {error}");
            None
        }
    };

    let app_config_handle = app_config.clone();
    let app = application()
        .with_http_client(Arc::new(
            mezon_client::image_disk_cache::DiskImageCacheClient::new(
                mezon_client::transport_runtime::new_http_client_with_user_agent(
                    IMAGE_CLIENT_USER_AGENT,
                ),
                app_config
                    .media_origins()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                app_config.upload_img_url.clone(),
                app_config.base_img_url.clone(),
            ),
        ))
        .with_assets(mezon_ui::util::assets::Assets);

    {
        let tx = url_tx.clone();
        app.on_open_urls(move |urls| {
            for url in urls {
                if url.starts_with("mezonapp://") {
                    let _ = tx.unbounded_send(url);
                }
            }
        });
    }

    #[cfg(target_os = "macos")]
    app.on_reopen(show_main_window);

    app.run(move |cx: &mut App| {
        tracing::debug!("App started");
        install_foreground_watchdog(cx);

        #[cfg(target_os = "linux")]
        {
            if let Err(error) = mezon_webview::init_gtk() {
                tracing::error!("gtk init failed: {error:#}");
            }
            cx.spawn(async move |async_cx| {
                let foreground = async_cx.foreground_executor().clone();
                loop {
                    async_cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    foreground
                        .spawn(async move {
                            mezon_webview::pump_gtk_events();
                        })
                        .detach();
                }
            })
            .detach();
        }

        #[cfg(target_os = "windows")]
        {
            cx.spawn(async move |async_cx| {
                loop {
                    async_cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    mezon_ui::channel_app::process_pending_windows_webviews(async_cx);
                }
            })
            .detach();
        }

        #[cfg(target_os = "linux")]
        cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);

        // Register gg sans font (TTFs pre-decompressed by build.rs)
        let gg_sans_paths: &[(&[u8], &str)] = &[
            (
                include_bytes!(concat!(env!("OUT_DIR"), "/ggsans-Normal.ttf")),
                "Normal",
            ),
            (
                include_bytes!(concat!(env!("OUT_DIR"), "/ggsans-Medium.ttf")),
                "Medium",
            ),
            (
                include_bytes!(concat!(env!("OUT_DIR"), "/ggsans-Semibold.ttf")),
                "Semibold",
            ),
            (
                include_bytes!(concat!(env!("OUT_DIR"), "/ggsans-Bold.ttf")),
                "Bold",
            ),
            (
                include_bytes!(concat!(env!("OUT_DIR"), "/ggsans-ExtraBold.ttf")),
                "ExtraBold",
            ),
        ];
        let fonts: Vec<Cow<'static, [u8]>> = gg_sans_paths
            .iter()
            .map(|(data, _)| Cow::Borrowed(*data))
            .collect();
        if !fonts.is_empty() {
            if let Err(e) = cx.text_system().add_fonts(fonts) {
                tracing::error!("Failed to register gg sans fonts: {e}");
            } else {
                tracing::info!("Registered gg sans font ({} weights)", gg_sans_paths.len());
            }
        }

        init_ui(cx);

        AppConfig::init_global(app_config_handle, cx);

        let wake_task = cx.spawn(async move |cx: &mut AsyncApp| {
            while wake_rx.next().await.is_some() {
                cx.update(|cx| {
                    if let Some(store) = mezon_store::AutoUpdateStore::try_global(cx) {
                        store.update(cx, |store, cx| store.check(false, cx));
                    }
                });
            }
        });
        cx.set_global(WakeCheckTaskGlobal(wake_task));

        mezon_ui::theme::set_theme(mezon_ui::theme::resolve_theme(&settings.theme), cx);

        if std::env::var("MEZON_DEV_GALLERY").is_ok() {
            open_dev_gallery_window(cx);
            return;
        }

        {
            let auto_start = settings.auto_start;
            cx.background_executor()
                .spawn(async move {
                    mezon_native::autostart::sync_auto_start(auto_start);
                    mezon_native::deep_link::register_deep_link_scheme();
                    mezon_native::notifications::init();
                })
                .detach();
        }

        // Listen for deep link URLs forwarded from secondary instances.
        {
            let tx = url_tx.clone();
            lock.listen_for_urls(move |url| {
                let _ = tx.unbounded_send(url);
            });
        }

        cx.set_global(SingleInstanceGlobal(lock));

        // If we were launched with a deep link, inject it immediately.
        if let Some(url) = initial_url {
            let _ = url_tx.unbounded_send(url);
        }

        let settings_entity = cx.new(|_| settings.clone());
        mezon_store::Settings::init_global(&settings_entity, cx);

        let settings_for_mcp = settings_entity.clone();
        let (auth_state_handle, window_handle) = open_main_window(
            cx,
            &settings,
            settings_entity,
            client.clone(),
            api.clone(),
            transport.clone(),
            initial_auth_state,
        );

        let _mcp_runtime = if let Some((runtime, mcp_cmd_rx)) = mcp_runtime {
            mcp::McpRuntime::spawn_ui_bridge(
                cx,
                auth_state_handle.clone(),
                settings_for_mcp,
                mcp_cmd_rx,
            );
            register_mcp_server_hooks(cx, runtime.controller());
            Some(runtime)
        } else {
            None
        };

        register_main_window(window_handle, cx);

        #[cfg(target_os = "windows")]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(Some(RawWindowHandle::Win32(win32))) = cx
                .update_window(window_handle, |_, window, _| {
                    window.window_handle().ok().map(|handle| handle.as_raw())
                })
            {
                let hwnd = win32.hwnd.get();
                mezon_native::badge::set_main_window_hwnd(hwnd);
                mezon_native::window_icon::apply_dpi_aware_icons(hwnd);
            }
        }

        install_badge_bridge(cx);

        let deep_link_task = {
            let auth_state = auth_state_handle.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                while let Some(url) = url_rx.next().await {
                    if url == mezon_native::instance::ACTIVATE_MESSAGE {
                        tracing::debug!("Relaunched while running — showing main window");
                        cx.update(activate_main_window);
                        continue;
                    }
                    tracing::info!(
                        "Received deep link: {}",
                        url.split(['?', '#']).next().unwrap_or_default()
                    );
                    cx.update(activate_main_window);
                    if url.starts_with("mezonapp://callback") {
                        let flow_pending = cx.update(|cx| {
                            matches!(
                                auth_state.read(cx),
                                AuthState::NotAuthenticated | AuthState::AwaitingCallback
                            )
                        });
                        if !flow_pending {
                            tracing::warn!(
                                "Ignoring OAuth callback deep link: no login flow pending"
                            );
                            continue;
                        }
                        if let Some(token) = mezon_store::token_from_oauth_callback_url(&url) {
                            let client = cx
                                .update(|cx| mezon_store::LoginStore::global(cx).read(cx).client());
                            let auth = auth_state.clone();
                            match client.authenticate_mezon(&token).await {
                                Ok(session) if !session.token.is_empty() => {
                                    let session_kc = session.clone();
                                    cx.background_executor()
                                        .spawn(async move {
                                            if let Err(e) =
                                                mezon_store::LoginStore::persist_session(
                                                    &session_kc,
                                                )
                                            {
                                                tracing::warn!(
                                                    "Failed to save OAuth session: {e}"
                                                );
                                            }
                                        })
                                        .detach();
                                    cx.update(|cx| {
                                        auth.update(cx, |state, cx| {
                                            *state = AuthState::Connecting(session.clone());
                                            cx.notify();
                                        });
                                        if !session.id_token.is_empty() {
                                            mezon_store::WalletStore::global(cx).update(
                                                cx,
                                                |wallet, cx| {
                                                    wallet
                                                        .fetch_zk_proofs_after_login(&session, cx);
                                                },
                                            );
                                        }
                                        mezon_ui::router::replace(
                                            cx,
                                            mezon_ui::router::Route::Chat,
                                        );
                                    });
                                }
                                Ok(_) => {
                                    tracing::warn!(
                                        "OAuth callback returned a session without a token"
                                    );
                                    cx.update(|cx| {
                                        auth.update(cx, |state, cx| {
                                            *state = AuthState::NotAuthenticated;
                                            cx.notify();
                                        });
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!("OAuth callback token exchange failed: {e}");
                                    cx.update(|cx| {
                                        auth.update(cx, |state, cx| {
                                            *state = AuthState::NotAuthenticated;
                                            cx.notify();
                                        });
                                    });
                                }
                            }
                        } else {
                            cx.update(|cx| {
                                auth_state.update(cx, |state, cx| {
                                    *state = AuthState::AwaitingCallback;
                                    cx.notify();
                                });
                                mezon_ui::router::replace(cx, mezon_ui::router::Route::Chat);
                            });
                        }
                    } else {
                        cx.update(|cx| {
                            if let Some(route) = mezon_ui::router::parse_link(&url) {
                                mezon_ui::router::navigate(cx, route);
                            }
                        });
                    }
                }
            })
        };

        let tray = setup_tray(cx);
        mezon_ui::app::window_controls::set_runs_in_background(cx, tray.is_some());
        if let Some((tray, tray_tasks)) = tray {
            cx.set_global(TrayGlobal(tray));
            cx.set_global(TrayTasksGlobal {
                _deep_link: deep_link_task,
                _tray_tasks: tray_tasks,
            });
        } else {
            deep_link_task.detach();
        }

        cx.activate(true);
    });
}

fn open_dev_gallery_window(cx: &mut App) {
    let options = WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Mezon Component Gallery".into()),
            appears_transparent: false,
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(900.0), px(800.0)),
            cx,
        ))),
        app_id: linux_app_id(),
        ..Default::default()
    };

    cx.open_window(options, |window, cx| {
        cx.new(|cx| mezon_ui::DevGallery::new(window, cx))
    })
    .unwrap_or_else(|e| {
        tracing::error!("Failed to open dev gallery window: {e}");
        std::process::exit(1);
    });

    cx.activate(true);
}

fn open_main_window(
    cx: &mut App,
    settings: &Settings,
    settings_entity: Entity<Settings>,
    client: Arc<MezonClient>,
    api: Arc<AppApi>,
    transport: Arc<TransportClient>,
    initial_auth: AuthState,
) -> (Entity<AuthState>, gpui::AnyWindowHandle) {
    let window_bounds = if let Some([x, y, w, h]) = settings.window_bounds {
        WindowBounds::Windowed(Bounds {
            origin: gpui::point(px(x as f32), px(y as f32)),
            size: size(px(w as f32), px(h as f32)),
        })
    } else {
        WindowBounds::Windowed(Bounds::centered(
            None,
            size(
                px(mezon_ui::MAIN_WINDOW_DEFAULT_WIDTH),
                px(mezon_ui::MAIN_WINDOW_DEFAULT_HEIGHT),
            ),
            cx,
        ))
    };

    let options = WindowOptions {
        titlebar: Some(window_title_options()),
        window_bounds: Some(window_bounds),
        window_min_size: Some(size(
            px(mezon_ui::MAIN_WINDOW_MIN_WIDTH),
            px(mezon_ui::MAIN_WINDOW_MIN_HEIGHT),
        )),
        kind: gpui::WindowKind::Normal,
        focus: true,
        show: true,
        window_decorations: main_window_decorations(),
        app_id: linux_app_id(),
        ..Default::default()
    };

    let auth_state = cx.new(|_| initial_auth);
    mezon_store::AutoUpdateStore::init(mezon_store::AppConfig::global(cx).update_url.clone(), cx);
    let title_bar = cx.new(|cx| TitleBar::new(settings_entity.clone(), cx));

    mezon_store::LoginStore::init(client, api.clone(), auth_state.clone(), cx);
    mezon_store::RealtimeDispatch::init(api.clone(), cx);
    mezon_store::ConnectionStore::init(transport, api.clone(), auth_state.clone(), cx);
    mezon_store::ClanList::init(api.clone(), cx);
    mezon_store::ChannelList::init(api.clone(), cx);
    mezon_store::ChannelSettingsStore::init(api.clone(), cx);
    mezon_store::DirectMessageStore::init(api.clone(), cx);
    mezon_store::FriendStore::init(api.clone(), cx);
    mezon_store::ActivityStore::init(api.clone(), cx);
    mezon_store::BadgeService::init(auth_state.clone(), cx);
    mezon_store::MessagesStore::init(api.clone(), cx);
    mezon_store::ThreadsStore::init(api.clone(), cx);
    mezon_store::MessageSearchStore::init(api.clone(), cx);
    mezon_store::AuditLogStore::init(api.clone(), cx);
    mezon_store::InboxStore::init(api.clone(), cx);
    mezon_store::TopicsStore::init(api.clone(), cx);
    mezon_store::TopicBadgeStore::init(api.clone(), auth_state.clone(), cx);
    mezon_store::PinnedMessagesStore::init(api.clone(), cx);
    mezon_store::CanvasStore::init(api.clone(), cx);
    mezon_store::PresenceStore::init(api.clone(), cx);
    mezon_store::StreamStore::init(api.clone(), cx);
    mezon_store::VoiceStore::init(api.clone(), cx);
    mezon_store::ClanMembersStore::init(api.clone(), cx);
    mezon_store::EmojiStore::init(api.clone(), cx);
    mezon_store::StickerStore::init(api.clone(), cx);
    mezon_store::GifStore::init(cx);
    mezon_store::ChannelMembersStore::init(api.clone(), cx);
    mezon_store::ChannelPermissionsStore::init(api.clone(), cx);
    mezon_store::ChannelUsersStore::init(api.clone(), cx);
    mezon_store::ChannelRolePermissionsStore::init(api.clone(), cx);
    mezon_store::GroupMembersStore::init(api.clone(), cx);
    mezon_store::UsersByUserStore::init(api.clone(), cx);
    mezon_store::RolesStore::init(api.clone(), cx);
    mezon_store::WebhookStore::init(api.clone(), cx);
    mezon_store::EventsStore::init(api.clone(), cx);
    mezon_store::GalleryStore::init(api.clone(), cx);
    mezon_store::ChannelMediaStore::init(api.clone(), cx);
    mezon_store::FilesStore::init(api.clone(), cx);
    mezon_store::PermissionStore::init(api.clone(), auth_state.clone(), cx);
    mezon_store::NotificationSettingStore::init(api.clone(), auth_state.clone(), cx);
    mezon_store::NotificationPushStore::init(api.clone(), auth_state.clone(), cx);
    mezon_store::ClanLoadScheduler::init(cx);
    mezon_store::QuickMenuStore::init(api.clone(), cx);
    mezon_store::ComposeStore::init(cx);
    mezon_store::WalletStore::init(auth_state.clone(), cx);
    mezon_ui::WalletToastBridge::init(cx);
    mezon_ui::chat::channel_settings::channel_acl::init(api.clone(), cx);
    mezon_store::AccountStore::init(api, cx);

    let platform_store = mezon_store::PlatformStore::init(cx);
    mezon_store::PlatformStore::set_open_url(
        &platform_store,
        std::sync::Arc::new(|url: &str| mezon_native::open_url(url)),
        cx,
    );
    mezon_store::PlatformStore::set_save_attachment(
        &platform_store,
        std::sync::Arc::new(|url: &str, filename: &str| save_attachment(url, filename)),
        cx,
    );
    mezon_store::PlatformStore::set_notifier(
        &platform_store,
        std::sync::Arc::new(|n: mezon_store::DesktopNotification| {
            mezon_native::notifications::show(&mezon_native::notifications::Notification {
                title: n.title,
                body: n.body,
                channel_id: n.channel_id,
                clan_id: n.clan_id,
                link: n.link,
                icon_path: n.icon_path,
            });
        }),
        cx,
    );
    mezon_store::PlatformStore::set_cli_install(
        &platform_store,
        mezon_store::CliInstallHooks {
            visible: std::sync::Arc::new(mezon_native::cli_install::menu_visible),
            installed: std::sync::Arc::new(mezon_native::cli_install::is_installed),
            toggle: std::sync::Arc::new(mezon_native::cli_install::toggle),
        },
        cx,
    );
    mezon_store::PlatformStore::set_notification_permit(
        &platform_store,
        std::sync::Arc::new(mezon_native::notifications::notifications_permitted),
        cx,
    );
    mezon_store::PlatformStore::set_current_location(
        &platform_store,
        std::sync::Arc::new(mezon_native::location::current_location),
        cx,
    );

    {
        let (click_tx, mut click_rx) =
            futures::channel::mpsc::unbounded::<mezon_native::notifications::NotificationTarget>();
        mezon_native::notifications::set_click_handler(Box::new(move |target| {
            let _ = click_tx.unbounded_send(target);
        }));
        let task = cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(target) = click_rx.next().await {
                cx.update(|cx| {
                    activate_main_window(cx);
                    tracing::debug!(
                        clan_id = ?target.clan_id,
                        channel_id = %target.channel_id,
                        link = ?target.link,
                        "notification clicked"
                    );
                    match notification_route(&target) {
                        Some(route) => mezon_ui::router::navigate(cx, route),
                        None => tracing::warn!("notification click with unroutable target"),
                    }
                });
            }
        });
        cx.set_global(NotificationClickGlobal(task));
    }

    let audio_store = mezon_store::AudioStore::init(cx);
    mezon_store::AudioStore::set_device_enumerator(
        &audio_store,
        std::sync::Arc::new(|| {
            let inputs = mezon_native::audio::enumerate_input_devices()
                .into_iter()
                .map(|d| mezon_store::AudioDeviceInfo {
                    id: d.id,
                    name: d.name,
                })
                .collect::<Vec<_>>();
            let outputs = mezon_native::audio::enumerate_output_devices()
                .into_iter()
                .map(|d| mezon_store::AudioDeviceInfo {
                    id: d.id,
                    name: d.name,
                })
                .collect::<Vec<_>>();
            (inputs, outputs)
        }),
        cx,
    );
    mezon_store::AudioStore::set_mic_capture_factory(
        &audio_store,
        std::sync::Arc::new(|device_id: &str, sender: flume::Sender<f32>| {
            mezon_native::audio::MicCapture::start(device_id, sender)
                .map(|capture| Box::new(capture) as Box<dyn Send>)
                .map_err(|e| e.to_string())
        }),
        cx,
    );
    mezon_store::AudioStore::set_mic_pcm_capture_factory(
        &audio_store,
        std::sync::Arc::new(|sender: flume::Sender<Vec<f32>>| {
            mezon_native::audio::MicPcmCapture::start(sender)
                .map(|(capture, format)| {
                    (
                        Box::new(capture) as Box<dyn Send>,
                        mezon_store::MicPcmFormat {
                            sample_rate: format.sample_rate,
                            channels: format.channels,
                        },
                    )
                })
                .map_err(|e| e.to_string())
        }),
        cx,
    );

    let root_auth_state = auth_state.clone();
    let window_handle = cx
        .open_window(options, move |_window, cx| {
            cx.new(|cx| RootView::new(title_bar, root_auth_state, settings_entity, cx))
        })
        .unwrap_or_else(|e| {
            tracing::error!("Failed to open main window: {e}");
            std::process::exit(1);
        });

    mezon_ui::app::window_controls::configure_window(cx, window_handle);

    (auth_state, window_handle.into())
}

fn register_mcp_server_hooks(cx: &mut App, controller: Arc<mezon_mcp::McpController>) {
    let runtime = mezon_client::transport_runtime::handle();
    let platform = mezon_store::PlatformStore::global(cx);
    mezon_store::PlatformStore::set_mcp_server(
        &platform,
        mezon_store::McpServerHooks {
            status: Arc::new({
                let controller = controller.clone();
                let runtime = runtime.clone();
                move || mcp_status_snapshot(&runtime, &controller)
            }),
            start: Arc::new({
                let controller = controller.clone();
                let runtime = runtime.clone();
                move |read_only| {
                    runtime.block_on(controller.start(read_only, None))?;
                    Ok(mcp_status_snapshot(&runtime, &controller))
                }
            }),
            stop: Arc::new({
                let controller = controller.clone();
                let runtime = runtime.clone();
                move || {
                    runtime.block_on(controller.stop())?;
                    Ok(mcp_status_snapshot(&runtime, &controller))
                }
            }),
        },
        cx,
    );
}

fn mcp_status_snapshot(
    runtime: &tokio::runtime::Handle,
    controller: &mezon_mcp::McpController,
) -> mezon_store::McpServerStatus {
    let status = runtime.block_on(controller.status());
    mezon_store::McpServerStatus {
        running: status.running,
        port: status.port,
        read_only: status.read_only,
        url: status.url,
    }
}

struct SingleInstanceGlobal(#[allow(dead_code)] SingleInstance);
impl gpui::Global for SingleInstanceGlobal {}

struct BadgeBridgeGlobal {
    _clan: gpui::Subscription,
    _dm: gpui::Subscription,
    _friends: gpui::Subscription,
    _notification: gpui::Subscription,
}
impl gpui::Global for BadgeBridgeGlobal {}

fn install_badge_bridge(cx: &mut App) {
    let clan_list = mezon_store::ClanList::global(cx);
    let dm_store = mezon_store::DirectMessageStore::global(cx);
    let friend_store = mezon_store::FriendStore::global(cx);
    let notification_store = mezon_store::NotificationSettingStore::global(cx);

    update_native_badge(cx);

    let clan_sub = cx.observe(&clan_list, |_, cx| update_native_badge(cx));
    let dm_sub = cx.observe(&dm_store, |_, cx| update_native_badge(cx));
    let friends_sub = cx.observe(&friend_store, |_, cx| update_native_badge(cx));
    let notification_sub = cx.observe(&notification_store, |_, cx| update_native_badge(cx));

    cx.set_global(BadgeBridgeGlobal {
        _clan: clan_sub,
        _dm: dm_sub,
        _friends: friends_sub,
        _notification: notification_sub,
    });
}

/// Aggregate the OS dock/taskbar badge total: clan mention badges + unread DMs
/// that are not muted + pending friend requests. Muted DMs are excluded (React
/// parity: `selectTotalUnreadDM` filters `isMute`), summed with saturation.
fn badge_total(
    clan_total: u32,
    dm_unread: impl Iterator<Item = (u32, bool)>,
    pending_friend_requests: u32,
) -> u32 {
    let dm_total: u32 = dm_unread
        .filter(|&(unread, muted)| mezon_store::dm_counts_toward_unread_badge(unread, muted))
        .map(|(unread, _)| unread)
        .sum();
    clan_total
        .saturating_add(dm_total)
        .saturating_add(pending_friend_requests)
}

fn update_native_badge(cx: &App) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static LAST_BADGE: AtomicU32 = AtomicU32::new(0);

    let clan_total: u32 = mezon_store::ClanList::global(cx)
        .read(cx)
        .clans
        .iter()
        .map(|clan| clan.badge_count)
        .sum();
    let notification_store = mezon_store::NotificationSettingStore::try_global(cx);
    let muted = notification_store.as_ref().map(|store| store.read(cx));
    let dm_store = mezon_store::DirectMessageStore::global(cx);
    let dm_read = dm_store.read(cx);
    let pending_friend_requests = mezon_store::FriendStore::try_global(cx)
        .map(|store| store.read(cx).pending_incoming_count() as u32)
        .unwrap_or(0);
    let total = badge_total(
        clan_total,
        dm_read.channels().iter().map(|ch| {
            (
                ch.unread_count,
                muted.is_some_and(|s| s.is_time_muted(ch.id)),
            )
        }),
        pending_friend_requests,
    );

    if LAST_BADGE.swap(total, Ordering::Relaxed) != total {
        mezon_native::badge::set_badge_count(total);
    }
}

fn show_main_window(cx: &mut App) {
    activate_main_window(cx);
}

struct NotificationClickGlobal(#[allow(dead_code)] gpui::Task<()>);
impl gpui::Global for NotificationClickGlobal {}

fn notification_route(
    target: &mezon_native::notifications::NotificationTarget,
) -> Option<mezon_ui::Route> {
    // Prefer the server-rendered path (React `extras.link`), which routes correctly
    // regardless of whether the clan is cached locally. The link is an absolute URL
    // (`https://mezon.ai/chat/...`), so route on its path component.
    if let Some(link) = target.link.as_deref().filter(|l| !l.is_empty()) {
        let path = link
            .split_once("://")
            .map(|(_, rest)| rest.find('/').map_or("/", |i| &rest[i..]))
            .unwrap_or(link);
        let route = mezon_ui::Route::from_path(path);
        if !matches!(route, mezon_ui::Route::NotFound { .. }) {
            return Some(route);
        }
    }
    let channel_id = target.channel_id.parse::<mezon_store::ChannelId>().ok()?;
    match target.clan_id.as_deref() {
        Some(clan) => {
            let clan_id = clan.parse::<mezon_store::ClanId>().ok()?;
            Some(mezon_ui::Route::Channel {
                clan_id,
                channel_id,
            })
        }
        None => Some(mezon_ui::Route::DirectMessage {
            direct_id: channel_id,
            message_type: "3".into(),
        }),
    }
}

struct TrayGlobal(#[allow(dead_code)] mezon_native::tray::MezonTray);
impl gpui::Global for TrayGlobal {}

struct WakeCheckTaskGlobal(#[allow(dead_code)] gpui::Task<()>);
impl gpui::Global for WakeCheckTaskGlobal {}

struct TrayTasksGlobal {
    _deep_link: gpui::Task<()>,
    _tray_tasks: TrayTasks,
}
impl gpui::Global for TrayTasksGlobal {}

struct TrayTasks {
    _show: gpui::Task<()>,
    _update: gpui::Task<()>,
    _quit: gpui::Task<()>,
}

fn setup_tray(cx: &mut App) -> Option<(mezon_native::tray::MezonTray, TrayTasks)> {
    let (show_tx, mut show_rx) = futures::channel::mpsc::unbounded::<()>();
    let (update_tx, mut update_rx) = futures::channel::mpsc::unbounded::<()>();
    let (quit_tx, mut quit_rx) = futures::channel::mpsc::unbounded::<()>();

    let show_task = cx.spawn(async move |cx: &mut AsyncApp| {
        while show_rx.next().await.is_some() {
            cx.update(show_main_window);
        }
    });

    let update_task = cx.spawn(async move |cx: &mut AsyncApp| {
        while update_rx.next().await.is_some() {
            cx.update(|cx| {
                show_main_window(cx);
                if let Some(store) = mezon_store::AutoUpdateStore::try_global(cx) {
                    store.update(cx, |store, cx| store.check(true, cx));
                }
            });
        }
    });

    let quit_task = cx.spawn(async move |cx: &mut AsyncApp| {
        if quit_rx.next().await.is_some() {
            cx.update(|cx| cx.quit());
        }
    });

    match mezon_native::tray::MezonTray::new(
        move || {
            tracing::debug!("Tray: Show Mezon");
            let _ = show_tx.unbounded_send(());
        },
        move || {
            tracing::info!("Tray: Check for updates requested");
            let _ = update_tx.unbounded_send(());
        },
        move || {
            tracing::info!("Tray: Quit requested");
            let _ = quit_tx.unbounded_send(());
        },
    ) {
        Ok(tray) => {
            tracing::debug!("System tray initialised");
            Some((
                tray,
                TrayTasks {
                    _show: show_task,
                    _update: update_task,
                    _quit: quit_task,
                },
            ))
        }
        Err(e) => {
            tracing::warn!("Failed to create system tray: {e}");
            None
        }
    }
}

fn save_attachment(url: &str, filename: &str) -> anyhow::Result<()> {
    mezon_client::clean_download_url(url)
        .ok_or_else(|| anyhow::anyhow!("save_attachment rejected: invalid url"))?;
    let url = url.to_string();
    let filename = filename.to_string();
    mezon_client::transport_runtime::handle().spawn(async move {
        match mezon_client::download_url_to_downloads(&url, &filename).await {
            Ok(path) => tracing::info!("saved attachment to {}", path.display()),
            Err(e) => tracing::warn!("save attachment failed: {e}"),
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::badge_total;

    #[test]
    fn badge_total_sums_clan_dm_and_friend_requests() {
        let dms = [(2u32, false), (0, false)];
        assert_eq!(badge_total(4, dms.into_iter(), 3), 9);
    }

    #[test]
    fn badge_total_excludes_muted_dms() {
        // clan 2 + unread DMs 3 and 1 (non-muted) + muted DM 5 (ignored) + 1 friend req.
        let dms = [(3u32, false), (5, true), (1, false)];
        assert_eq!(badge_total(2, dms.into_iter(), 1), 7);
    }

    #[test]
    fn badge_total_all_muted_dms_contribute_zero() {
        let dms = [(9u32, true), (4, true)];
        assert_eq!(badge_total(0, dms.into_iter(), 0), 0);
    }

    #[test]
    fn badge_total_is_zero_when_everything_empty() {
        assert_eq!(badge_total(0, std::iter::empty(), 0), 0);
    }

    #[test]
    fn badge_total_saturates_instead_of_overflowing() {
        let dms = [(10u32, false)];
        assert_eq!(badge_total(u32::MAX, dms.into_iter(), 5), u32::MAX);
    }
}
