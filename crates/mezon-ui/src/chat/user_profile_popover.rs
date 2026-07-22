use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, CursorStyle, DismissEvent, Div, ElementId,
    EventEmitter, FocusHandle, Focusable, FontWeight, MouseDownEvent, ParentElement, Render,
    SharedString, Stateful, StyleRefinement, Styled, Window, div, prelude::*, px, svg,
};
use mezon_store::{
    BadgeService, ChannelList, DirectMessageBody, DirectMessageStore, FriendState, FriendStore,
    PresenceStore, ProfileContext, RolesStore, Settings, UserId, resolve_user_profile,
};
use ui::{Clickable, PopoverMenu, Toggleable};

use crate::app::shell::{FriendRemovalKind, Shell};
use crate::chat::message::{ShareContactModal, share_contact_subject};
use crate::components::primitives::{Avatar, Icon, IconName, Input, InputEvent, InputState};
use crate::image_cache::LruImageCache;
use crate::router::{Route, navigate};
use crate::theme::{ActiveTheme, Theme};

const BANNER_HEIGHT: f32 = 105.;
const AVATAR_SIZE: f32 = 90.;
const AVATAR_BORDER: f32 = 6.;

pub struct UserProfilePopover {
    focus_handle: FocusHandle,
    user_id: UserId,
    context: ProfileContext,
    settings: gpui::Entity<Settings>,
    avatar_image_cache: gpui::Entity<LruImageCache>,
    message_input: gpui::Entity<InputState>,
    friend_menu_open: bool,
    sending_message: bool,
    _roles_sub: Option<gpui::Subscription>,
    _friend_sub: gpui::Subscription,
    _presence_sub: gpui::Subscription,
    _channel_sub: Option<gpui::Subscription>,
    _input_sub: gpui::Subscription,
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

        let roles_sub = RolesStore::try_global(cx)
            .map(|roles_store| cx.observe(&roles_store, |_, _, cx| cx.notify()));
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
            friend_menu_open: false,
            sending_message: false,
            _roles_sub: roles_sub,
            _friend_sub: friend_sub,
            _presence_sub: presence_sub,
            _channel_sub: channel_sub,
            _input_sub: input_sub,
        }
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

        let (display_name, username, avatar_raw, about_me, create_time, online) = match &profile {
            Some(p) => (
                SharedString::from(p.display_name.as_str()),
                SharedString::from(p.username.as_str()),
                p.avatar_url.clone(),
                SharedString::from(p.about_me.as_str()),
                p.create_time_seconds,
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

        let custom_status = PresenceStore::global(cx)
            .read(cx)
            .user_status(self.user_id)
            .unwrap_or("")
            .to_string();

        let member_since = format_member_since(create_time);
        let status_icon = if online {
            IconName::OnlineStatus
        } else {
            IconName::OfflineStatus
        };

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
        let clan_id_opt = match self.context {
            ProfileContext::Clan(id) => Some(id),
            _ => None,
        };
        let role_ids = profile
            .as_ref()
            .map(|p| p.role_ids.as_slice())
            .unwrap_or_default();
        let roles: Vec<(SharedString, SharedString)> = clan_id_opt
            .zip(RolesStore::try_global(cx))
            .map(|(clan_id, rs)| {
                rs.read(cx)
                    .roles_for(clan_id, role_ids)
                    .into_iter()
                    .map(|r| {
                        (
                            SharedString::from(r.name.as_str()),
                            SharedString::from(r.color.as_str()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

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
            .child(render_avatar_row(avatar, status_icon, custom_status, theme))
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
                        .when(!is_dm && create_time > 0, |d| {
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
                        .when(is_clan && !roles.is_empty(), |d| {
                            d.child(section_divider(theme.tokens.theme_border_input))
                                .child(section_label(
                                    mezon_i18n::t(&locale, "userProfile.aboutMe.roles.headerTitle"),
                                    theme.tokens.text_theme_primary,
                                ))
                                .child(div().mt_1().flex().flex_wrap().gap_2().children(
                                    roles.iter().map(|(name, color)| {
                                        role_pill(name.clone(), color.as_ref(), theme.as_ref())
                                    }),
                                ))
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
                                    .on_click({
                                        let locale = locale.clone();
                                        move |_: &ClickEvent, _window, cx| {
                                            let msg = mezon_i18n::t(&locale, "common.comingSoon")
                                                .to_string();
                                            Shell::global(cx).update(cx, move |shell, cx| {
                                                shell.info(msg, cx);
                                            });
                                        }
                                    }),
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

fn share_contact_icon() -> gpui::AnyElement {
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

fn banner_icon_shell(
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

fn banner_icon_button(
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

    buttons.push(
        banner_icon_button("profile-transfer", IconName::Transaction, false, true, {
            let locale = locale.clone();
            move |_: &ClickEvent, _window, cx| {
                let msg = mezon_i18n::t(&locale, "common.comingSoon").to_string();
                Shell::global(cx).update(cx, move |shell, cx| shell.info(msg, cx));
            }
        })
        .into_any_element(),
    );

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
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .right(px(8.))
                        .child(Icon::new(status_icon).size(px(16.))),
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

fn format_member_since(create_time_seconds: u32) -> String {
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

fn role_pill(name: SharedString, color: &str, theme: &Theme) -> AnyElement {
    let dot_color = parse_role_color(color).unwrap_or(theme.text_muted);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(4.))
        .bg(theme.tokens.bg_active_member_channel)
        .child(div().size(px(8.)).rounded_full().bg(dot_color))
        .child(
            div()
                .text_xs()
                .text_color(theme.tokens.text_theme_primary)
                .child(name),
        )
        .into_any_element()
}

fn parse_role_color(s: &str) -> Option<gpui::Rgba> {
    let s = s.trim().strip_prefix('#')?;
    let (r, g, b) = match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            (r, g, b)
        }
        3 => {
            let r = u8::from_str_radix(&s[0..1], 16).ok()?;
            let g = u8::from_str_radix(&s[1..2], 16).ok()?;
            let b = u8::from_str_radix(&s[2..3], 16).ok()?;
            (r * 17, g * 17, b * 17)
        }
        _ => return None,
    };
    Some(gpui::Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    })
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
