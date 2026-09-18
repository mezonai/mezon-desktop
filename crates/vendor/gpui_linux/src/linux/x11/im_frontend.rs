use std::cell::Cell;
use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, RawFd};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dbus::arg::{RefArg, Variant};
use dbus::blocking::{Connection, Proxy};
use dbus::channel::{BusType, Channel, MatchingReceiver, Token};
use dbus::message::MatchRule;
use dbus::{Message, Path as DbusPath};
use gpui::ImeSurroundingText;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const KEY_TIMEOUT: Duration = Duration::from_millis(80);
const MODIFIER_TIMEOUT: Duration = Duration::from_millis(16);
const SIGNAL_WAIT: Duration = Duration::from_millis(16);
const COMMIT_WAIT: Duration = Duration::from_millis(40);
const KEY_BUDGET: Duration = Duration::from_millis(80);
const SLOW_RESPONSE: Duration = Duration::from_millis(24);
const QUARANTINE_WINDOW: Duration = Duration::from_millis(30);
const QUARANTINE_RESET_TIMEOUT: Duration = Duration::from_millis(8);
const DEAD_AFTER_FAILS: u8 = 3;
const FCITX5_DEST: &str = "org.fcitx.Fcitx5";
const FCITX5_IM_PATH: &str = "/org/freedesktop/portal/inputmethod";
const FCITX5_IM_IFACE: &str = "org.fcitx.Fcitx.InputMethod1";
const FCITX5_IC_IFACE: &str = "org.fcitx.Fcitx.InputContext1";
const IBUS_DEST: &str = "org.freedesktop.IBus";
const IBUS_PATH: &str = "/org/freedesktop/IBus";
const IBUS_IFACE: &str = "org.freedesktop.IBus";
const IBUS_IC_IFACE: &str = "org.freedesktop.IBus.InputContext";
const IBUS_SERVICE_IFACE: &str = "org.freedesktop.IBus.Service";
const DESTROY_TIMEOUT: Duration = Duration::from_millis(20);
pub(crate) const IBUS_RELEASE_MASK: u32 = 1 << 30;

const FCITX5_CAP_PREEDIT: u64 = 1 << 1;
const FCITX5_CAP_FORMATTED_PREEDIT: u64 = 1 << 4;
const FCITX5_CAP_CLIENT_UNFOCUS_COMMIT: u64 = 1 << 5;
const FCITX5_CAP_SURROUNDING_TEXT: u64 = 1 << 6;
const FCITX5_CAP_KEY_EVENT_ORDER_FIX: u64 = 1 << 37;
const FCITX5_CAPS: u64 = FCITX5_CAP_PREEDIT
    | FCITX5_CAP_FORMATTED_PREEDIT
    | FCITX5_CAP_CLIENT_UNFOCUS_COMMIT
    | FCITX5_CAP_SURROUNDING_TEXT
    | FCITX5_CAP_KEY_EVENT_ORDER_FIX;

const IBUS_CAP_PREEDIT_TEXT: u32 = 1 << 0;
const IBUS_CAP_FOCUS: u32 = 1 << 3;
const IBUS_CAP_SURROUNDING_TEXT: u32 = 1 << 5;
const IBUS_CAPS: u32 = IBUS_CAP_PREEDIT_TEXT | IBUS_CAP_FOCUS | IBUS_CAP_SURROUNDING_TEXT;

const FCITX5_BATCH_COMMIT: u32 = 0;
const FCITX5_BATCH_PREEDIT: u32 = 1;
const FCITX5_BATCH_FORWARD: u32 = 2;
const FCITX5_BATCH_DELETE: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImKind {
    Fcitx5,
    IBus,
}

#[derive(Debug)]
pub(crate) enum ImEvent {
    Commit(String),
    Preedit {
        text: String,
        caret_chars: i32,
    },
    DeleteSurrounding {
        offset: i32,
        nchars: u32,
    },
    ForwardKey {
        keyval: u32,
        state: u32,
        is_release: bool,
    },
    ClearPreedit,
    HidePreedit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IbusSurroundingEnc {
    Unknown,
    IBusText,
    PlainVariant,
}

pub(crate) struct X11ImContext {
    conn: Connection,
    dest: String,
    path: DbusPath<'static>,
    kind: ImKind,
    events: Arc<Mutex<Vec<ImEvent>>>,
    tokens: Vec<Token>,
    fail_count: Cell<u8>,
    ibus_surrounding: Cell<IbusSurroundingEnc>,
    fcitx_key_mode: Cell<FcitxKeyMode>,
    quarantine_until: Cell<Option<Instant>>,
    slow_daemon: Cell<bool>,
    ibus_fcitx_shim: bool,
    watch: RawFd,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FcitxKeyMode {
    Unknown,
    Batch,
    Single,
}

pub(crate) struct DbusWatchFd(RawFd);

impl AsFd for DbusWatchFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.0) }
    }
}

impl X11ImContext {
    pub(crate) fn connect() -> Option<Self> {
        for kind in connect_order(&preferred_backends()) {
            match kind {
                ImKind::Fcitx5 => match connect_fcitx5() {
                    Ok(ctx) => return Some(ctx),
                    Err(_) => {}
                },
                ImKind::IBus => match connect_ibus() {
                    Ok(ctx) => return Some(ctx),
                    Err(_) => {}
                },
            }
        }
        None
    }

    pub(crate) fn kind(&self) -> ImKind {
        self.kind
    }

    pub(crate) fn ibus_fcitx_shim(&self) -> bool {
        self.ibus_fcitx_shim
    }

    pub(crate) fn watch_fd(&self) -> DbusWatchFd {
        DbusWatchFd(self.watch)
    }

    pub(crate) fn process_io(&mut self) {
        let _ = self.conn.process(Duration::ZERO);
    }

    fn has_events(&self) -> bool {
        self.events
            .lock()
            .map(|events| !events.is_empty())
            .unwrap_or(false)
    }

    fn only_clears_preedit(&self) -> bool {
        self.events
            .lock()
            .map(|events| events_only_clear_preedit(&events))
            .unwrap_or(false)
    }

    pub(crate) fn wait_for_events(&mut self, budget: Duration) {
        self.wait_until(budget, |this| this.has_events());
    }

    fn wait_for_text_event(&mut self, budget: Duration) {
        self.wait_until(budget, |this| this.has_text_event());
    }

    fn wait_until(&mut self, budget: Duration, ready: fn(&Self) -> bool) {
        let started = Instant::now();
        loop {
            let _ = self.conn.process(Duration::ZERO);
            if ready(self) {
                return;
            }
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return;
            }
            let _ = self.conn.process(remaining.min(Duration::from_millis(4)));
            if ready(self) {
                return;
            }
        }
    }

    fn has_text_event(&self) -> bool {
        self.events
            .lock()
            .map(|events| {
                events.iter().any(|event| match event {
                    ImEvent::Commit(text) => !text.is_empty(),
                    ImEvent::Preedit { text, .. } => !text.is_empty(),
                    ImEvent::ForwardKey {
                        is_release: false, ..
                    } => true,
                    _ => false,
                })
            })
            .unwrap_or(false)
    }

    fn push_events(&self, incoming: Vec<ImEvent>) {
        if incoming.is_empty() {
            return;
        }
        if let Ok(mut events) = self.events.lock() {
            events.extend(incoming);
        }
    }

    pub(crate) fn take_events(&self) -> Vec<ImEvent> {
        self.events
            .lock()
            .map(|mut events| {
                drain_events_with_quarantine(&mut events, &self.quarantine_until, Instant::now())
            })
            .unwrap_or_default()
    }

    fn discard_queued_events(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
    }

    fn begin_quarantine(&self) {
        self.quarantine_until
            .set(Some(Instant::now() + QUARANTINE_WINDOW));
        self.discard_queued_events();
        let _ = self
            .proxy_with(QUARANTINE_RESET_TIMEOUT)
            .method_call::<(), _, _, _>(self.ic_iface(), "Reset", ());
    }

    pub(crate) fn process_key(
        &mut self,
        keyval: u32,
        keycode: u32,
        state: u32,
        is_release: bool,
        time: u32,
        warm: bool,
    ) -> Result<bool, String> {
        if self.quarantine_until.get().is_some() {
            self.process_io();
            self.discard_queued_events();
        }
        let modifier = is_modifier_keyval(keyval);
        let timeout = if modifier {
            MODIFIER_TIMEOUT
        } else if warm {
            CONNECT_TIMEOUT
        } else {
            KEY_TIMEOUT
        };
        let started = Instant::now();
        let result = match self.kind {
            ImKind::Fcitx5 => {
                self.process_fcitx5_key(keyval, keycode, state, is_release, time, timeout)
            }
            ImKind::IBus => self
                .proxy_with(timeout)
                .method_call(
                    IBUS_IC_IFACE,
                    "ProcessKeyEvent",
                    (
                        keyval,
                        keycode.saturating_sub(8),
                        ibus_key_state(state, is_release),
                    ),
                )
                .map(|(handled,): (bool,)| handled)
                .map_err(|error| error.to_string()),
        };
        let call_elapsed = started.elapsed();
        if !modifier {
            self.slow_daemon.set(call_elapsed >= SLOW_RESPONSE);
        }
        self.process_io();
        if !is_release
            && !modifier
            && keyval != 0xff1b
            && !self.slow_daemon.get()
            && result.as_ref().is_ok_and(|filtered| *filtered)
        {
            if !self.has_events() {
                let budget = remaining_key_budget(started.elapsed(), SIGNAL_WAIT);
                if !budget.is_zero() {
                    self.wait_for_events(budget);
                    self.process_io();
                }
            }
            if self.only_clears_preedit() {
                let budget = remaining_key_budget(started.elapsed(), COMMIT_WAIT);
                if !budget.is_zero() {
                    self.wait_for_text_event(budget);
                    self.process_io();
                }
            }
        }
        match result {
            Ok(filtered) => {
                self.fail_count.set(0);
                Ok(filtered)
            }
            Err(error) => {
                if !modifier {
                    self.fail_count.set(self.fail_count.get().saturating_add(1));
                    self.begin_quarantine();
                }
                Err(error)
            }
        }
    }

    fn process_fcitx5_key(
        &self,
        keyval: u32,
        keycode: u32,
        state: u32,
        is_release: bool,
        time: u32,
        timeout: Duration,
    ) -> Result<bool, String> {
        match self.fcitx_key_mode.get() {
            FcitxKeyMode::Batch => self
                .process_fcitx5_batch(keyval, keycode, state, is_release, time, timeout)
                .map_err(|error| error.to_string()),
            FcitxKeyMode::Single => {
                self.process_fcitx5_single(keyval, keycode, state, is_release, time, timeout)
            }
            FcitxKeyMode::Unknown => {
                match self.process_fcitx5_batch(keyval, keycode, state, is_release, time, timeout) {
                    Ok(handled) => {
                        self.fcitx_key_mode.set(FcitxKeyMode::Batch);
                        Ok(handled)
                    }
                    Err(error) if is_unknown_dbus_method(&error) => {
                        self.fcitx_key_mode.set(FcitxKeyMode::Single);
                        self.process_fcitx5_single(
                            keyval, keycode, state, is_release, time, timeout,
                        )
                    }
                    Err(error) => Err(error.to_string()),
                }
            }
        }
    }

    fn process_fcitx5_batch(
        &self,
        keyval: u32,
        keycode: u32,
        state: u32,
        is_release: bool,
        time: u32,
        timeout: Duration,
    ) -> Result<bool, dbus::Error> {
        let (rows, handled): (Vec<(u32, Variant<Box<dyn RefArg>>)>, bool) =
            self.proxy_with(timeout).method_call(
                FCITX5_IC_IFACE,
                "ProcessKeyEventBatch",
                (keyval, keycode, state, is_release, time),
            )?;
        let mut incoming = Vec::new();
        for (kind, variant) in rows {
            if let Some(event) = parse_fcitx5_batch_item(kind, variant.0.as_ref()) {
                incoming.push(event);
            }
        }
        self.push_events(incoming);
        Ok(handled)
    }

    fn process_fcitx5_single(
        &self,
        keyval: u32,
        keycode: u32,
        state: u32,
        is_release: bool,
        time: u32,
        timeout: Duration,
    ) -> Result<bool, String> {
        self.proxy_with(timeout)
            .method_call(
                FCITX5_IC_IFACE,
                "ProcessKeyEvent",
                (keyval, keycode, state, is_release, time),
            )
            .map(|(handled,): (bool,)| handled)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn is_dead(&self) -> bool {
        self.fail_count.get() >= DEAD_AFTER_FAILS
    }

    pub(crate) fn set_surrounding(&self, surrounding: &ImeSurroundingText) {
        let cursor = surrounding.cursor_chars();
        let anchor = surrounding.anchor_chars();
        let result = match self.kind {
            ImKind::Fcitx5 => self
                .proxy()
                .method_call(
                    FCITX5_IC_IFACE,
                    "SetSurroundingText",
                    (surrounding.text.as_str(), cursor, anchor),
                )
                .map(|(): ()| ()),
            ImKind::IBus => self.set_ibus_surrounding(&surrounding.text, cursor, anchor),
        };
        self.note_result(result);
    }

    fn set_ibus_surrounding(
        &self,
        text: &str,
        cursor: u32,
        anchor: u32,
    ) -> Result<(), dbus::Error> {
        match self.ibus_surrounding.get() {
            IbusSurroundingEnc::IBusText => self.call_ibus_surrounding_text(text, cursor, anchor),
            IbusSurroundingEnc::PlainVariant => {
                self.call_ibus_surrounding_plain(text, cursor, anchor)
            }
            IbusSurroundingEnc::Unknown => {
                match self.call_ibus_surrounding_text(text, cursor, anchor) {
                    Ok(()) => {
                        self.ibus_surrounding.set(IbusSurroundingEnc::IBusText);
                        Ok(())
                    }
                    Err(error)
                        if is_unknown_dbus_method(&error) || is_invalid_dbus_args(&error) =>
                    {
                        let result = self.call_ibus_surrounding_plain(text, cursor, anchor);
                        if result.is_ok() {
                            self.ibus_surrounding.set(IbusSurroundingEnc::PlainVariant);
                        }
                        result
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

    fn call_ibus_surrounding_text(
        &self,
        text: &str,
        cursor: u32,
        anchor: u32,
    ) -> Result<(), dbus::Error> {
        self.proxy().method_call(
            IBUS_IC_IFACE,
            "SetSurroundingText",
            (ibus_text_variant(text), cursor, anchor),
        )
    }

    fn call_ibus_surrounding_plain(
        &self,
        text: &str,
        cursor: u32,
        anchor: u32,
    ) -> Result<(), dbus::Error> {
        self.proxy().method_call(
            IBUS_IC_IFACE,
            "SetSurroundingText",
            (
                Variant(Box::new(text.to_string()) as Box<dyn RefArg>),
                cursor,
                anchor,
            ),
        )
    }

    pub(crate) fn set_cursor_rect(&self, x: i32, y: i32, width: i32, height: i32) {
        let result = match self.kind {
            ImKind::Fcitx5 => self
                .proxy()
                .method_call(FCITX5_IC_IFACE, "SetCursorRect", (x, y, width, height))
                .map(|(): ()| ()),
            ImKind::IBus => self
                .proxy()
                .method_call(IBUS_IC_IFACE, "SetCursorLocation", (x, y, width, height))
                .map(|(): ()| ()),
        };
        let _ = result;
    }

    pub(crate) fn focus_in(&self) {
        self.note_result(self.call_void_with("FocusIn", CONNECT_TIMEOUT));
    }

    pub(crate) fn focus_out(&self) {
        self.note_result(self.call_void_with("FocusOut", CONNECT_TIMEOUT));
    }

    pub(crate) fn reset(&self) {
        self.note_result(self.call_void("Reset"));
    }

    fn note_result<T, E>(&self, result: Result<T, E>) {
        match result {
            Ok(_) => self.fail_count.set(0),
            Err(_) => self.fail_count.set(self.fail_count.get().saturating_add(1)),
        }
    }

    fn call_void(&self, method: &str) -> Result<(), String> {
        self.call_void_with(method, KEY_TIMEOUT)
    }

    fn call_void_with(&self, method: &str, timeout: Duration) -> Result<(), String> {
        self.proxy_with(timeout)
            .method_call::<(), _, _, _>(self.ic_iface(), method, ())
            .map_err(|error| error.to_string())
    }

    fn ic_iface(&self) -> &'static str {
        match self.kind {
            ImKind::Fcitx5 => FCITX5_IC_IFACE,
            ImKind::IBus => IBUS_IC_IFACE,
        }
    }

    fn proxy(&self) -> Proxy<'_, &Connection> {
        self.proxy_with(KEY_TIMEOUT)
    }

    fn proxy_with(&self, timeout: Duration) -> Proxy<'_, &Connection> {
        self.conn.with_proxy(&self.dest, self.path.clone(), timeout)
    }
}

impl Drop for X11ImContext {
    fn drop(&mut self) {
        for token in self.tokens.drain(..) {
            self.conn.stop_receive(token);
        }
        let proxy = self
            .conn
            .with_proxy(&self.dest, self.path.clone(), DESTROY_TIMEOUT);
        let _ = match self.kind {
            ImKind::Fcitx5 => proxy.method_call::<(), _, _, _>(FCITX5_IC_IFACE, "DestroyIC", ()),
            ImKind::IBus => proxy.method_call::<(), _, _, _>(IBUS_SERVICE_IFACE, "Destroy", ()),
        };
    }
}

fn is_modifier_keyval(keyval: u32) -> bool {
    matches!(keyval, 0xffe1..=0xffee | 0xfe03 | 0xfe11 | 0xff7e)
}

fn is_unknown_dbus_method(error: &dbus::Error) -> bool {
    matches!(
        error.name(),
        Some("org.freedesktop.DBus.Error.UnknownMethod")
            | Some("org.freedesktop.DBus.Error.UnknownInterface")
    )
}

fn is_invalid_dbus_args(error: &dbus::Error) -> bool {
    error.name() == Some("org.freedesktop.DBus.Error.InvalidArgs")
}

fn preferred_backends() -> Vec<ImKind> {
    backends_from_env_values(
        &env_lower("XMODIFIERS"),
        &env_lower("GTK_IM_MODULE"),
        &env_lower("QT_IM_MODULE"),
    )
}

fn push_backend(backends: &mut Vec<ImKind>, kind: ImKind) {
    if !backends.contains(&kind) {
        backends.push(kind);
    }
}

fn backends_from_env_values(modifiers: &str, gtk: &str, qt: &str) -> Vec<ImKind> {
    let mut backends = Vec::new();
    let mentions_fcitx = [modifiers, gtk, qt]
        .iter()
        .any(|value| value.contains("fcitx"));
    let mentions_ibus = [modifiers, gtk, qt]
        .iter()
        .any(|value| value.contains("ibus"));
    if mentions_fcitx {
        push_backend(&mut backends, ImKind::IBus);
        push_backend(&mut backends, ImKind::Fcitx5);
    }
    if mentions_ibus {
        push_backend(&mut backends, ImKind::IBus);
    }
    if backends.is_empty() {
        push_backend(&mut backends, ImKind::Fcitx5);
        push_backend(&mut backends, ImKind::IBus);
    }
    backends
}

fn connect_order(preferred: &[ImKind]) -> Vec<ImKind> {
    let mut order = preferred.to_vec();
    for kind in [ImKind::Fcitx5, ImKind::IBus] {
        if !order.contains(&kind) {
            order.push(kind);
        }
    }
    order
}

pub(crate) fn ibus_key_state(state: u32, is_release: bool) -> u32 {
    if is_release {
        state | IBUS_RELEASE_MASK
    } else {
        state & !IBUS_RELEASE_MASK
    }
}

fn ibus_state_is_release(state: u32) -> bool {
    state & IBUS_RELEASE_MASK != 0
}

fn env_lower(key: &str) -> String {
    std::env::var(key).unwrap_or_default().to_ascii_lowercase()
}

fn connect_fcitx5() -> Result<X11ImContext, String> {
    let (conn, watch) = open_session()?;
    if !name_has_owner(&conn, FCITX5_DEST) {
        return Err("org.fcitx.Fcitx5 is not on the session bus".into());
    }
    let proxy = conn.with_proxy(FCITX5_DEST, FCITX5_IM_PATH, CONNECT_TIMEOUT);
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let args: Vec<(String, String)> = vec![
        ("program".into(), "mezon".into()),
        ("display".into(), display),
    ];
    let (path, _uuid): (DbusPath<'static>, Vec<u8>) = proxy
        .method_call(FCITX5_IM_IFACE, "CreateInputContext", (args,))
        .map_err(|error| error.to_string())?;
    let ic = conn.with_proxy(FCITX5_DEST, path.clone(), CONNECT_TIMEOUT);
    let _ =
        ic.method_call::<(), _, _, _>(FCITX5_IC_IFACE, "SetSupportedCapability", (FCITX5_CAPS,));
    ic.method_call::<(), _, _, _>(FCITX5_IC_IFACE, "SetCapability", (FCITX5_CAPS,))
        .map_err(|error| error.to_string())?;
    finish_context(
        conn,
        FCITX5_DEST.to_string(),
        path,
        ImKind::Fcitx5,
        watch,
        false,
    )
}

fn connect_ibus() -> Result<X11ImContext, String> {
    let (conn, dest, watch) = ibus_connection()?;
    let fcitx_shim = name_has_owner(&conn, FCITX5_DEST);
    let proxy = conn.with_proxy(&dest, IBUS_PATH, CONNECT_TIMEOUT);
    let (path,): (DbusPath<'static>,) = proxy
        .method_call(IBUS_IFACE, "CreateInputContext", ("mezon",))
        .map_err(|error| error.to_string())?;
    let ic = conn.with_proxy(&dest, path.clone(), CONNECT_TIMEOUT);
    ic.method_call::<(), _, _, _>(IBUS_IC_IFACE, "SetCapabilities", (IBUS_CAPS,))
        .map_err(|error| error.to_string())?;
    finish_context(conn, dest, path, ImKind::IBus, watch, fcitx_shim)
}

fn ibus_connection() -> Result<(Connection, String, RawFd), String> {
    if let Ok(address) = std::env::var("IBUS_ADDRESS")
        && !address.is_empty()
    {
        let (conn, watch) = open_address(&address)?;
        return Ok((conn, IBUS_DEST.to_string(), watch));
    }
    if let Ok(session) = Connection::new_session()
        && name_has_owner(&session, IBUS_DEST)
    {
        let address = session
            .with_proxy(IBUS_DEST, IBUS_PATH, CONNECT_TIMEOUT)
            .method_call(IBUS_IFACE, "GetAddress", ())
            .ok()
            .map(|(address,): (String,)| address)
            .unwrap_or_default();
        if !address.is_empty()
            && let Ok((private, watch)) = open_address(&address)
        {
            return Ok((private, IBUS_DEST.to_string(), watch));
        }
    }
    if let Some(address) = ibus_address_from_bus_file() {
        let (conn, watch) = open_address(&address)?;
        return Ok((conn, IBUS_DEST.to_string(), watch));
    }
    Err("ibus daemon address not found".into())
}

fn open_session() -> Result<(Connection, RawFd), String> {
    watched_connection(Channel::get_private(BusType::Session).map_err(|error| error.to_string())?)
}

fn open_address(address: &str) -> Result<(Connection, RawFd), String> {
    let decoded = percent_decode_dbus_address(address);
    let mut channel = Channel::open_private(&decoded)
        .or_else(|_| Channel::open_private(address))
        .map_err(|error| error.to_string())?;
    channel.register().map_err(|error| error.to_string())?;
    watched_connection(channel)
}

fn watched_connection(mut channel: Channel) -> Result<(Connection, RawFd), String> {
    let watch = dbus_watch_fd(&mut channel).ok_or_else(|| "dbus watch fd missing".to_string())?;
    Ok((Connection::from(channel), watch))
}

fn dbus_watch_fd(channel: &mut Channel) -> Option<RawFd> {
    channel.set_watch_enabled(true);
    #[allow(deprecated)]
    let watches = channel.watch_fds().ok()?;
    channel.set_watch_enabled(true);
    watches
        .iter()
        .find(|watch| watch.read)
        .or_else(|| watches.first())
        .map(|watch| watch.fd)
}

fn percent_decode_dbus_address(address: &str) -> String {
    let bytes = address.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3])
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| address.to_string())
}

fn ibus_address_from_bus_file() -> Option<String> {
    let machine = std::fs::read_to_string("/etc/machine-id")
        .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
        .ok()?;
    let machine = machine.trim();
    let display = std::env::var("DISPLAY").ok()?;
    let display_no = display
        .rsplit_once(':')
        .and_then(|(_, rest)| rest.split('.').next())
        .unwrap_or("0");
    let file_name = format!("{machine}-unix-{display_no}");
    let mut paths = Vec::new();
    if let Ok(config) = std::env::var("XDG_CONFIG_HOME") {
        paths.push(
            std::path::PathBuf::from(config)
                .join("ibus/bus")
                .join(&file_name),
        );
    }
    if let Ok(home) = std::env::var("HOME") {
        paths.push(
            std::path::PathBuf::from(home)
                .join(".config/ibus/bus")
                .join(&file_name),
        );
    }
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        paths.push(
            std::path::PathBuf::from(runtime)
                .join("ibus/bus")
                .join(&file_name),
        );
    }
    for path in paths {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            for line in contents.lines() {
                if let Some(address) = line.strip_prefix("IBUS_ADDRESS=")
                    && !address.is_empty()
                {
                    return Some(address.to_string());
                }
            }
        }
    }
    None
}

fn name_has_owner(conn: &Connection, name: &str) -> bool {
    conn.with_proxy("org.freedesktop.DBus", "/", CONNECT_TIMEOUT)
        .method_call("org.freedesktop.DBus", "NameHasOwner", (name,))
        .ok()
        .map(|(owned,): (bool,)| owned)
        .unwrap_or(false)
}

fn finish_context(
    mut conn: Connection,
    dest: String,
    path: DbusPath<'static>,
    kind: ImKind,
    watch: RawFd,
    ibus_fcitx_shim: bool,
) -> Result<X11ImContext, String> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut tokens = Vec::new();
    match kind {
        ImKind::Fcitx5 => {
            tokens.push(add_signal(
                &mut conn,
                &path,
                FCITX5_IC_IFACE,
                "CommitString",
                Arc::clone(&events),
                parse_fcitx5_commit,
            )?);
            tokens.push(add_signal(
                &mut conn,
                &path,
                FCITX5_IC_IFACE,
                "UpdateFormattedPreedit",
                Arc::clone(&events),
                parse_fcitx5_preedit,
            )?);
            tokens.push(add_signal(
                &mut conn,
                &path,
                FCITX5_IC_IFACE,
                "DeleteSurroundingText",
                Arc::clone(&events),
                parse_delete_surrounding,
            )?);
            tokens.push(add_signal(
                &mut conn,
                &path,
                FCITX5_IC_IFACE,
                "ForwardKey",
                Arc::clone(&events),
                parse_fcitx5_forward,
            )?);
        }
        ImKind::IBus => {
            tokens.push(add_signal(
                &mut conn,
                &path,
                IBUS_IC_IFACE,
                "CommitText",
                Arc::clone(&events),
                parse_ibus_commit,
            )?);
            tokens.push(add_signal(
                &mut conn,
                &path,
                IBUS_IC_IFACE,
                "UpdatePreeditText",
                Arc::clone(&events),
                parse_ibus_preedit,
            )?);
            tokens.push(add_signal(
                &mut conn,
                &path,
                IBUS_IC_IFACE,
                "HidePreeditText",
                Arc::clone(&events),
                parse_ibus_hide_preedit,
            )?);
            tokens.push(add_signal(
                &mut conn,
                &path,
                IBUS_IC_IFACE,
                "DeleteSurroundingText",
                Arc::clone(&events),
                parse_delete_surrounding,
            )?);
            tokens.push(add_signal(
                &mut conn,
                &path,
                IBUS_IC_IFACE,
                "ForwardKeyEvent",
                Arc::clone(&events),
                parse_ibus_forward,
            )?);
        }
    }
    Ok(X11ImContext {
        conn,
        dest,
        path,
        kind,
        events,
        tokens,
        fail_count: Cell::new(0),
        ibus_surrounding: Cell::new(IbusSurroundingEnc::Unknown),
        fcitx_key_mode: Cell::new(FcitxKeyMode::Unknown),
        quarantine_until: Cell::new(None),
        slow_daemon: Cell::new(false),
        ibus_fcitx_shim,
        watch,
    })
}

fn add_signal(
    conn: &mut Connection,
    path: &DbusPath<'static>,
    iface: &'static str,
    member: &'static str,
    events: Arc<Mutex<Vec<ImEvent>>>,
    parse: fn(&Message) -> Option<ImEvent>,
) -> Result<Token, String> {
    let mut rule = MatchRule::new_signal(iface, member);
    rule.path = Some(path.clone());
    conn.add_match_no_cb(&rule.match_str())
        .map_err(|error| error.to_string())?;
    Ok(conn.start_receive(
        rule,
        Box::new(move |message, _| {
            if let Some(event) = parse(&message)
                && let Ok(mut events) = events.lock()
            {
                events.push(event);
            }
            true
        }),
    ))
}

fn parse_fcitx5_commit(message: &Message) -> Option<ImEvent> {
    let (text,): (String,) = message.read1().ok()?;
    Some(ImEvent::Commit(text))
}

fn parse_fcitx5_preedit(message: &Message) -> Option<ImEvent> {
    let (chunks, caret): (Vec<(String, i32)>, i32) = message.read2().ok()?;
    let text: String = chunks.into_iter().map(|(piece, _)| piece).collect();
    if text.is_empty() {
        return Some(ImEvent::ClearPreedit);
    }
    let caret_chars = fcitx_byte_caret_to_chars(&text, caret);
    Some(ImEvent::Preedit { text, caret_chars })
}

fn parse_fcitx5_batch_item(kind: u32, arg: &dyn RefArg) -> Option<ImEvent> {
    match kind {
        FCITX5_BATCH_COMMIT => refarg_string(arg).map(ImEvent::Commit),
        FCITX5_BATCH_PREEDIT => parse_fcitx5_batch_preedit(arg),
        FCITX5_BATCH_FORWARD => parse_fcitx5_batch_forward(arg),
        FCITX5_BATCH_DELETE => parse_fcitx5_batch_delete(arg),
        _ => None,
    }
}

fn parse_fcitx5_batch_preedit(arg: &dyn RefArg) -> Option<ImEvent> {
    let mut iter = arg.as_iter()?;
    let chunks = iter.next()?;
    let caret = iter.next().and_then(|value| value.as_i64()).unwrap_or(0) as i32;
    let text = formatted_preedit_text(chunks);
    if text.is_empty() {
        return Some(ImEvent::ClearPreedit);
    }
    let caret_chars = fcitx_byte_caret_to_chars(&text, caret);
    Some(ImEvent::Preedit { text, caret_chars })
}

fn parse_fcitx5_batch_forward(arg: &dyn RefArg) -> Option<ImEvent> {
    let mut iter = arg.as_iter()?;
    let keyval = iter.next()?.as_u64()? as u32;
    let state = iter.next()?.as_u64()? as u32;
    let is_release = iter
        .next()
        .and_then(|value| value.as_u64().or_else(|| value.as_i64().map(|n| n as u64)))
        .is_some_and(|value| value != 0);
    Some(ImEvent::ForwardKey {
        keyval,
        state,
        is_release,
    })
}

fn parse_fcitx5_batch_delete(arg: &dyn RefArg) -> Option<ImEvent> {
    let mut iter = arg.as_iter()?;
    let offset = iter.next()?.as_i64()? as i32;
    let nchars = iter.next()?.as_u64()? as u32;
    Some(ImEvent::DeleteSurrounding { offset, nchars })
}

fn formatted_preedit_text(arg: &dyn RefArg) -> String {
    let Some(iter) = arg.as_iter() else {
        return String::new();
    };
    let mut text = String::new();
    for piece in iter {
        if let Some(s) = piece.as_str() {
            text.push_str(s);
            continue;
        }
        if let Some(mut inner) = piece.as_iter()
            && let Some(s) = inner.next().and_then(|value| value.as_str())
        {
            text.push_str(s);
        }
    }
    text
}

fn refarg_string(arg: &dyn RefArg) -> Option<String> {
    if let Some(text) = arg.as_str() {
        return Some(text.to_string());
    }
    let mut iter = arg.as_iter()?;
    refarg_string(iter.next()?)
}

fn parse_delete_surrounding(message: &Message) -> Option<ImEvent> {
    let (offset, nchars): (i32, u32) = message.read2().ok()?;
    Some(ImEvent::DeleteSurrounding { offset, nchars })
}

fn parse_fcitx5_forward(message: &Message) -> Option<ImEvent> {
    let (keyval, state, is_release): (u32, u32, bool) = message.read3().ok()?;
    Some(ImEvent::ForwardKey {
        keyval,
        state,
        is_release,
    })
}

fn parse_ibus_commit(message: &Message) -> Option<ImEvent> {
    let text = first_ibus_string(message)?;
    Some(ImEvent::Commit(text))
}

fn parse_ibus_preedit(message: &Message) -> Option<ImEvent> {
    let mut iter = message.iter_init();
    let text = iter
        .get_refarg()
        .and_then(|arg| extract_ibus_string(arg.as_ref()))?;
    let _ = iter.next();
    let caret_chars = iter.get::<u32>().unwrap_or(0) as i32;
    let _ = iter.next();
    let visible = iter.get::<bool>().unwrap_or(true);
    ibus_preedit_event(text, caret_chars, visible)
}

fn ibus_preedit_event(text: String, caret_chars: i32, _visible: bool) -> Option<ImEvent> {
    if text.is_empty() {
        return Some(ImEvent::ClearPreedit);
    }
    Some(ImEvent::Preedit { text, caret_chars })
}

fn parse_ibus_hide_preedit(_message: &Message) -> Option<ImEvent> {
    Some(ImEvent::HidePreedit)
}

fn events_only_clear_preedit(events: &[ImEvent]) -> bool {
    !events.is_empty()
        && events.iter().all(|event| match event {
            ImEvent::ClearPreedit | ImEvent::HidePreedit => true,
            ImEvent::Preedit { text, .. } => text.is_empty(),
            _ => false,
        })
}

fn parse_ibus_forward(message: &Message) -> Option<ImEvent> {
    let (keyval, _keycode, state): (u32, u32, u32) = message.read3().ok()?;
    Some(ImEvent::ForwardKey {
        keyval,
        state,
        is_release: ibus_state_is_release(state),
    })
}

fn first_ibus_string(message: &Message) -> Option<String> {
    let mut iter = message.iter_init();
    loop {
        if let Some(arg) = iter.get_refarg()
            && let Some(text) = extract_ibus_string(arg.as_ref())
        {
            return Some(text);
        }
        if !iter.next() {
            return None;
        }
    }
}

fn extract_ibus_string(arg: &dyn RefArg) -> Option<String> {
    if let Some(text) = arg.as_str()
        && !matches!(text, "IBusText" | "IBusAttrList" | "IBusAttribute")
    {
        return Some(text.to_string());
    }
    if let Some(iter) = arg.as_iter() {
        for child in iter {
            if let Some(text) = extract_ibus_string(child) {
                return Some(text);
            }
        }
    }
    None
}

fn ibus_text_variant(text: &str) -> Variant<Box<dyn RefArg>> {
    let attrs: Variant<Box<dyn RefArg>> = Variant(Box::new((
        "IBusAttrList".to_string(),
        HashMap::<String, Variant<Box<dyn RefArg>>>::new(),
        Vec::<Variant<Box<dyn RefArg>>>::new(),
    )) as Box<dyn RefArg>);
    Variant(Box::new((
        "IBusText".to_string(),
        HashMap::<String, Variant<Box<dyn RefArg>>>::new(),
        text.to_string(),
        attrs,
    )) as Box<dyn RefArg>)
}

pub(crate) fn caret_utf16_range(text: &str, caret_chars: i32) -> Option<std::ops::Range<usize>> {
    if text.is_empty() {
        return None;
    }
    let char_len = text.chars().count();
    let caret = if caret_chars < 0 {
        char_len
    } else {
        usize::try_from(caret_chars).unwrap_or(0).min(char_len)
    };
    let utf16: usize = text.chars().take(caret).map(char::len_utf16).sum();
    Some(utf16..utf16)
}

pub(crate) fn surrounding_char_delete_to_bytes(
    text: &str,
    cursor: usize,
    offset: i32,
    nchars: u32,
) -> (usize, usize) {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return (0, 0);
    }
    let caret_chars = text[..cursor].chars().count() as i32;
    let start_chars = caret_chars.saturating_add(offset).max(0) as usize;
    let end_chars = start_chars.saturating_add(nchars as usize);
    let start_bytes = char_index_to_byte(text, start_chars);
    let end_bytes = char_index_to_byte(text, end_chars);
    if start_bytes >= end_bytes {
        return (0, 0);
    }
    if end_bytes == cursor {
        return (cursor - start_bytes, 0);
    }
    if start_bytes == cursor {
        return (0, end_bytes - cursor);
    }
    if start_bytes < cursor && end_bytes > cursor {
        return (cursor - start_bytes, end_bytes - cursor);
    }
    (0, 0)
}

fn remaining_key_budget(elapsed: Duration, wait: Duration) -> Duration {
    KEY_BUDGET.saturating_sub(elapsed).min(wait)
}

fn drain_events_with_quarantine(
    events: &mut Vec<ImEvent>,
    quarantine_until: &Cell<Option<Instant>>,
    now: Instant,
) -> Vec<ImEvent> {
    match quarantine_until.get() {
        Some(deadline) => {
            events.clear();
            if now >= deadline {
                quarantine_until.set(None);
            }
            Vec::new()
        }
        None => std::mem::take(events),
    }
}

fn fcitx_byte_caret_to_chars(text: &str, caret_bytes: i32) -> i32 {
    if caret_bytes < 0 {
        return -1;
    }
    let mut bytes = usize::try_from(caret_bytes).unwrap_or(0).min(text.len());
    while bytes > 0 && !text.is_char_boundary(bytes) {
        bytes -= 1;
    }
    text[..bytes].chars().count() as i32
}

fn char_index_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbus::arg::RefArg;

    #[test]
    fn delete_one_char_before_caret_is_the_last_vowel() {
        assert_eq!(surrounding_char_delete_to_bytes("hoa", 3, -1, 1), (1, 0));
    }

    #[test]
    fn delete_one_char_after_caret() {
        assert_eq!(surrounding_char_delete_to_bytes("abcd", 0, 0, 1), (0, 1));
    }

    #[test]
    fn delete_range_spanning_caret() {
        assert_eq!(surrounding_char_delete_to_bytes("abcd", 2, -1, 2), (1, 1));
    }

    #[test]
    fn delete_disjoint_before_caret_is_rejected() {
        assert_eq!(surrounding_char_delete_to_bytes("abcd", 4, -4, 1), (0, 0));
    }

    #[test]
    fn delete_disjoint_after_caret_is_rejected() {
        assert_eq!(surrounding_char_delete_to_bytes("abcd", 0, 2, 1), (0, 0));
    }

    #[test]
    fn delete_spans_a_vietnamese_vowel() {
        let text = "hoá";
        let cursor = text.len();
        let (before, after) = surrounding_char_delete_to_bytes(text, cursor, -1, 1);
        assert_eq!(after, 0);
        assert_eq!(&text[cursor - before..cursor], "á");
    }

    #[test]
    fn fcitx_byte_caret_mid_vietnamese_preedit() {
        let text = "hóa";
        assert_eq!(fcitx_byte_caret_to_chars(text, 3), 2);
        assert_eq!(
            caret_utf16_range(text, fcitx_byte_caret_to_chars(text, 3)),
            Some(2..2)
        );
        assert_eq!(fcitx_byte_caret_to_chars(text, text.len() as i32), 3);
    }

    #[test]
    fn quarantine_drops_late_events_across_the_next_key() {
        let start = Instant::now();
        let quarantine = Cell::new(Some(start + QUARANTINE_WINDOW));
        let mut events = Vec::new();

        assert!(drain_events_with_quarantine(&mut events, &quarantine, start).is_empty());
        assert!(quarantine.get().is_some());

        events.push(ImEvent::Commit("a".into()));
        assert!(
            drain_events_with_quarantine(&mut events, &quarantine, start).is_empty(),
            "a late commit from the timed-out key must be discarded"
        );
        assert!(quarantine.get().is_some());

        events.push(ImEvent::Commit("b".into()));
        assert!(
            drain_events_with_quarantine(
                &mut events,
                &quarantine,
                start + QUARANTINE_WINDOW + Duration::from_millis(1)
            )
            .is_empty(),
            "the queue is emptied while quarantined, so the window expires with no events"
        );
        assert!(quarantine.get().is_none());

        events.push(ImEvent::Commit("c".into()));
        assert_eq!(
            drain_events_with_quarantine(&mut events, &quarantine, Instant::now()).len(),
            1
        );
    }

    #[test]
    fn key_budget_caps_total_blocking_time() {
        assert_eq!(
            remaining_key_budget(Duration::ZERO, SIGNAL_WAIT),
            SIGNAL_WAIT
        );
        assert_eq!(
            remaining_key_budget(KEY_BUDGET - Duration::from_millis(4), COMMIT_WAIT),
            Duration::from_millis(4)
        );
        assert!(remaining_key_budget(KEY_TIMEOUT, COMMIT_WAIT).is_zero());
    }

    #[test]
    fn fcitx5_caps_exclude_password_and_include_key_order_fix() {
        assert_eq!(FCITX5_CAP_PREEDIT, 1 << 1);
        assert_eq!(FCITX5_CAP_FORMATTED_PREEDIT, 1 << 4);
        assert_eq!(FCITX5_CAP_CLIENT_UNFOCUS_COMMIT, 1 << 5);
        assert_eq!(FCITX5_CAP_SURROUNDING_TEXT, 1 << 6);
        assert_eq!(FCITX5_CAP_KEY_EVENT_ORDER_FIX, 1 << 37);
        assert_eq!(FCITX5_CAPS & (1 << 3), 0);
        assert_eq!(FCITX5_CAPS & FCITX5_CAP_KEY_EVENT_ORDER_FIX, 1 << 37);
    }

    #[test]
    fn env_fcitx_prefers_ibus_compat_then_native() {
        assert_eq!(
            backends_from_env_values("@im=fcitx", "", ""),
            vec![ImKind::IBus, ImKind::Fcitx5]
        );
        assert_eq!(
            connect_order(&[ImKind::IBus, ImKind::Fcitx5]),
            vec![ImKind::IBus, ImKind::Fcitx5]
        );
    }

    #[test]
    fn env_ibus_is_tried_first_then_fcitx() {
        assert_eq!(backends_from_env_values("", "ibus", ""), vec![ImKind::IBus]);
        assert_eq!(
            connect_order(&[ImKind::IBus]),
            vec![ImKind::IBus, ImKind::Fcitx5]
        );
    }

    #[test]
    fn empty_env_tries_fcitx_then_ibus() {
        assert_eq!(
            backends_from_env_values("", "", ""),
            vec![ImKind::Fcitx5, ImKind::IBus]
        );
    }

    #[test]
    fn ibus_hidden_preedit_keeps_text() {
        assert!(matches!(
            ibus_preedit_event("đứt".into(), 1, false),
            Some(ImEvent::Preedit { text, caret_chars })
                if text == "đứt" && caret_chars == 1
        ));
        assert!(matches!(
            ibus_preedit_event("đứt".into(), 3, true),
            Some(ImEvent::Preedit { text, .. }) if text == "đứt"
        ));
        assert!(matches!(
            ibus_preedit_event(String::new(), 0, false),
            Some(ImEvent::ClearPreedit)
        ));
        assert!(matches!(
            ibus_preedit_event(String::new(), 0, true),
            Some(ImEvent::ClearPreedit)
        ));
        let hide = Message::new_signal(
            "/",
            "org.freedesktop.IBus.InputContext",
            "HidePreeditText",
        )
        .unwrap();
        assert!(matches!(
            parse_ibus_hide_preedit(&hide),
            Some(ImEvent::HidePreedit)
        ));
    }

    #[test]
    fn hide_only_waits_for_following_preedit() {
        assert!(events_only_clear_preedit(&[ImEvent::HidePreedit]));
        assert!(events_only_clear_preedit(&[
            ImEvent::HidePreedit,
            ImEvent::ClearPreedit,
        ]));
        assert!(!events_only_clear_preedit(&[
            ImEvent::HidePreedit,
            ImEvent::Preedit {
                text: "t".into(),
                caret_chars: 1,
            },
        ]));
        assert!(!events_only_clear_preedit(&[ImEvent::Commit("được ".into())]));
        assert!(!events_only_clear_preedit(&[]));
    }

    #[test]
    fn ibus_release_mask_is_bit_30() {
        assert_eq!(ibus_key_state(0, true), IBUS_RELEASE_MASK);
        assert_eq!(ibus_key_state(1, false), 1);
        assert_eq!(ibus_key_state(IBUS_RELEASE_MASK | 1, false), 1);
        assert!(ibus_state_is_release(IBUS_RELEASE_MASK));
        assert!(!ibus_state_is_release(0));
    }

    #[test]
    fn caret_utf16_range_tracks_vietnamese_chars() {
        assert_eq!(caret_utf16_range("", 0), None);
        assert_eq!(caret_utf16_range("hoa", 3), Some(3..3));
        assert_eq!(caret_utf16_range("hoá", 2), Some(2..2));
        assert_eq!(caret_utf16_range("ab", -1), Some(2..2));
    }

    #[test]
    fn empty_fcitx_preedit_is_hide() {
        assert!(matches!(
            parse_fcitx5_batch_item(FCITX5_BATCH_COMMIT, &"được".to_string()),
            Some(ImEvent::Commit(text)) if text == "được"
        ));
        let empty_chunks: Vec<(String, i32)> = Vec::new();
        let text = formatted_preedit_text(&empty_chunks);
        assert!(text.is_empty());
    }

    #[test]
    fn extract_ibus_text_skips_type_names() {
        assert_eq!(
            extract_ibus_string(&"hello".to_string()),
            Some("hello".into())
        );
        assert_eq!(extract_ibus_string(&"IBusText".to_string()), None);
        let nested = (
            "IBusText".to_string(),
            HashMap::<String, Variant<Box<dyn RefArg>>>::new(),
            "hóa".to_string(),
        );
        assert_eq!(extract_ibus_string(&nested), Some("hóa".into()));
    }

    #[test]
    fn dbus_address_decodes_at_in_unix_path() {
        let encoded = "unix:path=/home/gia.chuvan%40nccsoft.office/.cache/ibus/dbus-abc";
        assert_eq!(
            percent_decode_dbus_address(encoded),
            "unix:path=/home/gia.chuvan@nccsoft.office/.cache/ibus/dbus-abc"
        );
    }
}
