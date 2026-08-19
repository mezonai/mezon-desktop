use gpui::{App, SharedString};
use mezon_store::{
    CallPeer, CallPhase, CallStore, ChannelId, DirectKind, DirectMessageStore, Settings, UserId,
};

use crate::app::shell::Shell;
use crate::router::{Route, Router, navigate};

pub(crate) struct CallTarget {
    pub user: UserId,
    pub label: SharedString,
    pub avatar: SharedString,
    pub username: SharedString,
}

pub(crate) fn call_in_progress(cx: &App) -> bool {
    CallStore::try_global(cx).is_some_and(|store| store.read(cx).phase() != CallPhase::Idle)
}

pub(crate) fn current_dm_call_peer(cx: &App) -> Option<CallPeer> {
    let Route::DirectMessage { direct_id, .. } = Router::global(cx).read(cx).route() else {
        return None;
    };
    let store = DirectMessageStore::try_global(cx)?;
    let store = store.read(cx);
    let dm = store.find(direct_id)?;
    if dm.kind != DirectKind::Dm {
        return None;
    }
    let peer_user_id = dm.peer_user_id?;
    Some(CallPeer {
        user_id: peer_user_id.get(),
        channel_id: dm.id.get(),
        name: dm.label.clone(),
        avatar: (!dm.avatar.is_empty()).then(|| dm.avatar.clone()),
    })
}

pub(crate) fn call_current_dm(video: bool, cx: &mut App) {
    if warn_already_in_call(cx) {
        return;
    }
    let Some(peer) = current_dm_call_peer(cx) else {
        info_toast("common.comingSoon", cx);
        return;
    };
    CallStore::global(cx).update(cx, |store, cx| store.start_call(peer, video, cx));
}

pub(crate) fn call_user(
    target: CallTarget,
    video: bool,
    error_message: SharedString,
    cx: &mut App,
) {
    if warn_already_in_call(cx) {
        return;
    }
    let Some(store) = DirectMessageStore::try_global(cx) else {
        return;
    };
    let task = store.update(cx, |store, cx| {
        store.create_dm_with_user(
            target.user,
            target.label.to_string(),
            target.avatar.to_string(),
            target.username.to_string(),
            cx,
        )
    });
    cx.spawn(async move |cx| match task.await {
        Ok((channel_id, channel_type)) => {
            cx.update(|cx| {
                start_call_in(&target, channel_id, video, cx);
                navigate(
                    cx,
                    Route::DirectMessage {
                        direct_id: channel_id,
                        message_type: channel_type.to_string(),
                    },
                );
            });
        }
        Err(err) => {
            tracing::warn!("create DM before call failed: {err}");
            cx.update(|cx| {
                Shell::global(cx).update(cx, move |shell, cx| shell.error(error_message, cx));
            });
        }
    })
    .detach();
}

fn start_call_in(target: &CallTarget, channel: ChannelId, video: bool, cx: &mut App) {
    let peer = CallPeer {
        user_id: target.user.get(),
        channel_id: channel.get(),
        name: target.label.to_string(),
        avatar: (!target.avatar.is_empty()).then(|| target.avatar.to_string()),
    };
    CallStore::global(cx).update(cx, |store, cx| store.start_call(peer, video, cx));
}

fn warn_already_in_call(cx: &mut App) -> bool {
    if !call_in_progress(cx) {
        return false;
    }
    info_toast("channelTopbar.toastMessages.youAreOnAnotherCall", cx);
    true
}

fn info_toast(key: &'static str, cx: &mut App) {
    let locale = Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_else(|| "en".to_string());
    let message = mezon_i18n::t(&locale, key).to_string();
    Shell::global(cx).update(cx, move |shell, cx| shell.info(message, cx));
}
