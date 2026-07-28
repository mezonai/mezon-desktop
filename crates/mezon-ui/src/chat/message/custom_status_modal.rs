use gpui::{
    App, ClickEvent, Context, FocusHandle, Focusable, FontWeight, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use mezon_store::AccountStore;

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
};
use crate::theme::ActiveTheme;

const MAX_CUSTOM_STATUS_LEN: usize = 128;

#[derive(Clone, Copy)]
struct ClearOption {
    key: &'static str,
    minutes: i32,
    no_clear: bool,
}

const CLEAR_OPTIONS: [ClearOption; 5] = [
    ClearOption {
        key: "userProfile.statusProfile.customStatusModal.timeOptions.today",
        minutes: 0,
        no_clear: false,
    },
    ClearOption {
        key: "userProfile.statusProfile.customStatusModal.timeOptions.fourHours",
        minutes: 240,
        no_clear: false,
    },
    ClearOption {
        key: "userProfile.statusProfile.customStatusModal.timeOptions.oneHour",
        minutes: 60,
        no_clear: false,
    },
    ClearOption {
        key: "userProfile.statusProfile.customStatusModal.timeOptions.thirtyMinutes",
        minutes: 30,
        no_clear: false,
    },
    ClearOption {
        key: "userProfile.statusProfile.customStatusModal.timeOptions.dontClear",
        minutes: 0,
        no_clear: true,
    },
];

pub struct CustomStatusModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    display_name: SharedString,
    input: gpui::Entity<InputState>,
    selected: usize,
    menu_open: bool,
    _input_sub: Subscription,
}

impl Focusable for CustomStatusModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CustomStatusModal {
    pub fn open(locale: SharedString, window: &mut Window, cx: &mut App) {
        if Shell::global(cx).read(cx).has_modal() {
            return;
        }
        let account = AccountStore::try_global(cx).and_then(|a| a.read(cx).account.clone());
        let display_name = account
            .as_ref()
            .map(|a| {
                if a.display_name.is_empty() {
                    a.username.clone()
                } else {
                    a.display_name.clone()
                }
            })
            .unwrap_or_default();
        let current = account.map(|a| a.user_status).unwrap_or_default();

        let view = cx.new(|cx| {
            let placeholder = mezon_i18n::t(
                &locale,
                "userProfile.statusProfile.customStatusModal.placeholder",
            )
            .to_string();
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            if !current.is_empty() {
                input.update(cx, |state, cx| state.set_value(current.clone(), window, cx));
            }
            let input_sub = cx.subscribe_in(
                &input,
                window,
                |_this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let count = input.read(cx).value().chars().count();
                        if count > MAX_CUSTOM_STATUS_LEN {
                            let truncated: String = input
                                .read(cx)
                                .value()
                                .chars()
                                .take(MAX_CUSTOM_STATUS_LEN)
                                .collect();
                            let input = input.clone();
                            window.defer(cx, move |window, cx| {
                                input
                                    .update(cx, |state, cx| state.set_value(truncated, window, cx));
                            });
                        }
                    }
                },
            );
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                display_name: SharedString::from(display_name),
                input,
                selected: 0,
                menu_open: false,
                _input_sub: input_sub,
            }
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let text: String = self
            .input
            .read(cx)
            .value()
            .trim()
            .chars()
            .take(MAX_CUSTOM_STATUS_LEN)
            .collect();
        let option = CLEAR_OPTIONS[self.selected];
        let minutes = if option.no_clear {
            0
        } else if option.minutes == 0 {
            minutes_until_end_of_day()
        } else {
            option.minutes
        };
        if let Some(store) = AccountStore::try_global(cx) {
            store.update(cx, |store, cx| {
                store.set_custom_status(text, minutes, option.no_clear, cx)
            });
        }
        Self::close(cx);
    }
}

impl Render for CustomStatusModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let tk = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let entity = cx.entity();

        let text_primary = theme.tokens.text_theme_primary;
        let text_secondary = theme.tokens.text_secondary;
        let surface = theme.tokens.theme_setting_primary;
        let input_bg = theme.tokens.bg_input_secondary;
        let border = theme.tokens.border_primary;
        let hover_bg = theme.tokens.bg_item_hover;

        let whats_cookin = tk("userProfile.statusProfile.customStatusModal.whatsCookin")
            .replace("{{name}}", &self.display_name);
        let selected_label = tk(CLEAR_OPTIONS[self.selected].key);

        let mut dropdown = div()
            .id("custom-status-clear")
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("custom-status-clear-trigger")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(36.))
                    .px_3()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(input_bg)
                    .text_color(text_primary)
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.menu_open = !this.menu_open;
                        cx.notify();
                    }))
                    .child(div().text_sm().child(selected_label))
                    .child(
                        Icon::new(IconName::ArrowDown)
                            .size(px(16.))
                            .text_color(text_secondary),
                    ),
            );
        if self.menu_open {
            let mut menu = div()
                .absolute()
                .top_full()
                .left_0()
                .mt_1()
                .w(px(200.))
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .bg(surface)
                .shadow_lg()
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _window, cx| {
                    if this.menu_open {
                        this.menu_open = false;
                        cx.notify();
                    }
                }));
            for (index, option) in CLEAR_OPTIONS.iter().enumerate() {
                let label = tk(option.key);
                let is_selected = index == self.selected;
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("custom-status-clear-{index}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .h(px(36.))
                        .px_3()
                        .cursor_pointer()
                        .text_color(text_primary)
                        .hover(move |s| s.bg(hover_bg))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.selected = index;
                            this.menu_open = false;
                            cx.notify();
                        }))
                        .child(div().text_sm().child(label))
                        .when(is_selected, |d| {
                            d.child(
                                Icon::new(IconName::Check)
                                    .size(px(16.))
                                    .text_color(text_secondary),
                            )
                        }),
                );
            }
            dropdown = dropdown.child(menu);
        }

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.menu_open {
                    this.menu_open = false;
                    cx.notify();
                    return;
                }
                Self::close(cx);
            }))
            .occlude()
            .w(px(440.))
            .flex()
            .flex_col()
            .gap_5()
            .pt_4()
            .rounded(px(8.))
            .bg(surface)
            .shadow_lg()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_primary)
                    .text_center()
                    .child(tk("userProfile.statusProfile.customStatusModal.title")),
            )
            .child(
                div()
                    .px_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_secondary)
                            .child(whats_cookin),
                    )
                    .child(Input::new(&self.input).w_full().text_color(text_primary)),
            )
            .child(
                div()
                    .px_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_secondary)
                            .child(tk("userProfile.statusProfile.customStatusModal.clearAfter")),
                    )
                    .child(dropdown),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .p_4()
                    .child(
                        Button::new("custom-status-cancel")
                            .label(tk(
                                "userProfile.statusProfile.customStatusModal.buttons.cancel",
                            ))
                            .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
                    )
                    .child(
                        Button::new("custom-status-save")
                            .primary()
                            .label(tk(
                                "userProfile.statusProfile.customStatusModal.buttons.save",
                            ))
                            .on_click(move |_: &ClickEvent, _window, cx| {
                                entity.update(cx, |this, cx| this.save(cx));
                            }),
                    ),
            )
    }
}

fn minutes_until_end_of_day() -> i32 {
    let now = chrono::Local::now();
    let end = now
        .date_naive()
        .and_hms_opt(23, 59, 59)
        .and_then(|naive| naive.and_local_timezone(chrono::Local).single());
    match end {
        Some(end) => (end - now).num_minutes().max(0) as i32,
        None => 0,
    }
}
