use gpui::{
    Context, Entity, FocusHandle, FontWeight, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use mezon_store::{ClanId, ClanList};

use super::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmDeleteClanModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) clan_name: SharedString,
    pub(super) title: SharedString,
    pub(super) warning: SharedString,
    pub(super) name_label: SharedString,
    pub(super) incorrect_name: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) confirm_label: SharedString,
    pub(super) error_message: SharedString,
    pub(super) name_input: Entity<InputState>,
    pub(super) name_matches: Option<bool>,
    pub(super) _name_sub: Subscription,
}

impl ConfirmDeleteClanModal {
    pub(super) fn watch_name(
        name_input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe(name_input, |this, _, event: &InputEvent, cx| {
            if *event == InputEvent::Change {
                this.on_name_changed(cx);
            }
        })
    }

    fn on_name_changed(&mut self, cx: &mut Context<Self>) {
        let typed = self.name_input.read(cx).value().to_string();
        self.name_matches = if typed.is_empty() {
            None
        } else {
            Some(typed == self.clan_name.as_ref())
        };
        cx.notify();
    }

    fn delete(&self, cx: &mut Context<Self>) {
        if self.name_matches != Some(true) {
            return;
        }
        let clan_id = self.clan_id;
        let error_message = self.error_message.clone();
        let task = ClanList::global(cx).update(cx, |list, cx| list.delete_clan(clan_id, cx));
        cx.spawn(async move |_, cx| {
            if let Err(error) = task.await {
                tracing::error!("delete clan {clan_id} failed: {error}");
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(error_message, cx));
                });
            }
        })
        .detach();
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for ConfirmDeleteClanModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let can_delete = self.name_matches == Some(true);

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .on_action(cx.listener(|this, _: &::menu::Confirm, _window, cx| this.delete(cx)))
            .w(px(500.))
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                v_flex()
                    .w_full()
                    .gap(px(15.))
                    .p(px(16.))
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .truncate()
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .rounded_sm()
                            .p(px(10.))
                            .bg(gpui::rgb(0xf0b132))
                            .text_color(gpui::rgb(0x30232d))
                            .child(self.warning.clone()),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .mb(px(15.))
                            .child(
                                div()
                                    .text_base()
                                    .text_color(theme.text_primary)
                                    .child(self.name_label.clone()),
                            )
                            .child(
                                div()
                                    .my(px(7.))
                                    .child(Input::new(&self.name_input).w_full()),
                            )
                            .when(self.name_matches == Some(false), |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(gpui::rgb(0xfa777c))
                                        .child(self.incorrect_name.clone()),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .items_center()
                    .gap(px(20.))
                    .p(px(16.))
                    .rounded_b_md()
                    .bg(theme.tokens.theme_setting_nav)
                    .child(
                        Button::new("delete-clan-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("delete-clan-confirm")
                            .label(self.confirm_label.clone())
                            .danger()
                            .disabled(!can_delete)
                            .on_click(cx.listener(|this, _, _window, cx| this.delete(cx))),
                    ),
            )
    }
}
