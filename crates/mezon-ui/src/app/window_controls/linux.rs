use gpui::{
    AnyElement, App, Context, MouseButton, Pixels, Rgba, StyleRefinement, Window, prelude::*, px,
};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

use super::{
    control_button, controls_row, hide_main_window, window_control_hover_bg,
    window_control_icon_color,
};

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
            move |style| style.bg(hover_bg),
            |window, _| window.minimize_window(),
            cx,
        ))
        .child(window_control_button(
            "window-control-max",
            color,
            icon_size,
            zoom_icon,
            move |style| style.bg(hover_bg),
            |window, _| window.zoom_window(),
            cx,
        ))
        .child(window_control_button(
            "window-control-close",
            color,
            icon_size,
            IconName::Close,
            move |style| style.bg(hover_bg),
            |window, cx| {
                let is_main = crate::app::main_window::handle(cx) == Some(window.window_handle());
                if !is_main || !hide_main_window(window, cx) {
                    window.remove_window();
                }
            },
            cx,
        ))
}

fn window_control_button<V: 'static>(
    id: &'static str,
    color: Rgba,
    icon_size: Pixels,
    icon: IconName,
    hover: impl FnOnce(StyleRefinement) -> StyleRefinement + 'static,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    control_button(color)
        .id(id)
        .hover(hover)
        .on_hover(cx.listener(|_, _, _, cx| cx.notify()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
        .child(Icon::new(icon).size(icon_size).text_color(color))
        .into_any_element()
}
