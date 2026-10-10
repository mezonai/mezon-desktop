use gpui::{Div, FontWeight, ParentElement, Styled, div, px, rgb, white};

pub(crate) const BUZZ_LABEL: &str = "Buzz!!";
pub(crate) const BUZZ_COLOR: u32 = 0xef_44_44;

pub fn buzz_pill() -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .h(px(16.))
        .px(px(4.))
        .rounded(px(4.))
        .bg(rgb(BUZZ_COLOR))
        .text_color(white())
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(BUZZ_LABEL)
}
