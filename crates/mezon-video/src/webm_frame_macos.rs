use std::ffi::c_void;
use std::ptr;

use core_video::pixel_buffer::{
    CVPixelBuffer, CVPixelBufferRef, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use oxideav_vp8::decoder::Vp8DecodedFrame;

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

pub fn pixel_buffer_from_vp8(
    decoded: &Vp8DecodedFrame,
    max_size: Option<(u32, u32)>,
) -> Option<CVPixelBuffer> {
    let (width, height, y, u, v) = scaled_i420(decoded, max_size)?;
    let buffer = create_pixel_buffer(width, height)?;
    fill_biplanar(&buffer, width, height, &y, &u, &v)?;
    Some(buffer)
}

fn scaled_i420(
    decoded: &Vp8DecodedFrame,
    max_size: Option<(u32, u32)>,
) -> Option<(u32, u32, Vec<u8>, Vec<u8>, Vec<u8>)> {
    let (width, height) = match max_size {
        Some((max_w, max_h)) if max_w > 0 && max_h > 0 => {
            if decoded.width <= max_w && decoded.height <= max_h {
                (decoded.width, decoded.height)
            } else {
                let scale =
                    (max_w as f32 / decoded.width as f32).min(max_h as f32 / decoded.height as f32);
                (
                    ((decoded.width as f32 * scale).round() as u32).max(1),
                    ((decoded.height as f32 * scale).round() as u32).max(1),
                )
            }
        }
        _ => (decoded.width, decoded.height),
    };
    if width == decoded.width && height == decoded.height {
        return Some((
            width,
            height,
            decoded.y.clone(),
            decoded.u.clone(),
            decoded.v.clone(),
        ));
    }
    let bgra = crate::frame_util::i420_to_bgra(
        decoded.width,
        decoded.height,
        &decoded.y,
        &decoded.u,
        &decoded.v,
    )?;
    let scaled = scale_bgra(&bgra, decoded.width, decoded.height, width, height)?;
    i420_from_bgra(width, height, &scaled)
}

fn create_pixel_buffer(width: u32, height: u32) -> Option<CVPixelBuffer> {
    let mut buffer: CVPixelBufferRef = ptr::null_mut();
    let status = unsafe {
        CVPixelBufferCreate(
            ptr::null(),
            width as usize,
            height as usize,
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            ptr::null(),
            &mut buffer,
        )
    };
    if status != 0 || buffer.is_null() {
        return None;
    }
    Some(unsafe { CVPixelBuffer::wrap_under_create_rule(buffer) })
}

fn fill_biplanar(
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
        copy_y_plane(
            y_base as *mut u8,
            y_stride,
            y,
            width as usize,
            height as usize,
        );
        copy_uv_plane(
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

fn copy_y_plane(dst: *mut u8, dst_stride: usize, src: &[u8], width: usize, height: usize) {
    if dst.is_null() || dst_stride == 0 || width == 0 {
        return;
    }
    let row_bytes = width
        .min(dst_stride)
        .min(src.len().saturating_div(height.max(1)));
    for row in 0..height {
        let src_start = row * width;
        if src_start + row_bytes > src.len() {
            break;
        }
        unsafe {
            ptr::copy_nonoverlapping(
                src.as_ptr().add(src_start),
                dst.add(row * dst_stride),
                row_bytes,
            );
        }
    }
}

fn copy_uv_plane(dst: *mut u8, dst_stride: usize, u: &[u8], v: &[u8], width: usize, height: usize) {
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
                *dst.add(dst_idx) = u[u_idx];
                *dst.add(dst_idx + 1) = v[u_idx];
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

fn i420_from_bgra(
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
            y[row * w + col] = ((66 * r + 129 * g + 25 * b + 128) >> 8).clamp(0, 255) as u8;
            if row % 2 == 0 && col % 2 == 0 {
                let uv_row = row / 2;
                let uv_col = col / 2;
                let uv_idx = uv_row * uv_w + uv_col;
                u[uv_idx] = ((-38 * r - 74 * g + 112 * b + 128) >> 8).clamp(0, 255) as u8;
                v[uv_idx] = ((112 * r - 94 * g - 18 * b + 128) >> 8).clamp(0, 255) as u8;
            }
        }
    }
    Some((width, height, y, u, v))
}
