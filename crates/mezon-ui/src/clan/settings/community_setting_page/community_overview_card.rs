use gpui::{
    AnyElement, Context, FontWeight, Hsla, div, linear_color_stop, linear_gradient, prelude::*, px,
    rgb,
};

use super::CommunitySettingPage;
use crate::components::primitives::{FocusCycle, Icon, IconName, h_flex, v_flex};
use crate::theme::Theme;

const CARD_MAX_WIDTH: f32 = 672.0;
const CARD_RADIUS: f32 = 16.0;
const CARD_BORDER_OPACITY: f32 = 0.5;
const HEADER_GRADIENT_START: u32 = 0x22c55e;
const HEADER_GRADIENT_END: u32 = 0x2563eb;
const HEADER_SUBTITLE: u32 = 0xdcfce7;
const DISABLE_GRADIENT_START: u32 = 0xef4444;
const DISABLE_GRADIENT_END: u32 = 0xdc2626;
const DISABLE_HOVER_GRADIENT_START: u32 = 0xdc2626;
const DISABLE_HOVER_GRADIENT_END: u32 = 0xb91c1c;

impl CommunitySettingPage {
    pub(super) fn render_overview_card(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled = self.saving || self.draft.has_changes(&self.saved_draft);

        v_flex()
            .size_full()
            .min_h_0()
            .items_center()
            .py_6()
            .child(
                v_flex()
                    .w(px(CARD_MAX_WIDTH))
                    .max_w_full()
                    .max_h_full()
                    .min_h_0()
                    .overflow_hidden()
                    .rounded(px(CARD_RADIUS))
                    .border_1()
                    .border_color(Hsla::from(theme.text_primary).opacity(CARD_BORDER_OPACITY))
                    .bg(theme.tokens.bg_surface)
                    .shadow_lg()
                    .focus_cycle(self.input_focus_handles(cx))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_3()
                            .items_center()
                            .p_6()
                            .rounded_t(px(CARD_RADIUS))
                            .bg(linear_gradient(
                                90.0,
                                linear_color_stop(rgb(HEADER_GRADIENT_START), 0.0),
                                linear_color_stop(rgb(HEADER_GRADIENT_END), 1.0),
                            ))
                            .text_color(gpui::white())
                            .child(Icon::motion_scope(
                                div()
                                    .id("community-overview-icon")
                                    .flex_shrink_0()
                                    .size(px(48.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(12.0))
                                    .bg(gpui::white().alpha(0.2))
                                    .child(
                                        Icon::new(IconName::Community)
                                            .size(px(24.0))
                                            .color(gpui::white()),
                                    ),
                                true,
                            ))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(div().text_2xl().font_weight(FontWeight::BOLD).child(
                                        mezon_i18n::t(
                                            locale,
                                            "onBoardingClan.communitySettings.title",
                                        ),
                                    ))
                                    .child(div().text_color(rgb(HEADER_SUBTITLE)).child(
                                        mezon_i18n::t(
                                            locale,
                                            "onBoardingClan.communitySettings.subtitle",
                                        ),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("community-overview-scroll")
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(
                        self.render_community_fields(locale, theme, false, cx)
                            .child(
                                h_flex()
                                    .justify_end()
                                    .pt_6()
                                    .border_t_1()
                                    .border_color(theme.border)
                                    .child(
                                        div()
                                            .id("community-disable")
                                            .px_6()
                                            .py_3()
                                            .rounded(px(12.0))
                                            .bg(linear_gradient(
                                                90.0,
                                                linear_color_stop(rgb(DISABLE_GRADIENT_START), 0.0),
                                                linear_color_stop(rgb(DISABLE_GRADIENT_END), 1.0),
                                            ))
                                            .text_color(gpui::white())
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .shadow_lg()
                                            .child(mezon_i18n::t(
                                                locale,
                                                "onBoardingClan.communitySettings.buttons.disable",
                                            ))
                                            .map(|button| {
                                                if disabled {
                                                    button.opacity(0.5).cursor_not_allowed()
                                                } else {
                                                    button
                                                        .cursor_pointer()
                                                        .hover(|style| {
                                                            style.bg(linear_gradient(
                                                            90.0,
                                                            linear_color_stop(
                                                                rgb(DISABLE_HOVER_GRADIENT_START),
                                                                0.0,
                                                            ),
                                                            linear_color_stop(
                                                                rgb(DISABLE_HOVER_GRADIENT_END),
                                                                1.0,
                                                            ),
                                                        ))
                                                        })
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.request_disable_community(
                                                                    window, cx,
                                                                );
                                                            },
                                                        ))
                                                }
                                            }),
                                    ),
                            ),
                    ),
                        ),
            )
            .into_any_element()
    }
}
