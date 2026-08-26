use crate::app::shell::Shell;
use crate::components::compositions::channel_row::channel_type_icon;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Radio, Switch, h_flex,
    v_flex,
};
use crate::router::{Route, navigate};
use crate::theme::{ActiveTheme, Theme};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};
use mezon_store::{
    ChannelList, ChannelType, ClanId, CreateChannelError, Settings, validate_channel_name,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Validation {
    InvalidName,
    DuplicateName,
    ChannelLimit,
    Validated,
}

const CHANNEL_TYPE_OPTIONS: [ChannelType; 3] =
    [ChannelType::Text, ChannelType::Voice, ChannelType::Stream];

pub struct CreateChannelModal {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    category_id: String,
    category_name: SharedString,
    channel_list: Entity<ChannelList>,
    locale: String,
    name_input: Entity<InputState>,
    channel_type: ChannelType,
    is_private: bool,
    validation: Validation,
    validation_visible: bool,
    creating: bool,
    _input_sub: Subscription,
    _create_task: Option<Task<()>>,
}

impl Focusable for CreateChannelModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateChannelModal {
    pub fn new(
        clan_id: ClanId,
        category_id: String,
        category_name: String,
        channel_list: Entity<ChannelList>,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder: SharedString = mezon_i18n::t(&locale, "createChannel.labels.placeholder")
            .to_string()
            .into();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(40.))
                .radius(px(4.))
                .borderless()
                .bg(gpui::transparent_black())
        });
        let input_sub = cx.subscribe(&name_input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.revalidate(cx),
            InputEvent::PressEnter => this.create_channel(cx),
        });
        name_input.update(cx, |input, cx| input.focus(window, cx));

        Self {
            focus_handle: cx.focus_handle(),
            clan_id,
            category_id,
            category_name: SharedString::from(category_name),
            channel_list,
            locale,
            name_input,
            channel_type: ChannelType::Text,
            is_private: false,
            validation: Validation::InvalidName,
            validation_visible: false,
            creating: false,
            _input_sub: input_sub,
            _create_task: None,
        }
    }

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        let value = self.name_input.read(cx).value();
        self.validation_visible = !value.is_empty();
        self.validation = match validate_channel_name(value) {
            Ok(name)
                if self.channel_list.read(cx).channel_name_exists_in_category(
                    self.clan_id,
                    &self.category_id,
                    &name,
                ) =>
            {
                Validation::DuplicateName
            }
            Ok(_) => Validation::Validated,
            Err(_) => Validation::InvalidName,
        };
        cx.notify();
    }

    fn set_channel_type(&mut self, channel_type: ChannelType, cx: &mut Context<Self>) {
        if self.channel_type != channel_type {
            self.channel_type = channel_type;
            cx.notify();
        }
    }

    fn set_private(&mut self, private: bool, cx: &mut Context<Self>) {
        self.is_private = private;
        cx.notify();
    }

    fn create_channel(&mut self, cx: &mut Context<Self>) {
        if self.validation != Validation::Validated || self.creating {
            self.validation_visible = true;
            cx.notify();
            return;
        }
        let name = self.name_input.read(cx).value().to_string();
        self.creating = true;
        cx.notify();

        let clan_id = self.clan_id;
        let channel_type = self.channel_type;
        let private = self.is_private && channel_type == ChannelType::Text;
        let category_id = self.category_id.clone();
        let channel_list = self.channel_list.clone();
        let task = self.channel_list.update(cx, |store, cx| {
            store.create_channel(clan_id, category_id, name, channel_type, private, cx)
        });
        self._create_task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok((channel_id, created_type)) => {
                let _ = this.update(cx, |_, cx| {
                    if !matches!(created_type, ChannelType::Voice | ChannelType::Stream) {
                        channel_list.update(cx, |list, cx| list.select_channel(channel_id, cx));
                        navigate(
                            cx,
                            Route::Channel {
                                clan_id,
                                channel_id,
                            },
                        );
                    }
                    Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                });
            }
            Err(CreateChannelError::InvalidName) => {
                let _ = this.update(cx, |this, cx| {
                    this.validation = Validation::InvalidName;
                    this.validation_visible = true;
                    this.creating = false;
                    cx.notify();
                });
            }
            Err(CreateChannelError::DuplicateName) => {
                let _ = this.update(cx, |this, cx| {
                    this.validation = Validation::DuplicateName;
                    this.validation_visible = true;
                    this.creating = false;
                    cx.notify();
                });
            }
            Err(CreateChannelError::ChannelLimitExceeded) => {
                let _ = this.update(cx, |this, cx| {
                    this.validation = Validation::ChannelLimit;
                    this.validation_visible = true;
                    this.creating = false;
                    cx.notify();
                });
            }
            Err(CreateChannelError::Api(code)) => {
                tracing::error!("create channel failed: API code {code}");
                let _ = this.update(cx, |this, cx| {
                    this.creating = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    let locale = Settings::try_global(cx)
                        .map(|settings| settings.read(cx).language.clone())
                        .unwrap_or_else(|| "en".to_string());
                    let message = mezon_i18n::api_error(&locale, code).to_string();
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                });
            }
            Err(CreateChannelError::Other(msg)) => {
                tracing::error!("create channel failed: {msg}");
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

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn type_label_key(channel_type: ChannelType) -> &'static str {
        match channel_type {
            ChannelType::Voice => "createChannel.channelType.voice",
            ChannelType::Stream => "createChannel.channelType.stream",
            _ => "createChannel.channelType.text",
        }
    }

    fn type_description_key(channel_type: ChannelType) -> &'static str {
        match channel_type {
            ChannelType::Voice => "createChannel.channelType.descriptions.voice",
            ChannelType::Stream => "createChannel.channelType.descriptions.stream",
            _ => "createChannel.channelType.descriptions.text",
        }
    }
}

impl Render for CreateChannelModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let entity = cx.entity();
        let locale = self.locale.clone();

        let title: SharedString = mezon_i18n::t(&locale, "createChannel.header.title")
            .to_string()
            .into();
        let description: SharedString = mezon_i18n::t(&locale, "createChannel.labels.description")
            .to_string()
            .into();
        let choose_type: SharedString = mezon_i18n::t(&locale, "createChannel.labels.chooseType")
            .to_string()
            .into();
        let name_label: SharedString = mezon_i18n::t(&locale, "createChannel.labels.channelName")
            .to_string()
            .into();
        let invalid_msg: SharedString =
            mezon_i18n::t(&locale, "createChannel.validation.invalidName")
                .to_string()
                .into();
        let duplicate_msg: SharedString =
            mezon_i18n::t(&locale, "createChannel.validation.duplicateName")
                .to_string()
                .into();
        let channel_limit_msg: SharedString = mezon_i18n::t(&locale, "channel.createLimitExceeded")
            .to_string()
            .into();
        let private_label: SharedString = mezon_i18n::t(&locale, "createChannel.privacy.private")
            .to_string()
            .into();
        let private_desc: SharedString =
            mezon_i18n::t(&locale, "createChannel.privacy.description")
                .to_string()
                .into();
        let cancel_label: SharedString = mezon_i18n::t(&locale, "createChannel.buttons.cancel")
            .to_string()
            .into();
        let create_label: SharedString = if self.creating {
            mezon_i18n::t(&locale, "createChannel.buttons.creating")
                .to_string()
                .into()
        } else {
            mezon_i18n::t(&locale, "createChannel.buttons.create")
                .to_string()
                .into()
        };

        let show_invalid = self.validation_visible && self.validation == Validation::InvalidName;
        let show_duplicate =
            self.validation_visible && self.validation == Validation::DuplicateName;
        let show_channel_limit =
            self.validation_visible && self.validation == Validation::ChannelLimit;
        let can_create = self.validation == Validation::Validated && !self.creating;
        let selected_type = self.channel_type;

        let type_rows = CHANNEL_TYPE_OPTIONS
            .iter()
            .map(|channel_type| {
                render_type_row(
                    &theme,
                    &locale,
                    *channel_type,
                    selected_type == *channel_type,
                    entity.clone(),
                )
            })
            .collect::<Vec<_>>();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .w(px(684.))
            .max_w(px(684.))
            .rounded(px(16.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .p(px(20.))
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_start()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text_link)
                                    .child(self.category_name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("create-channel-close")
                            .w(px(28.))
                            .h(px(28.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .opacity(0.65)
                            .hover(|s| s.opacity(1.0))
                            .on_click(|_, _, cx| Self::close(cx))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(22.))
                                    .text_color(theme.text_secondary),
                            ),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(description),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(choose_type),
                    )
                    .children(type_rows),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(name_label),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .pl(px(12.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if show_invalid || show_duplicate || show_channel_limit {
                                theme.status_dnd
                            } else {
                                theme.tokens.border_focus
                            })
                            .bg(theme.tokens.bg_active_member_channel)
                            .child(
                                Icon::new(channel_type_icon(selected_type, false))
                                    .size(px(20.))
                                    .text_color(theme.text_secondary),
                            )
                            .child(div().flex_1().child(Input::new(&self.name_input))),
                    )
                    .child(
                        div()
                            .h(px(18.))
                            .text_xs()
                            .italic()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.status_dnd)
                            .when(show_invalid, |el| el.child(invalid_msg))
                            .when(show_duplicate, |el| el.child(duplicate_msg))
                            .when(show_channel_limit, |el| el.child(channel_limit_msg)),
                    ),
            )
            .when(selected_type == ChannelType::Text, |el| {
                let switch_entity = entity.clone();
                el.child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            Icon::new(IconName::Private)
                                                .size(px(24.))
                                                .text_color(theme.text_secondary),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme.tokens.text_theme_primary)
                                                .child(private_label),
                                        ),
                                )
                                .child(
                                    Switch::new("create-channel-private")
                                        .checked(self.is_private)
                                        .on_click(move |checked, _window, cx| {
                                            switch_entity.update(cx, |this, cx| {
                                                this.set_private(*checked, cx);
                                            });
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(private_desc),
                        ),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_3()
                    .mt(px(8.))
                    .font_weight(FontWeight(900.))
                    .child(
                        Button::new("create-channel-cancel")
                            .label(cancel_label)
                            .ghost()
                            .text_color(gpui::Hsla::from(theme.text_secondary))
                            .on_click(|_, _, cx| Self::close(cx)),
                    )
                    .child(
                        Button::new("create-channel-submit")
                            .label(create_label)
                            .primary()
                            .min_w(px(128.))
                            .disabled(!can_create)
                            .loading(self.creating)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.create_channel(cx);
                            })),
                    ),
            )
    }
}

fn render_type_row(
    theme: &Theme,
    locale: &str,
    channel_type: ChannelType,
    selected: bool,
    entity: Entity<CreateChannelModal>,
) -> AnyElement {
    let label: SharedString =
        mezon_i18n::t(locale, CreateChannelModal::type_label_key(channel_type))
            .to_string()
            .into();
    let description: SharedString = mezon_i18n::t(
        locale,
        CreateChannelModal::type_description_key(channel_type),
    )
    .to_string()
    .into();

    h_flex()
        .id(SharedString::from(format!(
            "create-channel-type-{}",
            channel_type.as_raw()
        )))
        .w_full()
        .px_2()
        .py_2()
        .gap_4()
        .items_center()
        .rounded_lg()
        .bg(theme.tokens.bg_active_member_channel)
        .cursor_pointer()
        .hover(|s| s.bg(theme.tokens.bg_item_hover))
        .on_click(move |_, _window, cx| {
            entity.update(cx, |this, cx| this.set_channel_type(channel_type, cx));
        })
        .child(
            Icon::new(channel_type_icon(channel_type, false))
                .size(px(24.))
                .text_color(theme.tokens.text_theme_primary),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.tokens.text_theme_primary)
                        .child(label),
                )
                .child(
                    div()
                        .text_sm()
                        .truncate()
                        .text_color(theme.text_secondary)
                        .child(description),
                ),
        )
        .child(
            Radio::new(SharedString::from(format!(
                "create-channel-type-radio-{}",
                channel_type.as_raw()
            )))
            .selected(selected),
        )
        .into_any_element()
}
