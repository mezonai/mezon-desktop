use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::AppApi;
use mezon_client::NotificationOverride;
use mezon_client::notification_setting::{ChannelNotificationSetting, MUTE_UNMUTE};

use crate::AuthState;
use crate::ids::{ChannelId, ClanId};

pub use mezon_client::notification_setting::{
    MUTE_FOR_1_HOUR_SEC, MUTE_FOR_3_HOURS_SEC, MUTE_FOR_8_HOURS_SEC, MUTE_FOR_15_MINUTES_SEC,
    MUTE_FOR_24_HOURS_SEC, NOTIFICATION_ALL_MESSAGE, NOTIFICATION_MENTION_MESSAGE,
    NOTIFICATION_NOTHING_MESSAGE,
};
pub use mezon_client::notification_setting::{
    MUTE_INFINITY, MUTE_INFINITY as MUTE_FOREVER, NOTIFICATION_DEFAULT,
};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
pub enum NotificationSettingEvent {
    Changed(ChannelId),
}

struct GlobalNotificationSettingStore(Entity<NotificationSettingStore>);
impl Global for GlobalNotificationSettingStore {}

/// Per-channel notification overrides, mirroring React `notificationSettingChannel.slice`.
/// Clan and category defaults are cached alongside so [`ChannelNotificationSetting::is_muted`]
/// can resolve the same three-level fallback the React `MuteButton` uses.
pub struct NotificationSettingStore {
    settings: HashMap<ChannelId, ChannelNotificationSetting>,
    clan_defaults: HashMap<ClanId, i32>,
    category_defaults: HashMap<String, i32>,
    category_mute: HashMap<String, i64>,
    overrides: HashMap<ClanId, Vec<NotificationOverride>>,
    in_flight: HashMap<ChannelId, Task<()>>,
    clan_prefetch: HashMap<ClanId, Task<()>>,
    category_in_flight: HashMap<String, Task<()>>,
    overrides_in_flight: HashMap<ClanId, Task<()>>,
    api: Arc<AppApi>,
    auth_state: Entity<AuthState>,
    _channel_sub: Subscription,
    _auth_sub: Subscription,
}

impl EventEmitter<NotificationSettingEvent> for NotificationSettingStore {}

impl NotificationSettingStore {
    pub fn init(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| {
            let auth_sub = cx.observe(&auth_state, Self::on_auth_changed);
            let channel_sub = cx.subscribe(
                &crate::channel::ChannelList::global(cx),
                |this: &mut Self, _, event: &crate::channel::ChannelEvent, cx| {
                    if let crate::channel::ChannelEvent::ActiveChannelChanged(Some(channel_id)) =
                        event
                    {
                        let clan_id = crate::clan::ClanList::global(cx)
                            .read(cx)
                            .active_clan()
                            .map(|clan| clan.id);
                        match clan_id {
                            Some(clan_id) => this.ensure_channel(clan_id, *channel_id, cx),
                            None => this.ensure_loaded(*channel_id, cx),
                        }
                    }
                },
            );
            let this = Self {
                settings: HashMap::new(),
                clan_defaults: HashMap::new(),
                category_defaults: HashMap::new(),
                category_mute: HashMap::new(),
                overrides: HashMap::new(),
                in_flight: HashMap::new(),
                clan_prefetch: HashMap::new(),
                category_in_flight: HashMap::new(),
                overrides_in_flight: HashMap::new(),
                api,
                auth_state: auth_state.clone(),
                _channel_sub: channel_sub,
                _auth_sub: auth_sub,
            };
            let entity = cx.entity();
            crate::realtime::RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
                dispatch.on(
                    crate::realtime::RealtimeKind::NotifUserChannel,
                    &entity,
                    |store, event, cx| store.handle_realtime(event, cx),
                );
            });
            this
        });
        cx.set_global(GlobalNotificationSettingStore(entity.clone()));
        entity
    }

    fn on_auth_changed(this: &mut Self, _auth: Entity<AuthState>, cx: &mut Context<Self>) {
        if matches!(this.auth_state.read(cx), AuthState::NotAuthenticated) {
            this.reset(cx);
        }
    }

    fn handle_realtime(&mut self, event: &mezon_client::RealtimeEvent, cx: &mut Context<Self>) {
        let mezon_client::RealtimeEvent::NotifUserChannel(dto) = event else {
            return;
        };
        let channel_id = ChannelId(dto.channel_id);
        if channel_id.is_zero() {
            return;
        }
        let setting = ChannelNotificationSetting::from_api(dto);
        self.settings.insert(channel_id, setting);
        cx.emit(NotificationSettingEvent::Changed(channel_id));
        cx.notify();
        let muted = setting.is_time_muted(now_ms());
        crate::channel::ChannelList::global(cx).update(cx, |channels, cx| {
            channels.set_channel_muted_any_clan(channel_id, muted, cx);
        });
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalNotificationSettingStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalNotificationSettingStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.settings.clear();
        self.clan_defaults.clear();
        self.category_defaults.clear();
        self.category_mute.clear();
        self.overrides.clear();
        self.in_flight.clear();
        self.clan_prefetch.clear();
        self.category_in_flight.clear();
        self.overrides_in_flight.clear();
        cx.notify();
    }

    pub fn setting(&self, channel_id: ChannelId) -> Option<ChannelNotificationSetting> {
        self.settings.get(&channel_id).copied()
    }

    pub fn level(&self, channel_id: ChannelId) -> i32 {
        self.setting(channel_id)
            .map(|s| s.level)
            .unwrap_or(NOTIFICATION_DEFAULT)
    }

    pub fn is_muted(&self, channel_id: ChannelId, clan_id: ClanId, cx: &App) -> bool {
        let category_id = crate::channel::ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .and_then(|ch| ch.category_id.clone());
        let category_default = category_id
            .as_deref()
            .and_then(|id| self.category_defaults.get(id).copied());
        self.setting(channel_id)
            .unwrap_or_default()
            .is_muted(category_default, self.clan_defaults.get(&clan_id).copied())
    }

    pub fn muted_until_ms(&self, channel_id: ChannelId) -> Option<i64> {
        self.setting(channel_id)
            .and_then(|s| s.muted_until_ms(now_ms()))
    }

    pub fn is_time_muted(&self, channel_id: ChannelId) -> bool {
        self.setting(channel_id)
            .is_some_and(|s| s.is_time_muted(now_ms()))
    }

    pub fn clan_default(&self, clan_id: ClanId) -> Option<i32> {
        self.clan_defaults.get(&clan_id).copied()
    }

    pub fn set_clan_default(&mut self, clan_id: ClanId, level: i32) {
        self.clan_defaults.insert(clan_id, level);
    }

    pub fn category_default(&self, category_id: &str) -> Option<i32> {
        self.category_defaults.get(category_id).copied()
    }

    pub fn category_muted_until_ms(&self, category_id: &str) -> Option<i64> {
        let expiry = self.category_mute.get(category_id).copied()?;
        (expiry > now_ms()).then_some(expiry)
    }

    pub fn category_is_time_muted(&self, category_id: &str) -> bool {
        match self.category_mute.get(category_id).copied() {
            Some(expiry) => expiry == i64::from(MUTE_FOREVER) || expiry > now_ms(),
            None => false,
        }
    }

    /// Mirrors React `changeCurrentClan`, which batches `fetchMutedChannels` and
    /// `getDefaultNotificationClan` so the bell icon is correct before it is opened.
    pub fn prefetch_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if clan_id.is_zero() || self.clan_prefetch.contains_key(&clan_id) {
            return;
        }
        let api = self.api.clone();
        let task = cx.spawn(async move |this, cx| {
            let (muted, clan_default) = tokio::join!(
                api.list_muted_channels(clan_id.get()),
                api.get_notification_clan(clan_id.get()),
            );
            let _ = this.update(cx, |store, cx| {
                store.clan_prefetch.remove(&clan_id);
                match muted {
                    Ok(ids) => store.seed_muted_channels(&ids),
                    Err(e) => tracing::warn!(
                        clan_id = clan_id.get(),
                        "failed to list muted channels: {e}"
                    ),
                }
                match clan_default {
                    Ok(level) => store.set_clan_default(clan_id, level),
                    Err(e) => tracing::warn!(
                        clan_id = clan_id.get(),
                        "failed to load clan notification default: {e}"
                    ),
                }
                cx.notify();
            });
        });
        self.clan_prefetch.insert(clan_id, task);
    }

    /// `list_muted_channels` returns ids only, so it can establish that a channel is
    /// muted but not when the mute expires; the exact expiry arrives with
    /// [`Self::ensure_loaded`] for the channel actually being opened.
    fn seed_muted_channels(&mut self, muted_ids: &[String]) {
        for id in muted_ids {
            let Ok(raw) = id.parse::<i64>() else {
                continue;
            };
            let channel_id = ChannelId(raw);
            if channel_id.is_zero() {
                continue;
            }
            let entry = self.settings.entry(channel_id).or_default();
            entry.id = raw;
            if entry.mute_until_ms == 0 {
                entry.mute_until_ms = i64::from(MUTE_INFINITY);
            }
        }
    }

    /// Load the override for a channel unless a request is already in flight.
    pub fn ensure_loaded(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if channel_id.is_zero()
            || self.settings.contains_key(&channel_id)
            || self.in_flight.contains_key(&channel_id)
        {
            return;
        }
        let api = self.api.clone();
        let task = cx.spawn(async move |this, cx| {
            let fetched = api.get_notification_channel(channel_id.get()).await;
            let _ = this.update(cx, |store, cx| {
                store.in_flight.remove(&channel_id);
                match fetched {
                    Ok(setting) => {
                        store.settings.insert(channel_id, setting);
                        cx.emit(NotificationSettingEvent::Changed(channel_id));
                        cx.notify();
                    }
                    Err(e) => {
                        tracing::warn!(
                            channel_id = channel_id.get(),
                            "failed to load channel notification setting: {e}"
                        );
                    }
                }
            });
        });
        self.in_flight.insert(channel_id, task);
    }

    pub fn ensure_channel(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.ensure_loaded(channel_id, cx);
        let category_id = crate::channel::ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .and_then(|ch| ch.category_id.clone());
        if let Some(category_id) = category_id {
            self.ensure_category_loaded(category_id, cx);
        }
    }

    pub fn ensure_category(&mut self, category_id: String, cx: &mut Context<Self>) {
        self.ensure_category_loaded(category_id, cx);
    }

    fn ensure_category_loaded(&mut self, category_id: String, cx: &mut Context<Self>) {
        let Ok(raw) = category_id.parse::<i64>() else {
            return;
        };
        if raw == 0
            || self.category_defaults.contains_key(&category_id)
            || self.category_in_flight.contains_key(&category_id)
        {
            return;
        }
        let api = self.api.clone();
        let key = category_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let fetched = api.get_notification_category_setting(raw).await;
            let _ = this.update(cx, |store, cx| {
                store.category_in_flight.remove(&key);
                match fetched {
                    Ok(setting) => {
                        store.category_defaults.insert(key.clone(), setting.level);
                        store.category_mute.insert(key, setting.mute_until_ms);
                        cx.notify();
                    }
                    Err(e) => tracing::warn!(
                        category_id = raw,
                        "failed to load category notification default: {e}"
                    ),
                }
            });
        });
        self.category_in_flight.insert(category_id, task);
    }

    fn write_row_muted(clan_id: ClanId, channel_id: ChannelId, muted: bool, cx: &mut App) {
        crate::channel::ChannelList::global(cx).update(cx, |channels, cx| {
            channels.set_channel_muted(clan_id, channel_id, muted, cx);
        });
    }

    pub fn set_level(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        level: i32,
        cx: &mut Context<Self>,
    ) {
        let previous = self.setting(channel_id);
        let reset_to_default = level == NOTIFICATION_DEFAULT;
        let previous_row_muted = crate::channel::ChannelList::global(cx)
            .read(cx)
            .muted(clan_id, channel_id);
        let entry = self.settings.entry(channel_id).or_default();
        entry.level = level;
        if reset_to_default {
            entry.id = 0;
            entry.mute_until_ms = 0;
        } else {
            entry.id = channel_id.get();
        }
        cx.emit(NotificationSettingEvent::Changed(channel_id));
        cx.notify();
        if reset_to_default {
            Self::write_row_muted(clan_id, channel_id, false, cx);
        }

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = if reset_to_default {
                let unmuted = api
                    .set_mute_channel(channel_id.get(), MUTE_UNMUTE, clan_id.get())
                    .await;
                let deleted = api.delete_notification_channel(channel_id.get()).await;
                unmuted.and(deleted)
            } else {
                api.set_notification_channel_setting(channel_id.get(), level, clan_id.get())
                    .await
            };
            if let Err(e) = result {
                tracing::warn!(
                    channel_id = channel_id.get(),
                    "failed to save notification level, rolling back: {e}"
                );
                let _ = this.update(cx, |store, cx| {
                    store.restore(channel_id, previous);
                    if reset_to_default {
                        Self::write_row_muted(clan_id, channel_id, previous_row_muted, cx);
                    }
                    cx.emit(NotificationSettingEvent::Changed(channel_id));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn set_mute(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        mute_seconds: i32,
        cx: &mut Context<Self>,
    ) {
        let previous = self.setting(channel_id);
        let previous_row_muted = crate::channel::ChannelList::global(cx)
            .read(cx)
            .muted(clan_id, channel_id);
        let expiry = ChannelNotificationSetting::expiry_from_duration(now_ms(), mute_seconds);
        let entry = self.settings.entry(channel_id).or_default();
        entry.mute_until_ms = expiry;
        if mute_seconds == MUTE_UNMUTE {
            entry.id = 0;
        } else {
            entry.id = channel_id.get();
        }
        cx.emit(NotificationSettingEvent::Changed(channel_id));
        cx.notify();
        let now_row_muted = mute_seconds != MUTE_UNMUTE;
        Self::write_row_muted(clan_id, channel_id, now_row_muted, cx);

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .set_mute_channel(channel_id.get(), mute_seconds, clan_id.get())
                .await
            {
                tracing::warn!(
                    channel_id = channel_id.get(),
                    "failed to save channel mute, rolling back: {e}"
                );
                let _ = this.update(cx, |store, cx| {
                    store.restore(channel_id, previous);
                    Self::write_row_muted(clan_id, channel_id, previous_row_muted, cx);
                    cx.emit(NotificationSettingEvent::Changed(channel_id));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn mute_forever(&mut self, channel_id: ChannelId, clan_id: ClanId, cx: &mut Context<Self>) {
        self.set_mute(channel_id, clan_id, MUTE_INFINITY, cx);
    }

    pub fn unmute(&mut self, channel_id: ChannelId, clan_id: ClanId, cx: &mut Context<Self>) {
        self.set_mute(channel_id, clan_id, MUTE_UNMUTE, cx);
    }

    pub fn set_clan_level(&mut self, clan_id: ClanId, level: i32, cx: &mut Context<Self>) {
        if clan_id.is_zero() {
            return;
        }
        let previous = self.clan_defaults.get(&clan_id).copied();
        self.clan_defaults.insert(clan_id, level);
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .set_notification_clan_setting(clan_id.get(), level)
                .await
            {
                tracing::warn!(
                    clan_id = clan_id.get(),
                    "failed to save clan notification level, rolling back: {e}"
                );
                let _ = this.update(cx, |store, cx| {
                    match previous {
                        Some(level) => {
                            store.clan_defaults.insert(clan_id, level);
                        }
                        None => {
                            store.clan_defaults.remove(&clan_id);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn set_category_level(
        &mut self,
        category_id: String,
        clan_id: ClanId,
        level: i32,
        cx: &mut Context<Self>,
    ) {
        let Ok(raw) = category_id.parse::<i64>() else {
            return;
        };
        if raw == 0 {
            return;
        }
        let previous = self.category_defaults.get(&category_id).copied();
        self.category_defaults.insert(category_id.clone(), level);
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .set_notification_category_setting(raw, level, clan_id.get())
                .await
            {
                tracing::warn!(
                    category_id = raw,
                    "failed to save category notification level, rolling back: {e}"
                );
                let _ = this.update(cx, |store, cx| {
                    match previous {
                        Some(level) => {
                            store.category_defaults.insert(category_id, level);
                        }
                        None => {
                            store.category_defaults.remove(&category_id);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn set_category_mute(
        &mut self,
        category_id: String,
        clan_id: ClanId,
        mute_seconds: i32,
        cx: &mut Context<Self>,
    ) {
        let Ok(raw) = category_id.parse::<i64>() else {
            return;
        };
        if raw == 0 {
            return;
        }
        let previous = self.category_mute.get(&category_id).copied();
        let expiry = ChannelNotificationSetting::expiry_from_duration(now_ms(), mute_seconds);
        self.category_mute.insert(category_id.clone(), expiry);
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .set_mute_category(raw, mute_seconds, clan_id.get())
                .await
            {
                tracing::warn!(
                    category_id = raw,
                    "failed to save category mute, rolling back: {e}"
                );
                let _ = this.update(cx, |store, cx| {
                    match previous {
                        Some(v) => {
                            store.category_mute.insert(category_id, v);
                        }
                        None => {
                            store.category_mute.remove(&category_id);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn mute_category_forever(
        &mut self,
        category_id: String,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        self.set_category_mute(category_id, clan_id, MUTE_FOREVER, cx);
    }

    pub fn unmute_category(
        &mut self,
        category_id: String,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        self.set_category_mute(category_id, clan_id, MUTE_UNMUTE, cx);
    }

    pub fn overrides(&self, clan_id: ClanId) -> Vec<NotificationOverride> {
        self.overrides.get(&clan_id).cloned().unwrap_or_default()
    }

    pub fn fetch_overrides(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if clan_id.is_zero() || self.overrides_in_flight.contains_key(&clan_id) {
            return;
        }
        let api = self.api.clone();
        let task = cx.spawn(async move |this, cx| {
            let fetched = api
                .get_channel_category_noti_settings_list(clan_id.get())
                .await;
            let _ = this.update(cx, |store, cx| {
                store.overrides_in_flight.remove(&clan_id);
                match fetched {
                    Ok(list) => {
                        store.seed_overrides(&list);
                        store.overrides.insert(clan_id, list);
                        cx.notify();
                    }
                    Err(e) => tracing::warn!(
                        clan_id = clan_id.get(),
                        "failed to load notification overrides: {e}"
                    ),
                }
            });
        });
        self.overrides_in_flight.insert(clan_id, task);
    }

    fn seed_overrides(&mut self, list: &[NotificationOverride]) {
        let mute_expiry = |muted: bool| if muted { i64::from(MUTE_FOREVER) } else { 0 };
        for o in list {
            if o.is_category {
                let key = o.id.to_string();
                self.category_defaults.insert(key.clone(), o.level);
                self.category_mute
                    .entry(key)
                    .or_insert_with(|| mute_expiry(o.muted));
            } else {
                let channel_id = ChannelId(o.id);
                let existed = self.settings.contains_key(&channel_id);
                let entry = self.settings.entry(channel_id).or_default();
                entry.id = o.id;
                entry.level = o.level;
                if !existed {
                    entry.mute_until_ms = mute_expiry(o.muted);
                }
            }
        }
    }

    pub fn add_channel_override(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let level = self
            .clan_defaults
            .get(&clan_id)
            .copied()
            .unwrap_or(NOTIFICATION_ALL_MESSAGE);
        self.set_level(channel_id, clan_id, level, cx);
        let list = self.overrides.entry(clan_id).or_default();
        if !list
            .iter()
            .any(|o| o.id == channel_id.get() && !o.is_category)
        {
            list.push(NotificationOverride {
                id: channel_id.get(),
                label,
                is_category: false,
                level,
                muted: false,
            });
        }
        cx.notify();
    }

    pub fn add_category_override(
        &mut self,
        category_id: String,
        clan_id: ClanId,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let Ok(raw) = category_id.parse::<i64>() else {
            return;
        };
        let level = self
            .clan_defaults
            .get(&clan_id)
            .copied()
            .unwrap_or(NOTIFICATION_ALL_MESSAGE);
        self.set_category_level(category_id, clan_id, level, cx);
        let list = self.overrides.entry(clan_id).or_default();
        if !list.iter().any(|o| o.id == raw && o.is_category) {
            list.push(NotificationOverride {
                id: raw,
                label,
                is_category: true,
                level,
                muted: false,
            });
        }
        cx.notify();
    }

    pub fn delete_channel_override(
        &mut self,
        channel_id: ChannelId,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        self.settings.remove(&channel_id);
        if let Some(list) = self.overrides.get_mut(&clan_id) {
            list.retain(|o| o.id != channel_id.get() || o.is_category);
        }
        Self::write_row_muted(clan_id, channel_id, false, cx);
        cx.emit(NotificationSettingEvent::Changed(channel_id));
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            let _ = api
                .set_mute_channel(channel_id.get(), MUTE_UNMUTE, clan_id.get())
                .await;
            if let Err(e) = api.delete_notification_channel(channel_id.get()).await {
                tracing::warn!(
                    channel_id = channel_id.get(),
                    "failed to delete channel override: {e}"
                );
            }
        })
        .detach();
    }

    pub fn delete_category_override(
        &mut self,
        category_id: String,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) {
        let Ok(raw) = category_id.parse::<i64>() else {
            return;
        };
        self.category_defaults.remove(&category_id);
        self.category_mute.remove(&category_id);
        if let Some(list) = self.overrides.get_mut(&clan_id) {
            list.retain(|o| o.id != raw || !o.is_category);
        }
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            let _ = api.set_mute_category(raw, MUTE_UNMUTE, clan_id.get()).await;
            if let Err(e) = api.delete_notification_category_setting(raw).await {
                tracing::warn!(category_id = raw, "failed to delete category override: {e}");
            }
        })
        .detach();
    }

    fn restore(&mut self, channel_id: ChannelId, previous: Option<ChannelNotificationSetting>) {
        match previous {
            Some(setting) => {
                self.settings.insert(channel_id, setting);
            }
            None => {
                self.settings.remove(&channel_id);
            }
        }
    }
}
