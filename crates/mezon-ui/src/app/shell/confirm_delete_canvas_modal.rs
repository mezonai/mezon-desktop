use gpui::{Context, FocusHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{CanvasStore, ChannelId, ClanId};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;

pub(super) struct ConfirmDeleteCanvasModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) canvas_id: String,
    pub(super) clan_id: ClanId,
    pub(super) channel_id: ChannelId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) cancel_label: SharedString,
    pub(super) delete_label: SharedString,
}

impl Render for ConfirmDeleteCanvasModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let canvas_id = self.canvas_id.clone();
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
                        Button::new("confirm-delete-canvas-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("confirm-delete-canvas-confirm")
                            .label(self.delete_label.clone())
                            .danger()
                            .on_click(move |_, _window, cx| {
                                CanvasStore::global(cx).update(cx, |store, cx| {
                                    store.delete(&canvas_id, cx);
                                });
                                if matches!(
                                    crate::router::Router::global(cx).read(cx).route(),
                                    Route::Canvas {
                                        canvas_id: active,
                                        ..
                                    } if active.to_string() == canvas_id
                                ) {
                                    navigate(
                                        cx,
                                        Route::Channel {
                                            clan_id,
                                            channel_id,
                                        },
                                    );
                                }
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    ),
            )
    }
}
