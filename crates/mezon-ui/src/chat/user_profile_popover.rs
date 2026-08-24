use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, CursorStyle, DismissEvent, Div, ElementId,
    EventEmitter, FocusHandle, Focusable, FontWeight, MouseButton, MouseDownEvent, ParentElement,
    Render, SharedString, Stateful, StyleRefinement, Styled, Window, deferred, div, img,
    prelude::*, px, svg,
};
use mezon_store::{
    BadgeService, ChannelList, ClanId, ClanMembersStore, DirectMessageBody, DirectMessageStore,
    FriendState, FriendStore, PERMISSION_CLAN_OWNER, PERMISSION_MANAGE_CLAN, PermissionStore,
    PresenceStore, ProfileContext, RoleId, RolesStore, Settings, UserId, current_user_status,
    resolve_user_profile,
};
use ui::{Clickable, PopoverMenu, Toggleable};

use crate::app::shell::{FriendRemovalKind, Shell};
use crate::chat::message::{SendTokenModal, ShareContactModal, share_contact_subject};
use crate::components::primitives::{Avatar, Icon, IconName, Input, InputEvent, InputState};
use crate::image_cache::LruImageCache;
use crate::router::{Route, navigate};
use crate::theme::{ActiveTheme, Theme};

const BANNER_HEIGHT: f32 = 105.;
const AVATAR_SIZE: f32 = 90.;
const AVATAR_BORDER: f32 = 6.;
const COLLAPSED_ROLE_CHIPS: usize = 6;

pub(crate) fn role_is_assignable(
    current_level: Option<i32>,
    role_max_level_permission: i32,
) -> bool {
    let role_level = if role_max_level_permission == 0 {
        -1
    } else {
        role_max_level_permission
    };
    current_level.is_some_and(|level| level > role_level)
}

#[derive(Clone)]
struct RoleChip {
    id: RoleId,
    name: SharedString,
    color: gpui::Rgba,
    icon: SharedString,
}

#[derive(Default)]
struct RoleSection {
    can_edit: bool,
    assigned: Vec<RoleChip>,
    candidates: Vec<RoleChip>,
}

pub struct UserProfilePopover {
    focus_handle: FocusHandle,
    user_id: UserId,
    context: ProfileContext,
    settings: gpui::Entity<Settings>,
    avatar_image_cache: gpui::Entity<LruImageCache>,
    message_input: gpui::Entity<InputState>,
    role_search: gpui::Entity<InputState>,
    roles: RoleSection,
    roles_dirty: bool,
    add_role_open: bool,
    show_all_roles: bool,
    friend_menu_open: bool,
    sending_message: bool,
    _roles_sub: Option<gpui::Subscription>,
    _clan_members_sub: gpui::Subscription,
    _permissions_sub: Option<gpui::Subscription>,
    _friend_sub: gpui::Subscription,
    _presence_sub: gpui::Subscription,
    _channel_sub: Option<gpui::Subscription>,
    _input_sub: gpui::Subscription,
    _role_search_sub: gpui::Subscription,
}

impl UserProfilePopover {
    pub fn new(
        user_id: UserId,
        context: ProfileContext,
        settings: gpui::Entity<Settings>,
        avatar_image_cache: gpui::Entity<LruImageCache>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let locale = settings.read(cx).language.clone();
        let message_ph = mezon_i18n::t(&locale, "userProfile.placeholders.messageUser")
            .replace("{{username}}", "");
        let message_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(message_ph)
                .text_size(px(14.))
        });
        let input_sub = cx.subscribe(
            &message_input,
            |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter) {
                    this.send_message(cx);
                }
            },
        );

        let role_search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(mezon_i18n::t(&locale, "userProfile.labels.role"))
                .text_size(px(14.))
                .borderless()
        });
        let role_search_sub = cx.subscribe(
            &role_search,
            |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.roles_dirty = true;
                    cx.notify();
                }
            },
        );

        let roles_sub = RolesStore::try_global(cx).map(|roles_store| {
            cx.observe(&roles_store, |this: &mut Self, _, cx| {
                this.roles_dirty = true;
                cx.notify();
            })
        });
        let clan_members_sub =
            cx.observe(&ClanMembersStore::global(cx), |this: &mut Self, _, cx| {
                this.roles_dirty = true;
                cx.notify();
            });
        let permissions_sub = PermissionStore::try_global(cx).map(|permission_store| {
            cx.observe(&permission_store, |this: &mut Self, _, cx| {
                this.roles_dirty = true;
                cx.notify();
            })
        });
        let friend_sub = cx.observe(&FriendStore::global(cx), |_, _, cx| cx.notify());
        let presence_sub = cx.observe(&PresenceStore::global(cx), |_, _, cx| cx.notify());
        let channel_sub = matches!(context, ProfileContext::Clan(_))
            .then(|| cx.observe(&ChannelList::global(cx), |_, _, cx| cx.notify()));

        Self {
            focus_handle,
            user_id,
            context,
            settings,
            avatar_image_cache,
            message_input,
            role_search,
            roles: RoleSection::default(),
            roles_dirty: true,
            add_role_open: false,
            show_all_roles: false,
            friend_menu_open: false,
            sending_message: false,
            _roles_sub: roles_sub,
            _clan_members_sub: clan_members_sub,
            _permissions_sub: permissions_sub,
            _friend_sub: friend_sub,
            _presence_sub: presence_sub,
            _channel_sub: channel_sub,
            _input_sub: input_sub,
            _role_search_sub: role_search_sub,
        }
    }

    fn clan_id(&self) -> Option<ClanId> {
        match self.context {
            ProfileContext::Clan(clan_id) => Some(clan_id),
            ProfileContext::Direct(_) => None,
        }
    }

    fn compute_roles(&self, cx: &App) -> RoleSection {
        let Some(clan_id) = self.clan_id() else {
            return RoleSection::default();
        };
        let (Some(roles_store), Some(permission_store)) =
            (RolesStore::try_global(cx), PermissionStore::try_global(cx))
        else {
            return RoleSection::default();
        };
        let assigned_ids = resolve_user_profile(self.user_id, self.context, cx)
            .map(|profile| profile.role_ids)
            .unwrap_or_default();
        let permissions = permission_store.read(cx);
        let can_edit = permissions.check(clan_id, None, PERMISSION_MANAGE_CLAN, cx);
        let is_clan_owner = permissions.check(clan_id, None, PERMISSION_CLAN_OWNER, cx);
        let level = permissions.current_permission_level(clan_id, cx);
        let query = self.role_search.read(cx).value().trim().to_lowercase();

        let roles = roles_store.read(cx);
        let mut section = RoleSection {
            can_edit,
            ..RoleSection::default()
        };
        for (role_id, role) in roles.active_roles_in_clan(clan_id) {
            if assigned_ids.contains(&role_id) {
                section.assigned.push(RoleChip {
                    id: role_id,
                    name: role.name.clone().into(),
                    color: chip_color(&role.color),
                    icon: crate::util::imgproxy::role_icon_url(cx, &role.icon).into(),
                });
                continue;
            }
            if roles.is_everyone_role(clan_id, role) {
                continue;
            }
            if !query.is_empty() && !role.name.to_lowercase().contains(&query) {
                continue;
            }
            if !is_clan_owner && !role_is_assignable(level, role.max_level_permission) {
                continue;
            }
            section.candidates.push(RoleChip {
                id: role_id,
                name: role.name.clone().into(),
                color: chip_color(&role.color),
                icon: crate::util::imgproxy::role_icon_url(cx, &role.icon).into(),
            });
        }
        section
    }

    fn mutate_role(&mut self, role_id: RoleId, add: bool, cx: &mut Context<Self>) {
        let Some(clan_id) = self.clan_id() else {
            return;
        };
        let Some(roles_store) = RolesStore::try_global(cx) else {
            return;
        };
        let user_id = self.user_id.get();
        let (add_ids, remove_ids) = if add {
            (vec![user_id], Vec::new())
        } else {
            (Vec::new(), vec![user_id])
        };
        let started = roles_store.update(cx, |store, cx| {
            store.mutate_role_members(clan_id, role_id, add_ids, remove_ids, cx)
        });
        if !started {
            return;
        }
        self.roles_dirty = true;
        cx.notify();
    }

    fn render_roles(&self, locale: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let total = self.roles.assigned.len();
        let overflow = total.saturating_sub(COLLAPSED_ROLE_CHIPS);
        let visible = if self.show_all_roles {
            total
        } else {
            total.min(COLLAPSED_ROLE_CHIPS)
        };

        let chips = div()
            .id("profile-role-chips")
            .mt_2()
            .flex()
            .flex_wrap()
            .gap_2()
            .when(self.show_all_roles, |el| {
                el.max_h(px(100.)).min_h_0().overflow_y_scroll()
            })
            .children(
                self.roles.assigned[..visible]
                    .iter()
                    .map(|chip| self.render_role_chip(chip, locale, &theme, cx)),
            )
            .when(overflow > 0 && !self.show_all_roles, |el| {
                el.child(
                    role_expander_pill(
                        "profile-roles-more",
                        format!("+ {overflow}"),
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.show_all_roles = true;
                            cx.notify();
                        }),
                    )
                    .ml_1(),
                )
            });

        div()
            .flex()
            .flex_col()
            .child(chips)
            .when(overflow > 0 && self.show_all_roles, |el| {
                el.child(
                    div()
                        .mt_1()
                        .flex()
                        .justify_start()
                        .child(role_expander_pill(
                            "profile-roles-less",
                            mezon_i18n::t(locale, "userProfile.labels.showLess"),
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.show_all_roles = false;
                                cx.notify();
                            }),
                        )),
                )
            })
            .when(self.roles.can_edit, |el| {
                el.child(self.render_add_role(locale, &theme, cx))
            })
            .into_any_element()
    }

    fn render_role_chip(
        &self,
        chip: &RoleChip,
        locale: &str,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let icon_cache = crate::image_cache::role_icon_cache(cx);
        let role_id = chip.id;
        let color = chip.color;
        let group: SharedString = format!("role-chip-{}", role_id.get()).into();
        div()
            .flex()
            .items_center()
            .gap_x_1()
            .rounded(px(4.))
            .p_1()
            .bg(theme.tokens.bg_active_member_channel)
            .text_color(theme.tokens.text_theme_primary)
            .when(self.roles.can_edit, |el| {
                el.child(
                    div()
                        .id(("profile-role-remove", role_id.get() as u64))
                        .group(group.clone())
                        .p(px(2.))
                        .rounded_full()
                        .bg(color)
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::IconRemove)
                                .size(px(8.))
                                .text_color(color)
                                .group_hover(group.clone(), |style| {
                                    style.text_color(gpui::black())
                                }),
                        )
                        .tooltip(ui::Tooltip::text(mezon_i18n::t(
                            locale,
                            "userProfile.labels.removeRole",
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.mutate_role(role_id, false, cx);
                        })),
                )
            })
            .when(!self.roles.can_edit, |el| {
                el.child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(color))
            })
            .when(!chip.icon.is_empty(), |el| {
                el.child(
                    img(chip.icon.clone())
                        .size(px(12.))
                        .flex_shrink_0()
                        .when_some(icon_cache.clone(), |el, cache| el.image_cache(&cache)),
                )
            })
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .max_w(px(120.))
                    .overflow_hidden()
                    .truncate()
                    .child(chip.name.clone()),
            )
            .into_any_element()
    }

    fn render_add_role(&self, locale: &str, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .mt_1()
            .border_1()
            .border_color(theme.tokens.border_primary)
            .when(self.add_role_open, |el| {
                el.child(deferred(self.render_add_role_panel(locale, theme, cx)))
            })
            .child(
                div()
                    .id("profile-add-role")
                    .flex()
                    .items_center()
                    .gap_x_1()
                    .rounded(px(4.))
                    .p_1()
                    .cursor_pointer()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(
                        Icon::new(IconName::Plus)
                            .size(px(20.))
                            .text_color(theme.tokens.text_theme_primary),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .child(mezon_i18n::t(locale, "userProfile.labels.addRole")),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_role_open = !this.add_role_open;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_add_role_panel(&self, locale: &str, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let list = div()
            .id("profile-role-candidates")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_1()
            .overflow_y_scroll()
            .when(self.roles.candidates.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .py_4()
                        .gap_y_4()
                        .items_center()
                        .text_color(theme.tokens.text_secondary)
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(mezon_i18n::t(locale, "userProfile.labels.nope")),
                        )
                        .child(div().child(mezon_i18n::t(locale, "userProfile.labels.typoError"))),
                )
            })
            .children(
                self.roles
                    .candidates
                    .iter()
                    .map(|chip| self.render_role_candidate(chip, theme, cx)),
            );

        div()
            .occlude()
            .absolute()
            .bottom(px(32.))
            .left_0()
            .w_full()
            .max_h(px(240.))
            .flex()
            .flex_col()
            .gap_3()
            .rounded_lg()
            .shadow_lg()
            .bg(theme.tokens.theme_setting_primary)
            .text_color(theme.tokens.text_theme_primary)
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(36.))
                    .child(
                        div()
                            .w_full()
                            .rounded_tl_lg()
                            .rounded_tr_lg()
                            .bg(theme.tokens.theme_setting_nav)
                            .child(
                                Input::new(&self.role_search)
                                    .w_full()
                                    .text_color(theme.tokens.text_theme_primary),
                            ),
                    )
                    .child(
                        div().absolute().right(px(8.)).top(px(8.)).child(
                            Icon::new(IconName::Search)
                                .size(px(20.))
                                .text_color(theme.tokens.text_theme_primary),
                        ),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    fn render_role_candidate(
        &self,
        chip: &RoleChip,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let icon_cache = crate::image_cache::role_icon_cache(cx);
        let role_id = chip.id;
        div()
            .id(("profile-role-candidate", role_id.get() as u64))
            .w_full()
            .p_2()
            .flex()
            .items_center()
            .gap_2()
            .text_base()
            .cursor_pointer()
            .text_color(theme.tokens.text_theme_primary)
            .hover(|style| style.bg(theme.tokens.bg_item_hover))
            .child(
                div()
                    .size(px(12.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(chip.color),
            )
            .when(!chip.icon.is_empty(), |el| {
                el.child(
                    img(chip.icon.clone())
                        .size(px(12.))
                        .flex_shrink_0()
                        .when_some(icon_cache.clone(), |el, cache| el.image_cache(&cache)),
                )
            })
            .child(div().overflow_hidden().truncate().child(chip.name.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.add_role_open = false;
                    this.mutate_role(role_id, true, cx);
                }),
            )
            .into_any_element()
    }

    fn send_message(&mut self, cx: &mut Context<Self>) {
        if self.sending_message {
            return;
        }
        let content = self.message_input.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }
        let Some(profile) = resolve_user_profile(self.user_id, self.context, cx) else {
            return;
        };
        if profile.username.is_empty() {
            return;
        }
        self.sending_message = true;
        cx.notify();
        let user_id = self.user_id;
        let label = profile.display_name.clone();
        let avatar = profile.avatar_url.clone();
        let username = profile.username.clone();
        let task = DirectMessageStore::global(cx).update(cx, |store, cx| {
            store.create_dm_and_send_text(
                user_id,
                label,
                avatar,
                username.clone(),
                DirectMessageBody::Text(content),
                cx,
            )
        });
        cx.spawn(async move |this, cx| match task.await {
            Ok((channel_id, channel_type)) => {
                let _ = this.update(cx, |this, cx| {
                    this.sending_message = false;
                    cx.emit(DismissEvent);
                });
                cx.update(|cx| {
                    navigate(
                        cx,
                        Route::DirectMessage {
                            direct_id: channel_id,
                            message_type: channel_type.to_string(),
                        },
                    );
                });
            }
            Err(err) => {
                tracing::warn!("profile message send failed: {err}");
                let _ = this.update(cx, |this, cx| {
                    this.sending_message = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

impl Focusable for UserProfilePopover {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for UserProfilePopover {}

impl Render for UserProfilePopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let profile = resolve_user_profile(self.user_id, self.context, cx);
        let locale: SharedString = self.settings.read(cx).language.clone().into();

        let message_placeholder = mezon_i18n::t(&locale, "userProfile.placeholders.messageUser")
            .replace(
                "{{username}}",
                profile
                    .as_ref()
                    .map(|p| p.display_name.as_str())
                    .unwrap_or(""),
            );
        self.message_input.update(cx, |input, cx| {
            input.set_placeholder(message_placeholder, cx);
        });

        let (display_name, username, avatar_raw, about_me, join_time, online) = match &profile {
            Some(p) => (
                SharedString::from(p.display_name.as_str()),
                SharedString::from(p.username.as_str()),
                p.avatar_url.clone(),
                SharedString::from(p.about_me.as_str()),
                p.join_time_seconds,
                p.online,
            ),
            None => (
                SharedString::default(),
                SharedString::default(),
                String::new(),
                SharedString::default(),
                0u32,
                false,
            ),
        };

        let own_status = current_user_status(cx)
            .filter(|(id, _)| *id == self.user_id)
            .map(|(_, status)| status);
        let custom_status = match &own_status {
            Some(status) => status.custom_status.clone(),
            None => PresenceStore::global(cx)
                .read(cx)
                .user_status(self.user_id)
                .unwrap_or("")
                .to_string(),
        };

        let member_since = format_member_since(join_time);
        let status_presence = match &own_status {
            Some(status) => status.presence,
            None if online => mezon_store::UserPresence::Online,
            None => mezon_store::UserPresence::Invisible,
        };
        let status_icon = crate::util::user_status::status_icon(status_presence);

        let avatar_proxied = if avatar_raw.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(crate::util::imgproxy::avatar_url(cx, &avatar_raw))
        };
        let mut avatar = Avatar::new()
            .name(display_name.clone())
            .size_px(px(AVATAR_SIZE))
            .image_cache(self.avatar_image_cache.clone());
        if !avatar_proxied.is_empty() {
            avatar = avatar
                .src(avatar_proxied)
                .fallback_src(SharedString::from(avatar_raw.clone()));
        } else if !avatar_raw.is_empty() {
            avatar = avatar.src(SharedString::from(avatar_raw.clone()));
        }

        let is_clan = matches!(self.context, ProfileContext::Clan(_));
        let is_dm = matches!(self.context, ProfileContext::Direct(_));
        if self.roles_dirty {
            self.roles = self.compute_roles(cx);
            self.roles_dirty = false;
        }
        let assigned_role_count = self.roles.assigned.len();
        let can_edit_roles = self.roles.can_edit;
        let show_roles_section = is_clan && (assigned_role_count > 0 || can_edit_roles);

        let me = BadgeService::global(cx).read(cx).current_user_id(cx);
        let is_self = me == Some(self.user_id);
        let friend_info = FriendStore::global(cx).read(cx).friend(self.user_id);
        let friend_state = friend_info.map(|f| f.state);
        let is_friend = friend_state == Some(FriendState::Friend);
        let did_i_block =
            friend_info.is_some_and(|f| f.state == FriendState::Blocked && Some(f.source_id) == me);
        let is_blocked = friend_state == Some(FriendState::Blocked);
        let show_share_contact = !is_self && is_friend && !did_i_block;
        let show_message_input =
            !username.is_empty() && !is_blocked && !is_self && !self.sending_message;

        let voice_info = (!is_dm && !is_self)
            .then(|| {
                ChannelList::global(cx)
                    .read(cx)
                    .in_voice_status(self.user_id)
            })
            .flatten();

        let banner_actions = if !is_self && !is_blocked {
            render_banner_actions(self, friend_state, show_share_contact, locale.clone(), cx)
        } else {
            Vec::new()
        };

        let theme = cx.theme();
        let banner_bg = gpui::rgb(0x818cf8);

        div()
            .occlude()
            .w(px(300.))
            .overflow_hidden()
            .rounded_lg()
            .bg(theme.bg_floating)
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .child(
                div()
                    .h(px(BANNER_HEIGHT))
                    .rounded_tl_lg()
                    .rounded_tr_lg()
                    .bg(banner_bg)
                    .flex()
                    .flex_row()
                    .justify_end()
                    .items_start()
                    .gap_2()
                    .p_2()
                    .children(
                        voice_info.map(|info| {
                            render_voice_button(info.clan_id, info.channel_id, theme, cx)
                        }),
                    )
                    .children(banner_actions),
            )
            .child(render_avatar_row(
                avatar,
                status_icon,
                crate::util::user_status::status_color(status_presence, theme),
                custom_status,
                theme,
            ))
            .child(
                div().px(px(16.)).child(
                    div()
                        .rounded(px(10.))
                        .p_2()
                        .my(px(16.))
                        .border_1()
                        .border_color(theme.tokens.theme_border_input)
                        .shadow_sm()
                        .bg(theme.tokens.bg_active_member_channel)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_lg()
                                .text_color(theme.tokens.text_theme_primary)
                                .overflow_hidden()
                                .truncate()
                                .child(display_name.clone()),
                        )
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.tokens.text_theme_primary)
                                .overflow_hidden()
                                .truncate()
                                .child(username.clone()),
                        )
                        .when(friend_state == Some(FriendState::InviteReceived), |d| {
                            d.child(render_pending_friend(
                                self.user_id,
                                username.as_ref(),
                                locale.as_ref(),
                            ))
                        })
                        .when(!is_dm && !about_me.is_empty(), |d| {
                            d.child(section_divider(theme.tokens.theme_border_input))
                                .child(section_label(
                                    mezon_i18n::t(&locale, "userProfile.labels.aboutMe"),
                                    theme.tokens.text_theme_primary,
                                ))
                                .child(
                                    div()
                                        .mt_1()
                                        .text_sm()
                                        .text_color(theme.tokens.text_secondary)
                                        .child(about_me.clone()),
                                )
                        })
                        .when(!is_dm && join_time > 0, |d| {
                            d.child(section_divider(theme.tokens.theme_border_input))
                                .child(section_label(
                                    mezon_i18n::t(&locale, "userProfile.labels.memberSince"),
                                    theme.tokens.text_theme_primary,
                                ))
                                .child(
                                    div()
                                        .mt_1()
                                        .text_sm()
                                        .text_color(theme.tokens.text_secondary)
                                        .child(member_since),
                                )
                        })
                        .when(show_roles_section, |d| {
                            d.child(section_divider(theme.tokens.theme_border_input))
                                .child(section_label(
                                    mezon_i18n::t(&locale, "userProfile.aboutMe.roles.headerTitle"),
                                    theme.tokens.text_theme_primary,
                                ))
                                .child(self.render_roles(&locale, cx))
                        })
                        .when(show_message_input, |d| {
                            d.child(
                                div()
                                    .occlude()
                                    .mt_2()
                                    .rounded(px(5.))
                                    .border_1()
                                    .border_color(theme.tokens.theme_border_input)
                                    .bg(theme.tokens.bg_theme_contexify)
                                    .child(
                                        Input::new(&self.message_input)
                                            .w_full()
                                            .text_color(theme.tokens.text_theme_primary),
                                    ),
                            )
                        })
                        .when(username.is_empty() && !is_self && !is_blocked, |d| {
                            d.child(
                                div()
                                    .mt_2()
                                    .p_2()
                                    .rounded(px(5.))
                                    .text_center()
                                    .text_sm()
                                    .italic()
                                    .bg(theme.tokens.bg_active_member_channel)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(mezon_i18n::t(
                                        &locale,
                                        "userProfile.labels.userNotFound",
                                    )),
                            )
                        })
                        .when(is_self, |d| {
                            d.child(
                                div()
                                    .id("profile-edit")
                                    .mt_2()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h(px(36.))
                                    .rounded(px(4.))
                                    .bg(theme.tokens.bg_button_secondary)
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.85))
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(mezon_i18n::t(&locale, "userProfile.labels.editProfile"))
                                    .on_click(cx.listener(|_, _: &ClickEvent, _window, cx| {
                                        cx.emit(gpui::DismissEvent);
                                        crate::router::navigate(
                                            cx,
                                            crate::router::Route::SettingsProfile,
                                        );
                                    })),
                            )
                        }),
                ),
            )
    }
}

const BANNER_ICON_BG: u32 = 0x272120;
const BANNER_ICON_BG_HOVER: u32 = 0x1e1a19;
const BANNER_ICON_PENDING_BG: u32 = 0x4e5058;
const SHARE_CONTACT_BODY: u32 = 0x656369;
const SHARE_CONTACT_CHECK: u32 = 0x549d5b;

pub(crate) fn share_contact_icon() -> gpui::AnyElement {
    div()
        .relative()
        .size(px(16.))
        .child(
            svg()
                .path("icons/icon-share-contact-base.svg")
                .size(px(16.))
                .flex_none()
                .text_color(gpui::rgb(SHARE_CONTACT_BODY)),
        )
        .child(
            svg()
                .path("icons/icon-share-contact-accent.svg")
                .absolute()
                .top_0()
                .left_0()
                .size(px(16.))
                .flex_none()
                .text_color(gpui::rgb(SHARE_CONTACT_CHECK)),
        )
        .into_any_element()
}

pub(crate) fn banner_icon_shell(
    id: impl Into<ElementId>,
    pending_style: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    icon: AnyElement,
) -> gpui::AnyElement {
    let bg = if pending_style {
        gpui::rgb(BANNER_ICON_PENDING_BG)
    } else {
        gpui::rgb(BANNER_ICON_BG)
    };
    let bg_hover = if pending_style {
        gpui::rgb(BANNER_ICON_PENDING_BG)
    } else {
        gpui::rgb(BANNER_ICON_BG_HOVER)
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(32.))
        .rounded_full()
        .bg(bg)
        .hover(|s| s.bg(bg_hover))
        .cursor(CursorStyle::PointingHand)
        .on_click(on_click)
        .child(icon)
        .into_any_element()
}

pub(crate) fn banner_icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    pending_style: bool,
    icon_white: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let mut icon_el = Icon::new(icon).size(px(16.));
    if icon_white {
        icon_el = icon_el.text_color(gpui::white());
    } else {
        icon_el = icon_el.text_color(gpui::hsla(0., 0., 0.92, 1.));
    }
    banner_icon_shell(id, pending_style, on_click, icon_el.into_any_element())
}

fn render_banner_actions(
    this: &UserProfilePopover,
    friend_state: Option<FriendState>,
    show_share_contact: bool,
    locale: SharedString,
    cx: &mut Context<UserProfilePopover>,
) -> Vec<AnyElement> {
    let mut buttons = Vec::new();
    let entity = cx.entity();

    buttons.push(banner_icon_shell(
        "profile-transfer",
        false,
        cx.listener(|this, _: &ClickEvent, window, cx| {
            let profile = resolve_user_profile(this.user_id, this.context, cx);
            let username = profile
                .as_ref()
                .map(|p| {
                    if !p.display_name.is_empty() {
                        p.display_name.clone()
                    } else {
                        p.username.clone()
                    }
                })
                .unwrap_or_default();
            let recipient_id = this.user_id.0.to_string();
            let locale: SharedString = this.settings.read(cx).language.clone().into();
            SendTokenModal::open(locale, Some((recipient_id, username)), window, cx);
        }),
        Icon::new(IconName::Transaction)
            .size(px(16.))
            .text_color(gpui::white())
            .into_any_element(),
    ));

    if show_share_contact {
        buttons.push(banner_icon_shell(
            "profile-share-contact",
            false,
            cx.listener(|this, _: &ClickEvent, window, cx| {
                let fallback = resolve_user_profile(this.user_id, this.context, cx)
                    .map(|p| p.display_name)
                    .unwrap_or_default();
                let contact =
                    share_contact_subject(this.user_id, &fallback, Some(this.context), cx);
                let locale = this.settings.read(cx).language.clone().into();
                ShareContactModal::open(contact, locale, window, cx);
            }),
            share_contact_icon(),
        ));
    }

    match friend_state {
        Some(FriendState::Friend) => {
            let user_id = this.user_id;
            let remove_username = resolve_user_profile(user_id, this.context, cx)
                .map(|p| p.username)
                .unwrap_or_default();
            buttons.push(
                div()
                    .relative()
                    .child(
                        banner_icon_button("profile-friend", IconName::IconFriend, false, false, {
                            let entity = entity.clone();
                            move |_: &ClickEvent, _window, cx| {
                                entity.update(cx, |this, cx| {
                                    this.friend_menu_open = !this.friend_menu_open;
                                    cx.notify();
                                });
                            }
                        })
                        .into_any_element(),
                    )
                    .when(this.friend_menu_open, |el| {
                        el.child(render_friend_menu(
                            entity.clone(),
                            locale.clone(),
                            user_id,
                            remove_username.clone(),
                            cx,
                        ))
                    })
                    .into_any_element(),
            );
        }
        Some(FriendState::InviteSent) => {
            buttons.push(
                banner_icon_button(
                    "profile-pending",
                    IconName::PendingFriend,
                    true,
                    false,
                    |_: &ClickEvent, _, _| {},
                )
                .into_any_element(),
            );
        }
        Some(FriendState::InviteReceived) => {
            buttons.push(
                banner_icon_button("profile-accept", IconName::IConAcceptFriend, true, false, {
                    let user_id = this.user_id;
                    move |_: &ClickEvent, _window, cx| {
                        FriendStore::global(cx)
                            .update(cx, |store, cx| store.accept_friend(user_id, cx));
                    }
                })
                .into_any_element(),
            );
            buttons.push(
                banner_icon_button("profile-ignore", IconName::IConIgnoreFriend, true, false, {
                    let user_id = this.user_id;
                    let username = resolve_user_profile(user_id, this.context, cx)
                        .map(|p| p.username)
                        .unwrap_or_default();
                    let locale_str = locale.to_string();
                    move |_: &ClickEvent, window, cx| {
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.confirm_remove_friend(
                                user_id,
                                &username,
                                FriendRemovalKind::RejectRequest,
                                &locale_str,
                                window,
                                cx,
                            );
                        });
                    }
                })
                .into_any_element(),
            );
        }
        _ => {
            if let Some(profile) = resolve_user_profile(this.user_id, this.context, cx) {
                let user_id = this.user_id;
                let username = profile.username.clone();
                let display_name = profile.display_name.clone();
                let avatar = profile.avatar_url.clone();
                buttons.push(
                    banner_icon_button(
                        "profile-add-friend",
                        IconName::AddPerson,
                        false,
                        false,
                        move |_: &ClickEvent, _window, cx| {
                            FriendStore::global(cx).update(cx, |store, cx| {
                                store.add_friend(
                                    user_id,
                                    username.clone(),
                                    display_name.clone(),
                                    avatar.clone(),
                                    cx,
                                );
                            });
                        },
                    )
                    .into_any_element(),
                );
            }
        }
    }

    buttons
}

fn render_friend_menu(
    entity: gpui::Entity<UserProfilePopover>,
    locale: SharedString,
    user_id: UserId,
    username: String,
    cx: &mut Context<UserProfilePopover>,
) -> AnyElement {
    let locale_str = locale.to_string();
    let locale_label = locale_str.clone();
    let theme = cx.theme();

    div()
        .absolute()
        .top(px(36.))
        .right_0()
        .w(px(165.))
        .p_2()
        .rounded_lg()
        .bg(theme.bg_floating)
        .shadow_lg()
        .child(
            ClickableContainer::new("profile-remove-friend")
                .cursor(CursorStyle::PointingHand)
                .on_click({
                    move |_: &ClickEvent, window, cx| {
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.confirm_remove_friend(
                                user_id,
                                &username,
                                FriendRemovalKind::RemoveFriend,
                                &locale_str,
                                window,
                                cx,
                            );
                        });
                        entity.update(cx, |this, cx| {
                            this.friend_menu_open = false;
                            cx.notify();
                        });
                    }
                })
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded(px(4.))
                        .text_sm()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(mezon_i18n::t(
                            locale_label.as_str(),
                            "userProfile.pendingContent.removeFriend",
                        )),
                ),
        )
        .into_any_element()
}

fn render_pending_friend(user_id: UserId, username: &str, locale: &str) -> AnyElement {
    let request_text = format!("{username} sent you a friend request.");
    let username_owned = username.to_string();
    let locale_owned = locale.to_string();

    div()
        .mt_2()
        .p_2()
        .rounded(px(4.))
        .bg(gpui::rgb(0x4e5058))
        .child(
            div()
                .text_sm()
                .text_color(gpui::rgb(0xaeaeae))
                .child(request_text),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap_3()
                .mt_2()
                .child(
                    div()
                        .id("profile-pending-accept")
                        .px_2()
                        .py_1()
                        .rounded(px(4.))
                        .bg(gpui::rgb(0x5265ec))
                        .text_color(gpui::white())
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .child(mezon_i18n::t(locale, "userProfile.accept"))
                        .on_click(move |_: &ClickEvent, _window, cx| {
                            FriendStore::global(cx)
                                .update(cx, |store, cx| store.accept_friend(user_id, cx));
                        }),
                )
                .child(
                    div()
                        .id("profile-pending-ignore")
                        .px_2()
                        .py_1()
                        .rounded(px(4.))
                        .bg(gpui::rgb(0x4e5058))
                        .text_color(gpui::white())
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .child(mezon_i18n::t(&locale_owned, "userProfile.ignore"))
                        .on_click(move |_: &ClickEvent, window, cx| {
                            Shell::global(cx).update(cx, |shell, cx| {
                                shell.confirm_remove_friend(
                                    user_id,
                                    &username_owned,
                                    FriendRemovalKind::RejectRequest,
                                    &locale_owned,
                                    window,
                                    cx,
                                );
                            });
                        }),
                ),
        )
        .into_any_element()
}

fn render_voice_button(
    clan_id: mezon_store::ClanId,
    channel_id: mezon_store::ChannelId,
    theme: &Theme,
    _cx: &App,
) -> AnyElement {
    div()
        .id("profile-in-voice")
        .flex()
        .flex_row()
        .items_center()
        .h(px(32.))
        .max_w(px(120.))
        .px_2()
        .rounded_full()
        .bg(gpui::rgb(0x272120))
        .hover(|s| s.bg(gpui::rgb(0x1e1a19)))
        .cursor_pointer()
        .on_click(move |_: &ClickEvent, _window, cx| {
            navigate(
                cx,
                Route::Channel {
                    clan_id,
                    channel_id,
                },
            );
        })
        .child(
            Icon::new(IconName::Speaker)
                .size(px(14.))
                .text_color(theme.status_online),
        )
        .into_any_element()
}

fn render_avatar_row(
    avatar: Avatar,
    status_icon: IconName,
    status_color: gpui::Rgba,
    custom_status: String,
    theme: &Theme,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(6.))
        .px(px(16.))
        .mt(px(-50.))
        .child(
            div()
                .relative()
                .flex_shrink_0()
                .child(
                    div()
                        .border(px(AVATAR_BORDER))
                        .border_color(theme.bg_floating)
                        .rounded_full()
                        .child(avatar),
                )
                .child(
                    div().absolute().bottom(px(4.)).right(px(8.)).child(
                        Icon::new(status_icon)
                            .size(px(16.))
                            .text_color(status_color),
                    ),
                ),
        )
        .when(!custom_status.is_empty(), |row| {
            row.child(
                div().flex_1().mt(px(30.)).min_w_0().child(
                    div()
                        .px_4()
                        .py_3()
                        .rounded_xl()
                        .bg(theme.bg_floating)
                        .shadow_md()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.tokens.text_theme_primary)
                        .overflow_hidden()
                        .child(custom_status),
                ),
            )
        })
        .into_any_element()
}

fn section_divider(color: gpui::Rgba) -> gpui::AnyElement {
    div()
        .w_full()
        .py_1()
        .border_b_1()
        .border_color(color)
        .opacity(0.7)
        .into_any_element()
}

fn section_label(text: impl Into<SharedString>, color: gpui::Rgba) -> gpui::AnyElement {
    div()
        .mt_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text.into().to_uppercase())
        .into_any_element()
}

pub(crate) fn format_member_since(create_time_seconds: u32) -> String {
    if create_time_seconds == 0 {
        return String::new();
    }
    let secs = create_time_seconds as i64;
    let days_since_epoch = secs / 86400;
    let (year, month, day) = days_to_date(days_since_epoch as u32);
    let month_name = MONTH_NAMES
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("Jan");
    format!("{month_name} {day:02}, {year}")
}

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn days_to_date(days_since_epoch: u32) -> (u32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn chip_color(color: &str) -> gpui::Rgba {
    parse_role_color(color)
        .unwrap_or_else(|| gpui::rgb(crate::chat::role_style::ROLE_FALLBACK_COLOR))
}

fn role_expander_pill(
    id: &'static str,
    label: impl Into<SharedString>,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_x_1()
        .rounded(px(4.))
        .p_1()
        .cursor_pointer()
        .bg(theme.surfaces.input_primary)
        .text_color(theme.tokens.text_theme_primary)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .px_1()
                .line_height(px(15.))
                .child(label.into()),
        )
        .on_click(on_click)
}

fn parse_role_color(s: &str) -> Option<gpui::Rgba> {
    let trimmed = s.trim();
    if !trimmed.starts_with('#') {
        return None;
    }
    mezon_store::parse_role_color(trimmed)
}

pub(crate) struct ClickableContainer(Stateful<Div>);

impl ClickableContainer {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self(div().id(id))
    }
}

impl ParentElement for ClickableContainer {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.0.extend(elements);
    }
}

impl Styled for ClickableContainer {
    fn style(&mut self) -> &mut StyleRefinement {
        self.0.style()
    }
}

impl IntoElement for ClickableContainer {
    type Element = Stateful<Div>;

    fn into_element(self) -> Stateful<Div> {
        self.0
    }
}

impl Clickable for ClickableContainer {
    fn on_click(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        Self(self.0.on_click(handler))
    }

    fn cursor_style(self, cursor_style: CursorStyle) -> Self {
        Self(self.0.cursor(cursor_style))
    }
}

impl Toggleable for ClickableContainer {
    fn toggle_state(self, _selected: bool) -> Self {
        self
    }
}

pub(crate) fn profile_popover_menu(
    id: impl Into<ElementId>,
    user_id: UserId,
    context: ProfileContext,
    settings: gpui::Entity<Settings>,
    avatar_image_cache: gpui::Entity<LruImageCache>,
) -> PopoverMenu<UserProfilePopover> {
    PopoverMenu::new(id)
        .menu(move |window, cx| {
            Some(cx.new(|cx| {
                UserProfilePopover::new(
                    user_id,
                    context,
                    settings.clone(),
                    avatar_image_cache.clone(),
                    window,
                    cx,
                )
            }))
        })
        .anchor(Anchor::TopRight)
        .attach(Anchor::TopLeft)
}
