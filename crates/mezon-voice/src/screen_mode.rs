use libwebrtc::rtp_parameters::DegradationPreference;
use libwebrtc::video_track::ContentHint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenShareMode {
    #[default]
    Text,
    Video,
}

impl ScreenShareMode {
    pub fn capture_fps(self) -> u32 {
        match self {
            Self::Text => 15,
            Self::Video => 30,
        }
    }

    pub fn encode_max_fps(self) -> f64 {
        match self {
            Self::Text => 20.0,
            Self::Video => 30.0,
        }
    }

    pub fn max_bitrate_bps(self) -> u64 {
        3_500_000
    }

    pub(crate) fn degradation(self) -> DegradationPreference {
        match self {
            Self::Text => DegradationPreference::MaintainResolution,
            Self::Video => DegradationPreference::MaintainFramerate,
        }
    }

    pub(crate) fn content_hint(self) -> ContentHint {
        match self {
            Self::Text => ContentHint::Text,
            Self::Video => ContentHint::Fluid,
        }
    }

    pub(crate) fn fallback_content_hint(self) -> ContentHint {
        match self {
            Self::Text => ContentHint::Detailed,
            Self::Video => ContentHint::Fluid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_mode_trades_motion_for_sharpness() {
        let mode = ScreenShareMode::Text;
        assert_eq!(mode.capture_fps(), 15);
        assert_eq!(mode.encode_max_fps(), 20.0);
        assert_eq!(
            mode.degradation(),
            DegradationPreference::MaintainResolution
        );
        assert_eq!(mode.content_hint(), ContentHint::Text);
        assert_eq!(mode.fallback_content_hint(), ContentHint::Detailed);
    }

    #[test]
    fn video_mode_trades_sharpness_for_motion() {
        let mode = ScreenShareMode::Video;
        assert_eq!(mode.capture_fps(), 30);
        assert_eq!(mode.encode_max_fps(), 30.0);
        assert_eq!(mode.degradation(), DegradationPreference::MaintainFramerate);
        assert_eq!(mode.content_hint(), ContentHint::Fluid);
    }

    #[test]
    fn encoder_cap_never_starves_the_capture() {
        for mode in [ScreenShareMode::Text, ScreenShareMode::Video] {
            assert!(mode.encode_max_fps() >= f64::from(mode.capture_fps()));
        }
    }

    #[test]
    fn both_modes_publish_at_the_sfu_screen_cap() {
        for mode in [ScreenShareMode::Text, ScreenShareMode::Video] {
            assert_eq!(mode.max_bitrate_bps(), 3_500_000);
        }
    }

    #[test]
    fn text_is_the_default() {
        assert_eq!(ScreenShareMode::default(), ScreenShareMode::Text);
    }
}
