use gpui::{App, ClipboardItem, SharedString, WeakEntity, Window};
use mezon_client::transport::QUICK_MENU_TYPE_QUICK;
use mezon_store::{
    AppConfig, BadgeService, ChannelPermissionsStore, DirectKind, DirectMessageStore, EmojiStore,
    Message, MessageCode, MessageId, MessagesStore, PERMISSION_DELETE_MESSAGE, PinnedMessagesStore,
    QuickMenuStore, ThreadsStore, TopicsStore,
};

use super::channel_messages::ChannelMessages;
use super::content::{first_link, open_message_link};
use super::forward_modal::ForwardMessageModal;
use super::pin_confirm_modals::ConfirmPinMessageModal;
use super::report_modal::ReportMessageModal;
use crate::app::shell::Shell;
use crate::components::primitives::{ContextMenu, IconName};

pub(crate) fn resolve_forward_group_in(
    messages: &[Message],
    message_id: MessageId,
    sender_id: &str,
) -> Vec<MessageId> {
    let Some(start) = messages.iter().position(|m| m.id == message_id) else {
        return vec![message_id];
    };
    let mut ids = vec![message_id];
    for m in &messages[start + 1..] {
        if m.combined_with_prev && m.sender_id.as_str() == sender_id {
            ids.push(m.id);
        } else {
            break;
        }
    }
    ids
}

pub(crate) fn resolve_forward_group(
    message_id: MessageId,
    sender_id: &str,
    cx: &App,
) -> Vec<MessageId> {
    let store = MessagesStore::global(cx);
    let store = store.read(cx);
    resolve_forward_group_in(store.messages(), message_id, sender_id)
}

fn append_quick_menus(
    menu: ContextMenu,
    message_id: MessageId,
    locale: &str,
    quick_menu_submenu_open: bool,
    host: WeakEntity<ChannelMessages>,
    cx: &App,
) -> ContextMenu {
    let Some(channel_id) = MessagesStore::global(cx).read(cx).active_channel_id() else {
        return menu;
    };
    let menu_names: Vec<_> = QuickMenuStore::global(cx)
        .read(cx)
        .items(channel_id, QUICK_MENU_TYPE_QUICK)
        .iter()
        .map(|item| item.menu_name.clone())
        .collect();
    if menu_names.is_empty() {
        return menu;
    }
    let options: Vec<crate::components::primitives::SubmenuOption> = menu_names
        .iter()
        .enumerate()
        .map(
            |(index, label)| crate::components::primitives::SubmenuOption {
                value: index as i32,
                label: label.clone(),
                selected: false,
                disabled: false,
            },
        )
        .collect();
    let label: SharedString = mezon_i18n::t(locale, "contextMenu.quickMenus").into();
    let host_open = host.clone();
    menu.submenu(
        label,
        None,
        options,
        quick_menu_submenu_open,
        move |_window, cx| {
            if let Some(view) = host_open.upgrade() {
                view.update(cx, |this, cx| this.set_quick_menu_submenu_open(true, cx));
            }
        },
        move |index, _window, cx| {
            let Some(name) = menu_names.get(index as usize) else {
                return;
            };
            MessagesStore::global(cx).update(cx, |store, cx| {
                store.execute_quick_menu(name.as_ref(), message_id, cx);
            });
        },
    )
}

fn close_quick_menu_submenu(
    host: WeakEntity<ChannelMessages>,
) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        if let Some(view) = host.upgrade() {
            view.update(cx, |this, cx| this.set_quick_menu_submenu_open(false, cx));
        }
    }
}

fn is_first_topic_message(message_id: MessageId, cx: &App) -> bool {
    let topics = TopicsStore::global(cx).read(cx);
    topics
        .origin_message()
        .is_some_and(|origin| origin.id == message_id)
}

fn channel_delete_blocked(msg: &Message, cx: &App) -> bool {
    if msg.topic_id.is_some() {
        return true;
    }
    TopicsStore::global(cx)
        .read(cx)
        .is_init_topic_message(msg.id)
}

fn sender_allows_give_coffee(msg: &Message, current_user_id: &str, cx: &App) -> bool {
    if current_user_id == msg.sender_id.as_str() {
        return false;
    }
    if msg.sender_id.is_empty() || msg.sender_id.as_str() == "0" {
        return false;
    }
    if let Some(config) = AppConfig::try_global(cx)
        && !config.anonymous_user_id.is_empty()
        && msg.sender_id == config.anonymous_user_id
    {
        return false;
    }
    true
}

pub(crate) fn delete_message_allowed(
    is_my_message: bool,
    is_group_dm: bool,
    is_owner_group_dm: bool,
    is_channel_or_thread_or_topic: bool,
    has_delete_permission: bool,
) -> bool {
    if is_my_message {
        return true;
    }
    if is_group_dm {
        return is_owner_group_dm;
    }
    is_channel_or_thread_or_topic && has_delete_permission
}

fn active_group_dm_owner(cx: &App) -> (bool, bool) {
    let Some(channel_id) = MessagesStore::global(cx).read(cx).active_channel_id() else {
        return (false, false);
    };
    let Some(store) = DirectMessageStore::try_global(cx) else {
        return (false, false);
    };
    let Some(direct) = store.read(cx).find(channel_id) else {
        return (false, false);
    };
    if direct.kind != DirectKind::Group {
        return (false, false);
    }
    let me = BadgeService::global(cx).read(cx).current_user_id(cx);
    (true, me.is_some() && direct.creator_id == me)
}

pub(crate) fn edit_message_allowed(
    is_my_message: bool,
    code: MessageCode,
    is_forwarded: bool,
    send_failed: bool,
) -> bool {
    is_my_message
        && !send_failed
        && !is_forwarded
        && code != MessageCode::SendToken
        && code != MessageCode::Poll
        && code.is_user_timeline()
}

pub(crate) fn message_is_editable(msg: &Message, current_user_id: &str) -> bool {
    edit_message_allowed(
        current_user_id == msg.sender_id.as_str(),
        msg.code,
        msg.is_forwarded,
        msg.send_failed,
    )
}

fn can_delete_message(msg: &Message, current_user_id: &str, is_topic_box: bool, cx: &App) -> bool {
    if is_topic_box {
        if is_first_topic_message(msg.id, cx) {
            return false;
        }
    } else if channel_delete_blocked(msg, cx) {
        return false;
    }
    let (is_group_dm, is_owner_group_dm) = active_group_dm_owner(cx);
    let messages = MessagesStore::global(cx).read(cx);
    let is_channel_or_thread_or_topic = is_topic_box || !messages.is_dm();
    let has_delete_permission = match (messages.active_channel_id(), messages.active_clan_id()) {
        (Some(channel_id), Some(clan_id)) => ChannelPermissionsStore::global(cx)
            .read(cx)
            .has_permission(PERMISSION_DELETE_MESSAGE, clan_id, channel_id),
        _ => false,
    };
    delete_message_allowed(
        current_user_id == msg.sender_id.as_str(),
        is_group_dm,
        is_owner_group_dm,
        is_channel_or_thread_or_topic,
        has_delete_permission,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    msg: &Message,
    current_user_id: &str,
    can_send_message: bool,
    locale: &str,
    show_forward_all: bool,
    is_topic_box: bool,
    reaction_submenu_open: bool,
    quick_menu_submenu_open: bool,
    selected_text: Option<String>,
    host: WeakEntity<ChannelMessages>,
    cx: &App,
) -> ContextMenu {
    if msg.send_failed {
        return build_failed_menu(msg, locale, host, cx);
    }
    if is_topic_box {
        return build_topic_menu(
            msg,
            current_user_id,
            locale,
            show_forward_all,
            reaction_submenu_open,
            quick_menu_submenu_open,
            selected_text,
            host,
            cx,
        );
    }
    build_channel_menu(
        msg,
        current_user_id,
        can_send_message,
        locale,
        show_forward_all,
        reaction_submenu_open,
        quick_menu_submenu_open,
        selected_text,
        host,
        cx,
    )
}

fn build_failed_menu(
    msg: &Message,
    locale: &str,
    host: WeakEntity<ChannelMessages>,
    _cx: &App,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key);
    let dismiss = {
        let host = host.clone();
        move |_window: &mut Window, cx: &mut App| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.close_context_menu(cx));
            }
        }
    };
    let message_id = msg.id;
    let locale_owned = locale.to_string();
    ContextMenu::new()
        .on_dismiss(dismiss)
        .item_trailing_icon(
            SharedString::from(t("contextMenu.resendMessage")),
            IconName::ResendMessageRightClick,
            move |_, cx| {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.resend_message(message_id, cx);
                });
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.info(
                        SharedString::from(mezon_i18n::t(
                            &locale_owned,
                            "contextMenu.messageResent",
                        )),
                        cx,
                    );
                });
            },
        )
        .danger_item_trailing_icon(
            SharedString::from(t("contextMenu.deleteMessage")),
            IconName::DeleteMessageRightClick,
            move |_, cx| {
                MessagesStore::global(cx)
                    .update(cx, |store, cx| store.remove_failed_message(message_id, cx));
            },
        )
}

#[allow(clippy::too_many_arguments)]
fn menu_with_reactions(
    dismiss: impl Fn(&mut Window, &mut App) + 'static,
    message_id: MessageId,
    reaction_submenu_open: bool,
    add_reaction_label: SharedString,
    view_more_label: SharedString,
    quick_emojis: Vec<(String, String)>,
    host: WeakEntity<ChannelMessages>,
) -> ContextMenu {
    let host_open = host.clone();
    let host_close = host.clone();
    let host_view_more = host;
    ContextMenu::new()
        .on_dismiss(dismiss)
        .quick_reactions(
            quick_emojis.clone(),
            move |emoji_id, shortname, _window, cx| {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.add_reaction(message_id, emoji_id, shortname, cx);
                });
            },
        )
        .on_reaction_close(move |_window, cx| {
            if let Some(view) = host_close.upgrade() {
                view.update(cx, |this, cx| this.set_reaction_submenu_open(false, cx));
            }
        })
        .reaction_submenu(
            add_reaction_label,
            view_more_label,
            quick_emojis,
            reaction_submenu_open,
            move |_window, cx| {
                if let Some(view) = host_open.upgrade() {
                    view.update(cx, |this, cx| this.set_reaction_submenu_open(true, cx));
                }
            },
            move |emoji_id, shortname, _window, cx| {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.add_reaction(message_id, emoji_id, shortname, cx);
                });
            },
            move |window, cx| {
                let position = window.mouse_position();
                let host = host_view_more.clone();
                window.defer(cx, move |window, cx| {
                    if let Some(view) = host.upgrade() {
                        view.update(cx, |this, cx| {
                            this.open_reaction_picker(message_id, position, window, cx);
                        });
                    }
                });
            },
        )
}

#[allow(clippy::too_many_arguments)]
fn build_topic_menu(
    msg: &Message,
    current_user_id: &str,
    locale: &str,
    show_forward_all: bool,
    reaction_submenu_open: bool,
    quick_menu_submenu_open: bool,
    selected_text: Option<String>,
    host: WeakEntity<ChannelMessages>,
    cx: &App,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key);
    let is_own_message = current_user_id == msg.sender_id.as_str();
    let is_poll = msg.code == MessageCode::Poll;

    let dismiss = {
        let host = host.clone();
        move |_window: &mut Window, cx: &mut App| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.close_context_menu(cx));
            }
        }
    };

    let quick_emojis = EmojiStore::global(cx)
        .read(cx)
        .recent(4)
        .into_iter()
        .map(|emoji| (emoji.id.clone(), emoji.shortname.clone()))
        .collect::<Vec<_>>();

    let mut menu = menu_with_reactions(
        dismiss,
        msg.id,
        reaction_submenu_open,
        t("contextMenu.addReaction").into(),
        t("contextMenu.viewMore").into(),
        quick_emojis,
        host.clone(),
    );

    if sender_allows_give_coffee(msg, current_user_id, cx) {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.giveACoffee"),
            IconName::DollarIconRightClick,
            move |_, cx| {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.give_coffee_reaction(message_id, cx);
                });
            },
        );
    }

    let show_edit = message_is_editable(msg, current_user_id);
    if show_edit {
        let host = host.clone();
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.editMessage"),
            IconName::PenEdit,
            move |window, cx| {
                let _ = host.update(cx, |this, cx| {
                    this.begin_edit(message_id, window, cx);
                });
            },
        );
    }

    menu = menu.separator();

    {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.reply"),
            IconName::ReplyRightClick,
            move |_, cx| {
                TopicsStore::global(cx).update(cx, |store, cx| store.set_reply_to(message_id, cx));
            },
        );
    }

    if !is_poll {
        let locale_owned = locale.to_string();
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.forwardMessage"),
            IconName::ForwardRightClick,
            move |window, cx| {
                ForwardMessageModal::open(
                    vec![message_id],
                    locale_owned.clone().into(),
                    window,
                    cx,
                );
            },
        );
    }

    if show_forward_all && !is_poll {
        let locale_owned = locale.to_string();
        let message_id = msg.id;
        let sender_id = msg.sender_id.clone();
        menu = menu.item_trailing_icon(
            t("contextMenu.forwardAllMessage"),
            IconName::ForwardAllRightClick,
            move |window, cx| {
                let ids = ChannelMessages::resolve_topic_forward_group(message_id, &sender_id, cx);
                ForwardMessageModal::open(ids, locale_owned.clone().into(), window, cx);
            },
        );
    }

    if !msg.content.is_empty() && !is_poll {
        let content = msg.content.clone();
        menu = menu.separator().item_trailing_icon(
            t("contextMenu.copyText"),
            IconName::CopyTextRightClick,
            move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(content.clone()));
            },
        );
    }

    if let Some(selected) = selected_text.filter(|text| !text.is_empty()) {
        menu = menu.item_trailing_icon(
            t("contextMenu.copyTextSelected"),
            IconName::CopyTextRightClick,
            move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(selected.clone()));
            },
        );
    }

    {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.addToInbox"),
            IconName::AddToInboxIcon,
            move |_window, cx| {
                MessagesStore::global(cx)
                    .update(cx, |store, cx| store.add_to_inbox(message_id, cx));
            },
        );
    }

    menu = append_quick_menus(
        menu,
        msg.id,
        locale,
        quick_menu_submenu_open,
        host.clone(),
        cx,
    );

    if !is_own_message {
        let message_id = msg.id;
        let locale_owned = locale.to_string();
        menu = menu.danger_item_trailing_icon(
            t("contextMenu.reportMessage"),
            IconName::ReportMessageRightClick,
            move |window, cx| {
                ReportMessageModal::open(message_id, locale_owned.clone().into(), window, cx);
            },
        );
    }

    if can_delete_message(msg, current_user_id, true, cx) {
        let message_id = msg.id;
        let locale_owned = locale.to_string();
        menu = menu.separator().danger_item_trailing_icon(
            t("contextMenu.deleteMessage"),
            IconName::DeleteMessageRightClick,
            move |window, cx| {
                let locale = locale_owned.clone();
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.confirm_delete_message(message_id, &locale, window, cx);
                });
            },
        );
    }

    menu.on_submenu_close(close_quick_menu_submenu(host))
}

#[allow(clippy::too_many_arguments)]
fn build_channel_menu(
    msg: &Message,
    current_user_id: &str,
    can_send_message: bool,
    locale: &str,
    show_forward_all: bool,
    reaction_submenu_open: bool,
    quick_menu_submenu_open: bool,
    selected_text: Option<String>,
    host: WeakEntity<ChannelMessages>,
    cx: &App,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key);
    let is_own_message = current_user_id == msg.sender_id.as_str();
    let is_poll = msg.code == MessageCode::Poll;
    let is_pinned = PinnedMessagesStore::global(cx)
        .read(cx)
        .is_pinned(&msg.id.to_string());
    let can_create_thread = !is_poll && ThreadsStore::global(cx).read(cx).can_create_thread(cx);

    let dismiss = {
        let host = host.clone();
        move |_window: &mut Window, cx: &mut App| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.close_context_menu(cx));
            }
        }
    };

    let quick_emojis = EmojiStore::global(cx)
        .read(cx)
        .recent(4)
        .into_iter()
        .map(|emoji| (emoji.id.clone(), emoji.shortname.clone()))
        .collect::<Vec<_>>();

    let mut menu = menu_with_reactions(
        dismiss,
        msg.id,
        reaction_submenu_open,
        t("contextMenu.addReaction").into(),
        t("contextMenu.viewMore").into(),
        quick_emojis,
        host.clone(),
    );

    if !is_own_message && sender_allows_give_coffee(msg, current_user_id, cx) {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.giveACoffee"),
            IconName::DollarIconRightClick,
            move |_, cx| {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.give_coffee_reaction(message_id, cx);
                });
            },
        );
    }

    let show_edit = message_is_editable(msg, current_user_id);
    if show_edit {
        let host = host.clone();
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.editMessage"),
            IconName::PenEdit,
            move |window, cx| {
                let _ = host.update(cx, |this, cx| {
                    this.begin_edit(message_id, window, cx);
                });
            },
        );
    }

    if is_pinned {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.unpinMessage"),
            IconName::PinMessageRightClick,
            move |_window, cx| {
                let message_id_str = message_id.to_string();
                if let Some(pin_id) = PinnedMessagesStore::global(cx)
                    .read(cx)
                    .pinned()
                    .iter()
                    .find(|p| p.message_id == message_id_str)
                    .map(|p| p.id.clone())
                {
                    PinnedMessagesStore::global(cx)
                        .update(cx, |store, cx| store.unpin(&pin_id, &message_id_str, cx));
                }
            },
        );
    }

    menu = menu.separator();

    if can_send_message || MessagesStore::global(cx).read(cx).is_dm() {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.reply"),
            IconName::ReplyRightClick,
            move |_, cx| {
                MessagesStore::global(cx)
                    .update(cx, |store, cx| store.set_reply_to(message_id, cx));
            },
        );
    }

    if !is_poll {
        let locale_owned = locale.to_string();
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.forwardMessage"),
            IconName::ForwardRightClick,
            move |window, cx| {
                ForwardMessageModal::open(
                    vec![message_id],
                    locale_owned.clone().into(),
                    window,
                    cx,
                );
            },
        );
    }

    if show_forward_all {
        let locale_owned = locale.to_string();
        let message_id = msg.id;
        let sender_id = msg.sender_id.clone();
        menu = menu.item_trailing_icon(
            t("contextMenu.forwardAllMessage"),
            IconName::ForwardAllRightClick,
            move |window, cx| {
                let ids = resolve_forward_group(message_id, &sender_id, cx);
                ForwardMessageModal::open(ids, locale_owned.clone().into(), window, cx);
            },
        );
    }

    if can_create_thread {
        menu = menu.item_trailing_icon(
            t("contextMenu.createThread"),
            IconName::ThreadIcon,
            move |_window, cx| {
                ThreadsStore::global(cx).update(cx, |store, cx| store.start_create(cx));
            },
        );
    }

    if !msg.content.is_empty() && !is_poll {
        let content = msg.content.clone();
        menu = menu.item_trailing_icon(
            t("contextMenu.copyText"),
            IconName::CopyTextRightClick,
            move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(content.clone()));
            },
        );
    }

    if let Some(selected) = selected_text.filter(|text| !text.is_empty()) {
        menu = menu.item_trailing_icon(
            t("contextMenu.copyTextSelected"),
            IconName::CopyTextRightClick,
            move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(selected.clone()));
            },
        );
    }

    if !is_pinned {
        let message_id = msg.id;
        let locale_owned = locale.to_string();
        menu = menu.item_trailing_icon(
            t("contextMenu.pinMessage"),
            IconName::PinMessageRightClick,
            move |window, cx| {
                ConfirmPinMessageModal::open(message_id, locale_owned.clone().into(), window, cx);
            },
        );
    }

    if is_poll && is_own_message {
        let message_id = msg.id;
        let poll_id = msg.poll.as_ref().map(|p| p.poll_id).unwrap_or(0);
        menu = menu.item_trailing_icon(
            t("contextMenu.endPollNow"),
            IconName::EndPollNowIcon,
            move |_window, cx| {
                mezon_store::MessagesStore::global(cx)
                    .update(cx, |store, cx| store.close_poll(poll_id, message_id, cx));
            },
        );
    }

    if TopicsStore::can_create_topic(cx) && TopicsStore::message_allows_topic_discussion(msg) {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.topicDiscussion"),
            IconName::TopicIcon,
            move |_window, cx| {
                TopicsStore::global(cx).update(cx, |store, cx| {
                    store.start_create_for_message(message_id, cx)
                });
            },
        );
    }

    menu = menu.separator();

    {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.markUnread"),
            IconName::MarkUnreadIcon,
            move |_window, cx| {
                MessagesStore::global(cx).update(cx, |store, cx| store.mark_unread(message_id, cx));
            },
        );
    }
    {
        let message_id = msg.id;
        menu = menu.item_trailing_icon(
            t("contextMenu.addToInbox"),
            IconName::AddToInboxIcon,
            move |_window, cx| {
                MessagesStore::global(cx)
                    .update(cx, |store, cx| store.add_to_inbox(message_id, cx));
            },
        );
    }
    menu = append_quick_menus(
        menu,
        msg.id,
        locale,
        quick_menu_submenu_open,
        host.clone(),
        cx,
    );

    let link = first_link(msg);
    let image = msg
        .attachments
        .iter()
        .find(|a| a.is_image())
        .map(|a| (a.url.clone(), a.filename.clone()));
    if link.is_some() || image.is_some() || !is_own_message {
        menu = menu.separator();
    }
    if let Some(link) = link {
        let link_for_copy = link.clone();
        menu = menu.item(t("contextMenu.copyLink"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(link_for_copy.clone()));
        });
        menu = menu.item(t("contextMenu.openLink"), move |_, cx| {
            open_message_link(link.clone(), cx);
        });
    }
    if !is_own_message {
        let message_id = msg.id;
        let locale_owned = locale.to_string();
        menu = menu.danger_item_trailing_icon(
            t("contextMenu.reportMessage"),
            IconName::ReportMessageRightClick,
            move |window, cx| {
                ReportMessageModal::open(message_id, locale_owned.clone().into(), window, cx);
            },
        );
    }
    if let Some((image_url, image_name)) = image {
        let url_for_copy = image_url.clone();
        let locale_for_copy = locale.to_string();
        menu = menu.item(t("contextMenu.copyImage"), move |_, cx| {
            let locale = locale_for_copy.clone();
            mezon_store::copy_image_url_to_clipboard(
                SharedString::from(url_for_copy.clone()),
                move |success, cx| {
                    let key = if success {
                        "contextMenu.imageCopiedToClipboard"
                    } else {
                        "contextMenu.errors.failedToCopyImage"
                    };
                    let message = mezon_i18n::t(&locale, key).to_string();
                    Shell::global(cx).update(cx, |shell, cx| {
                        if success {
                            shell.success(message, cx);
                        } else {
                            shell.error(message, cx);
                        }
                    });
                },
                cx,
            );
        });
        menu = menu.item(t("contextMenu.saveImage"), move |_, cx| {
            crate::util::download::save_with_progress_toast(
                SharedString::from(image_url.clone()),
                SharedString::from(image_name.clone()),
                cx,
            );
        });
    }

    if can_delete_message(msg, current_user_id, false, cx) {
        let message_id = msg.id;
        let locale_owned = locale.to_string();
        menu = menu.separator().danger_item_trailing_icon(
            t("contextMenu.deleteMessage"),
            IconName::DeleteMessageRightClick,
            move |window, cx| {
                let locale = locale_owned.clone();
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.confirm_delete_message(message_id, &locale, window, cx);
                });
            },
        );
    }

    menu.on_submenu_close(close_quick_menu_submenu(host))
}

#[cfg(test)]
mod delete_permission_tests {
    use super::delete_message_allowed;

    #[test]
    fn own_message_is_always_deletable() {
        assert!(delete_message_allowed(true, false, false, false, false));
        assert!(delete_message_allowed(true, true, false, false, false));
    }

    #[test]
    fn group_dm_defers_to_group_owner() {
        assert!(delete_message_allowed(false, true, true, false, false));
        assert!(!delete_message_allowed(false, true, false, false, true));
    }

    #[test]
    fn dm_never_allows_deleting_other_messages() {
        assert!(!delete_message_allowed(false, false, false, false, true));
    }

    #[test]
    fn channel_defers_to_overridden_permission() {
        assert!(delete_message_allowed(false, false, false, true, true));
        assert!(!delete_message_allowed(false, false, false, true, false));
    }

    #[test]
    fn clan_owner_has_no_bypass() {
        assert!(!delete_message_allowed(false, false, true, true, false));
    }
}

#[cfg(test)]
mod edit_permission_tests {
    use super::edit_message_allowed;
    use mezon_store::MessageCode;

    #[test]
    fn edit_allowed_only_for_own_plain_message() {
        assert!(edit_message_allowed(true, MessageCode::Chat, false, false));
        assert!(!edit_message_allowed(
            false,
            MessageCode::Chat,
            false,
            false
        ));
    }

    #[test]
    fn edit_rejects_message_kinds_without_editable_content() {
        for code in [
            MessageCode::SendToken,
            MessageCode::Poll,
            MessageCode::Welcome,
            MessageCode::AuditLog,
            MessageCode::CreateThread,
            MessageCode::Indicator,
        ] {
            assert!(
                !edit_message_allowed(true, code, false, false),
                "{code:?} must not be editable"
            );
        }
    }

    #[test]
    fn edit_rejects_forwarded_and_unsent_messages() {
        assert!(!edit_message_allowed(true, MessageCode::Chat, true, false));
        assert!(!edit_message_allowed(true, MessageCode::Chat, false, true));
    }
}
