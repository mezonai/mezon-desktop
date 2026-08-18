mod codec;
mod engine;

pub use codec::{AnswerPayload, IcePayload, OfferPayload, compress_sdp, decompress_sdp};
pub use engine::{CallConfig, CallEngine, EngineCommand, EngineEvent};

pub const WEBRTC_SDP_INIT: i32 = 0;
pub const WEBRTC_SDP_OFFER: i32 = 1;
pub const WEBRTC_SDP_ANSWER: i32 = 2;
pub const WEBRTC_ICE_CANDIDATE: i32 = 3;
pub const WEBRTC_SDP_QUIT: i32 = 4;
pub const WEBRTC_SDP_TIMEOUT: i32 = 5;
pub const WEBRTC_SDP_JOINED_OTHER_CALL: i32 = 7;
pub const WEBRTC_SDP_STATUS_REMOTE_MEDIA: i32 = 8;
pub const WEBRTC_CLEAR_CALL: i32 = 50;

pub const REMOTE_FRAME_KEY: u64 = 0xCA11_0001;
