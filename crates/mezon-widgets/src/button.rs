use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, Hsla, Pixels, SharedString, StyleRefinement,
    Window, div, prelude::*, px, transparent_black,
};

use super::sizing::{Sizable, Size};
use super::spinner::Spinner;
use mezon_theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
    Warning,
    Link,
}

pub trait ButtonVariants: Sized {
    fn with_variant(self, variant: ButtonVariant) -> Self;

    fn primary(self) -> Self {
        self.with_variant(ButtonVariant::Primary)
    }

    fn ghost(self) -> Self {
        self.with_variant(ButtonVariant::Ghost)
    }

    fn danger(self) -> Self {
        self.with_variant(ButtonVariant::Danger)
    }

    fn warning(self) -> Self {
        self.with_variant(ButtonVariant::Warning)
    }

    fn link(self) -> Self {
        self.with_variant(ButtonVariant::Link)
    }
}

struct ButtonLikeStyles {
    background: Hsla,
    label_color: Hsla,
}

fn darken(mut color: Hsla, amount: f32) -> Hsla {
    color.l = (color.l - amount).max(0.0);
    color
}

impl ButtonVariant {
    fn enabled(self, cx: &App) -> ButtonLikeStyles {
        let theme = cx.theme();
        match self {
            ButtonVariant::Primary => ButtonLikeStyles {
                background: theme.brand.into(),
                label_color: theme.text_primary.into(),
            },
            ButtonVariant::Secondary => ButtonLikeStyles {
                background: theme.bg_tertiary.into(),
                label_color: theme.text_primary.into(),
            },
            ButtonVariant::Ghost => ButtonLikeStyles {
                background: transparent_black(),
                label_color: theme.text_secondary.into(),
            },
            ButtonVariant::Danger => ButtonLikeStyles {
                background: theme.danger.into(),
                label_color: gpui::white(),
            },
            ButtonVariant::Warning => ButtonLikeStyles {
                background: theme.status_idle.into(),
                label_color: theme.text_primary.into(),
            },
            ButtonVariant::Link => ButtonLikeStyles {
                background: transparent_black(),
                label_color: theme.text_link.into(),
            },
        }
    }

    fn hovered(self, cx: &App) -> ButtonLikeStyles {
        let theme = cx.theme();
        match self {
            ButtonVariant::Primary => ButtonLikeStyles {
                background: theme.brand_hover.into(),
                label_color: theme.text_primary.into(),
            },
            ButtonVariant::Secondary => ButtonLikeStyles {
                background: theme.bg_hover.into(),
                label_color: theme.text_primary.into(),
            },
            ButtonVariant::Ghost => ButtonLikeStyles {
                background: theme.bg_hover.into(),
                label_color: theme.text_primary.into(),
            },
            ButtonVariant::Danger => ButtonLikeStyles {
                background: Hsla::from(theme.danger).opacity(0.85),
                label_color: gpui::white(),
            },
            ButtonVariant::Warning => ButtonLikeStyles {
                background: darken(theme.status_idle.into(), 0.06),
                label_color: theme.text_primary.into(),
            },
            ButtonVariant::Link => ButtonLikeStyles {
                background: theme.bg_hover.into(),
                label_color: theme.text_link.into(),
            },
        }
    }

    fn active(self, cx: &App) -> ButtonLikeStyles {
        let theme = cx.theme();
        match self {
            ButtonVariant::Primary => ButtonLikeStyles {
                background: darken(theme.brand_hover.into(), 0.04),
                label_color: theme.text_primary.into(),
            },
            ButtonVariant::Secondary | ButtonVariant::Ghost | ButtonVariant::Link => {
                ButtonLikeStyles {
                    background: darken(theme.bg_hover.into(), 0.04),
                    label_color: theme.text_primary.into(),
                }
            }
            ButtonVariant::Danger => ButtonLikeStyles {
                background: Hsla::from(theme.danger).opacity(0.85),
                label_color: gpui::white(),
            },
            ButtonVariant::Warning => ButtonLikeStyles {
                background: darken(theme.status_idle.into(), 0.1),
                label_color: theme.text_primary.into(),
            },
        }
    }
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Button {
    base: Div,
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<AnyElement>,
    variant: ButtonVariant,
    size: Size,
    disabled: bool,
    loading: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: div(),
            id: id.into(),
            label: None,
            icon: None,
            variant: ButtonVariant::default(),
            size: Size::Medium,
            disabled: false,
            loading: false,
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl ButtonVariants for Button {
    fn with_variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }
}

impl Sizable for Button {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

fn height(size: Size) -> Pixels {
    match size {
        Size::XSmall => px(20.),
        Size::Small => px(24.),
        Size::Medium => px(34.),
        Size::Large => px(40.),
    }
}

fn horizontal_padding(size: Size) -> Pixels {
    match size {
        Size::XSmall | Size::Small => px(8.),
        Size::Medium => px(10.),
        Size::Large => px(12.),
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let enabled = self.variant.enabled(cx);
        let hovered = self.variant.hovered(cx);
        let active = self.variant.active(cx);
        let interactive = self.on_click.is_some() && !self.disabled && !self.loading;
        let size = self.size;

        self.base
            .id(self.id)
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_center()
            .gap_1()
            .h(height(size))
            .px(horizontal_padding(size))
            .rounded_md()
            .text_sm()
            .bg(enabled.background)
            .text_color(enabled.label_color)
            .when(self.disabled || self.loading, |el| el.opacity(0.6))
            .when(interactive, |el| {
                el.cursor_pointer()
                    .hover(|style| style.bg(hovered.background))
                    .active(|style| style.bg(active.background))
            })
            .when_some(self.on_click.filter(|_| interactive), |el, handler| {
                el.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .when(self.loading, |el| el.child(Spinner::new().with_size(size)))
            .when_some(self.icon, |el, icon| el.child(icon))
            .when_some(self.label, |el, label| el.child(label))
    }
}
