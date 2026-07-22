use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, Render, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use mezon_store::MessagesStore;

use crate::app::shell::Shell;
use crate::components::primitives::{Button, ButtonVariants, InputEvent, InputState};
use crate::theme::ActiveTheme;

const MAX_BUZZ_LEN: usize = 160;

pub struct MessageBuzzModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    message: Entity<InputState>,
    _message_sub: Subscription,
}

impl Focusable for MessageBuzzModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MessageBuzzModal {
    pub fn open(locale: SharedString, window: &mut Window, cx: &mut App) {
        if Shell::global(cx).read(cx).has_modal() {
            return;
        }
        let view = cx.new(|cx| {
            let placeholder = mezon_i18n::t(&locale, "messageBuzz.enterMessage").to_string();
            let message = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .height(px(42.))
                    .validate(|value, _cx| value.chars().count() <= MAX_BUZZ_LEN)
            });
            let message_sub = cx.subscribe(
                &message,
                |this: &mut Self, _input, event: &InputEvent, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter => this.send(cx),
                },
            );
            message.update(cx, |input, cx| input.focus(window, cx));
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                message,
                _message_sub: message_sub,
            }
        });
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.clone().into(), cx));
        view.update(cx, |modal, cx| {
            modal
                .message
                .update(cx, |input, cx| input.focus(window, cx));
        });
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.message.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_buzz_message(text, cx);
        });
        Self::close(cx);
    }
}

impl Render for MessageBuzzModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let title = mezon_i18n::t(&self.locale, "messageBuzz.enterMessage");
        let send_label = mezon_i18n::t(&self.locale, "messageBuzz.send");
        let entity = cx.entity();
        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_this, _: &::menu::Cancel, _window, cx| {
                Self::close(cx);
            }))
            .w(px(400.))
            .rounded(px(8.))
            .bg(theme.tokens.theme_setting_primary)
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(title),
            )
            .child(self.message.clone())
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("buzz-cancel").label("Cancel").on_click(
                        move |_: &ClickEvent, _window, cx| {
                            MessageBuzzModal::close(cx);
                        },
                    ))
                    .child(
                        Button::new("buzz-send")
                            .primary()
                            .label(send_label)
                            .on_click(move |_: &ClickEvent, _window, cx| {
                                entity.update(cx, |this, cx| this.send(cx));
                            }),
                    ),
            )
    }
}
