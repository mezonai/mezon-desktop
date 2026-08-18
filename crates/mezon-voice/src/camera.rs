use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use livekit::track::LocalVideoTrack;
use livekit::webrtc::prelude::VideoBuffer;
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use nokhwa::Camera;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use nokhwa::{native_api_backend, query};

use crate::video::{VideoFrameStore, local_camera_key, rgb_to_i420, yuyv422_to_i420};

const TARGET_WIDTH: u32 = 640;
const TARGET_HEIGHT: u32 = 480;
const TARGET_FPS: u32 = 30;
const MAX_CAMERA_WIDTH: u32 = 1280;
const MAX_CAMERA_HEIGHT: u32 = 720;

#[derive(Debug, Clone)]
pub struct CameraDeviceInfo {
    pub id: String,
    pub name: String,
}

pub fn enumerate_cameras() -> Vec<CameraDeviceInfo> {
    let backend = native_api_backend().unwrap_or(ApiBackend::AVFoundation);
    match query(backend) {
        Ok(devices) => devices
            .into_iter()
            .map(|device| CameraDeviceInfo {
                id: device.index().as_string(),
                name: device.human_name(),
            })
            .collect(),
        Err(e) => {
            tracing::warn!("camera enumeration failed: {e}");
            Vec::new()
        }
    }
}

pub struct CameraController {
    stop: Arc<AtomicBool>,
    switch_tx: flume::Sender<Option<String>>,
}

impl CameraController {
    pub fn switch(&self, device_id: Option<String>) {
        let _ = self.switch_tx.send(device_id);
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for CameraController {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_camera(
    identity: String,
    frame_store: Arc<VideoFrameStore>,
    device_id: Option<String>,
) -> (
    CameraController,
    flume::Receiver<Result<LocalVideoTrack, String>>,
) {
    let stop = Arc::new(AtomicBool::new(false));
    let (switch_tx, switch_rx) = flume::unbounded::<Option<String>>();
    let (track_tx, track_rx) = flume::bounded(1);

    let thread_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("mezon-camera".into())
        .spawn(move || {
            let _guard = crate::runtime::handle().enter();

            if !request_macos_permission() {
                let _ = track_tx.send(Err("camera permission denied".into()));
                return;
            }

            let current_device = device_id;
            let camera = match open_camera(current_device.as_deref()) {
                Ok(camera) => camera,
                Err(e) => {
                    let _ = track_tx.send(Err(e));
                    return;
                }
            };

            let resolution = camera.resolution();
            let (source_width, source_height) =
                fit_dimensions(resolution.width(), resolution.height());
            let source = NativeVideoSource::new(
                VideoResolution {
                    width: source_width,
                    height: source_height,
                },
                false,
            );
            let track = LocalVideoTrack::create_video_track(
                "camera",
                RtcVideoSource::Native(source.clone()),
            );
            if track_tx.send(Ok(track)).is_err() {
                return;
            }

            capture_loop(
                camera,
                source,
                &frame_store,
                &identity,
                current_device,
                &thread_stop,
                &switch_rx,
            );
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn camera capture thread: {e}");
    }

    (CameraController { stop, switch_tx }, track_rx)
}

pub fn start_camera_into(
    identity: String,
    frame_store: Arc<VideoFrameStore>,
    device_id: Option<String>,
    source: NativeVideoSource,
    err_tx: flume::Sender<String>,
) -> CameraController {
    let stop = Arc::new(AtomicBool::new(false));
    let (switch_tx, switch_rx) = flume::unbounded::<Option<String>>();

    let thread_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("mezon-camera".into())
        .spawn(move || {
            let _guard = crate::runtime::handle().enter();

            if !request_macos_permission() {
                tracing::warn!("camera permission denied");
                let _ = err_tx.send("camera permission denied".into());
                return;
            }

            let current_device = device_id;
            let camera = match open_camera(current_device.as_deref()) {
                Ok(camera) => camera,
                Err(e) => {
                    tracing::warn!("camera start failed: {e}");
                    let _ = err_tx.send(e);
                    return;
                }
            };

            capture_loop(
                camera,
                source,
                &frame_store,
                &identity,
                current_device,
                &thread_stop,
                &switch_rx,
            );
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn camera capture thread: {e}");
    }

    CameraController { stop, switch_tx }
}

fn capture_loop(
    mut camera: Camera,
    source: NativeVideoSource,
    frame_store: &Arc<VideoFrameStore>,
    identity: &str,
    mut current_device: Option<String>,
    thread_stop: &AtomicBool,
    switch_rx: &flume::Receiver<Option<String>>,
) {
    let key = local_camera_key(identity);
    let started = Instant::now();
    let resolution = camera.resolution();
    tracing::info!(
        "camera capture started: {}x{}",
        resolution.width(),
        resolution.height()
    );

    let mut preview = Vec::new();
    let frame_interval = Duration::from_secs_f64(1.0 / TARGET_FPS as f64);
    let mut last_capture: Option<Instant> = None;

    'outer: loop {
        let switch_to = 'capture: loop {
            if thread_stop.load(Ordering::Relaxed) {
                break 'outer;
            }
            if let Ok(device) = switch_rx.try_recv() {
                let mut latest = device;
                while let Ok(next) = switch_rx.try_recv() {
                    latest = next;
                }
                if latest != current_device {
                    break 'capture latest;
                }
            }
            let buffer = match camera.frame() {
                Ok(buffer) => buffer,
                Err(e) => {
                    tracing::warn!("camera frame error: {e}");
                    continue;
                }
            };
            if let Some(last) = last_capture
                && last.elapsed() < frame_interval
            {
                continue;
            }
            last_capture = Some(Instant::now());
            push_camera_frame(
                &buffer,
                &source,
                frame_store,
                key,
                started.elapsed().as_micros() as i64,
                &mut preview,
            );
        };

        drop(camera);
        camera = match open_camera(switch_to.as_deref()) {
            Ok(camera) => {
                current_device = switch_to;
                let res = camera.resolution();
                tracing::info!("camera switched: {}x{}", res.width(), res.height());
                camera
            }
            Err(e) => {
                tracing::warn!("camera switch failed: {e}; reverting to previous device");
                match open_camera(current_device.as_deref()) {
                    Ok(camera) => camera,
                    Err(e) => {
                        tracing::error!("camera revert failed: {e}");
                        break 'outer;
                    }
                }
            }
        };
        last_capture = None;
    }

    frame_store.remove(key);
    tracing::info!("camera capture stopped");
}

fn push_camera_frame(
    buffer: &nokhwa::Buffer,
    source: &NativeVideoSource,
    frame_store: &VideoFrameStore,
    key: u64,
    timestamp_us: i64,
    preview: &mut Vec<u8>,
) {
    let dw = (buffer.resolution().width() & !1).max(2);
    let dh = (buffer.resolution().height() & !1).max(2);

    let mut i420 = I420Buffer::new(dw, dh);
    {
        let (sy, su, sv) = i420.strides();
        let (dy, du, dv) = i420.data_mut();
        match buffer.source_frame_format() {
            FrameFormat::YUYV => yuyv422_to_i420(
                buffer.buffer(),
                dw as usize,
                dh as usize,
                dy,
                du,
                dv,
                sy as usize,
                su as usize,
                sv as usize,
            ),
            _ => {
                let decoded = match buffer.decode_image::<RgbFormat>() {
                    Ok(image) => image,
                    Err(e) => {
                        tracing::warn!("camera decode error: {e}");
                        return;
                    }
                };
                let rw = (decoded.width().min(dw) & !1).max(2);
                let rh = (decoded.height().min(dh) & !1).max(2);
                rgb_to_i420(
                    decoded.as_raw(),
                    rw as usize,
                    rh as usize,
                    dy,
                    du,
                    dv,
                    sy as usize,
                    su as usize,
                    sv as usize,
                );
            }
        }
    }

    let (tw, th) = fit_dimensions(dw, dh);
    let i420 = if (tw, th) != (dw, dh) {
        i420.scale(tw as i32, th as i32)
    } else {
        i420
    };
    let i420 = flip_i420_horizontal(&i420);
    let fw = i420.width();
    let fh = i420.height();

    preview.resize((fw * fh * 4) as usize, 0);
    {
        let (sy, su, sv) = i420.strides();
        let (y, u, v) = i420.data();
        crate::video::i420_to_bgra_into(
            preview,
            y,
            u,
            v,
            sy as usize,
            su as usize,
            sv as usize,
            fw as usize,
            fh as usize,
        );
    }

    let frame = VideoFrame {
        rotation: VideoRotation::VideoRotation0,
        timestamp_us,
        frame_metadata: None,
        buffer: i420,
    };
    source.capture_frame(&frame);
    if let Some(recycled) = frame_store.publish(key, fw, fh, std::mem::take(preview)) {
        *preview = recycled;
    }
}

fn flip_i420_horizontal(src: &I420Buffer) -> I420Buffer {
    let width = src.width();
    let height = src.height();
    let mut dst = I420Buffer::new(width, height);

    let (ssy, ssu, ssv) = src.strides();
    let (src_y, src_u, src_v) = src.data();
    let (dsy, dsu, dsv) = dst.strides();
    let (dst_y, dst_u, dst_v) = dst.data_mut();

    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);

    flip_plane(src_y, ssy as usize, dst_y, dsy as usize, w, h);
    flip_plane(src_u, ssu as usize, dst_u, dsu as usize, cw, ch);
    flip_plane(src_v, ssv as usize, dst_v, dsv as usize, cw, ch);

    dst
}

fn flip_plane(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    width: usize,
    height: usize,
) {
    let rows = src
        .chunks(src_stride)
        .zip(dst.chunks_mut(dst_stride))
        .take(height);
    for (s, d) in rows {
        for (out, inp) in d[..width].iter_mut().zip(s[..width].iter().rev()) {
            *out = *inp;
        }
    }
}

fn fit_dimensions(width: u32, height: u32) -> (u32, u32) {
    let width = (width & !1).max(2);
    let height = (height & !1).max(2);
    if width <= MAX_CAMERA_WIDTH && height <= MAX_CAMERA_HEIGHT {
        return (width, height);
    }
    let scale =
        (MAX_CAMERA_WIDTH as f64 / width as f64).min(MAX_CAMERA_HEIGHT as f64 / height as f64);
    let scaled_width = (((width as f64 * scale).floor() as u32) & !1).max(2);
    let scaled_height = (((height as f64 * scale).floor() as u32) & !1).max(2);
    (scaled_width, scaled_height)
}

fn open_camera(device_id: Option<&str>) -> Result<Camera, String> {
    let indices = camera_indices_preferring(device_id);
    let target = Resolution::new(TARGET_WIDTH, TARGET_HEIGHT);
    let attempts: [RequestedFormat<'static>; 5] = [
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
            target,
            FrameFormat::YUYV,
            TARGET_FPS,
        ))),
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
            target,
            FrameFormat::MJPEG,
            TARGET_FPS,
        ))),
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestResolution(target)),
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestFrameRate(TARGET_FPS)),
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::None),
    ];

    let mut last_err = String::from("no camera formats attempted");
    for index in &indices {
        for requested in &attempts {
            match try_open_camera(index, *requested) {
                Ok(camera) => {
                    let format = camera.camera_format();
                    let resolution = camera.resolution();
                    tracing::info!(
                        "camera opened: {}x{} {:?} @ {}fps",
                        resolution.width(),
                        resolution.height(),
                        format.format(),
                        format.frame_rate(),
                    );
                    return Ok(camera);
                }
                Err(e) => last_err = e,
            }
        }
    }

    Err(last_err)
}

fn camera_indices() -> Vec<CameraIndex> {
    let backend = native_api_backend().unwrap_or(ApiBackend::AVFoundation);
    match query(backend) {
        Ok(devices) if !devices.is_empty() => {
            devices.into_iter().map(|d| d.index().clone()).collect()
        }
        Ok(_) | Err(_) => vec![CameraIndex::Index(0)],
    }
}

fn parse_camera_index(id: &str) -> CameraIndex {
    match id.parse::<u32>() {
        Ok(index) => CameraIndex::Index(index),
        Err(_) => CameraIndex::String(id.to_string()),
    }
}

fn camera_indices_preferring(device_id: Option<&str>) -> Vec<CameraIndex> {
    let Some(id) = device_id.filter(|id| !id.is_empty()) else {
        return camera_indices();
    };
    let chosen = parse_camera_index(id);
    let mut indices = camera_indices();
    indices.retain(|index| index != &chosen);
    indices.insert(0, chosen);
    indices
}

fn try_open_camera(index: &CameraIndex, requested: RequestedFormat<'_>) -> Result<Camera, String> {
    let mut camera =
        Camera::new(index.clone(), requested).map_err(|e| format!("open camera: {e}"))?;
    camera
        .open_stream()
        .map_err(|e| format!("open camera stream: {e}"))?;
    Ok(camera)
}

#[cfg(target_os = "macos")]
fn request_macos_permission() -> bool {
    use std::time::Duration;

    if nokhwa::nokhwa_check() {
        return true;
    }

    let (tx, rx) = flume::bounded(1);
    nokhwa::nokhwa_initialize(move |granted| {
        let _ = tx.send(granted);
    });
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(true) => true,
        Ok(false) => {
            tracing::warn!("camera permission denied");
            false
        }
        Err(_) => {
            tracing::warn!("camera permission request timed out");
            false
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn request_macos_permission() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn camera_authorization_status() -> i64 {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    unsafe {
        let Some(cls) = Class::get("AVCaptureDevice") else {
            return 3;
        };
        let media_type: id = NSString::alloc(nil).init_str("vide");
        msg_send![cls, authorizationStatusForMediaType: media_type]
    }
}

#[cfg(target_os = "macos")]
pub fn camera_denied() -> bool {
    matches!(camera_authorization_status(), 1 | 2)
}

#[cfg(not(target_os = "macos"))]
pub fn camera_denied() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::parse_camera_index;
    use nokhwa::utils::CameraIndex;

    #[test]
    fn parse_camera_index_numeric() {
        assert_eq!(parse_camera_index("3"), CameraIndex::Index(3));
    }

    #[test]
    fn parse_camera_index_path() {
        assert_eq!(
            parse_camera_index("/dev/video1"),
            CameraIndex::String("/dev/video1".to_string())
        );
    }
}
