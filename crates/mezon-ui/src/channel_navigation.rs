use gpui::App;
use mezon_store::{ChannelId, ChannelList, ClanId, ClanList, MessagesStore};

use crate::router::{self, Route, Router};

pub fn navigate_after_thread_removed(
    cx: &mut App,
    clan_id: ClanId,
    removed_id: ChannelId,
    parent_id: ChannelId,
) {
    let viewing_removed = match Router::global(cx).read(cx).route() {
        Route::Channel {
            channel_id: active, ..
        } => active == removed_id,
        Route::Thread { thread_id, .. } => thread_id == removed_id,
        Route::ChannelSettings {
            channel_id: active, ..
        }
        | Route::Canvas {
            channel_id: active, ..
        } => active == removed_id,
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
    let parent_in_clan = ChannelList::global(cx)
        .read(cx)
        .channel_in_clan(clan_id, parent_id);
    if !thread_removed_opens_parent(parent_id, parent_in_clan) {
        router::go_back(cx);
        return;
    }
    open_clan_channel(cx, clan_id, parent_id);
}

fn thread_removed_opens_parent(parent_id: ChannelId, parent_in_clan: bool) -> bool {
    !parent_id.is_zero() && parent_in_clan
}

pub fn navigate_after_channel_removed(cx: &mut App, clan_id: ClanId, removed_id: ChannelId) {
    let needs_nav = {
        let list = ChannelList::global(cx).read(cx);
        match Router::global(cx).read(cx).route() {
            Route::Channel {
                channel_id: active, ..
            } => {
                active == removed_id
                    || (list.is_locally_deleted(active)
                        && list.deleted_channel_parent(active) == Some(removed_id))
            }
            Route::Thread {
                channel_id: parent_id,
                thread_id,
                ..
            } => parent_id == removed_id || thread_id == removed_id,
            Route::ChannelSettings {
                channel_id: active, ..
            }
            | Route::Canvas {
                channel_id: active, ..
            } => {
                active == removed_id
                    || (list.is_locally_deleted(active)
                        && list.deleted_channel_parent(active) == Some(removed_id))
            }
            _ => false,
        }
    };
    if !needs_nav {
        return;
    }

    let target = fallback_channel_after_delete(cx, clan_id, removed_id);
    match target {
        Some(channel_id) => open_clan_channel(cx, clan_id, channel_id),
        None => router::replace(cx, Route::Chat),
    }
}

fn fallback_channel_after_delete(
    cx: &App,
    clan_id: ClanId,
    removed_id: ChannelId,
) -> Option<ChannelId> {
    let channels = ChannelList::global(cx).read(cx);
    let welcome = ClanList::global(cx)
        .read(cx)
        .welcome_channel_id(clan_id)
        .filter(|id| *id != removed_id && channels.channel_in_clan(clan_id, *id));
    let default_text = channels
        .default_channel_id(clan_id)
        .filter(|id| *id != removed_id);
    let remembered = channels
        .remembered_channel(clan_id)
        .filter(|id| *id != removed_id && channels.channel_in_clan(clan_id, *id));
    pick_fallback_channel_after_delete(welcome, default_text, remembered)
}

fn pick_fallback_channel_after_delete(
    welcome: Option<ChannelId>,
    default_text: Option<ChannelId>,
    remembered: Option<ChannelId>,
) -> Option<ChannelId> {
    welcome.or(default_text).or(remembered)
}

fn open_clan_channel(cx: &mut App, clan_id: ClanId, channel_id: ChannelId) {
    router::replace(
        cx,
        Route::Channel {
            clan_id,
            channel_id,
        },
    );
    ChannelList::global(cx).update(cx, |list, cx| {
        if list.active_channel_id != Some(channel_id) {
            list.record_previous_channel(clan_id, channel_id, cx);
            list.select_channel(channel_id, cx);
        }
    });
    MessagesStore::global(cx).update(cx, |store, cx| {
        store.open_channel_in_clan(clan_id, channel_id, cx);
    });
}

#[cfg(test)]
mod tests {
    use mezon_store::ChannelId;

    use super::pick_fallback_channel_after_delete;

    #[test]
    fn fallback_prefers_welcome_then_default_then_remembered() {
        assert_eq!(
            pick_fallback_channel_after_delete(
                Some(ChannelId(10)),
                Some(ChannelId(2)),
                Some(ChannelId(3)),
            ),
            Some(ChannelId(10))
        );
        assert_eq!(
            pick_fallback_channel_after_delete(None, Some(ChannelId(2)), Some(ChannelId(3)),),
            Some(ChannelId(2))
        );
        assert_eq!(
            pick_fallback_channel_after_delete(None, None, Some(ChannelId(3)),),
            Some(ChannelId(3))
        );
        assert_eq!(pick_fallback_channel_after_delete(None, None, None), None);
    }

    #[test]
    fn thread_removed_without_parent_goes_back() {
        assert!(!super::thread_removed_opens_parent(ChannelId(0), true));
        assert!(!super::thread_removed_opens_parent(ChannelId(9), false));
        assert!(super::thread_removed_opens_parent(ChannelId(9), true));
    }
}
