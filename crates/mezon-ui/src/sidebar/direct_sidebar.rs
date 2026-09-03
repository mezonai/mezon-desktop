use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FontWeight, MouseButton, MouseDownEvent, Pixels, Point, SharedString,
    UniformListScrollHandle, WeakEntity, Window, div, img, prelude::*, px, size, uniform_list,
};
use mezon_store::{
    ChannelEvent, ChannelId, ChannelList, ClanId, DirectChannel, DirectKind, DirectMessageStore,
    DmAvatarPresence, FriendState, FriendStore, NotificationSettingStore, PresenceEvent,
    PresenceStore, Settings, UserId,
};

use super::channel_sidebar::menu::{MUTE_DURATIONS, apply_mute, mute_label, submenu_options};
use super::create_message_group_modal::CreateMessageGroupModal;
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::app::shell::{FriendRemovalKind, Shell};
use crate::chat::add_members_to_group_modal::AddMembersToGroupModal;
use crate::chat::edit_group_modal::EditGroupModal;
use crate::chat::user_profile_modal::UserProfileModal;
use crate::command_palette::CommandPaletteModal;
use crate::components::compositions::{DM_ROW_HEIGHT, DmRow};
use crate::components::primitives::{ContextMenu, Icon, IconName, context_menu_at};
use crate::router::{Route, Router, navigate};
use crate::theme::{ActiveTheme, Theme};

const PINNED_LIST_MAX_HEIGHT: f32 = 215.;

fn pinned_list_height(pinned_count: usize) -> Pixels {
    let rows = pinned_count as f32 * DM_ROW_HEIGHT;
    px(rows.min(PINNED_LIST_MAX_HEIGHT))
}

struct DmMenu {
    position: Point<Pixels>,
    channel_id: ChannelId,
    mute_sub_open: bool,
    target: Option<DmMenuTarget>,
}

#[derive(PartialEq)]
struct DmItem {
    channel_id: ChannelId,
    id: SharedString,
    row_id: SharedString,
    group_name: SharedString,
    close_id: SharedString,
    label: SharedString,
    kind: DirectKind,
    unread: bool,
    presence_badge: DmAvatarPresence,
    in_voice: bool,
    muted: bool,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    pinned: bool,
}

struct DmRowCache {
    generation: u64,
    id: SharedString,
    row_id: SharedString,
    group_name: SharedString,
    close_id: SharedString,
    label: SharedString,
    avatar_raw: SharedString,
    avatar_src: SharedString,
}

#[derive(Default)]
struct DmRowCaches {
    entries: HashMap<ChannelId, DmRowCache>,
    generation: u64,
}

impl DmRowCaches {
    fn begin(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn entry(&mut self, ch: &DirectChannel, cx: &App) -> &DmRowCache {
        let generation = self.generation;
        let cached = self.entries.entry(ch.id).or_insert_with(|| DmRowCache {
            generation,
            id: SharedString::from(ch.id.to_string()),
            row_id: SharedString::from(format!("dm-{}", ch.id)),
            group_name: SharedString::from(format!("dm-row-{}", ch.id)),
            close_id: SharedString::from(format!("dm-close-{}", ch.id)),
            label: SharedString::from(ch.label.clone()),
            avatar_raw: SharedString::from(ch.avatar.clone()),
            avatar_src: SharedString::from(crate::util::imgproxy::avatar_url(cx, &ch.avatar)),
        });
        cached.generation = generation;
        if cached.label.as_ref() != ch.label {
            cached.label = SharedString::from(ch.label.clone());
        }
        if cached.avatar_raw.as_ref() != ch.avatar {
            cached.avatar_raw = SharedString::from(ch.avatar.clone());
            cached.avatar_src =
                SharedString::from(crate::util::imgproxy::avatar_url(cx, &ch.avatar));
        }
        cached
    }

    fn sweep(&mut self) {
        let generation = self.generation;
        self.entries
            .retain(|_, entry| entry.generation == generation);
    }
}

pub struct DirectSidebar {
    settings: Entity<Settings>,
    list_scroll: UniformListScrollHandle,
    row_caches: DmRowCaches,
    dm_items: Rc<Vec<DmItem>>,
    dm_items_fingerprint: u64,
    pending_rebuild: bool,
    pinned_scroll: gpui::ScrollHandle,
    open_menu: Option<DmMenu>,
    image_cache: Entity<crate::image_cache::LruImageCache>,
}

fn is_dm_route(cx: &App) -> bool {
    matches!(
        Router::global(cx).read(cx).route(),
        Route::Direct | Route::DirectMessage { .. } | Route::Friends
    )
}

fn dm_in_voice(ch: &DirectChannel, channels: &ChannelList) -> bool {
    ch.kind == DirectKind::Dm
        && ch
            .peer_user_id
            .is_some_and(|user_id| channels.in_voice_status(user_id).is_some())
}

fn dm_presence_badge(ch: &DirectChannel, presence: &PresenceStore) -> DmAvatarPresence {
    if ch.kind != DirectKind::Dm {
        return DmAvatarPresence::None;
    }
    ch.peer_user_id
        .map(|user_id| presence.dm_avatar_presence(user_id, ch.online))
        .unwrap_or(DmAvatarPresence::None)
}

fn dm_items_fingerprint(store: &DirectMessageStore, cx: &App) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    fn fold(hash: u64, bytes: &[u8]) -> u64 {
        bytes
            .iter()
            .fold(hash, |h, b| (h ^ u64::from(*b)).wrapping_mul(FNV_PRIME))
    }
    let channel_list = ChannelList::global(cx);
    let channels = channel_list.read(cx);
    let presence = PresenceStore::global(cx).read(cx);
    let notifications = NotificationSettingStore::try_global(cx);
    let notifications = notifications.as_ref().map(|store| store.read(cx));
    store.channels().iter().fold(FNV_OFFSET, |h, ch| {
        let h = fold(h, &ch.id.0.to_le_bytes());
        let h = fold(
            h,
            &[
                ch.kind as u8,
                u8::from(ch.is_unread()),
                dm_presence_badge(ch, presence) as u8,
                u8::from(dm_in_voice(ch, channels)),
                u8::from(notifications.is_some_and(|store| store.is_time_muted(ch.id))),
                u8::from(store.is_pinned(ch.id)),
            ],
        );
        let h = fold(h, ch.label.as_bytes());
        fold(h, ch.avatar.as_bytes())
    })
}

fn request_dm_close(channel_id: ChannelId, window: &mut Window, cx: &mut App) {
    let Some(store) = DirectMessageStore::try_global(cx) else {
        return;
    };
    let Some((is_group, group_name)) = store.read(cx).find(channel_id).map(|dm| {
        (
            dm.kind == DirectKind::Group,
            SharedString::from(dm.label.clone()),
        )
    }) else {
        return;
    };
    let locale = Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_default();
    Shell::global(cx).update(cx, |shell, cx| {
        if is_group {
            shell.confirm_leave_dm_group(channel_id, &group_name, &locale, window, cx);
        } else {
            shell.confirm_close_dm(channel_id, &locale, window, cx);
        }
    });
}

fn render_dm_row(
    item: &DmItem,
    theme: &Theme,
    selected: bool,
    suppress_hover: bool,
    image_cache: &Entity<crate::image_cache::LruImageCache>,
    in_voice_label: &SharedString,
    sidebar: WeakEntity<DirectSidebar>,
) -> gpui::AnyElement {
    let mut row = DmRow::with_ids(
        item.id.clone(),
        item.label.clone(),
        item.kind,
        item.row_id.clone().into(),
        item.group_name.clone(),
        item.close_id.clone(),
    )
    .selected(selected)
    .unread(item.unread)
    .presence_badge(item.presence_badge)
    .avatar_src(item.avatar_src.clone())
    .avatar_raw(item.avatar_raw.clone())
    .suppress_hover(suppress_hover)
    .image_cache(image_cache.clone())
    .on_close(item.channel_id, request_dm_close);
    if item.in_voice {
        row = row.in_voice_label(in_voice_label.clone());
    }
    let channel_id = item.channel_id;
    div()
        .w_full()
        .when(item.muted, |d| d.opacity(0.7))
        .on_mouse_down(
            MouseButton::Right,
            move |event: &MouseDownEvent, _window, cx| {
                let position = event.position;
                if let Some(view) = sidebar.upgrade() {
                    view.update(cx, |this, cx| {
                        this.open_menu = Some(DmMenu {
                            position,
                            channel_id,
                            mute_sub_open: false,
                            target: dm_menu_target(channel_id, cx),
                        });
                        if let Some(store) = NotificationSettingStore::try_global(cx) {
                            store.update(cx, |store, cx| {
                                store.ensure_channel(ClanId(0), channel_id, cx);
                            });
                        }
                        cx.notify();
                    });
                }
            },
        )
        .child(row.render(theme))
        .into_any_element()
}

fn build_dm_items(
    caches: &mut DmRowCaches,
    store: &DirectMessageStore,
    cx: &App,
) -> Rc<Vec<DmItem>> {
    let channel_list = ChannelList::global(cx);
    let channels = channel_list.read(cx);
    let presence = PresenceStore::global(cx).read(cx);
    let notifications = NotificationSettingStore::try_global(cx);
    let notifications = notifications.as_ref().map(|store| store.read(cx));
    let all = store.channels();
    let mut ordered: Vec<&DirectChannel> = Vec::with_capacity(all.len());
    ordered.extend(all.iter().filter(|ch| store.is_pinned(ch.id)));
    ordered.extend(all.iter().filter(|ch| !store.is_pinned(ch.id)));

    caches.begin();
    let items = ordered
        .into_iter()
        .map(|ch| {
            let pinned = store.is_pinned(ch.id);
            let unread = ch.is_unread();
            let presence_badge = dm_presence_badge(ch, presence);
            let in_voice = dm_in_voice(ch, channels);
            let muted = notifications.is_some_and(|store| store.is_time_muted(ch.id));
            let cached = caches.entry(ch, cx);
            DmItem {
                channel_id: ch.id,
                id: cached.id.clone(),
                row_id: cached.row_id.clone(),
                group_name: cached.group_name.clone(),
                close_id: cached.close_id.clone(),
                label: cached.label.clone(),
                kind: ch.kind,
                unread,
                presence_badge,
                in_voice,
                muted,
                avatar_src: cached.avatar_src.clone(),
                avatar_raw: cached.avatar_raw.clone(),
                pinned,
            }
        })
        .collect();
    caches.sweep();
    Rc::new(items)
}

fn dm_set_mute_submenu_open(
    sidebar: WeakEntity<DirectSidebar>,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let _ = sidebar.update(cx, |this, cx| {
            if let Some(menu) = this.open_menu.as_mut()
                && !menu.mute_sub_open
            {
                menu.mute_sub_open = true;
                cx.notify();
            }
        });
    }
}

fn dm_close_submenus(
    sidebar: WeakEntity<DirectSidebar>,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let _ = sidebar.update(cx, |this, cx| {
            if let Some(menu) = this.open_menu.as_mut()
                && menu.mute_sub_open
            {
                menu.mute_sub_open = false;
                cx.notify();
            }
        });
    }
}

#[derive(Clone)]
struct DmMenuTarget {
    is_group: bool,
    peer: Option<UserId>,
    peer_username: String,
    label: String,
    avatar: String,
    pinned: bool,
    friend_state: Option<FriendState>,
}

fn dm_menu_target(channel_id: ChannelId, cx: &App) -> Option<DmMenuTarget> {
    let store = DirectMessageStore::try_global(cx)?;
    let store = store.read(cx);
    let channel = store.find(channel_id)?;
    let peer = channel.peer_user_id;
    let friend_state = peer.and_then(|user| {
        FriendStore::try_global(cx)
            .and_then(|friends| friends.read(cx).friend(user).map(|friend| friend.state))
    });
    Some(DmMenuTarget {
        is_group: channel.kind == DirectKind::Group,
        peer,
        peer_username: channel.peer_username.clone(),
        label: channel.label.clone(),
        avatar: channel.avatar.clone(),
        pinned: store.is_pinned(channel_id),
        friend_state,
    })
}

fn build_dm_menu(
    sidebar: WeakEntity<DirectSidebar>,
    settings: Entity<Settings>,
    locale: &str,
    channel_id: ChannelId,
    target: Option<DmMenuTarget>,
    muted: bool,
    muted_until: Option<String>,
    mute_sub_open: bool,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let sidebar_dismiss = sidebar.clone();
    let mut menu = ContextMenu::new()
        .on_submenu_close(dm_close_submenus(sidebar.clone()))
        .on_dismiss(move |_window, cx| {
            let _ = sidebar_dismiss.update(cx, |this, cx| {
                this.open_menu = None;
                cx.notify();
            });
        });

    let is_group = target.as_ref().is_some_and(|target| target.is_group);
    let peer = target.as_ref().and_then(|target| target.peer);
    let friend_state = target.as_ref().and_then(|target| target.friend_state);
    let blocked = friend_state == Some(FriendState::Blocked);

    if !is_group && let Some(user) = peer {
        let profile_settings = settings.clone();
        menu = menu.item(
            t("directMessage.contextMenu.profile"),
            move |_window: &mut Window, cx: &mut App| {
                let avatar_image_cache = crate::image_cache::shared_avatar_cache(cx);
                let settings = profile_settings.clone();
                let modal = cx.new(|cx| {
                    UserProfileModal::new_for_direct(
                        user,
                        channel_id,
                        settings,
                        avatar_image_cache,
                        cx,
                    )
                });
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.show_fullscreen_modal(modal.into(), cx)
                });
            },
        );
    }

    menu = menu.item(
        t("channelMenu.menu.watchMenu.markAsRead"),
        move |_window: &mut Window, cx: &mut App| {
            DirectMessageStore::global(cx).update(cx, |store, cx| {
                store.mark_as_read(channel_id, cx);
            });
        },
    );

    let pinned = target.as_ref().is_some_and(|target| target.pinned);
    let pin_label = if pinned {
        t("directMessage.contextMenu.unpinConversation")
    } else {
        t("directMessage.contextMenu.pinConversation")
    };
    let pin_limit_message = t("directMessage.contextMenu.pinLimitExceeded");
    menu = menu.item(pin_label, move |_window: &mut Window, cx: &mut App| {
        let store = DirectMessageStore::global(cx);
        let full = store.read(cx).pinned_is_full() && !store.read(cx).is_pinned(channel_id);
        if full {
            let message = pin_limit_message.clone();
            Shell::global(cx).update(cx, move |shell, cx| shell.error(message, cx));
            return;
        }
        store.update(cx, |store, cx| store.toggle_pin(channel_id, cx));
    });

    menu = menu.separator();
    if muted {
        let label = match muted_until {
            Some(until) => format!("{} · {until}", mute_label(locale, false, true)),
            None => mute_label(locale, false, true),
        };
        menu = menu.item(label, move |_window: &mut Window, cx: &mut App| {
            if let Some(store) = NotificationSettingStore::try_global(cx) {
                store.update(cx, |store, cx| store.unmute(channel_id, ClanId(0), cx));
            }
        });
    } else {
        menu = menu.submenu(
            mute_label(locale, false, false),
            None,
            submenu_options(locale, MUTE_DURATIONS, -2),
            mute_sub_open,
            dm_set_mute_submenu_open(sidebar),
            apply_mute(channel_id, ClanId(0)),
        );
    }

    if !is_group
        && let Some(user) = peer
        && let Some(target) = target.as_ref()
    {
        let username = target.peer_username.clone();
        let display = target.label.clone();
        let locale_owned = locale.to_string();

        if !blocked {
            match friend_state {
                None if !username.is_empty() => {
                    let add_username = username.clone();
                    menu = menu.item(
                        t("directMessage.contextMenu.addFriend"),
                        move |_window: &mut Window, cx: &mut App| {
                            FriendStore::global(cx).update(cx, |store, cx| {
                                store.add_friend_by_username(add_username.clone(), cx)
                            });
                        },
                    );
                }
                Some(FriendState::Friend) => {
                    let remove_username = username.clone();
                    let remove_display = display.clone();
                    let remove_locale = locale_owned.clone();
                    menu = menu.danger_item(
                        t("directMessage.contextMenu.removeFriend"),
                        move |window: &mut Window, cx: &mut App| {
                            let name = if remove_username.is_empty() {
                                remove_display.clone()
                            } else {
                                remove_username.clone()
                            };
                            let locale = remove_locale.clone();
                            Shell::global(cx).update(cx, |shell, cx| {
                                shell.confirm_remove_friend(
                                    user,
                                    &name,
                                    FriendRemovalKind::RemoveFriend,
                                    &locale,
                                    window,
                                    cx,
                                );
                            });
                        },
                    );
                }
                _ => {}
            }
        }

        if friend_state == Some(FriendState::Friend) || blocked {
            if blocked {
                menu = menu.item(
                    t("directMessage.contextMenu.unblock"),
                    move |_window: &mut Window, cx: &mut App| {
                        FriendStore::global(cx)
                            .update(cx, |store, cx| store.unblock_friend(user, cx));
                    },
                );
            } else {
                menu = menu.danger_item(
                    t("directMessage.contextMenu.block"),
                    move |_window: &mut Window, cx: &mut App| {
                        FriendStore::global(cx)
                            .update(cx, |store, cx| store.block_friend(user, cx));
                    },
                );
            }
        }
    }

    if is_group && let Some(target) = target.as_ref() {
        let add_locale = locale.to_string();
        menu = menu.separator().item(
            t("common.addMembers"),
            move |window: &mut Window, cx: &mut App| {
                AddMembersToGroupModal::open(channel_id, add_locale.clone(), window, cx);
            },
        );

        let group_label = target.label.clone();
        let group_avatar = target.avatar.clone();
        let group_locale = locale.to_string();
        menu = menu.item(
            t("directMessage.contextMenu.editGroup"),
            move |window: &mut Window, cx: &mut App| {
                let modal = cx.new(|cx| {
                    EditGroupModal::new(
                        channel_id,
                        group_label.clone(),
                        group_avatar.clone(),
                        group_locale.clone(),
                        window,
                        cx,
                    )
                });
                Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
            },
        );

        let leave_locale = locale.to_string();
        let leave_name = target.label.clone();
        menu = menu.danger_item(
            t("directMessage.contextMenu.leaveGroup"),
            move |window: &mut Window, cx: &mut App| {
                let locale = leave_locale.clone();
                let name = leave_name.clone();
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.confirm_leave_dm_group(channel_id, &name, &locale, window, cx);
                });
            },
        );
    }

    menu
}

impl DirectSidebar {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let direct_store = DirectMessageStore::global(cx);

        cx.observe(&direct_store, |this, _, cx| this.refresh_dm_items(cx))
            .detach();
        cx.subscribe(&ChannelList::global(cx), |this, _, event, cx| {
            if matches!(event, ChannelEvent::InVoiceChanged) {
                this.refresh_dm_items(cx);
            }
        })
        .detach();
        cx.subscribe(&PresenceStore::global(cx), |this, _, event, cx| {
            if matches!(event, PresenceEvent::StatusChanged) {
                this.refresh_dm_items(cx);
            }
        })
        .detach();
        cx.observe(&Router::global(cx), |this, _, cx| {
            if this.pending_rebuild && is_dm_route(cx) {
                this.pending_rebuild = false;
                let store = DirectMessageStore::global(cx);
                this.dm_items_fingerprint = dm_items_fingerprint(store.read(cx), cx);
                this.dm_items = build_dm_items(&mut this.row_caches, store.read(cx), cx);
            }
            cx.notify();
        })
        .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&FriendStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&NotificationSettingStore::global(cx), |this, _, cx| {
            this.refresh_dm_items(cx)
        })
        .detach();

        let dm_items_fingerprint = dm_items_fingerprint(direct_store.read(cx), cx);
        let mut row_caches = DmRowCaches::default();
        let dm_items = build_dm_items(&mut row_caches, direct_store.read(cx), cx);

        Self {
            settings,
            list_scroll: UniformListScrollHandle::new(),
            row_caches,
            dm_items,
            dm_items_fingerprint,
            pending_rebuild: false,
            pinned_scroll: gpui::ScrollHandle::new(),
            open_menu: None,
            image_cache: cx.new(|cx| {
                crate::image_cache::LruImageCache::avatar_thumbnail_small(
                    "dm-list",
                    512,
                    12 * 1024 * 1024,
                    4 * 1024 * 1024,
                    cx,
                )
            }),
        }
    }

    fn refresh_dm_items(&mut self, cx: &mut Context<Self>) {
        if !is_dm_route(cx) {
            self.pending_rebuild = true;
            return;
        }
        let store = DirectMessageStore::global(cx);
        let fingerprint = dm_items_fingerprint(store.read(cx), cx);
        if fingerprint == self.dm_items_fingerprint {
            return;
        }
        self.dm_items_fingerprint = fingerprint;
        let items = build_dm_items(&mut self.row_caches, store.read(cx), cx);
        if self.dm_items != items {
            self.dm_items = items;
            cx.notify();
        }
    }

    fn render_search(&self, theme: &Theme, locale: &str) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        div()
            .w_full()
            .h(px(50.))
            .px_3()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("dm-search")
                    .w_full()
                    .h(px(36.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .rounded_lg()
                    .bg(theme.tokens.bg_tertiary)
                    .cursor_pointer()
                    .hover(move |this| this.bg(bg_hover))
                    .on_click(|_, _, cx| CommandPaletteModal::try_toggle_authenticated(cx))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(mezon_i18n::t(locale, "clan.findOrStartConversation")),
                    ),
            )
    }

    fn render_friends_button(
        &self,
        theme: &Theme,
        locale: &str,
        active: bool,
        pending: usize,
    ) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        div()
            .id("dm-friends")
            .relative()
            .children(crate::tour::probe(crate::tour::TourAnchor::FriendsButton))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .py_2()
            .px_3()
            .rounded_lg()
            .cursor_pointer()
            .when(active, |this| this.bg(bg_hover))
            .hover(move |this| this.bg(bg_hover))
            .on_click(|_, _window, cx| navigate(cx, Route::Friends))
            .child(img("icons/icon-friends.svg").size(px(20.)).flex_none())
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(locale, "directMessage.friends")),
            )
            .when(pending > 0, |el| {
                el.child(
                    div()
                        .absolute()
                        .right(px(25.))
                        .size(px(16.))
                        .rounded_full()
                        .bg(gpui::rgb(0xda_37_3c))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(9.))
                        .line_height(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui::white())
                        .child(SharedString::from(pending.to_string())),
                )
            })
    }

    fn render_section_header(&self, theme: &Theme, locale: &str) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .pt_4()
            .pb_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_muted)
                    .child(mezon_i18n::t(locale, "directMessage.directMessages")),
            )
            .child(
                div()
                    .id("dm-create")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(bg_hover))
                    .on_click({
                        let locale = locale.to_string();
                        move |_, window, cx| {
                            let locale = locale.clone();
                            let modal =
                                cx.new(|cx| CreateMessageGroupModal::new(locale, window, cx));
                            Shell::global(cx)
                                .update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
                        }
                    })
                    .child(
                        Icon::new(IconName::Plus)
                            .size(px(16.))
                            .text_color(theme.text_muted),
                    ),
            )
    }
}

impl Render for DirectSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("DirectSidebar");
        let theme = cx.theme().clone();
        let theme = &theme;
        let pinned_scroll = self.pinned_scroll.clone();
        let locale = self.settings.read(cx).language.clone();

        let pinned_count = self.dm_items.partition_point(|item| item.pinned);
        let count = self.dm_items.len() - pinned_count;
        let active_id = match Router::global(cx).read(cx).route() {
            Route::DirectMessage { direct_id, .. } => Some(direct_id),
            _ => None,
        };
        let items = self.dm_items.clone();
        let suppress_hover = self.list_scroll.is_scroll_hover_suppressed();
        let image_cache = self.image_cache.clone();
        let in_voice_label: SharedString = mezon_i18n::t(&locale, "memberPage.inVoice").into();
        let menu_sidebar = cx.entity().downgrade();

        let pinned_rows = (pinned_count > 0).then(|| {
            let theme = theme.clone();
            let items = self.dm_items.clone();
            let image_cache = self.image_cache.clone();
            let in_voice_label = in_voice_label.clone();
            let sidebar = menu_sidebar.clone();
            let pinned_scroll_inner = pinned_scroll.clone();
            div()
                .id("dm-pinned-list")
                .px_2()
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&pinned_scroll_inner)
                .children((0..pinned_count).filter_map(move |ix| {
                    items.get(ix).map(|item| {
                        render_dm_row(
                            item,
                            &theme,
                            active_id == Some(item.channel_id),
                            suppress_hover,
                            &image_cache,
                            &in_voice_label,
                            sidebar.clone(),
                        )
                    })
                }))
        });

        let list = uniform_list("dm-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let active_id = active_id;
            range
                .map(|ix| match items.get(pinned_count + ix) {
                    Some(item) => render_dm_row(
                        item,
                        &theme,
                        active_id == Some(item.channel_id),
                        suppress_hover,
                        &image_cache,
                        &in_voice_label,
                        menu_sidebar.clone(),
                    ),
                    None => div().into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .with_item_size(size(px(240.), px(DM_ROW_HEIGHT)))
        .smooth_line_scroll()
        .suppress_hover_while_scrolling()
        .track_scroll(&self.list_scroll)
        .flex_1()
        .h_full()
        .min_h_0()
        .px_2();

        let on_friends = matches!(Router::global(cx).read(cx).route(), Route::Friends);
        let friend_pending = FriendStore::global(cx).read(cx).pending_incoming_count();

        let overlay_sidebar = cx.entity().downgrade();
        let overlay_locale = locale.clone();
        let overlay_settings = self.settings.clone();
        let menu_overlay = self.open_menu.as_ref().map(|menu| {
            let store = NotificationSettingStore::try_global(cx);
            let muted = store
                .as_ref()
                .is_some_and(|s| s.read(cx).is_time_muted(menu.channel_id));
            let muted_until = store
                .as_ref()
                .and_then(|s| s.read(cx).muted_until_ms(menu.channel_id))
                .map(|ms| {
                    format!(
                        "{} {}",
                        mezon_i18n::t(&locale, "channelMenu.menu.notification.mutedUntil"),
                        crate::chat::notification_setting_popover::format_muted_until(ms)
                    )
                });
            (
                menu.position,
                menu.channel_id,
                menu.target.clone(),
                muted,
                muted_until,
                menu.mute_sub_open,
            )
        });

        div()
            .children(crate::tour::probe(crate::tour::TourAnchor::DirectList))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg_secondary)
            .child(self.render_search(theme, &locale))
            .child(div().px_2().pt_2().child(self.render_friends_button(
                theme,
                &locale,
                on_friends,
                friend_pending,
            )))
            .when_some(pinned_rows, |el, rows| {
                el.child(
                    div()
                        .w_full()
                        .px_4()
                        .pt_4()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(&locale, "directMessage.pinned")),
                )
                .child(
                    div()
                        .relative()
                        .h(pinned_list_height(pinned_count))
                        .child(rows)
                        .custom_scrollbars(
                            Scrollbars::always_visible(ScrollAxes::Vertical)
                                .tracked_scroll_handle(&pinned_scroll),
                            window,
                            cx,
                        ),
                )
            })
            .child(self.render_section_header(theme, &locale))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(list)
                    .custom_scrollbars(
                        Scrollbars::always_visible(ScrollAxes::Vertical)
                            .tracked_scroll_handle(&self.list_scroll),
                        window,
                        cx,
                    ),
            )
            .when_some(
                menu_overlay,
                move |el, (position, channel_id, target, muted, muted_until, mute_open)| {
                    el.child(context_menu_at(
                        position,
                        build_dm_menu(
                            overlay_sidebar.clone(),
                            overlay_settings.clone(),
                            &overlay_locale,
                            channel_id,
                            target,
                            muted,
                            muted_until,
                            mute_open,
                        ),
                    ))
                },
            )
    }
}
