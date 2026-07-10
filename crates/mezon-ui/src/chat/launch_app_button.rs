use crate::theme::Theme;
use gpui::{
    App, AppContext, BackgroundExecutor, Bounds, Context, Corners, Entity, FocusHandle, Focusable,
    FontWeight, ImageCache, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit,
    Pixels, Point, Render, RenderImage, Resource, ScrollDelta, ScrollWheelEvent, SharedString,
    SharedUri, Subscription, TitlebarOptions, UniformListScrollHandle, Window, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, canvas, div, img, point, prelude::*, px, relative,
    rgb, size, uniform_list,
};
use std::process::Command;

pub struct EmptyWindow {}

impl Render for EmptyWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        div().child("Cửa sổ mới trống")
    }
}

pub struct LaunchAppButton {}

impl Default for LaunchAppButton {
    fn default() -> Self {
        Self::new()
    }
}

impl LaunchAppButton {
    pub fn new() -> Self {
        Self {}
    }

    pub fn open_app_window(cx: &mut App) {
        let url = "https://www.youtube.com";

        #[cfg(target_os = "windows")]
        {
            use std::process::Command;

            Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn()
                .unwrap();
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open").arg(url).spawn().unwrap();
        }

        #[cfg(target_os = "linux")]
        {
            Command::new("xdg-open").arg(url).spawn().unwrap();
        }
    }

    pub fn render(&self, theme: &Theme) -> impl IntoElement {
        div()
            .flex()
            .h(px(50.0))
            .gap(px(12.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.text_secondary)
            .w_full()
            .px(px(12.0))
            // 👈 THAY THẾ: Thay .children(vec![...]) bằng các phương thức .child(...) riêng biệt
            .child(
                div()
                    .flex_1()
                    .h(px(40.0))
                    .border_1()
                    .flex()
                    .gap(px(4.0))
                    .items_center()
                    .justify_center()
                    .bg(rgb(0x222222))
                    .py(px(8.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .child(div().child("🎮"))
                    .child(div().child("Khởi chạy ứng dụng"))
                    .id("launch_app_button")
                    .on_click(|_, _, cx| {
                        Self::open_app_window(cx);
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(40.0))
                    .border_1()
                    .flex()
                    .gap(px(4.0))
                    .items_center()
                    .justify_center()
                    .bg(rgb(0x222222))
                    .py(px(8.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .child(div().child("📖"))
                    .child(div().child("Trợ giúp"))
                    .id("help_button")
                    .on_click(|_, _, _cx| {
                        // Xử lý nút trợ giúp tại đây nếu cần
                    }),
            )
    }
}
