use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, SharedString, Subscription,
    Task, Window, div, prelude::*, px, rgb,
};
use mezon_store::{
    ChannelId, ClanId, QUICK_MENU_TYPE_FLASH, QUICK_MENU_TYPE_QUICK, QuickMenuStore, Settings,
    is_valid_action_msg, is_valid_menu_name, name_exists,
};
use ui::Tooltip;

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, TextArea, TextAreaEvent,
    TextAreaField, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const COMMAND_CHIP: u32 = 0x00d4aa;
const TYPE_BADGE: u32 = 0x3b82f6;
const TYPE_BADGE_TEXT: u32 = 0x60a5fa;
const CALLOUT_BODY: u32 = 0x93c5fd;
const CARD_HOVER_BORDER: u32 = 0x4e5156;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickActionsSubTab {
    Flash,
    Menu,
}

impl QuickActionsSubTab {
    fn menu_type(self) -> i32 {
        match self {
            Self::Flash => QUICK_MENU_TYPE_FLASH,
            Self::Menu => QUICK_MENU_TYPE_QUICK,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlashFormError {
    None,
    InvalidName,
    DuplicateName,
    MessageTooLong,
}

pub struct QuickActionsTab {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    active_tab: QuickActionsSubTab,
    _subs: Vec<Subscription>,
}

impl QuickActionsTab {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        QuickMenuStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(channel_id, QUICK_MENU_TYPE_FLASH, cx);
            store.ensure_loaded(channel_id, QUICK_MENU_TYPE_QUICK, cx);
        });
        let subs = vec![
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&QuickMenuStore::global(cx), |_, _, cx| cx.notify()),
        ];
        Self {
            clan_id,
            channel_id,
            settings,
            active_tab: QuickActionsSubTab::Flash,
            _subs: subs,
        }
    }

    fn open_create_flash(&self, window: &mut Window, cx: &mut Context<Self>) {
        CreateFlashMessageModal::open(
            self.clan_id,
            self.channel_id,
            self.settings.clone(),
            None,
            window,
            cx,
        );
    }

    fn open_create_quick_menu(&self, window: &mut Window, cx: &mut Context<Self>) {
        CreateQuickMenuModal::open(
            self.clan_id,
            self.channel_id,
            self.settings.clone(),
            None,
            window,
            cx,
        );
    }

    fn open_edit_flash(
        &self,
        id: i64,
        menu_name: String,
        action_msg: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        CreateFlashMessageModal::open(
            self.clan_id,
            self.channel_id,
            self.settings.clone(),
            Some((id, menu_name, action_msg)),
            window,
            cx,
        );
    }

    fn open_edit_quick_menu(
        &self,
        id: i64,
        menu_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        CreateQuickMenuModal::open(
            self.clan_id,
            self.channel_id,
            self.settings.clone(),
            Some((id, menu_name)),
            window,
            cx,
        );
    }

    fn confirm_delete(
        &self,
        id: i64,
        command_label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let locale = self.settings.read(cx).language.clone();
        let is_flash = self.active_tab == QuickActionsSubTab::Flash;
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        Shell::global(cx).update(cx, |shell, cx| {
            shell.confirm_delete_quick_menu(
                clan_id,
                channel_id,
                id,
                command_label.as_ref(),
                is_flash,
                &locale,
                window,
                cx,
            );
        });
    }

    fn render_tab_button(
        &self,
        tab: QuickActionsSubTab,
        label: SharedString,
        count: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = self.active_tab == tab;
        div()
            .id(match tab {
                QuickActionsSubTab::Flash => "quick-actions-tab-flash",
                QuickActionsSubTab::Menu => "quick-actions-tab-menu",
            })
            .px(px(16.))
            .py(px(8.))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .rounded(px(6.))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .when(is_active, |el| el.bg(theme.brand).text_color(gpui::white()))
            .when(!is_active, |el| {
                el.text_color(theme.tokens.text_theme_primary)
                    .hover(|style| {
                        style
                            .text_color(theme.text_primary)
                            .bg(theme.tokens.bg_item_theme_hover)
                    })
            })
            .child(label)
            .child(
                div()
                    .px(px(8.))
                    .py(px(2.))
                    .rounded_full()
                    .text_xs()
                    .when(is_active, |el| el.bg(gpui::white().opacity(0.2)))
                    .when(!is_active, |el| {
                        el.bg(theme.tokens.theme_setting_nav)
                            .text_color(theme.tokens.text_theme_primary)
                    })
                    .child(count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.active_tab = tab;
                QuickMenuStore::global(cx).update(cx, |store, cx| {
                    store.ensure_loaded(this.channel_id, tab.menu_type(), cx);
                });
                cx.notify();
            }))
    }

    fn render_empty_state(&self, locale: &str, theme: &Theme) -> impl IntoElement {
        let (title_key, description_key) = match self.active_tab {
            QuickActionsSubTab::Flash => (
                "channelSetting.quickAction.emptyFlashMessage",
                "channelSetting.quickAction.emptyFlashMessageDescription",
            ),
            QuickActionsSubTab::Menu => (
                "channelSetting.quickAction.emptyQuickMenu",
                "channelSetting.quickAction.emptyQuickMenuDescription",
            ),
        };
        v_flex()
            .w_full()
            .items_center()
            .p(px(32.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.tokens.border_theme_primary)
            .bg(theme.tokens.theme_setting_nav)
            .child(
                Icon::new(IconName::QuickActionEmpty)
                    .size(px(48.))
                    .text_color(theme.tokens.text_theme_primary),
            )
            .child(
                div()
                    .mt_4()
                    .mb_2()
                    .text_lg()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(locale, title_key)),
            )
            .child(
                div()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(mezon_i18n::t(locale, description_key)),
            )
    }

    fn render_command_list(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_flash = self.active_tab == QuickActionsSubTab::Flash;
        let type_label: SharedString = if is_flash {
            mezon_i18n::t(locale, "channelSetting.quickAction.flashMessage").into()
        } else {
            mezon_i18n::t(locale, "channelSetting.quickAction.quickMenu").into()
        };
        let edit_title: SharedString =
            mezon_i18n::t(locale, "channelSetting.quickAction.editCommand").into();
        let delete_title: SharedString =
            mezon_i18n::t(locale, "channelSetting.quickAction.deleteCommand").into();
        let rows: Vec<(i64, String, String, SharedString, SharedString)> =
            QuickMenuStore::global(cx)
                .read(cx)
                .items(self.channel_id, self.active_tab.menu_type())
                .iter()
                .map(|item| {
                    let menu_name = item.menu_name.to_string();
                    let action_msg = item.action_msg.to_string();
                    let label = if is_flash {
                        format!("/{menu_name}")
                    } else {
                        menu_name.clone()
                    };
                    let preview = if is_flash {
                        item.action_msg.clone()
                    } else {
                        mezon_i18n::t(locale, "channelSetting.quickAction.triggersBot").into()
                    };
                    (item.id, menu_name, action_msg, label.into(), preview)
                })
                .collect();
        v_flex().w_full().gap_3().children(rows.into_iter().map(
            |(id, menu_name, action_msg, label, preview)| {
                let edit_title = edit_title.clone();
                let delete_title = delete_title.clone();
                let type_label = type_label.clone();
                let command_label = label.clone();
                h_flex()
                    .w_full()
                    .items_start()
                    .justify_between()
                    .p(px(16.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.tokens.border_theme_primary)
                    .bg(theme.tokens.theme_setting_nav)
                    .hover(|style| style.border_color(rgb(CARD_HOVER_BORDER)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .mb_2()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded(px(4.))
                                            .text_sm()
                                            .font_family("monospace")
                                            .text_color(rgb(COMMAND_CHIP))
                                            .bg(Hsla::from(rgb(COMMAND_CHIP)).opacity(0.1))
                                            .child(label),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_full()
                                            .text_xs()
                                            .text_color(rgb(TYPE_BADGE_TEXT))
                                            .bg(Hsla::from(rgb(TYPE_BADGE)).opacity(0.2))
                                            .child(type_label),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .when(!is_flash, |el| el.italic())
                                    .text_color(theme.text_secondary)
                                    .child(preview),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .ml_4()
                            .child(
                                div()
                                    .id(("qa-edit", id as u64))
                                    .p(px(6.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .text_color(theme.text_secondary)
                                    .hover(|style| {
                                        style.text_color(theme.text_primary).bg(theme.bg_hover)
                                    })
                                    .tooltip(Tooltip::text(edit_title))
                                    .child(
                                        Icon::new(IconName::QuickActionEdit)
                                            .size(px(14.))
                                            .text_color(theme.text_secondary),
                                    )
                                    .on_click(cx.listener({
                                        let menu_name = menu_name.clone();
                                        let action_msg = action_msg.clone();
                                        move |this, _, window, cx| {
                                            if this.active_tab == QuickActionsSubTab::Flash {
                                                this.open_edit_flash(
                                                    id,
                                                    menu_name.clone(),
                                                    action_msg.clone(),
                                                    window,
                                                    cx,
                                                );
                                            } else {
                                                this.open_edit_quick_menu(
                                                    id,
                                                    menu_name.clone(),
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id(("qa-delete", id as u64))
                                    .p(px(6.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .text_color(theme.text_secondary)
                                    .hover(|style| {
                                        style.text_color(theme.danger).bg(theme.danger_hover_bg)
                                    })
                                    .tooltip(Tooltip::text(delete_title))
                                    .child(
                                        Icon::new(IconName::QuickActionTrash)
                                            .size(px(14.))
                                            .text_color(theme.text_secondary),
                                    )
                                    .on_click(cx.listener({
                                        let command_label = command_label.clone();
                                        move |this, _, window, cx| {
                                            this.confirm_delete(
                                                id,
                                                command_label.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    )
            },
        ))
    }
}

impl Render for QuickActionsTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let store = QuickMenuStore::global(cx);
        let store = store.read(cx);
        let flash_count = store.items(self.channel_id, QUICK_MENU_TYPE_FLASH).len();
        let menu_count = store.items(self.channel_id, QUICK_MENU_TYPE_QUICK).len();
        let current_count = match self.active_tab {
            QuickActionsSubTab::Flash => flash_count,
            QuickActionsSubTab::Menu => menu_count,
        };
        let add_label = match self.active_tab {
            QuickActionsSubTab::Flash => {
                mezon_i18n::t(&locale, "channelSetting.quickAction.addFlashMessage")
            }
            QuickActionsSubTab::Menu => {
                mezon_i18n::t(&locale, "channelSetting.quickAction.addQuickMenu")
            }
        };
        v_flex()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .justify_between()
                    .mb(px(24.))
                    .child(
                        v_flex()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child(mezon_i18n::t(
                                        &locale,
                                        "channelSetting.quickAction.title",
                                    )),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_sm()
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(mezon_i18n::t(
                                        &locale,
                                        "channelSetting.quickAction.description",
                                    )),
                            ),
                    )
                    .child(
                        Button::new("quick-actions-add")
                            .label(add_label)
                            .icon(
                                Icon::new(IconName::AddCircle)
                                    .size(px(14.))
                                    .text_color(gpui::white()),
                            )
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| match this.active_tab {
                                QuickActionsSubTab::Flash => {
                                    this.open_create_flash(window, cx);
                                }
                                QuickActionsSubTab::Menu => {
                                    this.open_create_quick_menu(window, cx);
                                }
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .mb(px(24.))
                    .child(self.render_tab_button(
                        QuickActionsSubTab::Flash,
                        mezon_i18n::t(&locale, "channelSetting.quickAction.flashMessages").into(),
                        flash_count,
                        &theme,
                        cx,
                    ))
                    .child(self.render_tab_button(
                        QuickActionsSubTab::Menu,
                        mezon_i18n::t(&locale, "channelSetting.quickAction.quickMenus").into(),
                        menu_count,
                        &theme,
                        cx,
                    )),
            )
            .child(if current_count == 0 {
                self.render_empty_state(&locale, &theme).into_any_element()
            } else {
                self.render_command_list(&locale, &theme, cx)
                    .into_any_element()
            })
    }
}

struct CreateFlashMessageModal {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    name_input: Entity<InputState>,
    content_input: Entity<TextArea>,
    editing_id: Option<i64>,
    error: FlashFormError,
    submitting: bool,
    focus_handle: FocusHandle,
    _name_sub: Subscription,
    _content_sub: Subscription,
    _submit_task: Option<Task<()>>,
}

impl Focusable for CreateFlashMessageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateFlashMessageModal {
    fn open<T: 'static>(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        editing: Option<(i64, String, String)>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Entity<Self> {
        let modal = cx.new(|cx| Self::new(clan_id, channel_id, settings, editing, window, cx));
        let focus_handle = modal.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.clone().into(), cx));
        modal
    }

    fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        editing: Option<(i64, String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let locale = settings.read(cx).language.clone();
        let theme = cx.theme().clone();
        let name_placeholder: SharedString = "example".into();
        let content_placeholder: SharedString = mezon_i18n::t(
            &locale,
            "channelSetting.quickAction.messageContentPlaceholder",
        )
        .into();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(name_placeholder)
                .height(px(40.))
                .embedded(true)
                .borderless()
                .bg(gpui::transparent_black())
                .text_color(theme.text_primary)
                .text_size(px(15.))
        });
        let content_input = cx.new(|cx| {
            TextArea::new(window, cx)
                .placeholder(content_placeholder)
                .min_height(px(72.))
                .max_visible_lines(6)
                .bg(gpui::transparent_black())
                .text_color(theme.text_primary)
                .text_size(px(15.))
                .padding_x(px(12.))
                .radius(px(0.))
        });
        if let Some((_, name, content)) = &editing {
            name_input.update(cx, |input, cx| {
                input.set_value(name.clone(), window, cx);
            });
            content_input.update(cx, |input, cx| {
                input.set_value(content.clone(), cx);
            });
        }
        let name_sub = cx.subscribe(&name_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.revalidate(cx);
            }
        });
        let content_sub = cx.subscribe(&content_input, |this, _, event: &TextAreaEvent, cx| {
            if *event == TextAreaEvent::Change {
                this.revalidate(cx);
            }
        });
        Self {
            clan_id,
            channel_id,
            settings,
            name_input,
            content_input,
            editing_id: editing.map(|(id, _, _)| id),
            error: FlashFormError::None,
            submitting: false,
            focus_handle: cx.focus_handle(),
            _name_sub: name_sub,
            _content_sub: content_sub,
            _submit_task: None,
        }
    }

    fn draft_name(&self, cx: &App) -> String {
        self.name_input.read(cx).value().trim().to_string()
    }

    fn draft_content(&self, cx: &App) -> String {
        self.content_input.read(cx).value().trim().to_string()
    }

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        self.error = self.current_error(cx);
        cx.notify();
    }

    fn current_error(&self, cx: &App) -> FlashFormError {
        let name = self.draft_name(cx);
        let content = self.draft_content(cx);
        if !name.is_empty() && !is_valid_menu_name(&name) {
            return FlashFormError::InvalidName;
        }
        if !name.is_empty() {
            let items = QuickMenuStore::global(cx)
                .read(cx)
                .items(self.channel_id, QUICK_MENU_TYPE_FLASH);
            if name_exists(items, &name, self.editing_id) {
                return FlashFormError::DuplicateName;
            }
        }
        if !content.is_empty() && !is_valid_action_msg(&content) {
            return FlashFormError::MessageTooLong;
        }
        FlashFormError::None
    }

    fn can_submit(&self, cx: &App) -> bool {
        let name = self.draft_name(cx);
        let content = self.draft_content(cx);
        !self.submitting
            && !name.is_empty()
            && !content.is_empty()
            && self.current_error(cx) == FlashFormError::None
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            self.revalidate(cx);
            return;
        }
        let name = self.draft_name(cx);
        let content = self.draft_content(cx);
        self.submitting = true;
        cx.notify();
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let editing_id = self.editing_id;
        let task = QuickMenuStore::global(cx).update(cx, |store, cx| {
            if let Some(id) = editing_id {
                store.update(
                    clan_id,
                    channel_id,
                    id,
                    name,
                    content,
                    QUICK_MENU_TYPE_FLASH,
                    cx,
                )
            } else {
                store.add(
                    clan_id,
                    channel_id,
                    name,
                    content,
                    QUICK_MENU_TYPE_FLASH,
                    cx,
                )
            }
        });
        self._submit_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.submitting = false;
                this._submit_task = None;
                match result {
                    Ok(()) => {
                        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                    }
                    Err(err) => {
                        Shell::global(cx).update(cx, |shell, cx| shell.error(err, cx));
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for CreateFlashMessageModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let can_submit = self.can_submit(cx);
        let error = self.error;
        let error_text = match error {
            FlashFormError::InvalidName => Some(mezon_i18n::t(
                &locale,
                "channelSetting.quickAction.errorInvalidName",
            )),
            FlashFormError::DuplicateName => Some(mezon_i18n::t(
                &locale,
                "channelSetting.quickAction.errorDuplicateName",
            )),
            FlashFormError::MessageTooLong => Some(mezon_i18n::t(
                &locale,
                "channelSetting.quickAction.errorMessageTooLong",
            )),
            FlashFormError::None => None,
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_this, _: &::menu::Cancel, _window, cx| {
                Self::close(cx);
            }))
            .on_action(cx.listener(|this, _: &::menu::Confirm, _window, cx| {
                this.submit(cx);
            }))
            .w(px(448.))
            .rounded_lg()
            .bg(theme.tokens.theme_setting_primary)
            .text_color(theme.tokens.text_theme_primary)
            .child(
                div()
                    .p(px(24.))
                    .border_b_1()
                    .border_color(theme.tokens.border_theme_primary)
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(
                        &locale,
                        if self.editing_id.is_some() {
                            "channelSetting.quickAction.editFlashMessage"
                        } else {
                            "channelSetting.quickAction.createFlashMessage"
                        },
                    )),
            )
            .child(
                v_flex()
                    .p(px(24.))
                    .gap_4()
                    .child(
                        v_flex()
                            .w_full()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .mb_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "channelSetting.quickAction.commandName",
                                            )),
                                    )
                                    .child(div().text_sm().text_color(rgb(0xe44141)).child("*")),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .h(px(40.))
                                    .items_center()
                                    .pl(px(12.))
                                    .pr(px(12.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.tokens.border_theme_primary)
                                    .bg(theme.tokens.bg_input_secondary)
                                    .child(
                                        div()
                                            .mr_2()
                                            .font_family("monospace")
                                            .text_color(theme.text_primary)
                                            .child("/"),
                                    )
                                    .child(Input::new(&self.name_input).w_full()),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(mezon_i18n::t(
                                        &locale,
                                        "channelSetting.quickAction.commandNameHelper",
                                    )),
                            )
                            .when(
                                error == FlashFormError::InvalidName
                                    || error == FlashFormError::DuplicateName,
                                |el| {
                                    el.child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(theme.danger)
                                            .child(error_text.unwrap_or_default()),
                                    )
                                },
                            ),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .mb_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "channelSetting.quickAction.messageContent",
                                            )),
                                    )
                                    .child(div().text_sm().text_color(rgb(0xe44141)).child("*")),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_h(px(72.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.tokens.border_theme_primary)
                                    .bg(theme.tokens.bg_input_secondary)
                                    .overflow_hidden()
                                    .child(TextAreaField::new(&self.content_input).w_full()),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(mezon_i18n::t(
                                        &locale,
                                        "channelSetting.quickAction.messageContentDescription",
                                    )),
                            )
                            .when(error == FlashFormError::MessageTooLong, |el| {
                                el.child(
                                    div()
                                        .mt_1()
                                        .text_xs()
                                        .text_color(theme.danger)
                                        .child(error_text.unwrap_or_default()),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_3()
                            .pt_4()
                            .child(
                                Button::new("create-flash-cancel")
                                    .label(mezon_i18n::t(
                                        &locale,
                                        "channelSetting.quickAction.cancel",
                                    ))
                                    .link()
                                    .disabled(self.submitting)
                                    .on_click(|_, _, cx| Self::close(cx)),
                            )
                            .child(
                                Button::new("create-flash-submit")
                                    .label(mezon_i18n::t(
                                        &locale,
                                        if self.editing_id.is_some() {
                                            "channelSetting.quickAction.update"
                                        } else {
                                            "channelSetting.quickAction.create"
                                        },
                                    ))
                                    .primary()
                                    .loading(self.submitting)
                                    .disabled(!can_submit)
                                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                            ),
                    ),
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickMenuFormError {
    None,
    InvalidName,
    DuplicateName,
}

struct CreateQuickMenuModal {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    name_input: Entity<InputState>,
    editing_id: Option<i64>,
    error: QuickMenuFormError,
    submitting: bool,
    focus_handle: FocusHandle,
    _name_sub: Subscription,
    _submit_task: Option<Task<()>>,
}

impl Focusable for CreateQuickMenuModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateQuickMenuModal {
    fn open<T: 'static>(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        editing: Option<(i64, String)>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Entity<Self> {
        let modal = cx.new(|cx| Self::new(clan_id, channel_id, settings, editing, window, cx));
        let focus_handle = modal.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.clone().into(), cx));
        modal
    }

    fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        editing: Option<(i64, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = cx.theme().clone();
        let name_placeholder: SharedString = "menu-name".into();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(name_placeholder)
                .height(px(40.))
                .embedded(true)
                .borderless()
                .bg(gpui::transparent_black())
                .text_color(theme.text_primary)
                .text_size(px(15.))
        });
        if let Some((_, name)) = &editing {
            name_input.update(cx, |input, cx| {
                input.set_value(name.clone(), window, cx);
            });
        }
        let name_sub = cx.subscribe(&name_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.revalidate(cx);
            }
        });
        Self {
            clan_id,
            channel_id,
            settings,
            name_input,
            editing_id: editing.map(|(id, _)| id),
            error: QuickMenuFormError::None,
            submitting: false,
            focus_handle: cx.focus_handle(),
            _name_sub: name_sub,
            _submit_task: None,
        }
    }

    fn draft_name(&self, cx: &App) -> String {
        self.name_input.read(cx).value().trim().to_string()
    }

    fn revalidate(&mut self, cx: &mut Context<Self>) {
        self.error = self.current_error(cx);
        cx.notify();
    }

    fn current_error(&self, cx: &App) -> QuickMenuFormError {
        let name = self.draft_name(cx);
        if name.is_empty() {
            return QuickMenuFormError::None;
        }
        if !is_valid_menu_name(&name) {
            return QuickMenuFormError::InvalidName;
        }
        let items = QuickMenuStore::global(cx)
            .read(cx)
            .items(self.channel_id, QUICK_MENU_TYPE_QUICK);
        if name_exists(items, &name, self.editing_id) {
            return QuickMenuFormError::DuplicateName;
        }
        QuickMenuFormError::None
    }

    fn can_submit(&self, cx: &App) -> bool {
        !self.submitting
            && !self.draft_name(cx).is_empty()
            && self.current_error(cx) == QuickMenuFormError::None
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            self.revalidate(cx);
            return;
        }
        let name = self.draft_name(cx);
        self.submitting = true;
        cx.notify();
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let editing_id = self.editing_id;
        let task = QuickMenuStore::global(cx).update(cx, |store, cx| {
            if let Some(id) = editing_id {
                store.update(
                    clan_id,
                    channel_id,
                    id,
                    name,
                    "bot_event".to_string(),
                    QUICK_MENU_TYPE_QUICK,
                    cx,
                )
            } else {
                store.add(
                    clan_id,
                    channel_id,
                    name,
                    "bot_event".to_string(),
                    QUICK_MENU_TYPE_QUICK,
                    cx,
                )
            }
        });
        self._submit_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.submitting = false;
                this._submit_task = None;
                match result {
                    Ok(()) => {
                        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                    }
                    Err(err) => {
                        Shell::global(cx).update(cx, |shell, cx| shell.error(err, cx));
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for CreateQuickMenuModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let can_submit = self.can_submit(cx);
        let error = self.error;
        let error_text = match error {
            QuickMenuFormError::InvalidName => Some(mezon_i18n::t(
                &locale,
                "channelSetting.quickAction.errorInvalidName",
            )),
            QuickMenuFormError::DuplicateName => Some(mezon_i18n::t(
                &locale,
                "channelSetting.quickAction.errorDuplicateName",
            )),
            QuickMenuFormError::None => None,
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_this, _: &::menu::Cancel, _window, cx| {
                Self::close(cx);
            }))
            .on_action(cx.listener(|this, _: &::menu::Confirm, _window, cx| {
                this.submit(cx);
            }))
            .w(px(448.))
            .rounded_lg()
            .bg(theme.tokens.theme_setting_primary)
            .text_color(theme.tokens.text_theme_primary)
            .child(
                div()
                    .p(px(24.))
                    .border_b_1()
                    .border_color(theme.tokens.border_theme_primary)
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(mezon_i18n::t(
                        &locale,
                        if self.editing_id.is_some() {
                            "channelSetting.quickAction.editQuickMenu"
                        } else {
                            "channelSetting.quickAction.createQuickMenu"
                        },
                    )),
            )
            .child(
                v_flex()
                    .p(px(24.))
                    .gap_4()
                    .child(
                        v_flex()
                            .w_full()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .mb_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "channelSetting.quickAction.menuName",
                                            )),
                                    )
                                    .child(div().text_sm().text_color(rgb(0xe44141)).child("*")),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .h(px(40.))
                                    .items_center()
                                    .pl(px(12.))
                                    .pr(px(12.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.tokens.border_theme_primary)
                                    .bg(theme.tokens.bg_input_secondary)
                                    .child(Input::new(&self.name_input).w_full()),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(mezon_i18n::t(
                                        &locale,
                                        "channelSetting.quickAction.menuNameHelper",
                                    )),
                            )
                            .when(error != QuickMenuFormError::None, |el| {
                                el.child(
                                    div()
                                        .mt_1()
                                        .text_xs()
                                        .text_color(theme.danger)
                                        .child(error_text.unwrap_or_default()),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .items_start()
                            .gap_2()
                            .p_3()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(Hsla::from(rgb(TYPE_BADGE)).opacity(0.2))
                            .bg(Hsla::from(rgb(TYPE_BADGE)).opacity(0.1))
                            .child(
                                Icon::new(IconName::QuickActionInfo)
                                    .size(px(16.))
                                    .text_color(rgb(TYPE_BADGE_TEXT)),
                            )
                            .child(
                                v_flex()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TYPE_BADGE_TEXT))
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "channelSetting.quickAction.botEventTrigger",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(Hsla::from(rgb(CALLOUT_BODY)).opacity(0.8))
                                            .child(mezon_i18n::t(
                                                &locale,
                                                "channelSetting.quickAction.botEventDescription",
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_3()
                            .pt_4()
                            .child(
                                Button::new("create-quick-menu-cancel")
                                    .label(mezon_i18n::t(
                                        &locale,
                                        "channelSetting.quickAction.cancel",
                                    ))
                                    .link()
                                    .disabled(self.submitting)
                                    .on_click(|_, _, cx| Self::close(cx)),
                            )
                            .child(
                                Button::new("create-quick-menu-submit")
                                    .label(mezon_i18n::t(
                                        &locale,
                                        if self.editing_id.is_some() {
                                            "channelSetting.quickAction.update"
                                        } else {
                                            "channelSetting.quickAction.create"
                                        },
                                    ))
                                    .primary()
                                    .loading(self.submitting)
                                    .disabled(!can_submit)
                                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                            ),
                    ),
            )
    }
}
