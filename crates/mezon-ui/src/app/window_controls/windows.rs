use gpui::{
    AnyElement, Context, Pixels, Rgba, StyleRefinement, Window, WindowControlArea, prelude::*, px,
};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

use super::{control_button, controls_row, window_control_hover_bg, window_control_icon_color};

pub fn render_controls<V: 'static>(
    theme: &Theme,
    window: &Window,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let hover_bg = window_control_hover_bg(theme);
    let color = window_control_icon_color(theme);
    let icon_size = px(super::CONTROL_ICON_SIZE);
    let zoom_icon = if window.is_maximized() {
        IconName::WindowRestore
    } else {
        IconName::WindowMaximize
    };

    controls_row()
        .child(window_control_button(
            "window-control-min",
            color,
            icon_size,
            IconName::WindowMinimize,
            WindowControlArea::Min,
            move |style| style.bg(hover_bg),
            cx,
        ))
        .child(window_control_button(
            "window-control-max",
            color,
            icon_size,
            zoom_icon,
            WindowControlArea::Max,
            move |style| style.bg(hover_bg),
            cx,
        ))
        .child(window_control_button(
            "window-control-close",
            color,
            icon_size,
            IconName::Close,
            WindowControlArea::Close,
            move |style| style.bg(hover_bg),
            cx,
        ))
}

fn window_control_button<V: 'static>(
    id: &'static str,
    color: Rgba,
    icon_size: Pixels,
    icon: IconName,
    area: WindowControlArea,
    hover: impl FnOnce(StyleRefinement) -> StyleRefinement + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    control_button(color)
        .id(id)
        .hover(hover)
        .on_hover(cx.listener(|_, _, _, cx| cx.notify()))
        .window_control_area(area)
        .child(Icon::new(icon).size(icon_size).text_color(color))
        .into_any_element()
}
