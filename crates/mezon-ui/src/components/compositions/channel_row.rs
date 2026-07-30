use gpui::{AnyElement, Hsla, IntoElement, ParentElement, Pixels, Styled, div};
use mezon_store::ChannelType;

use crate::components::primitives::{Icon, IconName};

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
        (ChannelType::Thread, false) => IconName::ThreadIcon,
        (ChannelType::Thread, true) => IconName::ThreadIconLocker,
        (ChannelType::Forum, _) => IconName::Forum,
        (ChannelType::Announcement, _) => IconName::Announcement,
        (ChannelType::App, false) => IconName::AppChannelIcon,
        (ChannelType::App, true) => IconName::PrivateAppChannelIcon,
        (ChannelType::Unknown(_), _) => IconName::Hashtag,
    }
}

/// React draws the padlock of a private channel in `--bg-icon-theme-active`, one
/// shade brighter than the glyph under it. GPUI tints a whole SVG a single
/// colour, so the padlock has to be a second element stacked on the glyph.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChannelIcon {
    pub base: IconName,
    pub lock: Option<IconName>,
}

pub(crate) fn channel_icon(channel_type: ChannelType, private: bool) -> ChannelIcon {
    match (channel_type, private) {
        (ChannelType::Thread, true) => ChannelIcon {
            base: IconName::ThreadIcon,
            lock: Some(IconName::ThreadLock),
        },
        (ChannelType::Text, true) => ChannelIcon {
            base: IconName::Hashtag,
            lock: Some(IconName::HashtagLock),
        },
        _ => ChannelIcon {
            base: channel_type_icon(channel_type, private),
            lock: None,
        },
    }
}

pub(crate) fn render_channel_icon(
    icon: ChannelIcon,
    size: Pixels,
    color: Hsla,
    lock_color: Hsla,
) -> AnyElement {
    let Some(lock) = icon.lock else {
        return Icon::new(icon.base)
            .size(size)
            .text_color(color)
            .into_any_element();
    };
    div()
        .relative()
        .size(size)
        .flex_shrink_0()
        .child(Icon::new(icon.base).size(size).text_color(color))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .child(Icon::new(lock).size(size).text_color(lock_color)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon_path(channel_type: ChannelType, private: bool) -> &'static str {
        channel_type_icon(channel_type, private).path()
    }

    #[test]
    fn private_channels_get_the_locked_variant() {
        assert_eq!(
            icon_path(ChannelType::Thread, true),
            IconName::ThreadIconLocker.path()
        );
        assert_eq!(
            icon_path(ChannelType::Text, true),
            IconName::HashtagLocked.path()
        );
        assert_eq!(
            icon_path(ChannelType::Voice, true),
            IconName::SpeakerLocked.path()
        );
        assert_eq!(
            icon_path(ChannelType::App, true),
            IconName::PrivateAppChannelIcon.path()
        );
    }

    #[test]
    fn public_channels_get_the_plain_variant() {
        assert_eq!(
            icon_path(ChannelType::Thread, false),
            IconName::ThreadIcon.path()
        );
        assert_eq!(
            icon_path(ChannelType::Text, false),
            IconName::Hashtag.path()
        );
        assert_eq!(
            icon_path(ChannelType::Voice, false),
            IconName::Speaker.path()
        );
    }

    #[test]
    fn private_threads_and_channels_stack_a_separate_lock() {
        let thread = channel_icon(ChannelType::Thread, true);
        assert_eq!(thread.base.path(), IconName::ThreadIcon.path());
        assert_eq!(
            thread.lock.map(IconName::path),
            Some(IconName::ThreadLock.path())
        );

        let text = channel_icon(ChannelType::Text, true);
        assert_eq!(text.base.path(), IconName::Hashtag.path());
        assert_eq!(
            text.lock.map(IconName::path),
            Some(IconName::HashtagLock.path())
        );
    }

    #[test]
    fn public_channels_need_no_lock_overlay() {
        for channel_type in [ChannelType::Thread, ChannelType::Text, ChannelType::Voice] {
            let icon = channel_icon(channel_type, false);
            assert!(icon.lock.is_none());
            assert_eq!(
                icon.base.path(),
                channel_type_icon(channel_type, false).path()
            );
        }
    }

    #[test]
    fn types_with_a_combined_locked_glyph_keep_it() {
        let voice = channel_icon(ChannelType::Voice, true);
        assert_eq!(voice.base.path(), IconName::SpeakerLocked.path());
        assert!(voice.lock.is_none());

        let app = channel_icon(ChannelType::App, true);
        assert_eq!(app.base.path(), IconName::PrivateAppChannelIcon.path());
        assert!(app.lock.is_none());
    }

    #[test]
    fn streams_ignore_the_private_flag() {
        assert_eq!(
            icon_path(ChannelType::Stream, true),
            IconName::Stream.path()
        );
        assert_eq!(
            icon_path(ChannelType::Stream, false),
            IconName::Stream.path()
        );
    }
}
