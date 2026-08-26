use std::time::Duration;

use gpui::{
    Anchor, AnyElement, App, Context, DismissEvent, Entity, Focusable, FontWeight, Hsla, Pixels,
    Point, SharedString, Subscription, Task, UniformListScrollHandle, WeakEntity, Window, anchored,
    deferred, div, prelude::*, px, rgb, size, uniform_list,
};
use mezon_store::{
    AccountEvent, AccountStore, BAN_FOR_1_HOUR_SEC, BAN_FOR_3_HOURS_SEC, BAN_FOR_8_HOURS_SEC,
    BAN_FOR_15_MINUTES_SEC, BAN_FOR_24_HOURS_SEC, BAN_FOREVER, BadgeService, BannedUsersEvent,
    BannedUsersStore, ChannelEvent, ChannelId, ChannelList, ChannelMembersEvent,
    ChannelMembersStore, ChannelType, ClanId, ClanList, ClanMember, ClanMembersStore, DirectEvent,
    DirectKind, DirectMessageStore, DmAvatarPresence, FriendState, FriendStore, GroupMember,
    GroupMembersEvent, GroupMembersStore, PERMISSION_ADMINISTRATOR, PERMISSION_CLAN_OWNER,
    PermissionStore, PresenceEvent, PresenceStore, ProfileContext, RolesEvent, RolesStore,
    Settings, UserId, current_user_presence, current_user_status, split_members_by_status,
};

use crate::app::shell::{FriendRemovalKind, Shell};
use crate::chat::friends_page::open_dm_with_user;
use crate::chat::member_row_element::{MemberRowElement, RowDot};
use crate::chat::message::{ShareContactModal, share_contact_subject};
use crate::chat::role_style::{role_color_in, role_fallback_color};
use crate::chat::user_profile_modal::UserProfileModal;
use crate::chat::user_profile_popover::UserProfilePopover;
use crate::components::primitives::{
    Avatar, ContextMenu, IconName, SubmenuOption, context_menu_at,
};
use crate::image_cache::{LruImageCache, shared_avatar_cache};
use crate::router::{Route, Router};
use crate::theme::{ActiveTheme, Theme};
use crate::util::reactive::Derived;
use crate::util::text_utils::normalize_search_string;

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
    presence: DmAvatarPresence,
    user_status: SharedString,
    in_voice: bool,
    is_owner: bool,
    role_color: Option<Hsla>,
    rcm_id: SharedString,
}

struct RawMember {
    user_id: UserId,
    name: String,
    avatar_raw: String,
    online: bool,
    presence: DmAvatarPresence,
    user_status: String,
    in_voice: bool,
    role_color: Option<Hsla>,
}

struct ProfilePopoverState {
    popover: Entity<UserProfilePopover>,
    position: Point<Pixels>,
    _subscription: Subscription,
}

struct MemberMenuState {
    user_id: UserId,
    display_name: SharedString,
    position: Point<Pixels>,
    ban_sub_open: bool,
}

struct ActiveMemberList(WeakEntity<MemberListPanel>);
impl gpui::Global for ActiveMemberList {}

pub struct MemberListPanel {
    source: MemberSource,
    settings: Entity<Settings>,
    rows: Derived<Vec<Row>>,
    list_scroll: UniformListScrollHandle,
    avatar_image_cache: Entity<LruImageCache>,
    small_avatar_image_cache: Entity<LruImageCache>,
    active_context: Option<ProfileContext>,
    route_key: RouteKey,
    open_menu: Option<MemberMenuState>,
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
    pub(crate) fn profile_popover_open(&self) -> bool {
        self.open_profile.is_some()
    }

    fn menu_args(&self, panel: WeakEntity<Self>, cx: &App) -> Option<MemberMenuArgs> {
        let state = self.open_menu.as_ref()?;
        Some(MemberMenuArgs {
            user_id: state.user_id,
            display_name: state.display_name.clone(),
            position: state.position,
            ban_sub_open: state.ban_sub_open,
            context: self.active_context,
            settings: self.settings.clone(),
            locale: self.settings.read(cx).language.clone(),
            panel,
            permissions: MemberMenuPermissions::resolve(state.user_id, self.active_context, cx),
        })
    }

    fn probe_open_menu(
        &mut self,
        user_id: UserId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let display_name = self
            .rows
            .get()
            .iter()
            .find_map(|row| match row {
                Row::Member(member) if member.user_id == user_id => Some(member.name.clone()),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("user {user_id} is not in the visible member list"))?;
        self.open_menu = Some(MemberMenuState {
            user_id,
            display_name,
            position,
            ban_sub_open: false,
        });
        self.ensure_ban_list_loaded(user_id, cx);
        cx.notify();
        Ok(())
    }

    fn probe_close_menu(&mut self, cx: &mut Context<Self>) {
        self.open_menu = None;
        cx.notify();
    }

    fn ensure_ban_list_loaded(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        let permissions = MemberMenuPermissions::resolve(user_id, self.active_context, cx);
        let (Some(clan_id), Some(channel_id)) = (permissions.clan_id, permissions.channel_id)
        else {
            return;
        };
        if !permissions.show_ban {
            return;
        }
        BannedUsersStore::global(cx)
            .update(cx, |store, cx| store.ensure_loaded(clan_id, channel_id, cx));
    }

    pub fn new(
        source: MemberSource,
        settings: Entity<Settings>,
        avatar_image_cache: Entity<LruImageCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = Vec::new();
        subs.push(
            cx.subscribe(&BannedUsersStore::global(cx), |this, _, event, cx| {
                if !matches!(
                    event,
                    BannedUsersEvent::BanFailed | BannedUsersEvent::UnbanFailed
                ) {
                    return;
                }
                let locale = this.settings.read(cx).language.clone();
                let message = mezon_i18n::t(&locale, "common.somethingWentWrong").to_string();
                Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
            }),
        );
        subs.push(cx.observe(&BannedUsersStore::global(cx), |this, _, cx| {
            if this.open_menu.is_some() {
                cx.notify();
            }
        }));
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
        if let Some(account) = AccountStore::try_global(cx) {
            subs.push(cx.subscribe(&account, |this, _, event, cx| {
                if matches!(
                    event,
                    AccountEvent::StatusUpdated | AccountEvent::AccountLoaded
                ) {
                    this.rebuild(cx);
                }
            }));
        }

        match source {
            MemberSource::Channel => {
                subs.push(
                    cx.subscribe(&ClanMembersStore::global(cx), |this, _, event, cx| {
                        let clan_id = &event.clan_id();
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
                subs.push(cx.subscribe(&RolesStore::global(cx), |this, _, event, cx| {
                    let RolesEvent::Changed { clan_id } = event else {
                        return;
                    };
                    if shows_clan(*clan_id, cx) {
                        this.rebuild(cx);
                    }
                }));
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

        cx.set_global(ActiveMemberList(cx.entity().downgrade()));

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
    let roles = RolesStore::try_global(cx);
    let roles = roles.as_ref().map(|roles| roles.read(cx));
    let pool: Vec<&ClanMember> = match &ctx.filter_ids {
        Some(ids) => ids
            .iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .collect(),
        None => store.members(ctx.clan_id),
    };
    let own = current_user_status(cx);
    let own_presence = current_user_presence(cx);
    let (online_ids, offline_ids) = split_members_by_status(
        &pool,
        online,
        own.as_ref().map(|(id, status)| (*id, status.online)),
    );
    let to_raw = |ids: &[UserId], is_online: bool| -> Vec<RawMember> {
        ids.iter()
            .filter_map(|id| store.member(ctx.clan_id, *id))
            .map(|member| {
                let own_status = own
                    .as_ref()
                    .filter(|(id, _)| *id == member.id())
                    .map(|(_, status)| status);
                RawMember {
                    user_id: member.id(),
                    name: member.name().to_string(),
                    avatar_raw: member.avatar().to_string(),
                    online: is_online,
                    presence: presence.member_presence(member.id(), own_presence),
                    user_status: own_status.map_or_else(
                        || presence.user_status(member.id()).unwrap_or("").to_string(),
                        |status| status.custom_status.clone(),
                    ),
                    in_voice: is_online && channels.in_voice_status(member.id()).is_some(),
                    role_color: Some(role_color_in(roles, ctx.clan_id, member.id())),
                }
            })
            .collect()
    };
    (to_raw(&online_ids, true), to_raw(&offline_ids, false))
}

fn raw_member_json(member: &RawMember) -> serde_json::Value {
    serde_json::json!({
        "user_id": member.user_id.to_string(),
        "name": member.name,
        "online": member.online,
        "presence": format!("{:?}", member.presence),
        "user_status": member.user_status,
        "in_voice": member.in_voice,
    })
}

fn member_section_json(label: &str, members: &[RawMember]) -> serde_json::Value {
    serde_json::json!({
        "label": label,
        "header_count": members.len(),
        "rows": members.iter().map(raw_member_json).collect::<Vec<_>>(),
    })
}

fn active_member_panel(cx: &App) -> anyhow::Result<Entity<MemberListPanel>> {
    cx.try_global::<ActiveMemberList>()
        .and_then(|active| active.0.upgrade())
        .ok_or_else(|| anyhow::anyhow!("no member list is mounted; open a channel or group DM"))
}

fn member_menu_json(panel: &Entity<MemberListPanel>, cx: &App) -> serde_json::Value {
    let this = panel.read(cx);
    let Some(state) = this.open_menu.as_ref() else {
        return serde_json::json!({ "open": false, "items": [] });
    };
    let Some(args) = this.menu_args(panel.downgrade(), cx) else {
        return serde_json::json!({ "open": false, "items": [] });
    };
    let user_id = args.user_id.to_string();
    let display_name = args.display_name.to_string();
    let permissions = args.permissions;
    let items = build_member_menu(args)
        .probe_items()
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            serde_json::json!({
                "index": index,
                "kind": item.kind,
                "label": item.label,
                "disabled": item.disabled,
                "options": item
                    .options
                    .into_iter()
                    .map(|(value, label)| serde_json::json!({ "value": value, "label": label }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "open": true,
        "user_id": user_id,
        "display_name": display_name,
        "position": { "x": f32::from(state.position.x), "y": f32::from(state.position.y) },
        "ban_submenu_open": state.ban_sub_open,
        "permissions": {
            "is_self": permissions.is_self,
            "is_friend": permissions.is_friend,
            "is_blocked": permissions.is_blocked,
            "is_banned": permissions.is_banned,
            "show_ban": permissions.show_ban,
            "show_kick": permissions.show_kick,
            "show_remove_from_thread": permissions.show_remove_from_thread,
            "clan_id": permissions.clan_id.map(|id| id.to_string()),
            "channel_id": permissions.channel_id.map(|id| id.to_string()),
        },
        "items": items,
    })
}

pub fn member_menu_state(cx: &App) -> anyhow::Result<serde_json::Value> {
    let panel = active_member_panel(cx)?;
    Ok(member_menu_json(&panel, cx))
}

pub fn member_menu_open(
    user_id: UserId,
    position: Point<Pixels>,
    cx: &mut App,
) -> anyhow::Result<serde_json::Value> {
    let panel = active_member_panel(cx)?;
    panel.update(cx, |this, cx| this.probe_open_menu(user_id, position, cx))?;
    Ok(member_menu_json(&panel, cx))
}

pub fn member_menu_close(cx: &mut App) -> anyhow::Result<serde_json::Value> {
    let panel = active_member_panel(cx)?;
    panel.update(cx, |this, cx| this.probe_close_menu(cx));
    Ok(member_menu_json(&panel, cx))
}

pub fn member_menu_pick(
    index: usize,
    value: Option<i32>,
    window: &mut Window,
    cx: &mut App,
) -> anyhow::Result<serde_json::Value> {
    let panel = active_member_panel(cx)?;
    let args = panel
        .read(cx)
        .menu_args(panel.downgrade(), cx)
        .ok_or_else(|| anyhow::anyhow!("member menu is not open; call member_menu_open first"))?;
    let menu = build_member_menu(args);
    let picked = menu
        .probe_items()
        .get(index)
        .map(|item| (item.kind, item.label.clone()))
        .ok_or_else(|| anyhow::anyhow!("no menu item at index {index}"))?;
    menu.probe_activate(index, value, window, cx)?;
    Ok(serde_json::json!({
        "ok": true,
        "index": index,
        "kind": picked.0,
        "label": picked.1,
        "value": value,
        "menu_open": panel.read(cx).open_menu.is_some(),
    }))
}

pub fn member_list_snapshot(cx: &App) -> serde_json::Value {
    if let Some(direct_id) = active_group_dm(cx) {
        let members = group_raw_members(cx, direct_id);
        return serde_json::json!({
            "source": "group",
            "sections": [member_section_json("MEMBERS", &members)],
        });
    }
    let Some(ctx) = active_channel_context(cx) else {
        return serde_json::json!({ "source": "none", "sections": [] });
    };
    let (online, offline) = channel_raw_members(cx, ctx);
    serde_json::json!({
        "source": "channel",
        "sections": [
            member_section_json("ONLINE", &online),
            member_section_json("OFFLINE", &offline),
        ],
    })
}

fn group_raw_members(cx: &App, direct_id: ChannelId) -> Vec<RawMember> {
    let presence = PresenceStore::global(cx);
    let presence = presence.read(cx);
    let presence_online = &presence.user_online;
    let store = GroupMembersStore::global(cx);
    let store = store.read(cx);
    let mut members: Vec<&GroupMember> = store.members(direct_id).iter().collect();
    members.sort_by_cached_key(|m| m.name().to_lowercase());
    let own = current_user_status(cx);
    let own_presence = current_user_presence(cx);
    members
        .into_iter()
        .map(|member| {
            let own_status = own
                .as_ref()
                .filter(|(id, _)| *id == member.id())
                .map(|(_, status)| status);
            let online = own_status.map_or_else(
                || member.online || presence_online.contains(&member.id()),
                |status| status.online,
            );
            RawMember {
                user_id: member.id(),
                name: member.name().to_string(),
                avatar_raw: member.avatar().to_string(),
                online,
                presence: if online {
                    presence.member_presence(member.id(), own_presence)
                } else {
                    DmAvatarPresence::None
                },
                user_status: own_status.map_or_else(
                    || presence.user_status(member.id()).unwrap_or("").to_string(),
                    |status| status.custom_status.clone(),
                ),
                in_voice: false,
                role_color: None,
            }
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
        presence: raw.presence,
        user_status: single_line(raw.user_status).into(),
        in_voice: raw.in_voice,
        is_owner: owner_id == Some(raw.user_id),
        role_color: raw.role_color,
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

    let dot = match crate::util::user_status::presence_badge_color(member.presence) {
        None => RowDot::None,
        Some(color) if member.presence == DmAvatarPresence::Idle => RowDot::Icon {
            icon: IconName::DarkModeIcon,
            color: color.into(),
        },
        Some(color) => RowDot::Fill {
            color: color.into(),
            ring: theme.bg_secondary.into(),
        },
    };
    let name_color = dim(member.role_color.unwrap_or_else(role_fallback_color));
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
        .dot(dot)
        .owner_icon(owner_icon)
        .status(status)
        .status_icon(status_icon)
        .on_right_click({
            let panel = panel.clone();
            move |position, _window, cx| {
                if let Some(p) = panel.upgrade() {
                    p.update(cx, |this, cx| {
                        this.open_menu = Some(MemberMenuState {
                            user_id,
                            display_name: display_name.clone(),
                            position,
                            ban_sub_open: false,
                        });
                        this.ensure_ban_list_loaded(user_id, cx);
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
        let menu_overlay = self.menu_args(panel_weak.clone(), cx);
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
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.surfaces.direct_message.ramp())
            .child(list)
            .when_some(menu_overlay, |el, args| {
                let position = args.position;
                el.child(context_menu_at(position, build_member_menu(args)))
            })
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

#[derive(Clone, Copy, Default)]
struct MemberMenuPermissions {
    is_self: bool,
    show_ban: bool,
    show_kick: bool,
    show_remove_from_thread: bool,
    is_friend: bool,
    is_blocked: bool,
    blocked_by_me: bool,
    is_banned: bool,
    clan_id: Option<ClanId>,
    channel_id: Option<ChannelId>,
}

impl MemberMenuPermissions {
    fn resolve(user_id: UserId, context: Option<ProfileContext>, cx: &App) -> Self {
        let me = BadgeService::global(cx).read(cx).current_user_id(cx);
        let is_self = me == Some(user_id);
        let (is_friend, is_blocked, blocked_by_me) = FriendStore::try_global(cx)
            .and_then(|store| {
                store.read(cx).friend(user_id).map(|friend| {
                    let blocked = friend.state == FriendState::Blocked;
                    (
                        friend.state == FriendState::Friend,
                        blocked,
                        blocked && Some(friend.source_id) == me,
                    )
                })
            })
            .unwrap_or((false, false, false));
        let Some(ProfileContext::Clan(clan_id)) = context else {
            return Self {
                is_self,
                is_friend,
                is_blocked,
                blocked_by_me,
                ..Default::default()
            };
        };
        let (has_clan_owner, has_administrator) = match PermissionStore::try_global(cx) {
            Some(store) => {
                let store = store.read(cx);
                (
                    store.check(clan_id, None, PERMISSION_CLAN_OWNER, cx),
                    store.check(clan_id, None, PERMISSION_ADMINISTRATOR, cx),
                )
            }
            None => (false, false),
        };
        let member_is_clan_owner = ClanList::global(cx)
            .read(cx)
            .clan_by_id(clan_id)
            .is_some_and(|clan| clan.creator_id == user_id);
        let active_channel = ChannelList::global(cx)
            .read(cx)
            .active_channel()
            .map(|channel| (channel.id, channel.channel_type, channel.creator_id));
        let channel_id = active_channel.map(|(id, _, _)| id);
        let is_thread = active_channel
            .is_some_and(|(_, channel_type, _)| matches!(channel_type, ChannelType::Thread));
        let is_channel_creator =
            me.is_some() && active_channel.map(|(_, _, creator_id)| creator_id) == me;
        let elevated = has_clan_owner || (has_administrator && !member_is_clan_owner);
        let is_banned = channel_id.is_some_and(|channel_id| {
            BannedUsersStore::try_global(cx)
                .is_some_and(|store| store.read(cx).is_banned(channel_id, user_id))
        });
        Self {
            is_self,
            show_ban: has_administrator && !is_self,
            show_kick: !is_self && elevated,
            show_remove_from_thread: !is_self && is_thread && (is_channel_creator || elevated),
            is_friend,
            is_blocked,
            blocked_by_me,
            is_banned,
            clan_id: Some(clan_id),
            channel_id,
        }
    }
}

struct MemberMenuArgs {
    user_id: UserId,
    display_name: SharedString,
    position: Point<Pixels>,
    ban_sub_open: bool,
    context: Option<ProfileContext>,
    settings: Entity<Settings>,
    locale: String,
    panel: WeakEntity<MemberListPanel>,
    permissions: MemberMenuPermissions,
}

fn close_member_submenus(
    panel: WeakEntity<MemberListPanel>,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let _ = panel.update(cx, |this, cx| {
            if let Some(menu) = this.open_menu.as_mut()
                && menu.ban_sub_open
            {
                menu.ban_sub_open = false;
                cx.notify();
            }
        });
    }
}

fn close_member_menu(panel: &WeakEntity<MemberListPanel>, cx: &mut App) {
    if let Some(panel) = panel.upgrade() {
        panel.update(cx, |this, cx| {
            this.open_menu = None;
            cx.notify();
        });
    }
}

const BAN_DURATIONS: &[(i32, &str)] = &[
    (BAN_FOR_15_MINUTES_SEC, "contextMenu.muteFor15Minutes"),
    (BAN_FOR_1_HOUR_SEC, "contextMenu.muteFor1Hour"),
    (BAN_FOR_3_HOURS_SEC, "contextMenu.muteFor3Hours"),
    (BAN_FOR_8_HOURS_SEC, "contextMenu.muteFor8Hours"),
    (BAN_FOR_24_HOURS_SEC, "contextMenu.muteFor24Hours"),
    (BAN_FOREVER, "contextMenu.muteUntilTurnedBack"),
];

fn ban_duration_options(locale: &str) -> Vec<SubmenuOption> {
    BAN_DURATIONS
        .iter()
        .map(|(value, key)| SubmenuOption {
            value: *value,
            label: mezon_i18n::t(locale, key).into(),
            selected: false,
            disabled: false,
        })
        .collect()
}

fn build_member_menu(args: MemberMenuArgs) -> ContextMenu {
    let MemberMenuArgs {
        user_id,
        display_name,
        position: _,
        ban_sub_open,
        context,
        settings,
        locale,
        panel,
        permissions,
    } = args;
    let t = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
    let is_clan = matches!(context, Some(ProfileContext::Clan(_)));
    let is_self = permissions.is_self;

    let dismiss = {
        let panel = panel.clone();
        move |_window: &mut Window, cx: &mut App| close_member_menu(&panel, cx)
    };

    let remove_from_thread_label = mezon_i18n::t(&locale, "contextMenu.member.removeFromThread")
        .replace("{{username}}", display_name.as_ref());

    let mut menu = ContextMenu::new()
        .on_submenu_close(close_member_submenus(panel.clone()))
        .on_dismiss(dismiss)
        .item(t("contextMenu.member.profile"), {
            let panel = panel.clone();
            let settings = settings.clone();
            move |_window: &mut Window, cx: &mut App| {
                let clan_id = match context {
                    Some(ProfileContext::Clan(clan_id)) => clan_id,
                    _ => ClanId::default(),
                };
                close_member_menu(&panel, cx);
                if let Some(p) = panel.upgrade() {
                    p.update(cx, |this, cx| {
                        this.open_profile = None;
                        cx.notify();
                    });
                }
                let avatar_image_cache = shared_avatar_cache(cx);
                let settings = settings.clone();
                let modal = cx.new(|cx| {
                    UserProfileModal::new(user_id, clan_id, settings, avatar_image_cache, cx)
                });
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.show_fullscreen_modal(modal.into(), cx);
                });
            }
        });

    if !is_self {
        menu = menu.item(t("contextMenu.member.message"), {
            let panel = panel.clone();
            let locale = locale.clone();
            move |_window: &mut Window, cx: &mut App| {
                let error: SharedString =
                    mezon_i18n::t(&locale, "shareContact.card.messageError").into();
                open_dm_with_user(user_id, error, cx);
                close_member_menu(&panel, cx);
            }
        });

        if permissions.is_friend && !permissions.is_blocked {
            menu = menu.item(t("contextMenu.member.shareContact"), {
                let settings = settings.clone();
                let display_name = display_name.clone();
                let panel = panel.clone();
                move |window, cx| {
                    let contact =
                        share_contact_subject(user_id, display_name.as_ref(), context, cx);
                    let locale = settings.read(cx).language.clone().into();
                    ShareContactModal::open(contact, locale, window, cx);
                    close_member_menu(&panel, cx);
                }
            });
        }

        if !permissions.is_friend && !permissions.is_blocked {
            menu = menu.item(t("contextMenu.member.addFriend"), {
                let panel = panel.clone();
                let display_name = display_name.clone();
                let locale = locale.clone();
                move |_window: &mut Window, cx: &mut App| {
                    let contact =
                        share_contact_subject(user_id, display_name.as_ref(), context, cx);
                    if contact.username.is_empty() {
                        let message =
                            mezon_i18n::t(&locale, "friends.toast.sendAddFriendFail").to_string();
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                    } else {
                        FriendStore::global(cx).update(cx, |store, cx| {
                            store.add_friend(
                                user_id,
                                contact.username,
                                contact.display_name,
                                contact.avatar,
                                cx,
                            );
                        });
                    }
                    close_member_menu(&panel, cx);
                }
            });
        }

        if permissions.blocked_by_me {
            menu = menu.item(t("contextMenu.member.unblock"), {
                let panel = panel.clone();
                move |_window: &mut Window, cx: &mut App| {
                    FriendStore::global(cx).update(cx, |store, cx| {
                        store.unblock_friend(user_id, cx);
                    });
                    close_member_menu(&panel, cx);
                }
            });
        }

        if permissions.is_friend {
            menu = menu
                .separator()
                .danger_item(t("contextMenu.member.removeFriend"), {
                    let panel = panel.clone();
                    let display_name = display_name.clone();
                    let locale = locale.clone();
                    move |window: &mut Window, cx: &mut App| {
                        let contact =
                            share_contact_subject(user_id, display_name.as_ref(), context, cx);
                        let username = if contact.username.is_empty() {
                            contact.display_name
                        } else {
                            contact.username
                        };
                        close_member_menu(&panel, cx);
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.confirm_remove_friend(
                                user_id,
                                &username,
                                FriendRemovalKind::RemoveFriend,
                                &locale,
                                window,
                                cx,
                            );
                        });
                    }
                });
        }
    }

    let can_ban =
        permissions.show_ban && permissions.clan_id.is_some() && permissions.channel_id.is_some();
    let can_kick = permissions.show_kick && permissions.clan_id.is_some();
    let can_remove_from_thread =
        permissions.show_remove_from_thread && permissions.channel_id.is_some();

    if is_clan && (can_ban || can_kick || can_remove_from_thread) {
        menu = menu.separator();
        if can_ban
            && let Some(clan_id) = permissions.clan_id
            && let Some(channel_id) = permissions.channel_id
        {
            if permissions.is_banned {
                menu = menu.danger_item(t("contextMenu.member.unBanChat"), {
                    let panel = panel.clone();
                    move |_window: &mut Window, cx: &mut App| {
                        BannedUsersStore::global(cx).update(cx, |store, cx| {
                            store.unban(clan_id, channel_id, user_id, cx);
                        });
                        close_member_menu(&panel, cx);
                    }
                });
            } else {
                menu = menu.danger_submenu(
                    t("contextMenu.member.banChat"),
                    None,
                    ban_duration_options(&locale),
                    ban_sub_open,
                    {
                        let panel = panel.clone();
                        move |_window: &mut Window, cx: &mut App| {
                            if let Some(p) = panel.upgrade() {
                                p.update(cx, |this, cx| {
                                    if let Some(menu) = this.open_menu.as_mut()
                                        && !menu.ban_sub_open
                                    {
                                        menu.ban_sub_open = true;
                                        cx.notify();
                                    }
                                });
                            }
                        }
                    },
                    {
                        let panel = panel.clone();
                        move |ban_time: i32, _window: &mut Window, cx: &mut App| {
                            BannedUsersStore::global(cx).update(cx, |store, cx| {
                                store.ban(clan_id, channel_id, user_id, ban_time, cx);
                            });
                            close_member_menu(&panel, cx);
                        }
                    },
                );
            }
        }
        if can_kick && let Some(clan_id) = permissions.clan_id {
            menu = menu.danger_item(t("contextMenu.member.kick"), {
                let panel = panel.clone();
                let display_name = display_name.clone();
                let locale = locale.clone();
                move |window: &mut Window, cx: &mut App| {
                    let contact =
                        share_contact_subject(user_id, display_name.as_ref(), context, cx);
                    let username = if contact.username.is_empty() {
                        contact.display_name
                    } else {
                        contact.username
                    };
                    close_member_menu(&panel, cx);
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.confirm_kick_member(clan_id, user_id, &username, &locale, window, cx);
                    });
                }
            });
        }
        if can_remove_from_thread && let Some(channel_id) = permissions.channel_id {
            menu = menu.danger_item(remove_from_thread_label, {
                let panel = panel.clone();
                let locale = locale.clone();
                move |_window: &mut Window, cx: &mut App| {
                    remove_member_from_thread(channel_id, user_id, &locale, cx);
                    close_member_menu(&panel, cx);
                }
            });
        }
    }

    menu
}

fn remove_member_from_thread(channel_id: ChannelId, user_id: UserId, locale: &str, cx: &mut App) {
    let success: SharedString = mezon_i18n::t(
        locale,
        "clanOverviewSetting.permissions.toast.removeMemberThreadSuccess",
    )
    .into();
    let failure: SharedString = mezon_i18n::t(
        locale,
        "clanOverviewSetting.permissions.toast.removeMemberThreadFailed",
    )
    .into();
    let task = ChannelMembersStore::global(cx)
        .update(cx, |store, cx| store.remove_member(channel_id, user_id, cx));
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            Shell::global(cx).update(cx, |shell, cx| match result {
                Ok(()) => shell.success(success, cx),
                Err(error) => {
                    tracing::error!(
                        "remove member {user_id} from thread {channel_id} failed: {error}"
                    );
                    shell.error(failure, cx);
                }
            });
        });
    })
    .detach();
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
