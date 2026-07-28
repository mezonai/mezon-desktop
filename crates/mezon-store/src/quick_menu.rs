use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, Global, SharedString, Subscription, Task};
use mezon_client::transport::{QUICK_MENU_TYPE_FLASH, QUICK_MENU_TYPE_QUICK};
use mezon_client::{AppApi, ConnectionStatus};

use crate::channel::{ChannelEvent, ChannelList};
use crate::ids::ChannelId;
use crate::messages::{MessagesEvent, MessagesStore};

const CACHE_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct QuickMenuItem {
    pub id: i64,
    pub menu_name: SharedString,
    pub action_msg: SharedString,
    pub menu_type: i32,
}

pub struct QuickMenuStore {
    by_channel: HashMap<ChannelId, HashMap<i32, Vec<QuickMenuItem>>>,
    fetched_at: HashMap<(ChannelId, i32), Instant>,
    loading: HashMap<(ChannelId, i32), bool>,
    api: Arc<AppApi>,
    _channel_sub: Subscription,
    _messages_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalQuickMenuStore(Entity<QuickMenuStore>);
impl Global for GlobalQuickMenuStore {}

impl QuickMenuStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalQuickMenuStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalQuickMenuStore>().0.clone()
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.by_channel.clear();
        self.fetched_at.clear();
        self.loading.clear();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _channel, event, cx| {
            if let ChannelEvent::ActiveChannelChanged(channel_id) = event {
                this.on_channel_changed(*channel_id, cx);
            }
        });
        let messages_sub = cx.subscribe(&MessagesStore::global(cx), |this, _store, event, cx| {
            if matches!(event, MessagesEvent::Reset { .. }) {
                this.reset(cx);
            }
        });
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            by_channel: HashMap::new(),
            fetched_at: HashMap::new(),
            loading: HashMap::new(),
            api,
            _channel_sub: channel_sub,
            _messages_sub: messages_sub,
            _conn_watch: conn_watch,
        }
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = false;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected && !was_connected {
                    was_connected = true;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(channel_id) =
                            MessagesStore::global(cx).read(cx).active_channel_id()
                        {
                            this.ensure_loaded(channel_id, QUICK_MENU_TYPE_QUICK, cx);
                            this.ensure_loaded(channel_id, QUICK_MENU_TYPE_FLASH, cx);
                        }
                    });
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    fn on_channel_changed(&mut self, channel_id: Option<ChannelId>, cx: &mut Context<Self>) {
        let Some(channel_id) = channel_id else {
            return;
        };
        self.ensure_loaded(channel_id, QUICK_MENU_TYPE_QUICK, cx);
        self.ensure_loaded(channel_id, QUICK_MENU_TYPE_FLASH, cx);
    }

    pub fn items(&self, channel_id: ChannelId, menu_type: i32) -> &[QuickMenuItem] {
        self.by_channel
            .get(&channel_id)
            .and_then(|by_type| by_type.get(&menu_type))
            .map(|items| items.as_slice())
            .unwrap_or(&[])
    }

    pub fn ensure_loaded(&mut self, channel_id: ChannelId, menu_type: i32, cx: &mut Context<Self>) {
        let key = (channel_id, menu_type);
        if self.loading.get(&key).copied().unwrap_or(false) {
            return;
        }
        if self
            .fetched_at
            .get(&key)
            .is_some_and(|t| t.elapsed() < CACHE_TTL)
        {
            return;
        }
        self.loading.insert(key, true);
        let api = self.api.clone();
        let channel_num = channel_id.get();
        cx.spawn(async move |this, cx| {
            let result = api.list_quick_menu_access(channel_num, menu_type).await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&key);
                match result {
                    Ok(items) => {
                        this.fetched_at.insert(key, Instant::now());
                        let mapped = items
                            .into_iter()
                            .map(|item| QuickMenuItem {
                                id: item.id,
                                menu_name: item.menu_name.into(),
                                action_msg: item.action_msg.into(),
                                menu_type: item.menu_type,
                            })
                            .collect();
                        this.by_channel
                            .entry(channel_id)
                            .or_default()
                            .insert(menu_type, mapped);
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_quick_menu_access failed: {e}"),
                }
            });
        })
        .detach();
    }
}
