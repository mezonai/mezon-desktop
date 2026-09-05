
use super::mid::{self, MID_CAMERA, MID_SCREEN};

pub fn stabilize_inactive_video_sections(offer_sdp: &str, current_remote_sdp: Option<&str>) -> String {
    let Some(previous_sdp) = current_remote_sdp.filter(|s| !s.is_empty()) else {
        return offer_sdp.to_owned();
    };

    let previous = split_sections(previous_sdp);
    let Split {
        session,
        media: sections,
    } = split_sections(offer_sdp);

    let mut changed = false;
    let mut rebuilt: Vec<Vec<String>> = Vec::with_capacity(sections.len());

    for section in sections {
        let Some(stabilized) = stabilize_one(&section, &previous.media) else {
            rebuilt.push(section);
            continue;
        };
        changed = true;
        rebuilt.push(stabilized);
    }

    if !changed {
        return offer_sdp.to_owned();
    }

    let mut out = String::with_capacity(offer_sdp.len());
    for line in session {
        out.push_str(&line);
        out.push_str("\r\n");
    }
    for section in rebuilt {
        for line in section {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}

fn stabilize_one(section: &[String], previous: &[Vec<String>]) -> Option<Vec<String>> {
    let head = section.first()?;
    if !head.starts_with("m=video ") {
        return None;
    }
    if !section.iter().any(|l| l == "a=inactive") {
        return None;
    }

    let mid = section_mid(section)?;
    if mid.parse::<u32>().ok()? < 3 {
        return None;
    }

    let prev = previous
        .iter()
        .find(|s| section_mid(s).as_deref() == Some(mid.as_str()))?;
    if !prev.first().is_some_and(|l| l.starts_with("m=video ")) {
        return None;
    }

    let prev_codecs: Vec<String> = prev.iter().filter(|l| is_codec_line(l)).cloned().collect();
    if prev_codecs.is_empty() {
        return None;
    }

    let mut out: Vec<String> = section
        .iter()
        .filter(|l| !is_codec_line(l))
        .cloned()
        .collect();
    out[0] = prev[0].clone();
    let insert_at = out
        .iter()
        .position(|l| l == "a=rtcp-mux")
        .map_or(out.len(), |idx| idx + 1);
    out.splice(insert_at..insert_at, prev_codecs);
    Some(out)
}

pub fn patch_answer_for_sfu(sdp: &str, is_audience: bool) -> String {
    if !is_audience {
        return sdp.to_owned();
    }

    let split = split_sections(sdp);
    let mut changed = false;
    let mut rebuilt: Vec<Vec<String>> = Vec::with_capacity(split.media.len());

    for mut section in split.media {
        let is_video = section.first().is_some_and(|l| l.starts_with("m=video"));
        let is_camera_uplink = section_mid(&section).as_deref() == Some(MID_CAMERA);
        if is_video && is_camera_uplink {
            for line in section.iter_mut() {
                if line == "a=inactive" {
                    *line = "a=sendonly".to_owned();
                    changed = true;
                }
            }
        }
        rebuilt.push(section);
    }

    if !changed {
        return sdp.to_owned();
    }

    let mut out = String::with_capacity(sdp.len());
    for line in split.session {
        out.push_str(&line);
        out.push_str("\r\n");
    }
    for section in rebuilt {
        for line in section {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}

pub fn force_uplink_sendonly(answer_sdp: &str, offer_sdp: &str) -> String {
    const DIRECTIONS: [&str; 4] = ["a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive"];

    let offered = directions_by_mid(offer_sdp);
    let split = split_sections(answer_sdp);
    let mut changed = false;
    let mut rebuilt: Vec<Vec<String>> = Vec::with_capacity(split.media.len());

    for mut section in split.media {
        let invited = section_mid(&section).is_some_and(|mid| {
            mid::is_local_mid(&mid)
                && matches!(
                    offered.get(&mid).map(String::as_str),
                    Some("a=recvonly") | Some("a=sendrecv")
                )
        });
        if invited {
            match section
                .iter()
                .position(|line| DIRECTIONS.contains(&line.as_str()))
            {
                Some(idx) => {
                    if section[idx] != "a=sendonly" {
                        section[idx] = "a=sendonly".to_owned();
                        changed = true;
                    }
                }
                None => {
                    section.push("a=sendonly".to_owned());
                    changed = true;
                }
            }
        }
        rebuilt.push(section);
    }

    if !changed {
        return answer_sdp.to_owned();
    }

    let mut out = String::with_capacity(answer_sdp.len());
    for line in split.session {
        out.push_str(&line);
        out.push_str("\r\n");
    }
    for section in rebuilt {
        for line in section {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}

fn directions_by_mid(sdp: &str) -> std::collections::HashMap<String, String> {
    const DIRECTIONS: [&str; 4] = ["a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive"];
    let mut out = std::collections::HashMap::new();
    for section in split_sections(sdp).media {
        let Some(mid) = section_mid(&section) else {
            continue;
        };
        if let Some(direction) = section
            .iter()
            .find(|line| DIRECTIONS.contains(&line.as_str()))
        {
            out.insert(mid, direction.clone());
        }
    }
    out
}

pub fn codec_summary(sdp: &str) -> String {
    let mut out = String::new();
    for section in split_sections(sdp).media {
        let mid = section_mid(&section).unwrap_or_else(|| "?".to_owned());
        let codecs: Vec<String> = section
            .iter()
            .filter_map(|line| line.strip_prefix("a=rtpmap:"))
            .filter_map(|rest| {
                let (pt, description) = rest.split_once(' ')?;
                let name = description.split('/').next()?;
                Some(format!("{pt}/{name}"))
            })
            .collect();
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{mid}:[{}]", codecs.join(",")));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitrateLimits {
    pub min_kbps: u32,
    pub start_kbps: u32,
    pub max_kbps: u32,
}

pub fn munge_uplink_bitrates(sdp: &str, camera: BitrateLimits, screen: BitrateLimits) -> String {
    let split = split_sections(sdp);
    let mut rebuilt: Vec<Vec<String>> = Vec::with_capacity(split.media.len());
    let mut changed = false;

    for section in split.media {
        let limits = match section_mid(&section).as_deref() {
            Some(MID_CAMERA) => camera,
            Some(MID_SCREEN) => screen,
            _ => {
                rebuilt.push(section);
                continue;
            }
        };
        let (patched, touched) = apply_bitrate_hints(section, limits);
        changed |= touched;
        rebuilt.push(patched);
    }

    if !changed {
        return sdp.to_owned();
    }

    let mut out = String::with_capacity(sdp.len() + 128);
    for line in split.session {
        out.push_str(&line);
        out.push_str("\r\n");
    }
    for section in rebuilt {
        for line in section {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}

fn apply_bitrate_hints(section: Vec<String>, limits: BitrateLimits) -> (Vec<String>, bool) {
    let payload_types: Vec<String> = section
        .iter()
        .filter_map(|line| line.strip_prefix("a=rtpmap:"))
        .filter_map(|rest| {
            let (pt, description) = rest.split_once(' ')?;
            let name = description.split('/').next()?;
            (!name.eq_ignore_ascii_case("rtx")).then(|| pt.to_owned())
        })
        .collect();

    let hints = format!(
        "x-google-min-bitrate={};x-google-start-bitrate={};x-google-max-bitrate={}",
        limits.min_kbps, limits.start_kbps, limits.max_kbps
    );

    let mut out = section;
    let mut changed = false;
    for pt in payload_types {
        let fmtp_prefix = format!("a=fmtp:{pt} ");
        match out.iter().position(|line| line.starts_with(&fmtp_prefix)) {
            Some(idx) => {
                let existing = out[idx][fmtp_prefix.len()..].to_owned();
                let kept = strip_bitrate_hints(&existing);
                out[idx] = if kept.is_empty() {
                    format!("{fmtp_prefix}{hints}")
                } else {
                    format!("{fmtp_prefix}{kept};{hints}")
                };
            }
            None => {
                let rtpmap_prefix = format!("a=rtpmap:{pt} ");
                let Some(idx) = out.iter().position(|l| l.starts_with(&rtpmap_prefix)) else {
                    continue;
                };
                out.insert(idx + 1, format!("{fmtp_prefix}{hints}"));
            }
        }
        changed = true;
    }
    (out, changed)
}

fn strip_bitrate_hints(fmtp: &str) -> String {
    fmtp.split(';')
        .map(str::trim)
        .filter(|part| {
            !part.is_empty()
                && !part.starts_with("x-google-min-bitrate")
                && !part.starts_with("x-google-start-bitrate")
                && !part.starts_with("x-google-max-bitrate")
        })
        .collect::<Vec<_>>()
        .join(";")
}

pub fn setup_roles(sdp: &str) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for line in sdp.lines() {
        let Some(role) = line.trim().strip_prefix("a=setup:") else {
            continue;
        };
        let role = role.trim();
        if !role.is_empty() && !seen.contains(&role) {
            seen.push(role);
        }
    }
    if seen.is_empty() {
        "-".to_owned()
    } else {
        seen.join(",")
    }
}

pub fn ice_ufrags(sdp: &str) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for line in sdp.lines() {
        let Some(ufrag) = line.trim().strip_prefix("a=ice-ufrag:") else {
            continue;
        };
        let ufrag = ufrag.trim();
        if !ufrag.is_empty() && !seen.contains(&ufrag) {
            seen.push(ufrag);
        }
    }
    if seen.is_empty() {
        "-".to_owned()
    } else {
        seen.join(",")
    }
}

pub fn direction_summary(sdp: &str) -> String {
    let split = split_sections(sdp);
    let mut out = String::new();
    for section in &split.media {
        let mid = section_mid(section).unwrap_or_else(|| "?".to_owned());
        let kind = match section.first() {
            Some(line) if line.starts_with("m=audio") => "a",
            Some(line) if line.starts_with("m=video") => "v",
            _ => "?",
        };
        let direction = section
            .iter()
            .find_map(|line| match line.as_str() {
                "a=sendrecv" => Some("sendrecv"),
                "a=sendonly" => Some("sendonly"),
                "a=recvonly" => Some("recvonly"),
                "a=inactive" => Some("inactive"),
                _ => None,
            })
            .unwrap_or("?");
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{mid}{kind}={direction}"));
    }
    out
}

struct Split {
    session: Vec<String>,
    media: Vec<Vec<String>>,
}

fn split_sections(sdp: &str) -> Split {
    let mut session = Vec::new();
    let mut media: Vec<Vec<String>> = Vec::new();

    for raw in sdp.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            continue;
        }
        if line.starts_with("m=") {
            media.push(vec![line.to_owned()]);
        } else if let Some(current) = media.last_mut() {
            current.push(line.to_owned());
        } else {
            session.push(line.to_owned());
        }
    }

    Split { session, media }
}

fn section_mid(section: &[String]) -> Option<String> {
    section
        .iter()
        .find_map(|l| l.strip_prefix("a=mid:"))
        .map(|m| m.trim().to_owned())
}

fn is_codec_line(line: &str) -> bool {
    line.starts_with("a=rtpmap:") || line.starts_with("a=fmtp:") || line.starts_with("a=rtcp-fb:")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREV: &str = "v=0\r\n\
        m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
        a=mid:0\r\n\
        m=video 9 UDP/TLS/RTP/SAVPF 96 97\r\n\
        a=mid:4\r\n\
        a=rtcp-mux\r\n\
        a=rtpmap:96 VP8/90000\r\n\
        a=rtcp-fb:96 nack\r\n\
        a=fmtp:97 apt=96\r\n\
        a=sendonly\r\n";

    #[test]
    fn without_a_previous_answer_the_offer_is_untouched() {
        let offer = "v=0\r\nm=video 9 RTP/SAVPF 96\r\na=mid:4\r\na=inactive\r\n";
        assert_eq!(stabilize_inactive_video_sections(offer, None), offer);
        assert_eq!(stabilize_inactive_video_sections(offer, Some("")), offer);
    }

    #[test]
    fn an_inactive_remote_video_section_regains_its_codec_lines() {
        let offer = "v=0\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            m=video 0 UDP/TLS/RTP/SAVPF 96\r\n\
            a=mid:4\r\n\
            a=rtcp-mux\r\n\
            a=inactive\r\n";
        let got = stabilize_inactive_video_sections(offer, Some(PREV));
        assert!(got.contains("m=video 9 UDP/TLS/RTP/SAVPF 96 97\r\n"));
        assert!(got.contains("a=rtpmap:96 VP8/90000\r\n"));
        assert!(got.contains("a=rtcp-fb:96 nack\r\n"));
        assert!(got.contains("a=fmtp:97 apt=96\r\n"));
        assert!(got.contains("a=inactive\r\n"));
    }

    #[test]
    fn restored_codec_lines_land_directly_after_rtcp_mux() {
        let offer = "v=0\r\n\
            m=video 0 UDP/TLS/RTP/SAVPF 96\r\n\
            a=mid:4\r\n\
            a=rtcp-mux\r\n\
            a=inactive\r\n";
        let got = stabilize_inactive_video_sections(offer, Some(PREV));
        let mux = got.find("a=rtcp-mux").expect("rtcp-mux kept");
        let rtpmap = got.find("a=rtpmap:96").expect("codec restored");
        let inactive = got.find("a=inactive").expect("direction kept");
        assert!(mux < rtpmap, "codecs must follow rtcp-mux");
        assert!(rtpmap < inactive, "codecs must precede the trailing attributes");
    }

    #[test]
    fn stale_codec_lines_in_the_offer_are_replaced_not_duplicated() {
        let offer = "v=0\r\n\
            m=video 0 UDP/TLS/RTP/SAVPF 100\r\n\
            a=mid:4\r\n\
            a=rtcp-mux\r\n\
            a=rtpmap:100 H264/90000\r\n\
            a=inactive\r\n";
        let got = stabilize_inactive_video_sections(offer, Some(PREV));
        assert!(!got.contains("H264"));
        assert_eq!(got.matches("a=rtpmap:96 VP8/90000").count(), 1);
    }

    #[test]
    fn local_uplink_mids_are_left_alone() {
        let offer = "v=0\r\n\
            m=video 0 UDP/TLS/RTP/SAVPF 96\r\n\
            a=mid:1\r\n\
            a=rtcp-mux\r\n\
            a=inactive\r\n";
        let prev = "v=0\r\n\
            m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
            a=mid:1\r\n\
            a=rtcp-mux\r\n\
            a=rtpmap:96 VP8/90000\r\n";
        assert_eq!(stabilize_inactive_video_sections(offer, Some(prev)), offer);
    }

    #[test]
    fn an_active_remote_section_is_left_alone() {
        let offer = "v=0\r\n\
            m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
            a=mid:4\r\n\
            a=rtcp-mux\r\n\
            a=recvonly\r\n";
        assert_eq!(stabilize_inactive_video_sections(offer, Some(PREV)), offer);
    }

    #[test]
    fn a_mid_absent_from_the_previous_answer_is_left_alone() {
        let offer = "v=0\r\n\
            m=video 0 UDP/TLS/RTP/SAVPF 96\r\n\
            a=mid:9\r\n\
            a=rtcp-mux\r\n\
            a=inactive\r\n";
        assert_eq!(stabilize_inactive_video_sections(offer, Some(PREV)), offer);
    }

    #[test]
    fn a_speaker_answer_is_never_patched() {
        let answer = "v=0\r\nm=video 9 RTP/SAVPF 96\r\na=mid:1\r\na=inactive\r\n";
        assert_eq!(patch_answer_for_sfu(answer, false), answer);
    }

    #[test]
    fn an_audience_camera_uplink_is_reopened_as_sendonly() {
        let answer = "v=0\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            a=inactive\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:1\r\n\
            a=inactive\r\n";
        let got = patch_answer_for_sfu(answer, true);
        assert!(got.contains("a=mid:1\r\na=sendonly\r\n"));
        assert!(got.contains("a=mid:0\r\na=inactive\r\n"), "audio uplink untouched");
    }

    #[test]
    fn only_the_camera_uplink_is_reopened_not_remote_video() {
        let answer = "v=0\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:1\r\n\
            a=inactive\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:4\r\n\
            a=inactive\r\n";
        let got = patch_answer_for_sfu(answer, true);
        assert!(got.contains("a=mid:1\r\na=sendonly\r\n"));
        assert!(got.contains("a=mid:4\r\na=inactive\r\n"));
    }

    #[test]
    fn an_audience_answer_without_an_inactive_camera_is_unchanged() {
        let answer = "v=0\r\nm=video 9 RTP/SAVPF 96\r\na=mid:1\r\na=sendonly\r\n";
        assert_eq!(patch_answer_for_sfu(answer, true), answer);
    }

    #[test]
    fn bare_newline_input_is_normalised_to_crlf_when_patched() {
        let answer = "v=0\nm=video 9 RTP/SAVPF 96\na=mid:1\na=inactive\n";
        let got = patch_answer_for_sfu(answer, true);
        assert!(got.contains("a=sendonly\r\n"));
        assert!(!got.contains("a=inactive"));
    }

    #[test]
    fn the_direction_summary_lists_every_section_in_order() {
        let sdp = "v=0\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            a=sendonly\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:1\r\n\
            a=sendonly\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:4\r\n\
            a=recvonly\r\n";
        assert_eq!(direction_summary(sdp), "0a=sendonly 1v=sendonly 4v=recvonly");
    }

    #[test]
    fn a_section_without_a_direction_attribute_shows_a_question_mark() {
        let sdp = "v=0\r\nm=audio 9 RTP/SAVPF 111\r\na=mid:0\r\n";
        assert_eq!(direction_summary(sdp), "0a=?");
    }

    #[test]
    fn an_inactive_uplink_is_visible_in_the_summary() {
        let sdp = "v=0\r\nm=video 9 RTP/SAVPF 96\r\na=mid:1\r\na=inactive\r\n";
        assert_eq!(direction_summary(sdp), "1v=inactive");
    }

    const SPEAKER_OFFER: &str = "v=0\r\n\
        m=audio 9 RTP/SAVPF 111\r\n\
        a=mid:0\r\n\
        a=recvonly\r\n\
        m=video 9 RTP/SAVPF 96\r\n\
        a=mid:1\r\n\
        a=recvonly\r\n\
        m=video 9 RTP/SAVPF 98\r\n\
        a=mid:2\r\n\
        a=recvonly\r\n";

    const AUDIENCE_OFFER: &str = "v=0\r\n\
        m=audio 9 RTP/SAVPF 111\r\n\
        a=mid:0\r\n\
        a=recvonly\r\n\
        m=video 9 RTP/SAVPF 96\r\n\
        a=mid:1\r\n\
        a=inactive\r\n\
        m=video 9 RTP/SAVPF 98\r\n\
        a=mid:2\r\n\
        a=inactive\r\n";

    const DERIVED_ANSWER: &str = "v=0\r\n\
        m=audio 9 RTP/SAVPF 111\r\n\
        a=mid:0\r\n\
        a=inactive\r\n\
        m=video 9 RTP/SAVPF 96\r\n\
        a=mid:1\r\n\
        a=inactive\r\n\
        m=video 9 RTP/SAVPF 98\r\n\
        a=mid:2\r\n\
        a=inactive\r\n";

    #[test]
    fn a_speaker_opens_all_three_uplinks() {
        assert_eq!(
            direction_summary(&force_uplink_sendonly(DERIVED_ANSWER, SPEAKER_OFFER)),
            "0a=sendonly 1v=sendonly 2v=sendonly"
        );
    }

    #[test]
    fn an_audience_opens_only_the_audio_uplink() {
        assert_eq!(
            direction_summary(&force_uplink_sendonly(DERIVED_ANSWER, AUDIENCE_OFFER)),
            "0a=sendonly 1v=inactive 2v=inactive",
            "answering sendonly to an inactive offer is invalid per RFC 3264"
        );
    }

    #[test]
    fn remote_sections_are_never_touched() {
        let offer = "v=0\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            a=recvonly\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:3\r\n\
            a=sendonly\r\n";
        let answer = "v=0\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            a=inactive\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:3\r\n\
            a=recvonly\r\n";
        assert_eq!(
            direction_summary(&force_uplink_sendonly(answer, offer)),
            "0a=sendonly 3a=recvonly"
        );
    }

    #[test]
    fn an_uplink_with_no_direction_attribute_gains_one() {
        let answer = "v=0\r\nm=audio 9 RTP/SAVPF 111\r\na=mid:0\r\na=rtcp-mux\r\n";
        let got = force_uplink_sendonly(answer, SPEAKER_OFFER);
        assert!(got.contains("a=rtcp-mux\r\na=sendonly\r\n"));
    }

    #[test]
    fn an_answer_already_correct_is_returned_untouched() {
        let answer = "v=0\r\nm=audio 9 RTP/SAVPF 111\r\na=mid:0\r\na=sendonly\r\n";
        assert_eq!(force_uplink_sendonly(answer, SPEAKER_OFFER), answer);
    }

    #[test]
    fn a_mid_missing_from_the_offer_is_left_alone() {
        let answer = "v=0\r\nm=video 9 RTP/SAVPF 96\r\na=mid:2\r\na=inactive\r\n";
        let offer = "v=0\r\nm=audio 9 RTP/SAVPF 111\r\na=mid:0\r\na=recvonly\r\n";
        assert_eq!(force_uplink_sendonly(answer, offer), answer);
    }

    #[test]
    fn the_direction_summary_lists_every_section_in_order() {
        let sdp = "v=0\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            a=sendonly\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:1\r\n\
            a=sendonly\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:4\r\n\
            a=recvonly\r\n";
        assert_eq!(direction_summary(sdp), "0a=sendonly 1v=sendonly 4v=recvonly");
    }

    #[test]
    fn a_section_without_a_direction_attribute_shows_a_question_mark() {
        let sdp = "v=0\r\nm=audio 9 RTP/SAVPF 111\r\na=mid:0\r\n";
        assert_eq!(direction_summary(sdp), "0a=?");
    }

    #[test]
    fn an_inactive_uplink_is_visible_in_the_summary() {
        let sdp = "v=0\r\nm=video 9 RTP/SAVPF 96\r\na=mid:1\r\na=inactive\r\n";
        assert_eq!(direction_summary(sdp), "1v=inactive");
    }

    #[test]
    fn inactive_uplinks_are_forced_to_sendonly() {
        let answer = "v=0\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            a=inactive\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:1\r\n\
            a=inactive\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:2\r\n\
            a=inactive\r\n";
        assert_eq!(
            direction_summary(&force_uplink_sendonly(answer)),
            "0a=sendonly 1v=sendonly 2v=sendonly"
        );
    }

    #[test]
    fn remote_sections_keep_their_direction() {
        let answer = "v=0\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            a=inactive\r\n\
            m=audio 9 RTP/SAVPF 111\r\n\
            a=mid:3\r\n\
            a=recvonly\r\n";
        assert_eq!(
            direction_summary(&force_uplink_sendonly(answer)),
            "0a=sendonly 3a=recvonly"
        );
    }

    #[test]
    fn an_uplink_with_no_direction_attribute_gains_one() {
        let answer = "v=0\r\nm=audio 9 RTP/SAVPF 111\r\na=mid:0\r\na=rtcp-mux\r\n";
        let got = force_uplink_sendonly(answer);
        assert!(got.contains("a=rtcp-mux\r\na=sendonly\r\n"));
    }

    #[test]
    fn an_answer_already_sendonly_is_returned_untouched() {
        let answer = "v=0\r\nm=audio 9 RTP/SAVPF 111\r\na=mid:0\r\na=sendonly\r\n";
        assert_eq!(force_uplink_sendonly(answer), answer);
    }

    #[test]
    fn recvonly_uplinks_are_flipped_too() {
        let answer = "v=0\r\nm=video 9 RTP/SAVPF 96\r\na=mid:2\r\na=recvonly\r\n";
        assert_eq!(direction_summary(&force_uplink_sendonly(answer)), "2v=sendonly");
    }

    #[test]
    fn the_codec_summary_lists_payload_types_per_mid() {
        let sdp = "v=0\r\n\
            m=video 9 RTP/SAVPF 96 97\r\n\
            a=mid:1\r\n\
            a=rtpmap:96 VP8/90000\r\n\
            a=rtpmap:97 rtx/90000\r\n\
            m=video 9 RTP/SAVPF 98 99\r\n\
            a=mid:5\r\n\
            a=rtpmap:98 VP9/90000\r\n\
            a=rtpmap:99 rtx/90000\r\n";
        assert_eq!(codec_summary(sdp), "1:[96/VP8,97/rtx] 5:[98/VP9,99/rtx]");
    }

    #[test]
    fn a_section_with_no_rtpmap_shows_an_empty_list() {
        let sdp = "v=0\r\nm=video 0 RTP/SAVPF 96\r\na=mid:5\r\na=inactive\r\n";
        assert_eq!(codec_summary(sdp), "5:[]");
    }

    const CAM: BitrateLimits = BitrateLimits {
        min_kbps: 250,
        start_kbps: 500,
        max_kbps: 1000,
    };
    const SCR: BitrateLimits = BitrateLimits {
        min_kbps: 400,
        start_kbps: 1000,
        max_kbps: 2500,
    };

    #[test]
    fn an_fmtp_line_is_created_when_the_codec_has_none() {
        let sdp = "v=0\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:2\r\n\
            a=rtpmap:96 VP8/90000\r\n";
        let got = munge_uplink_bitrates(sdp, CAM, SCR);
        assert!(got.contains(
            "a=rtpmap:96 VP8/90000\r\n\
             a=fmtp:96 x-google-min-bitrate=400;x-google-start-bitrate=1000;x-google-max-bitrate=2500\r\n"
        ));
    }

    #[test]
    fn each_uplink_gets_its_own_limits() {
        let sdp = "v=0\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:1\r\n\
            a=rtpmap:96 VP8/90000\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:2\r\n\
            a=rtpmap:96 VP8/90000\r\n";
        let got = munge_uplink_bitrates(sdp, CAM, SCR);
        assert!(got.contains("x-google-start-bitrate=500"), "camera start");
        assert!(got.contains("x-google-start-bitrate=1000"), "screen start");
    }

    #[test]
    fn existing_fmtp_parameters_are_kept() {
        let sdp = "v=0\r\n\
            m=video 9 RTP/SAVPF 98\r\n\
            a=mid:2\r\n\
            a=rtpmap:98 VP9/90000\r\n\
            a=fmtp:98 profile-id=0\r\n";
        let got = munge_uplink_bitrates(sdp, CAM, SCR);
        assert!(got.contains("a=fmtp:98 profile-id=0;x-google-min-bitrate=400"));
    }

    #[test]
    fn stale_hints_are_replaced_rather_than_appended() {
        let sdp = "v=0\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:2\r\n\
            a=rtpmap:96 VP8/90000\r\n\
            a=fmtp:96 x-google-start-bitrate=100;profile-id=0\r\n";
        let got = munge_uplink_bitrates(sdp, CAM, SCR);
        assert_eq!(got.matches("x-google-start-bitrate").count(), 1);
        assert!(got.contains("x-google-start-bitrate=1000"));
        assert!(got.contains("profile-id=0"));
    }

    #[test]
    fn retransmission_payloads_are_skipped() {
        let sdp = "v=0\r\n\
            m=video 9 RTP/SAVPF 96 97\r\n\
            a=mid:2\r\n\
            a=rtpmap:96 VP8/90000\r\n\
            a=rtpmap:97 rtx/90000\r\n\
            a=fmtp:97 apt=96\r\n";
        let got = munge_uplink_bitrates(sdp, CAM, SCR);
        assert!(got.contains("a=fmtp:97 apt=96\r\n"), "rtx fmtp must be untouched");
        assert_eq!(got.matches("x-google-min-bitrate").count(), 1);
    }

    #[test]
    fn remote_sections_are_left_alone() {
        let sdp = "v=0\r\n\
            m=video 9 RTP/SAVPF 96\r\n\
            a=mid:5\r\n\
            a=rtpmap:96 VP8/90000\r\n";
        assert_eq!(munge_uplink_bitrates(sdp, CAM, SCR), sdp);
    }
}
