use gpui::Window;

#[cfg(target_os = "macos")]
pub fn record_window_id(window: &Window) -> Option<u64> {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    let view = appkit.ns_view.as_ptr() as *mut Object;
    unsafe {
        let ns_window: *mut Object = msg_send![view, window];
        if ns_window.is_null() {
            return None;
        }
        let number: isize = msg_send![ns_window, windowNumber];
        (number > 0).then_some(number as u64)
    }
}

#[cfg(target_os = "windows")]
pub fn record_window_id(window: &Window) -> Option<u64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return None;
    };
    Some(win32.hwnd.get() as u64)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn record_window_id(window: &Window) -> Option<u64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Xcb(xcb) => Some(xcb.window.get() as u64),
        RawWindowHandle::Xlib(xlib) => Some(xlib.window as u64),
        _ => None,
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd"
)))]
pub fn record_window_id(_window: &Window) -> Option<u64> {
    None
}
