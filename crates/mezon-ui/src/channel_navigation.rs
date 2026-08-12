use gpui::App;
use mezon_store::{ChannelId, ChannelList, ClanId, MessagesStore};

use crate::router::{self, Route, Router};

pub fn navigate_after_thread_removed(
    cx: &mut App,
    clan_id: ClanId,
    removed_id: ChannelId,
    parent_id: ChannelId,
) {
    if parent_id.is_zero() {
        return;
    }
    let viewing_removed = match Router::global(cx).read(cx).route() {
        Route::Channel {
            channel_id: active, ..
        } => active == removed_id,
        Route::Thread { thread_id, .. } => thread_id == removed_id,
        _ => false,
    };
    if !viewing_removed {
        return;
    }
    if !parent_id.is_zero() {
        ChannelList::global(cx).update(cx, |list, cx| {
            list.clear_compose_draft(parent_id, cx);
        });
    }
    let parent_available = ChannelList::global(cx)
        .read(cx)
        .channel_in_clan(clan_id, parent_id);
    if !parent_available {
        return;
    }
    router::replace(
        cx,
        Route::Channel {
            clan_id,
            channel_id: parent_id,
        },
    );
    ChannelList::global(cx).update(cx, |list, cx| {
        if list.active_channel_id != Some(parent_id) {
            list.record_previous_channel(clan_id, parent_id, cx);
            list.select_channel(parent_id, cx);
        }
    });
    MessagesStore::global(cx).update(cx, |store, cx| {
        store.open_channel_in_clan(clan_id, parent_id, cx);
    });
}
