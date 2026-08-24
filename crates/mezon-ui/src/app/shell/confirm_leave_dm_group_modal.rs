use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelId, DirectMessageStore};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmLeaveDmGroupModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) channel_id: ChannelId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) confirm_label: SharedString,
    pub(super) failed_message: SharedString,
    pub(super) leaving: bool,
}

impl Render for ConfirmLeaveDmGroupModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let leaving = self.leaving;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(440.))
            .gap_4()
            .p(px(20.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(self.description.clone()),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("confirm-leave-dm-group-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("confirm-leave-dm-group-confirm")
                            .label(self.confirm_label.clone())
                            .danger()
                            .disabled(leaving)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                if this.leaving {
                                    return;
                                }
                                this.leaving = true;
                                cx.notify();
                                let channel_id = this.channel_id;
                                let failed = this.failed_message.clone();
                                let task = DirectMessageStore::global(cx)
                                    .update(cx, |store, cx| store.leave_group(channel_id, cx));
                                cx.spawn(async move |this, cx| {
                                    let result = task.await;
                                    let _ = this.update(cx, |this, cx| {
                                        this.leaving = false;
                                        cx.notify();
                                    });
                                    cx.update(|cx| {
                                        Shell::global(cx).update(cx, |shell, cx| {
                                            shell.close_modal(cx);
                                            if result.is_err() {
                                                shell.error(failed, cx);
                                            }
                                        });
                                    });
                                })
                                .detach();
                            })),
                    ),
            )
    }
}
