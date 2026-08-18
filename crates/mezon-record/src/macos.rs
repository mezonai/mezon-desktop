use std::ffi::{CString, c_void};
use std::path::Path;
use std::ptr;
use std::time::Duration;

use objc::rc::StrongPtr;
use objc::runtime::{BOOL, NO, Object, YES};
use objc::{class, msg_send, sel, sel_impl};

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

const K_CV_PIXEL_FORMAT_NV12_FULL_RANGE: u32 = 0x34_32_30_66;
const K_CV_PIXEL_FORMAT_32_BGRA: u32 = 0x42_47_52_41;
const K_AUDIO_FORMAT_MPEG4_AAC: u32 = 0x61_61_63_20;
const K_AUDIO_FORMAT_FLAG_SIGNED_INTEGER: u32 = 1 << 2;
const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;
const K_CM_BLOCK_BUFFER_ASSURE_MEMORY_NOW: u32 = 1;
const VIDEO_BITRATE: i32 = 2_500_000;
const AUDIO_BITRATE: i32 = 128_000;
const FRAGMENT_SECONDS: i64 = 1;
const AV_ASSET_WRITER_STATUS_FAILED: isize = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct CmTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

unsafe impl objc::Encode for CmTime {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{?=qiIq}") }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMAudioFormatDescriptionCreate(
        allocator: *const c_void,
        asbd: *const AudioStreamBasicDescription,
        layout_size: usize,
        layout: *const c_void,
        magic_cookie_size: usize,
        magic_cookie: *const c_void,
        extensions: *const c_void,
        format_out: *mut *mut c_void,
    ) -> i32;
    fn CMBlockBufferCreateWithMemoryBlock(
        structure_allocator: *const c_void,
        memory_block: *mut c_void,
        block_length: usize,
        block_allocator: *const c_void,
        custom_block_source: *const c_void,
        offset_to_data: usize,
        data_length: usize,
        flags: u32,
        block_out: *mut *mut c_void,
    ) -> i32;
    fn CMBlockBufferReplaceDataBytes(
        source_bytes: *const c_void,
        destination: *mut c_void,
        offset: usize,
        length: usize,
    ) -> i32;
    fn CMAudioSampleBufferCreateWithPacketDescriptions(
        allocator: *const c_void,
        data_buffer: *mut c_void,
        data_ready: u8,
        make_data_ready_callback: *const c_void,
        make_data_ready_refcon: *mut c_void,
        format_description: *mut c_void,
        num_samples: isize,
        presentation_time: CmTime,
        packet_descriptions: *const c_void,
        sample_out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferCreate(
        allocator: *const c_void,
        width: usize,
        height: usize,
        pixel_format: u32,
        attributes: *const c_void,
        buffer_out: *mut *mut c_void,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(buffer: *mut c_void, plane: usize) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRowOfPlane(buffer: *mut c_void, plane: usize) -> usize;
    fn CVPixelBufferPoolCreatePixelBuffer(
        allocator: *const c_void,
        pool: *mut c_void,
        buffer_out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVFileTypeMPEG4: *const Object;
    static AVMediaTypeVideo: *const Object;
    static AVMediaTypeAudio: *const Object;
    static AVVideoCodecKey: *const Object;
    static AVVideoWidthKey: *const Object;
    static AVVideoHeightKey: *const Object;
    static AVVideoCompressionPropertiesKey: *const Object;
    static AVVideoAverageBitRateKey: *const Object;
    static AVVideoExpectedSourceFrameRateKey: *const Object;
    static AVVideoMaxKeyFrameIntervalKey: *const Object;
    static AVVideoAllowFrameReorderingKey: *const Object;
    static AVVideoCodecTypeH264: *const Object;
    static AVFormatIDKey: *const Object;
    static AVNumberOfChannelsKey: *const Object;
    static AVSampleRateKey: *const Object;
    static AVEncoderBitRateKey: *const Object;
}

unsafe fn ns_string_to_rust(value: *mut Object) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe {
        let utf8: *const std::os::raw::c_char = msg_send![value, UTF8String];
        if utf8.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(utf8)
            .to_string_lossy()
            .into_owned()
    }
}

unsafe fn ns_string(value: &str) -> *mut Object {
    let Ok(raw) = CString::new(value) else {
        return ptr::null_mut();
    };
    unsafe { msg_send![class!(NSString), stringWithUTF8String: raw.as_ptr()] }
}

unsafe fn ns_number_i32(value: i32) -> *mut Object {
    unsafe { msg_send![class!(NSNumber), numberWithInt: value] }
}

unsafe fn ns_number_f64(value: f64) -> *mut Object {
    unsafe { msg_send![class!(NSNumber), numberWithDouble: value] }
}

unsafe fn ns_dictionary(pairs: &[(*const Object, *mut Object)]) -> *mut Object {
    let keys: Vec<*const Object> = pairs.iter().map(|(key, _)| *key).collect();
    let values: Vec<*mut Object> = pairs.iter().map(|(_, value)| *value).collect();
    unsafe {
        msg_send![
            class!(NSDictionary),
            dictionaryWithObjects: values.as_ptr()
            forKeys: keys.as_ptr()
            count: pairs.len()
        ]
    }
}

fn cm_time(seconds: i64, timescale: i32) -> CmTime {
    CmTime {
        value: seconds,
        timescale,
        flags: 1,
        epoch: 0,
    }
}

fn cm_time_from_duration(pts: Duration) -> CmTime {
    CmTime {
        value: pts.as_nanos() as i64,
        timescale: 1_000_000_000,
        flags: 1,
        epoch: 0,
    }
}

struct AutoreleasePool(*mut Object);

impl AutoreleasePool {
    fn new() -> Self {
        let pool: *mut Object = unsafe {
            let pool: *mut Object = msg_send![class!(NSAutoreleasePool), alloc];
            msg_send![pool, init]
        };
        Self(pool)
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _: () = unsafe { msg_send![self.0, drain] };
        }
    }
}

struct AudioFormatDescription(*mut c_void);

impl Drop for AudioFormatDescription {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

unsafe impl Send for AudioFormatDescription {}

pub struct MacSink {
    writer: StrongPtr,
    video_input: Option<StrongPtr>,
    adaptor: Option<StrongPtr>,
    audio_input: StrongPtr,
    audio_format: AudioFormatDescription,
    video: Option<VideoConfig>,
    session_started: bool,
}

unsafe impl Send for MacSink {}

pub fn create_sink(
    path: &Path,
    video: Option<VideoConfig>,
) -> Result<Box<dyn RecordSink>, RecordError> {
    unsafe { MacSink::new(path, video) }.map(|sink| Box::new(sink) as Box<dyn RecordSink>)
}

impl MacSink {
    unsafe fn new(path: &Path, video: Option<VideoConfig>) -> Result<Self, RecordError> {
        unsafe {
            let path_string = ns_string(&path.to_string_lossy());
            if path_string.is_null() {
                return Err(RecordError::Create);
            }
            let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: path_string];
            if url.is_null() {
                return Err(RecordError::Create);
            }

            let writer: *mut Object = msg_send![class!(AVAssetWriter), alloc];
            let writer: *mut Object = msg_send![
                writer,
                initWithURL: url
                fileType: AVFileTypeMPEG4
                error: ptr::null_mut::<*mut Object>()
            ];
            if writer.is_null() {
                return Err(RecordError::Create);
            }
            let writer = StrongPtr::new(writer);
            let _: () = msg_send![
                *writer,
                setMovieFragmentInterval: cm_time(FRAGMENT_SECONDS, 1)
            ];

            let (video_input, adaptor) = match video {
                Some(config) => {
                    let no: *mut Object = msg_send![class!(NSNumber), numberWithBool: NO];
                    let fps = config.fps.max(1) as i32;
                    let compression = ns_dictionary(&[
                        (AVVideoAverageBitRateKey, ns_number_i32(VIDEO_BITRATE)),
                        (AVVideoExpectedSourceFrameRateKey, ns_number_i32(fps)),
                        (AVVideoMaxKeyFrameIntervalKey, ns_number_i32(fps * 2)),
                        (AVVideoAllowFrameReorderingKey, no),
                    ]);
                    let settings = ns_dictionary(&[
                        (AVVideoCodecKey, AVVideoCodecTypeH264 as *mut Object),
                        (AVVideoWidthKey, ns_number_i32(config.width as i32)),
                        (AVVideoHeightKey, ns_number_i32(config.height as i32)),
                        (AVVideoCompressionPropertiesKey, compression),
                    ]);
                    let input: *mut Object = msg_send![
                        class!(AVAssetWriterInput),
                        assetWriterInputWithMediaType: AVMediaTypeVideo
                        outputSettings: settings
                    ];
                    if input.is_null() {
                        return Err(RecordError::Create);
                    }
                    let _: () = msg_send![input, setExpectsMediaDataInRealTime: YES];

                    let empty: *mut Object = msg_send![class!(NSDictionary), dictionary];
                    let attributes = ns_dictionary(&[
                        (
                            ns_string("PixelFormatType") as *const Object,
                            ns_number_i32(K_CV_PIXEL_FORMAT_NV12_FULL_RANGE as i32),
                        ),
                        (
                            ns_string("Width") as *const Object,
                            ns_number_i32(config.width as i32),
                        ),
                        (
                            ns_string("Height") as *const Object,
                            ns_number_i32(config.height as i32),
                        ),
                        (ns_string("IOSurfaceProperties") as *const Object, empty),
                    ]);
                    let adaptor: *mut Object = msg_send![
                        class!(AVAssetWriterInputPixelBufferAdaptor),
                        assetWriterInputPixelBufferAdaptorWithAssetWriterInput: input
                        sourcePixelBufferAttributes: attributes
                    ];
                    let can_add: BOOL = msg_send![*writer, canAddInput: input];
                    if can_add == NO {
                        return Err(RecordError::Create);
                    }
                    let _: () = msg_send![*writer, addInput: input];
                    (
                        Some(StrongPtr::retain(input)),
                        Some(StrongPtr::retain(adaptor)),
                    )
                }
                None => (None, None),
            };

            let audio_settings = ns_dictionary(&[
                (
                    AVFormatIDKey,
                    ns_number_i32(K_AUDIO_FORMAT_MPEG4_AAC as i32),
                ),
                (AVNumberOfChannelsKey, ns_number_i32(RECORD_CHANNELS as i32)),
                (AVSampleRateKey, ns_number_f64(RECORD_SAMPLE_RATE as f64)),
                (AVEncoderBitRateKey, ns_number_i32(AUDIO_BITRATE)),
            ]);
            let audio_input: *mut Object = msg_send![
                class!(AVAssetWriterInput),
                assetWriterInputWithMediaType: AVMediaTypeAudio
                outputSettings: audio_settings
            ];
            if audio_input.is_null() {
                return Err(RecordError::Create);
            }
            let _: () = msg_send![audio_input, setExpectsMediaDataInRealTime: YES];
            let can_add: BOOL = msg_send![*writer, canAddInput: audio_input];
            if can_add == NO {
                return Err(RecordError::Create);
            }
            let _: () = msg_send![*writer, addInput: audio_input];

            let asbd = AudioStreamBasicDescription {
                sample_rate: RECORD_SAMPLE_RATE as f64,
                format_id: u32::from_be_bytes(*b"lpcm"),
                format_flags: K_AUDIO_FORMAT_FLAG_SIGNED_INTEGER | K_AUDIO_FORMAT_FLAG_IS_PACKED,
                bytes_per_packet: 2 * RECORD_CHANNELS,
                frames_per_packet: 1,
                bytes_per_frame: 2 * RECORD_CHANNELS,
                channels_per_frame: RECORD_CHANNELS,
                bits_per_channel: 16,
                reserved: 0,
            };
            let mut format: *mut c_void = ptr::null_mut();
            if CMAudioFormatDescriptionCreate(
                ptr::null(),
                &asbd,
                0,
                ptr::null(),
                0,
                ptr::null(),
                ptr::null(),
                &mut format,
            ) != 0
                || format.is_null()
            {
                return Err(RecordError::Create);
            }

            let started: BOOL = msg_send![*writer, startWriting];
            if started == NO {
                CFRelease(format);
                return Err(RecordError::Create);
            }

            Ok(Self {
                writer,
                video_input,
                adaptor,
                audio_input: StrongPtr::retain(audio_input),
                audio_format: AudioFormatDescription(format),
                video,
                session_started: false,
            })
        }
    }

    unsafe fn ensure_session(&mut self, pts: Duration) {
        if self.session_started {
            return;
        }
        unsafe {
            let _: () =
                msg_send![*self.writer, startSessionAtSourceTime: cm_time_from_duration(pts)];
        }
        self.session_started = true;
    }

    fn writer_failed(&self) -> bool {
        let status: isize = unsafe { msg_send![*self.writer, status] };
        if status == AV_ASSET_WRITER_STATUS_FAILED {
            tracing::error!("AVAssetWriter failed: {}", self.writer_error());
            return true;
        }
        false
    }

    fn writer_error(&self) -> String {
        unsafe {
            let error: *mut Object = msg_send![*self.writer, error];
            if error.is_null() {
                return "no error object".into();
            }
            let description: *mut Object = msg_send![error, localizedDescription];
            let domain: *mut Object = msg_send![error, domain];
            let code: isize = msg_send![error, code];
            let user_info: *mut Object = msg_send![error, userInfo];
            let info_text: *mut Object = msg_send![user_info, description];
            format!(
                "{} code={code} domain={} info={}",
                ns_string_to_rust(description),
                ns_string_to_rust(domain),
                ns_string_to_rust(info_text)
            )
        }
    }

    unsafe fn pixel_buffer_from_frame(
        &self,
        frame: &VideoFrameRef<'_>,
    ) -> Option<(*mut c_void, bool)> {
        match frame.data {
            PixelData::Nv12 {
                y,
                y_stride,
                uv,
                uv_stride,
            } => unsafe {
                let buffer = self.pooled_pixel_buffer().or_else(|| {
                    self.create_pixel_buffer(
                        frame.width,
                        frame.height,
                        K_CV_PIXEL_FORMAT_NV12_FULL_RANGE,
                    )
                })?;
                CVPixelBufferLockBaseAddress(buffer, 0);
                copy_plane(
                    CVPixelBufferGetBaseAddressOfPlane(buffer, 0),
                    CVPixelBufferGetBytesPerRowOfPlane(buffer, 0),
                    y,
                    y_stride,
                    frame.width as usize,
                    frame.height as usize,
                );
                copy_plane(
                    CVPixelBufferGetBaseAddressOfPlane(buffer, 1),
                    CVPixelBufferGetBytesPerRowOfPlane(buffer, 1),
                    uv,
                    uv_stride,
                    frame.width as usize,
                    frame.height as usize / 2,
                );
                CVPixelBufferUnlockBaseAddress(buffer, 0);
                Some((buffer, true))
            },
            PixelData::Bgra { data, stride } => unsafe {
                let buffer =
                    self.create_pixel_buffer(frame.width, frame.height, K_CV_PIXEL_FORMAT_32_BGRA)?;
                CVPixelBufferLockBaseAddress(buffer, 0);
                copy_plane(
                    CVPixelBufferGetBaseAddress(buffer),
                    CVPixelBufferGetBytesPerRow(buffer),
                    data,
                    stride,
                    frame.width as usize * 4,
                    frame.height as usize,
                );
                CVPixelBufferUnlockBaseAddress(buffer, 0);
                Some((buffer, true))
            },
        }
    }

    unsafe fn pooled_pixel_buffer(&self) -> Option<*mut c_void> {
        let adaptor = self.adaptor.as_ref()?;
        unsafe {
            let pool: *mut c_void = msg_send![**adaptor, pixelBufferPool];
            if pool.is_null() {
                return None;
            }
            let mut buffer: *mut c_void = ptr::null_mut();
            let status = CVPixelBufferPoolCreatePixelBuffer(ptr::null(), pool, &mut buffer);
            (status == 0 && !buffer.is_null()).then_some(buffer)
        }
    }

    unsafe fn create_pixel_buffer(
        &self,
        width: u32,
        height: u32,
        format: u32,
    ) -> Option<*mut c_void> {
        let mut buffer: *mut c_void = ptr::null_mut();
        let status = unsafe {
            CVPixelBufferCreate(
                ptr::null(),
                width as usize,
                height as usize,
                format,
                ptr::null(),
                &mut buffer,
            )
        };
        (status == 0 && !buffer.is_null()).then_some(buffer)
    }
}

unsafe fn copy_plane(
    dst: *mut c_void,
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    row_bytes: usize,
    rows: usize,
) {
    if dst.is_null() || dst_stride == 0 || src_stride == 0 {
        return;
    }
    let copy_bytes = row_bytes.min(dst_stride).min(src_stride);
    for row in 0..rows {
        let src_start = row * src_stride;
        if src_start + copy_bytes > src.len() {
            break;
        }
        unsafe {
            ptr::copy_nonoverlapping(
                src.as_ptr().add(src_start),
                (dst as *mut u8).add(row * dst_stride),
                copy_bytes,
            );
        }
    }
}

impl RecordSink for MacSink {
    fn push_audio(&mut self, pcm: &[i16], pts: Duration) -> Result<(), RecordError> {
        let _pool = AutoreleasePool::new();
        if self.writer_failed() {
            return Err(RecordError::Encode);
        }
        unsafe {
            self.ensure_session(pts);

            let ready: BOOL = msg_send![*self.audio_input, isReadyForMoreMediaData];
            if ready == NO {
                return Ok(());
            }

            let byte_len = std::mem::size_of_val(pcm);
            let mut block: *mut c_void = ptr::null_mut();
            if CMBlockBufferCreateWithMemoryBlock(
                ptr::null(),
                ptr::null_mut(),
                byte_len,
                ptr::null(),
                ptr::null(),
                0,
                byte_len,
                K_CM_BLOCK_BUFFER_ASSURE_MEMORY_NOW,
                &mut block,
            ) != 0
                || block.is_null()
            {
                return Err(RecordError::Encode);
            }
            if CMBlockBufferReplaceDataBytes(pcm.as_ptr() as *const c_void, block, 0, byte_len) != 0
            {
                CFRelease(block);
                return Err(RecordError::Encode);
            }

            let mut sample: *mut c_void = ptr::null_mut();
            let frames = (pcm.len() / RECORD_CHANNELS as usize) as isize;
            let status = CMAudioSampleBufferCreateWithPacketDescriptions(
                ptr::null(),
                block,
                1,
                ptr::null(),
                ptr::null_mut(),
                self.audio_format.0,
                frames,
                cm_time_from_duration(pts),
                ptr::null(),
                &mut sample,
            );
            CFRelease(block);
            if status != 0 || sample.is_null() {
                return Err(RecordError::Encode);
            }

            let appended: BOOL = msg_send![*self.audio_input, appendSampleBuffer: sample];
            CFRelease(sample);
            if appended == NO && self.writer_failed() {
                return Err(RecordError::Encode);
            }
        }
        Ok(())
    }

    fn push_video(&mut self, frame: VideoFrameRef<'_>, pts: Duration) -> Result<(), RecordError> {
        let _pool = AutoreleasePool::new();
        if self.video.is_none() || self.video_input.is_none() || self.adaptor.is_none() {
            return Ok(());
        }
        if self.writer_failed() {
            return Err(RecordError::Encode);
        }
        if let Some(config) = self.video
            && (frame.width != config.width || frame.height != config.height)
        {
            tracing::warn!(
                "skipping a {}x{} frame: the recording is locked to {}x{}",
                frame.width,
                frame.height,
                config.width,
                config.height
            );
            return Ok(());
        }
        unsafe {
            self.ensure_session(pts);
            let (Some(input), Some(adaptor)) = (&self.video_input, &self.adaptor) else {
                return Ok(());
            };
            let ready: BOOL = msg_send![**input, isReadyForMoreMediaData];
            if ready == NO {
                return Ok(());
            }
            let Some((buffer, owned)) = self.pixel_buffer_from_frame(&frame) else {
                return Ok(());
            };
            let appended: BOOL = msg_send![
                **adaptor,
                appendPixelBuffer: buffer
                withPresentationTime: cm_time_from_duration(pts)
            ];
            if owned {
                CFRelease(buffer);
            }
            if appended == NO && self.writer_failed() {
                return Err(RecordError::Encode);
            }
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<(), RecordError> {
        let _pool = AutoreleasePool::new();
        unsafe {
            if let Some(input) = &self.video_input {
                let _: () = msg_send![**input, markAsFinished];
            }
            let _: () = msg_send![*self.audio_input, markAsFinished];
            let _: () = msg_send![*self.writer, finishWriting];

            if self.writer_failed() {
                return Err(RecordError::Finish);
            }
        }
        Ok(())
    }
}
