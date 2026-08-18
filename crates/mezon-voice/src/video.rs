use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio::sync::watch;

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct VideoSurface(core_video::pixel_buffer::CVPixelBuffer);

#[cfg(target_os = "macos")]
unsafe impl Send for VideoSurface {}
#[cfg(target_os = "macos")]
unsafe impl Sync for VideoSurface {}

#[cfg(target_os = "macos")]
impl VideoSurface {
    #[allow(missing_docs)]
    pub fn into_inner(self) -> core_video::pixel_buffer::CVPixelBuffer {
        self.0
    }
}

#[derive(Clone)]
pub struct VideoFrameData {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    #[cfg(target_os = "macos")]
    pub surface: Option<VideoSurface>,
    pub seq: u64,
}

pub struct VideoFrameStore {
    state: Mutex<VideoFrameState>,
    seq: AtomicU64,
    recording: AtomicBool,
    frame_tx: watch::Sender<u64>,
}

impl Default for VideoFrameStore {
    fn default() -> Self {
        Self {
            state: Mutex::new(VideoFrameState::default()),
            seq: AtomicU64::new(0),
            recording: AtomicBool::new(false),
            frame_tx: watch::channel(0).0,
        }
    }
}

#[derive(Default)]
struct VideoFrameState {
    frames: HashMap<u64, Arc<VideoFrameData>>,
    recycled: HashMap<u64, Vec<Vec<u8>>>,
    active: HashSet<u64>,
    // The render path consumes frames with take_new, so a second reader would
    // race the UI for every frame and mostly lose. While a recording is running
    // each published frame is also kept here, where reads never remove it.
    recorded: HashMap<u64, Arc<VideoFrameData>>,
}

impl VideoFrameStore {
    pub fn publish(&self, key: u64, width: u32, height: u32, bgra: Vec<u8>) -> Option<Vec<u8>> {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let required = bgra.len();
        let frame = Arc::new(VideoFrameData {
            width,
            height,
            bgra,
            #[cfg(target_os = "macos")]
            surface: None,
            seq,
        });
        let recycled = {
            let mut state = self.state.lock();
            state.active.insert(key);
            if self.recording.load(Ordering::Relaxed) {
                state.recorded.insert(key, frame.clone());
            } else if !state.recorded.is_empty() {
                state.recorded.remove(&key);
            }
            let previous = state.frames.insert(key, frame);
            if let Some(buffer) = previous
                .and_then(|frame| Arc::try_unwrap(frame).ok())
                .map(|frame| frame.bgra)
            {
                Some(buffer)
            } else {
                take_recycled(&mut state, key, required)
            }
        };
        self.frame_tx.send_replace(seq.wrapping_add(1));
        recycled
    }

    pub fn get(&self, key: u64) -> Option<Arc<VideoFrameData>> {
        self.state.lock().frames.get(&key).cloned()
    }

    /// Non-destructive read for the recorder, which must not race the renderer.
    pub fn recorded(&self, key: u64) -> Option<Arc<VideoFrameData>> {
        self.state.lock().recorded.get(&key).cloned()
    }

    pub fn set_recording(&self, recording: bool) {
        self.recording.store(recording, Ordering::Relaxed);
        if !recording {
            self.state.lock().recorded.clear();
        }
    }

    pub fn publish_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    pub fn frame_watch(&self) -> watch::Receiver<u64> {
        self.frame_tx.subscribe()
    }

    pub fn take_new(&self, key: u64, since: Option<u64>) -> Option<VideoFrameData> {
        let mut state = self.state.lock();
        let is_new = match state.frames.get(&key) {
            Some(frame) => Some(frame.seq) != since,
            None => false,
        };
        if !is_new {
            return None;
        }
        let frame = state.frames.remove(&key)?;
        Some(Arc::try_unwrap(frame).unwrap_or_else(|frame| (*frame).clone()))
    }

    pub fn recycle(&self, key: u64, mut buffer: Vec<u8>) {
        let mut state = self.state.lock();
        if !state.active.contains(&key) {
            return;
        }
        buffer.clear();
        let buffers = state.recycled.entry(key).or_default();
        if buffers.len() < 3 {
            buffers.push(buffer);
        }
    }

    pub fn remove(&self, key: u64) {
        let mut state = self.state.lock();
        state.frames.remove(&key);
        state.recorded.remove(&key);
        state.recycled.remove(&key);
        state.active.remove(&key);
    }
}

fn take_recycled(state: &mut VideoFrameState, key: u64, required: usize) -> Option<Vec<u8>> {
    let buffers = state.recycled.get_mut(&key)?;
    let index = buffers
        .iter()
        .position(|buffer| buffer.capacity() >= required && buffer.capacity() <= required * 2)?;
    let buffer = buffers.swap_remove(index);
    if buffers.is_empty() {
        state.recycled.remove(&key);
    }
    Some(buffer)
}

pub fn track_frame_key(identity: &str, track_sid: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    identity.hash(&mut hasher);
    track_sid.hash(&mut hasher);
    hasher.finish()
}

pub fn local_camera_key(identity: &str) -> u64 {
    track_frame_key(identity, "__local_camera__")
}

pub fn local_screen_key(identity: &str) -> u64 {
    track_frame_key(identity, "__local_screen__")
}

#[inline]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[inline]
fn rgb_to_y(r: i32, g: i32, b: i32) -> u8 {
    clamp_u8(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16)
}
#[inline]
fn rgb_to_u(r: i32, g: i32, b: i32) -> u8 {
    clamp_u8(((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128)
}
#[inline]
fn rgb_to_v(r: i32, g: i32, b: i32) -> u8 {
    clamp_u8(((112 * r - 94 * g - 18 * b + 128) >> 8) + 128)
}

#[allow(clippy::too_many_arguments)]
fn pack_to_i420(
    src: &[u8],
    width: usize,
    height: usize,
    pixel_stride: usize,
    src_row_stride: usize,
    ro: usize,
    go: usize,
    bo: usize,
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    stride_y: usize,
    stride_u: usize,
    stride_v: usize,
) {
    for y in 0..height {
        let row = y * src_row_stride;
        let y_out = y * stride_y;
        for x in 0..width {
            let p = row + x * pixel_stride;
            if p + bo.max(ro).max(go) >= src.len() {
                continue;
            }
            let r = src[p + ro] as i32;
            let g = src[p + go] as i32;
            let b = src[p + bo] as i32;
            y_plane[y_out + x] = rgb_to_y(r, g, b);
        }
    }

    let cw = width / 2;
    let ch = height / 2;
    for cy in 0..ch {
        let src_y = cy * 2;
        let u_out = cy * stride_u;
        let v_out = cy * stride_v;
        for cx in 0..cw {
            let src_x = cx * 2;
            let p = src_y * src_row_stride + src_x * pixel_stride;
            if p + bo.max(ro).max(go) >= src.len() {
                continue;
            }
            let r = src[p + ro] as i32;
            let g = src[p + go] as i32;
            let b = src[p + bo] as i32;
            u_plane[u_out + cx] = rgb_to_u(r, g, b);
            v_plane[v_out + cx] = rgb_to_v(r, g, b);
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(target_os = "macos"))]
pub fn bgra_to_i420(
    bgra: &[u8],
    width: usize,
    height: usize,
    src_row_stride: usize,
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    stride_y: usize,
    stride_u: usize,
    stride_v: usize,
) {
    let mut planar = yuv::YuvPlanarImageMut {
        y_plane: yuv::BufferStoreMut::Borrowed(y_plane),
        y_stride: stride_y as u32,
        u_plane: yuv::BufferStoreMut::Borrowed(u_plane),
        u_stride: stride_u as u32,
        v_plane: yuv::BufferStoreMut::Borrowed(v_plane),
        v_stride: stride_v as u32,
        width: width as u32,
        height: height as u32,
    };
    let _ = yuv::bgra_to_yuv420(
        &mut planar,
        bgra,
        src_row_stride as u32,
        yuv::YuvRange::Limited,
        yuv::YuvStandardMatrix::Bt601,
        yuv::YuvConversionMode::Balanced,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn i420_to_bgra_into(
    out: &mut [u8],
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    stride_y: usize,
    stride_u: usize,
    stride_v: usize,
    width: usize,
    height: usize,
) {
    let needed = width * height * 4;
    if out.len() < needed {
        return;
    }

    let planar = yuv::YuvPlanarImage {
        y_plane,
        y_stride: stride_y as u32,
        u_plane,
        u_stride: stride_u as u32,
        v_plane,
        v_stride: stride_v as u32,
        width: width as u32,
        height: height as u32,
    };
    if yuv::yuv420_to_bgra(
        &planar,
        &mut out[..needed],
        (width * 4) as u32,
        yuv::YuvRange::Limited,
        yuv::YuvStandardMatrix::Bt601,
    )
    .is_ok()
    {
        return;
    }

    i420_to_bgra_scalar(
        out, y_plane, u_plane, v_plane, stride_y, stride_u, stride_v, width, height,
    );
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn nv12_full_to_i420(
    src_y: &[u8],
    src_uv: &[u8],
    src_stride_y: usize,
    src_stride_uv: usize,
    width: usize,
    height: usize,
    dst_y: &mut [u8],
    dst_u: &mut [u8],
    dst_v: &mut [u8],
    dst_stride_y: usize,
    dst_stride_u: usize,
    dst_stride_v: usize,
) {
    for row in 0..height {
        let src_start = row * src_stride_y;
        let dst_start = row * dst_stride_y;
        if src_start + width > src_y.len() || dst_start + width > dst_y.len() {
            break;
        }
        for (dst, src) in dst_y[dst_start..dst_start + width]
            .iter_mut()
            .zip(&src_y[src_start..src_start + width])
        {
            *dst = 16 + ((*src as u16 * 219 + 127) / 255) as u8;
        }
    }

    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    for row in 0..chroma_height {
        let src_start = row * src_stride_uv;
        let u_start = row * dst_stride_u;
        let v_start = row * dst_stride_v;
        if src_start + chroma_width * 2 > src_uv.len()
            || u_start + chroma_width > dst_u.len()
            || v_start + chroma_width > dst_v.len()
        {
            break;
        }
        for column in 0..chroma_width {
            let src = src_start + column * 2;
            dst_u[u_start + column] = 16 + ((src_uv[src] as u16 * 224 + 127) / 255) as u8;
            dst_v[v_start + column] = 16 + ((src_uv[src + 1] as u16 * 224 + 127) / 255) as u8;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn i420_to_bgra_scalar(
    out: &mut [u8],
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    stride_y: usize,
    stride_u: usize,
    stride_v: usize,
    width: usize,
    height: usize,
) {
    let cw = width / 2;
    for y in 0..height {
        let c_row = y / 2;
        let y_start = y * stride_y;
        let u_start = c_row * stride_u;
        let v_start = c_row * stride_v;
        if y_start + width > y_plane.len()
            || u_start + cw > u_plane.len()
            || v_start + cw > v_plane.len()
        {
            continue;
        }
        let y_row = &y_plane[y_start..y_start + width];
        let u_row = &u_plane[u_start..u_start + cw];
        let v_row = &v_plane[v_start..v_start + cw];
        let out_start = y * width * 4;
        let out_row = &mut out[out_start..out_start + cw * 8];

        for (((out_px, y_px), &u), &v) in out_row
            .chunks_exact_mut(8)
            .zip(y_row.chunks_exact(2))
            .zip(u_row)
            .zip(v_row)
        {
            let d = u as i32 - 128;
            let e = v as i32 - 128;
            let r_off = 409 * e + 128;
            let g_off = -100 * d - 208 * e + 128;
            let b_off = 516 * d + 128;

            let c0 = 298 * (y_px[0] as i32 - 16);
            out_px[0] = clamp_u8((c0 + b_off) >> 8);
            out_px[1] = clamp_u8((c0 + g_off) >> 8);
            out_px[2] = clamp_u8((c0 + r_off) >> 8);
            out_px[3] = 255;

            let c1 = 298 * (y_px[1] as i32 - 16);
            out_px[4] = clamp_u8((c1 + b_off) >> 8);
            out_px[5] = clamp_u8((c1 + g_off) >> 8);
            out_px[6] = clamp_u8((c1 + r_off) >> 8);
            out_px[7] = 255;
        }

        if width % 2 == 1 && u_start + cw < u_plane.len() && v_start + cw < v_plane.len() {
            let d = u_plane[u_start + cw] as i32 - 128;
            let e = v_plane[v_start + cw] as i32 - 128;
            let r_off = 409 * e + 128;
            let g_off = -100 * d - 208 * e + 128;
            let b_off = 516 * d + 128;
            let c = 298 * (y_row[width - 1] as i32 - 16);
            let px = out_start + (width - 1) * 4;
            out[px] = clamp_u8((c + b_off) >> 8);
            out[px + 1] = clamp_u8((c + g_off) >> 8);
            out[px + 2] = clamp_u8((c + r_off) >> 8);
            out[px + 3] = 255;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rgb_to_i420(
    rgb: &[u8],
    width: usize,
    height: usize,
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    stride_y: usize,
    stride_u: usize,
    stride_v: usize,
) {
    pack_to_i420(
        rgb,
        width,
        height,
        3,
        width * 3,
        0,
        1,
        2,
        y_plane,
        u_plane,
        v_plane,
        stride_y,
        stride_u,
        stride_v,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn yuyv422_to_i420(
    yuyv: &[u8],
    width: usize,
    height: usize,
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    stride_y: usize,
    stride_u: usize,
    stride_v: usize,
) {
    for y in 0..height {
        let row = y * width * 2;
        let y_out = y * stride_y;
        for x in (0..width).step_by(2) {
            let p = row + x * 2;
            if p + 3 >= yuyv.len() {
                continue;
            }
            y_plane[y_out + x] = yuyv[p];
            y_plane[y_out + x + 1] = yuyv[p + 2];
            if y % 2 == 0 {
                let cx = x / 2;
                let cy = y / 2;
                u_plane[cy * stride_u + cx] = yuyv[p + 1];
                v_plane[cy * stride_v + cx] = yuyv[p + 3];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_recorder_read_survives_the_renderer_taking_the_frame() {
        let store = VideoFrameStore::default();
        store.set_recording(true);
        store.publish(7, 2, 1, vec![9u8; 8]);

        assert!(
            store.take_new(7, None).is_some(),
            "renderer takes the frame"
        );
        assert!(store.get(7).is_none(), "the shared slot is now empty");

        let recorded = store.recorded(7).expect("recorder still sees the frame");
        assert_eq!(recorded.bgra, vec![9u8; 8]);
    }

    #[test]
    fn nothing_is_recorded_while_no_recording_runs() {
        let store = VideoFrameStore::default();
        store.publish(7, 2, 1, vec![9u8; 8]);
        assert!(store.recorded(7).is_none());
    }

    #[test]
    fn stopping_a_recording_releases_the_frames() {
        let store = VideoFrameStore::default();
        store.set_recording(true);
        store.publish(7, 2, 1, vec![9u8; 8]);
        store.set_recording(false);
        assert!(store.recorded(7).is_none());
        store.publish(7, 2, 1, vec![9u8; 8]);
        assert!(store.recorded(7).is_none(), "publishing must not refill it");
    }

    use super::*;

    fn reference_i420_to_bgra(
        y_plane: &[u8],
        u_plane: &[u8],
        v_plane: &[u8],
        stride_y: usize,
        stride_u: usize,
        stride_v: usize,
        width: usize,
        height: usize,
    ) -> Vec<u8> {
        let mut out = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let c = y_plane[y * stride_y + x] as i32 - 16;
                let d = u_plane[(y / 2) * stride_u + (x / 2)] as i32 - 128;
                let e = v_plane[(y / 2) * stride_v + (x / 2)] as i32 - 128;
                let o = (y * width + x) * 4;
                out[o] = clamp_u8((298 * c + 516 * d + 128) >> 8);
                out[o + 1] = clamp_u8((298 * c - 100 * d - 208 * e + 128) >> 8);
                out[o + 2] = clamp_u8((298 * c + 409 * e + 128) >> 8);
                out[o + 3] = 255;
            }
        }
        out
    }

    fn assert_bgra_close(actual: &[u8], expected: &[u8], width: usize, height: usize) {
        assert_eq!(actual.len(), expected.len());
        for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
            let (px, ch) = (i / 4, i % 4);
            let (x, y) = (px % width, px / width);
            if ch == 3 {
                assert_eq!(*a, 255, "alpha at ({x},{y}) of {width}x{height}");
            } else {
                assert!(
                    a.abs_diff(*e) <= 2,
                    "channel {ch} at ({x},{y}) of {width}x{height}: {a} vs {e}"
                );
            }
        }
    }

    #[test]
    fn i420_to_bgra_matches_reference() {
        let width = 8usize;
        let height = 6usize;
        let stride_y = width + 3;
        let cw = width / 2;
        let ch = height / 2;
        let stride_u = cw + 2;
        let stride_v = cw + 1;

        let y_plane: Vec<u8> = (0..stride_y * height)
            .map(|i| (16 + i * 7 % 220) as u8)
            .collect();
        let u_plane: Vec<u8> = (0..stride_u * ch)
            .map(|i| (16 + i * 13 % 225) as u8)
            .collect();
        let v_plane: Vec<u8> = (0..stride_v * ch)
            .map(|i| (16 + i * 29 % 225) as u8)
            .collect();

        let expected = reference_i420_to_bgra(
            &y_plane, &u_plane, &v_plane, stride_y, stride_u, stride_v, width, height,
        );

        let mut actual = vec![0u8; width * height * 4];
        i420_to_bgra_into(
            &mut actual,
            &y_plane,
            &u_plane,
            &v_plane,
            stride_y,
            stride_u,
            stride_v,
            width,
            height,
        );

        assert_bgra_close(&actual, &expected, width, height);
    }

    #[test]
    fn i420_to_bgra_matches_reference_odd_width() {
        let width = 7usize;
        let height = 6usize;
        let stride_y = width + 2;
        let cw = width / 2;
        let ch = height / 2;
        let stride_u = cw + 2;
        let stride_v = cw + 1;

        let y_plane: Vec<u8> = (0..stride_y * height)
            .map(|i| (16 + i * 7 % 220) as u8)
            .collect();
        let u_plane: Vec<u8> = (0..stride_u * ch)
            .map(|i| (16 + i * 13 % 225) as u8)
            .collect();
        let v_plane: Vec<u8> = (0..stride_v * ch)
            .map(|i| (16 + i * 29 % 225) as u8)
            .collect();

        let expected = reference_i420_to_bgra(
            &y_plane, &u_plane, &v_plane, stride_y, stride_u, stride_v, width, height,
        );

        let mut actual = vec![0u8; width * height * 4];
        i420_to_bgra_into(
            &mut actual,
            &y_plane,
            &u_plane,
            &v_plane,
            stride_y,
            stride_u,
            stride_v,
            width,
            height,
        );

        assert_bgra_close(&actual, &expected, width, height);
    }

    #[test]
    fn i420_to_bgra_scalar_matches_reference() {
        let width = 8usize;
        let height = 6usize;
        let stride_y = width;
        let cw = width / 2;
        let ch = height / 2;
        let stride_u = cw;
        let stride_v = cw;

        let y_plane: Vec<u8> = (0..stride_y * height)
            .map(|i| (i * 7 % 256) as u8)
            .collect();
        let u_plane: Vec<u8> = (0..stride_u * ch).map(|i| (i * 13 % 256) as u8).collect();
        let v_plane: Vec<u8> = (0..stride_v * ch).map(|i| (i * 29 % 256) as u8).collect();

        let expected = reference_i420_to_bgra(
            &y_plane, &u_plane, &v_plane, stride_y, stride_u, stride_v, width, height,
        );

        let mut actual = vec![0u8; width * height * 4];
        i420_to_bgra_scalar(
            &mut actual,
            &y_plane,
            &u_plane,
            &v_plane,
            stride_y,
            stride_u,
            stride_v,
            width,
            height,
        );

        assert_eq!(actual, expected);
    }

    #[test]
    fn take_new_moves_frame_out_and_clears() {
        let store = VideoFrameStore::default();
        store.publish(7, 2, 2, vec![1, 2, 3, 4]);
        assert_eq!(store.publish_seq(), 1);

        let frame = store.take_new(7, None).expect("frame present");
        assert_eq!((frame.width, frame.height), (2, 2));
        assert_eq!(frame.bgra, vec![1, 2, 3, 4]);

        assert!(store.take_new(7, None).is_none());
        assert!(store.get(7).is_none());
    }

    #[test]
    fn take_new_skips_when_seq_unchanged() {
        let store = VideoFrameStore::default();
        store.publish(1, 1, 1, vec![0, 0, 0, 0]);
        let seq = store.get(1).unwrap().seq;

        assert!(store.take_new(1, Some(seq)).is_none());
        assert!(store.take_new(1, Some(seq + 1)).is_some());
    }

    #[test]
    fn take_new_round_trip_publish_take_republish_take() {
        let store = VideoFrameStore::default();
        let key = 42;

        store.publish(key, 1, 1, vec![1]);
        let first = store.take_new(key, None).expect("first frame");
        let first_seq = first.seq;
        assert_eq!(first.bgra, vec![1]);

        store.publish(key, 1, 1, vec![2]);
        let second = store
            .take_new(key, Some(first_seq))
            .expect("frame after republish");
        assert_eq!(second.bgra, vec![2]);
        assert_ne!(second.seq, first_seq);

        store.publish(key, 1, 1, vec![3]);
        let current_seq = store.get(key).unwrap().seq;
        assert!(store.take_new(key, Some(current_seq)).is_none());
        let third = store
            .take_new(key, Some(current_seq.wrapping_sub(1)))
            .expect("inequality not ordering");
        assert_eq!(third.bgra, vec![3]);
    }

    #[test]
    fn recycled_render_buffer_returns_to_producer() {
        let store = VideoFrameStore::default();
        let key = 9;
        store.publish(key, 2, 2, vec![1; 16]);
        let frame = store.take_new(key, None).expect("frame");
        store.recycle(key, frame.bgra);

        let recycled = store
            .publish(key, 2, 2, vec![2; 16])
            .expect("recycled buffer");
        assert!(recycled.capacity() >= 16);
        assert!(recycled.is_empty());
    }

    #[test]
    fn nv12_full_range_converts_to_limited_i420() {
        let src_y = [0, 255, 128, 64];
        let src_uv = [0, 255];
        let mut y = [0; 4];
        let mut u = [0; 1];
        let mut v = [0; 1];
        nv12_full_to_i420(&src_y, &src_uv, 2, 2, 2, 2, &mut y, &mut u, &mut v, 2, 1, 1);
        assert_eq!(y, [16, 235, 126, 71]);
        assert_eq!(u, [16]);
        assert_eq!(v, [240]);
    }
}
