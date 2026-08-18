use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::compose::{DrawTile, Renderer, Scene, SourceImage, TileShape, accent_for};
use crate::video::{VideoFrameData, VideoFrameStore};
use mezon_record::{
    AudioSource, AudioTap, PixelData, RecordStats, Recorder, RecorderConfig, VideoConfig,
    VideoFrameRef, VideoTap,
};
use parking_lot::RwLock;

pub const RECORD_WIDTH: u32 = 1280;
pub const RECORD_HEIGHT: u32 = 720;
pub const RECORD_FPS: u32 = 30;

#[derive(Clone, Default)]
pub struct RecordTaps {
    slot: Arc<RwLock<Option<AudioTap>>>,
}

impl RecordTaps {
    pub fn set(&self, tap: Option<AudioTap>) {
        *self.slot.write() = tap;
    }

    pub fn push(&self, source: AudioSource, samples: &[i16], rate: u32, channels: u32) {
        let Some(guard) = self.slot.try_read() else {
            return;
        };
        if let Some(tap) = guard.as_ref() {
            tap.push(source, samples, rate, channels);
        }
    }
}

#[derive(Clone)]
pub struct RecordStarter {
    taps: RecordTaps,
    slot: Arc<RwLock<Option<RecordSession>>>,
}

impl RecordStarter {
    pub fn new(taps: RecordTaps, slot: Arc<RwLock<Option<RecordSession>>>) -> Self {
        Self { taps, slot }
    }

    pub fn start(
        &self,
        path: PathBuf,
        scene: Option<(Scene, Arc<VideoFrameStore>)>,
    ) -> Result<(), String> {
        if self.slot.read().is_some() {
            return Err("a recording is already running".into());
        }
        let session = RecordSession::start(path, self.taps.clone(), scene)?;
        *self.slot.write() = Some(session);
        Ok(())
    }
}

pub struct RecordSession {
    recorder: Option<Recorder>,
    taps: RecordTaps,
    stop: Arc<AtomicBool>,
    video_unavailable: Arc<AtomicBool>,
    frames: Option<Arc<VideoFrameStore>>,
    pump: Option<JoinHandle<()>>,
}

impl RecordSession {
    pub fn start(
        path: PathBuf,
        taps: RecordTaps,
        scene: Option<(Scene, Arc<VideoFrameStore>)>,
    ) -> Result<Self, String> {
        let video = scene.as_ref().map(|_| VideoConfig {
            width: RECORD_WIDTH,
            height: RECORD_HEIGHT,
            fps: RECORD_FPS,
        });
        let recorder =
            Recorder::start(RecorderConfig { path, video }).map_err(|error| error.to_string())?;
        taps.set(Some(recorder.audio_tap()));

        let stop = Arc::new(AtomicBool::new(false));
        let video_unavailable = Arc::new(AtomicBool::new(false));
        let mut recorded_frames = None;
        let pump = match scene {
            Some((scene, frames)) => {
                frames.set_recording(true);
                recorded_frames = Some(frames.clone());
                let pump_recorder = recorder.video_tap();
                let pump_stop = stop.clone();
                let pump_failed = video_unavailable.clone();
                match std::thread::Builder::new()
                    .name("mezon-record-compose".into())
                    .spawn(move || compose_pump(scene, frames, pump_recorder, pump_stop))
                {
                    Ok(handle) => Some(handle),
                    Err(error) => {
                        tracing::error!("could not start the call recording compositor: {error}");
                        pump_failed.store(true, Ordering::Relaxed);
                        None
                    }
                }
            }
            None => {
                video_unavailable.store(true, Ordering::Relaxed);
                None
            }
        };

        Ok(Self {
            recorder: Some(recorder),
            taps,
            stop,
            video_unavailable,
            frames: recorded_frames,
            pump,
        })
    }

    pub fn stats(&self) -> RecordStats {
        self.recorder
            .as_ref()
            .map(|recorder| recorder.stats())
            .unwrap_or_default()
    }

    pub fn video_unavailable(&self) -> bool {
        self.video_unavailable.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> bool {
        self.recorder
            .as_ref()
            .is_some_and(|recorder| recorder.failed())
    }

    pub fn finish(mut self) -> Result<PathBuf, String> {
        self.taps.set(None);
        if let Some(frames) = self.frames.take() {
            frames.set_recording(false);
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        let Some(recorder) = self.recorder.take() else {
            return Err("the recording was already stopped".into());
        };
        recorder.finish().map_err(|error| error.to_string())
    }
}

impl Drop for RecordSession {
    fn drop(&mut self) {
        self.taps.set(None);
        if let Some(frames) = self.frames.take() {
            frames.set_recording(false);
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

fn compose_pump(
    scene: Scene,
    frames: Arc<VideoFrameStore>,
    recorder: VideoTap,
    stop: Arc<AtomicBool>,
) {
    let Some(mut renderer) = Renderer::new(RECORD_WIDTH, RECORD_HEIGHT) else {
        tracing::error!("could not allocate the call recording compositor");
        return;
    };
    let interval = Duration::from_secs_f64(1.0 / RECORD_FPS as f64);

    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let tiles = scene.snapshot();
        let sources: Vec<Option<Arc<VideoFrameData>>> = tiles
            .iter()
            .map(|tile| tile.frame_key.and_then(|key| frames.recorded(key)))
            .collect();

        let draw: Vec<DrawTile<'_>> = tiles
            .iter()
            .zip(sources.iter())
            .map(|(tile, source)| DrawTile {
                image: source.as_ref().and_then(|frame| {
                    (!frame.bgra.is_empty()).then_some(SourceImage {
                        bgra: &frame.bgra,
                        width: frame.width,
                        height: frame.height,
                    })
                }),
                avatar: tile.avatar.as_ref().map(|avatar| SourceImage {
                    bgra: &avatar.bgra,
                    width: avatar.width,
                    height: avatar.height,
                }),
                label: tile.label.as_str(),
                initial: tile.initial.as_str(),
                accent: accent_for(&tile.key),
                shape: TileShape {
                    focused: tile.focused,
                    contain: tile.is_screen_share,
                },
                speaking: tile.speaking,
            })
            .collect();

        recorder.push(VideoFrameRef {
            width: RECORD_WIDTH,
            height: RECORD_HEIGHT,
            data: PixelData::Bgra {
                data: renderer.render(&draw),
                stride: RECORD_WIDTH as usize * 4,
            },
        });

        if let Some(rest) = interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}
