use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, Context, Entity, FontWeight, Hsla, MouseButton, RenderImage,
    Subscription, Task, Window, div, img, linear_color_stop, linear_gradient, prelude::*, px, rgb,
    rgba, svg, white,
};
use mezon_store::{AuthState, LoginMethod, LoginStore, Session, Settings, WalletStore};

use crate::app::window_controls;
use crate::components::compositions::OtpInput;
use crate::components::primitives::{Input, InputEvent, InputState, Spinner};

pub struct LoginView {
    auth_state: Entity<AuthState>,
    settings: Entity<Settings>,

    method: LoginMethod,

    otp_step: u8,
    otp_req_id: String,
    otp_email: String,

    email_input: Option<Entity<InputState>>,
    password_input: Option<Entity<InputState>>,
    otp_input: Option<Entity<OtpInput>>,

    loading: bool,
    show_loading: bool,
    signin_failed: bool,
    error: Option<String>,
    countdown: u32,

    qr_login_id: Option<String>,
    qr_expired: bool,
    qr_image: Option<Arc<RenderImage>>,
    qr_image_blurred: Option<Arc<RenderImage>>,
    is_remember: bool,
    pending_reset: bool,
    pending_otp_clear: bool,
    _field_subs: Vec<Subscription>,
    _loading_task: Option<Task<()>>,
    _countdown_task: Option<Task<()>>,
    _qr_poll_task: Option<Task<()>>,
}

impl LoginView {
    pub fn new(
        auth_state: Entity<AuthState>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        let qr_auth_state = auth_state.clone();
        let qr_auth_state_for_observe = qr_auth_state.clone();
        cx.observe(&auth_state, move |this, auth, cx| {
            match auth.read(cx) {
                AuthState::NotAuthenticated => {
                    this.reset_to_initial();
                    if this._qr_poll_task.is_none() {
                        this._qr_poll_task =
                            Some(Self::start_qr_flow(qr_auth_state_for_observe.clone(), cx));
                    }
                }
                AuthState::OtpRequested { .. } => {
                    if this._qr_poll_task.is_none() {
                        this._qr_poll_task =
                            Some(Self::start_qr_flow(qr_auth_state_for_observe.clone(), cx));
                    }
                }
                _ => {
                    this._qr_poll_task = None;
                    this._countdown_task = None;
                    this._loading_task = None;
                }
            }
            cx.notify();
        })
        .detach();
        let mut view = Self {
            auth_state,
            settings,
            method: LoginMethod::Otp,
            otp_step: 0,
            otp_req_id: String::new(),
            otp_email: String::new(),
            email_input: None,
            password_input: None,
            otp_input: None,
            loading: false,
            show_loading: false,
            signin_failed: false,
            error: None,
            countdown: 0,
            qr_login_id: None,
            qr_expired: false,
            qr_image: None,
            qr_image_blurred: None,
            is_remember: false,
            pending_reset: false,
            pending_otp_clear: false,
            _field_subs: Vec::new(),
            _loading_task: None,
            _countdown_task: None,
            _qr_poll_task: None,
        };
        if matches!(
            view.auth_state.read(cx),
            AuthState::NotAuthenticated | AuthState::OtpRequested { .. }
        ) {
            view._qr_poll_task = Some(Self::start_qr_flow(qr_auth_state.clone(), cx));
        }
        view
    }

    fn reset_to_initial(&mut self) {
        self.method = LoginMethod::Otp;
        self.otp_step = 0;
        self.otp_req_id.clear();
        self.otp_email.clear();
        self.error = None;
        self.countdown = 0;
        self.is_remember = false;
        self.qr_login_id = None;
        self.qr_image = None;
        self.qr_image_blurred = None;
        self.qr_expired = false;
        self.loading = false;
        self.show_loading = false;
        self.signin_failed = false;
        self._loading_task = None;
        self._countdown_task = None;
        self._qr_poll_task = None;
        self.pending_reset = true;
    }

    fn reset_flow_state(&mut self) {
        self.otp_step = 0;
        self.otp_req_id.clear();
        self.countdown = 0;
        self._countdown_task = None;
        self.signin_failed = false;
        self.error = None;
    }

    fn begin_loading(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.show_loading = false;
        self.error = None;
        self._loading_task = Some(cx.spawn(async move |this, cx| {
            let exec = cx.background_executor().clone();
            exec.timer(Duration::from_millis(LOADING_SHOW_DELAY_MS))
                .await;
            let still_loading = this
                .update(cx, |this, cx| {
                    if this.loading {
                        this.show_loading = true;
                        cx.notify();
                    }
                    this.loading
                })
                .unwrap_or(false);
            if !still_loading {
                return;
            }
            exec.timer(Duration::from_secs(LOADING_TIMEOUT_SECS)).await;
            let _ = this.update(cx, |this, cx| {
                if this.loading {
                    this.loading = false;
                    this.show_loading = false;
                    let locale = this.settings.read(cx).language.clone();
                    this.error = Some(
                        mezon_i18n::t(&locale, "common.errorBoundary.connectionFailed").to_string(),
                    );
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn end_loading(&mut self, cx: &mut Context<Self>) {
        self.loading = false;
        self.show_loading = false;
        self._loading_task = None;
        cx.notify();
    }

    fn ensure_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        if self.email_input.is_none() {
            let placeholder = mezon_i18n::t(&locale, "common.login.enterEmail");
            let input = cx.new(|cx| Self::login_input(window, cx, placeholder));
            self._field_subs
                .push(cx.subscribe_in(&input, window, Self::on_field_change));
            self.email_input = Some(input);
        }

        if self.password_input.is_none() {
            let placeholder = mezon_i18n::t(&locale, "common.login.password");
            let input = cx.new(|cx| {
                let mut input = Self::login_input(window, cx, placeholder).padding_right(px(64.));
                input.set_masked(true, window, cx);
                input
            });
            self._field_subs
                .push(cx.subscribe_in(&input, window, Self::on_field_change));
            self.password_input = Some(input);
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

    fn on_field_change(
        &mut self,
        _input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.signin_failed = false;
                cx.notify();
            }
            InputEvent::PressEnter => {
                let entity = cx.entity();
                let method = self.method;
                let otp_step = self.otp_step;
                window.defer(cx, move |window, cx| match method {
                    LoginMethod::Password => Self::handle_sign_in(&entity, cx),
                    LoginMethod::Otp => {
                        if otp_step == 0 {
                            Self::handle_send_otp(&entity, window, cx);
                        }
                    }
                });
            }
        }
    }

    fn login_input(
        window: &mut Window,
        cx: &mut Context<InputState>,
        placeholder: &str,
    ) -> InputState {
        InputState::new(window, cx)
            .placeholder(placeholder.to_string())
            .height(px(48.))
            .radius(px(8.))
            .padding_x(px(16.))
            .text_size(px(16.))
            .bg(rgb(0x1e1e1e))
            .text_color(white())
    }

    fn start_qr_flow(auth_state: Entity<AuthState>, cx: &mut Context<LoginView>) -> Task<()> {
        let client = LoginStore::global(cx).read(cx).client();
        cx.spawn(async move |this, cx| {
            let qr_result = client.create_qr_login().await;
            let login_id = match qr_result {
                Ok(qr) => qr.login_id,
                Err(e) => {
                    tracing::warn!("QR login create failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.qr_expired = true;
                        cx.notify();
                    });
                    return;
                }
            };
            let exec = cx.background_executor().clone();
            let qr_payload = format!("mezon.ai://login/{login_id}");
            let qr_images = exec
                .spawn(async move { build_qr_images(&qr_payload) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let (sharp, blurred) = match qr_images {
                    Some(images) => (Some(images.0), Some(images.1)),
                    None => (None, None),
                };
                this.qr_image = sharp;
                this.qr_image_blurred = blurred;
                this.qr_login_id = Some(login_id.clone());
                cx.notify();
            });

            let mut elapsed: u32 = 0;
            loop {
                exec.timer(std::time::Duration::from_secs(2)).await;
                elapsed += 2;
                if elapsed >= 60 {
                    let _ = this.update(cx, |this, cx| {
                        this.qr_expired = true;
                        cx.notify();
                    });
                    break;
                }
                let is_remember = this
                    .read_with(cx, |this, _| this.is_remember)
                    .unwrap_or(false);
                let result = client.confirm_qr_login(&login_id, is_remember).await;
                if let Ok(session) = result
                    && !session.token.is_empty()
                {
                    let _ = this.update(cx, |_this, cx| {
                        Self::on_auth_success(session, &auth_state, cx);
                    });
                    break;
                }
            }
        })
    }

    fn handle_send_otp(entity: &Entity<LoginView>, _window: &mut Window, cx: &mut App) {
        let email = entity
            .read(cx)
            .email_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        if let Some(err_key) = email_error_key(&email) {
            entity.update(cx, |this, cx| {
                let locale = this.settings.read(cx).language.clone();
                this.error = Some(mezon_i18n::t(&locale, err_key).to_string());
                cx.notify();
            });
            return;
        }

        if entity.read(cx).loading {
            return;
        }

        entity.update(cx, |this, cx| {
            this.begin_loading(cx);
        });

        let client = LoginStore::global(cx).read(cx).client();
        let email_clone = email.clone();
        let entity_clone = entity.clone();

        cx.spawn(async move |cx| {
            let result = client.request_otp(&email_clone).await;
            entity_clone.update(cx, |this, cx| {
                this.end_loading(cx);
                match result {
                    Ok(req_id) => {
                        this.otp_req_id = req_id.clone();
                        this.otp_email = email_clone.clone();
                        this.otp_step = 1;
                        this.countdown = OTP_COOLDOWN_SECS;
                        this.error = None;
                        this.pending_otp_clear = true;
                        this.auth_state.update(cx, |state, cx| {
                            *state = AuthState::OtpRequested {
                                req_id,
                                email: email_clone,
                            };
                            cx.notify();
                        });
                        this.start_countdown(cx);
                    }
                    Err(e) => {
                        let locale = this.settings.read(cx).language.clone();
                        this.error = Some(friendly_error(&locale, &e));
                        if is_rate_limited(&e) {
                            this.countdown = OTP_COOLDOWN_SECS;
                            this.start_countdown(cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn handle_confirm_otp(entity: &Entity<LoginView>, otp_code: String, cx: &mut App) {
        if entity.read(cx).loading {
            return;
        }

        let req_id = entity.read(cx).otp_req_id.clone();

        entity.update(cx, |this, cx| {
            this.begin_loading(cx);
        });

        let client = LoginStore::global(cx).read(cx).client();
        let auth_state = entity.read(cx).auth_state.clone();
        let entity_clone = entity.clone();

        cx.spawn(async move |cx| {
            let result = client.confirm_otp(&req_id, &otp_code).await;
            entity_clone.update(cx, |this, cx| {
                this.end_loading(cx);
                match result {
                    Ok(session) => {
                        Self::on_auth_success(session, &auth_state, cx);
                    }
                    Err(e) => {
                        let locale = this.settings.read(cx).language.clone();
                        this.error = Some(confirm_otp_error(&locale, &e));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn handle_sign_in(entity: &Entity<LoginView>, cx: &mut App) {
        let (email, password) = {
            let this = entity.read(cx);
            (
                this.email_input
                    .as_ref()
                    .map(|input| input.read(cx).value().to_string())
                    .unwrap_or_default(),
                this.password_input
                    .as_ref()
                    .map(|input| input.read(cx).value().to_string())
                    .unwrap_or_default(),
            )
        };

        let validation_key = email_error_key(&email).or({
            if password.is_empty() {
                Some("common.enterPassword")
            } else {
                None
            }
        });
        if let Some(err_key) = validation_key {
            entity.update(cx, |this, cx| {
                let locale = this.settings.read(cx).language.clone();
                this.error = Some(mezon_i18n::t(&locale, err_key).to_string());
                cx.notify();
            });
            return;
        }

        if entity.read(cx).loading {
            return;
        }

        entity.update(cx, |this, cx| {
            this.begin_loading(cx);
        });

        let client = LoginStore::global(cx).read(cx).client();
        let auth_state = entity.read(cx).auth_state.clone();
        let entity_clone = entity.clone();

        cx.spawn(async move |cx| {
            let result = client.authenticate_email(&email, &password).await;
            entity_clone.update(cx, |this, cx| {
                this.end_loading(cx);
                match result {
                    Ok(session) => {
                        Self::on_auth_success(session, &auth_state, cx);
                    }
                    Err(e) => {
                        let locale = this.settings.read(cx).language.clone();
                        this.error = Some(friendly_error(&locale, &e));
                        this.signin_failed = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_auth_success(session: Session, auth_state: &Entity<AuthState>, cx: &mut App) {
        tracing::info!("Authentication successful");
        tracing::debug!("  User ID: {}", session.user_id);
        tracing::debug!("  Username: {}", session.username);

        let session_for_keychain = session.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = LoginStore::persist_session(&session_for_keychain) {
                    tracing::warn!("Failed to save session to keychain: {e}");
                }
            })
            .detach();

        auth_state.update(cx, |state, cx| {
            *state = AuthState::Connecting(session.clone());
            tracing::debug!("User authenticated, connecting transport.");
            cx.notify();
        });

        if !session.id_token.is_empty() {
            WalletStore::global(cx).update(cx, |wallet, cx| {
                wallet.fetch_zk_proofs_after_login(&session, cx);
            });
        }
    }

    fn start_countdown(&mut self, cx: &mut Context<Self>) {
        self._countdown_task = Some(cx.spawn(async move |this, cx| {
            let exec = cx.background_executor().clone();
            loop {
                exec.timer(Duration::from_secs(1)).await;
                let should_stop = this
                    .update(cx, |this, cx| {
                        if this.countdown > 0 {
                            this.countdown -= 1;
                            cx.notify();
                        }
                        this.countdown == 0
                    })
                    .unwrap_or(true);
                if should_stop {
                    break;
                }
            }
        }));
    }

    fn reload_qr(entity: &Entity<LoginView>, cx: &mut App) {
        entity.update(cx, |this, cx| {
            this.qr_login_id = None;
            this.qr_image = None;
            this.qr_image_blurred = None;
            this.qr_expired = false;
            let auth_state = this.auth_state.clone();
            this._qr_poll_task = Some(Self::start_qr_flow(auth_state, cx));
            cx.notify();
        });
    }

    fn render_otp_form(&self, locale: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let show_loading = self.show_loading;
        let mut col = div().flex().flex_col();

        if self.otp_step == 0 {
            if let Some(input) = &self.email_input {
                col = col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(required_label(mezon_i18n::t(locale, "common.login.email")))
                        .child(Input::new(input)),
                );
            }
            let email = self
                .email_input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default();
            let countdown = self.countdown;
            let send_disabled = show_loading || !is_valid_email(&email) || countdown > 0;
            let send_label = if countdown > 0 {
                format!(
                    "{} ({})",
                    mezon_i18n::t(locale, "common.login.send"),
                    countdown
                )
            } else {
                mezon_i18n::t(locale, "common.login.send").to_string()
            };
            let entity = cx.entity().clone();
            col = col.child(div().mt_3().child(login_button(
                "otp-send",
                send_label,
                show_loading,
                send_disabled,
                move |window, cx| Self::handle_send_otp(&entity, window, cx),
            )));
        } else {
            let masked = mask_email(&self.otp_email);
            let message =
                mezon_i18n::t(locale, "common.login.otpSentMessage").replace("{{email}}", &masked);
            col = col.child(
                div()
                    .text_sm()
                    .text_center()
                    .mb_4()
                    .text_color(rgb(0xd1d5db))
                    .child(message),
            );
            if let Some(input) = &self.otp_input {
                col = col.child(input.clone());
            }
            let label = if self.countdown > 0 {
                format!(
                    "{} ({})",
                    mezon_i18n::t(locale, "common.login.resendOtp"),
                    self.countdown
                )
            } else {
                mezon_i18n::t(locale, "common.login.resendOtp").to_string()
            };
            let entity = cx.entity().clone();
            col = col.child(div().mt_3().child(login_button(
                "otp-resend",
                label,
                show_loading,
                show_loading || self.countdown > 0,
                move |window, cx| Self::handle_send_otp(&entity, window, cx),
            )));
            if self.countdown == 0 {
                let entity_back = cx.entity().clone();
                col = col.child(
                    div()
                        .mt_2()
                        .flex()
                        .justify_center()
                        .text_sm()
                        .text_color(rgb(0x3b82f6))
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.8))
                        .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                            entity_back.update(cx, |this, cx| {
                                this.reset_flow_state();
                                cx.notify();
                            });
                        })
                        .child(mezon_i18n::t(locale, "common.login.backToEmailInput")),
                );
            }
        }

        col = col.child(error_slot(self.error.as_deref()));
        col = col.child(self.otp_footer(locale, cx));

        div()
            .flex()
            .flex_col()
            .min_h(px(FORM_MIN_HEIGHT))
            .justify_center()
            .child(col)
            .into_any_element()
    }

    fn otp_footer(&self, locale: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entity = cx.entity().clone();
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(white())
                    .child(mezon_i18n::t(locale, "common.login.cannotAccessYourEmail")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x3b82f6))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                        entity.update(cx, |this, cx| {
                            this.reset_flow_state();
                            this.method = LoginMethod::Password;
                            cx.notify();
                        });
                    })
                    .child(mezon_i18n::t(locale, "common.login.loginByPassword")),
            )
            .into_any_element()
    }

    fn render_password_form(&self, locale: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let show_loading = self.show_loading;
        let mut col = div().flex().flex_col();
        if let Some(input) = &self.email_input {
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(required_label(mezon_i18n::t(locale, "common.login.email")))
                    .child(Input::new(input)),
            );
        }
        if let Some(input) = &self.password_input {
            col = col.child(
                div()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(required_label(mezon_i18n::t(
                        locale,
                        "common.login.password",
                    )))
                    .child(Input::new(input).mask_toggle()),
            );
        }
        col = col.child(error_slot(self.error.as_deref()));
        let email = self
            .email_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        let password = self
            .password_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        let signin_disabled =
            show_loading || self.signin_failed || !is_valid_email(&email) || password.is_empty();
        let entity = cx.entity().clone();
        div()
            .flex()
            .flex_col()
            .min_h(px(FORM_MIN_HEIGHT))
            .justify_between()
            .child(col)
            .child(login_button(
                "password-signin",
                mezon_i18n::t(locale, "common.login.logIn").to_string(),
                show_loading,
                signin_disabled,
                move |_window, cx| Self::handle_sign_in(&entity, cx),
            ))
            .into_any_element()
    }

    fn render_qr_panel(&self, locale: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let expired = self.qr_expired;

        let mut qr_box = div()
            .relative()
            .size(px(192.))
            .p(px(12.))
            .rounded(px(8.))
            .bg(white())
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden();

        let shown = if expired {
            self.qr_image_blurred.as_ref().or(self.qr_image.as_ref())
        } else {
            self.qr_image.as_ref()
        };
        if let Some(image) = shown {
            let mut qr_img = img(image.clone()).size(px(QR_DISPLAY_PX));
            if expired {
                qr_img = qr_img.opacity(QR_EXPIRED_OPACITY);
            }
            qr_box = qr_box.child(qr_img);
        } else if !expired {
            qr_box = qr_box.child(Spinner::new());
        }

        if expired {
            let entity = cx.entity().clone();
            qr_box = qr_box.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                        Self::reload_qr(&entity, cx);
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(44.))
                            .rounded_full()
                            .bg(white())
                            .shadow_md()
                            .child(
                                svg()
                                    .path("icons/reload-icon.svg")
                                    .size(px(22.))
                                    .text_color(rgb(0x155eef)),
                            ),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .w(px(224.))
            .h(px(320.))
            .p_4()
            .rounded(px(8.))
            .shadow_md()
            .bg(white())
            .child(qr_box)
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x6b7280))
                    .child(mezon_i18n::t(locale, "common.login.qr.signIn")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x9ca3af))
                    .child(mezon_i18n::t(locale, "common.login.qr.useMobile")),
            )
            .into_any_element()
    }
}

impl Render for LoginView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_fields(window, cx);
        if self.pending_reset {
            self.pending_reset = false;
            if let Some(input) = self.email_input.clone() {
                input.update(cx, |i, cx| i.set_value("", window, cx));
            }
            if let Some(input) = self.password_input.clone() {
                input.update(cx, |i, cx| i.set_value("", window, cx));
            }
            if let Some(otp) = self.otp_input.clone() {
                otp.update(cx, |o, cx| o.clear(window, cx));
            }
        }
        if self.pending_otp_clear {
            self.pending_otp_clear = false;
            if let Some(otp) = self.otp_input.clone() {
                otp.update(cx, |o, cx| o.clear(window, cx));
            }
        }
        let locale = self.settings.read(cx).language.clone();

        let blue = rgb(0x3b82f6);
        let purple = rgb(0xa855f7);
        let pink = rgb(0xf9a8d4);
        let gray400 = rgb(0x9ca3af);

        let is_otp = self.method == LoginMethod::Otp;
        let on_otp_step1 = is_otp && self.otp_step == 1;

        let mut left_col = div().flex().flex_col().flex_1().min_w_0();

        left_col = left_col.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .mb_2()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::BOLD)
                        .mb_1()
                        .text_color(white())
                        .child(mezon_i18n::t(&locale, "common.login.welcomeBack")),
                )
                .child(
                    div()
                        .text_color(gray400)
                        .child(mezon_i18n::t(&locale, "common.login.gladToMeetAgain")),
                ),
        );

        left_col = match self.method {
            LoginMethod::Otp => left_col.child(self.render_otp_form(&locale, cx)),
            LoginMethod::Password => left_col.child(self.render_password_form(&locale, cx)),
        };

        if !on_otp_step1 {
            let checked = self.is_remember;
            let entity_remember = cx.entity().clone();
            let checkbox = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    entity_remember.update(cx, |this, cx| {
                        this.is_remember = !this.is_remember;
                        cx.notify();
                    });
                })
                .child(
                    div()
                        .size(px(16.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(if checked { blue } else { rgb(0x6b7280) })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(checked, |el| el.bg(blue))
                        .when(checked, |el| {
                            el.child(
                                svg()
                                    .path("icons/check.svg")
                                    .size(px(11.))
                                    .text_color(white()),
                            )
                        }),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(gray400)
                        .child(mezon_i18n::t(&locale, "common.login.keepSignedIn")),
                );
            let mut row = div()
                .mt_4()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(checkbox);
            if !is_otp {
                let entity_toggle = cx.entity().clone();
                row = row.child(
                    div()
                        .text_sm()
                        .text_color(blue)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.8))
                        .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                            entity_toggle.update(cx, |this, cx| {
                                this.reset_flow_state();
                                this.method = LoginMethod::Otp;
                                cx.notify();
                            });
                        })
                        .child(mezon_i18n::t(&locale, "common.login.loginByOTP")),
                );
            }
            left_col = left_col.child(row);
        }

        let card = div()
            .flex()
            .flex_row()
            .gap_8()
            .items_center()
            .w(px(800.))
            .max_w_full()
            .p(px(56.))
            .rounded(px(16.))
            .shadow_lg()
            .bg(rgb(0x0b0b0b))
            .text_color(white())
            .occlude()
            .child(left_col)
            .child(self.render_qr_panel(&locale, cx));

        let background = window_controls::window_drag_handle(
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    135.,
                    linear_color_stop(blue, 0.),
                    linear_color_stop(purple, 0.5),
                ))
                .child(div().absolute().inset_0().bg(linear_gradient(
                    135.,
                    linear_color_stop(rgba(0xf9a8d400), 0.5),
                    linear_color_stop(pink, 1.),
                ))),
        );

        div()
            .relative()
            .flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .px_4()
            .child(background)
            .child(card)
    }
}

fn build_qr_images(data: &str) -> Option<(Arc<RenderImage>, Arc<RenderImage>)> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    if width == 0 {
        return None;
    }
    let colors = code.to_colors();
    let scale = (336 / width).max(2);
    let dim = (width * scale) as u32;
    let mut buf = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        dim,
        dim,
        image::Rgba([255, 255, 255, 255]),
    );
    for (i, color) in colors.iter().enumerate() {
        if *color != qrcode::Color::Dark {
            continue;
        }
        let ox = ((i % width) * scale) as u32;
        let oy = ((i / width) * scale) as u32;
        for dy in 0..scale as u32 {
            for dx in 0..scale as u32 {
                buf.put_pixel(ox + dx, oy + dy, image::Rgba([0, 0, 0, 255]));
            }
        }
    }
    let blur_dim = qr_blur_bitmap_dim(dim);
    let downscaled = image::imageops::resize(
        &buf,
        blur_dim,
        blur_dim,
        image::imageops::FilterType::Triangle,
    );
    let blurred = image::imageops::blur(&downscaled, qr_expired_blur_sigma(blur_dim));
    Some((
        Arc::new(RenderImage::new(vec![image::Frame::new(buf)])),
        Arc::new(RenderImage::new(vec![image::Frame::new(blurred)])),
    ))
}

fn qr_blur_bitmap_dim(bitmap_dim: u32) -> u32 {
    (bitmap_dim / QR_EXPIRED_BLUR_DOWNSCALE).max(QR_EXPIRED_BLUR_MIN_DIM)
}

fn qr_expired_blur_sigma(bitmap_dim: u32) -> f32 {
    QR_EXPIRED_BLUR_PX * bitmap_dim as f32 / QR_DISPLAY_PX
}

const QR_DISPLAY_PX: f32 = 168.;
const QR_EXPIRED_BLUR_PX: f32 = 12.;
const QR_EXPIRED_BLUR_DOWNSCALE: u32 = 4;
const QR_EXPIRED_BLUR_MIN_DIM: u32 = 48;
const QR_EXPIRED_OPACITY: f32 = 0.5;
const MAX_ERROR_LEN: usize = 120;
const ERROR_SLOT_HEIGHT: f32 = 32.;
const FORM_MIN_HEIGHT: f32 = 256.;
const OTP_COOLDOWN_SECS: u32 = 60;
const LOADING_SHOW_DELAY_MS: u64 = 200;
const LOADING_TIMEOUT_SECS: u64 = 30;

fn is_rate_limited(err: &anyhow::Error) -> bool {
    let chain: String = err
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    chain.contains("HTTP 429") || chain.to_lowercase().contains("too frequently")
}

fn error_slot(error: Option<&str>) -> gpui::AnyElement {
    let mut slot = div()
        .w_full()
        .min_w_0()
        .h(px(ERROR_SLOT_HEIGHT))
        .overflow_hidden();
    if let Some(err) = error {
        slot = slot.child(
            div()
                .w_full()
                .text_xs()
                .text_color(rgb(0xef4444))
                .child(err.to_string()),
        );
    }
    slot.into_any_element()
}

fn is_valid_email(email: &str) -> bool {
    let email = email.trim();
    if email.is_empty() || email.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && matches!(
            domain.rsplit_once('.'),
            Some((host, tld)) if !host.is_empty() && !tld.is_empty()
        )
}

fn email_error_key(email: &str) -> Option<&'static str> {
    if email.trim().is_empty() {
        Some("common.login.enterEmailToLogin")
    } else if !is_valid_email(email) {
        Some("common.login.invalidEmail")
    } else {
        None
    }
}

fn confirm_otp_error(locale: &str, err: &anyhow::Error) -> String {
    let chain: String = err
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(" :: ");
    if chain.contains("Network error") {
        return mezon_i18n::t(locale, "common.errorBoundary.offlineMessage").to_string();
    }
    mezon_i18n::t(locale, "common.login.invalidOtp").to_string()
}

fn friendly_error(locale: &str, err: &anyhow::Error) -> String {
    let chain: String = err
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(" :: ");
    if chain.contains("Network error") {
        return mezon_i18n::t(locale, "common.errorBoundary.offlineMessage").to_string();
    }
    match extract_backend_message(&chain) {
        Some(message) => clamp_text(&message),
        None => mezon_i18n::t(locale, "common.login.genericError").to_string(),
    }
}

fn extract_backend_message(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&raw[start..=end]).ok()?;
    let message = value
        .get("message")
        .and_then(|v| v.as_str())
        .or_else(|| value.pointer("/data/message").and_then(|v| v.as_str()))
        .or_else(|| value.pointer("/error/message").and_then(|v| v.as_str()))
        .or_else(|| {
            value
                .pointer("/response/data/message")
                .and_then(|v| v.as_str())
        })?;
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    Some(message.to_string())
}

fn clamp_text(text: &str) -> String {
    if text.chars().count() <= MAX_ERROR_LEN {
        return text.to_string();
    }
    let truncated: String = text.chars().take(MAX_ERROR_LEN).collect();
    format!("{}…", truncated.trim_end())
}

fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let head: String = local.chars().take(3).collect();
            let masked = "*".repeat(local.chars().count().saturating_sub(3));
            format!("{head}{masked}@{domain}")
        }
        None => email.to_string(),
    }
}

fn login_button(
    id: &'static str,
    label: String,
    loading: bool,
    disabled: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let inactive = disabled || loading;
    let mut btn = div()
        .id(id)
        .w_full()
        .h(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(rgb(0x5265ec))
        .text_color(white())
        .text_sm()
        .font_weight(FontWeight::MEDIUM);
    if inactive {
        btn = btn.opacity(0.5);
    } else {
        btn = btn
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x4752c4)))
            .on_click(move |_, window, cx| on_click(window, cx));
    }
    if loading {
        btn.child(typing_dots(white())).into_any_element()
    } else {
        btn.child(label).into_any_element()
    }
}

fn typing_dots(color: impl Into<Hsla>) -> gpui::AnyElement {
    let color = color.into();
    let dot = move |i: usize, offset: f32| {
        div().size(px(7.)).rounded_full().bg(color).with_animation(
            ("login-typing-dot", i),
            Animation::new(Duration::from_secs_f64(0.9)).repeat(),
            move |el, delta| {
                let p = (delta + offset) % 1.0;
                let o = 0.3 + 0.7 * (1.0 - (2.0 * p - 1.0).abs());
                el.opacity(o)
            },
        )
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_1()
        .child(dot(0, 0.0))
        .child(dot(1, 0.2))
        .child(dot(2, 0.4))
        .into_any_element()
}

fn required_label(label: &str) -> gpui::AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(0xd1d5db))
        .child(label.to_string())
        .child(div().text_color(rgb(0xef4444)).child(" *"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_qr_blur_is_scaled_from_display_px_to_bitmap_px() {
        assert_eq!(qr_expired_blur_sigma(QR_DISPLAY_PX as u32), 12.);
        assert_eq!(qr_expired_blur_sigma(336), 24.);
    }

    #[test]
    fn build_qr_images_blurs_a_downscaled_copy_not_the_full_bitmap() {
        let (sharp, blurred) = build_qr_images("mezon-login-id").expect("qr images");
        let sharp_len = sharp.as_ref().as_bytes(0).expect("sharp frame").len();
        let blurred_len = blurred.as_ref().as_bytes(0).expect("blurred frame").len();

        assert!(blurred_len > 0);
        assert!(
            blurred_len < sharp_len,
            "the expired QR is upscaled from a small blur, so it must not carry the full bitmap"
        );
    }

    #[test]
    fn qr_blur_downscale_keeps_a_usable_bitmap_and_a_small_kernel() {
        assert_eq!(qr_blur_bitmap_dim(320), 80);
        assert_eq!(
            qr_blur_bitmap_dim(96),
            QR_EXPIRED_BLUR_MIN_DIM,
            "a tiny QR must not be downscaled past the floor"
        );
        assert!(
            qr_expired_blur_sigma(qr_blur_bitmap_dim(320)) < 6.0,
            "blur cost is driven by the kernel, which grows with sigma"
        );
    }

    #[test]
    fn extracts_message_from_http_error() {
        let e = anyhow::anyhow!(
            "HTTP 429 on POST https://api.example.com/v2/account/authenticate/emailotp: {{\"code\":3,\"message\":\"Email OTP requested too frequently, please try again later\"}}"
        );
        assert_eq!(
            friendly_error("en", &e),
            "Email OTP requested too frequently, please try again later"
        );
    }

    #[test]
    fn confirm_otp_maps_backend_error_to_invalid_otp() {
        let e = anyhow::anyhow!(
            "HTTP 500 on POST https://api.example.com/v2/account/authenticate/confirmotp: {{\"code\":13,\"message\":\"Internal Error\"}}"
        );
        assert_eq!(
            confirm_otp_error("en", &e),
            mezon_i18n::t("en", "common.login.invalidOtp")
        );
    }

    #[test]
    fn confirm_otp_network_error_shows_offline_message() {
        let e = anyhow::anyhow!("error sending request")
            .context("Network error on POST https://api.example.com/x");
        assert_eq!(
            confirm_otp_error("en", &e),
            mezon_i18n::t("en", "common.errorBoundary.offlineMessage")
        );
    }

    #[test]
    fn network_error_shows_offline_message() {
        let e = anyhow::anyhow!("error sending request")
            .context("Network error on POST https://api.example.com/x");
        assert_eq!(
            friendly_error("en", &e),
            mezon_i18n::t("en", "common.errorBoundary.offlineMessage")
        );
    }

    #[test]
    fn missing_message_uses_generic_fallback() {
        let e = anyhow::anyhow!("HTTP 500 on POST https://x: {{\"code\":13}}");
        assert_eq!(
            friendly_error("en", &e),
            mezon_i18n::t("en", "common.login.genericError")
        );
    }

    #[test]
    fn non_json_error_uses_generic_fallback() {
        let e = anyhow::anyhow!("totally unexpected failure");
        assert_eq!(
            friendly_error("en", &e),
            mezon_i18n::t("en", "common.login.genericError")
        );
    }

    #[test]
    fn extract_prefers_message_field() {
        assert_eq!(
            extract_backend_message("noise {\"message\":\"hi there\"} trailing").as_deref(),
            Some("hi there")
        );
    }

    #[test]
    fn clamp_truncates_long_text() {
        let long = "a".repeat(500);
        let out = clamp_text(&long);
        assert!(out.chars().count() <= MAX_ERROR_LEN + 1);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn mask_email_hides_local_part() {
        assert_eq!(mask_email("abcdef@mezon.ai"), "abc***@mezon.ai");
    }

    #[test]
    fn valid_emails_pass() {
        for e in ["a@b.co", "user.name@mezon.ai", " user@host.com "] {
            assert!(is_valid_email(e), "{e} should be valid");
        }
    }

    #[test]
    fn invalid_emails_fail() {
        for e in [
            "",
            "user",
            "user@",
            "@host.com",
            "user@host",
            "user @host.com",
            "a@b@c.com",
            "user@.com",
            "user@host.",
        ] {
            assert!(!is_valid_email(e), "{e} should be invalid");
        }
    }

    #[test]
    fn email_error_key_maps_cases() {
        assert_eq!(
            email_error_key("  "),
            Some("common.login.enterEmailToLogin")
        );
        assert_eq!(email_error_key("nope"), Some("common.login.invalidEmail"));
        assert_eq!(email_error_key("ok@mezon.ai"), None);
    }

    #[test]
    fn detects_rate_limit() {
        let e = anyhow::anyhow!(
            "HTTP 429 on POST https://x: {{\"message\":\"Email OTP requested too frequently, please try again later\"}}"
        );
        assert!(is_rate_limited(&e));
    }

    #[test]
    fn non_rate_limit_not_detected() {
        let bad = anyhow::anyhow!("HTTP 400 on POST https://x: {{\"message\":\"bad request\"}}");
        assert!(!is_rate_limited(&bad));
        let net = anyhow::anyhow!("boom").context("Network error on POST https://x");
        assert!(!is_rate_limited(&net));
    }
}
