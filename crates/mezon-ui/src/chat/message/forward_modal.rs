use std::collections::HashSet;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, UniformListScrollHandle, Window, div, img, prelude::*, px, uniform_list,
};
use mezon_store::{
    BadgeService, ChannelId, ChannelList, ChannelType, ClanId, ClanList, DirectKind,
    DirectMessageStore, ForwardTarget, FriendState, FriendStore, MAX_FORWARD_MESSAGE_LENGTH,
    Message, MessageId, MessagesEvent, MessagesStore, UserId,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Checkbox, Icon, IconName, Input, InputEvent, InputState,
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

enum OptionKind {
    Channel {
        clan_name: SharedString,
        icon: IconName,
        lock: Option<IconName>,
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
fn channel_icon(channel_type: ChannelType, private: bool) -> (IconName, Option<IconName>) {
    let is_thread = matches!(channel_type, ChannelType::Thread);
    let base = if is_thread {
        IconName::ThreadIcon
    } else {
        IconName::Hashtag
    };
    let lock = private.then_some(if is_thread {
        IconName::ThreadLock
    } else {
        IconName::HashtagLock
    });
    (base, lock)
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

fn build_shared_content(message_ids: &[MessageId], cx: &App) -> SharedContent {
    let store = MessagesStore::global(cx);
    let store = store.read(cx);
    let messages = store.messages();
    let selected: Vec<&Message> = message_ids
        .iter()
        .filter_map(|id| messages.iter().find(|m| m.id == *id))
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
        let (channel_type, mode) = match channel.channel_type {
            ChannelType::Text => (1, 2),
            ChannelType::Thread => (7, 6),
            _ => continue,
        };
        let clan_name = clans
            .clan(channel.clan_id)
            .map(|clan| clan.name.as_str())
            .unwrap_or(channel.clan_name.as_str());
        let (icon, lock) = channel_icon(channel.channel_type, channel.private);
        options.push(ForwardOption {
            key: TargetKey::Channel(channel.id),
            label: SharedString::from(channel.name.clone()),
            avatar: SharedString::default(),
            avatar_raw: SharedString::default(),
            kind: OptionKind::Channel {
                clan_name: SharedString::from(clan_name.to_uppercase()),
                icon,
                lock,
            },
            filter_key: channel.name.to_lowercase(),
            sort_key: channel.last_sent_timestamp,
            target: ForwardTarget::Channel {
                clan_id: channel.clan_id,
                channel_id: channel.id,
                channel_type,
                mode,
                is_public: !channel.private,
                label: SharedString::from(format!("#{}", channel.name)),
            },
        });
    }

    options
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
    message_ids: Vec<MessageId>,
    shared: SharedContent,
    shared_summary: Option<SharedString>,
    options: Vec<ForwardOption>,
    filtered: Vec<usize>,
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
}

impl Focusable for ForwardMessageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ForwardMessageModal {
    pub fn open(
        message_ids: Vec<MessageId>,
        locale: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        if message_ids.is_empty() {
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
            let image_cache = crate::image_cache::shared_avatar_cache(cx);
            let options = build_options(cx);
            let filtered = (0..options.len().min(MAX_RESULTS)).collect();
            let shared = build_shared_content(&message_ids, cx);
            let shared_summary = shared.summary(&locale_for_labels);
            Self {
                fingerprint: source_fingerprint(cx),
                focus_handle: cx.focus_handle(),
                shared,
                shared_summary,
                locale,
                message_ids,
                options,
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
        if fingerprint == self.fingerprint {
            return;
        }
        self.fingerprint = fingerprint;
        self.options = build_options(cx);
        self.selected
            .retain(|key| self.options.iter().any(|o| o.key == *key));
        self.recompute_filtered(cx);
        cx.notify();
    }

    fn recompute_filtered(&mut self, cx: &App) {
        let value = self.search_input.read(cx).value();
        let query = value.trim();
        let (scope, needle) = match query.strip_prefix('@') {
            Some(rest) => (SearchScope::Members, rest),
            None => match query.strip_prefix('#') {
                Some(rest) => (SearchScope::Channels, rest),
                None => (SearchScope::All, query),
            },
        };
        self.scope = scope;
        let needle = needle.trim().to_lowercase();

        let options = &self.options;
        let mut hits: Vec<usize> = options
            .iter()
            .enumerate()
            .filter(|(_, o)| scope.accepts(o))
            .filter(|(_, o)| needle.is_empty() || o.filter_key.contains(&needle))
            .map(|(ix, _)| ix)
            .collect();
        hits.sort_by(|a, b| {
            let a = &options[*a];
            let b = &options[*b];
            let a_prefix = a.filter_key.starts_with(&needle);
            let b_prefix = b.filter_key.starts_with(&needle);
            b_prefix
                .cmp(&a_prefix)
                .then(b.sort_key.cmp(&a.sort_key))
                .then(a.filter_key.cmp(&b.filter_key))
        });
        hits.truncate(MAX_RESULTS);
        self.filtered = hits;
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
            .options
            .iter()
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
        let ids = self.message_ids.clone();
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
                    Some(&option_ix) => match this.options.get(option_ix) {
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
            .child(div().h(px(LIST_PX)).w_full().child(if count == 0 {
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
            }));

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
                    theme.status_dnd
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
            theme.status_dnd
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
        .bg(theme.tokens.bg_surface)
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
            .image_cache(image_cache.clone());
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
