use gpui::{
    AnyElement, InteractiveElement, SharedString, StatefulInteractiveElement, div, prelude::*, px,
};

use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use crate::theme::Theme;

fn required_label(label: SharedString, theme: &Theme) -> AnyElement {
    h_flex()
        .gap_1()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(label)
        .child(div().text_color(theme.danger_text).child("*"))
        .into_any_element()
}

pub(super) fn mission_editor_body(
    title_field: AnyElement,
    channel_label: SharedString,
    channel_select: AnyElement,
    channel_note: SharedString,
    complete_label: SharedString,
    radios: AnyElement,
    theme: &Theme,
) -> AnyElement {
    v_flex()
        .gap_4()
        .child(title_field)
        .child(div().border_t_1().border_color(theme.border))
        .child(required_label(channel_label, theme))
        .child(channel_select)
        .child(
            div()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(channel_note),
        )
        .child(div().border_t_1().border_color(theme.border))
        .child(required_label(complete_label, theme))
        .child(radios)
        .into_any_element()
}

pub(super) fn resource_editor_body(
    modal_title: SharedString,
    title_field: AnyElement,
    description_field: AnyElement,
    image_label: SharedString,
    browse_button: AnyElement,
    preview: Option<AnyElement>,
    theme: &Theme,
) -> AnyElement {
    v_flex()
        .gap_4()
        .child(
            div()
                .text_lg()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(modal_title),
        )
        .child(title_field)
        .child(div().border_t_1().border_color(theme.border))
        .child(description_field)
        .child(div().border_t_1().border_color(theme.border))
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(image_label),
        )
        .child(
            h_flex()
                .justify_between()
                .p_3()
                .rounded(px(6.0))
                .bg(theme.bg_secondary)
                .child(browse_button)
                .child(
                    div()
                        .size(px(44.0))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme.text_muted)
                        .flex()
                        .items_center()
                        .justify_center()
                        .when_some(preview, |container, preview| container.child(preview)),
                ),
        )
        .into_any_element()
}

pub(super) fn setup_modal(
    title: SharedString,
    body: AnyElement,
    footer: AnyElement,
    editor: Option<AnyElement>,
    width: f32,
    height: f32,
    theme: &Theme,
    on_close: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    v_flex()
        .w(px(width))
        .h(px(height))
        .rounded(px(8.0))
        .bg(theme.bg_primary)
        .shadow_lg()
        .child(
            h_flex()
                .w_full()
                .flex_shrink_0()
                .justify_between()
                .items_center()
                .px_5()
                .py_4()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .id("close-onboarding-setup")
                        .p_1()
                        .rounded_full()
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg_hover).text_color(theme.text_primary))
                        .child(
                            Icon::new(IconName::CloseIcon)
                                .size(px(20.0))
                                .text_color(theme.text_secondary),
                        )
                        .on_click(on_close),
                ),
        )
        .child(body)
        .child(footer)
        .when_some(editor, |modal, editor| modal.child(editor))
        .into_any_element()
}

pub(super) fn editor_modal(
    body: AnyElement,
    footer: AnyElement,
    theme: &Theme,
    on_close: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id("onboarding-editor-overlay")
        .absolute()
        .inset_0()
        .occlude()
        .bg(gpui::black().alpha(0.65))
        .flex()
        .items_center()
        .justify_center()
        .child(
            v_flex()
                .w(px(550.0))
                .max_h(px(700.0))
                .rounded(px(8.0))
                .bg(theme.bg_primary)
                .shadow_lg()
                .child(
                    h_flex().p_4().justify_end().child(
                        div()
                            .id("close-onboarding-editor")
                            .p_1()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg_hover))
                            .child(
                                Icon::new(IconName::CloseIcon)
                                    .size(px(20.0))
                                    .text_color(theme.text_secondary),
                            )
                            .on_click(on_close),
                    ),
                )
                .child(
                    div()
                        .id("onboarding-editor-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_5()
                        .pb_5()
                        .child(body),
                )
                .child(footer),
        )
        .into_any_element()
}
