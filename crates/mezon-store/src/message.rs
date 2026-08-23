use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui::{Rgba, SharedString};
use mezon_client::transport::{
    ApiMessageContent, ApiMessageReaction, ContentToken, is_here_user_id,
};

use crate::album_layout::AlbumLayout;
use crate::config::AppConfig;
use crate::ids::{ChannelId, MessageId, UserId};
use crate::message_time::{format_local_time_hhmm, local_datetime, local_day_key};

#[derive(Debug, Clone, Default)]
pub struct MessageAttachment {
    pub url: String,
    pub filename: String,
    pub filetype: String,
    pub width: u32,
    pub height: u32,
    pub thumbnail: String,
    pub duration: i32,
    pub size: u64,
    pub size_label: SharedString,
    pub presign_pending: bool,
    pub proxied_src: SharedString,
    pub thumbnail_proxied: SharedString,
    pub display_width: f32,
    pub display_height: f32,
    pub tenor_mp4: Option<SharedString>,
    pub local_source: Option<std::path::PathBuf>,
    pub uploading: bool,
    pub upload_failed: bool,
}

pub const STICKER_FILETYPE: &str = "sticker";

pub fn url_extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?;
    let tail = name.rsplit('@').next().unwrap_or(name);
    let ext = if tail.contains('.') {
        tail.rsplit('.').next()?
    } else {
        tail
    };
    (!ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

pub fn is_image_type(filetype: &str, url: &str) -> bool {
    let ext = url_extension(url);
    if is_svg_type(filetype) || ext.as_deref() == Some("svg") {
        return false;
    }
    let mime_image = filetype.starts_with("image") || filetype == STICKER_FILETYPE;
    mime_image
        || matches!(
            ext.as_deref(),
            Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "avif")
        )
}

fn is_svg_type(filetype: &str) -> bool {
    filetype.contains("svg+xml")
}

pub fn format_file_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} kB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} bytes")
    }
}

impl MessageAttachment {
    pub fn is_audio(&self) -> bool {
        self.filetype.contains("audio") && !self.is_unsupported_media()
    }

    pub fn is_image(&self) -> bool {
        is_image_type(&self.filetype, &self.url)
    }

    pub fn media_is_video(filetype: &str, url: &str) -> bool {
        ((filetype.contains("video/mp4") || filetype.contains("video/quicktime"))
            && !url.contains("tenor.com"))
            || (filetype.starts_with("video") && !filetype.ends_with("vnd.dlna.mpeg-tts"))
    }

    pub fn is_video(&self) -> bool {
        Self::media_is_video(&self.filetype, &self.url)
    }

    /// A Matroska video (`.webm`, and the `video/matroska` MIME a browser
    /// recorder writes) rides on whatever demuxer the platform player has:
    /// GStreamer reads it, AVFoundation (macOS) and Media Foundation (Windows)
    /// do not. There the inline player can only mount, fail, and sit on a play
    /// button that never does anything, so hand the file to the download box
    /// instead. Audio `.webm` (voice messages) is decoded in-app by symphonia
    /// and is deliberately left alone.
    fn is_undecodable_matroska(&self, ext: Option<&str>) -> bool {
        if cfg!(target_os = "linux") || self.filetype.contains("audio") {
            return false;
        }
        matches!(
            self.filetype.as_str(),
            "video/webm" | "video/matroska" | "video/x-matroska"
        ) || ext == Some("webm")
    }

    pub fn is_unsupported_media(&self) -> bool {
        if matches!(
            self.filetype.as_str(),
            "video/x-ms-wmv"
                | "video/wmv"
                | "video/avi"
                | "video/flv"
                | "video/mkv"
                | "video/rmvb"
                | "audio/wma"
                | "audio/ra"
                | "audio/atrac"
                | "image/tiff"
                | "image/bmp"
                | "image/psd"
        ) {
            return true;
        }
        let ext = url_extension(&self.filename).or_else(|| url_extension(&self.url));
        if self.is_undecodable_matroska(ext.as_deref()) {
            return true;
        }
        matches!(
            ext.as_deref(),
            Some(
                "wmv"
                    | "avi"
                    | "flv"
                    | "mkv"
                    | "rmvb"
                    | "wma"
                    | "ra"
                    | "tiff"
                    | "tif"
                    | "bmp"
                    | "psd"
            )
        )
    }
}

pub fn tenor_mp4_url(gif_url: &str) -> Option<String> {
    let rest = gif_url.strip_prefix("https://media.tenor.com/")?;
    let (media_id, name) = rest.split_once('/')?;
    let name = name.strip_suffix(".gif")?;
    if media_id.len() != 16 || !media_id.is_ascii() || name.is_empty() {
        return None;
    }
    let content_id = &media_id[..11];
    Some(format!(
        "https://media.tenor.com/{content_id}AAAPo/{name}.mp4"
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageCode {
    Chat,
    ChatUpdate,
    ChatRemove,
    Typing,
    Indicator,
    Welcome,
    CreateThread,
    CreatePin,
    MessageBuzz,
    Topic,
    AuditLog,
    SendToken,
    Ephemeral,
    UpcomingEvent,
    UpdateEphemeralMsg,
    DeleteEphemeralMsg,
    ShareContact,
    Location,
    Poll,
    DeleteThread,
    Unknown(i32),
}

impl MessageCode {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            0 => MessageCode::Chat,
            1 => MessageCode::ChatUpdate,
            2 => MessageCode::ChatRemove,
            3 => MessageCode::Typing,
            4 => MessageCode::Indicator,
            5 => MessageCode::Welcome,
            6 => MessageCode::CreateThread,
            7 => MessageCode::CreatePin,
            8 => MessageCode::MessageBuzz,
            9 => MessageCode::Topic,
            10 => MessageCode::AuditLog,
            11 => MessageCode::SendToken,
            12 => MessageCode::Ephemeral,
            13 => MessageCode::UpcomingEvent,
            14 => MessageCode::UpdateEphemeralMsg,
            15 => MessageCode::DeleteEphemeralMsg,
            16 => MessageCode::ShareContact,
            17 => MessageCode::Location,
            18 => MessageCode::Poll,
            19 => MessageCode::DeleteThread,
            other => MessageCode::Unknown(other),
        }
    }

    pub fn is_system(self) -> bool {
        matches!(
            self,
            MessageCode::Welcome
                | MessageCode::UpcomingEvent
                | MessageCode::CreateThread
                | MessageCode::CreatePin
                | MessageCode::AuditLog
                | MessageCode::DeleteThread
        )
    }

    pub fn is_user_timeline(self) -> bool {
        !matches!(
            self,
            MessageCode::Indicator
                | MessageCode::Typing
                | MessageCode::ChatUpdate
                | MessageCode::ChatRemove
                | MessageCode::UpdateEphemeralMsg
                | MessageCode::DeleteEphemeralMsg
        ) && !self.is_system()
    }
}

#[derive(Debug, Clone, Default)]
pub struct MessageReference {
    pub message_ref_id: MessageId,
    pub sender_id: UserId,
    pub sender_name: String,
    pub sender_clan_nick: String,
    pub sender_display_name: String,
    pub sender_username: String,
    pub sender_avatar: String,
    pub content: String,
    pub content_preview: SharedString,
    pub has_attachment: bool,
    pub has_embed: bool,
    pub is_poll: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReactionSender {
    pub sender_id: String,
    pub count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reaction {
    pub key: String,
    pub emoji: SharedString,
    pub emoji_id: SharedString,
    pub count: u32,
    pub count_label: SharedString,
    pub senders: Vec<ReactionSender>,
}

impl Reaction {
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn has_sender(&self, sender_id: &str) -> bool {
        self.senders
            .iter()
            .any(|s| s.sender_id == sender_id && s.count > 0)
    }

    fn refresh(&mut self) {
        self.count = self.senders.iter().map(|s| s.count).sum();
        self.count_label = format_reaction_count(self.count).into();
    }
}

pub fn format_reaction_count(count: u32) -> String {
    if count < 1000 {
        return count.to_string();
    }
    const UNITS: [&str; 5] = ["", "K", "M", "G", "T"];
    let unit_index = (((count as f64).log10() / 3.0).floor() as usize).min(UNITS.len() - 1);
    let value = count as f64 / 1000f64.powi(unit_index as i32);
    format!("{:.1}{}", value, UNITS[unit_index])
}

pub fn reaction_key<'a>(emoji_id: &'a str, emoji: &'a str) -> &'a str {
    if !emoji_id.is_empty() && emoji_id != "0" {
        emoji_id
    } else {
        emoji
    }
}

fn upsert_sender(senders: &mut Vec<ReactionSender>, sender_id: &str, count: u32, set: bool) {
    match senders.iter_mut().find(|s| s.sender_id == sender_id) {
        Some(s) => {
            s.count = if set {
                count
            } else {
                s.count.saturating_add(count)
            };
        }
        None => senders.push(ReactionSender {
            sender_id: sender_id.to_string(),
            count,
        }),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MentionTarget {
    pub user_id: Option<String>,
    pub role_id: Option<String>,
    pub username: String,
    pub s: i32,
    pub e: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkKind {
    #[default]
    Plain,
    YouTube,
    Facebook,
    TikTok,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageSpan {
    Text(SharedString),
    Bold(SharedString),
    Code(SharedString),
    CodeBlock {
        language: Option<String>,
        text: SharedString,
        fenced_source: SharedString,
    },
    Link {
        text: SharedString,
        url: String,
        kind: LinkKind,
    },
    Mention {
        display: SharedString,
        user_id: Option<String>,
        role_id: Option<String>,
    },
    Hashtag {
        display: SharedString,
        channel_id: Option<String>,
    },
    Emoji {
        name: SharedString,
        emoji_id: String,
        src: SharedString,
    },
    Canvas {
        title: SharedString,
        clan_id: String,
        channel_id: String,
        canvas_id: String,
    },
    Heading {
        level: u8,
        text: SharedString,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OgpPreview {
    pub url: String,
    pub title: SharedString,
    pub description: SharedString,
    pub description_collapsed: SharedString,
    pub image_proxied: SharedString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallLogType {
    StartCall,
    TimeoutCall,
    FinishCall,
    RejectCall,
    CancelCall,
    Unknown(i32),
}

impl CallLogType {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            1 => CallLogType::StartCall,
            2 => CallLogType::TimeoutCall,
            3 => CallLogType::FinishCall,
            4 => CallLogType::RejectCall,
            5 => CallLogType::CancelCall,
            other => CallLogType::Unknown(other),
        }
    }

    pub fn raw(self) -> i32 {
        match self {
            CallLogType::StartCall => 1,
            CallLogType::TimeoutCall => 2,
            CallLogType::FinishCall => 3,
            CallLogType::RejectCall => 4,
            CallLogType::CancelCall => 5,
            CallLogType::Unknown(other) => other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallLog {
    pub is_video: bool,
    pub log_type: CallLogType,
    pub show_call_back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedAuthor {
    pub name: SharedString,
    pub icon_proxied: SharedString,
    pub url: Option<SharedString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedImage {
    pub url_proxied: SharedString,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedFooter {
    pub text: SharedString,
    pub icon_proxied: SharedString,
}

#[derive(Debug, Clone)]
pub struct EmbedTextInput {
    pub id: SharedString,
    pub placeholder: SharedString,
    pub default_value: SharedString,
    pub multiline: bool,
    pub required: bool,
    pub disabled: bool,
}

#[derive(Debug, Clone)]
pub enum EmbedInput {
    Text(EmbedTextInput),
    Select(MessageSelect),
}

#[derive(Debug, Clone)]
pub struct EmbedField {
    pub name: SharedString,
    pub value: SharedString,
    pub inline: bool,
    pub input: Option<EmbedInput>,
}

#[derive(Debug, Clone)]
pub struct Embed {
    pub accent: Option<Rgba>,
    pub title: SharedString,
    pub url: Option<SharedString>,
    pub author: Option<EmbedAuthor>,
    pub description_spans: Vec<MessageSpan>,
    pub thumbnail_proxied: SharedString,
    pub image: Option<EmbedImage>,
    pub footer: Option<EmbedFooter>,
    pub fields: Arc<[EmbedField]>,
    pub timestamp: SharedString,
    pub footer_date: SharedString,
}

#[derive(Debug, Clone)]
pub struct MessageButton {
    pub label: SharedString,
    pub style: i32,
    pub url: Option<SharedString>,
    pub disable: bool,
    pub id: Option<SharedString>,
    pub icon: Option<SharedString>,
}

#[derive(Debug, Clone)]
pub struct MessageSelectOption {
    pub label: SharedString,
    pub value: SharedString,
    pub description: Option<SharedString>,
    pub default: bool,
}

#[derive(Debug, Clone)]
pub struct MessageSelect {
    pub select_type: i32,
    pub options: Vec<MessageSelectOption>,
    pub placeholder: Option<SharedString>,
    pub min_options: Option<i32>,
    pub max_options: Option<i32>,
    pub disabled: bool,
    pub id: Option<SharedString>,
    pub value_selected: Option<SharedString>,
}

pub fn select_allows_multiple(min_options: Option<i32>, max_options: Option<i32>) -> bool {
    min_options.is_some_and(|value| value > 1) || max_options.is_some_and(|value| value >= 2)
}

impl MessageSelect {
    pub fn allows_multiple(&self) -> bool {
        select_allows_multiple(self.min_options, self.max_options)
    }
}

#[derive(Debug, Clone)]
pub enum MessageComponent {
    Button(MessageButton),
    Select(MessageSelect),
    Other,
}

#[derive(Debug, Clone)]
pub struct MessageComponentRow {
    pub components: Vec<MessageComponent>,
}

#[derive(Debug, Clone)]
pub struct InvitePreview {
    pub title: SharedString,
    pub image_proxied: SharedString,
    pub banner_proxied: SharedString,
    pub member_count: i64,
    pub is_community: bool,
    pub clan_id: Option<String>,
    pub url: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollLabelSegment {
    Text(SharedString),
    Emoji(SharedString),
}

#[derive(Debug, Clone)]
pub struct PollAnswerView {
    pub index: i32,
    pub label: SharedString,
    pub segments: Vec<PollLabelSegment>,
}

#[derive(Debug, Clone)]
pub struct PollData {
    pub poll_id: i64,
    pub question: SharedString,
    pub answers: Vec<PollAnswerView>,
    pub answer_counts: Vec<i32>,
    pub total_votes: i32,
    pub percentages: Vec<u8>,
    pub expire_at: Option<i64>,
    pub is_closed: bool,
    pub allow_multiple: bool,
}

impl PollData {
    pub fn is_expired(&self, now_secs: i64) -> bool {
        self.expire_at.is_some_and(|exp| exp > 0 && exp < now_secs)
    }
}

#[derive(Debug, Clone)]
pub struct PollVoter {
    pub user_id: UserId,
    pub display_name: SharedString,
    pub username: SharedString,
    pub avatar_proxied: SharedString,
}

#[derive(Debug, Clone)]
pub struct PollDetail {
    pub total_votes: i32,
    pub answer_counts: Vec<i32>,
    pub voters_by_answer: Vec<Vec<PollVoter>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RichRunKind {
    Bold,
    Code,
    Link,
    Mention,
    RoleMention,
    Hashtag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RichClick {
    Link(SharedString),
    Mention(UserId),
    Channel(ChannelId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichRun {
    pub range: Range<usize>,
    pub kind: RichRunKind,
    pub click: Option<RichClick>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichLayout {
    pub text: SharedString,
    pub runs: Arc<[RichRun]>,
    pub content_tokens: Option<Arc<[RichToken]>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RichToken {
    Word(SharedString),
    LineBreak,
    Span(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenTransaction {
    pub title: SharedString,
    pub detail: SharedString,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: MessageId,
    pub sort_id: i64,
    pub row_anchor_id: MessageId,
    pub content: String,
    pub sender_id: String,
    pub sender_user_id: Option<UserId>,
    pub sender_name: SharedString,
    pub avatar_url: SharedString,
    pub avatar_proxied: SharedString,
    pub create_time: i64,
    pub update_time: i64,
    pub day_label: String,
    pub time_hhmm: SharedString,
    pub local_date: Option<chrono::NaiveDate>,
    pub code: MessageCode,
    pub is_edited: bool,
    pub is_forwarded: bool,
    pub show_forwarded_label: bool,
    pub combined_with_prev: bool,
    pub highlights_viewer_direct: bool,
    pub send_failed: bool,
    pub ogp: Option<Box<OgpPreview>>,
    pub poll: Option<Box<PollData>>,
    pub call_log: Option<CallLog>,
    pub embeds: Arc<[Embed]>,
    pub components: Arc<[MessageComponentRow]>,
    pub is_card: bool,
    pub topic_id: Option<ChannelId>,
    pub topic_creator_id: Option<UserId>,
    pub invite: Option<Box<InvitePreview>>,
    pub is_only_emoji: bool,
    pub is_deleted_placeholder: bool,
    pub spans: Vec<MessageSpan>,
    pub rich_layout: Option<Arc<RichLayout>>,
    pub token_transaction: Option<Box<TokenTransaction>>,
    pub mention_targets: Vec<MentionTarget>,
    pub references: Vec<MessageReference>,
    pub reactions: Vec<Reaction>,
    pub attachments: Vec<MessageAttachment>,
    pub album_layout: Option<AlbumLayout>,
    pub viewer_media: Arc<[ViewerMedia]>,
    /// The content JSON as received from the server, kept so forwarding can
    /// re-send the whole payload (markdown/emoji/hashtag/embed tokens) rather
    /// than just the plain text. `None` for optimistic messages.
    pub raw_content: Option<Arc<str>>,
}

#[derive(Debug, Clone)]
pub struct ViewerMedia {
    pub url: SharedString,
    pub filename: SharedString,
    pub viewer_src: SharedString,
}

pub const COMBINE_TIME_WINDOW: i64 = 600;

pub fn same_message_sender(a: &Message, b: &Message) -> bool {
    if let (Some(au), Some(bu)) = (resolved_sender_user_id(a), resolved_sender_user_id(b))
        && au == bu
    {
        return true;
    }
    let a_id = a.sender_id.as_str();
    let b_id = b.sender_id.as_str();
    !a_id.is_empty() && a_id != "0" && !b_id.is_empty() && b_id != "0" && a_id == b_id
}

fn resolved_sender_user_id(m: &Message) -> Option<UserId> {
    if let Some(uid) = m.sender_user_id.filter(|u| u.0 != 0) {
        return Some(uid);
    }
    if m.sender_id.is_empty() || m.sender_id == "0" {
        return None;
    }
    m.sender_id
        .parse::<i64>()
        .ok()
        .filter(|&id| id != 0)
        .map(UserId)
}

pub fn message_combined_with_prev(prev: Option<&Message>, msg: &Message) -> bool {
    if !msg.code.is_user_timeline() {
        return false;
    }
    let Some(prev) = prev else {
        return false;
    };
    if msg.create_time == 0 {
        return false;
    }
    let delta = msg.create_time - prev.create_time;
    same_message_sender(prev, msg) && delta < COMBINE_TIME_WINDOW
}

pub fn should_show_message_head(msg: &Message, is_combine: bool) -> bool {
    !msg.references.is_empty() || !is_combine
}

pub fn message_row_highlight(msg: &Message, viewer_id: Option<UserId>, role_ids: &[i64]) -> bool {
    viewer_highlight_direct(&msg.references, &msg.mention_targets, &msg.spans, viewer_id)
        || message_row_highlight_roles(msg, role_ids)
}

pub(crate) fn viewer_highlight_direct(
    references: &[MessageReference],
    mention_targets: &[MentionTarget],
    spans: &[MessageSpan],
    viewer_id: Option<UserId>,
) -> bool {
    let Some(viewer_id) = viewer_id else {
        return false;
    };
    if references
        .iter()
        .any(|reference| reference.sender_id == viewer_id)
    {
        return true;
    }
    if mention_targets.iter().any(|target| {
        target
            .user_id
            .as_deref()
            .is_some_and(|uid| mention_user_id_targets_viewer(uid, viewer_id))
    }) {
        return true;
    }
    spans.iter().any(|span| {
        matches!(span, MessageSpan::Mention { user_id, .. }
            if user_id
                .as_deref()
                .is_some_and(|uid| mention_user_id_targets_viewer(uid, viewer_id)))
    })
}

pub fn message_row_highlight_roles(msg: &Message, role_ids: &[i64]) -> bool {
    if role_ids.is_empty() {
        return false;
    }
    if msg.mention_targets.iter().any(|target| {
        target.role_id.as_deref().is_some_and(|rid| {
            rid.parse::<i64>()
                .ok()
                .is_some_and(|id| role_ids.contains(&id))
        })
    }) {
        return true;
    }
    msg.spans.iter().any(|span| {
        matches!(span, MessageSpan::Mention { role_id, .. }
            if role_id.as_deref().is_some_and(|r| !r.is_empty())
                && role_id
                    .as_deref()
                    .and_then(|r| r.parse::<i64>().ok())
                    .is_some_and(|id| role_ids.contains(&id)))
    })
}

fn mention_user_id_targets_viewer(uid: &str, viewer_id: UserId) -> bool {
    !uid.is_empty()
        && uid != "0"
        && (mezon_client::transport::is_here_user_id(uid)
            || uid.parse::<i64>().ok().map(UserId) == Some(viewer_id))
}

pub fn aggregate_reactions(raw: &[ApiMessageReaction]) -> Vec<Reaction> {
    let mut out: Vec<Reaction> = Vec::new();
    for r in raw {
        if r.action {
            continue;
        }
        let key = reaction_key(&r.emoji_id, &r.emoji);
        if key.is_empty() {
            continue;
        }
        let count = if r.count == 0 { 1 } else { r.count };
        let idx = match out.iter().position(|x| x.key == key) {
            Some(i) => i,
            None => {
                out.push(Reaction {
                    key: key.to_string(),
                    emoji: r.emoji.clone().into(),
                    emoji_id: r.emoji_id.clone().into(),
                    ..Default::default()
                });
                out.len() - 1
            }
        };
        upsert_sender(&mut out[idx].senders, &r.sender_id, count, true);
    }
    for r in out.iter_mut() {
        r.refresh();
    }
    out.retain(|r| r.count > 0);
    out
}

pub fn apply_reaction_event(
    reactions: &mut Vec<Reaction>,
    emoji_id: &str,
    emoji: &str,
    sender_id: &str,
    removed: bool,
) {
    let key = reaction_key(emoji_id, emoji);
    if key.is_empty() {
        return;
    }
    if removed {
        if let Some(pos) = reactions.iter().position(|x| x.key == key) {
            reactions[pos].senders.retain(|s| s.sender_id != sender_id);
            reactions[pos].refresh();
            if reactions[pos].count == 0 {
                reactions.remove(pos);
            }
        }
    } else if let Some(rec) = reactions.iter_mut().find(|x| x.key == key) {
        upsert_sender(&mut rec.senders, sender_id, 1, false);
        rec.refresh();
    } else {
        let mut rec = Reaction {
            key: key.to_string(),
            emoji: emoji.into(),
            emoji_id: emoji_id.into(),
            senders: vec![ReactionSender {
                sender_id: sender_id.to_string(),
                count: 1,
            }],
            ..Default::default()
        };
        rec.refresh();
        reactions.push(rec);
    }
}

pub fn rollback_reaction(
    reactions: &mut Vec<Reaction>,
    emoji_id: &str,
    emoji: &str,
    sender_id: &str,
    was_remove: bool,
) {
    if was_remove {
        apply_reaction_event(reactions, emoji_id, emoji, sender_id, false);
        return;
    }
    let key = reaction_key(emoji_id, emoji);
    let Some(pos) = reactions.iter().position(|x| x.key == key) else {
        return;
    };
    if let Some(s) = reactions[pos]
        .senders
        .iter_mut()
        .find(|s| s.sender_id == sender_id)
    {
        s.count = s.count.saturating_sub(1);
    }
    reactions[pos].senders.retain(|s| s.count > 0);
    reactions[pos].refresh();
    if reactions[pos].count == 0 {
        reactions.remove(pos);
    }
}

pub fn parse_spans(content: &ApiMessageContent) -> Vec<MessageSpan> {
    let text = &content.t;
    if text.is_empty() {
        return Vec::new();
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let total = units.len() as i64;
    let slice = |s: i64, e: i64| -> String {
        let s = s.clamp(0, total) as usize;
        let e = e.clamp(0, total) as usize;
        if e <= s {
            return String::new();
        }
        String::from_utf16_lossy(&units[s..e])
    };

    #[derive(Clone, Copy)]
    enum Kind {
        Mention,
        Hashtag,
        Emoji,
        Markdown,
        Link(LinkKind),
    }
    let mut toks: Vec<(i64, i64, Kind, ContentToken)> = Vec::new();
    let collect =
        |list: &[ContentToken], kind: Kind, toks: &mut Vec<(i64, i64, Kind, ContentToken)>| {
            for t in list {
                let s = t.s.unwrap_or(0);
                let e = t.e.unwrap_or(0);
                if e > s {
                    toks.push((s, e, kind, t.clone()));
                }
            }
        };
    collect(&content.mentions, Kind::Mention, &mut toks);
    collect(&content.hg, Kind::Hashtag, &mut toks);
    collect(&content.ej, Kind::Emoji, &mut toks);
    collect(&content.mk, Kind::Markdown, &mut toks);
    for t in &content.lk {
        let s = t.s.unwrap_or(0);
        let e = t.e.unwrap_or(0);
        if e > s {
            let text = slice(s, e);
            let target = resolve_link_url(t.url.as_deref().unwrap_or(""), &text);
            let kind = link_kind_from_marker(mezon_client::link_markdown_kind(&target));
            toks.push((s, e, Kind::Link(kind), t.clone()));
        }
    }
    collect(&content.vk, Kind::Link(LinkKind::Plain), &mut toks);
    collect(&content.lky, Kind::Link(LinkKind::YouTube), &mut toks);
    toks.sort_by_key(|t| t.0);

    let mut spans: Vec<MessageSpan> = Vec::new();
    let mut last = 0i64;
    let mut prev_end = i64::MIN;
    for (s, e, kind, tok) in toks {
        if s < prev_end {
            continue;
        }
        if last < s {
            spans.push(MessageSpan::Text(slice(last, s).into()));
        }
        let inner = slice(s, e);
        match kind {
            Kind::Mention => {
                let has_user = tok
                    .user_id
                    .as_deref()
                    .is_some_and(|u| u != "0" && !u.is_empty());
                let has_role = tok.role_id.as_deref().is_some_and(|r| !r.is_empty());
                let has_username = tok.username.as_deref().is_some_and(|u| !u.is_empty());
                if has_user || has_username {
                    spans.push(MessageSpan::Mention {
                        display: inner.into(),
                        user_id: tok.user_id.clone(),
                        role_id: None,
                    });
                } else if has_role {
                    spans.push(MessageSpan::Mention {
                        display: inner.into(),
                        user_id: None,
                        role_id: tok.role_id.clone(),
                    });
                } else {
                    spans.push(MessageSpan::Text(inner.into()));
                }
            }
            Kind::Hashtag => spans.push(MessageSpan::Hashtag {
                display: inner.into(),
                channel_id: tok.channel_id.clone(),
            }),
            Kind::Emoji => spans.push(MessageSpan::Emoji {
                name: inner.into(),
                emoji_id: tok.emojiid.clone().unwrap_or_default(),
                src: SharedString::default(),
            }),
            Kind::Link(link_kind) => {
                let url = tok.url.clone().unwrap_or_else(|| inner.clone());
                spans.push(resolve_link_span(
                    inner.into(),
                    url,
                    link_kind,
                    &content.cvtt,
                ));
            }
            Kind::Markdown => {
                let ty = tok.kind.as_deref().unwrap_or("");
                match ty {
                    "b" => spans.push(MessageSpan::Bold(strip_marker(&inner, "**").into())),
                    "c" | "s" => spans.push(MessageSpan::Code(strip_marker(&inner, "`").into())),
                    "t" | "pre" => {
                        let (language, text) = strip_code_fence(&inner);
                        spans.push(MessageSpan::CodeBlock {
                            language,
                            text: text.into(),
                            fenced_source: inner.clone().into(),
                        });
                    }
                    "lk" | "vk" | "lk_yt" | "lk_fb" | "lk_tt" => {
                        let url = tok.url.clone().unwrap_or_else(|| inner.clone());
                        spans.push(resolve_link_span(
                            inner.into(),
                            url,
                            link_kind_from_marker(ty),
                            &content.cvtt,
                        ));
                    }
                    "lk_ogp" => {
                        let url = tok.url.clone().unwrap_or_else(|| inner.clone());
                        spans.push(MessageSpan::Link {
                            text: inner.into(),
                            url,
                            kind: LinkKind::Plain,
                        });
                    }
                    _ => spans.push(MessageSpan::Text(inner.into())),
                }
            }
        }
        prev_end = e;
        last = e;
    }
    if last < total {
        spans.push(MessageSpan::Text(slice(last, total).into()));
    }
    apply_headings(spans)
}

pub fn inbox_spans_from_raw(raw_content: &str) -> Option<Vec<MessageSpan>> {
    let trimmed = raw_content.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return None;
    }
    let content: ApiMessageContent = serde_json::from_str(trimmed).ok()?;
    if content.t.is_empty() {
        return None;
    }
    Some(parse_spans(&content))
}

fn link_kind_from_marker(marker: &str) -> LinkKind {
    match marker {
        "lk_yt" => LinkKind::YouTube,
        "lk_fb" => LinkKind::Facebook,
        "lk_tt" => LinkKind::TikTok,
        _ => LinkKind::Plain,
    }
}

/// Inverse of [`link_kind_from_marker`] — `None` for a plain link, which carries no marker.
pub(crate) fn link_marker_from_kind(kind: LinkKind) -> Option<&'static str> {
    match kind {
        LinkKind::YouTube => Some(mezon_client::YOUTUBE_LINK_MARKDOWN_KIND),
        LinkKind::Facebook => Some(mezon_client::FACEBOOK_LINK_MARKDOWN_KIND),
        LinkKind::TikTok => Some(mezon_client::TIKTOK_LINK_MARKDOWN_KIND),
        LinkKind::Plain => None,
    }
}

struct ChannelUrlIds {
    clan_id: String,
    channel_id: String,
    canvas_id: Option<String>,
}

fn extract_channel_url_ids(url: &str) -> Option<ChannelUrlIds> {
    let marker = "/chat/clans/";
    let idx = url.find(marker)?;
    let mut parts = url[idx + marker.len()..].split('/');
    let clan_id = parts.next()?;
    if clan_id.is_empty() || !clan_id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if parts.next()? != "channels" {
        return None;
    }
    let channel_id = parts.next()?;
    if channel_id.is_empty() || !channel_id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let canvas_id = match parts.next() {
        Some("canvas") => parts
            .next()
            .map(|seg| seg.split(['?', '#']).next().unwrap_or(seg))
            .filter(|seg| !seg.is_empty())
            .map(str::to_string),
        _ => None,
    };
    Some(ChannelUrlIds {
        clan_id: clan_id.to_string(),
        channel_id: channel_id.to_string(),
        canvas_id,
    })
}

fn resolve_link_span(
    text: SharedString,
    url: String,
    kind: LinkKind,
    cvtt: &HashMap<String, String>,
) -> MessageSpan {
    if let Some(ids) = extract_channel_url_ids(&url) {
        if let Some(canvas_id) = ids.canvas_id {
            if let Some(title) = cvtt.get(&canvas_id).filter(|t| !t.is_empty()) {
                return MessageSpan::Canvas {
                    title: title.clone().into(),
                    clan_id: ids.clan_id,
                    channel_id: ids.channel_id,
                    canvas_id,
                };
            }
            return MessageSpan::Link { text, url, kind };
        }
        return MessageSpan::Hashtag {
            display: text,
            channel_id: Some(ids.channel_id),
        };
    }
    MessageSpan::Link { text, url, kind }
}

fn apply_headings(spans: Vec<MessageSpan>) -> Vec<MessageSpan> {
    if !spans
        .iter()
        .any(|span| matches!(span, MessageSpan::Text(t) if text_has_heading_line(t)))
    {
        return spans;
    }
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        match span {
            MessageSpan::Text(t) if text_has_heading_line(&t) => {
                split_text_headings(&t, &mut out);
            }
            other => out.push(other),
        }
    }
    out
}

fn text_has_heading_line(text: &str) -> bool {
    text.split('\n')
        .any(|line| parse_heading_line(line).is_some())
}

fn parse_heading_line(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    let body = rest.trim_start_matches([' ', '\t']);
    if body.len() == rest.len() || body.is_empty() {
        return None;
    }
    Some((hashes as u8, body))
}

fn split_text_headings(text: &str, out: &mut Vec<MessageSpan>) {
    let mut buf = String::new();
    for line in text.split('\n') {
        if let Some((level, body)) = parse_heading_line(line) {
            if !buf.is_empty() {
                out.push(MessageSpan::Text(std::mem::take(&mut buf).into()));
            }
            out.push(MessageSpan::Heading {
                level,
                text: body.to_string().into(),
            });
        } else {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
        }
    }
    if !buf.is_empty() {
        out.push(MessageSpan::Text(buf.into()));
    }
}

pub fn spans_only_emoji(spans: &[MessageSpan]) -> bool {
    let mut has_emoji = false;
    for span in spans {
        match span {
            MessageSpan::Emoji { .. } => has_emoji = true,
            MessageSpan::Text(t) if t.trim().is_empty() => {}
            _ => return false,
        }
    }
    has_emoji
}

pub fn markdown_edit_source(content: &str, spans: &[MessageSpan]) -> Option<String> {
    let has_markdown = spans.iter().any(|span| {
        matches!(
            span,
            MessageSpan::Bold(_) | MessageSpan::Code(_) | MessageSpan::CodeBlock { .. }
        )
    });
    if !has_markdown {
        return None;
    }
    let mut out = String::with_capacity(content.len() + 8);
    for span in spans {
        match span {
            MessageSpan::Text(t) => out.push_str(t),
            MessageSpan::Bold(t) => {
                out.push_str("**");
                out.push_str(t);
                out.push_str("**");
            }
            MessageSpan::Code(t) => {
                out.push('`');
                out.push_str(t);
                out.push('`');
            }
            MessageSpan::CodeBlock { fenced_source, .. } => {
                out.push_str("```");
                out.push_str(fenced_source);
                out.push_str("```");
            }
            MessageSpan::Link { text, .. } => out.push_str(text),
            MessageSpan::Mention { display, .. } => out.push_str(display),
            MessageSpan::Hashtag { display, .. } => out.push_str(display),
            MessageSpan::Emoji { name, .. } => out.push_str(name),
            MessageSpan::Canvas { .. } | MessageSpan::Heading { .. } => return None,
        }
    }
    (mezon_client::transport::strip_markdown(&out).text == content).then_some(out)
}

fn strip_marker(s: &str, marker: &str) -> String {
    let trimmed = s
        .strip_prefix(marker)
        .and_then(|r| r.strip_suffix(marker))
        .unwrap_or(s);
    trimmed.to_string()
}

pub fn strip_code_fence(s: &str) -> (Option<String>, String) {
    let mut body = s.trim();
    if let Some(rest) = body.strip_prefix("```") {
        body = rest;
        if let Some(rest) = body.strip_suffix("```") {
            body = rest;
        }
    }
    body = body.trim_matches('`');
    let body = body.trim();

    if let Some((first, rest)) = body.split_once('\n') {
        let rest = rest.trim_end();
        let candidate = first.trim();
        if !rest.is_empty() && is_code_fence_language(candidate) {
            return (Some(candidate.to_string()), rest.to_string());
        }
    }
    (None, body.to_string())
}

const CODE_FENCE_LANGUAGES: &[&str] = &[
    "c",
    "c++",
    "c#",
    "js",
    "ts",
    "py",
    "java",
    "javascript",
    "typescript",
    "python",
    "go",
    "rust",
    "kotlin",
    "sql",
    "html",
    "json",
    "css",
    "swift",
    "yaml",
    "php",
    "jsx",
    "bash",
];

fn is_code_fence_language(candidate: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }
    let lowered = candidate.to_ascii_lowercase();
    CODE_FENCE_LANGUAGES.contains(&lowered.as_str())
}

pub(crate) fn split_token_transaction(content: &str) -> TokenTransaction {
    match content.split_once(" | ") {
        Some((title, detail)) => TokenTransaction {
            title: title.trim().to_string().into(),
            detail: detail.trim().to_string().into(),
        },
        None => TokenTransaction {
            title: content.trim().to_string().into(),
            detail: SharedString::default(),
        },
    }
}

pub(crate) fn reply_preview_line(content: &str) -> String {
    const MAX_CHARS: usize = 120;
    let mut out = String::new();
    let mut chars = 0usize;
    let mut first = true;
    for word in content.split_whitespace() {
        if !first {
            out.push(' ');
            chars += 1;
        }
        first = false;
        for ch in word.chars() {
            if chars >= MAX_CHARS {
                return out;
            }
            out.push(ch);
            chars += 1;
        }
    }
    out
}

fn resolve_link_url(url: &str, text: &str) -> String {
    if !url.is_empty() {
        return url.to_string();
    }
    text.to_string()
}

fn rich_channel_id(raw: &str) -> Option<ChannelId> {
    raw.parse::<i64>()
        .ok()
        .map(ChannelId)
        .filter(|id| !id.is_zero())
}

pub fn build_rich_layout(spans: &[MessageSpan]) -> Option<Arc<RichLayout>> {
    if spans.is_empty() {
        return None;
    }
    let mut text = String::new();
    let mut runs: Vec<RichRun> = Vec::new();
    for span in spans {
        match span {
            MessageSpan::Text(t) => text.push_str(t),
            MessageSpan::Emoji { name, .. } => text.push_str(name),
            MessageSpan::Bold(t) => {
                let start = text.len();
                text.push_str(t);
                runs.push(RichRun {
                    range: start..text.len(),
                    kind: RichRunKind::Bold,
                    click: None,
                });
            }
            MessageSpan::Code(t) => {
                let start = text.len();
                text.push_str(t);
                runs.push(RichRun {
                    range: start..text.len(),
                    kind: RichRunKind::Code,
                    click: None,
                });
            }
            MessageSpan::Link { text: t, url, .. } => {
                let start = text.len();
                text.push_str(t);
                runs.push(RichRun {
                    range: start..text.len(),
                    kind: RichRunKind::Link,
                    click: Some(RichClick::Link(resolve_link_url(url, t).into())),
                });
            }
            MessageSpan::Mention {
                display,
                user_id,
                role_id,
            } => {
                let start = text.len();
                text.push_str(display);
                let is_role = role_id.as_deref().is_some_and(|r| !r.is_empty());
                let click = if is_role {
                    None
                } else {
                    user_id
                        .as_deref()
                        .filter(|u| !u.is_empty() && *u != "0" && !is_here_user_id(u))
                        .and_then(|u| u.parse::<i64>().ok())
                        .map(UserId)
                        .map(RichClick::Mention)
                };
                runs.push(RichRun {
                    range: start..text.len(),
                    kind: if is_role {
                        RichRunKind::RoleMention
                    } else {
                        RichRunKind::Mention
                    },
                    click,
                });
            }
            MessageSpan::Hashtag {
                display,
                channel_id,
            } => {
                let start = text.len();
                text.push_str(display);
                let click = channel_id
                    .as_deref()
                    .and_then(rich_channel_id)
                    .map(RichClick::Channel);
                runs.push(RichRun {
                    range: start..text.len(),
                    kind: RichRunKind::Hashtag,
                    click,
                });
            }
            MessageSpan::CodeBlock { .. } => {}
            MessageSpan::Canvas { title, .. } => text.push_str(title),
            MessageSpan::Heading { text: heading, .. } => text.push_str(heading),
        }
    }
    Some(Arc::new(RichLayout {
        text: text.into(),
        runs: runs.into(),
        content_tokens: build_content_tokens(spans),
    }))
}

fn build_content_tokens(spans: &[MessageSpan]) -> Option<Arc<[RichToken]>> {
    let needs_chip_path = spans.iter().any(|span| {
        matches!(
            span,
            MessageSpan::Hashtag { .. }
                | MessageSpan::Canvas { .. }
                | MessageSpan::Heading { .. }
                | MessageSpan::CodeBlock { .. }
        ) || matches!(span, MessageSpan::Link { kind, .. } if *kind != LinkKind::Plain)
            || matches!(span, MessageSpan::Emoji { emoji_id, .. } if !emoji_id.is_empty())
    });
    if !needs_chip_path {
        return None;
    }
    let mut tokens: Vec<RichToken> = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        match span {
            MessageSpan::Text(text) => {
                let mut first_line = true;
                for line in text.split('\n') {
                    if !first_line {
                        tokens.push(RichToken::LineBreak);
                    }
                    first_line = false;
                    for word in line.split_whitespace() {
                        tokens.push(RichToken::Word(SharedString::from(word.to_owned())));
                    }
                }
            }
            _ => tokens.push(RichToken::Span(index as u32)),
        }
    }
    Some(tokens.into())
}

pub fn recompute_message_grouping(messages: &mut [Message]) {
    for i in 0..messages.len() {
        let prev = if i > 0 { Some(&messages[i - 1]) } else { None };
        let combined = message_combined_with_prev(prev, &messages[i]);
        let show_forwarded_label = compute_show_forwarded_label(prev, &messages[i]);
        messages[i].combined_with_prev = combined;
        messages[i].show_forwarded_label = show_forwarded_label;
    }
}

pub(crate) fn compute_show_forwarded_label(prev: Option<&Message>, msg: &Message) -> bool {
    if !msg.is_forwarded {
        return false;
    }
    let Some(prev) = prev else {
        return true;
    };
    if !prev.is_forwarded {
        return true;
    }
    !same_message_sender(prev, msg) || (msg.create_time - prev.create_time).abs() > 600_000
}

pub fn message_sort_key(m: &Message) -> (u8, i64) {
    let not_first = u8::from(m.code != MessageCode::Indicator);
    (not_first, m.sort_id)
}

pub fn sort_messages(messages: &mut [Message]) {
    messages.sort_by_key(message_sort_key);
}

/// The 2x source size an emoji span is painted at: a message of only emoji
/// renders them jumbo, everything else inline. Resolved once here, when the
/// message is built, because the render path would otherwise rebuild the same
/// imgproxy URL for every emoji of every visible message on every frame.
const INLINE_EMOJI_SOURCE_PX: u32 = 48;
const JUMBO_EMOJI_SOURCE_PX: u32 = 96;

pub fn fill_emoji_sources(spans: &mut [MessageSpan], cfg: Option<&AppConfig>) {
    let Some(cfg) = cfg else {
        return;
    };
    let source_px = if spans_only_emoji(spans) {
        JUMBO_EMOJI_SOURCE_PX
    } else {
        INLINE_EMOJI_SOURCE_PX
    };
    for span in spans.iter_mut() {
        if let MessageSpan::Emoji { emoji_id, src, .. } = span
            && !emoji_id.is_empty()
        {
            *src = cfg.emoji_src_sized(emoji_id, source_px).into();
        }
    }
}

impl Message {
    pub fn new(
        id: MessageId,
        content: impl Into<String>,
        sender_id: impl Into<String>,
        sender_name: impl Into<SharedString>,
        create_time: i64,
    ) -> Self {
        let content: String = content.into();
        let spans = if content.is_empty() {
            Vec::new()
        } else {
            vec![MessageSpan::Text(content.as_str().into())]
        };
        let sender_id: String = sender_id.into();
        let sender_user_id = sender_id.parse::<i64>().ok().map(UserId);
        let rich_layout = build_rich_layout(&spans);
        Self {
            id,
            sort_id: id.get(),
            row_anchor_id: id,
            content,
            sender_id,
            sender_user_id,
            sender_name: sender_name.into(),
            avatar_url: SharedString::default(),
            avatar_proxied: SharedString::default(),
            create_time,
            update_time: 0,
            day_label: local_day_key(create_time),
            time_hhmm: format_local_time_hhmm(create_time).into(),
            local_date: local_datetime(create_time).map(|dt| dt.date_naive()),
            code: MessageCode::Chat,
            is_edited: false,
            is_forwarded: false,
            show_forwarded_label: false,
            combined_with_prev: false,
            highlights_viewer_direct: false,
            send_failed: false,
            ogp: None,
            poll: None,
            call_log: None,
            embeds: Vec::new().into(),
            components: Vec::new().into(),
            is_card: false,
            topic_id: None,
            topic_creator_id: None,
            invite: None,
            is_only_emoji: false,
            is_deleted_placeholder: false,
            spans,
            rich_layout,
            token_transaction: None,
            mention_targets: Vec::new(),
            references: Vec::new(),
            reactions: Vec::new(),
            attachments: Vec::new(),
            album_layout: None,
            viewer_media: Vec::new().into(),
            raw_content: None,
        }
    }

    pub fn with_sort_id(mut self, sort_id: i64) -> Self {
        self.sort_id = sort_id;
        self
    }

    pub fn with_raw_content(mut self, raw: &str) -> Self {
        self.raw_content = (!raw.is_empty()).then(|| Arc::from(raw));
        self
    }

    pub fn is_sending(&self) -> bool {
        (self.id.is_optimistic() || self.attachments.iter().any(|a| a.uploading))
            && !self.send_failed
    }

    /// `day_label`, `time_hhmm` and `local_date` are all derived from `create_time`, so they
    /// must only ever move as a unit — assigning `create_time` alone leaves the rendered
    /// timestamp describing the *previous* value (or blank, when the other value was 0).
    pub fn set_create_time(&mut self, create_time: i64) {
        self.create_time = create_time;
        self.day_label = local_day_key(create_time);
        self.time_hhmm = format_local_time_hhmm(create_time).into();
        self.local_date = local_datetime(create_time).map(|dt| dt.date_naive());
    }

    pub fn token_transaction_parts(&self) -> (SharedString, SharedString) {
        let tx = split_token_transaction(&self.content);
        (tx.title, tx.detail)
    }

    pub fn with_attachments(mut self, attachments: Vec<MessageAttachment>) -> Self {
        self.attachments = attachments;
        self
    }

    pub fn with_media_presentation(
        mut self,
        album_layout: Option<AlbumLayout>,
        viewer_media: Arc<[ViewerMedia]>,
    ) -> Self {
        self.album_layout = album_layout;
        self.viewer_media = viewer_media;
        self
    }

    pub fn with_code(mut self, code: MessageCode) -> Self {
        self.code = code;
        self
    }

    pub fn with_spans(mut self, spans: Vec<MessageSpan>) -> Self {
        self.rich_layout = build_rich_layout(&spans);
        self.is_only_emoji = spans_only_emoji(&spans);
        self.spans = spans;
        self
    }

    pub fn with_token_transaction(
        mut self,
        token_transaction: Option<Box<TokenTransaction>>,
    ) -> Self {
        self.token_transaction = token_transaction;
        self
    }

    pub fn with_mention_targets(mut self, mention_targets: Vec<MentionTarget>) -> Self {
        self.mention_targets = mention_targets;
        self
    }

    pub fn with_viewer_highlight(mut self, highlights_viewer_direct: bool) -> Self {
        self.highlights_viewer_direct = highlights_viewer_direct;
        self
    }

    pub fn with_references(mut self, references: Vec<MessageReference>) -> Self {
        self.references = references;
        self
    }

    pub fn with_reactions(mut self, reactions: Vec<Reaction>) -> Self {
        self.reactions = reactions;
        self
    }

    pub fn with_edited(mut self, update_time: i64, hide_editted: bool) -> Self {
        self.update_time = update_time;
        self.is_edited = update_time > 0 && update_time > self.create_time && !hide_editted;
        self
    }

    pub fn with_forwarded(mut self, forwarded: bool) -> Self {
        self.is_forwarded = forwarded;
        self
    }

    pub fn with_ogp(mut self, ogp: Option<Box<OgpPreview>>) -> Self {
        self.ogp = ogp;
        self
    }

    pub fn with_poll(mut self, poll: Option<Box<PollData>>) -> Self {
        self.poll = poll;
        self
    }

    pub fn with_call_log(mut self, call_log: Option<CallLog>) -> Self {
        self.call_log = call_log;
        self
    }

    pub fn with_embeds(mut self, embeds: Arc<[Embed]>) -> Self {
        self.embeds = embeds;
        self
    }

    pub fn with_components(mut self, components: Arc<[MessageComponentRow]>) -> Self {
        self.components = components;
        self
    }

    pub fn with_is_card(mut self, is_card: bool) -> Self {
        self.is_card = is_card;
        self
    }

    pub fn with_topic(
        mut self,
        topic_id: Option<ChannelId>,
        topic_creator_id: Option<UserId>,
    ) -> Self {
        self.topic_id = topic_id;
        self.topic_creator_id = topic_creator_id;
        self
    }

    pub fn with_invite(mut self, invite: Option<Box<InvitePreview>>) -> Self {
        self.invite = invite;
        self
    }

    pub fn with_only_emoji(mut self, is_only_emoji: bool) -> Self {
        self.is_only_emoji = is_only_emoji;
        self
    }

    pub fn with_deleted_placeholder(mut self, is_deleted_placeholder: bool) -> Self {
        self.is_deleted_placeholder = is_deleted_placeholder;
        self
    }

    pub fn with_avatar(mut self, avatar_url: impl Into<SharedString>) -> Self {
        self.avatar_url = avatar_url.into();
        self
    }

    pub fn with_avatar_proxied(mut self, proxied: impl Into<SharedString>) -> Self {
        self.avatar_proxied = proxied.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forwarded(id: i64, sender: &str, time: i64) -> Message {
        Message::new(MessageId(id), "m", sender, "U", time).with_forwarded(true)
    }

    #[test]
    fn forwarded_label_hidden_for_non_forwarded_message() {
        let msg = Message::new(MessageId(1), "m", "42", "U", 100);
        assert!(!compute_show_forwarded_label(None, &msg));
    }

    #[test]
    fn forwarded_label_shown_when_no_previous_row() {
        assert!(compute_show_forwarded_label(None, &forwarded(2, "42", 100)));
    }

    #[test]
    fn forwarded_label_shown_after_non_forwarded_previous() {
        let prev = Message::new(MessageId(1), "m", "42", "U", 100);
        assert!(compute_show_forwarded_label(
            Some(&prev),
            &forwarded(2, "42", 110)
        ));
    }

    #[test]
    fn forwarded_label_grouped_for_same_sender_burst() {
        let prev = forwarded(1, "42", 100);
        assert!(!compute_show_forwarded_label(
            Some(&prev),
            &forwarded(2, "42", 110)
        ));
    }

    #[test]
    fn forwarded_label_shown_for_different_sender() {
        let prev = forwarded(1, "42", 100);
        assert!(compute_show_forwarded_label(
            Some(&prev),
            &forwarded(2, "7", 110)
        ));
    }

    fn token(s: i64, e: i64) -> ContentToken {
        ContentToken {
            s: Some(s),
            e: Some(e),
            ..Default::default()
        }
    }

    #[test]
    fn edit_source_restores_stripped_markers_for_the_plain_text_composer() {
        let content = ApiMessageContent {
            t: "x @bob".into(),
            mk: vec![ContentToken {
                kind: Some("c".into()),
                ..token(0, 1)
            }],
            mentions: vec![ContentToken {
                user_id: Some("42".into()),
                username: Some("bob".into()),
                ..token(2, 6)
            }],
            ..Default::default()
        };
        let spans = parse_spans(&content);
        assert_eq!(
            markdown_edit_source(&content.t, &spans),
            Some("`x` @bob".to_string())
        );
    }

    fn edit_source_for_composer_text(raw: &str) -> (String, Option<String>) {
        let sent = mezon_client::transport::build_send_content(raw, &[], &[], &[]);
        let content: ApiMessageContent =
            serde_json::from_str(&sent.json).expect("send content json");
        let spans = parse_spans(&content);
        let source = markdown_edit_source(&content.t, &spans);
        (content.t.clone(), source)
    }

    #[test]
    fn code_fence_keeps_a_first_line_that_is_not_a_known_language() {
        for body in [
            "\na\nb\nc\n",
            "\nx\ny\n",
            "\nfoo\nbar()\n",
            "\nresult\nvalue\n",
        ] {
            let (language, text) = strip_code_fence(body);
            assert_eq!(language, None, "body {body:?} must have no language");
            assert_eq!(text, body.trim(), "body {body:?} must keep every line");
        }
    }

    #[test]
    fn code_fence_strips_only_a_whitelisted_language() {
        assert_eq!(
            strip_code_fence("rust\nlet x = 1;\n"),
            (Some("rust".into()), "let x = 1;".into())
        );
        assert_eq!(
            strip_code_fence("PYTHON\nprint(1)\n"),
            (Some("PYTHON".into()), "print(1)".into())
        );
        assert_eq!(
            strip_code_fence("rustacean\nlet x = 1;\n"),
            (None, "rustacean\nlet x = 1;".into())
        );
    }

    #[test]
    fn code_fence_keeps_a_lone_language_word_as_body() {
        assert_eq!(strip_code_fence("rust"), (None, "rust".into()));
    }

    #[test]
    fn edit_source_round_trips_a_bare_code_fence() {
        let raw = "```let x = 1;```";
        let (_, source) = edit_source_for_composer_text(raw);
        assert_eq!(source.as_deref(), Some(raw));
    }

    #[test]
    fn edit_source_round_trips_a_code_fence_with_a_language_tag() {
        let raw = "```rust\nlet x = 1;\n```";
        let (stored, source) = edit_source_for_composer_text(raw);
        assert!(
            source.is_some(),
            "editing must restore the fence; stored text was {stored:?}"
        );
    }

    #[test]
    fn edit_source_round_trips_a_multiline_code_fence() {
        let raw = "before\n```\nlet x = 1;\nfn main() {}\n```\nafter";
        let (stored, source) = edit_source_for_composer_text(raw);
        assert!(
            source.is_some(),
            "editing must restore the fence; stored text was {stored:?}"
        );
    }

    #[test]
    fn edit_source_keeps_text_that_already_carries_markers() {
        let content = ApiMessageContent {
            t: "`x`".into(),
            mk: vec![ContentToken {
                kind: Some("c".into()),
                ..token(0, 3)
            }],
            ..Default::default()
        };
        let spans = parse_spans(&content);
        assert_eq!(markdown_edit_source(&content.t, &spans), None);
    }

    #[test]
    fn edit_source_is_none_without_markdown() {
        let content = ApiMessageContent {
            t: "plain".into(),
            ..Default::default()
        };
        let spans = parse_spans(&content);
        assert_eq!(markdown_edit_source(&content.t, &spans), None);
    }

    #[test]
    fn parse_spans_plain_text() {
        let content = ApiMessageContent {
            t: "hello world".into(),
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![MessageSpan::Text("hello world".into())]
        );
    }

    #[test]
    fn parse_spans_classifies_a_bare_link_token_by_platform() {
        let url = "https://www.youtube.com/watch?v=lHW3fsJQ1sg";
        let content = ApiMessageContent {
            t: url.into(),
            lk: vec![token(0, url.len() as i64)],
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![MessageSpan::Link {
                text: url.into(),
                url: url.into(),
                kind: LinkKind::YouTube,
            }]
        );
    }

    #[test]
    fn parse_spans_keeps_a_link_token_plain_when_markdown_tokens_decide_the_kind() {
        let url = "https://www.youtube.com/watch?v=lHW3fsJQ1sg";
        let content = ApiMessageContent {
            t: url.into(),
            mk: vec![ContentToken {
                kind: Some("lk".into()),
                ..token(0, url.len() as i64)
            }],
            lk: vec![token(0, url.len() as i64)],
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![MessageSpan::Link {
                text: url.into(),
                url: url.into(),
                kind: LinkKind::Plain,
            }]
        );
    }

    #[test]
    fn parse_spans_classifies_a_link_token_by_its_target_not_its_display_text() {
        let text = "https://www.youtube.com/watch?v=lHW3fsJQ1sg";
        let content = ApiMessageContent {
            t: text.into(),
            lk: vec![ContentToken {
                url: Some("https://evil.example/".into()),
                ..token(0, text.len() as i64)
            }],
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![MessageSpan::Link {
                text: text.into(),
                url: "https://evil.example/".into(),
                kind: LinkKind::Plain,
            }],
            "the card brand must follow the url the click opens, not the text it shows"
        );
    }

    #[test]
    fn parse_spans_classifies_a_link_token_alongside_unrelated_markdown() {
        let text = "hi https://youtu.be/abc";
        let content = ApiMessageContent {
            t: text.into(),
            mk: vec![ContentToken {
                kind: Some("b".into()),
                ..token(0, 2)
            }],
            lk: vec![token(3, text.len() as i64)],
            ..Default::default()
        };
        let spans = parse_spans(&content);
        assert!(
            spans.iter().any(|span| matches!(
                span,
                MessageSpan::Link {
                    kind: LinkKind::YouTube,
                    ..
                }
            )),
            "an unrelated bold run must not downgrade the link to plain: {spans:?}"
        );
    }

    #[test]
    fn parse_spans_interleaves_mention_and_text() {
        let content = ApiMessageContent {
            t: "hi @bob !".into(),
            mentions: vec![ContentToken {
                user_id: Some("42".into()),
                username: Some("bob".into()),
                ..token(3, 7)
            }],
            ..Default::default()
        };
        let spans = parse_spans(&content);
        assert_eq!(
            spans,
            vec![
                MessageSpan::Text("hi ".into()),
                MessageSpan::Mention {
                    display: "@bob".into(),
                    user_id: Some("42".into()),
                    role_id: None,
                },
                MessageSpan::Text(" !".into()),
            ]
        );
    }

    #[test]
    fn parse_spans_keeps_role_id_when_token_carries_an_empty_username() {
        let content = ApiMessageContent {
            t: "hi @Admin".into(),
            mentions: vec![ContentToken {
                role_id: Some("9".into()),
                username: Some(String::new()),
                ..token(3, 9)
            }],
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![
                MessageSpan::Text("hi ".into()),
                MessageSpan::Mention {
                    display: "@Admin".into(),
                    user_id: None,
                    role_id: Some("9".into()),
                },
            ]
        );
    }

    #[test]
    fn parse_spans_prefers_user_when_a_role_token_carries_a_real_username() {
        let content = ApiMessageContent {
            t: "hi @Admin".into(),
            mentions: vec![ContentToken {
                role_id: Some("9".into()),
                username: Some("@Admin".into()),
                ..token(3, 9)
            }],
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![
                MessageSpan::Text("hi ".into()),
                MessageSpan::Mention {
                    display: "@Admin".into(),
                    user_id: None,
                    role_id: None,
                },
            ]
        );
    }

    #[test]
    fn parse_spans_reads_role_token_without_username() {
        let content: ApiMessageContent =
            serde_json::from_str(r#"{"t":"hi @Admin","mentions":[{"s":3,"e":9,"role_id":9}]}"#)
                .expect("role mention token json");
        assert_eq!(
            parse_spans(&content),
            vec![
                MessageSpan::Text("hi ".into()),
                MessageSpan::Mention {
                    display: "@Admin".into(),
                    user_id: None,
                    role_id: Some("9".into()),
                },
            ]
        );
    }

    #[test]
    fn parse_spans_keeps_username_only_mention_as_user() {
        let content = ApiMessageContent {
            t: "hi @bob".into(),
            mentions: vec![ContentToken {
                username: Some("bob".into()),
                ..token(3, 7)
            }],
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![
                MessageSpan::Text("hi ".into()),
                MessageSpan::Mention {
                    display: "@bob".into(),
                    user_id: None,
                    role_id: None,
                },
            ]
        );
    }

    #[test]
    fn parse_spans_treats_idless_empty_username_mention_as_plain_text() {
        let content = ApiMessageContent {
            t: "hi @here".into(),
            mentions: vec![ContentToken {
                user_id: Some("0".into()),
                username: Some(String::new()),
                ..token(3, 8)
            }],
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![
                MessageSpan::Text("hi ".into()),
                MessageSpan::Text("@here".into())
            ]
        );
    }

    #[test]
    fn parse_spans_strips_bold_markers() {
        let content = ApiMessageContent {
            t: "**hey**".into(),
            mk: vec![ContentToken {
                kind: Some("b".into()),
                ..token(0, 7)
            }],
            ..Default::default()
        };
        assert_eq!(parse_spans(&content), vec![MessageSpan::Bold("hey".into())]);
    }

    #[test]
    fn parse_spans_accepts_numeric_user_id_from_json() {
        let content: ApiMessageContent = serde_json::from_str(
            r#"{"t":"hi @bob","mentions":[{"s":3,"e":7,"user_id":42,"username":"bob"}]}"#,
        )
        .expect("mention token json");
        let spans = parse_spans(&content);
        assert_eq!(
            spans,
            vec![
                MessageSpan::Text("hi ".into()),
                MessageSpan::Mention {
                    display: "@bob".into(),
                    user_id: Some("42".into()),
                    role_id: None,
                },
            ]
        );
    }

    #[test]
    fn message_row_highlight_uses_entity_mention_targets() {
        let mut msg = Message::new(MessageId(1), "hi", "7", "alice", 100);
        msg.mention_targets = vec![MentionTarget {
            user_id: Some("42".into()),
            role_id: None,
            ..Default::default()
        }];
        assert!(message_row_highlight(&msg, Some(UserId(42)), &[]));
        assert!(!message_row_highlight(&msg, Some(UserId(7)), &[]));
    }

    #[test]
    fn message_row_highlight_detects_user_mention_and_reply() {
        let user = UserId(42);
        let mut msg = Message::new(MessageId(1), "hi", "7", "alice", 100);
        msg.spans = vec![MessageSpan::Mention {
            display: "@bob".into(),
            user_id: Some("42".into()),
            role_id: None,
        }];
        assert!(message_row_highlight(&msg, Some(user), &[]));

        let mut reply = Message::new(MessageId(2), "yo", "7", "alice", 101);
        reply.references.push(MessageReference {
            message_ref_id: MessageId(9),
            sender_id: user,
            ..Default::default()
        });
        assert!(message_row_highlight(&reply, Some(user), &[]));
    }

    #[test]
    fn parse_spans_handles_utf16_indices() {
        let content = ApiMessageContent {
            t: "😀 @x".into(),
            mentions: vec![ContentToken {
                user_id: Some("1".into()),
                ..token(3, 5)
            }],
            ..Default::default()
        };
        let spans = parse_spans(&content);
        assert_eq!(
            spans.last(),
            Some(&MessageSpan::Mention {
                display: "@x".into(),
                user_id: Some("1".into()),
                role_id: None,
            })
        );
    }

    #[test]
    fn aggregate_reactions_groups_by_emoji() {
        let raw = vec![
            ApiMessageReaction {
                emoji_id: "10".into(),
                emoji: ":a:".into(),
                count: 1,
                sender_id: "u1".into(),
                action: false,
            },
            ApiMessageReaction {
                emoji_id: "10".into(),
                emoji: ":a:".into(),
                count: 1,
                sender_id: "u2".into(),
                action: false,
            },
            ApiMessageReaction {
                emoji_id: "20".into(),
                emoji: ":b:".into(),
                count: 1,
                sender_id: "u1".into(),
                action: true,
            },
        ];
        let agg = aggregate_reactions(&raw);
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].key, "10");
        assert_eq!(agg[0].count(), 2);
        assert_eq!(agg[0].count_label, "2");
        let senders: Vec<&str> = agg[0]
            .senders
            .iter()
            .map(|s| s.sender_id.as_str())
            .collect();
        assert_eq!(senders, vec!["u1", "u2"]);
    }

    #[test]
    fn apply_reaction_add_increments_same_sender_count() {
        let mut reactions = vec![Reaction {
            key: "10".into(),
            emoji: ":a:".into(),
            emoji_id: "10".into(),
            count: 1,
            senders: vec![ReactionSender {
                sender_id: "u1".into(),
                count: 1,
            }],
            ..Default::default()
        }];
        apply_reaction_event(&mut reactions, "10", ":a:", "u2", false);
        assert_eq!(reactions[0].count(), 2);
        apply_reaction_event(&mut reactions, "10", ":a:", "u1", false);
        assert_eq!(reactions[0].count(), 3);
        assert!(reactions[0].has_sender("u1"));
        apply_reaction_event(&mut reactions, "10", ":a:", "u1", true);
        assert_eq!(reactions[0].count(), 1);
        assert!(!reactions[0].has_sender("u1"));
        apply_reaction_event(&mut reactions, "10", ":a:", "u2", true);
        assert!(reactions.is_empty());
    }

    #[test]
    fn rollback_reaction_undoes_optimistic_add() {
        let mut reactions = Vec::new();
        apply_reaction_event(&mut reactions, "10", ":a:", "u1", false);
        apply_reaction_event(&mut reactions, "10", ":a:", "u1", false);
        assert_eq!(reactions[0].count(), 2);
        rollback_reaction(&mut reactions, "10", ":a:", "u1", false);
        assert_eq!(reactions[0].count(), 1);
        rollback_reaction(&mut reactions, "10", ":a:", "u1", false);
        assert!(reactions.is_empty());
    }

    #[test]
    fn format_reaction_count_matches_react() {
        assert_eq!(format_reaction_count(0), "0");
        assert_eq!(format_reaction_count(999), "999");
        assert_eq!(format_reaction_count(1000), "1.0K");
        assert_eq!(format_reaction_count(1500), "1.5K");
        assert_eq!(format_reaction_count(1_000_000), "1.0M");
    }

    #[test]
    fn user_message_after_system_from_same_sender_combines() {
        let mut sys = Message::new(MessageId(1), "thread", "u1", "U1", 100);
        sys.code = MessageCode::CreateThread;
        let next = Message::new(MessageId(2), "hello", "u1", "U1", 110);
        assert!(message_combined_with_prev(Some(&sys), &next));
    }

    #[test]
    fn system_message_never_combines() {
        let prev = Message::new(MessageId(1), "a", "u1", "U1", 100);
        let mut sys = Message::new(MessageId(2), "joined", "u1", "U1", 110);
        sys.code = MessageCode::Welcome;
        assert!(!message_combined_with_prev(Some(&prev), &sys));
    }

    #[test]
    fn same_sender_matches_via_sender_user_id_when_ack_sender_id_is_zero() {
        let mut ack = Message::new(MessageId(1), "a", "0", "U1", 100);
        ack.sender_user_id = Some(UserId(42));
        let mut next = Message::new(MessageId(2), "b", "42", "U1", 105);
        next.sender_user_id = Some(UserId(42));
        assert!(same_message_sender(&ack, &next));
        assert!(message_combined_with_prev(Some(&ack), &next));
    }

    #[test]
    fn reply_message_still_shows_head_when_combined() {
        let prev = Message::new(MessageId(1), "a", "u1", "U1", 100);
        let mut reply = Message::new(MessageId(2), "b", "u1", "U1", 110);
        reply.references.push(MessageReference::default());
        assert!(message_combined_with_prev(Some(&prev), &reply));
        assert!(should_show_message_head(&reply, true));
    }

    #[test]
    fn topic_message_can_combine_with_chat() {
        let prev = Message::new(MessageId(1), "a", "u1", "U1", 100);
        let mut topic = Message::new(MessageId(2), "b", "u1", "U1", 110);
        topic.code = MessageCode::Topic;
        assert!(message_combined_with_prev(Some(&prev), &topic));
    }

    #[test]
    fn ack_server_time_ahead_of_next_optimistic_still_combines() {
        let ack = Message::new(MessageId(1), "a", "42", "U1", 105);
        let optimistic = Message::new(MessageId::next_optimistic(), "b", "42", "U1", 101);
        assert!(message_combined_with_prev(Some(&ack), &optimistic));
    }

    #[test]
    fn sparse_sender_id_zero_does_not_match_real_user() {
        let sparse = Message::new(MessageId(1), "a", "0", "U1", 100);
        let mine = Message::new(MessageId(2), "b", "42", "U1", 110);
        assert!(!same_message_sender(&sparse, &mine));
    }

    fn attachment(filetype: &str, url: &str) -> MessageAttachment {
        MessageAttachment {
            filetype: filetype.into(),
            url: url.into(),
            ..Default::default()
        }
    }

    const STICKER_IMGPROXY_URL: &str = "https://imgproxy.mezon.ai/K0YUZRIosDOcz5lY6qrgC6UIXmQgWzLjZv7VJ1RAA8c/rs:fit:100:100:1/mb:2097152/plain/https://cdn.mezon.ai/stickers/2039232970027438080.webp@webp";

    #[test]
    fn sticker_filetype_is_an_image_not_a_document() {
        assert!(attachment(STICKER_FILETYPE, STICKER_IMGPROXY_URL).is_image());
        assert!(attachment(STICKER_FILETYPE, "https://cdn.mezon.ai/stickers/1.webp").is_image());
        assert!(attachment(STICKER_FILETYPE, "").is_image());
    }

    #[test]
    fn imgproxy_output_format_is_read_as_the_extension() {
        assert_eq!(url_extension(STICKER_IMGPROXY_URL).as_deref(), Some("webp"));
        assert!(attachment("", STICKER_IMGPROXY_URL).is_image());
    }

    #[test]
    fn svg_is_a_file_row_not_an_image() {
        assert!(!attachment("image/svg+xml", "https://cdn.example/logo.svg").is_image());
        assert!(!attachment("", "https://cdn.example/logo.svg").is_image());
        assert!(!attachment("image", "https://cdn.example/logo.svg").is_image());
    }

    #[test]
    fn heic_and_bare_image_prefix_are_images() {
        assert!(attachment("image/heic", "https://cdn.example/a.heic").is_image());
        assert!(attachment("image", "https://cdn.example/a").is_image());
    }

    #[test]
    fn url_extension_handles_plain_query_and_extensionless_urls() {
        assert_eq!(
            url_extension("https://cdn.example/a/b/photo.PNG").as_deref(),
            Some("png")
        );
        assert_eq!(
            url_extension("https://cdn.example/photo.jpg?w=1#frag").as_deref(),
            Some("jpg")
        );
        assert_eq!(
            url_extension("https://cdn.example/asset@2x.png").as_deref(),
            Some("png")
        );
        assert_eq!(
            url_extension("https://cdn.example/no-extension").as_deref(),
            Some("no-extension")
        );
        assert!(!attachment("", "https://cdn.example/no-extension").is_image());
    }

    #[test]
    fn is_video_detects_mp4_and_quicktime() {
        assert!(attachment("video/mp4", "https://cdn.example/clip.mp4").is_video());
        assert!(attachment("video/quicktime", "https://cdn.example/clip.mov").is_video());
    }

    #[test]
    fn is_video_matches_video_prefix() {
        assert!(attachment("video/webm", "https://cdn.example/clip.webm").is_video());
    }

    #[test]
    fn is_video_excludes_mpeg_ts_stream() {
        assert!(!attachment("video/vnd.dlna.mpeg-tts", "https://cdn.example/x.ts").is_video());
    }

    #[test]
    fn is_video_false_for_image_and_bare_url() {
        assert!(!attachment("image/png", "https://cdn.example/x.png").is_video());
        assert!(!attachment("", "https://cdn.example/x.mp4").is_video());
    }

    #[test]
    fn tenor_gif_url_derives_mp4_variant() {
        assert_eq!(
            tenor_mp4_url(
                "https://media.tenor.com/rmtqGXO15tYAAAAC/may-day-flowers-happy-may-day.gif"
            )
            .as_deref(),
            Some("https://media.tenor.com/rmtqGXO15tYAAAPo/may-day-flowers-happy-may-day.mp4")
        );
        assert_eq!(
            tenor_mp4_url("https://media.tenor.com/lfDATg4Bhc0AAAAM/happy-cat.gif").as_deref(),
            Some("https://media.tenor.com/lfDATg4Bhc0AAAPo/happy-cat.mp4")
        );
    }

    #[test]
    fn tenor_mp4_url_rejects_non_tenor_and_malformed() {
        assert_eq!(tenor_mp4_url("https://cdn.example/uploaded.gif"), None);
        assert_eq!(
            tenor_mp4_url("https://media.tenor.com/rmtqGXO15tYAAAAC/clip.mp4"),
            None
        );
        assert_eq!(
            tenor_mp4_url("https://media.tenor.com/short/clip.gif"),
            None
        );
        assert_eq!(
            tenor_mp4_url("https://media.tenor.com/rmtqGXO15tYAAAAC/.gif"),
            None
        );
    }

    #[test]
    fn delete_thread_maps_from_raw_and_classifies_as_system() {
        assert_eq!(MessageCode::from_raw(19), MessageCode::DeleteThread);
        assert!(MessageCode::DeleteThread.is_system());
        assert!(!MessageCode::DeleteThread.is_user_timeline());
    }

    #[test]
    fn is_audio_true_for_audio_mime_false_for_unsupported_and_others() {
        assert!(attachment("audio/mpeg", "https://cdn.example/x.mp3").is_audio());
        assert!(!attachment("audio/wma", "https://cdn.example/x.wma").is_audio());
        assert!(!attachment("image/png", "https://cdn.example/x.png").is_audio());
    }

    #[test]
    fn format_file_size_matches_react_units() {
        assert_eq!(format_file_size(500), "500 bytes");
        assert_eq!(format_file_size(1500), "1.5 kB");
        assert_eq!(format_file_size(2_500_000), "2.5 MB");
    }

    fn link_token(url: &str, s: i64, e: i64) -> ContentToken {
        ContentToken {
            url: Some(url.into()),
            ..token(s, e)
        }
    }

    #[test]
    fn parse_spans_vk_token_renders_as_link() {
        let content = ApiMessageContent {
            t: "join vc".into(),
            vk: vec![link_token("https://mezon.ai/vc", 0, 7)],
            ..Default::default()
        };
        assert!(matches!(
            parse_spans(&content).as_slice(),
            [MessageSpan::Link { .. }]
        ));
    }

    #[test]
    fn parse_spans_channel_url_becomes_hashtag() {
        let url = "https://mezon.ai/chat/clans/1234567890123456789/channels/9876543210987654321";
        let content = ApiMessageContent {
            t: "here".into(),
            lk: vec![link_token(url, 0, 4)],
            ..Default::default()
        };
        assert!(matches!(
            parse_spans(&content).as_slice(),
            [MessageSpan::Hashtag {
                channel_id: Some(id),
                ..
            }] if id == "9876543210987654321"
        ));
    }

    #[test]
    fn parse_spans_heading_line_becomes_heading() {
        let content = ApiMessageContent {
            t: "# Title".into(),
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![MessageSpan::Heading {
                level: 1,
                text: "Title".into()
            }]
        );
    }

    #[test]
    fn parse_spans_canvas_url_with_cvtt_title_becomes_canvas() {
        let url = "https://mezon.ai/chat/clans/1234567890123456789/channels/9876543210987654321/canvas/abc";
        let mut cvtt = HashMap::new();
        cvtt.insert("abc".to_string(), "My Canvas".to_string());
        let content = ApiMessageContent {
            t: "doc".into(),
            lk: vec![link_token(url, 0, 3)],
            cvtt,
            ..Default::default()
        };
        assert!(matches!(
            parse_spans(&content).as_slice(),
            [MessageSpan::Canvas { title, canvas_id, .. }]
                if title == "My Canvas" && canvas_id == "abc"
        ));
    }

    #[test]
    fn unsupported_media_takes_precedence_over_video_and_image() {
        let avi = attachment("video/avi", "https://cdn.example/x.avi");
        assert!(avi.is_unsupported_media());
        assert!(avi.is_video());

        let bmp = attachment("image/bmp", "https://cdn.example/x.bmp");
        assert!(bmp.is_unsupported_media());
        assert!(bmp.is_image());

        let uploaded_bmp = attachment("image", "https://cdn.example/1234.bmp");
        assert!(uploaded_bmp.is_unsupported_media());

        let uploaded_wmv = attachment("video", "https://cdn.example/1234.wmv");
        assert!(uploaded_wmv.is_unsupported_media());
    }

    #[test]
    fn supported_video_and_image_are_not_unsupported() {
        let mp4 = attachment("video/mp4", "https://cdn.example/x.mp4");
        assert!(!mp4.is_unsupported_media());
        assert!(mp4.is_video());

        let png = attachment("image/png", "https://cdn.example/x.png");
        assert!(!png.is_unsupported_media());
        assert!(png.is_image());
    }

    #[test]
    fn matroska_video_is_unsupported_where_the_platform_cannot_demux_it() {
        // GStreamer reads Matroska; AVFoundation and Media Foundation do not.
        let expected = !cfg!(target_os = "linux");

        let webm = attachment("video/webm", "https://cdn.example/x.webm");
        assert_eq!(webm.is_unsupported_media(), expected);

        // A browser recorder writes `video/matroska`, and the web client uploads
        // the bare "video" category instead of a MIME, so the extension has to
        // carry the decision on its own.
        let matroska = attachment("video/matroska", "https://cdn.example/1234.webm");
        assert_eq!(matroska.is_unsupported_media(), expected);
        let uploaded = attachment("video", "https://cdn.example/1234.webm");
        assert_eq!(uploaded.is_unsupported_media(), expected);
    }

    #[test]
    fn webm_voice_messages_stay_playable_audio() {
        // Voice messages are WebM/Opus decoded in-app by symphonia, not by the
        // platform video player, so the container gate must not swallow them.
        let voice = attachment("audio/webm", "https://cdn.example/1234.webm");
        assert!(!voice.is_unsupported_media());
        assert!(voice.is_audio());
    }

    #[test]
    fn message_precomputes_local_day_key() {
        let ts = 1_609_459_200 + 48_300;
        let msg = Message::new(MessageId(1), "hi", "u", "User", ts);
        assert_eq!(msg.day_label, crate::message_time::local_day_key(ts));
    }

    #[test]
    fn sender_user_id_parsed_from_numeric_sender_id() {
        let msg = Message::new(MessageId(10), "hi", "42", "Alice", 0);
        assert_eq!(msg.sender_id, "42");
        assert_eq!(msg.sender_user_id, Some(UserId(42)));
    }

    #[test]
    fn sender_user_id_none_for_non_numeric_sender_id() {
        let msg = Message::new(MessageId::next_optimistic(), "hi", "u1", "Bob", 0);
        assert_eq!(msg.sender_id, "u1");
        assert_eq!(msg.sender_user_id, None);
    }

    #[test]
    fn sender_user_id_none_for_optimistic_temp_sender() {
        let msg = Message::new(
            MessageId::next_optimistic(),
            "hi",
            "temp-user",
            "Charlie",
            0,
        );
        assert_eq!(msg.sender_user_id, None);
    }

    #[test]
    fn optimistic_id_is_optimistic_real_id_is_not() {
        let opt = MessageId::next_optimistic();
        let real = MessageId(1_000_000_000_000_i64);
        assert!(opt.is_optimistic());
        assert!(!real.is_optimistic());
    }

    #[test]
    fn optimistic_ids_sort_after_real_ids() {
        let opt = MessageId::next_optimistic();
        let real = MessageId(i64::MAX / 2);
        assert!(real < opt);
    }

    #[test]
    fn optimistic_ids_are_unique_and_monotonic() {
        let a = MessageId::next_optimistic();
        let b = MessageId::next_optimistic();
        assert_ne!(a, b);
        assert!(a < b);
        assert!(a.is_optimistic() && b.is_optimistic());
    }

    #[test]
    fn cursor_guard_skips_optimistic_ids() {
        let optimistic = MessageId::next_optimistic();
        assert!(Some(optimistic).filter(|id| !id.is_optimistic()).is_none());
        let real = MessageId(123);
        assert_eq!(
            Some(real).filter(|id| !id.is_optimistic()),
            Some(MessageId(123))
        );
    }

    #[test]
    fn rich_layout_mention_text_matches_concatenation_and_marks_range() {
        let spans = vec![
            MessageSpan::Text("hi ".into()),
            MessageSpan::Mention {
                display: "@bob".into(),
                user_id: Some("42".into()),
                role_id: None,
            },
            MessageSpan::Text(" !".into()),
        ];
        let layout = build_rich_layout(&spans).expect("rich layout");
        assert_eq!(layout.text.as_ref(), "hi @bob !");
        assert_eq!(layout.runs.len(), 1);
        let run = &layout.runs[0];
        assert_eq!(run.kind, RichRunKind::Mention);
        assert_eq!(run.range, 3..7);
        assert_eq!(&layout.text[run.range.clone()], "@bob");
        assert_eq!(run.click, Some(RichClick::Mention(UserId(42))));
    }

    #[test]
    fn rich_layout_tags_role_mentions_apart_from_user_mentions() {
        let spans = vec![MessageSpan::Mention {
            display: "@Everyone".into(),
            user_id: None,
            role_id: Some("1841396057137745920".into()),
        }];
        let layout = build_rich_layout(&spans).expect("rich layout");
        assert_eq!(layout.runs.len(), 1);
        assert_eq!(layout.runs[0].kind, RichRunKind::RoleMention);
        assert_eq!(layout.runs[0].click, None);
    }

    #[test]
    fn rich_layout_link_and_code_carry_kind_and_click() {
        let spans = vec![
            MessageSpan::Code("x".into()),
            MessageSpan::Text(" ".into()),
            MessageSpan::Link {
                text: "site".into(),
                url: "https://mezon.ai".into(),
                kind: LinkKind::Plain,
            },
        ];
        let layout = build_rich_layout(&spans).expect("rich layout");
        assert_eq!(layout.text.as_ref(), "x site");
        assert_eq!(layout.runs.len(), 2);
        assert_eq!(layout.runs[0].kind, RichRunKind::Code);
        assert!(layout.runs[0].click.is_none());
        assert_eq!(layout.runs[1].kind, RichRunKind::Link);
        assert_eq!(
            layout.runs[1].click,
            Some(RichClick::Link("https://mezon.ai".into()))
        );
    }

    #[test]
    fn rich_layout_none_for_empty_spans() {
        assert!(build_rich_layout(&[]).is_none());
    }

    #[test]
    fn inbox_spans_from_raw_parses_code_block() {
        let raw = r#"{"t":"intro\n```\n#680066 line\n```","mk":[{"s":7,"e":26,"type":"t"}]}"#;
        let spans = inbox_spans_from_raw(raw).expect("spans");
        assert!(
            spans
                .iter()
                .any(|span| matches!(span, MessageSpan::CodeBlock { .. }))
        );
    }
}
