use gpui::{AnyElement, App, ElementId, FontWeight, Window, div, prelude::*};

use super::icon::{Icon, IconName};
use super::spinner::Spinner;
use crate::components::WindowAction;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    Xs,
    Sm,
    Md,
    Lg,
}

pub struct Button {
    id: ElementId,
    label: String,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    loading: bool,
    full_width: bool,
    icon: Option<IconName>,
    on_click: Option<WindowAction>,
}

impl Button {
    #[track_caller]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            id: std::panic::Location::caller().into(),
            label: label.into(),
            variant: ButtonVariant::Primary,
            size: ButtonSize::Md,
            disabled: false,
            loading: false,
            full_width: false,
            icon: None,
            on_click: None,
        }
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }

    pub fn size(mut self, s: ButtonSize) -> Self {
        self.size = s;
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    pub fn loading(mut self, l: bool) -> Self {
        self.loading = l;
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&mut Window, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.on_click = Some(std::sync::Arc::new(handler));
        self
    }

    pub fn render(self, theme: &Theme) -> AnyElement {
        let white = gpui::Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let (bg, bg_hover, text_color, border) = match self.variant {
            ButtonVariant::Primary => (theme.brand, theme.brand_hover, white, None::<gpui::Rgba>),
            ButtonVariant::Secondary => (
                gpui::Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
                theme.bg_hover,
                theme.text_primary,
                Some(theme.border),
            ),
            ButtonVariant::Ghost => (
                gpui::Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
                theme.bg_hover,
                theme.text_secondary,
                None::<gpui::Rgba>,
            ),
            ButtonVariant::Danger => (
                theme.status_dnd,
                gpui::Rgba {
                    r: 0.85,
                    g: 0.2,
                    b: 0.2,
                    a: 1.0,
                },
                white,
                None::<gpui::Rgba>,
            ),
        };

        let (px_h, px_x, text_sz) = match self.size {
            ButtonSize::Xs => (gpui::px(4.0), gpui::px(8.0), 10.0_f32),
            ButtonSize::Sm => (gpui::px(6.0), gpui::px(12.0), 12.0),
            ButtonSize::Md => (gpui::px(8.0), gpui::px(16.0), 14.0),
            ButtonSize::Lg => (gpui::px(10.0), gpui::px(20.0), 16.0),
        };

        let is_interactive = !self.disabled && !self.loading;
        let label = self.label.clone();
        let icon_name = self.icon;
        let loading = self.loading;
        let on_click = self.on_click.clone();

        let mut el = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .py(px_h)
            .px(px_x)
            .rounded_md()
            .bg(bg)
            .text_color(text_color)
            .font_weight(FontWeight::MEDIUM)
            .text_size(gpui::px(text_sz));

        if let Some(b) = border {
            el = el.border_1().border_color(b);
        }

        if self.full_width {
            el = el.w_full();
        }

        if is_interactive {
            let bg_h = bg_hover;
            el = el.cursor_pointer().hover(move |s| s.bg(bg_h));
        } else {
            el = el.opacity(if self.disabled { 0.5 } else { 0.8 });
        }

        // Leading icon
        if let Some(name) = icon_name {
            let icon_color = text_color;
            el = el.child(
                Icon::new(name)
                    .size(text_sz + 2.0)
                    .color(icon_color)
                    .render(theme),
            );
        }

        // Spinner (loading) or label
        if loading {
            el = el.child(
                Spinner::new()
                    .size(text_sz as u16)
                    .color(text_color)
                    .render(),
            );
        } else {
            el = el.child(div().child(label));
        }

        if is_interactive {
            let mut el = el.id(self.id).focusable();
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
