use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelId, ClanId, QuickMenuStore};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmDeleteQuickMenuModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) channel_id: ChannelId,
    pub(super) item_id: i64,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) delete_label: SharedString,
    pub(super) submitting: bool,
}

impl ConfirmDeleteQuickMenuModal {
    fn delete(&mut self, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        self.submitting = true;
        cx.notify();

        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let item_id = self.item_id;

        cx.spawn(async move |this, cx| {
            let task = this
                .update(cx, |_, cx| {
                    QuickMenuStore::global(cx).update(cx, |store, cx| {
                        store.delete(clan_id, channel_id, item_id, cx)
                    })
                })
                .ok();
            let Some(task) = task else {
                return;
            };
            let result = task.await;

            cx.update(|cx| {
                Shell::global(cx).update(cx, |shell, cx| match result {
                    Ok(()) => {
                        shell.close_modal(cx);
                    }
                    Err(err) => {
                        shell.error(err, cx);
                    }
                });
            });

            let _ = this.update(cx, |this, cx| {
                this.submitting = false;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ConfirmDeleteQuickMenuModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let submitting = self.submitting;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.submitting {
                    return;
                }
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
                        Button::new("confirm-delete-quick-menu-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .disabled(submitting)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                if this.submitting {
                                    return;
                                }
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            })),
                    )
                    .child(
                        Button::new("confirm-delete-quick-menu-confirm")
                            .label(self.delete_label.clone())
                            .danger()
                            .disabled(submitting)
                            .loading(submitting)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.delete(cx);
                            })),
                    ),
            )
    }
}
