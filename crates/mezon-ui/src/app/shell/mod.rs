//! App shell: a per-window aggregate (cf. Zed's `Workspace`) owning the cross-cutting overlay
//! layers — toasts and the modal stack — rendered on top of everything by `RootView`.
//!
//! Any view can surface a toast or a modal from anywhere via [`Shell::global`], instead of each
//! page wiring its own local toast/dialog state.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyView, App, AppContext, Context, Entity, Global, MouseButton, SharedString, Task, Window,
    deferred, div, hsla, prelude::*, px,
};

use crate::components::primitives::{InputState, Toast, ToastKind};
use crate::router::Route;

mod confirm_archive_channel_modal;
mod confirm_delete_account_modal;
mod confirm_delete_canvas_modal;
mod confirm_delete_category_modal;
mod confirm_delete_channel_modal;
mod confirm_delete_clan_modal;
mod confirm_delete_emoji_modal;
mod confirm_delete_message_modal;
mod confirm_delete_quick_menu_modal;
mod confirm_delete_role_modal;
mod confirm_delete_sound_modal;
mod confirm_delete_sticker_modal;
mod confirm_delete_thread_modal;
mod confirm_delete_webhook_modal;
mod confirm_destructive_modal;
mod confirm_kick_member_modal;
mod confirm_leave_clan_modal;
mod confirm_leave_thread_modal;
mod confirm_remove_friend_modal;
mod confirm_remove_group_member_modal;
mod disable_clan_community_modal;
mod transfer_owner_modal;
mod upload_limit_modal;
mod wallet_not_available_modal;
use confirm_archive_channel_modal::ConfirmArchiveChannelModal;
use confirm_delete_account_modal::ConfirmDeleteAccountModal;
use confirm_delete_canvas_modal::ConfirmDeleteCanvasModal;
use confirm_delete_category_modal::ConfirmDeleteCategoryModal;
use confirm_delete_channel_modal::ConfirmDeleteChannelModal;
use confirm_delete_clan_modal::ConfirmDeleteClanModal;
use confirm_delete_emoji_modal::ConfirmDeleteEmojiModal;
use confirm_delete_message_modal::ConfirmDeleteMessageModal;
use confirm_delete_quick_menu_modal::ConfirmDeleteQuickMenuModal;
use confirm_delete_role_modal::ConfirmDeleteRoleModal;
use confirm_delete_sound_modal::ConfirmDeleteSoundModal;
use confirm_delete_sticker_modal::ConfirmDeleteStickerModal;
use confirm_delete_thread_modal::ConfirmDeleteThreadModal;
use confirm_delete_webhook_modal::{ConfirmDeleteWebhookModal, WebhookDeleteTarget};
use confirm_destructive_modal::{ConfirmDestructive, ConfirmDestructiveModal};
use confirm_kick_member_modal::ConfirmKickMemberModal;
use confirm_leave_clan_modal::ConfirmLeaveClanModal;
use confirm_leave_thread_modal::ConfirmLeaveThreadModal;
pub use confirm_remove_friend_modal::FriendRemovalKind;
use confirm_remove_friend_modal::{ConfirmRemoveFriendModal, interpolate_username};
use confirm_remove_group_member_modal::ConfirmRemoveGroupMemberModal;
use disable_clan_community_modal::DisableClanCommunityModal;
use transfer_owner_modal::{TransferOwnerModal, TransferOwnerParty};
use upload_limit_modal::UploadLimitModal;
use wallet_not_available_modal::WalletNotAvailableModal;

const TOAST_TTL: Duration = Duration::from_secs(4);

struct ToastItem {
    id: usize,
    key: Option<SharedString>,
    message: SharedString,
    kind: ToastKind,
    progress: Option<f32>,
    /// How long the countdown bar has to drain; the bar animates itself, so nothing here ticks.
    countdown: Option<Duration>,
    _dismiss_task: Option<Task<()>>,
}

struct StackedModalHost {
    view: AnyView,
    focus_handle: gpui::FocusHandle,
}

impl Render for StackedModalHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.dismiss_modal(window, cx));
            }))
            .child(self.view.clone())
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_overlay(cx)
    }
}

/// Owns the window-level overlay layers (toasts + active modal). Registered as a [`Global`].
pub struct Shell {
    toasts: Vec<ToastItem>,
    modal: Option<AnyView>,
    modal_underlay: Option<(AnyView, bool, bool, Option<gpui::FocusHandle>)>,
    /// Focus saved by a stacked modal whose owner finished while it was the underlay.
    modal_restore_focus: Option<gpui::FocusHandle>,
    modal_fullscreen: bool,
    modal_backdrop_dismissible: bool,
    command_palette_open: bool,
    next_id: usize,
}

struct GlobalShell(Entity<Shell>);
impl Global for GlobalShell {}

impl Shell {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            toasts: Vec::new(),
            modal: None,
            modal_underlay: None,
            modal_restore_focus: None,
            modal_fullscreen: false,
            modal_backdrop_dismissible: true,
            command_palette_open: false,
            next_id: 0,
        });
        cx.set_global(GlobalShell(entity.clone()));
        entity
    }

    pub fn navigate_from_external_trigger(cx: &mut App, route: Route) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
        crate::router::navigate(cx, route);
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalShell>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalShell>().map(|shell| shell.0.clone())
    }

    /// Show a transient toast; it auto-dismisses after [`TOAST_TTL`].
    pub fn toast(
        &mut self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let ttl = TOAST_TTL;
        let dismiss_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ttl).await;
            this.update(cx, |this, cx| this.dismiss_by_id(id, cx)).ok();
        });
        self.toasts.push(ToastItem {
            id,
            key: None,
            message: message.into(),
            kind,
            progress: None,
            countdown: Some(ttl),
            _dismiss_task: Some(dismiss_task),
        });
        cx.notify();
    }

    /// Show a keyed toast that stays until something dismisses it. Used for conditions the user
    /// is still living with — being offline is not news that expires after four seconds.
    pub fn sticky(
        &mut self,
        key: impl Into<SharedString>,
        kind: ToastKind,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        if let Some(item) = self
            .toasts
            .iter_mut()
            .find(|t| t.key.as_ref() == Some(&key))
        {
            item.message = message.into();
            item.kind = kind;
            cx.notify();
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.toasts.push(ToastItem {
            id,
            key: Some(key),
            message: message.into(),
            kind,
            progress: None,
            countdown: None,
            _dismiss_task: None,
        });
        cx.notify();
    }

    /// Remove a keyed toast — the condition it reported is over.
    pub fn dismiss(&mut self, key: impl Into<SharedString>, cx: &mut Context<Self>) {
        let key = key.into();
        let before = self.toasts.len();
        self.toasts.retain(|t| t.key.as_ref() != Some(&key));
        if self.toasts.len() != before {
            cx.notify();
        }
    }

    fn dismiss_by_id(&mut self, id: usize, cx: &mut Context<Self>) {
        let before = self.toasts.len();
        self.toasts.retain(|t| t.id != id);
        if self.toasts.len() != before {
            cx.notify();
        }
    }

    /// Show or update a keyed progress toast. It does not auto-dismiss; call
    /// [`Shell::finish_toast`] with the same key when the operation completes.
    pub fn progress_toast(
        &mut self,
        key: impl Into<SharedString>,
        message: impl Into<SharedString>,
        progress: f32,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        let message = message.into();
        if let Some(item) = self
            .toasts
            .iter_mut()
            .find(|t| t.key.as_ref() == Some(&key))
        {
            item.message = message;
            item.progress = Some(progress.clamp(0., 1.));
        } else {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            self.toasts.push(ToastItem {
                id,
                key: Some(key),
                message,
                kind: ToastKind::Info,
                progress: Some(progress.clamp(0., 1.)),
                countdown: None,
                _dismiss_task: None,
            });
        }
        cx.notify();
    }

    /// Replace a keyed progress toast with a normal auto-dismissing toast.
    pub fn finish_toast(
        &mut self,
        key: impl Into<SharedString>,
        kind: ToastKind,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        self.toasts.retain(|t| t.key.as_ref() != Some(&key));
        self.toast(kind, message, cx);
    }

    pub fn info(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast(ToastKind::Info, message, cx);
    }

    pub fn success(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast(ToastKind::Success, message, cx);
    }

    pub fn error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast(ToastKind::Error, message, cx);
    }

    pub fn error_once(
        &mut self,
        key: impl Into<SharedString>,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        if self.toasts.iter().any(|t| t.key.as_ref() == Some(&key)) {
            return;
        }
        self.toast(ToastKind::Error, message, cx);
        if let Some(item) = self.toasts.last_mut() {
            item.key = Some(key);
        }
    }

    /// Show `view` as the active modal (backdrop click dismisses). The view renders its own card.
    pub fn show_modal(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.modal_underlay = None;
        self.modal_restore_focus = None;
        self.command_palette_open = false;
        self.modal_fullscreen = false;
        self.modal_backdrop_dismissible = true;
        self.modal = Some(view);
        cx.notify();
    }

    pub fn show_modal_keyboard_dismiss_only(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.show_modal(view, cx);
        self.modal_backdrop_dismissible = false;
    }

    pub fn modal_view_id(&self) -> Option<gpui::EntityId> {
        self.modal.as_ref().map(|modal| modal.entity_id())
    }

    /// Show `view` as a fullscreen modal (e.g. an image/media viewer): it renders its own
    /// full-viewport backdrop, so the overlay skips the centered card treatment and dim layer.
    pub fn show_fullscreen_modal(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.modal_underlay = None;
        self.modal_restore_focus = None;
        self.command_palette_open = false;
        self.modal_fullscreen = true;
        self.modal = Some(view);
        cx.notify();
    }

    pub fn show_stacked_modal(
        &mut self,
        view: AnyView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_focus = window.focused(cx);
        if let Some(current) = self.modal.take() {
            self.modal_underlay = Some((
                current,
                self.modal_fullscreen,
                self.command_palette_open,
                previous_focus,
            ));
        }
        let host = cx.new(|cx| StackedModalHost {
            view,
            focus_handle: cx.focus_handle(),
        });
        let focus_handle = host.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.command_palette_open = false;
        self.modal_fullscreen = false;
        self.modal = Some(host.into());
        cx.notify();
    }

    pub fn show_command_palette(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.modal_underlay = None;
        self.modal_restore_focus = None;
        self.command_palette_open = true;
        self.modal = Some(view);
        cx.notify();
    }

    pub fn command_palette_open(&self) -> bool {
        self.command_palette_open
    }

    /// Confirm-then-delete a message (mirrors React's `ModalDeleteMess`): shown when the
    /// user clears an inline edit to empty, or picks Delete from the message context menu.
    pub fn confirm_delete_message(
        &mut self,
        message_id: mezon_store::MessageId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title: SharedString = mezon_i18n::t(locale, "message.deleteMessageModal.title")
            .to_string()
            .into();
        let description: SharedString = mezon_i18n::t(
            locale,
            "message.deleteMessageModal.deleteMessageDescription",
        )
        .to_string()
        .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "message.deleteMessageModal.cancel")
            .to_string()
            .into();
        let delete_label: SharedString = mezon_i18n::t(locale, "message.deleteMessageModal.delete")
            .to_string()
            .into();
        let view = cx.new(|cx| ConfirmDeleteMessageModal {
            focus_handle: cx.focus_handle(),
            message_id,
            title,
            description,
            cancel_label,
            delete_label,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_leave_thread(
        &mut self,
        clan_id: mezon_store::ClanId,
        channel_id: mezon_store::ChannelId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title: SharedString =
            mezon_i18n::t(locale, "channelMenu.modalConFirmLeaveThread.title")
                .to_string()
                .into();
        let description: SharedString =
            mezon_i18n::t(locale, "channelMenu.modalConFirmLeaveThread.textConfirm")
                .to_string()
                .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "common.cancel").to_string().into();
        let leave_label: SharedString =
            mezon_i18n::t(locale, "channelMenu.modalConFirmLeaveThread.yesButton")
                .to_string()
                .into();
        let view = cx.new(|cx| ConfirmLeaveThreadModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            channel_id,
            title,
            description,
            cancel_label,
            leave_label,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_archive_channel(
        &mut self,
        clan_id: mezon_store::ClanId,
        channel_id: mezon_store::ChannelId,
        is_thread: bool,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (title_key, description_key, button_key) = if is_thread {
            (
                "channelMenu.modalConfirmArchiveThread.title",
                "channelMenu.modalConfirmArchiveThread.textConfirm",
                "channelMenu.modalConfirmArchiveThread.button",
            )
        } else {
            (
                "channelMenu.modalConfirmArchiveChannel.title",
                "channelMenu.modalConfirmArchiveChannel.textConfirm",
                "channelMenu.modalConfirmArchiveChannel.button",
            )
        };
        let title: SharedString = mezon_i18n::t(locale, title_key).to_string().into();
        let description: SharedString = mezon_i18n::t(locale, description_key).to_string().into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "common.cancel").to_string().into();
        let archive_label: SharedString = mezon_i18n::t(locale, button_key).to_string().into();
        let parent_id = mezon_store::ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .and_then(|channel| channel.parent_id)
            .or_else(|| {
                mezon_store::ChannelSettingsStore::global(cx)
                    .read(cx)
                    .row_by_id(clan_id, channel_id)
                    .map(|row| row.parent_id)
            })
            .unwrap_or(mezon_store::ChannelId(0));
        let view = cx.new(|cx| ConfirmArchiveChannelModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            channel_id,
            parent_id,
            is_thread,
            locale: locale.to_string(),
            title,
            description,
            cancel_label,
            archive_label,
            submitting: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_thread(
        &mut self,
        clan_id: mezon_store::ClanId,
        channel_id: mezon_store::ChannelId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (channel_name, parent_id) = mezon_store::ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .map(|channel| {
                (
                    channel.name.clone(),
                    channel.parent_id.unwrap_or(mezon_store::ChannelId(0)),
                )
            })
            .or_else(|| {
                mezon_store::ChannelSettingsStore::global(cx)
                    .read(cx)
                    .row_by_id(clan_id, channel_id)
                    .map(|row| (row.label.clone(), row.parent_id))
            })
            .unwrap_or_else(|| ("Unknown Channel".to_string(), mezon_store::ChannelId(0)));
        let title: SharedString =
            mezon_i18n::t(locale, "channelSetting.confirm.deleteThread.title")
                .to_string()
                .into();
        let description: SharedString =
            mezon_i18n::t(locale, "channelSetting.confirm.deleteThread.content")
                .replace("{{channelName}}", &channel_name)
                .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "common.cancel").to_string().into();
        let delete_label: SharedString =
            mezon_i18n::t(locale, "channelSetting.confirm.deleteThread.confirmText")
                .to_string()
                .into();
        let view = cx.new(|cx| ConfirmDeleteThreadModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            channel_id,
            parent_id,
            locale: locale.to_string(),
            title,
            description,
            cancel_label,
            delete_label,
            submitting: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_channel(
        &mut self,
        clan_id: mezon_store::ClanId,
        channel_id: mezon_store::ChannelId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel_name = mezon_store::ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .map(|channel| channel.name.clone())
            .or_else(|| {
                mezon_store::ChannelSettingsStore::global(cx)
                    .read(cx)
                    .row_by_id(clan_id, channel_id)
                    .map(|row| row.label.clone())
            })
            .unwrap_or_else(|| "Unknown Channel".to_string());
        let title: SharedString =
            mezon_i18n::t(locale, "channelSetting.confirm.deleteChannel.title")
                .to_string()
                .into();
        let description: SharedString =
            mezon_i18n::t(locale, "channelSetting.confirm.deleteChannel.content")
                .replace("{{channelName}}", &channel_name)
                .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "common.cancel").to_string().into();
        let delete_label: SharedString =
            mezon_i18n::t(locale, "channelSetting.confirm.deleteChannel.confirmText")
                .to_string()
                .into();
        let view = cx.new(|cx| ConfirmDeleteChannelModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            channel_id,
            locale: locale.to_string(),
            title,
            description,
            cancel_label,
            delete_label,
            submitting: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_category(
        &mut self,
        clan_id: mezon_store::ClanId,
        category_id: String,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let category_name = mezon_store::ChannelList::global(cx)
            .read(cx)
            .category_name(clan_id, &category_id)
            .unwrap_or_default()
            .to_string();
        let title: SharedString = mezon_i18n::t(locale, "common.modalConfirm.deleteCategoryTitle")
            .replace("{{name}}", &category_name)
            .into();
        let description: SharedString =
            mezon_i18n::t(locale, "clan.categoryOverview.cannotBeUndone")
                .to_string()
                .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "common.cancel").to_string().into();
        let delete_label: SharedString =
            mezon_i18n::t(locale, "clan.categoryOverview.deleteCategoryButton")
                .to_string()
                .into();
        let view = cx.new(|cx| ConfirmDeleteCategoryModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            category_id,
            locale: locale.to_string(),
            title,
            description,
            cancel_label,
            delete_label,
            submitting: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_canvas(
        &mut self,
        canvas_id: String,
        canvas_title: String,
        clan_id: mezon_store::ClanId,
        channel_id: mezon_store::ChannelId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let display_title = if canvas_title.trim().is_empty() {
            mezon_i18n::t(locale, "common.canvas.untitled").to_string()
        } else {
            canvas_title
        };
        let title: SharedString = mezon_i18n::t(locale, "common.canvas.deleteTitle")
            .replace("{{name}}", &display_title)
            .into();
        let description: SharedString = mezon_i18n::t(locale, "common.canvas.deleteMessage")
            .to_string()
            .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "common.cancel").to_string().into();
        let delete_label: SharedString = mezon_i18n::t(locale, "message.deleteMessageModal.delete")
            .to_string()
            .into();
        let view = cx.new(|cx| ConfirmDeleteCanvasModal {
            focus_handle: cx.focus_handle(),
            canvas_id,
            clan_id,
            channel_id,
            title,
            description,
            cancel_label,
            delete_label,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_emoji(
        &mut self,
        clan_id: mezon_store::ClanId,
        emoji_id: SharedString,
        shortname: SharedString,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let description = mezon_i18n::t(locale, "clanEmojiSetting.deleteModal.description")
            .replace("{{shortname}}", shortname.as_ref());
        let view = cx.new(|cx| ConfirmDeleteEmojiModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            emoji_id,
            title: mezon_i18n::t(locale, "clanEmojiSetting.deleteModal.title")
                .to_string()
                .into(),
            description: description.into(),
            cancel_label: mezon_i18n::t(locale, "clanEmojiSetting.deleteModal.cancel")
                .to_string()
                .into(),
            delete_label: mezon_i18n::t(locale, "clanEmojiSetting.deleteModal.confirm")
                .to_string()
                .into(),
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_role(
        &mut self,
        clan_id: mezon_store::ClanId,
        role_id: mezon_store::RoleId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title: SharedString = mezon_i18n::t(locale, "confirmations.deleteRole.title")
            .to_string()
            .into();
        let description: SharedString = mezon_i18n::t(locale, "confirmations.deleteRole.message")
            .to_string()
            .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "confirmations.deleteRole.cancel")
            .to_string()
            .into();
        let delete_label: SharedString = mezon_i18n::t(locale, "confirmations.deleteRole.confirm")
            .to_string()
            .into();
        let view = cx.new(|cx| ConfirmDeleteRoleModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            role_id,
            title,
            description,
            cancel_label,
            delete_label,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_sticker(
        &mut self,
        clan_id: mezon_store::ClanId,
        sticker_id: SharedString,
        shortname: SharedString,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let description = mezon_i18n::t(locale, "clanStickerSetting.deleteModal.description")
            .replace("{{shortname}}", shortname.as_ref());
        let view = cx.new(|cx| ConfirmDeleteStickerModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            sticker_id,
            title: mezon_i18n::t(locale, "clanStickerSetting.deleteModal.title")
                .to_string()
                .into(),
            description: description.into(),
            cancel_label: mezon_i18n::t(locale, "clanStickerSetting.deleteModal.cancel")
                .to_string()
                .into(),
            delete_label: mezon_i18n::t(locale, "clanStickerSetting.deleteModal.confirm")
                .to_string()
                .into(),
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    fn confirm_delete_webhook(
        &mut self,
        target: WebhookDeleteTarget,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = target.webhook_name();
        let title: SharedString = mezon_i18n::t(locale, target.delete_title_key())
            .to_string()
            .into();
        let description: SharedString = mezon_i18n::t(
            locale,
            "clanIntegrationsSetting.webhooksEdit.deleteWebhookConfirmation",
        )
        .replace("{{webhookName}}", name)
        .into();
        let cancel_label: SharedString =
            mezon_i18n::t(locale, "clanIntegrationsSetting.webhooksEdit.cancel")
                .to_string()
                .into();
        let delete_label: SharedString =
            mezon_i18n::t(locale, "clanIntegrationsSetting.webhooksEdit.yes")
                .to_string()
                .into();
        let view = cx.new(|cx| ConfirmDeleteWebhookModal {
            focus_handle: cx.focus_handle(),
            target,
            locale: locale.to_string(),
            title,
            description,
            cancel_label,
            delete_label,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_quick_menu(
        &mut self,
        clan_id: mezon_store::ClanId,
        channel_id: mezon_store::ChannelId,
        item_id: i64,
        command_label: &str,
        is_flash: bool,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let type_name = if is_flash {
            mezon_i18n::t(locale, "channelSetting.quickAction.flashMessage")
        } else {
            mezon_i18n::t(locale, "channelSetting.quickAction.quickMenu")
        };
        let title: SharedString = format!(
            "{} {}",
            mezon_i18n::t(locale, "channelSetting.quickAction.delete"),
            type_name
        )
        .into();
        let description: SharedString =
            mezon_i18n::t(locale, "channelSetting.quickAction.deleteTitle")
                .replace("{{command}}", command_label)
                .into();
        let view = cx.new(|cx| ConfirmDeleteQuickMenuModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            channel_id,
            item_id,
            title,
            description,
            cancel_label: mezon_i18n::t(locale, "channelSetting.quickAction.cancel")
                .to_string()
                .into(),
            delete_label: mezon_i18n::t(locale, "channelSetting.quickAction.delete")
                .to_string()
                .into(),
            submitting: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_sound(
        &mut self,
        clan_id: mezon_store::ClanId,
        sound_id: SharedString,
        shortname: SharedString,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let description = mezon_i18n::t(locale, "clanSoundSetting.deleteModal.description")
            .replace("{{shortname}}", shortname.as_ref());
        let view = cx.new(|cx| ConfirmDeleteSoundModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            sound_id,
            title: mezon_i18n::t(locale, "clanSoundSetting.deleteModal.title")
                .to_string()
                .into(),
            description: description.into(),
            cancel_label: mezon_i18n::t(locale, "clanSoundSetting.deleteModal.cancel")
                .to_string()
                .into(),
            delete_label: mezon_i18n::t(locale, "clanSoundSetting.deleteModal.confirm")
                .to_string()
                .into(),
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_channel_webhook(
        &mut self,
        webhook: mezon_store::ChannelWebhook,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_delete_webhook(WebhookDeleteTarget::Channel(webhook), locale, window, cx);
    }

    pub fn confirm_delete_clan_webhook(
        &mut self,
        webhook: mezon_store::ClanWebhook,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_delete_webhook(WebhookDeleteTarget::Clan(webhook), locale, window, cx);
    }

    pub fn confirm_kick_member(
        &mut self,
        clan_id: mezon_store::ClanId,
        user_id: mezon_store::UserId,
        display_username: &str,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clan_name = mezon_store::ClanList::global(cx)
            .read(cx)
            .clan_by_id(clan_id)
            .map(|clan| clan.name.clone())
            .unwrap_or_default();
        let clan_name = if clan_name.is_empty() {
            "clan".to_string()
        } else {
            clan_name
        };
        let title: SharedString = mezon_i18n::t(locale, "modalControls.kickMember.title")
            .replace("{{username}}", display_username)
            .replace("{{clanName}}", &clan_name)
            .into();
        let description: SharedString =
            mezon_i18n::t(locale, "modalControls.kickMember.description")
                .replace("{{username}}", display_username)
                .replace("{{clanName}}", &clan_name)
                .into();
        let reason_label: SharedString =
            mezon_i18n::t(locale, "modalControls.kickMember.reasonLabel")
                .to_string()
                .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "modalControls.buttons.cancel")
            .to_string()
            .into();
        let confirm_label: SharedString = mezon_i18n::t(locale, "modalControls.buttons.kick")
            .to_string()
            .into();
        let success_message: SharedString = mezon_i18n::t(
            locale,
            "clanOverviewSetting.permissions.toast.kickMemberSuccess",
        )
        .to_string()
        .into();
        let error_message: SharedString = mezon_i18n::t(
            locale,
            "clanOverviewSetting.permissions.toast.kickMemberFailed",
        )
        .to_string()
        .into();
        let view = cx.new(|cx| {
            let reason_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .height(px(64.))
                    .text_size(px(14.))
            });
            ConfirmKickMemberModal {
                focus_handle: cx.focus_handle(),
                clan_id,
                user_id,
                title,
                description,
                reason_label,
                cancel_label,
                confirm_label,
                success_message,
                error_message,
                reason_input,
            }
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_transfer_ownership(
        &mut self,
        clan_id: mezon_store::ClanId,
        new_owner_id: mezon_store::UserId,
        new_owner_name: &str,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clan_name = mezon_store::ClanList::global(cx)
            .read(cx)
            .clan_by_id(clan_id)
            .map(|clan| clan.name.clone())
            .unwrap_or_default();
        let members = mezon_store::ClanMembersStore::global(cx);
        let party = |user_id: mezon_store::UserId, fallback_name: &str| {
            let known =
                members
                    .read(cx)
                    .member(clan_id, user_id)
                    .map(|member| TransferOwnerParty {
                        name: member.name().to_string().into(),
                        avatar: member.avatar().to_string().into(),
                    });
            match known {
                Some(party) if !party.name.is_empty() => party,
                Some(party) => TransferOwnerParty {
                    name: fallback_name.to_string().into(),
                    ..party
                },
                None => TransferOwnerParty {
                    name: fallback_name.to_string().into(),
                    avatar: SharedString::default(),
                },
            }
        };
        let Some(current_user_id) = mezon_store::BadgeService::try_global(cx)
            .and_then(|badges| badges.read(cx).current_user_id(cx))
        else {
            tracing::error!("transfer ownership of {clan_id}: no signed-in user");
            let message = mezon_i18n::t(
                locale,
                "clanOverviewSetting.permissions.toast.transferOwnershipFailed",
            )
            .to_string();
            self.error(message, cx);
            return;
        };
        let self_account = mezon_store::AccountStore::try_global(cx)
            .and_then(|store| store.read(cx).account.clone());
        let self_name = self_account
            .as_ref()
            .map(|account| {
                if account.display_name.is_empty() {
                    account.username.clone()
                } else {
                    account.display_name.clone()
                }
            })
            .unwrap_or_default();
        let mut current_owner = party(current_user_id, &self_name);
        if current_owner.avatar.is_empty()
            && let Some(avatar) = self_account.and_then(|account| account.avatar_url)
        {
            current_owner.avatar = avatar.into();
        }
        let new_owner = party(new_owner_id, new_owner_name);
        let title: SharedString = mezon_i18n::t(locale, "transferOwner.title")
            .to_string()
            .into();
        let description: SharedString = mezon_i18n::t(locale, "transferOwner.description")
            .replace("{{clanName}}", &clan_name)
            .replace("{{memberName}}", &new_owner.name)
            .into();
        let confirmation: SharedString = mezon_i18n::t(locale, "transferOwner.confirmation")
            .replace("{{memberName}}", &new_owner.name)
            .into();
        let transfer_label: SharedString = mezon_i18n::t(locale, "transferOwner.buttons.transfer")
            .to_string()
            .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "transferOwner.buttons.cancel")
            .to_string()
            .into();
        let success_message: SharedString = mezon_i18n::t(locale, "common.transferredSuccessfully")
            .to_string()
            .into();
        let error_message: SharedString = mezon_i18n::t(
            locale,
            "clanOverviewSetting.permissions.toast.transferOwnershipFailed",
        )
        .to_string()
        .into();
        let avatar_cache = crate::image_cache::shared_avatar_cache(cx);
        let view = cx.new(|cx| TransferOwnerModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            new_owner_id,
            title,
            description,
            confirmation,
            transfer_label,
            cancel_label,
            success_message,
            error_message,
            current_owner,
            new_owner,
            avatar_cache,
            acknowledged: false,
            pending: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_clan(
        &mut self,
        clan_id: mezon_store::ClanId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(clan_name) = mezon_store::ClanList::global(cx)
            .read(cx)
            .clan_by_id(clan_id)
            .map(|clan| clan.name.clone())
            .filter(|name| !name.is_empty())
        else {
            let message = mezon_i18n::t(locale, "deleteClan.deleteClanModal.error").to_string();
            self.error(message, cx);
            return;
        };
        let title: SharedString = mezon_i18n::t(locale, "clanSettings.deleteClanTitle")
            .replace("{{clanName}}", &clan_name)
            .into();
        let warning: SharedString = mezon_i18n::t(locale, "deleteClan.confirmMessage")
            .to_string()
            .into();
        let name_label: SharedString = mezon_i18n::t(locale, "deleteClan.enterClanName")
            .to_string()
            .into();
        let incorrect_name: SharedString = mezon_i18n::t(locale, "deleteClan.incorrectName")
            .to_string()
            .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "deleteClan.cancel")
            .to_string()
            .into();
        let confirm_label: SharedString = mezon_i18n::t(locale, "clanSettings.sidebar.deleteClan")
            .to_string()
            .into();
        let error_message: SharedString = mezon_i18n::t(locale, "deleteClan.deleteClanModal.error")
            .to_string()
            .into();
        let clan_name: SharedString = clan_name.into();
        let view = cx.new(|cx| {
            let name_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(clan_name.clone())
                    .text_size(px(14.))
            });
            ConfirmDeleteClanModal {
                focus_handle: cx.focus_handle(),
                clan_id,
                clan_name,
                title,
                warning,
                name_label,
                incorrect_name,
                cancel_label,
                confirm_label,
                error_message,
                _name_sub: ConfirmDeleteClanModal::watch_name(&name_input, cx),
                name_input,
                name_matches: None,
            }
        });
        let name_input = view.read(cx).name_input.clone();
        name_input.update(cx, |input, cx| input.focus(window, cx));
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_leave_clan(
        &mut self,
        clan_id: mezon_store::ClanId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clan_name = mezon_store::ClanList::global(cx)
            .read(cx)
            .clan_by_id(clan_id)
            .map(|clan| clan.name.clone())
            .unwrap_or_default();
        let title: SharedString = format!(
            "{} {}",
            mezon_i18n::t(locale, "contextMenu.leave"),
            clan_name
        )
        .trim_end()
        .to_string()
        .into();
        let description: SharedString = mezon_i18n::t(locale, "common.modalConfirm.defaultMessage")
            .to_string()
            .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "common.cancel").to_string().into();
        let confirm_label: SharedString = mezon_i18n::t(locale, "contextMenu.leaveClan")
            .to_string()
            .into();
        let error_message: SharedString = mezon_i18n::t(locale, "common.somethingWentWrong")
            .to_string()
            .into();
        let view = cx.new(|cx| ConfirmLeaveClanModal {
            focus_handle: cx.focus_handle(),
            clan_id,
            title,
            description,
            cancel_label,
            confirm_label,
            error_message,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_remove_friend(
        &mut self,
        friend_id: mezon_store::UserId,
        display_username: &str,
        kind: FriendRemovalKind,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let username = if display_username.is_empty() {
            mezon_i18n::t(locale, "friendsPage.friend")
        } else {
            display_username
        };
        let (title, _) = interpolate_username(mezon_i18n::t(locale, kind.title_key()), username);
        let (description, description_bold) =
            interpolate_username(mezon_i18n::t(locale, kind.description_key()), username);
        let cancel_label: SharedString =
            mezon_i18n::t(locale, "friendsPage.removeFriendModal.cancel")
                .to_string()
                .into();
        let confirm_label: SharedString =
            mezon_i18n::t(locale, kind.confirm_key()).to_string().into();
        let view = cx.new(|cx| ConfirmRemoveFriendModal {
            focus_handle: cx.focus_handle(),
            friend_id,
            title,
            description,
            description_bold,
            cancel_label,
            confirm_label,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn show_upload_limit(
        &mut self,
        title: impl Into<SharedString>,
        content: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| UploadLimitModal {
            focus_handle: cx.focus_handle(),
            title: title.into(),
            content: content.into(),
        });
        let previous_focus = window.focused(cx);
        if let Some(current) = self.modal.take() {
            self.modal_underlay = Some((
                current,
                self.modal_fullscreen,
                self.command_palette_open,
                previous_focus,
            ));
        }
        let host = cx.new(|cx| StackedModalHost {
            view: view.into(),
            focus_handle: cx.focus_handle(),
        });
        let focus_handle = host.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.command_palette_open = false;
        self.modal_fullscreen = false;
        self.modal = Some(host.into());
        cx.notify();
    }

    pub fn show_wallet_not_available(
        &mut self,
        message: impl Into<SharedString>,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        let title = if message.is_empty() {
            mezon_i18n::t(locale, "message.wallet.notAvailable").into()
        } else {
            message
        };
        let view = cx.new(|cx| {
            WalletNotAvailableModal::new(
                cx.focus_handle(),
                title,
                mezon_i18n::t(locale, "message.wallet.descNotAvailable").into(),
                mezon_i18n::t(locale, "message.wallet.enableWallet").into(),
                mezon_i18n::t(locale, "message.wallet.enabling").into(),
                mezon_i18n::t(locale, "message.wallet.enabled").into(),
                mezon_i18n::t(locale, "message.wallet.cancel").into(),
                cx,
            )
        });
        let previous_focus = window.focused(cx);
        if let Some(current) = self.modal.take() {
            self.modal_underlay = Some((
                current,
                self.modal_fullscreen,
                self.command_palette_open,
                previous_focus,
            ));
        }
        let host = cx.new(|cx| StackedModalHost {
            view: view.into(),
            focus_handle: cx.focus_handle(),
        });
        let focus_handle = host.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.command_palette_open = false;
        self.modal_fullscreen = false;
        self.modal = Some(host.into());
        cx.notify();
    }

    pub fn confirm_close_dm(
        &mut self,
        channel_id: mezon_store::ChannelId,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_destructive(
            ConfirmDestructive {
                id: "confirm-close-dm",
                title: mezon_i18n::t(locale, "dmMessage.closeDmConfirm.title").into(),
                description: mezon_i18n::t(locale, "dmMessage.closeDmConfirm.content").into(),
                cancel_label: mezon_i18n::t(locale, "common.cancel").into(),
                confirm_label: mezon_i18n::t(locale, "dmMessage.closeDmConfirm.confirmText").into(),
                failed_message: mezon_i18n::t(locale, "dmMessage.closeDmConfirm.error").into(),
                action: Rc::new(move |cx: &mut App| {
                    mezon_store::DirectMessageStore::global(cx)
                        .update(cx, |store, cx| store.close_conversation(channel_id, cx))
                }),
            },
            window,
            cx,
        );
    }

    fn confirm_destructive(
        &mut self,
        params: ConfirmDestructive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ConfirmDestructive {
            id,
            title,
            description,
            cancel_label,
            confirm_label,
            failed_message,
            action,
        } = params;
        let view = cx.new(|cx| ConfirmDestructiveModal {
            focus_handle: cx.focus_handle(),
            cancel_id: SharedString::from(format!("{id}-cancel")),
            confirm_id: SharedString::from(format!("{id}-confirm")),
            title,
            description,
            cancel_label,
            confirm_label,
            failed_message,
            action,
            running: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_leave_dm_group(
        &mut self,
        channel_id: mezon_store::ChannelId,
        group_name: &str,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let description =
            mezon_i18n::t(locale, "leaveGroup.confirmMessage").replace("{{groupName}}", group_name);
        self.confirm_destructive(
            ConfirmDestructive {
                id: "confirm-leave-dm-group",
                title: mezon_i18n::t(locale, "leaveGroup.title")
                    .replace("{{groupName}}", group_name)
                    .into(),
                description: description.into(),
                cancel_label: mezon_i18n::t(locale, "leaveGroup.cancel").into(),
                confirm_label: mezon_i18n::t(locale, "leaveGroup.leaveGroup").into(),
                failed_message: mezon_i18n::t(locale, "common.somethingWentWrong").into(),
                action: Rc::new(move |cx: &mut App| {
                    mezon_store::DirectMessageStore::global(cx)
                        .update(cx, |store, cx| store.leave_group(channel_id, cx))
                }),
            },
            window,
            cx,
        );
    }

    pub fn confirm_remove_group_member(
        &mut self,
        channel_id: mezon_store::ChannelId,
        user_id: mezon_store::UserId,
        display_name: &str,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let description = mezon_i18n::t(locale, "directMessage.removeFromGroup.description")
            .replace("{{username}}", display_name);
        let view = cx.new(|cx| ConfirmRemoveGroupMemberModal {
            focus_handle: cx.focus_handle(),
            channel_id,
            user_id,
            title: mezon_i18n::t(locale, "directMessage.contextMenu.removeFromGroup").into(),
            description: description.into(),
            cancel_label: mezon_i18n::t(locale, "common.cancel").into(),
            confirm_label: mezon_i18n::t(locale, "common.confirm").into(),
            success_message: mezon_i18n::t(locale, "userProfile.userInfoDM.menu.removeSuccess")
                .into(),
            failed_message: mezon_i18n::t(locale, "userProfile.userInfoDM.menu.removeFailed")
                .into(),
            removing: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_delete_account(
        &mut self,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| ConfirmDeleteAccountModal {
            focus_handle: cx.focus_handle(),
            title: mezon_i18n::t(locale, "common.deleteAccount").into(),
            description: mezon_i18n::t(locale, "common.confirmDeleteAccount").into(),
            cancel_label: mezon_i18n::t(locale, "common.cancel").into(),
            delete_label: mezon_i18n::t(locale, "common.delete").into(),
            deleting: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn confirm_disable_clan_community(
        &mut self,
        on_confirm: impl Fn(&mut App) + 'static,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title: SharedString = mezon_i18n::t(
            locale,
            "onBoardingClan.communitySettings.disableModal.title",
        )
        .into();
        let description: SharedString = mezon_i18n::t(
            locale,
            "onBoardingClan.communitySettings.disableModal.description",
        )
        .into();
        let cancel_label: SharedString = mezon_i18n::t(locale, "common.cancel").into();
        let confirm_label: SharedString =
            mezon_i18n::t(locale, "onBoardingClan.communitySettings.buttons.disable").into();
        let view = cx.new(|cx| DisableClanCommunityModal {
            focus_handle: cx.focus_handle(),
            title,
            description,
            cancel_label,
            confirm_label,
            on_confirm: std::rc::Rc::new(on_confirm),
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
    }

    pub fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus_handle) = self.pop_modal(cx) {
            window.focus(&focus_handle, cx);
        }
    }

    fn pop_modal(&mut self, cx: &mut Context<Self>) -> Option<gpui::FocusHandle> {
        self.modal.take()?;
        let focus = if let Some((underlay, fullscreen, command_palette_open, focus_handle)) =
            self.modal_underlay.take()
        {
            self.modal = Some(underlay);
            self.modal_fullscreen = fullscreen;
            self.command_palette_open = command_palette_open;
            focus_handle
        } else {
            self.command_palette_open = false;
            self.modal_fullscreen = false;
            self.modal_restore_focus.take()
        };
        cx.notify();
        focus
    }

    pub fn close_modal_view(&mut self, view: gpui::EntityId, cx: &mut Context<Self>) {
        if self
            .modal
            .as_ref()
            .is_some_and(|modal| modal.entity_id() == view)
        {
            self.close_modal(cx);
        }
    }

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        if self.modal.is_none() && self.modal_underlay.is_none() {
            return;
        }
        self.modal_underlay.take();
        self.modal_restore_focus = None;
        self.modal.take();
        self.command_palette_open = false;
        self.modal_fullscreen = false;
        cx.notify();
    }

    pub fn close_modal_if_current(&mut self, owner: gpui::EntityId, cx: &mut Context<Self>) {
        if self.modal.as_ref().map(AnyView::entity_id) == Some(owner) {
            // Pop, not close: a modal stacked underneath this one is not ours to tear down.
            self.modal_restore_focus = self.pop_modal(cx);
            return;
        }
        if self
            .modal_underlay
            .as_ref()
            .map(|(view, ..)| view.entity_id())
            == Some(owner)
        {
            // The owner is buried under a newer modal. Drop it, but hand the focus it saved
            // to whoever dismisses the modal now on top, or that focus is lost for good.
            self.modal_restore_focus = self.modal_underlay.take().and_then(|(.., focus)| focus);
            cx.notify();
        }
    }

    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }

    /// The overlay (modal backdrop + toast stack), rendered on top by `RootView`.
    pub fn render_overlay(&self, cx: &App) -> impl IntoElement {
        let modal = self.modal.clone();
        let fullscreen = self.modal_fullscreen;
        let modal_underlay = self
            .modal_underlay
            .as_ref()
            .map(|(view, fullscreen, _, _)| (view.clone(), *fullscreen));
        let has_toasts = !self.toasts.is_empty();

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .when_some(modal_underlay, |el, (view, fullscreen)| {
                el.child(deferred(if fullscreen {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .child(div().size_full().child(view))
                        .into_any_element()
                } else {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(hsla(0., 0., 0., 0.5))
                        .child(view)
                        .into_any_element()
                }))
            })
            .when_some(modal, |el, view| {
                el.child(deferred(if fullscreen {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .key_context("modal_backdrop")
                        .on_action(|_: &::menu::Cancel, window, cx| {
                            Shell::global(cx)
                                .update(cx, |shell, cx| shell.dismiss_modal(window, cx));
                        })
                        .child(div().occlude().size_full().child(view))
                        .into_any_element()
                } else {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(hsla(0., 0., 0., 0.5))
                        .occlude()
                        .key_context("modal_backdrop")
                        .on_action(|_: &::menu::Cancel, window, cx| {
                            Shell::global(cx)
                                .update(cx, |shell, cx| shell.dismiss_modal(window, cx));
                        })
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            Shell::global(cx).update(cx, |shell, cx| {
                                if shell.modal_backdrop_dismissible {
                                    shell.dismiss_modal(window, cx);
                                }
                            });
                        })
                        .child(div().occlude().child(view))
                        .into_any_element()
                }))
            })
            .children(crate::tour::layer(cx))
            .when(has_toasts, |el| {
                el.child(deferred(
                    div()
                        .absolute()
                        .top(px(44.))
                        .right(px(16.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(self.toasts.iter().map(|item| {
                            let id = item.id;
                            let toast = Toast::new(item.message.clone())
                                .kind(item.kind)
                                .progress(item.progress)
                                .countdown(id, item.countdown);
                            div()
                                .id(("toast", id))
                                .cursor_pointer()
                                .on_click(move |_, _window, cx| {
                                    Shell::global(cx)
                                        .update(cx, |shell, cx| shell.dismiss_by_id(id, cx));
                                })
                                .child(toast)
                        })),
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clan::settings::ClanSettingsPage;
    use gpui::{IntoElement, TestAppContext};
    use mezon_store::{ChannelId, ClanId};

    struct StubModal;

    impl Render for StubModal {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn open_shell_with_modal(cx: &mut TestAppContext) -> Entity<Shell> {
        cx.update(|cx| {
            crate::router::Router::init(cx);
            crate::router::replace(
                cx,
                Route::ClanSettings {
                    clan_id: ClanId(7),
                    page: ClanSettingsPage::Emoji,
                },
            );
            let shell = Shell::init(cx);
            let modal = cx.new(|_| StubModal);
            shell.update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
            shell
        })
    }

    fn conversation_route() -> Route {
        Route::Channel {
            clan_id: ClanId(7),
            channel_id: ChannelId(42),
        }
    }

    #[gpui::test]
    fn a_late_request_cannot_close_the_modal_that_replaced_it(cx: &mut TestAppContext) {
        let shell = open_shell_with_modal(cx);
        let stale = cx.update(|cx| cx.new(|_| StubModal).entity_id());

        cx.update(|cx| {
            shell.update(cx, |shell, cx| shell.close_modal_if_current(stale, cx));
        });

        assert!(
            shell.read_with(cx, |shell, _| shell.has_modal()),
            "a request that outlived its dismissed modal must not close whichever modal took              its place"
        );
    }

    #[gpui::test]
    fn an_external_trigger_closes_the_settings_modal_it_navigates_away_from(
        cx: &mut TestAppContext,
    ) {
        let shell = open_shell_with_modal(cx);
        assert!(shell.read_with(cx, |shell, _| shell.has_modal()));

        cx.update(|cx| Shell::navigate_from_external_trigger(cx, conversation_route()));
        cx.run_until_parked();

        assert!(
            !shell.read_with(cx, |shell, _| shell.has_modal()),
            "a modal opened on the screen we just left must not keep covering the destination \
             a notification click navigated to"
        );
    }

    #[gpui::test]
    fn in_app_navigation_leaves_the_modal_alone(cx: &mut TestAppContext) {
        let shell = open_shell_with_modal(cx);

        cx.update(|cx| crate::router::navigate(cx, conversation_route()));
        cx.run_until_parked();

        assert!(
            shell.read_with(cx, |shell, _| shell.has_modal()),
            "only an external trigger closes the modal; a plain route change (an archived-thread \
             redirect, a permission-driven settings page swap) must leave it open"
        );
    }
}
