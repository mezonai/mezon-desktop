use std::cell::{Cell, RefCell};
use std::num::{NonZeroU16, NonZeroU32};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::sync::Arc;
use std::time::Duration;

use crate::AudioError;
use crate::decode::DecodedPcm;
use crate::stream::{ChunkState, PcmStream};

const STREAM_SPAN: usize = 32_768;

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

fn open_on(device: cpal::Device, broken: Arc<AtomicBool>) -> Result<MixerDeviceSink, String> {
    DeviceSinkBuilder::from_device(device)
        .map_err(|e| e.to_string())?
        .with_error_callback(move |err| {
            if !broken.swap(true, Ordering::Relaxed) {
                tracing::warn!("sound output stream error: {err}");
            }
        })
        .open_sink_or_fallback()
        .map_err(|e| e.to_string())
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
    let broken = Arc::new(AtomicBool::new(false));
    let opened = match device {
        Some(device) => open_on(device, broken.clone()),
        None => Err("no output device".to_string()),
    };
    let mut sink = match opened {
        Ok(sink) => sink,
        Err(e) => {
            tracing::warn!(
                requested = %requested,
                device = ?device_name,
                "sound output open failed, falling back to any output: {e}"
            );
            DeviceSinkBuilder::open_default_sink().map_err(|e| {
                tracing::warn!("sound output fallback failed: {e}");
                AudioError::Output(e.to_string())
            })?
        }
    };
    sink.log_on_drop(false);
    tracing::info!(
        requested = %requested,
        device = ?device_name,
        sample_rate = ?sink.config().sample_rate(),
        channels = ?sink.config().channel_count(),
        "sound output opened"
    );
    Ok(SharedSink {
        sink,
        requested,
        broken,
    })
}

fn shared_sink() -> Result<Rc<SharedSink>, AudioError> {
    let host = cpal::default_host();
    let requested = requested_output(&host);
    SHARED_SINK.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(existing) = slot.upgrade()
            && existing.requested == requested
            && !existing.broken.load(Ordering::Relaxed)
        {
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
    position: usize,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    duration: f64,
}

impl Iterator for SharedSamplesSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.samples.get(self.position).copied();
        if sample.is_some() {
            self.position += 1;
        }
        sample
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.samples.len().saturating_sub(self.position);
        (remaining, Some(remaining))
    }
}

impl Source for SharedSamplesSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.position >= self.samples.len() {
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
        let sample = (pos.as_secs_f64()
            * self.sample_rate.get() as f64
            * self.channels.get() as f64) as usize;
        let sample = sample.min(self.samples.len());
        self.position = sample - sample % channels;
        Ok(())
    }
}

struct PcmStreamSource {
    stream: Arc<PcmStream>,
    chunk: Option<Arc<[f32]>>,
    next_chunk: usize,
    offset: usize,
    silence_debt: usize,
    exhausted: bool,
    looping: bool,
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
}

impl PcmStreamSource {
    fn new(stream: Arc<PcmStream>) -> Self {
        let channels =
            NonZeroU16::new(stream.channels().clamp(1, 2) as u16).unwrap_or(NonZeroU16::MIN);
        let sample_rate =
            NonZeroU32::new(stream.sample_rate()).unwrap_or(NonZeroU32::new(48_000).unwrap());
        Self {
            stream,
            chunk: None,
            next_chunk: 0,
            offset: 0,
            silence_debt: 0,
            exhausted: false,
            looping: false,
            channels,
            sample_rate,
        }
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
        loop {
            if let Some(chunk) = &self.chunk
                && let Some(sample) = chunk.get(self.offset).copied()
            {
                self.offset += 1;
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

    fn try_seek(&mut self, _: Duration) -> Result<(), rodio::source::SeekError> {
        Err(rodio::source::SeekError::NotSupported {
            underlying_source: "PcmStreamSource",
        })
    }
}

enum Playable {
    Pcm(PcmData),
    Stream(Arc<PcmStream>),
}

pub struct AudioPlayer {
    sink: RefCell<Rc<SharedSink>>,
    player: RefCell<Player>,
    volume: Cell<f32>,
    data: RefCell<Option<Playable>>,
    started: Cell<bool>,
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
            started: Cell::new(false),
        })
    }

    fn follow_output(&self) {
        if !self.player.borrow().empty() {
            return;
        }
        let Ok(sink) = shared_sink() else {
            return;
        };
        let unchanged = Rc::ptr_eq(&sink, &self.sink.borrow());
        if unchanged {
            return;
        }
        let player = Player::connect_new(sink.sink.mixer());
        player.set_volume(self.volume.get());
        *self.player.borrow_mut() = player;
        *self.sink.borrow_mut() = sink;
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
    }

    pub fn set_stream(&self, stream: Arc<PcmStream>) {
        *self.data.borrow_mut() = Some(Playable::Stream(stream));
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
        if let Some(data) = self.data.borrow().as_ref() {
            let player = self.player.borrow();
            if player.empty() {
                match data {
                    Playable::Pcm(data) => player.append(SharedSamplesSource {
                        samples: Arc::clone(&data.samples),
                        position: 0,
                        channels: data.channels,
                        sample_rate: data.sample_rate,
                        duration: data.duration,
                    }),
                    Playable::Stream(stream) => {
                        player.append(PcmStreamSource::new(Arc::clone(stream)))
                    }
                }
            }
            self.started.set(true);
            player.play();
        }
    }

    pub fn play_looping(&self) {
        self.follow_output();
        if let Some(data) = self.data.borrow().as_ref() {
            let player = self.player.borrow();
            if player.empty() {
                match data {
                    Playable::Pcm(data) => player.append(
                        SharedSamplesSource {
                            samples: Arc::clone(&data.samples),
                            position: 0,
                            channels: data.channels,
                            sample_rate: data.sample_rate,
                            duration: data.duration,
                        }
                        .repeat_infinite(),
                    ),
                    Playable::Stream(stream) => {
                        player.append(PcmStreamSource::new(Arc::clone(stream)).looping())
                    }
                }
            }
            self.started.set(true);
            player.play();
        }
    }

    pub fn pause(&self) {
        self.player.borrow().pause();
    }

    pub fn is_playing(&self) -> bool {
        let player = self.player.borrow();
        !player.is_paused() && !player.empty()
    }

    pub fn finished(&self) -> bool {
        self.started.get() && self.player.borrow().empty()
    }

    pub fn position_secs(&self) -> f64 {
        self.player.borrow().get_pos().as_secs_f64()
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

    #[test]
    fn stream_source_pads_underruns_to_whole_frames() {
        let stream = Arc::new(PcmStream::new(2, 48_000, None));
        stream.push(&[1.0, -1.0, 2.0, -2.0]);
        let mut source = PcmStreamSource::new(Arc::clone(&stream));

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
    fn stream_source_ends_only_when_the_stream_completes() {
        let stream = Arc::new(PcmStream::new(1, 48_000, None));
        stream.push(&[0.5]);
        let mut source = PcmStreamSource::new(Arc::clone(&stream));

        assert_eq!(source.next(), Some(0.5));
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.current_span_len(), Some(STREAM_SPAN));

        stream.finish();
        assert_eq!(source.next(), None);
    }
}
