use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui::Task;
use mezon_audio::VoiceEncoder;
use mezon_store::{MicCaptureHandle, MicPcmFormat};

use super::attachments::PendingAttachment;

pub(super) const MIN_RECORDING_MILLIS: u128 = 500;
const VOICE_FILETYPE: &str = "audio/ogg";

pub(super) struct ActiveRecording {
    capture: Option<MicCaptureHandle>,
    generation: u64,
    started_at: Instant,
}

impl ActiveRecording {
    pub(super) fn opening(generation: u64) -> Self {
        Self {
            capture: None,
            generation,
            started_at: Instant::now(),
        }
    }

    pub(super) fn attach(&mut self, generation: u64, capture: MicCaptureHandle) -> bool {
        if self.generation != generation {
            return false;
        }
        self.capture = Some(capture);
        true
    }

    pub(super) fn is_live(&self) -> bool {
        self.capture.is_some()
    }

    pub(super) fn elapsed_millis(&self) -> u128 {
        self.started_at.elapsed().as_millis()
    }
}

pub(super) type RecordTask = Task<()>;

pub(super) async fn encode_recording(
    pcm: flume::Receiver<Vec<f32>>,
    format: MicPcmFormat,
) -> anyhow::Result<PendingAttachment> {
    let mut encoder = VoiceEncoder::new(format.sample_rate, format.channels)?;
    while let Ok(chunk) = pcm.recv_async().await {
        encoder.push(&chunk);
        encoder.encode_ready_frames()?;
    }
    let recording = encoder.finish()?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let filename = format!("audio-{stamp}.ogg");
    let path: PathBuf = std::env::temp_dir().join(&filename);
    let size = recording.bytes.len() as u64;
    std::fs::write(&path, recording.bytes)?;

    Ok(PendingAttachment {
        path,
        filename,
        filetype: VOICE_FILETYPE.to_string(),
        size,
        is_image: false,
        is_video: false,
        width: 0,
        height: 0,
        duration: recording.duration_secs.round() as i32,
        poster_jpeg: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(rate: u32, channels: u16, seconds: f64) -> flume::Receiver<Vec<f32>> {
        let (sender, receiver) = flume::unbounded();
        let frames = (rate as f64 * seconds) as usize;
        let mut chunk = Vec::new();
        for frame in 0..frames {
            let phase = frame as f64 / rate as f64 * 440.0 * std::f64::consts::TAU;
            let sample = (phase.sin() * 0.5) as f32;
            for _ in 0..channels {
                chunk.push(sample);
            }
            if chunk.len() >= 1024 {
                sender.send(std::mem::take(&mut chunk)).expect("send");
            }
        }
        if !chunk.is_empty() {
            sender.send(chunk).expect("send");
        }
        receiver
    }

    #[test]
    fn encodes_captured_pcm_into_a_playable_attachment() {
        let format = MicPcmFormat {
            sample_rate: 44_100,
            channels: 2,
        };
        let attachment =
            futures::executor::block_on(encode_recording(capture(44_100, 2, 1.6), format))
                .expect("recording");

        assert_eq!(attachment.filetype, "audio/ogg");
        assert!(attachment.filename.ends_with(".ogg"));
        assert_eq!(attachment.duration, 2);
        assert!(!attachment.is_image && !attachment.is_video);
        assert!(attachment.size > 0);

        let bytes = std::fs::read(&attachment.path).expect("recorded file exists");
        assert_eq!(attachment.size, bytes.len() as u64);
        let decoded = mezon_audio::decode_audio(bytes).expect("the player can decode it");
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.channels, 1);
        assert!((decoded.duration_secs() - 1.6).abs() < 0.1);

        std::fs::remove_file(&attachment.path).ok();
    }

    #[test]
    fn a_capture_with_no_samples_is_an_error() {
        let format = MicPcmFormat {
            sample_rate: 48_000,
            channels: 1,
        };
        let (sender, receiver) = flume::unbounded::<Vec<f32>>();
        drop(sender);
        assert!(futures::executor::block_on(encode_recording(receiver, format)).is_err());
    }
}
