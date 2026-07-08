use std::time::Duration;

use crate::components::primitives::{
    Avatar, Button as GpuiButton, ButtonVariants, Divider, Label, Sizable, Size, h_flex, v_flex,
};
use gpui::{Context, Entity, FontWeight, SharedString, Window, div, prelude::*, px};
use mezon_store::{AccountStore, Settings};

use crate::theme::ActiveTheme;

pub struct AccountPage {
    settings: Entity<Settings>,
    toast_message: Option<SharedString>,
}

impl AccountPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&AccountStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        AccountStore::global(cx).update(cx, |store, cx| store.ensure_account(cx));
        Self {
            settings,
            toast_message: None,
        }
    }

    fn show_toast(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast_message = Some(message.into());
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.toast_message = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for AccountPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let store = AccountStore::global(cx).read(cx);

        if store.account_error {
            return v_flex()
                .gap_4()
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.account.failedToLoad"))
                        .text_color(theme.text_muted),
                )
                .into_any_element();
        }

        if store.account_loading || store.account.is_none() {
            return v_flex()
                .gap_4()
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.account.loading"))
                        .text_color(theme.text_muted),
                )
                .into_any_element();
        }

        let account = store.account.as_ref().unwrap();

        let display_name: SharedString = account.display_name.clone().into();
        let username: SharedString = account.username.clone().into();

        let email_display = match &account.email {
            Some(email) if !email.is_empty() => SharedString::from(mask_email(email)),
            _ => SharedString::from(mezon_i18n::t(&locale, "common.notSet")),
        };

        let password_label = if account.password_setted {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.changePassword"))
        } else {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.setPassword"))
        };

        let phone_display = account
            .phone_number
            .as_ref()
            .map(|s| SharedString::from(s.as_str()))
            .unwrap_or(SharedString::from(mezon_i18n::t(&locale, "common.notSet")));

        let phone_label = if account.phone_number.is_some() {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.changePhone"))
        } else {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.setPhone"))
        };

        let avatar_url = account
            .avatar_url
            .as_ref()
            .map(|url| SharedString::from(crate::util::imgproxy::profile_url(cx, url)));

        v_flex()
            .gap_6()
            .text_sm()
            .child(
                v_flex()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    .shadow_md()
                    .child(div().h(px(100.0)).w_full().bg(theme.brand))
                    .child(
                        h_flex()
                            .px_6()
                            .py_4()
                            .gap_4()
                            .child(
                                Avatar::new()
                                    .when_some(avatar_url.clone(), |av, url| av.src(url))
                                    .name(display_name.clone())
                                    .with_size(Size::Large),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        Label::new(display_name.clone())
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary),
                                    )
                                    .child(
                                        Label::new(format!("@{}", username))
                                            .text_sm()
                                            .text_color(theme.text_muted),
                                    ),
                            )
                            .child(div().flex_1())
                            .child(
                                GpuiButton::new("edit-profile-btn")
                                    .label(mezon_i18n::t(
                                        &locale,
                                        "setting.account.editUserProfile",
                                    ))
                                    .text_color(theme.text_primary)
                                    .ghost()
                                    .on_click(move |_, _, cx| {
                                        crate::router::replace(
                                            cx,
                                            crate::router::Route::SettingsProfile,
                                        );
                                    }),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    .shadow_md()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "setting.account.displayName",
                                            )),
                                    )
                                    .child(
                                        Label::new(display_name.clone())
                                            .text_color(theme.text_muted),
                                    ),
                            ),
                    )
                    .child(Divider::horizontal())
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "setting.account.username",
                                            )),
                                    )
                                    .child(
                                        Label::new(format!("@{}", username))
                                            .text_color(theme.text_muted),
                                    ),
                            ),
                    )
                    .child(Divider::horizontal())
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(&locale, "setting.account.email")),
                                    )
                                    .child(Label::new(email_display).text_color(theme.text_muted)),
                            ),
                    )
                    .child(Divider::horizontal())
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "setting.account.password",
                                            )),
                                    )
                                    .child(Label::new("••••••••••").text_color(theme.text_muted)),
                            )
                            .child(
                                GpuiButton::new("password-btn")
                                    .label(password_label)
                                    .text_color(theme.text_primary)
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let locale = this.settings.read(cx).language.clone();
                                        this.show_toast(
                                            mezon_i18n::t(
                                                &locale,
                                                "setting.account.passwordComingSoon",
                                            ),
                                            cx,
                                        );
                                    })),
                            ),
                    )
                    .child(Divider::horizontal())
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(&locale, "setting.account.phone")),
                                    )
                                    .child(Label::new(phone_display).text_color(theme.text_muted)),
                            )
                            .child(
                                GpuiButton::new("phone-btn")
                                    .label(phone_label)
                                    .text_color(theme.text_primary)
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let locale = this.settings.read(cx).language.clone();
                                        this.show_toast(
                                            mezon_i18n::t(
                                                &locale,
                                                "setting.account.phoneComingSoon",
                                            ),
                                            cx,
                                        );
                                    })),
                            ),
                    ),
            )
            .when_some(self.toast_message.clone(), |this, msg| {
                this.child(div().text_sm().text_color(theme.text_muted).child(msg))
            })
            .into_any_element()
    }
}

fn mask_email(email: &str) -> String {
    let at = email.find('@').unwrap_or(email.len());
    if at > 1 {
        format!("{}***{}", &email[..1], &email[at..])
    } else {
        email.to_string()
    }
}
