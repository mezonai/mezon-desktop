#![allow(dead_code)]

pub(crate) fn tight_row_bytes(width: u32) -> Option<usize> {
    (width as usize).checked_mul(4)
}

pub(crate) fn tight_bgra_len(width: u32, height: u32) -> Option<usize> {
    tight_row_bytes(width)?.checked_mul(height as usize)
}

pub(crate) fn is_valid_bgra_len(width: u32, height: u32, len: usize) -> bool {
    width != 0 && height != 0 && tight_bgra_len(width, height) == Some(len)
}

pub(crate) fn pack_bgra_rows(
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let row_bytes = tight_row_bytes(width)?;
    if width == 0 || height == 0 || stride < row_bytes {
        return None;
    }
    let needed = stride.checked_mul(height as usize)?;
    if data.len() < needed {
        return None;
    }
    let mut out = vec![0u8; row_bytes.checked_mul(height as usize)?];
    for (dst_row, src_row) in out
        .chunks_exact_mut(row_bytes)
        .zip(data.chunks_exact(stride))
    {
        dst_row.copy_from_slice(&src_row[..row_bytes]);
    }
    Some(out)
}

pub(crate) fn pack_bgra_rows_turned(
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    quarter_turns: u8,
) -> Option<(u32, u32, Vec<u8>)> {
    let turns = quarter_turns % 4;
    if turns == 0 {
        return Some((width, height, pack_bgra_rows(data, stride, width, height)?));
    }
    let row_bytes = tight_row_bytes(width)?;
    if width == 0 || height == 0 || stride < row_bytes {
        return None;
    }
    if data.len() < stride.checked_mul(height as usize)? {
        return None;
    }
    let (source_width, source_height) = (width as usize, height as usize);
    let (turned_width, turned_height) = if turns % 2 == 1 {
        (source_height, source_width)
    } else {
        (source_width, source_height)
    };
    let mut out = vec![0u8; turned_width.checked_mul(turned_height)?.checked_mul(4)?];
    for y in 0..source_height {
        let row = &data[y * stride..y * stride + row_bytes];
        for x in 0..source_width {
            let (turned_x, turned_y) = match turns {
                1 => (source_height - 1 - y, x),
                2 => (source_width - 1 - x, source_height - 1 - y),
                _ => (y, source_width - 1 - x),
            };
            let from = x * 4;
            let to = (turned_y * turned_width + turned_x) * 4;
            out[to..to + 4].copy_from_slice(&row[from..from + 4]);
        }
    }
    Some((turned_width as u32, turned_height as u32, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tight_row_bytes_multiplies_by_four_and_guards_overflow() {
        assert_eq!(tight_row_bytes(0), Some(0));
        assert_eq!(tight_row_bytes(4), Some(16));
        assert_eq!(
            tight_row_bytes(u32::MAX),
            (u32::MAX as usize).checked_mul(4)
        );
    }

    #[test]
    fn is_valid_bgra_len_requires_exact_tight_length_and_nonzero_dims() {
        assert!(is_valid_bgra_len(2, 2, 16));
        assert!(!is_valid_bgra_len(2, 2, 15));
        assert!(!is_valid_bgra_len(2, 2, 17));
        assert!(!is_valid_bgra_len(0, 2, 0));
        assert!(!is_valid_bgra_len(2, 0, 0));
    }

    #[test]
    fn pack_bgra_rows_copies_tightly_packed_input_unchanged() {
        let data: Vec<u8> = (0..16).collect();
        let out = pack_bgra_rows(&data, 8, 2, 2).expect("tight pack");
        assert_eq!(out, data);
    }

    #[test]
    fn pack_bgra_rows_drops_row_padding() {
        let row0 = [10u8, 11, 12, 13, 14, 15, 16, 17];
        let row1 = [20u8, 21, 22, 23, 24, 25, 26, 27];
        let pad = [0xFFu8, 0xFF];
        let mut data = Vec::new();
        data.extend_from_slice(&row0);
        data.extend_from_slice(&pad);
        data.extend_from_slice(&row1);
        data.extend_from_slice(&pad);

        let out = pack_bgra_rows(&data, 10, 2, 2).expect("padded pack");
        assert_eq!(
            out,
            [
                10, 11, 12, 13, 14, 15, 16, 17, 20, 21, 22, 23, 24, 25, 26, 27
            ]
        );
    }

    #[test]
    fn pack_bgra_rows_rejects_stride_smaller_than_row() {
        assert!(pack_bgra_rows(&[0u8; 16], 4, 2, 2).is_none());
    }

    #[test]
    fn pack_bgra_rows_rejects_insufficient_data() {
        assert!(pack_bgra_rows(&[0u8; 8], 8, 2, 2).is_none());
    }

    fn corners() -> Vec<u8> {
        let mut out = Vec::new();
        for value in [1u8, 2, 3, 4] {
            out.extend_from_slice(&[value, value, value, 255]);
        }
        out
    }

    fn first_bytes(bgra: &[u8]) -> Vec<u8> {
        bgra.chunks_exact(4).map(|pixel| pixel[0]).collect()
    }

    #[test]
    fn a_quarter_turn_moves_the_top_left_pixel_to_the_top_right() {
        let (width, height, turned) =
            pack_bgra_rows_turned(&corners(), 8, 2, 2, 1).expect("quarter turn");
        assert_eq!((width, height), (2, 2));
        assert_eq!(first_bytes(&turned), vec![3, 1, 4, 2]);
    }

    #[test]
    fn a_half_turn_reverses_the_pixels() {
        let (_, _, turned) = pack_bgra_rows_turned(&corners(), 8, 2, 2, 2).expect("half turn");
        assert_eq!(first_bytes(&turned), vec![4, 3, 2, 1]);
    }

    #[test]
    fn three_quarter_turns_are_the_other_way_round() {
        let (_, _, turned) = pack_bgra_rows_turned(&corners(), 8, 2, 2, 3).expect("three quarters");
        assert_eq!(first_bytes(&turned), vec![2, 4, 1, 3]);
    }

    #[test]
    fn a_quarter_turn_swaps_the_side_lengths_and_drops_row_padding() {
        let mut data = Vec::new();
        for value in [1u8, 2, 3, 4, 5, 6] {
            data.extend_from_slice(&[value, value, value, 255]);
            if value % 3 == 0 {
                data.extend_from_slice(&[0xFF; 8]);
            }
        }
        let (width, height, turned) =
            pack_bgra_rows_turned(&data, 20, 3, 2, 1).expect("padded quarter turn");
        assert_eq!((width, height), (2, 3));
        assert_eq!(first_bytes(&turned), vec![4, 1, 5, 2, 6, 3]);
    }

    #[test]
    fn no_turn_packs_exactly_like_the_untured_path() {
        let data = corners();
        let (width, height, turned) = pack_bgra_rows_turned(&data, 8, 2, 2, 0).expect("no turn");
        assert_eq!((width, height), (2, 2));
        assert_eq!(turned, data);
    }

    #[test]
    fn a_turn_rejects_what_the_pack_rejects() {
        assert!(pack_bgra_rows_turned(&[0u8; 8], 8, 2, 2, 1).is_none());
        assert!(pack_bgra_rows_turned(&[0u8; 16], 4, 2, 2, 1).is_none());
        assert!(pack_bgra_rows_turned(&[0u8; 16], 8, 0, 2, 1).is_none());
    }

    #[test]
    fn pack_bgra_rows_rejects_zero_dimensions() {
        assert!(pack_bgra_rows(&[0u8; 16], 8, 0, 2).is_none());
        assert!(pack_bgra_rows(&[0u8; 16], 8, 2, 0).is_none());
    }
}
