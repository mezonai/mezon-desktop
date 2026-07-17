use std::collections::HashSet;

use gpui::{
    Anchor, AnyElement, App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable,
    FontWeight, Rgba, SharedString, Subscription, Window, div, prelude::*, px, rgb,
};
use mezon_store::{
    Message, MessageButton, MessageComponent, MessageId, MessageSelect, MessagesStore,
};
use ui::PopoverMenu;

use super::content::open_message_link;
use super::context::RowCtx;
use crate::chat::user_profile_popover::ClickableContainer;
use crate::components::primitives::{Icon, IconName};
use crate::theme::ActiveTheme;

const BUTTON_TEXT: u32 = 0xff_ff_ff;
const COLOR_SUCCESS: u32 = 0x16_a3_4a;
const COLOR_DANGER: u32 = 0xda_37_3c;
const CHIP_REMOVE: u32 = 0xef_44_44;
const CHIP_REMOVE_HOVER: u32 = 0xb9_1c_1c;
const SELECT_MAX_WIDTH: f32 = 400.0;
const DROPDOWN_MAX_HEIGHT: f32 = 200.0;

pub fn render_actions_panel(msg: &Message, ctx: &RowCtx) -> AnyElement {
    let sender_id = msg.sender_id.parse::<i64>().unwrap_or(0);
    let user_id = ctx.current_user_id.parse::<i64>().unwrap_or(0);
    let mut panel = div().flex().flex_col().w_full();
    let mut index = 0usize;
    for row in msg.components.iter() {
        let mut row_element = div().flex().flex_row().flex_wrap().gap_2().py_2();
        for component in &row.components {
            match component {
                MessageComponent::Button(button) => {
                    row_element = row_element.child(render_message_button(
                        button, msg.id, sender_id, user_id, index, ctx,
                    ));
                }
                MessageComponent::Select(select) => {
                    row_element =
                        row_element.child(render_message_select(select, msg.id, index, ctx));
                }
                MessageComponent::Other => {}
            }
            index += 1;
        }
        panel = panel.child(row_element);
    }
    panel.into_any_element()
}

fn render_message_button(
    button: &MessageButton,
    message_id: MessageId,
    sender_id: i64,
    user_id: i64,
    index: usize,
    ctx: &RowCtx,
) -> AnyElement {
    let has_url = button.url.is_some();
    let icon = if has_url {
        None
    } else {
        button_icon(button.icon.as_deref())
    };
    let element_id = SharedString::from(format!("msg-action-button-{}-{index}", message_id.get()));

    let mut element = div()
        .id(element_id)
        .flex()
        .items_center()
        .px_5()
        .py_1()
        .rounded(px(4.))
        .bg(button_bg(button.style, ctx))
        .text_color(rgb(BUTTON_TEXT))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|s| s.opacity(0.7));

    match button.url.clone() {
        Some(url) => {
            let url = url.to_string();
            element = element.on_click(move |_, _, cx| open_message_link(url.clone(), cx));
        }
        None => {
            if !button.disable
                && let Some(button_id) = button.id.clone()
            {
                element = element.on_click(move |_, _, cx| {
                    MessagesStore::global(cx).update(cx, |store, cx| {
                        store.click_message_button(
                            message_id,
                            button_id.clone(),
                            sender_id,
                            user_id,
                            cx,
                        );
                    });
                });
            }
        }
    }

    if let Some(icon) = icon {
        element = element.child(Icon::new(icon).size_4().text_color(rgb(BUTTON_TEXT)));
    }
    element = element.child(div().child(button.label.clone()));
    if has_url {
        element = element.child(
            Icon::new(IconName::ForwardRightClick)
                .size_4()
                .ml_2()
                .text_color(rgb(BUTTON_TEXT)),
        );
    }
    element.into_any_element()
}

fn render_message_select(
    select: &MessageSelect,
    message_id: MessageId,
    index: usize,
    ctx: &RowCtx,
) -> AnyElement {
    let theme = ctx.theme;
    let placeholder = select.placeholder.clone().unwrap_or_default();
    let note = select_note(select);
    let select_id = select.id.clone();

    let selected_values: Vec<SharedString> = match &select_id {
        Some(id) => MessagesStore::try_global(ctx.app)
            .map(|store| {
                store
                    .read(ctx.app)
                    .message_select_selection(message_id, id)
                    .to_vec()
            })
            .unwrap_or_default(),
        None => Vec::new(),
    };

    let mut left = div().flex().flex_col().min_w_0();
    if let Some(id) = &select_id
        && !selected_values.is_empty()
    {
        let mut chips = div().flex().flex_wrap().gap_2().mb_2();
        for value in &selected_values {
            let label = option_label(select, value);
            let chip_id = SharedString::from(format!(
                "msg-select-chip-{}-{index}-{value}",
                message_id.get()
            ));
            let remove_id = id.clone();
            let remove_value = value.clone();
            chips = chips.child(
                div()
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .child(div().child(label))
                    .child(
                        div()
                            .id(chip_id)
                            .ml_2()
                            .cursor_pointer()
                            .text_color(rgb(CHIP_REMOVE))
                            .hover(|s| s.text_color(rgb(CHIP_REMOVE_HOVER)))
                            .child("✕")
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let select_id = remove_id.clone();
                                let value = remove_value.clone();
                                MessagesStore::global(cx).update(cx, |store, cx| {
                                    let remaining: Vec<SharedString> = store
                                        .message_select_selection(message_id, &select_id)
                                        .iter()
                                        .filter(|v| **v != value)
                                        .cloned()
                                        .collect();
                                    store.set_message_select_selection(
                                        message_id, select_id, remaining, cx,
                                    );
                                });
                            }),
                    ),
            );
        }
        left = left.child(chips);
    }
    left = left.child(
        div()
            .flex()
            .flex_col()
            .items_start()
            .child(
                div()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(placeholder),
            )
            .child(div().text_size(px(12.)).italic().child(note)),
    );

    let box_id = SharedString::from(format!("msg-action-select-{}-{index}", message_id.get()));
    let trigger = ClickableContainer::new(box_id)
        .w_full()
        .max_w(px(SELECT_MAX_WIDTH))
        .flex()
        .flex_row()
        .justify_between()
        .items_center()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(theme.tokens.border_primary)
        .text_size(px(14.))
        .cursor_pointer()
        .child(left.into_any_element())
        .child(
            Icon::new(IconName::ArrowDownFill)
                .size_4()
                .text_color(theme.tokens.text_theme_primary)
                .into_any_element(),
        );

    let Some(select_id) = select_id else {
        return trigger.into_any_element();
    };

    let options: Vec<(SharedString, SharedString)> = select
        .options
        .iter()
        .map(|option| (option.label.clone(), option.value.clone()))
        .collect();
    let min = select.min_options;
    let max = select.max_options;
    let disabled = select.disabled;
    let options_len = select.options.len();
    let popover_id = SharedString::from(format!("msg-select-popover-{}-{index}", message_id.get()));

    PopoverMenu::new(popover_id)
        .menu(move |window, cx| {
            let select_id = select_id.clone();
            let options = options.clone();
            Some(cx.new(|cx| {
                SelectDropdown::new(
                    message_id,
                    select_id,
                    options,
                    min,
                    max,
                    disabled,
                    options_len,
                    window,
                    cx,
                )
            }))
        })
        .anchor(Anchor::TopLeft)
        .attach(Anchor::BottomLeft)
        .trigger(trigger)
        .into_any_element()
}

fn button_bg(style: i32, ctx: &RowCtx) -> Rgba {
    match style {
        3 => rgb(COLOR_SUCCESS),
        4 => rgb(COLOR_DANGER),
        2 | 5 => ctx.theme.tokens.bg_button_secondary,
        _ => ctx.theme.tokens.bg_button_primary,
    }
}

fn button_icon(icon: Option<&str>) -> Option<IconName> {
    match icon {
        Some("PLAY") => Some(IconName::RightFilledTriangle),
        Some("PAUSE") => Some(IconName::PauseIcon),
        _ => None,
    }
}

fn option_label(select: &MessageSelect, value: &SharedString) -> SharedString {
    select
        .options
        .iter()
        .find(|option| &option.value == value)
        .map(|option| option.label.clone())
        .unwrap_or_else(|| value.clone())
}

fn select_note(select: &MessageSelect) -> SharedString {
    let min = select.min_options.filter(|&value| value > 0);
    let max = select.max_options.filter(|&value| value > 0);
    match (min, max) {
        (Some(min), Some(max)) => format!("Select from {min} to {max} options").into(),
        (_, Some(max)) => format!(
            "Select up to {max} option{}",
            if max > 1 { "s" } else { "" }
        )
        .into(),
        (Some(min), _) => format!(
            "Select at least {min} option{}",
            if min > 1 { "s" } else { "" }
        )
        .into(),
        _ => "Select 1 option".into(),
    }
}

struct SelectDropdown {
    focus_handle: FocusHandle,
    message_id: MessageId,
    select_id: SharedString,
    available: Vec<(SharedString, SharedString)>,
    options_len: usize,
    min: Option<i32>,
    max: Option<i32>,
    disabled: bool,
    _blur_sub: Subscription,
}

impl EventEmitter<DismissEvent> for SelectDropdown {}

impl Focusable for SelectDropdown {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SelectDropdown {
    #[allow(clippy::too_many_arguments)]
    fn new(
        message_id: MessageId,
        select_id: SharedString,
        options: Vec<(SharedString, SharedString)>,
        min: Option<i32>,
        max: Option<i32>,
        disabled: bool,
        options_len: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let blur_sub = cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(DismissEvent));
        let selected: HashSet<SharedString> = MessagesStore::try_global(cx)
            .map(|store| {
                store
                    .read(cx)
                    .message_select_selection(message_id, &select_id)
                    .iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let available = options
            .into_iter()
            .filter(|(_, value)| !selected.contains(value))
            .collect();
        Self {
            focus_handle,
            message_id,
            select_id,
            available,
            options_len,
            min,
            max,
            disabled,
            _blur_sub: blur_sub,
        }
    }

    fn choose(&mut self, value: SharedString, cx: &mut Context<Self>) {
        if self.disabled {
            cx.emit(DismissEvent);
            return;
        }
        let store = MessagesStore::global(cx);
        let current = store
            .read(cx)
            .message_select_selection(self.message_id, &self.select_id)
            .to_vec();
        let max_effective = self
            .max
            .filter(|&value| value > 0)
            .map(|value| value as usize)
            .unwrap_or(self.options_len);
        if current.len() >= max_effective {
            cx.emit(DismissEvent);
            return;
        }
        let single = self.min.filter(|&value| value > 0).is_none()
            && self.max.filter(|&value| value > 0).is_none();
        let new_values = if single {
            vec![value]
        } else {
            let mut values = current;
            values.push(value);
            values
        };
        store.update(cx, |store, cx| {
            store.select_message_option(self.message_id, self.select_id.clone(), new_values, cx);
        });
        cx.emit(DismissEvent);
    }
}

impl Render for SelectDropdown {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let panel_id = SharedString::from(format!("msg-select-dropdown-{}", self.message_id.get()));
        let mut panel = div()
            .id(panel_id)
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .flex()
            .flex_col()
            .min_w(px(SELECT_MAX_WIDTH))
            .max_h(px(DROPDOWN_MAX_HEIGHT))
            .overflow_y_scroll()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_tertiary)
            .shadow_lg();
        for (label, value) in &self.available {
            let value = value.clone();
            let item_id = SharedString::from(format!(
                "msg-select-option-{}-{value}",
                self.message_id.get()
            ));
            panel = panel.child(
                div()
                    .id(item_id)
                    .w(px(SELECT_MAX_WIDTH))
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded_sm()
                    .py_2()
                    .px_4()
                    .text_size(px(14.))
                    .cursor_pointer()
                    .text_color(theme.text_secondary)
                    .hover(|s| s.bg(theme.bg_hover))
                    .child(label.clone())
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.choose(value.clone(), cx);
                    })),
            );
        }
        panel
    }
}
