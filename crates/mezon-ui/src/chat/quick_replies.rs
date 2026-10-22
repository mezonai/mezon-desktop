use mezon_store::message::{Message, MessageCode};
use mezon_store::{ChannelId, MessagesStore};

pub fn message_bucket(store: &MessagesStore, for_topic: bool) -> Option<ChannelId> {
    if for_topic {
        store.active_topic_id()
    } else {
        store.active_channel_id()
    }
}

pub fn last_text_message_for_quick_replies<'a>(
    store: &'a MessagesStore,
    bucket: ChannelId,
    viewer_id: Option<&str>,
) -> Option<&'a Message> {
    if !store.is_reading_live_tail(bucket) {
        return None;
    }
    last_unanswered_text_message(store.messages_in_channel(bucket), viewer_id)
}

fn last_unanswered_text_message<'a>(
    messages: &'a [Message],
    viewer_id: Option<&str>,
) -> Option<&'a Message> {
    let message = messages
        .iter()
        .rev()
        .find(|message| quick_reply_source_text(message).is_some())?;
    if viewer_id.is_some_and(|id| message.is_sent_by(id)) {
        None
    } else {
        Some(message)
    }
}

pub fn quick_reply_source_text(message: &Message) -> Option<&str> {
    if message.is_deleted_placeholder {
        return None;
    }
    if !matches!(message.code, MessageCode::Chat | MessageCode::ChatUpdate) {
        return None;
    }
    let trimmed = message.content.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::{last_unanswered_text_message, quick_reply_source_text};
    use mezon_store::Message;
    use mezon_store::ids::{ChannelId, MessageId};
    use mezon_store::message::MessageCode;

    fn minimal_message(content: &str, code: MessageCode) -> Message {
        Message {
            id: MessageId(1),
            channel_id: ChannelId(1),
            sort_id: 0,
            row_anchor_id: MessageId(1),
            content: content.to_string(),
            sender_id: String::new(),
            sender_user_id: None,
            sender_name: gpui::SharedString::default(),
            avatar_url: gpui::SharedString::default(),
            avatar_proxied: gpui::SharedString::default(),
            anonymous_sender: false,
            create_time: 0,
            update_time: 0,
            day_label: String::new(),
            time_hhmm: gpui::SharedString::default(),
            local_date: None,
            code,
            is_edited: false,
            hide_editted: false,
            is_forwarded: false,
            show_forwarded_label: false,
            combined_with_prev: false,
            highlights_viewer_direct: false,
            send_failed: false,
            ogp: None,
            poll: None,
            call_log: None,
            embeds: std::sync::Arc::new([]),
            components: std::sync::Arc::new([]),
            is_card: false,
            topic_id: None,
            topic_creator_id: None,
            invite: None,
            is_only_emoji: false,
            is_deleted_placeholder: false,
            spans: Vec::new(),
            rich_layout: None,
            token_transaction: None,
            mention_targets: Vec::new(),
            references: Vec::new(),
            reactions: Vec::new(),
            attachments: Vec::new(),
            album_layout: None,
            viewer_media: std::sync::Arc::new([]),
            raw_content: None,
            command: None,
        }
    }

    #[test]
    fn skips_empty_and_non_chat() {
        assert!(quick_reply_source_text(&minimal_message("", MessageCode::Chat)).is_none());
        assert!(quick_reply_source_text(&minimal_message("hi", MessageCode::Typing)).is_none());
        assert_eq!(
            quick_reply_source_text(&minimal_message("hello", MessageCode::Chat)),
            Some("hello")
        );
    }

    #[test]
    fn skips_pills_when_newest_text_is_ours() {
        let mut theirs = minimal_message("cau hoi", MessageCode::Chat);
        theirs.sender_id = "other".into();
        let mut ours = minimal_message("da tra loi", MessageCode::Chat);
        ours.sender_id = "me".into();
        let messages = [theirs, ours];
        assert!(last_unanswered_text_message(&messages, Some("me")).is_none());
        assert_eq!(
            last_unanswered_text_message(&messages, Some("other"))
                .map(|message| message.content.as_str()),
            Some("da tra loi")
        );
        assert_eq!(
            last_unanswered_text_message(&messages[..1], Some("me"))
                .map(|message| message.content.as_str()),
            Some("cau hoi")
        );
    }
}
