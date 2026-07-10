//! Central realtime router — the native analog of Zed's typed rpc routing
//! (`client.add_message_handler`, `channel_store.rs:190`).
//!
//! Instead of every store subscribing to one `broadcast<RealtimeEvent>` and filtering it with a
//! `match` (fan-out: each event cloned to every store, each store woken for events it ignores),
//! a single [`RealtimeDispatch`] owns the only subscription and **demuxes by event kind** to the
//! handlers registered for that kind. Handlers are bound to a [`gpui::WeakEntity`] and pruned
//! automatically when the owning store is dropped.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, Global, Task};
use mezon_client::{AppApi, RealtimeEvent};

/// Discriminant of the realtime events that a store can subscribe to. Mirrors the handled
/// [`RealtimeEvent`] variants so handlers register by kind (cf. routing rpc messages by type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RealtimeKind {
    ChannelMessage,
    MessageReaction,
    MessageTyping,
    ChannelPresence,
    StatusPresence,
    ChannelCreated,
    ChannelUpdated,
    ChannelDeleted,
    CategoryEvent,
    ClanUpdated,
    ClanDeleted,
    ClanEmoji,
    AddClanUser,
    UserClanRemoved,
    ClanProfileUpdated,
    SessionRefreshed,
    VoiceJoined,
    VoiceLeaved,
    VoiceReaction,
    MarkAsRead,
    LastPinMessage,
    UnpinMessage,
    LastSeenUpdated,
    UserChannelAdded,
    UserChannelRemoved,
    Notifications,
    AddFriend,
    RemoveFriend,
}

impl RealtimeKind {
    /// The kind of an event, or `None` for variants no store handles (skipped cheaply).
    fn of(event: &RealtimeEvent) -> Option<Self> {
        Some(match event {
            RealtimeEvent::ChannelMessage(_) => Self::ChannelMessage,
            RealtimeEvent::MessageReaction(_) => Self::MessageReaction,
            RealtimeEvent::MessageTyping(_) => Self::MessageTyping,
            RealtimeEvent::ChannelPresence(_) => Self::ChannelPresence,
            RealtimeEvent::StatusPresence(_) => Self::StatusPresence,
            RealtimeEvent::ChannelCreated(_) => Self::ChannelCreated,
            RealtimeEvent::ChannelUpdated(_) => Self::ChannelUpdated,
            RealtimeEvent::ChannelDeleted(_) => Self::ChannelDeleted,
            RealtimeEvent::CategoryEvent(_) => Self::CategoryEvent,
            RealtimeEvent::ClanUpdated(_) => Self::ClanUpdated,
            RealtimeEvent::ClanDeleted(_) => Self::ClanDeleted,
            RealtimeEvent::ClanEmoji(_) => Self::ClanEmoji,
            RealtimeEvent::AddClanUser(_) => Self::AddClanUser,
            RealtimeEvent::UserClanRemoved(_) => Self::UserClanRemoved,
            RealtimeEvent::ClanProfileUpdated(_) => Self::ClanProfileUpdated,
            RealtimeEvent::SessionRefreshed(_) => Self::SessionRefreshed,
            RealtimeEvent::VoiceJoined(_) => Self::VoiceJoined,
            RealtimeEvent::VoiceLeaved(_) => Self::VoiceLeaved,
            RealtimeEvent::VoiceReaction(_) => Self::VoiceReaction,
            RealtimeEvent::MarkAsRead(_) => Self::MarkAsRead,
            RealtimeEvent::LastPinMessage(_) => Self::LastPinMessage,
            RealtimeEvent::UnpinMessage(_) => Self::UnpinMessage,
            RealtimeEvent::LastSeenUpdated(_) => Self::LastSeenUpdated,
            RealtimeEvent::UserChannelAdded(_) => Self::UserChannelAdded,
            RealtimeEvent::UserChannelRemoved(_) => Self::UserChannelRemoved,
            RealtimeEvent::Notifications(_) => Self::Notifications,
            RealtimeEvent::AddFriend(_) => Self::AddFriend,
            RealtimeEvent::RemoveFriend(_) => Self::RemoveFriend,
            _ => return None,
        })
    }
}

type EventHandler = Box<dyn FnMut(&RealtimeEvent, &mut App) -> bool>;
type LaggedHandler = Box<dyn FnMut(&mut App) -> bool>;

/// Owns the single `AppApi` broadcast subscription and routes each event to the stores that
/// registered for its [`RealtimeKind`]. Registered as a [`Global`]; init it **before** the stores
/// that register handlers in their constructors.
pub struct RealtimeDispatch {
    handlers: HashMap<RealtimeKind, Vec<EventHandler>>,
    lagged: Vec<LaggedHandler>,
    _task: Task<()>,
}

struct GlobalRealtimeDispatch(Entity<RealtimeDispatch>);
impl Global for GlobalRealtimeDispatch {}

impl RealtimeDispatch {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalRealtimeDispatch(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalRealtimeDispatch>().0.clone()
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let task = cx.spawn(async move |this, cx| {
            let mut rx = api.subscribe();
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let Some(kind) = RealtimeKind::of(&event) else {
                            continue;
                        };
                        if this
                            .update(cx, |this, cx| this.dispatch(kind, &event, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if this
                            .update(cx, |this, cx| this.dispatch_lagged(cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Self {
            handlers: HashMap::new(),
            lagged: Vec::new(),
            _task: task,
        }
    }

    /// Run `handler` when an event of `kind` arrives, for as long as `entity` is alive.
    pub fn on<T: 'static>(
        &mut self,
        kind: RealtimeKind,
        entity: &Entity<T>,
        handler: impl Fn(&mut T, &RealtimeEvent, &mut Context<T>) + 'static,
    ) {
        let weak = entity.downgrade();
        self.handlers
            .entry(kind)
            .or_default()
            .push(Box::new(move |event, cx| {
                weak.update(cx, |store, cx| handler(store, event, cx))
                    .is_ok()
            }));
    }

    /// Run `handler` when the stream lagged (events were dropped) so the store can refetch.
    pub fn on_lagged<T: 'static>(
        &mut self,
        entity: &Entity<T>,
        handler: impl Fn(&mut T, &mut Context<T>) + 'static,
    ) {
        let weak = entity.downgrade();
        self.lagged.push(Box::new(move |cx| {
            weak.update(cx, |store, cx| handler(store, cx)).is_ok()
        }));
    }

    fn dispatch(&mut self, kind: RealtimeKind, event: &RealtimeEvent, cx: &mut Context<Self>) {
        if let Some(list) = self.handlers.get_mut(&kind) {
            list.retain_mut(|handler| handler(event, cx));
        }
    }

    fn dispatch_lagged(&mut self, cx: &mut Context<Self>) {
        tracing::warn!("realtime dispatch lagged — asking handlers to refetch");
        self.lagged.retain_mut(|handler| handler(cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_proto::{api, realtime};

    #[test]
    fn kind_of_maps_handled_variants() {
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::ChannelMessage(
                api::ChannelMessage::default()
            )),
            Some(RealtimeKind::ChannelMessage)
        );
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::ChannelDeleted(
                realtime::ChannelDeletedEvent::default()
            )),
            Some(RealtimeKind::ChannelDeleted)
        );
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::ClanUpdated(
                realtime::ClanUpdatedEvent::default()
            )),
            Some(RealtimeKind::ClanUpdated)
        );
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::SessionRefreshed(api::Session::default())),
            Some(RealtimeKind::SessionRefreshed)
        );
    }

    #[test]
    fn kind_of_maps_mark_as_read() {
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::MarkAsRead(realtime::MarkAsRead::default())),
            Some(RealtimeKind::MarkAsRead)
        );
    }

    #[test]
    fn kind_of_maps_user_channel_added() {
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::UserChannelAdded(
                realtime::UserChannelAdded::default()
            )),
            Some(RealtimeKind::UserChannelAdded)
        );
    }

    #[test]
    fn kind_of_maps_voice_reaction() {
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::VoiceReaction(
                realtime::VoiceReactionSend::default()
            )),
            Some(RealtimeKind::VoiceReaction)
        );
    }

    #[test]
    fn kind_of_returns_none_for_unhandled() {
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::CustomStatus(
                realtime::CustomStatusEvent::default()
            )),
            None
        );
    }

    #[test]
    fn kind_of_routes_friend_events() {
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::AddFriend(realtime::AddFriend::default())),
            Some(RealtimeKind::AddFriend)
        );
        assert_eq!(
            RealtimeKind::of(&RealtimeEvent::RemoveFriend(
                realtime::RemoveFriend::default()
            )),
            Some(RealtimeKind::RemoveFriend)
        );
    }
}
