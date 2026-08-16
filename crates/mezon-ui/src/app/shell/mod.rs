//! App shell: a per-window aggregate (cf. Zed's `Workspace`) owning the cross-cutting overlay
//! layers — toasts and the modal stack — rendered on top of everything by `RootView`.
//!
//! Any view can surface a toast or a modal from anywhere via [`Shell::global`], instead of each
//! page wiring its own local toast/dialog state.

use std::time::Duration;

use gpui::{
    AnyView, App, AppContext, Context, Entity, Global, MouseButton, SharedString, Task, Window,
    deferred, div, hsla, prelude::*, px,
};

use crate::components::primitives::{Toast, ToastKind};
use crate::router::Route;

mod coming_soon_modal;
mod confirm_archive_channel_modal;
mod confirm_delete_canvas_modal;
mod confirm_delete_channel_modal;
mod confirm_delete_emoji_modal;
mod confirm_delete_message_modal;
mod confirm_delete_role_modal;
mod confirm_delete_sound_modal;
mod confirm_delete_sticker_modal;
mod confirm_delete_thread_modal;
mod confirm_delete_webhook_modal;
mod confirm_leave_thread_modal;
mod confirm_remove_friend_modal;
mod disable_clan_community_modal;
mod upload_limit_modal;
mod wallet_not_available_modal;
use coming_soon_modal::ComingSoonModal;
use confirm_archive_channel_modal::ConfirmArchiveChannelModal;
use confirm_delete_canvas_modal::ConfirmDeleteCanvasModal;
use confirm_delete_channel_modal::ConfirmDeleteChannelModal;
use confirm_delete_emoji_modal::ConfirmDeleteEmojiModal;
use confirm_delete_message_modal::ConfirmDeleteMessageModal;
use confirm_delete_role_modal::ConfirmDeleteRoleModal;
use confirm_delete_sound_modal::ConfirmDeleteSoundModal;
use confirm_delete_sticker_modal::ConfirmDeleteStickerModal;
use confirm_delete_thread_modal::ConfirmDeleteThreadModal;
use confirm_delete_webhook_modal::{ConfirmDeleteWebhookModal, WebhookDeleteTarget};
use confirm_leave_thread_modal::ConfirmLeaveThreadModal;
pub use confirm_remove_friend_modal::FriendRemovalKind;
use confirm_remove_friend_modal::{ConfirmRemoveFriendModal, interpolate_username};
use disable_clan_community_modal::DisableClanCommunityModal;
use upload_limit_modal::UploadLimitModal;
use wallet_not_available_modal::WalletNotAvailableModal;

const TOAST_TTL: Duration = Duration::from_secs(4);

struct ToastItem {
    id: usize,
    key: Option<SharedString>,
    message: SharedString,
    kind: ToastKind,
    progress: Option<f32>,
    _ttl: Option<Task<()>>,
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

/// Owns the window-level overlay layers (toasts + active modal). Registered as a [`Global`].
pub struct Shell {
    toasts: Vec<ToastItem>,
    modal: Option<AnyView>,
    modal_underlay: Option<(AnyView, bool, bool, Option<gpui::FocusHandle>)>,
    modal_fullscreen: bool,
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
            modal_fullscreen: false,
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

    /// Show a transient toast; it auto-dismisses after [`TOAST_TTL`].
    pub fn toast(
        &mut self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let ttl = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_TTL).await;
            let _ = this.update(cx, |this, cx| {
                this.toasts.retain(|t| t.id != id);
                cx.notify();
            });
        });
        self.toasts.push(ToastItem {
            id,
            key: None,
            message: message.into(),
            kind,
            progress: None,
            _ttl: Some(ttl),
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
            _ttl: None,
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
                _ttl: None,
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
        self.command_palette_open = false;
        self.modal_fullscreen = false;
        self.modal = Some(view);
        cx.notify();
    }

    /// Show `view` as a fullscreen modal (e.g. an image/media viewer): it renders its own
    /// full-viewport backdrop, so the overlay skips the centered card treatment and dim layer.
    pub fn show_fullscreen_modal(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.modal_underlay = None;
        self.command_palette_open = false;
        self.modal_fullscreen = true;
        self.modal = Some(view);
        cx.notify();
    }

    pub fn show_command_palette(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.modal_underlay = None;
        self.command_palette_open = true;
        self.modal = Some(view);
        cx.notify();
    }

    pub fn command_palette_open(&self) -> bool {
        self.command_palette_open
    }

    /// Open a placeholder modal for a not-yet-implemented feature: the given `title` plus a
    /// "coming soon" body and a close button.
    pub fn show_coming_soon(
        &mut self,
        title: impl Into<SharedString>,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message: SharedString = mezon_i18n::t(locale, "common.comingSoon")
            .to_string()
            .into();
        let close_label: SharedString = mezon_i18n::t(locale, "common.close").to_string().into();
        let title = title.into();
        let view = cx.new(|cx| ComingSoonModal {
            focus_handle: cx.focus_handle(),
            title,
            message,
            close_label,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        self.show_modal(view.into(), cx);
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
        let view = cx.new(|cx| WalletNotAvailableModal {
            focus_handle: cx.focus_handle(),
            title,
            description: mezon_i18n::t(locale, "message.wallet.descNotAvailable").into(),
            enable_label: mezon_i18n::t(locale, "message.wallet.enableWallet").into(),
            cancel_label: mezon_i18n::t(locale, "message.wallet.cancel").into(),
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

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        if self.modal.take().is_some() {
            self.modal_underlay = None;
            self.command_palette_open = false;
            self.modal_fullscreen = false;
            cx.notify();
        }
    }

    pub fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.take().is_none() {
            return;
        }
        if let Some((underlay, fullscreen, command_palette_open, focus_handle)) =
            self.modal_underlay.take()
        {
            self.modal = Some(underlay);
            self.modal_fullscreen = fullscreen;
            self.command_palette_open = command_palette_open;
            if let Some(focus_handle) = focus_handle {
                window.focus(&focus_handle, cx);
            }
        } else {
            self.command_palette_open = false;
            self.modal_fullscreen = false;
        }
        cx.notify();
    }

    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }

    /// The overlay (modal backdrop + toast stack), rendered on top by `RootView`.
    pub fn render_overlay(&self) -> impl IntoElement {
        let modal = self.modal.clone();
        let fullscreen = self.modal_fullscreen;
        let modal_underlay = self
            .modal_underlay
            .as_ref()
            .map(|(view, fullscreen, _, _)| (view.clone(), *fullscreen));
        let has_toasts = !self.toasts.is_empty();
        let toasts: Vec<(usize, SharedString, ToastKind, Option<f32>)> = self
            .toasts
            .iter()
            .map(|t| (t.id, t.message.clone(), t.kind, t.progress))
            .collect();

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
                        .key_context("modal_backdrop")
                        .on_action(|_: &::menu::Cancel, window, cx| {
                            Shell::global(cx)
                                .update(cx, |shell, cx| shell.dismiss_modal(window, cx));
                        })
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            Shell::global(cx)
                                .update(cx, |shell, cx| shell.dismiss_modal(window, cx));
                        })
                        .child(div().occlude().child(view))
                        .into_any_element()
                }))
            })
            .when(has_toasts, |el| {
                el.child(deferred(
                    div()
                        .absolute()
                        .top(px(44.))
                        .right(px(16.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(toasts.into_iter().map(|(id, message, kind, progress)| {
                            div()
                                .id(("toast", id))
                                .cursor_pointer()
                                .on_click(move |_, _window, cx| {
                                    Shell::global(cx)
                                        .update(cx, |shell, cx| shell.dismiss_by_id(id, cx));
                                })
                                .child(Toast::new(message).kind(kind).progress(progress))
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
