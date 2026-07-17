mod decode;
mod encode;
mod playback;
mod stream;

pub use decode::{DecodedPcm, decode_audio};
pub use encode::{VoiceEncoder, VoiceRecording};
pub use playback::AudioPlayer;
pub use stream::{PcmStream, spawn_stream_decode};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("failed to demux audio: {0}")]
    Demux(String),
    #[error("failed to decode audio: {0}")]
    Decode(String),
    #[error("failed to encode audio: {0}")]
    Encode(String),
    #[error("no audio track in stream")]
    NoAudioTrack,
    #[error("no audio output device")]
    NoOutputDevice,
    #[error("audio output error: {0}")]
    Output(String),
}
