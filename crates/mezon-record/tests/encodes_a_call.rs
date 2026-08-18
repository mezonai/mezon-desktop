use std::path::PathBuf;
use std::time::Duration;

use mezon_record::{
    AudioSource, PixelData, RECORD_CHANNELS, RECORD_SAMPLE_RATE, Recorder, RecorderConfig,
    VideoConfig, VideoFrameRef,
};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const FPS: u32 = 30;
const SECONDS: u32 = 2;

fn output_path(extension: &str) -> (PathBuf, Option<tempfile::TempDir>) {
    match std::env::var_os("MEZON_RECORD_TEST_OUTPUT") {
        Some(path) => (PathBuf::from(path), None),
        None => {
            let dir = tempfile::tempdir().expect("temp dir");
            let path = dir.path().join(format!("call.{extension}"));
            (path, Some(dir))
        }
    }
}

fn frame_bytes(step: u32) -> Vec<u8> {
    let mut data = vec![0u8; WIDTH as usize * HEIGHT as usize * 4];
    for (index, pixel) in data.chunks_exact_mut(4).enumerate() {
        let column = (index % WIDTH as usize) as u32;
        pixel[0] = ((column + step * 8) % 256) as u8;
        pixel[1] = (step * 4 % 256) as u8;
        pixel[2] = ((index / WIDTH as usize) % 256) as u8;
        pixel[3] = 255;
    }
    data
}

fn tone(step: u32) -> Vec<i16> {
    let per_frame = RECORD_SAMPLE_RATE / FPS;
    (0..per_frame * RECORD_CHANNELS)
        .map(|index| {
            let t = (step * per_frame + index / RECORD_CHANNELS) as f32 / RECORD_SAMPLE_RATE as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16
        })
        .collect()
}

#[test]
fn a_call_recording_lands_a_playable_file() {
    if !mezon_record::is_supported() {
        eprintln!("skipping: this platform has no call-recording encoder");
        return;
    }

    let extension = mezon_record::file_extension();
    let (path, _guard) = output_path(extension);
    let recorder = Recorder::start(RecorderConfig {
        path: path.clone(),
        video: Some(VideoConfig {
            width: WIDTH,
            height: HEIGHT,
            fps: FPS,
        }),
    })
    .expect("the recorder should start");

    let audio = recorder.audio_tap();
    let video = recorder.video_tap();
    for step in 0..FPS * SECONDS {
        audio.push(
            AudioSource::Remote,
            &tone(step),
            RECORD_SAMPLE_RATE,
            RECORD_CHANNELS,
        );
        let pixels = frame_bytes(step);
        video.push(VideoFrameRef {
            width: WIDTH,
            height: HEIGHT,
            data: PixelData::Bgra {
                data: &pixels,
                stride: WIDTH as usize * 4,
            },
        });
        std::thread::sleep(Duration::from_millis(1000 / FPS as u64));
    }

    let stats = recorder.stats();
    assert!(stats.video_frames > 0, "no video frame reached the encoder");
    let written = recorder.finish().expect("the recorder should finalise");

    assert_eq!(written, path);
    let stem = path
        .file_stem()
        .expect("stem")
        .to_string_lossy()
        .to_string();
    let part = path.with_file_name(format!("{stem}.part.{extension}"));
    assert!(
        !part.exists(),
        "the .part file should be renamed away on a clean finish"
    );

    let bytes = std::fs::read(&written).expect("the recording should be readable");
    assert!(
        bytes.len() > 10_000,
        "a {SECONDS}s recording should not be {} bytes",
        bytes.len()
    );
    match extension {
        "mp4" | "mov" => assert_eq!(
            &bytes[4..8],
            b"ftyp",
            "an ISO base media file carries ftyp in its first box"
        ),
        _ => assert_eq!(
            &bytes[..4],
            &[0x1a, 0x45, 0xdf, 0xa3],
            "webm and mkv both start with the EBML magic"
        ),
    }
    assert_eq!(
        stats.dropped_video_frames, 0,
        "the encoder should keep up with {FPS} fps"
    );
}
