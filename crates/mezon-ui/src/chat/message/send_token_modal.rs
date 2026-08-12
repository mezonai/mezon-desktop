use std::collections::HashSet;

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton,
    SharedString, Subscription, Window, deferred, div, prelude::*, px, relative,
};
use mezon_store::{
    AccountStore, BadgeService, DirectMessageStore, FriendStore, UserId, UsersByUserStore,
    WalletStore,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
};
use crate::theme::ActiveTheme;

use mezon_store::TOKEN_DECIMAL_FACTOR as DECIMAL_FACTOR;
const MAX_CANDIDATES_SHOWN: usize = 50;
const MAX_AMOUNT_DIGITS: usize = 15;

#[derive(Clone)]
struct Candidate {
    id: String,
    username: SharedString,
    avatar: SharedString,
    filter_key: String,
}

pub struct SendTokenModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    search: Entity<InputState>,
    amount: Entity<InputState>,
    note: Entity<InputState>,
    candidates: Vec<Candidate>,
    visible: Vec<Candidate>,
    selected: Option<(String, SharedString)>,
    dropdown_open: bool,
    error: Option<SharedString>,
    sending: bool,
    suppress_search_change: bool,
    amount_reformat_queued: bool,
    _search_sub: Subscription,
    _amount_sub: Subscription,
}

impl Focusable for SendTokenModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SendTokenModal {
    pub fn open(
        locale: SharedString,
        prefill: Option<(String, String)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if Shell::global(cx).read(cx).has_modal() {
            return;
        }
        if let Some(wallet) = WalletStore::try_global(cx)
            && !wallet.read(cx).is_available()
        {
            wallet.update(cx, |wallet, cx| {
                wallet.enable_wallet_for_current_user(false, cx)
            });
        }
        let candidates = Self::build_candidates(cx);
        let view = cx.new(|cx| {
            let tr = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
            let search = cx.new(|cx| {
                InputState::new(window, cx).placeholder(tr(
                    "userProfile.statusProfile.sendTokenModal.placeholders.searchUsers",
                ))
            });
            let amount = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(tr(
                        "userProfile.statusProfile.sendTokenModal.placeholders.amountPlaceholder",
                    ))
                    .validate(|candidate, _| digit_count(candidate) <= MAX_AMOUNT_DIGITS)
            });
            let note = cx.new(|cx| {
                InputState::new(window, cx).placeholder(tr(
                    "userProfile.statusProfile.sendTokenModal.placeholders.notePlaceholder",
                ))
            });
            let default_note = tr("common.transferFunds");
            let search_sub = cx.subscribe(
                &search,
                |this: &mut Self, _input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        if this.suppress_search_change {
                            this.suppress_search_change = false;
                        } else {
                            this.selected = None;
                            this.dropdown_open = true;
                        }
                        this.recompute_visible(cx);
                        cx.notify();
                    }
                },
            );
            let amount_sub = cx.subscribe_in(
                &amount,
                window,
                |this: &mut Self, input, event: &InputEvent, window, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    this.error = None;
                    let raw = input.read(cx).value();
                    let needs_reformat = format_amount_input(raw, &this.locale) != raw;
                    if needs_reformat && !this.amount_reformat_queued {
                        this.amount_reformat_queued = true;
                        let input = input.clone();
                        let locale = this.locale.clone();
                        let view = cx.entity();
                        window.defer(cx, move |window, cx| {
                            view.update(cx, |this, _| this.amount_reformat_queued = false);
                            input.update(cx, |input, cx| {
                                let current = input.value().to_string();
                                let formatted = format_amount_input(&current, &locale);
                                if formatted != current {
                                    input.set_value(formatted, window, cx);
                                }
                            });
                        });
                    }
                    cx.notify();
                },
            );

            let mut this = Self {
                focus_handle: cx.focus_handle(),
                locale,
                search,
                amount,
                note,
                candidates,
                visible: Vec::new(),
                selected: None,
                dropdown_open: false,
                error: None,
                sending: false,
                suppress_search_change: false,
                amount_reformat_queued: false,
                _search_sub: search_sub,
                _amount_sub: amount_sub,
            };
            this.note
                .update(cx, |input, cx| input.set_value(default_note, window, cx));
            this.recompute_visible(cx);
            if let Some((id, username)) = prefill {
                this.select_recipient(id, username.into(), window, cx);
            }
            this
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn build_candidates(cx: &App) -> Vec<Candidate> {
        let me = BadgeService::try_global(cx).and_then(|b| b.read(cx).current_user_id(cx));
        let me_id = me.map(|id| id.0.to_string());
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Candidate> = Vec::new();

        if let Some(store) = FriendStore::try_global(cx) {
            for friend in store.read(cx).friends() {
                let id = friend.id.0.to_string();
                Self::push_candidate(
                    &mut out,
                    &mut seen,
                    me_id.as_deref(),
                    cx,
                    id,
                    &friend.username,
                    &friend.avatar_url,
                );
            }
        }
        if let Some(store) = UsersByUserStore::try_global(cx) {
            for user in store.read(cx).users() {
                let id = user.id.0.to_string();
                Self::push_candidate(
                    &mut out,
                    &mut seen,
                    me_id.as_deref(),
                    cx,
                    id,
                    &user.username,
                    &user.avatar_url,
                );
            }
        }
        out
    }

    fn push_candidate(
        out: &mut Vec<Candidate>,
        seen: &mut HashSet<String>,
        me_id: Option<&str>,
        cx: &App,
        id: String,
        username: &str,
        avatar_url: &str,
    ) {
        if id.is_empty() || id == "0" || Some(id.as_str()) == me_id || username.is_empty() {
            return;
        }
        if !seen.insert(id.clone()) {
            return;
        }
        let avatar = if avatar_url.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(crate::util::imgproxy::avatar_url(cx, avatar_url))
        };
        out.push(Candidate {
            id,
            username: SharedString::from(username.to_string()),
            avatar,
            filter_key: username.to_lowercase(),
        });
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn select_recipient(
        &mut self,
        id: String,
        username: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.suppress_search_change = true;
        self.search.update(cx, |input, cx| {
            input.set_value(username.clone(), window, cx)
        });
        self.selected = Some((id, username));
        self.dropdown_open = false;
        cx.notify();
    }

    fn recompute_visible(&mut self, cx: &App) {
        let needle = self.search.read(cx).value().trim().to_lowercase();
        let visible: Vec<Candidate> = self
            .candidates
            .iter()
            .filter(|candidate| needle.is_empty() || candidate.filter_key.contains(&needle))
            .take(MAX_CANDIDATES_SHOWN)
            .cloned()
            .collect();
        self.visible = visible;
    }

    fn amount_value(&self, cx: &App) -> i64 {
        parse_whole_token_amount(self.amount.read(cx).value())
    }

    fn exceeds_balance_for(&self, amount: i64, cx: &App) -> bool {
        if amount <= 0 {
            return false;
        }
        let Some(wallet) = WalletStore::try_global(cx) else {
            return false;
        };
        let Some(balance) = wallet.read(cx).balance() else {
            return false;
        };
        amount_exceeds_balance(amount, balance)
    }

    fn can_send_with(&self, amount: i64, exceeds: bool) -> bool {
        !self.sending && self.selected.is_some() && amount > 0 && !exceeds
    }

    fn can_send(&self, cx: &App) -> bool {
        let amount = self.amount_value(cx);
        self.can_send_with(amount, self.exceeds_balance_for(amount, cx))
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_send(cx) {
            return;
        }
        let wallet_available = WalletStore::try_global(cx)
            .map(|wallet| wallet.read(cx).is_available())
            .unwrap_or(false);
        if !wallet_available {
            let message = mezon_i18n::t(&self.locale, "message.wallet.notAvailable").to_string();
            let locale = self.locale.clone();
            Shell::global(cx).update(cx, |shell, cx| {
                shell.show_wallet_not_available(message, &locale, window, cx)
            });
            return;
        }
        let Some((recipient, recipient_username)) = self.selected.clone() else {
            return;
        };
        let amount = self.amount_value(cx);
        let note = {
            let text = self.note.read(cx).value().trim().to_string();
            (!text.is_empty()).then_some(text)
        };
        let card_note = note
            .clone()
            .unwrap_or_else(|| mezon_i18n::t(&self.locale, "common.transferFunds").to_string());
        let card_text = format!(
            "Funds Transferred: {}₫ | {card_note}",
            format_thousands(amount)
        );
        let recipient_user_id = recipient.parse::<i64>().ok().map(UserId);
        let Some(sender_id) =
            BadgeService::try_global(cx).and_then(|b| b.read(cx).current_user_id(cx))
        else {
            self.error = Some("Wallet is not available".into());
            cx.notify();
            return;
        };
        let sender = sender_id.0.to_string();
        let sender_username = AccountStore::try_global(cx)
            .and_then(|a| {
                a.read(cx)
                    .account
                    .as_ref()
                    .map(|acct| acct.username.clone())
            })
            .unwrap_or_default();

        self.sending = true;
        self.error = None;
        cx.notify();

        let task = WalletStore::global(cx).update(cx, |wallet, cx| {
            wallet.send_token(
                sender,
                sender_username,
                recipient,
                amount,
                note,
                None,
                false,
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.sending = false;
                match result {
                    Ok(_) => {
                        if let Some(user_id) = recipient_user_id {
                            let card = DirectMessageStore::global(cx).update(cx, |store, cx| {
                                store.create_dm_and_send_token_card(
                                    user_id,
                                    recipient_username.to_string(),
                                    card_text.clone(),
                                    cx,
                                )
                            });
                            card.detach();
                        }
                        let message =
                            mezon_i18n::t(&this.locale, "token.toast.success.sendSuccess")
                                .to_string();
                        Shell::global(cx).update(cx, |shell, cx| shell.success(message, cx));
                        Self::close(cx);
                    }
                    Err(error) => {
                        let message: SharedString = error.into();
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message.clone(), cx));
                        this.error = Some(message);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Render for SendTokenModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let entity = cx.entity();
        let locale = self.locale.clone();
        let tk = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let amount = self.amount_value(cx);
        let exceeds = self.exceeds_balance_for(amount, cx);
        let can_send = self.can_send_with(amount, exceeds);

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(tk("userProfile.statusProfile.sendTokenModal.title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.tokens.text_secondary)
                            .child(tk("userProfile.statusProfile.sendTokenModal.description")),
                    ),
            )
            .child(
                div()
                    .id("send-token-close")
                    .size_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(18.))
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            );

        let mut recipient_field = div().relative().child(
            div()
                .id("send-token-search")
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.dropdown_open = true;
                    this.recompute_visible(cx);
                    cx.notify();
                }))
                .child(
                    Input::new(&self.search)
                        .w_full()
                        .text_color(theme.tokens.text_secondary),
                ),
        );
        if self.dropdown_open {
            let mut menu = div()
                .id("send-token-dropdown")
                .absolute()
                .top_full()
                .left_0()
                .right_0()
                .mt_1()
                .flex()
                .flex_col()
                .min_h_0()
                .max_h(px(240.))
                .overflow_y_scroll()
                .rounded_md()
                .border_1()
                .border_color(theme.tokens.border_primary)
                .bg(theme.tokens.theme_setting_primary)
                .shadow_lg()
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.dropdown_open {
                        this.dropdown_open = false;
                        cx.notify();
                    }
                }));
            if self.visible.is_empty() {
                menu = menu.child(
                    div()
                        .px_3()
                        .py_3()
                        .text_sm()
                        .text_color(theme.tokens.text_secondary)
                        .child(tk("userProfile.statusProfile.sendTokenModal.noUsersFound")),
                );
            }
            for candidate in &self.visible {
                let id = candidate.id.clone();
                let username = candidate.username.clone();
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!(
                            "send-token-user-{}",
                            candidate.id
                        )))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.tokens.bg_item_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.select_recipient(id.clone(), username.clone(), window, cx);
                            }),
                        )
                        .child(
                            Avatar::new()
                                .name(candidate.username.clone())
                                .src(candidate.avatar.clone())
                                .size_px(px(28.)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.tokens.text_secondary)
                                .child(candidate.username.clone()),
                        ),
                );
            }
            recipient_field = recipient_field.child(deferred(menu));
        }

        let recipient_section = section(
            theme,
            tk("userProfile.statusProfile.sendTokenModal.fields.to"),
        )
        .child(recipient_field);

        let amount_section = section(
            theme,
            tk("userProfile.statusProfile.sendTokenModal.fields.amount"),
        )
        .child(
            div()
                .relative()
                .child(
                    Input::new(&self.amount)
                        .w_full()
                        .text_color(theme.tokens.text_secondary),
                )
                .child(
                    div()
                        .absolute()
                        .right_3()
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(tk("userProfile.statusProfile.sendTokenModal.currency")),
                ),
        )
        .when(exceeds, |d| {
            d.child(div().text_xs().text_color(gpui::rgb(0xef4444)).child(tk(
                "userProfile.statusProfile.sendTokenModal.errors.exceedWalletBalance",
            )))
        });

        let note_section = section(
            theme,
            tk("userProfile.statusProfile.sendTokenModal.fields.note"),
        )
        .child(
            Input::new(&self.note)
                .w_full()
                .text_color(theme.tokens.text_secondary),
        );

        let mut body = div()
            .flex()
            .flex_col()
            .gap_4()
            .px_4()
            .pb_2()
            .child(recipient_section)
            .child(amount_section)
            .child(note_section);
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(gpui::rgb(0xef4444))
                    .child(error.clone()),
            );
        }

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .p_4()
            .child(
                Button::new("send-token-cancel")
                    .label(tk(
                        "userProfile.statusProfile.sendTokenModal.buttons.cancel",
                    ))
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
            )
            .child(
                Button::new("send-token-submit")
                    .primary()
                    .label(tk(
                        "userProfile.statusProfile.sendTokenModal.buttons.sendTokens",
                    ))
                    .disabled(!can_send)
                    .on_click(move |_: &ClickEvent, window, cx| {
                        entity.update(cx, |this, cx| this.send(window, cx));
                    }),
            );

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.dropdown_open {
                    this.dropdown_open = false;
                    cx.notify();
                    return;
                }
                Self::close(cx);
            }))
            .occlude()
            .w(px(480.))
            .max_h(relative(0.85))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(header)
            .child(
                div()
                    .id("send-token-body-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(footer)
    }
}

fn digit_count(raw: &str) -> usize {
    raw.chars().filter(|c| c.is_ascii_digit()).count()
}

fn parse_whole_token_amount(raw: &str) -> i64 {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return 0;
    }
    significant.parse().unwrap_or(i64::MAX)
}

fn amount_exceeds_balance(amount: i64, balance: &str) -> bool {
    if amount <= 0 {
        return false;
    }
    let scaled = (amount as i128).saturating_mul(DECIMAL_FACTOR);
    let available: i128 = balance.trim().parse().unwrap_or(0);
    scaled > available
}

fn group_separator(locale: &str) -> char {
    if locale.starts_with("vi") { '.' } else { ',' }
}

fn format_amount_input(raw: &str, locale: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    group_digits(parse_whole_token_amount(raw), group_separator(locale))
}

fn group_digits(value: i64, separator: char) -> String {
    let digits = value.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(*byte as char);
    }
    out
}

fn format_thousands(n: i64) -> String {
    let out = group_digits(n, ',');
    if n < 0 { format!("-{out}") } else { out }
}

fn section(theme: &crate::theme::Theme, label: impl Into<SharedString>) -> gpui::Div {
    div().flex().flex_col().gap_2().child(
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.tokens.text_theme_primary)
            .child(label.into()),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DECIMAL_FACTOR, MAX_AMOUNT_DIGITS, amount_exceeds_balance, digit_count,
        format_amount_input, format_thousands, parse_whole_token_amount,
    };

    #[test]
    fn keeps_digits_like_the_react_handler() {
        assert_eq!(parse_whole_token_amount("1000"), 1000);
        assert_eq!(parse_whole_token_amount("12abc34"), 1234);
        assert_eq!(parse_whole_token_amount("-5"), 5);
        assert_eq!(parse_whole_token_amount(""), 0);
        assert_eq!(parse_whole_token_amount("   "), 0);
    }

    #[test]
    fn strips_group_separators_of_both_locales() {
        assert_eq!(parse_whole_token_amount("1,000"), 1000);
        assert_eq!(parse_whole_token_amount("1.000"), 1000);
        assert_eq!(parse_whole_token_amount("12,345,678"), 12_345_678);
        assert_eq!(parse_whole_token_amount("12.345.678"), 12_345_678);
    }

    #[test]
    fn an_overlong_digit_run_saturates_instead_of_reading_as_empty() {
        assert_eq!(parse_whole_token_amount(&"9".repeat(40)), i64::MAX);
        assert_eq!(parse_whole_token_amount(&"2".repeat(20)), i64::MAX);
    }

    #[test]
    fn leading_zeros_do_not_change_the_amount() {
        assert_eq!(parse_whole_token_amount("0"), 0);
        assert_eq!(parse_whole_token_amount("007"), 7);
        assert_eq!(parse_whole_token_amount(&"0".repeat(40)), 0);
    }

    #[test]
    fn the_digit_cap_keeps_the_amount_inside_i64() {
        let at_cap = "9".repeat(MAX_AMOUNT_DIGITS);
        assert_eq!(digit_count(&at_cap), MAX_AMOUNT_DIGITS);
        assert!(parse_whole_token_amount(&at_cap) < i64::MAX);
        assert!(
            (parse_whole_token_amount(&at_cap) as i128).saturating_mul(DECIMAL_FACTOR) < i128::MAX
        );
    }

    #[test]
    fn separators_do_not_count_against_the_digit_cap() {
        assert_eq!(digit_count("222,222,222"), 9);
        assert_eq!(digit_count("222.222.222"), 9);
        assert_eq!(digit_count(""), 0);
    }

    #[test]
    fn formatting_an_already_formatted_amount_changes_nothing() {
        for locale in ["en", "vi"] {
            for raw in [
                "0",
                "1",
                "222",
                "2222",
                "222222222",
                &"2".repeat(MAX_AMOUNT_DIGITS),
            ] {
                let once = format_amount_input(raw, locale);
                let twice = format_amount_input(&once, locale);
                assert_eq!(once, twice, "locale={locale} raw={raw}");
            }
        }
    }

    #[test]
    fn amount_input_groups_by_locale_like_intl_number_format() {
        assert_eq!(format_amount_input("1000", "en"), "1,000");
        assert_eq!(format_amount_input("1000", "vi"), "1.000");
        assert_eq!(format_amount_input("12345678", "vi"), "12.345.678");
        assert_eq!(format_amount_input("999", "en"), "999");
    }

    #[test]
    fn amount_input_reformats_its_own_output_idempotently() {
        for locale in ["en", "vi"] {
            let once = format_amount_input("12345678", locale);
            assert_eq!(format_amount_input(&once, locale), once);
            assert_eq!(parse_whole_token_amount(&once), 12_345_678);
        }
    }

    #[test]
    fn balance_check_scales_the_amount_before_comparing() {
        assert!(!amount_exceeds_balance(1, "1000000"));
        assert!(!amount_exceeds_balance(1, "2000000"));
        assert!(amount_exceeds_balance(2, "1000000"));
        assert!(amount_exceeds_balance(1, "999999"));
    }

    #[test]
    fn balance_check_is_inert_for_non_positive_amounts() {
        assert!(!amount_exceeds_balance(0, "0"));
        assert!(!amount_exceeds_balance(-1, "0"));
    }

    #[test]
    fn unparseable_balance_blocks_the_send() {
        assert!(amount_exceeds_balance(1, "abc"));
        assert!(amount_exceeds_balance(1, ""));
        assert!(!amount_exceeds_balance(1, " 1000000 "));
    }

    #[test]
    fn oversized_amount_saturates_instead_of_overflowing() {
        assert!(amount_exceeds_balance(i64::MAX, "1000000"));
    }

    #[test]
    fn clearing_the_amount_field_stays_empty() {
        assert_eq!(format_amount_input("", "vi"), "");
        assert_eq!(format_amount_input("abc", "vi"), "0");
    }

    #[test]
    fn grouped_output_round_trips_through_the_parser() {
        for value in [1i64, 999, 1000, 1_234_567, i64::MAX / DECIMAL_FACTOR as i64] {
            assert_eq!(parse_whole_token_amount(&format_thousands(value)), value);
        }
    }
}
