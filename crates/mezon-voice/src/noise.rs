use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use mezon_ns::{Mezon48k, MezonNSConfig, MezonNSEngine};

use crate::VoiceEvent;

const WEB_SUPPRESSION_INTENSITY: f32 = 1.6;
const MODEL_INPUT_TARGET_DBFS: f32 = -20.0;
const MAX_ATTENUATION_DB: f32 = 15.0;

enum Setting {
    Enabled { enabled: bool, generation: u64 },
    Reset,
}

pub(super) struct FilteredFrame {
    pub samples: Vec<i16>,
    pub generation: u64,
}

pub(super) struct NoiseProcessor {
    requested: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    ready: Arc<AtomicBool>,
    settings_tx: flume::Sender<Setting>,
    frames_tx: flume::Sender<Vec<i16>>,
    pub output_rx: flume::Receiver<FilteredFrame>,
}

impl NoiseProcessor {
    pub fn start(events: flume::Sender<VoiceEvent>) -> Self {
        let requested = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicBool::new(false));
        let (settings_tx, settings_rx) = flume::unbounded();
        // A short cushion for OS scheduling bursts without adding unbounded voice latency.
        let (frames_tx, frames_rx) = flume::bounded(8);
        let (output_tx, output_rx) = flume::bounded(8);
        let stale_output_rx = output_rx.clone();
        let current_requested = requested.clone();
        let current_generation = generation.clone();
        let current_ready = ready.clone();
        std::thread::Builder::new()
            .name("mezon-ns".into())
            .spawn(move || {
                enum Event {
                    Setting(Setting),
                    Frame(Vec<i16>),
                    Stop,
                }
                let mut filter: Option<Mezon48k> = None;
                let mut enabled = false;
                let mut active_generation = 0;
                let mut output_overruns = 0u64;
                let mut output_window_start = Instant::now();
                let mut output_window_drops = 0u32;
                loop {
                    let event = flume::Selector::new()
                        .recv(&settings_rx, |r| {
                            r.map(Event::Setting).unwrap_or(Event::Stop)
                        })
                        .recv(&frames_rx, |r| r.map(Event::Frame).unwrap_or(Event::Stop))
                        .wait();
                    match event {
                        Event::Setting(Setting::Enabled {
                            enabled: next,
                            generation,
                        }) => {
                            enabled = next;
                            active_generation = generation;
                            output_overruns = 0;
                            output_window_start = Instant::now();
                            output_window_drops = 0;
                            current_ready.store(false, Ordering::Release);
                            if !enabled {
                                if let Some(filter) = &mut filter {
                                    filter.reset();
                                }
                                while frames_rx.try_recv().is_ok() {}
                                while stale_output_rx.try_recv().is_ok() {}
                                let _ = events.send(VoiceEvent::NoiseSuppressionReady {
                                    generation,
                                    result: Ok(()),
                                });
                                continue;
                            }
                            let result = if let Some(filter) = &mut filter {
                                filter.reset();
                                filter.set_suppression_intensity(WEB_SUPPRESSION_INTENSITY);
                                Ok(())
                            } else {
                                let mut config = MezonNSConfig::default();
                                config.suppression_intensity = WEB_SUPPRESSION_INTENSITY;
                                config.enable_noise_gate = 0;
                                config.attenuation_limit_db = MAX_ATTENUATION_DB;
                                MezonNSEngine::create_embedded(Some(config))
                                    .and_then(|mut engine| {
                                        let input = [0i16; mezon_ns::FRAME_SIZE];
                                        let mut output = [0i16; mezon_ns::FRAME_SIZE];
                                        engine.process_frame_int16(&input, &mut output)?;
                                        engine.reset();
                                        engine.set_model_input_target_dbfs(MODEL_INPUT_TARGET_DBFS);
                                        filter = Some(Mezon48k::new(engine));
                                        Ok(())
                                    })
                                    .map_err(|e| e.to_string())
                            };
                            if result.is_err() {
                                enabled = false;
                            }
                            if result.is_ok()
                                && current_generation.load(Ordering::Acquire) == generation
                            {
                                current_ready.store(true, Ordering::Release);
                            }
                            let _ = events
                                .send(VoiceEvent::NoiseSuppressionReady { generation, result });
                        }
                        Event::Setting(Setting::Reset) => {
                            if let Some(filter) = &mut filter {
                                filter.reset();
                            }
                            while frames_rx.try_recv().is_ok() {}
                            while stale_output_rx.try_recv().is_ok() {}
                        }
                        Event::Frame(mut samples) => {
                            if !enabled
                                || current_generation.load(Ordering::Acquire) != active_generation
                            {
                                continue;
                            }
                            let Some(filter) = &mut filter else { continue };
                            let mut failed = None;
                            for chunk in samples.chunks_mut(mezon_ns::FRAME_SIZE_48K) {
                                let mut block = [0i16; mezon_ns::FRAME_SIZE_48K];
                                block[..chunk.len()].copy_from_slice(chunk);
                                if let Err(e) = filter.process_frame(&mut block) {
                                    failed = Some(e.to_string());
                                    break;
                                }
                                chunk.copy_from_slice(&block[..chunk.len()]);
                            }
                            if let Some(error) = failed {
                                enabled = false;
                                current_ready.store(false, Ordering::Release);
                                let _ = events.send(VoiceEvent::NoiseSuppressionReady {
                                    generation: active_generation,
                                    result: Err(error),
                                });
                                continue;
                            }
                            match output_tx.try_send(FilteredFrame {
                                samples,
                                generation: active_generation,
                            }) {
                                Ok(()) => {}
                                Err(_) => {
                                    output_overruns += 1;
                                    if output_window_start.elapsed() >= std::time::Duration::from_secs(2) {
                                        output_window_start = Instant::now();
                                        output_window_drops = 0;
                                    }
                                    output_window_drops += 1;
                                    if output_overruns == 1 || output_overruns % 20 == 0 {
                                        tracing::warn!(
                                            generation = active_generation,
                                            output_overruns,
                                            drops_in_window = output_window_drops,
                                            queued_outputs = output_tx.len(),
                                            "Mezon-NS output queue full; filtered frame skipped"
                                        );
                                    }
                                    if output_window_drops >= 12 {
                                        enabled = false;
                                        current_ready.store(false, Ordering::Release);
                                        current_requested.store(false, Ordering::Release);
                                        tracing::error!(
                                            generation = active_generation,
                                            "Mezon-NS disabled after sustained filtered output loss"
                                        );
                                        let _ = events.send(VoiceEvent::NoiseSuppressionReady {
                                            generation: active_generation,
                                            result: Err("Noise filter output could not keep up with live audio".into()),
                                        });
                                    }
                                }
                            }
                        }
                        Event::Stop => break,
                    }
                }
            })
            .expect("spawn Mezon-NS audio worker");
        Self {
            requested,
            generation,
            ready,
            settings_tx,
            frames_tx,
            output_rx,
        }
    }

    pub fn set_enabled(&self, enabled: bool, generation: u64) {
        self.generation.store(generation, Ordering::Release);
        self.requested.store(enabled, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        let _ = self.settings_tx.send(Setting::Enabled {
            enabled,
            generation,
        });
    }

    pub fn reset(&self) {
        let _ = self.settings_tx.send(Setting::Reset);
    }

    pub fn requested(&self) -> Arc<AtomicBool> {
        self.requested.clone()
    }
    pub fn generation(&self) -> Arc<AtomicU64> {
        self.generation.clone()
    }
    pub fn ready(&self) -> Arc<AtomicBool> {
        self.ready.clone()
    }
    pub fn frames(&self) -> flume::Sender<Vec<i16>> {
        self.frames_tx.clone()
    }
}
