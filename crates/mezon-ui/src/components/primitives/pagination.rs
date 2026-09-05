use gpui::{div, prelude::*, px};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaginationButton {
    Previous,
    Next,
    Page(usize),
}

pub fn pagination_items(current: usize, pages: usize) -> Vec<Option<usize>> {
    if pages <= 6 {
        return (0..pages).map(Some).collect();
    }
    if current <= 2 {
        let mut items = (0..5).map(Some).collect::<Vec<_>>();
        items.push(None);
        items.push(Some(pages - 1));
        return items;
    }
    if current >= pages - 3 {
        let mut items = vec![Some(0), None];
        items.extend(((pages - 6)..pages).map(Some));
        return items;
    }
    vec![
        Some(0),
        None,
        Some(current - 1),
        Some(current),
        Some(current + 1),
        None,
        Some(pages - 1),
    ]
}

pub fn pagination_button(
    id_prefix: &str,
    button: PaginationButton,
    disabled: bool,
    selected: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let id_suffix = match button {
        PaginationButton::Previous => "previous".to_string(),
        PaginationButton::Next => "next".to_string(),
        PaginationButton::Page(page) => format!("page-{page}"),
    };
    div()
        .id(format!("{id_prefix}-pagination-{id_suffix}"))
        .w(px(40.0))
        .h(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .border_1()
        .border_color(if selected {
            theme.text_primary
        } else {
            theme.border
        })
        .bg(if selected {
            theme.tokens.bg_active_button
        } else {
            theme.brand
        })
        .text_color(match button {
            PaginationButton::Page(_) => gpui::Hsla::from(theme.text_primary),
            PaginationButton::Previous | PaginationButton::Next => gpui::white(),
        })
        .when(disabled, |element| element.opacity(0.5))
        .when(!disabled, |element| element.cursor_pointer())
        .when_some(
            match button {
                PaginationButton::Page(page) => Some(page),
                _ => None,
            },
            |element, page| element.child(page.to_string()),
        )
        .when(
            matches!(button, PaginationButton::Previous | PaginationButton::Next),
            |element| {
                element.child(
                    Icon::new(IconName::ArrowRight)
                        .size(px(20.0))
                        .text_color(gpui::white())
                        .when(button == PaginationButton::Previous, |icon| {
                            icon.with_transformation(gpui::Transformation::rotate(gpui::radians(
                                std::f32::consts::PI,
                            )))
                        }),
                )
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_edges_match_management_lists() {
        assert_eq!(
            pagination_items(5, 6),
            vec![Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]
        );
        assert_eq!(
            pagination_items(0, 10),
            vec![Some(0), Some(1), Some(2), Some(3), Some(4), None, Some(9)]
        );
        assert_eq!(
            pagination_items(9, 10),
            vec![
                Some(0),
                None,
                Some(4),
                Some(5),
                Some(6),
                Some(7),
                Some(8),
                Some(9),
            ]
        );
    }
}
