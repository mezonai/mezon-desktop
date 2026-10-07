use std::cmp::Ordering;
use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, Render,
    SharedString, Subscription, Task, UniformListScrollHandle, Window, div, img, prelude::*, px,
    uniform_list,
};
use mezon_store::{
    BadgeService, ChannelEvent, ChannelId, ChannelList, ChannelType, ClanId, ClanList,
    CtrlKChannel, CtrlKSearchStore, DirectKind, DirectMessageStore, ForwardTarget, FriendState,
    FriendStore, MAX_FORWARD_MESSAGE_LENGTH, Message, MessageRef, MessagesEvent, MessagesStore,
    ProfileContext, SEARCH_CTRL_K_MAX_TEXT_BYTES, ShareContactSubject, UserId, UsersByUserStore,
    channel_join_params, is_age_restricted, resolve_avatar_url, resolve_user_profile,
};

use crate::app::shell::Shell;
use crate::command_palette::FILTER_DEBOUNCE_MS;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Checkbox, Icon, IconName, Input, InputEvent, InputState,
    Sizable, Size, Spinner,
};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};

const ROW_PX: f32 = 32.;
const LIST_PX: f32 = 300.;
const MAX_RESULTS: usize = 15;
const COUNTER_VISIBLE_AT: usize = MAX_FORWARD_MESSAGE_LENGTH - 200;
const COUNTER_WARN_AT: usize = MAX_FORWARD_MESSAGE_LENGTH - 100;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum TargetKey {
    Channel(ChannelId),
    User(UserId),
}

impl TargetKey {
    fn element_id(self) -> u64 {
        match self {
            Self::Channel(id) => id.get() as u64,
            Self::User(id) => id.0 as u64,
        }
    }
}

#[derive(Clone)]
enum OptionKind {
    Channel {
        clan_name: SharedString,
        icon: IconName,
        lock: Option<IconName>,
        parent_id: Option<ChannelId>,
    },
    Member {
        username: SharedString,
    },
    Group,
}

/// Section labels are uppercased once at open — doing it in `render` re-allocates
/// on every frame (the search caret alone repaints at ~2Hz).
fn upper(locale: &str, key: &'static str) -> SharedString {
    mezon_i18n::t(locale, key).to_uppercase().into()
}

/// the *active* icon shade (`--bg-icon-theme-active`) on top of a glyph drawn in
/// the dimmer `--bg-icon-theme` — two shades of one colour. GPUI tints a whole
/// SVG a single colour, so the lock has to be a second, stacked element.
fn channel_icon(
    channel_type: ChannelType,
    private: bool,
    age_restricted: i32,
) -> (IconName, Option<IconName>) {
    use crate::components::compositions::channel_row::channel_icon as compose_channel_icon;
    if matches!(channel_type, ChannelType::Text) && is_age_restricted(age_restricted) {
        return (IconName::HashtagWarning, None);
    }
    let composed = compose_channel_icon(channel_type, private, age_restricted);
    (composed.base, composed.lock)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchScope {
    All,
    Members,
    Channels,
}

impl SearchScope {
    fn accepts(self, option: &ForwardOption) -> bool {
        match self {
            Self::All => true,
            Self::Channels => matches!(option.kind, OptionKind::Channel { .. }),
            Self::Members => !matches!(option.kind, OptionKind::Channel { .. }),
        }
    }
}

/// The handful of colours a row needs. `uniform_list`'s closure runs on every
/// frame, so it must not clone the whole `Theme` (a ~100-field struct) to read
/// four of them.
#[derive(Clone, Copy)]
struct RowStyle {
    text: gpui::Rgba,
    sub: gpui::Rgba,
    hover_bg: gpui::Rgba,
    icon: gpui::Rgba,
    icon_active: gpui::Rgba,
}

#[derive(Clone)]
struct ForwardOption {
    key: TargetKey,
    label: SharedString,
    avatar: SharedString,
    avatar_raw: SharedString,
    kind: OptionKind,
    filter_key: String,
    sort_key: i64,
    target: ForwardTarget,
}

#[derive(Default)]
struct SharedContent {
    text: SharedString,
    thumbnail: Option<SharedString>,
    thumbnail_is_video: bool,
    extra: usize,
    images: usize,
    videos: usize,
    files: usize,
}

impl SharedContent {
    fn is_empty(&self) -> bool {
        self.text.is_empty() && self.images == 0 && self.videos == 0 && self.files == 0
    }

    fn summary(&self, locale: &str) -> Option<SharedString> {
        let t = |key: &'static str| mezon_i18n::t(locale, key);
        let mut parts: Vec<String> = Vec::new();
        for (count, one, many) in [
            (
                self.images,
                "forwardMessage.modal.image",
                "forwardMessage.modal.images",
            ),
            (
                self.videos,
                "forwardMessage.modal.video",
                "forwardMessage.modal.videos",
            ),
            (
                self.files,
                "forwardMessage.modal.file",
                "forwardMessage.modal.files",
            ),
        ] {
            if count > 0 {
                let noun = if count == 1 { t(one) } else { t(many) };
                parts.push(format!("{count} {noun}"));
            }
        }
        (!parts.is_empty()).then(|| parts.join(" · ").into())
    }
}

fn build_shared_content(sources: &[MessageRef], cx: &App) -> SharedContent {
    let store = MessagesStore::global(cx);
    let store = store.read(cx);
    let messages = store.messages();
    let selected: Vec<&Message> = sources
        .iter()
        .filter_map(|source| {
            store
                .message_in_channel(source.bucket, source.id)
                .or_else(|| messages.iter().find(|m| m.id == source.id))
        })
        .collect();

    let mut content = SharedContent {
        text: selected
            .iter()
            .map(|m| m.content.as_str())
            .find(|text| !text.is_empty())
            .unwrap_or_default()
            .into(),
        ..Default::default()
    };

    let attachments = selected.iter().flat_map(|m| m.attachments.iter());
    for attachment in attachments {
        if attachment.is_image() {
            content.images += 1;
            if content.thumbnail.is_none() {
                content.thumbnail = Some(attachment.proxied_src.clone());
            }
        } else if attachment.is_video() {
            content.videos += 1;
            if content.thumbnail.is_none() && !attachment.thumbnail_proxied.is_empty() {
                content.thumbnail = Some(attachment.thumbnail_proxied.clone());
                content.thumbnail_is_video = true;
            }
        } else {
            content.files += 1;
        }
    }
    let total = content.images + content.videos + content.files;
    content.extra = total.saturating_sub(1);
    content
}

fn build_options(cx: &App) -> Vec<ForwardOption> {
    let me = BadgeService::global(cx).read(cx).current_user_id(cx);
    let mut options = Vec::new();
    let mut dm_peers: HashSet<UserId> = HashSet::new();

    let friend_store = FriendStore::global(cx);
    let friends = friend_store.read(cx);
    let blocked: HashSet<UserId> = friends
        .friends()
        .iter()
        .filter(|f| f.state == FriendState::Blocked && Some(f.source_id) == me)
        .map(|f| f.id)
        .collect();

    for dm in DirectMessageStore::global(cx).read(cx).channels() {
        if let Some(peer) = dm.peer_user_id {
            if blocked.contains(&peer) || Some(peer) == me {
                continue;
            }
            dm_peers.insert(peer);
        }
        let avatar = if dm.avatar.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(crate::util::imgproxy::avatar_url(cx, &dm.avatar))
        };
        let is_group = matches!(dm.kind, DirectKind::Group);
        options.push(ForwardOption {
            key: TargetKey::Channel(dm.id),
            label: SharedString::from(dm.label.clone()),
            avatar,
            avatar_raw: SharedString::from(dm.avatar.clone()),
            kind: if is_group {
                OptionKind::Group
            } else {
                OptionKind::Member {
                    username: SharedString::from(dm.peer_username.clone()),
                }
            },
            filter_key: format!(
                "{} {}",
                dm.label.to_lowercase(),
                dm.peer_username.to_lowercase()
            ),
            sort_key: dm.last_sent_timestamp,
            target: ForwardTarget::Channel {
                clan_id: ClanId(0),
                channel_id: dm.id,
                channel_type: dm.kind.channel_type(),
                mode: dm.kind.stream_mode(),
                is_public: false,
                label: SharedString::from(dm.label.clone()),
            },
        });
    }

    for friend in friends.friends() {
        if friend.state != FriendState::Friend
            || Some(friend.id) == me
            || blocked.contains(&friend.id)
            || dm_peers.contains(&friend.id)
        {
            continue;
        }
        let avatar = if friend.avatar_url.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(crate::util::imgproxy::avatar_url(cx, &friend.avatar_url))
        };
        options.push(ForwardOption {
            key: TargetKey::User(friend.id),
            label: SharedString::from(friend.label().to_string()),
            avatar,
            avatar_raw: SharedString::from(friend.avatar_url.clone()),
            kind: OptionKind::Member {
                username: SharedString::from(friend.username.clone()),
            },
            filter_key: format!(
                "{} {}",
                friend.label().to_lowercase(),
                friend.username.to_lowercase()
            ),
            sort_key: 0,
            target: ForwardTarget::Friend {
                user_id: friend.id,
                label: friend.label().to_string(),
                avatar: friend.avatar_url.clone(),
                username: friend.username.clone(),
            },
        });
    }

    // `ListChannelByUser` does not carry `clan_name`, so resolve it from the clan
    let clans = ClanList::global(cx);
    let clans = clans.read(cx);

    for channel in ChannelList::global(cx).read(cx).user_channels() {
        let clan_name = clans
            .clan(channel.clan_id)
            .map(|clan| clan.name.as_str())
            .unwrap_or(channel.clan_name.as_str());
        options.extend(channel_option(ChannelRow {
            clan_id: channel.clan_id,
            channel_id: channel.id,
            parent_id: channel.parent_id,
            name: &channel.name,
            clan_name,
            channel_type: channel.channel_type,
            private: channel.private,
            age_restricted: channel.age_restricted,
            last_sent_timestamp: channel.last_sent_timestamp,
        }));
    }

    options
}

struct ChannelRow<'a> {
    clan_id: ClanId,
    channel_id: ChannelId,
    parent_id: Option<ChannelId>,
    name: &'a str,
    clan_name: &'a str,
    channel_type: ChannelType,
    private: bool,
    age_restricted: i32,
    last_sent_timestamp: i64,
}

fn channel_option(row: ChannelRow<'_>) -> Option<ForwardOption> {
    if !matches!(row.channel_type, ChannelType::Text | ChannelType::Thread) {
        return None;
    }
    let (is_public, channel_type, mode) =
        channel_join_params(row.channel_type, row.parent_id, row.private);
    let (icon, lock) = channel_icon(row.channel_type, row.private, row.age_restricted);
    Some(ForwardOption {
        key: TargetKey::Channel(row.channel_id),
        label: SharedString::from(row.name.to_string()),
        avatar: SharedString::default(),
        avatar_raw: SharedString::default(),
        kind: OptionKind::Channel {
            clan_name: SharedString::from(row.clan_name.to_uppercase()),
            icon,
            lock,
            parent_id: row.parent_id,
        },
        filter_key: row.name.to_lowercase(),
        sort_key: row.last_sent_timestamp,
        target: ForwardTarget::Channel {
            clan_id: row.clan_id,
            channel_id: row.channel_id,
            channel_type,
            mode,
            is_public,
            label: SharedString::from(format!("#{}", row.name)),
        },
    })
}

fn searched_channel_option(channel: &CtrlKChannel, clan_name: &str) -> Option<ForwardOption> {
    channel_option(ChannelRow {
        clan_id: channel.clan_id,
        channel_id: channel.channel_id,
        parent_id: channel.parent_id,
        name: &channel.label,
        clan_name,
        channel_type: ChannelType::from_raw(channel.channel_type as u32),
        private: channel.private,
        age_restricted: channel.age_restricted,
        last_sent_timestamp: 0,
    })
}

fn searched_options<'a>(
    searched: &'a [CtrlKChannel],
    hidden: impl Fn(&CtrlKChannel) -> bool + 'a,
    clan_name: impl Fn(ClanId) -> Option<&'a str> + 'a,
) -> impl Iterator<Item = ForwardOption> + 'a {
    searched
        .iter()
        .filter(move |channel| !hidden(channel))
        .filter_map(move |channel| searched_channel_option(channel, clan_name(channel.clan_id)?))
}

fn is_removed(channels: &ChannelList, channel_id: ChannelId, parent_id: Option<ChannelId>) -> bool {
    channels.is_locally_removed(channel_id)
        || parent_id.is_some_and(|parent_id| channels.is_locally_removed(parent_id))
}

fn upsert_searched(searched: &mut Vec<CtrlKChannel>, found: Vec<CtrlKChannel>) -> bool {
    let mut changed = false;
    for channel in found {
        match searched
            .iter_mut()
            .find(|known| known.channel_id == channel.channel_id)
        {
            Some(known) if *known == channel => {}
            Some(known) => {
                *known = channel;
                changed = true;
            }
            None => {
                searched.push(channel);
                changed = true;
            }
        }
    }
    changed
}

fn parse_search(query: &str) -> (SearchScope, &str) {
    let query = query.trim();
    let (scope, needle) = match query.strip_prefix('@') {
        Some(rest) => (SearchScope::Members, rest),
        None => match query.strip_prefix('#') {
            Some(rest) => (SearchScope::Channels, rest),
            None => (SearchScope::All, query),
        },
    };
    (scope, needle.trim())
}

fn server_channel_query(query: &str) -> Option<String> {
    let (scope, needle) = parse_search(query);
    (scope != SearchScope::Members
        && !needle.is_empty()
        && needle.len() <= SEARCH_CTRL_K_MAX_TEXT_BYTES)
        .then(|| needle.to_string())
}

fn server_query_key(query: &str) -> String {
    query.to_ascii_lowercase()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Local(usize),
    Searched(usize),
}

fn rank_order(a: &ForwardOption, b: &ForwardOption, needle: &str) -> Ordering {
    let a_prefix = a.filter_key.starts_with(needle);
    let b_prefix = b.filter_key.starts_with(needle);
    b_prefix
        .cmp(&a_prefix)
        .then(b.sort_key.cmp(&a.sort_key))
        .then(a.filter_key.cmp(&b.filter_key))
}

fn ranked_hits(options: &[ForwardOption], scope: SearchScope, needle: &str) -> Vec<usize> {
    let mut hits: Vec<usize> = options
        .iter()
        .enumerate()
        .filter(|(_, option)| {
            scope.accepts(option) && (needle.is_empty() || option.filter_key.contains(needle))
        })
        .map(|(ix, _)| ix)
        .collect();
    hits.sort_by(|a, b| rank_order(&options[*a], &options[*b], needle));
    hits.truncate(MAX_RESULTS);
    hits
}

fn search_hits(
    local: &[ForwardOption],
    searched: &[ForwardOption],
    scope: SearchScope,
    needle: &str,
) -> Vec<Hit> {
    let searched_hits = if needle.is_empty() {
        Vec::new()
    } else {
        ranked_hits(searched, scope, needle)
    };
    let mut hits: Vec<Hit> = ranked_hits(local, scope, needle)
        .into_iter()
        .map(Hit::Local)
        .chain(searched_hits.into_iter().map(Hit::Searched))
        .collect();
    let option = |hit: Hit| match hit {
        Hit::Local(ix) => &local[ix],
        Hit::Searched(ix) => &searched[ix],
    };
    hits.sort_by(|a, b| rank_order(option(*a), option(*b), needle));
    hits
}

/// Cheap gate for the store observers: the three source lists only need a
/// rebuild when one of them actually gains or loses rows. DM traffic notifies
/// the direct store on every incoming message — without this the modal would
/// rebuild its whole option list on each one.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fold_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn fold_str(hash: &mut u64, value: &str) {
    fold_bytes(hash, value.as_bytes());
    fold_bytes(hash, &[0xff]);
}

fn fold_u64(hash: &mut u64, value: u64) {
    fold_bytes(hash, &value.to_le_bytes());
}

fn fold_i64(hash: &mut u64, value: i64) {
    fold_bytes(hash, &value.to_le_bytes());
}

/// Everything `build_options` reads that can change a rendered row, folded without
/// allocating. `build_options` itself costs a `format!` plus several `String`
/// clones per row, so it must not run on every store notify just to be discarded.
fn source_fingerprint(cx: &App) -> u64 {
    let mut hash = FNV_OFFSET;
    for dm in DirectMessageStore::global(cx).read(cx).channels() {
        fold_i64(&mut hash, dm.id.get());
        fold_str(&mut hash, &dm.label);
        fold_str(&mut hash, &dm.avatar);
        fold_str(&mut hash, &dm.peer_username);
        fold_i64(&mut hash, dm.last_sent_timestamp);
        fold_i64(&mut hash, dm.peer_user_id.map_or(0, |id| id.get()));
    }
    for friend in FriendStore::global(cx).read(cx).friends() {
        fold_i64(&mut hash, friend.id.get());
        fold_u64(&mut hash, u64::from(friend.state == FriendState::Friend));
        fold_str(&mut hash, friend.label());
        fold_str(&mut hash, &friend.username);
        fold_str(&mut hash, &friend.avatar_url);
    }
    for channel in ChannelList::global(cx).read(cx).user_channels() {
        fold_i64(&mut hash, channel.id.get());
        fold_str(&mut hash, &channel.name);
        fold_str(&mut hash, &channel.clan_name);
        fold_i64(&mut hash, channel.clan_id.get());
        fold_i64(&mut hash, channel.last_sent_timestamp);
        fold_u64(&mut hash, u64::from(channel.private));
        fold_i64(&mut hash, channel.parent_id.map_or(0, |id| id.get()));
        fold_i64(&mut hash, i64::from(channel.age_restricted));
    }
    for clan in &ClanList::global(cx).read(cx).clans {
        fold_i64(&mut hash, clan.id.get());
        fold_str(&mut hash, &clan.name);
    }
    hash
}

pub struct ForwardMessageModal {
    fingerprint: u64,
    focus_handle: FocusHandle,
    locale: SharedString,
    sources: Vec<MessageRef>,
    shared: SharedContent,
    shared_summary: Option<SharedString>,
    options: Vec<ForwardOption>,
    searched_options: Vec<ForwardOption>,
    searched_channels: Vec<CtrlKChannel>,
    lost_channels: HashSet<ChannelId>,
    last_server_query: Option<String>,
    server_search_pending: bool,
    filtered: Vec<Hit>,
    scope: SearchScope,
    selected: HashSet<TargetKey>,
    search_input: Entity<InputState>,
    note_input: Entity<InputState>,
    note_len: usize,
    submitting: bool,
    progress: Option<(usize, usize)>,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    label_shared: SharedString,
    label_note: SharedString,
    label_members: SharedString,
    label_channels: SharedString,
    _search_sub: Subscription,
    _note_sub: Subscription,
    _channel_obs: Subscription,
    _dm_obs: Subscription,
    _friend_obs: Subscription,
    _clan_obs: Subscription,
    _messages_sub: Subscription,
    _access_lost_sub: Subscription,
    _server_search_task: Task<()>,
}

impl Focusable for ForwardMessageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ForwardMessageModal {
    pub fn open(sources: Vec<MessageRef>, locale: SharedString, window: &mut Window, cx: &mut App) {
        if sources.is_empty() {
            return;
        }
        DirectMessageStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        ChannelList::global(cx).update(cx, |store, cx| store.ensure_user_channels_loaded(cx));
        FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));

        let search_ph =
            mezon_i18n::t(&locale, "forwardMessage.modal.searchPlaceholder").to_string();
        let note_ph =
            mezon_i18n::t(&locale, "forwardMessage.modal.additionalMessagePlaceholder").to_string();

        let locale_for_labels = locale.clone();
        let view = cx.new(|cx| {
            let search_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(search_ph.clone())
                    .embedded(true)
                    .borderless()
                    .text_size(px(14.))
            });
            let note_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(note_ph.clone())
                    .embedded(true)
                    .borderless()
                    .text_size(px(14.))
            });
            let search_sub = cx.subscribe(
                &search_input,
                |this: &mut Self, _input, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        this.recompute_filtered(cx);
                        this.schedule_server_search(cx);
                        cx.notify();
                    }
                    InputEvent::PressEnter => this.send(cx),
                },
            );
            let note_sub = cx.subscribe(
                &note_input,
                |this: &mut Self, input, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        this.note_len = input.read(cx).value().trim().chars().count();
                        cx.notify();
                    }
                    InputEvent::PressEnter => this.send(cx),
                },
            );
            let channel_obs = cx.observe(&ChannelList::global(cx), |this: &mut Self, _, cx| {
                this.refresh_options(cx);
            });
            let dm_obs = cx.observe(&DirectMessageStore::global(cx), |this: &mut Self, _, cx| {
                this.refresh_options(cx);
            });
            let friend_obs = cx.observe(&FriendStore::global(cx), |this: &mut Self, _, cx| {
                this.refresh_options(cx);
            });
            let clan_obs = cx.observe(&ClanList::global(cx), |this: &mut Self, _, cx| {
                this.refresh_options(cx);
            });
            let messages_sub = cx.subscribe(
                &MessagesStore::global(cx),
                |this: &mut Self, _, event: &MessagesEvent, cx| match event {
                    MessagesEvent::ForwardProgress { current, total } => {
                        this.progress = Some((*current, *total));
                        cx.notify();
                    }
                    MessagesEvent::ForwardFinished { sent, failed } => {
                        this.finish(*sent, failed.clone(), cx)
                    }
                    _ => {}
                },
            );
            let access_lost_sub = cx.subscribe(
                &ChannelList::global(cx),
                |this: &mut Self, _, event: &ChannelEvent, cx| {
                    if let ChannelEvent::AccessLost(channel_id) = event {
                        this.forget_searched_channel(*channel_id, cx);
                    }
                },
            );
            let image_cache = crate::image_cache::shared_avatar_cache(cx);
            let options = build_options(cx);
            let filtered = (0..options.len().min(MAX_RESULTS))
                .map(Hit::Local)
                .collect();
            let shared = build_shared_content(&sources, cx);
            let shared_summary = shared.summary(&locale_for_labels);
            Self {
                fingerprint: source_fingerprint(cx),
                focus_handle: cx.focus_handle(),
                shared,
                shared_summary,
                locale,
                sources,
                options,
                searched_options: Vec::new(),
                searched_channels: Vec::new(),
                lost_channels: HashSet::new(),
                last_server_query: None,
                server_search_pending: false,
                filtered,
                scope: SearchScope::All,
                selected: HashSet::new(),
                search_input,
                note_input,
                note_len: 0,
                submitting: false,
                progress: None,
                scroll: UniformListScrollHandle::new(),
                image_cache,
                label_shared: upper(&locale_for_labels, "forwardMessage.modal.sharedContent"),
                label_note: upper(&locale_for_labels, "forwardMessage.modal.additionalMessage"),
                label_members: upper(
                    &locale_for_labels,
                    "forwardMessage.modal.searchFriendsUsers",
                ),
                label_channels: upper(&locale_for_labels, "forwardMessage.modal.searchingChannel"),
                _search_sub: search_sub,
                _note_sub: note_sub,
                _channel_obs: channel_obs,
                _dm_obs: dm_obs,
                _friend_obs: friend_obs,
                _clan_obs: clan_obs,
                _messages_sub: messages_sub,
                _access_lost_sub: access_lost_sub,
                _server_search_task: Task::ready(()),
            }
        });
        let focus_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    /// A partial failure names only the destinations that actually failed — the
    /// ones that went through are still reported as sent.
    fn finish(&mut self, sent: usize, failed: Vec<SharedString>, cx: &mut Context<Self>) {
        if !self.submitting {
            return;
        }
        self.submitting = false;
        self.progress = None;
        let locale = self.locale.clone();
        cx.defer(move |cx| {
            Shell::global(cx).update(cx, |shell, cx| {
                if failed.is_empty() {
                    shell.success(mezon_i18n::t(&locale, "forwardMessage.successMessage"), cx);
                } else {
                    let names = failed
                        .iter()
                        .map(SharedString::as_ref)
                        .collect::<Vec<_>>()
                        .join(", ");
                    let message = format!(
                        "{}: {names}",
                        mezon_i18n::t(&locale, "forwardMessage.errorMessage")
                    );
                    shell.error(SharedString::from(message), cx);
                    if sent > 0 {
                        shell.success(mezon_i18n::t(&locale, "forwardMessage.successMessage"), cx);
                    }
                }
                shell.close_modal(cx);
            });
        });
    }

    /// The DM / friend / channel lists are all fetched asynchronously, so the
    /// modal usually opens before any of them have landed. `ChannelList` only
    /// notifies (it emits no event) when `user_channels` arrive, hence observe
    /// rather than subscribe.
    fn refresh_options(&mut self, cx: &mut Context<Self>) {
        let fingerprint = source_fingerprint(cx);
        if fingerprint == self.fingerprint && !self.lists_a_removed_channel(cx) {
            return;
        }
        self.fingerprint = fingerprint;
        self.reload_options(cx);
    }

    fn reload_options(&mut self, cx: &mut Context<Self>) {
        self.options = build_options(cx);
        self.rebuild_searched_options(cx);
        self.prune_selection();
        self.recompute_filtered(cx);
        cx.notify();
    }

    fn all_options(&self) -> impl Iterator<Item = &ForwardOption> {
        self.options.iter().chain(&self.searched_options)
    }

    fn hit(&self, hit: Hit) -> Option<&ForwardOption> {
        match hit {
            Hit::Local(ix) => self.options.get(ix),
            Hit::Searched(ix) => self.searched_options.get(ix),
        }
    }

    fn prune_selection(&mut self) {
        self.selected.retain(|key| {
            self.options
                .iter()
                .chain(&self.searched_options)
                .any(|o| o.key == *key)
        });
    }

    fn lists_a_removed_channel(&self, cx: &App) -> bool {
        let channels = ChannelList::global(cx);
        let channels = channels.read(cx);
        self.searched_options
            .iter()
            .any(|option| match (option.key, &option.kind) {
                (TargetKey::Channel(id), OptionKind::Channel { parent_id, .. }) => {
                    is_removed(channels, id, *parent_id)
                }
                _ => false,
            })
    }

    fn forget_searched_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        self.lost_channels.insert(channel_id);
        let before = self.searched_channels.len();
        self.searched_channels
            .retain(|channel| channel.channel_id != channel_id);
        if self.searched_channels.len() != before {
            self.reload_options(cx);
        }
    }

    fn rebuild_searched_options(&mut self, cx: &App) {
        let channels = ChannelList::global(cx);
        let channels = channels.read(cx);
        let clans = ClanList::global(cx);
        let clans = clans.read(cx);
        self.searched_options = searched_options(
            &self.searched_channels,
            |channel| {
                channels.user_channel(channel.channel_id).is_some()
                    || is_removed(channels, channel.channel_id, channel.parent_id)
            },
            |clan_id| clans.clan(clan_id).map(|clan| clan.name.as_str()),
        )
        .collect();
    }

    fn schedule_server_search(&mut self, cx: &mut Context<Self>) {
        let query = server_channel_query(self.search_input.read(cx).value());
        let key = query.as_deref().map(server_query_key);
        if key == self.last_server_query {
            return;
        }
        self.last_server_query = key;
        let Some(query) = query else {
            self.server_search_pending = false;
            self._server_search_task = Task::ready(());
            return;
        };
        self.server_search_pending = true;
        self._server_search_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(FILTER_DEBOUNCE_MS))
                .await;
            let Ok(search) = this.update(cx, |this, cx| {
                this.lost_channels.clear();
                CtrlKSearchStore::global(cx)
                    .read(cx)
                    .search_channels(query, cx)
            }) else {
                return;
            };
            let result = search.await;
            let _ = this.update(cx, |this, cx| {
                this.server_search_pending = false;
                match result {
                    Ok(channels) => this.absorb_searched_channels(channels, cx),
                    Err(_) => this.last_server_query = None,
                }
                cx.notify();
            });
        });
    }

    fn absorb_searched_channels(&mut self, mut channels: Vec<CtrlKChannel>, cx: &App) {
        channels.retain(|channel| !self.lost_channels.contains(&channel.channel_id));
        let (_, needle) = self.current_query(cx);
        let kept = self.searched_channels.len();
        let selected = &self.selected;
        self.searched_channels.retain(|channel| {
            selected.contains(&TargetKey::Channel(channel.channel_id))
                || channel.label.to_lowercase().contains(&needle)
        });
        let pruned = self.searched_channels.len() != kept;
        if upsert_searched(&mut self.searched_channels, channels) || pruned {
            self.rebuild_searched_options(cx);
            self.prune_selection();
            self.recompute_filtered(cx);
        }
    }

    fn current_query(&self, cx: &App) -> (SearchScope, String) {
        let (scope, needle) = parse_search(self.search_input.read(cx).value());
        (scope, needle.to_lowercase())
    }

    fn recompute_filtered(&mut self, cx: &App) {
        let (scope, needle) = self.current_query(cx);
        self.scope = scope;
        self.filtered = search_hits(&self.options, &self.searched_options, scope, &needle);
    }

    fn toggle(&mut self, key: TargetKey) {
        if !self.selected.remove(&key) {
            self.selected.insert(key);
        }
    }

    fn note_too_long(&self) -> bool {
        self.note_len > MAX_FORWARD_MESSAGE_LENGTH
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.submitting || self.selected.is_empty() || self.note_too_long() {
            return;
        }
        let targets: Vec<ForwardTarget> = self
            .all_options()
            .filter(|o| self.selected.contains(&o.key))
            .map(|o| o.target.clone())
            .collect();
        if targets.is_empty() {
            return;
        }
        let note = {
            let value = self.note_input.read(cx).value().trim().to_string();
            (!value.is_empty()).then_some(value)
        };
        let ids = self.sources.clone();
        let started =
            MessagesStore::global(cx).update(cx, |store, cx| store.forward(ids, targets, note, cx));
        if !started {
            let locale = self.locale.clone();
            cx.defer(move |cx| {
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.error(mezon_i18n::t(&locale, "forwardMessage.errorMessage"), cx);
                });
            });
            return;
        }
        self.submitting = true;
        self.progress = None;
        cx.notify();
    }

    fn send_label(&self) -> SharedString {
        let locale = self.locale.as_ref();
        if let Some((current, total)) = self.progress {
            return mezon_i18n::t(locale, "forwardMessage.modal.sendingProgress")
                .replace("{{current}}", &current.to_string())
                .replace("{{total}}", &total.to_string())
                .into();
        }
        if self.submitting {
            return mezon_i18n::t(locale, "forwardMessage.modal.sending").into();
        }
        let send = mezon_i18n::t(locale, "forwardMessage.modal.send");
        match self.selected.len() {
            0 => send.into(),
            count @ 1..=99 => format!("{send} ({count})").into(),
            _ => format!("{send} (99+)").into(),
        }
    }
}

impl Render for ForwardMessageModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let entity = cx.entity();

        let header = div().pt_4().child(
            div()
                .w_full()
                .text_center()
                .text_size(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(&locale, "forwardMessage.modal.title")),
        );

        let search_focused = self
            .search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let search = div().px_4().pt_4().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(40.))
                .px(px(10.))
                .rounded_lg()
                .bg(theme.tokens.theme_input)
                .border_1()
                .border_color(if search_focused {
                    theme.brand
                } else {
                    theme.tokens.theme_border_input
                })
                .child(
                    Icon::new(IconName::Search)
                        .size_4()
                        .flex_shrink_0()
                        .text_color(theme.tokens.text_theme_primary),
                )
                .child(Input::new(&self.search_input).w_full()),
        );

        let scope_label = match self.scope {
            SearchScope::All => None,
            SearchScope::Members => Some(self.label_members.clone()),
            SearchScope::Channels => Some(self.label_channels.clone()),
        };
        let scope_header = scope_label.map(|label| {
            div()
                .px_4()
                .pt_3()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tokens.text_theme_primary)
                .child(label)
        });

        let count = self.filtered.len();
        let list_entity = entity.clone();
        let row_style = RowStyle {
            text: theme.tokens.text_theme_primary,
            sub: theme.tokens.text_theme_primary,
            hover_bg: theme.tokens.bg_item_hover,
            icon: theme.tokens.bg_icon_theme,
            icon_active: theme.tokens.bg_icon_theme_active,
        };
        let list = uniform_list("forward-target-list", count, move |range, _window, cx| {
            let this = list_entity.read(cx);
            range
                .map(|ix| match this.filtered.get(ix) {
                    Some(&hit) => match this.hit(hit) {
                        Some(option) => {
                            let selected = this.selected.contains(&option.key);
                            render_option_row(
                                &row_style,
                                &this.image_cache,
                                option,
                                selected,
                                &list_entity,
                            )
                        }
                        None => div().h(px(ROW_PX)).into_any_element(),
                    },
                    None => div().h(px(ROW_PX)).into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .size_full();

        let body = div()
            .px_4()
            .pt_3()
            .pb_2()
            .child(div().h(px(LIST_PX)).w_full().child(
                if count == 0 && self.server_search_pending {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Spinner::new()
                                .with_size(Size::Small)
                                .color(theme.tokens.text_theme_primary.into()),
                        )
                        .into_any_element()
                } else if count == 0 {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(mezon_i18n::t(&locale, "forwardMessage.modal.noResults"))
                        .into_any_element()
                } else {
                    list.into_any_element()
                },
            ));

        let shared = (!self.shared.is_empty()).then(|| {
            render_shared_content(
                theme,
                &self.shared,
                self.shared_summary.clone(),
                self.label_shared.clone(),
            )
        });

        let note_label = div()
            .pt_3()
            .pb_1()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(self.label_note.clone()),
            )
            .when(self.note_len >= COUNTER_VISIBLE_AT, |el| {
                let remaining = MAX_FORWARD_MESSAGE_LENGTH as isize - self.note_len as isize;
                let color = if remaining < 0 {
                    theme.danger_text
                } else if self.note_len >= COUNTER_WARN_AT {
                    theme.status_idle
                } else {
                    theme.tokens.text_theme_primary
                };
                el.child(
                    div()
                        .text_xs()
                        .text_color(color)
                        .child(SharedString::from(remaining.to_string())),
                )
            });

        let note_border = if self.note_too_long() {
            theme.danger_text
        } else {
            theme.tokens.theme_border_input
        };

        let note = div().px_4().child(note_label).child(
            div()
                .flex()
                .items_center()
                .h(px(40.))
                .px(px(10.))
                .rounded_lg()
                .bg(theme.tokens.theme_input)
                .border_1()
                .border_color(note_border)
                .child(Input::new(&self.note_input).w_full()),
        );

        let send_entity = entity.clone();
        let send_disabled = self.submitting || self.selected.is_empty() || self.note_too_long();

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap_4()
            .p_4()
            .child(
                div()
                    .id("forward-cancel")
                    .flex()
                    .items_center()
                    .h(px(40.))
                    .px_4()
                    .rounded_lg()
                    .text_size(px(16.))
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(&locale, "forwardMessage.modal.cancel"))
                    .when(!self.submitting, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.text_color(theme.tokens.text_secondary))
                            .on_click(|_: &ClickEvent, _window, cx| Self::close(cx))
                    })
                    .when(self.submitting, |el| el.opacity(0.5)),
            )
            .child(
                Button::new("forward-send")
                    .primary()
                    .label(self.send_label())
                    .loading(self.submitting)
                    .disabled(send_disabled)
                    .h(px(40.))
                    .on_click(move |_: &ClickEvent, _window, cx| {
                        send_entity.update(cx, |this, cx| this.send(cx));
                    }),
            );

        div()
            .track_focus(&self.focus_handle)
            // Escape is only bound to `menu::Cancel` inside the "menu" key context
            // (see `mezon_ui::init`), so a modal that omits it never sees the action.
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.submitting {
                    return;
                }
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .occlude()
            .image_cache(self.image_cache.clone())
            .w(px(550.))
            .max_h(gpui::relative(0.9))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(4.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(header)
            .child(search)
            .children(scope_header)
            .child(body)
            .children(shared)
            .child(note)
            .child(footer)
    }
}

fn render_shared_content(
    theme: &Theme,
    shared: &SharedContent,
    summary: Option<SharedString>,
    label: SharedString,
) -> AnyElement {
    let mut preview = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .rounded_lg()
        .bg(theme.surfaces.surface)
        .border_1()
        .border_color(theme.tokens.border_primary)
        .p_3();

    if let Some(thumbnail) = shared.thumbnail.clone() {
        let mut thumb = div()
            .relative()
            .size(px(40.))
            .flex_shrink_0()
            .child(img(thumbnail).size(px(40.)).rounded_md());
        if shared.thumbnail_is_video {
            thumb = thumb.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::PlayButton)
                            .size_4()
                            .text_color(theme.tokens.text_secondary),
                    ),
            );
        }
        if shared.extra > 0 {
            thumb = thumb.child(
                div()
                    .absolute()
                    .bottom_0()
                    .right_0()
                    .px_1()
                    .rounded_sm()
                    .bg(theme.tokens.bg_item_hover)
                    .text_xs()
                    .text_color(theme.tokens.text_secondary)
                    .child(SharedString::from(format!("+{}", shared.extra))),
            );
        }
        preview = preview.child(thumb);
    }

    let mut text_column = div().flex().flex_col().gap_1().min_w_0().flex_1();
    if !shared.text.is_empty() {
        text_column = text_column.child(
            div()
                .max_h(px(36.))
                .overflow_hidden()
                .text_sm()
                .text_color(theme.tokens.text_theme_message)
                .child(shared.text.clone()),
        );
    }
    if let Some(summary) = summary {
        text_column = text_column.child(
            div()
                .text_xs()
                .text_color(theme.tokens.text_theme_primary)
                .child(summary),
        );
    }

    div()
        .px_4()
        .pt_3()
        .flex()
        .flex_col()
        .child(
            div()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tokens.text_theme_primary)
                .child(label),
        )
        .child(preview.child(text_column))
        .into_any_element()
}

fn render_option_row(
    style: &RowStyle,
    image_cache: &Entity<LruImageCache>,
    option: &ForwardOption,
    selected: bool,
    entity: &Entity<ForwardMessageModal>,
) -> AnyElement {
    let ent = entity.clone();
    let key = option.key;
    let element_id = key.element_id() as usize;

    let is_channel = matches!(option.kind, OptionKind::Channel { .. });

    let leading: Option<AnyElement> = if let OptionKind::Channel { icon, lock, .. } = option.kind {
        let glyph_color = if lock.is_some() {
            style.icon
        } else {
            style.text
        };
        Some(
            div()
                .relative()
                .size(px(20.))
                .flex_shrink_0()
                .child(Icon::new(icon).size(px(20.)).text_color(glyph_color))
                .children(lock.map(|lock| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .child(Icon::new(lock).size(px(20.)).text_color(style.icon_active))
                }))
                .into_any_element(),
        )
    } else {
        // Not every user has an avatar — `Avatar` falls back to the name initials,
        // and retries the raw URL if the imgproxy one fails (same as FriendsPage).
        let mut avatar = Avatar::new()
            .name(option.label.clone())
            .size_px(px(16.))
            .image_cache(image_cache.clone())
            .group_default(
                matches!(option.kind, OptionKind::Group)
                    && option.avatar.is_empty()
                    && option.avatar_raw.is_empty(),
            );
        if !option.avatar.is_empty() {
            avatar = avatar.src(option.avatar.clone());
            if !option.avatar_raw.is_empty() && option.avatar_raw != option.avatar {
                avatar = avatar.fallback_src(option.avatar_raw.clone());
            }
        } else if !option.avatar_raw.is_empty() {
            avatar = avatar.src(option.avatar_raw.clone());
        }
        Some(div().flex_shrink_0().child(avatar).into_any_element())
    };

    let sub_text: Option<SharedString> = match &option.kind {
        OptionKind::Channel { clan_name, .. } => (!clan_name.is_empty()).then(|| clan_name.clone()),
        OptionKind::Member { username } => (!username.is_empty()).then(|| username.clone()),
        OptionKind::Group => None,
    };

    // name group on the left and the sub-text on the right. A DM passes
    // `wrapSuggestItemStyle="gap-x-1"` (name + username sit side by side); a
    // channel keeps the default `justify-between` (clan name pushed right).
    let mut suggest = div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(24.))
        .w_full()
        .when(is_channel, |el| el.justify_between())
        .when(!is_channel, |el| el.gap_1())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w_0()
                .children(leading)
                .child(
                    div()
                        .truncate()
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(style.text)
                        .child(option.label.clone()),
                ),
        );
    if let Some(sub_text) = sub_text {
        let size = if is_channel { px(10.) } else { px(13.) };
        suggest = suggest.child(
            div()
                .truncate()
                .text_size(size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(style.sub)
                .child(sub_text),
        );
    }

    let content = div()
        .id(("forward-option", element_id))
        .flex_1()
        .min_w_0()
        .mr_1()
        .cursor_pointer()
        .child(suggest)
        .on_click({
            let ent = ent.clone();
            move |_: &ClickEvent, _window, cx| {
                ent.update(cx, |this, cx| {
                    this.toggle(key);
                    cx.notify();
                });
            }
        });

    // background, just `bg-item-hover`.
    let hover_bg = style.hover_bg;
    div()
        .id(("forward-row", element_id))
        .h(px(ROW_PX))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .px_4()
        .rounded(px(4.))
        .hover(move |s| s.bg(hover_bg))
        .child(content)
        .child(
            div().flex_shrink_0().child(
                Checkbox::new(("forward-check", element_id))
                    .checked(selected)
                    .on_click(move |_checked, _window, cx| {
                        ent.update(cx, |this, cx| {
                            this.toggle(key);
                            cx.notify();
                        });
                    }),
            ),
        )
        .into_any_element()
}

fn share_option_excluded(option: &ForwardOption, exclude: UserId, cx: &App) -> bool {
    match &option.target {
        ForwardTarget::Friend { user_id, .. } => *user_id == exclude,
        ForwardTarget::Channel { channel_id, .. } => DirectMessageStore::global(cx)
            .read(cx)
            .channels()
            .iter()
            .find(|dm| dm.id == *channel_id)
            .is_some_and(|dm| {
                matches!(dm.kind, DirectKind::Dm) && dm.peer_user_id == Some(exclude)
            }),
    }
}

fn build_share_options(cx: &App, exclude: UserId) -> Vec<ForwardOption> {
    build_options(cx)
        .into_iter()
        .filter(|opt| !share_option_excluded(opt, exclude, cx))
        .collect()
}

pub fn share_contact_subject(
    user_id: UserId,
    fallback_name: &str,
    context: Option<ProfileContext>,
    cx: &App,
) -> ShareContactSubject {
    if let Some(ctx) = context
        && let Some(profile) = resolve_user_profile(user_id, ctx, cx)
    {
        let avatar = if profile.avatar_url.is_empty() {
            resolve_avatar_url(user_id, ctx, cx).unwrap_or_default()
        } else {
            profile.avatar_url
        };
        let mut username = profile.username;
        if username.is_empty() {
            username = friend_or_user_username(user_id, cx);
        }
        if !username.is_empty() {
            return ShareContactSubject {
                user_id,
                username,
                display_name: profile.display_name,
                avatar,
            };
        }
    }
    if let Some(friend) = FriendStore::global(cx).read(cx).friend(user_id) {
        let display_name = if friend.display_name.is_empty() {
            friend.username.clone()
        } else {
            friend.display_name.clone()
        };
        return ShareContactSubject {
            user_id,
            username: friend.username.clone(),
            display_name,
            avatar: friend.avatar_url.clone(),
        };
    }
    if let Some(user) = UsersByUserStore::global(cx).read(cx).user(user_id) {
        let display_name = if user.display_name.is_empty() {
            user.username.clone()
        } else {
            user.display_name.clone()
        };
        return ShareContactSubject {
            user_id,
            username: user.username.clone(),
            display_name,
            avatar: user.avatar_url.clone(),
        };
    }
    ShareContactSubject {
        user_id,
        username: String::new(),
        display_name: fallback_name.to_string(),
        avatar: String::new(),
    }
}

fn friend_or_user_username(user_id: UserId, cx: &App) -> String {
    if let Some(friend) = FriendStore::global(cx).read(cx).friend(user_id) {
        return friend.username.clone();
    }
    UsersByUserStore::global(cx)
        .read(cx)
        .user(user_id)
        .map(|user| user.username.clone())
        .unwrap_or_default()
}

pub struct ShareContactModal {
    contact: ShareContactSubject,
    fingerprint: u64,
    focus_handle: FocusHandle,
    locale: SharedString,
    options: Vec<ForwardOption>,
    filtered: Vec<usize>,
    selected: HashSet<TargetKey>,
    search_input: Entity<InputState>,
    submitting: bool,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    _search_sub: Subscription,
    _channel_obs: Subscription,
    _dm_obs: Subscription,
    _friend_obs: Subscription,
    _clan_obs: Subscription,
    _messages_sub: Subscription,
}

impl Focusable for ShareContactModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ShareContactModal {
    pub fn open(
        contact: ShareContactSubject,
        locale: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        DirectMessageStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        ChannelList::global(cx).update(cx, |store, cx| store.ensure_user_channels_loaded(cx));
        FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));

        let search_ph = mezon_i18n::t(&locale, "shareContact.modal.searchPlaceholder").to_string();
        let exclude = contact.user_id;
        let view = cx.new(|cx| {
            let search_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(search_ph.clone())
                    .height(px(40.))
            });
            let search_sub = cx.subscribe(
                &search_input,
                |this: &mut Self, _input, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        this.recompute_filtered(cx);
                        cx.notify();
                    }
                    InputEvent::PressEnter => this.send(cx),
                },
            );
            let channel_obs = cx.observe(&ChannelList::global(cx), |this: &mut Self, _, cx| {
                this.refresh_options(cx);
            });
            let dm_obs = cx.observe(&DirectMessageStore::global(cx), |this: &mut Self, _, cx| {
                this.refresh_options(cx);
            });
            let friend_obs = cx.observe(&FriendStore::global(cx), |this: &mut Self, _, cx| {
                this.refresh_options(cx);
            });
            let clan_obs = cx.observe(&ClanList::global(cx), |this: &mut Self, _, cx| {
                this.refresh_options(cx);
            });
            let messages_sub = cx.subscribe(
                &MessagesStore::global(cx),
                |this: &mut Self, _, event: &MessagesEvent, cx| {
                    if let MessagesEvent::ShareContactFinished { sent, failed } = event {
                        this.finish(*sent, failed.clone(), cx);
                    }
                },
            );
            let image_cache = crate::image_cache::shared_avatar_cache(cx);
            let options = build_share_options(cx, exclude);
            let filtered = (0..options.len().min(MAX_RESULTS)).collect();
            Self {
                contact,
                fingerprint: source_fingerprint(cx),
                focus_handle: cx.focus_handle(),
                locale,
                options,
                filtered,
                selected: HashSet::new(),
                search_input,
                submitting: false,
                scroll: UniformListScrollHandle::new(),
                image_cache,
                _search_sub: search_sub,
                _channel_obs: channel_obs,
                _dm_obs: dm_obs,
                _friend_obs: friend_obs,
                _clan_obs: clan_obs,
                _messages_sub: messages_sub,
            }
        });
        let focus_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn finish(&mut self, sent: usize, failed: Vec<SharedString>, cx: &mut Context<Self>) {
        if !self.submitting {
            return;
        }
        self.submitting = false;
        let locale = self.locale.clone();
        cx.defer(move |cx| {
            Shell::global(cx).update(cx, |shell, cx| {
                if failed.is_empty() && sent > 0 {
                    shell.success(
                        mezon_i18n::t(&locale, "shareContact.contactSharedSuccess"),
                        cx,
                    );
                } else {
                    shell.error(
                        mezon_i18n::t(&locale, "shareContact.contactSharedError"),
                        cx,
                    );
                }
                shell.close_modal(cx);
            });
        });
    }

    fn refresh_options(&mut self, cx: &mut Context<Self>) {
        let fingerprint = source_fingerprint(cx);
        if fingerprint == self.fingerprint {
            return;
        }
        self.fingerprint = fingerprint;
        let exclude = self.contact.user_id;
        self.options = build_share_options(cx, exclude);
        self.selected
            .retain(|key| self.options.iter().any(|o| o.key == *key));
        self.recompute_filtered(cx);
        cx.notify();
    }

    fn recompute_filtered(&mut self, cx: &App) {
        let value = self.search_input.read(cx).value();
        let needle = value.trim().to_lowercase();
        let options = &self.options;
        let mut hits: Vec<usize> = options
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                needle.is_empty()
                    || o.filter_key.contains(&needle)
                    || o.label.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect();
        hits.sort_by_key(|&i| options[i].sort_key);
        hits.truncate(MAX_RESULTS);
        self.filtered = hits;
    }

    fn toggle(&mut self, key: TargetKey) {
        if self.selected.contains(&key) {
            self.selected.remove(&key);
        } else {
            self.selected.insert(key);
        }
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.submitting || self.selected.is_empty() {
            return;
        }
        let targets: Vec<ForwardTarget> = self
            .options
            .iter()
            .filter(|o| self.selected.contains(&o.key))
            .map(|o| o.target.clone())
            .collect();
        if targets.is_empty() {
            return;
        }
        let contact = self.contact.clone();
        let started = MessagesStore::global(cx)
            .update(cx, |store, cx| store.share_contact(contact, targets, cx));
        if !started {
            let locale = self.locale.clone();
            cx.defer(move |cx| {
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.error(
                        mezon_i18n::t(&locale, "shareContact.contactSharedError"),
                        cx,
                    );
                });
            });
            return;
        }
        self.submitting = true;
        cx.notify();
    }
}

impl Render for ShareContactModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let send_entity = cx.entity().clone();
        let send_disabled = self.selected.is_empty() || self.submitting;
        let list_entity = cx.entity();

        let search_focused = self
            .search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(_window);
        let search = div().px_4().pt_4().child(
            div()
                .rounded_lg()
                .bg(theme.tokens.theme_input)
                .border_1()
                .border_color(if search_focused {
                    theme.brand
                } else {
                    theme.tokens.theme_border_input
                })
                .child(Input::new(&self.search_input).w_full()),
        );

        let row_style = RowStyle {
            text: theme.tokens.text_theme_primary,
            sub: theme.tokens.text_theme_primary,
            hover_bg: theme.tokens.bg_item_hover,
            icon: theme.tokens.bg_icon_theme,
            icon_active: theme.tokens.bg_icon_theme_active,
        };
        let count = self.filtered.len();
        let list = uniform_list("share-contact-list", count, move |range, _window, cx| {
            let this = list_entity.read(cx);
            range
                .map(|ix| match this.filtered.get(ix) {
                    Some(&option_ix) => match this.options.get(option_ix) {
                        Some(option) => {
                            let selected = this.selected.contains(&option.key);
                            let element_id = option.key.element_id() as usize;
                            render_share_option_row(
                                &row_style,
                                &this.image_cache,
                                option,
                                selected,
                                &list_entity,
                                option.key,
                                element_id,
                            )
                        }
                        None => div().h(px(ROW_PX)).into_any_element(),
                    },
                    None => div().h(px(ROW_PX)).into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .size_full();

        let body = div()
            .h(px(LIST_PX))
            .mt_4()
            .mb_2()
            .px_4()
            .overflow_hidden()
            .child(list);

        let avatar_raw = self.contact.avatar.clone();
        let avatar_proxied = if avatar_raw.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(crate::util::imgproxy::avatar_url(cx, &avatar_raw))
        };
        let mut preview_avatar = Avatar::new()
            .name(SharedString::from(self.contact.display_name.clone()))
            .size_px(px(40.))
            .image_cache(self.image_cache.clone());
        if !avatar_proxied.is_empty() {
            preview_avatar = preview_avatar.src(avatar_proxied);
            if !avatar_raw.is_empty() {
                preview_avatar = preview_avatar.fallback_src(SharedString::from(avatar_raw));
            }
        } else if !avatar_raw.is_empty() {
            preview_avatar = preview_avatar.src(SharedString::from(avatar_raw));
        }

        let preview = div()
            .px_4()
            .child(
                div()
                    .mb_2()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(
                        mezon_i18n::t(&locale, "shareContact.modal.contactPreview").to_uppercase(),
                    ),
            )
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .bg(theme.tokens.bg_active_member_channel)
                    .border_l(px(4.))
                    .border_color(theme.brand)
                    .child(preview_avatar)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .truncate()
                                    .child(self.contact.display_name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.tokens.text_secondary)
                                    .truncate()
                                    .child(format!("@{}", self.contact.username)),
                            ),
                    ),
            );

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap_4()
            .p_4()
            .child(
                div()
                    .id("share-contact-cancel")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(40.))
                    .px_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.tokens.theme_border_input)
                    .text_size(px(16.))
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(&locale, "shareContact.modal.cancel"))
                    .when(!self.submitting, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.text_color(theme.tokens.text_secondary))
                            .on_click(|_: &ClickEvent, _window, cx| ShareContactModal::close(cx))
                    })
                    .when(self.submitting, |el| el.opacity(0.5)),
            )
            .child(
                Button::new("share-contact-send")
                    .primary()
                    .label(mezon_i18n::t(&locale, "shareContact.modal.share"))
                    .loading(self.submitting)
                    .disabled(send_disabled)
                    .h(px(40.))
                    .on_click(move |_: &ClickEvent, _window, cx| {
                        send_entity.update(cx, |this, cx| this.send(cx));
                    }),
            );

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.submitting {
                    return;
                }
                ShareContactModal::close(cx);
            }))
            .occlude()
            .image_cache(self.image_cache.clone())
            .w(px(550.))
            .max_h(gpui::relative(0.9))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(4.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                div()
                    .pt_4()
                    .text_center()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(&locale, "shareContact.modal.title")),
            )
            .child(search)
            .child(body)
            .child(preview)
            .child(footer)
    }
}

fn render_share_option_row(
    style: &RowStyle,
    image_cache: &Entity<LruImageCache>,
    option: &ForwardOption,
    selected: bool,
    ent: &Entity<ShareContactModal>,
    key: TargetKey,
    element_id: usize,
) -> AnyElement {
    let ent = ent.clone();
    let is_channel = matches!(option.kind, OptionKind::Channel { .. });

    let leading: Option<AnyElement> = if let OptionKind::Channel { icon, lock, .. } = option.kind {
        let glyph_color = if lock.is_some() {
            style.icon
        } else {
            style.text
        };
        Some(
            div()
                .relative()
                .size(px(20.))
                .flex_shrink_0()
                .child(Icon::new(icon).size(px(20.)).text_color(glyph_color))
                .children(lock.map(|lock| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .child(Icon::new(lock).size(px(20.)).text_color(style.icon_active))
                }))
                .into_any_element(),
        )
    } else {
        let mut avatar = Avatar::new()
            .name(option.label.clone())
            .size_px(px(16.))
            .image_cache(image_cache.clone())
            .group_default(
                matches!(option.kind, OptionKind::Group)
                    && option.avatar.is_empty()
                    && option.avatar_raw.is_empty(),
            );
        if !option.avatar.is_empty() {
            avatar = avatar.src(option.avatar.clone());
            if !option.avatar_raw.is_empty() && option.avatar_raw != option.avatar {
                avatar = avatar.fallback_src(option.avatar_raw.clone());
            }
        } else if !option.avatar_raw.is_empty() {
            avatar = avatar.src(option.avatar_raw.clone());
        }
        Some(div().flex_shrink_0().child(avatar).into_any_element())
    };

    let sub_text: Option<SharedString> = match &option.kind {
        OptionKind::Channel { clan_name, .. } => (!clan_name.is_empty()).then(|| clan_name.clone()),
        OptionKind::Member { username } => (!username.is_empty()).then(|| username.clone()),
        OptionKind::Group => None,
    };

    let mut suggest = div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(24.))
        .w_full()
        .when(is_channel, |el| el.justify_between())
        .when(!is_channel, |el| el.gap_1())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w_0()
                .children(leading)
                .child(
                    div()
                        .truncate()
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(style.text)
                        .child(option.label.clone()),
                ),
        );
    if let Some(sub_text) = sub_text {
        let size = if is_channel { px(10.) } else { px(13.) };
        suggest = suggest.child(
            div()
                .truncate()
                .text_size(size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(style.sub)
                .child(sub_text),
        );
    }

    let content = div()
        .id(("share-contact-option", element_id))
        .flex_1()
        .min_w_0()
        .mr_1()
        .cursor_pointer()
        .child(suggest)
        .on_click({
            let ent = ent.clone();
            move |_: &ClickEvent, _window, cx| {
                ent.update(cx, |this, cx| {
                    this.toggle(key);
                    cx.notify();
                });
            }
        });

    let hover_bg = style.hover_bg;
    div()
        .id(("share-contact-row", element_id))
        .h(px(ROW_PX))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .px_4()
        .rounded(px(4.))
        .hover(move |s| s.bg(hover_bg))
        .child(content)
        .child(
            div().flex_shrink_0().child(
                Checkbox::new(("share-contact-check", element_id))
                    .checked(selected)
                    .on_click(move |_, _window, cx| {
                        ent.update(cx, |this, cx| {
                            this.toggle(key);
                            cx.notify();
                        });
                    }),
            ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(channel_type: ChannelType) -> ChannelRow<'static> {
        ChannelRow {
            clan_id: ClanId(2),
            channel_id: ChannelId(20),
            parent_id: None,
            name: "General",
            clan_name: "Komu",
            channel_type,
            private: false,
            age_restricted: 0,
            last_sent_timestamp: 0,
        }
    }

    #[test]
    fn server_search_skips_member_queries_and_strips_the_channel_prefix() {
        assert_eq!(server_channel_query("  gen "), Some("gen".to_string()));
        assert_eq!(
            server_channel_query("Đà"),
            Some("Đà".to_string()),
            "sent as typed, like Ctrl+K: ILIKE only folds non-ASCII case under a non-C locale"
        );
        assert_eq!(server_channel_query("# gen"), Some("gen".to_string()));
        assert_eq!(server_channel_query("@gen"), None);
        assert_eq!(server_channel_query("#  "), None);
        assert_eq!(server_channel_query(""), None);
        assert_eq!(
            server_channel_query(&"a".repeat(SEARCH_CTRL_K_MAX_TEXT_BYTES + 1)),
            None,
            "the client rejects longer text itself, so it is never worth a request"
        );
    }

    #[test]
    fn only_an_ascii_case_change_reuses_the_last_search() {
        assert_eq!(server_query_key("Gen"), server_query_key("gen"));
        assert_ne!(
            server_query_key("Đà"),
            server_query_key("đà"),
            "ILIKE folds non-ASCII case only under a non-C locale, so the server may answer \
             differently"
        );
    }

    #[test]
    fn only_text_channels_and_threads_are_forward_targets() {
        let Some(text) = channel_option(row(ChannelType::Text)) else {
            panic!("a text channel is a forward target");
        };
        assert!(matches!(
            text.target,
            ForwardTarget::Channel {
                clan_id: ClanId(2),
                channel_id: ChannelId(20),
                channel_type: 1,
                mode: 2,
                is_public: true,
                ..
            }
        ));
        assert_eq!(text.filter_key, "general");

        let Some(thread) = channel_option(row(ChannelType::Thread)) else {
            panic!("a thread is a forward target");
        };
        assert!(matches!(
            thread.target,
            ForwardTarget::Channel {
                channel_type: 7,
                mode: 6,
                is_public: false,
                ..
            }
        ));

        let Some(child) = channel_option(ChannelRow {
            parent_id: Some(ChannelId(1)),
            ..row(ChannelType::Text)
        }) else {
            panic!("a channel under a parent is a forward target");
        };
        assert!(
            matches!(
                child.target,
                ForwardTarget::Channel {
                    channel_type: 7,
                    mode: 6,
                    is_public: false,
                    ..
                }
            ),
            "anything under a parent is joined as a thread, like every other thread join"
        );

        assert!(channel_option(row(ChannelType::Voice)).is_none());
    }

    fn searched(clan_id: i64, channel_id: i64, channel_type: i32) -> CtrlKChannel {
        CtrlKChannel {
            clan_id: ClanId(clan_id),
            channel_id: ChannelId(channel_id),
            channel_type,
            label: "general".into(),
            ..Default::default()
        }
    }

    #[test]
    fn server_rows_show_only_for_known_clans_and_channels_not_listed_locally() {
        let rows = vec![
            searched(1, 10, 1),
            searched(2, 20, 1),
            searched(3, 30, 1),
            searched(2, 21, 10),
            searched(1, 11, 1),
            CtrlKChannel {
                parent_id: Some(ChannelId(10)),
                ..searched(1, 12, 1)
            },
        ];
        let listed_or_removed = HashSet::from([ChannelId(10)]);
        let known = |clan_id: ClanId| (clan_id != ClanId(3)).then_some("Komu");

        let shown: Vec<ChannelId> = searched_options(
            &rows,
            |channel| {
                listed_or_removed.contains(&channel.channel_id)
                    || channel
                        .parent_id
                        .is_some_and(|parent_id| listed_or_removed.contains(&parent_id))
            },
            known,
        )
        .filter_map(|option| match option.key {
            TargetKey::Channel(id) => Some(id),
            TargetKey::User(_) => None,
        })
        .collect();

        assert_eq!(
            shown,
            vec![ChannelId(20), ChannelId(11)],
            "a channel the local list holds (or knows was archived or deleted) is not repeated, \
             and neither is a thread under a removed parent; one missing locally still shows \
             even in a loaded clan, whose listing is capped; a clan missing from the clan list \
             stays hidden until it arrives; voice is no target"
        );
    }

    #[test]
    fn a_newer_server_row_replaces_the_one_already_kept() {
        let mut kept = vec![searched(2, 20, 1)];
        let mut renamed = searched(2, 20, 1);
        renamed.label = "general-old".into();
        renamed.private = true;

        assert!(upsert_searched(
            &mut kept,
            vec![renamed.clone(), searched(2, 22, 1)]
        ));
        assert_eq!(kept, vec![renamed.clone(), searched(2, 22, 1)]);
        assert!(
            !upsert_searched(&mut kept, vec![renamed]),
            "an unchanged row must not trigger a rebuild"
        );
    }

    fn channel_named(id: i64, name: &'static str, last_sent_timestamp: i64) -> ForwardOption {
        let Some(option) = channel_option(ChannelRow {
            channel_id: ChannelId(id),
            name,
            last_sent_timestamp,
            ..row(ChannelType::Text)
        }) else {
            panic!("a text channel is a forward target");
        };
        option
    }

    #[test]
    fn a_full_local_page_cannot_crowd_out_server_rows() {
        let local: Vec<ForwardOption> = (0..20)
            .map(|ix| channel_named(ix, "general", 100 + ix))
            .collect();
        let searched = vec![channel_named(99, "general", 0)];

        let hits = search_hits(&local, &searched, SearchScope::All, "general");

        assert_eq!(hits.len(), MAX_RESULTS + 1);
        assert!(
            hits.contains(&Hit::Searched(0)),
            "a channel only the server found has no last-sent time, so a page of local matches \
             would always cut it"
        );
    }

    #[test]
    fn server_rows_rank_with_local_rows_by_how_well_they_match() {
        let local = vec![channel_named(1, "devoops", 100)];
        let searched = vec![
            channel_named(10, "bug desktop linux", 0),
            channel_named(12, "opensource", 0),
        ];

        let hits = search_hits(&local, &searched, SearchScope::All, "op");

        assert_eq!(
            hits,
            vec![Hit::Searched(1), Hit::Local(0), Hit::Searched(0)],
            "a name that starts with the query beats one that only contains it, wherever it was \
             found; among equal matches the recently used local row comes first"
        );
        assert_eq!(
            search_hits(&local, &searched, SearchScope::All, ""),
            vec![Hit::Local(0)],
            "with no query only local rows are listed"
        );
    }
}
