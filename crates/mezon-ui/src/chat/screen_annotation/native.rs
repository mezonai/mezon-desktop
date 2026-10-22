use mezon_store::{PickedScreen, VoiceStore};

pub(crate) use platform::{NativeWindow, ShareTarget, native_window};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativeRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NativeRect {
    pub(crate) fn differs_from(&self, other: &NativeRect) -> bool {
        (self.x - other.x).abs() > 0.5
            || (self.y - other.y).abs() > 0.5
            || (self.width - other.width).abs() > 0.5
            || (self.height - other.height).abs() > 0.5
    }
}

pub(crate) fn share_target(store: &VoiceStore) -> Option<ShareTarget> {
    match store.shared_screen()? {
        PickedScreen::Target(target) => ShareTarget::from_scap(target),
        #[cfg(target_os = "linux")]
        PickedScreen::LinuxPortal => store.shared_monitor().map(ShareTarget::from_monitor),
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;

    use core_foundation::array::{CFArray, CFArrayGetValueAtIndex};
    use core_foundation::base::TCFType;
    use core_foundation::boolean::{CFBoolean, CFBooleanRef};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::CFString;
    use gpui::Window;
    use objc::runtime::{BOOL, NO, Object, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::NativeRect;

    const CG_WINDOW_LIST_OPTION_INCLUDING_WINDOW: u32 = 1 << 3;
    const NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL: u64 = 1 << 7;
    const NS_WINDOW_SHARING_NONE: u64 = 0;
    const OVERLAY_LEVEL: i64 = 101;
    const TOOLBAR_LEVEL: i64 = 102;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect)
        -> bool;
        fn CGDisplayBounds(display: u32) -> CGRect;
        fn CGMainDisplayID() -> u32;
    }

    fn native_rect(rect: CGRect) -> NativeRect {
        NativeRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub(crate) enum ShareTarget {
        Display(u32),
        Window(u32),
    }

    impl ShareTarget {
        pub(crate) fn from_scap(target: &scap::Target) -> Option<Self> {
            Some(match target {
                scap::Target::Display(display) => Self::Display(display.id),
                scap::Target::Window(window) => Self::Window(window.id),
            })
        }

        pub(crate) fn is_window(&self) -> bool {
            matches!(self, Self::Window(_))
        }

        pub(crate) fn rect(&self, _via: NativeWindow) -> Option<NativeRect> {
            match *self {
                Self::Display(id) => {
                    let bounds = unsafe { CGDisplayBounds(id) };
                    (bounds.width > 0.0 && bounds.height > 0.0).then(|| native_rect(bounds))
                }
                Self::Window(id) => onscreen_window_bounds(id),
            }
        }
    }

    fn onscreen_window_bounds(id: u32) -> Option<NativeRect> {
        let raw = unsafe { CGWindowListCopyWindowInfo(CG_WINDOW_LIST_OPTION_INCLUDING_WINDOW, id) };
        if raw.is_null() {
            return None;
        }
        let list: CFArray = unsafe { TCFType::wrap_under_create_rule(raw as _) };
        if list.is_empty() {
            return None;
        }
        let entry = unsafe { CFArrayGetValueAtIndex(list.as_concrete_TypeRef(), 0) };
        if entry.is_null() {
            return None;
        }
        let dict: CFDictionary = unsafe { TCFType::wrap_under_get_rule(entry as _) };
        let onscreen_key = CFString::new("kCGWindowIsOnscreen");
        let onscreen = dict
            .find(onscreen_key.as_concrete_TypeRef() as *const c_void)
            .map(|value| {
                bool::from(unsafe { CFBoolean::wrap_under_get_rule(*value as CFBooleanRef) })
            })
            .unwrap_or(false);
        if !onscreen {
            return None;
        }
        let bounds_key = CFString::new("kCGWindowBounds");
        let value = dict.find(bounds_key.as_concrete_TypeRef() as *const c_void)?;
        let mut rect = CGRect::default();
        unsafe { CGRectMakeWithDictionaryRepresentation(*value as CFDictionaryRef, &mut rect) }
            .then(|| native_rect(rect))
    }

    fn main_display_height() -> f64 {
        unsafe { CGDisplayBounds(CGMainDisplayID()) }.height
    }

    #[derive(Clone, Copy)]
    pub(crate) struct NativeWindow {
        window: usize,
        view: usize,
    }

    impl NativeWindow {
        fn ptr(self) -> *mut Object {
            self.window as *mut Object
        }

        fn view(self) -> *mut Object {
            self.view as *mut Object
        }

        pub(crate) fn scale(self, _window: &Window) -> f64 {
            1.0
        }

        pub(crate) fn frame(self) -> NativeRect {
            let frame: CGRect = unsafe { msg_send![self.ptr(), frame] };
            NativeRect {
                x: frame.x,
                y: main_display_height() - frame.y - frame.height,
                width: frame.width,
                height: frame.height,
            }
        }

        pub(crate) fn place(self, rect: NativeRect) {
            let frame = CGRect {
                x: rect.x,
                y: main_display_height() - rect.y - rect.height,
                width: rect.width,
                height: rect.height,
            };
            unsafe {
                let _: () = msg_send![self.ptr(), setFrame: frame display: YES];
            }
        }

        pub(crate) fn move_to(self, x: f64, y: f64) {
            let frame: CGRect = unsafe { msg_send![self.ptr(), frame] };
            let origin = CGPoint {
                x,
                y: main_display_height() - y - frame.height,
            };
            unsafe {
                let _: () = msg_send![self.ptr(), setFrameOrigin: origin];
            }
        }

        pub(crate) fn configure_overlay(self) {
            unsafe {
                let window = self.ptr();
                let _: () =
                    msg_send![window, setStyleMask: NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL];
                let _: BOOL = msg_send![window, makeFirstResponder: self.view()];
                let _: () = msg_send![window, setHasShadow: NO];
                let _: () = msg_send![window, setHidesOnDeactivate: NO];
                let _: () = msg_send![window, setLevel: OVERLAY_LEVEL];
            }
        }

        pub(crate) fn configure_toolbar(self, _overlay: NativeWindow) {
            unsafe {
                let window = self.ptr();
                let _: () =
                    msg_send![window, setStyleMask: NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL];
                let _: () = msg_send![window, setHidesOnDeactivate: NO];
                let _: () = msg_send![window, setLevel: TOOLBAR_LEVEL];
                let _: () = msg_send![window, setSharingType: NS_WINDOW_SHARING_NONE];
            }
        }

        pub(crate) fn cursor_position(self) -> Option<(f64, f64)> {
            let location: CGPoint = unsafe { msg_send![class!(NSEvent), mouseLocation] };
            let frame = self.frame();
            let y = main_display_height() - location.y;
            Some((location.x - frame.x, y - frame.y))
        }

        pub(crate) fn focus_keyboard(self) {
            unsafe {
                let is_key: BOOL = msg_send![self.ptr(), isKeyWindow];
                if is_key == NO {
                    let _: () = msg_send![self.ptr(), makeKeyWindow];
                }
                let responder: *mut Object = msg_send![self.ptr(), firstResponder];
                if responder != self.view() {
                    let _: BOOL = msg_send![self.ptr(), makeFirstResponder: self.view()];
                }
            }
        }

        pub(crate) fn set_click_through(self, enabled: bool) {
            let ignores = if enabled { YES } else { NO };
            unsafe {
                let _: () = msg_send![self.ptr(), setIgnoresMouseEvents: ignores];
            }
        }

        pub(crate) fn set_visible(self, visible: bool) {
            unsafe {
                if visible {
                    let _: () = msg_send![self.ptr(), orderFrontRegardless];
                } else {
                    let _: () = msg_send![self.ptr(), orderOut: std::ptr::null::<Object>()];
                }
            }
        }
    }

    pub(crate) fn native_window(window: &Window) -> Option<NativeWindow> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        let view = appkit.ns_view.as_ptr().cast::<Object>();
        let ns_window: *mut Object = unsafe { msg_send![view, window] };
        (!ns_window.is_null()).then_some(NativeWindow {
            window: ns_window as usize,
            view: view as usize,
        })
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO, ScreenToClient};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GWLP_HWNDPARENT, GetCursorPos, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST,
        IsIconic, IsWindowVisible, LWA_ALPHA, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetLayeredWindowAttributes, SetWindowDisplayAffinity,
        SetWindowLongPtrW, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WS_EX_LAYERED,
        WS_EX_TRANSPARENT,
    };

    use super::NativeRect;

    impl From<RECT> for NativeRect {
        fn from(rect: RECT) -> Self {
            NativeRect {
                x: rect.left as f64,
                y: rect.top as f64,
                width: (rect.right - rect.left) as f64,
                height: (rect.bottom - rect.top) as f64,
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub(crate) enum ShareTarget {
        Display(isize),
        Window(isize),
    }

    impl ShareTarget {
        pub(crate) fn from_scap(target: &scap::Target) -> Option<Self> {
            Some(match target {
                scap::Target::Display(display) => Self::Display(display.raw_handle.0 as isize),
                scap::Target::Window(window) => Self::Window(window.raw_handle.0 as isize),
            })
        }

        pub(crate) fn is_window(&self) -> bool {
            matches!(self, Self::Window(_))
        }

        pub(crate) fn rect(&self, _via: NativeWindow) -> Option<NativeRect> {
            match *self {
                Self::Display(monitor) => {
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    unsafe { GetMonitorInfoW(HMONITOR(monitor as *mut _), &mut info) }
                        .as_bool()
                        .then(|| info.rcMonitor.into())
                }
                Self::Window(hwnd) => visible_window_bounds(HWND(hwnd as *mut _)),
            }
        }
    }

    fn visible_window_bounds(hwnd: HWND) -> Option<NativeRect> {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { IsIconic(hwnd) }.as_bool() {
            return None;
        }
        let mut rect = RECT::default();
        let extended = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                std::ptr::from_mut(&mut rect).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
        };
        if extended.is_err() {
            unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
        }
        Some(rect.into())
    }

    #[derive(Clone, Copy)]
    pub(crate) struct NativeWindow(isize);

    impl NativeWindow {
        fn hwnd(self) -> HWND {
            HWND(self.0 as *mut _)
        }

        pub(crate) fn scale(self, window: &Window) -> f64 {
            window.scale_factor() as f64
        }

        pub(crate) fn frame(self) -> NativeRect {
            let mut rect = RECT::default();
            let _ = unsafe { GetWindowRect(self.hwnd(), &mut rect) };
            rect.into()
        }

        pub(crate) fn place(self, rect: NativeRect) {
            let _ = unsafe {
                SetWindowPos(
                    self.hwnd(),
                    Some(HWND_TOPMOST),
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    rect.width.round() as i32,
                    rect.height.round() as i32,
                    SWP_NOACTIVATE,
                )
            };
        }

        pub(crate) fn move_to(self, x: f64, y: f64) {
            let _ = unsafe {
                SetWindowPos(
                    self.hwnd(),
                    None,
                    x.round() as i32,
                    y.round() as i32,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
        }

        pub(crate) fn configure_overlay(self) {
            let hwnd = self.hwnd();
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
        }

        pub(crate) fn configure_toolbar(self, overlay: NativeWindow) {
            let hwnd = self.hwnd();
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, overlay.0);
                let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
                );
            }
        }

        pub(crate) fn set_click_through(self, enabled: bool) {
            let hwnd = self.hwnd();
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                let transparent = WS_EX_TRANSPARENT.0 as isize;
                let next = if enabled {
                    style | transparent
                } else {
                    style & !transparent
                };
                if next != style {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
                }
            }
        }

        pub(crate) fn cursor_position(self) -> Option<(f64, f64)> {
            let mut point = POINT::default();
            unsafe { GetCursorPos(&mut point) }.ok()?;
            if !unsafe { ScreenToClient(self.hwnd(), &mut point) }.as_bool() {
                return None;
            }
            Some((point.x as f64, point.y as f64))
        }

        pub(crate) fn focus_keyboard(self) {
            let _ = unsafe { SetFocus(Some(self.hwnd())) };
        }

        pub(crate) fn set_visible(self, visible: bool) {
            let _ = unsafe {
                ShowWindow(
                    self.hwnd(),
                    if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
                )
            };
        }
    }

    pub(crate) fn native_window(window: &Window) -> Option<NativeWindow> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return None;
        };
        Some(NativeWindow(win32.hwnd.get()))
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod platform {
    use gpui::Window;
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
    use x11rb::protocol::xproto::{
        ClipOrdering, ConfigureWindowAux, ConnectionExt as _, InputFocus, MapState, StackMode,
    };
    use x11rb::xcb_ffi::XCBConnection;

    use super::NativeRect;

    #[derive(Clone, Copy, Debug)]
    pub(crate) enum ShareTarget {
        Area(NativeRect),
        Window(u32),
    }

    impl ShareTarget {
        pub(crate) fn from_scap(target: &scap::Target) -> Option<Self> {
            Some(match target {
                scap::Target::Display(display) => Self::Area(NativeRect {
                    x: display.x_offset as f64,
                    y: display.y_offset as f64,
                    width: display.width as f64,
                    height: display.height as f64,
                }),
                scap::Target::Window(window) => {
                    Self::Window(xcb::Xid::resource_id(&window.raw_handle))
                }
            })
        }

        #[cfg(target_os = "linux")]
        pub(crate) fn from_monitor(monitor: mezon_voice::SharedMonitor) -> Self {
            Self::Area(NativeRect {
                x: monitor.x,
                y: monitor.y,
                width: monitor.width,
                height: monitor.height,
            })
        }

        pub(crate) fn is_window(&self) -> bool {
            matches!(self, Self::Window(_))
        }

        pub(crate) fn rect(&self, via: NativeWindow) -> Option<NativeRect> {
            match *self {
                Self::Area(rect) => Some(rect),
                Self::Window(xid) => via.with_connection(|conn, root| {
                    let attributes = conn.get_window_attributes(xid).ok()?.reply().ok()?;
                    if attributes.map_state != MapState::VIEWABLE {
                        return None;
                    }
                    window_rect(conn, root, xid)
                })?,
            }
        }
    }

    fn window_rect(conn: &XCBConnection, root: u32, xid: u32) -> Option<NativeRect> {
        let geometry = conn.get_geometry(xid).ok()?.reply().ok()?;
        let origin = conn
            .translate_coordinates(xid, root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some(NativeRect {
            x: origin.dst_x as f64,
            y: origin.dst_y as f64,
            width: geometry.width as f64,
            height: geometry.height as f64,
        })
    }

    #[derive(Clone, Copy)]
    pub(crate) struct NativeWindow {
        connection: usize,
        window: u32,
        root: u32,
    }

    impl NativeWindow {
        fn with_connection<R>(self, f: impl FnOnce(&XCBConnection, u32) -> R) -> Option<R> {
            let conn =
                unsafe { XCBConnection::from_raw_xcb_connection(self.connection as *mut _, false) }
                    .ok()?;
            let result = f(&conn, self.root);
            let _ = conn.flush();
            Some(result)
        }

        pub(crate) fn scale(self, window: &Window) -> f64 {
            window.scale_factor() as f64
        }

        pub(crate) fn frame(self) -> NativeRect {
            self.with_connection(|conn, root| window_rect(conn, root, self.window))
                .flatten()
                .unwrap_or(NativeRect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                })
        }

        pub(crate) fn place(self, rect: NativeRect) {
            self.with_connection(|conn, _| {
                let aux = ConfigureWindowAux::new()
                    .x(rect.x.round() as i32)
                    .y(rect.y.round() as i32)
                    .width(rect.width.round().max(1.0) as u32)
                    .height(rect.height.round().max(1.0) as u32);
                let _ = conn.configure_window(self.window, &aux);
            });
        }

        pub(crate) fn move_to(self, x: f64, y: f64) {
            self.with_connection(|conn, _| {
                let aux = ConfigureWindowAux::new()
                    .x(x.round() as i32)
                    .y(y.round() as i32);
                let _ = conn.configure_window(self.window, &aux);
            });
        }

        pub(crate) fn pointer_position(self) -> Option<(f64, f64)> {
            self.with_connection(|conn, root| {
                let reply = conn.query_pointer(root).ok()?.reply().ok()?;
                Some((reply.root_x as f64, reply.root_y as f64))
            })?
        }

        pub(crate) fn configure_overlay(self) {}

        pub(crate) fn cursor_position(self) -> Option<(f64, f64)> {
            self.with_connection(|conn, _| {
                let reply = conn.query_pointer(self.window).ok()?.reply().ok()?;
                Some((reply.win_x as f64, reply.win_y as f64))
            })?
        }

        pub(crate) fn focus_keyboard(self) {
            self.with_connection(|conn, _| {
                let _ = conn.set_input_focus(InputFocus::PARENT, self.window, x11rb::CURRENT_TIME);
            });
        }

        pub(crate) fn configure_toolbar(self, _overlay: NativeWindow) {
            self.with_connection(|conn, _| {
                let aux = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
                let _ = conn.configure_window(self.window, &aux);
            });
        }

        pub(crate) fn set_click_through(self, enabled: bool) {
            self.with_connection(|conn, _| {
                if enabled {
                    let _ = conn.shape_rectangles(
                        SO::SET,
                        SK::INPUT,
                        ClipOrdering::UNSORTED,
                        self.window,
                        0,
                        0,
                        &[],
                    );
                } else {
                    let _ = conn.shape_mask(SO::SET, SK::INPUT, self.window, 0, 0, x11rb::NONE);
                }
            });
        }

        pub(crate) fn set_visible(self, visible: bool) {
            self.with_connection(|conn, _| {
                if visible {
                    let _ = conn.map_window(self.window);
                } else {
                    let _ = conn.unmap_window(self.window);
                }
            });
        }
    }

    pub(crate) fn native_window(window: &Window) -> Option<NativeWindow> {
        let display = HasDisplayHandle::display_handle(window).ok()?;
        let RawDisplayHandle::Xcb(display) = display.as_raw() else {
            return None;
        };
        let connection = display.connection?;
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::Xcb(handle) = handle.as_raw() else {
            return None;
        };
        let mut native = NativeWindow {
            connection: connection.as_ptr() as usize,
            window: handle.window.get(),
            root: 0,
        };
        native.root = native.with_connection(|conn, _| {
            conn.setup()
                .roots
                .get(display.screen as usize)
                .map(|screen| screen.root)
        })??;
        Some(native)
    }
}
