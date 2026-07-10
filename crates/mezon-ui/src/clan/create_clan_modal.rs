use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, PathPromptOptions, SharedString,
    Subscription, Task, Window, div, img, prelude::*, px, rgb,
};
use mezon_store::{ClanImageMimeType, ClanList, CreateClanError, MAX_CLAN_LOGO_BYTES, Settings};

use crate::app::shell::Shell;
use crate::clan::templates::{TEMPLATES, TemplateId};
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::image_cache::LruImageCache;
use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModalStep {
    TemplateSelection,
    ClanDetails,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Validation {
    InvalidName,
    DuplicateName,
    Validated,
}

pub struct CreateClanModal {
    focus_handle: FocusHandle,
    clan_list: Entity<ClanList>,
    settings: Entity<Settings>,
    name_input: Entity<InputState>,
    step: ModalStep,
    selected_template: Option<TemplateId>,
    logo_url: Option<SharedString>,
    validation: Validation,
    creating: bool,
    image_cache: Entity<LruImageCache>,
    _name_sub: Subscription,
    _logo_task: Option<Task<()>>,
    _create_task: Option<Task<()>>,
}

fn is_valid_clan_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == ' '
}

fn is_valid_clan_name(s: &str) -> bool {
    if s.is_empty() || s.chars().count() > 64 {
        return false;
    }
    let first = s.chars().next().unwrap();
    if first == '_' || first == '-' || first == ' ' {
        return false;
    }
    s.chars().all(|c| is_valid_clan_name_char(c) && c != '\'')
}

impl Focusable for CreateClanModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateClanModal {
    pub fn new(
        clan_list: Entity<ClanList>,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let locale = settings.read(cx).language.clone();
        let placeholder: SharedString = mezon_i18n::t(&locale, "clan.createClanModal.placeholder")
            .to_string()
            .into();
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let name_sub = cx.subscribe(&name_input, |this, _, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.revalidate(cx);
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            clan_list,
            settings,
            name_input,
            step: ModalStep::TemplateSelection,
            selected_template: None,
            logo_url: None,
            validation: Validation::InvalidName,
            creating: false,
            image_cache: cx.new(|cx| {
                LruImageCache::avatar_thumbnail(
                    "clan-logo-preview",
                    2,
                    4 * 1024 * 1024,
                    4 * 1024 * 1024,
                    cx,
                )
            }),
            _name_sub: name_sub,
            _logo_task: None,
            _create_task: None,
        }
    }

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        let value = self.name_input.read(cx).value();
        let value = value.trim();
        self.validation = if is_valid_clan_name(value) {
            Validation::Validated
        } else {
            Validation::InvalidName
        };
        cx.notify();
    }

    fn go_to_details(&mut self, template: Option<TemplateId>, cx: &mut Context<Self>) {
        self.selected_template = template;
        self.step = ModalStep::ClanDetails;
        cx.notify();
    }

    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = ModalStep::TemplateSelection;
        self.selected_template = None;
        self.logo_url = None;
        self.validation = Validation::InvalidName;
        self.name_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn pick_logo(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let prompt: SharedString = mezon_i18n::t(&locale, "setting.profile.chooseAvatar")
            .to_string()
            .into();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt),
        });
        self._logo_task.take();
        self._logo_task = Some(cx.spawn(async move |this, cx| {
            let finish = |this: &mut CreateClanModal| {
                this._logo_task = None;
            };

            let paths = match rx.await {
                Ok(Ok(Some(p))) => p,
                _ => {
                    let _ = this.update(cx, |this, _| finish(this));
                    return;
                }
            };
            let path = match paths.into_iter().next() {
                Some(p) => p,
                None => {
                    let _ = this.update(cx, |this, _| finish(this));
                    return;
                }
            };

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !ClanImageMimeType::is_allowed_extension(&ext) {
                tracing::warn!("logo pick rejected: unsupported file type .{ext}");
                let _ = this.update(cx, |this, _| finish(this));
                return;
            }

            let path_buf = path.clone();
            let file_size = match cx
                .background_spawn(async move { std::fs::metadata(&path_buf).ok().map(|m| m.len()) })
                .await
            {
                Some(size) => size,
                None => {
                    tracing::warn!("logo pick: cannot read file metadata");
                    let _ = this.update(cx, |this, _| finish(this));
                    return;
                }
            };
            if file_size > MAX_CLAN_LOGO_BYTES {
                tracing::warn!(
                    "logo pick rejected: file size {} > {} bytes",
                    file_size,
                    MAX_CLAN_LOGO_BYTES
                );
                let _ = this.update(cx, |this, _| finish(this));
                return;
            }

            let task = match this.update(cx, |modal, cx| {
                modal.clan_list.update(cx, |store, cx| {
                    store.upload_clan_image(&path, MAX_CLAN_LOGO_BYTES, cx)
                })
            }) {
                Ok(t) => t,
                Err(_) => {
                    let _ = this.update(cx, |this, _| finish(this));
                    return;
                }
            };
            match task.await {
                Ok(url) => {
                    let _ = this.update(cx, |modal, cx| {
                        modal.logo_url = Some(SharedString::from(url));
                        finish(modal);
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("logo upload failed: {e}");
                    let _ = this.update(cx, |this, _| finish(this));
                }
            }
        }));
    }

    fn create_clan(&mut self, cx: &mut Context<Self>) {
        if self.validation != Validation::Validated || self.creating {
            return;
        }
        let name = self.name_input.read(cx).value().trim().to_string();
        let logo = self.logo_url.as_deref().unwrap_or_default().to_string();
        self.creating = true;
        cx.notify();

        let task = self
            .clan_list
            .update(cx, |store, cx| store.create_clan(name, logo, cx));
        self._create_task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok(_clan_id) => {
                cx.update(|cx| {
                    crate::router::navigate(cx, crate::router::Route::Chat);
                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                });
            }
            Err(CreateClanError::DuplicateName) => {
                let _ = this.update(cx, |this, cx| {
                    this.validation = Validation::DuplicateName;
                    this.creating = false;
                    cx.notify();
                });
            }
            Err(CreateClanError::Other(msg)) => {
                tracing::error!("create clan failed: {msg}");
                let _ = this.update(cx, |this, cx| {
                    this.creating = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.error(msg, cx);
                    });
                });
            }
        }));
    }

    fn render_template_step(&self, locale: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let title: SharedString = mezon_i18n::t(locale, "clan.clanTemplateModal.title")
            .to_string()
            .into();
        let description: SharedString = mezon_i18n::t(locale, "clan.clanTemplateModal.description")
            .to_string()
            .into();
        let create_my_own: SharedString =
            mezon_i18n::t(locale, "clan.clanTemplateModal.createMyOwn")
                .to_string()
                .into();
        let start_from: SharedString =
            mezon_i18n::t(locale, "clan.clanTemplateModal.startFromTemplate")
                .to_string()
                .into();

        let own_btn = div()
            .id("template-create-own")
            .w_full()
            .p_3()
            .mb_4()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .hover(|s| s.border_color(theme.brand))
            .on_click(cx.listener(|this, _, _window, cx| this.go_to_details(None, cx)))
            .child(
                Icon::new(IconName::Sparkles)
                    .size(px(20.))
                    .text_color(theme.tokens.text_theme_primary),
            )
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(create_my_own),
            );

        let template_label = div()
            .text_sm()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_3()
            .text_color(theme.text_muted)
            .child(start_from);

        let template_buttons: Vec<AnyElement> = TEMPLATES
            .iter()
            .map(|entry| {
                let label: SharedString = mezon_i18n::t(locale, entry.key).to_string().into();
                let template_id = entry.id;
                let icon = entry.icon;
                div()
                    .id(SharedString::from(format!("template-{}", entry.key)))
                    .w_full()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .mb_2()
                    .hover(|s| s.border_color(theme.brand))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.go_to_details(Some(template_id), cx)
                    }))
                    .child(
                        Icon::new(icon)
                            .size(px(20.))
                            .text_color(theme.tokens.text_theme_primary),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_sm()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(label),
                    )
                    .into_any_element()
            })
            .collect();

        v_flex()
            .px(px(20.))
            .py(px(16.))
            .w(px(480.))
            .items_center()
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .pb_2()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(16.))
                    .text_align(gpui::TextAlign::Center)
                    .mb(px(24.))
                    .text_color(theme.text_secondary)
                    .child(description),
            )
            .child(
                div()
                    .id("template-scroll")
                    .w_full()
                    .px_3()
                    .py_2()
                    .overflow_y_scroll()
                    .max_h(px(300.))
                    .child(own_btn)
                    .child(template_label)
                    .children(template_buttons),
            )
            .into_any_element()
    }

    fn render_details_step(&self, locale: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let title: SharedString = mezon_i18n::t(locale, "clan.createClanModal.title")
            .to_string()
            .into();
        let description: SharedString = mezon_i18n::t(locale, "clan.createClanModal.description")
            .to_string()
            .into();
        let upload_label: SharedString = mezon_i18n::t(locale, "clan.createClanModal.upload")
            .to_string()
            .into();
        let clan_name_label: SharedString = mezon_i18n::t(locale, "clan.createClanModal.clanName")
            .to_string()
            .into();
        let agreement: SharedString = mezon_i18n::t(locale, "clan.createClanModal.agreement")
            .to_string()
            .into();
        let guidelines: SharedString =
            mezon_i18n::t(locale, "clan.createClanModal.communityGuidelines")
                .to_string()
                .into();

        let logo_el: AnyElement = if let Some(ref url) = self.logo_url {
            img(url.clone())
                .w(px(81.))
                .h(px(81.))
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover)
                .into_any_element()
        } else {
            div()
                .id("logo-upload-placeholder")
                .w(px(81.))
                .h(px(81.))
                .rounded_full()
                .border_1()
                .border_color(theme.text_muted)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .relative()
                .on_click(cx.listener(|this, _, window, cx| this.pick_logo(window, cx)))
                .child(
                    div()
                        .absolute()
                        .top(px(-3.))
                        .right(px(0.))
                        .child(Icon::new(IconName::AddIcon).size(px(16.))),
                )
                .child(
                    Icon::new(IconName::UploadImage)
                        .size(px(24.))
                        .text_color(theme.text_secondary),
                )
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme.text_secondary)
                        .child(upload_label),
                )
                .into_any_element()
        };

        let show_invalid_name = self.validation == Validation::InvalidName;
        let show_duplicate = self.validation == Validation::DuplicateName;
        let invalid_name_msg: SharedString =
            mezon_i18n::t(locale, "clan.createClanModal.invalidName")
                .to_string()
                .into();
        let duplicate_msg: SharedString =
            mezon_i18n::t(locale, "clan.createClanModal.duplicateName")
                .to_string()
                .into();

        let name_input_entity = self.name_input.clone();

        v_flex()
            .px(px(20.))
            .py(px(16.))
            .w(px(480.))
            .items_center()
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .pb(px(16.))
                    .text_color(theme.tokens.text_theme_primary)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(16.))
                    .text_align(gpui::TextAlign::Center)
                    .text_color(theme.text_secondary)
                    .child(description),
            )
            .child(div().mt(px(32.)).mb(px(16.)).child(logo_el))
            .child(
                div()
                    .w_full()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_size(px(16.))
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(clan_name_label),
                            )
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .text_color(rgb(0xe44141))
                                    .child("*"),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(16.))
                            .mb_2()
                            .child(Input::new(&name_input_entity)),
                    )
                    .when(show_invalid_name || show_duplicate, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xe44141))
                                .italic()
                                .when(show_invalid_name, |e| e.child(invalid_name_msg))
                                .when(show_duplicate, |e| e.child(duplicate_msg)),
                        )
                    })
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(14.))
                            .text_color(theme.text_secondary)
                            .flex()
                            .flex_row()
                            .gap_1()
                            .child(div().child(agreement))
                            .child(div().text_color(theme.brand).child(guidelines))
                            .child(div().child(".")),
                    ),
            )
            .into_any_element()
    }
}

impl Render for CreateClanModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let step = self.step;
        let validation = self.validation;
        let creating = self.creating;
        let border = cx.theme().border;
        let brand = cx.theme().brand;
        let text_primary = cx.theme().text_primary;
        let theme_setting_primary = cx.theme().tokens.theme_setting_primary;

        let back_label: SharedString = mezon_i18n::t(&locale, "clan.createClanModal.back")
            .to_string()
            .into();
        let create_label: SharedString = mezon_i18n::t(&locale, "clan.createClanModal.create")
            .to_string()
            .into();

        let content = match step {
            ModalStep::TemplateSelection => self.render_template_step(&locale, cx),
            ModalStep::ClanDetails => self.render_details_step(&locale, cx),
        };

        let footer: AnyElement = match step {
            ModalStep::TemplateSelection => h_flex()
                .w_full()
                .justify_start()
                .px(px(20.))
                .py(px(16.))
                .border_t_1()
                .border_color(border)
                .child(
                    Button::new("back-close")
                        .label(back_label)
                        .ghost()
                        .text_color(gpui::Hsla::from(brand))
                        .on_click(|_, _, cx| {
                            Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                        }),
                )
                .into_any_element(),
            ModalStep::ClanDetails => h_flex()
                .w_full()
                .justify_between()
                .px(px(20.))
                .py(px(16.))
                .border_t_1()
                .border_color(border)
                .child(
                    Button::new("back-to-templates")
                        .label(back_label)
                        .ghost()
                        .text_color(gpui::Hsla::from(brand))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.go_back(window, cx);
                        })),
                )
                .child(
                    Button::new("create-clan")
                        .label(create_label)
                        .primary()
                        .disabled(validation != Validation::Validated || creating)
                        .loading(creating)
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.create_clan(cx);
                        })),
                )
                .into_any_element(),
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .image_cache(self.image_cache.clone())
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .min_w(px(320.))
            .max_w(px(480.))
            .rounded(px(12.))
            .bg(theme_setting_primary)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .p(px(16.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .id("modal-close")
                            .w(px(24.))
                            .h(px(24.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .opacity(0.5)
                            .hover(|s| s.opacity(1.0))
                            .text_size(px(18.))
                            .text_color(text_primary)
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            })
                            .child("×"),
                    ),
            )
            .child(content)
            .child(footer)
    }
}
