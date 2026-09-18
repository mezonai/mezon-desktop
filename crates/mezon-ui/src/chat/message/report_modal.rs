use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use mezon_store::{FriendEvent, FriendState, FriendStore, MessageId, MessagesStore, UserId};
use std::time::Duration;

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
};
use crate::theme::ActiveTheme;

const REASONS: [&str; 7] = [
    "spam",
    "harassment",
    "hate_speech",
    "violence",
    "inappropriate_content",
    "scam",
    "other",
];

const REASON_KEYS: [&str; 7] = [
    "contextMenu.reportMessageModal.reasons.spam",
    "contextMenu.reportMessageModal.reasons.harassment",
    "contextMenu.reportMessageModal.reasons.hate_speech",
    "contextMenu.reportMessageModal.reasons.violence",
    "contextMenu.reportMessageModal.reasons.inappropriate_content",
    "contextMenu.reportMessageModal.reasons.scam",
    "contextMenu.reportMessageModal.reasons.other",
];

const CUSTOM_REASON_MAX: usize = 64;

/// How long a block waits on `ListFriends` before giving up. The store reports a failed fetch
/// only to the log, so without a deadline a dropped list would leave the card pressed for good.
const FRIEND_LIST_WAIT: Duration = Duration::from_secs(8);

/// The report is a two-step flow, the way the web client runs it: pick a reason, then decide
/// whether to also ignore or block the person you just reported.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Reason,
    Actions,
}

/// Blocking is not instant: it needs the friend list to judge against, and then it needs the
/// server to accept. Until both have happened the card stays pressed rather than claiming
/// success the way an optimistic toast would.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockState {
    Idle,
    /// Asked to block before `ListFriends` landed — decide as soon as it does.
    AwaitingFriends,
    Sending,
}

pub struct ReportMessageModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    message_id: MessageId,
    sender_id: Option<UserId>,
    sender_name: SharedString,
    step: Step,
    submitting: bool,
    block_state: BlockState,
    selected: Option<usize>,
    custom_input: Entity<InputState>,
    _custom_sub: Subscription,
    _friend_sub: Subscription,
}

impl Focusable for ReportMessageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ReportMessageModal {
    pub fn open(
        message_id: MessageId,
        sender_id: Option<UserId>,
        sender_name: SharedString,
        locale: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        let placeholder = mezon_i18n::t(
            &locale,
            "contextMenu.reportMessageModal.customReasonPlaceholder",
        )
        .to_string();
        let view = cx.new(|cx| {
            let custom_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .multi_line(true)
                    .validate(|value, _cx| value.chars().count() <= CUSTOM_REASON_MAX)
            });
            let custom_sub = cx.subscribe(&custom_input, |_this, _input, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
            let friend_sub = cx.subscribe(
                &FriendStore::global(cx),
                |this: &mut Self, _store, event, cx| {
                    match event {
                        // The list just landed (or changed) — settle a block that was waiting on
                        // it rather than leaving the card pressed.
                        FriendEvent::Changed if this.block_state == BlockState::AwaitingFriends => {
                            this.block_state = BlockState::Idle;
                            this.block_user(cx);
                        }
                        FriendEvent::BlockSucceeded if this.block_state == BlockState::Sending => {
                            this.block_state = BlockState::Idle;
                            this.toast_success(
                                "contextMenu.reportMessageModal.userBlockedSuccess",
                                cx,
                            );
                            Self::close(cx);
                        }
                        FriendEvent::BlockFailed if this.block_state == BlockState::Sending => {
                            this.block_state = BlockState::Idle;
                            this.toast_error(
                                "contextMenu.reportMessageModal.userBlockedFailed",
                                cx,
                            );
                            cx.notify();
                        }
                        _ => {}
                    }
                },
            );
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                message_id,
                sender_id,
                sender_name,
                step: Step::Reason,
                submitting: false,
                block_state: BlockState::Idle,
                selected: None,
                custom_input,
                _custom_sub: custom_sub,
                _friend_sub: friend_sub,
            }
        });
        // The block action needs the friend list, which is otherwise only fetched by the pages
        // that show friends — a session that went straight to a clan channel has none, and
        // blocking a real friend would be refused.
        FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn is_submittable(&self, cx: &App) -> bool {
        match self.selected {
            None => false,
            Some(idx) if REASONS[idx] == "other" => {
                !self.custom_input.read(cx).value().trim().is_empty()
            }
            Some(_) => true,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.submitting || !self.is_submittable(cx) {
            return;
        }
        let Some(idx) = self.selected else {
            return;
        };
        let abuse_type = if REASONS[idx] == "other" {
            self.custom_input.read(cx).value().trim().to_string()
        } else {
            REASONS[idx].to_string()
        };
        let message_id = self.message_id;
        let report = MessagesStore::global(cx).update(cx, |store, cx| {
            store.report_message(message_id, abuse_type, cx)
        });
        self.submitting = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let sent = report.await.is_ok();
            let _ = this.update(cx, |this, cx| {
                this.submitting = false;
                // Only a report the server took earns the second step — a refused one leaves the
                // reason on screen so it can be sent again, the way the web client does.
                if sent {
                    this.step = Step::Actions;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toast_success(&self, key: &'static str, cx: &mut App) {
        let message = mezon_i18n::t(&self.locale, key).to_string();
        Shell::global(cx).update(cx, |shell, cx| shell.success(message, cx));
    }

    fn toast_error(&self, key: &'static str, cx: &mut App) {
        let message = mezon_i18n::t(&self.locale, key).to_string();
        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
    }

    /// Ignoring is a local acknowledgement in the web client too — there is no API behind it,
    /// so this stays a confirmation rather than pretending to filter anything.
    fn ignore_user(&mut self, cx: &mut Context<Self>) {
        self.toast_success("contextMenu.reportMessageModal.userMessagesIgnored", cx);
        Self::close(cx);
    }

    fn block_user(&mut self, cx: &mut Context<Self>) {
        if self.block_state != BlockState::Idle {
            return;
        }
        let Some(user_id) = self.sender_id else {
            self.toast_error("contextMenu.reportMessageModal.userBlockedFailed", cx);
            return;
        };
        let friends = FriendStore::global(cx);
        // "Not in the list" and "the list has not arrived" are different answers, and only the
        // first one is a refusal. `open` warmed the fetch; wait for it rather than turning a
        // slow round trip into "you can only block friends".
        if !friends.read(cx).has_loaded() {
            self.block_state = BlockState::AwaitingFriends;
            friends.update(cx, |store, cx| store.ensure_loaded(cx));
            cx.notify();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FRIEND_LIST_WAIT).await;
                let _ = this.update(cx, |this, cx| {
                    if this.block_state == BlockState::AwaitingFriends {
                        this.block_state = BlockState::Idle;
                        this.toast_error("contextMenu.reportMessageModal.userBlockedFailed", cx);
                        cx.notify();
                    }
                });
            })
            .detach();
            return;
        }
        let is_friend = friends
            .read(cx)
            .friend(user_id)
            .is_some_and(|friend| friend.state == FriendState::Friend);
        if !is_friend {
            self.toast_error("contextMenu.reportMessageModal.canOnlyBlockFriends", cx);
            return;
        }
        // `block_friend` is optimistic and reports the outcome through `FriendEvent`; the toast
        // and the close belong to that answer, not to having asked.
        self.block_state = BlockState::Sending;
        friends.update(cx, |store, cx| store.block_friend(user_id, cx));
        cx.notify();
    }
}

impl ReportMessageModal {
    /// Step two: the web client offers to ignore or block the reported person before closing.
    fn render_actions(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let t = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let name = self.sender_name.clone();
        // A block in flight owns the modal until the server answers: closing here would take the
        // subscription that carries the answer with it, leaving the outcome unreported.
        let busy = self.block_state != BlockState::Idle;

        let action_card = |id: &'static str,
                           title: String,
                           description: String,
                           title_color: gpui::Rgba,
                           name: SharedString| {
            div()
                .id(id)
                .flex()
                .flex_col()
                .items_start()
                .w_full()
                .gap_1()
                .p_4()
                .rounded_lg()
                .cursor_pointer()
                .bg(theme.tokens.theme_setting_nav)
                .when(busy, |card| card.opacity(0.6))
                .when(!busy, |card| {
                    card.hover(|s| s.bg(theme.tokens.bg_item_hover))
                })
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(title_color)
                        .child(title),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .text_xs()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(description)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.tokens.text_secondary)
                                .child(name),
                        ),
                )
        };

        let ignore_entity = cx.entity();
        let block_entity = cx.entity();

        div()
            .track_focus(&self.focus_handle)
            .occlude()
            .w(px(480.))
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .overflow_hidden()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(t("contextMenu.reportMessageModal.actionModal.title")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(t("contextMenu.reportMessageModal.actionModal.description")),
            )
            .child(
                action_card(
                    "report-ignore-user",
                    t("contextMenu.reportMessageModal.actionModal.ignoreMessages"),
                    t("contextMenu.reportMessageModal.actionModal.ignoreDescription"),
                    theme.tokens.text_secondary,
                    name.clone(),
                )
                .when(!busy, |card| {
                    card.on_click(move |_: &ClickEvent, _window, cx| {
                        ignore_entity.update(cx, |this, cx| this.ignore_user(cx));
                    })
                }),
            )
            .child(
                action_card(
                    "report-block-user",
                    t("contextMenu.reportMessageModal.actionModal.blockUser"),
                    t("contextMenu.reportMessageModal.actionModal.blockDescription"),
                    theme.danger_text,
                    name,
                )
                .when(!busy, |card| {
                    card.on_click(move |_: &ClickEvent, _window, cx| {
                        block_entity.update(cx, |this, cx| this.block_user(cx));
                    })
                }),
            )
            .child(
                div().flex().flex_row().justify_end().child(
                    div()
                        .id("report-skip-actions")
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.tokens.text_theme_primary)
                        .when(busy, |skip| skip.opacity(0.6))
                        .when(!busy, |skip| {
                            skip.hover(|s| s.bg(theme.tokens.bg_item_hover))
                                .on_click(|_: &ClickEvent, _window, cx| Self::close(cx))
                        })
                        .child(t("contextMenu.reportMessageModal.actionModal.skip")),
                ),
            )
            .into_any_element()
    }
}

impl Render for ReportMessageModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.locale.clone();
        let t = |key: &'static str| mezon_i18n::t(&locale, key).to_string();
        let entity = cx.entity();

        if self.step == Step::Actions {
            return self.render_actions(cx);
        }

        let header = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_6()
            .pb_2()
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.tokens.text_secondary)
                    .child(t("contextMenu.reportMessageModal.title")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(t("contextMenu.reportMessageModal.description")),
            );

        let mut reason_list = div().flex().flex_col().gap_1();
        for (idx, key) in REASON_KEYS.iter().enumerate() {
            let selected = self.selected == Some(idx);
            let ent = entity.clone();
            let label = t(key);
            let mut row = div()
                .id(("report-reason", idx))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .p_3()
                .rounded_lg()
                .cursor_pointer()
                .hover(|s| s.bg(theme.tokens.bg_item_theme_hover));
            if selected {
                row = row.bg(theme.tokens.bg_active_member_channel);
            }
            let text_color = if selected {
                theme.tokens.text_secondary
            } else {
                theme.tokens.text_theme_primary
            };
            let weight = if selected {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            };
            row = row
                .child(
                    div()
                        .text_sm()
                        .font_weight(weight)
                        .text_color(text_color)
                        .child(label),
                )
                .when(selected, |el| {
                    el.child(
                        Icon::new(IconName::Check)
                            .size_4()
                            .text_color(theme.tokens.text_secondary),
                    )
                })
                .on_click(move |_: &ClickEvent, _window, cx| {
                    ent.update(cx, |this, cx| {
                        this.selected = Some(idx);
                        cx.notify();
                    });
                });
            reason_list = reason_list.child(row);
        }

        let show_custom = self.selected.map(|idx| REASONS[idx] == "other") == Some(true);
        let custom_len = self.custom_input.read(cx).value().chars().count();
        let custom_section = show_custom.then(|| {
            div()
                .mt_3()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .rounded_lg()
                        .bg(theme.surfaces.surface)
                        .border_1()
                        .border_color(theme.tokens.border_primary)
                        .px_3()
                        .py_2()
                        .child(
                            Input::new(&self.custom_input)
                                .w_full()
                                .text_sm()
                                .text_color(theme.tokens.text_theme_message),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.tokens.text_theme_primary)
                        .child(format!("{custom_len}/{CUSTOM_REASON_MAX}")),
                )
        });

        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_6()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(t("contextMenu.reportMessageModal.selectReason")),
            )
            .child(reason_list)
            .children(custom_section);

        let submit_entity = entity.clone();
        let submit_label = if self.submitting {
            t("contextMenu.reportMessageModal.submitting")
        } else {
            t("contextMenu.reportMessageModal.submit")
        };
        let submit_disabled = self.submitting || !self.is_submittable(cx);

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap_3()
            .p_4()
            .bg(theme.tokens.theme_setting_nav)
            .child(
                div()
                    .id("report-cancel")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(theme.tokens.text_theme_primary)
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .child(t("contextMenu.reportMessageModal.cancel"))
                    .on_click(|_: &ClickEvent, _window, cx| Self::close(cx)),
            )
            .child(
                Button::new("report-submit")
                    .primary()
                    .label(submit_label)
                    .loading(self.submitting)
                    .disabled(submit_disabled)
                    .on_click(move |_: &ClickEvent, _window, cx| {
                        submit_entity.update(cx, |this, cx| this.submit(cx));
                    }),
            );

        div()
            .track_focus(&self.focus_handle)
            .occlude()
            .w(px(540.))
            .max_h(px(640.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(header)
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .child(footer)
            .into_any_element()
    }
}
