use gpui::{App, Context, FontWeight, MouseButton, Window, div, prelude::*};

use crate::components::primitives::{Icon, IconName};
use crate::components::{ToggleHandler, WindowAction};
use crate::theme::Theme;

pub struct SectionHeader {
    label: String,
    collapsible: bool,
    collapsed: bool,
    trailing_icon: Option<IconName>,
    on_toggle: Option<ToggleHandler>,
    on_action: Option<WindowAction>,
}

impl SectionHeader {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            collapsible: false,
            collapsed: false,
            trailing_icon: None,
            on_toggle: None,
            on_action: None,
        }
    }

    pub fn collapsible(mut self) -> Self {
        self.collapsible = true;
        self
    }

    pub fn collapsed(mut self, c: bool) -> Self {
        self.collapsed = c;
        self
    }

    pub fn trailing_icon(mut self, icon: IconName) -> Self {
        self.trailing_icon = Some(icon);
        self
    }

    pub fn on_toggle(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.on_toggle = Some(std::sync::Arc::new(handler));
        self
    }

    pub fn on_action(
        mut self,
        handler: impl Fn(&mut Window, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.on_action = Some(std::sync::Arc::new(handler));
        self
    }
}

impl Render for SectionHeader {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let label = self.label.clone();
        let collapsible = self.collapsible;
        let collapsed = self.collapsed;
        let on_toggle = self.on_toggle.clone();
        let on_action = self.on_action.clone();
        let trailing_icon = self.trailing_icon;

        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .w_full()
            .rounded_sm()
            .hover(|s| s.bg(theme.bg_hover));

        // Left: chevron (if collapsible) + label
        let chevron_icon = if collapsible {
            let icon_name = if collapsed {
                IconName::ArrowRight
            } else {
                IconName::ArrowDown
            };
            Some(
                Icon::new(icon_name)
                    .size(10.0)
                    .color(theme.text_muted)
                    .render(&theme),
            )
        } else {
            None
        };

        let mut left = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .flex_1()
            .cursor_pointer();

        if collapsible {
            let toggle = on_toggle.clone();
            left = left.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(handler) = &toggle {
                    handler(!collapsed, window, cx);
                }
            });
        }

        if let Some(ch) = chevron_icon {
            left = left.child(ch);
        }

        left = left.child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(label.to_uppercase()),
        );

        row = row.child(left);

        // Right: trailing icon action button
        if let Some(icon_name) = trailing_icon {
            let action = on_action.clone();
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(gpui::px(16.0))
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        if let Some(handler) = &action {
                            handler(window, cx);
                        }
                    })
                    .child(
                        Icon::new(icon_name)
                            .size(12.0)
                            .color(theme.text_muted)
                            .render(&theme),
                    ),
            );
        }

        row
    }
}
