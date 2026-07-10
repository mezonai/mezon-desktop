use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use livekit::track::LocalVideoTrack;
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

pub struct CameraStopper {
    stop: Arc<AtomicBool>,
}

impl CameraStopper {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for CameraStopper {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_camera(
    identity: String,
    frame_store: Arc<VideoFrameStore>,
) -> (
    CameraStopper,
    flume::Receiver<Result<LocalVideoTrack, String>>,
) {
    let stop = Arc::new(AtomicBool::new(false));
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

            let mut camera = match open_camera() {
                Ok(camera) => camera,
                Err(e) => {
                    let _ = track_tx.send(Err(e));
                    return;
                }
            };

            let resolution = camera.resolution();
            let width = (resolution.width() & !1).max(2);
            let height = (resolution.height() & !1).max(2);

            let source = NativeVideoSource::new(VideoResolution { width, height }, false);
            let track = LocalVideoTrack::create_video_track(
                "camera",
                RtcVideoSource::Native(source.clone()),
            );
            if track_tx.send(Ok(track)).is_err() {
                return;
            }

            let key = local_camera_key(&identity);
            let started = Instant::now();
            tracing::info!("camera capture started: {width}x{height}");

            let mut preview = Vec::with_capacity((width * height * 4) as usize);

            let frame_interval = Duration::from_secs_f64(1.0 / TARGET_FPS as f64);
            let mut last_capture: Option<Instant> = None;

            while !thread_stop.load(Ordering::Relaxed) {
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
                                    continue;
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

                preview.resize((dw * dh * 4) as usize, 0);
                {
                    let (sy, su, sv) = i420.strides();
                    let (y, u, v) = i420.data();
                    crate::video::i420_to_bgra_into(
                        &mut preview,
                        y,
                        u,
                        v,
                        sy as usize,
                        su as usize,
                        sv as usize,
                        dw as usize,
                        dh as usize,
                    );
                }

                let frame = VideoFrame {
                    rotation: VideoRotation::VideoRotation0,
                    timestamp_us: started.elapsed().as_micros() as i64,
                    frame_metadata: None,
                    buffer: i420,
                };
                source.capture_frame(&frame);
                frame_store.publish(key, dw, dh, std::mem::take(&mut preview));
            }

            frame_store.remove(local_camera_key(&identity));
            tracing::info!("camera capture stopped");
        });
    if let Err(e) = spawned {
        tracing::error!("failed to spawn camera capture thread: {e}");
    }

    (CameraStopper { stop }, track_rx)
}

fn open_camera() -> Result<Camera, String> {
    let indices = camera_indices();
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
                    tracing::info!(
                        "camera opened: {}x{} {:?} @ {}fps",
                        camera.resolution().width(),
                        camera.resolution().height(),
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
