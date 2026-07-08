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

fn main() -> Result<()> {
    init_logging();
    install_panic_hook();

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
    // Reuse the shared transport runtime for auxiliary background work (the tray's update
    // check) instead of standing up a second process-wide runtime.
    let rt_handle = Arc::new(mezon_client::transport_runtime::handle());

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
    mezon_native::power::subscribe(Box::new(|event| match event {
        mezon_native::power::PowerEvent::ScreenLocked => {
            tracing::info!("Screen locked");
        }
        mezon_native::power::PowerEvent::ScreenUnlocked => {
            tracing::info!("Screen unlocked");
        }
    }));

    let (url_tx, mut url_rx) = futures::channel::mpsc::unbounded::<String>();

    let app_config_handle = app_config.clone();
    let app = application()
        .with_http_client(Arc::new(
            mezon_client::image_disk_cache::DiskImageCacheClient::new(
                mezon_client::transport_runtime::new_http_client_with_user_agent(
                    IMAGE_CLIENT_USER_AGENT,
                ),
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

        let (auth_state_handle, window_handle) = open_main_window(
            cx,
            &settings,
            settings_entity,
            client.clone(),
            api.clone(),
            transport.clone(),
            initial_auth_state,
        );

        register_main_window(window_handle, cx);

        #[cfg(target_os = "windows")]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(Some(RawWindowHandle::Win32(win32))) = cx
                .update_window(window_handle, |_, window, _| {
                    window.window_handle().ok().map(|handle| handle.as_raw())
                })
            {
                mezon_native::badge::set_main_window_hwnd(win32.hwnd.get());
            }
        }

        install_badge_bridge(cx);

        let deep_link_task = {
            let auth_state = auth_state_handle.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                while let Some(url) = url_rx.next().await {
                    tracing::info!(
                        "Received deep link: {}",
                        url.split(['?', '#']).next().unwrap_or_default()
                    );
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
                                            *state = AuthState::Connecting(session);
                                            cx.notify();
                                        });
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

        if let Some((tray, tray_tasks)) = setup_tray(cx, rt_handle.clone()) {
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
    let title_bar = cx.new(|cx| TitleBar::new(settings_entity.clone(), cx));

    mezon_store::LoginStore::init(client, api.clone(), auth_state.clone(), cx);
    mezon_store::RealtimeDispatch::init(api.clone(), cx);
    mezon_store::ConnectionStore::init(transport, api.clone(), auth_state.clone(), cx);
    mezon_store::ClanList::init(api.clone(), cx);
    mezon_store::ClanMembersStore::init(api.clone(), cx);
    mezon_store::ChannelList::init(api.clone(), cx);
    mezon_store::DirectMessageStore::init(api.clone(), cx);
    mezon_store::BadgeService::init(auth_state.clone(), cx);
    mezon_store::MessagesStore::init(api.clone(), cx);
    mezon_store::ThreadsStore::init(api.clone(), cx);
    mezon_store::InboxStore::init(api.clone(), cx);
    mezon_store::TopicsStore::init(api.clone(), cx);
    mezon_store::TopicBadgeStore::init(api.clone(), auth_state.clone(), cx);
    mezon_store::PinnedMessagesStore::init(api.clone(), cx);
    mezon_store::PresenceStore::init(api.clone(), cx);
    mezon_store::VoiceStore::init(api.clone(), cx);
    mezon_store::ClanMembersStore::init(api.clone(), cx);
    mezon_store::EmojiStore::init(api.clone(), cx);
    mezon_store::StickerStore::init(api.clone(), cx);
    mezon_store::ChannelMembersStore::init(api.clone(), cx);
    mezon_store::ChannelPermissionsStore::init(api.clone(), cx);
    mezon_store::GroupMembersStore::init(api.clone(), cx);
    mezon_store::UsersByUserStore::init(api.clone(), cx);
    mezon_store::RolesStore::init(api.clone(), cx);
    mezon_store::GalleryStore::init(api.clone(), cx);
    mezon_store::PermissionStore::init(api.clone(), auth_state.clone(), cx);
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
            });
        }),
        cx,
    );

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

    let root_auth_state = auth_state.clone();
    let window_handle = cx
        .open_window(options, move |_window, cx| {
            cx.new(|cx| RootView::new(title_bar, root_auth_state, settings_entity, cx))
        })
        .unwrap_or_else(|e| {
            tracing::error!("Failed to open main window: {e}");
            std::process::exit(1);
        });

    #[cfg(target_os = "macos")]
    mezon_ui::app::window_controls::macos::configure_window(cx, window_handle);

    (auth_state, window_handle.into())
}

struct SingleInstanceGlobal(#[allow(dead_code)] SingleInstance);
impl gpui::Global for SingleInstanceGlobal {}

struct BadgeBridgeGlobal {
    _clan: gpui::Subscription,
    _dm: gpui::Subscription,
}
impl gpui::Global for BadgeBridgeGlobal {}

fn install_badge_bridge(cx: &mut App) {
    let clan_list = mezon_store::ClanList::global(cx);
    let dm_store = mezon_store::DirectMessageStore::global(cx);

    update_native_badge(cx);

    let clan_sub = cx.observe(&clan_list, |_, cx| update_native_badge(cx));
    let dm_sub = cx.observe(&dm_store, |_, cx| update_native_badge(cx));

    cx.set_global(BadgeBridgeGlobal {
        _clan: clan_sub,
        _dm: dm_sub,
    });
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
    let dm_total: u32 = mezon_store::DirectMessageStore::global(cx)
        .read(cx)
        .channels()
        .iter()
        .map(|channel| channel.unread_count)
        .sum();
    let total = clan_total.saturating_add(dm_total);

    if LAST_BADGE.swap(total, Ordering::Relaxed) != total {
        mezon_native::badge::set_badge_count(total);
    }
}

fn show_main_window(cx: &mut App) {
    activate_main_window(cx);
}

struct TrayGlobal(#[allow(dead_code)] mezon_native::tray::MezonTray);
impl gpui::Global for TrayGlobal {}

struct TrayTasksGlobal {
    _deep_link: gpui::Task<()>,
    _tray_tasks: TrayTasks,
}
impl gpui::Global for TrayTasksGlobal {}

struct TrayTasks {
    _show: gpui::Task<()>,
    _quit: gpui::Task<()>,
}

fn setup_tray(
    cx: &mut App,
    rt_handle: Arc<tokio::runtime::Handle>,
) -> Option<(mezon_native::tray::MezonTray, TrayTasks)> {
    let (show_tx, mut show_rx) = futures::channel::mpsc::unbounded::<()>();
    let (quit_tx, mut quit_rx) = futures::channel::mpsc::unbounded::<()>();

    let show_task = cx.spawn(async move |cx: &mut AsyncApp| {
        while show_rx.next().await.is_some() {
            cx.update(show_main_window);
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
            tracing::info!("Tray: Quit requested");
            let _ = quit_tx.unbounded_send(());
        },
        rt_handle,
    ) {
        Ok(tray) => {
            tracing::debug!("System tray initialised");
            Some((
                tray,
                TrayTasks {
                    _show: show_task,
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
