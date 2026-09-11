use std::collections::HashMap;

pub const MID_AUDIO: &str = "0";
pub const MID_CAMERA: &str = "1";
pub const MID_SCREEN: &str = "2";

const FIRST_REMOTE_MID: u32 = 3;
const MIDS_PER_REMOTE: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteKind {
    Audio,
    Camera,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteMid {
    pub slot: u32,
    pub kind: RemoteKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsidOccupant {
    pub user_id: String,
    pub peer_id: u32,
}

pub fn is_local_mid(mid: &str) -> bool {
    matches!(mid, MID_AUDIO | MID_CAMERA | MID_SCREEN)
}

pub fn classify(mid: &str) -> Option<RemoteMid> {
    let n: u32 = mid.trim().parse().ok()?;
    let offset = n.checked_sub(FIRST_REMOTE_MID)?;
    let kind = match offset % MIDS_PER_REMOTE {
        0 => RemoteKind::Audio,
        1 => RemoteKind::Camera,
        _ => RemoteKind::Screen,
    };
    Some(RemoteMid {
        slot: offset / MIDS_PER_REMOTE,
        kind,
    })
}

pub fn parse_msid_occupants(sdp: &str) -> HashMap<String, MsidOccupant> {
    let mut out = HashMap::new();
    let mut current_mid: Option<&str> = None;

    for raw in sdp.lines() {
        let line = raw.trim();
        if line.starts_with("m=") {
            current_mid = None;
        } else if let Some(mid) = line.strip_prefix("a=mid:") {
            current_mid = Some(mid.trim());
        } else if let Some(msid) = line.strip_prefix("a=msid:")
            && let Some(mid) = current_mid
            && let Some(occupant) = msid.split_whitespace().find_map(occupant_in_token)
        {
            out.insert(mid.to_owned(), occupant);
        }
    }

    out
}

fn occupant_in_token(token: &str) -> Option<MsidOccupant> {
    let mut user_id = None;
    let mut peer_id = 0;
    for segment in token.split('-') {
        if let Some(digits) = segment.strip_prefix('u')
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            user_id = Some(digits.to_owned());
        } else if let Some(digits) = segment.strip_prefix('p')
            && let Ok(id) = digits.parse::<u32>()
        {
            peer_id = id;
        }
    }
    user_id.map(|user_id| MsidOccupant { user_id, peer_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_three_mids_are_local_uplinks() {
        assert!(is_local_mid("0"));
        assert!(is_local_mid("1"));
        assert!(is_local_mid("2"));
        assert!(!is_local_mid("3"));
        assert_eq!(classify("0"), None);
        assert_eq!(classify("2"), None);
    }

    #[test]
    fn remote_mids_group_into_threes_per_slot() {
        assert_eq!(
            classify("3"),
            Some(RemoteMid {
                slot: 0,
                kind: RemoteKind::Audio
            })
        );
        assert_eq!(
            classify("4"),
            Some(RemoteMid {
                slot: 0,
                kind: RemoteKind::Camera
            })
        );
        assert_eq!(
            classify("5"),
            Some(RemoteMid {
                slot: 0,
                kind: RemoteKind::Screen
            })
        );
        assert_eq!(
            classify("6"),
            Some(RemoteMid {
                slot: 1,
                kind: RemoteKind::Audio
            })
        );
        assert_eq!(
            classify("11"),
            Some(RemoteMid {
                slot: 2,
                kind: RemoteKind::Screen
            })
        );
    }

    #[test]
    fn non_numeric_mids_are_ignored_rather_than_panicking() {
        assert_eq!(classify("audio"), None);
        assert_eq!(classify(""), None);
    }

    #[test]
    fn msid_occupants_are_keyed_by_the_mid_of_their_section() {
        let sdp = "v=0\r\n\
                   m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
                   a=mid:3\r\n\
                   a=msid:u1234-p7 audio-u1234-p7\r\n\
                   m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
                   a=mid:4\r\n\
                   a=msid:u1234-p7 video-u1234-p7\r\n\
                   m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
                   a=mid:6\r\n\
                   a=msid:u98-p12 audio-u98-p12\r\n";
        let map = parse_msid_occupants(sdp);
        assert_eq!(
            map.get("3"),
            Some(&MsidOccupant {
                user_id: "1234".to_owned(),
                peer_id: 7
            })
        );
        assert_eq!(map.get("4").map(|o| o.peer_id), Some(7));
        assert_eq!(
            map.get("6"),
            Some(&MsidOccupant {
                user_id: "98".to_owned(),
                peer_id: 12
            })
        );
    }

    #[test]
    fn a_new_m_line_clears_the_mid_so_msids_do_not_leak_across_sections() {
        let sdp = "m=audio 9 RTP/SAVPF 111\r\n\
                   a=mid:3\r\n\
                   m=video 9 RTP/SAVPF 96\r\n\
                   a=msid:room-u55-cam track-b\r\n";
        assert!(parse_msid_occupants(sdp).is_empty());
    }

    #[test]
    fn msids_without_a_user_segment_are_skipped() {
        let sdp = "m=audio 9 RTP/SAVPF 111\r\n\
                   a=mid:3\r\n\
                   a=msid:- track-a\r\n";
        assert!(parse_msid_occupants(sdp).is_empty());
    }

    #[test]
    fn a_user_segment_must_be_all_digits_after_the_u() {
        assert_eq!(occupant_in_token("room-u12ab-cam"), None);
        assert_eq!(occupant_in_token("room-u-cam"), None);
        assert_eq!(
            occupant_in_token("u77").map(|o| o.user_id),
            Some("77".to_owned())
        );
    }

    #[test]
    fn the_peer_segment_is_optional_and_must_be_numeric() {
        assert_eq!(occupant_in_token("u8-p9").map(|o| o.peer_id), Some(9));
        assert_eq!(occupant_in_token("audio-u8-p9").map(|o| o.peer_id), Some(9));
        assert_eq!(occupant_in_token("room-u1234-cam").map(|o| o.peer_id), Some(0));
        assert_eq!(occupant_in_token("u5-pxx").map(|o| o.peer_id), Some(0));
    }
}
