use crate::app::shell::Shell;
use crate::components::primitives::{
    Button as GpuiButton, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, h_flex,
    v_flex,
};
use crate::theme::ActiveTheme;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString, Subscription, Window,
    div, prelude::*, px, relative,
};
use mezon_store::{AccountEvent, AccountStore, PasswordSaveError};

#[derive(Clone, Copy)]
enum PasswordValidationError {
    Characters,
    Uppercase,
    Lowercase,
    Number,
    Symbol,
}

#[derive(Clone, Copy)]
enum CurrentPasswordError {
    Required,
    Incorrect,
}

fn validate_password(password: &str) -> Option<PasswordValidationError> {
    if password.chars().count() < 8 {
        Some(PasswordValidationError::Characters)
    } else if !password.chars().any(|c| c.is_ascii_uppercase()) {
        Some(PasswordValidationError::Uppercase)
    } else if !password.chars().any(|c| c.is_ascii_lowercase()) {
        Some(PasswordValidationError::Lowercase)
    } else if !password.chars().any(|c| c.is_ascii_digit()) {
        Some(PasswordValidationError::Number)
    } else if !password.chars().any(|c| !c.is_ascii_alphanumeric()) {
        Some(PasswordValidationError::Symbol)
    } else {
        None
    }
}

pub(super) struct PasswordModal {
    focus_handle: FocusHandle,
    locale: String,
    email: String,
    has_password: bool,
    current_input: Option<Entity<InputState>>,
    password_input: Entity<InputState>,
    confirm_input: Entity<InputState>,
    password_error: Option<PasswordValidationError>,
    confirm_mismatch: bool,
    current_error: Option<CurrentPasswordError>,
    submitted: bool,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for PasswordModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PasswordModal {
    pub(super) fn open(
        locale: String,
        email: String,
        has_password: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let modal = cx.new(|cx| Self::new(locale, email, has_password, window, cx));
        let focus = modal.read(cx).focus_handle.clone();
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
        window.focus(&focus, cx);
    }

    fn new(
        locale: String,
        email: String,
        has_password: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut make_password_input = |placeholder_key: &'static str, cx: &mut Context<Self>| {
            let placeholder: SharedString = mezon_i18n::t(&locale, placeholder_key).into();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .padding_right(px(64.))
                    .height(px(52.))
            });
            input.update(cx, |state, cx| state.set_masked(true, window, cx));
            input
        };
        let current_input = has_password.then(|| {
            make_password_input(
                "accountSetting.setPasswordAccount.placeholder.currentPassword",
                cx,
            )
        });
        let password_input =
            make_password_input("accountSetting.setPasswordAccount.placeholder.password", cx);
        let confirm_input = make_password_input(
            "accountSetting.setPasswordAccount.placeholder.confirmPassword",
            cx,
        );

        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.subscribe(
                &password_input,
                |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => this.revalidate(cx),
                    InputEvent::PressEnter => this.save(cx),
                },
            ),
        );
        subscriptions.push(
            cx.subscribe(
                &confirm_input,
                |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => this.revalidate(cx),
                    InputEvent::PressEnter => this.save(cx),
                },
            ),
        );
        if let Some(input) = &current_input {
            subscriptions.push(cx.subscribe(
                input,
                |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        this.current_error = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter => this.save(cx),
                },
            ));
        }
        subscriptions.push(cx.subscribe(
            &AccountStore::global(cx),
            |this, _, event: &AccountEvent, cx| match event {
                AccountEvent::PasswordSaved => {
                    this.saving = false;
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.close_modal(cx);
                        shell.success(
                            mezon_i18n::t(
                                &this.locale,
                                "accountSetting.setPasswordAccount.toast.success",
                            ),
                            cx,
                        );
                    });
                }
                AccountEvent::PasswordSaveFailed(error) => {
                    this.saving = false;
                    this.current_error = (*error == PasswordSaveError::IncorrectCurrentPassword)
                        .then_some(CurrentPasswordError::Incorrect);
                    let key = match error {
                        PasswordSaveError::IncorrectCurrentPassword => {
                            "accountSetting.setPasswordAccount.error.incorrectCurrent"
                        }
                        PasswordSaveError::UpdateFailed => {
                            "accountSetting.setPasswordAccount.error.updateFail"
                        }
                        PasswordSaveError::CreateFailed => {
                            "accountSetting.setPasswordAccount.error.createFail"
                        }
                    };
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.error(mezon_i18n::t(&this.locale, key), cx)
                    });
                    cx.notify();
                }
                _ => {}
            },
        ));

        Self {
            focus_handle: cx.focus_handle(),
            locale,
            email,
            has_password,
            current_input,
            password_input,
            confirm_input,
            password_error: None,
            confirm_mismatch: false,
            current_error: None,
            submitted: false,
            saving: false,
            _subscriptions: subscriptions,
        }
    }

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        let password = self.password_input.read(cx).value();
        let confirm = self.confirm_input.read(cx).value();
        self.password_error = (!password.is_empty() || self.submitted)
            .then(|| validate_password(password))
            .flatten();
        self.confirm_mismatch = (!confirm.is_empty() || self.submitted) && confirm != password;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.email.is_empty() {
            return;
        }
        self.submitted = true;
        self.revalidate(cx);
        if self
            .current_input
            .as_ref()
            .is_some_and(|input| input.read(cx).value().is_empty())
        {
            self.current_error = Some(CurrentPasswordError::Required);
            cx.notify();
            return;
        }
        let same_password = self
            .current_input
            .as_ref()
            .is_some_and(|input| input.read(cx).value() == self.password_input.read(cx).value());
        if same_password {
            Shell::global(cx).update(cx, |shell, cx| {
                shell.error(
                    mezon_i18n::t(
                        &self.locale,
                        "accountSetting.setPasswordAccount.error.samePass",
                    ),
                    cx,
                )
            });
            return;
        }
        let password_value = self.password_input.read(cx).value();
        if password_value.is_empty()
            || validate_password(password_value).is_some()
            || self.confirm_input.read(cx).value() != password_value
        {
            return;
        }

        let email = self.email.clone();
        let password = self.password_input.read(cx).value().to_string();
        let old_password = self
            .current_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        AccountStore::global(cx).update(cx, |store, cx| {
            store.save_password(email, password, old_password, cx)
        });
        self.saving = true;
        cx.notify();
    }

    fn error_text(&self, error: PasswordValidationError) -> SharedString {
        let key = match error {
            PasswordValidationError::Characters => {
                "accountSetting.setPasswordAccount.error.characters"
            }
            PasswordValidationError::Uppercase => {
                "accountSetting.setPasswordAccount.error.uppercase"
            }
            PasswordValidationError::Lowercase => {
                "accountSetting.setPasswordAccount.error.lowercase"
            }
            PasswordValidationError::Number => "accountSetting.setPasswordAccount.error.number",
            PasswordValidationError::Symbol => "accountSetting.setPasswordAccount.error.symbol",
        };
        mezon_i18n::t(&self.locale, key).into()
    }
}

impl Render for PasswordModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let can_submit = !self.saving && !self.email.is_empty();
        let field =
            |label: SharedString, input: Entity<InputState>, error: Option<SharedString>| {
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.tokens.text_secondary)
                            .child(label)
                            .child(div().text_color(theme.danger_text).child("*")),
                    )
                    .child(Input::new(&input).mask_toggle())
                    .when_some(error, |el, error| {
                        el.child(div().text_sm().text_color(theme.danger_text).child(error))
                    })
            };
        let current_error = self.current_error.map(|error| {
            let key = match error {
                CurrentPasswordError::Required => {
                    "accountSetting.setPasswordAccount.error.fillOldPass"
                }
                CurrentPasswordError::Incorrect => {
                    "accountSetting.setPasswordAccount.error.incorrectCurrent"
                }
            };
            mezon_i18n::t(&self.locale, key).into()
        });
        let password_error = self.password_error.map(|error| self.error_text(error));
        let confirm_error = self.confirm_mismatch.then(|| {
            mezon_i18n::t(
                &self.locale,
                "accountSetting.setPasswordAccount.error.notEqual",
            )
            .into()
        });

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(448.))
            .max_w(px(448.))
            .max_h(relative(0.9))
            .rounded_lg()
            .overflow_hidden()
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                v_flex()
                    .relative()
                    .gap_1()
                    .p_6()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(
                                &self.locale,
                                if self.has_password {
                                    "accountSetting.setPasswordAccount.changePassword"
                                } else {
                                    "accountSetting.setPasswordModal.title"
                                },
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(mezon_i18n::t(
                                &self.locale,
                                "accountSetting.setPasswordModal.description",
                            )),
                    )
                    .child(
                        div()
                            .id("password-modal-close")
                            .absolute()
                            .top_4()
                            .right_4()
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(theme.tokens.text_theme_primary)
                            .hover(|style| style.bg(theme.tokens.bg_item_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(20.))
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id("password-modal-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_4()
                    .p_6()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.tokens.text_secondary)
                                    .child(mezon_i18n::t(
                                        &self.locale,
                                        "accountSetting.setPasswordAccount.email",
                                    )),
                            )
                            .child(
                                div()
                                    .h(px(52.))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.bg_secondary)
                                    .text_color(theme.tokens.text_secondary)
                                    .child(self.email.clone()),
                            ),
                    )
                    .when_some(self.current_input.clone(), |el, input| {
                        el.child(field(
                            mezon_i18n::t(
                                &self.locale,
                                "accountSetting.setPasswordAccount.currentPassword",
                            )
                            .into(),
                            input,
                            current_error,
                        ))
                    })
                    .child(field(
                        mezon_i18n::t(&self.locale, "accountSetting.setPasswordAccount.password")
                            .into(),
                        self.password_input.clone(),
                        password_error,
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(mezon_i18n::t(
                                &self.locale,
                                "accountSetting.setPasswordAccount.description",
                            )),
                    )
                    .child(field(
                        mezon_i18n::t(
                            &self.locale,
                            "accountSetting.setPasswordAccount.confirmPassword",
                        )
                        .into(),
                        self.confirm_input.clone(),
                        confirm_error,
                    )),
            )
            .child(
                div().p_6().pt_0().child(
                    GpuiButton::new("password-modal-save")
                        .label(mezon_i18n::t(
                            &self.locale,
                            if self.saving {
                                "accountSetting.setPasswordModal.loading"
                            } else {
                                "accountSetting.setPasswordAccount.confirm"
                            },
                        ))
                        .primary()
                        .disabled(!can_submit)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_validation_matches_account_policy() {
        assert!(matches!(
            validate_password("Aa1!"),
            Some(PasswordValidationError::Characters)
        ));
        assert!(matches!(
            validate_password("lowercase1!"),
            Some(PasswordValidationError::Uppercase)
        ));
        assert!(matches!(
            validate_password("UPPERCASE1!"),
            Some(PasswordValidationError::Lowercase)
        ));
        assert!(matches!(
            validate_password("Password!"),
            Some(PasswordValidationError::Number)
        ));
        assert!(matches!(
            validate_password("Password1"),
            Some(PasswordValidationError::Symbol)
        ));
        assert!(validate_password("Password1!").is_none());
        assert!(matches!(
            validate_password("É1password!"),
            Some(PasswordValidationError::Uppercase)
        ));
    }
}
