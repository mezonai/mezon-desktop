use gpui::{Context, Entity, FocusHandle, FontWeight, SharedString, Window, div, prelude::*, px};
use mezon_store::{ClanId, ClanMembersStore, UserId};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, Input, InputState, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmKickMemberModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) user_id: UserId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) reason_label: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) confirm_label: SharedString,
    pub(super) success_message: SharedString,
    pub(super) error_message: SharedString,
    pub(super) reason_input: Entity<InputState>,
}

impl ConfirmKickMemberModal {
    fn kick(&self, cx: &mut Context<Self>) {
        let clan_id = self.clan_id;
        let user_id = self.user_id;
        let success_message = self.success_message.clone();
        let error_message = self.error_message.clone();
        let task = ClanMembersStore::global(cx)
            .update(cx, |store, cx| store.kick_member(clan_id, user_id, cx));
        cx.spawn(async move |_, cx| {
            let result = task.await;
            cx.update(|cx| {
                Shell::global(cx).update(cx, |shell, cx| match result {
                    Ok(()) => shell.success(success_message, cx),
                    Err(error) => {
                        tracing::error!("kick member {user_id} from {clan_id} failed: {error}");
                        shell.error(error_message, cx);
                    }
                });
            });
        })
        .detach();
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for ConfirmKickMemberModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(440.))
            .gap_2()
            .p(px(24.))
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
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
                div()
                    .pt(px(8.))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(self.reason_label.clone()),
            )
            .child(Input::new(&self.reason_input).w_full())
            .child(
                h_flex()
                    .pt(px(12.))
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("kick-member-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("kick-member-confirm")
                            .label(self.confirm_label.clone())
                            .danger()
                            .on_click(cx.listener(|this, _, _window, cx| this.kick(cx))),
                    ),
            )
    }
}
