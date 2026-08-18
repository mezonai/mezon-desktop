use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, SharedString, Window, div, prelude::*, px, relative,
};

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
    countdown: Option<(usize, Duration)>,
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

    /// Drain the bar from full to empty over `ttl`. GPUI's animation clock drives it off the
    /// display refresh, so the toast never has to re-render the window to move the bar. `id`
    /// keys the animation state and must stay stable for the life of the toast.
    pub fn countdown(mut self, id: usize, ttl: Option<Duration>) -> Self {
        self.countdown = ttl.map(|ttl| (id, ttl));
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
        let fill = div().h_full().rounded_full().bg(accent);
        let bar = match (self.countdown, self.progress) {
            (Some((id, ttl)), _) => Some(
                fill.with_animation(("toast-countdown", id), Animation::new(ttl), |el, delta| {
                    el.w(relative(1. - delta))
                })
                .into_any_element(),
            ),
            (None, Some(progress)) => {
                Some(fill.w(relative(progress.clamp(0., 1.))).into_any_element())
            }
            (None, None) => None,
        };

        div()
            .relative()
            .w(px(360.))
            .max_w(px(360.))
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
            .when_some(bar, |card, bar| {
                card.child(
                    div()
                        .absolute()
                        .bottom(px(3.))
                        .left(px(4.))
                        .right(px(4.))
                        .h(px(3.))
                        .rounded_full()
                        .bg(theme.bg_tertiary)
                        .child(bar),
                )
            })
    }
}
