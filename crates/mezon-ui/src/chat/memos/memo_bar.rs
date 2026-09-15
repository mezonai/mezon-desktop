use gpui::{App, ClickEvent, Hsla, Point, ScrollHandle, SharedString, Window, div, prelude::*, px};
use mezon_store::{
    AccountStore, FriendStore, MemoCreatorGroup, MemoStore, UserId, first_unread_memo_index,
    group_has_unread,
};

use super::{CreateMemoModal, MemoViewer};
use crate::components::primitives::{Avatar, Icon, IconName};
use crate::theme::Theme;
use crate::util::imgproxy;

const MEMO_AVATAR_SIZE: f32 = 52.;
const MEMO_RING_WIDTH_READ: f32 = 2.;
const MEMO_RING_WIDTH_UNREAD: f32 = 3.;
const MEMO_ITEM_WIDTH: f32 = 72.;
const MEMO_BAR_HEIGHT: f32 = 96.;
const MEMO_BAR_GAP: f32 = 8.;

struct CreatorDisplay {
    label: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
}

fn creator_display(creator_id: UserId, cx: &App) -> CreatorDisplay {
    if let Some(account) = AccountStore::global(cx).read(cx).account.as_ref()
        && UserId(account.user_id) == creator_id
    {
        let label = if account.display_name.is_empty() {
            account.username.clone()
        } else {
            account.display_name.clone()
        };
        let raw = account.avatar_url.as_deref().unwrap_or("");
        return CreatorDisplay {
            label: label.into(),
            avatar_src: imgproxy::avatar_url(cx, raw).into(),
            avatar_raw: raw.to_string().into(),
        };
    }
    if let Some(friend) = FriendStore::global(cx)
        .read(cx)
        .friends()
        .iter()
        .find(|friend| friend.id == creator_id)
    {
        return CreatorDisplay {
            label: friend.label().into(),
            avatar_src: imgproxy::avatar_url(cx, &friend.avatar_url).into(),
            avatar_raw: friend.avatar_url.clone().into(),
        };
    }
    CreatorDisplay {
        label: SharedString::from(creator_id.0.to_string()),
        avatar_src: SharedString::default(),
        avatar_raw: SharedString::default(),
    }
}

fn ring_color(theme: &Theme, unread: bool) -> Hsla {
    if unread {
        Hsla::from(theme.brand)
    } else {
        Hsla::from(theme.status_offline)
    }
}

fn scroll_memo_bar(scroll: &ScrollHandle, forward: bool) {
    let step = px(MEMO_ITEM_WIDTH + MEMO_BAR_GAP);
    let offset = scroll.offset();
    let max = scroll.max_offset();
    let next_x = if forward {
        (offset.x - step).max(-max.x)
    } else {
        (offset.x + step).min(px(0.))
    };
    scroll.set_offset(Point::new(next_x, offset.y));
}

fn memo_bar_overflow(scroll: &ScrollHandle) -> (bool, bool, bool) {
    let max = scroll.max_offset();
    let has_overflow = max.x > px(0.);
    if !has_overflow {
        return (false, false, false);
    }
    let offset = scroll.offset();
    let can_left = offset.x < px(0.);
    let can_right = offset.x > -max.x;
    (true, can_left, can_right)
}

fn render_scroll_arrow(
    id: &'static str,
    icon: IconName,
    enabled: bool,
    theme: &Theme,
    scroll: ScrollHandle,
    forward: bool,
    after_scroll: impl Fn(&mut App) + Clone + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .absolute()
        .top(px(28.))
        .when(forward, |element| element.right(px(0.)))
        .when(!forward, |element| element.left(px(0.)))
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded_full()
        .bg(theme.bg_floating)
        .border_1()
        .border_color(theme.border)
        .when(enabled, |element| {
            element
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg_hover))
                .on_click(move |_: &ClickEvent, _, cx| {
                    scroll_memo_bar(&scroll, forward);
                    after_scroll(cx);
                })
        })
        .when(!enabled, |element| element.opacity(0.35))
        .child(
            Icon::new(icon)
                .size(px(16.))
                .text_color(theme.text_secondary),
        )
}

pub fn render_memo_bar(
    scroll: &ScrollHandle,
    locale: &str,
    theme: &Theme,
    _window: &mut Window,
    after_scroll: impl Fn(&mut App) + Clone + 'static,
    cx: &mut App,
) -> impl IntoElement {
    let groups = MemoStore::global(cx).read(cx).groups().to_vec();
    let creating = MemoStore::global(cx).read(cx).is_creating();
    let your_story = mezon_i18n::t(locale, "memos.yourStory");
    let (has_overflow, can_scroll_left, can_scroll_right) = memo_bar_overflow(scroll);
    let scroll_for_left = scroll.clone();
    let scroll_for_right = scroll.clone();
    let after_scroll_left = after_scroll.clone();
    let after_scroll_right = after_scroll;

    div()
        .id("memo-bar")
        .relative()
        .w_full()
        .h(px(MEMO_BAR_HEIGHT))
        .mb_4()
        .when(has_overflow && can_scroll_left, |element| {
            element.child(render_scroll_arrow(
                "memo-bar-scroll-left",
                IconName::ArrowLeft,
                can_scroll_left,
                theme,
                scroll_for_left,
                false,
                after_scroll_left,
            ))
        })
        .when(has_overflow && can_scroll_right, |element| {
            element.child(render_scroll_arrow(
                "memo-bar-scroll-right",
                IconName::RightIcon,
                can_scroll_right,
                theme,
                scroll_for_right,
                true,
                after_scroll_right,
            ))
        })
        .child(
            div()
                .id("memo-bar-scroll")
                .w_full()
                .h_full()
                .overflow_x_scroll()
                .track_scroll(scroll)
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .child(render_create_tile(&your_story, theme, creating, locale, cx))
                .children(
                    groups
                        .into_iter()
                        .map(|group| render_creator_tile(group, theme, locale, cx)),
                ),
        )
}

fn render_create_tile(
    label: &str,
    theme: &Theme,
    creating: bool,
    locale: &str,
    _cx: &mut App,
) -> impl IntoElement {
    let locale = locale.to_string();
    div()
        .id("memo-create-tile")
        .w(px(MEMO_ITEM_WIDTH))
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .opacity(if creating { 0.5 } else { 1.0 })
        .on_click({
            let locale = locale.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                if creating {
                    return;
                }
                CreateMemoModal::open(locale.clone().into(), window, cx);
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(MEMO_AVATAR_SIZE + MEMO_RING_WIDTH_UNREAD * 2.))
                .rounded_full()
                .border_2()
                .border_dashed()
                .border_color(theme.tokens.border_primary)
                .child(
                    Icon::new(IconName::Plus)
                        .size(px(20.))
                        .text_color(theme.text_secondary),
                ),
        )
        .child(
            div()
                .w_full()
                .text_xs()
                .text_center()
                .truncate()
                .text_color(theme.text_secondary)
                .child(label.to_string()),
        )
}

fn render_creator_tile(
    group: MemoCreatorGroup,
    theme: &Theme,
    locale: &str,
    cx: &App,
) -> impl IntoElement {
    let display = creator_display(group.creator_id, cx);
    let unread = group_has_unread(&group);
    let ring = ring_color(theme, unread);
    let ring_width = if unread {
        MEMO_RING_WIDTH_UNREAD
    } else {
        MEMO_RING_WIDTH_READ
    };
    let start_index = first_unread_memo_index(&group);
    let creator_id = group.creator_id;
    let locale = locale.to_string();
    let label = display.label.clone();

    div()
        .id(SharedString::from(format!("memo-creator-{}", creator_id.0)))
        .w(px(MEMO_ITEM_WIDTH))
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .on_click(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            MemoViewer::open(creator_id, start_index, locale.clone().into(), window, cx);
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .p(px(ring_width))
                .rounded_full()
                .bg(ring)
                .child({
                    let mut avatar = Avatar::new()
                        .name(display.label.clone())
                        .size_px(px(MEMO_AVATAR_SIZE));
                    if !display.avatar_src.is_empty() {
                        avatar = avatar.src(display.avatar_src.clone());
                        if !display.avatar_raw.is_empty()
                            && display.avatar_raw != display.avatar_src
                        {
                            avatar = avatar.fallback_src(display.avatar_raw.clone());
                        }
                    } else if !display.avatar_raw.is_empty() {
                        avatar = avatar.src(display.avatar_raw.clone());
                    }
                    avatar
                }),
        )
        .child(
            div()
                .w_full()
                .text_xs()
                .text_center()
                .truncate()
                .text_color(theme.text_secondary)
                .child(label),
        )
}
