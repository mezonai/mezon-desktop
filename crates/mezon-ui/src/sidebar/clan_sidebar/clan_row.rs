use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Entity, Hsla, MouseButton,
    MouseDownEvent, Rgba, SharedString, WeakEntity, Window, div, img, prelude::*, px,
};
use mezon_store::notification_setting::{
    NOTIFICATION_ALL_MESSAGE, NOTIFICATION_MENTION_MESSAGE, NOTIFICATION_NOTHING_MESSAGE,
};
use mezon_store::{ChannelList, ClanId, ClanList, NotificationSettingStore};

use super::{ClanMenuArgs, ClanSidebar};
use crate::app::shell::Shell;
use crate::components::primitives::{
    ContextMenu, SubmenuOption, avatar_color, avatar_text_color, clipped_initials_tile,
    initials_tile, initials_tile_identified, mention_count_badge, name_initials,
};
use crate::router::{Route, Router};
use crate::theme::ActiveTheme;

pub(super) const CLAN_ROW_HEIGHT: f32 = 56.;

const CLAN_DRAG_INDICATOR_COLOR: u32 = 0x3b82f6;
const CLAN_AVATAR_PX: f32 = 40.;
const CLAN_AVATAR_RADIUS: f32 = 8.;

fn render_clan_initials_avatar(
    name: &str,
    avatar_id: SharedString,
    suppress_hover: bool,
    muted: bool,
    hover_bg: Hsla,
) -> AnyElement {
    if name.is_empty() {
        return div().size(px(CLAN_AVATAR_PX)).into_any_element();
    }
    let size = px(CLAN_AVATAR_PX);
    let radius = px(CLAN_AVATAR_RADIUS);
    let mut bg = avatar_color(name);
    if muted {
        bg = bg.grayscale();
    }
    let text_color = avatar_text_color(bg);
    let hover = (!suppress_hover).then_some(hover_bg);
    initials_tile_identified(
        size,
        Some(radius),
        bg,
        text_color,
        name_initials(name),
        avatar_id,
        hover,
    )
}

#[derive(Clone)]
pub(super) struct ClanReorderDrag {
    pub(super) index: usize,
    pub(super) name: SharedString,
    pub(super) avatar: Option<SharedString>,
    pub(super) muted: bool,
}

pub(super) struct ClanDragPreview {
    name: SharedString,
    avatar: Option<SharedString>,
    muted: bool,
}

impl Render for ClanDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let size = px(CLAN_AVATAR_PX);
        let radius = px(CLAN_AVATAR_RADIUS);
        if let Some(avatar) = self.avatar.clone() {
            img(avatar)
                .size(size)
                .rounded(radius)
                .object_fit(gpui::ObjectFit::Cover)
                .grayscale(self.muted)
                .opacity(0.8)
                .into_any_element()
        } else if self.name.is_empty() {
            div().size(size).opacity(0.8).into_any_element()
        } else {
            let mut bg = avatar_color(self.name.as_ref());
            if self.muted {
                bg = bg.grayscale();
            }
            div()
                .opacity(0.8)
                .child(initials_tile(
                    size,
                    Some(radius),
                    bg,
                    avatar_text_color(bg),
                    name_initials(self.name.as_ref()),
                ))
                .into_any_element()
        }
    }
}

const CLAN_NOTI_LEVELS: [(i32, &str); 3] = [
    (
        NOTIFICATION_ALL_MESSAGE,
        "channelMenu.menu.notification.all",
    ),
    (
        NOTIFICATION_MENTION_MESSAGE,
        "channelMenu.menu.notification.onlyMention",
    ),
    (
        NOTIFICATION_NOTHING_MESSAGE,
        "channelMenu.menu.notification.nothing",
    ),
];

pub(super) fn build_clan_rail_menu(
    sidebar: WeakEntity<ClanSidebar>,
    args: &ClanMenuArgs,
) -> ContextMenu {
    let ClanMenuArgs {
        clan_id,
        clan_default,
        noti_sub_open,
        can_leave,
        ..
    } = *args;
    let locale = args.locale.as_ref();
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let locale_owned = locale.to_string();
    let sidebar_dismiss = sidebar.clone();

    let sidebar_close = sidebar.clone();
    let mut menu = ContextMenu::new()
        .on_submenu_close(move |_window, cx| {
            let _ = sidebar_close.update(cx, |this, cx| this.close_clan_submenus(cx));
        })
        .on_dismiss(move |_window, cx| {
            if let Some(view) = sidebar_dismiss.upgrade() {
                view.update(cx, |this, cx| {
                    this.clan_menu = None;
                    cx.notify();
                });
            }
        });

    menu = menu
        .item(t("contextMenu.markAsRead"), move |_window, cx| {
            ChannelList::global(cx).update(cx, |channels, cx| {
                channels.mark_clan_as_read(clan_id, cx);
            });
        })
        .separator();

    let level = clan_default.unwrap_or(NOTIFICATION_ALL_MESSAGE);
    let options: Vec<SubmenuOption> = CLAN_NOTI_LEVELS
        .iter()
        .map(|(value, key)| SubmenuOption {
            value: *value,
            label: mezon_i18n::t(locale, key).into(),
            selected: *value == level,
            disabled: false,
        })
        .collect();
    let sub_text = CLAN_NOTI_LEVELS
        .iter()
        .find(|(value, _)| *value == level)
        .map(|(_, key)| mezon_i18n::t(locale, key).into());
    let sidebar_open = sidebar.clone();
    menu = menu.submenu(
        t("contextMenu.notificationSettings"),
        sub_text,
        options,
        noti_sub_open,
        move |_window, cx| {
            let _ = sidebar_open.update(cx, |this, cx| this.set_clan_noti_sub_open(cx));
        },
        move |lvl, _window, cx| {
            if let Some(store) = NotificationSettingStore::try_global(cx) {
                store.update(cx, |store, cx| store.set_clan_level(clan_id, lvl, cx));
            }
        },
    );

    menu = menu.item(t("contextMenu.editClanProfile"), move |_window, cx| {
        crate::router::navigate(cx, Route::SettingsClanProfile { clan_id });
    });

    if can_leave {
        let leave_locale = locale_owned;
        menu = menu.danger_item(t("contextMenu.leaveClan"), move |window, cx| {
            let leave_locale = leave_locale.clone();
            Shell::global(cx).update(cx, |shell, cx| {
                shell.confirm_leave_clan(clan_id, &leave_locale, window, cx);
            });
        });
    }

    menu
}

#[derive(Clone, PartialEq)]
pub(super) struct ClanRow {
    pub(super) id: SharedString,
    pub(super) id_num: ClanId,
    pub(super) row_id: SharedString,
    pub(super) group_name: SharedString,
    pub(super) name: SharedString,
    pub(super) proxied_avatar_url: Option<SharedString>,
    pub(super) avatar_id: SharedString,
    pub(super) badge_count: u32,
    pub(super) has_unread: bool,
    pub(super) muted: bool,
    pub active: bool,
}

fn activate_clan(clan_list: &Entity<ClanList>, clan_id: ClanId, cx: &mut App) {
    clan_list.update(cx, |m, cx| {
        m.select_clan(clan_id, cx);
    });
    if !matches!(
        Router::global(cx).read(cx).route(),
        Route::Chat | Route::Channel { .. }
    ) {
        crate::router::navigate(cx, Route::Chat);
    }
}

fn on_clan_click(
    clan_list: Entity<ClanList>,
    clan_id: SharedString,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        activate_clan(&clan_list, clan_id.parse().unwrap_or_default(), cx);
    }
}

pub(super) fn render_pill(
    is_active: bool,
    group_name: SharedString,
    pill_color: Rgba,
) -> AnyElement {
    if !is_active {
        return div().into_any_element();
    }

    let anim_id = SharedString::from(format!("pill-{group_name}"));
    div()
        .absolute()
        .left(px(8.))
        .top_0()
        .bottom_0()
        .flex()
        .items_center()
        .child(
            div()
                .w(px(4.))
                .rounded_full()
                .bg(pill_color)
                .with_animation(
                    anim_id,
                    Animation::new(Duration::from_millis(200))
                        .with_easing(|t| 1.0 - (1.0 - t).powi(3)),
                    |el, delta| el.h(px(32. * delta)),
                ),
        )
        .into_any_element()
}

pub(super) fn render_clan_row(
    rows: &[ClanRow],
    ix: usize,
    cx: &App,
    clan_list_handle: Entity<ClanList>,
    suppress_hover: bool,
    sidebar: WeakEntity<ClanSidebar>,
) -> AnyElement {
    let theme = cx.theme();
    let dm_active = matches!(
        Router::global(cx).read(cx).route(),
        Route::Direct | Route::DirectMessage { .. }
    );
    let Some(clan) = rows.get(ix) else {
        return div().into_any_element();
    };

    let clan_id = clan.id.clone();
    let prefetch_clan_id = clan.id_num;
    let is_active = clan.active && !dm_active;
    let show_badge = crate::SHOW_UNREAD_BADGE_COUNT && clan.badge_count > 0 && !clan.muted;
    let show_nub = clan.has_unread && clan.badge_count == 0 && !clan.muted && !is_active;
    let badge_count = clan.badge_count;
    let muted = clan.muted;
    let pill_color = theme.tokens.text_theme_primary;

    let hover_bg = theme.tokens.bg_button_add_friend;
    let element_bg = Hsla::from(theme.bg_tertiary);
    let avatar: AnyElement = if !clan.name.is_empty() {
        if let Some(ref proxied) = clan.proxied_avatar_url {
            clipped_initials_tile(
                px(CLAN_AVATAR_PX),
                px(CLAN_AVATAR_RADIUS),
                proxied.clone(),
                muted,
                element_bg,
                clan.name.as_ref(),
            )
        } else {
            render_clan_initials_avatar(
                clan.name.as_ref(),
                clan.avatar_id.clone(),
                suppress_hover,
                muted,
                Hsla::from(hover_bg),
            )
        }
    } else {
        div().size(px(CLAN_AVATAR_PX)).into_any_element()
    };

    let avatar_with_badge = div().relative().child(avatar).when(show_badge, |el| {
        el.child(
            mention_count_badge(badge_count)
                .absolute()
                .bottom(px(-1.))
                .right(px(-2.))
                .border_1()
                .border_color(gpui::white()),
        )
    });

    div()
        .id(clan.row_id.clone())
        .group(clan.group_name.clone())
        .relative()
        .w_full()
        .h(px(CLAN_ROW_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(render_pill(is_active, clan.group_name.clone(), pill_color))
        .when(show_nub, |el| {
            el.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .w(px(4.))
                            .h(px(8.))
                            .rounded_r(px(4.))
                            .bg(theme.tokens.bg_unread_message),
                    ),
            )
        })
        .on_drag(
            ClanReorderDrag {
                index: ix,
                name: clan.name.clone(),
                avatar: clan.proxied_avatar_url.clone(),
                muted,
            },
            |drag, _, _, cx| {
                cx.stop_propagation();
                let name = drag.name.clone();
                let avatar = drag.avatar.clone();
                let muted = drag.muted;
                cx.new(|_| ClanDragPreview {
                    name,
                    avatar,
                    muted,
                })
            },
        )
        .drag_over::<ClanReorderDrag>(move |style, drag, _, _| {
            if drag.index == ix {
                style
            } else if drag.index > ix {
                style
                    .border_t_2()
                    .border_color(gpui::rgb(CLAN_DRAG_INDICATOR_COLOR))
            } else {
                style
                    .border_b_2()
                    .border_color(gpui::rgb(CLAN_DRAG_INDICATOR_COLOR))
            }
        })
        .on_drop({
            let clan_list = clan_list_handle.clone();
            let clan_num = prefetch_clan_id;
            move |drag: &ClanReorderDrag, _, cx| {
                let from = drag.index;
                if from == ix {
                    activate_clan(&clan_list, clan_num, cx);
                    return;
                }
                clan_list.update(cx, |list, cx| list.move_clan(from, ix, cx));
            }
        })
        .on_click(on_clan_click(clan_list_handle, clan_id))
        .on_mouse_down(MouseButton::Right, {
            let clan_num = prefetch_clan_id;
            move |event: &MouseDownEvent, _window, cx| {
                let position = event.position;
                if let Some(view) = sidebar.upgrade() {
                    view.update(cx, |this, cx| {
                        this.open_clan_menu(clan_num, position, cx);
                    });
                }
            }
        })
        .child(avatar_with_badge)
        .into_any_element()
}
