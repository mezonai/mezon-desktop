//! Turning a decoded frame the right way up.
//!
//! A phone recording in portrait does not rotate the pixels: it stores a rotation
//! in the container and leaves the picture as the sensor saw it. macOS composes
//! that away inside AVFoundation and Linux hands it to `videoflip`, but Media
//! Foundation gives the frame back exactly as encoded, so on Windows the turn is
//! ours to do — for the poster and for every played frame.

use image::RgbImage;

/// Clockwise, because that is the direction that cancels the counter-clockwise
/// angle both the `tkhd` display matrix and `MF_MT_VIDEO_ROTATION` describe.
pub(crate) fn turn_rgb(image: RgbImage, quarter_turns: u8) -> RgbImage {
    match quarter_turns % 4 {
        1 => image::imageops::rotate90(&image),
        2 => image::imageops::rotate180(&image),
        3 => image::imageops::rotate270(&image),
        _ => image,
    }
}

/// The same turn over a tightly packed 4-bytes-per-pixel buffer, handing back the
/// dimensions it now has. Channel order does not matter to a rotation, so this is
/// equally a BGRA and an RGBA operation.
///
/// Only Windows turns whole frames — the other two platforms rotate inside their
/// decode pipeline — but it is built and tested everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn turn_bgra(
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    quarter_turns: u8,
) -> Option<(u32, u32, Vec<u8>)> {
    if quarter_turns.is_multiple_of(4) {
        return Some((width, height, bgra));
    }
    if !crate::frame_util::is_valid_bgra_len(width, height, bgra.len()) {
        return None;
    }
    let image = image::RgbaImage::from_raw(width, height, bgra)?;
    let turned = match quarter_turns % 4 {
        1 => image::imageops::rotate90(&image),
        2 => image::imageops::rotate180(&image),
        _ => image::imageops::rotate270(&image),
    };
    let (width, height) = turned.dimensions();
    Some((width, height, turned.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four pixels, each its own value, so a wrong turn cannot pass.
    fn corners() -> image::RgbaImage {
        image::RgbaImage::from_raw(
            2,
            2,
            vec![
                1, 1, 1, 255, // top-left
                2, 2, 2, 255, // top-right
                3, 3, 3, 255, // bottom-left
                4, 4, 4, 255, // bottom-right
            ],
        )
        .expect("2x2")
    }

    fn first_bytes(bgra: &[u8]) -> Vec<u8> {
        bgra.chunks_exact(4).map(|pixel| pixel[0]).collect()
    }

    #[test]
    fn a_quarter_turn_moves_the_top_left_pixel_to_the_top_right() {
        let (width, height, turned) = turn_bgra(corners().into_raw(), 2, 2, 1).expect("turn");
        assert_eq!((width, height), (2, 2));
        assert_eq!(first_bytes(&turned), vec![3, 1, 4, 2]);
    }

    #[test]
    fn three_quarter_turns_are_the_other_way_round() {
        let (_, _, turned) = turn_bgra(corners().into_raw(), 2, 2, 3).expect("turn");
        assert_eq!(first_bytes(&turned), vec![2, 4, 1, 3]);
    }

    #[test]
    fn a_half_turn_reverses_the_pixels() {
        let (_, _, turned) = turn_bgra(corners().into_raw(), 2, 2, 2).expect("turn");
        assert_eq!(first_bytes(&turned), vec![4, 3, 2, 1]);
    }

    #[test]
    fn turning_a_frame_a_quarter_swaps_its_side_lengths() {
        let bgra = vec![0u8; 4 * 4 * 2];
        let (width, height, _) = turn_bgra(bgra.clone(), 4, 2, 1).expect("turn");
        assert_eq!((width, height), (2, 4));
        let (width, height, _) = turn_bgra(bgra, 4, 2, 2).expect("turn");
        assert_eq!((width, height), (4, 2));
    }

    #[test]
    fn no_turn_hands_the_buffer_straight_back() {
        let (width, height, turned) = turn_bgra(vec![7u8; 16], 2, 2, 0).expect("turn");
        assert_eq!((width, height), (2, 2));
        assert_eq!(turned, vec![7u8; 16]);
    }

    #[test]
    fn a_buffer_that_is_not_the_size_it_claims_is_refused() {
        assert!(turn_bgra(vec![0u8; 12], 2, 2, 1).is_none());
        assert!(turn_bgra(vec![0u8; 0], 0, 2, 1).is_none());
    }

    #[test]
    fn the_rgb_turn_swaps_side_lengths_the_same_way() {
        let image = RgbImage::new(4, 2);
        assert_eq!(turn_rgb(image.clone(), 0).dimensions(), (4, 2));
        assert_eq!(turn_rgb(image.clone(), 1).dimensions(), (2, 4));
        assert_eq!(turn_rgb(image.clone(), 2).dimensions(), (4, 2));
        assert_eq!(turn_rgb(image, 3).dimensions(), (2, 4));
    }
}
