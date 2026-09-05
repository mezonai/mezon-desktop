//! Mapping between SDP m-line identifiers and the participants behind them.
//!
//! mezon-sfu lays the bundle out on a fixed schedule: the first three m-lines
//! are the local uplinks, and every remote participant owns the next three in
//! order. Nothing in the SDP states this — the layout is the contract.

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

pub fn is_local_mid(mid: &str) -> bool {
    matches!(mid, MID_AUDIO | MID_CAMERA | MID_SCREEN)
}

/// `None` for the three local uplinks and for anything that is not a plain
/// number, so callers can skip an m-line without a second parse.
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

/// Best-effort `mid -> user_id` recovered from the offer.
///
/// The membership messages are the primary source; this parse only fills the
/// window between an offer arriving and the matching `peer_joined`. The SFU
/// encodes the id as a `u<digits>` segment inside the msid, as in
/// `a=msid:room-u1234-cam track-7`.
pub fn parse_msid_user_ids(sdp: &str) -> HashMap<String, String> {
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
            && let Some(user_id) = msid.split_whitespace().find_map(user_id_in_token)
        {
            out.insert(mid.to_owned(), user_id);
        }
    }

    out
}

fn user_id_in_token(token: &str) -> Option<String> {
    token.split('-').find_map(|segment| {
        let digits = segment.strip_prefix('u')?;
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| digits.to_owned())
    })
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
    fn msid_user_ids_are_keyed_by_the_mid_of_their_section() {
        let sdp = "v=0\r\n\
                   m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
                   a=mid:3\r\n\
                   a=msid:room-u1234-mic track-a\r\n\
                   m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
                   a=mid:4\r\n\
                   a=msid:room-u1234-cam track-b\r\n\
                   m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
                   a=mid:6\r\n\
                   a=msid:room-u98-mic track-c\r\n";
        let map = parse_msid_user_ids(sdp);
        assert_eq!(map.get("3").map(String::as_str), Some("1234"));
        assert_eq!(map.get("4").map(String::as_str), Some("1234"));
        assert_eq!(map.get("6").map(String::as_str), Some("98"));
    }

    #[test]
    fn a_new_m_line_clears_the_mid_so_msids_do_not_leak_across_sections() {
        let sdp = "m=audio 9 RTP/SAVPF 111\r\n\
                   a=mid:3\r\n\
                   m=video 9 RTP/SAVPF 96\r\n\
                   a=msid:room-u55-cam track-b\r\n";
        assert!(parse_msid_user_ids(sdp).is_empty());
    }

    #[test]
    fn msids_without_a_user_segment_are_skipped() {
        let sdp = "m=audio 9 RTP/SAVPF 111\r\n\
                   a=mid:3\r\n\
                   a=msid:- track-a\r\n";
        assert!(parse_msid_user_ids(sdp).is_empty());
    }

    #[test]
    fn a_user_segment_must_be_all_digits_after_the_u() {
        assert_eq!(user_id_in_token("room-u12ab-cam"), None);
        assert_eq!(user_id_in_token("room-u-cam"), None);
        assert_eq!(user_id_in_token("u77"), Some("77".to_owned()));
    }
}
