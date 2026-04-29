use gpui::{App, Context, ElementId, Window, div, prelude::*};

use crate::components::WindowAction;
use crate::components::primitives::{ButtonSize, ButtonVariant, Icon, IconName};
use crate::theme::Theme;

pub struct IconButton {
    id: ElementId,
    icon: IconName,
    size: ButtonSize,
    variant: ButtonVariant,
    tooltip: Option<String>,
    disabled: bool,
    on_click: Option<WindowAction>,
}

impl IconButton {
    #[track_caller]
    pub fn new(icon: IconName) -> Self {
        Self {
            id: std::panic::Location::caller().into(),
            icon,
            size: ButtonSize::Sm,
            variant: ButtonVariant::Ghost,
            tooltip: None,
            disabled: false,
            on_click: None,
        }
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&mut Window, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.on_click = Some(std::sync::Arc::new(handler));
        self
    }
}

impl Render for IconButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();

        let icon_size = match self.size {
            ButtonSize::Xs => 14.0_f32,
            ButtonSize::Sm => 16.0,
            ButtonSize::Md => 20.0,
            ButtonSize::Lg => 24.0,
        };

        let btn_size = gpui::px(match self.size {
            ButtonSize::Xs => 24.0,
            ButtonSize::Sm => 32.0,
            ButtonSize::Md => 36.0,
            ButtonSize::Lg => 44.0,
        });

        let white = gpui::Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let (bg_hover, text_color) = match self.variant {
            ButtonVariant::Primary => (theme.brand_hover, white),
            ButtonVariant::Secondary => (theme.bg_hover, theme.text_primary),
            ButtonVariant::Ghost => (theme.bg_hover, theme.text_secondary),
            ButtonVariant::Danger => (theme.status_dnd, white),
        };

        let is_interactive = !self.disabled;
        let on_click = self.on_click.clone();

        let mut el = div()
            .flex()
            .items_center()
            .justify_center()
            .size(btn_size)
            .rounded_md()
            .text_color(text_color)
            .child(
                Icon::new(self.icon)
                    .size(icon_size)
                    .color(text_color)
                    .render(&theme),
            );

        if is_interactive {
            el = el.cursor_pointer().hover(move |s| s.bg(bg_hover));
        } else {
            el = el.opacity(0.5);
        }

        if is_interactive {
            let mut el = el.id(self.id.clone()).focusable();
            if let Some(handler) = on_click {
                el = el.on_click(move |event, window, cx| {
                    if event.standard_click() {
                        handler(window, cx);
                    }
                });
            }
            el.into_any_element()
        } else {
            el.into_any_element()
        }
    }
}
