use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Entity, FontWeight, PathPromptOptions, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use gpui_component::{
    Sizable, Size,
    avatar::Avatar,
    button::{Button as GpuiButton, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    v_flex,
};
use mezon_client::AppApi;

use crate::theme::Theme;
use crate::util::{check_connection, retry};

struct ProfileState {
    username: SharedString,
    display_name: SharedString,
    about_me: SharedString,
    avatar_url: Option<SharedString>,
    original_display_name: SharedString,
    original_about_me: SharedString,
    original_avatar_url: Option<SharedString>,
    loading: bool,
    saving: bool,
}

pub struct ProfilePage {
    api: Arc<AppApi>,
    profile: Option<ProfileState>,
    display_name_input: Option<Entity<InputState>>,
    about_me_input: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
    connection_error: bool,
    fetch_error: bool,
    _fetch_task: Option<Task<()>>,
    toast_message: Option<SharedString>,
    show_delete_confirm: bool,
}

impl ProfilePage {
    pub fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let api_clone = api.clone();
        let fetch_task = cx.spawn(async move |this, cx| {
            if check_connection(cx.background_executor(), &api_clone)
                .await
                .is_err()
            {
                this.update(cx, |this, cx| {
                    this.connection_error = true;
                    cx.notify();
                })
                .ok();
                return;
            }

            match retry(
                cx.background_executor(),
                5,
                Duration::from_millis(1000),
                || {
                    let api = api_clone.clone();
                    async move {
                        api.get_account().await.map_err(|e| {
                            tracing::error!("Failed to fetch account for profile, retrying: {}", e);
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
                        let about = acct.about_me.unwrap_or_default();
                        let avatar = acct.avatar_url;

                        this.profile = Some(ProfileState {
                            username: acct.username.into(),
                            display_name: display.clone().into(),
                            about_me: about.clone().into(),
                            avatar_url: avatar.clone().map(Into::into),
                            original_display_name: display.into(),
                            original_about_me: about.into(),
                            original_avatar_url: avatar.map(Into::into),
                            loading: false,
                            saving: false,
                        });

                        view_cx.notify();
                    })
                    .ok();
                }
                Err(_) => {
                    this.update(cx, |this, cx| {
                        this.fetch_error = true;
                        cx.notify();
                    })
                    .ok();
                }
            }
        });

        Self {
            api,
            profile: None,
            display_name_input: None,
            about_me_input: None,
            _subscriptions: Vec::new(),
            connection_error: false,
            fetch_error: false,
            _fetch_task: Some(fetch_task),
            toast_message: None,
            show_delete_confirm: false,
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

    fn init_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let display = cx.new(|cx| InputState::new(window, cx).placeholder("Display name"));
        let about = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Tell us about yourself")
        });

        if let Some(state) = &self.profile {
            display.update(cx, |input, cx| {
                input.set_value(&state.display_name, window, cx);
            });
            about.update(cx, |input, cx| {
                input.set_value(&state.about_me, window, cx);
            });
        }

        self._subscriptions.push(cx.subscribe_in(&display, window, {
            let display = display.clone();
            move |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let value = display.read(cx).value().to_string();
                    if let Some(state) = &mut this.profile {
                        state.display_name = value.into();
                    }
                    cx.notify();
                }
            }
        }));

        self._subscriptions.push(cx.subscribe_in(&about, window, {
            let about = about.clone();
            move |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let value = about.read(cx).value().to_string();
                    if let Some(state) = &mut this.profile {
                        state.about_me = value.into();
                    }
                    cx.notify();
                }
            }
        }));

        self.display_name_input = Some(display);
        self.about_me_input = Some(about);
    }

    fn is_dirty(&self) -> bool {
        if let Some(state) = &self.profile {
            state.display_name != state.original_display_name
                || state.about_me != state.original_about_me
                || state.avatar_url != state.original_avatar_url
        } else {
            false
        }
    }

    fn discard_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.profile.as_ref().map(|s| {
            (
                s.original_display_name.clone(),
                s.original_about_me.clone(),
                s.original_avatar_url.clone(),
            )
        });

        if let (Some((display_name, about_me, avatar_url)), Some(state)) =
            (original.clone(), &mut self.profile)
        {
            state.display_name = display_name;
            state.about_me = about_me;
            state.avatar_url = avatar_url;
        }

        if let Some((display_name, about_me, _)) = original {
            if let Some(input) = &self.display_name_input {
                input.update(cx, |input_state: &mut InputState, input_cx| {
                    input_state.set_value(display_name.clone(), window, input_cx);
                });
            }
            if let Some(input) = &self.about_me_input {
                input.update(cx, |input_state: &mut InputState, input_cx| {
                    input_state.set_value(about_me, window, input_cx);
                });
            }
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.profile else {
            return;
        };
        if state.saving {
            return;
        }
        state.saving = true;
        cx.notify();

        let api = self.api.clone();
        let display_name: String = state.display_name.to_string();
        let about_me: String = state.about_me.to_string();
        let avatar_url: Option<String> = state.avatar_url.as_ref().map(|s| s.to_string());

        cx.spawn(async move |this, cx| {
            if check_connection(cx.background_executor(), &api)
                .await
                .is_err()
            {
                this.update(cx, |this, cx| {
                    if let Some(state) = &mut this.profile {
                        state.saving = false;
                    }
                    this.show_toast("Connection lost. Please try again.", cx);
                    cx.notify();
                })
                .ok();
                return;
            }

            match api
                .update_account(Some(&display_name), avatar_url.as_deref(), Some(&about_me))
                .await
            {
                Ok(()) => {
                    this.update(cx, |this, cx| {
                        if let Some(state) = &mut this.profile {
                            state.original_display_name = state.display_name.clone();
                            state.original_about_me = state.about_me.clone();
                            state.original_avatar_url = state.avatar_url.clone();
                            state.saving = false;
                        }
                        this.show_toast("Profile saved", cx);
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |this, cx| {
                        if let Some(state) = &mut this.profile {
                            state.saving = false;
                        }
                        this.show_toast(format!("Failed to save: {}", e), cx);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }
}

impl Render for ProfilePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();

        if self.profile.as_ref().is_some_and(|p| !p.loading) && self.display_name_input.is_none() {
            self.init_inputs(window, cx);
        }

        if self.connection_error {
            return v_flex()
                .gap_4()
                .child(
                    Label::new("Profile")
                        .text_xl()
                        .text_color(theme.text_primary)
                        .font_weight(FontWeight::BOLD),
                )
                .child(Label::new("Connection failed").text_color(theme.text_muted))
                .into_any_element();
        }

        if self.fetch_error {
            return v_flex()
                .gap_4()
                .child(
                    Label::new("Profile")
                        .text_xl()
                        .text_color(theme.text_primary)
                        .font_weight(FontWeight::BOLD),
                )
                .child(Label::new("Failed to load profile data").text_color(theme.text_muted))
                .into_any_element();
        }

        if self.profile.is_none() || self.profile.as_ref().is_some_and(|p| p.loading) {
            return v_flex()
                .gap_4()
                .child(
                    Label::new("Profile")
                        .text_xl()
                        .text_color(theme.text_primary)
                        .font_weight(FontWeight::BOLD),
                )
                .child(Label::new("Loading profile...").text_color(theme.text_muted))
                .into_any_element();
        }

        let is_dirty = self.is_dirty();
        let entity = cx.entity().clone();

        let form = self.render_form(&theme, cx);
        let preview = self.render_preview(&theme);

        v_flex()
            .gap_6()
            .child(h_flex().gap_8().child(form).child(preview))
            // Unsaved changes warning
            .when(is_dirty, |el| {
                el.child(
                    v_flex()
                        .gap_3()
                        .pt_4()
                        .child(
                            h_flex().gap_2().items_center().child(
                                div()
                                    .text_sm()
                                    .text_color(theme.status_dnd)
                                    .child("⚠ Careful — you have unsaved changes!"),
                            ),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    GpuiButton::new("save-profile-btn")
                                        .label("Save Changes")
                                        .text_color(theme.text_secondary)
                                        .on_click({
                                            let e = entity.clone();
                                            move |_, _, cx| {
                                                e.update(cx, |this, view_cx| {
                                                    this.save(view_cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    GpuiButton::new("discard-profile-btn")
                                        .label("Discard")
                                        .text_color(theme.text_primary)
                                        .ghost()
                                        .on_click({
                                            let e = entity.clone();
                                            move |_, window, cx| {
                                                e.update(cx, |this, view_cx| {
                                                    this.discard_changes(window, view_cx);
                                                    view_cx.notify();
                                                });
                                            }
                                        }),
                                ),
                        ),
                )
            })
            // Delete Account button
            .child(
                GpuiButton::new("delete-account-btn")
                    .label("Delete Account")
                    .text_color(theme.status_dnd)
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_toast("Delete account confirmation coming soon", cx);
                    })),
            )
            // Delete confirmation
            .when(self.show_delete_confirm, |el| {
                el.child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .rounded_lg()
                        .bg(theme.bg_floating)
                        .child(
                            Label::new("Are you sure? This cannot be undone.")
                                .text_color(theme.text_primary),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    GpuiButton::new("confirm-delete-btn")
                                        .label("Delete Account")
                                        .text_color(theme.status_dnd),
                                )
                                .child(
                                    GpuiButton::new("cancel-delete-btn")
                                        .label("Cancel")
                                        .text_color(theme.text_primary)
                                        .ghost(),
                                ),
                        ),
                )
            })
            .when_some(self.toast_message.clone(), |this, msg| {
                this.child(div().text_sm().text_color(theme.text_muted).child(msg))
            })
            .into_any_element()
    }
}

impl ProfilePage {
    fn render_form(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let display_name: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.display_name.clone());
        let avatar_url = self.profile.as_ref().and_then(|p| p.avatar_url.clone());
        let about_me: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.about_me.clone());

        v_flex()
            .gap_4()
            .child(
                Label::new("Profile")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::BOLD),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Avatar::new()
                            .when_some(avatar_url.clone(), |av, url| av.src(url))
                            .name(display_name.clone())
                            .with_size(Size::Large),
                    )
                    .child(
                        GpuiButton::new("change-avatar-btn")
                            .label("Change Avatar")
                            .text_color(theme.text_primary)
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let api = this.api.clone();
                                let root_entity = cx.entity().clone();
                                let rx = cx.prompt_for_paths(PathPromptOptions {
                                    files: true,
                                    directories: false,
                                    multiple: false,
                                    prompt: Some("Choose an avatar image".into()),
                                });
                                cx.spawn(async move |_this, cx| {
                                    let paths = match rx.await {
                                        Ok(Ok(Some(p))) => p,
                                        _ => return,
                                    };
                                    let path = match paths.into_iter().next() {
                                        Some(p) => p,
                                        None => return,
                                    };

                                    if check_connection(cx.background_executor(), &api)
                                        .await
                                        .is_err()
                                    {
                                        root_entity.update(cx, |this, cx| {
                                            this.show_toast(
                                                "Connection lost. Please try again.",
                                                cx,
                                            );
                                        });
                                        return;
                                    }

                                    match api.upload_avatar(&path).await {
                                        Ok(url) => {
                                            root_entity.update(cx, |this, cx| {
                                                if let Some(state) = &mut this.profile {
                                                    state.avatar_url = Some(url.into());
                                                }
                                                cx.notify();
                                            });
                                        }
                                        Err(e) => {
                                            root_entity.update(cx, |this, cx| {
                                                this.show_toast(
                                                    format!("Failed to upload avatar: {}", e),
                                                    cx,
                                                );
                                            });
                                        }
                                    }
                                })
                                .detach();
                            })),
                    )
                    .child(
                        GpuiButton::new("remove-avatar-btn")
                            .label("Remove Avatar")
                            .text_color(theme.text_muted)
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(state) = &mut this.profile {
                                    state.avatar_url = None;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child("DISPLAY NAME"),
                    )
                    .child(
                        Input::new(
                            self.display_name_input
                                .as_ref()
                                .expect("display_name_input not initialized"),
                        )
                        .w_full(),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child("ABOUT ME"),
                    )
                    .child(
                        Input::new(
                            self.about_me_input
                                .as_ref()
                                .expect("about_me_input not initialized"),
                        )
                        .w_full()
                        .h(px(100.0)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(format!("{}/128", about_me.len())),
                    ),
            )
    }

    fn render_preview(&self, theme: &Theme) -> impl IntoElement {
        let display_name: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.display_name.clone());
        let about_me: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.about_me.clone());
        let username: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.username.clone());
        let avatar_url = self.profile.as_ref().and_then(|p| p.avatar_url.clone());

        v_flex()
            .gap_4()
            .child(
                Label::new("Preview")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::BOLD),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    // Color banner
                    .child(div().h(px(105.0)).w_full().bg(theme.brand))
                    .child(
                        v_flex()
                            .gap_4()
                            .px_6()
                            .py_6()
                            .child(
                                h_flex()
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
                                                Label::new(format!("@{}", username))
                                                    .text_sm()
                                                    .text_color(theme.text_muted),
                                            ),
                                    ),
                            )
                            .when(!about_me.is_empty(), |el| {
                                el.child(
                                    Label::new(about_me.clone())
                                        .text_sm()
                                        .text_color(theme.text_muted),
                                )
                            }),
                    ),
            )
    }
}
