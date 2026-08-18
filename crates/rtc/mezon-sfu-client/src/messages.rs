use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        room: String,
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
    },
    Answer {
        sdp: String,
    },
    PushToTalk,
    RoleChange {
        role: String,
    },
    Pong,
    Ping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Joined {
        #[serde(default)]
        room: String,
    },
    Offer {
        sdp: String,
    },
    RoleChanged {
        #[serde(default)]
        user_id: String,
        #[serde(default)]
        role: String,
    },
    PeerLeft {
        #[serde(default)]
        ufrag: String,
        #[serde(default)]
        user_id: String,
        #[serde(default)]
        peer_id: String,
        #[serde(default)]
        mid_audio: String,
        #[serde(default)]
        mid_video: String,
    },
    RoomSnapshot {
        #[serde(default)]
        members: Vec<SnapshotMember>,
    },
    PeerJoined {
        #[serde(default)]
        peer: Option<SnapshotMember>,
    },
    Ping,
    Pong,
    Error {
        message: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMember {
    #[serde(default)]
    pub user_id: String,
}

#[cfg(test)]
mod tests {
    use super::{ClientMessage, ServerMessage};

    #[test]
    fn join_serializes_flat_dropping_none_user_id() {
        let msg = ClientMessage::Join {
            room: "42".to_owned(),
            role: "speaker".to_owned(),
            token: Some("t".to_owned()),
            user_id: None,
        };
        let got: serde_json::Value = serde_json::to_value(&msg).expect("serialize join");
        let want = serde_json::json!({
            "type": "join",
            "room": "42",
            "role": "speaker",
            "token": "t"
        });
        assert_eq!(got, want);
    }

    #[test]
    fn parses_offer() {
        let parsed: ServerMessage =
            serde_json::from_str(r#"{"type":"offer","sdp":"v=0..."}"#).expect("parse offer");
        match parsed {
            ServerMessage::Offer { sdp } => assert_eq!(sdp, "v=0..."),
            other => panic!("expected Offer, got {other:?}"),
        }
    }

    #[test]
    fn parses_peer_left_with_all_quoted_numbers() {
        let parsed: ServerMessage = serde_json::from_str(
            r#"{"type":"peer_left","ufrag":"u","user_id":"7","peer_id":"3","mid_audio":"2","mid_video":"3"}"#,
        )
        .expect("parse peer_left");
        match parsed {
            ServerMessage::PeerLeft {
                ufrag,
                user_id,
                peer_id,
                mid_audio,
                mid_video,
            } => {
                assert_eq!(ufrag, "u");
                assert_eq!(user_id, "7");
                assert_eq!(peer_id, "3");
                assert_eq!(mid_audio, "2");
                assert_eq!(mid_video, "3");
            }
            other => panic!("expected PeerLeft, got {other:?}"),
        }
    }

    #[test]
    fn parses_joined_with_and_without_ice_servers() {
        let with_ice: ServerMessage =
            serde_json::from_str(r#"{"type":"joined","room":"42","iceServers":[]}"#)
                .expect("parse joined+ice");
        let bare: ServerMessage =
            serde_json::from_str(r#"{"type":"joined","room":"42"}"#).expect("parse joined bare");
        assert!(matches!(with_ice, ServerMessage::Joined { .. }));
        match bare {
            ServerMessage::Joined { room } => assert_eq!(room, "42"),
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    #[test]
    fn unknown_message_types_parse_to_unknown() {
        let parsed: ServerMessage =
            serde_json::from_str(r#"{"type":"some_future_type","foo":42}"#)
                .expect("unknown type must not fail the frame");
        assert!(matches!(parsed, ServerMessage::Unknown));
    }

    #[test]
    fn parses_room_snapshot_members() {
        let parsed: ServerMessage = serde_json::from_str(
            r#"{"type":"room_snapshot","room":"1","self_peer_id":3,"participant_count":2,"members":[{"peer_id":3,"user_id":"7","role":"speaker","is_mute":false},{"peer_id":4,"user_id":"9","role":"audience","is_mute":true}]}"#,
        )
        .expect("parse room_snapshot");
        match parsed {
            ServerMessage::RoomSnapshot { members } => {
                let ids: Vec<&str> = members.iter().map(|m| m.user_id.as_str()).collect();
                assert_eq!(ids, ["7", "9"]);
            }
            other => panic!("expected RoomSnapshot, got {other:?}"),
        }
    }

    #[test]
    fn parses_peer_joined() {
        let parsed: ServerMessage = serde_json::from_str(
            r#"{"type":"peer_joined","participant_count":2,"peer":{"peer_id":4,"user_id":"9","role":"speaker","is_mute":false}}"#,
        )
        .expect("parse peer_joined");
        match parsed {
            ServerMessage::PeerJoined { peer } => assert_eq!(peer.unwrap().user_id, "9"),
            other => panic!("expected PeerJoined, got {other:?}"),
        }
    }

    #[test]
    fn parses_error_code() {
        let parsed: ServerMessage =
            serde_json::from_str(r#"{"type":"error","message":"invalid_token"}"#)
                .expect("parse error");
        match parsed {
            ServerMessage::Error { message } => assert_eq!(message, "invalid_token"),
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
