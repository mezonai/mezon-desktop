use gpui::{div, prelude::*, px};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaginationButton {
    Previous,
    Next,
    Page(usize),
}

/// Slots the page-number strip draws once there are more pages than fit.
pub const PAGINATION_MAX_SLOTS: usize = 7;

/// How many slots [`pagination_items`] fills for `pages` pages. It depends on the
/// page count alone — never on the current page — so a caller can size the strip once
/// and paging only relabels slots instead of shifting what sits under the pointer.
pub fn pagination_slot_count(pages: usize) -> usize {
    pages.min(PAGINATION_MAX_SLOTS)
}

/// Page indices (0-based) to draw, `None` for an ellipsis. Past
/// [`PAGINATION_MAX_SLOTS`] pages this is MUI's rule with one boundary page and one
/// sibling on each side: always exactly seven entries, and an ellipsis never stands in
/// for a single page — that page is drawn instead.
pub fn pagination_items(current: usize, pages: usize) -> Vec<Option<usize>> {
    if pages <= PAGINATION_MAX_SLOTS {
        return (0..pages).map(Some).collect();
    }
    // 1-based from here on, as in MUI's usePagination.
    let page = current.min(pages - 1) + 1;
    let siblings_start = page.saturating_sub(1).min(pages - 4).max(3);
    let siblings_end = (page + 1).max(5).min(pages - 2);
    let mut items = Vec::with_capacity(PAGINATION_MAX_SLOTS);
    items.push(Some(0));
    items.push(if siblings_start > 3 { None } else { Some(1) });
    items.extend((siblings_start..=siblings_end).map(|page| Some(page - 1)));
    items.push(if siblings_end < pages - 2 {
        None
    } else {
        Some(pages - 2)
    });
    items.push(Some(pages - 1));
    items
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
            vec![Some(0), None, Some(5), Some(6), Some(7), Some(8), Some(9)]
        );
    }

    #[test]
    fn every_page_is_drawn_while_they_fit() {
        assert_eq!(pagination_items(0, 1), vec![Some(0)]);
        assert_eq!(pagination_items(3, 7), (0..7).map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn the_slot_count_never_depends_on_the_current_page() {
        for pages in [8, 9, 10, 50, 124] {
            for current in 0..pages {
                assert_eq!(
                    pagination_items(current, pages).len(),
                    pagination_slot_count(pages),
                    "page {current} of {pages}"
                );
            }
        }
    }

    #[test]
    fn head_middle_and_tail_follow_mui() {
        assert_eq!(
            pagination_items(0, 124),
            vec![Some(0), Some(1), Some(2), Some(3), Some(4), None, Some(123)]
        );
        assert_eq!(
            pagination_items(60, 124),
            vec![Some(0), None, Some(59), Some(60), Some(61), None, Some(123)]
        );
        assert_eq!(
            pagination_items(123, 124),
            vec![
                Some(0),
                None,
                Some(119),
                Some(120),
                Some(121),
                Some(122),
                Some(123)
            ]
        );
    }

    #[test]
    fn an_ellipsis_never_hides_just_one_page() {
        for pages in [8, 9, 10, 124] {
            for current in 0..pages {
                let items = pagination_items(current, pages);
                for window in items.windows(3) {
                    if let [Some(before), None, Some(after)] = window {
                        assert!(after - before > 2, "{items:?} (page {current} of {pages})");
                    }
                }
            }
        }
    }

    #[test]
    fn the_current_page_is_always_on_the_strip() {
        for pages in [8, 10, 124] {
            for current in 0..pages {
                assert!(pagination_items(current, pages).contains(&Some(current)));
            }
        }
    }
}
