//! X11 startup notification for the browser Mezon opens.
//!
//! Mezon runs under X11 (XWayland on a Wayland session), so it cannot hand the
//! browser a Wayland activation token of its own. Without one, GNOME opens the
//! link in a browser window it leaves in the background. Mutter (since 42) also
//! takes an X11 startup-notification ID as an activation token and raises the
//! window when the ID's timestamp is newer than the last user input, which is
//! how GTK apps under XWayland bring the browser forward.

use std::sync::atomic::{AtomicU32, Ordering};

use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

/// Announces a launch to the window manager and returns the startup ID the
/// launched program passes back (`DESKTOP_STARTUP_ID` / `XDG_ACTIVATION_TOKEN`)
/// when it shows its window. `None` when there is no X display to announce on.
pub(crate) fn announce_launch() -> Option<String> {
    std::env::var_os("DISPLAY")?;
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let window = conn.generate_id().ok()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        -1,
        -1,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .ok()?;
    let cookies = [
        conn.intern_atom(false, b"_MEZON_LAUNCH_TIME").ok()?,
        conn.intern_atom(false, b"_NET_STARTUP_INFO_BEGIN").ok()?,
        conn.intern_atom(false, b"_NET_STARTUP_INFO").ok()?,
    ];
    let mut atoms = [0; 3];
    for (atom, cookie) in atoms.iter_mut().zip(cookies) {
        *atom = cookie.reply().ok()?.atom;
    }
    let [stamp, begin, more] = atoms;

    // The X server stamps the PropertyNotify with its current time, which is
    // after the click that asked for the link.
    conn.change_property8(PropMode::REPLACE, window, stamp, AtomEnum::STRING, &[])
        .ok()?;
    conn.flush().ok()?;
    let time = loop {
        if let Event::PropertyNotify(event) = conn.wait_for_event().ok()?
            && event.window == window
        {
            break event.time;
        }
    };

    static LAUNCHES: AtomicU32 = AtomicU32::new(0);
    let id = startup_id(
        std::process::id(),
        LAUNCHES.fetch_add(1, Ordering::Relaxed),
        time,
    );
    for (index, data) in new_sequence_message(&id, screen).into_iter().enumerate() {
        let kind = if index == 0 { begin } else { more };
        let event = ClientMessageEvent::new(8, window, kind, data);
        conn.send_event(false, root, EventMask::PROPERTY_CHANGE, event)
            .ok()?;
    }
    conn.destroy_window(window).ok()?;
    conn.get_input_focus().ok()?.reply().ok()?;
    Some(id)
}

/// libstartup-notification reads the sequence timestamp from the `_TIME` part.
fn startup_id(pid: u32, launch: u32, time: u32) -> String {
    format!("mezon-{pid}-{launch}_TIME{time}")
}

/// The spec's `new:` message, NUL-terminated and split into the 20-byte pieces
/// of `_NET_STARTUP_INFO_BEGIN` followed by `_NET_STARTUP_INFO` client messages.
fn new_sequence_message(id: &str, screen: usize) -> Vec<[u8; 20]> {
    let message = format!("new: ID={id} NAME=Browser SCREEN={screen}\0");
    message
        .as_bytes()
        .chunks(20)
        .map(|chunk| {
            let mut data = [0; 20];
            data[..chunk.len()].copy_from_slice(chunk);
            data
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_id_carries_the_x_server_time() {
        assert_eq!(startup_id(4242, 3, 1056050), "mezon-4242-3_TIME1056050");
    }

    #[test]
    fn new_message_is_split_into_nul_terminated_20_byte_pieces() {
        let id = startup_id(4242, 0, 1056050);
        let pieces = new_sequence_message(&id, 0);
        let bytes: Vec<u8> = pieces.concat();
        let end = bytes.iter().position(|&b| b == 0).expect("NUL terminator");
        assert_eq!(
            std::str::from_utf8(&bytes[..end]).unwrap(),
            "new: ID=mezon-4242-0_TIME1056050 NAME=Browser SCREEN=0"
        );
        assert_eq!(pieces.len(), (end + 1).div_ceil(20));
        assert!(bytes[end..].iter().all(|&b| b == 0));
    }
}
