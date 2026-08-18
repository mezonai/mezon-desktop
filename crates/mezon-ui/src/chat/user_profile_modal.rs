use crate::app::shell::{FriendRemovalKind, Shell};
use crate::chat::message::{SendTokenModal, ShareContactModal, share_contact_subject};
use crate::chat::user_profile_popover::{
    banner_icon_button, banner_icon_shell, format_member_since, share_contact_icon,
};
use crate::components::compositions::CustomStatusBubble;
use crate::components::primitives::{Avatar, Icon, IconName};
use crate::image_cache::LruImageCache;
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;
use crate::util::avatar_color::spawn_banner_color_task;
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton,
    MouseDownEvent, Render, Rgba, Subscription, Task, Window, deferred, div, prelude::*, px,
};
use mezon_store::{
    AccountStore, BadgeService, ClanId, ClanMembersStore, FriendState, FriendStore, PresenceStore,
    ProfileContext, Settings, UserId,
};
use ui::Tooltip;

pub struct UserProfileModal {
    focus_handle: FocusHandle,
    user_id: UserId,
    clan_id: ClanId,
    settings: Entity<Settings>,
    avatar_image_cache: Entity<LruImageCache>,
    banner_color: Option<Rgba>,
    banner_source: String,
    is_self: bool,
    edit_options_open: bool,
    live_status: String,
    live_custom_status: String,
    custom_status_bubble: Entity<CustomStatusBubble>,
    _banner_task: Option<Task<()>>,
    _members_sub: Subscription,
    _presence_sub: Subscription,
    _friend_sub: Subscription,
    _account_sub: Subscription,
}

impl UserProfileModal {
    pub fn new(
        user_id: UserId,
        clan_id: ClanId,
        settings: Entity<Settings>,
        avatar_image_cache: Entity<LruImageCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let account = AccountStore::global(cx).read(cx).account.clone();
        let is_self = Self::resolve_is_self(user_id, clan_id, cx);
        let presence = PresenceStore::global(cx);
        let presence = presence.read(cx);
        let presence_status_snapshot = presence
            .presence_status(user_id)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if presence.is_online(user_id) {
                    "Online".to_string()
                } else {
                    "Invisible".to_string()
                }
            });
        let (live_status, live_custom_status) = if is_self {
            account
                .as_ref()
                .map(|account| (account.status.clone(), account.user_status.clone()))
                .unwrap_or_default()
        } else {
            (
                presence_status_snapshot.clone(),
                presence.user_status(user_id).unwrap_or("").to_string(),
            )
        };
        // The member row often arrives after the modal opens, so identity, status and
        // the banner source all have to be re-derived when the stores fill in.
        let members_sub = cx.observe(&ClanMembersStore::global(cx), |this, _, cx| {
            this.sync_identity(cx);
            this.refresh_banner_source(cx);
            cx.notify();
        });
        let presence_sub = cx.observe(&PresenceStore::global(cx), |this, _, cx| {
            if this.sync_presence_status(cx) {
                cx.notify();
            }
        });
        let friend_sub = cx.observe(&FriendStore::global(cx), |_, _, cx| cx.notify());
        let account_sub = cx.observe(&AccountStore::global(cx), |this, _, cx| {
            let identity_changed = this.sync_identity(cx);
            let status_changed = this.sync_account_status(cx);
            if identity_changed || status_changed {
                cx.notify();
            }
        });
        let source_avatar = Self::banner_source_for(user_id, clan_id, cx);

        let mut modal = Self {
            focus_handle: cx.focus_handle(),
            user_id,
            clan_id,
            settings,
            avatar_image_cache,
            banner_color: None,
            banner_source: source_avatar.clone(),
            is_self,
            edit_options_open: false,
            live_status,
            live_custom_status,
            custom_status_bubble: cx.new(|_| CustomStatusBubble::new()),
            _banner_task: None,
            _members_sub: members_sub,
            _presence_sub: presence_sub,
            _friend_sub: friend_sub,
            _account_sub: account_sub,
        };
        modal.load_banner_color(source_avatar, cx);
        modal
    }

    fn resolve_is_self(user_id: UserId, clan_id: ClanId, cx: &App) -> bool {
        let member_username = ClanMembersStore::global(cx)
            .read(cx)
            .member(clan_id, user_id)
            .map(|member| member.user.username.clone())
            .unwrap_or_default();
        BadgeService::global(cx)
            .read(cx)
            .current_user_id(cx)
            .is_some_and(|id| id == user_id)
            || AccountStore::global(cx)
                .read(cx)
                .account
                .as_ref()
                .is_some_and(|account| {
                    !member_username.is_empty()
                        && !account.username.is_empty()
                        && account.username == member_username
                })
    }

    /// Both username sides can still be empty on open, so `is_self` has to be
    /// re-derived rather than latched — otherwise a late account/member row leaves
    /// the viewer looking at Add-Friend and Transfer buttons on their own profile.
    fn sync_identity(&mut self, cx: &App) -> bool {
        let is_self = Self::resolve_is_self(self.user_id, self.clan_id, cx);
        if self.is_self == is_self {
            return false;
        }
        self.is_self = is_self;
        // The status source swaps with is_self, so pull the value that now applies.
        self.sync_account_status(cx);
        self.sync_presence_status(cx);
        true
    }

    fn banner_source_for(user_id: UserId, clan_id: ClanId, cx: &App) -> String {
        // Must match what the rendered Avatar resolves to, otherwise the banner
        // polls a resource the cache was never asked to load.
        let raw = ClanMembersStore::global(cx)
            .read(cx)
            .member(clan_id, user_id)
            .map(|member| member.avatar().to_string())
            .unwrap_or_default();
        crate::util::imgproxy::avatar_url(cx, &raw)
    }

    fn refresh_banner_source(&mut self, cx: &mut Context<Self>) {
        let source = Self::banner_source_for(self.user_id, self.clan_id, cx);
        if source == self.banner_source {
            return;
        }
        self.banner_source = source.clone();
        self.banner_color = None;
        self.load_banner_color(source, cx);
    }

    fn sync_account_status(&mut self, cx: &App) -> bool {
        if !self.is_self {
            return false;
        }
        let Some(account) = AccountStore::global(cx).read(cx).account.as_ref() else {
            return false;
        };
        let changed =
            self.live_status != account.status || self.live_custom_status != account.user_status;
        if changed {
            self.live_status = account.status.clone();
            self.live_custom_status = account.user_status.clone();
        }
        changed
    }

    fn sync_presence_status(&mut self, cx: &App) -> bool {
        if self.is_self {
            return false;
        }
        let presence = PresenceStore::global(cx);
        let presence = presence.read(cx);
        let status = presence.presence_status(self.user_id).unwrap_or_else(|| {
            if presence.is_online(self.user_id) {
                "Online"
            } else {
                "Invisible"
            }
        });
        let custom_status = presence.user_status(self.user_id).unwrap_or("");
        let changed =
            self.live_status != status || self.live_custom_status.as_str() != custom_status;
        if changed {
            self.live_status = status.to_string();
            self.live_custom_status = custom_status.to_string();
        }
        changed
    }

    fn load_banner_color(&mut self, avatar_url: String, cx: &mut Context<Self>) {
        if avatar_url.is_empty() {
            return;
        }
        self._banner_task = spawn_banner_color_task(
            self.avatar_image_cache.clone(),
            avatar_url,
            cx,
            |this, color, cx| {
                this.banner_color = Some(color);
                cx.notify();
            },
        );
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Focusable for UserProfileModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for UserProfileModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let member = ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, self.user_id)
            .cloned();
        let (display_name, username, raw_avatar, about_me, created_at) = member
            .as_ref()
            .map(|member| {
                (
                    member.name().to_string(),
                    member.user.username.clone(),
                    member.avatar().to_string(),
                    member.user.about_me.clone(),
                    member.user.create_time_seconds,
                )
            })
            .unwrap_or_default();
        let avatar = crate::util::imgproxy::avatar_url(cx, &raw_avatar);
        let is_self = self.is_self;
        let custom_status = self.live_custom_status.clone();
        let custom_status_bubble = self.custom_status_bubble.clone();
        custom_status_bubble.update(cx, |bubble, cx| {
            bubble.set_content(
                custom_status.clone().into(),
                px(250.),
                theme.tokens.bg_secondary,
                theme.border,
                theme.text_secondary,
                cx,
            );
        });
        let (status_icon, status_color) = profile_status(&self.live_status, &theme);
        let friend_state = FriendStore::global(cx)
            .read(cx)
            .friend(self.user_id)
            .map(|friend| friend.state);

        let member_since = format_member_since(created_at);
        let mut avatar_view = Avatar::new()
            .name(display_name.clone())
            .size_px(px(96.))
            .image_cache(self.avatar_image_cache.clone());
        if !avatar.is_empty() {
            avatar_view = avatar_view.src(avatar.clone());
        }

        let banner_color = self
            .banner_color
            .map(gpui::Hsla::from)
            .unwrap_or(theme.tokens.bg_secondary.into());

        div()
            .id("full-user-profile-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::hsla(0., 0., 0., 0.8))
            .track_focus(&self.focus_handle)
            .key_context("modal_backdrop")
            .on_action(|_: &::menu::Cancel, _window, cx| Self::close(cx))
            .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, cx| {
                Self::close(cx);
            })
            .child(
                div()
                    .id("full-user-profile-card")
                    .occlude()
                    .relative()
                    .w(px(600.))
                    .h(px(640.))
                    .max_h_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg_floating)
                    .shadow_lg()
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(180.))
                            .rounded_tl_lg()
                            .rounded_tr_lg()
                            .bg(banner_color)
                            .child(render_profile_actions(
                                is_self,
                                friend_state,
                                self.user_id,
                                self.clan_id,
                                &username,
                                &display_name,
                                &raw_avatar,
                                &locale,
                            )),
                    )
                    .child(
                        div()
                            .h(px(460.))
                            .pt(px(72.))
                            .px(px(20.))
                            .pb(px(16.))
                            .rounded_bl_lg()
                            .rounded_br_lg()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .bg(theme.surfaces.primary)
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .w_full()
                                            .truncate()
                                            .text_size(px(24.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_secondary)
                                            .child(display_name),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .truncate()
                                            .text_sm()
                                            .text_color(theme.text_secondary)
                                            .child(username),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .rounded_lg()
                                    .bg(theme.surfaces.secondary)
                                    .shadow_sm()
                                    .p_4()
                                    .child(
                                        div()
                                            .pb_2()
                                            .border_b_1()
                                            .border_color(theme.border)
                                            .text_sm()
                                            .text_color(theme.text_secondary)
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "userProfile.labels.aboutMe",
                                            )),
                                    )
                                    .when(!about_me.is_empty(), |content| {
                                        content.child(
                                            div()
                                                .mt_4()
                                                .text_sm()
                                                .text_color(theme.text_secondary)
                                                .child(about_me),
                                        )
                                    })
                                    .child(
                                        div()
                                            .mt_4()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_secondary)
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "userProfile.labels.memberSince",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .mt_2()
                                            .text_sm()
                                            .text_color(theme.text_secondary)
                                            .child(member_since),
                                    ),
                            ),
                    )
                    .child(deferred(
                        div()
                            .absolute()
                            .left(px(20.))
                            .top(px(132.))
                            .size(px(108.))
                            .rounded_full()
                            .bg(theme.bg_floating)
                            .p(px(6.))
                            .child(avatar_view)
                            .child(
                                div()
                                    .absolute()
                                    .right(px(5.))
                                    .bottom(px(5.))
                                    .p(px(2.))
                                    .rounded_full()
                                    .bg(theme.bg_floating)
                                    .child(
                                        Icon::new(status_icon)
                                            .size(px(19.))
                                            .text_color(status_color),
                                    ),
                            )
                            .when(!custom_status.is_empty(), |avatar| {
                                avatar.child(
                                    div()
                                        .absolute()
                                        .right(px(-20.))
                                        .top(px(25.))
                                        .size(px(14.))
                                        .rounded_full()
                                        .bg(theme.surfaces.secondary)
                                        .border_1()
                                        .border_color(theme.bg_floating),
                                )
                            }),
                    ))
                    .when(!custom_status.is_empty(), |card| {
                        card.child(deferred(
                            div()
                                .id("full-profile-custom-status")
                                .group("full-profile-custom-status")
                                .absolute()
                                .left(px(134.))
                                .top(px(194.))
                                .max_w(px(250.))
                                .on_hover({
                                    let custom_status_bubble = custom_status_bubble.clone();
                                    move |hovered: &bool, _window, cx| {
                                        custom_status_bubble.update(cx, |bubble, cx| {
                                            bubble.set_expanded(*hovered, cx);
                                        });
                                    }
                                })
                                .child(custom_status_bubble),
                        ))
                    })
                    .when(is_self, |card| {
                        card.child(deferred(
                            div()
                                .id("full-profile-edit")
                                .absolute()
                                .right(px(18.))
                                .top(px(200.))
                                .h(px(34.))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_1()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|style| style.bg(theme.bg_hover))
                                .child(
                                    Icon::new(IconName::PenEdit)
                                        .size(px(16.))
                                        .text_color(theme.text_secondary),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child(mezon_i18n::t(
                                            &locale,
                                            "userProfile.labels.editProfile",
                                        )),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.edit_options_open = !this.edit_options_open;
                                    cx.notify();
                                })),
                        ))
                        .when(self.edit_options_open, |card| {
                            let clan_id = self.clan_id;
                            card.child(deferred(
                                div()
                                    .id("full-profile-edit-options")
                                    .occlude()
                                    .absolute()
                                    .right(px(-192.))
                                    .top(px(180.))
                                    .w(px(180.))
                                    .p_2()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.surfaces.secondary)
                                    .shadow_lg()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        div()
                                            .id("edit-clan-profile-option")
                                            .cursor_pointer()
                                            .px_3()
                                            .py_2()
                                            .rounded_sm()
                                            .text_sm()
                                            .text_color(theme.text_secondary)
                                            .hover(|style| style.bg(theme.bg_hover))
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "common.userProfile.editClanProfile",
                                            ))
                                            .on_click(move |_, _, cx| {
                                                Self::close(cx);
                                                navigate(
                                                    cx,
                                                    Route::SettingsClanProfile { clan_id },
                                                );
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("edit-main-profile-option")
                                            .cursor_pointer()
                                            .px_3()
                                            .py_2()
                                            .rounded_sm()
                                            .text_sm()
                                            .text_color(theme.text_secondary)
                                            .hover(|style| style.bg(theme.bg_hover))
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "common.userProfile.editMainProfile",
                                            ))
                                            .on_click(|_, _, cx| {
                                                Self::close(cx);
                                                navigate(cx, Route::SettingsProfile);
                                            }),
                                    ),
                            ))
                        })
                    }),
            )
    }
}

fn render_profile_actions(
    is_self: bool,
    friend_state: Option<FriendState>,
    user_id: UserId,
    clan_id: ClanId,
    username: &str,
    display_name: &str,
    avatar: &str,
    locale: &str,
) -> AnyElement {
    if is_self {
        return div().into_any_element();
    }

    let transfer_username = username.to_string();
    let transfer_locale = locale.to_string();
    let mut actions = div()
        .absolute()
        .top(px(10.))
        .right(px(10.))
        .flex()
        .items_center()
        .gap_2()
        .child(profile_action_button(
            "full-profile-transfer",
            IconName::Transaction,
            mezon_i18n::t(locale, "common.transfer"),
            move |_, window, cx| {
                UserProfileModal::close(cx);
                SendTokenModal::open(
                    transfer_locale.clone().into(),
                    Some((user_id.0.to_string(), transfer_username.clone())),
                    window,
                    cx,
                );
            },
        ));

    if friend_state == Some(FriendState::Friend) {
        actions = actions.child(profile_share_contact_button(
            "full-profile-share-contact",
            mezon_i18n::t(locale, "common.shareContact"),
            user_id,
            clan_id,
            display_name,
            locale,
        ));
    }

    if friend_state == Some(FriendState::InviteReceived) {
        let ignore_username = username.to_string();
        let ignore_locale = locale.to_string();
        return actions
            .child(profile_action_button(
                "full-profile-accept-friend",
                IconName::IConAcceptFriend,
                mezon_i18n::t(locale, "common.accept"),
                move |_, _, cx| {
                    FriendStore::global(cx)
                        .update(cx, |store, cx| store.accept_friend(user_id, cx));
                },
            ))
            .child(profile_action_button(
                "full-profile-ignore-friend",
                IconName::IConIgnoreFriend,
                mezon_i18n::t(locale, "common.ignore"),
                move |_, window, cx| {
                    UserProfileModal::close(cx);
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.confirm_remove_friend(
                            user_id,
                            &ignore_username,
                            FriendRemovalKind::RejectRequest,
                            &ignore_locale,
                            window,
                            cx,
                        );
                    });
                },
            ))
            .into_any_element();
    }

    let friend_icon = match friend_state {
        Some(FriendState::Friend) => IconName::IconFriend,
        Some(FriendState::InviteSent) => IconName::PendingFriend,
        Some(FriendState::Blocked) => return actions.into_any_element(),
        None => IconName::AddPerson,
        Some(FriendState::InviteReceived) => IconName::IConAcceptFriend,
    };
    let action_username = username.to_string();
    let action_display_name = display_name.to_string();
    let action_avatar = avatar.to_string();
    let action_locale = locale.to_string();
    actions
        .child(profile_action_button(
            "full-profile-friend-state",
            friend_icon,
            match friend_state {
                Some(FriendState::Friend) => mezon_i18n::t(locale, "common.friend"),
                Some(FriendState::InviteSent) => mezon_i18n::t(locale, "common.pending"),
                Some(FriendState::InviteReceived) => mezon_i18n::t(locale, "common.accept"),
                _ => mezon_i18n::t(locale, "common.addFriend"),
            },
            move |_, window, cx| match friend_state {
                Some(FriendState::Friend) => {
                    UserProfileModal::close(cx);
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.confirm_remove_friend(
                            user_id,
                            &action_username,
                            FriendRemovalKind::RemoveFriend,
                            &action_locale,
                            window,
                            cx,
                        );
                    });
                }
                Some(FriendState::InviteSent) => {
                    UserProfileModal::close(cx);
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.confirm_remove_friend(
                            user_id,
                            &action_username,
                            FriendRemovalKind::CancelRequest,
                            &action_locale,
                            window,
                            cx,
                        );
                    });
                }
                None => {
                    FriendStore::global(cx).update(cx, |store, cx| {
                        store.add_friend(
                            user_id,
                            action_username.clone(),
                            action_display_name.clone(),
                            action_avatar.clone(),
                            cx,
                        );
                    });
                }
                _ => {}
            },
        ))
        .into_any_element()
}

fn profile_action_button(
    id: &'static str,
    icon: IconName,
    tooltip: impl Into<gpui::SharedString> + 'static,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(format!("{id}-tooltip"))
        .tooltip(Tooltip::text(tooltip))
        .child(banner_icon_button(id, icon, false, true, on_click))
        .into_any_element()
}

fn profile_share_contact_button(
    id: &'static str,
    tooltip: impl Into<gpui::SharedString> + 'static,
    user_id: UserId,
    clan_id: ClanId,
    display_name: &str,
    locale: &str,
) -> AnyElement {
    let display_name = display_name.to_string();
    let locale = locale.to_string();
    div()
        .id(format!("{id}-tooltip"))
        .tooltip(Tooltip::text(tooltip))
        .child(banner_icon_shell(
            id,
            false,
            move |_, window, cx| {
                let contact = share_contact_subject(
                    user_id,
                    &display_name,
                    Some(ProfileContext::Clan(clan_id)),
                    cx,
                );
                UserProfileModal::close(cx);
                ShareContactModal::open(contact, locale.clone().into(), window, cx);
            },
            share_contact_icon(),
        ))
        .into_any_element()
}

use crate::util::user_status::status_icon_and_color as profile_status;
