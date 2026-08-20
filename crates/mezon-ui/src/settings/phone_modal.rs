use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Pixels, SharedString, Subscription,
    Task, Window, div, prelude::*, px, relative, rgb,
};
use mezon_store::{AccountEvent, AccountStore, PhoneLinkError};

use crate::app::shell::Shell;
use crate::components::compositions::OtpInput;
use crate::components::primitives::{
    Button as GpuiButton, ButtonVariants, Dropdown, DropdownTriggerStyle, Input, InputEvent,
    InputState, h_flex, v_flex,
};
use crate::theme::ActiveTheme;

const OTP_TTL_SECONDS: u32 = 60;
const COUNTRIES: [(&str, &str); 3] = [("Vietnam", "+84"), ("Japan", "+81"), ("US", "+1")];
const LINK_COLOR: u32 = 0x5865f2;
const ERROR_LINE_HEIGHT: Pixels = px(20.);

fn parse_phone_vn(phone: &str) -> String {
    if let Some(rest) = phone.strip_prefix('0') {
        format!("+84{rest}")
    } else if phone.starts_with("+84") {
        phone.to_string()
    } else {
        format!("+84{phone}")
    }
}

fn is_valid_phone_vn(phone: &str) -> bool {
    let parsed = parse_phone_vn(phone);
    let Some(national) = parsed.strip_prefix("+84") else {
        return false;
    };
    let mut chars = national.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '3' | '5' | '7' | '8' | '9') {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    rest.len() == 8 && rest.iter().all(|c| c.is_ascii_digit())
}

fn mask_phone(phone: &str) -> String {
    let count = phone.chars().count();
    if count < 3 {
        return phone.to_string();
    }
    let tail: String = phone.chars().skip(count - 2).collect();
    format!("xxx-xxx{tail}")
}

pub(super) struct PhoneModal {
    focus_handle: FocusHandle,
    locale: String,
    country: usize,
    country_open: bool,
    phone_input: Entity<InputState>,
    otp_input: Entity<OtpInput>,
    awaiting_otp: bool,
    req_id: Option<String>,
    parsed_phone: String,
    phone_error: Option<SharedString>,
    otp_error: Option<SharedString>,
    countdown: u32,
    pending_otp_clear: bool,
    sending: bool,
    verifying: bool,
    _countdown: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for PhoneModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PhoneModal {
    pub(super) fn open(locale: String, window: &mut Window, cx: &mut App) {
        let modal = cx.new(|cx| Self::new(locale, window, cx));
        let focus = modal.read(cx).focus_handle.clone();
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
        window.focus(&focus, cx);
    }

    fn new(locale: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "accountSetting.setPhoneModal.phoneNumber").into();
        let phone_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(40.))
                .validate(|text, _cx| {
                    text.is_empty()
                        || text
                            .chars()
                            .enumerate()
                            .all(|(index, c)| c.is_ascii_digit() || (index == 0 && c == '+'))
                })
        });

        let entity = cx.entity().clone();
        let otp_input = cx.new(|cx| {
            OtpInput::new(window, cx, 6).on_complete(Arc::new(move |code, _window, cx| {
                entity.update(cx, |this, cx| this.verify(code, cx));
            }))
        });

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(
            &phone_input,
            |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.phone_error = None;
                    cx.notify();
                }
                InputEvent::PressEnter => this.submit(cx),
            },
        ));
        subscriptions.push(cx.subscribe(
            &AccountStore::global(cx),
            |this, _, event: &AccountEvent, cx| match event {
                AccountEvent::PhoneOtpSent(req_id) => {
                    this.sending = false;
                    this.req_id = Some(req_id.clone());
                    this.awaiting_otp = true;
                    this.phone_error = None;
                    this.otp_error = None;
                    this.pending_otp_clear = true;
                    this.start_countdown(cx);
                    cx.notify();
                }
                AccountEvent::PhoneOtpFailed(reason) => {
                    this.sending = false;
                    let key = match reason {
                        PhoneLinkError::AlreadyLinkedToAnother => {
                            "accountSetting.setPhoneModal.alreadyLinkedToAnother"
                        }
                        PhoneLinkError::Failed => "accountSetting.setPhoneModal.updatePhoneFail",
                    };
                    this.phone_error = Some(mezon_i18n::t(&this.locale, key).into());
                    cx.notify();
                }
                AccountEvent::PhoneVerified => {
                    this.verifying = false;
                    let message = mezon_i18n::t(
                        &this.locale,
                        "accountSetting.setPhoneModal.updatePhoneSuccess",
                    );
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.close_modal(cx);
                        shell.success(message, cx);
                    });
                }
                AccountEvent::PhoneVerifyFailed => {
                    this.verifying = false;
                    this.otp_error = Some(
                        mezon_i18n::t(&this.locale, "accountSetting.setPhoneModal.emptyOtp").into(),
                    );
                    cx.notify();
                }
                _ => {}
            },
        ));

        Self {
            focus_handle: cx.focus_handle(),
            locale,
            country: 0,
            country_open: false,
            phone_input,
            otp_input,
            awaiting_otp: false,
            req_id: None,
            parsed_phone: String::new(),
            phone_error: None,
            otp_error: None,
            countdown: 0,
            pending_otp_clear: false,
            sending: false,
            verifying: false,
            _countdown: None,
            _subscriptions: subscriptions,
        }
    }

    fn start_countdown(&mut self, cx: &mut Context<Self>) {
        self.countdown = OTP_TTL_SECONDS;
        self._countdown = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let expired = this.update(cx, |this, cx| {
                    this.countdown = this.countdown.saturating_sub(1);
                    if this.countdown == 0 {
                        this.otp_error = Some(
                            mezon_i18n::t(&this.locale, "accountSetting.setPhoneModal.otpExpired")
                                .into(),
                        );
                    }
                    cx.notify();
                    this.countdown == 0
                });
                if !matches!(expired, Ok(false)) {
                    break;
                }
            }
        }));
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.sending || self.verifying || self.countdown > 0 {
            return;
        }
        let phone = self.phone_input.read(cx).value().to_string();
        if phone.is_empty() {
            return;
        }
        if !is_valid_phone_vn(&phone) {
            self.phone_error = Some(
                mezon_i18n::t(&self.locale, "accountSetting.setPhoneModal.invalidPhone").into(),
            );
            cx.notify();
            return;
        }
        self.parsed_phone = parse_phone_vn(&phone);
        self.sending = true;
        cx.notify();
        let parsed = self.parsed_phone.clone();
        AccountStore::global(cx).update(cx, |store, cx| store.link_phone(parsed, cx));
    }

    fn verify(&mut self, code: String, cx: &mut Context<Self>) {
        if self.verifying || self.countdown == 0 {
            return;
        }
        let Some(req_id) = self.req_id.clone() else {
            return;
        };
        if code.chars().count() < 6 {
            self.otp_error =
                Some(mezon_i18n::t(&self.locale, "accountSetting.setPhoneModal.emptyOtp").into());
            cx.notify();
            return;
        }
        self.verifying = true;
        cx.notify();
        let phone = self.parsed_phone.clone();
        AccountStore::global(cx)
            .update(cx, |store, cx| store.verify_phone(req_id, code, phone, cx));
    }

    fn back_to_phone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.awaiting_otp = false;
        self.otp_input
            .update(cx, |input, cx| input.clear(window, cx));
        self.req_id = None;
        self.pending_otp_clear = false;
        self.otp_error = None;
        self.countdown = 0;
        self._countdown = None;
        cx.notify();
    }
}

impl Render for PhoneModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_otp_clear {
            self.pending_otp_clear = false;
            self.otp_input
                .clone()
                .update(cx, |otp, cx| otp.clear(window, cx));
        }
        let theme = cx.theme();
        let locale = self.locale.clone();
        let phone_empty = self.phone_input.read(cx).value().is_empty();
        let disabled = self.countdown > 0
            || self.phone_error.is_some()
            || self.sending
            || self.verifying
            || phone_empty;
        let submit_label: SharedString = if self.awaiting_otp {
            let resend = mezon_i18n::t(&locale, "accountSetting.setPhoneModal.resendOtp");
            if self.countdown > 0 {
                format!("{resend} ({})", self.countdown).into()
            } else {
                resend.into()
            }
        } else {
            mezon_i18n::t(&locale, "accountSetting.setPhoneModal.sendOTP").into()
        };

        let body = if self.awaiting_otp {
            let masked = mask_phone(&self.parsed_phone);
            let country_label = COUNTRIES
                .get(self.country)
                .map(|(name, dial)| format!("{name} ({dial})"))
                .unwrap_or_default();
            v_flex()
                .gap_4()
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.tokens.bg_input_secondary)
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .text_sm()
                                .child(div().text_color(theme.tokens.text_secondary).child(
                                    mezon_i18n::t(&locale, "accountSetting.setPhoneModal.country"),
                                ))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(country_label),
                                ),
                        )
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .text_sm()
                                .child(div().text_color(theme.tokens.text_secondary).child(
                                    mezon_i18n::t(
                                        &locale,
                                        "accountSetting.setPhoneModal.phoneNumber",
                                    ),
                                ))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(masked.clone()),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(theme.tokens.text_secondary)
                        .child(format!(
                            "{} {masked}",
                            mezon_i18n::t(&locale, "accountSetting.setPhoneModal.verifyOtpMessage")
                        )),
                )
                .child(self.otp_input.clone())
                .child(
                    div()
                        .h(ERROR_LINE_HEIGHT)
                        .text_sm()
                        .text_center()
                        .text_color(theme.danger_text)
                        .children(self.otp_error.clone()),
                )
                .when(self.countdown == 0, |el| {
                    el.child(
                        div()
                            .id("phone-back-to-input")
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(LINK_COLOR))
                            .cursor_pointer()
                            .child(mezon_i18n::t(
                                &locale,
                                "accountSetting.setPhoneModal.backToPhoneInput",
                            ))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.back_to_phone(window, cx)),
                            ),
                    )
                })
        } else {
            let country_items: Vec<SharedString> = COUNTRIES
                .iter()
                .map(|(name, _)| SharedString::from(*name))
                .collect();
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    v_flex()
                        .gap_2()
                        .w(px(150.))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.tokens.text_secondary)
                                .child(mezon_i18n::t(
                                    &locale,
                                    "accountSetting.setPhoneModal.countryCode",
                                )),
                        )
                        .child(
                            Dropdown::new("phone-country-select")
                                .items(country_items)
                                .selected(Some(self.country))
                                .open(self.country_open)
                                .trigger_style(DropdownTriggerStyle::InputPrimary)
                                .on_toggle({
                                    let entity = cx.entity().clone();
                                    move |_, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.country_open = !this.country_open;
                                            cx.notify();
                                        })
                                    }
                                })
                                .on_select({
                                    let entity = cx.entity().clone();
                                    move |index, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.country = index;
                                            this.country_open = false;
                                            cx.notify();
                                        })
                                    }
                                }),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.tokens.text_secondary)
                                .child(mezon_i18n::t(
                                    &locale,
                                    "accountSetting.setPhoneModal.phoneNumber",
                                )),
                        )
                        .child(Input::new(&self.phone_input))
                        .child(
                            div()
                                .h(ERROR_LINE_HEIGHT)
                                .text_sm()
                                .text_color(theme.danger_text)
                                .children(self.phone_error.clone()),
                        ),
                )
        };

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
                    .gap_1()
                    .p_6()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(mezon_i18n::t(&locale, "accountSetting.setPhoneNumber")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(mezon_i18n::t(
                                &locale,
                                "accountSetting.setPhoneModal.description",
                            )),
                    ),
            )
            .child(v_flex().p_6().child(body))
            .child(
                div().px_6().pb_6().child(
                    GpuiButton::new("phone-submit")
                        .label(submit_label)
                        .primary()
                        .disabled(disabled)
                        .w_full()
                        .on_click(cx.listener(|this, _, _window, cx| this.submit(cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{is_valid_phone_vn, mask_phone, parse_phone_vn};

    #[test]
    fn parses_local_and_international_forms() {
        assert_eq!(parse_phone_vn("0912345678"), "+84912345678");
        assert_eq!(parse_phone_vn("+84912345678"), "+84912345678");
        assert_eq!(parse_phone_vn("912345678"), "+84912345678");
    }

    #[test]
    fn accepts_valid_vn_mobile_prefixes() {
        for prefix in ['3', '5', '7', '8', '9'] {
            assert!(is_valid_phone_vn(&format!("0{prefix}12345678")));
        }
        assert!(is_valid_phone_vn("+84912345678"));
    }

    #[test]
    fn rejects_bad_prefix_and_length() {
        assert!(!is_valid_phone_vn("0112345678"));
        assert!(!is_valid_phone_vn("091234567"));
        assert!(!is_valid_phone_vn("09123456789"));
        assert!(!is_valid_phone_vn(""));
    }

    #[test]
    fn masks_all_but_the_last_two_digits() {
        assert_eq!(mask_phone("+84912345678"), "xxx-xxx78");
        assert_eq!(mask_phone("12"), "12");
    }
}
