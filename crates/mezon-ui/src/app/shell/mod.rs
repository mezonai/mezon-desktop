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

mod coming_soon_modal;
mod confirm_delete_canvas_modal;
mod confirm_delete_emoji_modal;
mod confirm_delete_message_modal;
mod confirm_delete_role_modal;
mod confirm_delete_sound_modal;
mod confirm_delete_sticker_modal;
mod confirm_delete_webhook_modal;
mod confirm_leave_thread_modal;
mod confirm_remove_friend_modal;
mod disable_clan_community_modal;
mod upload_limit_modal;
use coming_soon_modal::ComingSoonModal;
use confirm_delete_canvas_modal::ConfirmDeleteCanvasModal;
use confirm_delete_emoji_modal::ConfirmDeleteEmojiModal;
use confirm_delete_message_modal::ConfirmDeleteMessageModal;
use confirm_delete_role_modal::ConfirmDeleteRoleModal;
use confirm_delete_sound_modal::ConfirmDeleteSoundModal;
use confirm_delete_sticker_modal::ConfirmDeleteStickerModal;
use confirm_delete_webhook_modal::{ConfirmDeleteWebhookModal, WebhookDeleteTarget};
use confirm_leave_thread_modal::ConfirmLeaveThreadModal;
pub use confirm_remove_friend_modal::FriendRemovalKind;
use confirm_remove_friend_modal::{ConfirmRemoveFriendModal, interpolate_username};
use disable_clan_community_modal::DisableClanCommunityModal;
use upload_limit_modal::UploadLimitModal;

const TOAST_TTL: Duration = Duration::from_secs(4);

struct ToastItem {
    id: usize,
    key: Option<SharedString>,
    message: SharedString,
    kind: ToastKind,
    progress: Option<f32>,
    _ttl: Option<Task<()>>,
}

/// Owns the window-level overlay layers (toasts + active modal). Registered as a [`Global`].
pub struct Shell {
    toasts: Vec<ToastItem>,
    modal: Option<AnyView>,
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
            modal_fullscreen: false,
            command_palette_open: false,
            next_id: 0,
        });
        cx.set_global(GlobalShell(entity.clone()));
        entity
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

    /// Show `view` as the active modal (backdrop click dismisses). The view renders its own card.
    pub fn show_modal(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.command_palette_open = false;
        self.modal_fullscreen = false;
        self.modal = Some(view);
        cx.notify();
    }

    /// Show `view` as a fullscreen modal (e.g. an image/media viewer): it renders its own
    /// full-viewport backdrop, so the overlay skips the centered card treatment and dim layer.
    pub fn show_fullscreen_modal(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.command_palette_open = false;
        self.modal_fullscreen = true;
        self.modal = Some(view);
        cx.notify();
    }

    pub fn show_command_palette(&mut self, view: AnyView, cx: &mut Context<Self>) {
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

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        if self.modal.take().is_some() {
            self.command_palette_open = false;
            self.modal_fullscreen = false;
            cx.notify();
        }
    }

    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }

    /// The overlay (modal backdrop + toast stack), rendered on top by `RootView`.
    pub fn render_overlay(&self) -> impl IntoElement {
        let modal = self.modal.clone();
        let fullscreen = self.modal_fullscreen;
        let has_toasts = !self.toasts.is_empty();
        let toasts: Vec<(SharedString, ToastKind, Option<f32>)> = self
            .toasts
            .iter()
            .map(|t| (t.message.clone(), t.kind, t.progress))
            .collect();

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .when_some(modal, |el, view| {
                el.child(deferred(if fullscreen {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .key_context("modal_backdrop")
                        .on_action(|_: &::menu::Cancel, _window, cx| {
                            Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
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
                        .on_action(|_: &::menu::Cancel, _window, cx| {
                            Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
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
                        .children(toasts.into_iter().map(|(message, kind, progress)| {
                            Toast::new(message).kind(kind).progress(progress)
                        })),
                ))
            })
    }
}
