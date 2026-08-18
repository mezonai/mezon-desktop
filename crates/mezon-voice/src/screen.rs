use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mezon_codec::I420Frame;
use parking_lot::{Condvar, Mutex};
use scap::capturer::{Capturer, Options, Resolution};
use scap::frame::FrameType;
#[cfg(any(not(target_os = "macos"), test))]
use scap::frame::{BGRAFrame, Frame};

#[cfg(target_os = "macos")]
type CapturedScreenFrame = scap::capturer::engine::mac::PixelBuffer;
#[cfg(not(target_os = "macos"))]
type CapturedScreenFrame = BGRAFrame;

use crate::screen_picker::{
    PickedScreen, pick_is_window, pick_uses_portal, portal_source_types_for_pick,
    scap_target_for_pick,
};
#[cfg(not(target_os = "macos"))]
use crate::video::bgra_to_i420;
#[cfg(target_os = "macos")]
use crate::video::i420_to_bgra_into;
#[cfg(target_os = "macos")]
use crate::video::nv12_full_to_i420;
use crate::video::{VideoFrameStore, local_screen_key};

const CAPTURE_FPS: u32 = 30;
#[cfg(not(target_os = "macos"))]
const PREVIEW_MAX_WIDTH: u32 = 1280;
#[cfg(not(target_os = "macos"))]
const PREVIEW_MAX_HEIGHT: u32 = 800;
const SLOT_WAIT: Duration = Duration::from_millis(250);
#[cfg(target_os = "macos")]
const MAX_SCREEN_RESTART_ATTEMPTS: u32 = 5;
#[cfg(target_os = "macos")]
const SCREEN_RESTART_DELAY: Duration = Duration::from_millis(500);

#[derive(Default)]
struct SlotState {
    frame: Option<CapturedScreenFrame>,
    closed: bool,
    error: Option<String>,
}

#[derive(Default)]
struct LatestFrameSlot {
    state: Mutex<SlotState>,
    cond: Condvar,
}

impl LatestFrameSlot {
    fn publish(&self, frame: CapturedScreenFrame) {
        self.state.lock().frame = Some(frame);
        self.cond.notify_one();
    }

    fn close(&self) {
        self.state.lock().closed = true;
        self.cond.notify_one();
    }

    fn fail(&self, error: String) {
        let mut state = self.state.lock();
        state.error = Some(error);
        state.closed = true;
        self.cond.notify_one();
    }

    fn take_error(&self) -> Option<String> {
        self.state.lock().error.take()
    }

    fn take_latest(&self, stop: &AtomicBool) -> Option<CapturedScreenFrame> {
        let mut state = self.state.lock();
        loop {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(frame) = state.frame.take() {
                return Some(frame);
            }
            if state.closed {
                return None;
            }
            self.cond.wait_for(&mut state, SLOT_WAIT);
        }
    }
}

pub struct ScreenStopper {
    stop: Arc<AtomicBool>,
}

impl ScreenStopper {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ScreenStopper {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_screen(
    identity: String,
    frame_store: Arc<VideoFrameStore>,
    _full_res: Arc<AtomicBool>,
    pick: PickedScreen,
) -> (
    ScreenStopper,
    flume::Receiver<Result<(), String>>,
    flume::Receiver<I420Frame>,
) {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = flume::bounded(1);
    let (frame_tx, frame_rx) = flume::bounded::<I420Frame>(2);

    let thread_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("mezon-screen".into())
        .spawn(move || {
            if !scap::is_supported() {
                let _ = ready_tx.send(Err("screen capture not supported".into()));
                return;
            }
            if !scap::has_permission() && !scap::request_permission() {
                let _ = ready_tx.send(Err("screen recording permission denied".into()));
                return;
            }

            let is_window_share = pick_is_window(&pick);
            let use_portal = pick_uses_portal(&pick);
            let portal_source_types = portal_source_types_for_pick(&pick);
            let capture_target = match scap_target_for_pick(pick) {
                Ok(target) => target,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };

            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            tracing::info!(
                backend = if use_portal { "wayland-portal" } else { "x11" },
                target = ?capture_target,
                "starting screen capture"
            );

            let options = Options {
                fps: CAPTURE_FPS,
                target: capture_target,
                show_cursor: true,
                show_highlight: false,
                excluded_targets: None,
                #[cfg(target_os = "macos")]
                output_type: FrameType::YUVFrameFullRange,
                #[cfg(not(target_os = "macos"))]
                output_type: FrameType::BGRAFrame,
                output_resolution: Resolution::_720p,
                portal_source_types,
                use_portal,
                ..Default::default()
            };

            let slot = Arc::new(LatestFrameSlot::default());
            let pump_slot = slot.clone();
            let pump_stop = thread_stop.clone();
            let pump = std::thread::Builder::new()
                .name("mezon-screen-pump".into())
                .spawn(move || {
                    #[cfg(target_os = "macos")]
                    {
                        let mut restart_attempts = 0u32;
                        loop {
                            if pump_stop.load(Ordering::Relaxed) {
                                break;
                            }
                            let mut capturer = match Capturer::build(options.clone()) {
                                Ok(capturer) => capturer,
                                Err(e) => {
                                    pump_slot.fail(format!("screen capture init failed: {e:#}"));
                                    return;
                                }
                            };
                            capturer.start_capture();
                            let mut errored = false;
                            let mut produced = false;
                            while !pump_stop.load(Ordering::Relaxed) {
                                match capturer.raw().get_next_pixel_buffer() {
                                    Ok(frame) => {
                                        produced = true;
                                        pump_slot.publish(frame);
                                    }
                                    Err(_) => {
                                        errored = true;
                                        break;
                                    }
                                }
                            }
                            if !errored {
                                let _ =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        capturer.stop_capture()
                                    }));
                                break;
                            }
                            drop(capturer);
                            if produced {
                                restart_attempts = 0;
                            }
                            restart_attempts += 1;
                            if restart_attempts > MAX_SCREEN_RESTART_ATTEMPTS {
                                pump_slot.fail(
                                    "screen capture stopped and could not be restarted".into(),
                                );
                                return;
                            }
                            tracing::warn!(
                                attempt = restart_attempts,
                                max_attempts = MAX_SCREEN_RESTART_ATTEMPTS,
                                "screen capture source errored; restarting capture"
                            );
                            std::thread::sleep(SCREEN_RESTART_DELAY);
                        }
                        pump_slot.close();
                    }

                    #[cfg(not(target_os = "macos"))]
                    {
                        let mut capturer = match Capturer::build(options) {
                            Ok(capturer) => capturer,
                            Err(e) => {
                                pump_slot.fail(format!("screen capture init failed: {e:#}"));
                                return;
                            }
                        };
                        capturer.start_capture();
                        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                        while !pump_stop.load(Ordering::Relaxed) {
                            match capturer.get_next_frame_timeout(SLOT_WAIT) {
                                Ok(Some(frame)) => {
                                    if let Some(bgra) = frame_to_bgra(frame)
                                        && !bgra.data.is_empty()
                                    {
                                        pump_slot.publish(bgra);
                                    }
                                }
                                Ok(None) => continue,
                                Err(e) => {
                                    pump_slot.fail(format!("screen capture failed: {e:#}"));
                                    break;
                                }
                            }
                        }
                        #[cfg(target_os = "windows")]
                        while !pump_stop.load(Ordering::Relaxed) {
                            match capturer.get_next_frame() {
                                Ok(frame) => {
                                    if let Some(bgra) = frame_to_bgra(frame)
                                        && !bgra.data.is_empty()
                                    {
                                        pump_slot.publish(bgra);
                                    }
                                }
                                Err(e) => {
                                    pump_slot.fail(format!("screen capture failed: {e:#}"));
                                    break;
                                }
                            }
                        }
                        capturer.stop_capture();
                        pump_slot.close();
                    }
                });
            if let Err(e) = pump {
                let _ = ready_tx.send(Err(format!("screen capture pump failed: {e}")));
                return;
            }

            let key = local_screen_key(&identity);
            let mut src_w = 0u32;
            let mut src_h = 0u32;
            let mut started = false;
            let mut display_buf = Vec::new();

            while let Some(captured) = slot.take_latest(&thread_stop) {
                #[cfg(target_os = "macos")]
                let (full_w, full_h) =
                    (captured.width() as u32 & !1, captured.height() as u32 & !1);
                #[cfg(target_os = "macos")]
                let (width, height) = match captured.content_size() {
                    Some((cw, ch)) => ((cw as u32).min(full_w) & !1, (ch as u32).min(full_h) & !1),
                    None => (full_w, full_h),
                };
                #[cfg(not(target_os = "macos"))]
                let (width, height, row_stride) = (
                    captured.width as u32 & !1,
                    captured.height as u32 & !1,
                    captured.data.len() / captured.height.max(1) as usize,
                );
                if width < 2 || height < 2 {
                    continue;
                }

                #[cfg(not(target_os = "macos"))]
                if started
                    && is_window_share
                    && bgra_frame_is_uniform(
                        &captured.data,
                        width as usize,
                        height as usize,
                        row_stride,
                    )
                {
                    continue;
                }
                #[cfg(target_os = "macos")]
                if started
                    && is_window_share
                    && captured
                        .planes()
                        .first()
                        .is_some_and(|plane| plane_is_uniform(&plane.data()))
                {
                    continue;
                }

                if !started {
                    src_w = width;
                    src_h = height;
                    if ready_tx.send(Ok(())).is_err() {
                        return;
                    }
                    started = true;
                    tracing::info!("screen capture started: {src_w}x{src_h}");
                }

                if width != src_w || height != src_h {
                    src_w = width;
                    src_h = height;
                    tracing::info!("screen capture resized: {src_w}x{src_h}");
                }

                let w = src_w as usize;
                let h = src_h as usize;
                let cw = w.div_ceil(2);
                let ch = h.div_ceil(2);
                let mut y = vec![0u8; w * h];
                let mut u = vec![0u8; cw * ch];
                let mut v = vec![0u8; cw * ch];
                #[cfg(target_os = "macos")]
                {
                    let planes = captured.planes();
                    if planes.len() < 2 {
                        continue;
                    }
                    let yp = planes[0].data();
                    let uv = planes[1].data();
                    nv12_full_to_i420(
                        &yp,
                        &uv,
                        planes[0].bytes_per_row(),
                        planes[1].bytes_per_row(),
                        w,
                        h,
                        &mut y,
                        &mut u,
                        &mut v,
                        w,
                        cw,
                        cw,
                    );
                }
                #[cfg(not(target_os = "macos"))]
                {
                    bgra_to_i420(
                        &captured.data,
                        w,
                        h,
                        row_stride,
                        &mut y,
                        &mut u,
                        &mut v,
                        w,
                        cw,
                        cw,
                    );
                }

                #[cfg(target_os = "macos")]
                {
                    display_buf.resize(w * h * 4, 0);
                    i420_to_bgra_into(&mut display_buf, &y, &u, &v, w, cw, cw, w, h);
                    if let Some(recycled) =
                        frame_store.publish(key, src_w, src_h, std::mem::take(&mut display_buf))
                    {
                        display_buf = recycled;
                    }
                }

                #[cfg(not(target_os = "macos"))]
                {
                    let (pw, ph) = if _full_res.load(Ordering::Relaxed) {
                        (src_w, src_h)
                    } else {
                        scaled_dims(src_w, src_h, PREVIEW_MAX_WIDTH, PREVIEW_MAX_HEIGHT)
                    };
                    display_buf.resize((pw * ph * 4) as usize, 0);
                    downscale_bgra_into(
                        &mut display_buf,
                        &captured.data,
                        w,
                        h,
                        row_stride,
                        pw as usize,
                        ph as usize,
                    );
                    if let Some(recycled) =
                        frame_store.publish(key, pw, ph, std::mem::take(&mut display_buf))
                    {
                        display_buf = recycled;
                    }
                }

                let _ = frame_tx.try_send(I420Frame::tightly_packed(src_w, src_h, y, u, v));
            }

            frame_store.remove(local_screen_key(&identity));
            if !started {
                let msg = slot
                    .take_error()
                    .unwrap_or_else(|| "screen capture produced no frames".into());
                let _ = ready_tx.send(Err(msg));
            }
            tracing::info!("screen capture stopped");
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn screen capture thread: {e}");
    }

    (ScreenStopper { stop }, ready_rx, frame_rx)
}

#[cfg(any(not(target_os = "macos"), test))]
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
        Frame::RGB(frame) => {
            let mut data = Vec::with_capacity(frame.data.len() / 3 * 4);
            for px in frame.data.chunks_exact(3) {
                data.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            Some(BGRAFrame {
                display_time: frame.display_time,
                width: frame.width,
                height: frame.height,
                data,
            })
        }
        Frame::BGR0(frame) => {
            let mut data = Vec::with_capacity(frame.data.len() / 3 * 4);
            for px in frame.data.chunks_exact(3) {
                data.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            Some(BGRAFrame {
                display_time: frame.display_time,
                width: frame.width,
                height: frame.height,
                data,
            })
        }
        Frame::YUVFrame(_) => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn scaled_dims(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let scale = (max_width as f32 / width.max(1) as f32)
        .min(max_height as f32 / height.max(1) as f32)
        .min(1.0);
    let pw = ((width as f32 * scale) as u32).max(1);
    let ph = ((height as f32 * scale) as u32).max(1);
    (pw, ph)
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(target_os = "macos"))]
fn downscale_bgra_into(
    dst: &mut [u8],
    src: &[u8],
    src_width: usize,
    src_height: usize,
    row_stride: usize,
    dst_width: usize,
    dst_height: usize,
) {
    let dst_row_bytes = dst_width * 4;
    if dst_width == src_width && dst_height == src_height {
        for y in 0..dst_height {
            let s = y * row_stride;
            let d = y * dst_row_bytes;
            if s + dst_row_bytes <= src.len() && d + dst_row_bytes <= dst.len() {
                dst[d..d + dst_row_bytes].copy_from_slice(&src[s..s + dst_row_bytes]);
            }
        }
        return;
    }
    let col_offsets: Vec<usize> = (0..dst_width)
        .map(|x| x * src_width / dst_width * 4)
        .collect();
    for y in 0..dst_height {
        let sy = y * src_height / dst_height;
        let s_row = sy * row_stride;
        let d_row = y * dst_row_bytes;
        let Some(dst_row) = dst.get_mut(d_row..d_row + dst_row_bytes) else {
            continue;
        };
        for (d_px, &s_off) in dst_row.chunks_exact_mut(4).zip(&col_offsets) {
            if let Some(s_px) = src.get(s_row + s_off..s_row + s_off + 4) {
                d_px.copy_from_slice(s_px);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn bgra_frame_is_uniform(data: &[u8], width: usize, height: usize, row_stride: usize) -> bool {
    let row_bytes = width * 4;
    if row_bytes == 0 || height == 0 || row_stride < row_bytes || data.len() < row_bytes {
        return true;
    }
    let first = &data[..4];
    for y in 0..height {
        let start = y * row_stride;
        let Some(row) = data.get(start..start + row_bytes) else {
            break;
        };
        for px in row.chunks_exact(4) {
            if px != first {
                return false;
            }
        }
    }
    true
}

#[cfg(target_os = "macos")]
fn plane_is_uniform(data: &[u8]) -> bool {
    match data.split_first() {
        Some((first, rest)) => rest.iter().all(|b| b == first),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use scap::frame::{BGRxFrame, Frame, RGBFrame, RGBxFrame, XBGRFrame};

    use super::frame_to_bgra;

    fn frame_data(frame: Frame) -> Vec<u8> {
        frame_to_bgra(frame).expect("convertible frame").data
    }

    #[test]
    fn converts_bgrx_by_setting_alpha() {
        let data = frame_data(Frame::BGRx(BGRxFrame {
            display_time: 0,
            width: 1,
            height: 1,
            data: vec![1, 2, 3, 0],
        }));
        assert_eq!(data, vec![1, 2, 3, 255]);
    }

    #[test]
    fn converts_rgbx_by_swapping_red_blue() {
        let data = frame_data(Frame::RGBx(RGBxFrame {
            display_time: 0,
            width: 1,
            height: 1,
            data: vec![10, 20, 30, 0],
        }));
        assert_eq!(data, vec![30, 20, 10, 255]);
    }

    #[test]
    fn converts_xbgr_by_dropping_leading_pad() {
        let data = frame_data(Frame::XBGR(XBGRFrame {
            display_time: 0,
            width: 1,
            height: 1,
            data: vec![9, 1, 2, 3],
        }));
        assert_eq!(data, vec![1, 2, 3, 255]);
    }

    #[test]
    fn converts_rgb_to_bgra() {
        let data = frame_data(Frame::RGB(RGBFrame {
            display_time: 0,
            width: 2,
            height: 1,
            data: vec![10, 20, 30, 40, 50, 60],
        }));
        assert_eq!(data, vec![30, 20, 10, 255, 60, 50, 40, 255]);
    }
}
