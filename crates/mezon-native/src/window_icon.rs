#[cfg(target_os = "windows")]
pub fn apply_dpi_aware_icons(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    if let Err(e) = try_apply_dpi_aware_icons(hwnd) {
        tracing::warn!("Failed to apply DPI-aware window icons: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn apply_dpi_aware_icons(_hwnd: isize) {}

#[cfg(target_os = "windows")]
fn try_apply_dpi_aware_icons(hwnd: isize) -> windows::core::Result<()> {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows::Win32::UI::WindowsAndMessaging::{
        ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW, SM_CXICON,
        SM_CXSMICON, SendMessageW, WM_SETICON,
    };
    use windows::core::PCWSTR;

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        return Err(windows::core::Error::from_win32());
    }

    let big = unsafe { GetSystemMetricsForDpi(SM_CXICON, dpi) };
    let small = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    if big <= 0 || small <= 0 {
        return Err(windows::core::Error::from_win32());
    }

    let module = unsafe { GetModuleHandleW(None)? };
    let load = |cx: i32, cy: i32| -> windows::core::Result<_> {
        unsafe {
            LoadImageW(
                Some(module.into()),
                PCWSTR(1 as _),
                IMAGE_ICON,
                cx,
                cy,
                LR_DEFAULTCOLOR | LR_SHARED,
            )
        }
    };

    let big_icon = load(big, big)?;
    let small_icon = load(small, small)?;

    unsafe {
        SendMessageW(
            hwnd,
            WM_SETICON,
            WPARAM(ICON_BIG as usize),
            LPARAM(big_icon.0 as isize),
        );
        SendMessageW(
            hwnd,
            WM_SETICON,
            WPARAM(ICON_SMALL as usize),
            LPARAM(small_icon.0 as isize),
        );
    }

    Ok(())
}
