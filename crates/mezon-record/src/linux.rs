use std::path::Path;
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use crate::sink::{
    PixelData, RECORD_CHANNELS, RECORD_SAMPLE_RATE, RecordError, RecordSink, VideoConfig,
    VideoFrameRef,
};

pub fn is_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        ensure_init().is_ok()
            && has_element("audioconvert")
            && has_element("opusenc")
            && ((has_element("vp8enc") && has_element("webmmux"))
                || (has_element("x264enc") && has_element("matroskamux")))
    })
}

const EOS_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(10);
const VIDEO_BITRATE_KBPS: i32 = 2_500;
const AUDIO_BITRATE: i32 = 128_000;

fn has_element(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

fn ensure_init() -> Result<(), RecordError> {
    gst::init().map_err(|error| {
        tracing::error!("gstreamer init failed for call recording: {error}");
        RecordError::Create
    })
}

pub fn file_extension() -> &'static str {
    static EXTENSION: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    EXTENSION.get_or_init(|| {
        if ensure_init().is_ok() && has_element("vp8enc") && has_element("webmmux") {
            "webm"
        } else {
            "mkv"
        }
    })
}

pub struct LinuxSink {
    pipeline: gst::Pipeline,
    audio_src: gst_app::AppSrc,
    video_src: Option<gst_app::AppSrc>,
    video: Option<VideoConfig>,
}

pub fn create_sink(
    path: &Path,
    video: Option<VideoConfig>,
) -> Result<Box<dyn RecordSink>, RecordError> {
    LinuxSink::new(path, video).map(|sink| Box::new(sink) as Box<dyn RecordSink>)
}

impl LinuxSink {
    fn new(path: &Path, video: Option<VideoConfig>) -> Result<Self, RecordError> {
        ensure_init()?;

        let use_vp8 = has_element("vp8enc") && has_element("webmmux");
        let use_x264 = has_element("x264enc") && has_element("matroskamux");
        if !use_vp8 && !use_x264 {
            tracing::error!("no usable gstreamer encoder for call recording (vp8enc or x264enc)");
            return Err(RecordError::Unsupported);
        }

        let pipeline = gst::Pipeline::new();
        let make = |name: &str| -> Result<gst::Element, RecordError> {
            gst::ElementFactory::make(name).build().map_err(|error| {
                tracing::error!("could not create gstreamer element {name}: {error}");
                RecordError::Create
            })
        };

        let mux = make(if use_vp8 { "webmmux" } else { "matroskamux" })?;
        let filesink = make("filesink")?;
        filesink.set_property("location", path.to_string_lossy().as_ref());
        filesink.set_property("sync", false);

        let audio_src = gst_app::AppSrc::builder()
            .caps(
                &gst::Caps::builder("audio/x-raw")
                    .field("format", "S16LE")
                    .field("rate", RECORD_SAMPLE_RATE as i32)
                    .field("channels", RECORD_CHANNELS as i32)
                    .field("layout", "interleaved")
                    .build(),
            )
            .format(gst::Format::Time)
            .is_live(true)
            .build();
        let audio_convert = make("audioconvert")?;
        let audio_enc = make("opusenc")?;
        audio_enc.set_property("bitrate", AUDIO_BITRATE);
        let audio_queue = make("queue")?;

        pipeline
            .add_many([
                audio_src.upcast_ref::<gst::Element>(),
                &audio_convert,
                &audio_enc,
                &audio_queue,
                &mux,
                &filesink,
            ])
            .map_err(|_| RecordError::Create)?;
        gst::Element::link_many([
            audio_src.upcast_ref::<gst::Element>(),
            &audio_convert,
            &audio_enc,
            &audio_queue,
        ])
        .map_err(|_| RecordError::Create)?;
        audio_queue.link(&mux).map_err(|_| RecordError::Create)?;
        mux.link(&filesink).map_err(|_| RecordError::Create)?;

        let video_src = match video {
            Some(config) => {
                let src = gst_app::AppSrc::builder()
                    .caps(
                        &gst::Caps::builder("video/x-raw")
                            .field("format", "BGRA")
                            .field("width", config.width as i32)
                            .field("height", config.height as i32)
                            .field("framerate", gst::Fraction::new(config.fps as i32, 1))
                            .build(),
                    )
                    .format(gst::Format::Time)
                    .is_live(true)
                    .build();
                let convert = make("videoconvert")?;
                let encoder = if use_vp8 {
                    let encoder = make("vp8enc")?;
                    encoder.set_property("deadline", 1i64);
                    encoder.set_property("target-bitrate", VIDEO_BITRATE_KBPS * 1_000);
                    encoder
                } else {
                    let encoder = make("x264enc")?;
                    encoder.set_property("bitrate", VIDEO_BITRATE_KBPS as u32);
                    encoder.set_property_from_str("speed-preset", "veryfast");
                    encoder.set_property_from_str("tune", "zerolatency");
                    encoder
                };
                let queue = make("queue")?;

                pipeline
                    .add_many([src.upcast_ref::<gst::Element>(), &convert, &encoder, &queue])
                    .map_err(|_| RecordError::Create)?;
                gst::Element::link_many([
                    src.upcast_ref::<gst::Element>(),
                    &convert,
                    &encoder,
                    &queue,
                ])
                .map_err(|_| RecordError::Create)?;
                queue.link(&mux).map_err(|_| RecordError::Create)?;
                Some(src)
            }
            None => None,
        };

        pipeline.set_state(gst::State::Playing).map_err(|error| {
            tracing::error!("could not start the call recording pipeline: {error}");
            RecordError::Create
        })?;

        Ok(Self {
            pipeline,
            audio_src,
            video_src,
            video,
        })
    }
}

fn buffer_with_pts(data: Vec<u8>, pts: Duration) -> gst::Buffer {
    let mut buffer = gst::Buffer::from_mut_slice(data);
    if let Some(reference) = buffer.get_mut() {
        reference.set_pts(gst::ClockTime::from_nseconds(pts.as_nanos() as u64));
    }
    buffer
}

impl RecordSink for LinuxSink {
    fn push_audio(&mut self, pcm: &[i16], pts: Duration) -> Result<(), RecordError> {
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(pcm));
        for sample in pcm {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.audio_src
            .push_buffer(buffer_with_pts(bytes, pts))
            .map(|_| ())
            .map_err(|error| {
                tracing::error!("call recorder could not push audio: {error}");
                RecordError::Encode
            })
    }

    fn push_video(&mut self, frame: VideoFrameRef<'_>, pts: Duration) -> Result<(), RecordError> {
        let (Some(src), Some(config)) = (&self.video_src, self.video) else {
            return Ok(());
        };
        let PixelData::Bgra { data, stride } = frame.data else {
            return Ok(());
        };
        let row_bytes = config.width as usize * 4;
        let mut packed = Vec::with_capacity(row_bytes * config.height as usize);
        for row in 0..config.height as usize {
            let start = row * stride;
            let end = start + row_bytes;
            if end > data.len() {
                break;
            }
            packed.extend_from_slice(&data[start..end]);
        }
        packed.resize(row_bytes * config.height as usize, 0);

        src.push_buffer(buffer_with_pts(packed, pts))
            .map(|_| ())
            .map_err(|error| {
                tracing::error!("call recorder could not push video: {error}");
                RecordError::Encode
            })
    }

    fn finish(self: Box<Self>) -> Result<(), RecordError> {
        if let Some(src) = &self.video_src {
            let _ = src.end_of_stream();
        }
        let _ = self.audio_src.end_of_stream();

        let finished = match self.pipeline.bus() {
            Some(bus) => match bus.timed_pop_filtered(
                EOS_TIMEOUT,
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
                Some(message) => match message.view() {
                    gst::MessageView::Eos(_) => Ok(()),
                    other => {
                        tracing::error!("call recording pipeline ended with an error: {other:?}");
                        Err(RecordError::Finish)
                    }
                },
                None => {
                    tracing::error!("call recording pipeline did not flush before the timeout");
                    Err(RecordError::Finish)
                }
            },
            None => Ok(()),
        };
        let _ = self.pipeline.set_state(gst::State::Null);
        finished
    }
}
