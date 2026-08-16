use crate::screen_picker::PickedScreen;

#[derive(Clone, Debug)]
pub struct ScreenSharePreview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
const PREVIEW_MAX_WIDTH: u32 = 420;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
const PREVIEW_MAX_HEIGHT: u32 = 236;

pub fn capture_screen_share_preview(pick: &PickedScreen) -> Option<ScreenSharePreview> {
    match pick {
        PickedScreen::Target(target) => {
            #[cfg(target_os = "macos")]
            {
                capture_macos_preview(target)
            }

            #[cfg(target_os = "linux")]
            {
                capture_x11_preview(target)
            }

            #[cfg(target_os = "windows")]
            {
                capture_windows_preview(target)
            }

            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            {
                let _ = target;
                None
            }
        }
        #[cfg(target_os = "linux")]
        PickedScreen::LinuxPortal => None,
    }
}

#[cfg(target_os = "macos")]
fn capture_macos_preview(target: &scap::Target) -> Option<ScreenSharePreview> {
    use core_graphics_helmer_fork::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics_helmer_fork::window::{
        self, kCGWindowImageBoundsIgnoreFraming, kCGWindowImageNominalResolution,
        kCGWindowListOptionIncludingWindow,
    };

    let image = match target {
        scap::Target::Window(win) => {
            let bounds = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
            window::create_image(
                bounds,
                kCGWindowListOptionIncludingWindow,
                win.raw_handle,
                kCGWindowImageBoundsIgnoreFraming | kCGWindowImageNominalResolution,
            )?
        }
        scap::Target::Display(display) => display.raw_handle.image()?,
    };

    cg_image_to_preview(&image)
}

#[cfg(target_os = "macos")]
fn cg_image_to_preview(
    image: &core_graphics_helmer_fork::image::CGImage,
) -> Option<ScreenSharePreview> {
    use core_graphics_helmer_fork::base::{kCGBitmapByteOrder32Little, kCGImageAlphaNoneSkipFirst};
    use core_graphics_helmer_fork::color_space::{CGColorSpace, kCGColorSpaceSRGB};
    use core_graphics_helmer_fork::context::CGContext;
    use core_graphics_helmer_fork::geometry::{CGPoint, CGRect, CGSize};

    let width = image.width() as u32;
    let height = image.height() as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let (thumb_w, thumb_h) = preview_dimensions(width, height);

    let color_space = CGColorSpace::create_with_name(unsafe { kCGColorSpaceSRGB })
        .unwrap_or_else(CGColorSpace::create_device_rgb);
    let mut context = CGContext::create_bitmap_context(
        None,
        thumb_w as usize,
        thumb_h as usize,
        8,
        0,
        &color_space,
        kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little,
    );
    context.draw_image(
        CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(thumb_w as f64, thumb_h as f64),
        ),
        image,
    );

    let src_stride = context.bytes_per_row();
    let dst_stride = thumb_w as usize * 4;
    if src_stride < dst_stride {
        return None;
    }
    let src = context.data();
    let mut rgba = vec![0u8; dst_stride * thumb_h as usize];
    for y in 0..thumb_h as usize {
        let src_row = &src[y * src_stride..y * src_stride + dst_stride];
        let dst_row = &mut rgba[y * dst_stride..(y + 1) * dst_stride];
        dst_row.copy_from_slice(src_row);
        for pixel in dst_row.chunks_exact_mut(4) {
            pixel[3] = 0xff;
        }
    }

    Some(ScreenSharePreview {
        width: thumb_w,
        height: thumb_h,
        rgba,
    })
}

#[cfg(target_os = "linux")]
fn capture_x11_preview(target: &scap::Target) -> Option<ScreenSharePreview> {
    use xcb::x;

    let (conn, _) = xcb::Connection::connect(None).ok()?;

    let (drawable, src_x, src_y, width, height) = match target {
        scap::Target::Window(win) => {
            let drawable = x::Drawable::Window(win.raw_handle);
            let cookie = conn.send_request(&x::GetGeometry { drawable });
            let geometry = conn.wait_for_reply(cookie).ok()?;
            (drawable, 0, 0, geometry.width(), geometry.height())
        }
        scap::Target::Display(display) => (
            x::Drawable::Window(display.raw_handle),
            display.x_offset,
            display.y_offset,
            display.width,
            display.height,
        ),
    };
    if width == 0 || height == 0 {
        return None;
    }

    let cookie = conn.send_request(&x::GetImage {
        format: x::ImageFormat::ZPixmap,
        drawable,
        x: src_x,
        y: src_y,
        width,
        height,
        plane_mask: u32::MAX,
    });
    let image = conn.wait_for_reply(cookie).ok()?;
    if image.depth() != 24 && image.depth() != 32 {
        return None;
    }

    let width = u32::from(width);
    let height = u32::from(height);
    let data = image.data();
    let stride = data.len() / height as usize;
    if stride < width as usize * 4 {
        return None;
    }

    let lsb_first = matches!(conn.get_setup().image_byte_order(), x::ImageOrder::LsbFirst);
    let (thumb_w, thumb_h) = preview_dimensions(width, height);
    let mut rgba = vec![0u8; (thumb_w * thumb_h * 4) as usize];

    for y in 0..thumb_h as usize {
        let src_y = y * height as usize / thumb_h as usize;
        for x in 0..thumb_w as usize {
            let src_x = x * width as usize / thumb_w as usize;
            let src_offset = src_y * stride + src_x * 4;
            let dst_offset = (y * thumb_w as usize + x) * 4;
            if src_offset + 3 >= data.len() {
                continue;
            }
            let (b, g, r) = if lsb_first {
                (data[src_offset], data[src_offset + 1], data[src_offset + 2])
            } else {
                (
                    data[src_offset + 3],
                    data[src_offset + 2],
                    data[src_offset + 1],
                )
            };
            rgba[dst_offset] = b;
            rgba[dst_offset + 1] = g;
            rgba[dst_offset + 2] = r;
            rgba[dst_offset + 3] = 0xff;
        }
    }

    Some(ScreenSharePreview {
        width: thumb_w,
        height: thumb_h,
        rgba,
    })
}

#[cfg(target_os = "windows")]
fn capture_windows_preview(target: &scap::Target) -> Option<ScreenSharePreview> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CAPTUREBLT, GetDC, GetMonitorInfoW, MONITORINFO, ROP_CODE, ReleaseDC, SRCCOPY,
    };
    use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, PW_RENDERFULLCONTENT};

    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return None;
        }

        let captured = match target {
            scap::Target::Window(win) => {
                let hwnd = win.raw_handle;
                if IsIconic(hwnd).as_bool() {
                    None
                } else {
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).is_ok() {
                        let width = rect.right - rect.left;
                        let height = rect.bottom - rect.top;
                        capture_windows_dib(screen_dc, width, height, |dc| {
                            PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT))
                                .as_bool()
                        })
                        .map(|bgra| (width as u32, height as u32, bgra))
                    } else {
                        None
                    }
                }
            }
            scap::Target::Display(display) => {
                let mut info = MONITORINFO {
                    cbSize: core::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(display.raw_handle, &mut info).as_bool() {
                    let rect = info.rcMonitor;
                    let width = rect.right - rect.left;
                    let height = rect.bottom - rect.top;
                    capture_windows_dib(screen_dc, width, height, |dc| {
                        BitBlt(
                            dc,
                            0,
                            0,
                            width,
                            height,
                            Some(screen_dc),
                            rect.left,
                            rect.top,
                            ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
                        )
                        .is_ok()
                    })
                    .map(|bgra| (width as u32, height as u32, bgra))
                } else {
                    None
                }
            }
        };

        ReleaseDC(None, screen_dc);

        let (width, height, bgra) = captured?;
        windows_bgra_to_preview(width, height, &bgra)
    }
}

#[cfg(target_os = "windows")]
fn capture_windows_dib(
    screen_dc: windows::Win32::Graphics::Gdi::HDC,
    width: i32,
    height: i32,
    fill: impl FnOnce(windows::Win32::Graphics::Gdi::HDC) -> bool,
) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
    };

    if width <= 0 || height <= 0 {
        return None;
    }

    unsafe {
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        if mem_dc.is_invalid() {
            return None;
        }

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let dib = match CreateDIBSection(Some(mem_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) if !bits.is_null() => dib,
            _ => {
                let _ = DeleteDC(mem_dc);
                return None;
            }
        };

        let previous = SelectObject(mem_dc, HGDIOBJ(dib.0));
        let ok = fill(mem_dc);

        let pixels = if ok {
            let len = width as usize * height as usize * 4;
            Some(std::slice::from_raw_parts(bits as *const u8, len).to_vec())
        } else {
            None
        };

        SelectObject(mem_dc, previous);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem_dc);
        pixels
    }
}

#[cfg(target_os = "windows")]
fn windows_bgra_to_preview(width: u32, height: u32, bgra: &[u8]) -> Option<ScreenSharePreview> {
    if width == 0 || height == 0 {
        return None;
    }
    let stride = width as usize * 4;
    if bgra.len() < stride * height as usize {
        return None;
    }

    let (thumb_w, thumb_h) = preview_dimensions(width, height);
    let mut rgba = vec![0u8; (thumb_w * thumb_h * 4) as usize];

    for y in 0..thumb_h as usize {
        let src_y = y * height as usize / thumb_h as usize;
        for x in 0..thumb_w as usize {
            let src_x = x * width as usize / thumb_w as usize;
            let src_offset = src_y * stride + src_x * 4;
            let dst_offset = (y * thumb_w as usize + x) * 4;
            if src_offset + 3 >= bgra.len() {
                continue;
            }
            rgba[dst_offset] = bgra[src_offset];
            rgba[dst_offset + 1] = bgra[src_offset + 1];
            rgba[dst_offset + 2] = bgra[src_offset + 2];
            rgba[dst_offset + 3] = 0xff;
        }
    }

    Some(ScreenSharePreview {
        width: thumb_w,
        height: thumb_h,
        rgba,
    })
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let scale = (PREVIEW_MAX_WIDTH as f32 / width as f32)
        .min(PREVIEW_MAX_HEIGHT as f32 / height as f32)
        .min(1.0);
    let thumb_w = ((width as f32 * scale).round() as u32).max(1);
    let thumb_h = ((height as f32 * scale).round() as u32).max(1);
    (thumb_w, thumb_h)
}
