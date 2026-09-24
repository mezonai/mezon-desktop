use std::time::{Duration, Instant};

use gpui::{
    App, ClipboardItem, Context, ElementId, Hsla, IntoElement, Pixels, RenderOnce, SharedString,
    Task, Window, div, prelude::*, px,
};

use crate::components::primitives::{Icon, IconName};

const COPIED_DURATION: Duration = Duration::from_secs(5);
const COPY_ICON_SIZE: Pixels = px(16.);
const COPIED_ICON_SIZE: Pixels = px(20.);

pub struct CopyButtonState {
    copied_at: Option<Instant>,
    _reset: Option<Task<()>>,
}

impl CopyButtonState {
    fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            copied_at: None,
            _reset: None,
        }
    }

    fn is_copied(&self) -> bool {
        self.copied_at
            .is_some_and(|at| at.elapsed() < COPIED_DURATION)
    }

    fn mark_copied(&mut self, cx: &mut Context<Self>) {
        self.copied_at = Some(Instant::now());
        self._reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_DURATION).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        }));
        cx.notify();
    }
}

#[derive(IntoElement)]
pub struct CopyButton {
    id: ElementId,
    text: SharedString,
    color: Hsla,
}

impl CopyButton {
    pub fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        color: impl Into<Hsla>,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            color: color.into(),
        }
    }
}

impl RenderOnce for CopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, CopyButtonState::new);
        let (icon, size) = if state.read(cx).is_copied() {
            (IconName::PasteIcon, COPIED_ICON_SIZE)
        } else {
            (IconName::CopyIcon, COPY_ICON_SIZE)
        };
        let text = self.text;

        div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .occlude()
            .on_click(move |_, _, cx| {
                if text.is_empty() {
                    return;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                state.update(cx, |state, cx| state.mark_copied(cx));
            })
            .child(Icon::new(icon).size(size).text_color(self.color))
    }
}
