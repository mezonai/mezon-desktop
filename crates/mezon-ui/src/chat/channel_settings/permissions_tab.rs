use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FontWeight, Hsla, ListSizingBehavior, SharedString, Subscription, Window,
    div, img, prelude::*, px, rgb, size, uniform_list,
};
use mezon_store::{
    BadgeService, ChannelId, ChannelList, ChannelType, ChannelUserProfile, ChannelUsersEvent,
    ChannelUsersStore, ClanId, ClanMembersEvent, ClanMembersStore, RoleId, RolesStore, Settings,
    UserId,
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

pub(super) fn member_row(clan_id: ClanId, user_id: UserId, cx: &App) -> MemberRow {
    match ClanMembersStore::global(cx)
        .read(cx)
        .member(clan_id, user_id)
    {
        Some(member) => MemberRow {
            user_id,
            name: member.name().into(),
            username: member.user.username.clone().into(),
            avatar: member.avatar().into(),
        },
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
    let mut avatar = Avatar::new().name(row.name.clone()).size_px(size);
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

pub(super) fn role_row_from(role_id: RoleId, role: &mezon_store::ClanRoleDetail) -> RoleRow {
    RoleRow {
        role_id,
        title: role.name.clone().into(),
        icon: role.icon.clone().into(),
        color: role_tint(&role.color),
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
        ChannelUsersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(channel_id, cx);
        });
        RolesStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
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
                }
                cx.notify();
            }),
            cx.subscribe(
                &ChannelUsersStore::global(cx),
                |this, _, event: &ChannelUsersEvent, cx| {
                    let ChannelUsersEvent::Changed { channel_id } = event;
                    if *channel_id == this.channel_id {
                        this.refresh(cx);
                    }
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
            channel_fingerprint_seen: None,
            _subs: subs,
        };
        this.channel_fingerprint_seen = this.channel_fingerprint(cx);
        this.seed_from_store(cx);
        this.rebuild_rows(cx);
        this.sync_overrides(cx);
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
    fn shows_overrides(&self, cx: &App) -> bool {
        // Unknown yet means wait: the channel-list observer calls
        // `sync_overrides` again once the channel arrives.
        ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .is_some_and(|channel| channel.channel_type != ChannelType::Voice)
    }

    fn sync_overrides(&mut self, cx: &mut Context<Self>) {
        if self.overrides.is_some() || !self.shows_overrides(cx) {
            return;
        }
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let settings = self.settings.clone();
        let overrides = cx.new(|cx| PermissionOverrides::new(clan_id, channel_id, settings, cx));
        self.overrides_sub = Some(cx.observe(&overrides, |_, _, cx| cx.notify()));
        self.overrides = Some(overrides);
    }

    /// The settings shell paints this tab's save bar as a floating panel outside the
    /// scroll view, the same way the overview and integrations tabs do, so it stays put
    /// while the member list pages and scrolls.
    pub fn should_show_save_bar(&self, cx: &App) -> bool {
        self.private_enabled != self.private_initial || self.overrides_dirty(cx)
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
        match channel_users.profile(self.channel_id, user_id) {
            Some(profile) => member_matches("", &profile.display_name, &profile.username, needle),
            None => false,
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
        self.refresh(cx);
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.selected_role_ids.clear();
        self.selected_user_ids.clear();
        self.private_enabled = self.private_initial;
        if let Some(overrides) = self.overrides.clone() {
            overrides.update(cx, |overrides, cx| overrides.reset(cx));
        }
        self.refresh(cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.private_enabled != self.private_initial {
            self.save_private(cx);
        }
        if let Some(overrides) = self.overrides.clone() {
            overrides.update(cx, |overrides, cx| overrides.save(cx));
        }
    }

    fn save_private(&mut self, cx: &mut Context<Self>) {
        let Some(api) = channel_acl::api(cx) else {
            return;
        };
        let creator_id = match self.channel_creator_id(cx) {
            id if id.is_zero() => Self::current_user_id(cx),
            id => id,
        };
        let private_enabled = self.private_enabled;
        let user_ids =
            channel_acl::acl_user_ids(private_enabled, &self.selected_user_ids, Some(creator_id));
        let role_ids = channel_acl::acl_role_ids(private_enabled, &self.selected_role_ids);
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let staged_users = self.selected_user_ids.clone();
        let staged_roles = self.selected_role_ids.clone();

        cx.spawn(async move |this, cx| {
            let result = api
                .update_channel_private(
                    clan_id.get(),
                    channel_id.get(),
                    channel_acl::channel_private_payload(private_enabled),
                    user_ids,
                    role_ids,
                )
                .await;
            if let Err(error) = result {
                tracing::error!("update_channel_private failed for {channel_id}: {error}");
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.private_initial = private_enabled;
                this.selected_user_ids.clear();
                this.selected_role_ids.clear();
                if private_enabled {
                    ChannelUsersStore::global(cx).update(cx, |store, cx| {
                        store.add_users(channel_id, &staged_users, cx);
                    });
                    RolesStore::global(cx).update(cx, |store, cx| {
                        store.add_roles_to_channel(clan_id, channel_id, &staged_roles, cx);
                    });
                }
                this.refresh(cx);
            });
        })
        .detach();
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
                let AddMemRoleEvent::Staged { user_ids, role_ids } = event;
                for user_id in user_ids {
                    if !this.selected_user_ids.contains(user_id) {
                        this.selected_user_ids.push(*user_id);
                    }
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
        let enabled = self.private_enabled;
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
            .id("channel-private-toggle")
            .group("channel-private-toggle")
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
                    .group_hover("channel-private-toggle", move |style| {
                        style.bg(rgb(knob_hover))
                    }),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_private(cx)))
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
                        Some(row) => render_role_row(row, role_label, &theme, tab.clone(), cx)
                            .into_any_element(),
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
                        let row = channel_member_row(self.clan_id, self.channel_id, *user_id, cx);
                        render_member_row(
                            &row,
                            creator_id == *user_id,
                            creator_label,
                            theme,
                            tab.clone(),
                            locale,
                        )
                    }),
            )
            .into_any_element()
    }
}

fn render_role_row(
    row: &RoleRow,
    role_label: &'static str,
    theme: &Theme,
    tab: Entity<PermissionsTab>,
    cx: &mut App,
) -> impl IntoElement {
    let role_id = row.role_id;
    let group_name = SharedString::from(format!("channel-permission-role-{}", role_id.get()));
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
                .child(div().text_xs().child(role_label))
                .child(
                    div()
                        .id(("channel-permission-role-remove", role_id.get() as u64))
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::EscIcon)
                                .size(px(REMOVE_ICON_SIZE))
                                .text_color(theme.tokens.text_theme_primary),
                        )
                        .on_click(move |_, _, cx| {
                            tab.update(cx, |this, cx| this.remove_role(role_id, cx));
                        }),
                ),
        )
}

/// Floating save bar for the permissions tab, rendered by the settings shell outside the
/// scroll view so paging or scrolling the member list never moves it out of reach.
pub fn render_channel_permissions_save_bar(
    tab: Entity<PermissionsTab>,
    locale: &str,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let saving = tab
        .read(cx)
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

fn render_member_row(
    row: &MemberRow,
    is_creator: bool,
    creator_label: &'static str,
    theme: &Theme,
    tab: Entity<PermissionsTab>,
    locale: &str,
) -> impl IntoElement + use<> {
    let user_id = row.user_id;
    let group_name = SharedString::from(format!("channel-permission-member-{}", user_id.get()));
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
                .when(!is_creator, |el| {
                    el.child(
                        div()
                            .id(("channel-permission-member-remove", user_id.get() as u64))
                            .size(px(32.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .tooltip(move |_, cx| Tooltip::build(remove_label, cx))
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                tab.update(cx, |this, cx| this.remove_member(user_id, cx));
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
