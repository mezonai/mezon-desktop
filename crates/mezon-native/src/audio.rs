use cpal::Sample as _;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use flume::Sender;
use std::sync::Mutex;

#[cfg(target_os = "windows")]
pub const WINDOWS_COMMUNICATIONS_INPUT_DEVICE_ID: &str = "mezon:audio-input:windows-communications";

/// Describes a detected audio device.
#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    /// Stable persistent device identifier from `cpal::Device::id()`.
    /// Survives reboots and reconnects.
    pub id: String,
    /// Human-readable name reported by the OS (`cpal::Device::description()`).
    pub name: String,
}

/// Enumerate all available audio input (microphone) devices.
///
/// Returns an empty vec on error or when no devices are found.
pub fn enumerate_input_devices() -> Vec<AudioDeviceInfo> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        tracing::warn!("Failed to enumerate input devices");
        return vec![];
    };
    let devices = collect_devices(devices);
    #[cfg(target_os = "windows")]
    {
        let mut devices = devices;
        append_windows_communications_input(&mut devices);
        devices
    }
    #[cfg(not(target_os = "windows"))]
    {
        devices
    }
}

/// Enumerate all available audio output (speaker/headphone) devices.
///
/// Returns an empty vec on error or when no devices are found.
pub fn enumerate_output_devices() -> Vec<AudioDeviceInfo> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
        tracing::warn!("Failed to enumerate output devices");
        return vec![];
    };
    collect_devices(devices)
}

pub fn default_input_device_name() -> Option<String> {
    let host = cpal::default_host();
    let device = host.default_input_device()?;
    Some(device.description().ok()?.to_string())
}

pub fn default_output_device_name() -> Option<String> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    Some(device.description().ok()?.to_string())
}

#[cfg(not(target_os = "linux"))]
fn collect_devices(devices: impl Iterator<Item = cpal::Device>) -> Vec<AudioDeviceInfo> {
    devices
        .filter_map(|device| {
            let id = device.id().ok()?.to_string();
            let name = device.description().ok()?.to_string();
            Some(AudioDeviceInfo { id, name })
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn append_windows_communications_input(devices: &mut Vec<AudioDeviceInfo>) {
    let resolved = match resolve_input_device_id(WINDOWS_COMMUNICATIONS_INPUT_DEVICE_ID) {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!("Failed to resolve Windows communications microphone: {error}");
            return;
        }
    };
    let Some(device) = devices.iter().find(|device| device.id == resolved) else {
        tracing::warn!("Windows communications microphone is not in the active device list");
        return;
    };
    devices.push(AudioDeviceInfo {
        id: WINDOWS_COMMUNICATIONS_INPUT_DEVICE_ID.to_string(),
        name: format!("Communications - {}", device.name),
    });
}

#[cfg(target_os = "windows")]
pub fn resolve_input_device_id(device_id: &str) -> Result<String, String> {
    if device_id != WINDOWS_COMMUNICATIONS_INPUT_DEVICE_ID {
        return Ok(device_id.to_string());
    }
    windows_default_input_device_id(windows::Win32::Media::Audio::eCommunications)
}

#[cfg(target_os = "windows")]
fn windows_default_input_device_id(
    role: windows::Win32::Media::Audio::ERole,
) -> Result<String, String> {
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eCapture};
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize,
    };

    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        return Err(format!("COM initialization failed: {initialized}"));
    }
    let _com = ComGuard(initialized.is_ok());

    let endpoint_id = unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|error| format!("MMDeviceEnumerator creation failed: {error}"))?;
        let endpoint = enumerator
            .GetDefaultAudioEndpoint(eCapture, role)
            .map_err(|error| format!("default capture endpoint lookup failed: {error}"))?;
        let id = endpoint
            .GetId()
            .map_err(|error| format!("capture endpoint ID lookup failed: {error}"))?;
        let result = id
            .to_string()
            .map_err(|error| format!("capture endpoint ID conversion failed: {error}"));
        CoTaskMemFree(Some(id.0.cast()));
        result?
    };

    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|error| format!("input device enumeration failed: {error}"))?;
    devices
        .filter_map(|device| device.id().ok())
        .find(|id| id.1 == endpoint_id)
        .map(|id| id.to_string())
        .ok_or_else(|| "default capture endpoint is not available through CPAL".to_string())
}

#[cfg(target_os = "linux")]
fn alsa_pcm_is_hardware(pcm_id: &str) -> bool {
    pcm_id.contains("CARD=")
}

#[cfg(target_os = "linux")]
fn alsa_pcm_is_digital_output(pcm_id: &str) -> bool {
    pcm_id.starts_with("hdmi:") || pcm_id.starts_with("iec958:")
}

#[cfg(target_os = "linux")]
fn alsa_pcm_rank(pcm_id: &str) -> u8 {
    if pcm_id.starts_with("sysdefault:") {
        0
    } else if pcm_id.starts_with("plughw:") {
        1
    } else if pcm_id.starts_with("front:") {
        2
    } else if pcm_id.starts_with("hw:") {
        3
    } else {
        4
    }
}

#[cfg(target_os = "linux")]
fn best_pcm_per_device(mut ranked: Vec<(u8, AudioDeviceInfo)>) -> Vec<AudioDeviceInfo> {
    ranked.sort_by(|(a_rank, a), (b_rank, b)| a.name.cmp(&b.name).then(a_rank.cmp(b_rank)));
    let mut seen = std::collections::HashSet::new();
    ranked
        .into_iter()
        .filter(|(_, device)| seen.insert(device.name.clone()))
        .map(|(_, device)| device)
        .collect()
}

#[cfg(target_os = "linux")]
fn collect_devices(devices: impl Iterator<Item = cpal::Device>) -> Vec<AudioDeviceInfo> {
    let mut analog = Vec::new();
    let mut hardware = Vec::new();
    let mut all = Vec::new();

    for device in devices {
        let (Ok(id), Ok(description)) = (device.id(), device.description()) else {
            continue;
        };
        let pcm_id = description.driver().unwrap_or_default();
        let rank = alsa_pcm_rank(pcm_id);
        let info = AudioDeviceInfo {
            id: id.to_string(),
            name: description.to_string(),
        };

        all.push((rank, info.clone()));

        if alsa_pcm_is_hardware(pcm_id) {
            hardware.push((rank, info.clone()));
            if !alsa_pcm_is_digital_output(pcm_id) {
                analog.push((rank, info));
            }
        }
    }

    let analog = best_pcm_per_device(analog);
    if !analog.is_empty() {
        return analog;
    }
    let hardware = best_pcm_per_device(hardware);
    if !hardware.is_empty() {
        return hardware;
    }
    best_pcm_per_device(all)
}

/// A live microphone capture handle.
///
/// While this struct is alive, the audio stream is active and RMS levels
/// are sent through the provided `flume::Sender<f32>`.
///
/// Dropping this struct stops the stream and releases audio resources.
pub struct MicCapture {
    _stream: cpal::Stream,
}

impl MicCapture {
    /// Start capturing audio from the device identified by `device_id`
    /// (matching the `id` field of `AudioDeviceInfo`).
    ///
    /// RMS levels are computed per-buffer and sent through `sender`.
    /// Returns an error if the device cannot be found or the stream fails to open.
    pub fn start(device_id: &str, sender: Sender<f32>) -> Result<Self, MicCaptureError> {
        let host = cpal::default_host();

        #[cfg(target_os = "windows")]
        let resolved_device_id = resolve_input_device_id(device_id)
            .map_err(|error| MicCaptureError::DeviceNotFound(error.to_string()))?;
        #[cfg(target_os = "windows")]
        let device_id = resolved_device_id.as_str();

        let mut devices = host
            .input_devices()
            .map_err(MicCaptureError::EnumerationFailed)?;
        let device = devices
            .find(|d| matches!(d.id(), Ok(id) if id.to_string() == device_id))
            .ok_or_else(|| MicCaptureError::DeviceNotFound(device_id.to_string()))?;

        let config =
            device
                .default_input_config()
                .map_err(|e: cpal::DefaultStreamConfigError| {
                    MicCaptureError::ConfigError(e.to_string())
                })?;

        let err_fn = move |err| {
            tracing::warn!("Audio stream error: {err}");
        };

        let sender = Mutex::new(sender);

        let stream = device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let rms = compute_rms(data);
                    if let Ok(s) = sender.lock() {
                        let _ = s.try_send(rms);
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e: cpal::BuildStreamError| MicCaptureError::StreamError(e.to_string()))?;

        stream
            .play()
            .map_err(|e: cpal::PlayStreamError| MicCaptureError::StreamError(e.to_string()))?;

        Ok(Self { _stream: stream })
    }
}

/// Format of the PCM delivered by [`MicPcmCapture`].
#[derive(Debug, Clone, Copy)]
pub struct MicPcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// A live microphone capture handle that delivers raw interleaved PCM.
///
/// Unlike [`MicCapture`], which reduces every buffer to a single RMS level,
/// this hands the samples themselves to `sender` so they can be encoded.
///
/// Dropping this struct stops the stream and releases audio resources.
pub struct MicPcmCapture {
    _stream: cpal::Stream,
}

impl MicPcmCapture {
    /// Start capturing raw PCM from the default input device.
    ///
    /// Returns the device format alongside the handle; the samples are
    /// interleaved `f32` in that format, converted from whatever the device
    /// natively provides.
    ///
    /// `sender` must be unbounded: the samples are handed over from the
    /// high-priority audio thread with a non-blocking send, so a bounded
    /// channel that fills up would silently drop captured audio.
    pub fn start(sender: Sender<Vec<f32>>) -> Result<(Self, MicPcmFormat), MicCaptureError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| MicCaptureError::DeviceNotFound("default input".to_string()))?;

        let supported =
            device
                .default_input_config()
                .map_err(|e: cpal::DefaultStreamConfigError| {
                    MicCaptureError::ConfigError(e.to_string())
                })?;

        let format = MicPcmFormat {
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
        };
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        let stream = match sample_format {
            cpal::SampleFormat::I8 => build_pcm_stream::<i8>(&device, &config, sender),
            cpal::SampleFormat::I16 => build_pcm_stream::<i16>(&device, &config, sender),
            cpal::SampleFormat::I24 => build_pcm_stream::<cpal::I24>(&device, &config, sender),
            cpal::SampleFormat::I32 => build_pcm_stream::<i32>(&device, &config, sender),
            cpal::SampleFormat::I64 => build_pcm_stream::<i64>(&device, &config, sender),
            cpal::SampleFormat::U8 => build_pcm_stream::<u8>(&device, &config, sender),
            cpal::SampleFormat::U16 => build_pcm_stream::<u16>(&device, &config, sender),
            cpal::SampleFormat::U32 => build_pcm_stream::<u32>(&device, &config, sender),
            cpal::SampleFormat::U64 => build_pcm_stream::<u64>(&device, &config, sender),
            cpal::SampleFormat::F32 => build_pcm_stream::<f32>(&device, &config, sender),
            cpal::SampleFormat::F64 => build_pcm_stream::<f64>(&device, &config, sender),
            other => {
                return Err(MicCaptureError::ConfigError(format!(
                    "unsupported input sample format {other:?}"
                )));
            }
        }
        .map_err(|e: cpal::BuildStreamError| MicCaptureError::StreamError(e.to_string()))?;

        stream
            .play()
            .map_err(|e: cpal::PlayStreamError| MicCaptureError::StreamError(e.to_string()))?;

        Ok((Self { _stream: stream }, format))
    }
}

fn build_pcm_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sender: Sender<Vec<f32>>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let sender = Mutex::new(sender);
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let pcm = data
                .iter()
                .map(|sample| f32::from_sample(*sample))
                .collect::<Vec<f32>>();
            send_pcm(&sender, pcm);
        },
        |err| tracing::warn!("Mic capture stream error: {err}"),
        None,
    )
}

fn send_pcm(sender: &Mutex<Sender<Vec<f32>>>, pcm: Vec<f32>) {
    if let Ok(sender) = sender.lock() {
        let _ = sender.try_send(pcm);
    }
}

/// Compute the root-mean-square level of a buffer of f32 samples.
///
/// Returns `sqrt(sum(samples²) / samples.len())`.
/// For a buffer of all zeros, returns 0.0.
/// For a full-scale signal (±1.0), returns approximately 1.0.
fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Errors that can occur when starting mic capture.
#[derive(Debug, thiserror::Error)]
pub enum MicCaptureError {
    #[error("Device enumeration failed: {0}")]
    EnumerationFailed(cpal::DevicesError),

    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Could not get device config: {0}")]
    ConfigError(String),

    #[error("Stream error: {0}")]
    StreamError(String),
}
