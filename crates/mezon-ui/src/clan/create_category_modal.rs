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

pub struct CreateCategoryModal {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    channel_list: Entity<ChannelList>,
    locale: String,
    name_input: Entity<InputState>,
    validation: Validation,
    creating: bool,
    _input_sub: Subscription,
    _create_task: Option<Task<()>>,
}

impl Focusable for CreateCategoryModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateCategoryModal {
    pub fn new(
        clan_id: ClanId,
        channel_list: Entity<ChannelList>,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "clan.createCategoryModal.namePlaceholder")
                .to_string()
                .into();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(48.))
        });
        let input_sub = cx.subscribe(&name_input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.revalidate(cx),
            InputEvent::PressEnter => this.create_category(cx),
        });
        name_input.update(cx, |input, cx| input.focus(window, cx));

        Self {
            focus_handle: cx.focus_handle(),
            clan_id,
            channel_list,
            locale,
            name_input,
            validation: Validation::InvalidName,
            creating: false,
            _input_sub: input_sub,
            _create_task: None,
        }
    }

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        let value = self.name_input.read(cx).value();
        self.validation = match validate_category_name(value) {
            Ok(name)
                if self
                    .channel_list
                    .read(cx)
                    .category_name_exists(self.clan_id, &name) =>
            {
                Validation::DuplicateName
            }
            Ok(_) => Validation::Validated,
            Err(_) => Validation::InvalidName,
        };
        cx.notify();
    }

    fn create_category(&mut self, cx: &mut Context<Self>) {
        if self.validation != Validation::Validated || self.creating {
            return;
        }
        let name = self.name_input.read(cx).value().to_string();
        self.creating = true;
        cx.notify();

        let task = self.channel_list.update(cx, |store, cx| {
            store.create_category(self.clan_id, name, cx)
        });
        self._create_task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                });
            }
            Err(CreateCategoryError::InvalidName) => {
                let _ = this.update(cx, |this, cx| {
                    this.validation = Validation::InvalidName;
                    this.creating = false;
                    cx.notify();
                });
            }
            Err(CreateCategoryError::DuplicateName) => {
                let _ = this.update(cx, |this, cx| {
                    this.validation = Validation::DuplicateName;
                    this.creating = false;
                    cx.notify();
                });
            }
            Err(CreateCategoryError::Other(msg)) => {
                tracing::error!("create category failed: {msg}");
                let _ = this.update(cx, |this, cx| {
                    this.creating = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(msg, cx));
                });
            }
        }));
    }
}

impl Render for CreateCategoryModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let theme_setting_primary = cx.theme().tokens.theme_setting_primary;
        let title: SharedString = mezon_i18n::t(&self.locale, "clan.createCategoryModal.title")
            .to_string()
            .to_uppercase()
            .into();
        let name_label: SharedString =
            mezon_i18n::t(&self.locale, "clan.createCategoryModal.nameLabel")
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
        let cancel_label: SharedString =
            mezon_i18n::t(&self.locale, "clan.createCategoryModal.cancel")
                .to_string()
                .into();
        let create_label: SharedString =
            mezon_i18n::t(&self.locale, "clan.createCategoryModal.create")
                .to_string()
                .into();

        let name_input = self.name_input.clone();
        let show_invalid = self.validation == Validation::InvalidName;
        let show_duplicate = self.validation == Validation::DuplicateName;
        let can_create = self.validation == Validation::Validated && !self.creating;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(600.))
            .max_w(px(600.))
            .rounded(px(12.))
            .bg(theme_setting_primary)
            .shadow_lg()
            .p(px(30.))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .mb(px(28.))
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .id("create-category-close")
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
                            .mb(px(12.))
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
                            .text_color(theme.status_dnd)
                            .when(show_invalid, |el| el.child(invalid_msg))
                            .when(show_duplicate, |el| el.child(duplicate_msg)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_3()
                    .mt(px(28.))
                    .font_weight(FontWeight(900.))
                    .child(
                        Button::new("create-category-cancel")
                            .label(cancel_label)
                            .ghost()
                            .text_color(gpui::Hsla::from(theme.text_secondary))
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("create-category-submit")
                            .label(create_label)
                            .primary()
                            .min_w(px(128.))
                            .disabled(!can_create)
                            .loading(self.creating)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.create_category(cx);
                            })),
                    ),
            )
    }
}
