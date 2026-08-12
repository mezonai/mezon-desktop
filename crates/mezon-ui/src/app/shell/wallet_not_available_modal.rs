use gpui::{
    Context, FocusHandle, SharedString, Window, div, img, prelude::*, px, rgb, rgba, white,
};
use mezon_store::WalletStore;

use super::Shell;
use crate::components::primitives::{h_flex, v_flex};
use crate::theme::ActiveTheme;

const ENABLE_BUTTON_BG: u32 = 0x5865f2;
const ENABLE_BUTTON_BG_HOVER: u32 = 0x4752c4;
const ICON_CIRCLE_BG: u32 = 0x5865f233;

pub(super) struct WalletNotAvailableModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) enable_label: SharedString,
    pub(super) cancel_label: SharedString,
}

impl Render for WalletNotAvailableModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.dismiss_modal(window, cx));
            }))
            .w(px(448.))
            .rounded_lg()
            .border_1()
            .border_color(theme.tokens.border_primary)
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .overflow_hidden()
            .child(
                v_flex()
                    .px(px(24.))
                    .pt(px(32.))
                    .pb(px(24.))
                    .items_center()
                    .text_center()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(64.))
                            .rounded_full()
                            .bg(rgba(ICON_CIRCLE_BG))
                            .child(
                                img("icons/icon-clock-channel.svg")
                                    .size(px(40.))
                                    .flex_none(),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(self.title.clone()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(self.description.clone()),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .px(px(24.))
                    .py(px(16.))
                    .bg(theme.tokens.theme_setting_nav)
                    .child(
                        div()
                            .id("wallet-not-available-enable")
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .py(px(10.))
                            .px(px(16.))
                            .rounded(px(4.))
                            .bg(rgb(ENABLE_BUTTON_BG))
                            .hover(|this| this.bg(rgb(ENABLE_BUTTON_BG_HOVER)))
                            .cursor_pointer()
                            .text_color(white())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(self.enable_label.clone())
                            .on_click(|_, window, cx| {
                                WalletStore::global(cx).update(cx, |wallet, cx| {
                                    wallet.enable_wallet_for_current_user(true, cx)
                                });
                                Shell::global(cx)
                                    .update(cx, |shell, cx| shell.dismiss_modal(window, cx));
                            }),
                    )
                    .child(
                        div()
                            .id("wallet-not-available-cancel")
                            .flex()
                            .items_center()
                            .justify_center()
                            .py(px(10.))
                            .px(px(16.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(self.cancel_label.clone())
                            .on_click(|_, window, cx| {
                                Shell::global(cx)
                                    .update(cx, |shell, cx| shell.dismiss_modal(window, cx));
                            }),
                    ),
            )
    }
}
