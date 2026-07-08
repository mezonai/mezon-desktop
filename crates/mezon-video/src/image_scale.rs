use std::ffi::c_void;
use std::path::Path;
use std::ptr;

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::base::CFRelease;
use core_foundation_sys::string::CFStringRef;

use crate::ScaledImage;

const K_CG_IMAGE_ALPHA_PREMULTIPLIED_FIRST: u32 = 2;
const K_CG_BITMAP_BYTE_ORDER_32_LITTLE: u32 = 2 << 12;
const MAX_DIMENSION: usize = 30000;

#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithData(data: *const c_void, options: *const c_void) -> *const c_void;
    fn CGImageSourceCreateWithURL(url: *const c_void, options: *const c_void) -> *const c_void;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: *const c_void,
        index: usize,
        options: *const c_void,
    ) -> *const c_void;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFStringRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFStringRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: *const c_void) -> usize;
    fn CGImageGetHeight(image: *const c_void) -> usize;
    fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
    fn CGColorSpaceRelease(space: *const c_void);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *const c_void,
        bitmap_info: u32,
    ) -> *const c_void;
    fn CGContextDrawImage(context: *const c_void, rect: CgRect, image: *const c_void);
    fn CGContextRelease(context: *const c_void);
    fn CGImageRelease(image: *const c_void);
}

pub fn scaled_image_decode(bytes: &[u8], max_px: u32) -> Option<ScaledImage> {
    if bytes.is_empty() || max_px == 0 {
        return None;
    }
    unsafe {
        let data = CFData::from_buffer(bytes);
        let source =
            CGImageSourceCreateWithData(data.as_concrete_TypeRef() as *const c_void, ptr::null());
        if source.is_null() {
            return None;
        }
        let options = CFDictionary::from_CFType_pairs(&[
            (
                CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailFromImageAlways)
                    .as_CFType(),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailWithTransform)
                    .as_CFType(),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kCGImageSourceThumbnailMaxPixelSize).as_CFType(),
                CFNumber::from(max_px as i32).as_CFType(),
            ),
        ]);
        let image = CGImageSourceCreateThumbnailAtIndex(
            source,
            0,
            options.as_concrete_TypeRef() as *const c_void,
        );
        CFRelease(source);
        if image.is_null() {
            return None;
        }
        let result = draw_bgra(image);
        CGImageRelease(image);
        result
    }
}

pub fn scaled_image_decode_path(path: &Path, max_px: u32) -> Option<ScaledImage> {
    if max_px == 0 {
        return None;
    }
    let url = CFURL::from_path(path, false)?;
    unsafe {
        let source =
            CGImageSourceCreateWithURL(url.as_concrete_TypeRef() as *const c_void, ptr::null());
        if source.is_null() {
            return None;
        }
        let options = CFDictionary::from_CFType_pairs(&[
            (
                CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailFromImageAlways)
                    .as_CFType(),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailWithTransform)
                    .as_CFType(),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kCGImageSourceThumbnailMaxPixelSize).as_CFType(),
                CFNumber::from(max_px as i32).as_CFType(),
            ),
        ]);
        let image = CGImageSourceCreateThumbnailAtIndex(
            source,
            0,
            options.as_concrete_TypeRef() as *const c_void,
        );
        CFRelease(source);
        if image.is_null() {
            return None;
        }
        let result = draw_bgra(image);
        CGImageRelease(image);
        result
    }
}

unsafe fn draw_bgra(image: *const c_void) -> Option<ScaledImage> {
    let width = unsafe { CGImageGetWidth(image) };
    let height = unsafe { CGImageGetHeight(image) };
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }
    let bytes_per_row = width.checked_mul(4)?;
    let mut buffer = vec![0u8; bytes_per_row.checked_mul(height)?];

    let color_space = unsafe { CGColorSpaceCreateDeviceRGB() };
    if color_space.is_null() {
        return None;
    }
    let context = unsafe {
        CGBitmapContextCreate(
            buffer.as_mut_ptr() as *mut c_void,
            width,
            height,
            8,
            bytes_per_row,
            color_space,
            K_CG_IMAGE_ALPHA_PREMULTIPLIED_FIRST | K_CG_BITMAP_BYTE_ORDER_32_LITTLE,
        )
    };
    unsafe { CGColorSpaceRelease(color_space) };
    if context.is_null() {
        return None;
    }
    let rect = CgRect {
        x: 0.0,
        y: 0.0,
        width: width as f64,
        height: height as f64,
    };
    unsafe {
        CGContextDrawImage(context, rect, image);
        CGContextRelease(context);
    }

    for pixel in buffer.chunks_exact_mut(4) {
        let alpha = pixel[3];
        if alpha != 0 && alpha != 255 {
            let alpha = alpha as u16;
            pixel[0] = ((pixel[0] as u16 * 255 + alpha / 2) / alpha).min(255) as u8;
            pixel[1] = ((pixel[1] as u16 * 255 + alpha / 2) / alpha).min(255) as u8;
            pixel[2] = ((pixel[2] as u16 * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }

    Some(ScaledImage {
        width: width as u32,
        height: height as u32,
        bgra: buffer,
    })
}
