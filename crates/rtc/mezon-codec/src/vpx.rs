#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]

use std::ffi::{CStr, c_int, c_void};

use crate::{CodecError, EncodedFrame, I420Frame};

mod ffi {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/vpx_bindings.rs"));

    unsafe extern "C" {
        pub fn mezon_vpx_enc_init(
            ctx: *mut vpx_codec_ctx_t,
            iface: *mut vpx_codec_iface_t,
            cfg: *const vpx_codec_enc_cfg_t,
            flags: vpx_codec_flags_t,
        ) -> vpx_codec_err_t;

        pub fn mezon_vpx_dec_init(
            ctx: *mut vpx_codec_ctx_t,
            iface: *mut vpx_codec_iface_t,
            cfg: *const vpx_codec_dec_cfg_t,
            flags: vpx_codec_flags_t,
        ) -> vpx_codec_err_t;

        pub fn mezon_vpx_control_int(
            ctx: *mut vpx_codec_ctx_t,
            ctrl_id: ::std::os::raw::c_int,
            val: ::std::os::raw::c_int,
        ) -> vpx_codec_err_t;

        pub fn mezon_vpx_set_svc_params(
            ctx: *mut vpx_codec_ctx_t,
            params: *const vpx_svc_extra_cfg_t,
        ) -> vpx_codec_err_t;

        pub fn mezon_vpx_get_svc_layer_id(
            ctx: *mut vpx_codec_ctx_t,
            out: *mut vpx_svc_layer_id_t,
        ) -> vpx_codec_err_t;

        pub fn mezon_vpx_register_cx_callback(
            ctx: *mut vpx_codec_ctx_t,
            cb: ::std::option::Option<
                unsafe extern "C" fn(pkt: *mut vpx_codec_cx_pkt_t, user: *mut ::std::os::raw::c_void),
            >,
            user: *mut ::std::os::raw::c_void,
        ) -> vpx_codec_err_t;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VpxCodec {
    Vp8,
    Vp9,
}

impl VpxCodec {
    fn enc_iface(self) -> *mut ffi::vpx_codec_iface_t {
        unsafe {
            match self {
                VpxCodec::Vp8 => ffi::vpx_codec_vp8_cx(),
                VpxCodec::Vp9 => ffi::vpx_codec_vp9_cx(),
            }
        }
        .cast_mut()
    }

    fn dec_iface(self) -> *mut ffi::vpx_codec_iface_t {
        unsafe {
            match self {
                VpxCodec::Vp8 => ffi::vpx_codec_vp8_dx(),
                VpxCodec::Vp9 => ffi::vpx_codec_vp9_dx(),
            }
        }
        .cast_mut()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SvcConfig {
    pub spatial_layers: u8,
    pub temporal_layers: u8,
    pub ksvc: bool,
}

struct Ctx(Box<ffi::vpx_codec_ctx_t>);

// SAFETY: the context is only ever touched behind `&mut self`, so moving the
// owning encoder/decoder between threads is sound.
unsafe impl Send for Ctx {}

impl Ctx {
    fn uninit() -> Self {
        Ctx(Box::new(unsafe { std::mem::zeroed() }))
    }

    fn as_ptr(&mut self) -> *mut ffi::vpx_codec_ctx_t {
        std::ptr::addr_of_mut!(*self.0)
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        unsafe { ffi::vpx_codec_destroy(std::ptr::addr_of_mut!(*self.0)) };
    }
}

unsafe fn ctx_error(ctx: *mut ffi::vpx_codec_ctx_t) -> String {
    let msg = unsafe { ffi::vpx_codec_error(ctx) };
    if msg.is_null() {
        "unknown libvpx error".to_string()
    } else {
        unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned()
    }
}

fn check(
    ctx: *mut ffi::vpx_codec_ctx_t,
    err: ffi::vpx_codec_err_t,
    wrap: fn(String) -> CodecError,
) -> Result<(), CodecError> {
    if err == ffi::VPX_CODEC_OK {
        Ok(())
    } else {
        Err(wrap(format!(
            "libvpx error {err}: {}",
            unsafe { ctx_error(ctx) }
        )))
    }
}

struct SvcCollector {
    ctx: *mut ffi::vpx_codec_ctx_t,
    frames: Vec<EncodedFrame>,
    counter: u8,
}

struct SvcState {
    collector: Box<SvcCollector>,
    #[allow(dead_code)]
    config: SvcConfig,
}

unsafe extern "C" fn svc_pkt_cb(pkt: *mut ffi::vpx_codec_cx_pkt_t, user: *mut c_void) {
    if pkt.is_null() || user.is_null() {
        return;
    }
    let collector = unsafe { &mut *(user as *mut SvcCollector) };
    let pkt = unsafe { &*pkt };
    if pkt.kind != ffi::VPX_CODEC_CX_FRAME_PKT {
        return;
    }
    let frame = unsafe { &pkt.data.frame };
    if frame.buf.is_null() || frame.sz == 0 {
        return;
    }
    let data = unsafe { std::slice::from_raw_parts(frame.buf as *const u8, frame.sz) }.to_vec();
    let is_keyframe = (frame.flags & ffi::VPX_FRAME_IS_KEY) != 0;

    let spatial_layer = collector.counter;
    collector.counter = collector.counter.saturating_add(1);

    let mut layer: ffi::vpx_svc_layer_id_t = unsafe { std::mem::zeroed() };
    let temporal_layer =
        if unsafe { ffi::mezon_vpx_get_svc_layer_id(collector.ctx, &mut layer) } == ffi::VPX_CODEC_OK
        {
            layer.temporal_layer_id.max(0) as u8
        } else {
            0
        };

    collector.frames.push(EncodedFrame {
        data,
        is_keyframe,
        spatial_layer,
        temporal_layer,
    });
}

pub struct VpxEncoder {
    ctx: Ctx,
    cfg: ffi::vpx_codec_enc_cfg_t,
    codec: VpxCodec,
    svc: Option<SvcState>,
}

// SAFETY: see `Ctx`; the raw pointers held by `svc` point into boxed
// allocations owned by `self`, moved together with it.
unsafe impl Send for VpxEncoder {}

fn init_encoder_ctx(
    codec: VpxCodec,
    cfg: &ffi::vpx_codec_enc_cfg_t,
) -> Result<Ctx, CodecError> {
    let mut ctx = Ctx::uninit();
    let ptr = ctx.as_ptr();
    let err = unsafe { ffi::mezon_vpx_enc_init(ptr, codec.enc_iface(), cfg, 0) };
    check(ptr, err, CodecError::Init)?;
    let cpu_used = match codec {
        VpxCodec::Vp8 => 4,
        VpxCodec::Vp9 => 8,
    };
    unsafe { ffi::mezon_vpx_control_int(ptr, ffi::VP8E_SET_CPUUSED as c_int, cpu_used) };
    Ok(ctx)
}

impl VpxEncoder {
    pub fn new(
        codec: VpxCodec,
        width: u32,
        height: u32,
        target_bitrate_kbps: u32,
    ) -> Result<Self, CodecError> {
        let mut cfg: ffi::vpx_codec_enc_cfg_t = unsafe { std::mem::zeroed() };
        let err = unsafe { ffi::vpx_codec_enc_config_default(codec.enc_iface(), &mut cfg, 0) };
        if err != ffi::VPX_CODEC_OK {
            return Err(CodecError::Init(format!(
                "vpx_codec_enc_config_default failed: {err}"
            )));
        }
        cfg.g_w = width;
        cfg.g_h = height;
        cfg.g_timebase.num = 1;
        cfg.g_timebase.den = 90_000;
        cfg.rc_target_bitrate = target_bitrate_kbps;
        cfg.rc_end_usage = ffi::VPX_CBR;
        cfg.g_error_resilient = ffi::VPX_ERROR_RESILIENT_DEFAULT as ffi::vpx_codec_er_flags_t;
        cfg.g_lag_in_frames = 0;
        cfg.g_pass = ffi::VPX_RC_ONE_PASS;
        cfg.g_threads = 1;
        if codec == VpxCodec::Vp9 {
            cfg.g_profile = 0;
        }

        let ctx = init_encoder_ctx(codec, &cfg)?;
        Ok(Self {
            ctx,
            cfg,
            codec,
            svc: None,
        })
    }

    pub fn set_bitrate(&mut self, kbps: u32) -> Result<(), CodecError> {
        self.cfg.rc_target_bitrate = kbps;
        let ptr = self.ctx.as_ptr();
        let err = unsafe { ffi::vpx_codec_enc_config_set(ptr, &self.cfg) };
        check(ptr, err, CodecError::Init)
    }

    pub fn enable_vp9_svc(
        &mut self,
        cfg: SvcConfig,
        per_layer_kbps: &[u32],
    ) -> Result<(), CodecError> {
        if self.codec != VpxCodec::Vp9 {
            return Err(CodecError::Init(
                "SVC is only supported for VP9".to_string(),
            ));
        }
        let spatial = cfg.spatial_layers.clamp(1, 5) as usize;
        let temporal = cfg.temporal_layers.clamp(1, 3) as usize;

        self.cfg.ss_number_layers = spatial as u32;
        self.cfg.ts_number_layers = temporal as u32;

        if temporal > 1 {
            let periodicity = 1usize << (temporal - 1);
            self.cfg.ts_periodicity = periodicity as u32;
            for t in 0..temporal {
                self.cfg.ts_rate_decimator[t] = 1u32 << (temporal - 1 - t);
            }
            for i in 0..periodicity {
                let layer = if i == 0 {
                    0
                } else {
                    temporal - 1 - (i.trailing_zeros() as usize).min(temporal - 1)
                };
                self.cfg.ts_layer_id[i] = layer as u32;
            }
        }

        let total: u32 = per_layer_kbps.iter().copied().sum();
        if total > 0 {
            self.cfg.rc_target_bitrate = total;
        }
        for s in 0..spatial {
            let sl_kbps = per_layer_kbps.get(s).copied().unwrap_or(0);
            if s < 5 {
                self.cfg.ss_target_bitrate[s] = sl_kbps;
            }
            for t in 0..temporal {
                let idx = s * temporal + t;
                if idx < self.cfg.layer_target_bitrate.len() {
                    self.cfg.layer_target_bitrate[idx] =
                        sl_kbps * (t as u32 + 1) / temporal as u32;
                }
            }
        }

        self.ctx = init_encoder_ctx(self.codec, &self.cfg)?;
        let ptr = self.ctx.as_ptr();

        check(
            ptr,
            unsafe { ffi::mezon_vpx_control_int(ptr, ffi::VP9E_SET_SVC as c_int, 1) },
            CodecError::Init,
        )?;

        if cfg.ksvc && spatial >= 2 {
            check(
                ptr,
                unsafe {
                    ffi::mezon_vpx_control_int(
                        ptr,
                        ffi::VP9E_SET_SVC_INTER_LAYER_PRED as c_int,
                        2,
                    )
                },
                CodecError::Init,
            )?;
        }

        let mut params: ffi::vpx_svc_extra_cfg_t = unsafe { std::mem::zeroed() };
        for s in 0..spatial {
            params.scaling_factor_num[s] = 1;
            params.scaling_factor_den[s] = 1 << (spatial - 1 - s);
            params.max_quantizers[s] = 63;
            params.min_quantizers[s] = 0;
            params.speed_per_layer[s] = 8;
        }
        check(
            ptr,
            unsafe { ffi::mezon_vpx_set_svc_params(ptr, &params) },
            CodecError::Init,
        )?;

        let mut collector = Box::new(SvcCollector {
            ctx: ptr,
            frames: Vec::new(),
            counter: 0,
        });
        let user = std::ptr::addr_of_mut!(*collector) as *mut c_void;
        check(
            ptr,
            unsafe { ffi::mezon_vpx_register_cx_callback(ptr, Some(svc_pkt_cb), user) },
            CodecError::Init,
        )?;

        self.svc = Some(SvcState {
            collector,
            config: cfg,
        });
        Ok(())
    }

    pub fn encode(
        &mut self,
        frame: &I420Frame,
        force_keyframe: bool,
        pts: i64,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        let image = wrap_i420(frame);
        let flags: ffi::vpx_enc_frame_flags_t = if force_keyframe {
            ffi::VPX_EFLAG_FORCE_KF as ffi::vpx_enc_frame_flags_t
        } else {
            0
        };

        let ptr = self.ctx.as_ptr();
        if let Some(svc) = self.svc.as_mut() {
            svc.collector.frames.clear();
            svc.collector.counter = 0;
        }

        let err = unsafe {
            ffi::vpx_codec_encode(
                ptr,
                &image,
                pts,
                1,
                flags,
                ffi::VPX_DL_REALTIME as std::os::raw::c_ulong,
            )
        };
        check(ptr, err, CodecError::Encode)?;

        if let Some(svc) = self.svc.as_mut() {
            return Ok(std::mem::take(&mut svc.collector.frames));
        }

        let mut out = Vec::new();
        let mut iter: ffi::vpx_codec_iter_t = std::ptr::null();
        loop {
            let pkt = unsafe { ffi::vpx_codec_get_cx_data(ptr, &mut iter) };
            if pkt.is_null() {
                break;
            }
            let pkt = unsafe { &*pkt };
            if pkt.kind != ffi::VPX_CODEC_CX_FRAME_PKT {
                continue;
            }
            let f = unsafe { &pkt.data.frame };
            let data =
                unsafe { std::slice::from_raw_parts(f.buf as *const u8, f.sz) }.to_vec();
            out.push(EncodedFrame {
                data,
                is_keyframe: (f.flags & ffi::VPX_FRAME_IS_KEY) != 0,
                spatial_layer: 0,
                temporal_layer: 0,
            });
        }
        Ok(out)
    }
}

fn wrap_i420(frame: &I420Frame) -> ffi::vpx_image_t {
    let mut image = std::mem::MaybeUninit::<ffi::vpx_image_t>::zeroed();
    unsafe {
        ffi::vpx_img_wrap(
            image.as_mut_ptr(),
            ffi::VPX_IMG_FMT_I420,
            frame.width,
            frame.height,
            1,
            std::ptr::without_provenance_mut(1),
        );
        let img = &mut *image.as_mut_ptr();
        img.planes[0] = frame.y.as_ptr().cast_mut();
        img.planes[1] = frame.u.as_ptr().cast_mut();
        img.planes[2] = frame.v.as_ptr().cast_mut();
        img.stride[0] = frame.y_stride as c_int;
        img.stride[1] = frame.u_stride as c_int;
        img.stride[2] = frame.v_stride as c_int;
        image.assume_init()
    }
}

pub struct VpxDecoder {
    ctx: Ctx,
}

unsafe impl Send for VpxDecoder {}

impl VpxDecoder {
    pub fn new(codec: VpxCodec) -> Result<Self, CodecError> {
        let mut ctx = Ctx::uninit();
        let ptr = ctx.as_ptr();
        let err = unsafe {
            ffi::mezon_vpx_dec_init(ptr, codec.dec_iface(), std::ptr::null(), 0)
        };
        check(ptr, err, CodecError::Init)?;
        Ok(Self { ctx })
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<I420Frame>, CodecError> {
        let ptr = self.ctx.as_ptr();
        let err = unsafe {
            ffi::vpx_codec_decode(
                ptr,
                data.as_ptr(),
                data.len() as std::os::raw::c_uint,
                std::ptr::null_mut(),
                0,
            )
        };
        check(ptr, err, CodecError::Decode)?;

        let mut out = Vec::new();
        let mut iter: ffi::vpx_codec_iter_t = std::ptr::null();
        loop {
            let img = unsafe { ffi::vpx_codec_get_frame(ptr, &mut iter) };
            if img.is_null() {
                break;
            }
            out.push(unsafe { image_to_i420(img) });
        }
        Ok(out)
    }
}

unsafe fn image_to_i420(img: *const ffi::vpx_image_t) -> I420Frame {
    let img = unsafe { &*img };
    let width = img.d_w;
    let height = img.d_h;
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);

    let mut y = vec![0u8; (width * height) as usize];
    let mut u = vec![0u8; (cw * ch) as usize];
    let mut v = vec![0u8; (cw * ch) as usize];
    unsafe {
        copy_plane(img.planes[0], img.stride[0], width, height, &mut y);
        copy_plane(img.planes[1], img.stride[1], cw, ch, &mut u);
        copy_plane(img.planes[2], img.stride[2], cw, ch, &mut v);
    }

    I420Frame {
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

unsafe fn copy_plane(src: *const u8, stride: c_int, width: u32, height: u32, dst: &mut [u8]) {
    let stride = stride as usize;
    let width = width as usize;
    for row in 0..height as usize {
        let src_row = unsafe { src.add(row * stride) };
        let dst_start = row * width;
        unsafe {
            std::ptr::copy_nonoverlapping(src_row, dst[dst_start..].as_mut_ptr(), width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::I420Frame;

    fn gradient(width: u32, height: u32) -> I420Frame {
        let mut f = I420Frame::new_black(width, height);
        for row in 0..height {
            for col in 0..width {
                f.y[(row * width + col) as usize] = ((col * 255) / width) as u8;
            }
        }
        f
    }

    #[test]
    fn vp8_encode_decode_roundtrip_preserves_dimensions() {
        let (w, h) = (320u32, 240u32);
        let mut enc = VpxEncoder::new(VpxCodec::Vp8, w, h, 300).unwrap();
        let mut dec = VpxDecoder::new(VpxCodec::Vp8).unwrap();

        let frame = gradient(w, h);
        let pkts = enc.encode(&frame, true, 0).unwrap();
        assert!(
            pkts.iter().any(|p| p.is_keyframe),
            "first frame must be a keyframe"
        );

        let mut decoded = Vec::new();
        for p in &pkts {
            decoded.extend(dec.decode(&p.data).unwrap());
        }
        let out = decoded.pop().expect("one decoded frame");
        assert_eq!((out.width, out.height), (w, h));
        let left = out.y[(h / 2 * out.y_stride) as usize] as i32;
        let right = out.y[(h / 2 * out.y_stride + out.width - 1) as usize] as i32;
        assert!(
            right - left > 40,
            "gradient should survive (left={left}, right={right})"
        );
    }

    #[test]
    fn vp9_profile0_roundtrip_preserves_dimensions() {
        let (w, h) = (320u32, 240u32);
        let mut enc = VpxEncoder::new(VpxCodec::Vp9, w, h, 400).unwrap();
        let mut dec = VpxDecoder::new(VpxCodec::Vp9).unwrap();
        let frame = gradient(w, h);
        let pkts = enc.encode(&frame, true, 0).unwrap();
        assert!(pkts.iter().any(|p| p.is_keyframe));
        let mut decoded = Vec::new();
        for p in &pkts {
            decoded.extend(dec.decode(&p.data).unwrap());
        }
        let out = decoded.pop().expect("one decoded frame");
        assert_eq!((out.width, out.height), (w, h));
    }

    #[test]
    fn vp9_svc_produces_multiple_spatial_layers() {
        let (w, h) = (640u32, 480u32);
        let mut enc = VpxEncoder::new(VpxCodec::Vp9, w, h, 800).unwrap();
        enc.enable_vp9_svc(
            SvcConfig {
                spatial_layers: 2,
                temporal_layers: 1,
                ksvc: false,
            },
            &[300, 500],
        )
        .unwrap();
        let frame = gradient(w, h);
        let pkts = enc.encode(&frame, true, 0).unwrap();
        let layers: std::collections::BTreeSet<u8> =
            pkts.iter().map(|p| p.spatial_layer).collect();
        assert!(
            layers.len() >= 2,
            "expected >=2 spatial layers, got {layers:?}"
        );
    }

    #[test]
    fn vp9_svc_l3t3_enables_three_spatial_layers() {
        let (w, h) = (640u32, 480u32);
        let mut enc = VpxEncoder::new(VpxCodec::Vp9, w, h, 1200).unwrap();
        enc.enable_vp9_svc(
            SvcConfig {
                spatial_layers: 3,
                temporal_layers: 3,
                ksvc: false,
            },
            &[300, 300, 600],
        )
        .expect("l3t3 SVC must enable without libvpx errors");
        let frame = gradient(w, h);
        let pkts = enc.encode(&frame, true, 0).unwrap();
        let layers: std::collections::BTreeSet<u8> =
            pkts.iter().map(|p| p.spatial_layer).collect();
        assert!(
            layers.len() >= 3,
            "expected 3 spatial layers for l3t3, got {layers:?}"
        );
    }

    #[test]
    fn vp9_svc_l3t3_key_enables_ksvc() {
        let (w, h) = (640u32, 480u32);
        let mut enc = VpxEncoder::new(VpxCodec::Vp9, w, h, 1200).unwrap();
        enc.enable_vp9_svc(
            SvcConfig {
                spatial_layers: 3,
                temporal_layers: 3,
                ksvc: true,
            },
            &[300, 300, 600],
        )
        .expect("l3t3_key (K-SVC) must enable without libvpx errors");
        let frame = gradient(w, h);
        let pkts = enc.encode(&frame, true, 0).unwrap();
        assert!(pkts.iter().any(|p| p.is_keyframe));
    }

    #[test]
    fn vp9_svc_l2t2_enables_two_spatial_layers() {
        let (w, h) = (640u32, 480u32);
        let mut enc = VpxEncoder::new(VpxCodec::Vp9, w, h, 900).unwrap();
        enc.enable_vp9_svc(
            SvcConfig {
                spatial_layers: 2,
                temporal_layers: 2,
                ksvc: false,
            },
            &[300, 600],
        )
        .expect("l2t2 SVC must enable without libvpx errors");
        let frame = gradient(w, h);
        let pkts = enc.encode(&frame, true, 0).unwrap();
        let layers: std::collections::BTreeSet<u8> =
            pkts.iter().map(|p| p.spatial_layer).collect();
        assert!(layers.len() >= 2, "expected 2 spatial layers, got {layers:?}");
    }

    #[test]
    fn vp9_svc_l1t3_single_spatial_temporal_only() {
        let (w, h) = (640u32, 480u32);
        let mut enc = VpxEncoder::new(VpxCodec::Vp9, w, h, 800).unwrap();
        enc.enable_vp9_svc(
            SvcConfig {
                spatial_layers: 1,
                temporal_layers: 3,
                ksvc: false,
            },
            &[800],
        )
        .expect("l1t3 (temporal-only) SVC must enable without libvpx errors");
        let frame = gradient(w, h);
        let pkts = enc.encode(&frame, true, 0).unwrap();
        assert!(pkts.iter().any(|p| p.is_keyframe));
    }
}
