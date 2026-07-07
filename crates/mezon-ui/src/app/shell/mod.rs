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
mod confirm_delete_message_modal;
mod upload_limit_modal;
use coming_soon_modal::ComingSoonModal;
use confirm_delete_message_modal::ConfirmDeleteMessageModal;
use upload_limit_modal::UploadLimitModal;

const TOAST_TTL: Duration = Duration::from_secs(4);

struct ToastItem {
    id: usize,
    message: SharedString,
    kind: ToastKind,
    _ttl: Task<()>,
}

/// Owns the window-level overlay layers (toasts + active modal). Registered as a [`Global`].
pub struct Shell {
    toasts: Vec<ToastItem>,
    modal: Option<AnyView>,
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
            message: message.into(),
            kind,
            _ttl: ttl,
        });
        cx.notify();
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

    /// React's `TooManyUpload` popup: a red validation card shown when a drop/pick
    /// exceeds the attachment count or per-file size limit.
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

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        if self.modal.take().is_some() {
            self.command_palette_open = false;
            cx.notify();
        }
    }

    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }

    /// The overlay (modal backdrop + toast stack), rendered on top by `RootView`.
    pub fn render_overlay(&self) -> impl IntoElement {
        let modal = self.modal.clone();
        let has_toasts = !self.toasts.is_empty();
        let toasts: Vec<(SharedString, ToastKind)> = self
            .toasts
            .iter()
            .map(|t| (t.message.clone(), t.kind))
            .collect();

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .when_some(modal, |el, view| {
                el.child(deferred(
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
                        .child(div().occlude().child(view)),
                ))
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
                        .children(
                            toasts
                                .into_iter()
                                .map(|(message, kind)| Toast::new(message).kind(kind)),
                        ),
                ))
            })
    }
}
