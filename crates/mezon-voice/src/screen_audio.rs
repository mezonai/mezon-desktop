pub const SCREEN_AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const SCREEN_AUDIO_CHANNELS: u32 = 2;

#[cfg(target_os = "macos")]
pub use macos::{ScreenAudioCapture, start_screen_audio};

#[cfg(not(target_os = "macos"))]
pub use fallback::{ScreenAudioCapture, start_screen_audio};

#[cfg(not(target_os = "macos"))]
mod fallback {
    pub struct ScreenAudioCapture {
        pub rx: flume::Receiver<Vec<i16>>,
    }

    pub fn start_screen_audio() -> Result<ScreenAudioCapture, String> {
        Err("system audio capture is not supported on this platform".into())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use screencapturekit_sys::audio_buffer::CopiedAudioBuffer;
    use screencapturekit_sys::cm_sample_buffer_ref::CMSampleBufferRef;
    use screencapturekit_sys::content_filter::{UnsafeContentFilter, UnsafeInitParams};
    use screencapturekit_sys::os_types::base::CMTime;
    use screencapturekit_sys::os_types::rc::Id;
    use screencapturekit_sys::shareable_content::UnsafeSCShareableContent;
    use screencapturekit_sys::stream::UnsafeSCStream;
    use screencapturekit_sys::stream_configuration::UnsafeStreamConfiguration;
    use screencapturekit_sys::stream_error_handler::UnsafeSCStreamError;
    use screencapturekit_sys::stream_output_handler::UnsafeSCStreamOutput;

    use super::{SCREEN_AUDIO_CHANNELS, SCREEN_AUDIO_SAMPLE_RATE};

    const AUDIO_OUTPUT_TYPE: u8 = 1;

    pub struct ScreenAudioCapture {
        stream: Id<UnsafeSCStream>,
        pub rx: flume::Receiver<Vec<i16>>,
    }

    impl Drop for ScreenAudioCapture {
        fn drop(&mut self) {
            let _ = self.stream.stop_capture();
        }
    }

    struct AudioOutput {
        tx: flume::Sender<Vec<i16>>,
    }

    impl UnsafeSCStreamOutput for AudioOutput {
        fn did_output_sample_buffer(&self, sample: Id<CMSampleBufferRef>, of_type: u8) {
            if of_type != AUDIO_OUTPUT_TYPE {
                return;
            }
            let buffers = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sample.get_av_audio_buffer_list()
            })) {
                Ok(buffers) => buffers,
                Err(_) => {
                    return;
                }
            };
            let interleaved = interleave_stereo_i16(&buffers);
            if !interleaved.is_empty() {
                let _ = self.tx.try_send(interleaved);
            }
        }
    }

    struct AudioErrorHandler;

    impl UnsafeSCStreamError for AudioErrorHandler {
        fn handle_error(&self) {
            tracing::warn!("screen audio capture stream error");
        }
    }

    pub fn start_screen_audio() -> Result<ScreenAudioCapture, String> {
        let content = UnsafeSCShareableContent::get()
            .map_err(|e| format!("shareable content unavailable: {e}"))?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or_else(|| "no display available for system audio".to_string())?;
        let filter = UnsafeContentFilter::init(UnsafeInitParams::Display(display));
        let config = UnsafeStreamConfiguration {
            width: 2,
            height: 2,
            captures_audio: 1,
            sample_rate: SCREEN_AUDIO_SAMPLE_RATE,
            channel_count: SCREEN_AUDIO_CHANNELS,
            excludes_current_process_audio: 1,
            minimum_frame_interval: CMTime {
                value: 1,
                timescale: 1,
                epoch: 0,
                flags: 1,
            },
            ..Default::default()
        };
        let (tx, rx) = flume::bounded::<Vec<i16>>(64);
        let stream = UnsafeSCStream::init(filter, config.into(), AudioErrorHandler);
        stream.add_stream_output(AudioOutput { tx }, AUDIO_OUTPUT_TYPE);
        stream
            .start_capture()
            .map_err(|e| format!("screen audio start failed: {e}"))?;
        Ok(ScreenAudioCapture { stream, rx })
    }

    fn f32_samples(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn to_i16(sample: f32) -> i16 {
        (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }

    fn interleave_stereo_i16(buffers: &[CopiedAudioBuffer]) -> Vec<i16> {
        match buffers {
            [] => Vec::new(),
            [interleaved] => f32_samples(&interleaved.data).map(to_i16).collect(),
            [left, right, ..] => {
                let mut out = Vec::with_capacity(left.data.len() / 2);
                for (l, r) in f32_samples(&left.data).zip(f32_samples(&right.data)) {
                    out.push(to_i16(l));
                    out.push(to_i16(r));
                }
                out
            }
        }
    }
}
