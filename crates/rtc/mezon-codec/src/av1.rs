use dav1d::{Decoder, PixelLayout, PlanarImageComponent, Settings};

use crate::{CodecError, I420Frame};

pub struct Av1Decoder {
    inner: Decoder,
}

impl Av1Decoder {
    pub fn new() -> Result<Self, CodecError> {
        let mut settings = Settings::new();
        settings.set_n_threads(1);
        settings.set_max_frame_delay(1);
        let inner = Decoder::with_settings(&settings)
            .map_err(|e| CodecError::Init(format!("dav1d init failed: {e}")))?;
        Ok(Self { inner })
    }

    pub fn decode(&mut self, obu: &[u8]) -> Result<Vec<I420Frame>, CodecError> {
        match self.inner.send_data(obu.to_vec(), None, None, None) {
            Ok(()) => {}
            Err(e) if e.is_again() => {}
            Err(e) => return Err(CodecError::Decode(format!("dav1d send_data failed: {e}"))),
        }

        let mut frames = Vec::new();
        loop {
            match self.inner.get_picture() {
                Ok(pic) => frames.push(picture_to_i420(&pic)?),
                Err(e) if e.is_again() => break,
                Err(e) => {
                    return Err(CodecError::Decode(format!("dav1d get_picture failed: {e}")));
                }
            }
        }
        Ok(frames)
    }
}

fn picture_to_i420(pic: &dav1d::Picture) -> Result<I420Frame, CodecError> {
    if pic.pixel_layout() != PixelLayout::I420 {
        return Err(CodecError::Decode(format!(
            "unsupported AV1 pixel layout {:?} (only I420 is supported)",
            pic.pixel_layout()
        )));
    }
    if pic.bit_depth() != 8 {
        return Err(CodecError::Decode(format!(
            "unsupported AV1 bit depth {} (only 8-bit is supported)",
            pic.bit_depth()
        )));
    }

    let width = pic.width();
    let height = pic.height();
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);

    let y = copy_plane(pic, PlanarImageComponent::Y, width, height);
    let u = copy_plane(pic, PlanarImageComponent::U, cw, ch);
    let v = copy_plane(pic, PlanarImageComponent::V, cw, ch);

    Ok(I420Frame {
        width,
        height,
        y,
        u,
        v,
        y_stride: width,
        u_stride: cw,
        v_stride: cw,
    })
}

fn copy_plane(pic: &dav1d::Picture, comp: PlanarImageComponent, out_w: u32, out_h: u32) -> Vec<u8> {
    let stride = pic.stride(comp) as usize;
    let plane = pic.plane(comp);
    let src: &[u8] = &plane;
    let ow = out_w as usize;
    let mut dst = vec![0u8; ow * out_h as usize];
    for row in 0..out_h as usize {
        let s = row * stride;
        dst[row * ow..(row + 1) * ow].copy_from_slice(&src[s..s + ow]);
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;
    use rav1e::prelude::*;

    fn encode_keyframe(width: usize, height: usize) -> Vec<u8> {
        let enc = EncoderConfig {
            width,
            height,
            bit_depth: 8,
            chroma_sampling: ChromaSampling::Cs420,
            speed_settings: SpeedSettings::from_preset(10),
            ..Default::default()
        };
        let cfg = Config::new().with_encoder_config(enc);
        let mut ctx: Context<u8> = cfg.new_context().expect("rav1e context");

        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        let y: Vec<u8> = (0..width * height)
            .map(|i| ((i % width) * 255 / width) as u8)
            .collect();
        let u = vec![128u8; cw * ch];
        let v = vec![128u8; cw * ch];

        let mut frame = ctx.new_frame();
        frame.planes[0].copy_from_raw_u8(&y, width, 1);
        frame.planes[1].copy_from_raw_u8(&u, cw, 1);
        frame.planes[2].copy_from_raw_u8(&v, cw, 1);

        ctx.send_frame(frame).expect("send_frame");
        ctx.flush();

        let mut obu = Vec::new();
        loop {
            match ctx.receive_packet() {
                Ok(pkt) => obu.extend_from_slice(&pkt.data),
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::LimitReached) => break,
                Err(EncoderStatus::NeedMoreData) => break,
                Err(e) => panic!("rav1e receive_packet: {e:?}"),
            }
        }
        assert!(!obu.is_empty(), "rav1e produced no OBU data");
        obu
    }

    #[test]
    fn av1_decodes_a_keyframe() {
        let obu = encode_keyframe(64, 48);
        let mut dec = Av1Decoder::new().unwrap();
        let frames = dec.decode(&obu).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!((frames[0].width, frames[0].height), (64, 48));
        assert_eq!(frames[0].y.len(), 64 * 48);
    }
}
