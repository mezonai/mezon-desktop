use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
        let event = flume::Selector::new()
            .recv(&reverse_rx, |r| {
                r.map(Event::Reverse).unwrap_or(Event::Stop)
            })
            .recv(&capture_rx, |r| {
                r.map(Event::Capture).unwrap_or(Event::Stop)
            })
            .wait();
        match event {
            Event::Reverse(mut chunk) => {
                let _ = apm.process_reverse_stream(&mut chunk.data, chunk.rate, chunk.channels);
            }
            Event::Capture(mut chunk) => {
                let _ = apm.set_stream_delay_ms(chunk.delay_ms);
                let _ = apm.process_stream(&mut chunk.data, chunk.rate, chunk.channels);
                let level = ns_level.load(Ordering::Relaxed).min(100);
                if ns_enabled.load(Ordering::Relaxed) && level > 0 {
                    wet.clear();
                    wet.extend_from_slice(&chunk.data);
                    if ns
                        .process_stream(&mut wet, chunk.rate, chunk.channels)
                        .is_ok()
                    {
                        if level >= 100 {
                            chunk.data.copy_from_slice(&wet);
                        } else {
                            mix_noise_suppression(&mut chunk.data, &wet, level);
                        }
                    }
                }
                let _ = mic_tx.try_send(chunk.data);
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
    Shutdown,
}

pub struct AudioIo {
    ctrl_tx: flume::Sender<AudioCmd>,
    pub output_format: AudioFormat,
    pub mic_rx: flume::Receiver<Vec<i16>>,
    pub input_format_rx: flume::Receiver<AudioFormat>,
    pub mixer: Arc<PlaybackMixer>,
    ns_enabled: Arc<AtomicBool>,
    ns_level: Arc<AtomicU32>,
}

impl AudioIo {
    pub fn set_input_active(&self, active: bool) {
        let _ = self.ctrl_tx.send(AudioCmd::SetInputActive(active));
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
        let (in_fmt_tx, in_fmt_rx) = flume::bounded::<AudioFormat>(1);
        let ns_enabled = Arc::new(AtomicBool::new(false));
        let ns_level = Arc::new(AtomicU32::new(20));

        let ns_enabled_apm = ns_enabled.clone();
        let ns_level_apm = ns_level.clone();
        std::thread::Builder::new()
            .name("mezon-voice-apm".into())
            .spawn(move || run_apm(capture_rx, reverse_rx, mic_tx, ns_enabled_apm, ns_level_apm))?;

        let mixer_for_thread = mixer.clone();
        std::thread::Builder::new()
            .name("mezon-voice-audio".into())
            .spawn(move || {
                let out_latency_ms = Arc::new(AtomicU32::new(0));
                let prepared = prepare_output(
                    output_device_id.as_deref(),
                    mixer_for_thread,
                    reverse_tx,
                    out_latency_ms.clone(),
                );
                match prepared {
                    Ok((out_stream, out_fmt)) => {
                        let _ = out_fmt_tx.send(Ok(out_fmt));
                        let host = cpal::default_host();
                        let mut in_stream: Option<cpal::Stream> = None;
                        while let Ok(cmd) = ctrl_rx.recv() {
                            match cmd {
                                AudioCmd::SetInputActive(true) => {
                                    if in_stream.is_none() {
                                        request_macos_microphone_permission();
                                        match build_input(
                                            &host,
                                            input_device_id.as_deref(),
                                            capture_tx.clone(),
                                            out_latency_ms.clone(),
                                        ) {
                                            Ok((stream, in_fmt)) => {
                                                let _ = in_fmt_tx.try_send(in_fmt);
                                                in_stream = Some(stream);
                                            }
                                            Err(e) => {
                                                tracing::warn!("voice mic stream build failed: {e}")
                                            }
                                        }
                                    }
                                    if let Some(stream) = &in_stream
                                        && let Err(e) = stream.play()
                                    {
                                        tracing::warn!("voice mic stream play failed: {e}");
                                    }
                                }
                                AudioCmd::SetInputActive(false) => {
                                    if let Some(stream) = &in_stream
                                        && let Err(e) = stream.pause()
                                    {
                                        tracing::warn!("voice mic stream pause failed: {e}");
                                    }
                                }
                                AudioCmd::Shutdown => break,
                            }
                        }
                        drop(in_stream);
                        drop(out_stream);
                    }
                    Err(e) => {
                        let _ = out_fmt_tx.send(Err(e.to_string()));
                    }
                }
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
            output_format,
            mic_rx,
            input_format_rx: in_fmt_rx,
            mixer,
            ns_enabled,
            ns_level,
        })
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        let _ = self.ctrl_tx.send(AudioCmd::Shutdown);
    }
}

fn err_fn(e: cpal::StreamError) {
    tracing::warn!("voice audio stream error: {e}");
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
) -> Result<(cpal::Stream, AudioFormat)> {
    let host = cpal::default_host();
    let output = select_output(&host, output_device_id)?;
    let out_supported = output.default_output_config()?;
    let out_fmt = AudioFormat {
        sample_rate: out_supported.sample_rate(),
        channels: out_supported.channels() as u32,
    };
    let out_stream = build_output(&output, &out_supported, mixer, reverse_tx, out_latency_ms)?;
    out_stream.play()?;
    Ok((out_stream, out_fmt))
}

fn select_input(host: &cpal::Host, id: Option<&str>) -> Result<cpal::Device> {
    if let Some(id) = id
        && let Ok(mut devices) = host.input_devices()
        && let Some(device) = devices.find(|d| matches!(d.id(), Ok(did) if did.to_string() == id))
    {
        return Ok(device);
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no audio input device available"))
}

fn select_output(host: &cpal::Host, id: Option<&str>) -> Result<cpal::Device> {
    if let Some(id) = id
        && let Ok(mut devices) = host.output_devices()
        && let Some(device) = devices.find(|d| matches!(d.id(), Ok(did) if did.to_string() == id))
    {
        return Ok(device);
    }
    host.default_output_device()
        .ok_or_else(|| anyhow!("no audio output device available"))
}

fn build_input(
    host: &cpal::Host,
    id: Option<&str>,
    capture_tx: flume::Sender<CaptureChunk>,
    out_latency_ms: Arc<AtomicU32>,
) -> Result<(cpal::Stream, AudioFormat)> {
    let device = select_input(host, id)?;
    let supported = device.default_input_config()?;
    let in_fmt = AudioFormat {
        sample_rate: supported.sample_rate(),
        channels: supported.channels() as u32,
    };
    let config: cpal::StreamConfig = supported.config();
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
                    let delay = capture_delay_ms(info, &out_latency_ms);
                    acc.extend(data.iter().copied().map(f32_to_i16));
                    flush_capture(&mut acc, frame, rate, channels, delay, &tx);
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let tx = capture_tx;
            let mut acc: Vec<i16> = Vec::new();
            device.build_input_stream(
                &config,
                move |data: &[i16], info: &cpal::InputCallbackInfo| {
                    let delay = capture_delay_ms(info, &out_latency_ms);
                    acc.extend_from_slice(data);
                    flush_capture(&mut acc, frame, rate, channels, delay, &tx);
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let tx = capture_tx;
            let mut acc: Vec<i16> = Vec::new();
            device.build_input_stream(
                &config,
                move |data: &[u16], info: &cpal::InputCallbackInfo| {
                    let delay = capture_delay_ms(info, &out_latency_ms);
                    acc.extend(data.iter().map(|&u| (u as i32 - 32768) as i16));
                    flush_capture(&mut acc, frame, rate, channels, delay, &tx);
                },
                err_fn,
                None,
            )?
        }
        other => bail!("unsupported input sample format: {other:?}"),
    };
    Ok((stream, in_fmt))
}

fn build_output(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    mixer: Arc<PlaybackMixer>,
    reverse_tx: flume::Sender<ReverseChunk>,
    out_latency_ms: Arc<AtomicU32>,
) -> Result<cpal::Stream> {
    let config: cpal::StreamConfig = supported.config();
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
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let mut rev: Vec<i16> = Vec::new();
            let tx = reverse_tx;
            device.build_output_stream(
                &config,
                move |out: &mut [i16], info: &cpal::OutputCallbackInfo| {
                    update_out_latency(info, &out_latency_ms);
                    mixer.mix_into(out);
                    rev.extend_from_slice(out);
                    flush_reverse(&mut rev, frame, rate, channels, &tx);
                },
                err_fn,
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
                err_fn,
                None,
            )?
        }
        other => bail!("unsupported output sample format: {other:?}"),
    };
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_sums_tracks_and_clamps() {
        let mixer = PlaybackMixer::default();
        mixer.push(1, &[10_000, -10_000]);
        mixer.push(2, &[30_000, -30_000]);
        let mut out = [0i16; 2];
        mixer.mix_into(&mut out);
        assert_eq!(out[0], i16::MAX);
        assert_eq!(out[1], i16::MIN);
    }

    #[test]
    fn mixer_outputs_silence_when_empty() {
        let mixer = PlaybackMixer::default();
        let mut out = [123i16; 4];
        mixer.mix_into(&mut out);
        assert_eq!(out, [0, 0, 0, 0]);
    }

    #[test]
    fn mixer_drops_oldest_when_overflowing() {
        let mixer = PlaybackMixer::default();
        let big = vec![7i16; PlaybackMixer::MAX_BUFFERED + 100];
        mixer.push(1, &big);
        let len = mixer.tracks.lock().get(&1).unwrap().len();
        assert_eq!(len, PlaybackMixer::MAX_BUFFERED);
    }
}
