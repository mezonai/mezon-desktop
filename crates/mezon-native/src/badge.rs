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

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    set_badge_linux(count);
}

// ─── Linux ──────────────────────────────────────────────────────────────────────
//
// The Unity LauncherEntry D-Bus API (`com.canonical.Unity.LauncherEntry.Update`)
// is honoured by GNOME (Dash-to-Dock), KDE Plasma, Unity and others.

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn set_badge_linux(count: u32) {
    std::thread::spawn(move || {
        if let Err(e) = try_set_badge_linux(count) {
            tracing::warn!("Linux badge update failed: {e}");
        }
    });
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn try_set_badge_linux(count: u32) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashMap;
    use zbus::zvariant::Value;

    let connection = zbus::blocking::Connection::session()?;
    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("count", Value::I64(i64::from(count)));
    props.insert("count-visible", Value::Bool(count > 0));

    connection.emit_signal(
        None::<&str>,
        "/com/canonical/unity/launcherentry/mezon",
        "com.canonical.Unity.LauncherEntry",
        "Update",
        &("application://mezon.desktop", props),
    )?;
    Ok(())
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
    use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;

    let stored = MAIN_HWND.load(std::sync::atomic::Ordering::Relaxed);
    let hwnd = if stored != 0 {
        stored
    } else {
        unsafe { GetActiveWindow() }.0 as isize
    };
    if hwnd == 0 {
        return;
    }
    let _ = badge_worker().try_send((hwnd, count));
}

#[cfg(target_os = "windows")]
fn badge_worker() -> &'static crossbeam_channel::Sender<(isize, u32)> {
    static WORKER: std::sync::OnceLock<crossbeam_channel::Sender<(isize, u32)>> =
        std::sync::OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::bounded::<(isize, u32)>(4);
        if let Err(e) = std::thread::Builder::new()
            .name("mezon-taskbar-badge".into())
            .spawn(move || badge_worker_loop(&rx))
        {
            tracing::warn!("Failed to spawn taskbar badge thread: {e}");
        }
        tx
    })
}

#[cfg(target_os = "windows")]
fn badge_worker_loop(rx: &crossbeam_channel::Receiver<(isize, u32)>) {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::ITaskbarList3;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let mut taskbar: Option<ITaskbarList3> = None;
    while let Ok(mut job) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            job = newer;
        }

        if taskbar.is_none() {
            match create_taskbar_list() {
                Ok(list) => taskbar = Some(list),
                Err(e) => {
                    tracing::warn!("Windows taskbar interface unavailable: {e}");
                    continue;
                }
            }
        }

        let Some(list) = taskbar.as_ref() else {
            continue;
        };
        if let Err(e) = apply_overlay_icon(list, job.0, job.1) {
            tracing::warn!("Windows badge count failed: {e}");
            taskbar = None;
        }
    }
}

#[cfg(target_os = "windows")]
fn create_taskbar_list() -> windows::core::Result<windows::Win32::UI::Shell::ITaskbarList3> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};

    let taskbar: ITaskbarList3 =
        unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)? };
    unsafe { taskbar.HrInit()? };
    Ok(taskbar)
}

#[cfg(target_os = "windows")]
fn apply_overlay_icon(
    taskbar: &windows::Win32::UI::Shell::ITaskbarList3,
    hwnd: isize,
    count: u32,
) -> windows::core::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);

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

    let hicon = build_count_icon(count)?;
    let description = windows::core::HSTRING::from(format!("{count} unread"));
    let result = unsafe { taskbar.SetOverlayIcon(hwnd, hicon, &description) };
    unsafe {
        let _ = DestroyIcon(hicon);
    }
    result
}

/// Generate a 16×16 `HICON` with the badge count centred on a red circle
/// (matching the in-app `#DA373C` unread badges).
#[cfg(target_os = "windows")]
fn build_count_icon(
    count: u32,
) -> windows::core::Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
    use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
    use windows::Win32::Graphics::Gdi::{
        ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPV5HEADER, CLIP_DEFAULT_PRECIS, CreateBitmap,
        CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
        DEFAULT_PITCH, DIB_RGB_COLORS, DeleteDC, DeleteObject, Ellipse, FF_DONTCARE, FW_BOLD,
        FillRect, GdiFlush, GetDC, GetStockObject, GetTextExtentPoint32W, NULL_PEN,
        OUT_DEFAULT_PRECIS, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
        TextOutW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};
    use windows::core::PCWSTR;

    const ICON_SIZE: i32 = 16;
    const SS: i32 = 4;
    const HI: i32 = ICON_SIZE * SS;
    const BG_COLOR: u32 = 0x003C_37DA;
    const TEXT_COLOR: u32 = 0x00FF_FFFF;
    const BLACK: u32 = 0x0000_0000;

    fn argb_header(width: i32, height: i32) -> BITMAPV5HEADER {
        BITMAPV5HEADER {
            bV5Size: size_of::<BITMAPV5HEADER>() as u32,
            bV5Width: width,
            bV5Height: -height,
            bV5Planes: 1,
            bV5BitCount: 32,
            bV5Compression: BI_RGB,
            bV5RedMask: 0x00FF_0000,
            bV5GreenMask: 0x0000_FF00,
            bV5BlueMask: 0x0000_00FF,
            bV5AlphaMask: 0xFF00_0000,
            ..Default::default()
        }
    }

    let label = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    let text: Vec<u16> = label.encode_utf16().collect();
    let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();

    let hdc_screen = unsafe { GetDC(None) };
    let hdc = unsafe { CreateCompatibleDC(Some(hdc_screen)) };

    let hi_header = argb_header(HI, HI);
    let mut hi_bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let hi_dib = unsafe {
        CreateDIBSection(
            Some(hdc_screen),
            &hi_header as *const BITMAPV5HEADER as *const BITMAPINFO,
            DIB_RGB_COLORS,
            &mut hi_bits,
            None,
            0,
        )
    };
    let lo_header = argb_header(ICON_SIZE, ICON_SIZE);
    let mut lo_bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let lo_dib = unsafe {
        CreateDIBSection(
            Some(hdc_screen),
            &lo_header as *const BITMAPV5HEADER as *const BITMAPINFO,
            DIB_RGB_COLORS,
            &mut lo_bits,
            None,
            0,
        )
    };
    unsafe { ReleaseDC(None, hdc_screen) };

    let (hi_dib, lo_dib) = match (hi_dib, lo_dib) {
        (Ok(hi), Ok(lo)) => (hi, lo),
        (hi, lo) => {
            unsafe {
                if let Ok(hi) = hi {
                    let _ = DeleteObject(hi.into());
                }
                if let Ok(lo) = lo {
                    let _ = DeleteObject(lo.into());
                }
                let _ = DeleteDC(hdc);
            }
            return Err(windows::core::Error::from_win32());
        }
    };

    unsafe {
        let old_bmp = SelectObject(hdc, hi_dib.into());
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));

        let full = RECT {
            left: 0,
            top: 0,
            right: HI,
            bottom: HI,
        };
        let black_brush = CreateSolidBrush(COLORREF(BLACK));
        FillRect(hdc, &full, black_brush);
        let _ = DeleteObject(black_brush.into());

        let red_brush = CreateSolidBrush(COLORREF(BG_COLOR));
        let old_brush = SelectObject(hdc, red_brush.into());
        let _ = Ellipse(hdc, 0, 0, HI, HI);
        SelectObject(hdc, old_brush);
        let _ = DeleteObject(red_brush.into());

        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(TEXT_COLOR));

        let mut extent = SIZE::default();
        let mut old_font = None;
        for height in [HI * 5 / 8, HI / 2, HI * 7 / 16, HI * 3 / 8, HI / 4] {
            let font = CreateFontW(
                -height,
                0,
                0,
                0,
                FW_BOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                PCWSTR(face.as_ptr()),
            );
            let previous = SelectObject(hdc, font.into());
            if old_font.is_none() {
                old_font = Some(previous);
            } else {
                let _ = DeleteObject(previous);
            }
            let _ = GetTextExtentPoint32W(hdc, &text, &mut extent);
            if extent.cx <= HI * 3 / 4 {
                break;
            }
        }

        let _ = TextOutW(hdc, (HI - extent.cx) / 2, (HI - extent.cy) / 2, &text);

        if let Some(old_font) = old_font {
            let current = SelectObject(hdc, old_font);
            let _ = DeleteObject(current);
        }
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_bmp);
        let _ = GdiFlush();

        let src = std::slice::from_raw_parts(hi_bits as *const u32, (HI * HI) as usize);
        let dst =
            std::slice::from_raw_parts_mut(lo_bits as *mut u32, (ICON_SIZE * ICON_SIZE) as usize);
        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let (mut covered, mut r, mut g, mut b) = (0u32, 0u32, 0u32, 0u32);
                for sy in 0..SS {
                    for sx in 0..SS {
                        let px = src[((y * SS + sy) * HI + x * SS + sx) as usize];
                        if px & 0x00FF_FFFF == 0 {
                            continue;
                        }
                        covered += 1;
                        r += (px >> 16) & 0xFF;
                        g += (px >> 8) & 0xFF;
                        b += px & 0xFF;
                    }
                }
                let samples = (SS * SS) as u32;
                let alpha = covered * 255 / samples;
                let out = if covered == 0 {
                    0
                } else {
                    let scale = |c: u32| (c / covered) * alpha / 255;
                    (alpha << 24) | (scale(r) << 16) | (scale(g) << 8) | scale(b)
                };
                dst[(y * ICON_SIZE + x) as usize] = out;
            }
        }

        let _ = DeleteObject(hi_dib.into());
        let _ = DeleteDC(hdc);
    }

    let mask_bits = vec![0xFFu8; (ICON_SIZE * ICON_SIZE / 8) as usize];
    let hbmp_mask = unsafe {
        CreateBitmap(
            ICON_SIZE,
            ICON_SIZE,
            1,
            1,
            Some(mask_bits.as_ptr() as *const core::ffi::c_void),
        )
    };

    let icon_info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbmp_mask,
        hbmColor: lo_dib,
    };

    let created = unsafe { CreateIconIndirect(&icon_info) };

    unsafe {
        let _ = DeleteObject(lo_dib.into());
        let _ = DeleteObject(hbmp_mask.into());
    }

    Ok(created?)
}
