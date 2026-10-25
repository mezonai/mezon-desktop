use gpui::{App, Div, ElementId, FontWeight, SharedString, Stateful, div, prelude::*, px};

use super::Tooltip;
use crate::theme::ActiveTheme;

#[derive(Clone)]
pub(crate) struct MentionLabels {
    short: SharedString,
    full: SharedString,
}

impl MentionLabels {
    pub(crate) fn new(locale: &str) -> Self {
        Self {
            short: mezon_i18n::t(locale, "common.new").to_uppercase().into(),
            full: mezon_i18n::t(locale, "common.newMention")
                .to_uppercase()
                .into(),
        }
    }
}

pub(crate) fn mention_float_button(
    id: impl Into<ElementId>,
    labels: &MentionLabels,
    compact: bool,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .block_mouse_except_scroll()
        .max_w_full()
        .rounded_full()
        .bg(cx.theme().mention_badge)
        .text_color(gpui::white())
        .shadow_lg()
        .cursor_pointer()
        .when(compact, |el| {
            el.px_1p5()
                .py_0p5()
                .text_size(px(9.))
                .font_weight(FontWeight::BOLD)
                .hover(|style| style.opacity(0.9))
        })
        .when(!compact, |el| {
            el.px_2()
                .py_1()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .opacity(0.9)
                .hover(|style| style.opacity(0.95))
        })
        .tooltip(Tooltip::text(labels.full.clone()))
        .child(div().min_w_0().truncate().child(if compact {
            labels.short.clone()
        } else {
            labels.full.clone()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, ScrollDelta, ScrollWheelEvent, Window, point, size};
    use std::{cell::Cell, rc::Rc};

    struct ButtonHost {
        compact: bool,
        clicked: Rc<Cell<usize>>,
        behind: Rc<Cell<usize>>,
        scrolled: Rc<Cell<usize>>,
    }

    impl Render for ButtonHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let clicked = self.clicked.clone();
            let behind = self.behind.clone();
            let scrolled = self.scrolled.clone();
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .id("behind")
                        .size_full()
                        .on_click(move |_, _, _| behind.set(behind.get() + 1))
                        .on_scroll_wheel(move |_, _, _| scrolled.set(scrolled.get() + 1)),
                )
                .child(
                    div().absolute().top_0().left_0().child(
                        mention_float_button(
                            "test-mention",
                            &MentionLabels::new("en"),
                            self.compact,
                            cx,
                        )
                        .on_click(move |_, _, _| clicked.set(clicked.get() + 1)),
                    ),
                )
        }
    }

    #[gpui::test]
    fn both_surfaces_pass_wheel_and_block_click_through(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::set_theme(crate::theme::Theme::dark(), cx));
        for compact in [false, true] {
            let clicked = Rc::new(Cell::new(0));
            let behind = Rc::new(Cell::new(0));
            let scrolled = Rc::new(Cell::new(0));
            let (_host, cx) = cx.add_window_view(|_, _| ButtonHost {
                compact,
                clicked: clicked.clone(),
                behind: behind.clone(),
                scrolled: scrolled.clone(),
            });
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            cx.simulate_event(ScrollWheelEvent {
                position: point(px(4.), px(4.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
                ..Default::default()
            });
            assert_eq!(scrolled.get(), 1);
            cx.simulate_click(point(px(4.), px(4.)), gpui::Modifiers::none());
            assert_eq!(clicked.get(), 1);
            assert_eq!(behind.get(), 0);
        }
    }

    #[gpui::test]
    fn translated_compact_labels_fit_the_rail_in_all_themes(cx: &mut gpui::TestAppContext) {
        cx.update(crate::theme::init_theme_settings_provider);
        let cx = cx.add_empty_window();
        for theme in [
            crate::theme::Theme::dark(),
            crate::theme::Theme::light(),
            crate::theme::Theme::purple(),
        ] {
            cx.update(|_, cx| crate::theme::set_theme(theme, cx));
            for locale in [
                "en", "vi", "de", "fr", "ru", "blr", "es", "it", "jpn", "kr", "nl", "pl", "pt",
                "swe", "tt", "ukr",
            ] {
                let labels = MentionLabels::new(locale);
                assert_ne!(labels.short.as_ref(), "COMMON.NEW");
                let measured = Rc::new(Cell::new(px(0.)));
                let width = measured.clone();
                cx.draw(
                    point(px(0.), px(0.)),
                    size(
                        gpui::AvailableSpace::MaxContent,
                        gpui::AvailableSpace::MinContent,
                    ),
                    |_, cx| {
                        div()
                            .relative()
                            .font_family(crate::theme::ui_font_family(cx))
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(9.))
                            .child(labels.short.clone())
                            .child(
                                gpui::canvas(
                                    move |bounds, _, _| width.set(bounds.size.width),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                    },
                );
                assert!(
                    measured.get() > px(0.) && measured.get() + px(12.) <= px(72.),
                    "{locale}: {:?}",
                    measured.get()
                );
            }
        }
    }
}
