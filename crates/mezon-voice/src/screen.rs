use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use livekit::track::LocalVideoTrack;
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use parking_lot::{Condvar, Mutex};
use scap::capturer::{Capturer, Options, Resolution};
use scap::frame::{BGRAFrame, Frame, FrameType};

use crate::screen_picker::{PickedScreen, scap_target_for_pick};
use crate::video::{VideoFrameStore, bgra_to_i420, local_screen_key};

const CAPTURE_FPS: u32 = 30;
const PREVIEW_MAX_WIDTH: u32 = 1280;
const PREVIEW_MAX_HEIGHT: u32 = 800;
const SLOT_WAIT: Duration = Duration::from_millis(250);

#[derive(Default)]
struct SlotState {
    frame: Option<BGRAFrame>,
    closed: bool,
    error: Option<String>,
}

#[derive(Default)]
struct LatestFrameSlot {
    state: Mutex<SlotState>,
    cond: Condvar,
}

impl LatestFrameSlot {
    fn publish(&self, frame: BGRAFrame) {
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

    fn take_latest(&self, stop: &AtomicBool) -> Option<BGRAFrame> {
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
    full_res: Arc<AtomicBool>,
    pick: PickedScreen,
) -> (
    ScreenStopper,
    flume::Receiver<Result<LocalVideoTrack, String>>,
) {
    let stop = Arc::new(AtomicBool::new(false));
    let (track_tx, track_rx) = flume::bounded(1);

    let thread_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("mezon-screen".into())
        .spawn(move || {
            let _guard = crate::runtime::handle().enter();

            if !scap::is_supported() {
                let _ = track_tx.send(Err("screen capture not supported".into()));
                return;
            }
            if !scap::has_permission() && !scap::request_permission() {
                let _ = track_tx.send(Err("screen recording permission denied".into()));
                return;
            }

            let capture_target = match scap_target_for_pick(pick) {
                Ok(target) => target,
                Err(e) => {
                    let _ = track_tx.send(Err(e));
                    return;
                }
            };

            let options = Options {
                fps: CAPTURE_FPS,
                target: capture_target,
                show_cursor: true,
                show_highlight: false,
                excluded_targets: None,
                output_type: FrameType::BGRAFrame,
                output_resolution: Resolution::_1440p,
                ..Default::default()
            };

            let slot = Arc::new(LatestFrameSlot::default());
            let pump_slot = slot.clone();
            let pump_stop = thread_stop.clone();
            let pump = std::thread::Builder::new()
                .name("mezon-screen-pump".into())
                .spawn(move || {
                    let mut capturer = match Capturer::build(options) {
                        Ok(capturer) => capturer,
                        Err(e) => {
                            pump_slot.fail(format!("screen capture init failed: {e}"));
                            return;
                        }
                    };
                    capturer.start_capture();
                    while !pump_stop.load(Ordering::Relaxed) {
                        match capturer.get_next_frame() {
                            Ok(frame) => {
                                if let Some(bgra) = frame_to_bgra(frame)
                                    && !bgra.data.is_empty()
                                {
                                    pump_slot.publish(bgra);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    capturer.stop_capture();
                    pump_slot.close();
                });
            if let Err(e) = pump {
                let _ = track_tx.send(Err(format!("screen capture pump failed: {e}")));
                return;
            }

            let key = local_screen_key(&identity);
            let started = Instant::now();
            let mut source: Option<NativeVideoSource> = None;
            let mut src_w = 0u32;
            let mut src_h = 0u32;
            let mut sent_track = false;
            let mut display_buf = Vec::new();

            while let Some(bgra) = slot.take_latest(&thread_stop) {
                let width = (bgra.width as u32 & !1).max(2);
                let height = (bgra.height as u32 & !1).max(2);
                if width == 0 || height == 0 || bgra.data.is_empty() {
                    continue;
                }

                if source.is_none() {
                    src_w = width;
                    src_h = height;
                    let new_source = NativeVideoSource::new(
                        VideoResolution {
                            width: src_w,
                            height: src_h,
                        },
                        true,
                    );
                    let track = LocalVideoTrack::create_video_track(
                        "screen",
                        RtcVideoSource::Native(new_source.clone()),
                    );
                    source = Some(new_source);
                    if track_tx.send(Ok(track)).is_err() {
                        return;
                    }
                    sent_track = true;
                    tracing::info!("screen capture started: {src_w}x{src_h}");
                }

                if width != src_w || height != src_h {
                    tracing::debug!(
                        "screen capture resolution changed: {src_w}x{src_h} -> {width}x{height}"
                    );
                    src_w = width;
                    src_h = height;
                }

                let row_stride = bgra.data.len() / bgra.height.max(1) as usize;
                let mut i420 = I420Buffer::new(src_w, src_h);
                {
                    let (sy, su, sv) = i420.strides();
                    let (dy, du, dv) = i420.data_mut();
                    bgra_to_i420(
                        &bgra.data,
                        src_w as usize,
                        src_h as usize,
                        row_stride,
                        dy,
                        du,
                        dv,
                        sy as usize,
                        su as usize,
                        sv as usize,
                    );
                }
                if let Some(source) = &source {
                    let frame = VideoFrame {
                        rotation: VideoRotation::VideoRotation0,
                        timestamp_us: started.elapsed().as_micros() as i64,
                        frame_metadata: None,
                        buffer: i420,
                    };
                    source.capture_frame(&frame);
                }

                let (pw, ph) = if full_res.load(Ordering::Relaxed) {
                    (src_w, src_h)
                } else {
                    scaled_dims(src_w, src_h, PREVIEW_MAX_WIDTH, PREVIEW_MAX_HEIGHT)
                };
                display_buf.resize((pw * ph * 4) as usize, 0);
                downscale_bgra_into(
                    &mut display_buf,
                    &bgra.data,
                    src_w as usize,
                    src_h as usize,
                    row_stride,
                    pw as usize,
                    ph as usize,
                );
                frame_store.publish(key, pw, ph, std::mem::take(&mut display_buf));
            }

            frame_store.remove(local_screen_key(&identity));
            if !sent_track {
                let msg = slot
                    .take_error()
                    .unwrap_or_else(|| "screen capture produced no frames".into());
                let _ = track_tx.send(Err(msg));
            }
            tracing::info!("screen capture stopped");
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn screen capture thread: {e}");
    }

    (ScreenStopper { stop }, track_rx)
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

fn scaled_dims(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let scale = (max_width as f32 / width.max(1) as f32)
        .min(max_height as f32 / height.max(1) as f32)
        .min(1.0);
    let pw = ((width as f32 * scale) as u32).max(1);
    let ph = ((height as f32 * scale) as u32).max(1);
    (pw, ph)
}

#[allow(clippy::too_many_arguments)]
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
    for y in 0..dst_height {
        let sy = y * src_height / dst_height;
        let s_row = sy * row_stride;
        let d_row = y * dst_row_bytes;
        for x in 0..dst_width {
            let sx = x * src_width / dst_width;
            let s = s_row + sx * 4;
            let d = d_row + x * 4;
            if s + 4 <= src.len() && d + 4 <= dst.len() {
                dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
            }
        }
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
