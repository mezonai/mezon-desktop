//! Wire types for the mezon-sfu WebSocket signaling protocol.
//!
//! The field names and shapes here mirror `src/protocol/signaling/signaling.c`
//! in the mezon-sfu repository. Two quirks are worth knowing before editing:
//!
//! * The server derives the room id from the JWT in [`ClientMessage::Join`],
//!   not from its `room` field. `room` is sent for parity with the other
//!   clients and is ignored server-side.
//! * The server never sends `role_changed`; that identifier only exists as a
//!   local variable in the C source. Promotion and demotion are observed
//!   through [`ServerMessage::PushToTalkChanged`]. The variant is still parsed
//!   so a future server that does send it does not trip the `Unknown` arm.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        room: String,
        token: String,
        role: &'static str,
        /// Which codec the SFU should offer on the screen uplink. Omitting it
        /// makes the SFU offer both, VP9 first, and libwebrtc would then pick
        /// VP9 because it comes first in the m-line.
        screen_codec: &'static str,
    },
    Answer {
        sdp: String,
        offer_generation: u64,
    },
    Mute {
        is_mute: bool,
    },
    Camera {
        active: bool,
    },
    ShareScreen {
        active: bool,
    },
    PushToTalk {
        active: bool,
    },
    Visibility {
        visible: bool,
    },
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Joined {
        #[serde(default)]
        room: String,
        #[serde(default, rename = "iceServers")]
        ice_servers: Vec<IceServerSpec>,
    },
    Offer {
        #[serde(default)]
        offer_generation: u64,
        sdp: String,
    },
    RoomSnapshot {
        #[serde(default)]
        self_peer_id: u32,
        #[serde(default)]
        members: Vec<SnapshotMember>,
    },
    PeerJoined {
        #[serde(default)]
        peer: Option<SnapshotMember>,
    },
    PeerUpdated {
        #[serde(default)]
        peer: Option<SnapshotMember>,
    },
    PeerLeft {
        #[serde(default)]
        peer_id: u32,
        #[serde(default, deserialize_with = "flexible_mid")]
        mid_audio: u32,
        #[serde(default, deserialize_with = "flexible_mid")]
        mid_video: u32,
        #[serde(default, deserialize_with = "flexible_mid")]
        mid_screen: u32,
    },
    MuteChanged {
        #[serde(default)]
        is_mute: bool,
    },
    PushToTalkChanged {
        #[serde(default)]
        active: bool,
    },
    VisibilityChanged {
        #[serde(default)]
        visible: bool,
    },
    RoleChanged {
        #[serde(default)]
        role: String,
    },
    Ping,
    Pong,
    Error {
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IceServerSpec {
    #[serde(default, deserialize_with = "one_or_many")]
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SnapshotMember {
    #[serde(default)]
    pub peer_id: u32,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub is_mute: bool,
    #[serde(default)]
    pub camera_active: bool,
    #[serde(default)]
    pub screen_active: bool,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_audio: u32,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_video: u32,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_screen: u32,
}

impl SnapshotMember {
    pub fn is_audience(&self) -> bool {
        self.role == "audience"
    }
}

/// `peer_updated` omits the mid fields entirely, and older SFU builds quoted
/// them as strings where the current one emits bare numbers. Accept both rather
/// than failing the whole frame over a number that may not even be present.
fn flexible_mid<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Num(u32),
        Str(String),
    }

    Ok(match Option::<Repr>::deserialize(deserializer)? {
        Some(Repr::Num(n)) => n,
        Some(Repr::Str(s)) => s.trim().parse().unwrap_or(0),
        None => 0,
    })
}

fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        One(String),
        Many(Vec<String>),
    }

    Ok(match Option::<Repr>::deserialize(deserializer)? {
        Some(Repr::One(s)) => vec![s],
        Some(Repr::Many(v)) => v,
        None => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> ServerMessage {
        serde_json::from_str(raw).expect("parse server message")
    }

    #[test]
    fn join_serializes_to_the_shape_the_c_server_scans_for() {
        let msg = ClientMessage::Join {
            room: "42".to_owned(),
            token: "jwt".to_owned(),
            role: "audience",
            screen_codec: "vp8",
        };
        let got: serde_json::Value = serde_json::to_value(&msg).expect("serialize join");
        assert_eq!(
            got,
            serde_json::json!({
                "type": "join",
                "room": "42",
                "token": "jwt",
                "role": "audience",
                "screen_codec": "vp8"
            })
        );
    }

    #[test]
    fn push_to_talk_carries_the_active_flag() {
        let got: serde_json::Value =
            serde_json::to_value(ClientMessage::PushToTalk { active: true }).expect("serialize");
        assert_eq!(got, serde_json::json!({"type": "push_to_talk", "active": true}));
    }

    #[test]
    fn pong_serializes_without_a_payload() {
        let got: serde_json::Value = serde_json::to_value(ClientMessage::Pong).expect("serialize");
        assert_eq!(got, serde_json::json!({"type": "pong"}));
    }

    #[test]
    fn joined_carries_the_ice_servers_the_sfu_minted() {
        let msg = parse(
            r#"{"type":"joined","room":"7","iceServers":[
                {"urls":"stun:1.2.3.4:3478"},
                {"urls":"turn:1.2.3.4:3478?transport=udp","username":"u","credential":"p"}
            ]}"#,
        );
        let ServerMessage::Joined { room, ice_servers } = msg else {
            panic!("expected joined");
        };
        assert_eq!(room, "7");
        assert_eq!(ice_servers.len(), 2);
        assert_eq!(ice_servers[0].urls, ["stun:1.2.3.4:3478"]);
        assert_eq!(ice_servers[1].username, "u");
        assert_eq!(ice_servers[1].credential, "p");
    }

    #[test]
    fn joined_without_ice_servers_still_parses() {
        let msg = parse(r#"{"type":"joined","room":"7"}"#);
        let ServerMessage::Joined { ice_servers, .. } = msg else {
            panic!("expected joined");
        };
        assert!(ice_servers.is_empty());
    }

    #[test]
    fn offer_keeps_the_generation_for_the_answer() {
        let msg = parse(r#"{"type":"offer","offer_generation":9,"sdp":"v=0"}"#);
        assert_eq!(
            msg,
            ServerMessage::Offer {
                offer_generation: 9,
                sdp: "v=0".to_owned()
            }
        );
    }

    #[test]
    fn room_snapshot_members_carry_role_mute_and_mids() {
        let msg = parse(
            r#"{"type":"room_snapshot","room":"1","room_revision":4,"self_peer_id":3,
                "participant_count":2,"members":[
                {"peer_id":3,"user_id":"7","role":"speaker","is_mute":false,
                 "camera_requested":false,"camera_active":true,"screen_requested":false,
                 "screen_active":false,"ufrag":"ab","mid_audio":3,"mid_video":4,
                 "mid_screen":5,"slot":0,"assignment_generation":1},
                {"peer_id":4,"user_id":"9","role":"audience","is_mute":true,
                 "camera_requested":false,"camera_active":false,"screen_requested":false,
                 "screen_active":false,"ufrag":"cd","mid_audio":6,"mid_video":7,
                 "mid_screen":8,"slot":1,"assignment_generation":1}]}"#,
        );
        let ServerMessage::RoomSnapshot {
            self_peer_id,
            members,
        } = msg
        else {
            panic!("expected room_snapshot");
        };
        assert_eq!(self_peer_id, 3);
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].user_id, "7");
        assert!(!members[0].is_audience());
        assert!(members[0].camera_active);
        assert_eq!(members[0].mid_audio, 3);
        assert!(members[1].is_audience());
        assert!(members[1].is_mute);
        assert_eq!(members[1].mid_screen, 8);
    }

    #[test]
    fn peer_updated_without_mids_defaults_them_to_zero() {
        let msg = parse(
            r#"{"type":"peer_updated","peer":{"peer_id":4,"user_id":"9","role":"speaker",
                "is_mute":false,"camera_requested":true,"camera_active":true,
                "screen_requested":false,"screen_active":false}}"#,
        );
        let ServerMessage::PeerUpdated { peer } = msg else {
            panic!("expected peer_updated");
        };
        let peer = peer.expect("peer present");
        assert_eq!(peer.user_id, "9");
        assert!(peer.camera_active);
        assert_eq!(peer.mid_audio, 0);
    }

    #[test]
    fn peer_left_accepts_numeric_mids() {
        let msg = parse(
            r#"{"type":"peer_left","room_revision":5,"participant_count":1,"ufrag":"cd",
                "user_id":"9","peer_id":4,"mid_audio":6,"mid_video":7,"mid_screen":8,
                "slot":1,"assignment_generation":1}"#,
        );
        assert_eq!(
            msg,
            ServerMessage::PeerLeft {
                peer_id: 4,
                mid_audio: 6,
                mid_video: 7,
                mid_screen: 8
            }
        );
    }

    #[test]
    fn peer_left_accepts_quoted_mids_from_older_builds() {
        let msg = parse(
            r#"{"type":"peer_left","user_id":"9","peer_id":4,
                "mid_audio":"6","mid_video":"7","mid_screen":"8"}"#,
        );
        assert_eq!(
            msg,
            ServerMessage::PeerLeft {
                peer_id: 4,
                mid_audio: 6,
                mid_video: 7,
                mid_screen: 8
            }
        );
    }

    #[test]
    fn push_to_talk_changed_is_the_promotion_signal() {
        assert_eq!(
            parse(r#"{"type":"push_to_talk_changed","active":true}"#),
            ServerMessage::PushToTalkChanged { active: true }
        );
    }

    #[test]
    fn error_carries_the_code_in_message() {
        assert_eq!(
            parse(r#"{"type":"error","message":"invalid_token"}"#),
            ServerMessage::Error {
                message: "invalid_token".to_owned()
            }
        );
    }

    #[test]
    fn unrecognised_types_do_not_fail_the_frame() {
        assert_eq!(
            parse(r#"{"type":"participant_action_completed","ok":true}"#),
            ServerMessage::Unknown
        );
    }
}
