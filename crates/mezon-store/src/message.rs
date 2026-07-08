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
        self.filetype.starts_with("image/")
            || matches!(
                self.url
                    .split(['?', '#'])
                    .next()
                    .and_then(|u| u.rsplit('.').next())
                    .map(|ext| ext.to_ascii_lowercase())
                    .as_deref(),
                Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif")
            )
    }

    pub fn is_video(&self) -> bool {
        ((self.filetype.contains("video/mp4") || self.filetype.contains("video/quicktime"))
            && !self.url.contains("tenor.com"))
            || (self.filetype.starts_with("video") && !self.filetype.ends_with("vnd.dlna.mpeg-tts"))
    }

    pub fn is_unsupported_media(&self) -> bool {
        matches!(
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
    pub emoji_proxied: SharedString,
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

    fn refresh(&mut self, cfg: Option<&AppConfig>) {
        self.count = self.senders.iter().map(|s| s.count).sum();
        self.count_label = format_reaction_count(self.count).into();
        self.emoji_proxied = cfg
            .map(|c| c.emoji_src(&self.emoji_id))
            .unwrap_or_default()
            .into();
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedField {
    pub name: SharedString,
    pub value: SharedString,
    pub inline: bool,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenTransaction {
    pub title: SharedString,
    pub detail: SharedString,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: MessageId,
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

pub fn aggregate_reactions(raw: &[ApiMessageReaction], cfg: Option<&AppConfig>) -> Vec<Reaction> {
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
        r.refresh(cfg);
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
    cfg: Option<&AppConfig>,
) {
    let key = reaction_key(emoji_id, emoji);
    if key.is_empty() {
        return;
    }
    if removed {
        if let Some(pos) = reactions.iter().position(|x| x.key == key) {
            reactions[pos].senders.retain(|s| s.sender_id != sender_id);
            reactions[pos].refresh(cfg);
            if reactions[pos].count == 0 {
                reactions.remove(pos);
            }
        }
    } else if let Some(rec) = reactions.iter_mut().find(|x| x.key == key) {
        upsert_sender(&mut rec.senders, sender_id, 1, false);
        rec.refresh(cfg);
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
        rec.refresh(cfg);
        reactions.push(rec);
    }
}

pub fn rollback_reaction(
    reactions: &mut Vec<Reaction>,
    emoji_id: &str,
    emoji: &str,
    sender_id: &str,
    was_remove: bool,
    cfg: Option<&AppConfig>,
) {
    if was_remove {
        apply_reaction_event(reactions, emoji_id, emoji, sender_id, false, cfg);
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
    reactions[pos].refresh(cfg);
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
    collect(&content.lk, Kind::Link(LinkKind::Plain), &mut toks);
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
                let is_user = tok
                    .user_id
                    .as_deref()
                    .is_some_and(|u| u != "0" && !u.is_empty())
                    || tok.username.is_some();
                if is_user {
                    spans.push(MessageSpan::Mention {
                        display: inner.into(),
                        user_id: tok.user_id.clone(),
                        role_id: None,
                    });
                } else if tok.role_id.as_deref().is_some_and(|r| !r.is_empty()) {
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
                    "t" | "pre" => spans.push(MessageSpan::CodeBlock {
                        language: None,
                        text: strip_marker(&inner, "```").into(),
                    }),
                    "lk" | "vk" | "lk_yt" | "lk_fb" | "lk_tt" => {
                        let url = tok.url.clone().unwrap_or_else(|| inner.clone());
                        spans.push(resolve_link_span(
                            inner.into(),
                            url,
                            link_kind_from_marker(ty),
                            &content.cvtt,
                        ));
                    }
                    "lk_ogp" => {}
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

fn link_kind_from_marker(marker: &str) -> LinkKind {
    match marker {
        "lk_yt" => LinkKind::YouTube,
        "lk_fb" => LinkKind::Facebook,
        "lk_tt" => LinkKind::TikTok,
        _ => LinkKind::Plain,
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

fn strip_marker(s: &str, marker: &str) -> String {
    let trimmed = s
        .strip_prefix(marker)
        .and_then(|r| r.strip_suffix(marker))
        .unwrap_or(s);
    trimmed.to_string()
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
                let click = if role_id.as_deref().is_none_or(str::is_empty) {
                    user_id
                        .as_deref()
                        .filter(|u| !u.is_empty() && *u != "0" && !is_here_user_id(u))
                        .and_then(|u| u.parse::<i64>().ok())
                        .map(UserId)
                        .map(RichClick::Mention)
                } else {
                    None
                };
                runs.push(RichRun {
                    range: start..text.len(),
                    kind: RichRunKind::Mention,
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
    }))
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

fn compute_show_forwarded_label(prev: Option<&Message>, msg: &Message) -> bool {
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
    (not_first, m.id.get())
}

pub fn sort_messages(messages: &mut [Message]) {
    messages.sort_by_key(message_sort_key);
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
        }
    }

    pub fn is_sending(&self) -> bool {
        (self.id.is_optimistic() || self.attachments.iter().any(|a| a.uploading))
            && !self.send_failed
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
        let agg = aggregate_reactions(&raw, None);
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
        apply_reaction_event(&mut reactions, "10", ":a:", "u2", false, None);
        assert_eq!(reactions[0].count(), 2);
        apply_reaction_event(&mut reactions, "10", ":a:", "u1", false, None);
        assert_eq!(reactions[0].count(), 3);
        assert!(reactions[0].has_sender("u1"));
        apply_reaction_event(&mut reactions, "10", ":a:", "u1", true, None);
        assert_eq!(reactions[0].count(), 1);
        assert!(!reactions[0].has_sender("u1"));
        apply_reaction_event(&mut reactions, "10", ":a:", "u2", true, None);
        assert!(reactions.is_empty());
    }

    #[test]
    fn rollback_reaction_undoes_optimistic_add() {
        let mut reactions = Vec::new();
        apply_reaction_event(&mut reactions, "10", ":a:", "u1", false, None);
        apply_reaction_event(&mut reactions, "10", ":a:", "u1", false, None);
        assert_eq!(reactions[0].count(), 2);
        rollback_reaction(&mut reactions, "10", ":a:", "u1", false, None);
        assert_eq!(reactions[0].count(), 1);
        rollback_reaction(&mut reactions, "10", ":a:", "u1", false, None);
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

    #[test]
    fn is_video_detects_mp4_and_quicktime() {
        assert!(attachment("video/mp4", "https://cdn.mezon.ai/clip.mp4").is_video());
        assert!(attachment("video/quicktime", "https://cdn.mezon.ai/clip.mov").is_video());
    }

    #[test]
    fn is_video_matches_video_prefix() {
        assert!(attachment("video/webm", "https://cdn.mezon.ai/clip.webm").is_video());
    }

    #[test]
    fn is_video_excludes_mpeg_ts_stream() {
        assert!(!attachment("video/vnd.dlna.mpeg-tts", "https://cdn.mezon.ai/x.ts").is_video());
    }

    #[test]
    fn is_video_false_for_image_and_bare_url() {
        assert!(!attachment("image/png", "https://cdn.mezon.ai/x.png").is_video());
        assert!(!attachment("", "https://cdn.mezon.ai/x.mp4").is_video());
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
        assert_eq!(tenor_mp4_url("https://cdn.mezon.ai/uploaded.gif"), None);
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
        assert!(attachment("audio/mpeg", "https://cdn.mezon.ai/x.mp3").is_audio());
        assert!(!attachment("audio/wma", "https://cdn.mezon.ai/x.wma").is_audio());
        assert!(!attachment("image/png", "https://cdn.mezon.ai/x.png").is_audio());
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
        let avi = attachment("video/avi", "https://cdn.mezon.ai/x.avi");
        assert!(avi.is_unsupported_media());
        assert!(avi.is_video());

        let bmp = attachment("image/bmp", "https://cdn.mezon.ai/x.bmp");
        assert!(bmp.is_unsupported_media());
        assert!(bmp.is_image());
    }

    #[test]
    fn supported_video_and_image_are_not_unsupported() {
        let mp4 = attachment("video/mp4", "https://cdn.mezon.ai/x.mp4");
        assert!(!mp4.is_unsupported_media());
        assert!(mp4.is_video());

        let png = attachment("image/png", "https://cdn.mezon.ai/x.png");
        assert!(!png.is_unsupported_media());
        assert!(png.is_image());
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
}
