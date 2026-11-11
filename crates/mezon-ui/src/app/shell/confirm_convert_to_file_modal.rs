use gpui::{Context, FocusHandle, SharedString, WeakEntity, Window, div, prelude::*, px};

use super::Shell;
use crate::chat::MentionInput;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmConvertToFileModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) composer: WeakEntity<MentionInput>,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) confirm_label: SharedString,
}

impl ConfirmConvertToFileModal {
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = self.composer.clone();
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
        let _ = composer.update(cx, |c, cx| c.focus_input(window, cx));
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = self.composer.clone();
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
        let _ = composer.update(cx, |c, cx| c.convert_confirmed(window, cx));
    }
}

impl Render for ConfirmConvertToFileModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, window, cx| {
                this.cancel(window, cx);
            }))
            .on_action(cx.listener(|this, _: &::menu::Confirm, window, cx| {
                this.confirm(window, cx);
            }))
            .w(px(440.))
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                v_flex()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(20.))
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(15.))
                            .text_color(theme.text_primary)
                            .child(self.description.clone()),
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .items_center()
                    .gap_4()
                    .p(px(16.))
                    .bg(theme.bg_secondary)
                    .child(
                        Button::new("confirm-convert-to-file-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel(window, cx);
                            })),
                    )
                    .child(
                        Button::new("confirm-convert-to-file-confirm")
                            .label(self.confirm_label.clone())
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm(window, cx);
                            })),
                    ),
            )
    }
}
