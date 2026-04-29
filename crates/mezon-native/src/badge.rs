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
    }
}

// ─── Windows ──────────────────────────────────────────────────────────────────
//
// `ITaskbarList3::SetOverlayIcon` places a small icon in the bottom-right
// corner of the app's taskbar button.  We generate a 16×16 HICON on the fly
// that contains the count text, or pass NULL to clear it.

#[cfg(target_os = "windows")]
fn set_badge_windows(count: u32) {
    // SetOverlayIcon must be called on the thread that owns the HWND (the UI
    // thread).  We spawn a thread here but the real call needs to happen on the
    // message thread.  For Stage 0 we do a best-effort call from a dedicated
    // thread — GPUI will move this to the main window thread in Stage 2 when
    // we have proper WindowHandle access.
    std::thread::spawn(move || {
        if let Err(e) = try_set_overlay_icon(count) {
            tracing::warn!("Windows badge count failed: {e}");
        }
    });
}

#[cfg(target_os = "windows")]
fn try_set_overlay_icon(count: u32) -> windows::core::Result<()> {
    use windows::core::Interface as _;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, SetBkColor, SetTextColor, TextOutW, HBITMAP,
    };
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, ICONINFO};

    // Instantiate ITaskbarList3 via CoCreate.
    let taskbar: ITaskbarList3 = unsafe {
        windows::Win32::System::Com::CoCreateInstance(
            &TaskbarList,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )?
    };
    unsafe { taskbar.HrInit()? };

    if count == 0 {
        // Pass NULL icon to clear the overlay.
        unsafe { taskbar.SetOverlayIcon(HWND(0), None, &windows::core::HSTRING::new())? };
        return Ok(());
    }

    // Build a 16×16 badge bitmap with the count text.
    let hicon = build_count_icon(count)?;
    let description = windows::core::HSTRING::from(format!("{count} unread"));

    // HWND(0) — the OS uses the foreground window of the calling process.
    // Stage 2 will pass the real HWND from the stored WindowHandle.
    unsafe {
        taskbar.SetOverlayIcon(HWND(0), hicon, &description)?;
        DestroyIcon(hicon);
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
        FillRect, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TextOutW, TRANSPARENT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

    const SIZE: i32 = 16;
    // #5865F2 brand blue, white text
    const BG_COLOR: u32 = 0x00F26558; // COLORREF is BGR
    const TEXT_COLOR: u32 = 0x00FFFFFF;

    let hdc_screen = unsafe { GetDC(None) };
    let hdc = unsafe { CreateCompatibleDC(hdc_screen) };
    let hbmp_color = unsafe { CreateCompatibleBitmap(hdc_screen, SIZE, SIZE) };
    let hbmp_mask = unsafe { CreateCompatibleBitmap(hdc_screen, SIZE, SIZE) };
    unsafe { ReleaseDC(None, hdc_screen) };

    unsafe {
        SelectObject(hdc, hbmp_color);

        // Fill background circle.
        let brush = CreateSolidBrush(COLORREF(BG_COLOR));
        let rc = RECT {
            left: 0,
            top: 0,
            right: SIZE,
            bottom: SIZE,
        };
        FillRect(hdc, &rc, brush);
        DeleteObject(brush);

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
        hbmMask: hbmp_mask,
        hbmColor: hbmp_color,
    };

    let hicon = unsafe { CreateIconIndirect(&icon_info)? };

    unsafe {
        DeleteObject(hbmp_color);
        DeleteObject(hbmp_mask);
    }

    Ok(hicon)
}
