use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FontWeight, Hsla, ListSizingBehavior, SharedString, Subscription, Window,
    div, img, prelude::*, px, rgb, size, uniform_list,
};
use mezon_client::{EveryoneExcludeType, OverrideEveryoneChannel};
use mezon_store::{
    BadgeService, ChannelId, ChannelList, ChannelPermissionsStore, ChannelType, ChannelUserProfile,
    ChannelUsersEvent, ChannelUsersStore, ClanId, ClanMember, ClanMembersEvent, ClanMembersStore,
    RoleId, RolesStore, Settings, UserId,
};

use super::add_mem_role_modal::{AddMemRoleEvent, AddMemRoleModal};
use super::channel_acl::{self, member_matches, parse_search};
use super::permission_overrides::PermissionOverrides;
use crate::chat::role_style::role_fallback_color;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
    PaginationButton, Tooltip, h_flex, pagination_button, pagination_items, pagination_slot_count,
    v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const TOGGLE_TRACK_WIDTH: f32 = 32.0;
const TOGGLE_TRACK_HEIGHT: f32 = 16.0;
const TOGGLE_KNOB_SIZE: f32 = 16.0;
pub(super) const ROLE_ROW_HEIGHT: f32 = 36.0;
pub(super) const MEMBER_ROW_HEIGHT: f32 = 48.0;
const REMOVE_ICON_SIZE: f32 = 15.0;
const MEMBER_SEARCH_WIDTH: f32 = 220.0;
const MEMBER_SEARCH_HEIGHT: f32 = 30.0;
const MEMBER_PAGE_SIZE: usize = 8;

const TOGGLE_ON: u32 = 0x52_65_ec;
const TOGGLE_ON_HOVER: u32 = 0x46_54_c0;
const TOGGLE_OFF_TRACK: u32 = 0xcb_d5_e1;
const TOGGLE_OFF_TRACK_HOVER: u32 = 0x94_a3_b8;
const TOGGLE_OFF_KNOB: u32 = 0x64_74_8b;
const TOGGLE_OFF_KNOB_HOVER: u32 = 0x47_55_69;
const TOGGLE_KNOB_ON: u32 = 0xff_ff_ff;
const SYNC_ICON_COLOR: u32 = 0xf0_b0_33;
const REMOVE_HOVER_COLOR: u32 = 0xef_44_44;

pub(super) fn role_tint(color: &str) -> Hsla {
    match mezon_store::parse_role_color(color) {
        Some(rgba) => Hsla::from(rgba),
        None => role_fallback_color(),
    }
}

#[derive(Clone)]
pub(super) struct RoleRow {
    pub role_id: RoleId,
    pub title: SharedString,
    pub icon: SharedString,
    pub color: Hsla,
}

#[derive(Clone)]
pub(super) struct MemberRow {
    pub user_id: UserId,
    pub name: SharedString,
    pub username: SharedString,
    pub avatar: SharedString,
}

pub(super) fn member_row_from_clan_member(member: &ClanMember) -> MemberRow {
    MemberRow {
        user_id: member.id(),
        name: member.name().into(),
        username: member.user.username.clone().into(),
        avatar: member.avatar().into(),
    }
}

pub(super) fn member_row(clan_id: ClanId, user_id: UserId, cx: &App) -> MemberRow {
    match ClanMembersStore::global(cx)
        .read(cx)
        .member(clan_id, user_id)
    {
        Some(member) => member_row_from_clan_member(member),
        None => MemberRow {
            user_id,
            name: user_id.to_string().into(),
            username: SharedString::default(),
            avatar: SharedString::default(),
        },
    }
}

/// Same row, for a list that came from the channel's own user listing rather than the
/// clan roster. `ListClanUsers` is capped server-side and drops anyone who has left the
/// clan, so a channel user it cannot resolve would otherwise render as a nameless,
/// avatar-less row; the listing ships a username/display name/avatar of its own for
/// exactly that case.
pub(super) fn channel_member_row(
    clan_id: ClanId,
    channel_id: ChannelId,
    user_id: UserId,
    cx: &App,
) -> MemberRow {
    let row = member_row(clan_id, user_id, cx);
    if ClanMembersStore::global(cx)
        .read(cx)
        .member(clan_id, user_id)
        .is_some()
    {
        return row;
    }
    let store = ChannelUsersStore::global(cx);
    let Some(profile) = store.read(cx).profile(channel_id, user_id) else {
        return row;
    };
    member_row_from_profile(user_id, profile)
}

fn member_row_from_profile(user_id: UserId, profile: &ChannelUserProfile) -> MemberRow {
    let name = if !profile.display_name.is_empty() {
        profile.display_name.clone()
    } else if !profile.username.is_empty() {
        profile.username.clone()
    } else {
        user_id.to_string()
    };
    MemberRow {
        user_id,
        name: name.into(),
        username: profile.username.clone().into(),
        avatar: profile.avatar.clone().into(),
    }
}

pub(super) fn member_avatar(row: &MemberRow, size: gpui::Pixels) -> Avatar {
    let mut avatar = Avatar::new().size_px(size);
    if !row.avatar.is_empty() {
        avatar = avatar.src(row.avatar.clone());
    }
    avatar
}

pub(super) fn role_glyph(row: &RoleRow, cx: &mut App) -> gpui::AnyElement {
    if row.icon.is_empty() {
        Icon::new(IconName::RoleIcon)
            .size(px(20.0))
            .flex_shrink_0()
            .text_color(row.color)
            .into_any_element()
    } else {
        img(crate::util::imgproxy::role_icon_url(cx, &row.icon))
            .size(px(20.0))
            .aspect_square()
            .flex_shrink_0()
            .rounded(px(4.0))
            .image_cache(&crate::image_cache::shared_role_icon_cache(cx))
            .into_any_element()
    }
}

pub(super) fn page_count(len: usize, per_page: usize) -> usize {
    len.div_ceil(per_page.max(1)).max(1)
}

/// The rows `page` shows, clamped to the last page so a shrinking list (a removal, a
/// narrower search) can never leave the view pointing past the end.
pub(super) fn page_slice<T>(items: &[T], page: usize, per_page: usize) -> &[T] {
    let per_page = per_page.max(1);
    let page = page.min(page_count(items.len(), per_page).saturating_sub(1));
    let start = page * per_page;
    let end = (start + per_page).min(items.len());
    &items[start..end]
}

fn overrides_apply_to(private: bool, channel_type: ChannelType) -> bool {
    private && channel_type != ChannelType::Voice
}

pub(super) fn role_row_from(role_id: RoleId, role: &mezon_store::ClanRoleDetail) -> RoleRow {
    RoleRow {
        role_id,
        title: role.name.clone().into(),
        icon: role.icon.clone().into(),
        color: role_tint(&role.color),
    }
}

fn fallback_role_row(role_id: RoleId) -> RoleRow {
    RoleRow {
        role_id,
        title: role_id.to_string().into(),
        icon: SharedString::default(),
        color: role_fallback_color(),
    }
}

pub struct PermissionsTab {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    private_enabled: bool,
    private_initial: bool,
    seeded: bool,
    selected_user_ids: Vec<UserId>,
    staged_members: HashMap<UserId, ClanMember>,
    selected_role_ids: Vec<RoleId>,
    role_rows: Rc<Vec<RoleRow>>,
    member_ids: Rc<Vec<UserId>>,
    visible_member_ids: Rc<Vec<UserId>>,
    member_query: String,
    member_page: usize,
    member_search: Option<Entity<InputState>>,
    member_search_sub: Option<Subscription>,
    overrides: Option<Entity<PermissionOverrides>>,
    overrides_sub: Option<Subscription>,
    modal_sub: Option<Subscription>,
    everyone_enabled: bool,
    everyone_initial: bool,
    everyone_type: EveryoneExcludeType,
    everyone_type_initial: EveryoneExcludeType,
    everyone_user_ids: Vec<UserId>,
    everyone_user_ids_initial: Vec<UserId>,
    everyone_role_ids: Vec<RoleId>,
    everyone_role_ids_initial: Vec<RoleId>,
    everyone_staged_members: HashMap<UserId, ClanMember>,
    everyone_seeded: bool,
    everyone_load_generation: u64,
    everyone_saving: bool,
    everyone_modal_sub: Option<Subscription>,
    channel_fingerprint_seen: Option<(bool, UserId, ChannelType)>,
    _subs: Vec<Subscription>,
}

impl PermissionsTab {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let private = ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .is_some_and(|channel| channel.private);
        if private {
            ChannelUsersStore::global(cx).update(cx, |store, cx| {
                store.ensure_loaded(channel_id, cx);
            });
        }
        RolesStore::global(cx).update(cx, |store, cx| {
            if private {
                store.reload(clan_id, cx);
            } else {
                store.ensure_loaded(clan_id, cx);
            }
        });
        ClanMembersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });

        let subs = vec![
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&ChannelList::global(cx), |this, _, cx| {
                let fingerprint = this.channel_fingerprint(cx);
                if fingerprint != this.channel_fingerprint_seen {
                    this.channel_fingerprint_seen = fingerprint;
                    this.seed_from_store(cx);
                    this.rebuild_rows(cx);
                    this.sync_overrides(cx);
                    this.reload_access_lists(cx);
                    if this.everyone_dirty() {
                        this.everyone_load_generation =
                            this.everyone_load_generation.wrapping_add(1);
                    } else {
                        this.everyone_seeded = false;
                        this.load_everyone(cx);
                    }
                }
                cx.notify();
            }),
            cx.subscribe(
                &ChannelUsersStore::global(cx),
                |this, _, event: &ChannelUsersEvent, cx| match event {
                    ChannelUsersEvent::Changed { channel_id } if *channel_id == this.channel_id => {
                        this.refresh(cx);
                    }
                    ChannelUsersEvent::MembershipChanged { channel_id }
                        if *channel_id == this.channel_id =>
                    {
                        this.reload_access_lists(cx);
                    }
                    _ => {}
                },
            ),
            cx.observe(&RolesStore::global(cx), |this, _, cx| this.refresh(cx)),
            cx.subscribe(
                &ClanMembersStore::global(cx),
                |this, _, event: &ClanMembersEvent, cx| {
                    if event.clan_id() == this.clan_id {
                        this.apply_member_filter(cx);
                        cx.notify();
                    }
                },
            ),
        ];

        let mut this = Self {
            clan_id,
            channel_id,
            settings,
            private_enabled: false,
            private_initial: false,
            seeded: false,
            selected_user_ids: Vec::new(),
            staged_members: HashMap::new(),
            selected_role_ids: Vec::new(),
            role_rows: Rc::new(Vec::new()),
            member_ids: Rc::new(Vec::new()),
            visible_member_ids: Rc::new(Vec::new()),
            member_query: String::new(),
            member_page: 0,
            member_search: None,
            member_search_sub: None,
            overrides: None,
            overrides_sub: None,
            modal_sub: None,
            everyone_enabled: false,
            everyone_initial: false,
            everyone_type: EveryoneExcludeType::User,
            everyone_type_initial: EveryoneExcludeType::User,
            everyone_user_ids: Vec::new(),
            everyone_user_ids_initial: Vec::new(),
            everyone_role_ids: Vec::new(),
            everyone_role_ids_initial: Vec::new(),
            everyone_staged_members: HashMap::new(),
            everyone_seeded: false,
            everyone_load_generation: 0,
            everyone_saving: false,
            everyone_modal_sub: None,
            channel_fingerprint_seen: None,
            _subs: subs,
        };
        this.channel_fingerprint_seen = this.channel_fingerprint(cx);
        this.seed_from_store(cx);
        this.rebuild_rows(cx);
        this.sync_overrides(cx);
        this.load_everyone(cx);
        this
    }

    fn channel_fingerprint(&self, cx: &App) -> Option<(bool, UserId, ChannelType)> {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| (channel.private, channel.creator_id, channel.channel_type))
    }

    /// Voice channels get the private card and its member/role lists, not
    /// the override table: every override the server knows is a text
    /// permission (send message, manage threads, …) and would only invite
    /// toggles that do nothing in a voice room.
    fn shows_overrides(&self, cx: &App) -> Option<bool> {
        // Unknown yet means wait: the channel-list observer calls
        // `sync_overrides` again once the channel arrives.
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| overrides_apply_to(channel.private, channel.channel_type))
    }

    fn sync_overrides(&mut self, cx: &mut Context<Self>) {
        match self.shows_overrides(cx) {
            Some(true) if self.overrides.is_none() => {
                let clan_id = self.clan_id;
                let channel_id = self.channel_id;
                let settings = self.settings.clone();
                let overrides =
                    cx.new(|cx| PermissionOverrides::new(clan_id, channel_id, settings, cx));
                self.overrides_sub = Some(cx.observe(&overrides, |_, _, cx| cx.notify()));
                self.overrides = Some(overrides);
            }
            Some(false) => {
                self.overrides_sub = None;
                self.overrides = None;
            }
            _ => {}
        }
    }

    /// The settings shell paints this tab's save bar as a floating panel outside the
    /// scroll view, the same way the overview and integrations tabs do, so it stays put
    /// while the member list pages and scrolls.
    pub fn should_show_save_bar(&self, cx: &App) -> bool {
        self.private_enabled != self.private_initial
            || self.overrides_dirty(cx)
            || self.everyone_dirty()
    }

    fn everyone_dirty(&self) -> bool {
        channel_acl::everyone_is_dirty(
            self.everyone_enabled,
            self.everyone_initial,
            &self.everyone_user_ids,
            &self.everyone_user_ids_initial,
            &self.everyone_role_ids,
            &self.everyone_role_ids_initial,
        )
    }

    fn shows_everyone_override(&self, cx: &App) -> bool {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .is_some_and(|channel| {
                channel_acl::everyone_applies_to(
                    self.private_enabled || channel.private,
                    channel.is_thread(),
                    channel.channel_type,
                )
            })
    }

    fn load_everyone(&mut self, cx: &mut Context<Self>) {
        if self.everyone_seeded || self.everyone_dirty() || !self.shows_everyone_override(cx) {
            return;
        }
        let Some(api) = channel_acl::api(cx) else {
            return;
        };
        self.everyone_load_generation = self.everyone_load_generation.wrapping_add(1);
        let generation = self.everyone_load_generation;
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        cx.spawn(async move |this, cx| {
            match api
                .get_override_everyone_channel(clan_id.get(), channel_id.get())
                .await
            {
                Ok(data) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.everyone_load_generation != generation || this.everyone_dirty() {
                            return;
                        }
                        this.apply_everyone(data, cx);
                    });
                }
                Err(error) => {
                    tracing::error!(
                        "get_override_everyone_channel failed for {channel_id}: {error}"
                    );
                }
            }
        })
        .detach();
    }

    fn apply_everyone(&mut self, data: OverrideEveryoneChannel, cx: &mut Context<Self>) {
        if self.everyone_dirty() {
            return;
        }
        let kind = EveryoneExcludeType::from_lists(&data.user_ids, &data.role_ids);
        let user_ids: Vec<UserId> = data
            .user_ids
            .into_iter()
            .filter(|id| *id != 0)
            .map(UserId)
            .collect();
        let role_ids: Vec<RoleId> = data
            .role_ids
            .into_iter()
            .filter(|id| *id != 0)
            .map(RoleId)
            .collect();
        self.everyone_enabled = data.enabled;
        self.everyone_initial = data.enabled;
        self.everyone_type = kind;
        self.everyone_type_initial = kind;
        self.everyone_user_ids = user_ids.clone();
        self.everyone_user_ids_initial = user_ids;
        self.everyone_role_ids = role_ids.clone();
        self.everyone_role_ids_initial = role_ids;
        self.everyone_seeded = true;
        cx.notify();
    }

    fn overrides_dirty(&self, cx: &App) -> bool {
        self.overrides
            .as_ref()
            .is_some_and(|overrides| overrides.read(cx).has_pending())
    }

    fn seed_from_store(&mut self, cx: &mut Context<Self>) {
        if self.seeded {
            return;
        }
        let Some(private) = ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| channel.private)
        else {
            return;
        };
        self.seeded = true;
        self.private_initial = private;
        self.private_enabled = private;
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rebuild_rows(cx);
        self.sync_overrides(cx);
        cx.notify();
    }

    fn rebuild_rows(&mut self, cx: &App) {
        self.role_rows = Rc::new(self.compute_role_rows(cx));
        self.member_ids = Rc::new(self.compute_member_ids(cx));
        self.apply_member_filter(cx);
    }

    fn apply_member_filter(&mut self, cx: &App) {
        let needle = parse_search(&self.member_query).needle;
        // An empty query shares the full list's allocation instead of copying it.
        let visible = if needle.is_empty() {
            self.member_ids.clone()
        } else {
            Rc::new(
                self.member_ids
                    .iter()
                    .copied()
                    .filter(|user_id| self.member_matches_needle(*user_id, &needle, cx))
                    .collect::<Vec<_>>(),
            )
        };
        self.member_page = self
            .member_page
            .min(page_count(visible.len(), MEMBER_PAGE_SIZE).saturating_sub(1));
        self.visible_member_ids = visible;
    }

    /// Matches against the clan roster when it knows the user (nickname included) and
    /// against the channel listing's own identity otherwise, so a row that is only
    /// renderable from the listing stays searchable too.
    fn member_matches_needle(&self, user_id: UserId, needle: &str, cx: &App) -> bool {
        let members = ClanMembersStore::global(cx);
        if let Some(member) = members.read(cx).member(self.clan_id, user_id) {
            return member_matches(
                &member.clan_nick,
                &member.user.display_name,
                &member.user.username,
                needle,
            );
        }
        let channel_users = ChannelUsersStore::global(cx);
        let channel_users = channel_users.read(cx);
        if let Some(profile) = channel_users.profile(self.channel_id, user_id) {
            return member_matches("", &profile.display_name, &profile.username, needle);
        }
        self.staged_members.get(&user_id).is_some_and(|member| {
            member_matches(
                &member.clan_nick,
                &member.user.display_name,
                &member.user.username,
                needle,
            )
        })
    }

    fn listed_member_row(&self, user_id: UserId, cx: &App) -> MemberRow {
        let in_roster = ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, user_id)
            .is_some();
        match self.staged_members.get(&user_id) {
            Some(member) if !in_roster => member_row_from_clan_member(member),
            _ => channel_member_row(self.clan_id, self.channel_id, user_id, cx),
        }
    }

    fn ensure_member_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.member_search.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "channelSetting.channelPermission.searchMembers").into();
        // Embedded: the field paints no chrome of its own, so the row around it keeps
        // owning the background and it re-reads the theme on every frame instead of
        // freezing whichever one was live when the field was built.
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(MEMBER_SEARCH_HEIGHT))
                .text_size(px(14.0))
                .embedded(true)
        });
        self.member_search_sub =
            Some(cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if *event != InputEvent::Change {
                    return;
                }
                this.member_query = input.read(cx).value().to_string();
                this.member_page = 0;
                this.apply_member_filter(cx);
                cx.notify();
            }));
        self.member_search = Some(input);
    }

    fn reload_access_lists(&self, cx: &mut Context<Self>) {
        if !self.persisted_private(cx) {
            return;
        }
        let channel_id = self.channel_id;
        ChannelUsersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(channel_id, cx);
        });
        let clan_id = self.clan_id;
        RolesStore::global(cx).update(cx, |store, cx| store.reload(clan_id, cx));
    }

    fn persisted_private(&self, cx: &App) -> bool {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .is_some_and(|channel| channel.private)
    }

    fn channel_creator_id(&self, cx: &App) -> UserId {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| channel.creator_id)
            .unwrap_or(UserId(0))
    }

    fn category_name(&self, cx: &App) -> SharedString {
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| SharedString::from(channel.category_name.clone()))
            .unwrap_or_default()
    }

    fn current_user_id(cx: &App) -> UserId {
        BadgeService::try_global(cx)
            .and_then(|badge| badge.read(cx).current_user_id(cx))
            .unwrap_or(UserId(0))
    }

    fn active_channel_role_ids(&self, cx: &App) -> Vec<RoleId> {
        RolesStore::global(cx)
            .read(cx)
            .roles_for_channel(self.clan_id, self.channel_id)
            .into_iter()
            .filter(|(_, role)| role.role_channel_active)
            .map(|(role_id, _)| role_id)
            .collect()
    }

    fn compute_role_rows(&self, cx: &App) -> Vec<RoleRow> {
        let store = RolesStore::global(cx);
        let store = store.read(cx);
        if self.persisted_private(cx) {
            return store
                .roles_for_channel(self.clan_id, self.channel_id)
                .into_iter()
                .filter(|(_, role)| role.role_channel_active)
                .map(|(role_id, role)| role_row_from(role_id, role))
                .collect();
        }
        let on_channel = self.active_channel_role_ids(cx);
        store
            .active_roles_in_clan(self.clan_id)
            .into_iter()
            .filter(|(role_id, _)| {
                !on_channel.contains(role_id) && self.selected_role_ids.contains(role_id)
            })
            .map(|(role_id, role)| role_row_from(role_id, role))
            .collect()
    }

    fn compute_member_ids(&self, cx: &App) -> Vec<UserId> {
        let source: Vec<UserId> = if self.persisted_private(cx) {
            ChannelUsersStore::global(cx)
                .read(cx)
                .user_ids(self.channel_id)
                .to_vec()
        } else {
            self.selected_user_ids.clone()
        };
        let mut seen = HashSet::with_capacity(source.len());
        let mut unique = Vec::with_capacity(source.len());
        for user_id in source {
            if user_id.is_zero() || !seen.insert(user_id) {
                continue;
            }
            unique.push(user_id);
        }
        unique
    }

    fn toggle_private(&mut self, cx: &mut Context<Self>) {
        self.private_enabled = !self.private_enabled;
        if self.private_enabled {
            let channel_id = self.channel_id;
            ChannelUsersStore::global(cx).update(cx, |store, cx| {
                store.ensure_loaded(channel_id, cx);
            });
        }
        self.refresh(cx);
    }

    fn toggle_everyone(&mut self, cx: &mut Context<Self>) {
        self.everyone_enabled = !self.everyone_enabled;
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.selected_role_ids.clear();
        self.selected_user_ids.clear();
        self.staged_members.clear();
        self.private_enabled = self.private_initial;
        self.reset_everyone();
        if let Some(overrides) = self.overrides.clone() {
            overrides.update(cx, |overrides, cx| overrides.reset(cx));
        }
        self.refresh(cx);
    }

    fn reset_everyone(&mut self) {
        self.everyone_enabled = self.everyone_initial;
        self.everyone_type = self.everyone_type_initial;
        self.everyone_user_ids
            .clone_from(&self.everyone_user_ids_initial);
        self.everyone_role_ids
            .clone_from(&self.everyone_role_ids_initial);
        self.everyone_modal_sub = None;
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.everyone_saving {
            return;
        }
        let private_changed = self.private_enabled != self.private_initial;
        let going_private = private_changed && self.private_enabled;
        let everyone_dirty = self.everyone_dirty() && !going_private;
        let overrides = self.overrides.clone();
        if !private_changed && !everyone_dirty {
            if let Some(overrides) = overrides {
                overrides.update(cx, |overrides, cx| overrides.save(cx));
            }
            return;
        }
        let Some(api) = channel_acl::api(cx) else {
            return;
        };
        let locale = self.settings.read(cx).language.clone();
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let private_payload = private_changed.then(|| {
            let creator_id = match self.channel_creator_id(cx) {
                id if id.is_zero() => Self::current_user_id(cx),
                id => id,
            };
            let private_enabled = self.private_enabled;
            let user_ids = channel_acl::acl_user_ids(
                private_enabled,
                &self.selected_user_ids,
                Some(creator_id),
            );
            let role_ids = channel_acl::acl_role_ids(private_enabled, &self.selected_role_ids);
            let granted_roles = if private_enabled {
                self.selected_role_ids.clone()
            } else {
                Vec::new()
            };
            (
                private_enabled,
                channel_acl::channel_private_payload(private_enabled),
                user_ids,
                role_ids,
                granted_roles,
            )
        });
        let everyone_payload = everyone_dirty.then(|| {
            let enabled = self.everyone_enabled;
            let kind = self.everyone_type;
            let user_snapshot = self.everyone_user_ids.clone();
            let role_snapshot = self.everyone_role_ids.clone();
            let calls = channel_acl::everyone_set_calls(
                enabled,
                kind,
                &self.everyone_user_ids,
                &self.everyone_user_ids_initial,
                &self.everyone_role_ids,
                &self.everyone_role_ids_initial,
            );
            (enabled, kind, user_snapshot, role_snapshot, calls)
        });
        self.everyone_saving = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            if going_private
                && let Err(error) = api
                    .set_override_everyone_channel(
                        clan_id.get(),
                        channel_id.get(),
                        false,
                        EveryoneExcludeType::User,
                        Vec::new(),
                    )
                    .await
            {
                tracing::error!("set_override_everyone_channel failed for {channel_id}: {error}");
                let _ = this.update(cx, |this, cx| {
                    this.everyone_saving = false;
                    super::permission_overrides::report_acl_failure(&locale, cx);
                    cx.notify();
                });
                return;
            }
            if let Some((private_enabled, payload, user_ids, role_ids, granted_roles)) =
                private_payload
            {
                if let Err(error) = api
                    .update_channel_private(
                        clan_id.get(),
                        channel_id.get(),
                        payload,
                        user_ids,
                        role_ids,
                    )
                    .await
                {
                    tracing::error!("update_channel_private failed for {channel_id}: {error}");
                    let _ = this.update(cx, |this, cx| {
                        this.everyone_saving = false;
                        super::permission_overrides::report_acl_failure(&locale, cx);
                        cx.notify();
                    });
                    return;
                }
                let _ = this.update(cx, |this, cx| {
                    this.apply_private_saved(private_enabled, granted_roles, cx);
                });
            }
            if let Some((enabled, kind, user_snapshot, role_snapshot, calls)) = everyone_payload {
                let mut failed = false;
                let mut saved_users = false;
                let mut saved_roles = false;
                for (call_kind, exclude_ids) in calls {
                    if let Err(error) = api
                        .set_override_everyone_channel(
                            clan_id.get(),
                            channel_id.get(),
                            enabled,
                            call_kind,
                            exclude_ids,
                        )
                        .await
                    {
                        tracing::error!(
                            "set_override_everyone_channel failed for {channel_id}: {error}"
                        );
                        failed = true;
                        break;
                    }
                    match call_kind {
                        EveryoneExcludeType::User => saved_users = true,
                        EveryoneExcludeType::Role => saved_roles = true,
                    }
                }
                let _ = this.update(cx, |this, cx| {
                    if !enabled && !failed {
                        this.everyone_initial = false;
                        this.everyone_user_ids.clear();
                        this.everyone_role_ids.clear();
                        this.everyone_user_ids_initial.clear();
                        this.everyone_role_ids_initial.clear();
                        this.everyone_type = EveryoneExcludeType::User;
                        this.everyone_type_initial = EveryoneExcludeType::User;
                    } else if enabled {
                        if saved_users || saved_roles {
                            this.everyone_initial = true;
                        }
                        if saved_users {
                            this.everyone_user_ids_initial.clone_from(&user_snapshot);
                        }
                        if saved_roles {
                            this.everyone_role_ids_initial.clone_from(&role_snapshot);
                        }
                        if !failed {
                            this.everyone_type_initial = kind;
                        }
                    }
                    if !failed && let Some(store) = ChannelPermissionsStore::try_global(cx) {
                        store.update(cx, |store, cx| {
                            store.reload_channel(clan_id, channel_id, cx);
                        });
                    }
                    if failed {
                        super::permission_overrides::report_acl_failure(&locale, cx);
                    }
                });
                if failed {
                    let _ = this.update(cx, |this, cx| {
                        this.everyone_saving = false;
                        cx.notify();
                    });
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.everyone_saving = false;
                if let Some(overrides) = overrides {
                    overrides.update(cx, |overrides, cx| overrides.save(cx));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_private_saved(
        &mut self,
        private_enabled: bool,
        granted_roles: Vec<RoleId>,
        cx: &mut Context<Self>,
    ) {
        self.private_initial = private_enabled;
        self.selected_user_ids.clear();
        self.staged_members.clear();
        self.selected_role_ids.clear();
        if private_enabled {
            self.everyone_enabled = false;
            self.everyone_initial = false;
            self.everyone_user_ids.clear();
            self.everyone_user_ids_initial.clear();
            self.everyone_role_ids.clear();
            self.everyone_role_ids_initial.clear();
            self.everyone_type = EveryoneExcludeType::User;
            self.everyone_type_initial = EveryoneExcludeType::User;
            self.everyone_seeded = true;
            self.everyone_load_generation = self.everyone_load_generation.wrapping_add(1);
        }
        ChannelUsersStore::global(cx).update(cx, |store, cx| {
            store.apply_privacy_change(self.channel_id, private_enabled, cx);
        });
        RolesStore::global(cx).update(cx, |store, cx| {
            store.set_channel_roles(self.clan_id, self.channel_id, &granted_roles, cx);
        });
        self.refresh(cx);
    }

    fn remove_everyone_role(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        self.everyone_role_ids.retain(|id| *id != role_id);
        cx.notify();
    }

    fn remove_everyone_member(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        self.everyone_user_ids.retain(|id| *id != user_id);
        self.everyone_staged_members.remove(&user_id);
        cx.notify();
    }

    fn remove_role(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        self.selected_role_ids.retain(|id| *id != role_id);
        let persisted_private = self.persisted_private(cx);
        let on_channel = persisted_private && self.active_channel_role_ids(cx).contains(&role_id);
        self.refresh(cx);
        if !on_channel {
            return;
        }
        let Some(api) = channel_acl::api(cx) else {
            return;
        };
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        RolesStore::global(cx).update(cx, |store, cx| {
            store.remove_role_from_channel(clan_id, role_id, channel_id, cx);
        });
        let locale = self.settings.read(cx).language.clone();
        cx.spawn(async move |_, cx| {
            if let Err(error) = api
                .delete_role_channel_desc(role_id.get(), channel_id.get(), clan_id.get())
                .await
            {
                tracing::error!("delete_role_channel_desc failed for {role_id}: {error}");
                cx.update(|cx| {
                    RolesStore::global(cx).update(cx, |store, cx| {
                        store.add_roles_to_channel(clan_id, channel_id, &[role_id], cx);
                    });
                    super::permission_overrides::report_acl_failure(&locale, cx);
                });
            }
        })
        .detach();
    }

    fn remove_member(&mut self, user_id: UserId, cx: &mut Context<Self>) {
        if !self.persisted_private(cx) {
            self.selected_user_ids.retain(|id| *id != user_id);
            self.refresh(cx);
            return;
        }
        let present = ChannelUsersStore::global(cx)
            .read(cx)
            .user_ids(self.channel_id)
            .contains(&user_id);
        if !present {
            return;
        }
        let Some(api) = channel_acl::api(cx) else {
            return;
        };
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let is_self = Self::current_user_id(cx) == user_id;
        let locale = self.settings.read(cx).language.clone();
        cx.spawn(async move |this, cx| {
            if let Err(error) = api
                .remove_channel_users(channel_id.get(), vec![user_id.get().to_string()])
                .await
            {
                tracing::error!("remove_channel_users failed for {channel_id}: {error}");
                cx.update(|cx| {
                    super::permission_overrides::report_acl_failure(&locale, cx);
                });
                return;
            }
            let _ = this.update(cx, |this, cx| {
                ChannelUsersStore::global(cx).update(cx, |store, cx| {
                    store.remove_users(channel_id, &[user_id], cx);
                });
                this.selected_user_ids.retain(|id| *id != user_id);
                this.refresh(cx);
                if is_self && !clan_id.is_zero() {
                    crate::router::navigate(cx, crate::router::Route::ClanMembers { clan_id });
                }
            });
        })
        .detach();
    }

    fn open_add_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let modal = AddMemRoleModal::open(
            self.clan_id,
            self.channel_id,
            self.settings.clone(),
            self.selected_user_ids.clone(),
            self.selected_role_ids.clone(),
            window,
            cx,
        );
        self.modal_sub = Some(
            cx.subscribe(&modal, |this, _, event: &AddMemRoleEvent, cx| {
                let AddMemRoleEvent::Staged {
                    user_ids,
                    role_ids,
                    remote_members,
                } = event
                else {
                    return;
                };
                for user_id in user_ids {
                    if !this.selected_user_ids.contains(user_id) {
                        this.selected_user_ids.push(*user_id);
                    }
                }
                for member in remote_members {
                    this.staged_members.insert(member.id(), member.clone());
                }
                for role_id in role_ids {
                    if !this.selected_role_ids.contains(role_id) {
                        this.selected_role_ids.push(*role_id);
                    }
                }
                this.refresh(cx);
            }),
        );
    }

    fn open_everyone_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let modal = AddMemRoleModal::open_everyone_exclude(
            self.clan_id,
            self.channel_id,
            self.settings.clone(),
            self.everyone_type,
            self.everyone_user_ids.clone(),
            self.everyone_role_ids.clone(),
            window,
            cx,
        );
        self.everyone_modal_sub = Some(cx.subscribe(
            &modal,
            |this, _, event: &AddMemRoleEvent, cx| {
                let AddMemRoleEvent::EveryoneExclude {
                    kind,
                    user_ids,
                    role_ids,
                    remote_members,
                } = event
                else {
                    return;
                };
                this.everyone_type = *kind;
                match kind {
                    EveryoneExcludeType::User => {
                        this.everyone_user_ids = user_ids.clone();
                    }
                    EveryoneExcludeType::Role => {
                        this.everyone_role_ids = role_ids.clone();
                    }
                }
                for member in remote_members {
                    this.everyone_staged_members
                        .insert(member.id(), member.clone());
                }
                cx.notify();
            },
        ));
    }

    fn render_header(&self, locale: &str, theme: &Theme, cx: &App) -> impl IntoElement {
        v_flex()
            .child(
                div()
                    .mb_4()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(mezon_i18n::t(
                        locale,
                        "channelSetting.channelPermission.header.title",
                    )),
            )
            .child(
                div()
                    .mb_3()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(
                        locale,
                        "channelSetting.channelPermission.header.description",
                    )),
            )
            .child(
                h_flex()
                    .mt_4()
                    .p_4()
                    .items_start()
                    .child(
                        Icon::new(IconName::SyncIcon)
                            .size(px(20.0))
                            .flex_shrink_0()
                            .mr_2()
                            .text_color(rgb(SYNC_ICON_COLOR)),
                    )
                    .child(
                        div()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(
                                locale,
                                "channelSetting.channelPermission.header.syncedWithCategory",
                            )),
                    )
                    .child(
                        div()
                            .pl_1()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(self.category_name(cx)),
                    ),
            )
    }

    fn render_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_switch("channel-private-toggle", self.private_enabled)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_private(cx)))
    }

    fn render_everyone_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_switch("channel-everyone-toggle", self.everyone_enabled)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_everyone(cx)))
    }

    fn render_switch(&self, id: &'static str, enabled: bool) -> gpui::Stateful<gpui::Div> {
        let track_color = if enabled { TOGGLE_ON } else { TOGGLE_OFF_TRACK };
        let track_hover = if enabled {
            TOGGLE_ON_HOVER
        } else {
            TOGGLE_OFF_TRACK_HOVER
        };
        let knob_color = if enabled {
            TOGGLE_KNOB_ON
        } else {
            TOGGLE_OFF_KNOB
        };
        let knob_hover = if enabled {
            TOGGLE_KNOB_ON
        } else {
            TOGGLE_OFF_KNOB_HOVER
        };

        div()
            .id(id)
            .group(id)
            .flex_shrink_0()
            .relative()
            .w(px(TOGGLE_TRACK_WIDTH))
            .h(px(TOGGLE_TRACK_HEIGHT))
            .rounded(px(8.0))
            .cursor_pointer()
            .bg(rgb(track_color))
            .hover(|style| style.bg(rgb(track_hover)))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(if enabled { TOGGLE_KNOB_SIZE } else { 0.0 }))
                    .size(px(TOGGLE_KNOB_SIZE))
                    .rounded_full()
                    .bg(rgb(knob_color))
                    .group_hover(id, move |style| style.bg(rgb(knob_hover))),
            )
    }

    fn render_private_card(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.private_enabled;
        h_flex()
            .items_start()
            .justify_between()
            .p_4()
            .bg(theme.tokens.theme_setting_nav)
            .border_color(theme.tokens.border_primary)
            .when(expanded, |el| {
                el.border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .rounded_tl(px(8.0))
                    .rounded_tr(px(8.0))
            })
            .when(!expanded, |el| el.border_1().rounded(px(8.0)))
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .mb_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::LockIcon)
                                    .size(px(20.0))
                                    .flex_shrink_0()
                                    .text_color(theme.tokens.text_secondary),
                            )
                            .child(
                                div()
                                    .ml_2()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(mezon_i18n::t(
                                        locale,
                                        "channelSetting.channelPermission.privateChannel",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(
                                locale,
                                "channelSetting.channelPermission.basicViewDescription",
                            )),
                    ),
            )
            .child(self.render_toggle(cx))
    }

    fn render_section_label(label: &'static str, theme: &Theme) -> impl IntoElement {
        div()
            .pb_4()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(theme.tokens.text_theme_primary)
            .child(label.to_uppercase())
    }

    fn render_exclude_section_label(label: &'static str, theme: &Theme) -> impl IntoElement {
        div()
            .pb_4()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(theme.tokens.text_theme_primary)
            .child(label)
    }

    fn render_divider(theme: &Theme) -> impl IntoElement {
        div().h(px(1.0)).w_full().bg(theme.tokens.border_primary)
    }

    fn render_access_panel(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .p_4()
            .bg(theme.tokens.theme_setting_nav)
            .border_l_1()
            .border_r_1()
            .border_b_1()
            .border_color(theme.tokens.border_primary)
            .rounded_bl(px(8.0))
            .rounded_br(px(8.0))
            .child(
                h_flex()
                    .pb_4()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(
                                mezon_i18n::t(
                                    locale,
                                    "channelSetting.channelPermission.whoCanAccess",
                                )
                                .to_uppercase(),
                            ),
                    )
                    .child(
                        div()
                            .id("channel-permission-add")
                            .px_4()
                            .py_1()
                            .rounded_lg()
                            .cursor_pointer()
                            .bg(theme.tokens.button_theme_primary)
                            .text_color(gpui::white())
                            .hover(|style| style.bg(theme.tokens.bg_button_primary_hover))
                            .child(mezon_i18n::t(
                                locale,
                                "channelSetting.channelPermission.addMemberAndRoles",
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_add_modal(window, cx);
                            })),
                    ),
            )
            .child(Self::render_divider(theme))
            .child(
                v_flex()
                    .py_4()
                    .child(Self::render_section_label(
                        mezon_i18n::t(locale, "channelSetting.channelPermission.roles"),
                        theme,
                    ))
                    .child(self.render_role_list(locale, theme, cx)),
            )
            .child(Self::render_divider(theme))
            .child(self.render_members_section(locale, theme, window, cx))
    }

    fn render_members_section(
        &mut self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let total = self.member_ids.len();
        self.ensure_member_search(window, cx);
        let matched = self.visible_member_ids.len();
        let count: SharedString = if matched == total {
            total.to_string().into()
        } else {
            format!("{matched}/{total}").into()
        };

        v_flex()
            .py_4()
            .child(
                h_flex()
                    .pb_4()
                    .w_full()
                    .gap_x_3()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_x_2()
                            .items_center()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(
                                mezon_i18n::t(locale, "channelSetting.channelPermission.members")
                                    .to_uppercase(),
                            )
                            .when(total > 0, |el| {
                                el.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_2()
                                        .rounded_full()
                                        .bg(theme.tokens.bg_tertiary)
                                        .child(count),
                                )
                            }),
                    )
                    .when_some(self.member_search.clone(), |el, input| {
                        el.child(
                            h_flex()
                                .gap_2()
                                .child(Icon::new(IconName::Search).size(px(16.0)))
                                .child(div().flex_1().min_w_0().child(Input::new(&input)))
                                .flex_shrink_0()
                                .w(px(MEMBER_SEARCH_WIDTH))
                                .px_2()
                                .rounded_lg()
                                .bg(theme.tokens.bg_input_secondary),
                        )
                    }),
            )
            .child(self.render_member_list(locale, theme, cx))
            .child(self.render_member_pagination(cx))
    }

    fn render_member_pagination(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pages = page_count(self.visible_member_ids.len(), MEMBER_PAGE_SIZE);
        if pages <= 1 {
            return div().into_any_element();
        }
        let current = self.member_page.min(pages - 1);
        let theme = cx.theme().clone();
        let mut bar = h_flex()
            .w_full()
            .pt_3()
            .gap_2()
            .items_center()
            .justify_center();
        bar = bar.child(
            pagination_button(
                "channel-permission-members",
                PaginationButton::Previous,
                current == 0,
                false,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.go_to_member_page(|page| page.saturating_sub(1), cx)
            })),
        );
        let mut numbers = h_flex()
            // Every state fills the same number of fixed-width slots, so the strip keeps
            // one width and prev/next or a page number never slides under the pointer.
            .w(px((pagination_slot_count(pages) * 48 - 8) as f32))
            .flex_shrink_0()
            .gap_2()
            .items_center()
            .justify_center();
        for page in pagination_items(current, pages) {
            let Some(page) = page else {
                numbers = numbers.child(div().w(px(40.0)).text_center().child("…"));
                continue;
            };
            numbers = numbers.child(
                pagination_button(
                    "channel-permission-members",
                    PaginationButton::Page(page + 1),
                    false,
                    page == current,
                    &theme,
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.go_to_member_page(move |_| page, cx)),
                ),
            );
        }
        bar.child(numbers)
            .child(
                pagination_button(
                    "channel-permission-members",
                    PaginationButton::Next,
                    current + 1 >= pages,
                    false,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.go_to_member_page(|page| page.saturating_add(1), cx)
                })),
            )
            .into_any_element()
    }

    fn go_to_member_page(&mut self, pick: impl FnOnce(usize) -> usize, cx: &mut Context<Self>) {
        let pages = page_count(self.visible_member_ids.len(), MEMBER_PAGE_SIZE);
        let next = pick(self.member_page).min(pages.saturating_sub(1));
        if next == self.member_page {
            return;
        }
        self.member_page = next;
        cx.notify();
    }

    fn render_role_list(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let rows = self.role_rows.clone();
        if rows.is_empty() {
            return h_flex()
                .py_2()
                .gap_x_2()
                .items_center()
                .text_color(theme.tokens.text_theme_primary)
                .child(
                    Icon::new(IconName::RoleIcon)
                        .size(px(20.0))
                        .flex_shrink_0()
                        .text_color(theme.tokens.text_theme_primary),
                )
                .child(
                    div()
                        .text_sm()
                        .child(mezon_i18n::t(locale, "common.noRoles")),
                )
                .into_any_element();
        }

        let role_label = mezon_i18n::t(locale, "common.role");
        let tab = cx.entity();
        let count = rows.len();
        uniform_list(
            "channel-permission-roles",
            count,
            move |range, _window, cx| {
                let theme = cx.theme().clone();
                range
                    .map(|ix| match rows.get(ix) {
                        Some(row) => {
                            let tab = tab.clone();
                            let role_id = row.role_id;
                            render_acl_role_row(
                                row,
                                Some(role_label),
                                &theme,
                                Rc::new(move |cx| {
                                    tab.update(cx, |this, cx| this.remove_role(role_id, cx));
                                }),
                                "channel-permission-role-remove",
                                cx,
                            )
                            .into_any_element()
                        }
                        None => div().h(px(ROLE_ROW_HEIGHT)).into_any_element(),
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_item_size(size(px(0.0), px(ROLE_ROW_HEIGHT)))
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .w_full()
        .into_any_element()
    }

    fn render_member_list(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.member_ids.is_empty() {
            return div().into_any_element();
        }
        if self.visible_member_ids.is_empty() {
            return div()
                .h(px(MEMBER_ROW_HEIGHT * MEMBER_PAGE_SIZE as f32))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(
                    locale,
                    "channelSetting.channelPermission.noMembersFound",
                ))
                .into_any_element();
        }
        let creator_id = self.channel_creator_id(cx);
        let creator_label =
            mezon_i18n::t(locale, "channelSetting.channelPermission.ChannelCreator");
        let tab = cx.entity();
        v_flex()
            .w_full()
            .min_h(px(MEMBER_ROW_HEIGHT * MEMBER_PAGE_SIZE as f32))
            .children(
                page_slice(&self.visible_member_ids, self.member_page, MEMBER_PAGE_SIZE)
                    .iter()
                    .map(|user_id| {
                        let row = self.listed_member_row(*user_id, cx);
                        let tab = tab.clone();
                        let user_id = *user_id;
                        let on_remove = (creator_id != user_id).then(|| {
                            Rc::new(move |cx: &mut App| {
                                tab.update(cx, |this, cx| this.remove_member(user_id, cx));
                            }) as Rc<dyn Fn(&mut App)>
                        });
                        render_acl_member_row(
                            &row,
                            creator_id == user_id,
                            creator_label,
                            theme,
                            locale,
                            on_remove,
                            "channel-permission-member-remove",
                        )
                    }),
            )
            .into_any_element()
    }

    /// Floating save bar for the permissions tab, rendered by the settings shell outside the
    /// scroll view so paging or scrolling the member list never moves it out of reach.
    pub fn render_channel_permissions_save_bar(
        tab: Entity<PermissionsTab>,
        locale: &str,
        theme: &Theme,
        cx: &App,
    ) -> impl IntoElement {
        let tab_read = tab.read(cx);
        let saving = tab_read.everyone_saving
            || tab_read
                .overrides
                .as_ref()
                .is_some_and(|overrides| overrides.read(cx).is_saving());
        div()
            .absolute()
            .bottom(px(20.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .occlude()
            .child(
                div()
                    .w(px(700.0))
                    .max_w(gpui::relative(0.9))
                    .py(px(10.0))
                    .pl_4()
                    .pr(px(10.0))
                    .rounded(px(5.0))
                    .bg(theme.tokens.theme_setting_nav)
                    .border_1()
                    .border_color(theme.tokens.border_primary)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(mezon_i18n::t(
                                        locale,
                                        "clanSettings.modalSaveChanges.title",
                                    )),
                            )
                            .child(
                                h_flex()
                                    .gap(px(20.0))
                                    .items_center()
                                    .child(
                                        Button::new("channel-permission-reset")
                                            .disabled(saving)
                                            .label(mezon_i18n::t(
                                                locale,
                                                "clanSettings.modalSaveChanges.reset",
                                            ))
                                            .ghost()
                                            .on_click({
                                                let tab = tab.clone();
                                                move |_, _, cx| {
                                                    tab.update(cx, |this, cx| this.reset(cx));
                                                }
                                            }),
                                    )
                                    .child(
                                        Button::new("channel-permission-save")
                                            .disabled(saving)
                                            .label(mezon_i18n::t(
                                                locale,
                                                "clanSettings.modalSaveChanges.saveChanges",
                                            ))
                                            .primary()
                                            .on_click({
                                                let tab = tab.clone();
                                                move |_, _, cx| {
                                                    tab.update(cx, |this, cx| this.save(cx));
                                                }
                                            }),
                                    ),
                            ),
                    ),
            )
    }

    fn render_everyone_block(
        &self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .mt_4()
            .rounded(px(6.0))
            .overflow_hidden()
            .child(self.render_everyone_card(locale, theme, cx))
            .when(self.everyone_enabled, |el| {
                el.child(self.render_everyone_panel(locale, theme, window, cx))
            })
    }

    fn render_everyone_card(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.everyone_enabled;
        h_flex()
            .items_start()
            .justify_between()
            .p_4()
            .bg(theme.tokens.theme_setting_nav)
            .border_color(theme.tokens.border_primary)
            .when(expanded, |el| {
                el.border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .rounded_tl(px(8.0))
                    .rounded_tr(px(8.0))
            })
            .when(!expanded, |el| el.border_1().rounded(px(8.0)))
            .child(
                div()
                    .min_w_0()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(
                        locale,
                        "channelSetting.channelPermission.preventEveryoneSend",
                    )),
            )
            .child(self.render_everyone_toggle(cx))
    }

    fn render_everyone_panel(
        &self,
        locale: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .p_4()
            .bg(theme.tokens.theme_setting_nav)
            .border_l_1()
            .border_r_1()
            .border_b_1()
            .border_color(theme.tokens.border_primary)
            .rounded_bl(px(8.0))
            .rounded_br(px(8.0))
            .child(
                h_flex().pb_4().items_center().justify_end().child(
                    div()
                        .id("channel-everyone-add")
                        .px_4()
                        .py_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .bg(theme.tokens.button_theme_primary)
                        .text_color(gpui::white())
                        .hover(|style| style.bg(theme.tokens.bg_button_primary_hover))
                        .child(mezon_i18n::t(
                            locale,
                            "channelSetting.channelPermission.addExcludeMembersRoles",
                        ))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_everyone_modal(window, cx);
                        })),
                ),
            )
            .child(self.render_everyone_exclude_list(locale, theme, window, cx))
    }

    fn render_everyone_exclude_list(
        &self,
        locale: &str,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_roles = !self.everyone_role_ids.is_empty();
        let has_members = !self.everyone_user_ids.is_empty();
        if !has_roles && !has_members {
            return div().into_any_element();
        }
        let mut children = vec![
            Self::render_exclude_section_label(
                mezon_i18n::t(locale, "channelSetting.channelPermission.allowExcludeSend"),
                theme,
            )
            .into_any_element(),
        ];
        if has_roles {
            children.push(self.render_everyone_role_list(cx).into_any_element());
        }
        if has_members {
            children.push(
                self.render_everyone_member_list(locale, cx)
                    .into_any_element(),
            );
        }
        v_flex().children(children).into_any_element()
    }

    fn everyone_role_rows(&self, cx: &App) -> Vec<RoleRow> {
        let store = RolesStore::global(cx);
        let store = store.read(cx);
        self.everyone_role_ids
            .iter()
            .map(|role_id| {
                store
                    .clan_role(self.clan_id, *role_id)
                    .map(|role| role_row_from(*role_id, role))
                    .unwrap_or_else(|| fallback_role_row(*role_id))
            })
            .collect()
    }

    fn everyone_member_row(&self, user_id: UserId, cx: &App) -> MemberRow {
        match self.everyone_staged_members.get(&user_id) {
            Some(member) => member_row_from_clan_member(member),
            None => channel_member_row(self.clan_id, self.channel_id, user_id, cx),
        }
    }

    fn render_everyone_role_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = cx.entity();
        let rows = Rc::new(self.everyone_role_rows(cx));
        let count = rows.len();
        let height = px((count.min(MEMBER_PAGE_SIZE) as f32) * ROLE_ROW_HEIGHT);
        uniform_list(
            "channel-everyone-roles",
            count,
            move |range, _window, cx| {
                let theme = cx.theme().clone();
                range
                    .map(|ix| match rows.get(ix) {
                        Some(row) => {
                            let tab = tab.clone();
                            let role_id = row.role_id;
                            render_acl_role_row(
                                row,
                                None,
                                &theme,
                                Rc::new(move |cx| {
                                    tab.update(cx, |this, cx| {
                                        this.remove_everyone_role(role_id, cx)
                                    });
                                }),
                                "channel-everyone-role-remove",
                                cx,
                            )
                            .into_any_element()
                        }
                        None => div().h(px(ROLE_ROW_HEIGHT)).into_any_element(),
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_item_size(size(px(0.0), px(ROLE_ROW_HEIGHT)))
        .h(height)
        .w_full()
    }

    fn render_everyone_member_list(
        &self,
        locale: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tab = cx.entity();
        let locale = locale.to_string();
        let rows = Rc::new(
            self.everyone_user_ids
                .iter()
                .copied()
                .map(|user_id| self.everyone_member_row(user_id, cx))
                .collect::<Vec<_>>(),
        );
        let count = rows.len();
        let height = px((count.min(MEMBER_PAGE_SIZE) as f32) * MEMBER_ROW_HEIGHT);
        uniform_list(
            "channel-everyone-members",
            count,
            move |range, _window, cx| {
                let theme = cx.theme().clone();
                range
                    .map(|ix| match rows.get(ix) {
                        Some(row) => {
                            let tab = tab.clone();
                            let user_id = row.user_id;
                            render_acl_member_row(
                                row,
                                false,
                                "",
                                &theme,
                                &locale,
                                Some(Rc::new(move |cx: &mut App| {
                                    tab.update(cx, |this, cx| {
                                        this.remove_everyone_member(user_id, cx)
                                    });
                                })),
                                "channel-everyone-member-remove",
                            )
                            .into_any_element()
                        }
                        None => div().h(px(MEMBER_ROW_HEIGHT)).into_any_element(),
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_item_size(size(px(0.0), px(MEMBER_ROW_HEIGHT)))
        .h(height)
        .w_full()
    }
}

fn render_acl_role_row(
    row: &RoleRow,
    role_label: Option<&'static str>,
    theme: &Theme,
    on_remove: Rc<dyn Fn(&mut App)>,
    remove_id: &'static str,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let role_id = row.role_id;
    let group_name = SharedString::from(format!("{remove_id}-{}", role_id.get()));
    h_flex()
        .group(group_name.clone())
        .h(px(ROLE_ROW_HEIGHT))
        .w_full()
        .py_2()
        .items_center()
        .justify_between()
        .rounded(px(4.0))
        .text_color(theme.tokens.text_theme_primary)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_x_2()
                .items_center()
                .child(role_glyph(row, cx))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(row.title.clone()),
                ),
        )
        .child(
            h_flex()
                .flex_shrink_0()
                .gap_x_2()
                .items_center()
                .when_some(role_label, |el, label| {
                    el.child(div().text_xs().child(label))
                })
                .child(
                    div()
                        .id((remove_id, role_id.get() as u64))
                        .size(px(32.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .on_click({
                            let on_remove = on_remove.clone();
                            move |_, _, cx| on_remove(cx)
                        })
                        .child(
                            Icon::new(IconName::EscIcon)
                                .size(px(REMOVE_ICON_SIZE))
                                .text_color(theme.tokens.text_theme_primary)
                                .group_hover(group_name, |style| {
                                    style.text_color(rgb(REMOVE_HOVER_COLOR))
                                }),
                        ),
                ),
        )
}

fn render_acl_member_row(
    row: &MemberRow,
    is_creator: bool,
    creator_label: &'static str,
    theme: &Theme,
    locale: &str,
    on_remove: Option<Rc<dyn Fn(&mut App)>>,
    remove_id: &'static str,
) -> impl IntoElement + use<> {
    let user_id = row.user_id;
    let group_name = SharedString::from(format!("{remove_id}-{}", user_id.get()));
    let remove_label = mezon_i18n::t(locale, "channelSetting.channelPermission.removeMember");
    h_flex()
        .group(group_name.clone())
        .h(px(MEMBER_ROW_HEIGHT))
        .w_full()
        .py_2()
        .items_center()
        .justify_between()
        .rounded(px(4.0))
        .text_color(theme.tokens.text_theme_primary)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_x_2()
                .items_center()
                .child(member_avatar(row, px(32.0)))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(row.name.clone()),
                        )
                        .when(!row.username.is_empty() && row.username != row.name, |el| {
                            el.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme.tokens.text_secondary)
                                    .child(row.username.clone()),
                            )
                        }),
                ),
        )
        .child(
            h_flex()
                .flex_shrink_0()
                .gap_x_2()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .child(if is_creator { creator_label } else { "" }),
                )
                .when_some(on_remove, |el, on_remove| {
                    el.child(
                        div()
                            .id((remove_id, user_id.get() as u64))
                            .size(px(32.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .tooltip(move |_, cx| Tooltip::build(remove_label, cx))
                            .cursor_pointer()
                            .on_click({
                                let on_remove = on_remove.clone();
                                move |_, _, cx| on_remove(cx)
                            })
                            .child(
                                Icon::new(IconName::EscIcon)
                                    .size(px(REMOVE_ICON_SIZE))
                                    .text_color(theme.tokens.text_theme_primary)
                                    .group_hover(group_name.clone(), |style| {
                                        style.text_color(rgb(REMOVE_HOVER_COLOR))
                                    }),
                            ),
                    )
                }),
        )
}

impl Render for PermissionsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();

        v_flex()
            .w_full()
            .text_size(px(15.0))
            .child(self.render_header(&locale, &theme, cx))
            .child(
                v_flex()
                    .mt_4()
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .child(self.render_private_card(&locale, &theme, cx))
                    .when(self.private_enabled, |el| {
                        el.child(self.render_access_panel(&locale, &theme, window, cx))
                    }),
            )
            .when(self.shows_everyone_override(cx), |el| {
                el.child(self.render_everyone_block(&locale, &theme, window, cx))
            })
            .when(self.overrides.is_some(), |el| {
                el.child(
                    div()
                        .mt_10()
                        .mb(px(30.0))
                        .h(px(1.0))
                        .w_full()
                        .bg(theme.tokens.border_primary),
                )
            })
            .children(self.overrides.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(count: usize) -> Vec<UserId> {
        (0..count).map(|ix| UserId(ix as i64 + 1)).collect()
    }

    #[test]
    fn overrides_apply_only_to_a_private_channel_that_is_not_voice() {
        assert!(overrides_apply_to(true, ChannelType::Text));
        assert!(!overrides_apply_to(false, ChannelType::Text));
        assert!(!overrides_apply_to(true, ChannelType::Voice));
        assert!(!overrides_apply_to(false, ChannelType::Voice));
    }

    #[test]
    fn everyone_override_is_hidden_on_private_thread_and_non_text() {
        assert!(channel_acl::everyone_applies_to(
            false,
            false,
            ChannelType::Text
        ));
        assert!(!channel_acl::everyone_applies_to(
            true,
            false,
            ChannelType::Text
        ));
        assert!(!channel_acl::everyone_applies_to(
            false,
            true,
            ChannelType::Text
        ));
        assert!(!channel_acl::everyone_applies_to(
            false,
            false,
            ChannelType::Voice
        ));
    }

    #[test]
    fn an_empty_list_still_counts_as_one_page() {
        assert_eq!(page_count(0, MEMBER_PAGE_SIZE), 1);
        assert!(page_slice::<UserId>(&[], 3, MEMBER_PAGE_SIZE).is_empty());
    }

    #[test]
    fn a_page_is_only_added_once_the_previous_one_is_full() {
        assert_eq!(page_count(1, MEMBER_PAGE_SIZE), 1);
        assert_eq!(page_count(MEMBER_PAGE_SIZE, MEMBER_PAGE_SIZE), 1);
        assert_eq!(page_count(MEMBER_PAGE_SIZE + 1, MEMBER_PAGE_SIZE), 2);
        assert_eq!(page_count(MEMBER_PAGE_SIZE * 3, MEMBER_PAGE_SIZE), 3);
        assert_eq!(page_count(11, 10), 2);
    }

    #[test]
    fn every_member_appears_on_exactly_one_page() {
        let all = ids(MEMBER_PAGE_SIZE * 2 + 7);
        let mut walked = Vec::new();
        for page in 0..page_count(all.len(), MEMBER_PAGE_SIZE) {
            walked.extend_from_slice(page_slice(&all, page, MEMBER_PAGE_SIZE));
        }
        assert_eq!(walked, all);
    }

    #[test]
    fn the_last_page_holds_the_remainder() {
        let all = ids(MEMBER_PAGE_SIZE + 3);
        assert_eq!(page_slice(&all, 1, MEMBER_PAGE_SIZE).len(), 3);
        assert_eq!(
            page_slice(&all, 1, MEMBER_PAGE_SIZE),
            &all[MEMBER_PAGE_SIZE..]
        );
    }

    #[test]
    fn a_page_past_the_end_falls_back_to_the_last_one() {
        let all = ids(MEMBER_PAGE_SIZE + 1);
        assert_eq!(
            page_slice(&all, 99, MEMBER_PAGE_SIZE),
            page_slice(&all, 1, MEMBER_PAGE_SIZE)
        );
        assert_eq!(page_slice(&ids(3), 7, MEMBER_PAGE_SIZE), ids(3).as_slice());
    }

    #[test]
    fn a_listing_only_row_prefers_the_display_name_and_keeps_the_avatar() {
        let row = member_row_from_profile(
            UserId(9),
            &ChannelUserProfile {
                username: "wumpus".into(),
                display_name: "Wumpus".into(),
                avatar: "https://cdn/avatar.png".into(),
            },
        );
        assert_eq!(row.user_id, UserId(9));
        assert_eq!(row.name, "Wumpus");
        assert_eq!(row.username, "wumpus");
        assert_eq!(row.avatar, "https://cdn/avatar.png");
    }

    #[test]
    fn a_listing_only_row_falls_back_to_the_username_for_its_label() {
        let row = member_row_from_profile(
            UserId(9),
            &ChannelUserProfile {
                username: "wumpus".into(),
                display_name: String::new(),
                avatar: String::new(),
            },
        );
        assert_eq!(row.name, "wumpus");
        assert_eq!(row.username, "wumpus");
        assert!(row.avatar.is_empty());
    }

    #[test]
    fn a_member_without_any_identity_still_has_a_distinguishable_label() {
        let row = member_row_from_profile(UserId(9), &ChannelUserProfile::default());
        assert_eq!(row.name, "9");
        assert!(row.username.is_empty());
    }

    #[test]
    fn a_missing_role_still_renders_a_removable_row() {
        let row = fallback_role_row(RoleId(42));
        assert_eq!(row.role_id, RoleId(42));
        assert_eq!(row.title.as_ref(), "42");
        assert!(row.icon.is_empty());
    }

    #[test]
    fn six_digit_hex_is_parsed() {
        assert_eq!(role_tint("#ff0000"), Hsla::from(rgb(0xff_00_00)));
        assert_eq!(role_tint("00ff00"), Hsla::from(rgb(0x00_ff_00)));
    }

    #[test]
    fn non_ascii_colors_fall_back_instead_of_panicking() {
        for color in ["日", "€", "ab日本", "#日本", "日本語"] {
            assert_eq!(role_tint(color), role_fallback_color());
        }
    }

    #[test]
    fn shorthand_hex_is_expanded() {
        assert_eq!(role_tint("#f00"), Hsla::from(rgb(0xff_00_00)));
        assert_eq!(role_tint("#abc"), Hsla::from(rgb(0xaa_bb_cc)));
    }

    #[test]
    fn eight_digit_hex_drops_alpha() {
        assert_eq!(role_tint("#1234567f"), Hsla::from(rgb(0x12_34_56)));
    }

    #[test]
    fn empty_or_invalid_falls_back_to_default_role_color() {
        assert_eq!(role_tint(""), role_fallback_color());
        assert_eq!(role_tint("not-a-color"), role_fallback_color());
        assert_eq!(role_tint("#gggggg"), role_fallback_color());
    }
}
