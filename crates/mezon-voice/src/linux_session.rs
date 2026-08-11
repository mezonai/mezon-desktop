use std::sync::atomic::{AtomicBool, Ordering};

static WAYLAND_SESSION: AtomicBool = AtomicBool::new(false);

pub fn record_wayland_session() -> bool {
    let is_wayland = environment_is_wayland_session();
    if is_wayland {
        WAYLAND_SESSION.store(true, Ordering::Relaxed);
    }
    is_wayland
}

pub(crate) fn is_wayland_session() -> bool {
    WAYLAND_SESSION.load(Ordering::Relaxed) || environment_is_wayland_session()
}

fn environment_is_wayland_session() -> bool {
    std::env::var("MEZON_WAYLAND_SESSION").is_ok_and(|value| value == "1")
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty())
        || std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|session| session.trim().eq_ignore_ascii_case("wayland"))
}
