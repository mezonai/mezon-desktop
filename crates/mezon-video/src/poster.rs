use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, RgbImage};

const POSTER_JPEG_QUALITY: u8 = 80;

pub(crate) fn encode_poster_jpeg(
    bgra: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bottom_up: bool,
    max_edge: u32,
) -> Option<Vec<u8>> {
    let rgb = bgra_to_rgb(bgra, width, height, stride, bottom_up)?;
    let rgb = downscale(rgb, max_edge);
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, POSTER_JPEG_QUALITY)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(jpeg)
}

fn bgra_to_rgb(
    bgra: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bottom_up: bool,
) -> Option<RgbImage> {
    let row_bytes = crate::frame_util::tight_row_bytes(width)?;
    if width == 0 || height == 0 || stride < row_bytes {
        return None;
    }
    if bgra.len() < stride.checked_mul(height as usize)? {
        return None;
    }
    let rgb_row = (width as usize).checked_mul(3)?;
    let mut rgb = vec![0u8; rgb_row.checked_mul(height as usize)?];
    for (y, dst) in rgb.chunks_exact_mut(rgb_row).enumerate() {
        let src_row = if bottom_up {
            height as usize - 1 - y
        } else {
            y
        };
        let src = &bgra[src_row * stride..src_row * stride + row_bytes];
        for (pixel, out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(3)) {
            out[0] = pixel[2];
            out[1] = pixel[1];
            out[2] = pixel[0];
        }
    }
    RgbImage::from_raw(width, height, rgb)
}

fn downscale(image: RgbImage, max_edge: u32) -> RgbImage {
    let longest = image.width().max(image.height());
    if max_edge == 0 || longest <= max_edge {
        return image;
    }
    let scale = max_edge as f32 / longest as f32;
    let width = ((image.width() as f32 * scale).round() as u32).max(1);
    let height = ((image.height() as f32 * scale).round() as u32).max(1);
    image::imageops::resize(&image, width, height, FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgra_pixel(b: u8, g: u8, r: u8) -> [u8; 4] {
        [b, g, r, 255]
    }

    #[test]
    fn bgra_to_rgb_swaps_channels_and_drops_row_padding() {
        let mut data = Vec::new();
        data.extend_from_slice(&bgra_pixel(1, 2, 3));
        data.extend_from_slice(&[0xFF, 0xFF]);
        data.extend_from_slice(&bgra_pixel(4, 5, 6));
        data.extend_from_slice(&[0xFF, 0xFF]);

        let rgb = bgra_to_rgb(&data, 1, 2, 6, false).expect("padded rows pack");
        assert_eq!(rgb.as_raw(), &[3, 2, 1, 6, 5, 4]);
    }

    #[test]
    fn bgra_to_rgb_flips_a_bottom_up_buffer() {
        let mut data = Vec::new();
        data.extend_from_slice(&bgra_pixel(1, 2, 3));
        data.extend_from_slice(&bgra_pixel(4, 5, 6));

        let rgb = bgra_to_rgb(&data, 1, 2, 4, true).expect("bottom-up flip");
        assert_eq!(rgb.as_raw(), &[6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn bgra_to_rgb_rejects_short_buffers_and_zero_dimensions() {
        assert!(bgra_to_rgb(&[0u8; 4], 2, 2, 8, false).is_none());
        assert!(bgra_to_rgb(&[0u8; 16], 0, 2, 8, false).is_none());
        assert!(bgra_to_rgb(&[0u8; 16], 2, 2, 4, false).is_none());
    }

    #[test]
    fn downscale_caps_the_longest_edge_and_keeps_the_aspect_ratio() {
        let scaled = downscale(RgbImage::new(1000, 500), 100);
        assert_eq!((scaled.width(), scaled.height()), (100, 50));
    }

    #[test]
    fn downscale_leaves_a_small_frame_alone() {
        let scaled = downscale(RgbImage::new(64, 32), 480);
        assert_eq!((scaled.width(), scaled.height()), (64, 32));
    }

    #[test]
    fn encode_poster_jpeg_writes_a_jpeg_header() {
        let jpeg = encode_poster_jpeg(&[0u8; 4 * 4], 2, 2, 8, false, 480).expect("jpeg");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
