/// EmptyState — icon + title + subtitle + optional action button.
use std::sync::Arc;

use gpui::{App, Context, FontWeight, Window, div, prelude::*};

use crate::components::WindowAction;
use crate::components::primitives::{Button, ButtonSize, ButtonVariant, Icon, IconName};
use crate::theme::Theme;

pub struct EmptyState {
    icon: Option<IconName>,
    title: String,
    subtitle: Option<String>,
    action_label: Option<String>,
    on_action: Option<WindowAction>,
}

impl EmptyState {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            icon: None,
            title: title.into(),
            subtitle: None,
            action_label: None,
            on_action: None,
        }
    }

    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }

    pub fn subtitle(mut self, text: impl Into<String>) -> Self {
        self.subtitle = Some(text.into());
        self
    }

    pub fn action(
        mut self,
        label: impl Into<String>,
        handler: impl Fn(&mut Window, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.action_label = Some(label.into());
        self.on_action = Some(Arc::new(handler));
        self
    }
}

impl Render for EmptyState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();

        let mut content = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8();

        if let Some(icon_name) = self.icon {
            content = content.child(
                div()
                    .size_12()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_muted)
                    .child(Icon::new(icon_name).size(48.0).muted(&theme).render(&theme)),
            );
        }

        content = content.child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(self.title.clone()),
        );

        if let Some(sub) = &self.subtitle {
            content = content.child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .text_center()
                    .child(sub.clone()),
            );
        }

        if let Some(label) = &self.action_label {
            let btn = Button::new(label.clone())
                .variant(ButtonVariant::Primary)
                .size(ButtonSize::Sm);
            let btn = if let Some(handler) = self.on_action.clone() {
                btn.on_click(move |w, cx| handler(w, cx))
            } else {
                btn
            };
            content = content.child(btn.render(&theme));
        }

        content
    }
}
