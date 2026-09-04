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
            None => composer.hide_panel(window, cx),
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

fn with_composer<R>(
    cx: &mut App,
    f: impl FnOnce(
        &mut crate::chat::mention_input::MentionInput,
        &mut Window,
        &mut gpui::Context<crate::chat::mention_input::MentionInput>,
    ) -> R,
) -> anyhow::Result<R> {
    let composer = crate::chat::mention_input::MentionInput::active_composer(cx)
        .ok_or_else(|| anyhow::anyhow!("no composer is mounted; open a channel first"))?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        composer.update(cx, |composer, cx| f(composer, window, cx))
    })
}

fn composer_snapshot(cx: &mut App) -> anyhow::Result<Value> {
    let composer = crate::chat::mention_input::MentionInput::active_composer(cx)
        .ok_or_else(|| anyhow::anyhow!("no composer is mounted; open a channel first"))?;
    let composer = composer.read(cx);
    let (popup_open, selected, suggestions) = composer.probe_suggestions();
    let reply_target = mezon_store::MessagesStore::global(cx)
        .read(cx)
        .reply_target()
        .map(|draft| {
            json!({
                "message_id": draft.message_ref_id.get().to_string(),
                "sender": draft.sender_name,
            })
        });
    Ok(json!({
        "text": composer.probe_text(cx).to_string(),
        // Staged, not dropped: a dropped file is read on a background task, so
        // submitting before it lands here sends the message without it.
        "attachments": composer.probe_attachments(),
        // What the next submit will answer, if anything.
        "reply_target": reply_target,
        "popup_open": popup_open,
        "selected": selected,
        "suggestions": suggestions,
        "panel": composer.active_panel(cx).map(|tab| match tab {
            crate::chat::gif_sticker_emoji::SubPanel::Emoji => "emoji",
            crate::chat::gif_sticker_emoji::SubPanel::Stickers => "sticker",
            crate::chat::gif_sticker_emoji::SubPanel::Gifs => "gif",
            crate::chat::gif_sticker_emoji::SubPanel::Sounds => "sound",
        }),
    }))
}

pub fn composer_type(cx: &mut App, text: &str) -> anyhow::Result<Value> {
    with_composer(cx, |composer, window, cx| {
        composer.probe_set_text(text, window, cx);
    })?;
    composer_snapshot(cx)
}

pub fn composer_state(cx: &mut App) -> anyhow::Result<Value> {
    composer_snapshot(cx)
}

pub fn composer_pick(cx: &mut App, index: usize) -> anyhow::Result<Value> {
    let accepted = with_composer(cx, |composer, window, cx| {
        composer.probe_accept(index, window, cx)
    })?;
    if !accepted {
        anyhow::bail!("no suggestion at index {index}; call composer_type first");
    }
    composer_snapshot(cx)
}

fn edit_input_entity(
    cx: &mut App,
) -> anyhow::Result<(
    gpui::Entity<crate::chat::message::ChannelMessages>,
    gpui::Entity<crate::chat::mention_input::MentionInput>,
)> {
    let timeline = crate::chat::message::ChannelMessages::active_timeline(cx)
        .ok_or_else(|| anyhow::anyhow!("no message list is mounted; open a channel first"))?;
    let input = timeline
        .read(cx)
        .probe_edit_input()
        .ok_or_else(|| anyhow::anyhow!("no message is being edited; call edit_begin first"))?
        .1;
    Ok((timeline, input))
}

fn edit_snapshot(cx: &mut App) -> anyhow::Result<Value> {
    let (timeline, input) = edit_input_entity(cx)?;
    let message_id = timeline
        .read(cx)
        .probe_edit_input()
        .map(|(id, _)| id.get().to_string());
    let input = input.read(cx);
    let (popup_open, selected, suggestions) = input.probe_suggestions();
    Ok(json!({
        "message_id": message_id,
        "text": input.probe_text(cx).to_string(),
        "popup_open": popup_open,
        "selected": selected,
        "suggestions": suggestions,
    }))
}

struct ActiveTopicPanel {
    composer: gpui::WeakEntity<crate::chat::mention_input::MentionInput>,
    timeline: gpui::WeakEntity<crate::chat::message::ChannelMessages>,
}
impl gpui::Global for ActiveTopicPanel {}

pub fn register_active_topic_panel(
    composer: &gpui::Entity<crate::chat::mention_input::MentionInput>,
    timeline: &gpui::Entity<crate::chat::message::ChannelMessages>,
    cx: &mut App,
) {
    cx.set_global(ActiveTopicPanel {
        composer: composer.downgrade(),
        timeline: timeline.downgrade(),
    });
}

fn topic_panel(
    cx: &App,
) -> anyhow::Result<(
    gpui::Entity<crate::chat::mention_input::MentionInput>,
    gpui::Entity<crate::chat::message::ChannelMessages>,
)> {
    let panel = cx
        .try_global::<ActiveTopicPanel>()
        .ok_or_else(|| anyhow::anyhow!("no topic panel is mounted; call open_topic first"))?;
    let composer = panel
        .composer
        .upgrade()
        .ok_or_else(|| anyhow::anyhow!("the topic panel was closed"))?;
    let timeline = panel
        .timeline
        .upgrade()
        .ok_or_else(|| anyhow::anyhow!("the topic panel was closed"))?;
    Ok((composer, timeline))
}

fn topic_snapshot(cx: &App) -> anyhow::Result<Value> {
    let topics = mezon_store::TopicsStore::global(cx);
    let topics = topics.read(cx);
    let messages = mezon_store::MessagesStore::global(cx);
    let messages = messages.read(cx);
    let topic_id = topics.active_topic_id();
    let loaded = topic_id
        .map(|id| {
            messages
                .messages_in_channel(mezon_store::ChannelId(id))
                .len()
        })
        .unwrap_or(0);
    let (item_count, first_visible, at_bottom) = topic_panel(cx)
        .map(|(_, timeline)| timeline.read(cx).viewport_state())
        .unwrap_or((0, 0, false));
    let attachments = topic_panel(cx)
        .map(|(composer, _)| composer.read(cx).probe_attachments())
        .unwrap_or_default();
    Ok(json!({
        // The store's flag and the mounted view are two different things: an
        // occluded window drops the panel entity while the store still says the
        // topic is open, and every write tool then fails with "no topic panel is
        // mounted". Report both so a caller can tell those apart.
        "panel_open": topics.is_panel_open(),
        "panel_mounted": topic_panel(cx).is_ok(),
        "topic_id": topic_id.map(|id| id.to_string()),
        "origin_message_id": topics.origin_message().map(|m| m.id.get().to_string()),
        "loaded_count": loaded,
        "loading": messages.is_topic_loading(),
        "has_more_top": messages.topic_has_more_top(),
        "has_more_bottom": messages.topic_has_more_bottom(),
        "loading_more": messages.is_topic_loading_more(),
        "item_count": item_count,
        "first_visible_index": first_visible,
        "at_bottom": at_bottom,
        // Same reason as composer_state: topic_drop_paths returns before the file
        // is read, so this is what says the next topic_submit will carry it.
        "attachments": attachments,
    }))
}

pub fn open_topic(cx: &mut App, message_id: i64) -> anyhow::Result<Value> {
    let messages = mezon_store::MessagesStore::global(cx);
    if messages
        .read(cx)
        .viewport_message_by_id(mezon_store::MessageId(message_id))
        .is_none()
    {
        anyhow::bail!(
            "message {message_id} is not in the open channel's loaded history; open_channel and load_more_messages first"
        );
    }
    mezon_store::TopicsStore::global(cx).update(cx, |store, cx| {
        store.start_create_for_message(mezon_store::MessageId(message_id), cx);
    });
    if let Some(main_handle) = handle(cx) {
        cx.update_window(main_handle, |_, window, _| window.refresh())?;
    }
    topic_snapshot(cx)
}

pub fn close_topic(cx: &mut App) -> anyhow::Result<Value> {
    mezon_store::TopicsStore::global(cx).update(cx, |store, cx| store.close_panel(cx));
    Ok(json!({ "ok": true }))
}

pub fn topic_state(cx: &App) -> anyhow::Result<Value> {
    topic_snapshot(cx)
}

pub fn topic_type(cx: &mut App, text: &str) -> anyhow::Result<Value> {
    let (composer, _) = topic_panel(cx)?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        composer.update(cx, |composer, cx| composer.probe_set_text(text, window, cx));
    })?;
    topic_composer_snapshot(cx)
}

/// Accept one entry of the topic composer's suggestion popup — the `@`/`#`/`:` flow.
///
/// Typing `@name` alone only produces literal text: a real mention needs the picked
/// entry, which is what writes the id into the message's `mentions` field. Detection on
/// the receiving side reads that field, never the text, so a topic mention can only be
/// exercised end-to-end through here.
pub fn topic_pick(cx: &mut App, index: usize) -> anyhow::Result<Value> {
    let (composer, _) = topic_panel(cx)?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let accepted = cx.update_window(main_handle, |_, window, cx| {
        composer.update(cx, |composer, cx| composer.probe_accept(index, window, cx))
    })?;
    if !accepted {
        anyhow::bail!("no suggestion at index {index}; call topic_type first");
    }
    topic_composer_snapshot(cx)
}

fn topic_composer_snapshot(cx: &mut App) -> anyhow::Result<Value> {
    let (composer, _) = topic_panel(cx)?;
    let composer = composer.read(cx);
    let (popup_open, selected, suggestions) = composer.probe_suggestions();
    Ok(json!({
        "ok": true,
        "text": composer.probe_text(cx).to_string(),
        "attachments": composer.probe_attachments(),
        "popup_open": popup_open,
        "selected": selected,
        "suggestions": suggestions,
    }))
}

pub fn topic_submit(cx: &mut App) -> anyhow::Result<Value> {
    let (composer, _) = topic_panel(cx)?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let sent = composer.read(cx).probe_text(cx).to_string();
    cx.update_window(main_handle, |_, window, cx| {
        composer.update(cx, |composer, cx| composer.probe_enter(window, cx));
    })?;
    Ok(json!({ "ok": true, "sent": sent }))
}

pub fn topic_viewport_state(cx: &App) -> Option<(usize, usize, bool)> {
    topic_panel(cx)
        .ok()
        .map(|(_, timeline)| timeline.read(cx).viewport_state())
}

fn topic_list_bounds(cx: &App) -> Option<gpui::Bounds<gpui::Pixels>> {
    topic_panel(cx)
        .ok()
        .and_then(|(_, timeline)| timeline.read(cx).list_bounds())
        .filter(|bounds| bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.))
}

pub fn prime_topic_wheel_pointer(cx: &mut App) -> anyhow::Result<()> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let bounds = topic_list_bounds(cx)
        .ok_or_else(|| anyhow::anyhow!("no topic list is mounted; call open_topic first"))?;
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

pub fn dispatch_topic_wheel_tick(cx: &mut App, delta_y: f32) -> anyhow::Result<bool> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let bounds = topic_list_bounds(cx)
        .ok_or_else(|| anyhow::anyhow!("no topic list is mounted; call open_topic first"))?;
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

pub fn edit_begin(cx: &mut App, message_id: i64) -> anyhow::Result<Value> {
    let timeline = crate::chat::message::ChannelMessages::active_timeline(cx)
        .ok_or_else(|| anyhow::anyhow!("no message list is mounted; open a channel first"))?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        timeline.update(cx, |timeline, cx| {
            timeline.begin_edit(mezon_store::MessageId(message_id), window, cx);
        });
    })?;
    edit_snapshot(cx)
}

pub fn edit_type(cx: &mut App, text: &str) -> anyhow::Result<Value> {
    let (_, input) = edit_input_entity(cx)?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        input.update(cx, |input, cx| input.probe_set_text(text, window, cx));
    })?;
    edit_snapshot(cx)
}

pub fn edit_pick(cx: &mut App, index: usize) -> anyhow::Result<Value> {
    let (_, input) = edit_input_entity(cx)?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let accepted = cx.update_window(main_handle, |_, window, cx| {
        input.update(cx, |input, cx| input.probe_accept(index, window, cx))
    })?;
    if !accepted {
        anyhow::bail!("no suggestion at index {index}");
    }
    edit_snapshot(cx)
}

pub fn edit_state(cx: &mut App) -> anyhow::Result<Value> {
    edit_snapshot(cx)
}

pub fn edit_save(cx: &mut App) -> anyhow::Result<Value> {
    let before = edit_snapshot(cx)?;
    let timeline = crate::chat::message::ChannelMessages::active_timeline(cx)
        .ok_or_else(|| anyhow::anyhow!("no message list is mounted"))?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        timeline.update(cx, |timeline, cx| timeline.save_edit(window, cx));
    })?;
    Ok(json!({ "ok": true, "saved": before }))
}

pub fn composer_panel_send(
    cx: &mut App,
    kind: &str,
    url: String,
    filename: String,
    width: i32,
    height: i32,
) -> anyhow::Result<Value> {
    with_composer(cx, |composer, window, cx| {
        composer.probe_panel_send(kind, url, filename, width, height, window, cx)
    })??;
    Ok(json!({ "ok": true, "kind": kind }))
}

pub fn composer_drop_paths(cx: &mut App, paths: Vec<String>) -> anyhow::Result<Value> {
    let missing: Vec<&String> = paths
        .iter()
        .filter(|path| !std::path::Path::new(path).is_file())
        .collect();
    if let Some(path) = missing.first() {
        anyhow::bail!("file not found: {path}");
    }
    let count = paths.len();
    with_composer(cx, |composer, window, cx| {
        composer.add_dropped_paths(paths.into_iter().map(Into::into).collect(), window, cx);
    })?;
    Ok(json!({ "ok": true, "dropped": count }))
}

pub fn composer_submit(cx: &mut App) -> anyhow::Result<Value> {
    let before = composer_snapshot(cx)?;
    with_composer(cx, |composer, window, cx| {
        composer.probe_enter(window, cx);
    })?;
    Ok(json!({ "ok": true, "sent": before }))
}

/// Same as [`composer_drop_paths`] but onto the topic panel's own composer.
/// `with_composer` resolves the ACTIVE composer, which stays the channel's even
/// while the topic panel is open, so without this an attachment aimed at a topic
/// silently lands in the parent channel instead.
pub fn topic_drop_paths(cx: &mut App, paths: Vec<String>) -> anyhow::Result<Value> {
    if let Some(path) = paths
        .iter()
        .find(|path| !std::path::Path::new(path).is_file())
    {
        anyhow::bail!("file not found: {path}");
    }
    let count = paths.len();
    let (composer, _) = topic_panel(cx)?;
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        composer.update(cx, |composer, cx| {
            composer.add_dropped_paths(paths.into_iter().map(Into::into).collect(), window, cx);
        });
    })?;
    // Staging is asynchronous: this reports what was handed over, not what is
    // ready. Poll topic_state until `attachments` lists the files before calling
    // topic_submit, or the reply goes out without them.
    Ok(json!({ "ok": true, "dropped": count, "staged": false }))
}

/// Aim the composer at a message without sending anything, the way the row's
/// "Reply" action does. `reply_to_message` posts a text reply in one shot, so it
/// cannot carry a staged attachment — this is what lets a caller build a reply
/// that has one.
pub fn reply_begin(cx: &mut App, message_id: i64) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    store.update(cx, |store, cx| {
        store.set_reply_to(mezon_store::MessageId::new(message_id), cx)
    });
    let target = store.read(cx).reply_target().map(|draft| {
        json!({
            "message_id": draft.message_ref_id.get().to_string(),
            "sender": draft.sender_name,
        })
    });
    let Some(target) = target else {
        anyhow::bail!(
            "message {message_id} is not in the open channel's loaded history; open_channel and load_more_messages first"
        );
    };
    Ok(json!({ "ok": true, "reply_target": target }))
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

pub fn member_list_snapshot(cx: &App) -> anyhow::Result<Value> {
    Ok(crate::chat::member_list::member_list_snapshot(cx))
}

pub fn banned_users_task(
    cx: &mut App,
    clan_id: i64,
    channel_id: i64,
) -> gpui::Task<anyhow::Result<Value>> {
    let Some(store) = mezon_store::BannedUsersStore::try_global(cx) else {
        return gpui::Task::ready(Err(anyhow::anyhow!("banned users store unavailable")));
    };
    let task = store.update(cx, |store, cx| {
        store.fetch_raw(
            mezon_store::ClanId(clan_id),
            mezon_store::ChannelId(channel_id),
            cx,
        )
    });
    cx.background_spawn(async move {
        let entries = task.await?;
        Ok(json!({
            "clan_id": clan_id.to_string(),
            "channel_id": channel_id.to_string(),
            "count": entries.len(),
            "banned_users": entries
                .into_iter()
                .map(|e| json!({
                    "channel_id": e.channel_id.to_string(),
                    "banned_id": e.banned_id.to_string(),
                    "banner_id": e.banner_id.to_string(),
                    "ban_time": e.ban_time,
                    "reason": e.reason,
                }))
                .collect::<Vec<_>>(),
        }))
    })
}

pub fn close_modal(cx: &mut App) -> anyhow::Result<Value> {
    let shell = crate::app::shell::Shell::try_global(cx)
        .ok_or_else(|| anyhow::anyhow!("shell unavailable"))?;
    let had_modal = shell.read(cx).has_modal();
    shell.update(cx, |shell, cx| shell.close_modal(cx));
    Ok(json!({ "ok": true, "closed": had_modal }))
}

pub fn member_menu_state(cx: &App) -> anyhow::Result<Value> {
    crate::chat::member_list::member_menu_state(cx)
}

pub fn member_menu_open(
    cx: &mut App,
    user_id: mezon_store::UserId,
    x: f32,
    y: f32,
) -> anyhow::Result<Value> {
    crate::chat::member_list::member_menu_open(user_id, gpui::point(gpui::px(x), gpui::px(y)), cx)
}

pub fn member_menu_close(cx: &mut App) -> anyhow::Result<Value> {
    crate::chat::member_list::member_menu_close(cx)
}

pub fn member_menu_pick(cx: &mut App, index: usize, value: Option<i32>) -> anyhow::Result<Value> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        crate::chat::member_list::member_menu_pick(index, value, window, cx)
    })?
}

pub fn create_clan_task(
    cx: &mut App,
    name: String,
    logo: String,
) -> gpui::Task<anyhow::Result<Value>> {
    let Some(clan_list) = mezon_store::ClanList::try_global(cx) else {
        return gpui::Task::ready(Err(anyhow::anyhow!("clan store unavailable")));
    };
    let task = clan_list.update(cx, |list, cx| list.create_clan(name.clone(), logo, cx));
    cx.spawn(async move |_| {
        let clan_id = task.await.map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(json!({ "ok": true, "clan_id": clan_id, "name": name }))
    })
}

pub fn open_create_clan_modal(cx: &mut App) -> anyhow::Result<Value> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        let clan_list = mezon_store::ClanList::global(cx);
        let settings = mezon_store::Settings::try_global(cx)
            .ok_or_else(|| anyhow::anyhow!("settings unavailable"))?;
        let modal = cx.new(|cx| {
            crate::clan::create_clan_modal::CreateClanModal::new(clan_list, settings, window, cx)
        });
        crate::app::shell::Shell::global(cx)
            .update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
        anyhow::Ok(json!({ "ok": true }))
    })?
}

pub fn clan_menu_state(cx: &App) -> anyhow::Result<Value> {
    crate::sidebar::clan_sidebar::clan_menu_state(cx)
}

pub fn clan_menu_open(
    cx: &mut App,
    clan_id: mezon_store::ClanId,
    x: f32,
    y: f32,
) -> anyhow::Result<Value> {
    crate::sidebar::clan_sidebar::clan_menu_open(clan_id, gpui::point(gpui::px(x), gpui::px(y)), cx)
}

pub fn clan_menu_close(cx: &mut App) -> anyhow::Result<Value> {
    crate::sidebar::clan_sidebar::clan_menu_close(cx)
}

pub fn clan_menu_pick(cx: &mut App, index: usize, value: Option<i32>) -> anyhow::Result<Value> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        crate::sidebar::clan_sidebar::clan_menu_pick(index, value, window, cx)
    })?
}

pub fn list_categories(cx: &App, clan_id: mezon_store::ClanId) -> anyhow::Result<Value> {
    let channels = mezon_store::ChannelList::try_global(cx)
        .ok_or_else(|| anyhow::anyhow!("channel store unavailable"))?;
    let channels = channels.read(cx);
    let items: Vec<Value> = channels
        .categories_for_clan(clan_id)
        .iter()
        .map(|category| {
            json!({
                "id": category.id,
                "name": category.name,
                "order": category.order,
                "channel_count": category.channels.len(),
                "collapsed": channels.is_category_collapsed(clan_id, &category.id),
            })
        })
        .collect();
    Ok(Value::Array(items))
}

pub fn create_category_task(
    cx: &mut App,
    clan_id: mezon_store::ClanId,
    name: String,
) -> gpui::Task<anyhow::Result<Value>> {
    let Some(channels) = mezon_store::ChannelList::try_global(cx) else {
        return gpui::Task::ready(Err(anyhow::anyhow!("channel store unavailable")));
    };
    let task = channels.update(cx, |list, cx| {
        list.create_category(clan_id, name.clone(), cx)
    });
    cx.background_spawn(async move {
        task.await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(json!({ "ok": true, "name": name }))
    })
}

pub fn channel_menu_state(cx: &App) -> anyhow::Result<Value> {
    crate::sidebar::channel_sidebar::channel_menu_state(cx)
}

pub fn channel_menu_open(
    cx: &mut App,
    clan_id: mezon_store::ClanId,
    channel_id: mezon_store::ChannelId,
    x: f32,
    y: f32,
    in_favorites: bool,
) -> anyhow::Result<Value> {
    crate::sidebar::channel_sidebar::channel_menu_open(
        clan_id,
        channel_id,
        gpui::point(gpui::px(x), gpui::px(y)),
        in_favorites,
        cx,
    )
}

pub fn channel_menu_close(cx: &mut App) -> anyhow::Result<Value> {
    crate::sidebar::channel_sidebar::channel_menu_close(cx)
}

pub fn channel_menu_pick(cx: &mut App, index: usize, value: Option<i32>) -> anyhow::Result<Value> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        crate::sidebar::channel_sidebar::channel_menu_pick(index, value, window, cx)
    })?
}

pub fn category_menu_state(cx: &App) -> anyhow::Result<Value> {
    crate::sidebar::channel_sidebar::category_menu_state(cx)
}

pub fn category_menu_open(
    cx: &mut App,
    clan_id: mezon_store::ClanId,
    category_id: String,
    x: f32,
    y: f32,
) -> anyhow::Result<Value> {
    crate::sidebar::channel_sidebar::category_menu_open(
        clan_id,
        category_id,
        gpui::point(gpui::px(x), gpui::px(y)),
        cx,
    )
}

pub fn category_menu_close(cx: &mut App) -> anyhow::Result<Value> {
    crate::sidebar::channel_sidebar::category_menu_close(cx)
}

pub fn category_menu_pick(cx: &mut App, index: usize, value: Option<i32>) -> anyhow::Result<Value> {
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    cx.update_window(main_handle, |_, window, cx| {
        crate::sidebar::channel_sidebar::category_menu_pick(index, value, window, cx)
    })?
}

pub fn user_status_snapshot(cx: &App) -> anyhow::Result<Value> {
    let Some((user_id, status)) = mezon_store::current_user_status(cx) else {
        return Ok(json!({ "signed_in": false }));
    };
    let raw = mezon_store::AccountStore::try_global(cx)
        .and_then(|store| {
            store
                .read(cx)
                .account
                .as_ref()
                .map(|account| account.status.clone())
        })
        .unwrap_or_default();
    Ok(json!({
        "signed_in": true,
        "user_id": user_id.to_string(),
        "account_status": raw,
        "presence": format!("{:?}", status.presence),
        "custom_status": status.custom_status,
        "online": status.online,
    }))
}

pub fn set_user_status(
    cx: &mut App,
    status: &str,
    minutes: i32,
    until_turn_on: bool,
) -> anyhow::Result<Value> {
    let store = mezon_store::AccountStore::try_global(cx)
        .ok_or_else(|| anyhow::anyhow!("not signed in"))?;
    let before = store
        .read(cx)
        .account
        .as_ref()
        .map(|account| account.status.clone())
        .ok_or_else(|| anyhow::anyhow!("not signed in"))?;
    let status_changed = before != status;
    store.update(cx, |store, cx| {
        store.set_status(status.to_string(), minutes, until_turn_on, cx)
    });
    let after = store
        .read(cx)
        .account
        .as_ref()
        .map(|account| account.status.clone())
        .unwrap_or_default();
    Ok(json!({
        "ok": true,
        "requested": status,
        "minutes": minutes,
        "until_turn_on": until_turn_on,
        "status_before": before,
        "status_after": after,
        "status_changed": status_changed,
    }))
}

/// `topic` reads the open topic panel's buffer instead of the channel's. The
/// two are separate buckets and the channel stays "active" while a topic is
/// open, so without the switch a reply just sent into a topic is invisible here
/// — the caller would be looking at the parent channel and see nothing arrive.
pub fn list_loaded_messages(cx: &App, limit: usize, topic: bool) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    let store = store.read(cx);
    let topic_id = topic
        .then(|| {
            mezon_store::TopicsStore::global(cx)
                .read(cx)
                .active_topic_id()
        })
        .flatten();
    if topic && topic_id.is_none() {
        anyhow::bail!("no topic is open; call open_topic first");
    }
    let rows = match topic_id {
        Some(id) => store.messages_in_channel(mezon_store::ChannelId(id)),
        None => store.messages(),
    };
    let row_json = |m: &mezon_store::Message| {
        json!({
            "message_id": m.id.to_string(),
            "optimistic": m.id.is_optimistic(),
            "sort_id": m.sort_id,
            "sender": m.sender_name,
            "send_failed": m.send_failed,
            // Unix seconds as the row holds them. The presign expiry is computed
            // from this, so a row that shows 0 here is one the sweep can never act
            // on however old it gets.
            "create_time": m.create_time,
            "content": m.content.chars().take(60).collect::<String>(),
            // Attachment delivery state, so a test can tell "still uploading" apart
            // from "rendered" without reading pixels.
            "attachments": m.attachments.iter().map(|a| json!({
                "filetype": a.filetype,
                "name": a.filename,
                "uploading": a.uploading,
                "presign_pending": a.presign_pending,
                "upload_failed": a.upload_failed,
                "url": a.url,
            })).collect::<Vec<_>>(),
        })
    };
    let head: Vec<Value> = rows.iter().take(limit).map(row_json).collect();
    let tail: Vec<Value> = rows
        .iter()
        .skip(rows.len().saturating_sub(limit))
        .map(row_json)
        .collect();
    Ok(json!({
        "channel_id": match topic_id {
            Some(id) => Some(id.to_string()),
            None => store.active_channel_id().map(|id| id.to_string()),
        },
        "bucket": if topic_id.is_some() { "topic" } else { "channel" },
        "loaded_count": rows.len(),
        "has_more_bottom": if topic_id.is_some() {
            store.topic_has_more_bottom()
        } else {
            store.has_more_bottom()
        },
        "oldest": head,
        "newest": tail,
    }))
}

pub fn jump_to_message(cx: &mut App, message_id: i64) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    store.update(cx, |store, cx| {
        store.jump_to_message(mezon_store::MessageId::new(message_id), cx)
    });
    Ok(json!({ "ok": true, "message_id": message_id.to_string() }))
}

pub fn jump_to_present(cx: &mut App) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    store.update(cx, |store, cx| store.jump_to_present(cx));
    Ok(json!({ "ok": true }))
}

// Mirror the row's own click gate (`parts.rs`): the tile is inert while the
// bytes are still going up, after they failed, or before the url exists.
// Without these a tool opens a viewer on an empty url and answers ok, which is
// a pass for something a user cannot do.
fn ensure_attachment_viewable(
    attachment: &mezon_store::MessageAttachment,
    message_id: i64,
    attachment_index: usize,
) -> anyhow::Result<()> {
    if attachment.uploading {
        anyhow::bail!(
            "attachment {attachment_index} of message {message_id} is still uploading; the row \
             does not open the viewer yet"
        );
    }
    if attachment.upload_failed {
        anyhow::bail!(
            "attachment {attachment_index} of message {message_id} failed to upload, so there \
             is nothing to view"
        );
    }
    if attachment.presign_pending {
        anyhow::bail!(
            "attachment {attachment_index} of message {message_id} is still waiting for its \
             presign to finish, so its url does not resolve yet"
        );
    }
    if attachment.url.is_empty() {
        anyhow::bail!("attachment {attachment_index} of message {message_id} has no url yet");
    }
    Ok(())
}

pub fn open_message_image_viewer(
    settings: &gpui::Entity<mezon_store::Settings>,
    message_id: i64,
    attachment_index: usize,
    cx: &mut App,
) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    let topic_id = mezon_store::TopicsStore::global(cx)
        .read(cx)
        .active_topic_id();
    let (seed, id, create_time, uploader_id) = {
        let store = store.read(cx);
        // A reply lives in the topic's own bucket, not the channel's, so looking
        // only at the channel makes every topic attachment unreachable from here.
        let message = store
            .messages()
            .iter()
            .find(|message| message.id.0 == message_id)
            .or_else(|| {
                topic_id.and_then(|id| {
                    store
                        .messages_in_channel(mezon_store::ChannelId(id))
                        .iter()
                        .find(|message| message.id.0 == message_id)
                })
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "message {message_id} is not in the open channel's loaded history, nor in the open topic's"
                )
            })?;
        let attachment = message.attachments.get(attachment_index).ok_or_else(|| {
            anyhow::anyhow!("message {message_id} has no attachment at index {attachment_index}")
        })?;
        ensure_attachment_viewable(attachment, message_id, attachment_index)?;
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

pub fn open_message_pdf_viewer(
    settings: &gpui::Entity<mezon_store::Settings>,
    message_id: i64,
    attachment_index: usize,
    cx: &mut App,
) -> anyhow::Result<Value> {
    let store = mezon_store::MessagesStore::global(cx);
    let topic_id = mezon_store::TopicsStore::global(cx)
        .read(cx)
        .active_topic_id();
    let (url, filename) = {
        let store = store.read(cx);
        let message = store
            .messages()
            .iter()
            .find(|message| message.id.0 == message_id)
            .or_else(|| {
                topic_id.and_then(|id| {
                    store
                        .messages_in_channel(mezon_store::ChannelId(id))
                        .iter()
                        .find(|message| message.id.0 == message_id)
                })
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "message {message_id} is not in the open channel's loaded history, nor in the open topic's"
                )
            })?;
        let attachment = message.attachments.get(attachment_index).ok_or_else(|| {
            anyhow::anyhow!("message {message_id} has no attachment at index {attachment_index}")
        })?;
        ensure_attachment_viewable(attachment, message_id, attachment_index)?;
        if !mezon_store::is_pdf(&attachment.filetype, &attachment.filename) {
            anyhow::bail!(
                "attachment {attachment_index} of message {message_id} is not a pdf, so the row \
                 shows no view button"
            );
        }
        (
            gpui::SharedString::from(attachment.url.clone()),
            gpui::SharedString::from(attachment.filename.clone()),
        )
    };
    let main_handle = handle(cx).ok_or_else(|| anyhow::anyhow!("main window not found"))?;
    let opened = url.clone();
    cx.update_window(main_handle, |_, window, cx| {
        crate::pdf_viewer::open_pdf_viewer(
            crate::pdf_viewer::OpenPdfRequest {
                url,
                filename,
                settings: settings.clone(),
            },
            window,
            cx,
        );
    })?;
    Ok(serde_json::json!({ "ok": true, "url": opened }))
}

pub fn join_voice(channel_id: i64, clan_id: i64, cx: &mut App) -> anyhow::Result<Value> {
    let voice = mezon_store::VoiceStore::try_global(cx)
        .ok_or_else(|| anyhow::anyhow!("the voice store is not available"))?;
    let settings = mezon_store::Settings::try_global(cx)
        .ok_or_else(|| anyhow::anyhow!("settings are not available"))?;
    let (input, output, camera) = {
        let settings = settings.read(cx);
        (
            settings.input_device_id.clone(),
            settings.output_device_id.clone(),
            settings.camera_device_id.clone(),
        )
    };
    let handle = cx
        .active_window()
        .or_else(|| cx.windows().first().copied())
        .ok_or_else(|| anyhow::anyhow!("no window is open"))?;
    handle
        .update(cx, |_, window, cx| {
            voice.update(cx, |store, cx| {
                store.join(
                    channel_id.to_string(),
                    clan_id.to_string(),
                    String::new(),
                    input,
                    output,
                    camera,
                    window,
                    cx,
                );
            });
        })
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(serde_json::json!({ "ok": true }))
}

pub fn leave_voice(cx: &mut App) -> anyhow::Result<Value> {
    let voice = mezon_store::VoiceStore::try_global(cx)
        .ok_or_else(|| anyhow::anyhow!("the voice store is not available"))?;
    let handle = cx
        .active_window()
        .or_else(|| cx.windows().first().copied())
        .ok_or_else(|| anyhow::anyhow!("no window is open"))?;
    handle
        .update(cx, |_, window, cx| {
            voice.update(cx, |store, cx| store.leave(window, cx));
        })
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(serde_json::json!({ "ok": true }))
}

pub fn recording_state(cx: &mut App) -> Value {
    let Some(voice) = mezon_store::VoiceStore::try_global(cx) else {
        return serde_json::json!({ "state": "unavailable" });
    };
    let store = voice.read(cx);
    let state = match store.recording_state() {
        mezon_store::RecordingState::Idle => "idle",
        mezon_store::RecordingState::Starting => "starting",
        mezon_store::RecordingState::Recording => "recording",
        mezon_store::RecordingState::Stopping => "stopping",
    };
    serde_json::json!({
        "state": state,
        "elapsed_seconds": store.recording_elapsed().as_secs_f64(),
        "video_stalled": store.recording_stalled(),
        "can_record": store.can_record(),
        "in_call": store.connection().active_channel_id().is_some(),
    })
}

pub fn start_recording(path: Option<String>, cx: &mut App) -> anyhow::Result<Value> {
    let voice = mezon_store::VoiceStore::try_global(cx)
        .ok_or_else(|| anyhow::anyhow!("the voice store is not available"))?;
    let window_id = cx
        .active_window()
        .or_else(|| cx.windows().first().copied())
        .and_then(|handle| {
            handle
                .update(cx, |_, window, _| {
                    crate::chat::record_window::record_window_id(window)
                })
                .ok()
                .flatten()
        });
    voice.update(cx, |store, cx| {
        let target = match path {
            Some(path) => std::path::PathBuf::from(path),
            None => store.suggested_recording_path(),
        };
        store
            .start_recording_at(target.clone(), window_id, cx)
            .map(|()| serde_json::json!({ "ok": true, "path": target.to_string_lossy() }))
            .map_err(|error| anyhow::anyhow!(error))
    })
}

pub fn stop_recording(cx: &mut App) -> anyhow::Result<Value> {
    let voice = mezon_store::VoiceStore::try_global(cx)
        .ok_or_else(|| anyhow::anyhow!("the voice store is not available"))?;
    voice.update(cx, |store, cx| {
        store
            .request_stop_recording(cx)
            .map(|()| serde_json::json!({ "ok": true }))
            .map_err(|error| anyhow::anyhow!(error))
    })
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
