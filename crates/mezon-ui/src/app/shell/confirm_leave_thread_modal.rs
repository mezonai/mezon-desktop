use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelId, ClanId, ThreadsStore};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmLeaveThreadModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) channel_id: ChannelId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) leave_label: SharedString,
}

impl Render for ConfirmLeaveThreadModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
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
                        Button::new("confirm-leave-thread-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("confirm-leave-thread-confirm")
                            .label(self.leave_label.clone())
                            .danger()
                            .on_click(move |_, _window, cx| {
                                ThreadsStore::global(cx).update(cx, |store, cx| {
                                    store.leave_thread(clan_id, channel_id, cx);
                                });
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    ),
            )
    }
}
