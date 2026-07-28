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
    AppApi, ConnectionStatus, DEFAULT_WS_HOST, NetworkMonitor, RECONNECT_NETWORK_PROBE_TIMEOUT,
    RealtimeEvent, Session, TransportClient, favicon_probe_url, keychain,
    probe_network_reachability,
};

use crate::login::{session_credentials, spawn_session_logout};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::{AppConfig, AuthState};

const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
const CONNECT_CONFIRM_GRACE: std::time::Duration = std::time::Duration::from_secs(1);
const MAX_CONSECUTIVE_FAILURES: u32 = 5;
const RECONNECT_BACKOFF_CAP_SECS: u64 = 60;
const DEFAULT_TLS_PORT: u16 = 443;
/// Result of a single reconnect attempt. Only a rejected credential (the server accepted the TCP
/// connection but refused the handshake) is treated as a dead session; an unreachable server must
/// never discard the persisted session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConnectOutcome {
    Confirmed,
    HandshakeRejected,
    TransportError,
}

/// Owns the transport connection manager task + the auth-state observation. Registered as a
/// [`Global`] so it lives for the process; the held [`Task`]/[`Subscription`] cancel on drop.
pub struct ConnectionStore {
    online: bool,
    connecting_attempt: u32,
    _manager: Task<()>,
    _auth_observe: Subscription,
    _heartbeat: Task<()>,
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

        let heartbeat = Self::spawn_heartbeat(transport.clone(), api.clone(), wake.clone(), cx);

        let probe_url = AppConfig::try_global(cx)
            .map(|cfg| favicon_probe_url(&cfg.redirect_uri))
            .unwrap_or_else(|| favicon_probe_url(""));
        let tcp_default_port = AppConfig::try_global(cx).and_then(|cfg| cfg.tcp_port);
        let online_signal = network.online();

        let manager = cx.spawn(async move |this, cx| {
            let exec = cx.background_executor().clone();
            let mut connected_user_id: Option<String> = None;
            let mut retry_backoff_secs = 1u64;
            let mut consecutive_failures = 0u32;
            let mut handshake_rejections = 0u32;
            let mut connect_ack_rx = connect_ack_rx;

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

                let Some(session) = session else {
                    if connected_user_id.take().is_some() {
                        if let Err(e) = transport.close().await {
                            tracing::warn!("Failed to close TCP transport after logout: {e}");
                        }
                        api.set_status(ConnectionStatus::Disconnected);
                    }
                    retry_backoff_secs = 1;
                    consecutive_failures = 0;
                    handshake_rejections = 0;
                    wake.notified().await;
                    continue;
                };

                if connected_user_id.as_deref() == Some(session.user_id.as_str())
                    && transport.is_open().await
                {
                    retry_backoff_secs = 1;
                    consecutive_failures = 0;
                    handshake_rejections = 0;
                    wake.notified().await;
                    continue;
                }

                if requires_network_probe(consecutive_failures) {
                    let os_online = *online_signal.borrow();
                    let reachable = os_online
                        && probe_network_reachability(&probe_url, RECONNECT_NETWORK_PROBE_TIMEOUT)
                            .await;
                    if !reachable {
                        tracing::warn!(
                            "Network unreachable before reconnect attempt — backing off without consuming a retry"
                        );
                        promote_connecting_to_authenticated(&auth_state, cx);
                        retry_backoff_secs = next_backoff_secs(retry_backoff_secs);
                        backoff_wait(&exec, &wake, retry_backoff_secs).await;
                        continue;
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
                let token = session.ws_credential().to_string();
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
                            tracing::warn!(
                                "Connection not confirmed — handshake rejected or dropped"
                            );
                            let _ = transport.close().await;
                            ConnectOutcome::HandshakeRejected
                        }
                    }
                    Err(e) => {
                        tracing::error!("Shared abridged TCP transport connect failed: {e}");
                        ConnectOutcome::TransportError
                    }
                };

                if outcome == ConnectOutcome::Confirmed {
                    connected_user_id = Some(session.user_id.clone());
                    retry_backoff_secs = 1;
                    consecutive_failures = 0;
                    handshake_rejections = 0;
                    api.set_status(ConnectionStatus::Connected);
                    tracing::info!("Connection confirmed — handshake accepted");
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

                match outcome {
                    ConnectOutcome::HandshakeRejected => {
                        handshake_rejections += 1;
                        if reached_failure_limit(handshake_rejections) {
                            tracing::warn!(
                                "Handshake rejected {handshake_rejections} times consecutively — session credential is no longer valid, logging out"
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
                            consecutive_failures = 0;
                            handshake_rejections = 0;
                            retry_backoff_secs = 1;
                            continue;
                        }
                    }
                    ConnectOutcome::TransportError => {
                        handshake_rejections = 0;
                        if reached_failure_limit(consecutive_failures) {
                            tracing::warn!(
                                "Backend unreachable after {consecutive_failures} attempts — keeping session, retrying in the background"
                            );
                            promote_connecting_to_authenticated(&auth_state, cx);
                        }
                    }
                    ConnectOutcome::Confirmed => {}
                }

                retry_backoff_secs = next_backoff_secs(retry_backoff_secs);
                backoff_wait(&exec, &wake, retry_backoff_secs).await;
            }
        });

        Self {
            online,
            connecting_attempt: 0,
            _manager: manager,
            _auth_observe: auth_observe,
            _heartbeat: heartbeat,
            _online_watch: online_watch,
            _network: network,
        }
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
                    if ev.session_id.is_empty() {
                        return;
                    }
                    tracing::info!("Session refreshed over socket for user_id={}", ev.user_id);
                    match state {
                        AuthState::Authenticated(session) | AuthState::Connecting(session) => {
                            session.session_id = ev.session_id.clone();
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

fn requires_network_probe(consecutive_failures: u32) -> bool {
    consecutive_failures >= 1
}

fn reached_failure_limit(consecutive_failures: u32) -> bool {
    consecutive_failures >= MAX_CONSECUTIVE_FAILURES
}

fn next_backoff_secs(current: u64) -> u64 {
    current.saturating_mul(2).min(RECONNECT_BACKOFF_CAP_SECS)
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
    tokio::select! {
        _ = wake.notified() => {}
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

    struct ReconnectSim {
        consecutive_failures: u32,
        handshake_rejections: u32,
        logout_count: u32,
        surface: Surface,
        displayed_attempt: u32,
    }

    impl ReconnectSim {
        fn new() -> Self {
            Self {
                consecutive_failures: 0,
                handshake_rejections: 0,
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

        fn record_offline_gate(&mut self) {
            self.surface = Surface::AppShell;
        }

        fn record_handshake_rejection(&mut self) {
            self.consecutive_failures += 1;
            self.handshake_rejections += 1;
            if reached_failure_limit(self.handshake_rejections) {
                self.logout_count += 1;
                self.consecutive_failures = 0;
                self.handshake_rejections = 0;
                self.surface = Surface::LoggedOut;
            }
        }

        fn record_transport_error(&mut self) {
            self.consecutive_failures += 1;
            self.handshake_rejections = 0;
            if reached_failure_limit(self.consecutive_failures) {
                self.surface = Surface::AppShell;
            }
        }

        fn record_connect_success(&mut self) {
            self.consecutive_failures = 0;
            self.handshake_rejections = 0;
            self.surface = Surface::AppShell;
        }
    }

    #[test]
    fn five_consecutive_handshake_rejections_log_out_once_then_reset() {
        let mut sim = ReconnectSim::new();
        for _ in 0..4 {
            sim.record_handshake_rejection();
            assert_eq!(sim.logout_count, 0);
        }
        sim.record_handshake_rejection();
        assert_eq!(sim.logout_count, 1);
        assert_eq!(sim.consecutive_failures, 0);
        assert_eq!(sim.handshake_rejections, 0);
        assert!(!sim.probe_required());
    }

    #[test]
    fn transport_errors_never_log_out() {
        let mut sim = ReconnectSim::new();
        for _ in 0..50 {
            sim.record_transport_error();
        }
        assert_eq!(sim.logout_count, 0);
    }

    #[test]
    fn transport_error_at_limit_promotes_to_app_shell_keeping_session() {
        let mut sim = ReconnectSim::new();
        for _ in 0..4 {
            sim.record_transport_error();
            assert_eq!(sim.surface, Surface::Connecting);
        }
        sim.record_transport_error();
        assert_eq!(sim.surface, Surface::AppShell);
        assert_eq!(sim.logout_count, 0);
    }

    #[test]
    fn transport_error_resets_handshake_rejection_streak() {
        let mut sim = ReconnectSim::new();
        for _ in 0..4 {
            sim.record_handshake_rejection();
        }
        assert_eq!(sim.handshake_rejections, 4);

        sim.record_transport_error();
        assert_eq!(sim.handshake_rejections, 0);
        assert_eq!(sim.logout_count, 0);

        for _ in 0..4 {
            sim.record_handshake_rejection();
        }
        assert_eq!(sim.logout_count, 0);
        sim.record_handshake_rejection();
        assert_eq!(sim.logout_count, 1);
    }

    #[test]
    fn success_resets_failure_counter() {
        let mut sim = ReconnectSim::new();
        for _ in 0..4 {
            sim.record_handshake_rejection();
        }
        assert_eq!(sim.consecutive_failures, 4);

        sim.record_connect_success();
        assert_eq!(sim.consecutive_failures, 0);
        assert_eq!(sim.handshake_rejections, 0);
        assert_eq!(sim.logout_count, 0);

        for _ in 0..4 {
            sim.record_handshake_rejection();
        }
        assert_eq!(sim.logout_count, 0);
        sim.record_handshake_rejection();
        assert_eq!(sim.logout_count, 1);
    }

    #[test]
    fn offline_skips_do_not_consume_attempts_or_log_out() {
        let mut sim = ReconnectSim::new();
        sim.record_transport_error();
        assert!(sim.probe_required());

        for _ in 0..50 {
            assert!(sim.probe_required());
            assert_eq!(sim.logout_count, 0);
            assert_eq!(sim.consecutive_failures, 1);
        }
    }

    #[test]
    fn online_failure_below_limit_keeps_connecting() {
        let mut sim = ReconnectSim::new();
        for expected in 1..=4 {
            sim.record_handshake_rejection();
            assert_eq!(sim.consecutive_failures, expected);
            assert_eq!(sim.surface, Surface::Connecting);
        }
    }

    #[test]
    fn offline_gate_promotes_to_app_shell_without_consuming_attempt() {
        let mut sim = ReconnectSim::new();
        sim.record_transport_error();
        assert_eq!(sim.surface, Surface::Connecting);
        sim.begin_iteration();
        assert!(sim.probe_required());

        sim.record_offline_gate();
        assert_eq!(sim.surface, Surface::AppShell);
        assert_eq!(sim.consecutive_failures, 1);
        assert_eq!(sim.logout_count, 0);
    }

    #[test]
    fn fifth_handshake_rejection_logs_out_and_leaves_connecting() {
        let mut sim = ReconnectSim::new();
        for _ in 0..4 {
            sim.record_handshake_rejection();
            assert_eq!(sim.surface, Surface::Connecting);
        }
        sim.record_handshake_rejection();
        assert_eq!(sim.surface, Surface::LoggedOut);
    }

    #[test]
    fn connecting_attempt_mirrors_failures_and_resets_on_success() {
        let mut sim = ReconnectSim::new();
        sim.begin_iteration();
        assert_eq!(sim.displayed_attempt, 0);

        sim.record_handshake_rejection();
        sim.record_handshake_rejection();
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
            sim.record_handshake_rejection();
            sim.begin_iteration();
            assert_eq!(sim.surface, Surface::AppShell);
            assert_eq!(sim.displayed_attempt, 0);
        }
    }
}
