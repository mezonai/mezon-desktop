use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{ChannelList, ClanId};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmDeleteCategoryModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) clan_id: ClanId,
    pub(super) category_id: String,
    pub(super) locale: String,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) delete_label: SharedString,
    pub(super) submitting: bool,
}

impl ConfirmDeleteCategoryModal {
    fn delete(&mut self, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        self.submitting = true;
        cx.notify();

        let clan_id = self.clan_id;
        let category_id = self.category_id.clone();
        let locale = self.locale.clone();

        cx.spawn(async move |this, cx| {
            let task = this
                .update(cx, |_, cx| {
                    ChannelList::global(cx).update(cx, |store, cx| {
                        store.delete_category(clan_id, category_id.clone(), cx)
                    })
                })
                .ok();
            let Some(task) = task else {
                return;
            };
            let result = task.await;

            cx.update(|cx| {
                Shell::global(cx).update(cx, |shell, cx| match &result {
                    Ok(()) => shell.close_modal(cx),
                    Err(err) => {
                        tracing::error!("delete_category failed for {category_id}: {err}");
                        shell.error(
                            mezon_i18n::t(&locale, "contextMenu.deleteCategoryFailed").to_string(),
                            cx,
                        );
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

impl Render for ConfirmDeleteCategoryModal {
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
            .on_action(cx.listener(|this, _: &::menu::Confirm, _window, cx| {
                this.delete(cx);
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
                        Button::new("confirm-delete-category-cancel")
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
                        Button::new("confirm-delete-category-confirm")
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
