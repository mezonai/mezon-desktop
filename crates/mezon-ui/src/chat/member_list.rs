use std::time::Duration;

use gpui::{
    Anchor, AnyElement, App, Context, DismissEvent, Entity, Focusable, FontWeight, Hsla, Pixels,
    Point, SharedString, Subscription, Task, UniformListScrollHandle, WeakEntity, Window, anchored,
    deferred, div, prelude::*, px, rgb, size, uniform_list,
};
use mezon_store::{
    AccountStore, BadgeService, ChannelEvent, ChannelId, ChannelList, ChannelMembersEvent,
    ChannelMembersStore, ClanId, ClanList, ClanMember, ClanMembersEvent, ClanMembersStore,
    DirectEvent, DirectKind, DirectMessageStore, GroupMember, GroupMembersEvent, GroupMembersStore,
    PresenceEvent, PresenceStore, ProfileContext, Settings, UserId, split_members_by_status,
};

use crate::app::shell::Shell;
use crate::chat::member_row_element::MemberRowElement;
use crate::chat::message::{ShareContactModal, share_contact_subject};
use crate::chat::user_profile_popover::UserProfilePopover;
use crate::components::primitives::{Avatar, ContextMenu, IconName, context_menu_at};
use crate::image_cache::LruImageCache;
use crate::router::{Route, Router};
use crate::theme::{ActiveTheme, Theme};
use crate::util::reactive::Derived;
use crate::util::text_utils::normalize_search_string;

const DEFAULT_ROLE_COLOR: u32 = 0x99aab5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MemberSource {
    Channel,
    Group,
}

#[derive(PartialEq)]
enum HeaderKind {
    Online,
    Offline,
    Members,
}

const MEMBER_SKELETON_DELAY_MS: u64 = 400;
const MEMBER_SKELETON_ROWS: usize = 10;

#[derive(PartialEq)]
enum Row {
    Header { kind: HeaderKind, count: usize },
    Member(MemberRow),
    Skeleton,
}

#[derive(PartialEq)]
struct MemberRow {
    user_id: UserId,
    name: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    online: bool,
    user_status: SharedString,
    in_voice: bool,
    is_owner: bool,
    rcm_id: SharedString,
}

struct RawMember {
    user_id: UserId,
    name: String,
    avatar_raw: String,
    online: bool,
    user_status: String,
    in_voice: bool,
}

struct ProfilePopoverState {
    popover: Entity<UserProfilePopover>,
    position: Point<Pixels>,
    _subscription: Subscription,
}

pub struct MemberListPanel {
    source: MemberSource,
    settings: Entity<Settings>,
    rows: Derived<Vec<Row>>,
    list_scroll: UniformListScrollHandle,
    avatar_image_cache: Entity<LruImageCache>,
    small_avatar_image_cache: Entity<LruImageCache>,
    active_context: Option<ProfileContext>,
    route_key: RouteKey,
    open_menu: Option<(UserId, SharedString, Point<Pixels>)>,
    open_profile: Option<ProfilePopoverState>,
    rebuild_pending: bool,
    loading_channel: Option<ChannelId>,
    show_skeleton: bool,
    _skeleton_timer: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

#[derive(PartialEq, Eq)]
enum RouteKey {
    None,
    Channel {
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        filtered: bool,
    },
    Group(ChannelId),
}

impl MemberListPanel {
    pub fn new(
        source: MemberSource,
        settings: Entity<Settings>,
        avatar_image_cache: Entity<LruImageCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = Vec::new();
        subs.push(cx.observe(&Router::global(cx), |this, _, cx| {
            let key = route_key(this.source, cx);
            if key != this.route_key {
                this.route_key = key;
                this.rebuild(cx);
            }
        }));
        subs.push(cx.subscribe(
            &PresenceStore::global(cx),
            |this, _, event, cx| match event {
                PresenceEvent::TypingChanged { .. } => {}
                PresenceEvent::ChannelPresenceChanged { channel_id } => {
                    let relevant = match this.source {
                        MemberSource::Channel => shows_channel(*channel_id, cx),
                        MemberSource::Group => shows_group(*channel_id, cx),
                    };
                    if relevant {
                        this.rebuild(cx);
                    }
                }
                PresenceEvent::StatusChanged => this.rebuild(cx),
            },
        ));
        subs.push(cx.observe(&settings, |_, _, cx| cx.notify()));

        match source {
            MemberSource::Channel => {
                subs.push(
                    cx.subscribe(&ClanMembersStore::global(cx), |this, _, event, cx| {
                        let ClanMembersEvent::Changed { clan_id } = event;
                        if shows_clan(*clan_id, cx) {
                            this.rebuild(cx);
                        }
                    }),
                );
                subs.push(
                    cx.subscribe(&ChannelMembersStore::global(cx), |this, _, event, cx| {
                        let ChannelMembersEvent::Changed { channel_id } = event;
                        if shows_channel(*channel_id, cx) {
                            this.rebuild(cx);
                        }
                    }),
                );
                subs.push(
                    cx.subscribe(&ChannelList::global(cx), |this, _, event, cx| {
                        if matches!(
                            event,
                            ChannelEvent::ActiveChannelChanged(_) | ChannelEvent::InVoiceChanged
                        ) {
                            this.rebuild(cx);
                        }
                    }),
                );
            }
            MemberSource::Group => {
                subs.push(
                    cx.subscribe(&GroupMembersStore::global(cx), |this, _, event, cx| {
                        let GroupMembersEvent::Changed { channel_id } = event;
                        if shows_group(*channel_id, cx) {
                            this.rebuild(cx);
                        }
                    }),
                );
                subs.push(
                    cx.subscribe(&DirectMessageStore::global(cx), |this, _, event, cx| {
                        let DirectEvent::Changed { channel_id } = event;
                        let relevant = match channel_id {
                            Some(id) => shows_group(*id, cx),
                            None => true,
                        };
                        if relevant {
                            this.rebuild(cx);
                        }
                    }),
                );
            }
        }

        let mut this = Self {
            source,
            settings,
            rows: Derived::default(),
            list_scroll: UniformListScrollHandle::new(),
            small_avatar_image_cache: cx.new(|cx| {
                crate::image_cache::LruImageCache::avatar_thumbnail_small(
                    "member-list",
                    512,
                    16 * 1024 * 1024,
                    4 * 1024 * 1024,
                    cx,
                )
            }),
            avatar_image_cache,
            active_context: None,
            route_key: route_key(source, cx),
            open_menu: None,
            open_profile: None,
            rebuild_pending: false,
            loading_channel: None,
            show_skeleton: false,
            _skeleton_timer: None,
            _subs: subs,
        };
        this.recompute(cx);
        this
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        if self.rebuild_pending {
            return;
        }
        self.rebuild_pending = true;
        let handle = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |this, cx| {
                this.rebuild_pending = false;
                this.recompute(cx);
            });
        });
    }

    fn recompute(&mut self, cx: &mut Context<Self>) {
        if matches!(self.source, MemberSource::Channel)
            && let Some(pending_id) = pending_filter_channel(cx)
        {
            if self.loading_channel != Some(pending_id) {
                self.loading_channel = Some(pending_id);
                self.show_skeleton = false;
                self.arm_skeleton_timer(pending_id, cx);
            }
            if self.show_skeleton {
                self.rows.set(
                    (0..MEMBER_SKELETON_ROWS).map(|_| Row::Skeleton).collect(),
                    cx,
                );
            }
            return;
        }
        self.loading_channel = None;
        self.show_skeleton = false;
        self._skeleton_timer = None;
        self.active_context = match self.source {
            MemberSource::Channel => {
                active_channel_context(cx).map(|ctx| ProfileContext::Clan(ctx.clan_id))
            }
            MemberSource::Group => active_group_dm(cx).map(ProfileContext::Direct),
        };
        let rows = compute_rows(self.source, cx);
        self.rows.set(rows, cx);
    }

    fn arm_skeleton_timer(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        self._skeleton_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(MEMBER_SKELETON_DELAY_MS))
                .await;
            this.update(cx, |this, cx| {
                if this.loading_channel == Some(channel_id) && !this.show_skeleton {
                    this.show_skeleton = true;
                    this.recompute(cx);
                }
            })
            .ok();
        }));
    }
}

fn pending_filter_channel(cx: &App) -> Option<ChannelId> {
    if matches!(
        Router::global(cx).read(cx).route(),
        Route::DirectMessage { .. } | Route::Direct | Route::Friends
    ) {
        return None;
    }
    let channels = ChannelList::global(cx);
    let channels = channels.read(cx);
    let channel = channels.active_channel()?;
    let is_thread = channel.parent_id.map(|p| !p.is_zero()).unwrap_or(false);
    if !(channel.private || is_thread) {
        return None;
    }
    let channel_id = channel.id;
    let store = ChannelMembersStore::global(cx);
    let store = store.read(cx);
    if store.has_channel(channel_id) || !store.is_loading(channel_id) {
        return None;
    }
    Some(channel_id)
}

fn shows_clan(clan_id: ClanId, cx: &App) -> bool {
    ClanList::global(cx).read(cx).active_clan_id == Some(clan_id)
}

fn shows_channel(channel_id: ChannelId, cx: &App) -> bool {
    ChannelList::global(cx).read(cx).active_channel_id == Some(channel_id)
}

fn shows_group(channel_id: ChannelId, cx: &App) -> bool {
    active_group_dm(cx) == Some(channel_id)
}

fn route_key(source: MemberSource, cx: &App) -> RouteKey {
    match source {
        MemberSource::Group => match active_group_dm(cx) {
            Some(id) => RouteKey::Group(id),
            None => RouteKey::None,
        },
        MemberSource::Channel => {
            if matches!(
                Router::global(cx).read(cx).route(),
                Route::DirectMessage { .. } | Route::Direct | Route::Friends
            ) {
                return RouteKey::None;
            }
            let channels = ChannelList::global(cx);
            let channels = channels.read(cx);
            let (channel_id, filtered, clan_id) = match channels.active_channel() {
                Some(channel) => {
                    let is_thread = channel.parent_id.map(|p| !p.is_zero()).unwrap_or(false);
                    (
                        Some(channel.id),
                        channel.private || is_thread,
                        channel.clan_id,
                    )
                }
                None => (
                    None,
                    false,
                    ClanList::global(cx)
                        .read(cx)
                        .active_clan_id
                        .unwrap_or_default(),
                ),
            };
            if clan_id.is_zero() {
                return RouteKey::None;
            }
            RouteKey::Channel {
                clan_id,
                channel_id,
                filtered,
            }
        }
    }
}

fn compute_rows(source: MemberSource, cx: &mut Context<MemberListPanel>) -> Vec<Row> {
    match source {
        MemberSource::Group => {
            let Some(direct_id) = active_group_dm(cx) else {
                return Vec::new();
            };
            let members = group_raw_members(cx, direct_id);
            if members.is_empty() {
                return Vec::new();
            }
            let owner_id = DirectMessageStore::global(cx)
                .read(cx)
                .find(direct_id)
                .and_then(|dm| dm.creator_id);
            let mut rows = Vec::with_capacity(members.len() + 1);
            rows.push(Row::Header {
                kind: HeaderKind::Members,
                count: members.len(),
            });
            rows.extend(
                members
                    .into_iter()
                    .map(|raw| make_member_row(cx, raw, owner_id)),
            );
            rows
        }
        MemberSource::Channel => {
            let Some(ctx) = active_channel_context(cx) else {
                return Vec::new();
            };
            let owner_id = ClanList::global(cx)
                .read(cx)
                .clan(ctx.clan_id)
                .map(|clan| clan.creator_id);
            let (online_raw, offline_raw) = channel_raw_members(cx, ctx);
            let mut rows = Vec::with_capacity(online_raw.len() + offline_raw.len() + 2);
            rows.push(Row::Header {
                kind: HeaderKind::Online,
                count: online_raw.len(),
            });
            rows.extend(
                online_raw
                    .into_iter()
                    .map(|raw| make_member_row(cx, raw, owner_id)),
            );
            rows.push(Row::Header {
                kind: HeaderKind::Offline,
                count: offline_raw.len(),
            });
            rows.extend(
                offline_raw
                    .into_iter()
                    .map(|raw| make_member_row(cx, raw, owner_id)),
            );
            rows
        }
    }
}

fn active_group_dm(cx: &App) -> Option<ChannelId> {
    let Route::DirectMessage { direct_id, .. } = Router::global(cx).read(cx).route() else {
        return None;
    };
    let is_group = DirectMessageStore::global(cx)
        .read(cx)
        .find(direct_id)
        .map(|dm| dm.kind == DirectKind::Group)
        .unwrap_or(false);
    is_group.then_some(direct_id)
}

struct ChannelContext {
    clan_id: ClanId,
    filter_ids: Option<Vec<UserId>>,
}

fn active_channel_context(cx: &App) -> Option<ChannelContext> {
    if matches!(
        Router::global(cx).read(cx).route(),
        Route::DirectMessage { .. } | Route::Direct | Route::Friends
    ) {
        return None;
    }
    let channel_list = ChannelList::global(cx);
    let channels = channel_list.read(cx);
    let (channel_id, use_filter, clan_id) = match channels.active_channel() {
        Some(channel) => {
            let is_thread = channel.parent_id.map(|p| !p.is_zero()).unwrap_or(false);
            (
                Some(channel.id),
                channel.private || is_thread,
                channel.clan_id,
            )
        }
        None => (
            None,
            false,
            ClanList::global(cx)
                .read(cx)
                .active_clan_id
                .unwrap_or_default(),
        ),
    };
    if clan_id.is_zero() {
        return None;
    }
    let filter_ids = if use_filter {
        channel_id.map(|cid| ChannelMembersStore::global(cx).read(cx).member_ids(cid))
    } else {
        None
    };
    Some(ChannelContext {
        clan_id,
        filter_ids,
    })
}

fn channel_raw_members(cx: &App, ctx: ChannelContext) -> (Vec<RawMember>, Vec<RawMember>) {
    let presence = PresenceStore::global(cx);
    let presence = presence.read(cx);
    let online = &presence.user_online;
    let store = ClanMembersStore::global(cx);
    let store = store.read(cx);
    let channel_list = ChannelList::global(cx);
    let channels = channel_list.read(cx);
    let pool: Vec<&ClanMember> = match &ctx.filter_ids {
        Some(ids) => ids
            .iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .collect(),
        None => store.members(ctx.clan_id),
    };
    let (online_ids, offline_ids) = split_members_by_status(&pool, online);
    let to_raw = |ids: &[UserId], is_online: bool| -> Vec<RawMember> {
        ids.iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .map(|member| RawMember {
                user_id: member.id(),
                name: member.name().to_string(),
                avatar_raw: member.avatar().to_string(),
                online: is_online,
                user_status: presence.user_status(member.id()).unwrap_or("").to_string(),
                in_voice: is_online && channels.in_voice_status(member.id()).is_some(),
            })
            .collect()
    };
    (to_raw(&online_ids, true), to_raw(&offline_ids, false))
}

fn group_raw_members(cx: &App, direct_id: ChannelId) -> Vec<RawMember> {
    let presence = PresenceStore::global(cx);
    let presence = presence.read(cx);
    let presence_online = &presence.user_online;
    let store = GroupMembersStore::global(cx);
    let store = store.read(cx);
    let mut members: Vec<&GroupMember> = store.members(direct_id).iter().collect();
    members.sort_by_cached_key(|m| m.name().to_lowercase());
    members
        .into_iter()
        .map(|member| RawMember {
            user_id: member.id(),
            name: member.name().to_string(),
            avatar_raw: member.avatar().to_string(),
            online: member.online || presence_online.contains(&member.id()),
            user_status: presence.user_status(member.id()).unwrap_or("").to_string(),
            in_voice: false,
        })
        .collect()
}

#[derive(Clone)]
pub(crate) struct MentionMemberRaw {
    pub user_id: String,
    pub display: String,
    pub username: String,
    pub avatar_raw: String,
    pub display_lc: String,
    pub username_lc: String,
    pub display_norm: String,
    pub username_norm: String,
}

fn active_dm(cx: &App) -> Option<ChannelId> {
    match Router::global(cx).read(cx).route() {
        Route::DirectMessage { direct_id, .. } => Some(direct_id),
        _ => None,
    }
}

fn mention_member_raw(
    user_id: String,
    name: &str,
    username: &str,
    avatar: &str,
) -> MentionMemberRaw {
    MentionMemberRaw {
        user_id,
        display: name.to_string(),
        username: username.to_string(),
        avatar_raw: avatar.to_string(),
        display_lc: name.to_lowercase(),
        username_lc: username.to_lowercase(),
        display_norm: normalize_search_string(name),
        username_norm: normalize_search_string(username),
    }
}

struct MentionChannelContext {
    clan_id: ClanId,
    private_channel: Option<ChannelId>,
}

fn mention_channel_context(cx: &App) -> Option<MentionChannelContext> {
    if active_dm(cx).is_some() {
        return None;
    }
    let channel_list = ChannelList::global(cx);
    let channels = channel_list.read(cx);
    let active = channels.active_channel()?;
    let target = match active.parent_id.filter(|parent| !parent.is_zero()) {
        Some(parent_id) => channels
            .channel(active.clan_id, parent_id)
            .unwrap_or(active),
        None => active,
    };
    if target.clan_id.is_zero() {
        return None;
    }
    Some(MentionChannelContext {
        clan_id: target.clan_id,
        private_channel: target.private.then_some(target.id),
    })
}

pub(crate) fn mention_role_clan(cx: &App) -> Option<ClanId> {
    mention_channel_context(cx).map(|ctx| ctx.clan_id)
}

pub(crate) fn mention_direct_id(cx: &App) -> Option<ChannelId> {
    active_dm(cx)
}

pub(crate) fn mention_private_channel(cx: &App) -> Option<ChannelId> {
    mention_channel_context(cx).and_then(|ctx| ctx.private_channel)
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MentionScope {
    direct_id: Option<ChannelId>,
    clan_id: Option<ClanId>,
    channel_id: Option<ChannelId>,
}

pub(crate) fn mention_scope(cx: &App) -> MentionScope {
    if let Some(direct_id) = active_dm(cx) {
        return MentionScope {
            direct_id: Some(direct_id),
            clan_id: None,
            channel_id: None,
        };
    }
    let channel_list = ChannelList::global(cx);
    let channels = channel_list.read(cx);
    MentionScope {
        direct_id: None,
        clan_id: ClanList::global(cx).read(cx).active_clan_id,
        channel_id: channels.active_channel().map(|channel| channel.id),
    }
}

fn direct_pair_pool(cx: &App, direct_id: ChannelId) -> Vec<MentionMemberRaw> {
    let store = DirectMessageStore::global(cx);
    let Some(dm) = store.read(cx).find(direct_id) else {
        return Vec::new();
    };
    let Some(peer_id) = dm.peer_user_id else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(2);
    if let Some(self_id) = BadgeService::global(cx).read(cx).current_user_id(cx)
        && let Some(account) = AccountStore::try_global(cx)
        && let Some(me) = account.read(cx).account.as_ref()
    {
        let display = if me.display_name.is_empty() {
            me.username.as_str()
        } else {
            me.display_name.as_str()
        };
        out.push(mention_member_raw(
            self_id.to_string(),
            display,
            &me.username,
            me.avatar_url.as_deref().unwrap_or_default(),
        ));
    }
    out.push(mention_member_raw(
        peer_id.to_string(),
        &dm.label,
        &dm.peer_username,
        &dm.avatar,
    ));
    out
}

pub(crate) fn mention_member_pool(cx: &App) -> Vec<MentionMemberRaw> {
    let mut pool = if let Some(direct_id) = active_group_dm(cx) {
        let store = GroupMembersStore::global(cx);
        let store = store.read(cx);
        store
            .members(direct_id)
            .iter()
            .map(|m| mention_member_raw(m.id().to_string(), m.name(), &m.user.username, m.avatar()))
            .collect::<Vec<_>>()
    } else if let Some(direct_id) = active_dm(cx) {
        direct_pair_pool(cx, direct_id)
    } else {
        let Some(ctx) = mention_channel_context(cx) else {
            return Vec::new();
        };
        let store = ClanMembersStore::global(cx);
        let store = store.read(cx);
        let members: Vec<&ClanMember> = match ctx.private_channel {
            Some(channel_id) => ChannelMembersStore::global(cx)
                .read(cx)
                .member_ids(channel_id)
                .iter()
                .filter_map(|id| store.member(ctx.clan_id, *id))
                .collect(),
            None => store.members(ctx.clan_id),
        };
        members
            .iter()
            .map(|m| {
                mention_member_raw(
                    m.user.id.to_string(),
                    m.name(),
                    &m.user.username,
                    m.avatar(),
                )
            })
            .collect::<Vec<_>>()
    };
    pool.sort_by(|a, b| a.display_lc.cmp(&b.display_lc));
    pool
}

pub(crate) fn ensure_mention_members_loaded(cx: &mut App) {
    if let Some(direct_id) = active_group_dm(cx) {
        GroupMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(direct_id, cx));
        return;
    }
    if let Some(channel_id) = mention_channel_context(cx).and_then(|ctx| ctx.private_channel) {
        ChannelMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(channel_id, cx));
    }
}

fn single_line(s: String) -> String {
    if s.contains(['\n', '\r']) {
        s.replace(['\n', '\r'], " ")
    } else {
        s
    }
}

fn make_member_row(cx: &App, raw: RawMember, owner_id: Option<UserId>) -> Row {
    let avatar_src = if raw.avatar_raw.is_empty() {
        SharedString::default()
    } else {
        SharedString::from(crate::util::imgproxy::avatar_url(cx, &raw.avatar_raw))
    };
    let id = raw.user_id.0;
    Row::Member(MemberRow {
        rcm_id: SharedString::from(format!("member-rcm-{id}")),
        user_id: raw.user_id,
        name: single_line(raw.name).into(),
        avatar_src,
        avatar_raw: raw.avatar_raw.into(),
        online: raw.online,
        user_status: single_line(raw.user_status).into(),
        in_voice: raw.in_voice,
        is_owner: owner_id == Some(raw.user_id),
    })
}

fn render_header(theme: &Theme, locale: &str, kind: &HeaderKind, count: usize) -> AnyElement {
    let label = match kind {
        HeaderKind::Members => {
            format!("{} - {}", mezon_i18n::t(locale, "common.members"), count).to_uppercase()
        }
        HeaderKind::Online => mezon_i18n::t(locale, "memberPage.onlineCount")
            .replace("{{count}}", &count.to_string())
            .to_uppercase(),
        HeaderKind::Offline => mezon_i18n::t(locale, "memberPage.offlineCount")
            .replace("{{count}}", &count.to_string())
            .to_uppercase(),
    };
    let label_size = match kind {
        HeaderKind::Members => px(12.),
        HeaderKind::Online | HeaderKind::Offline => px(14.),
    };
    div()
        .flex()
        .items_center()
        .px_4()
        .h(px(48.))
        .child(
            div()
                .text_size(label_size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(label),
        )
        .into_any_element()
}

fn render_member_skeleton(theme: &Theme, ix: usize) -> AnyElement {
    let fill = theme.bg_hover;
    let widths = [70., 52., 84., 61., 45., 76., 58., 90., 66., 50.];
    let name_width = widths[ix % widths.len()];
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(9.))
        .px_4()
        .h(px(48.))
        .child(div().size(px(32.)).rounded_full().bg(fill).flex_shrink_0())
        .child(div().h(px(14.)).w(px(name_width)).rounded(px(4.)).bg(fill))
        .into_any_element()
}

fn render_member(
    theme: &Theme,
    member: &MemberRow,
    in_voice_label: &SharedString,
    avatar_image_cache: &Entity<LruImageCache>,
    small_avatar_image_cache: &Entity<LruImageCache>,
    context: Option<ProfileContext>,
    settings: &Entity<Settings>,
    panel: WeakEntity<MemberListPanel>,
) -> AnyElement {
    let mut avatar = Avatar::new()
        .name(member.name.clone())
        .size_px(px(32.))
        .image_cache(small_avatar_image_cache.clone());
    if !member.avatar_src.is_empty() {
        avatar = avatar
            .src(member.avatar_src.clone())
            .fallback_src(member.avatar_raw.clone());
    }
    let avatar: AnyElement = if member.online {
        avatar.into_any_element()
    } else {
        div().opacity(0.5).child(avatar).into_any_element()
    };

    let dim = |mut color: Hsla| {
        if !member.online {
            color.a *= 0.5;
        }
        color
    };

    let dot_fill = dim(if member.online {
        theme.status_online.into()
    } else {
        theme.text_muted.into()
    });
    let dot_border = dim(theme.bg_secondary.into());
    let name_color = dim(rgb(DEFAULT_ROLE_COLOR).into());
    let owner_icon = member.is_owner.then(|| dim(rgb(0xF0B132).into()));
    let status_color = {
        let mut color: Hsla = theme.text_primary.into();
        color.a *= 0.6;
        color
    };
    let status = if member.in_voice {
        Some((in_voice_label.clone(), status_color))
    } else {
        (!member.user_status.is_empty()).then(|| (member.user_status.clone(), dim(status_color)))
    };
    let status_icon = member.in_voice.then(|| {
        let mut green: Hsla = rgb(0x22c55e).into();
        green.a *= 0.6;
        (IconName::Speaker, green)
    });

    let user_id = member.user_id;
    let display_name = member.name.clone();

    let mut row = MemberRowElement::new(member.rcm_id.clone(), member.name.clone(), avatar)
        .name_color(name_color)
        .dot(dot_fill, dot_border)
        .owner_icon(owner_icon)
        .status(status)
        .status_icon(status_icon)
        .on_right_click({
            let panel = panel.clone();
            move |position, _window, cx| {
                if let Some(p) = panel.upgrade() {
                    p.update(cx, |this, cx| {
                        this.open_menu = Some((user_id, display_name.clone(), position));
                        cx.notify();
                    });
                }
            }
        });

    if let Some(ctx) = context {
        let panel = panel.clone();
        let settings = settings.clone();
        let avatar_image_cache = avatar_image_cache.clone();
        row = row.on_click(move |position, window, cx| {
            open_profile_popover(
                &panel,
                user_id,
                ctx,
                position,
                settings.clone(),
                avatar_image_cache.clone(),
                window,
                cx,
            );
        });
    }

    row.into_any_element()
}

fn open_profile_popover(
    panel: &WeakEntity<MemberListPanel>,
    user_id: UserId,
    context: ProfileContext,
    position: Point<Pixels>,
    settings: Entity<Settings>,
    avatar_image_cache: Entity<LruImageCache>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(panel) = panel.upgrade() else {
        return;
    };
    let popover = cx.new(|cx| {
        UserProfilePopover::new(user_id, context, settings, avatar_image_cache, window, cx)
    });
    let handle = popover.read(cx).focus_handle(cx);
    window.focus(&handle, cx);
    let subscription = cx.subscribe(&popover, {
        let panel = panel.downgrade();
        move |_popover, _event: &DismissEvent, cx| {
            if let Some(p) = panel.upgrade() {
                p.update(cx, |this, cx| {
                    this.open_profile = None;
                    cx.notify();
                });
            }
        }
    });
    panel.update(cx, |this, cx| {
        this.open_profile = Some(ProfilePopoverState {
            popover,
            position,
            _subscription: subscription,
        });
        cx.notify();
    });
}

impl Render for MemberListPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("MemberListPanel");
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let count = self.rows.get().len();
        let entity = cx.entity();
        let avatar_image_cache = self.avatar_image_cache.clone();
        let small_avatar_image_cache = self.small_avatar_image_cache.clone();
        let context = self.active_context;
        let settings = self.settings.clone();
        let panel_weak = cx.entity().downgrade();
        let menu_overlay = self.open_menu.as_ref().map(|(user_id, display_name, pos)| {
            (
                *user_id,
                display_name.clone(),
                *pos,
                context,
                settings.clone(),
                locale.clone(),
                panel_weak.clone(),
            )
        });
        let profile_overlay = self
            .open_profile
            .as_ref()
            .map(|state| (state.popover.clone(), state.position));

        let in_voice_label: SharedString = mezon_i18n::t(&locale, "memberPage.inVoice").into();
        let list = uniform_list("member-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let locale = locale.clone();
            let rows = entity.read(cx).rows.get();
            range
                .map(|ix| match rows.get(ix) {
                    Some(Row::Header { kind, count }) => {
                        render_header(&theme, &locale, kind, *count)
                    }
                    Some(Row::Member(member)) => render_member(
                        &theme,
                        member,
                        &in_voice_label,
                        &avatar_image_cache,
                        &small_avatar_image_cache,
                        context,
                        &settings,
                        panel_weak.clone(),
                    ),
                    Some(Row::Skeleton) => render_member_skeleton(&theme, ix),
                    None => div().into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .with_item_size(size(px(245.), px(48.)))
        .smooth_line_scroll()
        .suppress_hover_while_scrolling()
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0()
        .pr(px(2.));

        div()
            .flex()
            .flex_col()
            .w(px(245.))
            .h_full()
            .flex_shrink_0()
            .bg(theme.bg_secondary)
            .border_l_1()
            .border_color(theme.border)
            .child(list)
            .when_some(
                menu_overlay,
                |el, (user_id, display_name, pos, ctx, settings, locale, panel)| {
                    el.child(context_menu_at(
                        pos,
                        build_member_menu(user_id, display_name, ctx, settings, panel, &locale),
                    ))
                },
            )
            .when_some(profile_overlay, |el, (popover, pos)| {
                el.child(deferred(
                    anchored()
                        .position(pos)
                        .anchor(Anchor::TopRight)
                        .snap_to_window()
                        .child(popover),
                ))
            })
    }
}

fn toast_coming_soon(settings: Entity<Settings>) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let locale = settings.read(cx).language.clone();
        let msg = mezon_i18n::t(&locale, "common.comingSoon").to_string();
        Shell::global(cx).update(cx, |shell, cx| shell.info(msg, cx));
    }
}

fn build_member_menu(
    user_id: UserId,
    display_name: SharedString,
    context: Option<ProfileContext>,
    settings: Entity<Settings>,
    panel: WeakEntity<MemberListPanel>,
    locale: &str,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let is_clan = matches!(context, Some(ProfileContext::Clan(_)));

    let dismiss = {
        let panel = panel.clone();
        move |_window: &mut Window, cx: &mut App| {
            if let Some(p) = panel.upgrade() {
                p.update(cx, |this, cx| {
                    this.open_menu = None;
                    cx.notify();
                });
            }
        }
    };

    let remove_from_thread_label = mezon_i18n::t(locale, "contextMenu.member.removeFromThread")
        .replace("{{username}}", display_name.as_ref());

    let mut menu = ContextMenu::new()
        .on_dismiss(dismiss)
        .item(
            t("contextMenu.member.profile"),
            toast_coming_soon(settings.clone()),
        )
        .item(
            t("contextMenu.member.message"),
            toast_coming_soon(settings.clone()),
        )
        .item(t("contextMenu.member.shareContact"), {
            let settings = settings.clone();
            let display_name = display_name.clone();
            let panel = panel.clone();
            move |window, cx| {
                let contact = share_contact_subject(user_id, display_name.as_ref(), context, cx);
                let locale = settings.read(cx).language.clone().into();
                ShareContactModal::open(contact, locale, window, cx);
                if let Some(p) = panel.upgrade() {
                    p.update(cx, |this, cx| {
                        this.open_menu = None;
                        cx.notify();
                    });
                }
            }
        })
        .item(
            t("contextMenu.member.addFriend"),
            toast_coming_soon(settings.clone()),
        )
        .separator()
        .danger_item(
            t("contextMenu.member.removeFriend"),
            toast_coming_soon(settings.clone()),
        );

    if is_clan {
        menu = menu
            .separator()
            .danger_item(
                t("contextMenu.member.banChat"),
                toast_coming_soon(settings.clone()),
            )
            .danger_item(
                t("contextMenu.member.kick"),
                toast_coming_soon(settings.clone()),
            )
            .danger_item(remove_from_thread_label, toast_coming_soon(settings));
    }

    menu
}

#[cfg(test)]
mod scroll_tests {
    use std::cell::Cell;
    use std::ops::Range;
    use std::rc::Rc;
    use std::time::Duration;

    use gpui::{
        AppContext, Context, IntoElement, ListAlignment, ListState, MouseMoveEvent, Render,
        ScrollDelta, ScrollWheelEvent, Styled, TestAppContext, UniformListScrollHandle, Window,
        div, list, point, px, size, uniform_list,
    };

    struct TestView {
        scroll: UniformListScrollHandle,
        probes: Rc<Cell<usize>>,
    }

    struct VariableTestView {
        scroll: ListState,
    }

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let probes = self.probes.clone();
            uniform_list(
                "member-list-scroll-test",
                10,
                move |range: Range<usize>, _, _| {
                    if range == (0..1) {
                        probes.set(probes.get() + 1);
                    }
                    range.map(|_| div().h(px(48.)).w_full()).collect::<Vec<_>>()
                },
            )
            .with_item_size(size(px(100.), px(48.)))
            .smooth_line_scroll()
            .suppress_hover_while_scrolling()
            .track_scroll(&self.scroll)
            .size_full()
        }
    }

    impl Render for VariableTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            list(self.scroll.clone(), |_, _, _| {
                div().h(px(48.)).w_full().into_any_element()
            })
            .size_full()
        }
    }

    #[gpui::test]
    fn member_list_skips_size_probe_and_smooths_only_line_wheel(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let scroll = UniformListScrollHandle::new();
        let probes = Rc::new(Cell::new(0));
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, cx| {
            cx.new(|_| TestView {
                scroll: scroll.clone(),
                probes: probes.clone(),
            })
            .into_any_element()
        });

        assert_eq!(probes.get(), 0);
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        });
        assert!(scroll.is_smooth_wheel_scrolling());
        assert!(scroll.is_scroll_hover_suppressed());

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
            ..Default::default()
        });
        assert!(!scroll.is_smooth_wheel_scrolling());
        assert!(scroll.is_scroll_hover_suppressed());

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, cx| {
            cx.new(|_| TestView {
                scroll: scroll.clone(),
                probes: probes.clone(),
            })
            .into_any_element()
        });
        cx.executor().advance_clock(Duration::from_millis(350));
        cx.run_until_parked();
        assert!(!scroll.is_scroll_hover_active());
        assert!(scroll.is_scroll_hover_suppressed());
        cx.simulate_event(MouseMoveEvent {
            position: point(px(51.), px(100.)),
            ..Default::default()
        });
        assert!(!scroll.is_scroll_hover_suppressed());

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
            ..Default::default()
        });
        assert!(scroll.is_scroll_hover_suppressed());
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, cx| {
            cx.new(|_| TestView {
                scroll: scroll.clone(),
                probes: probes.clone(),
            })
            .into_any_element()
        });
        cx.executor().advance_clock(Duration::from_millis(350));
        cx.run_until_parked();
        assert!(!scroll.is_scroll_hover_active());
        assert!(scroll.is_scroll_hover_suppressed());
        cx.simulate_event(MouseMoveEvent {
            position: point(px(52.), px(100.)),
            ..Default::default()
        });
        assert!(!scroll.is_scroll_hover_suppressed());
    }

    #[gpui::test]
    fn variable_list_suppresses_hover_for_the_scroll_gesture(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let scroll = ListState::new(10, ListAlignment::Top, px(0.))
            .smooth_line_scroll()
            .suppress_hover_while_scrolling();
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, cx| {
            cx.new(|_| VariableTestView {
                scroll: scroll.clone(),
            })
            .into_any_element()
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        });
        assert!(scroll.is_scroll_hover_suppressed());

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
            ..Default::default()
        });

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, cx| {
            cx.new(|_| VariableTestView {
                scroll: scroll.clone(),
            })
            .into_any_element()
        });
        cx.executor().advance_clock(Duration::from_millis(350));
        cx.run_until_parked();
        assert!(!scroll.is_scroll_hover_active());
        assert!(scroll.is_scroll_hover_suppressed());
        cx.simulate_event(MouseMoveEvent {
            position: point(px(51.), px(100.)),
            ..Default::default()
        });
        assert!(!scroll.is_scroll_hover_suppressed());
    }
}
