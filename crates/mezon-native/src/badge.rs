//! Set the app badge count on the dock icon (macOS) or taskbar overlay (Windows).
//!
//! macOS  : `NSDockTile.setBadgeLabel` via the `objc` runtime.
//! Windows: `ITaskbarList3::SetOverlayIcon` — renders a small count bitmap
//!          as an overlay on the taskbar button.

pub fn set_badge_count(count: u32) {
    tracing::debug!("set_badge_count({})", count);

    #[cfg(target_os = "macos")]
    set_badge_macos(count);

    #[cfg(target_os = "windows")]
    set_badge_windows(count);
}

#[cfg(target_os = "windows")]
static MAIN_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(target_os = "windows")]
pub fn set_main_window_hwnd(hwnd: isize) {
    MAIN_HWND.store(hwnd, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(not(target_os = "windows"))]
pub fn set_main_window_hwnd(_hwnd: isize) {}

// ─── macOS ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn set_badge_macos(count: u32) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    // Safety: must be called from main thread (guaranteed — GPUI runs UI on main).
    unsafe {
        let cls = class!(NSApplication);
        let app: *mut Object = msg_send![cls, sharedApplication];
        let dock_tile: *mut Object = msg_send![app, dockTile];

        let label: *mut Object = if count == 0 {
            std::ptr::null_mut()
        } else {
            let label_str = count.to_string();
            let cls = class!(NSString);
            let s: *mut Object = msg_send![cls, alloc];
            let s: *mut Object = msg_send![s,
                initWithBytes: label_str.as_ptr()
                length: label_str.len()
                encoding: 4u64 // NSUTF8StringEncoding
            ];
            s
        };

        let _: () = msg_send![dock_tile, setBadgeLabel: label];
        if !label.is_null() {
            let _: () = msg_send![label, release];
        }
    }
}

// ─── Windows ──────────────────────────────────────────────────────────────────
//
// `ITaskbarList3::SetOverlayIcon` places a small icon in the bottom-right
// corner of the app's taskbar button.  We generate a 16×16 HICON on the fly
// that contains the count text, or pass NULL to clear it.

#[cfg(target_os = "windows")]
fn set_badge_windows(count: u32) {
    if let Err(e) = try_set_overlay_icon(count) {
        tracing::warn!("Windows badge count failed: {e}");
    }
}

#[cfg(target_os = "windows")]
fn try_set_overlay_icon(count: u32) -> windows::core::Result<()> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    use windows::Win32::Foundation::HWND;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let stored = MAIN_HWND.load(std::sync::atomic::Ordering::Relaxed);
    let hwnd = if stored != 0 {
        HWND(stored as *mut core::ffi::c_void)
    } else {
        unsafe { GetActiveWindow() }
    };

    let taskbar: ITaskbarList3 = unsafe {
        windows::Win32::System::Com::CoCreateInstance(
            &TaskbarList,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )?
    };
    unsafe { taskbar.HrInit()? };

    if count == 0 {
        // Pass null icon to clear the overlay.
        unsafe {
            taskbar.SetOverlayIcon(
                hwnd,
                HICON(std::ptr::null_mut()),
                &windows::core::HSTRING::new(),
            )?
        };
        return Ok(());
    }

    // Build a 16×16 badge bitmap with the count text.
    let hicon = build_count_icon(count)?;
    let description = windows::core::HSTRING::from(format!("{count} unread"));

    unsafe {
        taskbar.SetOverlayIcon(hwnd, hicon, &description)?;
        let _ = DestroyIcon(hicon);
    }
    Ok(())
}

/// Generate a 16×16 `HICON` containing the badge count text.
#[cfg(target_os = "windows")]
fn build_count_icon(
    count: u32,
) -> windows::core::Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
    use windows::Win32::Foundation::{COLORREF, RECT};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC, DeleteObject,
        FillRect, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

    const SIZE: i32 = 16;
    // #5865F2 brand blue, white text
    const BG_COLOR: u32 = 0x00F26558; // COLORREF is BGR
    const TEXT_COLOR: u32 = 0x00FFFFFF;

    let hdc_screen = unsafe { GetDC(None) };
    let hdc = unsafe { CreateCompatibleDC(Some(hdc_screen)) };
    let hbmp_color = unsafe { CreateCompatibleBitmap(hdc_screen, SIZE, SIZE) };
    let hbmp_mask = unsafe { CreateCompatibleBitmap(hdc_screen, SIZE, SIZE) };
    unsafe { ReleaseDC(None, hdc_screen) };

    unsafe {
        SelectObject(hdc, hbmp_color.into());

        // Fill background circle.
        let brush = CreateSolidBrush(COLORREF(BG_COLOR));
        let rc = RECT {
            left: 0,
            top: 0,
            right: SIZE,
            bottom: SIZE,
        };
        FillRect(hdc, &rc, brush);
        let _ = DeleteObject(brush.into());

        // Draw count text.
        let label = if count > 99 {
            "99+".to_owned()
        } else {
            count.to_string()
        };
        let text: Vec<u16> = label.encode_utf16().collect();
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(TEXT_COLOR));
        TextOutW(hdc, 1, 2, &text);

        DeleteDC(hdc);
    }

    let icon_info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbmp_mask.into(),
        hbmColor: hbmp_color.into(),
    };

    let hicon = unsafe { CreateIconIndirect(&icon_info)? };

    unsafe {
        DeleteObject(hbmp_color.into());
        DeleteObject(hbmp_mask.into());
    }

    Ok(hicon)
}
