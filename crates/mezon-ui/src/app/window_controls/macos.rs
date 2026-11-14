use block::ConcreteBlock;
use gpui::{App, Context, Window, div, prelude::*};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{Once, OnceLock};

use crate::theme::Theme;

const NS_KEY_DOWN_MASK: u64 = 1 << 10;
const NS_COMMAND_KEY_MASK: u64 = 1 << 20;
const NS_ALTERNATE_KEY_MASK: u64 = 1 << 19;
const NS_CONTROL_KEY_MASK: u64 = 1 << 18;
static INSTALL_SHORTCUTS: Once = Once::new();

#[allow(dead_code)]
struct EventMonitor(*mut Object);
unsafe impl Send for EventMonitor {}
unsafe impl Sync for EventMonitor {}

static EVENT_MONITOR: OnceLock<EventMonitor> = OnceLock::new();

pub fn render_controls<V: 'static>(
    _theme: &Theme,
    _window: &Window,
    _cx: &mut Context<V>,
) -> impl IntoElement {
    div()
}

pub fn hide_window(window: &Window) {
    window.hide_window();
}

pub fn hide_active_window(cx: &mut App) {
    let main_active =
        cx.active_window().is_some() && cx.active_window() == crate::app::main_window::handle(cx);
    update_active_window(cx, |window| hide_window(window));
    if main_active {
        super::note_main_window_hidden(cx);
    }
}

pub fn minimize_active_window(cx: &mut App) {
    update_active_window(cx, |window| window.minimize_window());
}

pub fn disable_window_fullscreen(window: &Window) {
    if let Some(view) = appkit_view(window) {
        disable_fullscreen(view);
    }
}

pub fn install_shortcuts(cx: &App) {
    INSTALL_SHORTCUTS.call_once(|| {
        let async_app = cx.to_async();
        let handler = ConcreteBlock::new(move |event: *mut Object| -> *mut Object {
            #[cfg(debug_assertions)]
            unsafe {
                let kc: u16 = msg_send![event, keyCode];
                if matches!(kc, 123..=126) {
                    eprintln!("[gpui_monitor] arrow keyCode={kc} (123=L 124=R 125=Down 126=Up)");
                }
            }
            if unsafe { handle_shortcut(event, &async_app) } {
                std::ptr::null_mut()
            } else {
                event
            }
        });
        let handler = handler.copy();

        unsafe {
            let monitor: *mut Object = msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: NS_KEY_DOWN_MASK handler: &*handler];
            if monitor.is_null() {
                tracing::warn!("Failed to install macOS shortcut monitor");
            } else {
                let _ = EVENT_MONITOR.set(EventMonitor(monitor));
            }
        }
    });
}

unsafe fn handle_shortcut(event: *mut Object, async_app: &gpui::AsyncApp) -> bool {
    let flags: u64 = msg_send![event, modifierFlags];
    if flags & NS_COMMAND_KEY_MASK == 0 {
        return false;
    }
    if flags & (NS_ALTERNATE_KEY_MASK | NS_CONTROL_KEY_MASK) != 0 {
        return false;
    }

    let chars: *mut Object = msg_send![event, charactersIgnoringModifiers];
    if chars.is_null() {
        return false;
    }
    let len: u64 = msg_send![chars, length];
    if len == 0 {
        return false;
    }
    let key: u16 = msg_send![chars, characterAtIndex: 0usize];
    let Ok(key) = u8::try_from(key) else {
        return false;
    };

    match key.to_ascii_lowercase() {
        b'w' => {
            let ns_window = unsafe { key_window() };
            unsafe {
                order_out(ns_window);
            }
            async_app.update(|cx| {
                if is_main_ns_window(ns_window, cx) {
                    super::note_main_window_hidden(cx);
                }
            });
            true
        }
        b'm' => {
            unsafe {
                miniaturize(key_window());
            }
            true
        }
        b'h' => {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![app, hide: std::ptr::null::<Object>()];
            true
        }
        b'q' => {
            async_app.update(|cx| cx.quit());
            true
        }
        _ => false,
    }
}

fn is_main_ns_window(ns_window: *mut Object, cx: &mut App) -> bool {
    if ns_window.is_null() {
        return false;
    }
    let Some(handle) = crate::app::main_window::handle(cx) else {
        return false;
    };
    handle
        .update(cx, |_, window, _| {
            let mut found = false;
            if let Some(view) = appkit_view(window) {
                with_ns_window(view, |main| found = main == ns_window);
            }
            found
        })
        .unwrap_or(false)
}

fn update_active_window(cx: &mut App, f: impl FnOnce(&mut Window)) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    let _ = handle.update(cx, |_, window, _| f(window));
}

unsafe fn key_window() -> *mut Object {
    let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
    msg_send![app, keyWindow]
}

unsafe fn order_out(ns_window: *mut Object) {
    if ns_window.is_null() {
        return;
    }
    let _: () = msg_send![ns_window, orderOut: std::ptr::null::<Object>()];
}

unsafe fn miniaturize(ns_window: *mut Object) {
    if ns_window.is_null() {
        return;
    }
    let _: () = msg_send![ns_window, miniaturize: std::ptr::null::<Object>()];
}

fn appkit_view(window: &Window) -> Option<*mut std::ffi::c_void> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    Some(appkit.ns_view.as_ptr())
}

fn with_ns_window(native_view: *mut std::ffi::c_void, f: impl FnOnce(*mut Object)) {
    if native_view.is_null() {
        return;
    }

    unsafe {
        let native_view = native_view.cast::<Object>();
        let ns_window: *mut Object = msg_send![native_view, window];
        if ns_window.is_null() {
            return;
        }
        f(ns_window);
    }
}

fn disable_fullscreen(native_view: *mut std::ffi::c_void) {
    const FULLSCREEN_PRIMARY: u64 = 1 << 7;
    const FULLSCREEN_AUXILIARY: u64 = 1 << 8;
    const FULLSCREEN_NONE: u64 = 1 << 9;
    const FULLSCREEN_STYLE_MASK: u64 = 1 << 14;

    with_ns_window(native_view, |window| unsafe {
        let style_mask: u64 = msg_send![window, styleMask];
        if style_mask & FULLSCREEN_STYLE_MASK != 0 {
            let _: () = msg_send![window, toggleFullScreen: std::ptr::null::<Object>()];
        }

        let current: u64 = msg_send![window, collectionBehavior];
        let behavior = (current & !(FULLSCREEN_PRIMARY | FULLSCREEN_NONE)) | FULLSCREEN_AUXILIARY;
        let _: () = msg_send![window, setCollectionBehavior: behavior];
    });
}
