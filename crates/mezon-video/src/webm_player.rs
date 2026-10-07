use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use matroska_demuxer::{DemuxError, Frame, MatroskaFile, TrackType};
use oxideav_vp8::state::Vp8DecoderState;
use parking_lot::Mutex;

use crate::{PlayerError, VideoFrame, VideoProbe};

const MAX_WEBM_BYTES: usize = 64 * 1024 * 1024;
const WEBM_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const WEBM_HTTP_RECV_BODY_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_DECODE_PIXELS: u64 = 4096 * 4096;

type WebmCursor = Cursor<Arc<[u8]>>;

fn matroska_codec_id(id: &str) -> &str {
    id.trim_end_matches('\0')
}

fn is_vp8_video_track(track: &matroska_demuxer::TrackEntry) -> bool {
    track.track_type() == TrackType::Video && matroska_codec_id(track.codec_id()) == "V_VP8"
}

fn has_audio_track(demuxer: &MatroskaFile<WebmCursor>) -> bool {
    demuxer
        .tracks()
        .iter()
        .any(|track| track.track_type() == TrackType::Audio)
}

pub(crate) fn is_webm_source(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/')
        .next()
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".webm"))
}

struct RawI420 {
    width: u32,
    height: u32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

#[cfg(windows)]
struct CachedBgra {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

struct DemuxState {
    demuxer: MatroskaFile<WebmCursor>,
    video_track: u64,
    timestamp_scale: u64,
    vp8: Vp8DecoderState,
    last_frame_ns: u64,
    last_emitted_ns: Option<u64>,
    raw: Option<RawI420>,
    pending: Option<(u64, RawI420)>,
    #[cfg(windows)]
    cached_bgra: Option<CachedBgra>,
    #[cfg(target_os = "macos")]
    cached: Option<VideoFrame>,
    #[cfg(target_os = "macos")]
    pixel_pool: [Option<VideoFrame>; 2],
    #[cfg(target_os = "macos")]
    pixel_pool_i: usize,
    eos: bool,
}

#[cfg(target_os = "macos")]
unsafe impl Send for DemuxState {}

pub struct WebmPlayerImpl {
    state: Mutex<DemuxState>,
    bytes: Arc<[u8]>,
    duration_seconds: f64,
    playing: AtomicBool,
    play_started_at: Mutex<Option<Instant>>,
    play_offset_ns: AtomicU64,
    volume_bits: AtomicU32,
    muted: AtomicBool,
    failed: AtomicBool,
    max_size: Option<(u32, u32)>,
}

impl WebmPlayerImpl {
    pub fn open(url: &str, max_size: Option<(u32, u32)>) -> Result<Self, PlayerError> {
        if url.is_empty() {
            return Err(PlayerError::InvalidUrl);
        }
        Self::open_bytes(load_bytes(url)?, max_size)
    }

    pub fn open_bytes(bytes: Vec<u8>, max_size: Option<(u32, u32)>) -> Result<Self, PlayerError> {
        let bytes: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
        let mut demuxer = open_demuxer(&bytes)?;
        if has_audio_track(&demuxer) {
            tracing::warn!(
                target: "mezon_video",
                "webm with audio is not decoded inline; use download or open externally"
            );
            return Err(PlayerError::Open);
        }
        let video_track = demuxer
            .tracks()
            .iter()
            .find(|track| is_vp8_video_track(track))
            .map(|track| track.track_number().get())
            .ok_or_else(|| {
                tracing::warn!(target: "mezon_video", "webm has no VP8 video track");
                PlayerError::Open
            })?;
        let timestamp_scale = demuxer.info().timestamp_scale().get();
        let header_duration = header_duration_seconds(demuxer.info());
        let duration_seconds = if let Some(duration) = header_duration {
            duration
        } else {
            let duration = video_duration_from_frames(&mut demuxer, video_track, timestamp_scale)?;
            demuxer = open_demuxer(&bytes)?;
            duration
        };
        let mut state = DemuxState {
            demuxer,
            video_track,
            timestamp_scale,
            vp8: Vp8DecoderState::new().with_max_pixels_per_frame(MAX_DECODE_PIXELS),
            last_frame_ns: 0,
            last_emitted_ns: None,
            raw: None,
            pending: None,
            #[cfg(windows)]
            cached_bgra: None,
            #[cfg(target_os = "macos")]
            cached: None,
            #[cfg(target_os = "macos")]
            pixel_pool: [None, None],
            #[cfg(target_os = "macos")]
            pixel_pool_i: 0,
            eos: false,
        };
        if !decode_until(&mut state, 0, max_size)? {
            return Err(PlayerError::Open);
        }
        Ok(Self {
            state: Mutex::new(state),
            bytes,
            duration_seconds,
            playing: AtomicBool::new(false),
            play_started_at: Mutex::new(None),
            play_offset_ns: AtomicU64::new(0),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            max_size,
        })
    }

    pub fn copy_frame(&self) -> Option<VideoFrame> {
        if self.failed.load(Ordering::SeqCst) {
            return None;
        }
        let playing = self.playing.load(Ordering::SeqCst);
        let target_ns = if playing {
            self.play_offset_ns
                .load(Ordering::SeqCst)
                .saturating_add(self.elapsed_ns())
        } else {
            self.play_offset_ns.load(Ordering::SeqCst)
        };
        let mut state = self.state.lock();
        if playing && !state.eos {
            if let Err(error) = advance_to(&mut state, target_ns, self.max_size) {
                tracing::warn!(target: "mezon_video", ?error, "webm play advance failed");
                self.failed.store(true, Ordering::SeqCst);
                return None;
            }
        }
        take_frame(&mut state)
    }

    pub fn play(&self) {
        if self.failed.load(Ordering::SeqCst) || self.duration_seconds <= 0.0 {
            return;
        }
        if !self.playing.swap(true, Ordering::SeqCst) {
            *self.play_started_at.lock() = Some(Instant::now());
        }
    }

    pub fn pause(&self) {
        if self.playing.swap(false, Ordering::SeqCst) {
            self.play_offset_ns
                .fetch_add(self.elapsed_ns(), Ordering::SeqCst);
            *self.play_started_at.lock() = None;
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::SeqCst)
    }

    pub fn current_time(&self) -> f64 {
        if self.duration_seconds <= 0.0 {
            return 0.0;
        }
        let ns = if self.playing.load(Ordering::SeqCst) {
            self.play_offset_ns
                .load(Ordering::SeqCst)
                .saturating_add(self.elapsed_ns())
        } else {
            self.play_offset_ns.load(Ordering::SeqCst)
        };
        (ns as f64 / 1_000_000_000.0).min(self.duration_seconds)
    }

    pub fn duration(&self) -> f64 {
        self.duration_seconds
    }

    pub fn seek(&self, to_seconds: f64) {
        if self.failed.load(Ordering::SeqCst) {
            return;
        }
        let target = if to_seconds.is_finite() && to_seconds >= 0.0 {
            to_seconds
        } else {
            0.0
        };
        let target_ns = (target * 1_000_000_000.0) as u64;
        self.playing.store(false, Ordering::SeqCst);
        *self.play_started_at.lock() = None;
        self.play_offset_ns.store(target_ns, Ordering::SeqCst);
        let mut state = self.state.lock();
        let can_advance = !state.eos && state.raw.is_some() && target_ns >= state.last_frame_ns;
        let seek_result = if can_advance {
            decode_until(&mut state, target_ns, self.max_size).map(|_| ())
        } else {
            reopen_at(&mut state, &self.bytes, target_ns, self.max_size)
        };
        if seek_result.is_err() {
            tracing::warn!(
                target: "mezon_video",
                target_ms = target_ns / 1_000_000,
                "webm seek failed"
            );
            self.failed.store(true, Ordering::SeqCst);
        }
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume_bits
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::SeqCst);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::SeqCst))
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::SeqCst);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::SeqCst)
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    fn elapsed_ns(&self) -> u64 {
        self.play_started_at
            .lock()
            .as_ref()
            .map(|started| started.elapsed().as_nanos() as u64)
            .unwrap_or(0)
    }
}

pub fn probe_webm(path: &str, max_poster_edge: u32) -> Option<VideoProbe> {
    let bytes = load_bytes(path).ok()?;
    let bytes: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
    let mut demuxer = open_demuxer(&bytes).ok()?;
    let video_track = demuxer
        .tracks()
        .iter()
        .find(|track| is_vp8_video_track(track))?
        .track_number()
        .get();
    let mut frame = Frame::default();
    while demuxer.next_frame(&mut frame).ok()? {
        if frame.track != video_track {
            continue;
        }
        let mut vp8 = Vp8DecoderState::new().with_max_pixels_per_frame(MAX_DECODE_PIXELS);
        let decoded = vp8.decode_frame(&frame.data).ok()?;
        if vp8.last_frame_shown() == Some(false) {
            continue;
        }
        #[cfg(windows)]
        let poster_jpeg = {
            let bgra = crate::frame_util::i420_to_bgra(
                decoded.width,
                decoded.height,
                &decoded.y,
                &decoded.u,
                &decoded.v,
            )?;
            crate::poster::encode_poster_jpeg(
                &bgra,
                decoded.width,
                decoded.height,
                (decoded.width as usize).saturating_mul(4),
                false,
                crate::poster::Turn::default(),
                max_poster_edge,
            )
        };
        #[cfg(target_os = "macos")]
        let _ = max_poster_edge;
        return Some(VideoProbe {
            width: decoded.width,
            height: decoded.height,
            #[cfg(windows)]
            poster_jpeg,
            #[cfg(target_os = "macos")]
            poster_jpeg: None,
        });
    }
    None
}

pub fn load_webm_bytes(url: &str) -> Result<Vec<u8>, PlayerError> {
    load_bytes(url)
}

fn open_demuxer(bytes: &Arc<[u8]>) -> Result<MatroskaFile<WebmCursor>, PlayerError> {
    MatroskaFile::open(Cursor::new(Arc::clone(bytes))).map_err(|error| {
        tracing::warn!(target: "mezon_video", ?error, "webm demuxer open failed");
        PlayerError::Open
    })
}

fn clear_frame_cache(state: &mut DemuxState) {
    state.vp8 = Vp8DecoderState::new().with_max_pixels_per_frame(MAX_DECODE_PIXELS);
    state.last_frame_ns = 0;
    state.last_emitted_ns = None;
    state.raw = None;
    state.pending = None;
    #[cfg(windows)]
    {
        state.cached_bgra = None;
    }
    #[cfg(target_os = "macos")]
    {
        state.cached = None;
        state.pixel_pool = [None, None];
        state.pixel_pool_i = 0;
    }
    state.eos = false;
}

fn reopen_at(
    state: &mut DemuxState,
    bytes: &Arc<[u8]>,
    target_ns: u64,
    max_size: Option<(u32, u32)>,
) -> Result<(), PlayerError> {
    state.demuxer = open_demuxer(bytes)?;
    clear_frame_cache(state);
    if !decode_until(state, target_ns, max_size)? {
        return Err(PlayerError::Open);
    }
    Ok(())
}

fn header_duration_seconds(info: &matroska_demuxer::Info) -> Option<f64> {
    let ticks = info.duration()?;
    let scale = info.timestamp_scale().get() as f64;
    let seconds = ticks * scale / 1_000_000_000.0;
    (seconds.is_finite() && seconds > 0.05).then_some(seconds)
}

fn video_duration_from_frames(
    demuxer: &mut MatroskaFile<WebmCursor>,
    video_track: u64,
    timestamp_scale: u64,
) -> Result<f64, PlayerError> {
    let mut frame = Frame::default();
    let mut max_ns = 0u64;
    while demuxer.next_frame(&mut frame).ok() == Some(true) {
        if frame.track == video_track {
            max_ns = max_ns.max(frame.timestamp.saturating_mul(timestamp_scale));
        }
    }
    Ok(if max_ns > 0 {
        max_ns as f64 / 1_000_000_000.0
    } else {
        0.0
    })
}

fn load_bytes(url: &str) -> Result<Vec<u8>, PlayerError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(WEBM_HTTP_CONNECT_TIMEOUT))
            .timeout_recv_body(Some(WEBM_HTTP_RECV_BODY_TIMEOUT))
            .build()
            .into();
        let mut response = agent.get(url).call().map_err(|error| {
            tracing::warn!(target: "mezon_video", ?error, "webm download failed");
            PlayerError::Open
        })?;
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_WEBM_BYTES as u64)
            .read_to_vec()
            .map_err(|_| PlayerError::Open)?;
        Ok(body)
    } else if let Some(path) = local_file_path(url) {
        read_local_capped(path)
    } else {
        Err(PlayerError::InvalidUrl)
    }
}

fn local_file_path(url: &str) -> Option<&str> {
    let path = if let Some(path) = url.strip_prefix("file://") {
        path
    } else if !url.contains("://") {
        url
    } else {
        return None;
    };
    if path.starts_with("//") || path.starts_with("\\\\") {
        return None;
    }
    #[cfg(windows)]
    {
        let trimmed = path.trim_start_matches('/');
        if trimmed.starts_with('\\') || trimmed.starts_with("//") {
            return None;
        }
        if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
            return Some(trimmed);
        }
    }
    Some(path)
}

fn read_local_capped(path: &str) -> Result<Vec<u8>, PlayerError> {
    let meta = std::fs::metadata(path).map_err(|_| PlayerError::Open)?;
    if !meta.is_file() || meta.len() > MAX_WEBM_BYTES as u64 {
        return Err(PlayerError::Open);
    }
    let bytes = std::fs::read(path).map_err(|_| PlayerError::Open)?;
    if bytes.len() > MAX_WEBM_BYTES {
        return Err(PlayerError::Open);
    }
    Ok(bytes)
}

fn take_frame(state: &mut DemuxState) -> Option<VideoFrame> {
    if state.last_emitted_ns == Some(state.last_frame_ns) {
        return None;
    }
    #[cfg(windows)]
    {
        let cached = state.cached_bgra.as_ref()?;
        let frame =
            crate::render_frame::bgra_to_frame(cached.width, cached.height, cached.bgra.clone())?;
        state.last_emitted_ns = Some(state.last_frame_ns);
        return Some(frame);
    }
    #[cfg(target_os = "macos")]
    {
        let frame = state.cached.clone()?;
        state.last_emitted_ns = Some(state.last_frame_ns);
        return Some(frame);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = state;
        None
    }
}

fn advance_to(
    state: &mut DemuxState,
    target_ns: u64,
    max_size: Option<(u32, u32)>,
) -> Result<bool, DemuxError> {
    if state.eos {
        return Ok(present_raw(state, max_size));
    }
    loop {
        let next = match state.pending.take() {
            Some(pending) => Some(pending),
            None => decode_next_shown_planes(state)?,
        };
        match next {
            None => {
                state.eos = true;
                break;
            }
            Some((timestamp_ns, raw)) => {
                if timestamp_ns > target_ns && state.raw.is_some() {
                    state.pending = Some((timestamp_ns, raw));
                    break;
                }
                state.last_frame_ns = timestamp_ns;
                state.raw = Some(raw);
                invalidate_presented(state);
                if timestamp_ns >= target_ns {
                    break;
                }
            }
        }
    }
    Ok(present_raw(state, max_size))
}

fn invalidate_presented(state: &mut DemuxState) {
    #[cfg(windows)]
    {
        state.cached_bgra = None;
    }
    #[cfg(target_os = "macos")]
    {
        state.cached = None;
    }
    state.last_emitted_ns = None;
}

fn present_raw(state: &mut DemuxState, max_size: Option<(u32, u32)>) -> bool {
    #[cfg(windows)]
    {
        if state.cached_bgra.is_some() {
            return true;
        }
        let Some(raw) = state.raw.as_ref() else {
            return false;
        };
        let Some(cached) = raw_to_bgra(raw, max_size) else {
            return false;
        };
        state.cached_bgra = Some(cached);
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        if state.cached.is_some() {
            return true;
        }
        let Some(raw) = state.raw.as_ref() else {
            return false;
        };
        let Some((out_w, out_h)) =
            crate::webm_frame_macos::output_size(raw.width, raw.height, max_size)
        else {
            return false;
        };
        let slot = state.pixel_pool_i % 2;
        state.pixel_pool_i = state.pixel_pool_i.wrapping_add(1);
        let reuse = state.pixel_pool[slot]
            .as_ref()
            .is_some_and(|buf| buf.get_width() as u32 == out_w && buf.get_height() as u32 == out_h);
        if !reuse {
            let Some(buffer) = crate::webm_frame_macos::create_pixel_buffer(out_w, out_h) else {
                return false;
            };
            state.pixel_pool[slot] = Some(buffer);
        }
        let Some(buffer) = state.pixel_pool[slot].as_ref() else {
            return false;
        };
        if crate::webm_frame_macos::fill_pixel_buffer_from_i420(
            buffer, raw.width, raw.height, &raw.y, &raw.u, &raw.v, max_size,
        )
        .is_none()
        {
            return false;
        }
        state.cached = Some(buffer.clone());
        return true;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (state, max_size);
        false
    }
}

fn decode_until(
    state: &mut DemuxState,
    target_ns: u64,
    max_size: Option<(u32, u32)>,
) -> Result<bool, PlayerError> {
    advance_to(state, target_ns, max_size).map_err(|error| {
        tracing::warn!(target: "mezon_video", ?error, "webm decode failed");
        PlayerError::Open
    })
}

fn decode_next_shown_planes(state: &mut DemuxState) -> Result<Option<(u64, RawI420)>, DemuxError> {
    let mut frame = Frame::default();
    while state.demuxer.next_frame(&mut frame)? {
        if frame.track != state.video_track {
            continue;
        }
        let timestamp_ns = frame.timestamp.saturating_mul(state.timestamp_scale);
        let decoded = match state.vp8.decode_frame(&frame.data) {
            Ok(decoded) => decoded,
            Err(_) => continue,
        };
        if state.vp8.last_frame_shown() == Some(false) {
            continue;
        }
        return Ok(Some((
            timestamp_ns,
            RawI420 {
                width: decoded.width,
                height: decoded.height,
                y: decoded.y,
                u: decoded.u,
                v: decoded.v,
            },
        )));
    }
    Ok(None)
}

#[cfg(windows)]
fn raw_to_bgra(raw: &RawI420, max_size: Option<(u32, u32)>) -> Option<CachedBgra> {
    let mut bgra = crate::frame_util::i420_to_bgra(raw.width, raw.height, &raw.y, &raw.u, &raw.v)?;
    let (width, height) = match max_size {
        Some((max_w, max_h)) if max_w > 0 && max_h > 0 => {
            let max_w = max_w.min(raw.width);
            let max_h = max_h.min(raw.height);
            if raw.width <= max_w && raw.height <= max_h {
                (raw.width, raw.height)
            } else {
                let scale = (max_w as f32 / raw.width as f32).min(max_h as f32 / raw.height as f32);
                let out_w = ((raw.width as f32 * scale).round() as u32).max(1);
                let out_h = ((raw.height as f32 * scale).round() as u32).max(1);
                bgra = scale_bgra(&bgra, raw.width, raw.height, out_w, out_h)?;
                (out_w, out_h)
            }
        }
        _ => (raw.width, raw.height),
    };
    Some(CachedBgra {
        width,
        height,
        bgra,
    })
}

#[cfg(windows)]
fn scale_bgra(source: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Option<Vec<u8>> {
    let src_w = src_w as usize;
    let src_h = src_h as usize;
    let dst_w = dst_w as usize;
    let dst_h = dst_h as usize;
    let src_stride = src_w.checked_mul(4)?;
    if source.len() < src_stride.checked_mul(src_h)? {
        return None;
    }
    let mut out = vec![0u8; dst_w.checked_mul(dst_h)?.checked_mul(4)?];
    for y in 0..dst_h {
        let src_y = y * src_h / dst_h;
        for x in 0..dst_w {
            let src_x = x * src_w / dst_w;
            let from = src_y * src_stride + src_x * 4;
            let to = y * dst_w * 4 + x * 4;
            out[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webm_sources_are_detected_by_extension() {
        assert!(is_webm_source("https://cdn.example/clip.webm"));
        assert!(is_webm_source("https://cdn.example/clip.webm?token=1"));
        assert!(!is_webm_source("https://cdn.example/clip.mp4"));
    }

    #[test]
    fn matroska_codec_id_strips_null_suffix() {
        assert_eq!(matroska_codec_id("V_VP8\0"), "V_VP8");
        assert_eq!(matroska_codec_id("V_VP8"), "V_VP8");
    }

    #[test]
    fn webm_player_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<WebmPlayerImpl>();
    }

    #[test]
    fn local_file_path_rejects_unc() {
        assert!(local_file_path("file:///tmp/a.webm").is_some());
        assert!(local_file_path("/tmp/a.webm").is_some());
        assert!(local_file_path("file:////server/share/a.webm").is_none());
        assert!(local_file_path("https://cdn.example/a.webm").is_none());
    }
}
