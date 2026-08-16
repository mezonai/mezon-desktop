use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use livekit::webrtc::native::apm::AudioProcessingModule;
use parking_lot::Mutex;

struct CaptureChunk {
    data: Vec<i16>,
    rate: i32,
    channels: i32,
    delay_ms: i32,
}

struct ReverseChunk {
    data: Vec<i16>,
    rate: i32,
    channels: i32,
}

struct WorkerExitSignal(flume::Sender<()>);

impl Drop for WorkerExitSignal {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

fn flush_capture(
    acc: &mut Vec<i16>,
    frame: usize,
    rate: i32,
    channels: i32,
    delay_ms: i32,
    tx: &flume::Sender<CaptureChunk>,
) {
    if frame == 0 {
        return;
    }
    while acc.len() >= frame {
        let data: Vec<i16> = acc.drain(..frame).collect();
        let _ = tx.try_send(CaptureChunk {
            data,
            rate,
            channels,
            delay_ms,
        });
    }
}

fn update_out_latency(info: &cpal::OutputCallbackInfo, out_latency_ms: &AtomicU32) {
    let ts = info.timestamp();
    if let Some(d) = ts.playback.duration_since(&ts.callback) {
        out_latency_ms.store(d.as_millis().min(500) as u32, Ordering::Relaxed);
    }
}

fn capture_delay_ms(info: &cpal::InputCallbackInfo, out_latency_ms: &AtomicU32) -> i32 {
    let ts = info.timestamp();
    let in_latency = ts
        .callback
        .duration_since(&ts.capture)
        .map(|d| d.as_millis().min(500) as u32)
        .unwrap_or(0);
    (out_latency_ms.load(Ordering::Relaxed) + in_latency).min(500) as i32
}

#[derive(Clone)]
struct CallbackHeartbeat {
    epoch: Instant,
    last_callback_ms: Arc<AtomicU64>,
}

impl CallbackHeartbeat {
    fn new() -> Self {
        let heartbeat = Self {
            epoch: Instant::now(),
            last_callback_ms: Arc::new(AtomicU64::new(0)),
        };
        heartbeat.mark();
        heartbeat
    }

    fn mark(&self) {
        self.last_callback_ms
            .store(self.now_ms(), Ordering::Relaxed);
    }

    fn tick(&self) -> u64 {
        self.last_callback_ms.load(Ordering::Relaxed)
    }

    fn age(&self) -> Duration {
        Duration::from_millis(self.now_ms().saturating_sub(self.tick()))
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
}

enum StallAction {
    Recover(u32),
    Exhausted,
}

struct StallRecovery {
    observed_tick: u64,
    attempts: u32,
    last_attempt: Option<Instant>,
    exhausted_reported: bool,
}

impl StallRecovery {
    fn new(heartbeat: &CallbackHeartbeat) -> Self {
        Self {
            observed_tick: heartbeat.tick(),
            attempts: 0,
            last_attempt: None,
            exhausted_reported: false,
        }
    }

    fn reset(&mut self, heartbeat: &CallbackHeartbeat) {
        heartbeat.mark();
        self.observed_tick = heartbeat.tick();
        self.attempts = 0;
        self.last_attempt = None;
        self.exhausted_reported = false;
    }

    fn poll(&mut self, heartbeat: &CallbackHeartbeat) -> Option<StallAction> {
        let tick = heartbeat.tick();
        if tick != self.observed_tick {
            self.observed_tick = tick;
            self.attempts = 0;
            self.last_attempt = None;
            self.exhausted_reported = false;
            return None;
        }
        if heartbeat.age() < AUDIO_CALLBACK_STALL_TIMEOUT {
            return None;
        }
        if self.attempts >= MAX_AUDIO_STALL_RECOVERIES {
            if !self.exhausted_reported {
                self.exhausted_reported = true;
                return Some(StallAction::Exhausted);
            }
            return None;
        }
        if self
            .last_attempt
            .is_some_and(|at| at.elapsed() < AUDIO_STALL_RETRY_INTERVAL)
        {
            return None;
        }
        self.attempts += 1;
        self.last_attempt = Some(Instant::now());
        Some(StallAction::Recover(self.attempts))
    }
}

fn flush_reverse(
    acc: &mut Vec<i16>,
    frame: usize,
    rate: i32,
    channels: i32,
    tx: &flume::Sender<ReverseChunk>,
) {
    if frame == 0 {
        return;
    }
    while acc.len() >= frame {
        let data: Vec<i16> = acc.drain(..frame).collect();
        let _ = tx.try_send(ReverseChunk {
            data,
            rate,
            channels,
        });
    }
}

fn mix_noise_suppression(dry: &mut [i16], wet: &[i16], level: u32) {
    for (d, w) in dry.iter_mut().zip(wet) {
        let dry_val = *d as i32;
        *d = (dry_val + ((*w as i32 - dry_val) * level as i32) / 100) as i16;
    }
}

fn process_reverse(apm: &mut AudioProcessingModule, mut chunk: ReverseChunk) {
    let _ = apm.process_reverse_stream(&mut chunk.data, chunk.rate, chunk.channels);
}

fn drain_reverse(apm: &mut AudioProcessingModule, reverse_rx: &flume::Receiver<ReverseChunk>) {
    for _ in 0..MAX_REVERSE_DRAIN_PER_CAPTURE {
        let Ok(render) = reverse_rx.try_recv() else {
            break;
        };
        process_reverse(apm, render);
    }
}

fn process_capture(
    apm: &mut AudioProcessingModule,
    ns: &mut AudioProcessingModule,
    wet: &mut Vec<i16>,
    reverse_rx: &flume::Receiver<ReverseChunk>,
    mic_tx: &flume::Sender<Vec<i16>>,
    ns_enabled: &AtomicBool,
    ns_level: &AtomicU32,
    mut chunk: CaptureChunk,
) {
    drain_reverse(apm, reverse_rx);
    let _ = apm.set_stream_delay_ms(chunk.delay_ms);
    let _ = apm.process_stream(&mut chunk.data, chunk.rate, chunk.channels);
    let level = ns_level.load(Ordering::Relaxed).min(100);
    if ns_enabled.load(Ordering::Relaxed) && level > 0 {
        wet.clear();
        wet.extend_from_slice(&chunk.data);
        if ns.process_stream(wet, chunk.rate, chunk.channels).is_ok() {
            if level >= 100 {
                chunk.data.copy_from_slice(wet);
            } else {
                mix_noise_suppression(&mut chunk.data, wet, level);
            }
        }
    }
    let _ = mic_tx.try_send(chunk.data);
}

fn run_apm(
    capture_rx: flume::Receiver<CaptureChunk>,
    reverse_rx: flume::Receiver<ReverseChunk>,
    mic_tx: flume::Sender<Vec<i16>>,
    ns_enabled: Arc<AtomicBool>,
    ns_level: Arc<AtomicU32>,
) {
    enum Event {
        Capture(CaptureChunk),
        Reverse(ReverseChunk),
        Stop,
    }
    let mut apm = AudioProcessingModule::new(true, true, true, false);
    let mut ns = AudioProcessingModule::new(false, false, false, true);
    let mut wet: Vec<i16> = Vec::new();
    loop {
        match capture_rx.try_recv() {
            Ok(chunk) => {
                process_capture(
                    &mut apm,
                    &mut ns,
                    &mut wet,
                    &reverse_rx,
                    &mic_tx,
                    &ns_enabled,
                    &ns_level,
                    chunk,
                );
                continue;
            }
            Err(flume::TryRecvError::Disconnected) => break,
            Err(flume::TryRecvError::Empty) => {}
        }
        let event = flume::Selector::new()
            .recv(&capture_rx, |r| {
                r.map(Event::Capture).unwrap_or(Event::Stop)
            })
            .recv(&reverse_rx, |r| {
                r.map(Event::Reverse).unwrap_or(Event::Stop)
            })
            .wait();
        match event {
            Event::Reverse(chunk) => {
                process_reverse(&mut apm, chunk);
            }
            Event::Capture(chunk) => {
                process_capture(
                    &mut apm,
                    &mut ns,
                    &mut wet,
                    &reverse_rx,
                    &mic_tx,
                    &ns_enabled,
                    &ns_level,
                    chunk,
                );
            }
            Event::Stop => break,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u32,
}

#[derive(Default)]
pub struct PlaybackMixer {
    tracks: Mutex<HashMap<u64, VecDeque<i16>>>,
}

impl PlaybackMixer {
    const MAX_BUFFERED: usize = 48_000;

    pub fn push(&self, key: u64, samples: &[i16]) {
        let mut tracks = self.tracks.lock();
        let buf = tracks.entry(key).or_default();
        buf.extend(samples.iter().copied());
        while buf.len() > Self::MAX_BUFFERED {
            buf.pop_front();
        }
    }

    pub fn remove(&self, key: u64) {
        self.tracks.lock().remove(&key);
    }

    fn mix_into(&self, out: &mut [i16]) {
        for slot in out.iter_mut() {
            *slot = 0;
        }
        let mut tracks = self.tracks.lock();
        for buf in tracks.values_mut() {
            for slot in out.iter_mut() {
                let Some(sample) = buf.pop_front() else {
                    break;
                };
                *slot =
                    (*slot as i32 + sample as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }
    }
}

enum AudioCmd {
    SetInputActive(bool),
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    RebuildInput,
    RebuildOutput,
    Shutdown,
}

#[derive(Debug, Clone, Copy)]
pub enum DeviceResetKind {
    Input,
    Output,
}

pub struct AudioIo {
    ctrl_tx: flume::Sender<AudioCmd>,
    audio_stopped_rx: flume::Receiver<()>,
    apm_stopped_rx: flume::Receiver<()>,
    pub output_format: AudioFormat,
    pub mic_rx: flume::Receiver<Vec<i16>>,
    pub input_format_rx: flume::Receiver<AudioFormat>,
    pub output_format_rx: flume::Receiver<AudioFormat>,
    pub device_reset_rx: flume::Receiver<DeviceResetKind>,
    pub mixer: Arc<PlaybackMixer>,
    ns_enabled: Arc<AtomicBool>,
    ns_level: Arc<AtomicU32>,
}

impl AudioIo {
    pub fn set_input_active(&self, active: bool) {
        let _ = self.ctrl_tx.send(AudioCmd::SetInputActive(active));
    }

    pub fn set_input_device(&self, device_id: Option<String>) {
        let _ = self.ctrl_tx.send(AudioCmd::SetInputDevice(device_id));
    }

    pub fn set_output_device(&self, device_id: Option<String>) {
        let _ = self.ctrl_tx.send(AudioCmd::SetOutputDevice(device_id));
    }

    pub fn set_noise_suppression(&self, enabled: bool, level: u8) {
        self.ns_enabled.store(enabled, Ordering::Relaxed);
        self.ns_level
            .store(level.min(100) as u32, Ordering::Relaxed);
    }

    pub fn start(
        input_device_id: Option<String>,
        output_device_id: Option<String>,
    ) -> Result<Self> {
        let mixer = Arc::new(PlaybackMixer::default());
        let (mic_tx, mic_rx) = flume::bounded::<Vec<i16>>(128);
        let (capture_tx, capture_rx) = flume::bounded::<CaptureChunk>(128);
        let (reverse_tx, reverse_rx) = flume::bounded::<ReverseChunk>(128);
        let (ctrl_tx, ctrl_rx) = flume::unbounded::<AudioCmd>();
        let (out_fmt_tx, out_fmt_rx) = flume::bounded::<Result<AudioFormat, String>>(1);
        let (in_fmt_tx, in_fmt_rx) = flume::unbounded::<AudioFormat>();
        let (out_change_tx, out_change_rx) = flume::unbounded::<AudioFormat>();
        let (device_reset_tx, device_reset_rx) = flume::unbounded::<DeviceResetKind>();
        let (audio_stopped_tx, audio_stopped_rx) = flume::bounded::<()>(1);
        let (apm_stopped_tx, apm_stopped_rx) = flume::bounded::<()>(1);
        let ns_enabled = Arc::new(AtomicBool::new(false));
        let ns_level = Arc::new(AtomicU32::new(20));

        let ns_enabled_apm = ns_enabled.clone();
        let ns_level_apm = ns_level.clone();
        std::thread::Builder::new()
            .name("mezon-voice-apm".into())
            .spawn(move || {
                let _exit = WorkerExitSignal(apm_stopped_tx);
                run_apm(capture_rx, reverse_rx, mic_tx, ns_enabled_apm, ns_level_apm);
            })?;

        let mixer_for_thread = mixer.clone();
        let rebuild_tx = ctrl_tx.clone();
        std::thread::Builder::new()
            .name("mezon-voice-audio".into())
            .spawn(move || {
                let _exit = WorkerExitSignal(audio_stopped_tx);
                let out_latency_ms = Arc::new(AtomicU32::new(0));
                let rebuild_pending = Arc::new(AtomicBool::new(false));
                let in_rebuild_pending = Arc::new(AtomicBool::new(false));
                let output_heartbeat = CallbackHeartbeat::new();
                let input_heartbeat = CallbackHeartbeat::new();
                let mut current_input_id = input_device_id;
                let mut current_output_id = output_device_id;
                let mut out_alive = Arc::new(AtomicBool::new(true));
                let mut in_alive = Arc::new(AtomicBool::new(false));
                let prepared = open_output_for_startup(
                    current_output_id.as_deref(),
                    &mixer_for_thread,
                    &reverse_tx,
                    &out_latency_ms,
                    &output_heartbeat,
                    &OutputErrorHook {
                        ctrl_tx: rebuild_tx.clone(),
                        rebuild_pending: rebuild_pending.clone(),
                        stream_alive: out_alive.clone(),
                    },
                );
                let (mut out_stream, mut out_fmt) = match prepared {
                    Ok((stream, fmt, opened_id)) => {
                        if current_output_id.is_some() && opened_id.is_none() {
                            let _ = device_reset_tx.send(DeviceResetKind::Output);
                        }
                        current_output_id = opened_id;
                        (stream, fmt)
                    }
                    Err(e) => {
                        let _ = out_fmt_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = out_fmt_tx.send(Ok(out_fmt));
                let host = cpal::default_host();
                let mut in_stream: Option<cpal::Stream> = None;
                let mut capture_started = false;
                let mut input_active = false;
                let mut output_healthy = true;
                let mut input_healthy = true;
                let mut last_rebuild: Option<Instant> = None;
                let mut last_in_rebuild: Option<Instant> = None;
                let mut output_absent_streak: u32 = 0;
                let mut input_absent_streak: u32 = 0;
                let mut current_in_fmt: Option<AudioFormat> = None;
                let mut output_stall_recovery = StallRecovery::new(&output_heartbeat);
                let mut input_stall_recovery = StallRecovery::new(&input_heartbeat);
                loop {
                    let cmd = match ctrl_rx.recv_timeout(AUDIO_HEALTH_CHECK_INTERVAL) {
                        Ok(cmd) => cmd,
                        Err(flume::RecvTimeoutError::Timeout) => {
                            match output_stall_recovery.poll(&output_heartbeat) {
                                Some(StallAction::Recover(attempt)) => {
                                    tracing::warn!(
                                        attempt,
                                        max_attempts = MAX_AUDIO_STALL_RECOVERIES,
                                        "voice output callback stalled; requesting stream recovery"
                                    );
                                    if !rebuild_pending.swap(true, Ordering::Relaxed) {
                                        let _ = rebuild_tx.send(AudioCmd::RebuildOutput);
                                    }
                                }
                                Some(StallAction::Exhausted) => tracing::warn!(
                                    "voice output callback recovery limit reached; waiting for callback or device change"
                                ),
                                None => {}
                            }
                            if input_active {
                                match input_stall_recovery.poll(&input_heartbeat) {
                                    Some(StallAction::Recover(attempt)) => {
                                        tracing::warn!(
                                            attempt,
                                            max_attempts = MAX_AUDIO_STALL_RECOVERIES,
                                            "voice mic callback stalled; requesting stream recovery"
                                        );
                                        if !in_rebuild_pending.swap(true, Ordering::Relaxed) {
                                            let _ = rebuild_tx.send(AudioCmd::RebuildInput);
                                        }
                                    }
                                    Some(StallAction::Exhausted) => tracing::warn!(
                                        "voice mic callback recovery limit reached; waiting for callback or device change"
                                    ),
                                    None => {}
                                }
                            } else {
                                input_stall_recovery.reset(&input_heartbeat);
                            }
                            continue;
                        }
                        Err(flume::RecvTimeoutError::Disconnected) => break,
                    };
                    match cmd {
                        AudioCmd::SetInputActive(active) => {
                            input_active = active;
                            in_alive.store(active, Ordering::Relaxed);
                            input_stall_recovery.reset(&input_heartbeat);
                            if !active {
                                if let Some(stream) = &in_stream
                                    && let Err(e) = stream.pause()
                                {
                                    tracing::warn!("voice mic stream pause failed: {e}");
                                }
                                continue;
                            }
                            capture_started = true;
                            if in_stream.is_none() {
                                request_macos_microphone_permission();
                                let new_alive = Arc::new(AtomicBool::new(true));
                                match build_input(
                                    &host,
                                    current_input_id.as_deref(),
                                    capture_tx.clone(),
                                    out_latency_ms.clone(),
                                    input_heartbeat.clone(),
                                    InputErrorHook {
                                        ctrl_tx: rebuild_tx.clone(),
                                        rebuild_pending: in_rebuild_pending.clone(),
                                        stream_alive: new_alive.clone(),
                                    },
                                ) {
                                    Ok((stream, in_fmt)) => {
                                        in_alive.store(false, Ordering::Relaxed);
                                        in_alive = new_alive;
                                        input_healthy = true;
                                        in_rebuild_pending.store(false, Ordering::Relaxed);
                                        current_in_fmt = Some(in_fmt);
                                        let _ = in_fmt_tx.send(in_fmt);
                                        in_stream = Some(stream);
                                    }
                                    Err(e) => {
                                        input_healthy = false;
                                        tracing::warn!("voice mic stream build failed: {e}");
                                        if !in_rebuild_pending.swap(true, Ordering::Relaxed) {
                                            let _ = rebuild_tx.send(AudioCmd::RebuildInput);
                                        }
                                    }
                                }
                            }
                            if let Some(stream) = &in_stream
                                && let Err(e) = stream.play()
                            {
                                tracing::warn!("voice mic stream play failed: {e}");
                            }
                        }
                        AudioCmd::SetInputDevice(device_id) => {
                            if input_healthy
                                && device_id.is_some()
                                && current_input_id == device_id
                            {
                                continue;
                            }
                            current_input_id = device_id;
                            input_absent_streak = 0;
                            input_stall_recovery.reset(&input_heartbeat);
                            if !capture_started {
                                continue;
                            }
                            request_macos_microphone_permission();
                            let new_alive = Arc::new(AtomicBool::new(input_active));
                            match build_input(
                                &host,
                                current_input_id.as_deref(),
                                capture_tx.clone(),
                                out_latency_ms.clone(),
                                input_heartbeat.clone(),
                                InputErrorHook {
                                    ctrl_tx: rebuild_tx.clone(),
                                    rebuild_pending: in_rebuild_pending.clone(),
                                    stream_alive: new_alive.clone(),
                                },
                            ) {
                                Ok((stream, in_fmt)) => {
                                    in_alive.store(false, Ordering::Relaxed);
                                    in_alive = new_alive;
                                    if let Some(old) = in_stream.take() {
                                        drop_stream_detached(old);
                                    }
                                    if input_active
                                        && let Err(e) = stream.play()
                                    {
                                        tracing::warn!("voice mic stream play failed: {e}");
                                    }
                                    in_stream = Some(stream);
                                    input_healthy = true;
                                    in_rebuild_pending.store(false, Ordering::Relaxed);
                                    current_in_fmt = Some(in_fmt);
                                    let _ = in_fmt_tx.send(in_fmt);
                                }
                                Err(e) => {
                                    input_healthy = false;
                                    tracing::warn!("voice mic stream rebuild failed: {e}")
                                }
                            }
                        }
                        AudioCmd::SetOutputDevice(device_id) => {
                            if output_healthy
                                && device_id.is_some()
                                && current_output_id == device_id
                            {
                                continue;
                            }
                            current_output_id = device_id;
                            output_absent_streak = 0;
                            output_stall_recovery.reset(&output_heartbeat);
                            let new_alive = Arc::new(AtomicBool::new(true));
                            match prepare_output(
                                current_output_id.as_deref(),
                                mixer_for_thread.clone(),
                                reverse_tx.clone(),
                                out_latency_ms.clone(),
                                output_heartbeat.clone(),
                                OutputErrorHook {
                                    ctrl_tx: rebuild_tx.clone(),
                                    rebuild_pending: rebuild_pending.clone(),
                                    stream_alive: new_alive.clone(),
                                },
                            ) {
                                Ok((new_stream, new_fmt)) => {
                                    out_alive.store(false, Ordering::Relaxed);
                                    out_alive = new_alive;
                                    drop_stream_detached(std::mem::replace(
                                        &mut out_stream,
                                        new_stream,
                                    ));
                                    output_healthy = true;
                                    rebuild_pending.store(false, Ordering::Relaxed);
                                    let changed = new_fmt.sample_rate != out_fmt.sample_rate
                                        || new_fmt.channels != out_fmt.channels;
                                    out_fmt = new_fmt;
                                    if changed {
                                        let _ = out_change_tx.send(new_fmt);
                                    }
                                }
                                Err(e) => {
                                    output_healthy = false;
                                    tracing::warn!("voice output stream rebuild failed: {e}")
                                }
                            }
                        }
                        AudioCmd::RebuildInput => {
                            if !in_rebuild_pending.load(Ordering::Relaxed) {
                                continue;
                            }
                            if !capture_started || !input_active {
                                in_rebuild_pending.store(false, Ordering::Relaxed);
                                continue;
                            }
                            if let Some(last) = last_in_rebuild
                                && let Some(wait) =
                                    STREAM_REBUILD_MIN_INTERVAL.checked_sub(last.elapsed())
                            {
                                std::thread::sleep(wait);
                            }
                            last_in_rebuild = Some(Instant::now());
                            request_macos_microphone_permission();
                            let new_alive = Arc::new(AtomicBool::new(true));
                            let rebuild = rebuild_input_stream(
                                &host,
                                current_input_id.as_deref(),
                                input_absent_streak,
                                &capture_tx,
                                &out_latency_ms,
                                &input_heartbeat,
                                &InputErrorHook {
                                    ctrl_tx: rebuild_tx.clone(),
                                    rebuild_pending: in_rebuild_pending.clone(),
                                    stream_alive: new_alive.clone(),
                                },
                            );
                            in_rebuild_pending.store(false, Ordering::Relaxed);
                            match rebuild {
                                InputRebuild::Installed {
                                    stream,
                                    fmt,
                                    active_id,
                                } => {
                                    if current_input_id.is_some() && active_id.is_none() {
                                        let _ = device_reset_tx.send(DeviceResetKind::Input);
                                    }
                                    input_absent_streak = 0;
                                    current_input_id = active_id;
                                    in_alive.store(false, Ordering::Relaxed);
                                    in_alive = new_alive;
                                    if let Some(old) = in_stream.take() {
                                        drop_stream_detached(old);
                                    }
                                    if let Err(e) = stream.play() {
                                        tracing::warn!("voice mic stream play failed: {e}");
                                    }
                                    in_stream = Some(stream);
                                    input_healthy = true;
                                    tracing::info!(
                                        "voice mic stream recovered: {}Hz/{}ch",
                                        fmt.sample_rate,
                                        fmt.channels,
                                    );
                                    let changed = match current_in_fmt {
                                        Some(f) => {
                                            f.sample_rate != fmt.sample_rate
                                                || f.channels != fmt.channels
                                        }
                                        None => true,
                                    };
                                    current_in_fmt = Some(fmt);
                                    if changed {
                                        let _ = in_fmt_tx.send(fmt);
                                    }
                                }
                                InputRebuild::KeepRetrying { absent_streak } => {
                                    input_absent_streak = absent_streak;
                                    input_healthy = false;
                                    if absent_streak > 0
                                        && absent_streak < INPUT_DISCONNECT_CONFIRM
                                        && !in_rebuild_pending.swap(true, Ordering::Relaxed)
                                    {
                                        let _ = rebuild_tx.send(AudioCmd::RebuildInput);
                                    }
                                }
                            }
                        }
                        AudioCmd::RebuildOutput => {
                            if !rebuild_pending.load(Ordering::Relaxed) {
                                continue;
                            }
                            if let Some(last) = last_rebuild
                                && let Some(wait) =
                                    STREAM_REBUILD_MIN_INTERVAL.checked_sub(last.elapsed())
                            {
                                std::thread::sleep(wait);
                            }
                            last_rebuild = Some(Instant::now());
                            let new_alive = Arc::new(AtomicBool::new(true));
                            let rebuild = rebuild_output_stream(
                                current_output_id.as_deref(),
                                output_absent_streak,
                                &mixer_for_thread,
                                &reverse_tx,
                                &out_latency_ms,
                                &output_heartbeat,
                                &OutputErrorHook {
                                    ctrl_tx: rebuild_tx.clone(),
                                    rebuild_pending: rebuild_pending.clone(),
                                    stream_alive: new_alive.clone(),
                                },
                            );
                            rebuild_pending.store(false, Ordering::Relaxed);
                            match rebuild {
                                OutputRebuild::Installed {
                                    stream,
                                    fmt,
                                    active_id,
                                } => {
                                    if current_output_id.is_some() && active_id.is_none() {
                                        let _ = device_reset_tx.send(DeviceResetKind::Output);
                                    }
                                    output_absent_streak = 0;
                                    current_output_id = active_id;
                                    out_alive.store(false, Ordering::Relaxed);
                                    out_alive = new_alive;
                                    drop_stream_detached(std::mem::replace(
                                        &mut out_stream,
                                        stream,
                                    ));
                                    output_healthy = true;
                                    tracing::info!(
                                        "voice output stream recovered: {}Hz/{}ch",
                                        fmt.sample_rate,
                                        fmt.channels,
                                    );
                                    let changed = fmt.sample_rate != out_fmt.sample_rate
                                        || fmt.channels != out_fmt.channels;
                                    out_fmt = fmt;
                                    if changed {
                                        let _ = out_change_tx.send(fmt);
                                    }
                                }
                                OutputRebuild::KeepRetrying { absent_streak } => {
                                    output_absent_streak = absent_streak;
                                    output_healthy = false;
                                    if absent_streak > 0
                                        && absent_streak < OUTPUT_DISCONNECT_CONFIRM
                                        && !rebuild_pending.swap(true, Ordering::Relaxed)
                                    {
                                        let _ = rebuild_tx.send(AudioCmd::RebuildOutput);
                                    }
                                }
                            }
                        }
                        AudioCmd::Shutdown => {
                            in_alive.store(false, Ordering::Relaxed);
                            out_alive.store(false, Ordering::Relaxed);
                            if let Some(stream) = &in_stream {
                                let _ = stream.pause();
                            }
                            let _ = out_stream.pause();
                            break;
                        }
                    }
                }
                drop(in_stream);
                drop(out_stream);
            })?;

        let output_format = out_fmt_rx
            .recv()
            .map_err(|_| anyhow!("voice audio thread terminated before init"))?
            .map_err(|e| anyhow!("audio device init failed: {e}"))?;

        tracing::info!(
            "voice audio output started: {}Hz/{}ch",
            output_format.sample_rate,
            output_format.channels,
        );

        Ok(Self {
            ctrl_tx,
            audio_stopped_rx,
            apm_stopped_rx,
            output_format,
            mic_rx,
            input_format_rx: in_fmt_rx,
            output_format_rx: out_change_rx,
            device_reset_rx,
            mixer,
            ns_enabled,
            ns_level,
        })
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        let started = Instant::now();
        let deadline = started + AUDIO_SHUTDOWN_TIMEOUT;
        let _ = self.ctrl_tx.send(AudioCmd::Shutdown);
        let audio_stopped = wait_for_worker(&self.audio_stopped_rx, deadline);
        let apm_stopped = wait_for_worker(&self.apm_stopped_rx, deadline);
        if audio_stopped && apm_stopped {
            tracing::debug!(
                elapsed_ms = started.elapsed().as_millis(),
                "voice audio workers stopped"
            );
        } else {
            tracing::warn!(
                audio_stopped,
                apm_stopped,
                "voice audio workers did not stop within {}ms",
                AUDIO_SHUTDOWN_TIMEOUT.as_millis(),
            );
        }
    }
}

fn wait_for_worker(receiver: &flume::Receiver<()>, deadline: Instant) -> bool {
    let remaining = deadline.saturating_duration_since(Instant::now());
    !remaining.is_zero() && receiver.recv_timeout(remaining).is_ok()
}

#[derive(Clone)]
struct InputErrorHook {
    ctrl_tx: flume::Sender<AudioCmd>,
    rebuild_pending: Arc<AtomicBool>,
    stream_alive: Arc<AtomicBool>,
}

impl InputErrorHook {
    fn fire_rebuild(&self) {
        if self.stream_alive.load(Ordering::Relaxed)
            && !self.rebuild_pending.swap(true, Ordering::Relaxed)
        {
            let _ = self.ctrl_tx.send(AudioCmd::RebuildInput);
        }
    }
}

fn input_err_fn(hook: InputErrorHook) -> impl FnMut(cpal::StreamError) + Send + 'static {
    let mut last_log: Option<Instant> = None;
    move |e| {
        if last_log.is_none_or(|at| at.elapsed() >= STREAM_ERROR_LOG_INTERVAL) {
            last_log = Some(Instant::now());
            tracing::warn!("voice audio input stream error: {e}");
        }
        if !matches!(e, cpal::StreamError::BufferUnderrun) {
            hook.fire_rebuild();
        }
    }
}

const STREAM_REBUILD_MIN_INTERVAL: Duration = Duration::from_secs(1);
const STREAM_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(1);
const AUDIO_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const AUDIO_CALLBACK_STALL_TIMEOUT: Duration = Duration::from_secs(5);
const AUDIO_STALL_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const MAX_AUDIO_STALL_RECOVERIES: u32 = 3;
const INITIAL_OUTPUT_OPEN_ATTEMPTS: u32 = 3;
const INITIAL_OUTPUT_RETRY_INTERVAL: Duration = Duration::from_millis(300);
const OUTPUT_DISCONNECT_CONFIRM: u32 = 2;
const INPUT_DISCONNECT_CONFIRM: u32 = 2;
const AUDIO_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REVERSE_DRAIN_PER_CAPTURE: usize = 8;

#[derive(Clone)]
struct OutputErrorHook {
    ctrl_tx: flume::Sender<AudioCmd>,
    rebuild_pending: Arc<AtomicBool>,
    stream_alive: Arc<AtomicBool>,
}

impl OutputErrorHook {
    fn fire_rebuild(&self) {
        if self.stream_alive.load(Ordering::Relaxed)
            && !self.rebuild_pending.swap(true, Ordering::Relaxed)
        {
            let _ = self.ctrl_tx.send(AudioCmd::RebuildOutput);
        }
    }
}

fn output_err_fn(hook: OutputErrorHook) -> impl FnMut(cpal::StreamError) + Send + 'static {
    let mut last_log: Option<Instant> = None;
    move |e| {
        if last_log.is_none_or(|at| at.elapsed() >= STREAM_ERROR_LOG_INTERVAL) {
            last_log = Some(Instant::now());
            tracing::warn!("voice audio output stream error: {e}");
        }
        if !matches!(e, cpal::StreamError::BufferUnderrun) {
            hook.fire_rebuild();
        }
    }
}

fn drop_stream_detached(stream: cpal::Stream) {
    let _ = std::thread::Builder::new()
        .name("mezon-voice-stream-drop".into())
        .spawn(move || drop(stream));
}

#[cfg(target_os = "macos")]
fn mic_authorization_status() -> i64 {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSString;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    unsafe {
        let Some(cls) = Class::get("AVCaptureDevice") else {
            return 3;
        };
        let media_type: id = NSString::alloc(nil).init_str("soun");
        msg_send![cls, authorizationStatusForMediaType: media_type]
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn microphone_denied() -> bool {
    matches!(mic_authorization_status(), 1 | 2)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn microphone_denied() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn request_macos_microphone_permission() {
    use std::time::Duration;

    use block::ConcreteBlock;
    use cocoa::base::{BOOL, NO, id, nil};
    use cocoa::foundation::NSString;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    if mic_authorization_status() != 0 {
        return;
    }
    let Some(cls) = Class::get("AVCaptureDevice") else {
        return;
    };
    let media_type: id = unsafe { NSString::alloc(nil).init_str("soun") };
    let (tx, rx) = flume::bounded::<bool>(1);
    let handler = ConcreteBlock::new(move |granted: BOOL| {
        let _ = tx.send(granted != NO);
    });
    let handler = handler.copy();
    let _: () = unsafe {
        msg_send![cls, requestAccessForMediaType: media_type completionHandler: &*handler]
    };
    let _ = rx.recv_timeout(Duration::from_secs(15));
}

#[cfg(not(target_os = "macos"))]
fn request_macos_microphone_permission() {}

fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn i16_to_f32(s: i16) -> f32 {
    s as f32 / -(i16::MIN as f32)
}

fn prepare_output(
    output_device_id: Option<&str>,
    mixer: Arc<PlaybackMixer>,
    reverse_tx: flume::Sender<ReverseChunk>,
    out_latency_ms: Arc<AtomicU32>,
    heartbeat: CallbackHeartbeat,
    err_hook: OutputErrorHook,
) -> Result<(cpal::Stream, AudioFormat)> {
    let host = cpal::default_host();
    let device = select_output(&host, output_device_id)?;
    open_output(
        &device,
        mixer,
        reverse_tx,
        out_latency_ms,
        heartbeat,
        err_hook,
    )
}

fn open_output_for_startup(
    output_device_id: Option<&str>,
    mixer: &Arc<PlaybackMixer>,
    reverse_tx: &flume::Sender<ReverseChunk>,
    out_latency_ms: &Arc<AtomicU32>,
    heartbeat: &CallbackHeartbeat,
    err_hook: &OutputErrorHook,
) -> Result<(cpal::Stream, AudioFormat, Option<String>)> {
    if let Some(id) = output_device_id {
        for attempt in 1..=INITIAL_OUTPUT_OPEN_ATTEMPTS {
            match prepare_output(
                Some(id),
                mixer.clone(),
                reverse_tx.clone(),
                out_latency_ms.clone(),
                heartbeat.clone(),
                err_hook.clone(),
            ) {
                Ok((stream, fmt)) => return Ok((stream, fmt, Some(id.to_string()))),
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        "preferred voice output device unavailable at startup: {e}"
                    );
                    if attempt < INITIAL_OUTPUT_OPEN_ATTEMPTS {
                        std::thread::sleep(INITIAL_OUTPUT_RETRY_INTERVAL);
                    }
                }
            }
        }
        tracing::warn!(
            "preferred voice output device still unavailable; starting on system default"
        );
    }
    let (stream, fmt) = prepare_output(
        None,
        mixer.clone(),
        reverse_tx.clone(),
        out_latency_ms.clone(),
        heartbeat.clone(),
        err_hook.clone(),
    )?;
    Ok((stream, fmt, None))
}

enum OutputRebuild {
    Installed {
        stream: cpal::Stream,
        fmt: AudioFormat,
        active_id: Option<String>,
    },
    KeepRetrying {
        absent_streak: u32,
    },
}

fn output_device_absent(id: &str) -> bool {
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(mut devices) => {
            !devices.any(|device| matches!(device.id(), Ok(found) if found.to_string() == id))
        }
        Err(_) => false,
    }
}

fn rebuild_output_stream(
    desired_id: Option<&str>,
    absent_streak: u32,
    mixer: &Arc<PlaybackMixer>,
    reverse_tx: &flume::Sender<ReverseChunk>,
    out_latency_ms: &Arc<AtomicU32>,
    heartbeat: &CallbackHeartbeat,
    err_hook: &OutputErrorHook,
) -> OutputRebuild {
    match prepare_output(
        desired_id,
        mixer.clone(),
        reverse_tx.clone(),
        out_latency_ms.clone(),
        heartbeat.clone(),
        err_hook.clone(),
    ) {
        Ok((stream, fmt)) => {
            return OutputRebuild::Installed {
                stream,
                fmt,
                active_id: desired_id.map(str::to_string),
            };
        }
        Err(e) => tracing::warn!("voice output recovery open failed: {e}"),
    }
    let Some(id) = desired_id else {
        return OutputRebuild::KeepRetrying { absent_streak: 0 };
    };
    if !output_device_absent(id) {
        return OutputRebuild::KeepRetrying { absent_streak: 0 };
    }
    let absent_streak = absent_streak + 1;
    if absent_streak < OUTPUT_DISCONNECT_CONFIRM {
        return OutputRebuild::KeepRetrying { absent_streak };
    }
    match prepare_output(
        None,
        mixer.clone(),
        reverse_tx.clone(),
        out_latency_ms.clone(),
        heartbeat.clone(),
        err_hook.clone(),
    ) {
        Ok((stream, fmt)) => {
            tracing::warn!(
                "preferred voice output device disconnected; switched to system default"
            );
            OutputRebuild::Installed {
                stream,
                fmt,
                active_id: None,
            }
        }
        Err(e) => {
            tracing::warn!("voice output fallback to system default failed: {e}");
            OutputRebuild::KeepRetrying { absent_streak }
        }
    }
}

fn open_output(
    device: &cpal::Device,
    mixer: Arc<PlaybackMixer>,
    reverse_tx: flume::Sender<ReverseChunk>,
    out_latency_ms: Arc<AtomicU32>,
    heartbeat: CallbackHeartbeat,
    err_hook: OutputErrorHook,
) -> Result<(cpal::Stream, AudioFormat)> {
    let supported = device.default_output_config()?;
    let out_fmt = AudioFormat {
        sample_rate: supported.sample_rate(),
        channels: supported.channels() as u32,
    };
    let stream = build_output(
        device,
        &supported,
        mixer,
        reverse_tx,
        out_latency_ms,
        heartbeat,
        err_hook,
    )?;
    stream.play()?;
    Ok((stream, out_fmt))
}

fn select_input(host: &cpal::Host, id: Option<&str>) -> Result<cpal::Device> {
    if let Some(id) = id {
        #[cfg(target_os = "windows")]
        let resolved_id = mezon_native::audio::resolve_input_device_id(id)
            .map_err(|error| anyhow!("failed to resolve Windows input device: {error}"))?;
        #[cfg(target_os = "windows")]
        let id = resolved_id.as_str();

        if let Ok(mut devices) = host.input_devices()
            && let Some(device) =
                devices.find(|device| matches!(device.id(), Ok(found) if found.to_string() == id))
        {
            return Ok(device);
        }
        return Err(anyhow!("audio input device is unavailable: {id}"));
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no audio input device available"))
}

fn select_output(host: &cpal::Host, id: Option<&str>) -> Result<cpal::Device> {
    if let Some(id) = id {
        if let Ok(mut devices) = host.output_devices()
            && let Some(device) =
                devices.find(|d| matches!(d.id(), Ok(did) if did.to_string() == id))
        {
            return Ok(device);
        }
        return Err(anyhow!("audio output device is unavailable: {id}"));
    }
    host.default_output_device()
        .ok_or_else(|| anyhow!("no audio output device available"))
}

fn low_latency_buffer(supported: &cpal::SupportedStreamConfig) -> cpal::BufferSize {
    match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            cpal::BufferSize::Fixed((supported.sample_rate() / 100).clamp(*min, *max))
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    }
}

fn playback_buffer(supported: &cpal::SupportedStreamConfig) -> cpal::BufferSize {
    match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            cpal::BufferSize::Fixed((supported.sample_rate() / 25).clamp(*min, *max))
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    }
}

fn build_input(
    host: &cpal::Host,
    id: Option<&str>,
    capture_tx: flume::Sender<CaptureChunk>,
    out_latency_ms: Arc<AtomicU32>,
    heartbeat: CallbackHeartbeat,
    err_hook: InputErrorHook,
) -> Result<(cpal::Stream, AudioFormat)> {
    let device = select_input(host, id)?;
    open_input(&device, capture_tx, out_latency_ms, heartbeat, err_hook)
}

enum InputRebuild {
    Installed {
        stream: cpal::Stream,
        fmt: AudioFormat,
        active_id: Option<String>,
    },
    KeepRetrying {
        absent_streak: u32,
    },
}

fn input_device_absent(host: &cpal::Host, id: &str) -> bool {
    match host.input_devices() {
        Ok(mut devices) => {
            !devices.any(|device| matches!(device.id(), Ok(found) if found.to_string() == id))
        }
        Err(_) => false,
    }
}

fn rebuild_input_stream(
    host: &cpal::Host,
    desired_id: Option<&str>,
    absent_streak: u32,
    capture_tx: &flume::Sender<CaptureChunk>,
    out_latency_ms: &Arc<AtomicU32>,
    heartbeat: &CallbackHeartbeat,
    err_hook: &InputErrorHook,
) -> InputRebuild {
    match build_input(
        host,
        desired_id,
        capture_tx.clone(),
        out_latency_ms.clone(),
        heartbeat.clone(),
        err_hook.clone(),
    ) {
        Ok((stream, fmt)) => {
            return InputRebuild::Installed {
                stream,
                fmt,
                active_id: desired_id.map(str::to_string),
            };
        }
        Err(e) => tracing::warn!("voice mic recovery open failed: {e}"),
    }
    let Some(id) = desired_id else {
        return InputRebuild::KeepRetrying { absent_streak: 0 };
    };
    if !input_device_absent(host, id) {
        return InputRebuild::KeepRetrying { absent_streak: 0 };
    }
    let absent_streak = absent_streak + 1;
    if absent_streak < INPUT_DISCONNECT_CONFIRM {
        return InputRebuild::KeepRetrying { absent_streak };
    }
    match build_input(
        host,
        None,
        capture_tx.clone(),
        out_latency_ms.clone(),
        heartbeat.clone(),
        err_hook.clone(),
    ) {
        Ok((stream, fmt)) => {
            tracing::warn!("preferred voice mic device disconnected; switched to system default");
            InputRebuild::Installed {
                stream,
                fmt,
                active_id: None,
            }
        }
        Err(e) => {
            tracing::warn!("voice mic fallback to system default failed: {e}");
            InputRebuild::KeepRetrying { absent_streak }
        }
    }
}

fn open_input(
    device: &cpal::Device,
    capture_tx: flume::Sender<CaptureChunk>,
    out_latency_ms: Arc<AtomicU32>,
    heartbeat: CallbackHeartbeat,
    err_hook: InputErrorHook,
) -> Result<(cpal::Stream, AudioFormat)> {
    let supported = device.default_input_config()?;
    let device_id = device
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let device_name = device
        .description()
        .map(|description| description.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let in_fmt = AudioFormat {
        sample_rate: supported.sample_rate(),
        channels: supported.channels() as u32,
    };
    let mut config: cpal::StreamConfig = supported.config();
    config.buffer_size = low_latency_buffer(&supported);
    let rate = in_fmt.sample_rate as i32;
    let channels = in_fmt.channels.max(1) as i32;
    let frame = (in_fmt.sample_rate as usize / 100) * in_fmt.channels.max(1) as usize;
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let tx = capture_tx;
            let mut acc: Vec<i16> = Vec::new();
            device.build_input_stream(
                &config,
                move |data: &[f32], info: &cpal::InputCallbackInfo| {
                    heartbeat.mark();
                    let delay = capture_delay_ms(info, &out_latency_ms);
                    acc.extend(data.iter().copied().map(f32_to_i16));
                    flush_capture(&mut acc, frame, rate, channels, delay, &tx);
                },
                input_err_fn(err_hook),
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let tx = capture_tx;
            let mut acc: Vec<i16> = Vec::new();
            device.build_input_stream(
                &config,
                move |data: &[i16], info: &cpal::InputCallbackInfo| {
                    heartbeat.mark();
                    let delay = capture_delay_ms(info, &out_latency_ms);
                    acc.extend_from_slice(data);
                    flush_capture(&mut acc, frame, rate, channels, delay, &tx);
                },
                input_err_fn(err_hook),
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let tx = capture_tx;
            let mut acc: Vec<i16> = Vec::new();
            device.build_input_stream(
                &config,
                move |data: &[u16], info: &cpal::InputCallbackInfo| {
                    heartbeat.mark();
                    let delay = capture_delay_ms(info, &out_latency_ms);
                    acc.extend(data.iter().map(|&u| (u as i32 - 32768) as i16));
                    flush_capture(&mut acc, frame, rate, channels, delay, &tx);
                },
                input_err_fn(err_hook),
                None,
            )?
        }
        other => bail!("unsupported input sample format: {other:?}"),
    };
    tracing::info!(
        device_id,
        device_name,
        sample_rate = in_fmt.sample_rate,
        channels = in_fmt.channels,
        sample_format = ?supported.sample_format(),
        "voice mic stream opened",
    );
    Ok((stream, in_fmt))
}

fn build_output(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    mixer: Arc<PlaybackMixer>,
    reverse_tx: flume::Sender<ReverseChunk>,
    out_latency_ms: Arc<AtomicU32>,
    heartbeat: CallbackHeartbeat,
    err_hook: OutputErrorHook,
) -> Result<cpal::Stream> {
    let mut config: cpal::StreamConfig = supported.config();
    config.buffer_size = playback_buffer(supported);
    let rate = supported.sample_rate() as i32;
    let channels = supported.channels().max(1) as i32;
    let frame = (supported.sample_rate() as usize / 100) * supported.channels().max(1) as usize;
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let mut tmp: Vec<i16> = Vec::new();
            let mut rev: Vec<i16> = Vec::new();
            let tx = reverse_tx;
            device.build_output_stream(
                &config,
                move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    heartbeat.mark();
                    update_out_latency(info, &out_latency_ms);
                    tmp.clear();
                    tmp.resize(out.len(), 0);
                    mixer.mix_into(&mut tmp);
                    rev.extend_from_slice(&tmp);
                    flush_reverse(&mut rev, frame, rate, channels, &tx);
                    for (o, s) in out.iter_mut().zip(tmp.iter().copied()) {
                        *o = i16_to_f32(s);
                    }
                },
                output_err_fn(err_hook),
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let mut rev: Vec<i16> = Vec::new();
            let tx = reverse_tx;
            device.build_output_stream(
                &config,
                move |out: &mut [i16], info: &cpal::OutputCallbackInfo| {
                    heartbeat.mark();
                    update_out_latency(info, &out_latency_ms);
                    mixer.mix_into(out);
                    rev.extend_from_slice(out);
                    flush_reverse(&mut rev, frame, rate, channels, &tx);
                },
                output_err_fn(err_hook),
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let mut tmp: Vec<i16> = Vec::new();
            let mut rev: Vec<i16> = Vec::new();
            let tx = reverse_tx;
            device.build_output_stream(
                &config,
                move |out: &mut [u16], info: &cpal::OutputCallbackInfo| {
                    heartbeat.mark();
                    update_out_latency(info, &out_latency_ms);
                    tmp.clear();
                    tmp.resize(out.len(), 0);
                    mixer.mix_into(&mut tmp);
                    rev.extend_from_slice(&tmp);
                    flush_reverse(&mut rev, frame, rate, channels, &tx);
                    for (o, s) in out.iter_mut().zip(tmp.iter().copied()) {
                        *o = (s as i32 + 32768) as u16;
                    }
                },
                output_err_fn(err_hook),
                None,
            )?
        }
        other => bail!("unsupported output sample format: {other:?}"),
    };
    Ok(stream)
}
