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

#[cfg(all(test, target_os = "macos"))]
mod probe {
    //! Run with `cargo test -p mezon-voice --lib probe -- --ignored --nocapture`
    //! while something outside Mezon is playing sound.
    use std::time::{Duration, Instant};

    /// The whole production chain minus LiveKit: the real ScreenCaptureKit
    /// stream, the same pump loop `start_screen_audio_track` runs, the real
    /// `RecordTaps` the recorder is wired through, and a real encoded file.
    #[test]
    #[ignore]
    fn shared_system_audio_reaches_the_recorded_file() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let path = std::env::temp_dir().join("mezon-screen-audio-probe.mp4");
        let _ = std::fs::remove_file(&path);
        let recorder = mezon_record::Recorder::start(mezon_record::RecorderConfig {
            path: path.clone(),
            video: None,
        })
        .expect("recorder");

        let taps = crate::record::RecordTaps::default();
        taps.set(Some(recorder.audio_tap()));

        let capture = super::start_screen_audio().expect("screen audio");
        let stop = Arc::new(AtomicBool::new(false));

        let pump_taps = taps.clone();
        let pump_rx = capture.rx.clone();
        let pump_stop = stop.clone();
        let pump = std::thread::spawn(move || {
            while !pump_stop.load(Ordering::Relaxed) {
                let Ok(samples) = pump_rx.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                if samples.len() as u32 / super::SCREEN_AUDIO_CHANNELS == 0 {
                    continue;
                }
                pump_taps.push(
                    mezon_record::AudioSource::Screen,
                    &samples,
                    super::SCREEN_AUDIO_SAMPLE_RATE,
                    super::SCREEN_AUDIO_CHANNELS,
                );
            }
        });

        // The playback device tees silence into the recorder the whole call,
        // and that is what paces the mixer, so a faithful run needs it.
        let silence = vec![0i16; 480 * 2];
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            taps.push(mezon_record::AudioSource::Remote, &silence, 48_000, 2);
            std::thread::sleep(Duration::from_millis(10));
        }

        stop.store(true, Ordering::Relaxed);
        pump.join().expect("pump");
        taps.set(None);
        let written = recorder.finish().expect("finish");

        let wav = written.with_extension("probe.wav");
        let status = std::process::Command::new("afconvert")
            .args(["-f", "WAVE", "-d", "LEI16"])
            .arg(&written)
            .arg(&wav)
            .status()
            .expect("afconvert");
        assert!(status.success(), "afconvert could not read the recording");
        let bytes = std::fs::read(&wav).expect("wav");
        let peak = bytes[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs() as i32)
            .max()
            .unwrap_or(0);
        println!("recorded {} bytes, audio peak={peak}", bytes.len());
        assert!(peak > 500, "the shared system audio never reached the file");
    }

    /// The real app already holds an SCStream for the shared video when the
    /// audio one is opened, so a second stream has to be allowed.
    #[test]
    #[ignore]
    fn a_second_capture_stream_still_delivers() {
        let first = match super::start_screen_audio() {
            Ok(capture) => capture,
            Err(error) => panic!("first start_screen_audio failed: {error}"),
        };
        let second = match super::start_screen_audio() {
            Ok(capture) => capture,
            Err(error) => panic!("second start_screen_audio failed: {error}"),
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let (mut a, mut b) = (0u64, 0u64);
        while Instant::now() < deadline {
            if first.rx.recv_timeout(Duration::from_millis(50)).is_ok() {
                a += 1;
            }
            if second.rx.recv_timeout(Duration::from_millis(50)).is_ok() {
                b += 1;
            }
        }
        println!("first={a} second={b}");
        assert!(a > 0 && b > 0, "one of the two streams went silent");
    }

    #[test]
    #[ignore]
    fn system_audio_capture_delivers_samples() {
        let capture = match super::start_screen_audio() {
            Ok(capture) => capture,
            Err(error) => panic!("start_screen_audio failed: {error}"),
        };
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut buffers = 0u64;
        let mut samples = 0u64;
        let mut peak = 0i32;
        while Instant::now() < deadline {
            if let Ok(chunk) = capture.rx.recv_timeout(Duration::from_millis(250)) {
                buffers += 1;
                samples += chunk.len() as u64;
                for value in chunk {
                    peak = peak.max(value.unsigned_abs() as i32);
                }
            }
        }
        println!("buffers={buffers} samples={samples} peak={peak}");
    }
}
