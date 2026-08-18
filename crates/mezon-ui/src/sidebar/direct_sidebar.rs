use std::rc::Rc;

use gpui::{
    App, Context, Entity, FontWeight, MouseButton, MouseDownEvent, Pixels, Point, SharedString,
    UniformListScrollHandle, WeakEntity, Window, div, img, prelude::*, px, size, uniform_list,
};
use mezon_store::{
    ChannelEvent, ChannelId, ChannelList, ClanId, DirectChannel, DirectKind, DirectMessageStore,
    DmAvatarPresence, FriendStore, NotificationSettingStore, PresenceEvent, PresenceStore,
    Settings,
};

use super::channel_sidebar::menu::{MUTE_DURATIONS, apply_mute, mute_label, submenu_options};
use super::create_message_group_modal::CreateMessageGroupModal;
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::app::shell::Shell;
use crate::command_palette::CommandPaletteModal;
use crate::components::compositions::{DM_ROW_HEIGHT, DmRow};
use crate::components::primitives::{ContextMenu, Icon, IconName, context_menu_at};
use crate::router::{Route, Router, navigate};
use crate::theme::{ActiveTheme, Theme};

struct DmMenu {
    position: Point<Pixels>,
    channel_id: ChannelId,
    mute_sub_open: bool,
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
}

pub struct DirectSidebar {
    settings: Entity<Settings>,
    list_scroll: UniformListScrollHandle,
    dm_items: Rc<Vec<DmItem>>,
    dm_items_fingerprint: u64,
    pending_rebuild: bool,
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
    store.channels().iter().fold(FNV_OFFSET, |h, ch| {
        let h = fold(h, &ch.id.0.to_le_bytes());
        let h = fold(
            h,
            &[
                ch.kind as u8,
                u8::from(ch.is_unread()),
                dm_presence_badge(ch, presence) as u8,
                u8::from(dm_in_voice(ch, channels)),
                u8::from(super::dm_muted(ch.id, cx)),
            ],
        );
        let h = fold(h, ch.label.as_bytes());
        fold(h, ch.avatar.as_bytes())
    })
}

fn build_dm_items(store: &DirectMessageStore, cx: &App) -> Rc<Vec<DmItem>> {
    let channel_list = ChannelList::global(cx);
    let channels = channel_list.read(cx);
    let presence = PresenceStore::global(cx).read(cx);
    Rc::new(
        store
            .channels()
            .iter()
            .map(|ch| DmItem {
                channel_id: ch.id,
                id: SharedString::from(ch.id.to_string()),
                row_id: SharedString::from(format!("dm-{}", ch.id)),
                group_name: SharedString::from(format!("dm-row-{}", ch.id)),
                close_id: SharedString::from(format!("dm-close-{}", ch.id)),
                label: SharedString::from(ch.label.clone()),
                kind: ch.kind,
                unread: ch.is_unread(),
                presence_badge: dm_presence_badge(ch, presence),
                in_voice: dm_in_voice(ch, channels),
                muted: super::dm_muted(ch.id, cx),
                avatar_src: SharedString::from(crate::util::imgproxy::avatar_url(cx, &ch.avatar)),
                avatar_raw: SharedString::from(ch.avatar.clone()),
            })
            .collect(),
    )
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

fn build_dm_menu(
    sidebar: WeakEntity<DirectSidebar>,
    locale: &str,
    channel_id: ChannelId,
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
        })
        .item(
            t("channelMenu.menu.watchMenu.markAsRead"),
            move |_window: &mut Window, cx: &mut App| {
                DirectMessageStore::global(cx).update(cx, |store, cx| {
                    store.mark_as_read(channel_id, cx);
                });
            },
        )
        .separator();
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
                this.dm_items = build_dm_items(store.read(cx), cx);
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
        let dm_items = build_dm_items(direct_store.read(cx), cx);

        Self {
            settings,
            list_scroll: UniformListScrollHandle::new(),
            dm_items,
            dm_items_fingerprint,
            pending_rebuild: false,
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
        let items = build_dm_items(store.read(cx), cx);
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
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();

        let count = self.dm_items.len();
        let active_id = match Router::global(cx).read(cx).route() {
            Route::DirectMessage { direct_id, .. } => Some(direct_id),
            _ => None,
        };
        let items = self.dm_items.clone();
        let suppress_hover = self.list_scroll.is_scroll_hover_suppressed();
        let image_cache = self.image_cache.clone();
        let in_voice_label: SharedString = mezon_i18n::t(&locale, "memberPage.inVoice").into();
        let menu_sidebar = cx.entity().downgrade();

        let list = uniform_list("dm-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let active_id = active_id;
            range
                .map(|ix| match items.get(ix) {
                    Some(item) => {
                        let selected = active_id == Some(item.channel_id);
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
                        .image_cache(image_cache.clone());
                        if item.in_voice {
                            row = row.in_voice_label(in_voice_label.clone());
                        }
                        let channel_id = item.channel_id;
                        let sidebar = menu_sidebar.clone();
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
                                            });
                                            if let Some(store) =
                                                NotificationSettingStore::try_global(cx)
                                            {
                                                store.update(cx, |store, cx| {
                                                    store.ensure_channel(ClanId(0), channel_id, cx);
                                                });
                                            }
                                            cx.notify();
                                        });
                                    }
                                },
                            )
                            .child(row.render(&theme))
                            .into_any_element()
                    }
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
                muted,
                muted_until,
                menu.mute_sub_open,
            )
        });

        div()
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
                move |el, (position, channel_id, muted, muted_until, mute_open)| {
                    el.child(context_menu_at(
                        position,
                        build_dm_menu(
                            overlay_sidebar.clone(),
                            &overlay_locale,
                            channel_id,
                            muted,
                            muted_until,
                            mute_open,
                        ),
                    ))
                },
            )
    }
}
