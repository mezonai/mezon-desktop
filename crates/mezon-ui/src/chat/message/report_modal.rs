use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use mezon_store::{MessageId, MessagesStore};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
};
use crate::theme::ActiveTheme;

const REASONS: [&str; 7] = [
    "spam",
    "harassment",
    "hate_speech",
    "violence",
    "inappropriate_content",
    "scam",
    "other",
];

const REASON_KEYS: [&str; 7] = [
    "contextMenu.reportMessageModal.reasons.spam",
    "contextMenu.reportMessageModal.reasons.harassment",
    "contextMenu.reportMessageModal.reasons.hate_speech",
    "contextMenu.reportMessageModal.reasons.violence",
    "contextMenu.reportMessageModal.reasons.inappropriate_content",
    "contextMenu.reportMessageModal.reasons.scam",
    "contextMenu.reportMessageModal.reasons.other",
];

const CUSTOM_REASON_MAX: usize = 64;

pub struct ReportMessageModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    message_id: MessageId,
    selected: Option<usize>,
    custom_input: Entity<InputState>,
    submitting: bool,
    _custom_sub: Subscription,
}

impl Focusable for ReportMessageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ReportMessageModal {
    pub fn open(message_id: MessageId, locale: SharedString, window: &mut Window, cx: &mut App) {
        let placeholder = mezon_i18n::t(
            &locale,
            "contextMenu.reportMessageModal.customReasonPlaceholder",
        )
        .to_string();
        let view = cx.new(|cx| {
            let custom_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .multi_line(true)
                    .validate(|value, _cx| value.chars().count() <= CUSTOM_REASON_MAX)
            });
            let custom_sub = cx.subscribe(&custom_input, |_this, _input, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                message_id,
                selected: None,
                custom_input,
                submitting: false,
                _custom_sub: custom_sub,
            }
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn is_submittable(&self, cx: &App) -> bool {
        match self.selected {
            None => false,
            Some(idx) if REASONS[idx] == "other" => {
                !self.custom_input.read(cx).value().trim().is_empty()
            }
            Some(_) => true,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.submitting || !self.is_submittable(cx) {
            return;
        }
        let Some(idx) = self.selected else {
            return;
        };
        let abuse_type = if REASONS[idx] == "other" {
            self.custom_input.read(cx).value().trim().to_string()
        } else {
            REASONS[idx].to_string()
        };
        self.submitting = true;
        let message_id = self.message_id;
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.report_message(message_id, abuse_type, cx);
        });
        Self::close(cx);
    }
}

impl Render for ReportMessageModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let t = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let entity = cx.entity();

        let header = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_6()
            .pb_2()
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(t("contextMenu.reportMessageModal.title")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(t("contextMenu.reportMessageModal.description")),
            );

        let mut reason_list = div().flex().flex_col().gap_1();
        for (idx, key) in REASON_KEYS.iter().enumerate() {
            let selected = self.selected == Some(idx);
            let ent = entity.clone();
            let label = t(key);
            let mut row = div()
                .id(("report-reason", idx))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .p_3()
                .rounded_lg()
                .cursor_pointer()
                .hover(|s| s.bg(theme.tokens.bg_item_theme_hover));
            if selected {
                row = row.bg(theme.tokens.bg_active_member_channel);
            }
            let text_color = if selected {
                theme.tokens.text_secondary
            } else {
                theme.tokens.text_theme_primary
            };
            let weight = if selected {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            };
            row = row
                .child(
                    div()
                        .text_sm()
                        .font_weight(weight)
                        .text_color(text_color)
                        .child(label),
                )
                .when(selected, |el| {
                    el.child(
                        Icon::new(IconName::Check)
                            .size_4()
                            .text_color(theme.tokens.text_secondary),
                    )
                })
                .on_click(move |_: &ClickEvent, _window, cx| {
                    ent.update(cx, |this, cx| {
                        this.selected = Some(idx);
                        cx.notify();
                    });
                });
            reason_list = reason_list.child(row);
        }

        let show_custom = self.selected.map(|idx| REASONS[idx] == "other") == Some(true);
        let custom_len = self.custom_input.read(cx).value().chars().count();
        let custom_section = show_custom.then(|| {
            div()
                .mt_3()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .rounded_lg()
                        .bg(theme.surfaces.surface)
                        .border_1()
                        .border_color(theme.tokens.border_primary)
                        .px_3()
                        .py_2()
                        .child(
                            Input::new(&self.custom_input)
                                .w_full()
                                .text_sm()
                                .text_color(theme.tokens.text_theme_message),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(format!("{custom_len}/{CUSTOM_REASON_MAX}")),
                )
        });

        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_6()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(t("contextMenu.reportMessageModal.selectReason")),
            )
            .child(reason_list)
            .children(custom_section);

        let submit_entity = entity.clone();
        let submit_label = if self.submitting {
            t("contextMenu.reportMessageModal.submitting")
        } else {
            t("contextMenu.reportMessageModal.submit")
        };
        let submit_disabled = self.submitting || !self.is_submittable(cx);

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap_3()
            .p_4()
            .bg(theme.tokens.theme_setting_nav)
            .child(
                div()
                    .id("report-cancel")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .child(t("contextMenu.reportMessageModal.cancel"))
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
            )
            .child(
                Button::new("report-submit")
                    .primary()
                    .label(submit_label)
                    .loading(self.submitting)
                    .disabled(submit_disabled)
                    .on_click(move |_: &ClickEvent, _window, cx| {
                        submit_entity.update(cx, |this, cx| this.submit(cx));
                    }),
            );

        div()
            .track_focus(&self.focus_handle)
            .occlude()
            .w(px(540.))
            .max_h(px(640.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(header)
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .child(footer)
    }
}
