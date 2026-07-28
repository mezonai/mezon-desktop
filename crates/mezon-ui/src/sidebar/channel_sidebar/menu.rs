use gpui::{App, ClickEvent, Entity, Pixels, Point, WeakEntity, Window};
use mezon_store::{ChannelId, ChannelList, ChannelType, ClanId};

use super::ChannelSidebar;
use crate::app::shell::Shell;
use crate::components::primitives::{ContextMenu, SubmenuOption};

pub(super) fn on_channel_click(
    channel_id: String,
    clan_id: Option<ClanId>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        if let Some(ref cid) = clan_id {
            crate::router::navigate(
                cx,
                crate::router::Route::Channel {
                    clan_id: *cid,
                    channel_id: channel_id.parse().unwrap_or_default(),
                },
            );
        }
    }
}

pub(super) fn on_category_click(
    channel_list: Entity<ChannelList>,
    clan_id: ClanId,
    category_id: String,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        channel_list.update(cx, |m, cx| {
            m.toggle_category(clan_id, &category_id, cx);
        });
    }
}

#[derive(Clone)]
pub(super) struct OpenMenu {
    pub(super) channel_type: ChannelType,
    pub(super) is_thread: bool,
    pub(super) position: Point<Pixels>,
    pub(super) channel_id: ChannelId,
    pub(super) clan_id: ClanId,
    pub(super) mute_sub_open: bool,
    pub(super) noti_sub_open: bool,
}

fn coming_soon_toast(message: String) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let message = message.clone();
        Shell::global(cx).update(cx, move |shell, cx| shell.info(message, cx));
    }
}

fn coming_soon_modal(title: String, locale: String) -> impl Fn(&mut Window, &mut App) + 'static {
    move |window: &mut Window, cx: &mut App| {
        let title = title.clone();
        let locale = locale.clone();
        Shell::global(cx).update(cx, |shell, cx| {
            shell.show_coming_soon(title, &locale, window, cx);
        });
    }
}

pub(crate) const MUTE_DURATIONS: &[(i32, &str)] = &[
    (
        mezon_store::notification_setting::MUTE_FOR_15_MINUTES_SEC,
        "channelMenu.menu.notification.for15Minutes",
    ),
    (
        mezon_store::notification_setting::MUTE_FOR_1_HOUR_SEC,
        "channelMenu.menu.notification.for1Hour",
    ),
    (
        mezon_store::notification_setting::MUTE_FOR_3_HOURS_SEC,
        "channelMenu.menu.notification.for3Hours",
    ),
    (
        mezon_store::notification_setting::MUTE_FOR_8_HOURS_SEC,
        "channelMenu.menu.notification.for8Hours",
    ),
    (
        mezon_store::notification_setting::MUTE_FOR_24_HOURS_SEC,
        "channelMenu.menu.notification.for24Hours",
    ),
    (
        mezon_store::notification_setting::MUTE_FOREVER,
        "channelMenu.menu.notification.untilTurnedBackOn",
    ),
];

pub(crate) const NOTI_LEVELS: &[(i32, &str)] = &[
    (
        mezon_store::notification_setting::NOTIFICATION_DEFAULT,
        "channelMenu.menu.notification.useCategoryDefault",
    ),
    (
        mezon_store::notification_setting::NOTIFICATION_ALL_MESSAGE,
        "channelMenu.menu.notification.all",
    ),
    (
        mezon_store::notification_setting::NOTIFICATION_MENTION_MESSAGE,
        "channelMenu.menu.notification.onlyMention",
    ),
    (
        mezon_store::notification_setting::NOTIFICATION_NOTHING_MESSAGE,
        "channelMenu.menu.notification.nothing",
    ),
];

/// Mirrors React `PanelChannel`: the row's subText shows the level actually in
/// effect, resolving `DEFAULT` through the clan default.
pub(crate) fn effective_level_label(locale: &str, level: i32, clan_default: Option<i32>) -> String {
    use mezon_store::notification_setting as ns;
    let effective = if level == ns::NOTIFICATION_DEFAULT {
        clan_default.unwrap_or(ns::NOTIFICATION_DEFAULT)
    } else {
        level
    };
    let key = match effective {
        v if v == ns::NOTIFICATION_ALL_MESSAGE => "channelMenu.menu.notification.all",
        v if v == ns::NOTIFICATION_MENTION_MESSAGE => "channelMenu.menu.notification.onlyMention",
        v if v == ns::NOTIFICATION_NOTHING_MESSAGE => "channelMenu.menu.notification.nothing",
        _ => "channelMenu.menu.notification.useCategoryDefault",
    };
    mezon_i18n::t(locale, key).to_string()
}

pub(crate) fn submenu_options(
    locale: &str,
    source: &[(i32, &'static str)],
    selected: i32,
) -> Vec<SubmenuOption> {
    source
        .iter()
        .map(|(value, key)| SubmenuOption {
            value: *value,
            label: mezon_i18n::t(locale, key).into(),
            selected: *value == selected,
        })
        .collect()
}

fn set_submenu_open(
    sidebar: WeakEntity<ChannelSidebar>,
    mute: bool,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let _ = sidebar.update(cx, |this, cx| {
            if let Some(menu) = this.open_menu.as_mut() {
                let (m, n) = if mute { (true, false) } else { (false, true) };
                if menu.mute_sub_open != m || menu.noti_sub_open != n {
                    menu.mute_sub_open = m;
                    menu.noti_sub_open = n;
                    cx.notify();
                }
            }
        });
    }
}

pub(crate) fn apply_mute(
    channel_id: ChannelId,
    clan_id: ClanId,
) -> impl Fn(i32, &mut Window, &mut App) + 'static {
    move |seconds: i32, _window: &mut Window, cx: &mut App| {
        if let Some(store) = mezon_store::NotificationSettingStore::try_global(cx) {
            store.update(cx, |store, cx| {
                store.set_mute(channel_id, clan_id, seconds, cx)
            });
        }
    }
}

pub(crate) fn apply_level(
    channel_id: ChannelId,
    clan_id: ClanId,
) -> impl Fn(i32, &mut Window, &mut App) + 'static {
    move |level: i32, _window: &mut Window, cx: &mut App| {
        if let Some(store) = mezon_store::NotificationSettingStore::try_global(cx) {
            store.update(cx, |store, cx| {
                store.set_level(channel_id, clan_id, level, cx)
            });
        }
    }
}

pub(crate) fn mute_label(locale: &str, is_thread: bool, muted: bool) -> String {
    let key = match (is_thread, muted) {
        (true, true) => "channelMenu.menu.notification.unmuteThreadStatus",
        (true, false) => "channelMenu.menu.notification.muteThreadStatus",
        (false, true) => "channelMenu.menu.notification.unmuteChannelStatus",
        (false, false) => "channelMenu.menu.notification.muteChannelStatus",
    };
    mezon_i18n::t(locale, key).to_string()
}

fn apply_unmute(
    channel_id: ChannelId,
    clan_id: ClanId,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        if let Some(store) = mezon_store::NotificationSettingStore::try_global(cx) {
            store.update(cx, |store, cx| store.unmute(channel_id, clan_id, cx));
        }
    }
}

fn apply_mute_forever(
    channel_id: ChannelId,
    clan_id: ClanId,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        if let Some(store) = mezon_store::NotificationSettingStore::try_global(cx) {
            store.update(cx, |store, cx| store.mute_forever(channel_id, clan_id, cx));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_mute_and_notification(
    menu: ContextMenu,
    sidebar: WeakEntity<ChannelSidebar>,
    locale: &str,
    channel_id: ChannelId,
    clan_id: ClanId,
    is_thread: bool,
    time_muted: bool,
    muted_until: Option<String>,
    level: i32,
    clan_default: Option<i32>,
    mute_sub_open: bool,
    noti_sub_open: bool,
    show_notification: bool,
) -> ContextMenu {
    let mut menu = if time_muted {
        let label = match muted_until {
            Some(until) => format!("{} · {until}", mute_label(locale, is_thread, true)),
            None => mute_label(locale, is_thread, true),
        };
        menu.item(label, apply_unmute(channel_id, clan_id))
    } else {
        menu.submenu_clickable(
            mute_label(locale, is_thread, false),
            None,
            submenu_options(locale, MUTE_DURATIONS, -2),
            mute_sub_open,
            set_submenu_open(sidebar.clone(), true),
            apply_mute(channel_id, clan_id),
            apply_mute_forever(channel_id, clan_id),
        )
    };
    if show_notification {
        menu = menu.submenu(
            mezon_i18n::t(locale, "channelMenu.menu.notification.notification").to_string(),
            Some(effective_level_label(locale, level, clan_default).into()),
            submenu_options(locale, NOTI_LEVELS, level),
            noti_sub_open,
            set_submenu_open(sidebar, false),
            apply_level(channel_id, clan_id),
        );
    }
    menu
}

pub(super) fn build_channel_menu(
    sidebar: WeakEntity<ChannelSidebar>,
    locale: &str,
    channel_type: ChannelType,
    is_thread: bool,
    channel_id: ChannelId,
    clan_id: ClanId,
    muted: bool,
    muted_until: Option<String>,
    level: i32,
    mute_sub_open: bool,
    noti_sub_open: bool,
    clan_default: Option<i32>,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let coming_soon = t("common.comingSoon");
    let locale_owned = locale.to_string();
    let sidebar_dismiss = sidebar.clone();
    let show_notification = matches!(channel_type, ChannelType::Text) || is_thread;

    let mut menu = ContextMenu::new().on_dismiss(move |_window, cx| {
        if let Some(view) = sidebar_dismiss.upgrade() {
            view.update(cx, |this, cx| {
                this.open_menu = None;
                cx.notify();
            });
        }
    });

    menu = menu
        .item(
            t("channelMenu.menu.watchMenu.markAsRead"),
            coming_soon_toast(coming_soon.clone()),
        )
        .separator()
        .item(
            t("channelMenu.menu.inviteMenu.copyLink"),
            coming_soon_toast(coming_soon.clone()),
        )
        .separator();

    if is_thread {
        let edit_label = t("channelMenu.menu.manageThreadMenu.editThread");
        let leave_label = t("channelMenu.menu.manageThreadMenu.leaveThread");
        let delete_label = t("channelMenu.menu.manageThreadMenu.deleteThread");
        menu = menu.item(
            t("channelMenu.menu.notification.archiveThread"),
            coming_soon_modal(
                t("channelMenu.menu.notification.archiveThread"),
                locale_owned.clone(),
            ),
        );
        menu = push_mute_and_notification(
            menu,
            sidebar.clone(),
            locale,
            channel_id,
            clan_id,
            true,
            muted,
            muted_until.clone(),
            level,
            clan_default,
            mute_sub_open,
            noti_sub_open,
            show_notification,
        );
        menu = menu
            .danger_item(
                leave_label.clone(),
                coming_soon_modal(leave_label, locale_owned.clone()),
            )
            .separator()
            .item(
                edit_label.clone(),
                coming_soon_modal(edit_label, locale_owned.clone()),
            )
            .danger_item(
                delete_label.clone(),
                coming_soon_modal(delete_label, locale_owned.clone()),
            );
    } else {
        let edit_label = t("channelMenu.menu.organizationMenu.edit");
        let delete_label = t("channelMenu.menu.organizationMenu.deleteChannel");
        menu = menu.item(
            t("channelMenu.menu.notification.archiveChannel"),
            coming_soon_modal(
                t("channelMenu.menu.notification.archiveChannel"),
                locale_owned.clone(),
            ),
        );
        menu = push_mute_and_notification(
            menu,
            sidebar.clone(),
            locale,
            channel_id,
            clan_id,
            false,
            muted,
            muted_until.clone(),
            level,
            clan_default,
            mute_sub_open,
            noti_sub_open,
            show_notification,
        );
        menu = menu
            .item(
                t("channelMenu.menu.inviteMenu.markFavorite"),
                coming_soon_toast(coming_soon.clone()),
            )
            .separator()
            .item(
                edit_label.clone(),
                coming_soon_modal(edit_label, locale_owned.clone()),
            );

        let create_label = match channel_type {
            ChannelType::Voice => Some(t("channelMenu.menu.organizationMenu.createVoiceChannel")),
            ChannelType::Text => Some(t("channelMenu.menu.organizationMenu.createTextChannel")),
            _ => None,
        };
        if let Some(create_label) = create_label {
            menu = menu.item(
                create_label.clone(),
                coming_soon_modal(create_label, locale_owned.clone()),
            );
        }

        menu = menu.danger_item(
            delete_label.clone(),
            coming_soon_modal(delete_label, locale_owned.clone()),
        );
    }

    menu
}

pub(crate) const CATEGORY_NOTI_LEVELS: &[(i32, &str)] = &[
    (
        mezon_store::notification_setting::NOTIFICATION_DEFAULT,
        "contextMenu.useClanDefault",
    ),
    (
        mezon_store::notification_setting::NOTIFICATION_ALL_MESSAGE,
        "channelMenu.menu.notification.all",
    ),
    (
        mezon_store::notification_setting::NOTIFICATION_MENTION_MESSAGE,
        "channelMenu.menu.notification.onlyMention",
    ),
    (
        mezon_store::notification_setting::NOTIFICATION_NOTHING_MESSAGE,
        "channelMenu.menu.notification.nothing",
    ),
];

pub(crate) fn category_level_label(locale: &str, level: i32) -> String {
    use mezon_store::notification_setting as ns;
    let key = match level {
        v if v == ns::NOTIFICATION_ALL_MESSAGE => "channelMenu.menu.notification.all",
        v if v == ns::NOTIFICATION_MENTION_MESSAGE => "channelMenu.menu.notification.onlyMention",
        v if v == ns::NOTIFICATION_NOTHING_MESSAGE => "channelMenu.menu.notification.nothing",
        _ => "contextMenu.useClanDefault",
    };
    mezon_i18n::t(locale, key).to_string()
}

#[derive(Clone)]
pub(super) struct CategoryMenu {
    pub(super) position: Point<Pixels>,
    pub(super) category_id: String,
    pub(super) clan_id: ClanId,
    pub(super) collapsed: bool,
    pub(super) mute_sub_open: bool,
    pub(super) noti_sub_open: bool,
}

fn set_category_submenu_open(
    sidebar: WeakEntity<ChannelSidebar>,
    mute: bool,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let _ = sidebar.update(cx, |this, cx| {
            if let Some(menu) = this.category_menu.as_mut() {
                let (m, n) = if mute { (true, false) } else { (false, true) };
                if menu.mute_sub_open != m || menu.noti_sub_open != n {
                    menu.mute_sub_open = m;
                    menu.noti_sub_open = n;
                    cx.notify();
                }
            }
        });
    }
}

fn apply_category_mute(
    category_id: String,
    clan_id: ClanId,
) -> impl Fn(i32, &mut Window, &mut App) + 'static {
    move |seconds: i32, _window: &mut Window, cx: &mut App| {
        if let Some(store) = mezon_store::NotificationSettingStore::try_global(cx) {
            let category_id = category_id.clone();
            store.update(cx, |store, cx| {
                store.set_category_mute(category_id, clan_id, seconds, cx)
            });
        }
    }
}

fn apply_category_level(
    category_id: String,
    clan_id: ClanId,
) -> impl Fn(i32, &mut Window, &mut App) + 'static {
    move |level: i32, _window: &mut Window, cx: &mut App| {
        if let Some(store) = mezon_store::NotificationSettingStore::try_global(cx) {
            let category_id = category_id.clone();
            store.update(cx, |store, cx| {
                store.set_category_level(category_id, clan_id, level, cx)
            });
        }
    }
}

fn apply_category_unmute(
    category_id: String,
    clan_id: ClanId,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        if let Some(store) = mezon_store::NotificationSettingStore::try_global(cx) {
            let category_id = category_id.clone();
            store.update(cx, |store, cx| {
                store.unmute_category(category_id, clan_id, cx)
            });
        }
    }
}

fn apply_category_mute_forever(
    category_id: String,
    clan_id: ClanId,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        if let Some(store) = mezon_store::NotificationSettingStore::try_global(cx) {
            let category_id = category_id.clone();
            store.update(cx, |store, cx| {
                store.mute_category_forever(category_id, clan_id, cx)
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_category_menu(
    sidebar: WeakEntity<ChannelSidebar>,
    channel_list: Entity<ChannelList>,
    locale: &str,
    category_id: String,
    clan_id: ClanId,
    collapsed: bool,
    time_muted: bool,
    muted_until: Option<String>,
    level: i32,
    mute_sub_open: bool,
    noti_sub_open: bool,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let coming_soon = t("common.comingSoon");
    let locale_owned = locale.to_string();
    let sidebar_dismiss = sidebar.clone();

    let mut menu = ContextMenu::new().on_dismiss(move |_window, cx| {
        if let Some(view) = sidebar_dismiss.upgrade() {
            view.update(cx, |this, cx| {
                this.category_menu = None;
                cx.notify();
            });
        }
    });

    menu = menu
        .item(
            t("contextMenu.markAsRead"),
            coming_soon_toast(coming_soon.clone()),
        )
        .separator()
        .checkbox_item(t("contextMenu.collapseCategory"), collapsed, {
            let channel_list = channel_list.clone();
            let category_id = category_id.clone();
            move |_window, cx| {
                channel_list.update(cx, |list, cx| {
                    list.toggle_category(clan_id, &category_id, cx);
                });
            }
        })
        .item(
            t("contextMenu.collapseAllCategories"),
            coming_soon_toast(coming_soon.clone()),
        )
        .separator();

    menu = if time_muted {
        let label = match muted_until {
            Some(until) => format!("{} · {until}", t("contextMenu.unmuteCategory")),
            None => t("contextMenu.unmuteCategory"),
        };
        menu.item(label, apply_category_unmute(category_id.clone(), clan_id))
    } else {
        menu.submenu_clickable(
            t("contextMenu.muteCategory"),
            None,
            submenu_options(locale, MUTE_DURATIONS, -2),
            mute_sub_open,
            set_category_submenu_open(sidebar.clone(), true),
            apply_category_mute(category_id.clone(), clan_id),
            apply_category_mute_forever(category_id.clone(), clan_id),
        )
    };

    menu = menu.submenu(
        t("contextMenu.notificationSettings"),
        Some(category_level_label(locale, level).into()),
        submenu_options(locale, CATEGORY_NOTI_LEVELS, level),
        noti_sub_open,
        set_category_submenu_open(sidebar, false),
        apply_category_level(category_id, clan_id),
    );

    let edit_label = t("contextMenu.editCategory");
    let delete_label = t("contextMenu.deleteCategory");
    menu = menu
        .separator()
        .item(
            edit_label.clone(),
            coming_soon_modal(edit_label, locale_owned.clone()),
        )
        .danger_item(
            delete_label.clone(),
            coming_soon_modal(delete_label, locale_owned),
        );

    menu
}
