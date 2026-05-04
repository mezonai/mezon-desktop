//! Screen lock/unlock event detection.
//!
//! macOS  : subscribes to `com.apple.screenIsLocked` / `com.apple.screenIsUnlocked`
//!          via `CFNotificationCenter` (the distributed notification centre).
//! Windows: registers a hidden message-only `HWND` and listens for
//!          `WM_WTSSESSION_CHANGE` via `WTSRegisterSessionNotification`.
//! Linux  : stub (D-Bus `org.freedesktop.login1` integration deferred).

pub enum PowerEvent {
    ScreenLocked,
    ScreenUnlocked,
}

pub type PowerEventCallback = Box<dyn Fn(PowerEvent) + Send + 'static>;

/// Subscribe to screen lock/unlock events.
/// The callback is invoked from a background thread.
pub fn subscribe(callback: PowerEventCallback) {
    #[cfg(target_os = "macos")]
    subscribe_macos(callback);

    #[cfg(target_os = "windows")]
    subscribe_windows(callback);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = callback;
        tracing::debug!("Power event subscription not yet implemented on Linux");
    }
}

// ─── macOS ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn subscribe_macos(callback: PowerEventCallback) {
    use std::os::raw::c_void;

    // The C trampoline is defined as a standalone `extern "C"` fn so that
    // its address can be passed to CFNotificationCenterAddObserver.
    extern "C" fn trampoline(
        _center: *mut c_void,
        observer: *mut c_void,
        name: *const c_void,
        _object: *const c_void,
        _user_info: *const c_void,
    ) {
        let cb = unsafe { &*(observer as *const PowerEventCallback) };
        let name_str = unsafe { cf_string_to_str(name) };
        let event = match name_str.as_deref() {
            Some("com.apple.screenIsLocked") => Some(PowerEvent::ScreenLocked),
            Some("com.apple.screenIsUnlocked") => Some(PowerEvent::ScreenUnlocked),
            _ => None,
        };
        if let Some(ev) = event {
            cb(ev);
        }
    }

    // Leak the callback — it lives for the process lifetime.
    // Transmit the raw address as a plain `usize` so it crosses the `Send` boundary.
    let raw: usize = Box::into_raw(Box::new(callback)) as usize;

    std::thread::Builder::new()
        .name("mezon-power-events".into())
        .spawn(move || unsafe {
            cf_register(trampoline, raw as *mut c_void);
            cf_run_loop_run();
        })
        .expect("Failed to spawn power-events thread");

    tracing::debug!("macOS screen lock/unlock notifications registered");
}

/// Register the trampoline with the distributed CFNotificationCenter for both
/// `com.apple.screenIsLocked` and `com.apple.screenIsUnlocked`.
#[cfg(target_os = "macos")]
unsafe fn cf_register(
    callback: extern "C" fn(
        *mut std::os::raw::c_void,
        *mut std::os::raw::c_void,
        *const std::os::raw::c_void,
        *const std::os::raw::c_void,
        *const std::os::raw::c_void,
    ),
    observer: *mut std::os::raw::c_void,
) {
    use std::os::raw::{c_char, c_void};

    unsafe extern "C" {
        fn CFNotificationCenterGetDistributedCenter() -> *mut c_void;
        fn CFNotificationCenterAddObserver(
            center: *mut c_void,
            observer: *const c_void,
            callback: extern "C" fn(
                *mut c_void,
                *mut c_void,
                *const c_void,
                *const c_void,
                *const c_void,
            ),
            name: *const c_void,
            object: *const c_void,
            suspension_behavior: i64,
        );
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> *const c_void;
    }

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;
    // CFNotificationSuspensionBehaviorDeliverImmediately = 4
    const DELIVER_IMMEDIATELY: i64 = 4;

    unsafe {
        let center = CFNotificationCenterGetDistributedCenter();
        for name_bytes in [
            b"com.apple.screenIsLocked\0" as &[u8],
            b"com.apple.screenIsUnlocked\0" as &[u8],
        ] {
            let cf_name = CFStringCreateWithCString(
                std::ptr::null(),
                name_bytes.as_ptr() as *const c_char,
                K_CF_STRING_ENCODING_UTF8,
            );
            CFNotificationCenterAddObserver(
                center,
                observer as *const c_void,
                callback,
                cf_name,
                std::ptr::null(),
                DELIVER_IMMEDIATELY,
            );
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn cf_run_loop_run() {
    unsafe extern "C" {
        fn CFRunLoopRun();
    }
    unsafe { CFRunLoopRun() };
}

/// Convert a CFStringRef to a Rust `String`.
/// Returns `None` if the pointer is null or the string uses a non-ASCII path.
#[cfg(target_os = "macos")]
unsafe fn cf_string_to_str(cf_str: *const std::os::raw::c_void) -> Option<String> {
    use std::os::raw::{c_char, c_void};

    unsafe extern "C" {
        fn CFStringGetCStringPtr(the_string: *const c_void, encoding: u32) -> *const c_char;
    }

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;
    if cf_str.is_null() {
        return None;
    }
    let ptr = unsafe { CFStringGetCStringPtr(cf_str, K_CF_STRING_ENCODING_UTF8) };
    if ptr.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

// ─── Windows ──────────────────────────────────────────────────────────────────

/// Module-level storage for the Windows power-event callback.
/// Accessible from `wnd_proc` which must be an `extern "system"` fn.
#[cfg(target_os = "windows")]
static WIN_POWER_CB: std::sync::OnceLock<std::sync::Mutex<Option<PowerEventCallback>>> =
    std::sync::OnceLock::new();

#[cfg(target_os = "windows")]
fn subscribe_windows(callback: PowerEventCallback) {
    WIN_POWER_CB
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap()
        .replace(callback);

    std::thread::Builder::new()
        .name("mezon-power-events".into())
        .spawn(|| unsafe { windows_message_loop() })
        .expect("Failed to spawn power-events thread");

    tracing::debug!("Windows screen lock/unlock notifications registered");
}

#[cfg(target_os = "windows")]
unsafe fn windows_message_loop() {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::RemoteDesktop::{
        NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
        HWND_MESSAGE, MSG, PostQuitMessage, RegisterClassExW, TranslateMessage, WINDOW_EX_STYLE,
        WM_DESTROY, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::PCWSTR;

    const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
    const WTS_SESSION_LOCK: usize = 7;
    const WTS_SESSION_UNLOCK: usize = 8;

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_WTSSESSION_CHANGE {
            if let Some(mtx) = WIN_POWER_CB.get() {
                if let Ok(guard) = mtx.lock() {
                    if let Some(cb) = guard.as_ref() {
                        match wparam.0 {
                            WTS_SESSION_LOCK => cb(PowerEvent::ScreenLocked),
                            WTS_SESSION_UNLOCK => cb(PowerEvent::ScreenUnlocked),
                            _ => {}
                        }
                    }
                }
            }
        } else if msg == WM_DESTROY {
            unsafe { PostQuitMessage(0) };
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    let class_name: Vec<u16> = "MezonPowerWnd\0".encode_utf16().collect();
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc) };

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
        .expect("CreateWindowExW failed for power-events HWND")
    };

    unsafe {
        WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION)
            .expect("WTSRegisterSessionNotification failed");
    }

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
