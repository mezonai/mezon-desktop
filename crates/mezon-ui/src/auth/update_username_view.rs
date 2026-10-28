use gpui::{
    App, Context, Entity, Focusable, FontWeight, MouseButton, Subscription, Window, div,
    prelude::*, px, rgb, white,
};
use mezon_store::{AuthState, LoginStore, Settings};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use super::login_view::{LoginView, error_slot, login_background, login_button};
use crate::components::primitives::{Input, InputEvent, InputState};

pub struct UpdateUsernameView {
    auth_state: Entity<AuthState>,
    settings: Entity<Settings>,
    username_input: Option<Entity<InputState>>,
    loading: bool,
    error: Option<String>,
    pending_reset: bool,
    pending_focus: bool,
    _subscriptions: Vec<Subscription>,
}

impl UpdateUsernameView {
    pub fn new(
        auth_state: Entity<AuthState>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&auth_state, |this, auth, cx| {
            if matches!(auth.read(cx), AuthState::UsernameRequired(_)) {
                this.loading = false;
                this.error = None;
                this.pending_reset = true;
                this.pending_focus = true;
            }
            cx.notify();
        })
        .detach();
        Self {
            auth_state,
            settings,
            username_input: None,
            loading: false,
            error: None,
            pending_reset: false,
            pending_focus: true,
            _subscriptions: Vec::new(),
        }
    }

    fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.username_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder = mezon_i18n::t(&locale, "common.updateUsername.yourName");
        let input = cx.new(|cx| LoginView::login_input(window, cx, placeholder));
        self._subscriptions
            .push(cx.subscribe_in(&input, window, Self::on_input_event));
        self.username_input = Some(input);
    }

    fn on_input_event(
        &mut self,
        _input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.error = None;
                cx.notify();
            }
            InputEvent::PressEnter => {
                let entity = cx.entity();
                window.defer(cx, move |_window, cx| Self::submit(&entity, cx));
            }
        }
    }

    fn typed_username(&self, cx: &App) -> String {
        self.username_input
            .as_ref()
            .map(|input| sanitize_username(&input.read(cx).value()))
            .unwrap_or_default()
    }

    fn submit(entity: &Entity<UpdateUsernameView>, cx: &mut App) {
        let username = entity.read(cx).typed_username(cx);
        if username.is_empty() || entity.read(cx).loading {
            return;
        }
        let auth_state = entity.read(cx).auth_state.clone();
        let session = match auth_state.read(cx) {
            AuthState::UsernameRequired(session) => session.clone(),
            _ => return,
        };

        entity.update(cx, |this, cx| {
            this.loading = true;
            this.error = None;
            cx.notify();
        });

        let client = LoginStore::global(cx).read(cx).client();
        let entity_clone = entity.clone();
        cx.spawn(async move |cx| {
            let result = client.update_username(&session, &username).await;
            entity_clone.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(updated) => LoginView::on_auth_success(updated, &this.auth_state, cx),
                    Err(e) => {
                        tracing::warn!("Updating the username failed: {e}");
                        let locale = this.settings.read(cx).language.clone();
                        this.error = Some(
                            mezon_i18n::t(&locale, "common.updateUsername.errorDuplicate")
                                .to_string(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for UpdateUsernameView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_input(window, cx);
        if self.pending_reset {
            self.pending_reset = false;
            if let Some(input) = self.username_input.clone() {
                input.update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        if self.pending_focus {
            self.pending_focus = false;
            if let Some(input) = &self.username_input {
                let handle = input.focus_handle(cx);
                window.focus(&handle, cx);
            }
        }

        let locale = self.settings.read(cx).language.clone();
        let blue = rgb(0x3b82f6);
        let gray400 = rgb(0x9ca3af);
        let username = self.typed_username(cx);
        let loading = self.loading;

        let mut form = div().flex().flex_col().gap_3();
        if let Some(input) = &self.username_input {
            form = form.child(Input::new(input));
        }
        let preview = if username.is_empty() {
            String::new()
        } else {
            mezon_i18n::t(&locale, "common.updateUsername.usernamePreview")
                .replace("{{username}}", &username)
        };
        form = form
            .child(
                div()
                    .min_h(px(20.))
                    .text_sm()
                    .text_center()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(white())
                    .child(preview),
            )
            .child(login_button(
                "update-username-submit",
                mezon_i18n::t(&locale, "common.updateUsername.update").to_string(),
                loading,
                username.is_empty(),
                {
                    let entity = cx.entity();
                    move |_window, cx| Self::submit(&entity, cx)
                },
            ))
            .child(error_slot(self.error.as_deref()));

        let skip = div()
            .mt_4()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_center()
                    .text_color(white())
                    .child(mezon_i18n::t(
                        &locale,
                        "common.updateUsername.skipUpdateQuestion",
                    )),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(blue)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .when(!loading, |el| {
                        el.on_mouse_down(MouseButton::Left, |_, _window, cx| {
                            LoginStore::global(cx)
                                .update(cx, |store, cx| store.discard_pending_signup(cx));
                        })
                    })
                    .child(mezon_i18n::t(
                        &locale,
                        "common.updateUsername.skipUpdateBack",
                    )),
            );

        let card = div()
            .flex()
            .flex_col()
            .w(px(480.))
            .max_w_full()
            .p(px(56.))
            .rounded(px(16.))
            .shadow_lg()
            .bg(rgb(0x0b0b0b))
            .text_color(white())
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .mb_6()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .mb_1()
                            .text_color(white())
                            .child(mezon_i18n::t(
                                &locale,
                                "common.updateUsername.enterUsername",
                            )),
                    )
                    .child(div().text_center().text_color(gray400).child(mezon_i18n::t(
                        &locale,
                        "common.updateUsername.usernamePlaceholder",
                    ))),
            )
            .child(form)
            .child(skip);

        div()
            .relative()
            .flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .px_4()
            .child(login_background())
            .child(card)
    }
}

const MAX_USERNAME_LEN: usize = 32;

pub(crate) fn sanitize_username(raw: &str) -> String {
    raw.trim()
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .map(|c| if matches!(c, 'đ' | 'Đ') { 'd' } else { c })
        .flat_map(char::to_lowercase)
        .filter(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '+' | '-')
        })
        .take(MAX_USERNAME_LEN)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sanitize_username;

    #[test]
    fn folds_vietnamese_diacritics_and_drops_spaces() {
        assert_eq!(sanitize_username("Nguyễn Văn Đạt"), "nguyenvandat");
    }

    #[test]
    fn keeps_server_allowed_symbols_and_drops_others() {
        assert_eq!(sanitize_username("  A.b-c_1+! "), "a.b-c_1+");
    }

    #[test]
    fn drops_non_latin_letters_and_caps_length() {
        assert_eq!(sanitize_username("日本abc"), "abc");
        assert_eq!(sanitize_username(&"a".repeat(40)).len(), 32);
    }

    #[test]
    fn symbol_only_input_is_empty() {
        assert_eq!(sanitize_username(" !@# "), "");
    }
}
