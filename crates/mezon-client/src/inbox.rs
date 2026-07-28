use anyhow::{Context, Result};
use mezon_proto::api;
use prost::Message as _;

pub const INBOX_PAGE_LIMIT: i32 = 50;
pub const DIRECTION_BEFORE_TIMESTAMP: i32 = 3;
pub const DIRECTION_AROUND_TIMESTAMP: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum InboxCategory {
    Mentions = 1,
    Messages = 2,
    ForYou = 3,
}

impl InboxCategory {
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Mentions),
            2 => Some(Self::Messages),
            3 => Some(Self::ForYou),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxMentionSpan {
    pub start: i32,
    pub end: i32,
    pub user_id: String,
    pub role_id: String,
    pub is_role: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxMessagePreview {
    pub message_id: String,
    pub channel_id: String,
    pub clan_id: String,
    pub sender_id: String,
    pub content: String,
    pub raw_content: String,
    pub avatar: String,
    pub display_name: String,
    pub username: String,
    pub create_time_seconds: u32,
    pub attachment_link: String,
    pub attachment_type: String,
    pub has_more_attachment: bool,
    pub mention_spans: Vec<InboxMentionSpan>,
}

impl InboxMessagePreview {
    pub fn body_text(&self) -> String {
        if !self.content.is_empty() {
            self.content.clone()
        } else {
            display_text_from_message_content(&self.raw_content)
        }
    }

    pub fn mention_spans_for_render(&self) -> Vec<InboxMentionSpan> {
        if !self.mention_spans.is_empty() {
            return self.mention_spans.clone();
        }
        mention_spans_from_json_content(&self.raw_content)
    }

    fn empty_content(content: String) -> Self {
        Self {
            message_id: String::new(),
            channel_id: String::new(),
            clan_id: String::new(),
            sender_id: String::new(),
            content: display_text_from_message_content(&content),
            raw_content: content,
            avatar: String::new(),
            display_name: String::new(),
            username: String::new(),
            create_time_seconds: 0,
            attachment_link: String::new(),
            attachment_type: String::new(),
            has_more_attachment: false,
            mention_spans: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxNotification {
    pub id: String,
    pub category: InboxCategory,
    pub subject: String,
    pub sender_id: String,
    pub clan_id: String,
    pub channel_id: String,
    pub topic_id: Option<String>,
    pub channel_type: i32,
    pub avatar_url: String,
    pub create_time_seconds: u32,
    pub code: i32,
    pub message: Option<InboxMessagePreview>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicDiscussion {
    pub id: String,
    pub message_id: String,
    pub clan_id: String,
    pub channel_id: String,
    pub creator_id: String,
    pub last_sender_id: String,
    pub content: String,
    pub last_message_timestamp: u32,
}

fn id_str(value: i64) -> String {
    value.to_string()
}

fn optional_id_str(value: i64) -> Option<String> {
    if value == 0 {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn display_text_from_message_content(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(text) = value.get("t").and_then(|v| v.as_str())
    {
        return text.to_string();
    }
    trimmed.to_string()
}

pub fn attachment_link_is_image(link: &str, filetype: &str) -> bool {
    if filetype.starts_with("image/") {
        return true;
    }
    link.rsplit('.').next().is_some_and(|ext| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif"
        )
    })
}

fn mention_spans_from_fcm(fcm: &api::DirectFcmProto) -> Vec<InboxMentionSpan> {
    fcm.mention_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let is_role = fcm.is_mention_role.get(index).copied().unwrap_or(false);
            InboxMentionSpan {
                start: fcm.position_s.get(index).copied().unwrap_or(0),
                end: fcm.position_e.get(index).copied().unwrap_or(0),
                user_id: if is_role {
                    String::new()
                } else {
                    id.to_string()
                },
                role_id: if is_role {
                    id.to_string()
                } else {
                    String::new()
                },
                is_role,
            }
        })
        .collect()
}

fn mention_spans_from_json_content(content: &str) -> Vec<InboxMentionSpan> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Vec::new();
    };
    value
        .get("mentions")
        .and_then(|mentions| mentions.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let role_id = item
                        .get("role_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let user_id = item
                        .get("user_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(InboxMentionSpan {
                        start: item.get("s")?.as_i64()? as i32,
                        end: item.get("e")?.as_i64()? as i32,
                        user_id,
                        role_id: role_id.clone(),
                        is_role: !role_id.is_empty(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn preview_from_fcm(fcm: api::DirectFcmProto) -> InboxMessagePreview {
    let mention_spans = mention_spans_from_fcm(&fcm);
    InboxMessagePreview {
        message_id: id_str(fcm.message_id),
        channel_id: id_str(fcm.channel_id),
        clan_id: id_str(fcm.clan_id),
        sender_id: id_str(fcm.sender_id),
        content: display_text_from_message_content(&fcm.content),
        raw_content: fcm.content.clone(),
        avatar: fcm.avatar,
        display_name: fcm.display_name,
        username: fcm.username,
        create_time_seconds: fcm.create_time_seconds.max(0) as u32,
        attachment_link: fcm.attachment_link,
        attachment_type: fcm.attachment_type,
        has_more_attachment: fcm.has_more_attachment,
        mention_spans,
    }
}
pub fn message_content_is_attachment(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content.trim())
        .ok()
        .is_some_and(|value| {
            value.get("embed").is_some()
                && value
                    .get("t")
                    .and_then(|t| t.as_str())
                    .is_none_or(str::is_empty)
        })
}

const SHARE_CONTACT_KEY: &str = "share_contact";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicReplyPreview {
    Text(String),
    Contact,
    Attachment,
    Interactive,
}

fn embed_value_is_share_contact(embed: &serde_json::Value) -> bool {
    embed
        .get("fields")
        .and_then(|fields| fields.as_array())
        .is_some_and(|fields| {
            fields.iter().any(|field| {
                field.get("name").and_then(|n| n.as_str()) == Some("key")
                    && field.get("value").and_then(|v| v.as_str()) == Some(SHARE_CONTACT_KEY)
            })
        })
}

pub fn topic_reply_preview(content: &str) -> TopicReplyPreview {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return TopicReplyPreview::Attachment;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return TopicReplyPreview::Text(trimmed.to_string());
    };
    if !value.is_object() {
        return TopicReplyPreview::Text(trimmed.to_string());
    }
    let embeds = value
        .get("embed")
        .and_then(|embed| embed.as_array())
        .map(|items| items.as_slice())
        .unwrap_or(&[]);
    if embeds.first().is_some_and(embed_value_is_share_contact) {
        return TopicReplyPreview::Contact;
    }
    let has_attachments = value
        .get("attachments")
        .and_then(|items| items.as_array())
        .is_some_and(|items| !items.is_empty());
    let has_components = value
        .get("components")
        .and_then(|items| items.as_array())
        .is_some_and(|items| !items.is_empty());
    if has_attachments || has_components {
        return TopicReplyPreview::Attachment;
    }
    if !embeds.is_empty() {
        return TopicReplyPreview::Interactive;
    }
    let text = value
        .get("t")
        .and_then(|text| text.as_str())
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return TopicReplyPreview::Attachment;
    }
    TopicReplyPreview::Text(text.to_string())
}

fn parse_notification_content(bytes: &[u8]) -> Option<InboxMessagePreview> {
    if bytes.is_empty() {
        return None;
    }
    let text = std::str::from_utf8(bytes).unwrap_or("");
    let first = bytes[0];
    if first == b'[' || first == b'{' {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
            if value.get("message_id").is_some() || value.get("sender_id").is_some() {
                return parse_message_preview_json(bytes);
            }
            let content = display_text_from_message_content(text);
            if !content.is_empty() || message_content_is_attachment(text) {
                let mut preview = InboxMessagePreview::empty_content(text.to_string());
                preview.mention_spans = mention_spans_from_json_content(text);
                return Some(preview);
            }
        }
        return parse_message_preview_json(bytes);
    }
    if let Ok(fcm) = api::DirectFcmProto::decode(bytes) {
        return Some(preview_from_fcm(fcm));
    }
    parse_message_preview_json(bytes)
}

fn parse_message_preview_json(bytes: &[u8]) -> Option<InboxMessagePreview> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(default)]
        message_id: serde_json::Value,
        #[serde(default)]
        channel_id: serde_json::Value,
        #[serde(default)]
        clan_id: serde_json::Value,
        #[serde(default)]
        sender_id: serde_json::Value,
        #[serde(default)]
        content: String,
        #[serde(default)]
        avatar: String,
        #[serde(default)]
        display_name: String,
        #[serde(default)]
        username: String,
        #[serde(default)]
        create_time_seconds: u32,
        #[serde(default)]
        attachment_link: String,
        #[serde(default)]
        attachment_type: String,
        #[serde(default)]
        has_more_attachment: bool,
    }

    fn json_id(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        }
    }

    let raw: Raw = serde_json::from_slice(bytes).ok()?;
    let mut preview = InboxMessagePreview {
        message_id: json_id(&raw.message_id),
        channel_id: json_id(&raw.channel_id),
        clan_id: json_id(&raw.clan_id),
        sender_id: json_id(&raw.sender_id),
        content: display_text_from_message_content(&raw.content),
        raw_content: raw.content.clone(),
        avatar: raw.avatar,
        display_name: raw.display_name,
        username: raw.username,
        create_time_seconds: raw.create_time_seconds,
        attachment_link: raw.attachment_link,
        attachment_type: raw.attachment_type,
        has_more_attachment: raw.has_more_attachment,
        mention_spans: mention_spans_from_json_content(&raw.content),
    };
    if preview.content.is_empty() && !preview.raw_content.is_empty() {
        preview.content = display_text_from_message_content(&preview.raw_content);
    }
    Some(preview)
}

impl InboxNotification {
    pub fn effective_clan_id(&self) -> Option<String> {
        if !self.clan_id.is_empty() && self.clan_id != "0" {
            return Some(self.clan_id.clone());
        }
        self.message.as_ref().and_then(|m| {
            if !m.clan_id.is_empty() && m.clan_id != "0" {
                Some(m.clan_id.clone())
            } else {
                None
            }
        })
    }

    pub fn effective_channel_id(&self) -> Option<String> {
        if !self.channel_id.is_empty() && self.channel_id != "0" {
            return Some(self.channel_id.clone());
        }
        self.message.as_ref().and_then(|m| {
            if !m.channel_id.is_empty() && m.channel_id != "0" {
                Some(m.channel_id.clone())
            } else {
                None
            }
        })
    }

    pub fn preview_text(&self) -> String {
        if let Some(message) = &self.message
            && !message.content.is_empty()
        {
            return message.content.clone();
        }
        if !self.subject.is_empty() {
            return self.subject.clone();
        }
        String::new()
    }

    pub fn message_timestamp(&self) -> u32 {
        self.message
            .as_ref()
            .map(|m| m.create_time_seconds)
            .filter(|ts| *ts > 0)
            .unwrap_or(self.create_time_seconds)
    }
}

impl TopicDiscussion {
    pub fn reply_preview(&self) -> TopicReplyPreview {
        topic_reply_preview(&self.content)
    }

    pub fn reply_preview_text(&self) -> String {
        match self.reply_preview() {
            TopicReplyPreview::Text(text) => text,
            _ => String::new(),
        }
    }

    pub fn reply_is_attachment(&self) -> bool {
        matches!(self.reply_preview(), TopicReplyPreview::Attachment)
    }
}

pub fn inbox_notification_from_api(n: api::Notification) -> Result<InboxNotification> {
    let category = InboxCategory::from_i32(n.category)
        .with_context(|| format!("unknown notification category {}", n.category))?;
    let mut message = parse_notification_content(&n.content);
    if message.is_none() && !n.content.is_empty() {
        let raw = std::str::from_utf8(&n.content).unwrap_or("");
        let content = display_text_from_message_content(raw);
        if !content.is_empty() || message_content_is_attachment(raw) {
            message = Some({
                let mut preview = InboxMessagePreview::empty_content(raw.to_string());
                preview.channel_id = id_str(n.channel_id);
                preview.clan_id = id_str(n.clan_id);
                preview.sender_id = id_str(n.sender_id);
                preview.avatar = n.avatar_url.clone();
                preview
            });
        }
    }
    if let Some(message) = message.as_mut() {
        if message.channel_id.is_empty() {
            message.channel_id = id_str(n.channel_id);
        }
        if message.clan_id.is_empty() {
            message.clan_id = id_str(n.clan_id);
        }
        if message.sender_id.is_empty() {
            message.sender_id = id_str(n.sender_id);
        }
        if message.avatar.is_empty() {
            message.avatar = n.avatar_url.clone();
        }
        if message.content.is_empty() {
            let raw = std::str::from_utf8(&n.content).unwrap_or("");
            message.content = display_text_from_message_content(raw);
            if message.raw_content.is_empty() {
                message.raw_content = raw.to_string();
            }
            if message.mention_spans.is_empty() {
                message.mention_spans = mention_spans_from_json_content(raw);
            }
        }
        if message.create_time_seconds == 0 && n.create_time_seconds > 0 {
            message.create_time_seconds = n.create_time_seconds;
        }
    }
    Ok(InboxNotification {
        id: id_str(n.id),
        category,
        subject: n.subject,
        sender_id: id_str(n.sender_id),
        clan_id: id_str(n.clan_id),
        channel_id: id_str(n.channel_id),
        topic_id: optional_id_str(n.topic_id),
        channel_type: n.channel_type,
        avatar_url: n.avatar_url,
        create_time_seconds: n.create_time_seconds,
        code: n.code,
        message,
    })
}

pub fn inbox_notifications_from_list(
    list: api::NotificationList,
) -> Result<Vec<InboxNotification>> {
    Ok(list
        .notifications
        .into_iter()
        .filter_map(|n| inbox_notification_from_api(n).ok())
        .collect())
}

pub fn topic_discussion_from_api(t: api::SdTopic) -> TopicDiscussion {
    let last_message_timestamp = t
        .last_sent_message
        .as_ref()
        .map(|m| m.timestamp_seconds)
        .unwrap_or(t.update_time_seconds);
    let last_sender_id = t
        .last_sent_message
        .as_ref()
        .map(|m| m.sender_id)
        .filter(|id| *id != 0)
        .unwrap_or(t.creator_id);
    TopicDiscussion {
        id: id_str(t.id),
        message_id: id_str(t.message_id),
        clan_id: id_str(t.clan_id),
        channel_id: id_str(t.channel_id),
        creator_id: id_str(t.creator_id),
        last_sender_id: id_str(last_sender_id),
        content: t.content.clone(),
        last_message_timestamp,
    }
}

pub fn topics_from_list(list: api::SdTopicList) -> Vec<TopicDiscussion> {
    list.topics
        .into_iter()
        .map(topic_discussion_from_api)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_from_i32() {
        assert_eq!(InboxCategory::from_i32(1), Some(InboxCategory::Mentions));
        assert_eq!(InboxCategory::from_i32(2), Some(InboxCategory::Messages));
        assert_eq!(InboxCategory::from_i32(3), Some(InboxCategory::ForYou));
        assert_eq!(InboxCategory::from_i32(99), None);
    }

    #[test]
    fn display_text_extracts_t_field() {
        assert_eq!(
            display_text_from_message_content(r#"{"t":"@user","mk":[]}"#),
            "@user"
        );
    }

    #[test]
    fn parse_mention_content_json() {
        let bytes = br#"{"t":"@gia.chuvan","mentions":[{"s":0,"e":11,"user_id":"1"}]}"#;
        let preview = parse_notification_content(bytes).expect("preview");
        assert_eq!(preview.content, "@gia.chuvan");
    }

    #[test]
    fn parse_json_content() {
        let bytes = br#"{"message_id":"42","channel_id":"7","clan_id":"1","sender_id":"9","content":"hello","avatar":"a.png"}"#;
        let preview = parse_notification_content(bytes).expect("preview");
        assert_eq!(preview.message_id, "42");
        assert_eq!(preview.content, "hello");
    }

    #[test]
    fn parse_invalid_content_returns_none_for_empty() {
        assert!(parse_notification_content(&[]).is_none());
    }

    #[test]
    fn topic_discussion_maps_fields() {
        let topic = topic_discussion_from_api(api::SdTopic {
            id: 10,
            message_id: 20,
            clan_id: 1,
            channel_id: 5,
            content: "topic title".into(),
            update_time_seconds: 100,
            ..Default::default()
        });
        assert_eq!(topic.id, "10");
        assert_eq!(topic.message_id, "20");
        assert_eq!(topic.content, "topic title");
        assert_eq!(
            topic.reply_preview(),
            TopicReplyPreview::Text("topic title".into())
        );
        assert!(!topic.reply_is_attachment());
    }

    #[test]
    fn topic_reply_preview_classifies_content_kinds() {
        assert_eq!(
            topic_reply_preview(r#"{"t":"hello #mezon 😀"}"#),
            TopicReplyPreview::Text("hello #mezon 😀".into())
        );
        assert_eq!(
            topic_reply_preview(r#"{"t":""}"#),
            TopicReplyPreview::Attachment
        );
        assert_eq!(
            topic_reply_preview(r#"{"t":"","attachments":[{"url":"a.png"}]}"#),
            TopicReplyPreview::Attachment
        );
        assert_eq!(
            topic_reply_preview(r#"{"t":"x","components":[{"type":1}]}"#),
            TopicReplyPreview::Attachment
        );
        assert_eq!(
            topic_reply_preview(r#"{"t":"","embed":[{"title":"card"}]}"#),
            TopicReplyPreview::Interactive
        );
        assert_eq!(
            topic_reply_preview(
                r#"{"embed":[{"fields":[{"name":"key","value":"share_contact"}]}]}"#
            ),
            TopicReplyPreview::Contact
        );
        assert_eq!(topic_reply_preview(""), TopicReplyPreview::Attachment);
    }
}
