use gpui::{AnyElement, ClickEvent, Entity, FontWeight, div, prelude::*, px};

use crate::chat::layout::ChatLayout;
use crate::chat::mention_input::MentionInput;
use crate::components::primitives::{
    Button, ButtonVariants, Checkbox, Icon, IconName, Input, InputState, h_flex, v_flex,
};
use crate::theme::Theme;

const PANEL_WIDTH: f32 = 510.;

pub struct CreateThreadPanelParams<'a> {
    pub thread_name_input: Entity<InputState>,
    pub message_input: Entity<MentionInput>,
    pub name_error: Option<&'a str>,
    pub submitting: bool,
    pub create_private: bool,
    pub locale: &'a str,
    pub theme: &'a Theme,
    pub layout: Entity<ChatLayout>,
}

pub fn render_create_thread_panel(params: CreateThreadPanelParams<'_>) -> AnyElement {
    let CreateThreadPanelParams {
        thread_name_input,
        message_input,
        name_error,
        submitting,
        create_private,
        locale,
        theme,
        layout,
    } = params;
    let tokens = &theme.tokens;
    let layout_close = layout.clone();
    let layout_footer_cancel = layout.clone();
    let layout_private = layout.clone();
    let send_layout = layout.clone();
    let private_label = mezon_i18n::t(
        locale,
        "channelTopbar.createThread.privateThreadDescription",
    );

    let error_label = name_error.map(|key| match key {
        "thread_name_too_short" => mezon_i18n::t(
            locale,
            "channelTopbar.createThread.toast.threadNameTooShort",
        ),
        "thread_name_exists" => {
            mezon_i18n::t(locale, "channelTopbar.createThread.toast.threadNameExists")
        }
        "initial_message_required" => mezon_i18n::t(
            locale,
            "channelTopbar.createThread.toast.initialMessageRequired",
        ),
        other => other,
    });

    let thread_icon = if create_private {
        IconName::ThreadIconLocker
    } else {
        IconName::ThreadIcon
    };

    v_flex()
        .w(px(PANEL_WIDTH))
        .min_w(px(PANEL_WIDTH))
        .flex_shrink_0()
        .h_full()
        .min_h_0()
        .overflow_hidden()
        .border_l_1()
        .border_color(tokens.border_primary)
        .bg(theme.bg_primary)
        .text_color(tokens.text_theme_message)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .px_4()
                .h(px(48.))
                .min_h(px(48.))
                .border_b_1()
                .border_color(tokens.border_primary)
                .bg(theme.bg_primary)
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(thread_icon)
                                .size_4()
                                .text_color(tokens.bg_icon_theme),
                        )
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(tokens.text_secondary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "channelTopbar.createThread.newThread",
                                )),
                        ),
                )
                .child(
                    div()
                        .id("create-thread-close")
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::Close)
                                .size_4()
                                .text_color(tokens.text_theme_primary_hover),
                        )
                        .on_click(move |_: &ClickEvent, _window, cx| {
                            layout_close.update(cx, |layout, cx| layout.close_create_thread(cx));
                        }),
                ),
        )
        .child(
            v_flex().flex_1().min_h_0().overflow_hidden().px_3().child(
                v_flex()
                    .flex_1()
                    .justify_end()
                    .child(
                        div()
                            .mx_4()
                            .mt_4()
                            .w(px(64.))
                            .h(px(64.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(tokens.bg_active_member_channel)
                            .child(
                                Icon::new(thread_icon)
                                    .size(px(28.))
                                    .text_color(tokens.bg_icon_theme),
                            ),
                    )
                    .child(
                        v_flex()
                            .mt_4()
                            .mb_4()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .mb_2()
                                    .text_color(tokens.text_secondary)
                                    .child(
                                        mezon_i18n::t(
                                            locale,
                                            "channelTopbar.createThread.threadName",
                                        )
                                        .to_uppercase(),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .h(px(40.))
                                    .rounded_lg()
                                    .bg(tokens.bg_active_member_channel)
                                    .border_1()
                                    .border_color(tokens.border_primary)
                                    .px(px(10.))
                                    .child(
                                        Input::new(&thread_name_input)
                                            .w_full()
                                            .text_base()
                                            .text_color(tokens.text_theme_message),
                                    ),
                            )
                            .when_some(error_label, |this, err| {
                                this.child(
                                    div()
                                        .mt_1()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.danger_text)
                                        .child(err.to_string()),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .mt_4()
                            .mb_4()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .mb_2()
                                    .text_color(tokens.text_secondary)
                                    .child(
                                        mezon_i18n::t(
                                            locale,
                                            "channelTopbar.createThread.privateThread",
                                        )
                                        .to_uppercase(),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Checkbox::new("create-thread-private")
                                            .checked(create_private)
                                            .on_click(move |checked, _window, cx| {
                                                layout_private.update(cx, |layout, cx| {
                                                    layout.set_create_thread_private(*checked, cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_base()
                                            .text_color(tokens.text_theme_primary)
                                            .child(private_label),
                                    ),
                            )
                            .when(create_private, |this| {
                                this.child(
                                    div()
                                        .mt_2()
                                        .text_xs()
                                        .text_color(tokens.text_theme_primary)
                                        .child(mezon_i18n::t(
                                            locale,
                                            "channelTopbar.createThread.inviteMessage",
                                        )),
                                )
                            }),
                    ),
            ),
        )
        .child(
            v_flex()
                .flex_shrink_0()
                .px_3()
                .pb_4()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .rounded_lg()
                        .bg(theme.surfaces.surface)
                        .border_1()
                        .border_color(tokens.border_primary)
                        .child(message_input.clone()),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .id("create-thread-cancel")
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .text_sm()
                                .text_color(tokens.text_theme_primary)
                                .hover(|s| s.bg(tokens.bg_item_hover))
                                .child(mezon_i18n::t(locale, "common.cancel"))
                                .on_click(move |_: &ClickEvent, _window, cx| {
                                    layout_footer_cancel.update(cx, |layout, cx| {
                                        layout.close_create_thread(cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("create-thread-send")
                                .label(mezon_i18n::t(locale, "channelTopbar.threads.createThread"))
                                .primary()
                                .loading(submitting)
                                .disabled(submitting)
                                .on_click(move |_: &ClickEvent, window, cx| {
                                    send_layout.update(cx, |layout, cx| {
                                        layout.submit_create_thread(window, cx);
                                    });
                                }),
                        ),
                ),
        )
        .into_any_element()
}
