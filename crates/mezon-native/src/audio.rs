use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use flume::Sender;
use std::sync::Mutex;

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
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().ok()?.to_string();
            Some(AudioDeviceInfo { id, name })
        })
        .collect()
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
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().ok()?.to_string();
            Some(AudioDeviceInfo { id, name })
        })
        .collect()
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
    pub fn start(
        device_id: &str,
        sender: Sender<f32>,
    ) -> Result<Self, MicCaptureError> {
        let host = cpal::default_host();

        let mut devices = host
            .input_devices()
            .map_err(MicCaptureError::EnumerationFailed)?;
        let device = devices
            .find(|d| matches!(d.id(), Ok(id) if id.to_string() == device_id))
            .ok_or_else(|| MicCaptureError::DeviceNotFound(device_id.to_string()))?;

        let config = device
            .default_input_config()
            .map_err(|e: cpal::DefaultStreamConfigError| MicCaptureError::ConfigError(e.to_string()))?;

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
                        let _ = s.send(rms);
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
