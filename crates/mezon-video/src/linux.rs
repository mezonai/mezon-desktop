use std::cell::Cell;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::VideoFrameExt;

use crate::poster::Turn;
use crate::{PlayerError, VideoFrame};

fn ensure_gstreamer() -> Result<(), PlayerError> {
    static INIT: Once = Once::new();
    static READY: AtomicBool = AtomicBool::new(false);
    INIT.call_once(|| {
        if gst::init().is_ok() {
            READY.store(true, Ordering::SeqCst);
        }
    });
    if READY.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(PlayerError::Open)
    }
}

pub struct PlayerImpl {
    playbin: gst::Element,
    appsink: gst_app::AppSink,
    failed: Cell<bool>,
}

impl PlayerImpl {
    pub fn open(url: &str, max_size: Option<(u32, u32)>) -> Result<Self, PlayerError> {
        if url.is_empty() {
            return Err(PlayerError::InvalidUrl);
        }
        ensure_gstreamer()?;
        Self::build(url, max_size).map_err(|error| {
            tracing::warn!(target: "mezon_video", %error, "failed to build gstreamer pipeline");
            PlayerError::Open
        })
    }

    fn build(url: &str, max_size: Option<(u32, u32)>) -> Result<Self, gst::glib::BoolError> {
        let playbin = gst::ElementFactory::make("playbin").build()?;
        let mut caps_builder =
            gst_video::VideoCapsBuilder::new().format(gst_video::VideoFormat::Bgra);
        if let Some((width, height)) = max_size
            && width > 0
            && height > 0
        {
            caps_builder = caps_builder.width(width as i32).height(height as i32);
        }
        let caps = caps_builder.build();
        let appsink = gst_app::AppSink::builder()
            .caps(&caps)
            .max_buffers(2)
            .drop(true)
            .build();
        playbin.set_property("uri", url);
        playbin.set_property("video-sink", &appsink);
        if let Some(filter) = auto_orientation_filter() {
            playbin.set_property("video-filter", &filter);
        }
        Ok(Self {
            playbin,
            appsink,
            failed: Cell::new(false),
        })
    }

    fn drain_bus(&self) {
        if let Some(bus) = self.playbin.bus() {
            while let Some(message) = bus.pop() {
                match message.view() {
                    gst::message::MessageView::Error(error) => self.note_error(error),
                    gst::message::MessageView::Eos(_) => {
                        let _ = self.playbin.set_state(gst::State::Paused);
                    }
                    _ => {}
                }
            }
        }
    }

    fn note_error(&self, error: &gst::message::Error) {
        if !self.failed.replace(true) {
            log_pipeline_error(error, "playback");
        }
    }

    pub fn copy_frame(&self) -> Option<VideoFrame> {
        self.drain_bus();
        if self.failed.get() {
            return None;
        }
        let sample = self.appsink.try_pull_sample(gst::ClockTime::ZERO)?;
        let buffer = sample.buffer()?;
        let caps = sample.caps()?;
        let info = gst_video::VideoInfo::from_caps(caps).ok()?;
        let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;

        let width = frame.width();
        let height = frame.height();
        let stride = usize::try_from(*frame.plane_stride().first()?).ok()?;
        let data = frame.plane_data(0).ok()?;
        let out = crate::frame_util::pack_bgra_rows(data, stride, width, height)?;
        crate::render_frame::bgra_to_frame(width, height, out)
    }

    pub fn play(&self) {
        if let Err(error) = self.playbin.set_state(gst::State::Playing) {
            tracing::warn!(target: "mezon_video", %error, "gstreamer set_state(Playing) failed");
        }
    }

    pub fn pause(&self) {
        if let Err(error) = self.playbin.set_state(gst::State::Paused) {
            tracing::warn!(target: "mezon_video", %error, "gstreamer set_state(Paused) failed");
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playbin.current_state() == gst::State::Playing
    }

    pub fn current_time(&self) -> f64 {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(clock_time_to_seconds)
            .unwrap_or(0.0)
    }

    pub fn duration(&self) -> f64 {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(clock_time_to_seconds)
            .unwrap_or(0.0)
    }

    pub fn seek(&self, to_seconds: f64) {
        let target = if to_seconds.is_finite() && to_seconds >= 0.0 {
            to_seconds
        } else {
            0.0
        };
        let position = gst::ClockTime::from_nseconds((target * 1_000_000_000.0) as u64);
        if let Err(error) = self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, position)
        {
            tracing::warn!(target: "mezon_video", %error, "gstreamer seek failed");
        }
    }

    pub fn set_volume(&self, volume: f32) {
        let target = (volume as f64).clamp(0.0, 1.0);
        self.playbin.set_property("volume", target);
    }

    pub fn volume(&self) -> f32 {
        self.playbin.property::<f64>("volume") as f32
    }

    pub fn set_muted(&self, muted: bool) {
        self.playbin.set_property("mute", muted);
    }

    pub fn is_muted(&self) -> bool {
        self.playbin.property::<bool>("mute")
    }

    pub fn failed(&self) -> bool {
        self.drain_bus();
        self.failed.get()
    }
}

impl Drop for PlayerImpl {
    fn drop(&mut self) {
        if let Err(error) = self.playbin.set_state(gst::State::Null) {
            tracing::warn!(target: "mezon_video", %error, "gstreamer teardown failed");
        }
    }
}

fn auto_orientation_filter() -> Option<gst::Element> {
    let filter = match gst::ElementFactory::make("videoflip").build() {
        Ok(filter) => filter,
        Err(error) => {
            tracing::warn!(
                target: "mezon_video",
                %error,
                "no videoflip element: a rotated video plays as it was encoded"
            );
            return None;
        }
    };
    if filter.find_property(VIDEO_DIRECTION_PROPERTY).is_none() {
        tracing::warn!(
            target: "mezon_video",
            "videoflip has no {VIDEO_DIRECTION_PROPERTY}: a rotated video plays as it was encoded"
        );
        return None;
    }
    filter.set_property(
        VIDEO_DIRECTION_PROPERTY,
        gst_video::VideoOrientationMethod::Auto,
    );
    Some(filter)
}

fn turn_from_tag(orientation: &str) -> Option<Turn> {
    let (mirrored, rotation) = match orientation.strip_prefix("flip-") {
        Some(rotation) => (true, rotation),
        None => (false, orientation),
    };
    let quarter_turns = match rotation {
        "rotate-0" => 0,
        "rotate-90" => 1,
        "rotate-180" => 2,
        "rotate-270" => 3,
        _ => return None,
    };
    Some(Turn {
        quarter_turns,
        mirrored,
    })
}

fn clock_time_to_seconds(time: gst::ClockTime) -> f64 {
    time.nseconds() as f64 / 1_000_000_000.0
}

const VIDEO_PAD_SIGNAL: &str = "get-video-pad";
const VIDEO_TAGS_SIGNAL: &str = "get-video-tags";
const VIDEO_DIRECTION_PROPERTY: &str = "video-direction";
const POSTER_TIME: gst::ClockTime = gst::ClockTime::from_seconds(1);
const PROBE_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);

pub fn probe_video(path: &str, max_poster_edge: u32) -> Option<crate::VideoProbe> {
    if path.is_empty() {
        return None;
    }
    let probe = gstreamer_probe(path, max_poster_edge);
    if probe.as_ref().is_some_and(|p| p.poster_jpeg.is_some()) {
        return probe;
    }
    crate::poster_fallback::probe_without_decoder(path, max_poster_edge).or(probe)
}

fn gstreamer_probe(path: &str, max_poster_edge: u32) -> Option<crate::VideoProbe> {
    ensure_gstreamer().ok()?;
    let uri = match gst::glib::filename_to_uri(path, None) {
        Ok(uri) => uri,
        Err(error) => {
            tracing::warn!(target: "mezon_video", %error, "video probe path is not a uri");
            return None;
        }
    };
    let pipeline = PosterPipeline::open(uri.as_str(), max_poster_edge)?;
    let probe = pipeline.probe(
        max_poster_edge,
        crate::poster_fallback::container_turn(path),
    );
    if probe.is_none() {
        report_bus_errors(&pipeline.playbin, "video probe");
        tracing::warn!(target: "mezon_video", "video probe produced no frame");
    }
    probe
}

fn log_pipeline_error(error: &gst::message::Error, what: &str) {
    let detail = error.debug().unwrap_or_default();
    tracing::warn!(
        target: "mezon_video",
        error = %error.error(),
        detail = %detail,
        "gstreamer {what} failed"
    );
    if error.error().matches(gst::CoreError::MissingPlugin) {
        tracing::warn!(
            target: "mezon_video",
            "no gstreamer decoder installed for this video: install gstreamer1.0-libav \
             (Debian/Ubuntu), gstreamer1-plugin-libav (Fedora) or gst-libav (Arch)"
        );
    }
}

fn report_bus_errors(element: &gst::Element, what: &str) {
    let Some(bus) = element.bus() else {
        return;
    };
    while let Some(message) = bus.pop() {
        if let gst::message::MessageView::Error(error) = message.view() {
            log_pipeline_error(error, what);
        }
    }
}

struct PosterPipeline {
    playbin: gst::Element,
    appsink: gst_app::AppSink,
    prescaled: bool,
    flipped: bool,
}

const PRESCALE_HEADROOM: u32 = 2;

fn prescale_edge(playbin: &gst::Element, max_poster_edge: u32) -> Option<i32> {
    gst::glib::subclass::SignalId::lookup(VIDEO_PAD_SIGNAL, playbin.type_())?;
    let edge = max_poster_edge.checked_mul(PRESCALE_HEADROOM)?;
    i32::try_from(edge).ok().filter(|edge| *edge > 0)
}

impl PosterPipeline {
    fn open(uri: &str, max_poster_edge: u32) -> Option<Self> {
        let playbin = gst::ElementFactory::make("playbin").build().ok()?;
        let prescale_edge = prescale_edge(&playbin, max_poster_edge);
        let mut caps = gst_video::VideoCapsBuilder::new().format(gst_video::VideoFormat::Bgra);
        if let Some(edge) = prescale_edge {
            caps = caps
                .width_range(1..=edge)
                .height_range(1..=edge)
                .pixel_aspect_ratio(gst::Fraction::new(1, 1));
        }
        let appsink = gst_app::AppSink::builder()
            .caps(&caps.build())
            .max_buffers(1)
            .drop(false)
            .sync(false)
            .build();
        let audio_sink = gst::ElementFactory::make("fakesink").build().ok()?;
        playbin.set_property("uri", uri);
        playbin.set_property("video-sink", &appsink);
        playbin.set_property("audio-sink", &audio_sink);
        let filter = auto_orientation_filter();
        if let Some(filter) = filter.as_ref() {
            playbin.set_property("video-filter", filter);
        }
        Some(Self {
            playbin,
            appsink,
            prescaled: prescale_edge.is_some(),
            flipped: filter.is_some(),
        })
    }

    fn probe(&self, max_poster_edge: u32, container: Option<Turn>) -> Option<crate::VideoProbe> {
        self.playbin.set_state(gst::State::Paused).ok()?;
        self.playbin.state(PROBE_TIMEOUT).0.ok()?;
        if self.playbin.query_duration::<gst::ClockTime>() > Some(POSTER_TIME)
            && self
                .playbin
                .seek_simple(
                    gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                    POSTER_TIME,
                )
                .is_ok()
        {
            self.playbin.state(PROBE_TIMEOUT).0.ok()?;
        }
        let sample = self.appsink.try_pull_preroll(PROBE_TIMEOUT)?;
        let buffer = sample.buffer()?;
        let info = gst_video::VideoInfo::from_caps(sample.caps()?).ok()?;
        let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;
        let width = frame.width();
        let height = frame.height();
        let stride = usize::try_from(*frame.plane_stride().first()?).ok()?;
        let tagged = self.tagged_turn();
        let display = container
            .filter(|container| !container.is_none())
            .or(tagged)
            .unwrap_or_default();
        let ours = if self.flipped && tagged.is_some() {
            Turn::default()
        } else {
            display
        };
        let poster_jpeg = crate::poster::encode_poster_jpeg(
            frame.plane_data(0).ok()?,
            width,
            height,
            stride,
            false,
            ours,
            max_poster_edge,
        );
        let (width, height) = if self.prescaled {
            display.applied_to(self.natural_size()?)
        } else {
            ours.applied_to((width, height))
        };
        Some(crate::VideoProbe {
            width,
            height,
            poster_jpeg,
        })
    }

    fn natural_size(&self) -> Option<(u32, u32)> {
        let pad = self
            .playbin
            .emit_by_name::<Option<gst::Pad>>(VIDEO_PAD_SIGNAL, &[&0i32])?;
        let caps = pad.current_caps()?;
        let info = gst_video::VideoInfo::from_caps(&caps).ok()?;
        Some((info.width(), info.height()))
    }

    fn tagged_turn(&self) -> Option<Turn> {
        self.video_tags()?
            .get::<gst::tags::ImageOrientation>()
            .and_then(|orientation| turn_from_tag(orientation.get()))
    }

    fn video_tags(&self) -> Option<gst::TagList> {
        gst::glib::subclass::SignalId::lookup(VIDEO_TAGS_SIGNAL, self.playbin.type_())?;
        self.playbin
            .emit_by_name::<Option<gst::TagList>>(VIDEO_TAGS_SIGNAL, &[&0i32])
    }
}

impl Drop for PosterPipeline {
    fn drop(&mut self) {
        if let Err(error) = self.playbin.set_state(gst::State::Null) {
            tracing::warn!(target: "mezon_video", %error, "gstreamer probe teardown failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_orientation_tag_maps_to_a_mirror_and_a_clockwise_turn() {
        assert_eq!(turn_from_tag("rotate-0"), Some(Turn::clockwise(0)));
        assert_eq!(turn_from_tag("rotate-90"), Some(Turn::clockwise(1)));
        assert_eq!(turn_from_tag("rotate-180"), Some(Turn::clockwise(2)));
        assert_eq!(turn_from_tag("rotate-270"), Some(Turn::clockwise(3)));
        assert_eq!(
            turn_from_tag("flip-rotate-90"),
            Some(Turn {
                quarter_turns: 1,
                mirrored: true
            })
        );
        assert_eq!(
            turn_from_tag("flip-rotate-0"),
            Some(Turn {
                quarter_turns: 0,
                mirrored: true
            })
        );
        assert_eq!(turn_from_tag("rotate-45"), None);
        assert_eq!(turn_from_tag(""), None);
    }

    #[test]
    fn the_filter_follows_the_stream_orientation() {
        if ensure_gstreamer().is_err() {
            return;
        }
        let Some(filter) = auto_orientation_filter() else {
            return;
        };
        assert_eq!(
            filter.property::<gst_video::VideoOrientationMethod>(VIDEO_DIRECTION_PROPERTY),
            gst_video::VideoOrientationMethod::Auto
        );
    }
}
