use std::path::Path;
use std::sync::Once;
use std::time::Duration;

use windows::Win32::Media::MediaFoundation::{
    IMFSinkWriter, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION, MFAudioFormat_AAC, MFAudioFormat_PCM,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Audio, MFMediaType_Video, MFSTARTUP_FULL, MFStartup,
    MFTranscodeContainerType_MPEG4, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, eAVEncH264VProfile_Main,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::HSTRING;
use yuv::{YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix, bgra_to_yuv_nv12};

use crate::sink::{
    PixelData, RECORD_CHANNELS, RECORD_SAMPLE_RATE, RecordError, RecordSink, VideoConfig,
    VideoFrameRef,
};

pub fn is_supported() -> bool {
    true
}

pub fn file_extension() -> &'static str {
    "mp4"
}

const VIDEO_BITRATE: u32 = 2_500_000;
const AUDIO_BYTES_PER_SECOND: u32 = 16_000;
const BYTES_PER_AUDIO_FRAME: u32 = 2 * RECORD_CHANNELS;

static MF_INIT: Once = Once::new();

fn ensure_media_foundation() {
    MF_INIT.call_once(|| unsafe {
        if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            tracing::error!("MFStartup failed for call recording: {error}");
        }
    });
}

struct Apartment(bool);

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

thread_local! {
    static APARTMENT: Apartment = Apartment(unsafe {
        // RPC_E_CHANGED_MODE means the thread already lives in an apartment we
        // can call from, so only undo the ones we actually joined.
        CoInitializeEx(None, COINIT_MULTITHREADED).is_ok()
    });
}

/// The sink writer is created on one background thread and then fed from the
/// audio worker and the compositor, and every one of those calls goes straight
/// into COM. A thread that never joined an apartment can fail its very first
/// `WriteSample`, which leaves a header-only MP4 behind, so join before calling.
fn ensure_thread_apartment() {
    APARTMENT.with(|_| ());
}

fn pack_ratio(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

fn hundred_nanos(pts: Duration) -> i64 {
    (pts.as_nanos() / 100) as i64
}

pub struct WindowsSink {
    writer: IMFSinkWriter,
    video_stream: Option<u32>,
    audio_stream: u32,
    video: Option<VideoConfig>,
    nv12: Vec<u8>,
    audio_samples: u64,
    video_samples: u64,
    dropped_frames: u64,
    finished: bool,
}

unsafe impl Send for WindowsSink {}

pub fn create_sink(
    path: &Path,
    video: Option<VideoConfig>,
) -> Result<Box<dyn RecordSink>, RecordError> {
    WindowsSink::new(path, video).map(|sink| Box::new(sink) as Box<dyn RecordSink>)
}

impl WindowsSink {
    fn new(path: &Path, video: Option<VideoConfig>) -> Result<Self, RecordError> {
        ensure_media_foundation();
        ensure_thread_apartment();
        unsafe {
            let target = HSTRING::from(path.as_os_str());
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).map_err(|_| RecordError::Create)?;
            let attributes = attributes.ok_or(RecordError::Create)?;
            attributes
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
                .map_err(|_| RecordError::Create)?;
            let writer: IMFSinkWriter = MFCreateSinkWriterFromURL(&target, None, &attributes)
                .map_err(|error| {
                    tracing::error!("could not create the call recording sink writer: {error}");
                    RecordError::Create
                })?;

            let video_stream = match video {
                Some(config) => {
                    let out_type = MFCreateMediaType().map_err(|_| RecordError::Create)?;
                    out_type
                        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                        .map_err(|_| RecordError::Create)?;
                    out_type
                        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                        .map_err(|_| RecordError::Create)?;
                    out_type
                        .SetUINT32(&MF_MT_AVG_BITRATE, VIDEO_BITRATE)
                        .map_err(|_| RecordError::Create)?;
                    out_type
                        .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)
                        .map_err(|_| RecordError::Create)?;
                    out_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                        .map_err(|_| RecordError::Create)?;
                    out_type
                        .SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(config.width, config.height))
                        .map_err(|_| RecordError::Create)?;
                    out_type
                        .SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(config.fps, 1))
                        .map_err(|_| RecordError::Create)?;
                    out_type
                        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_ratio(1, 1))
                        .map_err(|_| RecordError::Create)?;
                    let stream = writer.AddStream(&out_type).map_err(|error| {
                        tracing::error!("call recorder could not add the H.264 stream: {error}");
                        RecordError::Create
                    })?;

                    let in_type = MFCreateMediaType().map_err(|_| RecordError::Create)?;
                    in_type
                        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                        .map_err(|_| RecordError::Create)?;
                    in_type
                        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                        .map_err(|_| RecordError::Create)?;
                    in_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                        .map_err(|_| RecordError::Create)?;
                    in_type
                        .SetUINT64(&MF_MT_FRAME_SIZE, pack_ratio(config.width, config.height))
                        .map_err(|_| RecordError::Create)?;
                    in_type
                        .SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(config.fps, 1))
                        .map_err(|_| RecordError::Create)?;
                    in_type
                        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_ratio(1, 1))
                        .map_err(|_| RecordError::Create)?;
                    writer
                        .SetInputMediaType(stream, &in_type, None)
                        .map_err(|error| {
                            tracing::error!("call recorder rejected the NV12 input type: {error}");
                            RecordError::Create
                        })?;
                    Some(stream)
                }
                None => None,
            };

            let audio_out = MFCreateMediaType().map_err(|_| RecordError::Create)?;
            audio_out
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|_| RecordError::Create)?;
            audio_out
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
                .map_err(|_| RecordError::Create)?;
            audio_out
                .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                .map_err(|_| RecordError::Create)?;
            audio_out
                .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RECORD_SAMPLE_RATE)
                .map_err(|_| RecordError::Create)?;
            audio_out
                .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, RECORD_CHANNELS)
                .map_err(|_| RecordError::Create)?;
            audio_out
                .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_BYTES_PER_SECOND)
                .map_err(|_| RecordError::Create)?;
            let audio_stream = writer.AddStream(&audio_out).map_err(|error| {
                tracing::error!("call recorder could not add the AAC stream: {error}");
                RecordError::Create
            })?;

            let audio_in = MFCreateMediaType().map_err(|_| RecordError::Create)?;
            audio_in
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|_| RecordError::Create)?;
            audio_in
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .map_err(|_| RecordError::Create)?;
            audio_in
                .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                .map_err(|_| RecordError::Create)?;
            audio_in
                .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RECORD_SAMPLE_RATE)
                .map_err(|_| RecordError::Create)?;
            audio_in
                .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, RECORD_CHANNELS)
                .map_err(|_| RecordError::Create)?;
            audio_in
                .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, BYTES_PER_AUDIO_FRAME)
                .map_err(|_| RecordError::Create)?;
            // An uncompressed audio type is only complete with its byte rate and
            // its independent-samples flag; without them the AAC encoder is free
            // to reject every sample we hand it later on.
            audio_in
                .SetUINT32(
                    &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                    RECORD_SAMPLE_RATE * BYTES_PER_AUDIO_FRAME,
                )
                .map_err(|_| RecordError::Create)?;
            audio_in
                .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
                .map_err(|_| RecordError::Create)?;
            writer
                .SetInputMediaType(audio_stream, &audio_in, None)
                .map_err(|error| {
                    tracing::error!("call recorder rejected the PCM input type: {error}");
                    RecordError::Create
                })?;

            writer.BeginWriting().map_err(|error| {
                tracing::error!("could not begin writing the call recording: {error}");
                RecordError::Create
            })?;

            Ok(Self {
                writer,
                video_stream,
                audio_stream,
                video,
                nv12: Vec::new(),
                audio_samples: 0,
                video_samples: 0,
                dropped_frames: 0,
                finished: false,
            })
        }
    }

    unsafe fn write(
        &self,
        stream: u32,
        bytes: &[u8],
        pts: Duration,
        duration: Duration,
    ) -> Result<(), RecordError> {
        unsafe {
            let buffer =
                MFCreateMemoryBuffer(bytes.len() as u32).map_err(|_| RecordError::Encode)?;
            let mut target: *mut u8 = std::ptr::null_mut();
            buffer
                .Lock(&mut target, None, None)
                .map_err(|_| RecordError::Encode)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            buffer.Unlock().map_err(|_| RecordError::Encode)?;
            buffer
                .SetCurrentLength(bytes.len() as u32)
                .map_err(|_| RecordError::Encode)?;

            let sample = MFCreateSample().map_err(|_| RecordError::Encode)?;
            sample.AddBuffer(&buffer).map_err(|_| RecordError::Encode)?;
            sample
                .SetSampleTime(hundred_nanos(pts))
                .map_err(|_| RecordError::Encode)?;
            sample
                .SetSampleDuration(hundred_nanos(duration))
                .map_err(|_| RecordError::Encode)?;
            self.writer.WriteSample(stream, &sample).map_err(|error| {
                tracing::error!(
                    "call recorder could not write a sample to stream {stream}: {error}"
                );
                RecordError::Encode
            })
        }
    }
}

impl RecordSink for WindowsSink {
    fn push_audio(&mut self, pcm: &[i16], pts: Duration) -> Result<(), RecordError> {
        ensure_thread_apartment();
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(pcm));
        for sample in pcm {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let frames = (pcm.len() / RECORD_CHANNELS as usize) as u64;
        let duration = Duration::from_nanos(frames * 1_000_000_000 / RECORD_SAMPLE_RATE as u64);
        unsafe { self.write(self.audio_stream, &bytes, pts, duration) }?;
        self.audio_samples += 1;

        // The MP4 sink interleaves by timestamp, so it holds every audio sample
        // in memory until the video stream reaches the same point — and writes
        // nothing at all to the file while it waits. Tell it the video stream is
        // simply not there yet so the muxer keeps draining.
        if let Some(stream) = self.video_stream
            && self.video_samples == 0
        {
            unsafe {
                let _ = self.writer.SendStreamTick(stream, hundred_nanos(pts));
            }
        }
        Ok(())
    }

    fn push_video(&mut self, frame: VideoFrameRef<'_>, pts: Duration) -> Result<(), RecordError> {
        ensure_thread_apartment();
        let (Some(stream), Some(config)) = (self.video_stream, self.video) else {
            return Ok(());
        };
        let PixelData::Bgra { data, stride } = frame.data else {
            return Ok(());
        };

        let width = config.width;
        let height = config.height;
        let y_len = (width * height) as usize;
        let uv_len = y_len / 2;
        self.nv12.resize(y_len + uv_len, 0);
        let (y_plane, uv_plane) = self.nv12.split_at_mut(y_len);

        let mut planar = YuvBiPlanarImageMut {
            y_plane: yuv::BufferStoreMut::Borrowed(y_plane),
            y_stride: width,
            uv_plane: yuv::BufferStoreMut::Borrowed(uv_plane),
            uv_stride: width,
            width,
            height,
        };
        if let Err(error) = bgra_to_yuv_nv12(
            &mut planar,
            data,
            stride as u32,
            YuvRange::Limited,
            YuvStandardMatrix::Bt709,
            YuvConversionMode::Balanced,
        ) {
            // Dropping every frame silently is how a recording ends up with no
            // video track at all, so say it once rather than never.
            if self.dropped_frames == 0 {
                tracing::error!("call recorder could not convert a frame to NV12: {error}");
            }
            self.dropped_frames += 1;
            return Ok(());
        }

        let duration = Duration::from_secs_f64(1.0 / config.fps.max(1) as f64);
        let bytes = std::mem::take(&mut self.nv12);
        let result = unsafe { self.write(stream, &bytes, pts, duration) };
        self.nv12 = bytes;
        result?;
        self.video_samples += 1;
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), RecordError> {
        ensure_thread_apartment();
        self.finished = true;
        unsafe {
            self.writer.Finalize().map_err(|error| {
                tracing::error!("could not finalize the call recording: {error}");
                RecordError::Finish
            })?
        };
        tracing::info!(
            "the call recording encoded {} audio and {} video samples ({} frames dropped)",
            self.audio_samples,
            self.video_samples,
            self.dropped_frames
        );
        // Media Foundation reports success for a finalize that wrote nothing but
        // the container header, and that file opens in no player anywhere.
        if self.audio_samples == 0 && self.video_samples == 0 {
            tracing::error!("the call recording was finalized without a single encoded sample");
            return Err(RecordError::Finish);
        }
        Ok(())
    }
}

impl Drop for WindowsSink {
    fn drop(&mut self) {
        if !self.finished {
            ensure_thread_apartment();
            let _ = unsafe { self.writer.Finalize() };
        }
    }
}
