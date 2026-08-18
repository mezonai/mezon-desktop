#[derive(Clone)]
pub struct I420Frame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub y_stride: u32,
    pub u_stride: u32,
    pub v_stride: u32,
}

impl I420Frame {
    pub fn new_black(width: u32, height: u32) -> Self {
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        Self {
            width,
            height,
            y: vec![0u8; (width * height) as usize],
            u: vec![128u8; (cw * ch) as usize],
            v: vec![128u8; (cw * ch) as usize],
            y_stride: width,
            u_stride: cw,
            v_stride: cw,
        }
    }

    pub fn tightly_packed(width: u32, height: u32, y: Vec<u8>, u: Vec<u8>, v: Vec<u8>) -> Self {
        let cw = width.div_ceil(2);
        Self {
            width,
            height,
            y,
            u,
            v,
            y_stride: width,
            u_stride: cw,
            v_stride: cw,
        }
    }
}

#[derive(Clone, Default)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub spatial_layer: u8,
    pub temporal_layer: u8,
}

pub struct AudioFrame<'a> {
    pub samples: &'a [f32],
    pub channels: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_frame_has_correct_plane_sizes() {
        let f = I420Frame::new_black(1280, 720);
        assert_eq!(f.y.len(), 1280 * 720);
        assert_eq!(f.u.len(), 640 * 360);
        assert_eq!(f.v.len(), 640 * 360);
        assert_eq!(f.y_stride, 1280);
        assert_eq!(f.u_stride, 640);
        assert!(f.y.iter().all(|&b| b == 0));
        assert!(f.u.iter().all(|&b| b == 128));
    }
}
