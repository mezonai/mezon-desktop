use gpui::{Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px};

use crate::theme::ActiveTheme;

/// Colour of the line that shows where a dragged category would land.
pub(super) const DRAG_INDICATOR_COLOR: u32 = 0x3b82f6;

/// A category being dragged in the sidebar. `index` counts only the categories a user can
/// reorder, in the order they are drawn — Favourites is pinned and never takes part.
#[derive(Clone)]
pub(super) struct CategoryReorderDrag {
    pub(super) index: usize,
    pub(super) name: SharedString,
}

pub(super) struct CategoryDragPreview {
    pub(super) name: SharedString,
}

impl Render for CategoryDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(px(4.))
            .bg(theme.bg_tertiary)
            .text_sm()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_primary)
            .child(self.name.clone())
    }
}
