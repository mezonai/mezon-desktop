use gpui::{
    App, ClickEvent, Context, DismissEvent, Entity, FocusHandle, Focusable, FontWeight,
    MouseButton, SharedString, Subscription, Window, deferred, div, img, prelude::*, px, relative,
};
use mezon_store::MessagesStore;

use super::{ReactionPicker, ReactionPickerEvent};
use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
};
use crate::theme::ActiveTheme;

const MIN_ANSWERS: usize = 2;
const MAX_ANSWERS: usize = 20;
const MAX_QUESTION_LEN: usize = 300;

const DURATION_OPTIONS: [(&str, &str); 6] = [
    ("1", "message.poll.duration1Hour"),
    ("4", "message.poll.duration4Hours"),
    ("8", "message.poll.duration8Hours"),
    ("24", "message.poll.duration24Hours"),
    ("72", "message.poll.duration3Days"),
    ("168", "message.poll.duration1Week"),
];

pub struct CreatePollModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    question: Entity<InputState>,
    answers: Vec<Entity<InputState>>,
    answer_emoji_ids: Vec<Option<SharedString>>,
    emoji_picker: Option<(usize, Entity<ReactionPicker>)>,
    duration: SharedString,
    allow_multiple: bool,
    duration_open: bool,
    _question_sub: Subscription,
    answer_subs: Vec<Subscription>,
    emoji_subs: Vec<Subscription>,
}

impl Focusable for CreatePollModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn new_answer_input(
    locale: &SharedString,
    window: &mut Window,
    cx: &mut Context<CreatePollModal>,
) -> Entity<InputState> {
    let placeholder = mezon_i18n::t(locale, "message.poll.answerPlaceholder").to_string();
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .padding_x(px(40.))
            .padding_right(px(12.))
    })
}

fn answer_change_sub(
    input: &Entity<InputState>,
    cx: &mut Context<CreatePollModal>,
) -> Subscription {
    cx.subscribe(input, |_this, _input, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        }
    })
}

fn format_answer_label(text: &str, emoji_id: Option<&str>) -> String {
    match emoji_id {
        Some(emoji_id) => format!("[e:{emoji_id}] {text}"),
        None => text.to_string(),
    }
}

impl CreatePollModal {
    pub fn open(locale: SharedString, window: &mut Window, cx: &mut App) {
        if Shell::global(cx).read(cx).has_modal() {
            return;
        }
        let view = cx.new(|cx| {
            let question_placeholder =
                mezon_i18n::t(&locale, "message.poll.questionPlaceholder").to_string();
            let question = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(question_placeholder)
                    .validate(|value, _cx| value.chars().count() <= MAX_QUESTION_LEN)
            });
            let question_sub = cx.subscribe(&question, |_this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
            let answers = vec![
                new_answer_input(&locale, window, cx),
                new_answer_input(&locale, window, cx),
            ];
            let answer_subs = answers.iter().map(|a| answer_change_sub(a, cx)).collect();
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                question,
                answer_emoji_ids: vec![None; answers.len()],
                answers,
                emoji_picker: None,
                duration: "24".into(),
                allow_multiple: false,
                duration_open: false,
                _question_sub: question_sub,
                answer_subs,
                emoji_subs: Vec::new(),
            }
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn add_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.answers.len() >= MAX_ANSWERS {
            return;
        }
        let locale = self.locale.clone();
        let input = new_answer_input(&locale, window, cx);
        self.answer_subs.push(answer_change_sub(&input, cx));
        self.answers.push(input);
        self.answer_emoji_ids.push(None);
        cx.notify();
    }

    fn remove_answer(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.answers.len() <= MIN_ANSWERS || index >= self.answers.len() {
            return;
        }
        self.answers.remove(index);
        self.answer_emoji_ids.remove(index);
        drop(self.answer_subs.remove(index));
        match self.emoji_picker.take() {
            Some((picker_index, picker)) if picker_index != index => {
                let shifted = if picker_index > index {
                    picker_index - 1
                } else {
                    picker_index
                };
                self.emoji_picker = Some((shifted, picker));
            }
            Some(_) => self.emoji_subs.clear(),
            None => {}
        }
        cx.notify();
    }

    fn toggle_emoji_picker(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.emoji_picker.as_ref().map(|(i, _)| *i) == Some(index) {
            self.close_emoji_picker(cx);
            return;
        }
        let picker = cx.new(|cx| ReactionPicker::new(window, cx));
        let focus_handle = picker.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        self.emoji_subs = vec![
            cx.subscribe(
                &picker,
                move |this, _picker, event: &ReactionPickerEvent, cx| {
                    let ReactionPickerEvent::Picked { emoji_id, .. } = event;
                    this.set_answer_emoji(index, emoji_id.clone(), cx);
                },
            ),
            cx.subscribe(&picker, |this, _picker, _: &DismissEvent, cx| {
                this.close_emoji_picker(cx)
            }),
        ];
        self.emoji_picker = Some((index, picker));
        cx.notify();
    }

    fn set_answer_emoji(&mut self, index: usize, emoji_id: String, cx: &mut Context<Self>) {
        if let Some(slot) = self.answer_emoji_ids.get_mut(index) {
            *slot = Some(emoji_id.into());
        }
        self.close_emoji_picker(cx);
    }

    fn close_emoji_picker(&mut self, cx: &mut Context<Self>) {
        self.emoji_picker = None;
        self.emoji_subs.clear();
        cx.notify();
    }

    fn non_empty_answers(&self, cx: &App) -> Vec<String> {
        self.answers
            .iter()
            .enumerate()
            .filter_map(|(index, input)| {
                let text = input.read(cx).value().trim();
                if text.is_empty() {
                    return None;
                }
                let emoji_id = self.answer_emoji_ids.get(index).and_then(Option::as_ref);
                Some(format_answer_label(
                    text,
                    emoji_id.map(SharedString::as_ref),
                ))
            })
            .collect()
    }

    fn can_post(&self, cx: &App) -> bool {
        !self.question.read(cx).value().trim().is_empty()
            && self.non_empty_answers(cx).len() >= MIN_ANSWERS
    }

    fn post(&mut self, cx: &mut Context<Self>) {
        if !self.can_post(cx) {
            return;
        }
        let question = self.question.read(cx).value().trim().to_string();
        let answers = self.non_empty_answers(cx);
        let expire_hours = self.duration.parse::<i32>().unwrap_or(24);
        let poll_type = if self.allow_multiple { 1 } else { 0 };
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.create_poll(question, answers, expire_hours, poll_type, cx);
        });
        Self::close(cx);
    }

    fn duration_label(&self) -> &'static str {
        DURATION_OPTIONS
            .iter()
            .find(|(value, _)| *value == self.duration.as_ref())
            .map_or("message.poll.duration24Hours", |(_, key)| *key)
    }
}

impl Render for CreatePollModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let t = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let entity = cx.entity();

        let question_len = self.question.read(cx).value().chars().count();
        let can_post = self.can_post(cx);
        let answer_count = self.answers.len();

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p_4()
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(t("message.poll.createTitle")),
            )
            .child(
                div()
                    .id("poll-create-close")
                    .size_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(20.))
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            );

        let question_section = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(t("message.poll.question")),
            )
            .child(
                Input::new(&self.question)
                    .w_full()
                    .text_color(theme.tokens.text_secondary),
            )
            .child(
                div()
                    .text_right()
                    .text_xs()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(format!("{question_len} / {MAX_QUESTION_LEN}")),
            );

        let mut answer_rows = div()
            .id("poll-answers-scroll")
            .flex()
            .flex_col()
            .gap_2()
            .pr_1()
            .min_h_0()
            .max_h(px(280.))
            .overflow_y_scroll();
        for index in 0..answer_count {
            let input = &self.answers[index];
            let emoji_id = self
                .answer_emoji_ids
                .get(index)
                .and_then(Option::as_ref)
                .cloned();
            let emoji_glyph = match &emoji_id {
                Some(id) => img(crate::util::imgproxy::emoji_url(cx, id))
                    .size(px(20.))
                    .flex_none()
                    .into_any_element(),
                None => Icon::new(IconName::Smile)
                    .size(px(20.))
                    .flex_none()
                    .text_color(theme.tokens.text_theme_primary)
                    .hover(|s| s.text_color(theme.tokens.text_secondary))
                    .into_any_element(),
            };
            let mut input_wrap = div()
                .relative()
                .flex_1()
                .min_w_0()
                .child(
                    Input::new(input)
                        .w_full()
                        .text_color(theme.tokens.text_secondary),
                )
                .child(
                    div()
                        .id(("poll-answer-emoji", index))
                        .absolute()
                        .left(px(12.))
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.toggle_emoji_picker(index, window, cx);
                        }))
                        .child(emoji_glyph),
                );
            if let Some((picker_index, picker)) = &self.emoji_picker
                && *picker_index == index
            {
                input_wrap = input_wrap.child(deferred(
                    div()
                        .absolute()
                        .top_full()
                        .left_0()
                        .mt_1()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.close_emoji_picker(cx);
                        }))
                        .child(picker.clone()),
                ));
            }
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(input_wrap);
            if answer_count > MIN_ANSWERS {
                row = row.child(
                    div()
                        .id(("poll-remove-answer", index))
                        .size_6()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.tokens.bg_item_hover))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.remove_answer(index, cx);
                        }))
                        .child(
                            Icon::new(IconName::TrashIcon)
                                .size(px(18.))
                                .text_color(theme.tokens.text_theme_primary),
                        ),
                );
            }
            answer_rows = answer_rows.child(row);
        }

        let mut answers_section = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(t("message.poll.answers")),
            )
            .child(answer_rows);
        if answer_count < MAX_ANSWERS {
            answers_section = answers_section.child(
                div()
                    .id("poll-add-answer")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .mt_1()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .hover(|s| s.text_color(theme.tokens.text_secondary))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.add_answer(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::AddIcon)
                            .size(px(16.))
                            .flex_none()
                            .text_color(theme.tokens.text_theme_primary)
                            .hover(|s| s.text_color(theme.tokens.text_secondary)),
                    )
                    .child(t("message.poll.addAnotherAnswer")),
            );
        }

        let mut duration_control = div()
            .relative()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.duration_open {
                    this.duration_open = false;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("poll-duration-toggle")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.tokens.border_primary)
                    .bg(theme.tokens.bg_input_secondary)
                    .cursor_pointer()
                    .text_color(theme.tokens.text_secondary)
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.duration_open = !this.duration_open;
                        cx.notify();
                    }))
                    .child(t(self.duration_label()))
                    .child(
                        Icon::new(IconName::ArrowDown)
                            .size(px(18.))
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            );
        if self.duration_open {
            let mut menu = div()
                .id("poll-duration-menu")
                .absolute()
                .bottom_full()
                .left_0()
                .right_0()
                .mb_1()
                .flex()
                .flex_col()
                .min_h_0()
                .max_h(px(260.))
                .overflow_y_scroll()
                .rounded_md()
                .border_1()
                .border_color(theme.tokens.border_primary)
                .bg(theme.tokens.theme_setting_primary)
                .shadow_lg()
                .occlude();
            for (option_index, (value, label_key)) in DURATION_OPTIONS.into_iter().enumerate() {
                let value: SharedString = value.into();
                menu = menu.child(
                    div()
                        .id(("poll-duration-option", option_index))
                        .px_3()
                        .py_2()
                        .cursor_pointer()
                        .text_color(theme.tokens.text_secondary)
                        .hover(|s| s.bg(theme.tokens.bg_item_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.duration = value.clone();
                                this.duration_open = false;
                                cx.notify();
                            }),
                        )
                        .child(t(label_key)),
                );
            }
            duration_control = duration_control.child(deferred(menu));
        }

        let duration_section = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(t("message.poll.duration")),
            )
            .child(duration_control);

        let body = div()
            .flex()
            .flex_col()
            .gap_4()
            .px_4()
            .pb_2()
            .child(question_section)
            .child(answers_section)
            .child(duration_section);

        let allow_multiple = self.allow_multiple;
        let checkbox = div()
            .id("poll-allow-multiple")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.allow_multiple = !this.allow_multiple;
                cx.notify();
            }))
            .child(
                div()
                    .size_5()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.tokens.border_primary)
                    .when(allow_multiple, |d| {
                        d.bg(theme.tokens.bg_button_primary)
                            .border_color(theme.tokens.bg_button_primary)
                    })
                    .when(allow_multiple, |d| {
                        d.child(
                            Icon::new(IconName::Check)
                                .size(px(14.))
                                .text_color(theme.tokens.text_secondary),
                        )
                    }),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(t("message.poll.allowMultipleAnswers")),
            );

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p_4()
            .child(checkbox)
            .child(
                Button::new("poll-post")
                    .primary()
                    .label(t("message.poll.post"))
                    .disabled(!can_post)
                    .on_click(move |_: &ClickEvent, _window, cx| {
                        entity.update(cx, |this, cx| this.post(cx));
                    }),
            );

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.emoji_picker.is_some() {
                    this.close_emoji_picker(cx);
                    return;
                }
                if this.duration_open {
                    this.duration_open = false;
                    cx.notify();
                    return;
                }
                Self::close(cx);
            }))
            .occlude()
            .w(px(480.))
            .max_h(relative(0.85))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(header)
            .child(
                div()
                    .id("poll-body-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::format_answer_label;

    #[test]
    fn answer_label_plain_when_no_emoji() {
        assert_eq!(format_answer_label("Yes", None), "Yes");
    }

    #[test]
    fn answer_label_prefixes_emoji_token_like_react() {
        assert_eq!(
            format_answer_label("Yes", Some("1234567890")),
            "[e:1234567890] Yes"
        );
    }
}
