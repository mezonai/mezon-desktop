//! Black-box integration test: drive the public API exactly as a downstream
//! consumer (`mezon-rtc`) would — encode then decode both an audio and a video
//! stream and confirm the media survives the trip.

use mezon_codec::{
    AudioFrame, I420Frame, OpusDecoder, OpusEncoder, VpxCodec, VpxDecoder, VpxEncoder,
};

#[test]
fn public_api_audio_and_video_roundtrip() {
    // audio
    let mut ae = OpusEncoder::new(1, 32_000).unwrap();
    let mut ad = OpusDecoder::new(1).unwrap();
    let sine: Vec<f32> = (0..960)
        .map(|n| (n as f32 / 48000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();
    let mut pcm = vec![];
    for _ in 0..5 {
        pcm = ad
            .decode(
                &ae.encode(AudioFrame {
                    samples: &sine,
                    channels: 1,
                })
                .unwrap(),
            )
            .unwrap();
    }
    assert_eq!(pcm.len(), 960);

    // video
    let mut ve = VpxEncoder::new(VpxCodec::Vp9, 320, 240, 400).unwrap();
    let mut vd = VpxDecoder::new(VpxCodec::Vp9).unwrap();
    let pkts = ve.encode(&I420Frame::new_black(320, 240), true, 0).unwrap();
    let mut out = vec![];
    for p in &pkts {
        out.extend(vd.decode(&p.data).unwrap());
    }
    assert_eq!(
        out.last().map(|f| (f.width, f.height)),
        Some((320, 240))
    );
}
