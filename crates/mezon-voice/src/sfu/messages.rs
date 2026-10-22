use serde::{Deserialize, Deserializer, Serialize};

use super::mid::RemoteKind;

pub const MEDIA_POOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        room: String,
        token: String,
        role: &'static str,
        screen_codec: &'static str,
        media_pool_version: u32,
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
    RequestKeyframe {
        kind: &'static str,
        publisher_id: u32,
    },
    ParticipantAction {
        token: String,
    },
    SendMessage {
        message: String,
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
        #[serde(default)]
        media_pool_mode: String,
        #[serde(default)]
        max_audio_slots: u32,
        #[serde(default)]
        max_camera_slots: u32,
        #[serde(default)]
        max_screen_slots: u32,
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
        #[serde(default, deserialize_with = "flexible_generation")]
        assignment_generation: u64,
        #[serde(default)]
        peer_id: u32,
        #[serde(default, deserialize_with = "flexible_mid")]
        mid_audio: u32,
        #[serde(default, deserialize_with = "flexible_mid")]
        mid_video: u32,
        #[serde(default, deserialize_with = "flexible_mid")]
        mid_screen: u32,
    },
    SlotAssigned(AssignedSlot),
    SlotReleased {
        #[serde(default)]
        kind: Option<RemoteKind>,
        slot: u32,
        #[serde(deserialize_with = "flexible_generation")]
        generation: u64,
    },
    MuteChanged {
        #[serde(default)]
        is_mute: bool,
    },
    PushToTalkChanged {
        #[serde(default)]
        active: bool,
    },
    KeyframeRequested {
        success: bool,
        cached: bool,
        kind: String,
    },
    VisibilityChanged {
        #[serde(default)]
        visible: bool,
    },
    RoleChanged {
        #[serde(default)]
        role: String,
    },
    RoomMessage {
        #[serde(default)]
        message: String,
    },
    MessageSent,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SnapshotMember {
    #[serde(default, alias = "remote_slot")]
    pub slot: Option<u32>,
    #[serde(default, deserialize_with = "flexible_generation")]
    pub assignment_generation: u64,
    #[serde(default)]
    pub peer_id: u32,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub metadata: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub is_mute: bool,
    #[serde(default)]
    pub camera_active: bool,
    #[serde(default)]
    pub screen_requested: Option<bool>,
    #[serde(default)]
    pub screen_active: bool,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_audio: u32,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_video: u32,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_screen: u32,
    #[serde(default)]
    pub media_slots: Option<MediaSlots>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct MediaSlots {
    #[serde(default)]
    pub audio: Option<PoolSlot>,
    #[serde(default)]
    pub camera: Option<PoolSlot>,
    #[serde(default)]
    pub screen: Option<PoolSlot>,
}

impl MediaSlots {
    pub fn entries(&self) -> impl Iterator<Item = (RemoteKind, PoolSlot)> {
        [
            (RemoteKind::Audio, self.audio),
            (RemoteKind::Camera, self.camera),
            (RemoteKind::Screen, self.screen),
        ]
        .into_iter()
        .filter_map(|(kind, slot)| slot.map(|slot| (kind, slot)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct PoolSlot {
    pub slot: u32,
    #[serde(deserialize_with = "flexible_generation")]
    pub generation: u64,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum AssignedSlot {
    Pool(PoolSlotAssignment),
    Triple(SlotAssignment),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PoolSlotAssignment {
    pub kind: RemoteKind,
    pub slot: u32,
    #[serde(deserialize_with = "flexible_generation")]
    pub generation: u64,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid: u32,
    pub peer_id: u32,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SlotAssignment {
    pub slot: u32,
    #[serde(deserialize_with = "flexible_generation")]
    pub generation: u64,
    pub peer_id: u32,
    #[serde(default)]
    pub user_id: String,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_audio: u32,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_video: u32,
    #[serde(default, deserialize_with = "flexible_mid")]
    pub mid_screen: u32,
    pub audio_active: bool,
    pub video_active: bool,
    pub screen_active: bool,
}

impl SlotAssignment {
    pub fn mids(&self) -> Option<[u32; 3]> {
        let base = self.slot.checked_mul(3)?.checked_add(3)?;
        let expected = [base, base.checked_add(1)?, base.checked_add(2)?];
        let supplied = [self.mid_audio, self.mid_video, self.mid_screen];
        (self.generation > 0
            && self.peer_id != 0
            && supplied
                .iter()
                .zip(expected)
                .all(|(value, mid)| *value == 0 || *value == mid))
        .then_some(expected)
    }
}

fn flexible_generation<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Num(u64),
        Str(String),
    }
    match Option::<Repr>::deserialize(deserializer)? {
        Some(Repr::Num(n)) => Ok(n),
        Some(Repr::Str(s)) => s.parse().map_err(serde::de::Error::custom),
        None => Ok(0),
    }
}

impl SnapshotMember {
    pub fn is_sharing_screen(&self) -> bool {
        self.screen_active && self.screen_requested.unwrap_or(true)
    }

    pub fn is_audience(&self) -> bool {
        self.role == "audience"
    }
}

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
            media_pool_version: MEDIA_POOL_VERSION,
        };
        let got: serde_json::Value = serde_json::to_value(&msg).expect("serialize join");
        assert_eq!(
            got,
            serde_json::json!({
                "type": "join",
                "room": "42",
                "token": "jwt",
                "role": "audience",
                "screen_codec": "vp8",
                "media_pool_version": 1
            })
        );
    }

    #[test]
    fn push_to_talk_carries_the_active_flag() {
        let got: serde_json::Value =
            serde_json::to_value(ClientMessage::PushToTalk { active: true }).expect("serialize");
        assert_eq!(
            got,
            serde_json::json!({"type": "push_to_talk", "active": true})
        );
    }

    #[test]
    fn screen_keyframe_request_uses_numeric_publisher_id() {
        let value = serde_json::to_value(ClientMessage::RequestKeyframe {
            kind: "screen",
            publisher_id: 31,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type": "request_keyframe", "kind": "screen", "publisher_id": 31})
        );
    }

    #[test]
    fn keyframe_ack_parses_without_inventing_a_publisher_id() {
        assert_eq!(
            parse(r#"{"type":"keyframe_requested","success":true,"cached":false,"kind":"screen"}"#),
            ServerMessage::KeyframeRequested {
                success: true,
                cached: false,
                kind: "screen".into()
            }
        );
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
        let ServerMessage::Joined {
            room, ice_servers, ..
        } = msg
        else {
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
    fn joined_announces_a_decoupled_pool_layout() {
        let msg = parse(
            r#"{"type":"joined","room":"7","media_pool_version":1,"media_pool_mode":"decoupled",
                "max_audio_slots":6,"max_camera_slots":8,"max_screen_slots":4}"#,
        );
        let ServerMessage::Joined {
            media_pool_mode,
            max_audio_slots,
            max_camera_slots,
            max_screen_slots,
            ..
        } = msg
        else {
            panic!("expected joined");
        };
        assert_eq!(media_pool_mode, "decoupled");
        assert_eq!(
            (max_audio_slots, max_camera_slots, max_screen_slots),
            (6, 8, 4)
        );
    }

    #[test]
    fn slot_events_with_a_kind_address_one_media_pool() {
        let assigned = parse(
            r#"{"type":"slot_assigned","kind":"camera","slot":0,"generation":1,"mid":9,
                "peer_id":123,"user_id":"1779484387973271552","active":true}"#,
        );
        assert_eq!(
            assigned,
            ServerMessage::SlotAssigned(AssignedSlot::Pool(PoolSlotAssignment {
                kind: RemoteKind::Camera,
                slot: 0,
                generation: 1,
                mid: 9,
                peer_id: 123,
                user_id: "1779484387973271552".to_owned(),
                active: true
            }))
        );
        assert_eq!(
            parse(r#"{"type":"slot_released","kind":"screen","slot":2,"generation":7}"#),
            ServerMessage::SlotReleased {
                kind: Some(RemoteKind::Screen),
                slot: 2,
                generation: 7
            }
        );
    }

    #[test]
    fn slot_events_without_a_kind_keep_the_triple_layout() {
        let assigned = parse(
            r#"{"type":"slot_assigned","slot":1,"generation":4,"peer_id":7,"user_id":"9",
                "mid_audio":6,"mid_video":7,"mid_screen":8,
                "audio_active":true,"video_active":false,"screen_active":false}"#,
        );
        let ServerMessage::SlotAssigned(AssignedSlot::Triple(assignment)) = assigned else {
            panic!("expected a triple slot assignment");
        };
        assert_eq!(assignment.mids(), Some([6, 7, 8]));
        assert_eq!(
            parse(r#"{"type":"slot_released","slot":1,"generation":4}"#),
            ServerMessage::SlotReleased {
                kind: None,
                slot: 1,
                generation: 4
            }
        );
    }

    #[test]
    fn decoupled_members_carry_their_media_slots() {
        let msg = parse(
            r#"{"type":"peer_joined","peer":{"peer_id":4,"user_id":"9","role":"speaker",
                "media_slots":{"audio":{"slot":0,"generation":1,"mid":3},
                "camera":{"slot":2,"generation":1,"mid":11},"screen":null}}}"#,
        );
        let ServerMessage::PeerJoined { peer: Some(peer) } = msg else {
            panic!("expected peer_joined");
        };
        let slots = peer.media_slots.expect("media slots present");
        let granted: Vec<_> = slots
            .entries()
            .map(|(kind, slot)| (kind, slot.mid))
            .collect();
        assert_eq!(granted, [(RemoteKind::Audio, 3), (RemoteKind::Camera, 11)]);
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
    fn peer_joined_carries_the_metadata_the_peer_joined_with() {
        let msg = parse(
            r#"{"type":"peer_joined","room_revision":2,"participant_count":2,"peer":{
                "peer_id":4,"user_id":"1977","metadata":"alice;https://cdn/a.png",
                "role":"speaker","is_mute":false}}"#,
        );
        let ServerMessage::PeerJoined { peer } = msg else {
            panic!("expected peer_joined");
        };
        assert_eq!(
            peer.expect("peer present").metadata,
            "alice;https://cdn/a.png"
        );
    }

    #[test]
    fn a_room_message_carries_the_sender_payload() {
        let msg = parse(
            r#"{"type":"room_message","message":"{\"content\":\"hi\"}","user_id":"1977","peer_id":4}"#,
        );
        assert_eq!(
            msg,
            ServerMessage::RoomMessage {
                message: r#"{"content":"hi"}"#.to_owned()
            }
        );
    }

    #[test]
    fn the_sfu_acknowledges_a_relayed_room_message() {
        assert_eq!(
            parse(r#"{"type":"message_sent"}"#),
            ServerMessage::MessageSent
        );
    }

    #[test]
    fn send_message_serializes_to_the_sfu_chat_frame() {
        let frame = serde_json::to_value(ClientMessage::SendMessage {
            message: "hello".to_owned(),
        })
        .unwrap();
        assert_eq!(
            frame,
            serde_json::json!({"type": "send_message", "message": "hello"})
        );
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
                assignment_generation: 1,
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
                assignment_generation: 0,
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
