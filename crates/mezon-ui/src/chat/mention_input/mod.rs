mod attachments;
mod recorder;
mod text_field;

use std::any::Any;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::router::{Route, Router};
use gpui::{
    AnyElement, App, Bounds, Context, DismissEvent, Div, Entity, EventEmitter, Focusable,
    FontWeight, HighlightStyle, Hsla, Image, ImageFormat, IntoElement, KeyBinding, MouseButton,
    PathPromptOptions, Pixels, Rgba, ScrollStrategy, SharedString, Stateful, StyledText,
    Subscription, Task, UniformListScrollHandle, Window, actions, canvas, deferred, div, img,
    prelude::*, px, uniform_list,
};
use mezon_client::transport::QUICK_MENU_TYPE_FLASH;
use mezon_store::{
    AccountEvent, AccountStore, AppConfig, AudioStore, BadgeService, Channel, ChannelEvent,
    ChannelId, ChannelList, ChannelMembersEvent, ChannelMembersStore, ClanId, ClanList,
    ClanMembersEvent, ClanMembersStore, ComposeDraft, ComposeStore, ComposeToken, ComposeTokenKind,
    DirectEvent, DirectMessageStore, Emoji, EmojiEvent, EmojiStore, GroupMembersEvent,
    GroupMembersStore, MENTION_HERE_USER_ID, MessageSpan, MessagesStore, OgpResult,
    OutgoingAttachment, OutgoingContent, OutgoingEmoji, OutgoingHashtag, OutgoingMention,
    OutgoingOgp, QuickMenuStore, RolesEvent, RolesStore, Settings, fetch_invite_preview, fetch_ogp,
    first_previewable_url, internal_invite_id,
};
use std::time::Duration;

use attachments::{
    AttachmentLimit, MAX_FILE_ATTACHMENTS, PendingAttachment, build_pending, mime_from_extension,
    validate_batch,
};
use recorder::{ActiveRecording, MIN_RECORDING_MILLIS, RecordTask, encode_recording};

use crate::app::shell::Shell;
use crate::chat::gif_sticker_emoji::{GifStickerEmojiEvent, GifStickerEmojiPopup, SubPanel};
use crate::chat::member_list::{
    MentionMemberRaw, MentionScope, ensure_mention_members_loaded, mention_direct_id,
    mention_member_pool, mention_private_channel, mention_role_clan, mention_scope,
};
use crate::chat::message::CreatePollModal;
use crate::chat::message::MessageBuzzModal;
use crate::chat::message::ShareLocationModal;
use crate::chat::role_style::role_fallback_color;
use crate::components::primitives::{Avatar, Icon, IconName, ToastKind};
use crate::image_cache::{
    AVATAR_ENTRY_MAX_BYTES, AVATAR_IMAGE_CACHE_BYTES, AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::text_utils::{normalize_search_string, push_search_normalized};
use text_field::{
    MentionFieldEvent, MentionInputField, MentionInputState, MentionSpan, MentionSpanKind,
    byte_offset_to_utf16,
};

const KEY_CONTEXT: &str = "MezonMention";
const RECORDING_COLOR: Rgba = Rgba {
    r: 0xdc as f32 / 255.,
    g: 0x26 as f32 / 255.,
    b: 0x26 as f32 / 255.,
    a: 1.0,
};
const MAX_SUGGESTIONS: usize = 10;
const MAX_EMOJI_CANDIDATES: usize = 20;
const MENTION_HERE_DISPLAY: &str = "@here";
const MENTION_HERE_NORM: &str = "@HERE";
const CONVERT_TO_FILE_THRESHOLD: usize = 3700;
const CONVERT_PREFIX_LEN: usize = 8;
const STREAM_MODE_DM: i32 = 4;
const MENTION_ROW_PX: f32 = 40.;
const MENTION_POPUP_MAX_PX: f32 = MENTION_ROW_PX * MAX_SUGGESTIONS as f32;

actions!(
    mezon_mention,
    [
        MentionAccept,
        MentionDismiss,
        ToggleAnonymous,
        OpenMessageBuzz
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", MentionAccept, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", MentionDismiss, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-enter", ToggleAnonymous, None),
        KeyBinding::new("ctrl-shift-enter", ToggleAnonymous, None),
        KeyBinding::new("ctrl-g", OpenMessageBuzz, None),
    ]);
    cx.on_action(|_: &ToggleAnonymous, cx: &mut App| {
        toggle_anonymous_shortcut(cx);
    });
    cx.on_action(|_: &OpenMessageBuzz, cx: &mut App| {
        let Some(window_handle) =
            crate::app::main_window::handle(cx).or_else(|| cx.active_window())
        else {
            return;
        };
        cx.defer(move |cx| {
            let _ = cx.update_window(window_handle, |_, window, cx| {
                open_message_buzz(window, cx);
            });
        });
    });
}

fn toggle_anonymous_shortcut(cx: &mut App) {
    if matches!(
        Router::global(cx).read(cx).route(),
        Route::DirectMessage { .. } | Route::Direct | Route::Friends
    ) {
        return;
    }
    if ClanList::global(cx)
        .read(cx)
        .active_clan_id
        .and_then(|clan_id| ClanList::global(cx).read(cx).clan(clan_id))
        .is_some_and(|clan| clan.prevent_anonymous)
    {
        return;
    }
    MessagesStore::global(cx).update(cx, |store, cx| store.toggle_anonymous_mode(cx));
}

fn open_message_buzz(window: &mut Window, cx: &mut App) {
    if MessagesStore::global(cx)
        .read(cx)
        .active_channel_id()
        .is_none()
    {
        return;
    }
    let locale = Settings::try_global(cx)
        .map(|settings| SharedString::from(settings.read(cx).language.clone()))
        .unwrap_or_else(|| SharedString::from("en"));
    if MessagesStore::global(cx).read(cx).is_anonymous_mode() {
        let message =
            SharedString::from(mezon_i18n::t(&locale, "common.cannotSendBuzzWithAnonymous"));
        Shell::global(cx).update(cx, |shell, cx| shell.info(message, cx));
        return;
    }
    MessageBuzzModal::open(locale, window, cx);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sigil {
    At,
    Hash,
    Colon,
    Slash,
}

impl Sigil {
    fn slot(self) -> usize {
        match self {
            Sigil::At => 0,
            Sigil::Hash => 1,
            Sigil::Colon => 2,
            Sigil::Slash => 3,
        }
    }

    fn category_title(self, locale: &str) -> SharedString {
        let key = match self {
            Sigil::At => "messageBox.mentionCategories.members",
            Sigil::Hash => "messageBox.mentionCategories.textChannels",
            Sigil::Colon => "messageBox.mentionCategories.emojiMatching",
            Sigil::Slash => "messageBox.mentionCategories.commands",
        };
        SharedString::from(mezon_i18n::t(locale, key).to_uppercase())
    }

    fn from_byte(b: u8) -> Option<Self> {
        match b {
            b'@' => Some(Self::At),
            b'#' => Some(Self::Hash),
            b':' => Some(Self::Colon),
            b'/' => Some(Self::Slash),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Mention { user_id: String, role_id: String },
    Hashtag { channel_id: String },
    Emoji { emoji_id: String },
}

impl TokenKind {
    fn span_kind(&self) -> MentionSpanKind {
        match self {
            TokenKind::Mention { .. } => MentionSpanKind::Mention,
            TokenKind::Hashtag { .. } => MentionSpanKind::Hashtag,
            TokenKind::Emoji { .. } => MentionSpanKind::Emoji,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommittedToken {
    kind: TokenKind,
    display: String,
    start: usize,
    end: usize,
}

impl CommittedToken {
    fn to_compose(&self) -> ComposeToken {
        let kind = match &self.kind {
            TokenKind::Mention { user_id, role_id } => ComposeTokenKind::Mention {
                user_id: user_id.clone(),
                role_id: role_id.clone(),
            },
            TokenKind::Hashtag { channel_id } => ComposeTokenKind::Hashtag {
                channel_id: channel_id.clone(),
            },
            TokenKind::Emoji { emoji_id } => ComposeTokenKind::Emoji {
                emoji_id: emoji_id.clone(),
            },
        };
        ComposeToken {
            kind,
            display: self.display.clone(),
            start: self.start,
            end: self.end,
        }
    }

    fn from_compose(token: ComposeToken) -> Self {
        let kind = match token.kind {
            ComposeTokenKind::Mention { user_id, role_id } => {
                TokenKind::Mention { user_id, role_id }
            }
            ComposeTokenKind::Hashtag { channel_id } => TokenKind::Hashtag { channel_id },
            ComposeTokenKind::Emoji { emoji_id } => TokenKind::Emoji { emoji_id },
        };
        Self {
            kind,
            display: token.display,
            start: token.start,
            end: token.end,
        }
    }
}

#[derive(Clone)]
struct ChannelSuggestRaw {
    channel_id: String,
    /// Same channel as `channel_id`, kept typed so a row can look its voice
    /// occupancy up at render time. The pool outlives every join and leave —
    /// it is only rebuilt when the channel list itself reloads — so a `(busy)`
    /// flag baked in here would keep claiming whatever was true when you first
    /// typed `#` in this channel.
    id: ChannelId,
    clan_id: ClanId,
    name: String,
    name_lc: String,
    name_norm: String,
    sub_text: String,
    sub_text_norm: String,
}

#[derive(Clone)]
struct RoleSuggestRaw {
    role_id: String,
    title: String,
    title_lc: String,
    title_norm: String,
    icon_src: SharedString,
    color: Hsla,
}

#[derive(Clone)]
struct EmojiSuggestRaw {
    emoji_id: String,
    shortname: String,
    shortname_lc: String,
}

#[derive(Clone)]
struct SlashCommandRaw {
    id: SharedString,
    display: SharedString,
    display_lc: String,
    description: SharedString,
    action_msg: Option<SharedString>,
}

#[derive(Clone)]
enum Suggestion {
    Here,
    Member(Rc<MentionMemberRaw>, SharedString),
    Role(Rc<RoleSuggestRaw>),
    Channel(Rc<ChannelSuggestRaw>),
    Emoji(Rc<EmojiSuggestRaw>, SharedString),
    SlashCommand(Rc<SlashCommandRaw>),
}

impl Suggestion {
    fn group_order(&self) -> u8 {
        match self {
            Suggestion::Role(_) => 0,
            Suggestion::Member(..)
            | Suggestion::Channel(_)
            | Suggestion::Emoji(..)
            | Suggestion::SlashCommand(_)
            | Suggestion::Here => 1,
        }
    }

    fn item_key(&self) -> (u8, &str) {
        match self {
            Suggestion::Here => (0, MENTION_HERE_DISPLAY),
            Suggestion::Member(member, _) => (1, member.user_id.as_str()),
            Suggestion::Role(role) => (2, role.role_id.as_str()),
            Suggestion::Channel(channel) => (3, channel.channel_id.as_str()),
            Suggestion::Emoji(emoji, _) => (4, emoji.emoji_id.as_str()),
            Suggestion::SlashCommand(command) => (5, command.id.as_ref()),
        }
    }

    fn sort_keys(&self) -> (&str, &str) {
        match self {
            Suggestion::Here => (MENTION_HERE_DISPLAY, MENTION_HERE_DISPLAY),
            Suggestion::Member(member, _) => (&member.display_lc, &member.username_lc),
            Suggestion::Role(role) => (&role.title_lc, ""),
            Suggestion::Channel(channel) => (&channel.name_lc, ""),
            Suggestion::Emoji(emoji, _) => (&emoji.shortname_lc, ""),
            Suggestion::SlashCommand(command) => (command.display_lc.as_str(), ""),
        }
    }

    fn norm_keys(&self) -> (&str, &str) {
        match self {
            Suggestion::Here => (MENTION_HERE_NORM, MENTION_HERE_NORM),
            Suggestion::Member(member, _) => (&member.display_norm, &member.username_norm),
            Suggestion::Role(role) => (&role.title_norm, ""),
            Suggestion::Channel(channel) => (&channel.name_norm, &channel.sub_text_norm),
            Suggestion::Emoji(emoji, _) => (&emoji.shortname, ""),
            Suggestion::SlashCommand(command) => (command.display_lc.as_str(), ""),
        }
    }

    fn match_rank(&self, query_lc: &str) -> u8 {
        let (display, username) = self.sort_keys();
        if display == query_lc || username == query_lc {
            0
        } else if display.starts_with(query_lc) || username.starts_with(query_lc) {
            1
        } else {
            2
        }
    }
}

fn resolve_suggestion_media(items: &mut [Suggestion], cx: &App) {
    for item in items {
        if let Suggestion::Member(member, src) = item
            && !member.avatar_raw.is_empty()
        {
            *src = crate::util::imgproxy::avatar_url(cx, &member.avatar_raw).into();
        }
    }
}

fn normalized_with_offsets(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut normalized = String::with_capacity(text.len());
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(text.len());
    for (byte_ix, ch) in text.char_indices() {
        push_search_normalized(ch, &mut normalized);
        spans.resize(normalized.len(), (byte_ix, byte_ix + ch.len_utf8()));
    }
    (normalized, spans)
}

fn highlight_match_range(text: &str, query: &str) -> Option<std::ops::Range<usize>> {
    if text.is_empty() {
        return None;
    }
    let needle = normalize_search_string(query);
    if needle.is_empty() {
        return None;
    }
    let (haystack, spans) = normalized_with_offsets(text);
    let start = haystack.find(&needle)?;
    let end = start + needle.len();
    let from = spans.get(start)?.0;
    let to = spans.get(end - 1)?.1;
    Some(from..to)
}

fn highlighted_label(text: SharedString, query: &str) -> StyledText {
    match highlight_match_range(&text, query) {
        Some(range) => StyledText::new(text).with_highlights(vec![(
            range,
            HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
        )]),
        None => StyledText::new(text),
    }
}

fn prioritize_and_limit(mut items: Vec<Suggestion>, query: &str) -> Vec<Suggestion> {
    let query_lc = query.to_lowercase();
    items.sort_by_cached_key(|item| (item.group_order(), item.match_rank(&query_lc)));
    items.truncate(MAX_SUGGESTIONS);
    items
}

#[derive(Clone, PartialEq, Eq)]
pub enum MentionInputEvent {
    Submit,
    Cancel,
    SendSticker {
        url: String,
        filename: String,
    },
    SendGif {
        url: String,
        width: u32,
        height: u32,
    },
    SendSound {
        url: String,
        filename: String,
    },
    EditLastMessage,
}

struct ActiveComposer(gpui::WeakEntity<MentionInput>);
impl gpui::Global for ActiveComposer {}

pub struct MentionInput {
    input: Entity<MentionInputState>,
    committed: Vec<CommittedToken>,
    active_at: Option<usize>,
    active_sigil: Sigil,
    pooled: [Option<MentionScope>; 4],
    query_len: usize,
    active_query: SharedString,
    suggestions: Vec<Suggestion>,
    selected: usize,
    suggestion_scroll: UniformListScrollHandle,
    session_members: Vec<Rc<MentionMemberRaw>>,
    session_roles: Vec<Rc<RoleSuggestRaw>>,
    session_channels: Vec<Rc<ChannelSuggestRaw>>,
    session_emojis: Vec<Rc<EmojiSuggestRaw>>,
    session_commands: Vec<Rc<SlashCommandRaw>>,
    ephemeral_mode: bool,
    ephemeral_target: Option<(i64, SharedString)>,
    base_placeholder: SharedString,
    popup: Option<Entity<GifStickerEmojiPopup>>,
    toggle_bounds: Rc<Cell<Bounds<Pixels>>>,
    _popup_subs: Vec<Subscription>,
    avatar_cache: Entity<LruImageCache>,
    emoji_cache: Entity<LruImageCache>,
    preview_cache: Entity<LruImageCache>,
    settings: Entity<Settings>,
    pending_attachments: Vec<PendingAttachment>,
    recording: Option<ActiveRecording>,
    record_generation: u64,
    encoding_recording: bool,
    _record_task: Option<RecordTask>,
    compact: bool,
    file_menu_open: bool,
    overflow_to_file: bool,
    overflow_counter: Option<isize>,
    last_content: SharedString,
    draft_channel: Option<ChannelId>,
    suppress_typing: bool,
    ogp_preview: Option<OgpResult>,
    ogp_url: Option<String>,
    ogp_generation: u64,
    _ogp_task: Option<Task<()>>,
    _input_sub: Subscription,
    _store_subs: Vec<Subscription>,
}

impl EventEmitter<MentionInputEvent> for MentionInput {}

fn single_edit_region(old: &str, new: &str) -> (usize, usize, usize) {
    let prefix = old
        .as_bytes()
        .iter()
        .zip(new.as_bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .as_bytes()
        .iter()
        .rev()
        .zip(new.as_bytes().iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(max_suffix);
    (
        prefix,
        old.len() - prefix - suffix,
        new.len() - prefix - suffix,
    )
}

fn json_string_utf16_len(s: &str) -> usize {
    serde_json::to_string(s)
        .map(|j| j.encode_utf16().count())
        .unwrap_or_else(|_| s.encode_utf16().count() + 2)
}

fn content_payload_utf8_len(text: &str) -> usize {
    serde_json::to_string(&serde_json::json!({ "t": text }))
        .map(|j| j.len())
        .unwrap_or(text.len() + CONVERT_PREFIX_LEN)
}

fn needs_png_transcode(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Bmp | ImageFormat::Tiff | ImageFormat::Ico | ImageFormat::Pnm
    )
}

fn image_format_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Pnm => "pnm",
    }
}

fn write_clipboard_image(image: &Image, base: i64, index: usize) -> Option<PathBuf> {
    let dir = std::env::temp_dir();
    if needs_png_transcode(image.format())
        && let Ok(decoded) = image::load_from_memory(image.bytes())
    {
        let mut buffer = std::io::Cursor::new(Vec::new());
        if decoded
            .write_to(&mut buffer, image::ImageFormat::Png)
            .is_ok()
        {
            let path = dir.join(format!("pasted-image-{base}-{index}.png"));
            std::fs::write(&path, buffer.into_inner()).ok()?;
            return Some(path);
        }
    }
    let ext = image_format_extension(image.format());
    let path = dir.join(format!("pasted-image-{base}-{index}.{ext}"));
    std::fs::write(&path, image.bytes()).ok()?;
    Some(path)
}

const SUGGESTION_EMOJI_SOURCE_PX: u32 = 48;

impl MentionInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(placeholder, settings, false, window, cx);
        this.overflow_to_file = true;
        this
    }

    pub fn new_edit(
        placeholder: impl Into<SharedString>,
        settings: Entity<Settings>,
        content: &str,
        spans: &[MessageSpan],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(placeholder, settings, true, window, cx);
        this.seed(content, spans, window, cx);
        this
    }

    fn build(
        placeholder: impl Into<SharedString>,
        settings: Entity<Settings>,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = placeholder.into();
        let base_placeholder = placeholder.clone();
        let input = cx.new(|cx| {
            let state = MentionInputState::new(window, cx).placeholder(placeholder);
            if compact { state.compact() } else { state }
        });
        let input_sub = cx.subscribe_in(
            &input,
            window,
            |this, _input, event: &MentionFieldEvent, window, cx| match event {
                MentionFieldEvent::Change => this.on_change(cx),
                MentionFieldEvent::HistoryRestored => this.on_history_restored(cx),
                MentionFieldEvent::PressEnter => this.on_enter(window, cx),
                MentionFieldEvent::NavUp => this.on_nav_up(cx),
                MentionFieldEvent::NavDown => this.on_nav_down(cx),
                MentionFieldEvent::Paste(text) => this.on_paste(text.clone(), window, cx),
                MentionFieldEvent::PasteImages(images) => {
                    this.on_paste_images(images.clone(), window, cx)
                }
                MentionFieldEvent::PastePaths(paths) => {
                    this.on_paste_paths(paths.clone(), window, cx)
                }
            },
        );
        let store_subs = Self::subscribe_pool_sources(cx);
        let avatar_cache = crate::image_cache::shared_avatar_cache(cx);
        let emoji_cache = crate::image_cache::shared_emoji_cache(cx);
        let preview_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "composer-attachment-preview",
                AVATAR_IMAGE_CACHE_CAPACITY,
                AVATAR_IMAGE_CACHE_BYTES,
                AVATAR_ENTRY_MAX_BYTES,
                cx,
            )
        });
        Self {
            input,
            committed: Vec::new(),
            active_at: None,
            active_sigil: Sigil::At,
            pooled: [None; 4],
            query_len: 0,
            active_query: SharedString::default(),
            suggestions: Vec::new(),
            selected: 0,
            suggestion_scroll: UniformListScrollHandle::new(),
            session_members: Vec::new(),
            session_roles: Vec::new(),
            session_channels: Vec::new(),
            session_emojis: Vec::new(),
            session_commands: Vec::new(),
            ephemeral_mode: false,
            ephemeral_target: None,
            base_placeholder,
            popup: None,
            toggle_bounds: Rc::new(Cell::new(Bounds::default())),
            _popup_subs: Vec::new(),
            avatar_cache,
            emoji_cache,
            preview_cache,
            settings,
            pending_attachments: Vec::new(),
            recording: None,
            record_generation: 0,
            encoding_recording: false,
            _record_task: None,
            compact,
            file_menu_open: false,
            overflow_to_file: false,
            overflow_counter: None,
            last_content: SharedString::default(),
            draft_channel: None,
            suppress_typing: false,
            ogp_preview: None,
            ogp_url: None,
            ogp_generation: 0,
            _ogp_task: None,
            _input_sub: input_sub,
            _store_subs: store_subs,
        }
    }

    fn seed(
        &mut self,
        content: &str,
        spans: &[MessageSpan],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.committed = committed_from_spans(content, spans);
        self.input.update(cx, |input, cx| {
            input.set_value(content, window, cx);
        });
        self.sync_ranges(cx);
        self.sync_history_payload(cx);
    }

    pub fn focus_input(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    pub fn bind_channel(
        &mut self,
        channel_id: Option<ChannelId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.draft_channel == channel_id {
            return;
        }
        let Some(store) = ComposeStore::try_global(cx) else {
            self.draft_channel = channel_id;
            self.apply_draft(ComposeDraft::default(), window, cx);
            return;
        };
        let outgoing = self
            .draft_channel
            .map(|leaving| (leaving, self.take_draft(cx)));
        let persist_leaving = outgoing.as_ref().map(|(leaving, _)| {
            ChannelList::global(cx)
                .read(cx)
                .should_persist_compose_draft(*leaving)
        });
        let incoming = store.update(cx, |store, _| {
            if let Some((leaving, draft)) = outgoing {
                if persist_leaving.unwrap_or(false) {
                    match draft {
                        Some(draft) => store.set_draft(leaving, draft),
                        None => store.clear_draft(leaving),
                    }
                } else {
                    store.clear_draft(leaving);
                }
            }
            channel_id.and_then(|channel_id| store.take_draft(channel_id))
        });
        self.draft_channel = channel_id;
        self.apply_draft(incoming.unwrap_or_default(), window, cx);
    }

    fn take_draft(&mut self, cx: &mut Context<Self>) -> Option<ComposeDraft> {
        let text = self.input.read(cx).value().to_string();
        let attachments = std::mem::take(&mut self.pending_attachments);
        if text.trim().is_empty() && attachments.is_empty() {
            return None;
        }
        let tokens = self
            .committed
            .iter()
            .map(CommittedToken::to_compose)
            .collect();
        Some(ComposeDraft {
            text,
            tokens,
            attachments,
        })
    }

    fn apply_draft(&mut self, draft: ComposeDraft, window: &mut Window, cx: &mut Context<Self>) {
        let ComposeDraft {
            text,
            tokens,
            attachments,
        } = draft;
        self.committed = committed_from_compose_tokens(&text, tokens);
        self.pending_attachments = attachments;
        self.reset_popup();
        self.close_popup();
        self.clear_suggestions(cx);
        self.clear_ephemeral(cx);
        self.clear_ogp_preview(cx);
        self.overflow_counter = None;
        self.suppress_typing = true;
        self.last_content = SharedString::from(text.clone());
        self.input.update(cx, |input, cx| {
            input.prepare_channel_switch();
            input.set_mention_spans(Vec::new(), cx);
            input.set_value(text, window, cx);
        });
        self.sync_ranges(cx);
        self.sync_history_payload(cx);
        cx.notify();
    }

    pub fn take_payload(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(
        String,
        OutgoingContent,
        Vec<OutgoingAttachment>,
        Option<OutgoingOgp>,
    )> {
        let raw = self.input.read(cx).value().to_string();
        if raw.trim().is_empty() && self.pending_attachments.is_empty() {
            return None;
        }
        let text = if self.committed.is_empty() {
            raw.trim().to_string()
        } else {
            raw.trim_end().to_string()
        };
        if self.overflow_to_file
            && !text.is_empty()
            && content_payload_utf8_len(&text) > CONVERT_TO_FILE_THRESHOLD
        {
            self.convert_text_to_file(text, window, cx);
            self.committed.clear();
            self.reset_popup();
            self.close_popup();
            self.clear_ogp_preview(cx);
            self.input.update(cx, |input, cx| {
                input.set_mention_spans(Vec::new(), cx);
                input.set_value("", window, cx);
            });
            return None;
        }
        let mut content = OutgoingContent::default();
        for token in &self.committed {
            let s = byte_offset_to_utf16(&raw, token.start) as i32;
            let e = byte_offset_to_utf16(&raw, token.end) as i32;
            match &token.kind {
                TokenKind::Mention { user_id, role_id } => content.mentions.push(OutgoingMention {
                    user_id: user_id.clone(),
                    role_id: role_id.clone(),
                    display: token.display.clone(),
                    s,
                    e,
                }),
                TokenKind::Hashtag { channel_id } => content.hashtags.push(OutgoingHashtag {
                    channel_id: channel_id.clone(),
                    s,
                    e,
                }),
                TokenKind::Emoji { emoji_id } => content.emojis.push(OutgoingEmoji {
                    emoji_id: emoji_id.clone(),
                    s,
                    e,
                }),
            }
        }
        let attachments: Vec<OutgoingAttachment> = self
            .pending_attachments
            .drain(..)
            .map(|p| OutgoingAttachment {
                path: p.path,
                filename: p.filename,
                filetype: p.filetype,
                width: i32::try_from(p.width).unwrap_or(0),
                height: i32::try_from(p.height).unwrap_or(0),
                duration: p.duration,
                poster_jpeg: p.poster_jpeg,
            })
            .collect();
        let ogp = self.take_outgoing_ogp();
        self.committed.clear();
        self.reset_popup();
        self.close_popup();
        self.input.update(cx, |input, cx| {
            input.set_mention_spans(Vec::new(), cx);
            input.set_value("", window, cx);
        });
        Some((text, content, attachments, ogp))
    }

    fn toggle_file_menu(&mut self, cx: &mut Context<Self>) {
        self.file_menu_open = !self.file_menu_open;
        cx.notify();
    }

    fn open_create_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_menu_open = false;
        let locale = SharedString::from(self.settings.read(cx).language.clone());
        window.defer(cx, move |window, cx| {
            CreatePollModal::open(locale, window, cx);
        });
        cx.notify();
    }

    fn open_share_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_menu_open = false;
        if MessagesStore::global(cx).read(cx).is_anonymous_mode() {
            let locale = self.settings.read(cx).language.clone();
            let message = SharedString::from(mezon_i18n::t(
                &locale,
                "common.cannotSendLocationWithAnonymous",
            ));
            Shell::global(cx).update(cx, |shell, cx| shell.info(message, cx));
            cx.notify();
            return;
        }
        let locale = SharedString::from(self.settings.read(cx).language.clone());
        window.defer(cx, move |window, cx| {
            ShareLocationModal::open(locale, window, cx);
        });
        cx.notify();
    }

    fn render_file_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let locale = SharedString::from(self.settings.read(cx).language.clone());
        let show_poll = MessagesStore::global(cx).read(cx).mode() != STREAM_MODE_DM;
        let show_location = !MessagesStore::global(cx).read(cx).is_anonymous_mode();
        let text_color = theme.text_muted;
        let text_hover = theme.text_primary;
        let bg_hover = theme.bg_hover;

        let mut menu = div()
            .absolute()
            .bottom_full()
            .left_0()
            .mb_2()
            .min_w(px(200.))
            .flex()
            .flex_col()
            .p_2()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .id("file-menu-upload")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(text_color)
                    .hover(|s| s.bg(bg_hover).text_color(text_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.file_menu_open = false;
                            this.open_file_picker(window, cx);
                        }),
                    )
                    .child(
                        Icon::new(IconName::SelectFileIcon)
                            .size_5()
                            .flex_none()
                            .text_color(text_color)
                            .hover(|s| s.text_color(text_hover)),
                    )
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(
                        mezon_i18n::t(&locale, "message.fileSelection.uploadFile").to_string(),
                    )),
            );
        if show_location {
            menu =
                menu.child(
                    div()
                        .id("file-menu-location")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .rounded_md()
                        .cursor_pointer()
                        .text_color(text_color)
                        .hover(|s| s.bg(bg_hover).text_color(text_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.open_share_location(window, cx)),
                        )
                        .child(
                            Icon::new(IconName::Location)
                                .size_5()
                                .flex_none()
                                .text_color(text_color)
                                .hover(|s| s.text_color(text_hover)),
                        )
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(
                            mezon_i18n::t(&locale, "message.attachments.location").to_string(),
                        )),
                );
        }
        if show_poll {
            menu = menu.child(
                div()
                    .id("file-menu-poll")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(text_color)
                    .hover(|s| s.bg(bg_hover).text_color(text_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.open_create_poll(window, cx)),
                    )
                    .child(img("icons/check-list-icon.svg").size(px(20.)).flex_none())
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(
                        mezon_i18n::t(&locale, "message.fileSelection.createPoll").to_string(),
                    )),
            );
        }
        menu.into_any_element()
    }

    fn open_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let pending = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .filter_map(build_pending)
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| this.add_pending(pending, window, cx))
                .ok();
        })
        .detach();
    }

    pub fn add_dropped_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let pending = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .filter_map(build_pending)
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| this.add_pending(pending, window, cx))
                .ok();
        })
        .detach();
    }

    fn on_paste(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        if self.overflow_to_file {
            if json_string_utf16_len(&text) > CONVERT_TO_FILE_THRESHOLD {
                self.convert_text_to_file(text, window, cx);
                return;
            }
            let current = self.input.read(cx).value().to_string();
            let combined = format!("{current}{text}");
            if json_string_utf16_len(&combined) > CONVERT_TO_FILE_THRESHOLD {
                self.committed.clear();
                self.reset_popup();
                self.input.update(cx, |input, cx| {
                    input.set_mention_spans(Vec::new(), cx);
                    input.set_value("", window, cx);
                });
                self.convert_text_to_file(combined, window, cx);
                cx.notify();
                return;
            }
        }
        self.input
            .update(cx, |input, cx| input.insert_text(&text, window, cx));
    }

    fn convert_text_to_file(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let filename = format!("{}.txt", chrono::Utc::now().timestamp_millis());
        cx.spawn_in(window, async move |this, cx| {
            let pending = cx
                .background_spawn(async move {
                    let path = std::env::temp_dir().join(&filename);
                    if let Err(err) = std::fs::write(&path, text.as_bytes()) {
                        tracing::warn!("failed to write converted text attachment: {err}");
                        return None;
                    }
                    build_pending(path)
                })
                .await;
            if let Some(pending) = pending {
                this.update_in(cx, |this, window, cx| {
                    this.add_pending(vec![pending], window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    fn on_paste_images(&mut self, images: Vec<Image>, window: &mut Window, cx: &mut Context<Self>) {
        if images.is_empty() {
            return;
        }
        let base = chrono::Utc::now().timestamp_millis();
        cx.spawn_in(window, async move |this, cx| {
            let pending = cx
                .background_spawn(async move {
                    images
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, image)| {
                            build_pending(write_clipboard_image(&image, base, index)?)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| this.add_pending(pending, window, cx))
                .ok();
        })
        .detach();
    }

    fn on_paste_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let images: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| mime_from_extension(path).starts_with("image/"))
            .collect();
        self.add_dropped_paths(images, window, cx);
    }

    fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_some() || self.encoding_recording {
            return;
        }
        let Some(store) = AudioStore::try_global(cx) else {
            return;
        };
        let Some(factory) = store.read(cx).mic_pcm_capture_factory.clone() else {
            Self::show_recording_error(cx);
            return;
        };

        self.record_generation = self.record_generation.wrapping_add(1);
        let generation = self.record_generation;
        let (sender, receiver) = flume::unbounded();

        self._record_task = Some(cx.spawn_in(window, async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move { factory(sender) })
                .await;
            let (capture, format) = match opened {
                Ok(opened) => opened,
                Err(err) => {
                    tracing::warn!("voice recording unavailable: {err}");
                    let _ = this.update(cx, |this, cx| {
                        this.recording = None;
                        Self::show_recording_error(cx);
                        cx.notify();
                    });
                    return;
                }
            };

            let live = this
                .update(cx, |this, cx| {
                    let attached = this
                        .recording
                        .as_mut()
                        .is_some_and(|recording| recording.attach(generation, capture));
                    cx.notify();
                    attached
                })
                .unwrap_or(false);
            if !live {
                return;
            }

            let built = cx
                .background_executor()
                .spawn(encode_recording(receiver, format))
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.encoding_recording = false;
                match built {
                    Ok(attachment) => this.add_pending(vec![attachment], window, cx),
                    Err(err) => tracing::warn!("voice recording failed: {err}"),
                }
                cx.notify();
            });
        }));
        self.recording = Some(ActiveRecording::opening(generation));
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        let keep = recording.is_live() && recording.elapsed_millis() >= MIN_RECORDING_MILLIS;
        drop(recording);

        if keep {
            self.encoding_recording = true;
        } else {
            self._record_task = None;
        }
        cx.notify();
    }

    fn show_recording_error(cx: &mut Context<Self>) {
        Shell::global(cx).update(cx, |shell, cx| {
            shell.toast(ToastKind::Error, "Microphone unavailable", cx)
        });
    }

    fn add_pending(
        &mut self,
        candidates: Vec<PendingAttachment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if candidates.is_empty() {
            return;
        }
        match validate_batch(self.pending_attachments.len(), &candidates) {
            Ok(()) => {
                self.pending_attachments.extend(candidates);
                cx.notify();
            }
            Err(limit) => Self::show_upload_limit(limit, window, cx),
        }
    }

    fn show_upload_limit(limit: AttachmentLimit, window: &mut Window, cx: &mut Context<Self>) {
        let (title, content) = match limit {
            AttachmentLimit::Count => (
                "Upload limit exceeded!".to_string(),
                format!("You can only upload up to {MAX_FILE_ATTACHMENTS} files at a time."),
            ),
            AttachmentLimit::Size(bytes) => (
                "Upload size limit exceeded!".to_string(),
                format!("Maximum allowed size is {}MB", bytes / (1024 * 1024)),
            ),
        };
        Shell::global(cx).update(cx, |shell, cx| {
            shell.show_upload_limit(title, content, window, cx)
        });
    }

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.pending_attachments.len() {
            self.pending_attachments.remove(index);
            cx.notify();
        }
    }

    fn render_attachment_previews(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let chip_bg = theme.bg_tertiary;
        let badge_bg = theme.bg_floating;
        let badge_border = theme.border;
        let muted = theme.text_muted;
        let mut row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(8.))
            .px(px(8.))
            .pt(px(8.))
            .image_cache(self.preview_cache.clone());
        for (index, att) in self.pending_attachments.iter().enumerate() {
            let preview = if att.is_image {
                div()
                    .size(px(64.))
                    .rounded(px(6.))
                    .bg(chip_bg)
                    .overflow_hidden()
                    .child(
                        img(att.path.clone())
                            .size_full()
                            .object_fit(gpui::ObjectFit::Cover),
                    )
                    .into_any_element()
            } else {
                let icon = if att.is_video {
                    IconName::PlayButton
                } else {
                    IconName::FileIcon
                };
                div()
                    .size(px(64.))
                    .rounded(px(6.))
                    .bg(chip_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon).size_5().text_color(muted))
                    .into_any_element()
            };
            row = row.child(
                div().relative().child(preview).child(
                    div()
                        .id(("remove-attachment", index))
                        .absolute()
                        .top(px(-6.))
                        .right(px(-6.))
                        .size(px(18.))
                        .rounded_full()
                        .bg(badge_bg)
                        .border_1()
                        .border_color(badge_border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(Icon::new(IconName::Close).size_3().text_color(muted))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.remove_attachment(index, cx)
                        })),
                ),
            );
        }
        row.into_any_element()
    }

    fn popup_open(&self) -> bool {
        self.active_at.is_some() && !self.suggestions.is_empty()
    }

    fn close_suggestions(&mut self) {
        self.active_at = None;
        self.query_len = 0;
        self.active_query = SharedString::default();
        self.suggestions.clear();
        self.selected = 0;
    }

    fn reset_popup(&mut self) {
        self.close_suggestions();
        self.drop_pool();
    }

    fn end_mention(&mut self, cx: &mut Context<Self>) {
        self.hide(cx);
        self.drop_pool();
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        if self.active_at.is_some() || !self.suggestions.is_empty() {
            self.close_suggestions();
            cx.notify();
        }
    }

    fn clear_suggestions(&mut self, cx: &mut Context<Self>) {
        if !self.suggestions.is_empty() {
            self.suggestions.clear();
            self.selected = 0;
            cx.notify();
        }
    }

    fn drop_pool_slot(&mut self, sigil: Sigil) {
        self.pooled[sigil.slot()] = None;
        match sigil {
            Sigil::At => {
                self.session_members = Vec::new();
                self.session_roles = Vec::new();
            }
            Sigil::Hash => self.session_channels = Vec::new(),
            Sigil::Colon => self.session_emojis = Vec::new(),
            Sigil::Slash => self.session_commands = Vec::new(),
        }
    }

    fn invalidate_pool(&mut self, sigil: Sigil, cx: &mut Context<Self>) {
        self.drop_pool_slot(sigil);
        if self.active_at.is_none() || self.active_sigil != sigil {
            return;
        }
        let content = self.input.read(cx).value_shared();
        self.check_trigger(&content, cx);
    }

    fn on_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.accept(self.selected, window, cx);
        } else {
            cx.emit(MentionInputEvent::Submit);
        }
    }

    fn on_change(&mut self, cx: &mut Context<Self>) {
        let content = self.input.read(cx).value_shared();
        self.revalidate(&content, cx);
        self.after_content_change(content, cx);
    }

    fn after_content_change(&mut self, content: SharedString, cx: &mut Context<Self>) {
        self.check_trigger(&content, cx);
        self.overflow_counter = {
            let threshold = CONVERT_TO_FILE_THRESHOLD - CONVERT_PREFIX_LEN;
            if self.overflow_to_file && content.len() > threshold {
                let len = content.encode_utf16().count();
                (len > threshold).then_some(threshold as isize - len as isize)
            } else {
                None
            }
        };
        self.update_ogp_preview(&content, cx);
        self.last_content = content;
        if std::mem::take(&mut self.suppress_typing) {
            return;
        }
        MessagesStore::global(cx).update(cx, |store, cx| store.notify_typing(cx));
    }

    fn on_history_restored(&mut self, cx: &mut Context<Self>) {
        let payload = self.input.read(cx).history_payload();
        self.committed = payload
            .as_deref()
            .and_then(|p| p.downcast_ref::<Vec<CommittedToken>>())
            .cloned()
            .unwrap_or_default();
        self.sync_ranges(cx);
        let content = self.input.read(cx).value_shared();
        self.after_content_change(content, cx);
    }

    fn sync_history_payload(&self, cx: &mut Context<Self>) {
        let payload: Option<Rc<dyn Any>> = if self.committed.is_empty() {
            None
        } else {
            Some(Rc::new(self.committed.clone()))
        };
        self.input
            .update(cx, |input, _| input.set_history_payload(payload));
    }

    fn update_ogp_preview(&mut self, content: &str, cx: &mut Context<Self>) {
        if self.compact {
            return;
        }
        let internal_domain = AppConfig::try_global(cx).map(|cfg| cfg.domain_url.clone());
        let candidate = first_previewable_url(content, internal_domain.as_deref());
        let Some(url) = candidate else {
            if self.ogp_url.take().is_some() || self.ogp_preview.take().is_some() {
                self._ogp_task = None;
                self.ogp_generation += 1;
                cx.notify();
            }
            return;
        };
        if self.ogp_url.as_deref() == Some(url.as_str()) {
            return;
        }
        self.ogp_url = Some(url.clone());
        self.ogp_preview = None;
        self.ogp_generation += 1;
        let generation = self.ogp_generation;
        cx.notify();
        let invite_id = internal_invite_id(&url, internal_domain.as_deref());
        let invite_gateway = invite_id
            .is_some()
            .then(|| {
                AppConfig::try_global(cx).map(|cfg| {
                    (
                        cfg.api_gw_host.clone(),
                        cfg.api_gw_port,
                        cfg.api_secure,
                        cfg.api_key.clone(),
                    )
                })
            })
            .flatten();
        self._ogp_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let current = this
                .read_with(cx, |this, _| this.ogp_generation == generation)
                .unwrap_or(false);
            if !current {
                return;
            }
            let result = match (&invite_id, &invite_gateway) {
                (Some(invite_id), Some((host, port, secure, key))) => {
                    fetch_invite_preview(host, *port, *secure, key, &url, invite_id).await
                }
                _ => fetch_ogp(&url).await,
            };
            let _ = this.update(cx, |this, cx| {
                if this.ogp_generation != generation {
                    return;
                }
                this.ogp_preview = result.ok();
                cx.notify();
            });
        }));
    }

    pub fn ogp_preview(&self) -> Option<&OgpResult> {
        self.ogp_preview.as_ref()
    }

    fn apply_ephemeral_placeholder(&mut self, cx: &mut Context<Self>) {
        let placeholder = match &self.ephemeral_target {
            Some((_, display)) => {
                let template = mezon_i18n::t(
                    &self.settings.read(cx).language,
                    "messageBox.ephemeralMessage",
                );
                SharedString::from(template.replace("{{username}}", display))
            }
            None => self.base_placeholder.clone(),
        };
        self.input
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }

    pub fn take_ephemeral_receiver(&mut self, cx: &mut Context<Self>) -> Option<i64> {
        let receiver = self.ephemeral_target.take().map(|(id, _)| id);
        if receiver.is_some() {
            self.apply_ephemeral_placeholder(cx);
        }
        receiver
    }

    pub fn clear_ephemeral(&mut self, cx: &mut Context<Self>) {
        let had = self.ephemeral_mode || self.ephemeral_target.is_some();
        self.ephemeral_mode = false;
        self.ephemeral_target = None;
        if had {
            self.apply_ephemeral_placeholder(cx);
            cx.notify();
        }
    }

    pub fn clear_ogp_preview(&mut self, cx: &mut Context<Self>) {
        let had = self.ogp_url.take().is_some() | self.ogp_preview.take().is_some();
        self._ogp_task = None;
        self.ogp_generation += 1;
        if had {
            cx.notify();
        }
    }

    fn take_outgoing_ogp(&mut self) -> Option<OutgoingOgp> {
        let ogp = self.ogp_preview.take().map(|preview| preview.to_outgoing());
        self.ogp_url = None;
        self._ogp_task = None;
        self.ogp_generation += 1;
        ogp
    }

    fn revalidate(&mut self, content: &str, cx: &mut Context<Self>) {
        let before = self.committed.len();
        let all_in_place = self
            .committed
            .iter()
            .all(|token| content.get(token.start..token.end) == Some(token.display.as_str()));
        if all_in_place {
            return;
        }
        let (_, removed, inserted) = single_edit_region(&self.last_content, content);
        let delta = inserted as isize - removed as isize;
        self.committed.sort_by_key(|token| (token.start, token.end));
        let mut min_next = 0usize;
        let mut reanchored = false;
        self.committed.retain_mut(|token| {
            let candidates = [Some(token.start), token.start.checked_add_signed(delta)];
            for start in candidates.into_iter().flatten() {
                if start < min_next {
                    continue;
                }
                let Some(end) = start.checked_add(token.display.len()) else {
                    continue;
                };
                if content.get(start..end) == Some(token.display.as_str()) {
                    reanchored |= token.start != start;
                    token.start = start;
                    token.end = end;
                    min_next = end;
                    return true;
                }
            }
            false
        });
        if reanchored || self.committed.len() != before {
            self.sync_ranges(cx);
            self.sync_history_payload(cx);
        }
    }

    fn sync_ranges(&self, cx: &mut Context<Self>) {
        let spans = self
            .committed
            .iter()
            .map(|token| MentionSpan {
                range: token.start..token.end,
                kind: token.kind.span_kind(),
            })
            .collect();
        self.input
            .update(cx, |input, cx| input.set_mention_spans(spans, cx));
    }

    fn check_trigger(&mut self, content: &str, cx: &mut Context<Self>) {
        let mut cursor = self.input.read(cx).cursor().min(content.len());
        while cursor > 0 && !content.is_char_boundary(cursor) {
            cursor -= 1;
        }
        if cursor == 0 || content.is_empty() {
            self.end_mention(cx);
            return;
        }

        let bytes = content.as_bytes();
        let mut found = None;
        let mut index = cursor;
        while index > 0 {
            index -= 1;
            let ch = bytes[index];
            if let Some(sigil) = Sigil::from_byte(ch)
                && (index == 0 || bytes[index - 1] == b' ' || bytes[index - 1] == b'\n')
            {
                found = Some((index, sigil));
                break;
            }
            if ch == b' ' || ch == b'\n' {
                break;
            }
        }

        let Some((at, sigil)) = found else {
            self.end_mention(cx);
            return;
        };

        if sigil == Sigil::Slash && at != 0 {
            self.end_mention(cx);
            return;
        }

        let query = &content[at + 1..cursor];
        if sigil == Sigil::Colon && query.is_empty() {
            self.hide(cx);
            return;
        }

        let scope = mention_scope(cx);
        if self.pooled[sigil.slot()] != Some(scope) {
            self.refresh_pool(sigil, cx);
            self.pooled[sigil.slot()] = (!self.pool_is_empty(sigil)).then_some(scope);
        }
        self.active_at = Some(at);
        self.active_sigil = sigil;
        self.query_len = query.len() + 1;
        self.active_query = SharedString::from(query.to_string());
        self.build_suggestions(query, cx);
        if self.suggestions.is_empty() {
            self.clear_suggestions(cx);
        } else {
            cx.notify();
        }
    }

    fn subscribe_pool_sources(cx: &mut Context<Self>) -> Vec<Subscription> {
        let mut subs = vec![
            cx.subscribe(
                &ChannelList::global(cx),
                |this, _, event: &ChannelEvent, cx| match event {
                    ChannelEvent::ActiveChannelChanged(_) => {
                        this.hide(cx);
                        this.drop_pool();
                    }
                    ChannelEvent::ClanChannelsLoaded(_) | ChannelEvent::UserChannelsLoaded => {
                        this.invalidate_pool(Sigil::Hash, cx);
                    }
                    ChannelEvent::Unread(_) | ChannelEvent::InVoiceChanged => {}
                    ChannelEvent::ArchivedByAdministrator { .. } => {}
                },
            ),
            cx.subscribe(
                &DirectMessageStore::global(cx),
                |this, _, event: &DirectEvent, cx| {
                    let DirectEvent::Changed { channel_id } = event;
                    if channel_id.is_none() || *channel_id == mention_direct_id(cx) {
                        this.invalidate_pool(Sigil::At, cx);
                    }
                },
            ),
            cx.subscribe(
                &ClanMembersStore::global(cx),
                |this, _, event: &ClanMembersEvent, cx| {
                    let clan_id = &event.clan_id();
                    if mention_role_clan(cx) == Some(*clan_id) {
                        this.invalidate_pool(Sigil::At, cx);
                    }
                },
            ),
            cx.subscribe(
                &ChannelMembersStore::global(cx),
                |this, _, event: &ChannelMembersEvent, cx| {
                    let ChannelMembersEvent::Changed { channel_id } = event;
                    if mention_private_channel(cx) == Some(*channel_id) {
                        this.invalidate_pool(Sigil::At, cx);
                    }
                },
            ),
            cx.subscribe(
                &GroupMembersStore::global(cx),
                |this, _, event: &GroupMembersEvent, cx| {
                    let GroupMembersEvent::Changed { channel_id } = event;
                    if mention_direct_id(cx) == Some(*channel_id) {
                        this.invalidate_pool(Sigil::At, cx);
                    }
                },
            ),
        ];
        if let Some(store) = RolesStore::try_global(cx) {
            subs.push(cx.subscribe(&store, |this, _, event: &RolesEvent, cx| {
                if let RolesEvent::Changed { clan_id } = event
                    && mention_role_clan(cx) == Some(*clan_id)
                {
                    this.invalidate_pool(Sigil::At, cx);
                }
            }));
        }
        if let Some(store) = AccountStore::try_global(cx) {
            subs.push(cx.subscribe(&store, |this, _, event: &AccountEvent, cx| {
                if matches!(
                    event,
                    AccountEvent::AccountLoaded | AccountEvent::AccountSaved
                ) {
                    this.invalidate_pool(Sigil::At, cx);
                }
            }));
        }
        if let Some(store) = EmojiStore::try_global(cx) {
            subs.push(cx.subscribe(&store, |this, _, _: &EmojiEvent, cx| {
                this.invalidate_pool(Sigil::Colon, cx)
            }));
        }
        subs
    }

    fn drop_pool(&mut self) {
        self.pooled = [None; 4];
        self.session_members = Vec::new();
        self.session_roles = Vec::new();
        self.session_channels = Vec::new();
        self.session_emojis = Vec::new();
        self.session_commands = Vec::new();
    }

    fn refresh_pool(&mut self, sigil: Sigil, cx: &mut Context<Self>) {
        match sigil {
            Sigil::At => {
                ensure_mention_members_loaded(cx);
                if let Some(clan_id) = mention_role_clan(cx)
                    && let Some(store) = RolesStore::try_global(cx)
                {
                    store.update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
                }
                self.session_members = mention_member_pool(cx).into_iter().map(Rc::new).collect();
                self.session_roles = role_suggest_pool(cx).into_iter().map(Rc::new).collect();
            }
            Sigil::Hash => {
                ChannelList::global(cx)
                    .update(cx, |store, cx| store.ensure_user_channels_loaded(cx));
                self.session_channels = channel_suggest_pool(cx).into_iter().map(Rc::new).collect()
            }
            Sigil::Colon => {
                if let Some(store) = EmojiStore::try_global(cx) {
                    store.update(cx, |store, cx| store.ensure_loaded(cx));
                }
                self.session_emojis = emoji_suggest_pool(cx).into_iter().map(Rc::new).collect();
            }
            Sigil::Slash => {
                let locale = self.settings.read(cx).language.clone();
                if let Some(channel_id) = MessagesStore::global(cx).read(cx).active_channel_id() {
                    QuickMenuStore::global(cx).update(cx, |store, cx| {
                        store.ensure_loaded(channel_id, QUICK_MENU_TYPE_FLASH, cx);
                    });
                }
                self.session_commands = slash_command_pool(&locale, cx);
            }
        }
    }

    fn pool_is_empty(&self, sigil: Sigil) -> bool {
        match sigil {
            Sigil::At => self.session_members.is_empty() && self.session_roles.is_empty(),
            Sigil::Hash => self.session_channels.is_empty(),
            Sigil::Colon => self.session_emojis.is_empty(),
            Sigil::Slash => self.session_commands.is_empty(),
        }
    }

    fn slash_candidates(&self, query: &str) -> Vec<Suggestion> {
        let query_lc = query.to_lowercase();
        self.session_commands
            .iter()
            .filter(|command| command.display_lc.starts_with(&query_lc))
            .map(|command| Suggestion::SlashCommand(command.clone()))
            .collect()
    }

    fn build_suggestions(&mut self, query: &str, cx: &App) {
        let mut candidates = match self.active_sigil {
            Sigil::At => self.at_candidates(query),
            Sigil::Hash => self.hash_candidates(query),
            Sigil::Colon => self.colon_candidates(query, cx),
            Sigil::Slash => self.slash_candidates(query),
        };
        if self.ephemeral_mode && self.active_sigil == Sigil::At {
            let self_id = BadgeService::try_global(cx)
                .and_then(|badge| badge.read(cx).current_user_id(cx))
                .map(|uid| uid.to_string());
            candidates.retain(|suggestion| match suggestion {
                Suggestion::Here => false,
                Suggestion::Member(member, _) => Some(&member.user_id) != self_id.as_ref(),
                _ => true,
            });
        }
        let mut suggestions = prioritize_and_limit(candidates, query);
        resolve_suggestion_media(&mut suggestions, cx);
        let same_items = suggestions.len() == self.suggestions.len()
            && suggestions
                .iter()
                .zip(self.suggestions.iter())
                .all(|(new, old)| new.item_key() == old.item_key());
        if !same_items || self.selected >= suggestions.len() {
            self.selected = 0;
        }
        self.suggestions = suggestions;
        self.suggestion_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
    }

    fn at_candidates(&self, query: &str) -> Vec<Suggestion> {
        let mut out: Vec<Suggestion> = Vec::new();
        if query.is_empty() {
            out.reserve(self.session_members.len() + self.session_roles.len() + 1);
            out.extend(
                self.session_members
                    .iter()
                    .map(|member| Suggestion::Member(member.clone(), SharedString::default())),
            );
            out.extend(
                self.session_roles
                    .iter()
                    .map(|role| Suggestion::Role(role.clone())),
            );
            out.push(Suggestion::Here);
            return out;
        }
        let needle = normalize_search_string(query);
        for member in &self.session_members {
            if member.display_norm.contains(&needle) || member.username_norm.contains(&needle) {
                out.push(Suggestion::Member(member.clone(), SharedString::default()));
            }
        }
        for role in &self.session_roles {
            if role.title_norm.contains(&needle) {
                out.push(Suggestion::Role(role.clone()));
            }
        }
        if MENTION_HERE_NORM.contains(&needle) {
            out.push(Suggestion::Here);
        }
        if !needle.is_empty() {
            out.sort_by_cached_key(|item| {
                let display = item.norm_keys().0;
                let exact = u8::from(display != needle);
                let index = display.find(&needle).map_or(i64::MAX, |i| i as i64);
                (exact, index)
            });
        }
        out
    }

    fn hash_candidates(&self, query: &str) -> Vec<Suggestion> {
        let needle = normalize_search_string(query);
        self.session_channels
            .iter()
            .filter(|channel| {
                channel.name_norm.contains(&needle) || channel.sub_text_norm.contains(&needle)
            })
            .map(|channel| Suggestion::Channel(channel.clone()))
            .collect()
    }

    fn colon_candidates(&self, query: &str, cx: &App) -> Vec<Suggestion> {
        let needle = query.to_lowercase();
        let mut out = Vec::new();
        for emoji in &self.session_emojis {
            if out.len() >= MAX_EMOJI_CANDIDATES {
                break;
            }
            if emoji.shortname.contains(&needle) {
                let src = crate::util::imgproxy::emoji_url_sized(
                    cx,
                    &emoji.emoji_id,
                    SUGGESTION_EMOJI_SOURCE_PX,
                );
                out.push(Suggestion::Emoji(emoji.clone(), src.into()));
            }
        }
        out
    }

    fn accept(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.active_at else {
            return;
        };
        let Some(suggestion) = self.suggestions.get(index).cloned() else {
            return;
        };
        let content_len = self.input.read(cx).value().len();
        let replace_end = (at + self.query_len).min(content_len);
        if let Suggestion::SlashCommand(command) = &suggestion {
            if command.id.as_ref() == "ephemeral" {
                self.input.update(cx, |input, cx| {
                    input.replace_range(at..replace_end, "@", window, cx)
                });
                self.ephemeral_mode = true;
                self.reset_popup();
                self.sync_ranges(cx);
                cx.notify();
            } else if let Some(action_msg) = command.action_msg.as_ref() {
                self.input.update(cx, |input, cx| {
                    input.replace_range(at..replace_end, action_msg.as_ref(), window, cx)
                });
                self.reset_popup();
                self.sync_ranges(cx);
                cx.notify();
            }
            return;
        }
        if self.ephemeral_mode {
            if let Suggestion::Member(member, _) = &suggestion {
                if let Ok(user_id) = member.user_id.parse::<i64>() {
                    self.ephemeral_target = Some((user_id, member.display.clone().into()));
                }
                self.ephemeral_mode = false;
                self.input.update(cx, |input, cx| {
                    input.replace_range(at..replace_end, "", window, cx)
                });
                self.apply_ephemeral_placeholder(cx);
                self.reset_popup();
                self.sync_ranges(cx);
                cx.notify();
                return;
            }
            self.ephemeral_mode = false;
        }
        let (display, kind) = match suggestion {
            Suggestion::SlashCommand(_) => return,
            Suggestion::Here => (
                "@here".to_string(),
                TokenKind::Mention {
                    user_id: MENTION_HERE_USER_ID.to_string(),
                    role_id: String::new(),
                },
            ),
            Suggestion::Member(member, _) => (
                format!("@{}", member.display),
                TokenKind::Mention {
                    user_id: member.user_id.clone(),
                    role_id: String::new(),
                },
            ),
            Suggestion::Role(role) => (
                format!("@{}", role.title),
                TokenKind::Mention {
                    user_id: String::new(),
                    role_id: role.role_id.clone(),
                },
            ),
            Suggestion::Channel(channel) => (
                format!("#{}", channel.name),
                TokenKind::Hashtag {
                    channel_id: channel.channel_id.clone(),
                },
            ),
            Suggestion::Emoji(emoji, _) => (
                emoji.shortname.clone(),
                TokenKind::Emoji {
                    emoji_id: emoji.emoji_id.clone(),
                },
            ),
        };
        let inserted = format!("{display} ");
        self.input.update(cx, |input, cx| {
            input.replace_range(at..replace_end, &inserted, window, cx)
        });
        let end = at + display.len();
        self.committed.push(CommittedToken {
            kind,
            display,
            start: at,
            end,
        });
        self.reset_popup();
        self.sync_ranges(cx);
        self.sync_history_payload(cx);
        cx.notify();
    }

    fn picker_toggle(
        &self,
        id: &'static str,
        icon: IconName,
        color: Rgba,
        hover_bg: Rgba,
        hover_color: Rgba,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(20.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(hover_bg))
            .child(
                Icon::new(icon)
                    .size_5()
                    .text_color(color)
                    .hover(|s| s.text_color(hover_color)),
            )
    }

    fn toggle_tab(&mut self, tab: SubPanel, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(popup) = &self.popup {
            if popup.read(cx).active_tab() == tab {
                self.close_popup();
            } else {
                popup.update(cx, |popup, cx| popup.set_tab(tab, window, cx));
            }
            cx.notify();
            return;
        }
        self.reset_popup();
        self.open_popup(tab, window, cx);
        cx.notify();
    }

    fn open_popup(&mut self, tab: SubPanel, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.locale(cx);
        let popup = cx.new(|cx| GifStickerEmojiPopup::new(tab, locale, window, cx));
        let pick_sub = cx.subscribe_in(
            &popup,
            window,
            |this, _popup, event, window, cx| match event {
                GifStickerEmojiEvent::Emoji { emoji_id, emoji } => this.insert_emoji(
                    EmojiSuggestRaw {
                        emoji_id: emoji_id.clone(),
                        shortname: emoji.clone(),
                        shortname_lc: emoji.to_lowercase(),
                    },
                    window,
                    cx,
                ),
                GifStickerEmojiEvent::Sticker { url, filename } => {
                    this.close_popup();
                    cx.emit(MentionInputEvent::SendSticker {
                        url: url.clone(),
                        filename: filename.clone(),
                    });
                    cx.notify();
                }
                GifStickerEmojiEvent::Gif { url, width, height } => {
                    this.close_popup();
                    cx.emit(MentionInputEvent::SendGif {
                        url: url.clone(),
                        width: *width,
                        height: *height,
                    });
                    cx.notify();
                }
                GifStickerEmojiEvent::Sound { url, filename } => {
                    this.close_popup();
                    cx.emit(MentionInputEvent::SendSound {
                        url: url.clone(),
                        filename: filename.clone(),
                    });
                    cx.notify();
                }
            },
        );
        let dismiss_sub = cx.subscribe(&popup, |this, _popup, _: &DismissEvent, cx| {
            this.close_popup();
            cx.notify();
        });
        self._popup_subs = vec![pick_sub, dismiss_sub];
        self.popup = Some(popup);
    }

    fn close_popup(&mut self) {
        self.popup = None;
        self._popup_subs.clear();
    }

    pub fn show_panel(&mut self, tab: SubPanel, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(popup) = &self.popup {
            popup.update(cx, |popup, cx| popup.set_tab(tab, window, cx));
        } else {
            self.open_popup(tab, window, cx);
        }
        cx.notify();
    }

    pub fn hide_panel(&mut self, cx: &mut Context<Self>) {
        self.close_popup();
        cx.notify();
    }

    pub fn active_panel(&self, cx: &App) -> Option<SubPanel> {
        self.popup.as_ref().map(|popup| popup.read(cx).active_tab())
    }

    pub fn register_as_active_composer(entity: &Entity<Self>, cx: &mut App) {
        cx.set_global(ActiveComposer(entity.downgrade()));
    }

    pub fn active_composer(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<ActiveComposer>()
            .and_then(|composer| composer.0.upgrade())
    }

    pub(crate) fn probe_set_text(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
        });
        self.on_change(cx);
    }

    pub(crate) fn probe_text(&self, cx: &App) -> SharedString {
        self.input.read(cx).value_shared()
    }

    pub(crate) fn probe_suggestions(&self) -> (bool, usize, Vec<String>) {
        let labels = self
            .suggestions
            .iter()
            .map(|suggestion| suggestion.sort_keys().0.to_string())
            .collect();
        (self.popup_open(), self.selected, labels)
    }

    pub(crate) fn probe_accept(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.popup_open() || index >= self.suggestions.len() {
            return false;
        }
        self.accept(index, window, cx);
        true
    }

    pub(crate) fn probe_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.on_enter(window, cx);
    }

    pub(crate) fn probe_panel_send(
        &mut self,
        kind: &str,
        url: String,
        filename: String,
        width: i32,
        height: i32,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.close_popup();
        match kind {
            "sticker" => cx.emit(MentionInputEvent::SendSticker { url, filename }),
            "gif" => cx.emit(MentionInputEvent::SendGif {
                url,
                width: width.max(0) as u32,
                height: height.max(0) as u32,
            }),
            "sound" => cx.emit(MentionInputEvent::SendSound { url, filename }),
            other => anyhow::bail!("unknown panel kind: {other}"),
        }
        cx.notify();
        Ok(())
    }

    fn insert_emoji(
        &mut self,
        emoji: EmojiSuggestRaw,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content_len = self.input.read(cx).value().len();
        let at = self.input.read(cx).cursor().min(content_len);
        let display = emoji.shortname;
        let inserted = format!("{display} ");
        self.input.update(cx, |input, cx| {
            input.replace_range(at..at, &inserted, window, cx)
        });
        let end = at + display.len();
        self.committed.push(CommittedToken {
            kind: TokenKind::Emoji {
                emoji_id: emoji.emoji_id,
            },
            display,
            start: at,
            end,
        });
        self.sync_ranges(cx);
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn on_nav_up(&mut self, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.selected = self.selected.saturating_sub(1);
            self.suggestion_scroll
                .scroll_to_item(self.selected, ScrollStrategy::Nearest);
            cx.notify();
            return;
        }
        if self.input.read(cx).value().is_empty() {
            cx.emit(MentionInputEvent::EditLastMessage);
            return;
        }
        self.input
            .update(cx, |input, cx| input.move_caret_line(-1, cx));
    }

    fn on_nav_down(&mut self, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.selected = (self.selected + 1).min(self.suggestions.len() - 1);
            self.suggestion_scroll
                .scroll_to_item(self.selected, ScrollStrategy::Nearest);
            cx.notify();
        } else {
            self.input
                .update(cx, |input, cx| input.move_caret_line(1, cx));
        }
    }

    fn on_accept(&mut self, _: &MentionAccept, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.accept(self.selected, window, cx);
        }
    }

    fn on_dismiss(&mut self, _: &MentionDismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.hide(cx);
        } else if self.file_menu_open {
            self.file_menu_open = false;
            cx.notify();
        } else if self.ephemeral_mode || self.ephemeral_target.is_some() {
            self.clear_ephemeral(cx);
            self.committed.clear();
            self.input.update(cx, |input, cx| {
                input.set_mention_spans(Vec::new(), cx);
                input.set_value("", window, cx);
            });
        } else if self.popup.is_some() {
            self.close_popup();
            cx.notify();
        } else {
            cx.emit(MentionInputEvent::Cancel);
        }
    }

    fn render_suggestion_row(
        &self,
        theme: &Theme,
        locale: &str,
        index: usize,
        suggestion: &Suggestion,
        voice_busy: bool,
        entity: &Entity<MentionInput>,
    ) -> AnyElement {
        let text_primary = theme.tokens.text_theme_primary;
        let text_muted = theme.text_muted;
        let selected_bg = theme.tokens.bg_active_member_channel;
        let is_selected = index == self.selected;
        let query = self.active_query.clone();

        let mut display_color = Hsla::from(text_primary);
        let (leading, display, secondary): (Option<AnyElement>, SharedString, SharedString) =
            match suggestion {
                Suggestion::Here => (
                    None,
                    "@here".into(),
                    mezon_i18n::t(locale, "messageBox.suggestions.notifyEveryone").into(),
                ),
                Suggestion::Member(member, avatar_src) => {
                    let mut avatar = Avatar::new()
                        .name(member.display.clone())
                        .size_px(px(20.))
                        .image_cache(self.avatar_cache.clone());
                    if !avatar_src.is_empty() {
                        avatar = avatar
                            .src(avatar_src.clone())
                            .fallback_src(member.avatar_raw.clone());
                    }
                    (
                        Some(avatar.into_any_element()),
                        member.display.clone().into(),
                        format!("@{}", member.username.to_lowercase()).into(),
                    )
                }
                Suggestion::Role(role) => {
                    display_color = role.color;
                    let leading = if role.icon_src.is_empty() {
                        Icon::new(IconName::RoleIcon)
                            .size(px(20.))
                            .flex_shrink_0()
                            .text_color(role.color)
                            .into_any_element()
                    } else {
                        img(role.icon_src.clone())
                            .size(px(20.))
                            .flex_shrink_0()
                            .rounded(px(4.))
                            .into_any_element()
                    };
                    (
                        Some(leading),
                        role.title.clone().into(),
                        SharedString::default(),
                    )
                }
                Suggestion::Channel(channel) => (
                    Some(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.))
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_muted)
                            .child("#")
                            .into_any_element(),
                    ),
                    channel.name.clone().into(),
                    channel.sub_text.clone().into(),
                ),
                Suggestion::Emoji(emoji, src) => {
                    let leading = if src.is_empty() {
                        None
                    } else {
                        Some(
                            img(src.clone())
                                .image_cache(&self.emoji_cache)
                                .id("suggestion-emoji-frames")
                                .size(px(22.))
                                .into_any_element(),
                        )
                    };
                    (
                        leading,
                        emoji.shortname.clone().into(),
                        SharedString::default(),
                    )
                }
                Suggestion::SlashCommand(command) => (
                    Some(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.))
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_muted)
                            .child("/")
                            .into_any_element(),
                    ),
                    command.display.clone(),
                    command.description.clone(),
                ),
            };

        div()
            .id(("mention-suggestion", index))
            .h(px(MENTION_ROW_PX))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .w_full()
            .px_3()
            .py_2()
            .rounded(px(8.))
            .cursor_pointer()
            .when(is_selected, |row| row.bg(selected_bg))
            .on_hover({
                let entity = entity.clone();
                move |hovered: &bool, _window: &mut Window, cx: &mut App| {
                    if *hovered {
                        entity.update(cx, |this, cx| {
                            if this.selected != index {
                                this.selected = index;
                                cx.notify();
                            }
                        });
                    }
                }
            })
            .on_click({
                let entity = entity.clone();
                move |_event, window: &mut Window, cx: &mut App| {
                    entity.update(cx, |this, cx| this.accept(index, window, cx));
                }
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .when_some(leading, |row, leading| row.child(leading))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(display_color)
                            .child(highlighted_label(display, &query)),
                    )
                    .when(voice_busy, |row| {
                        row.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(15.))
                                .italic()
                                .text_color(theme.danger_text)
                                .child("(busy)"),
                        )
                    }),
            )
            .when(!secondary.is_empty(), |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(highlighted_label(secondary, &query)),
                )
            })
            .into_any_element()
    }

    fn render_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.popup.clone()?;
        let toggle_bounds = self.toggle_bounds.clone();
        Some(
            deferred(
                div()
                    .absolute()
                    .bottom_full()
                    .right(px(20.))
                    .mb(px(10.))
                    .occlude()
                    .on_mouse_down_out(cx.listener(
                        move |this, event: &gpui::MouseDownEvent, _, cx| {
                            if toggle_bounds.get().contains(&event.position) {
                                return;
                            }
                            this.close_popup();
                            cx.notify();
                        },
                    ))
                    .child(popup),
            )
            .into_any_element(),
        )
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn build_suggestion_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let popup_bg = theme.tokens.bg_ping_member;
        let border = theme.border;
        let entity = cx.entity();
        let count = self.suggestions.len();
        let list_h = (count as f32 * MENTION_ROW_PX).min(MENTION_POPUP_MAX_PX);
        let locale = self.locale(cx);
        let header_title = self.active_sigil.category_title(&locale);
        let list = uniform_list(
            "mention-suggestion-list",
            count,
            move |range, _window, cx| {
                let theme = cx.theme();
                let channels = ChannelList::global(cx).read(cx);
                let this = entity.read(cx);
                range
                    .map(|ix| match this.suggestions.get(ix) {
                        Some(suggestion) => {
                            let voice_busy = match suggestion {
                                Suggestion::Channel(channel) => channels
                                    .channel(channel.clan_id, channel.id)
                                    .is_some_and(Channel::voice_busy),
                                _ => false,
                            };
                            this.render_suggestion_row(
                                theme, &locale, ix, suggestion, voice_busy, &entity,
                            )
                        }
                        None => div().h(px(MENTION_ROW_PX)).into_any_element(),
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.suggestion_scroll)
        .h(px(list_h))
        .w_full();
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .p_2()
            .h(px(40.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(header_title),
            );
        deferred(
            div()
                .image_cache(self.avatar_cache.clone())
                .id("mention-popup")
                .absolute()
                .bottom_full()
                .left_0()
                .right_0()
                .mb(px(8.))
                .rounded(px(8.))
                .border_1()
                .border_color(border)
                .bg(popup_bg)
                .shadow_lg()
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _event, _window, cx| this.hide(cx)))
                .child(header)
                .child(list),
        )
        .into_any_element()
    }

    fn render_compact(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.popup_open();
        let popup = open.then(|| self.build_suggestion_popup(cx));
        div()
            .relative()
            .w_full()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::on_accept))
            .on_action(cx.listener(Self::on_dismiss))
            .child(MentionInputField::new(&self.input))
            .when_some(popup, |this, popup| this.child(popup))
            .into_any_element()
    }
}

fn channel_suggest_raw(channel: &Channel) -> ChannelSuggestRaw {
    let sub_text = if channel.category_name.is_empty() {
        channel.clan_name.clone()
    } else {
        channel.category_name.clone()
    };
    ChannelSuggestRaw {
        channel_id: channel.id.to_string(),
        id: channel.id,
        clan_id: channel.clan_id,
        name: channel.name.clone(),
        name_lc: channel.name.to_lowercase(),
        name_norm: normalize_search_string(&channel.name),
        sub_text_norm: normalize_search_string(&sub_text),
        sub_text,
    }
}

fn has_hashtag_content(raw: &ChannelSuggestRaw) -> bool {
    !raw.channel_id.is_empty() || !raw.name.is_empty() || !raw.sub_text.is_empty()
}

fn channel_suggest_pool(cx: &App) -> Vec<ChannelSuggestRaw> {
    let channel_list = ChannelList::global(cx);
    let channel_list = channel_list.read(cx);
    if mention_direct_id(cx).is_some() {
        return channel_list
            .user_channels()
            .map(channel_suggest_raw)
            .filter(has_hashtag_content)
            .collect();
    }
    let Some(clan_id) = ClanList::global(cx).read(cx).active_clan_id else {
        return Vec::new();
    };
    let mut seen: std::collections::HashSet<ChannelId> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for category in channel_list.categories_for_clan(clan_id) {
        for channel in &category.channels {
            if !seen.insert(channel.id) {
                continue;
            }
            let raw = channel_suggest_raw(channel);
            if has_hashtag_content(&raw) {
                out.push(raw);
            }
        }
    }
    out
}

fn role_suggest_pool(cx: &App) -> Vec<RoleSuggestRaw> {
    let Some(clan_id) = mention_role_clan(cx) else {
        return Vec::new();
    };
    let Some(store) = RolesStore::try_global(cx) else {
        return Vec::new();
    };
    store
        .read(cx)
        .roles_in_clan(clan_id)
        .into_iter()
        .map(|(role_id, role)| RoleSuggestRaw {
            role_id: role_id.to_string(),
            title: role.name.clone(),
            title_lc: role.name.to_lowercase(),
            title_norm: normalize_search_string(&role.name),
            icon_src: if role.icon.is_empty() {
                SharedString::default()
            } else {
                SharedString::from(crate::util::imgproxy::role_icon_url(cx, &role.icon))
            },
            color: match mezon_store::parse_role_color(&role.color) {
                Some(rgba) => Hsla::from(rgba),
                None => role_fallback_color(),
            },
        })
        .collect()
}

fn sale_item_id_from_source(src: &str) -> String {
    let file_name = src.rsplit('/').next().unwrap_or_default();
    match file_name.rsplit_once('.') {
        Some((stem, _)) => stem.to_string(),
        None => String::new(),
    }
}

fn emoji_suggest_id(emoji: &Emoji) -> String {
    if emoji.is_for_sale && !emoji.src.is_empty() {
        let sale_id = sale_item_id_from_source(&emoji.src);
        if !sale_id.is_empty() {
            return sale_id;
        }
    }
    emoji.id.clone()
}

fn slash_command_pool(locale: &str, cx: &App) -> Vec<Rc<SlashCommandRaw>> {
    let description: SharedString =
        mezon_i18n::t(locale, "messageBox.slashCommands.ephemeral.description").into();
    let mut commands = vec![Rc::new(SlashCommandRaw {
        id: "ephemeral".into(),
        display: "ephemeral".into(),
        display_lc: "ephemeral".to_string(),
        description,
        action_msg: None,
    })];
    let Some(channel_id) = MessagesStore::global(cx).read(cx).active_channel_id() else {
        return commands;
    };
    for item in QuickMenuStore::global(cx)
        .read(cx)
        .items(channel_id, QUICK_MENU_TYPE_FLASH)
    {
        let display = item.menu_name.to_string();
        commands.push(Rc::new(SlashCommandRaw {
            id: format!("quick_menu_{}", item.id).into(),
            display: display.clone().into(),
            display_lc: display.to_lowercase(),
            description: item.action_msg.clone(),
            action_msg: Some(item.action_msg.clone()),
        }));
    }
    commands
}

fn emoji_suggest_pool(cx: &App) -> Vec<EmojiSuggestRaw> {
    let Some(store) = EmojiStore::try_global(cx) else {
        return Vec::new();
    };
    store
        .read(cx)
        .all()
        .into_iter()
        .map(|emoji| EmojiSuggestRaw {
            emoji_id: emoji_suggest_id(emoji),
            shortname: emoji.shortname.clone(),
            shortname_lc: emoji.shortname.to_lowercase(),
        })
        .collect()
}

fn committed_from_compose_tokens(text: &str, tokens: Vec<ComposeToken>) -> Vec<CommittedToken> {
    tokens
        .into_iter()
        .filter(|token| text.get(token.start..token.end) == Some(token.display.as_str()))
        .map(CommittedToken::from_compose)
        .collect()
}

fn committed_from_spans(content: &str, spans: &[MessageSpan]) -> Vec<CommittedToken> {
    let mut committed = Vec::new();
    let mut search_from = 0usize;
    for span in spans {
        let (display, kind) = match span {
            MessageSpan::Mention {
                display,
                user_id,
                role_id,
            } => (
                display.to_string(),
                TokenKind::Mention {
                    user_id: user_id.clone().unwrap_or_default(),
                    role_id: role_id.clone().unwrap_or_default(),
                },
            ),
            MessageSpan::Hashtag {
                display,
                channel_id,
            } => (
                display.to_string(),
                TokenKind::Hashtag {
                    channel_id: channel_id.clone().unwrap_or_default(),
                },
            ),
            MessageSpan::Emoji { name, emoji_id, .. } => (
                name.to_string(),
                TokenKind::Emoji {
                    emoji_id: emoji_id.clone(),
                },
            ),
            _ => continue,
        };
        if display.is_empty() {
            continue;
        }
        let Some(rel) = content
            .get(search_from..)
            .and_then(|hay| hay.find(&display))
        else {
            continue;
        };
        let start = search_from + rel;
        let end = start + display.len();
        committed.push(CommittedToken {
            kind,
            display,
            start,
            end,
        });
        search_from = end;
    }
    committed
}

impl Render for MentionInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.compact {
            return self.render_compact(cx);
        }
        let open = self.popup_open();
        let active_tab = self.popup.as_ref().map(|p| p.read(cx).active_tab());
        let theme = cx.theme().clone();
        let toggle_color = if active_tab == Some(SubPanel::Emoji) {
            theme.text_primary
        } else {
            theme.text_muted
        };
        let sticker_color = if active_tab == Some(SubPanel::Stickers) {
            theme.text_primary
        } else {
            theme.text_muted
        };
        let gif_color = if active_tab == Some(SubPanel::Gifs) {
            theme.text_primary
        } else {
            theme.text_muted
        };
        let icon_hover = theme.text_primary;
        let icon_bg_hover = theme.bg_hover;
        let has_pending = !self.pending_attachments.is_empty();
        let overflow_counter = self.overflow_counter;

        let popup = open.then(|| self.build_suggestion_popup(cx));

        let picker = self.render_popup(cx);
        let previews = has_pending.then(|| self.render_attachment_previews(cx));
        let file_menu = self.file_menu_open.then(|| self.render_file_menu(cx));

        let input_area = div()
            .relative()
            .w_full()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::on_dismiss))
            .when(open, |this| this.on_action(cx.listener(Self::on_accept)))
            .child(MentionInputField::new(&self.input))
            .child(
                div()
                    .absolute()
                    .left(px(12.))
                    .top(px(10.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this.file_menu_open {
                            this.file_menu_open = false;
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .id("attachment-add")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.bg(icon_bg_hover))
                            .child(
                                Icon::new(IconName::AddCircle)
                                    .size_5()
                                    .text_color(theme.text_muted)
                                    .hover(|s| s.text_color(icon_hover)),
                            )
                            .on_click(
                                cx.listener(|this, _event, _window, cx| this.toggle_file_menu(cx)),
                            ),
                    )
                    .when_some(file_menu, |el, menu| el.child(menu)),
            )
            .child(
                div()
                    .id("mic-record")
                    .absolute()
                    .right(px(96.))
                    .top(px(12.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(icon_bg_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, window, cx| this.start_recording(window, cx)),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.stop_recording(cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.stop_recording(cx)),
                    )
                    .child(if self.recording.is_some() {
                        Icon::new(IconName::MicEnable)
                            .size_5()
                            .text_color(RECORDING_COLOR)
                    } else {
                        Icon::new(IconName::MicEnable)
                            .size_5()
                            .text_color(theme.text_muted)
                            .hover(|s| s.text_color(icon_hover))
                    }),
            )
            .child(
                div()
                    .absolute()
                    .right(px(12.))
                    .top(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        canvas(
                            {
                                let toggle_bounds = self.toggle_bounds.clone();
                                move |bounds, _, _| toggle_bounds.set(bounds)
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        self.picker_toggle(
                            "gif-toggle",
                            IconName::Gif,
                            gif_color,
                            icon_bg_hover,
                            icon_hover,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_tab(SubPanel::Gifs, window, cx)
                        })),
                    )
                    .child(
                        self.picker_toggle(
                            "sticker-toggle",
                            IconName::Sticker,
                            sticker_color,
                            icon_bg_hover,
                            icon_hover,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_tab(SubPanel::Stickers, window, cx)
                        })),
                    )
                    .child(
                        self.picker_toggle(
                            "emoji-toggle",
                            IconName::Smile,
                            toggle_color,
                            icon_bg_hover,
                            icon_hover,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_tab(SubPanel::Emoji, window, cx)
                        })),
                    ),
            )
            .when_some(popup, |this, popup| this.child(popup))
            .when_some(picker, |this, picker| this.child(picker))
            .when_some(overflow_counter, |this, remaining| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .right(px(8.))
                        .text_xs()
                        .text_color(gpui::rgb(0xfca5a5))
                        .child(remaining.to_string()),
                )
            });

        div()
            .flex()
            .flex_col()
            .w_full()
            .when_some(previews, |this, previews| this.child(previews))
            .child(input_area)
            .into_any_element()
    }
}

#[cfg(test)]
mod convert_tests {
    use super::{
        CONVERT_PREFIX_LEN, CONVERT_TO_FILE_THRESHOLD, content_payload_utf8_len,
        json_string_utf16_len,
    };

    #[test]
    fn prefix_len_matches_empty_content_wrapper() {
        assert_eq!(CONVERT_PREFIX_LEN, 8);
        assert_eq!(content_payload_utf8_len(""), 8);
    }

    #[test]
    fn json_utf16_len_counts_quotes_and_escapes_like_js() {
        assert_eq!(json_string_utf16_len(""), 2);
        assert_eq!(json_string_utf16_len("ab"), 4);
        assert_eq!(json_string_utf16_len("\n"), 4);
    }

    #[test]
    fn send_measures_utf8_bytes_paste_measures_utf16_units() {
        let s = "é";
        assert_eq!(json_string_utf16_len(s), 3);
        assert_eq!(content_payload_utf8_len(s), 10);
    }

    #[test]
    fn threshold_boundary_is_strict() {
        let text = "a".repeat(CONVERT_TO_FILE_THRESHOLD - CONVERT_PREFIX_LEN);
        assert_eq!(content_payload_utf8_len(&text), CONVERT_TO_FILE_THRESHOLD);
        assert!(content_payload_utf8_len(&text) <= CONVERT_TO_FILE_THRESHOLD);
        let over = "a".repeat(CONVERT_TO_FILE_THRESHOLD - CONVERT_PREFIX_LEN + 1);
        assert!(content_payload_utf8_len(&over) > CONVERT_TO_FILE_THRESHOLD);
    }
}

#[cfg(test)]
mod suggest_tests {
    use super::sale_item_id_from_source;

    #[test]
    fn sale_id_is_source_basename_without_extension() {
        assert_eq!(
            sale_item_id_from_source("https://cdn.example/emojis/1750123.webp"),
            "1750123"
        );
        assert_eq!(sale_item_id_from_source("1750123.webp"), "1750123");
        assert_eq!(sale_item_id_from_source("a/b/c.tar.gz"), "c.tar");
    }

    #[test]
    fn sale_id_is_empty_without_extension() {
        assert_eq!(sale_item_id_from_source("https://cdn.example/emojis"), "");
        assert_eq!(sale_item_id_from_source(""), "");
    }
}

#[cfg(test)]
mod suggestion_order_tests {
    use super::{
        MentionMemberRaw, RoleSuggestRaw, Suggestion, highlight_match_range, prioritize_and_limit,
        role_fallback_color,
    };
    use gpui::SharedString;
    use std::rc::Rc;

    fn member(display: &str) -> Suggestion {
        Suggestion::Member(
            Rc::new(MentionMemberRaw {
                user_id: display.to_string(),
                display: display.to_string(),
                username: display.to_lowercase(),
                avatar_raw: String::new(),
                display_lc: display.to_lowercase(),
                username_lc: display.to_lowercase(),
                display_norm: display.to_uppercase(),
                username_norm: display.to_uppercase(),
            }),
            SharedString::default(),
        )
    }

    fn role(title: &str) -> Suggestion {
        Suggestion::Role(Rc::new(RoleSuggestRaw {
            role_id: title.to_string(),
            title: title.to_string(),
            title_lc: title.to_lowercase(),
            title_norm: title.to_uppercase(),
            icon_src: SharedString::default(),
            color: role_fallback_color(),
        }))
    }

    fn labels(items: &[Suggestion]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                Suggestion::Member(m, _) => m.display.clone(),
                Suggestion::Role(r) => r.title.clone(),
                Suggestion::Here => "@here".to_string(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn roles_rank_above_members_and_here() {
        let items = vec![
            member("alice"),
            member("bob"),
            Suggestion::Here,
            role("admin"),
            role("mod"),
        ];
        assert_eq!(
            labels(&prioritize_and_limit(items, "")),
            vec!["admin", "mod", "alice", "bob", "@here"]
        );
    }

    #[test]
    fn relevance_orders_within_the_member_group() {
        let items = vec![member("zeta"), Suggestion::Here, member("here-bot")];
        assert_eq!(
            labels(&prioritize_and_limit(items, "here")),
            vec!["here-bot", "zeta", "@here"]
        );
    }

    #[test]
    fn highlight_spans_the_query_inside_the_label() {
        assert_eq!(highlight_match_range("Moderator", "der"), Some(2..5));
        assert_eq!(highlight_match_range("Moderator", "MOD"), Some(0..3));
        assert_eq!(highlight_match_range("Moderator", "zz"), None);
        assert_eq!(highlight_match_range("Moderator", ""), None);
    }

    #[test]
    fn highlight_range_is_byte_safe_for_accented_labels() {
        let label = "Quản trị viên";
        let range = highlight_match_range(label, "tri").expect("accent-insensitive match");
        assert_eq!(&label[range], "trị");
    }
}

#[cfg(test)]
mod draft_tests {
    use super::{CommittedToken, TokenKind, committed_from_compose_tokens};
    use mezon_store::{ComposeToken, ComposeTokenKind};

    fn mention(display: &str, start: usize, end: usize) -> ComposeToken {
        ComposeToken {
            kind: ComposeTokenKind::Mention {
                user_id: "42".to_string(),
                role_id: String::new(),
            },
            display: display.to_string(),
            start,
            end,
        }
    }

    #[test]
    fn keeps_tokens_that_still_match_the_text() {
        let restored = committed_from_compose_tokens("hi @bob", vec![mention("@bob", 3, 7)]);

        assert_eq!(
            restored,
            vec![CommittedToken {
                kind: TokenKind::Mention {
                    user_id: "42".to_string(),
                    role_id: String::new(),
                },
                display: "@bob".to_string(),
                start: 3,
                end: 7,
            }]
        );
    }

    #[test]
    fn drops_tokens_whose_range_no_longer_holds_the_display() {
        let restored = committed_from_compose_tokens("hi @amy", vec![mention("@bob", 3, 7)]);

        assert!(restored.is_empty());
    }

    #[test]
    fn drops_tokens_pointing_past_the_end_of_the_text() {
        let restored = committed_from_compose_tokens("hi", vec![mention("@bob", 3, 7)]);

        assert!(restored.is_empty());
    }

    #[test]
    fn drops_tokens_straddling_a_char_boundary() {
        let restored = committed_from_compose_tokens("héllo @bob", vec![mention("@bob", 1, 5)]);

        assert!(restored.is_empty());
    }

    #[test]
    fn restores_every_token_kind() {
        let text = "a @bob #gen :joy:";
        let tokens = vec![
            mention("@bob", 2, 6),
            ComposeToken {
                kind: ComposeTokenKind::Hashtag {
                    channel_id: "7".to_string(),
                },
                display: "#gen".to_string(),
                start: 7,
                end: 11,
            },
            ComposeToken {
                kind: ComposeTokenKind::Emoji {
                    emoji_id: "9".to_string(),
                },
                display: ":joy:".to_string(),
                start: 12,
                end: 17,
            },
        ];

        let restored = committed_from_compose_tokens(text, tokens);

        assert_eq!(restored.len(), 3);
        assert!(matches!(restored[0].kind, TokenKind::Mention { .. }));
        assert!(matches!(restored[1].kind, TokenKind::Hashtag { .. }));
        assert!(matches!(restored[2].kind, TokenKind::Emoji { .. }));
    }
}
