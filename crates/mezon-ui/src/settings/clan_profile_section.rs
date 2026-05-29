use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Entity, FontWeight, PathPromptOptions, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use gpui_component::{
    Disableable as _, Sizable, Size,
    avatar::Avatar,
    button::{Button as GpuiButton, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    v_flex,
};
use mezon_client::AppApi;
use mezon_store::{ClanList, Settings};

use crate::theme::{Theme, resolve_theme};

struct ClanProfileState {
    selected_clan_id: SharedString,
    nick_name: SharedString,
    avatar_url: Option<SharedString>,
    original_nick_name: SharedString,
    original_avatar_url: Option<SharedString>,
    loading: bool,
    saving: bool,
    duplicate_error: bool,
    #[allow(dead_code)]
    fetched: bool,
}

pub struct ClanProfileSection {
    api: Arc<AppApi>,
    settings: Entity<Settings>,
    clan_list: Entity<ClanList>,
    profile: Option<ClanProfileState>,
    display_name: SharedString,
    username: SharedString,
    nick_name_input: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
    _fetch_task: Option<Task<()>>,
    _debounce_task: Option<Task<()>>,
    toast_message: Option<SharedString>,
}

impl ClanProfileSection {
    pub fn new(
        api: Arc<AppApi>,
        settings: Entity<Settings>,
        clan_list: Entity<ClanList>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());
        let _ = cx.observe(&clan_list, |_, _, cx| cx.notify());
        Self {
            api,
            settings,
            clan_list,
            profile: None,
            display_name: SharedString::default(),
            username: SharedString::default(),
            nick_name_input: None,
            _subscriptions: Vec::new(),
            _fetch_task: None,
            _debounce_task: None,
            toast_message: None,
        }
    }

    pub fn set_user_profile(&mut self, display_name: SharedString, username: SharedString) {
        self.display_name = display_name;
        self.username = username;
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
        let nick = cx.new(|cx| InputState::new(window, cx).placeholder("Clan nickname"));

        if let Some(state) = &self.profile {
            nick.update(cx, |input, cx| {
                input.set_value(&state.nick_name, window, cx);
            });
        }

        let api = self.api.clone();
        self._subscriptions.push(cx.subscribe_in(&nick, window, {
            let nick = nick.clone();
            move |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let value = nick.read(cx).value().to_string();
                    if let Some(state) = &mut this.profile
                        && !state.saving
                    {
                        state.nick_name = value.clone().into();
                        state.duplicate_error = false;
                    }
                    cx.notify();

                    let value = value.trim().to_string();
                    if value.len() >= 2 {
                        let clan_id = this
                            .profile
                            .as_ref()
                            .map_or("".to_string(), |s| s.selected_clan_id.to_string());
                        let api = api.clone();
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(600))
                                .await;
                            let is_dup = api
                                .check_duplicate_clan_nickname(&clan_id, &value)
                                .await
                                .unwrap_or(false);
                            let _ = this.update(cx, |this, cx| {
                                if let Some(state) = &mut this.profile {
                                    state.duplicate_error = is_dup;
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                }
            }
        }));

        self._subscriptions.push(cx.observe(&nick, {
            move |this: &mut Self, input: Entity<InputState>, cx| {
                let value = input.read(cx).value().to_string();
                if let Some(state) = &mut this.profile
                    && !state.saving
                    && state.nick_name.as_ref() != value
                {
                    state.nick_name = value.into();
                    cx.notify();
                }
            }
        }));

        self.nick_name_input = Some(nick);
    }

    fn is_dirty(&self) -> bool {
        if let Some(state) = &self.profile {
            state.nick_name != state.original_nick_name
                || state.avatar_url != state.original_avatar_url
        } else {
            false
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.profile else {
            return;
        };
        if state.saving || state.duplicate_error {
            return;
        }
        state.saving = true;
        cx.notify();

        let api = self.api.clone();
        let clan_id: String = state.selected_clan_id.to_string();
        let nick_name: String = state.nick_name.to_string();
        let avatar_url: Option<String> = state.avatar_url.as_ref().map(|s| s.to_string());

        cx.spawn(async move |this, cx| {
            match api
                .update_user_clan_profile(&clan_id, &nick_name, avatar_url.as_deref())
                .await
            {
                Ok(()) => {
                    this.update(cx, |this, cx| {
                        if let Some(state) = &mut this.profile {
                            state.original_nick_name = state.nick_name.clone();
                            state.original_avatar_url = state.avatar_url.clone();
                            state.saving = false;
                        }
                        this.show_toast("Clan profile saved", cx);
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |this, cx| {
                        if let Some(state) = &mut this.profile {
                            state.saving = false;
                        }
                        this.show_toast(format!("Failed to save clan profile: {}", e), cx);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.profile {
            state.nick_name = state.original_nick_name.clone();
            state.avatar_url = state.original_avatar_url.clone();
            state.duplicate_error = false;
        }
        if let (Some(input), Some(state)) = (&self.nick_name_input, &self.profile) {
            input.update(cx, |input_state, cx| {
                input_state.set_value(state.nick_name.clone(), window, cx);
            });
        }
    }

    pub fn fetch(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let clan_id = clan_id.to_string();
        let entity = cx.entity().clone();
        self.profile = Some(ClanProfileState {
            selected_clan_id: clan_id.clone().into(),
            nick_name: "".into(),
            avatar_url: None,
            original_nick_name: "".into(),
            original_avatar_url: None,
            loading: true,
            saving: false,
            duplicate_error: false,
            fetched: false,
        });
        cx.notify();

        self._fetch_task =
            Some(cx.spawn(
                async move |_, cx| match api.get_user_clan_profile(&clan_id).await {
                    Ok(profile) => {
                        entity.update(cx, |this, cx| {
                            let nick: SharedString = profile.nick_name.clone().into();
                            let avatar: Option<SharedString> = Some(profile.avatar.clone())
                                .filter(|s| !s.is_empty())
                                .map(Into::into);
                            this.profile = Some(ClanProfileState {
                                selected_clan_id: clan_id.clone().into(),
                                nick_name: nick.clone(),
                                avatar_url: avatar.clone(),
                                original_nick_name: nick,
                                original_avatar_url: avatar,
                                loading: false,
                                saving: false,
                                duplicate_error: false,
                                fetched: true,
                            });
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        entity.update(cx, |this, cx| {
                            this.profile = Some(ClanProfileState {
                                selected_clan_id: clan_id.clone().into(),
                                nick_name: "".into(),
                                avatar_url: None,
                                original_nick_name: "".into(),
                                original_avatar_url: None,
                                loading: false,
                                saving: false,
                                duplicate_error: false,
                                fetched: true,
                            });
                            this.show_toast(format!("Failed to load clan profile: {}", e), cx);
                            cx.notify();
                        });
                    }
                },
            ));
    }
}

impl Render for ClanProfileSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(&self.settings.read(cx).theme);

        if self.profile.as_ref().is_some_and(|p| !p.loading) && self.nick_name_input.is_none() {
            self.init_inputs(window, cx);
        }

        let is_dirty = self.is_dirty();
        let entity = cx.entity().clone();
        let loading = self.profile.as_ref().is_some_and(|p| p.loading);
        let saving = self.profile.as_ref().map(|p| p.saving).unwrap_or(false);

        let clans = self.clan_list.read(cx);
        let clan_options: Vec<(SharedString, SharedString)> = clans
            .clans
            .iter()
            .map(|c| (c.id.clone().into(), c.name.clone().into()))
            .collect();

        let selected_clan_id: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |s| s.selected_clan_id.clone());

        let nick_name: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |s| s.nick_name.clone());

        let avatar_url = self.profile.as_ref().and_then(|s| s.avatar_url.clone());

        let duplicate_error = self.profile.as_ref().is_some_and(|s| s.duplicate_error);

        let form = self.render_clan_form(
            &theme,
            &clan_options,
            &selected_clan_id,
            &nick_name,
            avatar_url.clone(),
            loading,
            duplicate_error,
            cx,
        );
        let preview = Self::render_clan_preview(
            &theme,
            &nick_name,
            avatar_url,
            &self.display_name,
            &self.username,
        );

        v_flex()
            .gap_6()
            .child(h_flex().gap_8().child(form).child(preview))
            .when(is_dirty || saving, |el| {
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
                                    GpuiButton::new("clan-save-btn")
                                        .label(if saving { "Saving…" } else { "Save Changes" })
                                        .disabled(saving)
                                        .text_color(theme.text_secondary)
                                        .on_click({
                                            let e = entity.clone();
                                            move |_, _, cx| {
                                                e.update(cx, |this, cx| {
                                                    this.save(cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    GpuiButton::new("clan-discard-btn")
                                        .label("Discard")
                                        .disabled(saving)
                                        .text_color(theme.text_primary)
                                        .ghost()
                                        .on_click({
                                            let e = entity.clone();
                                            move |_, window, cx| {
                                                e.update(cx, |this, cx| {
                                                    this.discard(window, cx);
                                                });
                                            }
                                        }),
                                ),
                        ),
                )
            })
            .when_some(self.toast_message.clone(), |this, msg| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .bg(theme.bg_floating)
                        .rounded_md()
                        .text_sm()
                        .text_color(theme.text_primary)
                        .child(msg),
                )
            })
            .into_any_element()
    }
}

impl ClanProfileSection {
    #[allow(clippy::too_many_arguments)]
    fn render_clan_form(
        &self,
        theme: &Theme,
        clan_options: &[(SharedString, SharedString)],
        selected_clan_id: &SharedString,
        nick_name: &SharedString,
        avatar_url: Option<SharedString>,
        loading: bool,
        duplicate_error: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap_4()
            .child(
                v_flex().gap_1().child(
                    div()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child("Customize how you appear in each clan."),
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
                            .child("CHOOSE A CLAN"),
                    )
                    .child({
                        let entity = cx.entity().clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(clan_options.iter().map(move |(id, name)| {
                                let is_selected = *id == *selected_clan_id;
                                let click_id = id.clone();
                                let e = entity.clone();
                                div()
                                    .id(SharedString::from(format!("clan-opt-{}", id)))
                                    .flex()
                                    .items_center()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .when(is_selected, |el| {
                                        el.bg(gpui::Rgba {
                                            r: 233.0 / 255.0,
                                            g: 233.0 / 255.0,
                                            b: 233.0 / 255.0,
                                            a: 0.08,
                                        })
                                    })
                                    .hover(|el| {
                                        el.bg(gpui::Rgba {
                                            r: 1.0,
                                            g: 1.0,
                                            b: 1.0,
                                            a: 0.05,
                                        })
                                    })
                                    .child(name.clone())
                                    .child(div().flex_1())
                                    .when(is_selected, |el| {
                                        el.child(Label::new("✓").text_color(theme.brand).text_sm())
                                    })
                                    .on_click({
                                        let e = e.clone();
                                        let click_id = click_id.clone();
                                        move |_, _, cx| {
                                            e.update(cx, |this, cx| {
                                                this.fetch(&click_id, cx);
                                            });
                                        }
                                    })
                            }))
                    }),
            )
            .when(loading, |el| {
                el.child(
                    Label::new("Loading clan profile...")
                        .text_color(theme.text_muted)
                        .text_sm(),
                )
            })
            .when(!loading, |el| {
                el.child(
                    v_flex()
                        .gap_4()
                        .child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    Avatar::new()
                                        .when_some(avatar_url.clone(), |av, url| av.src(url))
                                        .name(nick_name.clone())
                                        .with_size(Size::Large),
                                )
                                .child(
                                    GpuiButton::new("clan-change-avatar-btn")
                                        .label("Change Avatar")
                                        .text_color(theme.text_primary)
                                        .ghost()
                                        .on_click({
                                            let api = self.api.clone();
                                            let entity = cx.entity().clone();
                                            move |_, _, cx| {
                                                let api = api.clone();
                                                let entity = entity.clone();
                                                let rx = cx.prompt_for_paths(PathPromptOptions {
                                                    files: true,
                                                    directories: false,
                                                    multiple: false,
                                                    prompt: Some("Choose an avatar image".into()),
                                                });
                                                cx.spawn(async move |cx| {
                                                    let paths = match rx.await {
                                                        Ok(Ok(Some(p))) => p,
                                                        _ => return,
                                                    };
                                                    let path = match paths.into_iter().next() {
                                                        Some(p) => p,
                                                        None => return,
                                                    };
                                                    match api.upload_avatar(&path).await {
                                                        Ok(url) => {
                                                            entity.update(cx, |this, cx| {
                                                                if let Some(state) =
                                                                    &mut this.profile
                                                                {
                                                                    state.avatar_url =
                                                                        Some(url.into());
                                                                }
                                                                cx.notify();
                                                            });
                                                        }
                                                        Err(e) => {
                                                            entity.update(cx, |this, cx| {
                                                                this.show_toast(format!(
                                                                    "Failed to upload avatar: {}",
                                                                    e,
                                                                ), cx);
                                                            });
                                                        }
                                                    }
                                                })
                                                .detach();
                                            }
                                        }),
                                )
                                .child(
                                    GpuiButton::new("clan-remove-avatar-btn")
                                        .label("Remove Avatar")
                                        .text_color(theme.text_muted)
                                        .ghost()
                                        .on_click({
                                            let entity = cx.entity().clone();
                                            move |_, _, cx| {
                                                entity.clone().update(cx, |this, cx| {
                                                    if let Some(state) = &mut this.profile {
                                                        state.avatar_url = None;
                                                    }
                                                    cx.notify();
                                                });
                                            }
                                        }),
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
                                        .child("CLAN NICKNAME"),
                                )
                                .child(Input::new(
                                    self.nick_name_input
                                        .as_ref()
                                        .expect("nick_name_input not initialized"),
                                ))
                                .when(duplicate_error, |el| {
                                    el.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.status_dnd)
                                            .child("Nickname already exists"),
                                    )
                                }),
                        ),
                )
            })
    }

    fn render_clan_preview(
        theme: &Theme,
        nick_name: &SharedString,
        avatar_url: Option<SharedString>,
        display_name: &SharedString,
        username: &SharedString,
    ) -> impl IntoElement {
        let display_label = if nick_name.is_empty() {
            display_name.clone()
        } else {
            nick_name.clone()
        };

        v_flex()
            .gap_4()
            .child(
                Label::new("Preview")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    .child(div().h(px(105.0)).w_full().bg(theme.brand))
                    .child(
                        v_flex().gap_4().px_6().py_6().child(
                            h_flex()
                                .gap_4()
                                .child(
                                    Avatar::new()
                                        .when_some(avatar_url, |av, url| av.src(url))
                                        .name(nick_name.clone())
                                        .with_size(Size::Large),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            Label::new(display_label)
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
                        ),
                    ),
            )
    }
}
