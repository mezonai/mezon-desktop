use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, FontWeight, SharedString, Task, Window, div, prelude::*};
use gpui_component::{
    Sizable, Size,
    avatar::Avatar,
    button::{Button as GpuiButton, ButtonVariants},
    divider::Divider,
    h_flex,
    label::Label,
    v_flex,
};
use mezon_client::AppApi;

use crate::components::NavigateFn;
use crate::theme::Theme;
use crate::util::{check_connection, retry};

struct AccountState {
    username: SharedString,
    display_name: SharedString,
    email: SharedString,
    avatar_url: Option<SharedString>,
    phone_number: Option<SharedString>,
    password_setted: bool,
}

pub struct AccountPage {
    navigate: NavigateFn,
    account: Option<AccountState>,
    connection_ready: bool,
    connection_error: bool,
    fetch_error: bool,
    loading: bool,
    toast_message: Option<SharedString>,
    _fetch_task: Option<Task<()>>,
}

impl AccountPage {
    pub fn new(api: Arc<AppApi>, navigate: NavigateFn, cx: &mut Context<Self>) -> Self {
        let api_clone = api.clone();
        let fetch_task = cx.spawn(async move |this, cx| {
            if check_connection(cx.background_executor(), &api_clone)
                .await
                .is_err()
            {
                this.update(cx, |this, cx| {
                    this.connection_error = true;
                    this.loading = false;
                    cx.notify();
                })
                .ok();
                return;
            }

            this.update(cx, |this, cx| {
                this.connection_ready = true;
                cx.notify();
            })
            .ok();

            match retry(
                cx.background_executor(),
                5,
                Duration::from_millis(1000),
                || {
                    let api = api_clone.clone();
                    async move {
                        api.get_account().await.map_err(|e| {
                            tracing::error!("Failed to fetch account, retrying: {}", e);
                            e
                        })
                    }
                },
            )
            .await
            {
                Ok(acct) => {
                    this.update(cx, |this, view_cx| {
                        let display = acct
                            .display_name
                            .clone()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| acct.username.clone());
                        this.account = Some(AccountState {
                            username: acct.username.into(),
                            display_name: display.into(),
                            email: acct.email.unwrap_or_default().into(),
                            avatar_url: acct.avatar_url.map(Into::into),
                            phone_number: acct.phone_number.map(Into::into),
                            password_setted: acct.password_setted,
                        });
                        this.loading = false;
                        view_cx.notify();
                    })
                    .ok();
                }
                Err(_) => {
                    this.update(cx, |this, cx| {
                        this.fetch_error = true;
                        this.loading = false;
                        cx.notify();
                    })
                    .ok();
                }
            }
        });

        Self {
            navigate,
            account: None,
            connection_ready: false,
            connection_error: false,
            fetch_error: false,
            loading: true,
            toast_message: None,
            _fetch_task: Some(fetch_task),
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
        let theme = Theme::dark();

        if self.connection_error {
            return v_flex()
                .gap_4()
                .child(Label::new("Connection failed").text_color(theme.text_muted))
                .into_any_element();
        }

        if self.fetch_error {
            return v_flex()
                .gap_4()
                .child(Label::new("Failed to load account data").text_color(theme.text_muted))
                .into_any_element();
        }

        if !self.connection_ready {
            return v_flex()
                .gap_4()
                .child(Label::new("Connecting...").text_color(theme.text_muted))
                .into_any_element();
        }

        if self.loading || self.account.is_none() {
            return v_flex()
                .gap_4()
                .child(Label::new("Loading account...").text_color(theme.text_muted))
                .into_any_element();
        }

        let account = self.account.as_ref().unwrap();

        let username = account.username.clone();
        let display_name = if account.display_name.is_empty() {
            username.clone()
        } else {
            account.display_name.clone()
        };
        let email = if account.email.is_empty() {
            SharedString::from("No email")
        } else {
            account.email.clone()
        };
        let avatar_url = account.avatar_url.clone();
        let phone = account.phone_number.clone();
        let password_setted = account.password_setted;

        let password_label = if password_setted {
            SharedString::from("Change Password")
        } else {
            SharedString::from("Set Password")
        };

        let phone_display = phone.clone().unwrap_or(SharedString::from("Not set"));
        let phone_label = if phone.is_some() {
            SharedString::from("Change Phone")
        } else {
            SharedString::from("Set Phone")
        };

        v_flex()
            .gap_6()
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        Avatar::new()
                            .when_some(avatar_url, |av, url| av.src(url.clone()))
                            .name(display_name.clone())
                            .with_size(Size::Large),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(display_name)
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text_primary),
                            )
                            .child(
                                Label::new(format!("@{}", username))
                                    .text_sm()
                                    .text_color(theme.text_primary),
                            )
                            .child(Label::new(email).text_sm().text_color(theme.text_primary)),
                    )
                    .child(div().flex_1())
                    .child(
                        GpuiButton::new("edit-profile-btn")
                            .label("Edit")
                            .text_color(theme.text_primary)
                            .ghost()
                            .on_click({
                                let nav = self.navigate.clone();
                                move |_, _, cx| {
                                    nav("/settings/profile", cx);
                                }
                            }),
                    ),
            )
            .child(Divider::horizontal())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Label::new("Password").text_color(theme.text_primary))
                            .child(Label::new("••••••••••").text_color(theme.text_muted)),
                    )
                    .child(
                        GpuiButton::new("set-password-btn")
                            .label(password_label)
                            .text_color(theme.text_primary)
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_toast("Password management coming soon", cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Label::new("Phone").text_color(theme.text_primary))
                            .child(Label::new(phone_display).text_color(theme.text_muted)),
                    )
                    .child(
                        GpuiButton::new("set-phone-btn")
                            .label(phone_label)
                            .text_color(theme.text_primary)
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_toast("Phone management coming soon", cx);
                            })),
                    ),
            )
            .when_some(self.toast_message.clone(), |this, msg| {
                this.child(div().text_sm().text_color(theme.text_muted).child(msg))
            })
            .into_any_element()
    }
}
