use anyhow::Result;
use gpui::{App, AppContext, AsyncApp, Bounds, Entity, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;
use mezon_client::{AppApi, MezonClient, TransportClient, keychain};
use mezon_native::instance::SingleInstance;
use mezon_store::{AuthState, Settings};
use mezon_ui::{RootView, init as init_ui, title_bar::TitleBar};
use std::sync::Arc;
use tracing_subscriber::{EnvFilter, fmt};

fn main() -> Result<()> {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("mezon=debug,info")),
        )
        .init();

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

fn run_app(lock: SingleInstance, initial_url: Option<String>) {
    // Build a multi-threaded tokio runtime that lives for the entire process.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");
    let rt_handle = Arc::new(rt.handle().clone());

    let settings = rt.block_on(Settings::load()).unwrap_or_default();

    tracing::debug!(
        "Settings: theme={}, zoom={}, auto_start={}",
        settings.theme,
        settings.zoom_factor,
        settings.auto_start
    );

    // ── Determine initial auth state from keychain ────────────────────────────
    let client = Arc::new(MezonClient::default());
    let transport = Arc::new(TransportClient::new(String::new()));
    let api = Arc::new(AppApi::new(transport.clone()));
    let initial_auth_state = resolve_initial_auth_state(&rt, &client);

    // Sync login-item with settings.
    mezon_native::autostart::sync_auto_start(settings.auto_start);

    // Register mezonapp:// deep link scheme (idempotent).
    mezon_native::deep_link::register_deep_link_scheme();

    // Subscribe to screen lock/unlock events.
    mezon_native::power::subscribe(Box::new(|event| match event {
        mezon_native::power::PowerEvent::ScreenLocked => {
            tracing::info!("Screen locked");
        }
        mezon_native::power::PowerEvent::ScreenUnlocked => {
            tracing::info!("Screen unlocked");
        }
    }));

    application().with_assets(gpui_component_assets::Assets).run(move |cx: &mut App| {
        tracing::debug!("App started");
        init_ui(cx);

        // Shared channel so background threads can send deep link URLs to the GPUI main thread.
        let (url_tx, url_rx) = std::sync::mpsc::channel::<String>();

        // Listen for deep link URLs forwarded from secondary instances.
        {
            let tx = url_tx.clone();
            lock.listen_for_urls(move |url| {
                let _ = tx.send(url);
            });
        }

        // If we were launched with a deep link, inject it immediately.
        if let Some(url) = initial_url {
            let _ = url_tx.send(url);
        }

        // Open the main window and obtain the auth_state entity handle.
        let auth_state_handle = open_main_window(
            cx,
            &settings,
            client.clone(),
            api.clone(),
            initial_auth_state,
        );

        // Background task: poll `url_rx` and update auth state on the main thread.
        {
            let auth_state = auth_state_handle.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let exec = cx.background_executor().clone();
                loop {
                    match url_rx.try_recv() {
                        Ok(url) => {
                            tracing::info!("Received deep link: {url}");
                            cx.update(|cx| {
                                auth_state.update(cx, |state, cx| {
                                    if url.starts_with("mezonapp://callback") {
                                        // Deep link OAuth — kept for future use.
                                        *state = AuthState::AwaitingCallback;
                                    }
                                    cx.notify();
                                });
                            });
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                    exec.timer(std::time::Duration::from_millis(100)).await;
                }
            })
            .detach();
        }

        // Background refresh task: wake every 60s, refresh if session is near expiry.
        spawn_refresh_task(cx, auth_state_handle.clone(), client.clone());
        spawn_transport_task(
            cx,
            auth_state_handle.clone(),
            transport.clone(),
            api.clone(),
        );

        // System tray.
        let _tray = setup_tray(cx, rt_handle.clone());

        cx.activate(true);
    });
}

/// Keep the shared TCP transport connected for authenticated UI API calls.
fn spawn_transport_task(
    cx: &mut App,
    auth_state: Entity<AuthState>,
    transport: Arc<TransportClient>,
    api: Arc<AppApi>,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let exec = cx.background_executor().clone();
        let mut connected_token: Option<String> = None;

        loop {
            exec.timer(std::time::Duration::from_millis(500)).await;

            let session = match cx.update(|cx| auth_state.read(cx).clone()) {
                AuthState::Authenticated(session) => Some(session),
                _ => None,
            };

            let Some(session) = session else {
                if connected_token.take().is_some() {
                    if let Err(e) = transport.close().await {
                        tracing::warn!("Failed to close TCP transport after logout: {e}");
                    }
                }
                continue;
            };

            if connected_token.as_deref() == Some(session.token.as_str())
                && transport.is_open().await
            {
                continue;
            }

            let Some(host) = session.tcp_host.clone() else {
                tracing::warn!("Authenticated session missing tcp_host; TCP APIs unavailable");
                continue;
            };
            // let Some(port) = session.tcp_port else {
            //     tracing::warn!("Authenticated session missing tcp_port; TCP APIs unavailable");
            //     continue;
            // };
            let port = 7349;

            if transport.is_open().await {
                if let Err(e) = transport.close().await {
                    tracing::warn!("Failed to close stale TCP transport: {e}");
                }
            }

            tracing::info!("Connecting shared TCP transport to {host}:{port}");
            let token = session.token.clone();
            match transport
                .connect(
                    &host,
                    port,
                    &token,
                    move |cid, code, message| {
                        tracing::debug!(
                            "TCP server message: cid={}, code={}, len={}",
                            cid,
                            code,
                            message.len()
                        );
                    },
                    move |was_clean| {
                        if was_clean {
                            tracing::info!("TCP transport closed cleanly");
                        } else {
                            tracing::warn!("TCP transport closed with error");
                        }
                    },
                )
                .await
            {
                Ok(()) => {
                    connected_token = Some(token);
                    tracing::info!("Shared TCP transport connected");

                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(250))
                        .await;
                    match api.get_account().await {
                        Ok(account) => {
                            tracing::info!("get_account over shared TCP succeeded");
                            tracing::info!("  User ID: {}", account.user_id);
                            tracing::info!("  Username: {}", account.username);
                            tracing::info!("  Email: {:?}", account.email);
                            tracing::info!("  Display name: {:?}", account.display_name);
                        }
                        Err(e) => tracing::error!("get_account over shared TCP failed: {e}"),
                    }
                }
                Err(e) => {
                    connected_token = None;
                    tracing::error!("Shared TCP transport connect failed: {e}");
                    exec.timer(std::time::Duration::from_secs(3)).await;
                }
            }
        }
    })
    .detach();
}

/// Attempt to restore a valid session from the OS keychain.
///
/// - Valid + not expired → `Authenticated`
/// - Valid + expired     → try silent refresh → `Authenticated` on success, else `NotAuthenticated`
/// - Nothing stored      → `NotAuthenticated`
fn resolve_initial_auth_state(rt: &tokio::runtime::Runtime, client: &MezonClient) -> AuthState {
    match keychain::load_session() {
        None => {
            tracing::info!("No stored session — showing login");
            AuthState::NotAuthenticated
        }
        Some(session) if !mezon_client::Session::is_expired(&session) => {
            tracing::info!("Restored valid session for user_id={}", session.user_id);
            AuthState::Authenticated(session)
        }
        Some(session) => {
            tracing::info!("Stored session expired — attempting silent refresh");
            match rt.block_on(client.refresh_session(&session.refresh_token, false)) {
                Ok(new_session) => {
                    tracing::info!(
                        "Silent refresh succeeded for user_id={}",
                        new_session.user_id
                    );
                    if let Err(e) = keychain::save_session(&new_session) {
                        tracing::warn!("Failed to update keychain after refresh: {e}");
                    }
                    AuthState::Authenticated(new_session)
                }
                Err(e) => {
                    tracing::warn!("Silent refresh failed: {e} — showing login");
                    let _ = keychain::clear_session();
                    AuthState::NotAuthenticated
                }
            }
        }
    }
}

/// Spawn a background task that refreshes the session ~60 seconds before it expires.
fn spawn_refresh_task(cx: &mut App, auth_state: Entity<AuthState>, client: Arc<MezonClient>) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let exec = cx.background_executor().clone();
        loop {
            // Check every 60 seconds.
            exec.timer(std::time::Duration::from_secs(60)).await;

            // Read current session.
            let session_opt = {
                let state = cx.update(|cx| auth_state.read(cx).clone());
                match state {
                    AuthState::Authenticated(session) => Some(session),
                    _ => None,
                }
            };

            let Some(session) = session_opt else { continue };

            // Refresh if within 5 minutes of expiry.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let should_refresh =
                session.expires_at > 0 && session.expires_at.saturating_sub(now) < 300;

            if !should_refresh {
                continue;
            }

            tracing::info!("Session nearing expiry — refreshing");
            match client.refresh_session(&session.refresh_token, false).await {
                Ok(new_session) => {
                    tracing::info!("Session refreshed for user_id={}", new_session.user_id);
                    if let Err(e) = keychain::save_session(&new_session) {
                        tracing::warn!("Failed to save refreshed session to keychain: {e}");
                    }
                    cx.update(|cx| {
                        auth_state.update(cx, |state, cx| {
                            *state = AuthState::Authenticated(new_session);
                            cx.notify();
                        });
                    });
                }
                Err(e) => {
                    tracing::warn!("Session refresh failed: {e} — logging out");
                    let _ = keychain::clear_session();
                    cx.update(|cx| {
                        auth_state.update(cx, |state, cx| {
                            *state = AuthState::NotAuthenticated;
                            cx.notify();
                        });
                    });
                }
            }
        }
    })
    .detach();
}

/// Open the main window and return a cloneable handle to the `AuthState` entity.
fn open_main_window(
    cx: &mut App,
    settings: &Settings,
    client: Arc<MezonClient>,
    api: Arc<AppApi>,
    initial_auth: AuthState,
) -> Entity<AuthState> {
    let window_bounds = if let Some([x, y, w, h]) = settings.window_bounds {
        WindowBounds::Windowed(Bounds {
            origin: gpui::point(px(x as f32), px(y as f32)),
            size: size(px(w as f32), px(h as f32)),
        })
    } else {
        WindowBounds::Windowed(Bounds::centered(None, size(px(1280.0), px(720.0)), cx))
    };

    let options = WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(-100.0), px(-100.0))),
        }),
        window_bounds: Some(window_bounds),
        window_min_size: Some(size(px(950.0), px(500.0))),
        kind: gpui::WindowKind::Normal,
        focus: true,
        show: true,
        ..Default::default()
    };

    // Smuggle the Entity out of the open_window closure via an Arc<Mutex<Option<_>>>.
    let auth_out = Arc::new(std::sync::Mutex::new(None::<Entity<AuthState>>));
    let auth_out_clone = auth_out.clone();

    cx.open_window(options, move |window, cx| {
        let auth_state = cx.new(|_cx| initial_auth);
        *auth_out_clone.lock().unwrap() = Some(auth_state.clone());

        let title_bar = cx.new(|_cx| TitleBar::new("Mezon"));
        let view = cx.new(|cx| RootView::new(title_bar, auth_state, client, api, cx));
        cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })
    .unwrap_or_else(|e| {
        tracing::error!("Failed to open main window: {e}");
        std::process::exit(1);
    });

    auth_out
        .lock()
        .unwrap()
        .clone()
        .expect("auth_state not initialised after open_window")
}

/// Create the system tray (best-effort — log a warning on failure).
fn setup_tray(
    cx: &mut App,
    rt_handle: Arc<tokio::runtime::Handle>,
) -> Option<mezon_native::tray::MezonTray> {
    let quit_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let quit_flag_clone = quit_flag.clone();

    // Background task that watches the quit flag and fires cx.quit().
    cx.spawn(async move |cx: &mut AsyncApp| {
        let exec = cx.background_executor().clone();
        loop {
            if quit_flag_clone.load(std::sync::atomic::Ordering::Relaxed) {
                cx.update(|cx| cx.quit());
                break;
            }
            exec.timer(std::time::Duration::from_millis(200)).await;
        }
    })
    .detach();

    // TODO Stage 2: store WindowHandle and bring window to front on on_show.
    match mezon_native::tray::MezonTray::new(
        || {
            tracing::debug!("Tray: Show Mezon");
        },
        move || {
            tracing::info!("Tray: Quit requested");
            quit_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        },
        rt_handle,
    ) {
        Ok(tray) => {
            tracing::debug!("System tray initialised");
            Some(tray)
        }
        Err(e) => {
            tracing::warn!("Failed to create system tray: {e}");
            None
        }
    }
}
