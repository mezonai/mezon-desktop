use std::path::Path;

use gdk_pixbuf::prelude::*;
use gdk_pixbuf::{Pixbuf, PixbufLoader};

use crate::ScaledImage;

fn target_size(width: u32, height: u32, max_px: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest == 0 || longest <= max_px {
        return (width, height);
    }
    let scale = max_px as f64 / longest as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

fn pixbuf_to_scaled(pixbuf: &Pixbuf) -> Option<ScaledImage> {
    let width = pixbuf.width();
    let height = pixbuf.height();
    if width <= 0 || height <= 0 {
        return None;
    }
    let width = width as usize;
    let height = height as usize;
    let channels = pixbuf.n_channels() as usize;
    let rowstride = pixbuf.rowstride() as usize;
    let has_alpha = pixbuf.has_alpha();
    if channels < 3 {
        return None;
    }
    let bytes = pixbuf.read_pixel_bytes();
    let pixels: &[u8] = bytes.as_ref();

    let mut bgra = vec![0u8; width * height * 4];
    for y in 0..height {
        let row = y * rowstride;
        for x in 0..width {
            let src = row + x * channels;
            if src + channels > pixels.len() {
                return None;
            }
            let dst = (y * width + x) * 4;
            bgra[dst] = pixels[src + 2];
            bgra[dst + 1] = pixels[src + 1];
            bgra[dst + 2] = pixels[src];
            bgra[dst + 3] = if has_alpha { pixels[src + 3] } else { 255 };
        }
    }

    Some(ScaledImage {
        width: width as u32,
        height: height as u32,
        bgra,
    })
}

pub fn scaled_image_decode_path(path: &Path, max_px: u32) -> Option<ScaledImage> {
    let (original_width, original_height) = match Pixbuf::file_info(path) {
        Some((_, width, height)) if width > 0 && height > 0 => (width as u32, height as u32),
        _ => return None,
    };
    let (target_width, target_height) = target_size(original_width, original_height, max_px);
    let pixbuf =
        Pixbuf::from_file_at_scale(path, target_width as i32, target_height as i32, false).ok()?;
    pixbuf_to_scaled(&pixbuf)
}

pub fn scaled_image_decode(bytes: &[u8], max_px: u32) -> Option<ScaledImage> {
    let loader = PixbufLoader::new();
    let max = max_px as i32;
    loader.connect_size_prepared(move |loader, width, height| {
        if width > max || height > max {
            let longest = width.max(height);
            let scale = max as f64 / longest as f64;
            let target_width = ((width as f64 * scale).round() as i32).max(1);
            let target_height = ((height as f64 * scale).round() as i32).max(1);
            loader.set_size(target_width, target_height);
        }
    });
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    let pixbuf = loader.pixbuf()?;
    pixbuf_to_scaled(&pixbuf)
}
