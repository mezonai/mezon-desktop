use anyhow::Context as _;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gpui::{App, AppContext, Window};
use image::{ImageFormat, RgbaImage};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use scap::Target;
use scap::capturer::{Capturer, Options, Resolution};
use scap::frame::{BGRAFrame, BGRFrame, Frame, FrameType, RGBFrame};
use scap::{get_all_targets, has_permission, is_supported, request_permission};
use serde_json::{Value, json};
use std::io::Cursor;

use crate::app::main_window::handle;

const CHAT_SIDEBAR_WIDTH: f32 = 344.0;

pub fn capture_png(cx: &mut App, chat_only: bool) -> anyhow::Result<Value> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;

    let (window_id, scale_factor) = cx
        .update_window(main_handle, |_, window, _| {
            native_window_id(window).map(|window_id| (window_id, window.scale_factor()))
        })
        .context("Reading main window handle")?
        .context("Resolving native window id")?;

    capture_with_scap(window_id, scale_factor, chat_only)
}

fn capture_with_scap(window_id: u32, scale_factor: f32, chat_only: bool) -> anyhow::Result<Value> {
    if !is_supported() {
        anyhow::bail!("Screen capture is not supported on this platform");
    }
    if !has_permission() && !request_permission() {
        anyhow::bail!("Screen recording permission is required to capture the window");
    }

    let target = find_scap_target(window_id)?;
    let bgra = capture_frame(&target).context("Capturing window frame")?;
    let mut rgba = bgra_to_rgba(&bgra).context("Converting capture to RGBA")?;

    if chat_only {
        rgba = crop_chat_region(rgba, scale_factor)?;
    }

    let png = encode_png(&rgba)?;
    let encoded = BASE64.encode(&png);
    Ok(json!({
        "format": "png",
        "width": rgba.width(),
        "height": rgba.height(),
        "region": if chat_only { "chat" } else { "window" },
        "source": "scap",
        "data_base64": encoded,
    }))
}

fn find_scap_target(window_id: u32) -> anyhow::Result<Target> {
    get_all_targets()
        .context("Listing screen capture targets")?
        .into_iter()
        .find(|target| match target {
            Target::Window(window) => window.id == window_id,
            Target::Display(_) => false,
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Main window (id={window_id}) was not found among screen capture targets; ensure the window is visible"
            )
        })
}

fn capture_frame(target: &Target) -> anyhow::Result<BGRAFrame> {
    let options = Options {
        fps: 1,
        show_cursor: false,
        show_highlight: false,
        target: Some(target.clone()),
        crop_area: None,
        output_type: FrameType::BGRAFrame,
        output_resolution: Resolution::Captured,
        excluded_targets: None,
        portal_source_types: None,
        use_portal: false,
    };

    let mut capturer = Capturer::build(options).context("Building screen capturer")?;
    capturer.start_capture();
    let frame = capturer
        .get_next_frame()
        .context("Receiving captured frame");
    capturer.stop_capture();
    let frame = frame?;
    frame_to_bgra(frame).ok_or_else(|| anyhow::anyhow!("Unsupported capture frame format"))
}

fn frame_to_bgra(frame: Frame) -> Option<BGRAFrame> {
    match frame {
        Frame::BGRA(frame) => Some(frame),
        Frame::BGRx(mut frame) => {
            for px in frame.data.chunks_exact_mut(4) {
                px[3] = 255;
            }
            Some(BGRAFrame {
                display_time: frame.display_time,
                width: frame.width,
                height: frame.height,
                data: frame.data,
            })
        }
        Frame::RGBx(mut frame) => {
            for px in frame.data.chunks_exact_mut(4) {
                px.swap(0, 2);
                px[3] = 255;
            }
            Some(BGRAFrame {
                display_time: frame.display_time,
                width: frame.width,
                height: frame.height,
                data: frame.data,
            })
        }
        Frame::XBGR(mut frame) => {
            for px in frame.data.chunks_exact_mut(4) {
                let (b, g, r) = (px[1], px[2], px[3]);
                px[0] = b;
                px[1] = g;
                px[2] = r;
                px[3] = 255;
            }
            Some(BGRAFrame {
                display_time: frame.display_time,
                width: frame.width,
                height: frame.height,
                data: frame.data,
            })
        }
        Frame::RGB(RGBFrame {
            display_time,
            width,
            height,
            data,
        }) => {
            let mut bgra_data = Vec::with_capacity(data.len() / 3 * 4);
            for px in data.chunks_exact(3) {
                bgra_data.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            Some(BGRAFrame {
                display_time,
                width,
                height,
                data: bgra_data,
            })
        }
        Frame::BGR0(BGRFrame {
            display_time,
            width,
            height,
            data,
        }) => {
            let mut bgra_data = Vec::with_capacity(data.len() / 3 * 4);
            for px in data.chunks_exact(3) {
                bgra_data.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            Some(BGRAFrame {
                display_time,
                width,
                height,
                data: bgra_data,
            })
        }
        Frame::YUVFrame(_) => None,
    }
}

fn bgra_to_rgba(bgra: &BGRAFrame) -> anyhow::Result<RgbaImage> {
    let width = u32::try_from(bgra.width).context("Invalid capture width")?;
    let height = u32::try_from(bgra.height).context("Invalid capture height")?;
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("Capture dimensions overflow")?;
    if bgra.data.len() < expected_len {
        anyhow::bail!(
            "Capture buffer too small (expected {expected_len} bytes, got {})",
            bgra.data.len()
        );
    }

    let mut rgba = vec![0u8; expected_len];
    for (src, dst) in bgra.data[..expected_len]
        .chunks_exact(4)
        .zip(rgba.chunks_exact_mut(4))
    {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }

    RgbaImage::from_raw(width, height, rgba).context("Building RGBA image")
}

fn crop_chat_region(rgba: RgbaImage, scale_factor: f32) -> anyhow::Result<RgbaImage> {
    let sidebar_px = (CHAT_SIDEBAR_WIDTH * scale_factor).round() as u32;
    let full_width = rgba.width();
    let height = rgba.height();
    if sidebar_px >= full_width {
        anyhow::bail!(
            "chat capture region is empty (sidebar={sidebar_px}px, image={full_width}px)"
        );
    }
    let crop_width = full_width - sidebar_px;
    Ok(image::imageops::crop_imm(&rgba, sidebar_px, 0, crop_width, height).to_image())
}

fn encode_png(rgba: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    rgba.write_to(&mut Cursor::new(&mut buffer), ImageFormat::Png)
        .context("Encoding capture PNG")?;
    Ok(buffer)
}

fn native_window_id(window: &Window) -> anyhow::Result<u32> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|_| anyhow::anyhow!("Reading native window handle"))?;
    match handle.as_raw() {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(appkit) => macos_window_id(appkit.ns_view.as_ptr()),
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(win32) => Ok(win32.hwnd.get() as u32),
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        RawWindowHandle::Xcb(xcb) => Ok(xcb.window.get()),
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        RawWindowHandle::Xlib(xlib) => Ok(xlib.window as u32),
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        RawWindowHandle::Wayland(_) => {
            anyhow::bail!("Window capture is not supported on Wayland sessions")
        }
        _ => anyhow::bail!("Unsupported window handle for screen capture"),
    }
}

#[cfg(target_os = "macos")]
fn macos_window_id(native_view: *mut std::ffi::c_void) -> anyhow::Result<u32> {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};

    if native_view.is_null() {
        anyhow::bail!("Native AppKit view is null");
    }

    unsafe {
        let native_view = native_view.cast::<Object>();
        let ns_window: *mut Object = msg_send![native_view, window];
        if ns_window.is_null() {
            anyhow::bail!("Native AppKit window is null");
        }
        let window_number: i32 = msg_send![ns_window, windowNumber];
        if window_number <= 0 {
            anyhow::bail!("Native AppKit window number is invalid");
        }
        Ok(window_number as u32)
    }
}

pub fn show_main_window(cx: &mut App) -> anyhow::Result<()> {
    let Some(handle) = handle(cx) else {
        anyhow::bail!("Main window not found");
    };
    cx.update_window(handle, |_, window, _| window.activate_window())?;
    Ok(())
}

pub fn set_composer_panel(cx: &mut App, kind: Option<&str>) -> anyhow::Result<Value> {
    let tab = match kind {
        None => None,
        Some(kind) => Some(match kind.to_ascii_lowercase().as_str() {
            "emoji" | "emojis" => crate::chat::gif_sticker_emoji::SubPanel::Emoji,
            "sticker" | "stickers" => crate::chat::gif_sticker_emoji::SubPanel::Stickers,
            "gif" | "gifs" => crate::chat::gif_sticker_emoji::SubPanel::Gifs,
            "sound" | "sounds" => crate::chat::gif_sticker_emoji::SubPanel::Sounds,
            other => anyhow::bail!("unknown panel kind: {other}"),
        }),
    };
    let composer = crate::chat::mention_input::MentionInput::active_composer(cx)
        .ok_or_else(|| anyhow::anyhow!("no composer is mounted; open a channel first"))?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        composer.update(cx, |composer, cx| match tab {
            Some(tab) => composer.show_panel(tab, window, cx),
            None => composer.hide_panel(cx),
        });
    })?;
    let open = composer.read(cx).active_panel(cx).map(|tab| match tab {
        crate::chat::gif_sticker_emoji::SubPanel::Emoji => "emoji",
        crate::chat::gif_sticker_emoji::SubPanel::Stickers => "sticker",
        crate::chat::gif_sticker_emoji::SubPanel::Gifs => "gif",
        crate::chat::gif_sticker_emoji::SubPanel::Sounds => "sound",
    });
    Ok(serde_json::json!({ "ok": true, "panel": open }))
}

pub const WHEEL_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);
pub const WHEEL_MAX_TICKS: u32 = 500;

pub fn message_viewport_state(cx: &App) -> Option<(usize, usize, bool)> {
    crate::chat::message::ChannelMessages::active_timeline(cx)
        .map(|timeline| timeline.read(cx).viewport_state())
}

pub fn prime_wheel_pointer(cx: &mut App) -> anyhow::Result<()> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let bounds = message_list_bounds(cx)
        .ok_or_else(|| anyhow::anyhow!("no message list is mounted; open a channel first"))?;
    cx.update_window(main_handle, |_, window, cx| {
        window.dispatch_event(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: bounds.center(),
                pressed_button: None,
                modifiers: gpui::Modifiers::default(),
            }),
            cx,
        );
        window.refresh();
    })?;
    Ok(())
}

pub fn dispatch_wheel_tick(cx: &mut App, delta_y: f32) -> anyhow::Result<bool> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let bounds = message_list_bounds(cx)
        .ok_or_else(|| anyhow::anyhow!("no message list is mounted; open a channel first"))?;
    let consumed = cx.update_window(main_handle, |_, window, cx| {
        let event = gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
            position: bounds.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(delta_y))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        !window.dispatch_event(event, cx).propagate
    })?;
    Ok(consumed)
}

fn message_list_bounds(cx: &App) -> Option<gpui::Bounds<gpui::Pixels>> {
    crate::chat::message::ChannelMessages::active_timeline(cx)
        .and_then(|timeline| timeline.read(cx).list_bounds())
        .filter(|bounds| bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.))
}

pub fn scroll_messages(cx: &mut App, to_top: bool) -> anyhow::Result<Value> {
    let timeline = crate::chat::message::ChannelMessages::active_timeline(cx)
        .ok_or_else(|| anyhow::anyhow!("no message list is mounted; open a channel first"))?;
    let (item_count, first_visible, at_bottom) = timeline.update(cx, |timeline, cx| {
        if to_top {
            timeline.scroll_viewport_to_top(cx);
        } else {
            timeline.scroll_viewport_to_bottom(cx);
        }
        timeline.viewport_state()
    });
    Ok(json!({
        "ok": true,
        "to": if to_top { "top" } else { "bottom" },
        "item_count": item_count,
        "first_visible_index": first_visible,
        "at_bottom": at_bottom,
    }))
}

pub fn open_message_image_viewer(
    settings: &gpui::Entity<mezon_store::Settings>,
    message_id: i64,
    attachment_index: usize,
    cx: &mut App,
) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    let (seed, id, create_time, uploader_id) = {
        let store = store.read(cx);
        let message = store
            .messages()
            .iter()
            .find(|message| message.id.0 == message_id)
            .ok_or_else(|| {
                anyhow::anyhow!("message {message_id} is not in the open channel's loaded history")
            })?;
        let attachment = message.attachments.get(attachment_index).ok_or_else(|| {
            anyhow::anyhow!("message {message_id} has no attachment at index {attachment_index}")
        })?;
        if attachment.presign_pending {
            anyhow::bail!(
                "attachment {attachment_index} of message {message_id} is still waiting for its \
                 presign to finish, so its url does not resolve yet"
            );
        }
        (
            mezon_store::AttachmentSeedInput::from_message(attachment),
            message.id,
            message.create_time,
            crate::chat::message::parts::viewer_uploader_id(message),
        )
    };
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let opened = seed.url.clone();
    cx.update_window(main_handle, |_, window, cx| {
        crate::chat::message::parts::open_viewer_from_message(
            settings,
            seed,
            id,
            create_time,
            uploader_id,
            window,
            cx,
        );
    })?;
    Ok(serde_json::json!({ "ok": true, "url": opened }))
}

pub fn go_back(cx: &mut App) -> anyhow::Result<()> {
    crate::router::Router::global(cx).update(cx, |router, _| router.go_back());
    Ok(())
}

pub fn go_forward(cx: &mut App) -> anyhow::Result<()> {
    crate::router::Router::global(cx).update(cx, |router, _| router.go_forward());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::frame_to_bgra;
    use scap::frame::{BGRxFrame, Frame, RGBFrame, RGBxFrame, XBGRFrame};

    #[test]
    fn converts_bgrx_frames() {
        let frame = frame_to_bgra(Frame::BGRx(BGRxFrame {
            display_time: 0,
            width: 1,
            height: 1,
            data: vec![1, 2, 3, 0],
        }))
        .expect("bgrx frame");
        assert_eq!(frame.data, vec![1, 2, 3, 255]);
    }

    #[test]
    fn converts_rgb_frames() {
        let frame = frame_to_bgra(Frame::RGB(RGBFrame {
            display_time: 0,
            width: 1,
            height: 1,
            data: vec![10, 20, 30],
        }))
        .expect("rgb frame");
        assert_eq!(frame.data, vec![30, 20, 10, 255]);
    }

    #[test]
    fn converts_rgbx_frames() {
        let frame = frame_to_bgra(Frame::RGBx(RGBxFrame {
            display_time: 0,
            width: 1,
            height: 1,
            data: vec![10, 20, 30, 0],
        }))
        .expect("rgbx frame");
        assert_eq!(frame.data, vec![30, 20, 10, 255]);
    }

    #[test]
    fn converts_xbgr_frames() {
        let frame = frame_to_bgra(Frame::XBGR(XBGRFrame {
            display_time: 0,
            width: 1,
            height: 1,
            data: vec![9, 1, 2, 3],
        }))
        .expect("xbgr frame");
        assert_eq!(frame.data, vec![1, 2, 3, 255]);
    }
}
