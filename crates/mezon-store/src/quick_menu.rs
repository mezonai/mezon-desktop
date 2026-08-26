use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, Global, SharedString, Subscription, Task};
pub use mezon_client::transport::{QUICK_MENU_TYPE_FLASH, QUICK_MENU_TYPE_QUICK};
use mezon_client::{AppApi, ConnectionStatus};
use regex::Regex;

use crate::channel::{ChannelEvent, ChannelList};
use crate::emoji::generate_snowflake_id;
use crate::ids::{ChannelId, ClanId};
use crate::messages::{MessagesEvent, MessagesStore};

const CACHE_TTL: Duration = Duration::from_secs(300);
pub const QUICK_MENU_NAME_MAX_RUNES: usize = 64;
pub const QUICK_MENU_ACTION_MSG_MAX_BYTES: usize = 512;

static MENU_NAME_CHAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\p{L}\p{N}\p{So}_\ \-\.\+]$").expect("quick menu name char regex")
});

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
    reload_seq: HashMap<(ChannelId, i32), u64>,
    api: Arc<AppApi>,
    _channel_sub: Subscription,
    _messages_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalQuickMenuStore(Entity<QuickMenuStore>);
impl Global for GlobalQuickMenuStore {}

pub fn is_valid_menu_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let Some(first) = name.chars().next() else {
        return false;
    };
    if first == '_' || first == '-' {
        return false;
    }
    if name.chars().count() > QUICK_MENU_NAME_MAX_RUNES {
        return false;
    }
    name.chars().all(is_valid_menu_name_char)
}

pub fn is_valid_action_msg(action_msg: &str) -> bool {
    !action_msg.is_empty() && action_msg.len() <= QUICK_MENU_ACTION_MSG_MAX_BYTES
}

pub fn name_exists(items: &[QuickMenuItem], name: &str, exclude_id: Option<i64>) -> bool {
    items
        .iter()
        .any(|item| exclude_id.is_none_or(|id| item.id != id) && item.menu_name.as_ref() == name)
}

fn is_valid_menu_name_char(c: char) -> bool {
    if is_quick_menu_emoji(c) {
        return true;
    }
    let mut buf = [0u8; 4];
    MENU_NAME_CHAR.is_match(c.encode_utf8(&mut buf))
}

fn is_quick_menu_emoji(c: char) -> bool {
    matches!(
        c as u32,
        0x1F600..=0x1F64F
            | 0x1F300..=0x1F5FF
            | 0x1F680..=0x1F6FF
            | 0x1F700..=0x1F77F
            | 0x1F780..=0x1F7FF
            | 0x1F800..=0x1F8FF
            | 0x1F900..=0x1F9FF
            | 0x1FA00..=0x1FA6F
            | 0x1FA70..=0x1FAFF
    )
}

fn apply_add(
    by_channel: &mut HashMap<ChannelId, HashMap<i32, Vec<QuickMenuItem>>>,
    channel_id: ChannelId,
    item: QuickMenuItem,
) {
    by_channel
        .entry(channel_id)
        .or_default()
        .entry(item.menu_type)
        .or_default()
        .push(item);
}

fn apply_update(
    by_channel: &mut HashMap<ChannelId, HashMap<i32, Vec<QuickMenuItem>>>,
    channel_id: ChannelId,
    item: QuickMenuItem,
) {
    let Some(items) = by_channel
        .get_mut(&channel_id)
        .and_then(|by_type| by_type.get_mut(&item.menu_type))
    else {
        return;
    };
    if let Some(existing) = items.iter_mut().find(|existing| existing.id == item.id) {
        *existing = item;
    }
}

fn apply_delete(
    by_channel: &mut HashMap<ChannelId, HashMap<i32, Vec<QuickMenuItem>>>,
    channel_id: ChannelId,
    id: i64,
) {
    let Some(by_type) = by_channel.get_mut(&channel_id) else {
        return;
    };
    for items in by_type.values_mut() {
        items.retain(|item| item.id != id);
    }
}

impl QuickMenuStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalQuickMenuStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalQuickMenuStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalQuickMenuStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.by_channel.clear();
        self.fetched_at.clear();
        self.loading.clear();
        self.reload_seq.clear();
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
            reload_seq: HashMap::new(),
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

    pub fn is_loading(&self, channel_id: ChannelId, menu_type: i32) -> bool {
        self.loading
            .get(&(channel_id, menu_type))
            .copied()
            .unwrap_or(false)
    }

    pub fn has_any(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|by_type| by_type.values().any(|items| !items.is_empty()))
    }

    pub fn has_items(&self, channel_id: ChannelId, menu_type: i32) -> bool {
        !self.items(channel_id, menu_type).is_empty()
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
        self.reload(channel_id, menu_type, cx);
    }

    pub fn refresh(&mut self, channel_id: ChannelId, menu_type: i32, cx: &mut Context<Self>) {
        self.fetched_at.remove(&(channel_id, menu_type));
        self.reload(channel_id, menu_type, cx);
    }

    fn reload(&mut self, channel_id: ChannelId, menu_type: i32, cx: &mut Context<Self>) {
        let key = (channel_id, menu_type);
        let generation = {
            let entry = self.reload_seq.entry(key).or_insert(0);
            *entry += 1;
            *entry
        };
        self.loading.insert(key, true);
        let api = self.api.clone();
        let channel_num = channel_id.get();
        cx.spawn(async move |this, cx| {
            let result = api.list_quick_menu_access(channel_num, menu_type).await;
            let _ = this.update(cx, |this, cx| {
                if this.reload_seq.get(&key) != Some(&generation) {
                    return;
                }
                this.loading.remove(&key);
                cx.notify();
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
                    }
                    Err(e) => tracing::error!("list_quick_menu_access failed: {e}"),
                }
            });
        })
        .detach();
    }

    pub fn add(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        menu_name: String,
        action_msg: String,
        menu_type: i32,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let id = generate_snowflake_id();
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            api.add_quick_menu_access(
                id,
                clan_id.get(),
                channel_id.get(),
                &menu_name,
                &action_msg,
                menu_type,
            )
            .await
            .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                apply_add(
                    &mut this.by_channel,
                    channel_id,
                    QuickMenuItem {
                        id,
                        menu_name: menu_name.into(),
                        action_msg: action_msg.into(),
                        menu_type,
                    },
                );
                this.refresh(channel_id, menu_type, cx);
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn update(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        id: i64,
        menu_name: String,
        action_msg: String,
        menu_type: i32,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            api.update_quick_menu_access(
                id,
                clan_id.get(),
                channel_id.get(),
                &menu_name,
                &action_msg,
                menu_type,
            )
            .await
            .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                apply_update(
                    &mut this.by_channel,
                    channel_id,
                    QuickMenuItem {
                        id,
                        menu_name: menu_name.into(),
                        action_msg: action_msg.into(),
                        menu_type,
                    },
                );
                this.refresh(channel_id, menu_type, cx);
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn delete(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            api.delete_quick_menu_access(id, clan_id.get())
                .await
                .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                apply_delete(&mut this.by_channel, channel_id, id);
                this.fetched_at.remove(&(channel_id, QUICK_MENU_TYPE_FLASH));
                this.fetched_at.remove(&(channel_id, QUICK_MENU_TYPE_QUICK));
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, name: &str, action: &str, menu_type: i32) -> QuickMenuItem {
        QuickMenuItem {
            id,
            menu_name: name.into(),
            action_msg: action.into(),
            menu_type,
        }
    }

    #[test]
    fn menu_name_rejects_empty_leading_underscore_dash_and_over_64() {
        assert!(!is_valid_menu_name(""));
        assert!(!is_valid_menu_name("_hello"));
        assert!(!is_valid_menu_name("-hello"));
        assert!(is_valid_menu_name("hello"));
        assert!(is_valid_menu_name(&"a".repeat(64)));
        assert!(!is_valid_menu_name(&"a".repeat(65)));
    }

    #[test]
    fn menu_name_allows_letters_numbers_space_underscore_dash_dot_plus() {
        assert!(is_valid_menu_name("hello world"));
        assert!(is_valid_menu_name("hello_world"));
        assert!(is_valid_menu_name("hello-world"));
        assert!(is_valid_menu_name("hello.world"));
        assert!(is_valid_menu_name("hello+world"));
        assert!(is_valid_menu_name("hello2"));
        assert!(!is_valid_menu_name("hello!"));
        assert!(!is_valid_menu_name("hello/"));
    }

    #[test]
    fn menu_name_allows_emoji_in_server_ranges() {
        assert!(is_valid_menu_name("hello😀"));
        assert!(is_valid_menu_name("😀hello"));
    }

    #[test]
    fn action_msg_requires_non_empty_and_max_512_bytes() {
        assert!(!is_valid_action_msg(""));
        assert!(is_valid_action_msg("hi"));
        assert!(is_valid_action_msg(&"a".repeat(512)));
        assert!(!is_valid_action_msg(&"a".repeat(513)));
    }

    #[test]
    fn name_exists_is_case_sensitive_and_honors_exclude_id() {
        let items = vec![
            item(1, "hello", "msg", QUICK_MENU_TYPE_FLASH),
            item(2, "other", "msg", QUICK_MENU_TYPE_FLASH),
        ];
        assert!(name_exists(&items, "hello", None));
        assert!(!name_exists(&items, "Hello", None));
        assert!(!name_exists(&items, "hello", Some(1)));
        assert!(name_exists(&items, "hello", Some(2)));
        assert!(!name_exists(&items, "missing", None));
    }

    #[test]
    fn apply_add_update_delete_keep_type_buckets() {
        let channel = ChannelId(10);
        let mut by_channel = HashMap::new();
        apply_add(
            &mut by_channel,
            channel,
            item(1, "flash", "hi", QUICK_MENU_TYPE_FLASH),
        );
        apply_add(
            &mut by_channel,
            channel,
            item(2, "menu", "bot_event", QUICK_MENU_TYPE_QUICK),
        );
        assert_eq!(by_channel[&channel][&QUICK_MENU_TYPE_FLASH].len(), 1);
        assert_eq!(by_channel[&channel][&QUICK_MENU_TYPE_QUICK].len(), 1);

        apply_update(
            &mut by_channel,
            channel,
            item(1, "flash2", "hello", QUICK_MENU_TYPE_FLASH),
        );
        assert_eq!(
            by_channel[&channel][&QUICK_MENU_TYPE_FLASH][0]
                .menu_name
                .as_ref(),
            "flash2"
        );
        assert_eq!(
            by_channel[&channel][&QUICK_MENU_TYPE_FLASH][0]
                .action_msg
                .as_ref(),
            "hello"
        );

        apply_delete(&mut by_channel, channel, 1);
        assert!(by_channel[&channel][&QUICK_MENU_TYPE_FLASH].is_empty());
        assert_eq!(by_channel[&channel][&QUICK_MENU_TYPE_QUICK].len(), 1);

        apply_delete(&mut by_channel, channel, 2);
        assert!(by_channel[&channel][&QUICK_MENU_TYPE_QUICK].is_empty());
    }
}
