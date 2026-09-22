use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FontWeight, Hsla, ListSizingBehavior, SharedString, Subscription,
    UniformListScrollHandle, Window, div, img, prelude::*, px, rgb, size, uniform_list,
};
use mezon_store::{
    BadgeService, ChannelId, ChannelList, ChannelType, ChannelUsersStore, ClanId, ClanMembersStore,
    RoleId, RolesStore, Settings, UserId,
};

use super::add_mem_role_modal::{AddMemRoleEvent, AddMemRoleModal};
use super::channel_acl;
use super::permission_overrides::PermissionOverrides;
use crate::chat::role_style::role_fallback_color;
use crate::components::primitives::{Avatar, Icon, IconName, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};

const TOGGLE_TRACK_WIDTH: f32 = 32.0;
const TOGGLE_TRACK_HEIGHT: f32 = 16.0;
const TOGGLE_KNOB_SIZE: f32 = 16.0;
pub(super) const ROLE_ROW_HEIGHT: f32 = 36.0;
pub(super) const MEMBER_ROW_HEIGHT: f32 = 40.0;
const REMOVE_ICON_SIZE: f32 = 15.0;

const TOGGLE_ON: u32 = 0x52_65_ec;
const TOGGLE_ON_HOVER: u32 = 0x46_54_c0;
const TOGGLE_OFF_TRACK: u32 = 0xcb_d5_e1;
const TOGGLE_OFF_TRACK_HOVER: u32 = 0x94_a3_b8;
const TOGGLE_OFF_KNOB: u32 = 0x64_74_8b;
const TOGGLE_OFF_KNOB_HOVER: u32 = 0x47_55_69;
const TOGGLE_KNOB_ON: u32 = 0xff_ff_ff;
const SYNC_ICON_COLOR: u32 = 0xf0_b0_33;
const REMOVE_HOVER_COLOR: u32 = 0xef_44_44;
const RESET_BUTTON_BG: u32 = 0x4b_55_63;
const SAVE_BUTTON_BG: u32 = 0x25_63_eb;

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
            name: SharedString::default(),
            username: SharedString::default(),
            avatar: SharedString::default(),
        },
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
    member_scroll: UniformListScrollHandle,
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
            cx.observe(&ChannelUsersStore::global(cx), |this, _, cx| {
                this.refresh(cx);
            }),
            cx.observe(&RolesStore::global(cx), |this, _, cx| this.refresh(cx)),
            cx.observe(&ClanMembersStore::global(cx), |_, _, cx| cx.notify()),
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
            member_scroll: UniformListScrollHandle::new(),
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
        &self,
        locale: &str,
        theme: &Theme,
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
            .child(
                v_flex()
                    .py_4()
                    .child(Self::render_section_label(
                        mezon_i18n::t(locale, "channelSetting.channelPermission.members"),
                        theme,
                    ))
                    .child(self.render_member_list(locale, cx)),
            )
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

    fn render_member_list(&self, locale: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ids = self.member_ids.clone();
        if ids.is_empty() {
            return div().into_any_element();
        }
        let clan_id = self.clan_id;
        let creator_id = self.channel_creator_id(cx);
        let creator_label =
            mezon_i18n::t(locale, "channelSetting.channelPermission.ChannelCreator");
        let tab = cx.entity();
        let count = ids.len();
        uniform_list(
            "channel-permission-members",
            count,
            move |range, _window, cx| {
                let theme = cx.theme().clone();
                range
                    .map(|ix| match ids.get(ix) {
                        Some(user_id) => {
                            let row = member_row(clan_id, *user_id, cx);
                            let is_creator = !creator_id.is_zero() && creator_id == *user_id;
                            render_member_row(&row, is_creator, creator_label, &theme, tab.clone())
                                .into_any_element()
                        }
                        None => div().h(px(MEMBER_ROW_HEIGHT)).into_any_element(),
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_item_size(size(px(0.0), px(MEMBER_ROW_HEIGHT)))
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .track_scroll(&self.member_scroll)
        .w_full()
        .into_any_element()
    }

    fn render_save_bar(&self, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .relative()
            .mt_8()
            .w_full()
            .max_w(px(815.0))
            .gap_2()
            .p_3()
            .pr_0()
            .rounded(px(4.0))
            .justify_end()
            .text_color(gpui::white())
            .child(
                div()
                    .id("channel-permission-reset")
                    .p(px(8.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .bg(rgb(RESET_BUTTON_BG))
                    .child(mezon_i18n::t(locale, "channelSetting.unsavedChanges.reset"))
                    .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
            )
            .child(
                div()
                    .id("channel-permission-save")
                    .p(px(8.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .bg(rgb(SAVE_BUTTON_BG))
                    .child(mezon_i18n::t(
                        locale,
                        "channelSetting.unsavedChanges.saveChanges",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
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

fn render_member_row(
    row: &MemberRow,
    is_creator: bool,
    creator_label: &'static str,
    theme: &Theme,
    tab: Entity<PermissionsTab>,
) -> impl IntoElement {
    let user_id = row.user_id;
    let group_name = SharedString::from(format!("channel-permission-member-{}", user_id.get()));
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
                .min_w_0()
                .gap_x_2()
                .items_center()
                .child(member_avatar(row, px(24.0)))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tokens.text_secondary)
                        .child(row.name.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::LIGHT)
                        .child(row.username.clone()),
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
                .child(
                    div()
                        .id(("channel-permission-member-remove", user_id.get() as u64))
                        .when(!is_creator, |el| {
                            el.cursor_pointer().on_click(move |_, _, cx| {
                                tab.update(cx, |this, cx| this.remove_member(user_id, cx));
                            })
                        })
                        .child(
                            Icon::new(IconName::EscIcon)
                                .size(px(REMOVE_ICON_SIZE))
                                .text_color(if is_creator {
                                    theme.tokens.text_secondary
                                } else {
                                    theme.tokens.text_theme_primary
                                })
                                .when(!is_creator, |icon| {
                                    icon.group_hover(group_name.clone(), |style| {
                                        style.text_color(rgb(REMOVE_HOVER_COLOR))
                                    })
                                }),
                        ),
                ),
        )
}

impl Render for PermissionsTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let dirty = self.private_enabled != self.private_initial || self.overrides_dirty(cx);

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
                        el.child(self.render_access_panel(&locale, &theme, cx))
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
            .when(dirty, |el| el.child(self.render_save_bar(&locale, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
