//! LoginView — Stage 1 auth screen.
//!
//! Two login modes:
//!   • OTP (default) — two-step: email → OTP code entry
//!   • Password — email + password form
//!
//! The view holds `Entity<AuthState>` and updates it on successful auth.
//! `Arc<MezonClient>` is injected at construction and used for all API calls.

use std::sync::Arc;

use gpui::{App, Context, Entity, FontWeight, MouseButton, Window, div, prelude::*};
use gpui_component::{
    Disableable as _,
    button::{Button, ButtonVariants as _},
};
use mezon_client::{MezonClient, Session, keychain};
use mezon_store::{AuthState, LoginMethod, Settings};

use crate::components::compositions::{FormField, OtpInput};
use crate::theme::resolve_theme;

// ─── LoginView state ──────────────────────────────────────────────────────────

pub struct LoginView {
    /// Injected API client.
    client: Arc<MezonClient>,
    /// Handle to the global auth state so we can transition it on success.
    auth_state: Entity<AuthState>,
    /// Handle to settings for theme resolution.
    settings: Entity<Settings>,

    /// Which login mode is active.
    method: LoginMethod,

    /// OTP mode — step 0: email entry; step 1: OTP code entry.
    otp_step: u8,
    /// The `req_id` returned by the server after a successful OTP request.
    otp_req_id: String,
    /// The email used for OTP (shown in the "code sent to …" label).
    otp_email: String,

    /// Shared email field (used by both modes on step 0).
    email_field: Option<Entity<FormField>>,
    /// Password field (password mode only).
    password_field: Option<Entity<FormField>>,
    /// OTP digit input with auto-advance.
    otp_input: Option<Entity<OtpInput>>,

    /// `true` while an async API call is in-flight.
    loading: bool,
    /// Displayed error message (None = hidden).
    error: Option<String>,
    /// Countdown in seconds for OTP resend (0 = show "Resend" button).
    countdown: u32,
}

impl LoginView {
    pub fn new(
        client: Arc<MezonClient>,
        auth_state: Entity<AuthState>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());
        Self {
            client,
            auth_state,
            settings,
            method: LoginMethod::Otp,
            otp_step: 0,
            otp_req_id: String::new(),
            otp_email: String::new(),
            email_field: None,
            password_field: None,
            otp_input: None,
            loading: false,
            error: None,
            countdown: 0,
        }
    }

    fn ensure_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.email_field.is_none() {
            self.email_field = Some(cx.new(|cx| FormField::new(window, cx, "Email")));
        }

        if self.password_field.is_none() {
            self.password_field = Some(cx.new(|cx| {
                let field = FormField::new(window, cx, "Password");
                field.set_masked(window, cx);
                field
            }));
        }

        if self.otp_input.is_none() {
            let entity = cx.entity().clone();
            self.otp_input = Some(cx.new(|cx| {
                OtpInput::new(window, cx, 6).on_complete(Arc::new(move |code, _window, cx| {
                    Self::handle_confirm_otp(&entity, code, cx);
                }))
            }));
        }
    }

    // ── Action handlers ───────────────────────────────────────────────────────

    /// Called when "Send OTP" is pressed.
    fn handle_send_otp(entity: &Entity<LoginView>, _window: &mut Window, cx: &mut App) {
        let email = entity
            .read(cx)
            .email_field
            .as_ref()
            .map(|field| field.read(cx).value(cx))
            .unwrap_or_default();
        if email.trim().is_empty() {
            entity.update(cx, |this, cx| {
                this.error = Some("Please enter your email address.".to_owned());
                cx.notify();
            });
            return;
        }

        entity.update(cx, |this, cx| {
            this.loading = true;
            this.error = None;
            cx.notify();
        });

        let client = entity.read(cx).client.clone();
        let email_clone = email.clone();
        let entity_clone = entity.clone();

        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            let result = client.request_otp(&email_clone).await;
            cx.update(|cx| {
                entity_clone.update(cx, |this, cx| {
                    this.loading = false;
                    match result {
                        Ok(req_id) => {
                            this.otp_req_id = req_id.clone();
                            this.otp_email = email_clone.clone();
                            this.otp_step = 1;
                            this.countdown = 60;
                            this.error = None;
                            // Sync store state so RootView knows OTP was sent.
                            this.auth_state.update(cx, |state, cx| {
                                *state = AuthState::OtpRequested {
                                    req_id,
                                    email: email_clone,
                                };
                                cx.notify();
                            });
                        }
                        Err(e) => {
                            this.error = Some(format!("{e}"));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();

        // Start countdown timer.
        Self::start_countdown(entity, cx);
    }

    /// Called when the user has filled all 6 OTP digits.
    fn handle_confirm_otp(entity: &Entity<LoginView>, otp_code: String, cx: &mut App) {
        let req_id = entity.read(cx).otp_req_id.clone();

        entity.update(cx, |this, cx| {
            this.loading = true;
            this.error = None;
            cx.notify();
        });

        let client = entity.read(cx).client.clone();
        let auth_state = entity.read(cx).auth_state.clone();
        let entity_clone = entity.clone();

        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            let result = client.confirm_otp(&req_id, &otp_code).await;
            cx.update(|cx| {
                entity_clone.update(cx, |this, cx| {
                    this.loading = false;
                    match result {
                        Ok(session) => {
                            Self::on_auth_success(session, &auth_state, cx);
                        }
                        Err(e) => {
                            this.error = Some(format!("{e}"));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Called when "Sign In" (password mode) is pressed.
    fn handle_sign_in(entity: &Entity<LoginView>, cx: &mut App) {
        let (email, password) = {
            let this = entity.read(cx);
            (
                this.email_field
                    .as_ref()
                    .map(|field| field.read(cx).value(cx))
                    .unwrap_or_default(),
                this.password_field
                    .as_ref()
                    .map(|field| field.read(cx).value(cx))
                    .unwrap_or_default(),
            )
        };

        if email.trim().is_empty() || password.is_empty() {
            entity.update(cx, |this, cx| {
                this.error = Some("Please enter your email and password.".to_owned());
                cx.notify();
            });
            return;
        }

        entity.update(cx, |this, cx| {
            this.loading = true;
            this.error = None;
            cx.notify();
        });

        let client = entity.read(cx).client.clone();
        let auth_state = entity.read(cx).auth_state.clone();
        let entity_clone = entity.clone();

        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            let result = client.authenticate_email(&email, &password).await;
            cx.update(|cx| {
                entity_clone.update(cx, |this, cx| {
                    this.loading = false;
                    match result {
                        Ok(session) => {
                            Self::on_auth_success(session, &auth_state, cx);
                        }
                        Err(e) => {
                            this.error = Some(format!("{e}"));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Shared post-auth success handler: save to keychain and transition state.
    fn on_auth_success(session: Session, auth_state: &Entity<AuthState>, cx: &mut App) {
        if let Err(e) = keychain::save_session(&session) {
            tracing::warn!("Failed to save session to keychain: {e}");
        }

        tracing::info!("✓ Authentication successful");
        tracing::info!("  User ID: {}", session.user_id);
        tracing::info!("  Username: {}", session.username);
        tracing::info!("  WS URL: {:?}", session.ws_url);
        tracing::info!("  API URL: {:?}", session.api_url);
        tracing::info!("  TCP URL: {:?}", session.tcp_url);

        auth_state.update(cx, |state, cx| {
            *state = AuthState::Connecting(session);
            tracing::debug!("User authenticated, connecting transport.");
            cx.notify();
        });
    }

    /// Start a 60-second countdown, ticking every second.
    fn start_countdown(entity: &Entity<LoginView>, cx: &mut App) {
        let entity_clone = entity.clone();
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            let exec = cx.background_executor().clone();
            loop {
                exec.timer(std::time::Duration::from_secs(1)).await;
                let should_stop = cx.update(|cx| {
                    entity_clone.update(cx, |this, cx| {
                        if this.countdown > 0 {
                            this.countdown -= 1;
                            cx.notify();
                        }
                        this.countdown == 0
                    })
                });
                if should_stop {
                    break;
                }
            }
        })
        .detach();
    }
}

// ─── Render ──────────────────────────────────────────────────────────────────

impl Render for LoginView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_fields(window, cx);
        let theme = resolve_theme(&self.settings.read(cx).theme);

        // Outer centered column.
        let root = div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .size_full();

        // Card container.
        let mut card = div()
            .flex()
            .flex_col()
            .gap_4()
            .w(gpui::px(360.0))
            .p_8()
            .rounded_lg()
            .bg(theme.bg_secondary);

        // Logo + wordmark.
        card = card.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .mb_2()
                .child(div().size_16().bg(theme.brand).rounded_lg())
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child("Mezon"),
                ),
        );

        match self.method {
            LoginMethod::Otp => {
                if self.otp_step == 0 {
                    // Step 0: email entry.
                    card = card
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child("Sign in with OTP"),
                        )
                        .child(self.email_field.as_ref().expect("email field").clone());

                    let loading = self.loading;
                    let entity = cx.entity().clone();
                    card = card.child(
                        div().w_full().child(
                            Button::new("send-otp")
                                .label("Send OTP")
                                .primary()
                                .w_full()
                                .loading(loading)
                                .disabled(loading)
                                .on_click(move |_, window, cx| {
                                    Self::handle_send_otp(&entity, window, cx);
                                })
                                .into_any_element(),
                        ),
                    );
                } else {
                    // Step 1: OTP code entry.
                    card =
                        card.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child("Enter verification code"),
                                )
                                .child(div().text_xs().text_color(theme.text_secondary).child(
                                    format!("We sent a 6-digit code to {}", self.otp_email),
                                )),
                        );

                    // OTP digit boxes with auto-advance.
                    card = card.child(self.otp_input.as_ref().expect("otp_input").clone());

                    // Loading spinner (shown while verifying code).
                    if self.loading {
                        card = card.child(
                            div()
                                .flex()
                                .justify_center()
                                .child(gpui_component::spinner::Spinner::new()),
                        );
                    }

                    // Resend / countdown row.
                    let countdown = self.countdown;
                    if countdown > 0 {
                        card = card.child(
                            div()
                                .flex()
                                .justify_center()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(format!("Resend code in {countdown}s")),
                        );
                    } else {
                        let entity = cx.entity().clone();
                        card = card.child(
                            div()
                                .flex()
                                .justify_center()
                                .text_xs()
                                .text_color(theme.brand)
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.8))
                                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                                    // Go back to email step and resend.
                                    entity.update(cx, |this, cx| {
                                        this.otp_step = 0;
                                        cx.notify();
                                    });
                                })
                                .child("Resend code"),
                        );
                    }

                    // Back link.
                    let entity_back = cx.entity().clone();
                    card = card.child(
                        div()
                            .flex()
                            .justify_center()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                                entity_back.update(cx, |this, cx| {
                                    this.otp_step = 0;
                                    this.otp_req_id.clear();
                                    this.error = None;
                                    cx.notify();
                                });
                            })
                            .child("← Change email"),
                    );
                }
            }

            LoginMethod::Password => {
                // Email + password form.
                card = card
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child("Sign in with password"),
                    )
                    .child(self.email_field.as_ref().expect("email field").clone())
                    .child(
                        self.password_field
                            .as_ref()
                            .expect("password field")
                            .clone(),
                    );

                // Forgot password link.
                card = card.child(
                    div()
                        .flex()
                        .justify_end()
                        .text_xs()
                        .text_color(theme.brand)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.8))
                        .on_mouse_down(MouseButton::Left, |_, _window, _cx| {
                            let _ = mezon_native::open_url("https://mezon.ai/forgot-password");
                        })
                        .child("Forgot password?"),
                );

                let loading = self.loading;
                let entity = cx.entity().clone();
                card = card.child(
                    div().w_full().child(
                        Button::new("sign-in")
                            .label("Sign In")
                            .primary()
                            .w_full()
                            .loading(loading)
                            .disabled(loading)
                            .on_click(move |_, _window, cx| {
                                Self::handle_sign_in(&entity, cx);
                            })
                            .into_any_element(),
                    ),
                );
            }
        }

        // Error label.
        if let Some(err) = &self.error {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(theme.status_dnd)
                    .child(err.clone()),
            );
        }

        // Divider.
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().h(gpui::px(1.0)).bg(theme.border))
                .child(div().text_xs().text_color(theme.text_muted).child("or"))
                .child(div().flex_1().h(gpui::px(1.0)).bg(theme.border)),
        );

        // Toggle login method link.
        let toggle_label = match self.method {
            LoginMethod::Otp => "Login by Password",
            LoginMethod::Password => "Login by OTP",
        };
        let entity_toggle = cx.entity().clone();
        card = card.child(
            div()
                .flex()
                .justify_center()
                .text_xs()
                .text_color(theme.brand)
                .cursor_pointer()
                .hover(|s| s.opacity(0.8))
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    entity_toggle.update(cx, |this, cx| {
                        this.method = match this.method {
                            LoginMethod::Otp => LoginMethod::Password,
                            LoginMethod::Password => LoginMethod::Otp,
                        };
                        this.otp_step = 0;
                        this.error = None;
                        cx.notify();
                    });
                })
                .child(toggle_label),
        );

        root.child(card)
    }
}
