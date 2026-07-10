use mezon_store::ChannelType;

use crate::components::primitives::IconName;

pub(crate) fn shows_left_unread_nub(channel_type: ChannelType) -> bool {
    !matches!(
        channel_type,
        ChannelType::Voice | ChannelType::Stream | ChannelType::App | ChannelType::Unknown(_)
    )
}

pub(crate) fn channel_type_icon(channel_type: ChannelType, private: bool) -> IconName {
    match (channel_type, private) {
        (ChannelType::Text, false) => IconName::Hashtag,
        (ChannelType::Text, true) => IconName::HashtagLocked,
        (ChannelType::Voice, false) => IconName::Speaker,
        (ChannelType::Voice, true) => IconName::SpeakerLocked,
        (ChannelType::Stream, _) => IconName::Stream,
        (ChannelType::Thread, _) => IconName::ThreadIcon,
        (ChannelType::Forum, _) => IconName::Forum,
        (ChannelType::Announcement, _) => IconName::Announcement,
        (ChannelType::App, false) => IconName::AppChannelIcon,
        (ChannelType::App, true) => IconName::PrivateAppChannelIcon,
        (ChannelType::Unknown(_), _) => IconName::Hashtag,
    }
}
