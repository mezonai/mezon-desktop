//! Client for the in-house mezon-sfu Selective Forwarding Unit.
//!
//! [`engine`] owns the live connection; the other three modules are pure so the
//! parts that historically break — the wire format, the m-line layout, and the
//! two SDP rewrites — can be tested without a server or a PeerConnection.

pub mod engine;
pub mod messages;
pub mod mid;
pub mod sdp;

pub use engine::{ScreenTrack, SfuConfig, SfuEngine, SfuEvent, SfuPeer, SfuRole};
