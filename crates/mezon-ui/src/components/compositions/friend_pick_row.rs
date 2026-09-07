use gpui::{AnyElement, App, ClickEvent, FontWeight, SharedString, div, prelude::*, px, svg};
use mezon_store::{Friend, UserId};

use crate::theme::Theme;
use crate::util::imgproxy;

pub const FRIEND_PICK_ROW_HEIGHT: f32 = 40.;

#[derive(Clone)]
pub struct FriendPickRow {
    pub user_id: UserId,
    pub key: SharedString,
    pub name: SharedString,
    pub username: SharedString,
    pub avatar_src: SharedString,
    pub avatar_raw: SharedString,
    name_lc: String,
    username_lc: String,
}

impl FriendPickRow {
    pub fn from_friend(friend: &Friend, cx: &App) -> Self {
        let avatar_src = if friend.avatar_url.is_empty() {
            String::new()
        } else {
            imgproxy::avatar_url(cx, &friend.avatar_url)
        };
        Self {
            user_id: friend.id,
            key: format!("friend-{}", friend.id).into(),
            name: SharedString::from(friend.label().to_string()),
            username: SharedString::from(friend.username.clone()),
            avatar_src: SharedString::from(avatar_src),
            avatar_raw: SharedString::from(friend.avatar_url.clone()),
            name_lc: friend.label().to_lowercase(),
            username_lc: friend.username.to_lowercase(),
        }
    }

    pub fn matches_lowercase_query(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        self.name_lc.contains(query) || self.username_lc.contains(query)
    }
}

pub fn render_friend_pick_row(
    theme: &Theme,
    row: &FriendPickRow,
    selected: bool,
    on_toggle: impl Fn(UserId, &mut App) + 'static,
) -> AnyElement {
    let mut avatar = crate::components::primitives::Avatar::new()
        .name(row.name.clone())
        .size_px(px(32.));
    if !row.avatar_src.is_empty() {
        avatar = avatar.src(row.avatar_src.clone());
        if !row.avatar_raw.is_empty() && row.avatar_raw != row.avatar_src {
            avatar = avatar.fallback_src(row.avatar_raw.clone());
        }
    } else if !row.avatar_raw.is_empty() {
        avatar = avatar.src(row.avatar_raw.clone());
    }

    let user_id = row.user_id;
    let checkbox_border = if selected {
        theme.brand
    } else {
        theme.interactive_normal
    };

    div()
        .pl(px(12.))
        .pr(px(8.))
        .child(
            div()
                .id(SharedString::from(format!("pick-row-{}", row.key)))
                .h(px(FRIEND_PICK_ROW_HEIGHT))
                .w_full()
                .px(px(8.))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .rounded_lg()
                .cursor_pointer()
                .hover(|s| s.bg(theme.tokens.bg_active_member_channel))
                .on_click(move |_: &ClickEvent, _window, cx| on_toggle(user_id, cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .flex_1()
                        .child(avatar)
                        .child(
                            div()
                                .truncate()
                                .text_size(px(14.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.tokens.text_theme_primary)
                                .child(row.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(14.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_secondary)
                                .child(row.username.clone()),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .size(px(16.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(checkbox_border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(selected, |el| {
                            el.child(
                                svg()
                                    .path("icons/check.svg")
                                    .size(px(12.))
                                    .flex_none()
                                    .text_color(theme.brand),
                            )
                        }),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, username: &str) -> FriendPickRow {
        FriendPickRow {
            user_id: UserId(1),
            key: "friend-1".into(),
            name: name.to_string().into(),
            username: username.to_string().into(),
            avatar_src: SharedString::default(),
            avatar_raw: SharedString::default(),
            name_lc: name.to_lowercase(),
            username_lc: username.to_lowercase(),
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(row("Alice", "alice").matches_lowercase_query(""));
    }

    #[test]
    fn matches_display_name_and_username_case_insensitively() {
        let row = row("Alice Nguyen", "alice99");
        assert!(row.matches_lowercase_query("nguyen"));
        assert!(row.matches_lowercase_query("alice9"));
        assert!(!row.matches_lowercase_query("bob"));
    }
}
