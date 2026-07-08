use std::sync::Arc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ListSizingBehavior, SharedString, Task,
    UniformListScrollHandle, Window, div, img, prelude::*, px, relative, rgba, uniform_list,
};
use mezon_store::{MessageId, MessagesStore, PollAnswerView, PollVoter};

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName};
use crate::image_cache::LruImageCache;
use crate::theme::ActiveTheme;

pub struct PollDetailModal {
    focus_handle: FocusHandle,
    locale: SharedString,
    question: SharedString,
    answers: Vec<PollAnswerView>,
    answer_counts: Vec<i32>,
    total_votes: i32,
    selected_index: usize,
    voters_by_answer: Option<Vec<Arc<[PollVoter]>>>,
    loading: bool,
    voter_scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    _fetch: Task<()>,
}

impl Focusable for PollDetailModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PollDetailModal {
    pub fn open(
        poll_id: i64,
        message_id: MessageId,
        locale: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        let store = MessagesStore::global(cx);
        let Some((question, answers, answer_counts, total_votes)) =
            store.read(cx).active_message(message_id).and_then(|msg| {
                msg.poll.as_ref().map(|poll| {
                    (
                        poll.question.clone(),
                        poll.answers.clone(),
                        poll.answer_counts.clone(),
                        poll.total_votes,
                    )
                })
            })
        else {
            return;
        };

        let fetch_task = store.update(cx, |store, cx| {
            store.fetch_poll_detail(poll_id, message_id, cx)
        });

        let view = cx.new(|cx| {
            let image_cache = crate::image_cache::shared_avatar_cache(cx);
            let fetch = cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
                let result = fetch_task.await;
                let _ = this.update(cx, |modal, cx| {
                    modal.loading = false;
                    if let Ok(detail) = result {
                        modal.answer_counts = detail.answer_counts;
                        modal.total_votes = detail.total_votes;
                        modal.voters_by_answer =
                            Some(detail.voters_by_answer.into_iter().map(Arc::from).collect());
                    }
                    cx.notify();
                });
            });
            Self {
                focus_handle: cx.focus_handle(),
                locale,
                question,
                answers,
                answer_counts,
                total_votes,
                selected_index: 0,
                voters_by_answer: None,
                loading: true,
                voter_scroll: UniformListScrollHandle::new(),
                image_cache,
                _fetch: fetch,
            }
        });

        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for PollDetailModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let total_word = if self.total_votes < 2 {
            "message.poll.vote"
        } else {
            "message.poll.votes"
        };
        let locale = self.locale.clone();
        let total_label = format!(
            "{} {}",
            self.total_votes,
            mezon_i18n::t(&locale, total_word)
        );

        let mut answer_list = div().flex().flex_col().gap_2().w(relative(0.35)).min_w_0();
        for (i, answer) in self.answers.iter().enumerate() {
            let count = self.answer_counts.get(i).copied().unwrap_or(0);
            let selected = i == self.selected_index;
            answer_list = answer_list.child(
                div()
                    .id(SharedString::from(format!("poll-detail-answer-{i}")))
                    .px_3()
                    .py_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(16.))
                    .when(selected, |d| d.bg(theme.tokens.bg_item_hover))
                    .text_color(theme.tokens.text_secondary)
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .on_click(cx.listener(move |modal, _, _, cx| {
                        if modal.selected_index != i {
                            modal.selected_index = i;
                            modal.voter_scroll = UniformListScrollHandle::new();
                        }
                        cx.notify();
                    }))
                    .child(format!("{} ({count})", answer.label)),
            );
        }

        let voters = self
            .voters_by_answer
            .as_ref()
            .and_then(|by_answer| by_answer.get(self.selected_index));
        let voter_panel = if self.loading {
            div()
                .text_sm()
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(&locale, "message.poll.loadingVoterDetails"))
                .into_any_element()
        } else if voters.map(|v| v.is_empty()).unwrap_or(true) {
            div()
                .text_sm()
                .text_color(theme.tokens.text_theme_primary)
                .child(mezon_i18n::t(&locale, "message.poll.noVoterDetails"))
                .into_any_element()
        } else {
            let rows: Arc<[PollVoter]> = voters.cloned().unwrap_or_default();
            let count = rows.len();
            uniform_list("poll-detail-voters", count, move |range, _window, cx| {
                let theme = cx.theme().clone();
                range
                    .map(|ix| match rows.get(ix) {
                        Some(voter) => render_voter(voter, &theme),
                        None => div().into_any_element(),
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.voter_scroll)
            .with_sizing_behavior(ListSizingBehavior::Infer)
            .size_full()
            .into_any_element()
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .p_4()
            .border_b_1()
            .border_color(theme.tokens.border_theme_primary)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.tokens.text_secondary)
                            .child(self.question.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.tokens.text_theme_primary)
                            .child(total_label),
                    ),
            )
            .child(
                div()
                    .id("poll-detail-close")
                    .size_6()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.tokens.bg_item_hover))
                    .on_click(|_, _, cx| Self::close(cx))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(20.))
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            );

        let body = div()
            .flex()
            .flex_row()
            .gap_4()
            .p_4()
            .flex_1()
            .min_h_0()
            .child(answer_list)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(voter_panel),
            );

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00_00_00_99))
            .child(
                div()
                    .occlude()
                    .image_cache(self.image_cache.clone())
                    .min_w(px(600.))
                    .max_w(px(620.))
                    .max_h(relative(0.85))
                    .flex()
                    .flex_col()
                    .rounded(px(8.))
                    .bg(theme.tokens.theme_setting_primary)
                    .child(header)
                    .child(body),
            )
    }
}

fn render_voter(voter: &PollVoter, theme: &crate::theme::Theme) -> gpui::AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            img(voter.avatar_proxied.clone())
                .size(px(40.))
                .rounded_full()
                .flex_shrink_0(),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.tokens.text_secondary)
                        .child(voter.display_name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(voter.username.clone()),
                ),
        )
        .into_any_element()
}
