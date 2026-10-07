use gpui::{App, ElementId, SharedString, Window, div, prelude::*, px, svg};

use super::stack::h_flex;
use crate::theme::ActiveTheme;

type ToggleHandler = Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    disabled: bool,
    label: Option<SharedString>,
    on_click: Option<ToggleHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            checked: false,
            disabled: false,
            label: None,
            on_click: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let checked = self.checked;
        let enabled = !self.disabled;
        let border = if checked {
            theme.brand
        } else {
            theme.interactive_normal
        };

        let box_el = div()
            .size(px(18.))
            .rounded(px(4.))
            .border_1()
            .border_color(border)
            .flex()
            .items_center()
            .justify_center()
            .when(checked, |el| el.bg(theme.brand))
            .when(checked, |el| {
                el.child(
                    svg()
                        .path("icons/check.svg")
                        .size(px(12.))
                        .flex_none()
                        .text_color(gpui::white()),
                )
            });

        h_flex()
            .id(self.id)
            .gap_2()
            .items_center()
            .text_sm()
            .text_color(theme.text_primary)
            .when(enabled, |el| el.cursor_pointer())
            .when(!enabled, |el| el.opacity(0.6))
            .when_some(self.on_click.filter(|_| enabled), |el, handler| {
                el.on_click(move |_, window, cx| handler(&!checked, window, cx))
            })
            .child(box_el)
            .when_some(self.label, |el, label| el.child(label))
    }
}

#[derive(IntoElement)]
pub struct Radio {
    id: ElementId,
    selected: bool,
    disabled: bool,
    label: Option<SharedString>,
    on_click: Option<ToggleHandler>,
}

impl Radio {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            selected: false,
            disabled: false,
            label: None,
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Radio {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.selected;
        let enabled = !self.disabled;
        let border = if selected {
            theme.brand
        } else {
            theme.interactive_normal
        };

        let circle = div()
            .size(px(18.))
            .rounded_full()
            .border_1()
            .border_color(border)
            .flex()
            .items_center()
            .justify_center()
            .when(selected, |el| {
                el.child(div().size(px(8.)).rounded_full().bg(theme.brand))
            });

        h_flex()
            .id(self.id)
            .gap_2()
            .items_center()
            .text_sm()
            .text_color(theme.text_primary)
            .when(enabled, |el| el.cursor_pointer())
            .when(!enabled, |el| el.opacity(0.6))
            .when_some(self.on_click.filter(|_| enabled), |el, handler| {
                el.on_click(move |_, window, cx| handler(&!selected, window, cx))
            })
            .child(circle)
            .when_some(self.label, |el, label| el.child(label))
    }
}
