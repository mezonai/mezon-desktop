use std::cell::Cell;
use std::ffi::{CString, c_void};
use std::ptr;

use core_foundation::base::TCFType;
use core_video::pixel_buffer::{
    CVPixelBuffer, CVPixelBufferRef, kCVPixelBufferHeightKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use objc::rc::StrongPtr;
use objc::runtime::{BOOL, NO, Object, YES};
use objc::{class, msg_send, sel, sel_impl};

use crate::{PlayerError, VideoProbe};

pub type VideoFrame = CVPixelBuffer;

#[repr(C)]
#[derive(Clone, Copy)]
struct CgSize {
    width: f64,
    height: f64,
}

unsafe impl objc::Encode for CgSize {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGSize=dd}") }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgAffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

unsafe impl objc::Encode for CgAffineTransform {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGAffineTransform=dddddd}") }
    }
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageDestinationCreateWithData(
        data: *mut c_void,
        uti: *const c_void,
        count: usize,
        options: *const c_void,
    ) -> *mut c_void;
    fn CGImageDestinationAddImage(dest: *mut c_void, image: *mut c_void, properties: *const c_void);
    fn CGImageDestinationFinalize(dest: *mut c_void) -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

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

const CM_TIME_FLAG_VALID: u32 = 1;
const CM_TIME_FLAG_INDEFINITE: u32 = 1 << 4;
const SEEK_TIMESCALE: i32 = 600;
const AV_PLAYER_ITEM_STATUS_READY_TO_PLAY: isize = 1;
const AV_PLAYER_ITEM_STATUS_FAILED: isize = 2;
const STALE_FRAME_BUDGET: u8 = 10;
const TRANSFORM_WAIT_POLLS: u8 = 30;

fn cm_time_to_seconds(time: CmTime) -> f64 {
    if time.flags & CM_TIME_FLAG_VALID == 0
        || time.flags & CM_TIME_FLAG_INDEFINITE != 0
        || time.timescale == 0
    {
        return 0.0;
    }
    let seconds = time.value as f64 / time.timescale as f64;
    if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    }
}

fn seconds_to_cm_time(seconds: f64) -> CmTime {
    let clamped = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    CmTime {
        value: (clamped * SEEK_TIMESCALE as f64).round() as i64,
        timescale: SEEK_TIMESCALE,
        flags: CM_TIME_FLAG_VALID,
        epoch: 0,
    }
}

pub struct PlayerImpl {
    player: StrongPtr,
    asset: StrongPtr,
    output: StrongPtr,
    max_size: Option<(u32, u32)>,
    transform_checked: Cell<bool>,
    transform_polls_left: Cell<u8>,
    upright_is_portrait: Cell<Option<bool>>,
    stale_frames_left: Cell<u8>,
}

impl PlayerImpl {
    pub fn open(url: &str, max_size: Option<(u32, u32)>) -> Result<Self, PlayerError> {
        let c_url = CString::new(url).map_err(|_| PlayerError::InvalidUrl)?;
        unsafe {
            let ns_string = class!(NSString);
            let url_string: *mut Object =
                msg_send![ns_string, stringWithUTF8String: c_url.as_ptr()];
            if url_string.is_null() {
                return Err(PlayerError::InvalidUrl);
            }
            let ns_url_class = class!(NSURL);
            let ns_url: *mut Object = msg_send![ns_url_class, URLWithString: url_string];
            if ns_url.is_null() {
                return Err(PlayerError::InvalidUrl);
            }

            let asset_class = class!(AVURLAsset);
            let asset_alloc: *mut Object = msg_send![asset_class, alloc];
            let asset: *mut Object =
                msg_send![asset_alloc, initWithURL: ns_url options: ptr::null::<Object>()];
            if asset.is_null() {
                return Err(PlayerError::Open);
            }
            let asset = StrongPtr::new(asset);

            let item_class = class!(AVPlayerItem);
            let item_alloc: *mut Object = msg_send![item_class, alloc];
            let item: *mut Object = msg_send![item_alloc, initWithAsset: *asset];
            if item.is_null() {
                return Err(PlayerError::Open);
            }
            let item = StrongPtr::new(item);

            let attributes = pixel_buffer_attributes(max_size)?;
            let output_class = class!(AVPlayerItemVideoOutput);
            let output_alloc: *mut Object = msg_send![output_class, alloc];
            let output: *mut Object =
                msg_send![output_alloc, initWithPixelBufferAttributes: *attributes];
            if output.is_null() {
                return Err(PlayerError::Open);
            }
            let output = StrongPtr::new(output);

            let _: () = msg_send![*item, addOutput: *output];

            let player_class = class!(AVPlayer);
            let player_alloc: *mut Object = msg_send![player_class, alloc];
            let player: *mut Object = msg_send![player_alloc, initWithPlayerItem: *item];
            if player.is_null() {
                return Err(PlayerError::Open);
            }
            let player = StrongPtr::new(player);

            Ok(Self {
                player,
                asset,
                output,
                max_size,
                transform_checked: Cell::new(false),
                transform_polls_left: Cell::new(TRANSFORM_WAIT_POLLS),
                upright_is_portrait: Cell::new(None),
                stale_frames_left: Cell::new(0),
            })
        }
    }

    pub fn copy_frame(&self) -> Option<VideoFrame> {
        self.apply_preferred_transform();
        if self.waiting_for_transform() {
            return None;
        }
        unsafe {
            let time: CmTime = msg_send![*self.player, currentTime];
            let has_new: BOOL = msg_send![*self.output, hasNewPixelBufferForItemTime: time];
            if has_new == NO {
                return None;
            }
            let buffer: CVPixelBufferRef = msg_send![
                *self.output,
                copyPixelBufferForItemTime: time
                itemTimeForDisplay: ptr::null_mut::<CmTime>()
            ];
            if buffer.is_null() {
                return None;
            }
            let frame = CVPixelBuffer::wrap_under_create_rule(buffer);
            if !frame_format_is_renderable(frame.get_pixel_format()) {
                return None;
            }
            if self.is_stale_pre_composition_frame(&frame) {
                return None;
            }
            Some(frame)
        }
    }

    fn apply_preferred_transform(&self) {
        if self.transform_checked.get() {
            return;
        }
        objc::rc::autoreleasepool(|| unsafe {
            let item = self.current_item();
            if item.is_null() {
                return;
            }
            let status: isize = msg_send![item, status];
            if status != AV_PLAYER_ITEM_STATUS_READY_TO_PLAY {
                return;
            }
            let Some(track) = first_video_track(*self.asset) else {
                return;
            };
            let transform = track_preferred_transform(track);
            self.transform_checked.set(true);
            if transform_is_identity(transform) {
                return;
            }
            let composition_class = class!(AVMutableVideoComposition);
            let composition: *mut Object =
                msg_send![composition_class, videoCompositionWithPropertiesOfAsset: *self.asset];
            if composition.is_null() {
                return;
            }
            let encoded_size: CgSize = msg_send![track, naturalSize];
            let render_size: CgSize = msg_send![composition, renderSize];
            if render_size.width != encoded_size.width || render_size.height != encoded_size.height
            {
                let _: () = msg_send![item, setVideoComposition: composition];
            } else {
                self.restart_with_composition(composition);
            }
            if self.max_size.is_none() && render_size.width != render_size.height {
                self.upright_is_portrait
                    .set(Some(render_size.height > render_size.width));
                self.stale_frames_left.set(STALE_FRAME_BUDGET);
            }
        });
    }

    fn restart_with_composition(&self, composition: *mut Object) {
        unsafe {
            let resume_at = self.current_time();
            let item_class = class!(AVPlayerItem);
            let item_alloc: *mut Object = msg_send![item_class, alloc];
            let item: *mut Object = msg_send![item_alloc, initWithAsset: *self.asset];
            if item.is_null() {
                return;
            }
            let item = StrongPtr::new(item);
            let _: () = msg_send![*item, setVideoComposition: composition];
            let previous = self.current_item();
            if !previous.is_null() {
                let _: () = msg_send![previous, removeOutput: *self.output];
            }
            let _: () = msg_send![*item, addOutput: *self.output];
            let _: () = msg_send![*self.player, replaceCurrentItemWithPlayerItem: *item];
            self.seek(resume_at);
        }
    }

    fn waiting_for_transform(&self) -> bool {
        if self.transform_checked.get() {
            return false;
        }
        let left = self.transform_polls_left.get();
        self.transform_polls_left.set(left.saturating_sub(1));
        left > 0
    }

    fn current_item(&self) -> *mut Object {
        unsafe { msg_send![*self.player, currentItem] }
    }

    fn is_stale_pre_composition_frame(&self, frame: &VideoFrame) -> bool {
        let Some(portrait) = self.upright_is_portrait.get() else {
            return false;
        };
        let budget = self.stale_frames_left.get();
        if budget == 0 || (frame.get_height() > frame.get_width()) == portrait {
            self.upright_is_portrait.set(None);
            return false;
        }
        self.stale_frames_left.set(budget - 1);
        true
    }

    pub fn play(&self) {
        unsafe {
            let _: () = msg_send![*self.player, play];
        }
    }

    pub fn pause(&self) {
        unsafe {
            let _: () = msg_send![*self.player, pause];
        }
    }

    pub fn is_playing(&self) -> bool {
        unsafe {
            let rate: f32 = msg_send![*self.player, rate];
            rate != 0.0
        }
    }

    pub fn current_time(&self) -> f64 {
        unsafe {
            let time: CmTime = msg_send![*self.player, currentTime];
            cm_time_to_seconds(time)
        }
    }

    pub fn duration(&self) -> f64 {
        let item = self.current_item();
        if item.is_null() {
            return 0.0;
        }
        unsafe {
            let time: CmTime = msg_send![item, duration];
            cm_time_to_seconds(time)
        }
    }

    pub fn seek(&self, to_seconds: f64) {
        unsafe {
            let target = seconds_to_cm_time(to_seconds);
            let zero = CmTime {
                value: 0,
                timescale: 1,
                flags: CM_TIME_FLAG_VALID,
                epoch: 0,
            };
            let _: () = msg_send![
                *self.player,
                seekToTime: target
                toleranceBefore: zero
                toleranceAfter: zero
            ];
        }
    }

    pub fn set_volume(&self, volume: f32) {
        unsafe {
            let _: () = msg_send![*self.player, setVolume: volume.clamp(0.0, 1.0)];
        }
    }

    pub fn volume(&self) -> f32 {
        unsafe {
            let value: f32 = msg_send![*self.player, volume];
            value
        }
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe {
            let value: BOOL = if muted { YES } else { NO };
            let _: () = msg_send![*self.player, setMuted: value];
        }
    }

    pub fn is_muted(&self) -> bool {
        unsafe {
            let value: BOOL = msg_send![*self.player, isMuted];
            value != NO
        }
    }

    pub fn failed(&self) -> bool {
        let item = self.current_item();
        if item.is_null() {
            return false;
        }
        unsafe {
            let status: isize = msg_send![item, status];
            status == AV_PLAYER_ITEM_STATUS_FAILED
        }
    }
}

impl Drop for PlayerImpl {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![*self.player, pause];
        }
    }
}

fn pixel_buffer_attributes(max_size: Option<(u32, u32)>) -> Result<StrongPtr, PlayerError> {
    unsafe {
        let number_class = class!(NSNumber);
        let format = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;
        let format_number: *mut Object = msg_send![number_class, numberWithUnsignedInt: format];
        let metal_number: *mut Object = msg_send![number_class, numberWithBool: YES];
        if format_number.is_null() || metal_number.is_null() {
            return Err(PlayerError::Open);
        }

        let format_key = kCVPixelBufferPixelFormatTypeKey as *const c_void as *mut Object;
        let metal_key = kCVPixelBufferMetalCompatibilityKey as *const c_void as *mut Object;

        let mut objects: Vec<*mut Object> = vec![format_number, metal_number];
        let mut keys: Vec<*mut Object> = vec![format_key, metal_key];

        if let Some((width, height)) = max_size
            && width > 0
            && height > 0
        {
            let width_number: *mut Object = msg_send![number_class, numberWithUnsignedInt: width];
            let height_number: *mut Object = msg_send![number_class, numberWithUnsignedInt: height];
            if width_number.is_null() || height_number.is_null() {
                return Err(PlayerError::Open);
            }
            objects.push(width_number);
            objects.push(height_number);
            keys.push(kCVPixelBufferWidthKey as *const c_void as *mut Object);
            keys.push(kCVPixelBufferHeightKey as *const c_void as *mut Object);
        }

        let count = objects.len();
        let dictionary_class = class!(NSDictionary);
        let dictionary: *mut Object = msg_send![
            dictionary_class,
            dictionaryWithObjects: objects.as_ptr()
            forKeys: keys.as_ptr()
            count: count
        ];
        if dictionary.is_null() {
            return Err(PlayerError::Open);
        }
        let retained: *mut Object = msg_send![dictionary, retain];
        Ok(StrongPtr::new(retained))
    }
}

fn frame_format_is_renderable(format: u32) -> bool {
    format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
}

const POSTER_TIME_SECONDS: f64 = 1.0;

pub fn probe_video(path: &str, max_poster_edge: u32) -> Option<VideoProbe> {
    let c_path = CString::new(path).ok()?;
    objc::rc::autoreleasepool(|| unsafe {
        let ns_string = class!(NSString);
        let path_string: *mut Object = msg_send![ns_string, stringWithUTF8String: c_path.as_ptr()];
        if path_string.is_null() {
            return None;
        }
        let ns_url_class = class!(NSURL);
        let file_url: *mut Object = msg_send![ns_url_class, fileURLWithPath: path_string];
        if file_url.is_null() {
            return None;
        }
        let asset_class = class!(AVURLAsset);
        let asset_alloc: *mut Object = msg_send![asset_class, alloc];
        let asset: *mut Object =
            msg_send![asset_alloc, initWithURL: file_url options: ptr::null::<Object>()];
        if asset.is_null() {
            return None;
        }
        let asset = StrongPtr::new(asset);

        let (width, height) = video_natural_size(*asset)?;
        let poster_jpeg = generate_poster(*asset, max_poster_edge);
        Some(VideoProbe {
            width,
            height,
            poster_jpeg,
        })
    })
}

fn first_video_track(asset: *mut Object) -> Option<*mut Object> {
    unsafe {
        let ns_string = class!(NSString);
        let media_type: *mut Object = msg_send![ns_string, stringWithUTF8String: c"vide".as_ptr()];
        if media_type.is_null() {
            return None;
        }
        let tracks: *mut Object = msg_send![asset, tracksWithMediaType: media_type];
        if tracks.is_null() {
            return None;
        }
        let count: usize = msg_send![tracks, count];
        if count == 0 {
            return None;
        }
        let track: *mut Object = msg_send![tracks, objectAtIndex: 0usize];
        if track.is_null() {
            return None;
        }
        Some(track)
    }
}

fn transform_is_identity(transform: CgAffineTransform) -> bool {
    const EPSILON: f64 = 1e-6;
    (transform.a - 1.0).abs() < EPSILON
        && transform.b.abs() < EPSILON
        && transform.c.abs() < EPSILON
        && (transform.d - 1.0).abs() < EPSILON
}

fn track_preferred_transform(track: *mut Object) -> CgAffineTransform {
    unsafe { msg_send![track, preferredTransform] }
}

fn video_natural_size(asset: *mut Object) -> Option<(u32, u32)> {
    let track = first_video_track(asset)?;
    let transform = track_preferred_transform(track);
    unsafe {
        let size: CgSize = msg_send![track, naturalSize];
        let width = ((size.width * transform.a).abs() + (size.height * transform.c).abs()).round();
        let height = ((size.width * transform.b).abs() + (size.height * transform.d).abs()).round();
        if !(width.is_finite() && height.is_finite()) || width < 1.0 || height < 1.0 {
            return None;
        }
        Some((width as u32, height as u32))
    }
}

fn generate_poster(asset: *mut Object, max_edge: u32) -> Option<Vec<u8>> {
    unsafe {
        let generator_class = class!(AVAssetImageGenerator);
        let generator_alloc: *mut Object = msg_send![generator_class, alloc];
        let generator: *mut Object = msg_send![generator_alloc, initWithAsset: asset];
        if generator.is_null() {
            return None;
        }
        let generator = StrongPtr::new(generator);
        let _: () = msg_send![*generator, setAppliesPreferredTrackTransform: YES];
        if max_edge > 0 {
            let size = CgSize {
                width: max_edge as f64,
                height: max_edge as f64,
            };
            let _: () = msg_send![*generator, setMaximumSize: size];
        }
        let time = CmTime {
            value: (POSTER_TIME_SECONDS * SEEK_TIMESCALE as f64) as i64,
            timescale: SEEK_TIMESCALE,
            flags: CM_TIME_FLAG_VALID,
            epoch: 0,
        };
        let cg_image: *mut c_void = msg_send![
            *generator,
            copyCGImageAtTime: time
            actualTime: ptr::null_mut::<CmTime>()
            error: ptr::null_mut::<*mut Object>()
        ];
        if cg_image.is_null() {
            return None;
        }
        let jpeg = encode_cgimage_jpeg(cg_image);
        CFRelease(cg_image);
        jpeg
    }
}

fn encode_cgimage_jpeg(cg_image: *mut c_void) -> Option<Vec<u8>> {
    unsafe {
        let data_class = class!(NSMutableData);
        let data: *mut Object = msg_send![data_class, data];
        if data.is_null() {
            return None;
        }
        let ns_string = class!(NSString);
        let uti: *mut Object = msg_send![ns_string, stringWithUTF8String: c"public.jpeg".as_ptr()];
        if uti.is_null() {
            return None;
        }
        let dest = CGImageDestinationCreateWithData(
            data as *mut c_void,
            uti as *const c_void,
            1,
            ptr::null(),
        );
        if dest.is_null() {
            return None;
        }
        CGImageDestinationAddImage(dest, cg_image, ptr::null());
        let finalized = CGImageDestinationFinalize(dest);
        CFRelease(dest as *const c_void);
        if finalized == 0 {
            return None;
        }
        let length: usize = msg_send![data, length];
        if length == 0 {
            return None;
        }
        let bytes: *const u8 = msg_send![data, bytes];
        if bytes.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(bytes, length).to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOURCC_420V: u32 = 0x3432_3076;
    const FOURCC_X420: u32 = 0x7834_3230;
    const FOURCC_BGRA: u32 = 0x4247_5241;

    const IDENTITY: CgAffineTransform = CgAffineTransform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    fn quarter_turns(quarters: u32) -> CgAffineTransform {
        let angle = std::f64::consts::FRAC_PI_2 * quarters as f64;
        CgAffineTransform {
            a: angle.cos(),
            b: angle.sin(),
            c: -angle.sin(),
            d: angle.cos(),
            tx: 0.0,
            ty: 0.0,
        }
    }

    #[test]
    fn only_an_unrotated_track_reads_as_identity() {
        assert!(transform_is_identity(IDENTITY));
        assert!(transform_is_identity(CgAffineTransform {
            tx: 120.0,
            ty: -8.0,
            ..IDENTITY
        }));
        for quarters in 1..4 {
            assert!(!transform_is_identity(quarter_turns(quarters)));
        }
        assert!(!transform_is_identity(CgAffineTransform {
            a: -1.0,
            ..IDENTITY
        }));
    }

    #[test]
    fn cm_time_matches_core_media_layout() {
        assert_eq!(std::mem::size_of::<CmTime>(), 24);
        assert_eq!(std::mem::align_of::<CmTime>(), 8);
    }

    #[test]
    fn requested_format_is_the_format_the_renderer_asserts() {
        assert_eq!(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, 0x3432_3066);
    }

    #[test]
    fn only_the_renderer_required_format_passes_the_guard() {
        assert!(frame_format_is_renderable(
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        ));
        assert!(!frame_format_is_renderable(FOURCC_420V));
        assert!(!frame_format_is_renderable(FOURCC_X420));
        assert!(!frame_format_is_renderable(FOURCC_BGRA));
    }

    #[test]
    fn cm_time_to_seconds_handles_valid_and_degenerate() {
        let valid = CmTime {
            value: 3000,
            timescale: 600,
            flags: CM_TIME_FLAG_VALID,
            epoch: 0,
        };
        assert_eq!(cm_time_to_seconds(valid), 5.0);

        let invalid = CmTime {
            value: 3000,
            timescale: 600,
            flags: 0,
            epoch: 0,
        };
        assert_eq!(cm_time_to_seconds(invalid), 0.0);

        let indefinite = CmTime {
            value: 0,
            timescale: 600,
            flags: CM_TIME_FLAG_VALID | CM_TIME_FLAG_INDEFINITE,
            epoch: 0,
        };
        assert_eq!(cm_time_to_seconds(indefinite), 0.0);

        let zero_scale = CmTime {
            value: 3000,
            timescale: 0,
            flags: CM_TIME_FLAG_VALID,
            epoch: 0,
        };
        assert_eq!(cm_time_to_seconds(zero_scale), 0.0);
    }

    #[test]
    fn seconds_to_cm_time_uses_seek_timescale_and_clamps() {
        let t = seconds_to_cm_time(5.0);
        assert_eq!(t.timescale, SEEK_TIMESCALE);
        assert_eq!(t.value, 3000);
        assert_eq!(t.flags, CM_TIME_FLAG_VALID);

        let negative = seconds_to_cm_time(-3.0);
        assert_eq!(negative.value, 0);

        let not_finite = seconds_to_cm_time(f64::NAN);
        assert_eq!(not_finite.value, 0);
    }

    #[test]
    fn cm_time_roundtrip_through_seconds() {
        let seconds = 12.5;
        assert_eq!(cm_time_to_seconds(seconds_to_cm_time(seconds)), seconds);
    }
}
