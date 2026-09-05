use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelId, GroupMembersStore, UserId};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmRemoveGroupMemberModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) channel_id: ChannelId,
    pub(super) user_id: UserId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) confirm_label: SharedString,
    pub(super) success_message: SharedString,
    pub(super) failed_message: SharedString,
    pub(super) removing: bool,
}

impl Render for ConfirmRemoveGroupMemberModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let removing = self.removing;

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
                        Button::new("confirm-remove-group-member-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("confirm-remove-group-member-confirm")
                            .label(self.confirm_label.clone())
                            .danger()
                            .disabled(removing)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                if this.removing {
                                    return;
                                }
                                this.removing = true;
                                cx.notify();
                                let channel_id = this.channel_id;
                                let user_id = this.user_id;
                                let success = this.success_message.clone();
                                let failed = this.failed_message.clone();
                                let task = GroupMembersStore::global(cx).update(cx, |store, cx| {
                                    store.remove_member(channel_id, user_id, cx)
                                });
                                cx.spawn(async move |this, cx| {
                                    let result = task.await;
                                    let _ = this.update(cx, |this, cx| {
                                        this.removing = false;
                                        cx.notify();
                                    });
                                    cx.update(|cx| {
                                        Shell::global(cx).update(cx, |shell, cx| {
                                            shell.close_modal(cx);
                                            match result {
                                                Ok(()) => shell.success(success, cx),
                                                Err(error) => {
                                                    tracing::error!(
                                                        "remove member {user_id} from group {channel_id} failed: {error}"
                                                    );
                                                    shell.error(failed, cx);
                                                }
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
