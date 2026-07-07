use gpui::{App, ClickEvent, Entity, Pixels, Point, WeakEntity, Window};
use mezon_store::{ChannelList, ChannelType, ClanId};

use super::ChannelSidebar;
use crate::app::shell::Shell;
use crate::components::primitives::ContextMenu;

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

pub(super) fn build_channel_menu(
    sidebar: WeakEntity<ChannelSidebar>,
    locale: &str,
    channel_type: ChannelType,
    is_thread: bool,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let coming_soon = t("common.comingSoon");
    let locale_owned = locale.to_string();

    let mut menu = ContextMenu::new().on_dismiss(move |_window, cx| {
        if let Some(view) = sidebar.upgrade() {
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
        menu = menu
            .item(
                t("channelMenu.menu.notification.archiveThread"),
                coming_soon_modal(
                    t("channelMenu.menu.notification.archiveThread"),
                    locale_owned.clone(),
                ),
            )
            .item(
                t("channelMenu.menu.notification.muteThread"),
                coming_soon_toast(coming_soon.clone()),
            )
            .item(
                t("channelMenu.menu.notification.notification"),
                coming_soon_toast(coming_soon.clone()),
            )
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
        menu = menu
            .item(
                t("channelMenu.menu.notification.archiveChannel"),
                coming_soon_modal(
                    t("channelMenu.menu.notification.archiveChannel"),
                    locale_owned.clone(),
                ),
            )
            .item(
                t("channelMenu.menu.notification.muteChannel"),
                coming_soon_toast(coming_soon.clone()),
            )
            .item(
                t("channelMenu.menu.notification.notification"),
                coming_soon_toast(coming_soon.clone()),
            )
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
