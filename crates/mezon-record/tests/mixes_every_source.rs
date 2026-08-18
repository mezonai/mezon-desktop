use std::path::PathBuf;
use std::time::Duration;

use mezon_record::{AudioSource, RECORD_CHANNELS, RECORD_SAMPLE_RATE, Recorder, RecorderConfig};

const BLOCK_FRAMES: u32 = RECORD_SAMPLE_RATE / 100;
const SECONDS: u32 = 2;

fn silence() -> Vec<i16> {
    vec![0i16; (BLOCK_FRAMES * RECORD_CHANNELS) as usize]
}

fn tone(step: u32) -> Vec<i16> {
    (0..BLOCK_FRAMES * RECORD_CHANNELS)
        .map(|index| {
            let frame = step * BLOCK_FRAMES + index / RECORD_CHANNELS;
            let t = frame as f32 / RECORD_SAMPLE_RATE as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 12000.0) as i16
        })
        .collect()
}

fn peak_amplitude(path: &PathBuf) -> Option<i32> {
    let wav = path.with_extension("probe.wav");
    let status = std::process::Command::new("afconvert")
        .args(["-f", "WAVE", "-d", "LEI16"])
        .arg(path)
        .arg(&wav)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let bytes = std::fs::read(&wav).ok()?;
    let _ = std::fs::remove_file(&wav);
    let peak = bytes[44..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs() as i32)
        .max()?;
    Some(peak)
}

/// Sharing a video while nobody in the call is talking is the case that broke:
/// the only sound in the room comes from a source the recorder does not use to
/// pace itself, so it must survive on its own.
#[test]
fn screen_audio_lands_in_the_recording_when_only_silence_comes_from_the_call() {
    if !mezon_record::is_supported() {
        eprintln!("skipping: this platform has no call-recording encoder");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir
        .path()
        .join(format!("call.{}", mezon_record::file_extension()));
    let recorder = Recorder::start(RecorderConfig {
        path: path.clone(),
        video: None,
    })
    .expect("the recorder should start");

    let audio = recorder.audio_tap();
    for step in 0..SECONDS * 100 {
        audio.push(
            AudioSource::Remote,
            &silence(),
            RECORD_SAMPLE_RATE,
            RECORD_CHANNELS,
        );
        audio.push(
            AudioSource::Screen,
            &tone(step),
            RECORD_SAMPLE_RATE,
            RECORD_CHANNELS,
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let written = recorder.finish().expect("the recorder should finalise");
    let Some(peak) = peak_amplitude(&written) else {
        eprintln!("skipping the amplitude check: no decoder on this machine");
        return;
    };
    assert!(
        peak > 2000,
        "the shared screen's audio never made it into the recording (peak {peak})"
    );
}
