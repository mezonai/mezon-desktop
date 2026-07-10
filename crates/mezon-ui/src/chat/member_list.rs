use std::time::Duration;

use gpui::{
    AnyElement, App, Context, DismissEvent, Entity, Focusable, FontWeight, Hsla, Pixels, Point,
    SharedString, Subscription, Task, UniformListScrollHandle, WeakEntity, Window, anchored,
    deferred, div, prelude::*, px, rgb, uniform_list,
};
use mezon_store::{
    ChannelEvent, ChannelId, ChannelList, ChannelMembersEvent, ChannelMembersStore, ClanId,
    ClanList, ClanMember, ClanMembersEvent, ClanMembersStore, DirectKind, DirectMessageStore,
    GroupMember, GroupMembersEvent, GroupMembersStore, PresenceEvent, PresenceStore,
    ProfileContext, Settings, UserId, split_members_by_status,
};
use ui::utils::ROUNDED_BORDER_WINDOW;

use crate::app::shell::Shell;
use crate::chat::member_row_element::MemberRowElement;
use crate::chat::user_profile_popover::UserProfilePopover;
use crate::components::primitives::{Avatar, ContextMenu, context_menu_at};
use crate::image_cache::LruImageCache;
use crate::router::{Route, Router};
use crate::theme::{ActiveTheme, Theme};
use crate::util::reactive::Derived;

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
    is_owner: bool,
    rcm_id: SharedString,
}

struct RawMember {
    user_id: UserId,
    name: String,
    avatar_raw: String,
    online: bool,
    user_status: String,
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
        cx.observe(&Router::global(cx), |this, _, cx| {
            let key = route_key(this.source, cx);
            if key != this.route_key {
                this.route_key = key;
                this.rebuild(cx);
            }
        })
        .detach();
        cx.subscribe(
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
        )
        .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();

        match source {
            MemberSource::Channel => {
                cx.subscribe(&ClanMembersStore::global(cx), |this, _, event, cx| {
                    let ClanMembersEvent::Changed { clan_id } = event;
                    if shows_clan(*clan_id, cx) {
                        this.rebuild(cx);
                    }
                })
                .detach();
                cx.subscribe(&ChannelMembersStore::global(cx), |this, _, event, cx| {
                    let ChannelMembersEvent::Changed { channel_id } = event;
                    if shows_channel(*channel_id, cx) {
                        this.rebuild(cx);
                    }
                })
                .detach();
                cx.subscribe(&ChannelList::global(cx), |this, _, event, cx| {
                    if let ChannelEvent::ActiveChannelChanged(_) = event {
                        this.rebuild(cx);
                    }
                })
                .detach();
            }
            MemberSource::Group => {
                cx.subscribe(&GroupMembersStore::global(cx), |this, _, event, cx| {
                    let GroupMembersEvent::Changed { channel_id } = event;
                    if shows_group(*channel_id, cx) {
                        this.rebuild(cx);
                    }
                })
                .detach();
                cx.observe(&DirectMessageStore::global(cx), |this, _, cx| {
                    this.rebuild(cx)
                })
                .detach();
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
    pub avatar_src: SharedString,
}

fn mention_avatar_src(cx: &App, avatar: &str) -> SharedString {
    if avatar.is_empty() {
        SharedString::default()
    } else {
        SharedString::from(crate::util::imgproxy::avatar_url(cx, avatar))
    }
}

pub(crate) fn mention_member_pool(cx: &App) -> Vec<MentionMemberRaw> {
    if let Some(direct_id) = active_group_dm(cx) {
        let store = GroupMembersStore::global(cx);
        let store = store.read(cx);
        return store
            .members(direct_id)
            .iter()
            .map(|m| MentionMemberRaw {
                user_id: m.id().to_string(),
                display: m.name().to_string(),
                username: m.user.username.clone(),
                avatar_raw: m.avatar().to_string(),
                display_lc: m.name().to_lowercase(),
                username_lc: m.user.username.to_lowercase(),
                avatar_src: mention_avatar_src(cx, m.avatar()),
            })
            .collect();
    }
    let Some(ctx) = active_channel_context(cx) else {
        return Vec::new();
    };
    let store = ClanMembersStore::global(cx);
    let store = store.read(cx);
    let pool: Vec<&ClanMember> = match &ctx.filter_ids {
        Some(ids) => ids
            .iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .collect(),
        None => store.members(ctx.clan_id),
    };
    pool.iter()
        .map(|m| MentionMemberRaw {
            user_id: m.user.id.to_string(),
            display: m.name().to_string(),
            username: m.user.username.clone(),
            avatar_raw: m.avatar().to_string(),
            display_lc: m.name().to_lowercase(),
            username_lc: m.user.username.to_lowercase(),
            avatar_src: mention_avatar_src(cx, m.avatar()),
        })
        .collect()
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
    let status = (!member.user_status.is_empty()).then(|| {
        let mut color: Hsla = theme.text_primary.into();
        color.a *= 0.6;
        (member.user_status.clone(), dim(color))
    });

    let user_id = member.user_id;
    let display_name = member.name.clone();

    let mut row = MemberRowElement::new(member.rcm_id.clone(), member.name.clone(), avatar)
        .name_color(name_color)
        .dot(dot_fill, dot_border)
        .owner_icon(owner_icon)
        .status(status)
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
            .rounded_br(px(ROUNDED_BORDER_WINDOW))
            .border_l_1()
            .border_color(theme.border)
            .child(list)
            .when_some(
                menu_overlay,
                |el, (_user_id, display_name, pos, ctx, settings, locale, panel)| {
                    el.child(context_menu_at(
                        pos,
                        build_member_menu(display_name, ctx, settings, panel, &locale),
                    ))
                },
            )
            .when_some(profile_overlay, |el, (popover, pos)| {
                el.child(deferred(
                    anchored().position(pos).snap_to_window().child(popover),
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
        .item(
            t("contextMenu.member.shareContact"),
            toast_coming_soon(settings.clone()),
        )
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
