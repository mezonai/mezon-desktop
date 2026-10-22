use std::ffi::c_void;
use std::ptr;

use core_foundation::base::TCFType;
use core_video::pixel_buffer::{
    CVPixelBuffer, CVPixelBufferRef, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferMetalCompatibilityKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use objc::runtime::{Object, YES};
use objc::{class, msg_send, sel, sel_impl};

const LOCK_READ_WRITE: u64 = 0;

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferCreate(
        allocator: *const c_void,
        width: usize,
        height: usize,
        pixel_format: u32,
        attributes: *const c_void,
        buffer_out: *mut CVPixelBufferRef,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddressOfPlane(buffer: CVPixelBufferRef, plane: usize) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRowOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
}

pub fn pixel_buffer_from_i420(
    width: u32,
    height: u32,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    max_size: Option<(u32, u32)>,
) -> Option<CVPixelBuffer> {
    let (out_w, out_h) = output_size(width, height, max_size)?;
    let buffer = create_pixel_buffer(out_w, out_h)?;
    fill_pixel_buffer_from_i420(&buffer, width, height, y, u, v, max_size)?;
    Some(buffer)
}

pub fn fill_pixel_buffer_from_i420(
    buffer: &CVPixelBuffer,
    width: u32,
    height: u32,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    max_size: Option<(u32, u32)>,
) -> Option<()> {
    let (out_w, out_h) = output_size(width, height, max_size)?;
    if buffer.get_width() as u32 != out_w || buffer.get_height() as u32 != out_h {
        return None;
    }
    if out_w == width && out_h == height {
        return fill_biplanar_full_from_limited(buffer, width, height, y, u, v);
    }
    let bgra = crate::frame_util::i420_to_bgra(width, height, y, u, v)?;
    let scaled = scale_bgra(&bgra, width, height, out_w, out_h)?;
    let (_, _, sy, su, sv) = i420_limited_from_bgra(out_w, out_h, &scaled)?;
    fill_biplanar_full_from_limited(buffer, out_w, out_h, &sy, &su, &sv)
}

pub fn output_size(width: u32, height: u32, max_size: Option<(u32, u32)>) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    Some(match max_size {
        Some((max_w, max_h)) if max_w > 0 && max_h > 0 => {
            if width <= max_w && height <= max_h {
                (width, height)
            } else {
                let scale = (max_w as f32 / width as f32).min(max_h as f32 / height as f32);
                (
                    ((width as f32 * scale).round() as u32).max(1),
                    ((height as f32 * scale).round() as u32).max(1),
                )
            }
        }
        _ => (width, height),
    })
}

pub fn create_pixel_buffer(width: u32, height: u32) -> Option<CVPixelBuffer> {
    let attributes = pixel_buffer_attributes()?;
    let mut buffer: CVPixelBufferRef = ptr::null_mut();
    let status = unsafe {
        CVPixelBufferCreate(
            ptr::null(),
            width as usize,
            height as usize,
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            attributes,
            &mut buffer,
        )
    };
    if status != 0 || buffer.is_null() {
        return None;
    }
    Some(unsafe { CVPixelBuffer::wrap_under_create_rule(buffer) })
}

fn pixel_buffer_attributes() -> Option<*const c_void> {
    unsafe {
        let number_class = class!(NSNumber);
        let metal_number: *mut Object = msg_send![number_class, numberWithBool: YES];
        if metal_number.is_null() {
            return None;
        }
        let empty_iosurface: *mut Object = msg_send![class!(NSDictionary), dictionary];
        if empty_iosurface.is_null() {
            return None;
        }
        let metal_key = kCVPixelBufferMetalCompatibilityKey as *const c_void as *mut Object;
        let iosurface_key = kCVPixelBufferIOSurfacePropertiesKey as *const c_void as *mut Object;
        let objects = [metal_number, empty_iosurface];
        let keys = [metal_key, iosurface_key];
        let dictionary: *mut Object = msg_send![
            class!(NSDictionary),
            dictionaryWithObjects: objects.as_ptr()
            forKeys: keys.as_ptr()
            count: 2usize
        ];
        if dictionary.is_null() {
            return None;
        }
        Some(dictionary as *const c_void)
    }
}

fn fill_biplanar_full_from_limited(
    buffer: &CVPixelBuffer,
    width: u32,
    height: u32,
    y: &[u8],
    u: &[u8],
    v: &[u8],
) -> Option<()> {
    let buffer_ref = buffer.as_concrete_TypeRef();
    if unsafe { CVPixelBufferLockBaseAddress(buffer_ref, LOCK_READ_WRITE) } != 0 {
        return None;
    }
    let ok = (|| {
        let y_base = unsafe { CVPixelBufferGetBaseAddressOfPlane(buffer_ref, 0) };
        let uv_base = unsafe { CVPixelBufferGetBaseAddressOfPlane(buffer_ref, 1) };
        if y_base.is_null() || uv_base.is_null() {
            return None;
        }
        let y_stride = unsafe { CVPixelBufferGetBytesPerRowOfPlane(buffer_ref, 0) };
        let uv_stride = unsafe { CVPixelBufferGetBytesPerRowOfPlane(buffer_ref, 1) };
        copy_y_limited_to_full(
            y_base as *mut u8,
            y_stride,
            y,
            width as usize,
            height as usize,
        );
        copy_uv_limited_to_full(
            uv_base as *mut u8,
            uv_stride,
            u,
            v,
            width as usize,
            height as usize,
        );
        Some(())
    })();
    let _ = unsafe { CVPixelBufferUnlockBaseAddress(buffer_ref, LOCK_READ_WRITE) };
    ok
}

fn expand_y_limited_to_full(y: u8) -> u8 {
    ((((i32::from(y) - 16) * 255) + 109) / 219).clamp(0, 255) as u8
}

fn expand_c_limited_to_full(c: u8) -> u8 {
    ((((i32::from(c) - 128) * 255) + 112) / 224 + 128).clamp(0, 255) as u8
}

fn copy_y_limited_to_full(
    dst: *mut u8,
    dst_stride: usize,
    src: &[u8],
    width: usize,
    height: usize,
) {
    if dst.is_null() || dst_stride == 0 || width == 0 {
        return;
    }
    for row in 0..height {
        let src_start = row * width;
        if src_start >= src.len() {
            break;
        }
        let row_bytes = width.min(src.len() - src_start).min(dst_stride);
        for col in 0..row_bytes {
            unsafe {
                *dst.add(row * dst_stride + col) = expand_y_limited_to_full(src[src_start + col]);
            }
        }
    }
}

fn copy_uv_limited_to_full(
    dst: *mut u8,
    dst_stride: usize,
    u: &[u8],
    v: &[u8],
    width: usize,
    height: usize,
) {
    if dst.is_null() || dst_stride == 0 || width == 0 || height == 0 {
        return;
    }
    let uv_width = width.div_ceil(2);
    let uv_height = height.div_ceil(2);
    for row in 0..uv_height {
        for col in 0..uv_width {
            let u_idx = row * uv_width + col;
            let dst_idx = row * dst_stride + col * 2;
            if u_idx >= u.len() || u_idx >= v.len() || dst_idx + 1 >= row * dst_stride + dst_stride
            {
                continue;
            }
            unsafe {
                *dst.add(dst_idx) = expand_c_limited_to_full(u[u_idx]);
                *dst.add(dst_idx + 1) = expand_c_limited_to_full(v[u_idx]);
            }
        }
    }
}

fn scale_bgra(source: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Option<Vec<u8>> {
    let src_w = src_w as usize;
    let src_h = src_h as usize;
    let dst_w = dst_w as usize;
    let dst_h = dst_h as usize;
    let src_stride = src_w.checked_mul(4)?;
    if source.len() < src_stride.checked_mul(src_h)? {
        return None;
    }
    let mut out = vec![0u8; dst_w.checked_mul(dst_h)?.checked_mul(4)?];
    for y in 0..dst_h {
        let src_y = y * src_h / dst_h;
        for x in 0..dst_w {
            let src_x = x * src_w / dst_w;
            let from = src_y * src_stride + src_x * 4;
            let to = y * dst_w * 4 + x * 4;
            out[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
    Some(out)
}

fn i420_limited_from_bgra(
    width: u32,
    height: u32,
    bgra: &[u8],
) -> Option<(u32, u32, Vec<u8>, Vec<u8>, Vec<u8>)> {
    let w = width as usize;
    let h = height as usize;
    let stride = w.checked_mul(4)?;
    if bgra.len() < stride.checked_mul(h)? {
        return None;
    }
    let uv_w = w.div_ceil(2);
    let uv_h = h.div_ceil(2);
    let mut y = vec![0u8; w * h];
    let mut u = vec![0u8; uv_w * uv_h];
    let mut v = vec![0u8; uv_w * uv_h];
    for row in 0..h {
        for col in 0..w {
            let i = row * stride + col * 4;
            let b = bgra[i] as i32;
            let g = bgra[i + 1] as i32;
            let r = bgra[i + 2] as i32;
            y[row * w + col] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
            if row % 2 == 0 && col % 2 == 0 {
                let uv_idx = (row / 2) * uv_w + col / 2;
                u[uv_idx] = ((((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
                v[uv_idx] = ((((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
            }
        }
    }
    Some((width, height, y, u, v))
}
