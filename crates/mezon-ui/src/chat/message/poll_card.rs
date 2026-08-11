use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Bounds, FontWeight, ObjectFit, Pixels, SharedString,
    div, img, prelude::*, px, relative, rgb, rgba,
};
use mezon_store::{Message, MessageId, MessagesStore, PollAnswerView, PollData, PollLabelSegment};

use super::content::{
    ATTACHMENT_PLACEHOLDER, SELECTION_BG, SelectableSectionCursor, SelectableTextContext,
};
use super::context::RowCtx;
use super::poll_detail_modal::PollDetailModal;
use super::selection::{SelectableRegion, TextSegment};
use crate::components::primitives::{Icon, IconName};

const BLUE_600: u32 = 0x2563eb;
const BLUE_500: u32 = 0x3b82f6;
const RED_500: u32 = 0xef4444;
const RED_500_10: u32 = 0xef44_4419;
const ANSWERS_SCROLL_AFTER: usize = 5;
const POLL_SCROLL_HANDLE_LIMIT: usize = 64;
const BAR_FILL_MS: u64 = 650;
const PERCENT_POP_MS: u64 = 400;

fn poll_ease_out(delta: f32) -> f32 {
    1.0 - (1.0 - delta).powi(5)
}

fn inner_scroll_has_room(before_offset_y: Pixels, max_offset_y: Pixels, delta_y: Pixels) -> bool {
    if delta_y < px(0.) {
        before_offset_y > -max_offset_y
    } else if delta_y > px(0.) {
        before_offset_y < px(0.)
    } else {
        false
    }
}

pub fn render_poll_card(
    msg: &Message,
    ctx: &RowCtx,
    base: usize,
    selection_context: &SelectableTextContext,
) -> AnyElement {
    let Some(poll) = msg.poll.as_ref() else {
        return div().into_any_element();
    };
    let theme = ctx.theme;
    let now_secs = ctx.now.timestamp();
    let is_expired = poll.is_expired(now_secs);
    let is_closed = poll.is_closed;

    let store = MessagesStore::global(ctx.app).read(ctx.app);
    let ui = store.poll_ui_state(msg.id);
    let selected: &[i32] = ui.map(|s| s.selected.as_slice()).unwrap_or(&[]);
    let show_results = ui.map(|s| s.show_results).unwrap_or(false);
    let voting = ui.map(|s| s.voting).unwrap_or(false);
    let voted: &[i32] = store.poll_my_vote(msg.id).unwrap_or(&[]);
    let animate_results = store.poll_result_animating(msg.id);

    let has_voted = !voted.is_empty();
    let can_select = !has_voted && !show_results && !is_closed && !is_expired;
    let should_show_results = has_voted || show_results || is_closed || is_expired;
    let total_votes = poll.total_votes;
    let msg_id = msg.id;
    let poll_id = poll.poll_id;
    let allow_multiple = poll.allow_multiple;
    let mut selection_cursor = SelectableSectionCursor::new(base);
    let question_range = selection_cursor.section(&poll.question);

    let mut header = div().flex().flex_row().items_center().gap_2().mb_1().child(
        div()
            .cursor(gpui::CursorStyle::IBeam)
            .flex_1()
            .min_w_0()
            .text_size(px(15.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.tokens.text_secondary)
            .when_some(question_range, |div, range| {
                div.child(selection_context.text_node(&poll.question, range))
            }),
    );
    if is_closed || is_expired {
        header = header.child(
            div()
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .rounded(px(4.))
                .bg(rgba(RED_500_10))
                .text_color(rgb(RED_500))
                .child(mezon_i18n::t(ctx.locale, "message.poll.ended")),
        );
    }

    let subtitle_text = if is_closed || is_expired {
        mezon_i18n::t(ctx.locale, "message.poll.finalResults")
    } else if allow_multiple {
        mezon_i18n::t(ctx.locale, "message.poll.selectOneOrMore")
    } else {
        mezon_i18n::t(ctx.locale, "message.poll.selectOne")
    };
    let subtitle = div()
        .text_xs()
        .text_color(theme.tokens.text_theme_primary)
        .mb_3()
        .child(subtitle_text);

    let answer_clip = (poll.answers.len() > ANSWERS_SCROLL_AFTER).then(|| Rc::new(Cell::new(None)));
    let answer_rows: Vec<AnyElement> = poll
        .answers
        .iter()
        .enumerate()
        .map(|(i, answer)| {
            let answer_text = selectable_poll_answer_text(answer);
            let answer_base = selection_cursor
                .section(&answer_text)
                .map(|range| range.start);
            render_answer_row(
                i,
                answer,
                poll,
                ctx,
                msg_id,
                allow_multiple,
                selected,
                voted,
                should_show_results,
                can_select,
                has_voted,
                animate_results,
                answer_base,
                selection_context,
                answer_clip.as_ref(),
            )
        })
        .collect();
    let answers_col = if let Some(clip) = answer_clip {
        let scroll_handle = {
            let mut memo = ctx.row_memo.borrow_mut();
            if memo.poll_scrolls.len() >= POLL_SCROLL_HANDLE_LIMIT
                && !memo.poll_scrolls.contains_key(&msg_id)
            {
                memo.poll_scrolls.clear();
            }
            memo.poll_scrolls.entry(msg_id).or_default().clone()
        };
        let chain_handle = scroll_handle.clone();
        let scroll = div()
            .id(("poll-answers", msg_id.get() as usize))
            .flex()
            .flex_col()
            .gap_2()
            .mb_3()
            .min_h_0()
            .max_h(px(280.))
            .overflow_y_scroll()
            .track_scroll(&scroll_handle)
            .on_scroll_wheel(move |event, window, cx| {
                let delta_y = event.delta.pixel_delta(window.line_height()).y;
                let before_offset_y = chain_handle.offset().y - delta_y;
                if inner_scroll_has_room(before_offset_y, chain_handle.max_offset().y, delta_y) {
                    cx.stop_propagation();
                }
            })
            .pr_1()
            .children(answer_rows)
            .into_any_element();
        SelectableRegion::new(scroll, clip, None).into_any_element()
    } else {
        div()
            .id(("poll-answers", msg_id.get() as usize))
            .flex()
            .flex_col()
            .gap_2()
            .mb_3()
            .children(answer_rows)
            .into_any_element()
    };

    let footer = render_footer(
        poll,
        ctx,
        msg_id,
        poll_id,
        total_votes,
        is_closed,
        is_expired,
        has_voted,
        show_results,
        selected,
        voting,
        now_secs,
    );

    div()
        .w_full()
        .child(
            div()
                .max_w(px(420.))
                .rounded(px(4.))
                .p_3()
                .border_1()
                .border_color(theme.tokens.border_primary)
                .bg(theme.tokens.bg_active_member_channel)
                .child(header)
                .child(subtitle)
                .child(answers_col)
                .child(footer),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_answer_row(
    position: usize,
    answer: &PollAnswerView,
    poll: &PollData,
    ctx: &RowCtx,
    msg_id: MessageId,
    allow_multiple: bool,
    selected: &[i32],
    voted: &[i32],
    should_show_results: bool,
    can_select: bool,
    has_voted: bool,
    animate_results: bool,
    answer_base: Option<usize>,
    selection_context: &SelectableTextContext,
    answer_clip: Option<&Rc<Cell<Option<Bounds<Pixels>>>>>,
) -> AnyElement {
    let theme = ctx.theme;
    let index = position as i32;
    let count = poll.answer_counts.get(position).copied().unwrap_or(0);
    let percentage = poll.percentages.get(position).copied().unwrap_or(0);
    let is_voted = voted.contains(&index);
    let is_selected = selected.contains(&index);
    let can_toggle = !should_show_results;

    let border_color = if should_show_results {
        theme.tokens.border_primary
    } else if is_selected {
        theme.tokens.text_theme_primary
    } else {
        rgba(0x0000_0000)
    };

    let mut row = div()
        .id(("poll-answer", position))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .flex_shrink_0()
        .px_3()
        .py(px(10.))
        .rounded(px(4.))
        .border_1()
        .border_color(border_color)
        .overflow_hidden()
        .bg(theme.tokens.bg_item_hover);

    if can_toggle {
        let selection = ctx.selection.clone();
        row = row.cursor_pointer().on_click(move |_, _, cx| {
            if selection.borrow().has_selection() {
                return;
            }
            MessagesStore::global(cx).update(cx, |store, cx| {
                store.toggle_poll_answer(msg_id, index, allow_multiple, cx);
            });
        });
    }

    if should_show_results {
        let target = percentage as f32 / 100.0;
        let bar = div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .bg(rgb(BLUE_600));
        row = row.child(if animate_results {
            bar.with_animation(
                ("poll-bar", position),
                Animation::new(Duration::from_millis(BAR_FILL_MS)).with_easing(poll_ease_out),
                move |el, delta| el.w(relative(target * delta)).opacity(0.6 + 0.4 * delta),
            )
            .into_any_element()
        } else {
            bar.w(relative(target)).into_any_element()
        });
    }

    row = row.child(
        div()
            .cursor(gpui::CursorStyle::IBeam)
            .relative()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.tokens.text_secondary)
            .child(render_poll_label(
                answer,
                answer_base,
                selection_context,
                ctx,
                answer_clip,
            )),
    );

    let mut right = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .flex_shrink_0()
        .pl_2();
    if should_show_results {
        let vote_word = vote_word(ctx, count);
        let percent_label = div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.tokens.text_secondary)
            .child(format!("{percentage}% {count} {vote_word}"));
        right = right.child(if animate_results {
            percent_label
                .with_animation(
                    ("poll-percent", position),
                    Animation::new(Duration::from_millis(PERCENT_POP_MS))
                        .with_easing(poll_ease_out),
                    move |el, delta| el.opacity(delta),
                )
                .into_any_element()
        } else {
            percent_label.into_any_element()
        });
    }
    if can_select {
        let mut circle = div()
            .size_5()
            .rounded_full()
            .border_2()
            .border_color(theme.tokens.text_theme_primary)
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0();
        if is_selected {
            circle = circle.child(div().w(px(10.)).h(px(10.)).rounded_full().bg(rgb(BLUE_500)));
        }
        right = right.child(circle);
    }
    if has_voted && is_voted {
        right = right.child(
            div()
                .size_5()
                .rounded_full()
                .bg(theme.tokens.text_theme_primary)
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(
                    Icon::new(IconName::Check)
                        .size(px(12.))
                        .text_color(theme.tokens.text_secondary),
                ),
        );
    }

    row.child(right).into_any_element()
}

fn render_poll_label(
    answer: &PollAnswerView,
    base: Option<usize>,
    selection_context: &SelectableTextContext,
    ctx: &RowCtx,
    answer_clip: Option<&Rc<Cell<Option<Bounds<Pixels>>>>>,
) -> AnyElement {
    let mut row = div().flex().flex_row().items_center().overflow_hidden();
    let mut cursor = base.map(SelectableSectionCursor::new);
    for (segment_index, segment) in answer.segments.iter().enumerate() {
        match segment {
            PollLabelSegment::Text(text) => {
                let range = cursor.as_mut().and_then(|cursor| cursor.inline(text));
                row = row.child(div().overflow_hidden().when_some(range, |div, range| {
                    div.child(match answer_clip {
                        Some(clip) => {
                            selection_context.clipped_text_node(text, range, clip.clone())
                        }
                        None => selection_context.text_node(text, range),
                    })
                }));
            }
            PollLabelSegment::Emoji(src) => {
                if src.is_empty() {
                    continue;
                }
                let placeholder = ATTACHMENT_PLACEHOLDER.to_string();
                let range = cursor
                    .as_mut()
                    .and_then(|cursor| cursor.inline(&placeholder));
                let image = img(src.clone())
                    .w(px(20.))
                    .h(px(20.))
                    .object_fit(ObjectFit::Contain)
                    .image_cache(&ctx.icon_cache)
                    .id(("poll-label-emoji-frames", segment_index))
                    .into_any_element();
                if let Some(range) = range {
                    let bounds = Rc::new(Cell::new(None));
                    let mut segment = TextSegment::bounded(range.clone(), bounds.clone());
                    if let Some(clip) = answer_clip {
                        segment = segment.clipped(clip.clone());
                    }
                    selection_context.push_segment(segment);
                    row = row.child(SelectableRegion::new(
                        image,
                        bounds,
                        selection_context
                            .is_selected(&range)
                            .then(|| rgba(SELECTION_BG)),
                    ));
                } else {
                    row = row.child(image);
                }
            }
        }
    }
    row.into_any_element()
}

fn selectable_poll_answer_text(answer: &PollAnswerView) -> String {
    let mut text = String::new();
    for segment in &answer.segments {
        match segment {
            PollLabelSegment::Text(value) => text.push_str(value),
            PollLabelSegment::Emoji(src) if !src.is_empty() => text.push(ATTACHMENT_PLACEHOLDER),
            PollLabelSegment::Emoji(_) => {}
        }
    }
    text
}

pub(crate) fn selectable_poll_text(poll: &PollData) -> String {
    let mut text = poll.question.to_string();
    for answer in &poll.answers {
        let answer = selectable_poll_answer_text(answer);
        if answer.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&answer);
    }
    text
}

#[allow(clippy::too_many_arguments)]
fn render_footer(
    poll: &PollData,
    ctx: &RowCtx,
    msg_id: MessageId,
    poll_id: i64,
    total_votes: i32,
    is_closed: bool,
    is_expired: bool,
    has_voted: bool,
    show_results: bool,
    selected: &[i32],
    voting: bool,
    now_secs: i64,
) -> AnyElement {
    let theme = ctx.theme;
    let total_label = format!("{total_votes} {}", vote_word(ctx, total_votes));
    let modal_settings = ctx.settings.clone();
    let total_selection = ctx.selection.clone();

    let mut left = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_1()
        .min_w_0()
        .text_xs()
        .text_color(theme.tokens.text_theme_primary)
        .child(
            div()
                .id(("poll-total", msg_id.get() as usize))
                .cursor_pointer()
                .hover(|s| s.underline())
                .on_click(move |_, window, cx| {
                    if total_selection.borrow().has_selection() {
                        return;
                    }
                    let locale = SharedString::from(modal_settings.read(cx).language.clone());
                    PollDetailModal::open(poll_id, msg_id, locale, window, cx);
                })
                .child(total_label),
        );
    if !is_closed
        && !is_expired
        && let Some(time_label) = time_remaining_label(poll.expire_at, now_secs, ctx.locale)
    {
        left = left.child(div().child(format!(
            "• {time_label} {}",
            mezon_i18n::t(ctx.locale, "message.poll.left")
        )));
    }

    let mut buttons = div().flex().flex_row().flex_shrink_0().gap_2();
    if !has_voted && !is_closed && !is_expired {
        let selection = ctx.selection.clone();
        let toggle_key = if show_results {
            "message.poll.backToVote"
        } else {
            "message.poll.showResults"
        };
        buttons = buttons.child(
            div()
                .id(("poll-toggle", msg_id.get() as usize))
                .px_1()
                .py(px(6.))
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .rounded(px(4.))
                .border_1()
                .border_color(theme.tokens.border_primary)
                .text_color(theme.tokens.text_theme_primary)
                .cursor_pointer()
                .hover(|s| s.text_color(theme.tokens.text_secondary))
                .on_click(move |_, _, cx| {
                    if !selection.borrow().has_selection() {
                        MessagesStore::global(cx)
                            .update(cx, |store, cx| store.toggle_poll_results(msg_id, cx));
                    }
                })
                .child(mezon_i18n::t(ctx.locale, toggle_key)),
        );
    }
    if !has_voted && !show_results && !is_closed && !is_expired {
        let disabled = selected.is_empty() || voting;
        let mut btn = div()
            .id(("poll-vote", msg_id.get() as usize))
            .px_4()
            .py(px(6.))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .rounded(px(4.))
            .bg(theme.tokens.bg_button_primary)
            .text_color(theme.tokens.text_secondary)
            .child(mezon_i18n::t(ctx.locale, "message.poll.voteButton"));
        if disabled {
            btn = btn.opacity(0.5);
        } else {
            let selection = ctx.selection.clone();
            btn = btn
                .cursor_pointer()
                .hover(|s| s.bg(theme.tokens.bg_button_primary_hover))
                .on_click(move |_, _, cx| {
                    if !selection.borrow().has_selection() {
                        MessagesStore::global(cx)
                            .update(cx, |store, cx| store.submit_poll_vote(poll_id, msg_id, cx));
                    }
                });
        }
        buttons = buttons.child(btn);
    }
    if has_voted && !is_closed && !is_expired {
        let mut btn = div()
            .id(("poll-remove", msg_id.get() as usize))
            .px_4()
            .py(px(6.))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .rounded(px(4.))
            .border_1()
            .border_color(theme.tokens.border_primary)
            .bg(theme.tokens.bg_button_secondary)
            .text_color(theme.tokens.text_theme_primary)
            .child(mezon_i18n::t(ctx.locale, "message.poll.removeVote"));
        if voting {
            btn = btn.opacity(0.5);
        } else {
            let selection = ctx.selection.clone();
            btn = btn
                .cursor_pointer()
                .hover(|s| s.bg(theme.tokens.bg_secondary_button_hover))
                .on_click(move |_, _, cx| {
                    if !selection.borrow().has_selection() {
                        MessagesStore::global(cx)
                            .update(cx, |store, cx| store.remove_poll_vote(poll_id, msg_id, cx));
                    }
                });
        }
        buttons = buttons.child(btn);
    }

    div()
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .gap_2()
        .pt_1()
        .child(left)
        .child(buttons)
        .into_any_element()
}

fn vote_word(ctx: &RowCtx, count: i32) -> &'static str {
    if count < 2 {
        mezon_i18n::t(ctx.locale, "message.poll.vote")
    } else {
        mezon_i18n::t(ctx.locale, "message.poll.votes")
    }
}

fn time_remaining_label(expire_at: Option<i64>, now_secs: i64, locale: &str) -> Option<String> {
    let diff = expire_at? - now_secs;
    if diff <= 0 {
        return None;
    }
    let days = diff / 86_400;
    let hours = (diff % 86_400) / 3_600;
    let minutes = (diff % 3_600) / 60;
    let label = if days > 0 {
        fmt_count(mezon_i18n::t(locale, "message.poll.durationDays"), days)
    } else if hours > 0 {
        fmt_count(mezon_i18n::t(locale, "message.poll.durationHours"), hours)
    } else if minutes > 0 {
        fmt_count(
            mezon_i18n::t(locale, "message.poll.durationMinutes"),
            minutes,
        )
    } else {
        mezon_i18n::t(locale, "message.poll.durationLessThanMinute").to_string()
    };
    Some(label)
}

fn fmt_count(template: &str, count: i64) -> String {
    template.replace("{{count}}", &count.to_string())
}

#[cfg(test)]
mod tests {
    use super::inner_scroll_has_room;
    use gpui::px;

    #[test]
    fn holds_scroll_while_inner_list_can_move_down() {
        assert!(inner_scroll_has_room(px(-100.), px(200.), px(-40.)));
    }

    #[test]
    fn releases_scroll_at_inner_bottom() {
        assert!(!inner_scroll_has_room(px(-200.), px(200.), px(-40.)));
    }

    #[test]
    fn holds_scroll_while_inner_list_can_move_up() {
        assert!(inner_scroll_has_room(px(-100.), px(200.), px(40.)));
    }

    #[test]
    fn releases_scroll_at_inner_top() {
        assert!(!inner_scroll_has_room(px(0.), px(200.), px(40.)));
    }

    #[test]
    fn zero_delta_never_holds() {
        assert!(!inner_scroll_has_room(px(-50.), px(200.), px(0.)));
    }
}
