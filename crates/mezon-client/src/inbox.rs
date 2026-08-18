use anyhow::{Context, Result};
use mezon_proto::api;
use prost::Message as _;

pub const INBOX_PAGE_LIMIT: i32 = 50;
pub const DIRECTION_BEFORE_TIMESTAMP: i32 = 3;
pub const DIRECTION_AROUND_TIMESTAMP: i32 = 2;
pub const INBOX_MESSAGE_MARK_CODE: i32 = -12;

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
    pub attachment_filename: String,
    pub attachment_size: u64,
    pub attachment_thumbnail: String,
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
            attachment_filename: String::new(),
            attachment_size: 0,
            attachment_thumbnail: String::new(),
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

pub fn is_valid_inbox_message_id(id: &str) -> bool {
    !id.is_empty() && id != "0" && id.parse::<i64>().is_ok_and(|value| value > 0)
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
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(text) = value.get("t").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if value.is_object() {
            return String::new();
        }
    }
    trimmed.to_string()
}

pub fn attachment_link_is_image(link: &str, filetype: &str) -> bool {
    if filetype.contains("svg+xml") {
        return false;
    }
    if filetype.starts_with("image/") || filetype == "sticker" {
        return true;
    }
    url_extension(link).is_some_and(|ext| {
        matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "avif"
        )
    })
}

pub fn attachment_link_is_video(link: &str, filetype: &str) -> bool {
    ((filetype.contains("video/mp4") || filetype.contains("video/quicktime"))
        && !link.contains("tenor.com"))
        || (filetype.starts_with("video") && !filetype.ends_with("vnd.dlna.mpeg-tts"))
        || url_extension(link).is_some_and(|ext| matches!(ext.as_str(), "mp4" | "mov" | "webm"))
}

fn url_extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().and_then(|name| {
        name.rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .filter(|ext| !ext.is_empty() && !ext.contains('/'))
    })
}

fn filename_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("Attachment")
        .to_string()
}

fn json_u64(value: &serde_json::Value) -> u64 {
    match value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_i64().and_then(|v| u64::try_from(v).ok()))
            .or_else(|| n.as_f64().and_then(|v| (v >= 0.0).then_some(v as u64)))
            .unwrap_or(0),
        serde_json::Value::String(raw) => raw.parse().unwrap_or(0),
        _ => 0,
    }
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
    let mut preview = InboxMessagePreview {
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
        attachment_filename: String::new(),
        attachment_size: 0,
        attachment_thumbnail: String::new(),
        has_more_attachment: fcm.has_more_attachment,
        mention_spans,
    };
    apply_preview_attachments(&mut preview);
    preview
}

fn preview_from_channel_message(message: api::ChannelMessage) -> InboxMessagePreview {
    let mut preview = InboxMessagePreview {
        message_id: id_str(message.message_id),
        channel_id: id_str(message.channel_id),
        clan_id: id_str(message.clan_id),
        sender_id: id_str(message.sender_id),
        content: display_text_from_message_content(&message.content),
        raw_content: message.content,
        avatar: message.avatar,
        display_name: message.display_name,
        username: message.username,
        create_time_seconds: message.create_time_seconds,
        attachment_link: String::new(),
        attachment_type: String::new(),
        attachment_filename: String::new(),
        attachment_size: 0,
        attachment_thumbnail: String::new(),
        has_more_attachment: false,
        mention_spans: Vec::new(),
    };
    apply_preview_attachments(&mut preview);
    if preview.attachment_link.is_empty() && !message.attachments.is_empty() {
        apply_preview_attachments_from_bytes(&mut preview, &message.attachments);
    }
    preview
}

fn enrich_message_preview_from_raw(preview: &mut InboxMessagePreview, raw: &[u8]) {
    if is_valid_inbox_message_id(&preview.message_id) {
        return;
    }
    let (message_id, _) = crate::transport::parse_notification_content(raw);
    if message_id > 0 {
        preview.message_id = message_id.to_string();
    }
}
pub fn message_content_is_attachment(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content.trim())
        .ok()
        .is_some_and(|value| {
            let text_empty = value
                .get("t")
                .and_then(|t| t.as_str())
                .is_none_or(str::is_empty);
            if !text_empty {
                return false;
            }
            json_has_rich_payload(&value)
        })
}

fn json_has_rich_payload(value: &serde_json::Value) -> bool {
    let has_embed = value.get("embed").is_some_and(|embed| match embed {
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Null => false,
        _ => true,
    });
    has_embed || json_attachments(value).is_some_and(|items| !items.is_empty()) || {
        value
            .get("components")
            .and_then(|items| items.as_array())
            .is_some_and(|items| !items.is_empty())
    }
}

fn json_attachments(value: &serde_json::Value) -> Option<Vec<serde_json::Value>> {
    let attachments = value.get("attachments")?;
    if let Some(items) = attachments.as_array() {
        return Some(items.clone());
    }
    attachments
        .as_str()
        .and_then(|raw| serde_json::from_str::<Vec<serde_json::Value>>(raw).ok())
}

struct ParsedInboxAttachment {
    url: String,
    filetype: String,
    filename: String,
    size: u64,
    thumbnail: String,
    has_more: bool,
}

fn first_attachment_from_json(value: &serde_json::Value) -> Option<ParsedInboxAttachment> {
    let items = json_attachments(value)?;
    let item = items.iter().find(|item| {
        item.get("url")
            .and_then(|v| v.as_str())
            .is_some_and(|url| !url.is_empty())
    })?;
    let url = item.get("url").and_then(|v| v.as_str())?.to_string();
    let filetype = item
        .get("filetype")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let filename = item
        .get("filename")
        .and_then(|v| v.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| filename_from_url(&url));
    let size = item.get("size").map(json_u64).unwrap_or(0);
    let thumbnail = item
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(ParsedInboxAttachment {
        url,
        filetype,
        filename,
        size,
        thumbnail,
        has_more: items.len() > 1,
    })
}

fn apply_preview_attachments(preview: &mut InboxMessagePreview) {
    let sources = [preview.raw_content.clone(), preview.content.clone()];
    for raw in sources {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if apply_first_attachment(preview, &value) {
            break;
        }
        if let Some(inner) = value.get("content").and_then(|v| v.as_str())
            && let Ok(inner_value) = serde_json::from_str::<serde_json::Value>(inner)
            && apply_first_attachment(preview, &inner_value)
        {
            break;
        }
    }
    if !preview.attachment_link.is_empty() && preview.attachment_filename.is_empty() {
        preview.attachment_filename = filename_from_url(&preview.attachment_link);
    }
}

fn apply_first_attachment(preview: &mut InboxMessagePreview, value: &serde_json::Value) -> bool {
    let Some(parsed) = first_attachment_from_json(value) else {
        return false;
    };
    if preview.attachment_link.is_empty() {
        preview.attachment_link = parsed.url;
        preview.attachment_type = parsed.filetype;
        preview.has_more_attachment = parsed.has_more;
    }
    if preview.attachment_filename.is_empty() {
        preview.attachment_filename = parsed.filename;
    }
    if preview.attachment_size == 0 {
        preview.attachment_size = parsed.size;
    }
    if preview.attachment_thumbnail.is_empty() {
        preview.attachment_thumbnail = parsed.thumbnail;
    }
    true
}

fn apply_preview_attachments_from_bytes(preview: &mut InboxMessagePreview, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let attachments = match std::str::from_utf8(bytes) {
        Ok(raw) => crate::transport::parse_search_attachment_field(raw),
        Err(_) => Vec::new(),
    };
    let Some(first) = attachments.iter().find(|att| !att.url.is_empty()) else {
        return;
    };
    if preview.attachment_link.is_empty() {
        preview.attachment_link = first.url.clone();
        preview.attachment_type = first.filetype.clone();
        preview.has_more_attachment = attachments.len() > 1;
    }
    if preview.attachment_filename.is_empty() {
        preview.attachment_filename = if first.filename.is_empty() {
            filename_from_url(&first.url)
        } else {
            first.filename.clone()
        };
    }
    if preview.attachment_size == 0 {
        preview.attachment_size = first.size.max(0) as u64;
    }
    if preview.attachment_thumbnail.is_empty() {
        preview.attachment_thumbnail = first.thumbnail.clone();
    }
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
                apply_preview_attachments(&mut preview);
                return Some(preview);
            }
        }
        return parse_message_preview_json(bytes);
    }
    if let Ok(message) = api::ChannelMessage::decode(bytes)
        && message.message_id > 0
    {
        return Some(preview_from_channel_message(message));
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
        #[serde(default)]
        attachments: serde_json::Value,
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
        attachment_filename: String::new(),
        attachment_size: 0,
        attachment_thumbnail: String::new(),
        has_more_attachment: raw.has_more_attachment,
        mention_spans: mention_spans_from_json_content(&raw.content),
    };
    if preview.content.is_empty() && !preview.raw_content.is_empty() {
        preview.content = display_text_from_message_content(&preview.raw_content);
    }
    apply_preview_attachments(&mut preview);
    apply_first_attachment(
        &mut preview,
        &serde_json::json!({ "attachments": raw.attachments }),
    );
    Some(preview)
}

impl InboxNotification {
    pub fn effective_message_id(&self) -> Option<String> {
        self.message.as_ref().and_then(|preview| {
            is_valid_inbox_message_id(&preview.message_id).then(|| preview.message_id.clone())
        })
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedInboxMessageInput {
    pub message_id: i64,
    pub channel_id: i64,
    pub clan_id: i64,
    pub sender_id: i64,
    pub username: String,
    pub display_name: String,
    pub avatar: String,
    pub content_json: String,
    pub create_time_seconds: u32,
    pub attachment_link: String,
    pub attachment_type: String,
    pub attachment_filename: String,
    pub attachment_size: u64,
    pub attachment_thumbnail: String,
    pub has_more_attachment: bool,
    pub mention_spans: Vec<InboxMentionSpan>,
    pub channel_type: i32,
    pub topic_id: Option<i64>,
}

pub fn pending_inbox_notification_id(message_id: i64) -> String {
    format!("pending-{message_id}")
}

pub fn is_pending_inbox_notification_id(id: &str) -> bool {
    id.starts_with("pending-")
}

pub fn inbox_notification_from_marked_message_local(
    marked: &MarkedInboxMessageInput,
) -> InboxNotification {
    let mut notification = inbox_notification_from_marked_message(
        marked.message_id,
        marked.create_time_seconds,
        marked,
    );
    notification.id = pending_inbox_notification_id(marked.message_id);
    notification
}

pub fn inbox_notification_from_marked_message(
    notification_id: i64,
    create_time_seconds: u32,
    marked: &MarkedInboxMessageInput,
) -> InboxNotification {
    let mut preview = InboxMessagePreview {
        message_id: id_str(marked.message_id),
        channel_id: id_str(marked.channel_id),
        clan_id: id_str(marked.clan_id),
        sender_id: id_str(marked.sender_id),
        content: display_text_from_message_content(&marked.content_json),
        raw_content: marked.content_json.clone(),
        avatar: marked.avatar.clone(),
        display_name: marked.display_name.clone(),
        username: marked.username.clone(),
        create_time_seconds: marked.create_time_seconds,
        attachment_link: marked.attachment_link.clone(),
        attachment_type: marked.attachment_type.clone(),
        attachment_filename: marked.attachment_filename.clone(),
        attachment_size: marked.attachment_size,
        attachment_thumbnail: marked.attachment_thumbnail.clone(),
        has_more_attachment: marked.has_more_attachment,
        mention_spans: marked.mention_spans.clone(),
    };
    apply_preview_attachments(&mut preview);
    InboxNotification {
        id: id_str(notification_id),
        category: InboxCategory::Messages,
        subject: "Message To Inbox".into(),
        sender_id: id_str(marked.sender_id),
        clan_id: id_str(marked.clan_id),
        channel_id: id_str(marked.channel_id),
        topic_id: marked
            .topic_id
            .filter(|id| *id != 0)
            .map(|id| id.to_string()),
        channel_type: marked.channel_type,
        avatar_url: marked.avatar.clone(),
        create_time_seconds,
        code: INBOX_MESSAGE_MARK_CODE,
        message: Some(preview),
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
                apply_preview_attachments(&mut preview);
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
        enrich_message_preview_from_raw(message, &n.content);
        apply_preview_attachments(message);
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
    fn display_text_hides_attachment_only_json() {
        assert_eq!(
            display_text_from_message_content(
                r#"{"attachments":[{"filename":"a.pdf","url":"https://cdn/a.pdf","filetype":"application/pdf"}]}"#
            ),
            ""
        );
    }

    #[test]
    fn message_content_is_attachment_detects_attachments_array() {
        assert!(message_content_is_attachment(
            r#"{"attachments":[{"filename":"a.pdf","url":"https://cdn/a.pdf","filetype":"application/pdf"}]}"#
        ));
        assert!(!message_content_is_attachment(r#"{"t":"hello"}"#));
    }

    #[test]
    fn parse_notification_content_extracts_webhook_attachments() {
        let bytes = br#"{"attachments":[{"filename":"bao_cao.pdf","url":"https://cdn/bao_cao.pdf","filetype":"application/pdf"},{"filename":"anh.png","url":"https://cdn/anh.png","filetype":"image/png"}]}"#;
        let preview = parse_notification_content(bytes).expect("preview");
        assert!(preview.content.is_empty());
        assert_eq!(preview.attachment_link, "https://cdn/bao_cao.pdf");
        assert_eq!(preview.attachment_type, "application/pdf");
        assert_eq!(preview.attachment_filename, "bao_cao.pdf");
        assert!(preview.has_more_attachment);
    }

    #[test]
    fn parse_notification_content_extracts_video_attachment() {
        let bytes = br#"{"attachments":[{"filename":"clip.mp4","url":"https://cdn/clip.mp4","filetype":"video/mp4","size":2048000,"thumbnail":"https://cdn/clip.jpg"}]}"#;
        let preview = parse_notification_content(bytes).expect("preview");
        assert_eq!(preview.attachment_link, "https://cdn/clip.mp4");
        assert_eq!(preview.attachment_type, "video/mp4");
        assert_eq!(preview.attachment_filename, "clip.mp4");
        assert_eq!(preview.attachment_size, 2_048_000);
        assert_eq!(preview.attachment_thumbnail, "https://cdn/clip.jpg");
        assert!(attachment_link_is_video(
            &preview.attachment_link,
            &preview.attachment_type
        ));
    }

    #[test]
    fn attachment_link_is_video_detects_mp4() {
        assert!(attachment_link_is_video(
            "https://cdn/clip.mp4",
            "video/mp4"
        ));
        assert!(!attachment_link_is_video(
            "https://cdn/bao_cao.pdf",
            "application/pdf"
        ));
    }

    #[test]
    fn parse_message_preview_json_reads_attachments_inside_content() {
        let bytes = br#"{"message_id":"42","sender_id":"9","content":"{\"attachments\":[{\"url\":\"https://cdn/a.pdf\",\"filetype\":\"application/pdf\"}]}","display_name":"KOMU"}"#;
        let preview = parse_notification_content(bytes).expect("preview");
        assert!(preview.content.is_empty());
        assert_eq!(preview.attachment_link, "https://cdn/a.pdf");
        assert_eq!(preview.attachment_type, "application/pdf");
    }

    #[test]
    fn parse_message_preview_json_reads_sibling_attachments() {
        let bytes = br#"{"message_id":"42","sender_id":"9","content":"{\"t\":\"\"}","attachments":[{"url":"https://cdn/b.pdf","filetype":"application/pdf","filename":"b.pdf","size":512}],"display_name":"KOMU"}"#;
        let preview = parse_notification_content(bytes).expect("preview");
        assert_eq!(preview.attachment_link, "https://cdn/b.pdf");
        assert_eq!(preview.attachment_type, "application/pdf");
        assert_eq!(preview.attachment_filename, "b.pdf");
        assert_eq!(preview.attachment_size, 512);
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
    fn parse_notification_content_reads_channel_message_bytes() {
        let message = api::ChannelMessage {
            message_id: 99,
            channel_id: 7,
            clan_id: 1,
            sender_id: 9,
            content: r#"{"t":"hello"}"#.into(),
            create_time_seconds: 1_700_000_000,
            ..Default::default()
        };
        let bytes = message.encode_to_vec();
        let notification = inbox_notification_from_api(api::Notification {
            id: 1,
            category: InboxCategory::Mentions as i32,
            content: bytes,
            channel_id: 7,
            clan_id: 1,
            ..Default::default()
        })
        .expect("notification");
        assert_eq!(notification.effective_message_id().as_deref(), Some("99"));
    }

    #[test]
    fn inbox_notification_from_marked_message_local_uses_pending_id() {
        let marked = MarkedInboxMessageInput {
            message_id: 42,
            channel_id: 7,
            clan_id: 1,
            sender_id: 9,
            username: "user".into(),
            display_name: "User".into(),
            avatar: "a.png".into(),
            content_json: r#"{"t":"hello"}"#.into(),
            create_time_seconds: 1_700_000_000,
            attachment_link: String::new(),
            attachment_type: String::new(),
            attachment_filename: String::new(),
            attachment_size: 0,
            attachment_thumbnail: String::new(),
            has_more_attachment: false,
            mention_spans: Vec::new(),
            channel_type: 1,
            topic_id: None,
        };
        let notification = inbox_notification_from_marked_message_local(&marked);
        assert_eq!(notification.id, "pending-42");
        assert_eq!(notification.effective_message_id().as_deref(), Some("42"));
    }

    #[test]
    fn inbox_notification_from_marked_message_builds_preview() {
        let marked = MarkedInboxMessageInput {
            message_id: 42,
            channel_id: 7,
            clan_id: 1,
            sender_id: 9,
            username: "user".into(),
            display_name: "User".into(),
            avatar: "a.png".into(),
            content_json: r#"{"t":"hello"}"#.into(),
            create_time_seconds: 1_700_000_000,
            attachment_link: String::new(),
            attachment_type: String::new(),
            attachment_filename: String::new(),
            attachment_size: 0,
            attachment_thumbnail: String::new(),
            has_more_attachment: false,
            mention_spans: Vec::new(),
            channel_type: 1,
            topic_id: None,
        };
        let notification = inbox_notification_from_marked_message(99, 1_700_000_001, &marked);
        assert_eq!(notification.id, "99");
        assert_eq!(notification.category, InboxCategory::Messages);
        assert_eq!(notification.code, INBOX_MESSAGE_MARK_CODE);
        assert_eq!(notification.effective_message_id().as_deref(), Some("42"));
    }

    #[test]
    fn effective_message_id_rejects_zero() {
        let mut preview = InboxMessagePreview::empty_content(String::new());
        preview.message_id = "0".into();
        let notification = InboxNotification {
            id: "1".into(),
            category: InboxCategory::Messages,
            subject: String::new(),
            sender_id: String::new(),
            clan_id: "1".into(),
            channel_id: "2".into(),
            topic_id: None,
            channel_type: 0,
            avatar_url: String::new(),
            create_time_seconds: 0,
            code: 0,
            message: Some(preview),
        };
        assert!(notification.effective_message_id().is_none());
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
