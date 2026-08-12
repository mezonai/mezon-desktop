use std::time::Duration;

use gpui::{
    App, ClickEvent, ClipboardItem, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, SharedString, Subscription, Task, Window, deferred, div, prelude::*, px,
};
use mezon_store::{
    AccountStore, BadgeService, Settings, UserPresence, WalletStore, current_user_status,
};

use crate::chat::message::{CustomStatusModal, SendTokenModal, TransactionHistoryModal};
use crate::components::compositions::CustomStatusBubble;
use crate::components::primitives::{Avatar, Icon, IconName};
use crate::theme::ActiveTheme;
use crate::util::user_status::{status_color, status_icon, status_label_key};

use mezon_store::TOKEN_DECIMAL_FACTOR as DECIMAL_FACTOR;
const CURRENCY_SYMBOL: &str = "đồng";
const COPY_USER_ID_RESET_MS: u64 = 1000;

const STATUS_OPTIONS: [UserPresence; 4] = [
    UserPresence::Online,
    UserPresence::Idle,
    UserPresence::Dnd,
    UserPresence::Invisible,
];

const STATUS_DURATIONS: [(&str, i32, bool); 6] = [
    (
        "userProfile.statusProfile.statusDuration.for30Minutes",
        30,
        false,
    ),
    (
        "userProfile.statusProfile.statusDuration.for1Hour",
        60,
        false,
    ),
    (
        "userProfile.statusProfile.statusDuration.for3Hours",
        180,
        false,
    ),
    (
        "userProfile.statusProfile.statusDuration.for8Hours",
        480,
        false,
    ),
    (
        "userProfile.statusProfile.statusDuration.for24Hours",
        1440,
        false,
    ),
    ("userProfile.statusProfile.statusDuration.forever", 0, true),
];

pub struct FooterProfilePopup {
    focus_handle: FocusHandle,
    locale: SharedString,
    user_id: String,
    display_name: SharedString,
    username: SharedString,
    avatar: SharedString,
    balance: SharedString,
    status: UserPresence,
    user_status: String,
    status_menu_open: bool,
    status_duration_for: Option<UserPresence>,
    custom_status_bubble: Entity<CustomStatusBubble>,
    user_id_copied: bool,
    _copy_user_id_reset: Option<Task<()>>,
    _account_sub: Option<Subscription>,
}

impl Focusable for FooterProfilePopup {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for FooterProfilePopup {}

impl FooterProfilePopup {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let locale: SharedString = Settings::try_global(cx)
            .map(|s| s.read(cx).language.clone())
            .unwrap_or_default()
            .into();
        let user_id = BadgeService::try_global(cx)
            .and_then(|b| b.read(cx).current_user_id(cx))
            .map(|id| id.0.to_string())
            .unwrap_or_default();
        let account = AccountStore::try_global(cx).and_then(|a| a.read(cx).account.clone());
        let username = account
            .as_ref()
            .map(|a| a.username.clone())
            .unwrap_or_default();
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
        let avatar_raw = account
            .as_ref()
            .and_then(|a| a.avatar_url.clone())
            .unwrap_or_default();
        let avatar = if avatar_raw.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(crate::util::imgproxy::avatar_url(cx, &avatar_raw))
        };
        let own = current_user_status(cx)
            .map(|(_, status)| status)
            .unwrap_or_default();
        let balance = WalletStore::try_global(cx)
            .and_then(|w| w.read(cx).balance().map(format_balance))
            .unwrap_or_else(|| "0".to_string());
        let account_sub = AccountStore::try_global(cx)
            .map(|store| cx.observe(&store, |this, _, cx| this.sync_status(cx)));

        Self {
            focus_handle: cx.focus_handle(),
            locale,
            user_id,
            display_name: SharedString::from(display_name),
            username: SharedString::from(username),
            avatar,
            balance: SharedString::from(balance),
            status: own.presence,
            user_status: own.custom_status,
            status_menu_open: false,
            status_duration_for: None,
            custom_status_bubble: cx.new(|_| CustomStatusBubble::new()),
            user_id_copied: false,
            _copy_user_id_reset: None,
            _account_sub: account_sub,
        }
    }

    fn sync_status(&mut self, cx: &mut Context<Self>) {
        let own = current_user_status(cx)
            .map(|(_, status)| status)
            .unwrap_or_default();
        if self.status == own.presence && self.user_status == own.custom_status {
            return;
        }
        self.status = own.presence;
        self.user_status = own.custom_status;
        cx.notify();
    }

    fn copy_user_id(&mut self, cx: &mut Context<Self>) {
        if self.user_id.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.user_id.clone()));
        self.user_id_copied = true;
        cx.notify();
        self._copy_user_id_reset.take();
        let executor = cx.background_executor().clone();
        self._copy_user_id_reset = Some(cx.spawn(async move |this, cx| {
            executor
                .timer(Duration::from_millis(COPY_USER_ID_RESET_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.user_id_copied = false;
                this._copy_user_id_reset = None;
                cx.notify();
            });
        }));
    }

    fn apply_status(
        &mut self,
        status: UserPresence,
        minutes: i32,
        until_turn_on: bool,
        cx: &mut Context<Self>,
    ) {
        self.status = status;
        self.status_menu_open = false;
        self.status_duration_for = None;
        if let Some(store) = AccountStore::try_global(cx) {
            store.update(cx, |store, cx| {
                store.set_status(status.as_status().to_string(), minutes, until_turn_on, cx)
            });
        }
        cx.notify();
    }

    fn action_row(
        id: &'static str,
        icon: IconName,
        label: String,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme();
        let icon_color = theme.tokens.text_secondary;
        let text_color = theme.tokens.text_theme_primary;
        let hover_bg = theme.tokens.bg_item_hover;
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_2()
            .py(px(6.))
            .rounded_sm()
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                on_click(this, window, cx);
            }))
            .child(Icon::new(icon).size(px(16.)).text_color(icon_color))
            .child(div().text_sm().text_color(text_color).child(label))
    }
}

impl Render for FooterProfilePopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.locale.clone();
        let tk = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let (
            bg_banner,
            bg_card,
            bg_card_surface,
            bg_box,
            bg_status_menu,
            bubble_bg,
            text_primary,
            text_secondary,
            border,
            hover_bg,
        ) = {
            let theme = cx.theme();
            (
                theme.tokens.bg_button_primary,
                theme.tokens.bg_outside_footer,
                theme.surfaces.outside_footer.ramp(),
                theme.tokens.theme_setting_primary,
                theme.tokens.bg_theme_contexify,
                theme.tokens.bg_surface,
                theme.tokens.text_theme_primary,
                theme.tokens.text_secondary,
                theme.tokens.border_primary,
                theme.tokens.bg_item_hover,
            )
        };
        let current_status_color = status_color(self.status, cx.theme());

        let banner = div().w_full().h(px(105.)).rounded_t(px(8.)).bg(bg_banner);

        let custom_status = self.user_status.clone();
        let has_custom = !custom_status.is_empty();
        let custom_status_bubble = self.custom_status_bubble.clone();
        custom_status_bubble.update(cx, |bubble, cx| {
            bubble.set_content(
                custom_status.clone().into(),
                px(212.),
                bubble_bg,
                border,
                text_primary,
                cx,
            );
        });
        let avatar_row = div().relative().flex().flex_row().mt(px(-50.)).child(
            div()
                .relative()
                .ml(px(16.))
                .p(px(6.))
                .w(px(90.))
                .h(px(90.))
                .rounded_full()
                .bg(bg_card_surface)
                .child(
                    Avatar::new()
                        .name(self.display_name.clone())
                        .src(self.avatar.clone())
                        .size_px(px(78.)),
                )
                .child(
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .right(px(6.))
                        .size(px(18.))
                        .rounded_full()
                        .border_2()
                        .border_color(bg_card)
                        .bg(current_status_color),
                ),
        );

        let custom_status_overlay = div()
            .id("footer-custom-status-wrap")
            .group("footer-custom-status")
            .absolute()
            .left(px(112.))
            .top(px(85.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .on_hover({
                let custom_status_bubble = custom_status_bubble.clone();
                move |hovered: &bool, _window, cx| {
                    custom_status_bubble.update(cx, |bubble, cx| {
                        bubble.set_expanded(*hovered, cx);
                    });
                }
            })
            .child(div().size(px(12.)).rounded_full().bg(bubble_bg).shadow_md())
            .child(
                div()
                    .relative()
                    .max_w(px(212.))
                    .child(custom_status_bubble)
                    .child(
                        div()
                            .absolute()
                            .top(px(-16.))
                            .right(px(4.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .rounded_full()
                            .bg(bubble_bg)
                            .border_1()
                            .border_color(border)
                            .p(px(2.))
                            .shadow_md()
                            .opacity(0.)
                            .group_hover("footer-custom-status", |s| s.opacity(1.))
                            .child(
                                div()
                                    .id("footer-custom-status-edit")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px_1()
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(hover_bg))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        let locale = this.locale.clone();
                                        cx.emit(DismissEvent);
                                        CustomStatusModal::open(locale, window, cx);
                                    }))
                                    .child(
                                        Icon::new(IconName::PenEdit)
                                            .size(px(14.))
                                            .text_color(text_secondary),
                                    ),
                            )
                            .child(
                                div()
                                    .id("footer-custom-status-clear")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px_1()
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(hover_bg))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        this.user_status = String::new();
                                        if let Some(store) = AccountStore::try_global(cx) {
                                            store.update(cx, |store, cx| {
                                                store.set_custom_status(String::new(), 0, true, cx)
                                            });
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        Icon::new(IconName::DeleteMessageRightClick)
                                            .size(px(14.))
                                            .text_color(gpui::rgb(0xdc2626)),
                                    ),
                            ),
                    ),
            );

        let identity = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_primary)
                    .truncate()
                    .child(self.display_name.clone()),
            )
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_secondary)
                    .truncate()
                    .child(format!("@{}", self.username)),
            );

        let balance_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_2()
            .py(px(6.))
            .child(
                Icon::new(IconName::Check)
                    .size(px(16.))
                    .text_color(text_secondary),
            )
            .child(div().text_sm().text_color(text_primary).child(format!(
                "{}: {} {CURRENCY_SYMBOL}",
                tk("userProfile.statusProfile.balance"),
                self.balance
            )));

        let custom_status_key = if self.user_status.is_empty() {
            "userProfile.statusProfile.setCustomStatus"
        } else {
            "userProfile.statusProfile.editCustomStatus"
        };

        let mut status_section = div().relative().flex().flex_col().child(
            div()
                .id("footer-action-status")
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .px_2()
                .py(px(6.))
                .rounded_sm()
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.status_menu_open = !this.status_menu_open;
                    if !this.status_menu_open {
                        this.status_duration_for = None;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(
                            Icon::new(status_icon(self.status))
                                .size(px(16.))
                                .text_color(current_status_color),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(text_primary)
                                .child(tk(status_label_key(self.status))),
                        ),
                )
                .child(
                    Icon::new(IconName::RightIcon)
                        .size(px(14.))
                        .text_color(text_secondary),
                ),
        );
        if self.status_menu_open {
            let mut menu = div()
                .absolute()
                .left_full()
                .bottom_0()
                .ml_2()
                .w(px(210.))
                .flex()
                .flex_col()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(bg_status_menu)
                .shadow_lg()
                .occlude();
            for value in STATUS_OPTIONS {
                let color = status_color(value, cx.theme());
                let has_duration = value != UserPresence::Online;
                let expanded = self.status_duration_for == Some(value);
                let option_id = value.as_status();
                let mut row = div()
                    .id(SharedString::from(format!("footer-status-{option_id}")))
                    .group(SharedString::from(format!("footer-status-{option_id}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_2()
                    .py(px(6.))
                    .text_color(text_primary)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).text_color(text_secondary))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        if has_duration {
                            this.status_duration_for = if this.status_duration_for == Some(value) {
                                None
                            } else {
                                Some(value)
                            };
                            cx.notify();
                        } else {
                            this.apply_status(value, 0, true, cx);
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                Icon::new(status_icon(value))
                                    .size(px(16.))
                                    .text_color(color),
                            )
                            .child(div().text_sm().child(tk(status_label_key(value)))),
                    );
                if has_duration {
                    row = row.child(
                        Icon::new(IconName::RightIcon)
                            .size(px(12.))
                            .text_color(text_primary)
                            .group_hover(
                                SharedString::from(format!("footer-status-{option_id}")),
                                move |s| s.text_color(text_secondary),
                            ),
                    );
                }
                menu = menu.child(row);
                if expanded {
                    let mut durations = div().flex().flex_col().ml_3();
                    for (dur_key, minutes, until) in STATUS_DURATIONS {
                        durations = durations.child(
                            div()
                                .id(SharedString::from(format!(
                                    "footer-status-{option_id}-{minutes}-{until}"
                                )))
                                .flex()
                                .flex_row()
                                .items_center()
                                .px_2()
                                .py(px(5.))
                                .text_color(text_primary)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover_bg).text_color(text_secondary))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.apply_status(value, minutes, until, cx);
                                }))
                                .child(div().text_sm().child(tk(dur_key))),
                        );
                    }
                    menu = menu.child(durations);
                }
            }
            status_section = status_section.child(deferred(menu).with_priority(1));
        }

        let actions = div()
            .flex()
            .flex_col()
            .gap_1()
            .mt_2()
            .child(balance_row)
            .child(Self::action_row(
                "footer-action-transfer",
                IconName::SendMoney,
                tk("userProfile.statusProfile.transferFunds"),
                |this, window, cx| {
                    let locale = this.locale.clone();
                    cx.emit(DismissEvent);
                    SendTokenModal::open(locale, None, window, cx);
                },
                cx,
            ))
            .child(Self::action_row(
                "footer-action-history",
                IconName::History,
                tk("userProfile.statusProfile.historyTransaction.title"),
                |this, window, cx| {
                    let locale = this.locale.clone();
                    cx.emit(DismissEvent);
                    TransactionHistoryModal::open(locale, window, cx);
                },
                cx,
            ))
            .child(Self::action_row(
                "footer-action-custom-status",
                IconName::SmilingFace,
                tk(custom_status_key),
                |this, window, cx| {
                    let locale = this.locale.clone();
                    cx.emit(DismissEvent);
                    CustomStatusModal::open(locale, window, cx);
                },
                cx,
            ))
            .child(status_section)
            .child(div().w_full().my_1().h(px(1.)).bg(border).opacity(0.7))
            .child({
                let user_id_copied = self.user_id_copied;
                let copy_label = tk("userProfile.statusProfile.copyUserId");
                div()
                    .id("footer-action-copy-id")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_2()
                    .py(px(6.))
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.copy_user_id(cx);
                    }))
                    .child(
                        Icon::new(if user_id_copied {
                            IconName::Tick
                        } else {
                            IconName::CopyIcon
                        })
                        .size(px(16.))
                        .text_color(text_secondary),
                    )
                    .child(div().text_sm().text_color(text_primary).child(copy_label))
            });

        div()
            .track_focus(&self.focus_handle)
            .relative()
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.status_duration_for.is_some() {
                    this.status_duration_for = None;
                    cx.notify();
                    return;
                }
                if this.status_menu_open {
                    this.status_menu_open = false;
                    cx.notify();
                    return;
                }
                cx.emit(DismissEvent);
            }))
            .occlude()
            .w(px(340.))
            .flex()
            .flex_col()
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .bg(bg_card_surface)
            .shadow_lg()
            .child(banner)
            .child(avatar_row)
            .child(
                div()
                    .px(px(16.))
                    .when(has_custom, |container| container.mt(px(24.)))
                    .child(
                        div()
                            .w_full()
                            .my(px(16.))
                            .p_2()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(border)
                            .bg(bg_box)
                            .flex()
                            .flex_col()
                            .child(identity)
                            .child(actions),
                    ),
            )
            .when(has_custom, |popup| popup.child(custom_status_overlay))
    }
}

fn format_balance(scaled: &str) -> String {
    let value: i128 = scaled.trim().parse().unwrap_or(0);
    format_thousands(value / DECIMAL_FACTOR)
}

fn format_thousands(n: i128) -> String {
    let digits = n.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    if n < 0 { format!("-{out}") } else { out }
}
