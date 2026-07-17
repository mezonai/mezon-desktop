use gpui::{App, SharedString, Window, div, prelude::*, px, relative};

use super::icon::{Icon, IconName};
use super::stack::{h_flex, v_flex};
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
}

impl Toast {
    pub fn new(message: impl Into<SharedString>) -> Self {
        Self {
            message: message.into(),
            kind: ToastKind::Info,
            progress: None,
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
            ToastKind::Error => (theme.status_dnd, IconName::TriangleAlert),
        };

        let track = theme.bg_tertiary;
        h_flex()
            .gap_2()
            .items_start()
            .min_w(px(240.))
            .max_w(px(360.))
            .px(px(12.))
            .py(px(10.))
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(Icon::new(icon).size_4().text_color(accent))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(self.message),
                    )
                    .when_some(self.progress, |el, progress| {
                        el.child(
                            div()
                                .w_full()
                                .h(px(4.))
                                .rounded_full()
                                .overflow_hidden()
                                .bg(track)
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(progress.clamp(0., 1.)))
                                        .rounded_full()
                                        .bg(accent),
                                ),
                        )
                    }),
            )
    }
}
