use gpui::{AnyElement, Hsla, IntoElement, ParentElement, Pixels, Styled, div, px};
use mezon_store::{ChannelType, is_age_restricted};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

pub(crate) fn shows_left_unread_nub(channel_type: ChannelType) -> bool {
    !matches!(
        channel_type,
        ChannelType::Voice | ChannelType::Stream | ChannelType::App | ChannelType::Unknown(_)
    )
}

pub(crate) fn channel_type_icon(
    channel_type: ChannelType,
    private: bool,
    age_restricted: i32,
) -> IconName {
    match channel_type {
        ChannelType::Text if is_age_restricted(age_restricted) => IconName::HashtagWarning,
        ChannelType::Text if private => IconName::HashtagLocked,
        ChannelType::Text => IconName::Hashtag,
        ChannelType::Voice if private => IconName::SpeakerLocked,
        ChannelType::Voice => IconName::Speaker,
        ChannelType::Stream => IconName::Stream,
        ChannelType::Thread if private => IconName::ThreadIconLocker,
        ChannelType::Thread => IconName::ThreadIcon,
        ChannelType::Forum => IconName::Forum,
        ChannelType::Announcement => IconName::Announcement,
        ChannelType::App if private => IconName::PrivateAppChannelIcon,
        ChannelType::App => IconName::AppChannelIcon,
        ChannelType::Unknown(_) => IconName::Hashtag,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChannelIcon {
    pub base: IconName,
    pub lock: Option<IconName>,
}

impl ChannelIcon {
    fn combined_glyph(self) -> IconName {
        match self.lock {
            Some(IconName::HashtagLock) => IconName::HashtagLocked,
            Some(IconName::ThreadLock) => IconName::ThreadIconLocker,
            _ => self.base,
        }
    }
}

pub(crate) fn channel_icon(
    channel_type: ChannelType,
    private: bool,
    age_restricted: i32,
) -> ChannelIcon {
    match (channel_type, private, is_age_restricted(age_restricted)) {
        (ChannelType::Text, _, true) => ChannelIcon {
            base: IconName::HashtagWarning,
            lock: None,
        },
        (ChannelType::Thread, true, _) => ChannelIcon {
            base: IconName::ThreadIcon,
            lock: Some(IconName::ThreadLock),
        },
        (ChannelType::Text, true, false) => ChannelIcon {
            base: IconName::Hashtag,
            lock: Some(IconName::HashtagLock),
        },
        _ => ChannelIcon {
            base: channel_type_icon(channel_type, private, age_restricted),
            lock: None,
        },
    }
}

pub(crate) fn render_channel_icon(
    icon: ChannelIcon,
    size: Pixels,
    color: Hsla,
    _lock_color: Hsla,
) -> AnyElement {
    Icon::new(icon.combined_glyph())
        .size(size)
        .text_color(color)
        .into_any_element()
}

pub(crate) fn voice_busy_tag(theme: &Theme) -> AnyElement {
    div()
        .flex_shrink_0()
        .text_size(px(15.))
        .text_color(theme.danger_text)
        .child("(busy)")
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_private_glyph_has_no_extra_lock_layer() {
        assert_eq!(
            channel_icon(ChannelType::Text, true, 0).combined_glyph(),
            IconName::HashtagLocked
        );
        assert_eq!(
            channel_icon(ChannelType::Thread, true, 0).combined_glyph(),
            IconName::ThreadIconLocker
        );
        assert_eq!(
            channel_icon(ChannelType::Text, false, 0).combined_glyph(),
            IconName::Hashtag
        );
        assert_eq!(
            channel_icon(ChannelType::Text, true, 1).combined_glyph(),
            IconName::HashtagWarning
        );
    }

    fn icon_path(channel_type: ChannelType, private: bool, age_restricted: i32) -> &'static str {
        channel_type_icon(channel_type, private, age_restricted).path()
    }

    #[test]
    fn private_channels_get_the_locked_variant() {
        assert_eq!(
            icon_path(ChannelType::Thread, true, 0),
            IconName::ThreadIconLocker.path()
        );
        assert_eq!(
            icon_path(ChannelType::Text, true, 0),
            IconName::HashtagLocked.path()
        );
        assert_eq!(
            icon_path(ChannelType::Voice, true, 0),
            IconName::SpeakerLocked.path()
        );
        assert_eq!(
            icon_path(ChannelType::App, true, 0),
            IconName::PrivateAppChannelIcon.path()
        );
    }

    #[test]
    fn public_channels_get_the_plain_variant() {
        assert_eq!(
            icon_path(ChannelType::Thread, false, 0),
            IconName::ThreadIcon.path()
        );
        assert_eq!(
            icon_path(ChannelType::Text, false, 0),
            IconName::Hashtag.path()
        );
        assert_eq!(
            icon_path(ChannelType::Voice, false, 0),
            IconName::Speaker.path()
        );
    }

    #[test]
    fn age_restricted_text_channels_use_the_warning_glyph() {
        assert_eq!(
            icon_path(ChannelType::Text, false, 1),
            IconName::HashtagWarning.path()
        );
        assert_eq!(
            icon_path(ChannelType::Text, true, 1),
            IconName::HashtagWarning.path()
        );
        let icon = channel_icon(ChannelType::Text, true, 1);
        assert_eq!(icon.base.path(), IconName::HashtagWarning.path());
        assert!(icon.lock.is_none());
    }

    #[test]
    fn private_threads_and_channels_stack_a_separate_lock() {
        let thread = channel_icon(ChannelType::Thread, true, 0);
        assert_eq!(thread.base.path(), IconName::ThreadIcon.path());
        assert_eq!(
            thread.lock.map(IconName::path),
            Some(IconName::ThreadLock.path())
        );

        let text = channel_icon(ChannelType::Text, true, 0);
        assert_eq!(text.base.path(), IconName::Hashtag.path());
        assert_eq!(
            text.lock.map(IconName::path),
            Some(IconName::HashtagLock.path())
        );
    }

    #[test]
    fn public_channels_need_no_lock_overlay() {
        for channel_type in [ChannelType::Thread, ChannelType::Text, ChannelType::Voice] {
            let icon = channel_icon(channel_type, false, 0);
            assert!(icon.lock.is_none());
            assert_eq!(
                icon.base.path(),
                channel_type_icon(channel_type, false, 0).path()
            );
        }
    }

    #[test]
    fn types_with_a_combined_locked_glyph_keep_it() {
        let voice = channel_icon(ChannelType::Voice, true, 0);
        assert_eq!(voice.base.path(), IconName::SpeakerLocked.path());
        assert!(voice.lock.is_none());

        let app = channel_icon(ChannelType::App, true, 0);
        assert_eq!(app.base.path(), IconName::PrivateAppChannelIcon.path());
        assert!(app.lock.is_none());
    }

    #[test]
    fn streams_ignore_the_private_flag() {
        assert_eq!(
            icon_path(ChannelType::Stream, true, 0),
            IconName::Stream.path()
        );
        assert_eq!(
            icon_path(ChannelType::Stream, false, 0),
            IconName::Stream.path()
        );
    }
}
