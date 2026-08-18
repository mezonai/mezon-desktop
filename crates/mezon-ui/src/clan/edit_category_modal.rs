use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::theme::ActiveTheme;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString, Subscription, Task,
    Window, div, prelude::*, px,
};
use mezon_store::{ChannelList, ClanId, CreateCategoryError, validate_category_name};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Validation {
    InvalidName,
    DuplicateName,
    Validated,
}

pub struct EditCategoryModal {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    category_id: String,
    channel_list: Entity<ChannelList>,
    locale: String,
    initial_name: String,
    name_input: Entity<InputState>,
    validation: Validation,
    validation_visible: bool,
    saving: bool,
    _input_sub: Subscription,
    _save_task: Option<Task<()>>,
}

impl Focusable for EditCategoryModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EditCategoryModal {
    pub fn new(
        clan_id: ClanId,
        category_id: String,
        channel_list: Entity<ChannelList>,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_name = channel_list
            .read(cx)
            .category_name(clan_id, &category_id)
            .unwrap_or_default()
            .to_string();
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "clan.categoryOverview.categoryNamePlaceholder")
                .to_string()
                .into();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(40.))
        });
        name_input.update(cx, |input, cx| {
            input.set_value(initial_name.clone(), window, cx);
        });
        let input_sub = cx.subscribe(&name_input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.revalidate(cx),
            InputEvent::PressEnter => this.save(cx),
        });
        name_input.update(cx, |input, cx| input.focus(window, cx));

        Self {
            focus_handle: cx.focus_handle(),
            clan_id,
            category_id,
            channel_list,
            locale,
            initial_name,
            name_input,
            validation: Validation::Validated,
            validation_visible: false,
            saving: false,
            _input_sub: input_sub,
            _save_task: None,
        }
    }

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        let value = self.name_input.read(cx).value();
        self.validation_visible = !value.is_empty();
        self.validation = match validate_category_name(value) {
            Ok(name)
                if self.channel_list.read(cx).category_name_exists_excluding(
                    self.clan_id,
                    &name,
                    &self.category_id,
                ) =>
            {
                Validation::DuplicateName
            }
            Ok(_) => Validation::Validated,
            Err(_) => Validation::InvalidName,
        };
        cx.notify();
    }

    fn has_changes(&self, cx: &App) -> bool {
        self.name_input.read(cx).value().trim() != self.initial_name.trim()
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        if !self.has_changes(cx) {
            Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            return;
        }
        if self.validation != Validation::Validated {
            self.validation_visible = true;
            cx.notify();
            return;
        }
        let name = self.name_input.read(cx).value().to_string();
        let category_id = self.category_id.clone();
        self.saving = true;
        cx.notify();

        let task = self.channel_list.update(cx, |store, cx| {
            store.rename_category(self.clan_id, category_id, name, cx)
        });
        self._save_task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                });
            }
            Err(CreateCategoryError::InvalidName) => {
                let _ = this.update(cx, |this, cx| {
                    this.validation = Validation::InvalidName;
                    this.validation_visible = true;
                    this.saving = false;
                    cx.notify();
                });
            }
            Err(CreateCategoryError::DuplicateName) => {
                let _ = this.update(cx, |this, cx| {
                    this.validation = Validation::DuplicateName;
                    this.validation_visible = true;
                    this.saving = false;
                    cx.notify();
                });
            }
            Err(CreateCategoryError::Other(msg)) => {
                tracing::error!("rename category failed: {msg}");
                let _ = this.update(cx, |this, cx| {
                    this.saving = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(msg, cx));
                });
            }
        }));
    }
}

impl Render for EditCategoryModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let theme_setting_primary = cx.theme().tokens.theme_setting_primary;
        let title: SharedString = mezon_i18n::t(&self.locale, "contextMenu.editCategory")
            .to_string()
            .to_uppercase()
            .into();
        let name_label: SharedString =
            mezon_i18n::t(&self.locale, "clan.categoryOverview.categoryName")
                .to_string()
                .into();
        let invalid_msg: SharedString =
            mezon_i18n::t(&self.locale, "clan.createCategoryModal.invalidName")
                .to_string()
                .into();
        let duplicate_msg: SharedString =
            mezon_i18n::t(&self.locale, "clan.createCategoryModal.duplicateName")
                .to_string()
                .into();
        let cancel_label: SharedString = mezon_i18n::t(&self.locale, "common.cancel")
            .to_string()
            .into();
        let save_label: SharedString = mezon_i18n::t(&self.locale, "common.saveChanges")
            .to_string()
            .into();

        let name_input = self.name_input.clone();
        let show_invalid = self.validation_visible && self.validation == Validation::InvalidName;
        let show_duplicate =
            self.validation_visible && self.validation == Validation::DuplicateName;
        let can_save =
            self.validation == Validation::Validated && !self.saving && self.has_changes(cx);

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.saving {
                    return;
                }
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(455.))
            .max_w(px(455.))
            .rounded(px(12.))
            .bg(theme_setting_primary)
            .shadow_lg()
            .p(px(20.))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .mb(px(20.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .id("edit-category-close")
                            .w(px(28.))
                            .h(px(28.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .opacity(0.65)
                            .hover(|s| s.opacity(1.0))
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            })
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(22.))
                                    .text_color(theme.text_secondary),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .child(
                        div()
                            .mb(px(8.))
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(name_label),
                    )
                    .child(Input::new(&name_input))
                    .child(
                        div()
                            .mt(px(4.))
                            .h(px(18.))
                            .text_xs()
                            .italic()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.danger_text)
                            .when(show_invalid, |el| el.child(invalid_msg))
                            .when(show_duplicate, |el| el.child(duplicate_msg)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_3()
                    .mt(px(20.))
                    .font_weight(FontWeight(900.))
                    .child(
                        Button::new("edit-category-cancel")
                            .label(cancel_label)
                            .ghost()
                            .text_color(gpui::Hsla::from(theme.text_secondary))
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("edit-category-submit")
                            .label(save_label)
                            .primary()
                            .min_w(px(128.))
                            .disabled(!can_save)
                            .loading(self.saving)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.save(cx);
                            })),
                    ),
            )
    }
}
