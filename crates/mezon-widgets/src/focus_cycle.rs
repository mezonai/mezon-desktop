use std::rc::Rc;

use gpui::{App, FocusHandle, InteractiveElement, KeyBinding, KeyContext, Window, actions};

pub const FORM_KEY_CONTEXT: &str = "MezonForm";

actions!(mezon_form, [FocusNextField, FocusPrevField]);

pub fn init(cx: &mut App) {
    cx.bind_keys(form_bindings());
}

fn form_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("tab", FocusNextField, Some(FORM_KEY_CONTEXT)),
        KeyBinding::new("shift-tab", FocusPrevField, Some(FORM_KEY_CONTEXT)),
    ]
}

fn next_field(current: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (current, forward) {
        (Some(index), true) => (index + 1) % len,
        (Some(index), false) => (index + len - 1) % len,
        (None, true) => 0,
        (None, false) => len - 1,
    })
}

pub fn cycle_focus(fields: &[FocusHandle], forward: bool, window: &mut Window, cx: &mut App) {
    let current = fields.iter().position(|field| field.is_focused(window));
    let Some(next) = next_field(current, fields.len(), forward) else {
        return;
    };
    window.focus(&fields[next], cx);
}

pub trait FocusCycle: InteractiveElement + Sized {
    fn focus_cycle(self, fields: impl IntoIterator<Item = FocusHandle>) -> Self {
        let fields: Vec<FocusHandle> = fields.into_iter().collect();
        if fields.len() < 2 {
            return self;
        }
        self.focus_cycle_with_context(KeyContext::default(), fields)
    }

    fn focus_cycle_with_context(
        self,
        context: impl TryInto<KeyContext>,
        fields: impl IntoIterator<Item = FocusHandle>,
    ) -> Self {
        let mut context = context.try_into().unwrap_or_default();
        let fields: Vec<FocusHandle> = fields.into_iter().collect();
        if fields.len() < 2 {
            return self.key_context(context);
        }
        context.add(FORM_KEY_CONTEXT);
        let forward: Rc<[FocusHandle]> = fields.into();
        let backward = forward.clone();
        self.key_context(context)
            .on_action(move |_: &FocusNextField, window, cx| {
                cycle_focus(&forward, true, window, cx);
            })
            .on_action(move |_: &FocusPrevField, window, cx| {
                cycle_focus(&backward, false, window, cx);
            })
    }
}

impl<E: InteractiveElement> FocusCycle for E {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputState;
    use gpui::{
        Context, Entity, Focusable, KeyContext, Render, ScrollHandle, TestAppContext,
        VisualTestContext, div, prelude::*, px,
    };

    #[test]
    fn tab_walks_the_fields_and_wraps_at_the_end() {
        assert_eq!(next_field(Some(0), 3, true), Some(1));
        assert_eq!(next_field(Some(2), 3, true), Some(0));
        assert_eq!(next_field(Some(0), 3, false), Some(2));
        assert_eq!(next_field(Some(1), 3, false), Some(0));
    }

    #[test]
    fn tab_enters_the_form_at_the_matching_end() {
        assert_eq!(next_field(None, 3, true), Some(0));
        assert_eq!(next_field(None, 3, false), Some(2));
    }

    #[test]
    fn a_form_without_fields_stays_put() {
        assert_eq!(next_field(None, 0, true), None);
        assert_eq!(next_field(Some(0), 0, false), None);
    }

    #[test]
    fn a_single_field_form_keeps_focus_where_it_is() {
        assert_eq!(next_field(Some(0), 1, true), Some(0));
        assert_eq!(next_field(Some(0), 1, false), Some(0));
    }

    struct TestForm {
        first: Entity<InputState>,
        second: Entity<InputState>,
    }

    impl Render for TestForm {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .focus_cycle([self.first.focus_handle(cx), self.second.focus_handle(cx)])
                .child(self.first.clone())
                .child(self.second.clone())
        }
    }

    #[gpui::test]
    fn tab_walks_a_drawn_form_and_shift_tab_walks_back(cx: &mut TestAppContext) {
        cx.update(|cx| {
            mezon_theme::set_theme(mezon_theme::resolve_theme("dark"), cx);
            crate::text_actions::init(cx);
            init(cx);
        });

        let window = cx.add_window(|window, cx| TestForm {
            first: cx.new(|cx| InputState::new(window, cx)),
            second: cx.new(|cx| InputState::new(window, cx)),
        });
        let (first, second) = window
            .update(cx, |form, _, cx| {
                (form.first.focus_handle(cx), form.second.focus_handle(cx))
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();

        cx.update(|window, cx| window.focus(&first, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert!(
            cx.update(|window, _| second.is_focused(window)),
            "tab must move focus to the second field"
        );

        cx.simulate_keystrokes("shift-tab");
        assert!(
            cx.update(|window, _| first.is_focused(window)),
            "shift-tab must move focus back to the first field"
        );

        cx.update(|window, cx| window.focus(&second, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert!(
            cx.update(|window, _| first.is_focused(window)),
            "tab must wrap from the last field to the first"
        );
    }

    struct FocusedCardForm {
        card: gpui::FocusHandle,
        first: Entity<InputState>,
        second: Entity<InputState>,
    }

    impl Render for FocusedCardForm {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.card)
                .focus_cycle_with_context(
                    "menu",
                    [self.first.focus_handle(cx), self.second.focus_handle(cx)],
                )
                .child(self.first.clone())
                .child(self.second.clone())
        }
    }

    #[gpui::test]
    fn tab_enters_the_form_from_its_focused_card_and_keeps_the_card_context(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            mezon_theme::set_theme(mezon_theme::resolve_theme("dark"), cx);
            crate::text_actions::init(cx);
            init(cx);
        });

        let window = cx.add_window(|window, cx| FocusedCardForm {
            card: cx.focus_handle(),
            first: cx.new(|cx| InputState::new(window, cx)),
            second: cx.new(|cx| InputState::new(window, cx)),
        });
        let (card, first) = window
            .update(cx, |form, _, cx| {
                (form.card.clone(), form.first.focus_handle(cx))
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();

        cx.update(|window, cx| window.focus(&card, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert!(
            cx.update(|window, _| first.is_focused(window)),
            "tab from the focused card must enter the form at its first field"
        );

        let mut expected = KeyContext::default();
        expected.add("menu");
        expected.add(FORM_KEY_CONTEXT);
        let card_context = cx.update(|window, _| {
            window
                .context_stack()
                .into_iter()
                .find(|context| context.contains("menu"))
        });
        assert_eq!(
            card_context,
            Some(expected),
            "the card must keep its menu context alongside the form context"
        );
    }

    struct ScrollingForm {
        scroll: ScrollHandle,
        far: Entity<InputState>,
    }

    impl Render for ScrollingForm {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("scrolling-form")
                .h(px(100.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(div().h(px(1000.)))
                .child(self.far.clone())
        }
    }

    #[gpui::test]
    fn focusing_a_field_below_the_fold_scrolls_it_into_view(cx: &mut TestAppContext) {
        cx.update(|cx| {
            mezon_theme::set_theme(mezon_theme::resolve_theme("dark"), cx);
            crate::text_actions::init(cx);
            init(cx);
        });

        let window = cx.add_window(|window, cx| ScrollingForm {
            scroll: ScrollHandle::new(),
            far: cx.new(|cx| InputState::new(window, cx)),
        });
        let (scroll, far) = window
            .update(cx, |form, _, cx| {
                (form.scroll.clone(), form.far.focus_handle(cx))
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        assert_eq!(scroll.offset().y, px(0.));

        cx.update(|window, cx| window.focus(&far, cx));
        cx.run_until_parked();
        assert!(
            scroll.offset().y < px(0.),
            "focusing a field below the fold must scroll its container to reveal it"
        );
    }

    #[test]
    fn tab_only_fires_inside_a_form() {
        let mut inside = KeyContext::default();
        inside.add(FORM_KEY_CONTEXT);
        let outside = KeyContext::default();

        for binding in form_bindings() {
            let name = binding.action().name().to_string();
            let predicate = binding
                .predicate()
                .unwrap_or_else(|| panic!("{name} must be scoped to a context"));
            assert!(
                predicate.eval(std::slice::from_ref(&inside)),
                "{name} must fire inside {FORM_KEY_CONTEXT}"
            );
            assert!(
                !predicate.eval(std::slice::from_ref(&outside)),
                "{name} must not fire outside {FORM_KEY_CONTEXT}"
            );
        }
    }
}
