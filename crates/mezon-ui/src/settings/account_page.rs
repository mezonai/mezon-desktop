use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, FontWeight, SharedString, Task, Window, div, prelude::*, px};
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
        let navigate = self.navigate.clone();

        let display_name = if account.display_name.is_empty() {
            account.username.clone()
        } else {
            account.display_name.clone()
        };

        let email_display = if account.email.is_empty() {
            SharedString::from("Not set")
        } else {
            SharedString::from(mask_email(&account.email))
        };

        let email_label = if account.email.is_empty() {
            SharedString::from("Set Email")
        } else {
            SharedString::from("Change Email")
        };

        let password_label = if account.password_setted {
            SharedString::from("Change Password")
        } else {
            SharedString::from("Set Password")
        };

        let phone_display = account
            .phone_number
            .clone()
            .unwrap_or(SharedString::from("Not set"));

        let phone_label = if account.phone_number.is_some() {
            SharedString::from("Change Phone")
        } else {
            SharedString::from("Set Phone")
        };

        let avatar_url = account.avatar_url.clone();

        v_flex()
            .gap_6()
            // Profile Card
            .child(
                v_flex()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    .child(
                        // Color Banner
                        div().h(px(100.0)).w_full().bg(theme.brand),
                    )
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
                                            .text_xl()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.text_primary),
                                    )
                                    .child(
                                        Label::new(format!("@{}", account.username))
                                            .text_sm()
                                            .text_color(theme.text_muted),
                                    ),
                            )
                            .child(div().flex_1())
                            .child(
                                GpuiButton::new("edit-profile-btn")
                                    .label("Edit User Profile")
                                    .text_color(theme.text_primary)
                                    .ghost()
                                    .on_click(move |_, _, cx| {
                                        navigate("/settings/profile", cx);
                                    }),
                            ),
                    ),
            )
            // Info Cards
            .child(
                v_flex()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    // Display Name
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
                                        Label::new("Display Name").text_color(theme.text_primary),
                                    )
                                    .child(
                                        Label::new(display_name.clone())
                                            .text_color(theme.text_muted),
                                    ),
                            ),
                    )
                    .child(Divider::horizontal())
                    // Username
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Label::new("Username").text_color(theme.text_primary))
                                    .child(
                                        Label::new(format!("@{}", account.username))
                                            .text_color(theme.text_muted),
                                    ),
                            ),
                    )
                    .child(Divider::horizontal())
                    // Email
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Label::new("Email").text_color(theme.text_primary))
                                    .child(Label::new(email_display).text_color(theme.text_muted)),
                            ),
                    )
                    .child(Divider::horizontal())
                    // Password
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Label::new("Password").text_color(theme.text_primary))
                                    .child(Label::new("••••••••••").text_color(theme.text_muted)),
                            )
                            .child(
                                GpuiButton::new("password-btn")
                                    .label(password_label)
                                    .text_color(theme.text_primary)
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_toast("Password management coming soon", cx);
                                    })),
                            ),
                    )
                    .child(Divider::horizontal())
                    // Phone
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .px_6()
                            .py_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Label::new("Phone").text_color(theme.text_primary))
                                    .child(Label::new(phone_display).text_color(theme.text_muted)),
                            )
                            .child(
                                GpuiButton::new("phone-btn")
                                    .label(phone_label)
                                    .text_color(theme.text_primary)
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_toast("Phone management coming soon", cx);
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
