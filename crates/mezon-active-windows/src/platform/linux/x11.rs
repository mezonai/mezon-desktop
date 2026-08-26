use crate::info::{ActiveWindowInfo, parse_wm_class};
use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::xproto::{Atom, ConnectionExt, GetPropertyReply, Window};

pub fn get_active_window_x11() -> anyhow::Result<ActiveWindowInfo> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|error| anyhow::anyhow!("X11 connection failed: {error}"))?;
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let root = screen.root;

    let active_window_atom = get_atom(&conn, "_NET_ACTIVE_WINDOW")?;
    let wm_pid_atom = get_atom(&conn, "_NET_WM_PID")?;
    let wm_class_atom = get_atom(&conn, "WM_CLASS")?;
    let wm_desktop_atom = get_atom(&conn, "_NET_WM_DESKTOP")?;
    let wm_window_type_atom = get_atom(&conn, "_NET_WM_WINDOW_TYPE")?;

    let active_reply = conn
        .get_property(
            false,
            root,
            active_window_atom,
            x11rb::protocol::xproto::AtomEnum::WINDOW,
            0,
            1000,
        )
        .map_err(|error| anyhow::anyhow!("Failed to get active window property: {error}"))?
        .reply()
        .map_err(|error| anyhow::anyhow!("Property reply failed: {error}"))?;

    let active_window = get_long_val(&active_reply)
        .ok_or_else(|| anyhow::anyhow!("_NET_ACTIVE_WINDOW is empty"))?
        as Window;

    if active_window == 0 {
        return Err(anyhow::anyhow!("Active window is 0"));
    }

    let pid = window_pid(&conn, active_window, wm_pid_atom).unwrap_or(0);
    let window_class = window_class(&conn, active_window, wm_class_atom);

    let desktop = conn
        .get_property(
            false,
            active_window,
            wm_desktop_atom,
            x11rb::protocol::xproto::AtomEnum::CARDINAL,
            0,
            1000,
        )
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| get_long_val(&reply))
        .unwrap_or(0);

    let win_type = conn
        .get_property(
            false,
            active_window,
            wm_window_type_atom,
            x11rb::protocol::xproto::AtomEnum::ATOM,
            0,
            1000,
        )
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| get_long_val(&reply))
        .unwrap_or(0);

    let idle_time = conn
        .screensaver_query_info(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.ms_since_user_input / 1000)
        .unwrap_or(0);

    Ok(ActiveWindowInfo {
        os: "linux".to_string(),
        window_class,
        window_name: String::new(),
        window_desktop: desktop.to_string(),
        window_type: win_type.to_string(),
        window_pid: pid.to_string(),
        idle_time: idle_time.to_string(),
    })
}

fn window_pid<C: Connection>(conn: &C, window: Window, wm_pid_atom: Atom) -> Option<u32> {
    conn.get_property(
        false,
        window,
        wm_pid_atom,
        x11rb::protocol::xproto::AtomEnum::CARDINAL,
        0,
        1000,
    )
    .ok()
    .and_then(|cookie| cookie.reply().ok())
    .and_then(|reply| get_long_val(&reply))
}

fn window_class<C: Connection>(conn: &C, window: Window, wm_class_atom: Atom) -> String {
    let raw_wm_class = conn
        .get_property(
            false,
            window,
            wm_class_atom,
            x11rb::protocol::xproto::AtomEnum::STRING,
            0,
            1000,
        )
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| get_string_val(&reply))
        .unwrap_or_default();
    let (_, class_name) = parse_wm_class(&raw_wm_class);
    if class_name.is_empty() {
        raw_wm_class
    } else {
        class_name
    }
}

fn get_atom<C: Connection>(conn: &C, name: &str) -> anyhow::Result<Atom> {
    let reply = conn
        .intern_atom(false, name.as_bytes())
        .map_err(|error| anyhow::anyhow!("Intern atom failed: {error}"))?
        .reply()
        .map_err(|error| anyhow::anyhow!("Intern atom reply failed: {error}"))?;
    Ok(reply.atom)
}

fn get_long_val(reply: &GetPropertyReply) -> Option<u32> {
    if reply.value.len() >= 4 {
        Some(u32::from_ne_bytes([
            reply.value[0],
            reply.value[1],
            reply.value[2],
            reply.value[3],
        ]))
    } else {
        None
    }
}

fn get_string_val(reply: &GetPropertyReply) -> String {
    String::from_utf8_lossy(&reply.value).into_owned()
}
