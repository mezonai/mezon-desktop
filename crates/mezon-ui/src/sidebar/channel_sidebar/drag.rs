use gpui::{Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px};
use mezon_store::{ChannelId, SidebarOrderKey};

use crate::theme::ActiveTheme;

/// Colour of the line that shows where a dragged row would land.
pub(super) const DRAG_INDICATOR_COLOR: u32 = 0x3b82f6;

/// A category being dragged in the sidebar. `index` counts only the categories a user can
/// reorder, in the order they are drawn — Favourites is pinned and never takes part.
#[derive(Clone)]
pub(super) struct CategoryReorderDrag {
    pub(super) index: usize,
    pub(super) name: SharedString,
}

/// A channel or thread row being dragged in the sidebar, carried on the row itself so a drop
/// target can tell at a glance whether the two belong together.
#[derive(Clone, PartialEq)]
pub(super) struct ChannelReorderDrag {
    /// The list the row sits in. A drop only counts inside the same one: moving a channel to
    /// another category, or a thread to another channel, is the server's to record and this
    /// order is nobody's but this machine's.
    pub(super) key: SidebarOrderKey,
    /// Where the row sits among the rows of that list as drawn, which is what decides the
    /// edge the drop indicator goes on.
    pub(super) index: usize,
    pub(super) channel_id: ChannelId,
}

pub(super) struct RowDragPreview {
    pub(super) name: SharedString,
}

impl Render for RowDragPreview {
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
