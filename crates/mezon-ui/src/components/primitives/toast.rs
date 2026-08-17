use gpui::{App, SharedString, Window, div, prelude::*, px, relative};

use super::icon::{Icon, IconName};
use super::stack::h_flex;
use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastKind {
    #[default]
    Info,
    Success,
    Error,
}

#[derive(IntoElement)]
pub struct Toast {
    message: SharedString,
    kind: ToastKind,
    progress: Option<f32>,
    countdown: Option<f32>,
}

impl Toast {
    pub fn new(message: impl Into<SharedString>) -> Self {
        Self {
            message: message.into(),
            kind: ToastKind::Info,
            progress: None,
            countdown: None,
        }
    }

    pub fn kind(mut self, kind: ToastKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn progress(mut self, progress: Option<f32>) -> Self {
        self.progress = progress;
        self
    }

    pub fn countdown(mut self, progress: Option<f32>) -> Self {
        self.countdown = progress;
        self
    }

    pub fn success(message: impl Into<SharedString>) -> Self {
        Self::new(message).kind(ToastKind::Success)
    }

    pub fn error(message: impl Into<SharedString>) -> Self {
        Self::new(message).kind(ToastKind::Error)
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (accent, icon) = match self.kind {
            ToastKind::Info => (theme.text_link, IconName::Inbox),
            ToastKind::Success => (theme.status_online, IconName::Check),
            ToastKind::Error => (theme.danger_text, IconName::TriangleAlert),
        };
        let bar_progress = self.countdown.or(self.progress);

        div()
            .relative()
            .w(px(360.))
            .min_h(px(56.))
            .rounded(px(10.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_secondary)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .min_h(px(55.))
                    .items_center()
                    .gap(px(12.))
                    .pl(px(16.))
                    .pr(px(10.))
                    .py(px(11.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(20.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(accent)
                            .child(Icon::new(icon).size(px(13.)).text_color(theme.bg_floating)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(self.message),
                    ),
            )
            .when(bar_progress.is_some(), |card| {
                card.child(
                    div()
                        .absolute()
                        .bottom(px(3.))
                        .left(px(4.))
                        .right(px(4.))
                        .h(px(3.))
                        .rounded_full()
                        .bg(theme.bg_tertiary)
                        .child(
                            div()
                                .h_full()
                                .w(relative(bar_progress.unwrap_or_default().clamp(0., 1.)))
                                .rounded_full()
                                .bg(accent),
                        ),
                )
            })
    }
}
