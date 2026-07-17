use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use symphonia::core::io::{MediaSource, MediaSourceStream};

use crate::AudioError;
use crate::decode::{PcmSpec, decode_packets};

const PREROLL_SECS: f64 = 0.5;
const CHUNK_FRAMES: usize = 2048;

pub(crate) enum ChunkState {
    Ready(Arc<[f32]>),
    Pending,
    Complete,
}

struct StreamBuffer {
    chunks: Vec<Arc<[f32]>>,
    complete: bool,
}

pub struct PcmStream {
    buffer: Mutex<StreamBuffer>,
    ready_samples: AtomicUsize,
    channels: usize,
    sample_rate: u32,
    duration_hint: Option<f64>,
}

impl PcmStream {
    pub(crate) fn new(channels: usize, sample_rate: u32, duration_hint: Option<f64>) -> Self {
        Self {
            buffer: Mutex::new(StreamBuffer {
                chunks: Vec::new(),
                complete: false,
            }),
            ready_samples: AtomicUsize::new(0),
            channels: channels.max(1),
            sample_rate: sample_rate.max(1),
            duration_hint,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn duration_secs(&self) -> f64 {
        if !self.buffer.lock().complete {
            return self.duration_hint.unwrap_or(0.0);
        }
        let frames = self.ready_samples.load(Ordering::Acquire) / self.channels;
        frames as f64 / self.sample_rate as f64
    }

    pub(crate) fn chunk_at(&self, index: usize) -> ChunkState {
        let buffer = self.buffer.lock();
        match buffer.chunks.get(index) {
            Some(chunk) => ChunkState::Ready(Arc::clone(chunk)),
            None if buffer.complete => ChunkState::Complete,
            None => ChunkState::Pending,
        }
    }

    pub(crate) fn push(&self, samples: &[f32]) {
        self.buffer.lock().chunks.push(Arc::from(samples));
        self.ready_samples
            .fetch_add(samples.len(), Ordering::Release);
    }

    pub(crate) fn finish(&self) {
        self.buffer.lock().complete = true;
    }

    fn ready_frames(&self) -> usize {
        self.ready_samples.load(Ordering::Acquire) / self.channels
    }

    fn preroll_frames(&self) -> usize {
        (PREROLL_SECS * self.sample_rate as f64) as usize
    }
}

pub fn spawn_stream_decode(
    bytes: flume::Receiver<Vec<u8>>,
) -> flume::Receiver<Result<Arc<PcmStream>, AudioError>> {
    let (ready_tx, ready_rx) = flume::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("mezon-audio-decode".into())
        .spawn(move || decode_thread(bytes, &ready_tx));
    if let Err(err) = spawned {
        return failed_receiver(AudioError::Decode(format!(
            "failed to start audio decoder: {err}"
        )));
    }
    ready_rx
}

fn failed_receiver(err: AudioError) -> flume::Receiver<Result<Arc<PcmStream>, AudioError>> {
    let (tx, rx) = flume::bounded(1);
    let _ = tx.send(Err(err));
    rx
}

fn decode_thread(
    bytes: flume::Receiver<Vec<u8>>,
    ready_tx: &flume::Sender<Result<Arc<PcmStream>, AudioError>>,
) {
    let source = ChunkReader::new(bytes);
    let mss = MediaSourceStream::new(Box::new(source), Default::default());

    let mut stream: Option<Arc<PcmStream>> = None;
    let mut pending: Vec<f32> = Vec::new();
    let mut announced = false;
    let duration_hint = std::cell::Cell::new(None);

    let result = decode_packets(
        mss,
        |duration| duration_hint.set(duration),
        |samples, spec| {
            let stream = stream.get_or_insert_with(|| {
                Arc::new(PcmStream::new(
                    spec.channels.min(2),
                    spec.sample_rate,
                    duration_hint.get(),
                ))
            });
            append_downmixed(&mut pending, samples, spec, stream.channels());
            if pending.len() >= CHUNK_FRAMES * stream.channels() {
                stream.push(&pending);
                pending.clear();
            }
            if !announced && stream.ready_frames() >= stream.preroll_frames() {
                announced = true;
                let _ = ready_tx.send(Ok(Arc::clone(stream)));
            }
        },
    );

    let Some(stream) = stream else {
        let err = result
            .err()
            .unwrap_or_else(|| AudioError::Decode("audio stream produced no samples".into()));
        let _ = ready_tx.send(Err(err));
        return;
    };

    if !pending.is_empty() {
        stream.push(&pending);
    }
    stream.finish();

    if let Err(err) = result {
        tracing::warn!("audio stream decode ended with error: {err}");
    }
    if !announced {
        let _ = ready_tx.send(Ok(stream));
    }
}

fn append_downmixed(out: &mut Vec<f32>, samples: &[f32], spec: PcmSpec, out_channels: usize) {
    if spec.channels == out_channels {
        out.extend_from_slice(samples);
        return;
    }
    for frame in samples.chunks_exact(spec.channels) {
        out.push(frame.iter().sum::<f32>() / spec.channels as f32);
    }
}

struct ChunkReader {
    bytes: flume::Receiver<Vec<u8>>,
    current: Vec<u8>,
    offset: usize,
    ended: bool,
}

impl ChunkReader {
    fn new(bytes: flume::Receiver<Vec<u8>>) -> Self {
        Self {
            bytes,
            current: Vec::new(),
            offset: 0,
            ended: false,
        }
    }
}

impl Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let available = self.current.len() - self.offset;
            if available > 0 {
                let n = available.min(buf.len());
                buf[..n].copy_from_slice(&self.current[self.offset..self.offset + n]);
                self.offset += n;
                return Ok(n);
            }
            if self.ended {
                return Ok(0);
            }
            match self.bytes.recv() {
                Ok(chunk) => {
                    self.current = chunk;
                    self.offset = 0;
                }
                Err(_) => {
                    self.ended = true;
                    return Ok(0);
                }
            }
        }
    }
}

impl Seek for ChunkReader {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "audio stream is not seekable",
        ))
    }
}

impl MediaSource for ChunkReader {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const RATE: u32 = 48_000;

    pub(crate) fn wav_sine(seconds: f64) -> Vec<u8> {
        let frames = (RATE as f64 * seconds) as u32;
        let data_len = frames * 2;
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for frame in 0..frames {
            let phase = frame as f64 / RATE as f64 * 440.0 * std::f64::consts::TAU;
            let sample = (phase.sin() * 16_384.0) as i16;
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }

    fn drain(stream: &Arc<PcmStream>) -> Vec<f32> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut samples = Vec::new();
        let mut index = 0;
        loop {
            match stream.chunk_at(index) {
                ChunkState::Ready(chunk) => {
                    samples.extend_from_slice(&chunk);
                    index += 1;
                }
                ChunkState::Complete => return samples,
                ChunkState::Pending => {
                    assert!(Instant::now() < deadline, "stream never completed");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    #[test]
    fn streamed_pcm_matches_buffered_decode() {
        let bytes = wav_sine(2.0);
        let (tx, rx) = flume::bounded(4);
        let ready = spawn_stream_decode(rx);
        for chunk in bytes.chunks(4096) {
            tx.send(chunk.to_vec()).expect("decoder alive");
        }
        drop(tx);

        let stream = ready
            .recv_timeout(Duration::from_secs(10))
            .expect("ready signal")
            .expect("stream decodes");
        let streamed = drain(&stream);
        let buffered = crate::decode_audio(bytes).expect("buffered decode");

        assert_eq!(stream.channels(), buffered.channels);
        assert_eq!(stream.sample_rate(), buffered.sample_rate);
        assert_eq!(streamed.len(), buffered.samples.len());
        assert!(
            streamed
                .iter()
                .zip(buffered.samples.iter())
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
        assert!((stream.duration_secs() - 2.0).abs() < 0.01);
    }

    #[test]
    fn total_duration_is_known_before_the_stream_completes() {
        let bytes = wav_sine(20.0);
        let (tx, rx) = flume::bounded(4);
        let ready = spawn_stream_decode(rx);

        let preroll_bytes = 44 + RATE as usize * 2;
        let mut sent = 0;
        for chunk in bytes.chunks(8192) {
            if sent >= preroll_bytes {
                break;
            }
            tx.send(chunk.to_vec()).expect("decoder alive");
            sent += chunk.len();
        }

        let stream = ready
            .recv_timeout(Duration::from_secs(10))
            .expect("ready")
            .expect("stream decodes");

        assert!(!matches!(stream.chunk_at(usize::MAX), ChunkState::Complete));
        assert!(
            (stream.duration_secs() - 20.0).abs() < 0.1,
            "expected the container duration up front, got {}",
            stream.duration_secs()
        );
        drop(tx);
    }

    #[test]
    fn playback_starts_before_the_whole_file_arrives() {
        let bytes = wav_sine(20.0);
        let (tx, rx) = flume::bounded(4);
        let ready = spawn_stream_decode(rx);

        let preroll_bytes = 44 + RATE as usize * 2;
        let mut sent = 0;
        for chunk in bytes.chunks(8192) {
            if sent >= preroll_bytes {
                break;
            }
            tx.send(chunk.to_vec()).expect("decoder alive");
            sent += chunk.len();
        }

        let stream = ready
            .recv_timeout(Duration::from_secs(10))
            .expect("ready before the download finished")
            .expect("stream decodes");

        assert!(sent * 10 < bytes.len(), "fed {sent} of {}", bytes.len());
        assert!(matches!(stream.chunk_at(0), ChunkState::Ready(_)));
        drop(tx);
    }
}
