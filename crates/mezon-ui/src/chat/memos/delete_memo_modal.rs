use gpui::{App, Window};
use mezon_store::UserId;

use crate::app::shell::Shell;

pub fn confirm_delete_memo(
    creator_id: UserId,
    memo_id: i64,
    locale: &str,
    window: &mut Window,
    cx: &mut App,
) {
    Shell::global(cx).update(cx, |shell, cx| {
        shell.confirm_delete_memo(creator_id, memo_id, locale, window, cx);
    });
}
