use std::cell::{Cell, RefCell};
use std::num::{NonZeroU16, NonZeroU32};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;
use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::AudioError;
use crate::decode::DecodedPcm;
use crate::stream::{ChunkState, PcmStream};

const STREAM_SPAN: usize = 32_768;
const OUTPUT_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const PLAYBACK_STALL_INTERVAL: Duration = Duration::from_secs(3);
const OUTPUT_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const STALL_RECOVERY_INTERVAL: Duration = Duration::from_secs(15);
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(50);
const HEARTBEAT_STALE: Duration = Duration::from_secs(2);
const HEARTBEAT_STARTUP_GRACE: Duration = Duration::from_secs(5);
const NO_HEARTBEAT: u64 = u64::MAX;

static PREFERRED_OUTPUT: Mutex<Option<String>> = Mutex::new(None);

thread_local! {
    static SHARED_SINK: RefCell<Weak<SharedSink>> = const { RefCell::new(Weak::new()) };
}

pub fn set_output_device(device_id: Option<String>) {
    let mut preferred = PREFERRED_OUTPUT.lock();
    if *preferred != device_id {
        tracing::info!(device_id = ?device_id, "sound output device preference changed");
        *preferred = device_id;
    }
}

struct SharedSink {
    sink: MixerDeviceSink,
    requested: String,
    broken: Arc<AtomicBool>,
    actual_id: Option<String>,
    heartbeat: OutputHeartbeat,
    last_preferred_probe: Cell<Instant>,
}

impl SharedSink {
    fn new(
        mut sink: MixerDeviceSink,
        requested: String,
        broken: Arc<AtomicBool>,
        actual_id: Option<String>,
    ) -> Self {
        sink.log_on_drop(false);
        let heartbeat = OutputHeartbeat::attach(&sink);
        Self {
            sink,
            requested,
            broken,
            actual_id,
            heartbeat,
            last_preferred_probe: Cell::new(Instant::now()),
        }
    }

    fn usable(&self) -> bool {
        if self.broken.load(Ordering::Relaxed) {
            return false;
        }
        if !self.heartbeat.stalled() {
            return true;
        }
        if !self.broken.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                device = ?self.actual_id,
                "sound output stopped pulling audio, reopening"
            );
        }
        false
    }
}

struct OutputHeartbeat {
    origin: Instant,
    last_beat_ms: Arc<AtomicU64>,
}

impl OutputHeartbeat {
    fn attach(sink: &MixerDeviceSink) -> Self {
        let origin = Instant::now();
        let last_beat_ms = Arc::new(AtomicU64::new(NO_HEARTBEAT));
        let config = sink.config();
        sink.mixer().add(HeartbeatSource::new(
            origin,
            Arc::clone(&last_beat_ms),
            config.channel_count(),
            config.sample_rate(),
        ));
        Self {
            origin,
            last_beat_ms,
        }
    }

    fn stalled(&self) -> bool {
        output_stalled(
            self.origin.elapsed(),
            self.last_beat_ms.load(Ordering::Relaxed),
        )
    }
}

fn output_stalled(since_open: Duration, last_beat_ms: u64) -> bool {
    if last_beat_ms == NO_HEARTBEAT {
        return since_open >= HEARTBEAT_STARTUP_GRACE;
    }
    since_open.saturating_sub(Duration::from_millis(last_beat_ms)) >= HEARTBEAT_STALE
}

struct HeartbeatSource {
    origin: Instant,
    last_beat_ms: Arc<AtomicU64>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    samples_per_beat: usize,
    until_beat: usize,
}

impl HeartbeatSource {
    fn new(
        origin: Instant,
        last_beat_ms: Arc<AtomicU64>,
        channels: NonZeroU16,
        sample_rate: NonZeroU32,
    ) -> Self {
        let samples_per_second = u128::from(sample_rate.get()) * u128::from(channels.get());
        let samples_per_beat =
            usize::try_from(samples_per_second * HEARTBEAT_INTERVAL.as_millis() / 1000)
                .unwrap_or(usize::MAX)
                .max(1);
        Self {
            origin,
            last_beat_ms,
            channels,
            sample_rate,
            samples_per_beat,
            until_beat: 0,
        }
    }
}

impl Iterator for HeartbeatSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.until_beat == 0 {
            self.until_beat = self.samples_per_beat;
            let elapsed_ms = self
                .origin
                .elapsed()
                .as_millis()
                .min(u128::from(NO_HEARTBEAT - 1)) as u64;
            self.last_beat_ms.store(elapsed_ms, Ordering::Relaxed);
        }
        self.until_beat -= 1;
        Some(0.0)
    }
}

impl Source for HeartbeatSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZeroU16 {
        self.channels
    }

    fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

fn requested_output(host: &cpal::Host) -> String {
    if let Some(id) = PREFERRED_OUTPUT.lock().clone() {
        return id;
    }
    host.default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string())
        .unwrap_or_default()
}

fn find_output(host: &cpal::Host, id: &str) -> Option<cpal::Device> {
    host.output_devices()
        .ok()?
        .find(|device| matches!(device.id(), Ok(found) if found.to_string() == id))
}

fn open_on(device: cpal::Device) -> Result<(MixerDeviceSink, Arc<AtomicBool>), String> {
    let broken = Arc::new(AtomicBool::new(false));
    let sink = DeviceSinkBuilder::from_device(device)
        .map_err(|e| e.to_string())?
        .with_error_callback({
            let broken = Arc::clone(&broken);
            move |err| {
                if !broken.swap(true, Ordering::Relaxed) {
                    tracing::warn!("sound output stream error: {err}");
                }
            }
        })
        .open_sink_or_fallback()
        .map_err(|e| e.to_string())?;
    Ok((sink, broken))
}

fn open_any_output(
    host: &cpal::Host,
) -> Result<
    (
        MixerDeviceSink,
        Option<String>,
        Option<String>,
        Arc<AtomicBool>,
    ),
    String,
> {
    if let Some(device) = host.default_output_device() {
        let name = device
            .description()
            .ok()
            .map(|description| description.to_string());
        let id = device.id().ok().map(|id| id.to_string());
        if let Ok((sink, broken)) = open_on(device) {
            return Ok((sink, name, id, broken));
        }
    }
    let devices = host.output_devices().map_err(|e| e.to_string())?;
    let mut last_error = "no output device".to_string();
    for device in devices {
        let name = device
            .description()
            .ok()
            .map(|description| description.to_string());
        let id = device.id().ok().map(|id| id.to_string());
        match open_on(device) {
            Ok((sink, broken)) => return Ok((sink, name, id, broken)),
            Err(err) => last_error = err,
        }
    }
    Err(last_error)
}

fn open_shared_sink(host: &cpal::Host, requested: String) -> Result<SharedSink, AudioError> {
    let preferred = PREFERRED_OUTPUT.lock().clone();
    let device = preferred
        .as_deref()
        .and_then(|id| find_output(host, id))
        .or_else(|| host.default_output_device());
    let device_name = device
        .as_ref()
        .and_then(|device| device.description().ok())
        .map(|description| description.to_string());
    let device_id = device
        .as_ref()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());
    let opened = match device {
        Some(device) => open_on(device),
        None => Err("no output device".to_string()),
    };
    let (sink, actual_device_name, actual_id, broken) = match opened {
        Ok((sink, broken)) => (sink, device_name.clone(), device_id, broken),
        Err(e) => {
            tracing::warn!(
                requested = %requested,
                device = ?device_name,
                "sound output open failed, falling back to any output: {e}"
            );
            open_any_output(host).map_err(|e| {
                tracing::warn!("sound output fallback failed: {e}");
                AudioError::Output(e)
            })?
        }
    };
    tracing::info!(
        requested = %requested,
        device = ?actual_device_name,
        actual_id = ?actual_id,
        sample_rate = ?sink.config().sample_rate(),
        channels = ?sink.config().channel_count(),
        "sound output opened"
    );
    Ok(SharedSink::new(sink, requested, broken, actual_id))
}

fn restore_preferred_output(host: &cpal::Host, requested: &str) -> Option<Rc<SharedSink>> {
    let device = find_output(host, requested)?;
    let device_name = device
        .description()
        .ok()
        .map(|description| description.to_string());
    let (sink, broken) = open_on(device).ok()?;
    tracing::info!(
        requested,
        device = ?device_name,
        "sound output preference restored"
    );
    Some(Rc::new(SharedSink::new(
        sink,
        requested.to_string(),
        broken,
        Some(requested.to_string()),
    )))
}

fn replacement_opened(sink: &Rc<SharedSink>) -> bool {
    SHARED_SINK.with(|cell| {
        cell.borrow()
            .upgrade()
            .is_some_and(|current| !Rc::ptr_eq(&current, sink))
    })
}

fn shared_sink() -> Result<Rc<SharedSink>, AudioError> {
    let host = cpal::default_host();
    let requested = requested_output(&host);
    SHARED_SINK.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(existing) = slot.upgrade()
            && existing.requested == requested
            && existing.usable()
        {
            if !requested.is_empty()
                && existing.actual_id.as_deref() != Some(requested.as_str())
                && existing.last_preferred_probe.get().elapsed() >= OUTPUT_RETRY_INTERVAL
            {
                existing.last_preferred_probe.set(Instant::now());
                if let Some(sink) = restore_preferred_output(&host, &requested) {
                    *slot = Rc::downgrade(&sink);
                    return Ok(sink);
                }
            }
            return Ok(existing);
        }
        let sink = Rc::new(open_shared_sink(&host, requested)?);
        *slot = Rc::downgrade(&sink);
        Ok(sink)
    })
}

struct PcmData {
    samples: Arc<[f32]>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    duration: f64,
}

fn downmix_to_playable(samples: Arc<[f32]>, channels: usize) -> (Arc<[f32]>, u16) {
    let ch = channels.max(1);
    if ch <= 2 {
        return (samples, ch as u16);
    }
    let frames = samples.len() / ch;
    let mut mono = Vec::with_capacity(frames);
    for frame in samples.chunks_exact(ch) {
        mono.push(frame.iter().sum::<f32>() / ch as f32);
    }
    (mono.into(), 1)
}

struct SharedSamplesSource {
    samples: Arc<[f32]>,
    position: Arc<AtomicUsize>,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    duration: f64,
}

fn secs_to_index(secs: f64, sample_rate: u32, channels: usize) -> usize {
    let frames = (secs.max(0.0) * f64::from(sample_rate)) as usize;
    frames.saturating_mul(channels.max(1))
}

fn align_phase(current: usize, target: usize, channels: usize, len: usize) -> usize {
    if target >= len {
        return len;
    }
    if channels <= 1 {
        return target;
    }
    let phase = if current < len { current % channels } else { 0 };
    let back = (target % channels + channels - phase) % channels;
    target.saturating_sub(back)
}

fn store_aligned(cursor: &AtomicUsize, target: usize, channels: usize, len: usize) {
    let mut current = cursor.load(Ordering::Relaxed);
    loop {
        let aligned = align_phase(current, target, channels, len);
        match cursor.compare_exchange_weak(current, aligned, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

impl Iterator for SharedSamplesSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let index = self.position.fetch_add(1, Ordering::Relaxed);
        self.samples.get(index).copied()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let position = self.position.load(Ordering::Relaxed);
        let remaining = self.samples.len().saturating_sub(position);
        (remaining, Some(remaining))
    }
}

impl Source for SharedSamplesSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.position.load(Ordering::Relaxed) >= self.samples.len() {
            Some(0)
        } else {
            Some(self.samples.len())
        }
    }

    fn channels(&self) -> NonZeroU16 {
        self.channels
    }

    fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(self.duration))
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let channels = self.channels.get() as usize;
        let sample = secs_to_index(pos.as_secs_f64(), self.sample_rate.get(), channels)
            .min(self.samples.len());
        store_aligned(&self.position, sample, channels, self.samples.len());
        Ok(())
    }
}

struct PcmStreamSource {
    stream: Arc<PcmStream>,
    cursor: Arc<AtomicUsize>,
    chunk: Option<Arc<[f32]>>,
    next_chunk: usize,
    offset: usize,
    at: usize,
    silence_debt: usize,
    exhausted: bool,
    looping: bool,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
}

impl PcmStreamSource {
    fn new(stream: Arc<PcmStream>, cursor: Arc<AtomicUsize>) -> Self {
        let channels =
            NonZeroU16::new(stream.channels().clamp(1, 2) as u16).unwrap_or(NonZeroU16::MIN);
        let sample_rate =
            NonZeroU32::new(stream.sample_rate()).unwrap_or(NonZeroU32::new(48_000).unwrap());
        Self {
            stream,
            cursor,
            chunk: None,
            next_chunk: 0,
            offset: 0,
            at: 0,
            silence_debt: 0,
            exhausted: false,
            looping: false,
            channels,
            sample_rate,
        }
    }

    fn jump_to(&mut self, target: usize) -> bool {
        let Some((index, offset, chunk)) = self.stream.locate(target) else {
            return false;
        };
        self.chunk = Some(chunk);
        self.offset = offset;
        self.next_chunk = index + 1;
        self.silence_debt = 0;
        self.exhausted = false;
        true
    }

    fn looping(mut self) -> Self {
        self.looping = true;
        self
    }

    fn silence(&mut self) -> Option<f32> {
        self.silence_debt += 1;
        Some(0.0)
    }
}

impl Iterator for PcmStreamSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let commanded = self.cursor.load(Ordering::Relaxed);
        if commanded != self.at && self.jump_to(commanded) {
            self.at = commanded;
        }
        loop {
            if let Some(chunk) = &self.chunk
                && let Some(sample) = chunk.get(self.offset).copied()
            {
                self.offset += 1;
                let played = self.at;
                self.at += 1;
                let _ = self.cursor.compare_exchange(
                    played,
                    self.at,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
                return Some(sample);
            }
            if !self
                .silence_debt
                .is_multiple_of(self.channels.get() as usize)
            {
                return self.silence();
            }
            match self.stream.chunk_at(self.next_chunk) {
                ChunkState::Ready(chunk) => {
                    self.chunk = Some(chunk);
                    self.next_chunk += 1;
                    self.offset = 0;
                    self.silence_debt = 0;
                }
                ChunkState::Pending => return self.silence(),
                ChunkState::Complete => {
                    if self.looping && self.next_chunk > 0 {
                        self.next_chunk = 0;
                        self.offset = 0;
                        self.chunk = None;
                        self.at = 0;
                        self.cursor.store(0, Ordering::Relaxed);
                        self.silence_debt = 0;
                        continue;
                    }
                    self.exhausted = true;
                    return None;
                }
            }
        }
    }
}

impl Source for PcmStreamSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.exhausted {
            Some(0)
        } else {
            Some(STREAM_SPAN)
        }
    }

    fn channels(&self) -> NonZeroU16 {
        self.channels
    }

    fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let channels = self.channels.get() as usize;
        let len = self.stream.buffered_samples();
        let complete = self.stream.is_complete();
        let target = secs_to_index(pos.as_secs_f64(), self.sample_rate.get(), channels);
        let limit = if complete { len } else { len.saturating_add(1) };
        let aligned = align_phase(self.at, target.min(limit), channels, limit);
        if !complete && aligned >= len {
            return Err(rodio::source::SeekError::NotSupported {
                underlying_source: "PcmStreamSource",
            });
        }
        if !self.jump_to(aligned) {
            return Err(rodio::source::SeekError::NotSupported {
                underlying_source: "PcmStreamSource",
            });
        }
        self.at = aligned;
        self.cursor.store(aligned, Ordering::Relaxed);
        Ok(())
    }
}

enum Playable {
    Pcm(PcmData),
    Stream(Arc<PcmStream>),
}

enum QueuedSource {
    Pcm(SharedSamplesSource),
    Stream(PcmStreamSource),
}

struct PreparedSource {
    cursor: Arc<AtomicUsize>,
    source: QueuedSource,
}

fn append_source(player: &Player, source: QueuedSource, looping: bool) {
    match (source, looping) {
        (QueuedSource::Pcm(source), true) => player.append(source.repeat_infinite()),
        (QueuedSource::Stream(source), true) => player.append(source.looping()),
        (QueuedSource::Pcm(source), false) => player.append(source),
        (QueuedSource::Stream(source), false) => player.append(source),
    }
}

#[derive(Default)]
struct OutputProgress {
    last_sample: Option<(usize, Instant)>,
    last_recovery: Option<Instant>,
}

impl OutputProgress {
    fn reset(&mut self) {
        self.last_sample = None;
    }

    fn observe(&mut self, now: Instant, playing: bool, sample: usize, playable: bool) -> bool {
        if !playing || !playable {
            self.reset();
            return false;
        }
        match self.last_sample {
            Some((previous, since)) if previous == sample => {
                let retry_allowed = self.last_recovery.is_none_or(|recovered| {
                    now.duration_since(recovered) >= STALL_RECOVERY_INTERVAL
                });
                if retry_allowed && now.duration_since(since) >= PLAYBACK_STALL_INTERVAL {
                    self.last_recovery = Some(now);
                    self.reset();
                    true
                } else {
                    false
                }
            }
            _ => {
                self.last_sample = Some((sample, now));
                false
            }
        }
    }
}

pub struct AudioPlayer {
    sink: RefCell<Rc<SharedSink>>,
    player: RefCell<Player>,
    volume: Cell<f32>,
    data: RefCell<Option<Playable>>,
    cursor: RefCell<Option<Arc<AtomicUsize>>>,
    started: Cell<bool>,
    looping: Cell<bool>,
    last_output_check: Cell<Option<Instant>>,
    output_progress: RefCell<OutputProgress>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self, AudioError> {
        let sink = shared_sink()?;
        let player = Player::connect_new(sink.sink.mixer());
        Ok(Self {
            sink: RefCell::new(sink),
            player: RefCell::new(player),
            volume: Cell::new(1.0),
            data: RefCell::new(None),
            cursor: RefCell::new(None),
            started: Cell::new(false),
            looping: Cell::new(false),
            last_output_check: Cell::new(None),
            output_progress: RefCell::new(OutputProgress::default()),
        })
    }

    fn follow_output(&self) {
        let Ok(sink) = shared_sink() else {
            return;
        };
        if Rc::ptr_eq(&sink, &self.sink.borrow()) {
            return;
        }
        let had_source = !self.player.borrow().empty();
        let playing = self.is_playing();
        let prepared = if had_source {
            self.prepare_source(self.position_secs())
        } else {
            None
        };
        if had_source && prepared.is_none() {
            return;
        }
        let player = Player::connect_new(sink.sink.mixer());
        player.set_volume(self.volume.get());
        if !playing {
            player.pause();
        }
        if let Some(prepared) = prepared {
            append_source(&player, prepared.source, self.looping.get());
            *self.cursor.borrow_mut() = Some(prepared.cursor);
        }
        *self.player.borrow_mut() = player;
        *self.sink.borrow_mut() = sink;
        self.output_progress.borrow_mut().reset();
    }

    pub fn set_data(&self, pcm: DecodedPcm) {
        let sample_rate =
            NonZeroU32::new(pcm.sample_rate).unwrap_or(NonZeroU32::new(48_000).unwrap());
        let duration = pcm.duration_secs();
        let (samples, channel_count) = downmix_to_playable(pcm.samples, pcm.channels);
        let channels = NonZeroU16::new(channel_count).unwrap_or(NonZeroU16::MIN);
        *self.data.borrow_mut() = Some(Playable::Pcm(PcmData {
            samples,
            channels,
            sample_rate,
            duration,
        }));
        *self.cursor.borrow_mut() = None;
        self.looping.set(false);
        self.output_progress.borrow_mut().reset();
    }

    pub fn set_stream(&self, stream: Arc<PcmStream>) {
        *self.data.borrow_mut() = Some(Playable::Stream(stream));
        *self.cursor.borrow_mut() = None;
        self.looping.set(false);
        self.output_progress.borrow_mut().reset();
    }

    pub fn is_ready(&self) -> bool {
        self.data.borrow().is_some()
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume.set(volume);
        self.player.borrow().set_volume(volume);
    }

    pub fn play(&self) {
        self.follow_output();
        if self.data.borrow().is_none() {
            return;
        }
        if self.player.borrow().empty() {
            self.looping.set(false);
            let _ = self.requeue_at(0.0, true);
            return;
        }
        self.started.set(true);
        self.player.borrow().play();
    }

    pub fn play_looping(&self) {
        self.follow_output();
        if let Some(data) = self.data.borrow().as_ref() {
            let player = self.player.borrow();
            if player.empty() {
                self.looping.set(true);
                match data {
                    Playable::Pcm(data) => player.append(
                        SharedSamplesSource {
                            samples: Arc::clone(&data.samples),
                            position: Arc::new(AtomicUsize::new(0)),
                            channels: data.channels,
                            sample_rate: data.sample_rate,
                            duration: data.duration,
                        }
                        .repeat_infinite(),
                    ),
                    Playable::Stream(stream) => player.append(
                        PcmStreamSource::new(Arc::clone(stream), Arc::new(AtomicUsize::new(0)))
                            .looping(),
                    ),
                }
            }
            self.started.set(true);
            player.play();
        }
    }

    pub fn pause(&self) {
        self.player.borrow().pause();
    }

    pub fn seek(&self, secs: f64) -> bool {
        let duration = self.duration_secs();
        let secs = if duration > 0.0 {
            secs.clamp(0.0, duration)
        } else {
            secs.max(0.0)
        };
        if self.data.borrow().is_none() {
            return false;
        }
        self.follow_output();
        let playing = self.is_playing();
        if !self.player.borrow().empty()
            && let Some(cursor) = self.cursor.borrow().clone()
            && self.retarget(&cursor, secs)
        {
            return true;
        }
        self.requeue_at(secs, playing)
    }

    fn retarget(&self, cursor: &AtomicUsize, secs: f64) -> bool {
        let data = self.data.borrow();
        let Some(data) = data.as_ref() else {
            return false;
        };
        match data {
            Playable::Pcm(pcm) => {
                let channels = pcm.channels.get() as usize;
                let index =
                    secs_to_index(secs, pcm.sample_rate.get(), channels).min(pcm.samples.len());
                store_aligned(cursor, index, channels, pcm.samples.len());
                true
            }
            Playable::Stream(stream) => {
                let channels = stream.channels().max(1);
                let len = stream.buffered_samples();
                let index = secs_to_index(secs, stream.sample_rate().max(1), channels);
                if !stream.is_complete() && index >= len {
                    return false;
                }
                let limit = if stream.is_complete() {
                    len
                } else {
                    len.max(1)
                };
                let aligned = align_phase(cursor.load(Ordering::Relaxed), index, channels, limit);
                if stream.locate(aligned).is_none() {
                    return false;
                }
                store_aligned(cursor, aligned, channels, limit);
                true
            }
        }
    }

    fn prepare_source(&self, secs: f64) -> Option<PreparedSource> {
        let data = self.data.borrow();
        let data = data.as_ref()?;
        let cursor = Arc::new(AtomicUsize::new(0));
        let source = match data {
            Playable::Pcm(pcm) => {
                let channels = pcm.channels.get() as usize;
                let index =
                    secs_to_index(secs, pcm.sample_rate.get(), channels).min(pcm.samples.len());
                store_aligned(&cursor, index, channels, pcm.samples.len());
                QueuedSource::Pcm(SharedSamplesSource {
                    samples: Arc::clone(&pcm.samples),
                    position: Arc::clone(&cursor),
                    channels: pcm.channels,
                    sample_rate: pcm.sample_rate,
                    duration: pcm.duration,
                })
            }
            Playable::Stream(stream) => {
                let channels = stream.channels().max(1);
                let len = stream.buffered_samples();
                let index = secs_to_index(secs, stream.sample_rate().max(1), channels);
                if index > 0 {
                    if !stream.is_complete() && index >= len {
                        return None;
                    }
                    let limit = len.max(1);
                    let aligned = align_phase(0, index, channels, limit);
                    stream.locate(aligned)?;
                    cursor.store(aligned, Ordering::Relaxed);
                }
                QueuedSource::Stream(PcmStreamSource::new(
                    Arc::clone(stream),
                    Arc::clone(&cursor),
                ))
            }
        };
        Some(PreparedSource { cursor, source })
    }

    fn requeue_at(&self, secs: f64, playing: bool) -> bool {
        let Some(prepared) = self.prepare_source(secs) else {
            return false;
        };
        let player = {
            let sink = self.sink.borrow();
            let player = Player::connect_new(sink.sink.mixer());
            player.set_volume(self.volume.get());
            if !playing {
                player.pause();
            }
            append_source(&player, prepared.source, self.looping.get());
            player
        };
        *self.player.borrow_mut() = player;
        *self.cursor.borrow_mut() = Some(prepared.cursor);
        self.started.set(true);
        true
    }

    pub fn is_playing(&self) -> bool {
        let player = self.player.borrow();
        !player.is_paused() && !player.empty()
    }

    pub fn finished(&self) -> bool {
        self.started.get() && self.player.borrow().empty()
    }

    pub fn poll_output(&self) {
        let now = Instant::now();
        if self.output_check_due(now) {
            self.last_output_check.set(Some(now));
            self.follow_output();
        }
        if self.playback_stalled(now) {
            tracing::warn!("sound playback stalled, restarting the source");
            self.restart_source();
        }
    }

    fn output_check_due(&self, now: Instant) -> bool {
        let sink = Rc::clone(&self.sink.borrow());
        let check_interval = if sink.broken.load(Ordering::Relaxed) {
            if replacement_opened(&sink) {
                return true;
            }
            OUTPUT_RETRY_INTERVAL
        } else if sink.heartbeat.stalled() {
            return true;
        } else {
            OUTPUT_CHECK_INTERVAL
        };
        self.last_output_check
            .get()
            .is_none_or(|checked| now.duration_since(checked) >= check_interval)
    }

    fn playback_stalled(&self, now: Instant) -> bool {
        if self.sink.borrow().broken.load(Ordering::Relaxed) {
            self.output_progress.borrow_mut().reset();
            return false;
        }
        let Some(cursor) = self.cursor.borrow().clone() else {
            return false;
        };
        let index = cursor.load(Ordering::Relaxed);
        let playable = match self.data.borrow().as_ref() {
            Some(Playable::Pcm(_)) => true,
            Some(Playable::Stream(stream)) => {
                index < stream.buffered_samples() || stream.is_complete()
            }
            None => false,
        };
        self.output_progress
            .borrow_mut()
            .observe(now, self.is_playing(), index, playable)
    }

    fn restart_source(&self) {
        let sink = Rc::clone(&self.sink.borrow());
        self.follow_output();
        if Rc::ptr_eq(&sink, &self.sink.borrow()) {
            self.requeue_at(self.position_secs(), true);
        }
    }

    pub fn position_secs(&self) -> f64 {
        let Some(cursor) = self.cursor.borrow().clone() else {
            return 0.0;
        };
        let index = cursor.load(Ordering::Relaxed);
        let data = self.data.borrow();
        let Some(data) = data.as_ref() else {
            return 0.0;
        };
        let (channels, rate, duration) = match data {
            Playable::Pcm(pcm) => (
                pcm.channels.get() as usize,
                pcm.sample_rate.get(),
                pcm.duration,
            ),
            Playable::Stream(stream) => (
                stream.channels().max(1),
                stream.sample_rate().max(1),
                stream.duration_secs(),
            ),
        };
        if channels == 0 || rate == 0 {
            return 0.0;
        }
        let secs = index as f64 / f64::from(rate) / channels as f64;
        if duration > 0.0 {
            secs.min(duration)
        } else {
            secs
        }
    }

    pub fn duration_secs(&self) -> f64 {
        match self.data.borrow().as_ref() {
            Some(Playable::Pcm(data)) => data.duration,
            Some(Playable::Stream(stream)) => stream.duration_secs(),
            None => 0.0,
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.player.get_mut().stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source;

    #[test]
    fn stalled_output_ignores_buffering_and_bounds_retries() {
        let start = Instant::now();
        let mut progress = OutputProgress::default();

        assert!(!progress.observe(start, true, 0, false));
        assert!(!progress.observe(start + Duration::from_secs(10), true, 0, false));
        assert!(!progress.observe(start + Duration::from_secs(10), true, 0, true));
        assert!(!progress.observe(start + Duration::from_secs(12), true, 0, true));
        assert!(progress.observe(start + Duration::from_secs(13), true, 0, true));

        assert!(!progress.observe(start + Duration::from_secs(14), true, 0, true));
        assert!(!progress.observe(start + Duration::from_secs(18), true, 0, true));
        assert!(!progress.observe(start + Duration::from_secs(29), false, 0, true));
        assert!(!progress.observe(start + Duration::from_secs(30), true, 0, true));
        assert!(!progress.observe(start + Duration::from_secs(32), true, 0, true));
        assert!(progress.observe(start + Duration::from_secs(33), true, 0, true));
    }

    #[test]
    fn a_silent_output_is_stalled_only_after_its_grace_windows() {
        assert!(!output_stalled(Duration::from_secs(4), NO_HEARTBEAT));
        assert!(output_stalled(HEARTBEAT_STARTUP_GRACE, NO_HEARTBEAT));
        assert!(!output_stalled(Duration::from_secs(60), 58_500));
        assert!(output_stalled(Duration::from_secs(60), 58_000));
        assert!(output_stalled(Duration::from_secs(9 * 3600), 1_000));
    }

    #[test]
    fn heartbeat_beats_while_the_mixer_pulls_audio() {
        let channels = NonZeroU16::new(2).unwrap();
        let sample_rate = NonZeroU32::new(48_000).unwrap();
        let (mixer, mut output) = rodio::mixer::mixer(channels, sample_rate);
        let last_beat_ms = Arc::new(AtomicU64::new(NO_HEARTBEAT));
        mixer.add(HeartbeatSource::new(
            Instant::now(),
            Arc::clone(&last_beat_ms),
            channels,
            sample_rate,
        ));

        assert_eq!(last_beat_ms.load(Ordering::Relaxed), NO_HEARTBEAT);
        let pulled: Vec<f32> = (&mut output).take(48_000 * 2).collect();
        assert_eq!(pulled.len(), 48_000 * 2);
        assert!(pulled.iter().all(|sample| *sample == 0.0));
        assert_ne!(last_beat_ms.load(Ordering::Relaxed), NO_HEARTBEAT);
        assert!(output.next().is_some());
    }

    #[test]
    fn stream_source_pads_underruns_to_whole_frames() {
        let stream = Arc::new(PcmStream::new(2, 48_000, None));
        stream.push(&[1.0, -1.0, 2.0, -2.0]);
        let mut source = PcmStreamSource::new(Arc::clone(&stream), Arc::new(AtomicUsize::new(0)));

        assert_eq!(
            (&mut source).take(4).collect::<Vec<_>>(),
            vec![1.0, -1.0, 2.0, -2.0]
        );

        let starved: Vec<f32> = (&mut source).take(3).collect();
        assert_eq!(starved, vec![0.0, 0.0, 0.0]);

        stream.push(&[3.0, -3.0]);
        let resumed: Vec<f32> = (&mut source).take(3).collect();
        assert_eq!(resumed, vec![0.0, 3.0, -3.0]);

        stream.finish();
        assert_eq!(source.next(), None);
        assert_eq!(source.current_span_len(), Some(0));
    }

    #[test]
    fn stream_source_seeks_into_a_later_chunk() {
        let stream = Arc::new(PcmStream::new(1, 8, Some(1.0)));
        stream.push(&[0.0, 1.0, 2.0, 3.0]);
        stream.push(&[4.0, 5.0, 6.0, 7.0]);
        stream.finish();
        let mut source = PcmStreamSource::new(stream, Arc::new(AtomicUsize::new(0)));
        source
            .try_seek(Duration::from_secs_f64(0.5))
            .expect("buffered audio can seek");
        assert_eq!(source.next(), Some(4.0));
    }

    #[test]
    fn stream_source_rejects_a_seek_past_the_buffer() {
        let stream = Arc::new(PcmStream::new(1, 8, Some(2.0)));
        stream.push(&[0.0, 1.0, 2.0, 3.0]);
        let mut source = PcmStreamSource::new(stream, Arc::new(AtomicUsize::new(0)));
        assert!(source.try_seek(Duration::from_secs_f64(1.0)).is_err());
        assert_eq!(source.next(), Some(0.0));
    }

    #[test]
    fn stereo_seek_keeps_the_channel_phase() {
        let samples: Arc<[f32]> = (0..32).map(|index| index as f32).collect::<Vec<_>>().into();
        let position = Arc::new(AtomicUsize::new(1));
        let mut source = SharedSamplesSource {
            samples,
            position: Arc::clone(&position),
            channels: NonZeroU16::new(2).unwrap(),
            sample_rate: NonZeroU32::new(8).unwrap(),
            duration: 2.0,
        };
        source
            .try_seek(Duration::from_secs_f64(1.0))
            .expect("pcm can seek");
        assert_eq!(position.load(Ordering::Relaxed) % 2, 1);
        assert_eq!(source.next(), Some(15.0));
    }

    #[test]
    fn stream_source_ends_only_when_the_stream_completes() {
        let stream = Arc::new(PcmStream::new(1, 48_000, None));
        stream.push(&[0.5]);
        let mut source = PcmStreamSource::new(Arc::clone(&stream), Arc::new(AtomicUsize::new(0)));

        assert_eq!(source.next(), Some(0.5));
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.current_span_len(), Some(STREAM_SPAN));

        stream.finish();
        assert_eq!(source.next(), None);
    }
}
