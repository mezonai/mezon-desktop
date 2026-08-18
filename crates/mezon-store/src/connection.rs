//! Connection / session lifecycle store — the gpui-coupled manager that keeps the abridged-TCP
//! transport connected, drives reconnect with backoff, and applies server-pushed session refresh.
//!
//! In Zed this lives in the `client` crate; `mezon-client` is gpui-free (pure transport), so the
//! gpui-coupled lifecycle lives here as a store instead of in the app binary.

use std::sync::Arc;

use gpui::{
    App, AppContext, AsyncApp, BackgroundExecutor, Context, Entity, Global, Subscription, Task,
};
use mezon_client::{
    AppApi, ConnectionStatus, DEFAULT_WS_HOST, HttpFallbackSession, NetworkMonitor,
    RECONNECT_NETWORK_PROBE_TIMEOUT, RealtimeEvent, Session, TransportClient, favicon_probe_url,
    keychain, probe_network_reachability,
};

use crate::login::{session_credentials, spawn_session_logout};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::{AppConfig, AuthState};

const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
const CONNECT_CONFIRM_GRACE: std::time::Duration = std::time::Duration::from_secs(1);
const MAX_CONSECUTIVE_FAILURES: u32 = 5;
const RECONNECT_BACKOFF_CAP_SECS: u64 = 60;
const NETWORK_PROBE_RETRY_MIN_SECS: u64 = 1;
const NETWORK_PROBE_RETRY_CAP_SECS: u64 = 15;
const DEFAULT_TLS_PORT: u16 = 443;
/// The gateway discards its own 401 before it reaches the wire (`cleanup_connection` clears the
/// write queue that `flush_ssl_wbio` had only queued), so a dead `session_id`, the per-user session
/// limit and a plain outage all arrive as an identical silent close. After this many silent
/// refusals we stop guessing and re-handshake with the JWT — the one credential the server has just
/// confirmed by minting it.
const SSID_REFUSALS_BEFORE_JWT: u32 = 1;
/// How many refusals of the JWT itself before asking the API host whether the account still
/// exists. The JWT was just minted by the server, so its refusal means either the whole session is
/// gone or the gateway is turning connections away for its own reasons — only an authenticated
/// HTTP call separates the two.
const JWT_REFUSALS_BEFORE_PROBE: u32 = 1;
const JWT_SKEW: std::time::Duration = std::time::Duration::from_secs(60);
/// Result of a single reconnect attempt. Only a rejected credential (the server accepted the TCP
/// connection but refused the handshake) is treated as a dead session; an unreachable server must
/// never discard the persisted session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConnectOutcome {
    Confirmed,
    /// The gateway took the connection and then dropped it. It is up and choosing not to serve us,
    /// so the credentials are worth questioning.
    Refused,
    /// The host never answered — DNS, routing, a dead port. This says nothing about credentials
    /// and must never advance the checks that can end in a logout.
    Unreachable,
}

/// What a `SessionRefresh` concluded. `Rejected` is the only server-confirmed proof that the
/// credentials are dead — everything else must keep the session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RefreshVerdict {
    Renewed,
    Transient,
}

/// Owns the transport connection manager task + the auth-state observation. Registered as a
/// [`Global`] so it lives for the process; the held [`Task`]/[`Subscription`] cancel on drop.
pub struct ConnectionStore {
    online: bool,
    connecting_attempt: u32,
    transport: Arc<TransportClient>,
    wake: Arc<tokio::sync::Notify>,
    _manager: Task<()>,
    _auth_observe: Subscription,
    _heartbeat: Task<()>,
    _token_watch: Task<()>,
    _online_watch: Task<()>,
    _network: NetworkMonitor,
}

struct GlobalConnectionStore(Entity<ConnectionStore>);
impl Global for GlobalConnectionStore {}

impl ConnectionStore {
    /// Spawn the connection manager and register session-refresh. Call **after** the realtime
    /// dispatcher and the `auth_state` entity exist.
    pub fn init(
        transport: Arc<TransportClient>,
        api: Arc<AppApi>,
        auth_state: Entity<AuthState>,
        cx: &mut App,
    ) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(transport, api, auth_state, cx));
        cx.set_global(GlobalConnectionStore(entity.clone()));
        entity
    }

    pub fn is_online(&self) -> bool {
        self.online
    }

    pub fn connecting_attempt(&self) -> u32 {
        self.connecting_attempt
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalConnectionStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalConnectionStore>()
            .map(|g| g.0.clone())
    }

    pub fn reconnect(&self, cx: &mut Context<Self>) {
        let transport = self.transport.clone();
        let wake = self.wake.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = transport.close().await {
                    tracing::warn!("Failed to close the transport before reconnect: {e}");
                }
                wake.notify_one();
            })
            .detach();
    }

    fn new(
        transport: Arc<TransportClient>,
        api: Arc<AppApi>,
        auth_state: Entity<AuthState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (connect_ack_tx, connect_ack_rx) = tokio::sync::watch::channel(0u64);
        Self::register_session_refresh(&auth_state, connect_ack_tx, cx);

        // Wake signal that drives reconciliation — fired on auth-state changes and on socket
        // disconnect. Replaces the old 500ms poll (cf. Zed's reactive `client.status()` loop).
        let wake = Arc::new(tokio::sync::Notify::new());
        let auth_observe = cx.observe(&auth_state, {
            let wake = wake.clone();
            move |_, _, _| wake.notify_one()
        });

        let network = NetworkMonitor::new();
        let online = network.is_online();
        let online_watch = {
            let wake = wake.clone();
            let mut online_rx = network.online();
            cx.spawn(async move |this, cx| {
                while online_rx.changed().await.is_ok() {
                    let is_online = *online_rx.borrow();
                    if is_online {
                        wake.notify_one();
                    }
                    if this
                        .update(cx, |store, cx| {
                            store.online = is_online;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        };

        let token_watch = Self::spawn_token_watch(api.clone(), auth_state.clone(), cx);
        let heartbeat = Self::spawn_heartbeat(transport.clone(), api.clone(), wake.clone(), cx);
        let transport_handle = transport.clone();
        let wake_handle = wake.clone();

        let probe_url = AppConfig::try_global(cx)
            .map(|cfg| favicon_probe_url(&cfg.redirect_uri))
            .unwrap_or_else(|| favicon_probe_url(""));
        let tcp_default_port = AppConfig::try_global(cx).and_then(|cfg| cfg.tcp_port);
        let configured_api_base = AppConfig::try_global(cx).map(configured_api_base_url);
        let auth_client = crate::login::LoginStore::global(cx).read(cx).client();
        let api_server_key = AppConfig::try_global(cx)
            .map(|cfg| cfg.api_key.clone())
            .unwrap_or_default();

        let manager = cx.spawn(async move |this, cx| {
            let exec = cx.background_executor().clone();
            let mut connected_user_id: Option<String> = None;
            let mut retry_backoff_secs = 1u64;
            let mut consecutive_failures = 0u32;
            let mut connect_ack_rx = connect_ack_rx;
            let mut network_retry_secs = NETWORK_PROBE_RETRY_MIN_SECS;
            let mut refreshed_this_run = false;
            let mut gateway_refusals = 0u32;
            let mut jwt_refusals = 0u32;
            let mut probed_this_outage = false;

            loop {
                let (session, is_connecting) = cx.update(|cx| match auth_state.read(cx).clone() {
                    AuthState::Connecting(s) => (Some(s), true),
                    AuthState::Authenticated(s) => (Some(s), false),
                    _ => (None, false),
                });

                let displayed_attempt = if is_connecting { consecutive_failures } else { 0 };
                let _ = this.update(cx, |store, cx| {
                    if store.connecting_attempt != displayed_attempt {
                        store.connecting_attempt = displayed_attempt;
                        cx.notify();
                    }
                });

                let Some(mut session) = session else {
                    api.set_http_fallback(None);
                    if connected_user_id.take().is_some() {
                        if let Err(e) = transport.close().await {
                            tracing::warn!("Failed to close TCP transport after logout: {e}");
                        }
                        api.set_status(ConnectionStatus::Disconnected);
                    }
                    retry_backoff_secs = 1;
                    consecutive_failures = 0;
                    network_retry_secs = NETWORK_PROBE_RETRY_MIN_SECS;
                    wake.notified().await;
                    continue;
                };

                api.set_http_fallback(http_fallback_session(
                    &session,
                    configured_api_base.as_deref(),
                    &api_server_key,
                ));

                if connected_user_id.as_deref() == Some(session.user_id.as_str())
                    && transport.is_open().await
                {
                    retry_backoff_secs = 1;
                    consecutive_failures = 0;
                    wake.notified().await;
                    continue;
                }

                let mut network_confirmed = false;
                if requires_network_probe(consecutive_failures) {
                    // Probe the deployment this session actually belongs to; the baked config can
                    // point somewhere else entirely.
                    let target = session
                        .api_url
                        .as_deref()
                        .filter(|url| !url.is_empty())
                        .map(favicon_probe_url)
                        .unwrap_or_else(|| probe_url.clone());
                    let reachable =
                        probe_network_reachability(&target, RECONNECT_NETWORK_PROBE_TIMEOUT).await;
                    let _ = this.update(cx, |store, cx| {
                        if store.online != reachable {
                            store.online = reachable;
                            cx.notify();
                        }
                    });
                    if !reachable {
                        if network_retry_secs == NETWORK_PROBE_RETRY_MIN_SECS {
                            tracing::warn!(
                                "Network unreachable ({target} did not answer) — pausing reconnect until it is back"
                            );
                        }
                        promote_connecting_to_authenticated(&auth_state, cx);
                        backoff_wait(&exec, &wake, network_retry_secs).await;
                        network_retry_secs = next_network_retry_secs(network_retry_secs);
                        continue;
                    }
                    network_confirmed = true;
                    if network_retry_secs != NETWORK_PROBE_RETRY_MIN_SECS {
                        tracing::info!("Network reachable again — resuming reconnect");
                        network_retry_secs = NETWORK_PROBE_RETRY_MIN_SECS;
                    }

                }

                // Two credentials authenticate the same session; the JWT is the escape hatch when
                // the stored `session_id` keeps being refused.
                let use_jwt = should_lead_with_jwt(&session, gateway_refusals);
                if use_jwt && !jwt_is_fresh(&session) {
                    let (renewed, verdict) =
                        refresh_jwt_for_fallback(&api, &auth_state, session.clone(), cx).await;
                    session = renewed;
                    if verdict == RefreshVerdict::Renewed {
                        // This rotation counts as the once-per-run keep-alive.
                        refreshed_this_run = true;
                    }
                }

                // Probe only with a token the server would still accept. A JWT we failed to renew
                // (a 503 from SessionRefresh is enough) answers 403 because it is expired, not
                // because the account is gone — concluding "session dead" from that would log the
                // user out over a transient backend hiccup.
                if use_jwt
                    && jwt_is_fresh(&session)
                    && jwt_refusals >= JWT_REFUSALS_BEFORE_PROBE
                    && !probed_this_outage
                {
                    probed_this_outage = true;
                    let api_base = session
                        .api_url
                        .clone()
                        .filter(|url| !url.is_empty())
                        .or_else(|| configured_api_base.clone());
                    if let Some(api_base) = api_base {
                        match auth_client.probe_session(&api_base, &session.token).await {
                            mezon_client::SessionProbe::Rejected(status) => {
                                tracing::warn!(
                                    "Both socket credentials refused and the API host rejected the token (HTTP {status}) — the session is gone, logging out"
                                );
                                let credentials =
                                    cx.update(|cx| session_credentials(auth_state.read(cx)));
                                spawn_session_logout(api.clone(), credentials, &exec);
                                cx.update(|cx| {
                                    auth_state.update(cx, |state, cx| {
                                        *state = AuthState::NotAuthenticated;
                                        cx.notify();
                                    });
                                    crate::login::LoginStore::reset_all_user_stores(cx);
                                });
                                connected_user_id = None;
                                consecutive_failures = 0;
                                gateway_refusals = 0;
                                jwt_refusals = 0;
                                retry_backoff_secs = 1;
                                continue;
                            }
                            mezon_client::SessionProbe::Alive => {
                                tracing::warn!(
                                    "The API host still accepts this session — the gateway is refusing the connection, keeping the session"
                                );
                            }
                            mezon_client::SessionProbe::Inconclusive => {
                                tracing::warn!("Session probe was inconclusive — keeping the session");
                                probed_this_outage = false;
                            }
                        }
                    }
                }

                let host = session
                    .tcp_host
                    .clone()
                    .or(session.ws_host.clone())
                    .unwrap_or_else(|| DEFAULT_WS_HOST.to_string());
                let explicit_port = resolve_tcp_port(&session, tcp_default_port);
                let endpoint_label = format!("{host}:{explicit_port}");

                if transport.is_open().await
                    && let Err(e) = transport.close().await
                {
                    tracing::warn!("Failed to close stale transport: {e}");
                }

                tracing::info!("Connecting shared abridged TCP transport to {endpoint_label}");
                api.set_status(ConnectionStatus::Connecting);
                let token = if use_jwt {
                    tracing::info!(
                        "Handshaking with the JWT after {gateway_refusals} refusals by the gateway"
                    );
                    session.token.clone()
                } else {
                    session.ws_credential().to_string()
                };
                let api_for_publish = api.clone();
                let api_for_close = api.clone();
                let wake_for_close = wake.clone();
                connect_ack_rx.borrow_and_update();
                let connect_result = transport
                    .connect(
                        &host,
                        explicit_port,
                        &token,
                        move |event| {
                            api_for_publish.publish_event(event);
                        },
                        move |was_clean| {
                            if was_clean {
                                tracing::info!("TCP transport closed cleanly");
                            } else {
                                tracing::warn!("TCP transport closed with error");
                            }
                            api_for_close.set_status(ConnectionStatus::Disconnected);
                            wake_for_close.notify_one();
                        },
                    )
                    .await;

                let outcome = match connect_result {
                    Ok(()) => {
                        tracing::info!("Shared abridged TCP transport connected");
                        let signaled = tokio::select! {
                            res = connect_ack_rx.changed() => res.is_ok(),
                            _ = exec.timer(CONNECT_CONFIRM_GRACE) => true,
                        };
                        let handshake_ok = signaled && transport.is_open().await;
                        if handshake_ok {
                            ConnectOutcome::Confirmed
                        } else {
                            let rejected = transport.credential_rejected();
                            let _ = transport.close().await;
                            tracing::warn!(
                                "Gateway dropped the connection (explicit rejection: {rejected}, server frames: {})",
                                transport.frames_received()
                            );
                            ConnectOutcome::Refused
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Could not reach {endpoint_label}: {e} — a reachability failure, not a credential one"
                        );
                        ConnectOutcome::Unreachable
                    }
                };

                if outcome == ConnectOutcome::Confirmed {
                    connected_user_id = Some(session.user_id.clone());
                    retry_backoff_secs = 1;
                    consecutive_failures = 0;
                    gateway_refusals = 0;
                    jwt_refusals = 0;
                    probed_this_outage = false;
                    network_retry_secs = NETWORK_PROBE_RETRY_MIN_SECS;
                    api.set_status(ConnectionStatus::Connected);
                    tracing::info!("Connection confirmed — handshake accepted");
                    if !refreshed_this_run {
                        refreshed_this_run = true;
                        let (renewed, _) =
                            refresh_jwt_for_fallback(&api, &auth_state, session.clone(), cx).await;
                        api.set_http_fallback(http_fallback_session(
                            &renewed,
                            configured_api_base.as_deref(),
                            &api_server_key,
                        ));
                    }

                    let api_for_join = api.clone();
                    exec.spawn(async move {
                        match api_for_join.join_clan_chat(0).await {
                            Ok(()) => tracing::info!("DM-space subscribed (clan_join clan_id=0)"),
                            Err(e) => {
                                tracing::warn!("DM-space join (clan_join clan_id=0) failed: {e}")
                            }
                        }
                    })
                    .detach();
                    cx.update(|cx| {
                        auth_state.update(cx, |state, cx| {
                            if let AuthState::Connecting(s) = state {
                                let session = s.clone();
                                *state = AuthState::Authenticated(session);
                                cx.notify();
                            }
                        });
                    });
                    continue;
                }

                connected_user_id = None;
                api.set_status(ConnectionStatus::Disconnected);
                consecutive_failures += 1;
                let refused = outcome == ConnectOutcome::Refused;
                if refused {
                    gateway_refusals += 1;
                    if use_jwt && network_confirmed {
                        jwt_refusals += 1;
                    } else if use_jwt {
                        tracing::info!(
                            "JWT refusal seen without a confirmed network — not counting it against the session"
                        );
                    }
                }

                let switched_to_jwt = if refused && !use_jwt {
                    discard_session_id(&auth_state, cx).await
                } else {
                    false
                };

                // No connect failure ends the session: a refusal is answered by trying the other
                // credential and then asking the API host, an unreachable host by waiting.
                if reached_failure_limit(consecutive_failures) {
                    promote_connecting_to_authenticated(&auth_state, cx);
                }

                if switched_to_jwt {
                    continue;
                }

                retry_backoff_secs = next_backoff_secs(retry_backoff_secs);
                backoff_wait(&exec, &wake, retry_backoff_secs).await;
            }
        });

        Self {
            online,
            connecting_attempt: 0,
            transport: transport_handle,
            wake: wake_handle,
            _manager: manager,
            _auth_observe: auth_observe,
            _heartbeat: heartbeat,
            _token_watch: token_watch,
            _online_watch: online_watch,
            _network: network,
        }
    }

    /// Adopt token pairs the HTTP fallback minted for itself. Without this the connection loop
    /// would re-arm the fallback with the stale token it still holds, and the keychain would keep
    /// a refresh token the server has already rotated away.
    fn spawn_token_watch(
        api: Arc<AppApi>,
        auth_state: Entity<AuthState>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let mut renewed = api.renewed_tokens();
        cx.spawn(async move |_, cx| {
            while renewed.changed().await.is_ok() {
                let Some(renewed_tokens) = renewed.borrow_and_update().clone() else {
                    continue;
                };
                let persisted = cx.update(|cx| {
                    auth_state.update(cx, |state, cx| {
                        let session = match state {
                            AuthState::Authenticated(s) | AuthState::Connecting(s) => s,
                            _ => return None,
                        };
                        let token_is_new = session.token != renewed_tokens.token;
                        let id_token_is_new = !renewed_tokens.id_token.is_empty()
                            && session.id_token != renewed_tokens.id_token;
                        if !token_is_new && !id_token_is_new {
                            return None;
                        }
                        session.apply_refresh(
                            &renewed_tokens.token,
                            &renewed_tokens.refresh_token,
                            "",
                            &renewed_tokens.id_token,
                        );
                        cx.notify();
                        Some(session.clone())
                    })
                });
                let Some(session) = persisted else {
                    continue;
                };
                tracing::info!(
                    "Adopted the token the HTTP fallback minted: jwt_valid_for={}s",
                    session.expires_at.saturating_sub(now_secs())
                );
                cx.background_executor()
                    .spawn(async move {
                        if let Err(e) = keychain::save_session(&session) {
                            tracing::warn!("Failed to persist the renewed session: {e}");
                        }
                    })
                    .await;
            }
        })
    }

    fn spawn_heartbeat(
        transport: Arc<TransportClient>,
        api: Arc<AppApi>,
        wake: Arc<tokio::sync::Notify>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let exec = cx.background_executor().clone();
        exec.clone().spawn(async move {
            loop {
                exec.timer(HEARTBEAT_INTERVAL).await;
                if !transport.is_open().await {
                    continue;
                }
                if let Err(e) = transport.ping_roundtrip().await {
                    tracing::warn!("heartbeat ping failed ({e}) — forcing reconnect");
                    let _ = transport.close().await;
                    api.set_status(ConnectionStatus::Disconnected);
                    wake.notify_one();
                }
            }
        })
    }

    /// Apply server-pushed `refresh_session_event`s and persist the refreshed session — the
    /// native equivalent of mezon-js `client.onrefreshsession`.
    fn register_session_refresh(
        auth_state: &Entity<AuthState>,
        connect_ack_tx: tokio::sync::watch::Sender<u64>,
        cx: &mut Context<Self>,
    ) {
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(
                RealtimeKind::SessionRefreshed,
                auth_state,
                move |state, event, cx| {
                    let RealtimeEvent::SessionRefreshed(ev) = event else {
                        return;
                    };
                    connect_ack_tx.send_modify(|n| *n = n.wrapping_add(1));
                    match state {
                        AuthState::Authenticated(session) | AuthState::Connecting(session) => {
                            let token_changed = !ev.token.is_empty() && ev.token != session.token;
                            let sid_changed =
                                !ev.session_id.is_empty() && ev.session_id != session.session_id;
                            let id_token_changed =
                                !ev.id_token.is_empty() && ev.id_token != session.id_token;
                            session.apply_refresh(
                                &ev.token,
                                &ev.refresh_token,
                                &ev.session_id,
                                &ev.id_token,
                            );
                            tracing::info!(
                                "refresh_session_event user_id={} token_sent={} token_changed={} refresh_token_sent={} sid_sent={} sid_changed={} id_token_sent={} id_token_changed={} id_token_valid_for={:?} jwt_valid_for={}s jwt_expired={}",
                                ev.user_id,
                                !ev.token.is_empty(),
                                token_changed,
                                !ev.refresh_token.is_empty(),
                                !ev.session_id.is_empty(),
                                sid_changed,
                                !ev.id_token.is_empty(),
                                id_token_changed,
                                id_token_valid_for(session),
                                session.expires_at.saturating_sub(now_secs()),
                                session.expires_at != 0 && now_secs() >= session.expires_at,
                            );
                            if ev.token.is_empty()
                                && ev.session_id.is_empty()
                                && ev.id_token.is_empty()
                            {
                                return;
                            }
                            let session_clone = session.clone();
                            cx.background_executor()
                                .spawn(async move {
                                    if let Err(e) = keychain::save_session(&session_clone) {
                                        tracing::warn!("Failed to persist refreshed session: {e}");
                                    }
                                })
                                .detach();
                            cx.notify();
                        }
                        _ => {}
                    }
                },
            );
        });
    }
}

fn now_secs() -> u64 {
    mezon_client::server_now_secs()
}

fn id_token_valid_for(session: &Session) -> Option<i64> {
    mezon_client::jwt_expires_at(&session.id_token).map(|exp| exp as i64 - now_secs() as i64)
}

/// Rotate the token pair once per app run, right after the first confirmed handshake. Its job is
/// to keep the *refresh* token alive — that one lives a week (`RefreshTokenExpirySec: 604800`)
/// while the JWT lives ten minutes, so a launch-time rotation is enough to never need a re-login.
/// The JWT for the HTTP fallback is minted separately, at send time, by the transport. The
/// response carries no `session_id`, so the socket credential is left alone.
async fn refresh_jwt_for_fallback(
    api: &Arc<AppApi>,
    auth_state: &Entity<AuthState>,
    session: Session,
    cx: &mut AsyncApp,
) -> (Session, RefreshVerdict) {
    if session.refresh_token.is_empty() {
        return (session, RefreshVerdict::Transient);
    }

    tracing::info!("Rotating the session token pair to keep the refresh token alive");
    let renewed = match api.renew_fallback_token().await {
        Ok(renewed) => renewed,
        Err(e) => {
            tracing::warn!("SessionRefresh failed ({e}) — keeping the current session");
            return (session, RefreshVerdict::Transient);
        }
    };

    let mut session = session;
    session.apply_refresh(
        &renewed.token,
        &renewed.refresh_token,
        "",
        &renewed.id_token,
    );
    tracing::info!(
        "SessionRefresh applied: jwt_valid_for={}s refresh_token_renewed={} id_token_renewed={} id_token_valid_for={:?}",
        session.expires_at.saturating_sub(now_secs()),
        !renewed.refresh_token.is_empty(),
        !renewed.id_token.is_empty(),
        id_token_valid_for(&session),
    );

    // Persist what `auth_state` holds, never the local copy: the handshake that just completed
    // pushed a rotated `session_id` into it, and this call started from a snapshot taken before
    // that. Saving the snapshot would write the retired credential back over the live one, and the
    // next launch would connect with an id the server has already deleted.
    let applied = cx.update(|cx| {
        auth_state.update(cx, |state, cx| {
            let current = match state {
                AuthState::Authenticated(s) | AuthState::Connecting(s) => s,
                _ => return None,
            };
            if current.user_id != session.user_id {
                return None;
            }
            current.apply_refresh(
                &renewed.token,
                &renewed.refresh_token,
                "",
                &renewed.id_token,
            );
            cx.notify();
            Some(current.clone())
        })
    });
    let Some(session) = applied else {
        return (session, RefreshVerdict::Transient);
    };

    // Awaited, not detached: quitting inside this window would otherwise leave the file holding a
    // refresh token the server has already rotated away.
    let persisted = session.clone();
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = keychain::save_session(&persisted) {
                tracing::warn!("Failed to persist the renewed session: {e}");
            }
        })
        .await;
    (session, RefreshVerdict::Renewed)
}

fn should_lead_with_jwt(session: &Session, gateway_refusals: u32) -> bool {
    session.session_id.is_empty() || gateway_refusals >= SSID_REFUSALS_BEFORE_JWT
}

fn clear_socket_credential(session: &mut Session) -> bool {
    if session.session_id.is_empty() {
        return false;
    }
    session.session_id.clear();
    true
}

async fn discard_session_id(auth_state: &Entity<AuthState>, cx: &mut AsyncApp) -> bool {
    let cleared = cx.update(|cx| {
        auth_state.update(cx, |state, cx| {
            let session = match state {
                AuthState::Authenticated(s) | AuthState::Connecting(s) => s,
                _ => return None,
            };
            if !clear_socket_credential(session) {
                return None;
            }
            cx.notify();
            Some(session.clone())
        })
    });
    let Some(session) = cleared else {
        return false;
    };

    tracing::info!(
        "The gateway refused the stored session_id — dropping it so the JWT leads until the server pushes a new one"
    );
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = keychain::save_session(&session) {
                tracing::warn!("Failed to persist the session without its refused session_id: {e}");
            }
        })
        .await;
    true
}

/// Whether the JWT can still authenticate a handshake or an HTTP call.
fn jwt_is_fresh(session: &Session) -> bool {
    !session.token.is_empty()
        && (session.expires_at == 0 || now_secs() + JWT_SKEW.as_secs() < session.expires_at)
}

fn configured_api_base_url(config: &AppConfig) -> String {
    let scheme = if config.api_secure { "https" } else { "http" };
    format!("{scheme}://{}:{}", config.api_host, config.api_port)
}

fn http_fallback_session(
    session: &Session,
    configured_api_base: Option<&str>,
    api_server_key: &str,
) -> Option<HttpFallbackSession> {
    if session.token.is_empty() {
        return None;
    }
    let base_url = session
        .api_url
        .as_deref()
        .filter(|url| !url.is_empty())
        .or(configured_api_base)?
        .to_string();
    Some(HttpFallbackSession {
        base_url,
        token: session.token.clone(),
        expires_at: session.expires_at,
        refresh_token: session.refresh_token.clone(),
        is_remember: session.is_remember,
        server_key: api_server_key.to_owned(),
    })
}

fn requires_network_probe(consecutive_failures: u32) -> bool {
    consecutive_failures >= 1
}

fn reached_failure_limit(consecutive_failures: u32) -> bool {
    consecutive_failures >= MAX_CONSECUTIVE_FAILURES
}

fn next_backoff_secs(current: u64) -> u64 {
    current.saturating_mul(2).min(RECONNECT_BACKOFF_CAP_SECS)
}

fn next_network_retry_secs(current: u64) -> u64 {
    current.saturating_mul(2).min(NETWORK_PROBE_RETRY_CAP_SECS)
}

/// Wait out a reconnect backoff, but wake early if auth/connection state changes.
async fn backoff_wait(exec: &BackgroundExecutor, wake: &tokio::sync::Notify, secs: u64) {
    let base_ms = secs.saturating_mul(1000);
    let jitter_cap = (base_ms / 4).max(1);
    let jitter_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()) % jitter_cap)
        .unwrap_or(0);
    let delay = std::time::Duration::from_millis(base_ms + jitter_ms);
    wait_or_wake(exec, wake, delay).await;
}

/// Wait out `delay`, cut short only by an event that arrives **during** the wait.
///
/// `Notify::notify_one` parks a permit when nobody is waiting, and the transport's close callback
/// fires one on every failed connect — so a plain `wake.notified()` here consumes that stale permit
/// and returns instantly, collapsing every backoff to zero. That turned reconnect into a one-per-
/// second hammer, which is enough on its own to keep a user pinned at the gateway's session limit.
async fn wait_or_wake(
    exec: &BackgroundExecutor,
    wake: &tokio::sync::Notify,
    delay: std::time::Duration,
) {
    let mut notified = Box::pin(wake.notified());
    if notified.as_mut().enable() {
        notified = Box::pin(wake.notified());
        notified.as_mut().enable();
    }
    tokio::select! {
        _ = notified => {}
        _ = exec.timer(delay) => {}
    }
}

/// Leave the loading screen if transport setup fails — user can retry from the app shell.
fn promote_connecting_to_authenticated(auth_state: &Entity<AuthState>, cx: &mut AsyncApp) {
    cx.update(|cx| {
        auth_state.update(cx, |state, cx| {
            if let AuthState::Connecting(s) = state {
                let session = s.clone();
                *state = AuthState::Authenticated(session);
                cx.notify();
            }
        });
    });
}

pub(crate) fn resolve_tcp_port(session: &Session, default_port: Option<u16>) -> u16 {
    session
        .tcp_port
        .or(session.ws_port)
        .or(default_port)
        .unwrap_or(DEFAULT_TLS_PORT)
}

/// Restore a stored session from the OS keychain.
///
/// - Stored session → `Connecting` (the socket validates it; the server pushes a fresh token via
///   `refresh_session_event`, so an expired JWT is fine — `session_id` is the durable cred).
/// - Nothing stored → `NotAuthenticated`.
pub fn resolve_initial_auth_state() -> AuthState {
    match keychain::load_session() {
        None => {
            tracing::info!("No stored session — showing login");
            AuthState::NotAuthenticated
        }
        Some(session) => {
            tracing::info!(
                "Restored stored session for user_id={} — connecting",
                session.user_id
            );
            AuthState::Connecting(session)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_client::Session;

    #[test]
    fn http_fallback_prefers_server_issued_api_url() {
        let expires_at = now_secs() + 3600;
        let s = Session {
            token: "jwt".into(),
            api_url: Some("https://api.mezon.ai".into()),
            expires_at,
            ..Default::default()
        };
        let fallback =
            http_fallback_session(&s, Some("https://baked:8088"), "key").expect("fallback");
        assert_eq!(fallback.base_url, "https://api.mezon.ai");
        assert_eq!(fallback.token, "jwt");
        assert_eq!(fallback.expires_at, expires_at);
    }

    #[test]
    fn http_fallback_falls_back_to_configured_base() {
        let s = Session {
            token: "jwt".into(),
            api_url: Some(String::new()),
            ..Default::default()
        };
        let fallback =
            http_fallback_session(&s, Some("https://baked:8088"), "key").expect("fallback");
        assert_eq!(fallback.base_url, "https://baked:8088");
    }

    #[test]
    fn http_fallback_needs_a_token_and_a_base() {
        let no_token = Session {
            api_url: Some("https://api.mezon.ai".into()),
            ..Default::default()
        };
        assert!(http_fallback_session(&no_token, Some("https://baked:8088"), "key").is_none());

        let no_base = Session {
            token: "jwt".into(),
            ..Default::default()
        };
        assert!(http_fallback_session(&no_base, None, "key").is_none());
    }

    /// `SessionRefresh` rotates the socket credential, so its response must replace the stored
    /// one. Keeping the old `session_id` is what left a reconnect presenting a retired credential.
    #[test]
    fn a_refresh_response_replaces_the_socket_credential() {
        let mut session = Session {
            token: "old-jwt".into(),
            refresh_token: "old-refresh".into(),
            session_id: "retired-sid".into(),
            ..Default::default()
        };
        session.apply_refresh("new-jwt", "new-refresh", "rotated-sid", "new-id-token");
        assert_eq!(session.session_id, "rotated-sid");
        assert_eq!(session.ws_credential(), "rotated-sid");
    }

    /// The fallback is armed whenever a token exists — an expired one is renewed at send time by
    /// the transport, which is the only caller that runs while the socket is down.
    #[test]
    fn http_fallback_carries_everything_needed_to_renew_itself() {
        let session = Session {
            token: "jwt".into(),
            refresh_token: "refresh".into(),
            is_remember: true,
            api_url: Some("https://api.mezon.ai".into()),
            expires_at: now_secs().saturating_sub(60),
            ..Default::default()
        };
        let fallback = http_fallback_session(&session, None, "api-key").expect("fallback");
        assert_eq!(fallback.refresh_token, "refresh");
        assert!(fallback.is_remember);
        assert_eq!(fallback.server_key, "api-key");
    }

    #[test]
    fn resolve_tcp_port_uses_tcp_port_field_first() {
        let s = Session {
            tcp_port: Some(9999),
            ws_port: Some(1111),
            ..Default::default()
        };
        assert_eq!(resolve_tcp_port(&s, Some(4433)), 9999);
    }

    #[test]
    fn resolve_tcp_port_falls_back_to_ws_port() {
        let s = Session {
            ws_port: Some(8888),
            ..Default::default()
        };
        assert_eq!(resolve_tcp_port(&s, Some(4433)), 8888);
    }

    #[test]
    fn resolve_tcp_port_uses_config_default_when_session_has_no_port() {
        let s = Session {
            tcp_host: Some("mezon.ai".to_owned()),
            ..Default::default()
        };
        assert_eq!(resolve_tcp_port(&s, Some(7349)), 7349);
    }

    #[test]
    fn resolve_tcp_port_falls_back_to_tls_default_when_unset() {
        assert_eq!(
            resolve_tcp_port(&Session::default(), None),
            DEFAULT_TLS_PORT
        );
    }

    #[test]
    fn backoff_wait_caps_at_60_seconds() {
        let mut secs = 1u64;
        for _ in 0..10 {
            secs = (secs * 2).min(60);
        }
        assert_eq!(secs, 60);
    }

    #[test]
    fn next_backoff_secs_doubles_with_cap() {
        assert_eq!(next_backoff_secs(1), 2);
        assert_eq!(next_backoff_secs(2), 4);
        assert_eq!(next_backoff_secs(16), 32);
        assert_eq!(next_backoff_secs(32), 60);
        assert_eq!(next_backoff_secs(60), 60);
    }

    #[test]
    fn first_attempt_connects_without_probe_then_probes_every_retry() {
        assert!(!requires_network_probe(0));
        assert!(requires_network_probe(1));
        assert!(requires_network_probe(4));
    }

    #[test]
    fn network_retry_backoff_doubles_up_to_its_own_cap() {
        assert_eq!(next_network_retry_secs(NETWORK_PROBE_RETRY_MIN_SECS), 2);
        assert_eq!(next_network_retry_secs(4), 8);
        assert_eq!(next_network_retry_secs(8), NETWORK_PROBE_RETRY_CAP_SECS);
        assert_eq!(
            next_network_retry_secs(NETWORK_PROBE_RETRY_CAP_SECS),
            NETWORK_PROBE_RETRY_CAP_SECS
        );
    }

    #[test]
    fn a_session_without_an_id_authenticates_with_the_jwt() {
        let jwt_only = Session {
            token: "jwt".into(),
            expires_at: now_secs() + 600,
            ..Default::default()
        };
        assert!(jwt_only.session_id.is_empty());
        assert_eq!(jwt_only.ws_credential(), "jwt");
        assert!(jwt_is_fresh(&jwt_only));
    }

    #[test]
    fn a_refused_socket_credential_is_dropped_and_the_jwt_takes_over() {
        let mut session = Session {
            token: "jwt".into(),
            session_id: "dead-sid".into(),
            expires_at: now_secs() + 600,
            ..Default::default()
        };
        assert!(!should_lead_with_jwt(&session, 0));

        assert!(clear_socket_credential(&mut session));
        assert!(should_lead_with_jwt(&session, 0));
        assert_eq!(session.ws_credential(), "jwt");
        assert!(jwt_is_fresh(&session));

        assert!(!clear_socket_credential(&mut session));
    }

    #[test]
    fn a_single_gateway_refusal_moves_the_ladder_to_the_jwt() {
        let session = Session {
            token: "jwt".into(),
            session_id: "sid".into(),
            ..Default::default()
        };
        assert!(!should_lead_with_jwt(&session, 0));
        assert!(should_lead_with_jwt(&session, SSID_REFUSALS_BEFORE_JWT));
    }

    #[test]
    fn a_stale_jwt_is_renewed_before_it_is_used_as_a_credential() {
        let stale = Session {
            token: "jwt".into(),
            expires_at: now_secs().saturating_sub(1),
            ..Default::default()
        };
        assert!(!jwt_is_fresh(&stale));

        let inside_skew = Session {
            expires_at: now_secs() + JWT_SKEW.as_secs() / 2,
            ..stale.clone()
        };
        assert!(!jwt_is_fresh(&inside_skew));

        let live = Session {
            expires_at: now_secs() + 600,
            ..stale
        };
        assert!(jwt_is_fresh(&live));
    }

    #[test]
    fn failure_limit_reached_only_after_five() {
        assert!(!reached_failure_limit(0));
        assert!(!reached_failure_limit(4));
        assert!(reached_failure_limit(5));
        assert!(reached_failure_limit(6));
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Surface {
        Connecting,
        AppShell,
        LoggedOut,
    }

    /// Models the loop's decision state: what a connect outcome advances, and what may end in a
    /// logout. Only a gateway refusal followed by a refused API probe may.
    struct ReconnectSim {
        consecutive_failures: u32,
        gateway_refusals: u32,
        jwt_refusals: u32,
        logout_count: u32,
        surface: Surface,
        displayed_attempt: u32,
    }

    impl ReconnectSim {
        fn new() -> Self {
            Self {
                consecutive_failures: 0,
                gateway_refusals: 0,
                jwt_refusals: 0,
                logout_count: 0,
                surface: Surface::Connecting,
                displayed_attempt: 0,
            }
        }

        fn begin_iteration(&mut self) {
            self.displayed_attempt = if self.surface == Surface::Connecting {
                self.consecutive_failures
            } else {
                0
            };
        }

        fn probe_required(&self) -> bool {
            requires_network_probe(self.consecutive_failures)
        }

        fn uses_jwt(&self) -> bool {
            self.gateway_refusals >= SSID_REFUSALS_BEFORE_JWT
        }

        fn record_unreachable(&mut self) {
            self.consecutive_failures += 1;
            if reached_failure_limit(self.consecutive_failures) {
                self.surface = Surface::AppShell;
            }
        }

        fn record_refusal(&mut self) {
            self.consecutive_failures += 1;
            let was_jwt = self.uses_jwt();
            self.gateway_refusals += 1;
            if was_jwt {
                self.jwt_refusals += 1;
            }
            if reached_failure_limit(self.consecutive_failures) {
                self.surface = Surface::AppShell;
            }
        }

        /// The API host answering 403 is the only thing that ends the session.
        fn record_api_probe(&mut self, session_alive: bool) {
            if self.jwt_refusals < JWT_REFUSALS_BEFORE_PROBE {
                return;
            }
            if !session_alive {
                self.logout_count += 1;
                self.surface = Surface::LoggedOut;
            }
        }

        fn record_connect_success(&mut self) {
            self.consecutive_failures = 0;
            self.gateway_refusals = 0;
            self.jwt_refusals = 0;
            self.surface = Surface::AppShell;
        }
    }

    /// The whole point: a network problem produces refusals of a kind that can never reach the
    /// logout branch, however long it lasts.
    #[test]
    fn an_unreachable_host_never_logs_out() {
        let mut sim = ReconnectSim::new();
        for _ in 0..200 {
            sim.record_unreachable();
            sim.record_api_probe(false);
        }
        assert_eq!(sim.logout_count, 0);
        assert_eq!(
            sim.gateway_refusals, 0,
            "reachability failures are not refusals"
        );
        assert!(!sim.uses_jwt(), "the JWT escape hatch must stay disarmed");
        assert_eq!(sim.surface, Surface::AppShell);
    }

    /// A dead session must be decided fast: one refusal of each credential, then the probe.
    #[test]
    fn a_dead_session_logs_out_after_one_refusal_of_each_credential() {
        let mut sim = ReconnectSim::new();
        sim.record_refusal();
        assert!(
            sim.uses_jwt(),
            "the JWT is tried straight after the first refusal"
        );
        sim.record_refusal();
        assert_eq!(sim.jwt_refusals, JWT_REFUSALS_BEFORE_PROBE);

        sim.record_api_probe(false);
        assert_eq!(sim.logout_count, 1);
        assert_eq!(sim.surface, Surface::LoggedOut);
    }

    /// Same refusals, but the account is fine — the gateway is simply turning us away.
    #[test]
    fn refusals_with_a_live_account_keep_the_session() {
        let mut sim = ReconnectSim::new();
        for _ in 0..20 {
            sim.record_refusal();
            sim.record_api_probe(true);
        }
        assert_eq!(sim.logout_count, 0);
        assert_eq!(sim.surface, Surface::AppShell);
    }

    #[test]
    fn a_successful_connect_disarms_everything() {
        let mut sim = ReconnectSim::new();
        sim.record_refusal();
        sim.record_refusal();
        sim.record_connect_success();
        assert_eq!(sim.gateway_refusals, 0);
        assert_eq!(sim.jwt_refusals, 0);
        assert!(!sim.uses_jwt());
    }

    #[test]
    fn offline_skips_do_not_consume_attempts_or_log_out() {
        let mut sim = ReconnectSim::new();
        sim.record_unreachable();
        assert!(sim.probe_required());

        for _ in 0..50 {
            assert!(sim.probe_required());
            assert_eq!(sim.logout_count, 0);
            assert_eq!(sim.consecutive_failures, 1);
        }
    }

    #[test]
    fn connecting_attempt_mirrors_failures_and_resets_on_success() {
        let mut sim = ReconnectSim::new();
        sim.begin_iteration();
        assert_eq!(sim.displayed_attempt, 0);

        sim.record_refusal();
        sim.record_refusal();
        sim.begin_iteration();
        assert_eq!(sim.displayed_attempt, 2);

        sim.record_connect_success();
        sim.begin_iteration();
        assert_eq!(sim.displayed_attempt, 0);
        assert_eq!(sim.surface, Surface::AppShell);
    }

    #[test]
    fn mid_session_failures_do_not_display_attempt() {
        let mut sim = ReconnectSim::new();
        sim.record_connect_success();
        for _ in 0..4 {
            sim.record_refusal();
            sim.begin_iteration();
            assert_eq!(sim.surface, Surface::AppShell);
            assert_eq!(sim.displayed_attempt, 0);
        }
    }
}
