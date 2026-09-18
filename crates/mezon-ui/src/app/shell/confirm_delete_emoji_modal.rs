use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ClanId, EmojiStore};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmDeleteEmojiModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) emoji_id: SharedString,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) delete_label: SharedString,
}

impl Render for ConfirmDeleteEmojiModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let owner = cx.entity().entity_id();
        let clan_id = self.clan_id;
        let emoji_id = self.emoji_id.clone();

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
                        Button::new("confirm-delete-emoji-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("confirm-delete-emoji-confirm")
                            .label(self.delete_label.clone())
                            .danger()
                            .on_click(move |_, _window, cx| {
                                let task = EmojiStore::global(cx).update(cx, |store, cx| {
                                    store.delete_emoji(emoji_id.as_ref(), clan_id, cx)
                                });
                                cx.spawn(async move |cx| match task.await {
                                    Ok(()) => {
                                        cx.update(|cx| {
                                            Shell::global(cx).update(cx, |shell, cx| {
                                                shell.close_modal_if_current(owner, cx)
                                            });
                                        });
                                    }
                                    Err(err) => {
                                        cx.update(|cx| {
                                            Shell::global(cx).update(cx, |shell, cx| {
                                                shell.error(
                                                    format!("Failed to delete emoji: {err}"),
                                                    cx,
                                                );
                                            });
                                        });
                                    }
                                })
                                .detach();
                            }),
                    ),
            )
    }
}
