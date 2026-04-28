use crate::theme::Theme;
use gpui::{div, prelude::*, rgb, Context, MouseButton, Window};

/// Custom frameless title bar.
pub struct TitleBar {
    title: String,
}

impl TitleBar {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }
}

impl Render for TitleBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let title = self.title.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h_8()
            .bg(theme.title_bar_bg)
            // Drag region — move window by dragging the title bar
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                window.start_window_move();
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .child(div().size_4().bg(theme.brand).rounded_sm())
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_secondary)
                            .child(title),
                    ),
            )
            .child(div().flex_1())
            // Window controls (Windows/Linux only — macOS hides traffic lights)
            .when(cfg!(not(target_os = "macos")), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h_full()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w_11()
                                .h_full()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .hover(move |s| s.bg(theme.bg_hover))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, window, _| {
                                    window.minimize_window();
                                })
                                .child("─"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w_11()
                                .h_full()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .hover(move |s| s.bg(theme.bg_hover))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, window, _| {
                                    window.zoom_window();
                                })
                                .child("□"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w_11()
                                .h_full()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .hover(|s| s.bg(rgb(0xc42b1c)))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, window, _| {
                                    window.remove_window();
                                })
                                .child("✕"),
                        ),
                )
            })
    }
}
